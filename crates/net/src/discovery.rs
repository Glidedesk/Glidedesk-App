//! mDNS/DNS-SD discovery (PLAN §3.1). Announcements go out only on the
//! interfaces the server is bound to. Everything received is untrusted.

use std::collections::HashSet;
use std::net::IpAddr;

use glidedesk_proto::{DeviceId, MDNS_SERVICE, PROTOCOL_VERSION};
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::NetError;

const MAX_NAME: usize = 64;

/// A server seen on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAd {
    pub id: DeviceId,
    pub name: String,
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    pub protocol: u16,
    pub app_version: String,
    /// The server's computer (host) name, so users can type it to connect.
    pub host: String,
    /// mDNS instance name (used to match removals).
    pub fullname: String,
}

impl ServerAd {
    /// Does a name typed by the user refer to this server? Compares the
    /// display name and the computer name, ignoring case and a `.local` suffix.
    #[must_use]
    pub fn matches_name(&self, typed: &str) -> bool {
        let norm = |s: &str| {
            let s = s.trim().trim_end_matches('.').to_lowercase();
            s.strip_suffix(".local").map_or_else(|| s.clone(), str::to_owned)
        };
        let want = norm(typed);
        !want.is_empty() && (norm(&self.name) == want || norm(&self.host) == want)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryEvent {
    Found(ServerAd),
    Lost { fullname: String },
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect()
}

#[allow(clippy::needless_pass_by_value)] // used as `map_err(mdns_err)`
fn mdns_err(e: mdns_sd::Error) -> NetError {
    NetError::Discovery(e.to_string())
}

/// Registered mDNS announcement; unregisters on drop.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    /// `interfaces`: `None` = all interfaces, else only these OS names / addresses.
    /// IPv4 only.
    pub fn start(
        id: DeviceId,
        name: &str,
        host_name: &str,
        app_version: &str,
        port: u16,
        interfaces: Option<(&[String], &[IpAddr])>,
    ) -> Result<Self, NetError> {
        let daemon = ServiceDaemon::new().map_err(mdns_err)?;
        if let Some((names, addrs)) = interfaces {
            daemon.disable_interface(IfKind::All).map_err(mdns_err)?;
            let mut kinds: Vec<IfKind> = names.iter().cloned().map(IfKind::Name).collect();
            kinds.extend(addrs.iter().copied().filter(IpAddr::is_ipv4).map(IfKind::Addr));
            if !kinds.is_empty() {
                daemon.enable_interface(kinds).map_err(mdns_err)?;
            }
        }
        // Last, so it wins over the interface names enabled above.
        daemon.disable_interface(IfKind::IPv6).map_err(mdns_err)?;
        let id_s = id.to_string();
        let instance = format!("{} ({})", clean(name), &id_s[..8]);
        let host = format!("glidedesk-{}.local.", &id_s[..12]);
        let props = [
            ("id", id_s.as_str()),
            ("name", &clean(name)),
            ("host", &clean(host_name)),
            ("v", app_version),
            ("p", &PROTOCOL_VERSION.to_string()),
        ];
        let info = ServiceInfo::new(MDNS_SERVICE, &instance, &host, "", port, &props[..])
            .map_err(mdns_err)?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        daemon.register(info).map_err(mdns_err)?;
        debug!(%fullname, "mDNS announced");
        Ok(Self { daemon, fullname })
    }
}

impl std::fmt::Debug for Advertiser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Advertiser").field("fullname", &self.fullname).finish_non_exhaustive()
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        // Sends a goodbye so clients mark us offline immediately.
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Browses for servers; events arrive on the returned channel until dropped.
pub struct Browser {
    daemon: ServiceDaemon,
}

impl Browser {
    pub fn start(interface: Option<&str>) -> Result<(Self, mpsc::Receiver<DiscoveryEvent>), NetError> {
        let daemon = ServiceDaemon::new().map_err(mdns_err)?;
        if let Some(i) = interface.filter(|i| !i.is_empty()) {
            daemon.disable_interface(IfKind::All).map_err(mdns_err)?;
            daemon.enable_interface(IfKind::Name(i.to_owned())).map_err(mdns_err)?;
        }
        daemon.disable_interface(IfKind::IPv6).map_err(mdns_err)?;
        let rx_mdns = daemon.browse(MDNS_SERVICE).map_err(mdns_err)?;
        let (tx, rx) = mpsc::channel(32);
        std::thread::Builder::new()
            .name("gd-mdns-browse".into())
            .spawn(move || {
                while let Ok(ev) = rx_mdns.recv() {
                    let out = match ev {
                        ServiceEvent::ServiceResolved(s) => parse(&s).map(DiscoveryEvent::Found),
                        ServiceEvent::ServiceRemoved(_, fullname) => Some(DiscoveryEvent::Lost { fullname }),
                        ServiceEvent::SearchStopped(_) => break,
                        _ => None,
                    };
                    if let Some(out) = out
                        && tx.blocking_send(out).is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(NetError::Io)?;
        Ok((Self { daemon }, rx))
    }
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser").finish_non_exhaustive()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(MDNS_SERVICE);
        let _ = self.daemon.shutdown();
    }
}

fn parse(s: &mdns_sd::ResolvedService) -> Option<ServerAd> {
    let props = &s.txt_properties;
    let Some(id) = props.get_property_val_str("id").and_then(DeviceId::parse) else {
        warn!(fullname = %s.fullname, "ignoring mDNS record without a valid id");
        return None;
    };
    let protocol = props.get_property_val_str("p").and_then(|p| p.parse().ok()).unwrap_or(0);
    let mut seen = HashSet::new();
    let addrs: Vec<IpAddr> = s
        .addresses
        .iter()
        .map(mdns_sd::ScopedIp::to_ip_addr)
        .filter(|a| a.is_ipv4() && seen.insert(*a))
        .take(16)
        .collect();
    if addrs.is_empty() || s.port == 0 {
        return None;
    }
    Some(ServerAd {
        id,
        name: clean(props.get_property_val_str("name").unwrap_or("Glidedesk")),
        addrs,
        port: s.port,
        protocol,
        app_version: clean(props.get_property_val_str("v").unwrap_or("")),
        host: clean(props.get_property_val_str("host").unwrap_or("")),
        fullname: s.fullname.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_names_match_display_or_computer_name() {
        let ad = ServerAd {
            id: DeviceId([1; 16]),
            name: "Studio Mac".into(),
            addrs: vec![],
            port: 1,
            protocol: PROTOCOL_VERSION,
            app_version: String::new(),
            host: "Soykots-MacBook-Pro".into(),
            fullname: String::new(),
        };
        assert!(ad.matches_name("studio mac"));
        assert!(ad.matches_name("soykots-macbook-pro.local"));
        assert!(ad.matches_name("SOYKOTS-MACBOOK-PRO"));
        assert!(!ad.matches_name("office-pc"));
        assert!(!ad.matches_name("  "));
    }
}
