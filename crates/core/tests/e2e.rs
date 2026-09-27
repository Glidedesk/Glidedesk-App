#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Server and client talking over real QUIC on loopback, with mock input.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nexpingdesk_config::{BindMode, Config, Role};
use nexpingdesk_core::{ClientDeps, HealthState, LinkState, ServerCommand, ServerDeps, client, server};
use nexpingdesk_input::mock::{self, ControlCall, MockInjector};
use nexpingdesk_input::{CaptureEvent, Gesture, InputError, gesture};
use nexpingdesk_proto::{DeviceId, GoodbyeReason, Input, KeyCode, MonitorId, MonitorInfo, Point, Rect};

const SERVER_ID: DeviceId = DeviceId([0x11; 16]);
const CLIENT_ID: DeviceId = DeviceId([0x22; 16]);

fn monitor(w: i32, h: i32) -> Vec<MonitorInfo> {
    vec![MonitorInfo {
        id: MonitorId("m".into()),
        name: "m".into(),
        bounds: Rect::new(0, 0, w, h),
        scale: 1.0,
        primary: true,
    }]
}

fn source(m: Vec<MonitorInfo>) -> nexpingdesk_core::MonitorSource {
    Arc::new(move || Ok::<_, InputError>(m.clone()))
}

fn server_config() -> Config {
    let mut c = Config::default();
    c.device.id = SERVER_ID;
    c.device.name = "server".into();
    c.device.role = Role::Server;
    c.server.network.mode = BindMode::Addresses;
    c.server.network.addresses = vec!["127.0.0.1".parse().unwrap()];
    c.server.network.port = 0;
    c.server.network.discovery = false;
    c.server.health.interval_ms = 300;
    c.server.health.miss_threshold = 3;
    c
}

async fn wait_for<T: Clone>(rx: &mut tokio::sync::watch::Receiver<T>, what: &str, pred: impl Fn(&T) -> bool) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let v = rx.borrow_and_update().clone();
                if pred(&v) {
                    return v;
                }
            }
            rx.changed().await.expect("sender alive");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

