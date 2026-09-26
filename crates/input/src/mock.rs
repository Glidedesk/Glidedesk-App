//! In-memory capture/injection for tests of the core without an OS.

use std::sync::{Arc, Mutex};

use glidedesk_proto::{Input, LedState, Point};
use tokio::sync::mpsc;

use crate::{CAPTURE_QUEUE, Capture, CaptureControl, CaptureEvent, Injector, InputError, Pressed};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlCall {
    Grab(bool),
    Warp(Point),
    Stop,
    PasteHold(bool),
    ReplayPaste(glidedesk_proto::KeyCode),
}

#[derive(Debug, Default)]
pub struct MockControl {
    pub calls: Mutex<Vec<ControlCall>>,
    pub pos: Mutex<Point>,
}

impl CaptureControl for MockControl {
    fn set_grab(&self, grab: bool) {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(ControlCall::Grab(grab));
    }
    fn warp(&self, p: Point) {
        *self.pos.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = p;
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(ControlCall::Warp(p));
    }
    fn cursor(&self) -> Option<Point> {
        Some(*self.pos.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
    fn stop(&self) {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(ControlCall::Stop);
    }
    fn set_paste_hold(&self, on: bool) {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(ControlCall::PasteHold(on));
    }
    fn holds_paste(&self) -> bool {
        true
    }
    fn replay_paste(&self, key: glidedesk_proto::KeyCode) {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(ControlCall::ReplayPaste(key));
    }
}

/// Returns a capture whose events are fed by the returned sender.
#[must_use]
pub fn capture() -> (Capture, mpsc::Sender<CaptureEvent>, Arc<MockControl>) {
    let (tx, rx) = mpsc::channel(CAPTURE_QUEUE);
    let control = Arc::new(MockControl::default());
    (Capture { control: control.clone(), events: rx }, tx, control)
}

/// Records every injected event.
#[derive(Clone, Debug, Default)]
pub struct MockInjector {
    pub log: Arc<Mutex<Vec<Input>>>,
    pressed: Arc<Mutex<Pressed>>,
    pos: Arc<Mutex<Point>>,
}

impl MockInjector {
    #[must_use]
    pub fn events(&self) -> Vec<Input> {
        self.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl Injector for MockInjector {
    fn inject(&mut self, ev: &Input) -> Result<(), InputError> {
        let mut pressed = self.pressed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match *ev {
            Input::Key { key, down } => {
                pressed.key(key, down);
            }
            Input::Button { button, down } => pressed.button(button, down),
            Input::MouseAbs(p) => *self.pos.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = p,
            _ => {}
        }
        drop(pressed);
        self.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(*ev);
        if matches!(ev, Input::ReleaseAll) {
            // Same contract as the real injectors.
            self.release_all();
        }
        Ok(())
    }

    fn release_all(&mut self) {
        let (keys, buttons) = self.pressed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take_all();
        let mut log = self.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for key in keys {
            log.push(Input::Key { key, down: false });
        }
        for button in buttons {
            log.push(Input::Button { button, down: false });
        }
    }

    fn set_leds(&mut self, leds: LedState) {
        self.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Input::Leds(leds));
    }

    fn cursor(&self) -> Option<Point> {
        Some(*self.pos.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}
