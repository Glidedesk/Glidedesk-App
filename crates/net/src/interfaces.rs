//! Network interface listing and bind-address resolution.
//! Nexpingdesk runs on IPv4 only: IPv6 addresses are never listed or bound.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use nexpingdesk_config::BindMode;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceKind {
    Ethernet,
    Wifi,
    Vpn,
    Loopback,
    Virtual,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IfAddr {
    pub ip: IpAddr,
    pub prefix: u8,
}

impl IfAddr {
    /// Does `other` share this address's subnet?
    #[must_use]
    pub fn same_subnet(&self, other: IpAddr) -> bool {
        match (self.ip, other) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(self.prefix.min(32))) };
                u32::from(a) & mask == u32::from(b) & mask
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NetInterface {
    /// OS name (`en0`, adapter GUID/alias) — what the config stores.
    pub name: String,
    /// Human name ("Wi-Fi", "Ethernet 2").
    pub friendly_name: String,
    pub kind: InterfaceKind,
    pub up: bool,
    pub addrs: Vec<IfAddr>,
    pub mac: Option<String>,
}

fn kind_of(i: &netdev::Interface) -> InterfaceKind {
    if i.is_loopback() {
        return InterfaceKind::Loopback;
    }
    let t = format!("{:?}", i.if_type).to_ascii_lowercase();
    let name = i.name.to_ascii_lowercase();
    let desc = i.description.clone().unwrap_or_default().to_ascii_lowercase();
    if t.contains("wireless") || t.contains("wifi") || desc.contains("wi-fi") || desc.contains("wireless") {
        InterfaceKind::Wifi
    } else if t.contains("tunnel")
        || t.contains("ppp")
        || name.starts_with("utun")
        || name.starts_with("tun")
        || name.starts_with("wg")
        || desc.contains("vpn")
        || desc.contains("wireguard")
        || desc.contains("tap-")
    {
        InterfaceKind::Vpn
    } else if name.starts_with("bridge")
        || name.starts_with("vmnet")
        || name.starts_with("veth")
        || name.starts_with("docker")
        || desc.contains("hyper-v")
        || desc.contains("virtual")
    {
        InterfaceKind::Virtual
    } else if t.contains("ethernet") {
        InterfaceKind::Ethernet
    } else {
        InterfaceKind::Other
    }
}

/// All interfaces with at least one IPv4 address.
#[must_use]
pub fn list_interfaces() -> Vec<NetInterface> {
    netdev::get_interfaces()
        .into_iter()
        .map(|i| {
            let addrs: Vec<IfAddr> =
                i.ipv4.iter().map(|n| IfAddr { ip: IpAddr::V4(n.addr()), prefix: n.prefix_len() }).collect();
            NetInterface {
                friendly_name: i.friendly_name.clone().unwrap_or_else(|| i.name.clone()),
                kind: kind_of(&i),
                up: i.is_up(),
                mac: i.mac_addr.map(|m| m.to_string()),
                name: i.name.clone(),
                addrs,
            }
        })
        .filter(|i| !i.addrs.is_empty())
        .collect()
}

/// Where to listen for a bind configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindPlan {
    /// Every IPv4 address: `0.0.0.0:port`.
    Wildcard(u16),
    /// These addresses (may be empty while every chosen network is off).
    Addrs(Vec<SocketAddr>),
}

/// Resolves the configured mode to concrete IPv4 sockets. Chosen interfaces
/// that are off (or have no IPv4 address) are skipped; see [`unavailable`].
#[must_use]
pub fn resolve_bind(
    mode: BindMode,
    interfaces: &[String],
    addresses: &[IpAddr],
    port: u16,
    all: &[NetInterface],
) -> BindPlan {
    let v4 = |ip: &IpAddr| ip.is_ipv4() && !ip.is_unspecified();
    // Sorted: the OS lists interfaces in no fixed order, and the plan is
    // compared between scans to notice networks going off or on.
    let sorted = |mut v: Vec<SocketAddr>| {
        v.sort_unstable();
        v.dedup();
        v
    };
    match mode {
        BindMode::All => BindPlan::Wildcard(port),
        BindMode::Interfaces => BindPlan::Addrs(sorted(
            all.iter()
                .filter(|i| interfaces.contains(&i.name) && i.up)
                .flat_map(|i| i.addrs.iter().filter(|a| v4(&a.ip)).map(|a| SocketAddr::new(a.ip, port)))
                .collect(),
        )),
        BindMode::Addresses => BindPlan::Addrs(sorted(
            addresses
                .iter()
                .filter(|ip| v4(ip) && all.iter().any(|i| i.up && i.addrs.iter().any(|a| a.ip == **ip)))
                .map(|ip| SocketAddr::new(*ip, port))
                .collect(),
        )),
    }
}

