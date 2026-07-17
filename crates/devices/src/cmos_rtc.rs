// SPDX-License-Identifier: AGPL-3.0-or-later
//! Motorola MC146818 CMOS RAM / real-time clock (§7.6).
//!
//! Models the PC/AT's battery-backed clock chip. Two ports are decoded: `0x70`
//! is the index register (its top bit is the **NMI disable** control), and `0x71`
//! is the data window into the selected index. The chip holds **64 bytes** of
//! CMOS NVRAM; the first 14 indices are the RTC time registers and control
//! registers A–D, and the rest is general-purpose CMOS storage the BIOS uses.
//!
//! ## Time model
//!
//! Wall-clock time is banned from the execution path, so the RTC does **not**
//! read the host clock. It is seeded with an initial time and advances purely off
//! [`Device::tick`]: a configurable number of virtual-clock ticks make one RTC
//! second. Each second increments the time and raises the update-ended flag; the
//! **update-in-progress (UIP)** flag in register A is asserted during the short
//! window before each update. Setting the SET bit (register B bit 7) freezes the
//! clock, exactly as on real silicon.
//!
//! Time is kept internally in binary and encoded on read / decoded on write
//! according to register B: the DM bit (bit 2) selects binary vs **BCD**, and the
//! 24/12 bit (bit 1) selects the hour format. This keeps the BCD conversion in
//! one place (round-trip tested).
//!
//! ## IRQ8
//!
//! The RTC's interrupt output is **IRQ8**, exposed through
//! [`Device::irq_pending`]. On the AT, IRQ8 is a line on the *slave* 8259 PIC,
//! which is not modelled yet — so nothing consumes this line. The query exists so
//! the interrupt seam is complete: [`Device::irq_vector`] returns `None` because
//! the PIC owns vectoring (§5.4). Reading register C clears the flags and
//! deasserts the line, as on hardware.
//!
//! ## Coverage matrix (datasheet vs modelled)
//!
//! | Feature                              | Modelled | Notes                          |
//! |--------------------------------------|----------|--------------------------------|
//! | 64-byte CMOS NVRAM                    | yes      | full read/write                |
//! | Time/date registers, BCD & binary    | yes      | per register B DM bit          |
//! | 12/24-hour mode                       | yes      | per register B bit 1, PM = `0x80` |
//! | Per-second update + UIP flag          | yes      | driven by `tick`               |
//! | Update-ended interrupt (UF/UIE)       | yes      | drives IRQ8 line               |
//! | Alarm interrupt (AF/AIE)              | yes      | with `0xC0` "don't care" match |
//! | NMI-disable bit (`0x70` bit 7)        | yes      | stored + exposed               |
//! | Register D VRT (valid RAM & time)     | yes      | reads back set                 |
//!
//! Deliberately **not** modelled:
//! - **Periodic interrupt** (register A rate-select RS bits): the rate bits are
//!   stored, but no periodic (PF) interrupt is generated. Boot firmware programs
//!   the periodic timer for services this project does not run.
//! - **The square-wave output** (register B SQWE / the SQW pin).
//! - **Daylight-savings auto-adjust** (register B DSE): the bit is stored only.
//! - **Oscillator control** via register A DV bits beyond the SET-bit freeze.

