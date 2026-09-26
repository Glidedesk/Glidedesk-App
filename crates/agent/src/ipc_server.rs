//! Accepts UI connections and forwards requests to the agent loop.

use glidedesk_ipc::transport::{self, Listener, Stream};
use glidedesk_ipc::{Event, Message, RequestEnvelope};
use serde_json::Value;
use tokio::io::BufReader;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::debug;

use glidedesk_ipc::AgentStatus;

/// A request plus where to send the answer.
pub struct Call {
    pub request: glidedesk_ipc::Request,
    pub reply: oneshot::Sender<Result<Value, String>>,
}

pub fn spawn(
    mut listener: Listener,
    calls: mpsc::Sender<Call>,
    status: watch::Receiver<AgentStatus>,
    events: broadcast::Sender<Event>,
) {
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok(stream) => {
                    tokio::spawn(connection(stream, calls.clone(), status.clone(), events.subscribe()));
                }
                Err(e) => {
                    debug!(error = %e, "IPC accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
}

async fn connection(
    stream: Stream,
    calls: mpsc::Sender<Call>,
    mut status: watch::Receiver<AgentStatus>,
    mut events: broadcast::Receiver<Event>,
) {
    let (r, mut w) = tokio::io::split(stream);
    let mut reader = BufReader::new(r);
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    // Writer: responses and (after `subscribe`) pushed events.
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let Ok(line) = serde_json::to_vec(&msg) else { continue };
            if transport::write_line(&mut w, &line).await.is_err() {
                break;
            }
        }
    });
    let mut subscribed = false;
    let mut buf = Vec::new();
    loop {
        tokio::select! {
            line = transport::read_line(&mut reader, &mut buf) => {
                match line {
                    Ok(Some(())) => {}
                    _ => break,
                }
                let env: RequestEnvelope = match serde_json::from_slice(&buf) {
                    Ok(e) => e,
                    Err(e) => {
                        // Answer under the caller's number so it gets the error
                        // instead of waiting forever.
                        let seq = serde_json::from_slice::<Value>(&buf)
                            .ok()
                            .and_then(|v| v.get("seq").and_then(Value::as_u64))
                            .unwrap_or(0);
                        let _ = out_tx.send(Message::err(seq, format!("bad request: {e}"))).await;
                        continue;
                    }
                };
                if matches!(env.request, glidedesk_ipc::Request::Subscribe) {
                    subscribed = true;
                    let snapshot = status.borrow_and_update().clone();
                    let _ = out_tx.send(Message::ok(env.id, Value::Null)).await;
                    let _ = out_tx.send(Message::Event(Event::Status(Box::new(snapshot)))).await;
                    continue;
                }
                let (tx, rx) = oneshot::channel();
                if calls.send(Call { request: env.request, reply: tx }).await.is_err() {
                    break;
                }
                let out = out_tx.clone();
                let id = env.id;
                tokio::spawn(async move {
                    let msg = match rx.await {
                        Ok(Ok(v)) => Message::ok(id, v),
                        Ok(Err(e)) => Message::err(id, e),
                        Err(_) => Message::err(id, "agent is shutting down"),
                    };
                    let _ = out.send(msg).await;
                });
            }
            changed = status.changed(), if subscribed => {
                if changed.is_err() {
                    break;
                }
                let snapshot = status.borrow_and_update().clone();
                if out_tx.try_send(Message::Event(Event::Status(Box::new(snapshot)))).is_err() {
                    debug!("UI slow; status update skipped");
                }
            }
            ev = events.recv(), if subscribed => {
                match ev {
                    Ok(ev) => {
                        let _ = out_tx.send(Message::Event(ev)).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    drop(out_tx);
    let _ = writer.await;
}
