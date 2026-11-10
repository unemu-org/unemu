// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side durable journal lifecycle; persistent-media.md § Persist at command
//! completion, § Open and tail recovery and § Host errors and determinism.
//!
//! This is a disk-format service, not controller hardware. Assembly can adapt its
//! complete-sector batches to `device::SectorMedia` without exposing this status
//! channel to the controller. Reads use only the reconstructed memory image.
//! No wall clock, worker thread or host failure becomes a guest input.
//!
//! Each successful command/end flushes userspace output and calls `File::sync_all`.
//! On Apple targets std supplies F_FULLFSYNC and retries Interrupted internally.
//! Creation publishes a synchronized temporary header without replacing an
//! existing destination. Mount verifies the opened canonical path, removes
//! creation-temporary leftovers under the lock and synchronizes the directory.
//! Dangling journal symlinks are refused before locking. Unsupported
//! barriers fail rather than weaken this contract. Tests measure operation
//! ordering, fault handling and real file reopen/lock behavior;
//! they do not measure power-loss durability. Preservation of synchronized bytes
//! and honest storage/cache barriers remain assumptions of the design.

use crate::{
    scan_reader, ClosingReason, Error, Frame, Header, Record, Refusal, Status, VerificationReport,
    SECTOR_LEN,
};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// First host output failure, retained independently of the mounted media.
/// A confirmed sequence is a successful barrier, not inferred crash survival.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{operation}: {message} (last confirmed sequence {last_confirmed_sequence})")]
pub struct SinkFailure {
    pub operation: &'static str,
    pub message: String,
    pub last_confirmed_sequence: u64,
}

/// Frontend-owned status, polled only at host cooperation/finalization boundaries.
/// Clones share a first-error latch; there is deliberately no clear operation.
#[derive(Clone, Debug, Default)]
pub struct SinkStatus(Rc<RefCell<Option<SinkFailure>>>);

impl SinkStatus {
    pub fn failure(&self) -> Option<SinkFailure> {
        self.0.borrow().clone()
    }

    fn latch(&self, failure: SinkFailure) -> SinkFailure {
        self.0.borrow_mut().get_or_insert(failure).clone()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(transparent)]
    Host(#[from] SinkFailure),
    #[error(transparent)]
    Format(#[from] Error),
    #[error("journal recovery refused: {0:?}")]
    Refused(Refusal),
}

/// Memory-backed host media and its exclusive journal session.
///
/// The assembly adapter supplies in-range LBAs in controller transfer order.
/// Duplicate LBAs use their last bytes and last occurrence order; out-of-range
/// batches are ignored in full, matching the volatile media contract.
/// `finish` consumes the session; dropping without it only releases handles,
/// leaving complete commands to be recovered on the next mount. Retention and
/// frontend boundary wiring are separate services, not implicit sink policies.
pub struct JournalMedia {
    image: Vec<u8>,
    output: Box<dyn JournalIo>,
    _lock: Box<dyn Any>,
    status: SinkStatus,
    sequence: u64,
    generation: u64,
    confirmed: u64,
    mount_report: VerificationReport,
}

impl JournalMedia {
    /// Whether a candidate has this journal's generated decimal temporary name.
    /// Validation and cleanup share this boundary to avoid nested namespaces.
    pub fn is_creation_temp(journal: &Path, candidate: &Path) -> bool {
        if journal.parent() != candidate.parent() {
            return false;
        }
        let (Some(journal_name), Some(name)) = (journal.file_name(), candidate.file_name()) else {
            return false;
        };
        let mut prefix = journal_name.to_os_string();
        prefix.push(".create-");
        let prefix = prefix.as_encoded_bytes();
        let name = name.as_encoded_bytes();
        name.strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_suffix(b".tmp"))
            .is_some_and(|counter| !counter.is_empty() && counter.iter().all(u8::is_ascii_digit))
    }

    /// Whether a filename contains the reserved creation marker. Rejecting it
    /// prevents nested journals from owning overlapping temporary namespaces.
    pub fn is_creation_temp_name(path: &Path) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let name = name.as_encoded_bytes();
        let marker = b".create-";
        name.windows(marker.len()).any(|part| part == marker)
    }

