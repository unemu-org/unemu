// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8042 keyboard controller (§7.5).
//!
//! Models the PC/AT "keyboard controller" — the 8042 microcontroller sitting
//! between the keyboard's serial line and the ISA bus. Two ports are decoded:
//! `0x60` (data) and `0x64` (status on read, command on write).
//!
//! The controller owns an **output buffer** (the byte the CPU reads at `0x60`),
//! a **status register** (`0x64` read), an internal **command byte** (the
//! controller's configuration, read/written through the `0x20`/`0x60` command
//! pair), and an **output port** whose bit 1 is the **A20 gate**. Scancodes
//! arriving from the keyboard are modelled as an input queue that
//! [`I8042::push_scancode`] feeds; a queued byte moves into the output buffer,
//! sets OBF, and — when keyboard interrupts are enabled — asserts the **IRQ1**
//! line surfaced through [`Device::irq_pending`].
//!
//! ## A20 gate
//!
//! Command `0xD1` (write output port) latches a new output port; bit 1 is the
//! A20 gate, exposed through [`I8042::a20_enabled`]. This device only models the
//! *gate bit as state*. The actual masking of address line 20 on guest memory
//! accesses is a CPU/bus concern handled elsewhere and is deliberately **out of
//! scope here** (see the A20 note in `backlog/docs/architecture.md`).
//!
//! ## Coverage matrix (datasheet vs modelled)
//!
//! Controller commands written to `0x64`:
//!
//! | Cmd    | Meaning                     | Modelled | Notes                        |
//! |--------|-----------------------------|----------|------------------------------|
//! | `0x20` | Read command byte           | yes      | → output buffer              |
//! | `0x60` | Write command byte          | yes      | next `0x60` write is the byte |
//! | `0xAA` | Controller self-test        | yes      | → `0x55` (pass)              |
//! | `0xAB` | Keyboard interface test     | yes      | → `0x00` (no error)          |
//! | `0xAD` | Disable keyboard interface  | yes      | sets command-byte bit 4       |
//! | `0xAE` | Enable keyboard interface   | yes      | clears command-byte bit 4     |
//! | `0xA7` | Disable aux (mouse)         | yes      | sets command-byte bit 5       |
//! | `0xA8` | Enable aux (mouse)          | yes      | clears command-byte bit 5     |
//! | `0xA9` | Test aux port               | yes      | → `0x00` (no error)          |
//! | `0xC0` | Read input port             | yes      | → benign strap value         |
//! | `0xD0` | Read output port            | yes      | → output buffer              |
//! | `0xD1` | Write output port           | yes      | next `0x60` write; bit 1 = A20 |
//! | `0xD4` | Write byte to aux (mouse)   | yes      | next `0x60` write routed to aux |
//!
//! ## PS/2 auxiliary (mouse) channel: interface present, no device
//!
//! The stock BIOS probes the aux channel (aux-interface test `0xA9`, aux
//! enable/disable, `0xD4` write-to-aux). We model the **interface as present and
//! healthy** — `0xA9` returns "no error" — but **no mouse is attached**. A byte
//! routed to the aux device via `0xD4` is accepted and **dropped**: no aux output
//! byte, no aux ACK, no aux OBF. The BIOS's mouse-detection reads then time out
//! cleanly (`jc .timeout`) rather than blocking on a byte that never arrives.
//!
//! ## Keyboard reset / BAT
//!
//! A plain host-to-keyboard byte (`0x60` data write) is acknowledged with a single
//! `0xFA` (ACK). The one keyboard command interpreted is **reset** (`0xFF`): it is
//! ACKed with `0xFA` and then, modelling the keyboard's power-on self-test passing,
//! yields `0xAA` (**BAT OK**) so the BIOS's post-reset wait for `0xAA` completes.
//!
//! Deliberately **not** modelled (decoded as no-ops where a byte is consumed):
//! - **The PS/2 auxiliary (mouse) device** itself and the aux output buffer / IRQ12
//!   (see the aux-channel note above — the interface answers, no device does).
//! - **Password / security commands** (`0xA4`–`0xA6`).
//! - **`0xFE` pulse-reset** (system reset via the output port) and `0xF0`–`0xFF`
//!   output-line pulses: the CPU-reset seam is out of scope here.
//! - **Keyboard-side command semantics** beyond reset/ACK: the keyboard's own
//!   command set (LED set, typematic, scancode set select) is not interpreted.
//! - **Test inputs** read (`0xE0`).

