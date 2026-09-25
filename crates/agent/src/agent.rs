//! The agent loop: owns the configuration, runs the server or client
//! runtime, answers IPC requests and publishes status.

use std::sync::Arc;
use std::time::Duration;

use glidedesk_config::{Config, ConfigStore, ExportScope, ImportOptions, Role};
use glidedesk_core::{ClientCommand, ClientHandle, Notice, ServerCommand, ServerHandle, client, server};
use glidedesk_ipc::{AgentStatus, Event, NoticeView, PermissionsView, Request};
use glidedesk_proto::{DeviceId, GoodbyeReason, Platform};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info, warn};

use crate::ipc_server::Call;
use crate::state::StateFile;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

enum Runtime {
    Idle,
    Server(ServerHandle),
    Client(ClientHandle),
}

pub struct Agent {
    store: ConfigStore,
    config: Config,
    issues: Vec<String>,
    read_only: bool,
    state: StateFile,
    runtime: Runtime,
    /// Sharing was switched off by the user (tray "Stop").
    stopped: bool,
    error: Option<String>,
    status: watch::Sender<AgentStatus>,
    events: broadcast::Sender<Event>,
    log_dir: String,
    /// Last permission check (`None` before the first one).
    permissions_ready: Option<bool>,
}

impl Agent {
    pub fn new(
        mut store: ConfigStore,
        status: watch::Sender<AgentStatus>,
        events: broadcast::Sender<Event>,
        log_dir: String,
    ) -> Result<Self, String> {
        let loaded = store.load_or_create().map_err(|e| e.to_string())?;
        let issues = loaded.issues.iter().map(|i| format!("{}: {}", i.key, i.message)).collect();
        let mut state = StateFile::load(store.dir());
        if state.state.last_version.as_deref() != Some(VERSION) {
            if let Some(old) = &state.state.last_version {
                info!(from = %old, to = VERSION, "upgraded");
            }
            state.state.last_version = Some(VERSION.to_owned());
            state.save();
        }
        Ok(Self {
            store,
            config: loaded.config,
            issues,
            read_only: loaded.read_only,
            state,
            runtime: Runtime::Idle,
            stopped: false,
            error: None,
            status,
            events,
            log_dir,
            permissions_ready: None,
        })
    }

    fn host_name() -> String {
        glidedesk_platform::host_name()
    }

    // -----------------------------------------------------------------------
    // runtime lifecycle
    // -----------------------------------------------------------------------

    fn start_runtime(&mut self) {
        if !matches!(self.runtime, Runtime::Idle) || self.stopped {
            return;
        }
        self.error = None;
        let monitors: glidedesk_core::MonitorSource = Arc::new(glidedesk_input::monitors);
        match self.config.device.role {
            Role::Unset => return,
            Role::Server => {
                let capture = match glidedesk_input::start_capture() {
                    Ok(c) => c,
                    Err(e) => {
                        self.fail(format!("cannot capture the keyboard and mouse: {e}"));
                        return;
                    }
                };
                let deps = server::ServerDeps {
                    capture,
                    monitors,
                    known_monitors: self.state.state.known_monitors.clone(),
                    app_version: VERSION.into(),
                    host_name: Self::host_name(),
                    clipboard: self.clipboard(),
                };
                match server::start(self.config.clone(), deps) {
                    Ok(h) => self.runtime = Runtime::Server(h),
                    Err(e) => self.fail(format!("cannot start the server: {e}")),
                }
            }
            Role::Client => {
                let injector = match glidedesk_input::injector() {
                    Ok(i) => i,
                    Err(e) => {
                        self.fail(format!("cannot control this computer's input: {e}"));
                        return;
                    }
                };
                let deps = client::ClientDeps {
                    injector,
                    monitors,
                    status: Arc::new(glidedesk_platform::session_status),
                    app_version: VERSION.into(),
                    host_name: Self::host_name(),
                    preferred_server: self.state.state.last_server,
                    clipboard: self.clipboard(),
                };
                self.runtime = Runtime::Client(client::start(self.config.clone(), deps));
            }
        }
        info!(role = ?self.config.device.role, "sharing started");
    }

