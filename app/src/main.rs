//! Glidedesk tray + settings app. Thin: all work happens in
//! `glidedesk-agent`; this process shows the tray, the settings window
//! (created on demand) and notifications.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod autostart;
mod commands;
mod link;
mod tray;
mod windows;

use std::sync::Arc;

use glidedesk_config::Role;
use glidedesk_ipc::Request;
use tauri::{AppHandle, Manager, RunEvent};

use crate::link::Link;

pub fn sharing_flags(app: &AppHandle) -> (bool, bool) {
    let s = app.state::<Arc<Link>>().status();
    (s.clipboard, s.files)
}

pub fn notifications_enabled(app: &AppHandle) -> bool {
    let s = app.state::<Arc<Link>>().status();
    s.notifications || s.version.is_empty()
}

/// Flips the clipboard or files switch in the right place for this role.
pub async fn toggle_sharing(_app: &AppHandle, link: &Link, clipboard: bool) -> Result<serde_json::Value, String> {
    let cfg = link.request(Request::GetConfig).await?;
    let mut cfg: glidedesk_config::Config = serde_json::from_value(cfg).map_err(|e| e.to_string())?;
    match (cfg.device.role, clipboard) {
        (Role::Client, true) => cfg.client.accept_clipboard = !cfg.client.accept_clipboard,
        (Role::Client, false) => cfg.client.accept_files = !cfg.client.accept_files,
        (_, true) => cfg.server.sharing.clipboard = !cfg.server.sharing.clipboard,
        (_, false) => cfg.server.sharing.files = !cfg.server.sharing.files,
    }
    link.request(Request::SetConfig { config: Box::new(cfg) }).await
}

/// Keeps the OS login item in line with "Start at login".
pub async fn sync_autostart(app: &AppHandle, link: &Link) {
    // Status may not have arrived yet right after connecting: ask directly.
    let want = match link.request(Request::Status).await {
        Ok(v) => v.get("start_at_login").and_then(serde_json::Value::as_bool).unwrap_or(true),
        Err(_) => return,
    };
    if let Err(e) = autostart::set(app, want) {
        tracing::warn!(error = %e, "could not update start-at-login");
    }
}

/// Name of the marker file that makes a copy portable (the Windows portable zip ships it).
pub const PORTABLE_MARKER: &str = "glidedesk.portable";

/// Portable copy: a `glidedesk.portable` file next to the executable keeps all
/// settings, logs and received files in `Glidedesk Data` beside it (nothing in
/// the user profile). Must run before any other thread starts.
fn enable_portable_mode() {
    if std::env::var_os("GLIDEDESK_HOME").is_some() {
        return;
    }
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf)) else {
        return;
    };
    if dir.join(PORTABLE_MARKER).is_file() {
        // SAFETY: called first thing in `main`, before any thread exists; the
        // agent child inherits the variable from here.
        unsafe {
            std::env::set_var("GLIDEDESK_HOME", dir.join("Glidedesk Data"));
            std::env::set_var("GLIDEDESK_PORTABLE", "1");
        }
    }
}

pub fn is_portable() -> bool {
    std::env::var_os("GLIDEDESK_PORTABLE").is_some()
}

fn main() {
    enable_portable_mode();
    // One executable does everything: background agent, self-test,
    // installer hook, or the tray/settings app.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    if has("--agent") {
        std::process::exit(i32::from(glidedesk_agent::run_agent() != std::process::ExitCode::SUCCESS));
    }
    if has("--selftest") {
        let _ = glidedesk_agent::run_selftest();
        return;
    }
    if has("--shutdown") {
        let _ = glidedesk_agent::shutdown_running_agent();
        return;
    }
    if has("--version") {
        println!("Glidedesk {}", glidedesk_agent::VERSION);
        return;
    }
    let _ =
        tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new("warn,glidedesk=info")).try_init();
    let link = Arc::new(Link::default());
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Launching again (Start menu, Finder) opens the window.
            windows::show_main(app, None);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(link.clone())
        .manage(tray::TrayState::default())
        .invoke_handler(tauri::generate_handler![
            commands::agent,
            commands::status,
            commands::export_settings,
            commands::import_settings,
            commands::open_external,
            commands::request_permissions
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            tray::create(app.handle())?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(link::supervise(handle.clone(), link.clone()));
            // First run (or opened by the user, not by login): show the window.
            let background = std::env::args().any(|a| a == "--background");
            if !background {
                windows::show_main(&handle, None);
            }
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Glidedesk could not start: {e}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            // Closing the last window keeps the tray running; only Quit exits.
            let quitting = handle.state::<Arc<Link>>().quitting.load(std::sync::atomic::Ordering::SeqCst);
            if code.is_none() && !quitting {
                api.prevent_exit();
            }
        }
    });
}
