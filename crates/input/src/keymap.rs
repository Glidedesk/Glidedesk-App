//! Physical key translation: USB HID usage (wire format) ↔ macOS virtual
//! keycode ↔ Windows set-1 scancode. Physical keys (not characters) are sent,
//! so the *client's* keyboard layout decides what is typed — like a real
//! keyboard plugged into that machine.

use std::sync::OnceLock;

use nexpingdesk_proto::KeyCode;

/// Windows scancodes with the `0xE0` prefix are stored as `0xE000 | code`.
pub const EXT: u16 = 0xE000;
const NONE: u16 = 0xFFFF;

/// Media keys live outside the HID keyboard page; we give them codes after 0xE7.
pub mod media {
    pub const PLAY_PAUSE: u16 = 0xE8;
    pub const STOP: u16 = 0xE9;
    pub const PREV: u16 = 0xEA;
    pub const NEXT: u16 = 0xEB;
    pub const EJECT: u16 = 0xEC;
    pub const BRIGHTNESS_DOWN: u16 = 0xED;
    pub const BRIGHTNESS_UP: u16 = 0xEE;
}

pub mod hid {
    pub const A: u16 = 0x04;
    pub const D: u16 = 0x07;
    pub const ENTER: u16 = 0x28;
    pub const ESCAPE: u16 = 0x29;
    pub const TAB: u16 = 0x2B;
    pub const CAPS_LOCK: u16 = 0x39;
    pub const F1: u16 = 0x3A;
    pub const PRINT_SCREEN: u16 = 0x46;
    pub const SCROLL_LOCK: u16 = 0x47;
    pub const PAUSE: u16 = 0x48;
    pub const DELETE: u16 = 0x4C;
    pub const RIGHT: u16 = 0x4F;
    pub const LEFT: u16 = 0x50;
    pub const DOWN: u16 = 0x51;
    pub const UP: u16 = 0x52;
    pub const NUM_LOCK: u16 = 0x53;
    pub const F13: u16 = 0x68;
    pub const F14: u16 = 0x69;
    pub const F15: u16 = 0x6A;
    pub const MUTE: u16 = 0x7F;
    pub const VOLUME_UP: u16 = 0x80;
    pub const VOLUME_DOWN: u16 = 0x81;
    pub const LEFT_CTRL: u16 = 0xE0;
    pub const LEFT_SHIFT: u16 = 0xE1;
    pub const LEFT_ALT: u16 = 0xE2;
    pub const LEFT_META: u16 = 0xE3;
    pub const RIGHT_CTRL: u16 = 0xE4;
    pub const RIGHT_SHIFT: u16 = 0xE5;
    pub const RIGHT_ALT: u16 = 0xE6;
    pub const RIGHT_META: u16 = 0xE7;
}

