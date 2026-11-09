// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8253/8254 Programmable Interval Timer (§7.4).
//!
//! Models the PIT's four port-mapped registers at `0x40..=0x43`: `0x40`/`0x41`/
//! `0x42` are the three counters' data ports and `0x43` is the mode/command
//! register. Each counter is an independent 16-bit down-counter clocked by the
//! PIT input clock. On the PC/AT that input is the ~1.193182 MHz OSC/12 signal,
//! but this device models **counts, not real time**: [`Device::tick`] catches up
//! arithmetically to the supplied PIT input clock. Only channel 0 OUT transitions
//! and its pending load require clock visits; accesses synchronize all counters
//! through [`Device::sync`] before reading, latching or programming (§6). This keeps
//! the device deterministic and testable without a wall clock.
//!
//! Counter 0's OUT pin is wired to IRQ0 on the real machine; here it is surfaced
//! through [`Device::irq_pending`]. Counters 1 (DRAM refresh) and 2 (PC speaker)
//! are modelled as counters but their OUT pins are not routed anywhere yet.
//!
//! ## Clocking model
//!
//! A real 8254 does not decrement the count on the input clock directly; the
//! newly written count value is loaded into the counting element on the clock
//! edge *following* the write, and gating/OUT transitions happen relative to
//! that. This model reproduces that one-clock load delay so the OUT period seen
//! at counter 0 equals the programmed divisor exactly (the core datasheet
//! invariant). See the coverage matrix below.
//!
//! ## Coverage matrix (datasheet vs modelled)
//!
//! Access modes (command bits 5-4):
//!
//! | Value | Meaning                    | Modelled |
//! |-------|----------------------------|----------|
//! | `00`  | Counter-latch command      | yes      |
//! | `01`  | Read/load LSB only         | yes      |
//! | `10`  | Read/load MSB only         | yes      |
//! | `11`  | Read/load LSB then MSB     | yes      |
//!
//! Operating modes (command bits 3-1):
//!
//! | Mode | Name                          | Modelled | Notes                     |
//! |------|-------------------------------|----------|---------------------------|
//! | 0    | Interrupt on terminal count   | yes      | OUT low until TC, then high |
//! | 1    | Hardware retriggerable one-shot | no     | needs a real GATE input   |
//! | 2    | Rate generator                | yes      | IRQ0 timer tick mode      |
//! | 3    | Square wave generator         | yes      | Even/odd divisors; 0 means 65536 |
//! | 4    | Software triggered strobe     | no       | rarely used on the PC     |
//! | 5    | Hardware triggered strobe     | no       | needs a real GATE input   |
//!
//! Modes 6 and 7 alias to modes 2 and 3 on real silicon; this model applies the
//! same aliasing.
//!
//! Per the Intel 8254 datasheet, Mode 3 ("Square Wave Mode"), OUT starts high.
//! Even counts give equal high/low halves; odd counts load N-1 and keep OUT high
//! for (N+1)/2 clocks and low for (N-1)/2 clocks.
//!
//! Deliberately **not** modelled:
//! - **BCD counting** (command bit 0): the bit is stored and echoed in the
//!   read-back path but counting is always binary. The PC BIOS programs binary
//!   mode, so this does not affect boot fidelity.
//! - **The 8254 read-back command** (`0x43` with bits 7-6 == `11`): it is decoded
//!   only far enough to be a no-op; status/count read-back is not implemented.
//! - **The GATE input**: all counters are treated as permanently gated on
//!   (GATE = 1), which is the PC/AT wiring for counters 0 and 1. Counter 2's
//!   GATE is software-controllable via port `0x61` on the real machine; that port
//!   belongs to a different chip and is out of scope here.
//! - **The status-latch command** and the null-count flag.
//!
//! Snapshots contain all counter registers, byte/latch phases and the absolute
//! PIT time base. OUT deadlines are derived from those saved counter phases.
//! See Intel 8254 datasheet, Counter Programming and Modes 0, 2 and 3.

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "8253-pit";

/// First port decoded by the PIT (counter 0 data).
const BASE: u16 = 0x40;

/// Command/mode register port (`0x43`).
const COMMAND: u16 = 0x43;

/// Number of counters in the chip.
const COUNTERS: usize = 3;

/// Snapshot layout version tag, first byte of [`Device::save`] output.
const SNAPSHOT_VERSION: u8 = 2;

/// The access mode selected by command bits 5-4, controlling how the LSB/MSB of
/// a counter are written and read.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Access {
    /// Read/load the low byte only (bits 5-4 == `01`).
    LoOnly,
    /// Read/load the high byte only (bits 5-4 == `10`).
    HiOnly,
    /// Read/load the low byte then the high byte (bits 5-4 == `11`).
    LoHi,
}

impl Access {
    /// Decode command bits 5-4 into an access mode. `00` (latch) is handled
    /// before this is reached, so it is not represented here.
    fn from_command(cmd: u8) -> Access {
        match (cmd >> 4) & 0b11 {
            1 => Access::LoOnly,
            2 => Access::HiOnly,
            _ => Access::LoHi, // 3; 0 is the latch command, filtered earlier.
        }
    }

    /// Serialize for snapshots.
    fn to_bits(self) -> u8 {
        match self {
            Access::LoOnly => 1,
            Access::HiOnly => 2,
            Access::LoHi => 3,
        }
    }

    /// Deserialize from a snapshot byte.
    fn from_bits(bits: u8) -> Access {
        match bits {
            1 => Access::LoOnly,
            2 => Access::HiOnly,
            _ => Access::LoHi,
        }
    }
}

