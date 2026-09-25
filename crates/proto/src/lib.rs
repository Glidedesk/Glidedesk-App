//! Glidedesk wire protocol.
//!
//! Every QUIC stream carries length-prefixed [`postcard`] frames:
//! `u32 little-endian length` followed by the encoded message. Frame sizes
//! are capped per stream kind so a hostile peer cannot make us allocate
//! unbounded memory (there is no authentication — see PLAN §7).
//!
//! Streams of one connection:
//! * **control** — bidirectional, opened by the client. [`Control`] messages.
//! * **input** — unidirectional server → client. [`Input`] messages, ordered
//!   and reliable so a key-up or button-up is never lost.

#![forbid(unsafe_code)]

pub mod geom;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use geom::{Point, Rect, Side};

/// Wire protocol version. Bump on any incompatible change.
pub const PROTOCOL_VERSION: u16 = 2;
/// QUIC/TLS ALPN. Kept at 1 across protocol versions so an older peer still
/// connects far enough to be told "update Glidedesk" (`Hello.protocol` decides).
pub const ALPN: &[u8] = b"glidedesk/1";
/// Default UDP port.
pub const DEFAULT_PORT: u16 = 24_850;
/// mDNS service type.
pub const MDNS_SERVICE: &str = "_glidedesk._udp.local.";

/// Largest control frame we accept (monitor lists, settings, status).
pub const MAX_CONTROL_FRAME: usize = 256 * 1024;
/// Largest input frame (a single event is a few bytes).
pub const MAX_INPUT_FRAME: usize = 1024;

// ---------------------------------------------------------------------------
// identifiers
// ---------------------------------------------------------------------------

/// Stable random identity of an installation (not a secret).
///
/// Serialised as 32 hex digits in human-readable formats (TOML/JSON) and as
/// 16 raw bytes on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(pub [u8; 16]);

impl Serialize for DeviceId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() { s.collect_str(self) } else { self.0.serialize(s) }
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if d.is_human_readable() {
            let s = String::deserialize(d)?;
            DeviceId::parse(&s).ok_or_else(|| serde::de::Error::custom("device id must be 32 hex digits"))
        } else {
            <[u8; 16]>::deserialize(d).map(DeviceId)
        }
    }
}

impl DeviceId {
    /// Parses the 32-hex-digit form produced by `Display`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut out = [0u8; 16];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({self})")
    }
}

/// Stable identifier of one monitor on one machine
/// (EDID vendor/model/serial where available, else a position/size fingerprint).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MonitorId(pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub name: String,
    /// Bounds in the machine's native desktop coordinates.
    pub bounds: Rect,
    /// Backing scale factor (macOS) or DPI / 96 (Windows).
    pub scale: f32,
    pub primary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
    Other,
}

impl Platform {
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }
}

/// Optional capabilities a peer supports; unknown bits are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features(pub u32);

impl Features {
    pub const CLIPBOARD: u32 = 1 << 0;
    pub const FILES: u32 = 1 << 1;
    pub const LED_SYNC: u32 = 1 << 2;
    pub const DRAW_CURSOR: u32 = 1 << 3;
    pub const LOCK_SYNC: u32 = 1 << 4;

    #[must_use]
    pub const fn has(self, bit: u32) -> bool {
        self.0 & bit == bit
    }
    #[must_use]
    pub const fn with(self, bit: u32) -> Self {
        Self(self.0 | bit)
    }
}

// ---------------------------------------------------------------------------
// control stream
// ---------------------------------------------------------------------------

/// Client-side preferences sent with `Hello`. `None` = use the server's
/// per-client setting (PLAN §6.9: client local override › server per-client).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientPrefs {
    pub mouse_speed: Option<f32>,
    pub scroll_speed: Option<f32>,
    pub scroll_invert: Option<bool>,
    /// 0 = auto, 1 = none, 2 = swap Ctrl/Cmd.
    pub key_remap: Option<u8>,
    /// Local veto: both sides must allow clipboard / files.
    pub accept_clipboard: bool,
    pub accept_files: bool,
    pub draw_cursor: bool,
    pub led_sync: bool,
}