    /// Resolve symlinks and acquire a stable `<journal>.lock` before touching
    /// journal contents, then verify the canonical path again after opening.
    /// A missing journal's resolved parent forms the expected canonical path for
    /// that comparison. Never remove the sidecar: retention uses the same lock.
    /// The base is the exact already-loaded immutable image, not a path to reread.
    pub fn open(
        path: &Path,
        base: &[u8],
        geometry_length: u64,
        status: SinkStatus,
    ) -> Result<Self, OpenError> {
        let path = (|| {
            refuse_creation_temp_name(path)?;
            let canonical = canonical_journal_path(path)?;
            refuse_creation_temp_name(&canonical)?;
            Ok::<_, io::Error>(canonical)
        })()
        .map_err(|error| {
            OpenError::Host(status.latch(SinkFailure {
                operation: "resolve journal path",
                message: error.to_string(),
                last_confirmed_sequence: 0,
            }))
        })?;
        Self::open_with(&mut HostFilesystem, &path, base, geometry_length, status)
    }

    /// Inject a failing write sink after a successful real mount. This optional
    /// test seam lets consumers verify their boundary policy without process-wide
    /// resource limits or filesystem-specific quotas. Memory updates stay real.
    #[cfg(feature = "test-support")]
    pub fn with_failing_output(mut self) -> Self {
        self.output = Box::new(FailingOutput(self.output));
        self
    }

    fn open_with(
        fs: &mut impl Filesystem,
        path: &Path,
        base: &[u8],
        geometry_length: u64,
        status: SinkStatus,
    ) -> Result<Self, OpenError> {
        if let Some(failure) = status.failure() {
            return Err(failure.into());
        }
        let header = Header::new(base, 0)?;
        if header.base_length != geometry_length {
            return Err(Error::GeometryLength.into());
        }
        let confirmed = Cell::new(0);
        let host = |operation, error: io::Error| {
            OpenError::Host(status.latch(SinkFailure {
                operation,
                message: error.to_string(),
                last_confirmed_sequence: confirmed.get(),
            }))
        };
        let lock = fs.lock(path).map_err(|e| host("lock", e))?;
        let (mut output, created) = match fs.open(path).map_err(|e| host("open", e))? {
            Some(output) => (output, false),
            None => {
                let (mut output, temporary) = fs
                    .create_temp(path)
                    .map_err(|e| host("create temporary header", e))?;
                let initialization = (|| {
                    output
                        .write_all(&header.encode()?)
                        .map_err(|e| host("header write", e))?;
                    barrier(output.as_mut()).map_err(|e| host("header sync", e))?;
                    fs.publish(&temporary, path)
                        .map_err(|e| host("publish header", e))?;
                    fs.remove_temp(&temporary)
                        .map_err(|e| host("unlink temporary header", e))?;
                    Ok::<_, OpenError>(())
                })();
                if initialization.is_err() {
                    // Unpublished leftovers are never mounted. Best-effort cleanup
                    // must not replace the original failure in the status handle.
                    let _ = fs.remove_temp(&temporary);
                }
                initialization?;
                (output, true)
            }
        };
        fs.verify_path(path)
            .map_err(|e| host("verify journal path", e))?;
        fs.cleanup_creation_temps(path, &header.encode()?)
            .map_err(|e| host("remove creation temporaries", e))?;
        // Cover both a prior publication and removal of its leftover hard link.
        fs.sync_directory(path)
            .map_err(|e| host("directory sync", e))?;
        let scan = scan_reader(&mut output, base, geometry_length).map_err(|e| host("scan", e))?;
        let image = match scan.image {
            Some(image) => image,
            None => match scan.report.status {
                Status::Refused(ref refusal) => return Err(OpenError::Refused(refusal.clone())),
                _ => return Err(Error::HeaderLength.into()),
            },
        };
        let recovered_command = if scan.report.unsealed_tip.is_some() {
            scan.report.last_durable_sequence
        } else {
            scan.report
                .generations
                .iter()
                .rev()
                .find(|g| g.commands != 0)
                .map_or(0, |g| g.last_sequence - 1)
        };
        let mut sequence = scan.report.last_durable_sequence;
        let mut closed = scan.report.last_closed_generation.unwrap_or(0);
        if scan.report.discarded_bytes != 0 {
            output
                .truncate(scan.report.safe_offset as u64)
                .map_err(|e| host("recovery truncate", e))?;
            barrier(output.as_mut()).map_err(|e| host("recovery sync", e))?;
        }
        if !created && scan.report.discarded_bytes == 0 {
            // Confirm surviving history with our own barrier; file contents alone
            // cannot reveal whether a previous process acknowledged its sync.
            barrier(output.as_mut()).map_err(|e| host("mount sync", e))?;
        }
        confirmed.set(recovered_command);
        output
            .seek(SeekFrom::Start(scan.report.safe_offset as u64))
            .map_err(|e| host("append seek", e))?;
        if let Some(seal) = scan.report.recovery_seal()? {
            // Do not publish a marker that prevents any subsequent session id.
            seal.generation
                .checked_add(1)
                .ok_or(Error::IntegerExhausted)?;
            seal.sequence
                .checked_add(1)
                .ok_or(Error::IntegerExhausted)?;
            output
                .write_all(&seal.encode()?)
                .map_err(|e| host("recovery seal", e))?;
            barrier(output.as_mut()).map_err(|e| host("recovery seal sync", e))?;
            sequence = seal.sequence;
            closed = seal.generation;
        }
        let generation = closed.checked_add(1).ok_or(Error::IntegerExhausted)?;
        sequence.checked_add(1).ok_or(Error::IntegerExhausted)?;
        Ok(Self {
            image,
            output,
            _lock: lock,
            status,
            sequence,
            generation,
            confirmed: confirmed.get(),
            mount_report: scan.report,
        })
    }

