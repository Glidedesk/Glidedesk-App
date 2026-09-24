//! Windows clipboard. `SetClipboardData` needs an owner window, and Windows
//! sends that window messages synchronously, so all clipboard work happens on
//! one dedicated thread that owns a message-only window and pumps messages.

#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PostThreadMessageW,
    RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WNDCLASSW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::convert::{cf_html_to_html, dib_to_png, hdrop_to_paths, html_to_cf_html, paths_to_hdrop, png_to_dib};
use crate::{ClipData, ClipError, Clipboard, within};

const WM_APP_WORK: u32 = WM_APP + 7;
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

enum Job {
    Read(u64, mpsc::Sender<Result<ClipData, ClipError>>),
    Write(Box<ClipData>, mpsc::Sender<Result<(), ClipError>>),
}

#[derive(Debug)]
pub struct WinClipboard {
    jobs: mpsc::Sender<Job>,
    thread_id: u32,
}

struct Formats {
    html: u32,
    rtf: u32,
    png: u32,
    effect: u32,
}

fn register(name: PCWSTR) -> u32 {
    // SAFETY: constant, NUL-terminated format name.
    unsafe { RegisterClipboardFormatW(name) }
}

impl WinClipboard {
    pub fn start() -> Result<Self, ClipError> {
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, ClipError>>();
        std::thread::Builder::new()
            .name("gd-clipboard".into())
            .spawn(move || worker(&jobs_rx, &ready_tx))
            .map_err(|e| ClipError::Os(e.to_string()))?;
        let thread_id = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| ClipError::Os("clipboard thread did not start".into()))??;
        Ok(Self { jobs: jobs_tx, thread_id })
    }

    fn submit(&self, job: Job) -> Result<(), ClipError> {
        self.jobs.send(job).map_err(|_| ClipError::Os("clipboard thread stopped".into()))?;
        // SAFETY: wake the worker's GetMessage loop.
        unsafe { PostThreadMessageW(self.thread_id, WM_APP_WORK, WPARAM(0), LPARAM(0)) }
            .map_err(|e| ClipError::Os(e.to_string()))
    }
}

impl Clipboard for WinClipboard {
    fn sequence(&self) -> u64 {
        // SAFETY: thread-safe query.
        u64::from(unsafe { GetClipboardSequenceNumber() })
    }

    fn read(&mut self, limit: u64) -> Result<ClipData, ClipError> {
        let (tx, rx) = mpsc::channel();
        self.submit(Job::Read(limit, tx))?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| ClipError::Busy)?
    }

    fn write(&mut self, data: &ClipData) -> Result<(), ClipError> {
        let (tx, rx) = mpsc::channel();
        self.submit(Job::Write(Box::new(data.clone()), tx))?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| ClipError::Busy)?
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: default handling.
    unsafe { DefWindowProcW(h, m, w, l) }
}

