//! Modifier translation between platforms (PLAN §3.4.2): when a Mac drives a
//! Windows PC (or the reverse) Cmd and Ctrl swap by default, so muscle memory
//! (Cmd+C / Ctrl+C) keeps working.

use glidedesk_config::RemapPreset;
use glidedesk_proto::{KeyCode, Platform};

use crate::keymap::hid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remap {
    swap_ctrl_meta: bool,
}

impl Remap {
    #[must_use]
    pub fn new(preset: RemapPreset, server: Platform, client: Platform) -> Self {
        let swap = match preset {
            RemapPreset::None => false,
            RemapPreset::SwapCtrlMeta => true,
            RemapPreset::Auto => (server == Platform::Macos) != (client == Platform::Macos),
        };
        Self { swap_ctrl_meta: swap }
    }

    #[must_use]
    pub const fn identity() -> Self {
        Self { swap_ctrl_meta: false }
    }

    #[must_use]
    pub const fn apply(self, k: KeyCode) -> KeyCode {
        if !self.swap_ctrl_meta {
            return k;
        }
        KeyCode(match k.0 {
            hid::LEFT_CTRL => hid::LEFT_META,
            hid::LEFT_META => hid::LEFT_CTRL,
            hid::RIGHT_CTRL => hid::RIGHT_META,
            hid::RIGHT_META => hid::RIGHT_CTRL,
            other => other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_swaps_only_across_platforms() {
        let mac_win = Remap::new(RemapPreset::Auto, Platform::Macos, Platform::Windows);
        assert_eq!(mac_win.apply(KeyCode(hid::LEFT_META)), KeyCode(hid::LEFT_CTRL));
        assert_eq!(mac_win.apply(KeyCode(hid::LEFT_CTRL)), KeyCode(hid::LEFT_META));
        assert_eq!(mac_win.apply(KeyCode(hid::A)), KeyCode(hid::A));
        let win_win = Remap::new(RemapPreset::Auto, Platform::Windows, Platform::Windows);
        assert_eq!(win_win.apply(KeyCode(hid::LEFT_META)), KeyCode(hid::LEFT_META));
        let forced = Remap::new(RemapPreset::SwapCtrlMeta, Platform::Windows, Platform::Windows);
        assert_eq!(forced.apply(KeyCode(hid::RIGHT_CTRL)), KeyCode(hid::RIGHT_META));
    }
}
