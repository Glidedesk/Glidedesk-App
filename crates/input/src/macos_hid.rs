//! External mice, taken exclusively while another computer has control (macOS).
//!
//! Mouse software such as Logi Options+ reads a mouse's gesture button and
//! wheels itself and acts on them through private system calls (Mission
//! Control, switching desktops): no event tap ever sees that, so those gestures
//! went on acting on this Mac while the cursor was on another computer. While
//! grabbed, every external pointing device is therefore opened exclusively
//! ("seized"): neither macOS nor that software gets its reports, and Glidedesk
//! reads the standard ones itself — motion, buttons, wheels. When control comes
//! back the devices are closed and belong to the system again.
//!
//! Not seized: built-in devices (the trackpad) and Apple's own mice and
//! trackpads, whose gestures macOS turns into events the session tap already
//! keeps from acting here. Buttons that such software took over for its own
//! actions report nothing standard, so they do nothing while seized.
//!
//! Needs Input Monitoring. Without it opening fails and those mice keep the
//! event-tap path.

#![allow(unsafe_code)]

use std::collections::{HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use glidedesk_proto::MouseButton;
use objc2_core_foundation::{CFRetained, CFRunLoop, CFString, kCFRunLoopCommonModes};
use tracing::{debug, info, warn};

type Ref = *mut c_void;
type IoReturn = i32;
type ValueCallback = unsafe extern "C-unwind" fn(context: *mut c_void, result: IoReturn, sender: Ref, value: Ref);

const SEIZE: u32 = 1; // kIOHIDOptionsTypeSeizeDevice
const OPTIONS_NONE: u32 = 0;
const LISTEN_EVENT: u32 = 1; // kIOHIDRequestTypeListenEvent
const ACCESS_GRANTED: u32 = 0; // kIOHIDAccessTypeGranted
const CF_NUMBER_SINT64: isize = 4; // kCFNumberSInt64Type
const CF_NUMBER_DOUBLE: isize = 13; // kCFNumberDoubleType

const PAGE_GENERIC_DESKTOP: u32 = 0x01;
const PAGE_BUTTON: u32 = 0x09;
const PAGE_CONSUMER: u32 = 0x0C;
const PAGE_VENDOR: u32 = 0xFF00;
const USAGE_POINTER: u32 = 0x01;
const USAGE_MOUSE: u32 = 0x02;
const USAGE_X: u32 = 0x30;
const USAGE_Y: u32 = 0x31;
const USAGE_WHEEL: u32 = 0x38;
const USAGE_AC_PAN: u32 = 0x238;
/// Apple's USB and Bluetooth vendor ids.
const APPLE_VENDORS: [i64; 2] = [0x05AC, 0x004C];

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDManagerCreate(allocator: *const c_void, options: u32) -> Ref;
    fn IOHIDManagerSetDeviceMatching(manager: Ref, matching: *const c_void);
    fn IOHIDManagerCopyDevices(manager: Ref) -> *const c_void;
    fn IOHIDDeviceOpen(device: Ref, options: u32) -> IoReturn;
    fn IOHIDDeviceClose(device: Ref, options: u32) -> IoReturn;
    fn IOHIDDeviceScheduleWithRunLoop(device: Ref, run_loop: *const c_void, mode: *const c_void);
    fn IOHIDDeviceUnscheduleFromRunLoop(device: Ref, run_loop: *const c_void, mode: *const c_void);
    fn IOHIDDeviceRegisterInputValueCallback(device: Ref, callback: Option<ValueCallback>, context: *mut c_void);
    fn IOHIDDeviceGetProperty(device: Ref, key: *const c_void) -> *const c_void;
    fn IOHIDDeviceConformsTo(device: Ref, page: u32, usage: u32) -> u8;
    fn IOHIDValueGetElement(value: Ref) -> Ref;
    fn IOHIDValueGetIntegerValue(value: Ref) -> isize;
    fn IOHIDElementGetUsagePage(element: Ref) -> u32;
    fn IOHIDElementGetUsage(element: Ref) -> u32;
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFSetGetCount(set: *const c_void) -> isize;
    fn CFSetGetValues(set: *const c_void, values: *mut *const c_void);
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: *const c_void, kind: isize, value: *mut c_void) -> u8;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(boolean: *const c_void) -> u8;
    fn CFPreferencesCopyAppValue(key: *const c_void, application: *const c_void) -> *const c_void;
    static kCFPreferencesAnyApplication: *const c_void;
}

