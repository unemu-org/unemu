// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8253/8254 Programmable Interval Timer (§7.4).
//!
//! Models the PIT's four port-mapped registers at `0x40..=0x43`: `0x40`/`0x41`/
//! `0x42` are the three counters' data ports and `0x43` is the mode/command
//! register. Each counter is an independent 16-bit down-counter clocked by the
//! PIT input clock. On the PC/AT that input is the ~1.193182 MHz OSC/12 signal,
//! but this device models **counts, not real time**: [`Device::tick`] catches up
//! arithmetically from supplied virtual ticks to PIT input edges. Only channel 0 OUT transitions
//! and its pending load require scheduler visits. A connected speaker observer
//! also records channel 2 phase boundaries during catch-up; accesses synchronize all counters
//! through [`Device::sync`] before reading, latching or programming (§6). This keeps
//! the device deterministic and testable without a wall clock.
//!
//! Counter 0's OUT pin is wired to IRQ0 on the real machine; here it is surfaced
//! through [`Device::irq_pending`] as a sustained level, never a consumed pulse.
//! Intel 8254 datasheet, Modes 0, 2 and 3: OUT stays high after terminal count
//! in Mode 0 until reload, goes low for one clock per period in Mode 2, and
//! alternates high/low half-periods in Mode 3. Count/status reads leave OUT
//! unchanged. See `backlog/docs/design/scheduler.md`, §PIC state must follow CPU
//! acceptance. Counters 1 (DRAM refresh) and 2 (PC speaker)
//! feed port 61h through [`SharedPit8253`]: channel 1 clocks refresh detect and
//! channel 2 supplies OUT and accepts the software GATE and speaker data enable.
//! See `backlog/docs/design/audio.md` §PC speaker for timestamped audio wiring.
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
//! Intel 8254 datasheet, Modes 2 and 3: rewriting a running counter takes
//! effect at the end of the current period (Mode 2) or half-period (Mode 3).
//! Only a complete two-byte count is eligible for loading; the current Mode 3
//! half keeps its original odd/even duration.
//!
//! Intel 8254 Programmable Interval Timer datasheet, Read-Back Command,
//! Status Byte and Counter Latch Command: count and status latch independently
//! for any selected counters. Unread latches ignore further latch requests;
//! status is read first, then count in the programmed RW format (Read Operations).
//! Latching and reading leave the counting element running.
//!
//! NULL COUNT is set by programming or a complete count write (the second byte
//! for LSB/MSB). It clears when CR transfers to CE: the existing one-clock initial
//! load, or the running Mode 2/3 period/half-period boundary. A separate flag is
//! needed because the next-clock load flag does not track those running reloads.
//! See Intel 8254 datasheet, Read-Back Command, Null Count Operation.
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
//! 8254 read-back (command bits 7-6 = `11`; not present on the 8253):
//!
//! | Feature | Modelled |
//! |---------|----------|
//! | Active-low COUNT/STATUS requests; counter selection bits 3-1 | yes |
//! | Single/multiple counter count and status latches; status-first reads | yes |
//! | Unread count/status latches held independently until consumed | yes |
//! | Reprogramming releases the count latch | yes |
//! | Reprogramming discards a pending status latch | ASSUMED; datasheet only specifies count latch release |
//! | OUT, NULL COUNT, RW, original mode bits (including 6/7), BCD flag | yes |
//! | Reserved read-back bit 0 (must be 0 per Intel 8254 datasheet, Read-Back Command) | ignored; D3h acts as D2h |
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
//! - **GATE in Modes 1/4/5**: these modes remain unsupported. Counters 0/1
//!   are tied high; the shared AT handle wires channel 2 to port 61h bit 0
//!   for Modes 0/2/3 (Intel 8254 datasheet, GATE Pin and those modes).
//! - **Divisor 1 in Modes 2/3**: unsupported waveform. Intel 8254 datasheet,
//!   Mode 2 explicitly forbids 1; Mode 3's (N-1)/2 low half would be zero.
//!   Assumption retained for deterministic compatibility, without a silicon
//!   fidelity claim: Mode 2 loads 1 with OUT high, then stays low on subsequent
//!   clocks until a valid reload. Reprogramming before that first low clock
//!   leaves OUT high and creates no rising edge. Mode 3 alternates one high
//!   clock and 32768 low clocks through the binary wrap.
//!
//! Snapshots contain all counter registers, byte/latch phases and the absolute
//! PIT time base. OUT deadlines are derived from those saved counter phases.
//! See Intel 8254 datasheet, Counter Programming and Modes 0, 2 and 3.

use device::clock::PIT;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

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
const SNAPSHOT_VERSION: u8 = 5;

/// Bytes per counter in the snapshot: mode, access, flags, reload, count,
/// count latch, active reload, write LSB, programmed mode/flags, status latch.
const PER_COUNTER: usize = 1 + 1 + 1 + 2 + 2 + 2 + 2 + 1 + 1 + 1;

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
    /// Original M2-M0 bits, since status must echo aliases exactly as written.
    programmed_mode: u8,
    /// Access mode for LSB/MSB sequencing.
    access: Access,
    /// BCD flag (command bit 0). Stored only; counting is always binary.
    bcd: bool,
    /// The reload value written by the guest; `0` means 65536.
    reload: u16,
    /// Reload that established the current period/half-period.
    active_reload: u16,
    /// First byte of an incomplete two-byte write, isolated from live reloads.
    write_low: u8,
    /// The live counting element. Loads one tick after an initial write; mode 3 clears
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
    /// Independent status latch; reading it must not consume a count byte.
    status_latch: Option<u8>,
    /// The latest complete count has not yet transferred from CR to CE.
    null_count: bool,
    /// Initial/software load pending on the next clock. Running Modes 2/3
    /// instead adopt a complete reload at their period/half-period boundary.
    reload_pending: bool,
    /// Whether the counter has ever been programmed (has a live count).
    armed: bool,
}

impl Counter {
    /// Power-on state: OUT high (as a real 8254 idles), no program loaded.
    fn new() -> Counter {
        Counter {
            mode: 0,
            programmed_mode: 0,
            access: Access::LoHi,
            bcd: false,
            reload: 0,
            active_reload: 0,
            write_low: 0,
            count: 0,
            out: true,
            write_phase: BytePhase::Low,
            read_phase: BytePhase::Low,
            latch: None,
            status_latch: None,
            null_count: true,
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
        self.programmed_mode = mode;
        // Modes 6 and 7 alias to 2 and 3 on real silicon.
        self.mode = match mode {
            6 => 2,
            7 => 3,
            other => other,
        };
        self.access = Access::from_command(cmd);
        self.bcd = cmd & 1 != 0;
        self.write_phase = BytePhase::Low;
        self.write_low = 0;
        self.read_phase = BytePhase::Low;
        self.latch = None;
        self.status_latch = None;
        self.null_count = true;
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

    /// Intel 8254 datasheet, Read-Back Command, Status Byte: the first unread
    /// status wins independently of the count latch and its byte phase.
    fn latch_status(&mut self) {
        if self.status_latch.is_none() {
            self.status_latch = Some(
                (u8::from(self.out) << 7)
                    | (u8::from(self.null_count) << 6)
                    | (self.access.to_bits() << 4)
                    | (self.programmed_mode << 1)
                    | u8::from(self.bcd),
            );
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
                    self.write_low = byte;
                    self.write_phase = BytePhase::High;
                }
                BytePhase::High => {
                    self.reload = u16::from(self.write_low) | (u16::from(byte) << 8);
                    self.write_low = 0;
                    self.write_phase = BytePhase::Low;
                    self.arm_reload();
                }
            },
        }
    }

