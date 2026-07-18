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
//! SMC-safe `note_write` path, §4.3), and reports the byte count back through
//! [`Device::dma_complete`] — at which point the controller finishes its result
//! phase and raises IRQ6. The controller never holds a memory reference and the
//! 8237 never sees the disk.
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
//! `offset = lba * 512`. A READ to an unattached drive terminates abnormally with
//! the not-ready status; a READ outside the drive's geometry terminates
//! abnormally with a no-data status.
//!
//! ## Coverage matrix (datasheet vs modelled)
//!
//! | Command                        | Opcode | Modelled | Notes                          |
//! |--------------------------------|--------|----------|--------------------------------|
//! | SPECIFY                        | `0x03` | yes      | timings accepted, not enforced |
//! | RECALIBRATE                    | `0x07` | yes      | seek to cylinder 0, IRQ6       |
//! | SENSE INTERRUPT STATUS         | `0x08` | yes      | ST0 + present cylinder         |
//! | SEEK                           | `0x0F` | yes      | set present cylinder, IRQ6     |
//! | READ ID                        | `0x4A` | yes      | current C/H/R/N                |
//! | READ DATA                      | `0x06` | yes      | DMA transfer to memory, IRQ6   |
//!
//! Deliberately **not** modelled:
//! - **WRITE DATA / FORMAT TRACK / WRITE DELETED** and the other write-side
//!   commands: the boot path only reads. The backing image is read-only here.
//! - **Non-DMA (PIO) transfer mode**: transfers always go through the 8237. The
//!   DOR's DMA-enable bit is honoured for interrupt gating but the controller has
//!   no data-port streaming path.
//! - **Cycle-accurate timing**: SPECIFY's step-rate / head-load / head-unload
//!   times are accepted and discarded; seeks and reads complete immediately in
//!   virtual time. There is no index-pulse or rotational-latency model.
//! - **The multitrack (MT) / skip (SK) modifiers and MFM/FM selection**: the high
//!   command bits are decoded off but do not change the transfer (every access is
//!   treated as a normal MFM read within one track).

use device::{AccessDir, Device, DeviceError, DmaMemory, DmaRequest, TraceEvent, VClock};

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
/// [`FloppyGeometry`] plus the raw backing image (read-only for the boot path).
#[derive(Clone, Debug)]
pub struct Drive {
    /// Physical format of the mounted disk.
    pub geometry: FloppyGeometry,
    /// Raw image bytes; typically [`FloppyGeometry::image_bytes`] long. Reads
    /// past the end of a short image return zero.
    pub image: Vec<u8>,
}

impl Drive {
    /// Mount `image` with the given `geometry`.
    #[must_use]
    pub fn new(geometry: FloppyGeometry, image: Vec<u8>) -> Self {
        Self { geometry, image }
    }
}

/// Snapshot layout version tag.
const SNAPSHOT_VERSION: u8 = 1;

/// ST0 bit 5: seek end (set after a completed RECALIBRATE / SEEK).
const ST0_SEEK_END: u8 = 1 << 5;
/// ST0 interrupt-code bits 7-6 == `11`: abnormal termination / ready-line change,
/// reported by each of the four polling SENSE INTERRUPT STATUS calls a reset
/// raises.
const ST0_POLL: u8 = 0b1100_0000;
/// ST0 interrupt-code bits 7-6 == `01`: the command terminated abnormally.
const ST0_ABNORMAL: u8 = 0b0100_0000;
/// ST0 bit 3 (NR): the addressed drive is not ready (no disk mounted).
const ST0_NOT_READY: u8 = 1 << 3;
/// ST1 bit 2 (ND): no data — the requested sector was not found on the track.
const ST1_NO_DATA: u8 = 1 << 2;

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
}

