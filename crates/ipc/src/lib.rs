//! Agent ⇄ UI interface: status views, request/event protocol and the
//! private local transport.

pub mod client;
pub mod protocol;
pub mod transport;
pub mod views;

pub use client::{AgentClient, IpcError};
pub use protocol::{Event, Message, NoticeView, Request, RequestEnvelope};
pub use transport::Endpoint;
pub use views::*;