    /// Mark that a complete reload value has been written; the counting element
    /// loads at the mode's next legal boundary.
    fn arm_reload(&mut self) {
        self.null_count = true;
        self.reload_pending = self.reload_pending || !self.armed || !matches!(self.mode, 2 | 3);
        self.armed = true;
        // Modes 0 drop OUT low immediately when a new count is written.
        if self.mode == 0 {
            self.out = false;
        }
    }

    /// Handle a read from this counter's data port, returning the byte value.
    fn read_data(&mut self) -> u8 {
        if let Some(status) = self.status_latch.take() {
            return status;
        }
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
        if self.active_reload & 1 != 0 && self.out {
            u64::from(self.count) / 2 + 1
        } else if self.count == 0 {
            32768
        } else {
            u64::from(self.count) / 2
        }
    }

    /// Arithmetic equivalent of repeated input edges, including the one-clock
    /// load delay; returns the number of OUT rising edges (Intel 8254 datasheet,
    /// Modes 0, 2 and 3). Byte/latch phases
    /// belong to accesses and must survive advancement unchanged.
    fn advance(&mut self, mut clocks: u64) -> u64 {
        let mut rising = 0;
        if clocks == 0 {
            return 0;
        }
        if self.reload_pending {
            self.reload_pending = false;
            self.null_count = false;
            self.active_reload = self.reload;
            self.count = self.initial() as u16;
            if self.mode == 3 {
                self.count &= !1;
            }
            clocks -= 1;
        }
        if clocks == 0 || !self.armed {
            return 0;
        }
        match self.mode {
            0 => {
                let terminal = if self.count == 0 {
                    65536
                } else {
                    u64::from(self.count)
                };
                if clocks >= terminal {
                    rising = u64::from(!self.out);
                    self.out = true;
                }
                self.count = self.count.wrapping_sub(clocks as u16);
            }
            2 => {
                let reloading = if self.count == 0 {
                    65536
                } else {
                    u64::from(self.count)
                };
                if clocks < reloading {
                    self.count = self.count.wrapping_sub(clocks as u16);
                } else {
                    self.active_reload = self.reload;
                    self.null_count = false;
                    // Divisor 1 has no high pulse in the retained waveform.
                    if self.reload != 1 {
                        // A just-loaded illegal count 1 can still have OUT high;
                        // reloading it cannot create a low-to-high transition.
                        rising = u64::from(reloading != 1 || !self.out)
                            + (clocks - reloading) / u64::from(self.initial());
                    }
                    let phase = (clocks - reloading) % u64::from(self.initial());
                    self.count = (self.initial() as u16).wrapping_sub(phase as u16);
                }
                self.out = self.count != 1;
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
                    rising += u64::from(self.out);
                    self.active_reload = self.reload;
                    self.null_count = false;
                    self.count = self.reload & !1;
                    rising += clocks / (high + low);
                    clocks %= high + low;
                }
            }
            _ => self.count = self.count.wrapping_sub(clocks as u16),
        }
        rising
    }

    #[cfg(test)]
    /// Advance the counting element by one input clock edge.
    fn tick(&mut self) {
        // The reload written last tick loads now (one-clock 8254 load delay).
        if self.reload_pending {
            self.reload_pending = false;
            self.null_count = false;
            self.active_reload = self.reload;
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
            self.active_reload = self.reload;
            self.null_count = false;
            self.count = self.initial() as u16;
        } else {
            self.count = self.count.wrapping_sub(1);
        }
        self.out = self.count != 1;
    }

    /// Intel 8254 datasheet, Mode 3 ("Square Wave Mode"): decrement by two,
    /// reloading N for even divisors or N-1 for odd divisors. An odd divisor
    /// holds OUT high for one extra clock after the count expires, giving a
    /// high half of (N+1)/2 clocks and a low half of (N-1)/2 clocks.
    #[cfg(test)]
    fn tick_mode3(&mut self) {
        let odd_high = self.active_reload & 1 != 0 && self.out;
        if odd_high && self.count == 0 {
            self.out = false;
            self.active_reload = self.reload;
            self.null_count = false;
            self.count = self.reload & !1;
            return;
        }
        self.count = self.count.wrapping_sub(2);
        if self.count == 0 && !odd_high {
            self.out = !self.out;
            self.active_reload = self.reload;
            self.null_count = false;
            self.count = self.reload & !1;
        }
    }
}

/// The 8253/8254 Programmable Interval Timer (§7.4).
///
/// Decodes ports `0x40..=0x43`. Counter 0's OUT drives the machine timer
/// interrupt (IRQ0) and is exposed through [`Device::irq_pending`];
/// the 8259 PIC owns vectoring while the PIT only drives a line. Counters 1
/// and 2 feed the AT system-control port through [`SharedPit8253`].
pub struct Pit8253 {
    counters: [Counter; COUNTERS],
    /// AT refresh-detect flip-flop; every channel-1 OUT rising edge toggles it.
    refresh: bool,
    /// Standalone PITs retain a tied-high gate; AT assembly connects port 61h.
    gate2: bool,
    speaker_data: bool,
    speaker_level: bool,
    speaker: Option<Box<dyn FnMut(VClock, bool)>>,
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
            refresh: false,
            gate2: true,
            speaker_data: false,
            speaker_level: false,
            speaker: None,
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

    /// Every counter as a host diagnostic sees it, at the clock the timer
    /// was last brought to (the bus brings it to its own clock before a
    /// host borrows the device): programmed mode, access mode, BCD flag,
    /// count register, counting element, OUT and GATE, and whether a count
    /// or status latch waits to be read (Intel 8254 datasheet, Control Word
    /// Format, Read-Back Command, Modes 0–5, GATE). Counters 0 and 1 have
    /// their GATE tied high on the AT. Reading it is no port access and
    /// changes nothing.
    #[must_use]
    pub fn diagnostic_state(&self) -> PitDiagnostic {
        PitDiagnostic {
            at: self.now,
            channels: std::array::from_fn(|index| {
                let c = &self.counters[index];
                PitChannelDiagnostic {
                    mode: c.programmed_mode,
                    access: match c.access {
                        Access::LoOnly => "lsb",
                        Access::HiOnly => "msb",
                        Access::LoHi => "lsb/msb",
                    },
                    bcd: c.bcd,
                    reload: c.reload,
                    count: c.count,
                    out: c.out,
                    gate: index != 2 || self.gate2,
                    armed: c.armed,
                    null_count: c.null_count,
                    count_latched: c.latch.is_some(),
                    status_latched: c.status_latch.is_some(),
                }
            }),
        }
    }

