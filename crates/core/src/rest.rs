//! Keeps a still mouse still on the other computer.
//!
//! A resting mouse still reports motion now and then: a high-resolution sensor
//! (e.g. a Logitech MX Master) picks up a bumped desk or typing, a trackpad a
//! brushing palm. On the server those are fractions of a pixel nobody sees, but
//! each one reaches a client as whole pixels, so its cursor crept on its own
//! while the hand was off the mouse. At rest, motion is held back until it adds
//! up to a real move within a short time; once moving, everything passes.

use std::time::{Duration, Instant};

/// Motion (pixels, on either axis) that wakes a resting mouse.
pub const WAKE_PX: i32 = 4;
/// It must add up within this time; slower motion is noise and is dropped.
pub const WAKE_WINDOW: Duration = Duration::from_millis(120);
/// A mouse that passed no motion for this long is at rest again.
pub const REST_AFTER: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestFilter {
    /// Moving until then (each passed motion extends it); `None` = at rest.
    moving_until: Option<Instant>,
    /// Motion held back at rest, since `held_since`.
    held: (i32, i32),
    held_since: Option<Instant>,
}

impl RestFilter {
    /// The motion to pass on for `d`, reported at `now`.
    pub fn filter(&mut self, d: (i32, i32), now: Instant) -> (i32, i32) {
        if d == (0, 0) {
            return d;
        }
        if self.moving_until.is_some_and(|t| now < t) {
            self.moving_until = Some(now + REST_AFTER);
            return d;
        }
        if self.held_since.is_none_or(|t| now.duration_since(t) > WAKE_WINDOW) {
            self.held = (0, 0);
            self.held_since = Some(now);
        }
        self.held = (self.held.0.saturating_add(d.0), self.held.1.saturating_add(d.1));
        if self.held.0.abs().max(self.held.1.abs()) < WAKE_PX {
            return (0, 0);
        }
        let out = self.held;
        *self = Self { moving_until: Some(now + REST_AFTER), ..Self::default() };
        out
    }

    /// Back to rest (control moved to another computer).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(start: Instant, n: u64) -> Instant {
        start + Duration::from_millis(n)
    }

    #[test]
    fn sensor_noise_at_rest_never_moves_the_cursor() {
        let mut f = RestFilter::default();
        let t0 = Instant::now();
        // A twitch every 40 ms for 10 s, wobbling around the same spot.
        let wobble = [(1, 0), (-1, 1), (0, -1), (1, 0)];
        let mut total = (0, 0);
        for (i, d) in (0..250u64).zip(wobble.iter().cycle()) {
            let out = f.filter(*d, ms(t0, i * 40));
            total = (total.0 + out.0, total.1 + out.1);
        }
        assert_eq!(total, (0, 0), "a resting mouse moved the cursor");
    }

    #[test]
    fn a_real_move_passes_whole_including_its_start() {
        let mut f = RestFilter::default();
        let t0 = Instant::now();
        let mut total = (0, 0);
        for i in 0..50u64 {
            let out = f.filter((2, 1), ms(t0, i * 8));
            total = (total.0 + out.0, total.1 + out.1);
        }
        assert_eq!(total, (100, 50), "motion was lost");
    }

    #[test]
    fn a_slow_deliberate_move_wakes_within_a_few_events() {
        let mut f = RestFilter::default();
        let t0 = Instant::now();
        let outs: Vec<_> = (0..6u64).map(|i| f.filter((1, 0), ms(t0, i * 8))).collect();
        assert_eq!(outs, vec![(0, 0), (0, 0), (0, 0), (4, 0), (1, 0), (1, 0)]);
    }

    #[test]
    fn it_rests_again_after_a_pause() {
        let mut f = RestFilter::default();
        let t0 = Instant::now();
        assert_eq!(f.filter((10, 0), t0), (10, 0));
        assert_eq!(f.filter((1, 0), ms(t0, 100)), (1, 0), "still moving");
        assert_eq!(f.filter((1, 0), ms(t0, 100 + 300)), (0, 0), "at rest again");
        f.reset();
        assert_eq!(f.filter((1, 0), ms(t0, 500)), (0, 0));
    }
}