use device::{AccessDir, Device, DeviceError, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "mc146818-rtc";

/// Index/NMI-control port.
const PORT_INDEX: u16 = 0x70;
/// Data port into the selected CMOS index.
const PORT_DATA: u16 = 0x71;

/// Number of CMOS NVRAM bytes.
const NVRAM_LEN: usize = 64;

/// Snapshot layout version tag.
const SNAPSHOT_VERSION: u8 = 1;

// CMOS register indices.
const REG_SECONDS: usize = 0x00;
const REG_SECONDS_ALARM: usize = 0x01;
const REG_MINUTES: usize = 0x02;
const REG_MINUTES_ALARM: usize = 0x03;
const REG_HOURS: usize = 0x04;
const REG_HOURS_ALARM: usize = 0x05;
const REG_WEEKDAY: usize = 0x06;
const REG_DAY: usize = 0x07;
const REG_MONTH: usize = 0x08;
const REG_YEAR: usize = 0x09;
const REG_A: usize = 0x0A;
const REG_B: usize = 0x0B;
const REG_C: usize = 0x0C;
const REG_D: usize = 0x0D;

/// Register A bit 7: update in progress (read-only).
const A_UIP: u8 = 1 << 7;

/// Register B bit 1: 24-hour mode (else 12-hour).
const B_24HOUR: u8 = 1 << 1;
/// Register B bit 2: binary data mode (else BCD).
const B_BINARY: u8 = 1 << 2;
/// Register B bit 4: update-ended interrupt enable.
const B_UIE: u8 = 1 << 4;
/// Register B bit 5: alarm interrupt enable.
const B_AIE: u8 = 1 << 5;
/// Register B bit 7: SET — freeze the clock for programming.
const B_SET: u8 = 1 << 7;

/// Register C bit 4: update-ended flag.
const C_UF: u8 = 1 << 4;
/// Register C bit 5: alarm flag.
const C_AF: u8 = 1 << 5;
/// Register C bit 7: interrupt request flag (OR of the enabled flags).
const C_IRQF: u8 = 1 << 7;

/// Register D bit 7: valid RAM and time (battery good). Always reads set.
const D_VRT: u8 = 1 << 7;

/// The 12-hour PM indicator in the hours register.
const HOUR_PM: u8 = 0x80;

/// An alarm register value with both top bits set is a "don't care" that matches
/// any time (per the datasheet).
const ALARM_DONT_CARE: u8 = 0xC0;

/// Default virtual-clock ticks per RTC second. The virtual clock counts PIT input
/// clocks (~1.193182 MHz); one RTC second is that many ticks.
const DEFAULT_SECOND_TICKS: u64 = 1_193_182;

/// Default UIP window: the fraction of a second before the update during which
/// the update-in-progress flag reads set. ~1 ms models the real ~244 µs closely
/// enough for firmware that polls UIP.
const DEFAULT_UIP_TICKS: u64 = 1_193;

/// Broken-down binary time held by the RTC. Encoded to the guest format on read.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RtcTime {
    second: u8,
    minute: u8,
    hour: u8,
    weekday: u8,
    day: u8,
    month: u8,
    year: u8,
}

impl RtcTime {
    /// A deterministic default epoch (2000-01-01, a Saturday). Chosen so the
    /// device never reads a wall clock.
    fn default_epoch() -> RtcTime {
        RtcTime {
            second: 0,
            minute: 0,
            hour: 0,
            weekday: 7,
            day: 1,
            month: 1,
            year: 0,
        }
    }

    /// Days in `month` (1..=12) for the two-digit `year` (Gregorian; centuries
    /// divisible by 400 are leap, but two-digit `00` is treated as a leap year to
    /// match the common 2000 case).
    fn days_in_month(month: u8, year: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if year % 4 == 0 => 29,
            2 => 28,
            _ => 30,
        }
    }

    /// Advance by one second, carrying through the whole date.
    fn advance_one_second(&mut self) {
        self.second += 1;
        if self.second < 60 {
            return;
        }
        self.second = 0;
        self.minute += 1;
        if self.minute < 60 {
            return;
        }
        self.minute = 0;
        self.hour += 1;
        if self.hour < 24 {
            return;
        }
        self.hour = 0;
        self.weekday = if self.weekday >= 7 {
            1
        } else {
            self.weekday + 1
        };
        self.day += 1;
        if self.day <= Self::days_in_month(self.month, self.year) {
            return;
        }
        self.day = 1;
        self.month += 1;
        if self.month <= 12 {
            return;
        }
        self.month = 1;
        self.year = self.year.wrapping_add(1) % 100;
    }
}

