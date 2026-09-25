//! Glidedesk core: the server hub (capture → engine → network), the client
//! runtime (network → injection) and connection health.

#![forbid(unsafe_code)]

pub mod client;
pub mod health;
pub mod server;
pub mod sync;
pub mod transfers;

use std::sync::Arc;

use glidedesk_config::Config;
use glidedesk_input::InputError;
use glidedesk_proto::{DeviceId, MonitorInfo};

pub use client::{ClientCommand, ClientDeps, ClientHandle};
pub use glidedesk_ipc::views::{ClientSideView, ClientView, HealthState, LinkState, ServerView};
pub use health::HealthConfig;
pub use server::{ServerCommand, ServerDeps, ServerHandle};

/// Returns this machine's monitors (platform function, or a mock in tests).
pub type MonitorSource = Arc<dyn Fn() -> Result<Vec<MonitorInfo>, InputError> + Send + Sync>;

/// Events for the agent (persist, notify, show overlays).
#[derive(Clone, Debug)]
pub enum Notice {
    /// The server changed its own config (new client auto-added/placed, renamed).
    ConfigChanged(Box<Config>),
    /// Monitors of a client, for the state cache.
    MonitorsSeen {
        id: DeviceId,
        monitors: Vec<MonitorInfo>,
    },
    NewClient {
        id: DeviceId,
        name: String,
        address: String,
    },
    ClientOnline {
        id: DeviceId,
        name: String,
        notify: bool,
    },
    ClientOffline {
        id: DeviceId,
        name: String,
        notify: bool,
    },
    ServerConnected {
        id: DeviceId,
        name: String,
    },
    /// Show the "identify screens" overlay with this label.
    Identify {
        label: String,
    },
    /// Something the user asked for failed (e.g. pasting offered files).
    Error {
        message: String,
    },
    /// Something the user should know that isn't an error.
    Info {
        message: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("network: {0}")]
    Net(String),
    #[error("input: {0}")]
    Input(String),
    #[error("protocol: {0}")]
    Protocol(String),
}
