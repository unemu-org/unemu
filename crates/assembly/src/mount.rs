// SPDX-License-Identifier: AGPL-3.0-or-later
//! The mount seam: the one object the assembler asks for what a
//! [`MachineConfig`] names on the host side, instead of opening it.
//!
//! A config names three kinds of host things by path: the bytes of an
//! [`ImageSource::File`](crate::ImageSource::File) (a ROM, a ROM part, a disk
//! image mounted without a journal, the CMOS image of an NVRAM journal), the
//! verified base of a journalled disk, and the journal of a
//! [`WriteMode::Journal`](crate::WriteMode::Journal) or an NVRAM journal. A
//! path is a key here, not a file the assembler opens: `machine_file::FileMount` answers
//! each key by opening the file it names, which the host-side assembly entry points use; another [`Mount`] answers from the contract's objects, a
//! [`ContentReader`] for a base and a [`Record`] for a journal, with no host
//! file at all. See `backlog/docs/design/core-contract.md` §12.5 (move 2) and
//! §14.4, and `backlog/docs/design/persistent-media.md` § Integration.
//!
//! A machine keeps its mount for its whole life: a catalogue's journal opens
//! when its medium is inserted, which can be long after assembly.

use std::any::Any;
use std::io;
use std::path::Path;
use std::rc::Rc;

use unemu_core_api::{ContentReader, Error, ErrorKind, Record};

use crate::{ConfigError, MachineConfig};

/// Where the assembler gets what a config names by path.
///
/// Each method is asked with the path the config holds. An answer that fails
/// with [`MountError::Host`] is reported as the config's read or journal
/// failure for that path, worded as the assembler words a file's failure; a
/// [`MountError::Config`] is reported unchanged.
pub trait Mount {
    /// Whether keys name the same host files as appended media.
    fn is_file_mount(&self) -> bool {
        false
    }
    /// The bytes of an image the config names by path: a ROM, a ROM part, a
    /// disk image mounted read-only or volatile, the CMOS image of an NVRAM
    /// journal. The assembler checks the expected SHA-256 and the length
    /// itself.
    fn image(&self, path: &Path) -> Result<ImageBytes, MountError>;

    /// The verified base of a journalled disk. `check_length` takes the
    /// base's length before any byte is read and refuses a length the drive
    /// cannot take; a mount that reads bytes before it knows the length calls
    /// it first, and returns its error unchanged. The assembler checks the
    /// length and the expected SHA-256 again on the answer, against the
    /// digest the answer carries: a mount's digest is trusted, never
    /// recomputed from the bytes ([`DiskBase::from_reader`]).
    fn disk_base(
        &self,
        path: &Path,
        check_length: &dyn Fn(u64) -> Result<(), ConfigError>,
    ) -> Result<DiskBase, MountError>;

    /// The journal of a journalled disk or of CMOS RAM. Asked when the
    /// journal opens: at assembly for a fixed disk, a single floppy and CMOS
    /// RAM, at each insert for a catalogue's medium. A record answer is held
    /// exclusively until the medium is ejected or the session ends.
    fn journal(&self, path: &Path) -> Result<JournalStore, MountError>;

    /// Check the journal outputs of `media` before anything is opened.
    /// `media` is the config with one drive per catalogue entry. The
    /// assembler itself refuses two journal outputs with the same key, for
    /// every mount, after this check. A file mount checks canonical paths
    /// and aliases between every output and every input; a mount of records
    /// has nothing on the host to alias, since a record is held exclusively
    /// when it opens.
    ///
    /// On a machine that took media by path after assembly, `media` holds
    /// those media too. When the machine's mount is not `machine_file::FileMount`, the
    /// assembler checks the appended media's host paths only against each
    /// other, and this method sees them beside the mount's own outputs; a
    /// mount whose keys stand for host files (a table from record name to
    /// journal file, say) must canonicalize the appended paths and check them
    /// against its own files here, since nothing else does.
    fn check_outputs(&self, media: &MachineConfig) -> Result<(), ConfigError>;
}

