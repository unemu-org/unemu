// SPDX-License-Identifier: AGPL-3.0-or-later
//! Explicit little-endian wire encoding; see persistent-media.md § Chosen format.

use sha2::{Digest, Sha256};

pub const HEADER_LEN: usize = 128;
pub const FRAME_LEN: usize = 640;
pub const SECTOR_LEN: usize = 512;
const HEADER_MAGIC: &[u8; 8] = b"UPCFJNL\0";
const FRAME_MAGIC: &[u8; 8] = b"UPCFREC\0";

/// A format, binding or grammar error; offsets are supplied by the scanner.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("journal header must be exactly 128 bytes")]
    HeaderLength,
    #[error("invalid journal header magic")]
    HeaderMagic,
    #[error("unsupported journal version {0}")]
    Version(u16),
    #[error("journal header checksum mismatch")]
    HeaderChecksum,
    #[error("base length must be 64 bytes or positive and sector aligned, and match the header")]
    BaseLength,
    #[error("configured geometry length does not match the base")]
    GeometryLength,
    #[error("base digest does not match the journal")]
    BaseDigest,
    #[error("base read failed: {0}")]
    BaseRead(String),
    #[error("journal frame must be exactly 640 bytes")]
    FrameLength,
    #[error("invalid journal frame magic")]
    FrameMagic,
    #[error("journal frame checksum mismatch")]
    FrameChecksum,
    #[error("unknown journal frame kind {0}")]
    UnknownKind(u8),
    #[error("invalid journal encoding: {0}")]
    Encoding(&'static str),
    #[error("expected sequence {expected}, found {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("expected generation {expected}, found {actual}")]
    Generation { expected: u64, actual: u64 },
    #[error("sector LBA {0} is outside the base image")]
    SectorBounds(u64),
    #[error("duplicate sector LBA {0} in one command")]
    DuplicateSector(u64),
    #[error("expected command sector count {expected}, found {actual}")]
    CommandCount { expected: u64, actual: u64 },
    #[error("ordinary command has no sectors")]
    EmptyCommand,
    #[error("generation ends inside a command")]
    GenerationInsideCommand,
    #[error("invalid retention anchor: {0}")]
    Anchor(&'static str),
    #[error("journal counter exhausted")]
    IntegerExhausted,
    #[error("incomplete final frame")]
    PartialFrame,
    #[error("final command has no COMMAND_END")]
    PendingCommand,
}

/// Base identity and oldest retained generation. Fields are validated on encode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub base_length: u64,
    pub base_digest: [u8; 32],
    pub anchor_generation: u64,
}

impl Header {
    pub fn new(base: &[u8], anchor_generation: u64) -> Result<Self, Error> {
        let base_length = u64::try_from(base.len()).map_err(|_| Error::BaseLength)?;
        check_base_length(base_length)?;
        Ok(Self {
            base_length,
            base_digest: digest(base),
            anchor_generation,
        })
    }

