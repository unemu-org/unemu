// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM PC/XT system-board 8255 wiring and 83-key serial keyboard receiver.
//!
//! IBM Personal Computer XT Technical Reference (6280089, March 1986),
//! §1 "System Board", "8255A I/O Bit Map" (pp. 1-27–1-28), §2
//! "Coprocessor", §4 "83-Key Keyboard" and "Block Diagram" (p. 4-5);
//! Intel 8255A datasheet, Mode 0 and Bit Set/Reset.
//! See `backlog/docs/design/firmware-and-machine-classes.md` §4.1 and §9 item 14.
//!
//! ## Coverage matrix
//! | Feature | Coverage / evidence |
//! |---|---|
//! | PA: keyboard receive shift register or diagnostic outputs | READ: §1 8255A I/O Bit Map; §4 Block Diagram connects PA only to LS322, never SW1 |
//! | PB7=1 clears receiver; PA then reads 00h | clear function READ: §1 bit map; exact cleared read value ASSUMED |
//! | PB0 gate, PB1 speaker, PB2 spare, PB3 SW1 half select | READ: §1 bit map; PB3=0 switches 1–4, =1 switches 5–8 |
//! | PB4 RAM parity disable, PB5 I/O check disable, PB6 low holds clock low | READ: §1 bit map; checking enabled when PB4/PB5=0 |
//! | PC0–3 SW1 half, PC4 spare, PC5 OUT2, PC6 I/O check, PC7 RAM parity | READ: §1 bit map; spare PC4 reading zero ASSUMED |
//! | SW1 bit 0 normal POST=1 / loop=0, bit 1 installed 8087=1 | READ: §1 bit map, §2 Coprocessor and §5 BIOS listing, POST loop |
//! | SW1 banks, display, drive count plus one; no SW2 | READ: §1 bit map and one 8-switch pack description; display 00 reserved, exposed for option-ROM rigs by ASSUMPTION |
//! | 83 keys, make 01h / break 81h, 16-code buffer | READ: §4 83-Key Keyboard (p. 4-4); positional codes match existing frontend table |
//! | Overflow code FFh | READ: §5 BIOS listing, INT 9 overrun check and K62 beep (11/08/82 and 01/10/86); replacing the last queued byte on overflow, coalescing repeats, is ASSUMED |
//! | Reset sequence 08h, ~20 ms low, C8h, 48h, IRQ1/AAh | READ: §5 BIOS listing, KBD_RESET (01/10/86) |
//! | Minimum reset low interval | ASSUMED: 1,193 PIT clocks (~1 ms) rejects immediate mode-set/release; §4 83-Key Keyboard gives no minimum; the 12.5 ms at p. 4-26 is for 101/102 keys |
//! | CPU-rate coupling | ASSUMED compatibility threshold: CPU steps are not cycles; §5 BIOS KBD_RESET holds for 10,582 LOOP iterations, so rate 1/1 exceeds 1,193 clocks; board qualification must ensure this at every supported CPU rate |
//! | PPI partial decode | READ: §1 I/O Address Map (1-24), 060h–06Fh; A0/A1 register aliases implemented in the XT wrapper |
//! | Keyboard reset and receive latch | READ: §4 Block Diagram (4-5); PB7 clears the LS322; keyboard reset retains an unread board byte and queues AAh behind it |
//! | Serial shifting / BAT delay / typematic | byte-level only; no bit timing, physical BAT latency, spontaneous power-on BAT or host commands |
//! | Queue pacing | first byte immediate; next byte one virtual PIT clock after acknowledge release (ASSUMED abstraction, §4 Block Diagram) |
//! | Undriven PB pins | ASSUMED: treated as low, §1 8255A I/O Bit Map; only driven PB6 starts a reset interval |
//! | PIT / parity / audio / NMI coupling | pin setters and control getters only; no board wiring or NMI source |
//! | Snapshot | PPI, pins, receive latch, bounded queue, reset interval, deadline, clock; traces excluded |

use crate::Ppi8255;
use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};
use std::collections::VecDeque;

/// Display encoding in SW1 bits 4–5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XtDisplay {
    /// Video adapter with its own firmware (00b).
    OptionRom,
    /// CGA 40-column startup (01b).
    Cga40,
    /// CGA 80-column startup (10b).
    Cga80,
    /// Monochrome adapter (11b).
    Monochrome,
}

/// Configuration used to derive electrical SW1 input levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XtSwitchConfig {
    /// Number of diskette drives, one through four.
    pub floppy_drives: u8,
    /// Populated motherboard memory banks, one through four (not total RAM).
    pub memory_banks: u8,
    /// Startup display selection.
    pub display: XtDisplay,
    /// Whether an 8087 is installed.
    pub fpu_present: bool,
    /// Loop POST instead of normal bring-up (SW1 bit 0 low).
    pub loop_on_post: bool,
}

/// Out-of-range motherboard switch configuration.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum XtSwitchError {
    /// SW1 has only two bits for the drive count minus one.
    #[error("XT floppy drive count must be between one and four (SW1 cannot encode zero drives)")]
    FloppyDrives,
    /// SW1 encodes one to four populated motherboard banks.
    #[error("XT memory bank count must be between one and four")]
    MemoryBanks,
}

