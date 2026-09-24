//! QUIC endpoints bound to the selected interfaces/IPs (PLAN §6.2).

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{Connection, Endpoint, EndpointConfig, IdleTimeout, TokioRuntime, TransportConfig, VarInt};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::interfaces::{BindPlan, unspecified};
use crate::{NetError, tls};

/// Timing derived from the health settings (PLAN §6.7).
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    pub keep_alive: Duration,
    pub idle_timeout: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self { keep_alive: Duration::from_secs(1), idle_timeout: Duration::from_secs(10) }
    }
}

fn transport(t: Tuning) -> Result<TransportConfig, NetError> {
    let mut c = TransportConfig::default();
    c.keep_alive_interval(Some(t.keep_alive));
    c.max_idle_timeout(Some(IdleTimeout::try_from(t.idle_timeout).map_err(|e| NetError::Config(e.to_string()))?));
    // control + input + a bounded number of transfer streams
    c.max_concurrent_bidi_streams(VarInt::from_u32(32));
    c.max_concurrent_uni_streams(VarInt::from_u32(32));
    // Big windows so file transfers reach link speed; memory stays bounded by quinn.
    c.stream_receive_window(VarInt::from_u32(8 * 1024 * 1024));
    c.receive_window(VarInt::from_u32(32 * 1024 * 1024));
    c.send_window(32 * 1024 * 1024);
    c.datagram_receive_buffer_size(None);
    Ok(c)
}

/// Result of binding one address.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BindStatus {
    pub addr: SocketAddr,
    pub error: Option<String>,
}

fn udp_socket(addr: SocketAddr, dual_stack: bool) -> std::io::Result<UdpSocket> {
    let domain = if addr.is_ipv6() { Domain::IPV6 } else { Domain::IPV4 };
    let s = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    if addr.is_ipv6() {
        s.set_only_v6(!dual_stack)?;
    }
    s.bind(&addr.into())?;
    s.set_nonblocking(true)?;
    Ok(s.into())
}

/// Listening side: one endpoint per bound address, incoming connections merged.
#[derive(Debug)]
pub struct Server {
    endpoints: Vec<Endpoint>,
    incoming: mpsc::Receiver<quinn::Incoming>,
    fingerprint: [u8; 32],
}

impl Server {
    /// Binds every address of `plan`; addresses that fail are reported, not fatal,
    /// unless none succeed.
    pub fn bind(plan: &BindPlan, tuning: Tuning) -> Result<(Self, Vec<BindStatus>), NetError> {
        let (tls_cfg, fingerprint) = tls::server_config()?;
        let crypto = QuicServerConfig::try_from(tls_cfg).map_err(|e| NetError::Tls(e.to_string()))?;
        let mut server_cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        server_cfg.transport_config(Arc::new(transport(tuning)?));
        // Connection migration is not needed on a LAN and widens the attack surface.
        server_cfg.migration(false);

        let targets: Vec<(SocketAddr, bool)> = match plan {
            BindPlan::Wildcard(port) => vec![(SocketAddr::new(unspecified(true), *port), true)],
            BindPlan::Addrs(a) => a.iter().map(|a| (*a, false)).collect(),
        };
        let mut statuses = Vec::new();
        let mut endpoints = Vec::new();
        for (addr, dual) in targets {
            let sock = udp_socket(addr, dual).or_else(|e| {
                // No IPv6 on this host: fall back to IPv4 wildcard.
                if dual { udp_socket(SocketAddr::new(unspecified(false), addr.port()), false) } else { Err(e) }
            });
            match sock.and_then(|s| {
                Endpoint::new(EndpointConfig::default(), Some(server_cfg.clone()), s, Arc::new(TokioRuntime))
            }) {
                Ok(ep) => {
                    debug!(%addr, "listening");
                    statuses.push(BindStatus { addr: ep.local_addr().unwrap_or(addr), error: None });
                    endpoints.push(ep);
                }
                Err(e) => {
                    warn!(%addr, error = %e, "bind failed");
                    statuses.push(BindStatus { addr, error: Some(e.to_string()) });
                }
            }
        }
        if endpoints.is_empty() {
            return Err(NetError::NothingBound(statuses));
        }
        let (tx, rx) = mpsc::channel(64);
        for ep in &endpoints {
            let ep = ep.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(inc) = ep.accept().await {
                    if tx.send(inc).await.is_err() {
                        break;
                    }
                }
            });
        }
        Ok((Self { endpoints, incoming: rx, fingerprint }, statuses))
    }

    /// Next incoming connection attempt (not yet accepted).
    pub async fn accept(&mut self) -> Option<quinn::Incoming> {
        self.incoming.recv().await
    }

    #[must_use]
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.endpoints.iter().filter_map(|e| e.local_addr().ok()).collect()
    }

    #[must_use]
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    /// Closes every endpoint and connection immediately.
    pub fn close(&self, reason: &[u8]) {
        for ep in &self.endpoints {
            ep.close(VarInt::from_u32(0), reason);
        }
    }
}

/// Client endpoint (optionally bound to one interface address).
pub fn client_endpoint(bind_ip: Option<IpAddr>, server_is_v6: bool, tuning: Tuning) -> Result<Endpoint, NetError> {
    let ip = bind_ip.unwrap_or_else(|| unspecified(server_is_v6));
    let sock = udp_socket(SocketAddr::new(ip, 0), false).map_err(NetError::Io)?;
    let mut ep = Endpoint::new(EndpointConfig::default(), None, sock, Arc::new(TokioRuntime)).map_err(NetError::Io)?;
    let crypto = QuicClientConfig::try_from(tls::client_config()?).map_err(|e| NetError::Tls(e.to_string()))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(Arc::new(transport(tuning)?));
    ep.set_default_client_config(cfg);
    Ok(ep)
}

/// Connects to a server.
pub async fn connect(ep: &Endpoint, addr: SocketAddr) -> Result<Connection, NetError> {
    let connecting = ep.connect(addr, "glidedesk").map_err(|e| NetError::Connect(e.to_string()))?;
    connecting.await.map_err(|e| NetError::Connect(e.to_string()))
}
