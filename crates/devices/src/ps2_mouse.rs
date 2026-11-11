// SPDX-License-Identifier: AGPL-3.0-or-later
//! Standard PS/2 mouse on the 8042 auxiliary serial link (§7.5).
//!
//! IBM Personal System/2 Hardware Interface Technical Reference, Common
//! Interfaces, "Pointing Device", Operating Modes, Data Reporting and Commands,
//! defines the command exchanges, signed counters and nonlinear 2:1 scaling.
//! See `backlog/docs/design/spec.md` §7.5. Host motion is already in device
//! counts: X positive is right, Y positive is up; resolution is a reported
//! setting, not a conversion from host pixels or millimetres.
//!
//! | Commands | Coverage |
//! | --- | --- |
//! | FF, F6 | Reset/BAT/ID and defaults (100 samples/s, resolution 2, 1:1, disabled stream) |
//! | F4, F5, F3 + byte | Reporting enable/disable; sample rates 10, 20, 40, 60, 80, 100, 200 |
//! | E8 + byte, E6, E7, E9, F2 | Resolution 0–3, scaling, three-byte status and ID 00 |
//! | EA, F0, EB | Stream, remote, unscaled read-data packet |
//! | EE, EC | Wrap echo and return to the preceding mode (stream disabled); FF still resets |
//! | FE | Resend the last transmitted byte, without an ACK |
//! | Invalid command/argument | FE on first error, FC on a consecutive error; valid input clears the error |
//!
//! Movement accumulates in signed counters, saturating at -256/+255 and latching
//! overflow until a packet or command clears them. The 2:1 table is applied to
//! stream packets only; EB read-data bypasses scaling. Three buttons are
//! supported (middle is an extension of the IBM two-button device). Status E9
//! uses left/middle/right in bits 2/1/0, unlike packet left/right/middle bits 0/1/2.
//!
//! Stream packets are generated at input injection boundaries, with changes
//! coalesced while output is pending. Sample rate is stored/reported, but does
//! not pace packets in virtual time. This deliberate coverage gap avoids
//! inventing a host time base at this device-only seam. Bit-level signalling,
//! BAT delays, physical resolution conversion, wheel IDs, full-packet resend
//! (FE repeats one byte here) and parity faults are
//! not modelled. A 16-byte output queue retains older replies on overflow and
//! drops a whole new reply. A second invalid argument abandons the parameter
//! exchange; further invalid bytes return FC until valid input arrives. This
//! recovery policy makes a malformed parameter exchange bounded.
//! Snapshots include counters, buttons, modes, partial arguments, error/resend
//! state and queued output; the controller owns serial delivery timing. Mouse
//! snapshot version 2 also identifies autonomous output so interruption behaves
//! identically after restore.
//!
//! IBM Personal System/2 Hardware Interface Technical Reference, Common
//! Interfaces, "Pointing Device", Commands and Data Transmission (Data Input),
//! permits the host to override output to send a command; ACK supersedes other
//! mouse output. ASSUMED at this byte-level seam: discard the unsent remainder
//! of an autonomous packet when host input arrives, giving the response priority.
//! Bytes already delivered to the controller remain there. Command replies keep
//! their existing FIFO policy. Commands clear movement counters except FE;
//! EB reports the accumulated motion before clearing it.
//!
//! The same manual, Commands, "Reset Wrap Mode (Hex EC)", returns to the
//! preceding mode, with stream reporting disabled on leaving wrap. EC outside
//! wrap leaves reporting unchanged. FE is interpreted before a pending argument
//! and resends the last byte without consuming that argument or clearing motion.

use std::{any::Any, collections::VecDeque};

use device::DeviceError;

use crate::i8042::AuxDevice;

const NAME: &str = "ps2-mouse";
const CAPACITY: usize = 16;
const ACK: u8 = 0xFA;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Argument {
    None,
    SampleRate,
    Resolution,
}

/// Three-byte PS/2 mouse, attached through [`AuxDevice`], not an ISA port decoder.
/// IBM PS/2 Hardware Interface Technical Reference, Common Interfaces,
/// "Pointing Device", Commands and Data Reporting; see module coverage notes.
#[derive(Clone)]
pub struct Ps2Mouse {
    reporting: bool,
    remote: bool,
    wrap: bool,
    scaling: bool,
    sample_rate: u8,
    resolution: u8,
    argument: Argument,
    error: bool,
    last_sent: Option<u8>,
    dx: i16,
    dy: i16,
    overflow_x: bool,
    overflow_y: bool,
    buttons: u8,
    reported_buttons: u8,
    output: VecDeque<u8>,
    /// Only autonomous packets may be abandoned when the host requests a reply.
    stream_output: bool,
}

