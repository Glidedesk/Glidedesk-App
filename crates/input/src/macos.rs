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

use nexpingdesk_proto::{Input, KeyCode, LedState, MonitorId, MonitorInfo, MouseButton, Point, Rect};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, CGPoint, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGAssociateMouseAndMouseCursorPosition, CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode,
    CGDisplayHideCursor, CGDisplayIsBuiltin, CGDisplayMode, CGDisplayModelNumber, CGDisplaySerialNumber,
    CGDisplayShowCursor, CGDisplayVendorNumber, CGEvent, CGEventField, CGEventFlags, CGEventSource,
    CGEventSourceStateID, CGEventTapCallBack, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGGetActiveDisplayList, CGMainDisplayID, CGMouseButton, CGPreflightListenEventAccess,
    CGPreflightPostEventAccess, CGRequestPostEventAccess, CGScrollEventUnit, CGWarpMouseCursorPosition,
};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::keymap::{self, hid};
use crate::macos_hid::{self, HidEvent};
use crate::pin;
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

fn cursor_visible() -> bool {
    // SAFETY: argument-less query of the window server.
    unsafe { CGCursorIsVisible() != 0 }
}

/// Runs the calling (capture) thread at the priority of the UI. At the default
/// one, a busy app — scrolling a heavy page, say — kept it waiting, and every
/// event the tap holds with it: scrolling and motion stuttered in that app.
fn prioritise_input_thread() {
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
    }
    // SAFETY: changes only the calling thread's scheduling class.
    let err = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    if err != 0 {
        debug!(err, "could not raise the capture thread's priority");
    }
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

/// Turns off the pause (default 0.25 s) macOS inserts after a cursor warp, during
/// which it drops real mouse input — felt as stutter while another computer has
/// control. The call is deprecated but still honoured; resolved at run time.
fn disable_warp_suppression() {
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    type SetInterval = unsafe extern "C" fn(f64) -> i32;
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name is always safe to call.
        let p = unsafe { dlsym(RTLD_DEFAULT, c"CGSetLocalEventsSuppressionInterval".as_ptr()) };
        if let Some(p) = NonNull::new(p) {
            // SAFETY: the symbol has this C signature (CoreGraphics, since 10.0).
            unsafe {
                let set: SetInterval = std::mem::transmute(p.as_ptr());
                set(0.0);
            }
        }
    });
}

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

/// Accessibility as of *now*. `CGPreflightPostEventAccess` can keep answering
/// "no" for the rest of the process after the user grants access in System
/// Settings; `AXIsProcessTrusted` follows changes live.
fn accessibility_granted() -> bool {
    // SAFETY: argument-less query of the process's trust state.
    let trusted = unsafe { AXIsProcessTrusted() };
    trusted || CGPreflightPostEventAccess()
}

pub fn permissions() -> Permissions {
    Permissions { accessibility: accessibility_granted(), input_monitoring: CGPreflightListenEventAccess() }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    fn AXIsProcessTrusted() -> bool;
    /// Deprecated but working; returns `boolean_t`.
    fn CGCursorIsVisible() -> u32;
}

