// SPDX-License-Identifier: AGPL-3.0-or-later
//! Loadable-image configuration (§4).

use std::path::PathBuf;

/// Where the bytes of an image or drive come from (§4).
///
/// Shared by [`LoadImage`] and [`crate::DriveConfig`] so a blob and a disk are
/// sourced the same three ways: literal bytes, a file read at load time, or a
/// generated run of zeroes of a given size. Only the first and last have a size
/// known at validation time; a [`ImageSource::File`] is checked when it is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageSource {
    /// The bytes inline in the config.
    Inline(Vec<u8>),
    /// A file read at load time.
    File(PathBuf),
    /// A generated run of `size` zero bytes (a blank disk, a cleared region).
    Blank {
        /// Number of zero bytes to generate.
        size: u64,
    },
}

impl ImageSource {
    /// The image's size in bytes if it is known without I/O ([`ImageSource::Inline`]
    /// or [`ImageSource::Blank`]); `None` for a [`ImageSource::File`], whose size
    /// is only known when it is read.
    pub fn known_len(&self) -> Option<u64> {
        match self {
            ImageSource::Inline(bytes) => Some(bytes.len() as u64),
            ImageSource::Blank { size } => Some(*size),
            ImageSource::File(_) => None,
        }
    }
}

/// How a [`LoadImage`] is mapped into the address space (§4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// A masked ROM: executable and readable, guest stores dropped.
    Rom,
    /// A blob loaded into existing RAM (the guest may overwrite it).
    RamLoad,
}

/// Guest firmware services declared by ROM images.
///
/// The initial vocabulary is `floppy` and `fixed-disk`, both INT 13h services.
/// These are declarations, not firmware identification; see
/// `backlog/docs/design/machine-config.md` § Firmware services.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FirmwareService {
    /// INT 13h diskette services.
    Floppy,
    /// INT 13h fixed-disk services.
    FixedDisk,
}

impl std::fmt::Display for FirmwareService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Floppy => "floppy",
            Self::FixedDisk => "fixed-disk",
        })
    }
}

/// One blob placed into the address space (§4).
///
/// The **single home** for anything loaded at a fixed address — a masked BIOS
/// ROM, an option ROM, a RAM-loaded boot blob. There is deliberately no separate
/// "firmware" / "cpu blob" / per-region ROM path: everything loaded is a
/// `LoadImage`, distinguished only by its [`ImageKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadImage {
    /// Guest-physical base address the image is placed at.
    pub base: u64,
    /// Where the image's bytes come from.
    pub source: ImageSource,
    /// Whether the image is a masked ROM or a RAM load.
    pub kind: ImageKind,
    /// Guest services supplied by this ROM; RAM loads cannot provide services.
    pub provides: Vec<FirmwareService>,
}
