//! A Logitech mouse's gesture button while another computer has control.
//!
//! Logi Options+ "diverts" the gesture button (and often Back / Forward / the
//! middle button): the mouse then no longer reports them as buttons but as
//! HID++ 2.0 notifications on its vendor channel — and, while the gesture
//! button is held, its motion too. Options+ turns them into Mission Control,
//! switching desktops, … on the Mac. While another computer has control the
//! mouse is seized (see `macos_hid`), so Options+ gets nothing and the gesture
//! did nothing anywhere. Here those notifications are read instead: the
//! buttons go to the other computer as buttons, and a gesture as that
//! computer's own shortcut for it ([`shortcut`]), so it acts there only.
//!
//! Only notifications are read, nothing is sent to the mouse: the feature
//! index of `REPROG_CONTROLS_V4` is learnt from the first diverted-buttons
//! notification of a device, which lists only controls known here.

use nexpingdesk_proto::{MouseButton, Platform};

use crate::keymap::hid;

/// What the gesture button did: a press without moving, or a move while held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Click,
    Up,
    Down,
    Left,
    Right,
}

/// What one HID++ report (or a move while the gesture button is held) means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Button { button: MouseButton, down: bool },
    Gesture(Gesture),
}

/// HID++ long report: id, device index, feature index, function | software id, 16 parameter bytes.
const LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
/// `REPROG_CONTROLS_V4` events (software id 0): the diverted controls held now…
const DIVERTED_BUTTONS: u8 = 0x00;
/// …and the motion of a mouse whose gesture button is held (raw counts).
const DIVERTED_RAW_XY: u8 = 0x10;

/// Control ids (CIDs).
const CID_MIDDLE: u16 = 0x0052;
const CID_BACK: u16 = 0x0053;
const CID_FORWARD: u16 = 0x0056;
const CID_GESTURE: u16 = 0x00C3;
const CID_VIRTUAL_GESTURE: u16 = 0x00D7;
const CID_SMART_SHIFT: u16 = 0x00C4;
const KNOWN_CIDS: [u16; 6] = [CID_MIDDLE, CID_BACK, CID_FORWARD, CID_GESTURE, CID_VIRTUAL_GESTURE, CID_SMART_SHIFT];

/// Movement (counts or points) while held that makes a gesture of a press.
pub const THRESHOLD: i32 = 50;

fn is_gesture(cid: u16) -> bool {
    cid == CID_GESTURE || cid == CID_VIRTUAL_GESTURE
}