use std::collections::VecDeque;

use device::{AccessDir, Device, DeviceError, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "8042-kbc";

/// Data port (read: output buffer; write: to keyboard or a pending command byte).
const PORT_DATA: u16 = 0x60;

/// Status port on read; command port on write.
const PORT_CMD: u16 = 0x64;

/// Snapshot layout version tag, first byte of [`Device::save`] output.
const SNAPSHOT_VERSION: u8 = 1;

/// Status register bit 0: output buffer full (a byte is ready to read at `0x60`).
const ST_OBF: u8 = 1 << 0;
/// Status register bit 1: input buffer full (a written byte is not yet consumed).
const ST_IBF: u8 = 1 << 1;
/// Status register bit 2: system flag (mirrors command-byte bit 2).
const ST_SYS: u8 = 1 << 2;
/// Status register bit 3: command/data — set when the last write was to `0x64`.
const ST_CMD: u8 = 1 << 3;
/// Status register bit 4: keyboard not inhibited (unlocked). Always reported set.
const ST_UNLOCKED: u8 = 1 << 4;

/// Command-byte bit 0: keyboard-interrupt (IRQ1) enable.
const CB_KBD_INT: u8 = 1 << 0;
/// Command-byte bit 2: system flag, echoed into the status register.
const CB_SYS: u8 = 1 << 2;
/// Command-byte bit 4: keyboard interface disabled.
const CB_KBD_DISABLE: u8 = 1 << 4;
/// Command-byte bit 5: aux interface disabled.
const CB_AUX_DISABLE: u8 = 1 << 5;

/// Output-port bit 0: CPU reset line (active low). Stored; reset seam is elsewhere.
const OP_CPU_RESET: u8 = 1 << 0;
/// Output-port bit 1: the A20 gate. Set = A20 enabled.
const OP_A20: u8 = 1 << 1;

/// Self-test (`0xAA`) success response byte.
const SELF_TEST_PASS: u8 = 0x55;
/// Interface-test (`0xAB`/`0xA9`) success response byte.
const IFACE_TEST_PASS: u8 = 0x00;
/// Keyboard acknowledge byte returned for a host-to-keyboard data write.
const KBD_ACK: u8 = 0xFA;
/// Keyboard "reset" command; the host writes it to the keyboard via `0x60`.
const KBD_CMD_RESET: u8 = 0xFF;
/// Basic-assurance-test "OK" byte the keyboard emits after a successful reset.
const KBD_BAT_OK: u8 = 0xAA;

/// Value returned for a read-input-port (`0xC0`) command.
///
/// The input port carries board straps the BIOS samples but that a headless AT
/// has no real wiring for. A benign constant keeps POST moving: keyboard **not
/// inhibited** (bit 7), color/CGA-class display (bit 6 clear), manufacturing
/// jumper **absent** (bit 5 set, i.e. normal operation rather than the factory
/// test loop). The low nibble is unwired and read as zero.
const INPUT_PORT_VALUE: u8 = 0b1010_0000;

/// Default command byte at power-on: keyboard enabled, interrupts off,
/// translation off. The BIOS reprograms this early in POST.
const CMD_BYTE_RESET: u8 = 0x00;

/// Default output port at power-on: A20 enabled (matching the "A20 always-on for
/// now" decision) and the CPU-reset line inactive (high).
const OUTPUT_PORT_RESET: u8 = OP_A20 | OP_CPU_RESET;

/// What a subsequent write to the data port (`0x60`) means, set by the preceding
/// command on `0x64`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Expect {
    /// A byte destined for the keyboard device (the default).
    KeyboardData,
    /// The new controller command byte (after command `0x60`).
    CommandByte,
    /// The new output port value (after command `0xD1`).
    OutputPort,
    /// A byte to route to the auxiliary (mouse) device (after command `0xD4`).
    /// With no mouse attached the byte is dropped.
    AuxData,
}

impl Expect {
    fn to_bits(self) -> u8 {
        match self {
            Expect::KeyboardData => 0,
            Expect::CommandByte => 1,
            Expect::OutputPort => 2,
            Expect::AuxData => 3,
        }
    }

