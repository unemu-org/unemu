// SPDX-License-Identifier: AGPL-3.0-or-later
//! Functional ATA-2 PIO channel with two independently selected disks.
//!
//! Sources: AT Attachment Interface with Extensions (ATA-2), ANSI X3.279,
//! §§6.1–6.2 (task file), §7.2 (addressing), command descriptions in §7,
//! §§8.2–8.5 (reset and PIO protocols), and §7.13 (diagnostics). Section numbers
//! follow X3T10/0948D revision 4c. IBM PC AT Technical Reference, "Fixed Disk
//! and Diskette Drive Adapter", Programming Interface, supplies the PC port
//! and interrupt mapping. See `backlog/docs/design/spec.md` §7.1 and
//! `backlog/docs/design/firmware-and-machine-classes.md` §§2.2, 9.
//!
//! Hardware coverage matrix (`backlog/docs/testing.md` §3):
//!
//! | Defined | Modelled | Skipped / approximation |
//! | --- | --- | --- |
//! | ATA-2 §§6.2.2, 6.2.12 task file | Byte registers, word data, alternate status, nIEN, SRST; writes accepted when BSY=0; a new command replaces a pending transfer | No 8-bit data or 32-bit host adapter extension |
//! | §§6.1, 7.2 addressing | CHS translation, LBA28, 512-byte sectors; a lone disk must be device 0 | Default and translated geometry bounded to XUB limits: 16383 cylinders, 16 heads, 63 sectors per track |
//! | §7.18 parameters | Default parameters select default CHS; rejected translation blocks media access with IDNF until valid parameters or reset | No physical geometry or mechanical faults |
//! | §7 command descriptions | IDENTIFY, sector/retry and multiple reads/writes, READ VERIFY (40h/41h, §7.26), multiple mode, parameters, features, seek | Other commands ABRT; no DMA, packet, power management or cache |
//! | §§8.3–8.5 protocols | DRQ blocks, interrupt acknowledgement, errors | Zero-delay completion; BSY only while SRST is held, no IORDY/bus timing |
//! | §7 SET FEATURES | 03h: default PIO (00h/01h), PIO modes 0–4 (08h–0Ch), accepted without timing effects | Other features and DMA modes ABRT |
//! | §7.13 diagnostics | Command-block signature and success; translation and multiple mode retained | No physical diagnostic failures |
//! | §7.27 RECALIBRATE | Cylinder/head zero; sector 1 in CHS, 0 in LBA | No mechanical movement |
//! | §8.7 device 0 only | Absent-slave status/alternate status 00h; other accesses echo device 0 | No electrical bus behaviour |
//! | §§7.24, 7.37, 8.3–8.4 multiple errors | Whole-block address checks; IDNF completes without DRQ | No genuine media faults or ECC; no new DRQ-with-error transfers |
//! | §§8.2, 7.2 reset | Signature and default translation; software reset disables multiple mode | No electrical DASP/PDIAG handshake |
//! | IBM adapter mapping | Configurable task/control ports and IRQ (1F0h/3F6h/14 defaults) | Floppy port sharing and machine wiring |
//!
//! Media receives one ordered batch per completed write command. A sector is
//! committed after its 256th data word; partial sectors are discarded. Normal
//! completion, address failure, replacement by a new command, diagnostics, SRST,
//! hardware reset and successful snapshot restore end the interrupted command
//! and submit its committed prefix. Reads see that prefix while a command is
//! active. A future persistent IDE backend must call [`Ata::flush_pending`]
//! at session end to submit completed sectors of an abandoned command; guest
//! drive selection alone does not finish it. Media has no host error channel. ATA-2 §6.2.8 (Error register, ABRT)
//! defines aborted commands; this model aborts writes on protected media.
//!
//! Disk writes are volatile by default. Like the floppy controller, snapshots exclude
//! backing media: restore requires the same mounted geometry/capacity and does
//! not roll back completed disk writes. Partial PIO sector buffers, translated
//! geometry, IRQ state and trace clock are saved. Media identity/content must
//! be managed by the caller. Reset retains media, restores default translation
//! and clears trace history while preserving its policy. Restore preserves the
//! run-scoped trace buffer. Snapshots preserve the PIO addressing-mode latch
//! separately from writable task registers.
//! Accesses outside DRQ or in the wrong direction are
//! ignored (data reads return FFFFh). The data register accepts only complete
//! 16-bit cycles; 8-bit and 32-bit accesses are rejected. Host-bus IOCS16#
//! cycle splitting belongs to the bus, not this device.

use device::media::{InMemoryMedia, SectorMedia, SectorWrite};

use device::{
    AccessDir, Device, DeviceError, NextEvent, TraceBuffer, TraceEvent, TracePolicy, VClock,
};

const NAME: &str = "ata";
const READY: u8 = 0x50;
const BSY: u8 = 0x80;
const DRQ: u8 = 8;
const ERR: u8 = 1;
const ABRT: u8 = 4;
const IDNF: u8 = 0x10;
const MAX_MULTIPLE: u8 = 16;
const SECTOR: usize = 512;
const SNAPSHOT_VERSION: u8 = 1;
const SNAPSHOT_TASK_LEN: usize = 37 + SECTOR;

/// Logical CHS translation (ATA-2 §7.2); sector numbers on the wire start at 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtaGeometry {
    /// Number of logical cylinders, 1–16383 (XTIDE Universal BIOS geometry limit).
    pub cylinders: u16,
    /// Number of logical heads, 1–16.
    pub heads: u8,
    /// Number of logical sectors per track, 1–63 (XTIDE Universal BIOS limit).
    pub sectors_per_track: u8,
}

impl AtaGeometry {
    /// Validate the CHS bounds accepted by XTIDE Universal BIOS AT IDENTIFY.
    pub fn new(cylinders: u16, heads: u8, sectors_per_track: u8) -> Result<Self, AtaConfigError> {
        let geometry = Self {
            cylinders,
            heads,
            sectors_per_track,
        };
        if !geometry.valid() {
            return Err(AtaConfigError::Geometry);
        }
        Ok(geometry)
    }

    fn valid(self) -> bool {
        (1..=16383).contains(&self.cylinders)
            && (1..=16).contains(&self.heads)
            && (1..=63).contains(&self.sectors_per_track)
    }

    fn capacity(self) -> u32 {
        u32::from(self.cylinders) * u32::from(self.heads) * u32::from(self.sectors_per_track)
    }
}

/// Invalid host-side disk geometry, image or channel configuration.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AtaConfigError {
    /// ATA-2 §6.1 requires a lone disk to be device 0.
    #[error("ATA device 1 requires device 0")]
    MissingMaster,
    /// CHS must be nonzero, encodable and fit within the backing media.
    #[error("invalid ATA geometry: cylinders must be 1..=16383, heads 1..=16, sectors per track 1..=63, and CHS capacity must fit the image")]
    Geometry,
    /// Images contain 1–2^28 complete 512-byte sectors.
    #[error("invalid ATA image length")]
    Image,
    /// Advertised capacity exceeds the mounted media's addressable sectors.
    #[error("ATA capacity {capacity} sectors exceeds media capacity {sector_count} sectors")]
    MediaCapacity {
        /// Capacity advertised by the ATA disk.
        capacity: u32,
        /// Addressable sectors supplied by the medium.
        sector_count: u64,
    },
    /// The eight task ports must not wrap/overlap control; IRQ must fit ISA.
    #[error("invalid ATA ports or IRQ")]
    Resources,
}

/// ATA sector media with an explicit LBA28 capacity, independently of CHS.
pub struct AtaDrive {
    geometry: AtaGeometry,
    capacity: u32,
    media: Box<dyn SectorMedia>,
    committed: Vec<SectorWrite>,
}

impl AtaDrive {
    /// Validate and mount complete sectors; CHS may expose less than LBA28.
    pub fn new(geometry: AtaGeometry, image: Vec<u8>) -> Result<Self, AtaConfigError> {
        if image.is_empty() || !image.len().is_multiple_of(SECTOR) || image.len() / SECTOR > 1 << 28
        {
            return Err(AtaConfigError::Image);
        }
        AtaGeometry::new(
            geometry.cylinders,
            geometry.heads,
            geometry.sectors_per_track,
        )?;
        if geometry.capacity() as usize > image.len() / SECTOR {
            return Err(AtaConfigError::Geometry);
        }
        let capacity = (image.len() / SECTOR) as u32;
        Self::with_media(
            geometry,
            capacity,
            Box::new(InMemoryMedia::new(image, u64::from(capacity), false)),
        )
    }

    /// Mount host-independent media with an explicit LBA28 capacity.
    pub fn with_media(
        geometry: AtaGeometry,
        capacity: u32,
        media: Box<dyn SectorMedia>,
    ) -> Result<Self, AtaConfigError> {
        AtaGeometry::new(
            geometry.cylinders,
            geometry.heads,
            geometry.sectors_per_track,
        )?;
        if capacity == 0 || capacity > 1 << 28 {
            return Err(AtaConfigError::Image);
        }
        if u64::from(capacity) > media.sector_count() {
            return Err(AtaConfigError::MediaCapacity {
                capacity,
                sector_count: media.sector_count(),
            });
        }
        if geometry.capacity() > capacity {
            return Err(AtaConfigError::Geometry);
        }
        Ok(Self {
            geometry,
            capacity,
            media,
            committed: Vec::new(),
        })
    }

    /// Current readable bytes, including sectors completed by an active command.
    pub fn image(&self) -> Vec<u8> {
        (0..self.capacity)
            .flat_map(|lba| self.read_sector(lba))
            .collect()
    }

    /// Number of addressable LBA sectors (ATA-2 IDENTIFY words 60–61).
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    fn read_sector(&self, lba: u32) -> [u8; SECTOR] {
        self.committed
            .iter()
            .rev()
            .find(|sector| sector.lba == u64::from(lba))
            .map_or_else(
                || self.media.read_sector(u64::from(lba)),
                |sector| sector.bytes,
            )
    }

    fn finish_write(&mut self) {
        if !self.committed.is_empty() {
            self.media.write_batch(&self.committed);
            self.committed.clear();
        }
    }
}

#[derive(Clone)]
struct Task {
    // Feature writes and error reads occupy the same port but separate latches.
    features: u8,
    error: u8,
    count: u8,
    sector: u8,
    low: u8,
    high: u8,
    head: u8,
    status: u8,
    irq: bool,
    geometry: AtaGeometry,
    multiple: u8,
    translation_valid: bool,
    multiple_transfer: bool,
    transfer_head: u8,
    // 0 idle, 1 read sectors, 2 write sectors, 3 identify.
    phase: u8,
    lba: u32,
    remaining: u16,
    word: u16,
    block_left: u8,
    block_size: u8,
    buffer: [u8; SECTOR],
}

impl Task {
    fn new(drive: Option<&AtaDrive>, index: usize) -> Self {
        Self {
            features: 0,
            error: 1,
            count: 1,
            sector: 1,
            low: 0,
            high: 0,
            head: (index as u8) << 4,
            status: if drive.is_some() { READY } else { 0 },
            irq: false,
            geometry: drive.map_or(
                AtaGeometry {
                    cylinders: 1,
                    heads: 1,
                    sectors_per_track: 1,
                },
                |d| d.geometry,
            ),
            multiple: 0,
            translation_valid: true,
            multiple_transfer: false,
            transfer_head: 0,
            phase: 0,
            lba: 0,
            remaining: 0,
            word: 0,
            block_left: 0,
            block_size: 0,
            buffer: [0; SECTOR],
        }
    }

    fn stop(&mut self) {
        self.phase = 0;
        self.multiple_transfer = false;
        self.transfer_head = 0;
        self.remaining = 0;
        self.word = 0;
        self.block_left = 0;
        self.block_size = 0;
        self.lba = 0;
        self.buffer.fill(0);
    }

    fn fail(&mut self, error: u8) {
        self.stop();
        self.status = READY | ERR;
        self.error = error;
        self.irq = true;
    }

