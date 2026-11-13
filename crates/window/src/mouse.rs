// SPDX-License-Identifier: AGPL-3.0-or-later
//! Captured host input for the configured PS/2 or serial pointing device (spec.md §9.4).
//!
//! IBM PS/2 Hardware Interface Technical Reference, Common Interfaces,
//! "Pointing Device", Data Reporting, defines positive-up counts and packet
//! button bits. Host policy uses one raw motion unit per count, carries fractions,
//! and preserves button transitions between coalesced motion segments. Assembly
//! converts serial input to Microsoft positive-down counts; each device owns
//! packet saturation or splitting. See `backlog/docs/design/observation.md`
//! § Host input for capture, release and boundary ordering.

use assembly::{inject_mouse, AssembledMachine};
use std::collections::{BTreeSet, VecDeque};
use winit::{
    event::{ElementState, MouseButton},
    keyboard::KeyCode,
};

/// Relative device counts and the current left/right/middle button bits (0/1/2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseInput {
    pub dx: i32,
    pub dy: i32,
    pub buttons: u8,
}

/// Pure capture policy and input FIFO; event handlers never borrow the machine.
/// A newly captured pointer must also be grabbed by the host window. If that
/// fails, call [`Self::release`] before accepting further events.
pub struct HostMouse {
    enabled: bool,
    captured: bool,
    focused: bool,
    inside: bool,
    keys: BTreeSet<KeyCode>,
    suppress_g: bool,
    capture_button: Option<MouseButton>,
    fraction: (f64, f64),
    buttons: u8,
    delivered_buttons: u8,
    pending: VecDeque<MouseInput>,
}

impl HostMouse {
    /// Enable capture only for a machine configured with a host mouse target.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            captured: false,
            focused: true,
            inside: false,
            keys: BTreeSet::new(),
            suppress_g: false,
            capture_button: None,
            fraction: (0.0, 0.0),
            buttons: 0,
            delivered_buttons: 0,
            pending: VecDeque::new(),
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

