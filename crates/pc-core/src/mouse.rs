// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pointer input for the configured PS/2 or serial pointing device (spec.md §9.4).
//!
//! IBM PS/2 Hardware Interface Technical Reference, Common Interfaces,
//! "Pointing Device", Data Reporting, defines positive-up counts and packet
//! button bits. Motion arrives in counts already (the front end carries host
//! fractions); this queue preserves button transitions between coalesced
//! motion segments. Assembly converts serial input to Microsoft positive-down
//! counts; each device owns packet saturation or splitting. See
//! `backlog/docs/design/observation.md` § Host input for capture, release and
//! boundary ordering.

use assembly::{inject_mouse, AssembledMachine};
use std::collections::VecDeque;

/// Relative device counts and the current left/right/middle button bits (0/1/2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseInput {
    pub dx: i32,
    pub dy: i32,
    pub buttons: u8,
}

/// The input FIFO of the pointing device; queueing never borrows the machine.
#[derive(Debug, Default)]
pub struct PcMouse {
    buttons: u8,
    delivered_buttons: u8,
    pending: VecDeque<MouseInput>,
}

impl PcMouse {
    /// One button's transition: 0 left, 1 right, 2 middle; others are ignored.
    pub fn button(&mut self, button: u8, pressed: bool) {
        let bit = match button {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => return,
        };
        let buttons = if pressed {
            self.buttons | bit
        } else {
            self.buttons & !bit
        };
        self.set_buttons(buttons);
    }

    fn set_buttons(&mut self, buttons: u8) {
        if buttons != self.buttons {
            self.buttons = buttons;
            // A zero-motion entry separates motion on opposite sides of a click.
            self.pending.push_back(MouseInput {
                buttons,
                ..MouseInput::default()
            });
        }
    }

    /// Keep queued input, then release every guest button at the next boundary.
    pub fn release(&mut self) {
        self.set_buttons(0);
    }

    /// Queue relative movement in counts, positive Y up. Sums saturate at i32
    /// limits, leaving packet limits to the target device.
    pub fn motion(&mut self, dx: i32, dy: i32) {
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

    /// Deliver queued segments in order while `inject` accepts them.
    pub fn deliver(&mut self, mut inject: impl FnMut(MouseInput) -> bool) -> Vec<MouseInput> {
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