    /// System clipboard; a failure is reported instead of silently disabling sync.
    fn clipboard(&self) -> Option<Box<dyn glidedesk_clipboard::Clipboard>> {
        match glidedesk_clipboard::system() {
            Ok(c) => Some(c),
            Err(e) => {
                warn!(error = %e, "clipboard unavailable");
                let _ = self.events.send(Event::Notice(NoticeView::Error {
                    message: format!("Clipboard sharing is unavailable on this computer: {e}"),
                }));
                None
            }
        }
    }

    fn fail(&mut self, msg: String) {
        error!("{msg}");
        let _ = self.events.send(Event::Notice(NoticeView::Error { message: msg.clone() }));
        self.error = Some(msg);
    }

    async fn stop_runtime(&mut self, reason: GoodbyeReason) {
        let rt = std::mem::replace(&mut self.runtime, Runtime::Idle);
        let task = match rt {
            Runtime::Idle => return,
            Runtime::Server(h) => {
                let _ = h.commands.send(ServerCommand::Shutdown(reason)).await;
                h.task
            }
            Runtime::Client(h) => {
                let _ = h.commands.send(ClientCommand::Shutdown(reason)).await;
                h.task
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), task).await.is_err() {
            warn!("runtime did not stop in time");
        }
        info!("sharing stopped");
    }

    async fn restart_runtime(&mut self) {
        self.stop_runtime(GoodbyeReason::Restarting).await;
        self.start_runtime();
    }

    // -----------------------------------------------------------------------
    // status
    // -----------------------------------------------------------------------

    fn publish(&self) {
        let perms = glidedesk_input::permissions();
        let (server, client) = match &self.runtime {
            Runtime::Idle => (None, None),
            Runtime::Server(h) => (Some(h.status.borrow().clone()), None),
            Runtime::Client(h) => (None, Some(h.status.borrow().clone())),
        };
        let status = AgentStatus {
            version: VERSION.into(),
            platform: Some(Platform::current()),
            role: self.config.device.role,
            running: !matches!(self.runtime, Runtime::Idle),
            error: self.error.clone(),
            server,
            client,
            permissions: PermissionsView {
                accessibility: perms.accessibility,
                input_monitoring: perms.input_monitoring,
            },
            config_issues: self.issues.clone(),
            config_read_only: self.read_only,
            config_dir: self.store.dir().display().to_string(),
            log_dir: self.log_dir.clone(),
            clipboard: if self.config.device.role == Role::Client {
                self.config.client.accept_clipboard
            } else {
                self.config.server.sharing.clipboard
            },
            files: if self.config.device.role == Role::Client {
                self.config.client.accept_files
            } else {
                self.config.server.sharing.files
            },
            notifications: self.config.general.notifications,
            start_at_login: self.config.general.start_at_login,
            device_name: if self.config.device.name.is_empty() {
                Self::host_name()
            } else {
                self.config.device.name.clone()
            },
        };
        self.status.send_if_modified(|old| {
            if *old == status {
                false
            } else {
                *old = status;
                true
            }
        });
    }

    // -----------------------------------------------------------------------
    // main loop
    // -----------------------------------------------------------------------

