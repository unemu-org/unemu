// SPDX-License-Identifier: AGPL-3.0-or-later
//! Microsoft two-button serial mouse on a UART's receive line.
//!
//! Microsoft Mouse Programmer's Reference (serial protocol requirements):
//! 1200 bit/s, 7N1, identification M, then 01LRYYXX / X / Y packets, with signed eight-bit motion and positive Y down. Microchip AN519,
//! Table 1, Microsoft Format, also describes the three-byte format. See
//! `backlog/docs/design/machine-files.md` §Serial mouse and
//! `backlog/docs/design/observation.md` §Captured PS/2 mouse.
//!
//! | Hardware behaviour | Coverage |
//! | --- | --- |
//! | DTR and RTS power/reset | Both must be high; either low cancels unsent output and motion |
//! | Power-on identification | M after ASSUMED 10 ms startup, plus one serial frame |
//! | Motion and buttons | Two buttons, signed -128..127 per axis, excess split in order |
//! | Line format | Fixed 1200 7N1; UART owns byte timing and mismatch errors |
//! | Host accumulation | Saturating i32 counts; latest buttons coalesce while busy |
//! | Logitech extensions | Not modelled: no M3, middle button or fourth byte |
//! | Electrical signalling | No voltage, bit sampling, acceleration or physical resolution |
//!
//! Startup is 11932 PIT clocks (10 ms rounded up); it is a modelling
//! assumption rather than a measured mouse power-on interval. Host input while
//! unpowered drops motion but remembers current buttons, so a held button is
//! reported after identification. Snapshots include the power lines, startup
//! deadline, pending packet, counters and button state, identified by a model tag.

use device::clock::{ClockDomain, Ratio};
use std::{any::Any, collections::VecDeque};

use device::{DeviceError, VClock};

use crate::uart16550::{SerialFormat, SerialPeripheral};

const MODEL: &str = "microsoft-serial-mouse";
/// Assumed 10 ms startup, converted through the exact board clock.
pub const STARTUP_CLOCKS: u64 = ClockDomain::from_hz(Ratio::new(100, 1)).ticks(1);

/// Microsoft serial protocol endpoint; no ISA register decoder of its own.
#[derive(Clone, Default)]
pub struct SerialMouse {
    dtr: bool,
    rts: bool,
    ready: VClock,
    dx: i32,
    dy: i32,
    buttons: u8,
    sent_buttons: u8,
    pending: VecDeque<u8>,
}

impl SerialMouse {
    /// Construct an unpowered two-button mouse.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulate device counts (positive Y down), left/right in bits 0/1.
    pub fn inject_motion(&mut self, dx: i32, dy: i32, buttons: u8) {
        self.buttons = buttons & 3;
        if self.dtr && self.rts {
            self.dx = self.dx.saturating_add(dx);
            self.dy = self.dy.saturating_add(dy);
        }
    }

    fn changed(&self) -> bool {
        self.dx != 0 || self.dy != 0 || self.buttons != self.sent_buttons
    }
}

impl SerialPeripheral for SerialMouse {
    fn name(&self) -> &'static str {
        MODEL
    }

    fn format(&self) -> SerialFormat {
        SerialFormat {
            baud: 1200,
            data_bits: 7,
            parity: false,
            stop_bits: 1,
        }
    }

    fn modem_control(&mut self, dtr: bool, rts: bool, now: VClock) -> bool {
        let was_powered = self.dtr && self.rts;
        self.dtr = dtr;
        self.rts = rts;
        if !(dtr && rts) {
            self.pending.clear();
            self.dx = 0;
            self.dy = 0;
            self.ready = VClock::ZERO;
        } else if !was_powered {
            self.ready = VClock(now.0.saturating_add(STARTUP_CLOCKS));
            self.pending.push_back(b'M');
            self.sent_buttons = 0;
        }
        was_powered && !(dtr && rts)
    }

    fn ready_at(&self) -> Option<VClock> {
        (self.dtr && self.rts && (!self.pending.is_empty() || self.changed())).then_some(self.ready)
    }

    fn read_byte(&mut self) -> Option<u8> {
        self.ready_at()?;
        if self.pending.is_empty() {
            let x = self.dx.clamp(-128, 127);
            let y = self.dy.clamp(-128, 127);
            self.dx -= x;
            self.dy -= y;
            let (x, y) = (x as u8, y as u8);
            self.pending.extend([
                0x40 | ((self.buttons & 1) << 5)
                    | ((self.buttons & 2) << 3)
                    | ((y >> 6) << 2)
                    | (x >> 6),
                x & 0x3f,
                y & 0x3f,
            ]);
            self.sent_buttons = self.buttons;
        }
        self.pending.pop_front()
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn save(&self) -> Vec<u8> {
        let mut data = vec![
            1,
            u8::from(self.dtr),
            u8::from(self.rts),
            self.buttons,
            self.sent_buttons,
        ];
        data.extend(self.ready.0.to_le_bytes());
        data.extend(self.dx.to_le_bytes());
        data.extend(self.dy.to_le_bytes());
        data.push(self.pending.len() as u8);
        data.extend(&self.pending);
        data
    }

    fn restore(&mut self, data: &[u8], dtr: bool, rts: bool) -> Result<(), DeviceError> {
        let invalid = || DeviceError::InvalidSnapshot {
            device: MODEL,
            reason: "invalid serial mouse state",
        };
        if data.len() < 22
            || data[0] != 1
            || data[1] > 1
            || data[2] > 1
            || data[3] > 3
            || data[4] > 3
            || data[21] > 2
            || data.len() != 22 + usize::from(data[21])
            || data[22..].iter().any(|b| *b > 0x7f)
        {
            return Err(invalid());
        }
        let next = Self {
            dtr: data[1] != 0,
            rts: data[2] != 0,
            buttons: data[3],
            sent_buttons: data[4],
            ready: VClock(u64::from_le_bytes(std::array::from_fn(|i| data[5 + i]))),
            dx: i32::from_le_bytes(std::array::from_fn(|i| data[13 + i])),
            dy: i32::from_le_bytes(std::array::from_fn(|i| data[17 + i])),
            pending: data[22..].iter().copied().collect(),
        };
        if next.dtr != dtr || next.rts != rts {
            return Err(invalid());
        }
        if !(next.dtr && next.rts)
            && (!next.pending.is_empty()
                || next.dx != 0
                || next.dy != 0
                || next.ready != VClock::ZERO)
        {
            return Err(invalid());
        }
        *self = next;
        Ok(())
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod clock_domain_regressions {
    use super::*;

    #[test]
    fn clock_domain_mouse_startup_uses_elapsed_board_time() {
        let mut mouse = SerialMouse::new();
        mouse.modem_control(true, true, VClock::ZERO);
        assert_eq!(
            mouse.ready_at(),
            Some(ClockDomain::from_hz(Ratio::new(100, 1)).edge_at(1))
        );
    }
}
