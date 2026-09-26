//! Switch guards: rules that stop accidental switching.

use std::time::{Duration, Instant};

use glidedesk_proto::{DeviceId, Side};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModifierKey {
    Shift,
    Ctrl,
    Alt,
    /// Cmd on macOS, Win on Windows.
    Meta,
}

/// Currently held modifiers (either side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    #[must_use]
    pub const fn has(self, key: ModifierKey) -> bool {
        match key {
            ModifierKey::Shift => self.shift,
            ModifierKey::Ctrl => self.ctrl,
            ModifierKey::Alt => self.alt,
            ModifierKey::Meta => self.meta,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SwitchPolicy {
    /// Time the cursor must keep pushing against the edge (0 = off).
    pub delay_ms: u32,
    /// Hit the edge twice within this window (0 = off).
    pub double_tap_ms: u32,
    /// Modifier that must be held to switch.
    pub modifier: Option<ModifierKey>,
    /// Pixels near a monitor corner where switching is blocked (0 = off).
    pub dead_corner_px: u32,
    /// Leaving the last screen wraps to the first one.
    pub wrap: bool,
    /// Do not switch while a full-screen app is focused on the server.
    pub block_fullscreen: bool,
}

impl Default for SwitchPolicy {
    fn default() -> Self {
        Self { delay_ms: 0, double_tap_ms: 0, modifier: None, dead_corner_px: 0, wrap: false, block_fullscreen: true }
    }
}

/// Per-event facts the platform layer knows.
#[derive(Clone, Copy, Debug)]
pub struct EdgeContext {
    pub now: Instant,
    pub mods: Mods,
    /// A full-screen app that is not on the allow-list is focused.
    pub fullscreen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    /// Keep pushing (or call `Engine::poll`) at this time.
    WaitUntil(Instant),
}

type EdgeKey = (DeviceId, Side);

#[derive(Debug, Default)]
pub(crate) struct GuardState {
    pushing: Option<(EdgeKey, Instant)>,
    tap: Option<(EdgeKey, Instant, bool)>,
}

impl GuardState {
    /// The cursor is no longer against any edge.
    pub(crate) fn away(&mut self) {
        self.pushing = None;
        if let Some((_, _, left)) = self.tap.as_mut() {
            *left = true;
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// `corner_distance` is the distance along the edge to the nearest corner of the monitor.
    pub(crate) fn evaluate(
        &mut self,
        key: EdgeKey,
        ctx: &EdgeContext,
        policy: &SwitchPolicy,
        corner_distance: u32,
    ) -> Decision {
        if let Some(m) = policy.modifier
            && !ctx.mods.has(m)
        {
            return Decision::Deny;
        }
        if policy.block_fullscreen && ctx.fullscreen {
            return Decision::Deny;
        }
        if corner_distance < policy.dead_corner_px {
            return Decision::Deny;
        }

        if policy.double_tap_ms > 0 {
            let window = Duration::from_millis(u64::from(policy.double_tap_ms));
            return match self.tap {
                Some((k, at, left)) if k == key && left && ctx.now.duration_since(at) <= window => {
                    self.tap = None;
                    Decision::Allow
                }
                Some((k, _, false)) if k == key => Decision::Deny, // still on the first push
                _ => {
                    self.tap = Some((key, ctx.now, false));
                    Decision::Deny
                }
            };
        }

        if policy.delay_ms > 0 {
            let delay = Duration::from_millis(u64::from(policy.delay_ms));
            let since = match self.pushing {
                Some((k, t)) if k == key => t,
                _ => {
                    self.pushing = Some((key, ctx.now));
                    ctx.now
                }
            };
            let ready = since + delay;
            if ctx.now < ready {
                return Decision::WaitUntil(ready);
            }
        }
        self.pushing = None;
        Decision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: EdgeKey = (DeviceId([1; 16]), Side::Right);

    fn ctx(now: Instant) -> EdgeContext {
        EdgeContext { now, mods: Mods::default(), fullscreen: false }
    }

    #[test]
    fn default_policy_allows_immediately() {
        let mut g = GuardState::default();
        assert_eq!(g.evaluate(KEY, &ctx(Instant::now()), &SwitchPolicy::default(), 100), Decision::Allow);
    }

    #[test]
    fn delay_waits_then_allows() {
        let p = SwitchPolicy { delay_ms: 200, ..Default::default() };
        let mut g = GuardState::default();
        let t0 = Instant::now();
        assert_eq!(g.evaluate(KEY, &ctx(t0), &p, 100), Decision::WaitUntil(t0 + Duration::from_millis(200)));
        assert!(matches!(g.evaluate(KEY, &ctx(t0 + Duration::from_millis(100)), &p, 100), Decision::WaitUntil(_)));
        assert_eq!(g.evaluate(KEY, &ctx(t0 + Duration::from_millis(200)), &p, 100), Decision::Allow);
    }

    #[test]
    fn delay_resets_when_leaving_edge() {
        let p = SwitchPolicy { delay_ms: 200, ..Default::default() };
        let mut g = GuardState::default();
        let t0 = Instant::now();
        let _ = g.evaluate(KEY, &ctx(t0), &p, 100);
        g.away();
        let t1 = t0 + Duration::from_millis(250);
        assert!(matches!(g.evaluate(KEY, &ctx(t1), &p, 100), Decision::WaitUntil(_)));
    }

    #[test]
    fn double_tap_needs_leave_and_return() {
        let p = SwitchPolicy { double_tap_ms: 400, ..Default::default() };
        let mut g = GuardState::default();
        let t0 = Instant::now();
        assert_eq!(g.evaluate(KEY, &ctx(t0), &p, 100), Decision::Deny);
        // Still pushing on the first tap: no switch.
        assert_eq!(g.evaluate(KEY, &ctx(t0 + Duration::from_millis(50)), &p, 100), Decision::Deny);
        g.away();
        assert_eq!(g.evaluate(KEY, &ctx(t0 + Duration::from_millis(300)), &p, 100), Decision::Allow);
    }

    #[test]
    fn double_tap_expires() {
        let p = SwitchPolicy { double_tap_ms: 400, ..Default::default() };
        let mut g = GuardState::default();
        let t0 = Instant::now();
        let _ = g.evaluate(KEY, &ctx(t0), &p, 100);
        g.away();
        assert_eq!(g.evaluate(KEY, &ctx(t0 + Duration::from_millis(900)), &p, 100), Decision::Deny);
    }

    #[test]
    fn modifier_fullscreen_and_corners_block() {
        let mut g = GuardState::default();
        let now = Instant::now();
        let p = SwitchPolicy { modifier: Some(ModifierKey::Ctrl), ..Default::default() };
        assert_eq!(g.evaluate(KEY, &ctx(now), &p, 100), Decision::Deny);
        let held = EdgeContext { mods: Mods { ctrl: true, ..Default::default() }, ..ctx(now) };
        assert_eq!(g.evaluate(KEY, &held, &p, 100), Decision::Allow);

        let fs = EdgeContext { fullscreen: true, ..ctx(now) };
        assert_eq!(g.evaluate(KEY, &fs, &SwitchPolicy::default(), 100), Decision::Deny);

        let corners = SwitchPolicy { dead_corner_px: 20, ..Default::default() };
        assert_eq!(g.evaluate(KEY, &ctx(now), &corners, 5), Decision::Deny);
        assert_eq!(g.evaluate(KEY, &ctx(now), &corners, 20), Decision::Allow);
    }
}