/// Which byte a `LoHi` access will touch next.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BytePhase {
    /// The next data-port access is the low byte.
    Low,
    /// The next data-port access is the high byte.
    High,
}

/// One of the three independent counting channels.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Counter {
    /// Operating mode 0..=5 as programmed (6/7 are folded to 2/3).
    mode: u8,
    /// Access mode for LSB/MSB sequencing.
    access: Access,
    /// BCD flag (command bit 0). Stored only; counting is always binary.
    bcd: bool,
    /// The reload value written by the guest; `0` means 65536.
    reload: u16,
    /// The live counting element. Loads one tick after a write; mode 3 clears
    /// the reload's low bit so the decrement-by-two sequence can reach zero.
    count: u16,
    /// Current OUT pin level.
    out: bool,
    /// Which byte a `LoHi` write/read touches next.
    write_phase: BytePhase,
    /// Which byte a `LoHi` read touches next (tracked separately from writes).
    read_phase: BytePhase,
    /// Latched value for a counter-latch readback, if one is pending.
    latch: Option<u16>,
    /// Set after a full reload write; the counting element loads on the next
    /// tick, reproducing the 8254's one-clock load delay.
    reload_pending: bool,
    /// Whether the counter has ever been programmed (has a live count).
    armed: bool,
}

impl Counter {
    /// Power-on state: OUT high (as a real 8254 idles), no program loaded.
    fn new() -> Counter {
        Counter {
            mode: 0,
            access: Access::LoHi,
            bcd: false,
            reload: 0,
            count: 0,
            out: true,
            write_phase: BytePhase::Low,
            read_phase: BytePhase::Low,
            latch: None,
            reload_pending: false,
            armed: false,
        }
    }

    /// Effective initial count: a written `0` divisor means the full 65536.
    fn initial(&self) -> u32 {
        if self.reload == 0 {
            65536
        } else {
            u32::from(self.reload)
        }
    }

    /// Apply a mode/command byte that selected this counter (not a latch).
    fn program(&mut self, cmd: u8) {
        let mode = (cmd >> 1) & 0b111;
        // Modes 6 and 7 alias to 2 and 3 on real silicon.
        self.mode = match mode {
            6 => 2,
            7 => 3,
            other => other,
        };
        self.access = Access::from_command(cmd);
        self.bcd = cmd & 1 != 0;
        self.write_phase = BytePhase::Low;
        self.read_phase = BytePhase::Low;
        self.latch = None;
        self.reload_pending = false;
        self.armed = false;
        // Writing the control word sets OUT to its mode-defined idle level.
        // Modes 0 start OUT low; modes 2 and 3 start OUT high.
        self.out = self.mode != 0;
    }

    /// Latch the current count for a subsequent readback (latch command).
    fn latch_count(&mut self) {
        if self.latch.is_none() {
            self.latch = Some(self.count);
            self.read_phase = BytePhase::Low;
        }
    }

    /// Handle a write to this counter's data port.
    fn write_data(&mut self, byte: u8) {
        match self.access {
            Access::LoOnly => {
                self.reload = (self.reload & 0xFF00) | u16::from(byte);
                self.arm_reload();
            }
            Access::HiOnly => {
                self.reload = (self.reload & 0x00FF) | (u16::from(byte) << 8);
                self.arm_reload();
            }
            Access::LoHi => match self.write_phase {
                BytePhase::Low => {
                    self.reload = (self.reload & 0xFF00) | u16::from(byte);
                    self.write_phase = BytePhase::High;
                }
                BytePhase::High => {
                    self.reload = (self.reload & 0x00FF) | (u16::from(byte) << 8);
                    self.write_phase = BytePhase::Low;
                    self.arm_reload();
                }
            },
        }
    }

    /// Mark that a complete reload value has been written; the counting element
    /// loads on the next tick.
    fn arm_reload(&mut self) {
        self.armed = true;
        self.reload_pending = true;
        // Modes 0 drop OUT low immediately when a new count is written.
        if self.mode == 0 {
            self.out = false;
        }
    }

    /// Handle a read from this counter's data port, returning the byte value.
    fn read_data(&mut self) -> u8 {
        let value = self.latch.unwrap_or(self.count);
        match self.access {
            Access::LoOnly => {
                self.latch = None;
                value as u8
            }
            Access::HiOnly => {
                self.latch = None;
                (value >> 8) as u8
            }
            Access::LoHi => match self.read_phase {
                BytePhase::Low => {
                    self.read_phase = BytePhase::High;
                    value as u8
                }
                BytePhase::High => {
                    // A two-byte latch readback is now fully consumed.
                    self.read_phase = BytePhase::Low;
                    self.latch = None;
                    (value >> 8) as u8
                }
            },
        }
    }

    /// Clocks until the next Mode 3 OUT transition. Intel 8254 datasheet,
    /// Mode 3: odd high halves include the clock spent at zero. A zero even
    /// count wraps through 65536; this also preserves the model's divisor-1 case.
    fn mode3_edge(&self) -> u64 {
        if self.reload & 1 != 0 && self.out {
            u64::from(self.count) / 2 + 1
        } else if self.count == 0 {
            32768
        } else {
            u64::from(self.count) / 2
        }
    }

