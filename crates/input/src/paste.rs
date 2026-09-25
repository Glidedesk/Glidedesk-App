//! Paste interception for files that are only offered, not yet copied
//! (PLAN §14.2). When another computer offered files, this computer's paste
//! shortcut (⌘V on a Mac, Ctrl+V or Shift+Insert elsewhere) is held back. The
//! files are fetched and put on the clipboard, then the paste is replayed so the
//! file manager copies them itself.

use glidedesk_proto::{KeyCode, Platform};

use crate::keymap::hid;
use crate::modifiers::Pressed;

const V: u16 = 0x19;
const INSERT: u16 = 0x49;

/// What to do with one key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Deliver normally.
    Pass,
    /// Swallow it (part of a held paste).
    Swallow,
    /// A paste started: swallow it and fetch the offered files, then `replay`.
    Fetch,
}

/// Follows the keys typed on one computer and spots its paste shortcut.
#[derive(Clone, Debug)]
pub struct PasteGuard {
    platform: Platform,
    pressed: Pressed,
    /// The key (V / Insert) whose press we swallowed; its release is swallowed too.
    held: Option<KeyCode>,
}

impl PasteGuard {
    #[must_use]
    pub fn new(platform: Platform) -> Self {
        Self { platform, pressed: Pressed::default(), held: None }
    }

    fn chord(&self, key: KeyCode) -> Option<KeyCode> {
        let m = self.pressed.mods();
        let is_paste = match (self.platform, key.0) {
            (Platform::Macos, V) => m.meta && !m.ctrl && !m.alt,
            (_, V) => m.ctrl && !m.meta && !m.alt,
            (Platform::Macos, _) => false,
            (_, INSERT) => m.shift && !m.ctrl && !m.alt,
            _ => false,
        };
        is_paste.then_some(key)
    }

    /// Feeds one key event. `offer_pending`: files are waiting to be pasted here.
    pub fn on_key(&mut self, key: KeyCode, down: bool, offer_pending: bool) -> Verdict {
        if !down && self.held == Some(key) {
            self.held = None;
            return Verdict::Swallow;
        }
        if down && self.held == Some(key) {
            return Verdict::Swallow; // auto-repeat while fetching
        }
        let verdict = if down && offer_pending && self.held.is_none() && self.chord(key).is_some() {
            self.held = Some(key);
            Verdict::Fetch
        } else {
            Verdict::Pass
        };
        if verdict == Verdict::Pass {
            self.pressed.key(key, down);
        }
        verdict
    }

    /// Key events that perform the paste now: the paste key, wrapped in its
    /// modifier if the user already let go of it.
    #[must_use]
    pub fn replay(&self, key: KeyCode) -> Vec<(KeyCode, bool)> {
        let m = self.pressed.mods();
        let (modifier, held) = match (self.platform, key.0) {
            (Platform::Macos, _) => (hid::LEFT_META, m.meta),
            (_, INSERT) => (hid::LEFT_SHIFT, m.shift),
            _ => (hid::LEFT_CTRL, m.ctrl),
        };
        let mut out = Vec::with_capacity(4);
        if !held {
            out.push((KeyCode(modifier), true));
        }
        out.push((key, true));
        out.push((key, false));
        if !held {
            out.push((KeyCode(modifier), false));
        }
        out
    }

    /// Forget held state (link dropped, focus moved).
    pub fn reset(&mut self) {
        self.pressed = Pressed::default();
        self.held = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(v: u16) -> KeyCode {
        KeyCode(v)
    }

    #[test]
    fn cmd_v_on_a_mac_is_held_only_with_an_offer() {
        let mut g = PasteGuard::new(Platform::Macos);
        assert_eq!(g.on_key(k(hid::LEFT_META), true, true), Verdict::Pass);
        assert_eq!(g.on_key(k(V), true, false), Verdict::Pass, "no offer: normal paste");
        assert_eq!(g.on_key(k(V), false, false), Verdict::Pass);
        assert_eq!(g.on_key(k(V), true, true), Verdict::Fetch);
        assert_eq!(g.on_key(k(V), true, true), Verdict::Swallow, "repeat");
        assert_eq!(g.on_key(k(V), false, true), Verdict::Swallow);
        assert_eq!(g.replay(k(V)), vec![(k(V), true), (k(V), false)], "Cmd still held");
        assert_eq!(g.on_key(k(hid::LEFT_META), false, true), Verdict::Pass);
        assert_eq!(g.replay(k(V)).len(), 4, "Cmd released: wrapped");
    }

    #[test]
    fn pc_shortcuts() {
        let mut g = PasteGuard::new(Platform::Windows);
        let _ = g.on_key(k(hid::LEFT_META), true, true);
        assert_eq!(g.on_key(k(V), true, true), Verdict::Pass, "Win+V is the clipboard history");
        let _ = g.on_key(k(V), false, true);
        let _ = g.on_key(k(hid::LEFT_META), false, true);
        let _ = g.on_key(k(hid::RIGHT_CTRL), true, true);
        assert_eq!(g.on_key(k(V), true, true), Verdict::Fetch);
        g.reset();
        let _ = g.on_key(k(hid::LEFT_SHIFT), true, true);
        assert_eq!(g.on_key(k(INSERT), true, true), Verdict::Fetch);
        assert_eq!(g.replay(k(INSERT)), vec![(k(INSERT), true), (k(INSERT), false)]);
        let mut mac = PasteGuard::new(Platform::Macos);
        let _ = mac.on_key(k(hid::LEFT_CTRL), true, true);
        assert_eq!(mac.on_key(k(V), true, true), Verdict::Pass, "Ctrl+V is not paste on a Mac");
    }
}
