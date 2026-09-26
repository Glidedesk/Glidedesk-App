//! Commands the web view may call. Every request is re-validated here by
//! deserialising it into the typed agent protocol — the page cannot send
//! anything the agent does not understand.

use std::sync::Arc;

use glidedesk_ipc::Request;
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt as _;

use crate::link::Link;

const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024;

#[tauri::command]
pub async fn agent(link: State<'_, Arc<Link>>, app: AppHandle, request: Value) -> Result<Value, String> {
    let req: Request = serde_json::from_value(request).map_err(|e| format!("invalid request: {e}"))?;
    let sets_config = matches!(req, Request::SetConfig { .. });
    let quit = matches!(req, Request::Quit);
    if quit {
        link.quitting.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let out = link.request(req).await;
    if sets_config && out.is_ok() {
        crate::sync_autostart(&app, &link).await;
    }
    out
}

#[tauri::command]
pub async fn status(link: State<'_, Arc<Link>>) -> Result<Value, String> {
    serde_json::to_value(link.status()).map_err(|e| e.to_string())
}

/// Export settings to a file the user picks.
#[tauri::command]
pub async fn export_settings(app: AppHandle, link: State<'_, Arc<Link>>, layout_only: bool) -> Result<bool, String> {
    let text = link.request(Request::ExportConfig { layout_only }).await?;
    let text = text.as_str().ok_or("unexpected export result")?.to_owned();
    let name = if layout_only { "glidedesk-layout.glidedesk.toml" } else { "glidedesk-settings.glidedesk.toml" };
    let picked = tokio::task::spawn_blocking(move || {
        app.dialog().file().add_filter("Glidedesk settings", &["toml"]).set_file_name(name).blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(false) };
    std::fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(true)
}

/// Pick a settings file and return the agent's import preview (nothing applied yet).
#[tauri::command]
pub async fn import_settings(app: AppHandle, link: State<'_, Arc<Link>>) -> Result<Value, String> {
    let picked = tokio::task::spawn_blocking(move || {
        app.dialog().file().add_filter("Glidedesk settings", &["toml"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(Value::Null) };
    let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if len > MAX_IMPORT_BYTES {
        return Err("that file is too large to be a Glidedesk settings file".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    link.request(Request::ImportPreview { text }).await
}

/// macOS: asks for Accessibility from *this* process — the one macOS knows as
/// Glidedesk — so the prompt and the Settings list show the app's name and icon.
/// The background agent is started by this app and shares its permission.
#[tauri::command]
pub fn request_permissions() {
    glidedesk_input::request_permissions();
}

/// Opens the relevant OS settings page (permissions, firewall) or the log folder.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command arguments are owned
pub fn open_external(target: String) -> Result<(), String> {
    let status = |r: std::io::Result<std::process::Child>| r.map(|_| ()).map_err(|e| e.to_string());
    match target.as_str() {
        #[cfg(target_os = "macos")]
        "accessibility" => status(
            std::process::Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                .spawn(),
        ),
        #[cfg(target_os = "macos")]
        "input-monitoring" => status(
            std::process::Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
                .spawn(),
        ),
        "logs" => {
            let logs = log_dir();
            #[cfg(target_os = "macos")]
            return status(std::process::Command::new("open").arg(logs).spawn());
            #[cfg(windows)]
            return status(std::process::Command::new("explorer.exe").arg(logs).spawn());
            #[cfg(target_os = "linux")]
            return {
                let _ = std::fs::create_dir_all(&logs);
                status(std::process::Command::new("xdg-open").arg(logs).spawn())
            };
            #[allow(unreachable_code)]
            Err("not supported".into())
        }
        "uninstall" if crate::is_portable() => Err(
            "This is the portable version: quit Glidedesk and delete its folder (settings are in \"Glidedesk Data\" inside it)."
                .into(),
        ),
        "uninstall" => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let dir = exe.parent().ok_or("no install folder")?;
            #[cfg(target_os = "macos")]
            {
                // Glidedesk.app/Contents/MacOS/glidedesk → Contents/Resources/uninstall.sh
                let script = dir.join("../Resources/uninstall.sh");
                return status(std::process::Command::new("/bin/sh").arg(script).spawn());
            }
            #[cfg(windows)]
            {
                // `start` goes through ShellExecute, so Windows shows the UAC prompt.
                let un = dir.join("uninstall.exe");
                return status(std::process::Command::new("cmd").args(["/C", "start", ""]).arg(un).spawn());
            }
            #[cfg(target_os = "linux")]
            {
                let _ = dir;
                return Err(
                    "On Linux, remove Glidedesk with your package manager (e.g. `sudo apt remove glidedesk`). \
                            Your settings in ~/.config/glidedesk are kept."
                        .into(),
                );
            }
            #[allow(unreachable_code)]
            Err("not supported".into())
        }
        _ => Err("unknown target".into()),
    }
}

fn log_dir() -> std::path::PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "LOCALAPPDATA" } else { "HOME" }).map(std::path::PathBuf::from);
    match home {
        Some(h) if cfg!(target_os = "macos") => h.join("Library/Logs/Glidedesk"),
        Some(h) if cfg!(windows) => h.join("Glidedesk").join("logs"),
        Some(h) => h.join(".local/share/glidedesk/logs"),
        None => std::env::temp_dir(),
    }
}
