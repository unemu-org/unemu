// SPDX-License-Identifier: AGPL-3.0-or-later
//! Isolated process-spawning cleanup tests; persistent-media.md § Tests.
//! A fork here cannot inherit locks held by the library test binary.
#![cfg(unix)]

#[path = "../src/temporary.rs"]
mod temporary;
use media::{Header, SECTOR_LEN};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};
use temporary::*;
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "media-fifo-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[cfg(unix)]
fn assert_fifo_child_completes(test: &str, exercise: impl FnOnce(&Path)) {
    use std::process::{Command, Stdio};
    if let Some(path) = std::env::var_os("MEDIA_TEST_CREATION_FIFO") {
        exercise(Path::new(&path));
        return;
    }
    let dir = TempDir::new();
    let path = dir.0.join("fifo");
    assert!(Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test])
        .env("MEDIA_TEST_CREATION_FIFO", &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if started.elapsed() > std::time::Duration::from_secs(3) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("opening a creation entry blocked on a FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(unix)]
#[test]
fn creation_temp_fifo_open_does_not_block() {
    assert_fifo_child_completes("creation_temp_fifo_open_does_not_block", |path| {
        let file = HostCreationTempIo.open(path).unwrap();
        assert!(!file.metadata().unwrap().file_type().is_file());
        cleanup_retention_temp(&mut HostCreationTempIo, path).unwrap();
    });
}

#[cfg(unix)]
struct SymlinkSwapCreationIo(PathBuf);

#[cfg(unix)]
impl CreationTempIo for SymlinkSwapCreationIo {
    fn metadata(&mut self, path: &Path) -> io::Result<std::fs::Metadata> {
        HostCreationTempIo.metadata(path)
    }
    fn open(&mut self, path: &Path) -> io::Result<File> {
        std::fs::remove_file(path)?;
        std::os::unix::fs::symlink(&self.0, path)?;
        HostCreationTempIo.open(path)
    }
    fn remove(&mut self, path: &Path) -> io::Result<()> {
        HostCreationTempIo.remove(path)
    }
}

#[cfg(unix)]
#[test]
fn cleanup_symlink_swap_to_fifo_does_not_block() {
    assert_fifo_child_completes("cleanup_symlink_swap_to_fifo_does_not_block", |fifo| {
        let candidate = fifo.with_extension("create-7.tmp");
        let header = Header::new(&[0; 2 * SECTOR_LEN], 0)
            .unwrap()
            .encode()
            .unwrap();
        std::fs::write(&candidate, header).unwrap();
        cleanup_creation_temp(&mut SymlinkSwapCreationIo(fifo.into()), &candidate, &header)
            .unwrap();
        assert!(std::fs::symlink_metadata(candidate)
            .unwrap()
            .file_type()
            .is_symlink());
    });
}
