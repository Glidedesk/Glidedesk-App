#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real QUIC over loopback: bind, connect, exchange frames, enforce limits.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use glidedesk_config::IpFilter;
use glidedesk_net::*;
use glidedesk_proto::{Control, Input, KeyCode, MAX_CONTROL_FRAME, MAX_INPUT_FRAME};

fn localhost(port: u16) -> BindPlan {
    BindPlan::Addrs(vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)])
}

#[tokio::test]
async fn control_and_input_streams_roundtrip() {
    let (mut server, status) = Server::bind(&localhost(0), Tuning::default()).unwrap();
    assert!(status.iter().all(|s| s.error.is_none()));
    let addr = server.local_addrs()[0];

    let srv = tokio::spawn(async move {
        let conn = server.accept().await.unwrap().await.unwrap();
        let (send, recv) = conn.accept_bi().await.unwrap();
        let mut r = FrameReader::new(recv, MAX_CONTROL_FRAME);
        let mut w = FrameWriter::new(send, MAX_CONTROL_FRAME);
        let msg: Control = r.recv().await.unwrap().unwrap();
        assert_eq!(msg, Control::Ping { seq: 1, sent_us: 2, rtt_us: 0 });
        w.send(&Control::Pong { seq: 1, sent_us: 2, status: glidedesk_proto::ClientStatus::default() }).await.unwrap();
        let mut input = FrameWriter::new(conn.open_uni().await.unwrap(), MAX_INPUT_FRAME);
        let keys: Vec<Input> = (0..100).map(|i| Input::Key { key: KeyCode(i), down: i % 2 == 0 }).collect();
        input.send_batch(&keys).await.unwrap();
        input.finish();
        conn.closed().await;
    });

    let ep = client_endpoint(None, false, Tuning::default()).unwrap();
    let conn = connect(&ep, addr).await.unwrap();
    let (send, recv) = conn.open_bi().await.unwrap();
    let mut w = FrameWriter::new(send, MAX_CONTROL_FRAME);
    let mut r = FrameReader::new(recv, MAX_CONTROL_FRAME);
    w.send(&Control::Ping { seq: 1, sent_us: 2, rtt_us: 0 }).await.unwrap();
    assert!(matches!(r.recv::<Control>().await.unwrap(), Some(Control::Pong { seq: 1, .. })));
    let mut input = FrameReader::new(conn.accept_uni().await.unwrap(), MAX_INPUT_FRAME);
    let mut n = 0;
    while let Some(ev) = input.recv::<Input>().await.unwrap() {
        assert_eq!(ev, Input::Key { key: KeyCode(n), down: n % 2 == 0 });
        n += 1;
    }
    assert_eq!(n, 100);
    conn.close(0u32.into(), b"done");
    tokio::time::timeout(Duration::from_secs(5), srv).await.unwrap().unwrap();
}

#[tokio::test]
async fn oversized_frame_is_rejected_by_reader() {
    let (mut server, _) = Server::bind(&localhost(0), Tuning::default()).unwrap();
    let addr = server.local_addrs()[0];
    let srv = tokio::spawn(async move {
        let conn = server.accept().await.unwrap().await.unwrap();
        let (_send, recv) = conn.accept_bi().await.unwrap();
        let mut r = FrameReader::new(recv, 16);
        r.recv::<Control>().await
    });
    let ep = client_endpoint(None, false, Tuning::default()).unwrap();
    let conn = connect(&ep, addr).await.unwrap();
    let (send, _recv) = conn.open_bi().await.unwrap();
    let mut w = FrameWriter::new(send, MAX_CONTROL_FRAME);
    w.send(&Control::Identify { label: "x".repeat(1000) }).await.unwrap();
    let res = tokio::time::timeout(Duration::from_secs(5), srv).await.unwrap().unwrap();
    assert!(res.is_err(), "reader must refuse frames above its limit");
}

#[tokio::test]
async fn binding_a_foreign_address_is_reported_not_fatal() {
    let plan = BindPlan::Addrs(vec!["127.0.0.1:0".parse().unwrap(), "203.0.113.7:0".parse().unwrap()]);
    let (_server, status) = Server::bind(&plan, Tuning::default()).unwrap();
    assert_eq!(status.iter().filter(|s| s.error.is_some()).count(), 1);
    let only_bad = BindPlan::Addrs(vec!["203.0.113.7:0".parse().unwrap()]);
    assert!(matches!(Server::bind(&only_bad, Tuning::default()), Err(NetError::NothingBound(_))));
}

#[test]
fn admission_filters() {
    let a = Admission {
        filter: IpFilter::new(&["192.168.0.0/16".into()], &[]).unwrap(),
        same_subnet: Some(vec![IfAddr { ip: "192.168.1.1".parse().unwrap(), prefix: 24, scope_id: 0 }]),
    };
    assert!(a.admits("192.168.1.50".parse().unwrap()));
    assert!(!a.admits("192.168.2.50".parse().unwrap()), "outside our subnet");
    assert!(!a.admits("10.0.0.1".parse().unwrap()), "not allowed");
    assert!(a.admits("::ffff:192.168.1.9".parse().unwrap()), "v4-mapped");
}