/// The NEC µPD765 / Intel 8272A floppy disk controller (§7.8).
///
/// Decodes `0x3F0..=0x3F7`; the modelled registers are the DOR, MSR, data, and
/// DIR/CCR ports. Drives IRQ6 through [`Device::irq_pending`]; like every other
/// device the 8259 PIC owns vectoring, so [`Device::irq_vector`] is `None`.
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
    /// IRQ6 line level (asserted on command completion, cleared when serviced).
    irq: bool,
    /// Remaining post-reset SENSE INTERRUPT STATUS polls that report the
    /// abnormal / ready-change status a controller reset raises for each drive.
    reset_poll: u8,
    /// A READ transfer queued for the machine to run through the 8237.
    pending_request: Option<DmaRequest>,
    /// The READ context whose result phase is completed once the DMA finishes.
    pending_read: Option<PendingRead>,
    /// Media mounted in each of the four drive positions the DOR can select.
    /// `None` is an empty drive bay; a READ addressed to one is "not ready".
    drives: [Option<Drive>; 4],
    /// Trace events observed since the last [`Device::take_trace`].
    trace: Vec<TraceEvent>,
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
            drives: [None, None, None, None],
            trace: Vec::new(),
            now: VClock::ZERO,
        }
    }

    /// Create a controller with a single 1.44 MB disk mounted in drive 0, backed
    /// by `image` (typically [`FLOPPY_IMAGE_BYTES`] bytes). This is the legacy
    /// single-drive entry point; it is exactly `with_drives` carrying one
    /// high-density drive.
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
        self.irq
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
            _ => None,
        }
    }

    /// Handle a write to the digital output register, detecting the reset edge.
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
            self.reset_poll = 4;
            self.irq = true;
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
            _ => self.finish_no_result(),
        }
    }

    /// SPECIFY: accept the step-rate / head timings and return to idle. No
    /// interrupt, no result phase.
    fn cmd_specify(&mut self) {
        self.finish_no_result();
    }

    /// RECALIBRATE: seek the selected drive to cylinder 0 and raise IRQ6. The
    /// result is read later via SENSE INTERRUPT STATUS.
    fn cmd_recalibrate(&mut self) {
        let drive = self.command[1] & 0b11;
        self.present_cylinder[usize::from(drive)] = 0;
        self.st0 = ST0_SEEK_END | drive;
        self.raise_irq();
        self.finish_no_result();
    }

    /// SEEK: set the selected drive's present cylinder and raise IRQ6.
    fn cmd_seek(&mut self) {
        let drive_head = self.command[1];
        let drive = drive_head & 0b11;
        let head = (drive_head >> 2) & 1;
        let cylinder = self.command[2];
        self.present_cylinder[usize::from(drive)] = cylinder;
        self.st0 = ST0_SEEK_END | (head << 2) | drive;
        self.raise_irq();
        self.finish_no_result();
    }

    /// SENSE INTERRUPT STATUS: report ST0 and the present cylinder, clearing the
    /// pending interrupt. After a reset the first four calls report the drive-poll
    /// status instead.
    fn cmd_sense_interrupt(&mut self) {
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

    /// READ ID: report the C/H/R/N of the current position and raise IRQ6.
    fn cmd_read_id(&mut self) {
        let drive_head = self.command[1];
        let drive = drive_head & 0b11;
        let head = (drive_head >> 2) & 1;
        let cylinder = self.present_cylinder[usize::from(drive)];
        self.st0 = (head << 2) | drive;
        self.raise_irq();
        self.result = vec![self.st0, 0, 0, cylinder, head, 1, 2];
        self.result_index = 0;
        self.phase = Phase::Result;
    }

    /// READ DATA: pull the sectors from `R` to `EOT` on the addressed track out
    /// of the backing image and queue them as a DMA request. The result phase and
    /// IRQ6 follow once the machine reports the transfer complete.
    fn cmd_read_data(&mut self) {
        let drive_head = self.command[1];
        let drive = drive_head & 0b11;
        let head = (drive_head >> 2) & 1;
        let cylinder = self.command[2];
        let param_head = self.command[3];
        let sector = self.command[4];
        let sector_size_code = self.command[5];
        let end_of_track = self.command[6];

        // Resolve the addressed drive's mounted media; an empty bay is not ready.
        let Some(media) = self.drives[usize::from(drive)].as_ref() else {
            self.read_fault(
                drive,
                head,
                cylinder,
                sector,
                sector_size_code,
                ST0_NOT_READY,
                0,
            );
            return;
        };
        let geometry = media.geometry;

        // An address outside the drive's geometry has no matching data field.
        if !geometry.contains(cylinder, param_head, sector) {
            self.read_fault(
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

        // Reads span R..=EOT on the one track; the 8237's programmed count clamps
        // how many of those bytes actually land in memory.
        let last = end_of_track.max(sector);
        let sector_count = u32::from(last.saturating_sub(sector)) + 1;
        let mut data = Vec::with_capacity(sector_count as usize * SECTOR_SIZE);
        for i in 0..sector_count {
            let s = sector + i as u8;
            let offset = geometry.chs_to_offset(cylinder, param_head, s);
            for j in 0..SECTOR_SIZE {
                data.push(media.image.get(offset + j).copied().unwrap_or(0));
            }
        }

        self.pending_read = Some(PendingRead {
            drive,
            head,
            cylinder,
            sector,
            sector_size_code,
        });
        self.pending_request = Some(DmaRequest {
            channel: FLOPPY_DMA_CHANNEL,
            data,
        });
        self.phase = Phase::Execution;
    }

    /// Terminate a READ DATA abnormally without a DMA transfer: build the
    /// seven-byte result with the given ST0/ST1 fault bits and raise IRQ6.
    #[allow(clippy::too_many_arguments)]
    fn read_fault(
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

    /// Complete a READ transfer once the machine has moved `transferred` bytes:
    /// advance the sector past what was read, build the seven-byte result, and
    /// raise IRQ6.
    fn complete_read(&mut self, transferred: usize) {
        let Some(read) = self.pending_read.take() else {
            return;
        };
        let sectors_done = (transferred / SECTOR_SIZE) as u8;
        let next_sector = read.sector + sectors_done;
        self.st0 = (read.head << 2) | read.drive;
        self.raise_irq();
        self.result = vec![
            self.st0,
            0,
            0,
            read.cylinder,
            read.head,
            next_sector,
            read.sector_size_code,
        ];
        self.result_index = 0;
        self.phase = Phase::Result;
    }

    /// Assert the IRQ6 line if the DMA/interrupt path is enabled in the DOR.
    fn raise_irq(&mut self) {
        if self.dor & DOR_DMA_ENABLE != 0 {
            self.irq = true;
        }
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
        // No autonomous timed behaviour: seeks and reads complete immediately in
        // virtual time. The clock is stamped onto traces only.
        self.now = now;
    }

    fn irq_pending(&self) -> bool {
        self.irq
    }

    fn irq_vector(&self) -> Option<u8> {
        // The controller drives the IRQ6 line; the 8259 PIC resolves it to a
        // vector on INTA.
        None
    }

    fn take_dma_request(&mut self) -> Option<DmaRequest> {
        self.pending_request.take()
    }

    fn dma_complete(&mut self, transferred: usize) {
        self.complete_read(transferred);
    }

    fn reset(&mut self) {
        // A controller reset clears the registers but does not eject any disk.
        let drives = std::mem::take(&mut self.drives);
        *self = Self::with_drives(drives);
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
        self.dor = next()?;
        self.phase = match next()? {
            0 => Phase::Idle,
            1 => Phase::Command,
            2 => Phase::Execution,
            3 => Phase::Result,
            _ => return Err(bad("invalid phase tag")),
        };
        self.st0 = next()?;
        self.irq = next()? != 0;
        self.reset_poll = next()?;
        for pcn in &mut self.present_cylinder {
            *pcn = next()?;
        }
        self.expected = usize::from(next()?);
        let command_len = usize::from(next()?);
        self.command.clear();
        for _ in 0..command_len {
            self.command.push(next()?);
        }
        let result_len = usize::from(next()?);
        self.result.clear();
        for _ in 0..result_len {
            self.result.push(next()?);
        }
        self.result_index = usize::from(next()?);
        // A snapshot is taken at rest, never mid-transfer.
        self.pending_request = None;
        self.pending_read = None;
        Ok(())
    }

    fn dma_service_write(&mut self, _channel: u8, _mem: &mut dyn DmaMemory, _data: &[u8]) -> usize {
        // The floppy controller is a DMA *requester*, never the controller.
        0
    }

    fn take_trace(&mut self) -> Vec<TraceEvent> {
        std::mem::take(&mut self.trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dma8237::Dma8237;
    use std::collections::HashMap;
    use testkit::MockBus;

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

    /// A ready controller with a single 1.44 MB disk in drive 0 (legacy shape).
    fn ready_fdc(image: Vec<u8>) -> Upd765 {
        ready_drives([
            Some(Drive::new(FloppyGeometry::HD1440, image)),
            None,
            None,
            None,
        ])
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
        assert_eq!(fdc.irq_vector(), None);
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
        assert_eq!(request.data, image, "the queued bytes are sector 0");
        assert!(
            fdc.take_dma_request().is_none(),
            "the request is taken once"
        );

        fdc.dma_complete(SECTOR_SIZE);
        assert!(fdc.irq_pending(), "IRQ6 rises on transfer completion");
        let res = results(&mut fdc, 7);
        assert_eq!(res[0] & 0xC0, 0, "ST0 normal termination");
        assert_eq!(res[3], 0, "result cylinder");
        assert_eq!(res[5], 2, "next sector = R + 1");
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
        let n = dma.dma_service_write(request.channel, &mut mem, &request.data);
        assert_eq!(n, SECTOR_SIZE, "the whole sector transfers");
        fdc.dma_complete(n);

        for (i, &expected) in image.iter().enumerate() {
            assert_eq!(
                mem.dma_read(0x07C00 + i as u32),
                expected,
                "byte {i} in memory"
            );
        }
        assert!(fdc.irq_pending());
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
        let data = fdc
            .take_dma_request()
            .expect("a valid read queues a DMA request")
            .data;
        fdc.dma_complete(data.len());
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

    #[test]
    fn read_from_empty_drive_bay_is_not_ready() {
        // Only drive 0 is populated; a read addressed to drive 1 finds no disk.
        let mut fdc = ready_fdc(patterned(FloppyGeometry::HD1440, 0x44));
        command(
            &mut fdc,
            &[0x06, 0x01, 0x00, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF],
        );
        assert!(fdc.take_dma_request().is_none(), "no media, no transfer");
        let res = results(&mut fdc, 7);
        assert_eq!(res[0] & 0xC0, ST0_ABNORMAL, "abnormal termination");
        assert_eq!(res[0] & ST0_NOT_READY, ST0_NOT_READY, "ST0 not-ready");
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
        let out = Command::new("mformat")
            .args(["-C", "-f", "360", "-v", "TEST360", "-i"])
            .arg(&path)
            .arg("::")
            .output()
            .expect("mformat must be on PATH (provided by the devShell's mtools)");
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
