//! Decides, inside the OS input callback, whether a local key event is
//! swallowed. Keys already held when the grab starts keep working locally
//! for their release, so nothing gets stuck on the server when control moves
//! to a client mid-press (e.g. holding Shift while crossing an edge). Mouse
//! buttons likewise: one held down across the switch (a thumb button, a hotkey
//! switch mid-drag) would otherwise stay down here, and the next click on this
//! computer only released it — it took a second click.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};

use glidedesk_proto::KeyCode;

use crate::paste::{PasteGuard, Verdict};

#[derive(Debug)]
pub(crate) struct KeyGate {
    inner: Mutex<Inner>,
    /// Files were offered to this computer: hold its paste shortcut (§14.2).
    paste_hold: AtomicBool,
    /// Mouse buttons down now, one bit per OS button number.
    buttons: AtomicU32,
    /// Buttons that were down when the grab started: their release stays local.
    buttons_passthrough: AtomicU32,
}

impl Default for KeyGate {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                held: HashSet::new(),
                passthrough: HashSet::new(),
                paste: PasteGuard::new(glidedesk_proto::Platform::current()),
            }),
            paste_hold: AtomicBool::new(false),
            buttons: AtomicU32::new(0),
            buttons_passthrough: AtomicU32::new(0),
        }
    }
}

#[derive(Debug)]
struct Inner {
    held: HashSet<KeyCode>,
    passthrough: HashSet<KeyCode>,
    paste: PasteGuard,
}

/// Decision for one local key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GateOut {
    pub swallow: bool,
    /// The user pressed paste while files are only offered: fetch, then replay.
    pub paste: bool,
}

impl KeyGate {
    pub(crate) fn set_paste_hold(&self, on: bool) {
        self.paste_hold.store(on, Ordering::Relaxed);
    }

    /// Decides whether the event is swallowed (and whether it starts a held paste).
    pub(crate) fn on_key(&self, key: KeyCode, down: bool, grabbed: bool) -> GateOut {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if down {
            g.held.insert(key);
        } else {
            g.held.remove(&key);
        }
        if !grabbed {
            let hold = self.paste_hold.load(Ordering::Relaxed);
            return match g.paste.on_key(key, down, hold) {
                Verdict::Pass => GateOut { swallow: false, paste: false },
                Verdict::Swallow => GateOut { swallow: true, paste: false },
                Verdict::Fetch => GateOut { swallow: true, paste: true },
            };
        }
        GateOut { swallow: down || !g.passthrough.remove(&key), paste: false }
    }

    /// Records a mouse button event (`number`: the OS's button number). While
    /// grabbed, returns `true` for the release of a button held since before the
    /// grab: it must reach this computer, which got its press.
    pub(crate) fn on_button(&self, number: u32, down: bool, grabbed: bool) -> bool {
        let Some(bit) = 1u32.checked_shl(number) else { return false };
        if down {
            self.buttons.fetch_or(bit, Ordering::SeqCst);
            self.buttons_passthrough.fetch_and(!bit, Ordering::SeqCst);
            return false;
        }
        self.buttons.fetch_and(!bit, Ordering::SeqCst);
        let passthrough = self.buttons_passthrough.fetch_and(!bit, Ordering::SeqCst) & bit != 0;
        grabbed && passthrough
    }

    /// Key events that perform a paste now (after the files arrived).
    pub(crate) fn replay(&self, key: KeyCode) -> Vec<(KeyCode, bool)> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).paste.replay(key)
    }

    /// Call when the grab starts.
    pub(crate) fn on_grab(&self) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let held = g.held.clone();
        g.passthrough = held;
        self.buttons_passthrough.store(self.buttons.load(Ordering::SeqCst), Ordering::SeqCst);
        // A held paste's key-up now takes the grabbed path: don't wait for it,
        // or that key would stay swallowed for good.
        g.paste.release_held();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_held_before_grab_release_locally() {
        let g = KeyGate::default();
        let shift = KeyCode(0xE1);
        let a = KeyCode(0x04);
        let sw = |o: GateOut| o.swallow;
        assert!(!sw(g.on_key(shift, true, false)));
        g.on_grab();
        assert!(sw(g.on_key(a, true, true)), "new keys are swallowed");
        assert!(sw(g.on_key(a, false, true)));
        assert!(!sw(g.on_key(shift, false, true)), "pre-grab key-up reaches the local OS");
        assert!(sw(g.on_key(shift, true, true)), "pressing it again while grabbed is swallowed");
    }

    #[test]
    fn buttons_held_before_grab_release_locally() {
        let g = KeyGate::default();
        let (left, back) = (0, 3);
        assert!(!g.on_button(back, true, false));
        g.on_grab();
        assert!(!g.on_button(left, true, true), "new presses go to the client");
        assert!(!g.on_button(left, false, true));
        assert!(g.on_button(back, false, true), "pre-grab release reaches the local OS");
        assert!(!g.on_button(back, true, true), "pressed again while grabbed: the client's");
        assert!(!g.on_button(back, false, true));
        assert!(!g.on_button(40, false, true), "button numbers past 31 are never passed");
    }
}
