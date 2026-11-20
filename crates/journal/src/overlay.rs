// SPDX-License-Identifier: AGPL-3.0-or-later
//! Immutable sector sources and a sparse writable top; persistent-media.md
//! § Memory and hashing. This disk-format seam can also back hash-linked layers.

use crate::{Header, SECTOR_LEN};
use std::collections::BTreeMap;
use std::io;
use std::rc::Rc;

/// One immutable parent in a sector-overlay chain. Host errors remain outside
/// controller state; the assembly adapter latches them at its host boundary.
/// See persistent-media.md § Memory and hashing.
pub trait SectorSource {
    fn byte_len(&self) -> u64;
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]>;
    fn check_unchanged(&self) -> io::Result<()>;
    /// Read an aligned range for bounded streaming exports. The final short CMOS
    /// block is permitted; layer implementations need only supply sector reads.
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        for (index, chunk) in bytes.chunks_mut(SECTOR_LEN).enumerate() {
            chunk.copy_from_slice(&self.read_sector(first_lba + index as u64)?[..chunk.len()]);
        }
        self.check_unchanged()
    }
}

/// A verified immutable sector source; persistent-media.md § Memory and hashing.
#[derive(Clone, Debug)]
pub struct VerifiedBase {
    source: Rc<dyn SectorSource>,
    identity: Header,
}

impl std::fmt::Debug for dyn SectorSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectorSource")
            .field("byte_len", &self.byte_len())
            .finish()
    }
}

impl VerifiedBase {
    /// Wrap an immutable source whose complete identity the caller has already
    /// established, such as an all-zero blank disk or a verified layer chain.
    /// Reads go straight to the source; nothing is copied, hashed or materialized.
    pub fn from_verified_source(
        source: Rc<dyn SectorSource>,
        identity: Header,
    ) -> io::Result<Self> {
        if identity.base_length != source.byte_len() {
            return Err(io::Error::other("verified base length mismatch"));
        }
        identity.encode().map_err(io::Error::other)?;
        source.check_unchanged()?;
        Ok(Self { source, identity })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::Error> {
        let base = Self::memory(bytes);
        base.identity.encode()?;
        Ok(base)
    }

    pub(crate) fn memory(bytes: &[u8]) -> Self {
        Self {
            identity: Header {
                base_length: bytes.len() as u64,
                base_digest: crate::codec::digest(bytes),
                anchor_generation: 0,
            },
            source: Rc::new(MemorySource(bytes.to_vec())),
        }
    }

    pub fn identity(&self) -> &Header {
        &self.identity
    }
}

impl SectorSource for VerifiedBase {
    fn byte_len(&self) -> u64 {
        self.identity.base_length
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        self.source.read_sector(lba)
    }
    fn check_unchanged(&self) -> io::Result<()> {
        self.source.check_unchanged()
    }
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        self.source.read_range(first_lba, bytes)
    }
}

struct MemorySource(Vec<u8>);
impl SectorSource for MemorySource {
    fn byte_len(&self) -> u64 {
        self.0.len() as u64
    }
    fn check_unchanged(&self) -> io::Result<()> {
        Ok(())
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        let mut bytes = [0; SECTOR_LEN];
        if let Some(start) = usize::try_from(lba)
            .ok()
            .and_then(|n| n.checked_mul(SECTOR_LEN))
            .filter(|&n| n < self.0.len())
        {
            let end = (start + SECTOR_LEN).min(self.0.len());
            bytes[..end - start].copy_from_slice(&self.0[start..end]);
        }
        Ok(bytes)
    }
}

/// Deterministically ordered replacements over an immutable parent. A future
/// layer can implement `SectorSource` and become this top's parent unchanged.
/// See persistent-media.md § Memory and hashing.
pub struct SparseOverlay {
    parent: Rc<dyn SectorSource>,
    sectors: BTreeMap<u64, Box<[u8; SECTOR_LEN]>>,
}
impl std::fmt::Debug for SparseOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparseOverlay")
            .field("byte_len", &self.byte_len())
            .field("sectors", &self.sectors)
            .finish()
    }
}
impl SparseOverlay {
    pub fn new(parent: Rc<dyn SectorSource>) -> Self {
        Self {
            parent,
            sectors: BTreeMap::new(),
        }
    }
    /// Test only written sectors; an unchanged disk needs no full comparison pass.
    pub fn has_changes(&self) -> io::Result<bool> {
        for (&lba, bytes) in &self.sectors {
            if self.parent.read_sector(lba)? != **bytes {
                return Ok(true);
            }
        }
        self.check_unchanged()?;
        Ok(false)
    }
    pub fn written_sectors(&self) -> usize {
        self.sectors.len()
    }
    pub fn replacements(&self) -> impl Iterator<Item = (&u64, &Box<[u8; SECTOR_LEN]>)> {
        self.sectors.iter()
    }
    pub fn replace(&mut self, lba: u64, bytes: Box<[u8; SECTOR_LEN]>) {
        self.sectors.insert(lba, bytes);
    }
    pub fn same_replacements(&self, other: &Self) -> bool {
        self.sectors == other.sectors
    }
    /// Explicit capture helper for small callers; mount and maintenance never
    /// materialize an image. Export streams sectors instead.
    pub fn materialize(&self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        for lba in 0..self.byte_len().div_ceil(SECTOR_LEN as u64) {
            bytes.extend_from_slice(&self.read_sector(lba)?);
        }
        bytes.truncate(self.byte_len() as usize);
        Ok(bytes)
    }
}
impl SectorSource for SparseOverlay {
    fn byte_len(&self) -> u64 {
        self.parent.byte_len()
    }
    fn check_unchanged(&self) -> io::Result<()> {
        self.parent.check_unchanged()
    }
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        self.parent.read_range(first_lba, bytes)?;
        let end = first_lba + bytes.len().div_ceil(SECTOR_LEN) as u64;
        for (&lba, sector) in self.sectors.range(first_lba..end) {
            let start = (lba - first_lba) as usize * SECTOR_LEN;
            let count = (bytes.len() - start).min(SECTOR_LEN);
            bytes[start..start + count].copy_from_slice(&sector[..count]);
        }
        self.check_unchanged()
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        self.check_unchanged()?;
        self.sectors
            .get(&lba)
            .map_or_else(|| self.parent.read_sector(lba), |bytes| Ok(**bytes))
    }
}
