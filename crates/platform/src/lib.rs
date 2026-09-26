//! Small OS helpers the agent needs: host name, log folder, session/lock
//! state for heartbeats, Wake-on-LAN.

#![cfg_attr(not(any(target_os = "macos", windows)), forbid(unsafe_code))]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;

use nexpingdesk_proto::ClientStatus;

/// Friendly machine name ("Office-PC"), without a `.local` suffix.
#[must_use]
pub fn host_name() -> String {
    let raw = gethostname::gethostname().to_string_lossy().into_owned();
    let name = raw.strip_suffix(".local").unwrap_or(&raw).trim().to_owned();
    if name.is_empty() { "Computer".into() } else { name }
}

/// Per-user log folder (created on demand by the logger).
#[must_use]
pub fn log_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("NEXPINGDESK_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(home).join("logs");
    }
    let base = directories::BaseDirs::new();
    #[cfg(target_os = "macos")]
    let dir = base.map(|b| b.home_dir().join("Library/Logs/Nexpingdesk"));
    #[cfg(windows)]
    let dir = base.map(|b| b.data_local_dir().join("Nexpingdesk").join("logs"));
    #[cfg(not(any(target_os = "macos", windows)))]
    let dir = base.map(|b| b.data_local_dir().join("nexpingdesk").join("logs"));
    dir.unwrap_or_else(|| std::env::temp_dir().join("nexpingdesk-logs"))
}

/// Lock / sleep / remote-session state reported in every pong.
#[must_use]
pub fn session_status() -> ClientStatus {
    #[cfg(target_os = "macos")]
    return macos::session_status();
    #[cfg(windows)]
    return windows::session_status();
    #[allow(unreachable_code)]
    ClientStatus::default()
}

/// Moves a file or folder to the Trash / Recycle Bin (recoverable; used for
/// cut & paste — originals are never deleted permanently).
pub fn move_to_trash(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::trash(path);
    #[cfg(windows)]
    return windows::trash(path);
    #[cfg(target_os = "linux")]
    {
        // freedesktop Trash through the desktop's own tool (GNOME/GTK, then KDE).
        for (cmd, args) in [("gio", vec!["trash"]), ("kioclient6", vec!["move"]), ("kioclient5", vec!["move"])] {
            let mut c = std::process::Command::new(cmd);
            c.args(&args).arg(path);
            if args[0] == "move" {
                c.arg("trash:/");
            }
            if c.status().is_ok_and(|s| s.success()) {
                return Ok(());
            }
        }
        return Err("could not move to the Trash (install gio or kioclient)".into());
    }
    #[allow(unreachable_code)]
    {
        let _ = path;
        Err("moving to the trash is not supported on this platform".into())
    }
}

