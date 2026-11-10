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
//! ## Serial delivery of queued bytes (one IRQ1 edge per byte)
//!
//! The keyboard link is serial: a byte becomes available in the output buffer on
//! a transmission deadline, not the instant the previous byte is read at `0x60`.
//! Reading a keyboard byte starts the next frame's transmission time, shared by
//! scancodes and keyboard replies. The first byte after power-on remains available
//! immediately; bit-level signalling and host-to-keyboard transmission are not
//! modelled. Controller replies bypass the serial deadline. When the CPU reads a
//! byte the buffer empties and the IRQ1 line falls; only a later [`Device::tick`]
//! shifts the next queued byte in and re-raises the line. This
//! matters because the machine's PIC is edge-triggered: it latches one interrupt
//! per *rising* edge of the line. If a read instantly refilled the buffer the line
//! would never fall, so several queued scancodes would collapse into one sustained
//! high level and interrupt only once. Delivering on a tick — after holding one
//! tick so the fall is observed — gives every queued byte its own rising edge and
//! therefore its own IRQ1.
//!
//! Intel 8042 datasheet, "Data Bus Buffer" and "Interrupts", describes the
//! host buffer interface. The two PIT boundaries above are the compatibility
//! model's IRQ visibility policy, separate from `KEYBOARD_BYTE_CLOCKS`. See
//! `backlog/docs/design/scheduler.md` § Device migration inventory.
//!
//! ## Bounded buffering and response ordering
//!
//! IBM Personal Computer AT Technical Reference (March 1984), "Keyboard Buffer"
//! (p. 4-3), specifies a 16-character keyboard FIFO; "Keyboard Controller"
//! requires the host to receive an acknowledgement before sending the
//! next keyboard byte. This model limits queued keyboard replies to 16 bytes;
//! overflow drops the whole new reply (ACK and BAT together for reset), preserving
//! older bytes. This limit is a compatibility policy for a host that does not
//! wait for ACK, not a claim that the physical keyboard buffers 16 responses.
//! The host scancode queue stages an entire frontend script, rather than modelling
//! the physical keyboard FIFO, and does not consume reply capacity. Physical FIFO
//! overruns and the reserved seventeenth overrun-code position are not modelled.
//!
//! Intel UPI-41A/41AH/42/42AH User's Manual, "General Handshaking Protocol"
//! (p. 60, step 5), requires the UPI to wait for OBF=0 before writing DBBOUT.
//! Accordingly, a controller reply waits behind an occupied output buffer,
//! without displacing its keyboard byte. One pending controller reply is kept
//! and gets the next free output slot before further keyboard traffic, since
//! the controller can service its command while the keyboard link is blocked.
//! Further replies issued before that slot drains are dropped (keep oldest);
//! pipelined controller commands and firmware execution timing are not modelled.
//! Controller replies retain the existing polling-only policy (no IRQ1).
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
//! `0xFA` (ACK). **Reset** (`0xFF`) is
//! ACKed with `0xFA` and then, modelling the keyboard's power-on self-test passing,
//! yields `0xAA` (**BAT OK**) so the BIOS's post-reset wait for `0xAA` completes.
//! Set/Reset Status Indicators (`0xED`) records the following parameter byte;
//! bit 1 is NumLock. Keyboard/device reset clears the LEDs. IBM Personal
//! System/2 Hardware Interface Technical Reference — Common Interfaces (October
//! 1990), Keyboards (101- and 102-Key), "Set/Reset Status Indicators (Hex ED)",
//! "Reset (Hex FF)" and "Basic Assurance Test", specifies this state and LEDs
//! turning off at the end of BAT. See `backlog/docs/design/observation.md` § Host
//! input: frontend navigation wrappers follow these guest-controlled LEDs.
//!
//! Deliberately **not** modelled (decoded as no-ops where a byte is consumed):
//! - **The PS/2 auxiliary (mouse) device** itself and the aux output buffer / IRQ12
//!   (see the aux-channel note above — the interface answers, no device does).
//! - **Password / security commands** (`0xA4`–`0xA6`).
//! - **`0xFE` pulse-reset** (system reset via the output port) and `0xF0`–`0xFF`
//!   output-line pulses: the CPU-reset seam is out of scope here.
//! - **Keyboard-side command semantics** beyond reset/ACK and LED state:
//!   typematic-rate and scancode-set commands are not interpreted.
//! - **Test inputs** read (`0xE0`).
//!
//! Snapshots contain controller registers, tagged keyboard queues, pending
//! replies, IRQ refill holds, the absolute PIT time base and the pending serial
//! frame deadline (Intel 8042 datasheet, Data Bus Buffer and Interrupts), plus
//! guest keyboard LEDs and the pending LED parameter exchange.

use std::collections::VecDeque;

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "8042-kbc";

/// Data port (read: output buffer; write: to keyboard or a pending command byte).
const PORT_DATA: u16 = 0x60;

/// Status port on read; command port on write.
const PORT_CMD: u16 = 0x64;

/// Snapshot layout version tag, first byte of [`Device::save`] output.
const SNAPSHOT_VERSION: u8 = 4;

/// Keyboard frame time in PIT input clocks (1,193,182 Hz), rounded up.
///
/// IBM Personal Computer AT Technical Reference (March 1984), "Receiving Data
/// from the Keyboard" (p. 1-32), defines 11-bit frames and inhibits the link until
/// the host accepts the byte. It specifies a 2 ms receive timeout, not a baud rate.
/// For timing, this compatibility model uses IBM Personal System/2 Hardware
/// Interface Technical Reference (October 1990), "Keyboard and Auxiliary Device
/// Controller", "Auxiliary Device and System Timings", Figure 6 (p. 14):
/// 30–50 us per clock phase on the shared serial interface. Choose 50 us high +
/// 50 us low (10 kHz), the slow end of that range, to allow a full frame before
/// another keyboard byte can interrupt the host:
/// ceil(11 * 100 us * 1,193,182 Hz) = 1313 clocks.
const KEYBOARD_BYTE_CLOCKS: u64 = 1313;

