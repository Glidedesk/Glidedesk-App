//! Network interface listing and bind-address resolution (PLAN §6.2).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use glidedesk_config::BindMode;
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
    /// IPv6 scope (link-local); 0 otherwise.
    pub scope_id: u32,
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
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(self.prefix.min(128))) };
                u128::from(a) & mask == u128::from(b) & mask
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

/// All interfaces with at least one address.
#[must_use]
pub fn list_interfaces() -> Vec<NetInterface> {
    netdev::get_interfaces()
        .into_iter()
        .map(|i| {
            let mut addrs: Vec<IfAddr> = i
                .ipv4
                .iter()
                .map(|n| IfAddr { ip: IpAddr::V4(n.addr()), prefix: n.prefix_len(), scope_id: 0 })
                .collect();
            for (k, n) in i.ipv6.iter().enumerate() {
                let scope_id = i.ipv6_scope_ids.get(k).copied().unwrap_or(0);
                addrs.push(IfAddr { ip: IpAddr::V6(n.addr()), prefix: n.prefix_len(), scope_id });
            }
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
    /// One dual-stack wildcard socket (`[::]:port`, falls back to `0.0.0.0`).
    Wildcard(u16),
    Addrs(Vec<SocketAddr>),
}

/// Resolves the configured mode to concrete sockets. IPv6 link-local
/// addresses get their scope id so the bind works.
#[must_use]
pub fn resolve_bind(
    mode: BindMode,
    interfaces: &[String],
    addresses: &[IpAddr],
    port: u16,
    all: &[NetInterface],
) -> BindPlan {
    let sock = |a: &IfAddr| match a.ip {
        IpAddr::V4(v4) => SocketAddr::new(IpAddr::V4(v4), port),
        IpAddr::V6(v6) => SocketAddr::V6(std::net::SocketAddrV6::new(v6, port, 0, a.scope_id)),
    };
    match mode {
        BindMode::All => BindPlan::Wildcard(port),
        BindMode::Interfaces => BindPlan::Addrs(
            all.iter()
                .filter(|i| interfaces.contains(&i.name) && i.up)
                .flat_map(|i| i.addrs.iter().map(sock))
                .collect(),
        ),
        BindMode::Addresses => BindPlan::Addrs(
            addresses
                .iter()
                .map(|ip| {
                    all.iter()
                        .flat_map(|i| i.addrs.iter())
                        .find(|a| a.ip == *ip)
                        .map_or_else(|| SocketAddr::new(*ip, port), sock)
                })
                .collect(),
        ),
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

/// Wildcard addresses for a family.
#[must_use]
pub const fn unspecified(v6: bool) -> IpAddr {
    if v6 { IpAddr::V6(Ipv6Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::UNSPECIFIED) }
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
            addrs: ips.iter().map(|(ip, p)| IfAddr { ip: ip.parse().unwrap(), prefix: *p, scope_id: 0 }).collect(),
            mac: None,
        }
    }

    #[test]
    fn resolves_modes() {
        let all = vec![iface("en0", &[("192.168.1.10", 24), ("fd00::10", 64)]), iface("en1", &[("10.0.0.2", 8)])];
        assert_eq!(resolve_bind(BindMode::All, &[], &[], 5, &all), BindPlan::Wildcard(5));
        let BindPlan::Addrs(a) = resolve_bind(BindMode::Interfaces, &["en0".into()], &[], 5, &all) else { panic!() };
        assert_eq!(a.len(), 2);
        let BindPlan::Addrs(a) = resolve_bind(BindMode::Addresses, &[], &["10.0.0.2".parse().unwrap()], 5, &all) else {
            panic!()
        };
        assert_eq!(a, vec!["10.0.0.2:5".parse().unwrap()]);
    }

    #[test]
    fn subnet_membership() {
        let a = IfAddr { ip: "192.168.1.10".parse().unwrap(), prefix: 24, scope_id: 0 };
        assert!(a.same_subnet("192.168.1.200".parse().unwrap()));
        assert!(!a.same_subnet("192.168.2.1".parse().unwrap()));
        assert!(!a.same_subnet("fd00::1".parse().unwrap()));
    }

    #[test]
    fn listing_does_not_panic() {
        let _ = list_interfaces();
    }
}
