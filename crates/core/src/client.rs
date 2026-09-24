//! Client runtime: find the server, connect, inject what it sends, answer
//! heartbeats, reconnect with back-off (PLAN §6.7).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glidedesk_config::{Config, RemapPreset};
use glidedesk_input::Injector;
use glidedesk_net::{Browser, DiscoveryEvent, FrameReader, FrameWriter, Tuning};
use glidedesk_proto::{
    ClientPrefs, ClientSettings, ClientStatus, Control, DEFAULT_PORT, DeviceId, Features, GoodbyeReason, Hello, Input,
    LedState, MAX_CONTROL_FRAME, MAX_INPUT_FRAME, MonitorInfo, PROTOCOL_VERSION, Platform, RejectReason, stream_kind,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::{CoreError, MonitorSource, Notice};
use glidedesk_ipc::views::{ClientSideView, LinkState, MachineView};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
const MONITOR_POLL: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum ClientCommand {
    Reconnect,
    ApplyConfig(Box<Config>),
    Shutdown(GoodbyeReason),
}

#[derive(Debug)]
pub struct ClientHandle {
    pub commands: mpsc::Sender<ClientCommand>,
    pub status: watch::Receiver<ClientSideView>,
    pub notices: mpsc::Receiver<Notice>,
    pub task: JoinHandle<()>,
}

/// Reports lock / sleep / session state for pongs (platform-provided).
pub type StatusSource = Arc<dyn Fn() -> ClientStatus + Send + Sync>;

pub struct ClientDeps {
    pub injector: Box<dyn Injector>,
    pub monitors: MonitorSource,
    pub status: StatusSource,
    pub app_version: String,
    pub host_name: String,
    /// Server we connected to last time (preferred when several are found).
    pub preferred_server: Option<DeviceId>,
    /// System clipboard (`None` where unsupported).
    pub clipboard: Option<Box<dyn glidedesk_clipboard::Clipboard>>,
}

impl std::fmt::Debug for ClientDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientDeps").field("app_version", &self.app_version).finish_non_exhaustive()
    }
}

pub fn start(config: Config, deps: ClientDeps) -> ClientHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let (status_tx, status_rx) = watch::channel(ClientSideView::default());
    let (notice_tx, notice_rx) = mpsc::channel(64);
    let task = tokio::spawn(run(config, deps, cmd_rx, status_tx, notice_tx));
    ClientHandle { commands: cmd_tx, status: status_rx, notices: notice_rx, task }
}

/// Injection runs on its own OS thread: OS calls stay off the async workers
/// and events are applied strictly in order.
enum InjectCmd {
    Input(Input),
    Leds(LedState),
}

fn spawn_injector(mut inj: Box<dyn Injector>) -> std::sync::mpsc::Sender<InjectCmd> {
    let (tx, rx) = std::sync::mpsc::channel::<InjectCmd>();
    let spawned = std::thread::Builder::new().name("gd-inject".into()).spawn(move || {
        let mut errors = 0u32;
        while let Ok(cmd) = rx.recv() {
            let res = match cmd {
                InjectCmd::Input(ev) => inj.inject(&ev),
                InjectCmd::Leds(l) => {
                    inj.set_leds(l);
                    Ok(())
                }
            };
            if let Err(e) = res {
                errors += 1;
                // Rate-limit: blocked injection (UAC / lock screen) fails per event.
                if errors == 1 || errors.is_multiple_of(500) {
                    warn!(error = %e, count = errors, "input injection failed");
                }
            }
        }
        inj.release_all();
    });
    if let Err(e) = spawned {
        warn!(error = %e, "could not start the injection thread");
    }
    tx
}

fn prefs(cfg: &Config) -> ClientPrefs {
    let c = &cfg.client;
    #[allow(clippy::cast_possible_truncation)]
    ClientPrefs {
        mouse_speed: c.mouse_speed.map(|v| v as f32),
        scroll_speed: c.scroll_speed.map(|v| v as f32),
        scroll_invert: c.scroll_invert,
        key_remap: c.key_remap.map(|r| match r {
            RemapPreset::Auto => 0,
            RemapPreset::None => 1,
            RemapPreset::SwapCtrlMeta => 2,
        }),
        accept_clipboard: c.accept_clipboard,
        accept_files: c.accept_files,
        draw_cursor: c.draw_cursor,
        led_sync: c.led_sync,
    }
}