async fn wait_injected(inj: &MockInjector, what: &str, pred: impl Fn(&[Input]) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if pred(&inj.events()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}: {:?}", inj.events()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_session() {
    let _ = tracing_subscriber::fmt().with_env_filter("nexpingdesk=debug").with_test_writer().try_init();

    // --- server
    let (capture, cap_tx, cap_ctl) = mock::capture();
    let srv = server::start(
        server_config(),
        ServerDeps {
            capture,
            monitors: source(monitor(1000, 500)),
            known_monitors: HashMap::new(),
            forgotten: std::collections::HashSet::new(),
            app_version: "test".into(),
            host_name: "server-host".into(),
            clipboard: None,
        },
    )
    .expect("server starts");
    let mut srv_status = srv.status.clone();
    let view = wait_for(&mut srv_status, "server running", |v| v.running).await;
    let addr: std::net::SocketAddr = view.bind.iter().find(|b| b.error.is_none()).expect("bound").addr.parse().unwrap();

    // --- client
    let mut ccfg = Config::default();
    ccfg.device.id = CLIENT_ID;
    ccfg.device.name = "client".into();
    ccfg.device.role = Role::Client;
    ccfg.client.server_address = addr.to_string();
    let injector = MockInjector::default();
    let cli = client::start(
        ccfg,
        ClientDeps {
            injector: Box::new(injector.clone()),
            monitors: source(monitor(2000, 1000)),
            status: Arc::new(nexpingdesk_proto::ClientStatus::default),
            app_version: "test".into(),
            host_name: "client-host".into(),
            preferred_server: None,
            clipboard: None,
        },
    );
    let mut cli_status = cli.status.clone();
    wait_for(&mut cli_status, "client connected", |v| v.state == LinkState::Connected).await;
    let view = wait_for(&mut srv_status, "client online", |v| {
        v.clients.iter().any(|c| c.id == CLIENT_ID && c.state == HealthState::Online)
    })
    .await;
    assert_eq!(view.clients[0].name, "client");

    // Auto-placed on the right: push against the right edge.
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 250), dx: 4, dy: 0 }).await.unwrap();
    wait_injected(&injector, "enter", |e| e.iter().any(|i| matches!(i, Input::MouseAbs(p) if p.x == 0))).await;
    assert!(cap_ctl.calls.lock().unwrap().contains(&ControlCall::Grab(true)));
    wait_for(&mut srv_status, "focus on client", |v| v.focus == Some(CLIENT_ID)).await;
    wait_for(&mut cli_status, "client active", |v| v.active).await;

    // Motion and keys go to the client.
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 250), dx: 100, dy: 0 }).await.unwrap();
    cap_tx.send(CaptureEvent::Key { key: KeyCode(0x04), down: true }).await.unwrap();
    wait_injected(&injector, "key down", |e| e.contains(&Input::Key { key: KeyCode(0x04), down: true })).await;
    wait_injected(&injector, "moved", |e| e.iter().any(|i| matches!(i, Input::MouseAbs(p) if p.x == 100))).await;

    // The gesture button: the client's own shortcut for it, pressed then released.
    cap_tx.send(CaptureEvent::Gesture(Gesture::Left)).await.unwrap();
    let keys = gesture::shortcut(Gesture::Left, nexpingdesk_proto::Platform::current());
    let want: Vec<Input> = keys
        .iter()
        .map(|&k| Input::Key { key: KeyCode(k), down: true })
        .chain(keys.iter().rev().map(|&k| Input::Key { key: KeyCode(k), down: false }))
        .collect();
    wait_injected(&injector, "gesture shortcut", |e| e.windows(want.len()).any(|w| w == want.as_slice())).await;

    // Back across the left edge while 'A' is still held: the client must release it.
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 250), dx: -150, dy: 0 }).await.unwrap();
    wait_for(&mut srv_status, "focus home", |v| v.focus.is_none()).await;
    wait_injected(&injector, "released", |e| e.contains(&Input::Key { key: KeyCode(0x04), down: false })).await;
    assert!(cap_ctl.calls.lock().unwrap().contains(&ControlCall::Grab(false)));
    assert!(
        cap_ctl.calls.lock().unwrap().iter().any(|c| matches!(c, ControlCall::Warp(p) if (990..=999).contains(&p.x)))
    );

    // Hotkey Ctrl+Alt+L locks the cursor: pushing the edge does nothing.
    for (k, d) in [(0xE0, true), (0xE2, true), (0x0F, true), (0x0F, false), (0xE2, false), (0xE0, false)] {
        cap_tx.send(CaptureEvent::Key { key: KeyCode(k), down: d }).await.unwrap();
    }
    wait_for(&mut srv_status, "locked", |v| v.locked).await;
    srv.commands.send(ServerCommand::SetLocked(false)).await.unwrap();
    wait_for(&mut srv_status, "unlocked", |v| !v.locked).await;

    // Client leaves politely → offline immediately.
    cli.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Quitting)).await.unwrap();
    wait_for(&mut srv_status, "client offline", |v| {
        v.clients.iter().any(|c| c.id == CLIENT_ID && c.state == HealthState::Offline)
    })
    .await;

    // An offline client is a wall.
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 250), dx: 4, dy: 0 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(srv_status.borrow().focus.is_none());

    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), srv.task).await.unwrap().unwrap();
    assert!(cap_ctl.calls.lock().unwrap().contains(&ControlCall::Stop));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_client_goes_offline_by_heartbeat() {
    let (capture, _cap_tx, _cap_ctl) = mock::capture();
    let srv = server::start(
        server_config(),
        ServerDeps {
            capture,
            monitors: source(monitor(800, 600)),
            known_monitors: HashMap::new(),
            forgotten: std::collections::HashSet::new(),
            app_version: "test".into(),
            host_name: "s".into(),
            clipboard: None,
        },
    )
    .unwrap();
    let mut st = srv.status.clone();
    let addr: std::net::SocketAddr = wait_for(&mut st, "running", |v| v.running).await.bind[0].addr.parse().unwrap();

    // A raw peer that says hello and then never answers pings.
    let ep = nexpingdesk_net::client_endpoint(None, nexpingdesk_net::Tuning::default()).unwrap();
    let conn = nexpingdesk_net::connect(&ep, addr).await.unwrap();
    let (send, recv) = conn.open_bi().await.unwrap();
    let mut w = nexpingdesk_net::FrameWriter::new(send, nexpingdesk_proto::MAX_CONTROL_FRAME);
    w.send(&nexpingdesk_proto::Control::Hello(nexpingdesk_proto::Hello {
        protocol: nexpingdesk_proto::PROTOCOL_VERSION,
        app_version: "x".into(),
        device_id: CLIENT_ID,
        name: "mute".into(),
        platform: nexpingdesk_proto::Platform::Windows,
        monitors: monitor(640, 480),
        features: nexpingdesk_proto::Features::default(),
        prefs: nexpingdesk_proto::ClientPrefs::default(),
    }))
    .await
    .unwrap();
    let _keep = (recv, conn.clone());
    wait_for(&mut st, "online", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;
    let started = std::time::Instant::now();
    wait_for(&mut st, "offline by heartbeat", |v| v.clients.iter().any(|c| c.state == HealthState::Offline)).await;
    let took = started.elapsed();
    assert!(took < Duration::from_secs(3), "offline after {took:?} (3 × 300 ms + grace expected)");
    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clipboard_and_files_follow_the_cursor() {
    use nexpingdesk_clipboard::ClipData;
    use nexpingdesk_clipboard::mock::MockClipboard;

    let server_clip = MockClipboard::default();
    let client_clip = MockClipboard::default();
    let client_inj = MockInjector::default();
    let (capture, cap_tx, cap_ctl) = mock::capture();
    let srv = server::start(
        server_config(),
        ServerDeps {
            capture,
            monitors: source(monitor(1000, 500)),
            known_monitors: HashMap::new(),
            forgotten: std::collections::HashSet::new(),
            app_version: "test".into(),
            host_name: "s".into(),
            clipboard: Some(Box::new(server_clip.clone())),
        },
    )
    .unwrap();
    let mut st = srv.status.clone();
    let addr: std::net::SocketAddr = wait_for(&mut st, "running", |v| v.running).await.bind[0].addr.parse().unwrap();
    let mut ccfg = Config::default();
    ccfg.device.id = CLIENT_ID;
    ccfg.device.role = Role::Client;
    ccfg.client.server_address = addr.to_string();
    let cli = client::start(
        ccfg,
        ClientDeps {
            injector: Box::new(client_inj.clone()),
            monitors: source(monitor(800, 600)),
            status: Arc::new(nexpingdesk_proto::ClientStatus::default),
            app_version: "test".into(),
            host_name: "c".into(),
            preferred_server: None,
            clipboard: Some(Box::new(client_clip.clone())),
        },
    );
    wait_for(&mut st, "online", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;

    let wait_clip = |clip: MockClipboard, what: &'static str, pred: fn(&ClipData) -> bool| async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !pred(&clip.contents()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}: {:?}", clip.contents()));
    };

    // 1. Server → client when the cursor enters.
    server_clip.set_external(ClipData { text: Some("hello from server".into()), ..Default::default() });
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: 5, dy: 0 }).await.unwrap();
    wait_clip(client_clip.clone(), "server text on client", |c| c.text.as_deref() == Some("hello from server")).await;

    // 2. Client → server when the cursor comes back.
    client_clip.set_external(ClipData {
        text: Some("from client".into()),
        html: Some("<b>x</b>".into()),
        ..Default::default()
    });
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: -10, dy: 0 }).await.unwrap();
    wait_clip(server_clip.clone(), "client text on server", |c| c.text.as_deref() == Some("from client")).await;
    assert_eq!(server_clip.contents().html.as_deref(), Some("<b>x</b>"));

    // 3. No echo: entering again must not send the client its own clipboard back.
    let seq_before = nexpingdesk_clipboard::Clipboard::sequence(&client_clip);
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: 5, dy: 0 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(nexpingdesk_clipboard::Clipboard::sequence(&client_clip), seq_before, "clipboard echoed back");
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: -10, dy: 0 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 4. Files (§14.2): copy a folder on the server and enter the client. Only an
    //    offer arrives: the old clipboard is replaced, nothing is copied yet.
    let src = tempfile::tempdir().unwrap();
    let folder = src.path().join("Project");
    std::fs::create_dir_all(folder.join("assets")).unwrap();
    std::fs::write(folder.join("assets/logo.bin"), vec![9u8; 700_000]).unwrap();
    std::fs::write(folder.join("notes.txt"), b"ship it").unwrap();
    server_clip.set_external(ClipData { files: vec![folder], ..Default::default() });
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: 5, dy: 0 }).await.unwrap();
    wait_clip(client_clip.clone(), "offer placeholder on client", |c| {
        c.text.as_deref().is_some_and(|t| t.starts_with("Project from"))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client_clip.contents().files.is_empty(), "files must not copy by themselves");
    assert!(srv.status.borrow().transfers.is_empty(), "no transfer before a paste");

    //    Going back without pasting must not download them either (they used to
    //    be fetched so they could be passed on).
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: -10, dy: 0 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(srv.status.borrow().transfers.is_empty(), "returning must not fetch offered files");
    assert!(!server_clip.contents().files.is_empty(), "server clipboard untouched");
    assert!(client_clip.contents().files.is_empty());
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: 5, dy: 0 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    //    The user pastes on the client (Ctrl+V, ⌘V on macOS): the V press is
    //    held, the files arrive, then the paste is replayed.
    let ctrl = if cfg!(target_os = "macos") { KeyCode(0xE3) } else { KeyCode(0xE0) };
    let v = KeyCode(0x19);
    for (k, down) in [(ctrl, true), (v, true), (v, false)] {
        cap_tx.send(CaptureEvent::Key { key: k, down }).await.unwrap();
    }
    wait_clip(client_clip.clone(), "files on client", |c| !c.files.is_empty()).await;
    let got = client_clip.contents().files[0].clone();
    assert!(got.ends_with("Project"), "{got:?}");
    assert_eq!(std::fs::read(got.join("notes.txt")).unwrap(), b"ship it");
    assert_eq!(std::fs::read(got.join("assets/logo.bin")).unwrap().len(), 700_000);
    wait_injected(&client_inj, "paste replayed after the files", |e| {
        let at = e.iter().position(|i| *i == Input::Key { key: v, down: true });
        at.is_some_and(|i| e[i + 1..].contains(&Input::Key { key: v, down: false }))
    })
    .await;
    cap_tx.send(CaptureEvent::Key { key: ctrl, down: false }).await.unwrap();

    // 5. Client → server: the server holds its own paste shortcut while offered
    //    files wait, fetches them on paste and replays it.
    let doc = src.path().join("report.pdf");
    std::fs::write(&doc, vec![7u8; 4096]).unwrap();
    client_clip.set_external(ClipData { files: vec![doc], ..Default::default() });
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: -10, dy: 0 }).await.unwrap();
    wait_clip(server_clip.clone(), "offer on server", |c| {
        c.text.as_deref().is_some_and(|t| t.starts_with("report.pdf"))
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !cap_ctl.calls.lock().unwrap().contains(&ControlCall::PasteHold(true)) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("paste hold on");
    cap_tx.send(CaptureEvent::PasteRequested { key: v }).await.unwrap();
    wait_clip(server_clip.clone(), "files on server", |c| !c.files.is_empty()).await;
    assert_eq!(std::fs::read(&server_clip.contents().files[0]).unwrap().len(), 4096);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !cap_ctl.calls.lock().unwrap().contains(&ControlCall::ReplayPaste(v)) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("paste replayed on server");

    cli.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Quitting)).await.unwrap();
    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
}