    /// Capture on a client-area press, swallowing both edges of that click.
    /// Returns true only when a host grab needs to be attempted.
    pub fn button_event(&mut self, button: MouseButton, state: ElementState) -> bool {
        if self.capture_button == Some(button) {
            if state == ElementState::Released {
                self.capture_button = None;
            }
            return false;
        }
        if !self.captured {
            if self.enabled && self.focused && self.inside && state == ElementState::Pressed {
                self.captured = true;
                self.capture_button = Some(button);
                return true;
            }
            return false;
        }
        let bit = match button {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
            _ => return false,
        };
        let buttons = if state == ElementState::Pressed {
            self.buttons | bit
        } else {
            self.buttons & !bit
        };
        if buttons != self.buttons {
            self.buttons = buttons;
            // A zero-motion entry separates motion on opposite sides of a click.
            self.pending.push_back(MouseInput {
                buttons,
                ..MouseInput::default()
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

    /// Focus loss releases guest buttons and discards sub-count capture residue.
    pub fn focus_changed(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
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
        if self.buttons != 0 {
            self.buttons = 0;
            self.pending.push_back(MouseInput::default());
        }
    }

    /// Queue raw relative host movement (Y down), with a fixed 1:1 scale.
    /// Finite values truncate toward zero with carried fractions. Conversion and
    /// sums saturate at i32 limits, leaving packet limits to the target device.
    /// Non-finite values are ignored per axis so one bad event cannot poison it.
    pub fn motion(&mut self, dx: f64, dy: f64) {
        if !self.captured {
            return;
        }
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
        if let Some(last) = self
            .pending
            .back_mut()
            .filter(|last| last.buttons == self.buttons)
        {
            last.dx = last.dx.saturating_add(dx);
            last.dy = last.dy.saturating_add(dy);
        } else {
            self.pending.push_back(MouseInput {
                dx,
                dy,
                buttons: self.buttons,
            });
        }
    }

    fn deliver(&mut self, mut inject: impl FnMut(MouseInput) -> bool) -> Vec<MouseInput> {
        let mut delivered = Vec::new();
        while let Some(&input) = self.pending.front() {
            if !inject(input) {
                break;
            }
            self.pending.pop_front();
            delivered.push(input);
        }
        delivered
    }

    /// Deliver segments in order immediately before the next bounded run.
    /// Button changes wait until the mouse's output is empty so a busy packet
    /// cannot swallow a subsequent press/release pair. Motion with unchanged
    /// buttons can still coalesce in the device. Keyboard readiness is separate.
    pub fn inject_at_boundary(&mut self, assembled: &mut AssembledMachine) -> Vec<MouseInput> {
        let Some(index) = assembled.mouse_device_index() else {
            return Vec::new();
        };
        let mut buttons = self.delivered_buttons;
        let delivered = self.deliver(|input| {
            if input.buttons != buttons && !assembled.mouse_input_ready() {
                return false;
            }
            if !inject_mouse(
                &mut assembled.machine,
                index,
                input.dx,
                input.dy,
                input.buttons,
            ) {
                return false;
            }
            buttons = input.buttons;
            true
        });
        self.delivered_buttons = buttons;
        delivered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ElementState::{Pressed, Released};
    use KeyCode::*;
    use MouseButton::{Left, Middle, Right};

    fn captured() -> HostMouse {
        let mut mouse = HostMouse::new(true);
        mouse.pointer_inside(true);
        assert!(mouse.button_event(Left, Pressed));
        mouse.button_event(Left, Released);
        mouse
    }

    fn drain(mouse: &mut HostMouse) -> Vec<MouseInput> {
        mouse.deliver(|_| true)
    }

    fn input(dx: i32, dy: i32, buttons: u8) -> MouseInput {
        MouseInput { dx, dy, buttons }
    }

    #[test]
    fn conversion_flips_y_and_retains_fractional_motion_across_boundaries() {
        let mut mouse = captured();
        mouse.motion(1.25, -2.5);
        assert_eq!(drain(&mut mouse), [input(1, 2, 0)]);
        mouse.motion(0.25, -0.25);
        assert!(drain(&mut mouse).is_empty());
        mouse.motion(0.5, -0.25);
        assert_eq!(drain(&mut mouse), [input(1, 1, 0)]);
        mouse.motion(-0.5, 0.5);
        mouse.motion(-0.5, 0.5);
        assert_eq!(drain(&mut mouse), [input(-1, -1, 0)]);
    }

    #[test]
    fn large_deltas_preserve_device_overflow_and_saturate_without_panics() {
        let mut mouse = captured();
        mouse.motion(1000.0, 2000.0);
        assert_eq!(drain(&mut mouse), [input(1000, -2000, 0)]);
        mouse.motion(f64::MAX, f64::MAX);
        mouse.motion(42.0, 42.0);
        assert_eq!(drain(&mut mouse), [input(i32::MAX, i32::MIN, 0)]);
        mouse.motion(f64::NEG_INFINITY, f64::NAN);
        mouse.motion(1.0, -1.0);
        assert_eq!(drain(&mut mouse), [input(1, 1, 0)]);
    }

    #[test]
    fn capture_requires_a_mouse_focus_and_client_click_and_swallows_both_edges() {
        for enabled in [false, true] {
            let mut mouse = HostMouse::new(enabled);
            mouse.motion(5.0, 5.0);
            assert!(!mouse.button_event(Left, Pressed));
            mouse.pointer_inside(true);
            mouse.focus_changed(false);
            mouse.pointer_inside(true);
            assert!(!mouse.button_event(Left, Pressed));
            mouse.focus_changed(true);
            assert!(!mouse.button_event(Left, Released));
            assert_eq!(mouse.button_event(Left, Pressed), enabled);
            assert_eq!(mouse.captured(), enabled);
            mouse.button_event(Left, Pressed);
            mouse.button_event(Left, Released);
            assert!(drain(&mut mouse).is_empty());
        }
    }

    #[test]
    fn buttons_use_packet_bits_and_keep_transitions_in_order() {
        let mut mouse = captured();
        for button in [Left, Right, Middle] {
            mouse.button_event(button, Pressed);
            mouse.button_event(button, Pressed);
        }
        mouse.button_event(MouseButton::Back, Pressed);
        for button in [Right, Left, Middle] {
            mouse.button_event(button, Released);
            mouse.button_event(button, Released);
        }
        assert_eq!(
            drain(&mut mouse),
            [1, 3, 7, 5, 4, 0].map(|b| input(0, 0, b))
        );
    }

    #[test]
    fn boundary_coalesces_motion_but_preserves_button_order_and_retries_absent_route() {
        let mut mouse = captured();
        mouse.motion(1.5, 2.0);
        mouse.motion(2.5, -3.0);
        mouse.motion(4.0, 5.0);
        mouse.button_event(Right, Pressed);
        mouse.motion(3.0, 2.0);
        mouse.motion(2.0, -4.0);
        mouse.button_event(Right, Released);
        mouse.motion(-1.0, 1.0);
        assert!(mouse.deliver(|_| false).is_empty());
        assert_eq!(
            drain(&mut mouse),
            [input(8, -4, 0), input(5, 2, 2), input(-1, -1, 0)]
        );
        assert!(drain(&mut mouse).is_empty());
    }

    #[test]
    fn release_chord_consumes_g_pair_and_repeats_but_forwards_other_keys() {
        for (ctrl, alt) in [(ControlLeft, AltLeft), (ControlRight, AltRight)] {
            let mut mouse = captured();
            assert!(!mouse.key_event(ctrl, Pressed));
            assert!(!mouse.key_event(alt, Pressed));
            assert!(!mouse.key_event(KeyA, Pressed));
            assert!(mouse.key_event(KeyG, Pressed));
            assert!(!mouse.captured());
            assert!(mouse.key_event(KeyG, Pressed));
            assert!(mouse.key_event(KeyG, Released));
            assert!(!mouse.key_event(ctrl, Released));
            assert!(!mouse.key_event(alt, Released));
            assert!(!mouse.key_event(KeyG, Pressed));
        }
    }

    #[test]
    fn release_chord_during_capture_click_swallows_release_and_allows_next_click() {
        let mut mouse = HostMouse::new(true);
        mouse.pointer_inside(true);
        assert!(mouse.button_event(Left, Pressed));
        mouse.key_event(ControlLeft, Pressed);
        mouse.key_event(AltLeft, Pressed);
        assert!(mouse.key_event(KeyG, Pressed));
        assert!(!mouse.captured());
        assert!(!mouse.button_event(Left, Released));
        assert!(drain(&mut mouse).is_empty());
        assert!(mouse.button_event(Left, Pressed));
        mouse.button_event(Left, Released);
        assert!(drain(&mut mouse).is_empty());
        mouse.button_event(Left, Pressed);
        mouse.button_event(Left, Released);
        assert_eq!(drain(&mut mouse), [input(0, 0, 1), input(0, 0, 0)]);
    }

    #[test]
    fn incomplete_chord_uncaptured_keys_and_already_held_g_are_forwarded() {
        let mut mouse = captured();
        assert!(!mouse.key_event(ControlLeft, Pressed));
        assert!(!mouse.key_event(KeyG, Pressed));
        assert!(!mouse.key_event(AltLeft, Pressed));
        assert!(!mouse.key_event(KeyG, Pressed));
        assert!(!mouse.key_event(KeyG, Released));
        assert!(mouse.captured());
        let mut mouse = HostMouse::new(false);
        for key in [ControlLeft, AltLeft, KeyG] {
            assert!(!mouse.key_event(key, Pressed));
        }
    }

    #[test]
    fn focus_loss_queues_button_release_ignores_motion_and_resets_capture_residue() {
        let mut mouse = captured();
        mouse.button_event(Left, Pressed);
        mouse.motion(2.75, -3.75);
        mouse.focus_changed(false);
        assert!(!mouse.captured());
        mouse.motion(40.0, 50.0);
        mouse.button_event(Right, Pressed);
        assert_eq!(drain(&mut mouse), [input(2, 3, 1), input(0, 0, 0)]);
        mouse.focus_changed(true);
        assert!(!mouse.button_event(Left, Pressed));
        mouse.pointer_inside(true);
        assert!(mouse.button_event(Left, Pressed));
        mouse.motion(0.5, -0.5);
        assert!(drain(&mut mouse).is_empty());
        mouse.motion(0.5, -0.5);
        assert_eq!(drain(&mut mouse), [input(1, 1, 0)]);
    }
}

#[cfg(test)]
mod serial_tests {
    use super::*;
    use assembly::{DeviceId, DeviceKind};
    use winit::event::ElementState::{Pressed, Released};

    #[test]
    fn serial_mouse_boundary_routes_y_down_and_preserves_busy_button_edges() {
        let mut config = assembly::at_boot(vec![0xf4]);
        for device in &mut config.devices {
            if let DeviceKind::Uart(c) = &mut device.kind {
                c.mouse = true;
            }
        }
        let mut assembled = assembly::assemble(&config).unwrap();
        let index = assembled.device_index[&DeviceId::Uart];
        assert_eq!(assembled.mouse_device_index(), Some(index));
        for (port, value) in [
            (0x3fb, 0x80),
            (0x3f8, 96),
            (0x3f9, 0),
            (0x3fb, 2),
            (0x3fc, 3),
        ] {
            assembled
                .machine
                .bus_mut()
                .port_write(port, 1, value)
                .unwrap();
        }
        let origin = assembled.machine.clock();
        assembled.machine.bus_mut().tick_all(origin.advance(20_881));
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x3f8, 1).unwrap(),
            u32::from(b'M')
        );
        let mut host = HostMouse::new(true);
        host.pointer_inside(true);
        assert!(host.button_event(MouseButton::Left, Pressed));
        host.button_event(MouseButton::Left, Released);
        host.motion(5.0, 7.0);
        host.button_event(MouseButton::Right, Pressed);
        host.button_event(MouseButton::Right, Released);
        let before = assembled.machine.bus().device(index).unwrap().save();
        assert_eq!(
            host.inject_at_boundary(&mut assembled),
            [MouseInput {
                dx: 5,
                dy: -7,
                buttons: 0
            }]
        );
        assert_ne!(
            assembled.machine.bus().device(index).unwrap().save(),
            before
        );
        let mut now = 20_881;
        let mut packet = |assembled: &mut AssembledMachine| {
            std::array::from_fn::<_, 3, _>(|_| {
                now += 8949;
                assembled.machine.bus_mut().tick_all(origin.advance(now));
                assembled.machine.bus_mut().port_read(0x3f8, 1).unwrap() as u8
            })
        };
        assert_eq!(packet(&mut assembled), [0x40, 5, 7]);
        assert_eq!(host.inject_at_boundary(&mut assembled).len(), 1);
        assert_eq!(packet(&mut assembled), [0x50, 0, 0]);
        assert_eq!(host.inject_at_boundary(&mut assembled).len(), 1);
        assert_eq!(packet(&mut assembled), [0x40, 0, 0]);
        assert!(host.inject_at_boundary(&mut assembled).is_empty());
    }
}
