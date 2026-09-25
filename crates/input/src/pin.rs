//! Keeping the local cursor still while another computer has control (macOS).
//!
//! A background app can't stop macOS from moving the real cursor: the event tap
//! only swallows the events. So the (hidden) cursor is pulled back to a pin in
//! the middle of the screen as soon as it drifts [`PIN_RADIUS`] points away —
//! otherwise it roams the whole screen in step with the hand, and shows up
//! doing so whenever macOS makes it visible again.
//!
//! Every pull (a warp) disturbs the next motion event: its delta fields contain
//! the jump. That event is measured from where the cursor was put instead, so
//! frequent pulls lose no motion. Events that were already queued before the
//! warp still carry their own, correct delta; they are told apart by position
//! (still near where the cursor was, not near where it went).

use glidedesk_proto::Point;

/// How far (points) the hidden cursor may drift from the pin before it is pulled back.
pub const PIN_RADIUS: i32 = 40;

/// Largest motion (points) accepted from one event; a larger value is a warp
/// artefact and would fling the cursor on the other computer.
pub const MAX_STEP: i32 = 250;

/// Events queued before a warp we accept before assuming the warp's own event was missed.
const STALE_EVENTS: u8 = 4;

/// A warp whose first motion event has not been seen yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Warp {
    /// Where the cursor was.
    pub from: Point,
    /// Where it was put.
    pub to: Point,
    left: u8,
}

impl Warp {
    #[must_use]
    pub const fn new(from: Point, to: Point) -> Self {
        Self { from, to, left: STALE_EVENTS }
    }
}

/// Is the cursor at `real` far enough from `pin` to be pulled back?
#[must_use]
pub const fn needs_repin(real: Point, pin: Point) -> bool {
    (real.x - pin.x).abs() > PIN_RADIUS || (real.y - pin.y).abs() > PIN_RADIUS
}

/// Motion of one event at `loc` with delta fields `raw`, given the pending warp
/// (if any). Returns the motion and the warp still pending afterwards.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn motion(warp: Option<Warp>, loc: (f64, f64), raw: (i32, i32)) -> ((i32, i32), Option<Warp>) {
    let step = |v: i32| v.clamp(-MAX_STEP, MAX_STEP);
    let Some(w) = warp else { return ((step(raw.0), step(raw.1)), None) };
    let dist = |p: Point| (loc.0 - f64::from(p.x)).hypot(loc.1 - f64::from(p.y));
    if dist(w.to) <= dist(w.from) {
        // Made after the warp: it moved the cursor from where we put it.
        let d = |l: f64, p: i32| step((l - f64::from(p)).round() as i32);
        ((d(loc.0, w.to.x), d(loc.1, w.to.y)), None)
    } else if w.left > 1 {
        // Queued before the warp: its own delta is right; keep waiting.
        ((step(raw.0), step(raw.1)), Some(Warp { left: w.left - 1, ..w }))
    } else {
        ((step(raw.0), step(raw.1)), None)
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
        assert_eq!(motion(None, (0.0, 0.0), (3, -2)), ((3, -2), None));
        assert_eq!(motion(None, (0.0, 0.0), (900, -900)), ((MAX_STEP, -MAX_STEP), None));
    }

    #[test]
    fn the_first_event_after_a_warp_is_measured_from_the_pin_not_lost() {
        let from = Point::new(PIN.x + 45, PIN.y + 10);
        let w = Warp::new(from, PIN);
        // The hand moved (6, -3) after the cursor was put back; the delta fields
        // report the jump as well (-45, -10) + (6, -3).
        let (d, left) = motion(Some(w), (f64::from(PIN.x) + 6.2, f64::from(PIN.y) - 2.9), (-39, -13));
        assert_eq!(d, (6, -3));
        assert_eq!(left, None);
    }

    #[test]
    fn events_queued_before_the_warp_keep_their_own_motion() {
        let from = Point::new(PIN.x + 45, PIN.y);
        let w = Warp::new(from, PIN);
        // Still at the old place: made before the warp.
        let (d, left) = motion(Some(w), (f64::from(from.x) + 2.0, f64::from(from.y)), (2, 0));
        assert_eq!(d, (2, 0));
        assert_eq!(left.map(|w| w.to), Some(PIN), "still waiting for the warp's own event");
        // Then the warp's own event.
        let (d, left) = motion(left, (f64::from(PIN.x) + 4.0, f64::from(PIN.y) + 1.0), (-41, 1));
        assert_eq!(d, (4, 1));
        assert_eq!(left, None);
    }

    #[test]
    fn a_missed_warp_event_stops_being_awaited() {
        let from = Point::new(PIN.x + 60, PIN.y);
        let mut w = Some(Warp::new(from, PIN));
        for _ in 0..STALE_EVENTS {
            w = motion(w, (f64::from(from.x), f64::from(from.y)), (1, 0)).1;
        }
        assert_eq!(w, None);
    }

    #[test]
    fn many_small_pulls_add_up_to_the_real_motion() {
        // The hand moves 10 points right per event for 100 events; the cursor
        // is pulled back whenever it leaves the radius. Nothing may be lost.
        let (mut real, mut warp, mut total) = (PIN, None, 0);
        for _ in 0..100 {
            real = Point::new(real.x + 10, real.y);
            let raw_x = warp.map_or(10, |w: Warp| real.x - w.from.x);
            let ((dx, _), next) = motion(warp, (f64::from(real.x), f64::from(real.y)), (raw_x, 0));
            total += dx;
            warp = next;
            if needs_repin(real, PIN) {
                warp = Some(Warp::new(real, PIN));
                real = PIN;
            }
        }
        assert_eq!(total, 1000);
    }
}
