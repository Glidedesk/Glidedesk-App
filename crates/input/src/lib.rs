//! Keyboard/mouse capture (server) and injection (client), display
//! enumeration and permission checks, behind platform-neutral traits.
//!
//! Capture callbacks run on a dedicated OS thread inside the system's input
//! path, so they never block: events are handed over with `try_send`, and the
//! only decision taken synchronously is "grabbed → swallow the local event".

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
mod gate;
pub mod hotkey;
pub mod keymap;
pub mod mock;
pub mod modifiers;
pub mod paste;
pub mod remap;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::fmt::Debug;
use std::sync::Arc;

use glidedesk_proto::{Input, KeyCode, LedState, MonitorInfo, MouseButton, Point};
use tokio::sync::mpsc;

pub use hotkey::{Hotkey, HotkeyError};
pub use modifiers::Pressed;
pub use remap::Remap;

/// Capacity of the capture → core channel. Full = events are dropped rather
/// than ever blocking the OS input thread.
pub const CAPTURE_QUEUE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureEvent {
    /// Pointer motion. `pos` is the local cursor position (meaningless while
    /// grabbed); `dx/dy` is the raw push, also valid at screen edges.
    Motion {
        pos: Point,
        dx: i32,
        dy: i32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    /// 1/120-notch units, +y = away from the user.
    Wheel {
        dx: i32,
        dy: i32,
    },
    Key {
        key: KeyCode,
        down: bool,
    },
    /// Monitor layout or resolution changed.
    DisplaysChanged,
    /// The OS stopped delivering events for a moment (macOS tap timeout) and
    /// capture was re-armed. Held keys may have been lost.
    Interrupted,
    /// The local paste shortcut was held back because files are only offered
    /// (§14.2). Fetch them, then call `CaptureControl::replay_paste(key)`.
    PasteRequested {
        key: KeyCode,
    },
}

/// Control surface of a running capture thread.
pub trait CaptureControl: Send + Sync + Debug {
    /// `true`: swallow local input, hide and pin the local cursor.
    fn set_grab(&self, grab: bool);
    /// Move the local cursor.
    fn warp(&self, p: Point);
    /// Current local cursor position.
    fn cursor(&self) -> Option<Point>;
    /// Caps/Num/Scroll Lock state of this machine (sent to a client on enter).
    fn leds(&self) -> LedState {
        LedState::default()
    }
    /// Stops the capture thread.
    fn stop(&self);
    /// Hold this computer's paste shortcut while offered files wait (§14.2).
    /// Backends that can't hold keys ignore it (files are then fetched at once).
    fn set_paste_hold(&self, _on: bool) {}
    /// Can this backend hold the paste shortcut?
    fn holds_paste(&self) -> bool {
        false
    }
    /// Performs the held paste now (the files are on the clipboard).
    fn replay_paste(&self, _key: KeyCode) {}
}

pub struct Capture {
    pub control: Arc<dyn CaptureControl>,
    pub events: mpsc::Receiver<CaptureEvent>,
}

impl Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture").field("control", &self.control).finish_non_exhaustive()
    }
}

/// Synthesises input on this machine (client side).
pub trait Injector: Send + Debug {
    fn inject(&mut self, ev: &Input) -> Result<(), InputError>;
    /// Releases every key and button pressed through this injector.
    fn release_all(&mut self);
    fn set_leds(&mut self, leds: LedState);
    fn cursor(&self) -> Option<Point>;
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("not supported on this platform")]
    Unsupported,
    #[error("missing permission: {0}")]
    Permission(&'static str),
    #[error("system call failed: {0}")]
    Os(String),
}

/// OS permissions the app needs (PLAN §6.6, §14 B2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Permissions {
    /// macOS Accessibility: posting events *and* the active event tap the
    /// server uses. Linux: a virtual input device (uinput) or `XTest`. Always true on Windows.
    pub accessibility: bool,
    /// macOS Input Monitoring (listen-only taps — not needed by Glidedesk, shown for
    /// information). Linux: an X11 session, needed to share this computer's input.
    pub input_monitoring: bool,
}

impl Permissions {
    /// Everything the given role needs is allowed.
    #[must_use]
    pub const fn ready(&self, server: bool) -> bool {
        if cfg!(target_os = "linux") && server { self.input_monitoring } else { self.accessibility }
    }
}

/// Starts capturing local input (server).
pub fn start_capture() -> Result<Capture, InputError> {
    #[cfg(target_os = "macos")]
    return macos::start_capture();
    #[cfg(target_os = "windows")]
    return windows::start_capture();
    #[cfg(target_os = "linux")]
    return linux::start_capture();
    #[allow(unreachable_code)]
    Err(InputError::Unsupported)
}

/// Creates an injector (client).
pub fn injector() -> Result<Box<dyn Injector>, InputError> {
    #[cfg(target_os = "macos")]
    return macos::injector();
    #[cfg(target_os = "windows")]
    return windows::injector();
    #[cfg(target_os = "linux")]
    return linux::injector();
    #[allow(unreachable_code)]
    Err(InputError::Unsupported)
}

/// Connected monitors in native desktop coordinates.
pub fn monitors() -> Result<Vec<MonitorInfo>, InputError> {
    #[cfg(target_os = "macos")]
    return macos::monitors();
    #[cfg(target_os = "windows")]
    return windows::monitors();
    #[cfg(target_os = "linux")]
    return linux::monitors();
    #[allow(unreachable_code)]
    Err(InputError::Unsupported)
}

#[must_use]
pub fn permissions() -> Permissions {
    #[cfg(target_os = "macos")]
    return macos::permissions();
    #[cfg(target_os = "linux")]
    return linux::permissions();
    #[allow(unreachable_code)]
    Permissions { accessibility: true, input_monitoring: true }
}

/// Shows the OS permission prompts (macOS) — no-op elsewhere.
pub fn request_permissions() {
    #[cfg(target_os = "macos")]
    macos::request_permissions();
}

/// Name of the foreground app if it is full screen (lower-case executable
/// name, e.g. `game.exe`, `keynote`), else `None`.
#[must_use]
pub fn fullscreen_app() -> Option<String> {
    #[cfg(target_os = "macos")]
    return macos::fullscreen_app();
    #[cfg(target_os = "windows")]
    return windows::fullscreen_app();
    #[cfg(target_os = "linux")]
    return linux::fullscreen_app();
    #[allow(unreachable_code)]
    None
}

/// macOS Secure Keyboard Entry is on: key presses can't be captured (they stay
/// on this computer). Always `false` elsewhere.
#[must_use]
pub fn secure_input_active() -> bool {
    #[cfg(target_os = "macos")]
    return macos::secure_input_active();
    #[allow(unreachable_code)]
    false
}

/// Called once at process start (Windows: per-monitor DPI awareness).
pub fn init_process() {
    #[cfg(target_os = "windows")]
    windows::init_process();
}
