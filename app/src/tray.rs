//! Menu-bar / notification-area icon and menu (PLAN §6.8).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use glidedesk_config::Role;
use glidedesk_ipc::{AgentStatus, HealthState, LinkState, Request};
use glidedesk_proto::DeviceId;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::link::Link;

const TRAY_ID: &str = "glidedesk";

/// Last menu "signature": only rebuild the native menu when it would look different.
#[derive(Default)]
pub struct TrayState {
    signature: Mutex<String>,
}

fn icon(stopped: bool) -> Option<Image<'static>> {
    let bytes: &'static [u8] = if cfg!(target_os = "macos") {
        if stopped {
            include_bytes!("../icons/tray-template-stopped@2x.png")
        } else {
            include_bytes!("../icons/tray-template@2x.png")
        }
    } else if crate::windows::taskbar_is_light() {
        if stopped {
            include_bytes!("../icons/tray-dark-stopped.png")
        } else {
            include_bytes!("../icons/tray-dark.png")
        }
    } else if stopped {
        include_bytes!("../icons/tray-light-stopped.png")
    } else {
        include_bytes!("../icons/tray-light.png")
    };
    Image::from_bytes(bytes).ok()
}

pub fn create(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let menu = Menu::with_items(app, &[&MenuItem::with_id(app, "open", "Open Glidedesk…", true, None::<&str>)?])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Glidedesk")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            // Windows convention: double-click opens the main window.
            if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = event {
                crate::windows::show_main(tray.app_handle(), None);
            }
            let _ = MouseButtonState::Up;
        });
    if let Some(i) = icon(true) {
        builder = builder.icon(i).icon_as_template(cfg!(target_os = "macos"));
    }
    builder.build(app)
}

fn state_label(s: HealthState) -> &'static str {
    match s {
        HealthState::Online => "online",
        HealthState::Degraded => "slow",
        HealthState::Locked => "locked",
        HealthState::Connecting => "connecting",
        HealthState::Offline => "offline",
        HealthState::NeverConnected => "not connected yet",
    }
}

fn dot(s: HealthState) -> &'static str {
    match s {
        HealthState::Online => "🟢",
        HealthState::Degraded => "🟡",
        HealthState::Locked => "🔒",
        HealthState::Connecting => "🔄",
        HealthState::Offline | HealthState::NeverConnected => "⚪",
    }
}