    /// Original read-only recovery evidence, including discarded byte count and
    /// interrupted tip. Successful open has already performed these actions.
    pub fn mount_report(&self) -> &VerificationReport {
        &self.mount_report
    }

    pub fn status(&self) -> SinkStatus {
        self.status.clone()
    }

    pub fn image(&self) -> &[u8] {
        &self.image
    }

    pub fn read_sector(&self, lba: u64) -> [u8; SECTOR_LEN] {
        let mut bytes = [0; SECTOR_LEN];
        if let Some(range) = self.range(lba) {
            bytes.copy_from_slice(&self.image[range]);
        }
        bytes
    }

    pub fn write_protected(&self) -> bool {
        false
    }

    fn range(&self, lba: u64) -> Option<std::ops::Range<usize>> {
        let start = usize::try_from(lba).ok()?.checked_mul(SECTOR_LEN)?;
        let end = start.checked_add(SECTOR_LEN)?;
        (end <= self.image.len()).then_some(start..end)
    }

    /// Apply a completed nonempty command before attempting host output. The
    /// assembly adapter maps its sector records to these `(LBA, bytes)` pairs.
    /// Host failures have no result channel here and never undo memory writes.
    pub fn write_batch(&mut self, sectors: &[(u64, [u8; SECTOR_LEN])]) {
        if sectors.is_empty() || sectors.iter().any(|(lba, _)| self.range(*lba).is_none()) {
            return;
        }
        for (lba, bytes) in sectors {
            let start = *lba as usize * SECTOR_LEN;
            self.image[start..start + SECTOR_LEN].copy_from_slice(bytes);
        }
        if self.status.failure().is_some() {
            return;
        }
        let mut seen = BTreeSet::new();
        let mut replacements: Vec<_> = sectors
            .iter()
            .rev()
            .filter(|(lba, _)| seen.insert(*lba))
            .collect();
        replacements.reverse();
        let result = (|| {
            self.reserve(replacements.len() as u64 + 1)?;
            for (lba, bytes) in &replacements {
                self.append(Record::Sector {
                    lba: *lba,
                    bytes: Box::new(*bytes),
                })?;
            }
            self.append(Record::CommandEnd {
                sectors: replacements.len() as u64,
            })?;
            self.sync(true)
        })();
        if let Err(failure) = result {
            self.status.latch(failure);
        }
    }

    /// Caller must be at a completed machine service boundary. Empty generations
    /// are intentional recovery points; no automatic checkpoint cadence exists.
    pub fn checkpoint(&mut self) -> Result<(), SinkFailure> {
        self.close_generation(ClosingReason::Checkpoint)
    }

