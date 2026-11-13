// SPDX-License-Identifier: AGPL-3.0-or-later
//! Atomic creation of immutable raw-image bases; machines/README.md § Preparing
//! disks and persistent-media.md § Offline commands. Filesystem metadata is
//! supplied by the caller; this module does not model a disk controller.

use crate::offline::{HostIo, ToolIo};
use std::fs;
use std::io;
use std::path::Path;

/// Publish a zero-filled image with an optional metadata prefix.
///
/// A sibling temporary is synced before publication. Without `force`, a hard
/// link provides atomic no-clobber publication (the same contract as offline
/// commit); with `force`, rename replaces the directory entry, never the old
/// inode. Open images, hard links and symlink targets keep their original bytes.
/// Sparse zero ranges are permitted. No existing base is opened for writing.
pub fn image(path: &Path, bytes: u64, prefix: &[u8], force: bool) -> io::Result<()> {
    image_with(&mut HostIo, path, bytes, prefix, force)
}

fn image_with(
    io: &mut impl ToolIo,
    path: &Path,
    bytes: u64,
    prefix: &[u8],
    force: bool,
) -> io::Result<()> {
    if bytes == 0 || prefix.len() as u64 > bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid image length",
        ));
    }
    if !force {
        match fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "output exists; use --force to replace its directory entry",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let (mut file, temporary) = io.create_temporary(path, "create", None)?;
    let result = (|| {
        file.set_len(bytes)?;
        io.write_temporary(&mut file, prefix)?;
        io.sync(&file, "image temporary sync")?;
        io.publish_temporary(&temporary, path, force)
    })();
    // The name belongs to this invocation only; never remove another writer's
    // temporary or follow an output symlink to clean up a failed publication.
    let cleanup = fs::remove_file(&temporary);
    result?;
    if let Err(error) = cleanup {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(io::Error::new(
                error.kind(),
                format!("image published, temporary cleanup failed: {error}"),
            ));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::File::open(parent)?.sync_all().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("image published, directory sync failed: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FaultIo {
        phase: u8,
    }
    impl ToolIo for FaultIo {
        fn write_temporary(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
            file.write_all(&bytes[..bytes.len().min(7)])?;
            if self.phase == 0 {
                return Err(io::Error::other("injected partial write"));
            }
            Ok(())
        }
        fn sync(&mut self, _: &File, _: &'static str) -> io::Result<()> {
            if self.phase == 1 {
                return Err(io::Error::other("injected sync failure"));
            }
            Ok(())
        }
        fn publish_temporary(&mut self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::Error::other("injected publication failure"))
        }
    }

    #[test]
    fn partial_write_sync_and_publication_failures_leave_no_partial_image() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for existing in [false, true] {
            for phase in 0..3 {
                let dir = std::env::temp_dir().join(format!(
                    "unemupc98-create-fault-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::create_dir(&dir).unwrap();
                let destination = dir.join("disk.img");
                if existing {
                    fs::write(&destination, b"old base").unwrap();
                }
                let mut io = FaultIo { phase };
                assert!(image_with(&mut io, &destination, 4096, &[1; 512], existing).is_err());
                if existing {
                    assert_eq!(fs::read(&destination).unwrap(), b"old base");
                } else {
                    assert!(!destination.exists());
                }
                assert_eq!(fs::read_dir(&dir).unwrap().count(), usize::from(existing));
                fs::remove_dir_all(dir).unwrap();
            }
        }
    }

    #[test]
    fn concurrent_destination_is_not_clobbered() {
        let dir =
            std::env::temp_dir().join(format!("unemupc98-create-race-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("disk.img");
        struct RacingIo;
        impl ToolIo for RacingIo {
            fn publish_temporary(
                &mut self,
                temporary: &Path,
                path: &Path,
                replace: bool,
            ) -> io::Result<()> {
                fs::write(path, b"concurrent output")?;
                HostIo.publish_temporary(temporary, path, replace)
            }
        }
        assert!(image_with(&mut RacingIo, &destination, 4096, &[], false).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"concurrent output");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
