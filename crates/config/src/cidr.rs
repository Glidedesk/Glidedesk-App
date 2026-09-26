//! Minimal CIDR parsing/matching for the client allow/block lists.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("'{0}' is not an IP address or CIDR range")]
pub struct CidrError(pub String);

impl Cidr {
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, normalize(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(self.prefix)) };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(self.prefix)) };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// IPv4-mapped IPv6 addresses (`::ffff:1.2.3.4`) are treated as IPv4.
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 @ IpAddr::V4(_) => v4,
    }
}

impl FromStr for Cidr {
    type Err = CidrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || CidrError(s.to_owned());
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr = normalize(addr.parse::<IpAddr>().map_err(|_| err())?);
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(err)?,
            None => max,
        };
        Ok(Self { addr, prefix })
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

/// Parsed allow/block filter. An empty allow list admits everyone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IpFilter {
    allow: Vec<Cidr>,
    block: Vec<Cidr>,
}

impl IpFilter {
    /// # Errors
    /// Returns the first entry that does not parse.
    pub fn new(allow: &[String], block: &[String]) -> Result<Self, CidrError> {
        let parse = |v: &[String]| v.iter().map(|s| s.parse()).collect::<Result<Vec<Cidr>, _>>();
        Ok(Self { allow: parse(allow)?, block: parse(block)? })
    }

    #[must_use]
    pub fn admits(&self, ip: IpAddr) -> bool {
        if self.block.iter().any(|c| c.contains(ip)) {
            return false;
        }
        self.allow.is_empty() || self.allow.iter().any(|c| c.contains(ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn v4_ranges() {
        let c: Cidr = "192.168.1.0/24".parse().unwrap();
        assert!(c.contains(ip("192.168.1.77")));
        assert!(!c.contains(ip("192.168.2.1")));
        assert!(c.contains(ip("::ffff:192.168.1.5")), "mapped v6 counts as v4");
        let all: Cidr = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains(ip("8.8.8.8")));
    }

    #[test]
    fn v6_and_single_hosts() {
        let c: Cidr = "fd00::/8".parse().unwrap();
        assert!(c.contains(ip("fd12::1")));
        assert!(!c.contains(ip("fe80::1")));
        let host: Cidr = "10.0.0.5".parse().unwrap();
        assert!(host.contains(ip("10.0.0.5")) && !host.contains(ip("10.0.0.6")));
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "10.0.0.0/33", "fd00::/129", "hello", "1.2.3.4/x"] {
            assert!(bad.parse::<Cidr>().is_err(), "{bad}");
        }
    }

    #[test]
    fn filter_semantics() {
        let f = IpFilter::new(&["10.0.0.0/8".into()], &["10.0.0.66".into()]).unwrap();
        assert!(f.admits(ip("10.1.2.3")));
        assert!(!f.admits(ip("10.0.0.66")), "block wins");
        assert!(!f.admits(ip("192.168.0.1")));
        assert!(IpFilter::default().admits(ip("1.1.1.1")));
    }
}
