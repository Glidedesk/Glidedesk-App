//! Validation and repair. A hand-edited or imported file with bad values is
//! repaired (values clamped / entries dropped) and every change is reported,
//! instead of refusing to start.

use std::collections::HashSet;

use crate::cidr::Cidr;
use crate::schema::Config;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// Dotted key, e.g. `server.health.interval_ms`.
    pub key: String,
    pub message: String,
}

impl Issue {
    fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self { key: key.into(), message: message.into() }
    }
}

fn clamp_u32(v: &mut u32, lo: u32, hi: u32, key: &str, issues: &mut Vec<Issue>) {
    let c = (*v).clamp(lo, hi);
    if c != *v {
        issues.push(Issue::new(key, format!("{v} is outside {lo}..={hi}; using {c}")));
        *v = c;
    }
}

fn clamp_f64(v: &mut f64, lo: f64, hi: f64, default: f64, key: &str, issues: &mut Vec<Issue>) {
    let c = if v.is_finite() { v.clamp(lo, hi) } else { default };
    if (c - *v).abs() > f64::EPSILON || !v.is_finite() {
        issues.push(Issue::new(key, format!("{v} is outside {lo}..={hi}; using {c}")));
        *v = c;
    }
}

fn retain_cidrs(list: &mut Vec<String>, key: &str, issues: &mut Vec<Issue>) {
    list.retain(|s| {
        let ok = s.parse::<Cidr>().is_ok();
        if !ok {
            issues.push(Issue::new(key, format!("dropped invalid entry '{s}'")));
        }
        ok
    });
}

impl Config {
    /// Repairs out-of-range values in place and reports what changed.
    pub fn sanitize(&mut self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let i = &mut issues;

        if self.device.name.chars().count() > 64 {
            self.device.name = self.device.name.chars().take(64).collect();
            i.push(Issue::new("device.name", "shortened to 64 characters"));
        }

        let net = &mut self.server.network;
        if net.port == 0 {
            net.port = glidedesk_proto::DEFAULT_PORT;
            i.push(Issue::new("server.network.port", "port 0 is not allowed; using the default"));
        }
        // IPv4 only: an IPv6 listen address can never be bound.
        net.addresses.retain(|a| {
            if a.is_ipv4() {
                return true;
            }
            i.push(Issue::new("server.network.addresses", format!("dropped {a}: Glidedesk uses IPv4 only")));
            false
        });
        let mut seen_if = HashSet::new();
        net.interfaces.retain(|n| seen_if.insert(n.clone()));
        retain_cidrs(&mut net.allow_list, "server.network.allow_list", i);
        retain_cidrs(&mut net.block_list, "server.network.block_list", i);

        let h = &mut self.server.health;
        clamp_u32(&mut h.interval_ms, 500, 10_000, "server.health.interval_ms", i);
        clamp_u32(&mut h.miss_threshold, 1, 20, "server.health.miss_threshold", i);
        clamp_u32(&mut h.degraded_latency_ms, 5, 5_000, "server.health.degraded_latency_ms", i);
        let min_idle = h.interval_ms.saturating_mul(h.miss_threshold + 1);
        clamp_u32(&mut h.idle_timeout_ms, min_idle, 120_000, "server.health.idle_timeout_ms", i);

        let sw = &mut self.server.switching;
        clamp_u32(&mut sw.delay_ms, 0, 5_000, "server.switching.delay_ms", i);
        clamp_u32(&mut sw.double_tap_ms, 0, 2_000, "server.switching.double_tap_ms", i);
        clamp_u32(&mut sw.dead_corner_px, 0, 500, "server.switching.dead_corner_px", i);
        clamp_u32(&mut self.server.visuals.identify_seconds, 1, 30, "server.visuals.identify_seconds", i);

        let mut seen = HashSet::new();
        self.server.clients.retain(|c| {
            let fresh = seen.insert(c.id);
            if !fresh {
                i.push(Issue::new("server.clients", format!("dropped duplicate client {}", c.id)));
            }
            fresh
        });
        for c in &mut self.server.clients {
            let key = format!("server.clients.{}", c.id);
            clamp_f64(&mut c.mouse_speed, 0.1, 10.0, 1.0, &format!("{key}.mouse_speed"), i);
            clamp_f64(&mut c.scroll_speed, 0.1, 10.0, 1.0, &format!("{key}.scroll_speed"), i);
            if !c.mac_address.is_empty() && !is_mac_address(&c.mac_address) {
                i.push(Issue::new(format!("{key}.mac_address"), format!("'{}' is not a MAC address", c.mac_address)));
                c.mac_address.clear();
            }
        }

        let cl = &mut self.client;
        for (v, key) in [(&mut cl.mouse_speed, "client.mouse_speed"), (&mut cl.scroll_speed, "client.scroll_speed")] {
            if let Some(x) = v.as_mut() {
                clamp_f64(x, 0.1, 10.0, 1.0, key, i);
            }
        }

        self.layout.links.retain(|l| {
            let ok = l.from_span.is_valid() && l.to_span.is_valid() && l.from != l.to;
            if !ok {
                i.push(Issue::new("layout.link", format!("dropped invalid link {} → {}", l.from, l.to)));
            }
            ok
        });
        issues
    }
}

#[must_use]
pub fn is_mac_address(s: &str) -> bool {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ClientEntry;
    use glidedesk_proto::DeviceId;

    #[test]
    fn defaults_are_valid() {
        let mut c = Config::default();
        assert!(c.sanitize().is_empty());
    }

    #[test]
    fn repairs_out_of_range_values() {
        let mut c = Config::default();
        c.server.health.interval_ms = 1;
        c.server.network.port = 0;
        c.server.network.allow_list = vec!["10.0.0.0/8".into(), "bogus".into()];
        c.server.clients = vec![
            ClientEntry {
                id: DeviceId([1; 16]),
                mouse_speed: f64::NAN,
                mac_address: "zz".into(),
                ..Default::default()
            },
            ClientEntry { id: DeviceId([1; 16]), ..Default::default() },
        ];
        let issues = c.sanitize();
        assert_eq!(c.server.health.interval_ms, 500);
        assert_ne!(c.server.network.port, 0);
        assert_eq!(c.server.network.allow_list, vec!["10.0.0.0/8".to_string()]);
        assert_eq!(c.server.clients.len(), 1);
        assert!((c.server.clients[0].mouse_speed - 1.0).abs() < f64::EPSILON);
        assert!(c.server.clients[0].mac_address.is_empty());
        assert!(issues.len() >= 6, "{issues:#?}");
        // Sanitising twice is a no-op.
        assert!(c.sanitize().is_empty());
    }

    #[test]
    fn mac_addresses() {
        assert!(is_mac_address("aa:bb:cc:dd:ee:ff"));
        assert!(is_mac_address("AA-BB-CC-DD-EE-01"));
        assert!(!is_mac_address("aa:bb:cc:dd:ee"));
        assert!(!is_mac_address("gg:bb:cc:dd:ee:ff"));
    }
}
