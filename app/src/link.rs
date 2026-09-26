//! Keeps the connection to the agent: starts it when missing, restarts it
//! if it crashes, forwards its events to the tray and the web view.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use glidedesk_ipc::{AgentClient, AgentStatus, Endpoint, Event, IpcError, NoticeView, Request};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Shared between the supervisor, tray and commands.
#[derive(Debug, Default)]
pub struct Link {
    client: Mutex<Option<Arc<AgentClient>>>,
    status: RwLock<AgentStatus>,
    /// The user chose Quit: do not respawn the agent, exit when it is gone.
    pub quitting: AtomicBool,
    /// Tray "Restart": the agent exits on purpose and must come back.
    pub restarting: AtomicBool,
}

impl Link {
    pub fn status(&self) -> AgentStatus {
        self.status.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub async fn request(&self, req: Request) -> Result<Value, String> {
        let client = self.client.lock().await.clone().ok_or_else(|| "Glidedesk agent is starting…".to_owned())?;
        client.request(req).await.map_err(|e| e.to_string())
    }
}

fn spawn_agent() {
    // The agent is this same executable in background mode.
    let Ok(path) = std::env::current_exe() else {
        warn!("cannot find the Glidedesk executable");
        return;
    };
    let mut cmd = Command::new(&path);
    cmd.arg("--agent");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => info!(pid = child.id(), "agent started"),
        Err(e) => warn!(error = %e, "could not start the agent"),
    }
}

/// Runs forever: connect, subscribe, pump events; respawn the agent when needed.
pub async fn supervise(app: AppHandle, link: Arc<Link>) {
    let Ok(ep) = Endpoint::default_for_user() else {
        warn!("no agent endpoint");
        return;
    };
    let mut delay = Duration::from_millis(200);
    let mut spawned_at: Option<std::time::Instant> = None;
    loop {
        if link.quitting.load(Ordering::SeqCst) {
            app.exit(0);
            return;
        }
        match AgentClient::connect(&ep).await {
            Ok((client, mut events)) => {
                delay = Duration::from_millis(200);
                if let Err(e) = client.request(Request::Subscribe).await {
                    warn!(error = %e, "subscribing to the agent failed; retrying");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(2));
                    continue;
                }
                *link.client.lock().await = Some(client.clone());
                link.restarting.store(false, Ordering::SeqCst);
                crate::sync_autostart(&app, &link).await;
                while let Some(ev) = events.recv().await {
                    match ev {
                        Event::Status(s) => {
                            let prev = link.status();
                            if !prev.version.is_empty()
                                && !prev.permissions.accessibility
                                && s.permissions.accessibility
                            {
                                // Granted in System Settings: come back to the front.
                                crate::windows::raise_main(&app);
                            }
                            *link.status.write().unwrap_or_else(std::sync::PoisonError::into_inner) = (*s).clone();
                            let _ = app.emit("agent-status", &*s);
                            refresh_tray(&app, *s);
                        }
                        Event::Notice(n) => on_notice(&app, &n),
                        Event::ConfigChanged => {
                            let _ = app.emit("agent-config", ());
                        }
                        Event::Exiting => {
                            if !link.restarting.load(Ordering::SeqCst) {
                                // Quit from the tray, the installer (upgrade) or OS shutdown.
                                link.quitting.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
                *link.client.lock().await = None;
                refresh_tray(&app, AgentStatus::default());
            }
            Err(IpcError::NotRunning | IpcError::Io(_)) => {
                // Avoid spawn storms: at most one spawn per 3 s.
                if !link.quitting.load(Ordering::SeqCst)
                    && spawned_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3))
                {
                    spawn_agent();
                    spawned_at = Some(std::time::Instant::now());
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(2));
            }
            Err(e) => {
                warn!(error = %e, "agent connection failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

fn on_notice(app: &AppHandle, n: &NoticeView) {
    use tauri_plugin_notification::NotificationExt as _;
    let (title, body) = match n {
        NoticeView::Identify { label } => {
            crate::windows::show_identify(app, label);
            return;
        }
        NoticeView::NewClient { name, address, .. } => {
            ("New computer connected".to_owned(), format!("{name} ({address}) joined. You can place it in Layout."))
        }
        NoticeView::ClientOnline { name, .. } => (format!("{name} is online"), "Ready to use.".to_owned()),
        NoticeView::ClientOffline { name, .. } => {
            (format!("{name} went offline"), "The cursor will not move there until it is back.".to_owned())
        }
        NoticeView::ServerConnected { name } => {
            ("Connected".to_owned(), format!("This computer is now controlled by {name}."))
        }
        NoticeView::Error { message } => ("Glidedesk problem".to_owned(), message.clone()),
        NoticeView::Info { message } => ("Glidedesk".to_owned(), message.clone()),
    };
    let _ = app.emit("agent-notice", n);
    if crate::notifications_enabled(app) {
        let _ = app.notification().builder().title(title).body(body).show();
    }
    let _ = app.get_webview_window("main");
}

/// Updates the tray on the main thread. Called from here (an async worker) the
/// menu's text updates left their autoreleased objects on a thread that never
/// drains them (macOS): the app's memory grew with every status, once a second.
fn refresh_tray(app: &AppHandle, status: AgentStatus) {
    let handle = app.clone();
    if app.run_on_main_thread(move || crate::tray::refresh(&handle, &status)).is_err() {
        tracing::debug!("tray not refreshed: the app is closing");
    }
}
