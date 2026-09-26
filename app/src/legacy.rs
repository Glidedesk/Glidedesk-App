//! Takes over from a copy installed under the app's former name
//! (`nexpingdesk_config::LEGACY_APP_DIR_NAME`).
//!
//! Two copies would both capture the keyboard and mouse, so the old one must
//! go. Linux packages replace the old package and the Windows installer removes
//! the old program; on macOS the old app bundle is stopped and moved to the
//! Trash here. Old login entries are removed on every platform. Settings are
//! moved by the agent when it first opens them (`ConfigStore::default_location`).

/// Runs once per start of the app (not the agent); cheap when nothing is left.
pub fn retire_old_install() {
    crate::autostart::remove_legacy();
    #[cfg(target_os = "macos")]
    retire_old_bundle();
}

#[cfg(target_os = "macos")]
fn retire_old_bundle() {
    use std::path::PathBuf;
    use std::process::Command;

    let name = nexpingdesk_config::LEGACY_APP_DIR_NAME;
    let bundle = format!("{name}.app");
    let mut places = vec![PathBuf::from("/Applications").join(&bundle)];
    if let Some(home) = std::env::var_os("HOME") {
        places.push(PathBuf::from(home).join("Applications").join(&bundle));
    }
    for app in places.into_iter().filter(|p| p.is_dir()) {
        // Its agent quits (the old binary knows where its own agent listens).
        let exe = app.join("Contents/MacOS").join(name.to_lowercase());
        if exe.is_file() {
            let _ = Command::new(&exe).arg("--shutdown").status();
        }
        match nexpingdesk_platform::trash(&app) {
            Ok(()) => tracing::info!(app = %app.display(), "moved the app's former version to the Trash"),
            Err(e) => tracing::warn!(app = %app.display(), error = %e, "could not remove the app's former version"),
        }
    }
}
