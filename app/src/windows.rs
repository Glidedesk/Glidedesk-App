//! Window helpers. The settings window is created on demand and destroyed
//! when closed, so nothing heavy stays in memory in the tray (PLAN §6.10).

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub fn show_main(app: &AppHandle, page: Option<&str>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(p) = page {
            let _ = w.emit("navigate", p);
        }
        return;
    }
    let url = match page {
        Some(p) => format!("index.html#/{p}"),
        None => "index.html".to_owned(),
    };
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App(url.into()))
        .title("Glidedesk")
        .inner_size(1100.0, 740.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .focused(true);
    // macOS: content under a transparent title bar (like Finder / System Settings).
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    let built = builder.build();
    if let Err(e) = built {
        tracing::warn!(error = %e, "could not open the window");
    }
}

/// Big translucent label in the middle of the screen for a few seconds.
pub fn show_identify(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window("identify") {
        let _ = w.close();
    }
    let safe: String = label.chars().filter(|c| c.is_alphanumeric() || " -_.'()".contains(*c)).take(64).collect();
    let url = format!("index.html#/identify?label={}", encode(&safe));
    let w = WebviewWindowBuilder::new(app, "identify", WebviewUrl::App(url.into()))
        .title("Glidedesk")
        .inner_size(640.0, 260.0)
        .center()
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .build();
    if let Ok(w) = w {
        let _ = w.set_ignore_cursor_events(true);
        let handle = w.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let _ = handle.close();
        });
    }
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Windows: is the taskbar light (so the tray icon must be dark)?
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn taskbar_is_light() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut value = 0u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);
    // SAFETY: valid key/value names and an out-buffer of `size` bytes.
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    ok.is_ok() && value == 1
}

#[cfg(not(windows))]
pub fn taskbar_is_light() -> bool {
    false
}
