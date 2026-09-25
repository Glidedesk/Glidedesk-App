//! Server runtime: one "hub" task owns all state (engine, clients, health)
//! and reacts to capture events, network events, timers and commands.
//! Per-connection reader/writer tasks only move bytes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use glidedesk_config::{ClientEntry, Config, IpFilter, RemapPreset};
use glidedesk_input::{Capture, CaptureControl, CaptureEvent, Hotkey, Pressed, Remap, hotkey};
use glidedesk_layout::{EdgeContext, Engine, Focus, Layout, LinkSpec, Machine, Outcome, Warning};
use glidedesk_net::{Admission, Advertiser, BindStatus, FrameReader, FrameWriter, Server as NetServer, Tuning};
use glidedesk_proto::{
    ClientPrefs, ClientSettings, Control, DeviceId, GoodbyeReason, Hello, Input, KeyCode, MAX_CONTROL_FRAME,
    MAX_INPUT_FRAME, MonitorInfo, PROTOCOL_VERSION, Platform, Point, RejectReason, Side, Welcome,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::health::{Health, HealthConfig, HealthState};
use crate::{CoreError, MonitorSource, Notice};
use glidedesk_ipc::views::{BindView, ClientView, MachineView, ServerView};

/// Includes the client's Argon2 password hashing when a password is set.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MONITOR_POLL: Duration = Duration::from_secs(2);
const INPUT_QUEUE: usize = 1024;
const CONTROL_QUEUE: usize = 64;
const MAX_CLIENTS: usize = 64;
const SECURE_INPUT_WARNING: &str = "typing";
const EDGE_NOTE: &str = "The cursor stayed here";

/// Commands from the agent / UI / tray.
#[derive(Debug)]
pub enum ServerCommand {
    SwitchTo(DeviceId),
    SwitchHome,
    SwitchNext,
    SwitchPrevious,
    ToggleLock,
    SetLocked(bool),
    /// Drop every client link; clients reconnect on their own.
    ReconnectAll,
    Identify,
    Disconnect(DeviceId),
    /// New settings; `removed` are clients the user chose to forget. Clients
    /// missing from `config` for any other reason are kept.
    ApplyConfig {
        config: Box<Config>,
        removed: Vec<DeviceId>,
    },
    /// Fetch offered files now, without a paste to replay.
    FetchOffer,
    Shutdown(GoodbyeReason),
}

#[derive(Debug)]
pub struct ServerHandle {
    pub commands: mpsc::Sender<ServerCommand>,
    pub status: watch::Receiver<ServerView>,
    pub notices: mpsc::Receiver<Notice>,
    pub task: JoinHandle<()>,
}

pub struct ServerDeps {
    pub capture: Capture,
    pub monitors: MonitorSource,
    /// Monitors of clients seen before (from the state file), so offline
    /// clients still appear in the layout.
    pub known_monitors: HashMap<DeviceId, Vec<MonitorInfo>>,
    pub app_version: String,
    pub host_name: String,
    /// System clipboard (`None` where unsupported).
    pub clipboard: Option<Box<dyn glidedesk_clipboard::Clipboard>>,
}

impl std::fmt::Debug for ServerDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerDeps").field("app_version", &self.app_version).finish_non_exhaustive()
    }
}

/// Starts listening and returns immediately; the hub runs until `Shutdown`.
pub fn start(config: Config, deps: ServerDeps) -> Result<ServerHandle, CoreError> {
    let local_monitors = (deps.monitors)().map_err(|e| CoreError::Input(e.to_string()))?;
    let net_cfg = &config.server.network;
    let all_ifaces = glidedesk_net::list_interfaces();
    let plan = glidedesk_net::resolve_bind(
        net_cfg.mode,
        &net_cfg.interfaces,
        &net_cfg.addresses,
        net_cfg.port,
        &all_ifaces,
        net_cfg.ipv6,
    );
    let tuning = Tuning {
        keep_alive: Duration::from_millis(u64::from(config.server.health.interval_ms)),
        idle_timeout: Duration::from_millis(u64::from(config.server.health.idle_timeout_ms)),
    };
    let (net, bind) = NetServer::bind(&plan, tuning).map_err(|e| CoreError::Net(e.to_string()))?;
    let admission = Admission {
        filter: IpFilter::new(&net_cfg.allow_list, &net_cfg.block_list).unwrap_or_default(),
        same_subnet: net_cfg.same_subnet_only.then(|| glidedesk_net::interfaces::bound_ifaddrs(&plan, &all_ifaces)),
    };
    let name = if config.device.name.is_empty() { deps.host_name.clone() } else { config.device.name.clone() };
    let advertiser = if net_cfg.discovery {
        let port = net.local_addrs().first().map_or(net_cfg.port, SocketAddr::port);
        let scope = match net_cfg.mode {
            glidedesk_config::BindMode::All => None,
            _ => Some((net_cfg.interfaces.as_slice(), net_cfg.addresses.as_slice())),
        };
        match Advertiser::start(config.device.id, &name, &deps.host_name, &deps.app_version, port, scope, net_cfg.ipv6)
        {
            Ok(a) => Some(a),
            Err(e) => {
                warn!(error = %e, "mDNS announcement failed; clients must use the address");
                None
            }
        }
    } else {
        None
    };

    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (status_tx, status_rx) = watch::channel(ServerView::default());
    let (notice_tx, notice_rx) = mpsc::channel(256);
    let (event_tx, event_rx) = mpsc::channel(1024);

    let mut hub = Hub::new(config, deps, local_monitors, name, net, bind, admission, status_tx, notice_tx, event_tx);
    hub.advertiser = advertiser;
    let task = tokio::spawn(async move { hub.run(cmd_rx, event_rx).await });
    Ok(ServerHandle { commands: cmd_tx, status: status_rx, notices: notice_rx, task })
}

// ---------------------------------------------------------------------------

enum HubEvent {
    /// A client opened a clipboard/files stream.
    Stream {
        id: DeviceId,
        generation: u64,
        kind: u8,
        stream: quinn::RecvStream,
    },
    /// The client moved files we sent as "cut": tell the sender side.
    Taken {
        id: DeviceId,
        set: glidedesk_proto::FileSetId,
    },
    Hello {
        conn: quinn::Connection,
        writer: FrameWriter,
        reader: FrameReader,
        hello: Box<Hello>,
        addr: SocketAddr,
        /// Our password proof for the `Welcome` (only when a password is set).
        proof: Option<Vec<u8>>,
    },
    Control {
        id: DeviceId,
        generation: u64,
        msg: Control,
    },
    Closed {
        id: DeviceId,
        generation: u64,
        reason: String,
    },
}

struct Conn {
    generation: u64,
    connection: quinn::Connection,
    addr: SocketAddr,
    control: mpsc::Sender<Control>,
    input: mpsc::Sender<Input>,
    pings: VecDeque<(u32, Instant)>,
}

struct Slot {
    name: String,
    platform: Option<Platform>,
    app_version: Option<String>,
    monitors: Vec<MonitorInfo>,
    prefs: ClientPrefs,
    health: Health,
    conn: Option<Conn>,
    last_seen: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Lock,
    Home,
    Next,
    Previous,
    ReconnectAll,
    Identify,
}