/// (HID usage, macOS kVK, Windows scancode)
#[rustfmt::skip]
const TABLE: &[(u16, u16, u16)] = &[
    // letters
    (0x04, 0x00, 0x1E), (0x05, 0x0B, 0x30), (0x06, 0x08, 0x2E), (0x07, 0x02, 0x20),
    (0x08, 0x0E, 0x12), (0x09, 0x03, 0x21), (0x0A, 0x05, 0x22), (0x0B, 0x04, 0x23),
    (0x0C, 0x22, 0x17), (0x0D, 0x26, 0x24), (0x0E, 0x28, 0x25), (0x0F, 0x25, 0x26),
    (0x10, 0x2E, 0x32), (0x11, 0x2D, 0x31), (0x12, 0x1F, 0x18), (0x13, 0x23, 0x19),
    (0x14, 0x0C, 0x10), (0x15, 0x0F, 0x13), (0x16, 0x01, 0x1F), (0x17, 0x11, 0x14),
    (0x18, 0x20, 0x16), (0x19, 0x09, 0x2F), (0x1A, 0x0D, 0x11), (0x1B, 0x07, 0x2D),
    (0x1C, 0x10, 0x15), (0x1D, 0x06, 0x2C),
    // digits 1..9, 0
    (0x1E, 0x12, 0x02), (0x1F, 0x13, 0x03), (0x20, 0x14, 0x04), (0x21, 0x15, 0x05),
    (0x22, 0x17, 0x06), (0x23, 0x16, 0x07), (0x24, 0x1A, 0x08), (0x25, 0x1C, 0x09),
    (0x26, 0x19, 0x0A), (0x27, 0x1D, 0x0B),
    // editing & punctuation
    (0x28, 0x24, 0x1C), (0x29, 0x35, 0x01), (0x2A, 0x33, 0x0E), (0x2B, 0x30, 0x0F),
    (0x2C, 0x31, 0x39), (0x2D, 0x1B, 0x0C), (0x2E, 0x18, 0x0D), (0x2F, 0x21, 0x1A),
    (0x30, 0x1E, 0x1B), (0x31, 0x2A, 0x2B), (0x32, NONE, NONE), (0x33, 0x29, 0x27),
    (0x34, 0x27, 0x28), (0x35, 0x32, 0x29), (0x36, 0x2B, 0x33), (0x37, 0x2F, 0x34),
    (0x38, 0x2C, 0x35), (0x39, 0x39, 0x3A),
    // F1..F12
    (0x3A, 0x7A, 0x3B), (0x3B, 0x78, 0x3C), (0x3C, 0x63, 0x3D), (0x3D, 0x76, 0x3E),
    (0x3E, 0x60, 0x3F), (0x3F, 0x61, 0x40), (0x40, 0x62, 0x41), (0x41, 0x64, 0x42),
    (0x42, 0x65, 0x43), (0x43, 0x6D, 0x44), (0x44, 0x67, 0x57), (0x45, 0x6F, 0x58),
    // PrintScreen, ScrollLock, Pause (Mac has none; see mac_fallback)
    (0x46, NONE, EXT | 0x37), (0x47, NONE, 0x46), (0x48, NONE, NONE),
    // navigation
    (0x49, 0x72, EXT | 0x52), (0x4A, 0x73, EXT | 0x47), (0x4B, 0x74, EXT | 0x49),
    (0x4C, 0x75, EXT | 0x53), (0x4D, 0x77, EXT | 0x4F), (0x4E, 0x79, EXT | 0x51),
    (0x4F, 0x7C, EXT | 0x4D), (0x50, 0x7B, EXT | 0x4B), (0x51, 0x7D, EXT | 0x50),
    (0x52, 0x7E, EXT | 0x48),
    // keypad
    (0x53, 0x47, NONE), (0x54, 0x4B, EXT | 0x35), (0x55, 0x43, 0x37), (0x56, 0x4E, 0x4A),
    (0x57, 0x45, 0x4E), (0x58, 0x4C, EXT | 0x1C), (0x59, 0x53, 0x4F), (0x5A, 0x54, 0x50),
    (0x5B, 0x55, 0x51), (0x5C, 0x56, 0x4B), (0x5D, 0x57, 0x4C), (0x5E, 0x58, 0x4D),
    (0x5F, 0x59, 0x47), (0x60, 0x5B, 0x48), (0x61, 0x5C, 0x49), (0x62, 0x52, 0x52),
    (0x63, 0x41, 0x53), (0x67, 0x51, 0x59), (0x85, 0x5F, 0x7E),
    // ISO extra key, application/menu, power
    (0x64, 0x0A, 0x56), (0x65, 0x6E, EXT | 0x5D), (0x66, NONE, EXT | 0x5E),
    // F13..F24
    (0x68, 0x69, 0x64), (0x69, 0x6B, 0x65), (0x6A, 0x71, 0x66), (0x6B, 0x6A, 0x67),
    (0x6C, 0x40, 0x68), (0x6D, 0x4F, 0x69), (0x6E, 0x50, 0x6A), (0x6F, 0x5A, 0x6B),
    (0x70, NONE, 0x6C), (0x71, NONE, 0x6D), (0x72, NONE, 0x6E), (0x73, NONE, 0x76),
    // volume (keyboard page)
    (0x7F, 0x4A, EXT | 0x20), (0x80, 0x48, EXT | 0x30), (0x81, 0x49, EXT | 0x2E),
    // international / language
    (0x87, 0x5E, 0x73), (0x88, 0x68, 0x70), (0x89, 0x5D, 0x7D), (0x8A, NONE, 0x79),
    (0x8B, NONE, 0x7B), (0x90, 0x68, 0x72), (0x91, 0x66, 0x71),
    // modifiers
    (0xE0, 0x3B, 0x1D), (0xE1, 0x38, 0x2A), (0xE2, 0x3A, 0x38), (0xE3, 0x37, EXT | 0x5B),
    (0xE4, 0x3E, EXT | 0x1D), (0xE5, 0x3C, 0x36), (0xE6, 0x3D, EXT | 0x38), (0xE7, 0x36, EXT | 0x5C),
    // media (our codes)
    (media::PLAY_PAUSE, NONE, EXT | 0x22), (media::STOP, NONE, EXT | 0x24),
    (media::PREV, NONE, EXT | 0x10), (media::NEXT, NONE, EXT | 0x19),
    (media::EJECT, NONE, NONE), (media::BRIGHTNESS_DOWN, NONE, NONE), (media::BRIGHTNESS_UP, NONE, NONE),
];

