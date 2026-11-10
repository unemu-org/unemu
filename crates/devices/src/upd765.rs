// SPDX-License-Identifier: AGPL-3.0-or-later
//! NEC µPD765 / Intel 8272A floppy disk controller (§7.8).
//!
//! Models the FDC as wired on a PC/AT: the digital output register (`0x3F2`),
//! the main status register (`0x3F4`), the data register (`0x3F5`) that carries
//! the command / parameter / result byte stream, and the digital input /
//! configuration-control register (`0x3F7`). It drives the classic three-phase
//! protocol — **command** (opcode + parameters), **execution** (the data
//! transfer), **result** (status bytes) — and raises **IRQ6** on command
//! completion.
//!
//! ## Data transfer runs through the 8237 DMA
//!
//! A real FDC does not touch memory; it hands each byte to the DMA controller
//! over the DREQ/DACK handshake, and the 8237 writes it to the address the guest
//! programmed. This model keeps that separation: on a READ, the controller pulls
//! the sector bytes from its backing image and **queues** them as a
//! [`device::DmaRequest`] on channel 2. The machine drains the request, streams
//! the bytes through the 8237's channel (which writes guest RAM through the
//! SMC-safe `note_write` path, §4.3), and reports the byte count and TC through
//! [`Device::dma_complete`] — at which point the controller finishes its result
//! phase and raises IRQ6. The controller never holds a memory reference and the
//! 8237 never sees the disk.
//!
//! READ/WRITE DATA honour MT: after EOT on head 0, a multi-track command
//! continues at sector 1 on head 1 of the same cylinder. Each sector is checked
//! against the physical geometry before mapping it into the backing image.
//! See NEC µPD765 datasheet, Functional Description of Commands, READ DATA /
//! WRITE DATA and Table 2; Intel 8272A datasheet, Command Descriptions, READ DATA
//! / WRITE DATA and Table 8.
//!
//! ## Geometry and per-drive media
//!
//! Geometry is per-drive data, not a hardcoded constant. Each of the four
//! drive-select positions the DOR can address may carry its own
//! [`Drive`]: a [`FloppyGeometry`] (cylinders × heads × sectors, 512-byte
//! sectors) plus a raw backing image. The four standard PC floppy formats are
//! provided as [`FloppyGeometry`] constants — 1.44 MB (80×2×18), 1.2 MB
//! (80×2×15), 720 KB (80×2×9), and 360 KB (40×2×9) — so drive 0 and drive 1 can
//! differ. A CHS address maps to an image offset with the addressed drive's own
//! geometry by `lba = (cyl * heads + head) * sectors_per_track + (sector - 1)`,
//! `offset = lba * 512`. An empty bay supplies no index pulses: READ ID and
//! READ/WRITE DATA wait in execution until reset. An address outside mounted
//! media's geometry terminates abnormally with a no-data status.
//!
//! A WRITE mutates the mounted image **in memory only**: it changes the drive's
//! backing `Vec`, but nothing writes those bytes back to a host image file — a
//! fresh run starts again from the on-disk image. Persisting a write to a
//! file-backed image is deliberately out of scope here.
//!
//! ## Coverage matrix (datasheet vs modelled)
//!
//! | Command                        | Opcode | Modelled | Notes                          |
//! |--------------------------------|--------|----------|--------------------------------|
//! | SPECIFY                        | `0x03` | yes      | timings accepted, not enforced |
//! | RECALIBRATE                    | `0x07` | partial  | PCN=0; empty bay: SE+EC, abnormal IRQ6 |
//! | SENSE INTERRUPT STATUS         | `0x08` | yes      | ST0 + PCN, or invalid if idle  |
//! | SEEK                           | `0x0F` | yes      | set present cylinder, IRQ6     |
//! | READ ID                        | `0x4A` | partial  | mounted: synthetic R=1/N=2; empty: waits for index |
//! | READ DATA                      | `0x06` | yes      | DMA transfer to memory, IRQ6   |
//! | WRITE DATA                     | `0x05` | yes      | DMA transfer from memory, IRQ6 |
//!
//! IBM Personal Computer AT Technical Reference, Fixed Disk and Diskette
//! Drive Adapter, Digital Input Register (3F7h) and Interface Lines: connector
//! pin 34 is Diskette Change, not READY. The board model holds the FDC's RDY
//! active for all four selects, consistent with the four RESET ready-change
//! polls (NEC µPD765A datasheet, RESET / SENSE INTERRUPT STATUS).
//! ASSUMED: RDY is held high independently of the selected slot; the available
//! adapter logic diagram, sheet 4 of 9, does not resolve the tie unambiguously.
//! No drive-presence/media distinction is modelled: an empty slot supplies
//! neither TRACK 0 nor index. NEC µPD765A, SEEK / RECALIBRATE and ST0: SEEK
//! reaches NCN normally; RECALIBRATE clears PCN even when TRACK 0 is absent,
//! then ends with SE+EC and IC=01 after 77 unsuccessful steps. The step delay
//! is collapsed, as for mounted seeks. READ ID / ST1 MA requires index pulses
//! to end an unsuccessful ID search; without them there is no result or IRQ.
//! This also applies to READ/WRITE DATA's ID search. The controller remains
//! Dormant deterministically; a BIOS's timer IRQs or polling instruction budget
//! can expire its software timeout, then DOR reset aborts the search and makes
//! the four reset SIS results available. No controller timeout is invented.
//!
//! IBM PC AT Technical Reference, Digital Output Register (3F2h), bit 3 and
//! adapter logic diagram sheet 5 of 9: IRQ6 is pending interrupt AND bit 3.
//! ASSUMED: a DMA request is retained while the same gate is closed. Real
//! hardware would end a data transfer with an overrun (ST1 OR) when bytes are
//! not serviced in time; this model has no data-rate timing, so it waits.
//!
//! Deliberately **not** modelled:
//! - **Rotating READ ID**: the datasheet's first ID encountered depends on
//!   rotational phase, absent from the flat-sector image. The model always
//!   chooses R=1/N=2 on a present track, rather than inventing angular position.
//! - **Fault-result CHRN without a readable ID**: NEC µPD765A datasheet,
//!   Command Symbol Description (H/HD) and Instruction Set require H=HD in
//!   valid commands; no mismatched-head result is specified. Assumption:
//!   pre-transfer failures echo command C/R/N and physical HD as result H;
//!   an empty-bay READ ID has no result CHRN. Logical-ID error reporting
//!   needs an ID-field/rotational model and is not claimed.
//! - **WRITE DELETED DATA** (`0x09`), **SENSE DRIVE STATUS** (`0x04`, whose ST3
//!   is the other place the write-protect line surfaces on real hardware), and
//!   **FORMAT TRACK**: the modelled write path is a plain WRITE DATA. A protected
//!   disk is still honoured — WRITE DATA to one faults with ST1 not-writable.
//! - **Non-DMA (PIO) transfer mode**: transfers always go through the 8237. The
//!   DOR's DMA-enable bit gates IRQ6 and DMA requests, but the controller has
//!   no data-port streaming path.
//! - **Cycle-accurate timing**: SPECIFY's step-rate / head-load / head-unload
//!   times are accepted and discarded; seeks complete immediately, and mounted
//!   reads finish on DMA completion. No rotational-latency model exists; empty
//!   bays never synthesize index pulses.
//! - **The skip (SK) modifier and MFM/FM selection**: SK and MFM do not change
//!   the transfer; READ and WRITE treat the raw image as normal MFM sectors.
//!
//! DMA completion carries TC separately from the byte count. TC ends normally;
//! without TC a short transfer reports overrun, a missing sector reports no data,
//! and exhausting EOT reports end of cylinder (NEC µPD765 datasheet, READ DATA,
//! WRITE DATA and Status Register 1).
//!
//! Snapshots contain registers, command/result phases, pending DMA context
//! and request data, and the absolute trace time base. Mounted media and wiring
//! belong to the receiving instance and must match the saved controller. This
//! model has no timed execution deadlines (NEC µPD765 datasheet, Execution Phase).
//! Snapshot v3 stores the interrupt latch before the DOR gate and permits an
//! empty-bay ID search without DMA; earlier versions are rejected. Restore
//! validates the waiting opcode, full command, empty slot, and absence of
//! results and DMA before accepting that execution phase.

use device::{
    AccessDir, Device, DeviceError, DmaOp, DmaOutcome, DmaRequest, NextEvent, TraceEvent, VClock,
};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "upd765-fdc";

/// First port decoded by the controller.
const PORT_BASE: u16 = 0x3F0;
/// Last port decoded by the controller.
const PORT_END: u16 = 0x3F7;
/// Digital output register (motor / drive-select / reset / DMA-enable).
const DOR: u16 = 0x3F2;
/// Main status register (read-only).
const MSR: u16 = 0x3F4;
/// Command / parameter / result data register.
const DATA: u16 = 0x3F5;
/// Digital input register (read) / configuration control register (write).
const DIR_CCR: u16 = 0x3F7;

/// DOR bit 2: `0` holds the controller in reset, `1` is normal operation.
const DOR_NOT_RESET: u8 = 1 << 2;
/// DOR bit 3: enable the DMA / interrupt path.
const DOR_DMA_ENABLE: u8 = 1 << 3;

/// MSR bit 7 (RQM): the data register is ready for a host transfer.
const MSR_RQM: u8 = 1 << 7;
/// MSR bit 6 (DIO): transfer direction — set means FDC→host (result phase).
const MSR_DIO: u8 = 1 << 6;
/// MSR bit 4 (CB): a command is in progress.
const MSR_BUSY: u8 = 1 << 4;

/// The DMA channel the PC floppy controller is wired to.
const FLOPPY_DMA_CHANNEL: u8 = 2;

/// Bytes per sector. Every standard PC floppy format uses 512-byte sectors.
const SECTOR_SIZE: usize = 512;

/// Total size of a 1.44 MB image in bytes, kept as a public constant for the
/// callers that seed a default high-density disk.
pub const FLOPPY_IMAGE_BYTES: usize = FloppyGeometry::HD1440.image_bytes();

/// The physical layout of a floppy disk: cylinders, heads, and sectors per
/// track, with a fixed 512-byte sector. This is the data the CHS→offset mapping
/// and the total image size are derived from, so a drive's format is a value the
/// machine configures rather than a constant baked into the controller.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FloppyGeometry {
    /// Number of cylinders (tracks per side).
    pub cylinders: u32,
    /// Number of heads (recording surfaces): 1 single-sided, 2 double-sided.
    pub heads: u32,
    /// Sectors per track.
    pub sectors_per_track: u32,
}

impl FloppyGeometry {
    /// 1.44 MB high-density 3.5″ floppy: 80 cylinders × 2 heads × 18 sectors.
    pub const HD1440: Self = Self {
        cylinders: 80,
        heads: 2,
        sectors_per_track: 18,
    };
    /// 1.2 MB high-density 5.25″ floppy: 80 cylinders × 2 heads × 15 sectors.
    pub const HD1200: Self = Self {
        cylinders: 80,
        heads: 2,
        sectors_per_track: 15,
    };
    /// 720 KB double-density 3.5″ floppy: 80 cylinders × 2 heads × 9 sectors.
    pub const DD720: Self = Self {
        cylinders: 80,
        heads: 2,
        sectors_per_track: 9,
    };
    /// 360 KB double-density 5.25″ floppy: 40 cylinders × 2 heads × 9 sectors.
    pub const DD360: Self = Self {
        cylinders: 40,
        heads: 2,
        sectors_per_track: 9,
    };

    /// Total size in bytes of a raw image with this geometry.
    #[must_use]
    pub const fn image_bytes(&self) -> usize {
        (self.cylinders * self.heads * self.sectors_per_track) as usize * SECTOR_SIZE
    }

    /// Byte offset of a CHS sector within an image of this geometry. Sector
    /// numbers are 1-based on the wire, so sector 1 is the first on the track.
    fn chs_to_offset(&self, cylinder: u8, head: u8, sector: u8) -> usize {
        let lba = (u32::from(cylinder) * self.heads + u32::from(head)) * self.sectors_per_track
            + u32::from(sector.saturating_sub(1));
        lba as usize * SECTOR_SIZE
    }

    /// Whether a CHS address lies within this geometry (a valid, readable
    /// sector). Sectors are 1-based, so sector 0 is out of range.
    fn contains(&self, cylinder: u8, head: u8, sector: u8) -> bool {
        u32::from(cylinder) < self.cylinders
            && u32::from(head) < self.heads
            && sector >= 1
            && u32::from(sector) <= self.sectors_per_track
    }
}

