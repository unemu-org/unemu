// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side durable journal lifecycle; persistent-media.md § Persist at command
//! completion, § Open and tail recovery and § Host errors and determinism.
//!
//! This is a disk-format service, not controller hardware. Assembly can adapt its
//! complete-sector batches to `device::SectorMedia` without exposing this status
//! channel to the controller. Reads consult sparse replacements, then the verified base.
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

use crate::temporary::{cleanup_creation_temp, cleanup_retention_temp, HostCreationTempIo};
use crate::{
    scan_overlay_reader, ClosingReason, Error, Frame, Header, Record, Refusal, SectorSource,
    SparseOverlay, Status, VerificationReport, VerifiedBase, FRAME_LEN, SECTOR_LEN,
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
    output: Box<dyn JournalIo>,
    _lock: Box<dyn Any>,
    status: SinkStatus,
    sequence: u64,
    generation: u64,
    confirmed: u64,
    mount_report: VerificationReport,
    base: VerifiedBase,
    path: PathBuf,
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
        Self::open_with_lock(
            &mut HostFilesystem,
            &path,
            base,
            geometry_length,
            status,
            None,
            base_metadata,
        )
    }

    /// Inject a failing write sink after a successful real mount. This optional
    /// test seam lets consumers verify their boundary policy without process-wide
    /// resource limits or filesystem-specific quotas. Memory updates stay real.
    #[cfg(feature = "test-support")]
    pub fn with_failing_output(mut self) -> Self {
        self.output = Box::new(FailingOutput(self.output));
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
        Self::open_with_lock(
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
        lock: File,
        base_metadata: &std::fs::Metadata,
    ) -> Result<Self, OpenError> {
        Self::open_with_lock(
            &mut HostFilesystem,
            path,
            base,
            base.byte_len(),
            SinkStatus::default(),
            Some(Box::new(lock)),
            Some(base_metadata),
        )
    }

    fn open_with_lock(
        fs: &mut impl Filesystem,
        path: &Path,
        base: &VerifiedBase,
        geometry_length: u64,
        status: SinkStatus,
        lock: Option<Box<dyn Any>>,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, OpenError> {
        if let Some(failure) = status.failure() {
            return Err(failure.into());
        }
        let header = base.identity().clone();
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
        let lock = match lock {
            Some(lock) => lock,
            None => fs.lock(path).map_err(|e| host("lock", e))?,
        };
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
        fs.cleanup_creation_temps(path, &header.encode()?, base_metadata)
            .map_err(|e| host("remove creation temporaries", e))?;
        // Cover both a prior publication and removal of its leftover hard link.
        fs.sync_directory(path)
            .map_err(|e| host("directory sync", e))?;
        let scan =
            scan_overlay_reader(&mut output, base, geometry_length).map_err(|e| host("scan", e))?;
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
            closed_count: scan.report.generations.len()
                + usize::from(scan.report.unsealed_tip.is_some()),
            mount_report: scan.report,
            base: base.clone(),
            path: path.to_owned(),
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
        io: &mut impl crate::offline::ToolIo,
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

    fn prune_with(&mut self, io: &mut impl crate::offline::ToolIo) -> Result<(), SinkFailure> {
        if self.status.failure().is_some() {
            return Ok(());
        }
        let mut uncertain = false;
        let mut temporary = None;
        let result = (|| -> io::Result<()> {
            let old = scan_overlay_reader(&mut self.output, &self.base, self.base.byte_len())?;
            if old.report.status != Status::Valid || old.report.unsealed_tip.is_some() {
                return Err(io::Error::other(
                    "retention requires a valid closed boundary",
                ));
            }
            drop(old.overlay);
            let retained = &old.report.generations
                [old.report.generations.len() - self.keep_generations as usize..];
            let anchor = &retained[0];
            let state = crate::history::prefix(&mut self.output, anchor.end_offset, &self.base)?
                .overlay
                .ok_or_else(|| io::Error::other("anchor reconstruction refused"))?;
            let permissions = std::fs::metadata(&self.path)?.permissions();
            let (mut replacement, name) =
                io.create_temporary(&self.path, "retain", Some(&permissions))?;
            temporary = Some(name.clone());
            // The requested creation mode is still filtered by the process umask.
            io.set_permissions(&replacement, permissions)?;
            let encode = |error| io::Error::other(error);
            io.write_temporary(
                &mut replacement,
                &Header {
                    anchor_generation: anchor.id,
                    ..self.base.identity().clone()
                }
                .encode()
                .map_err(encode)?,
            )?;
            let mut sequence = 0;
            let mut sectors = 0;
            let mut emit = |record, generation, file: &mut File| -> io::Result<()> {
                sequence += 1;
                io.write_temporary(
                    file,
                    &Frame {
                        sequence,
                        generation,
                        record,
                    }
                    .encode()
                    .map_err(encode)?,
                )
            };
            for (&lba, bytes) in state.replacements() {
                if **bytes != self.base.read_sector(lba)? {
                    emit(
                        Record::Sector {
                            lba,
                            bytes: bytes.clone(),
                        },
                        anchor.id,
                        &mut replacement,
                    )?;
                    sectors += 1;
                }
            }
            drop(state);
            emit(Record::CommandEnd { sectors }, anchor.id, &mut replacement)?;
            emit(
                Record::GenerationEnd {
                    reason: ClosingReason::RetentionAnchor,
                },
                anchor.id,
                &mut replacement,
            )?;
            let mut new_ends = vec![replacement.stream_position()? as usize];
            self.output
                .seek(SeekFrom::Start(anchor.end_offset as u64))?;
            let end = old.report.safe_offset;
            for _ in (anchor.end_offset..end).step_by(FRAME_LEN) {
                let mut bytes = [0; FRAME_LEN];
                self.output.read_exact(&mut bytes)?;
                let frame = Frame::decode(&bytes).map_err(encode)?;
                let closed = matches!(frame.record, Record::GenerationEnd { .. });
                emit(frame.record, frame.generation, &mut replacement)?;
                if closed {
                    new_ends.push(replacement.stream_position()? as usize);
                }
            }
            // Verify every retained recovery point, not only the final image:
            // otherwise an omitted overwritten sector could escape detection.
            for (generation, new_end) in retained.iter().zip(new_ends) {
                let before =
                    crate::history::prefix(&mut self.output, generation.end_offset, &self.base)?;
                let after = crate::history::prefix(&mut replacement, new_end, &self.base)?;
                if after.report.status != Status::Valid
                    || after.report.unsealed_tip.is_some()
                    || !equivalent(after.overlay.as_ref(), before.overlay.as_ref(), &self.base)?
                {
                    return Err(io::Error::other("retention state verification failed"));
                }
            }
            let check = scan_overlay_reader(&mut replacement, &self.base, self.base.byte_len())?;
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
            replacement.seek(SeekFrom::End(0))?;
            io.sync(&replacement, "temporary sync")?;
            uncertain = true;
            io.publish_temporary(&name, &self.path, true)?;
            self.output = Box::new(replacement);
            self.sequence = check.report.last_durable_sequence;
            self.confirmed = check
                .report
                .generations
                .iter()
                .rev()
                .find(|g| g.commands != 0)
                .map_or(0, |g| g.last_sequence - 1);
            self.closed_count = check.report.generations.len();
            io.sync(
                &File::open(crate::offline::parent(&self.path))?,
                "publication directory sync",
            )?;
            Ok(())
        })();
        if let Some(name) = temporary {
            let _ = std::fs::remove_file(name);
        }
        // Scans seek arbitrarily; even a failed rewrite must leave append at EOF.
        let seek = self.output.seek(SeekFrom::End(0));
        if seek.is_err() {
            uncertain = true;
        }
        match result.and(seek.map(|_| ())) {
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

#[cfg(feature = "test-support")]
struct FailingRetentionIo(&'static str);

#[cfg(feature = "test-support")]
impl crate::offline::ToolIo for FailingRetentionIo {
    fn write_temporary(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        if self.0 == "temporary write" {
            return Err(io::Error::other("injected retention write failure"));
        }
        file.write_all(bytes)
    }
    fn sync(&mut self, file: &File, role: &'static str) -> io::Result<()> {
        if self.0 == role {
            return Err(io::Error::other("injected retention barrier failure"));
        }
        platform_sync(file)
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

/// Namespace and lock operations are separate from the journal inode so the
/// lock remains stable across later atomic retention replacements.
trait Filesystem {
    fn lock(&mut self, path: &Path) -> io::Result<Box<dyn Any>>;
    fn open(&mut self, path: &Path) -> io::Result<Option<Box<dyn JournalIo>>>;
    fn verify_path(&mut self, path: &Path) -> io::Result<()>;
    fn cleanup_creation_temps(
        &mut self,
        path: &Path,
        header: &[u8],
        base_metadata: Option<&std::fs::Metadata>,
    ) -> io::Result<()>;
    fn create_temp(&mut self, path: &Path) -> io::Result<(Box<dyn JournalIo>, PathBuf)>;
    fn publish(&mut self, temporary: &Path, path: &Path) -> io::Result<()>;
    fn remove_temp(&mut self, temporary: &Path) -> io::Result<()>;
    fn sync_directory(&mut self, path: &Path) -> io::Result<()>;
}

struct ProtectedBaseIo<'a>(&'a std::fs::Metadata);
impl crate::temporary::CreationTempIo for ProtectedBaseIo<'_> {
    fn metadata(&mut self, path: &Path) -> io::Result<std::fs::Metadata> {
        HostCreationTempIo.metadata(path)
    }
    fn open(&mut self, path: &Path) -> io::Result<File> {
        HostCreationTempIo.open(path)
    }
    fn remove(&mut self, path: &Path) -> io::Result<()> {
        HostCreationTempIo.remove(path)
    }
    fn protected_base(&self) -> Option<&std::fs::Metadata> {
        Some(self.0)
    }
}

struct HostFilesystem;

impl Filesystem for HostFilesystem {
    fn lock(&mut self, path: &Path) -> io::Result<Box<dyn Any>> {
        Ok(Box::new(lock_journal(path, false)?))
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

    fn cleanup_creation_temps(
        &mut self,
        path: &Path,
        header: &[u8],
        base_metadata: Option<&std::fs::Metadata>,
    ) -> io::Result<()> {
        let parent = path.parent().unwrap_or(Path::new("."));
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let candidate = entry.path();
            let mut protected;
            let io: &mut dyn crate::temporary::CreationTempIo = if let Some(base) = base_metadata {
                protected = ProtectedBaseIo(base);
                &mut protected
            } else {
                &mut HostCreationTempIo
            };
            if JournalMedia::is_creation_temp(path, &candidate) {
                cleanup_creation_temp(io, &candidate, header)?;
            } else if JournalMedia::is_retention_temp(path, &candidate) {
                cleanup_retention_temp(io, &candidate)?;
            }
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

pub(crate) fn refuse_creation_temp_name(path: &Path) -> io::Result<()> {
    if JournalMedia::is_creation_temp_name(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "journal path {} uses a reserved creation-temporary name or replacement-temporary name",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(crate) fn canonical_journal_path(path: &Path) -> io::Result<PathBuf> {
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
pub(crate) fn platform_sync(file: &File) -> io::Result<()> {
    // std uses F_FULLFSYNC on Apple targets with an Interrupted retry. Calling
    // fcntl again would duplicate the device-cache flush for every command.
    file.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn platform_sync(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "journal durability is supported only on Unix",
    ))
}

pub(crate) fn journal_lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

pub(crate) fn inspection_lock(path: &Path) -> io::Result<Option<File>> {
    match lock_journal(path, true) {
        Ok(file) => Ok(Some(file)),
        Err(error) => {
            #[cfg(unix)]
            if matches!(
                error.raw_os_error(),
                Some(libc::EACCES) | Some(libc::EPERM) | Some(libc::EROFS)
            ) && matches!(std::fs::symlink_metadata(journal_lock_path(path)), Err(e) if e.kind() == io::ErrorKind::NotFound)
            {
                // A writing session must have created this entry. Never bypass
                // an existing lock, even if it is unreadable or a dangling link.
                return Ok(None);
            }
            Err(error)
        }
    }
}

pub(crate) fn lock_journal(path: &Path, shared: bool) -> io::Result<File> {
    let name = journal_lock_path(path);
    let file = if shared {
        match File::open(&name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&name)
                {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        File::open(&name)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(name)?
    };
    let result = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    result.map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "journal is locked by another session or tool",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;
