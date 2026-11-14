// SPDX-License-Identifier: AGPL-3.0-or-later
//! Immutable sector sources and a sparse writable top; persistent-media.md
//! § Memory and hashing. This disk-format seam can also back hash-linked layers.

use crate::{Header, SECTOR_LEN};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
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

/// A base's verified identity, shared without rehashing or copying its bytes.
#[derive(Clone)]
pub struct VerifiedBase {
    source: Rc<dyn SectorSource>,
    file: Option<Rc<File>>,
    identity: Header,
    metadata: Option<Metadata>,
}

impl std::fmt::Debug for VerifiedBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedBase")
            .field("identity", &self.identity)
            .field("file_backed", &self.metadata.is_some())
            .finish()
    }
}

impl VerifiedBase {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::from_file(File::open(path)?)
    }

    /// Verify an already opened read-only file after the caller checks its geometry.
    pub fn from_file(mut file: File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("base must be a regular file"));
        }
        let length = metadata.len();
        #[cfg(feature = "test-support")]
        note_data_pass(length);
        let identity = Header {
            base_length: length,
            base_digest: hash_reader(&mut file)?,
            anchor_generation: 0,
        };
        identity.encode().map_err(io::Error::other)?;
        let file = Rc::new(file);
        let source = FileSource {
            file: file.clone(),
            metadata: metadata.clone(),
        };
        source.check_unchanged()?;
        Ok(Self {
            source: Rc::new(source),
            file: Some(file),
            identity,
            metadata: Some(metadata),
        })
    }

    /// Reuse the same open descriptor and metadata from a completed full-file
    /// verification. The caller supplies that verification's SHA-256 identity.
    pub fn from_verified_file(
        file: File,
        identity: Header,
        metadata: Metadata,
    ) -> io::Result<Self> {
        if identity.base_length != metadata.len() {
            return Err(io::Error::other("verified base length mismatch"));
        }
        identity.encode().map_err(io::Error::other)?;
        let file = Rc::new(file);
        let source = FileSource {
            file: file.clone(),
            metadata: metadata.clone(),
        };
        source.check_unchanged()?;
        Ok(Self {
            source: Rc::new(source),
            file: Some(file),
            identity,
            metadata: Some(metadata),
        })
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
            file: None,
            metadata: None,
        }
    }

    /// Duplicate the verified descriptor, never a replacement at its old path.
    pub fn try_clone_verified_file(&self) -> io::Result<File> {
        self.check_unchanged()?;
        self.file
            .as_ref()
            .ok_or_else(|| io::Error::other("base is not file-backed"))?
            .try_clone()
    }

    pub fn identity(&self) -> &Header {
        &self.identity
    }
    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
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

