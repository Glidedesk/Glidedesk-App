#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::time::{Duration, Instant};

use nexpingdesk_layout::*;
use nexpingdesk_proto::{DeviceId, MonitorId, MonitorInfo, Point, Rect, Side};
use proptest::prelude::*;

const SERVER: DeviceId = DeviceId([1; 16]);
const CLIENT: DeviceId = DeviceId([2; 16]);
const CLIENT2: DeviceId = DeviceId([3; 16]);

fn mon(id: &str, r: Rect) -> MonitorInfo {
    MonitorInfo { id: MonitorId(id.into()), name: id.into(), bounds: r, scale: 1.0, primary: false }
}

fn ctx() -> EdgeContext {
    EdgeContext { now: Instant::now(), mods: Mods::default(), fullscreen: false }
}

/// Server: Monitor "top" (0,0 1000x500) above "bottom" (0,500 1000x500).
fn stacked_server() -> Machine {
    Machine {
        id: SERVER,
        monitors: vec![mon("top", Rect::new(0, 0, 1000, 500)), mon("bottom", Rect::new(0, 500, 1000, 500))],
    }
}

fn client() -> Machine {
    Machine { id: CLIENT, monitors: vec![mon("c", Rect::new(0, 0, 2000, 1000))] }
}

fn engine(spec: LinkSpec) -> Engine {
    let layout = Layout::build([stacked_server(), client()], &[spec]);
    assert!(layout.warnings().is_empty(), "{:?}", layout.warnings());
    Engine::new(SERVER, layout, SwitchPolicy { block_fullscreen: true, ..Default::default() })
}

#[test]
fn all_monitors_continuous_mapping() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    // Top of the top monitor → top of client.
    let out = e.on_local_move(Point::new(999, 0), 5, 0, ctx());
    assert_eq!(out, Outcome::Enter { machine: CLIENT, pos: Point::new(0, 0), previous: None });
    assert_eq!(e.focus(), Focus::Remote { machine: CLIENT, pos: Point::new(0, 0) });
}

#[test]
fn bottom_monitor_maps_to_lower_half_in_continuous_mode() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    let Outcome::Enter { pos, .. } = e.on_local_move(Point::new(999, 750), 3, 0, ctx()) else { panic!() };
    assert_eq!(pos, Point::new(0, 750));
}

#[test]
fn single_monitor_handover_blocks_other_monitor() {
    let spec = LinkSpec {
        handover: MonitorSelection::single(MonitorId("bottom".into())),
        ..LinkSpec::simple(SERVER, Side::Right, CLIENT)
    };
    let mut e = engine(spec);
    assert_eq!(e.on_local_move(Point::new(999, 100), 5, 0, ctx()), Outcome::None, "top monitor is a wall");
    let Outcome::Enter { pos, .. } = e.on_local_move(Point::new(999, 500), 5, 0, ctx()) else { panic!() };
    // Bottom monitor alone spans the whole client edge.
    assert_eq!(pos, Point::new(0, 0));
}

#[test]
fn per_monitor_mapping_and_return_to_origin_monitor() {
    let spec = LinkSpec { mapping: Mapping::PerMonitor, ..LinkSpec::simple(SERVER, Side::Right, CLIENT) };
    let mut e = engine(spec);
    // Middle of the bottom monitor → middle of the client.
    let Outcome::Enter { pos, .. } = e.on_local_move(Point::new(999, 750), 5, 0, ctx()) else { panic!() };
    assert_eq!(pos.x, 0);
    assert!((499..=501).contains(&pos.y), "middle of the client, got {pos:?}");
    // Walk back left to the client's left edge: must land on the bottom monitor.
    let out = e.on_remote_move(-5, 0, ctx());
    let Outcome::Return { pos, previous } = out else { panic!("{out:?}") };
    assert_eq!(previous, CLIENT);
    assert!(pos.y >= 500 && pos.y < 1000, "landed on bottom monitor, got {pos:?}");
    assert_eq!(pos.x, 999);
}

