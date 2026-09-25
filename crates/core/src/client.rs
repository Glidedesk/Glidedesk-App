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
    /// Fetch offered files now, without a paste to replay.
    FetchOffer,
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

/// What the server applies here, for the window.
fn applied(s: &ClientSettings) -> glidedesk_ipc::views::AppliedView {
    glidedesk_ipc::views::AppliedView {
        mouse_speed: s.mouse_speed,
        scroll_speed: s.scroll_speed,
        scroll_invert: s.scroll_invert,
        key_remap: match s.key_remap {
            0 => "auto",
            2 => "swap-ctrl-meta",
            _ => "none",
        }
        .into(),
    }
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

/// Splits `name[:port]`.
fn split_port(a: &str) -> (&str, Option<u16>) {
    match a.rsplit_once(':') {
        Some((host, p)) if !host.contains(':') => p.parse().map_or((a, None), |p| (host, Some(p))),
        _ => (a, None),
    }
}

/// IPv4 addresses only (Glidedesk runs on IPv4).
async fn dns(host: &str, port: u16) -> Option<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> =
        tokio::net::lookup_host((host, port)).await.ok()?.filter(SocketAddr::is_ipv4).collect();
    (!addrs.is_empty()).then_some(addrs)
}

/// Resolves what the user typed as the server (§14 B3): an IPv4 address
/// (`ip`, `ip:port`), or a **computer name** — matched against Glidedesk
/// servers announced on the network (display or computer name), then
/// `name.local` (mDNS) and DNS. Whichever answers first wins. IPv4 only.
async fn resolve(address: &str, interface: &str) -> Result<Vec<SocketAddr>, CoreError> {
    let a = address.trim();
    let ip_only = a.trim_start_matches('[').split(']').next().unwrap_or(a);
    if a.parse::<SocketAddr>().is_ok_and(|sa| sa.is_ipv6()) || ip_only.parse::<std::net::Ipv6Addr>().is_ok() {
        return Err(CoreError::Net(format!("'{a}' is an IPv6 address; Glidedesk uses IPv4 only")));
    }
    if let Ok(sa) = a.parse::<SocketAddr>() {
        return Ok(vec![sa]);
    }
    if let Ok(ip) = a.parse::<std::net::Ipv4Addr>() {
        return Ok(vec![SocketAddr::new(IpAddr::V4(ip), DEFAULT_PORT)]);
    }
    let (host, port) = split_port(a);
    if host.is_empty() || host.len() > 253 || host.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(CoreError::Net(format!("'{a}' is not a computer name or address")));
    }
    let announced = async {
        let Ok((_browser, mut rx)) = Browser::start(Some(interface)) else { return std::future::pending().await };
        while let Some(ev) = rx.recv().await {
            if let DiscoveryEvent::Found(ad) = ev
                && ad.protocol == PROTOCOL_VERSION
                && ad.matches_name(host)
            {
                return ad.addrs.iter().map(|ip| SocketAddr::new(*ip, port.unwrap_or(ad.port))).collect();
            }
        }
        std::future::pending().await
    };
    let by_dns = async {
        let p = port.unwrap_or(DEFAULT_PORT);
        let local = if host.contains('.') { None } else { dns(&format!("{host}.local"), p).await };
        match local {
            Some(a) => a,
            None => match dns(host, p).await {
                Some(a) => a,
                None => std::future::pending().await,
            },
        }
    };
    tokio::select! {
        a = announced => Ok(a),
        a = by_dns => Ok(a),
        () = tokio::time::sleep(Duration::from_secs(8)) => {
            Err(CoreError::Net(format!("can't find a computer called '{host}' on the network")))
        }
    }
}

/// The interface to use: the chosen one while it is up with an IPv4 address;
/// otherwise any (`""`) and a note saying why, so the link keeps working.
fn usable_interface(cfg: &Config) -> (String, Option<String>) {
    let name = cfg.client.interface.trim();
    if name.is_empty() {
        return (String::new(), None);
    }
    let all = glidedesk_net::list_interfaces();
    match all.iter().find(|i| i.name == name) {
        Some(i) if i.up && i.addrs.iter().any(|a| a.ip.is_ipv4()) => (name.to_owned(), None),
        Some(i) => (String::new(), Some(format!("{} ({name}) is off — using any network", i.friendly_name))),
        None => (String::new(), Some(format!("network {name} is not connected — using any network"))),
    }
}

