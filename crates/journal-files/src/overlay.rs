// SPDX-License-Identifier: AGPL-3.0-or-later
//! Immutable sector sources and a sparse writable top; persistent-media.md
//! § Memory and hashing. This disk-format seam can also back hash-linked layers.

use crate::{Header, SECTOR_LEN};
pub use journal::{SectorSource, SparseOverlay};
use sha2::{Digest, Sha256};
#[cfg(feature = "test-support")]
use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

/// A base's verified identity, shared without rehashing or copying its bytes.
#[derive(Clone)]
pub struct VerifiedBase {
    source: journal::VerifiedBase,
    file: Option<Rc<File>>,
    metadata: Option<Metadata>,
}

impl std::fmt::Debug for VerifiedBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedBase")
            .field("identity", self.identity())
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
            source: journal::VerifiedBase::from_verified_source(Rc::new(source), identity)?,
            file: Some(file),
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
            source: journal::VerifiedBase::from_verified_source(Rc::new(source), identity)?,
            file: Some(file),
            metadata: Some(metadata),
        })
    }

    /// Wrap a verified immutable sector source without rehashing it.
    pub fn from_verified_source(
        source: Rc<dyn SectorSource>,
        identity: Header,
    ) -> io::Result<Self> {
        Ok(Self {
            source: journal::VerifiedBase::from_verified_source(source, identity)?,
            file: None,
            metadata: None,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::Error> {
        Ok(Self {
            source: journal::VerifiedBase::from_bytes(bytes)?,
            file: None,
            metadata: None,
        })
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
        self.source.identity()
    }
    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }
}

impl SectorSource for VerifiedBase {
    fn byte_len(&self) -> u64 {
        self.identity().base_length
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

impl std::ops::Deref for VerifiedBase {
    type Target = journal::VerifiedBase;
    fn deref(&self) -> &Self::Target {
        &self.source
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
pub fn hash_reader(file: &mut (impl Read + Seek)) -> io::Result<[u8; 32]> {
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
pub fn note_data_pass(length: u64) {
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

    #[cfg(unix)]
    #[test]
    fn base_reads_preserve_underlying_io_error_kind_and_message() {
        use std::os::unix::fs::FileExt;

        let path =
            std::env::temp_dir().join(format!("journal-base-read-failure-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        file.set_len(SECTOR_LEN as u64).unwrap();
        let metadata = file.metadata().unwrap();
        let expected = file.read_exact_at(&mut [0; SECTOR_LEN], 0).unwrap_err();
        let base = VerifiedBase::from_verified_file(
            file,
            Header::new(&[0; SECTOR_LEN], 0).unwrap(),
            metadata,
        )
        .unwrap();

        let errors = [
            base.read_sector(0).unwrap_err(),
            base.read_range(0, &mut [0; SECTOR_LEN]).unwrap_err(),
        ];
        for error in errors {
            assert_eq!(
                (error.kind(), error.to_string()),
                (expected.kind(), expected.to_string())
            );
        }
    }

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
            source: journal::VerifiedBase::from_verified_source(
                Rc::new(FailedRead),
                Header::new(&[0; 512], 0).unwrap(),
            )
            .unwrap(),
            file: None,
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
