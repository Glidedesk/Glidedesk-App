//! macOS backend: `CGEventTap` capture, `CGEventPost` injection, CoreGraphics
//! display enumeration, Accessibility / Input Monitoring checks.
//!
//! Key events are never logged: the capture sees every keystroke on the
//! machine and only forwards them to a client while the cursor is there.

#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use glidedesk_proto::{Input, KeyCode, LedState, MonitorId, MonitorInfo, MouseButton, Point, Rect};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, CGPoint, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGAssociateMouseAndMouseCursorPosition, CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode,
    CGDisplayHideCursor, CGDisplayIsBuiltin, CGDisplayMode, CGDisplayModelNumber, CGDisplaySerialNumber,
    CGDisplayShowCursor, CGDisplayVendorNumber, CGEvent, CGEventField, CGEventFlags, CGEventSource,
    CGEventSourceStateID, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
    CGGetActiveDisplayList, CGMainDisplayID, CGMouseButton, CGPreflightListenEventAccess, CGPreflightPostEventAccess,
    CGRequestPostEventAccess, CGScrollEventUnit, CGWarpMouseCursorPosition,
};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::keymap::{self, hid};
use crate::{CAPTURE_QUEUE, Capture, CaptureControl, CaptureEvent, Injector, InputError, Permissions, Pressed};

/// Wheel units (1/120 notch) per pixel of continuous (trackpad) scrolling.
const UNITS_PER_PIXEL: f64 = 3.0;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// displays, cursor, permissions
// ---------------------------------------------------------------------------

#[allow(clippy::cast_possible_truncation)]
fn to_rect(r: objc2_core_foundation::CGRect) -> Rect {
    Rect::new(
        r.origin.x.floor() as i32,
        r.origin.y.floor() as i32,
        r.size.width.round() as i32,
        r.size.height.round() as i32,
    )
}

fn display_ids() -> Vec<CGDirectDisplayID> {
    let mut ids = [0 as CGDirectDisplayID; 32];
    let mut count = 0u32;
    // SAFETY: both pointers are valid for the given capacity (32).
    let err = unsafe { CGGetActiveDisplayList(32, ids.as_mut_ptr(), &raw mut count) };
    if err.0 != 0 {
        return Vec::new();
    }
    ids[..count.min(32) as usize].to_vec()
}

#[allow(clippy::cast_precision_loss)]
pub fn monitors() -> Result<Vec<MonitorInfo>, InputError> {
    let main = CGMainDisplayID();
    let mut out = Vec::new();
    for id in display_ids() {
        let bounds = to_rect(CGDisplayBounds(id));
        let mode: Option<CFRetained<CGDisplayMode>> = CGDisplayCopyDisplayMode(id);
        let scale = mode.as_deref().map_or(1.0, |m| {
            let w = CGDisplayMode::width(Some(m));
            let pw = CGDisplayMode::pixel_width(Some(m));
            if w == 0 { 1.0 } else { pw as f32 / w as f32 }
        });
        let (vendor, model, serial) = (CGDisplayVendorNumber(id), CGDisplayModelNumber(id), CGDisplaySerialNumber(id));
        let builtin = CGDisplayIsBuiltin(id);
        let stable = if builtin {
            "mac:builtin".to_owned()
        } else if serial != 0 {
            format!("mac:{vendor:x}:{model:x}:{serial:x}")
        } else {
            format!("mac:{vendor:x}:{model:x}:@{},{}", bounds.x, bounds.y)
        };
        out.push(MonitorInfo {
            id: MonitorId(stable),
            name: if builtin { "Built-in Display".into() } else { format!("Display {vendor:04X}-{model:04X}") },
            bounds,
            scale,
            primary: id == main,
        });
    }
    if out.is_empty() {
        return Err(InputError::Os("no active displays".into()));
    }
    Ok(out)
}

#[allow(clippy::cast_possible_truncation)]
fn cursor_pos() -> Option<Point> {
    let ev = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&ev));
    Some(Point::new(p.x.floor() as i32, p.y.floor() as i32))
}

fn cg(p: Point) -> CGPoint {
    CGPoint { x: f64::from(p.x), y: f64::from(p.y) }
}

