//! `config.toml` schema (PLAN §6.5, §6.9).
//!
//! Every field has a default, so older files load and new options appear
//! automatically. Unknown keys from newer versions are ignored on load.

use std::net::IpAddr;
use std::path::PathBuf;

use glidedesk_layout::{LinkSpec, SwitchPolicy};
use glidedesk_proto::{DEFAULT_PORT, DeviceId};
use serde::{Deserialize, Serialize};

/// Current schema version written by this build.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub device: Device,
    pub general: General,
    pub server: Server,
    pub client: Client,
    pub layout: LayoutConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            device: Device::default(),
            general: General::default(),
            server: Server::default(),
            client: Client::default(),
            layout: LayoutConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// First run: the wizard asks.
    #[default]
    Unset,
    Server,
    Client,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
    /// Stable identity of this installation; generated on first run.
    pub id: DeviceId,
    /// Display name; empty = host name.
    pub name: String,
    pub role: Role,
}

impl Default for Device {
    fn default() -> Self {
        Self { id: DeviceId([0; 16]), name: String::new(), role: Role::Unset }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub start_at_login: bool,
    pub notifications: bool,
    pub theme: Theme,
    /// BCP-47 tag or "system".
    pub language: String,
    pub log_level: LogLevel,
    /// Ask before "Quit" stops input sharing.
    pub confirm_quit: bool,
    /// Check for updates in the private release location.
    pub auto_update: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            start_at_login: true,
            notifications: true,
            theme: Theme::System,
            language: "system".into(),
            log_level: LogLevel::Info,
            confirm_quit: false,
            auto_update: true,
        }
    }
}

// ---------------------------------------------------------------------------
// server
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindMode {
    /// All interfaces, including ones that appear later.
    #[default]
    All,
    /// Every address of the listed interfaces (follows DHCP changes).
    Interfaces,
    /// Exactly the listed IP addresses.
    Addresses,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Network {
    pub mode: BindMode,
    /// Interface names (OS names, e.g. `en0`, or Windows adapter GUID/alias).
    pub interfaces: Vec<String>,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    /// Only accept clients from a subnet of one of our bound interfaces.
    pub same_subnet_only: bool,
    /// CIDR filters (`192.168.1.0/24`, `fd00::/8`, single IPs). Empty = everyone.
    pub allow_list: Vec<String>,
    pub block_list: Vec<String>,
    /// Announce with mDNS on the bound interfaces.
    pub discovery: bool,
    /// Also listen and announce on IPv6 (off: IPv4 only).
    pub ipv6: bool,
    /// Clients must know this password (PLAN §14.1). Only a salted Argon2id key
    /// is stored, never the password. `None` = open.
    pub password: Option<StoredPassword>,
}

/// Salt and Argon2id-derived key of the server password, hex encoded.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPassword {
    pub salt: String,
    pub key: String,
}

impl std::fmt::Debug for StoredPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredPassword(..)")
    }
}