struct Hub {
    config: Config,
    local_id: DeviceId,
    local_name: String,
    local_platform: Platform,
    local_monitors: Vec<MonitorInfo>,
    monitors: MonitorSource,
    capture: Arc<dyn CaptureControl>,
    capture_events: Option<mpsc::Receiver<CaptureEvent>>,
    engine: Engine,
    slots: HashMap<DeviceId, Slot>,
    pressed: Pressed,
    consumed_keys: HashSet<KeyCode>,
    hotkeys: Vec<(Hotkey, Action)>,
    net: NetServer,
    bind: Vec<BindStatus>,
    admission: Admission,
    advertiser: Option<Advertiser>,
    status_tx: watch::Sender<ServerView>,
    notice_tx: mpsc::Sender<Notice>,
    event_tx: mpsc::Sender<HubEvent>,
    epoch: Instant,
    next_generation: u64,
    last_local_pos: Point,
    app_version: String,
    warnings: Vec<String>,
    sync: Arc<crate::sync::Sync>,
    /// Cached "is a full-screen app focused?" (checked at most twice a second).
    fullscreen: Option<(Instant, bool)>,
    /// Clipboard sequence last sent to each client.
    last_sent: HashMap<DeviceId, u64>,
    /// Diagnostics: first capture event seen, last "why no switch" log line.
    capture_seen: bool,
    last_edge_note: Option<(Instant, String)>,
    /// Last layout summary logged ("PC is on the right of Mac").
    layout_summary: String,
    /// Server password (PLAN §14.1); `None` = open.
    verifier: Option<Arc<glidedesk_net::auth::Verifier>>,
    throttle: Arc<glidedesk_net::auth::Throttle>,
}

fn health_cfg(c: &Config) -> HealthConfig {
    HealthConfig {
        interval: Duration::from_millis(u64::from(c.server.health.interval_ms)),
        miss_threshold: c.server.health.miss_threshold,
        degraded_latency: Duration::from_millis(u64::from(c.server.health.degraded_latency_ms)),
    }
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Units per device-independent pixel: macOS uses points (1), Windows physical pixels (scale).
fn units_per_dip(platform: Option<Platform>, monitors: &[MonitorInfo]) -> f64 {
    let primary = monitors.iter().find(|m| m.primary).or_else(|| monitors.first());
    match (platform, primary) {
        (Some(Platform::Windows), Some(m)) if m.scale > 0.0 => f64::from(m.scale),
        _ => 1.0,
    }
}

impl Hub {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: Config,
        deps: ServerDeps,
        local_monitors: Vec<MonitorInfo>,
        local_name: String,
        net: NetServer,
        bind: Vec<BindStatus>,
        admission: Admission,
        status_tx: watch::Sender<ServerView>,
        notice_tx: mpsc::Sender<Notice>,
        event_tx: mpsc::Sender<HubEvent>,
    ) -> Self {
        let local_id = config.device.id;
        let hcfg = health_cfg(&config);
        let mut slots = HashMap::new();
        for entry in &config.server.clients {
            let monitors = deps.known_monitors.get(&entry.id).cloned().unwrap_or_default();
            slots.insert(
                entry.id,
                Slot {
                    name: entry.name.clone(),
                    platform: None,
                    app_version: None,
                    monitors,
                    prefs: ClientPrefs::default(),
                    health: Health::new(hcfg),
                    conn: None,
                    last_seen: None,
                },
            );
        }
        let engine = Engine::new(local_id, Layout::default(), config.server.switching.clone());
        let mut hub = Self {
            local_id,
            local_name,
            local_platform: Platform::current(),
            local_monitors,
            monitors: deps.monitors,
            capture: deps.capture.control.clone(),
            capture_events: Some(deps.capture.events),
            engine,
            slots,
            pressed: Pressed::default(),
            consumed_keys: HashSet::new(),
            hotkeys: Vec::new(),
            net,
            bind,
            admission,
            advertiser: None,
            status_tx,
            notice_tx,
            event_tx,
            epoch: Instant::now(),
            next_generation: 1,
            last_local_pos: Point::default(),
            app_version: deps.app_version,
            warnings: Vec::new(),
            sync: crate::sync::Sync::new(deps.clipboard),
            fullscreen: None,
            last_sent: HashMap::new(),
            capture_seen: false,
            last_edge_note: None,
            layout_summary: String::new(),
            verifier: config
                .server
                .network
                .password
                .as_ref()
                .and_then(|p| glidedesk_net::auth::Verifier::from_hex(&p.salt, &p.key))
                .map(Arc::new),
            throttle: Arc::default(),
            config,
        };
        hub.load_hotkeys();
        hub.rebuild_layout();
        hub
    }

    async fn run(mut self, mut cmds: mpsc::Receiver<ServerCommand>, mut events: mpsc::Receiver<HubEvent>) {
        let Some(mut capture) = self.capture_events.take() else { return };
        let mut heartbeat = tokio::time::interval(health_cfg(&self.config).interval);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut transfer_tick = tokio::time::interval(Duration::from_millis(500));
        let mut paste_hold = self.sync.hold_watch();
        let mut monitor_poll = tokio::time::interval(MONITOR_POLL);
        monitor_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        self.publish();
        info!(addrs = ?self.net.local_addrs(), "server running");

        loop {
            let deadline = self.engine.pending_deadline();
            tokio::select! {
                biased;
                Some(ev) = capture.recv() => self.on_capture(ev),
                Some(ev) = events.recv() => self.on_event(ev),
                cmd = cmds.recv() => match cmd {
                    Some(ServerCommand::Shutdown(reason)) => {
                        self.shutdown(reason).await;
                        break;
                    }
                    None => {
                        self.shutdown(GoodbyeReason::Stopping).await;
                        break;
                    }
                    Some(c) => self.on_command(c),
                },
                incoming = self.net.accept() => match incoming {
                    Some(inc) => self.on_incoming(inc),
                    None => break,
                },
                _ = heartbeat.tick() => self.on_heartbeat(),
                _ = monitor_poll.tick() => {
                    self.poll_monitors();
                    self.check_secure_input();
                    self.expire_edge_note();
                }
                _ = transfer_tick.tick(), if self.sync.transfers.any_active() => self.publish(),
                Ok(()) = paste_hold.changed() => {
                    let on = *paste_hold.borrow_and_update();
                    self.capture.set_paste_hold(on);
                    self.publish();
                }
                () = sleep_until(deadline) => {
                    let out = self.engine.poll(Instant::now());
                    self.apply(out, 0, 0);
                }
            }
        }
        info!("server stopped");
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await,
        None => std::future::pending().await,
    }
}

// ---------------------------------------------------------------------------
// hub: input
// ---------------------------------------------------------------------------

impl Hub {
    fn ctx(&self) -> EdgeContext {
        EdgeContext { now: Instant::now(), mods: self.pressed.mods(), fullscreen: false }
    }

    /// A full-screen app (not on the allow list) is focused on this machine.
    fn fullscreen_blocked(&mut self) -> bool {
        if !self.config.server.switching.block_fullscreen {
            return false;
        }
        let now = Instant::now();
        if let Some((at, v)) = self.fullscreen
            && now.duration_since(at) < Duration::from_millis(500)
        {
            return v;
        }
        let allow = &self.config.server.fullscreen_allow;
        let v =
            glidedesk_input::fullscreen_app().is_some_and(|app| !allow.iter().any(|a| a.eq_ignore_ascii_case(&app)));
        self.fullscreen = Some((now, v));
        v
    }

