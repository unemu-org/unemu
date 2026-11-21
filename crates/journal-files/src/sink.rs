// SPDX-License-Identifier: AGPL-3.0-or-later
//! File constructors and fault injection for the record-backed journal lifecycle; persistent-media.md § Persist at command
//! completion, § Open and tail recovery and § Host errors and determinism.
//!
//! This is a disk-format service, not controller hardware. Assembly can adapt its
//! complete-sector batches to `device::SectorMedia` without exposing this status
//! channel to the controller. Reads consult sparse replacements, then the verified base.
//! No wall clock, worker thread or host failure becomes a guest input.
//!
//! The journal is kept in one record of the contract (`unemu_core_api::Record`,
//! core-contract.md §12.5): it reads the record, appends the frames of a
//! command and synchronizes once per command/end, cuts an invalid tail at mount,
//! creates a new journal by committing a replacement that holds only the header,
//! and rewrites retained history through a verified replacement. It opens no
//! file. The journal file on the host, with its stable `FILE.lock`, its
//! no-clobber header publication and its temporary-name namespaces, is
//! `crate::journal_file`. Tests measure operation ordering, fault handling and
//! real file reopen/lock behavior; they do not measure power-loss durability.
//! Preservation of synchronized bytes and honest storage/cache barriers remain
//! assumptions of the design.

#[cfg(test)]
use crate::journal_file::{
    barrier, canonical_journal_path, platform_sync, Filesystem, HostFilesystem, JournalIo,
};
use crate::journal_file::{
    journal_error, journal_replacement, journal_stage, resolve_journal_path, FileRecord,
    ToolFilesystem,
};
use crate::offline::ToolIo;
#[cfg(test)]
use crate::temporary::{cleanup_creation_temp, cleanup_retention_temp, HostCreationTempIo};
use crate::{ClosingReason, Error, SectorSource, SparseOverlay, VerifiedBase};
#[cfg(test)]
use crate::{Frame, Header, Record, Status, SECTOR_LEN};
use contract::Record as _;
#[cfg(test)]
use std::any::Any;
use std::cell::RefCell;
#[cfg(any(test, feature = "test-support"))]
use std::fs::File;
#[cfg(test)]
use std::fs::OpenOptions;
#[cfg(any(test, feature = "test-support"))]
use std::io;
#[cfg(test)]
use std::io::{Read, Seek, SeekFrom, Write};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::rc::Rc;
use unemu_core_api as contract;

pub use journal::{MaintenanceFailure, OpenError, SinkFailure, SinkStatus};

/// A record-backed journal with the host's path constructors and retention seam.
pub struct JournalMedia {
    inner: journal::JournalMedia,
    file: Option<Rc<RefCell<FileRecord>>>,
    path: std::path::PathBuf,
    #[cfg(feature = "test-support")]
    retention_failure: Option<&'static str>,
}