/// IPv4 address of `interface` (`""` = any).
fn bind_ip(interface: &str) -> Option<IpAddr> {
    if interface.is_empty() {
        return None;
    }
    glidedesk_net::list_interfaces()
        .into_iter()
        .find(|i| i.name == interface)
        .and_then(|i| i.addrs.into_iter().map(|a| a.ip).find(|ip| ip.is_ipv4() && !ip.is_loopback()))
}

/// Reads frames on its own task until the stream ends or fails; the last item is
/// `Ok(None)` (clean end) or the error.
fn spawn_frames<T: serde::de::DeserializeOwned + Send + 'static>(
    mut reader: FrameReader,
) -> mpsc::Receiver<Result<Option<T>, glidedesk_net::NetError>> {
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(async move {
        loop {
            let item = reader.recv::<T>().await;
            let last = !matches!(item, Ok(Some(_)));
            if tx.send(item).await.is_err() || last {
                return;
            }
        }
    });
    rx
}

enum SessionEnd {
    Command(ClientCommand),
    Lost(String),
    Rejected(RejectReason),
    /// Password problem: retrying can't help (and would trip the server's
    /// lockout), so wait until the settings change or the user reconnects.
    Auth(String),
    Goodbye(GoodbyeReason),
}

fn reject_text(r: RejectReason) -> String {
    match r {
        RejectReason::IncompatibleProtocol => {
            "the server runs a different Glidedesk version; update both computers".into()
        }
        RejectReason::Blocked => "this computer is blocked on the server".into(),
        RejectReason::NotAllowed => "the server doesn't accept this computer".into(),
        RejectReason::ServerStopping => "the server is stopping".into(),
        RejectReason::RoleMismatch => "the other computer is not a server".into(),
        RejectReason::WrongPassword => "wrong password — check it under Server".into(),
        RejectReason::PasswordRequired => "the server needs a password — enter it under Server".into(),
        RejectReason::TooManyAttempts => "too many wrong passwords; the server blocks this computer for a while".into(),
        RejectReason::Forgotten => {
            "the server removed this computer (Forget) — press Reconnect to join it again".into()
        }
    }
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
    /// The user pressed Reconnect: join even if the server forgot us.
    rejoin: bool,
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
        rejoin: false,
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
                r.rejoin = true;
                backoff = BACKOFF_MIN;
                continue;
            }
            SessionEnd::Command(ClientCommand::FetchOffer) => {
                backoff = BACKOFF_MIN;
                continue;
            }
            SessionEnd::Rejected(reason)
                if matches!(
                    reason,
                    RejectReason::WrongPassword
                        | RejectReason::PasswordRequired
                        | RejectReason::TooManyAttempts
                        | RejectReason::Forgotten
                ) =>
            {
                let text = reject_text(reason);
                if !r.wait_for_change(&mut cmds, text).await {
                    break;
                }
                backoff = BACKOFF_MIN;
                continue;
            }
            SessionEnd::Auth(text) => {
                if !r.wait_for_change(&mut cmds, text).await {
                    break;
                }
                backoff = BACKOFF_MIN;
                continue;
            }
            SessionEnd::Rejected(reason) => {
                r.update(|v| {
                    v.state = LinkState::Rejected;
                    v.message = Some(reject_text(reason));
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
                Some(ClientCommand::Reconnect) => r.rejoin = true,
                Some(ClientCommand::FetchOffer) => {}
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
    /// Shows `text` as a refusal and waits for new settings or "Reconnect".
    /// Returns `false` when the client must stop.
    async fn wait_for_change(&mut self, cmds: &mut mpsc::Receiver<ClientCommand>, text: String) -> bool {
        warn!(reason = %text, "not connecting");
        self.update(|v| {
            v.state = LinkState::Rejected;
            v.message = Some(text);
            v.active = false;
            v.latency_ms = None;
        });
        loop {
            match cmds.recv().await {
                Some(ClientCommand::ApplyConfig(c)) => {
                    self.config = *c;
                    return true;
                }
                Some(ClientCommand::Reconnect) => {
                    self.rejoin = true;
                    return true;
                }
                Some(ClientCommand::FetchOffer) => {} // not connected: nothing to fetch
                Some(ClientCommand::Shutdown(_)) | None => return false,
            }
        }
    }

    fn update(&self, f: impl FnOnce(&mut ClientSideView)) {
        self.status.send_modify(|v| {
            f(v);
            v.local =
                MachineView { id: Some(self.config.device.id), name: self.name(), monitors: self.monitors.clone() };
            v.activity = self.sync.activity();
        });
    }

    fn name(&self) -> String {
        if self.config.device.name.is_empty() { self.host_name.clone() } else { self.config.device.name.clone() }
    }

    /// Finds candidate server addresses (configured or via mDNS).
    async fn find_server(&mut self, cmds: &mut mpsc::Receiver<ClientCommand>) -> Result<Vec<SocketAddr>, SessionEnd> {
        let (interface, note) = usable_interface(&self.config);
        if let Some(n) = &note {
            warn!("{n}");
        }
        let configured = self.config.client.server_address.trim().to_owned();
        if !configured.is_empty() {
            self.update(|v| {
                v.state = LinkState::Connecting;
                v.server_address = Some(configured.clone());
                v.message.clone_from(&note);
            });
            return tokio::select! {
                r = resolve(&configured, &interface) => r.map_err(|e| SessionEnd::Lost(e.to_string())),
                cmd = cmds.recv() => Err(SessionEnd::Command(cmd.unwrap_or(ClientCommand::Shutdown(GoodbyeReason::Stopping)))),
            };
        }
        self.update(|v| {
            v.state = LinkState::Searching;
            v.message.clone_from(&note);
        });
        let (_browser, mut rx) = Browser::start(Some(interface.as_str()))
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
        let (interface, _) = usable_interface(&self.config);
        let ordered: Vec<SocketAddr> = addrs.iter().copied().filter(SocketAddr::is_ipv4).collect();
        let mut last_err = if ordered.is_empty() {
            String::from("the server has no IPv4 address (Glidedesk uses IPv4 only)")
        } else {
            String::from("no address")
        };
        for addr in ordered {
            let ep = match glidedesk_net::client_endpoint(bind_ip(&interface), tuning) {
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
            features: {
                let f = Features::default().with(Features::LED_SYNC).with(Features::DRAW_CURSOR);
                if self.rejoin { f.with(Features::REJOIN) } else { f }
            },
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

    fn receive_stream(&self, kind: u8, stream: quinn::RecvStream, conn: &quinn::Connection, settings: &ClientSettings) {
        let (allow_clip, allow_files) =
            (settings.clipboard && settings.clipboard_receive, settings.files && settings.clipboard_receive);
        let (sync, limit, conn) = (self.sync.clone(), settings.clipboard_limit, conn.clone());
        let peer = self.status.borrow().server_name.clone().unwrap_or_else(|| "server".into());
        tokio::spawn(async move {
            // Offered files wait for a paste here (see `on_input`).
            if let Err(e) = sync.receive(kind, stream, conn, None, peer, allow_clip, allow_files, limit).await {
                warn!(error = %e, "receiving clipboard failed");
            }
        });
    }

    /// Keys from the server pass through the paste guard: ⌘V / Ctrl+V while
    /// files are only offered is held until they arrived (§14.2).
    fn on_input(
        &mut self,
        ev: Input,
        guard: &mut glidedesk_input::paste::PasteGuard,
        taken: &mpsc::Sender<glidedesk_proto::FileSetId>,
    ) -> bool {
        use glidedesk_input::paste::Verdict;
        let verdict = match ev {
            Input::Key { key, down } => guard.on_key(key, down, self.sync.offer_pending()),
            Input::ReleaseAll => {
                guard.reset();
                Verdict::Pass
            }
            _ => Verdict::Pass,
        };
        match verdict {
            Verdict::Pass => self.inject.send(InjectCmd::Input(ev)).is_ok(),
            Verdict::Swallow => true,
            Verdict::Fetch => {
                let Input::Key { key, .. } = ev else { return true };
                let replay = guard.replay(key);
                let (sync, inject, taken, notices) =
                    (self.sync.clone(), self.inject.clone(), taken.clone(), self.notices.clone());
                tokio::spawn(async move {
                    match sync.fetch_offer("you pasted here").await {
                        Ok(got) => {
                            for (k, down) in replay {
                                let _ = inject.send(InjectCmd::Input(Input::Key { key: k, down }));
                            }
                            if let Some(files) = got
                                && files.cut
                                && crate::sync::wait_taken(files.roots).await
                            {
                                let _ = taken.send(files.set).await;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "fetching offered files failed");
                            let _ = notices.send(Notice::Error { message: format!("Paste failed: {e}") }).await;
                        }
                    }
                });
                true
            }
        }
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
        let mut expected_proof: Option<[u8; glidedesk_net::auth::PROOF_LEN]> = None;
        let welcome = loop {
            match tokio::time::timeout(CONNECT_TIMEOUT, reader.recv::<Control>()).await {
                Ok(Ok(Some(Control::Welcome(w)))) => break w,
                Ok(Ok(Some(Control::Reject(r)))) => return SessionEnd::Rejected(r),
                Ok(Ok(Some(Control::AuthChallenge { salt, message }))) if expected_proof.is_none() => {
                    let password = self.config.client.password.clone();
                    if password.is_empty() {
                        let _ = control.send(&Control::Goodbye(GoodbyeReason::Disconnected)).await;
                        conn.close(0u32.into(), b"password required");
                        return SessionEnd::Rejected(RejectReason::PasswordRequired);
                    }
                    let mut exporter = [0u8; 32];
                    if conn.export_keying_material(&mut exporter, glidedesk_net::auth::EXPORTER_LABEL, b"").is_err() {
                        return SessionEnd::Lost("TLS exporter unavailable".into());
                    }
                    let answer = tokio::task::spawn_blocking(move || {
                        glidedesk_net::auth::client_answer(&password, &salt, &message, &exporter)
                    })
                    .await;
                    let answer = match answer {
                        Ok(Ok(a)) => a,
                        Ok(Err(e)) => return SessionEnd::Lost(format!("password check failed: {e}")),
                        Err(e) => return SessionEnd::Lost(e.to_string()),
                    };
                    expected_proof = Some(answer.expected_server_proof);
                    let reply = Control::AuthResponse { message: answer.message, proof: answer.proof.to_vec() };
                    if let Err(e) = control.send(&reply).await {
                        return SessionEnd::Lost(e.to_string());
                    }
                }
                Ok(Ok(_)) => return SessionEnd::Lost("unexpected reply from server".into()),
                Ok(Err(e)) => return SessionEnd::Lost(e.to_string()),
                Err(_) => return SessionEnd::Lost("server did not answer".into()),
            }
        };
        // Mutual check: a computer with a password only trusts a server that proves it too.
        if !self.config.client.password.is_empty() {
            let ok = match (&expected_proof, &welcome.auth_proof) {
                (Some(want), Some(got)) => glidedesk_net::auth::server_proof_ok(want, got),
                _ => false,
            };
            if !ok {
                conn.close(0u32.into(), b"server not verified");
                return SessionEnd::Auth(if welcome.auth_proof.is_none() {
                    "a password is set here, but the server doesn't use one — remove it here or set it on the server"
                        .into()
                } else {
                    "the server couldn't prove it knows the password; not connecting".into()
                });
            }
        }
        let mut settings: ClientSettings = welcome.settings;
        let (taken_tx, mut taken_rx) = mpsc::channel::<glidedesk_proto::FileSetId>(8);
        // The first unidirectional stream is the input stream; anything else
        // (clipboard, files) is handled on its own task.
        let input = loop {
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
            self.receive_stream(kind[0], s, &conn, &settings);
        };
        let mut paste_guard = glidedesk_input::paste::PasteGuard::new(Platform::current());
        // `FrameReader::recv` isn't cancel-safe (a frame read half-way would be lost
        // when another `select!` branch wins), so each stream gets its own reader task.
        let mut input = spawn_frames::<Input>(input);
        let mut reader = spawn_frames::<Control>(reader);
        let mut offer_watch = self.sync.hold_watch();
        self.preferred = Some(welcome.device_id);
        self.last_sent = None;
        self.rejoin = false;
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
            v.applied = Some(applied(&settings));
            // Still say so when the chosen network is off (we use another one).
            v.message = usable_interface(&self.config).1;
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
                            self.receive_stream(kind[0], s, &conn, &settings);
                        }
                    }
                    Err(e) => return SessionEnd::Lost(e.to_string()),
                },
                bi = conn.accept_bi() => match bi {
                    // The server pasted files this computer offered: send them.
                    Ok((send, mut recv)) => {
                        let (sync, peer) = (self.sync.clone(), server_name.clone());
                        tokio::spawn(async move {
                            let mut kind = [0u8; 1];
                            let ok = tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut kind))
                                .await
                                .is_ok_and(|r| r.is_ok());
                            if ok && kind[0] == stream_kind::FETCH {
                                sync.serve_fetch(send, recv, None, peer).await;
                            } else {
                                let _ = recv.stop(0u32.into());
                            }
                        });
                    }
                    Err(e) => return SessionEnd::Lost(e.to_string()),
                },
                Some(set) = taken_rx.recv() => {
                    if let Err(e) = control.send(&Control::FilesTaken(set)).await {
                        return SessionEnd::Lost(e.to_string());
                    }
                }
                Ok(()) = offer_watch.changed() => {
                    let _ = *offer_watch.borrow_and_update();
                    let offer = self.sync.offer_description();
                    self.status.send_modify(|v| v.offer = offer);
                }
                _ = transfer_tick.tick(), if self.sync.transfers.any_active() => {
                    let t = self.sync.transfers.snapshot();
                    self.status.send_modify(|v| v.transfers = t);
                }
                ev = input.recv() => match ev.unwrap_or(Ok(None)) {
                    Ok(Some(ev)) => {
                        if !self.on_input(ev, &mut paste_guard, &taken_tx) {
                            return SessionEnd::Lost("injection thread stopped".into());
                        }
                    }
                    Ok(None) => return SessionEnd::Lost("input stream closed".into()),
                    Err(e) => return SessionEnd::Lost(e.to_string()),
                },
                msg = reader.recv() => match msg.unwrap_or(Ok(None)) {
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
                            v.applied = Some(applied(&settings));
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
                    let sync = self.sync.clone();
                    tokio::task::spawn_blocking(move || sync.observe());
                    self.update(|_| {}); // refreshes the activity log
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
                                || c.client.interface != self.config.client.interface
                                || c.client.password != self.config.client.password;
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
                        ClientCommand::FetchOffer => {
                            let (sync, taken, notices) = (self.sync.clone(), taken_tx.clone(), self.notices.clone());
                            tokio::spawn(async move {
                                match sync.fetch_offer("you chose Get them now").await {
                                    Ok(Some(files)) if files.cut => {
                                        if crate::sync::wait_taken(files.roots).await {
                                            let _ = taken.send(files.set).await;
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        let _ = notices.send(Notice::Error { message: format!("Receiving files failed: {e}") }).await;
                                    }
                                }
                            });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_split_handles_names() {
        assert_eq!(split_port("office-pc"), ("office-pc", None));
        assert_eq!(split_port("office-pc:9000"), ("office-pc", Some(9000)));
        assert_eq!(split_port("name:notaport"), ("name:notaport", None));
    }

    #[tokio::test]
    async fn ip_addresses_resolve_without_lookups_and_ipv6_is_refused() {
        let a = resolve("192.168.1.20", "").await.unwrap();
        assert_eq!(a, vec![SocketAddr::new("192.168.1.20".parse().unwrap(), DEFAULT_PORT)]);
        let b = resolve("192.168.1.20:9000", "").await.unwrap();
        assert_eq!(b[0].port(), 9000);
        for v6 in ["fe80::1", "[fe80::1]:9000", "::1"] {
            let e = resolve(v6, "").await.unwrap_err().to_string();
            assert!(e.contains("IPv4 only"), "{v6}: {e}");
        }
        assert!(resolve("bad name", "").await.is_err());
    }

    #[test]
    fn a_chosen_interface_that_does_not_exist_falls_back_to_any() {
        let mut cfg = Config::default();
        cfg.client.interface = "gd-no-such-if0".into();
        let (iface, note) = usable_interface(&cfg);
        assert_eq!(iface, "");
        assert!(note.unwrap().contains("using any network"));
    }
}