    pub async fn run(mut self, mut calls: mpsc::Receiver<Call>) {
        self.start_runtime();
        self.publish();
        let mut perm_check = tokio::time::interval(Duration::from_secs(3));
        perm_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                call = calls.recv() => {
                    let Some(call) = call else { break };
                    let quit = matches!(call.request, Request::Quit);
                    let result = self.handle(call.request).await;
                    let _ = call.reply.send(result);
                    self.publish();
                    if quit {
                        break;
                    }
                }
                ev = runtime_event(&mut self.runtime) => {
                    match ev {
                        RtEvent::Changed => {}
                        RtEvent::Notice(n) => self.on_notice(n),
                        // Runtime ended by itself (fatal error): report and go idle.
                        RtEvent::Ended => {
                            self.runtime = Runtime::Idle;
                            if !self.stopped {
                                self.fail("sharing stopped unexpectedly; use Restart".into());
                            }
                        }
                    }
                    self.publish();
                }
                _ = perm_check.tick() => {
                    self.on_permission_tick().await;
                    self.publish();
                }
                () = shutdown_signal() => {
                    info!("shutdown signal");
                    break;
                }
            }
        }
        self.stop_runtime(GoodbyeReason::Quitting).await;
        let _ = self.events.send(Event::Exiting);
        // Give IPC writers a moment to deliver `Exiting`.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Follows permission changes made in System Settings while running:
    /// granted → start (or restart, so the event tap is recreated with the new
    /// rights); revoked → stop with a clear message instead of leaking input.
    async fn on_permission_tick(&mut self) {
        let server = self.config.device.role == Role::Server;
        let ready = glidedesk_input::permissions().ready(server);
        let was = self.permissions_ready.replace(ready);
        if self.stopped || self.config.device.role == Role::Unset {
            return;
        }
        match (was, ready) {
            (Some(false), true) => {
                info!("permission granted; starting sharing");
                self.restart_runtime().await;
            }
            (Some(true), false) => {
                warn!("permission revoked; stopping sharing");
                self.stop_runtime(GoodbyeReason::Stopping).await;
                self.fail(if cfg!(target_os = "macos") {
                    "Accessibility permission was turned off; allow Glidedesk in System Settings → Privacy & Security → Accessibility".into()
                } else {
                    "access to the keyboard and mouse was lost; see Advanced → Self-test".into()
                });
            }
            _ if ready && self.error.is_some() && matches!(self.runtime, Runtime::Idle) => self.start_runtime(),
            _ => {}
        }
    }

    fn on_notice(&mut self, n: Notice) {
        let view = match n {
            Notice::ConfigChanged(cfg) => {
                self.config = *cfg;
                self.save_config();
                // Open settings windows reload, so they never save a stale client list.
                let _ = self.events.send(Event::ConfigChanged);
                None
            }
            Notice::MonitorsSeen { id, monitors } => {
                if self.state.state.known_monitors.get(&id) != Some(&monitors) {
                    self.state.state.known_monitors.insert(id, monitors);
                    self.state.save();
                }
                None
            }
            Notice::NewClient { id, name, address } => Some(NoticeView::NewClient { id, name, address }),
            Notice::ClientOnline { id, name, notify } => notify.then_some(NoticeView::ClientOnline { id, name }),
            Notice::ClientOffline { id, name, notify } => notify.then_some(NoticeView::ClientOffline { id, name }),
            Notice::ServerConnected { id, name } => {
                if self.state.state.last_server != Some(id) {
                    self.state.state.last_server = Some(id);
                    self.state.save();
                }
                Some(NoticeView::ServerConnected { name })
            }
            Notice::Identify { label } => Some(NoticeView::Identify { label }),
        };
        if let Some(v) = view {
            let _ = self.events.send(Event::Notice(v));
        }
    }

    fn save_config(&mut self) {
        if let Err(e) = self.store.save(&self.config) {
            self.fail(format!("could not save settings: {e}"));
        }
    }

    // -----------------------------------------------------------------------
    // requests
    // -----------------------------------------------------------------------

    async fn server_cmd(&self, cmd: ServerCommand) -> Result<Value, String> {
        match &self.runtime {
            Runtime::Server(h) => {
                h.commands.send(cmd).await.map_err(|_| "server is not running".to_owned())?;
                Ok(Value::Null)
            }
            _ => Err("this computer is not sharing as a server".into()),
        }
    }

    /// Does switching from `old` to `new` need a full runtime restart?
    fn needs_restart(old: &Config, new: &Config) -> bool {
        old.device != new.device || old.server.network != new.server.network || old.server.health != new.server.health
    }

    /// Applies settings from the UI. Clients the server knows but `new` lacks are
    /// kept (the window may have loaded before they joined); only `removed` go.
    async fn apply_config(&mut self, mut new: Config, removed: &[DeviceId]) -> Result<Value, String> {
        if self.read_only {
            return Err("settings were written by a newer Glidedesk and are read-only".into());
        }
        new.device.id = self.config.device.id; // identity is never changed through the UI
        let kept = new.keep_known_clients(&self.config, removed);
        if !kept.is_empty() {
            info!(count = kept.len(), "kept clients missing from a settings save");
        }
        let issues = new.sanitize();
        self.store.save(&new).map_err(|e| e.to_string())?;
        let restart = Self::needs_restart(&self.config, &new);
        self.config = new;
        self.issues.clear();
        if restart {
            self.restart_runtime().await;
        } else {
            match &self.runtime {
                Runtime::Server(h) => {
                    let cmd =
                        ServerCommand::ApplyConfig { config: Box::new(self.config.clone()), removed: removed.to_vec() };
                    let _ = h.commands.send(cmd).await;
                }
                Runtime::Client(h) => {
                    let _ = h.commands.send(ClientCommand::ApplyConfig(Box::new(self.config.clone()))).await;
                }
                Runtime::Idle => self.start_runtime(),
            }
        }
        if !kept.is_empty() || !issues.is_empty() {
            // The saved settings differ from what the window sent: it reloads.
            let _ = self.events.send(Event::ConfigChanged);
        }
        Ok(json!({ "issues": issues.iter().map(|i| format!("{}: {}", i.key, i.message)).collect::<Vec<_>>() }))
    }

    fn forget_client(&mut self, id: DeviceId) -> Config {
        let mut cfg = self.config.clone();
        cfg.server.clients.retain(|c| c.id != id);
        cfg.layout.links.retain(|l| l.from != id && l.to != id);
        cfg.layout.tiles.retain(|t| t.machine != id);
        self.state.state.known_monitors.remove(&id);
        self.state.save();
        cfg
    }

    #[allow(clippy::too_many_lines)]
    async fn handle(&mut self, req: Request) -> Result<Value, String> {
        match req {
            // Subscribe is handled by the connection; Quit's work happens after the reply.
            Request::Subscribe | Request::Quit => Ok(Value::Null),
            Request::Status => Ok(serde_json::to_value(&*self.status.borrow()).unwrap_or_default()),
            Request::GetConfig => Ok(to_json(&self.config)),
            Request::SetConfig { config } => self.apply_config(*config, &[]).await,
            Request::Start => {
                self.stopped = false;
                self.start_runtime();
                Ok(Value::Null)
            }
            Request::Stop => {
                self.stopped = true;
                self.stop_runtime(GoodbyeReason::Stopping).await;
                Ok(Value::Null)
            }
            Request::Reload => {
                let loaded = self.store.load_or_create().map_err(|e| e.to_string())?;
                self.config = loaded.config;
                self.issues = loaded.issues.iter().map(|i| format!("{}: {}", i.key, i.message)).collect();
                self.read_only = loaded.read_only;
                self.stopped = false;
                self.restart_runtime().await;
                let _ = self.events.send(Event::ConfigChanged);
                Ok(Value::Null)
            }
            Request::SwitchTo { id } => self.server_cmd(ServerCommand::SwitchTo(id)).await,
            Request::SwitchHome => self.server_cmd(ServerCommand::SwitchHome).await,
            Request::SetLocked { locked } => self.server_cmd(ServerCommand::SetLocked(locked)).await,
            Request::ReconnectAll => match &self.runtime {
                Runtime::Client(h) => {
                    let _ = h.commands.send(ClientCommand::Reconnect).await;
                    Ok(Value::Null)
                }
                _ => self.server_cmd(ServerCommand::ReconnectAll).await,
            },
            Request::Identify => {
                if matches!(self.runtime, Runtime::Server(_)) {
                    self.server_cmd(ServerCommand::Identify).await
                } else {
                    let _ = self.events.send(Event::Notice(NoticeView::Identify { label: Self::host_name() }));
                    Ok(Value::Null)
                }
            }
            Request::Disconnect { id } => self.server_cmd(ServerCommand::Disconnect(id)).await,
            Request::SetBlocked { id, blocked } => {
                let mut cfg = self.config.clone();
                let entry = cfg.server.clients.iter_mut().find(|c| c.id == id).ok_or("unknown client")?;
                entry.blocked = blocked;
                self.apply_config(cfg, &[]).await
            }
            Request::Forget { id } => {
                let cfg = self.forget_client(id);
                self.apply_config(cfg, &[id]).await
            }
            Request::ListInterfaces => Ok(to_json(&glidedesk_net::list_interfaces())),
            Request::ExportConfig { layout_only } => {
                let scope = if layout_only { ExportScope::LayoutOnly } else { ExportScope::Full };
                glidedesk_config::export(&self.config, scope, VERSION).map(Value::String).map_err(|e| e.to_string())
            }
            Request::ImportPreview { text } => {
                let p = glidedesk_config::import(&text, &self.config, ImportOptions::default())
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "config": p.config,
                    "layout_only": p.layout_only,
                    "issues": p.issues.iter().map(|i| format!("{}: {}", i.key, i.message)).collect::<Vec<_>>(),
                    "changed_sections": p.changed_sections,
                }))
            }
            Request::ResetConfig => {
                let fresh = self.store.reset(&self.config).map_err(|e| e.to_string())?;
                self.config = fresh;
                self.restart_runtime().await;
                let _ = self.events.send(Event::ConfigChanged);
                Ok(Value::Null)
            }
            Request::RequestPermissions => {
                glidedesk_input::request_permissions();
                Ok(Value::Null)
            }
            Request::Wake { id } => {
                let entry = self.config.server.client(&id).ok_or("unknown client")?;
                let mac = glidedesk_platform::parse_mac(&entry.mac_address)
                    .ok_or("set this computer's MAC address first (Computers → Wake-on-LAN)")?;
                glidedesk_platform::wake_on_lan(mac).map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            Request::SelfTest => Ok(crate::selftest::run(matches!(self.runtime, Runtime::Server(_)))),
            Request::SetServerPassword { password } => {
                let mut cfg = self.config.clone();
                cfg.server.network.password = if password.is_empty() {
                    None
                } else {
                    if password.chars().count() > 256 {
                        return Err("the password is too long (256 characters at most)".into());
                    }
                    let v = tokio::task::spawn_blocking(move || glidedesk_net::auth::Verifier::new(&password))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?;
                    let (salt, key) = v.to_hex();
                    Some(glidedesk_config::StoredPassword { salt, key })
                };
                // Network settings changed: sharing restarts and every client signs in again.
                let out = self.apply_config(cfg, &[]).await;
                let _ = self.events.send(Event::ConfigChanged);
                out
            }
        }
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_default()
}

enum RtEvent {
    Changed,
    Notice(Notice),
    Ended,
}

async fn runtime_event(rt: &mut Runtime) -> RtEvent {
    match rt {
        Runtime::Idle => std::future::pending().await,
        Runtime::Server(h) => tokio::select! {
            r = h.status.changed() => if r.is_ok() { RtEvent::Changed } else { RtEvent::Ended },
            n = h.notices.recv() => n.map_or(RtEvent::Ended, RtEvent::Notice),
        },
        Runtime::Client(h) => tokio::select! {
            r = h.status.changed() => if r.is_ok() { RtEvent::Changed } else { RtEvent::Ended },
            n = h.notices.recv() => n.map_or(RtEvent::Ended, RtEvent::Notice),
        },
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        let (Ok(mut c), Ok(mut close), Ok(mut shut)) = (
            tokio::signal::windows::ctrl_c(),
            tokio::signal::windows::ctrl_close(),
            tokio::signal::windows::ctrl_shutdown(),
        ) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = c.recv() => {}
            _ = close.recv() => {}
            _ = shut.recv() => {}
        }
    }
}
