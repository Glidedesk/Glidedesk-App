//! Decides, inside the OS input callback, whether a local key event is
//! swallowed. Keys already held when the grab starts keep working locally
//! for their release, so nothing gets stuck on the server when control moves
//! to a client mid-press (e.g. holding Shift while crossing an edge).

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use glidedesk_proto::KeyCode;

#[derive(Debug, Default)]
pub(crate) struct KeyGate {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    held: HashSet<KeyCode>,
    passthrough: HashSet<KeyCode>,
}

impl KeyGate {
    /// Returns `true` when the event must be swallowed.
    pub(crate) fn on_key(&self, key: KeyCode, down: bool, grabbed: bool) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if down {
            g.held.insert(key);
        } else {
            g.held.remove(&key);
        }
        if !grabbed {
            return false;
        }
        down || !g.passthrough.remove(&key)
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
        assert!(!g.on_key(shift, true, false));
        g.on_grab();
        assert!(g.on_key(a, true, true), "new keys are swallowed");
        assert!(g.on_key(a, false, true));
        assert!(!g.on_key(shift, false, true), "pre-grab key-up reaches the local OS");
        assert!(g.on_key(shift, true, true), "pressing it again while grabbed is swallowed");
    }
}
