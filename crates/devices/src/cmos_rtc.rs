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
//! [`Device::tick`]. IBM PC AT Technical Reference, System Board, "System
//! Timers" and "Oscillator": the timer input is the 14.31818 MHz oscillator
//! divided by twelve. Using its exact nominal ratio, 14,318,180 / 12,
//! one RTC second is 3,579,545 / 3 PIT clocks. The default phase counts thirds
//! of a PIT clock and carries the remainder across updates; events round up
//! to the first whole PIT clock at or after their boundary. Explicit
//! [`CmosRtc::with_timing`] overrides still count whole PIT clocks.
//! Each second increments the time and raises the update-ended flag; the
//! **update-in-progress (UIP)** flag in register A is asserted during the short
//! window before each update. Setting the SET bit (register B bit 7) freezes the
//! calendar updates while the periodic divider keeps running.
//! The default UIP width retains the model's approximately 1 ms polling window:
//! floor(3,579,545 / 1000) = 3579 thirds, or 1193 PIT clocks. This is an
//! approximation of the Motorola MC146818 datasheet, "Update Cycle" (244 µs
//! from UIP assertion to update start), not a model of the update bus cycle.
//!
//! Motorola MC146818 datasheet, "Update Cycle" and "Registers A–C", describes
//! the UIP/update and interrupt flags. Deadlines preserve the configured PIT
//! clocks per second and frozen SET phase; see
//! `backlog/docs/design/scheduler.md` § Device migration inventory.
//!
//! Time is kept internally in binary and encoded on read / decoded on write
//! according to register B: the DM bit (bit 2) selects binary vs **BCD**, and the
//! 24/12 bit (bit 1) selects the hour format. This keeps the BCD conversion in
//! one place (round-trip tested).
//!
//! ## IRQ8
//!
//! The RTC's interrupt output is **IRQ8**, exposed through
//! [`Device::irq_pending`]. On the AT, IRQ8 is a line on the *slave* 8259 PIC;
//! the machine edge-detects this output onto that slave input, which cascades
//! into the master at IRQ2. The PIC owns interrupt vectoring (§5.4). Reading register C clears the flags and deasserts
//! the line, as on hardware.
//!
//! ## Coverage matrix (datasheet vs modelled)
//!
//! | Feature                              | Modelled | Notes                          |
//! |--------------------------------------|----------|--------------------------------|
//! | 64-byte CMOS NVRAM                    | yes      | full read/write                |
//! | Time/date registers, BCD & binary    | yes      | per register B DM bit          |
//! | 12/24-hour mode                       | yes      | per register B bit 1, PM = `0x80` |
//! | Per-second update + UIP flag          | yes      | driven by `tick`               |
//! | Periodic interrupt (PF/PIE)          | partial  | rational 32,768 Hz divider; phase separate from calendar counter |
//! | Update-ended interrupt (UF/UIE)       | yes      | drives IRQ8 line               |
//! | Alarm interrupt (AF/AIE)              | yes      | with `0xC0` "don't care" match |
//! | NMI-disable bit (`0x70` bit 7)        | yes      | stored + exposed               |
//! | Register D VRT (valid RAM & time)     | yes      | reads back set                 |
//! | Config NVRAM seed + AT checksum       | yes      | opt-in via [`CmosConfig`]      |
//! | RESET input                          | yes      | clears interrupt enables/flags and SQWE; preserves clock/calendar/RAM |
//!
//! Motorola MC146818 datasheet, "Time, Calendar, and Alarm Locations", Table 3,
//! specifies the valid register ranges. Out-of-range values have undefined
//! behaviour on the chip. The model accepts them, decodes BCD nibbles without
//! validation, and wraps binary increments at 256 before applying the normal
//! carry rules, so guest writes cannot overflow the host's time fields.
//!
//! Deliberately **not** modelled:
//! - **The square-wave output** (register B SQWE / the SQW pin).
//! - **Daylight-savings auto-adjust** (register B DSE): the bit is stored only.
//! - **Oscillator control** beyond the AT's DV=010 (32,768 Hz) time base:
//!   other DV selections suppress periodic events; calendar timing retains the
//!   configured second period. Divider-reset startup delay is not modelled.
//!   There is no board oscillator switching interface in this device.
//!
//! Motorola MC146818A datasheet, Periodic Interrupt Selection, Table 5 and
//! Registers A–C: PF latches independently of PIE; SET stops calendar updates,
//! not the periodic divider. Events round up to whole PIT clocks with carried
//! oscillator phase. Assumption: the selected divider tap's falling edges are
//! aligned to oscillator counts divisible by its period, with phase zero on
//! first entering DV=010. Unsupported DV values freeze this phase; DV=11x
//! clears it. The datasheet does not define a battery-backed power-on phase.
//! ASSUMED approximation: periodic phase and the calendar update counter are
//! separate. SET freezes only the calendar counter; divider reset clears only
//! periodic phase. Updates and periodic edges can therefore lose alignment after
//! either operation. MC146818A, Update Cycle and Figure 15 (Update-Ended and
//! Periodic Interrupt Relationship), relates UIP/UF to PF through the shared
//! divider; that hardware relationship is not reproduced by these counters.
//!
//! Snapshots contain CMOS RAM, calendar, interrupt/UIP state, the subsecond
//! phase, timing ratios and the absolute PIT time base. UIP and second deadlines
//! are derived from that phase (MC146818 datasheet, Update Cycle). Guest-written
//! calendar bytes remain unrestricted, just as on the register interface.

use std::{cell::RefCell, rc::Rc};

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "mc146818-rtc";

/// Index/NMI-control port.
const PORT_INDEX: u16 = 0x70;
/// Data port into the selected CMOS index.
const PORT_DATA: u16 = 0x71;

/// Number of CMOS NVRAM bytes.
const NVRAM_LEN: usize = 64;

/// Snapshot layout version tag.
const SNAPSHOT_VERSION: u8 = 4;

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

// Standard PC/AT configuration NVRAM indices. These sit past the RTC/control
// registers in the general-purpose CMOS storage the BIOS reads at POST.
/// Diagnostic status: nonzero bits flag POST problems (power loss, checksum
/// mismatch, config/memory-size mismatch, bad time). All-clear = 0x00.
const REG_DIAGNOSTIC: usize = 0x0E;
/// Shutdown status byte the BIOS uses to route a reset. Normal boot = 0x00.
const REG_SHUTDOWN: usize = 0x0F;
/// Floppy drive types: high nibble = drive 0, low nibble = drive 1
/// (0 = none, 4 = 1.44 MB 3.5").
const REG_FLOPPY_TYPE: usize = 0x10;
/// Hard-disk type byte (high nibble = drive 0, low = drive 1; 0 = none).
const REG_HDD_TYPE: usize = 0x12;
/// Equipment installed byte (floppy/coprocessor/display/drive-count bits).
const REG_EQUIPMENT: usize = 0x14;
/// Base (conventional) memory size in KB, little-endian low byte.
const REG_BASE_MEM_LO: usize = 0x15;
/// Base memory size high byte.
const REG_BASE_MEM_HI: usize = 0x16;
/// Extended memory size (KB above 1 MB), little-endian low byte.
const REG_EXT_MEM_LO: usize = 0x17;
/// Extended memory size high byte.
const REG_EXT_MEM_HI: usize = 0x18;
/// Configuration checksum, high byte (see [`CHECKSUM_RANGE`]).
const REG_CHECKSUM_HI: usize = 0x2E;
/// Configuration checksum, low byte.
const REG_CHECKSUM_LO: usize = 0x2F;

/// NVRAM indices covered by the standard AT configuration checksum, inclusive.
/// The BIOS sums these bytes into a 16-bit word and compares against the value
/// stored big-endian at 0x2E/0x2F; a mismatch drops POST into the interactive
/// SETUP prompt, so seeding must store the matching sum.
const CHECKSUM_RANGE: std::ops::RangeInclusive<usize> = 0x10..=0x2D;

/// Register A bit 7: update in progress (read-only).
const A_UIP: u8 = 1 << 7;

