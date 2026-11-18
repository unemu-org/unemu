// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helpers for the tests of crates built on the store, behind the
//! `test-support` feature: a scratch directory for stores, preferences and
//! other files a test writes. The emulation crates keep their own copy in
//! `assembly`, which they already depend on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A unique test directory removed on success and during panic unwinding.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Create an empty directory with a process-local unique name.
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("test directory: {error}"),
            }
        }
    }

    /// Borrow the directory for test inputs and output paths.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
