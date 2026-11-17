// SPDX-License-Identifier: AGPL-3.0-or-later
//! FAT12 diskette volumes: the four PC/AT diskette formats, one deterministic
//! formatter, and a builder that fills a fresh volume from host files and
//! folders, on one disk or split across several.
//!
//! On-disk structures follow the Microsoft FAT Specification: the boot sector
//! and BIOS Parameter Block (Boot Sector and BPB), 12-bit cluster entries in
//! two identical FAT copies (FAT Data Structure), the fixed root directory and
//! subdirectory clusters that start with `.` and `..` entries (FAT Directory
//! Structure). Short names follow its basis-name and numeric-tail generation
//! with the differences listed in [`ShortName`]. No long-name (VFAT) entries
//! are written: the volumes are plain DOS. Geometry and media descriptors are
//! the IBM Personal Computer AT Technical Reference diskette formats (360K,
//! 1.2M, 720K, 1.44M) with the BPB values DOS FORMAT writes for them.
//! See machines/README.md § Preparing disks.
//!
//! Every byte depends only on the request and on the host files' names and
//! contents. Timestamps are the FAT epoch (1980-01-01 00:00:00), the volume
//! serial is fixed, entries are ordered by the bytes of their host names, and
//! clusters are allocated contiguously in that order. Images are built in
//! memory; [`crate::create::image`] publishes them. Nothing is written for a
//! request that does not fit.

mod layout;
mod names;
#[cfg(any(test, feature = "test-support"))]
pub mod reader;
mod split;
mod tree;

#[cfg(test)]
mod tests;

pub use names::ShortName;

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Bytes per sector on every PC/AT diskette format.
const SECTOR: usize = 512;
/// Bytes per directory entry (FAT Directory Structure).
const ENTRY: usize = 32;
/// Fixed volume serial so identical requests give identical images.
const SERIAL: u32 = 0x2895_9855;
/// FAT date of 1980-01-01, the earliest a directory entry can encode.
const FAT_EPOCH_DATE: u16 = 0x21;

/// One of the four PC/AT diskette formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloppyFormat {
    /// 3.5-inch high density: 80 cylinders, 2 heads, 18 sectors per track.
    Hd144,
    /// 5.25-inch high density: 80 cylinders, 2 heads, 15 sectors per track.
    Hd12,
    /// 3.5-inch double density: 80 cylinders, 2 heads, 9 sectors per track.
    Dd720,
    /// 5.25-inch double density: 40 cylinders, 2 heads, 9 sectors per track.
    Dd360,
}

/// BPB values per format: the geometry and the layout DOS FORMAT uses.
struct Layout {
    cylinders: u16,
    sectors_per_track: u16,
    cluster_sectors: u8,
    root_entries: u16,
    media: u8,
    fat_sectors: u16,
}

impl FloppyFormat {
    /// Every format, largest first.
    pub const ALL: [Self; 4] = [Self::Hd144, Self::Hd12, Self::Dd720, Self::Dd360];

    /// The name the command line and machine files use: `1.44M`, `1.2M`,
    /// `720K` or `360K`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hd144 => "1.44M",
            Self::Hd12 => "1.2M",
            Self::Dd720 => "720K",
            Self::Dd360 => "360K",
        }
    }

    /// The format with this [`name`](Self::name).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.name() == name)
    }

    const fn layout(self) -> Layout {
        let (cylinders, sectors_per_track, cluster_sectors, root_entries, media, fat_sectors) =
            match self {
                Self::Hd144 => (80, 18, 1, 224, 0xF0, 9),
                Self::Hd12 => (80, 15, 1, 224, 0xF9, 7),
                Self::Dd720 => (80, 9, 2, 112, 0xF9, 3),
                Self::Dd360 => (40, 9, 2, 112, 0xFD, 2),
            };
        Layout {
            cylinders,
            sectors_per_track,
            cluster_sectors,
            root_entries,
            media,
            fat_sectors,
        }
    }

    /// Sectors on the whole disk (two heads).
    pub const fn total_sectors(self) -> u32 {
        let layout = self.layout();
        layout.cylinders as u32 * 2 * layout.sectors_per_track as u32
    }

    /// Length of a raw image of this format.
    pub const fn image_bytes(self) -> u64 {
        self.total_sectors() as u64 * SECTOR as u64
    }

    /// Bytes per allocation unit.
    pub const fn cluster_bytes(self) -> u64 {
        self.layout().cluster_sectors as u64 * SECTOR as u64
    }

    /// Entries the fixed root directory holds, a volume label included.
    pub const fn root_entries(self) -> usize {
        self.layout().root_entries as usize
    }

    /// Allocation units in the data area.
    pub const fn data_clusters(self) -> u64 {
        let layout = self.layout();
        let data_start =
            1 + 2 * layout.fat_sectors as u32 + layout.root_entries as u32 * 32 / SECTOR as u32;
        ((self.total_sectors() - data_start) / layout.cluster_sectors as u32) as u64
    }

    /// Bytes of the data area: what DOS reports free on an empty disk.
    pub const fn data_bytes(self) -> u64 {
        self.data_clusters() * self.cluster_bytes()
    }

    fn fat_start(self) -> usize {
        SECTOR
    }

    fn fat_bytes(self) -> usize {
        usize::from(self.layout().fat_sectors) * SECTOR
    }

    fn root_start(self) -> usize {
        self.fat_start() + 2 * self.fat_bytes()
    }

    fn data_start(self) -> usize {
        self.root_start() + self.root_entries() * ENTRY
    }
}