    fn record_speaker(&mut self, at: VClock) {
        let high = self.speaker_data && self.out(2);
        if high != self.speaker_level {
            self.speaker_level = high;
            if let Some(record) = &mut self.speaker {
                record(at, high);
            }
        }
    }

    /// Intel 8254 datasheet, Modes 0, 2 and 3: visit phase boundaries only,
    /// retaining every OUT2 transition even when lazy sync skips whole periods.
    fn advance_speaker(&mut self, mut at: VClock, until: VClock) {
        while PIT.edges_between(at, until) != 0 {
            let c = &self.counters[2];
            let next = if c.reload_pending {
                1
            } else if !c.armed {
                PIT.edges_between(at, until) as u64
            } else {
                match c.mode {
                    0 if !c.out => {
                        if c.count == 0 {
                            65536
                        } else {
                            u64::from(c.count)
                        }
                    }
                    2 if !c.out && c.reload == 1 && c.count == 1 => {
                        PIT.edges_between(at, until) as u64
                    }
                    2 if !c.out => 1,
                    2 => {
                        if c.count == 0 {
                            65535
                        } else {
                            u64::from(c.count.saturating_sub(1)).max(1)
                        }
                    }
                    3 => c.mode3_edge(),
                    _ => PIT.edges_between(at, until) as u64,
                }
            };
            let clocks = next.min(PIT.edges_between(at, until) as u64);
            self.counters[2].advance(clocks);
            at = PIT
                .checked_edge_at(PIT.edges_at(at) + u128::from(clocks))
                .unwrap_or(until);
            self.record_speaker(at);
        }
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

    /// Apply a write to the mode/command register (`0x43`). Intel 8254 datasheet,
    /// Read-Back Command: D5/D4 request count/status when low, D3-D1 select
    /// counters when high. Like a classic latch, this observes the clock caught
    /// up by the bus through Device::sync before the port access (§6).
    fn write_command(&mut self, cmd: u8) {
        let select = (cmd >> 6) & 0b11;
        if select == 0b11 {
            for (index, counter) in self.counters.iter_mut().enumerate() {
                if cmd & (2 << index) != 0 {
                    if cmd & 0x20 == 0 {
                        counter.latch_count();
                    }
                    if cmd & 0x10 == 0 {
                        counter.latch_status();
                    }
                }
            }
            return;
        }
        let idx = select as usize;
        if (cmd >> 4) & 0b11 == 0 {
            // Counter-latch command: latch the current count for readback.
            self.counters[idx].latch_count();
        } else {
            let was_high = self.counters[idx].out;
            self.counters[idx].program(cmd);
            if idx == 1 && !was_high && self.counters[idx].out {
                self.refresh = !self.refresh;
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
        self.record_speaker(self.now);
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
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now < self.now {
            return;
        }
        let elapsed = PIT.edges_between(self.now, now) as u64;
        let previous = self.now;
        self.now = now;
        for (index, counter) in self.counters.iter_mut().enumerate() {
            if index == 2 && self.gate2 && self.speaker_data && self.speaker.is_some() {
                continue;
            }
            if index == 2 && !self.gate2 {
                // Intel 8254 datasheet, Mode 0: GATE inhibits counting, not
                // loading. Modes 2/3 force OUT high while GATE is low.
                if elapsed != 0 && counter.reload_pending {
                    counter.advance(1);
                }
                if matches!(counter.mode, 2 | 3) {
                    counter.out = true;
                }
            } else {
                let rising = counter.advance(elapsed);
                if index == 1 && rising & 1 != 0 {
                    self.refresh = !self.refresh;
                }
            }
        }
        if self.gate2 && self.speaker_data && self.speaker.is_some() {
            self.advance_speaker(previous, now);
        }
        self.record_speaker(now);
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
                        65535
                    } else {
                        u64::from(c.count.saturating_sub(1)).max(1)
                    }
                }
                3 => c.mode3_edge(),
                // Unsupported modes decrement but do not change OUT.
                _ => return NextEvent::Dormant,
            }
        };
        PIT.checked_edge_at(PIT.edges_at(self.now) + u128::from(clocks))
            .map_or(NextEvent::Dormant, NextEvent::At)
    }

    fn irq_pending(&self) -> bool {
        // Counter 0's OUT pin is the IRQ0 source. The 8259 PIC owns vectoring,
        // so only the line level is exposed here.
        self.counters[0].out
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        // Audio wiring and the externally driven data pin outlive timer reset.
        let speaker = self.speaker.take();
        let speaker_data = self.speaker_data;
        let speaker_level = self.speaker_level;
        *self = Pit8253::new();
        self.speaker = speaker;
        self.speaker_data = speaker_data;
        self.speaker_level = speaker_level;
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        // Layout: version, then per counter: mode, access, flags, reload(2),
        // count(2), latch(2), active reload(2), write LSB, programmed mode with
        // status-latch presence in bit 3, status latch; AT line flags, PIT clock(8).
        let mut blob = Vec::with_capacity(1 + COUNTERS * PER_COUNTER + 1 + 8);
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
            if c.null_count {
                flags |= 1 << 7;
            }
            blob.push(flags);
            blob.extend_from_slice(&c.reload.to_le_bytes());
            blob.extend_from_slice(&c.count.to_le_bytes());
            blob.extend_from_slice(&c.latch.unwrap_or(0).to_le_bytes());
            blob.extend_from_slice(&c.active_reload.to_le_bytes());
            blob.push(c.write_low);
            blob.push(c.programmed_mode | (u8::from(c.status_latch.is_some()) << 3));
            blob.push(c.status_latch.unwrap_or(0));
        }
        blob.push(u8::from(self.refresh) | (u8::from(self.gate2) << 1));
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        let expected = 1 + COUNTERS * PER_COUNTER + 1 + 8;
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
            let programmed_mode = snapshot[off + 12] & 7;
            let effective_mode = match programmed_mode {
                6 | 7 => programmed_mode - 4,
                other => other,
            };
            let status_latched = snapshot[off + 12] & 8 != 0;
            let status = snapshot[off + 13];
            if snapshot[off] > 5
                || !(1..=3).contains(&snapshot[off + 1])
                || snapshot[off + 12] & 0xF0 != 0
                || effective_mode != snapshot[off]
                || (!status_latched && status != 0)
                || (status_latched
                    && status & 0x3F
                        != (snapshot[off + 1] << 4) | (programmed_mode << 1) | (flags & 1))
                || (flags & 0x04 == 0 && snapshot[off + 11] != 0)
                || (flags & 0x10 != 0 && flags & 0x20 == 0)
                || (flags & 0x80 == 0 && (flags & 0x10 != 0 || flags & 0x20 == 0))
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
            c.programmed_mode = programmed_mode;
            c.status_latch = status_latched.then_some(status);
            c.null_count = flags & 0x80 != 0;
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
            c.active_reload = u16::from_le_bytes([snapshot[off + 9], snapshot[off + 10]]);
            c.write_low = snapshot[off + 11];
            c.reload = u16::from_le_bytes([snapshot[off + 3], snapshot[off + 4]]);
            c.count = u16::from_le_bytes([snapshot[off + 5], snapshot[off + 6]]);
            let latch = u16::from_le_bytes([snapshot[off + 7], snapshot[off + 8]]);
            c.latch = if flags & (1 << 6) != 0 {
                Some(latch)
            } else {
                None
            };
        }
        let lines = snapshot[1 + COUNTERS * PER_COUNTER];
        if lines & !3 != 0 {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid AT line state",
            });
        }
        self.refresh = lines & 1 != 0;
        self.gate2 = lines & 2 != 0;
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