fn start_client(cfg: Config, injector: &MockInjector) -> nexpingdesk_core::ClientHandle {
    client::start(
        cfg,
        ClientDeps {
            injector: Box::new(injector.clone()),
            monitors: source(monitor(800, 600)),
            status: Arc::new(nexpingdesk_proto::ClientStatus::default),
            app_version: "test".into(),
            host_name: "client-host".into(),
            preferred_server: None,
            clipboard: None,
        },
    )
}

/// A server with a password only admits clients that know it,
/// never lists the others, and a client with a password never trusts an open server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_protects_the_server() {
    let mut scfg = server_config();
    let v = nexpingdesk_net::auth::Verifier::new("hunter22").unwrap();
    let (salt, key) = v.to_hex();
    scfg.server.network.password = Some(nexpingdesk_config::StoredPassword { salt, key });
    let (capture, _cap_tx, _cap_ctl) = mock::capture();
    let deps = ServerDeps {
        capture,
        monitors: source(monitor(1000, 500)),
        known_monitors: HashMap::new(),
        forgotten: std::collections::HashSet::new(),
        app_version: "test".into(),
        host_name: "server-host".into(),
        clipboard: None,
    };
    let srv = server::start(scfg, deps).expect("server starts");
    let mut srv_status = srv.status.clone();
    let view = wait_for(&mut srv_status, "server running", |v| v.running).await;
    let addr: std::net::SocketAddr = view.bind.iter().find(|b| b.error.is_none()).unwrap().addr.parse().unwrap();

    let mut ccfg = Config::default();
    ccfg.device.id = CLIENT_ID;
    ccfg.device.role = Role::Client;
    ccfg.client.server_address = addr.to_string();

    // No password → refused, not listed.
    let inj = MockInjector::default();
    let none = start_client(ccfg.clone(), &inj);
    let mut st = none.status.clone();
    let v = wait_for(&mut st, "refused without password", |v| v.state == LinkState::Rejected).await;
    assert!(v.message.unwrap_or_default().contains("needs a password"));
    assert!(srv_status.borrow().clients.is_empty(), "unauthenticated client listed");

    // Wrong password → refused; the right one (new settings) → connected.
    let mut wrong = ccfg.clone();
    wrong.client.password = "nope-nope".into();
    none.commands.send(nexpingdesk_core::ClientCommand::ApplyConfig(Box::new(wrong))).await.unwrap();
    let v = wait_for(&mut st, "wrong password", |v| v.message.as_deref().is_some_and(|m| m.contains("wrong password")))
        .await;
    assert_eq!(v.state, LinkState::Rejected);
    assert!(srv_status.borrow().clients.is_empty());

    let mut right = ccfg.clone();
    right.client.password = "hunter22".into();
    none.commands.send(nexpingdesk_core::ClientCommand::ApplyConfig(Box::new(right.clone()))).await.unwrap();
    wait_for(&mut st, "connected with password", |v| v.state == LinkState::Connected).await;
    wait_for(&mut srv_status, "listed", |v| v.clients.iter().any(|c| c.id == CLIENT_ID)).await;
    none.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Quitting)).await.unwrap();
    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), srv.task).await.unwrap().unwrap();

    // A client with a password refuses a server without one (it could be an impostor).
    let (capture, _t, _c) = mock::capture();
    let open = server::start(
        server_config(),
        ServerDeps {
            capture,
            monitors: source(monitor(1000, 500)),
            known_monitors: HashMap::new(),
            forgotten: std::collections::HashSet::new(),
            app_version: "test".into(),
            host_name: "server-host".into(),
            clipboard: None,
        },
    )
    .unwrap();
    let mut os = open.status.clone();
    let view = wait_for(&mut os, "open server running", |v| v.running).await;
    right.client.server_address = view.bind.iter().find(|b| b.error.is_none()).unwrap().addr.clone();
    let cautious = start_client(right, &MockInjector::default());
    let mut cs = cautious.status.clone();
    let v = wait_for(&mut cs, "refuses open server", |v| v.state == LinkState::Rejected).await;
    assert!(v.message.unwrap_or_default().contains("doesn't use one"));
    open.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
}