    fn address(&self) -> Option<u32> {
        if !self.translation_valid {
            return None;
        }
        if self.head & 0x40 != 0 {
            Some(u32::from_le_bytes([
                self.sector,
                self.low,
                self.high,
                self.head & 15,
            ]))
        } else {
            let cylinder = u16::from_le_bytes([self.low, self.high]);
            let head = self.head & 15;
            (cylinder < self.geometry.cylinders
                && head < self.geometry.heads
                && self.sector != 0
                && self.sector <= self.geometry.sectors_per_track)
                .then(|| {
                    (u32::from(cylinder) * u32::from(self.geometry.heads) + u32::from(head))
                        * u32::from(self.geometry.sectors_per_track)
                        + u32::from(self.sector - 1)
                })
        }
    }

    fn addressing_head(&self) -> u8 {
        if matches!(self.phase, 1 | 2) {
            self.transfer_head
        } else {
            self.head
        }
    }

    fn media_limit(&self, drive: &AtaDrive) -> u32 {
        if self.addressing_head() & 0x40 != 0 {
            drive.capacity()
        } else {
            drive.capacity().min(self.geometry.capacity())
        }
    }

    /// ATA-2 §§8.3 g, 8.4 g allow an address error to finish without data.
    fn prepare_block(&mut self, drive: &AtaDrive) -> bool {
        self.block_left = u16::from(self.block_size).min(self.remaining) as u8;
        let limit = self.media_limit(drive);
        if self.multiple_transfer && self.lba + u32::from(self.block_left) > limit {
            let failing = self.lba.max(limit);
            self.count = (self.lba + u32::from(self.remaining) - failing) as u8;
            self.lba = failing;
            self.post_address();
            self.fail(IDNF);
            return false;
        }
        true
    }

    fn load_sector(&mut self, drive: &AtaDrive) {
        self.buffer.fill(0);
        if self.phase == 1 && self.lba < self.media_limit(drive) {
            self.buffer = drive.read_sector(self.lba);
        }
    }

    /// ATA-2 §6.2.7: address posting preserves the host's DEV and reserved bits.
    fn post_address(&mut self) {
        let head = (self.head & 0xB0) | (self.addressing_head() & 0x40);
        if head & 0x40 != 0 {
            let bytes = self.lba.to_le_bytes();
            self.sector = bytes[0];
            self.low = bytes[1];
            self.high = bytes[2];
            self.head = (head & 0xF0) | (bytes[3] & 15);
        } else {
            let track = self.lba / u32::from(self.geometry.sectors_per_track);
            self.sector = (self.lba % u32::from(self.geometry.sectors_per_track)) as u8 + 1;
            self.head = (head & 0xF0) | (track % u32::from(self.geometry.heads)) as u8;
            let bytes = ((track / u32::from(self.geometry.heads)) as u16).to_le_bytes();
            self.low = bytes[0];
            self.high = bytes[1];
        }
    }
}

/// ATA-2 task file and PIO state behind the [`Device`] seam.
///
/// Register writes reach both devices (ATA-2 §6.1); commands reach the selected
/// device, except diagnostics. ATA-2 §§6.2.2, 6.2.12: only BSY blocks task writes;
/// a Command write abandons the current PIO buffer and starts the new request.
/// Selection and task writes do not change the active transfer's latched address.
pub struct Ata {
    base: u16,
    control_port: u16,
    irq: u8,
    drives: [Option<AtaDrive>; 2],
    tasks: [Task; 2],
    selected: usize,
    control: u8,
    now: VClock,
    trace: TraceBuffer,
}

impl Default for Ata {
    fn default() -> Self {
        Self::new()
    }
}

impl Ata {
    /// Empty primary channel with PC AT defaults.
    pub fn new() -> Self {
        Self::configured([None, None])
    }

    /// Primary channel with validated master/slave media (ATA-2 §6.1).
    pub fn with_drives(drives: [Option<AtaDrive>; 2]) -> Result<Self, AtaConfigError> {
        if drives[0].is_none() && drives[1].is_some() {
            return Err(AtaConfigError::MissingMaster);
        }
        Ok(Self::configured(drives))
    }

    fn configured(drives: [Option<AtaDrive>; 2]) -> Self {
        let tasks = std::array::from_fn(|i| Task::new(drives[i].as_ref(), i));
        Self {
            base: 0x1F0,
            control_port: 0x3F6,
            irq: 14,
            drives,
            tasks,
            selected: 0,
            control: 0,
            now: VClock::ZERO,
            trace: TraceBuffer::default(),
        }
    }

    /// Select host resources without changing disk state.
    pub fn with_resources(
        mut self,
        base: u16,
        control_port: u16,
        irq: u8,
    ) -> Result<Self, AtaConfigError> {
        let Some(end) = base.checked_add(7) else {
            return Err(AtaConfigError::Resources);
        };
        if (base..=end).contains(&control_port) || irq > 15 {
            return Err(AtaConfigError::Resources);
        }
        self.base = base;
        self.control_port = control_port;
        self.irq = irq;
        Ok(self)
    }

    /// Configured ISA interrupt number; routing belongs to machine assembly.
    pub fn irq_number(&self) -> u8 {
        self.irq
    }

    /// Mounted disk, including its current volatile writes.
    pub fn drive(&self, index: usize) -> Option<&AtaDrive> {
        self.drives.get(index)?.as_ref()
    }

    /// Submit complete sectors at session end only, leaving guest state intact.
    /// A future persistent IDE backend must call this when no further guest
    /// activity will end the pending command. Calling it mid-command splits that
    /// command into multiple batches. Partial sectors remain controller state
    /// and are never included in the batch.
    pub fn flush_pending(&mut self) {
        self.finish_writes();
    }

    fn signature(&mut self) {
        self.finish_writes();
        self.tasks = std::array::from_fn(|i| Task::new(self.drives[i].as_ref(), i));
        self.selected = 0;
    }

    /// ATA-2 §7.13 resets registers, not the programmed translation or multiple mode.
    fn diagnostic_signature(&mut self) {
        self.finish_writes();
        for (task, drive) in self.tasks.iter_mut().zip(&self.drives) {
            task.stop();
            task.error = 1;
            task.count = 1;
            task.sector = 1;
            task.low = 0;
            task.high = 0;
            task.head = 0;
            task.status = if drive.is_some() { READY } else { 0 };
            task.irq = false;
        }
        self.selected = 0;
    }

    fn register_device(&self) -> usize {
        if self.drives[self.selected].is_none() {
            0
        } else {
            self.selected
        }
    }

    fn finish_writes(&mut self) {
        for drive in self.drives.iter_mut().flatten() {
            drive.finish_write();
        }
    }

    fn command(&mut self, command: u8) {
        if self.control & 4 != 0 {
            return;
        }
        if command == 0x90 {
            self.diagnostic_signature();
            self.tasks[0].irq = self.drives[0].is_some();
            return;
        }
        let index = self.selected;
        if let Some(drive) = &mut self.drives[index] {
            drive.finish_write();
        }
        let Some(drive) = &self.drives[index] else {
            return;
        };
        let task = &mut self.tasks[index];
        task.irq = false;
        if task.status & BSY != 0 {
            return;
        }
        task.stop();
        task.error = 0;
        task.status = READY;
        match command {
            0xEC => {
                task.buffer = Self::identify(drive, task, index);
                task.phase = 3;
                task.remaining = 1;
                task.block_size = 1;
                task.block_left = 1;
                task.status |= DRQ;
                task.irq = true;
            }
            0x20 | 0x21 | 0x30 | 0x31 | 0xC4 | 0xC5 => {
                // ATA-2 defines no fixed-disk write-protect condition. This
                // model reports protected writes with its abort error.
                if matches!(command, 0x30 | 0x31 | 0xC5) && drive.media.write_protected() {
                    task.fail(ABRT);
                    return;
                }
                let multiple = matches!(command, 0xC4 | 0xC5);
                if multiple && task.multiple == 0 {
                    task.fail(ABRT);
                    return;
                }
                let Some(lba) = task.address() else {
                    task.fail(IDNF);
                    return;
                };
                if lba >= drive.capacity() {
                    task.fail(IDNF);
                    return;
                }
                task.lba = lba;
                task.remaining = if task.count == 0 {
                    256
                } else {
                    u16::from(task.count)
                };
                task.block_size = if multiple { task.multiple } else { 1 };
                task.multiple_transfer = multiple;
                task.phase = if matches!(command, 0x30 | 0x31 | 0xC5) {
                    2
                } else {
                    1
                };
                task.transfer_head = task.head;
                if !task.prepare_block(drive) {
                    return;
                }
                task.status |= DRQ;
                task.load_sector(drive);
                if task.phase == 1 {
                    task.irq = true;
                }
            }
            0x40 | 0x41 => {
                let Some(address) = task.address() else {
                    task.fail(IDNF);
                    return;
                };
                task.lba = address;
                let requested = if task.count == 0 {
                    256
                } else {
                    u16::from(task.count)
                };
                let available = task
                    .media_limit(drive)
                    .saturating_sub(address)
                    .min(u32::from(requested));
                task.count = (u32::from(requested) - available) as u8;
                if available < u32::from(requested) {
                    task.lba += available;
                    task.post_address();
                    task.fail(IDNF);
                } else {
                    task.lba += available - 1;
                    task.post_address();
                    task.stop();
                    task.irq = true;
                }
            }
            0xC6 => {
                if task.count != 0 && (!task.count.is_power_of_two() || task.count > MAX_MULTIPLE) {
                    task.multiple = 0;
                    task.fail(ABRT);
                } else {
                    task.multiple = task.count;
                    task.irq = true;
                }
            }
            0x91 => {
                let heads = (task.head & 15) + 1;
                let sectors = task.count;
                let cylinders = if sectors == 0 {
                    0
                } else {
                    drive.capacity() / (u32::from(heads) * u32::from(sectors))
                };
                if cylinders == 0 || sectors > 63 {
                    task.translation_valid = false;
                    task.fail(ABRT);
                } else {
                    task.geometry = if heads == drive.geometry.heads
                        && sectors == drive.geometry.sectors_per_track
                    {
                        drive.geometry
                    } else {
                        AtaGeometry {
                            cylinders: cylinders.min(16383) as u16,
                            heads,
                            sectors_per_track: sectors,
                        }
                    };
                    task.translation_valid = true;
                    task.irq = true;
                }
            }
            0xEF => {
                if task.features == 3 && matches!(task.count, 0 | 1 | 8..=12) {
                    task.irq = true;
                } else {
                    task.fail(ABRT);
                }
            }
            0x10..=0x1F => {
                task.low = 0;
                task.high = 0;
                task.head &= 0xF0;
                task.sector = u8::from(task.head & 0x40 == 0);
                task.irq = true;
            }
            0x70..=0x7F => {
                if task.address().is_some_and(|lba| lba < drive.capacity()) {
                    task.irq = true;
                } else {
                    task.fail(IDNF);
                }
            }
            _ => task.fail(ABRT),
        }
    }

    fn identify(drive: &AtaDrive, task: &Task, index: usize) -> [u8; SECTOR] {
        let mut words = [0u16; 256];
        words[0] = 0x40;
        words[1] = drive.geometry.cylinders;
        words[3] = u16::from(drive.geometry.heads);
        words[6] = u16::from(drive.geometry.sectors_per_track);
        for (start, len, value) in [
            (
                10,
                20,
                if index == 0 {
                    "UNEMU000000000000001"
                } else {
                    "UNEMU000000000000002"
                },
            ),
            (23, 8, "1.0"),
            (27, 40, "UNEMU ATA-2 PIO DISK"),
        ] {
            for i in 0..len / 2 {
                words[start + i] = u16::from_be_bytes([
                    value.as_bytes().get(i * 2).copied().unwrap_or(b' '),
                    value.as_bytes().get(i * 2 + 1).copied().unwrap_or(b' '),
                ]);
            }
        }
        words[47] = 0x8000 | u16::from(MAX_MULTIPLE);
        words[49] = 0x0E00; // LBA and IORDY; no DMA capability.
        words[51] = 0x0200;
        words[53] = 2 | u16::from(task.translation_valid);
        words[54] = task.geometry.cylinders;
        words[55] = u16::from(task.geometry.heads);
        words[56] = u16::from(task.geometry.sectors_per_track);
        let chs = task.geometry.capacity();
        words[57] = chs as u16;
        words[58] = (chs >> 16) as u16;
        words[59] = 0x100 | u16::from(task.multiple);
        words[60] = drive.capacity() as u16;
        words[61] = (drive.capacity() >> 16) as u16;
        words[64] = 3;
        words[67] = 240;
        words[68] = 120;
        std::array::from_fn(|i| words[i / 2].to_le_bytes()[i % 2])
    }