/// Shows macOS's "“Nexpingdesk” would like to control this computer" prompt and
/// adds the app to the Accessibility list. Call it from the app process (the one
/// macOS knows as Nexpingdesk), not from the background agent.
///
/// Accessibility first: an *active* event tap and posting events both fall
/// under it. Once it is there, Input Monitoring (optional) for taking external
/// mice exclusively; granting it makes macOS ask to quit the app once.
pub fn request_permissions() {
    if accessibility_granted() {
        // Optional: keeps an external mouse's gestures off this Mac while another
        // computer has control (see `macos_hid`). Asked once Accessibility is there.
        if !macos_hid::access_granted() {
            macos_hid::request_access();
        }
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

/// The event taps.
///
/// An active tap holds every event it asks for until its callback returned.
/// Asking for every move and scroll all the time put this thread in the way of
/// all mouse input on this Mac; so the mouse tap that can swallow only runs
/// while another computer has control, and a listen-only tap, which holds
/// nothing up, watches the mouse while this Mac has it (the screen edges).
struct Taps {
    /// The keyboard, at the HID level: always on (the paste hold, §14.2).
    keys: CFRetained<CFMachPort>,
    /// The mouse, at the HID level: on while another computer has control, or
    /// all the time without `listen`.
    mouse: CFRetained<CFMachPort>,
    /// The mouse, listen-only: on while this Mac has control. Needs Input
    /// Monitoring; missing without it.
    listen: Option<CFRetained<CFMachPort>>,
    /// After the HID taps: catches what other software posts itself (see
    /// `session_callback`), while another computer has control. Missing when
    /// macOS refused it.
    session: Option<CFRetained<CFMachPort>>,
}
// SAFETY: CGEventTapEnable and CFMachPortInvalidate may be called from any thread.
unsafe impl Send for Taps {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tap {
    Keys,
    Mouse,
    Listen,
    Session,
}

impl Taps {
    fn get(&self, which: Tap) -> Option<&CFRetained<CFMachPort>> {
        match which {
            Tap::Keys => Some(&self.keys),
            Tap::Mouse => Some(&self.mouse),
            Tap::Listen => self.listen.as_ref(),
            Tap::Session => self.session.as_ref(),
        }
    }

    /// Whether `which` belongs on, with control elsewhere (`grabbed`) or here.
    fn wanted(&self, which: Tap, grabbed: bool) -> bool {
        match which {
            Tap::Keys => true,
            Tap::Mouse => grabbed || self.listen.is_none(),
            Tap::Listen => !grabbed,
            Tap::Session => grabbed,
        }
    }

    /// Switches the taps for who has control: the ones taking over first, so a
    /// button release is never missed (seen twice meanwhile is harmless). Only
    /// taps in the wrong state are touched: switching one off makes macOS
    /// report it (see `tap_disabled`).
    fn arm(&self, grabbed: bool) {
        for on in [true, false] {
            for which in [Tap::Mouse, Tap::Listen, Tap::Session] {
                if let Some(t) = self.get(which)
                    && self.wanted(which, grabbed) == on
                    && CGEvent::tap_is_enabled(t) != on
                {
                    CGEvent::tap_enable(t, on);
                }
            }
        }
    }

    fn invalidate(&self) {
        for t in
            [Some(&self.keys), Some(&self.mouse), self.listen.as_ref(), self.session.as_ref()].into_iter().flatten()
        {
            remove_tap(t);
        }
    }
}

fn remove_tap(t: &CFMachPort) {
    CGEvent::tap_enable(t, false);
    t.invalidate();
}

struct Shared {
    grabbed: AtomicBool,
    stopped: AtomicBool,
    tx: mpsc::Sender<CaptureEvent>,
    run_loop: OnceLock<SendRunLoop>,
    /// `None` until the capture thread created them and again once it tore them
    /// down: `front_taps` must not create taps after that, their callbacks would
    /// outlive `Shared`.
    taps: Mutex<Option<Taps>>,
    /// Where the hidden cursor is kept while grabbed, and how far it may drift.
    pin: Mutex<(Point, i32)>,
    /// External mice, taken exclusively while grabbed (see `macos_hid`).
    seizer: Mutex<Option<macos_hid::Seizer>>,
    /// Motion across our warps (see `crate::pin`). Its lock also keeps the
    /// capture thread from pulling the cursor back while control changes hands.
    motion: Mutex<pin::Tracker>,
    /// Device-dependent modifier bits last seen (to derive up/down).
    last_flags: Mutex<u64>,
    gate: crate::gate::KeyGate,
    /// Keeps the cursor hidden while grabbed (`watch_cursor`); woken on a grab.
    watcher: OnceLock<std::thread::Thread>,
}

impl Shared {
    /// A seized external mouse did something (only while grabbed).
    fn on_hid(&self, ev: HidEvent) {
        if !self.grabbed.load(Ordering::SeqCst) {
            return;
        }
        self.emit(match ev {
            HidEvent::Motion { dx, dy } => CaptureEvent::Motion { pos: lock(&self.pin).0, dx, dy },
            HidEvent::Button { button, down } => CaptureEvent::Button { button, down },
            HidEvent::Wheel { dx, dy } => CaptureEvent::Wheel { dx, dy },
            HidEvent::Gesture(g) => CaptureEvent::Gesture(g),
        });
    }

    /// Hides the cursor again if something showed it while another computer
    /// has control. Hidden once at hand-over, it came back for good whenever
    /// mouse software or an app switch showed it (e.g. on an extra button),
    /// sitting still in the middle of this screen while the hand was elsewhere.
    fn keep_cursor_hidden(&self) {
        if !self.grabbed.load(Ordering::SeqCst) || !cursor_visible() {
            return;
        }
        // Under `motion`, like `set_grab`: never hidden after control came back.
        let _motion = lock(&self.motion);
        if self.grabbed.load(Ordering::SeqCst) {
            // Show + hide: our hide count stays 1 (a hide alone would not act at
            // a count already above 0, and one show would no longer undo it).
            let display = CGMainDisplayID();
            let _ = CGDisplayShowCursor(display);
            let _ = CGDisplayHideCursor(display);
        }
    }

    fn emit(&self, ev: CaptureEvent) {
        // Never block the system input path; drop on overflow.
        if self.tx.try_send(ev).is_err() {
            debug!("capture queue full; event dropped");
        }
    }

    /// Recreates the active taps in front of every other tap at their location.
    ///
    /// Taps run newest first. Mouse software (Logi Options+, `SteerMouse`,
    /// `BetterTouchTool`, …) started after Nexpingdesk — at login, or restarted
    /// by its updater — would see an external mouse's extra buttons before our
    /// tap swallows them and act on them here as well: Back/Forward went back
    /// on both computers. In front again, ours swallows them first while another
    /// computer has control, so that software never sees them. The new taps
    /// are armed for a grab, as `set_grab` wants them before one.
    fn front_taps(self: &Arc<Self>) {
        let Some(rl) = self.run_loop.get() else { return };
        let mut taps = lock(&self.taps);
        let Some(mut old) = taps.take() else { return };
        // Old off first: no event may pass through both (it would be sent twice).
        CGEvent::tap_enable(&old.keys, false);
        CGEvent::tap_enable(&old.mouse, false);
        let user = Arc::as_ptr(self).cast_mut().cast::<c_void>();
        let hid = |events, callback| new_tap(CGEventTapLocation::HIDEventTap, events, callback, user, &rl.0);
        let (keys, mouse) = match (hid(key_events(), Some(keys_callback)), hid(mouse_events(), Some(mouse_callback))) {
            (Some(keys), Some(mouse)) => (keys, mouse),
            (keys, mouse) => {
                warn!("could not move the event taps in front of other software");
                for t in keys.iter().chain(&mouse) {
                    remove_tap(t);
                }
                CGEvent::tap_enable(&old.keys, true);
                CGEvent::tap_enable(&old.mouse, true);
                old.arm(true);
                *taps = Some(old);
                return;
            }
        };
        if let Some(t) = &old.session {
            CGEvent::tap_enable(t, false);
        }
        let session = new_tap(
            CGEventTapLocation::AnnotatedSessionEventTap,
            session_events(),
            Some(session_callback),
            user,
            &rl.0,
        );
        let new = Taps { keys, mouse, listen: old.listen.take(), session };
        new.arm(true);
        old.invalidate();
        *taps = Some(new);
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
        // The taps that swallow only run while another computer has control
        // (see `Taps`, `GRAB_ONLY`). They are on before `grabbed` says so and
        // off only after: they pass everything while not grabbed, so no event
        // in between can slip through to this Mac.
        if grab && !self.0.grabbed.load(Ordering::SeqCst) {
            self.0.front_taps();
        }
        // Held until the cursor is where it belongs: the capture thread can't pull
        // it back to the pin after control returned here (it checks `grabbed`
        // under this lock), which left it stuck in the middle, cut off from the mouse.
        let mut motion = lock(&self.0.motion);
        let was = self.0.grabbed.swap(grab, Ordering::SeqCst);
        if was == grab {
            return;
        }
        if let Some(t) = lock(&self.0.taps).as_ref() {
            t.arm(grab);
        }
        if grab {
            self.0.gate.on_grab();
            // Park the hidden cursor in the middle of the main screen. macOS keeps
            // moving the real cursor under an event tap (and "disconnect mouse from
            // cursor" only applies to the foreground app), so it is pulled back
            // here as soon as it drifts a little (see `crate::pin`). Parked at the edge it used to
            // hit the screen border, stop producing motion, and the pointer on the
            // other computer got stuck before it could come back (§14 B5/B7).
            let centre = to_rect(CGDisplayBounds(CGMainDisplayID()));
            let pin = Point::new(centre.x + centre.w / 2, centre.y + centre.h / 2);
            *lock(&self.0.pin) = (pin, pin::radius_for(centre));
            disable_warp_suppression();
            allow_background_cursor_hiding();
            let from = cursor_pos().unwrap_or(pin);
            repin(pin);
            motion.warped(from, pin);
            let _ = CGDisplayHideCursor(CGMainDisplayID());
            if let Some(w) = self.0.watcher.get() {
                w.unpark();
            }
            // Mice are seized and released without `motion` held: opening and
            // closing a Bluetooth mouse is slow, and the HID tap needs that lock
            // for every move. Blocked right while the hand crossed the edge, it
            // timed out, macOS switched it off, and the Mac got every event: its
            // cursor moved along and extra buttons acted here too.
            drop(motion);
            if let Some(s) = lock(&self.0.seizer).as_mut() {
                s.seize();
            }
        } else {
            let _ = CGAssociateMouseAndMouseCursorPosition(true);
            let _ = CGDisplayShowCursor(CGMainDisplayID());
            drop(motion);
            if let Some(s) = lock(&self.0.seizer).as_mut() {
                s.release();
            }
        }
    }

    fn warp(&self, p: Point) {
        let mut motion = lock(&self.0.motion);
        let from = cursor_pos().unwrap_or(p);
        let _ = CGWarpMouseCursorPosition(cg(p));
        motion.warped(from, p);
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
        if let Some(w) = self.0.watcher.get() {
            w.unpark();
        }
        if let Some(rl) = self.0.run_loop.get() {
            rl.0.stop();
        }
    }
}

/// Event types (`NSEventType` numbers) only swallowed while grabbed, never forwarded:
/// tablet pointer/proximity, rotate/begin/end gesture, gesture, magnify, swipe,
/// smart magnify, quick look, pressure, direct touch, change mode. Only the
/// session tap asks for them, and only while grabbed: a tap that sees gesture
/// events, even one that lets them all through, turns this Mac's mouse and
/// trackpad gestures off (swipe between spaces, Mission Control, smart zoom).
const GRAB_ONLY: [u32; 13] = [23, 24, 18, 19, 20, 29, 30, 31, 32, 33, 34, 37, 38];

fn mask(types: &[CGEventType]) -> u64 {
    types.iter().fold(0u64, |m, t| m | (1u64 << t.0))
}

/// What the HID taps ask for: never gesture or system-defined events (see
/// `GRAB_ONLY`); the session tap, running only while grabbed, does.
fn key_events() -> u64 {
    mask(&[CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged])
}

/// While this Mac has control only motion (the screen edges) and buttons (held
/// ones keep the cursor here) matter; not scrolling.
fn listen_events() -> u64 {
    mask(&[
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
    ])
}

fn mouse_events() -> u64 {
    listen_events() | mask(&[CGEventType::ScrollWheel])
}

fn session_events() -> u64 {
    key_events() | mouse_events() | (1u64 << SYSTEM_DEFINED) | GRAB_ONLY.iter().fold(0u64, |m, t| m | (1u64 << t))
}

/// An active tap at the head of `location`, in front of every tap already
/// there, serviced by `rl`. `user` must stay valid for as long as the tap exists.
fn new_tap(
    location: CGEventTapLocation,
    events: u64,
    callback: CGEventTapCallBack,
    user: *mut c_void,
    rl: &CFRunLoop,
) -> Option<CFRetained<CFMachPort>> {
    create_tap(location, CGEventTapOptions::Default, events, callback, user, rl)
}

/// A tap as [`new_tap`], active or listen-only.
fn create_tap(
    location: CGEventTapLocation,
    options: CGEventTapOptions,
    events: u64,
    callback: CGEventTapCallBack,
    user: *mut c_void,
    rl: &CFRunLoop,
) -> Option<CFRetained<CFMachPort>> {
    // SAFETY: every callback matches CGEventTapCallBack; the caller keeps `user` valid.
    let tap = unsafe {
        CGEvent::tap_create(location, CGEventTapPlacement::HeadInsertEventTap, options, events, callback, user)
    }?;
    // Only a tap that is serviced by this run loop may stay: an unserviced
    // active tap would hold up input.
    let Some(src) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
        tap.invalidate();
        return None;
    };
    // SAFETY: kCFRunLoopCommonModes is a valid static CF string.
    rl.add_source(Some(&src), unsafe { kCFRunLoopCommonModes });
    // Added from another thread by `front_taps`: have the loop pick it up now.
    rl.wake_up();
    CGEvent::tap_enable(&tap, true);
    Some(tap)
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

/// `NSEventTypeSystemDefined`: media / special keys (volume, play, brightness,
/// …) and the actions mouse utilities give extra buttons. Never key events.
const SYSTEM_DEFINED: u32 = 14;
/// Its subtype for media / special keys (`NX_SUBTYPE_AUX_CONTROL_BUTTONS`).
const AUX_CONTROL_BUTTONS: i16 = 8;

/// Put (as `EventSourceUserData`) on events the HID tap lets through while
/// grabbed (key releases of keys held before the grab), so the session tap lets
/// them through as well. Other input tools tag their own events in the same
/// field, so the value is random per run rather than a constant they could share.
fn passed_mark() -> i64 {
    static MARK: OnceLock<i64> = OnceLock::new();
    *MARK.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0));
        let mixed = (nanos ^ (u64::from(std::process::id()) << 32)).rotate_left(17) | 1;
        i64::from_ne_bytes(mixed.to_ne_bytes())
    })
}