struct Maps {
    hid_to_mac: Vec<u16>,
    hid_to_win: Vec<u16>,
    mac_to_hid: Vec<u16>,
    win_to_hid: Vec<u16>,
}

fn maps() -> &'static Maps {
    static MAPS: OnceLock<Maps> = OnceLock::new();
    MAPS.get_or_init(|| {
        let mut m = Maps {
            hid_to_mac: vec![NONE; 256],
            hid_to_win: vec![NONE; 256],
            mac_to_hid: vec![NONE; 256],
            // index: low byte + 256 when extended
            win_to_hid: vec![NONE; 512],
        };
        for &(h, mac, win) in TABLE {
            let hi = usize::from(h);
            m.hid_to_mac[hi] = mac;
            m.hid_to_win[hi] = win;
            // First mapping wins in reverse direction (e.g. two HID codes share Kana on macOS).
            if mac != NONE && m.mac_to_hid[usize::from(mac)] == NONE {
                m.mac_to_hid[usize::from(mac)] = h;
            }
            if win != NONE {
                let idx = win_index(win);
                if m.win_to_hid[idx] == NONE {
                    m.win_to_hid[idx] = h;
                }
            }
        }
        m
    })
}

fn win_index(scan: u16) -> usize {
    usize::from(scan & 0xFF) + if scan & EXT == EXT { 256 } else { 0 }
}

/// HID → macOS virtual keycode. PrintScreen/ScrollLock/Pause become F13/F14/F15
/// (their position on Apple keyboards).
#[must_use]
pub fn hid_to_mac(k: KeyCode) -> Option<u16> {
    let fallback = match k.0 {
        hid::PRINT_SCREEN => return Some(0x69),
        hid::SCROLL_LOCK => return Some(0x6B),
        hid::PAUSE => return Some(0x71),
        _ => NONE,
    };
    let v = maps().hid_to_mac.get(usize::from(k.0)).copied().unwrap_or(fallback);
    (v != NONE).then_some(v)
}

#[must_use]
pub fn mac_to_hid(code: u16) -> Option<KeyCode> {
    let v = maps().mac_to_hid.get(usize::from(code)).copied().unwrap_or(NONE);
    (v != NONE).then_some(KeyCode(v))
}

/// A macOS media / special key: the `data1` of a system-defined event with
/// subtype 8 (`NX_SUBTYPE_AUX_CONTROL_BUTTONS`) → key and "pressed". These keys
/// (volume, play, brightness, …) never arrive as key events; mouse utilities
/// also send them for buttons set to such actions. Repeats count as presses.
#[must_use]
pub fn mac_aux_key(data1: i64) -> Option<(KeyCode, bool)> {
    let nx = (data1 >> 16) & 0xFFFF;
    let down = match (data1 >> 8) & 0xFF {
        0x0A => true,
        0x0B => false,
        _ => return None,
    };
    // NX_KEYTYPE_* from IOKit's ev_keymap.h.
    let key = match nx {
        0 => hid::VOLUME_UP,
        1 => hid::VOLUME_DOWN,
        2 => media::BRIGHTNESS_UP,
        3 => media::BRIGHTNESS_DOWN,
        7 => hid::MUTE,
        14 => media::EJECT,
        16 => media::PLAY_PAUSE,
        17 | 19 => media::NEXT,
        18 | 20 => media::PREV,
        _ => return None,
    };
    Some((KeyCode(key), down))
}

/// HID → Windows scancode (`EXT` flag for `0xE0`-prefixed keys).
#[must_use]
pub fn hid_to_win(k: KeyCode) -> Option<u16> {
    let v = maps().hid_to_win.get(usize::from(k.0)).copied().unwrap_or(NONE);
    (v != NONE).then_some(v)
}

