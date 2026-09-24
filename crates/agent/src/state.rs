//! Small runtime state kept next to the config (not user settings):
//! monitors of known clients and the last server we joined.

use std::collections::HashMap;
use std::path::PathBuf;

use glidedesk_proto::{DeviceId, MonitorInfo};
use serde::{Deserialize, Serialize};
use tracing::warn;

const FILE: &str = "state.json";
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub known_monitors: HashMap<DeviceId, Vec<MonitorInfo>>,
    pub last_server: Option<DeviceId>,
    /// App version that last ran; a change means "just upgraded".
    pub last_version: Option<String>,
}

#[derive(Debug)]
pub struct StateFile {
    path: PathBuf,
    pub state: State,
}

impl StateFile {
    pub fn load(dir: &std::path::Path) -> Self {
        let path = dir.join(FILE);
        let state = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= MAX_BYTES)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { path, state }
    }

    pub fn save(&self) {
        match serde_json::to_vec_pretty(&self.state) {
            Ok(bytes) => {
                if let Err(e) = glidedesk_config::store::write_atomic(&self.path, &bytes) {
                    warn!(error = %e, "could not save state");
                }
            }
            Err(e) => warn!(error = %e, "could not serialise state"),
        }
    }
}