    /// Logs (at most every few seconds) why pushing an edge didn't switch, so
    /// "the mouse doesn't go to the other computer" can be diagnosed from the log.
    fn note_edge(&mut self, pos: Point, dx: i32, dy: i32, ctx: &EdgeContext) {
        let Some(mut why) = self.engine.explain_edge(self.local_id, pos, dx, dy, ctx) else { return };
        if why == "not blocked" {
            // Allowed, yet no switch: the one case that must never be silent.
            why = format!("the push was allowed but did not switch (dx={dx}, dy={dy}) — please report this");
        }
        let now = Instant::now();
        if self
            .last_edge_note
            .as_ref()
            .is_some_and(|(t, w)| *w == why && now.duration_since(*t) < Duration::from_secs(5))
        {
            return;
        }
        info!(x = pos.x, y = pos.y, "edge push did not switch: {why}");
        // Also shown in the window for a few seconds (cleared on the heartbeat).
        self.warnings.retain(|w| !w.starts_with(EDGE_NOTE));
        self.warnings.push(format!("{EDGE_NOTE}: {why}"));
        self.last_edge_note = Some((now, why));
        self.publish();
    }

    /// Drops the "why no switch" note once it's a few seconds old.
    fn expire_edge_note(&mut self) {
        let old = self.last_edge_note.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(8));
        if old && self.warnings.iter().any(|w| w.starts_with(EDGE_NOTE)) {
            self.warnings.retain(|w| !w.starts_with(EDGE_NOTE));
            self.publish();
        }
    }

    fn focused(&self) -> Option<DeviceId> {
        match self.engine.focus() {
            Focus::Remote { machine, .. } => Some(machine),
            Focus::Local => None,
        }
    }

    fn on_capture(&mut self, ev: CaptureEvent) {
        if !self.capture_seen {
            self.capture_seen = true;
            info!("keyboard/mouse capture is receiving events");
        }
        match ev {
            CaptureEvent::Motion { pos, dx, dy } => {
                let ctx = self.ctx();
                let out = match self.engine.focus() {
                    Focus::Local => {
                        let ctx = EdgeContext { fullscreen: self.fullscreen_blocked(), ..ctx };
                        self.last_local_pos = pos;
                        // Never switch while a button is held: its release would be lost locally.
                        let (dx, dy) = if self.pressed.any_button() { (0, 0) } else { (dx, dy) };
                        let out = self.engine.on_local_move(pos, dx, dy, ctx);
                        if out == Outcome::None && (dx != 0 || dy != 0) {
                            self.note_edge(pos, dx, dy, &ctx);
                        }
                        out
                    }
                    Focus::Remote { .. } => self.engine.on_remote_move(dx, dy, ctx),
                };
                self.apply(out, dx, dy);
            }
            CaptureEvent::Button { button, down } => {
                self.pressed.button(button, down);
                if let Some(id) = self.focused() {
                    self.send_input(id, Input::Button { button, down });
                }
            }
            CaptureEvent::Wheel { dx, dy } => {
                if let Some(id) = self.focused() {
                    let (speed, invert) = self.scroll_for(id);
                    let sign = if invert { -1.0 } else { 1.0 };
                    #[allow(clippy::cast_possible_truncation)]
                    let scale = |v: i32| (f64::from(v) * speed * sign).round() as i32;
                    self.send_input(id, Input::Wheel { dx: scale(dx), dy: scale(dy) });
                }
            }
            CaptureEvent::Key { key, down } => self.on_key(key, down),
            CaptureEvent::PasteRequested { key } => self.paste_offered_files(Some(key)),
            CaptureEvent::DisplaysChanged => self.poll_monitors(),
            CaptureEvent::Interrupted => {
                // Events may have been lost: make sure nothing stays pressed remotely.
                if let Some(id) = self.focused() {
                    self.send_input(id, Input::ReleaseAll);
                }
                self.pressed = Pressed::default();
            }
        }
    }

    fn on_key(&mut self, key: KeyCode, down: bool) {
        let mods_before = self.pressed.mods();
        let fresh = self.pressed.key(key, down);
        if down
            && fresh
            && let Some((_, action)) = self.hotkeys.iter().find(|(h, _)| h.matches(key, mods_before)).copied()
        {
            self.consumed_keys.insert(key);
            self.run_action(action);
            return;
        }
        if self.consumed_keys.contains(&key) {
            if !down {
                self.consumed_keys.remove(&key);
            }
            return;
        }
        if let Some(id) = self.focused() {
            let remap = self.remap_for(id);
            self.send_input(id, Input::Key { key: remap.apply(key), down });
        }
    }

    fn run_action(&mut self, action: Action) {
        let out = match action {
            Action::Lock => {
                let locked = !self.engine.locked();
                self.engine.set_locked(locked);
                self.publish();
                Outcome::None
            }
            Action::Home => self.engine.switch_to(self.local_id),
            Action::Next | Action::Previous => {
                let order = self.switch_order();
                let cur = self.focused().unwrap_or(self.local_id);
                let idx = order.iter().position(|m| *m == cur).unwrap_or(0);
                let n = order.len();
                let next = if matches!(action, Action::Next) { (idx + 1) % n } else { (idx + n - 1) % n };
                self.engine.switch_to(order[next])
            }
            Action::ReconnectAll => {
                self.reconnect_all();
                Outcome::None
            }
            Action::Identify => {
                self.identify();
                Outcome::None
            }
        };
        self.apply(out, 0, 0);
    }

    /// Local machine first, then reachable clients in config order.
    fn switch_order(&self) -> Vec<DeviceId> {
        let mut v = vec![self.local_id];
        v.extend(
            self.config
                .server
                .clients
                .iter()
                .map(|c| c.id)
                .filter(|id| self.slots.get(id).is_some_and(|s| s.health.state().reachable())),
        );
        v
    }

    fn apply(&mut self, out: Outcome, dx: i32, dy: i32) {
        match out {
            Outcome::None => {}
            Outcome::Move { pos } => {
                if let Some(id) = self.focused() {
                    if self.entry(id).is_some_and(|e| e.relative_mouse) {
                        self.send_input(id, Input::MouseRel { dx, dy });
                    } else {
                        self.send_input(id, Input::MouseAbs(pos));
                    }
                }
            }
            Outcome::Enter { machine, pos, previous } => {
                match previous {
                    Some(prev) => self.leave(prev),
                    None => self.capture.set_grab(true),
                }
                let leds = self.capture.leds();
                self.send_control(machine, Control::Enter { pos, leds });
                self.send_input(machine, Input::MouseAbs(pos));
                // Modifiers held while crossing (e.g. Shift-drag) go along.
                let remap = self.remap_for(machine);
                let held: Vec<KeyCode> = (0xE0..=0xE7).map(KeyCode).filter(|k| self.pressed.is_down(*k)).collect();
                for k in held {
                    self.send_input(machine, Input::Key { key: remap.apply(k), down: true });
                }
                self.push_clipboard(machine);
                self.check_secure_input();
                info!(client = %self.name_of(machine), x = pos.x, y = pos.y, "cursor went to another computer");
                self.publish();
            }
            Outcome::Return { pos, previous } => {
                self.leave(previous);
                self.capture.set_grab(false);
                self.capture.warp(pos);
                self.last_local_pos = pos;
                info!(x = pos.x, y = pos.y, "cursor came back to this computer");
                self.publish();
            }
        }
    }

    fn leave(&mut self, id: DeviceId) {
        self.send_input(id, Input::ReleaseAll);
        self.send_control(id, Control::Leave);
    }

    fn entry(&self, id: DeviceId) -> Option<&ClientEntry> {
        self.config.server.client(&id)
    }

    fn remap_for(&self, id: DeviceId) -> Remap {
        let slot = self.slots.get(&id);
        let preset = slot
            .and_then(|s| s.prefs.key_remap)
            .map(|v| match v {
                0 => RemapPreset::Auto,
                2 => RemapPreset::SwapCtrlMeta,
                _ => RemapPreset::None,
            })
            .or_else(|| self.entry(id).map(|e| e.key_remap))
            .unwrap_or_default();
        let client_platform = slot.and_then(|s| s.platform).unwrap_or(self.local_platform);
        Remap::new(preset, self.local_platform, client_platform)
    }

    fn scroll_for(&self, id: DeviceId) -> (f64, bool) {
        let entry = self.entry(id);
        let prefs = self.slots.get(&id).map(|s| &s.prefs);
        let speed = prefs.and_then(|p| p.scroll_speed).map(f64::from).or(entry.map(|e| e.scroll_speed)).unwrap_or(1.0);
        let invert = prefs.and_then(|p| p.scroll_invert).or(entry.map(|e| e.scroll_invert)).unwrap_or(false);
        (speed.clamp(0.1, 10.0), invert)
    }

    fn send_input(&mut self, id: DeviceId, ev: Input) {
        let Some(conn) = self.slots.get(&id).and_then(|s| s.conn.as_ref()) else { return };
        match conn.input.try_send(ev) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) if matches!(ev, Input::MouseAbs(_) | Input::MouseRel { .. }) => {
                // Motion is replaceable; drop it under back-pressure.
            }
            Err(_) => {
                warn!(client = %id, "client is not keeping up; disconnecting");
                self.drop_client(id, "input queue overflow");
            }
        }
    }

    fn send_control(&mut self, id: DeviceId, msg: Control) {
        let Some(conn) = self.slots.get(&id).and_then(|s| s.conn.as_ref()) else { return };
        if conn.control.try_send(msg).is_err() {
            self.drop_client(id, "control queue overflow");
        }
    }
}

