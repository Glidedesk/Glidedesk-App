//! Tracks held keys/buttons so modifiers can be reported and everything can
//! be released when a link drops (no stuck keys).

use std::collections::BTreeSet;

use nexpingdesk_layout::Mods;
use nexpingdesk_proto::{KeyCode, MouseButton};

use crate::keymap::hid;

#[derive(Clone, Debug, Default)]
pub struct Pressed {
    keys: BTreeSet<KeyCode>,
    buttons: Vec<MouseButton>,
}

impl Pressed {
    /// Records a key; returns `false` for a repeat of an already-held key.
    pub fn key(&mut self, key: KeyCode, down: bool) -> bool {
        if down { self.keys.insert(key) } else { self.keys.remove(&key) }
    }

    pub fn button(&mut self, b: MouseButton, down: bool) {
        if down {
            if !self.buttons.contains(&b) {
                self.buttons.push(b);
            }
        } else {
            self.buttons.retain(|x| *x != b);
        }
    }

    #[must_use]
    pub fn mods(&self) -> Mods {
        let has = |a, b| self.keys.contains(&KeyCode(a)) || self.keys.contains(&KeyCode(b));
        Mods {
            ctrl: has(hid::LEFT_CTRL, hid::RIGHT_CTRL),
            shift: has(hid::LEFT_SHIFT, hid::RIGHT_SHIFT),
            alt: has(hid::LEFT_ALT, hid::RIGHT_ALT),
            meta: has(hid::LEFT_META, hid::RIGHT_META),
        }
    }

    #[must_use]
    pub fn any_button(&self) -> bool {
        !self.buttons.is_empty()
    }

    #[must_use]
    pub fn buttons(&self) -> &[MouseButton] {
        &self.buttons
    }

    #[must_use]
    pub fn is_down(&self, key: KeyCode) -> bool {
        self.keys.contains(&key)
    }

    /// Drains everything held: non-modifiers first, then modifiers.
    pub fn take_all(&mut self) -> (Vec<KeyCode>, Vec<MouseButton>) {
        let mut keys: Vec<KeyCode> = std::mem::take(&mut self.keys).into_iter().collect();
        keys.sort_by_key(|k| crate::keymap::is_modifier(*k));
        (keys, std::mem::take(&mut self.buttons))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_and_releases() {
        let mut p = Pressed::default();
        assert!(p.key(KeyCode(hid::LEFT_CTRL), true));
        assert!(p.key(KeyCode(hid::A), true));
        assert!(!p.key(KeyCode(hid::A), true), "repeat");
        p.button(MouseButton::Left, true);
        assert!(p.mods().ctrl && !p.mods().shift);
        let (keys, buttons) = p.take_all();
        assert_eq!(keys, vec![KeyCode(hid::A), KeyCode(hid::LEFT_CTRL)], "modifiers released last");
        assert_eq!(buttons, vec![MouseButton::Left]);
        assert!(!p.any_button() && !p.mods().ctrl);
    }
}
