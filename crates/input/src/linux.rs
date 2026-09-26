//! Linux backend.
//!
//! * Injection (client): **uinput** virtual devices — works on X11 and
//!   Wayland; the package's udev rule gives the signed-in user access to
//!   `/dev/uinput`. Falls back to XTest when uinput is not accessible.
//! * Capture (server): **X11** only — XInput2 raw events, pointer/keyboard
//!   grab while another computer has control, XFixes to hide the cursor.
//!   Wayland has no general capture API yet; the app explains this.
//! * Monitors: RandR (also served by XWayland on Wayland desktops).
//!
//! Key events are never logged.

#![allow(clippy::doc_markdown)] // X11 extension names (XInput2, XTest, RandR…)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode as EvKey, RelativeAxisCode, UinputAbsSetup,
};
use nexpingdesk_proto::{Input, KeyCode, LedState, MonitorId, MonitorInfo, MouseButton, Point, Rect};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, GrabStatus, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::keymap;
use crate::{CAPTURE_QUEUE, Capture, CaptureControl, CaptureEvent, Injector, InputError, Permissions, Pressed};

const ABS_MAX: i32 = 32_767;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn os(e: impl std::fmt::Display) -> InputError {
    InputError::Os(e.to_string())
}

fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

fn x11() -> Result<(RustConnection, usize), InputError> {
    x11rb::connect(None).map_err(|e| InputError::Os(format!("cannot connect to the X server: {e}")))
}

fn uinput_writable() -> bool {
    std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok()
}

pub fn permissions() -> Permissions {
    Permissions { accessibility: uinput_writable() || x11().is_ok(), input_monitoring: !is_wayland() && x11().is_ok() }
}

// ---------------------------------------------------------------------------
// monitors
// ---------------------------------------------------------------------------

pub fn monitors() -> Result<Vec<MonitorInfo>, InputError> {
    let (conn, screen) = x11()?;
    let root = conn.setup().roots[screen].root;
    let reply = conn.randr_get_monitors(root, true).map_err(os)?.reply().map_err(os)?;
    let mut out = Vec::new();
    for m in reply.monitors {
        let name = conn
            .get_atom_name(m.name)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or_else(|| "Display".into(), |r| String::from_utf8_lossy(&r.name).into_owned());
        out.push(MonitorInfo {
            id: MonitorId(format!("x11:{name}")),
            name,
            bounds: Rect::new(i32::from(m.x), i32::from(m.y), i32::from(m.width), i32::from(m.height)),
            scale: 1.0,
            primary: m.primary,
        });
    }
    if !out.is_empty() && !out.iter().any(|m| m.primary) {
        out[0].primary = true;
    }
    if out.is_empty() {
        let s = &conn.setup().roots[screen];
        out.push(MonitorInfo {
            id: MonitorId("x11:screen".into()),
            name: "Screen".into(),
            bounds: Rect::new(0, 0, i32::from(s.width_in_pixels), i32::from(s.height_in_pixels)),
            scale: 1.0,
            primary: true,
        });
    }
    Ok(out)
}

fn desktop() -> Rect {
    monitors().ok().and_then(|m| Rect::union_all(m.iter().map(|m| &m.bounds))).unwrap_or(Rect::new(0, 0, 1920, 1080))
}

// ---------------------------------------------------------------------------
// capture (X11)
// ---------------------------------------------------------------------------

struct Shared {
    conn: RustConnection,
    root: u32,
    wake: u32,
    grabbed: AtomicBool,
    stopped: AtomicBool,
    pin: Mutex<Point>,
    /// Master pointer and keyboard (XI2 device ids) grabbed while a client has control.
    masters: (u16, u16),
    tx: mpsc::Sender<CaptureEvent>,
}

impl Shared {
    fn emit(&self, ev: CaptureEvent) {
        if self.tx.try_send(ev).is_err() {
            debug!("capture queue full; event dropped");
        }
    }

    fn cursor(&self) -> Option<Point> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some(Point::new(i32::from(r.root_x), i32::from(r.root_y)))
    }

    fn warp(&self, p: Point) {
        let (x, y) = (i16::try_from(p.x).unwrap_or(i16::MAX), i16::try_from(p.y).unwrap_or(i16::MAX));
        let _ = self.conn.warp_pointer(NONE, self.root, 0, 0, 0, 0, x, y);
        let _ = self.conn.flush();
    }
}

