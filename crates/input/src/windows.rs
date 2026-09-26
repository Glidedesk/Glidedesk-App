//! Windows backend: low-level hooks + Raw Input capture, `SendInput`
//! injection, monitor enumeration with per-monitor DPI.
//!
//! Key events are never logged: the capture sees every keystroke on the
//! machine and only forwards them to a client while the cursor is there.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use glidedesk_proto::{Input, KeyCode, LedState, MonitorId, MonitorInfo, MouseButton, Point, Rect};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, PROCESS_PER_MONITOR_DPI_AWARE,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateCursor, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    EDD_GET_DEVICE_INTERFACE_NAME, GetCursorPos, GetMessageW, GetSystemMetrics, HC_ACTION, HCURSOR, HWND_MESSAGE,
    HWND_TOPMOST, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_UP, LWA_ALPHA, MSG, MSLLHOOKSTRUCT, PostThreadMessageW,
    RegisterClassW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_MOUSEPRESENT, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SPI_GETMOUSEKEYS, SPI_SETMOUSEKEYS, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetCursorPos, SetLayeredWindowAttributes, SetWindowPos, SetWindowsHookExW,
    ShowWindow, SystemParametersInfoW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WINDOW_EX_STYLE, WM_APP, WM_DISPLAYCHANGE, WM_INPUT, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::keymap::{self, EXT};
use crate::{CAPTURE_QUEUE, Capture, CaptureControl, CaptureEvent, Injector, InputError, Pressed};

/// Marks events we inject so our own hooks can ignore them.
const INJECT_MARKER: usize = 0x6744_6B73; // "gdks"
const WM_APP_GRAB: u32 = WM_APP + 1;
const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;
const MKF_MOUSEKEYSON: u32 = 0x1;
const MKF_AVAILABLE: u32 = 0x2;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn init_process() {
    // SAFETY: plain Win32 calls with constant arguments.
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            // Windows Server 2016 / 10 1607: V2 context not available.
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }
    }
}

// ---------------------------------------------------------------------------
// monitors
// ---------------------------------------------------------------------------

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

fn rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
}

unsafe extern "system" fn monitor_cb(hmon: HMONITOR, _hdc: HDC, _r: *mut RECT, data: LPARAM) -> windows::core::BOOL {
    // SAFETY: `data` is the `&mut Vec<MonitorInfo>` passed by `monitors()`.
    let out = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = u32::try_from(std::mem::size_of::<MONITORINFOEXW>()).unwrap_or(0);
    // SAFETY: `info` is a correctly sized MONITORINFOEXW.
    if !unsafe { GetMonitorInfoW(hmon, (&raw mut info).cast::<MONITORINFO>()) }.as_bool() {
        return true.into();
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: valid monitor handle and out-pointers.
    let _ = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &raw mut dx, &raw mut dy) };
    let device = wide_to_string(&info.szDevice);
    let mut dd = DISPLAY_DEVICEW {
        cb: u32::try_from(std::mem::size_of::<DISPLAY_DEVICEW>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: szDevice is NUL-terminated; dd is correctly sized.
    let have =
        unsafe { EnumDisplayDevicesW(PCWSTR(info.szDevice.as_ptr()), 0, &raw mut dd, EDD_GET_DEVICE_INTERFACE_NAME) }
            .as_bool();
    let bounds = rect(info.monitorInfo.rcMonitor);
    let (id, name) = if have {
        let id = wide_to_string(&dd.DeviceID);
        let name = wide_to_string(&dd.DeviceString);
        (if id.is_empty() { format!("win:{device}") } else { format!("win:{id}") }, name)
    } else {
        (format!("win:{device}@{},{}", bounds.x, bounds.y), device.clone())
    };
    #[allow(clippy::cast_precision_loss)]
    out.push(MonitorInfo {
        id: MonitorId(id),
        name: if name.is_empty() { device } else { name },
        bounds,
        scale: dx as f32 / 96.0,
        primary: info.monitorInfo.dwFlags & 1 == 1, // MONITORINFOF_PRIMARY
    });
    true.into()
}

pub fn monitors() -> Result<Vec<MonitorInfo>, InputError> {
    let mut out: Vec<MonitorInfo> = Vec::new();
    // SAFETY: the callback only writes into `out`, which outlives the call.
    let ok = unsafe { EnumDisplayMonitors(None, None, Some(monitor_cb), LPARAM((&raw mut out) as isize)) };
    if !ok.as_bool() || out.is_empty() {
        return Err(InputError::Os("EnumDisplayMonitors failed".into()));
    }
    Ok(out)
}

fn cursor_pos() -> Option<Point> {
    let mut p = POINT::default();
    // SAFETY: valid out-pointer.
    unsafe { GetCursorPos(&raw mut p) }.ok().map(|()| Point::new(p.x, p.y))
}

// ---------------------------------------------------------------------------
// capture
// ---------------------------------------------------------------------------

struct Shared {
    grabbed: AtomicBool,
    tx: mpsc::Sender<CaptureEvent>,
    thread_id: AtomicU32,
    pin: Mutex<Point>,
    /// Monitor rectangles for edge checks (refreshed on `WM_DISPLAYCHANGE`).
    rects: RwLock<Vec<Rect>>,
    gate: crate::gate::KeyGate,
}

impl Shared {
    fn emit(&self, ev: CaptureEvent) {
        if self.tx.try_send(ev).is_err() {
            debug!("capture queue full; event dropped");
        }
    }

    fn on_outer_edge(&self, p: Point) -> bool {
        let rects = self.rects.read().unwrap_or_else(PoisonError::into_inner);
        let inside = |q: Point| rects.iter().any(|r| r.contains(q));
        inside(p) && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| !inside(Point::new(p.x + dx, p.y + dy)))
    }

    fn clamp(&self, p: Point) -> Point {
        let rects = self.rects.read().unwrap_or_else(PoisonError::into_inner);
        if rects.iter().any(|r| r.contains(p)) {
            return p;
        }
        rects
            .iter()
            .map(|r| r.clamp(p))
            .min_by_key(|c| i64::from(c.x - p.x).pow(2) + i64::from(c.y - p.y).pow(2))
            .unwrap_or(p)
    }
}