impl XtSwitchConfig {
    /// Derive raw SW1 levels: bit 0 normal POST, bit 1 installed 8087,
    /// bits 2–3 banks minus one, bits 4–5 display, bits 6–7 drives minus one.
    /// This describes motherboard options; expansion RAM is absent from SW1.
    /// IBM Personal Computer XT Technical Reference (March 1986), §1
    /// "8255A I/O Bit Map", §2 "Coprocessor", §5 BIOS listing (POST loop).
    pub fn sw1(self) -> Result<u8, XtSwitchError> {
        if !(1..=4).contains(&self.floppy_drives) {
            return Err(XtSwitchError::FloppyDrives);
        }
        if !(1..=4).contains(&self.memory_banks) {
            return Err(XtSwitchError::MemoryBanks);
        }
        let display = match self.display {
            XtDisplay::OptionRom => 0,
            XtDisplay::Cga40 => 1,
            XtDisplay::Cga80 => 2,
            XtDisplay::Monochrome => 3,
        };
        Ok(u8::from(!self.loop_on_post)
            | (u8::from(self.fpu_present) << 1)
            | ((self.memory_banks - 1) << 2)
            | (display << 4)
            | ((self.floppy_drives - 1) << 6))
    }
}

impl Default for XtSwitchConfig {
    fn default() -> Self {
        Self {
            floppy_drives: 2,
            memory_banks: 4,
            display: XtDisplay::Monochrome,
            fpu_present: false,
            loop_on_post: false,
        }
    }
}

/// The 83 original XT physical US key positions; no E0/E1 or enhanced keys.
/// Values match the shared non-extended subset of the frontend's set-1 table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum XtKey {
    /// Physical US keyboard position Escape.
    Escape = 0x01,
    /// Physical US keyboard position Digit1.
    Digit1 = 0x02,
    /// Physical US keyboard position Digit2.
    Digit2 = 0x03,
    /// Physical US keyboard position Digit3.
    Digit3 = 0x04,
    /// Physical US keyboard position Digit4.
    Digit4 = 0x05,
    /// Physical US keyboard position Digit5.
    Digit5 = 0x06,
    /// Physical US keyboard position Digit6.
    Digit6 = 0x07,
    /// Physical US keyboard position Digit7.
    Digit7 = 0x08,
    /// Physical US keyboard position Digit8.
    Digit8 = 0x09,
    /// Physical US keyboard position Digit9.
    Digit9 = 0x0A,
    /// Physical US keyboard position Digit0.
    Digit0 = 0x0B,
    /// Physical US keyboard position Minus.
    Minus = 0x0C,
    /// Physical US keyboard position Equal.
    Equal = 0x0D,
    /// Physical US keyboard position Backspace.
    Backspace = 0x0E,
    /// Physical US keyboard position Tab.
    Tab = 0x0F,
    /// Physical US keyboard position KeyQ.
    KeyQ = 0x10,
    /// Physical US keyboard position KeyW.
    KeyW = 0x11,
    /// Physical US keyboard position KeyE.
    KeyE = 0x12,
    /// Physical US keyboard position KeyR.
    KeyR = 0x13,
    /// Physical US keyboard position KeyT.
    KeyT = 0x14,
    /// Physical US keyboard position KeyY.
    KeyY = 0x15,
    /// Physical US keyboard position KeyU.
    KeyU = 0x16,
    /// Physical US keyboard position KeyI.
    KeyI = 0x17,
    /// Physical US keyboard position KeyO.
    KeyO = 0x18,
    /// Physical US keyboard position KeyP.
    KeyP = 0x19,
    /// Physical US keyboard position BracketLeft.
    BracketLeft = 0x1A,
    /// Physical US keyboard position BracketRight.
    BracketRight = 0x1B,
    /// Physical US keyboard position Enter.
    Enter = 0x1C,
    /// Physical US keyboard position ControlLeft.
    ControlLeft = 0x1D,
    /// Physical US keyboard position KeyA.
    KeyA = 0x1E,
    /// Physical US keyboard position KeyS.
    KeyS = 0x1F,
    /// Physical US keyboard position KeyD.
    KeyD = 0x20,
    /// Physical US keyboard position KeyF.
    KeyF = 0x21,
    /// Physical US keyboard position KeyG.
    KeyG = 0x22,
    /// Physical US keyboard position KeyH.
    KeyH = 0x23,
    /// Physical US keyboard position KeyJ.
    KeyJ = 0x24,
    /// Physical US keyboard position KeyK.
    KeyK = 0x25,
    /// Physical US keyboard position KeyL.
    KeyL = 0x26,
    /// Physical US keyboard position Semicolon.
    Semicolon = 0x27,
    /// Physical US keyboard position Quote.
    Quote = 0x28,
    /// Physical US keyboard position Backquote.
    Backquote = 0x29,
    /// Physical US keyboard position ShiftLeft.
    ShiftLeft = 0x2A,
    /// Physical US keyboard position Backslash.
    Backslash = 0x2B,
    /// Physical US keyboard position KeyZ.
    KeyZ = 0x2C,
    /// Physical US keyboard position KeyX.
    KeyX = 0x2D,
    /// Physical US keyboard position KeyC.
    KeyC = 0x2E,
    /// Physical US keyboard position KeyV.
    KeyV = 0x2F,
    /// Physical US keyboard position KeyB.
    KeyB = 0x30,
    /// Physical US keyboard position KeyN.
    KeyN = 0x31,
    /// Physical US keyboard position KeyM.
    KeyM = 0x32,
    /// Physical US keyboard position Comma.
    Comma = 0x33,
    /// Physical US keyboard position Period.
    Period = 0x34,
    /// Physical US keyboard position Slash.
    Slash = 0x35,
    /// Physical US keyboard position ShiftRight.
    ShiftRight = 0x36,
    /// Physical US keyboard position NumpadMultiply.
    NumpadMultiply = 0x37,
    /// Physical US keyboard position AltLeft.
    AltLeft = 0x38,
    /// Physical US keyboard position Space.
    Space = 0x39,
    /// Physical US keyboard position CapsLock.
    CapsLock = 0x3A,
    /// Physical US keyboard position F1.
    F1 = 0x3B,
    /// Physical US keyboard position F2.
    F2 = 0x3C,
    /// Physical US keyboard position F3.
    F3 = 0x3D,
    /// Physical US keyboard position F4.
    F4 = 0x3E,
    /// Physical US keyboard position F5.
    F5 = 0x3F,
    /// Physical US keyboard position F6.
    F6 = 0x40,
    /// Physical US keyboard position F7.
    F7 = 0x41,
    /// Physical US keyboard position F8.
    F8 = 0x42,
    /// Physical US keyboard position F9.
    F9 = 0x43,
    /// Physical US keyboard position F10.
    F10 = 0x44,
    /// Physical US keyboard position NumLock.
    NumLock = 0x45,
    /// Physical US keyboard position ScrollLock.
    ScrollLock = 0x46,
    /// Physical US keyboard position Numpad7.
    Numpad7 = 0x47,
    /// Physical US keyboard position Numpad8.
    Numpad8 = 0x48,
    /// Physical US keyboard position Numpad9.
    Numpad9 = 0x49,
    /// Physical US keyboard position NumpadSubtract.
    NumpadSubtract = 0x4A,
    /// Physical US keyboard position Numpad4.
    Numpad4 = 0x4B,
    /// Physical US keyboard position Numpad5.
    Numpad5 = 0x4C,
    /// Physical US keyboard position Numpad6.
    Numpad6 = 0x4D,
    /// Physical US keyboard position NumpadAdd.
    NumpadAdd = 0x4E,
    /// Physical US keyboard position Numpad1.
    Numpad1 = 0x4F,
    /// Physical US keyboard position Numpad2.
    Numpad2 = 0x50,
    /// Physical US keyboard position Numpad3.
    Numpad3 = 0x51,
    /// Physical US keyboard position Numpad0.
    Numpad0 = 0x52,
    /// Physical US keyboard position NumpadDecimal.
    NumpadDecimal = 0x53,
}

