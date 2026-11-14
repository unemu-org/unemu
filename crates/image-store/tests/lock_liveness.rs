// SPDX-License-Identifier: AGPL-3.0-or-later
//! Isolated process-exit lock checks use a bounded handshake so leaked locks fail
//! rather than hanging the suite. No sibling test can lend the child a lock fd.

use image_store::Store;
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    process::{Child, Command},
    time::{Duration, Instant},
};

struct Owner(Child);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn until_ready(mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "lock handshake timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn exiting_owner_releases_kernel_lock_without_removing_lock_file() {
    let root = std::env::temp_dir().join(format!("image-store-exit-test-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let store = Store::open(&root).unwrap();
    let lock = root.join("locks/index.lock");
    let mut child = Owner(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lock_owner_child"])
            .env("IMAGE_STORE_LOCK_TEST_ROOT", &root)
            .spawn()
            .unwrap(),
    );
    until_ready(|| root.join("ready").exists());
    let challenger = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    assert!(matches!(
        challenger.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    fs::write(root.join("exit"), b"").unwrap();
    until_ready(|| {
        child.0.try_wait().unwrap().is_some_and(|s| {
            assert!(s.success());
            true
        })
    });
    until_ready(|| match challenger.try_lock() {
        Ok(()) => true,
        Err(TryLockError::WouldBlock) => false,
        Err(TryLockError::Error(error)) => panic!("lock check failed: {error}"),
    });
    challenger.unlock().unwrap();
    drop(challenger);
    assert_eq!(fs::read(&lock).unwrap(), b"left by exited owner");
    Store::open(store.root()).unwrap();
    assert_eq!(fs::read(&lock).unwrap(), b"left by exited owner");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lock_owner_child() {
    let Some(root) = std::env::var_os("IMAGE_STORE_LOCK_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let file = File::options()
        .read(true)
        .write(true)
        .open(root.join("locks/index.lock"))
        .unwrap();
    file.try_lock().unwrap();
    fs::write(root.join("locks/index.lock"), b"left by exited owner").unwrap();
    fs::write(root.join("ready"), b"").unwrap();
    until_ready(|| root.join("exit").exists());
    std::mem::forget(file);
    std::process::exit(0);
}
