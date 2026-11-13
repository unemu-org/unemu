// SPDX-License-Identifier: AGPL-3.0-or-later
//! Loadable-image configuration (§4).

use std::path::PathBuf;

/// Where the bytes of an image or drive come from (§4).
///
/// Shared by [`LoadImage`] and [`crate::DriveConfig`] so a blob and a disk are
/// sourced from literal bytes, files, or generated zeroes. System ROMs also
/// accept interleaved chip dumps; their lengths are checked when read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageSource {
    /// The bytes inline in the config.
    Inline(Vec<u8>),
    /// A complete EPROM retained for identification, with only its suffix mapped.
    /// Container layouts come from measured image identities; see
    /// `backlog/docs/design/firmware-and-machine-classes.md` §Video ROM placement.
    RomContainer {
        /// Full file bytes; SHA-256 pins always cover these bytes.
        bytes: Vec<u8>,
        /// First mapped byte in the container.
        offset: usize,
    },
    /// A file read at load time.
    File(PathBuf),
    /// Even/odd EPROM parts assembled before firmware identification.
    Interleaved(Vec<ImagePart>),
    /// A generated run of `size` zero bytes (a blank disk, a cleared region).
    Blank {
        /// Number of zero bytes to generate.
        size: u64,
    },
}

impl ImageSource {
    /// Mapped size when known without I/O, excluding a ROM container prefix.
    /// File and interleaved sizes require I/O; an out-of-bounds prefix has no size.
    pub fn known_len(&self) -> Option<u64> {
        match self {
            ImageSource::Inline(bytes) => Some(bytes.len() as u64),
            ImageSource::RomContainer { bytes, offset } => {
                bytes.len().checked_sub(*offset).map(|len| len as u64)
            }
            ImageSource::Blank { size } => Some(*size),
            ImageSource::File(_) | ImageSource::Interleaved(_) => None,
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
/// Recognized firmware uses its catalogue set; nonempty restatements must match.
/// Unknown firmware under the escape option uses the declared set. See
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

/// Byte lane of a system BIOS EPROM.
/// MAME src/mame/pc/at.cpp, ibm5170/ibm5170a: U27 even and U47 odd;
/// four 27128 dumps in U17/U37-low, U27/U47-high order reproduce the MAME-listed
/// 27256 pair's image (measured; firmware design §3.5).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane {
    /// Bytes at offset + 2*i.
    Even,
    /// Bytes at offset + 2*i + 1.
    Odd,
}
impl std::fmt::Display for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Even => "even",
            Self::Odd => "odd",
        })
    }
}
/// One file-backed EPROM; offset is measured in assembled bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePart {
    /// User-supplied chip dump.
    pub path: PathBuf,
    /// Physical byte lane.
    pub lane: Lane,
    /// Start of this pair in the assembled image.
    pub offset: u64,
}

/// Interleave already-read chip bytes, requiring complete, contiguous pairs.
/// See `backlog/docs/design/firmware-and-machine-classes.md` §3.5.
pub fn interleave_parts(parts: &[(Lane, u64, Vec<u8>)]) -> Result<Vec<u8>, crate::ConfigError> {
    use crate::ConfigError::FirmwareParts;
    let error = |message: String| FirmwareParts { message };
    if !parts.iter().any(|p| p.0 == Lane::Even) || !parts.iter().any(|p| p.0 == Lane::Odd) {
        return Err(error(
            "parts require at least one even and one odd part".into(),
        ));
    }
    let mut pairs = std::collections::BTreeMap::<u64, [Option<&[u8]>; 2]>::new();
    for (lane, offset, bytes) in parts {
        let slot = &mut pairs.entry(*offset).or_default()[usize::from(*lane == Lane::Odd)];
        if slot.replace(bytes).is_some() {
            return Err(error(format!("duplicate {lane} part at offset {offset}")));
        }
    }
    let mut total = 0u64;
    for (offset, pair) in &pairs {
        let [Some(even), Some(odd)] = pair else {
            return Err(error(format!(
                "offset {offset} requires exactly one even and one odd part"
            )));
        };
        if even.len() != odd.len() {
            return Err(error(format!(
                "parts at offset {offset} have unequal sizes: even {}, odd {}",
                even.len(),
                odd.len()
            )));
        }
        if even.is_empty() {
            return Err(error(format!("parts at offset {offset} must be nonempty")));
        }
        if *offset != total {
            return Err(error(format!(
                "parts {} at offset {offset}; expected offset {total}",
                if *offset < total {
                    "overlap"
                } else {
                    "leave a gap"
                }
            )));
        }
        total = total
            .checked_add(
                (even.len() as u64)
                    .checked_mul(2)
                    .ok_or_else(|| error("assembled size overflows".into()))?,
            )
            .ok_or_else(|| error("assembled size overflows".into()))?;
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(
            usize::try_from(total).map_err(|_| error("assembled image is too large".into()))?,
        )
        .map_err(|_| error("assembled image is too large".into()))?;
    for pair in pairs.values() {
        if let [Some(even), Some(odd)] = pair {
            for (a, b) in even.iter().zip(*odd) {
                bytes.extend([*a, *b]);
            }
        }
    }
    Ok(bytes)
}
