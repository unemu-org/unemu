// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8259A Programmable Interrupt Controller (§7.3).
//!
//! Models **one 8259A chip**. It aggregates eight input request lines into one
//! INTR to the CPU and answers the CPU's INTA cycle with an 8-bit interrupt
//! vector, `vector_base + irq` (§5.1, §5.4).
//!
//! The IBM PC/AT wires **two** of these chips in a cascade: a master at ports
//! `0x20`/`0x21` carrying IRQ0–IRQ7, and a slave at `0xA0`/`0xA1` carrying
//! IRQ8–IRQ15 whose INT output feeds the master's IRQ2 input. A single
//! [`Pic8259`] plays either role — [`Pic8259::new`] wires it as the master and
//! [`Pic8259::new_slave`] as the slave, the only difference being which port
//! pair it decodes. The two chips are coordinated into a 16-line controller by
//! the `chipset` crate's cascade wrapper; this module models one chip and
//! exposes the priority queries (`highest_pending`, `highest_in_service`) that
//! wrapper needs to route a cascaded acknowledge and EOI to the right chip.
//!
//! The chip carries two faces:
//! - the [`Device`] face — its programming ports, decoded at `0x20`/`0x21`;
//! - the [`InterruptController`] face — the seam to the CPU: input lines in,
//!   INTR + vector out.
//!
//! ## Programming model
//!
//! Writes are interpreted by an initialization state machine. A write to the
//! command port `0x20` with bit 4 (`D4`) set is **ICW1**; it starts the
//! initialization sequence and the following data-port writes are consumed as
//! **ICW2** (vector base), then — because ICW1 requests cascade — **ICW3**, then
//! (because ICW1 requests it) **ICW4**. Once initialization completes, data-port
//! writes program the interrupt mask (**OCW1** / IMR) and command-port writes
//! are operation commands (**OCW2** = EOI/priority, **OCW3** = read-register
//! select). The real-mode PC BIOS programs the master with vector base `0x08`,
//! so IRQ0 is vector `0x08`.
//!
//! ## Priority and state
//!
//! Three 8-bit registers, one bit per line:
//! - **IRR** (interrupt request register): lines that have requested service.
//! - **ISR** (in-service register): lines currently being serviced.
//! - **IMR** (interrupt mask register): masked lines (`1` = masked).
//!
//! Priority is **fixed**: IRQ0 is highest, IRQ7 lowest. A request surfaces only
//! if it is in the IRR, not masked, and no equal-or-higher-priority line is
//! already in service. Acknowledge (INTA) moves the winning IRR bit into the
//! ISR; end-of-interrupt clears it from the ISR.
//!
//! ## Trigger mode
//!
//! Modelled as **edge-triggered**: a request latches into the IRR on the rising
//! edge of an input line ([`InterruptController::set_irq_line`] going low→high).
//! Level-triggered mode (ICW1 `LTIM`) is not modelled; see the coverage matrix.
//!
//! ## Coverage matrix (datasheet feature → status)
//!
//! Modelled:
//! - A single chip decoding either the master ports `0x20`/`0x21` or the slave
//!   ports `0xA0`/`0xA1`, selected at construction.
//! - **Cascade / slave PIC**: a master and a slave combine into the PC/AT's 16
//!   input lines (IRQ0–IRQ15), the slave's INT output feeding master IRQ2. The
//!   two-chip coordination — a cascaded acknowledge fetching the slave's vector,
//!   and an EOI clearing both chips — lives in the `chipset` cascade wrapper,
//!   built on this chip's `highest_pending` / `highest_in_service` queries.
//!   ICW3 (which line the cascade is on) is still accepted and ignored; the
//!   wrapper hardwires the cascade to IRQ2 as every PC/AT does.
//! - ICW1–ICW4 initialization state machine; ICW2 sets the vector base.
//! - 8086/8088 interrupt vectoring; ICW4 `µPM` is stored but not decoded.
//! - IRR / ISR / IMR with fixed priority (line 0 highest … line 7 lowest).
//! - OCW1 (IMR programming).
//! - OCW2 non-specific EOI (`0x20`) and specific EOI (`0x60 | level`).
//! - OCW3 read-register select (read IRR / ISR through the command port).
//! - Edge-triggered request latching.
//!
//! Skipped (documented gaps; not needed for timer/keyboard bring-up):
//! - **Level-triggered mode** (ICW1 `LTIM`): input latching is always edge-based.
//! - **Rotating / automatic-rotation priority** (OCW2 rotate bits): priority is
//!   fixed. Rotate commands are accepted but do not rotate.
//! - **Automatic EOI** (ICW4 `AEOI`): the bit is stored but auto-EOI is not
//!   performed; software must issue an explicit EOI.
//! - **Special mask mode** and **special fully-nested mode** (OCW3 SMM / ICW4
//!   SFNM). In particular the cascade wrapper handles one in-flight cascaded
//!   interrupt at a time: two slave interrupts nested on top of each other (the
//!   case SFNM exists for) are not modelled.
//! - **Poll mode** (OCW3 poll bit).
//! - **MCS-80/85 vectoring** (`µPM=0`, also selected when ICW4 is omitted):
//!   vectoring still uses the 8086/8088 path. Intel 8259A datasheet,
//!   Initialization Command Words (ICW4) and Interrupt Sequence.
//! - **Buffered mode** (ICW4 buffered bits).
//! - **Spurious IRQ7/IRQ15** (Intel 8259A datasheet, Interrupt Sequence): no
//!   default IR7 vector or spurious ISR semantics. Poll and acknowledge are one
//!   atomic machine transaction, with no separate INTR/INTA pin interval in
//!   which a request can disappear. The cascade wrapper withdraws stale IRQ2
//!   requests; modelling the electrical race requires that interval first.
//!
//! Snapshots contain request/service/mask and initialization registers,
//! read selection, input line levels and the absolute trace time base. Keeping
//! line levels preserves edge detection (Intel 8259A datasheet, Edge and Level
//! Triggered Modes). Port wiring is supplied by the receiving instance.

