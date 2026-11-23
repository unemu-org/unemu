// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests of the desktop's pointer path as a session joins its halves: the
//! capture policy of `machine_view::pointer` and the PC's pointing-device
//! queue of `unemu_pc_core::mouse` (IBM PS/2 Hardware Interface Technical
//! Reference, Common Interfaces, "Pointing Device", Data Reporting). The
//! session hands every captured event to the machine at once and releases
//! everything on focus loss; this shim does the same without a session.

use assembly::AssembledMachine;
use machine_view::pointer::PointerCapture;
use unemu_core_api::{InputEvent, PortId};
use unemu_pc_core::{MouseInput, PcMouse};
use winit::{
    event::{ElementState, MouseButton},
    keyboard::KeyCode,
};

/// The capture policy feeding the PC's pointer queue.
struct HostMouse {
    capture: PointerCapture,
    device: PcMouse,
}

impl HostMouse {
    fn new(enabled: bool) -> Self {
        Self {
            capture: PointerCapture::new(enabled.then_some((PortId(1), 3))),
            device: PcMouse::default(),
        }
    }

    fn forward(&mut self) {
        for event in self.capture.take_events() {
            match event {
                InputEvent::PointerMotion { dx, dy, .. } => self.device.motion(dx, dy),
                InputEvent::PointerButton {
                    button, pressed, ..
                } => self.device.button(button, pressed),
                _ => {}
            }
        }
    }

    fn captured(&self) -> bool {
        self.capture.captured()
    }

    fn pointer_inside(&mut self, inside: bool) {
        self.capture.pointer_inside(inside);
    }

    fn button_event(&mut self, button: MouseButton, state: ElementState) -> bool {
        let grab = self.capture.button_event(button, state);
        self.forward();
        grab
    }

    fn key_event(&mut self, key: KeyCode, state: ElementState) -> bool {
        let taken = self.capture.key_event(key, state);
        self.forward();
        taken
    }

    fn recapture(&mut self) -> bool {
        self.capture.recapture()
    }

    /// As a running session: focus loss releases everything in the machine.
    fn focus_changed(&mut self, focused: bool) {
        self.capture.focus_changed(focused, !focused);
        self.forward();
        if !focused {
            self.device.release();
        }
    }

    fn motion(&mut self, dx: f64, dy: f64) {
        self.capture.motion(dx, dy);
        self.forward();
    }

    fn deliver(&mut self, inject: impl FnMut(MouseInput) -> bool) -> Vec<MouseInput> {
        self.device.deliver(inject)
    }

    fn inject_at_boundary(&mut self, assembled: &mut AssembledMachine) -> Vec<MouseInput> {
        self.device.inject_at_boundary(assembled)
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

    #[test]
    fn recapture_resumes_an_interrupted_capture_only_for_a_focused_mouse() {
        assert!(!HostMouse::new(false).recapture());
        let mut mouse = captured();
        assert!(!mouse.recapture());
        mouse.focus_changed(false);
        assert!(!mouse.recapture());
        assert!(!mouse.captured());
        mouse.focus_changed(true);
        assert!(mouse.recapture());
        assert!(mouse.captured());
        // No capture click is pending, so the next press reaches the guest.
        mouse.button_event(Left, Pressed);
        assert_eq!(drain(&mut mouse), [input(0, 0, 1)]);
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
        let mut assembled = machine_file::assemble(&config).unwrap();
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