/// Parses `aa:bb:cc:dd:ee:ff` / `aa-bb-…`.
#[must_use]
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let mut out = [0u8; 6];
    let mut parts = s.split([':', '-']);
    for b in &mut out {
        *b = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// Sends a Wake-on-LAN magic packet (UDP broadcast, ports 9 and 7).
pub fn wake_on_lan(mac: [u8; 6]) -> std::io::Result<()> {
    let mut packet = [0xFFu8; 102];
    for i in 0..16 {
        packet[6 + i * 6..12 + i * 6].copy_from_slice(&mac);
    }
    let sock = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))?;
    sock.set_broadcast(true)?;
    for port in [9, 7] {
        sock.send_to(&packet, SocketAddr::from((Ipv4Addr::BROADCAST, port)))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use nexpingdesk_proto::ClientStatus;
    use objc2_core_foundation::{CFBoolean, CFString, CFType};
    use objc2_core_graphics::CGSessionCopyCurrentDictionary;

    fn flag(key: &str) -> bool {
        let Some(dict) = CGSessionCopyCurrentDictionary() else { return false };
        let key = CFString::from_str(key);
        // SAFETY: key is a valid CFString; the returned value is borrowed from `dict`.
        let v = unsafe { dict.value((&raw const *key).cast()) };
        if v.is_null() {
            return false;
        }
        // SAFETY: non-null CF object owned by `dict`, which outlives this use.
        let obj: &CFType = unsafe { &*v.cast::<CFType>() };
        obj.downcast_ref::<CFBoolean>().is_some_and(CFBoolean::as_bool)
    }

    pub fn trash(path: &std::path::Path) -> Result<(), String> {
        use objc2_foundation::{NSFileManager, NSString, NSURL};
        // Its own autorelease pool: called on worker threads that have none.
        objc2::rc::autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None).map_err(|e| e.to_string())
        })
    }

    pub fn session_status() -> ClientStatus {
        ClientStatus {
            screen_locked: flag("CGSSessionScreenIsLocked"),
            // Fast user switching: our session is not the one on screen.
            session_inactive: !flag("kCGSSessionOnConsoleKey"),
            ..ClientStatus::default()
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use nexpingdesk_proto::ClientStatus;
    use windows::Win32::System::RemoteDesktop::{
        WTS_CONNECTSTATE_CLASS, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSActive, WTSConnectState,
        WTSFreeMemory, WTSQuerySessionInformationW,
    };
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_SWITCHDESKTOP, OpenInputDesktop,
    };
    use windows::core::PWSTR;

    /// The input desktop is not ours (lock screen / UAC secure desktop).
    fn input_desktop_blocked() -> bool {
        // SAFETY: plain Win32 calls; the handle is closed below.
        unsafe {
            match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) {
                Ok(h) => {
                    let _ = CloseDesktop(h);
                    false
                }
                Err(_) => true,
            }
        }
    }

    /// RDP session disconnected/minimised: injection has no effect.
    fn session_active() -> bool {
        let mut buf = PWSTR::null();
        let mut len = 0u32;
        // SAFETY: WTS allocates `buf`; freed with WTSFreeMemory.
        unsafe {
            if WTSQuerySessionInformationW(
                Some(WTS_CURRENT_SERVER_HANDLE),
                WTS_CURRENT_SESSION,
                WTSConnectState,
                &raw mut buf,
                &raw mut len,
            )
            .is_err()
                || buf.is_null()
            {
                return true;
            }
            // The buffer holds one WTS_CONNECTSTATE_CLASS (i32); copy it byte-wise
            // because the returned pointer is only guaranteed 2-byte aligned.
            let mut raw = [0u8; 4];
            std::ptr::copy_nonoverlapping(buf.0.cast::<u8>(), raw.as_mut_ptr(), (len as usize).min(4));
            WTSFreeMemory(buf.0.cast());
            WTS_CONNECTSTATE_CLASS(i32::from_ne_bytes(raw)) == WTSActive
        }
    }

    pub fn trash(path: &std::path::Path) -> Result<(), String> {
        use windows::Win32::UI::Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW,
        };
        // Double-NUL-terminated path list.
        let mut from: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        from.extend_from_slice(&[0, 0]);
        let mut op = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: windows::core::PCWSTR(from.as_ptr()),
            fFlags: u16::try_from((FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: `from` outlives the call; the struct is fully initialised.
        let rc = unsafe { SHFileOperationW(&raw mut op) };
        if rc == 0 && !op.fAnyOperationsAborted.as_bool() {
            Ok(())
        } else {
            Err(format!("moving to the Recycle Bin failed ({rc})"))
        }
    }

    pub fn session_status() -> ClientStatus {
        ClientStatus {
            screen_locked: input_desktop_blocked(),
            session_inactive: !session_active(),
            ..ClientStatus::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_parsing() {
        assert_eq!(parse_mac("aa:bb:cc:dd:ee:0f"), Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x0f]));
        assert_eq!(parse_mac("AA-BB-CC-DD-EE-FF"), Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
        assert_eq!(parse_mac("aa:bb"), None);
        assert_eq!(parse_mac("aa:bb:cc:dd:ee:ff:00"), None);
    }

    #[test]
    fn host_name_is_not_empty() {
        assert!(!host_name().is_empty());
        assert!(!log_dir().as_os_str().is_empty());
    }
}