/// Input Monitoring is allowed (needed to seize).
pub fn access_granted() -> bool {
    // SAFETY: argument-only query of the process's TCC state.
    unsafe { IOHIDCheckAccess(LISTEN_EVENT) == ACCESS_GRANTED }
}

/// Shows macOS's Input Monitoring prompt (once; afterwards only System Settings).
pub fn request_access() {
    // SAFETY: as above.
    let _ = unsafe { IOHIDRequestAccess(LISTEN_EVENT) };
}

/// What a seized mouse did, already in this Mac's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidEvent {
    /// Pointer motion in points, accelerated like this Mac's own pointer.
    Motion {
        dx: i32,
        dy: i32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    /// Wheel units of 1/120 notch, scrolling the way this Mac does.
    Wheel {
        dx: i32,
        dy: i32,
    },
}

fn as_ptr<T: ?Sized + objc2_core_foundation::Type>(r: &CFRetained<T>) -> *const c_void {
    CFRetained::as_ptr(r).as_ptr().cast_const().cast()
}

fn cf_number<T: Default>(v: *const c_void, kind: isize) -> Option<T> {
    // SAFETY: `v` is null or a live CF object; its type is checked before use,
    // and `kind` matches `T` (SInt64 → i64, Double → f64) at every call site.
    unsafe {
        if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
            return None;
        }
        let mut out = T::default();
        (CFNumberGetValue(v, kind, (&raw mut out).cast()) != 0).then_some(out)
    }
}

fn number(v: *const c_void) -> Option<i64> {
    cf_number::<i64>(v, CF_NUMBER_SINT64)
}

fn boolean(v: *const c_void) -> Option<bool> {
    // SAFETY: `v` is null or a live CF object; its type is checked before use.
    let is_bool = !v.is_null() && unsafe { CFGetTypeID(v) == CFBooleanGetTypeID() };
    if is_bool {
        // SAFETY: a CFBoolean.
        return Some(unsafe { CFBooleanGetValue(v) } != 0);
    }
    number(v).map(|n| n != 0)
}

fn property(device: Ref, key: &'static str) -> *const c_void {
    let k = CFString::from_static_str(key);
    // SAFETY: valid device and CFString key; the value is borrowed from the device.
    unsafe { IOHIDDeviceGetProperty(device, as_ptr(&k)) }
}

/// An owned CF reference from a Copy function (released on drop).
struct Owned(*const c_void);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: an owned reference.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// A global preference (`defaults read -g <key>`), if set.
fn global_pref(key: &'static str) -> Owned {
    let k = CFString::from_static_str(key);
    // SAFETY: valid key; kCFPreferencesAnyApplication is a static CFString.
    Owned(unsafe { CFPreferencesCopyAppValue(as_ptr(&k), kCFPreferencesAnyApplication) })
}

/// How the pointer moves on this Mac: tracking speed and natural scrolling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feel {
    /// System Settings → Mouse → Tracking speed (0…3; macOS's default 0.6875).
    pub scaling: f64,
    pub natural_scrolling: bool,
}

impl Feel {
    fn current() -> Self {
        let scaling = global_pref("com.apple.mouse.scaling");
        let natural = global_pref("com.apple.swipescrolldirection");
        Self {
            scaling: cf_number::<f64>(scaling.0, CF_NUMBER_DOUBLE).filter(|v| *v >= 0.0).unwrap_or(DEFAULT_SCALING),
            natural_scrolling: boolean(natural.0).unwrap_or(true),
        }
    }
}