use device::{AccessDir, Device, DeviceError, InterruptController, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "8259a-pic";

/// Master command port (A0 = 0), the default port base: ICW1, OCW2, OCW3. The
/// matching data port (A0 = 1) is one above it: ICW2/ICW3/ICW4 during init,
/// OCW1 (IMR) afterwards.
const PORT_COMMAND: u16 = 0x20;
/// Slave command port base on the PC/AT; its data port is `0xA1`.
const SLAVE_PORT_COMMAND: u16 = 0xA0;

/// ICW1 identifier bit (D4): a command-port write with this set starts init.
const ICW1_INIT: u8 = 0x10;
/// ICW1 `IC4` bit (D0): the sequence includes ICW4.
const ICW1_IC4: u8 = 0x01;
/// ICW1 `SNGL` bit (D1): single PIC — when set, ICW3 is not expected.
const ICW1_SNGL: u8 = 0x02;

/// OCW2 identifier: bits D4,D3 are `0,0` (distinguishes it from ICW1/OCW3).
/// The EOI command bit is D5.
const OCW2_EOI: u8 = 0x20;
/// OCW2 `SL` bit (D6): the EOI is level-specific (specific EOI).
const OCW2_SPECIFIC: u8 = 0x40;
/// OCW2 level-select mask (D2..D0): the IRQ level for a specific command.
const OCW2_LEVEL: u8 = 0x07;

/// OCW3 identifier bit (D3): a command-port write with D4=0,D3=1 is OCW3.
const OCW3_ID: u8 = 0x08;
/// OCW3 read-register enable (D1): D0 then selects IRR (0) or ISR (1).
const OCW3_READ: u8 = 0x02;
/// OCW3 read-register select (D0): `1` = read ISR, `0` = read IRR.
const OCW3_READ_ISR: u8 = 0x01;

/// Snapshot layout version tag, first byte of [`Device::save`] output.
const SNAPSHOT_VERSION: u8 = 2;
/// Snapshot length: version + irr + isr + imr + base + icw1 + icw4 + state +
/// read_isr flag + input line levels (one byte each), then the 8-byte clock of
/// the last synchronisation.
const SNAPSHOT_LEN: usize = 18;

/// Which register a command-port read returns, set by OCW3 (§7.3).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ReadSelect {
    /// Command-port reads return the interrupt request register.
    Irr,
    /// Command-port reads return the in-service register.
    Isr,
}

/// Position in the ICW1→ICW4 initialization sequence (§7.3).
///
/// A command-port write with the ICW1 bit set restarts the sequence at
/// [`InitState::ExpectIcw2`]; each subsequent data-port write advances it until
/// initialization is [`InitState::Ready`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum InitState {
    /// Initialization complete; data-port writes program the IMR (OCW1).
    Ready,
    /// The next data-port write is ICW2 (vector base).
    ExpectIcw2,
    /// The next data-port write is ICW3 (cascade; accepted and ignored).
    ExpectIcw3,
    /// The next data-port write is ICW4 (mode bits).
    ExpectIcw4,
}