// ---------------------------------------------------------------------------
// hub: connections & health
// ---------------------------------------------------------------------------

impl Hub {
    fn on_incoming(&mut self, inc: quinn::Incoming) {
        let addr = inc.remote_address();
        if !self.admission.admits(addr.ip()) {
            debug!(%addr, "connection refused by filter");
            inc.refuse();
            return;
        }
        if self.throttle.blocked(addr.ip(), Instant::now()) {
            debug!(%addr, "refused: too many wrong passwords");
            inc.refuse();
            return;
        }
        if self.slots.values().filter(|s| s.conn.is_some()).count() >= MAX_CLIENTS {
            inc.refuse();
            return;
        }
        let tx = self.event_tx.clone();
        let (verifier, throttle) = (self.verifier.clone(), self.throttle.clone());
        tokio::spawn(async move {
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(inc, verifier, throttle)).await {
                Ok(Ok((conn, writer, reader, hello, proof))) => {
                    let _ =
                        tx.send(HubEvent::Hello { conn, writer, reader, hello: Box::new(hello), addr, proof }).await;
                }
                Ok(Err(e)) => debug!(%addr, error = %e, "handshake failed"),
                Err(_) => debug!(%addr, "handshake timed out"),
            }
        });
    }

    fn on_event(&mut self, ev: HubEvent) {
        match ev {
            HubEvent::Hello { conn, writer, reader, hello, addr, proof } => {
                self.on_hello(conn, writer, reader, *hello, addr, proof);
            }
            HubEvent::Control { id, generation, msg } => {
                if self.slots.get(&id).and_then(|s| s.conn.as_ref()).is_some_and(|c| c.generation == generation) {
                    self.on_control(id, msg);
                }
            }
            HubEvent::Closed { id, generation, reason } => {
                if self.slots.get(&id).and_then(|s| s.conn.as_ref()).is_some_and(|c| c.generation == generation) {
                    self.drop_client(id, &reason);
                }
            }
            HubEvent::Stream { id, generation, kind, stream } => {
                if self.slots.get(&id).and_then(|s| s.conn.as_ref()).is_some_and(|c| c.generation == generation) {
                    self.on_stream(id, kind, stream);
                }
            }
            HubEvent::Taken { id, set } => self.send_control(id, Control::FilesTaken(set)),
        }
    }

    fn on_hello(
        &mut self,
        conn: quinn::Connection,
        writer: FrameWriter,
        reader: FrameReader,
        hello: Hello,
        addr: SocketAddr,
        proof: Option<Vec<u8>>,
    ) {
        let id = hello.device_id;
        let reject = if hello.protocol != PROTOCOL_VERSION {
            Some(RejectReason::IncompatibleProtocol)
        } else if id == self.local_id {
            Some(RejectReason::RoleMismatch)
        } else if self.entry(id).is_some_and(|e| e.blocked) {
            Some(RejectReason::Blocked)
        } else {
            None
        };
        if let Some(reason) = reject {
            info!(client = %id, ?reason, "client rejected");
            tokio::spawn(async move {
                let mut w = writer;
                let _ = w.send(&Control::Reject(reason)).await;
                w.finish();
                tokio::time::sleep(Duration::from_millis(200)).await;
                conn.close(1u32.into(), b"rejected");
            });
            return;
        }
        // Replace an older link of the same client.
        if self.slots.get(&id).is_some_and(|s| s.conn.is_some()) {
            self.drop_client(id, "replaced by a new connection");
        }
        let is_new = self.entry(id).is_none();
        let name = sanitize_name(&hello.name);
        if is_new {
            self.config.server.clients.push(ClientEntry { id, name: name.clone(), ..Default::default() });
            self.auto_place(id);
            let _ = self.notice_tx.try_send(Notice::ConfigChanged(Box::new(self.config.clone())));
        } else if let Some(e) = self.config.server.clients.iter_mut().find(|e| e.id == id)
            && e.name != name
        {
            e.name.clone_from(&name);
            let _ = self.notice_tx.try_send(Notice::ConfigChanged(Box::new(self.config.clone())));
        }

        let generation = self.next_generation;
        self.next_generation += 1;
        let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
        let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE);
        spawn_control_writer(writer, control_rx, id, generation, self.event_tx.clone());
        spawn_input_writer(conn.clone(), input_rx, id, generation, self.event_tx.clone());
        spawn_reader(reader, id, generation, self.event_tx.clone());
        spawn_uni_acceptor(conn.clone(), id, generation, self.event_tx.clone());
        spawn_fetch_server(conn.clone(), id, name.clone(), self.sync.clone());
        self.last_sent.remove(&id);

        let hcfg = health_cfg(&self.config);
        let slot = self.slots.entry(id).or_insert_with(|| Slot {
            name: name.clone(),
            platform: None,
            app_version: None,
            monitors: Vec::new(),
            prefs: ClientPrefs::default(),
            health: Health::new(hcfg),
            conn: None,
            last_seen: None,
        });
        slot.name.clone_from(&name);
        slot.platform = Some(hello.platform);
        slot.app_version = Some(sanitize_name(&hello.app_version));
        slot.monitors = sanitize_monitors(hello.monitors);
        slot.prefs = hello.prefs;
        slot.health.connected(Instant::now());
        slot.last_seen = Some(SystemTime::now());
        slot.conn = Some(Conn {
            generation,
            connection: conn,
            addr,
            control: control_tx,
            input: input_tx,
            pings: VecDeque::with_capacity(8),
        });
        let monitors = slot.monitors.clone();
        let welcome = Control::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            app_version: self.app_version.clone(),
            device_id: self.local_id,
            name: self.local_name.clone(),
            platform: self.local_platform,
            settings: self.settings_for(id),
            auth_proof: proof,
        });
        self.send_control(id, welcome);
        let _ = self.notice_tx.try_send(Notice::MonitorsSeen { id, monitors });
        let notify = self.entry(id).is_none_or(|e| e.notifications);
        let _ = self.notice_tx.try_send(if is_new {
            Notice::NewClient { id, name: name.clone(), address: addr.ip().to_string() }
        } else {
            Notice::ClientOnline { id, name: name.clone(), notify }
        });
        info!(client = %id, %addr, "client connected");
        self.rebuild_layout();
        self.publish();
    }

    fn on_control(&mut self, id: DeviceId, msg: Control) {
        match msg {
            Control::Pong { seq, status, .. } => {
                let Some(slot) = self.slots.get_mut(&id) else { return };
                let sent = slot.conn.as_mut().and_then(|c| {
                    let pos = c.pings.iter().position(|(s, _)| *s == seq)?;
                    let (_, t) = c.pings.remove(pos)?;
                    Some(t)
                });
                if let Some(sent) = sent {
                    let before = slot.health.state();
                    slot.health.pong(Instant::now(), sent, status);
                    slot.last_seen = Some(SystemTime::now());
                    if before.reachable() != slot.health.state().reachable() || before != slot.health.state() {
                        let reachable = slot.health.state().reachable();
                        let out = self.engine.set_available(id, reachable);
                        self.apply(out, 0, 0);
                        self.publish();
                    }
                }
            }
            Control::MonitorsChanged(m) => {
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.monitors = sanitize_monitors(m);
                    let monitors = slot.monitors.clone();
                    let _ = self.notice_tx.try_send(Notice::MonitorsSeen { id, monitors });
                    self.rebuild_layout();
                    self.publish();
                }
            }
            Control::Prefs(p) => {
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.prefs = p;
                    let settings = self.settings_for(id);
                    self.send_control(id, Control::Settings(settings));
                    self.rebuild_layout();
                }
            }
            Control::FilesTaken(set) => {
                tokio::spawn(self.sync.clone().files_taken(set, Some(id)));
            }
            Control::Goodbye(reason) => {
                debug!(client = %id, ?reason, "client said goodbye");
                self.drop_client(id, "client left");
            }
            // Messages only a server sends, or not expected here: ignore.
            _ => {}
        }
    }

    fn drop_client(&mut self, id: DeviceId, reason: &str) {
        let Some(slot) = self.slots.get_mut(&id) else { return };
        let Some(conn) = slot.conn.take() else { return };
        conn.connection.close(0u32.into(), reason.as_bytes());
        slot.health.disconnected();
        let name = slot.name.clone();
        info!(client = %id, reason, "client disconnected");
        let out = self.engine.set_available(id, false);
        self.apply(out, 0, 0);
        let notify = self.entry(id).is_none_or(|e| e.notifications) && self.config.server.visuals.notify_offline;
        let _ = self.notice_tx.try_send(Notice::ClientOffline { id, name, notify });
        self.publish();
    }

    fn on_heartbeat(&mut self) {
        let now = Instant::now();
        let sent_us = u64::try_from(now.duration_since(self.epoch).as_micros()).unwrap_or(u64::MAX);
        let mut lost = Vec::new();
        let ids: Vec<DeviceId> = self.slots.keys().copied().collect();
        for id in ids {
            let Some(slot) = self.slots.get_mut(&id) else { continue };
            if slot.conn.is_none() {
                continue;
            }
            if slot.health.tick(now) {
                lost.push(id);
                continue;
            }
            let seq = slot.health.next_ping();
            let rtt_us = slot.health.rtt().map_or(0, |d| u32::try_from(d.as_micros()).unwrap_or(u32::MAX));
            if let Some(c) = slot.conn.as_mut() {
                if c.pings.len() >= 8 {
                    c.pings.pop_front();
                }
                c.pings.push_back((seq, now));
            }
            self.send_control(id, Control::Ping { seq, sent_us, rtt_us });
        }
        for id in lost {
            self.drop_client(id, "heartbeat timeout");
        }
        self.publish();
    }

    /// macOS Secure Keyboard Entry keeps key presses on this Mac while a client
    /// has control; say so instead of silently typing into the wrong computer.
    fn check_secure_input(&mut self) {
        let on = self.focused().is_some() && glidedesk_input::secure_input_active();
        let has = self.warnings.iter().any(|w| w.starts_with(SECURE_INPUT_WARNING));
        if on == has {
            return;
        }
        if on {
            self.warnings.push(format!(
                "{SECURE_INPUT_WARNING}: an app on this Mac turned on Secure Keyboard Entry (a password field, \
                 Terminal or a password manager), so typing can't be sent to other computers until it is closed"
            ));
        } else {
            self.warnings.retain(|w| !w.starts_with(SECURE_INPUT_WARNING));
        }
        self.publish();
    }

    fn poll_monitors(&mut self) {
        match (self.monitors)() {
            Ok(m) if m != self.local_monitors => {
                info!("local monitors changed");
                self.local_monitors = m;
                self.rebuild_layout();
                self.publish();
            }
            Ok(_) => {}
            Err(e) => debug!(error = %e, "monitor query failed"),
        }
    }

    fn reconnect_all(&mut self) {
        let ids: Vec<DeviceId> = self.slots.iter().filter(|(_, s)| s.conn.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            self.send_control(id, Control::Goodbye(GoodbyeReason::Restarting));
            self.drop_client(id, "reconnect requested");
        }
    }

    fn identify(&mut self) {
        let _ = self.notice_tx.try_send(Notice::Identify { label: self.local_name.clone() });
        let ids: Vec<(DeviceId, String)> =
            self.slots.iter().filter(|(_, s)| s.conn.is_some()).map(|(id, s)| (*id, s.name.clone())).collect();
        for (id, label) in ids {
            self.send_control(id, Control::Identify { label });
        }
    }

    async fn shutdown(&mut self, reason: GoodbyeReason) {
        if let Some(id) = self.focused() {
            self.leave(id);
        }
        self.capture.set_grab(false);
        let ids: Vec<DeviceId> = self.slots.iter().filter(|(_, s)| s.conn.is_some()).map(|(id, _)| *id).collect();
        for id in &ids {
            self.send_control(*id, Control::Goodbye(reason));
        }
        // Let writers flush the goodbyes.
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.capture.stop();
        self.advertiser = None;
        self.net.close(b"server stopping");
        self.net.wait_closed().await;
        for slot in self.slots.values_mut() {
            slot.conn = None;
            slot.health.disconnected();
        }
        let mut view = self.view();
        view.running = false;
        self.status_tx.send_replace(view);
    }
}

