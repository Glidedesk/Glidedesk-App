//! Status snapshots the agent publishes to the tray / settings UI.

use glidedesk_config::Role;
use glidedesk_proto::{ClientStatus, DeviceId, MonitorInfo, Platform};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HealthState {
    /// Known from config, never connected in this session.
    NeverConnected,
    Connecting,
    Online,
    /// Connected but slow (latency above threshold).
    Degraded,
    /// Connected, screen locked / going to sleep / session inactive.
    Locked,
    Offline,
}

impl HealthState {
    /// Can the cursor enter this client?
    #[must_use]
    pub const fn reachable(self) -> bool {
        matches!(self, HealthState::Online | HealthState::Degraded | HealthState::Locked)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindView {
    pub addr: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineView {
    pub id: Option<DeviceId>,
    pub name: String,
    pub monitors: Vec<MonitorInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientView {
    pub id: DeviceId,
    pub name: String,
    pub platform: Option<Platform>,
    pub state: HealthState,
    pub latency_ms: Option<f32>,
    /// Unix seconds of the last heartbeat answer.
    pub last_seen: Option<u64>,
    pub address: Option<String>,
    pub app_version: Option<String>,
    pub monitors: Vec<MonitorInfo>,
    pub status: ClientStatus,
    pub blocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransferState {
    Active,
    Done,
    Failed,
}

/// One clipboard or file transfer (for the Transfers list and the tray).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransferView {
    pub id: u64,
    pub peer: String,
    /// `true` = this computer sends.
    pub outgoing: bool,
    pub name: String,
    pub done: u64,
    pub total: u64,
    pub state: TransferState,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerView {
    pub running: bool,
    pub bind: Vec<BindView>,
    /// Short hex fingerprint of this session's TLS certificate.
    pub fingerprint: String,
    pub local: MachineView,
    pub clients: Vec<ClientView>,
    /// Client that currently has the cursor (`None` = this machine).
    pub focus: Option<DeviceId>,
    pub locked: bool,
    pub warnings: Vec<String>,
    pub transfers: Vec<TransferView>,
    /// Files another computer offered, waiting for a paste here (§14.2).
    pub offer: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkState {
    #[default]
    Stopped,
    Searching,
    Connecting,
    Connected,
    /// The server refused us (blocked, incompatible version…).
    Rejected,
    Offline,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientSideView {
    pub state: LinkState,
    pub server_id: Option<DeviceId>,
    pub server_name: Option<String>,
    pub server_address: Option<String>,
    pub server_version: Option<String>,
    pub latency_ms: Option<f32>,
    /// The cursor is on this machine right now.
    pub active: bool,
    pub clipboard: bool,
    pub files: bool,
    pub message: Option<String>,
    pub local: MachineView,
    pub transfers: Vec<TransferView>,
    /// Files the server offered, waiting for a paste here (§14.2).
    pub offer: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsView {
    pub accessibility: bool,
    pub input_monitoring: bool,
}

/// Everything the tray and settings window show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentStatus {
    pub version: String,
    pub platform: Option<Platform>,
    pub role: Role,
    /// Sharing is on (server listening / client connecting).
    pub running: bool,
    /// Why sharing could not start (port in use, permission missing…).
    pub error: Option<String>,
    pub server: Option<ServerView>,
    pub client: Option<ClientSideView>,
    pub permissions: PermissionsView,
    pub config_issues: Vec<String>,
    pub config_read_only: bool,
    pub config_dir: String,
    pub log_dir: String,
    /// Effective quick toggles for the tray (server: sharing; client: local acceptance).
    pub clipboard: bool,
    pub files: bool,
    pub notifications: bool,
    pub start_at_login: bool,
    pub device_name: String,
}