/// One counter as a host diagnostic sees it; see [`Pit8253::diagnostic_state`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PitChannelDiagnostic {
    /// The M2–M0 bits as programmed (6 and 7 act as 2 and 3).
    pub mode: u8,
    /// Access mode: `lsb`, `msb` or `lsb/msb`.
    pub access: &'static str,
    /// The BCD flag (stored; counting is binary).
    pub bcd: bool,
    /// The count register as last written; 0 means 65536.
    pub reload: u16,
    /// The counting element.
    pub count: u16,
    /// The OUT pin level.
    pub out: bool,
    /// The GATE input level.
    pub gate: bool,
    /// Whether a count was ever loaded since the last control word.
    pub armed: bool,
    /// The latest count has not reached the counting element yet.
    pub null_count: bool,
    /// A latched count waits to be read.
    pub count_latched: bool,
    /// A latched status byte waits to be read.
    pub status_latched: bool,
}

/// The three counters as a host diagnostic sees them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PitDiagnostic {
    /// The PIT clock the timer state was last brought to.
    pub at: VClock,
    /// Counters 0, 1 and 2.
    pub channels: [PitChannelDiagnostic; COUNTERS],
}

/// Channel-2 speaker inputs at the timer's last synchronised clock.
///
/// IBM Personal Computer AT Technical Reference, System Board, Speaker: port B
/// bit 0 drives counter 2's GATE and bit 1 enables the speaker data; Intel 8254
/// datasheet, Control Word Format and Mode 3: the programmed mode and count
/// register set the tone. A host diagnostic reads this without synchronising,
/// advancing or otherwise changing the timer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SpeakerControls {
    /// The PIT clock the timer state was last brought to.
    pub at: VClock,
    /// Port 61h bit 0, counter 2's GATE input.
    pub gate: bool,
    /// Port 61h bit 1, speaker data enable.
    pub data: bool,
    /// Counter 2's operating mode, 0 to 5.
    pub mode: u8,
    /// Counter 2's count register as last written; 0 means 65536.
    pub divisor: u16,
}

/// Shared AT timer wiring (§7.4; scheduler design §Device migration inventory).
///
/// Like the shared PIC faces, this handle keeps one device state behind both
/// the PIT ports and system-control status. The PIT snapshot owns the refresh
/// flip-flop and channel-2 gate, so restore order cannot lose a refresh edge.
/// IBM Personal Computer AT Technical Reference, System Timers and
/// System Board I/O, Port B: channel 1 requests refresh; channel 2 drives bit 5.
#[derive(Clone, Default)]
pub struct SharedPit8253(
    Rc<RefCell<Pit8253>>,
    // A bound GATE comes from the board, rather than the standalone tied-high input.
    Rc<Cell<bool>>,
);

impl SharedPit8253 {
    /// IBM Personal Computer AT Technical Reference, System Board, Speaker:
    /// the drive signal is OUT2 AND port B bit 1. Register one board observer;
    /// callbacks occur only on pin changes, not on each retired instruction.
    pub fn observe_speaker(&self, mut record: impl FnMut(VClock, bool) + 'static) {
        let mut pit = self.0.borrow_mut();
        pit.speaker_level = pit.speaker_data && pit.out(2);
        record(pit.now, pit.speaker_level);
        pit.speaker = Some(Box::new(record));
    }

    /// [`Pit8253::diagnostic_state`] of the shared timer; never synchronises it.
    #[must_use]
    pub fn diagnostic_state(&self) -> PitDiagnostic {
        self.0.borrow().diagnostic_state()
    }

    /// Read the speaker inputs for host diagnostics; never synchronises the timer.
    #[must_use]
    pub fn speaker_controls(&self) -> SpeakerControls {
        let pit = self.0.borrow();
        let counter = &pit.counters[2];
        SpeakerControls {
            at: pit.now,
            gate: pit.gate2,
            data: pit.speaker_data,
            mode: counter.mode,
            divisor: counter.reload,
        }
    }

    pub(crate) fn set_speaker_data(&self, now: VClock, enabled: bool) {
        let mut pit = self.0.borrow_mut();
        pit.sync(now);
        pit.speaker_data = enabled;
        pit.record_speaker(now);
    }

    pub(crate) fn clear_speaker_data(&self) {
        let now = self.0.borrow().now;
        self.set_speaker_data(now, false);
    }

    /// Board wiring survives device reset and is not snapshot state.
    pub(crate) fn bind_gate2(&self) {
        self.1.set(true);
    }

    /// Re-drive a cleared board control without advancing either reset clock.
    pub(crate) fn clear_gate2(&self) {
        let now = self.0.borrow().now;
        self.set_gate2(now, false);
    }

    /// Sample the AT refresh flip-flop and timer-2 OUT at the access clock.
    pub(crate) fn status(&self, now: VClock) -> (bool, bool) {
        let mut pit = self.0.borrow_mut();
        pit.sync(now);
        (pit.refresh, pit.out(2))
    }

    /// Intel 8254 datasheet, Modes 0, 2 and 3, GATE: inhibit Mode 0;
    /// force OUT high in Modes 2/3 and reload on the next rising gate edge.
    pub(crate) fn set_gate2(&self, now: VClock, high: bool) {
        let mut pit = self.0.borrow_mut();
        pit.sync(now);
        if pit.gate2 != high {
            let c = &mut pit.counters[2];
            if matches!(c.mode, 2 | 3) {
                if high {
                    c.reload_pending = c.armed;
                } else {
                    c.out = true;
                }
            }
            pit.gate2 = high;
        }
        pit.record_speaker(now);
    }
}

