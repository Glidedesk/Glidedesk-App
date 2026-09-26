//! Start at login.
//!
//! Windows and Linux use the autostart plugin (per-user `Run` key / XDG
//! autostart entry). macOS writes its own `LaunchAgent` that opens the **app
//! bundle** through `LaunchServices` (`open -g -j -a …`). Starting the binary
//! directly (as the plugin does) gives a process macOS doesn't treat as the app,
//! so permission prompts had no app name. `AssociatedBundleIdentifiers` shows
//! Nexpingdesk with its name and icon under Login Items.

use tauri::AppHandle;

#[cfg(not(target_os = "macos"))]
pub fn set(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt as _;
    let al = app.autolaunch();
    let has = al.is_enabled().unwrap_or(false);
    match (enabled, has) {
        (true, false) => al.enable().map_err(|e| e.to_string()),
        (false, true) => al.disable().map_err(|e| e.to_string()),
        _ => Ok(()),
    }
}

/// Removes start-at-login entries of the app's former name.
pub fn remove_legacy() {
    #[cfg(target_os = "macos")]
    mac::remove_legacy_agent();
    // XDG autostart entry of the autostart plugin, named after the product.
    #[cfg(target_os = "linux")]
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|c| c.join("autostart"))
    {
        let name = nexpingdesk_config::LEGACY_APP_DIR_NAME;
        for file in [format!("{name}.desktop"), format!("{}.desktop", name.to_lowercase())] {
            let path = dir.join(file);
            if std::fs::read_to_string(&path).is_ok_and(|t| t.to_lowercase().contains(&name.to_lowercase())) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    // The plugin's per-user `Run` value, named after the product.
    #[cfg(windows)]
    windows_run::remove(nexpingdesk_config::LEGACY_APP_DIR_NAME);
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_run {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    use windows::core::{HSTRING, w};

    pub fn remove(value: &str) {
        // SAFETY: valid key path and value name; deleting a missing value is harmless.
        let _ = unsafe {
            RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\CurrentVersion\Run"),
                &HSTRING::from(value),
            )
        };
    }
}

#[cfg(target_os = "macos")]
pub fn set(_app: &AppHandle, enabled: bool) -> Result<(), String> {
    mac::remove_legacy_agent();
    let path = mac::plist_path().ok_or("no home folder")?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let want = mac::plist(&mac::launch_target().ok_or("cannot find Nexpingdesk.app")?);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(want.as_str()) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, want).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};

    pub const LABEL: &str = "app.nexpingdesk.desktop.login";
    const BUNDLE_ID: &str = "app.nexpingdesk.desktop";

    pub fn plist_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    /// `…/Nexpingdesk.app` when running from a bundle, else the executable itself.
    pub fn launch_target() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let bundle = exe.parent()?.parent()?.parent()?;
        if bundle.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
            Some(bundle.to_path_buf())
        } else {
            Some(exe)
        }
    }

    fn xml(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    pub fn plist(target: &Path) -> String {
        let t = xml(&target.to_string_lossy());
        let args = if target.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
            // LaunchServices start: macOS knows it is Nexpingdesk (name, icon, permissions).
            format!(
                "<string>/usr/bin/open</string><string>-g</string><string>-j</string><string>-a</string>\
                 <string>{t}</string><string>--args</string><string>--background</string>"
            )
        } else {
            format!("<string>{t}</string><string>--background</string>")
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array>{args}</array>
  <key>RunAtLoad</key><true/>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>ProcessType</key><string>Interactive</string>
  <key>AssociatedBundleIdentifiers</key><array><string>{BUNDLE_ID}</string></array>
</dict>
</plist>
"#
        )
    }

    /// Login items of the app's former name (`LEGACY_APP_DIR_NAME`): the first
    /// versions' `~/Library/LaunchAgents/<Name>.plist` (it started the binary
    /// directly) and the later `app.<name>.desktop.login.plist`. Removed, only
    /// if they are ours, so the old app no longer starts at login.
    pub fn remove_legacy_agent() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let agents = PathBuf::from(home).join("Library/LaunchAgents");
        let name = nexpingdesk_config::LEGACY_APP_DIR_NAME;
        let lower = name.to_lowercase();
        for file in [format!("{name}.plist"), format!("app.{lower}.desktop.login.plist")] {
            let path = agents.join(file);
            if let Ok(text) = std::fs::read_to_string(&path)
                && text.contains(&format!("{name}.app"))
            {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn opens_the_bundle_through_launch_services() {
            let p = plist(Path::new("/Applications/Nexpingdesk.app"));
            assert!(p.contains("<string>/usr/bin/open</string>"));
            assert!(p.contains("<string>/Applications/Nexpingdesk.app</string>"));
            assert!(p.contains("<string>--background</string>"));
            assert!(p.contains("AssociatedBundleIdentifiers"));
            let dev = plist(Path::new("/tmp/a&b/nexpingdesk"));
            assert!(dev.contains("/tmp/a&amp;b/nexpingdesk"));
            assert!(!dev.contains("/usr/bin/open"));
        }
    }
}