/// Register B bit 1: 24-hour mode (else 12-hour).
const B_24HOUR: u8 = 1 << 1;
/// Register B bit 2: binary data mode (else BCD).
const B_BINARY: u8 = 1 << 2;
/// Register B bit 3: square-wave output enable.
const B_SQWE: u8 = 1 << 3;
/// Register B bit 4: update-ended interrupt enable.
const B_UIE: u8 = 1 << 4;
/// Register B bit 5: alarm interrupt enable.
const B_AIE: u8 = 1 << 5;
/// Register B bit 6: periodic interrupt enable.
const B_PIE: u8 = 1 << 6;
/// Register B bit 7: SET — freeze the clock for programming.
const B_SET: u8 = 1 << 7;

/// Register C bit 4: update-ended flag.
const C_UF: u8 = 1 << 4;
/// Register C bit 5: alarm flag.
const C_AF: u8 = 1 << 5;
/// Register C bit 6: periodic flag (read-to-clear).
const C_PF: u8 = 1 << 6;
/// Register C bit 7: interrupt request flag (OR of the enabled flags).
const C_IRQF: u8 = 1 << 7;

/// Register D bit 7: valid RAM and time (battery good). Always reads set.
const D_VRT: u8 = 1 << 7;

/// The 12-hour PM indicator in the hours register.
const HOUR_PM: u8 = 0x80;

/// An alarm register value with both top bits set is a "don't care" that matches
/// any time (per the datasheet).
const ALARM_DONT_CARE: u8 = 0xC0;

/// Exact nominal PIT clocks per second, expressed in thirds of a PIT clock.
const DEFAULT_SECOND_TICKS: u64 = 3_579_545;
const DEFAULT_CLOCK_UNITS: u8 = 3;

/// Retain the approximately 1 ms polling window in the same phase units.
const DEFAULT_UIP_TICKS: u64 = DEFAULT_SECOND_TICKS / 1000;

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
            2 if year.is_multiple_of(4) => 29,
            2 => 28,
            _ => 30,
        }
    }

    /// Advance by one second, carrying through the whole date.
    fn advance_one_second(&mut self) {
        self.second = self.second.wrapping_add(1);
        if self.second < 60 {
            return;
        }
        self.second = 0;
        self.minute = self.minute.wrapping_add(1);
        if self.minute < 60 {
            return;
        }
        self.minute = 0;
        self.hour = self.hour.wrapping_add(1);
        if self.hour < 24 {
            return;
        }
        self.hour = 0;
        self.advance_one_day();
    }

    fn advance_one_day(&mut self) {
        self.weekday = if self.weekday >= 7 {
            1
        } else {
            self.weekday + 1
        };
        self.day = self.day.wrapping_add(1);
        if self.day <= Self::days_in_month(self.month, self.year) {
            return;
        }
        self.day = 1;
        self.month = self.month.wrapping_add(1);
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

/// Standard PC/AT configuration seeded into the CMOS NVRAM (§7.6).
///
/// A stock BIOS reads its machine configuration — floppy/HDD types, base and
/// extended memory size, the equipment byte — from CMOS NVRAM and validates a
/// checksum over that area before continuing POST. A blank NVRAM (our power-on
/// state) fails the checksum, and the firmware then stops at an interactive
/// "press F1 to run SETUP" prompt that a headless boot cannot answer. Passing
/// this to [`CmosRtc::with_config`] writes the bytes the BIOS expects plus a
/// matching checksum, so POST proceeds unattended.
///
/// [`CmosConfig::default`] describes a plain AT: one 1.44 MB floppy, no
/// hard disk, EGA/VGA display, 640 KB base memory, no extended memory.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CmosConfig {
    /// Floppy drive type nibbles at index 0x10: high = drive 0, low = drive 1
    /// (0 = absent, 4 = 1.44 MB 3.5"). Default `0x40` — one 1.44 MB drive 0.
    pub floppy_types: u8,
    /// Hard-disk type byte at index 0x12 (0 = none). Default `0x00`.
    pub hard_disk_types: u8,
    /// Equipment installed byte at index 0x14. See [`CmosConfig::default`] for
    /// the bit assignment. Default `0x01`.
    pub equipment: u8,
    /// Base (conventional) memory size in KB. Default 640.
    pub base_memory_kb: u16,
    /// Extended memory size in KB above 1 MB. Default 0 (plain 1 MB machine).
    pub extended_memory_kb: u16,
}

impl Default for CmosConfig {
    fn default() -> Self {
        // Equipment byte (index 0x14), bit by bit:
        //   bit 0    = 1  a floppy/boot diskette drive is installed
        //   bit 1    = 0  no math coprocessor (80287)
        //   bits 2-3 = 00 unused on the AT
        //   bits 4-5 = 00 primary display is EGA/VGA (the adapter BIOS owns the
        //                 mode; 01/10 would select 40/80-col CGA, 11 MDA)
        //   bits 6-7 = 00 one floppy drive installed (01 = two, ...)
        CmosConfig {
            floppy_types: 0x40,
            hard_disk_types: 0x00,
            equipment: 0x01,
            base_memory_kb: 640,
            extended_memory_kb: 0,
        }
    }
}

/// The MC146818 CMOS/RTC device (§7.6).
///
/// Decodes ports `0x70`/`0x71`. Drives **IRQ8** through [`Device::irq_pending`]
/// on the slave PIC, cascaded through master IRQ2.
pub struct CmosRtc {
    /// 64-byte CMOS store. Time registers are shadowed by `time` on access; their
    /// slots here back general reads only for non-time indices.
    nvram: [u8; NVRAM_LEN],
    /// Optional host observer; excluded from hardware snapshots and restored from RAM.
    nvram_store: Option<NvramStore>,
    /// The live broken-down time.
    time: RtcTime,
    /// Currently selected CMOS index (low 6 bits meaningful).
    index: usize,
    /// NMI-disable state from `0x70` bit 7 (`true` = NMI masked).
    nmi_disabled: bool,
    /// Phase units accumulated toward the next second.
    accum: u64,
    /// Phase units that make one RTC second (the PIT-clock ratio numerator).
    second_ticks: u64,
    /// Phase units per PIT clock (the PIT-clock ratio denominator).
    clock_units: u8,
    /// Fractional oscillator clocks, in units of 1 / second_ticks.
    oscillator_phase: u64,
    /// Whole oscillator clocks within the current 32,768-clock divider cycle.
    divider_phase: u16,
    /// Width of the UIP window in phase units before each update.
    uip_ticks: u64,
    /// Whether the update-in-progress window is currently open.
    uip: bool,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

/// Read-only observation of MC146818 general RAM (datasheet, RAM locations
/// 0Eh–3Fh). See persistent-media.md § CMOS NVRAM journals. Comparing snapshots
/// coalesces byte writes without putting host persistence in the device.
#[derive(Clone)]
pub struct NvramStore(Rc<RefCell<[u8; 50]>>);

impl NvramStore {
    /// Current general RAM, excluding every clock, alarm and status register.
    pub fn snapshot(&self) -> [u8; 50] {
        *self.0.borrow()
    }
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
            nvram_store: None,
            time: RtcTime::default_epoch(),
            index: 0,
            nmi_disabled: false,
            accum: 0,
            second_ticks: DEFAULT_SECOND_TICKS,
            clock_units: DEFAULT_CLOCK_UNITS,
            oscillator_phase: 0,
            divider_phase: 0,
            uip_ticks: DEFAULT_UIP_TICKS,
            uip: false,
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// Initialize only general RAM, preserving live clock/control defaults
    /// (MC146818 datasheet, RAM and Registers A–D).
    #[must_use]
    pub fn with_general_nvram(mut self, bytes: [u8; 50]) -> Self {
        self.nvram[14..].copy_from_slice(&bytes);
        self.publish_nvram();
        self
    }

    /// Observe general RAM at host persistence boundaries (MC146818, RAM).
    pub fn nvram_store(&mut self) -> NvramStore {
        if let Some(store) = &self.nvram_store {
            return store.clone();
        }
        let store = NvramStore(Rc::new(RefCell::new(std::array::from_fn(|i| {
            self.nvram[14 + i]
        }))));
        self.nvram_store = Some(store.clone());
        store
    }