    fn from_bits(bits: u8) -> Expect {
        match bits {
            1 => Expect::CommandByte,
            2 => Expect::OutputPort,
            3 => Expect::AuxData,
            _ => Expect::KeyboardData,
        }
    }
}

/// Where the byte currently in the output buffer came from. Only keyboard-sourced
/// bytes drive the IRQ1 line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Source {
    /// The buffer is empty.
    Empty,
    /// A scancode (or keyboard ACK) from the keyboard.
    Keyboard,
    /// A controller command response (self-test result, command byte, …).
    Controller,
}

impl Source {
    fn to_bits(self) -> u8 {
        match self {
            Source::Empty => 0,
            Source::Keyboard => 1,
            Source::Controller => 2,
        }
    }

    fn from_bits(bits: u8) -> Source {
        match bits {
            1 => Source::Keyboard,
            2 => Source::Controller,
            _ => Source::Empty,
        }
    }
}

/// The 8042 keyboard controller (§7.5).
///
/// Decodes ports `0x60` and `0x64`. Drives the **IRQ1** line through
/// [`Device::irq_pending`] when a keyboard byte is ready and keyboard interrupts
/// are enabled in the command byte; [`Device::irq_vector`] is `None` because the
/// 8259 PIC owns vectoring — the controller only drives a line (§5.4).
pub struct I8042 {
    /// The byte the CPU reads at `0x60`.
    output_buffer: u8,
    /// Output-buffer-full: a byte is waiting to be read.
    output_full: bool,
    /// Provenance of the buffered byte (gates IRQ1).
    source: Source,
    /// Input-buffer-full. Writes are consumed synchronously, so this reads clear;
    /// it exists to model the status bit faithfully.
    input_full: bool,
    /// Set when the last guest write targeted the command port (`0x64`).
    last_was_command: bool,
    /// The controller configuration byte (read via `0x20`, written via `0x60`).
    command_byte: u8,
    /// The output port; bit 1 is the A20 gate.
    output_port: u8,
    /// What the next `0x60` write means.
    expect: Expect,
    /// Scancodes waiting to enter the output buffer.
    input_queue: VecDeque<u8>,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: Vec<TraceEvent>,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl I8042 {
    /// Create an 8042 in power-on state decoding ports `0x60` and `0x64`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            output_buffer: 0,
            output_full: false,
            source: Source::Empty,
            input_full: false,
            last_was_command: false,
            command_byte: CMD_BYTE_RESET,
            output_port: OUTPUT_PORT_RESET,
            expect: Expect::KeyboardData,
            input_queue: VecDeque::new(),
            trace: Vec::new(),
            now: VClock::ZERO,
        }
    }

    /// Push a scancode from the keyboard into the input queue. If the output
    /// buffer is free it moves in immediately, setting OBF and (when enabled)
    /// asserting IRQ1.
    pub fn push_scancode(&mut self, scancode: u8) {
        self.input_queue.push_back(scancode);
        self.refill_output();
    }

    /// Whether the A20 gate bit is currently set in the output port.
    ///
    /// This is only the modelled gate *state*; masking address line 20 on guest
    /// accesses is handled outside this device (see the module docs).
    #[must_use]
    pub fn a20_enabled(&self) -> bool {
        self.output_port & OP_A20 != 0
    }

    /// The current output port value (bit 1 = A20, bit 0 = CPU reset line).
    #[must_use]
    pub fn output_port(&self) -> u8 {
        self.output_port
    }

    /// The current controller command byte.
    #[must_use]
    pub fn command_byte(&self) -> u8 {
        self.command_byte
    }

    /// Assemble the status register read back at `0x64`.
    fn status(&self) -> u8 {
        let mut st = ST_UNLOCKED;
        if self.output_full {
            st |= ST_OBF;
        }
        if self.input_full {
            st |= ST_IBF;
        }
        if self.command_byte & CB_SYS != 0 {
            st |= ST_SYS;
        }
        if self.last_was_command {
            st |= ST_CMD;
        }
        st
    }

    /// Load the next queued scancode into the output buffer if it is free and the
    /// keyboard interface is enabled.
    fn refill_output(&mut self) {
        if self.output_full || self.command_byte & CB_KBD_DISABLE != 0 {
            return;
        }
        if let Some(scancode) = self.input_queue.pop_front() {
            self.output_buffer = scancode;
            self.output_full = true;
            self.source = Source::Keyboard;
        }
    }

    /// Place a controller-sourced response byte into the output buffer.
    fn respond(&mut self, byte: u8) {
        self.output_buffer = byte;
        self.output_full = true;
        self.source = Source::Controller;
    }

    /// Read and consume the output buffer (data-port read).
    fn read_data(&mut self) -> u8 {
        let value = self.output_buffer;
        self.output_full = false;
        self.source = Source::Empty;
        // A freed buffer immediately accepts the next queued scancode.
        self.refill_output();
        value
    }

    /// Service a write to the command port (`0x64`).
    fn write_command(&mut self, cmd: u8) {
        self.last_was_command = true;
        match cmd {
            0x20 => {
                let cb = self.command_byte;
                self.respond(cb);
            }
            0x60 => self.expect = Expect::CommandByte,
            0xAA => {
                // Controller self-test also sets the system flag on real POST.
                self.command_byte |= CB_SYS;
                self.respond(SELF_TEST_PASS);
            }
            0xAB => self.respond(IFACE_TEST_PASS),
            0xA9 => self.respond(IFACE_TEST_PASS),
            0xAD => self.command_byte |= CB_KBD_DISABLE,
            0xAE => {
                self.command_byte &= !CB_KBD_DISABLE;
                // Re-enabling the interface may release a queued scancode.
                self.refill_output();
            }
            0xA7 => self.command_byte |= CB_AUX_DISABLE,
            0xA8 => self.command_byte &= !CB_AUX_DISABLE,
            0xC0 => self.respond(INPUT_PORT_VALUE),
            0xD0 => {
                let op = self.output_port;
                self.respond(op);
            }
            0xD1 => self.expect = Expect::OutputPort,
            0xD4 => self.expect = Expect::AuxData,
            // Unmodelled commands (password, output-line pulses, resets, …) are ignored.
            _ => {}
        }
    }

    /// Service a write to the data port (`0x60`).
    fn write_data(&mut self, byte: u8) {
        self.last_was_command = false;
        match self.expect {
            Expect::KeyboardData => {
                // Host-to-keyboard byte: acknowledge. The keyboard command set is
                // not interpreted, except reset, which additionally reports its
                // power-on self-test (BAT) as passing so the BIOS's wait completes.
                self.input_queue.push_back(KBD_ACK);
                if byte == KBD_CMD_RESET {
                    self.input_queue.push_back(KBD_BAT_OK);
                }
                self.refill_output();
            }
            Expect::CommandByte => {
                self.command_byte = byte;
                self.expect = Expect::KeyboardData;
                // A changed enable/disable bit may gate or release a scancode.
                self.refill_output();
            }
            Expect::OutputPort => {
                self.output_port = byte;
                self.expect = Expect::KeyboardData;
            }
            Expect::AuxData => {
                // Routed to the (absent) mouse: accept and drop it. No aux device
                // answers, so no byte reaches the output buffer and the BIOS's
                // mouse-detect read times out cleanly instead of hanging.
                self.expect = Expect::KeyboardData;
            }
        }
    }

    /// Record a serviced port access on the tracing seam.
    fn trace_port(&mut self, port: u16, size: u8, dir: AccessDir, value: u32) {
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir,
            value,
        });
    }
}

