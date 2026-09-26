//! Hotkey strings ("Ctrl+Alt+L") ↔ key combinations.

use std::fmt;
use std::str::FromStr;

use nexpingdesk_layout::Mods;
use nexpingdesk_proto::KeyCode;

use crate::keymap::hid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: KeyCode,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HotkeyError {
    #[error("empty hotkey")]
    Empty,
    #[error("unknown key '{0}'")]
    UnknownKey(String),
    #[error("a hotkey needs at least one of Ctrl, Alt, Shift or Cmd/Win")]
    NoModifier,
    #[error("a hotkey has exactly one non-modifier key")]
    KeyCount,
}

impl Hotkey {
    /// Does a key-down of `key` with `mods` held trigger this hotkey?
    #[must_use]
    pub fn matches(&self, key: KeyCode, mods: Mods) -> bool {
        key == self.key
            && mods.ctrl == self.ctrl
            && mods.shift == self.shift
            && mods.alt == self.alt
            && mods.meta == self.meta
    }
}

fn named_key(name: &str) -> Option<u16> {
    let n = name.to_ascii_lowercase();
    if n.len() == 1 {
        let c = n.as_bytes()[0];
        return match c {
            b'a'..=b'z' => Some(hid::A + u16::from(c - b'a')),
            b'1'..=b'9' => Some(0x1E + u16::from(c - b'1')),
            b'0' => Some(0x27),
            b'-' => Some(0x2D),
            b'=' => Some(0x2E),
            b'[' => Some(0x2F),
            b']' => Some(0x30),
            b'\\' => Some(0x31),
            b';' => Some(0x33),
            b'\'' => Some(0x34),
            b'`' => Some(0x35),
            b',' => Some(0x36),
            b'.' => Some(0x37),
            b'/' => Some(0x38),
            _ => None,
        };
    }
    if let Some(num) = n.strip_prefix('f').and_then(|d| d.parse::<u16>().ok()) {
        return match num {
            1..=12 => Some(hid::F1 + num - 1),
            13..=24 => Some(hid::F13 + num - 13),
            _ => None,
        };
    }
    Some(match n.as_str() {
        "enter" | "return" => hid::ENTER,
        "esc" | "escape" => hid::ESCAPE,
        "backspace" => 0x2A,
        "tab" => 0x2B,
        "space" => 0x2C,
        "insert" => 0x49,
        "home" => 0x4A,
        "pageup" => 0x4B,
        "delete" | "del" => hid::DELETE,
        "end" => 0x4D,
        "pagedown" => 0x4E,
        "right" => hid::RIGHT,
        "left" => hid::LEFT,
        "down" => hid::DOWN,
        "up" => hid::UP,
        "printscreen" => hid::PRINT_SCREEN,
        "scrolllock" => hid::SCROLL_LOCK,
        "pause" => hid::PAUSE,
        _ => return None,
    })
}

fn key_name(code: u16) -> String {
    match code {
        0x04..=0x1D => char::from(b'A' + u8::try_from(code - 0x04).unwrap_or(0)).to_string(),
        0x1E..=0x26 => char::from(b'1' + u8::try_from(code - 0x1E).unwrap_or(0)).to_string(),
        0x27 => "0".into(),
        0x3A..=0x45 => format!("F{}", code - hid::F1 + 1),
        0x68..=0x73 => format!("F{}", code - hid::F13 + 13),
        hid::RIGHT => "Right".into(),
        hid::LEFT => "Left".into(),
        hid::UP => "Up".into(),
        hid::DOWN => "Down".into(),
        hid::ENTER => "Enter".into(),
        hid::ESCAPE => "Esc".into(),
        0x2C => "Space".into(),
        0x2B => "Tab".into(),
        0x4A => "Home".into(),
        0x4D => "End".into(),
        0x4B => "PageUp".into(),
        0x4E => "PageDown".into(),
        hid::DELETE => "Delete".into(),
        0x49 => "Insert".into(),
        other => format!("Key{other:#04x}"),
    }
}

impl FromStr for Hotkey {
    type Err = HotkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let (mut ctrl, mut shift, mut alt, mut meta) = (false, false, false, false);
        let mut key = None;
        for p in parts {
            match p.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" | "option" | "opt" => alt = true,
                "cmd" | "command" | "meta" | "win" | "super" => meta = true,
                other => {
                    if key.is_some() {
                        return Err(HotkeyError::KeyCount);
                    }
                    key = Some(named_key(other).ok_or_else(|| HotkeyError::UnknownKey(p.to_owned()))?);
                }
            }
        }
        let key = KeyCode(key.ok_or(HotkeyError::KeyCount)?);
        if !(ctrl || shift || alt || meta || matches!(key.0, 0x3A..=0x45 | 0x68..=0x73)) {
            return Err(HotkeyError::NoModifier); // bare F-keys are allowed
        }
        Ok(Self { ctrl, shift, alt, meta, key })
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift"), (self.meta, "Cmd")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.key.0))
    }
}

/// Parses an optional hotkey setting ("" = disabled).
pub fn parse_optional(s: &str) -> Result<Option<Hotkey>, HotkeyError> {
    if s.trim().is_empty() { Ok(None) } else { s.parse().map(Some) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        let h: Hotkey = "ctrl + alt + l".parse().unwrap();
        assert!(h.ctrl && h.alt && !h.shift && !h.meta);
        assert_eq!(h.key, KeyCode(0x0F));
        assert_eq!(h.to_string(), "Ctrl+Alt+L");
        let h: Hotkey = "Cmd+Shift+F5".parse().unwrap();
        assert_eq!(h.to_string(), "Shift+Cmd+F5");
        assert_eq!("Ctrl+Alt+Right".parse::<Hotkey>().unwrap().key, KeyCode(hid::RIGHT));
        assert!("F9".parse::<Hotkey>().is_ok());
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("".parse::<Hotkey>(), Err(HotkeyError::Empty));
        assert_eq!("L".parse::<Hotkey>(), Err(HotkeyError::NoModifier));
        assert_eq!("Ctrl+A+B".parse::<Hotkey>(), Err(HotkeyError::KeyCount));
        assert!(matches!("Ctrl+Bogus".parse::<Hotkey>(), Err(HotkeyError::UnknownKey(_))));
        assert_eq!(parse_optional("  "), Ok(None));
    }

    #[test]
    fn matching_requires_exact_modifiers() {
        let h: Hotkey = "Ctrl+Alt+L".parse().unwrap();
        let m = Mods { ctrl: true, alt: true, ..Default::default() };
        assert!(h.matches(KeyCode(0x0F), m));
        assert!(!h.matches(KeyCode(0x0F), Mods { shift: true, ..m }));
        assert!(!h.matches(KeyCode(0x10), m));
    }
}