impl fmt::Display for FloppyFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Parse a volume label: 1–11 ASCII characters, upper-cased and space padded,
/// without leading spaces or FAT filename punctuation.
pub fn volume_label(value: &str) -> Result<[u8; 11], &'static str> {
    if value.len() > 11
        || value.starts_with(' ')
        || value.trim().is_empty()
        || !value.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        || value.bytes().any(|b| b"\"*+,./:;<=>?[\\]|".contains(&b))
    {
        return Err("label must contain 1..=11 ASCII characters, without leading spaces or FAT filename punctuation");
    }
    let mut label = [b' '; 11];
    for (out, byte) in label.iter_mut().zip(value.bytes()) {
        *out = byte.to_ascii_uppercase();
    }
    Ok(label)
}

/// Format metadata of an empty volume: boot sector, both FATs and the root
/// directory. The data area that follows is all zero, so a caller may publish
/// this as the prefix of a zero-filled image.
///
/// The boot sector disables interrupts and halts in a loop: a data disk booted
/// by mistake stops instead of executing filesystem metadata. With a label the
/// root holds its volume-label entry (attribute 08h, FAT epoch dates).
pub fn format_metadata(format: FloppyFormat, label: Option<[u8; 11]>) -> Vec<u8> {
    let layout = format.layout();
    let root_start = format.root_start();
    let mut disk = vec![0; format.data_start()];
    disk[..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    disk[3..11].copy_from_slice(b"UNEMUPC ");
    for (offset, value) in [
        (11, SECTOR as u16),
        (14, 1),
        (17, layout.root_entries),
        (19, format.total_sectors() as u16),
        (22, layout.fat_sectors),
        (24, layout.sectors_per_track),
        (26, 2),
    ] {
        disk[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    disk[13] = layout.cluster_sectors;
    disk[16] = 2;
    disk[21] = layout.media;
    disk[38] = 0x29;
    disk[39..43].copy_from_slice(&SERIAL.to_le_bytes());
    disk[43..54].copy_from_slice(&label.unwrap_or(*b"NO NAME    "));
    disk[54..62].copy_from_slice(b"FAT12   ");
    disk[62..66].copy_from_slice(&[0xFA, 0xF4, 0xEB, 0xFD]);
    disk[510..512].copy_from_slice(&[0x55, 0xAA]);
    for start in [format.fat_start(), format.fat_start() + format.fat_bytes()] {
        disk[start..start + 3].copy_from_slice(&[layout.media, 0xFF, 0xFF]);
    }
    if let Some(label) = label {
        disk[root_start..root_start + 11].copy_from_slice(&label);
        disk[root_start + 11] = 8;
        for offset in [16, 18, 24] {
            disk[root_start + offset..root_start + offset + 2]
                .copy_from_slice(&FAT_EPOCH_DATE.to_le_bytes());
        }
    }
    disk
}

/// What to put on the disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Host files and folders. A folder becomes a subdirectory with its whole
    /// tree; the order given does not matter. A named source is never skipped:
    /// one that is a symbolic link, host metadata, a device or one of
    /// `exclude`, or that names the same host entry as another source, is
    /// refused.
    pub sources: Vec<PathBuf>,
    /// One folder at the root that every source goes under.
    pub top: Option<ShortName>,
    /// The diskette format.
    pub format: FloppyFormat,
    /// Volume label; split sets number it per disk.
    pub label: Option<[u8; 11]>,
    /// Host paths never copied: the images being written, when they sit
    /// inside a source tree. Paths that do not exist are ignored; entries met
    /// in a folder are reported as skipped.
    pub exclude: Vec<PathBuf>,
}

/// A host file placed on a disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedFile {
    /// Where it came from.
    pub host: PathBuf,
    /// Its path from the disk's root, without a drive, e.g. `GAME\DATA\LEVEL1.DAT`.
    pub dos_path: String,
    /// Its length.
    pub bytes: u64,
}

/// A host name that is not a valid 8.3 name as it stands. Changes of letter
/// case alone are not renames: DOS matches names regardless of case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    /// The host file or folder.
    pub host: PathBuf,
    /// Its path on the disk, the new name last.
    pub dos_path: String,
}