/// A floppy mounted in one of the controller's four drive positions: its
/// [`FloppyGeometry`] plus the raw backing image, and whether the disk carries a
/// write-protect tab.
#[derive(Clone, Debug)]
pub struct Drive {
    /// Physical format of the mounted disk.
    pub geometry: FloppyGeometry,
    /// Raw image bytes; typically [`FloppyGeometry::image_bytes`] long. Reads
    /// past the end of a short image return zero; a write past the end grows it.
    pub image: Vec<u8>,
    /// Whether the disk is write-protected. A WRITE DATA to a protected disk
    /// terminates abnormally (ST1 not-writable) and never mutates the image.
    pub write_protected: bool,
}

impl Drive {
    /// Mount a writable `image` with the given `geometry`.
    #[must_use]
    pub fn new(geometry: FloppyGeometry, image: Vec<u8>) -> Self {
        Self {
            geometry,
            image,
            write_protected: false,
        }
    }

    /// Mount a write-protected `image` with the given `geometry`: reads behave as
    /// normal, but a WRITE DATA is rejected with the not-writable status.
    #[must_use]
    pub fn new_read_only(geometry: FloppyGeometry, image: Vec<u8>) -> Self {
        Self {
            geometry,
            image,
            write_protected: true,
        }
    }
}

/// Snapshot layout version tag.
const SNAPSHOT_VERSION: u8 = 3;

/// ST0 bit 5: seek end (set after a completed RECALIBRATE / SEEK).
const ST0_SEEK_END: u8 = 1 << 5;
/// ST0 interrupt-code bits 7-6 == `11`: abnormal termination / ready-line change,
/// reported by each of the four polling SENSE INTERRUPT STATUS calls a reset
/// raises.
const ST0_POLL: u8 = 0b1100_0000;
/// ST0 interrupt-code bits 7-6 == `01`: the command terminated abnormally.
const ST0_ABNORMAL: u8 = 0b0100_0000;
/// ST0 bit 4 (EC): TRACK 0 was not reached within 77 recalibration steps.
const ST0_EQUIPMENT_CHECK: u8 = 1 << 4;
/// ST1 bit 2 (ND): no data — the requested sector was not found on the track.
/// NEC µPD765 / Intel 8272A datasheets, Status Register 1, No Data.
const ST1_NO_DATA: u8 = 1 << 2;
/// ST1 bit 1 (NW): not writable — a WRITE addressed a write-protected disk.
const ST1_NOT_WRITABLE: u8 = 1 << 1;
/// NEC µPD765 / Intel 8272A datasheets, Status Register 1, End of Cylinder.
const ST1_END_OF_CYLINDER: u8 = 1 << 7;
/// NEC µPD765 / Intel 8272A datasheets, Status Register 1, Overrun.
const ST1_OVERRUN: u8 = 1 << 4;

/// Which phase of the command protocol the controller is in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// Idle, ready to accept a command opcode.
    Idle,
    /// Collecting the parameter bytes that follow an opcode.
    Command,
    /// Running a transfer (bytes move over DMA, not the data port).
    Execution,
    /// Handing result status bytes back to the host.
    Result,
}

/// Context captured when a READ DATA enters execution, used to build the result
/// phase once the DMA transfer completes.
#[derive(Copy, Clone, Debug)]
struct PendingRead {
    drive: u8,
    head: u8,
    cylinder: u8,
    sector: u8,
    sector_size_code: u8,
    requested: usize,
    missing_sector: bool,
}

/// Context captured when a WRITE DATA enters execution, used to commit the DMA
/// bytes to media and build the result phase once the transfer completes.
///
/// Unlike [`PendingRead`] this also carries `param_head` — the head field from
/// the command parameters — because committing the bytes needs the same
/// CHS→offset mapping the read path uses. `head` retains the drive-select head
/// from which the result ID advances.
#[derive(Copy, Clone, Debug)]
struct PendingWrite {
    drive: u8,
    head: u8,
    param_head: u8,
    cylinder: u8,
    sector: u8,
    sector_size_code: u8,
    requested: usize,
    missing_sector: bool,
}

