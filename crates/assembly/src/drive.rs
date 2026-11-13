// SPDX-License-Identifier: AGPL-3.0-or-later
//! Drive configuration (§7.7).

use std::path::PathBuf;

use crate::device::DeviceId;
use crate::image::ImageSource;

/// The cylinders/heads/sectors geometry of a floppy disk (§7.7).
///
/// The image size an FDC expects is fixed by the geometry
/// ([`FloppyGeometry::image_bytes`]); a mismatched backing image is a
/// [`crate::ConfigError`]. The four standard PC formats are provided as
/// constructors. (The FDC device that *reads* this geometry is a separate task;
/// here it is only the config type.)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FloppyGeometry {
    /// Cylinders (tracks per side).
    pub cylinders: u32,
    /// Heads (sides).
    pub heads: u32,
    /// Sectors per track.
    pub sectors_per_track: u32,
    /// Bytes per sector. Validation accepts 512 only, matching the current FDC.
    pub bytes_per_sector: u32,
}

impl FloppyGeometry {
    /// 1.44 MB high-density 3.5": 80 × 2 × 18 × 512.
    pub const HD_1_44: Self = Self::chs(80, 2, 18);
    /// 1.2 MB high-density 5.25": 80 × 2 × 15 × 512.
    pub const HD_1_2: Self = Self::chs(80, 2, 15);
    /// 720 KB double-density 3.5": 80 × 2 × 9 × 512.
    pub const DD_720K: Self = Self::chs(80, 2, 9);
    /// 360 KB double-density 5.25": 40 × 2 × 9 × 512.
    pub const DD_360K: Self = Self::chs(40, 2, 9);

    /// A geometry with the standard 512-byte sector.
    const fn chs(cylinders: u32, heads: u32, sectors_per_track: u32) -> Self {
        Self {
            cylinders,
            heads,
            sectors_per_track,
            bytes_per_sector: 512,
        }
    }

    /// The exact backing-image size this geometry implies, in bytes.
    pub fn image_bytes(&self) -> u64 {
        u64::from(self.cylinders)
            * u64::from(self.heads)
            * u64::from(self.sectors_per_track)
            * u64::from(self.bytes_per_sector)
    }
}

/// The cylinders/heads/sectors geometry of a fixed disk (§7.7).
///
/// The same CHS shape as a floppy but without fixed presets — a hard disk's
/// geometry is per-image. [`HardDiskGeometry::image_bytes`] gives the size a
/// matching image must have.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HardDiskGeometry {
    /// Cylinders.
    pub cylinders: u32,
    /// Heads.
    pub heads: u32,
    /// Sectors per track.
    pub sectors_per_track: u32,
    /// Bytes per sector.
    pub bytes_per_sector: u32,
}

impl HardDiskGeometry {
    /// The exact backing-image size this geometry implies, in bytes.
    pub fn image_bytes(&self) -> u64 {
        u64::from(self.cylinders)
            * u64::from(self.heads)
            * u64::from(self.sectors_per_track)
            * u64::from(self.bytes_per_sector)
    }
}

/// Whether a drive is a floppy or a fixed disk, with its geometry (§7.7).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DriveKind {
    /// A removable floppy disk.
    Floppy {
        /// The floppy's geometry.
        geometry: FloppyGeometry,
    },
    /// A fixed (hard) disk.
    ///
    /// Binds to an ATA channel's master (unit 0) or slave (unit 1).
    /// An FDC cannot serve this kind.
    HardDisk {
        /// The disk's geometry.
        geometry: HardDiskGeometry,
    },
}

impl DriveKind {
    /// The backing-image size this drive's geometry implies, in bytes.
    pub fn image_bytes(&self) -> u64 {
        match self {
            DriveKind::Floppy { geometry } => geometry.image_bytes(),
            DriveKind::HardDisk { geometry } => geometry.image_bytes(),
        }
    }
}

/// One drive attached to a disk controller (§7.7).
///
/// Drives live once, at the top level of a [`crate::MachineConfig`], and bind to
/// a named `controller` and its `unit` — there is no second drive list inside the
/// controller's own config. The backing `image` must match the geometry's size
/// when that size is known ([`ImageSource::known_len`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveConfig {
    /// Optional removable-media catalogue; replaces the legacy image/write fields.
    pub media: Option<FloppyMediaList>,
    /// Floppy or hard disk, with geometry.
    pub kind: DriveKind,
    /// Where the drive's contents come from.
    pub image: ImageSource,
    /// Expected SHA-256 of a file source, checked on the bytes mounted by assembly.
    /// Inline and blank sources do not require a second integrity check.
    pub expected_sha256: Option<[u8; 32]>,
    /// The stable role of the controller serving this medium.
    pub controller: DeviceId,
    /// The drive-select unit on that controller (0 or 1 on the standard FDC).
    pub unit: u8,
    /// Whether guest writes are volatile, protected or journaled.
    pub write: WriteMode,
}

/// Guest write policy for mounted media (see `backlog/docs/design/machine-config.md`
/// §Drive binding and write mode). The original host image is always immutable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WriteMode {
    /// Guest writes update only the in-memory image, the existing default.
    #[default]
    Volatile,
    /// Guest writes report the controller's existing not-writable status and
    /// leave the mounted image unchanged.
    ReadOnly,
    /// Persist complete commands in an explicitly selected journal.
    Journal {
        /// Journal output, relative to the process working directory.
        path: PathBuf,
        /// Positive closed-generation retention count, reserved for retention.
        keep_generations: u32,
    },
}

impl WriteMode {
    /// Shared default number of closed generations retained by media journals.
    pub const DEFAULT_KEEP_GENERATIONS: u32 = 10;

    /// Select a journal with the default ten closed generations.
    pub fn journal(path: impl Into<PathBuf>) -> Self {
        Self::Journal {
            path: path.into(),
            keep_generations: Self::DEFAULT_KEEP_GENERATIONS,
        }
    }
}

/// Ordered floppy media and the power-on door state (machine-files.md § Drives).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloppyMediaList {
    /// Each image must match the drive geometry; mixed-density mechanics are absent.
    pub entries: Vec<FloppyMedium>,
    /// Start without a disk. The change line still starts clear.
    pub empty: bool,
}

/// One immutable base and its independent overlay policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloppyMedium {
    /// Source of the immutable base bytes.
    pub image: ImageSource,
    /// Optional expected digest, verified on the bytes actually mounted.
    pub expected_sha256: Option<[u8; 32]>,
    /// Guest write policy for this medium.
    pub write: WriteMode,
    /// Display name, normally the source file name (never a host path).
    pub name: String,
}

impl DriveConfig {
    pub(crate) fn with_medium(&self, medium: &FloppyMedium) -> Self {
        Self {
            media: None,
            image: medium.image.clone(),
            expected_sha256: medium.expected_sha256,
            write: medium.write.clone(),
            ..self.clone()
        }
    }
}