/// "Forget" sticks: the client is refused (not re-added) until its user
/// presses Reconnect. Files that were already copied when the server
/// started are not offered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forget_sticks_until_reconnect_and_old_files_are_not_offered() {
    use nexpingdesk_clipboard::ClipData;
    use nexpingdesk_clipboard::mock::MockClipboard;

    let old = tempfile::tempdir().unwrap();
    let file = old.path().join("old.txt");
    std::fs::write(&file, b"old").unwrap();
    let server_clip = MockClipboard::default();
    server_clip.set_external(ClipData { files: vec![file], ..Default::default() });
    let client_clip = MockClipboard::default();
    client_clip.set_external(ClipData { text: Some("mine".into()), ..Default::default() });

    let (capture, cap_tx, _cap_ctl) = mock::capture();
    let scfg = server_config();
    let srv = server::start(
        scfg.clone(),
        ServerDeps {
            capture,
            monitors: source(monitor(1000, 500)),
            known_monitors: HashMap::new(),
            forgotten: std::collections::HashSet::new(),
            app_version: "test".into(),
            host_name: "s".into(),
            clipboard: Some(Box::new(server_clip.clone())),
        },
    )
    .unwrap();
    let mut st = srv.status.clone();
    let addr = wait_for(&mut st, "running", |v| v.running).await.bind[0].addr.clone();
    let mut ccfg = Config::default();
    ccfg.device.id = CLIENT_ID;
    ccfg.device.role = Role::Client;
    ccfg.client.server_address = addr;
    let cli = client::start(
        ccfg,
        ClientDeps {
            injector: Box::new(MockInjector::default()),
            monitors: source(monitor(800, 600)),
            status: Arc::new(nexpingdesk_proto::ClientStatus::default),
            app_version: "test".into(),
            host_name: "c".into(),
            preferred_server: None,
            clipboard: Some(Box::new(client_clip.clone())),
        },
    );
    wait_for(&mut st, "online", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;

    // Old files: entering the client offers nothing and leaves its clipboard alone.
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: 5, dy: 0 }).await.unwrap();
    let v = wait_for(&mut st, "not-offered note", |v| v.activity.iter().any(|a| a.text.contains("not offered"))).await;
    assert!(v.activity[0].text.contains("already on the clipboard when Nexpingdesk started"), "{:?}", v.activity);
    assert_eq!(client_clip.contents().text.as_deref(), Some("mine"));
    cap_tx.send(CaptureEvent::Motion { pos: Point::new(999, 100), dx: -10, dy: 0 }).await.unwrap();

    // Forget: removed from the settings, refused on reconnect, not re-added.
    let mut forgot = scfg.clone();
    forgot.server.clients.retain(|c| c.id != CLIENT_ID);
    forgot.layout.links.clear();
    srv.commands.send(ServerCommand::ApplyConfig { config: Box::new(forgot), removed: vec![CLIENT_ID] }).await.unwrap();
    let mut cs = cli.status.clone();
    let v = wait_for(&mut cs, "refused after forget", |v| v.state == LinkState::Rejected).await;
    assert!(v.message.unwrap_or_default().contains("Reconnect"));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(st.borrow().clients.iter().all(|c| c.id != CLIENT_ID), "a forgotten client came back by itself");

    // The client's user presses Reconnect: it joins again.
    cli.commands.send(nexpingdesk_core::ClientCommand::Reconnect).await.unwrap();
    wait_for(&mut cs, "rejoined", |v| v.state == LinkState::Connected).await;
    wait_for(&mut st, "listed again", |v| v.clients.iter().any(|c| c.id == CLIENT_ID)).await;

    cli.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Quitting)).await.unwrap();
    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
}