/// Why a host entry was left out of a folder, or refused as a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Desktop metadata (`.DS_Store`, `._*` AppleDouble files, `__MACOSX`
    /// archive folders, `Thumbs.db`, `desktop.ini`), not part of the program.
    HostJunk,
    /// Symbolic links are never followed.
    SymbolicLink,
    /// A device, socket or pipe.
    NotRegular,
    /// One of [`Request::exclude`]: an image being written.
    Output,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::HostJunk => "host metadata, not part of the program",
            Self::SymbolicLink => "symbolic link, not followed",
            Self::NotRegular => "not a regular file or folder",
            Self::Output => "the image being written",
        })
    }
}

/// What to say about a source the user named but that cannot be copied.
fn refusal(reason: &SkipReason) -> &'static str {
    match reason {
        SkipReason::HostJunk => "is host metadata, not part of a program; leave it out",
        SkipReason::SymbolicLink => "is a symbolic link; name its target",
        SkipReason::NotRegular => "is not a regular file or folder",
        SkipReason::Output => "is the image being written",
    }
}

/// A host entry that was left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The host path.
    pub host: PathBuf,
    /// Why.
    pub reason: SkipReason,
}

/// One planned disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskPlan {
    /// Volume label written to the BPB and the root.
    pub label: Option<[u8; 11]>,
    /// Files on this disk, in tree order.
    pub files: Vec<PlacedFile>,
    /// Data-area bytes in use, cluster rounding and folders included.
    pub used_bytes: u64,
    /// Data-area bytes left, as DOS DIR reports them.
    pub free_bytes: u64,
}

/// A checked plan: names, skips and the contents of every disk. Images are
/// only built from a plan, so nothing is produced for a request that does not
/// fit.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The diskette format of every disk.
    pub format: FloppyFormat,
    /// The disks, one for [`plan`], one or more for [`plan_split`].
    pub disks: Vec<DiskPlan>,
    /// Every host name that changed, in tree order.
    pub renames: Vec<Rename>,
    /// Every host entry left out, in tree order.
    pub skipped: Vec<Skipped>,
    tree: tree::Tree,
    members: Vec<Vec<bool>>,
}

impl Plan {
    /// Build the image of disk `index`: [`FloppyFormat::image_bytes`] long.
    /// Host files are read now; one that changed length since planning is an
    /// error rather than a truncated or padded copy.
    ///
    /// # Panics
    ///
    /// If `index` is not below `disks.len()`.
    pub fn image(&self, index: usize) -> Result<Vec<u8>, BuildError> {
        layout::image(
            self.format,
            &self.tree,
            &self.members[index],
            self.disks[index].label,
        )
    }

    /// Build every disk's image, in order.
    pub fn images(&self) -> Result<Vec<Vec<u8>>, BuildError> {
        (0..self.disks.len())
            .map(|index| self.image(index))
            .collect()
    }
}