#[derive(Clone)]
struct Control(Arc<Shared>);

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Capture").field("grabbed", &self.0.grabbed.load(Ordering::Relaxed)).finish()
    }
}

impl CaptureControl for Control {
    fn set_grab(&self, grab: bool) {
        let s = &self.0;
        if s.grabbed.swap(grab, Ordering::SeqCst) == grab {
            return;
        }
        if grab {
            // Park the hidden cursor mid-screen so deltas never hit an edge.
            let d = monitors().ok().and_then(|m| m.into_iter().find(|m| m.primary)).map_or_else(desktop, |m| m.bounds);
            let centre = Point::new(d.x + d.w / 2, d.y + d.h / 2);
            *lock(&s.pin) = centre;
            s.warp(centre);
            // An XI2 grab that asks for the raw events: with a core grab the X
            // server stops sending raw events to the grabbing client (XI ≥ 2.1
            // "already delivered to the grab"), and motion, keys and buttons
            // would stop reaching us the moment the cursor left this screen.
            let (pointer, keyboard) = s.masters;
            let pointer_mask = xinput::XIEventMask::RAW_MOTION
                | xinput::XIEventMask::RAW_BUTTON_PRESS
                | xinput::XIEventMask::RAW_BUTTON_RELEASE;
            let key_mask = xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE;
            for (dev, mask) in [(pointer, pointer_mask), (keyboard, key_mask)] {
                let status = s
                    .conn
                    .xinput_xi_grab_device(
                        s.root,
                        CURRENT_TIME,
                        NONE,
                        dev,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                        xinput::GrabOwner::NO_OWNER,
                        &[u32::from(mask)],
                    )
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| r.status);
                if status != Some(GrabStatus::SUCCESS) {
                    warn!(device = dev, ?status, "could not grab the input device");
                }
            }
            let _ = s.conn.xfixes_hide_cursor(s.root);
        } else {
            let (pointer, keyboard) = s.masters;
            let _ = s.conn.xinput_xi_ungrab_device(CURRENT_TIME, pointer);
            let _ = s.conn.xinput_xi_ungrab_device(CURRENT_TIME, keyboard);
            let _ = s.conn.xfixes_show_cursor(s.root);
        }
        let _ = s.conn.flush();
    }

    fn warp(&self, p: Point) {
        self.0.warp(p);
    }

    fn cursor(&self) -> Option<Point> {
        self.0.cursor()
    }

    fn leds(&self) -> LedState {
        led_state(&self.0.conn)
    }

    fn stop(&self) {
        self.set_grab(false);
        self.0.stopped.store(true, Ordering::SeqCst);
        // Wake the event loop.
        let ev = ClientMessageEvent::new(32, self.0.wake, AtomEnum::NOTICE, [0u32; 5]);
        let _ = self.0.conn.send_event(false, self.0.wake, EventMask::NO_EVENT, ev);
        let _ = self.0.conn.flush();
    }
}

fn led_state(conn: &RustConnection) -> LedState {
    conn.get_keyboard_control()
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| LedState { caps: r.led_mask & 1 != 0, num: r.led_mask & 2 != 0, scroll: r.led_mask & 4 != 0 })
        .unwrap_or_default()
}

fn fp(v: xinput::Fp3232) -> f64 {
    f64::from(v.integral) + f64::from(v.frac) / 4_294_967_296.0
}

/// X/Y deltas from a raw motion event (axes 0 and 1 when present).
#[allow(clippy::cast_possible_truncation)]
fn raw_delta(ev: &xinput::RawMotionEvent) -> (i32, i32) {
    let mut vals = ev.axisvalues.iter();
    let mut out = [0.0f64; 2];
    let mut axis = 0usize;
    for word in &ev.valuator_mask {
        for bit in 0..32 {
            if word & (1 << bit) != 0 {
                let Some(v) = vals.next() else { break };
                if axis < 2 {
                    out[axis] = fp(*v);
                }
            }
            axis += 1;
        }
    }
    (out[0].round() as i32, out[1].round() as i32)
}