/// Hook procedures have no user pointer; one capture per process.
static ACTIVE: RwLock<Option<Arc<Shared>>> = RwLock::new(None);

fn active() -> Option<Arc<Shared>> {
    ACTIVE.read().unwrap_or_else(PoisonError::into_inner).clone()
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let pass = || {
        // SAFETY: forwarding the unchanged hook arguments.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    };
    if code != i32::try_from(HC_ACTION).unwrap_or(0) {
        return pass();
    }
    let Some(shared) = active() else { return pass() };
    // SAFETY: for WH_MOUSE_LL, lparam points to a valid MSLLHOOKSTRUCT.
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    if info.dwExtraInfo == INJECT_MARKER {
        return pass();
    }
    let grabbed = shared.grabbed.load(Ordering::Relaxed);
    let hi = i32::from((info.mouseData >> 16) as i16);
    #[allow(clippy::cast_possible_truncation)]
    let msg = wparam.0 as u32;
    match msg {
        WM_MOUSEMOVE => {
            let pt = Point::new(info.pt.x, info.pt.y);
            if grabbed {
                let pin = *lock(&shared.pin);
                shared.emit(CaptureEvent::Motion { pos: pin, dx: pt.x - pin.x, dy: pt.y - pin.y });
            } else {
                let old = cursor_pos().unwrap_or(pt);
                let pos = shared.clamp(pt);
                shared.emit(CaptureEvent::Motion { pos, dx: pt.x - old.x, dy: pt.y - old.y });
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP => {
            shared.emit(CaptureEvent::Button { button: MouseButton::Left, down: msg == WM_LBUTTONDOWN });
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP => {
            shared.emit(CaptureEvent::Button { button: MouseButton::Right, down: msg == WM_RBUTTONDOWN });
        }
        WM_MBUTTONDOWN | WM_MBUTTONUP => {
            shared.emit(CaptureEvent::Button { button: MouseButton::Middle, down: msg == WM_MBUTTONDOWN });
        }
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let button = if hi == 2 { MouseButton::Forward } else { MouseButton::Back };
            shared.emit(CaptureEvent::Button { button, down: msg == WM_XBUTTONDOWN });
        }
        WM_MOUSEWHEEL => shared.emit(CaptureEvent::Wheel { dx: 0, dy: hi }),
        WM_MOUSEHWHEEL => shared.emit(CaptureEvent::Wheel { dx: hi, dy: 0 }),
        _ => return pass(),
    }
    if grabbed { LRESULT(1) } else { pass() }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let pass = || {
        // SAFETY: forwarding the unchanged hook arguments.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    };
    if code != i32::try_from(HC_ACTION).unwrap_or(0) {
        return pass();
    }
    let Some(shared) = active() else { return pass() };
    // SAFETY: for WH_KEYBOARD_LL, lparam points to a valid KBDLLHOOKSTRUCT.
    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    if info.dwExtraInfo == INJECT_MARKER {
        return pass();
    }
    #[allow(clippy::cast_possible_truncation)]
    let msg = wparam.0 as u32;
    let down = (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) && info.flags.0 & LLKHF_UP.0 == 0;
    let extended = info.flags.0 & LLKHF_EXTENDED.0 != 0;
    let scan = u16::try_from(info.scanCode & 0xFF).unwrap_or(0);
    let key = keymap::win_vk_to_hid(info.vkCode)
        .filter(|_| scan == 0 || matches!(info.vkCode, 0x13 | 0x90))
        .or_else(|| keymap::win_to_hid(scan, extended));
    let grabbed = shared.grabbed.load(Ordering::Relaxed);
    let swallow = match key {
        Some(key) => {
            shared.emit(CaptureEvent::Key { key, down });
            let out = shared.gate.on_key(key, down, grabbed);
            if out.paste {
                shared.emit(CaptureEvent::PasteRequested { key });
            }
            out.swallow
        }
        None => grabbed,
    };
    if swallow { LRESULT(1) } else { pass() }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            if let Some(shared) = active() {
                let grabbed = shared.grabbed.load(Ordering::Relaxed);
                let mut raw = RAWINPUT::default();
                let mut size = u32::try_from(std::mem::size_of::<RAWINPUT>()).unwrap_or(0);
                let header = u32::try_from(std::mem::size_of::<RAWINPUTHEADER>()).unwrap_or(0);
                // SAFETY: buffer is a RAWINPUT of `size` bytes.
                let got = unsafe {
                    GetRawInputData(
                        HRAWINPUT(lparam.0 as _),
                        RID_INPUT,
                        Some((&raw mut raw).cast()),
                        &raw mut size,
                        header,
                    )
                };
                if got != u32::MAX && raw.header.dwType == RIM_TYPEMOUSE.0 {
                    // SAFETY: dwType says the union holds mouse data.
                    let m = unsafe { raw.data.mouse };
                    let relative = m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 == 0;
                    let moved = relative && (m.lLastX != 0 || m.lLastY != 0);
                    if moved && grabbed {
                        // The hook normally keeps the cursor on the pin. If it moved,
                        // the hook didn't see this input (an elevated window is in
                        // front, or Windows dropped a slow hook): forward the raw
                        // motion and pull the cursor back, so it can't wander on this PC.
                        let pin = *lock(&shared.pin);
                        if cursor_pos().is_some_and(|p| p != pin) {
                            shared.emit(CaptureEvent::Motion { pos: pin, dx: m.lLastX, dy: m.lLastY });
                            // SAFETY: plain Win32 call.
                            let _ = unsafe { SetCursorPos(pin.x, pin.y) };
                        }
                    } else if moved
                        && let Some(pos) = cursor_pos()
                        && shared.on_outer_edge(pos)
                    {
                        // The cursor is clamped at an edge: report the raw push.
                        shared.emit(CaptureEvent::Motion { pos, dx: m.lLastX.signum(), dy: m.lLastY.signum() });
                    }
                }
            }
            // SAFETY: default processing releases the raw input buffer.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DISPLAYCHANGE => {
            if let Some(shared) = active() {
                if let Ok(m) = monitors() {
                    *shared.rects.write().unwrap_or_else(PoisonError::into_inner) =
                        m.iter().map(|m| m.bounds).collect();
                }
                shared.emit(CaptureEvent::DisplaysChanged);
            }
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Fully transparent cursor for the veil window.
fn blank_cursor(instance: HINSTANCE) -> Option<HCURSOR> {
    let and_mask = [0xFFu8; 32 * 4];
    let xor_mask = [0u8; 32 * 4];
    // SAFETY: masks are 32x32 1-bpp bitmaps.
    unsafe { CreateCursor(Some(instance), 0, 0, 32, 32, and_mask.as_ptr().cast(), xor_mask.as_ptr().cast()) }.ok()
}

#[derive(Clone)]
struct Control(Arc<Shared>);

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WinCapture").field("grabbed", &self.0.grabbed.load(Ordering::Relaxed)).finish()
    }
}

impl CaptureControl for Control {
    fn set_grab(&self, grab: bool) {
        if grab {
            self.0.gate.on_grab();
            // Park the (hidden) cursor in the middle of the main screen: deltas
            // are measured against this point and can never be clamped by a
            // screen edge while another computer has control.
            let centre = monitors()
                .ok()
                .and_then(|m| m.into_iter().find(|m| m.primary))
                .map(|m| Point::new(m.bounds.x + m.bounds.w / 2, m.bounds.y + m.bounds.h / 2))
                .or_else(cursor_pos);
            if let Some(p) = centre {
                *lock(&self.0.pin) = p;
            }
        }
        if self.0.grabbed.swap(grab, Ordering::SeqCst) != grab {
            // The veil window belongs to the capture thread.
            // SAFETY: posting a message to our own thread id.
            let _ = unsafe {
                PostThreadMessageW(
                    self.0.thread_id.load(Ordering::SeqCst),
                    WM_APP_GRAB,
                    WPARAM(usize::from(grab)),
                    LPARAM(0),
                )
            };
        }
    }

    fn warp(&self, p: Point) {
        // SAFETY: plain Win32 call.
        let _ = unsafe { SetCursorPos(p.x, p.y) };
    }

    fn cursor(&self) -> Option<Point> {
        cursor_pos()
    }

    fn leds(&self) -> LedState {
        // SAFETY: GetKeyState is always safe to call.
        let on = |vk: i32| unsafe { GetKeyState(vk) } & 1 == 1;
        LedState { caps: on(0x14), num: on(0x90), scroll: on(0x91) }
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
        // SAFETY: posting WM_QUIT to the capture thread.
        let _ = unsafe { PostThreadMessageW(self.0.thread_id.load(Ordering::SeqCst), WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

fn capture_thread(shared: &Arc<Shared>, ready: &std::sync::mpsc::Sender<Result<(), InputError>>) {
    // SAFETY: Win32 setup on this thread; every handle is released below.
    unsafe {
        shared.thread_id.store(GetCurrentThreadId(), Ordering::SeqCst);
        let Ok(module) = GetModuleHandleW(None) else {
            let _ = ready.send(Err(InputError::Os("GetModuleHandle failed".into())));
            return;
        };
        let instance = HINSTANCE(module.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("GlidedeskCapture"),
            hCursor: blank_cursor(instance).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&raw const class);
        // Message-only window for Raw Input and display-change messages.
        let msg_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("GlidedeskCapture"),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
        .ok();
        // Tiny, nearly invisible topmost window placed under the pinned cursor
        // while grabbed: its blank class cursor hides the pointer.
        let veil = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("GlidedeskCapture"),
            w!(""),
            WS_POPUP,
            0,
            0,
            16,
            16,
            None,
            None,
            Some(instance),
            None,
        )
        .ok();
        if let Some(v) = veil {
            let _ = SetLayeredWindowAttributes(v, windows::Win32::Foundation::COLORREF(0), 1, LWA_ALPHA);
        }
        if let Some(h) = msg_hwnd {
            let dev = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x02, dwFlags: RIDEV_INPUTSINK, hwndTarget: h };
            let _ = RegisterRawInputDevices(&[dev], u32::try_from(std::mem::size_of::<RAWINPUTDEVICE>()).unwrap_or(0));
        }
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance), 0);
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance), 0);
        let (Ok(mut mouse), Ok(mut keyboard)) = (mouse, keyboard) else {
            let _ = ready.send(Err(InputError::Os("SetWindowsHookEx failed".into())));
            return;
        };
        let _ = ready.send(Ok(()));

        let mut msg = MSG::default();
        while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.is_invalid() && msg.message == WM_APP_GRAB {
                if msg.wParam.0 == 1 {
                    // Hooks run newest first. Mouse software (Logitech Options+, …)
                    // started after Glidedesk would see an external mouse's extra
                    // buttons before our hook swallows them and act on them here as
                    // well. Reinstalled, ours is in front again (and back, should
                    // Windows have dropped it as too slow). No hook runs before this
                    // handler returns, so no event reaches both the old and the new one.
                    if let Ok(h) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance), 0) {
                        let _ = UnhookWindowsHookEx(std::mem::replace(&mut mouse, h));
                    }
                    if let Ok(h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance), 0) {
                        let _ = UnhookWindowsHookEx(std::mem::replace(&mut keyboard, h));
                    }
                }
                if let Some(v) = veil {
                    if msg.wParam.0 == 1 {
                        let p = *lock(&shared.pin);
                        let _ = SetWindowPos(v, Some(HWND_TOPMOST), p.x - 8, p.y - 8, 16, 16, SWP_NOACTIVATE);
                        let _ = ShowWindow(v, SW_SHOWNOACTIVATE);
                        // Nudge so Windows re-evaluates the cursor shape over the veil.
                        let _ = SetCursorPos(p.x, p.y);
                    } else {
                        let _ = ShowWindow(v, SW_HIDE);
                    }
                }
                continue;
            }
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
        let _ = UnhookWindowsHookEx(mouse);
        let _ = UnhookWindowsHookEx(keyboard);
        if let Some(v) = veil {
            let _ = DestroyWindow(v);
        }
        if let Some(h) = msg_hwnd {
            let _ = DestroyWindow(h);
        }
    }
    debug!("capture thread stopped");
}