const DEFAULT_SCALING: f64 = 0.6875;
/// Points per inch at slow speed (default tracking speed).
const SLOW_POINTS_PER_INCH: f64 = 300.0;
/// Resolution assumed when a mouse doesn't report one.
const DEFAULT_DPI: f64 = 1000.0;
/// Speed is measured over this much recent motion.
const SPEED_WINDOW: Duration = Duration::from_millis(30);

/// Turns a mouse's counts into accelerated points, like macOS's own curve:
/// steady when slow, several times faster for quick moves.
#[derive(Debug)]
pub struct Accel {
    dpi: f64,
    factor: f64,
    recent: VecDeque<(Instant, f64)>,
    rem: (f64, f64),
}

impl Accel {
    pub fn new(dpi: f64, feel: Feel) -> Self {
        let factor = (feel.scaling / DEFAULT_SCALING).clamp(0.2, 4.0).sqrt();
        Self { dpi: if dpi > 0.0 { dpi } else { DEFAULT_DPI }, factor, recent: VecDeque::new(), rem: (0.0, 0.0) }
    }

    /// Gain (points per inch) at `ips` inches per second.
    fn points_per_inch(&self, ips: f64) -> f64 {
        let boost = (1.0 + 0.9 * (ips - 1.0).max(0.0).powf(0.9)).min(6.0);
        SLOW_POINTS_PER_INCH * boost * self.factor
    }

    /// Points for `counts` on one axis (`x` or not), reported at `now`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub fn apply(&mut self, counts: i64, x: bool, now: Instant) -> i32 {
        let c = counts as f64;
        self.recent.push_back((now, c.abs()));
        while self.recent.front().is_some_and(|(t, _)| now.duration_since(*t) > SPEED_WINDOW) {
            self.recent.pop_front();
        }
        let window = self.recent.iter().map(|(_, v)| v).sum::<f64>();
        let ips = window / self.dpi / SPEED_WINDOW.as_secs_f64();
        let points = c / self.dpi * self.points_per_inch(ips);
        let rem = if x { &mut self.rem.0 } else { &mut self.rem.1 };
        let total = points + *rem;
        let whole = total.trunc();
        *rem = total - whole;
        whole as i32
    }
}

/// What one value of a seized device means (`accel` turns counts into points).
pub fn translate(
    page: u32,
    usage: u32,
    value: i64,
    accel: &mut dyn FnMut(i64, bool) -> i32,
    natural: bool,
) -> Option<HidEvent> {
    let sign = if natural { -1 } else { 1 };
    let notches = i32::try_from(value.clamp(-1000, 1000)).unwrap_or(0);
    match (page, usage) {
        (PAGE_GENERIC_DESKTOP, USAGE_X) if value != 0 => Some(HidEvent::Motion { dx: accel(value, true), dy: 0 }),
        (PAGE_GENERIC_DESKTOP, USAGE_Y) if value != 0 => Some(HidEvent::Motion { dx: 0, dy: accel(value, false) }),
        // Positive = away from the user (scroll up), as a Mac reports it without
        // natural scrolling; the protocol carries it as seen on this Mac.
        (PAGE_GENERIC_DESKTOP, USAGE_WHEEL) if value != 0 => Some(HidEvent::Wheel { dx: 0, dy: notches * 120 * sign }),
        // AC Pan: positive = right; a Mac's horizontal axis is positive = left.
        (PAGE_CONSUMER, USAGE_AC_PAN) if value != 0 => Some(HidEvent::Wheel { dx: -notches * 120 * sign, dy: 0 }),
        (PAGE_BUTTON, n) => {
            let button = match n {
                1 => MouseButton::Left,
                2 => MouseButton::Right,
                3 => MouseButton::Middle,
                4 => MouseButton::Back,
                5 => MouseButton::Forward,
                _ => return None,
            };
            Some(HidEvent::Button { button, down: value != 0 })
        }
        _ => None,
    }
}