/// Why a request cannot become a disk.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// A host path could not be read.
    #[error("{}: {source}", path.display())]
    Io {
        /// The host path.
        path: PathBuf,
        /// The host error.
        source: io::Error,
    },
    /// The request names no source.
    #[error("nothing to put on the disk: name at least one file or folder")]
    NothingToAdd,
    /// A named source cannot be copied; it is refused rather than skipped.
    #[error("{} {}", path.display(), refusal(reason))]
    Refused {
        /// The source as named.
        path: PathBuf,
        /// What it is.
        reason: SkipReason,
    },
    /// Two sources name the same host file or folder, which would put it on
    /// the disk twice.
    #[error("{} and {} are the same host file or folder; name it once", first.display(), second.display())]
    SameSource {
        /// The source named first.
        first: PathBuf,
        /// The source naming it again.
        second: PathBuf,
    },
    /// A folder's path on the disk is longer than DOS can hold as a current
    /// directory.
    #[error("{}: its DOS path {dos_path} has {length} characters; DOS allows at most 64 for a folder, counting the leading backslash", host.display())]
    PathTooLong {
        /// The host folder.
        host: PathBuf,
        /// Its path on the disk, with the leading backslash.
        dos_path: String,
        /// Characters in `dos_path`.
        length: usize,
    },
    /// A source path has no final name to put on the disk, such as `..`.
    #[error("{}: name a file or folder; the path has no final name", path.display())]
    Unnamed {
        /// The source path.
        path: PathBuf,
    },
    /// A host file changed length, or stopped being a regular file, between
    /// planning and building.
    #[error("{}: changed while the disk was being built", path.display())]
    Changed {
        /// The host file.
        path: PathBuf,
    },
    /// So many names in one folder share a basis name that no numeric tail
    /// fits in eight characters.
    #[error("{}: too many names in its folder shorten to the same 8.3 name", path.display())]
    NoShortName {
        /// The host entry left without a name.
        path: PathBuf,
    },
    /// One file cannot fit even on an empty disk.
    #[error("{} ({bytes} bytes) does not fit on an empty {format} disk: {room} bytes are free for it", host.display())]
    FileTooLarge {
        /// The host file.
        host: PathBuf,
        /// Its path on the disk.
        dos_path: String,
        /// Its length.
        bytes: u64,
        /// Data-area bytes left for it on an empty disk after its folders.
        room: u64,
        /// The diskette format.
        format: FloppyFormat,
    },
    /// The files do not fit on one disk.
    #[error("the files need {needed} bytes of a {format} disk's {capacity}: {shortfall} bytes short (cluster rounding and folders included)")]
    DoesNotFit {
        /// The diskette format.
        format: FloppyFormat,
        /// Data-area bytes needed.
        needed: u64,
        /// Data-area bytes of the format.
        capacity: u64,
        /// `needed - capacity`.
        shortfall: u64,
    },
    /// The fixed root directory has too few entries.
    #[error("the root folder of a {format} disk holds {capacity} entries but {needed} are needed; put the files under a top folder")]
    RootFull {
        /// The diskette format.
        format: FloppyFormat,
        /// Root entries needed, a volume label included.
        needed: usize,
        /// Root entries of the format.
        capacity: usize,
    },
}

/// Plan one disk holding every source. Fails with the shortfall when it does
/// not fit, or names a file that cannot fit even alone.
pub fn plan(request: &Request) -> Result<Plan, BuildError> {
    split::single(request.format, request.label, tree::scan(request)?)
}

/// Plan as many disks of one format as the sources need. Items go greedily in
/// tree order; every disk repeats the folders above its files, so copying each
/// disk's tree onto one drive in turn rebuilds the whole tree. With a label and
/// more than one disk, disk `k` is labelled `<LABEL> k`, the label shortened to
/// fit eleven characters. A file that cannot fit on an empty disk is refused,
/// never split.
pub fn plan_split(request: &Request) -> Result<Plan, BuildError> {
    split::greedy(request.format, request.label, tree::scan(request)?)
}

/// Build one disk holding every source: [`plan`] then [`Plan::image`].
pub fn build(request: &Request) -> Result<(Vec<u8>, Plan), BuildError> {
    let plan = plan(request)?;
    Ok((plan.image(0)?, plan))
}

/// Build every disk of a split set: [`plan_split`] then [`Plan::images`].
pub fn build_split(request: &Request) -> Result<(Vec<Vec<u8>>, Plan), BuildError> {
    let plan = plan_split(request)?;
    Ok((plan.images()?, plan))
}