impl Default for I8042 {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for I8042 {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port {
            PORT_DATA => u32::from(self.read_data()),
            PORT_CMD => u32::from(self.status()),
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                })
            }
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let byte = value as u8;
        match port {
            PORT_DATA => self.write_data(byte),
            PORT_CMD => self.write_command(byte),
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                })
            }
        }
        self.trace_port(port, size, AccessDir::Write, value);
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }

    fn mmio_write(&mut self, addr: u64, _size: u8, _value: u64) -> Result<(), DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }

    fn tick(&mut self, now: VClock) {
        self.now = now;
    }

    fn irq_pending(&self) -> bool {
        // IRQ1 asserts when a keyboard byte is ready and keyboard interrupts are
        // enabled. Controller command responses do not drive the line.
        self.output_full && self.source == Source::Keyboard && self.command_byte & CB_KBD_INT != 0
    }

    fn irq_vector(&self) -> Option<u8> {
        // The controller drives IRQ1 as a line; the 8259 PIC resolves it to a
        // vector on INTA (§5.4).
        None
    }

    fn reset(&mut self) {
        *self = I8042::new();
    }

    fn save(&self) -> Vec<u8> {
        // Layout: version, command byte, output port, output buffer, flags,
        // expect, source, queue length (2), queue bytes.
        let mut blob = Vec::with_capacity(8 + self.input_queue.len());
        blob.push(SNAPSHOT_VERSION);
        blob.push(self.command_byte);
        blob.push(self.output_port);
        blob.push(self.output_buffer);
        let mut flags = 0u8;
        if self.output_full {
            flags |= 1 << 0;
        }
        if self.input_full {
            flags |= 1 << 1;
        }
        if self.last_was_command {
            flags |= 1 << 2;
        }
        blob.push(flags);
        blob.push(self.expect.to_bits());
        blob.push(self.source.to_bits());
        let len = self.input_queue.len() as u16;
        blob.extend_from_slice(&len.to_le_bytes());
        blob.extend(self.input_queue.iter().copied());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        const HEADER: usize = 1 + 1 + 1 + 1 + 1 + 1 + 1 + 2;
        if snapshot.len() < HEADER {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot shorter than header",
            });
        }
        if snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        let command_byte = snapshot[1];
        let output_port = snapshot[2];
        let output_buffer = snapshot[3];
        let flags = snapshot[4];
        let expect = Expect::from_bits(snapshot[5]);
        let source = Source::from_bits(snapshot[6]);
        let len = u16::from_le_bytes([snapshot[7], snapshot[8]]) as usize;
        let body = &snapshot[HEADER..];
        if body.len() != len {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "queue length mismatch",
            });
        }
        self.command_byte = command_byte;
        self.output_port = output_port;
        self.output_buffer = output_buffer;
        self.output_full = flags & (1 << 0) != 0;
        self.input_full = flags & (1 << 1) != 0;
        self.last_was_command = flags & (1 << 2) != 0;
        self.expect = expect;
        self.source = source;
        self.input_queue = body.iter().copied().collect();
        Ok(())
    }

    fn take_trace(&mut self) -> Vec<TraceEvent> {
        std::mem::take(&mut self.trace)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        // Opt into the host-side downcast seam so the frontend can inject keyboard
        // scancodes ([`I8042::push_scancode`]) into this controller mid-run.
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    /// Command-byte bit 1: aux-interrupt (IRQ12) enable. Stored only; no mouse is
    /// modelled, so it exists only to exercise the command-byte round trip.
    const CB_AUX_INT: u8 = 1 << 1;

    #[test]
    fn power_on_state() {
        let kbc = I8042::new();
        assert!(!kbc.irq_pending());
        assert_eq!(kbc.irq_vector(), None);
        // A20 defaults enabled per the "A20 always-on for now" decision.
        assert!(kbc.a20_enabled());
    }

    #[test]
    fn status_reflects_command_vs_data_write() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xAD).unwrap(); // command write
        assert_ne!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_CMD, 0);
        kbc.port_write(PORT_DATA, 1, 0x00).unwrap(); // data write
        assert_eq!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_CMD, 0);
    }

    #[test]
    fn self_test_returns_0x55() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xAA).unwrap();
        // OBF set, buffer holds 0x55.
        assert_ne!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert_eq!(
            kbc.port_read(PORT_DATA, 1).unwrap(),
            u32::from(SELF_TEST_PASS)
        );
        // System flag was set by the self-test.
        assert_ne!(kbc.command_byte() & CB_SYS, 0);
    }

    #[test]
    fn interface_tests_return_no_error() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xAB).unwrap();
        assert_eq!(
            kbc.port_read(PORT_DATA, 1).unwrap(),
            u32::from(IFACE_TEST_PASS)
        );
        kbc.port_write(PORT_CMD, 1, 0xA9).unwrap();
        assert_eq!(
            kbc.port_read(PORT_DATA, 1).unwrap(),
            u32::from(IFACE_TEST_PASS)
        );
    }

    #[test]
    fn read_and_write_command_byte() {
        let mut kbc = I8042::new();
        // Write a command byte with keyboard interrupts enabled.
        kbc.port_write(PORT_CMD, 1, 0x60).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT | CB_AUX_INT))
            .unwrap();
        assert_eq!(kbc.command_byte(), CB_KBD_INT | CB_AUX_INT);
        // Read it back through 0x20.
        kbc.port_write(PORT_CMD, 1, 0x20).unwrap();
        assert_eq!(
            kbc.port_read(PORT_DATA, 1).unwrap() as u8,
            CB_KBD_INT | CB_AUX_INT
        );
    }

    #[test]
    fn scancode_sets_obf_and_reads_back() {
        let mut kbc = I8042::new();
        kbc.push_scancode(0x1E); // 'A' make code
        assert_ne!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x1E);
        // Buffer now empty.
        assert_eq!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
    }

    #[test]
    fn queued_scancodes_drain_in_order() {
        let mut kbc = I8042::new();
        kbc.push_scancode(0x10);
        kbc.push_scancode(0x11);
        kbc.push_scancode(0x12);
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x10);
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x11);
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x12);
    }

    #[test]
    fn irq1_asserted_only_when_enabled() {
        let mut kbc = I8042::new();
        // Interrupts disabled at reset: a scancode sets OBF but not IRQ1.
        kbc.push_scancode(0x20);
        assert!(!kbc.irq_pending());
        // Enable keyboard interrupts, then a fresh scancode raises IRQ1.
        kbc.port_read(PORT_DATA, 1).unwrap(); // drain
        kbc.port_write(PORT_CMD, 1, 0x60).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
        kbc.push_scancode(0x21);
        assert!(kbc.irq_pending());
        // Reading the byte deasserts the line.
        kbc.port_read(PORT_DATA, 1).unwrap();
        assert!(!kbc.irq_pending());
    }

    #[test]
    fn controller_response_does_not_raise_irq1() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0x60).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
        // A command response fills OBF but is controller-sourced.
        kbc.port_write(PORT_CMD, 1, 0xAA).unwrap();
        assert!(!kbc.irq_pending());
    }

    #[test]
    fn disable_keyboard_gates_the_queue() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xAD).unwrap(); // disable
        kbc.push_scancode(0x30);
        // Disabled interface holds the scancode out of the buffer.
        assert_eq!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        kbc.port_write(PORT_CMD, 1, 0xAE).unwrap(); // enable releases it
        assert_ne!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x30);
    }

    #[test]
    fn write_output_port_sets_a20() {
        let mut kbc = I8042::new();
        // Clear A20 via the output port.
        kbc.port_write(PORT_CMD, 1, 0xD1).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(OP_CPU_RESET))
            .unwrap();
        assert!(!kbc.a20_enabled());
        // Set it again.
        kbc.port_write(PORT_CMD, 1, 0xD1).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(OP_A20 | OP_CPU_RESET))
            .unwrap();
        assert!(kbc.a20_enabled());
    }

    #[test]
    fn read_output_port_returns_current_value() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xD0).unwrap();
        assert_eq!(
            kbc.port_read(PORT_DATA, 1).unwrap() as u8,
            OUTPUT_PORT_RESET
        );
    }

    #[test]
    fn keyboard_data_write_is_acknowledged() {
        let mut kbc = I8042::new();
        // Writing a keyboard command (e.g. set-LEDs 0xED) yields an ACK byte.
        kbc.port_write(PORT_DATA, 1, 0xED).unwrap();
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut kbc = I8042::new();
        assert_eq!(
            kbc.mmio_read(0x60, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x60,
            }
        );
    }

    #[test]
    fn unmapped_port_errors() {
        let mut kbc = I8042::new();
        assert_eq!(
            kbc.port_read(0x61, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 0x61,
            }
        );
    }

    #[test]
    fn reset_returns_to_power_on() {
        let mut kbc = I8042::new();
        kbc.push_scancode(0x40);
        kbc.port_write(PORT_CMD, 1, 0x60).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
        kbc.reset();
        assert_eq!(kbc.command_byte(), CMD_BYTE_RESET);
        assert!(!kbc.irq_pending());
        assert_eq!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
    }

    #[test]
    fn save_restore_round_trip() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0x60).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
        kbc.port_write(PORT_CMD, 1, 0xD1).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(OP_CPU_RESET))
            .unwrap(); // A20 off
        kbc.push_scancode(0x50);
        kbc.push_scancode(0x51);

        let blob = kbc.save();
        let mut restored = I8042::new();
        restored.restore(&blob).unwrap();
        assert_eq!(restored.command_byte(), CB_KBD_INT);
        assert!(!restored.a20_enabled());
        assert_eq!(restored.irq_pending(), kbc.irq_pending());
        // Queue survives and drains identically.
        assert_eq!(restored.port_read(PORT_DATA, 1).unwrap(), 0x50);
        assert_eq!(restored.port_read(PORT_DATA, 1).unwrap(), 0x51);
    }

    #[test]
    fn restore_rejects_bad_snapshot() {
        let mut kbc = I8042::new();
        assert_eq!(
            kbc.restore(&[SNAPSHOT_VERSION, 0]).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot shorter than header",
            }
        );
        let mut blob = kbc.save();
        blob[0] = 0xFF;
        assert_eq!(
            kbc.restore(&blob).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            }
        );
    }

    #[test]
    fn read_input_port_returns_benign_value() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xC0).unwrap();
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap() as u8, INPUT_PORT_VALUE);
        // Bit 7 (keyboard not inhibited) and bit 5 (manufacturing jumper absent)
        // must be set, else some BIOSes treat the board as locked or in test mode.
        assert_ne!(INPUT_PORT_VALUE & (1 << 7), 0);
        assert_ne!(INPUT_PORT_VALUE & (1 << 5), 0);
    }

    #[test]
    fn enable_disable_toggle_command_byte_bits() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xAD).unwrap();
        assert_ne!(kbc.command_byte() & CB_KBD_DISABLE, 0);
        kbc.port_write(PORT_CMD, 1, 0xAE).unwrap();
        assert_eq!(kbc.command_byte() & CB_KBD_DISABLE, 0);
        kbc.port_write(PORT_CMD, 1, 0xA7).unwrap();
        assert_ne!(kbc.command_byte() & CB_AUX_DISABLE, 0);
        kbc.port_write(PORT_CMD, 1, 0xA8).unwrap();
        assert_eq!(kbc.command_byte() & CB_AUX_DISABLE, 0);
    }

    #[test]
    fn aux_write_with_no_mouse_produces_no_byte() {
        let mut kbc = I8042::new();
        // Route a mouse reset (0xFF) to the absent aux device.
        kbc.port_write(PORT_CMD, 1, 0xD4).unwrap();
        kbc.port_write(PORT_DATA, 1, 0xFF).unwrap();
        // No aux device answers: the output buffer stays empty, so a detect read
        // sees no OBF and the BIOS times out rather than hanging.
        assert_eq!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert!(!kbc.irq_pending());
        // The controller is back to expecting keyboard data.
        kbc.port_write(PORT_DATA, 1, 0xED).unwrap();
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
    }

    #[test]
    fn aux_interface_test_passes_without_a_device() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_CMD, 1, 0xA9).unwrap();
        assert_eq!(
            kbc.port_read(PORT_DATA, 1).unwrap(),
            u32::from(IFACE_TEST_PASS)
        );
    }

    #[test]
    fn keyboard_reset_yields_ack_then_bat() {
        let mut kbc = I8042::new();
        kbc.port_write(PORT_DATA, 1, u32::from(KBD_CMD_RESET))
            .unwrap();
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_BAT_OK));
        // Nothing left after the reset handshake.
        assert_eq!(kbc.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
    }

    #[test]
    fn aux_expect_survives_save_restore() {
        let mut kbc = I8042::new();
        // Leave the controller mid-0xD4 (expecting an aux byte), then snapshot.
        kbc.port_write(PORT_CMD, 1, 0xD4).unwrap();
        let blob = kbc.save();
        let mut restored = I8042::new();
        restored.restore(&blob).unwrap();
        // The pending aux byte is still dropped after restore; no byte appears.
        restored.port_write(PORT_DATA, 1, 0xFF).unwrap();
        assert_eq!(restored.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
    }

    #[test]
    fn mock_bus_isolation_harness() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.tick(1);
        bus.port_write(PORT_CMD, 1, 0xAA).unwrap();
        assert_eq!(
            bus.port_read(PORT_DATA, 1).unwrap(),
            u32::from(SELF_TEST_PASS)
        );
    }
}