/// Encode a 0..=99 binary value as BCD.
fn to_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

/// Decode a BCD byte to binary.
fn from_bcd(value: u8) -> u8 {
    (value >> 4) * 10 + (value & 0x0F)
}

/// The MC146818 CMOS/RTC device (§7.6).
///
/// Decodes ports `0x70`/`0x71`. Drives **IRQ8** through [`Device::irq_pending`]
/// (unconsumed until the slave PIC is modelled); [`Device::irq_vector`] is
/// `None`.
pub struct CmosRtc {
    /// 64-byte CMOS store. Time registers are shadowed by `time` on access; their
    /// slots here back general reads only for non-time indices.
    nvram: [u8; NVRAM_LEN],
    /// The live broken-down time.
    time: RtcTime,
    /// Currently selected CMOS index (low 6 bits meaningful).
    index: usize,
    /// NMI-disable state from `0x70` bit 7 (`true` = NMI masked).
    nmi_disabled: bool,
    /// Virtual-clock ticks accumulated toward the next second.
    accum: u64,
    /// Virtual-clock ticks that make one RTC second.
    second_ticks: u64,
    /// Width of the UIP window before each update.
    uip_ticks: u64,
    /// Whether the update-in-progress window is currently open.
    uip: bool,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: Vec<TraceEvent>,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl CmosRtc {
    /// Create an RTC seeded with the default epoch and the real per-second tick
    /// ratio, decoding ports `0x70`/`0x71`.
    #[must_use]
    pub fn new() -> Self {
        let mut nvram = [0u8; NVRAM_LEN];
        // Register B defaults to 24-hour BCD with no interrupts enabled, matching
        // the common BIOS configuration.
        nvram[REG_B] = B_24HOUR;
        // Register D reports valid RAM and time (battery good).
        nvram[REG_D] = D_VRT;
        Self {
            nvram,
            time: RtcTime::default_epoch(),
            index: 0,
            nmi_disabled: false,
            accum: 0,
            second_ticks: DEFAULT_SECOND_TICKS,
            uip_ticks: DEFAULT_UIP_TICKS,
            uip: false,
            trace: Vec::new(),
            now: VClock::ZERO,
        }
    }

    /// Override the virtual-clock timing (ticks per second and the UIP window).
    /// Intended for tests that cannot afford a million ticks per second.
    #[must_use]
    pub fn with_timing(mut self, second_ticks: u64, uip_ticks: u64) -> Self {
        self.second_ticks = second_ticks.max(1);
        self.uip_ticks = uip_ticks.min(self.second_ticks);
        self
    }

    /// Seed the broken-down time. Values are plain binary (0-based month/day are
    /// not used — `month`/`day` are 1-based).
    #[allow(clippy::too_many_arguments)]
    pub fn set_time(
        &mut self,
        second: u8,
        minute: u8,
        hour: u8,
        weekday: u8,
        day: u8,
        month: u8,
        year: u8,
    ) {
        self.time = RtcTime {
            second,
            minute,
            hour,
            weekday,
            day,
            month,
            year,
        };
    }

    /// Whether NMI is currently masked (from `0x70` bit 7).
    #[must_use]
    pub fn nmi_disabled(&self) -> bool {
        self.nmi_disabled
    }

    /// Whether register B's binary data mode is selected (else BCD).
    fn binary_mode(&self) -> bool {
        self.nvram[REG_B] & B_BINARY != 0
    }

    /// Encode a binary count into the guest format (binary or BCD per register B).
    fn encode(&self, value: u8) -> u8 {
        if self.binary_mode() {
            value
        } else {
            to_bcd(value)
        }
    }

    /// Decode a guest-format count into binary.
    fn decode(&self, value: u8) -> u8 {
        if self.binary_mode() {
            value
        } else {
            from_bcd(value)
        }
    }

    /// Encode the hour register, honouring 12/24-hour mode.
    fn encode_hour(&self) -> u8 {
        let h = self.time.hour;
        if self.nvram[REG_B] & B_24HOUR != 0 {
            self.encode(h)
        } else {
            // 12-hour: 1..=12 with the PM bit for the afternoon.
            let pm = h >= 12;
            let mut twelve = h % 12;
            if twelve == 0 {
                twelve = 12;
            }
            let mut enc = self.encode(twelve);
            if pm {
                enc |= HOUR_PM;
            }
            enc
        }
    }

    /// Decode a written hour register back into 0..=23 binary.
    fn decode_hour(&self, value: u8) -> u8 {
        if self.nvram[REG_B] & B_24HOUR != 0 {
            self.decode(value)
        } else {
            let pm = value & HOUR_PM != 0;
            let twelve = self.decode(value & !HOUR_PM);
            let base = twelve % 12;
            if pm {
                base + 12
            } else {
                base
            }
        }
    }

    /// Read the register at the selected `index`, applying live overlays.
    fn read_index(&mut self) -> u8 {
        match self.index {
            REG_SECONDS => self.encode(self.time.second),
            REG_MINUTES => self.encode(self.time.minute),
            REG_HOURS => self.encode_hour(),
            REG_WEEKDAY => self.encode(self.time.weekday),
            REG_DAY => self.encode(self.time.day),
            REG_MONTH => self.encode(self.time.month),
            REG_YEAR => self.encode(self.time.year),
            REG_A => {
                let mut a = self.nvram[REG_A] & !A_UIP;
                if self.uip {
                    a |= A_UIP;
                }
                a
            }
            REG_C => {
                // Register C is read-to-clear: return the flags, then drop them.
                let c = self.nvram[REG_C];
                self.nvram[REG_C] = 0;
                c
            }
            REG_D => self.nvram[REG_D] | D_VRT,
            other => self.nvram[other],
        }
    }

    /// Write the register at the selected `index`.
    fn write_index(&mut self, value: u8) {
        match self.index {
            REG_SECONDS => self.time.second = self.decode(value),
            REG_MINUTES => self.time.minute = self.decode(value),
            REG_HOURS => self.time.hour = self.decode_hour(value),
            REG_WEEKDAY => self.time.weekday = self.decode(value),
            REG_DAY => self.time.day = self.decode(value),
            REG_MONTH => self.time.month = self.decode(value),
            REG_YEAR => self.time.year = self.decode(value),
            // Register A UIP bit is read-only; keep the rest.
            REG_A => self.nvram[REG_A] = value & !A_UIP,
            REG_B => self.nvram[REG_B] = value,
            // Register C is read-only; writes are ignored.
            REG_C => {}
            // Register D VRT is read-only; store the remaining bits.
            REG_D => self.nvram[REG_D] = (value & !D_VRT) | (self.nvram[REG_D] & D_VRT),
            other => self.nvram[other] = value,
        }
    }

    /// Whether the alarm registers match the current time (with don't-cares).
    fn alarm_matches(&self) -> bool {
        let field = |alarm_idx: usize, now: u8| {
            let a = self.nvram[alarm_idx];
            a >= ALARM_DONT_CARE || self.decode(a) == now
        };
        field(REG_SECONDS_ALARM, self.time.second)
            && field(REG_MINUTES_ALARM, self.time.minute)
            && field(REG_HOURS_ALARM, self.time.hour)
    }

    /// Recompute register C's IRQF from the enabled flags. The periodic flag
    /// (PF) is intentionally excluded: periodic interrupts are not generated (see
    /// the coverage matrix).
    fn refresh_irqf(&mut self) {
        let b = self.nvram[REG_B];
        let c = self.nvram[REG_C];
        let active = ((c & C_UF != 0) && (b & B_UIE != 0)) || ((c & C_AF != 0) && (b & B_AIE != 0));
        if active {
            self.nvram[REG_C] |= C_IRQF;
        } else {
            self.nvram[REG_C] &= !C_IRQF;
        }
    }

    /// Advance the clock by one second, raising the update-ended and alarm flags.
    fn tick_one_second(&mut self) {
        self.time.advance_one_second();
        self.nvram[REG_C] |= C_UF;
        if self.alarm_matches() {
            self.nvram[REG_C] |= C_AF;
        }
        self.refresh_irqf();
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

impl Default for CmosRtc {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for CmosRtc {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port {
            // The index register is write-only on real hardware; return the last
            // index with the NMI bit so the port is at least self-consistent.
            PORT_INDEX => {
                let nmi = if self.nmi_disabled { 0x80 } else { 0 };
                u32::from(nmi | self.index as u8)
            }
            PORT_DATA => u32::from(self.read_index()),
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
            PORT_INDEX => {
                self.nmi_disabled = byte & 0x80 != 0;
                self.index = usize::from(byte & 0x3F);
            }
            PORT_DATA => {
                self.write_index(byte);
                // A changed enable bit can gate an already-pending flag.
                self.refresh_irqf();
            }
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
        let elapsed = now.0.saturating_sub(self.now.0);
        self.now = now;
        // The SET bit freezes the oscillator: no advance, no UIP.
        if self.nvram[REG_B] & B_SET != 0 {
            self.uip = false;
            return;
        }
        self.accum += elapsed;
        while self.accum >= self.second_ticks {
            self.accum -= self.second_ticks;
            self.tick_one_second();
        }
        // UIP reads set during the window just before the next update.
        self.uip = self.accum >= self.second_ticks - self.uip_ticks;
    }

    fn irq_pending(&self) -> bool {
        // IRQ8 is asserted whenever register C's IRQF is set. Nothing consumes it
        // until the slave PIC is modelled; the query completes the seam.
        self.nvram[REG_C] & C_IRQF != 0
    }

    fn irq_vector(&self) -> Option<u8> {
        None
    }

    fn reset(&mut self) {
        let time = self.time;
        let second_ticks = self.second_ticks;
        let uip_ticks = self.uip_ticks;
        *self = CmosRtc::new().with_timing(second_ticks, uip_ticks);
        // Preserve wall time across a controller reset, as a battery-backed clock
        // does; only volatile control/flag state returns to power-on.
        self.time = time;
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(1 + NVRAM_LEN + 7 + 1 + 1 + 8 * 3);
        blob.push(SNAPSHOT_VERSION);
        blob.extend_from_slice(&self.nvram);
        blob.push(self.time.second);
        blob.push(self.time.minute);
        blob.push(self.time.hour);
        blob.push(self.time.weekday);
        blob.push(self.time.day);
        blob.push(self.time.month);
        blob.push(self.time.year);
        blob.push(self.index as u8);
        blob.push(u8::from(self.nmi_disabled) | (u8::from(self.uip) << 1));
        blob.extend_from_slice(&self.accum.to_le_bytes());
        blob.extend_from_slice(&self.second_ticks.to_le_bytes());
        blob.extend_from_slice(&self.uip_ticks.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        const LEN: usize = 1 + NVRAM_LEN + 7 + 1 + 1 + 24;
        if snapshot.len() != LEN {
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
        let mut off = 1;
        self.nvram.copy_from_slice(&snapshot[off..off + NVRAM_LEN]);
        off += NVRAM_LEN;
        self.time = RtcTime {
            second: snapshot[off],
            minute: snapshot[off + 1],
            hour: snapshot[off + 2],
            weekday: snapshot[off + 3],
            day: snapshot[off + 4],
            month: snapshot[off + 5],
            year: snapshot[off + 6],
        };
        off += 7;
        self.index = usize::from(snapshot[off] & 0x3F);
        off += 1;
        let flags = snapshot[off];
        self.nmi_disabled = flags & 0x01 != 0;
        self.uip = flags & 0x02 != 0;
        off += 1;
        let read_u64 = |o: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&snapshot[o..o + 8]);
            u64::from_le_bytes(b)
        };
        self.accum = read_u64(off);
        self.second_ticks = read_u64(off + 8);
        self.uip_ticks = read_u64(off + 16);
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

    /// Register C bit 6: periodic flag. Not generated (see the coverage matrix);
    /// referenced only to assert it stays clear.
    const C_PF: u8 = 1 << 6;

    /// Select `index` (NMI left enabled) then read `0x71`.
    fn read_reg(rtc: &mut CmosRtc, index: u8) -> u8 {
        rtc.port_write(PORT_INDEX, 1, u32::from(index)).unwrap();
        rtc.port_read(PORT_DATA, 1).unwrap() as u8
    }

    /// Select `index` then write `0x71`.
    fn write_reg(rtc: &mut CmosRtc, index: u8, value: u8) {
        rtc.port_write(PORT_INDEX, 1, u32::from(index)).unwrap();
        rtc.port_write(PORT_DATA, 1, u32::from(value)).unwrap();
    }

    #[test]
    fn bcd_round_trip_property() {
        // Every 0..=99 value survives a BCD encode/decode round trip.
        for v in 0u8..=99 {
            assert_eq!(from_bcd(to_bcd(v)), v, "bcd round trip failed at {v}");
        }
    }

    #[test]
    fn nvram_general_bytes_round_trip() {
        let mut rtc = CmosRtc::new();
        for idx in 0x0E..0x40u8 {
            write_reg(&mut rtc, idx, idx.wrapping_mul(3));
        }
        for idx in 0x0E..0x40u8 {
            assert_eq!(read_reg(&mut rtc, idx), idx.wrapping_mul(3));
        }
    }

    #[test]
    fn time_reads_bcd_by_default() {
        let mut rtc = CmosRtc::new();
        rtc.set_time(45, 30, 13, 3, 25, 12, 24); // 13:30:45, 2024-12-25
                                                 // Register B default is BCD 24-hour.
        assert_eq!(read_reg(&mut rtc, REG_SECONDS as u8), 0x45);
        assert_eq!(read_reg(&mut rtc, REG_MINUTES as u8), 0x30);
        assert_eq!(read_reg(&mut rtc, REG_HOURS as u8), 0x13);
        assert_eq!(read_reg(&mut rtc, REG_MONTH as u8), 0x12);
        assert_eq!(read_reg(&mut rtc, REG_YEAR as u8), 0x24);
    }

    #[test]
    fn binary_mode_reads_binary() {
        let mut rtc = CmosRtc::new();
        write_reg(&mut rtc, REG_B as u8, B_24HOUR | B_BINARY);
        rtc.set_time(45, 30, 13, 3, 25, 12, 24);
        assert_eq!(read_reg(&mut rtc, REG_SECONDS as u8), 45);
        assert_eq!(read_reg(&mut rtc, REG_HOURS as u8), 13);
    }

    #[test]
    fn twelve_hour_mode_sets_pm_bit() {
        let mut rtc = CmosRtc::new();
        write_reg(&mut rtc, REG_B as u8, 0); // BCD, 12-hour
        rtc.set_time(0, 0, 13, 1, 1, 1, 0); // 13:00 -> 1 PM
        let h = read_reg(&mut rtc, REG_HOURS as u8);
        assert_ne!(h & HOUR_PM, 0, "PM bit must be set for 13:00");
        assert_eq!(h & !HOUR_PM, 0x01, "12-hour value must be 1");
        // Midnight reads as 12 AM.
        rtc.set_time(0, 0, 0, 1, 1, 1, 0);
        let h0 = read_reg(&mut rtc, REG_HOURS as u8);
        assert_eq!(h0 & HOUR_PM, 0);
        assert_eq!(h0 & !HOUR_PM, 0x12);
    }

    #[test]
    fn hour_write_read_round_trip_12h() {
        let mut rtc = CmosRtc::new();
        write_reg(&mut rtc, REG_B as u8, 0); // BCD 12-hour
                                             // Write 1 PM in 12-hour form, confirm it reads back the same.
        write_reg(&mut rtc, REG_HOURS as u8, 0x01 | HOUR_PM);
        assert_eq!(read_reg(&mut rtc, REG_HOURS as u8), 0x01 | HOUR_PM);
    }

    #[test]
    fn tick_advances_one_second() {
        let mut rtc = CmosRtc::new().with_timing(100, 10);
        rtc.set_time(58, 59, 23, 6, 28, 2, 24); // 23:59:58, 2024-02-28 (leap)
        rtc.tick(VClock(100));
        assert_eq!(read_reg(&mut rtc, REG_SECONDS as u8), 0x59);
        rtc.tick(VClock(200)); // -> 00:00:00 next day (Feb 29)
        assert_eq!(read_reg(&mut rtc, REG_SECONDS as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_MINUTES as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_DAY as u8), 0x29);
    }

    #[test]
    fn uip_flag_set_before_update() {
        let mut rtc = CmosRtc::new().with_timing(100, 10);
        rtc.tick(VClock(50)); // mid-second: UIP clear
        assert_eq!(read_reg(&mut rtc, REG_A as u8) & A_UIP, 0);
        rtc.tick(VClock(95)); // within the 10-tick window before update
        assert_ne!(read_reg(&mut rtc, REG_A as u8) & A_UIP, 0);
    }

    #[test]
    fn set_bit_freezes_clock() {
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        rtc.set_time(30, 0, 0, 1, 1, 1, 0);
        write_reg(&mut rtc, REG_B as u8, B_24HOUR | B_SET);
        rtc.tick(VClock(1000));
        assert_eq!(read_reg(&mut rtc, REG_SECONDS as u8), 0x30);
        assert_eq!(read_reg(&mut rtc, REG_A as u8) & A_UIP, 0);
    }

    #[test]
    fn update_ended_drives_irq8_when_enabled() {
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        write_reg(&mut rtc, REG_B as u8, B_24HOUR | B_UIE);
        assert!(!rtc.irq_pending());
        rtc.tick(VClock(10)); // one second elapses -> UF, IRQF
        assert!(rtc.irq_pending());
        assert_eq!(rtc.irq_vector(), None);
        // Reading register C clears the flags and deasserts the line.
        let c = read_reg(&mut rtc, REG_C as u8);
        assert_ne!(c & C_UF, 0);
        assert!(!rtc.irq_pending());
    }

    #[test]
    fn update_ended_without_enable_does_not_assert() {
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        write_reg(&mut rtc, REG_B as u8, B_24HOUR); // UIE off
        rtc.tick(VClock(10));
        assert!(!rtc.irq_pending());
        // The flag is still latched in register C, just not routed to IRQ.
        assert_ne!(read_reg(&mut rtc, REG_C as u8) & C_UF, 0);
    }

    #[test]
    fn alarm_matches_with_dont_care() {
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        rtc.set_time(59, 15, 10, 1, 1, 1, 0);
        // Alarm at any second, minute 16, hour 10.
        write_reg(&mut rtc, REG_SECONDS_ALARM as u8, ALARM_DONT_CARE);
        write_reg(&mut rtc, REG_MINUTES_ALARM as u8, to_bcd(16));
        write_reg(&mut rtc, REG_HOURS_ALARM as u8, to_bcd(10));
        write_reg(&mut rtc, REG_B as u8, B_24HOUR | B_AIE);
        rtc.tick(VClock(10)); // 10:15:59 -> 10:16:00, alarm fires
        assert!(rtc.irq_pending());
        assert_ne!(read_reg(&mut rtc, REG_C as u8) & C_AF, 0);
    }

    #[test]
    fn periodic_flag_never_generated() {
        // Rate bits stored but PF is documented as not modelled.
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        write_reg(&mut rtc, REG_A as u8, 0x26); // a periodic rate
        write_reg(&mut rtc, REG_B as u8, B_24HOUR);
        rtc.tick(VClock(1000));
        assert_eq!(read_reg(&mut rtc, REG_C as u8) & C_PF, 0);
    }

    #[test]
    fn nmi_disable_bit_tracked() {
        let mut rtc = CmosRtc::new();
        rtc.port_write(PORT_INDEX, 1, 0x80 | u32::from(REG_D as u8))
            .unwrap();
        assert!(rtc.nmi_disabled());
        rtc.port_write(PORT_INDEX, 1, u32::from(REG_D as u8))
            .unwrap();
        assert!(!rtc.nmi_disabled());
    }

    #[test]
    fn register_d_reports_valid() {
        let mut rtc = CmosRtc::new();
        assert_ne!(read_reg(&mut rtc, REG_D as u8) & D_VRT, 0);
    }

    #[test]
    fn register_a_uip_is_read_only() {
        let mut rtc = CmosRtc::new();
        write_reg(&mut rtc, REG_A as u8, 0xFF); // try to set UIP + rate bits
                                                // Rate bits stored, but the UIP bit is not settable by a write.
        assert_eq!(read_reg(&mut rtc, REG_A as u8) & A_UIP, 0);
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut rtc = CmosRtc::new();
        assert_eq!(
            rtc.mmio_read(0x70, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x70,
            }
        );
    }

    #[test]
    fn unmapped_port_errors() {
        let mut rtc = CmosRtc::new();
        assert_eq!(
            rtc.port_read(0x72, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 0x72,
            }
        );
    }

    #[test]
    fn save_restore_round_trip() {
        let mut rtc = CmosRtc::new().with_timing(500, 50);
        rtc.set_time(12, 34, 20, 4, 15, 6, 30);
        write_reg(&mut rtc, REG_B as u8, B_24HOUR | B_UIE);
        write_reg(&mut rtc, 0x20, 0xAB);
        rtc.tick(VClock(250));

        let blob = rtc.save();
        let mut restored = CmosRtc::new();
        restored.restore(&blob).unwrap();
        restored.now = rtc.now;

        assert_eq!(
            read_reg(&mut restored, REG_SECONDS as u8),
            read_reg(&mut rtc, REG_SECONDS as u8)
        );
        assert_eq!(read_reg(&mut restored, 0x20), 0xAB);
        // Continues ticking identically.
        rtc.tick(VClock(1000));
        restored.tick(VClock(1000));
        assert_eq!(
            read_reg(&mut restored, REG_SECONDS as u8),
            read_reg(&mut rtc, REG_SECONDS as u8)
        );
    }

    #[test]
    fn restore_rejects_bad_snapshot() {
        let mut rtc = CmosRtc::new();
        assert_eq!(
            rtc.restore(&[SNAPSHOT_VERSION, 0, 0]).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unexpected snapshot length",
            }
        );
    }

    #[test]
    fn reset_preserves_time_clears_flags() {
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        rtc.set_time(5, 6, 7, 1, 2, 3, 24);
        write_reg(&mut rtc, REG_B as u8, B_24HOUR | B_UIE);
        rtc.tick(VClock(10));
        assert!(rtc.irq_pending());
        rtc.reset();
        // Wall time survives; volatile flags do not.
        assert_eq!(read_reg(&mut rtc, REG_HOURS as u8), 0x07);
        assert!(!rtc.irq_pending());
    }

    #[test]
    fn mock_bus_isolation_harness() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new()));
        bus.port_write(PORT_INDEX, 1, u32::from(REG_D as u8))
            .unwrap();
        assert_ne!(bus.port_read(PORT_DATA, 1).unwrap() as u8 & D_VRT, 0);
    }
}