/// The Intel 8259A master PIC (§7.3).
///
/// Constructed with [`Pic8259::new`]; decodes ports `0x20` and `0x21`. Devices
/// drive input lines through the [`InterruptController`] face and the CPU
/// integration polls / acknowledges / EOIs through it; the [`Device`] face is
/// the guest's programming interface.
pub struct Pic8259 {
    /// Interrupt request register: latched, pending requests (bit per line).
    irr: u8,
    /// In-service register: lines currently being serviced (bit per line).
    isr: u8,
    /// Interrupt mask register (OCW1): `1` = the line is masked.
    imr: u8,
    /// Vector base from ICW2; the vector for IRQ`n` is `vector_base + n`.
    vector_base: u8,
    /// The ICW1 byte of the current configuration, retained for its mode bits.
    icw1: u8,
    /// The ICW4 byte of the current configuration, retained for its mode bits.
    icw4: u8,
    /// Position in the initialization sequence.
    init_state: InitState,
    /// Which register a command-port read returns (OCW3).
    read_select: ReadSelect,
    /// Last level of each input line, to detect rising edges for edge-triggered
    /// latching. Bit per line; `1` = the line is currently high.
    line_level: u8,
    /// Command-port address this chip decodes; its data port is `port_base + 1`.
    /// `0x20` for the master, `0xA0` for the slave.
    port_base: u16,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl Default for Pic8259 {
    fn default() -> Self {
        Self::new()
    }
}

impl Pic8259 {
    /// Create a master 8259A (ports `0x20`/`0x21`) in its power-on state.
    ///
    /// All registers are clear and no initialization has been performed; the
    /// guest must run the ICW1–ICW4 sequence before any vector is meaningful.
    #[must_use]
    pub fn new() -> Self {
        Self::with_port_base(PORT_COMMAND)
    }

    /// Create a slave 8259A (ports `0xA0`/`0xA1`) in its power-on state.
    ///
    /// Identical to [`Pic8259::new`] except for the port pair it decodes. On the
    /// PC/AT the slave's INT output feeds the master's IRQ2 input; that wiring is
    /// the cascade wrapper's job, not the chip's — this chip only knows it
    /// answers at the slave ports and programs its own vector base (`0x70` under
    /// the PC BIOS, so IRQ8 is vector `0x70`).
    #[must_use]
    pub fn new_slave() -> Self {
        Self::with_port_base(SLAVE_PORT_COMMAND)
    }