/// Warps the cursor to `p` without the 0.25 s input pause macOS adds after a
/// warp, and keeps mouse and cursor disconnected while grabbed.
fn repin(p: Point) {
    let _ = CGWarpMouseCursorPosition(cg(p));
    // Re-associating right after a warp cancels the local-events suppression interval.
    let _ = CGAssociateMouseAndMouseCursorPosition(true);
    let _ = CGAssociateMouseAndMouseCursorPosition(false);
}

/// macOS "natural" scrolling (System Settings → Mouse/Trackpad). Scroll is sent
/// between computers in *device* direction (wheel away from you = up, as on a
/// PC); each Mac turns it natural again on its own side, so every computer
/// scrolls the way it is set up (§14 B8). Cached; re-read every few seconds.
fn natural_scrolling() -> bool {
    use std::sync::atomic::AtomicU64;
    static VALUE: AtomicBool = AtomicBool::new(true);
    static READ_AT: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if now.saturating_sub(READ_AT.load(Ordering::Relaxed)) >= 5 {
        READ_AT.store(now, Ordering::Relaxed);
        let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
        let key = objc2_foundation::NSString::from_str("com.apple.swipescrolldirection");
        // Missing key = the system default, which is natural scrolling on.
        let on = defaults.objectForKey(&key).is_none() || defaults.boolForKey(&key);
        VALUE.store(on, Ordering::Relaxed);
    }
    VALUE.load(Ordering::Relaxed)
}

/// Distance (points) the real cursor may drift from the pin before it is pulled back.
const REPIN_DRIFT: i32 = 60;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

/// Secure Keyboard Entry (password fields, Terminal's "Secure Keyboard Entry",
/// some password managers) hides key presses from every event tap: typing then
/// stays on this Mac instead of going to the other computer.
pub fn secure_input_active() -> bool {
    // SAFETY: HIToolbox query without arguments.
    unsafe { IsSecureEventInputEnabled() }
}

pub fn permissions() -> Permissions {
    Permissions { accessibility: CGPreflightPostEventAccess(), input_monitoring: CGPreflightListenEventAccess() }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

/// Shows macOS's "“Glidedesk” would like to control this computer" prompt and
/// adds the app to the Accessibility list. Call it from the app process (the one
/// macOS knows as Glidedesk), not from the background agent.
///
/// Only Accessibility is requested: an *active* event tap and posting events
/// both fall under it. Input Monitoring covers listen-only taps, which Glidedesk
/// doesn't use, and granting it makes macOS ask to quit the app.
pub fn request_permissions() {
    if CGPreflightPostEventAccess() {
        return;
    }
    let key = objc2_foundation::NSString::from_str("AXTrustedCheckOptionPrompt");
    let yes = objc2_foundation::NSNumber::new_bool(true);
    let options = objc2_foundation::NSDictionary::from_slices(&[&*key], &[&*yes]);
    // SAFETY: an NSDictionary is toll-free bridged to the CFDictionary the call expects.
    let trusted = unsafe { AXIsProcessTrustedWithOptions(objc2::rc::Retained::as_ptr(&options).cast()) };
    if !trusted {
        // Older systems only register the app through this call.
        let _ = CGRequestPostEventAccess();
    }
}

/// Lets a background process hide the cursor (same technique as Synergy/Deskflow).
/// Resolved at run time; if the private symbol is missing we simply keep the cursor visible.
fn allow_background_cursor_hiding() {
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    type MainConn = unsafe extern "C" fn() -> i32;
    type SetProp = unsafe extern "C" fn(i32, i32, *const c_void, *const c_void) -> i32;
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let sym = |name: &CStr| {
            // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name is always safe to call.
            let p = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
            NonNull::new(p)
        };
        let (Some(conn), Some(set)) = (sym(c"_CGSDefaultConnection"), sym(c"CGSSetConnectionProperty")) else {
            debug!("background cursor hiding unavailable");
            return;
        };
        let key = objc2_core_foundation::CFString::from_static_str("SetsCursorInBackground");
        let yes = objc2_core_foundation::CFBoolean::new(true);
        // SAFETY: the symbols have these C signatures (stable since 10.x);
        // CF objects are valid for the duration of the call.
        unsafe {
            let conn: MainConn = std::mem::transmute(conn.as_ptr());
            let set: SetProp = std::mem::transmute(set.as_ptr());
            let c = conn();
            set(c, c, (&raw const *key).cast(), (&raw const *yes).cast());
        }
    });
}