pub fn start_capture() -> Result<Capture, InputError> {
    let (tx, rx) = mpsc::channel(CAPTURE_QUEUE);
    let rects = monitors()?.iter().map(|m| m.bounds).collect();
    let shared = Arc::new(Shared {
        grabbed: AtomicBool::new(false),
        tx,
        thread_id: AtomicU32::new(0),
        pin: Mutex::new(Point::default()),
        rects: RwLock::new(rects),
        gate: crate::gate::KeyGate::default(),
    });
    *ACTIVE.write().unwrap_or_else(PoisonError::into_inner) = Some(shared.clone());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread_shared = shared.clone();
    std::thread::Builder::new()
        .name("gd-capture".into())
        .spawn(move || {
            capture_thread(&thread_shared, &ready_tx);
            let mut slot = ACTIVE.write().unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, &thread_shared)) {
                *slot = None;
            }
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

#[derive(Debug)]
struct WinInjector {
    pressed: Pressed,
    /// Mouse Keys settings to restore on drop, when we turned the feature on.
    mouse_keys_restore: Option<MouseKeys>,
}

#[allow(clippy::unnecessary_wraps)] // same signature on every platform
pub fn injector() -> Result<Box<dyn Injector>, InputError> {
    Ok(Box::new(WinInjector { pressed: Pressed::default(), mouse_keys_restore: ensure_cursor_visible() }))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct MouseKeys {
    cb_size: u32,
    dw_flags: u32,
    i_max_speed: u32,
    i_time_to_max_speed: u32,
    i_ctrl_speed: u32,
    dw_reserved1: u32,
    dw_reserved2: u32,
}

/// Windows hides the pointer when no mouse is attached (servers, VMs).
/// Turning on Mouse Keys makes it visible again (same trick as Mouse Without Borders).
/// Returns the previous settings when Mouse Keys was switched on (to restore later).
fn ensure_cursor_visible() -> Option<MouseKeys> {
    // SAFETY: plain Win32 calls with a correctly sized MOUSEKEYS struct.
    unsafe {
        if GetSystemMetrics(SM_MOUSEPRESENT) != 0 {
            return None;
        }
        let size = u32::try_from(std::mem::size_of::<MouseKeys>()).unwrap_or(0);
        let mut mk = MouseKeys { cb_size: size, ..Default::default() };
        if SystemParametersInfoW(
            SPI_GETMOUSEKEYS,
            size,
            Some((&raw mut mk).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
        .is_err()
        {
            return None;
        }
        if mk.dw_flags & MKF_MOUSEKEYSON != 0 {
            return None;
        }
        let previous = mk;
        mk.dw_flags |= MKF_MOUSEKEYSON | MKF_AVAILABLE;
        SystemParametersInfoW(
            SPI_SETMOUSEKEYS,
            size,
            Some((&raw mut mk).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
        .is_ok()
        .then_some(previous)
    }
}

fn send(inputs: &[INPUT]) -> Result<(), InputError> {
    // SAFETY: slice of fully initialised INPUT structs.
    let n = unsafe { SendInput(inputs, i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(0)) };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        Err(InputError::Os("SendInput was blocked (UIPI or secure desktop)".into()))
    }
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: INJECT_MARKER },
        },
    }
}

fn keyboard(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: INJECT_MARKER },
        },
    }
}

