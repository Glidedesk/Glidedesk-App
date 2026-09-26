//! Keeping the local cursor still while another computer has control (macOS).
//!
//! A background app can't stop macOS from moving the real cursor: the event tap
//! only swallows the events. So the (hidden) cursor is pulled back to a pin in
//! the middle of the screen once it drifts [`PIN_RADIUS`] points away —
//! otherwise it roams the whole screen in step with the hand, and shows up
//! doing so whenever macOS makes it visible again.
//!
//! Every pull (a warp) disturbs one motion event: the first one made after it
//! reports the jump as well (its delta = the hand's motion + the jump). Events
//! already queued before the warp report only the hand's motion, and even the
//! event's location can't tell them apart (it may predate the warp). So each
//! event after a warp is judged by its delta alone: taking the jump out must
//! make it fit the recent motion better. Only one warp is in flight at a time.
//! Getting this wrong sent a fast mouse (many events, big steps) backwards by
//! the jump every few events, so it hardly moved on the other computer.

use glidedesk_proto::Point;

/// How far (points) the hidden cursor may drift from the pin before it is pulled back.
pub const PIN_RADIUS: i32 = 80;

/// Largest motion (points) accepted from one event; a larger value is a warp
/// artefact and would fling the cursor on the other computer.
pub const MAX_STEP: i32 = 250;

/// Events after a warp without its jump before we stop expecting it.
const STALE_EVENTS: u8 = 6;

/// A warp whose jump has not been seen in an event yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Warp {
    /// The jump: where the cursor was put minus where it was.
    jump: (i32, i32),
    left: u8,
}

/// Turns motion events into the hand's motion, across our warps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tracker {
    warp: Option<Warp>,
    /// Motion of the previous event: the hand's recent motion.
    last: (i32, i32),
}

/// Is the cursor at `real` far enough from `pin` to be pulled back?
#[must_use]
pub const fn needs_repin(real: Point, pin: Point) -> bool {
    (real.x - pin.x).abs() > PIN_RADIUS || (real.y - pin.y).abs() > PIN_RADIUS
}

fn step(v: (i32, i32)) -> (i32, i32) {
    (v.0.clamp(-MAX_STEP, MAX_STEP), v.1.clamp(-MAX_STEP, MAX_STEP))
}

fn dist2(a: (i32, i32), b: (i32, i32)) -> i64 {
    let (x, y) = (i64::from(a.0) - i64::from(b.0), i64::from(a.1) - i64::from(b.1));
    x * x + y * y
}

impl Tracker {
    /// The cursor was warped from `from` to `to`.
    pub fn warped(&mut self, from: Point, to: Point) {
        let jump = (to.x - from.x, to.y - from.y);
        self.warp = (jump != (0, 0)).then_some(Warp { jump, left: STALE_EVENTS });
    }

    /// No warp in flight: a new one may be made without confusing the two jumps.
    #[must_use]
    pub const fn settled(&self) -> bool {
        self.warp.is_none()
    }

