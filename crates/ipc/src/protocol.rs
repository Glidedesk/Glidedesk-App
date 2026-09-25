//! Agent ⇄ UI protocol: newline-delimited JSON over a private local socket.
//!
//! UI → agent: `{"id": 7, "cmd": "set_locked", "locked": true}`
//! agent → UI: `{"type": "response", "id": 7, "ok": true, "data": …}`
//!             `{"type": "event", "event": "status", "data": …}`

use glidedesk_config::Config;
use glidedesk_proto::DeviceId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::views::AgentStatus;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Push status/notice events on this connection from now on.
    Subscribe,
    Status,
    GetConfig,
    SetConfig {
        config: Box<Config>,
    },
    /// Start / stop sharing (the agent stays alive).
    Start,
    Stop,
    /// Re-read config from disk and restart sharing.
    Reload,
    /// Stop sharing and exit the agent.
    Quit,
    SwitchTo {
        id: DeviceId,
    },
    SwitchHome,
    SetLocked {
        locked: bool,
    },
    ReconnectAll,
    Identify,
    Disconnect {
        id: DeviceId,
    },
    SetBlocked {
        id: DeviceId,
        blocked: bool,
    },
    /// Remove a client from the server's list and layout.
    Forget {
        id: DeviceId,
    },
    ListInterfaces,
    ExportConfig {
        layout_only: bool,
    },
    ImportPreview {
        text: String,
    },
    ResetConfig,
    RequestPermissions,
    Wake {
        id: DeviceId,
    },
    SelfTest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

/// Something the UI should tell the user about (tray notification).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NoticeView {
    NewClient { id: DeviceId, name: String, address: String },
    ClientOnline { id: DeviceId, name: String },
    ClientOffline { id: DeviceId, name: String },
    ServerConnected { name: String },
    Identify { label: String },
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Response {
        id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        data: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Event(Event),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    Status(Box<AgentStatus>),
    Notice(NoticeView),
    /// Settings changed outside the settings window (a new client joined,
    /// reset, reload): the window reloads them before its next save.
    ConfigChanged,
    /// The agent is exiting (Quit, upgrade, OS shutdown). The tray app exits too
    /// unless it asked for a restart itself.
    Exiting,
}

impl Message {
    #[must_use]
    pub fn ok(id: u64, data: Value) -> Self {
        Message::Response { id, ok: true, data, error: None }
    }
    #[must_use]
    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Message::Response { id, ok: false, data: Value::Null, error: Some(error.into()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_format() {
        let r = RequestEnvelope { id: 7, request: Request::SetLocked { locked: true } };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"id":7,"cmd":"set_locked","locked":true}"#);
        assert_eq!(serde_json::from_str::<RequestEnvelope>(&s).unwrap(), r);
        let bad = serde_json::from_str::<RequestEnvelope>(r#"{"id":1,"cmd":"format_disk"}"#);
        assert!(bad.is_err(), "unknown commands are rejected");
    }

    #[test]
    fn message_wire_format() {
        let m = Message::ok(3, serde_json::json!({"a": 1}));
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"type":"response","id":3,"ok":true,"data":{"a":1}}"#);
        let e = Message::Event(Event::Notice(NoticeView::Identify { label: "PC".into() }));
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Message>(&s).unwrap(), e);
    }
}