// ---------------------------------------------------------------------------
// hub: configuration, layout, status
// ---------------------------------------------------------------------------

impl Hub {
    fn on_command(&mut self, cmd: ServerCommand) {
        match cmd {
            ServerCommand::SwitchTo(id) => {
                let out = self.engine.switch_to(id);
                self.apply(out, 0, 0);
            }
            ServerCommand::SwitchHome => self.run_action(Action::Home),
            ServerCommand::SwitchNext => self.run_action(Action::Next),
            ServerCommand::SwitchPrevious => self.run_action(Action::Previous),
            ServerCommand::ToggleLock => self.run_action(Action::Lock),
            ServerCommand::SetLocked(l) => {
                self.engine.set_locked(l);
                self.publish();
            }
            ServerCommand::ReconnectAll => self.reconnect_all(),
            ServerCommand::Identify => self.identify(),
            ServerCommand::Disconnect(id) => self.drop_client(id, "disconnected by user"),
            ServerCommand::ApplyConfig { config, removed } => self.apply_config(*config, &removed),
            ServerCommand::FetchOffer => self.paste_offered_files(None),
            ServerCommand::Shutdown(_) => {} // handled in the loop
        }
    }

    fn apply_config(&mut self, mut cfg: Config, removed: &[DeviceId]) {
        // A client may have joined after the agent built `cfg`: keep it.
        let kept = cfg.keep_known_clients(&self.config, removed);
        self.config = cfg;
        if !kept.is_empty() {
            let _ = self.notice_tx.try_send(Notice::ConfigChanged(Box::new(self.config.clone())));
        }
        self.engine.set_policy(self.config.server.switching.clone());
        let hcfg = health_cfg(&self.config);
        for slot in self.slots.values_mut() {
            slot.health.set_config(hcfg);
        }
        // Clients that were removed or blocked lose their link.
        let gone: Vec<DeviceId> =
            self.slots.keys().copied().filter(|id| self.config.server.client(id).is_none_or(|e| e.blocked)).collect();
        for id in &gone {
            self.drop_client(*id, "removed or blocked");
        }
        self.slots.retain(|id, _| self.config.server.client(id).is_some());
        for entry in &self.config.server.clients {
            self.slots.entry(entry.id).or_insert_with(|| Slot {
                name: entry.name.clone(),
                platform: None,
                app_version: None,
                monitors: Vec::new(),
                prefs: ClientPrefs::default(),
                health: Health::new(hcfg),
                conn: None,
                last_seen: None,
            });
        }
        let ids: Vec<DeviceId> = self.slots.iter().filter(|(_, s)| s.conn.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            let s = self.settings_for(id);
            self.send_control(id, Control::Settings(s));
        }
        self.load_hotkeys();
        self.rebuild_layout();
        self.publish();
    }