    /// Arithmetic equivalent of repeated input edges, including the one-clock
    /// load delay (Intel 8254 datasheet, Modes 0, 2 and 3). Byte/latch phases
    /// belong to accesses and must survive advancement unchanged.
    fn advance(&mut self, mut clocks: u64) {
        if clocks == 0 {
            return;
        }
        if self.reload_pending {
            self.reload_pending = false;
            self.count = self.initial() as u16;
            if self.mode == 3 {
                self.count &= !1;
            }
            clocks -= 1;
        }
        if clocks == 0 || !self.armed {
            return;
        }
        match self.mode {
            0 => {
                let terminal = if self.count == 0 {
                    65536
                } else {
                    u64::from(self.count)
                };
                if clocks >= terminal {
                    self.out = true;
                }
                self.count = self.count.wrapping_sub(clocks as u16);
            }
            2 => {
                let falling = if self.count == 0 {
                    65536
                } else {
                    u64::from(self.count)
                };
                if clocks < falling {
                    self.count = self.count.wrapping_sub(clocks as u16);
                    self.out = true;
                } else {
                    let phase = (clocks - falling) % u64::from(self.initial());
                    self.count = (self.initial() as u16).wrapping_sub(phase as u16);
                    self.out = phase != 0;
                }
            }
            3 => {
                // After the first edge the reload defines both half-periods.
                // Reduce whole periods so at most two further edges remain,
                // regardless of the jump length or a partially written reload.
                let half = u64::from(self.reload & !1) / 2;
                let low = if half == 0 { 32768 } else { half };
                let high = if self.reload & 1 != 0 { half + 1 } else { low };
                while clocks != 0 {
                    let edge = self.mode3_edge();
                    if clocks < edge {
                        self.count = self.count.wrapping_sub((clocks * 2) as u16);
                        break;
                    }
                    clocks -= edge;
                    self.out = !self.out;
                    self.count = self.reload & !1;
                    clocks %= high + low;
                }
            }
            _ => self.count = self.count.wrapping_sub(clocks as u16),
        }
    }

    #[cfg(test)]
    /// Advance the counting element by one input clock edge.
    fn tick(&mut self) {
        // The reload written last tick loads now (one-clock 8254 load delay).
        if self.reload_pending {
            self.reload_pending = false;
            self.count = self.initial() as u16;
            if self.mode == 3 {
                self.count &= !1;
            }
            return;
        }
        if !self.armed {
            return;
        }
        match self.mode {
            0 => self.tick_mode0(),
            2 => self.tick_mode2(),
            3 => self.tick_mode3(),
            // Unmodelled modes still decrement so reads are not frozen, but
            // drive no OUT transitions. They are documented as not modelled.
            _ => {
                self.count = self.count.wrapping_sub(1);
            }
        }
    }

    /// Mode 0: interrupt on terminal count. OUT stays low after a reload and
    /// goes high when the count reaches 0, then remains high.
    #[cfg(test)]
    fn tick_mode0(&mut self) {
        if self.count == 0 {
            // Already at terminal count; keep wrapping like the real counter.
            self.count = self.count.wrapping_sub(1);
            return;
        }
        self.count -= 1;
        if self.count == 0 {
            self.out = true;
        }
    }

    /// Mode 2: rate generator. OUT is high while counting down; it goes low for
    /// exactly one clock when the count reaches 1, then the counter reloads to
    /// its initial value and OUT returns high. Successive OUT-low pulses are
    /// `initial` clocks apart.
    #[cfg(test)]
    fn tick_mode2(&mut self) {
        if self.count == 1 {
            // Passing through 1: emit the one-clock OUT-low pulse and reload.
            self.out = false;
            self.count = self.initial() as u16;
        } else {
            // Normal count-down; OUT is high the rest of the period.
            self.out = true;
            self.count = self.count.wrapping_sub(1);
        }
    }

    /// Intel 8254 datasheet, Mode 3 ("Square Wave Mode"): decrement by two,
    /// reloading N for even divisors or N-1 for odd divisors. An odd divisor
    /// holds OUT high for one extra clock after the count expires, giving a
    /// high half of (N+1)/2 clocks and a low half of (N-1)/2 clocks.
    #[cfg(test)]
    fn tick_mode3(&mut self) {
        let odd_high = self.reload & 1 != 0 && self.out;
        if odd_high && self.count == 0 {
            self.out = false;
            self.count = self.reload & !1;
            return;
        }
        self.count = self.count.wrapping_sub(2);
        if self.count == 0 && !odd_high {
            self.out = !self.out;
            self.count = self.reload & !1;
        }
    }
}