impl Ps2Mouse {
    /// Power-on defaults, without an unsolicited BAT before a host reset.
    #[must_use]
    pub fn new() -> Self {
        Self {
            reporting: false,
            remote: false,
            wrap: false,
            scaling: false,
            sample_rate: 100,
            resolution: 2,
            argument: Argument::None,
            error: false,
            last_sent: None,
            dx: 0,
            dy: 0,
            overflow_x: false,
            overflow_y: false,
            buttons: 0,
            reported_buttons: 0,
            output: VecDeque::new(),
            stream_output: false,
        }
    }

    /// Accumulate device counts and current buttons (left/right/middle bits 0/1/2).
    /// Y positive is up. Remote/disabled/wrap modes retain input until read or
    /// cleared by a command. No host clock or machine input route is involved.
    pub fn inject_motion(&mut self, dx: i32, dy: i32, buttons: u8) {
        Self::accumulate(&mut self.dx, &mut self.overflow_x, dx);
        Self::accumulate(&mut self.dy, &mut self.overflow_y, dy);
        self.buttons = buttons & 7;
        self.stream_packet();
    }

    fn accumulate(counter: &mut i16, overflow: &mut bool, delta: i32) {
        if !*overflow {
            let total = i64::from(*counter) + i64::from(delta);
            *overflow = !(-256..=255).contains(&total);
            *counter = total.clamp(-256, 255) as i16;
        }
    }

    fn clear_motion(&mut self) {
        self.dx = 0;
        self.dy = 0;
        self.overflow_x = false;
        self.overflow_y = false;
    }

    fn defaults(&mut self) {
        self.reporting = false;
        self.remote = false;
        self.wrap = false;
        self.scaling = false;
        self.sample_rate = 100;
        self.resolution = 2;
        self.argument = Argument::None;
        self.clear_motion();
    }

    fn queue(&mut self, bytes: &[u8]) -> bool {
        if self.output.len() + bytes.len() > CAPACITY {
            return false;
        }
        self.output.extend(bytes);
        true
    }

    fn scaled(value: i16) -> i16 {
        let magnitude = match value.abs() {
            0 => 0,
            1 | 2 => 1,
            3 => 3,
            4 => 6,
            5 => 9,
            n => n * 2,
        };
        magnitude * value.signum()
    }

    fn packet(&self, scale: bool) -> [u8; 3] {
        let x = if scale {
            Self::scaled(self.dx)
        } else {
            self.dx
        };
        let y = if scale {
            Self::scaled(self.dy)
        } else {
            self.dy
        };
        let flags = 8
            | self.buttons
            | (u8::from(x < 0) << 4)
            | (u8::from(y < 0) << 5)
            | (u8::from(self.overflow_x || !(-256..=255).contains(&x)) << 6)
            | (u8::from(self.overflow_y || !(-256..=255).contains(&y)) << 7);
        [flags, x.clamp(-256, 255) as u8, y.clamp(-256, 255) as u8]
    }

    fn stream_packet(&mut self) {
        if self.reporting
            && !self.remote
            && !self.wrap
            && self.argument == Argument::None
            && self.output.is_empty()
            && (self.dx != 0 || self.dy != 0 || self.buttons != self.reported_buttons)
        {
            self.queue(&self.packet(self.scaling));
            self.stream_output = true;
            self.clear_motion();
            self.reported_buttons = self.buttons;
        }
    }

    fn invalid(&mut self) {
        self.clear_motion();
        self.queue(&[if self.error { 0xFC } else { 0xFE }]);
        if self.error {
            self.argument = Argument::None;
        }
        self.error = true;
    }

    fn valid_rate(rate: u8) -> bool {
        matches!(rate, 10 | 20 | 40 | 60 | 80 | 100 | 200)
    }
}

impl Default for Ps2Mouse {
    fn default() -> Self {
        Self::new()
    }
}