fn clip_client(addr: &str, clip: &nexpingdesk_clipboard::mock::MockClipboard) -> nexpingdesk_core::ClientHandle {
    let mut ccfg = Config::default();
    ccfg.device.id = CLIENT_ID;
    ccfg.device.role = Role::Client;
    addr.clone_into(&mut ccfg.client.server_address);
    client::start(
        ccfg,
        ClientDeps {
            injector: Box::new(MockInjector::default()),
            monitors: source(monitor(800, 600)),
            status: Arc::new(nexpingdesk_proto::ClientStatus::default),
            app_version: "test".into(),
            host_name: "c".into(),
            preferred_server: None,
            clipboard: Some(Box::new(clip.clone())),
        },
    )
}

fn clip_server(
    cfg: Config,
    clip: &nexpingdesk_clipboard::mock::MockClipboard,
) -> (server::ServerHandle, tokio::sync::mpsc::Sender<CaptureEvent>, Arc<mock::MockControl>) {
    let (capture, cap_tx, cap_ctl) = mock::capture();
    let srv = server::start(
        cfg,
        ServerDeps {
            capture,
            monitors: source(monitor(1000, 500)),
            known_monitors: HashMap::new(),
            forgotten: std::collections::HashSet::new(),
            app_version: "test".into(),
            host_name: "s".into(),
            clipboard: Some(Box::new(clip.clone())),
        },
    )
    .unwrap();
    (srv, cap_tx, cap_ctl)
}

