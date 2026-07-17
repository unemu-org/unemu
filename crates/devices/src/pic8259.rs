// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8259A Programmable Interrupt Controller (§7.3).
//!
//! Models a **single master 8259A** at ports `0x20` (command) and `0x21`
//! (data), the IBM PC/AT's primary interrupt controller. It aggregates eight
//! input request lines (IRQ0–IRQ7) into one INTR to the CPU and answers the
//! CPU's INTA cycle with an 8-bit interrupt vector, `vector_base + irq` (§5.1,
//! §5.4).
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
//! - Single master PIC, ports `0x20`/`0x21`.
//! - ICW1–ICW4 initialization state machine; ICW2 sets the vector base.
//! - 8086/8088 mode (ICW4 `µPM` bit); the only interrupt-vector mode a PC uses.
//! - IRR / ISR / IMR with fixed priority (IRQ0 highest … IRQ7 lowest).
//! - OCW1 (IMR programming).
//! - OCW2 non-specific EOI (`0x20`) and specific EOI (`0x60 | level`).
//! - OCW3 read-register select (read IRR / ISR through the command port).
//! - Edge-triggered request latching.
//!
//! Skipped (documented gaps; not needed for timer/keyboard bring-up):
//! - **Cascade / slave PIC** (`0xA0`/`0xA1`, IRQ2 cascade). Only the master's
//!   IRQ0–IRQ7 are modelled. ICW1's cascade request and ICW3 are accepted and
//!   ignored.
//! - **Level-triggered mode** (ICW1 `LTIM`): input latching is always edge-based.
//! - **Rotating / automatic-rotation priority** (OCW2 rotate bits): priority is
//!   fixed. Rotate commands are accepted but do not rotate.
//! - **Automatic EOI** (ICW4 `AEOI`): the bit is stored but auto-EOI is not
//!   performed; software must issue an explicit EOI.
//! - **Special mask mode** and **special fully-nested mode** (OCW3 SMM / ICW4
//!   SFNM).
//! - **Poll mode** (OCW3 poll bit).
//! - **Buffered mode** (ICW4 buffered bits).

use device::{AccessDir, Device, DeviceError, InterruptController, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "8259a-pic";

/// Command port (A0 = 0): ICW1, OCW2, OCW3.
const PORT_COMMAND: u16 = 0x20;
/// Data port (A0 = 1): ICW2/ICW3/ICW4 during init, OCW1 (IMR) afterwards.
const PORT_DATA: u16 = 0x21;

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
const SNAPSHOT_VERSION: u8 = 1;
/// Snapshot length: version + irr + isr + imr + base + icw1 + icw4 + state +
/// read_isr flag.
const SNAPSHOT_LEN: usize = 9;

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
    /// Trace events observed since the last [`Device::take_trace`].
    trace: Vec<TraceEvent>,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl Default for Pic8259 {
    fn default() -> Self {
        Self::new()
    }
}

impl Pic8259 {
    /// Create a master 8259A in its power-on state.
    ///
    /// All registers are clear and no initialization has been performed; the
    /// guest must run the ICW1–ICW4 sequence before any vector is meaningful.
    #[must_use]
    pub fn new() -> Self {
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
            trace: Vec::new(),
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

    /// The highest-priority line that may be serviced right now, or `None`.
    ///
    /// A line qualifies when it is requested (IRR), not masked (IMR), and no
    /// line of equal-or-higher priority is already in service (ISR). Priority is
    /// fixed with IRQ0 highest, so the search runs low bit to high bit.
    fn highest_pending(&self) -> Option<u8> {
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

    /// Clear the highest-priority in-service bit (non-specific EOI).
    fn clear_highest_in_service(&mut self) {
        for irq in 0u8..8 {
            let bit = 1u8 << irq;
            if self.isr & bit != 0 {
                self.isr &= !bit;
                return;
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
        let value = match port {
            PORT_COMMAND => match self.read_select {
                ReadSelect::Irr => u32::from(self.irr),
                ReadSelect::Isr => u32::from(self.isr),
            },
            PORT_DATA => u32::from(self.imr),
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port.wrapping_sub(PORT_COMMAND)),
                });
            }
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let byte = value as u8;
        match port {
            PORT_COMMAND => self.write_command(byte),
            PORT_DATA => self.write_data(byte),
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port.wrapping_sub(PORT_COMMAND)),
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
        // The PIC is edge/level driven, not clocked; only stamp trace time.
        self.now = now;
    }

    fn irq_pending(&self) -> bool {
        // The aggregated INTR output: a serviceable request is pending.
        self.highest_pending().is_some()
    }

    fn irq_vector(&self) -> Option<u8> {
        self.poll()
    }

    fn reset(&mut self) {
        *self = Pic8259::new();
    }

    fn save(&self) -> Vec<u8> {
        let read_isr = u8::from(self.read_select == ReadSelect::Isr);
        let init_state = match self.init_state {
            InitState::Ready => 0,
            InitState::ExpectIcw2 => 1,
            InitState::ExpectIcw3 => 2,
            InitState::ExpectIcw4 => 3,
        };
        vec![
            SNAPSHOT_VERSION,
            self.irr,
            self.isr,
            self.imr,
            self.vector_base,
            self.icw1,
            self.icw4,
            init_state,
            read_isr,
        ]
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
        // The input-line levels are not snapshotted; on restore no line is held.
        self.line_level = 0;
        Ok(())
    }

    fn take_trace(&mut self) -> Vec<TraceEvent> {
        std::mem::take(&mut self.trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    /// Real-mode PC vector base for the master PIC: IRQ0 → vector 0x08.
    const BASE: u8 = 0x08;

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
        let mut bus = MockBus::new(Box::new(Pic8259::new()));
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
}