// ---------------------------------------------------------------------------
// capture
// ---------------------------------------------------------------------------

struct SendRunLoop(CFRetained<CFRunLoop>);
// SAFETY: CFRunLoopStop / CFRunLoopWakeUp are documented as thread-safe; we
// only call those from other threads.
unsafe impl Send for SendRunLoop {}
unsafe impl Sync for SendRunLoop {}

struct SendPort(CFRetained<CFMachPort>);
// SAFETY: CGEventTapEnable may be called from any thread.
unsafe impl Send for SendPort {}
unsafe impl Sync for SendPort {}

struct Shared {
    grabbed: AtomicBool,
    stopped: AtomicBool,
    tx: mpsc::Sender<CaptureEvent>,
    run_loop: OnceLock<SendRunLoop>,
    tap: OnceLock<SendPort>,
    pin: Mutex<Point>,
    /// Device-dependent modifier bits last seen (to derive up/down).
    last_flags: Mutex<u64>,
    gate: crate::gate::KeyGate,
}

impl Shared {
    fn emit(&self, ev: CaptureEvent) {
        // Never block the system input path; drop on overflow.
        if self.tx.try_send(ev).is_err() {
            debug!("capture queue full; event dropped");
        }
    }
}

#[derive(Clone)]
struct Control(Arc<Shared>);

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacCapture").field("grabbed", &self.0.grabbed.load(Ordering::Relaxed)).finish()
    }
}

impl CaptureControl for Control {
    fn set_grab(&self, grab: bool) {
        let was = self.0.grabbed.swap(grab, Ordering::SeqCst);
        if was == grab {
            return;
        }
        if grab {
            self.0.gate.on_grab();
            // Park the hidden cursor in the middle of the main screen. macOS keeps
            // moving the real cursor under an event tap (and "disconnect mouse from
            // cursor" only applies to the foreground app), so it is pulled back
            // here whenever it drifts (see `repin`). Parked at the edge it used to
            // hit the screen border, stop producing motion, and the pointer on the
            // other computer got stuck before it could come back (§14 B5/B7).
            let centre = to_rect(CGDisplayBounds(CGMainDisplayID()));
            let pin = Point::new(centre.x + centre.w / 2, centre.y + centre.h / 2);
            *lock(&self.0.pin) = pin;
            allow_background_cursor_hiding();
            repin(pin);
            let _ = CGDisplayHideCursor(CGMainDisplayID());
        } else {
            let _ = CGAssociateMouseAndMouseCursorPosition(true);
            let _ = CGDisplayShowCursor(CGMainDisplayID());
        }
    }

    fn warp(&self, p: Point) {
        let _ = CGWarpMouseCursorPosition(cg(p));
        // Warping briefly suppresses local events; re-associate immediately.
        if !self.0.grabbed.load(Ordering::SeqCst) {
            let _ = CGAssociateMouseAndMouseCursorPosition(true);
        }
    }

    fn cursor(&self) -> Option<Point> {
        cursor_pos()
    }

    fn leds(&self) -> LedState {
        let flags = CGEventSource::flags_state(CGEventSourceStateID::HIDSystemState);
        LedState { caps: flags.contains(CGEventFlags::MaskAlphaShift), num: false, scroll: false }
    }

    fn set_paste_hold(&self, on: bool) {
        self.0.gate.set_paste_hold(on);
    }

    fn holds_paste(&self) -> bool {
        true
    }

    fn replay_paste(&self, key: KeyCode) {
        // Injected like a client would; our own tap/hook lets it through
        // (the hold is already off, and Windows marks our events).
        match crate::injector() {
            Ok(mut inj) => {
                for (k, down) in self.0.gate.replay(key) {
                    let _ = inj.inject(&Input::Key { key: k, down });
                }
            }
            Err(e) => warn!(error = %e, "could not replay the paste"),
        }
    }

