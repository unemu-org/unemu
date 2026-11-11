// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline floppy/CMOS journal inspection and backed-up mutation, as specified in
//! `backlog/docs/design/persistent-media.md` § Offline commands and § Locking
//! and retention. Inspections may report an absent lock on read-only storage.

use crate::sink::{
    canonical_journal_path, inspection_lock, journal_lock_path, lock_journal, platform_sync,
    refuse_creation_temp_name,
};
use crate::{scan_reader, JournalMedia, OpenError, Refusal, Scan, Status, HEADER_LEN};
use std::fs::{File, OpenOptions, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{role} {}: {source}", path.display())]
    PathIo {
        role: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("backup {}: {source}; a partial, invalid backup was left at {}", path.display(), path.display())]
    PartialBackup { path: PathBuf, source: io::Error },
    #[error("journal verification refused: {0:?}")]
    Refused(Refusal),
    #[error("journal {}: {source}", path.display())]
    Recovery { path: PathBuf, source: OpenError },
}

fn path_error(role: &'static str, path: &Path, source: io::Error) -> ToolError {
    ToolError::PathIo {
        role,
        path: path.to_owned(),
        source,
    }
}

fn refuse_reserved_path(role: &str, path: &Path) -> Result<(), ToolError> {
    let resolved = canonical_journal_path(path).ok();
    if JournalMedia::is_creation_temp_name(path)
        || resolved
            .as_ref()
            .is_some_and(|p| JournalMedia::is_creation_temp_name(p))
    {
        return Err(ToolError::Config(format!(
            "{role} {} uses or resolves to a reserved journal temporary name",
            path.display()
        )));
    }
    Ok(())
}

/// Inspection opens journal data read-only; mutation requires an exclusive lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Inspect,
    Mutate,
}

/// An existing journal. A missing inspection lock is reported explicitly;
/// mutation always owns the stable exclusive lock until its final verification.
pub struct JournalTool {
    path: PathBuf,
    base: Vec<u8>,
    base_metadata: std::fs::Metadata,
    journal: File,
    lock: Option<File>,
    access: Access,
}

impl JournalTool {
    pub fn open(base: &Path, journal: &Path, access: Access) -> Result<Self, ToolError> {
        Self::open_with(&mut HostIo, base, journal, access)
    }

    fn open_with(
        io: &mut impl ToolIo,
        base: &Path,
        journal: &Path,
        access: Access,
    ) -> Result<Self, ToolError> {
        refuse_reserved_path("base", base)?;
        refuse_reserved_path("journal", journal)?;
        let journal_error = |error| path_error("journal", journal, error);
        refuse_creation_temp_name(journal).map_err(journal_error)?;
        let mut base_file = io
            .open_input(base, false)
            .map_err(|e| path_error("base", base, e))?;
        // Check both inputs before creating lock metadata, but reopen the journal
        // under the lock so a cooperating retention replacement is respected.
        drop(io.open_input(journal, false).map_err(journal_error)?);
        let path = canonical_journal_path(journal).map_err(journal_error)?;
        refuse_creation_temp_name(&path).map_err(journal_error)?;
        let lock_path = journal_lock_path(&path);
        refuse_reserved_path("lock", &lock_path)?;
        let lock_error = |error| path_error("lock", &lock_path, error);
        let lock = if access == Access::Inspect {
            inspection_lock(&path).map_err(lock_error)?
        } else {
            Some(lock_journal(&path, false).map_err(lock_error)?)
        };
        let file = io
            .open_input(&path, access == Access::Mutate)
            .map_err(journal_error)?;
        if io.canonicalize(&path).map_err(journal_error)? != path {
            return Err(journal_error(io::Error::other(
                "journal canonical path changed after locking",
            )));
        }
        let base_metadata = base_file
            .metadata()
            .map_err(|e| path_error("base", base, e))?;
        let mut bytes = Vec::new();
        base_file
            .read_to_end(&mut bytes)
            .map_err(|e| path_error("base", base, e))?;
        Ok(Self {
            path,
            base: bytes,
            base_metadata,
            journal: file,
            lock,
            access,
        })
    }

    pub fn has_lock(&self) -> bool {
        self.lock.is_some()
    }

    /// Replay and validate every frame without changing image or journal bytes.
    pub fn verify(&mut self) -> Result<Scan, ToolError> {
        scan_reader(&mut self.journal, &self.base, self.base.len() as u64)
            .map_err(|e| path_error("journal", &self.path, e))
    }

    /// Export one closed recovery point without changing either input. The shared
    /// lock stays held through verification and no-clobber publication.
    pub fn commit(&mut self, generation: u64, output: &Path) -> Result<(), ToolError> {
        self.commit_with(&mut HostIo, generation, output)
    }

