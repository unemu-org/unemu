// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side durable journal lifecycle; persistent-media.md § Persist at command
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
use crate::journal_file::{resolve_journal_path, FileRecord};
use crate::offline::ToolIo;
use crate::scan::scan_record;
use crate::store::{io_error, ContractRecord, Cursor, JournalRecord};
#[cfg(test)]
use crate::temporary::{cleanup_creation_temp, cleanup_retention_temp, HostCreationTempIo};
use crate::{
    scan_overlay_reader, ClosingReason, Error, Frame, Header, Record, Refusal, SectorSource,
    SparseOverlay, Status, VerificationReport, VerifiedBase, FRAME_LEN, SECTOR_LEN,
};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
#[cfg(any(test, feature = "test-support"))]
use std::fs::File;
#[cfg(test)]
use std::fs::OpenOptions;
use std::io;
#[cfg(test)]
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::rc::Rc;
use unemu_core_api as contract;

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
pub struct SinkStatus(
    Rc<RefCell<Option<SinkFailure>>>,
    Rc<RefCell<Option<MaintenanceFailure>>>,
);

/// Retention failure after the guest generation was already synchronized.
/// Uncertain publication requires a fresh mount before further host appends.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("retention maintenance {state}: {message}", state = if *.uncertain { "completion uncertain; reopen required" } else { "failed before publication" })]
pub struct MaintenanceFailure {
    pub uncertain: bool,
    pub message: String,
    pub last_confirmed_sequence: u64,
}

impl SinkStatus {
    /// Record an immutable source failure through the existing host-only latch.
    pub fn latch_host_error(&self, operation: &'static str, message: String) {
        self.latch(SinkFailure {
            operation,
            message,
            last_confirmed_sequence: 0,
        });
    }

    pub fn maintenance_failure(&self) -> Option<MaintenanceFailure> {
        self.1.borrow().clone()
    }

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

/// Sparse host media and its exclusive journal session.
///
/// The assembly adapter supplies in-range LBAs in controller transfer order.
/// Duplicate LBAs use their last bytes and last occurrence order; out-of-range
/// batches are ignored in full, matching the volatile media contract.
/// `finish` consumes the session; dropping without it only releases handles,
/// leaving complete commands to be recovered on the next mount. Retention runs
/// only after a synchronized close, under the same stable lock.
pub struct JournalMedia {
    image: SparseOverlay,
    record: Box<dyn JournalRecord>,
    status: SinkStatus,
    sequence: u64,
    generation: u64,
    confirmed: u64,
    mount_report: VerificationReport,
    base: VerifiedBase,
    keep_generations: u32,
    closed_count: usize,
    #[cfg(feature = "test-support")]
    retention_failure: Option<&'static str>,
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
        self.keep_generations = keep_generations.max(1);
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
        base.check_unchanged().map_err(|e| {
            OpenError::Host(status.latch(SinkFailure {
                operation: "verify base",
                message: e.to_string(),
                last_confirmed_sequence: 0,
            }))
        })?;
        let path = resolve_journal_path(path).map_err(|error| {
            OpenError::Host(status.latch(SinkFailure {
                operation: "resolve journal path",
                message: error.to_string(),
                last_confirmed_sequence: 0,
            }))
        })?;
        Self::open_file(
            &mut crate::journal_file::HostFilesystem,
            &path,
            base,
            geometry_length,
            status,
            None,
            base_metadata,
        )
    }

    /// Mount the journal kept in `record`, a record of the contract: a
    /// journal file ([`crate::JournalFile`]), a buffer in memory or any record
    /// a front end's storage hands over. An empty record becomes a new journal
    /// whose header is committed through a replacement, so a journal with half
    /// a header never exists; a record with bytes is recovered exactly as a
    /// journal file is. The record is held until the session ends.
    ///
    /// Host failures are latched in `status`, named by the journal's step
    /// ("scan", "mount sync", "write", ...) with the record's message.
    pub fn open_record(
        record: Box<dyn contract::Record>,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
    ) -> Result<Self, OpenError> {
        base.check_unchanged().map_err(|e| {
            OpenError::Host(status.latch(SinkFailure {
                operation: "verify base",
                message: e.to_string(),
                last_confirmed_sequence: 0,
            }))
        })?;
        Self::check_open(base, geometry_length, &status)?;
        let host = |operation, error: contract::Error| {
            OpenError::Host(status.latch(SinkFailure {
                operation,
                message: error.to_string(),
                last_confirmed_sequence: 0,
            }))
        };
        let mut record = ContractRecord(record);
        let created = contract::Record::size(&record).map_err(|e| host("open", e))? == 0;
        if created {
            let header = base.identity().encode()?;
            let mut replacement = contract::Record::replace(&mut record)
                .map_err(|e| host("create temporary header", e))?;
            replacement
                .append(&header)
                .map_err(|e| host("header write", e))?;
            replacement
                .commit()
                .map_err(|e| host("publish header", e))?;
        }
        Self::mount(Box::new(record), base, geometry_length, status, created)
    }

