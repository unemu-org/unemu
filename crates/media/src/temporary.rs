// SPDX-License-Identifier: AGPL-3.0-or-later
//! Safe orphan removal under the stable journal lock; persistent-media.md
//! § Locking and retention. Namespace selection belongs to the caller.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

/// Host entry operations for creation and retention orphans; see
/// persistent-media.md § Locking and retention. Identity checks use the opened file.
pub(crate) trait CreationTempIo {
    fn metadata(&mut self, path: &Path) -> io::Result<std::fs::Metadata>;
    fn open(&mut self, path: &Path) -> io::Result<File>;
    fn remove(&mut self, path: &Path) -> io::Result<()>;
    fn protected_base(&self) -> Option<&std::fs::Metadata> {
        None
    }
}

pub(crate) struct HostCreationTempIo;

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

pub(crate) fn cleanup_creation_temp(
    io: &mut (impl CreationTempIo + ?Sized),
    candidate: &Path,
    header: &[u8],
) -> io::Result<()> {
    cleanup_temporary(io, candidate, Some(header))
}

pub(crate) fn cleanup_retention_temp(
    io: &mut (impl CreationTempIo + ?Sized),
    candidate: &Path,
) -> io::Result<()> {
    cleanup_temporary(io, candidate, None)
}

fn cleanup_temporary(
    io: &mut (impl CreationTempIo + ?Sized),
    candidate: &Path,
    header: Option<&[u8]>,
) -> io::Result<()> {
    let metadata = match io.metadata(candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() || header.is_some_and(|h| metadata.len() > h.len() as u64) {
        return Ok(());
    }
    let Ok(mut file) = io.open(candidate) else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        let opened = file.metadata()?;
        if !same_creation_file(&metadata, &opened)
            || io
                .protected_base()
                .is_some_and(|base| same_creation_file(base, &opened))
        {
            return Ok(());
        }
    }
    if let Some(header) = header {
        let mut bytes = Vec::new();
        if Read::by_ref(&mut file)
            .take(header.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Ok(());
        }
        // Creation writes only its header. Retention owns its entire reserved
        // namespace and can leave any prefix of a full journal after a crash.
        if !header.starts_with(&bytes) {
            return Ok(());
        }
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