    /// Explicit successful session termination. On execution failure, drop the
    /// media instead so recovery can identify its commands as interrupted.
    pub fn finish(mut self) -> Result<(), SinkFailure> {
        self.close_generation(ClosingReason::SessionEnd)
    }

    fn close_generation(&mut self, reason: ClosingReason) -> Result<(), SinkFailure> {
        if let Some(failure) = self.status.failure() {
            return Err(failure);
        }
        let result = (|| {
            self.reserve(1)?;
            let next = self
                .generation
                .checked_add(1)
                .ok_or_else(|| self.failure("generation", Error::IntegerExhausted))?;
            self.append(Record::GenerationEnd { reason })?;
            self.sync(false)?;
            self.generation = next;
            Ok(())
        })();
        result.map_err(|failure| self.status.latch(failure))
    }

    fn reserve(&self, frames: u64) -> Result<(), SinkFailure> {
        // The scanner needs a representable next sequence even at a clean EOF.
        self.sequence
            .checked_add(frames)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| self.failure("sequence", Error::IntegerExhausted))?;
        Ok(())
    }

    fn append(&mut self, record: Record) -> Result<(), SinkFailure> {
        let sequence = self.sequence + 1;
        let bytes = Frame {
            sequence,
            generation: self.generation,
            record,
        }
        .encode()
        .map_err(|e| self.failure("encode", e))?;
        self.output
            .write_all(&bytes)
            .map_err(|e| self.failure("write", e))?;
        self.sequence = sequence;
        Ok(())
    }

    fn sync(&mut self, command: bool) -> Result<(), SinkFailure> {
        barrier(self.output.as_mut()).map_err(|e| self.failure("command/end sync", e))?;
        if command {
            self.confirmed = self.sequence;
        }
        Ok(())
    }

    fn failure(&self, operation: &'static str, error: impl std::fmt::Display) -> SinkFailure {
        SinkFailure {
            operation,
            message: error.to_string(),
            last_confirmed_sequence: self.confirmed,
        }
    }
}

/// Injectable host boundary; write_all remains the standard library's partial /
/// interrupted-write loop, so tests exercise exactly the production append path.
trait JournalIo: Read + Write + Seek {
    fn sync(&mut self) -> io::Result<()>;
    fn truncate(&mut self, length: u64) -> io::Result<()>;
}

fn barrier(output: &mut dyn JournalIo) -> io::Result<()> {
    output.flush()?;
    output.sync()
}

#[cfg(feature = "test-support")]
struct FailingOutput(Box<dyn JournalIo>);

#[cfg(feature = "test-support")]
impl Read for FailingOutput {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.read(bytes)
    }
}

#[cfg(feature = "test-support")]
impl Seek for FailingOutput {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

#[cfg(feature = "test-support")]
impl Write for FailingOutput {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("injected journal output failure"))
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(feature = "test-support")]
impl JournalIo for FailingOutput {
    fn sync(&mut self) -> io::Result<()> {
        self.0.sync()
    }
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        self.0.truncate(length)
    }
}

/// Namespace and lock operations are separate from the journal inode so the
/// lock remains stable across later atomic retention replacements.
trait Filesystem {
    fn lock(&mut self, path: &Path) -> io::Result<Box<dyn Any>>;
    fn open(&mut self, path: &Path) -> io::Result<Option<Box<dyn JournalIo>>>;
    fn verify_path(&mut self, path: &Path) -> io::Result<()>;
    fn cleanup_creation_temps(&mut self, path: &Path, header: &[u8]) -> io::Result<()>;
    fn create_temp(&mut self, path: &Path) -> io::Result<(Box<dyn JournalIo>, PathBuf)>;
    fn publish(&mut self, temporary: &Path, path: &Path) -> io::Result<()>;
    fn remove_temp(&mut self, temporary: &Path) -> io::Result<()>;
    fn sync_directory(&mut self, path: &Path) -> io::Result<()>;
}

struct HostFilesystem;