    /// Create an 8259A in its power-on state decoding `port_base` / `port_base+1`.
    #[must_use]
    fn with_port_base(port_base: u16) -> Self {
        Self {
            irr: 0,
            isr: 0,
            imr: 0,
            vector_base: 0,
            icw1: 0,
            icw4: 0,
            init_state: InitState::Ready,
            read_select: ReadSelect::Irr,
            line_level: 0,
            port_base,
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// The current interrupt request register (pending, latched requests).
    #[must_use]
    pub fn irr(&self) -> u8 {
        self.irr
    }

    /// The current in-service register (lines being serviced).
    #[must_use]
    pub fn isr(&self) -> u8 {
        self.isr
    }

    /// The current interrupt mask register (`1` = masked).
    #[must_use]
    pub fn imr(&self) -> u8 {
        self.imr
    }

    /// The programmed vector base (ICW2); the vector for IRQ`n` is `base + n`.
    #[must_use]
    pub fn vector_base(&self) -> u8 {
        self.vector_base
    }

    /// Withdraw a latched request without changing its input level or ISR.
    ///
    /// Intel 8259A datasheet, "Edge and Level Triggered Modes": an IR input
    /// must remain high until after the falling edge of the first INTA. The
    /// cascade wrapper uses this when slave INT falls before INTA; device
    /// pulses retain their edge-latched requests unless explicitly withdrawn.
    pub fn withdraw_request(&mut self, line: u8) {
        if line < 8 {
            self.irr &= !(1u8 << line);
        }
    }

    /// The highest-priority line that may be serviced right now, or `None`.
    ///
    /// A line qualifies when it is requested (IRR), not masked (IMR), and no
    /// line of equal-or-higher priority is already in service (ISR). Priority is
    /// fixed with line 0 highest, so the search runs low bit to high bit.
    ///
    /// The cascade wrapper reads this to decide whether the master's winning
    /// line is the cascade input (line 2), and to pick the slave line whose
    /// vector a cascaded acknowledge should return.
    #[must_use]
    pub fn highest_pending(&self) -> Option<u8> {
        let requestable = self.irr & !self.imr;
        for irq in 0u8..8 {
            let bit = 1u8 << irq;
            // A line of equal-or-higher priority already in service blocks this
            // one and every lower line below it.
            if self.isr & (bit.wrapping_sub(1) | bit) != 0 {
                return None;
            }
            if requestable & bit != 0 {
                return Some(irq);
            }
        }
        None
    }

    /// Program the initialization command words / IMR from a data-port write.
    fn write_data(&mut self, byte: u8) {
        match self.init_state {
            InitState::ExpectIcw2 => {
                // ICW2 carries the vector base in its high five bits; the low
                // three are supplied by the acknowledged level, so mask them.
                self.vector_base = byte & 0xF8;
                self.init_state = if self.icw1 & ICW1_SNGL != 0 {
                    // Single PIC: no ICW3. Skip straight to ICW4 if requested.
                    if self.icw1 & ICW1_IC4 != 0 {
                        InitState::ExpectIcw4
                    } else {
                        InitState::Ready
                    }
                } else {
                    InitState::ExpectIcw3
                };
            }
            InitState::ExpectIcw3 => {
                // ICW3 configures cascade wiring, which a single master PIC does
                // not use: accept and ignore.
                self.init_state = if self.icw1 & ICW1_IC4 != 0 {
                    InitState::ExpectIcw4
                } else {
                    InitState::Ready
                };
            }
            InitState::ExpectIcw4 => {
                self.icw4 = byte;
                self.init_state = InitState::Ready;
            }
            InitState::Ready => {
                // OCW1: program the interrupt mask.
                self.imr = byte;
            }
        }
    }

    /// Interpret a command-port write: ICW1 (start init), OCW2 (EOI), or OCW3.
    fn write_command(&mut self, byte: u8) {
        if byte & ICW1_INIT != 0 {
            // ICW1: restart initialization. IMR and priority state are cleared.
            self.icw1 = byte;
            // Intel 8259A datasheet, Initialization Command Words: omitting
            // ICW4 selects zero for every ICW4 function, even after re-init.
            if byte & ICW1_IC4 == 0 {
                self.icw4 = 0;
            }
            self.imr = 0;
            self.isr = 0;
            self.irr = 0;
            self.read_select = ReadSelect::Irr;
            self.init_state = InitState::ExpectIcw2;
            return;
        }
        if byte & OCW3_ID != 0 {
            // OCW3: only the read-register select is modelled.
            if byte & OCW3_READ != 0 {
                self.read_select = if byte & OCW3_READ_ISR != 0 {
                    ReadSelect::Isr
                } else {
                    ReadSelect::Irr
                };
            }
            return;
        }
        // OCW2: end-of-interrupt commands (rotate bits accepted, not modelled).
        if byte & OCW2_EOI != 0 {
            if byte & OCW2_SPECIFIC != 0 {
                let level = byte & OCW2_LEVEL;
                self.isr &= !(1u8 << level);
            } else {
                self.clear_highest_in_service();
            }
        }
    }

    /// The highest-priority line currently in service (top of the ISR), or
    /// `None` if nothing is in service.
    ///
    /// A non-specific EOI clears exactly this line. The cascade wrapper reads it
    /// to tell whether the interrupt being ended came in through the cascade
    /// (master line 2 in service), so it can also EOI the slave.
    #[must_use]
    pub fn highest_in_service(&self) -> Option<u8> {
        (0u8..8).find(|&irq| self.isr & (1u8 << irq) != 0)
    }

    /// Clear the highest-priority in-service bit (non-specific EOI).
    fn clear_highest_in_service(&mut self) {
        if let Some(irq) = self.highest_in_service() {
            self.isr &= !(1u8 << irq);
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

impl InterruptController for Pic8259 {
    fn set_irq_line(&mut self, irq: u8, level: bool) {
        if irq >= 8 {
            return;
        }
        let bit = 1u8 << irq;
        let was_high = self.line_level & bit != 0;
        if level {
            self.line_level |= bit;
            // Edge-triggered: latch a request only on a rising edge.
            if !was_high {
                self.irr |= bit;
            }
        } else {
            self.line_level &= !bit;
        }
    }

    fn poll(&self) -> Option<u8> {
        self.highest_pending().map(|irq| self.vector_base + irq)
    }

    fn acknowledge(&mut self) -> Option<u8> {
        let irq = self.highest_pending()?;
        let bit = 1u8 << irq;
        // INTA: the request moves from pending to in-service.
        self.irr &= !bit;
        self.isr |= bit;
        Some(self.vector_base + irq)
    }

    fn end_of_interrupt(&mut self) {
        self.clear_highest_in_service();
    }
}

impl Device for Pic8259 {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port.checked_sub(self.port_base) {
            // Command port: OCW3 selects which register reads back.
            Some(0) => match self.read_select {
                ReadSelect::Irr => u32::from(self.irr),
                ReadSelect::Isr => u32::from(self.isr),
            },
            // Data port: the interrupt mask.
            Some(1) => u32::from(self.imr),
            other => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(other.unwrap_or(port)),
                });
            }
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let byte = value as u8;
        match port.checked_sub(self.port_base) {
            Some(0) => self.write_command(byte),
            Some(1) => self.write_data(byte),
            other => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(other.unwrap_or(port)),
                });
            }
        }
        self.trace_port(port, size, AccessDir::Write, value);
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
        // The 8259A is port-mapped on the PC/AT, never memory-mapped.
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
        // The PIC is edge/level driven, not clocked; only stamp trace time.
        self.now = now;
    }