    fn load_hotkeys(&mut self) {
        let h = &self.config.server.hotkeys;
        let mut out = Vec::new();
        self.warnings.retain(|w| !w.starts_with("hotkey"));
        for (text, action) in [
            (&h.lock_cursor, Action::Lock),
            (&h.switch_home, Action::Home),
            (&h.switch_next, Action::Next),
            (&h.switch_previous, Action::Previous),
            (&h.reconnect_all, Action::ReconnectAll),
            (&h.identify, Action::Identify),
        ] {
            match hotkey::parse_optional(text) {
                Ok(Some(k)) => out.push((k, action)),
                Ok(None) => {}
                Err(e) => self.warnings.push(format!("hotkey '{text}': {e}")),
            }
        }
        self.hotkeys = out;
    }

    /// Effective settings for a client: both sides must allow privacy features.
    fn settings_for(&self, id: DeviceId) -> ClientSettings {
        let sharing = &self.config.server.sharing;
        let entry = self.entry(id);
        let prefs = self.slots.get(&id).map(|s| s.prefs.clone()).unwrap_or_default();
        let dir = sharing.direction;
        ClientSettings {
            clipboard: sharing.clipboard && entry.is_none_or(|e| e.clipboard) && prefs.accept_clipboard,
            files: sharing.files && entry.is_none_or(|e| e.files) && prefs.accept_files,
            draw_cursor: prefs.draw_cursor,
            led_sync: prefs.led_sync,
            clipboard_receive: dir != glidedesk_config::Direction::FromClients,
            clipboard_send: dir != glidedesk_config::Direction::ToClients,
            clipboard_limit: sharing.max_clipboard_bytes,
        }
    }

    /// Sends our clipboard to `id` when it changed since the last time (PLAN §6.3).
    fn push_clipboard(&mut self, id: DeviceId) {
        let s = self.settings_for(id);
        let allow_clip = s.clipboard && s.clipboard_receive;
        let allow_files = s.files && s.clipboard_receive;
        if !(allow_clip || allow_files) || !self.sync.available() {
            return;
        }
        let Some(seq) = self.sync.changed_for(Some(id), self.last_sent.get(&id).copied()) else { return };
        let Some(conn) = self.slots.get(&id).and_then(|s| s.conn.as_ref()).map(|c| c.connection.clone()) else {
            return;
        };
        self.last_sent.insert(id, seq);
        let name = self.name_of(id);
        let sync = self.sync.clone();
        tokio::spawn(async move {
            if let Err(e) = sync.send(conn, Some(id), name, allow_clip, allow_files, s.clipboard_limit).await {
                debug!(error = %e, "clipboard send failed");
            }
        });
    }

    /// A client sent its clipboard or offered files (when the cursor came back).
    fn on_stream(&mut self, id: DeviceId, kind: u8, stream: quinn::RecvStream) {
        let s = self.settings_for(id);
        let sync = self.sync.clone();
        let name = self.name_of(id);
        let Some(conn) = self.slots.get(&id).and_then(|s| s.conn.as_ref()).map(|c| c.connection.clone()) else {
            return;
        };
        let (allow_clip, allow_files) = (s.clipboard && s.clipboard_send, s.files && s.clipboard_send);
        let holds = self.capture.holds_paste();
        let (events, notices, capture) = (self.event_tx.clone(), self.notice_tx.clone(), self.capture.clone());
        tokio::spawn(async move {
            match sync
                .clone()
                .receive(kind, stream, conn, Some(id), name, allow_clip, allow_files, s.clipboard_limit)
                .await
            {
                // This capture can't hold the paste shortcut (Linux X11): fetch right away.
                Ok(crate::sync::Received::Offer) if !holds => {
                    fetch_and_paste(sync, None, capture, events, notices).await;
                }
                Ok(_) => {}
                Err(e) => warn!(error = %e, "receiving clipboard failed"),
            }
        });
    }

    /// The user pasted on this computer while files are only offered: fetch
    /// them, then replay the paste (`key`) so the file manager copies them.
    fn paste_offered_files(&mut self, key: Option<KeyCode>) {
        let (sync, capture) = (self.sync.clone(), self.capture.clone());
        let (events, notices) = (self.event_tx.clone(), self.notice_tx.clone());
        tokio::spawn(fetch_and_paste(sync, key, capture, events, notices));
    }