fn button(detail: u32) -> Option<MouseButton> {
    Some(match detail {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        8 => MouseButton::Back,
        9 => MouseButton::Forward,
        _ => return None,
    })
}

pub fn start_capture() -> Result<Capture, InputError> {
    if is_wayland() {
        return Err(InputError::Os(
            "sharing this computer's keyboard and mouse is not supported on Wayland yet — sign in with an X11 session, \
             or use this computer as a client"
                .into(),
        ));
    }
    let (conn, screen) = x11()?;
    let root = conn.setup().roots[screen].root;
    conn.xinput_xi_query_version(2, 2)
        .map_err(os)?
        .reply()
        .map_err(|_| InputError::Os("XInput 2.2 is not available".into()))?;
    let _ = conn.xfixes_query_version(5, 0).map_err(os)?.reply();
    let mask = xinput::XIEventMask::RAW_MOTION
        | xinput::XIEventMask::RAW_KEY_PRESS
        | xinput::XIEventMask::RAW_KEY_RELEASE
        | xinput::XIEventMask::RAW_BUTTON_PRESS
        | xinput::XIEventMask::RAW_BUTTON_RELEASE;
    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask { deviceid: xinput::Device::ALL_MASTER.into(), mask: vec![mask] }],
    )
    .map_err(os)?
    .check()
    .map_err(os)?;
    let masters = master_devices(&conn);
    let wake = conn.generate_id().map_err(os)?;
    conn.create_window(0, wake, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())
        .map_err(os)?;
    conn.flush().map_err(os)?;

    let (tx, rx) = mpsc::channel(CAPTURE_QUEUE);
    let shared = Arc::new(Shared {
        conn,
        root,
        wake,
        grabbed: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        pin: Mutex::new(Point::default()),
        masters,
        tx,
    });
    let s = shared.clone();
    std::thread::Builder::new()
        .name("nd-capture".into())
        .spawn(move || capture_loop(&s))
        .map_err(|e| InputError::Os(e.to_string()))?;
    Ok(Capture { control: Arc::new(Control(shared)), events: rx })
}

/// XI2 ids of the master pointer and keyboard (2 and 3 on a normal X server).
fn master_devices(conn: &RustConnection) -> (u16, u16) {
    let info = conn.xinput_xi_query_device(xinput::Device::ALL_MASTER).ok().and_then(|c| c.reply().ok());
    let find = |kind: xinput::DeviceType, fallback: u16| {
        info.as_ref().and_then(|r| r.infos.iter().find(|d| d.type_ == kind).map(|d| d.deviceid)).unwrap_or(fallback)
    };
    (find(xinput::DeviceType::MASTER_POINTER, 2), find(xinput::DeviceType::MASTER_KEYBOARD, 3))
}

fn capture_loop(s: &Shared) {
    while !s.stopped.load(Ordering::SeqCst) {
        let ev = match s.conn.wait_for_event() {
            Ok(ev) => ev,
            Err(e) => {
                warn!(error = %e, "X11 connection lost; capture stopped");
                s.emit(CaptureEvent::Interrupted);
                return;
            }
        };
        let grabbed = s.grabbed.load(Ordering::Relaxed);
        match ev {
            Event::XinputRawMotion(m) => {
                let (dx, dy) = raw_delta(&m);
                if dx == 0 && dy == 0 {
                    continue;
                }
                if grabbed {
                    let pin = *lock(&s.pin);
                    s.emit(CaptureEvent::Motion { pos: pin, dx, dy });
                    s.warp(pin);
                } else if let Some(pos) = s.cursor() {
                    s.emit(CaptureEvent::Motion { pos, dx, dy });
                }
            }
            Event::XinputRawKeyPress(k) => raw_key(s, k.detail, true),
            Event::XinputRawKeyRelease(k) => raw_key(s, k.detail, false),
            Event::XinputRawButtonPress(b) => raw_button(s, b.detail, true),
            Event::XinputRawButtonRelease(b) => raw_button(s, b.detail, false),
            Event::RandrScreenChangeNotify(_) => s.emit(CaptureEvent::DisplaysChanged),
            _ => {}
        }
    }
    debug!("capture thread stopped");
}