impl Default for Network {
    fn default() -> Self {
        Self {
            mode: BindMode::All,
            interfaces: Vec::new(),
            addresses: Vec::new(),
            port: DEFAULT_PORT,
            same_subnet_only: false,
            allow_list: Vec::new(),
            block_list: Vec::new(),
            discovery: true,
            ipv6: false,
            password: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    pub interval_ms: u32,
    pub miss_threshold: u32,
    pub degraded_latency_ms: u32,
    pub idle_timeout_ms: u32,
}

impl Default for Health {
    fn default() -> Self {
        Self { interval_ms: 1000, miss_threshold: 3, degraded_latency_ms: 80, idle_timeout_ms: 10_000 }
    }
}

/// Hotkeys as text (`"Ctrl+Alt+L"`); empty = disabled. Parsed by the input crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub lock_cursor: String,
    pub switch_home: String,
    pub switch_next: String,
    pub switch_previous: String,
    pub reconnect_all: String,
    pub identify: String,
    pub toggle_sharing: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            lock_cursor: "Ctrl+Alt+L".into(),
            switch_home: "Ctrl+Alt+H".into(),
            switch_next: "Ctrl+Alt+Right".into(),
            switch_previous: "Ctrl+Alt+Left".into(),
            reconnect_all: "Ctrl+Alt+R".into(),
            identify: "Ctrl+Alt+I".into(),
            toggle_sharing: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    #[default]
    Both,
    ToClients,
    FromClients,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharing {
    pub clipboard: bool,
    pub files: bool,
    pub direction: Direction,
    /// Items up to this size are pushed immediately; larger ones on paste.
    pub eager_bytes: u64,
    /// 0 = unlimited.
    pub max_clipboard_bytes: u64,
}

impl Default for Sharing {
    fn default() -> Self {
        Self { clipboard: true, files: true, direction: Direction::Both, eager_bytes: 1 << 20, max_clipboard_bytes: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Visuals {
    pub cursor_locator: bool,
    pub identify_seconds: u32,
    pub notify_connect: bool,
    pub notify_offline: bool,
}

impl Default for Visuals {
    fn default() -> Self {
        Self { cursor_locator: true, identify_seconds: 3, notify_connect: true, notify_offline: true }
    }
}

/// Modifier translation between platforms (PLAN §3.4.2, §14 B8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemapPreset {
    /// Swap Cmd↔Ctrl only when a Mac controls a PC or the other way round.
    Auto,
    /// Native: every key does what it does on a keyboard plugged into that computer.
    #[default]
    None,
    SwapCtrlMeta,
}

/// A client as remembered by the server (PLAN §6.7, §6.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientEntry {
    pub id: DeviceId,
    pub name: String,
    pub mouse_speed: f64,
    pub scroll_speed: f64,
    pub scroll_invert: bool,
    pub key_remap: RemapPreset,
    pub clipboard: bool,
    pub files: bool,
    pub relative_mouse: bool,
    pub notifications: bool,
    pub blocked: bool,
    /// For Wake-on-LAN (`aa:bb:cc:dd:ee:ff`).
    pub mac_address: String,
}

impl Default for ClientEntry {
    fn default() -> Self {
        Self {
            id: DeviceId([0; 16]),
            name: String::new(),
            mouse_speed: 1.0,
            scroll_speed: 1.0,
            scroll_invert: false,
            key_remap: RemapPreset::None,
            clipboard: true,
            files: true,
            relative_mouse: false,
            notifications: true,
            blocked: false,
            mac_address: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    pub network: Network,
    pub health: Health,
    pub switching: SwitchPolicy,
    /// Executables allowed to keep switching while full-screen (e.g. `code.exe`).
    pub fullscreen_allow: Vec<String>,
    pub hotkeys: Hotkeys,
    pub sharing: Sharing,
    pub visuals: Visuals,
    pub clients: Vec<ClientEntry>,
}

impl Server {
    #[must_use]
    pub fn client(&self, id: &DeviceId) -> Option<&ClientEntry> {
        self.clients.iter().find(|c| &c.id == id)
    }
}

// ---------------------------------------------------------------------------
// client
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Client {
    /// Computer name, `host`, `host:port`, `ip`, `ip:port`; empty = find with mDNS.
    pub server_address: String,
    /// The server's password, if it has one (kept in this private settings file).
    pub password: String,
    /// Also connect over IPv6 (off: IPv4 only, unless an IPv6 address is typed).
    pub ipv6: bool,
    /// Interface used to reach the server; empty = any.
    pub interface: String,
    /// Where received files land; `None` = Downloads.
    pub receive_dir: Option<PathBuf>,
    /// Local veto: both sides must allow clipboard/files.
    pub accept_clipboard: bool,
    pub accept_files: bool,
    /// Local overrides of the server's per-client settings (`None` = use server's).
    pub mouse_speed: Option<f64>,
    pub scroll_speed: Option<f64>,
    pub scroll_invert: Option<bool>,
    pub key_remap: Option<RemapPreset>,
    /// Draw a software cursor when no mouse is attached (servers, VMs).
    pub draw_cursor: bool,
    pub led_sync: bool,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            server_address: String::new(),
            password: String::new(),
            ipv6: false,
            interface: String::new(),
            receive_dir: None,
            accept_clipboard: true,
            accept_files: true,
            mouse_speed: None,
            scroll_speed: None,
            scroll_invert: None,
            key_remap: None,
            draw_cursor: true,
            led_sync: true,
        }
    }
}

// ---------------------------------------------------------------------------
// layout
// ---------------------------------------------------------------------------

/// Where a computer tile is drawn in the layout editor (UI only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub machine: DeviceId,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    pub tiles: Vec<Tile>,
    #[serde(rename = "link")]
    pub links: Vec<LinkSpec>,
}