/// The HID keyboard tap: every real key event, before anything else.
unsafe extern "C-unwind" fn keys_callback(
    _proxy: CGEventTapProxy,
    ty: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user` is the `Arc<Shared>` pointer kept alive by the capture
    // thread for as long as the taps exist.
    let shared = unsafe { &*(user as *const Shared) };
    // SAFETY: the callback's own event.
    unsafe { hid_callback(shared, Tap::Keys, ty, event) }
}

/// The HID mouse tap that swallows: every real mouse event while grabbed.
unsafe extern "C-unwind" fn mouse_callback(
    _proxy: CGEventTapProxy,
    ty: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: as in `keys_callback`.
    let shared = unsafe { &*(user as *const Shared) };
    // SAFETY: the callback's own event.
    unsafe { hid_callback(shared, Tap::Mouse, ty, event) }
}

/// The listen-only mouse tap, while this Mac has control: nothing waits for it.
unsafe extern "C-unwind" fn listen_callback(
    _proxy: CGEventTapProxy,
    ty: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: as in `keys_callback`.
    let shared = unsafe { &*(user as *const Shared) };
    // SAFETY: the event pointer is valid for the duration of the callback.
    let ev = unsafe { event.as_ref() };
    let pass = event.as_ptr();
    // Grabbed, the mouse tap has it (this one only overlaps it at hand-over).
    if !tap_disabled(shared, ty, Tap::Listen) && !shared.grabbed.load(Ordering::Relaxed) {
        let _ = handle(shared, ty, ev, pass);
    }
    pass
}

/// # Safety
/// `event` is the event a tap callback was given, valid while it runs.
unsafe fn hid_callback(shared: &Shared, which: Tap, ty: CGEventType, event: NonNull<CGEvent>) -> *mut CGEvent {
    // SAFETY: see above.
    let ev = unsafe { event.as_ref() };
    let pass = event.as_ptr();
    if tap_disabled(shared, ty, which) {
        return pass;
    }
    let out = handle(shared, ty, ev, pass);
    if !out.is_null() && shared.grabbed.load(Ordering::Relaxed) {
        CGEvent::set_integer_value_field(Some(ev), CGEventField::EventSourceUserData, passed_mark());
    }
    out
}

/// The session tap. Mouse utilities (Logi Options+, `SteerMouse`, `BetterTouchTool`,
/// …) read extra buttons themselves and post what they are set to — a shortcut
/// like ⌘C, a scroll, a click — past the HID tap. While grabbed, the HID tap
/// already swallowed every real event, so anything else arriving here was posted
/// by software: it belongs to the client too, not to this Mac.
unsafe extern "C-unwind" fn session_callback(
    _proxy: CGEventTapProxy,
    ty: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: as in `keys_callback`.
    let shared = unsafe { &*(user as *const Shared) };
    // SAFETY: the event pointer is valid for the duration of the callback.
    let ev = unsafe { event.as_ref() };
    let pass = event.as_ptr();
    if tap_disabled(shared, ty, Tap::Session) || !shared.grabbed.load(Ordering::Relaxed) {
        return pass;
    }
    if CGEvent::integer_value_field(Some(ev), CGEventField::EventSourceUserData) == passed_mark() {
        return pass;
    }
    // Motion posted by software (a keep-awake jiggler, a remote-control or
    // gesture tool) is not the hand's: it must not move the client's cursor,
    // which then moved on its own. Swallowed like everything else here.
    if matches!(
        ty,
        CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged
    ) {
        return std::ptr::null_mut();
    }
    handle(shared, ty, ev, pass)
}

/// macOS turns a tap off when it is too slow or on user input: turn it back on.
///
/// Each tap is told about its own disabling, including the times Nexpingdesk
/// switched it off itself: the session tap is off while this Mac has control
/// (so is the mouse tap, with a listen-only one), and switching it off makes
/// macOS report that too. Only a tap that should be
/// on and is off is switched back on, and only that is an interruption. Setting
/// both taps on every report made macOS report again, forever: the capture
/// thread spent most of its time switching taps (a core busy, the Mac hot), and
/// every round told the client to release everything.
///
/// A timeout report is proof enough that the tap is off: it is always switched
/// back on (a no-op if it already is, so it can't loop). Trusting
/// `CGEventTapIsEnabled` there left the HID tap off while a client had control.
fn tap_disabled(shared: &Shared, ty: CGEventType, which: Tap) -> bool {
    let timeout = ty == CGEventType::TapDisabledByTimeout;
    if !timeout && ty != CGEventType::TapDisabledByUserInput {
        return false;
    }
    if shared.stopped.load(Ordering::SeqCst) {
        return true;
    }
    let grabbed = shared.grabbed.load(Ordering::SeqCst);
    let reenabled = lock(&shared.taps).as_ref().is_some_and(|taps| {
        let tap = taps.get(which).filter(|_| taps.wanted(which, grabbed));
        tap.is_some_and(|t| {
            let off = timeout || !CGEvent::tap_is_enabled(t);
            if off {
                CGEvent::tap_enable(t, true);
            }
            off
        })
    });
    if reenabled {
        shared.emit(CaptureEvent::Interrupted);
    }
    true
}

/// Media / special key of a system-defined event, if it is one.
fn aux_key(ev: &CGEvent) -> Option<(KeyCode, bool)> {
    objc2::rc::autoreleasepool(|_| {
        let ns = objc2_app_kit::NSEvent::eventWithCGEvent(ev)?;
        if ns.subtype().0 != AUX_CONTROL_BUTTONS {
            return None;
        }
        keymap::mac_aux_key(i64::try_from(ns.data1()).ok()?)
    })
}

/// Turns one local event into capture events; returns `pass` to let it through
/// to this Mac, or null to swallow it (everything, while grabbed).
#[allow(clippy::cast_possible_truncation)]
fn handle(shared: &Shared, ty: CGEventType, ev: &CGEvent, pass: *mut CGEvent) -> *mut CGEvent {
    let field = |f: CGEventField| CGEvent::integer_value_field(Some(ev), f);
    match ty {
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => {
            let loc = CGEvent::location(Some(ev));
            let pos = Point::new(loc.x.floor() as i32, loc.y.floor() as i32);
            // The integer delta fields. (The double accessor of the same fields
            // reports much larger numbers for real trackpad events.)
            let raw = (field(CGEventField::MouseEventDeltaX) as i32, field(CGEventField::MouseEventDeltaY) as i32);
            // Lock order: `motion`, then `pin` (as in `set_grab`).
            let mut motion = lock(&shared.motion);
            let (dx, dy) = motion.motion(raw);
            let pos = if shared.grabbed.load(Ordering::SeqCst) {
                // Keep the hidden cursor near the pin, judged by where it really is
                // (the event's location can lag a warp). Left to roam, it follows the
                // hand all over this screen. One warp at a time (see `crate::pin`).
                let (at, radius) = *lock(&shared.pin);
                if motion.settled()
                    && let Some(real) = cursor_pos()
                    && pin::needs_repin(real, at, radius)
                {
                    repin(at);
                    motion.warped(real, at);
                }
                at
            } else {
                pos
            };
            drop(motion);
            shared.emit(CaptureEvent::Motion { pos, dx, dy });
        }
        CGEventType::LeftMouseDown
        | CGEventType::LeftMouseUp
        | CGEventType::RightMouseDown
        | CGEventType::RightMouseUp
        | CGEventType::OtherMouseDown
        | CGEventType::OtherMouseUp => {
            let down =
                matches!(ty, CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown);
            let number = match ty {
                CGEventType::LeftMouseDown | CGEventType::LeftMouseUp => 0,
                CGEventType::RightMouseDown | CGEventType::RightMouseUp => 1,
                _ => field(CGEventField::MouseEventButtonNumber),
            };
            let grabbed = shared.grabbed.load(Ordering::Relaxed);
            if shared.gate.on_button(u32::try_from(number).unwrap_or(u32::MAX), down, grabbed) {
                return pass;
            }
            // Buttons 6 and up have no counterpart on the other computer: swallowed
            // while grabbed, but not sent as a middle click (as they used to be).
            let button = match number {
                0 => Some(MouseButton::Left),
                1 => Some(MouseButton::Right),
                2 => Some(MouseButton::Middle),
                3 => Some(MouseButton::Back),
                4 => Some(MouseButton::Forward),
                _ => None,
            };
            if let Some(button) = button {
                shared.emit(CaptureEvent::Button { button, down });
            }
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
            // Sent as seen here — natural scrolling already applied — so other
            // computers scroll the way this one does (protocol 3). Only while
            // grabbed: scrolling here is none of the server's business.
            if shared.grabbed.load(Ordering::Relaxed) && (dx != 0.0 || dy != 0.0) {
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
        // Media / special keys go to the computer that has control.
        t if t.0 == SYSTEM_DEFINED => {
            if shared.grabbed.load(Ordering::Relaxed)
                && let Some((key, down)) = aux_key(ev)
            {
                shared.emit(CaptureEvent::Key { key, down });
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
    let (shared, events) = start_taps()?;
    Ok(Capture { control: Arc::new(Control(shared)), events })
}

fn start_taps() -> Result<(Arc<Shared>, mpsc::Receiver<CaptureEvent>), InputError> {
    // An active (filtering) tap needs Accessibility only; see `request_permissions`.
    // No pre-check: creating the tap is the real test (a cached "no" must not
    // block a permission the user has just granted). It fails below without it.
    let (tx, rx) = mpsc::channel(CAPTURE_QUEUE);
    let shared = Arc::new(Shared {
        grabbed: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        tx,
        run_loop: OnceLock::new(),
        taps: Mutex::new(None),
        pin: Mutex::new((Point::default(), pin::PIN_RADIUS)),
        seizer: Mutex::new(None),
        motion: Mutex::new(pin::Tracker::default()),
        last_flags: Mutex::new(0),
        gate: crate::gate::KeyGate::default(),
        watcher: OnceLock::new(),
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), InputError>>();
    let thread_shared = shared.clone();
    std::thread::Builder::new()
        .name("nd-capture".into())
        .spawn(move || {
            prioritise_input_thread();
            let user = Arc::into_raw(thread_shared.clone()) as *mut c_void;
            let Some(rl) = CFRunLoop::current() else {
                // SAFETY: reclaim the leaked Arc; no tap was created.
                drop(unsafe { Arc::from_raw(user as *const Shared) });
                let _ = ready_tx.send(Err(InputError::Os("no run loop".into())));
                return;
            };
            let hid = |events, callback| new_tap(CGEventTapLocation::HIDEventTap, events, callback, user, &rl);
            let (keys, mouse) =
                match (hid(key_events(), Some(keys_callback)), hid(mouse_events(), Some(mouse_callback))) {
                    (Some(keys), Some(mouse)) => (keys, mouse),
                    (keys, mouse) => {
                        for t in keys.iter().chain(&mouse) {
                            remove_tap(t);
                        }
                        // SAFETY: as above; the taps created are gone.
                        drop(unsafe { Arc::from_raw(user as *const Shared) });
                        let _ = ready_tx.send(Err(InputError::Permission("Accessibility")));
                        return;
                    }
                };
            // A listen-only tap falls under Input Monitoring: without it macOS
            // would ask from this background process (see `request_permissions`).
            let listen = CGPreflightListenEventAccess()
                .then(|| {
                    create_tap(
                        CGEventTapLocation::HIDEventTap,
                        CGEventTapOptions::ListenOnly,
                        listen_events(),
                        Some(listen_callback),
                        user,
                        &rl,
                    )
                })
                .flatten();
            let session = new_tap(
                CGEventTapLocation::AnnotatedSessionEventTap,
                session_events(),
                Some(session_callback),
                user,
                &rl,
            );
            if session.is_none() {
                warn!(
                    "session event tap unavailable: buttons that mouse software turns into shortcuts stay on this Mac"
                );
            }
            let taps = Taps { keys, mouse, listen, session };
            // Session (and mouse) tap off until another computer has control (`set_grab`).
            taps.arm(thread_shared.grabbed.load(Ordering::SeqCst));
            *lock(&thread_shared.taps) = Some(taps);
            // Weak: the seizer's callbacks must not keep `Shared` alive.
            let weak = Arc::downgrade(&thread_shared);
            let sink = Box::new(move |e| {
                if let Some(s) = weak.upgrade() {
                    s.on_hid(e);
                }
            });
            *lock(&thread_shared.seizer) = Some(macos_hid::Seizer::new(rl.clone(), sink));
            let _ = thread_shared.run_loop.set(SendRunLoop(rl));
            let _ = ready_tx.send(Ok(()));
            while !thread_shared.stopped.load(Ordering::SeqCst) {
                CFRunLoop::run();
            }
            // Mice back to the system before the run loop they report on goes.
            drop(lock(&thread_shared.seizer).take());
            // `front_taps` may have replaced them: tear down the current ones.
            if let Some(taps) = lock(&thread_shared.taps).take() {
                taps.invalidate();
            }
            // SAFETY: every tap is disabled and invalidated, and none can be
            // created any more (`taps` is `None`); no more callbacks.
            drop(unsafe { Arc::from_raw(user as *const Shared) });
            debug!("capture thread stopped");
        })
        .map_err(|e| InputError::Os(e.to_string()))?;
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| InputError::Os("capture thread did not start".into()))??;
    let weak = Arc::downgrade(&shared);
    match std::thread::Builder::new().name("nd-cursor".into()).spawn(move || watch_cursor(&weak)) {
        Ok(h) => {
            let _ = shared.watcher.set(h.thread().clone());
        }
        Err(e) => warn!(error = %e, "cursor watcher not started: the hidden cursor may show up again"),
    }
    Ok((shared, rx))
}

/// How often the hidden cursor is checked while another computer has control.
const CURSOR_CHECK: Duration = Duration::from_millis(50);

/// Keeps the cursor hidden while grabbed; sleeps otherwise (woken on a grab).
fn watch_cursor(shared: &std::sync::Weak<Shared>) {
    loop {
        let Some(s) = shared.upgrade() else { return };
        if s.stopped.load(Ordering::SeqCst) {
            return;
        }
        let grabbed = s.grabbed.load(Ordering::SeqCst);
        if grabbed {
            s.keep_cursor_hidden();
        }
        drop(s);
        if grabbed {
            std::thread::sleep(CURSOR_CHECK);
        } else {
            std::thread::park_timeout(Duration::from_secs(5));
        }
    }
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
    if !accessibility_granted() {
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
        // Already in the server's scrolling direction; posted events skip this
        // Mac's natural-scrolling inversion, so it scrolls like the server.
        let fx = f64::from(dx) / UNITS_PER_PIXEL + self.wheel_rem.0;
        let fy = f64::from(dy) / UNITS_PER_PIXEL + self.wheel_rem.1;
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
///
/// The front app is taken from the window list (ordered front to back), not
/// `NSWorkspace`: in a background process without a run loop the workspace's
/// "frontmost application" is never refreshed and could block switching forever.
pub fn fullscreen_app() -> Option<String> {
    use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
    use objc2_core_graphics::{
        CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowBounds,
        kCGWindowLayer, kCGWindowOwnerName, kCGWindowOwnerPID,
    };

    let displays: Vec<Rect> = display_ids().into_iter().map(|id| to_rect(CGDisplayBounds(id))).collect();
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )?;
    // SAFETY: CGWindowList returns a CFArray of CFDictionary; the constant keys are CFStrings.
    unsafe {
        let value = |d: &CFDictionary, key: &CFString| -> Option<&CFType> {
            let v = d.value((&raw const *key).cast());
            (!v.is_null()).then(|| &*v.cast::<CFType>())
        };
        let num = |d: &CFDictionary, key: &CFString| value(d, key)?.downcast_ref::<CFNumber>()?.as_i64();
        let bounds = |d: &CFDictionary| {
            let b = d.value((&raw const *kCGWindowBounds).cast());
            if b.is_null() {
                return None;
            }
            let mut r = objc2_core_foundation::CGRect::default();
            CGRectMakeWithDictionaryRepresentation(Some(&*b.cast::<CFDictionary>()), &raw mut r).then(|| to_rect(r))
        };
        let dicts: Vec<&CFDictionary> = (0..list.count())
            .filter_map(|i| {
                let d = list.value_at_index(i).cast::<CFDictionary>();
                (!d.is_null()).then(|| &*d)
            })
            .collect();
        // Front-most normal window = front-most app.
        let front = dicts.iter().find(|d| num(d, kCGWindowLayer) == Some(0))?;
        let pid = num(front, kCGWindowOwnerPID)?;
        let name = value(front, kCGWindowOwnerName)
            .and_then(|v| v.downcast_ref::<CFString>())
            .map(|n| n.to_string().to_lowercase())
            .unwrap_or_default();
        if name == "nexpingdesk" {
            return None;
        }
        let covers = dicts
            .iter()
            .filter(|d| num(d, kCGWindowOwnerPID) == Some(pid) && num(d, kCGWindowLayer) == Some(0))
            .any(|d| {
                bounds(d).is_some_and(|w| {
                    displays
                        .iter()
                        .any(|d| w.x <= d.x && w.y <= d.y && w.right() >= d.right() && w.bottom() >= d.bottom())
                })
            });
        covers.then_some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real event tap takes over this Mac's keyboard and cursor while grabbed:
    /// only where asked for (the macOS CI runner sets `NEXPINGDESK_TAP_TESTS`).
    fn enabled() -> bool {
        std::env::var_os("NEXPINGDESK_TAP_TESTS").is_some()
    }

    /// One test, run in order: the checks post real events, and a grabbed tap
    /// of a check running in parallel (tests are separate processes) would
    /// swallow another check's events.
    #[test]
    fn real_event_taps() {
        if !enabled() {
            return;
        }
        the_session_tap_only_runs_while_a_client_has_control();
        an_idle_mac_does_not_keep_switching_taps();
        shortcuts_posted_by_mouse_software_go_to_the_client_while_grabbed();
        media_keys_go_to_the_client_while_grabbed();
        clicks_and_motion_go_to_the_client_once_and_the_mac_cursor_stays();
        extra_buttons_never_reach_mouse_software_started_later();
        motion_posted_by_software_does_not_move_the_client();
        the_mouse_is_only_held_up_while_a_client_has_control();
        a_cursor_shown_again_is_hidden_while_a_client_has_control();
    }

    fn tap_on(shared: &Shared, which: Tap) -> Option<bool> {
        lock(&shared.taps).as_ref().expect("taps").get(which).map(|t| CGEvent::tap_is_enabled(t))
    }

    /// While this Mac has control the mouse is only watched, by a listen-only
    /// tap that holds nothing up (given Input Monitoring); the tap that
    /// swallows runs only while a client has control. Local motion is seen
    /// exactly once either way (the screen edges need it).
    fn the_mouse_is_only_held_up_while_a_client_has_control() {
        let (shared, mut events) = start_taps().expect("event taps (Accessibility)");
        let control = Control(shared.clone());
        let listening = tap_on(&shared, Tap::Listen).is_some();
        assert_eq!(tap_on(&shared, Tap::Mouse), Some(!listening), "mouse tap state with this Mac in control");
        control.set_grab(true);
        assert_eq!(tap_on(&shared, Tap::Mouse), Some(true), "a client has control: the mouse must be swallowed");
        assert_ne!(tap_on(&shared, Tap::Listen), Some(true), "listen-only tap on while grabbed");
        control.set_grab(false);
        assert_eq!(tap_on(&shared, Tap::Mouse), Some(!listening), "mouse tap state after control came back");
        assert_ne!(tap_on(&shared, Tap::Listen), Some(false), "listen-only tap off after control came back");
        std::thread::sleep(Duration::from_millis(100));
        let _ = drain(&mut events, 100);
        let at = cursor_pos().expect("cursor");
        post_mouse(CGEventTapLocation::HIDEventTap, CGEventType::MouseMoved, at, CGMouseButton::Left, (0, 0));
        let seen = drain(&mut events, 400).into_iter().filter(|e| matches!(e, CaptureEvent::Motion { .. })).count();
        control.stop();
        assert_eq!(seen, 1, "local motion reached the server {seen} times (listen-only tap: {listening})");
    }

    /// Something shows the cursor while a client has control (mouse software,
    /// an app switch; here a show of our own): it is hidden again, and shown
    /// once control is back.
    fn a_cursor_shown_again_is_hidden_while_a_client_has_control() {
        let cap = start_capture().expect("event tap (Accessibility)");
        cap.control.set_grab(true);
        std::thread::sleep(Duration::from_millis(200));
        let hidden_at_grab = !cursor_visible();
        let _ = CGDisplayShowCursor(CGMainDisplayID());
        std::thread::sleep(Duration::from_millis(300));
        let hidden_again = !cursor_visible();
        cap.control.set_grab(false);
        let shown_after = cursor_visible();
        cap.control.stop();
        if !hidden_at_grab {
            // No hiding from the background on this system: nothing to keep.
            return;
        }
        assert!(hidden_again, "the cursor stayed visible on this Mac while a client had control");
        assert!(shown_after, "the cursor stayed hidden after control came back");
    }

    fn drain(rx: &mut mpsc::Receiver<CaptureEvent>, for_ms: u64) -> Vec<CaptureEvent> {
        let until = Instant::now() + Duration::from_millis(for_ms);
        let mut out = Vec::new();
        while Instant::now() < until {
            match rx.try_recv() {
                Ok(e) => out.push(e),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        out
    }

    fn post_mouse(at: CGEventTapLocation, ty: CGEventType, p: Point, button: CGMouseButton, delta: (i64, i64)) {
        let ev = CGEvent::new_mouse_event(None, ty, cg(p), button).expect("mouse event");
        CGEvent::set_integer_value_field(Some(&ev), CGEventField::MouseEventDeltaX, delta.0);
        CGEvent::set_integer_value_field(Some(&ev), CGEventField::MouseEventDeltaY, delta.1);
        CGEvent::post(at, Some(&ev));
    }

    /// An external mouse while a client has control: its clicks arrive at the
    /// hardware level, or re-posted by mouse software at the session level. Each
    /// must reach the client exactly once, down before up; its motion must arrive
    /// in full; and this Mac's cursor must stay put near the pin.
    fn clicks_and_motion_go_to_the_client_once_and_the_mac_cursor_stays() {
        let mut cap = start_capture().expect("event tap (Accessibility)");
        cap.control.set_grab(true);
        std::thread::sleep(Duration::from_millis(200));
        let start = cursor_pos().expect("cursor");
        let _ = drain(&mut cap.events, 100);

        for at in [CGEventTapLocation::HIDEventTap, CGEventTapLocation::SessionEventTap] {
            post_mouse(at, CGEventType::LeftMouseDown, start, CGMouseButton::Left, (0, 0));
            post_mouse(at, CGEventType::LeftMouseUp, start, CGMouseButton::Left, (0, 0));
            let clicks: Vec<bool> = drain(&mut cap.events, 500)
                .into_iter()
                .filter_map(|e| match e {
                    CaptureEvent::Button { button: MouseButton::Left, down } => Some(down),
                    _ => None,
                })
                .collect();
            assert_eq!(clicks, vec![true, false], "a click posted at {at:?} reached the client {clicks:?}");
        }

        let mut p = start;
        for _ in 0..40 {
            p = Point::new(p.x + 25, p.y + 5);
            post_mouse(CGEventTapLocation::HIDEventTap, CGEventType::MouseMoved, p, CGMouseButton::Left, (25, 5));
            std::thread::sleep(Duration::from_millis(2));
        }
        // A posted event puts the cursor where it says, pulled back or not (a
        // hand's motion is relative): whether it ends near the pin would depend
        // on which event the last pull-back fell on. Motionless events where the
        // cursor is let the capture settle and pull it back, as for a real mouse.
        for _ in 0..10 {
            let here = cursor_pos().expect("cursor");
            post_mouse(CGEventTapLocation::HIDEventTap, CGEventType::MouseMoved, here, CGMouseButton::Left, (0, 0));
            std::thread::sleep(Duration::from_millis(5));
        }
        let (dx, dy) = drain(&mut cap.events, 500).into_iter().fold((0, 0), |(x, y), e| match e {
            CaptureEvent::Motion { dx, dy, .. } => (x + dx, y + dy),
            _ => (x, y),
        });
        let now = cursor_pos().expect("cursor");
        cap.control.set_grab(false);
        cap.control.stop();
        assert_eq!((dx, dy), (1000, 200), "the client got a different motion than the mouse made");
        let radius = pin::radius_for(to_rect(CGDisplayBounds(CGMainDisplayID())));
        assert!(!pin::needs_repin(now, start, radius), "this Mac's cursor moved away: {start:?} -> {now:?}");
    }

    /// Motion posted at the session level (by software, past the HID tap) while
    /// a client has control: swallowed, and never sent to the client.
    fn motion_posted_by_software_does_not_move_the_client() {
        let mut cap = start_capture().expect("event tap (Accessibility)");
        cap.control.set_grab(true);
        std::thread::sleep(Duration::from_millis(200));
        let start = cursor_pos().expect("cursor");
        let _ = drain(&mut cap.events, 100);
        let mut p = start;
        for _ in 0..20 {
            p = Point::new(p.x + 3, p.y);
            post_mouse(CGEventTapLocation::SessionEventTap, CGEventType::MouseMoved, p, CGMouseButton::Left, (3, 0));
            std::thread::sleep(Duration::from_millis(5));
        }
        let moved = drain(&mut cap.events, 400).into_iter().any(|e| matches!(e, CaptureEvent::Motion { .. }));
        cap.control.set_grab(false);
        cap.control.stop();
        assert!(!moved, "motion posted by software was sent to the client");
    }

    /// Back presses (button 3) seen by the stand-in for mouse software below.
    static OTHER_SOFTWARE_SAW: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// A tap like Logi Options+ installs: it acts on extra buttons and lets them through.
    unsafe extern "C-unwind" fn other_software(
        _proxy: CGEventTapProxy,
        ty: CGEventType,
        event: NonNull<CGEvent>,
        _user: *mut c_void,
    ) -> *mut CGEvent {
        // SAFETY: the event pointer is valid for the duration of the callback.
        let ev = unsafe { event.as_ref() };
        if ty == CGEventType::OtherMouseDown
            && CGEvent::integer_value_field(Some(ev), CGEventField::MouseEventButtonNumber) == 3
        {
            OTHER_SOFTWARE_SAW.fetch_add(1, Ordering::SeqCst);
        }
        event.as_ptr()
    }

    /// An external mouse's Back button while a client has control, with mouse
    /// software whose tap was created after ours (so in front of it): the
    /// press must reach the client once and never that software, or it acts on
    /// this Mac as well.
    fn extra_buttons_never_reach_mouse_software_started_later() {
        let mut cap = start_capture().expect("event tap (Accessibility)");
        let (rl_tx, rl_rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let other = std::thread::spawn(move || {
            let rl = CFRunLoop::current().expect("run loop");
            let events = mask(&[CGEventType::OtherMouseDown, CGEventType::OtherMouseUp]);
            let tap = new_tap(CGEventTapLocation::HIDEventTap, events, Some(other_software), std::ptr::null_mut(), &rl)
                .expect("second event tap");
            rl_tx.send(SendRunLoop(rl.clone())).expect("send run loop");
            while !stop_thread.load(Ordering::SeqCst) {
                CFRunLoop::run();
            }
            CGEvent::tap_enable(&tap, false);
            tap.invalidate();
        });
        let other_rl = rl_rx.recv().expect("second tap started");

        cap.control.set_grab(true);
        std::thread::sleep(Duration::from_millis(200));
        let at = cursor_pos().expect("cursor");
        let _ = drain(&mut cap.events, 100);
        for ty in [CGEventType::OtherMouseDown, CGEventType::OtherMouseUp] {
            let ev = CGEvent::new_mouse_event(None, ty, cg(at), CGMouseButton::Center).expect("mouse event");
            CGEvent::set_integer_value_field(Some(&ev), CGEventField::MouseEventButtonNumber, 3);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        }
        let back: Vec<bool> = drain(&mut cap.events, 500)
            .into_iter()
            .filter_map(|e| match e {
                CaptureEvent::Button { button: MouseButton::Back, down } => Some(down),
                _ => None,
            })
            .collect();
        cap.control.set_grab(false);
        cap.control.stop();
        stop.store(true, Ordering::SeqCst);
        while !other.is_finished() {
            other_rl.0.stop();
            std::thread::sleep(Duration::from_millis(10));
        }
        other.join().expect("second tap thread");
        assert_eq!(back, vec![true, false], "Back did not reach the client exactly once: {back:?}");
        assert_eq!(OTHER_SOFTWARE_SAW.load(Ordering::SeqCst), 0, "mouse software saw Back: it acts on this Mac too");
    }

    /// Posts a key the way mouse utilities (Logi Options+, `SteerMouse`, …) do for
    /// buttons set to a shortcut: at the session level, past the HID tap.
    fn post_key(code: u16, down: bool, flags: CGEventFlags) {
        let ev = CGEvent::new_keyboard_event(None, code, down).expect("keyboard event");
        CGEvent::set_flags(Some(&ev), flags);
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&ev));
    }

    fn saw_key(rx: &mut mpsc::Receiver<CaptureEvent>, want: KeyCode) -> bool {
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            match rx.try_recv() {
                Ok(CaptureEvent::Key { key, down: true }) if key == want => return true,
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        false
    }

    fn the_session_tap_only_runs_while_a_client_has_control() {
        let (shared, _events) = start_taps().expect("event taps (Accessibility)");
        let control = Control(shared.clone());
        let session_on = || {
            let taps = lock(&shared.taps);
            CGEvent::tap_is_enabled(taps.as_ref().and_then(|t| t.session.as_ref()).expect("session tap"))
        };
        assert!(!session_on(), "on while this Mac has control: its gestures stop working");
        control.set_grab(true);
        assert!(session_on(), "off while a client has control: gestures and remapped buttons act here");
        control.set_grab(false);
        assert!(!session_on(), "still on after control came back");
        control.stop();
    }

    /// Switching the session tap off (control back here) makes macOS report
    /// it; answering that by setting both taps again made it report again,
    /// forever: a busy core and a stream of "interrupted" events.
    fn an_idle_mac_does_not_keep_switching_taps() {
        let mut cap = start_capture().expect("event tap (Accessibility)");
        cap.control.set_grab(true);
        std::thread::sleep(Duration::from_millis(100));
        cap.control.set_grab(false);
        let _ = drain(&mut cap.events, 100);
        let interrupted =
            drain(&mut cap.events, 700).into_iter().filter(|e| matches!(e, CaptureEvent::Interrupted)).count();
        cap.control.stop();
        assert_eq!(interrupted, 0, "the taps kept being switched while nothing happened");
    }

    fn shortcuts_posted_by_mouse_software_go_to_the_client_while_grabbed() {
        let mut cap = start_capture().expect("event tap (Accessibility)");
        cap.control.set_grab(true);
        std::thread::sleep(Duration::from_millis(200));
        // ⌘C from a mouse button set to "Copy".
        post_key(0x08, true, CGEventFlags::MaskCommand);
        post_key(0x08, false, CGEventFlags::MaskCommand);
        let got = saw_key(&mut cap.events, KeyCode(0x06));
        cap.control.set_grab(false);
        cap.control.stop();
        assert!(got, "a shortcut posted by mouse software was not sent to the client (it acts on this Mac)");
    }

    /// A media / special key: a system-defined event, not a key event.
    fn post_media(nx_key: i64, down: bool) {
        let state: i64 = if down { 0x0A } else { 0x0B };
        let data1 = isize::try_from((nx_key << 16) | (state << 8)).expect("data1");
        let ns = objc2_app_kit::NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            objc2_app_kit::NSEventType::SystemDefined,
            CGPoint { x: 0.0, y: 0.0 },
            objc2_app_kit::NSEventModifierFlags(0),
            0.0,
            0,
            None,
            AUX_CONTROL_BUTTONS,
            data1,
            -1,
        )
        .expect("system-defined event");
        let ev = ns.CGEvent().expect("its CGEvent");
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
    }

    fn media_keys_go_to_the_client_while_grabbed() {
        let mut cap = start_capture().expect("event tap (Accessibility)");
        cap.control.set_grab(true);
        std::thread::sleep(Duration::from_millis(200));
        post_media(0, true); // NX_KEYTYPE_SOUND_UP
        post_media(0, false);
        let got = saw_key(&mut cap.events, KeyCode(hid::VOLUME_UP));
        cap.control.set_grab(false);
        cap.control.stop();
        assert!(got, "volume up was not sent to the client (it acts on this Mac)");
    }
}
