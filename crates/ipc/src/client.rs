//! UI-side connection to the agent: request/response with ids plus a stream
//! of pushed events.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::protocol::{Event, Message, Request, RequestEnvelope};
use crate::transport::{self, Endpoint};

const TIMEOUT: Duration = Duration::from_secs(15);

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("agent is not running")]
    NotRunning,
    #[error("connection to the agent closed")]
    Closed,
    #[error("agent did not answer in time")]
    Timeout,
    #[error("{0}")]
    Agent(String),
    #[error("bad reply: {0}")]
    Decode(String),
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
}

type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

pub struct AgentClient {
    writer: Mutex<BoxWrite>,
    pending: Pending,
    next_id: AtomicU64,
}

impl std::fmt::Debug for AgentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentClient").finish_non_exhaustive()
    }
}

impl AgentClient {
    /// Connects; pushed events arrive on the returned receiver.
    pub async fn connect(ep: &Endpoint) -> Result<(Arc<Self>, mpsc::Receiver<Event>), IpcError> {
        let stream = transport::connect(ep).await.map_err(|e| match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => IpcError::NotRunning,
            _ => IpcError::Io(e),
        })?;
        let (r, w) = tokio::io::split(stream);
        Ok(Self::from_parts(r, Box::new(w)))
    }

    fn from_parts<R: AsyncRead + Send + Unpin + 'static>(r: R, w: BoxWrite) -> (Arc<Self>, mpsc::Receiver<Event>) {
        let pending: Pending = Arc::default();
        let (ev_tx, ev_rx) = mpsc::channel(64);
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(r);
            let mut buf = Vec::new();
            while let Ok(Some(())) = transport::read_line(&mut reader, &mut buf).await {
                match serde_json::from_slice::<Message>(&buf) {
                    Ok(Message::Response { id, ok, data, error }) => {
                        let tx = reader_pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id);
                        if let Some(tx) = tx {
                            let _ = tx.send(if ok { Ok(data) } else { Err(error.unwrap_or_default()) });
                        }
                    }
                    Ok(Message::Event(ev)) => {
                        if ev_tx.try_send(ev).is_err() {
                            // UI is slow: a newer status will follow.
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "bad message from agent"),
                }
            }
            // Fail everything still waiting.
            reader_pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        });
        (Arc::new(Self { writer: Mutex::new(w), pending, next_id: AtomicU64::new(1) }), ev_rx)
    }

    /// Sends a request and waits for its reply.
    pub async fn request(&self, request: Request) -> Result<Value, IpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id, tx);
        let line = serde_json::to_vec(&RequestEnvelope { id, request }).map_err(|e| IpcError::Decode(e.to_string()))?;
        {
            let mut w = self.writer.lock().await;
            if let Err(e) = transport::write_line(&mut *w, &line).await {
                self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id);
                return Err(IpcError::Io(e));
            }
        }
        match tokio::time::timeout(TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(IpcError::Agent(e)),
            Ok(Err(_)) => Err(IpcError::Closed),
            Err(_) => {
                self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id);
                Err(IpcError::Timeout)
            }
        }
    }

    /// Typed convenience wrapper.
    pub async fn call<T: DeserializeOwned>(&self, request: Request) -> Result<T, IpcError> {
        let v = self.request(request).await?;
        serde_json::from_value(v).map_err(|e| IpcError::Decode(e.to_string()))
    }
}