    /// Zero-config placement: put a new client on a free side of the server.
    fn auto_place(&mut self, id: DeviceId) {
        if self.config.layout.links.iter().any(|l| l.from == id || l.to == id) {
            return;
        }
        let used: HashSet<Side> =
            self.config.layout.links.iter().filter(|l| l.from == self.local_id).map(|l| l.side).collect();
        let side = [Side::Right, Side::Left, Side::Top, Side::Bottom].into_iter().find(|s| !used.contains(s));
        let link = if let Some(side) = side {
            LinkSpec::simple(self.local_id, side, id)
        } else {
            // Every side is taken: append to the end of the right-hand chain.
            let mut end = self.local_id;
            let mut seen = HashSet::from([end]);
            while let Some(next) =
                self.config.layout.links.iter().find(|l| l.from == end && l.side == Side::Right).map(|l| l.to)
            {
                if !seen.insert(next) {
                    break;
                }
                end = next;
            }
            LinkSpec::simple(end, Side::Right, id)
        };
        info!(client = %id, side = ?link.side, "placed new client automatically");
        self.config.layout.links.push(link);
    }

    fn rebuild_layout(&mut self) {
        let summary = self
            .config
            .layout
            .links
            .iter()
            .map(|l| {
                format!("{} is on the {:?} of {}", self.name_of(l.to), l.side, self.name_of(l.from)).to_lowercase()
            })
            .collect::<Vec<_>>()
            .join("; ");
        if summary != self.layout_summary {
            info!("layout: {}", if summary.is_empty() { "no computers placed" } else { &summary });
            self.layout_summary = summary;
        }
        let mut machines = vec![Machine { id: self.local_id, monitors: self.local_monitors.clone() }];
        machines.extend(
            self.slots
                .iter()
                .filter(|(_, s)| !s.monitors.is_empty())
                .map(|(id, s)| Machine { id: *id, monitors: s.monitors.clone() }),
        );
        let layout = Layout::build(machines, &self.config.layout.links);
        self.warnings.retain(|w| w.starts_with("hotkey") || w.starts_with(SECURE_INPUT_WARNING));
        for w in layout.warnings() {
            match w {
                Warning::SelectionFellBack { machine, side } => {
                    self.warnings.push(format!(
                        "layout: chosen monitors of {} ({side:?}) are not connected; using all",
                        self.name_of(*machine)
                    ));
                }
                Warning::NoOuterEdge { machine, side } => {
                    self.warnings.push(format!(
                        "layout: {} has no free {side:?} edge on the chosen monitors",
                        self.name_of(*machine)
                    ));
                }
                // Unknown machines are clients that never connected yet — expected.
                Warning::UnknownMachine(_) | Warning::InvalidSpan { .. } => {}
            }
        }
        let out = self.engine.set_layout(layout);
        let server_units = units_per_dip(Some(self.local_platform), &self.local_monitors);
        let states: Vec<(DeviceId, bool, f64)> = self
            .slots
            .iter()
            .map(|(id, s)| {
                let user = s
                    .prefs
                    .mouse_speed
                    .map(f64::from)
                    .or_else(|| self.config.server.client(id).map(|e| e.mouse_speed))
                    .unwrap_or(1.0);
                let auto = units_per_dip(s.platform, &s.monitors) / server_units;
                (*id, s.health.state().reachable(), (user * auto).clamp(0.05, 20.0))
            })
            .collect();
        for (id, reachable, speed) in states {
            self.engine.set_speed(id, speed);
            let o = self.engine.set_available(id, reachable);
            self.apply(o, 0, 0);
        }
        self.apply(out, 0, 0);
    }

    fn name_of(&self, id: DeviceId) -> String {
        if id == self.local_id {
            return self.local_name.clone();
        }
        self.slots.get(&id).map_or_else(|| id.to_string(), |s| s.name.clone())
    }

    fn view(&self) -> ServerView {
        let mut clients: Vec<ClientView> = self
            .config
            .server
            .clients
            .iter()
            .filter_map(|e| {
                let s = self.slots.get(&e.id)?;
                #[allow(clippy::cast_precision_loss)]
                let latency_ms = s.health.rtt().filter(|_| s.conn.is_some()).map(|d| d.as_micros() as f32 / 1000.0);
                Some(ClientView {
                    id: e.id,
                    name: if s.name.is_empty() { e.name.clone() } else { s.name.clone() },
                    platform: s.platform,
                    state: s.health.state(),
                    latency_ms,
                    last_seen: s.last_seen.map(unix),
                    address: s.conn.as_ref().map(|c| c.addr.ip().to_string()),
                    app_version: s.app_version.clone(),
                    monitors: s.monitors.clone(),
                    status: s.health.status(),
                    blocked: e.blocked,
                })
            })
            .collect();
        clients.sort_by_key(|c| (c.state != HealthState::Online, c.name.to_lowercase()));
        let fp = self.net.fingerprint();
        ServerView {
            running: true,
            bind: self.bind.iter().map(|b| BindView { addr: b.addr.to_string(), error: b.error.clone() }).collect(),
            fingerprint: fp[..8].iter().fold(String::with_capacity(16), |mut acc, b| {
                use std::fmt::Write as _;
                let _ = write!(acc, "{b:02x}");
                acc
            }),
            local: MachineView {
                id: Some(self.local_id),
                name: self.local_name.clone(),
                monitors: self.local_monitors.clone(),
            },
            clients,
            focus: self.focused(),
            locked: self.engine.locked(),
            warnings: self.warnings.clone(),
            transfers: self.sync.transfers.snapshot(),
            offer: self.sync.offer_description(),
        }
    }

    fn publish(&self) {
        self.status_tx.send_replace(self.view());
    }
}

// ---------------------------------------------------------------------------
// connection tasks
// ---------------------------------------------------------------------------

/// Fetches offered files (§14.2); on success replays the held paste and, for
/// cut files, reports the move to their origin once they left staging.
async fn fetch_and_paste(
    sync: Arc<crate::sync::Sync>,
    key: Option<KeyCode>,
    capture: Arc<dyn CaptureControl>,
    events: mpsc::Sender<HubEvent>,
    notices: mpsc::Sender<Notice>,
) {
    match sync.fetch_offer().await {
        Ok(got) => {
            if let Some(k) = key {
                capture.replay_paste(k);
            }
            if let Some(files) = got
                && files.cut
                && let Some(id) = files.origin
                && crate::sync::wait_taken(files.roots).await
            {
                let _ = events.send(HubEvent::Taken { id, set: files.set }).await;
            }
        }
        Err(e) => {
            warn!(error = %e, "fetching offered files failed");
            let _ = notices.send(Notice::Error { message: format!("Paste failed: {e}") }).await;
        }
    }
}

/// Answers fetches: the client pasted files this computer offered.
fn spawn_fetch_server(conn: quinn::Connection, id: DeviceId, name: String, sync: Arc<crate::sync::Sync>) {
    tokio::spawn(async move {
        while let Ok((send, mut recv)) = conn.accept_bi().await {
            let mut kind = [0u8; 1];
            let ok = tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut kind))
                .await
                .is_ok_and(|r| r.is_ok());
            if ok && kind[0] == glidedesk_proto::stream_kind::FETCH {
                tokio::spawn(sync.clone().serve_fetch(send, recv, Some(id), name.clone()));
            } else {
                let _ = recv.stop(0u32.into());
            }
        }
    });
}

