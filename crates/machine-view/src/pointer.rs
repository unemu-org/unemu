// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host pointer capture for a machine's relative pointing device (spec.md §9.4).
//!
//! A machine with a relative pointer captures on a client-area click, and
//! that click is swallowed. Command+G on macOS (Ctrl+Alt+G elsewhere), focus
//! loss and suspension release capture and the guest's buttons. Raw host
//! motion uses one host unit per device count, positive Y up, with carried
//! fractions; the machine owns coalescing, back-pressure and overflow. See
//! `backlog/docs/design/observation.md` § Host input for capture, release and
//! boundary ordering.

use std::collections::BTreeSet;
use unemu_core_api::{InputEvent, PortId};
use winit::{
    event::{ElementState, MouseButton},
    keyboard::KeyCode,
};

/// Pure capture policy; event handlers never borrow the machine. Guest
/// events wait in [`PointerCapture::take_events`] until the session hands
/// them to the machine. A newly captured pointer must also be grabbed by the
/// host window. If that fails, call [`Self::release`] before accepting
/// further events.
pub struct PointerCapture {
    port: Option<PortId>,
    captured: bool,
    focused: bool,
    inside: bool,
    keys: BTreeSet<KeyCode>,
    suppress_g: bool,
    capture_button: Option<MouseButton>,
    fraction: (f64, f64),
    /// The guest's held buttons as packet bits: left 1, right 2, middle 4.
    buttons: u8,
    events: Vec<InputEvent>,
}

impl PointerCapture {
    /// Capture only for a machine with a relative pointer on `port`.
    pub fn new(port: Option<PortId>) -> Self {
        Self {
            port,
            captured: false,
            focused: true,
            inside: false,
            keys: BTreeSet::new(),
            suppress_g: false,
            capture_button: None,
            fraction: (0.0, 0.0),
            buttons: 0,
            events: Vec::new(),
        }
    }

    /// Whether raw movement and subsequent clicks belong to the guest.
    pub fn captured(&self) -> bool {
        self.captured
    }

    /// Client-area presence gates the initial capture click only.
    pub fn pointer_inside(&mut self, inside: bool) {
        self.inside = inside;
    }

    /// The guest events queued since the last call, in order.
    pub fn take_events(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.events)
    }

    /// Capture on a client-area press, swallowing both edges of that click.
    /// Returns true only when a host grab needs to be attempted.
    pub fn button_event(&mut self, button: MouseButton, state: ElementState) -> bool {
        if self.capture_button == Some(button) {
            if state == ElementState::Released {
                self.capture_button = None;
            }
            return false;
        }
        let Some(port) = self.port else {
            return false;
        };
        if !self.captured {
            if self.focused && self.inside && state == ElementState::Pressed {
                self.captured = true;
                self.capture_button = Some(button);
                return true;
            }
            return false;
        }
        let (number, bit) = match button {
            MouseButton::Left => (0, 1),
            MouseButton::Right => (1, 2),
            MouseButton::Middle => (2, 4),
            _ => return false,
        };
        let pressed = state == ElementState::Pressed;
        let buttons = if pressed {
            self.buttons | bit
        } else {
            self.buttons & !bit
        };
        if buttons != self.buttons {
            self.buttons = buttons;
            self.events.push(InputEvent::PointerButton {
                port,
                button: number,
                pressed,
            });
        }
        false
    }

    /// Consume the G pair of Ctrl+Alt+G while captured. Modifier keys continue
    /// through the ordinary keyboard path, including their eventual releases.
    /// A G already held by the guest is never stolen by a later modifier press.
    pub fn key_event(&mut self, key: KeyCode, state: ElementState) -> bool {
        use KeyCode::*;
        let fresh = if state == ElementState::Pressed {
            self.keys.insert(key)
        } else {
            self.keys.remove(&key);
            false
        };
        if key == KeyG && self.suppress_g {
            if state == ElementState::Released {
                self.suppress_g = false;
            }
            return true;
        }
        if self.captured
            && key == KeyG
            && state == ElementState::Pressed
            && fresh
            && [ControlLeft, ControlRight]
                .iter()
                .any(|key| self.keys.contains(key))
            && [AltLeft, AltRight]
                .iter()
                .any(|key| self.keys.contains(key))
        {
            self.suppress_g = true;
            self.release();
            return true;
        }
        false
    }

    /// Resume a capture that a host dialog interrupted, without a capture
    /// click. Returns true only when a host grab needs to be attempted.
    pub fn recapture(&mut self) -> bool {
        if self.captured || self.port.is_none() || !self.focused {
            return false;
        }
        self.captured = true;
        true
    }

    /// Focus loss releases guest buttons and discards sub-count capture
    /// residue. With `released_by_machine` the session has already told the
    /// machine to release everything ([`InputEvent::ReleaseAll`]), so the
    /// buttons go up there in one step and no event is queued here.
    pub fn focus_changed(&mut self, focused: bool, released_by_machine: bool) {
        self.focused = focused;
        if !focused {
            if released_by_machine {
                self.buttons = 0;
            }
            self.release();
            self.inside = false;
            self.keys.clear();
            self.suppress_g = false;
            self.capture_button = None;
        }
    }

    /// Keep queued input, then release any guest buttons at the next boundary.
    pub fn release(&mut self) {
        self.captured = false;
        self.fraction = (0.0, 0.0);
        let Some(port) = self.port else {
            return;
        };
        for (number, bit) in [(0, 1), (1, 2), (2, 4)] {
            if self.buttons & bit != 0 {
                self.events.push(InputEvent::PointerButton {
                    port,
                    button: number,
                    pressed: false,
                });
            }
        }
        self.buttons = 0;
    }

    /// Queue raw relative host movement (Y down), with a fixed 1:1 scale.
    /// Finite values truncate toward zero with carried fractions. Conversion
    /// saturates at i32 limits, leaving packet limits to the target device.
    /// Non-finite values are ignored per axis so one bad event cannot poison it.
    pub fn motion(&mut self, dx: f64, dy: f64) {
        let Some(port) = self.port.filter(|_| self.captured) else {
            return;
        };
        fn counts(delta: f64, fraction: &mut f64) -> i32 {
            if !delta.is_finite() {
                return 0;
            }
            let total = delta + *fraction;
            let whole = total.trunc();
            *fraction = total - whole;
            whole as i32
        }
        let dx = counts(dx, &mut self.fraction.0);
        let dy = counts(-dy, &mut self.fraction.1);
        if dx == 0 && dy == 0 {
            return;
        }
        self.events.push(InputEvent::PointerMotion { port, dx, dy });
    }
}