    fn commit_with(
        &mut self,
        io: &mut impl ToolIo,
        generation: u64,
        output: &Path,
    ) -> Result<(), ToolError> {
        if self.lock.is_none() {
            return Err(ToolError::Config(
                "export requires a shared journal lock".into(),
            ));
        }
        let scan = self.validated()?;
        if scan.report.needs_recovery() {
            return Err(ToolError::Config(
                "export requires a sealed, valid journal tip".into(),
            ));
        }
        refuse_reserved_path("output", output)?;
        match std::fs::symlink_metadata(output) {
            Ok(_) => {
                return Err(ToolError::Config(
                    "output path must be unused (including input aliases)".into(),
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(path_error(
                    "export failed; nothing was published; output",
                    output,
                    e,
                ))
            }
        }
        let end = if generation == 0
            && scan
                .report
                .header
                .as_ref()
                .is_some_and(|h| h.anchor_generation == 0)
        {
            HEADER_LEN
        } else {
            scan.report
                .generations
                .iter()
                .find(|g| g.id == generation)
                .map(|g| g.end_offset)
                .ok_or_else(|| {
                    ToolError::Config(format!(
                        "generation {generation} is not a retained closed generation"
                    ))
                })?
        };
        let image = crate::history::prefix(&mut self.journal, end, &self.base)
            .map_err(|e| path_error("journal", &self.path, e))?
            .image
            .ok_or_else(|| ToolError::Config("generation reconstruction refused".into()))?;
        let (mut temp, name) = io
            .create_temporary(output, "export", None)
            .map_err(|e| path_error("export failed; nothing was published; output", output, e))?;
        let mut publishing = false;
        let mut published = false;
        let result = (|| {
            io.write_temporary(&mut temp, &image)?;
            temp.seek(SeekFrom::Start(0))?;
            let mut check = Vec::new();
            temp.read_to_end(&mut check)?;
            if check.len() != image.len()
                || crate::codec::digest(&check) != crate::codec::digest(&image)
            {
                return Err(io::Error::other("export size/digest verification failed"));
            }
            io.sync(&temp, "temporary sync")?;
            publishing = true;
            io.publish_temporary(&name, output, false)?;
            published = true;
            io.sync(&File::open(parent(output))?, "publication directory sync")
        })();
        // Only this operation's create_new file is ours to unlink. Orphans from
        // interrupted exports are never mounted or cleaned by journal recovery.
        let _ = std::fs::remove_file(&name);
        result.map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                ToolError::Config("output path must be unused (including input aliases)".into())
            } else {
                let role = if publishing && !published && hard_link_unsupported(&e) {
                    "export needs hard-link support on the output filesystem; nothing was published; output"
                } else if publishing {
                    "export publication uncertain; output"
                } else {
                    "export failed; nothing was published; output"
                };
                path_error(role, output, e)
            }
        })
    }

    /// Back up before applying precisely the writable mount's recovery. Dropping
    /// the recovered session avoids adding an unrelated empty session end.
    pub fn repair_tail(self, backup: &Path) -> Result<Scan, ToolError> {
        self.repair_with(&mut HostIo, backup)
    }

    fn repair_with(mut self, io: &mut impl ToolIo, backup: &Path) -> Result<Scan, ToolError> {
        self.require_mutation()?;
        self.validated()?;
        self.backup(io, backup)?;
        let lock = self
            .lock
            .take()
            .ok_or_else(|| ToolError::Config("mutation requires an exclusive lock".into()))?;
        let mounted = io
            .recover(&self.path, &self.base, lock, &self.base_metadata)
            .map_err(|source| ToolError::Recovery {
                path: self.path.clone(),
                source,
            })?;
        // A concurrent writer must not change the recovered state during this scan.
        let scan = self.verify()?;
        drop(mounted);
        Ok(scan)
    }

    /// Validate before a frontend announces the destructive operation. The lock
    /// remains held, and rollback validates again before making its backup.
    pub fn validate_rollback(&mut self, generation: u64) -> Result<(), ToolError> {
        self.rollback_end(generation).map(|_| ())
    }

    pub fn rollback(&mut self, generation: u64, backup: &Path) -> Result<Scan, ToolError> {
        self.rollback_with(&mut HostIo, generation, backup)
    }

    fn rollback_with(
        &mut self,
        io: &mut impl ToolIo,
        generation: u64,
        backup: &Path,
    ) -> Result<Scan, ToolError> {
        let end = self.rollback_end(generation)?;
        self.backup(io, backup)?;
        io.truncate(&self.journal, end as u64)
            .map_err(|e| path_error("journal", &self.path, e))?;
        io.sync(&self.journal, "journal sync")
            .map_err(|e| path_error("journal", &self.path, e))?;
        self.verify()
    }

    fn rollback_end(&mut self, generation: u64) -> Result<usize, ToolError> {
        self.require_mutation()?;
        let scan = self.validated()?;
        if generation == 0
            && scan
                .report
                .header
                .as_ref()
                .is_some_and(|h| h.anchor_generation == 0)
        {
            return Ok(HEADER_LEN);
        }
        scan.report
            .generations
            .iter()
            .find(|g| g.id == generation)
            .map(|g| g.end_offset)
            .ok_or_else(|| {
                ToolError::Config(format!(
                    "generation {generation} is not a retained closed generation"
                ))
            })
    }

    fn require_mutation(&self) -> Result<(), ToolError> {
        if self.access != Access::Mutate || self.lock.is_none() {
            return Err(ToolError::Config(
                "mutation requires an exclusive lock".into(),
            ));
        }
        Ok(())
    }

    fn validated(&mut self) -> Result<Scan, ToolError> {
        let scan = self.verify()?;
        if let Status::Refused(ref refusal) = scan.report.status {
            return Err(ToolError::Refused(refusal.clone()));
        }
        Ok(scan)
    }

    fn backup(&mut self, io: &mut impl ToolIo, path: &Path) -> Result<(), ToolError> {
        // Mount cleanup must never mistake a header-only backup for an orphan.
        refuse_reserved_path("backup", path)?;
        let permissions = self
            .journal
            .metadata()
            .map_err(|e| path_error("journal", &self.path, e))?
            .permissions();
        let backup = match io.create_backup(path, &permissions) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(ToolError::Config(format!(
                    "backup {}: path must be unused (including aliases of the base or journal)",
                    path.display()
                )));
            }
            Err(error) => return Err(path_error("backup", path, error)),
        };
        let result = (|| {
            // Creation applies the mode before publication; restoring it exactly
            // also preserves restrictive bits removed by the caller's umask.
            #[cfg(unix)]
            backup.set_permissions(permissions)?;
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let mut host = HostBackup {
                journal: &mut self.journal,
                backup,
                directory: File::open(parent)?,
                io,
            };
            host.copy()?;
            host.sync_file()?;
            host.sync_directory()
        })();
        result.map_err(|source| ToolError::PartialBackup {
            path: path.to_owned(),
            source,
        })
    }
}

