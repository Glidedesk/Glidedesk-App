//! Decides, inside the OS input callback, whether a local key event is
//! swallowed. Keys already held when the grab starts keep working locally
//! for their release, so nothing gets stuck on the server when control moves
//! to a client mid-press (e.g. holding Shift while crossing an edge).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use glidedesk_proto::KeyCode;

use crate::paste::{PasteGuard, Verdict};

#[derive(Debug)]
pub(crate) struct KeyGate {
    inner: Mutex<Inner>,
    /// Files were offered to this computer: hold its paste shortcut (§14.2).
    paste_hold: AtomicBool,
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

    /// Key events that perform a paste now (after the files arrived).
    pub(crate) fn replay(&self, key: KeyCode) -> Vec<(KeyCode, bool)> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).paste.replay(key)
    }

    /// Call when the grab starts.
    pub(crate) fn on_grab(&self) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let held = g.held.clone();
        g.passthrough = held;
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
}
