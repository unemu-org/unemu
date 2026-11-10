// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-independent sector media for storage controllers.
//!
//! Sectors are 512 bytes, as used by the PC floppy formats (NEC µPD765
//! datasheet, WRITE DATA, N=2). The contract groups replacements at command
//! completion; see `backlog/docs/design/persistent-media.md`, §Integration.
//! It has no clock or host error channel: host failures must be latched outside
//! guest-visible controller state.

/// Bytes in one complete replacement sector.
pub const SECTOR_SIZE: usize = 512;

/// One complete sector replacement in a command's transfer order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectorWrite {
    /// Zero-based logical sector address.
    pub lba: u64,
    /// Final bytes, including any padding supplied by the controller.
    pub bytes: [u8; SECTOR_SIZE],
}

/// Hardware-neutral mounted media covering the configured drive geometry.
///
/// Controllers translate sector IDs to LBAs and reject write-protected commands
/// before submission (NEC µPD765 datasheet, WRITE DATA and Status Register 1,
/// NW). Each nonempty completed write submits exactly one ordered batch,
/// including replacements committed before an abnormal result. Implementations
/// must apply all replacements to readable memory before returning, and must
/// never expose host failures through reads, write protection or completion.
/// See `backlog/docs/design/persistent-media.md`, §Integration.
pub trait SectorMedia {
    /// Read a complete sector at an LBA within the mounted drive geometry.
    fn read_sector(&self, lba: u64) -> [u8; SECTOR_SIZE];

    /// Apply one nonempty completed command, with no guest-visible error result.
    fn write_batch(&mut self, sectors: &[SectorWrite]);

    /// Whether the medium rejects guest writes; constant while mounted.
    fn write_protected(&self) -> bool;
}

/// Volatile raw image with zero reads and growth beyond a short image's end.
///
/// No host I/O occurs. Controllers enforce the immutable write-protection
/// property before submitting a batch. The caller supplies the mounted drive's
/// geometry-derived sector count independently of the initial image length, so
/// short images can grow within that capacity. Reads beyond stored bytes return
/// zeros. Batches containing any LBA outside the capacity or whose byte range
/// cannot be represented are ignored in full. This bounds growth without
/// exposing an error to the controller.
#[derive(Clone, Debug)]
pub struct InMemoryMedia {
    image: Vec<u8>,
    sector_count: u64,
    write_protected: bool,
}

impl InMemoryMedia {
    fn sector_range(&self, lba: u64) -> Option<std::ops::Range<usize>> {
        if lba >= self.sector_count {
            return None;
        }
        let offset = usize::try_from(lba).ok()?.checked_mul(SECTOR_SIZE)?;
        let end = offset.checked_add(SECTOR_SIZE)?;
        Some(offset..end)
    }

    /// Mount raw bytes with the drive geometry's sector count, optionally protected.
    /// The image may be shorter than the capacity; writes grow it only as needed.
    #[must_use]
    pub fn new(image: Vec<u8>, sector_count: u64, write_protected: bool) -> Self {
        Self {
            image,
            sector_count,
            write_protected,
        }
    }
}

impl SectorMedia for InMemoryMedia {
    fn read_sector(&self, lba: u64) -> [u8; SECTOR_SIZE] {
        let Some(range) = self.sector_range(lba) else {
            return [0; SECTOR_SIZE];
        };
        let mut bytes = [0; SECTOR_SIZE];
        for (byte, index) in bytes.iter_mut().zip(range) {
            *byte = self.image.get(index).copied().unwrap_or(0);
        }
        bytes
    }

    fn write_batch(&mut self, sectors: &[SectorWrite]) {
        debug_assert!(!self.write_protected, "write batch on protected media");
        let Some(replacements) = sectors
            .iter()
            .map(|sector| self.sector_range(sector.lba).map(|range| (sector, range)))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        for (sector, range) in replacements {
            if self.image.len() < range.end {
                self.image.resize(range.end, 0);
            }
            self.image[range].copy_from_slice(&sector.bytes);
        }
    }