type Handshake = (quinn::Connection, FrameWriter, FrameReader, Hello, Option<Vec<u8>>);

/// Reads `Hello` and, when a password is set, runs the PAKE (PLAN §14.1).
/// Nothing about a client is kept before it passes.
async fn handshake(
    inc: quinn::Incoming,
    verifier: Option<Arc<glidedesk_net::auth::Verifier>>,
    throttle: Arc<glidedesk_net::auth::Throttle>,
) -> Result<Handshake, CoreError> {
    use glidedesk_net::auth::{EXPORTER_LABEL, ServerChallenge};
    let net = |e: &dyn std::fmt::Display| CoreError::Net(e.to_string());
    let conn = inc.await.map_err(|e| net(&e))?;
    let (send, recv) = conn.accept_bi().await.map_err(|e| net(&e))?;
    let mut reader = FrameReader::new(recv, MAX_CONTROL_FRAME);
    let mut writer = FrameWriter::new(send, MAX_CONTROL_FRAME);
    let Some(Control::Hello(hello)) = reader.recv::<Control>().await.map_err(|e| net(&e))? else {
        return Err(CoreError::Protocol("expected Hello".into()));
    };
    // Old clients are told to update by `on_hello`; they can't do the PAKE.
    let Some(v) = verifier.filter(|_| hello.protocol == PROTOCOL_VERSION) else {
        return Ok((conn, writer, reader, hello, None));
    };
    let ip = conn.remote_address().ip();
    let challenge = ServerChallenge::start(&v);
    let msg = Control::AuthChallenge { salt: v.salt.to_vec(), message: challenge.message.clone() };
    writer.send(&msg).await.map_err(|e| net(&e))?;
    let Some(Control::AuthResponse { message, proof }) = reader.recv::<Control>().await.map_err(|e| net(&e))? else {
        // The client has no password: it shows "password required" itself.
        return Err(CoreError::Protocol("client has no password".into()));
    };
    let mut exporter = [0u8; 32];
    conn.export_keying_material(&mut exporter, EXPORTER_LABEL, b"").map_err(|_| net(&"TLS exporter"))?;
    match challenge.finish(&message, &proof, &exporter) {
        Ok(server_proof) => {
            throttle.success(ip);
            Ok((conn, writer, reader, hello, Some(server_proof.to_vec())))
        }
        Err(e) => {
            let wait = throttle.fail(ip, Instant::now());
            warn!(%ip, lockout_s = wait.as_secs(), "wrong password from a client");
            let reason = if wait.is_zero() { RejectReason::WrongPassword } else { RejectReason::TooManyAttempts };
            let _ = writer.send(&Control::Reject(reason)).await;
            writer.finish();
            tokio::time::sleep(Duration::from_millis(200)).await;
            conn.close(1u32.into(), b"authentication failed");
            Err(CoreError::Protocol(e.to_string()))
        }
    }
}

fn spawn_control_writer(
    mut w: FrameWriter,
    mut rx: mpsc::Receiver<Control>,
    id: DeviceId,
    generation: u64,
    events: mpsc::Sender<HubEvent>,
) {
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let last = matches!(msg, Control::Goodbye(_));
            if let Err(e) = w.send(&msg).await {
                let _ = events.send(HubEvent::Closed { id, generation, reason: e.to_string() }).await;
                return;
            }
            if last {
                w.finish();
                return;
            }
        }
    });
}

fn spawn_input_writer(
    conn: quinn::Connection,
    mut rx: mpsc::Receiver<Input>,
    id: DeviceId,
    generation: u64,
    events: mpsc::Sender<HubEvent>,
) {
    tokio::spawn(async move {
        let send = match conn.open_uni().await {
            Ok(s) => s,
            Err(e) => {
                let _ = events.send(HubEvent::Closed { id, generation, reason: e.to_string() }).await;
                return;
            }
        };
        let mut send = send;
        // Stream kind first; a QUIC stream only becomes visible to the peer
        // once data is sent, so this also announces it immediately.
        if let Err(e) = send.write_all(&[glidedesk_proto::stream_kind::INPUT]).await {
            let _ = events.send(HubEvent::Closed { id, generation, reason: e.to_string() }).await;
            return;
        }
        let mut w = FrameWriter::new(send, MAX_INPUT_FRAME);
        if let Err(e) = w.send(&Input::ReleaseAll).await {
            let _ = events.send(HubEvent::Closed { id, generation, reason: e.to_string() }).await;
            return;
        }
        let mut batch: Vec<Input> = Vec::with_capacity(64);
        while let Some(first) = rx.recv().await {
            batch.clear();
            batch.push(first);
            // Drain whatever else is queued and coalesce consecutive absolute moves.
            while batch.len() < 256 {
                match rx.try_recv() {
                    Ok(ev) => match (ev, batch.last_mut()) {
                        (Input::MouseAbs(p), Some(Input::MouseAbs(last))) => *last = p,
                        _ => batch.push(ev),
                    },
                    Err(_) => break,
                }
            }
            if let Err(e) = w.send_batch(batch.iter()).await {
                let _ = events.send(HubEvent::Closed { id, generation, reason: e.to_string() }).await;
                return;
            }
        }
    });
}

/// Accepts clipboard/files streams the client opens.
fn spawn_uni_acceptor(conn: quinn::Connection, id: DeviceId, generation: u64, events: mpsc::Sender<HubEvent>) {
    tokio::spawn(async move {
        while let Ok(mut stream) = conn.accept_uni().await {
            let mut kind = [0u8; 1];
            if stream.read_exact(&mut kind).await.is_err() {
                continue;
            }
            if events.send(HubEvent::Stream { id, generation, kind: kind[0], stream }).await.is_err() {
                return;
            }
        }
    });
}

fn spawn_reader(mut reader: FrameReader, id: DeviceId, generation: u64, events: mpsc::Sender<HubEvent>) {
    tokio::spawn(async move {
        loop {
            match reader.recv::<Control>().await {
                Ok(Some(msg)) => {
                    if events.send(HubEvent::Control { id, generation, msg }).await.is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = events.send(HubEvent::Closed { id, generation, reason: "stream closed".into() }).await;
                    return;
                }
                Err(e) => {
                    let _ = events.send(HubEvent::Closed { id, generation, reason: e.to_string() }).await;
                    return;
                }
            }
        }
    });
}

/// Peer-supplied text is untrusted: strip control characters, cap length.
fn sanitize_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(64).collect();
    if t.trim().is_empty() { "Unnamed computer".into() } else { t.trim().to_owned() }
}

/// Keeps at most 16 valid monitors with sane sizes.
fn sanitize_monitors(m: Vec<MonitorInfo>) -> Vec<MonitorInfo> {
    m.into_iter()
        .filter(|m| m.bounds.is_valid() && m.bounds.w <= 100_000 && m.bounds.h <= 100_000 && m.scale.is_finite())
        .take(16)
        .map(|mut m| {
            m.name = sanitize_name(&m.name);
            m.id.0 = m.id.0.chars().filter(|c| !c.is_control()).take(256).collect();
            m.scale = m.scale.clamp(0.25, 8.0);
            m
        })
        .collect()
}