    /// Inject a failing write sink after a successful real mount. This optional
    /// test seam lets consumers verify their boundary policy without process-wide
    /// resource limits or filesystem-specific quotas. Memory updates stay real.
    #[cfg(feature = "test-support")]
    pub fn with_failing_output(mut self) -> Self {
        self.record = Box::new(FailingAppends(self.record));
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
            None,
        )
    }

    pub(crate) fn recover_locked(
        path: &Path,
        base: &VerifiedBase,
        lock: std::fs::File,
        base_metadata: &std::fs::Metadata,
    ) -> Result<Self, OpenError> {
        Self::open_file(
            &mut crate::journal_file::HostFilesystem,
            path,
            base,
            base.byte_len(),
            SinkStatus::default(),
            Some(Box::new(lock)),
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
    fn open_file(
        fs: &mut dyn crate::journal_file::Filesystem,
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        lock: Option<Box<dyn Any>>,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, OpenError> {
        Self::check_open(base, geometry_length, &status)?;
        let header = base.identity().encode()?;
        let host = |(operation, error): crate::journal_file::Step| {
            OpenError::Host(status.latch(SinkFailure {
                operation,
                message: error.to_string(),
                last_confirmed_sequence: 0,
            }))
        };
        let mut record =
            FileRecord::open(fs, path, lock, header.to_vec(), base_metadata).map_err(host)?;
        let created = !record.exists();
        if created {
            record.create(fs).map_err(host)?;
        }
        Self::mount(Box::new(record), base, geometry_length, status, created)
    }

    /// Recover the journal in `record` and make it ready for appends: replay
    /// it, cut an invalid tail, confirm the surviving history with a barrier
    /// of our own, and seal an interrupted session.
    fn mount(
        mut record: Box<dyn JournalRecord>,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        created: bool,
    ) -> Result<Self, OpenError> {
        let confirmed = Cell::new(0);
        let host = |operation, message: String| {
            OpenError::Host(status.latch(SinkFailure {
                operation,
                message,
                last_confirmed_sequence: confirmed.get(),
            }))
        };
        let scan = scan_record(record.as_mut(), base, geometry_length)
            .map_err(|e| host("scan", e.to_string()))?;
        let image = match scan.overlay {
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
            record
                .truncate(scan.report.safe_offset as u64)
                .map_err(|e| host("recovery truncate", e.to_string()))?;
            record
                .sync()
                .map_err(|e| host("recovery sync", e.to_string()))?;
        }
        if !created && scan.report.discarded_bytes == 0 {
            // Confirm surviving history with our own barrier; file contents alone
            // cannot reveal whether a previous process acknowledged its sync.
            record
                .sync()
                .map_err(|e| host("mount sync", e.to_string()))?;
        }
        confirmed.set(recovered_command);
        // Appends go to the end of the record, which is the safe offset now.
        // For a journal file this asks the host for the length, which also
        // places the file's cursor there.
        let end = record
            .size()
            .map_err(|e| host("append seek", e.to_string()))?;
        if end != scan.report.safe_offset as u64 {
            return Err(host(
                "append seek",
                format!(
                    "record holds {end} bytes after recovery, expected {}",
                    scan.report.safe_offset
                ),
            ));
        }
        if let Some(seal) = scan.report.recovery_seal()? {
            // Do not publish a marker that prevents any subsequent session id.
            seal.generation
                .checked_add(1)
                .ok_or(Error::IntegerExhausted)?;
            seal.sequence
                .checked_add(1)
                .ok_or(Error::IntegerExhausted)?;
            record
                .append(&seal.encode()?)
                .map_err(|e| host("recovery seal", e.to_string()))?;
            record
                .sync()
                .map_err(|e| host("recovery seal sync", e.to_string()))?;
            sequence = seal.sequence;
            closed = seal.generation;
        }
        let generation = closed.checked_add(1).ok_or(Error::IntegerExhausted)?;
        sequence.checked_add(1).ok_or(Error::IntegerExhausted)?;
        Ok(Self {
            image,
            record,
            status,
            sequence,
            generation,
            confirmed: confirmed.get(),
            closed_count: scan.report.generations.len()
                + usize::from(scan.report.unsealed_tip.is_some()),
            mount_report: scan.report,
            base: base.clone(),
            keep_generations: 10,
            #[cfg(feature = "test-support")]
            retention_failure: None,
        })
    }

    /// Original read-only recovery evidence, including discarded byte count and
    /// interrupted tip. Successful open has already performed these actions.
    pub fn mount_report(&self) -> &VerificationReport {
        &self.mount_report
    }

    pub fn status(&self) -> SinkStatus {
        self.check_base();
        self.status.clone()
    }

    /// Check host integrity at frontend boundaries even without a sector read.
    pub fn check_base(&self) {
        if let Err(error) = self.base.check_unchanged() {
            self.status.latch(self.failure("base changed", error));
        }
    }

    /// Explicit full-image capture for small tools/tests. Mount, finish and
    /// maintenance use sector access instead, so disk size does not set their RAM use.
    pub fn image(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for lba in 0..self.byte_len().div_ceil(SECTOR_LEN as u64) {
            bytes.extend_from_slice(&self.read_sector(lba));
        }
        bytes.truncate(self.byte_len() as usize);
        bytes
    }

    pub fn byte_len(&self) -> u64 {
        self.image.byte_len()
    }
    pub fn written_sectors(&self) -> usize {
        self.image.written_sectors()
    }

    pub fn read_sector(&self, lba: u64) -> [u8; SECTOR_LEN] {
        match self.image.read_sector(lba) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.status.latch(self.failure("base read", error));
                [0; SECTOR_LEN]
            }
        }
    }

    pub fn write_protected(&self) -> bool {
        false
    }

    fn in_range(&self, lba: u64) -> bool {
        lba < self.byte_len().div_ceil(SECTOR_LEN as u64)
    }

    /// Apply a completed nonempty command before attempting host output. The
    /// assembly adapter maps its sector records to these `(LBA, bytes)` pairs.
    /// Host failures have no result channel here and never undo memory writes.
    /// Malformed CMOS batches latch an error before touching memory or output.
    pub fn write_batch(&mut self, sectors: &[(u64, [u8; SECTOR_LEN])]) {
        if sectors.is_empty() || sectors.iter().any(|(lba, _)| !self.in_range(*lba)) {
            return;
        }
        if self.base.byte_len() == 64
            && sectors
                .iter()
                .any(|(_, bytes)| !crate::codec::valid_nvram_block(&self.read_sector(0), bytes))
        {
            self.status
                .latch(self.failure("write", "NVRAM immutable bytes or padding"));
            return;
        }
        if let Err(error) = self.base.check_unchanged() {
            self.status.latch(self.failure("base changed", error));
            return;
        }
        for (lba, bytes) in sectors {
            self.image.replace(*lba, Box::new(*bytes));
        }
        if self.status.failure().is_some() {
            return;
        }
        if let Some(failure) = self.status.maintenance_failure().filter(|f| f.uncertain) {
            self.status
                .latch(self.failure("journal appends disabled", failure));
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
        (self.image, result)
    }

    fn close_generation(&mut self, reason: ClosingReason) -> Result<(), SinkFailure> {
        #[cfg(feature = "test-support")]
        if let Some(operation) = self.retention_failure {
            return self.close_with(reason, &mut FailingRetentionIo(operation));
        }
        self.close_with(reason, &mut crate::offline::HostIo)
    }

    fn close_with(
        &mut self,
        reason: ClosingReason,
        io: &mut impl ToolIo,
    ) -> Result<(), SinkFailure> {
        if let Err(error) = self.base.check_unchanged() {
            return Err(self.status.latch(self.failure("base changed", error)));
        }
        if let Some(failure) = self.status.maintenance_failure().filter(|f| f.uncertain) {
            return Err(self.failure("retention", failure));
        }
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
        result.map_err(|failure| self.status.latch(failure))?;
        self.closed_count += 1;
        if self.closed_count > self.keep_generations as usize {
            let _ = self.prune_with(io);
        }
        if reason == ClosingReason::Checkpoint {
            if let Some(failure) = self.status.maintenance_failure().filter(|f| f.uncertain) {
                self.status
                    .latch(self.failure("journal appends disabled", failure));
            }
        }
        Ok(())
    }

    fn prune_with(&mut self, io: &mut impl ToolIo) -> Result<(), SinkFailure> {
        if self.status.failure().is_some() {
            return Ok(());
        }
        let mut uncertain = false;
        let result = (|| -> io::Result<()> {
            let old = scan_record(self.record.as_mut(), &self.base, self.base.byte_len())?;
            if old.report.status != Status::Valid || old.report.unsealed_tip.is_some() {
                return Err(io::Error::other(
                    "retention requires a valid closed boundary",
                ));
            }
            drop(old.overlay);
            let retained = &old.report.generations
                [old.report.generations.len() - self.keep_generations as usize..];
            let anchor = &retained[0];
            let state = crate::history::prefix(
                &mut Cursor::new(self.record.as_mut())?,
                anchor.end_offset,
                &self.base,
            )?
            .overlay
            .ok_or_else(|| io::Error::other("anchor reconstruction refused"))?;
            let mut replacement = self.record.replace_with(io).map_err(io_error)?;
            let encode = |error| io::Error::other(error);
            let new_header = Header {
                anchor_generation: anchor.id,
                ..self.base.identity().clone()
            }
            .encode()
            .map_err(encode)?;
            replacement.append(&new_header).map_err(io_error)?;
            let mut sequence = 0;
            let mut sectors = 0;
            let mut emit = |record,
                            generation,
                            replacement: &mut dyn contract::Replacement|
             -> io::Result<()> {
                sequence += 1;
                replacement
                    .append(
                        &Frame {
                            sequence,
                            generation,
                            record,
                        }
                        .encode()
                        .map_err(encode)?,
                    )
                    .map_err(io_error)
            };
            for (&lba, bytes) in state.replacements() {
                if **bytes != self.base.read_sector(lba)? {
                    emit(
                        Record::Sector {
                            lba,
                            bytes: bytes.clone(),
                        },
                        anchor.id,
                        replacement.as_mut(),
                    )?;
                    sectors += 1;
                }
            }
            drop(state);
            emit(
                Record::CommandEnd { sectors },
                anchor.id,
                replacement.as_mut(),
            )?;
            emit(
                Record::GenerationEnd {
                    reason: ClosingReason::RetentionAnchor,
                },
                anchor.id,
                replacement.as_mut(),
            )?;
            let mut new_ends = vec![replacement.size().map_err(io_error)? as usize];
            let end = old.report.safe_offset;
            for offset in (anchor.end_offset..end).step_by(FRAME_LEN) {
                let mut bytes = [0; FRAME_LEN];
                self.record
                    .read_at(offset as u64, &mut bytes)
                    .map_err(io_error)?;
                let frame = Frame::decode(&bytes).map_err(encode)?;
                let closed = matches!(frame.record, Record::GenerationEnd { .. });
                emit(frame.record, frame.generation, replacement.as_mut())?;
                if closed {
                    new_ends.push(replacement.size().map_err(io_error)? as usize);
                }
            }
            // Verify every retained recovery point, not only the final image:
            // otherwise an omitted overwritten sector could escape detection.
            for (generation, new_end) in retained.iter().zip(new_ends) {
                let before = crate::history::prefix(
                    &mut Cursor::new(self.record.as_mut())?,
                    generation.end_offset,
                    &self.base,
                )?;
                let after = crate::history::prefix(
                    &mut Cursor::new(replacement.as_mut())?,
                    new_end,
                    &self.base,
                )?;
                if after.report.status != Status::Valid
                    || after.report.unsealed_tip.is_some()
                    || !equivalent(after.overlay.as_ref(), before.overlay.as_ref(), &self.base)?
                {
                    return Err(io::Error::other("retention state verification failed"));
                }
            }
            let check = scan_overlay_reader(
                &mut Cursor::new(replacement.as_mut())?,
                &self.base,
                self.base.byte_len(),
            )?;
            if check.report.status != Status::Valid
                || check.report.unsealed_tip.is_some()
                || check.report.generations.len() != retained.len()
                || check
                    .report
                    .generations
                    .iter()
                    .zip(retained)
                    .any(|(a, b)| a.id != b.id || (a.id != anchor.id && a.reason != b.reason))
            {
                return Err(io::Error::other("retention history verification failed"));
            }
            drop(check.overlay);
            let new_length = replacement.size().map_err(io_error)?;
            // A commit that fails after its new contents are in place leaves the
            // record holding them: their numbers count from then on. Reads
            // still work after an uncertain failure, so the record itself
            // says which contents it holds: the new ones have the new length
            // and the new anchor in their header, which the old ones cannot
            // have, as the anchor is a generation the old history retained.
            let committed = replacement.commit();
            if let Err(error) = &committed {
                uncertain = error.kind == contract::ErrorKind::StorageUncertain;
                if !(uncertain && self.holds(new_length, &new_header)) {
                    return committed.map_err(io_error);
                }
            }
            self.sequence = check.report.last_durable_sequence;
            self.confirmed = check
                .report
                .generations
                .iter()
                .rev()
                .find(|g| g.commands != 0)
                .map_or(0, |g| g.last_sequence - 1);
            self.closed_count = check.report.generations.len();
            committed.map_err(io_error)
        })();
        // Scans read anywhere; even a failed rewrite must leave append at the
        // end. For a journal file this asks the host for the length.
        let seek = self.record.size();
        if seek.is_err() {
            uncertain = true;
        }
        match result.and(seek.map(|_| ()).map_err(io_error)) {
            Ok(()) => {
                *self.status.1.borrow_mut() = None;
                Ok(())
            }
            Err(error) => {
                let failure = MaintenanceFailure {
                    uncertain,
                    message: error.to_string(),
                    last_confirmed_sequence: self.confirmed,
                };
                *self.status.1.borrow_mut() = Some(failure.clone());
                let host = self.failure("retention", failure);
                Err(host)
            }
        }
    }

    /// Whether the record now holds `length` bytes that start with `header`.
    /// A record that cannot be read is taken to hold its old contents.
    fn holds(&mut self, length: u64, header: &[u8]) -> bool {
        let mut start = vec![0; header.len()];
        self.record.size().is_ok_and(|size| size == length)
            && self.record.read_at(0, &mut start).is_ok()
            && start == header
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
        self.record
            .append(&bytes)
            .map_err(|e| self.failure("write", e))?;
        self.sequence = sequence;
        Ok(())
    }

    fn sync(&mut self, command: bool) -> Result<(), SinkFailure> {
        self.record
            .sync()
            .map_err(|e| self.failure("command/end sync", e))?;
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

fn equivalent(
    a: Option<&SparseOverlay>,
    b: Option<&SparseOverlay>,
    base: &VerifiedBase,
) -> io::Result<bool> {
    let (Some(a), Some(b)) = (a, b) else {
        return Ok(false);
    };
    if a.same_replacements(b) {
        return Ok(true);
    }
    for (&lba, _) in a.replacements().chain(b.replacements()) {
        if a.read_sector(lba)? != b.read_sector(lba)? {
            return Ok(false);
        }
    }
    base.check_unchanged()?;
    Ok(true)
}

/// The test seam of `with_failing_output`: every append fails, as a host
/// write that runs out of space does; everything else reaches the record.
#[cfg(feature = "test-support")]
struct FailingAppends(Box<dyn JournalRecord>);

#[cfg(feature = "test-support")]
impl contract::Record for FailingAppends {
    fn size(&self) -> Result<u64, contract::Error> {
        self.0.size()
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error> {
        self.0.read_at(offset, buf)
    }
    fn append(&mut self, _bytes: &[u8]) -> Result<(), contract::Error> {
        Err(contract::Error::new(
            contract::ErrorKind::StorageUncertain,
            "injected journal output failure",
        ))
    }
    fn truncate(&mut self, size: u64) -> Result<(), contract::Error> {
        self.0.truncate(size)
    }
    fn sync(&mut self) -> Result<(), contract::Error> {
        self.0.sync()
    }
    fn replace(&mut self) -> Result<Box<dyn contract::Replacement>, contract::Error> {
        self.0.replace()
    }
}

#[cfg(feature = "test-support")]
impl JournalRecord for FailingAppends {
    fn replace_with<'t>(
        &mut self,
        tool: &'t mut dyn ToolIo,
    ) -> Result<Box<dyn contract::Replacement + 't>, contract::Error> {
        self.0.replace_with(tool)
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