    /// The hand's motion in one event whose delta fields read `raw`.
    pub fn motion(&mut self, raw: (i32, i32)) -> (i32, i32) {
        let own = match self.warp {
            None => raw,
            Some(w) => {
                let fixed = (raw.0 - w.jump.0, raw.1 - w.jump.1);
                if dist2(fixed, self.last) < dist2(raw, self.last) {
                    // Made after the warp: its delta includes the jump.
                    self.warp = None;
                    fixed
                } else {
                    // Queued before the warp: its delta is the hand's alone.
                    self.warp = (w.left > 1).then_some(Warp { left: w.left - 1, ..w });
                    raw
                }
            }
        };
        let own = step(own);
        self.last = own;
        own
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: Point = Point::new(720, 450);

    #[test]
    fn drift_beyond_the_radius_is_pulled_back() {
        assert!(!needs_repin(PIN, PIN));
        assert!(!needs_repin(Point::new(PIN.x + PIN_RADIUS, PIN.y - PIN_RADIUS), PIN));
        assert!(needs_repin(Point::new(PIN.x + PIN_RADIUS + 1, PIN.y), PIN));
        assert!(needs_repin(Point::new(PIN.x, PIN.y - PIN_RADIUS - 1), PIN));
    }

    #[test]
    fn without_a_warp_the_delta_fields_are_used_and_capped() {
        let mut t = Tracker::default();
        assert_eq!(t.motion((3, -2)), (3, -2));
        assert_eq!(t.motion((900, -900)), (MAX_STEP, -MAX_STEP));
    }

    #[test]
    fn the_event_with_the_jump_gives_the_hands_motion() {
        let mut t = Tracker::default();
        let _ = t.motion((30, 0));
        t.warped(Point::new(PIN.x + 90, PIN.y), PIN); // jump (-90, 0)
        assert_eq!(t.motion((-60, 1)), (30, 1), "30 right, not 60 left");
        assert!(t.settled());
    }

    #[test]
    fn events_queued_before_the_warp_keep_their_motion() {
        let mut t = Tracker::default();
        let _ = t.motion((30, 0));
        t.warped(Point::new(PIN.x + 90, PIN.y), PIN);
        assert_eq!(t.motion((30, 0)), (30, 0));
        assert_eq!(t.motion((28, 0)), (28, 0));
        assert!(!t.settled(), "the jump is still to come");
        assert_eq!(t.motion((-62, 0)), (28, 0));
        assert!(t.settled());
    }

    #[test]
    fn a_warp_whose_jump_never_shows_stops_being_expected() {
        let mut t = Tracker::default();
        t.warped(Point::new(PIN.x + 90, PIN.y), PIN);
        for _ in 0..STALE_EVENTS {
            assert_eq!(t.motion((5, 0)), (5, 0));
        }
        assert!(t.settled());
    }

    #[test]
    fn a_warp_to_the_same_place_changes_nothing() {
        let mut t = Tracker::default();
        t.warped(PIN, PIN);
        assert!(t.settled());
        assert_eq!(t.motion((4, 4)), (4, 4));
    }

    /// How macOS delivers motion, modelled: the device moves the cursor and each
    /// event reports the change since the previous event's position; a warp moves
    /// the cursor without an event, so the next event made reports the jump too.
    /// The tap sees events `lag` events late and warps like the capture does.
    /// Every event must come out as exactly the hand's motion.
    fn simulate(moves: &[(i32, i32)], lag: usize) -> Vec<((i32, i32), (i32, i32))> {
        let (mut cursor, mut reported) = (PIN, PIN);
        let mut queue = std::collections::VecDeque::new();
        let mut t = Tracker::default();
        let mut out = Vec::new();
        for m in moves.iter().copied().chain(std::iter::repeat_n((0, 0), lag)) {
            cursor = Point::new(cursor.x + m.0, cursor.y + m.1);
            queue.push_back((m, (cursor.x - reported.x, cursor.y - reported.y)));
            reported = cursor;
            if queue.len() > lag {
                let (hand, raw) = queue.pop_front().expect("queued");
                out.push((hand, t.motion(raw)));
                if t.settled() && needs_repin(cursor, PIN) {
                    t.warped(cursor, PIN);
                    cursor = PIN;
                }
            }
        }
        out
    }

    fn fast_mouse() -> Vec<(i32, i32)> {
        // A 1000 Hz mouse flicked right, then left, then diagonally: each flick
        // speeds up to 60 points per event and slows down again (a hand can't
        // turn around at full speed).
        let mut v = Vec::new();
        for i in 0..400 {
            let j = i % 100;
            let speed = 1 + if j < 50 { j * 59 / 50 } else { (99 - j) * 59 / 50 };
            let dir = match i / 100 {
                0 => (1, 0),
                1 => (-1, 0),
                2 => (1, 1),
                _ => (-1, 1),
            };
            v.push((dir.0 * speed, dir.1 * speed / 2));
        }
        v
    }

    #[test]
    fn a_fast_mouse_moves_exactly_as_the_hand_did_whatever_the_queueing() {
        let moves = fast_mouse();
        for lag in 0..=3 {
            let out = simulate(&moves, lag);
            let wrong: Vec<_> = out.iter().enumerate().filter(|(_, (hand, got))| hand != got).collect();
            assert!(
                wrong.is_empty(),
                "lag {lag}: {} of {} events wrong, first: {:?}",
                wrong.len(),
                out.len(),
                wrong.first()
            );
        }
    }

    #[test]
    fn a_slow_trackpad_moves_exactly_as_the_hand_did() {
        let moves: Vec<(i32, i32)> = (0..300).map(|i| if i % 3 == 0 { (2, -1) } else { (1, 0) }).collect();
        for lag in 0..=3 {
            let out = simulate(&moves, lag);
            assert!(out.iter().all(|(hand, got)| hand == got), "lag {lag}");
        }
    }
}