/// Maximum queued keyboard reply bytes; see the module buffering policy.
const KEYBOARD_REPLY_CAPACITY: usize = 16;

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

/// A staged keyboard byte, tagged so frontend scripts do not consume reply slots.
struct KeyboardByte {
    value: u8,
    is_reply: bool,
}

/// The 8042 keyboard controller (§7.5).
///
/// Decodes ports `0x60` and `0x64`. Drives the **IRQ1** line through
/// [`Device::irq_pending`] when a keyboard byte is ready and keyboard interrupts
/// are enabled in the command byte; the
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
    keyboard_leds: u8,
    awaiting_leds: bool,
    /// Scancodes and keyboard replies waiting to enter the output buffer.
    input_queue: VecDeque<KeyboardByte>,
    /// Only command replies count toward the keyboard-side capacity limit.
    queued_replies: usize,
    /// A controller reply waiting for the host to empty DBBOUT.
    controller_reply: Option<u8>,
    /// Set when a read empties the output buffer, to hold the next serial refill
    /// for one tick so the IRQ1 line is observed low (its falling edge) before the
    /// next queued byte re-raises it. Preserved in snapshots.
    refill_hold: bool,
    /// Earliest next keyboard byte after the host releases the serial link.
    keyboard_ready_at: Option<VClock>,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
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
            keyboard_leds: 0,
            awaiting_leds: false,
            input_queue: VecDeque::new(),
            queued_replies: 0,
            controller_reply: None,
            refill_hold: false,
            keyboard_ready_at: None,
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// Push a scancode from the keyboard into the input queue. If the output
    /// buffer is free and neither serial timing nor IRQ visibility holds it, it
    /// moves in immediately, setting OBF and (when enabled) asserting IRQ1. This
    /// host staging queue accepts whole frontend scripts; see the module
    /// buffering policy.
    pub fn push_scancode(&mut self, scancode: u8) {
        self.input_queue.push_back(KeyboardByte {
            value: scancode,
            is_reply: false,
        });
        self.refill_output();
    }

    /// Last guest Set/Reset Status Indicators parameter; bit 1 is NumLock.
    /// IBM PS/2 Hardware Interface Technical Reference — Common Interfaces,
    /// Keyboards, "Set/Reset Status Indicators (Hex ED)". Reading this host-only
    /// state neither accesses a port nor changes the controller.
    #[must_use]
    pub fn keyboard_leds(&self) -> u8 {
        self.keyboard_leds
    }

    /// Whether a frontend can hand over one byte without staging it internally.
    /// Intel 8042 datasheet, "Data Bus Buffer", and Intel UPI-41A/41AH/42/42AH
    /// User's Manual, "General Handshaking Protocol": an unread output byte
    /// must be consumed first. This host-only readiness check also respects the
    /// model's serial deadline, IRQ hold and interface inhibit (§7.5); it never
    /// performs a guest port transaction or changes device state.
    #[must_use]
    pub fn host_input_ready(&self) -> bool {
        !self.output_full
            && self.input_queue.is_empty()
            && self.command_byte & CB_KBD_DISABLE == 0
            && !self.refill_hold
            && self.keyboard_ready_at.is_none_or(|at| self.now >= at)
            && self.controller_reply.is_none()
            && !self.awaiting_leds
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

    /// A waiting controller reply precedes keyboard traffic once DBBOUT is free.
    fn refill_output(&mut self) {
        if self.output_full {
            return;
        }
        if let Some(byte) = self.controller_reply.take() {
            self.output_buffer = byte;
            self.output_full = true;
            self.source = Source::Controller;
            return;
        }
        if self.refill_hold
            || self.keyboard_ready_at.is_some_and(|at| self.now < at)
            || self.command_byte & CB_KBD_DISABLE != 0
        {
            return;
        }
        if let Some(byte) = self.input_queue.pop_front() {
            if byte.is_reply {
                self.queued_replies -= 1;
            }
            self.output_buffer = byte.value;
            self.output_full = true;
            self.source = Source::Keyboard;
        }
    }

    /// Wait for OBF=0 as required by Intel's General Handshaking Protocol.
    fn respond(&mut self, byte: u8) {
        self.controller_reply.get_or_insert(byte);
        self.refill_output();
    }

    /// Read and consume the output buffer (data-port read).
    fn read_data(&mut self) -> u8 {
        let value = self.output_buffer;
        if self.output_full && self.source == Source::Keyboard {
            // Only a consumed keyboard byte releases the link for the next
            // frame. Empty reads and controller replies cannot restart it.
            self.keyboard_ready_at = Some(VClock(self.now.0.saturating_add(KEYBOARD_BYTE_CLOCKS)));
        }
        self.output_full = false;
        self.source = Source::Empty;
        // Keep the IRQ fall visible even when no serial frame is pending.
        self.refill_hold = true;
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
                // A command overrides an unfinished LED parameter exchange.
                // Replies retain the existing ACK/BAT ordering and capacity policy.
                if self.awaiting_leds && byte < 0x80 {
                    self.keyboard_leds = byte;
                    self.awaiting_leds = false;
                } else {
                    self.awaiting_leds = byte == 0xED;
                    if byte == KBD_CMD_RESET {
                        self.keyboard_leds = 0;
                    }
                }
                let reply_len = if byte == KBD_CMD_RESET { 2 } else { 1 };
                // Keep reset's ACK/BAT together: a lone ACK promises a BAT that
                // would never arrive, and a lone BAT has no matching command ACK.
                if self.queued_replies + reply_len <= KEYBOARD_REPLY_CAPACITY {
                    self.input_queue.push_back(KeyboardByte {
                        value: KBD_ACK,
                        is_reply: true,
                    });
                    if byte == KBD_CMD_RESET {
                        self.input_queue.push_back(KeyboardByte {
                            value: KBD_BAT_OK,
                            is_reply: true,
                        });
                    }
                    self.queued_replies += reply_len;
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
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now < self.now {
            return;
        }
        let elapsed = now.0.saturating_sub(self.now.0);
        self.now = now;
        if elapsed == 0 {
            return;
        }
        if self.keyboard_ready_at.is_some_and(|at| now >= at) {
            self.keyboard_ready_at = None;
        }
        // A read holds the line low through the first clock; only the second
        // clock may refill. Access synchronization must not consume that hold.
        if self.refill_hold {
            self.refill_hold = false;
            if elapsed == 1 {
                return;
            }
        }
        self.refill_output();
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        if self.refill_hold || (!self.output_full && self.controller_reply.is_some()) {
            self.now
                .0
                .checked_add(1)
                .map_or(NextEvent::Dormant, |at| NextEvent::At(VClock(at)))
        } else if let Some(at) = self.keyboard_ready_at.filter(|at| *at > self.now) {
            NextEvent::At(at)
        } else if !self.output_full
            && self.command_byte & CB_KBD_DISABLE == 0
            && !self.input_queue.is_empty()
        {
            self.now
                .0
                .checked_add(1)
                .map_or(NextEvent::Dormant, |at| NextEvent::At(VClock(at)))
        } else {
            NextEvent::Dormant
        }
    }

    fn irq_pending(&self) -> bool {
        // IRQ1 asserts when a keyboard byte is ready and keyboard interrupts are
        // enabled. Controller command responses do not drive the line.
        self.output_full && self.source == Source::Keyboard && self.command_byte & CB_KBD_INT != 0
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        *self = I8042::new();
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        // Layout: version, command byte, output port, output buffer, flags,
        // expect, source, queue length (8), pending controller reply,
        // clock (8), keyboard deadline (8), LEDs, awaiting LED parameter, queue pairs
        // (value, is_reply). Host scripts can exceed a u16 byte count.
        // Version 4 adds guest LEDs and the pending LED parameter exchange.
        let mut blob = Vec::with_capacity(34 + 2 * self.input_queue.len());
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
        if self.refill_hold {
            flags |= 1 << 3;
        }
        if self.controller_reply.is_some() {
            flags |= 1 << 4;
        }
        if self.keyboard_ready_at.is_some() {
            flags |= 1 << 5;
        }
        blob.push(flags);
        blob.push(self.expect.to_bits());
        blob.push(self.source.to_bits());
        let len = self.input_queue.len() as u64;
        blob.extend_from_slice(&len.to_le_bytes());
        blob.push(self.controller_reply.unwrap_or(0));
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob.extend_from_slice(
            &self
                .keyboard_ready_at
                .unwrap_or(VClock::ZERO)
                .0
                .to_le_bytes(),
        );
        blob.extend([self.keyboard_leds, u8::from(self.awaiting_leds)]);
        for byte in &self.input_queue {
            blob.extend([byte.value, u8::from(byte.is_reply)]);
        }
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        const HEADER: usize = 1 + 1 + 1 + 1 + 1 + 1 + 1 + 8 + 1 + 8 + 8 + 1 + 1;
        if snapshot
            .first()
            .is_some_and(|&version| version != SNAPSHOT_VERSION)
        {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        if snapshot.len() < HEADER {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot shorter than header",
            });
        }
        let command_byte = snapshot[1];
        let output_port = snapshot[2];
        let output_buffer = snapshot[3];
        let flags = snapshot[4];
        if flags & !0x3D != 0
            || snapshot[32] >= 0x80
            || snapshot[33] > 1
            || snapshot[5] > 3
            || snapshot[6] > 2
            || (flags & 1 != 0) != (snapshot[6] != 0)
            || (flags & 0x10 == 0 && snapshot[15] != 0)
            || (flags & 0x20 == 0 && snapshot[24..32] != [0; 8])
        {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid keyboard controller state",
            });
        }
        let now = u64::from_le_bytes(std::array::from_fn(|i| snapshot[16 + i]));
        let ready_at = u64::from_le_bytes(std::array::from_fn(|i| snapshot[24 + i]));
        if flags & 0x20 != 0 && (ready_at < now || ready_at - now > KEYBOARD_BYTE_CLOCKS) {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid keyboard deadline",
            });
        }
        let expect = Expect::from_bits(snapshot[5]);
        let source = Source::from_bits(snapshot[6]);
        let len = u64::from_le_bytes([
            snapshot[7],
            snapshot[8],
            snapshot[9],
            snapshot[10],
            snapshot[11],
            snapshot[12],
            snapshot[13],
            snapshot[14],
        ]);
        let body = &snapshot[HEADER..];
        if !body.len().is_multiple_of(2) || (body.len() / 2) as u64 != len {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "queue length mismatch",
            });
        }
        let (pairs, _) = body.as_chunks::<2>();
        if pairs.iter().any(|&[_, tag]| tag > 1) {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid keyboard byte tag",
            });
        }
        let queued_replies = pairs.iter().filter(|&&[_, tag]| tag != 0).count();
        if queued_replies > KEYBOARD_REPLY_CAPACITY {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "keyboard replies exceed capacity",
            });
        }
        self.keyboard_leds = snapshot[32];
        self.awaiting_leds = snapshot[33] != 0;
        self.command_byte = command_byte;
        self.output_port = output_port;
        self.output_buffer = output_buffer;
        self.output_full = flags & (1 << 0) != 0;
        self.input_full = flags & (1 << 1) != 0;
        self.last_was_command = flags & (1 << 2) != 0;
        self.expect = expect;
        self.source = source;
        self.input_queue = pairs
            .iter()
            .map(|&[value, tag]| KeyboardByte {
                value,
                is_reply: tag != 0,
            })
            .collect();
        self.queued_replies = queued_replies;
        self.refill_hold = flags & (1 << 3) != 0;
        self.controller_reply = (flags & (1 << 4) != 0).then_some(snapshot[15]);
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| {
            snapshot[16 + i]
        })));
        self.keyboard_ready_at = (flags & (1 << 5) != 0).then(|| {
            VClock(u64::from_le_bytes(std::array::from_fn(|i| {
                snapshot[24 + i]
            })))
        });
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

    fn bus_keyboard(bus: &mut MockBus) -> &mut I8042 {
        bus.device_mut()
            .as_any_mut()
            .unwrap()
            .downcast_mut::<I8042>()
            .unwrap()
    }

    fn set_leds(bus: &mut MockBus, leds: u8) {
        bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        bus.port_write(PORT_DATA, 1, u32::from(leds)).unwrap();
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        bus.tick(KEYBOARD_BYTE_CLOCKS);
    }

    #[test]
    fn keyboard_led_parameter_is_guest_controlled_and_accessor_has_no_side_effects() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.device_mut()
            .set_trace_policy(device::TracePolicy::Ring { capacity: 64 });
        assert_eq!(bus_keyboard(&mut bus).keyboard_leds(), 0);
        for leds in 0..8 {
            set_leds(&mut bus, leds);
            bus.device_mut().take_trace();
            let before = bus.device().save();
            assert_eq!(bus_keyboard(&mut bus).keyboard_leds(), leds);
            assert_eq!(bus_keyboard(&mut bus).keyboard_leds(), leds);
            assert_eq!(bus.device().save(), before);
            assert!(bus.device_mut().take_trace().is_empty());
        }
        bus.port_write(PORT_DATA, 1, 0).unwrap();
        assert_eq!(
            bus_keyboard(&mut bus).keyboard_leds(),
            7,
            "data without ED cannot change LEDs"
        );
    }

    #[test]
    fn keyboard_and_device_reset_clear_leds_and_cancel_a_pending_led_parameter() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        set_leds(&mut bus, 7);
        bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        bus.port_read(PORT_DATA, 1).unwrap();
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        assert_eq!(
            bus_keyboard(&mut bus).keyboard_leds(),
            7,
            "ED alone does not change LEDs"
        );
        bus.port_write(PORT_DATA, 1, u32::from(KBD_CMD_RESET))
            .unwrap();
        assert_eq!(bus_keyboard(&mut bus).keyboard_leds(), 0);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_BAT_OK));
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        bus.port_write(PORT_DATA, 1, 3).unwrap();
        assert_eq!(
            bus_keyboard(&mut bus).keyboard_leds(),
            0,
            "reset cancels ED"
        );
        bus.port_read(PORT_DATA, 1).unwrap();
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        set_leds(&mut bus, 2);
        bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        bus.device_mut().reset();
        assert_eq!(bus_keyboard(&mut bus).keyboard_leds(), 0);
        bus.port_write(PORT_DATA, 1, 3).unwrap();
        assert_eq!(bus_keyboard(&mut bus).keyboard_leds(), 0);
    }

    #[test]
    fn keyboard_snapshot_preserves_leds_and_the_pending_parameter_exchange() {
        let mut source = MockBus::new(Box::new(I8042::new()));
        set_leds(&mut source, 5);
        let mut restored = MockBus::new(Box::new(I8042::new()));
        let blob = source.device().save();
        assert_eq!(blob[0], 4);
        restored.device_mut().restore(&blob).unwrap();
        assert_eq!(bus_keyboard(&mut restored).keyboard_leds(), 5);
        assert_eq!(restored.device().save(), blob);
        for bus in [&mut source, &mut restored] {
            bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        }
        let pending = source.device().save();
        restored.device_mut().reset();
        restored.device_mut().restore(&pending).unwrap();
        for bus in [&mut source, &mut restored] {
            bus.port_write(PORT_DATA, 1, 2).unwrap();
            assert_eq!(bus_keyboard(bus).keyboard_leds(), 2);
        }
        assert_eq!(source.device().save(), restored.device().save());
    }

    #[test]
    fn keyboard_led_snapshot_rejects_legacy_and_invalid_blobs_before_mutation() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        set_leds(&mut bus, 5);
        bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        let before = bus.device().save();
        assert_eq!(before[0], 4);
        let mut legacy = before.clone();
        legacy.drain(32..34);
        legacy[0] = 3;
        assert!(matches!(
            bus.device_mut().restore(&legacy),
            Err(DeviceError::InvalidSnapshot {
                reason: "unsupported snapshot version",
                ..
            })
        ));
        assert_eq!(bus.device().save(), before);
        let mut invalid = before.clone();
        invalid[32] = 7;
        invalid[33] = 2;
        assert!(matches!(
            bus.device_mut().restore(&invalid),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(bus.device().save(), before);
        let mut trailing = before.clone();
        trailing.extend([0x1E, 0]);
        assert!(matches!(
            bus.device_mut().restore(&trailing),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(bus.device().save(), before);
        for length in 0..before.len() {
            assert!(matches!(
                bus.device_mut().restore(&before[..length]),
                Err(DeviceError::InvalidSnapshot { .. })
            ));
            assert_eq!(bus.device().save(), before);
        }
    }

    #[test]
    fn keyboard_snapshot_rejects_unreachable_led_bytes_without_mutation() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        set_leds(&mut bus, 0x7F);
        let before = bus.device().save();
        bus.device_mut().restore(&before).unwrap();
        assert_eq!(bus.device().save(), before);
        for leds in 0x80..=0xFF {
            let mut invalid = before.clone();
            invalid[32] = leds;
            assert!(
                matches!(
                    bus.device_mut().restore(&invalid),
                    Err(DeviceError::InvalidSnapshot { .. })
                ),
                "accepted unreachable LED byte {leds:#04x}"
            );
            assert_eq!(bus.device().save(), before, "LED byte {leds:#04x}");
        }
    }

    #[test]
    fn host_readiness_is_side_effect_free_and_checks_every_delivery_blocker() {
        for blocker in 0..8 {
            let mut bus = MockBus::new(Box::new(I8042::new()));
            bus.device_mut()
                .set_trace_policy(device::TracePolicy::Ring { capacity: 8 });
            let kbc = bus_keyboard(&mut bus);
            match blocker {
                0 => {}
                1 => kbc.output_full = true,
                2 => kbc.input_queue.push_back(KeyboardByte {
                    value: 0x1E,
                    is_reply: false,
                }),
                3 => kbc.command_byte |= CB_KBD_DISABLE,
                4 => kbc.refill_hold = true,
                5 => kbc.keyboard_ready_at = Some(VClock(10)),
                6 => kbc.controller_reply = Some(0x55),
                7 => kbc.awaiting_leds = true,
                _ => unreachable!(),
            }
            let before = bus.device().save();
            for _ in 0..2 {
                assert_eq!(
                    bus_keyboard(&mut bus).host_input_ready(),
                    blocker == 0,
                    "blocker {blocker}"
                );
            }
            assert_eq!(bus.device().save(), before);
            assert!(bus.device_mut().take_trace().is_empty());
        }
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus_keyboard(&mut bus).push_scancode(0x1E);
        bus.port_read(PORT_DATA, 1).unwrap();
        assert!(!bus_keyboard(&mut bus).host_input_ready());
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        assert!(bus_keyboard(&mut bus).host_input_ready());
        bus.port_write(PORT_CMD, 1, 0xAD).unwrap();
        assert!(!bus_keyboard(&mut bus).host_input_ready());
        bus.port_write(PORT_CMD, 1, 0xAE).unwrap();
        assert!(bus_keyboard(&mut bus).host_input_ready());
    }

    #[test]
    fn serial_scancodes_wait_for_a_complete_frame_after_read() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.tick(37);
        bus_keyboard(&mut bus).push_scancode(0x1E);
        bus_keyboard(&mut bus).push_scancode(0x9E);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x1E);
        bus.tick(2);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(37 + KEYBOARD_BYTE_CLOCKS))
        );
        bus.tick(KEYBOARD_BYTE_CLOCKS - 3);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        bus.tick(1);
        assert_ne!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x9E);
    }

    #[test]
    fn serial_keyboard_reply_waits_and_controller_response_bypasses_frame() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus_keyboard(&mut bus).push_scancode(0x1E);
        bus.port_write(PORT_DATA, 1, u32::from(KBD_CMD_RESET))
            .unwrap();
        bus.port_write(PORT_CMD, 1, 0xD0).unwrap();
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x1E);
        bus.tick(1);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        bus.tick(1);
        assert_eq!(
            bus.port_read(PORT_DATA, 1).unwrap(),
            u32::from(OUTPUT_PORT_RESET)
        );
        bus.port_write(PORT_CMD, 1, 0xAB).unwrap();
        assert_ne!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert_eq!(
            bus.port_read(PORT_DATA, 1).unwrap(),
            u32::from(IFACE_TEST_PASS)
        );
        bus.tick(KEYBOARD_BYTE_CLOCKS - 3);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        bus.tick(1);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        bus.tick(KEYBOARD_BYTE_CLOCKS - 1);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        bus.tick(1);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_BAT_OK));
    }

    #[test]
    fn serial_empty_reads_and_interface_toggles_do_not_restart_frame() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus_keyboard(&mut bus).push_scancode(0x1E);
        bus.port_read(PORT_DATA, 1).unwrap();
        bus.tick(100);
        bus.port_read(PORT_DATA, 1).unwrap();
        bus.port_write(PORT_CMD, 1, 0xAD).unwrap();
        bus_keyboard(&mut bus).push_scancode(0x9E);
        bus.port_write(PORT_CMD, 1, 0xAE).unwrap();
        bus.tick(KEYBOARD_BYTE_CLOCKS - 101);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        bus.tick(1);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x9E);
    }

    #[test]
    fn serial_snapshot_preserves_partially_elapsed_frame_and_clock() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.tick(71);
        bus_keyboard(&mut bus).push_scancode(0x1E);
        bus_keyboard(&mut bus).push_scancode(0x9E);
        bus.port_read(PORT_DATA, 1).unwrap();
        bus.tick(100);
        let blob = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&blob).unwrap();
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(71 + KEYBOARD_BYTE_CLOCKS))
        );
        bus.tick(KEYBOARD_BYTE_CLOCKS - 101);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        bus.tick(1);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x9E);
    }

    #[test]
    fn response_flood_stays_bounded_and_snapshot_restores() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.port_write(PORT_CMD, 1, 0xAD).unwrap();
        for _ in 0..65_537 {
            bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        }
        let blob = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&blob).unwrap();
        assert_eq!(bus_keyboard(&mut bus).input_queue.len(), 16);
        bus.port_write(PORT_CMD, 1, 0xAE).unwrap();
        for _ in 0..16 {
            assert_ne!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
            bus.tick(KEYBOARD_BYTE_CLOCKS);
        }
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
    }

    #[test]
    fn response_overflow_drops_whole_reset_reply_and_preserves_fifo() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.port_write(PORT_CMD, 1, 0xAD).unwrap();
        bus_keyboard(&mut bus).push_scancode(0x1E);
        for _ in 0..15 {
            bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        }
        // Only one position remains: neither ACK nor BAT may be queued alone.
        bus.port_write(PORT_DATA, 1, u32::from(KBD_CMD_RESET))
            .unwrap();
        assert_eq!(bus_keyboard(&mut bus).queued_replies, 15);
        bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        assert_eq!(bus_keyboard(&mut bus).queued_replies, 16);
        bus.port_write(PORT_CMD, 1, 0xAE).unwrap();
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x1E);
        for _ in 0..16 {
            bus.tick(KEYBOARD_BYTE_CLOCKS);
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        }
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        // Space becomes usable again after draining.
        bus.port_write(PORT_DATA, 1, u32::from(KBD_CMD_RESET))
            .unwrap();
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_BAT_OK));
    }

    #[test]
    fn host_scancode_script_exceeds_u16_without_consuming_reply_capacity() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.port_write(PORT_CMD, 1, 0xAD).unwrap();
        for byte in 0..65_537u32 {
            bus_keyboard(&mut bus).push_scancode(byte as u8);
        }
        let blob = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&blob).unwrap();
        assert_eq!(bus_keyboard(&mut bus).input_queue.len(), 65_537);
        assert_eq!(bus_keyboard(&mut bus).queued_replies, 0);
        bus.port_write(PORT_DATA, 1, 0xED).unwrap();
        assert_eq!(bus_keyboard(&mut bus).queued_replies, 1);
        bus.port_write(PORT_CMD, 1, 0xAE).unwrap();
        for byte in 0..65_537u32 {
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(byte as u8));
            bus.tick(KEYBOARD_BYTE_CLOCKS);
        }
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        bus.tick(KEYBOARD_BYTE_CLOCKS);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
    }

    #[test]
    fn controller_reply_waits_behind_keyboard_and_precedes_queued_scancodes() {
        for command in [0x20, 0xAA, 0xAB, 0xA9, 0xC0, 0xD0] {
            let mut bus = MockBus::new(Box::new(I8042::new()));
            bus.port_write(PORT_CMD, 1, 0x60).unwrap();
            bus.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
            bus_keyboard(&mut bus).push_scancode(0x1E);
            bus_keyboard(&mut bus).push_scancode(0x9E);
            bus.port_write(PORT_CMD, 1, command).unwrap();
            assert!(bus.device().irq_pending());
            let blob = bus.device().save();
            bus.device_mut().reset();
            bus.device_mut().restore(&blob).unwrap();
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x1E);
            assert!(!bus.device().irq_pending());
            bus.tick(2);
            let expected = match command {
                0x20 => CB_KBD_INT,
                0xAA => SELF_TEST_PASS,
                0xC0 => INPUT_PORT_VALUE,
                0xD0 => OUTPUT_PORT_RESET,
                _ => IFACE_TEST_PASS,
            };
            assert_ne!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
            assert!(!bus.device().irq_pending());
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(expected));
            bus.tick(KEYBOARD_BYTE_CLOCKS);
            assert!(bus.device().irq_pending());
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x9E);
        }
    }

    #[test]
    fn controller_reply_slot_preserves_first_response_when_host_pipelines() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus_keyboard(&mut bus).push_scancode(0x1E);
        bus.port_write(PORT_CMD, 1, 0xD0).unwrap();
        for _ in 0..32 {
            bus.port_write(PORT_CMD, 1, 0xAA).unwrap();
        }
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x1E);
        // Controller replies must progress even with the keyboard inhibited.
        bus.port_write(PORT_CMD, 1, 0xAD).unwrap();
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(1))
        );
        bus.tick(1);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(2))
        );
        bus.device_mut().sync(VClock(1));
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
        bus.tick(99);
        assert_eq!(
            bus.port_read(PORT_DATA, 1).unwrap(),
            u32::from(OUTPUT_PORT_RESET)
        );
        bus.tick(100);
        assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
    }

    #[test]
    fn all_keyboard_refill_paths_respect_low_irq_hold() {
        for path in 0..4 {
            let mut bus = MockBus::new(Box::new(I8042::new()));
            bus.port_write(PORT_CMD, 1, 0x60).unwrap();
            bus.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
            bus_keyboard(&mut bus).push_scancode(0x1E);
            if path == 1 || path == 2 {
                bus_keyboard(&mut bus).push_scancode(0x9E);
            }
            assert!(bus.device().irq_pending());
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x1E);
            match path {
                0 => bus_keyboard(&mut bus).push_scancode(0x9E),
                1 => {
                    bus.port_write(PORT_CMD, 1, 0x60).unwrap();
                    bus.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
                }
                2 => {
                    bus.port_write(PORT_CMD, 1, 0xAD).unwrap();
                    bus.port_write(PORT_CMD, 1, 0xAE).unwrap();
                }
                _ => bus.port_write(PORT_DATA, 1, 0xED).unwrap(),
            }
            bus.device_mut().sync(VClock::ZERO);
            assert!(!bus.device().irq_pending(), "refill path {path}");
            assert_eq!(bus.port_read(PORT_CMD, 1).unwrap() as u8 & ST_OBF, 0);
            bus.tick(1);
            assert!(!bus.device().irq_pending());
            bus.tick(KEYBOARD_BYTE_CLOCKS - 1);
            assert!(bus.device().irq_pending());
            let expected = if path == 3 { KBD_ACK } else { 0x9E };
            assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), u32::from(expected));
        }
    }

    #[test]
    fn snapshot_preserves_low_irq_hold_before_host_injection() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.port_write(PORT_CMD, 1, 0x60).unwrap();
        bus.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
        bus_keyboard(&mut bus).push_scancode(0x1E);
        bus.port_read(PORT_DATA, 1).unwrap();
        let blob = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&blob).unwrap();
        bus_keyboard(&mut bus).push_scancode(0x9E);
        assert!(!bus.device().irq_pending());
        bus.tick(1);
        assert!(!bus.device().irq_pending());
        bus.tick(KEYBOARD_BYTE_CLOCKS - 1);
        assert!(bus.device().irq_pending());
    }

    fn reference_clock(kbc: &mut I8042, clock: u64) {
        kbc.now = VClock(clock);
        if kbc.keyboard_ready_at.is_some_and(|at| clock >= at.0) {
            kbc.keyboard_ready_at = None;
        }
        if kbc.refill_hold {
            kbc.refill_hold = false;
        } else {
            kbc.refill_output();
        }
    }

    #[test]
    fn serial_direct_catch_up_matches_per_clock_with_pending_controller_reply() {
        for jump in [
            1,
            2,
            100,
            KEYBOARD_BYTE_CLOCKS - 1,
            KEYBOARD_BYTE_CLOCKS,
            KEYBOARD_BYTE_CLOCKS + 1,
            100_003,
        ] {
            for (inhibit, controller_reply) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let mut reference = crate::trace_policy_tests::traced(I8042::new());
                let mut jumped =
                    MockBus::new(Box::new(crate::trace_policy_tests::traced(I8042::new())));
                for kbc in [&mut reference, bus_keyboard(&mut jumped)] {
                    kbc.port_write(PORT_CMD, 1, 0x60).unwrap();
                    kbc.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
                    kbc.push_scancode(0x1E);
                    kbc.push_scancode(0x9E);
                    if controller_reply {
                        kbc.port_write(PORT_CMD, 1, 0xD0).unwrap();
                    }
                    kbc.port_read(PORT_DATA, 1).unwrap();
                    if inhibit {
                        kbc.port_write(PORT_CMD, 1, 0xAD).unwrap();
                    }
                }
                for clock in 1..=jump {
                    reference_clock(&mut reference, clock);
                }
                jumped.tick(jump);
                assert_eq!(reference.save(), jumped.device().save());
                assert_eq!(reference.take_trace(), jumped.device_mut().take_trace());
                assert_eq!(
                    jumped.device().irq_pending(),
                    !inhibit && !controller_reply && jump >= KEYBOARD_BYTE_CLOCKS
                );
                if controller_reply && jump >= 2 {
                    assert_eq!(
                        jumped.port_read(PORT_DATA, 1).unwrap(),
                        u32::from(OUTPUT_PORT_RESET)
                    );
                    reference.port_read(PORT_DATA, 1).unwrap();
                    for clock in jump + 1..=jump + KEYBOARD_BYTE_CLOCKS {
                        reference_clock(&mut reference, clock);
                    }
                    jumped.tick(KEYBOARD_BYTE_CLOCKS);
                    assert_eq!(reference.save(), jumped.device().save());
                    assert_eq!(reference.irq_pending(), jumped.device().irq_pending());
                    assert_eq!(jumped.device().irq_pending(), !inhibit);
                }
            }
        }
    }

    #[test]
    fn scheduled_matches_per_clock_seeded_accesses_and_long_jumps() {
        let mut reference = crate::trace_policy_tests::traced(I8042::new());
        let mut scheduled = MockBus::new(Box::new(crate::trace_policy_tests::traced(I8042::new())));
        let mut seed = 0x8042_1468_1816_5501u64;
        let mut expected_edges = Vec::new();
        let mut actual_edges = Vec::new();
        let mut expected_level = false;
        let mut actual_level = false;
        let mut pin_sample = u64::MAX;
        let mut saw_full_queue = false;
        let mut saw_pending_reply = false;
        let mut saw_held_keyboard = false;
        for step in 0..256 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let jump = match step % 31 {
                0 => 100_003,
                1 => KEYBOARD_BYTE_CLOCKS - 1,
                2 => KEYBOARD_BYTE_CLOCKS,
                3 => KEYBOARD_BYTE_CLOCKS + 1,
                _ => (seed >> 32) % 9,
            };
            let access = scheduled.clock().0 + jump;
            loop {
                let due = match scheduled.device().next_event(scheduled.clock()) {
                    device::NextEvent::At(at) => {
                        assert!(at > scheduled.clock());
                        at.0
                    }
                    device::NextEvent::Dormant => u64::MAX,
                    device::NextEvent::EveryPitClock => panic!("keyboard still polls every clock"),
                };
                let target = access.min(due).min(pin_sample);
                let previous_clock = scheduled.clock().0;
                for clock in scheduled.clock().0 + 1..=target {
                    reference_clock(&mut reference, clock);
                    if reference.irq_pending() != expected_level {
                        expected_level = reference.irq_pending();
                        expected_edges.push((clock, expected_level));
                    }
                }
                scheduled.tick(target - scheduled.clock().0);
                if target > previous_clock && scheduled.device().irq_pending() != actual_level {
                    actual_level = scheduled.device().irq_pending();
                    actual_edges.push((target, actual_level));
                }
                assert_eq!(reference.save(), scheduled.device().save());
                assert_eq!(reference.irq_pending(), scheduled.device().irq_pending());
                let kbc = scheduled
                    .device_mut()
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<I8042>()
                    .unwrap();
                assert_eq!(reference.refill_hold, kbc.refill_hold);
                if target == pin_sample {
                    pin_sample = u64::MAX;
                }
                if target == access {
                    break;
                }
            }
            scheduled.device_mut().sync(VClock(access));
            match step % 10 {
                0 | 1 => {
                    let count = if step % 20 == 0 { 32 } else { 1 };
                    for offset in 0..count {
                        let byte = (seed as u8).wrapping_add(offset);
                        reference.push_scancode(byte);
                        bus_keyboard(&mut scheduled).push_scancode(byte);
                    }
                }
                2 | 3 => {
                    assert_eq!(
                        reference.port_read(PORT_DATA, 1).unwrap(),
                        scheduled.port_read(PORT_DATA, 1).unwrap()
                    );
                    // Same-clock host traffic cannot bypass the low IRQ hold.
                    reference.push_scancode(0x1E);
                    bus_keyboard(&mut scheduled).push_scancode(0x1E);
                    for (port, value) in [
                        (PORT_DATA, 0xFF),
                        (PORT_CMD, 0x60),
                        (PORT_DATA, u32::from(CB_KBD_INT)),
                        (PORT_CMD, 0xAD),
                        (PORT_CMD, 0xAE),
                    ] {
                        reference.port_write(port, 1, value).unwrap();
                        scheduled.port_write(port, 1, value).unwrap();
                    }
                }
                4 => assert_eq!(
                    reference.port_read(PORT_CMD, 1).unwrap(),
                    scheduled.port_read(PORT_CMD, 1).unwrap()
                ),
                5 => {
                    reference.port_write(PORT_CMD, 1, 0x60).unwrap();
                    scheduled.port_write(PORT_CMD, 1, 0x60).unwrap();
                    let value =
                        u32::from(CB_KBD_INT | if seed & 0x100 != 0 { CB_KBD_DISABLE } else { 0 });
                    reference.port_write(PORT_DATA, 1, value).unwrap();
                    scheduled.port_write(PORT_DATA, 1, value).unwrap();
                }
                6 => {
                    let command =
                        [0xAD, 0xAE, 0xAA, 0xAB, 0x20, 0xD0, 0xD1, 0xD4][(seed >> 8) as usize % 8];
                    reference.port_write(PORT_CMD, 1, command).unwrap();
                    scheduled.port_write(PORT_CMD, 1, command).unwrap();
                    // Exercise a busy output buffer, a full reply slot, and
                    // controller progress with the keyboard interface disabled.
                    for command in [0xD0, 0xAA, 0xAD] {
                        reference.port_write(PORT_CMD, 1, command).unwrap();
                        scheduled.port_write(PORT_CMD, 1, command).unwrap();
                    }
                }
                7 | 8 => {
                    let value = if step % 2 == 0 { 0xFF } else { seed as u32 };
                    for _ in 0..20 {
                        reference.port_write(PORT_DATA, 1, value).unwrap();
                        scheduled.port_write(PORT_DATA, 1, value).unwrap();
                    }
                }
                _ => {
                    let snapshot = reference.save();
                    reference.restore(&snapshot).unwrap();
                    scheduled.device_mut().restore(&snapshot).unwrap();
                }
            }
            saw_full_queue |= reference.queued_replies == KEYBOARD_REPLY_CAPACITY;
            saw_pending_reply |= reference.controller_reply.is_some();
            saw_held_keyboard |= reference.refill_hold && !reference.input_queue.is_empty();
            // Access-induced line changes are sampled on the next clock by both engines.
            assert_eq!(reference.save(), scheduled.device().save());
            assert_eq!(reference.irq_pending(), scheduled.device().irq_pending());
            assert_eq!(reference.take_trace(), scheduled.device_mut().take_trace());
            pin_sample = access + 1;
        }
        assert_eq!(expected_edges, actual_edges);
        assert!(expected_edges.iter().any(|(_, level)| *level));
        assert!(saw_full_queue && saw_pending_reply && saw_held_keyboard);
    }

    #[test]
    fn refill_deadlines_preserve_hold_zero_elapsed_and_catch_up() {
        let mut kbc = I8042::new();
        assert_eq!(kbc.next_event(VClock::ZERO), device::NextEvent::Dormant);
        kbc.port_write(PORT_CMD, 1, 0x60).unwrap();
        kbc.port_write(PORT_DATA, 1, u32::from(CB_KBD_INT)).unwrap();
        kbc.push_scancode(0x1E);
        kbc.push_scancode(0x9E);
        kbc.port_read(PORT_DATA, 1).unwrap();
        assert_eq!(
            kbc.next_event(VClock::ZERO),
            device::NextEvent::At(VClock(1))
        );
        kbc.sync(VClock::ZERO);
        assert!(kbc.refill_hold);
        kbc.tick(VClock(1));
        assert!(!kbc.irq_pending());
        assert_eq!(
            kbc.next_event(VClock(1)),
            device::NextEvent::At(VClock(KEYBOARD_BYTE_CLOCKS))
        );
        kbc.sync(VClock(1));
        assert!(!kbc.irq_pending());
        kbc.tick(VClock(KEYBOARD_BYTE_CLOCKS));
        assert!(kbc.irq_pending());
        assert_eq!(
            kbc.next_event(VClock(KEYBOARD_BYTE_CLOCKS)),
            device::NextEvent::Dormant
        );
        kbc.push_scancode(0x30);
        kbc.port_read(PORT_DATA, 1).unwrap();
        kbc.sync(VClock(100_003));
        assert!(kbc.irq_pending());
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x30);
    }

    #[test]
    fn power_on_state() {
        let kbc = I8042::new();
        assert!(!kbc.irq_pending());
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
        // The first byte is available at once; each later byte shifts in serially,
        // on a tick after the prior one is read (the hold tick lets the line fall,
        // the serial frame completes). See the serial-delivery module docs.
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x10);
        kbc.tick(VClock(1)); // hold: line observed low
        kbc.tick(VClock(KEYBOARD_BYTE_CLOCKS)); // delivers 0x11
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), 0x11);
        kbc.tick(VClock(2 * KEYBOARD_BYTE_CLOCKS));
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
        assert!(!kbc.irq_pending());
        kbc.tick(VClock(1));
        assert!(!kbc.irq_pending());
        kbc.tick(VClock(KEYBOARD_BYTE_CLOCKS));
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
        // Queue survives and drains identically: 0x50 is in the buffer, 0x51 shifts
        // in on a tick after 0x50 is read (serial delivery).
        assert_eq!(restored.port_read(PORT_DATA, 1).unwrap(), 0x50);
        restored.tick(VClock(1));
        restored.tick(VClock(KEYBOARD_BYTE_CLOCKS));
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
    fn restore_rejects_oversized_queue_and_old_version_without_mutation() {
        let mut bus = MockBus::new(Box::new(I8042::new()));
        bus.port_write(PORT_CMD, 1, 0xD0).unwrap();
        let original = bus.device().save();
        let mut oversized = original.clone();
        oversized[7..15].copy_from_slice(&17u64.to_le_bytes());
        for _ in 0..17 {
            oversized.extend([KBD_ACK, 1]);
        }
        assert_eq!(
            bus.device_mut().restore(&oversized).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "keyboard replies exceed capacity",
            }
        );
        assert_eq!(bus.device().save(), original);
        let mut old_version = original.clone();
        old_version[0] = 2;
        assert_eq!(
            bus.device_mut().restore(&old_version).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            }
        );
        assert_eq!(bus.device().save(), original);
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
        // ACK is available at once; BAT OK shifts in on a tick after ACK is read.
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_ACK));
        kbc.tick(VClock(1));
        kbc.tick(VClock(KEYBOARD_BYTE_CLOCKS));
        assert_eq!(kbc.port_read(PORT_DATA, 1).unwrap(), u32::from(KBD_BAT_OK));
        kbc.tick(VClock(2 * KEYBOARD_BYTE_CLOCKS));
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