impl Filesystem for HostFilesystem {
    fn lock(&mut self, path: &Path) -> io::Result<Box<dyn Any>> {
        let mut name = path.as_os_str().to_os_string();
        name.push(".lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(name)?;
        file.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => {
                io::Error::new(io::ErrorKind::WouldBlock, "journal is locked")
            }
            std::fs::TryLockError::Error(error) => error,
        })?;
        Ok(Box::new(file))
    }

    fn open(&mut self, path: &Path) -> io::Result<Option<Box<dyn JournalIo>>> {
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => Ok(Some(Box::new(file))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn verify_path(&mut self, path: &Path) -> io::Result<()> {
        if path.canonicalize()? != path {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal canonical path changed after locking",
            ));
        }
        Ok(())
    }

    fn cleanup_creation_temps(&mut self, path: &Path, header: &[u8]) -> io::Result<()> {
        let parent = path.parent().unwrap_or(Path::new("."));
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let candidate = entry.path();
            if !JournalMedia::is_creation_temp(path, &candidate) {
                continue;
            }
            cleanup_creation_temp(&mut HostCreationTempIo, &candidate, header)?;
        }
        Ok(())
    }

    fn create_temp(&mut self, path: &Path) -> io::Result<(Box<dyn JournalIo>, PathBuf)> {
        for counter in 0..u64::MAX {
            let mut name = path.as_os_str().to_os_string();
            name.push(format!(".create-{counter}.tmp"));
            let temporary = PathBuf::from(name);
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => return Ok((Box::new(file), temporary)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("temporary header names exhausted"))
    }

    fn publish(&mut self, temporary: &Path, path: &Path) -> io::Result<()> {
        std::fs::hard_link(temporary, path)
    }

    fn remove_temp(&mut self, temporary: &Path) -> io::Result<()> {
        std::fs::remove_file(temporary)
    }

    fn sync_directory(&mut self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        platform_sync(&File::open(parent)?)
    }
}

impl JournalIo for File {
    fn sync(&mut self) -> io::Result<()> {
        platform_sync(self)
    }
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        self.set_len(length)
    }
}

trait CreationTempIo {
    fn metadata(&mut self, path: &Path) -> io::Result<std::fs::Metadata>;
    fn open(&mut self, path: &Path) -> io::Result<File>;
    fn remove(&mut self, path: &Path) -> io::Result<()>;
}

struct HostCreationTempIo;

impl CreationTempIo for HostCreationTempIo {
    fn metadata(&mut self, path: &Path) -> io::Result<std::fs::Metadata> {
        std::fs::symlink_metadata(path)
    }

    fn open(&mut self, path: &Path) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        options.open(path)
    }

    fn remove(&mut self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
}

#[cfg(unix)]
fn same_creation_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    after.file_type().is_file() && before.dev() == after.dev() && before.ino() == after.ino()
}

fn cleanup_creation_temp(
    io: &mut impl CreationTempIo,
    candidate: &Path,
    header: &[u8],
) -> io::Result<()> {
    let metadata = match io.metadata(candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() || metadata.len() > header.len() as u64 {
        return Ok(());
    }
    let Ok(mut file) = io.open(candidate) else {
        return Ok(());
    };
    #[cfg(unix)]
    if !same_creation_file(&metadata, &file.metadata()?) {
        return Ok(());
    }
    let mut bytes = Vec::new();
    if Read::by_ref(&mut file)
        .take(header.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Ok(());
    }
    // Creation writes only this header: interruption can leave an empty file,
    // any byte prefix, or the complete header. Foreign entries stay untouched.
    if !header.starts_with(&bytes) {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let current = match io.metadata(candidate) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !same_creation_file(&metadata, &current) {
            return Ok(());
        }
        // A path replacement between this final identity check and unlink is
        // still possible; callers must keep the journal directory trusted.
    }
    match io.remove(candidate) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn refuse_creation_temp_name(path: &Path) -> io::Result<()> {
    if JournalMedia::is_creation_temp_name(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "journal path {} uses a reserved creation-temporary name",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn canonical_journal_path(path: &Path) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "journal path is a dangling symlink",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            // This is the expected canonical name for post-open verification,
            // not an additional locking mechanism for parent-directory aliases.
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "journal path must have a filename",
                )
            })?;
            Ok(parent.canonicalize()?.join(name))
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn platform_sync(file: &File) -> io::Result<()> {
    // std uses F_FULLFSYNC on Apple targets with an Interrupted retry. Calling
    // fcntl again would duplicate the device-cache flush for every command.
    file.sync_all()
}

#[cfg(not(unix))]
fn platform_sync(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "journal durability is supported only on Unix",
    ))
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;