    fn stop(&self) {
        self.set_grab(false);
        self.0.stopped.store(true, Ordering::SeqCst);
        if let Some(rl) = self.0.run_loop.get() {
            rl.0.stop();
        }
    }
}

/// Event types (`NSEventType` numbers) only swallowed while grabbed, never forwarded:
/// tablet pointer/proximity, rotate/begin/end gesture, gesture, magnify, swipe,
/// smart magnify, quick look, pressure, direct touch, change mode.
const GRAB_ONLY: [u32; 13] = [23, 24, 18, 19, 20, 29, 30, 31, 32, 33, 34, 37, 38];

fn mask(types: &[CGEventType]) -> u64 {
    types.iter().fold(0u64, |m, t| m | (1u64 << t.0))
}

/// Device-dependent modifier bits (IOLLEvent.h `NX_DEVICE*KEYMASK`).
const DEV_LCTL: u64 = 0x0001;
const DEV_LSHIFT: u64 = 0x0002;
const DEV_RSHIFT: u64 = 0x0004;
const DEV_LCMD: u64 = 0x0008;
const DEV_RCMD: u64 = 0x0010;
const DEV_LALT: u64 = 0x0020;
const DEV_RALT: u64 = 0x0040;
const DEV_RCTL: u64 = 0x2000;

fn modifier_bit(mac_code: u16) -> Option<u64> {
    Some(match mac_code {
        0x3B => DEV_LCTL,
        0x3E => DEV_RCTL,
        0x38 => DEV_LSHIFT,
        0x3C => DEV_RSHIFT,
        0x3A => DEV_LALT,
        0x3D => DEV_RALT,
        0x37 => DEV_LCMD,
        0x36 => DEV_RCMD,
        _ => return None,
    })
}