impl Default for ClientPrefs {
    fn default() -> Self {
        Self {
            mouse_speed: None,
            scroll_speed: None,
            scroll_invert: None,
            key_remap: None,
            accept_clipboard: true,
            accept_files: true,
            draw_cursor: true,
            led_sync: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u16,
    pub app_version: String,
    pub device_id: DeviceId,
    pub name: String,
    pub platform: Platform,
    pub monitors: Vec<MonitorInfo>,
    pub features: Features,
    pub prefs: ClientPrefs,
}

/// Settings the server pushes to one client (already merged with the
/// client's own local veto — see PLAN §6.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientSettings {
    pub clipboard: bool,
    pub files: bool,
    /// Draw a software cursor when the client has no pointing device.
    pub draw_cursor: bool,
    /// Sync Caps/Num/Scroll lock LEDs.
    pub led_sync: bool,
    /// Clipboard may flow server → client.
    pub clipboard_receive: bool,
    /// Clipboard may flow client → server.
    pub clipboard_send: bool,
    /// Largest clipboard payload either side sends (bytes, 0 = no limit).
    pub clipboard_limit: u64,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            clipboard: true,
            files: true,
            draw_cursor: true,
            led_sync: true,
            clipboard_receive: true,
            clipboard_send: true,
            clipboard_limit: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// data streams (unidirectional, first byte = kind)
// ---------------------------------------------------------------------------

/// First byte of every unidirectional stream.
pub mod stream_kind {
    pub const INPUT: u8 = 0;
    pub const CLIPBOARD: u8 = 1;
    pub const FILES: u8 = 2;
}

/// Hard cap for an in-memory clipboard payload (files are streamed separately).
pub const MAX_CLIPBOARD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClipFormat {
    Text,
    Html,
    Rtf,
    Png,
}

/// Header of a clipboard stream; the payloads follow back to back in `parts` order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipHeader {
    pub parts: Vec<(ClipFormat, u64)>,
    /// Present when the clipboard holds files: they follow as a file transfer.
    pub files: Option<FileSetId>,
}

/// Identifies one set of copied files for a transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileSetId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol: u16,
    pub app_version: String,
    pub device_id: DeviceId,
    pub name: String,
    pub platform: Platform,
    pub settings: ClientSettings,
    /// The server's password proof (PLAN §14.1); `None` when it has no password.
    pub auth_proof: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    IncompatibleProtocol,
    NotAllowed,
    Blocked,
    ServerStopping,
    RoleMismatch,
    /// Appended in protocol 2 (order matters on the wire).
    WrongPassword,
    PasswordRequired,
    TooManyAttempts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoodbyeReason {
    Stopping,
    Restarting,
    Quitting,
    Upgrading,
    Disconnected,
}

/// Health report piggy-backed on every pong (PLAN §6.7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientStatus {
    pub screen_locked: bool,
    pub going_to_sleep: bool,
    /// Windows: RDP session disconnected/minimised — injection will not work.
    pub session_inactive: bool,
    /// Missing OS permission (e.g. macOS Accessibility).
    pub permission_missing: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedState {
    pub caps: bool,
    pub num: bool,
    pub scroll: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Control {
    Hello(Hello),
    Welcome(Welcome),
    Reject(RejectReason),
    /// `rtt_us`: last measured round trip, so the client can show latency too.
    Ping {
        seq: u32,
        sent_us: u64,
        rtt_us: u32,
    },
    Pong {
        seq: u32,
        sent_us: u64,
        status: ClientStatus,
    },
    /// Server hands control to the client with the cursor at `pos`.
    Enter {
        pos: Point,
        leds: LedState,
    },
    /// Server takes control back.
    Leave,
    MonitorsChanged(Vec<MonitorInfo>),
    /// Client-side preferences changed (client → server).
    Prefs(ClientPrefs),
    Settings(ClientSettings),
    /// Show the big "identify screens" overlay for a few seconds.
    Identify {
        label: String,
    },
    /// The receiver's user moved (pasted-as-move) a received file set: the
    /// sender may now move the originals to the Trash (cut & paste).
    FilesTaken(FileSetId),
    Goodbye(GoodbyeReason),
    // --- protocol 2 (appended: variant order is the wire format) ---
    /// Server → client after `Hello` when a password is set: Argon2 salt and
    /// the server's SPAKE2 message.
    AuthChallenge {
        salt: Vec<u8>,
        message: Vec<u8>,
    },
    /// Client → server: its SPAKE2 message and proof (HMAC over the TLS exporter).
    AuthResponse {
        message: Vec<u8>,
        proof: Vec<u8>,
    },
}

// ---------------------------------------------------------------------------
// input stream
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Cross-platform key identifier: USB HID usage ID (page 0x07 keyboard,
/// with consumer-page media keys mapped to 0xE8.. as in `input::keymap`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KeyCode(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Input {
    /// Absolute position in the client's desktop coordinates.
    MouseAbs(Point),
    /// Relative motion (relative-mouse mode, games).
    MouseRel {
        dx: i32,
        dy: i32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    /// Scroll in 1/120-notch units (Windows `WHEEL_DELTA` convention), +y = up/away.
    Wheel {
        dx: i32,
        dy: i32,
    },
    Key {
        key: KeyCode,
        down: bool,
    },
    /// Release every key and button the client has pressed on our behalf.
    ReleaseAll,
    Leds(LedState),
}

// ---------------------------------------------------------------------------
// framing
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {size} bytes exceeds limit of {limit}")]
    TooLarge { size: usize, limit: usize },
    #[error("malformed frame: {0}")]
    Decode(#[from] postcard::Error),
}

/// Appends one length-prefixed frame to `out`.
///
/// # Errors
/// Fails if the encoded message is larger than `limit`.
pub fn encode_frame<T: Serialize>(msg: &T, limit: usize, out: &mut Vec<u8>) -> Result<(), FrameError> {
    let body = postcard::to_stdvec(msg)?;
    let size = body.len();
    if size > limit {
        return Err(FrameError::TooLarge { size, limit });
    }
    let len = u32::try_from(size).map_err(|_| FrameError::TooLarge { size, limit })?;
    out.reserve(4 + size);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(())
}

/// Validates a frame length header read from the wire.
///
/// # Errors
/// Fails if the announced size is above `limit`.
pub fn check_frame_len(header: [u8; 4], limit: usize) -> Result<usize, FrameError> {
    let size = u32::from_le_bytes(header) as usize;
    if size > limit {
        return Err(FrameError::TooLarge { size, limit });
    }
    Ok(size)
}

/// Decodes one frame body (without the length header).
///
/// # Errors
/// Fails on malformed input; trailing bytes are rejected.
pub fn decode_body<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, FrameError> {
    let (msg, rest) = postcard::take_from_bytes(body)?;
    if !rest.is_empty() {
        return Err(FrameError::Decode(postcard::Error::DeserializeBadEncoding));
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Serialize + for<'a> Deserialize<'a> + PartialEq + fmt::Debug>(msg: &T, limit: usize) {
        let mut buf = Vec::new();
        encode_frame(msg, limit, &mut buf).unwrap();
        let len = check_frame_len(buf[..4].try_into().unwrap(), limit).unwrap();
        assert_eq!(len, buf.len() - 4);
        let back: T = decode_body(&buf[4..]).unwrap();
        assert_eq!(&back, msg);
    }

    #[test]
    fn device_id_text_roundtrip() {
        let id = DeviceId([0xAB; 16]);
        assert_eq!(DeviceId::parse(&id.to_string()), Some(id));
        assert_eq!(DeviceId::parse("xyz"), None);
        assert_eq!(DeviceId::parse(&"g".repeat(32)), None);
    }

    #[test]
    fn control_messages_roundtrip() {
        let hello = Control::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            app_version: "1.0.0".into(),
            device_id: DeviceId([7; 16]),
            name: "Office-PC".into(),
            platform: Platform::Windows,
            monitors: vec![MonitorInfo {
                id: MonitorId("mon:DEL:1".into()),
                name: "Dell".into(),
                bounds: Rect::new(-1920, 0, 1920, 1080),
                scale: 1.25,
                primary: false,
            }],
            features: Features::default().with(Features::CLIPBOARD),
            prefs: ClientPrefs::default(),
        });
        roundtrip(&hello, MAX_CONTROL_FRAME);
        roundtrip(&Control::Ping { seq: 9, sent_us: 123, rtt_us: 800 }, MAX_CONTROL_FRAME);
        roundtrip(&Control::Enter { pos: Point::new(5, 6), leds: LedState::default() }, MAX_CONTROL_FRAME);
    }

    #[test]
    fn input_messages_roundtrip_and_stay_small() {
        for msg in [
            Input::MouseAbs(Point::new(-100, 2000)),
            Input::MouseRel { dx: -3, dy: 4 },
            Input::Button { button: MouseButton::Forward, down: true },
            Input::Wheel { dx: 0, dy: -120 },
            Input::Key { key: KeyCode(0x04), down: false },
            Input::ReleaseAll,
        ] {
            roundtrip(&msg, MAX_INPUT_FRAME);
            let mut buf = Vec::new();
            encode_frame(&msg, MAX_INPUT_FRAME, &mut buf).unwrap();
            assert!(buf.len() <= 16, "{msg:?} encodes to {} bytes", buf.len());
        }
    }

    #[test]
    fn oversized_frames_are_rejected_and_buffer_untouched() {
        let mut buf = vec![1, 2, 3];
        let big = Control::Identify { label: "x".repeat(100) };
        assert!(matches!(encode_frame(&big, 10, &mut buf), Err(FrameError::TooLarge { .. })));
        assert_eq!(buf, vec![1, 2, 3]);
        assert!(check_frame_len(u32::MAX.to_le_bytes(), MAX_CONTROL_FRAME).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut buf = Vec::new();
        encode_frame(&Input::ReleaseAll, MAX_INPUT_FRAME, &mut buf).unwrap();
        buf.push(0);
        assert!(decode_body::<Input>(&buf[4..]).is_err());
    }

    #[test]
    fn garbage_never_panics() {
        // Cheap fuzz: every short byte pattern must decode or error, never panic.
        for a in 0u8..=255 {
            for b in [0u8, 1, 0x7f, 0x80, 0xff] {
                let _ = decode_body::<Control>(&[a, b, b, a]);
                let _ = decode_body::<Input>(&[a, b]);
            }
        }
    }
}
