// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host directory fixtures for file-backed tests (core-contract.md §12.10).

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

#[cfg(test)]
mod tests {
    use super::TempDir;

    #[test]
    fn directories_with_the_same_label_are_independent_and_removed() {
        let first = TempDir::new("directory-fixture");
        let second = TempDir::new("directory-fixture");
        let path = first.path().to_owned();
        assert_ne!(path, second.path());
        std::fs::write(path.join("input"), b"fixture").unwrap();
        drop(first);
        assert!(!path.exists());
        assert!(second.path().is_dir());
    }

    #[test]
    fn directory_is_removed_during_panic_unwinding() {
        let mut path = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let directory = TempDir::new("directory-unwind");
            path = Some(directory.path().to_owned());
            std::fs::write(directory.path().join("input"), b"fixture").unwrap();
            panic!("exercise directory cleanup");
        }));
        assert!(result.is_err());
        assert!(!path.unwrap().exists());
    }
}