    pub fn encode(&self) -> Result<[u8; HEADER_LEN], Error> {
        check_base_length(self.base_length)?;
        let mut bytes = [0; HEADER_LEN];
        bytes[..8].copy_from_slice(HEADER_MAGIC);
        bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        bytes[12..16].copy_from_slice(&(SECTOR_LEN as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.base_length.to_le_bytes());
        bytes[24..56].copy_from_slice(&self.base_digest);
        bytes[56..64].copy_from_slice(&self.anchor_generation.to_le_bytes());
        let hash = digest(&bytes[..96]);
        bytes[96..].copy_from_slice(&hash);
        Ok(bytes)
    }

    /// Parse exactly one header and bind it to the already loaded base/geometry.
    pub fn decode(bytes: &[u8], base: &[u8], geometry_length: u64) -> Result<Self, Error> {
        let header = Self::decode_fields(bytes)?;
        if base.len() as u64 != header.base_length {
            return Err(Error::BaseLength);
        }
        if geometry_length != header.base_length {
            return Err(Error::GeometryLength);
        }
        if digest(base) != header.base_digest {
            return Err(Error::BaseDigest);
        }
        Ok(header)
    }

    pub(crate) fn decode_identity(
        bytes: &[u8],
        base: &Self,
        geometry_length: u64,
    ) -> Result<Self, Error> {
        let header = Self::decode_fields(bytes)?;
        let base_length = header.base_length;
        if base.base_length != base_length {
            return Err(Error::BaseLength);
        }
        if geometry_length != base_length {
            return Err(Error::GeometryLength);
        }
        let base_digest = base.base_digest;
        if base_digest != bytes[24..56] {
            return Err(Error::BaseDigest);
        }
        Ok(Self {
            base_length,
            base_digest,
            anchor_generation: read_u64(&bytes[56..64]),
        })
    }
    /// Read a checksummed header for advisory displays without loading its base.
    /// Execution and publication must still bind it to a verified base identity.
    pub fn decode_unbound(bytes: &[u8]) -> Result<Self, Error> {
        Self::decode_fields(bytes)
    }

    fn decode_fields(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != HEADER_LEN {
            return Err(Error::HeaderLength);
        }
        if &bytes[..8] != HEADER_MAGIC {
            return Err(Error::HeaderMagic);
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != 1 {
            return Err(Error::Version(version));
        }
        if bytes[10..12] != (HEADER_LEN as u16).to_le_bytes() {
            return Err(Error::HeaderLength);
        }
        if digest(&bytes[..96]) != bytes[96..] {
            return Err(Error::HeaderChecksum);
        }
        if bytes[12..16] != (SECTOR_LEN as u32).to_le_bytes()
            || bytes[64..96].iter().any(|&b| b != 0)
        {
            return Err(Error::Encoding("header sizes, flags or reserved bytes"));
        }
        let base_length = read_u64(&bytes[16..24]);
        check_base_length(base_length)?;
        let mut base_digest = [0; 32];
        base_digest.copy_from_slice(&bytes[24..56]);
        Ok(Self {
            base_length,
            base_digest,
            anchor_generation: read_u64(&bytes[56..64]),
        })
    }
}

/// The CMOS clock/control prefix and AT shutdown byte are not journal state.
/// See persistent-media.md § CMOS NVRAM journals.
pub(crate) fn valid_nvram_block(base: &[u8], bytes: &[u8; SECTOR_LEN]) -> bool {
    bytes[..14] == base[..14] && bytes[15] == base[15] && bytes[64..].iter().all(|&b| b == 0)
}

fn check_base_length(length: u64) -> Result<(), Error> {
    if length != 64 && (length == 0 || !length.is_multiple_of(SECTOR_LEN as u64)) {
        return Err(Error::BaseLength);
    }
    Ok(())
}

/// Generation-end target values defined by the version-1 format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum ClosingReason {
    SessionEnd = 1,
    Checkpoint = 2,
    Recovered = 3,
    RetentionAnchor = 4,
}

/// Complete replacements only: partial DMA writes must already be zero padded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Sector {
        lba: u64,
        bytes: Box<[u8; SECTOR_LEN]>,
    },
    CommandEnd {
        sectors: u64,
    },
    GenerationEnd {
        reason: ClosingReason,
    },
}

/// One frame; local encoding checks do not replace whole-journal grammar checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub sequence: u64,
    pub generation: u64,
    pub record: Record,
}

