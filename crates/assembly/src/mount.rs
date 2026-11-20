// SPDX-License-Identifier: AGPL-3.0-or-later
//! The mount seam: the one object the assembler asks for what a
//! [`MachineConfig`] names on the host side, instead of opening it.
//!
//! A config names three kinds of host things by path: the bytes of an
//! [`ImageSource::File`](crate::ImageSource::File) (a ROM, a ROM part, a disk
//! image mounted without a journal, the CMOS image of an NVRAM journal), the
//! verified base of a journalled disk, and the journal of a
//! [`WriteMode::Journal`](crate::WriteMode::Journal) or an NVRAM journal. A
//! path is a key here, not a file the assembler opens: [`FileMount`] answers
//! each key by opening the file it names, which is what every `assemble*`
//! function but [`crate::assemble_mounted`] does; another [`Mount`] answers from the contract's objects, a
//! [`ContentReader`] for a base and a [`Record`] for a journal, with no host
//! file at all. See `backlog/docs/design/core-contract.md` §12.5 (move 2) and
//! §14.4, and `backlog/docs/design/persistent-media.md` § Integration.
//!
//! A machine keeps its mount for its whole life: a catalogue's journal opens
//! when its medium is inserted, which can be long after assembly.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
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
    /// those media too. When the machine's mount is not [`FileMount`], the
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
    origin: Option<std::fs::Metadata>,
}

impl ImageBytes {
    /// Bytes from anywhere but a host file the assembler must protect.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            origin: None,
        }
    }

    /// The bytes read from the host file whose identity is `metadata`. A
    /// journal over these bytes never removes that file, even when it is
    /// linked under a name the journal reserves for its temporaries.
    pub(crate) fn from_file(bytes: Vec<u8>, metadata: std::fs::Metadata) -> Self {
        Self {
            bytes,
            origin: Some(metadata),
        }
    }

    /// The bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_parts(self) -> (Vec<u8>, Option<std::fs::Metadata>) {
        (self.bytes, self.origin)
    }
}

/// The verified, read-only base of a journalled disk: its bytes, its length
/// and its SHA-256, which a journal binds in its header. Nothing writes
/// through it.
#[derive(Clone, Debug)]
pub struct DiskBase {
    base: media::VerifiedBase,
    origin: Option<std::fs::Metadata>,
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
        let identity = media::Header {
            base_length: size,
            base_digest: sha256,
            anchor_generation: 0,
        };
        media::VerifiedBase::from_verified_source(Rc::new(ReaderSource(reader)), identity)
            .map(Self::from)
            .map_err(|error| Error::new(ErrorKind::Content, error.to_string()).into())
    }

    /// A base in memory, hashed here.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MountError> {
        media::VerifiedBase::from_bytes(bytes)
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

    pub(crate) fn with_origin(mut self, origin: Option<std::fs::Metadata>) -> Self {
        self.origin = origin;
        self
    }

    pub(crate) fn verified(&self) -> &media::VerifiedBase {
        &self.base
    }

    pub(crate) fn origin(&self) -> Option<&std::fs::Metadata> {
        self.origin.as_ref()
    }
}

impl From<media::VerifiedBase> for DiskBase {
    fn from(base: media::VerifiedBase) -> Self {
        Self { base, origin: None }
    }
}

/// Sector reads of a disk base over a [`ContentReader`].
struct ReaderSource(Box<dyn ContentReader>);

impl media::SectorSource for ReaderSource {
    fn byte_len(&self) -> u64 {
        self.0.size()
    }

    fn read_sector(&self, lba: u64) -> io::Result<[u8; media::SECTOR_LEN]> {
        let mut bytes = [0; media::SECTOR_LEN];
        let length = self.0.size();
        if let Some(offset) = lba
            .checked_mul(media::SECTOR_LEN as u64)
            .filter(|&offset| offset < length)
        {
            let count = (length - offset).min(media::SECTOR_LEN as u64) as usize;
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
            .checked_mul(media::SECTOR_LEN as u64)
            .ok_or_else(|| io::Error::other("base offset overflow"))?;
        self.0.read_at(offset, bytes).map_err(io::Error::other)
    }
}

/// Where one journal is kept.
pub struct JournalStore(Store);

pub(crate) enum Store {
    /// A journal file at a host path, opened by the journal's own path
    /// constructors with its `FILE.lock`.
    File(PathBuf),
    /// A record of the contract, held exclusively already.
    Record(Box<dyn Record>),
}

impl JournalStore {
    /// A journal kept in a record of the contract. An empty record becomes a
    /// new journal; the record's bytes are the version-1 journal format, the
    /// same bytes a journal file holds.
    pub fn record(record: Box<dyn Record>) -> Self {
        Self(Store::Record(record))
    }

    pub(crate) fn file(path: &Path) -> Self {
        Self(Store::File(path.into()))
    }

    pub(crate) fn into_store(self) -> Store {
        self.0
    }
}

/// The mount over host files: each path is the file it names. This is what
/// every `assemble*` function but [`crate::assemble_mounted`] uses, and what
/// a medium appended by path uses.
///
/// Bases the caller verified already are taken from its table by path and
/// never hashed again; every other base is hashed in full when it is mounted.
#[derive(Clone, Debug, Default)]
pub struct FileMount {
    verified: BTreeMap<PathBuf, media::VerifiedBase>,
}

impl FileMount {
    /// Open every path on the host.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the bases in `verified` by their exact path instead of opening
    /// and hashing those files; their metadata is checked again.
    pub fn with_verified_bases(verified: BTreeMap<PathBuf, media::VerifiedBase>) -> Self {
        Self { verified }
    }
}

impl Mount for FileMount {
    fn image(&self, path: &Path) -> Result<ImageBytes, MountError> {
        let mut file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(ImageBytes::from_file(bytes, metadata))
    }

    fn disk_base(
        &self,
        path: &Path,
        check_length: &dyn Fn(u64) -> Result<(), ConfigError>,
    ) -> Result<DiskBase, MountError> {
        use media::SectorSource;
        if let Some(base) = self.verified.get(path) {
            base.check_unchanged()?;
            return Ok(base.clone().into());
        }
        let file = std::fs::File::open(path)?;
        check_length(file.metadata()?.len())?;
        let base = media::VerifiedBase::from_file(file)?;
        let origin = base.metadata().cloned();
        Ok(DiskBase::from(base).with_origin(origin))
    }

    fn journal(&self, path: &Path) -> Result<JournalStore, MountError> {
        Ok(JournalStore::file(path))
    }

    fn check_outputs(&self, media: &MachineConfig) -> Result<(), ConfigError> {
        crate::persistence::validate_paths(media)
    }
}

/// The mount a machine was assembled with, kept for the machine's life
/// ([`crate::AssembledMachine::mount`]). The default is host files: what
/// every `assemble*` function but [`crate::assemble_mounted`] mounts, and
/// what a machine put together from parts by hand has.
#[derive(Default)]
pub struct MachineMount(pub(crate) MountKind);

#[derive(Default)]
pub(crate) enum MountKind {
    /// Host files. A medium appended by path is checked against every other
    /// path of the config.
    #[default]
    Files,
    /// Another mount. A medium appended by path is a host file all the same;
    /// the mount checks its own outputs, and the media appended by path are
    /// checked against each other (`host_media`, by drive index).
    Other {
        mount: Rc<dyn Mount>,
        host_media: Vec<(usize, crate::FloppyMedium)>,
    },
}