/// `host`, `host:port`, `ip`, `ip:port`, `[v6]:port`.
async fn resolve(address: &str) -> Result<Vec<SocketAddr>, CoreError> {
    let a = address.trim();
    if let Ok(sa) = a.parse::<SocketAddr>() {
        return Ok(vec![sa]);
    }
    if let Ok(ip) = a.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, DEFAULT_PORT)]);
    }
    let with_port = if a.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        a.to_owned()
    } else {
        format!("{a}:{DEFAULT_PORT}")
    };
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(with_port)
        .await
        .map_err(|e| CoreError::Net(format!("cannot resolve '{a}': {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(CoreError::Net(format!("'{a}' has no addresses")));
    }
    Ok(addrs)
}

/// IP of the chosen local interface, if the user picked one.
fn bind_ip(cfg: &Config, want_v6: bool) -> Option<IpAddr> {
    let name = cfg.client.interface.trim();
    if name.is_empty() {
        return None;
    }
    glidedesk_net::list_interfaces()
        .into_iter()
        .find(|i| i.name == name)
        .and_then(|i| i.addrs.into_iter().map(|a| a.ip).find(|ip| ip.is_ipv6() == want_v6 && !ip.is_loopback()))
}

enum SessionEnd {
    Command(ClientCommand),
    Lost(String),
    Rejected(RejectReason),
    Goodbye(GoodbyeReason),
}

struct Runner {
    config: Config,
    deps_monitors: MonitorSource,
    status_fn: StatusSource,
    inject: std::sync::mpsc::Sender<InjectCmd>,
    status: watch::Sender<ClientSideView>,
    notices: mpsc::Sender<Notice>,
    app_version: String,
    host_name: String,
    preferred: Option<DeviceId>,
    monitors: Vec<MonitorInfo>,
    sync: std::sync::Arc<crate::sync::Sync>,
    /// Clipboard sequence last sent to the server.
    last_sent: Option<u64>,
}

async fn run(
    config: Config,
    deps: ClientDeps,
    mut cmds: mpsc::Receiver<ClientCommand>,
    status: watch::Sender<ClientSideView>,
    notices: mpsc::Sender<Notice>,
) {
    let monitors = (deps.monitors)().unwrap_or_default();
    let mut r = Runner {
        inject: spawn_injector(deps.injector),
        config,
        deps_monitors: deps.monitors,
        status_fn: deps.status,
        status,
        notices,
        app_version: deps.app_version,
        host_name: deps.host_name,
        preferred: deps.preferred_server,
        monitors,
        sync: crate::sync::Sync::new(deps.clipboard),
        last_sent: None,
    };
    let mut backoff = BACKOFF_MIN;
    loop {
        let end = r.session(&mut cmds).await;
        let _ = r.inject.send(InjectCmd::Input(Input::ReleaseAll));
        match end {
            SessionEnd::Command(ClientCommand::Shutdown(_)) => break,
            SessionEnd::Command(ClientCommand::ApplyConfig(c)) => {
                r.config = *c;
                backoff = BACKOFF_MIN;
                continue;
            }
            SessionEnd::Command(ClientCommand::Reconnect) => {
                backoff = BACKOFF_MIN;
                continue;
            }
            SessionEnd::Rejected(reason) => {
                r.update(|v| {
                    v.state = LinkState::Rejected;
                    v.message = Some(format!("the server refused the connection ({reason:?})"));
                    v.active = false;
                });
                backoff = BACKOFF_MAX * 2;
            }
            SessionEnd::Goodbye(reason) => {
                info!(?reason, "server said goodbye");
                r.update(|v| {
                    v.state = LinkState::Offline;
                    v.active = false;
                    v.latency_ms = None;
                    v.message = Some(format!("server {reason:?}").to_lowercase());
                });
                backoff = BACKOFF_MIN;
            }
            SessionEnd::Lost(why) => {
                debug!(%why, "link lost");
                r.update(|v| {
                    if v.state != LinkState::Searching {
                        v.state = LinkState::Offline;
                    }
                    v.active = false;
                    v.latency_ms = None;
                    v.message = Some(why);
                });
            }
        }
        // Back-off, but stay responsive to commands.
        tokio::select! {
            () = tokio::time::sleep(backoff) => {}
            cmd = cmds.recv() => match cmd {
                Some(ClientCommand::Shutdown(_)) | None => break,
                Some(ClientCommand::ApplyConfig(c)) => r.config = *c,
                Some(ClientCommand::Reconnect) => {}
            }
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    r.update(|v| {
        v.state = LinkState::Stopped;
        v.active = false;
    });
    info!("client stopped");
}

impl Runner {
    fn update(&self, f: impl FnOnce(&mut ClientSideView)) {
        self.status.send_modify(|v| {
            f(v);
            v.local =
                MachineView { id: Some(self.config.device.id), name: self.name(), monitors: self.monitors.clone() };
        });
    }

    fn name(&self) -> String {
        if self.config.device.name.is_empty() { self.host_name.clone() } else { self.config.device.name.clone() }
    }

    /// Finds candidate server addresses (configured or via mDNS).
    async fn find_server(&mut self, cmds: &mut mpsc::Receiver<ClientCommand>) -> Result<Vec<SocketAddr>, SessionEnd> {
        let configured = self.config.client.server_address.trim().to_owned();
        if !configured.is_empty() {
            self.update(|v| {
                v.state = LinkState::Connecting;
                v.server_address = Some(configured.clone());
            });
            return resolve(&configured).await.map_err(|e| SessionEnd::Lost(e.to_string()));
        }
        self.update(|v| {
            v.state = LinkState::Searching;
            v.message = None;
        });
        let (_browser, mut rx) = Browser::start(Some(self.config.client.interface.as_str()))
            .map_err(|e| SessionEnd::Lost(format!("discovery unavailable: {e}")))?;
        let deadline = tokio::time::sleep(Duration::from_secs(10));
        tokio::pin!(deadline);
        let mut first: Option<Vec<SocketAddr>> = None;
        loop {
            tokio::select! {
                ev = rx.recv() => match ev {
                    Some(DiscoveryEvent::Found(ad)) if ad.protocol == PROTOCOL_VERSION && ad.id != self.config.device.id => {
                        let addrs: Vec<SocketAddr> = ad.addrs.iter().map(|ip| SocketAddr::new(*ip, ad.port)).collect();
                        if self.preferred.is_none() || self.preferred == Some(ad.id) {
                            return Ok(addrs);
                        }
                        first.get_or_insert(addrs);
                    }
                    Some(_) => {}
                    None => return Err(SessionEnd::Lost("discovery stopped".into())),
                },
                () = &mut deadline => {
                    return first.ok_or_else(|| SessionEnd::Lost("no server found on the network".into()));
                }
                cmd = cmds.recv() => return Err(SessionEnd::Command(cmd.unwrap_or(ClientCommand::Shutdown(GoodbyeReason::Stopping)))),
            }
        }
    }

    async fn connect(&self, addrs: &[SocketAddr]) -> Result<(quinn::Endpoint, quinn::Connection, SocketAddr), String> {
        let tuning = Tuning::default();
        let mut last_err = String::from("no address");
        // Prefer IPv4 then IPv6; link-local last.
        let mut ordered = addrs.to_vec();
        ordered.sort_by_key(|a| (a.is_ipv6(), matches!(a.ip(), IpAddr::V6(v6) if v6.is_unicast_link_local())));
        for addr in ordered {
            let ep = match glidedesk_net::client_endpoint(bind_ip(&self.config, addr.is_ipv6()), addr.is_ipv6(), tuning)
            {
                Ok(ep) => ep,
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            };
            match tokio::time::timeout(CONNECT_TIMEOUT, glidedesk_net::connect(&ep, addr)).await {
                Ok(Ok(conn)) => return Ok((ep, conn, addr)),
                Ok(Err(e)) => last_err = e.to_string(),
                Err(_) => last_err = format!("{addr}: timed out"),
            }
        }
        Err(last_err)
    }

    fn hello(&self) -> Hello {
        Hello {
            protocol: PROTOCOL_VERSION,
            app_version: self.app_version.clone(),
            device_id: self.config.device.id,
            name: self.name(),
            platform: Platform::current(),
            monitors: self.monitors.clone(),
            features: Features::default().with(Features::LED_SYNC).with(Features::DRAW_CURSOR),
            prefs: prefs(&self.config),
        }
    }

    /// Cursor went back to the server: send our clipboard if it changed.
    fn push_clipboard(&mut self, conn: &quinn::Connection, settings: &ClientSettings, peer: String) {
        let (allow_clip, allow_files) =
            (settings.clipboard && settings.clipboard_send, settings.files && settings.clipboard_send);
        if !(allow_clip || allow_files) || !self.sync.available() {
            return;
        }
        let Some(seq) = self.sync.changed_for(None, self.last_sent) else { return };
        self.last_sent = Some(seq);
        let (sync, conn, limit) = (self.sync.clone(), conn.clone(), settings.clipboard_limit);
        tokio::spawn(async move {
            if let Err(e) = sync.send(conn, None, peer, allow_clip, allow_files, limit).await {
                debug!(error = %e, "clipboard send failed");
            }
        });
    }

    fn receive_stream(
        &self,
        kind: u8,
        stream: quinn::RecvStream,
        settings: &ClientSettings,
        taken: &mpsc::Sender<glidedesk_proto::FileSetId>,
    ) {
        let (allow_clip, allow_files) =
            (settings.clipboard && settings.clipboard_receive, settings.files && settings.clipboard_receive);
        let (sync, taken, limit) = (self.sync.clone(), taken.clone(), settings.clipboard_limit);
        let peer = self.status.borrow().server_name.clone().unwrap_or_else(|| "server".into());
        tokio::spawn(async move {
            match sync.receive(kind, stream, None, peer, allow_clip, allow_files, limit).await {
                Ok(Some(files)) if files.cut => {
                    if crate::sync::wait_taken(files.roots).await {
                        let _ = taken.send(files.set).await;
                    }
                }
                Ok(_) => {}
                Err(e) => warn!(error = %e, "receiving clipboard failed"),
            }
        });
    }

    #[allow(clippy::too_many_lines)]
    async fn session(&mut self, cmds: &mut mpsc::Receiver<ClientCommand>) -> SessionEnd {
        let addrs = match self.find_server(cmds).await {
            Ok(a) => a,
            Err(end) => return end,
        };
        self.update(|v| v.state = LinkState::Connecting);
        let (_ep, conn, addr) = match self.connect(&addrs).await {
            Ok(x) => x,
            Err(e) => return SessionEnd::Lost(e),
        };
        let (send, recv) = match conn.open_bi().await {
            Ok(s) => s,
            Err(e) => return SessionEnd::Lost(e.to_string()),
        };
        let mut control = FrameWriter::new(send, MAX_CONTROL_FRAME);
        let mut reader = FrameReader::new(recv, MAX_CONTROL_FRAME);
        if let Err(e) = control.send(&Control::Hello(self.hello())).await {
            return SessionEnd::Lost(e.to_string());
        }
        let welcome = match tokio::time::timeout(CONNECT_TIMEOUT, reader.recv::<Control>()).await {
            Ok(Ok(Some(Control::Welcome(w)))) => w,
            Ok(Ok(Some(Control::Reject(r)))) => return SessionEnd::Rejected(r),
            Ok(Ok(_)) => return SessionEnd::Lost("unexpected reply from server".into()),
            Ok(Err(e)) => return SessionEnd::Lost(e.to_string()),
            Err(_) => return SessionEnd::Lost("server did not answer".into()),
        };
        let mut settings: ClientSettings = welcome.settings;
        let (taken_tx, mut taken_rx) = mpsc::channel::<glidedesk_proto::FileSetId>(8);
        // The first unidirectional stream is the input stream; anything else
        // (clipboard, files) is handled on its own task.
        let mut input = loop {
            let Ok(Ok(mut s)) = tokio::time::timeout(CONNECT_TIMEOUT, conn.accept_uni()).await else {
                return SessionEnd::Lost("server did not open the input stream".into());
            };
            let mut kind = [0u8; 1];
            if s.read_exact(&mut kind).await.is_err() {
                continue;
            }
            if kind[0] == stream_kind::INPUT {
                break FrameReader::new(s, MAX_INPUT_FRAME);
            }
            self.receive_stream(kind[0], s, &settings, &taken_tx);
        };
        self.preferred = Some(welcome.device_id);
        self.last_sent = None;
        let server_name: String = welcome.name.chars().filter(|c| !c.is_control()).take(64).collect();
        info!(server = %server_name, %addr, "connected");
        let _ = self.notices.try_send(Notice::ServerConnected { id: welcome.device_id, name: server_name.clone() });
        self.update(|v| {
            v.state = LinkState::Connected;
            v.server_id = Some(welcome.device_id);
            v.server_name = Some(server_name.clone());
            v.server_address = Some(addr.to_string());
            v.server_version = Some(welcome.app_version.chars().take(32).collect());
            v.clipboard = settings.clipboard;
            v.files = settings.files;
            v.message = None;
        });

        let mut monitor_poll = tokio::time::interval(MONITOR_POLL);
        monitor_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_ping = Instant::now();
        let mut transfer_tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                biased;
                st = conn.accept_uni() => match st {
                    Ok(mut s) => {
                        let mut kind = [0u8; 1];
                        if s.read_exact(&mut kind).await.is_ok() {
                            self.receive_stream(kind[0], s, &settings, &taken_tx);
                        }
                    }
                    Err(e) => return SessionEnd::Lost(e.to_string()),
                },
                Some(set) = taken_rx.recv() => {
                    if let Err(e) = control.send(&Control::FilesTaken(set)).await {
                        return SessionEnd::Lost(e.to_string());
                    }
                }
                _ = transfer_tick.tick(), if self.sync.transfers.any_active() => {
                    let t = self.sync.transfers.snapshot();
                    self.status.send_modify(|v| v.transfers = t);
                }
                ev = input.recv::<Input>() => match ev {
                    Ok(Some(ev)) => {
                        if self.inject.send(InjectCmd::Input(ev)).is_err() {
                            return SessionEnd::Lost("injection thread stopped".into());
                        }
                    }
                    Ok(None) => return SessionEnd::Lost("input stream closed".into()),
                    Err(e) => return SessionEnd::Lost(e.to_string()),
                },
                msg = reader.recv::<Control>() => match msg {
                    Ok(Some(Control::Ping { seq, sent_us, rtt_us })) => {
                        last_ping = Instant::now();
                        let status = (self.status_fn)();
                        if let Err(e) = control.send(&Control::Pong { seq, sent_us, status }).await {
                            return SessionEnd::Lost(e.to_string());
                        }
                        #[allow(clippy::cast_precision_loss)]
                        let latency = (rtt_us > 0).then_some(rtt_us as f32 / 1000.0);
                        if latency.is_some() {
                            self.status.send_modify(|v| v.latency_ms = latency);
                        }
                    }
                    Ok(Some(Control::Enter { pos, leds })) => {
                        let _ = self.inject.send(InjectCmd::Input(Input::MouseAbs(pos)));
                        if settings.led_sync {
                            let _ = self.inject.send(InjectCmd::Leds(leds));
                        }
                        self.update(|v| v.active = true);
                    }
                    Ok(Some(Control::Leave)) => {
                        let _ = self.inject.send(InjectCmd::Input(Input::ReleaseAll));
                        self.update(|v| v.active = false);
                        self.push_clipboard(&conn, &settings, server_name.clone());
                    }
                    Ok(Some(Control::FilesTaken(set))) => {
                        tokio::spawn(self.sync.clone().files_taken(set, None));
                    }
                    Ok(Some(Control::Settings(s))) => {
                        settings = s;
                        self.update(|v| {
                            v.clipboard = settings.clipboard;
                            v.files = settings.files;
                        });
                    }
                    Ok(Some(Control::Identify { label })) => {
                        let label: String = label.chars().filter(|c| !c.is_control()).take(64).collect();
                        let _ = self.notices.try_send(Notice::Identify { label });
                    }
                    Ok(Some(Control::Goodbye(reason))) => return SessionEnd::Goodbye(reason),
                    Ok(Some(_)) => {} // not meaningful for a client
                    Ok(None) => return SessionEnd::Lost("server closed the link".into()),
                    Err(e) => return SessionEnd::Lost(e.to_string()),
                },
                _ = monitor_poll.tick() => {
                    if let Ok(m) = (self.deps_monitors)()
                        && m != self.monitors
                    {
                        self.monitors.clone_from(&m);
                        if let Err(e) = control.send(&Control::MonitorsChanged(m)).await {
                            return SessionEnd::Lost(e.to_string());
                        }
                        self.update(|_| {});
                    }
                    // The server pings every interval; long silence means the link is dead
                    // even if QUIC has not timed out yet.
                    if last_ping.elapsed() > Duration::from_secs(15) {
                        return SessionEnd::Lost("no heartbeat from server".into());
                    }
                }
                cmd = cmds.recv() => {
                    let cmd = cmd.unwrap_or(ClientCommand::Shutdown(GoodbyeReason::Stopping));
                    match cmd {
                        ClientCommand::ApplyConfig(c) => {
                            let address_changed = c.client.server_address != self.config.client.server_address
                                || c.client.interface != self.config.client.interface;
                            self.config = *c;
                            if address_changed {
                                let _ = control.send(&Control::Goodbye(GoodbyeReason::Restarting)).await;
                                return SessionEnd::Command(ClientCommand::Reconnect);
                            }
                            if let Err(e) = control.send(&Control::Prefs(prefs(&self.config))).await {
                                return SessionEnd::Lost(e.to_string());
                            }
                            self.update(|_| {});
                        }
                        ClientCommand::Shutdown(reason) => {
                            let _ = control.send(&Control::Goodbye(reason)).await;
                            control.finish();
                            conn.close(0u32.into(), b"client stopping");
                            return SessionEnd::Command(ClientCommand::Shutdown(reason));
                        }
                        ClientCommand::Reconnect => {
                            let _ = control.send(&Control::Goodbye(GoodbyeReason::Restarting)).await;
                            conn.close(0u32.into(), b"reconnect");
                            return SessionEnd::Command(ClientCommand::Reconnect);
                        }
                    }
                }
            }
        }
    }
}