fn ago(unix: Option<u64>) -> String {
    let Some(t) = unix else { return String::new() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let d = now.saturating_sub(t);
    if d < 90 {
        " · last seen just now".into()
    } else if d < 3600 * 2 {
        format!(" · last seen {} min ago", d / 60)
    } else if d < 86_400 * 2 {
        format!(" · last seen {} h ago", d / 3600)
    } else {
        format!(" · last seen {} days ago", d / 86_400)
    }
}

struct Model {
    header: String,
    running: bool,
    role: Role,
    clients: Vec<(DeviceId, String)>,
    wakeable: Vec<(DeviceId, String)>,
    locked: bool,
    clipboard: bool,
    files: bool,
    connected: bool,
}

fn model(s: &AgentStatus, clipboard: bool, files: bool) -> Model {
    let mut clients = Vec::new();
    let mut wakeable = Vec::new();
    let header = match (s.role, &s.server, &s.client) {
        (_, _, _) if s.version.is_empty() => "Glidedesk — starting…".to_owned(),
        (Role::Unset, ..) => "Glidedesk — set up needed".to_owned(),
        (Role::Server, Some(v), _) => {
            for c in &v.clients {
                #[allow(clippy::cast_possible_truncation)]
                let latency = c
                    .latency_ms
                    .filter(|_| c.state.reachable())
                    .map(|l| format!(" · {} ms", (l / 5.0).round() as i64 * 5))
                    .unwrap_or_default();
                let seen = if c.state.reachable() { String::new() } else { ago(c.last_seen) };
                clients.push((c.id, format!("{} {} — {}{latency}{seen}", dot(c.state), c.name, state_label(c.state))));
                if !c.state.reachable() {
                    wakeable.push((c.id, c.name.clone()));
                }
            }
            let online = v.clients.iter().filter(|c| c.state.reachable()).count();
            format!("Glidedesk — Server · Sharing ({online} of {} online)", v.clients.len())
        }
        (Role::Client, _, Some(c)) => match c.state {
            LinkState::Connected => format!("Glidedesk — Connected to {}", c.server_name.clone().unwrap_or_default()),
            LinkState::Searching => "Glidedesk — Searching for the server…".into(),
            LinkState::Connecting => "Glidedesk — Connecting…".into(),
            LinkState::Rejected => "Glidedesk — Refused by the server".into(),
            LinkState::Offline => "Glidedesk — Server offline".into(),
            LinkState::Stopped => "Glidedesk — Stopped".into(),
        },
        _ if !s.running => {
            if s.error.is_some() {
                "Glidedesk — Not sharing (see Settings)".into()
            } else {
                "Glidedesk — Stopped".into()
            }
        }
        _ => "Glidedesk".into(),
    };
    Model {
        header,
        running: s.running,
        role: s.role,
        clients,
        wakeable,
        locked: s.server.as_ref().is_some_and(|v| v.locked),
        clipboard,
        files,
        connected: !s.version.is_empty(),
    }
}

fn build(app: &AppHandle, m: &Model) -> tauri::Result<Menu<Wry>> {
    let sep = || PredefinedMenuItem::separator(app);
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    items.push(Box::new(MenuItem::with_id(app, "header", &m.header, false, None::<&str>)?));
    items.push(Box::new(sep()?));

    if m.role == Role::Server && !m.clients.is_empty() {
        let mut subs: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
        for (id, label) in &m.clients {
            subs.push(Box::new(MenuItem::with_id(app, format!("switch:{id}"), label, true, None::<&str>)?));
        }
        if !m.wakeable.is_empty() {
            subs.push(Box::new(sep()?));
            for (id, name) in &m.wakeable {
                subs.push(Box::new(MenuItem::with_id(
                    app,
                    format!("wake:{id}"),
                    format!("Wake {name}"),
                    true,
                    None::<&str>,
                )?));
            }
        }
        let refs: Vec<&dyn IsMenuItem<Wry>> = subs.iter().map(AsRef::as_ref).collect();
        items.push(Box::new(Submenu::with_id_and_items(app, "computers", "Computers", true, &refs)?));
        items.push(Box::new(sep()?));
    }
    if m.role != Role::Unset && m.connected {
        items.push(Box::new(CheckMenuItem::with_id(
            app,
            "clipboard",
            "Share clipboard",
            true,
            m.clipboard,
            None::<&str>,
        )?));
        items.push(Box::new(CheckMenuItem::with_id(app, "files", "Share files", true, m.files, None::<&str>)?));
        if m.role == Role::Server {
            items.push(Box::new(CheckMenuItem::with_id(
                app,
                "lock",
                "Lock cursor to this screen",
                m.running,
                m.locked,
                None::<&str>,
            )?));
        }
        items.push(Box::new(sep()?));
        items.push(Box::new(MenuItem::with_id(
            app,
            "toggle",
            if m.running { "■ Stop sharing" } else { "▶ Start sharing" },
            true,
            None::<&str>,
        )?));
        items.push(Box::new(MenuItem::with_id(app, "restart", "↻ Restart Glidedesk", true, None::<&str>)?));
        items.push(Box::new(MenuItem::with_id(app, "reconnect", "⟳ Reconnect all", m.running, None::<&str>)?));
        items.push(Box::new(MenuItem::with_id(app, "identify", "Identify screens", m.running, None::<&str>)?));
        items.push(Box::new(sep()?));
    }
    items.push(Box::new(MenuItem::with_id(app, "open", "Open Glidedesk…", true, None::<&str>)?));
    items.push(Box::new(MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?));
    items.push(Box::new(sep()?));
    items.push(Box::new(MenuItem::with_id(app, "quit", "Quit Glidedesk", true, None::<&str>)?));
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(AsRef::as_ref).collect();
    Menu::with_items(app, &refs)
}

/// Rebuilds the tray from a new status (cheap no-op when nothing visible changed).
pub fn refresh(app: &AppHandle, s: &AgentStatus) {
    let (clipboard, files) = crate::sharing_flags(app);
    let m = model(s, clipboard, files);
    let signature = format!(
        "{}|{}|{:?}|{:?}|{}|{}|{}|{}|{:?}",
        m.header, m.running, m.role, m.clients, m.locked, m.clipboard, m.files, m.connected, m.wakeable
    );
    let tray_state = app.state::<TrayState>();
    {
        let mut last = tray_state.signature.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if *last == signature {
            return;
        }
        last.clone_from(&signature);
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    if let Ok(menu) = build(app, &m) {
        let _ = tray.set_menu(Some(menu));
    }
    let stopped = !s.running || s.error.is_some();
    if let Some(i) = icon(stopped) {
        let _ = tray.set_icon(Some(i));
        #[cfg(target_os = "macos")]
        let _ = tray.set_icon_as_template(true);
    }
    let _ = tray.set_tooltip(Some(m.header));
}

fn on_menu(app: &AppHandle, id: &str) {
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move {
        let link: Arc<Link> = app.state::<Arc<Link>>().inner().clone();
        let result = match id.as_str() {
            "open" => {
                crate::windows::show_main(&app, None);
                Ok(serde_json::Value::Null)
            }
            "settings" => {
                crate::windows::show_main(&app, Some("general"));
                Ok(serde_json::Value::Null)
            }
            "toggle" => link.request(if link.status().running { Request::Stop } else { Request::Start }).await,
            "restart" => {
                link.restarting.store(true, Ordering::SeqCst);
                link.request(Request::Quit).await
            }
            "reconnect" => link.request(Request::ReconnectAll).await,
            "identify" => link.request(Request::Identify).await,
            "lock" => {
                let locked = link.status().server.is_some_and(|v| v.locked);
                link.request(Request::SetLocked { locked: !locked }).await
            }
            "clipboard" | "files" => crate::toggle_sharing(&app, &link, id == "clipboard").await,
            "quit" => {
                link.quitting.store(true, Ordering::SeqCst);
                let r = link.request(Request::Quit).await;
                // If the agent is already gone, exit right away.
                if r.is_err() {
                    app.exit(0);
                }
                r
            }
            other => {
                if let Some(id) = other.strip_prefix("switch:").and_then(DeviceId::parse) {
                    link.request(Request::SwitchTo { id }).await
                } else if let Some(id) = other.strip_prefix("wake:").and_then(DeviceId::parse) {
                    link.request(Request::Wake { id }).await
                } else {
                    Ok(serde_json::Value::Null)
                }
            }
        };
        if let Err(e) = result {
            use tauri_plugin_notification::NotificationExt as _;
            let _ = app.notification().builder().title("Glidedesk").body(e).show();
        }
    });
}