    fn irq_pending(&self) -> bool {
        // The aggregated INTR output: a serviceable request is pending.
        self.highest_pending().is_some()
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        // Keep the wired role (master vs slave port pair); reset only chip state.
        *self = Pic8259::with_port_base(self.port_base);
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        let read_isr = u8::from(self.read_select == ReadSelect::Isr);
        let init_state = match self.init_state {
            InitState::Ready => 0,
            InitState::ExpectIcw2 => 1,
            InitState::ExpectIcw3 => 2,
            InitState::ExpectIcw4 => 3,
        };
        let mut blob = vec![
            SNAPSHOT_VERSION,
            self.irr,
            self.isr,
            self.imr,
            self.vector_base,
            self.icw1,
            self.icw4,
            init_state,
            read_isr,
            self.line_level,
        ];
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() != SNAPSHOT_LEN {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot length mismatch",
            });
        }
        if snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        let init_state = match snapshot[7] {
            0 => InitState::Ready,
            1 => InitState::ExpectIcw2,
            2 => InitState::ExpectIcw3,
            3 => InitState::ExpectIcw4,
            _ => {
                return Err(DeviceError::InvalidSnapshot {
                    device: NAME,
                    reason: "invalid init state",
                });
            }
        };
        if (snapshot[5] & ICW1_IC4 == 0 && snapshot[6] != 0)
            || snapshot[4] & 7 != 0
            || snapshot[8] > 1
            || (snapshot[7] != 0 && snapshot[5] & ICW1_INIT == 0)
            || (snapshot[7] == 2 && snapshot[5] & ICW1_SNGL != 0)
            || (snapshot[7] == 3 && snapshot[5] & ICW1_IC4 == 0)
        {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid interrupt state",
            });
        }
        self.irr = snapshot[1];
        self.isr = snapshot[2];
        self.imr = snapshot[3];
        self.vector_base = snapshot[4];
        self.icw1 = snapshot[5];
        self.icw4 = snapshot[6];
        self.init_state = init_state;
        self.read_select = if snapshot[8] != 0 {
            ReadSelect::Isr
        } else {
            ReadSelect::Irr
        };
        self.line_level = snapshot[9];
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| {
            snapshot[snapshot.len() - 8 + i]
        })));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    #[test]
    fn restore_rejects_icw4_without_ic4_atomically() {
        let mut bus = MockBus::new(Box::new(Pic8259::new()));
        let before = bus.device().save();
        let mut invalid = before.clone();
        invalid[6] = 1;
        assert!(bus.device_mut().restore(&invalid).is_err());
        assert_eq!(bus.device().save(), before);
    }

    #[test]
    fn fidelity_icw1_without_ic4_clears_old_icw4() {
        let mut bus = MockBus::new(Box::new(Pic8259::new()));
        for (port, value) in [(0x20, 0x13), (0x21, 0x20), (0x21, 0x1F), (0x20, 0x12)] {
            bus.port_write(port, 1, value).unwrap();
        }
        assert_eq!(bus.device().save()[6], 0);
    }

    /// Master data port (A0 = 1): ICW2/ICW3/ICW4 during init, OCW1 afterwards.
    const PORT_DATA: u16 = PORT_COMMAND + 1;
    /// Real-mode PC vector base for the master PIC: IRQ0 → vector 0x08.
    const BASE: u8 = 0x08;
    /// Real-mode PC vector base for the slave PIC: IRQ8 → vector 0x70.
    const SLAVE_BASE: u8 = 0x70;

    /// ICW4 `µPM` bit (D0): 8086/8088 mode (vs MCS-80/85). PCs always set it.
    const ICW4_8086: u8 = 0x01;

    /// Run the standard ICW1–ICW4 sequence a PC BIOS issues: single-ish master,
    /// edge-triggered, cascade requested (ICW3 ignored), 8086 mode.
    fn init_master(pic: &mut Pic8259) {
        // ICW1: init + ICW4 needed (cascade, edge-triggered).
        pic.port_write(PORT_COMMAND, 1, u32::from(ICW1_INIT | ICW1_IC4))
            .unwrap();
        // ICW2: vector base 0x08.
        pic.port_write(PORT_DATA, 1, u32::from(BASE)).unwrap();
        // ICW3: cascade wiring (ignored).
        pic.port_write(PORT_DATA, 1, 0x04).unwrap();
        // ICW4: 8086 mode.
        pic.port_write(PORT_DATA, 1, u32::from(ICW4_8086)).unwrap();
    }

    #[test]
    fn init_sequence_sets_vector_base() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        assert_eq!(pic.vector_base(), BASE);
        assert_eq!(pic.init_state, InitState::Ready);
        // 8086 mode recorded.
        assert_eq!(pic.icw4 & ICW4_8086, ICW4_8086);
    }

    #[test]
    fn icw2_masks_low_three_bits() {
        let mut pic = Pic8259::new();
        pic.port_write(PORT_COMMAND, 1, u32::from(ICW1_INIT | ICW1_IC4))
            .unwrap();
        // A base with dirty low bits is truncated to the 8-vector boundary.
        pic.port_write(PORT_DATA, 1, 0x0B).unwrap();
        pic.port_write(PORT_DATA, 1, 0x04).unwrap();
        pic.port_write(PORT_DATA, 1, u32::from(ICW4_8086)).unwrap();
        assert_eq!(pic.vector_base(), 0x08);
    }

    #[test]
    fn single_pic_skips_icw3() {
        let mut pic = Pic8259::new();
        // ICW1 with SNGL set and ICW4 requested: ICW3 is not expected.
        pic.port_write(PORT_COMMAND, 1, u32::from(ICW1_INIT | ICW1_IC4 | ICW1_SNGL))
            .unwrap();
        assert_eq!(pic.init_state, InitState::ExpectIcw2);
        pic.port_write(PORT_DATA, 1, u32::from(BASE)).unwrap();
        assert_eq!(pic.init_state, InitState::ExpectIcw4);
        pic.port_write(PORT_DATA, 1, u32::from(ICW4_8086)).unwrap();
        assert_eq!(pic.init_state, InitState::Ready);
        assert_eq!(pic.vector_base(), BASE);
    }

    #[test]
    fn raising_line_sets_irr() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(0, true);
        assert_eq!(pic.irr(), 0x01);
        pic.set_irq_line(3, true);
        assert_eq!(pic.irr(), 0x09);
    }

    #[test]
    fn edge_triggered_latches_only_on_rising_edge() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(1, true);
        assert_eq!(pic.irr(), 0x02);
        // Acknowledge clears IRR; a still-high line must not re-latch.
        assert_eq!(pic.acknowledge(), Some(BASE + 1));
        assert_eq!(pic.irr(), 0x00);
        pic.set_irq_line(1, true); // still high, no new edge
        assert_eq!(pic.irr(), 0x00);
        // Drop and re-raise: a fresh edge latches again.
        pic.set_irq_line(1, false);
        pic.set_irq_line(1, true);
        assert_eq!(pic.irr(), 0x02);
    }

    #[test]
    fn withdrawing_request_preserves_other_requests_service_and_edge_detection() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(2, true);
        assert_eq!(pic.acknowledge(), Some(BASE + 2));
        pic.set_irq_line(2, false);
        pic.set_irq_line(2, true);
        pic.set_irq_line(6, true);
        assert_eq!(pic.irr(), 0x44);

        pic.withdraw_request(2);
        assert_eq!(pic.irr(), 0x40);
        assert_eq!(pic.isr(), 0x04);
        pic.set_irq_line(2, true);
        assert_eq!(pic.irr(), 0x40);
        pic.set_irq_line(2, false);
        pic.set_irq_line(2, true);
        assert_eq!(pic.irr(), 0x44);
        pic.withdraw_request(8);
        assert_eq!(pic.irr(), 0x44);
    }

    #[test]
    fn acknowledge_moves_irr_to_isr_and_returns_vector() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(4, true);
        assert_eq!(pic.poll(), Some(BASE + 4));
        assert_eq!(pic.acknowledge(), Some(BASE + 4));
        assert_eq!(pic.irr(), 0x00);
        assert_eq!(pic.isr(), 1 << 4);
    }

    #[test]
    fn eoi_clears_in_service() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(2, true);
        pic.acknowledge().unwrap();
        assert_eq!(pic.isr(), 1 << 2);
        // Non-specific EOI via OCW2.
        pic.port_write(PORT_COMMAND, 1, u32::from(OCW2_EOI))
            .unwrap();
        assert_eq!(pic.isr(), 0x00);
    }

    #[test]
    fn specific_eoi_clears_named_level() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(5, true);
        pic.acknowledge().unwrap();
        // Specific EOI for level 5.
        pic.port_write(PORT_COMMAND, 1, u32::from(OCW2_EOI | OCW2_SPECIFIC | 5))
            .unwrap();
        assert_eq!(pic.isr(), 0x00);
    }

    #[test]
    fn masked_line_does_not_surface() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        // OCW1: mask IRQ0.
        pic.port_write(PORT_DATA, 1, 0x01).unwrap();
        pic.set_irq_line(0, true);
        // The request is latched but not serviceable while masked.
        assert_eq!(pic.irr(), 0x01);
        assert_eq!(pic.poll(), None);
        assert!(!pic.irq_pending());
        assert_eq!(pic.acknowledge(), None);
        // Unmasking surfaces it.
        pic.port_write(PORT_DATA, 1, 0x00).unwrap();
        assert_eq!(pic.poll(), Some(BASE));
    }

    #[test]
    fn higher_priority_preempts_pending_lower() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        // Raise a lower-priority line, then a higher one; both pending.
        pic.set_irq_line(6, true);
        pic.set_irq_line(1, true);
        // Priority resolution picks the higher-priority (lower-numbered) line.
        assert_eq!(pic.poll(), Some(BASE + 1));
        assert_eq!(pic.acknowledge(), Some(BASE + 1));
        // While IRQ1 is in service, the still-pending IRQ6 is blocked.
        assert_eq!(pic.poll(), None);
        // After EOI on IRQ1, the lower line becomes serviceable.
        pic.port_write(PORT_COMMAND, 1, u32::from(OCW2_EOI))
            .unwrap();
        assert_eq!(pic.poll(), Some(BASE + 6));
    }

    #[test]
    fn higher_line_preempts_only_if_not_in_service() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        // IRQ3 goes into service.
        pic.set_irq_line(3, true);
        pic.acknowledge().unwrap();
        // A now-pending higher-priority IRQ1 outranks the in-service IRQ3 and
        // may be nested on top of it.
        pic.set_irq_line(1, true);
        assert_eq!(pic.poll(), Some(BASE + 1));
        // A pending lower-priority IRQ5 stays blocked by the in-service IRQ3.
        pic.set_irq_line(5, true);
        assert_eq!(pic.poll(), Some(BASE + 1));
    }

    #[test]
    fn ocw3_selects_irr_or_isr_readback() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(0, true);
        pic.acknowledge().unwrap(); // IRQ0 in service now
        pic.set_irq_line(1, true); // IRQ1 pending in IRR
                                   // OCW3 read ISR.
        pic.port_write(
            PORT_COMMAND,
            1,
            u32::from(OCW3_ID | OCW3_READ | OCW3_READ_ISR),
        )
        .unwrap();
        assert_eq!(
            pic.port_read(PORT_COMMAND, 1).unwrap(),
            u32::from(pic.isr())
        );
        // OCW3 read IRR.
        pic.port_write(PORT_COMMAND, 1, u32::from(OCW3_ID | OCW3_READ))
            .unwrap();
        assert_eq!(
            pic.port_read(PORT_COMMAND, 1).unwrap(),
            u32::from(pic.irr())
        );
    }

    #[test]
    fn imr_reads_back_on_data_port() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.port_write(PORT_DATA, 1, 0xA5).unwrap();
        assert_eq!(pic.port_read(PORT_DATA, 1).unwrap(), 0xA5);
        assert_eq!(pic.imr(), 0xA5);
    }

    #[test]
    fn reinit_clears_mask_and_service_state() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.port_write(PORT_DATA, 1, 0xFF).unwrap(); // mask everything
        pic.set_irq_line(0, true);
        // A second ICW1 restarts init and clears IMR/IRR/ISR.
        pic.port_write(PORT_COMMAND, 1, u32::from(ICW1_INIT | ICW1_IC4))
            .unwrap();
        assert_eq!(pic.imr(), 0x00);
        assert_eq!(pic.irr(), 0x00);
        assert_eq!(pic.isr(), 0x00);
        assert_eq!(pic.init_state, InitState::ExpectIcw2);
    }

    #[test]
    fn unmapped_port_is_rejected() {
        let mut pic = Pic8259::new();
        let err = pic.port_read(0x22, 1).unwrap_err();
        assert_eq!(
            err,
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 2,
            }
        );
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut pic = Pic8259::new();
        assert_eq!(
            pic.mmio_read(0x1000, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x1000,
            }
        );
        assert_eq!(
            pic.mmio_write(0x1000, 1, 0).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x1000,
            }
        );
    }

    #[test]
    fn save_restore_round_trip_preserves_state() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.port_write(PORT_DATA, 1, 0x3C).unwrap(); // IMR
        pic.set_irq_line(0, true);
        pic.acknowledge().unwrap(); // IRQ0 in service
        pic.set_irq_line(4, true); // IRQ4 pending

        let blob = pic.save();
        let mut restored = Pic8259::new();
        restored.restore(&blob).unwrap();

        assert_eq!(restored.vector_base(), BASE);
        assert_eq!(restored.imr(), 0x3C);
        assert_eq!(restored.irr(), 1 << 4);
        assert_eq!(restored.isr(), 1 << 0);
    }

    #[test]
    fn restore_rejects_bad_length() {
        let mut pic = Pic8259::new();
        let err = pic.restore(&[SNAPSHOT_VERSION, 0, 0]).unwrap_err();
        assert_eq!(
            err,
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot length mismatch",
            }
        );
    }

    #[test]
    fn reset_returns_to_power_on() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        pic.set_irq_line(0, true);
        pic.acknowledge().unwrap();
        pic.reset();
        assert_eq!(pic.vector_base(), 0);
        assert_eq!(pic.irr(), 0);
        assert_eq!(pic.isr(), 0);
        assert_eq!(pic.init_state, InitState::Ready);
    }

    #[test]
    fn mock_bus_isolation_harness() {
        // Prove the device runs under the isolation harness and its programming
        // ports resolve through `&mut dyn Device`.
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(Pic8259::new())));
        bus.tick(7);
        bus.port_write(PORT_COMMAND, 1, u32::from(ICW1_INIT | ICW1_IC4))
            .unwrap();
        bus.port_write(PORT_DATA, 1, u32::from(BASE)).unwrap();
        bus.port_write(PORT_DATA, 1, 0x04).unwrap();
        bus.port_write(PORT_DATA, 1, u32::from(ICW4_8086)).unwrap();
        // Mask reads back through the data port.
        bus.port_write(PORT_DATA, 1, 0x11).unwrap();
        assert_eq!(bus.port_read(PORT_DATA, 1).unwrap(), 0x11);

        // Every serviced access is on the tracing seam, stamped with tick time.
        let traced = bus
            .device_mut()
            .take_trace()
            .into_iter()
            .any(|ev| matches!(ev, TraceEvent::Port { at, .. } if at == VClock(7)));
        assert!(traced);
    }

    /// Run the ICW1–ICW4 sequence on a slave through its own ports.
    fn init_slave(pic: &mut Pic8259) {
        pic.port_write(SLAVE_PORT_COMMAND, 1, u32::from(ICW1_INIT | ICW1_IC4))
            .unwrap();
        // ICW2: slave vector base 0x70.
        pic.port_write(SLAVE_PORT_COMMAND + 1, 1, u32::from(SLAVE_BASE))
            .unwrap();
        // ICW3: slave identity (which master line it cascades on) — ignored.
        pic.port_write(SLAVE_PORT_COMMAND + 1, 1, 0x02).unwrap();
        // ICW4: 8086 mode.
        pic.port_write(SLAVE_PORT_COMMAND + 1, 1, u32::from(ICW4_8086))
            .unwrap();
    }

    #[test]
    fn slave_decodes_its_own_port_pair() {
        let mut pic = Pic8259::new_slave();
        init_slave(&mut pic);
        assert_eq!(pic.vector_base(), SLAVE_BASE);
        // IMR reads back through the slave data port, not the master's.
        pic.port_write(SLAVE_PORT_COMMAND + 1, 1, 0x5A).unwrap();
        assert_eq!(pic.port_read(SLAVE_PORT_COMMAND + 1, 1).unwrap(), 0x5A);
        // The master ports are unmapped on a slave chip.
        assert!(pic.port_read(PORT_COMMAND, 1).is_err());
    }

    #[test]
    fn slave_latches_and_resolves_its_lines() {
        let mut pic = Pic8259::new_slave();
        init_slave(&mut pic);
        // Line 0 of the slave is IRQ8; its vector is the slave base.
        pic.set_irq_line(0, true);
        assert_eq!(pic.irr(), 0x01);
        // A higher-priority slave line preempts a pending lower one.
        pic.set_irq_line(2, true);
        assert_eq!(pic.poll(), Some(SLAVE_BASE));
        assert_eq!(pic.acknowledge(), Some(SLAVE_BASE));
        assert_eq!(pic.highest_in_service(), Some(0));
    }

    #[test]
    fn highest_in_service_tracks_isr_top() {
        let mut pic = Pic8259::new();
        init_master(&mut pic);
        assert_eq!(pic.highest_in_service(), None);
        pic.set_irq_line(4, true);
        pic.acknowledge().unwrap();
        assert_eq!(pic.highest_in_service(), Some(4));
        // A nested higher-priority interrupt becomes the new top of the ISR.
        pic.set_irq_line(1, true);
        pic.acknowledge().unwrap();
        assert_eq!(pic.highest_in_service(), Some(1));
    }

    #[test]
    fn reset_keeps_slave_port_wiring() {
        let mut pic = Pic8259::new_slave();
        init_slave(&mut pic);
        pic.reset();
        // Back to power-on state, but still a slave: master ports stay unmapped.
        assert_eq!(pic.vector_base(), 0);
        assert!(pic.port_read(PORT_COMMAND, 1).is_err());
        assert!(pic.port_read(SLAVE_PORT_COMMAND, 1).is_ok());
    }
}
