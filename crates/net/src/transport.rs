//! QUIC endpoints bound to the selected interfaces/IPs.

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

/// Timing derived from the health settings.
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

fn udp_socket(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    if !addr.is_ipv4() {
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Nexpingdesk uses IPv4 only"));
    }
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.bind(&addr.into())?;
    s.set_nonblocking(true)?;
    Ok(s.into())
}

/// Binds, retrying "address in use" for a moment: right after a restart the
/// previous server may still be letting go of the port.
fn bind_retry(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let mut tries = 0;
    loop {
        match udp_socket(addr) {
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

/// Listening side: one endpoint per bound address, incoming connections merged.
/// Addresses come and go with [`Server::update`] (a network switched off or on)
/// without disturbing connections on the others.
#[derive(Debug)]
pub struct Server {
    endpoints: Vec<(SocketAddr, Endpoint)>,
    config: quinn::ServerConfig,
    /// Kept so the merged channel never closes, even with no endpoint.
    tx: mpsc::Sender<quinn::Incoming>,
    incoming: mpsc::Receiver<quinn::Incoming>,
    fingerprint: [u8; 32],
}

impl Server {
    /// Binds every address of `plan`. Addresses that fail are reported, not
    /// fatal; the wildcard plan must bind. An empty plan listens nowhere until
    /// [`Server::update`] brings an address.
    pub fn bind(plan: &BindPlan, tuning: Tuning) -> Result<(Self, Vec<BindStatus>), NetError> {
        let (tls_cfg, fingerprint) = tls::server_config()?;
        let crypto = QuicServerConfig::try_from(tls_cfg).map_err(|e| NetError::Tls(e.to_string()))?;
        let mut server_cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        server_cfg.transport_config(Arc::new(transport(tuning)?));
        // Connection migration is not needed on a LAN and widens the attack surface.
        server_cfg.migration(false);
        let (tx, incoming) = mpsc::channel(64);
        let mut server = Self { endpoints: Vec::new(), config: server_cfg, tx, incoming, fingerprint };
        let statuses = server.listen(plan, true);
        if matches!(plan, BindPlan::Wildcard(_)) && server.endpoints.is_empty() {
            return Err(NetError::NothingBound(statuses));
        }
        Ok((server, statuses))
    }

    /// Listens on exactly the addresses of `plan`: binds new ones, closes the
    /// ones no longer wanted, keeps the rest (and their connections). Never
    /// blocks: an address that is busy is reported and can be retried by
    /// calling this again.
    pub fn update(&mut self, plan: &BindPlan) -> Vec<BindStatus> {
        self.listen(plan, false)
    }

    /// `retry`: wait (blocking, up to 2 s) for a port still held by the previous
    /// server — only at start, never from a running event loop.
    fn listen(&mut self, plan: &BindPlan, retry: bool) -> Vec<BindStatus> {
        let wanted: Vec<SocketAddr> = match plan {
            BindPlan::Wildcard(port) => vec![SocketAddr::new(unspecified(), *port)],
            // IPv6 entries fail to bind below and are reported (IPv4 only).
            BindPlan::Addrs(a) => a.clone(),
        };
        self.endpoints.retain(|(addr, ep)| {
            let keep = wanted.contains(addr);
            if !keep {
                debug!(%addr, "no longer listening");
                ep.close(VarInt::from_u32(0), b"network gone");
            }
            keep
        });
        let mut statuses = Vec::new();
        for addr in wanted {
            if let Some((_, ep)) = self.endpoints.iter().find(|(a, _)| *a == addr) {
                statuses.push(BindStatus { addr: ep.local_addr().unwrap_or(addr), error: None });
                continue;
            }
            let sock = if retry { bind_retry(addr) } else { udp_socket(addr) };
            let ep = sock.and_then(|s| {
                Endpoint::new(EndpointConfig::default(), Some(self.config.clone()), s, Arc::new(TokioRuntime))
            });
            match ep {
                Ok(ep) => {
                    debug!(%addr, "listening");
                    statuses.push(BindStatus { addr: ep.local_addr().unwrap_or(addr), error: None });
                    let (accepting, tx) = (ep.clone(), self.tx.clone());
                    tokio::spawn(async move {
                        while let Some(inc) = accepting.accept().await {
                            if tx.send(inc).await.is_err() {
                                break;
                            }
                        }
                    });
                    self.endpoints.push((addr, ep));
                }
                Err(e) => {
                    warn!(%addr, error = %e, "bind failed");
                    statuses.push(BindStatus { addr, error: Some(e.to_string()) });
                }
            }
        }
        statuses
    }

    /// Next incoming connection attempt (not yet accepted).
    pub async fn accept(&mut self) -> Option<quinn::Incoming> {
        self.incoming.recv().await
    }

    #[must_use]
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.endpoints.iter().filter_map(|(_, e)| e.local_addr().ok()).collect()
    }

    #[must_use]
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    /// Closes every endpoint and connection immediately.
    pub fn close(&self, reason: &[u8]) {
        for (_, ep) in &self.endpoints {
            ep.close(VarInt::from_u32(0), reason);
        }
    }

    /// Waits (bounded) until closed endpoints have released their sockets, so a
    /// restarted server can bind the same port straight away.
    pub async fn wait_closed(&self) {
        for (_, ep) in &self.endpoints {
            let _ = tokio::time::timeout(Duration::from_secs(2), ep.wait_idle()).await;
        }
    }
}

/// Client endpoint (IPv4, optionally bound to one interface address).
pub fn client_endpoint(bind_ip: Option<IpAddr>, tuning: Tuning) -> Result<Endpoint, NetError> {
    let ip = bind_ip.filter(IpAddr::is_ipv4).unwrap_or_else(unspecified);
    let sock = udp_socket(SocketAddr::new(ip, 0)).map_err(NetError::Io)?;
    let mut ep = Endpoint::new(EndpointConfig::default(), None, sock, Arc::new(TokioRuntime)).map_err(NetError::Io)?;
    let crypto = QuicClientConfig::try_from(tls::client_config()?).map_err(|e| NetError::Tls(e.to_string()))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(Arc::new(transport(tuning)?));
    ep.set_default_client_config(cfg);
    Ok(ep)
}

/// Connects to a server (IPv4 only).
pub async fn connect(ep: &Endpoint, addr: SocketAddr) -> Result<Connection, NetError> {
    if !addr.is_ipv4() {
        return Err(NetError::Connect(format!("{addr} is IPv6; Nexpingdesk uses IPv4 only")));
    }
    let connecting = ep.connect(addr, "nexpingdesk").map_err(|e| NetError::Connect(e.to_string()))?;
    connecting.await.map_err(|e| NetError::Connect(e.to_string()))
}
