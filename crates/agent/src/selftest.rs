//! `glidedesk-agent --selftest` and the UI "Diagnostics → Run self-test":
//! repeatable checks for real-machine testing (PLAN §10.2).

use std::time::Instant;

use serde_json::{Value, json};

pub fn run(server_running: bool) -> Value {
    let started = Instant::now();
    let perms = glidedesk_input::permissions();
    let monitors = glidedesk_input::monitors();
    let interfaces = glidedesk_net::list_interfaces();
    // Capture can only be tested when the server is not already capturing.
    let capture = if server_running {
        json!({ "ok": true, "note": "server running (capture in use)" })
    } else {
        match glidedesk_input::start_capture() {
            Ok(c) => {
                c.control.stop();
                json!({ "ok": true })
            }
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    };
    let injector = match glidedesk_input::injector() {
        Ok(mut inj) => {
            // Harmless round trip: read the cursor and "move" it where it already is.
            let before = inj.cursor();
            let moved = before.map(|p| inj.inject(&glidedesk_proto::Input::MouseAbs(p)).is_ok());
            json!({ "ok": true, "cursor": before, "inject_move": moved })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    let loopback = loopback_latency();
    json!({
        "version": crate::agent::VERSION,
        "platform": glidedesk_proto::Platform::current(),
        "host": glidedesk_platform::host_name(),
        "permissions": { "accessibility": perms.accessibility, "input_monitoring": perms.input_monitoring },
        "monitors": monitors.map_err(|e| e.to_string()),
        "interfaces": interfaces,
        "capture": capture,
        "injector": injector,
        "session": glidedesk_platform::session_status(),
        "loopback_quic_ms": loopback,
        "took_ms": started.elapsed().as_millis(),
    })
}

/// QUIC round trip over 127.0.0.1 (measures crypto + stack overhead).
fn loopback_latency() -> Value {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return Value::Null;
    };
    rt.block_on(async {
        let plan = glidedesk_net::BindPlan::Addrs(vec![std::net::SocketAddr::from(([127, 0, 0, 1], 0))]);
        let Ok((mut server, _)) = glidedesk_net::Server::bind(&plan, glidedesk_net::Tuning::default()) else {
            return Value::Null;
        };
        let Some(addr) = server.local_addrs().first().copied() else { return Value::Null };
        tokio::spawn(async move {
            if let Some(inc) = server.accept().await
                && let Ok(conn) = inc.await
                && let Ok((mut s, mut r)) = conn.accept_bi().await
            {
                let mut b = [0u8; 1];
                for _ in 0..20 {
                    if r.read_exact(&mut b).await.is_err() || s.write_all(&b).await.is_err() {
                        break;
                    }
                }
                let _ = conn.closed().await;
            }
        });
        let Ok(ep) = glidedesk_net::client_endpoint(None, false, glidedesk_net::Tuning::default()) else {
            return Value::Null;
        };
        let Ok(conn) = glidedesk_net::connect(&ep, addr).await else { return Value::Null };
        let Ok((mut s, mut r)) = conn.open_bi().await else { return Value::Null };
        let mut samples = Vec::new();
        let mut b = [7u8; 1];
        for _ in 0..20 {
            let t = Instant::now();
            if s.write_all(&b).await.is_err() || r.read_exact(&mut b).await.is_err() {
                break;
            }
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        conn.close(0u32.into(), b"done");
        samples.sort_by(f64::total_cmp);
        let p = |q: f64| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
            let i = ((samples.len() as f64 - 1.0) * q).round() as usize;
            samples.get(i).copied()
        };
        json!({ "p50": p(0.5), "p95": p(0.95) })
    })
}