    fn publish_nvram(&self) {
        if let Some(store) = &self.nvram_store {
            store.0.borrow_mut().copy_from_slice(&self.nvram[14..]);
        }
    }

    /// Override the virtual-clock timing (ticks per second and the UIP window).
    /// Intended for tests that cannot afford a million ticks per second.
    #[must_use]
    pub fn with_timing(mut self, second_ticks: u64, uip_ticks: u64) -> Self {
        self.second_ticks = second_ticks.max(1);
        self.uip_ticks = uip_ticks.min(self.second_ticks);
        self.clock_units = 1;
        self
    }

    /// Seed the configuration NVRAM with a valid AT layout so a stock BIOS's
    /// POST checksum passes and it does not stop at the SETUP prompt. Opt-in:
    /// [`CmosRtc::new`] leaves the NVRAM blank as a real battery-dead chip would.
    #[must_use]
    pub fn with_config(mut self, config: CmosConfig) -> Self {
        self.seed_config(&config);
        self
    }

    /// Initialize battery-backed bytes without AT checksum repair.
    ///
    /// Motorola MC146818 datasheet, RAM, Time, Calendar, and Alarm Locations,
    /// and Registers A, C and D: UIP and interrupt status are read-only live
    /// state, and D reports valid RAM/time with reserved bits clear. Loading an
    /// image clears these status bits so stale flags cannot inject an IRQ or
    /// produce a snapshot the device cannot restore. Time reads use the
    /// separately seeded live calendar. Register B selects its encoding. See
    /// `backlog/docs/design/machine-config.md` § RTC start and NVRAM source.
    #[must_use]
    pub fn with_nvram(mut self, bytes: [u8; NVRAM_LEN]) -> Self {
        self.nvram = bytes;
        self.nvram[REG_A] &= !A_UIP;
        self.nvram[REG_C] = 0;
        self.refresh_irqf();
        self.nvram[REG_D] = D_VRT;
        self.publish_nvram();
        self
    }

    /// Write the standard configuration bytes and recompute the checksum. The
    /// diagnostic (0x0E) and shutdown (0x0F) status bytes are cleared so the
    /// BIOS sees no prior fault.
    pub fn seed_config(&mut self, config: &CmosConfig) {
        self.nvram[REG_DIAGNOSTIC] = 0x00;
        self.nvram[REG_SHUTDOWN] = 0x00;
        self.nvram[REG_FLOPPY_TYPE] = config.floppy_types;
        self.nvram[REG_HDD_TYPE] = config.hard_disk_types;
        self.nvram[REG_EQUIPMENT] = config.equipment;
        let base = config.base_memory_kb.to_le_bytes();
        self.nvram[REG_BASE_MEM_LO] = base[0];
        self.nvram[REG_BASE_MEM_HI] = base[1];
        let ext = config.extended_memory_kb.to_le_bytes();
        self.nvram[REG_EXT_MEM_LO] = ext[0];
        self.nvram[REG_EXT_MEM_HI] = ext[1];
        self.write_config_checksum();
        self.publish_nvram();
    }

    /// 16-bit sum of the config bytes the BIOS protects ([`CHECKSUM_RANGE`]).
    fn config_checksum(&self) -> u16 {
        CHECKSUM_RANGE
            .map(|i| u16::from(self.nvram[i]))
            .fold(0u16, u16::wrapping_add)
    }

    /// Store the current [`config_checksum`](Self::config_checksum) big-endian
    /// at 0x2E (high) / 0x2F (low), the order the AT BIOS reads it back in.
    fn write_config_checksum(&mut self) {
        let [hi, lo] = self.config_checksum().to_be_bytes();
        self.nvram[REG_CHECKSUM_HI] = hi;
        self.nvram[REG_CHECKSUM_LO] = lo;
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
            REG_A => {
                self.nvram[REG_A] = value & !A_UIP;
                if value & 0x60 == 0x60 {
                    self.oscillator_phase = 0;
                    self.divider_phase = 0;
                }
            }
            REG_B => self.nvram[REG_B] = value,
            // Register C is read-only; writes are ignored.
            REG_C => {}
            // Register D VRT is read-only; store the remaining bits.
            REG_D => self.nvram[REG_D] = (value & !D_VRT) | (self.nvram[REG_D] & D_VRT),
            other => self.nvram[other] = value,
        }
        if self.index >= 14 {
            self.publish_nvram();
        }
    }

    /// Whether the alarm registers match the current time (with don't-cares).
    /// Motorola MC146818 datasheet, "Time, Calendar, and Alarm Locations",
    /// Table 3: the hour alarm uses the same 12/24-hour format and PM bit as
    /// the hour register, in both binary and BCD modes.
    fn alarm_matches(&self) -> bool {
        let field = |alarm_idx: usize, now: u8| {
            let a = self.nvram[alarm_idx];
            a >= ALARM_DONT_CARE || self.decode(a) == now
        };
        field(REG_SECONDS_ALARM, self.time.second)
            && field(REG_MINUTES_ALARM, self.time.minute)
            && (self.nvram[REG_HOURS_ALARM] >= ALARM_DONT_CARE
                || self.decode_hour(self.nvram[REG_HOURS_ALARM]) == self.time.hour)
    }

    /// Bound catch-up even when a restored time base precedes a very large
    /// clock. Calendar and alarm state repeats daily apart from the date; the
    /// two-digit calendar repeats every 36525 days (MC146818, Time and Calendar).
    fn advance_seconds(&mut self, mut seconds: u64) {
        while seconds != 0
            && (self.time.second >= 60 || self.time.minute >= 60 || self.time.hour >= 24)
        {
            self.tick_one_second();
            seconds -= 1;
        }
        let mut days = seconds / 86400;
        if days != 0 {
            let alarm_possible = |index, limit| {
                self.nvram[index] >= ALARM_DONT_CARE || self.decode(self.nvram[index]) < limit
            };
            if alarm_possible(REG_SECONDS_ALARM, 60)
                && alarm_possible(REG_MINUTES_ALARM, 60)
                && (self.nvram[REG_HOURS_ALARM] >= ALARM_DONT_CARE
                    || self.decode_hour(self.nvram[REG_HOURS_ALARM]) < 24)
            {
                self.nvram[REG_C] |= C_AF;
            }
            self.nvram[REG_C] |= C_UF;
            // Guest writes may leave invalid dates. A finite prefix of daily
            // carries normalizes them before whole calendar cycles can be skipped.
            while days != 0
                && (self.time.year >= 100
                    || !(1..=12).contains(&self.time.month)
                    || self.time.day == 0
                    || self.time.day > RtcTime::days_in_month(self.time.month, self.time.year)
                    || !(1..=7).contains(&self.time.weekday))
            {
                self.time.advance_one_day();
                days -= 1;
            }
            let cycles = days / 36525;
            if cycles != 0 {
                self.time.weekday =
                    ((u64::from(self.time.weekday - 1) + (cycles % 7) * 6) % 7 + 1) as u8;
                days %= 36525;
            }
            for _ in 0..days {
                self.time.advance_one_day();
            }
            self.refresh_irqf();
        }
        for _ in 0..seconds % 86400 {
            self.tick_one_second();
        }
    }

