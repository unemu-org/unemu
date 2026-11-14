// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local filesystem and publication primitives for OCI Image Layout; §3.1 of
//! machine-images requires kernel lock liveness and atomic no-clobber publication.

use crate::{Error, Result};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub(crate) fn lock_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?)
}

pub(crate) fn shared_lease(path: &Path) -> Result<File> {
    let file = lock_file(path)?;
    file.lock_shared()?;
    Ok(file)
}

pub(crate) fn exclusive_lock(path: &Path) -> Result<File> {
    let file = lock_file(path)?;
    file.lock()?;
    Ok(file)
}

pub(crate) fn probe_lock(path: &Path) -> Result<File> {
    let owner = exclusive_lock(path)?;
    let challenger = lock_file(path)?;
    match challenger.try_lock() {
        Err(fs::TryLockError::WouldBlock) => Ok(owner),
        Err(fs::TryLockError::Error(error)) => Err(error.into()),
        Ok(()) => Err(Error::UnsupportedFilesystem(
            "independent descriptors bypass advisory locks".into(),
        )),
    }
}

pub(crate) struct Temporary {
    pub file: File,
    pub path: PathBuf,
    armed: bool,
}

impl Temporary {
    pub fn create(staging: &Path) -> Result<Self> {
        loop {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            // Local staging names need fresh entropy across PID reuse; none of it
            // enters stored content or affects machine identity.
            use std::hash::{Hash, Hasher};
            let mut nonce = std::collections::hash_map::DefaultHasher::new();
            std::time::SystemTime::now().hash(&mut nonce);
            std::process::id().hash(&mut nonce);
            sequence.hash(&mut nonce);
            (&sequence as *const u64 as usize).hash(&mut nonce);
            let path = staging.join(format!(
                "tmp-{}-{sequence}-{:016x}",
                std::process::id(),
                nonce.finish()
            ));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        path,
                        armed: true,
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Temporary {
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn c_path(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| crate::invalid("path contains a NUL byte"))
}

pub(crate) fn publish_blob(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let source = c_path(source)?;
        let destination = c_path(destination)?;
        // No-clobber rename prevents a concurrent importer from replacing the inode
        // already held by readers. Both strings remain alive for the system call.
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (source, destination);
        Err(Error::UnsupportedFilesystem(
            "atomic no-clobber rename is unavailable on this host".into(),
        ))
    }
}

pub(crate) fn require_local(root: &Path) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let root = c_path(root)?;
        let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // statfs initializes the complete structure on success; the path is a
        // live NUL-terminated string for the duration of the call.
        if unsafe { libc::statfs(root.as_ptr(), info.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let info = unsafe { info.assume_init() };
        #[cfg(target_os = "macos")]
        let local = info.f_flags & libc::MNT_LOCAL as u32 != 0;
        // A conservative allowlist refuses unknown and network-backed filesystems.
        #[cfg(target_os = "linux")]
        let local = matches!(
            info.f_type as u32,
            0xef53 | 0x01021994 | 0x9123683e | 0x58465342 | 0x2fc12fc1 | 0xf2f52010 | 0x794c7630
        );
        if !local {
            return Err(Error::UnsupportedFilesystem(
                "root is not a recognized local filesystem".into(),
            ));
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = root;
        Err(Error::UnsupportedFilesystem(
            "locality detection is unavailable on this host".into(),
        ))
    }
}

pub(crate) fn probe_publish(staging: &Path) -> Result<()> {
    probe_publish_with(staging, publish_blob)
}

fn probe_publish_with(staging: &Path, publish: impl Fn(&Path, &Path) -> Result<()>) -> Result<()> {
    use std::io::Write;
    let mut source = Temporary::create(staging)?;
    let destination = Temporary::create(staging)?;
    source.file.write_all(b"probe")?;
    source.file.sync_all()?;
    let outcome = (|| {
        match publish(&source.path, &destination.path) {
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
            Ok(()) => {
                return Err(Error::UnsupportedFilesystem(
                    "no-clobber rename replaced its destination".into(),
                ))
            }
        }
        if !fs::read(&destination.path)?.is_empty() || fs::read(&source.path)? != b"probe" {
            return Err(Error::UnsupportedFilesystem(
                "no-clobber rename changed existing bytes".into(),
            ));
        }
        fs::remove_file(&destination.path)?;
        publish(&source.path, &destination.path)?;
        source.disarm();
        if fs::read(&destination.path)? != b"probe" {
            return Err(Error::UnsupportedFilesystem(
                "no-clobber rename lost published bytes".into(),
            ));
        }
        Ok(())
    })();
    match outcome {
        #[cfg(unix)]
        Err(Error::Io(error))
            if matches!(
                error.raw_os_error(),
                Some(libc::EINVAL) | Some(libc::ENOTSUP)
            ) =>
        {
            Err(Error::UnsupportedFilesystem(format!(
                "no-clobber rename is unavailable: {error}"
            )))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "image-store-probe-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn publish_probe_cleans_success_and_unsupported_failures() {
        let root = directory();
        probe_publish(&root).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        #[cfg(unix)]
        for code in [libc::EINVAL, libc::ENOTSUP] {
            assert!(matches!(
                probe_publish_with(&root, |_, _| Err(io::Error::from_raw_os_error(code).into())),
                Err(Error::UnsupportedFilesystem(_))
            ));
            assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
            let calls = std::cell::Cell::new(0);
            assert!(matches!(
                probe_publish_with(&root, |_, _| {
                    let call = calls.get();
                    calls.set(call + 1);
                    Err(
                        io::Error::from_raw_os_error(if call == 0 { libc::EEXIST } else { code })
                            .into(),
                    )
                }),
                Err(Error::UnsupportedFilesystem(_))
            ));
            assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        }
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn successful_rename_disarms_cleanup_before_a_name_is_reused() {
        let root = directory();
        let mut temp = Temporary::create(&root).unwrap();
        let old_name = temp.path.clone();
        let destination = root.join("published");
        publish_blob(&temp.path, &destination).unwrap();
        temp.disarm();
        fs::write(&old_name, b"new owner").unwrap();
        drop(temp);
        assert_eq!(fs::read(old_name).unwrap(), b"new owner");
        fs::remove_dir_all(root).unwrap();
    }
}