/// Chosen interfaces / addresses that can't be used right now, described for
/// the user ("Wi-Fi (en0) is off"). Sharing goes on over the others.
#[must_use]
pub fn unavailable(mode: BindMode, interfaces: &[String], addresses: &[IpAddr], all: &[NetInterface]) -> Vec<String> {
    match mode {
        BindMode::All => Vec::new(),
        BindMode::Interfaces => interfaces
            .iter()
            .filter_map(|name| match all.iter().find(|i| &i.name == name) {
                None => Some(format!("network {name} is not connected")),
                Some(i) if !i.up => Some(format!("{} is off", label(i))),
                Some(i) if !i.addrs.iter().any(|a| a.ip.is_ipv4()) => Some(format!("{} has no IPv4 address", label(i))),
                Some(_) => None,
            })
            .collect(),
        BindMode::Addresses => addresses
            .iter()
            .filter_map(|ip| {
                if !ip.is_ipv4() {
                    return Some(format!("{ip} is not an IPv4 address"));
                }
                let owner = all.iter().find(|i| i.addrs.iter().any(|a| a.ip == *ip));
                match owner {
                    None => Some(format!("{ip} is not on this computer right now")),
                    Some(i) if !i.up => Some(format!("{ip} ({}) is off", label(i))),
                    Some(_) => None,
                }
            })
            .collect(),
    }
}

/// "Wi-Fi (en0)", or just "en0" when it has no other name.
fn label(i: &NetInterface) -> String {
    if i.friendly_name.is_empty() || i.friendly_name == i.name {
        i.name.clone()
    } else {
        format!("{} ({})", i.friendly_name, i.name)
    }
}

/// Addresses of the interfaces we are bound to (for "same subnet only" and mDNS).
#[must_use]
pub fn bound_ifaddrs(plan: &BindPlan, all: &[NetInterface]) -> Vec<IfAddr> {
    match plan {
        BindPlan::Wildcard(_) => all.iter().filter(|i| i.up).flat_map(|i| i.addrs.iter().cloned()).collect(),
        BindPlan::Addrs(socks) => {
            all.iter().flat_map(|i| i.addrs.iter()).filter(|a| socks.iter().any(|s| s.ip() == a.ip)).cloned().collect()
        }
    }
}

/// The IPv4 wildcard address.
#[must_use]
pub const fn unspecified() -> IpAddr {
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, ips: &[(&str, u8)]) -> NetInterface {
        NetInterface {
            name: name.into(),
            friendly_name: name.into(),
            kind: InterfaceKind::Ethernet,
            up: true,
            addrs: ips.iter().map(|(ip, p)| IfAddr { ip: ip.parse().unwrap(), prefix: *p }).collect(),
            mac: None,
        }
    }

    #[test]
    fn resolves_modes() {
        let all = vec![iface("en0", &[("192.168.1.10", 24)]), iface("en1", &[("10.0.0.2", 8)])];
        assert_eq!(resolve_bind(BindMode::All, &[], &[], 5, &all), BindPlan::Wildcard(5));
        let plan = resolve_bind(BindMode::Interfaces, &["en0".into()], &[], 5, &all);
        assert_eq!(plan, BindPlan::Addrs(vec!["192.168.1.10:5".parse().unwrap()]));
        let plan = resolve_bind(BindMode::Addresses, &[], &["10.0.0.2".parse().unwrap()], 5, &all);
        assert_eq!(plan, BindPlan::Addrs(vec!["10.0.0.2:5".parse().unwrap()]));
    }

    #[test]
    fn an_interface_that_is_off_is_skipped_and_reported() {
        let mut wifi = iface("en0", &[("192.168.1.10", 24)]);
        wifi.friendly_name = "Wi-Fi".into();
        wifi.up = false;
        let all = vec![wifi, iface("en5", &[("192.168.1.11", 24)])];
        let chosen = ["en0".to_owned(), "en5".to_owned(), "en9".to_owned()];
        let plan = resolve_bind(BindMode::Interfaces, &chosen, &[], 5, &all);
        assert_eq!(plan, BindPlan::Addrs(vec!["192.168.1.11:5".parse().unwrap()]));
        let why = unavailable(BindMode::Interfaces, &chosen, &[], &all);
        assert_eq!(why, vec!["Wi-Fi (en0) is off".to_owned(), "network en9 is not connected".to_owned()]);
        let ips = ["192.168.1.10".parse().unwrap(), "192.168.1.11".parse().unwrap(), "10.9.9.9".parse().unwrap()];
        let plan = resolve_bind(BindMode::Addresses, &[], &ips, 5, &all);
        assert_eq!(plan, BindPlan::Addrs(vec!["192.168.1.11:5".parse().unwrap()]));
        assert_eq!(unavailable(BindMode::Addresses, &[], &ips, &all).len(), 2);
    }

    #[test]
    fn subnet_membership() {
        let a = IfAddr { ip: "192.168.1.10".parse().unwrap(), prefix: 24 };
        assert!(a.same_subnet("192.168.1.200".parse().unwrap()));
        assert!(!a.same_subnet("192.168.2.1".parse().unwrap()));
        assert!(!a.same_subnet("fd00::1".parse().unwrap()));
    }

    #[test]
    fn listing_does_not_panic() {
        let _ = list_interfaces();
    }
}