impl Deref for JournalMedia {
    type Target = journal::JournalMedia;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for JournalMedia {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl JournalMedia {
    fn wrap(inner: journal::JournalMedia, file: Option<Rc<RefCell<FileRecord>>>) -> Self {
        Self {
            inner,
            file,
            path: Default::default(),
            #[cfg(feature = "test-support")]
            retention_failure: None,
        }
    }
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

    /// Retention owns a separate namespace; creation cleanup must never unlink it.
    pub fn is_retention_temp(journal: &Path, candidate: &Path) -> bool {
        if journal.parent() != candidate.parent() {
            return false;
        }
        let (Some(journal), Some(candidate)) = (journal.file_name(), candidate.file_name()) else {
            return false;
        };
        let mut prefix = journal.to_os_string();
        prefix.push(".retain-");
        candidate
            .as_encoded_bytes()
            .strip_prefix(prefix.as_encoded_bytes())
            .and_then(|s| s.strip_suffix(b".tmp"))
            .is_some_and(|s| !s.is_empty() && s.iter().all(u8::is_ascii_digit))
    }

    /// Reserved retention names cannot be mounted as an input or journal.
    ///
    /// `pc-config`'s lowering keeps a copy of this rule
    /// (`has_retention_marker`) so that it needs no journal; the test
    /// `the_lowering_reserves_the_journal_retention_names` in
    /// `machine-file/src/paths.rs` holds the two equal. Change both together.
    pub fn is_retention_temp_name(path: &Path) -> bool {
        path.file_name().is_some_and(|name| {
            name.as_encoded_bytes()
                .windows(8)
                .any(|part| part == b".retain-")
        })
    }

    /// Set the positive assembly retention policy before executing the session.
    pub fn with_retention(mut self, keep_generations: u32) -> Self {
        self.inner = self.inner.with_retention(keep_generations);
        self
    }

    /// Whether a filename contains a reserved temporary marker. Rejecting it
    /// prevents nested journals from owning overlapping temporary namespaces.
    pub fn is_creation_temp_name(path: &Path) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let name = name.as_encoded_bytes();
        [b".create-".as_slice(), b".retain-", b".export-"]
            .iter()
            .any(|marker| name.windows(marker.len()).any(|part| part == *marker))
    }

    /// Resolve symlinks and acquire a stable `<journal>.lock` before touching
    /// journal contents, then verify the canonical path again after opening.
    /// A missing journal's resolved parent forms the expected canonical path for
    /// that comparison. Never remove the sidecar: retention uses the same lock.
    /// The base is the exact already-loaded immutable image, not a path to reread.
    /// File-backed callers use `open_with_base_metadata` to protect input aliases.
    pub fn open(
        path: &Path,
        base: &[u8],
        geometry_length: u64,
        status: SinkStatus,
    ) -> Result<Self, OpenError> {
        Self::open_host(
            path,
            &VerifiedBase::from_bytes(base)?,
            geometry_length,
            status,
            None,
        )
    }

    /// Protect the identity of the file read to obtain these exact base bytes.
    /// Cleanup must not unlink that file even through a reserved-name hard link.
    pub fn open_with_base_metadata(
        path: &Path,
        base: &[u8],
        geometry_length: u64,
        status: SinkStatus,
        base_metadata: &std::fs::Metadata,
    ) -> Result<Self, OpenError> {
        Self::open_host(
            path,
            &VerifiedBase::from_bytes(base)?,
            geometry_length,
            status,
            Some(base_metadata),
        )
    }

    /// Mount a once-verified read-only file without loading its bytes again.
    pub fn open_verified(
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
    ) -> Result<Self, OpenError> {
        Self::open_host(path, base, geometry_length, status, base.metadata())
    }

    /// Preserve a caller's input alias protection for in-memory bases too.
    pub fn open_verified_with_metadata(
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, OpenError> {
        Self::open_host(
            path,
            base,
            geometry_length,
            status,
            metadata.or(base.metadata()),
        )
    }

    fn open_host(
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, OpenError> {
        crate::check_journal_path(path).map_err(OpenError::Usage)?;
        base.check_unchanged().map_err(|e| {
            OpenError::Host(latch(
                &status,
                SinkFailure {
                    operation: "verify base",
                    message: e.to_string(),
                    last_confirmed_sequence: 0,
                },
            ))
        })?;
        let path = resolve_journal_path(path).map_err(|error| {
            OpenError::Host(latch(
                &status,
                SinkFailure {
                    operation: "resolve journal path",
                    message: error.to_string(),
                    last_confirmed_sequence: 0,
                },
            ))
        })?;
        Self::open_path(&path, base, geometry_length, status, None, base_metadata)
    }

    fn open_path(
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        lock: Option<unemu_core_files::PathLock>,
        metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, OpenError> {
        Self::open_path_with(
            path,
            base,
            geometry_length,
            status,
            lock,
            metadata,
            Rc::new(crate::journal_file::JournalFilesystem),
        )
    }

    fn open_path_with(
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        lock: Option<unemu_core_files::PathLock>,
        metadata: Option<&std::fs::Metadata>,
        fs: Rc<dyn unemu_core_files::Filesystem>,
    ) -> Result<Self, OpenError> {
        Self::check_open(base, geometry_length, &status)?;
        let header = base.identity().encode()?;
        let mut record =
            FileRecord::open_with(fs, path, lock, &header, metadata).map_err(|error| {
                OpenError::Host(latch(
                    &status,
                    SinkFailure {
                        operation: journal_stage(error.stage),
                        message: error.source.to_string(),
                        last_confirmed_sequence: 0,
                    },
                ))
            })?;
        let created = !record.exists();
        if created {
            let host = |operation, error: contract::Error| {
                let operation = crate::journal_file::creation_stage(&error, operation);
                OpenError::Host(latch(
                    &status,
                    SinkFailure {
                        operation,
                        message: journal_error(error).message,
                        last_confirmed_sequence: 0,
                    },
                ))
            };
            let mut replacement = record
                .replace()
                .map_err(|error| host("create temporary header", error))?;
            replacement
                .append(&header)
                .map_err(|error| host("header write", error))?;
            replacement
                .commit()
                .map_err(|error| host("publish header", error))?;
        }
        let mut media = Self::mount_file(record, base, geometry_length, status, created)?;
        media.path = path.to_owned();
        Ok(media)
    }

    fn mount_file(
        record: FileRecord,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        created: bool,
    ) -> Result<Self, OpenError> {
        let record = Rc::new(RefCell::new(record));
        let inner = journal::JournalMedia::mount(
            Box::new(SharedRecord(record.clone())),
            base.core(),
            geometry_length,
            status,
            created,
        )?;
        Ok(Self::wrap(inner, Some(record)))
    }

    /// Mount any exclusive contract record, with byte handling in `journal`.
    pub fn open_record(
        record: Box<dyn contract::Record>,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
    ) -> Result<Self, OpenError> {
        journal::JournalMedia::open_record(record, base.core(), geometry_length, status)
            .map(|inner| Self::wrap(inner, None))
    }

    /// Inject a failing output for consumer boundary tests.
    #[cfg(feature = "test-support")]
    pub fn with_failing_output(mut self) -> Self {
        self.inner = self.inner.with_failing_output();
        self
    }

    /// Inject a retention failure at one publication boundary for consumer tests.
    /// The generation close and all command barriers remain real.
    #[cfg(feature = "test-support")]
    pub fn with_failing_retention(mut self, operation: &'static str) -> Self {
        self.retention_failure = Some(operation);
        self
    }

    #[cfg(test)]
    fn open_with(
        fs: &mut impl Filesystem,
        path: &Path,
        base: &[u8],
        geometry_length: u64,
        status: SinkStatus,
    ) -> Result<Self, OpenError> {
        Self::open_file(
            fs,
            path,
            &VerifiedBase::from_bytes(base)?,
            geometry_length,
            status,
            None,
        )
    }

    pub(crate) fn recover_locked(
        path: &Path,
        base: &VerifiedBase,
        lock: unemu_core_files::PathLock,
        base_metadata: &std::fs::Metadata,
    ) -> Result<Self, OpenError> {
        Self::open_path(
            path,
            base,
            base.byte_len(),
            SinkStatus::default(),
            Some(lock),
            Some(base_metadata),
        )
    }

    /// Refuse a mount on a latched status or a base of another length, before
    /// any storage is touched.
    fn check_open(
        base: &VerifiedBase,
        geometry_length: u64,
        status: &SinkStatus,
    ) -> Result<(), OpenError> {
        if let Some(failure) = status.failure() {
            return Err(failure.into());
        }
        if base.identity().base_length != geometry_length {
            return Err(Error::GeometryLength.into());
        }
        Ok(())
    }

    /// Lock and open the journal file, create it when it is missing, then
    /// mount it.
    #[cfg(test)]
    fn open_file(
        fs: &mut dyn crate::journal_file::Filesystem,
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, OpenError> {
        Self::check_open(base, geometry_length, &status)?;
        let header = base.identity().encode()?;
        let host = |(operation, error): (&'static str, io::Error)| {
            OpenError::Host(latch(
                &status,
                SinkFailure {
                    operation,
                    message: error.to_string(),
                    last_confirmed_sequence: 0,
                },
            ))
        };
        let (record, created) =
            crate::journal_file::tests::open(fs, path, &header, base_metadata).map_err(host)?;
        let mut media = Self::mount_file(record, base, geometry_length, status, created)?;
        media.path = path.to_owned();
        Ok(media)
    }

    /// Caller must be at a completed machine service boundary. Empty generations
    /// are intentional recovery points; no automatic checkpoint cadence exists.
    pub fn checkpoint(&mut self) -> Result<(), SinkFailure> {
        self.close_generation(ClosingReason::Checkpoint)
    }

    /// Explicit successful session termination. On execution failure, drop the
    /// media instead so recovery can identify its commands as interrupted.
    pub fn finish(self) -> Result<(), SinkFailure> {
        self.finish_overlay().map(|_| ())
    }

    /// Release the writer and lock while moving readable media to the owner.
    pub fn finish_overlay(self) -> Result<SparseOverlay, SinkFailure> {
        let (overlay, result) = self.finish_readable();
        result.map(|()| overlay)
    }

    /// Preserve readable completed writes even when the closing barrier fails.
    pub fn finish_readable(mut self) -> (SparseOverlay, Result<(), SinkFailure>) {
        let result = self.close_generation(ClosingReason::SessionEnd);
        (self.inner.into_overlay(), result)
    }

    fn close_generation(&mut self, reason: ClosingReason) -> Result<(), SinkFailure> {
        #[cfg(feature = "test-support")]
        if let Some(operation) = self.retention_failure {
            return self.close_with(reason, Rc::new(RefCell::new(FailingRetentionIo(operation))));
        }
        self.close_with(reason, Rc::new(RefCell::new(crate::offline::HostIo)))
    }

    fn close_with(
        &mut self,
        reason: ClosingReason,
        io: Rc<RefCell<dyn ToolIo>>,
    ) -> Result<(), SinkFailure> {
        let file = self.file.clone();
        let fs = ToolFilesystem::new(io, self.path.clone());
        self.inner.close_with(reason, &mut |record| match &file {
            Some(file) => journal_replacement(file.borrow_mut().replace_with(&fs)),
            None => record.replace(),
        })
    }
    #[cfg(test)]
    fn prune_with(&mut self, io: Rc<RefCell<dyn ToolIo>>) -> Result<(), SinkFailure> {
        let file = self.file.clone();
        let fs = ToolFilesystem::new(io, self.path.clone());
        self.inner.prune_with(&mut |record| match &file {
            Some(file) => journal_replacement(file.borrow_mut().replace_with(&fs)),
            None => record.replace(),
        })
    }
}

fn latch(status: &SinkStatus, failure: SinkFailure) -> SinkFailure {
    status.latch_host_error(failure.operation, failure.message.clone());
    status.failure().unwrap_or(failure)
}

struct SharedRecord(Rc<RefCell<FileRecord>>);
impl contract::Record for SharedRecord {
    fn size(&self) -> Result<u64, contract::Error> {
        self.0.borrow().size().map_err(journal_error)
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<(), contract::Error> {
        self.0
            .borrow_mut()
            .read_at(offset, bytes)
            .map_err(journal_error)
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), contract::Error> {
        self.0.borrow_mut().append(bytes).map_err(journal_error)
    }
    fn truncate(&mut self, size: u64) -> Result<(), contract::Error> {
        self.0.borrow_mut().truncate(size).map_err(journal_error)
    }
    fn sync(&mut self) -> Result<(), contract::Error> {
        self.0.borrow_mut().sync().map_err(journal_error)
    }
    fn replace(&mut self) -> Result<Box<dyn contract::Replacement>, contract::Error> {
        journal_replacement(self.0.borrow_mut().replace())
    }
}

#[cfg(feature = "test-support")]
struct FailingRetentionIo(&'static str);

#[cfg(feature = "test-support")]
impl ToolIo for FailingRetentionIo {
    fn write_temporary(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        if self.0 == "temporary write" {
            return Err(io::Error::other("injected retention write failure"));
        }
        std::io::Write::write_all(file, bytes)
    }
    fn sync(&mut self, file: &File, role: &'static str) -> io::Result<()> {
        if self.0 == role {
            return Err(io::Error::other("injected retention barrier failure"));
        }
        crate::journal_file::platform_sync(file)
    }
    fn publish_temporary(
        &mut self,
        temporary: &Path,
        path: &Path,
        replace: bool,
    ) -> io::Result<()> {
        if self.0 == "replace" {
            return Err(io::Error::other("injected retention publication failure"));
        }
        crate::offline::HostIo.publish_temporary(temporary, path, replace)
    }
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;