impl Device for SharedPit8253 {
    fn name(&self) -> &'static str {
        NAME
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.0.borrow_mut().port_read(port, size)
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.0.borrow_mut().port_write(port, size, value)
    }
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        self.0.borrow_mut().mmio_read(addr, size)
    }
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.0.borrow_mut().mmio_write(addr, size, value)
    }
    fn tick(&mut self, now: VClock) {
        self.0.borrow_mut().tick(now);
    }
    fn next_event(&self, now: VClock) -> NextEvent {
        self.0.borrow().next_event(now)
    }
    fn irq_pending(&self) -> bool {
        self.0.borrow().irq_pending()
    }
    fn reset(&mut self) {
        let mut pit = self.0.borrow_mut();
        // Intel 8254 datasheet, GATE Pin: the board drives this input independently
        // of counter programming. Preserve a bound level across timer reset so
        // resetting the timer cannot undo the system-control port's reset.
        let gate2 = self.1.get().then_some(pit.gate2);
        pit.reset();
        if let Some(high) = gate2 {
            pit.gate2 = high;
        }
    }
    fn save(&self) -> Vec<u8> {
        self.0.borrow().save()
    }
    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        self.0.borrow_mut().restore(snapshot)
    }
    fn set_trace_policy(&mut self, policy: device::TracePolicy) {
        self.0.borrow_mut().set_trace_policy(policy);
    }
    fn trace_dropped(&self) -> u64 {
        self.0.borrow().trace_dropped()
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.0.borrow_mut().take_trace()
    }
    /// Host diagnostics reach the shared timer through this downcast; see
    /// [`SharedPit8253::diagnostic_state`].
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    /// The diagnostic state shows what each counter was programmed with and
    /// where it counts; reading it changes nothing.
    #[test]
    fn diagnostic_state_shows_each_counter_mode_count_out_and_gate() {
        let pit = SharedPit8253::default();
        let mut bus = MockBus::new(Box::new(pit.clone()));
        // Counter 0: mode 2, lsb/msb, divisor 1000.
        bus.port_write(CMD, 1, cmd(0, 3, 2, 0)).unwrap();
        bus.port_write(BASE, 1, 0xE8).unwrap();
        bus.port_write(BASE, 1, 0x03).unwrap();
        bus.tick(11);
        // Counter 2: mode 3, lsb only, latched for a read.
        bus.port_write(CMD, 1, cmd(2, 1, 3, 0)).unwrap();
        bus.port_write(CMD, 1, 0x80).unwrap();
        let before = bus.device().save();
        let state = pit.diagnostic_state();
        assert_eq!(bus.device().save(), before, "reading changed the timer");
        let zero = state.channels[0];
        assert_eq!((zero.mode, zero.access, zero.reload), (2, "lsb/msb", 1000));
        // The load takes one clock, then ten clocks count down.
        assert_eq!(zero.count, 990);
        assert!(zero.out && zero.gate && zero.armed && !zero.null_count);
        let two = state.channels[2];
        assert_eq!((two.mode, two.access, two.armed), (3, "lsb", false));
        assert!(two.count_latched && two.null_count);
        assert_eq!(state.at, bus.clock());
        let channel1 = state.channels[1];
        assert!(!channel1.armed && channel1.gate);
    }

    fn fidelity_pit_bus(mode: u8, divisor: u8) -> MockBus {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        bus.port_write(CMD, 1, cmd(0, 3, mode, 0)).unwrap();
        bus.port_write(C0, 1, u32::from(divisor)).unwrap();
        bus.port_write(C0, 1, 0).unwrap();
        bus.tick(1);
        bus
    }

    fn fidelity_count(bus: &mut MockBus) -> u16 {
        bus.port_write(CMD, 1, 0).unwrap();
        let lo = bus.port_read(C0, 1).unwrap() as u16;
        lo | ((bus.port_read(C0, 1).unwrap() as u16) << 8)
    }

    #[test]
    fn fidelity_mode3_odd_half_keeps_its_extra_clock_after_even_reload() {
        let mut bus = fidelity_pit_bus(3, 5);
        bus.tick(1);
        bus.port_write(C0, 1, 4).unwrap();
        bus.port_write(C0, 1, 0).unwrap();
        bus.tick(1);
        assert_eq!(fidelity_count(&mut bus), 0);
        assert!(bus.device().irq_pending());
        bus.tick(1);
        assert!(!bus.device().irq_pending());
        assert_eq!(fidelity_count(&mut bus), 4);
    }

    #[test]
    fn fidelity_rewriting_before_initial_load_keeps_load_pending() {
        for mode in [2, 3] {
            let mut bus = MockBus::new(Box::new(Pit8253::new()));
            bus.port_write(CMD, 1, cmd(0, 3, mode, 0)).unwrap();
            for reload in [8, 4] {
                bus.port_write(C0, 1, reload).unwrap();
                bus.port_write(C0, 1, 0).unwrap();
            }
            bus.tick(1);
            assert_eq!(fidelity_count(&mut bus), 4);
        }
    }

    #[test]
    fn fidelity_mode2_reload_waits_for_current_period() {
        let mut bus = fidelity_pit_bus(2, 8);
        bus.tick(2);
        bus.port_write(C0, 1, 4).unwrap();
        bus.port_write(C0, 1, 0).unwrap();
        bus.tick(1);
        assert_eq!(fidelity_count(&mut bus), 5);
        bus.tick(4);
        assert_eq!(fidelity_count(&mut bus), 1);
        assert!(!bus.device().irq_pending());
        bus.tick(1);
        assert_eq!(fidelity_count(&mut bus), 4);
        assert!(bus.device().irq_pending());
    }

    #[test]
    fn fidelity_mode3_reload_waits_for_current_half() {
        let mut bus = fidelity_pit_bus(3, 8);
        bus.tick(1);
        bus.port_write(C0, 1, 5).unwrap();
        bus.port_write(C0, 1, 0).unwrap();
        bus.tick(1);
        assert_eq!(fidelity_count(&mut bus), 4);
        bus.tick(2);
        assert!(!bus.device().irq_pending());
        assert_eq!(fidelity_count(&mut bus), 4);
        bus.tick(2);
        assert!(bus.device().irq_pending());
        bus.tick(3);
        assert!(!bus.device().irq_pending());
    }

    #[test]
    fn fidelity_mode3_partial_reload_does_not_change_parity() {
        let mut bus = fidelity_pit_bus(3, 4);
        bus.port_write(C0, 1, 5).unwrap();
        let snapshot = bus.device().save();
        let mut restored = MockBus::new(Box::new(Pit8253::new()));
        restored.device_mut().restore(&snapshot).unwrap();
        restored.tick(1);
        for b in [&mut bus, &mut restored] {
            b.tick(2);
            assert!(!b.device().irq_pending());
            assert_eq!(fidelity_count(b), 4);
            b.tick(2);
            assert!(b.device().irq_pending());
        }
    }

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
        let mut rising = 0;
        for _ in 0..clocks {
            let was_high = reference.out;
            reference.tick();
            rising += u64::from(!was_high && reference.out);
        }
        assert_eq!(
            scheduled.advance(clocks),
            rising,
            "rising edges in jump {clocks}"
        );
        assert_eq!(reference, scheduled, "jump {clocks}");
    }

    #[test]
    fn mode2_divisor_one_reprogramming_does_not_invent_a_rising_edge() {
        // Intel 8254 datasheet, Mode 2 forbids count 1. The retained binary
        // waveform must still count only actual low-to-high OUT transitions.
        for count in [2u16, 3, 18, 0] {
            let mut reference = Counter::new();
            reference.program(cmd(1, 3, 2, 0) as u8);
            reference.write_data(1);
            reference.write_data(0);
            let mut scheduled = reference.clone();
            compare_jump(&mut reference, &mut scheduled, 1);
            assert!(reference.out);
            for counter in [&mut reference, &mut scheduled] {
                counter.write_data(count as u8);
                counter.write_data((count >> 8) as u8);
            }
            compare_jump(&mut reference, &mut scheduled, 1);
            compare_jump(&mut reference, &mut scheduled, 131_073);
        }
    }

    #[test]
    fn shared_reset_restores_the_standalone_pit_power_on_gate() {
        let mut shared = SharedPit8253::default();
        shared.set_gate2(VClock::ZERO, false);
        shared.tick(VClock(33));
        shared.reset();
        assert_eq!(shared.save(), Pit8253::new().save());
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
                // A partial write must leave the current counting phase intact.
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
                        let refresh_out = reference.out(1);
                        for counter in &mut reference.counters {
                            counter.tick();
                        }
                        if !refresh_out && reference.out(1) {
                            reference.refresh = !reference.refresh;
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
        assert_eq!(pit.next_event(VClock(1)), NextEvent::At(VClock(5)));
        tick(&mut pit, 4);
        assert!(!pit.out(0));
        assert_eq!(pit.next_event(VClock(5)), NextEvent::At(VClock(6)));
    }

    #[test]
    fn reset_and_power_on_state() {
        let pit = Pit8253::new();
        // Counters idle with OUT high (mode-3/2 idle) and no live count.
        assert!(pit.out(0));
        assert_eq!(pit.count(0), 0);
        assert!(pit.irq_pending()); // OUT high == line asserted per our mapping.
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
    }

    #[test]
    fn command_register_read_is_benign() {
        let mut pit = Pit8253::new();
        assert_eq!(pit.port_read(CMD, 1).unwrap(), 0);
    }

    #[test]
    fn readback_with_neither_latch_requested_is_a_noop() {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        readback_program(&mut bus, 0, 3, 2, 0x1234);
        bus.tick(1);
        // Neither active-low latch request is asserted.
        bus.port_write(CMD, 1, cmd(0b11, 0b11, 0b111, 0)).unwrap();
        bus.tick(3);
        assert_eq!(bus.port_read(C0, 1).unwrap(), 0x31);
        assert_eq!(bus.port_read(C0, 1).unwrap(), 0x12);
    }

    fn readback_program(bus: &mut MockBus, channel: u8, access: u8, mode: u8, count: u16) {
        let port = C0 + u16::from(channel);
        bus.port_write(CMD, 1, cmd(channel, access, mode, 0))
            .unwrap();
        if access != 2 {
            bus.port_write(port, 1, u32::from(count & 255)).unwrap();
        }
        if access != 1 {
            bus.port_write(port, 1, u32::from(count >> 8)).unwrap();
        }
    }

    fn readback_count(bus: &mut MockBus, channel: u8, access: u8) -> u16 {
        let port = C0 + u16::from(channel);
        let first = bus.port_read(port, 1).unwrap() as u16;
        match access {
            1 => first,
            2 => first << 8,
            _ => first | ((bus.port_read(port, 1).unwrap() as u16) << 8),
        }
    }

    fn readback_status(bus: &mut MockBus, channel: u8) -> u32 {
        bus.port_write(CMD, 1, 0xE0 | (2 << channel)).unwrap();
        bus.port_read(C0 + u16::from(channel), 1).unwrap()
    }

    #[test]
    fn readback_count_only_selects_one_or_several_counters_in_every_format() {
        for access in [1, 2, 3] {
            for selection in 0..8 {
                let mut bus = MockBus::new(Box::new(Pit8253::new()));
                for channel in 0..3 {
                    readback_program(&mut bus, channel, access, 0, 0xABCD);
                }
                bus.tick(1);
                bus.port_write(CMD, 1, 0xD0 | (selection << 1)).unwrap();
                bus.tick(7);
                let initial = match access {
                    1 => 0x00CD,
                    2 => 0xAB00,
                    _ => 0xABCD,
                };
                for channel in 0..3 {
                    let expected = if selection & (1 << channel) != 0 {
                        initial
                    } else {
                        initial - 7
                    };
                    let mask = match access {
                        1 => 0x00FF,
                        2 => 0xFF00,
                        _ => 0xFFFF,
                    };
                    assert_eq!(readback_count(&mut bus, channel, access), expected & mask);
                }
            }
        }
    }

    #[test]
    fn readback_status_only_preserves_programmed_fields_including_mode_aliases() {
        for channel in 0..3 {
            for access in 1..=3 {
                for mode in 0..=7 {
                    for bcd in 0..=1 {
                        let mut bus = MockBus::new(Box::new(Pit8253::new()));
                        bus.port_write(CMD, 1, cmd(channel, access, mode, bcd))
                            .unwrap();
                        let output = if mode == 0 { 0 } else { 0x80 };
                        assert_eq!(
                            readback_status(&mut bus, channel),
                            output | 0x40 | cmd(0, access, mode, bcd)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn readback_status_only_selects_multiple_counters_without_latching_counts() {
        for selection in 0..8 {
            let mut bus = MockBus::new(Box::new(Pit8253::new()));
            for channel in 0..3 {
                readback_program(&mut bus, channel, 3, 0, 0x1234);
            }
            bus.tick(1);
            bus.port_write(CMD, 1, 0xE0 | (selection << 1)).unwrap();
            bus.tick(4);
            for channel in 0..3 {
                if selection & (1 << channel) != 0 {
                    assert_eq!(bus.port_read(C0 + u16::from(channel), 1).unwrap(), 0x30);
                }
                assert_eq!(readback_count(&mut bus, channel, 3), 0x1230);
            }
        }
    }

    #[test]
    fn readback_both_returns_status_before_count_in_every_format_and_selection() {
        for selection in 0..8 {
            let mut bus = MockBus::new(Box::new(Pit8253::new()));
            for channel in 0..3 {
                readback_program(&mut bus, channel, channel + 1, 0, 0xABCD);
            }
            bus.tick(1);
            bus.port_write(CMD, 1, 0xC0 | (selection << 1)).unwrap();
            bus.tick(7);
            for channel in 0..3 {
                let access = channel + 1;
                let selected = selection & (1 << channel) != 0;
                if selected {
                    assert_eq!(
                        bus.port_read(C0 + u16::from(channel), 1).unwrap(),
                        u32::from(access << 4)
                    );
                }
                let expected = match (access, selected) {
                    (1, true) => 0xCD,
                    (1, false) => 0xC6,
                    (2, true) => 0xAB00,
                    (2, false) => 0xAA00,
                    (_, true) => 0xABCD,
                    (_, false) => 0xABC6,
                };
                assert_eq!(readback_count(&mut bus, channel, access), expected);
            }
        }
    }

    #[test]
    fn readback_latches_ignore_repeated_commands_while_counting_continues() {
        let mut bus = fidelity_pit_bus(2, 8);
        bus.port_write(CMD, 1, 0xC2).unwrap();
        bus.tick(7);
        bus.port_write(CMD, 1, 0xC2).unwrap();
        bus.port_write(CMD, 1, 0).unwrap();
        assert_eq!(bus.port_read(C0, 1).unwrap(), 0xB4);
        assert_eq!(bus.port_read(C0, 1).unwrap(), 8);
        // A partially read count is still latched; status is independent.
        bus.port_write(CMD, 1, 0xC2).unwrap();
        bus.port_write(CMD, 1, 0).unwrap();
        assert_eq!(bus.port_read(C0, 1).unwrap(), 0x34);
        bus.tick(1);
        assert_eq!(bus.port_read(C0, 1).unwrap(), 0);
        assert_eq!(readback_count(&mut bus, 0, 3), 8);
        bus.tick(2);
        bus.port_write(CMD, 1, 0xD2).unwrap();
        assert_eq!(readback_count(&mut bus, 0, 3), 6);
    }

    #[test]
    fn readback_and_classic_count_latches_share_the_first_unread_value() {
        for first in [0, 0xD2] {
            let mut bus = fidelity_pit_bus(2, 0xFE);
            bus.port_write(CMD, 1, first).unwrap();
            bus.tick(5);
            bus.port_write(CMD, 1, if first == 0 { 0xD2 } else { 0 })
                .unwrap();
            bus.tick(3);
            assert_eq!(readback_count(&mut bus, 0, 3), 0xFE);
            assert_eq!(readback_count(&mut bus, 0, 3), 0xF6);

            let mut bus = MockBus::new(Box::new(Pit8253::new()));
            readback_program(&mut bus, 0, 3, 0, 0x1234);
            bus.tick(1);
            bus.port_write(CMD, 1, first).unwrap();
            assert_eq!(bus.port_read(C0, 1).unwrap(), 0x34);
            bus.tick(0x101);
            bus.port_write(CMD, 1, if first == 0 { 0xD2 } else { 0 })
                .unwrap();
            assert_eq!(bus.port_read(C0, 1).unwrap(), 0x12);
            assert_eq!(readback_count(&mut bus, 0, 3), 0x1133);
        }
    }

    #[test]
    fn readback_status_has_priority_regardless_of_latch_order() {
        for count_first in [false, true] {
            let mut bus = fidelity_pit_bus(2, 8);
            let commands = if count_first { [0, 0xE2] } else { [0xE2, 0] };
            for command in commands {
                bus.port_write(CMD, 1, command).unwrap();
            }
            bus.tick(3);
            assert_eq!(bus.port_read(C0, 1).unwrap(), 0xB4);
            assert_eq!(readback_count(&mut bus, 0, 3), 8);
        }
    }

    #[test]
    fn readback_status_does_not_advance_a_live_two_byte_read() {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        readback_program(&mut bus, 0, 3, 0, 0x1234);
        bus.tick(1);
        assert_eq!(bus.port_read(C0, 1).unwrap(), 0x34);
        assert_eq!(readback_status(&mut bus, 0), 0x30);
        assert_eq!(bus.port_read(C0, 1).unwrap(), 0x12);
    }

    #[test]
    fn readback_status_reports_out_across_mode2_and_mode3_periods() {
        for (mode, levels) in [
            (2, [true, true, true, true, false, true]),
            (3, [true, true, true, false, false, true]),
        ] {
            let mut bus = fidelity_pit_bus(mode, 5);
            for high in levels {
                assert_eq!(
                    readback_status(&mut bus, 0),
                    if high { 0xB0 } else { 0x30 } | u32::from(mode << 1)
                );
                bus.tick(1);
            }
        }
    }

    /// Intel 8254 datasheet, Modes 2 and 3: IRQ0 is OUT0, including the
    /// one-clock rate-generator low phase and both square-wave half-periods.
    #[test]
    fn irq_level_matches_out0_across_status_and_count_reads() {
        for (mode, levels) in [
            (2, [true, true, true, true, false, true]),
            (3, [true, true, true, false, false, true]),
        ] {
            let mut bus = fidelity_pit_bus(mode, 5);
            for high in levels {
                assert_eq!(bus.device().irq_pending(), high);
                assert_eq!(readback_status(&mut bus, 0) & 0x80 != 0, high);
                bus.port_write(CMD, 1, 0).unwrap();
                bus.port_read(C0, 1).unwrap();
                bus.port_read(C0, 1).unwrap();
                assert_eq!(bus.device().irq_pending(), high);
                assert_eq!(bus.device().irq_output(0), high);
                bus.tick(1);
            }
        }
    }

    #[test]
    fn readback_null_count_tracks_control_complete_write_and_initial_load() {
        for channel in 0..3 {
            for access in 1..=3 {
                let mut bus = MockBus::new(Box::new(Pit8253::new()));
                let port = C0 + u16::from(channel);
                bus.port_write(CMD, 1, cmd(channel, access, 0, 0)).unwrap();
                let fields = u32::from(access << 4);
                assert_eq!(readback_status(&mut bus, channel), 0x40 | fields);
                bus.port_write(port, 1, 8).unwrap();
                bus.tick(1);
                assert_eq!(
                    readback_status(&mut bus, channel),
                    fields | if access == 3 { 0x40 } else { 0 }
                );
                if access == 3 {
                    bus.port_write(port, 1, 0).unwrap();
                    assert_eq!(readback_status(&mut bus, channel), 0x40 | fields);
                    bus.tick(1);
                    assert_eq!(readback_status(&mut bus, channel), fields);
                }
                bus.port_write(port, 1, 4).unwrap();
                if access == 3 {
                    assert_eq!(readback_status(&mut bus, channel), fields);
                    bus.port_write(port, 1, 0).unwrap();
                }
                assert_eq!(readback_status(&mut bus, channel), 0x40 | fields);
                bus.tick(1);
                assert_eq!(readback_status(&mut bus, channel), fields);
            }
        }
    }

    #[test]
    fn readback_reprogramming_sets_only_the_selected_counters_null_count() {
        for selected in 0..3 {
            let mut bus = MockBus::new(Box::new(Pit8253::new()));
            for channel in 0..3 {
                readback_program(&mut bus, channel, 3, 2, 8);
            }
            bus.tick(1);
            for channel in 0..3 {
                assert_eq!(readback_status(&mut bus, channel) & 0x40, 0);
            }
            let waiting = (selected + 1) % 3;
            let port = C0 + u16::from(waiting);
            bus.port_write(port, 1, 4).unwrap();
            bus.port_write(port, 1, 0).unwrap();
            let before: Vec<_> = (0..3)
                .map(|channel| readback_status(&mut bus, channel) & 0x40)
                .collect();
            assert_eq!(before[usize::from(waiting)], 0x40);

            bus.port_write(CMD, 1, cmd(selected, 3, 3, 0)).unwrap();
            for channel in 0..3 {
                assert_eq!(
                    readback_status(&mut bus, channel) & 0x40,
                    if channel == selected {
                        0x40
                    } else {
                        before[usize::from(channel)]
                    }
                );
            }
        }
    }

    #[test]
    fn readback_null_count_waits_for_running_mode2_or_mode3_reload_boundary() {
        for (mode, remaining) in [(2, 7), (3, 3)] {
            let mut bus = fidelity_pit_bus(mode, 8);
            bus.tick(1);
            bus.port_write(C0, 1, 4).unwrap();
            assert_eq!(readback_status(&mut bus, 0) & 0x40, 0);
            bus.port_write(C0, 1, 0).unwrap();
            assert_eq!(readback_status(&mut bus, 0) & 0x40, 0x40);
            bus.tick(remaining - 1);
            assert_eq!(readback_status(&mut bus, 0) & 0x40, 0x40);
            bus.tick(1);
            assert_eq!(readback_status(&mut bus, 0) & 0x40, 0);
            assert_eq!(readback_count(&mut bus, 0, 3), 4);
        }
    }

    #[test]
    fn readback_d2_and_d3_match_classic_latch_at_the_same_sync_clock() {
        for mode in [0, 2, 3] {
            for clock in [1, 17, 65536, 100003] {
                for command in [0xD2, 0xD3] {
                    let mut classic = MockBus::new(Box::new(Pit8253::new()));
                    let mut readback = MockBus::new(Box::new(Pit8253::new()));
                    for (bus, command) in [(&mut classic, 0), (&mut readback, command)] {
                        readback_program(bus, 0, 3, mode, 1193);
                        bus.device_mut().sync(VClock(clock));
                        bus.port_write(CMD, 1, command).unwrap();
                        bus.device_mut().sync(VClock(clock + 11));
                    }
                    assert_eq!(
                        readback_count(&mut readback, 0, 3),
                        readback_count(&mut classic, 0, 3)
                    );
                }
            }
        }
    }

    #[test]
    fn readback_snapshot_preserves_pending_status_count_null_and_raw_mode() {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        readback_program(&mut bus, 0, 3, 6, 8);
        bus.port_write(CMD, 1, 0xC2).unwrap();
        let snapshot = bus.device().save();
        let mut restored = MockBus::new(Box::new(Pit8253::new()));
        restored.device_mut().restore(&snapshot).unwrap();
        for b in [&mut bus, &mut restored] {
            b.tick(1);
            b.port_write(CMD, 1, 0xE2).unwrap();
            assert_eq!(b.port_read(C0, 1).unwrap(), 0xFC);
            assert_eq!(readback_count(b, 0, 3), 0);
            assert_eq!(readback_status(b, 0), 0xBC);
        }
    }

    #[test]
    fn readback_snapshot_preserves_null_count_while_mode2_reload_waits() {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        for channel in 0..3 {
            readback_program(&mut bus, channel, 3, 2, 8);
        }
        bus.tick(3);
        bus.port_write(C0, 1, 4).unwrap();
        bus.port_write(C0, 1, 0).unwrap();
        assert_eq!(readback_status(&mut bus, 0) & 0x40, 0x40);
        let snapshot = bus.device().save();
        let mut restored = MockBus::new(Box::new(Pit8253::new()));
        restored.device_mut().restore(&snapshot).unwrap();
        assert_eq!(readback_status(&mut restored, 0) & 0x40, 0x40);
        assert_eq!(readback_count(&mut restored, 0, 3), 6);
        restored.device_mut().sync(VClock(8));
        assert_eq!(readback_status(&mut restored, 0) & 0x40, 0x40);
        restored.device_mut().sync(VClock(9));
        assert_eq!(readback_status(&mut restored, 0) & 0x40, 0);
        assert_eq!(readback_count(&mut restored, 0, 3), 4);
    }

    #[test]
    fn restore_rejects_clear_null_count_before_the_initial_load() {
        for channel in 0..3 {
            for state in 0..3 {
                let mut bus = MockBus::new(Box::new(Pit8253::new()));
                match state {
                    1 => bus.port_write(CMD, 1, cmd(channel, 3, 2, 0)).unwrap(),
                    2 => readback_program(&mut bus, channel, 3, 2, 8),
                    _ => {}
                }
                let valid = bus.device().save();
                bus.device_mut().restore(&valid).unwrap();
                let mut invalid = valid.clone();
                invalid[1 + usize::from(channel) * PER_COUNTER + 2] &= !0x80;
                assert_eq!(
                    bus.device_mut().restore(&invalid),
                    Err(DeviceError::InvalidSnapshot {
                        device: NAME,
                        reason: "invalid counter state",
                    })
                );
                assert_eq!(bus.device().save(), valid);
            }
        }
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

#[cfg(test)]
mod clock_domain_regressions {
    use super::*;

    use testkit::MockBus;
    #[test]
    fn clock_domain_pit_load_count_and_deadline_follow_input_edges() {
        let mut bus = MockBus::new(Box::new(Pit8253::new()));
        bus.port_write(0x43, 1, 0x30).unwrap();
        bus.port_write(0x40, 1, 4).unwrap();
        bus.port_write(0x40, 1, 0).unwrap();
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(PIT.edge_at(1))
        );
        bus.tick(PIT.ticks(1));
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(PIT.edge_at(5))
        );
        bus.tick(PIT.ticks(3));
        assert!(!bus.device().irq_pending());
        bus.tick(PIT.ticks(1));
        assert!(bus.device().irq_pending());
    }
    #[test]
    fn clock_domain_speaker_catch_up_stamps_input_edges() {
        let pit = SharedPit8253::default();
        let edges = Rc::new(RefCell::new(Vec::new()));
        let output = edges.clone();
        pit.observe_speaker(move |at, level| output.borrow_mut().push((at, level)));
        pit.set_speaker_data(VClock::ZERO, true);
        let mut bus = MockBus::new(Box::new(pit));
        bus.port_write(0x43, 1, 0xb6).unwrap();
        bus.port_write(0x42, 1, 4).unwrap();
        bus.port_write(0x42, 1, 0).unwrap();
        edges.borrow_mut().clear();
        bus.tick(PIT.ticks(9));
        assert_eq!(
            *edges.borrow(),
            [
                (PIT.edge_at(3), false),
                (PIT.edge_at(5), true),
                (PIT.edge_at(7), false),
                (PIT.edge_at(9), true)
            ]
        );
    }
}