#[must_use]
pub fn win_to_hid(scan: u16, extended: bool) -> Option<KeyCode> {
    let s = (scan & 0xFF) | if extended { EXT } else { 0 };
    let v = maps().win_to_hid.get(win_index(s)).copied().unwrap_or(NONE);
    (v != NONE).then_some(KeyCode(v))
}

/// Keys Windows needs to inject by virtual-key code rather than scancode.
#[must_use]
pub const fn win_vk_only(k: KeyCode) -> Option<u16> {
    match k.0 {
        hid::PAUSE => Some(0x13),    // VK_PAUSE
        hid::NUM_LOCK => Some(0x90), // VK_NUMLOCK
        _ => None,
    }
}

/// Windows virtual-key → HID for events that arrive without a scancode
/// (media keys on some keyboards, injected input).
#[must_use]
pub const fn win_vk_to_hid(vk: u32) -> Option<KeyCode> {
    let h = match vk {
        0xAD => hid::MUTE,
        0xAE => hid::VOLUME_DOWN,
        0xAF => hid::VOLUME_UP,
        0xB0 => media::NEXT,
        0xB1 => media::PREV,
        0xB2 => media::STOP,
        0xB3 => media::PLAY_PAUSE,
        0x13 => hid::PAUSE,
        0x90 => hid::NUM_LOCK,
        0x2C => hid::PRINT_SCREEN,
        _ => return None,
    };
    Some(KeyCode(h))
}

/// HID → Linux evdev key code (`KEY_*`). X11 key codes are this + 8.
#[must_use]
pub fn hid_to_linux(k: KeyCode) -> Option<u16> {
    let explicit = match k.0 {
        hid::PRINT_SCREEN => 99,
        hid::SCROLL_LOCK => 70,
        hid::PAUSE => 119,
        0x49 => 110, // Insert
        0x4A => 102, // Home
        0x4B => 104, // PageUp
        hid::DELETE => 111,
        0x4D => 107, // End
        0x4E => 109, // PageDown
        hid::RIGHT => 106,
        hid::LEFT => 105,
        hid::DOWN => 108,
        hid::UP => 103,
        hid::NUM_LOCK => 69,
        0x54 => 98,                        // KP /
        0x58 => 96,                        // KP Enter
        0x65 => 127,                       // Menu (Compose)
        0x66 => 116,                       // Power
        0x67 => 117,                       // KP =
        0x68..=0x73 => 183 + (k.0 - 0x68), // F13–F24
        hid::MUTE => 113,
        hid::VOLUME_UP => 115,
        hid::VOLUME_DOWN => 114,
        0x85 => 121, // KP ,
        0x87 => 89,  // Ro
        0x88 => 93,  // Katakana/Hiragana
        0x89 => 124, // Yen
        0x8A => 92,  // Henkan
        0x8B => 94,  // Muhenkan
        0x90 => 122, // Hangeul
        0x91 => 123, // Hanja
        hid::LEFT_META => 125,
        hid::RIGHT_CTRL => 97,
        hid::RIGHT_ALT => 100,
        hid::RIGHT_META => 126,
        media::PLAY_PAUSE => 164,
        media::STOP => 166,
        media::PREV => 165,
        media::NEXT => 163,
        media::EJECT => 161,
        media::BRIGHTNESS_DOWN => 224,
        media::BRIGHTNESS_UP => 225,
        _ => 0,
    };
    if explicit != 0 {
        return Some(explicit);
    }
    // Everything else: evdev codes equal the non-extended AT set-1 scancodes.
    hid_to_win(k).filter(|s| s & EXT == 0 && *s < 0x59)
}

/// Linux evdev key code → HID.
#[must_use]
pub fn linux_to_hid(code: u16) -> Option<KeyCode> {
    static REV: OnceLock<Vec<u16>> = OnceLock::new();
    let rev = REV.get_or_init(|| {
        let mut v = vec![NONE; 256];
        for h in 0u16..=0xEE {
            if let Some(l) = hid_to_linux(KeyCode(h))
                && let Some(slot) = v.get_mut(usize::from(l))
                && *slot == NONE
            {
                *slot = h;
            }
        }
        v
    });
    rev.get(usize::from(code)).copied().filter(|h| *h != NONE).map(KeyCode)
}