#[allow(clippy::cast_possible_truncation)]
unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    ty: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user` is the `Arc<Shared>` pointer kept alive by the capture
    // thread for as long as the tap exists.
    let shared = unsafe { &*(user as *const Shared) };
    // SAFETY: the event pointer is valid for the duration of the callback.
    let ev = unsafe { event.as_ref() };
    let pass = event.as_ptr();

    if ty == CGEventType::TapDisabledByTimeout || ty == CGEventType::TapDisabledByUserInput {
        if let Some(tap) = shared.tap.get() {
            CGEvent::tap_enable(&tap.0, true);
        }
        shared.emit(CaptureEvent::Interrupted);
        return pass;
    }

    let field = |f: CGEventField| CGEvent::integer_value_field(Some(ev), f);
    match ty {
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => {
            let loc = CGEvent::location(Some(ev));
            let pos = Point::new(loc.x.floor() as i32, loc.y.floor() as i32);
            let (dx, dy) = (field(CGEventField::MouseEventDeltaX) as i32, field(CGEventField::MouseEventDeltaY) as i32);
            let pos = if shared.grabbed.load(Ordering::Relaxed) {
                let pin = *lock(&shared.pin);
                if (pos.x - pin.x).abs() > REPIN_DRIFT || (pos.y - pin.y).abs() > REPIN_DRIFT {
                    repin(pin);
                }
                pin
            } else {
                pos
            };
            shared.emit(CaptureEvent::Motion { pos, dx, dy });
        }
        CGEventType::LeftMouseDown | CGEventType::LeftMouseUp => {
            shared.emit(CaptureEvent::Button { button: MouseButton::Left, down: ty == CGEventType::LeftMouseDown });
        }
        CGEventType::RightMouseDown | CGEventType::RightMouseUp => {
            shared.emit(CaptureEvent::Button { button: MouseButton::Right, down: ty == CGEventType::RightMouseDown });
        }
        CGEventType::OtherMouseDown | CGEventType::OtherMouseUp => {
            let button = match field(CGEventField::MouseEventButtonNumber) {
                3 => MouseButton::Back,
                4 => MouseButton::Forward,
                _ => MouseButton::Middle,
            };
            shared.emit(CaptureEvent::Button { button, down: ty == CGEventType::OtherMouseDown });
        }
        CGEventType::ScrollWheel => {
            let continuous = field(CGEventField::ScrollWheelEventIsContinuous) != 0;
            let (dy, dx) = if continuous {
                let px = |f| CGEvent::double_value_field(Some(ev), f) * UNITS_PER_PIXEL;
                (px(CGEventField::ScrollWheelEventPointDeltaAxis1), px(CGEventField::ScrollWheelEventPointDeltaAxis2))
            } else {
                let fixed = |f| CGEvent::double_value_field(Some(ev), f) * 120.0;
                (
                    fixed(CGEventField::ScrollWheelEventFixedPtDeltaAxis1),
                    fixed(CGEventField::ScrollWheelEventFixedPtDeltaAxis2),
                )
            };
            // Natural scrolling inverted these before the tap saw them: undo it.
            let sign = if natural_scrolling() { -1.0 } else { 1.0 };
            let (dx, dy) = (dx * sign, dy * sign);
            if dx != 0.0 || dy != 0.0 {
                shared.emit(CaptureEvent::Wheel { dx: dx.round() as i32, dy: dy.round() as i32 });
            }
        }
        CGEventType::KeyDown | CGEventType::KeyUp => {
            let code = u16::try_from(field(CGEventField::KeyboardEventKeycode)).unwrap_or(u16::MAX);
            let down = ty == CGEventType::KeyDown;
            let grabbed = shared.grabbed.load(Ordering::Relaxed);
            if let Some(key) = keymap::mac_to_hid(code) {
                shared.emit(CaptureEvent::Key { key, down });
                let out = shared.gate.on_key(key, down, grabbed);
                if out.paste {
                    shared.emit(CaptureEvent::PasteRequested { key });
                }
                return if out.swallow { std::ptr::null_mut() } else { pass };
            }
        }
        CGEventType::FlagsChanged => {
            let code = u16::try_from(field(CGEventField::KeyboardEventKeycode)).unwrap_or(u16::MAX);
            let flags = CGEvent::flags(Some(ev)).0;
            if code == 0x39 {
                // Caps Lock only reports its toggled state: emit a full press.
                shared.emit(CaptureEvent::Key { key: KeyCode(hid::CAPS_LOCK), down: true });
                shared.emit(CaptureEvent::Key { key: KeyCode(hid::CAPS_LOCK), down: false });
            } else if let (Some(bit), Some(key)) = (modifier_bit(code), keymap::mac_to_hid(code)) {
                *lock(&shared.last_flags) = flags;
                let down = flags & bit != 0;
                shared.emit(CaptureEvent::Key { key, down });
                let grabbed = shared.grabbed.load(Ordering::Relaxed);
                return if shared.gate.on_key(key, down, grabbed).swallow { std::ptr::null_mut() } else { pass };
            }
        }
        // Trackpad gestures, Force Touch pressure and tablet events must not act on
        // this Mac (e.g. swipe between spaces) while another computer has control.
        t if GRAB_ONLY.contains(&t.0) => {}
        _ => return pass,
    }
    if shared.grabbed.load(Ordering::Relaxed) { std::ptr::null_mut() } else { pass }
}

pub fn start_capture() -> Result<Capture, InputError> {
    // An active (filtering) tap needs Accessibility only; see `request_permissions`.
    if !CGPreflightPostEventAccess() {
        return Err(InputError::Permission("Accessibility"));
    }
    let (tx, rx) = mpsc::channel(CAPTURE_QUEUE);
    let shared = Arc::new(Shared {
        grabbed: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        tx,
        run_loop: OnceLock::new(),
        tap: OnceLock::new(),
        pin: Mutex::new(Point::default()),
        last_flags: Mutex::new(0),
        gate: crate::gate::KeyGate::default(),
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), InputError>>();
    let thread_shared = shared.clone();
    std::thread::Builder::new()
        .name("gd-capture".into())
        .spawn(move || {
            let events = GRAB_ONLY.iter().fold(0u64, |m, t| m | (1u64 << t))
                | mask(&[
                    CGEventType::MouseMoved,
                    CGEventType::LeftMouseDown,
                    CGEventType::LeftMouseUp,
                    CGEventType::RightMouseDown,
                    CGEventType::RightMouseUp,
                    CGEventType::OtherMouseDown,
                    CGEventType::OtherMouseUp,
                    CGEventType::LeftMouseDragged,
                    CGEventType::RightMouseDragged,
                    CGEventType::OtherMouseDragged,
                    CGEventType::ScrollWheel,
                    CGEventType::KeyDown,
                    CGEventType::KeyUp,
                    CGEventType::FlagsChanged,
                ]);
            let user = Arc::into_raw(thread_shared.clone()) as *mut c_void;
            // SAFETY: callback matches CGEventTapCallBack; `user` stays valid
            // until `Arc::from_raw` below, after the run loop has exited.
            let tap = unsafe {
                CGEvent::tap_create(
                    CGEventTapLocation::HIDEventTap,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::Default,
                    events,
                    Some(tap_callback),
                    user,
                )
            };
            let Some(tap) = tap else {
                // SAFETY: reclaim the leaked Arc; the tap was never created.
                drop(unsafe { Arc::from_raw(user as *const Shared) });
                let _ = ready_tx.send(Err(InputError::Permission("Accessibility")));
                return;
            };
            let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
                // SAFETY: as above.
                drop(unsafe { Arc::from_raw(user as *const Shared) });
                let _ = ready_tx.send(Err(InputError::Os("CFMachPortCreateRunLoopSource failed".into())));
                return;
            };
            let Some(rl) = CFRunLoop::current() else {
                // SAFETY: as above.
                drop(unsafe { Arc::from_raw(user as *const Shared) });
                let _ = ready_tx.send(Err(InputError::Os("no run loop".into())));
                return;
            };
            // SAFETY: kCFRunLoopCommonModes is a valid static CF string.
            rl.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
            CGEvent::tap_enable(&tap, true);
            let _ = thread_shared.run_loop.set(SendRunLoop(rl));
            let _ = thread_shared.tap.set(SendPort(tap.clone()));
            let _ = ready_tx.send(Ok(()));
            while !thread_shared.stopped.load(Ordering::SeqCst) {
                CFRunLoop::run();
            }
            CGEvent::tap_enable(&tap, false);
            tap.invalidate();
            // SAFETY: the tap is disabled and invalidated; no more callbacks.
            drop(unsafe { Arc::from_raw(user as *const Shared) });
            debug!("capture thread stopped");
        })
        .map_err(|e| InputError::Os(e.to_string()))?;
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| InputError::Os("capture thread did not start".into()))??;
    Ok(Capture { control: Arc::new(Control(shared)), events: rx })
}

// ---------------------------------------------------------------------------
// injection
// ---------------------------------------------------------------------------

struct MacInjector {
    source: Option<CFRetained<CGEventSource>>,
    pressed: Pressed,
    pos: Point,
    last_click: Option<(MouseButton, Instant, Point, i64)>,
    wheel_rem: (f64, f64),
}

impl std::fmt::Debug for MacInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacInjector").field("pos", &self.pos).finish_non_exhaustive()
    }
}

// SAFETY: CGEventSource is a CF object usable from any thread; the injector is
// used by one task at a time.
unsafe impl Send for MacInjector {}

pub fn injector() -> Result<Box<dyn Injector>, InputError> {
    if !CGPreflightPostEventAccess() {
        return Err(InputError::Permission("Accessibility"));
    }
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState);
    Ok(Box::new(MacInjector {
        source,
        pressed: Pressed::default(),
        pos: cursor_pos().unwrap_or_default(),
        last_click: None,
        wheel_rem: (0.0, 0.0),
    }))
}

impl MacInjector {
    fn flags(&self) -> CGEventFlags {
        let m = self.pressed.mods();
        let mut f = CGEventFlags::empty();
        if m.shift {
            f |= CGEventFlags::MaskShift;
        }
        if m.ctrl {
            f |= CGEventFlags::MaskControl;
        }
        if m.alt {
            f |= CGEventFlags::MaskAlternate;
        }
        if m.meta {
            f |= CGEventFlags::MaskCommand;
        }
        f
    }

    fn post(ev: Option<CFRetained<CGEvent>>) -> Result<(), InputError> {
        let ev = ev.ok_or_else(|| InputError::Os("CGEventCreate failed".into()))?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        Ok(())
    }

    fn clamp_to_desktop(p: Point) -> Point {
        let rects: Vec<Rect> = display_ids().into_iter().map(|id| to_rect(CGDisplayBounds(id))).collect();
        if rects.iter().any(|r| r.contains(p)) {
            return p;
        }
        rects
            .iter()
            .map(|r| r.clamp(p))
            .min_by_key(|c| i64::from(c.x - p.x).pow(2) + i64::from(c.y - p.y).pow(2))
            .unwrap_or(p)
    }

    fn move_to(&mut self, p: Point, dx: i32, dy: i32) -> Result<(), InputError> {
        let (ty, btn) = match self.pressed.buttons().first() {
            Some(MouseButton::Left) => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
            Some(MouseButton::Right) => (CGEventType::RightMouseDragged, CGMouseButton::Right),
            Some(_) => (CGEventType::OtherMouseDragged, CGMouseButton::Center),
            None => (CGEventType::MouseMoved, CGMouseButton::Left),
        };
        let ev = CGEvent::new_mouse_event(self.source.as_deref(), ty, cg(p), btn);
        if let Some(e) = ev.as_deref() {
            CGEvent::set_integer_value_field(Some(e), CGEventField::MouseEventDeltaX, i64::from(dx));
            CGEvent::set_integer_value_field(Some(e), CGEventField::MouseEventDeltaY, i64::from(dy));
            CGEvent::set_flags(Some(e), self.flags());
        }
        self.pos = p;
        Self::post(ev)
    }

    fn button(&mut self, b: MouseButton, down: bool) -> Result<(), InputError> {
        let (down_ty, up_ty, cgb, number) = match b {
            MouseButton::Left => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGMouseButton::Left, 0),
            MouseButton::Right => (CGEventType::RightMouseDown, CGEventType::RightMouseUp, CGMouseButton::Right, 1),
            MouseButton::Middle => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGMouseButton::Center, 2),
            MouseButton::Back => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGMouseButton::Center, 3),
            MouseButton::Forward => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGMouseButton::Center, 4),
        };
        // Multi-click detection so double/triple clicks work in apps.
        let clicks = if down {
            let now = Instant::now();
            let n = match self.last_click {
                Some((lb, at, p, n))
                    if lb == b
                        && now.duration_since(at) < Duration::from_millis(500)
                        && (p.x - self.pos.x).abs() <= 4
                        && (p.y - self.pos.y).abs() <= 4 =>
                {
                    n + 1
                }
                _ => 1,
            };
            self.last_click = Some((b, now, self.pos, n));
            n
        } else {
            self.last_click.map_or(1, |c| c.3)
        };
        self.pressed.button(b, down);
        let ev =
            CGEvent::new_mouse_event(self.source.as_deref(), if down { down_ty } else { up_ty }, cg(self.pos), cgb);
        if let Some(e) = ev.as_deref() {
            CGEvent::set_integer_value_field(Some(e), CGEventField::MouseEventClickState, clicks);
            CGEvent::set_integer_value_field(Some(e), CGEventField::MouseEventButtonNumber, number);
            CGEvent::set_flags(Some(e), self.flags());
        }
        Self::post(ev)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        // Posted events skip the system's natural-scrolling inversion: apply it here.
        let sign = if natural_scrolling() { -1.0 } else { 1.0 };
        let fx = f64::from(dx) * sign / UNITS_PER_PIXEL + self.wheel_rem.0;
        let fy = f64::from(dy) * sign / UNITS_PER_PIXEL + self.wheel_rem.1;
        let (px, py) = (fx.trunc(), fy.trunc());
        self.wheel_rem = (fx - px, fy - py);
        if px == 0.0 && py == 0.0 {
            return Ok(());
        }
        let ev = CGEvent::new_scroll_wheel_event2(
            self.source.as_deref(),
            CGScrollEventUnit::Pixel,
            2,
            py as i32,
            px as i32,
            0,
        );
        Self::post(ev)
    }

    fn key(&mut self, key: KeyCode, down: bool) -> Result<(), InputError> {
        let Some(code) = keymap::hid_to_mac(key) else {
            warn!(key = key.0, "no macOS key for HID usage");
            return Ok(());
        };
        self.pressed.key(key, down);
        let ev = CGEvent::new_keyboard_event(self.source.as_deref(), code, down);
        if let Some(e) = ev.as_deref() {
            if keymap::is_modifier(key) {
                CGEvent::set_type(Some(e), CGEventType::FlagsChanged);
            }
            CGEvent::set_flags(Some(e), self.flags());
        }
        Self::post(ev)
    }
}

impl Injector for MacInjector {
    fn inject(&mut self, ev: &Input) -> Result<(), InputError> {
        match *ev {
            Input::MouseAbs(p) => {
                let p = Self::clamp_to_desktop(p);
                let (dx, dy) = (p.x - self.pos.x, p.y - self.pos.y);
                self.move_to(p, dx, dy)
            }
            Input::MouseRel { dx, dy } => {
                let p =
                    Self::clamp_to_desktop(Point::new(self.pos.x.saturating_add(dx), self.pos.y.saturating_add(dy)));
                self.move_to(p, dx, dy)
            }
            Input::Button { button, down } => self.button(button, down),
            Input::Wheel { dx, dy } => self.wheel(dx, dy),
            Input::Key { key, down } => self.key(key, down),
            Input::ReleaseAll => {
                self.release_all();
                Ok(())
            }
            Input::Leds(l) => {
                self.set_leds(l);
                Ok(())
            }
        }
    }

    fn release_all(&mut self) {
        let (keys, buttons) = self.pressed.take_all();
        for b in buttons {
            let _ = self.button(b, false);
        }
        for k in keys {
            let _ = self.key(k, false);
        }
    }

    fn set_leds(&mut self, _leds: LedState) {
        // Caps Lock state is carried by the Caps Lock key events themselves;
        // Num/Scroll Lock do not exist on macOS.
    }

    fn cursor(&self) -> Option<Point> {
        cursor_pos()
    }
}

/// Frontmost app is full screen if one of its normal-layer windows covers a whole display.
pub fn fullscreen_app() -> Option<String> {
    use objc2_app_kit::NSWorkspace;
    use objc2_core_foundation::{CFDictionary, CFNumber, CFType};
    use objc2_core_graphics::{
        CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowBounds,
        kCGWindowLayer, kCGWindowOwnerPID,
    };

    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    if app.processIdentifier() == std::process::id().cast_signed() {
        return None;
    }
    let pid = i64::from(app.processIdentifier());
    let name = app
        .executableURL()
        .and_then(|u| u.lastPathComponent())
        .map(|n| n.to_string().to_lowercase())
        .unwrap_or_default();
    let displays: Vec<Rect> = display_ids().into_iter().map(|id| to_rect(CGDisplayBounds(id))).collect();
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )?;
    // SAFETY: CGWindowList returns a CFArray of CFDictionary; the constant keys are CFStrings.
    unsafe {
        let num = |d: &CFDictionary, key: &objc2_core_foundation::CFString| -> Option<i64> {
            let v = d.value((&raw const *key).cast());
            if v.is_null() {
                return None;
            }
            (*v.cast::<CFType>()).downcast_ref::<CFNumber>()?.as_i64()
        };
        for i in 0..list.count() {
            let d = list.value_at_index(i).cast::<CFDictionary>();
            if d.is_null() {
                continue;
            }
            let d = &*d;
            if num(d, kCGWindowOwnerPID) != Some(pid) || num(d, kCGWindowLayer) != Some(0) {
                continue;
            }
            let b = d.value((&raw const *kCGWindowBounds).cast());
            if b.is_null() {
                continue;
            }
            let mut r = objc2_core_foundation::CGRect::default();
            if CGRectMakeWithDictionaryRepresentation(Some(&*b.cast::<CFDictionary>()), &raw mut r) {
                let w = to_rect(r);
                if displays
                    .iter()
                    .any(|d| w.x <= d.x && w.y <= d.y && w.right() >= d.right() && w.bottom() >= d.bottom())
                {
                    return Some(name);
                }
            }
        }
    }
    None
}