fn raw_key(s: &Shared, detail: u32, down: bool) {
    if let Some(key) = u16::try_from(detail).ok().and_then(|c| c.checked_sub(8)).and_then(keymap::linux_to_hid) {
        s.emit(CaptureEvent::Key { key, down });
    }
}

fn raw_button(s: &Shared, detail: u32, down: bool) {
    match detail {
        // X11 reports wheel notches as buttons 4–7 (the press is enough).
        4 | 5 => {
            if down {
                s.emit(CaptureEvent::Wheel { dx: 0, dy: if detail == 4 { 120 } else { -120 } });
            }
        }
        6 | 7 => {
            if down {
                s.emit(CaptureEvent::Wheel { dx: if detail == 7 { 120 } else { -120 }, dy: 0 });
            }
        }
        d => {
            if let Some(button) = button(d) {
                s.emit(CaptureEvent::Button { button, down });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// injection
// ---------------------------------------------------------------------------

enum Backend {
    /// Kernel virtual devices (X11 and Wayland).
    Uinput { keyboard: Box<VirtualDevice>, tablet: Box<VirtualDevice>, mouse: Box<VirtualDevice> },
    /// XTest (X11 only).
    Xtest { conn: Box<RustConnection>, root: u32 },
}

const DESKTOP_REFRESH: Duration = Duration::from_secs(2);

struct LinuxInjector {
    backend: Backend,
    pressed: Pressed,
    pos: Point,
    /// All monitors; re-read every few seconds so a screen plugged in (or a
    /// remote-desktop window resized) is reachable without a restart.
    desktop: Rect,
    desktop_at: std::time::Instant,
    wheel_rem: (i32, i32),
}

impl std::fmt::Debug for LinuxInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.backend {
            Backend::Uinput { .. } => "uinput",
            Backend::Xtest { .. } => "xtest",
        };
        f.debug_struct("LinuxInjector").field("backend", &kind).finish_non_exhaustive()
    }
}

const BUTTONS: [(MouseButton, EvKey, u8); 5] = [
    (MouseButton::Left, EvKey::BTN_LEFT, 1),
    (MouseButton::Middle, EvKey::BTN_MIDDLE, 2),
    (MouseButton::Right, EvKey::BTN_RIGHT, 3),
    (MouseButton::Back, EvKey::BTN_SIDE, 8),
    (MouseButton::Forward, EvKey::BTN_EXTRA, 9),
];

fn uinput_backend() -> std::io::Result<Backend> {
    let mut keys = AttributeSet::<EvKey>::new();
    for h in 0u16..=0xEE {
        if let Some(code) = keymap::hid_to_linux(KeyCode(h)) {
            keys.insert(EvKey::new(code));
        }
    }
    let keyboard = VirtualDevice::builder()?.name("Nexpingdesk keyboard").with_keys(&keys)?.build()?;
    // Absolute pointer like a VM "tablet": compositors map it over all monitors.
    let mut buttons = AttributeSet::<EvKey>::new();
    for (_, k, _) in BUTTONS {
        buttons.insert(k);
    }
    let info = AbsInfo::new(0, 0, ABS_MAX, 0, 0, 1);
    let tablet = VirtualDevice::builder()?
        .name("Nexpingdesk pointer")
        .with_keys(&buttons)?
        .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, info))?
        .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, info))?
        .build()?;
    let mut rel = AttributeSet::<RelativeAxisCode>::new();
    for a in [
        RelativeAxisCode::REL_X,
        RelativeAxisCode::REL_Y,
        RelativeAxisCode::REL_WHEEL,
        RelativeAxisCode::REL_HWHEEL,
        RelativeAxisCode::REL_WHEEL_HI_RES,
        RelativeAxisCode::REL_HWHEEL_HI_RES,
    ] {
        rel.insert(a);
    }
    let mouse =
        VirtualDevice::builder()?.name("Nexpingdesk mouse").with_keys(&buttons)?.with_relative_axes(&rel)?.build()?;
    Ok(Backend::Uinput { keyboard: Box::new(keyboard), tablet: Box::new(tablet), mouse: Box::new(mouse) })
}

