// SPDX-License-Identifier: AGPL-3.0-or-later
//! Generic workspace deletion and lock invariants; machine-images §3.1.
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn generic_locks_and_discard_tombstones_survive_interruption() {
    let temp = Temp(std::env::temp_dir().join(format!(
        "store-workspace-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let store = image_store::Store::open(&temp.0).unwrap();
    assert!(store.open_workspace("typo").is_err());
    assert!(!temp.0.join("workspaces/typo").exists());
    let owner = store.workspace("session").unwrap();
    owner.write("session.toml", b"schema=1\n").unwrap();
    let data = owner.path().join("other-provider/parked");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("data"), b"preserve").unwrap();
    let lock = fs::File::create(data.join("data.lock")).unwrap();
    lock.try_lock().unwrap();
    assert!(owner.discard().is_err());
    drop(lock);
    let owner = store.open_workspace("session").unwrap();
    owner.write("discard.toml", b"schema=1\n").unwrap();
    fs::rename(
        owner.path().join("other-provider"),
        owner.path().join("discarded-other-provider"),
    )
    .unwrap();
    drop(owner);
    let owner = store.open_workspace("session").unwrap();
    assert!(owner.read("session.toml").unwrap().is_some());
    owner.discard().unwrap();
    assert!(!temp.0.join("workspaces/session/session.toml").exists());
    assert!(!temp.0.join("workspaces/session/discard.toml").exists());
    assert!(temp.0.join("workspaces/session/workspace.lock").exists());
}

#[test]
fn workspace_inspection_does_not_take_ownership_or_accept_escaping_names() {
    let temp = Temp(std::env::temp_dir().join(format!(
        "store-workspace-inspection-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let store = image_store::Store::open(&temp.0).unwrap();
    let owner = store.workspace("gaming").unwrap();
    assert_eq!(store.workspace_path("gaming").unwrap(), owner.path());
    assert!(store.open_workspace("gaming").is_err());
    assert!(store.workspace_path("../escape").is_err());
    assert!(store.workspace_path("/absolute").is_err());
    assert!(store.workspace_path("missing").is_ok());
    assert!(!temp.0.join("workspaces/missing").exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            temp.0.join("workspaces/gaming"),
            temp.0.join("workspaces/alias"),
        )
        .unwrap();
        assert!(store.workspace_path("alias").is_err());
    }
}

#[test]
fn complete_discard_removes_directory_and_lock_then_allows_a_new_owner() {
    let temp = Temp(std::env::temp_dir().join(format!(
        "store-complete-discard-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let store = image_store::Store::open(&temp.0).unwrap();
    let owner = store.workspace("disposable").unwrap();
    owner.write("desktop.toml", b"schema=1\n").unwrap();
    assert_eq!(store.list_desktop_workspaces().unwrap(), ["disposable"]);
    assert!(store.workspace("disposable").is_err());
    owner.discard_and_remove().unwrap();
    assert!(!store.workspace_path("disposable").unwrap().exists());
    assert!(store.list_desktop_workspaces().unwrap().is_empty());
    let replacement = store.workspace("disposable").unwrap();
    assert!(store.workspace("disposable").is_err());
    replacement.discard_and_remove().unwrap();
}