impl Frame {
    pub fn encode(&self) -> Result<[u8; FRAME_LEN], Error> {
        if self.sequence == 0 || self.generation == 0 {
            return Err(Error::Encoding("zero sequence or generation"));
        }
        let mut bytes = [0; FRAME_LEN];
        bytes[..8].copy_from_slice(FRAME_MAGIC);
        bytes[10..12].copy_from_slice(&64u16.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.generation.to_le_bytes());
        let target = match &self.record {
            Record::Sector { lba, bytes: sector } => {
                bytes[8] = 1;
                bytes[12..16].copy_from_slice(&(SECTOR_LEN as u32).to_le_bytes());
                bytes[64..576].copy_from_slice(sector.as_ref());
                *lba
            }
            Record::CommandEnd { sectors } => {
                bytes[8] = 2;
                *sectors
            }
            Record::GenerationEnd { reason } => {
                bytes[8] = 3;
                *reason as u64
            }
        };
        bytes[32..40].copy_from_slice(&target.to_le_bytes());
        let hash = digest(&bytes[..608]);
        bytes[608..].copy_from_slice(&hash);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != FRAME_LEN {
            return Err(Error::FrameLength);
        }
        if &bytes[..8] != FRAME_MAGIC {
            return Err(Error::FrameMagic);
        }
        if digest(&bytes[..608]) != bytes[608..] {
            return Err(Error::FrameChecksum);
        }
        if bytes[9] != 0
            || bytes[10..12] != 64u16.to_le_bytes()
            || bytes[40..64].iter().any(|&b| b != 0)
            || bytes[576..608].iter().any(|&b| b != 0)
        {
            return Err(Error::Encoding("frame header, flags or reserved bytes"));
        }
        let target = read_u64(&bytes[32..40]);
        let record = match bytes[8] {
            1 => {
                if bytes[12..16] != (SECTOR_LEN as u32).to_le_bytes() {
                    return Err(Error::Encoding("sector payload length"));
                }
                let mut sector = Box::new([0; SECTOR_LEN]);
                sector.copy_from_slice(&bytes[64..576]);
                Record::Sector {
                    lba: target,
                    bytes: sector,
                }
            }
            2 | 3 => {
                if bytes[12..16] != [0; 4] || bytes[64..576].iter().any(|&b| b != 0) {
                    return Err(Error::Encoding("marker payload"));
                }
                if bytes[8] == 2 {
                    Record::CommandEnd { sectors: target }
                } else {
                    let reason = match target {
                        1 => ClosingReason::SessionEnd,
                        2 => ClosingReason::Checkpoint,
                        3 => ClosingReason::Recovered,
                        4 => ClosingReason::RetentionAnchor,
                        _ => return Err(Error::Encoding("closing reason")),
                    };
                    Record::GenerationEnd { reason }
                }
            }
            kind => return Err(Error::UnknownKind(kind)),
        };
        let sequence = read_u64(&bytes[16..24]);
        let generation = read_u64(&bytes[24..32]);
        if sequence == 0 || generation == 0 {
            return Err(Error::Encoding("zero sequence or generation"));
        }
        Ok(Self {
            sequence,
            generation,
            record,
        })
    }
}

pub(crate) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn read_u64(bytes: &[u8]) -> u64 {
    let mut array = [0; 8];
    array.copy_from_slice(bytes);
    u64::from_le_bytes(array)
}

pub(crate) fn intact_generation_end(bytes: &[u8]) -> bool {
    // A later marker is evidence of history even when its ids or grammar disagree.
    bytes.len() == FRAME_LEN
        && &bytes[..8] == FRAME_MAGIC
        && bytes[8] == 3
        && digest(&bytes[..608]) == bytes[608..]
}

#[cfg(test)]
mod header_order_tests {
    use super::*;
    #[test]
    fn header_errors_precede_invalid_base_and_geometry() {
        assert_eq!(Header::decode(&[], &[], 0), Err(Error::HeaderLength));
        assert_eq!(
            Header::decode(&[0; HEADER_LEN], &[], 0),
            Err(Error::HeaderMagic)
        );
        let mut bytes = Header::new(&[0; 512], 0).unwrap().encode().unwrap();
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(Header::decode(&bytes, &[], 0), Err(Error::Version(2)));
    }
}