/// A failure answering one [`Mount`] request.
#[derive(Debug, thiserror::Error)]
pub enum MountError {
    /// The request itself is refused by a rule of the config.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The content or the record could not be had. Its message is what the
    /// assembler reports after the path.
    #[error(transparent)]
    Host(#[from] Error),
}

impl From<io::Error> for MountError {
    fn from(error: io::Error) -> Self {
        Self::Host(Error::new(ErrorKind::Storage, error.to_string()))
    }
}

/// The bytes of one image, as a [`Mount`] read them.
pub struct ImageBytes {
    bytes: Vec<u8>,
    origin: Option<Rc<dyn Any>>,
}

impl ImageBytes {
    /// Bytes from anywhere but a host file the assembler must protect.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            origin: None,
        }
    }

    /// Bytes with an opaque input identity that only the mount interprets.
    /// A host mount uses it to protect the input during journal publication.
    pub fn with_origin(bytes: Vec<u8>, origin: Rc<dyn Any>) -> Self {
        Self {
            bytes,
            origin: Some(origin),
        }
    }

    /// The bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_parts(self) -> (Vec<u8>, Option<Rc<dyn Any>>) {
        (self.bytes, self.origin)
    }
}

/// The verified, read-only base of a journalled disk: its bytes, its length
/// and its SHA-256, which a journal binds in its header. Nothing writes
/// through it.
#[derive(Clone)]
pub struct DiskBase {
    base: journal::VerifiedBase,
    origin: Option<Rc<dyn Any>>,
}

impl DiskBase {
    /// A base read through a [`ContentReader`] of the contract, whose length
    /// and SHA-256 the caller verified in full. The base is not hashed again.
    /// Reads go to the reader on demand; a failed read latches a host error at
    /// the next run boundary and the guest sees zeros.
    pub fn from_reader(
        reader: Box<dyn ContentReader>,
        sha256: [u8; 32],
        size: u64,
    ) -> Result<Self, MountError> {
        if reader.size() != size {
            return Err(Error::new(
                ErrorKind::Content,
                format!(
                    "the content holds {} bytes, not the {size} verified",
                    reader.size()
                ),
            )
            .into());
        }
        let identity = journal::Header {
            base_length: size,
            base_digest: sha256,
            anchor_generation: 0,
        };
        journal::VerifiedBase::from_verified_source(Rc::new(ReaderSource(reader)), identity)
            .map(Self::from)
            .map_err(|error| Error::new(ErrorKind::Content, error.to_string()).into())
    }

    /// A base in memory, hashed here.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MountError> {
        journal::VerifiedBase::from_bytes(bytes)
            .map(Self::from)
            .map_err(|error| Error::new(ErrorKind::Content, error.to_string()).into())
    }

    /// The base's length in bytes.
    pub fn len(&self) -> u64 {
        self.base.identity().base_length
    }

    /// Whether the base has no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The base's SHA-256.
    pub fn sha256(&self) -> [u8; 32] {
        self.base.identity().base_digest
    }

    pub fn with_origin(mut self, origin: Option<Rc<dyn Any>>) -> Self {
        self.origin = origin;
        self
    }

    pub fn verified(&self) -> &journal::VerifiedBase {
        &self.base
    }

    pub fn origin(&self) -> Option<&Rc<dyn Any>> {
        self.origin.as_ref()
    }
}

impl From<journal::VerifiedBase> for DiskBase {
    fn from(base: journal::VerifiedBase) -> Self {
        Self { base, origin: None }
    }
}

/// Sector reads of a disk base over a [`ContentReader`].
struct ReaderSource(Box<dyn ContentReader>);

impl journal::SectorSource for ReaderSource {
    fn byte_len(&self) -> u64 {
        self.0.size()
    }

    fn read_sector(&self, lba: u64) -> io::Result<[u8; journal::SECTOR_LEN]> {
        let mut bytes = [0; journal::SECTOR_LEN];
        let length = self.0.size();
        if let Some(offset) = lba
            .checked_mul(journal::SECTOR_LEN as u64)
            .filter(|&offset| offset < length)
        {
            let count = (length - offset).min(journal::SECTOR_LEN as u64) as usize;
            self.0
                .read_at(offset, &mut bytes[..count])
                .map_err(io::Error::other)?;
        }
        Ok(bytes)
    }