    fn data(&mut self, index: usize, write: Option<u16>) -> u16 {
        let task = &mut self.tasks[index];
        if self.control & 4 != 0
            || self.drives[index].is_none()
            || task.phase == 0
            || write.is_some() != (task.phase == 2)
        {
            return 0xFFFF;
        }
        let offset = usize::from(task.word) * 2;
        let result = u16::from_le_bytes([task.buffer[offset], task.buffer[offset + 1]]);
        if let Some(word) = write {
            task.buffer[offset..offset + 2].copy_from_slice(&word.to_le_bytes());
        }
        task.word += 1;
        if task.word != 256 {
            return result;
        }
        if task.phase == 3 {
            task.stop();
            task.status = READY;
            return result;
        }
        let Some(drive) = &mut self.drives[index] else {
            return result;
        };
        if task.phase == 2 && task.lba < task.media_limit(drive) {
            drive.committed.push(SectorWrite {
                lba: u64::from(task.lba),
                bytes: task.buffer,
            });
        }
        task.remaining -= 1;
        task.block_left -= 1;
        task.word = 0;
        task.lba += 1;
        task.count = task.remaining as u8;
        if task.remaining == 0 {
            if task.phase == 2 {
                task.irq = true;
            }
            drive.finish_write();
            task.stop();
            task.status = READY;
            return result;
        }
        task.post_address();
        if !task.multiple_transfer && task.lba >= task.media_limit(drive) {
            drive.finish_write();
            task.fail(IDNF);
            return result;
        }
        if task.block_left == 0 {
            if !task.prepare_block(drive) {
                drive.finish_write();
                return result;
            }
            task.irq = true;
        }
        task.load_sector(drive);
        result
    }

    fn port_offset(&self, port: u16, size: u8) -> Result<u16, DeviceError> {
        let offset = if port == self.control_port {
            Some(8)
        } else {
            port.checked_sub(self.base).filter(|&offset| offset < 8)
        };
        match offset {
            Some(offset) if (offset == 0 && size == 2) || (offset != 0 && size == 1) => Ok(offset),
            _ => Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            }),
        }
    }

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