/// Compatibility hold interval: reject adjacent mode-set/release writes while
/// allowing the BIOS loop under instruction-based virtual timing, not CPU cycles.
const RESET_LOW_CLOCKS: u64 = 1_193;
/// The keyboard buffer excludes the system board's receive shift register.
const QUEUE_CAPACITY: usize = 16;

/// XT motherboard wiring over an independent Mode 0 PPI.
pub struct XtSystemPpi {
    ppi: Ppi8255,
    sw1: u8,
    timer2: bool,
    io_check: bool,
    parity: bool,
    latch: Option<u8>,
    queue: VecDeque<u8>,
    clock_low_since: Option<VClock>,
    ready_at: Option<VClock>,
    now: VClock,
    trace: device::TraceBuffer,
}

impl XtSystemPpi {
    /// Validate switch configuration and construct the unwired motherboard device.
    pub fn new(config: XtSwitchConfig) -> Result<Self, XtSwitchError> {
        Ok(Self {
            ppi: Ppi8255::new(0x60),
            sw1: config.sw1()?,
            timer2: false,
            io_check: false,
            parity: false,
            latch: None,
            queue: VecDeque::new(),
            clock_low_since: None,
            ready_at: None,
            now: VClock::ZERO,
            trace: device::TraceBuffer::default(),
        })
    }
    /// Host key event; repeated presses deterministically repeat the make byte.
    pub fn key_event(&mut self, key: XtKey, pressed: bool) {
        if self.queue.len() < QUEUE_CAPACITY {
            self.queue
                .push_back(key as u8 | if pressed { 0 } else { 0x80 });
        } else {
            self.queue.pop_back();
            self.queue.push_back(0xFF);
        }
        self.refill();
    }
    /// External timer OUT2 and latched I/O-check / RAM-parity status pins.
    /// Checks are not generated or routed to NMI by this device.
    pub fn set_status_inputs(&mut self, timer2: bool, io_check: bool, parity: bool) {
        self.timer2 = timer2;
        self.io_check = io_check;
        self.parity = parity;
    }
    fn register(port: u16) -> u16 {
        if (0x60..=0x6F).contains(&port) {
            0x60 | (port & 3)
        } else {
            port
        }
    }
    fn pb(&self) -> u8 {
        self.ppi.outputs()[1]
    }
    /// Timer channel 2 GATE output (PB0).
    pub fn timer2_gate(&self) -> bool {
        self.pb() & 1 != 0
    }
    /// Speaker data output (PB1), with no audio synthesis.
    pub fn speaker_data(&self) -> bool {
        self.pb() & 2 != 0
    }
    /// RAM parity checking is enabled while PB4 is low.
    pub fn parity_check_enabled(&self) -> bool {
        self.pb() & 0x10 == 0
    }
    /// I/O channel checking is enabled while PB5 is low.
    pub fn io_check_enabled(&self) -> bool {
        self.pb() & 0x20 == 0
    }
    /// Keyboard clock is held low while PB6 is low.
    pub fn keyboard_clock_low(&self) -> bool {
        self.pb() & 0x40 == 0
    }
    fn refill(&mut self) {
        if self.pb() & 0xC0 != 0x40 || self.latch.is_some() || self.ready_at.is_some() {
            return;
        }
        self.latch = self.queue.pop_front();
    }
    fn input_pins(&self) -> [u8; 3] {
        let pb = self.pb();
        let switches = if pb & 8 != 0 {
            self.sw1 >> 4
        } else {
            self.sw1 & 0x0F
        };
        let pc = switches
            | (u8::from(self.timer2) << 5)
            | (u8::from(self.io_check) << 6)
            | (u8::from(self.parity) << 7);
        [
            if pb & 0x80 != 0 {
                0
            } else {
                self.latch.unwrap_or(0)
            },
            0,
            pc,
        ]
    }
    fn drive_inputs(&mut self) {
        self.ppi.set_inputs(self.input_pins());
    }
}
impl Device for XtSystemPpi {
    fn name(&self) -> &'static str {
        "xt-system-ppi"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.drive_inputs();
        let value = self.ppi.port_read(Self::register(port), size)?;
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value,
        });
        Ok(value)
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let old = self.pb();
        let register = Self::register(port);
        self.ppi.port_write(register, size, value)?;
        let pb = self.pb();
        if register == 0x61 || (register == 0x63 && value & 0x80 != 0 && value & 0x64 == 0) {
            if self.ppi.output_masks()[1] & 0x40 == 0 {
                self.clock_low_since = None;
            } else if pb & 0x40 == 0 {
                self.clock_low_since.get_or_insert(self.now);
            } else if old & 0x40 == 0
                && self
                    .clock_low_since
                    .take()
                    .is_some_and(|since| self.now.0.saturating_sub(since.0) >= RESET_LOW_CLOCKS)
            {
                self.queue.clear();
                self.queue.push_back(0xAA);
            }
            if pb & 0x80 != 0 {
                self.latch = None;
                self.ready_at = None;
            } else if old & 0x80 != 0 {
                self.ready_at = Some(self.now.advance(1));
            }
            self.refill();
        }
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Write,
            value,
        });
        Ok(())
    }
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        self.ppi.mmio_read(addr, size)
    }
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.ppi.mmio_write(addr, size, value)
    }
    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now < self.now {
            return;
        }
        self.now = now;
        self.ppi.tick(now);
        if self.ready_at.is_some_and(|at| now >= at) {
            self.ready_at = None;
        }
        self.refill();
    }
    fn next_event(&self, _now: VClock) -> NextEvent {
        self.ready_at.map_or(NextEvent::Dormant, NextEvent::At)
    }
    fn irq_pending(&self) -> bool {
        self.latch.is_some() && self.pb() & 0x80 == 0
    }
    fn reset(&mut self) {
        self.ppi.reset();
        self.timer2 = false;
        self.io_check = false;
        self.parity = false;
        self.latch = None;
        self.queue.clear();
        self.clock_low_since = None;
        self.ready_at = None;
        self.now = VClock::ZERO;
        self.trace.clear();
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = vec![
            2,
            self.sw1,
            self.timer2.into(),
            self.io_check.into(),
            self.parity.into(),
            self.latch.is_some().into(),
            self.latch.unwrap_or(0),
            self.clock_low_since.is_some().into(),
            self.ready_at.is_some().into(),
        ];
        blob.extend(self.ready_at.unwrap_or(VClock::ZERO).0.to_le_bytes());
        blob.extend(self.now.0.to_le_bytes());
        let mut ppi = self.ppi.save();
        ppi[5..8].copy_from_slice(&self.input_pins());
        blob.extend(ppi);
        blob.extend(self.clock_low_since.unwrap_or(VClock::ZERO).0.to_le_bytes());
        blob.extend(self.queue.iter());
        blob
    }
    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        let invalid = || DeviceError::InvalidSnapshot {
            device: "xt-system-ppi",
            reason: "invalid XT PPI snapshot",
        };
        if !(49..=49 + QUEUE_CAPACITY).contains(&blob.len())
            || blob[0] != 2
            || blob[1] != self.sw1
            || [2, 3, 4, 5, 7, 8].iter().any(|&i| blob[i] > 1)
        {
            return Err(invalid());
        }
        let deadline = VClock(u64::from_le_bytes(std::array::from_fn(|i| blob[9 + i])));
        let now = VClock(u64::from_le_bytes(std::array::from_fn(|i| blob[17 + i])));
        let mut ppi = Ppi8255::new(0x60);
        ppi.restore(&blob[25..41])?;
        let pb = ppi.outputs()[1];
        let low_since = VClock(u64::from_le_bytes(std::array::from_fn(|i| blob[41 + i])));
        let valid_byte = |b: u8| b == 0xAA || b == 0xFF || (1..=0x53).contains(&(b & 0x7F));
        if blob[33..41] != blob[17..25]
            || (blob[5] == 0 && blob[6] != 0)
            || (blob[5] != 0 && (!valid_byte(blob[6]) || pb & 0x80 != 0))
            || (blob[8] == 0 && deadline != VClock::ZERO)
            || (blob[8] != 0 && (deadline <= now || blob[5] != 0 || pb & 0x80 != 0))
            || (blob[7] == 0 && low_since != VClock::ZERO)
            || (blob[7] != 0
                && (low_since > now || pb & 0x40 != 0 || ppi.output_masks()[1] & 0x40 == 0))
            || blob[49..].iter().any(|&b| !valid_byte(b))
        {
            return Err(invalid());
        }
        self.ppi = ppi;
        self.timer2 = blob[2] != 0;
        self.io_check = blob[3] != 0;
        self.parity = blob[4] != 0;
        self.latch = (blob[5] != 0).then_some(blob[6]);
        self.clock_low_since = (blob[7] != 0).then_some(low_since);
        self.ready_at = (blob[8] != 0).then_some(deadline);
        self.now = now;
        self.queue = blob[49..].iter().copied().collect();
        Ok(())
    }
    fn set_trace_policy(&mut self, policy: device::TracePolicy) {
        self.trace.set_policy(policy);
    }
    fn trace_dropped(&self) -> u64 {
        self.trace.dropped()
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.trace.take()
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    fn xt(bus: &mut MockBus) -> &mut XtSystemPpi {
        bus.device_mut()
            .as_any_mut()
            .unwrap()
            .downcast_mut()
            .unwrap()
    }
    fn bus() -> MockBus {
        MockBus::new(Box::new(
            XtSystemPpi::new(XtSwitchConfig::default()).unwrap(),
        ))
    }
    fn acknowledge(bus: &mut MockBus) {
        let pb = bus.port_read(0x61, 1).unwrap();
        bus.port_write(0x61, 1, pb | 0x80).unwrap();
        assert!(!bus.device().irq_pending());
        bus.port_write(0x61, 1, pb & !0x80).unwrap();
        assert!(!bus.device().irq_pending());
    }
    fn initialized() -> MockBus {
        let mut bus = bus();
        bus.port_write(0x63, 1, 0x99).unwrap();
        bus.port_write(0x61, 1, 0x08).unwrap();
        bus.tick(23_864);
        bus.port_write(0x61, 1, 0xC8).unwrap();
        bus.port_write(0x61, 1, 0x48).unwrap();
        bus.tick(1);
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xAA);
        acknowledge(&mut bus);
        bus.port_write(0x61, 1, 0x40).unwrap();
        bus.tick(1);
        bus
    }

    #[test]
    fn switch_derivation_and_both_read_paths_have_literal_values() {
        for (config, expected) in [
            (XtSwitchConfig::default(), 0x7D),
            (
                XtSwitchConfig {
                    floppy_drives: 1,
                    memory_banks: 1,
                    display: XtDisplay::OptionRom,
                    fpu_present: true,
                    loop_on_post: false,
                },
                0x03,
            ),
            (
                XtSwitchConfig {
                    floppy_drives: 1,
                    memory_banks: 2,
                    display: XtDisplay::Cga40,
                    fpu_present: true,
                    loop_on_post: false,
                },
                0x17,
            ),
            (
                XtSwitchConfig {
                    floppy_drives: 3,
                    memory_banks: 3,
                    display: XtDisplay::Cga80,
                    fpu_present: false,
                    loop_on_post: false,
                },
                0xA9,
            ),
            (
                XtSwitchConfig {
                    floppy_drives: 4,
                    memory_banks: 4,
                    display: XtDisplay::Monochrome,
                    fpu_present: false,
                    loop_on_post: false,
                },
                0xFD,
            ),
        ] {
            assert_eq!(config.sw1().unwrap(), expected);
            let mut bus = MockBus::new(Box::new(XtSystemPpi::new(config).unwrap()));
            bus.port_write(0x63, 1, 0x99).unwrap();
            bus.port_write(0x61, 1, 0x80).unwrap();
            assert_eq!(bus.port_read(0x60, 1).unwrap(), 0);
            assert_eq!(bus.port_read(0x62, 1).unwrap(), u32::from(expected & 15));
            bus.port_write(0x61, 1, 0x88).unwrap();
            assert_eq!(bus.port_read(0x62, 1).unwrap(), u32::from(expected >> 4));
            bus.port_write(0x61, 1, 0x8C).unwrap();
            assert_eq!(bus.port_read(0x62, 1).unwrap(), u32::from(expected >> 4));
            assert_eq!(bus.port_read(0x60, 1).unwrap(), 0);
            assert!(!bus.device().irq_pending());
        }
        for (drives, banks, error) in [
            (0, 4, XtSwitchError::FloppyDrives),
            (5, 4, XtSwitchError::FloppyDrives),
            (1, 0, XtSwitchError::MemoryBanks),
            (1, 5, XtSwitchError::MemoryBanks),
        ] {
            let actual = XtSwitchConfig {
                floppy_drives: drives,
                memory_banks: banks,
                ..XtSwitchConfig::default()
            }
            .sw1();
            assert_eq!(actual, Err(error));
        }
        assert_eq!(
            XtSwitchError::FloppyDrives.to_string(),
            "XT floppy drive count must be between one and four (SW1 cannot encode zero drives)"
        );
    }

    #[test]
    fn post_loop_and_coprocessor_switches_are_independent_of_drive_count() {
        for (loop_on_post, fpu_present, low) in [
            (false, false, 0x0D),
            (false, true, 0x0F),
            (true, false, 0x0C),
            (true, true, 0x0E),
        ] {
            for floppy_drives in 1..=4 {
                let mut bus = MockBus::new(Box::new(
                    XtSystemPpi::new(XtSwitchConfig {
                        loop_on_post,
                        fpu_present,
                        floppy_drives,
                        ..XtSwitchConfig::default()
                    })
                    .unwrap(),
                ));
                bus.port_write(0x63, 1, 0x99).unwrap();
                assert_eq!(bus.port_read(0x62, 1).unwrap(), low);
                bus.port_write(0x61, 1, 8).unwrap();
                assert_eq!(
                    bus.port_read(0x62, 1).unwrap(),
                    ((floppy_drives - 1) << 2 | 3).into()
                );
            }
        }
    }

    #[test]
    fn pb_controls_and_pc_pins_are_independent_of_at_port_61() {
        let mut bus = initialized();
        bus.port_write(0x61, 1, 0x73).unwrap();
        assert_eq!(bus.port_read(0x61, 1).unwrap(), 0x73);
        assert!(xt(&mut bus).timer2_gate());
        assert!(xt(&mut bus).speaker_data());
        assert!(!xt(&mut bus).parity_check_enabled());
        assert!(!xt(&mut bus).io_check_enabled());
        assert!(!xt(&mut bus).keyboard_clock_low());
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0x0D);
        xt(&mut bus).set_status_inputs(true, true, true);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0xED);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0xED);
        bus.port_write(0x61, 1, 0x48).unwrap();
        assert!(!xt(&mut bus).timer2_gate());
        assert!(!xt(&mut bus).speaker_data());
        assert!(xt(&mut bus).parity_check_enabled());
        assert!(xt(&mut bus).io_check_enabled());
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0xE7);
        bus.port_write(0x63, 1, 0x80).unwrap();
        bus.port_write(0x60, 1, 0x34).unwrap();
        bus.port_write(0x62, 1, 0x56).unwrap();
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x34);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0x56);
        assert!(bus.port_read(0x70, 1).is_err());
        assert!(bus.mmio_write(0, 1, 0).is_err());
    }

    #[test]
    fn reset_clock_handshake_clear_and_scan_latch() {
        let mut bus = bus();
        bus.port_write(0x63, 1, 0x99).unwrap();
        bus.port_write(0x61, 1, 0x08).unwrap();
        bus.tick(23_864);
        assert!(!bus.device().irq_pending());
        bus.port_write(0x61, 1, 0xC8).unwrap();
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 7);
        bus.port_write(0x61, 1, 0x48).unwrap();
        assert!(!bus.device().irq_pending());
        bus.tick(1);
        assert!(bus.device().irq_pending());
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xAA);
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xAA);
        assert!(bus.device().irq_pending());
        bus.port_write(0x61, 1, 0xC8).unwrap();
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0);
        bus.port_write(0x61, 1, 0x48).unwrap();
        bus.tick(1);
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0);
        assert!(!bus.device().irq_pending());
    }

    #[test]
    fn reset_requires_a_complete_clock_low_interval() {
        assert_eq!(RESET_LOW_CLOCKS, 1_193);
        for (clocks, reset) in [(0, false), (1_192, false), (1_193, true)] {
            let mut bus = bus();
            bus.port_write(0x63, 1, 0x99).unwrap();
            bus.tick(clocks);
            bus.port_write(0x61, 1, 0x40).unwrap();
            assert_eq!(bus.device().irq_pending(), reset);
            assert_eq!(
                bus.port_read(0x60, 1).unwrap(),
                if reset { 0xAA } else { 0 }
            );
        }
        let mut bus = bus();
        bus.port_write(0x63, 1, 0x99).unwrap();
        bus.tick(1_000);
        bus.port_write(0x61, 1, 8).unwrap();
        bus.tick(193);
        bus.port_write(0x61, 1, 0x48).unwrap();
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xAA);
    }

    #[test]
    fn full_keyboard_queue_reports_overrun_and_restores_marker() {
        let mut bus = initialized();
        xt(&mut bus).key_event(XtKey::Escape, true);
        for _ in 0..16 {
            xt(&mut bus).key_event(XtKey::Digit1, true);
        }
        xt(&mut bus).key_event(XtKey::KeyA, true);
        xt(&mut bus).key_event(XtKey::KeyA, false);
        assert_eq!(xt(&mut bus).queue.len(), 16);
        let full = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&full).unwrap();
        for byte in std::iter::once(1)
            .chain(std::iter::repeat_n(2, 15))
            .chain(std::iter::once(0xFF))
        {
            assert!(bus.device().irq_pending());
            assert_eq!(bus.port_read(0x60, 1).unwrap(), byte);
            if byte == 0xFF {
                let received = bus.device().save();
                bus.device_mut().reset();
                bus.device_mut().restore(&received).unwrap();
                assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xFF);
                assert!(bus.device().irq_pending());
            }
            acknowledge(&mut bus);
            bus.tick(1);
        }
        assert!(!bus.device().irq_pending());
        xt(&mut bus).key_event(XtKey::KeyB, true);
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x30);
        let before = bus.device().save();
        let mut oversized = full;
        oversized.push(1);
        assert!(bus.device_mut().restore(&oversized).is_err());
        assert_eq!(bus.device().save(), before);
    }

    #[test]
    fn bios_reset_loop_succeeds_at_one_retired_step_per_pit_clock() {
        let mut bus = bus();
        bus.port_write(0x63, 1, 0x99).unwrap();
        bus.port_write(0x61, 1, 0x08).unwrap();
        for _ in 0..10_582 {
            bus.tick(1);
        }
        assert_eq!(bus.clock(), VClock(10_582));
        bus.port_write(0x61, 1, 0xC8).unwrap();
        bus.port_write(0x61, 1, 0x48).unwrap();
        assert!(!bus.device().irq_pending());
        bus.tick(1);
        assert!(bus.device().irq_pending());
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xAA);
    }

    #[test]
    fn keyboard_reset_preserves_board_receive_latch_until_pb7_clear() {
        let mut bus = initialized();
        xt(&mut bus).key_event(XtKey::KeyA, true);
        xt(&mut bus).key_event(XtKey::KeyB, true);
        bus.port_write(0x61, 1, 0).unwrap();
        bus.tick(1_193);
        bus.port_write(0x61, 1, 0x40).unwrap();
        assert!(bus.device().irq_pending());
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x1E);
        let occupied = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&occupied).unwrap();
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x1E);
        acknowledge(&mut bus);
        bus.tick(1);
        assert!(bus.device().irq_pending());
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xAA);
        acknowledge(&mut bus);
        bus.tick(1);
        assert!(!bus.device().irq_pending());
    }

    #[test]
    fn ppi_aliases_share_registers_and_keyboard_wiring() {
        let mut bus = bus();
        bus.device_mut()
            .set_trace_policy(device::TracePolicy::Ring { capacity: 256 });
        for control in [0x63, 0x67, 0x6B, 0x6F] {
            bus.port_write(control, 1, 0x80).unwrap();
            for port in 0x60..=0x6F {
                assert_eq!(
                    bus.port_read(port, 1).unwrap(),
                    if port & 3 == 3 { 0xFF } else { 0 }
                );
            }
        }
        for group in 0..4 {
            let base = 0x60 + group * 4;
            bus.port_write(base, 1, 0x20 + u32::from(group)).unwrap();
            bus.port_write(base + 1, 1, 0x40 + u32::from(group))
                .unwrap();
            bus.port_write(base + 2, 1, 0x30 + u32::from(group))
                .unwrap();
            assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x20 + u32::from(group));
            assert_eq!(bus.port_read(0x61, 1).unwrap(), 0x40 + u32::from(group));
            assert_eq!(bus.port_read(0x62, 1).unwrap(), 0x30 + u32::from(group));
        }
        assert!(bus.device_mut().take_trace().contains(&TraceEvent::Port {
            at: VClock::ZERO,
            port: 0x6D,
            size: 1,
            dir: AccessDir::Write,
            value: 0x43,
        }));
        bus.port_write(0x67, 1, 0x99).unwrap();
        xt(&mut bus).key_event(XtKey::KeyA, true);
        bus.port_write(0x65, 1, 0x40).unwrap();
        assert_eq!(bus.port_read(0x64, 1).unwrap(), 0x1E);
        assert!(bus.device().irq_pending());
        bus.port_write(0x69, 1, 0xC0).unwrap();
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.port_read(0x6C, 1).unwrap(), 0);
        bus.port_write(0x6D, 1, 0x40).unwrap();
        bus.tick(1);
        assert!(!bus.device().irq_pending());
        for port in [0x5F, 0x70] {
            assert!(bus.port_read(port, 1).is_err());
            assert!(bus.port_write(port, 1, 0).is_err());
        }
    }

    #[test]
    fn host_events_keep_set1_order_and_separate_irq_edges() {
        let mut bus = initialized();
        for (key, pressed) in [
            (XtKey::KeyA, true),
            (XtKey::KeyA, false),
            (XtKey::F10, true),
            (XtKey::NumpadDecimal, false),
        ] {
            xt(&mut bus).key_event(key, pressed);
        }
        for byte in [0x1E, 0x9E, 0x44, 0xD3] {
            assert!(bus.device().irq_pending());
            assert_eq!(bus.port_read(0x60, 1).unwrap(), byte);
            acknowledge(&mut bus);
            assert_eq!(
                bus.device().next_event(bus.clock()),
                NextEvent::At(bus.clock().advance(1))
            );
            let now = bus.clock();
            bus.device_mut().sync(now);
            assert!(!bus.device().irq_pending());
            bus.tick(1);
        }
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
    }

    #[test]
    fn all_83_positions_emit_one_make_and_one_break() {
        let mut bus = initialized();
        let keys = [
            XtKey::Escape,
            XtKey::Digit1,
            XtKey::Digit2,
            XtKey::Digit3,
            XtKey::Digit4,
            XtKey::Digit5,
            XtKey::Digit6,
            XtKey::Digit7,
            XtKey::Digit8,
            XtKey::Digit9,
            XtKey::Digit0,
            XtKey::Minus,
            XtKey::Equal,
            XtKey::Backspace,
            XtKey::Tab,
            XtKey::KeyQ,
            XtKey::KeyW,
            XtKey::KeyE,
            XtKey::KeyR,
            XtKey::KeyT,
            XtKey::KeyY,
            XtKey::KeyU,
            XtKey::KeyI,
            XtKey::KeyO,
            XtKey::KeyP,
            XtKey::BracketLeft,
            XtKey::BracketRight,
            XtKey::Enter,
            XtKey::ControlLeft,
            XtKey::KeyA,
            XtKey::KeyS,
            XtKey::KeyD,
            XtKey::KeyF,
            XtKey::KeyG,
            XtKey::KeyH,
            XtKey::KeyJ,
            XtKey::KeyK,
            XtKey::KeyL,
            XtKey::Semicolon,
            XtKey::Quote,
            XtKey::Backquote,
            XtKey::ShiftLeft,
            XtKey::Backslash,
            XtKey::KeyZ,
            XtKey::KeyX,
            XtKey::KeyC,
            XtKey::KeyV,
            XtKey::KeyB,
            XtKey::KeyN,
            XtKey::KeyM,
            XtKey::Comma,
            XtKey::Period,
            XtKey::Slash,
            XtKey::ShiftRight,
            XtKey::NumpadMultiply,
            XtKey::AltLeft,
            XtKey::Space,
            XtKey::CapsLock,
            XtKey::F1,
            XtKey::F2,
            XtKey::F3,
            XtKey::F4,
            XtKey::F5,
            XtKey::F6,
            XtKey::F7,
            XtKey::F8,
            XtKey::F9,
            XtKey::F10,
            XtKey::NumLock,
            XtKey::ScrollLock,
            XtKey::Numpad7,
            XtKey::Numpad8,
            XtKey::Numpad9,
            XtKey::NumpadSubtract,
            XtKey::Numpad4,
            XtKey::Numpad5,
            XtKey::Numpad6,
            XtKey::NumpadAdd,
            XtKey::Numpad1,
            XtKey::Numpad2,
            XtKey::Numpad3,
            XtKey::Numpad0,
            XtKey::NumpadDecimal,
        ];
        assert_eq!(keys.len(), 83);
        for (index, key) in keys.into_iter().enumerate() {
            for (pressed, byte) in [(true, index as u32 + 1), (false, index as u32 + 0x81)] {
                xt(&mut bus).key_event(key, pressed);
                assert_eq!(bus.port_read(0x60, 1).unwrap(), byte);
                acknowledge(&mut bus);
                bus.tick(1);
            }
        }
    }

    #[test]
    fn snapshot_preserves_queue_deadline_pins_and_reset_interval() {
        let mut bus = initialized();
        xt(&mut bus).key_event(XtKey::KeyA, true);
        xt(&mut bus).key_event(XtKey::KeyB, false);
        xt(&mut bus).set_status_inputs(true, false, true);
        let occupied = bus.device().save();
        acknowledge(&mut bus);
        let waiting = bus.device().save();
        let refill_at = bus.clock().advance(1);
        bus.tick(1);
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xB0);
        bus.device_mut().restore(&occupied).unwrap();
        assert!(bus.device().irq_pending());
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x1E);
        bus.device_mut().restore(&waiting).unwrap();
        assert!(!bus.device().irq_pending());
        bus.device_mut().tick(refill_at);
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xB0);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0xAD);
        acknowledge(&mut bus);
        bus.tick(1);
        bus.port_write(0x61, 1, 0).unwrap();
        bus.tick(RESET_LOW_CLOCKS - 1);
        let armed = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&armed).unwrap();
        bus.tick(1);
        bus.port_write(0x61, 1, 0x40).unwrap();
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xAA);
        let before = bus.device().save();
        for invalid in [
            vec![],
            {
                let mut b = before.clone();
                b[0] = 1;
                b
            },
            {
                let mut b = before.clone();
                b[1] ^= 1;
                b
            },
            {
                let mut b = before.clone();
                b[5] = 2;
                b
            },
            {
                let mut b = before.clone();
                b.push(0xE0);
                b
            },
            {
                let mut b = before.clone();
                b[33] ^= 1;
                b
            },
            {
                let mut b = before.clone();
                b[7] = 1;
                b[41..49].copy_from_slice(&u64::MAX.to_le_bytes());
                b
            },
        ] {
            assert!(bus.device_mut().restore(&invalid).is_err());
            assert_eq!(bus.device().save(), before);
        }
    }
}