/// Callback context, alive as long as the [`Seizer`].
struct Context {
    sink: Box<dyn Fn(HidEvent) + Send + Sync>,
    /// Acceleration per device (keyed by its address) and this Mac's feel.
    state: Mutex<(Vec<(usize, Accel)>, Feel)>,
}

unsafe extern "C-unwind" fn on_value(context: *mut c_void, _result: IoReturn, sender: Ref, value: Ref) {
    if context.is_null() || value.is_null() {
        return;
    }
    // SAFETY: `context` is the boxed `Context` owned by the `Seizer`, which
    // unregisters this callback before freeing it.
    let ctx = unsafe { &*context.cast::<Context>() };
    // SAFETY: `value` is valid for the duration of the callback.
    let element = unsafe { IOHIDValueGetElement(value) };
    if element.is_null() {
        return;
    }
    // SAFETY: a valid element and value.
    let (page, usage, v) = unsafe {
        (
            IOHIDElementGetUsagePage(element),
            IOHIDElementGetUsage(element),
            i64::try_from(IOHIDValueGetIntegerValue(value)).unwrap_or(0),
        )
    };
    let now = Instant::now();
    let event = {
        let mut guard = ctx.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (accels, feel) = &mut *guard;
        let feel = *feel;
        let key = sender as usize;
        let i = accels.iter().position(|(k, _)| *k == key).unwrap_or_else(|| {
            accels.push((key, Accel::new(dpi(sender), feel)));
            accels.len() - 1
        });
        let accel = &mut accels[i].1;
        translate(page, usage, v, &mut |c, x| accel.apply(c, x, now), feel.natural_scrolling)
    };
    if let Some(e) = event {
        (ctx.sink)(e);
    }
}

/// A device's reported resolution (dpi), if any.
fn dpi(device: Ref) -> f64 {
    if device.is_null() {
        return DEFAULT_DPI;
    }
    // "HIDPointerResolution": dpi as 16.16 fixed point.
    #[allow(clippy::cast_precision_loss)]
    number(property(device, "HIDPointerResolution")).filter(|v| *v > 0).map_or(DEFAULT_DPI, |v| v as f64 / 65536.0)
}

struct Device {
    device: Ref,
    /// Its values are read (a mouse), or it is only kept from others (a vendor
    /// interface of the same hardware, e.g. the one a receiver's software uses).
    reads: bool,
}

/// Seizes and releases the external mice.
pub struct Seizer {
    context: *mut Context,
    run_loop: CFRetained<CFRunLoop>,
    seized: Vec<Device>,
    /// Closed devices, still referenced: `release` runs on another thread than
    /// the callbacks, and one already running may still use its device.
    /// Let go of at the next `seize` (or on drop).
    closed: Vec<Ref>,
    warned: bool,
}

// SAFETY: the raw pointers are CF objects (thread-safe reference counting) and
// the boxed context (`Sync`); a `Seizer` is only used under the capture's lock.
unsafe impl Send for Seizer {}

impl std::fmt::Debug for Seizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seizer").field("seized", &self.seized.len()).finish_non_exhaustive()
    }
}

impl Seizer {
    /// Callbacks run on `run_loop` (the capture thread's) and go to `sink`.
    pub fn new(run_loop: CFRetained<CFRunLoop>, sink: Box<dyn Fn(HidEvent) + Send + Sync>) -> Self {
        let context = Box::into_raw(Box::new(Context { sink, state: Mutex::new((Vec::new(), Feel::current())) }));
        Self { context, run_loop, seized: Vec::new(), closed: Vec::new(), warned: false }
    }

    fn mode() -> *const c_void {
        // SAFETY: kCFRunLoopCommonModes is a valid static CFString.
        unsafe { kCFRunLoopCommonModes }.map_or(std::ptr::null(), |m| std::ptr::from_ref(m).cast())
    }

