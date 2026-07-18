// SPDX-License-Identifier: AGPL-3.0-or-later
//! Drive configuration (§7.7).

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
    /// Bytes per sector.
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
    /// This variant is the **attachment seam for a future XT-IDE/ATA device**. A
    /// `HardDisk` drive is first-class config today — it is validated, it derives
    /// its CMOS `hard_disk_types` nibble, and it binds to a controller by
    /// `controller_slot` — but the assembler skips it during device construction
    /// because no fixed-disk controller is modelled yet. When one lands it plugs
    /// into the same routing the floppy uses (see `build_fdc_drives` in the
    /// assembler): filter `drives` for `HardDisk` and route by `controller_slot`,
    /// no new config shape required.
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
/// a controller by `controller_slot` — there is no second drive list inside the
/// controller's own config. The backing `image` must match the geometry's size
/// when that size is known ([`ImageSource::known_len`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveConfig {
    /// Floppy or hard disk, with geometry.
    pub kind: DriveKind,
    /// Where the drive's contents come from.
    pub image: ImageSource,
    /// The controller drive slot this drive occupies.
    pub controller_slot: u8,
    /// Whether the mounted disk is write-protected. Set means a guest WRITE to
    /// the disk faults (not-writable) and never mutates the image; the default
    /// (`false`) is a writable disk. The flag lives on the drive because it is a
    /// property of the mounted medium, not of the controller.
    pub write_protect: bool,
}
