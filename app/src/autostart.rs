//! Start at login (PLAN §13.1 R25, §14 B1).
//!
//! Windows and Linux use the autostart plugin (per-user `Run` key / XDG
//! autostart entry). macOS writes its own `LaunchAgent` that opens the **app
//! bundle** through `LaunchServices` (`open -g -j -a …`). Starting the binary
//! directly (as the plugin does) gives a process macOS doesn't treat as the app,
//! so permission prompts had no app name. `AssociatedBundleIdentifiers` shows
//! Glidedesk with its name and icon under Login Items.

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
    let want = mac::plist(&mac::launch_target().ok_or("cannot find Glidedesk.app")?);
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

    pub const LABEL: &str = "app.glidedesk.desktop.login";
    const BUNDLE_ID: &str = "app.glidedesk.desktop";

    pub fn plist_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    /// `…/Glidedesk.app` when running from a bundle, else the executable itself.
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
            // LaunchServices start: macOS knows it is Glidedesk (name, icon, permissions).
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

    /// Versions up to 0.1 registered `~/Library/LaunchAgents/Glidedesk.plist`,
    /// which started the binary directly. Remove it (only if it is ours).
    pub fn remove_legacy_agent() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let old = PathBuf::from(home).join("Library/LaunchAgents/Glidedesk.plist");
        if let Ok(text) = std::fs::read_to_string(&old)
            && text.contains("Glidedesk.app/Contents/MacOS/glidedesk")
        {
            let _ = std::fs::remove_file(&old);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn opens_the_bundle_through_launch_services() {
            let p = plist(Path::new("/Applications/Glidedesk.app"));
            assert!(p.contains("<string>/usr/bin/open</string>"));
            assert!(p.contains("<string>/Applications/Glidedesk.app</string>"));
            assert!(p.contains("<string>--background</string>"));
            assert!(p.contains("AssociatedBundleIdentifiers"));
            let dev = plist(Path::new("/tmp/a&b/glidedesk"));
            assert!(dev.contains("/tmp/a&amp;b/glidedesk"));
            assert!(!dev.contains("/usr/bin/open"));
        }
    }
}