    /// Takes every external mouse (and its vendor interfaces) for Glidedesk.
    pub fn seize(&mut self) {
        if !self.seized.is_empty() {
            return;
        }
        self.forget_closed();
        if !access_granted() {
            if !self.warned {
                self.warned = true;
                warn!(
                    "Input Monitoring is off: an external mouse's gestures (e.g. from Logi Options+) can still act \
                     on this Mac while another computer has control"
                );
            }
            return;
        }
        {
            // SAFETY: the context lives as long as `self`.
            let ctx = unsafe { &*self.context };
            *ctx.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = (Vec::new(), Feel::current());
        }
        let run_loop = as_ptr(&self.run_loop);
        for (device, reads) in external_devices() {
            // SAFETY: a retained device from `external_devices`.
            let r = unsafe { IOHIDDeviceOpen(device, SEIZE) };
            if r != 0 {
                debug!(code = r, "could not take a mouse exclusively");
                // SAFETY: the reference `external_devices` handed over.
                unsafe { CFRelease(device.cast_const()) };
                continue;
            }
            if reads {
                // SAFETY: an open device; the context outlives the registration.
                unsafe {
                    IOHIDDeviceRegisterInputValueCallback(device, Some(on_value), self.context.cast());
                    IOHIDDeviceScheduleWithRunLoop(device, run_loop, Self::mode());
                }
            }
            self.seized.push(Device { device, reads });
        }
        self.run_loop.wake_up();
        let mice = self.seized.iter().filter(|d| d.reads).count();
        if mice > 0 {
            info!(mice, "external mice taken while another computer has control");
        }
    }

    /// Gives the mice back to the system.
    pub fn release(&mut self) {
        let run_loop = as_ptr(&self.run_loop);
        for d in self.seized.drain(..) {
            // SAFETY: devices opened in `seize`, unregistered before they go.
            unsafe {
                if d.reads {
                    IOHIDDeviceRegisterInputValueCallback(d.device, None, std::ptr::null_mut());
                    IOHIDDeviceUnscheduleFromRunLoop(d.device, run_loop, Self::mode());
                }
                let _ = IOHIDDeviceClose(d.device, OPTIONS_NONE);
            }
            self.closed.push(d.device);
        }
    }

    fn forget_closed(&mut self) {
        for d in self.closed.drain(..) {
            // SAFETY: the reference kept by `release`; no callback can be using
            // it any more (it was unscheduled a whole grab ago).
            unsafe { CFRelease(d.cast_const()) };
        }
    }
}

impl Drop for Seizer {
    fn drop(&mut self) {
        self.release();
        self.forget_closed();
        // SAFETY: no device refers to the context any more.
        drop(unsafe { Box::from_raw(self.context) });
    }
}

