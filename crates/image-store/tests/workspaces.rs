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