pub fn injector() -> Result<Box<dyn Injector>, InputError> {
    let backend = match uinput_backend() {
        Ok(b) => b,
        Err(e) => {
            debug!(error = %e, "uinput unavailable; trying XTest");
            if is_wayland() {
                return Err(InputError::Permission(
                    "access to /dev/uinput (install the Nexpingdesk package, or add the udev rule from the .tar.gz)",
                ));
            }
            let (conn, screen) = x11()?;
            let root = conn.setup().roots[screen].root;
            conn.xtest_get_version(2, 2)
                .map_err(os)?
                .reply()
                .map_err(|_| InputError::Os("XTest is not available".into()))?;
            Backend::Xtest { conn: Box::new(conn), root }
        }
    };
    // Let the compositor register the new devices before the first event.
    std::thread::sleep(Duration::from_millis(150));
    Ok(Box::new(LinuxInjector {
        backend,
        pressed: Pressed::default(),
        pos: Point::default(),
        desktop: desktop(),
        desktop_at: std::time::Instant::now(),
        wheel_rem: (0, 0),
    }))
}

fn ev(t: EventType, code: u16, value: i32) -> InputEvent {
    InputEvent::new(t.0, code, value)
}

impl LinuxInjector {
    fn io(r: std::io::Result<()>) -> Result<(), InputError> {
        r.map_err(os)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn move_abs(&mut self, p: Point) -> Result<(), InputError> {
        if self.desktop_at.elapsed() > DESKTOP_REFRESH {
            self.desktop = desktop();
            self.desktop_at = std::time::Instant::now();
        }
        let d = self.desktop;
        let p = d.clamp(p);
        self.pos = p;
        match &mut self.backend {
            Backend::Uinput { tablet, .. } => {
                let scale = |v: i32, o: i32, size: i32| {
                    ((i64::from(v - o) * i64::from(ABS_MAX)) / i64::from((size - 1).max(1))) as i32
                };
                Self::io(tablet.emit(&[
                    ev(EventType::ABSOLUTE, AbsoluteAxisCode::ABS_X.0, scale(p.x, d.x, d.w)),
                    ev(EventType::ABSOLUTE, AbsoluteAxisCode::ABS_Y.0, scale(p.y, d.y, d.h)),
                ]))
            }
            Backend::Xtest { conn, root } => {
                let (x, y) = (i16::try_from(p.x).unwrap_or(i16::MAX), i16::try_from(p.y).unwrap_or(i16::MAX));
                conn.xtest_fake_input(6, 0, 0, *root, x, y, 0).map_err(os)?;
                conn.flush().map_err(os)
            }
        }
    }

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        match &mut self.backend {
            Backend::Uinput { mouse, .. } => Self::io(mouse.emit(&[
                ev(EventType::RELATIVE, RelativeAxisCode::REL_X.0, dx),
                ev(EventType::RELATIVE, RelativeAxisCode::REL_Y.0, dy),
            ])),
            Backend::Xtest { .. } => {
                let p = Point::new(self.pos.x.saturating_add(dx), self.pos.y.saturating_add(dy));
                self.move_abs(p)
            }
        }
    }