/// Actual offline filesystem operations, including barriers. Recording tests
/// delegate to HostIo so production HostBackup calls cannot bypass their trace.
/// See persistent-media.md § Offline commands and § Locking and retention.
pub(crate) trait ToolIo {
    fn create_temporary(
        &mut self,
        path: &Path,
        marker: &str,
        permissions: Option<&Permissions>,
    ) -> io::Result<(File, PathBuf)> {
        for counter in 0..u64::MAX {
            let mut name = path.as_os_str().to_os_string();
            name.push(format!(".{marker}-{counter}.tmp"));
            let name = PathBuf::from(name);
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                options.mode(permissions.map_or(0o600, |p| p.mode() & 0o7777));
            }
            #[cfg(not(unix))]
            let _ = permissions;
            match options.open(&name) {
                Ok(file) => return Ok((file, name)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("temporary names exhausted"))
    }
    fn set_permissions(&mut self, file: &File, permissions: Permissions) -> io::Result<()> {
        file.set_permissions(permissions)
    }
    fn write_temporary(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        file.write_all(bytes)
    }
    fn publish_temporary(
        &mut self,
        temporary: &Path,
        path: &Path,
        replace: bool,
    ) -> io::Result<()> {
        if replace {
            std::fs::rename(temporary, path)
        } else {
            std::fs::hard_link(temporary, path)
        }
    }
    fn open_input(&mut self, path: &Path, write: bool) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.read(true).write(write);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("input must be a regular file"));
        }
        Ok(file)
    }
    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        path.canonicalize()
    }
    fn create_backup(&mut self, path: &Path, permissions: &Permissions) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(permissions.mode() & 0o7777);
        }
        #[cfg(not(unix))]
        let _ = permissions;
        options.open(path)
    }
    fn copy(&mut self, journal: &mut File, backup: &mut File) -> io::Result<()> {
        journal.seek(SeekFrom::Start(0))?;
        io::copy(journal, backup)?;
        Ok(())
    }
    fn sync(&mut self, file: &File, _role: &'static str) -> io::Result<()> {
        platform_sync(file)
    }
    fn truncate(&mut self, file: &File, end: u64) -> io::Result<()> {
        file.set_len(end)
    }
    fn recover(
        &mut self,
        path: &Path,
        base: &[u8],
        lock: File,
        base_metadata: &std::fs::Metadata,
    ) -> Result<JournalMedia, OpenError> {
        JournalMedia::recover_locked(path, base, lock, base_metadata)
    }
}
pub(crate) struct HostIo;
impl ToolIo for HostIo {}

fn hard_link_unsupported(error: &io::Error) -> bool {
    #[cfg(unix)]
    return matches!(
        error.raw_os_error(),
        Some(libc::EPERM) | Some(libc::ENOTSUP) | Some(libc::EXDEV)
    );
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

pub(crate) fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

struct HostBackup<'a, I> {
    journal: &'a mut File,
    backup: File,
    directory: File,
    io: &'a mut I,
}
impl<I: ToolIo> HostBackup<'_, I> {
    fn copy(&mut self) -> io::Result<()> {
        self.io.copy(self.journal, &mut self.backup)
    }
    fn sync_file(&mut self) -> io::Result<()> {
        self.io.sync(&self.backup, "backup file sync")
    }
    fn sync_directory(&mut self) -> io::Result<()> {
        self.io.sync(&self.directory, "backup directory sync")
    }
}

#[cfg(test)]
#[path = "offline_tests.rs"]
mod tests;