impl Device for Ata {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let offset = self.port_offset(port, size)?;
        let index = self.register_device();
        let value = if offset == 0 {
            u32::from(self.data(index, None))
        } else if self.drives[index].is_none()
            || (self.drives[self.selected].is_none() && matches!(offset, 7 | 8))
        {
            0
        } else {
            let task = &mut self.tasks[index];
            u32::from(match offset {
                1 => task.error,
                2 => task.count,
                3 => task.sector,
                4 => task.low,
                5 => task.high,
                6 => task.head,
                7 => {
                    task.irq = false;
                    task.status
                }
                _ => task.status,
            })
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let offset = self.port_offset(port, size)?;
        let byte = value as u8;
        match offset {
            0 => {
                self.data(self.register_device(), Some(value as u16));
            }
            8 => {
                let previous = self.control;
                self.control = byte & 6;
                if self.control & 4 != 0 {
                    self.finish_writes();
                    for (task, drive) in self.tasks.iter_mut().zip(&self.drives) {
                        task.stop();
                        task.irq = false;
                        task.status = if drive.is_some() { BSY } else { 0 };
                    }
                } else if previous & 4 != 0 {
                    self.signature();
                }
            }
            6 if self.control & 4 == 0 => {
                self.selected = usize::from((byte >> 4) & 1);
                for (task, drive) in self.tasks.iter_mut().zip(&self.drives) {
                    if drive.is_some() && task.status & BSY == 0 {
                        task.head = byte;
                    }
                }
            }
            7 => self.command(byte),
            _ => {
                for (task, drive) in self.tasks.iter_mut().zip(&self.drives) {
                    if drive.is_none() || task.status & BSY != 0 || self.control & 4 != 0 {
                        continue;
                    }
                    match offset {
                        1 => task.features = byte,
                        2 => task.count = byte,
                        3 => task.sector = byte,
                        4 => task.low = byte,
                        5 => task.high = byte,
                        _ => {}
                    }
                }
            }
        }
        self.trace_port(port, size, AccessDir::Write, value);
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, _: u8) -> Result<u64, DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn mmio_write(&mut self, addr: u64, _: u8, _: u64) -> Result<(), DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now >= self.now {
            self.now = now;
        }
    }
    fn next_event(&self, _: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        self.control & 6 == 0
            && self.drives[self.selected].is_some()
            && self.tasks[self.selected].irq
    }
    fn reset(&mut self) {
        self.signature();
        self.control = 0;
        self.now = VClock::ZERO;
        self.trace.clear();
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = vec![SNAPSHOT_VERSION];
        blob.extend_from_slice(&self.base.to_le_bytes());
        blob.extend_from_slice(&self.control_port.to_le_bytes());
        blob.extend_from_slice(&[self.irq, self.selected as u8, self.control]);
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        for (task, drive) in self.tasks.iter().zip(&self.drives) {
            blob.push(u8::from(drive.is_some()));
            let geometry = drive.as_ref().map_or(
                AtaGeometry {
                    cylinders: 0,
                    heads: 0,
                    sectors_per_track: 0,
                },
                |d| d.geometry,
            );
            blob.extend_from_slice(&geometry.cylinders.to_le_bytes());
            blob.extend_from_slice(&[geometry.heads, geometry.sectors_per_track]);
            blob.extend_from_slice(&drive.as_ref().map_or(0, AtaDrive::capacity).to_le_bytes());
            blob.extend_from_slice(&[
                task.features,
                task.error,
                task.count,
                task.sector,
                task.low,
                task.high,
                task.head,
                task.status,
                u8::from(task.irq),
            ]);
            blob.extend_from_slice(&task.geometry.cylinders.to_le_bytes());
            blob.extend_from_slice(&[
                task.geometry.heads,
                task.geometry.sectors_per_track,
                task.multiple,
                task.phase,
            ]);
            blob.extend_from_slice(&task.lba.to_le_bytes());
            blob.extend_from_slice(&task.remaining.to_le_bytes());
            blob.extend_from_slice(&task.word.to_le_bytes());
            blob.extend_from_slice(&[task.block_left, task.block_size]);
            blob.extend_from_slice(&task.buffer);
            blob.extend_from_slice(&[
                u8::from(task.translation_valid),
                u8::from(task.multiple_transfer),
            ]);
            blob.push(task.transfer_head);
        }
        blob
    }
    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        let bad = |reason| DeviceError::InvalidSnapshot {
            device: NAME,
            reason,
        };
        let Some(&version) = snapshot.first() else {
            return Err(bad("snapshot truncated"));
        };
        if version != SNAPSHOT_VERSION {
            return Err(bad("unsupported snapshot version"));
        }
        // Fixed-size parsing bounds every subsequent read, before any state changes.
        if snapshot.len() != 16 + 2 * SNAPSHOT_TASK_LEN {
            return Err(bad("snapshot length mismatch"));
        }
        let mut bytes = snapshot[1..].iter().copied();
        let mut byte = || bytes.next().unwrap_or(0);
        let base = u16::from_le_bytes(std::array::from_fn(|_| byte()));
        let control_port = u16::from_le_bytes(std::array::from_fn(|_| byte()));
        let irq = byte();
        let selected = byte();
        let control = byte();
        let now = VClock(u64::from_le_bytes(std::array::from_fn(|_| byte())));
        if base != self.base
            || control_port != self.control_port
            || irq != self.irq
            || selected > 1
            || control & !6 != 0
        {
            return Err(bad("invalid channel configuration"));
        }
        let mut tasks = self.tasks.clone();
        for (task, drive) in tasks.iter_mut().zip(&self.drives) {
            let present = byte();
            let cylinders = u16::from_le_bytes([byte(), byte()]);
            let heads = byte();
            let sectors_per_track = byte();
            let capacity = u32::from_le_bytes(std::array::from_fn(|_| byte()));
            let geometry = AtaGeometry {
                cylinders,
                heads,
                sectors_per_track,
            };
            let expected = drive.as_ref().map_or(
                AtaGeometry {
                    cylinders: 0,
                    heads: 0,
                    sectors_per_track: 0,
                },
                |d| d.geometry,
            );
            if present != u8::from(drive.is_some())
                || capacity != drive.as_ref().map_or(0, AtaDrive::capacity)
                || geometry != expected
            {
                return Err(bad("mounted media configuration mismatch"));
            }
            task.features = byte();
            task.error = byte();
            task.count = byte();
            task.sector = byte();
            task.low = byte();
            task.high = byte();
            task.head = byte();
            task.status = byte();
            let pending = byte();
            task.irq = pending != 0;
            task.geometry.cylinders = u16::from_le_bytes([byte(), byte()]);
            task.geometry.heads = byte();
            task.geometry.sectors_per_track = byte();
            task.multiple = byte();
            task.phase = byte();
            task.lba = u32::from_le_bytes(std::array::from_fn(|_| byte()));
            task.remaining = u16::from_le_bytes([byte(), byte()]);
            task.word = u16::from_le_bytes([byte(), byte()]);
            task.block_left = byte();
            task.block_size = byte();
            task.buffer = std::array::from_fn(|_| byte());
            let translation_valid = byte();
            let multiple_transfer = byte();
            task.translation_valid = translation_valid != 0;
            task.multiple_transfer = multiple_transfer != 0;
            task.transfer_head = byte();
            if pending > 1
                || translation_valid > 1
                || multiple_transfer > 1
                || !task.geometry.valid()
                || (drive.is_some() && task.geometry.capacity() > capacity)
                || (task.multiple != 0
                    && (!task.multiple.is_power_of_two() || task.multiple > MAX_MULTIPLE))
            {
                return Err(bad("invalid task state"));
            }
            let idle = task.phase == 0
                && task.remaining == 0
                && task.word == 0
                && task.block_left == 0
                && task.block_size == 0
                && task.lba == 0
                && task.buffer == [0; SECTOR]
                && !task.multiple_transfer
                && task.transfer_head == 0;
            let active = (1..=3).contains(&task.phase)
                && (task.phase != 2
                    || drive
                        .as_ref()
                        .is_some_and(|drive| !drive.media.write_protected()))
                && drive.is_some()
                && control & 4 == 0
                && task.status == READY | DRQ
                && task.error == 0
                && task.word < 256
                && (1..=256).contains(&task.remaining)
                && task.block_size.is_power_of_two()
                && (task.multiple_transfer || task.block_size == 1)
                && task.block_size <= MAX_MULTIPLE
                && (task.block_size == 1 || task.block_size == task.multiple)
                && task.block_left != 0
                && task.block_left <= task.block_size
                && u16::from(task.block_left) <= task.remaining
                && if task.phase == 3 {
                    task.remaining == 1
                        && task.block_size == 1
                        && task.lba == 0
                        && !task.multiple_transfer
                        && task.transfer_head == 0
                } else {
                    task.translation_valid
                        && drive
                            .as_ref()
                            .is_some_and(|d| task.lba < task.media_limit(d))
                };
            let idle_status = if drive.is_none() {
                task.status == 0 && !task.irq
            } else if control & 4 != 0 {
                task.status == BSY && !task.irq
            } else {
                task.status == READY
                    || (task.status == READY | ERR && matches!(task.error, ABRT | IDNF))
            };
            if !(idle && idle_status || active) {
                return Err(bad("inconsistent PIO phase"));
            }
        }
        self.finish_writes();
        self.tasks = tasks;
        self.selected = usize::from(selected);
        self.control = control;
        self.now = now;
        Ok(())
    }
    fn set_trace_policy(&mut self, policy: TracePolicy) {
        self.trace.set_policy(policy);
    }
    fn trace_dropped(&self) -> u64 {
        self.trace.dropped()
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.trace.take()
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    const GEOMETRY: AtaGeometry = AtaGeometry {
        cylinders: 8,
        heads: 2,
        sectors_per_track: 4,
    };

    fn disk(fill: u8) -> AtaDrive {
        AtaDrive::new(GEOMETRY, vec![fill; 64 * SECTOR]).unwrap()
    }
    fn controller() -> Ata {
        Ata::with_drives([Some(disk(0xA5)), None]).unwrap()
    }
    fn bus() -> MockBus {
        MockBus::new(Box::new(controller()))
    }
    fn put(bus: &mut MockBus, offset: u16, value: u8) {
        bus.port_write(0x1F0 + offset, 1, u32::from(value)).unwrap();
    }
    fn get(bus: &mut MockBus, offset: u16) -> u8 {
        bus.port_read(0x1F0 + offset, 1).unwrap() as u8
    }
    fn command(bus: &mut MockBus, opcode: u8) {
        put(bus, 7, opcode);
    }
    fn status(bus: &mut MockBus) -> u8 {
        get(bus, 7)
    }
    fn alt(bus: &mut MockBus) -> u8 {
        bus.port_read(0x3F6, 1).unwrap() as u8
    }
    fn control(bus: &mut MockBus, value: u8) {
        bus.port_write(0x3F6, 1, u32::from(value)).unwrap();
    }
    fn lba(bus: &mut MockBus, address: u32, count: u8, slave: bool) {
        put(
            bus,
            6,
            0xE0 | (u8::from(slave) << 4) | ((address >> 24) as u8 & 15),
        );
        put(bus, 2, count);
        put(bus, 3, address as u8);
        put(bus, 4, (address >> 8) as u8);
        put(bus, 5, (address >> 16) as u8);
    }
    fn chs(bus: &mut MockBus, cylinder: u16, head: u8, sector: u8, count: u8) {
        put(bus, 6, 0xA0 | head);
        put(bus, 2, count);
        put(bus, 3, sector);
        put(bus, 4, cylinder as u8);
        put(bus, 5, (cylinder >> 8) as u8);
    }
    fn read_sector(bus: &mut MockBus) -> Vec<u8> {
        let mut bytes = Vec::new();
        for _ in 0..256 {
            bytes.extend_from_slice(&(bus.port_read(0x1F0, 2).unwrap() as u16).to_le_bytes());
        }
        bytes
    }
    fn write_sector(bus: &mut MockBus, bytes: &[u8]) {
        assert_eq!(bytes.len(), SECTOR);
        for word in bytes.as_chunks::<2>().0 {
            bus.port_write(0x1F0, 2, u32::from(u16::from_le_bytes([word[0], word[1]])))
                .unwrap();
        }
    }
    fn identify(bus: &mut MockBus) -> Vec<u16> {
        command(bus, 0xEC);
        assert_eq!(status(bus), READY | DRQ);
        read_sector(bus)
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect()
    }
    fn assert_error(bus: &mut MockBus, error: u8) {
        assert!(bus.device().irq_pending());
        assert_eq!(status(bus), READY | ERR);
        assert_eq!(get(bus, 1), error);
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xFFFF);
    }
    fn set_multiple(bus: &mut MockBus, count: u8) {
        put(bus, 2, count);
        command(bus, 0xC6);
        assert_eq!(status(bus), READY);
    }

    #[test]
    fn identify_reports_geometry_capacity_strings_and_only_pio_capabilities() {
        let mut bus = bus();
        let words = identify(&mut bus);
        assert_eq!(words.len(), 256);
        assert_eq!((words[0], words[1], words[3], words[6]), (0x40, 8, 2, 4));
        assert_eq!(&words[47..50], &[0x8010, 0, 0x0E00]);
        assert_eq!(&words[53..65], &[3, 8, 2, 4, 64, 0, 0x100, 64, 0, 0, 0, 3]);
        for (start, len, expected) in [
            (10, 10, "UNEMU000000000000001"),
            (23, 4, "1.0     "),
            (27, 20, "UNEMU ATA-2 PIO DISK                    "),
        ] {
            let bytes: Vec<_> = words[start..start + len]
                .iter()
                .flat_map(|w| w.to_be_bytes())
                .collect();
            assert_eq!(String::from_utf8(bytes).unwrap(), expected);
        }
        assert_eq!(status(&mut bus), READY);
        assert!(!bus.device().irq_pending());
    }

    #[test]
    fn sector_commands_round_trip_every_byte_with_and_without_retries() {
        for (read, write) in [(0x20, 0x30), (0x21, 0x31)] {
            let mut bus = bus();
            let payload: Vec<_> = (0..SECTOR).map(|i| (i ^ (i >> 8)) as u8).collect();
            lba(&mut bus, 17, 2, false);
            command(&mut bus, write);
            assert!(!bus.device().irq_pending());
            assert_eq!(alt(&mut bus), READY | DRQ);
            write_sector(&mut bus, &payload);
            assert!(bus.device().irq_pending());
            assert_eq!(get(&mut bus, 2), 1);
            assert_eq!(get(&mut bus, 3), 18);
            assert_eq!(status(&mut bus), READY | DRQ);
            write_sector(&mut bus, &vec![0x69; SECTOR]);
            assert_eq!(get(&mut bus, 2), 0);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY);
            lba(&mut bus, 17, 2, false);
            command(&mut bus, read);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY | DRQ);
            assert_eq!(read_sector(&mut bus), payload);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY | DRQ);
            assert_eq!(read_sector(&mut bus), vec![0x69; SECTOR]);
            assert_eq!(get(&mut bus, 2), 0);
            assert_eq!(status(&mut bus), READY);
            assert!(!bus.device().irq_pending());
        }
    }

    #[test]
    fn chs_crosses_head_and_cylinder_boundaries_without_changing_lba_capacity() {
        let mut bus = bus();
        chs(&mut bus, 0, 1, 4, 2);
        command(&mut bus, 0x30);
        write_sector(&mut bus, &vec![0x12; SECTOR]);
        assert_eq!(
            (get(&mut bus, 4), get(&mut bus, 6) & 15, get(&mut bus, 3)),
            (1, 0, 1)
        );
        write_sector(&mut bus, &vec![0x34; SECTOR]);
        lba(&mut bus, 7, 2, false);
        command(&mut bus, 0x20);
        assert_eq!(read_sector(&mut bus), vec![0x12; SECTOR]);
        assert_eq!(read_sector(&mut bus), vec![0x34; SECTOR]);
    }

    #[test]
    fn sector_count_zero_means_256_and_lba_bytes_do_not_alias() {
        let geometry = AtaGeometry {
            cylinders: 512,
            heads: 1,
            sectors_per_track: 1,
        };
        let mut image = vec![0; 512 * SECTOR];
        for (i, sector) in image.as_chunks_mut::<SECTOR>().0.iter_mut().enumerate() {
            sector.fill(i as u8);
        }
        let mut bus = MockBus::new(Box::new(
            Ata::with_drives([Some(AtaDrive::new(geometry, image).unwrap()), None]).unwrap(),
        ));
        lba(&mut bus, 256, 0, false);
        command(&mut bus, 0x20);
        for i in 0..256 {
            assert_eq!(status(&mut bus), READY | DRQ);
            assert_eq!(read_sector(&mut bus), vec![i as u8; SECTOR]);
        }
        assert_eq!(status(&mut bus), READY);
        assert_eq!(get(&mut bus, 2), 0);
        for address in [512, 0x10000, 0x1000000, 0xFFFFFFF] {
            lba(&mut bus, address, 1, false);
            command(&mut bus, 0x20);
            assert_error(&mut bus, IDNF);
        }
    }

    #[test]
    fn all_media_commands_reject_invalid_lba_and_chs() {
        for opcode in [0x20, 0x21, 0x30, 0x31, 0xC4, 0xC5, 0x70, 0x7F] {
            let mut bus = bus();
            set_multiple(&mut bus, 2);
            lba(&mut bus, 64, 1, false);
            command(&mut bus, opcode);
            if opcode == 0xC4 {
                status(&mut bus);
                read_sector(&mut bus);
                read_sector(&mut bus);
            }
            if opcode == 0xC5 {
                write_sector(&mut bus, &vec![0; SECTOR]);
                write_sector(&mut bus, &vec![0; SECTOR]);
            }
            if opcode == 0xC4 {
                assert_eq!(status(&mut bus), READY | ERR);
                assert_eq!(get(&mut bus, 1), IDNF);
            } else {
                assert_error(&mut bus, IDNF);
            }
            for (cylinder, head, sector) in
                [(8, 0, 1), (0, 2, 1), (0, 0, 0), (0, 0, 5), (0xFFFF, 0, 1)]
            {
                chs(&mut bus, cylinder, head, sector, 1);
                command(&mut bus, opcode);
                assert_error(&mut bus, IDNF);
            }
        }
    }

    #[test]
    fn crossing_end_of_media_preserves_completed_sectors_and_posts_failing_address() {
        for opcode in [0x20, 0x21, 0x30, 0x31] {
            let mut bus = bus();
            set_multiple(&mut bus, 1);
            lba(&mut bus, 63, 2, false);
            command(&mut bus, opcode);
            status(&mut bus);
            if matches!(opcode, 0x30 | 0x31 | 0xC5) {
                write_sector(&mut bus, &vec![0x5A; SECTOR]);
            } else {
                assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
            }
            assert_error(&mut bus, IDNF);
            assert_eq!(get(&mut bus, 2), 1);
            assert_eq!(get(&mut bus, 3), 64);
            lba(&mut bus, 63, 1, false);
            command(&mut bus, 0x20);
            status(&mut bus);
            assert_eq!(
                read_sector(&mut bus),
                vec![
                    if matches!(opcode, 0x30 | 0x31 | 0xC5) {
                        0x5A
                    } else {
                        0xA5
                    };
                    SECTOR
                ]
            );
        }
    }

    #[test]
    fn multiple_commands_interrupt_only_at_full_and_partial_block_boundaries() {
        for block in [1, 2, 4, 8, 16] {
            let mut bus = bus();
            set_multiple(&mut bus, block);
            let words = identify(&mut bus);
            assert_eq!(words[59], 0x100 | u16::from(block));
            let sectors = block * 2 + 1;
            lba(&mut bus, 0, sectors, false);
            command(&mut bus, 0xC5);
            assert!(!bus.device().irq_pending());
            for i in 0..sectors {
                write_sector(&mut bus, &vec![i; SECTOR]);
                let interrupt = (i + 1) % block == 0 || i + 1 == sectors;
                assert_eq!(
                    bus.device().irq_pending(),
                    interrupt,
                    "write block {block} sector {i}"
                );
                assert_eq!(
                    status(&mut bus),
                    if i + 1 == sectors { READY } else { READY | DRQ }
                );
            }
            lba(&mut bus, 0, sectors, false);
            command(&mut bus, 0xC4);
            assert!(bus.device().irq_pending());
            status(&mut bus);
            for i in 0..sectors {
                assert_eq!(read_sector(&mut bus), vec![i; SECTOR]);
                let interrupt = (i + 1) % block == 0 && i + 1 < sectors;
                assert_eq!(
                    bus.device().irq_pending(),
                    interrupt,
                    "read block {block} sector {i}"
                );
                assert_eq!(
                    status(&mut bus),
                    if i + 1 == sectors { READY } else { READY | DRQ }
                );
            }
        }
    }

    #[test]
    fn invalid_multiple_mode_disables_transfers_and_zero_disables_cleanly() {
        for invalid in [3, 17, 32, 255] {
            let mut bus = bus();
            set_multiple(&mut bus, 2);
            put(&mut bus, 2, invalid);
            command(&mut bus, 0xC6);
            assert_error(&mut bus, ABRT);
            for opcode in [0xC4, 0xC5] {
                lba(&mut bus, 0, 1, false);
                command(&mut bus, opcode);
                assert_error(&mut bus, ABRT);
            }
        }
        let mut bus = bus();
        for opcode in [0xC4, 0xC5] {
            lba(&mut bus, 0, 1, false);
            command(&mut bus, opcode);
            assert_error(&mut bus, ABRT);
        }
        set_multiple(&mut bus, 16);
        set_multiple(&mut bus, 0);
        command(&mut bus, 0xC4);
        assert_error(&mut bus, ABRT);
    }

    #[test]
    fn irq_status_acknowledges_alternate_status_preserves_and_nien_gates() {
        let mut bus = bus();
        control(&mut bus, 2);
        command(&mut bus, 0xEC);
        assert!(!bus.device().irq_pending());
        assert_eq!(alt(&mut bus), READY | DRQ);
        control(&mut bus, 0);
        assert!(bus.device().irq_pending());
        for _ in 0..4 {
            assert_eq!(alt(&mut bus), READY | DRQ);
            assert!(bus.device().irq_pending());
        }
        control(&mut bus, 2);
        assert!(!bus.device().irq_pending());
        assert_eq!(status(&mut bus), READY | DRQ);
        control(&mut bus, 0);
        assert!(!bus.device().irq_pending());
        read_sector(&mut bus);
        assert!(!bus.device().irq_pending());
        command(&mut bus, 0);
        assert!(bus.device().irq_pending());
        assert_eq!(alt(&mut bus), READY | ERR);
        assert!(bus.device().irq_pending());
        assert_error(&mut bus, ABRT);
    }

    #[test]
    fn reset_cancels_partial_writes_posts_signature_and_preserves_media() {
        let mut bus = bus();
        set_multiple(&mut bus, 4);
        lba(&mut bus, 0, 2, false);
        command(&mut bus, 0x30);
        write_sector(&mut bus, &vec![0x5A; SECTOR]);
        bus.port_write(0x1F0, 2, 0x1234).unwrap();
        control(&mut bus, 6);
        assert_eq!(alt(&mut bus), BSY);
        assert!(!bus.device().irq_pending());
        command(&mut bus, 0xEC);
        put(&mut bus, 2, 77);
        bus.port_write(0x1F0, 2, 0xFFFF).unwrap();
        control(&mut bus, 2);
        assert_eq!(
            (
                get(&mut bus, 1),
                get(&mut bus, 2),
                get(&mut bus, 3),
                get(&mut bus, 4),
                get(&mut bus, 5),
                get(&mut bus, 6)
            ),
            (1, 1, 1, 0, 0, 0)
        );
        assert_eq!(status(&mut bus), READY);
        assert!(!bus.device().irq_pending());
        command(&mut bus, 0xEC);
        assert!(!bus.device().irq_pending());
        read_sector(&mut bus);
        control(&mut bus, 0);
        bus.device_mut().reset();
        assert_eq!(identify(&mut bus)[59], 0x100);
        lba(&mut bus, 0, 2, false);
        command(&mut bus, 0x20);
        status(&mut bus);
        assert_eq!(read_sector(&mut bus), vec![0x5A; SECTOR]);
        status(&mut bus);
        assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
    }

    #[test]
    fn initialize_parameters_translates_chs_and_reset_restores_defaults() {
        let mut bus = bus();
        put(&mut bus, 6, 0xA3);
        put(&mut bus, 2, 2);
        command(&mut bus, 0x91);
        assert!(bus.device().irq_pending());
        assert_eq!(status(&mut bus), READY);
        let words = identify(&mut bus);
        assert_eq!(&words[54..59], &[8, 4, 2, 64, 0]);
        chs(&mut bus, 1, 3, 2, 1);
        command(&mut bus, 0x30);
        write_sector(&mut bus, &vec![0xC3; SECTOR]);
        lba(&mut bus, 15, 1, false);
        command(&mut bus, 0x20);
        assert_eq!(read_sector(&mut bus), vec![0xC3; SECTOR]);
        put(&mut bus, 2, 0);
        command(&mut bus, 0x91);
        assert_error(&mut bus, ABRT);
        put(&mut bus, 6, 0xAF);
        put(&mut bus, 2, 255);
        command(&mut bus, 0x91);
        assert_error(&mut bus, ABRT);
        assert_eq!(&identify(&mut bus)[54..59], &[8, 4, 2, 64, 0]);
        control(&mut bus, 4);
        control(&mut bus, 0);
        assert_eq!(&identify(&mut bus)[54..59], &[8, 2, 4, 64, 0]);
    }

    #[test]
    fn set_features_accepts_only_supported_pio_modes() {
        let mut bus = bus();
        for mode in [0, 1, 8, 9, 10, 11, 12] {
            put(&mut bus, 1, 3);
            put(&mut bus, 2, mode);
            command(&mut bus, 0xEF);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY);
            lba(&mut bus, 0, 1, false);
            command(&mut bus, 0x20);
            status(&mut bus);
            assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
        }
        for mode in [2, 7, 13, 0x10, 0x20, 0xFF] {
            put(&mut bus, 1, 3);
            put(&mut bus, 2, mode);
            command(&mut bus, 0xEF);
            assert_error(&mut bus, ABRT);
        }
        for feature in [0, 1, 2, 0x81, 0xFF] {
            put(&mut bus, 1, feature);
            command(&mut bus, 0xEF);
            assert_error(&mut bus, ABRT);
        }
    }

    #[test]
    fn seek_recalibrate_and_diagnostics_complete_without_data() {
        let mut bus = bus();
        for opcode in [0x70, 0x7F] {
            chs(&mut bus, 7, 1, 4, 1);
            command(&mut bus, opcode);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY);
        }
        for opcode in [0x10, 0x1F] {
            chs(&mut bus, 7, 1, 4, 1);
            command(&mut bus, opcode);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY);
            assert_eq!((get(&mut bus, 4), get(&mut bus, 5)), (0, 0));
        }
        put(&mut bus, 6, 0xF0);
        command(&mut bus, 0x90);
        assert!(bus.device().irq_pending());
        assert_eq!(status(&mut bus), READY);
        assert_eq!(
            (
                get(&mut bus, 1),
                get(&mut bus, 2),
                get(&mut bus, 3),
                get(&mut bus, 4),
                get(&mut bus, 5),
                get(&mut bus, 6)
            ),
            (1, 1, 1, 0, 0, 0)
        );
    }

    #[test]
    fn absent_slave_echoes_master_registers_but_never_completes_commands() {
        for opcode in [
            0xEC, 0x20, 0x21, 0x30, 0x31, 0x40, 0x41, 0xC4, 0xC5, 0xC6, 0x91, 0xEF, 0x10, 0x70, 0,
        ] {
            let mut bus = bus();
            lba(&mut bus, 0x55AA, 0x55, true);
            put(&mut bus, 1, 3);
            command(&mut bus, opcode);
            assert_eq!(get(&mut bus, 1), 1);
            assert_eq!(get(&mut bus, 2), 0x55);
            assert_eq!(get(&mut bus, 3), 0xAA);
            assert_eq!(get(&mut bus, 4), 0x55);
            assert_eq!(get(&mut bus, 5), 0);
            assert_eq!(get(&mut bus, 6), 0xF0);
            assert_eq!(status(&mut bus), 0);
            assert_eq!(alt(&mut bus), 0);
            assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xFFFF);
            assert!(!bus.device().irq_pending());
            put(&mut bus, 6, 0xE0);
            assert_eq!(status(&mut bus), READY);
        }
    }

    #[test]
    fn master_and_slave_media_translation_modes_and_irqs_are_independent() {
        let mut bus = MockBus::new(Box::new(
            Ata::with_drives([Some(disk(0xA5)), Some(disk(0x69))]).unwrap(),
        ));
        let master_serial = identify(&mut bus)[10..20].to_vec();
        lba(&mut bus, 0, 1, true);
        let slave_serial = identify(&mut bus)[10..20].to_vec();
        assert_ne!(master_serial, slave_serial);
        set_multiple(&mut bus, 4);
        put(&mut bus, 6, 0xE0);
        assert_eq!(identify(&mut bus)[59], 0x100);
        lba(&mut bus, 0, 1, true);
        command(&mut bus, 0x30);
        write_sector(&mut bus, &vec![0x42; SECTOR]);
        assert!(bus.device().irq_pending());
        put(&mut bus, 6, 0xE0);
        assert!(!bus.device().irq_pending());
        put(&mut bus, 6, 0xF0);
        assert!(bus.device().irq_pending());
        status(&mut bus);
        for (slave, expected) in [(false, 0xA5), (true, 0x42)] {
            lba(&mut bus, 0, 1, slave);
            command(&mut bus, 0x20);
            status(&mut bus);
            assert_eq!(read_sector(&mut bus), vec![expected; SECTOR]);
        }
        command(&mut bus, 0x90);
        assert!(bus.device().irq_pending());
        status(&mut bus);
        put(&mut bus, 6, 0xF0);
        assert!(!bus.device().irq_pending());
        assert_eq!(get(&mut bus, 1), 1);
        control(&mut bus, 4);
        control(&mut bus, 0);
        put(&mut bus, 6, 0xF0);
        assert_eq!(
            (
                get(&mut bus, 2),
                get(&mut bus, 3),
                get(&mut bus, 4),
                get(&mut bus, 5)
            ),
            (1, 1, 0, 0)
        );
    }

    #[test]
    fn unsupported_commands_abort_and_a_valid_command_clears_error() {
        let mut bus = bus();
        for opcode in [0, 0x22, 0xC8, 0xCA, 0xA1, 0xE7, 0xFF] {
            command(&mut bus, opcode);
            assert_error(&mut bus, ABRT);
            assert_eq!(identify(&mut bus)[0], 0x40);
            assert_eq!(get(&mut bus, 1), 0);
        }
    }

    #[test]
    fn wrong_direction_width_or_idle_data_access_cannot_consume_a_word() {
        let mut bus = bus();
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xFFFF);
        let initial = bus.device().save();
        bus.port_write(0x1F0, 2, 0).unwrap();
        assert_eq!(bus.device().save(), initial);
        for (port, width) in [
            (0x1F0, 1),
            (0x1F0, 4),
            (0x1F1, 2),
            (0x3F6, 2),
            (0x1EF, 1),
            (0x1F8, 1),
        ] {
            assert!(bus.port_read(port, width).is_err());
            assert!(bus.port_write(port, width, 0).is_err());
        }
        assert!(bus.mmio_read(0, 1).is_err());
        assert!(bus.mmio_write(0, 1, 0).is_err());
        command(&mut bus, 0xEC);
        status(&mut bus);
        let before = bus.device().save();
        bus.port_write(0x1F0, 2, 0).unwrap();
        assert_eq!(bus.device().save(), before);
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0x40);
        control(&mut bus, 4);
        control(&mut bus, 0);
        lba(&mut bus, 0, 1, false);
        command(&mut bus, 0x30);
        let before = bus.device().save();
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xFFFF);
        assert_eq!(bus.device().save(), before);
    }

    #[test]
    fn snapshots_continue_identify_reads_and_writes_at_word_and_block_boundaries() {
        for opcode in [0xEC, 0x20, 0x21, 0x30, 0x31, 0xC4, 0xC5] {
            for prefix in [0, 1, 255, 256, 257, 511, 512] {
                if opcode == 0xEC && prefix >= 256 {
                    continue;
                }
                let mut source = bus();
                set_multiple(&mut source, 2);
                lba(&mut source, 0, 3, false);
                command(&mut source, opcode);
                let write = matches!(opcode, 0x30 | 0x31 | 0xC5);
                for i in 0..prefix {
                    if write {
                        source.port_write(0x1F0, 2, i).unwrap();
                    } else {
                        source.port_read(0x1F0, 2).unwrap();
                    }
                }
                source.device_mut().sync(VClock(137));
                let saved = source.device().save();
                let mut restored = bus();
                // Completed writes are media, so supply matching media before restore.
                if write {
                    set_multiple(&mut restored, 2);
                    lba(&mut restored, 0, 3, false);
                    command(&mut restored, opcode);
                    for i in 0..prefix {
                        restored.port_write(0x1F0, 2, i).unwrap();
                    }
                }
                restored.device_mut().restore(&saved).unwrap();
                assert_eq!(restored.device().save(), saved);
                let end = if opcode == 0xEC { 256 } else { 768 };
                for i in prefix..end {
                    if write {
                        source.port_write(0x1F0, 2, i).unwrap();
                        restored.port_write(0x1F0, 2, i).unwrap();
                    } else {
                        assert_eq!(source.port_read(0x1F0, 2), restored.port_read(0x1F0, 2));
                    }
                    assert_eq!(
                        source.device().irq_pending(),
                        restored.device().irq_pending()
                    );
                }
                assert_eq!(source.device().save(), restored.device().save());
                if write {
                    for device in [&mut source, &mut restored] {
                        lba(device, 0, 3, false);
                        command(device, 0x20);
                    }
                    for i in 0..768 {
                        let expected = i;
                        assert_eq!(source.port_read(0x1F0, 2).unwrap(), expected);
                        assert_eq!(restored.port_read(0x1F0, 2).unwrap(), expected);
                    }
                }
            }
        }
    }

    #[test]
    fn malformed_snapshots_reject_without_mutating_hardware_media_or_trace() {
        let mut bus = bus();
        bus.device_mut()
            .set_trace_policy(TracePolicy::Ring { capacity: 16 });
        lba(&mut bus, 0, 3, false);
        command(&mut bus, 0x30);
        bus.port_write(0x1F0, 2, 0x1234).unwrap();
        let valid = bus.device().save();
        let mut invalid = Vec::new();
        for length in 0..valid.len() {
            invalid.push(valid[..length].to_vec());
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        invalid.push(trailing);
        // Header, mounted media, task flags, geometry and PIO cursor invariants.
        for (offset, value) in [
            (0, 4),
            (1, 0),
            (5, 16),
            (6, 2),
            (7, 1),
            (16, 0),
            (17, 0),
            (21, 0),
            (32, 0),
            (33, 2),
            (34, 0),
            (36, 17),
            (38, 3),
            (39, 4),
            (44, 0),
            (46, 1),
            (48, 0),
            (49, 3),
        ] {
            let mut blob = valid.clone();
            blob[offset] = value;
            if blob != valid {
                invalid.push(blob);
            }
        }
        for blob in invalid {
            assert!(matches!(
                bus.device_mut().restore(&blob),
                Err(DeviceError::InvalidSnapshot { .. })
            ));
            assert_eq!(bus.device().save(), valid);
        }
        assert_eq!(bus.device().trace_dropped(), 0);
        assert_eq!(bus.device_mut().take_trace().len(), 7);
        let mut empty = Ata::new();
        assert!(empty.restore(&valid).is_err());
        let mut different = Ata::with_drives([
            Some(
                AtaDrive::new(
                    AtaGeometry {
                        cylinders: 4,
                        heads: 2,
                        sectors_per_track: 4,
                    },
                    vec![0; 32 * SECTOR],
                )
                .unwrap(),
            ),
            None,
        ])
        .unwrap();
        assert!(different.restore(&valid).is_err());
    }

    #[test]
    fn restore_does_not_roll_back_media_and_reset_does_not_eject_it() {
        let mut ata = controller();
        let before = ata.save();
        assert_eq!(ata.drive(0).unwrap().image()[0], 0xA5);
        let mut bus = MockBus::new(Box::new(ata));
        lba(&mut bus, 0, 1, false);
        command(&mut bus, 0x30);
        write_sector(&mut bus, &vec![0x69; SECTOR]);
        bus.device_mut().restore(&before).unwrap();
        bus.device_mut().reset();
        lba(&mut bus, 0, 1, false);
        command(&mut bus, 0x20);
        assert_eq!(read_sector(&mut bus), vec![0x69; SECTOR]);
        ata = controller();
        ata.reset();
        assert!(ata.drive(0).is_some());
        assert!(ata.drive(2).is_none());
    }

    #[test]
    fn dormant_scheduler_matches_per_clock_ticks_in_every_pio_phase_and_reset() {
        let mut stepped = bus();
        let mut skipped = bus();
        for command_byte in [0xEC, 0x20, 0x30, 0xFF] {
            for device in [&mut stepped, &mut skipped] {
                control(device, 4);
                control(device, 0);
                lba(device, 0, 2, false);
                command(device, command_byte);
            }
            let start = stepped.clock().0;
            for _ in 0..127 {
                stepped.tick(1);
            }
            skipped.tick(127);
            assert_eq!(
                stepped.device().next_event(VClock(start + 127)),
                NextEvent::Dormant
            );
            assert_eq!(stepped.device().save(), skipped.device().save());
            assert_eq!(
                stepped.device().irq_pending(),
                skipped.device().irq_pending()
            );
            if command_byte == 0x30 {
                stepped.port_write(0x1F0, 2, 0x1234).unwrap();
                skipped.port_write(0x1F0, 2, 0x1234).unwrap();
            } else {
                assert_eq!(stepped.port_read(0x1F0, 2), skipped.port_read(0x1F0, 2));
            }
            for device in [&mut stepped, &mut skipped] {
                control(device, 4);
            }
            stepped.tick(1);
            skipped.tick(1);
            assert_eq!(stepped.device().save(), skipped.device().save());
            assert_eq!(
                stepped.device().next_event(stepped.clock()),
                NextEvent::Dormant
            );
        }
    }

    #[test]
    fn resources_and_media_validation_reject_invalid_host_configuration() {
        for (geometry, length) in [
            (GEOMETRY, 0),
            (GEOMETRY, 513),
            (GEOMETRY, 512),
            (
                AtaGeometry {
                    heads: 17,
                    ..GEOMETRY
                },
                64 * SECTOR,
            ),
            (
                AtaGeometry {
                    cylinders: 0,
                    ..GEOMETRY
                },
                64 * SECTOR,
            ),
            (
                AtaGeometry {
                    sectors_per_track: 0,
                    ..GEOMETRY
                },
                64 * SECTOR,
            ),
        ] {
            assert!(AtaDrive::new(geometry, vec![0; length]).is_err());
        }
        for (base, control, irq) in [(0xFFFA, 0, 14), (0x1F0, 0x1F7, 14), (0x1F0, 0x3F6, 16)] {
            assert!(Ata::new().with_resources(base, control, irq).is_err());
        }
        let ata = controller().with_resources(0x170, 0x376, 15).unwrap();
        assert_eq!(ata.irq_number(), 15);
        let mut bus = MockBus::new(Box::new(ata));
        assert_eq!(bus.port_read(0x177, 1).unwrap(), u32::from(READY));
        bus.port_write(0x177, 1, 0xEC).unwrap();
        assert_eq!(bus.port_read(0x376, 1).unwrap(), u32::from(READY | DRQ));
        assert_eq!(bus.port_read(0x170, 2).unwrap(), 0x40);
        assert!(bus.port_read(0x1F7, 1).is_err());
    }

    #[test]
    fn verify_retries_check_chs_lba_counts_and_post_the_first_failure() {
        for opcode in [0x40, 0x41] {
            let mut bus = bus();
            control(&mut bus, 2);
            lba(&mut bus, 7, 3, false);
            command(&mut bus, opcode);
            assert_eq!(alt(&mut bus), READY);
            assert!(!bus.device().irq_pending());
            control(&mut bus, 0);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY);
            assert_eq!(get(&mut bus, 2), 0);
            assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xFFFF);
            chs(&mut bus, 0, 1, 4, 3);
            command(&mut bus, opcode);
            assert_eq!(status(&mut bus), READY);
            assert_eq!(
                (get(&mut bus, 4), get(&mut bus, 6) & 15, get(&mut bus, 3)),
                (1, 0, 2)
            );
            for chs_mode in [false, true] {
                if chs_mode {
                    chs(&mut bus, 7, 1, 4, 3);
                } else {
                    lba(&mut bus, 63, 3, false);
                }
                command(&mut bus, opcode);
                assert_error(&mut bus, IDNF);
                assert_eq!(get(&mut bus, 2), 2);
                if chs_mode {
                    assert_eq!(
                        (get(&mut bus, 4), get(&mut bus, 6) & 15, get(&mut bus, 3)),
                        (8, 0, 1)
                    );
                } else {
                    assert_eq!(get(&mut bus, 3), 64);
                }
            }
            lba(&mut bus, 64, 4, false);
            command(&mut bus, opcode);
            assert_error(&mut bus, IDNF);
            assert_eq!(get(&mut bus, 2), 4);
            chs(&mut bus, 0, 2, 1, 4);
            command(&mut bus, opcode);
            assert_error(&mut bus, IDNF);
            assert_eq!(get(&mut bus, 2), 4);
            lba(&mut bus, 0, 0, false);
            command(&mut bus, opcode);
            assert_error(&mut bus, IDNF);
            assert_eq!(get(&mut bus, 2), 192);
            assert_eq!(get(&mut bus, 3), 64);
            let drive = AtaDrive::new(
                AtaGeometry::new(512, 1, 1).unwrap(),
                vec![0x69; 512 * SECTOR],
            )
            .unwrap();
            let mut bus = MockBus::new(Box::new(Ata::with_drives([Some(drive), None]).unwrap()));
            lba(&mut bus, 100, 0, false);
            command(&mut bus, opcode);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY);
            assert_eq!(get(&mut bus, 2), 0);
            lba(&mut bus, 100, 1, false);
            command(&mut bus, 0x20);
            assert_eq!(read_sector(&mut bus), vec![0x69; SECTOR]);
        }
    }

    #[test]
    fn diagnostics_retain_translation_and_multiple_mode_for_subsequent_reads() {
        let mut bus = bus();
        put(&mut bus, 6, 0xA3);
        put(&mut bus, 2, 2);
        command(&mut bus, 0x91);
        status(&mut bus);
        set_multiple(&mut bus, 2);
        command(&mut bus, 0x90);
        assert_eq!(status(&mut bus), READY);
        let words = identify(&mut bus);
        assert_eq!(&words[54..59], &[8, 4, 2, 64, 0]);
        assert_eq!(words[59], 0x102);
        chs(&mut bus, 0, 3, 2, 3);
        command(&mut bus, 0xC4);
        assert_eq!(status(&mut bus), READY | DRQ);
        assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
        assert!(!bus.device().irq_pending());
        assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
        assert!(bus.device().irq_pending());
        status(&mut bus);
        assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
        assert_eq!(status(&mut bus), READY);
    }

    #[test]
    fn recalibrate_posts_chs_or_lba_origin_for_every_opcode() {
        for opcode in 0x10..=0x1F {
            let mut bus = bus();
            chs(&mut bus, 7, 1, 4, 3);
            command(&mut bus, opcode);
            assert_eq!(status(&mut bus), READY);
            assert_eq!(
                (
                    get(&mut bus, 6),
                    get(&mut bus, 3),
                    get(&mut bus, 4),
                    get(&mut bus, 5)
                ),
                (0xA0, 1, 0, 0)
            );
            lba(&mut bus, 0x07123456, 3, false);
            command(&mut bus, opcode);
            assert_eq!(status(&mut bus), READY);
            assert_eq!(
                (
                    get(&mut bus, 6),
                    get(&mut bus, 3),
                    get(&mut bus, 4),
                    get(&mut bus, 5)
                ),
                (0xE0, 0, 0, 0)
            );
        }
    }

    #[test]
    fn rejected_translation_blocks_all_media_until_valid_parameters_or_reset() {
        let mut bus = bus();
        set_multiple(&mut bus, 2);
        put(&mut bus, 2, 0);
        command(&mut bus, 0x91);
        assert_error(&mut bus, ABRT);
        let saved = bus.device().save();
        let mut restored = self::bus();
        restored.device_mut().restore(&saved).unwrap();
        for device in [&mut bus, &mut restored] {
            command(device, 0x90);
            status(device);
            for opcode in [0x20, 0x21, 0x30, 0x31, 0x40, 0x41, 0xC4, 0xC5, 0x70] {
                for chs_mode in [false, true] {
                    if chs_mode {
                        chs(device, 0, 0, 1, 1);
                    } else {
                        lba(device, 0, 1, false);
                    }
                    command(device, opcode);
                    assert_error(device, IDNF);
                }
            }
            assert_eq!(identify(device)[53] & 1, 0);
            put(device, 6, 0xA1);
            put(device, 2, 4);
            command(device, 0x91);
            assert_eq!(status(device), READY);
            lba(device, 0, 1, false);
            command(device, 0x20);
            assert_eq!(read_sector(device), vec![0xA5; SECTOR]);
            put(device, 2, 0);
            command(device, 0x91);
            assert_error(device, ABRT);
            control(device, 4);
            control(device, 0);
            lba(device, 0, 1, false);
            command(device, 0x40);
            assert_eq!(status(device), READY);
        }
    }

    #[test]
    fn default_parameters_select_default_geometry_when_lba_capacity_is_larger() {
        let drive = AtaDrive::new(GEOMETRY, vec![0; 80 * SECTOR]).unwrap();
        let mut bus = MockBus::new(Box::new(Ata::with_drives([Some(drive), None]).unwrap()));
        put(&mut bus, 6, 0xA0);
        put(&mut bus, 2, 4);
        command(&mut bus, 0x91);
        status(&mut bus);
        assert_eq!(&identify(&mut bus)[54..59], &[20, 1, 4, 80, 0]);
        put(&mut bus, 6, 0xA1);
        put(&mut bus, 2, 4);
        command(&mut bus, 0x91);
        status(&mut bus);
        let words = identify(&mut bus);
        assert_eq!(&words[54..59], &[8, 2, 4, 64, 0]);
        assert_eq!(words[60], 80);
        chs(&mut bus, 8, 0, 1, 1);
        command(&mut bus, 0x40);
        assert_error(&mut bus, IDNF);
        lba(&mut bus, 79, 1, false);
        command(&mut bus, 0x40);
        assert_eq!(status(&mut bus), READY);
    }

    #[test]
    fn absent_slave_data_echo_does_not_acknowledge_master_interrupt() {
        let mut bus = bus();
        command(&mut bus, 0xEC);
        assert!(bus.device().irq_pending());
        put(&mut bus, 6, 0xF0);
        assert_eq!(status(&mut bus), 0);
        assert_eq!(alt(&mut bus), 0);
        assert_eq!(get(&mut bus, 1), 0);
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0x40);
        put(&mut bus, 6, 0xE0);
        assert!(bus.device().irq_pending());
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 8);
    }

    #[test]
    fn multiple_address_errors_finish_before_data_and_post_the_failing_address() {
        for opcode in [0xC4, 0xC5] {
            for (start, count, block, transferred) in [(62, 5, 2, 2), (63, 4, 4, 0), (64, 3, 4, 0)]
            {
                for chs_mode in [false, true] {
                    if chs_mode && start == 64 {
                        continue;
                    }
                    let mut bus = bus();
                    set_multiple(&mut bus, block);
                    if chs_mode {
                        chs(&mut bus, 7, 1, (start - 60 + 1) as u8, count);
                    } else {
                        lba(&mut bus, start, count, false);
                    }
                    command(&mut bus, opcode);
                    for _ in 0..transferred {
                        assert_eq!(status(&mut bus), READY | DRQ);
                        if opcode == 0xC4 {
                            assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
                        } else {
                            write_sector(&mut bus, &vec![0x5A; SECTOR]);
                        }
                    }
                    assert_error(&mut bus, IDNF);
                    if chs_mode {
                        assert_eq!(
                            (get(&mut bus, 4), get(&mut bus, 6) & 15, get(&mut bus, 3)),
                            (8, 0, 1)
                        );
                    } else {
                        assert_eq!(get(&mut bus, 3), 64);
                    }
                    assert_eq!(get(&mut bus, 2), 3);
                    lba(&mut bus, start.min(63), 1, false);
                    command(&mut bus, 0x20);
                    assert_eq!(
                        read_sector(&mut bus),
                        vec![
                            if opcode == 0xC5 && transferred != 0 {
                                0x5A
                            } else {
                                0xA5
                            };
                            SECTOR
                        ]
                    );
                }
            }
        }
    }

    #[test]
    fn command_write_during_drq_replaces_transfer_and_posts_its_own_irq() {
        let mut bus = bus();
        command(&mut bus, 0xEC);
        assert!(bus.device().irq_pending());
        command(&mut bus, 0);
        assert_error(&mut bus, ABRT);
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xFFFF);
        lba(&mut bus, 0, 2, false);
        command(&mut bus, 0x30);
        write_sector(&mut bus, &vec![0x42; SECTOR]);
        assert!(bus.device().irq_pending());
        command(&mut bus, 0xEC);
        assert!(bus.device().irq_pending());
        assert_eq!(status(&mut bus), READY | DRQ);
        assert_eq!(read_sector(&mut bus)[..2], [0x40, 0]);
        assert_eq!(status(&mut bus), READY);
    }

    #[test]
    fn a_single_disk_requires_device_zero() {
        assert!(matches!(
            Ata::with_drives([None, Some(disk(0))]),
            Err(AtaConfigError::MissingMaster)
        ));
        assert!(Ata::with_drives([Some(disk(0)), None]).is_ok());
        assert!(Ata::with_drives([Some(disk(0)), Some(disk(1))]).is_ok());
    }

    #[test]
    fn geometry_constructor_and_drive_enforce_xub_identify_bounds() {
        assert!(AtaGeometry::new(16383, 16, 63).is_ok());
        for (cylinders, heads, sectors) in [
            (0, 1, 1),
            (16384, 1, 1),
            (1, 0, 1),
            (1, 17, 1),
            (1, 1, 0),
            (1, 1, 64),
        ] {
            let error = AtaGeometry::new(cylinders, heads, sectors).unwrap_err();
            assert_eq!(error, AtaConfigError::Geometry);
            assert!(error.to_string().contains("16383"));
            assert!(error.to_string().contains("63"));
        }
        for geometry in [
            AtaGeometry {
                cylinders: 16384,
                heads: 1,
                sectors_per_track: 1,
            },
            AtaGeometry {
                cylinders: 1,
                heads: 1,
                sectors_per_track: 64,
            },
        ] {
            assert!(matches!(
                AtaDrive::new(geometry, vec![0; geometry.capacity() as usize * SECTOR]),
                Err(AtaConfigError::Geometry)
            ));
        }
    }

    #[test]
    fn invalid_data_widths_leave_an_active_word_transfer_unchanged() {
        let mut bus = bus();
        command(&mut bus, 0xEC);
        let saved = bus.device().save();
        for width in [1, 4] {
            assert!(bus.port_read(0x1F0, width).is_err());
            assert!(bus.port_write(0x1F0, width, 0).is_err());
            assert_eq!(bus.device().save(), saved);
        }
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0x40);
    }

    fn read_lba_zero_after_abandon(bus: &mut MockBus) {
        for (offset, value) in [(1, 3), (2, 7), (3, 0x21), (4, 0x34), (5, 0x12), (6, 0xE3)] {
            put(bus, offset, value);
        }
        for (offset, value) in [(2, 7), (3, 0x21), (4, 0x34), (5, 0x12), (6, 0xE3)] {
            assert_eq!(get(bus, offset), value);
        }
        lba(bus, 0, 1, false);
        command(bus, 0x20);
        assert_eq!(status(bus), READY | DRQ);
        assert_eq!(get(bus, 1), 0);
        assert_eq!(get(bus, 3), 0);
        assert_eq!(read_sector(bus), vec![0xA5; SECTOR]);
        assert_eq!(status(bus), READY);
        assert_eq!(get(bus, 2), 0);
    }

    #[test]
    fn abandoned_read_multiple_allows_a_new_read_without_reset() {
        let mut bus = bus();
        set_multiple(&mut bus, 2);
        lba(&mut bus, 63, 2, false);
        command(&mut bus, 0xC4);
        assert_eq!(alt(&mut bus), READY | ERR);
        assert_eq!(get(&mut bus, 1), IDNF);
        assert_eq!(get(&mut bus, 3), 64);
        read_lba_zero_after_abandon(&mut bus);
        chs(&mut bus, 0, 0, 2, 4);
        command(&mut bus, 0xC4);
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xA5A5);
        read_lba_zero_after_abandon(&mut bus);
    }

    #[test]
    fn abandoned_write_multiple_discards_partial_sector_and_accepts_a_new_read() {
        let mut bus = bus();
        set_multiple(&mut bus, 2);
        chs(&mut bus, 0, 0, 2, 4);
        command(&mut bus, 0xC5);
        bus.port_write(0x1F0, 2, 0x1234).unwrap();
        assert_eq!(alt(&mut bus), READY | DRQ);
        read_lba_zero_after_abandon(&mut bus);
        lba(&mut bus, 1, 1, false);
        command(&mut bus, 0x20);
        assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
        assert_eq!(status(&mut bus), READY);
    }

    #[test]
    fn abandoned_read_sectors_allows_new_parameters_and_a_new_command() {
        let mut bus = bus();
        chs(&mut bus, 0, 0, 2, 2);
        command(&mut bus, 0x20);
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xA5A5);
        read_lba_zero_after_abandon(&mut bus);
    }

    #[test]
    fn task_writes_during_drq_preserve_latched_addressing_and_snapshot() {
        for opcode in [0x20, 0x30] {
            let mut image = vec![0xA5; 80 * SECTOR];
            image[70 * SECTOR..71 * SECTOR].fill(0x70);
            image[71 * SECTOR..72 * SECTOR].fill(0x71);
            let make = || {
                Ata::with_drives([Some(AtaDrive::new(GEOMETRY, image.clone()).unwrap()), None])
                    .unwrap()
            };
            let mut source = MockBus::new(Box::new(make()));
            lba(&mut source, 70, 2, false);
            command(&mut source, opcode);
            chs(&mut source, 0, 0, 1, 9);
            let saved = source.device().save();
            let mut restored = MockBus::new(Box::new(make()));
            restored.device_mut().restore(&saved).unwrap();
            assert_eq!(restored.device().save(), saved);
            for device in [&mut source, &mut restored] {
                if opcode == 0x20 {
                    assert_eq!(read_sector(device), vec![0x70; SECTOR]);
                    assert_eq!(read_sector(device), vec![0x71; SECTOR]);
                } else {
                    write_sector(device, &vec![0x42; SECTOR]);
                    write_sector(device, &vec![0x69; SECTOR]);
                    assert_eq!(status(device), READY);
                    lba(device, 70, 2, false);
                    command(device, 0x20);
                    assert_eq!(read_sector(device), vec![0x42; SECTOR]);
                    assert_eq!(read_sector(device), vec![0x69; SECTOR]);
                }
                assert_eq!(status(device), READY);
            }
        }
    }

    #[test]
    fn unsupported_snapshot_versions_are_rejected_without_mutation() {
        let mut bus = bus();
        lba(&mut bus, 0, 2, false);
        command(&mut bus, 0x30);
        bus.port_write(0x1F0, 2, 0x1234).unwrap();
        let valid = bus.device().save();
        assert_eq!(valid[0], 1);
        for version in 0..=u8::MAX {
            if version == 1 {
                continue;
            }
            let mut invalid = valid.clone();
            invalid[0] = version;
            assert!(
                matches!(
                    bus.device_mut().restore(&invalid),
                    Err(DeviceError::InvalidSnapshot {
                        reason: "unsupported snapshot version",
                        ..
                    })
                ),
                "accepted or misdiagnosed snapshot version {version}"
            );
            assert_eq!(
                bus.device().save(),
                valid,
                "version {version} mutated state"
            );
        }
        bus.device_mut().restore(&valid).unwrap();
        assert_eq!(bus.device().save(), valid);
    }

    #[test]
    fn posted_address_keeps_live_device_selection_during_pio() {
        for lba_mode in [true, false] {
            for host_lba in [true, false] {
                for opcode in [0x20, 0x30, 0xC4, 0xC5] {
                    let write = matches!(opcode, 0x30 | 0xC5);
                    let mut bus = bus();
                    set_multiple(&mut bus, 2);
                    if lba_mode {
                        lba(&mut bus, 3, 3, false);
                    } else {
                        chs(&mut bus, 0, 0, 4, 3);
                    }
                    command(&mut bus, opcode);
                    if write {
                        bus.port_write(0x1F0, 2, 0x5A5A).unwrap();
                    } else {
                        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xA5A5);
                    }
                    put(&mut bus, 6, 0xB0 | if host_lba { 0x40 } else { 0 });
                    assert_eq!(get(&mut bus, 6) & 0x10, 0x10);
                    assert_eq!(alt(&mut bus), 0);
                    for _ in 1..256 {
                        if write {
                            bus.port_write(0x1F0, 2, 0x5A5A).unwrap();
                        } else {
                            assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0xA5A5);
                        }
                    }
                    assert_eq!(get(&mut bus, 6), if lba_mode { 0xF0 } else { 0xB1 });
                    assert_eq!(get(&mut bus, 3), if lba_mode { 4 } else { 1 });
                    for _ in 0..2 {
                        if write {
                            write_sector(&mut bus, &vec![0x5A; SECTOR]);
                        } else {
                            assert_eq!(read_sector(&mut bus), vec![0xA5; SECTOR]);
                        }
                        assert_eq!(
                            get(&mut bus, 6) & 0x50,
                            0x10 | if lba_mode { 0x40 } else { 0 }
                        );
                    }
                    assert_eq!(status(&mut bus), 0);
                    put(&mut bus, 6, if lba_mode { 0xE0 } else { 0xA0 });
                    assert_eq!(status(&mut bus), READY);
                }
            }
        }
    }

    #[test]
    fn compact_snapshot_preserves_pio_and_rejects_invalid_state_atomically() {
        let mut source = bus();
        lba(&mut source, 0, 1, false);
        command(&mut source, 0x30);
        source.port_write(0x1F0, 2, 0x1234).unwrap();
        source.device_mut().sync(VClock(137));
        let valid = source.device().save();
        assert_eq!(valid[0], 1);
        assert_eq!(valid.len(), 1114);
        let mut target = bus();
        target.device_mut().sync(VClock(77));
        let before = target.device().save();
        let mut padded = valid[..16].to_vec();
        for index in 0..2 {
            let start = 16 + index * 549;
            padded.extend_from_slice(&valid[start..start + 548]);
            padded.extend_from_slice(&u32::MAX.to_le_bytes());
            padded.push(valid[start + 548]);
        }
        assert!(matches!(
            target.device_mut().restore(&padded),
            Err(DeviceError::InvalidSnapshot {
                reason: "snapshot length mismatch",
                ..
            })
        ));
        assert_eq!(target.device().save(), before);
        let mut invalid = valid.clone();
        *invalid.last_mut().unwrap() = 1;
        assert!(matches!(
            target.device_mut().restore(&invalid),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(target.device().save(), before);
        target.device_mut().restore(&valid).unwrap();
        assert_eq!(target.device().save(), valid);
        for device in [&mut source, &mut target] {
            for _ in 1..256 {
                device.port_write(0x1F0, 2, 0x5678).unwrap();
            }
            assert_eq!(status(device), READY);
            lba(device, 0, 1, false);
            command(device, 0x20);
            let bytes = read_sector(device);
            assert_eq!(&bytes[..4], &[0x34, 0x12, 0x78, 0x56]);
            assert_eq!(status(device), READY);
        }
        assert_eq!(source.device().save(), target.device().save());
    }
    type Batches = std::rc::Rc<std::cell::RefCell<Vec<Vec<SectorWrite>>>>;

    struct RecordingMedia {
        memory: InMemoryMedia,
        batches: Batches,
    }
    impl SectorMedia for RecordingMedia {
        fn sector_count(&self) -> u64 {
            self.memory.sector_count()
        }
        fn read_sector(&self, lba: u64) -> [u8; SECTOR] {
            self.memory.read_sector(lba)
        }
        fn write_batch(&mut self, sectors: &[SectorWrite]) {
            self.batches.borrow_mut().push(sectors.to_vec());
            self.memory.write_batch(sectors);
        }
        fn write_protected(&self) -> bool {
            self.memory.write_protected()
        }
    }
    fn recording_bus(protected: bool) -> (MockBus, Batches) {
        let batches = Batches::default();
        let drive = AtaDrive::with_media(
            GEOMETRY,
            64,
            Box::new(RecordingMedia {
                memory: InMemoryMedia::new(vec![0xA5; 64 * SECTOR], 64, protected),
                batches: batches.clone(),
            }),
        )
        .unwrap();
        (
            MockBus::new(Box::new(Ata::with_drives([Some(drive), None]).unwrap())),
            batches,
        )
    }

    #[test]
    fn media_contract_batches_each_write_command_in_transfer_order() {
        for opcode in [0x30, 0x31, 0xC5] {
            let (mut bus, batches) = recording_bus(false);
            put(&mut bus, 2, 2);
            command(&mut bus, 0xC6);
            for count in [1, 3] {
                lba(&mut bus, 5, count, false);
                command(&mut bus, opcode);
                let before = batches.borrow().len();
                for sector in 0..count {
                    write_sector(&mut bus, &[0x40 + sector; SECTOR]);
                    if sector + 1 < count {
                        assert_eq!(batches.borrow().len(), before);
                    }
                }
                assert_eq!(status(&mut bus), READY);
                assert!(!bus.device().irq_pending());
                let expected: Vec<_> = (0..count)
                    .map(|sector| SectorWrite {
                        lba: 5 + u64::from(sector),
                        bytes: [0x40 + sector; SECTOR],
                    })
                    .collect();
                assert_eq!(batches.borrow()[before], expected);
                assert_eq!(batches.borrow().len(), before + 1);
                lba(&mut bus, 5, count, false);
                command(&mut bus, 0x20);
                for sector in 0..count {
                    assert_eq!(read_sector(&mut bus), [0x40 + sector; SECTOR]);
                }
            }
        }
    }

    #[test]
    fn media_contract_interruptions_submit_only_complete_sectors() {
        for end in [0, 1, 2, 3, 4] {
            let (mut bus, batches) = recording_bus(false);
            lba(&mut bus, 2, 3, false);
            command(&mut bus, 0x30);
            write_sector(&mut bus, &[0x55; SECTOR]);
            bus.port_write(0x1F0, 2, 0xEEEE).unwrap();
            assert!(batches.borrow().is_empty());
            match end {
                0 => command(&mut bus, 0xFF),
                1 => command(&mut bus, 0x90),
                2 => control(&mut bus, 4),
                3 => bus.device_mut().reset(),
                _ => {
                    let snapshot = bus.device().save();
                    bus.device_mut().restore(&snapshot).unwrap();
                }
            }
            assert_eq!(
                *batches.borrow(),
                vec![vec![SectorWrite {
                    lba: 2,
                    bytes: [0x55; SECTOR]
                }]]
            );
            if end == 2 {
                control(&mut bus, 0);
            }
            lba(&mut bus, 2, 2, false);
            command(&mut bus, 0x20);
            assert_eq!(read_sector(&mut bus), [0x55; SECTOR]);
            assert_eq!(read_sector(&mut bus), [0xA5; SECTOR]);
            assert_eq!(batches.borrow().len(), 1);
        }
    }

    #[test]
    fn media_contract_address_failure_submits_committed_prefix() {
        for opcode in [0x30, 0xC5] {
            let (mut bus, batches) = recording_bus(false);
            put(&mut bus, 2, 2);
            command(&mut bus, 0xC6);
            lba(&mut bus, 62, 3, false);
            command(&mut bus, opcode);
            write_sector(&mut bus, &[0x62; SECTOR]);
            assert!(batches.borrow().is_empty());
            write_sector(&mut bus, &[0x63; SECTOR]);
            assert_eq!(status(&mut bus), READY | ERR);
            assert_eq!(get(&mut bus, 1), IDNF);
            assert_eq!(
                *batches.borrow(),
                vec![vec![
                    SectorWrite {
                        lba: 62,
                        bytes: [0x62; SECTOR]
                    },
                    SectorWrite {
                        lba: 63,
                        bytes: [0x63; SECTOR]
                    },
                ]]
            );
        }
    }

    #[test]
    fn media_contract_readonly_aborts_every_write_without_batch() {
        for opcode in [0x30, 0x31, 0xC5] {
            let (mut bus, batches) = recording_bus(true);
            put(&mut bus, 2, 2);
            command(&mut bus, 0xC6);
            lba(&mut bus, 2, 3, false);
            command(&mut bus, opcode);
            assert!(bus.device().irq_pending());
            assert_eq!(status(&mut bus), READY | ERR);
            assert_eq!(get(&mut bus, 1), ABRT);
            write_sector(&mut bus, &[0x55; SECTOR]);
            assert!(batches.borrow().is_empty());
            lba(&mut bus, 2, 1, false);
            command(&mut bus, 0x20);
            assert_eq!(read_sector(&mut bus), [0xA5; SECTOR]);
            assert!(batches.borrow().is_empty());
        }
    }
    #[test]
    fn media_contract_incomplete_sector_aborts_without_batch() {
        let (mut bus, batches) = recording_bus(false);
        lba(&mut bus, 2, 1, false);
        command(&mut bus, 0x30);
        for _ in 0..255 {
            bus.port_write(0x1F0, 2, 0x1234).unwrap();
        }
        command(&mut bus, 0xFF);
        assert_eq!(status(&mut bus), READY | ERR);
        assert!(batches.borrow().is_empty());
        lba(&mut bus, 2, 1, false);
        command(&mut bus, 0x20);
        assert_eq!(read_sector(&mut bus), [0xA5; SECTOR]);
    }
    #[test]
    fn media_contract_restore_rejects_active_write_on_readonly_media() {
        let (mut writable, _) = recording_bus(false);
        lba(&mut writable, 2, 1, false);
        command(&mut writable, 0x30);
        let snapshot = writable.device().save();
        let (mut protected, batches) = recording_bus(true);
        let before = protected.device().save();
        assert!(matches!(
            protected.device_mut().restore(&snapshot),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(protected.device().save(), before);
        assert!(batches.borrow().is_empty());
    }
    #[test]
    fn media_contract_host_flush_submits_complete_prefix_once() {
        let batches = Batches::default();
        let drive = AtaDrive::with_media(
            GEOMETRY,
            64,
            Box::new(RecordingMedia {
                memory: InMemoryMedia::new(vec![0xA5; 64 * SECTOR], 64, false),
                batches: batches.clone(),
            }),
        )
        .unwrap();
        let mut ata = Ata::with_drives([Some(drive), None]).unwrap();
        for (offset, value) in [(6, 0xE0), (2, 3), (3, 2), (4, 0), (5, 0), (7, 0x30)] {
            ata.port_write(0x1F0 + offset, 1, value).unwrap();
        }
        for _ in 0..256 {
            ata.port_write(0x1F0, 2, 0x5555).unwrap();
        }
        ata.port_write(0x1F0, 2, 0x1234).unwrap();
        ata.port_write(0x1F6, 1, 0xF0).unwrap();
        assert!(batches.borrow().is_empty());
        let before = ata.save();
        ata.flush_pending();
        assert_eq!(
            *batches.borrow(),
            vec![vec![SectorWrite {
                lba: 2,
                bytes: [0x55; SECTOR]
            }]]
        );
        assert_eq!(ata.save(), before);
        ata.flush_pending();
        assert_eq!(batches.borrow().len(), 1);
        assert_eq!(
            ata.drives[0].as_ref().unwrap().read_sector(3),
            [0xA5; SECTOR]
        );
    }

    #[test]
    fn media_contract_active_write_overlay_is_readable_before_submission() {
        let batches = Batches::default();
        let drive = AtaDrive::with_media(
            GEOMETRY,
            64,
            Box::new(RecordingMedia {
                memory: InMemoryMedia::new(vec![0xA5; 64 * SECTOR], 64, false),
                batches: batches.clone(),
            }),
        )
        .unwrap();
        let mut ata = Ata::with_drives([Some(drive), None]).unwrap();
        for (offset, value) in [(6, 0xE0), (2, 3), (3, 2), (4, 0), (5, 0), (7, 0x30)] {
            ata.port_write(0x1F0 + offset, 1, value).unwrap();
        }
        for _ in 0..256 {
            ata.port_write(0x1F0, 2, 0x5555).unwrap();
        }
        ata.port_write(0x1F0, 2, 0x1234).unwrap();
        let before = ata.save();
        let drive = ata.drive(0).unwrap();
        assert_eq!(drive.media.read_sector(2), [0xA5; SECTOR]);
        assert_eq!(drive.read_sector(2), [0x55; SECTOR]);
        assert_eq!(&drive.image()[2 * SECTOR..3 * SECTOR], &[0x55; SECTOR]);
        assert_eq!(drive.read_sector(3), [0xA5; SECTOR]);
        assert_eq!(ata.save(), before);
        assert!(batches.borrow().is_empty());
    }

    #[test]
    fn media_contract_capacity_cannot_exceed_mounted_sectors() {
        let media = InMemoryMedia::new(Vec::new(), 63, false);
        assert_eq!(media.sector_count(), 63);
        let error = match AtaDrive::with_media(GEOMETRY, 64, Box::new(media)) {
            Err(error) => error,
            Ok(_) => panic!("short media must be refused"),
        };
        assert_eq!(
            error,
            AtaConfigError::MediaCapacity {
                capacity: 64,
                sector_count: 63
            }
        );
        assert_eq!(
            error.to_string(),
            "ATA capacity 64 sectors exceeds media capacity 63 sectors"
        );
        assert!(AtaDrive::with_media(
            GEOMETRY,
            64,
            Box::new(InMemoryMedia::new(Vec::new(), 64, false))
        )
        .is_ok());
        assert!(AtaDrive::with_media(
            GEOMETRY,
            64,
            Box::new(InMemoryMedia::new(Vec::new(), 65, false))
        )
        .is_ok());
    }
}