/// The NEC µPD765 / Intel 8272A floppy disk controller (§7.8).
///
/// Decodes `0x3F0..=0x3F7`; the modelled registers are the DOR, MSR, data, and
/// DIR/CCR ports. Drives IRQ6 through [`Device::irq_pending`]; the 8259 PIC
/// owns interrupt vectoring.
pub struct Upd765 {
    /// Digital output register: motor-on bits, drive select, reset, DMA-enable.
    dor: u8,
    /// Current protocol phase.
    phase: Phase,
    /// Command opcode plus the parameter bytes collected so far.
    command: Vec<u8>,
    /// Total bytes (opcode + parameters) expected before execution.
    expected: usize,
    /// Result status bytes queued for the host to read.
    result: Vec<u8>,
    /// Index of the next result byte to hand back.
    result_index: usize,
    /// Present cylinder number per drive (advanced by SEEK / RECALIBRATE).
    present_cylinder: [u8; 4],
    /// Status register 0 from the last completed command.
    st0: u8,
    /// Controller interrupt latch, before the DOR bit-3 bus gate.
    irq: bool,
    /// Remaining post-reset SENSE INTERRUPT STATUS polls that report the
    /// abnormal / ready-change status a controller reset raises for each drive.
    reset_poll: u8,
    /// A transfer queued for the machine to run through the 8237 (READ moves
    /// device→memory, WRITE moves memory→device).
    pending_request: Option<DmaRequest>,
    /// The READ context whose result phase is completed once the DMA finishes.
    pending_read: Option<PendingRead>,
    /// The WRITE context whose media commit and result phase follow the DMA.
    pending_write: Option<PendingWrite>,
    /// Media mounted in each of the four drive positions the DOR can select.
    /// `None` supplies neither TRACK 0 nor index pulses; RDY remains active.
    drives: [Option<Drive>; 4],
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl Upd765 {
    /// Create a controller in power-on state with no disk attached.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dor: 0,
            phase: Phase::Idle,
            command: Vec::new(),
            expected: 0,
            result: Vec::new(),
            result_index: 0,
            present_cylinder: [0; 4],
            st0: 0,
            irq: false,
            reset_poll: 0,
            pending_request: None,
            pending_read: None,
            pending_write: None,
            drives: [None, None, None, None],
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// Create a controller with a single 1.44 MB disk mounted in drive 0, backed
    /// by `image` (typically [`FLOPPY_IMAGE_BYTES`] bytes). Equivalent to `with_drives` carrying
    /// one high-density drive.
    #[must_use]
    pub fn with_image(image: Vec<u8>) -> Self {
        Self::with_drives([
            Some(Drive::new(FloppyGeometry::HD1440, image)),
            None,
            None,
            None,
        ])
    }

    /// Create a controller with media mounted per drive position. Each entry
    /// carries its own geometry, so drive 0 and drive 1 may differ.
    #[must_use]
    pub fn with_drives(drives: [Option<Drive>; 4]) -> Self {
        Self {
            drives,
            ..Self::new()
        }
    }

    /// The current IRQ6 line level, for tests and the machine's edge detector.
    #[must_use]
    pub fn irq_line(&self) -> bool {
        self.irq && self.dor & DOR_DMA_ENABLE != 0
    }

    /// The present cylinder recorded for `drive`, for tests.
    #[must_use]
    pub fn present_cylinder(&self, drive: usize) -> u8 {
        self.present_cylinder[drive & 0b11]
    }

    /// The full main status register byte for the current phase.
    fn msr(&self) -> u8 {
        match self.phase {
            Phase::Idle => MSR_RQM,
            Phase::Command => MSR_RQM | MSR_BUSY,
            // The host does not drive the data port during a DMA transfer.
            Phase::Execution => MSR_BUSY,
            Phase::Result => MSR_RQM | MSR_DIO | MSR_BUSY,
        }
    }

    /// Total command length (opcode + parameters) for `opcode`, or `None` if the
    /// opcode is not modelled. The low five bits select the command; the high
    /// bits (MT/MFM/SK) do not change the length.
    fn command_length(opcode: u8) -> Option<usize> {
        match opcode & 0x1F {
            0x03 => Some(3), // SPECIFY: 2 parameters
            0x07 => Some(2), // RECALIBRATE: 1 parameter
            0x08 => Some(1), // SENSE INTERRUPT STATUS: no parameters
            0x0F => Some(3), // SEEK: 2 parameters
            0x0A => Some(2), // READ ID: 1 parameter
            0x06 => Some(9), // READ DATA: 8 parameters
            0x05 => Some(9), // WRITE DATA: 8 parameters
            _ => None,
        }
    }

    /// IBM PC AT Technical Reference, Fixed Disk and Diskette Drive Adapter,
    /// Digital Output Register: bit 3 gates reset-completion IRQ6 too.
    /// NEC µPD765A datasheet, RESET: reset completion is sensed with SIS.
    fn write_dor(&mut self, value: u8) {
        let leaving_reset = self.dor & DOR_NOT_RESET == 0 && value & DOR_NOT_RESET != 0;
        self.dor = value;
        if leaving_reset {
            // Coming out of reset raises an interrupt; the BIOS then issues four
            // SENSE INTERRUPT STATUS commands, one reporting each drive.
            self.phase = Phase::Idle;
            self.command.clear();
            self.result.clear();
            self.result_index = 0;
            self.pending_request = None;
            self.pending_read = None;
            self.pending_write = None;
            self.reset_poll = 4;
            self.raise_irq();
        }
    }

    /// Handle a write to the data register (a command / parameter byte).
    fn write_data(&mut self, byte: u8) {
        match self.phase {
            Phase::Idle => {
                let Some(len) = Self::command_length(byte) else {
                    // An unknown opcode leaves an invalid-command status for the
                    // host's next result read, matching the real controller.
                    self.st0 = 0x80;
                    self.result = vec![0x80];
                    self.result_index = 0;
                    self.phase = Phase::Result;
                    return;
                };
                self.command.clear();
                self.command.push(byte);
                self.expected = len;
                if self.command.len() == self.expected {
                    self.execute();
                } else {
                    self.phase = Phase::Command;
                }
            }
            Phase::Command => {
                self.command.push(byte);
                if self.command.len() == self.expected {
                    self.execute();
                }
            }
            // Bytes written during execution or result are ignored by the real
            // controller; nothing to collect.
            Phase::Execution | Phase::Result => {}
        }
    }

    /// Dispatch a fully-collected command to its handler.
    fn execute(&mut self) {
        let opcode = self.command[0] & 0x1F;
        match opcode {
            0x03 => self.cmd_specify(),
            0x07 => self.cmd_recalibrate(),
            0x08 => self.cmd_sense_interrupt(),
            0x0F => self.cmd_seek(),
            0x0A => self.cmd_read_id(),
            0x06 => self.cmd_read_data(),
            0x05 => self.cmd_write_data(),
            _ => self.finish_no_result(),
        }
    }

    /// SPECIFY: accept the step-rate / head timings and return to idle. No
    /// interrupt, no result phase.
    fn cmd_specify(&mut self) {
        self.finish_no_result();
    }

    /// RECALIBRATE: seek the selected drive to cylinder 0 and raise IRQ6. The
    /// result is read later via SENSE INTERRUPT STATUS. NEC µPD765A datasheet,
    /// RECALIBRATE: PCN is cleared before checking TRACK 0; failure after 77
    /// steps reports equipment check, with the step delay collapsed here.
    fn cmd_recalibrate(&mut self) {
        let drive = self.command[1] & 0b11;
        self.st0 = ST0_SEEK_END | drive;
        self.present_cylinder[usize::from(drive)] = 0;
        if self.drives[usize::from(drive)].is_none() {
            self.st0 |= ST0_ABNORMAL | ST0_EQUIPMENT_CHECK;
        }
        self.raise_irq();
        self.finish_no_result();
    }

    /// SEEK: set the selected drive's present cylinder and raise IRQ6.
    /// NEC µPD765A datasheet, SEEK: the PCN counter advances to NCN without
    /// position feedback. The AT adapter keeps RDY active even for an empty bay.
    fn cmd_seek(&mut self) {
        let drive_head = self.command[1];
        let drive = drive_head & 0b11;
        let head = (drive_head >> 2) & 1;
        let cylinder = self.command[2];
        self.st0 = ST0_SEEK_END | (head << 2) | drive;
        self.present_cylinder[usize::from(drive)] = cylinder;
        self.raise_irq();
        self.finish_no_result();
    }

    /// SENSE INTERRUPT STATUS: report ST0 and the present cylinder, clearing the
    /// pending interrupt. After a reset the first four calls report the drive-poll
    /// status instead. With nothing pending, return only invalid-command ST0.
    /// See NEC µPD765 datasheet, Functional Description of Commands, SENSE
    /// INTERRUPT STATUS / INVALID; Intel 8272A datasheet, Command Descriptions,
    /// SENSE INTERRUPT STATUS / INVALID.
    fn cmd_sense_interrupt(&mut self) {
        if self.reset_poll == 0 && !self.irq {
            self.st0 = 0x80;
            self.result = vec![self.st0];
            self.result_index = 0;
            self.phase = Phase::Result;
            return;
        }
        let (st0, pcn) = if self.reset_poll > 0 {
            let drive = 4 - self.reset_poll;
            self.reset_poll -= 1;
            (ST0_POLL | drive, 0)
        } else {
            let drive = usize::from(self.st0 & 0b11);
            (self.st0, self.present_cylinder[drive])
        };
        self.irq = false;
        self.result = vec![st0, pcn];
        self.result_index = 0;
        self.phase = Phase::Result;
    }

    /// READ ID: report synthetic C/H/R/N on mounted media. Without index pulses,
    /// stay in execution until reset (NEC µPD765A, READ ID and ST1 MA).
    fn cmd_read_id(&mut self) {
        let drive_head = self.command[1];
        let drive = drive_head & 0b11;
        let head = (drive_head >> 2) & 1;
        let cylinder = self.present_cylinder[usize::from(drive)];
        if self.drives[usize::from(drive)].is_none() {
            self.phase = Phase::Execution;
            return;
        }
        self.st0 = (head << 2) | drive;
        self.raise_irq();
        self.result = vec![self.st0, 0, 0, cylinder, head, 1, 2];
        self.result_index = 0;
        self.phase = Phase::Result;
    }

    /// READ DATA: queue sectors through EOT, continuing onto head 1 when MT is
    /// set. Only sector IDs within the physical geometry can reach the image;
    /// an invalid starting CHS returns ND (NEC µPD765 datasheet, READ DATA;
    /// Intel 8272A datasheet, READ DATA and Status Register 1, ND).
    /// Result and IRQ6 follow DMA completion.
    fn cmd_read_data(&mut self) {
        let drive_head = self.command[1];
        let drive = drive_head & 0b11;
        let head = (drive_head >> 2) & 1;
        let cylinder = self.command[2];
        let param_head = self.command[3];
        let sector = self.command[4];
        let sector_size_code = self.command[5];

        // With RDY active and no index pulses, the ID search has no endpoint
        // (NEC µPD765A datasheet, READ DATA and Status Register 1, MA).
        let Some(media) = self.drives[usize::from(drive)].as_ref() else {
            self.phase = Phase::Execution;
            return;
        };
        let geometry = media.geometry;

        // An address outside the drive's geometry has no matching data field.
        if !geometry.contains(cylinder, param_head, sector) {
            self.command_fault(
                drive,
                head,
                cylinder,
                sector,
                sector_size_code,
                0,
                ST1_NO_DATA,
            );
            return;
        }

        let mut data = Vec::new();
        for (h, s) in self
            .data_sectors(param_head, sector)
            .take_while(|&(h, s)| geometry.contains(cylinder, h, s))
        {
            let offset = geometry.chs_to_offset(cylinder, h, s);
            for j in 0..SECTOR_SIZE {
                data.push(media.image.get(offset + j).copied().unwrap_or(0));
            }
        }

        let requested = data.len();
        let missing_sector = self
            .data_sectors(param_head, sector)
            .nth(requested / SECTOR_SIZE)
            .is_some();
        self.pending_read = Some(PendingRead {
            drive,
            head,
            cylinder,
            sector,
            sector_size_code,
            requested,
            missing_sector,
        });
        self.pending_request = Some(DmaRequest {
            channel: FLOPPY_DMA_CHANNEL,
            op: DmaOp::ToMemory(data),
        });
        self.phase = Phase::Execution;
    }

    /// Sector IDs in transfer order. MT spans at most the two sides of one
    /// cylinder, not the next cylinder (NEC µPD765 datasheet, READ DATA,
    /// Multi-Track; Intel 8272A datasheet, READ DATA, Transfer Capacity).
    fn data_sectors(&self, head: u8, sector: u8) -> impl Iterator<Item = (u8, u8)> {
        let eot = self.command[6];
        let next_head = self.command[0] & 0x80 != 0 && head == 0 && sector <= eot;
        (sector..=eot.max(sector))
            .map(move |r| (head, r))
            .chain((1..=eot).filter(move |_| next_head).map(|r| (1, r)))
    }

    /// Result ID after a transfer terminated by the host: at EOT, R becomes 1;
    /// MT complements H, and C advances unless MT ended on side 0. N is unchanged.
    /// NEC µPD765 datasheet, READ DATA, Table 2; Intel 8272A datasheet, READ DATA,
    /// Table 8, ID Information When Processor Terminates Command.
    fn result_chs(
        &self,
        mut cylinder: u8,
        mut head: u8,
        mut sector: u8,
        sectors_done: usize,
    ) -> (u8, u8, u8) {
        let mt = self.command[0] & 0x80 != 0;
        for _ in 0..sectors_done {
            if sector == self.command[6] {
                sector = 1;
                if !mt || head == 1 {
                    cylinder = cylinder.wrapping_add(1);
                }
                if mt {
                    head ^= 1;
                }
            } else {
                sector = sector.wrapping_add(1);
            }
        }
        (cylinder, head, sector)
    }

    /// Terminate a READ or WRITE DATA abnormally: build the seven-byte result
    /// with the given ST0/ST1 fault bits and raise IRQ6.
    #[allow(clippy::too_many_arguments)]
    fn command_fault(
        &mut self,
        drive: u8,
        head: u8,
        cylinder: u8,
        sector: u8,
        sector_size_code: u8,
        st0_extra: u8,
        st1: u8,
    ) {
        self.st0 = ST0_ABNORMAL | st0_extra | (head << 2) | drive;
        self.raise_irq();
        self.result = vec![self.st0, st1, 0, cylinder, head, sector, sector_size_code];
        self.result_index = 0;
        self.phase = Phase::Result;
    }

    /// TC terminates READ/WRITE normally even when EOT exceeds the geometry.
    /// Without TC, a stalled DMA is an overrun; consuming the valid prefix leaves
    /// either a missing sector (ND) or the command's EOT limit (EN).
    /// NEC µPD765 datasheet, Functional Description of Commands, READ DATA /
    /// WRITE DATA and Status Registers 0/1; Intel 8272A datasheet, Command
    /// Descriptions, READ DATA / WRITE DATA and Status Register 1, OR / ND / EN.
    fn transfer_status(
        transferred: usize,
        requested: usize,
        terminal_count: bool,
        missing_sector: bool,
    ) -> u8 {
        if terminal_count {
            0
        } else if transferred < requested {
            ST1_OVERRUN
        } else if missing_sector {
            ST1_NO_DATA
        } else {
            ST1_END_OF_CYLINDER
        }
    }

    /// Complete a READ transfer once the machine has moved `transferred` bytes:
    /// advance the sector past what was read, build the seven-byte result, and
    /// raise IRQ6.
    fn complete_read(&mut self, transferred: usize, terminal_count: bool) {
        let Some(read) = self.pending_read.take() else {
            return;
        };
        let sectors_done = transferred / SECTOR_SIZE;
        let (cylinder, head, next_sector) =
            self.result_chs(read.cylinder, read.head, read.sector, sectors_done);
        let active_head = self
            .data_sectors(read.head, read.sector)
            .take(sectors_done)
            .last()
            .map_or(read.head, |(h, _)| h);
        let st1 = Self::transfer_status(
            transferred,
            read.requested,
            terminal_count,
            read.missing_sector,
        );
        self.st0 = (active_head << 2) | read.drive | if st1 == 0 { 0 } else { ST0_ABNORMAL };
        self.raise_irq();
        self.result = vec![
            self.st0,
            st1,
            0,
            cylinder,
            head,
            next_sector,
            read.sector_size_code,
        ];
        self.result_index = 0;
        self.phase = Phase::Result;
    }

    /// WRITE DATA: request sectors through EOT and, with MT, head 1, to be
    /// pulled out of memory over DMA and committed to the backing image. The
    /// result phase and IRQ6 follow once the machine reports the transfer
    /// complete. Mirrors [`Upd765::cmd_read_data`] with the DMA direction
    /// reversed.
    fn cmd_write_data(&mut self) {
        let drive_head = self.command[1];
        let drive = drive_head & 0b11;
        let head = (drive_head >> 2) & 1;
        let cylinder = self.command[2];
        let param_head = self.command[3];
        let sector = self.command[4];
        let sector_size_code = self.command[5];

        // With RDY active and no index pulses, the ID search has no endpoint
        // (NEC µPD765A datasheet, READ DATA and Status Register 1, MA).
        let Some(media) = self.drives[usize::from(drive)].as_ref() else {
            self.phase = Phase::Execution;
            return;
        };
        let geometry = media.geometry;

        // A write-protected disk rejects the command before any media is touched.
        if media.write_protected {
            self.command_fault(
                drive,
                head,
                cylinder,
                sector,
                sector_size_code,
                0,
                ST1_NOT_WRITABLE,
            );
            return;
        }

        // An address outside the drive's geometry has no matching data field.
        if !geometry.contains(cylinder, param_head, sector) {
            self.command_fault(
                drive,
                head,
                cylinder,
                sector,
                sector_size_code,
                0,
                ST1_NO_DATA,
            );
            return;
        }

        let sector_count = self
            .data_sectors(param_head, sector)
            .take_while(|&(h, s)| geometry.contains(cylinder, h, s))
            .count();

        let requested = sector_count * SECTOR_SIZE;
        let missing_sector = self
            .data_sectors(param_head, sector)
            .nth(sector_count)
            .is_some();
        self.pending_write = Some(PendingWrite {
            drive,
            head,
            param_head,
            cylinder,
            sector,
            sector_size_code,
            requested,
            missing_sector,
        });
        self.pending_request = Some(DmaRequest {
            channel: FLOPPY_DMA_CHANNEL,
            op: DmaOp::FromMemory {
                len: sector_count * SECTOR_SIZE,
            },
        });
        self.phase = Phase::Execution;
    }

    /// Commit a WRITE transfer once the machine has pulled `bytes` out of memory:
    /// write them into the addressed drive's image, advance the sector, build the
    /// seven-byte result, and raise IRQ6. Mirrors [`Upd765::complete_read`].
    ///
    /// A floppy write is atomic per sector. If terminal count cut the DMA short
    /// mid-sector, the last partially-supplied sector is still committed whole
    /// with its tail zero-filled — so the number of sectors written rounds *up*
    /// (the read path, which reports how many whole sectors reached memory, rounds
    /// down). A write that runs past a short backing image grows the image,
    /// mirroring the read path treating past-the-end bytes as zero.
    fn complete_write(&mut self, bytes: Vec<u8>, terminal_count: bool) {
        let Some(write) = self.pending_write.take() else {
            return;
        };
        let sectors_written = bytes.len().div_ceil(SECTOR_SIZE);
        let sectors = self.data_sectors(write.param_head, write.sector);
        if let Some(media) = self.drives[usize::from(write.drive)].as_mut() {
            for ((head, sector), chunk) in sectors.zip(bytes.chunks(SECTOR_SIZE)) {
                let offset = media.geometry.chs_to_offset(write.cylinder, head, sector);
                let end = offset + SECTOR_SIZE;
                if media.image.len() < end {
                    media.image.resize(end, 0);
                }
                // Clear the whole sector-aligned region first so a short final
                // sector's tail is zero, then overlay the supplied bytes.
                media.image[offset..end].fill(0);
                media.image[offset..offset + chunk.len()].copy_from_slice(chunk);
            }
        }
        let (cylinder, head, next_sector) =
            self.result_chs(write.cylinder, write.head, write.sector, sectors_written);
        let active_head = self
            .data_sectors(write.head, write.sector)
            .take(sectors_written)
            .last()
            .map_or(write.head, |(h, _)| h);
        let st1 = Self::transfer_status(
            bytes.len(),
            write.requested,
            terminal_count,
            write.missing_sector,
        );
        self.st0 = (active_head << 2) | write.drive | if st1 == 0 { 0 } else { ST0_ABNORMAL };
        self.raise_irq();
        self.result = vec![
            self.st0,
            st1,
            0,
            cylinder,
            head,
            next_sector,
            write.sector_size_code,
        ];
        self.result_index = 0;
        self.phase = Phase::Result;
    }

    /// Latch controller completion independently of the adapter's bus gate.
    fn raise_irq(&mut self) {
        self.irq = true;
    }

    /// Finish a command that has no result phase, returning to idle.
    fn finish_no_result(&mut self) {
        self.phase = Phase::Idle;
        self.command.clear();
    }

    /// Read the next result byte, returning to idle (and lowering IRQ6) once the
    /// last one is consumed.
    fn read_result(&mut self) -> u8 {
        if self.result_index >= self.result.len() {
            return 0;
        }
        let byte = self.result[self.result_index];
        self.result_index += 1;
        if self.result_index >= self.result.len() {
            self.phase = Phase::Idle;
            self.command.clear();
            self.result.clear();
            self.result_index = 0;
            // Reading the last result byte acknowledges the read/read-id interrupt.
            self.irq = false;
        }
        byte
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

impl Default for Upd765 {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for Upd765 {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        if !(PORT_BASE..=PORT_END).contains(&port) {
            return Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            });
        }
        let value = match port {
            DOR => u32::from(self.dor),
            MSR => u32::from(self.msr()),
            DATA => u32::from(self.read_result()),
            // Digital input register: bit 7 is disk-change, reported clear (the
            // disk is present and unchanged). The rest float high on real boards.
            DIR_CCR => 0x00,
            // Other ports in the decoded window are not driven by this controller.
            _ => 0xFF,
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        if !(PORT_BASE..=PORT_END).contains(&port) {
            return Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            });
        }
        let byte = value as u8;
        match port {
            DOR => self.write_dor(byte),
            DATA => self.write_data(byte),
            // The configuration-control register selects the data rate, which this
            // functional model does not act on.
            DIR_CCR => {}
            // MSR is read-only; other ports are not driven.
            _ => {}
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
        // Empty-bay searches cannot advance without external index pulses;
        // mounted transfers finish through DMA. This clock only stamps traces.
        self.now = now;
    }

    /// NEC µPD765 datasheet, SPECIFY, SEEK and RECALIBRATE: step/head timings
    /// are accepted but not enforced here. Reset IRQ and seeks finish on port
    /// access; READ/WRITE DATA finishes on DMA completion, with no timed phase.
    fn next_event(&self, _: VClock) -> NextEvent {
        NextEvent::Dormant
    }

    fn irq_pending(&self) -> bool {
        self.irq_line()
    }

    fn take_dma_request(&mut self) -> Option<DmaRequest> {
        if self.dor & DOR_DMA_ENABLE != 0 {
            self.pending_request.take()
        } else {
            None
        }
    }

    fn dma_complete(&mut self, outcome: DmaOutcome) {
        match outcome {
            DmaOutcome::ToMemory {
                transferred,
                terminal_count,
            } => self.complete_read(transferred, terminal_count),
            DmaOutcome::FromMemory {
                bytes,
                terminal_count,
            } => self.complete_write(bytes, terminal_count),
        }
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        // A controller reset clears the registers but does not eject any disk.
        let drives = std::mem::take(&mut self.drives);
        *self = Self::with_drives(drives);
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        // The backing image is media, not controller state; it is not snapshotted.
        let phase_tag = match self.phase {
            Phase::Idle => 0,
            Phase::Command => 1,
            Phase::Execution => 2,
            Phase::Result => 3,
        };
        let mut blob = vec![
            SNAPSHOT_VERSION,
            self.dor,
            phase_tag,
            self.st0,
            u8::from(self.irq),
            self.reset_poll,
        ];
        blob.extend_from_slice(&self.present_cylinder);
        blob.push(self.expected as u8);
        blob.push(self.command.len() as u8);
        blob.extend_from_slice(&self.command);
        blob.push(self.result.len() as u8);
        blob.extend_from_slice(&self.result);
        blob.push(self.result_index as u8);
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        let pending = self
            .pending_read
            .map(|r| {
                (
                    1,
                    r.drive,
                    r.head,
                    self.command[3],
                    r.cylinder,
                    r.sector,
                    r.sector_size_code,
                    r.requested,
                    r.missing_sector,
                )
            })
            .or_else(|| {
                self.pending_write.map(|w| {
                    (
                        2,
                        w.drive,
                        w.head,
                        w.param_head,
                        w.cylinder,
                        w.sector,
                        w.sector_size_code,
                        w.requested,
                        w.missing_sector,
                    )
                })
            });
        if let Some((tag, drive, head, param_head, cylinder, sector, size, requested, missing)) =
            pending
        {
            blob.extend([tag, drive, head, param_head, cylinder, sector, size]);
            blob.extend_from_slice(&(requested as u32).to_le_bytes());
            blob.push(u8::from(missing));
        } else {
            blob.push(0);
        }
        blob.push(u8::from(self.pending_request.is_some()));
        if let Some(DmaRequest {
            op: DmaOp::ToMemory(bytes),
            ..
        }) = &self.pending_request
        {
            blob.extend_from_slice(bytes);
        }
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        let bad = |reason| DeviceError::InvalidSnapshot {
            device: NAME,
            reason,
        };
        let mut it = snapshot.iter().copied();
        let mut next = || it.next().ok_or(bad("snapshot truncated"));
        if next()? != SNAPSHOT_VERSION {
            return Err(bad("unsupported snapshot version"));
        }
        let dor = next()?;
        let phase = match next()? {
            0 => Phase::Idle,
            1 => Phase::Command,
            2 => Phase::Execution,
            3 => Phase::Result,
            _ => return Err(bad("invalid phase tag")),
        };
        let st0 = next()?;
        let irq = next()?;
        let reset_poll = next()?;
        let mut present_cylinder = [0; 4];
        for pcn in &mut present_cylinder {
            *pcn = next()?;
        }
        let expected = usize::from(next()?);
        let command_len = usize::from(next()?);
        if command_len > 9 || !matches!(expected, 0 | 1 | 2 | 3 | 9) || irq > 1 || reset_poll > 4 {
            return Err(bad("invalid command state"));
        }
        let mut command = Vec::new();
        for _ in 0..command_len {
            command.push(next()?);
        }
        let result_len = usize::from(next()?);
        if result_len > 7 {
            return Err(bad("invalid result length"));
        }
        let mut result = Vec::new();
        for _ in 0..result_len {
            result.push(next()?);
        }
        let result_index = usize::from(next()?);
        let mut clock = [0; 8];
        for byte in &mut clock {
            *byte = next()?;
        }
        let now = VClock(u64::from_le_bytes(clock));
        let pending_tag = next()?;
        let mut pending_read = None;
        let mut pending_write = None;
        let mut requested = 0;
        if pending_tag != 0 {
            if pending_tag > 2
                || phase != Phase::Execution
                || command_len != 9
                || expected != 9
                || command[0] & 0x1F != if pending_tag == 1 { 6 } else { 5 }
            {
                return Err(bad("inconsistent transfer phase"));
            }
            let drive = next()?;
            let head = next()?;
            let param_head = next()?;
            let cylinder = next()?;
            let sector = next()?;
            let sector_size_code = next()?;
            let mut count = [0; 4];
            for byte in &mut count {
                *byte = next()?;
            }
            requested = u32::from_le_bytes(count) as usize;
            let missing_sector = next()?;
            if drive != command[1] & 3
                || head != (command[1] >> 2) & 1
                || param_head != command[3]
                || cylinder != command[2]
                || sector != command[4]
                || sector_size_code != command[5]
                || requested == 0
                || requested > 510 * SECTOR_SIZE
                || !requested.is_multiple_of(SECTOR_SIZE)
                || missing_sector > 1
            {
                return Err(bad("invalid transfer context"));
            }
            let Some(media) = &self.drives[usize::from(drive)] else {
                return Err(bad("transfer requires mounted media"));
            };
            if !media.geometry.contains(cylinder, param_head, sector)
                || (pending_tag == 2 && media.write_protected)
            {
                return Err(bad("transfer incompatible with media"));
            }
            let eot = command[6];
            let next_head = command[0] & 0x80 != 0 && param_head == 0 && sector <= eot;
            let mut sectors = (sector..=eot.max(sector))
                .map(|r| (param_head, r))
                .chain((1..=eot).filter(|_| next_head).map(|r| (1, r)));
            for _ in 0..requested / SECTOR_SIZE {
                if !sectors
                    .next()
                    .is_some_and(|(h, r)| media.geometry.contains(cylinder, h, r))
                {
                    return Err(bad("transfer length exceeds media"));
                }
            }
            let following = sectors.next();
            if following.is_some_and(|(h, r)| media.geometry.contains(cylinder, h, r))
                || (missing_sector != 0) != following.is_some()
            {
                return Err(bad("inconsistent transfer boundary"));
            }
            if pending_tag == 1 {
                pending_read = Some(PendingRead {
                    drive,
                    head,
                    cylinder,
                    sector,
                    sector_size_code,
                    requested,
                    missing_sector: missing_sector != 0,
                });
            } else {
                pending_write = Some(PendingWrite {
                    drive,
                    head,
                    param_head,
                    cylinder,
                    sector,
                    sector_size_code,
                    requested,
                    missing_sector: missing_sector != 0,
                });
            }
        }
        let request_tag = next()?;
        if request_tag > 1 || (request_tag != 0 && pending_tag == 0) {
            return Err(bad("invalid DMA request tag"));
        }
        let pending_request = if request_tag != 0 {
            let op = if pending_tag == 1 {
                let mut bytes = Vec::new();
                for _ in 0..requested {
                    bytes.push(next()?);
                }
                DmaOp::ToMemory(bytes)
            } else {
                DmaOp::FromMemory { len: requested }
            };
            Some(DmaRequest {
                channel: FLOPPY_DMA_CHANNEL,
                op,
            })
        } else {
            None
        };
        if it.next().is_some() {
            return Err(bad("trailing snapshot bytes"));
        }
        let valid_command = command.first().and_then(|&op| Self::command_length(op));
        let command_consistent =
            command.is_empty() || (valid_command == Some(expected) && command_len <= expected);
        let result_consistent = match command.first().map(|op| op & 0x1F) {
            Some(8) => matches!(result_len, 1 | 2),
            Some(5 | 6 | 0x0A) => result_len == 7,
            None => result_len == 1,
            _ => false,
        };
        let valid_phase = command_consistent
            && match phase {
                Phase::Idle => {
                    command.is_empty() && result.is_empty() && result_index == 0 && pending_tag == 0
                }
                Phase::Command => {
                    !command.is_empty()
                        && command_len < expected
                        && result.is_empty()
                        && result_index == 0
                        && pending_tag == 0
                }
                Phase::Execution => {
                    let waiting_for_index = pending_tag == 0
                        && matches!(command.first().map(|op| op & 0x1F), Some(5 | 6 | 0x0A))
                        && command_len == expected
                        && self.drives[usize::from(command[1] & 3)].is_none();
                    (pending_tag != 0 || waiting_for_index)
                        && result.is_empty()
                        && result_index == 0
                }
                Phase::Result => {
                    result_consistent
                        && result_index < result_len
                        && pending_tag == 0
                        && (command.is_empty() || command_len == expected)
                }
            };
        if !valid_phase {
            return Err(bad("inconsistent command phase"));
        }
        self.dor = dor;
        self.phase = phase;
        self.st0 = st0;
        self.irq = irq != 0;
        self.reset_poll = reset_poll;
        self.present_cylinder = present_cylinder;
        self.expected = expected;
        self.command = command;
        self.result = result;
        self.result_index = result_index;
        self.now = now;
        self.pending_request = pending_request;
        self.pending_read = pending_read;
        self.pending_write = pending_write;
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
    fn dor_clearing_gate_removes_existing_irq() {
        let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
        bus_command(&mut bus, &[0x0F, 0, 7]);
        assert!(bus.device().irq_pending());
        bus.port_write(DOR, 1, 4).unwrap();
        assert!(!bus.device().irq_pending());
        bus.port_write(DOR, 1, 12).unwrap();
        assert!(bus.device().irq_pending());
    }

    #[test]
    fn dor_gate_retains_dma_request() {
        let mut bus = MockBus::new(Box::new(ready_fdc(vec![0x5A; SECTOR_SIZE])));
        bus.port_write(DOR, 1, 4).unwrap();
        bus_command(&mut bus, &[6, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF]);
        assert!(bus.device_mut().take_dma_request().is_none());
        bus.port_write(DOR, 1, 12).unwrap();
        let request = bus.device_mut().take_dma_request().unwrap();
        assert!(matches!(request.op, DmaOp::ToMemory(bytes) if bytes == vec![0x5A; SECTOR_SIZE]));
    }

    #[test]
    fn empty_bay_recalibrate_clears_pcn_and_reports_equipment_check() {
        let mut fdc = ready_fdc(Vec::new());
        fdc.present_cylinder[1] = 90;
        let mut bus = MockBus::new(Box::new(fdc));
        bus_command(&mut bus, &[7, 1]);
        bus_command(&mut bus, &[8]);
        assert_eq!(
            bus_results(&mut bus, 2),
            vec![ST0_ABNORMAL | ST0_SEEK_END | 0x10 | 1, 0]
        );
    }

    #[test]
    fn wait_snapshot_rejects_inconsistent_state_atomically() {
        let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
        bus_command(&mut bus, &[0x4A, 1]);
        let saved = bus.device().save();
        for (index, value) in [(0, 2), (12, 0x03), (13, 0), (11, 1)] {
            let mut invalid = saved.clone();
            invalid[index] = value;
            assert!(bus.device_mut().restore(&invalid).is_err(), "index={index}");
            assert_eq!(bus.device().save(), saved);
        }
        let mut mounted = MockBus::new(Box::new(ready_drives([
            None,
            Some(Drive::new(FloppyGeometry::HD1440, Vec::new())),
            None,
            None,
        ])));
        let before = mounted.device().save();
        assert!(mounted.device_mut().restore(&saved).is_err());
        assert_eq!(mounted.device().save(), before);
    }

    #[test]
    fn dor_gate_exposes_and_hides_pending_irq() {
        let mut bus = MockBus::new(Box::new(Upd765::new()));
        bus.port_write(DOR, 1, 4).unwrap();
        assert!(!bus.device().irq_pending());
        bus.port_write(DOR, 1, 12).unwrap();
        assert!(bus.device().irq_pending());
        bus.port_write(DOR, 1, 4).unwrap();
        assert!(!bus.device().irq_pending());
        bus_command(&mut bus, &[8]);
        assert_eq!(bus_results(&mut bus, 2), vec![ST0_POLL, 0]);
        for _ in 0..3 {
            bus_command(&mut bus, &[8]);
            bus_results(&mut bus, 2);
        }
        bus_command(&mut bus, &[0x0F, 1, 7]);
        assert!(!bus.device().irq_pending());
        let saved = bus.device().save();
        bus.device_mut().restore(&saved).unwrap();
        bus.port_write(DOR, 1, 12).unwrap();
        assert!(bus.device().irq_pending());
        bus.port_write(DOR, 1, 4).unwrap();
        assert!(!bus.device().irq_pending());
        bus_command(&mut bus, &[8]);
        assert_eq!(bus_results(&mut bus, 2), vec![ST0_SEEK_END | 1, 7]);
        bus.port_write(DOR, 1, 12).unwrap();
        assert!(!bus.device().irq_pending());
    }

    #[test]
    fn empty_bay_seek_and_recalibrate_follow_at_signals() {
        let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
        bus_command(&mut bus, &[0x0F, 5, 90]);
        assert!(bus.device().irq_pending());
        bus_command(&mut bus, &[8]);
        assert_eq!(bus_results(&mut bus, 2), vec![ST0_SEEK_END | 5, 90]);
        bus_command(&mut bus, &[7, 1]);
        assert!(bus.device().irq_pending());
        bus_command(&mut bus, &[8]);
        assert_eq!(
            bus_results(&mut bus, 2),
            vec![ST0_ABNORMAL | ST0_SEEK_END | 0x10 | 1, 0]
        );
    }

    #[test]
    fn empty_bay_data_commands_wait_without_dma_or_irq() {
        for opcode in [5, 6] {
            let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
            bus_command(&mut bus, &[opcode, 1, 0, 0, 1, 2, 1, 0x1B, 0xFF]);
            bus.tick(1_000_000);
            assert_eq!(bus.port_read(MSR, 1).unwrap(), u32::from(MSR_BUSY));
            assert!(!bus.device().irq_pending());
            assert!(bus.device_mut().take_dma_request().is_none());
            let saved = bus.device().save();
            bus.device_mut().restore(&saved).unwrap();
            assert_eq!(bus.device().save(), saved);
        }
    }

    #[test]
    fn empty_bay_read_id_waits_until_reset() {
        let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
        bus_command(&mut bus, &[0x4A, 1]);
        for elapsed in [1, 1000, 1_000_000] {
            bus.tick(elapsed);
            assert_eq!(bus.port_read(MSR, 1).unwrap(), u32::from(MSR_BUSY));
            assert!(!bus.device().irq_pending());
            assert_eq!(bus.device().next_event(VClock(elapsed)), NextEvent::Dormant);
            assert!(bus.device_mut().take_dma_request().is_none());
        }
        let saved = bus.device().save();
        bus.device_mut().restore(&saved).unwrap();
        assert_eq!(bus.device().save(), saved);
        bus_command(&mut bus, &[8]);
        assert_eq!(bus.port_read(MSR, 1).unwrap(), u32::from(MSR_BUSY));
        bus.port_write(DOR, 1, 0).unwrap();
        bus.port_write(DOR, 1, 12).unwrap();
        assert!(bus.device().irq_pending());
        bus_command(&mut bus, &[8]);
        assert_eq!(bus_results(&mut bus, 2), vec![ST0_POLL, 0]);
    }

    #[test]
    fn fidelity_reset_irq_obeys_dor_gate() {
        let mut bus = MockBus::new(Box::new(Upd765::new()));
        bus.port_write(DOR, 1, 4).unwrap();
        assert!(!bus.device().irq_pending());
        for drive in 0..4 {
            bus_command(&mut bus, &[0x08]);
            assert_eq!(bus_results(&mut bus, 2), vec![ST0_POLL | drive, 0]);
        }
    }

    use crate::dma8237::Dma8237;
    use device::DmaMemory;
    use std::collections::HashMap;

    /// NEC µPD765 datasheet, SPECIFY, SEEK, RECALIBRATE and READ/WRITE DATA:
    /// this functional model completes seeks/reset on access and transfers on
    /// DMA completion; SPECIFY timings do not create an autonomous phase.
    #[test]
    fn dormant_fdc_matches_per_clock_reset_seek_dma_and_traces() {
        let mut reference = MockBus::new(Box::new(Upd765::with_image(vec![0x5A; 512])));
        let mut scheduled = MockBus::new(Box::new(Upd765::with_image(vec![0x5A; 512])));
        for (index, bytes) in [
            vec![], // Release reset, then drain the four sense results.
            vec![0x03, 0xDF, 0x02],
            vec![0x0F, 0, 7],
            vec![0x07, 0],
            vec![0x46, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF],
            vec![0x45, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF],
        ]
        .into_iter()
        .enumerate()
        {
            let target = 1 + index as u64 * 13107;
            while reference.clock().0 < target {
                reference.tick(1);
            }
            assert_eq!(
                scheduled.device().next_event(VClock(target)),
                device::NextEvent::Dormant
            );
            scheduled.device_mut().sync(VClock(target));
            for bus in [&mut reference, &mut scheduled] {
                if bytes.is_empty() {
                    bus.port_write(DOR, 1, 0x1C).unwrap();
                    assert!(bus.device().irq_pending());
                    for _ in 0..4 {
                        bus_command(bus, &[0x08]);
                        bus_results(bus, 2);
                    }
                } else {
                    bus_command(bus, &bytes);
                    if matches!(index, 2 | 3) {
                        assert!(bus.device().irq_pending());
                        bus_command(bus, &[0x08]);
                        bus_results(bus, 2);
                    } else if index >= 4 {
                        assert!(!bus.device().irq_pending());
                        finish_test_dma(bus, 512, true);
                        assert!(bus.device().irq_pending());
                        bus_results(bus, 7);
                    }
                }
                assert_eq!(
                    bus.device().next_event(VClock(target)),
                    device::NextEvent::Dormant
                );
            }
            assert_eq!(
                reference.port_read(MSR, 1).unwrap(),
                scheduled.port_read(MSR, 1).unwrap()
            );
            assert_eq!(reference.device().save(), scheduled.device().save());
            assert_eq!(
                reference.device().irq_pending(),
                scheduled.device().irq_pending()
            );
            assert_eq!(
                reference.device_mut().take_trace(),
                scheduled.device_mut().take_trace()
            );
        }
    }

    /// A sparse mock guest memory implementing the SMC-safe DMA seam.
    #[derive(Default)]
    struct MockMem {
        cells: HashMap<u32, u8>,
    }

    impl DmaMemory for MockMem {
        fn dma_read(&self, addr: u32) -> u8 {
            self.cells.get(&addr).copied().unwrap_or(0)
        }
        fn dma_write(&mut self, addr: u32, value: u8) {
            self.cells.insert(addr, value);
        }
    }

    /// Issue a command opcode plus its parameters through the data port.
    fn command(fdc: &mut Upd765, bytes: &[u8]) {
        for &b in bytes {
            fdc.port_write(DATA, 1, u32::from(b)).unwrap();
        }
    }

    /// Read `n` result bytes back through the data port.
    fn results(fdc: &mut Upd765, n: usize) -> Vec<u8> {
        (0..n)
            .map(|_| fdc.port_read(DATA, 1).unwrap() as u8)
            .collect()
    }

    /// A controller with the given per-drive media, out of reset with the
    /// DMA/interrupt path enabled and the reset polls drained.
    fn ready_drives(drives: [Option<Drive>; 4]) -> Upd765 {
        let mut fdc = Upd765::with_drives(drives);
        // Leave reset (bit 2), enable DMA (bit 3), motor 0 on (bit 4), drive 0.
        fdc.port_write(DOR, 1, 0x1C).unwrap();
        // The reset raises IRQ6 and queues the four polling senses; drain them so
        // tests start from a clean interrupt state.
        for _ in 0..4 {
            command(&mut fdc, &[0x08]);
            let _ = results(&mut fdc, 2);
        }
        fdc.irq = false;
        fdc
    }

    /// A ready controller with a single 1.44 MB disk in drive 0 (single-drive setup).
    fn ready_fdc(image: Vec<u8>) -> Upd765 {
        ready_drives([
            Some(Drive::new(FloppyGeometry::HD1440, image)),
            None,
            None,
            None,
        ])
    }

    fn bus_command(bus: &mut MockBus, bytes: &[u8]) {
        for &byte in bytes {
            bus.port_write(DATA, 1, u32::from(byte)).unwrap();
        }
    }

    fn bus_results(bus: &mut MockBus, count: usize) -> Vec<u8> {
        (0..count)
            .map(|_| bus.port_read(DATA, 1).unwrap() as u8)
            .collect()
    }

    fn finish_test_dma(bus: &mut MockBus, transferred: usize, terminal_count: bool) {
        let request = bus.device_mut().take_dma_request().unwrap();
        let outcome = match request.op {
            DmaOp::ToMemory(_) => DmaOutcome::ToMemory {
                transferred,
                terminal_count,
            },
            DmaOp::FromMemory { .. } => DmaOutcome::FromMemory {
                bytes: vec![0x5A; transferred],
                terminal_count,
            },
        };
        bus.device_mut().dma_complete(outcome);
        assert!(bus.device().irq_pending());
    }

    #[test]
    fn dma_count_distinguishes_eot_and_missing_sector_from_tc() {
        for opcode in [0x06, 0x05, 0x86, 0x85] {
            for (sector, eot, fault) in [(1, 1, ST1_END_OF_CYLINDER), (18, 36, ST1_NO_DATA)] {
                for count in [511, 1023] {
                    let mut fdc = MockBus::new(Box::new(ready_fdc(Vec::new())));
                    let mut dma = Dma8237::new();
                    let mut mem = MockMem::default();
                    program_dma_from_memory(&mut dma, 0, count);
                    if opcode & 0x1F == 6 {
                        dma.port_write(0x0B, 1, 0x46).unwrap();
                    }
                    let mut dma = MockBus::new(Box::new(dma));
                    bus_command(&mut fdc, &[opcode, 4, 7, 1, sector, 2, eot, 0x1B, 0xFF]);
                    let request = fdc.device_mut().take_dma_request().unwrap();
                    let outcome = match request.op {
                        DmaOp::ToMemory(data) => {
                            let transfer = dma.device_mut().dma_service_write(
                                request.channel,
                                &mut mem,
                                &data,
                            );
                            assert_eq!(transfer.transferred, SECTOR_SIZE);
                            DmaOutcome::ToMemory {
                                transferred: transfer.transferred,
                                terminal_count: transfer.terminal_count,
                            }
                        }
                        DmaOp::FromMemory { len } => {
                            let mut bytes = vec![0; len];
                            let transfer = dma.device_mut().dma_service_read(
                                request.channel,
                                &mut mem,
                                &mut bytes,
                            );
                            assert_eq!(transfer.transferred, SECTOR_SIZE);
                            bytes.truncate(transfer.transferred);
                            DmaOutcome::FromMemory {
                                bytes,
                                terminal_count: transfer.terminal_count,
                            }
                        }
                    };
                    fdc.device_mut().dma_complete(outcome);
                    assert!(fdc.device().irq_pending());
                    let result = bus_results(&mut fdc, 7);
                    assert_eq!(
                        result[..3],
                        if count == 511 {
                            [4, 0, 0]
                        } else {
                            [ST0_ABNORMAL | 4, fault, 0]
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn dma_eot_without_tc_reports_end_of_cylinder() {
        for opcode in [0x06, 0x05, 0x86, 0x85] {
            for tc in [false, true] {
                let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
                bus_command(&mut bus, &[opcode, 4, 7, 1, 1, 2, 1, 0x1B, 0xFF]);
                finish_test_dma(&mut bus, SECTOR_SIZE, tc);
                let result = bus_results(&mut bus, 7);
                assert_eq!(
                    &result[..3],
                    &[
                        4 | if tc { 0 } else { ST0_ABNORMAL },
                        if tc { 0 } else { 0x80 },
                        0
                    ]
                );
                if tc {
                    assert_eq!(
                        &result[3..],
                        &[8, if opcode & 0x80 != 0 { 0 } else { 1 }, 1, 2]
                    );
                }
            }
        }
    }

    #[test]
    fn dma_short_without_tc_reports_overrun() {
        for opcode in [0x06, 0x05, 0x86, 0x85] {
            for transferred in [0, 127, SECTOR_SIZE, SECTOR_SIZE + 127] {
                let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
                bus_command(&mut bus, &[opcode, 4, 7, 1, 1, 2, 2, 0x1B, 0xFF]);
                finish_test_dma(&mut bus, transferred, false);
                assert_eq!(&bus_results(&mut bus, 7)[..3], &[ST0_ABNORMAL | 4, 0x10, 0]);
            }
        }
    }

    #[test]
    fn dma_missing_sector_after_prefix_without_tc_reports_no_data() {
        for opcode in [0x06, 0x05, 0x86, 0x85] {
            let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
            bus_command(&mut bus, &[opcode, 0, 7, 0, 18, 2, 36, 0x1B, 0xFF]);
            finish_test_dma(&mut bus, SECTOR_SIZE, false);
            assert_eq!(
                bus_results(&mut bus, 7),
                [ST0_ABNORMAL, ST1_NO_DATA, 0, 7, 0, 19, 2]
            );
        }
    }

    #[test]
    fn dma_tc_before_missing_sector_preserves_normal_result() {
        for opcode in [0x06, 0x05, 0x86, 0x85] {
            for transferred in [127, SECTOR_SIZE] {
                let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
                bus_command(&mut bus, &[opcode, 0, 7, 0, 1, 2, 36, 0x1B, 0xFF]);
                finish_test_dma(&mut bus, transferred, true);
                let result = bus_results(&mut bus, 7);
                assert_eq!(&result[..3], &[0, 0, 0]);
                let sectors = if opcode & 0x1F == 5 {
                    transferred.div_ceil(SECTOR_SIZE)
                } else {
                    transferred / SECTOR_SIZE
                };
                assert_eq!(&result[3..], &[7, 0, 1 + sectors as u8, 2]);
            }
        }
    }

    #[test]
    fn sense_without_pending_interrupt_returns_one_invalid_byte() {
        let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
        // Consume a seek completion so the controller has a stale ST0 and PCN.
        bus_command(&mut bus, &[0x0F, 0x00, 25]);
        bus_command(&mut bus, &[0x08]);
        assert_eq!(bus_results(&mut bus, 2), [ST0_SEEK_END, 25]);
        for _ in 0..2 {
            bus_command(&mut bus, &[0x08]);
            assert_eq!(
                bus.port_read(MSR, 1).unwrap() as u8,
                MSR_RQM | MSR_DIO | MSR_BUSY
            );
            assert_eq!(bus_results(&mut bus, 1), [0x80]);
            assert_eq!(bus.port_read(MSR, 1).unwrap() as u8, MSR_RQM);
            assert!(!bus.device().irq_pending());
        }
    }

    #[test]
    fn data_result_at_eot_advances_chrn_for_both_heads_and_mt_modes() {
        for opcode in [0x06, 0x05] {
            for (mt, head, expected_c, expected_h) in
                [(0, 0, 8, 0), (0, 1, 8, 1), (0x80, 0, 7, 1), (0x80, 1, 8, 0)]
            {
                let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
                bus_command(
                    &mut bus,
                    &[mt | opcode, head << 2, 7, head, 2, 2, 3, 0x1B, 0xFF],
                );
                let request = bus.device_mut().take_dma_request().unwrap();
                assert_eq!(request.channel, FLOPPY_DMA_CHANNEL);
                let outcome = match request.op {
                    DmaOp::ToMemory(_) => DmaOutcome::ToMemory {
                        transferred: 2 * SECTOR_SIZE,
                        terminal_count: true,
                    },
                    DmaOp::FromMemory { .. } => DmaOutcome::FromMemory {
                        bytes: vec![0x5A; 2 * SECTOR_SIZE],
                        terminal_count: true,
                    },
                };
                bus.device_mut().dma_complete(outcome);
                let result = bus_results(&mut bus, 7);
                assert_eq!(
                    &result[1..],
                    &[0, 0, expected_c, expected_h, 1, 2],
                    "opcode={opcode:#x}, MT={mt:#x}, head={head}"
                );
                assert_eq!(result[0] & 0xC0, 0);
                assert_eq!(bus.port_read(MSR, 1).unwrap() as u8, MSR_RQM);
            }
        }
    }

    #[test]
    fn data_result_before_eot_keeps_existing_bytes() {
        for opcode in [0x06, 0x05, 0x86, 0x85, 0xE6, 0xC5] {
            for (sector, eot) in [(2, 4), (18, 36)] {
                let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
                bus_command(&mut bus, &[opcode, 0x04, 7, 1, sector, 2, eot, 0x1B, 0xFF]);
                let request = bus.device_mut().take_dma_request().unwrap();
                let outcome = match request.op {
                    DmaOp::ToMemory(_) => DmaOutcome::ToMemory {
                        transferred: SECTOR_SIZE,
                        terminal_count: true,
                    },
                    DmaOp::FromMemory { .. } => DmaOutcome::FromMemory {
                        bytes: vec![0x5A; SECTOR_SIZE],
                        terminal_count: true,
                    },
                };
                bus.device_mut().dma_complete(outcome);
                assert_eq!(bus_results(&mut bus, 7), [4, 0, 0, 7, 1, sector + 1, 2]);
            }
        }
    }

    #[test]
    fn multitrack_read_continues_at_sector_one_on_head_one() {
        let geometry = FloppyGeometry::DD360;
        let image = patterned(geometry, 0xA3);
        let mut bus = MockBus::new(Box::new(ready_drives([
            Some(Drive::new(geometry, image.clone())),
            None,
            None,
            None,
        ])));
        // EOT is smaller than the physical track: continuation must skip the
        // remaining sectors on head 0 rather than use a contiguous image slice.
        bus_command(&mut bus, &[0x86, 0, 7, 0, 2, 2, 3, 0x1B, 0xFF]);
        let request = bus.device_mut().take_dma_request().unwrap();
        let DmaOp::ToMemory(data) = request.op else {
            panic!("READ DMA direction")
        };
        let mut expected = Vec::new();
        for (head, sector) in [(0, 2), (0, 3), (1, 1), (1, 2), (1, 3)] {
            let offset = geometry.chs_to_offset(7, head, sector);
            expected.extend_from_slice(&image[offset..offset + SECTOR_SIZE]);
        }
        assert_eq!(data.len(), expected.len());
        assert_eq!(data, expected);
        bus.device_mut().dma_complete(DmaOutcome::ToMemory {
            transferred: data.len(),
            terminal_count: true,
        });
        let result = bus_results(&mut bus, 7);
        assert_eq!(&result[1..], &[0, 0, 8, 0, 1, 2]);
    }

    #[test]
    fn multitrack_write_continues_on_head_one_without_touching_track_tail() {
        let geometry = FloppyGeometry::DD360;
        let original = patterned(geometry, 0xC7);
        let mut bus = MockBus::new(Box::new(ready_drives([
            Some(Drive::new(geometry, original.clone())),
            None,
            None,
            None,
        ])));
        bus_command(&mut bus, &[0x85, 0, 7, 0, 2, 2, 3, 0x1B, 0xFF]);
        let request = bus.device_mut().take_dma_request().unwrap();
        assert_eq!(
            request.op,
            DmaOp::FromMemory {
                len: 5 * SECTOR_SIZE
            }
        );
        let mut expected = original;
        let mut bytes = Vec::new();
        for (index, (head, sector)) in [(0, 2), (0, 3), (1, 1), (1, 2), (1, 3)]
            .into_iter()
            .enumerate()
        {
            let data = vec![0x30 + index as u8; SECTOR_SIZE];
            bytes.extend_from_slice(&data);
            let offset = geometry.chs_to_offset(7, head, sector);
            expected[offset..offset + SECTOR_SIZE].copy_from_slice(&data);
        }
        bus.device_mut().dma_complete(DmaOutcome::FromMemory {
            bytes,
            terminal_count: true,
        });
        assert_eq!(&bus_results(&mut bus, 7)[1..], &[0, 0, 8, 0, 1, 2]);
        // Read every sector of both tracks through the public DMA seam.
        for head in 0..2 {
            bus_command(&mut bus, &[0x06, head << 2, 7, head, 1, 2, 9, 0x1B, 0xFF]);
            let DmaOp::ToMemory(data) = bus.device_mut().take_dma_request().unwrap().op else {
                panic!("READ DMA direction")
            };
            let offset = geometry.chs_to_offset(7, head, 1);
            assert_eq!(data, expected[offset..offset + 9 * SECTOR_SIZE]);
            bus.device_mut().dma_complete(DmaOutcome::ToMemory {
                transferred: data.len(),
                terminal_count: true,
            });
            let _ = bus_results(&mut bus, 7);
        }
    }

    #[test]
    fn data_stops_at_physical_track_without_aliasing_next_track() {
        for opcode in [0x06, 0x05, 0x86, 0x85] {
            let geometry = FloppyGeometry::DD360;
            let image = patterned(geometry, 0x5C);
            let mut bus = MockBus::new(Box::new(ready_drives([
                Some(Drive::new(geometry, image.clone())),
                None,
                None,
                None,
            ])));
            bus_command(&mut bus, &[opcode, 0, 7, 0, 9, 2, 10, 0x1B, 0xFF]);
            let request = bus.device_mut().take_dma_request().unwrap();
            let outcome = match request.op {
                DmaOp::ToMemory(data) => {
                    let offset = geometry.chs_to_offset(7, 0, 9);
                    assert_eq!(data.len(), SECTOR_SIZE, "no sector 10 from another track");
                    assert_eq!(data, image[offset..offset + SECTOR_SIZE]);
                    DmaOutcome::ToMemory {
                        transferred: data.len(),
                        terminal_count: true,
                    }
                }
                DmaOp::FromMemory { len } => {
                    assert_eq!(len, SECTOR_SIZE, "no write into another track");
                    DmaOutcome::FromMemory {
                        bytes: vec![0xAA; len],
                        terminal_count: true,
                    }
                }
            };
            bus.device_mut().dma_complete(outcome);
            assert!(bus.device().irq_pending());
            // TC on the last valid sector prevents a search for the absent next ID.
            assert_eq!(bus_results(&mut bus, 7), [0, 0, 0, 7, 0, 10, 2]);
            // The next track remains byte-identical even after WRITE DATA.
            bus_command(&mut bus, &[0x06, 4, 7, 1, 1, 2, 1, 0x1B, 0xFF]);
            let DmaOp::ToMemory(data) = bus.device_mut().take_dma_request().unwrap().op else {
                panic!("READ DMA direction")
            };
            let offset = geometry.chs_to_offset(7, 1, 1);
            assert_eq!(data, image[offset..offset + SECTOR_SIZE]);
            bus.device_mut().dma_complete(DmaOutcome::ToMemory {
                transferred: data.len(),
                terminal_count: true,
            });
            let _ = bus_results(&mut bus, 7);
        }
    }

    #[test]
    fn data_starting_past_physical_track_faults_without_dma() {
        for opcode in [0x06, 0x05] {
            let mut bus = MockBus::new(Box::new(ready_fdc(Vec::new())));
            bus_command(&mut bus, &[opcode, 4, 7, 1, 19, 2, 19, 0x1B, 0xFF]);
            assert!(bus.device_mut().take_dma_request().is_none());
            assert_eq!(
                bus_results(&mut bus, 7),
                [ST0_ABNORMAL | 4, ST1_NO_DATA, 0, 7, 1, 19, 2]
            );
        }
    }

    #[test]
    fn power_on_state_is_idle_and_ready() {
        let fdc = Upd765::new();
        assert_eq!(
            fdc.msr(),
            MSR_RQM,
            "idle controller reports RQM ready, DIO host"
        );
        assert!(!fdc.irq_pending());
    }

    #[test]
    fn chs_maps_to_image_offset() {
        let g = FloppyGeometry::HD1440;
        assert_eq!(g.chs_to_offset(0, 0, 1), 0);
        assert_eq!(g.chs_to_offset(0, 0, 2), SECTOR_SIZE);
        assert_eq!(g.chs_to_offset(0, 1, 1), 18 * SECTOR_SIZE);
        assert_eq!(g.chs_to_offset(1, 0, 1), 36 * SECTOR_SIZE);
    }

    #[test]
    fn chs_offset_and_size_track_the_geometry() {
        // 360 KB has 9 sectors per track over 40 cylinders, so head 1 and the
        // next cylinder sit at different offsets than a 1.44 MB disk would.
        let g = FloppyGeometry::DD360;
        assert_eq!(g.image_bytes(), 40 * 2 * 9 * SECTOR_SIZE);
        assert_eq!(g.chs_to_offset(0, 0, 1), 0);
        assert_eq!(g.chs_to_offset(0, 1, 1), 9 * SECTOR_SIZE);
        assert_eq!(g.chs_to_offset(1, 0, 1), 18 * SECTOR_SIZE);

        // 1.2 MB and 720 KB image sizes.
        assert_eq!(
            FloppyGeometry::HD1200.image_bytes(),
            80 * 2 * 15 * SECTOR_SIZE
        );
        assert_eq!(
            FloppyGeometry::DD720.image_bytes(),
            80 * 2 * 9 * SECTOR_SIZE
        );
        assert_eq!(FloppyGeometry::HD1440.image_bytes(), FLOPPY_IMAGE_BYTES);

        // A cylinder past a 40-cylinder disk, or a sector past the track, is out.
        assert!(!FloppyGeometry::DD360.contains(40, 0, 1));
        assert!(!FloppyGeometry::DD360.contains(0, 0, 10));
        assert!(!FloppyGeometry::DD360.contains(0, 2, 1));
        assert!(FloppyGeometry::DD360.contains(39, 1, 9));
    }

    #[test]
    fn dor_reset_edge_raises_irq_and_queues_polls() {
        let mut fdc = Upd765::new();
        fdc.port_write(DOR, 1, 0x1C).unwrap();
        assert!(fdc.irq_pending(), "leaving reset raises IRQ6");
        assert_eq!(fdc.reset_poll, 4);
        // Four SENSE INTERRUPT STATUS calls report the polling status.
        for i in 0..4 {
            command(&mut fdc, &[0x08]);
            let res = results(&mut fdc, 2);
            assert_eq!(res[0], ST0_POLL | i, "poll {i} reports abnormal status");
            assert_eq!(res[1], 0, "present cylinder 0 after reset");
        }
        assert!(!fdc.irq_pending(), "the interrupt clears once sensed");
    }

    #[test]
    fn specify_has_no_result_and_no_irq() {
        let mut fdc = ready_fdc(Vec::new());
        command(&mut fdc, &[0x03, 0xDF, 0x02]);
        assert_eq!(fdc.msr(), MSR_RQM, "back to idle after SPECIFY");
        assert!(!fdc.irq_pending(), "SPECIFY raises no interrupt");
    }

    #[test]
    fn recalibrate_seeks_to_zero_and_interrupts() {
        let mut fdc = ready_fdc(Vec::new());
        // Move the drive off track 0 first so recalibrate is observable.
        command(&mut fdc, &[0x0F, 0x00, 40]); // SEEK drive 0 -> cyl 40
        command(&mut fdc, &[0x08]);
        let _ = results(&mut fdc, 2);
        fdc.irq = false;
        command(&mut fdc, &[0x07, 0x00]); // RECALIBRATE drive 0
        assert!(fdc.irq_pending(), "RECALIBRATE raises IRQ6");
        command(&mut fdc, &[0x08]); // SENSE INTERRUPT STATUS
        let res = results(&mut fdc, 2);
        assert_eq!(res[0], ST0_SEEK_END, "seek-end for drive 0");
        assert_eq!(res[1], 0, "recalibrated to cylinder 0");
        assert_eq!(fdc.present_cylinder(0), 0);
    }

    #[test]
    fn seek_sets_present_cylinder_and_interrupts() {
        let mut fdc = ready_fdc(Vec::new());
        command(&mut fdc, &[0x0F, 0x00, 25]); // SEEK drive 0 -> cyl 25
        assert!(fdc.irq_pending(), "SEEK raises IRQ6");
        assert_eq!(fdc.present_cylinder(0), 25);
        command(&mut fdc, &[0x08]);
        let res = results(&mut fdc, 2);
        assert_eq!(res[0], ST0_SEEK_END, "seek-end, drive 0, head 0");
        assert_eq!(res[1], 25, "present cylinder 25");
        assert!(!fdc.irq_pending());
    }

    #[test]
    fn read_id_returns_current_position() {
        let mut fdc = ready_fdc(Vec::new());
        command(&mut fdc, &[0x0F, 0x00, 7]); // SEEK to cyl 7
        command(&mut fdc, &[0x08]);
        let _ = results(&mut fdc, 2);
        fdc.irq = false;
        command(&mut fdc, &[0x4A, 0x00]); // READ ID, drive 0 head 0
        assert!(fdc.irq_pending());
        let res = results(&mut fdc, 7);
        assert_eq!(res[3], 7, "C = present cylinder");
        assert_eq!(res[5], 1, "R = 1");
        assert_eq!(res[6], 2, "N = 2 (512 bytes)");
        assert!(
            !fdc.irq_pending(),
            "reading the result acknowledges the interrupt"
        );
    }

    #[test]
    fn read_data_queues_the_sector_and_raises_irq() {
        // Seed a distinctive first sector.
        let mut image = vec![0u8; SECTOR_SIZE];
        for (i, b) in image.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let mut fdc = ready_fdc(image.clone());

        // READ DATA: drive/head 0, C=0, H=0, R=1, N=2, EOT=1, GPL, DTL.
        command(
            &mut fdc,
            &[0x06, 0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        assert_eq!(
            fdc.phase,
            Phase::Execution,
            "READ enters execution awaiting DMA"
        );
        assert!(
            !fdc.irq_pending(),
            "no interrupt until the transfer completes"
        );

        let request = fdc.take_dma_request().expect("a DMA request is queued");
        assert_eq!(request.channel, FLOPPY_DMA_CHANNEL);
        assert_eq!(
            request.op,
            DmaOp::ToMemory(image.clone()),
            "the queued bytes are sector 0, moved device -> memory"
        );
        assert!(
            fdc.take_dma_request().is_none(),
            "the request is taken once"
        );

        fdc.dma_complete(DmaOutcome::ToMemory {
            transferred: SECTOR_SIZE,
            terminal_count: true,
        });
        assert!(fdc.irq_pending(), "IRQ6 rises on transfer completion");
        let res = results(&mut fdc, 7);
        assert_eq!(res[0] & 0xC0, 0, "ST0 normal termination");
        assert_eq!(res[3], 1, "result cylinder advances at EOT");
        assert_eq!(res[5], 1, "result sector resets at EOT");
        assert_eq!(res[6], 2, "N echoed");
    }

    #[test]
    fn read_data_transfers_through_the_8237_into_memory() {
        let mut image = vec![0u8; SECTOR_SIZE];
        for (i, b) in image.iter_mut().enumerate() {
            *b = (0xA0 ^ i) as u8;
        }
        let mut fdc = ready_fdc(image.clone());
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();

        // Program 8237 channel 2 for a write transfer (device -> memory) to
        // physical 0x07C00, count 511 (512 bytes).
        dma.port_write(0x0A, 1, 0x06).unwrap(); // mask channel 2
        dma.port_write(0x0C, 1, 0).unwrap(); // clear flip-flop
        dma.port_write(0x0B, 1, 0x46).unwrap(); // mode: ch2, write, single
        dma.port_write(0x04, 1, 0x00).unwrap(); // addr low
        dma.port_write(0x04, 1, 0x7C).unwrap(); // addr high
        dma.port_write(0x81, 1, 0x00).unwrap(); // page (channel 2)
        dma.port_write(0x05, 1, 0xFF).unwrap(); // count low
        dma.port_write(0x05, 1, 0x01).unwrap(); // count high
        dma.port_write(0x0A, 1, 0x02).unwrap(); // unmask channel 2

        command(
            &mut fdc,
            &[0x06, 0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        let request = fdc.take_dma_request().expect("queued request");
        let DmaOp::ToMemory(data) = request.op else {
            panic!("a floppy read queues a device -> memory transfer");
        };
        let n = dma.dma_service_write(request.channel, &mut mem, &data);
        assert_eq!(n.transferred, SECTOR_SIZE, "the whole sector transfers");
        fdc.dma_complete(DmaOutcome::ToMemory {
            transferred: n.transferred,
            terminal_count: n.terminal_count,
        });

        for (i, &expected) in image.iter().enumerate() {
            assert_eq!(
                mem.dma_read(0x07C00 + i as u32),
                expected,
                "byte {i} in memory"
            );
        }
        assert!(fdc.irq_pending());
    }

    /// Program an 8237 channel 2 for a memory→device (read) transfer at physical
    /// `address`, moving `count + 1` bytes — the direction a floppy WRITE drains.
    fn program_dma_from_memory(dma: &mut Dma8237, address: u32, count: u16) {
        let addr = address as u16;
        dma.port_write(0x0A, 1, 0x06).unwrap(); // mask channel 2
        dma.port_write(0x0C, 1, 0).unwrap(); // clear flip-flop
        dma.port_write(0x0B, 1, 0x4A).unwrap(); // mode: ch2, read, single
        dma.port_write(0x04, 1, u32::from(addr & 0xFF)).unwrap(); // addr low
        dma.port_write(0x04, 1, u32::from(addr >> 8)).unwrap(); // addr high
        dma.port_write(0x81, 1, address >> 16).unwrap(); // page (channel 2)
        dma.port_write(0x05, 1, u32::from(count & 0xFF)).unwrap(); // count low
        dma.port_write(0x05, 1, u32::from(count >> 8)).unwrap(); // count high
        dma.port_write(0x0A, 1, 0x02).unwrap(); // unmask channel 2
    }

    #[test]
    fn write_data_commits_memory_then_reads_back_identical() {
        // Drive 0 starts with an empty image, so the write must also grow the
        // backing buffer to hold the sector.
        let mut fdc = ready_fdc(Vec::new());
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();

        // A distinctive sector sitting in guest memory at physical 0x07C00.
        let pattern: Vec<u8> = (0..SECTOR_SIZE).map(|i| (0x5A ^ i) as u8).collect();
        for (i, &b) in pattern.iter().enumerate() {
            mem.dma_write(0x07C00 + i as u32, b);
        }
        program_dma_from_memory(&mut dma, 0x07C00, (SECTOR_SIZE - 1) as u16);

        // WRITE DATA: drive/head 0, C=0, H=0, R=1, N=2, EOT=1.
        command(
            &mut fdc,
            &[0x05, 0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        assert_eq!(
            fdc.msr(),
            MSR_BUSY,
            "the write execution phase reports BUSY only (host drives no data port)"
        );
        assert!(
            !fdc.irq_pending(),
            "no interrupt until the transfer completes"
        );

        let request = fdc
            .take_dma_request()
            .expect("a write queues a DMA request");
        let DmaOp::FromMemory { len } = request.op else {
            panic!("a floppy write queues a memory -> device transfer");
        };
        assert_eq!(len, SECTOR_SIZE, "one sector requested from memory");
        let mut buf = vec![0u8; len];
        let n = dma.dma_service_read(request.channel, &mut mem, &mut buf);
        assert_eq!(n.transferred, SECTOR_SIZE, "the whole sector transfers");
        buf.truncate(n.transferred);
        fdc.dma_complete(DmaOutcome::FromMemory {
            bytes: buf,
            terminal_count: n.terminal_count,
        });

        assert!(fdc.irq_pending(), "IRQ6 rises on write completion");
        let res = results(&mut fdc, 7);
        assert_eq!(res[0] & 0xC0, 0, "ST0 normal termination");
        assert_eq!(&res[3..], &[1, 0, 1, 2], "result ID advances at EOT");

        // Read the same CHS back through the controller: the bytes committed to
        // media are byte-identical to what was written from memory.
        let read_back = read_sector(&mut fdc, 0x00, 0, 0, 1);
        assert_eq!(
            read_back, pattern,
            "the sector reads back exactly as written"
        );
    }

    #[test]
    fn write_out_of_geometry_faults_no_data() {
        // 360 KB disk has 40 cylinders; cylinder 40 is one past the last.
        let mut fdc = ready_drives([
            Some(Drive::new(FloppyGeometry::DD360, vec![0u8; SECTOR_SIZE])),
            None,
            None,
            None,
        ]);
        command(
            &mut fdc,
            &[0x05, 0x00, 40, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        assert!(
            fdc.take_dma_request().is_none(),
            "an out-of-geometry write queues no transfer"
        );
        let res = results(&mut fdc, 7);
        assert_eq!(res[0] & 0x40, 0x40, "ST0 abnormal termination");
        assert_eq!(
            res[1] & 0x04,
            0x04,
            "ST1 ND: no data field past the geometry"
        );
    }

    #[test]
    fn write_to_protected_disk_faults_not_writable() {
        let original = vec![0xAAu8; SECTOR_SIZE];
        let mut fdc = ready_drives([
            Some(Drive::new_read_only(
                FloppyGeometry::HD1440,
                original.clone(),
            )),
            None,
            None,
            None,
        ]);
        command(
            &mut fdc,
            &[0x05, 0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        assert!(
            fdc.take_dma_request().is_none(),
            "a protected disk queues no transfer"
        );
        let res = results(&mut fdc, 7);
        assert_eq!(res[0] & 0x40, 0x40, "ST0 abnormal termination");
        assert_eq!(res[1] & 0x02, 0x02, "ST1 NW: not writable");
        assert_eq!(
            fdc.drives[0].as_ref().map(|d| &d.image),
            Some(&original),
            "the image is not mutated"
        );
    }

    #[test]
    fn writeterminal_count_zero_fills_partial_sector() {
        // Drive 0 starts empty; a short write must grow the image and commit a
        // whole sector even though only part of it was supplied.
        let mut fdc = ready_fdc(Vec::new());
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();

        const SUPPLIED: usize = 100;
        let pattern: Vec<u8> = (0..SUPPLIED).map(|i| (0xC3 ^ i) as u8).collect();
        for (i, &b) in pattern.iter().enumerate() {
            mem.dma_write(0x07C00 + i as u32, b);
        }
        // Program only 100 bytes (count 99): terminal count cuts the sector short.
        program_dma_from_memory(&mut dma, 0x07C00, (SUPPLIED - 1) as u16);

        command(
            &mut fdc,
            &[0x05, 0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        let request = fdc
            .take_dma_request()
            .expect("a write queues a DMA request");
        let DmaOp::FromMemory { len } = request.op else {
            panic!("a floppy write queues a memory -> device transfer");
        };
        let mut buf = vec![0u8; len];
        let n = dma.dma_service_read(request.channel, &mut mem, &mut buf);
        assert_eq!(
            n.transferred, SUPPLIED,
            "terminal count clamps the transfer"
        );
        buf.truncate(n.transferred);
        fdc.dma_complete(DmaOutcome::FromMemory {
            bytes: buf,
            terminal_count: n.terminal_count,
        });

        let res = results(&mut fdc, 7);
        assert_eq!(&res[3..], &[1, 0, 1, 2], "the completed sector reaches EOT");
        let image = &fdc.drives[0].as_ref().unwrap().image;
        assert_eq!(
            image.len(),
            SECTOR_SIZE,
            "the short image grew to hold the sector"
        );
        assert_eq!(&image[..SUPPLIED], &pattern[..], "supplied bytes committed");
        assert!(
            image[SUPPLIED..SECTOR_SIZE].iter().all(|&b| b == 0),
            "the tail of the partial sector is zero-filled"
        );
    }

    #[test]
    fn unknown_command_reports_invalid() {
        let mut fdc = ready_fdc(Vec::new());
        command(&mut fdc, &[0x1D]); // not a modelled opcode
        let res = results(&mut fdc, 1);
        assert_eq!(res[0], 0x80, "invalid-command status");
    }

    #[test]
    fn reset_preserves_the_disk_but_clears_registers() {
        let image = vec![0x5Au8; SECTOR_SIZE];
        let mut fdc = ready_fdc(image.clone());
        command(&mut fdc, &[0x0F, 0x00, 30]); // SEEK to cyl 30
        fdc.reset();
        assert_eq!(fdc.present_cylinder(0), 0, "registers cleared");
        assert_eq!(fdc.dor, 0);
        assert!(!fdc.irq_pending());
        assert_eq!(
            fdc.drives[0].as_ref().map(|d| &d.image),
            Some(&image),
            "the disk stays attached across reset"
        );
    }

    #[test]
    fn save_restore_round_trip() {
        let mut fdc = ready_fdc(vec![0u8; SECTOR_SIZE]);
        command(&mut fdc, &[0x0F, 0x00, 42]); // SEEK, leaves an interrupt pending
        let blob = fdc.save();

        let mut restored = Upd765::new();
        restored.restore(&blob).unwrap();
        assert_eq!(restored.present_cylinder(0), 42);
        assert_eq!(restored.dor, fdc.dor);
        assert_eq!(restored.irq_pending(), fdc.irq_pending());

        // A pending SENSE resolves identically after restore.
        command(&mut restored, &[0x08]);
        let res = results(&mut restored, 2);
        assert_eq!(res[1], 42);
    }

    #[test]
    fn restore_rejects_bad_snapshot() {
        let mut fdc = Upd765::new();
        assert!(matches!(
            fdc.restore(&[0xFF]),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert!(matches!(
            fdc.restore(&[]),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
    }

    #[test]
    fn mock_bus_isolation_harness() {
        let mut bus = MockBus::new(Box::new(Upd765::new()));
        bus.tick(1);
        // MSR reads ready at power-on through the bus decode.
        assert_eq!(bus.port_read(MSR, 1).unwrap() as u8, MSR_RQM);
        // A DOR write out of reset raises the IRQ line, visible through the seam.
        bus.port_write(DOR, 1, 0x1C).unwrap();
        assert!(bus.device().irq_pending());
    }

    #[test]
    fn port_outside_window_is_unmapped() {
        let mut fdc = Upd765::new();
        assert!(matches!(
            fdc.port_read(0x3E0, 1),
            Err(DeviceError::UnmappedRegister { kind: "port", .. })
        ));
    }

    /// A distinctive full-geometry image whose every byte is a function of its
    /// offset and a per-disk seed, so a read landing on the wrong offset or the
    /// wrong drive is caught byte-for-byte.
    fn patterned(geometry: FloppyGeometry, seed: u8) -> Vec<u8> {
        let mut img = vec![0u8; geometry.image_bytes()];
        for (i, b) in img.iter_mut().enumerate() {
            *b = seed ^ (i % 251) as u8;
        }
        img
    }

    /// Issue a single-sector READ DATA, run the DMA to completion, drain the
    /// result phase, and return the sector bytes the controller pulled.
    fn read_sector(fdc: &mut Upd765, drive_head: u8, c: u8, h: u8, r: u8) -> Vec<u8> {
        command(fdc, &[0x06, drive_head, c, h, r, 0x02, r, 0x1B, 0xFF]);
        let DmaOp::ToMemory(data) = fdc
            .take_dma_request()
            .expect("a valid read queues a DMA request")
            .op
        else {
            panic!("a floppy read queues a device -> memory transfer");
        };
        fdc.dma_complete(DmaOutcome::ToMemory {
            transferred: data.len(),
            terminal_count: true,
        });
        let _ = results(fdc, 7);
        data
    }

    #[test]
    fn read_uses_the_drive_geometry_for_the_offset() {
        // Read cylinder 1 on both a 360 KB and a 1.44 MB disk: the same CHS lands
        // at different image offsets because the geometries differ.
        for geometry in [
            FloppyGeometry::DD360,
            FloppyGeometry::DD720,
            FloppyGeometry::HD1200,
            FloppyGeometry::HD1440,
        ] {
            let image = patterned(geometry, 0x33);
            let mut fdc =
                ready_drives([Some(Drive::new(geometry, image.clone())), None, None, None]);
            let data = read_sector(&mut fdc, 0x00, 1, 0, 1);
            let offset = geometry.chs_to_offset(1, 0, 1);
            assert_eq!(
                data,
                image[offset..offset + SECTOR_SIZE],
                "read must use {geometry:?} to compute the offset"
            );
        }
    }

    #[test]
    fn drives_zero_and_one_carry_independent_media() {
        let img0 = patterned(FloppyGeometry::DD360, 0x11);
        let img1 = patterned(FloppyGeometry::HD1440, 0xEE);
        let mut fdc = ready_drives([
            Some(Drive::new(FloppyGeometry::DD360, img0.clone())),
            Some(Drive::new(FloppyGeometry::HD1440, img1.clone())),
            None,
            None,
        ]);

        // Drive 0 (drive-select bits 00) reads its own 360 KB media at the 360 KB
        // offset for C=1; drive 1 (bits 01) reads its 1.44 MB media at the 1.44 MB
        // offset for the same CHS. Neither sees the other's disk.
        let d0 = read_sector(&mut fdc, 0x00, 1, 0, 1);
        let o0 = FloppyGeometry::DD360.chs_to_offset(1, 0, 1);
        assert_eq!(d0, img0[o0..o0 + SECTOR_SIZE], "drive 0 reads 360 KB media");

        let d1 = read_sector(&mut fdc, 0x01, 1, 0, 1);
        let o1 = FloppyGeometry::HD1440.chs_to_offset(1, 0, 1);
        assert_eq!(
            d1,
            img1[o1..o1 + SECTOR_SIZE],
            "drive 1 reads 1.44 MB media"
        );
        assert_ne!(d0, d1, "the two drives return different bytes");
    }

    #[test]
    fn read_outside_geometry_terminates_abnormally() {
        // Cylinder 60 does not exist on a 40-cylinder 360 KB disk.
        let image = patterned(FloppyGeometry::DD360, 0x22);
        let mut fdc = ready_drives([
            Some(Drive::new(FloppyGeometry::DD360, image)),
            None,
            None,
            None,
        ]);
        command(
            &mut fdc,
            &[0x06, 0x00, 60, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        assert!(
            fdc.take_dma_request().is_none(),
            "an out-of-geometry read moves no data"
        );
        let res = results(&mut fdc, 7);
        assert_eq!(res[0] & 0xC0, ST0_ABNORMAL, "ST0 abnormal termination");
        assert_eq!(res[1] & ST1_NO_DATA, ST1_NO_DATA, "ST1 no-data");
    }

    /// Build a real FAT12 360 KB image with `mformat` (from the devShell's
    /// mtools), attach it as a 360 KB drive, and read sector 0 through the full
    /// command path — proving a non-1.44 MB geometry reads a genuine on-disk
    /// filesystem, boot signature and all.
    #[test]
    fn reads_a_real_fat12_360k_image() {
        use std::process::Command;

        let path = std::env::temp_dir().join(format!(
            "upd765_360k_{}_{}.img",
            std::process::id(),
            line!()
        ));
        let out = match Command::new("mformat")
            .args(["-C", "-f", "360", "-v", "TEST360", "-i"])
            .arg(&path)
            .arg("::")
            .output()
        {
            Ok(out) => out,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let message = "missing mformat — run this test in nix develop (mtools)";
                assert!(
                    std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "{message}"
                );
                eprintln!("skipping: {message}");
                return;
            }
            Err(error) => panic!("cannot run mformat: {error}"),
        };
        assert!(
            out.status.success(),
            "mformat failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let image = std::fs::read(&path).expect("read the freshly formatted image");
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            image.len(),
            FloppyGeometry::DD360.image_bytes(),
            "a 360 KB image is exactly 40*2*9*512 bytes"
        );

        let mut fdc = ready_drives([
            Some(Drive::new(FloppyGeometry::DD360, image.clone())),
            None,
            None,
            None,
        ]);

        // Sector 1 (LBA 0) is the boot sector; mformat writes the 0x55 0xAA
        // signature at the end of it.
        let boot = read_sector(&mut fdc, 0x00, 0, 0, 1);
        assert_eq!(boot.len(), SECTOR_SIZE);
        assert_eq!(
            &boot[..],
            &image[..SECTOR_SIZE],
            "sector 0 matches the image"
        );
        assert_eq!(
            (boot[510], boot[511]),
            (0x55, 0xAA),
            "the FAT12 boot sector carries the boot signature"
        );
    }
}
