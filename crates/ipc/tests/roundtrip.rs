#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real socket: listener + client, request/response and pushed events.

use glidedesk_ipc::transport::{Listener, read_line, write_line};
use glidedesk_ipc::{AgentClient, Endpoint, Event, Message, NoticeView, Request, RequestEnvelope};
use tokio::io::BufReader;

#[tokio::test]
async fn request_response_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let ep = Endpoint(dir.path().join("a.sock").to_string_lossy().into_owned());
    let mut listener = Listener::bind(&ep).await.unwrap();
    // A second agent must not be able to bind.
    assert_eq!(Listener::bind(&ep).await.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);

    let server = tokio::spawn(async move {
        // The liveness probe from the second bind shows up as an empty connection first.
        let (mut r, mut w, mut buf) = loop {
            let s = listener.accept().await.unwrap();
            let (r, w) = tokio::io::split(s);
            let mut r = BufReader::new(r);
            let mut buf = Vec::new();
            if read_line(&mut r, &mut buf).await.unwrap().is_some() {
                break (r, w, buf);
            }
        };
        let req: RequestEnvelope = serde_json::from_slice(&buf).unwrap();
        assert_eq!(req.request, Request::Identify);
        let ev = Message::Event(Event::Notice(NoticeView::Identify { label: "x".into() }));
        write_line(&mut w, &serde_json::to_vec(&ev).unwrap()).await.unwrap();
        let reply = Message::ok(req.id, serde_json::json!("done"));
        write_line(&mut w, &serde_json::to_vec(&reply).unwrap()).await.unwrap();
        read_line(&mut r, &mut buf).await.unwrap().unwrap();
        let req: RequestEnvelope = serde_json::from_slice(&buf).unwrap();
        write_line(&mut w, &serde_json::to_vec(&Message::err(req.id, "nope")).unwrap()).await.unwrap();
    });

    let (client, mut events) = AgentClient::connect(&ep).await.unwrap();
    let v: String = client.call(Request::Identify).await.unwrap();
    assert_eq!(v, "done");
    assert!(matches!(events.recv().await, Some(Event::Notice(NoticeView::Identify { .. }))));
    let err = client.request(Request::Quit).await.unwrap_err();
    assert_eq!(err.to_string(), "nope");
    server.await.unwrap();
}

#[tokio::test]
async fn oversized_lines_are_refused() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let writer = tokio::spawn(async move {
        let mut b = b;
        let big = vec![b'x'; glidedesk_ipc::transport::MAX_LINE + 10];
        let _ = tokio::io::AsyncWriteExt::write_all(&mut b, &big).await;
    });
    let mut r = BufReader::new(a);
    let mut buf = Vec::new();
    assert!(read_line(&mut r, &mut buf).await.is_err());
    writer.abort();
}