fn button(cid: u16) -> Option<MouseButton> {
    match cid {
        CID_MIDDLE => Some(MouseButton::Middle),
        CID_BACK => Some(MouseButton::Back),
        CID_FORWARD => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Follows the diverted controls of every seized Logitech mouse.
#[derive(Debug, Default)]
pub struct Tracker {
    /// One per HID++ device (a receiver can pair several mice).
    mice: Vec<Mouse>,
}

/// One mouse's diverted controls.
#[derive(Debug)]
struct Mouse {
    /// HID++ device index, and its feature index of `REPROG_CONTROLS_V4`.
    device: u8,
    feature: u8,
    /// Diverted controls down now.
    held: Vec<u16>,
    /// Motion since the gesture button went down, and whether it already acted.
    gesture: Option<((i32, i32), bool)>,
}

impl Tracker {
    /// A gesture button is down: pointer motion belongs to the gesture.
    pub fn holding(&self) -> bool {
        self.mice.iter().any(|m| m.gesture.is_some())
    }

    /// One input report of a seized device, starting with its report id.
    pub fn report(&mut self, report: &[u8]) -> Vec<Action> {
        if report.len() < LONG_LEN || report[0] != LONG {
            return Vec::new();
        }
        let (device, feature, function) = (report[1], report[2], report[3]);
        let params = &report[4..LONG_LEN];
        let known = self.mice.iter().position(|m| m.device == device && m.feature == feature);
        match function {
            DIVERTED_BUTTONS => {
                let cids: Vec<u16> =
                    params[..8].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).filter(|&c| c != 0).collect();
                let i = if let Some(i) = known {
                    i
                } else {
                    // Learnt only from a press listing nothing but controls known
                    // here, and no other data: nothing else looks like that.
                    let plausible = feature != 0
                        && !cids.is_empty()
                        && cids.iter().all(|c| KNOWN_CIDS.contains(c))
                        && params[8..].iter().all(|&b| b == 0);
                    if !plausible {
                        return Vec::new();
                    }
                    self.mice.retain(|m| m.device != device);
                    self.mice.push(Mouse { device, feature, held: Vec::new(), gesture: None });
                    self.mice.len() - 1
                };
                self.mice[i].buttons(&cids)
            }
            DIVERTED_RAW_XY => {
                let Some(i) = known else { return Vec::new() };
                let dx = i16::from_be_bytes([params[0], params[1]]);
                let dy = i16::from_be_bytes([params[2], params[3]]);
                self.mice[i].motion(i32::from(dx), i32::from(dy)).map(Action::Gesture).into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    /// Pointer motion (+y = down) while a gesture button is held: it goes to
    /// that mouse's gesture. A gesture acts once per press, as soon as the
    /// motion is clear.
    pub fn motion(&mut self, dx: i32, dy: i32) -> Option<Gesture> {
        self.mice.iter_mut().find(|m| m.gesture.is_some())?.motion(dx, dy)
    }
}

impl Mouse {
    fn buttons(&mut self, now: &[u16]) -> Vec<Action> {
        let mut out = Vec::new();
        for &cid in self.held.iter().filter(|c| !now.contains(c)) {
            if is_gesture(cid) {
                if let Some((_, false)) = self.gesture.take() {
                    out.push(Action::Gesture(Gesture::Click));
                }
            } else if let Some(button) = button(cid) {
                out.push(Action::Button { button, down: false });
            }
        }
        for &cid in now.iter().filter(|c| !self.held.contains(c)) {
            if is_gesture(cid) {
                self.gesture = Some(((0, 0), false));
            } else if let Some(button) = button(cid) {
                out.push(Action::Button { button, down: true });
            }
        }
        if !now.iter().copied().any(is_gesture) {
            self.gesture = None;
        }
        self.held = now.to_vec();
        out
    }

    fn motion(&mut self, dx: i32, dy: i32) -> Option<Gesture> {
        let ((x, y), acted) = self.gesture.as_mut()?;
        if *acted {
            return None;
        }
        *x = x.saturating_add(dx);
        *y = y.saturating_add(dy);
        if x.abs().max(y.abs()) < THRESHOLD {
            return None;
        }
        *acted = true;
        Some(if x.abs() > y.abs() {
            if *x < 0 { Gesture::Left } else { Gesture::Right }
        } else if *y < 0 {
            Gesture::Up
        } else {
            Gesture::Down
        })
    }
}

/// The keys (HID usages, pressed in order, released in reverse) that do on
/// `platform` what Logi Options+ does by default for the gesture: overview
/// (click / up), show the desktop or the app's windows (down), the desktop
/// on the left / right (left / right).
#[allow(clippy::match_same_arms)] // one row per platform and gesture
pub fn shortcut(gesture: Gesture, platform: Platform) -> &'static [u16] {
    use hid::{D, DOWN, LEFT, LEFT_ALT, LEFT_CTRL, LEFT_META, RIGHT, TAB, UP};
    match (platform, gesture) {
        (Platform::Windows, Gesture::Click | Gesture::Up) => &[LEFT_META, TAB],
        (Platform::Windows, Gesture::Down) => &[LEFT_META, D],
        (Platform::Windows, Gesture::Left) => &[LEFT_CTRL, LEFT_META, LEFT],
        (Platform::Windows, Gesture::Right) => &[LEFT_CTRL, LEFT_META, RIGHT],
        (Platform::Macos, Gesture::Click | Gesture::Up) => &[LEFT_CTRL, UP],
        (Platform::Macos, Gesture::Down) => &[LEFT_CTRL, DOWN],
        (Platform::Macos, Gesture::Left) => &[LEFT_CTRL, LEFT],
        (Platform::Macos, Gesture::Right) => &[LEFT_CTRL, RIGHT],
        (Platform::Linux | Platform::Other, Gesture::Click | Gesture::Up) => &[LEFT_META],
        (Platform::Linux | Platform::Other, Gesture::Down) => &[LEFT_META, D],
        (Platform::Linux | Platform::Other, Gesture::Left) => &[LEFT_CTRL, LEFT_ALT, LEFT],
        (Platform::Linux | Platform::Other, Gesture::Right) => &[LEFT_CTRL, LEFT_ALT, RIGHT],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: u8 = 0x02;
    const FEAT: u8 = 0x09;

    fn buttons(dev: u8, feat: u8, cids: &[u16]) -> Vec<u8> {
        let mut r = vec![LONG, dev, feat, DIVERTED_BUTTONS];
        r.extend(cids.iter().flat_map(|c| c.to_be_bytes()));
        r.resize(LONG_LEN, 0);
        r
    }

    fn raw_xy(dx: i16, dy: i16) -> Vec<u8> {
        let mut r = vec![LONG, DEV, FEAT, DIVERTED_RAW_XY];
        r.extend(dx.to_be_bytes());
        r.extend(dy.to_be_bytes());
        r.resize(LONG_LEN, 0);
        r
    }

    #[test]
    fn a_press_without_moving_is_a_click() {
        let mut t = Tracker::default();
        assert!(t.report(&buttons(DEV, FEAT, &[CID_GESTURE])).is_empty());
        assert!(t.holding());
        assert!(t.report(&raw_xy(3, -2)).is_empty(), "small motion is no gesture");
        assert_eq!(t.report(&buttons(DEV, FEAT, &[])), vec![Action::Gesture(Gesture::Click)]);
        assert!(!t.holding());
    }

    #[test]
    fn a_move_while_held_is_one_gesture_in_its_main_direction() {
        let cases = [
            ((0, -60), Gesture::Up),
            ((0, 60), Gesture::Down),
            ((-60, 12), Gesture::Left),
            ((60, -12), Gesture::Right),
        ];
        for ((dx, dy), want) in cases {
            let mut t = Tracker::default();
            t.report(&buttons(DEV, FEAT, &[CID_GESTURE]));
            let mut got = Vec::new();
            for _ in 0..6 {
                got.extend(t.report(&raw_xy(dx / 6, dy / 6)));
            }
            assert_eq!(got, vec![Action::Gesture(want)], "moved by ({dx}, {dy})");
            assert!(t.report(&buttons(DEV, FEAT, &[])).is_empty(), "no click after a gesture");
        }
    }

    #[test]
    fn pointer_motion_counts_when_the_mouse_does_not_divert_it() {
        let mut t = Tracker::default();
        t.report(&buttons(DEV, FEAT, &[CID_VIRTUAL_GESTURE]));
        assert_eq!(t.motion(-30, 0), None);
        assert_eq!(t.motion(-30, 5), Some(Gesture::Left));
        assert_eq!(t.motion(-300, 0), None, "one gesture per press");
        assert_eq!(Tracker::default().motion(100, 0), None, "not held: ordinary motion");
    }

    #[test]
    fn diverted_back_and_forward_are_buttons() {
        let mut t = Tracker::default();
        let down = |b| Action::Button { button: b, down: true };
        let up = |b| Action::Button { button: b, down: false };
        assert_eq!(t.report(&buttons(DEV, FEAT, &[CID_BACK])), vec![down(MouseButton::Back)]);
        assert_eq!(t.report(&buttons(DEV, FEAT, &[CID_BACK, CID_FORWARD])), vec![down(MouseButton::Forward)]);
        assert_eq!(t.report(&buttons(DEV, FEAT, &[])), vec![up(MouseButton::Back), up(MouseButton::Forward)]);
    }

    #[test]
    fn two_mice_on_one_receiver_do_not_mix_up_their_buttons() {
        let mut t = Tracker::default();
        t.report(&buttons(1, FEAT, &[CID_GESTURE]));
        assert!(t.report(&buttons(2, FEAT, &[CID_BACK])).len() == 1, "Back of the second mouse");
        assert!(t.report(&buttons(2, FEAT, &[])).len() == 1, "its release");
        assert!(t.holding(), "the first mouse still holds its gesture button");
        assert_eq!(t.report(&buttons(1, FEAT, &[])), vec![Action::Gesture(Gesture::Click)]);
    }

    #[test]
    fn other_reports_are_ignored() {
        let mut t = Tracker::default();
        let mut unknown = buttons(DEV, FEAT, &[0x1234]);
        assert!(t.report(&unknown).is_empty(), "unknown control");
        unknown = buttons(DEV, FEAT, &[CID_GESTURE]);
        unknown[15] = 7;
        assert!(t.report(&unknown).is_empty(), "extra data: another feature's event");
        assert!(t.report(&buttons(DEV, 0, &[CID_GESTURE])).is_empty(), "root feature has no events");
        assert!(t.report(&raw_xy(100, 0)).is_empty(), "motion of a feature not learnt");
        assert!(t.report(&[LONG, DEV, FEAT]).is_empty(), "too short");
        let mut short = buttons(DEV, FEAT, &[CID_GESTURE]);
        short[0] = 0x10;
        assert!(t.report(&short).is_empty(), "not a long report");
        assert!(!t.holding());
    }

    #[test]
    fn every_platform_has_a_shortcut_for_every_gesture() {
        for p in [Platform::Windows, Platform::Macos, Platform::Linux, Platform::Other] {
            for g in [Gesture::Click, Gesture::Up, Gesture::Down, Gesture::Left, Gesture::Right] {
                assert!(!shortcut(g, p).is_empty(), "{g:?} on {p:?}");
            }
            assert_ne!(shortcut(Gesture::Left, p), shortcut(Gesture::Right, p));
        }
    }
}