async fn wait_clip(
    clip: &nexpingdesk_clipboard::mock::MockClipboard,
    what: &str,
    pred: impl Fn(&nexpingdesk_clipboard::ClipData) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !pred(&clip.contents()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}: {:?}", clip.contents()));
}

/// A client that restarts (new process, its clipboard lost, its offers gone)
/// must work fully again without restarting the server: the server sends it
/// the clipboard it got from it before, and an offer from the old link no
/// longer holds the server's paste shortcut.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_client_works_without_restarting_the_server() {
    use nexpingdesk_clipboard::ClipData;
    use nexpingdesk_clipboard::mock::MockClipboard;

    let server_clip = MockClipboard::default();
    let (srv, cap_tx, cap_ctl) = clip_server(server_config(), &server_clip);
    let mut st = srv.status.clone();
    let addr = wait_for(&mut st, "running", |v| v.running).await.bind[0].addr.clone();
    let enter = || CaptureEvent::Motion { pos: Point::new(999, 100), dx: 5, dy: 0 };
    let back = || CaptureEvent::Motion { pos: Point::new(999, 100), dx: -10, dy: 0 };

    // The client copies text; it reaches the server when the cursor comes back.
    let clip1 = MockClipboard::default();
    let cli = clip_client(&addr, &clip1);
    wait_for(&mut st, "online", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;
    cap_tx.send(enter()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    clip1.set_external(ClipData { text: Some("copied on the client".into()), ..Default::default() });
    cap_tx.send(back()).await.unwrap();
    wait_clip(&server_clip, "client text on server", |c| c.text.as_deref() == Some("copied on the client")).await;

    // The client restarts: a new process with an empty clipboard.
    cli.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Restarting)).await.unwrap();
    wait_for(&mut st, "offline", |v| v.clients.iter().all(|c| c.state == HealthState::Offline)).await;
    let clip2 = MockClipboard::default();
    let cli = clip_client(&addr, &clip2);
    let v = wait_for(&mut st, "online again", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;
    assert_eq!(v.clients.len(), 1, "the restarted client is the same computer");

    // Entering it again brings the text back (it used to be skipped as an echo).
    cap_tx.send(enter()).await.unwrap();
    wait_clip(&clip2, "text back on the restarted client", |c| c.text.as_deref() == Some("copied on the client")).await;

    // The restarted client offers a file, then restarts once more before the paste.
    let src = tempfile::tempdir().unwrap();
    let doc = src.path().join("report.pdf");
    std::fs::write(&doc, vec![7u8; 4096]).unwrap();
    clip2.set_external(ClipData { files: vec![doc], ..Default::default() });
    cap_tx.send(back()).await.unwrap();
    wait_for(&mut st, "offer on server", |v| v.offer.is_some()).await;
    assert!(cap_ctl.calls.lock().unwrap().contains(&ControlCall::PasteHold(true)));
    cli.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Restarting)).await.unwrap();

    // The offer died with that link: the paste shortcut is released, the offer is gone.
    let v = wait_for(&mut st, "offer withdrawn", |v| v.offer.is_none()).await;
    assert!(v.activity.iter().any(|a| a.text.contains("no longer offered")), "{:?}", v.activity);
    tokio::time::timeout(Duration::from_secs(5), async {
        while cap_ctl.calls.lock().unwrap().iter().rev().find(|c| matches!(c, ControlCall::PasteHold(_)))
            != Some(&ControlCall::PasteHold(false))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("paste hold released");

    // And it connects a third time as usual; the withdrawn offer's placeholder
    // text is not passed on as if it were something the user copied.
    let clip3 = MockClipboard::default();
    let cli = clip_client(&addr, &clip3);
    wait_for(&mut st, "online a third time", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;
    cap_tx.send(enter()).await.unwrap();
    wait_for(&mut st, "cursor on the client", |v| v.focus == Some(CLIENT_ID)).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(nexpingdesk_clipboard::Clipboard::sequence(&clip3), 0, "placeholder sent: {:?}", clip3.contents());
    cap_tx.send(back()).await.unwrap();
    cli.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Quitting)).await.unwrap();
    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
}