#[test]
fn remote_motion_moves_and_walls() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    e.on_local_move(Point::new(999, 10), 1, 0, ctx());
    assert_eq!(e.on_remote_move(100, 0, ctx()), Outcome::Move { pos: Point::new(100, 10) });
    // Top edge of the client has no link → wall.
    assert_eq!(e.on_remote_move(0, -50, ctx()), Outcome::Move { pos: Point::new(100, 0) });
    assert_eq!(e.on_remote_move(0, -50, ctx()), Outcome::None);
}

#[test]
fn offline_client_is_a_wall_and_active_client_loss_returns_home() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    assert_eq!(e.set_available(CLIENT, false), Outcome::None);
    assert_eq!(e.on_local_move(Point::new(999, 10), 5, 0, ctx()), Outcome::None);
    e.set_available(CLIENT, true);
    assert!(matches!(e.on_local_move(Point::new(999, 10), 5, 0, ctx()), Outcome::Enter { .. }));
    let out = e.set_available(CLIENT, false);
    assert_eq!(out, Outcome::Return { pos: Point::new(999, 10), previous: CLIENT });
    assert_eq!(e.focus(), Focus::Local);
}

#[test]
fn lock_cursor_prevents_switching() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    e.set_locked(true);
    assert_eq!(e.on_local_move(Point::new(999, 10), 5, 0, ctx()), Outcome::None);
}

#[test]
fn delay_guard_switches_on_poll() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    e.set_policy(SwitchPolicy { delay_ms: 150, ..Default::default() });
    let t0 = Instant::now();
    let c0 = EdgeContext { now: t0, ..ctx() };
    assert_eq!(e.on_local_move(Point::new(999, 10), 5, 0, c0), Outcome::None);
    let deadline = e.pending_deadline().expect("pending");
    assert_eq!(deadline, t0 + Duration::from_millis(150));
    assert_eq!(e.poll(t0 + Duration::from_millis(100)), Outcome::None);
    assert!(matches!(e.poll(deadline), Outcome::Enter { .. }));
}

#[test]
fn chain_server_client_client2() {
    let c2 = Machine { id: CLIENT2, monitors: vec![mon("d", Rect::new(0, 0, 800, 600))] };
    let specs = [LinkSpec::simple(SERVER, Side::Right, CLIENT), LinkSpec::simple(CLIENT, Side::Right, CLIENT2)];
    let layout = Layout::build([stacked_server(), client(), c2], &specs);
    let mut e = Engine::new(SERVER, layout, SwitchPolicy::default());
    e.on_local_move(Point::new(999, 0), 1, 0, ctx());
    assert!(matches!(e.on_remote_move(1999, 0, ctx()), Outcome::Move { .. }));
    let out = e.on_remote_move(5, 0, ctx());
    assert!(matches!(out, Outcome::Enter { machine: CLIENT2, previous: Some(CLIENT), .. }), "{out:?}");
    // And back again through the derived return path.
    let out = e.on_remote_move(-5, 0, ctx());
    assert!(matches!(out, Outcome::Enter { machine: CLIENT, previous: Some(CLIENT2), .. }), "{out:?}");
}

#[test]
fn wrap_from_last_to_first() {
    let specs = [LinkSpec::simple(SERVER, Side::Right, CLIENT)];
    let layout = Layout::build([stacked_server(), client()], &specs);
    let mut e = Engine::new(SERVER, layout, SwitchPolicy { wrap: true, ..Default::default() });
    e.on_local_move(Point::new(999, 0), 1, 0, ctx());
    e.on_remote_move(1999, 0, ctx());
    // Client's right edge has no link: wrap back to the server's left edge.
    let out = e.on_remote_move(5, 0, ctx());
    let Outcome::Return { pos, .. } = out else { panic!("{out:?}") };
    assert_eq!(pos.x, 0);
}

#[test]
fn switch_to_hotkey_and_home() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    assert!(matches!(e.switch_to(CLIENT), Outcome::Enter { pos: Point { x: 1000, y: 500 }, .. }));
    assert!(matches!(e.switch_to(SERVER), Outcome::Return { .. }));
}