    fn write_protected(&self) -> bool {
        self.write_protected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_geometry_accepts_lba_5759_and_rejects_lba_5760() {
        let mut media = InMemoryMedia::new(Vec::new(), 80 * 2 * 36, false);
        media.write_batch(&[SectorWrite {
            lba: 5759,
            bytes: [0xBB; SECTOR_SIZE],
        }]);
        assert_eq!(media.image.len(), 5760 * SECTOR_SIZE);
        assert_eq!(media.read_sector(5759), [0xBB; SECTOR_SIZE]);
        let original = media.image.clone();
        media.write_batch(&[
            SectorWrite {
                lba: 0,
                bytes: [0xCC; SECTOR_SIZE],
            },
            SectorWrite {
                lba: 5760,
                bytes: [0xDD; SECTOR_SIZE],
            },
        ]);
        assert_eq!(media.image, original);
        assert_eq!(media.read_sector(5760), [0; SECTOR_SIZE]);
    }

    #[test]
    fn hd1440_rejects_lba_2880_while_larger_geometry_accepts_it() {
        let mut small = InMemoryMedia::new(Vec::new(), 80 * 2 * 18, false);
        let mut large = InMemoryMedia::new(Vec::new(), 80 * 2 * 36, false);
        let batch = [SectorWrite {
            lba: 2880,
            bytes: [0xBB; SECTOR_SIZE],
        }];
        small.write_batch(&batch);
        large.write_batch(&batch);
        assert!(small.image.is_empty());
        assert_eq!(small.read_sector(2880), [0; SECTOR_SIZE]);
        assert_eq!(large.read_sector(2880), [0xBB; SECTOR_SIZE]);
    }

    #[test]
    fn out_of_range_lbas_reject_entire_batch_without_growth_or_aliasing() {
        for (sector_count, lba) in [
            (7, 7),
            (u64::MAX, u64::MAX / SECTOR_SIZE as u64 + 1),
            (u64::MAX, u64::MAX / SECTOR_SIZE as u64),
            (u64::MAX, u64::MAX),
        ] {
            let original = vec![0xAA; SECTOR_SIZE];
            let mut media = InMemoryMedia::new(original.clone(), sector_count, false);
            assert_eq!(media.read_sector(lba), [0; SECTOR_SIZE]);
            media.write_batch(&[
                SectorWrite {
                    lba: 0,
                    bytes: [0xCC; SECTOR_SIZE],
                },
                SectorWrite {
                    lba,
                    bytes: [0xBB; SECTOR_SIZE],
                },
            ]);
            assert_eq!(
                media.image, original,
                "invalid LBA {lba} rejects the whole command"
            );
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "write batch on protected media")]
    fn protected_media_rejects_direct_batch_in_debug_builds() {
        let mut media = InMemoryMedia::new(vec![0xAA; SECTOR_SIZE], 1, true);
        media.write_batch(&[SectorWrite {
            lba: 0,
            bytes: [0xBB; SECTOR_SIZE],
        }]);
    }

    #[test]
    fn short_image_reads_zero_and_grows_with_complete_replacements() {
        let mut media = InMemoryMedia::new(vec![0xAA; 100], 3, false);
        let mut expected = [0; SECTOR_SIZE];
        expected[..100].fill(0xAA);
        assert_eq!(media.read_sector(0), expected);
        assert_eq!(media.read_sector(2), [0; SECTOR_SIZE]);
        media.write_batch(&[
            SectorWrite {
                lba: 2,
                bytes: [0xBB; SECTOR_SIZE],
            },
            SectorWrite {
                lba: 0,
                bytes: [0xCC; SECTOR_SIZE],
            },
        ]);
        assert_eq!(media.image.len(), 3 * SECTOR_SIZE);
        assert_eq!(media.read_sector(0), [0xCC; SECTOR_SIZE]);
        assert_eq!(media.read_sector(1), [0; SECTOR_SIZE]);
        assert_eq!(media.read_sector(2), [0xBB; SECTOR_SIZE]);
        assert!(!media.write_protected());
        assert!(InMemoryMedia::new(Vec::new(), 3, true).write_protected());
    }
}