    fn button(&mut self, b: MouseButton, down: bool) -> Result<(), InputError> {
        self.pressed.button(b, down);
        let Some((_, key, x11)) = BUTTONS.iter().find(|(m, _, _)| *m == b).copied() else { return Ok(()) };
        match &mut self.backend {
            Backend::Uinput { tablet, .. } => Self::io(tablet.emit(&[ev(EventType::KEY, key.code(), i32::from(down))])),
            Backend::Xtest { conn, root } => {
                conn.xtest_fake_input(if down { 4 } else { 5 }, x11, 0, *root, 0, 0, 0).map_err(os)?;
                conn.flush().map_err(os)
            }
        }
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        match &mut self.backend {
            Backend::Uinput { mouse, .. } => {
                // Hi-res units are 1/120 notch, exactly our wire unit.
                self.wheel_rem.0 += dx;
                self.wheel_rem.1 += dy;
                let (nx, ny) = (self.wheel_rem.0 / 120, self.wheel_rem.1 / 120);
                self.wheel_rem.0 -= nx * 120;
                self.wheel_rem.1 -= ny * 120;
                Self::io(mouse.emit(&[
                    ev(EventType::RELATIVE, RelativeAxisCode::REL_WHEEL_HI_RES.0, dy),
                    ev(EventType::RELATIVE, RelativeAxisCode::REL_HWHEEL_HI_RES.0, dx),
                    ev(EventType::RELATIVE, RelativeAxisCode::REL_WHEEL.0, ny),
                    ev(EventType::RELATIVE, RelativeAxisCode::REL_HWHEEL.0, nx),
                ]))
            }
            Backend::Xtest { conn, root } => {
                self.wheel_rem.0 += dx;
                self.wheel_rem.1 += dy;
                let root = *root;
                let mut clicks = Vec::new();
                while self.wheel_rem.1 >= 120 {
                    clicks.push(4u8);
                    self.wheel_rem.1 -= 120;
                }
                while self.wheel_rem.1 <= -120 {
                    clicks.push(5);
                    self.wheel_rem.1 += 120;
                }
                while self.wheel_rem.0 >= 120 {
                    clicks.push(7);
                    self.wheel_rem.0 -= 120;
                }
                while self.wheel_rem.0 <= -120 {
                    clicks.push(6);
                    self.wheel_rem.0 += 120;
                }
                for b in clicks {
                    conn.xtest_fake_input(4, b, 0, root, 0, 0, 0).map_err(os)?;
                    conn.xtest_fake_input(5, b, 0, root, 0, 0, 0).map_err(os)?;
                }
                conn.flush().map_err(os)
            }
        }
    }

    fn key(&mut self, key: KeyCode, down: bool) -> Result<(), InputError> {
        let Some(code) = keymap::hid_to_linux(key) else {
            warn!(key = key.0, "no Linux key for HID usage");
            return Ok(());
        };
        self.pressed.key(key, down);
        match &mut self.backend {
            Backend::Uinput { keyboard, .. } => Self::io(keyboard.emit(&[ev(EventType::KEY, code, i32::from(down))])),
            Backend::Xtest { conn, root } => {
                let detail = u8::try_from(code + 8).unwrap_or(0);
                conn.xtest_fake_input(if down { 2 } else { 3 }, detail, 0, *root, 0, 0, 0).map_err(os)?;
                conn.flush().map_err(os)
            }
        }
    }
}

impl Injector for LinuxInjector {
    fn inject(&mut self, ev: &Input) -> Result<(), InputError> {
        match *ev {
            Input::MouseAbs(p) => self.move_abs(p),
            Input::MouseRel { dx, dy } => self.move_rel(dx, dy),
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

    fn set_leds(&mut self, leds: LedState) {
        // Toggle lock keys whose state differs (the state is only known on X11).
        let Ok((conn, _)) = x11() else { return };
        let now = led_state(&conn);
        for (want, have, key) in
            [(leds.caps, now.caps, keymap::hid::CAPS_LOCK), (leds.num, now.num, keymap::hid::NUM_LOCK)]
        {
            if want != have {
                let _ = self.key(KeyCode(key), true);
                let _ = self.key(KeyCode(key), false);
            }
        }
    }

    fn cursor(&self) -> Option<Point> {
        let (conn, screen) = x11().ok()?;
        let r = conn.query_pointer(conn.setup().roots[screen].root).ok()?.reply().ok()?;
        Some(Point::new(i32::from(r.root_x), i32::from(r.root_y)))
    }
}

impl Drop for LinuxInjector {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Foreground window in full screen (X11 EWMH), returns its class name.
pub fn fullscreen_app() -> Option<String> {
    let (conn, screen) = x11().ok()?;
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
    let (active, state, full) =
        (atom("_NET_ACTIVE_WINDOW")?, atom("_NET_WM_STATE")?, atom("_NET_WM_STATE_FULLSCREEN")?);
    let win = conn.get_property(false, root, active, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?.value32()?.next()?;
    if win == 0 {
        return None;
    }
    let states: Vec<u32> =
        conn.get_property(false, win, state, AtomEnum::ATOM, 0, 32).ok()?.reply().ok()?.value32()?.collect();
    if !states.contains(&full) {
        return None;
    }
    let class = conn.get_property(false, win, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256).ok()?.reply().ok()?.value;
    Some(class.split(|b| *b == 0).nth(1).map(|c| String::from_utf8_lossy(c).to_lowercase()).unwrap_or_default())
}