#[test]
fn missing_monitor_warns_and_falls_back() {
    let spec = LinkSpec {
        handover: MonitorSelection::single(MonitorId("unplugged".into())),
        ..LinkSpec::simple(SERVER, Side::Right, CLIENT)
    };
    let layout = Layout::build([stacked_server(), client()], &[spec]);
    assert!(layout.warnings().contains(&Warning::SelectionFellBack { machine: SERVER, side: Side::Right }));
    let mut e = Engine::new(SERVER, layout, SwitchPolicy::default());
    assert!(matches!(e.on_local_move(Point::new(999, 100), 1, 0, ctx()), Outcome::Enter { .. }));
}

#[test]
fn speed_factor_accumulates_fractions() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    e.set_speed(CLIENT, 0.5);
    e.on_local_move(Point::new(999, 10), 1, 0, ctx());
    e.on_remote_move(1, 0, ctx());
    assert_eq!(e.on_remote_move(1, 0, ctx()), Outcome::Move { pos: Point::new(1, 10) });
}

/// A client with two screens: a 1080p one and, on its right, a taller one
/// at 150 % (Windows reports physical pixels).
fn two_screen_client() -> Machine {
    let mut big = mon("big", Rect::new(1920, -200, 2560, 1440));
    big.scale = 1.5;
    Machine { id: CLIENT, monitors: vec![mon("a", Rect::new(0, 0, 1920, 1080)), big] }
}

#[test]
fn the_cursor_reaches_every_client_monitor_and_speed_follows_each_screen() {
    let layout =
        Layout::build([stacked_server(), two_screen_client()], &[LinkSpec::simple(SERVER, Side::Right, CLIENT)]);
    let mut e = Engine::new(SERVER, layout, SwitchPolicy::default());
    e.set_units(CLIENT, vec![1.0, 1.5]);
    assert!(matches!(e.on_local_move(Point::new(999, 250), 5, 0, ctx()), Outcome::Enter { .. }));
    // Walk right across the first screen onto the second.
    let mut last = Point::default();
    for _ in 0..30 {
        if let Outcome::Move { pos } = e.on_remote_move(100, 0, ctx()) {
            last = pos;
        }
    }
    assert!(last.x >= 1920, "reached the second screen: {last:?}");
    // On the 150 % screen 10 px of motion move 15 of its pixels.
    let before = last;
    let Outcome::Move { pos } = e.on_remote_move(-10, 0, ctx()) else { panic!("moved") };
    assert_eq!(pos.x, before.x - 15);
    // Up into the part of the tall screen that is above the first one.
    let Outcome::Move { pos } = e.on_remote_move(0, -300, ctx()) else { panic!("moved") };
    assert!(pos.y < 0, "the taller screen is reachable above the first: {pos:?}");
    // Back left onto the first screen at a height both share.
    e.on_remote_move(0, 600, ctx());
    let mut back = Point::default();
    for _ in 0..40 {
        if let Outcome::Move { pos } = e.on_remote_move(-100, 0, ctx()) {
            back = pos;
        }
    }
    assert!(back.x < 1920, "back on the first screen: {back:?}");
}

fn arb_monitors() -> impl Strategy<Value = Vec<MonitorInfo>> {
    // Up to 3 monitors placed in a row with random sizes and vertical offsets.
    prop::collection::vec((200i32..3000, 200i32..2000, -500i32..500), 1..=3).prop_map(|specs| {
        let mut x = 0;
        specs
            .into_iter()
            .enumerate()
            .map(|(i, (w, h, dy))| {
                let m = mon(&format!("m{i}"), Rect::new(x, dy, w, h));
                x += w;
                m
            })
            .collect()
    })
}