/// Is this a modifier key (Ctrl/Shift/Alt/Meta, either side)?
#[must_use]
pub const fn is_modifier(k: KeyCode) -> bool {
    matches!(k.0, 0xE0..=0xE7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_media_keys_decode_from_system_defined_events() {
        // data1 = key << 16 | state << 8 (0x0A down, 0x0B up) | repeat.
        let ev = |nx: i64, state: i64| (nx << 16) | (state << 8);
        assert_eq!(mac_aux_key(ev(0, 0x0A)), Some((KeyCode(hid::VOLUME_UP), true)));
        assert_eq!(mac_aux_key(ev(1, 0x0B)), Some((KeyCode(hid::VOLUME_DOWN), false)));
        assert_eq!(mac_aux_key(ev(7, 0x0A)), Some((KeyCode(hid::MUTE), true)));
        assert_eq!(mac_aux_key(ev(16, 0x0A) | 1), Some((KeyCode(media::PLAY_PAUSE), true)), "repeat");
        assert_eq!(mac_aux_key(ev(17, 0x0A)), Some((KeyCode(media::NEXT), true)));
        assert_eq!(mac_aux_key(ev(20, 0x0B)), Some((KeyCode(media::PREV), false)));
        assert_eq!(mac_aux_key(ev(21, 0x0A)), None, "keyboard backlight: not forwarded");
        assert_eq!(mac_aux_key(ev(0, 0x00)), None, "not a press or release");
    }

    #[test]
    fn hid_codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (h, _, _) in TABLE {
            assert!(seen.insert(*h), "duplicate HID {h:#x}");
        }
    }

    #[test]
    fn mac_roundtrip_for_every_mapped_key() {
        for &(h, mac, _) in TABLE {
            if mac == NONE || h == 0x90 {
                continue; // LANG1 shares kVK_JIS_Kana with Intl2
            }
            assert_eq!(hid_to_mac(KeyCode(h)), Some(mac), "hid {h:#x}");
            assert_eq!(mac_to_hid(mac), Some(KeyCode(h)), "mac {mac:#x}");
        }
    }

    #[test]
    fn win_roundtrip_for_every_mapped_key() {
        for &(h, _, win) in TABLE {
            if win == NONE {
                continue;
            }
            assert_eq!(hid_to_win(KeyCode(h)), Some(win), "hid {h:#x}");
            assert_eq!(win_to_hid(win & 0xFF, win & EXT == EXT), Some(KeyCode(h)), "win {win:#x}");
        }
    }

    #[test]
    fn linux_codes() {
        assert_eq!(hid_to_linux(KeyCode(hid::A)), Some(30));
        assert_eq!(hid_to_linux(KeyCode(0x05)), Some(48), "B");
        assert_eq!(hid_to_linux(KeyCode(hid::ENTER)), Some(28));
        assert_eq!(hid_to_linux(KeyCode(hid::LEFT)), Some(105));
        assert_eq!(hid_to_linux(KeyCode(hid::RIGHT_CTRL)), Some(97));
        assert_eq!(hid_to_linux(KeyCode(0x45)), Some(88), "F12");
        assert_eq!(hid_to_linux(KeyCode(0x73)), Some(194), "F24");
        assert_eq!(hid_to_linux(KeyCode(0x5C)), Some(75), "KP4");
        // Round trip for every mapped key.
        for h in 0u16..=0xEE {
            if let Some(l) = hid_to_linux(KeyCode(h)) {
                assert_eq!(linux_to_hid(l), Some(KeyCode(h)), "hid {h:#x} → {l}");
            }
        }
    }

    #[test]
    fn well_known_keys() {
        assert_eq!(hid_to_mac(KeyCode(hid::A)), Some(0x00));
        assert_eq!(hid_to_win(KeyCode(hid::A)), Some(0x1E));
        assert_eq!(win_to_hid(0x4B, true), Some(KeyCode(hid::LEFT)));
        assert_eq!(win_to_hid(0x4B, false), Some(KeyCode(0x5C)), "keypad 4 is not Left");
        assert_eq!(hid_to_mac(KeyCode(hid::PRINT_SCREEN)), Some(0x69));
        assert_eq!(mac_to_hid(0x37), Some(KeyCode(hid::LEFT_META)));
        assert!(is_modifier(KeyCode(hid::RIGHT_ALT)));
    }
}