    /// A reader reports changed bytes on its next read.
    fn check_unchanged(&self) -> io::Result<()> {
        Ok(())
    }

    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        let offset = first_lba
            .checked_mul(journal::SECTOR_LEN as u64)
            .ok_or_else(|| io::Error::other("base offset overflow"))?;
        self.0.read_at(offset, bytes).map_err(io::Error::other)
    }
}

/// Where one journal is kept.
pub struct JournalStore(Box<JournalOpener>);

type JournalOpener = dyn FnOnce(
    &DiskBase,
    u64,
    journal::SinkStatus,
) -> Result<Box<dyn journal::JournalSession>, journal::OpenError>;
impl JournalStore {
    /// Open a record using the journal's byte format and lifecycle.
    pub fn record(record: Box<dyn Record>) -> Self {
        Self::new(move |base, length, status| {
            journal::JournalMedia::open_record(record, base.verified(), length, status)
                .map(|session| Box::new(session) as Box<dyn journal::JournalSession>)
        })
    }
    /// Defer journal opening until the assembler supplies its verified base.
    /// The host can protect the base's origin while creating the record.
    pub fn new(
        open: impl FnOnce(
                &DiskBase,
                u64,
                journal::SinkStatus,
            ) -> Result<Box<dyn journal::JournalSession>, journal::OpenError>
            + 'static,
    ) -> Self {
        Self(Box::new(open))
    }
    pub(crate) fn open(
        self,
        base: &DiskBase,
        length: u64,
        status: journal::SinkStatus,
    ) -> Result<Box<dyn journal::JournalSession>, journal::OpenError> {
        (self.0)(base, length, status)
    }
}

/// A mount for configs containing only inline images.
#[derive(Default)]
pub struct InlineMount;
impl Mount for InlineMount {
    fn is_file_mount(&self) -> bool {
        // Appends to a hand-built machine still validate its whole host config.
        true
    }

    fn image(&self, _path: &Path) -> Result<ImageBytes, MountError> {
        Err(Error::new(ErrorKind::Content, "a file mount is required").into())
    }
    fn disk_base(
        &self,
        _path: &Path,
        _check_length: &dyn Fn(u64) -> Result<(), ConfigError>,
    ) -> Result<DiskBase, MountError> {
        Err(Error::new(ErrorKind::Content, "a file mount is required").into())
    }
    fn journal(&self, _path: &Path) -> Result<JournalStore, MountError> {
        Err(Error::new(ErrorKind::Storage, "a journal mount is required").into())
    }
    fn check_outputs(&self, _media: &MachineConfig) -> Result<(), ConfigError> {
        Ok(())
    }
}

/// The mount retained for later catalogue insertions.
#[derive(Clone)]
pub struct MachineMount {
    mount: Rc<dyn Mount>,
    appended: Vec<(usize, crate::FloppyMedium)>,
}
impl Default for MachineMount {
    fn default() -> Self {
        Self::new(Rc::new(InlineMount))
    }
}
impl MachineMount {
    /// Keep a mount alive for the machine and all future insertions.
    pub fn new(mount: Rc<dyn Mount>) -> Self {
        Self {
            mount,
            appended: Vec::new(),
        }
    }
    /// File media appended after assembly, checked separately on record mounts.
    pub fn appended_media(&self) -> &[(usize, crate::FloppyMedium)] {
        &self.appended
    }
    /// Retain the validated file media only after a successful append.
    pub fn set_appended_media(&mut self, media: Vec<(usize, crate::FloppyMedium)>) {
        self.appended = media;
    }

    /// The retained mount, including its output validation rules.
    pub fn mount(&self) -> Rc<dyn Mount> {
        self.mount.clone()
    }
}

impl std::fmt::Debug for DiskBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskBase")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}