proptest! {
    #[test]
    fn remote_cursor_always_stays_on_a_monitor(
        server in arb_monitors(),
        client_mons in arb_monitors(),
        moves in prop::collection::vec((-400i32..400, -400i32..400), 1..60),
        per_monitor in any::<bool>(),
    ) {
        let spec = LinkSpec {
            mapping: if per_monitor { Mapping::PerMonitor } else { Mapping::Continuous },
            ..LinkSpec::simple(SERVER, Side::Right, CLIENT)
        };
        let layout = Layout::build(
            [Machine { id: SERVER, monitors: server.clone() }, Machine { id: CLIENT, monitors: client_mons.clone() }],
            &[spec],
        );
        let mut e = Engine::new(SERVER, layout, SwitchPolicy::default());
        e.switch_to(CLIENT);
        for (dx, dy) in moves {
            match e.on_remote_move(dx, dy, ctx()) {
                Outcome::Move { pos } | Outcome::Enter { pos, .. } => {
                    prop_assert!(client_mons.iter().any(|m| m.bounds.contains(pos)), "{pos:?}");
                }
                Outcome::Return { pos, .. } => {
                    prop_assert!(server.iter().any(|m| m.bounds.contains(pos)), "{pos:?}");
                    break;
                }
                Outcome::None => {}
            }
        }
    }

    #[test]
    fn every_outer_right_edge_pixel_enters_a_valid_client_point(
        server in arb_monitors(),
        client_mons in arb_monitors(),
        sample in 0.0f64..1.0,
    ) {
        let layout = Layout::build(
            [Machine { id: SERVER, monitors: server.clone() }, Machine { id: CLIENT, monitors: client_mons.clone() }],
            &[LinkSpec::simple(SERVER, Side::Right, CLIENT)],
        );
        let rects: Vec<Rect> = server.iter().map(|m| m.bounds).collect();
        for seg in nexpingdesk_layout::edges::outer_segments(&rects, Side::Right) {
            #[allow(clippy::cast_possible_truncation)]
            let a = seg.start + ((f64::from(seg.len()) * sample) as i32).min(seg.len() - 1);
            let mut e = Engine::new(SERVER, layout.clone(), SwitchPolicy::default());
            match e.on_local_move(seg.point_at(a), 1, 0, ctx()) {
                Outcome::Enter { pos, .. } => prop_assert!(client_mons.iter().any(|m| m.bounds.contains(pos))),
                other => prop_assert!(false, "no switch: {other:?}"),
            }
        }
    }
}

/// §14 B7: the client (2000 wide) is twice as wide as the server (1000). The
/// server's capture delivers steady motion while the cursor is away (it is
/// re-pinned to the screen centre), so crossing the whole client and pushing
/// on its far edge always returns home on the first try.
#[test]
fn crossing_a_wide_client_and_back_returns_home_first_time() {
    let mut e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    assert!(matches!(e.on_local_move(Point::new(999, 200), 5, 0, ctx()), Outcome::Enter { .. }));
    for _ in 0..150 {
        let out = e.on_remote_move(20, 0, ctx());
        assert!(matches!(out, Outcome::Move { .. } | Outcome::None));
    }
    let mut back = None;
    for i in 0..200 {
        if let Outcome::Return { pos, .. } = e.on_remote_move(-20, 0, ctx()) {
            back = Some((i, pos));
            break;
        }
    }
    let (steps, pos) = back.expect("never came back");
    assert!(steps <= 101, "took {steps} pushes");
    assert_eq!(pos.x, 999);
    assert_eq!(e.focus(), Focus::Local);
}

/// The log/UI explanation when pushing an edge doesn't switch.
#[test]
fn explains_why_an_edge_push_does_not_switch() {
    let e = engine(LinkSpec::simple(SERVER, Side::Right, CLIENT));
    let why = e.explain_edge(SERVER, Point::new(500, 0), 0, -5, &ctx()).unwrap();
    assert!(why.contains("top edge") && why.contains("right"), "{why}");
    assert!(e.explain_edge(SERVER, Point::new(500, 200), 0, -5, &ctx()).is_none(), "not at an edge");
    assert_eq!(e.explain_edge(SERVER, Point::new(999, 200), 5, 0, &ctx()).as_deref(), Some("not blocked"));
}