    /// Recompute IRQF from all three flag/enable pairs (MC146818A, Register C).
    fn refresh_irqf(&mut self) {
        let b = self.nvram[REG_B];
        let c = self.nvram[REG_C];
        let active = ((c & C_UF != 0) && (b & B_UIE != 0))
            || ((c & C_AF != 0) && (b & B_AIE != 0))
            || ((c & C_PF != 0) && (b & B_PIE != 0));
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

    /// MC146818A datasheet, Table 5, 32.768 kHz time base: RS=1/2
    /// reuse the 256/128 Hz taps; RS=0 disables periodic events.
    fn periodic_divisor(&self) -> Option<u64> {
        if self.nvram[REG_A] & 0x70 != 0x20 {
            return None;
        }
        match self.nvram[REG_A] & 0x0F {
            0 => None,
            1 => Some(128),
            2 => Some(256),
            rate => Some(1 << (rate - 1)),
        }
    }

    fn advance_periodic(&mut self, elapsed: u64) {
        if self.nvram[REG_A] & 0x70 != 0x20 {
            return;
        }
        // u128 holds a full u64 clock jump multiplied by the oscillator rate.
        let phase = u128::from(self.oscillator_phase)
            + u128::from(elapsed) * u128::from(self.clock_units) * 32768;
        let ticks = phase / u128::from(self.second_ticks);
        self.oscillator_phase = (phase % u128::from(self.second_ticks)) as u64;
        if let Some(divisor) = self.periodic_divisor() {
            let until_edge = divisor - u64::from(self.divider_phase) % divisor;
            if ticks >= u128::from(until_edge) {
                self.nvram[REG_C] |= C_PF;
            }
        }
        self.divider_phase = ((u128::from(self.divider_phase) + ticks) % 32768) as u16;
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
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now < self.now {
            return;
        }
        let elapsed = now.0.saturating_sub(self.now.0);
        self.now = now;
        if elapsed == 0 {
            return;
        }
        self.advance_periodic(elapsed);
        // SET inhibits calendar updates, while the periodic divider keeps running.
        if self.nvram[REG_B] & B_SET != 0 {
            self.uip = false;
            return;
        }
        let remaining = self.second_ticks - self.accum;
        let elapsed_units = u128::from(elapsed) * u128::from(self.clock_units);
        if elapsed_units < u128::from(remaining) {
            self.accum += elapsed_units as u64;
        } else {
            // Only second boundaries need division or calendar work. Subtract
            // the partial second first so even a u64-sized jump cannot overflow.
            let after = elapsed_units - u128::from(remaining);
            self.accum = (after % u128::from(self.second_ticks)) as u64;
            self.advance_seconds(1 + (after / u128::from(self.second_ticks)) as u64);
        }
        self.uip = self.accum >= self.second_ticks - self.uip_ticks;
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        let calendar_clocks = if self.nvram[REG_B] & B_SET != 0 {
            if !self.uip {
                u64::MAX
            } else {
                // SET clears a pre-existing UIP on the next clock, as in the
                // per-clock model, rather than during a zero-elapsed access.
                1
            }
        } else {
            let uip_start = self.second_ticks - self.uip_ticks;
            if self.uip != (self.accum >= uip_start) {
                1
            } else if !self.uip && self.uip_ticks != 0 {
                (uip_start - self.accum).div_ceil(u64::from(self.clock_units))
            } else {
                (self.second_ticks - self.accum).div_ceil(u64::from(self.clock_units))
            }
        };
        // Once PF is latched, further edges cannot change any observable flag
        // until register C is read. Access synchronization still carries phase.
        let periodic_clocks = self
            .periodic_divisor()
            .filter(|_| self.nvram[REG_C] & C_PF == 0)
            .map(|divisor| {
                let ticks = divisor - u64::from(self.divider_phase) % divisor;
                let numerator = u128::from(ticks) * u128::from(self.second_ticks)
                    - u128::from(self.oscillator_phase);
                u64::try_from(numerator.div_ceil(32768 * u128::from(self.clock_units)))
                    .unwrap_or(u64::MAX)
            })
            .unwrap_or(u64::MAX);
        let clocks = calendar_clocks.min(periodic_clocks);
        if clocks == u64::MAX {
            return NextEvent::Dormant;
        }
        self.now
            .0
            .checked_add(clocks)
            .map_or(NextEvent::Dormant, |at| NextEvent::At(VClock(at)))
    }

    fn irq_pending(&self) -> bool {
        // IRQ8 is asserted whenever register C's IRQF is set; the machine wires
        // this output onto the slave PIC's IRQ8 input.
        self.nvram[REG_C] & C_IRQF != 0
    }

    /// Motorola MC146818 datasheet, "RESET — RESET, INPUT": RESET leaves
    /// clock, calendar and RAM running, clearing only the interrupt enables,
    /// interrupt flags and square-wave enable. Retaining the virtual time base
    /// and update phase prevents an extra or missed second after reset.
    fn reset(&mut self) {
        self.trace.clear();
        self.nvram[REG_B] &= !(B_PIE | B_AIE | B_UIE | B_SQWE);
        self.nvram[REG_C] &= !(C_IRQF | C_PF | C_AF | C_UF);
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(1 + NVRAM_LEN + 7 + 1 + 1 + 8 * 4 + 11);
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
        blob.extend_from_slice(&self.oscillator_phase.to_le_bytes());
        blob.extend_from_slice(&self.divider_phase.to_le_bytes());
        blob.push(self.clock_units);
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        const LEN: usize = 1 + NVRAM_LEN + 7 + 1 + 1 + 32 + 11;
        if snapshot
            .first()
            .is_some_and(|&version| version != SNAPSHOT_VERSION)
        {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        if snapshot.len() != LEN {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unexpected snapshot length",
            });
        }
        let read_timing = |off| u64::from_le_bytes(std::array::from_fn(|i| snapshot[off + i]));
        let timing = 1 + NVRAM_LEN + 7 + 2;
        let accum = read_timing(timing);
        let second_ticks = read_timing(timing + 8);
        let uip_ticks = read_timing(timing + 16);
        let oscillator_phase = read_timing(timing + 24);
        let divider_phase = u16::from_le_bytes([snapshot[timing + 32], snapshot[timing + 33]]);
        let clock_units = snapshot[timing + 34];
        if second_ticks == 0
            || !matches!(clock_units, 1 | 3)
            || second_ticks < u64::from(clock_units)
            || oscillator_phase >= second_ticks
            || divider_phase >= 32768
            || accum >= second_ticks
            || uip_ticks > second_ticks
            || snapshot[1 + NVRAM_LEN + 7] >= NVRAM_LEN as u8
            || snapshot[1 + NVRAM_LEN + 8] & !3 != 0
            || snapshot[1 + REG_A] & A_UIP != 0
            || snapshot[1 + REG_C] & 0x0F != 0
        {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid clock state",
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
        self.clock_units = clock_units;
        self.oscillator_phase = oscillator_phase;
        self.divider_phase = divider_phase;
        self.now = VClock(read_u64(off + 35));
        self.publish_nvram();
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
    fn nvram_observer_initialization_and_restore_publish_on_mock_bus() {
        for source in 0..4 {
            let mut rtc = CmosRtc::new();
            let store = rtc.nvram_store();
            match source {
                0 => rtc = rtc.with_nvram([0xA5; 64]),
                1 => rtc.seed_config(&CmosConfig::default()),
                2 => rtc = rtc.with_general_nvram([0x5A; 50]),
                _ => {}
            }
            let mut bus = MockBus::new(Box::new(rtc));
            if source == 3 {
                let snapshot = CmosRtc::new().with_general_nvram([0x67; 50]).save();
                bus.device_mut().restore(&snapshot).unwrap();
            }
            let expected: [u8; 50] = std::array::from_fn(|i| bus_read_reg(&mut bus, i + 14));
            assert_ne!(expected, [0; 50]);
            assert_eq!(store.snapshot(), expected, "publish path {source}");
        }
    }

    #[test]
    fn nvram_observer_tracks_only_general_ram_on_mock_bus() {
        let mut rtc = CmosRtc::new().with_general_nvram([0xAA; 50]);
        let store = rtc.nvram_store();
        let mut bus = MockBus::new(Box::new(rtc));
        assert_eq!(store.snapshot(), [0xAA; 50]);
        for index in 0..14 {
            bus_write_reg(&mut bus, index, 0x12);
            assert_eq!(store.snapshot(), [0xAA; 50], "clock/control {index:#x}");
        }
        for index in 14..64 {
            let before = store.snapshot();
            bus_write_reg(&mut bus, index, index as u8);
            assert_ne!(store.snapshot(), before, "general RAM {index:#x}");
            assert_eq!(store.snapshot()[index - 14], index as u8);
            assert_eq!(bus_read_reg(&mut bus, index), index as u8);
        }
        bus.tick(10_000);
        assert_eq!(store.snapshot(), std::array::from_fn(|i| (i + 14) as u8));
        bus.device_mut().reset();
        assert_eq!(store.snapshot(), std::array::from_fn(|i| (i + 14) as u8));
    }

    #[test]
    fn exact_default_rate_has_no_long_term_drift_in_uneven_chunks() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new()));
        bus_write_reg(&mut bus, REG_B, B_24HOUR | B_BINARY);
        let total = 3_579_545 * 1_000_000;
        let chunks = [1, 100_000_000_003, 3_579_544, 234_567_891_011, 17];
        let mut step = 0;
        while bus.clock().0 < total {
            bus.tick(chunks[step % chunks.len()].min(total - bus.clock().0));
            step += 1;
            let seconds = bus.clock().0 * 3 / 3_579_545;
            let days = seconds / 86400;
            let (month, day) = if days < 31 {
                (1, days + 1)
            } else {
                (2, days - 30)
            };
            for (register, expected) in [
                (REG_SECONDS, seconds % 60),
                (REG_MINUTES, seconds / 60 % 60),
                (REG_HOURS, seconds / 3600 % 24),
                (REG_WEEKDAY, (6 + days) % 7 + 1),
                (REG_DAY, day),
                (REG_MONTH, month),
                (REG_YEAR, 0),
            ] {
                assert_eq!(
                    u64::from(bus_read_reg(&mut bus, register)),
                    expected,
                    "register {register:#x} at PIT clock {}",
                    bus.clock().0
                );
            }
        }
        assert_eq!(bus.clock(), VClock(3_579_545_000_000));
    }

    #[test]
    fn exact_default_rate_first_three_updates_keep_fractional_phase() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new()));
        for (second, deadline) in [(1, 1_193_182), (2, 2_386_364), (3, 3_579_545)] {
            bus.tick(deadline - 1 - bus.clock().0);
            assert_eq!(bus_read_reg(&mut bus, REG_SECONDS), second - 1);
            assert_eq!(
                bus.device().next_event(bus.clock()),
                NextEvent::At(VClock(deadline))
            );
            bus_read_reg(&mut bus, REG_C);
            bus.tick(1);
            assert_eq!(bus_read_reg(&mut bus, REG_SECONDS), second);
            assert_ne!(bus_read_reg(&mut bus, REG_C) & C_UF, 0);
        }
    }

    #[test]
    fn exact_default_rate_uip_window_rounds_up_with_carried_phase() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new()));
        for (start, update) in [
            (1_191_989, 1_193_182),
            (2_385_171, 2_386_364),
            (3_578_352, 3_579_545),
        ] {
            assert_eq!(
                bus.device().next_event(bus.clock()),
                NextEvent::At(VClock(start))
            );
            bus.tick(start - 1 - bus.clock().0);
            assert_eq!(bus_read_reg(&mut bus, REG_A) & A_UIP, 0);
            bus.tick(1);
            assert_eq!(bus_read_reg(&mut bus, REG_A) & A_UIP, A_UIP);
            assert_eq!(
                bus.device().next_event(bus.clock()),
                NextEvent::At(VClock(update))
            );
            bus.tick(update - 1 - bus.clock().0);
            assert_eq!(bus_read_reg(&mut bus, REG_A) & A_UIP, A_UIP);
            bus.tick(1);
            assert_eq!(bus_read_reg(&mut bus, REG_A) & A_UIP, 0);
        }
    }

    #[test]
    fn explicit_integer_timing_matches_per_clock_reference() {
        let mut per_clock = crate::trace_policy_tests::traced(CmosRtc::new()).with_timing(64, 8);
        let mut bus = MockBus::new(Box::new(
            crate::trace_policy_tests::traced(CmosRtc::new()).with_timing(64, 8),
        ));
        for (register, value) in [(REG_A, 0x26), (REG_B, B_24HOUR | B_UIE | B_PIE)] {
            write_reg(&mut per_clock, register as u8, value);
            bus_write_reg(&mut bus, register, value);
        }
        for clock in 1..=512 {
            reference_clock(&mut per_clock, clock);
            bus.tick(1);
            for register in 0..NVRAM_LEN {
                let actual = bus_read_reg(&mut bus, register);
                assert_eq!(
                    actual,
                    read_reg(&mut per_clock, register as u8),
                    "register {register:#x} at PIT clock {clock}"
                );
                match register {
                    REG_SECONDS => assert_eq!(actual, to_bcd((clock / 64) as u8)),
                    REG_A => assert_eq!(actual, 0x26 | if clock % 64 >= 56 { A_UIP } else { 0 }),
                    REG_C => assert_eq!(
                        actual,
                        C_PF | C_IRQF | if clock % 64 == 0 { C_UF } else { 0 }
                    ),
                    _ => {}
                }
            }
            // Reading C above clears PF; at 1024 Hz every next clock has an edge.
            assert_eq!(
                bus.device().next_event(bus.clock()),
                NextEvent::At(VClock(clock + 1))
            );
            assert_eq!(bus.device().save(), per_clock.save());
            assert_eq!(bus.device_mut().take_trace(), per_clock.take_trace());
        }
    }

    #[test]
    fn initialization_nvram_image_preserves_writable_bytes_and_clears_status() {
        let image = std::array::from_fn(|index| (index as u8).wrapping_mul(37));
        let rtc = CmosRtc::new().with_nvram(image);
        for (index, byte) in image.into_iter().enumerate() {
            let expected = match index {
                REG_A => byte & !A_UIP,
                REG_C => 0,
                REG_D => D_VRT,
                _ => byte,
            };
            assert_eq!(rtc.nvram[index], expected, "backing byte {index:#x}");
        }
        let mut bus = MockBus::new(Box::new(rtc));
        for index in (0x0E..64).chain([1, 3, 5, REG_B]) {
            assert_eq!(
                bus_read_reg(&mut bus, index),
                image[index],
                "register {index:#x}"
            );
        }
        assert_eq!(bus_read_reg(&mut bus, REG_C), 0);
        assert_eq!(bus_read_reg(&mut bus, REG_A), image[REG_A] & !A_UIP);
        assert_eq!(bus_read_reg(&mut bus, REG_D), D_VRT);
        bus_write_reg(&mut bus, 0x20, 0xAB);
        assert_eq!(bus_read_reg(&mut bus, 0x20), 0xAB);
    }

    #[test]
    fn initialization_hostile_nvram_image_round_trips_snapshot() {
        for (a, c, d) in [(0xA6, 0, 0x80), (0x26, 0x0F, 0x80), (0xFF, 0xFF, 0x7F)] {
            let mut image = [0xFF; 64];
            image[REG_A] = a;
            image[REG_B] = B_24HOUR;
            image[REG_C] = c;
            image[REG_D] = d;
            let bus = MockBus::new(Box::new(CmosRtc::new().with_nvram(image)));
            let snapshot = bus.device().save();
            let mut restored = MockBus::new(Box::new(CmosRtc::new()));
            restored.device_mut().restore(&snapshot).unwrap();
            assert_eq!(restored.device().save(), snapshot);
            assert_eq!(bus_read_reg(&mut restored, REG_A), a & !A_UIP);
            assert_eq!(bus_read_reg(&mut restored, REG_C), 0);
            assert_eq!(bus_read_reg(&mut restored, REG_D), D_VRT);
        }
    }

    #[test]
    fn initialization_nvram_image_has_no_power_on_irq() {
        for enables in [0, B_UIE | B_AIE | B_PIE] {
            let mut image = [0; 64];
            image[REG_B] = B_24HOUR | enables;
            image[REG_C] = C_IRQF | C_PF | C_AF | C_UF;
            let mut bus = MockBus::new(Box::new(CmosRtc::new().with_nvram(image)));
            assert!(!bus.device().irq_pending());
            assert_eq!(bus_read_reg(&mut bus, REG_C), 0);
            bus.tick(0);
            assert!(!bus.device().irq_pending());
        }
    }

    #[test]
    fn fidelity_periodic_deadlines_round_up_without_drift() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new()));
        bus_write_reg(&mut bus, REG_A, 0x26);
        bus_write_reg(&mut bus, REG_B, B_24HOUR | B_PIE);
        for period in 1u64..=1024 {
            let expected =
                (period * DEFAULT_SECOND_TICKS).div_ceil(1024 * u64::from(DEFAULT_CLOCK_UNITS));
            assert_eq!(
                bus.device().next_event(bus.clock()),
                NextEvent::At(VClock(expected))
            );
            bus.tick(expected - bus.clock().0);
            assert!(bus.device().irq_pending());
            assert_eq!(
                bus_read_reg(&mut bus, REG_C) & (C_PF | C_IRQF),
                C_PF | C_IRQF
            );
            assert!(!bus.device().irq_pending());
            // UIP is an independent earlier deadline near the second boundary.
            if period == 1022 {
                bus.tick(
                    (DEFAULT_SECOND_TICKS - DEFAULT_UIP_TICKS)
                        .div_ceil(u64::from(DEFAULT_CLOCK_UNITS))
                        - bus.clock().0,
                );
            }
        }
    }

    #[test]
    fn fidelity_periodic_flag_is_independent_of_pie_and_set() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new()));
        bus_write_reg(&mut bus, REG_A, 0x26);
        bus_write_reg(&mut bus, REG_B, B_24HOUR | B_SET);
        bus.tick(DEFAULT_SECOND_TICKS.div_ceil(1024 * u64::from(DEFAULT_CLOCK_UNITS)));
        assert!(!bus.device().irq_pending());
        bus_write_reg(&mut bus, REG_B, B_24HOUR | B_SET | B_PIE);
        assert!(bus.device().irq_pending());
        assert_eq!(bus_read_reg(&mut bus, REG_C), C_PF | C_IRQF);
        assert_eq!(bus_read_reg(&mut bus, REG_SECONDS), 0);
    }

    #[test]
    fn fidelity_periodic_rate_disable_divider_reset_and_device_reset() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new()));
        bus_write_reg(&mut bus, REG_A, 0x26);
        bus_write_reg(&mut bus, REG_B, B_SET | B_PIE);
        bus.tick(1000);
        bus.device_mut().reset();
        bus_write_reg(&mut bus, REG_B, B_SET | B_PIE);
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(1166))
        );
        bus.tick(166);
        assert!(bus.device().irq_pending());
        bus_read_reg(&mut bus, REG_C);
        bus_write_reg(&mut bus, REG_A, 0x20);
        assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
        bus.tick(100);
        assert_eq!(bus_read_reg(&mut bus, REG_C), 0);
        bus_write_reg(&mut bus, REG_A, 0x66);
        bus.tick(100);
        assert_eq!(bus_read_reg(&mut bus, REG_C), 0);
        bus_write_reg(&mut bus, REG_A, 0x26);
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(bus.clock().0 + 1166))
        );
    }

    #[test]
    fn fidelity_periodic_rates_and_snapshot_phase() {
        for (rs, hz) in [(1, 256u64), (2, 128), (3, 8192), (6, 1024), (15, 2)] {
            let mut bus = MockBus::new(Box::new(CmosRtc::new()));
            bus_write_reg(&mut bus, REG_A, 0x20 | rs);
            bus_write_reg(&mut bus, REG_B, B_24HOUR | B_PIE);
            let deadline = DEFAULT_SECOND_TICKS.div_ceil(hz * u64::from(DEFAULT_CLOCK_UNITS));
            bus.tick(deadline - 1);
            assert_eq!(bus_read_reg(&mut bus, REG_C) & C_PF, 0);
            let mut copy = CmosRtc::new();
            copy.restore(&bus.device().save()).unwrap();
            bus.tick(1);
            copy.sync(VClock(deadline));
            assert_eq!(copy.save(), bus.device().save());
            assert!(copy.irq_pending());
            assert_eq!(bus_read_reg(&mut bus, REG_C) & C_PF, C_PF);
        }
    }

    #[test]
    fn bounded_calendar_catch_up_matches_second_steps_and_guest_written_dates() {
        for (second, minute, hour, day, month, year) in [
            (58, 59, 23, 28, 2, 0),
            (58, 59, 23, 31, 12, 99),
            (255, 255, 255, 255, 255, 255),
            (99, 99, 99, 0, 0, 200),
        ] {
            for alarm in [0, 0xC0, 0xFF, 0x59] {
                let mut fast = CmosRtc::new().with_timing(1, 0);
                fast.set_time(second, minute, hour, 255, day, month, year);
                fast.nvram[REG_SECONDS_ALARM] = alarm;
                fast.nvram[REG_B] |= B_UIE | B_AIE;
                let mut stepped = CmosRtc::new();
                stepped.restore(&fast.save()).unwrap();
                let seconds = 3 * 86400 + 123;
                fast.advance_seconds(seconds);
                for _ in 0..seconds {
                    stepped.tick_one_second();
                }
                assert_eq!(fast.save(), stepped.save());
            }
        }
    }

    #[test]
    fn restored_rtc_handles_maximum_clock_jump_without_unbounded_second_loop() {
        let source = CmosRtc::new().with_timing(1, 0);
        let mut restored = CmosRtc::new();
        restored.restore(&source.save()).unwrap();
        restored.sync(VClock(u64::MAX));
        let total_days = u64::MAX / 86400;
        let mut days = total_days % 36525;
        let mut year = 0;
        while days >= if year % 4 == 0 { 366 } else { 365 } {
            days -= if year % 4 == 0 { 366 } else { 365 };
            year += 1;
        }
        let mut month = 1;
        while days >= u64::from(RtcTime::days_in_month(month, year)) {
            days -= u64::from(RtcTime::days_in_month(month, year));
            month += 1;
        }
        assert_eq!(
            restored.time,
            RtcTime {
                second: (u64::MAX % 60) as u8,
                minute: (u64::MAX / 60 % 60) as u8,
                hour: (u64::MAX / 3600 % 24) as u8,
                weekday: ((6 + total_days % 7) % 7 + 1) as u8,
                day: days as u8 + 1,
                month,
                year,
            }
        );
        assert_eq!(restored.now, VClock(u64::MAX));
        assert_eq!(restored.accum, 0);
        assert_eq!(restored.nvram[REG_C], C_UF | C_AF);
    }

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

    fn bus_read_reg(bus: &mut MockBus, index: usize) -> u8 {
        bus.port_write(PORT_INDEX, 1, index as u32).unwrap();
        bus.port_read(PORT_DATA, 1).unwrap() as u8
    }

    fn bus_write_reg(bus: &mut MockBus, index: usize, value: u8) {
        bus.port_write(PORT_INDEX, 1, index as u32).unwrap();
        bus.port_write(PORT_DATA, 1, u32::from(value)).unwrap();
    }

    fn invalid_time_writes_do_not_panic(binary: bool) {
        let mut panicked = Vec::new();
        for index in [
            REG_SECONDS,
            REG_MINUTES,
            REG_HOURS,
            REG_WEEKDAY,
            REG_DAY,
            REG_MONTH,
            REG_YEAR,
        ] {
            let result = std::panic::catch_unwind(|| {
                let mut rtc = CmosRtc::new().with_timing(10, 2);
                rtc.set_time(59, 59, 23, 7, 31, 12, 99);
                let mut bus = MockBus::new(Box::new(rtc));
                let mode = B_24HOUR | if binary { B_BINARY } else { 0 };
                bus_write_reg(&mut bus, REG_B, mode);
                bus_write_reg(&mut bus, index, 0xFF);
                bus.tick(10);
                // Exercise guest reads too, including BCD encoding of invalid data.
                for register in 0..=REG_YEAR {
                    bus_read_reg(&mut bus, register);
                }
            });
            if result.is_err() {
                panicked.push(index);
            }
        }
        assert!(
            panicked.is_empty(),
            "panicked for time registers {panicked:?}"
        );
    }

    fn reference_clock(rtc: &mut CmosRtc, clock: u64) {
        rtc.now = VClock(clock);
        rtc.advance_periodic(1);
        if rtc.nvram[REG_B] & B_SET != 0 {
            rtc.uip = false;
            return;
        }
        rtc.accum += 1;
        if rtc.accum >= rtc.second_ticks {
            rtc.accum -= rtc.second_ticks;
            rtc.tick_one_second();
        }
        rtc.uip = rtc.accum >= rtc.second_ticks - rtc.uip_ticks;
    }

    #[test]
    fn scheduled_matches_per_clock_seeded_accesses_and_long_jumps() {
        for (second, window) in [(1, 0), (1, 1), (10, 0), (10, 2), (10, 10), (127, 19)] {
            let mut reference =
                crate::trace_policy_tests::traced(CmosRtc::new()).with_timing(second, window);
            reference.set_time(58, 59, 23, 7, 28, 2, 0);
            let mut scheduled = MockBus::new(Box::new(
                crate::trace_policy_tests::traced(CmosRtc::new()).with_timing(second, window),
            ));
            scheduled.device_mut().restore(&reference.save()).unwrap();
            let mut seed = 0x8042_1468_1816_5501u64;
            let mut expected_edges = Vec::new();
            let mut actual_edges = Vec::new();
            let mut expected_level = false;
            let mut actual_level = false;
            let mut pin_sample = u64::MAX;
            for step in 0..128 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let jump = if step % 31 == 0 {
                    10_003
                } else {
                    (seed >> 32) % (second * 2 + 1)
                };
                let access = scheduled.clock().0 + jump;
                loop {
                    let due = match scheduled.device().next_event(scheduled.clock()) {
                        device::NextEvent::At(at) => {
                            assert!(at > scheduled.clock());
                            at.0
                        }
                        device::NextEvent::Dormant => u64::MAX,
                        device::NextEvent::EveryPitClock => panic!("RTC still polls every clock"),
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
                    assert_eq!(
                        reference.save(),
                        scheduled.device().save(),
                        "timing {second}/{window}, clock {target}"
                    );
                    assert_eq!(reference.irq_pending(), scheduled.device().irq_pending());
                    if target > previous_clock && scheduled.device().irq_pending() != actual_level {
                        actual_level = scheduled.device().irq_pending();
                        actual_edges.push((target, actual_level));
                    }
                    if target == pin_sample {
                        pin_sample = u64::MAX;
                    }
                    if target == access {
                        break;
                    }
                }
                scheduled.device_mut().sync(VClock(access));
                let (index, value) = match step % 8 {
                    0 => (
                        REG_B,
                        B_UIE
                            | B_AIE
                            | if seed & 0x100 != 0 {
                                B_BINARY | B_24HOUR
                            } else {
                                0
                            },
                    ),
                    1 => (
                        REG_SECONDS_ALARM,
                        if seed & 0x100 != 0 { 0xC0 } else { seed as u8 },
                    ),
                    2 => (REG_MINUTES_ALARM, 0xC0),
                    3 => (REG_HOURS_ALARM, 0xC0),
                    4 => (REG_B, B_SET | B_UIE | B_AIE | B_24HOUR),
                    5 => (REG_SECONDS, seed as u8),
                    6 => (REG_A, seed as u8),
                    _ => (0x10 + (seed >> 8) as usize % 32, seed as u8),
                };
                write_reg(&mut reference, index as u8, value);
                bus_write_reg(&mut scheduled, index, value);
                for index in [
                    REG_A,
                    REG_SECONDS,
                    REG_MINUTES,
                    REG_HOURS,
                    REG_DAY,
                    REG_MONTH,
                    REG_YEAR,
                    REG_C,
                ] {
                    assert_eq!(
                        read_reg(&mut reference, index as u8),
                        bus_read_reg(&mut scheduled, index)
                    );
                    assert_eq!(reference.irq_pending(), scheduled.device().irq_pending());
                }
                if step % 17 == 0 {
                    reference.reset();
                    scheduled.device_mut().reset();
                }
                if step % 19 == 0 {
                    let snapshot = reference.save();
                    reference.restore(&snapshot).unwrap();
                    scheduled.device_mut().restore(&snapshot).unwrap();
                }
                assert_eq!(reference.save(), scheduled.device().save());
                assert_eq!(reference.take_trace(), scheduled.device_mut().take_trace());
                pin_sample = access + 1;
            }
            assert_eq!(expected_edges, actual_edges);
            assert!(expected_edges.iter().any(|(_, level)| *level));
        }
    }

    #[test]
    fn deadlines_follow_uip_update_and_set_without_zero_elapsed_changes() {
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        assert_eq!(
            rtc.next_event(VClock::ZERO),
            device::NextEvent::At(VClock(8))
        );
        rtc.tick(VClock(8));
        assert!(rtc.uip);
        assert_eq!(rtc.next_event(VClock(8)), device::NextEvent::At(VClock(10)));
        write_reg(&mut rtc, REG_B as u8, B_SET);
        rtc.sync(VClock(8));
        assert!(rtc.uip);
        assert_eq!(rtc.next_event(VClock(8)), device::NextEvent::At(VClock(9)));
        rtc.tick(VClock(9));
        assert!(!rtc.uip);
        assert_eq!(rtc.next_event(VClock(9)), device::NextEvent::Dormant);
        rtc.sync(VClock(100));
        write_reg(&mut rtc, REG_B as u8, B_UIE);
        assert_eq!(
            rtc.next_event(VClock(100)),
            device::NextEvent::At(VClock(101))
        );
        rtc.tick(VClock(101));
        assert!(rtc.uip);
        rtc.tick(VClock(102));
        assert!(rtc.irq_pending());
        assert_eq!(rtc.time.second, 1);
        assert_eq!(read_reg(&mut rtc, REG_C as u8), C_UF | C_IRQF);
        assert!(!rtc.irq_pending());
    }

    #[test]
    fn catch_up_without_deadline_visits_matches_per_clock() {
        for (second, window) in [(1, 1), (10, 2), (127, 0), (127, 127)] {
            let mut reference = CmosRtc::new().with_timing(second, window);
            let mut scheduled = MockBus::new(Box::new(CmosRtc::new().with_timing(second, window)));
            for index in [REG_SECONDS_ALARM, REG_MINUTES_ALARM, REG_HOURS_ALARM] {
                write_reg(&mut reference, index as u8, 0xC0);
                bus_write_reg(&mut scheduled, index, 0xC0);
            }
            write_reg(&mut reference, REG_B as u8, B_UIE | B_AIE | B_24HOUR);
            bus_write_reg(&mut scheduled, REG_B, B_UIE | B_AIE | B_24HOUR);
            for target in [0, 1, 1, 2, 128, 10_003, 200_003] {
                for clock in reference.now.0 + 1..=target {
                    reference_clock(&mut reference, clock);
                }
                scheduled.device_mut().sync(VClock(target));
                assert_eq!(reference.save(), scheduled.device().save());
                assert_eq!(reference.irq_pending(), scheduled.device().irq_pending());
            }
        }
    }

    #[test]
    fn invalid_binary_time_writes_do_not_panic() {
        invalid_time_writes_do_not_panic(true);
    }

    #[test]
    fn invalid_bcd_time_writes_do_not_panic() {
        invalid_time_writes_do_not_panic(false);
    }

    #[test]
    fn reset_preserves_battery_backed_registers() {
        let mut rtc = CmosRtc::new()
            .with_timing(10, 2)
            .with_config(CmosConfig::default());
        rtc.set_time(5, 6, 17, 4, 25, 12, 24);
        let mut bus = MockBus::new(Box::new(rtc));
        bus_write_reg(&mut bus, REG_B, B_BINARY | B_UIE);
        bus_write_reg(&mut bus, REG_SECONDS_ALARM, 0xC0);
        bus_write_reg(&mut bus, REG_MINUTES_ALARM, 42);
        bus_write_reg(&mut bus, REG_HOURS_ALARM, HOUR_PM | 5);
        bus_write_reg(&mut bus, REG_SHUTDOWN, 0x0A);
        bus_write_reg(&mut bus, 0x30, 0xAB);
        bus_write_reg(&mut bus, REG_A, 0x26);
        bus.tick(10);
        assert!(bus.device().irq_pending());
        let indices: Vec<_> = (0..=REG_A).chain(REG_D..NVRAM_LEN).collect();
        let before: Vec<_> = indices
            .iter()
            .map(|&index| bus_read_reg(&mut bus, index))
            .collect();
        bus.device_mut().reset();
        for (index, expected) in indices.into_iter().zip(before) {
            assert_eq!(
                bus_read_reg(&mut bus, index),
                expected,
                "register {index:#x}"
            );
        }
    }

    #[test]
    fn reset_clears_only_interrupt_enables_square_wave_and_flags() {
        for control in 0..=u8::MAX {
            let mut rtc = CmosRtc::new();
            rtc.nvram[REG_B] = control;
            // PF is not generated by the model, but RESET must clear it too.
            rtc.nvram[REG_C] = C_IRQF | C_PF | C_AF | C_UF;
            let mut bus = MockBus::new(Box::new(rtc));
            bus.device_mut().reset();
            assert_eq!(bus_read_reg(&mut bus, REG_B), control & !0x78);
            assert_eq!(bus_read_reg(&mut bus, REG_C), 0);
            assert!(!bus.device().irq_pending());
        }
    }

    #[test]
    fn reset_preserves_clock_base_and_update_phase() {
        let mut bus = MockBus::new(Box::new(CmosRtc::new().with_timing(10, 2)));
        bus.tick(29);
        assert_eq!(bus_read_reg(&mut bus, REG_SECONDS), 0x02);
        assert_ne!(bus_read_reg(&mut bus, REG_A) & A_UIP, 0);
        bus.device_mut().reset();
        assert_ne!(bus_read_reg(&mut bus, REG_A) & A_UIP, 0);
        bus.tick(0);
        assert_eq!(bus_read_reg(&mut bus, REG_SECONDS), 0x02);
        bus.tick(1);
        assert_eq!(bus_read_reg(&mut bus, REG_SECONDS), 0x03);
        assert_eq!(bus_read_reg(&mut bus, REG_A) & A_UIP, 0);
        assert_ne!(bus_read_reg(&mut bus, REG_C) & C_UF, 0);
        assert!(!bus.device().irq_pending());
    }

    fn twelve_hour_alarms_match_am_and_pm(binary: bool) {
        for hour in (13..24).chain(0..13) {
            for alarm_hour in 0..24 {
                let mut rtc = CmosRtc::new().with_timing(10, 2);
                rtc.set_time(0, 0, hour, 1, 1, 1, 0);
                let mut bus = MockBus::new(Box::new(rtc));
                bus_write_reg(&mut bus, REG_B, B_AIE | if binary { B_BINARY } else { 0 });
                bus_write_reg(&mut bus, REG_SECONDS_ALARM, 1);
                bus_write_reg(&mut bus, REG_MINUTES_ALARM, 0);
                let twelve = if alarm_hour % 12 == 0 {
                    12
                } else {
                    alarm_hour % 12
                };
                let encoded = if binary { twelve } else { to_bcd(twelve) };
                let pm = if alarm_hour >= 12 { HOUR_PM } else { 0 };
                bus_write_reg(&mut bus, REG_HOURS_ALARM, encoded | pm);
                bus.tick(10);
                let matches = hour == alarm_hour;
                assert_eq!(bus.device().irq_pending(), matches, "{hour}, {alarm_hour}");
                assert_eq!(bus_read_reg(&mut bus, REG_C) & C_AF != 0, matches);
            }
        }
    }

    #[test]
    fn twelve_hour_bcd_alarms_match_am_and_pm() {
        twelve_hour_alarms_match_am_and_pm(false);
    }

    #[test]
    fn twelve_hour_binary_alarms_match_am_and_pm() {
        twelve_hour_alarms_match_am_and_pm(true);
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
    fn periodic_flag_latches_without_interrupt_enable() {
        // PF latches even when PIE masks the interrupt output.
        let mut rtc = CmosRtc::new().with_timing(10, 2);
        write_reg(&mut rtc, REG_A as u8, 0x26); // a periodic rate
        write_reg(&mut rtc, REG_B as u8, B_24HOUR);
        rtc.tick(VClock(1000));
        assert_eq!(read_reg(&mut rtc, REG_C as u8) & C_PF, C_PF);
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

    #[test]
    fn seeded_config_reads_back_through_ports() {
        let mut rtc = CmosRtc::new().with_config(CmosConfig::default());
        // The BIOS reads these through the 0x70/0x71 index/data protocol.
        assert_eq!(read_reg(&mut rtc, REG_DIAGNOSTIC as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_SHUTDOWN as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_FLOPPY_TYPE as u8), 0x40);
        assert_eq!(read_reg(&mut rtc, REG_HDD_TYPE as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_EQUIPMENT as u8), 0x01);
        assert_eq!(read_reg(&mut rtc, REG_BASE_MEM_LO as u8), 0x80);
        assert_eq!(read_reg(&mut rtc, REG_BASE_MEM_HI as u8), 0x02); // 640 KB
        assert_eq!(read_reg(&mut rtc, REG_EXT_MEM_LO as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_EXT_MEM_HI as u8), 0x00);
    }

    #[test]
    fn seeded_checksum_matches_config_bytes() {
        let mut rtc = CmosRtc::new().with_config(CmosConfig::default());
        // Independently recompute the BIOS check: sum of 0x10..=0x2D, compared
        // against the big-endian word stored at 0x2E/0x2F.
        let sum: u16 = (0x10u8..=0x2D)
            .map(|i| u16::from(read_reg(&mut rtc, i)))
            .fold(0u16, u16::wrapping_add);
        let stored = (u16::from(read_reg(&mut rtc, REG_CHECKSUM_HI as u8)) << 8)
            | u16::from(read_reg(&mut rtc, REG_CHECKSUM_LO as u8));
        assert_eq!(stored, sum, "stored checksum must equal the config sum");
    }

    #[test]
    fn custom_config_reseeds_checksum() {
        let config = CmosConfig {
            floppy_types: 0x24, // two drives: 1.2 MB (2) and 1.44 MB (4)
            extended_memory_kb: 15 * 1024,
            ..CmosConfig::default()
        };
        let mut rtc = CmosRtc::new().with_config(config);
        let sum: u16 = (0x10u8..=0x2D)
            .map(|i| u16::from(read_reg(&mut rtc, i)))
            .fold(0u16, u16::wrapping_add);
        let stored = (u16::from(read_reg(&mut rtc, REG_CHECKSUM_HI as u8)) << 8)
            | u16::from(read_reg(&mut rtc, REG_CHECKSUM_LO as u8));
        assert_eq!(stored, sum);
    }

    #[test]
    fn new_leaves_config_blank() {
        // Seeding is opt-in: a plain construction is still a blank power-on chip.
        let mut rtc = CmosRtc::new();
        assert_eq!(read_reg(&mut rtc, REG_FLOPPY_TYPE as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_CHECKSUM_HI as u8), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_CHECKSUM_LO as u8), 0x00);
    }

    #[test]
    fn seeded_config_survives_save_restore() {
        let rtc = CmosRtc::new().with_config(CmosConfig::default());
        let blob = rtc.save();
        let mut restored = CmosRtc::new();
        restored.restore(&blob).unwrap();
        assert_eq!(read_reg(&mut restored, REG_FLOPPY_TYPE as u8), 0x40);
        assert_eq!(read_reg(&mut restored, REG_BASE_MEM_HI as u8), 0x02);
        let sum: u16 = (0x10u8..=0x2D)
            .map(|i| u16::from(read_reg(&mut restored, i)))
            .fold(0u16, u16::wrapping_add);
        let stored = (u16::from(read_reg(&mut restored, REG_CHECKSUM_HI as u8)) << 8)
            | u16::from(read_reg(&mut restored, REG_CHECKSUM_LO as u8));
        assert_eq!(stored, sum);
    }
}
