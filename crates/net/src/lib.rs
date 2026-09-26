//! Networking: QUIC transport on the selected interfaces/IPs, TLS without
//! authentication (encryption only), admission filtering and mDNS
//! discovery.

#![forbid(unsafe_code)]

pub mod auth;
pub mod discovery;
pub mod framed;
pub mod interfaces;
pub mod tls;
pub mod transport;

use std::net::IpAddr;

use glidedesk_config::IpFilter;

pub use discovery::{Advertiser, Browser, DiscoveryEvent, ServerAd};
pub use framed::{FrameReader, FrameWriter};
pub use interfaces::{BindPlan, IfAddr, InterfaceKind, NetInterface, list_interfaces, resolve_bind, unavailable};
pub use transport::{BindStatus, Server, Tuning, client_endpoint, connect};

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS setup: {0}")]
    Tls(String),
    #[error("transport configuration: {0}")]
    Config(String),
    #[error("could not listen on any address: {0:?}")]
    NothingBound(Vec<BindStatus>),
    #[error("connect: {0}")]
    Connect(String),
    #[error("stream: {0}")]
    Stream(String),
    #[error("frame: {0}")]
    Frame(#[from] glidedesk_proto::FrameError),
    #[error("discovery: {0}")]
    Discovery(String),
}

/// Decides whether a connecting peer may even start a handshake
/// (not authentication — a filter).
#[derive(Clone, Debug, Default)]
pub struct Admission {
    pub filter: IpFilter,
    /// When set, the peer must be in the subnet of one of these addresses.
    pub same_subnet: Option<Vec<IfAddr>>,
}

impl Admission {
    #[must_use]
    pub fn admits(&self, ip: IpAddr) -> bool {
        // IPv4 only (an IPv4-mapped IPv6 address counts as IPv4).
        let ip = match ip {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                None => return false,
            },
            v4 @ IpAddr::V4(_) => v4,
        };
        if !self.filter.admits(ip) {
            return false;
        }
        match &self.same_subnet {
            None => true,
            Some(nets) => ip.is_loopback() || nets.iter().any(|n| n.same_subnet(ip)),
        }
    }
}