impl WinInjector {
    fn move_abs(p: Point) -> Result<(), InputError> {
        // SAFETY: plain metric queries.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
            )
        };
        // Normalise so that Windows' floor(n * w / 65536) lands exactly on the pixel.
        let norm = |v: i32, origin: i32, size: i32| {
            let rel = i64::from((v - origin).clamp(0, size - 1));
            i32::try_from((rel * 65_536 + i64::from(size) - 1) / i64::from(size)).unwrap_or(0)
        };
        send(&[mouse(
            norm(p.x, vx, vw),
            norm(p.y, vy, vh),
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )])
    }

    fn button(&mut self, b: MouseButton, down: bool) -> Result<(), InputError> {
        self.pressed.button(b, down);
        let (flags, data) = match (b, down) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        };
        send(&[mouse(0, 0, data, flags)])
    }

    fn key(&mut self, key: KeyCode, down: bool) -> Result<(), InputError> {
        let up = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
        let input = if let Some(vk) = keymap::win_vk_only(key) {
            let ext = if vk == 0x90 { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
            keyboard(vk, 0, up | ext)
        } else if let Some(scan) = keymap::hid_to_win(key) {
            let ext = if scan & EXT == EXT { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
            keyboard(0, scan & 0xFF, KEYEVENTF_SCANCODE | ext | up)
        } else {
            warn!(key = key.0, "no Windows key for HID usage");
            return Ok(());
        };
        self.pressed.key(key, down);
        send(&[input])
    }

    fn toggle_lock(vk: u16, want: bool) {
        // SAFETY: GetKeyState is always safe to call.
        let on = unsafe { GetKeyState(i32::from(vk)) } & 1 == 1;
        if on != want {
            let ext = if vk == 0x90 { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
            let _ = send(&[keyboard(vk, 0, ext), keyboard(vk, 0, ext | KEYEVENTF_KEYUP)]);
        }
    }
}

impl Injector for WinInjector {
    #[allow(clippy::cast_sign_loss)]
    fn inject(&mut self, ev: &Input) -> Result<(), InputError> {
        match *ev {
            Input::MouseAbs(p) => Self::move_abs(p),
            Input::MouseRel { dx, dy } => send(&[mouse(dx, dy, 0, MOUSEEVENTF_MOVE)]),
            Input::Button { button, down } => self.button(button, down),
            Input::Wheel { dx, dy } => {
                if dy != 0 {
                    send(&[mouse(0, 0, dy as u32, MOUSEEVENTF_WHEEL)])?;
                }
                if dx != 0 {
                    send(&[mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL)])?;
                }
                Ok(())
            }
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
        Self::toggle_lock(0x14, leds.caps); // VK_CAPITAL
        Self::toggle_lock(0x90, leds.num); // VK_NUMLOCK
        Self::toggle_lock(0x91, leds.scroll); // VK_SCROLL
    }

    fn cursor(&self) -> Option<Point> {
        cursor_pos()
    }
}

impl Drop for WinInjector {
    fn drop(&mut self) {
        self.release_all();
        if let Some(mut mk) = self.mouse_keys_restore.take() {
            let size = mk.cb_size;
            // SAFETY: correctly sized MOUSEKEYS struct saved earlier.
            let res = unsafe {
                SystemParametersInfoW(
                    SPI_SETMOUSEKEYS,
                    size,
                    Some((&raw mut mk).cast()),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
                )
            };
            if res.is_err() {
                warn!("could not restore the Mouse Keys setting");
            }
        }
    }
}

/// Foreground window covering its whole monitor (games, presentations, video).
pub fn fullscreen_app() -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
    };

    // SAFETY: plain Win32 queries on the foreground window; the process handle is closed.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut class = [0u16; 64];
        let n = usize::try_from(GetClassNameW(hwnd, &mut class)).unwrap_or(0);
        let class = String::from_utf16_lossy(&class[..n]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return None;
        }
        let mut wr = RECT::default();
        GetWindowRect(hwnd, &raw mut wr).ok()?;
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
            ..Default::default()
        };
        if !GetMonitorInfoW(mon, &raw mut info).as_bool() {
            return None;
        }
        let m = info.rcMonitor;
        if !(wr.left <= m.left && wr.top <= m.top && wr.right >= m.right && wr.bottom >= m.bottom) {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
        let mut name = String::new();
        if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = [0u16; 520];
            let mut len = u32::try_from(buf.len()).unwrap_or(0);
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &raw mut len)
                .is_ok()
            {
                let full = String::from_utf16_lossy(&buf[..len as usize]);
                name = full.rsplit('\\').next().unwrap_or("").to_lowercase();
            }
            let _ = CloseHandle(h);
        }
        Some(name)
    }
}