fn worker(jobs: &mpsc::Receiver<Job>, ready: &mpsc::Sender<Result<u32, ClipError>>) {
    // SAFETY: window creation on this thread; the window lives as long as the loop.
    let hwnd = unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            let _ = ready.send(Err(ClipError::Os("GetModuleHandle".into())));
            return;
        };
        let instance = HINSTANCE(module.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: w!("GlidedeskClipboard"),
            ..Default::default()
        };
        RegisterClassW(&raw const class);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("GlidedeskClipboard"),
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
    };
    let Ok(hwnd) = hwnd else {
        let _ = ready.send(Err(ClipError::Os("cannot create clipboard window".into())));
        return;
    };
    let formats = Formats {
        html: register(w!("HTML Format")),
        rtf: register(w!("Rich Text Format")),
        png: register(w!("PNG")),
        effect: register(w!("Preferred DropEffect")),
    };
    // SAFETY: plain call.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let mut msg = MSG::default();
    // SAFETY: standard message loop on the owning thread.
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.as_bool() {
        while let Ok(job) = jobs.try_recv() {
            match job {
                Job::Read(limit, tx) => {
                    let _ = tx.send(with_open(hwnd, || read_all(&formats, limit)));
                }
                Job::Write(data, tx) => {
                    let _ = tx.send(with_open(hwnd, || write_all(&formats, &data)));
                }
            }
        }
        // SAFETY: standard dispatch.
        unsafe {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

/// Opens the clipboard (retrying while another app holds it) around `f`.
fn with_open<T>(hwnd: HWND, f: impl FnOnce() -> Result<T, ClipError>) -> Result<T, ClipError> {
    let mut opened = false;
    for _ in 0..20 {
        // SAFETY: our own window handle.
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if !opened {
        return Err(ClipError::Busy);
    }
    let r = f();
    // SAFETY: we opened it above.
    let _ = unsafe { CloseClipboard() };
    r
}

fn available(fmt: u32) -> bool {
    // SAFETY: plain query while the clipboard is open.
    unsafe { IsClipboardFormatAvailable(fmt) }.is_ok()
}

/// Copies the bytes of a clipboard HGLOBAL (at most `limit`, 0 = any).
fn get_bytes(fmt: u32, limit: u64) -> Option<Vec<u8>> {
    if !available(fmt) {
        return None;
    }
    // SAFETY: the clipboard is open; the handle stays valid until CloseClipboard.
    unsafe {
        let h = GetClipboardData(fmt).ok()?;
        let g = HGLOBAL(h.0);
        let size = GlobalSize(g);
        if size == 0 || !within(limit, size) {
            return None;
        }
        let p = GlobalLock(g);
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

#[allow(clippy::unnecessary_wraps)] // same shape as write_all for `with_open`
fn read_all(f: &Formats, limit: u64) -> Result<ClipData, ClipError> {
    let mut out = ClipData::default();
    if let Some(b) = get_bytes(u32::from(CF_HDROP.0), 0)
        && let Ok(paths) = hdrop_to_paths(&b)
    {
        out.files = paths.into_iter().map(PathBuf::from).collect();
        out.cut = get_bytes(f.effect, 0)
            .and_then(|e| e.get(..4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])))
            .is_some_and(|e| e & DROPEFFECT_MOVE != 0);
        return Ok(out);
    }
    if let Some(b) = get_bytes(u32::from(CF_UNICODETEXT.0), limit.saturating_mul(2)) {
        let units: Vec<u16> =
            b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|u| *u != 0).collect();
        out.text = Some(String::from_utf16_lossy(&units));
    }
    if let Some(b) = get_bytes(f.html, limit) {
        out.html = cf_html_to_html(&b).ok();
    }
    if let Some(mut b) = get_bytes(f.rtf, limit) {
        if let Some(end) = b.iter().position(|x| *x == 0) {
            b.truncate(end);
        }
        out.rtf = Some(b);
    }
    if let Some(b) = get_bytes(f.png, limit) {
        out.png = Some(b);
    } else if let Some(b) = get_bytes(u32::from(CF_DIBV5.0), limit).or_else(|| get_bytes(u32::from(CF_DIB.0), limit)) {
        out.png = dib_to_png(&b).ok();
    }
    Ok(out)
}

/// Hands one buffer to the clipboard (the system owns it on success).
fn put(fmt: u32, bytes: &[u8]) -> Result<(), ClipError> {
    // SAFETY: allocate, fill, unlock, then transfer ownership; freed on failure.
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| ClipError::Os(e.to_string()))?;
        let p = GlobalLock(g);
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return Err(ClipError::Os("GlobalLock failed".into()));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(g);
        if SetClipboardData(fmt, Some(HANDLE(g.0))).is_err() {
            let _ = GlobalFree(Some(g));
            return Err(ClipError::Os("SetClipboardData failed".into()));
        }
    }
    Ok(())
}

fn write_all(f: &Formats, data: &ClipData) -> Result<(), ClipError> {
    // SAFETY: the clipboard is open with our window as owner.
    unsafe { EmptyClipboard() }.map_err(|e| ClipError::Os(e.to_string()))?;
    if !data.files.is_empty() {
        let paths: Vec<String> = data.files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        put(u32::from(CF_HDROP.0), &paths_to_hdrop(&paths))?;
        let effect = if data.cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
        put(f.effect, &effect.to_le_bytes())?;
        return Ok(());
    }
    if let Some(t) = &data.text {
        let mut wide: Vec<u8> = t.encode_utf16().flat_map(u16::to_le_bytes).collect();
        wide.extend_from_slice(&[0, 0]);
        put(u32::from(CF_UNICODETEXT.0), &wide)?;
    }
    if let Some(h) = &data.html {
        let mut b = html_to_cf_html(h);
        b.push(0);
        put(f.html, &b)?;
    }
    if let Some(r) = &data.rtf {
        let mut b = r.clone();
        b.push(0);
        put(f.rtf, &b)?;
    }
    if let Some(p) = &data.png {
        put(f.png, p)?;
        // Classic apps (Paint, Office) read CF_DIB.
        if let Ok(dib) = png_to_dib(p) {
            put(u32::from(CF_DIB.0), &dib)?;
        }
    }
    Ok(())
}