/// External pointing devices to read, and vendor interfaces of the same
/// hardware to keep from others. Each is retained for the caller.
fn external_devices() -> Vec<(Ref, bool)> {
    // SAFETY: a fresh manager matching every HID device, released below; the
    // set's values are borrowed from the set until retained.
    unsafe {
        let manager = IOHIDManagerCreate(std::ptr::null(), OPTIONS_NONE);
        if manager.is_null() {
            return Vec::new();
        }
        let manager = Owned(manager.cast_const());
        IOHIDManagerSetDeviceMatching(manager.0.cast_mut(), std::ptr::null());
        let set = Owned(IOHIDManagerCopyDevices(manager.0.cast_mut()));
        if set.0.is_null() {
            return Vec::new();
        }
        let n = usize::try_from(CFSetGetCount(set.0)).unwrap_or(0);
        let mut values = vec![std::ptr::null(); n];
        CFSetGetValues(set.0, values.as_mut_ptr());
        let all: Vec<Ref> = values.into_iter().filter(|d| !d.is_null()).map(<*const c_void>::cast_mut).collect();
        let is_mouse = |d: Ref| {
            IOHIDDeviceConformsTo(d, PAGE_GENERIC_DESKTOP, USAGE_MOUSE) != 0
                || IOHIDDeviceConformsTo(d, PAGE_GENERIC_DESKTOP, USAGE_POINTER) != 0
        };
        let external = |d: Ref| {
            let built_in = boolean(property(d, "Built-In")).unwrap_or(false);
            let vendor = number(property(d, "VendorID")).unwrap_or(0);
            !built_in && !APPLE_VENDORS.contains(&vendor)
        };
        let location = |d: Ref| number(property(d, "LocationID"));
        let mice: Vec<Ref> = all.iter().copied().filter(|&d| is_mouse(d) && external(d)).collect();
        let places: HashSet<i64> = mice.iter().filter_map(|&d| location(d)).collect();
        let vendor_parts = all.iter().copied().filter(|&d| {
            !is_mouse(d)
                && number(property(d, "PrimaryUsagePage")).is_some_and(|p| p >= i64::from(PAGE_VENDOR))
                && location(d).is_some_and(|l| places.contains(&l))
        });
        mice.iter()
            .map(|&d| (d, true))
            .chain(vendor_parts.map(|d| (d, false)))
            .map(|(d, reads)| (CFRetain(d.cast_const()).cast_mut(), reads))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEEL: Feel = Feel { scaling: DEFAULT_SCALING, natural_scrolling: true };

    fn at(t0: Instant, ms: u32) -> Instant {
        t0 + Duration::from_millis(u64::from(ms))
    }

    #[test]
    fn slow_motion_is_steady_and_fast_motion_is_accelerated() {
        let t0 = Instant::now();
        let mut slow = Accel::new(1000.0, FEEL);
        // 2 counts every 8 ms at 1000 dpi: 0.25 in/s, 0.2 in in total.
        let s: i32 = (0..100u32).map(|i| slow.apply(2, true, at(t0, i * 8))).sum();
        let mut fast = Accel::new(1000.0, FEEL);
        // 40 counts every 8 ms: 5 in/s.
        let f: i32 = (0..100u32).map(|i| fast.apply(40, true, at(t0, i * 8))).sum();
        assert!((55..=65).contains(&s), "0.2 in slowly should be ~60 points, got {s}");
        assert!(f > 40 * s, "fast motion must be accelerated: {f} vs {s}");
    }

    #[test]
    fn remainders_are_kept_so_slow_motion_is_not_lost() {
        let t0 = Instant::now();
        let mut a = Accel::new(4000.0, FEEL);
        let total: i32 = (0..1000u32).map(|i| a.apply(1, false, at(t0, i * 50))).sum();
        assert!((74..=75).contains(&total), "1000 counts at 4000 dpi = 0.25 in = 75 points, got {total}");
    }

    #[test]
    fn values_translate_to_buttons_wheels_and_motion() {
        let mut accel = |c: i64, _x: bool| i32::try_from(c).unwrap_or(0);
        let mut t = |page, usage, v, natural| translate(page, usage, v, &mut accel, natural);
        assert_eq!(t(PAGE_BUTTON, 4, 1, true), Some(HidEvent::Button { button: MouseButton::Back, down: true }));
        assert_eq!(t(PAGE_BUTTON, 5, 0, true), Some(HidEvent::Button { button: MouseButton::Forward, down: false }));
        assert_eq!(t(PAGE_BUTTON, 9, 1, true), None, "no counterpart on the other computer");
        assert_eq!(t(PAGE_GENERIC_DESKTOP, USAGE_WHEEL, 1, false), Some(HidEvent::Wheel { dx: 0, dy: 120 }));
        assert_eq!(t(PAGE_GENERIC_DESKTOP, USAGE_WHEEL, 1, true), Some(HidEvent::Wheel { dx: 0, dy: -120 }));
        assert_eq!(t(PAGE_CONSUMER, USAGE_AC_PAN, 1, false), Some(HidEvent::Wheel { dx: -120, dy: 0 }));
        assert_eq!(t(PAGE_GENERIC_DESKTOP, USAGE_X, 7, true), Some(HidEvent::Motion { dx: 7, dy: 0 }));
        assert_eq!(t(PAGE_GENERIC_DESKTOP, USAGE_Y, 0, true), None, "no motion, no event");
        assert_eq!(t(PAGE_VENDOR, 1, 1, true), None, "vendor reports are only kept from others");
    }
}