/// The other way round: a server that restarted gets the client's clipboard
/// again, even when that clipboard came from the server before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_server_gets_the_client_clipboard_again() {
    use nexpingdesk_clipboard::ClipData;
    use nexpingdesk_clipboard::mock::MockClipboard;

    // A fixed port, so the restarted server is found at the same address.
    let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut cfg = server_config();
    cfg.server.network.port = port;
    let addr = format!("127.0.0.1:{port}");
    let enter = || CaptureEvent::Motion { pos: Point::new(999, 100), dx: 5, dy: 0 };
    let back = || CaptureEvent::Motion { pos: Point::new(999, 100), dx: -10, dy: 0 };

    let server_clip = MockClipboard::default();
    let (srv, cap_tx, _) = clip_server(cfg.clone(), &server_clip);
    let mut st = srv.status.clone();
    let client_clip = MockClipboard::default();
    let cli = clip_client(&addr, &client_clip);
    wait_for(&mut st, "online", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;
    server_clip.set_external(ClipData { text: Some("copied on the server".into()), ..Default::default() });
    cap_tx.send(enter()).await.unwrap();
    wait_clip(&client_clip, "server text on client", |c| c.text.as_deref() == Some("copied on the server")).await;
    cap_tx.send(back()).await.unwrap();

    // The server restarts with an empty clipboard (and meets the client anew).
    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Restarting)).await.unwrap();
    let _ = srv.task.await;
    let server_clip = MockClipboard::default();
    let (srv, cap_tx, _) = clip_server(cfg, &server_clip);
    let mut st = srv.status.clone();
    wait_for(&mut st, "online after the restart", |v| v.clients.iter().any(|c| c.state == HealthState::Online)).await;
    cap_tx.send(enter()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    cap_tx.send(back()).await.unwrap();
    wait_clip(&server_clip, "client clipboard on the restarted server", |c| {
        c.text.as_deref() == Some("copied on the server")
    })
    .await;

    cli.commands.send(nexpingdesk_core::ClientCommand::Shutdown(GoodbyeReason::Quitting)).await.unwrap();
    srv.commands.send(ServerCommand::Shutdown(GoodbyeReason::Stopping)).await.unwrap();
}