/// The 8253/8254 Programmable Interval Timer (§7.4).
///
/// Decodes ports `0x40..=0x43`. Counter 0's OUT drives the machine timer
/// interrupt (IRQ0) and is exposed through [`Device::irq_pending`];
/// [`Device::irq_vector`] returns `None` because vectoring is owned by the 8259
/// PIC, not the PIT — the PIT only drives a line. Counters 1 and 2 are modelled
/// but their OUT pins are not routed.
pub struct Pit8253 {
    counters: [Counter; COUNTERS],
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl Pit8253 {
    /// Create a PIT in power-on state decoding ports `0x40..=0x43`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counters: [Counter::new(), Counter::new(), Counter::new()],
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// The current OUT pin level of counter `n` (0..=2). Counter 0's OUT is the
    /// IRQ0 source.
    #[must_use]
    pub fn out(&self, n: usize) -> bool {
        self.counters[n].out
    }

    /// The live counting element of counter `n` (0..=2), for tests and tracing.
    #[must_use]
    pub fn count(&self, n: usize) -> u16 {
        self.counters[n].count
    }

    /// Byte offset of `port` within the PIT window, or an unmapped-port error.
    fn offset(&self, port: u16) -> Result<u16, DeviceError> {
        if !(BASE..=COMMAND).contains(&port) {
            return Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port.wrapping_sub(BASE)),
            });
        }
        Ok(port - BASE)
    }

    /// Apply a write to the mode/command register (`0x43`).
    fn write_command(&mut self, cmd: u8) {
        let select = (cmd >> 6) & 0b11;
        if select == 0b11 {
            // The 8254 read-back command; decoded to a no-op (not modelled).
            return;
        }
        let idx = select as usize;
        if (cmd >> 4) & 0b11 == 0 {
            // Counter-latch command: latch the current count for readback.
            self.counters[idx].latch_count();
        } else {
            self.counters[idx].program(cmd);
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

impl Default for Pit8253 {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for Pit8253 {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let offset = self.offset(port)?;
        let value = match offset {
            0..=2 => u32::from(self.counters[offset as usize].read_data()),
            // The command register is write-only on the 8253/8254; reads float.
            3 => 0,
            _ => unreachable!("offset is bounded to 0..=3 by Self::offset"),
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let offset = self.offset(port)?;
        let byte = value as u8;
        match offset {
            0..=2 => self.counters[offset as usize].write_data(byte),
            3 => self.write_command(byte),
            _ => unreachable!("offset is bounded to 0..=3 by Self::offset"),
        }
        self.trace_port(port, size, AccessDir::Write, value);
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
        // The PIT is port-mapped on the PC/AT, never memory-mapped.
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
        // Each virtual-clock tick beyond the last one is one PIT input clock.
        let elapsed = now.0.saturating_sub(self.now.0);
        self.now = now;
        for counter in &mut self.counters {
            counter.advance(elapsed);
        }
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        let c = &self.counters[0];
        let clocks = if c.reload_pending {
            // The load does not necessarily change OUT, but establishes the
            // counter phase and the reference loop's first observed pin level.
            1
        } else if !c.armed {
            return NextEvent::Dormant;
        } else {
            match c.mode {
                0 if c.out => return NextEvent::Dormant,
                0 => {
                    if c.count == 0 {
                        65536
                    } else {
                        u64::from(c.count)
                    }
                }
                2 if !c.out && c.count == 1 && c.reload == 1 => return NextEvent::Dormant,
                2 if !c.out => 1,
                2 => {
                    if c.count == 0 {
                        65536
                    } else {
                        u64::from(c.count)
                    }
                }
                3 => c.mode3_edge(),
                // Unsupported modes decrement but do not change OUT.
                _ => return NextEvent::Dormant,
            }
        };
        self.now
            .0
            .checked_add(clocks)
            .map_or(NextEvent::Dormant, |at| NextEvent::At(VClock(at)))
    }

    fn irq_pending(&self) -> bool {
        // Counter 0's OUT pin is the IRQ0 source. The 8259 PIC owns vectoring,
        // so only the line level is exposed here.
        self.counters[0].out
    }

    fn irq_vector(&self) -> Option<u8> {
        // The PIT drives a line, not a vector: the 8259 PIC resolves IRQ0 to a
        // vector on INTA. See spec §5.4.
        None
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        *self = Pit8253::new();
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        // Layout: version, then per counter: mode, access, flags, reload(2),
        // count(2), latch(2); followed by the absolute PIT clock(8).
        let mut blob = Vec::with_capacity(1 + COUNTERS * 9 + 8);
        blob.push(SNAPSHOT_VERSION);
        for c in &self.counters {
            blob.push(c.mode);
            blob.push(c.access.to_bits());
            let mut flags = 0u8;
            if c.bcd {
                flags |= 1 << 0;
            }
            if c.out {
                flags |= 1 << 1;
            }
            if c.write_phase == BytePhase::High {
                flags |= 1 << 2;
            }
            if c.read_phase == BytePhase::High {
                flags |= 1 << 3;
            }
            if c.reload_pending {
                flags |= 1 << 4;
            }
            if c.armed {
                flags |= 1 << 5;
            }
            if c.latch.is_some() {
                flags |= 1 << 6;
            }
            blob.push(flags);
            blob.extend_from_slice(&c.reload.to_le_bytes());
            blob.extend_from_slice(&c.count.to_le_bytes());
            blob.extend_from_slice(&c.latch.unwrap_or(0).to_le_bytes());
        }
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        const PER_COUNTER: usize = 1 + 1 + 1 + 2 + 2 + 2;
        let expected = 1 + COUNTERS * PER_COUNTER + 8;
        if snapshot.len() != expected {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unexpected snapshot length",
            });
        }
        if snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        let mut counters = [Counter::new(), Counter::new(), Counter::new()];
        for (i, c) in counters.iter_mut().enumerate() {
            let off = 1 + i * PER_COUNTER;
            let flags = snapshot[off + 2];
            let count = u16::from_le_bytes([snapshot[off + 5], snapshot[off + 6]]);
            if snapshot[off] > 5
                || !(1..=3).contains(&snapshot[off + 1])
                || flags & 0x80 != 0
                || (flags & 0x10 != 0 && flags & 0x20 == 0)
                || (snapshot[off + 1] != 3 && flags & 0x0C != 0)
                || (snapshot[off] == 3 && flags & 0x20 != 0 && flags & 0x10 == 0 && count & 1 != 0)
                || (flags & 0x40 == 0 && snapshot[off + 7..off + 9] != [0, 0])
            {
                return Err(DeviceError::InvalidSnapshot {
                    device: NAME,
                    reason: "invalid counter state",
                });
            }
            c.mode = snapshot[off];
            c.access = Access::from_bits(snapshot[off + 1]);
            let flags = snapshot[off + 2];
            c.bcd = flags & (1 << 0) != 0;
            c.out = flags & (1 << 1) != 0;
            c.write_phase = if flags & (1 << 2) != 0 {
                BytePhase::High
            } else {
                BytePhase::Low
            };
            c.read_phase = if flags & (1 << 3) != 0 {
                BytePhase::High
            } else {
                BytePhase::Low
            };
            c.reload_pending = flags & (1 << 4) != 0;
            c.armed = flags & (1 << 5) != 0;
            c.reload = u16::from_le_bytes([snapshot[off + 3], snapshot[off + 4]]);
            c.count = u16::from_le_bytes([snapshot[off + 5], snapshot[off + 6]]);
            let latch = u16::from_le_bytes([snapshot[off + 7], snapshot[off + 8]]);
            c.latch = if flags & (1 << 6) != 0 {
                Some(latch)
            } else {
                None
            };
        }
        self.counters = counters;
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

    const C0: u16 = 0x40;
    const C1: u16 = 0x41;
    const C2: u16 = 0x42;
    const CMD: u16 = 0x43;

    /// Command byte builder: `select`<<6 | `access`<<4 | `mode`<<1 | `bcd`.
    fn cmd(select: u8, access: u8, mode: u8, bcd: u8) -> u32 {
        u32::from((select << 6) | (access << 4) | (mode << 1) | bcd)
    }

    /// Program counter 0 for a lo/hi write of `divisor` in the given `mode`.
    fn program_c0(pit: &mut Pit8253, mode: u8, divisor: u16) {
        pit.port_write(CMD, 1, cmd(0, 0b11, mode, 0)).unwrap();
        pit.port_write(C0, 1, u32::from(divisor & 0xFF)).unwrap();
        pit.port_write(C0, 1, u32::from(divisor >> 8)).unwrap();
    }

    /// Advance the PIT by `n` input clocks from its current virtual time.
    fn tick(pit: &mut Pit8253, n: u64) {
        let base = pit.now.0;
        pit.tick(VClock(base + n));
    }

    fn compare_jump(reference: &mut Counter, scheduled: &mut Counter, clocks: u64) {
        for _ in 0..clocks {
            reference.tick();
        }
        scheduled.advance(clocks);
        assert_eq!(reference, scheduled, "jump {clocks}");
    }

    #[test]
    fn arithmetic_matches_original_stepper_for_every_mode_and_u16_reload() {
        for mode in [0, 2, 3] {
            for reload in 0..=u16::MAX {
                let mut reference = Counter::new();
                reference.program(cmd(0, 0b11, mode, 0) as u8);
                reference.write_data(reload as u8);
                reference.write_data((reload >> 8) as u8);
                let mut scheduled = reference.clone();
                for clocks in [0, 1, 2, 17] {
                    compare_jump(&mut reference, &mut scheduled, clocks);
                }
                reference.latch_count();
                scheduled.latch_count();
                assert_eq!(reference.read_data(), scheduled.read_data());
                compare_jump(&mut reference, &mut scheduled, 13);
                assert_eq!(reference.read_data(), scheduled.read_data());
                // A low-byte write changes reload before a new load is armed.
                reference.write_data(5);
                scheduled.write_data(5);
                compare_jump(&mut reference, &mut scheduled, 19);
                reference.write_data(0);
                scheduled.write_data(0);
                for clocks in [0, 1, 37] {
                    compare_jump(&mut reference, &mut scheduled, clocks);
                }
            }
        }
    }

    #[test]
    fn arithmetic_preserves_wraps_terminal_counts_and_byte_access_modes() {
        for mode in [0, 2, 3, 6, 7] {
            for reload in [0, 1, 2, 3, 5, 8, 1193, u16::MAX] {
                for access in [1, 2, 3] {
                    let mut reference = Counter::new();
                    reference.program(cmd(0, access, mode, 0) as u8);
                    reference.write_data(reload as u8);
                    if access == 3 {
                        reference.write_data((reload >> 8) as u8);
                    }
                    let mut scheduled = reference.clone();
                    for clocks in [1, 65535, 1, 1, 131073] {
                        compare_jump(&mut reference, &mut scheduled, clocks);
                        reference.latch_count();
                        scheduled.latch_count();
                        assert_eq!(reference.read_data(), scheduled.read_data());
                    }
                }
            }
        }
    }

    #[test]
    fn mock_bus_deadline_jumps_preserve_state_accesses_and_complete_out_history() {
        for mode in [0, 2, 3] {
            for reload in [0u16, 1, 2, 3, 5, 8, 1193, u16::MAX] {
                let mut reference = crate::trace_policy_tests::traced(Pit8253::new());
                let mut scheduled =
                    MockBus::new(Box::new(crate::trace_policy_tests::traced(Pit8253::new())));
                for channel in 0..3 {
                    let data = C0 + u16::from(channel);
                    for (port, value) in [
                        (CMD, cmd(channel, 3, mode, 0)),
                        (data, u32::from(reload & 255)),
                        (data, u32::from(reload >> 8)),
                    ] {
                        reference.port_write(port, 1, value).unwrap();
                        scheduled.port_write(port, 1, value).unwrap();
                    }
                }
                let mut expected_edges = Vec::new();
                let mut actual_edges = Vec::new();
                let mut expected_level = reference.out(0);
                let mut actual_level = scheduled.device().irq_pending();
                let end = 196615;
                while scheduled.clock().0 < end {
                    let now = scheduled.clock().0;
                    let access = (now / 997 + 1) * 997;
                    let due = match scheduled.device().next_event(scheduled.clock()) {
                        NextEvent::At(at) => at.0,
                        NextEvent::Dormant => end,
                        NextEvent::EveryPitClock => panic!("PIT must provide deadlines"),
                    };
                    let target = due.min(access).min(end);
                    assert!(target > now);
                    for clock in now + 1..=target {
                        for counter in &mut reference.counters {
                            counter.tick();
                        }
                        reference.now = VClock(clock);
                        if reference.out(0) != expected_level {
                            expected_level = reference.out(0);
                            expected_edges.push((clock, expected_level));
                        }
                    }
                    scheduled.tick(target - now);
                    if scheduled.device().irq_pending() != actual_level {
                        actual_level = scheduled.device().irq_pending();
                        actual_edges.push((target, actual_level));
                    }
                    assert_eq!(
                        reference.save(),
                        scheduled.device().save(),
                        "mode {mode}, reload {reload}, clock {target}"
                    );
                    if target == access {
                        // Leave channel 2's latch partially consumed while its
                        // counter continues; latch commands must not replace it.
                        for channel in 0..3 {
                            let value = cmd(channel, 0, 0, 0);
                            reference.port_write(CMD, 1, value).unwrap();
                            scheduled.port_write(CMD, 1, value).unwrap();
                            let data = C0 + u16::from(channel);
                            assert_eq!(
                                reference.port_read(data, 1).unwrap(),
                                scheduled.port_read(data, 1).unwrap()
                            );
                            if channel != 2 {
                                assert_eq!(
                                    reference.port_read(data, 1).unwrap(),
                                    scheduled.port_read(data, 1).unwrap()
                                );
                            }
                        }
                        assert_eq!(reference.take_trace(), scheduled.device_mut().take_trace());
                    }
                }
                assert_eq!(expected_edges, actual_edges, "mode {mode}, reload {reload}");
            }
        }
    }

    #[test]
    fn deadlines_follow_only_channel_zero_loads_and_out_edges() {
        let mut pit = Pit8253::new();
        assert_eq!(pit.next_event(VClock::ZERO), NextEvent::Dormant);
        pit.port_write(CMD, 1, cmd(1, 0b11, 3, 0)).unwrap();
        pit.port_write(C1, 1, 3).unwrap();
        pit.port_write(C1, 1, 0).unwrap();
        assert_eq!(pit.next_event(VClock::ZERO), NextEvent::Dormant);
        program_c0(&mut pit, 2, 5);
        assert_eq!(pit.next_event(VClock::ZERO), NextEvent::At(VClock(1)));
        tick(&mut pit, 1);
        assert_eq!(pit.next_event(VClock(1)), NextEvent::At(VClock(6)));
        tick(&mut pit, 5);
        assert!(!pit.out(0));
        assert_eq!(pit.next_event(VClock(6)), NextEvent::At(VClock(7)));
    }

    #[test]
    fn reset_and_power_on_state() {
        let pit = Pit8253::new();
        // Counters idle with OUT high (mode-3/2 idle) and no live count.
        assert!(pit.out(0));
        assert_eq!(pit.count(0), 0);
        assert!(pit.irq_pending()); // OUT high == line asserted per our mapping.
        assert_eq!(pit.irq_vector(), None);
    }

    #[test]
    fn lohi_write_loads_after_one_tick() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 5);
        // The count loads on the tick following the full write.
        assert_eq!(pit.count(0), 0);
        tick(&mut pit, 1);
        assert_eq!(pit.count(0), 5);
    }

    #[test]
    fn lo_only_and_hi_only_access() {
        let mut pit = Pit8253::new();
        // Lo-only: only the low byte is programmable.
        pit.port_write(CMD, 1, cmd(0, 0b01, 2, 0)).unwrap();
        pit.port_write(C0, 1, 0x34).unwrap();
        tick(&mut pit, 1);
        assert_eq!(pit.count(0), 0x0034);

        // Hi-only on counter 1.
        pit.port_write(CMD, 1, cmd(1, 0b10, 2, 0)).unwrap();
        pit.port_write(C1, 1, 0x12).unwrap();
        tick(&mut pit, 1);
        assert_eq!(pit.count(1), 0x1200);
    }

    #[test]
    fn zero_divisor_means_65536() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 0);
        tick(&mut pit, 1);
        // 65536 wraps to 0 in the 16-bit element; the effective period is 65536.
        assert_eq!(pit.count(0), 0);
    }

    #[test]
    fn mode2_out_period_equals_divisor() {
        // Datasheet invariant: in mode 2 the OUT low pulse recurs every `divisor`
        // input clocks. Measure the gap between successive OUT falling edges.
        for divisor in [3u16, 4, 7, 10, 100] {
            let mut pit = Pit8253::new();
            program_c0(&mut pit, 2, divisor);

            let mut falling_edges = Vec::new();
            let mut prev = pit.out(0);
            // Load tick + several full periods.
            for step in 1..=(divisor as u64 * 4 + 2) {
                tick(&mut pit, 1);
                let now = pit.out(0);
                if prev && !now {
                    falling_edges.push(step);
                }
                prev = now;
            }
            // Consecutive falling edges are `divisor` clocks apart.
            assert!(
                falling_edges.len() >= 2,
                "divisor {divisor}: too few edges {falling_edges:?}"
            );
            for w in falling_edges.windows(2) {
                assert_eq!(
                    w[1] - w[0],
                    u64::from(divisor),
                    "divisor {divisor}: period mismatch in {falling_edges:?}"
                );
            }
        }
    }

    #[test]
    fn mode2_out_is_low_for_exactly_one_clock() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 6);
        let mut low_run = 0u32;
        let mut max_low = 0u32;
        for _ in 0..40 {
            tick(&mut pit, 1);
            if pit.out(0) {
                low_run = 0;
            } else {
                low_run += 1;
                max_low = max_low.max(low_run);
            }
        }
        assert_eq!(max_low, 1, "mode 2 OUT must pulse low for one clock only");
    }

    #[test]
    fn mode3_out_period_equals_divisor() {
        // Mode 3 square wave: a full high+low period equals `divisor` clocks.
        for divisor in [4u16, 6, 8, 20, 100] {
            let mut pit = Pit8253::new();
            program_c0(&mut pit, 3, divisor);

            let mut rising = Vec::new();
            let mut prev = pit.out(0);
            for step in 1..=(divisor as u64 * 4 + 2) {
                tick(&mut pit, 1);
                let now = pit.out(0);
                if !prev && now {
                    rising.push(step);
                }
                prev = now;
            }
            assert!(
                rising.len() >= 2,
                "divisor {divisor}: too few rising edges {rising:?}"
            );
            for w in rising.windows(2) {
                assert_eq!(
                    w[1] - w[0],
                    u64::from(divisor),
                    "divisor {divisor}: square-wave period mismatch in {rising:?}"
                );
            }
        }
    }

    #[test]
    fn mode3_half_period_is_symmetric_for_even_divisor() {
        let mut pit = Pit8253::new();
        let divisor = 8u16;
        program_c0(&mut pit, 3, divisor);
        // After the load, count the clocks OUT stays in each level over a period.
        let mut levels = Vec::new();
        for _ in 0..(divisor as u64 * 3) {
            tick(&mut pit, 1);
            levels.push(pit.out(0));
        }
        // For an even divisor the high and low halves are equal length.
        let highs = levels.iter().filter(|&&b| b).count();
        let lows = levels.len() - highs;
        assert_eq!(highs, lows, "even-divisor square wave must be 50% duty");
    }

    fn assert_mode3_waveform(divisor: u16) {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        bus.port_write(CMD, 1, cmd(0, 0b11, 3, 0)).unwrap();
        bus.port_write(C0, 1, u32::from(divisor & 0xFF)).unwrap();
        bus.port_write(C0, 1, u32::from(divisor >> 8)).unwrap();
        assert!(bus.device().irq_pending());
        bus.tick(1); // The load clock precedes the first high half-period.
        assert!(bus.device().irq_pending());

        let period = if divisor == 0 {
            65536
        } else {
            u64::from(divisor)
        };
        let high_clocks = period.div_ceil(2);
        let mut rising_edges = Vec::new();
        let mut falling_edges = Vec::new();
        let mut previous = true;
        for clock in 1..=period * 3 {
            bus.tick(1);
            let out = bus.device().irq_pending();
            assert_eq!(
                out,
                clock % period < high_clocks,
                "divisor {divisor}, clock {clock}: incorrect OUT/IRQ0 level"
            );
            if previous && !out {
                falling_edges.push(clock);
            } else if !previous && out {
                rising_edges.push(clock);
            }
            previous = out;
        }
        assert_eq!(rising_edges, vec![period, period * 2, period * 3]);
        assert_eq!(
            falling_edges,
            vec![high_clocks, period + high_clocks, period * 2 + high_clocks]
        );
    }

    #[test]
    fn mode3_odd_divisor_3_has_exact_high_low_clocks_and_irq_edges() {
        assert_mode3_waveform(3);
    }

    #[test]
    fn mode3_odd_divisor_5_has_exact_high_low_clocks_and_irq_edges() {
        assert_mode3_waveform(5);
    }

    #[test]
    fn mode3_odd_divisor_1193_has_exact_high_low_clocks_and_irq_edges() {
        assert_mode3_waveform(1193);
    }

    #[test]
    fn mode3_even_divisor_has_exact_high_low_clocks_and_irq_edges() {
        assert_mode3_waveform(8);
    }

    #[test]
    fn mode3_zero_divisor_has_65536_clock_period() {
        assert_mode3_waveform(0);
    }

    #[test]
    fn mode3_odd_count_expires_one_clock_before_out_falls() {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        bus.port_write(CMD, 1, cmd(0, 0b11, 3, 0)).unwrap();
        bus.port_write(C0, 1, 5).unwrap();
        bus.port_write(C0, 1, 0).unwrap();
        bus.tick(1);

        for (expected_count, expected_out) in [
            (4, true),
            (2, true),
            (0, true),
            (4, false),
            (2, false),
            (4, true),
        ] {
            bus.port_write(CMD, 1, cmd(0, 0b00, 0, 0)).unwrap();
            let lo = bus.port_read(C0, 1).unwrap();
            let hi = bus.port_read(C0, 1).unwrap();
            assert_eq!((hi << 8) | lo, expected_count);
            assert_eq!(bus.device().irq_pending(), expected_out);
            bus.tick(1);
        }
    }

    #[test]
    fn mode0_out_goes_high_at_terminal_count() {
        let mut pit = Pit8253::new();
        // Mode 0: OUT low after program, high when count reaches 0.
        pit.port_write(CMD, 1, cmd(0, 0b11, 0, 0)).unwrap();
        pit.port_write(C0, 1, 3).unwrap();
        pit.port_write(C0, 1, 0).unwrap();
        assert!(!pit.out(0), "mode 0 OUT is low until terminal count");
        tick(&mut pit, 1); // load (count = 3)
        assert_eq!(pit.count(0), 3);
        assert!(!pit.out(0));
        tick(&mut pit, 1); // 2
        tick(&mut pit, 1); // 1
        assert!(!pit.out(0));
        tick(&mut pit, 1); // 0 -> OUT high
        assert_eq!(pit.count(0), 0);
        assert!(pit.out(0), "mode 0 OUT must be high at terminal count");
        // OUT stays high afterwards.
        tick(&mut pit, 1);
        assert!(pit.out(0));
    }

    #[test]
    fn counter_latch_readback_freezes_value() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 100);
        tick(&mut pit, 1); // load 100
        tick(&mut pit, 10); // count now around 90
        let live = pit.count(0);
        // Latch, then keep ticking; the readback must reflect the latch instant.
        pit.port_write(CMD, 1, cmd(0, 0b00, 0, 0)).unwrap();
        tick(&mut pit, 20);
        let lo = pit.port_read(C0, 1).unwrap();
        let hi = pit.port_read(C0, 1).unwrap();
        let latched = (hi << 8) | lo;
        assert_eq!(
            latched as u16, live,
            "latched readback must be frozen count"
        );
        // After the latch is consumed, reads follow the live count again.
        let lo2 = pit.port_read(C0, 1).unwrap();
        let hi2 = pit.port_read(C0, 1).unwrap();
        let live2 = (hi2 << 8) | lo2;
        assert_ne!(live2 as u16, live, "post-latch read must track live count");
    }

    #[test]
    fn counter0_out_drives_irq_line_not_vector() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 4);
        // Drive to the OUT-low pulse and confirm irq_pending tracks OUT.
        let mut saw_low = false;
        for _ in 0..12 {
            tick(&mut pit, 1);
            assert_eq!(pit.irq_pending(), pit.out(0));
            if !pit.out(0) {
                saw_low = true;
            }
        }
        assert!(saw_low, "counter 0 must pulse its OUT/IRQ0 line low");
        assert_eq!(pit.irq_vector(), None);
    }

    #[test]
    fn command_register_read_is_benign() {
        let mut pit = Pit8253::new();
        assert_eq!(pit.port_read(CMD, 1).unwrap(), 0);
    }

    #[test]
    fn readback_command_is_a_noop() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 10);
        tick(&mut pit, 1);
        let before = pit.count(0);
        // Read-back command (bits 7-6 == 11): decoded but not modelled.
        pit.port_write(CMD, 1, cmd(0b11, 0b11, 0b111, 0)).unwrap();
        assert_eq!(pit.count(0), before, "read-back must not disturb counters");
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut pit = Pit8253::new();
        assert_eq!(
            pit.mmio_read(0x400, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x400,
            }
        );
        assert_eq!(
            pit.mmio_write(0x400, 1, 0).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x400,
            }
        );
    }

    #[test]
    fn port_outside_window_is_unmapped() {
        let mut pit = Pit8253::new();
        let err = pit.port_read(0x44, 1).unwrap_err();
        assert_eq!(
            err,
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 4,
            }
        );
    }

    #[test]
    fn reset_returns_to_power_on() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 100);
        tick(&mut pit, 5);
        pit.reset();
        assert_eq!(pit.count(0), 0);
        assert!(pit.out(0));
    }

    #[test]
    fn save_restore_round_trip() {
        let mut pit = Pit8253::new();
        program_c0(&mut pit, 2, 500);
        pit.port_write(CMD, 1, cmd(1, 0b11, 3, 0)).unwrap();
        pit.port_write(C1, 1, 0x10).unwrap();
        pit.port_write(C1, 1, 0x27).unwrap(); // divisor 0x2710 = 10000
        pit.port_write(CMD, 1, cmd(2, 0b01, 0, 0)).unwrap();
        pit.port_write(C2, 1, 0x55).unwrap();
        tick(&mut pit, 7);

        let blob = pit.save();
        let mut restored = Pit8253::new();
        restored.restore(&blob).unwrap();

        for n in 0..COUNTERS {
            assert_eq!(restored.count(n), pit.count(n), "count {n}");
            assert_eq!(restored.out(n), pit.out(n), "out {n}");
        }
        // Restored state continues identically under further ticks.
        tick(&mut pit, 20);
        restored.tick(VClock(pit.now.0));
        for n in 0..COUNTERS {
            assert_eq!(restored.count(n), pit.count(n), "post-restore count {n}");
        }
    }

    #[test]
    fn restore_rejects_bad_snapshot() {
        let mut pit = Pit8253::new();
        assert_eq!(
            pit.restore(&[SNAPSHOT_VERSION, 0, 0]).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unexpected snapshot length",
            }
        );
        let mut blob = pit.save();
        blob[0] = 0xFF;
        assert_eq!(
            pit.restore(&blob).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            }
        );
    }

    #[test]
    fn trace_seam_records_accesses() {
        let mut pit = crate::trace_policy_tests::traced(Pit8253::new());
        pit.tick(VClock(3));
        program_c0(&mut pit, 2, 12);
        let writes: Vec<(u16, u32)> = pit
            .take_trace()
            .into_iter()
            .filter_map(|ev| match ev {
                TraceEvent::Port {
                    port,
                    dir: AccessDir::Write,
                    value,
                    at,
                    ..
                } => {
                    assert_eq!(at, VClock(3));
                    Some((port, value))
                }
                _ => None,
            })
            .collect();
        assert_eq!(writes, vec![(CMD, cmd(0, 0b11, 2, 0)), (C0, 12), (C0, 0)]);
    }

    #[test]
    fn mock_bus_isolation_harness() {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        // Program counter 0, mode 2, divisor 4 through the bus.
        bus.port_write(CMD, 1, cmd(0, 0b11, 2, 0)).unwrap();
        bus.port_write(C0, 1, 4).unwrap();
        bus.port_write(C0, 1, 0).unwrap();
        // Tick a full period and confirm the IRQ0 line pulsed low once.
        let mut saw_low = false;
        for _ in 0..8 {
            bus.tick(1);
            if !bus.device().irq_pending() {
                saw_low = true;
            }
        }
        assert!(saw_low);
    }
}
