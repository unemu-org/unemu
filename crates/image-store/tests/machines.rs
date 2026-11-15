// SPDX-License-Identifier: AGPL-3.0-or-later
//! The local machine file and workspace moves; machine-images §3.2.
use image_store::{
    machine::{Machine, Origin},
    media_type, Descriptor, Digest, Store,
};
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
fn temp() -> Temp {
    Temp(std::env::temp_dir().join(format!(
        "store-machines-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )))
}
fn image() -> Descriptor {
    Descriptor::new(media_type::MANIFEST, Digest::of(b"manifest"), 8)
}

#[test]
fn machine_records_persist_atomically_and_keep_names_unique() {
    let temp = temp();
    let store = Store::open(&temp.0).unwrap();
    assert!(store.read_machines().unwrap().is_empty());
    assert!(!temp.0.join("local/machines.toml").exists());
    store
        .edit_machines(|machines| {
            let mut machine = Machine::new("DOS games", image(), "dos:1.0.0", Origin::New);
            machine.labels = vec!["games".into()];
            machine.settings = Some("memory = { extended_kib = 1024 }".parse().unwrap());
            machines.insert("dos-games", machine)
        })
        .unwrap();
    let reopened = Store::open_existing(&temp.0)
        .unwrap()
        .read_machines()
        .unwrap();
    let machine = reopened.get("dos-games").unwrap();
    assert_eq!(machine.name, "DOS games");
    assert_eq!(machine.source, "dos:1.0.0");
    assert_eq!(machine.labels, ["games"]);
    assert_eq!(
        machine.settings.as_ref().unwrap()["memory"]["extended_kib"].as_integer(),
        Some(1024)
    );
    // A clash ignoring case leaves the file as it was.
    let before = fs::read(temp.0.join("local/machines.toml")).unwrap();
    assert!(store
        .edit_machines(|machines| machines
            .insert("other", Machine::new("dos GAMES", image(), "", Origin::New)))
        .is_err());
    assert!(store
        .edit_machines(
            |machines| machines.insert("Bad/Id", Machine::new("Fine", image(), "", Origin::New))
        )
        .is_err());
    assert_eq!(
        fs::read(temp.0.join("local/machines.toml")).unwrap(),
        before
    );
    // Ids never reuse a machine's or any workspace directory's.
    drop(store.workspace("keen").unwrap());
    let machines = store.read_machines().unwrap();
    assert_eq!(store.free_machine_id(&machines, "Keen").unwrap(), "keen-2");
    assert_eq!(
        store.free_machine_id(&machines, "DOS games").unwrap(),
        "dos-games-2"
    );
    assert_eq!(
        store.free_machine_id(&machines, "Win 3.1").unwrap(),
        "win-3-1"
    );
    // Desktop and Gaming-mode prefixes are never handed out.
    for name in ["Fresh install", "Regular", "gaming-pc", "Disposable"] {
        let id = store.free_machine_id(&machines, name).unwrap();
        assert!(id.starts_with("m-"), "{id}");
        assert!(!image_store::machine::reserved_id(&id));
    }
}

#[test]
fn a_newer_or_damaged_machine_file_fails_closed_and_is_never_rewritten() {
    let temp = temp();
    let store = Store::open(&temp.0).unwrap();
    fs::create_dir_all(temp.0.join("local")).unwrap();
    for text in ["schema = 2\n", "schema = 1\n[machines.x]\nname = 1\n"] {
        fs::write(temp.0.join("local/machines.toml"), text).unwrap();
        assert!(store.read_machines().is_err());
        assert!(store.edit_machines(|_| Ok(())).is_err());
        assert_eq!(
            fs::read_to_string(temp.0.join("local/machines.toml")).unwrap(),
            text
        );
    }
}

#[test]
fn a_workspace_moves_with_its_lock_and_never_over_another() {
    let temp = temp();
    let store = Store::open(&temp.0).unwrap();
    drop(store.workspace("taken").unwrap());
    let mut owner = store.workspace("regular-0123").unwrap();
    owner.write("session.toml", b"schema = 1\n").unwrap();
    fs::create_dir(owner.path().join("media")).unwrap();
    fs::write(owner.path().join("media/disk.upfj"), b"journal").unwrap();
    assert!(owner.rename("taken").is_err());
    assert!(owner.rename("Not Valid").is_err());
    owner.rename("open-at-286").unwrap();
    assert_eq!(owner.name(), "open-at-286");
    assert_eq!(owner.path(), store.workspace_path("open-at-286").unwrap());
    assert!(!store.workspace_path("regular-0123").unwrap().exists());
    assert_eq!(
        fs::read(owner.path().join("media/disk.upfj")).unwrap(),
        b"journal"
    );
    // The moved owner still holds the workspace.
    assert!(store.workspace("open-at-286").is_err());
    owner.clear().unwrap();
    assert!(owner.read("session.toml").unwrap().is_none());
    assert!(!owner.path().join("media").exists());
    assert!(store.workspace("open-at-286").is_err());
    drop(owner);
    drop(store.workspace("open-at-286").unwrap());
}