impl AuxDevice for Ps2Mouse {
    fn name(&self) -> &'static str {
        NAME
    }

    fn receive(&mut self, byte: u8) {
        if self.stream_output {
            self.output.clear();
            self.stream_output = false;
        }
        if self.wrap && !matches!(byte, 0xEC | 0xFF) {
            self.queue(&[byte]);
            return;
        }
        if byte == 0xFF {
            let buttons = self.buttons;
            *self = Self::new();
            self.buttons = buttons;
            self.reported_buttons = buttons;
            self.queue(&[ACK, 0xAA, 0]);
            return;
        }
        if byte == 0xFE {
            self.queue(&[self.last_sent.unwrap_or(0xFE)]);
            self.error = false;
            return;
        }
        if self.argument != Argument::None {
            let valid = match self.argument {
                Argument::SampleRate => Self::valid_rate(byte),
                Argument::Resolution => byte <= 3,
                Argument::None => false,
            };
            if !valid {
                self.invalid();
                return;
            }
            match self.argument {
                Argument::SampleRate => self.sample_rate = byte,
                Argument::Resolution => self.resolution = byte,
                Argument::None => {}
            }
            self.argument = Argument::None;
            self.error = false;
            self.clear_motion();
            self.queue(&[ACK]);
            return;
        }
        if !matches!(byte, 0xE6..=0xEA | 0xEB | 0xEC | 0xEE | 0xF0 | 0xF2..=0xF6) {
            self.invalid();
            return;
        }
        self.error = false;
        let mut reply = vec![ACK];
        match byte {
            0xF6 => self.defaults(),
            0xF5 => self.reporting = false,
            0xF4 => self.reporting = true,
            0xF3 => self.argument = Argument::SampleRate,
            0xF2 => reply.push(0),
            0xF0 => self.remote = true,
            0xEE => self.wrap = true,
            0xEC if self.wrap => {
                self.wrap = false;
                if !self.remote {
                    self.reporting = false;
                }
            }
            0xEB => {
                reply.extend(self.packet(false));
            }
            0xEA => self.remote = false,
            0xE9 => {
                let status = (u8::from(self.remote) << 6)
                    | (u8::from(self.reporting) << 5)
                    | (u8::from(self.scaling) << 4)
                    | ((self.buttons & 1) << 2)
                    | ((self.buttons & 4) >> 1)
                    | ((self.buttons & 2) >> 1);
                reply.extend([status, self.resolution, self.sample_rate]);
            }
            0xE8 => self.argument = Argument::Resolution,
            0xE7 => self.scaling = true,
            0xE6 => self.scaling = false,
            _ => {}
        }
        if self.queue(&reply) && byte == 0xEB {
            self.reported_buttons = self.buttons;
        }
        self.clear_motion();
    }

    fn has_output(&self) -> bool {
        !self.output.is_empty()
    }

    fn read_byte(&mut self) -> Option<u8> {
        let byte = self.output.pop_front()?;
        self.last_sent = Some(byte);
        if self.output.is_empty() {
            self.stream_output = false;
        }
        self.stream_packet();
        Some(byte)
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn save(&self) -> Vec<u8> {
        let flags = u8::from(self.reporting)
            | (u8::from(self.remote) << 1)
            | (u8::from(self.wrap) << 2)
            | (u8::from(self.scaling) << 3)
            | (u8::from(self.error) << 4)
            | (u8::from(self.last_sent.is_some()) << 5)
            | (u8::from(self.overflow_x) << 6)
            | (u8::from(self.overflow_y) << 7);
        let mut blob = vec![
            2,
            flags,
            self.sample_rate,
            self.resolution,
            match self.argument {
                Argument::None => 0,
                Argument::SampleRate => 1,
                Argument::Resolution => 2,
            },
            self.last_sent.unwrap_or(0),
            self.buttons,
            self.reported_buttons,
        ];
        blob.extend(self.dx.to_le_bytes());
        blob.extend(self.dy.to_le_bytes());
        blob.extend([self.output.len() as u8, u8::from(self.stream_output)]);
        blob.extend(&self.output);
        blob
    }

    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        let invalid = || DeviceError::InvalidSnapshot {
            device: NAME,
            reason: "invalid mouse state",
        };
        if blob.len() < 14
            || blob[0] != 2
            || !Self::valid_rate(blob[2])
            || blob[3] > 3
            || blob[4] > 2
            || blob[6] > 7
            || blob[7] > 7
            || usize::from(blob[12]) > CAPACITY
            || blob.len() != 14 + usize::from(blob[12])
            || blob[13] > 1
            || (blob[13] != 0 && (!(1..=3).contains(&blob[12]) || blob[1] & 7 != 1 || blob[4] != 0))
            || (blob[1] & 32 == 0 && blob[5] != 0)
        {
            return Err(invalid());
        }
        let dx = i16::from_le_bytes([blob[8], blob[9]]);
        let dy = i16::from_le_bytes([blob[10], blob[11]]);
        if !(-256..=255).contains(&dx)
            || !(-256..=255).contains(&dy)
            || (blob[1] & 64 != 0 && !matches!(dx, -256 | 255))
            || (blob[1] & 128 != 0 && !matches!(dy, -256 | 255))
        {
            return Err(invalid());
        }
        self.reporting = blob[1] & 1 != 0;
        self.remote = blob[1] & 2 != 0;
        self.wrap = blob[1] & 4 != 0;
        self.scaling = blob[1] & 8 != 0;
        self.error = blob[1] & 16 != 0;
        self.last_sent = (blob[1] & 32 != 0).then_some(blob[5]);
        self.overflow_x = blob[1] & 64 != 0;
        self.overflow_y = blob[1] & 128 != 0;
        self.sample_rate = blob[2];
        self.resolution = blob[3];
        self.argument = match blob[4] {
            1 => Argument::SampleRate,
            2 => Argument::Resolution,
            _ => Argument::None,
        };
        self.buttons = blob[6];
        self.reported_buttons = blob[7];
        self.dx = dx;
        self.dy = dy;
        self.stream_output = blob[13] != 0;
        self.output = blob[14..].iter().copied().collect();
        Ok(())
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::I8042;
    use device::{Device, NextEvent, TracePolicy};
    use testkit::MockBus;

    fn bus() -> MockBus {
        MockBus::new(Box::new(
            I8042::new().with_aux_device(Box::new(Ps2Mouse::new())),
        ))
    }

    fn controller(bus: &mut MockBus) -> &mut I8042 {
        bus.device_mut()
            .as_any_mut()
            .unwrap()
            .downcast_mut()
            .unwrap()
    }

    fn mouse(bus: &mut MockBus) -> &mut Ps2Mouse {
        controller(bus)
            .aux_device_mut()
            .unwrap()
            .as_any_mut()
            .downcast_mut()
            .unwrap()
    }

    fn send(bus: &mut MockBus, byte: u8) {
        bus.port_write(0x64, 1, 0xD4).unwrap();
        bus.port_write(0x60, 1, u32::from(byte)).unwrap();
    }

    fn drain(bus: &mut MockBus, bytes: &[u8]) {
        for &byte in bytes {
            bus.tick(1313);
            assert_eq!(bus.port_read(0x64, 1).unwrap() & 0x21, 0x21);
            assert_eq!(bus.port_read(0x60, 1).unwrap(), u32::from(byte));
        }
        bus.tick(1313);
        assert_eq!(bus.port_read(0x64, 1).unwrap() & 0x21, 0);
    }

    fn command(bus: &mut MockBus, byte: u8, bytes: &[u8]) {
        send(bus, byte);
        drain(bus, bytes);
    }

    #[test]
    fn command_replies_interrupt_pending_and_partly_transmitted_stream_packets() {
        for consumed in 0..=2 {
            for restored in [false, true] {
                for cmd in [0xF5, 0xF2, 0xEB] {
                    let mut bus = bus();
                    command(&mut bus, 0xF4, &[ACK]);
                    mouse(&mut bus).inject_motion(5, 7, 0);
                    for byte in [8, 5].into_iter().take(consumed) {
                        bus.tick(1313);
                        assert_eq!(bus.port_read(0x60, 1).unwrap(), byte);
                    }
                    mouse(&mut bus).inject_motion(11, -13, 0);
                    if restored {
                        let snapshot = bus.device().save();
                        bus.device_mut().reset();
                        bus.device_mut().restore(&snapshot).unwrap();
                    }
                    let reply: &[u8] = match cmd {
                        0xF2 => &[ACK, 0],
                        0xEB => &[ACK, 0x28, 11, 243],
                        _ => &[ACK],
                    };
                    command(&mut bus, cmd, reply);
                    command(&mut bus, 0xEB, &[ACK, 8, 0, 0]);
                }
            }
        }
    }

    #[test]
    fn reset_wrap_outside_wrap_preserves_enabled_stream_reporting() {
        let mut bus = bus();
        command(&mut bus, 0xF4, &[ACK]);
        command(&mut bus, 0xEC, &[ACK]);
        command(&mut bus, 0xE9, &[ACK, 0x20, 2, 100]);
        mouse(&mut bus).inject_motion(5, 7, 0);
        drain(&mut bus, &[8, 5, 7]);
    }

    #[test]
    fn resend_during_pending_arguments_preserves_argument_and_motion() {
        for (setting, argument) in [(0xF3, 80), (0xE8, 3)] {
            let mut bus = bus();
            command(&mut bus, 0xF0, &[ACK]);
            command(&mut bus, setting, &[ACK]);
            mouse(&mut bus).inject_motion(11, -13, 0);
            command(&mut bus, 0xFE, &[ACK]);
            assert_eq!((mouse(&mut bus).dx, mouse(&mut bus).dy), (11, -13));
            command(&mut bus, argument, &[ACK]);
            let (resolution, rate) = if setting == 0xF3 { (2, 80) } else { (3, 100) };
            command(&mut bus, 0xE9, &[ACK, 0x40, resolution, rate]);
            command(&mut bus, 0xEB, &[ACK, 8, 0, 0]);
        }
    }

    #[test]
    fn reset_defaults_identification_and_status() {
        let mut bus = bus();
        command(&mut bus, 0xFF, &[ACK, 0xAA, 0]);
        command(&mut bus, 0xF2, &[ACK, 0]);
        command(&mut bus, 0xE9, &[ACK, 0, 2, 100]);
        for cmd in [0xF4, 0xF0, 0xE7] {
            command(&mut bus, cmd, &[ACK]);
        }
        command(&mut bus, 0xF3, &[ACK]);
        command(&mut bus, 200, &[ACK]);
        command(&mut bus, 0xE8, &[ACK]);
        command(&mut bus, 3, &[ACK]);
        command(&mut bus, 0xE9, &[ACK, 0x70, 3, 200]);
        command(&mut bus, 0xF6, &[ACK]);
        command(&mut bus, 0xE9, &[ACK, 0, 2, 100]);
        mouse(&mut bus).inject_motion(5, 7, 0);
        command(&mut bus, 0xFF, &[ACK, 0xAA, 0]);
        command(&mut bus, 0xEB, &[ACK, 8, 0, 0]);
    }

    #[test]
    fn all_sample_rates_and_resolutions_acknowledge_and_round_trip() {
        let mut bus = bus();
        command(&mut bus, 0xFF, &[ACK, 0xAA, 0]);
        for rate in [10, 20, 40, 60, 80, 100, 200] {
            for resolution in 0..=3 {
                command(&mut bus, 0xF3, &[ACK]);
                command(&mut bus, rate, &[ACK]);
                command(&mut bus, 0xE8, &[ACK]);
                command(&mut bus, resolution, &[ACK]);
                command(&mut bus, 0xE9, &[ACK, 0, resolution, rate]);
            }
        }
    }

    #[test]
    fn invalid_commands_and_arguments_resend_then_error_and_recover() {
        let mut bus = bus();
        command(&mut bus, 0xFF, &[ACK, 0xAA, 0]);
        for (setting, invalid, valid) in [(0xF3, 0, 80), (0xE8, 4, 1)] {
            command(&mut bus, setting, &[ACK]);
            command(&mut bus, invalid, &[0xFE]);
            command(&mut bus, valid, &[ACK]);
            command(&mut bus, setting, &[ACK]);
            command(&mut bus, invalid, &[0xFE]);
            command(&mut bus, invalid, &[0xFC]);
            command(&mut bus, 0xF2, &[ACK, 0]);
        }
        for unknown in [0, 0xE5, 0xED, 0xEF, 0xF1, 0xF7, 0xFC] {
            command(&mut bus, unknown, &[0xFE]);
            command(&mut bus, unknown, &[0xFC]);
            command(&mut bus, unknown, &[0xFC]);
            command(&mut bus, 0xF2, &[ACK, 0]);
        }
    }

    #[test]
    fn resend_repeats_last_transmitted_byte_without_acknowledgement() {
        let mut bus = bus();
        command(&mut bus, 0xFF, &[ACK, 0xAA, 0]);
        command(&mut bus, 0xFE, &[0]);
        command(&mut bus, 0xE6, &[ACK]);
        command(&mut bus, 0xFE, &[ACK]);
        command(&mut bus, 0xF0, &[ACK]);
        mouse(&mut bus).inject_motion(3, -7, 0);
        command(&mut bus, 0xEB, &[ACK, 0x28, 3, 249]);
        command(&mut bus, 0xFE, &[249]);
        command(&mut bus, 0xFE, &[249]);
    }

    #[test]
    fn wrap_echoes_every_byte_except_reset_wrap_and_reset() {
        let mut bus = bus();
        command(&mut bus, 0xF0, &[ACK]);
        command(&mut bus, 0xEE, &[ACK]);
        for byte in 0..=255u8 {
            if !matches!(byte, 0xEC | 0xFF) {
                command(&mut bus, byte, &[byte]);
            }
        }
        command(&mut bus, 0xEC, &[ACK]);
        command(&mut bus, 0xE9, &[ACK, 0x40, 2, 100]);
        command(&mut bus, 0xEE, &[ACK]);
        command(&mut bus, 0xFF, &[ACK, 0xAA, 0]);
        command(&mut bus, 0xE9, &[ACK, 0, 2, 100]);
    }

    #[test]
    fn packets_encode_each_button_sign_and_counter_boundary() {
        let mut bus = bus();
        command(&mut bus, 0xF0, &[ACK]);
        for buttons in 0..8 {
            mouse(&mut bus).inject_motion(1, -1, buttons);
            command(&mut bus, 0xEB, &[ACK, 0x28 | buttons, 1, 255]);
            let status_buttons = ((buttons & 1) << 2) | ((buttons & 4) >> 1) | ((buttons & 2) >> 1);
            command(&mut bus, 0xE9, &[ACK, 0x40 | status_buttons, 2, 100]);
        }
        for (x, y, flags, low_x, low_y) in [
            (0, 0, 8, 0, 0),
            (255, -256, 0x28, 255, 0),
            (255, -255, 0x28, 255, 1),
            (-255, 255, 0x18, 1, 255),
            (256, -257, 0xE8, 255, 0),
            (-257, 256, 0xD8, 0, 255),
            (i32::MIN, i32::MAX, 0xD8, 0, 255),
        ] {
            mouse(&mut bus).inject_motion(x, y, 0);
            command(&mut bus, 0xEB, &[ACK, flags, low_x, low_y]);
            command(&mut bus, 0xEB, &[ACK, 8, 0, 0]);
        }
    }

    #[test]
    fn accumulated_motion_latches_overflow_until_packet_or_command() {
        let mut bus = bus();
        command(&mut bus, 0xF0, &[ACK]);
        for (x, y) in [(100, -200), (155, -55)] {
            mouse(&mut bus).inject_motion(x, y, 0);
        }
        command(&mut bus, 0xEB, &[ACK, 0x28, 255, 1]);
        for (x, y) in [(255, -256), (1, -1), (-200, 200)] {
            mouse(&mut bus).inject_motion(x, y, 0);
        }
        command(&mut bus, 0xEB, &[ACK, 0xE8, 255, 0]);
        mouse(&mut bus).inject_motion(10, 20, 0);
        command(&mut bus, 0xE6, &[ACK]);
        command(&mut bus, 0xEB, &[ACK, 8, 0, 0]);
    }

    #[test]
    fn scaling_table_applies_to_stream_but_read_data_is_unscaled() {
        let mut bus = bus();
        command(&mut bus, 0xF4, &[ACK]);
        command(&mut bus, 0xE7, &[ACK]);
        for (input, output) in [
            (0, 0),
            (1, 1),
            (2, 1),
            (3, 3),
            (4, 6),
            (5, 9),
            (6, 12),
            (127, 254),
        ] {
            if input == 0 {
                continue;
            }
            mouse(&mut bus).inject_motion(input, -input, 0);
            drain(&mut bus, &[0x28, output, (-(i16::from(output))) as u8]);
        }
        mouse(&mut bus).inject_motion(128, -128, 0);
        drain(&mut bus, &[0x68, 255, 0]);
        command(&mut bus, 0xF0, &[ACK]);
        mouse(&mut bus).inject_motion(4, -5, 0);
        command(&mut bus, 0xEB, &[ACK, 0x28, 4, 251]);
        command(&mut bus, 0xEA, &[ACK]);
        command(&mut bus, 0xE6, &[ACK]);
        mouse(&mut bus).inject_motion(4, -5, 0);
        drain(&mut bus, &[0x28, 4, 251]);
    }

    #[test]
    fn reporting_stream_remote_and_button_only_changes() {
        let mut bus = bus();
        command(&mut bus, 0xF5, &[ACK]);
        mouse(&mut bus).inject_motion(7, 8, 0);
        assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
        bus.tick(100_000);
        assert_eq!(bus.port_read(0x64, 1).unwrap() & 1, 0);
        command(&mut bus, 0xF4, &[ACK]);
        mouse(&mut bus).inject_motion(0, 0, 0);
        assert!(!mouse(&mut bus).has_output());
        mouse(&mut bus).inject_motion(0, 0, 1);
        drain(&mut bus, &[9, 0, 0]);
        mouse(&mut bus).inject_motion(0, 0, 1);
        assert!(!mouse(&mut bus).has_output());
        command(&mut bus, 0xF0, &[ACK]);
        mouse(&mut bus).inject_motion(-3, 4, 2);
        bus.tick(100_000);
        assert_eq!(bus.port_read(0x64, 1).unwrap() & 1, 0);
        command(&mut bus, 0xEB, &[ACK, 0x1A, 253, 4]);
        command(&mut bus, 0xEA, &[ACK]);
        mouse(&mut bus).inject_motion(0, 0, 0);
        drain(&mut bus, &[8, 0, 0]);
        command(&mut bus, 0xF5, &[ACK]);
        mouse(&mut bus).inject_motion(1, 2, 0);
        assert!(!mouse(&mut bus).has_output());
        command(&mut bus, 0xEB, &[ACK, 8, 1, 2]);
    }

    #[test]
    fn busy_stream_coalesces_injections_and_sample_rate_does_not_pace() {
        let mut bus = bus();
        command(&mut bus, 0xF3, &[ACK]);
        command(&mut bus, 10, &[ACK]);
        command(&mut bus, 0xF4, &[ACK]);
        mouse(&mut bus).inject_motion(1, 2, 0);
        mouse(&mut bus).inject_motion(3, 4, 0);
        mouse(&mut bus).inject_motion(5, 6, 1);
        drain(&mut bus, &[8, 1, 2, 9, 8, 10]);
    }

    #[test]
    fn snapshots_resume_arguments_wrap_errors_partial_packets_and_accumulators() {
        for phase in 0..7 {
            let mut source = bus();
            command(&mut source, 0xFF, &[ACK, 0xAA, 0]);
            command(&mut source, 0xF0, &[ACK]);
            match phase {
                0 => command(&mut source, 0xF3, &[ACK]),
                1 => command(&mut source, 0xE8, &[ACK]),
                2 => command(&mut source, 0xEE, &[ACK]),
                3 => {
                    command(&mut source, 0xE8, &[ACK]);
                    command(&mut source, 9, &[0xFE]);
                }
                4 => {
                    mouse(&mut source).inject_motion(900, -900, 5);
                }
                5 => {
                    mouse(&mut source).inject_motion(3, -9, 2);
                    send(&mut source, 0xEB);
                    source.tick(1313);
                    assert_eq!(source.port_read(0x60, 1).unwrap(), u32::from(ACK));
                    source.tick(30);
                }
                _ => {
                    source.port_write(0x64, 1, 0xD4).unwrap();
                }
            }
            let saved = source.device().save();
            let mut restored = bus();
            restored.device_mut().restore(&saved).unwrap();
            restored.tick(source.clock().0);
            for bus in [&mut source, &mut restored] {
                match phase {
                    0 => command(bus, 80, &[ACK]),
                    1 => command(bus, 3, &[ACK]),
                    2 => {
                        command(bus, 0xF2, &[0xF2]);
                        command(bus, 0xEC, &[ACK]);
                    }
                    3 => command(bus, 9, &[0xFC]),
                    4 => command(bus, 0xEB, &[ACK, 0xED, 255, 0]),
                    5 => drain(bus, &[0x2A, 3, 247]),
                    _ => {
                        bus.port_write(0x60, 1, 0xF2).unwrap();
                        drain(bus, &[ACK, 0]);
                    }
                }
                command(
                    bus,
                    0xFE,
                    &[match phase {
                        5 => 247,
                        4 | 6 => 0,
                        3 => 0xFC,
                        _ => ACK,
                    }],
                );
            }
            assert_eq!(source.device().save(), restored.device().save());
        }
    }

    #[test]
    fn snapshots_reject_truncation_bad_mouse_state_and_wrong_attachment_atomically() {
        let mut bus = bus();
        command(&mut bus, 0xF0, &[ACK]);
        mouse(&mut bus).inject_motion(-33, 75, 7);
        let saved = bus.device().save();
        for length in 0..saved.len() {
            assert!(bus.device_mut().restore(&saved[..length]).is_err());
            assert_eq!(bus.device().save(), saved);
        }
        let mut absent = I8042::new();
        let absent_before = absent.save();
        assert!(absent.restore(&saved).is_err());
        assert_eq!(absent.save(), absent_before);
        assert!(bus.device_mut().restore(&absent_before).is_err());
        assert_eq!(bus.device().save(), saved);
        let mouse_blob = mouse(&mut bus).save();
        for (index, value) in [
            (0, 1),
            (2, 11),
            (3, 4),
            (4, 3),
            (6, 8),
            (7, 8),
            (9, 127),
            (12, 17),
            (13, 1),
            (13, 2),
        ] {
            let mut invalid = saved.clone();
            let offset = invalid.len() - mouse_blob.len();
            invalid[offset + index] = value;
            assert!(bus.device_mut().restore(&invalid).is_err());
            assert_eq!(bus.device().save(), saved);
        }
        bus.device_mut()
            .set_trace_policy(TracePolicy::Ring { capacity: 8 });
        bus.port_read(0x64, 1).unwrap();
        bus.device_mut().restore(&saved).unwrap();
        assert_eq!(bus.device_mut().take_trace().len(), 1);
    }

    #[test]
    fn wrap_returns_stream_disabled_and_snapshot_keeps_all_settings() {
        let mut source = bus();
        command(&mut source, 0xF4, &[ACK]);
        command(&mut source, 0xE7, &[ACK]);
        command(&mut source, 0xF3, &[ACK]);
        command(&mut source, 80, &[ACK]);
        command(&mut source, 0xE8, &[ACK]);
        command(&mut source, 3, &[ACK]);
        command(&mut source, 0xEE, &[ACK]);
        let saved = source.device().save();
        let mut restored = bus();
        restored.device_mut().restore(&saved).unwrap();
        restored.tick(source.clock().0);
        for bus in [&mut source, &mut restored] {
            mouse(bus).inject_motion(20, -30, 1);
            assert!(!mouse(bus).has_output());
            command(bus, 0xEC, &[ACK]);
            command(bus, 0xE9, &[ACK, 0x14, 3, 80]);
            command(bus, 0xEC, &[ACK]);
            mouse(bus).inject_motion(3, 4, 1);
            assert!(!mouse(bus).has_output());
            command(bus, 0xEB, &[ACK, 9, 3, 4]);
            command(bus, 0xF4, &[ACK]);
            command(bus, 0xEC, &[ACK]);
            command(bus, 0xE9, &[ACK, 0x34, 3, 80]);
        }
        assert_eq!(source.device().save(), restored.device().save());
    }

    #[test]
    fn reply_flood_is_bounded_and_reset_keeps_the_attachment() {
        let mut bus = bus();
        command(&mut bus, 0xF0, &[ACK]);
        bus.port_write(0x64, 1, 0xA7).unwrap();
        for _ in 0..1000 {
            send(&mut bus, 0xE9);
        }
        assert_eq!(mouse(&mut bus).output.len(), 16);
        let snapshot = bus.device().save();
        bus.device_mut().restore(&snapshot).unwrap();
        bus.port_write(0x64, 1, 0xA8).unwrap();
        drain(
            &mut bus,
            &[
                ACK, 0x40, 2, 100, ACK, 0x40, 2, 100, ACK, 0x40, 2, 100, ACK, 0x40, 2, 100,
            ],
        );
        bus.device_mut().reset();
        command(&mut bus, 0xFF, &[ACK, 0xAA, 0]);
    }
}