struct FileSource {
    file: Rc<File>,
    metadata: Metadata,
}
impl SectorSource for FileSource {
    fn byte_len(&self) -> u64 {
        self.metadata.len()
    }
    fn check_unchanged(&self) -> io::Result<()> {
        let now = self.file.metadata()?;
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            now.len() == self.metadata.len()
                && now.mtime() == self.metadata.mtime()
                && now.mtime_nsec() == self.metadata.mtime_nsec()
                && now.ctime() == self.metadata.ctime()
                && now.ctime_nsec() == self.metadata.ctime_nsec()
        };
        #[cfg(not(unix))]
        let same =
            now.len() == self.metadata.len() && now.modified()? == self.metadata.modified()?;
        if same {
            Ok(())
        } else {
            Err(io::Error::other("base file changed after verification"))
        }
    }
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        self.check_unchanged()?;
        let offset = first_lba
            .checked_mul(SECTOR_LEN as u64)
            .ok_or_else(|| io::Error::other("base offset overflow"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_exact_at(bytes, offset)?;
        }
        #[cfg(not(unix))]
        {
            let mut file = self.file.try_clone()?;
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(bytes)?;
        }
        self.check_unchanged()
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        self.check_unchanged()?;
        let mut bytes = [0; SECTOR_LEN];
        if let Some(offset) = lba
            .checked_mul(SECTOR_LEN as u64)
            .filter(|&n| n < self.byte_len())
        {
            let count = (self.byte_len() - offset).min(SECTOR_LEN as u64) as usize;
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileExt;
                self.file.read_exact_at(&mut bytes[..count], offset)?;
            }
            #[cfg(not(unix))]
            {
                let mut file = self.file.try_clone()?;
                file.seek(SeekFrom::Start(offset))?;
                file.read_exact(&mut bytes[..count])?;
            }
        }
        self.check_unchanged()?;
        Ok(bytes)
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
    pub(crate) fn replacements(&self) -> impl Iterator<Item = (&u64, &Box<[u8; SECTOR_LEN]>)> {
        self.sectors.iter()
    }
    pub(crate) fn replace(&mut self, lba: u64, bytes: Box<[u8; SECTOR_LEN]>) {
        self.sectors.insert(lba, bytes);
    }
    pub(crate) fn same_replacements(&self, other: &Self) -> bool {
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

pub(crate) fn hash_reader(file: &mut (impl Read + Seek)) -> io::Result<[u8; 32]> {
    #[cfg(any(test, feature = "test-support"))]
    HASH_PASSES.with(|count| count.set(count.get() + 1));
    loop {
        match file.seek(SeekFrom::Start(0)) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = match file.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}
#[cfg(any(test, feature = "test-support"))]
thread_local! { pub(crate) static HASH_PASSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Number of streamed hash passes on this thread, for launch-path regression tests.
#[cfg(feature = "test-support")]
pub fn hash_passes() -> usize {
    HASH_PASSES.with(|count| count.get())
}

#[cfg(feature = "test-support")]
thread_local! { static DATA_PASSES: std::cell::RefCell<BTreeMap<u64, usize>> = const { std::cell::RefCell::new(BTreeMap::new()) }; }
#[cfg(feature = "test-support")]
pub(crate) fn note_data_pass(length: u64) {
    DATA_PASSES.with(|counts| *counts.borrow_mut().entry(length).or_default() += 1);
}
/// Complete raw/cache/logical passes of a given length on this thread.
#[cfg(feature = "test-support")]
pub fn data_passes(length: u64) -> usize {
    DATA_PASSES.with(|counts| counts.borrow().get(&length).copied().unwrap_or(0))
}

#[cfg(test)]
mod read_failure_tests {
    use super::*;
    use crate::{scan_overlay_reader, ClosingReason, Frame, Record};

    #[test]
    fn streaming_hash_retries_interrupted_reads() {
        struct InterruptedOnce {
            source: io::Cursor<Vec<u8>>,
            pending: bool,
            pending_seek: bool,
        }
        impl Read for InterruptedOnce {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if std::mem::take(&mut self.pending) {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.source.read(bytes)
            }
        }
        impl Seek for InterruptedOnce {
            fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
                if std::mem::take(&mut self.pending_seek) {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.source.seek(from)
            }
        }
        let bytes = vec![0x42; 100_000];
        let mut source = InterruptedOnce {
            source: io::Cursor::new(bytes.clone()),
            pending: true,
            pending_seek: true,
        };
        assert_eq!(
            hash_reader(&mut source).unwrap(),
            crate::codec::digest(&bytes)
        );
    }

    struct FailedRead;
    impl SectorSource for FailedRead {
        fn byte_len(&self) -> u64 {
            512
        }
        fn check_unchanged(&self) -> io::Result<()> {
            Ok(())
        }
        fn read_sector(&self, _: u64) -> io::Result<[u8; SECTOR_LEN]> {
            Err(io::Error::other("injected base read failure"))
        }
    }

    #[test]
    fn base_read_failure_during_anchor_validation_is_never_a_repairable_tail() {
        let base = VerifiedBase {
            source: Rc::new(FailedRead),
            file: None,
            identity: Header::new(&[0; 512], 0).unwrap(),
            metadata: None,
        };
        let mut header = base.identity().clone();
        header.anchor_generation = 1;
        let mut bytes = header.encode().unwrap().to_vec();
        for (index, record) in [
            Record::Sector {
                lba: 0,
                bytes: Box::new([1; 512]),
            },
            Record::CommandEnd { sectors: 1 },
            Record::GenerationEnd {
                reason: ClosingReason::RetentionAnchor,
            },
        ]
        .into_iter()
        .enumerate()
        {
            bytes.extend_from_slice(
                &Frame {
                    sequence: index as u64 + 1,
                    generation: 1,
                    record,
                }
                .encode()
                .unwrap(),
            );
        }
        let error = scan_overlay_reader(&mut io::Cursor::new(bytes), &base, 512).unwrap_err();
        assert!(error.to_string().contains("injected base read failure"));
    }
}
