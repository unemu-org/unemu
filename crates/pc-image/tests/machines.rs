// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machines as named local instances of images; machine-images §3.2. A
//! synthetic machine with the open Xi8088 BIOS and XTIDE ROM, a 360K floppy
//! catalogue, a fixed disk and a journaled CMOS exercises every machine
//! operation and the migration of workspaces made before machines existed.

use image_store::{
    machine::{Machine, Origin},
    BlobSource, Store,
};
use pc_image::{
    machine::{self, State},
    workspace, PcProvider, CMOS,
};
use std::fs;
use toml::{Table, Value};
#[path = "support/workspace.rs"]
mod fixture;
use fixture::{fixtures, Fixture};

/// Start a machine as a frontend does once it has assembled: prepare, then
/// confirm.
fn start(
    owner: &image_store::workspace::Workspace<'_>,
    last_run: Option<String>,
) -> machine::Prepared {
    let prepared = machine::prepare(owner).unwrap();
    machine::confirm(owner, &prepared.inserted, last_run).unwrap();
    prepared
}
fn state(store: &Store, id: &str) -> State {
    machine::get(store, id).unwrap().state
}
fn journals(store: &Store, id: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(store.workspace_path(id).unwrap().join("media"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".upfj"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
fn cmos(fixture: &Fixture, id: &str) -> [u8; 64] {
    let image = fixture
        .store
        .resolve_descriptor(
            &machine::pinned_image(&fixture.store, id).unwrap(),
            &PcProvider,
        )
        .unwrap();
    let d = image
        .manifest
        .layers
        .iter()
        .find(|d| d.media_type == CMOS)
        .unwrap();
    let base = fixture.store.read_verified(d, 64).unwrap();
    let path = fixture
        .store
        .workspace_path(id)
        .unwrap()
        .join("media/cmos.upfj");
    let Ok(journal) = fs::read(path) else {
        return base.try_into().unwrap();
    };
    let scan = media::scan(&journal, &base, 64);
    scan.image.unwrap()[..64].try_into().unwrap()
}
fn checksum(bytes: &mut [u8; 64]) {
    let sum = bytes[0x10..=0x2D]
        .iter()
        .fold(0u16, |sum, &b| sum.wrapping_add(u16::from(b)));
    bytes[0x2E..=0x2F].copy_from_slice(&sum.to_be_bytes());
}
fn with_extended(machine: &Table, kib: i64) -> Table {
    let mut machine = machine.clone();
    machine
        .get_mut("memory")
        .and_then(Value::as_table_mut)
        .unwrap()
        .insert("extended_kib".into(), kib.into());
    machine
}
/// Publish a titled image from a scratch workspace, as Save as new image does.
fn titled(fixture: &Fixture, tag: &str, title: &str) -> image_store::Descriptor {
    let owner = fixture.store.workspace("titled-scratch").unwrap();
    workspace::start(&owner, "test:source").unwrap();
    let image = workspace::publish_as(&owner, tag, None, false, Some(title), |_| Ok(())).unwrap();
    owner.discard_and_remove().unwrap();
    image
}

#[test]
fn new_machines_take_the_image_title_then_a_number_and_a_slug_id() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    titled(&fixture, "dos:1.0.0", "DOS games");
    let first = machine::create_from_tag(store, "dos:1.0.0", None, Some("today".into())).unwrap();
    let second = machine::create_from_tag(store, "dos:1.0.0", None, None).unwrap();
    assert_eq!(first, "dos-games");
    assert_eq!(second, "dos-games-2");
    let listing = machine::get(store, &first).unwrap();
    assert_eq!(listing.machine.name, "DOS games");
    assert_eq!(listing.machine.source, "dos:1.0.0");
    assert_eq!(listing.machine.created.as_deref(), Some("today"));
    assert_eq!(listing.machine.origin, Origin::New);
    assert_eq!(listing.tags, ["dos:1.0.0"]);
    assert_eq!(listing.state, State::NeverStarted);
    assert_eq!(listing.state.to_string(), "Never started");
    assert!(!listing.detached());
    assert_eq!(
        machine::get(store, &second).unwrap().machine.name,
        "DOS games (2)"
    );
    // The workspace is pinned at once, without verifying or materializing.
    assert!(workspace::pinned_image_at(store, &first).unwrap().is_some());
    // An untitled image falls back to its reference.
    let plain = machine::create_from_tag(store, "test:source", None, None).unwrap();
    assert_eq!(
        machine::get(store, &plain).unwrap().machine.name,
        "test:source"
    );
    assert_eq!(plain, "test-source");
    // Names are unique ignoring case, 1–64 characters.
    let error = machine::create_from_tag(store, "test:source", Some("dos GAMES"), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("exists already"), "{error}");
    assert!(machine::create_from_tag(store, "test:source", Some("  "), None).is_err());
    assert!(machine::create_from_tag(store, "test:source", Some(&"n".repeat(65)), None).is_err());
    // A failed create leaves nothing behind.
    assert_eq!(machine::list(store).unwrap().len(), 3);
    assert!(!store.workspace_path("dos-games-3").unwrap().exists());
    machine::rename(store, &first, "Keen").unwrap();
    let listing = machine::get(store, &first).unwrap();
    assert_eq!(
        (listing.id.as_str(), listing.machine.name.as_str()),
        ("dos-games", "Keen")
    );
    assert!(machine::rename(store, &second, "KEEN").is_err());
    machine::set_favorite(store, &first, true).unwrap();
    machine::set_labels(
        store,
        &first,
        vec![" games ".into(), "games".into(), "".into()],
    )
    .unwrap();
    let listing = machine::get(store, &first).unwrap();
    assert!(listing.machine.favorite);
    assert_eq!(listing.machine.labels, ["games"]);
}

/// A machine made from an untitled image pulled under a host-qualified tag
/// is named after that tag, and its id becomes a folder name: the host's
/// dots, the port's colon and the path's slashes must not reach it.
#[test]
fn a_machine_from_a_host_qualified_tag_gets_a_plain_folder_name() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let image = store.tag_descriptor("test:source").unwrap();
    let tag = "nas.example:5000/private/x:1";
    store
        .update_pulled_tag(tag, &image, None, &PcProvider, "nas.example:5000")
        .unwrap();
    let id = machine::create_from_tag(store, tag, None, None).unwrap();
    assert_eq!(id, "nas-example-5000-private-x-1");
    let folder = store.workspace_path(&id).unwrap();
    assert_eq!(folder.parent().unwrap(), store.root().join("workspaces"));
    assert!(folder.is_dir());
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.machine.name, tag);
    assert_eq!(listing.machine.source, tag);
    // The machine pins the image, not the annotations of the tag it came by,
    // a pulled tag's or one made here.
    assert_eq!(listing.machine.image.digest, image.digest);
    assert!(
        listing.machine.image.annotations.is_empty(),
        "{:?}",
        listing.machine.image.annotations
    );
    store
        .update_tag("made-here:1", &image, None, &PcProvider)
        .unwrap();
    assert!(store
        .list_tags(None, 100)
        .unwrap()
        .tags
        .iter()
        .any(|t| t.made_here()));
    let id = machine::create_from_tag(store, "made-here:1", Some("Made here"), None).unwrap();
    let listing = machine::get(store, &id).unwrap();
    assert!(
        listing.machine.image.annotations.is_empty(),
        "{:?}",
        listing.machine.image.annotations
    );
    // Nor where a push from here sent it.
    store
        .record_push(
            "made-here:1",
            &image.digest,
            "registry.lan.example/private/made-here:1",
            1,
        )
        .unwrap();
    let id = machine::create_from_tag(store, "made-here:1", Some("Pushed"), None).unwrap();
    let listing = machine::get(store, &id).unwrap();
    assert!(
        listing.machine.image.annotations.is_empty(),
        "{:?}",
        listing.machine.image.annotations
    );
}

/// A listing that probes a machine while another listing's probe holds the
/// workspace lock for its instant reads the machine as stopped: only an
/// owner's exclusive lock means running.
#[test]
fn a_concurrent_listing_probe_does_not_read_as_running() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Lab"), None).unwrap();
    drop(store.workspace(&id).unwrap());
    let stopped = state(store, &id);
    assert_ne!(stopped, State::Running);
    let probe = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.workspace_path(&id).unwrap().join("workspace.lock"))
        .unwrap();
    probe.try_lock_shared().unwrap();
    assert_eq!(state(store, &id), stopped);
    let listed = machine::list(store).unwrap();
    let listed = listed.iter().find(|l| l.id == id).unwrap();
    assert_eq!(listed.state, stopped);
    drop(probe);
    let _owner = store.workspace(&id).unwrap();
    assert_eq!(state(store, &id), State::Running);
}

#[test]
fn state_shows_changed_drives_cmos_and_running() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Lab"), None).unwrap();
    {
        let owner = store.workspace(&id).unwrap();
        assert_eq!(state(store, &id), State::Running);
        let started = start(&owner, Some("run 1".into()));
        assert!(started.inserted.is_empty());
        assert_eq!(started.session.image, fixture.image.digest);
    }
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.machine.last_run.as_deref(), Some("run 1"));
    assert_eq!(listing.state, State::SameAsImage);
    fixture.changed(&id, 7);
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.state.to_string(), "Changed: A:, C:, CMOS");
    assert!(listing.change_bytes > 0);
    assert_eq!(
        listing
            .changes
            .iter()
            .map(|c| (c.drive.as_str(), c.sectors))
            .collect::<Vec<_>>(),
        [("A:", Some(1)), ("A:", Some(1)), ("C:", Some(1))]
    );
    // A machine operation refuses while another owner runs it.
    let owner = store.workspace(&id).unwrap();
    assert!(machine::reset(store, &id)
        .unwrap_err()
        .to_string()
        .contains("running"));
    drop(owner);
}

#[test]
fn duplicate_copies_changes_settings_inserts_and_labels_independently() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("DOS"), None).unwrap();
    fixture.changed(&id, 5);
    let settings = machine::settings(store, &id).unwrap();
    machine::change_settings(store, &id, with_extended(&settings.machine, 4096), true).unwrap();
    machine::set_labels(store, &id, vec!["games".into()]).unwrap();
    machine::set_favorite(store, &id, true).unwrap();
    let set = fixture.image.clone();
    let pending = vec![image_store::machine::PendingInsert {
        drive: "A:".into(),
        set,
        reference: "set:1".into(),
        medium: "disk-1".into(),
        all: false,
        journal: true,
    }];
    machine::set_pending_inserts(store, &id, pending.clone()).unwrap();
    let copy = machine::duplicate(store, &id, None, Some("now".into())).unwrap();
    let (original, copied) = (
        machine::get(store, &id).unwrap(),
        machine::get(store, &copy).unwrap(),
    );
    assert_eq!(copied.machine.name, "DOS (2)");
    assert_eq!(copied.machine.origin, Origin::Duplicate);
    assert_eq!(copied.machine.labels, ["games"]);
    assert!(!copied.machine.favorite);
    assert_eq!(copied.machine.last_run, None);
    assert_eq!(copied.machine.pending, pending);
    assert_eq!(copied.machine.settings, original.machine.settings);
    assert_eq!(copied.state, original.state);
    assert_eq!(journals(store, &copy), journals(store, &id));
    assert_eq!(cmos(&fixture, &copy), cmos(&fixture, &id));
    for name in journals(store, &id) {
        let path = |id: &str| store.workspace_path(id).unwrap().join("media").join(&name);
        assert_eq!(fs::read(path(&copy)).unwrap(), fs::read(path(&id)).unwrap());
    }
    // Later writes stay with their machine.
    let before: Vec<Vec<u8>> = journals(store, &id)
        .iter()
        .map(|n| fs::read(store.workspace_path(&id).unwrap().join("media").join(n)).unwrap())
        .collect();
    fixture.changed(&copy, 9);
    let after: Vec<Vec<u8>> = journals(store, &id)
        .iter()
        .map(|n| fs::read(store.workspace_path(&id).unwrap().join("media").join(n)).unwrap())
        .collect();
    assert_eq!(before, after);
    assert_ne!(cmos(&fixture, &copy), cmos(&fixture, &id));
    assert!(machine::duplicate(store, &id, Some("dos"), None).is_err());
}

#[test]
fn reset_drops_changes_and_inserts_but_keeps_identity_and_delete_keeps_the_image() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Work"), None).unwrap();
    {
        let owner = store.workspace(&id).unwrap();
        start(&owner, Some("yesterday".into()));
    }
    fixture.changed(&id, 3);
    machine::set_favorite(store, &id, true).unwrap();
    machine::set_labels(store, &id, vec!["keep".into()]).unwrap();
    let settings = machine::settings(store, &id).unwrap();
    machine::change_settings(store, &id, with_extended(&settings.machine, 2048), true).unwrap();
    machine::set_pending_inserts(
        store,
        &id,
        vec![image_store::machine::PendingInsert {
            drive: "A:".into(),
            set: fixture.image.clone(),
            reference: String::new(),
            medium: "disk-1".into(),
            all: false,
            journal: false,
        }],
    )
    .unwrap();
    machine::reset(store, &id).unwrap();
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.state, State::SameAsImage);
    assert!(journals(store, &id).is_empty());
    assert_eq!(listing.machine.name, "Work");
    assert!(listing.machine.favorite);
    assert_eq!(listing.machine.labels, ["keep"]);
    assert_eq!(listing.machine.last_run.as_deref(), Some("yesterday"));
    assert!(listing.machine.pending.is_empty());
    assert!(listing.machine.settings.is_none());
    assert_eq!(
        workspace::pinned_image_at(store, &id).unwrap(),
        Some(fixture.image.digest.clone())
    );
    machine::delete(store, &id).unwrap();
    assert!(machine::get(store, &id).is_err());
    assert!(!store.workspace_path(&id).unwrap().exists());
    assert_eq!(
        store.tag_descriptor("test:source").unwrap().digest,
        fixture.image.digest
    );
    store.resolve("test:source", &PcProvider).unwrap();
}

#[test]
fn settings_apply_from_the_next_start_and_recompute_the_machines_cmos() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Mem"), None).unwrap();
    let base = cmos(&fixture, &id);
    let image_tables = machine::settings(store, &id).unwrap().image;
    let wanted = with_extended(&image_tables, 4096);
    // A dry run checks and reports without keeping anything.
    let dry = machine::change_settings(store, &id, wanted.clone(), false).unwrap();
    assert!(dry.diff.contains(&pc_image::revision::DiffLine::Added(
        "extended_kib = 4096".into()
    )));
    assert!(machine::get(store, &id).unwrap().machine.settings.is_none());
    assert_eq!(cmos(&fixture, &id), base);
    let change = machine::change_settings(store, &id, wanted, true).unwrap();
    let mut expected = base;
    expected[0x17..=0x18].copy_from_slice(&4096u16.to_le_bytes());
    expected[0x30..=0x31].copy_from_slice(&4096u16.to_le_bytes());
    checksum(&mut expected);
    let offsets: Vec<usize> = change.cmos_changes.iter().map(|c| c.offset).collect();
    assert_eq!(
        offsets,
        (0..64)
            .filter(|&i| expected[i] != base[i])
            .collect::<Vec<_>>()
    );
    assert_eq!(cmos(&fixture, &id), expected);
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.state.to_string(), "Changed: CMOS, settings");
    // The image never changes; the next Start lowers the machine's settings.
    assert_eq!(machine::settings(store, &id).unwrap().image, image_tables);
    {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        assert_eq!(
            started.session.machine.memory.address_space_bytes,
            0x100000 + 4096 * 1024
        );
    }
    let resolved = pc_image::resolve(store, "test:source", &fixture.path.join("s")).unwrap();
    assert_ne!(resolved.memory.address_space_bytes, 0x100000 + 4096 * 1024);
    // The image's own settings drop the overlay and restore the CMOS fields.
    machine::change_settings(store, &id, image_tables, true).unwrap();
    let listing = machine::get(store, &id).unwrap();
    assert!(listing.machine.settings.is_none());
    assert_eq!(cmos(&fixture, &id), base);
    assert!(machine::settings(store, &id).unwrap().diff.is_empty());
}

#[test]
fn commit_moves_the_machine_onto_the_new_image_or_keeps_it_on_the_old() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Saver"), None).unwrap();
    fixture.changed(&id, 42);
    let tables = machine::settings(store, &id).unwrap().machine;
    machine::change_settings(store, &id, with_extended(&tables, 4096), true).unwrap();
    // Keep on the old image: a new image with the changes stands alone.
    let kept = machine::commit(store, &id, "saved:1.0.0", None, None, true).unwrap();
    fixture.assert_complete("saved:1.0.0", 42);
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.machine.image.digest, fixture.image.digest);
    assert!(listing.machine.settings.is_some());
    assert_eq!(listing.state.to_string(), "Changed: A:, C:, CMOS, settings");
    let saved = store.resolve("saved:1.0.0", &PcProvider).unwrap();
    assert_eq!(saved.image.digest, kept.digest);
    assert_eq!(saved.manifest.annotations[workspace::TITLE], "Saver");
    let lowered = pc_image::resolve(store, "saved:1.0.0", &fixture.path.join("k")).unwrap();
    assert_eq!(lowered.memory.address_space_bytes, 0x100000 + 4096 * 1024);
    // The default moves the machine onto the new image with fresh journals.
    let moved = machine::commit(store, &id, "saved:2.0.0", None, Some("Saved"), false).unwrap();
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.machine.image.digest, moved.digest);
    assert_eq!(listing.machine.source, "saved:2.0.0");
    assert!(listing.machine.settings.is_none());
    assert!(listing.changes.is_empty());
    assert_eq!(listing.state, State::NeverStarted);
    fixture.assert_complete("saved:2.0.0", 42);
    fixture.assert_complete("test:source", 0);
    {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        assert_eq!(started.session.image, moved.digest);
        assert_eq!(
            started.session.machine.memory.address_space_bytes,
            0x100000 + 4096 * 1024
        );
    }
    // A version tag never moves.
    assert!(machine::commit(store, &id, "saved:2.0.0", None, None, false).is_err());
}

#[test]
fn a_machine_whose_tag_moves_or_goes_stays_on_its_image() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let newer = titled(&fixture, "other:1.0.0", "Other");
    store
        .update_tag("test:latest", &fixture.image, None, &PcProvider)
        .unwrap();
    let id = machine::create_from_tag(store, "test:latest", Some("Pinned"), None).unwrap();
    store
        .update_tag(
            "test:latest",
            &newer,
            Some(&fixture.image.digest),
            &PcProvider,
        )
        .unwrap();
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(
        listing.moved_to.as_ref().map(|d| &d.digest),
        Some(&newer.digest)
    );
    assert_eq!(listing.machine.image.digest, fixture.image.digest);
    assert!(!listing.detached());
    store
        .remove_tag("test:source", &fixture.image.digest)
        .unwrap();
    let listing = machine::get(store, &id).unwrap();
    assert!(listing.detached());
    // A detached machine still starts from its pinned image.
    let owner = store.workspace(&id).unwrap();
    let started = start(&owner, None);
    assert_eq!(started.session.image, fixture.image.digest);
}

#[test]
fn migration_adopts_old_workspaces_without_losing_data_and_is_idempotent() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let titled_image = titled(&fixture, "open-at:1.1.0", "Open AT 286");
    store
        .edit_library(&titled_image.digest, |entry| {
            entry.favorite = true;
            entry.last_run = Some("2026-11-13 20:30 UTC".into());
            Ok(())
        })
        .unwrap();
    // The old layout: a desktop workspace named from the image digest, one
    // named on the command line, unfinished Gaming-mode copies and a damaged
    // workspace.
    let regular = format!("regular-{}", &titled_image.digest.hex()[..56]);
    let desktop = |name: &str| {
        let owner = store.workspace(name).unwrap();
        owner
            .write(
                "desktop.toml",
                b"schema = 1\nkind = \"regular\"\nreference = \"open-at:1.1.0\"\ntitle = \"Open AT 286\"\nrecovery = true\n",
            )
            .unwrap();
    };
    fixture.changed_from(&regular, "open-at:1.1.0", 11);
    desktop(&regular);
    fixture.changed("play", 12);
    let disposable = format!("disposable-{}-1-0", &fixture.image.digest.hex()[..16]);
    fixture.changed(&disposable, 13);
    fixture.changed("fresh-0", 14);
    {
        let broken = store.workspace("broken").unwrap();
        fs::write(broken.path().join("session.toml"), "not a session").unwrap();
    }
    let journal = |id: &str| {
        fs::read(
            store
                .workspace_path(id)
                .unwrap()
                .join("media/medium-0.upfj"),
        )
        .unwrap()
    };
    let regular_journal = journal(&regular);
    let migration = machine::migrate(store).unwrap();
    // Adopted in the order of the workspace names.
    assert_eq!(migration.adopted, ["play", "open-at-286"]);
    // One damaged workspace is reported and does not stop the others.
    assert_eq!(migration.skipped.len(), 1);
    assert_eq!(migration.skipped[0].0, "broken");
    let open = machine::get(store, "open-at-286").unwrap();
    assert_eq!(open.machine.name, "Open AT 286");
    assert_eq!(open.machine.origin, Origin::Migration);
    assert_eq!(open.machine.source, "open-at:1.1.0");
    assert!(open.machine.favorite);
    assert_eq!(
        open.machine.last_run.as_deref(),
        Some("2026-11-13 20:30 UTC")
    );
    // The move is done, so the record no longer names the old workspace.
    assert_eq!(open.machine.migrated_from, None);
    assert_eq!(open.machine.image.digest, titled_image.digest);
    assert!(open.machine.image.annotations.is_empty());
    assert_eq!(journal("open-at-286"), regular_journal);
    assert!(!store.workspace_path(&regular).unwrap().exists());
    // The desktop's record of its old sessions went with the move.
    assert!(!store
        .workspace_path("open-at-286")
        .unwrap()
        .join("desktop.toml")
        .exists());
    assert!(open.state.to_string().starts_with("Changed: A:, C:"));
    let play = machine::get(store, "play").unwrap();
    assert_eq!(play.machine.name, "play");
    assert_eq!(play.machine.image.digest, fixture.image.digest);
    assert!(!play.machine.favorite);
    // Gaming-mode workspaces stay as they are.
    for name in [disposable.as_str(), "fresh-0"] {
        assert!(machine::get(store, name).is_err());
        assert!(store
            .workspace_path(name)
            .unwrap()
            .join("session.toml")
            .exists());
    }
    let again = machine::migrate(store).unwrap();
    assert!(again.adopted.is_empty());
    assert_eq!(machine::list(store).unwrap().len(), 2);
    // The adopted machine runs with its kept journals.
    {
        let owner = machine::own(store, "open-at-286").unwrap();
        start(&owner, None);
    }
    // The desktop of an older build makes its workspace again under the old
    // name and works there: that work becomes a machine of its own.
    fixture.changed_from(&regular, "open-at:1.1.0", 15);
    desktop(&regular);
    let later = journal(&regular);
    let migration = machine::migrate(store).unwrap();
    assert_eq!(migration.adopted, ["open-at-286-2"]);
    let second = machine::get(store, "open-at-286-2").unwrap();
    assert_eq!(second.machine.name, "Open AT 286 (2)");
    assert_eq!(journal("open-at-286-2"), later);
    assert_eq!(journal("open-at-286"), regular_journal);
    assert!(!store.workspace_path(&regular).unwrap().exists());
    // A damaged machine shows its problem; the list still works.
    fs::write(
        store.workspace_path("play").unwrap().join("session.toml"),
        "damaged",
    )
    .unwrap();
    let listed = machine::list(store).unwrap();
    assert_eq!(listed.len(), 3);
    let play = listed.iter().find(|l| l.id == "play").unwrap();
    assert!(play.problem.is_some());
    assert!(listed
        .iter()
        .filter(|l| l.id != "play")
        .all(|l| l.problem.is_none()));
}

fn pending_move(fixture: &Fixture, value: u8) -> String {
    let regular = format!("regular-{}", &fixture.image.digest.hex()[..56]);
    fixture.changed(&regular, value);
    // The record was written; the process stopped before the move.
    fixture
        .store
        .edit_machines(|machines| {
            let mut record = Machine::new(
                "Recovered",
                fixture.image.clone(),
                "test:source",
                Origin::Migration,
            );
            record.migrated_from = Some(regular.clone());
            machines.insert("recovered", record)
        })
        .unwrap();
    regular
}

#[test]
fn an_interrupted_migration_move_completes_on_the_next_run() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let regular = pending_move(&fixture, 21);
    // An empty directory at the id, left by an older build, is no obstacle.
    drop(store.workspace("recovered").unwrap());
    let migration = machine::migrate(store).unwrap();
    assert!(migration.adopted.is_empty() && migration.skipped.is_empty());
    assert!(!store.workspace_path(&regular).unwrap().exists());
    assert!(store
        .workspace_path("recovered")
        .unwrap()
        .join("media/medium-0.upfj")
        .exists());
    let listing = machine::get(store, "recovered").unwrap();
    assert!(listing.state.to_string().starts_with("Changed:"));
    assert_eq!(listing.machine.migrated_from, None);
}

#[test]
fn a_machine_whose_old_workspace_is_in_use_refuses_until_the_move_is_made() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let regular = pending_move(&fixture, 22);
    let id_directory = store.workspace_path("recovered").unwrap();
    let desktop = store.workspace(&regular).unwrap();
    let migration = machine::migrate(store).unwrap();
    assert!(migration.adopted.is_empty() && migration.skipped.is_empty());
    let refused = |error: pc_image::Error| {
        let message = error.to_string();
        assert!(message.contains("still being moved"), "{message}");
    };
    refused(machine::own(store, "recovered").err().unwrap());
    refused(machine::reset(store, "recovered").unwrap_err());
    let tables = machine::settings(store, "recovered").unwrap().machine;
    refused(
        machine::change_settings(store, "recovered", with_extended(&tables, 2048), true)
            .unwrap_err(),
    );
    refused(machine::set_pending_inserts(store, "recovered", Vec::new()).unwrap_err());
    refused(machine::commit(store, "recovered", "saved:1.0.0", None, None, false).unwrap_err());
    assert!(
        !id_directory.exists(),
        "no empty workspace is made at the id"
    );
    // A frontend that made the workspace itself is refused too.
    {
        let direct = store.workspace("recovered").unwrap();
        refused(machine::prepare(&direct).err().unwrap());
    }
    drop(desktop);
    let owner = machine::own(store, "recovered").unwrap();
    assert!(owner.path().join("media/medium-0.upfj").exists());
    assert!(!store.workspace_path(&regular).unwrap().exists());
    start(&owner, Some("now".into()));
    drop(owner);
    let listing = machine::get(store, "recovered").unwrap();
    assert_eq!(listing.machine.migrated_from, None);
    assert_eq!(listing.machine.last_run.as_deref(), Some("now"));
}

#[test]
fn settings_recompute_an_unjournaled_cmos_at_every_start_and_in_the_new_image() {
    let Some(fixture) = Fixture::without_cmos_journal() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Copied CMOS"), None).unwrap();
    let base = cmos(&fixture, &id);
    let tables = machine::settings(store, &id).unwrap().image;
    let change = machine::change_settings(store, &id, with_extended(&tables, 4096), true).unwrap();
    assert!(change.cmos_note.is_none());
    let mut expected = base;
    expected[0x17..=0x18].copy_from_slice(&4096u16.to_le_bytes());
    expected[0x30..=0x31].copy_from_slice(&4096u16.to_le_bytes());
    checksum(&mut expected);
    let offsets: Vec<usize> = change.cmos_changes.iter().map(|c| c.offset).collect();
    assert_eq!(
        offsets,
        (0..64)
            .filter(|&i| expected[i] != base[i])
            .collect::<Vec<_>>()
    );
    // Nothing is journaled; the machine's state is its settings alone.
    assert!(journals(store, &id).is_empty());
    assert_eq!(state(store, &id).to_string(), "Changed: settings");
    let rtc = |machine: &assembly::MachineConfig| {
        machine
            .devices
            .iter()
            .find_map(|d| match &d.kind {
                assembly::DeviceKind::Rtc(rtc) => Some(rtc.nvram.clone()),
                _ => None,
            })
            .unwrap()
    };
    {
        let owner = machine::own(store, &id).unwrap();
        let prepared = machine::prepare(&owner).unwrap();
        assert_eq!(
            rtc(&prepared.session.machine),
            assembly::NvramSource::Image(expected)
        );
    }
    machine::commit(store, &id, "copied:1.0.0", None, None, true).unwrap();
    let saved = store.resolve("copied:1.0.0", &PcProvider).unwrap();
    let d = saved
        .manifest
        .layers
        .iter()
        .find(|d| d.media_type == CMOS)
        .unwrap();
    assert_eq!(store.read_verified(d, 64).unwrap(), expected);
    // The source image keeps its own CMOS.
    assert_eq!(cmos(&fixture, &id), base);
}

/// Gaming mode on a machine plays a copy of the machine as it is: its
/// journals and settings, while the machine stays as it was. Keep as new machine… renames the copy into a machine of its own with
/// the source's settings and labels; Keep as new image… publishes a copy
/// with the source's settings.
#[test]
fn a_gaming_copy_plays_the_machine_as_it_is_and_is_kept_as_a_machine_or_an_image() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("DOS"), None).unwrap();
    fixture.changed(&id, 5);
    let tables = machine::settings(store, &id).unwrap().machine;
    machine::change_settings(store, &id, with_extended(&tables, 4096), true).unwrap();
    machine::set_labels(store, &id, vec!["games".into()]).unwrap();
    let before = machine::get(store, &id).unwrap();
    let bytes = |workspace: &str, name: &str| {
        fs::read(
            store
                .workspace_path(workspace)
                .unwrap()
                .join("media")
                .join(name),
        )
        .unwrap()
    };
    let copy_journals = |copy: &str| {
        journals(store, &id)
            .iter()
            .all(|name| bytes(copy, name) == bytes(&id, name))
    };
    for copy in ["disposable-copy-1", "disposable-copy-2"] {
        let source = machine::own(store, &id).unwrap();
        let owner = store.workspace(copy).unwrap();
        let prepared = machine::prepare_copy(&source, &owner).unwrap();
        assert_eq!(
            prepared.session.machine.memory.address_space_bytes,
            0x100000 + 4096 * 1024
        );
        drop(prepared);
        assert!(copy_journals(copy));
        // The copy records what it runs with.
        let record = machine::copy_of(&owner).unwrap().unwrap();
        assert_eq!(record.machine, id);
        assert_eq!(record.name, "DOS");
        assert_eq!(record.labels, ["games"]);
        assert_eq!(record.settings, before.machine.settings);
    }
    // The machine itself is unchanged by its copies.
    let after = machine::get(store, &id).unwrap();
    assert_eq!(after.machine.last_run, before.machine.last_run);
    assert_eq!(after.machine.settings, before.machine.settings);
    assert_eq!(after.changes, before.changes);
    // The machine changes after its copies were made, then goes: keeping and
    // publishing a copy still use what the copy ran with.
    machine::change_settings(store, &id, with_extended(&tables, 2048), true).unwrap();
    machine::set_labels(store, &id, vec!["other".into()]).unwrap();
    {
        // A power cycle prepares the copy again with its own record.
        let owner = store.workspace("disposable-copy-2").unwrap();
        let again = machine::resume_copy(&owner).unwrap();
        assert_eq!(
            again.session.machine.memory.address_space_bytes,
            0x100000 + 4096 * 1024
        );
    }
    machine::delete(store, &id).unwrap();
    // Keep as new machine…: the copy's workspace becomes the machine's.
    let kept =
        machine::keep_copy(store, "disposable-copy-1", "DOS saves", Some("now".into())).unwrap();
    assert!(!store.workspace_path("disposable-copy-1").unwrap().exists());
    let listing = machine::get(store, &kept).unwrap();
    assert_eq!(listing.machine.name, "DOS saves");
    assert_eq!(listing.machine.labels, ["games"]);
    assert_eq!(listing.machine.settings, before.machine.settings);
    assert_eq!(listing.machine.source, "test:source");
    assert_eq!(listing.machine.origin, Origin::Duplicate);
    assert!(!store
        .workspace_path(&kept)
        .unwrap()
        .join("copy-of.toml")
        .exists());
    assert!(machine::keep_copy(store, "disposable-copy-2", "dos saves", None).is_err());
    // Keep as new image…: the copy published with the settings it ran with.
    let owner = store.workspace("disposable-copy-2").unwrap();
    machine::publish_copy(&owner, "copy:1.0.0", None, false, Some("Copy")).unwrap();
    drop(owner);
    fixture.assert_complete("copy:1.0.0", 5);
    let lowered = pc_image::resolve(store, "copy:1.0.0", &fixture.path.join("c")).unwrap();
    assert_eq!(lowered.memory.address_space_bytes, 0x100000 + 4096 * 1024);
}

/// An interrupted "keeps its changes" Save as new image… is finished with the
/// choice it recorded; the default commit, which would move the machine,
/// refuses it.
#[test]
fn an_interrupted_save_that_keeps_the_changes_is_finished_as_it_was_chosen() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Keeper"), None).unwrap();
    fixture.changed(&id, 21);
    let interrupted =
        machine::commit_with_hook(store, &id, "kept:1.0.0", None, None, true, |step| {
            if step == workspace::PublicationStep::IntentStored {
                Err(pc_image::Error::Invalid("injected interruption".into()))
            } else {
                Ok(())
            }
        });
    assert!(interrupted.is_err());
    assert_eq!(
        machine::pending_commit(store, &id).unwrap(),
        Some(machine::PendingCommit {
            tag: "kept:1.0.0".into(),
            expected: None,
            keep: true,
        })
    );
    assert!(machine::get(store, &id).unwrap().publication_pending);
    assert!(machine::commit(store, &id, "kept:1.0.0", None, None, false).is_err());
    machine::finish_commit(store, &id).unwrap();
    assert_eq!(machine::pending_commit(store, &id).unwrap(), None);
    fixture.assert_complete("kept:1.0.0", 21);
    let listing = machine::get(store, &id).unwrap();
    assert_eq!(listing.machine.image.digest, fixture.image.digest);
    assert!(!listing.changes.is_empty(), "the machine keeps its changes");
    assert!(machine::finish_commit(store, &id).is_err());
}

/// Pending disks change one drive at a time on the record as it is, so
/// setting A: and then B: keeps both, and clearing one keeps the other.
#[test]
fn pending_disks_change_one_drive_at_a_time() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Drives"), None).unwrap();
    let insert = |drive: &str, medium: &str| image_store::machine::PendingInsert {
        drive: drive.into(),
        set: fixture.image.clone(),
        reference: "set:1".into(),
        medium: medium.into(),
        all: false,
        journal: true,
    };
    machine::set_pending_insert(store, &id, "A:", Some(insert("A:", "disk-1"))).unwrap();
    machine::set_pending_insert(store, &id, "B:", Some(insert("B:", "disk-2"))).unwrap();
    machine::set_pending_insert(store, &id, "A:", Some(insert("A:", "disk-3"))).unwrap();
    let pending = machine::get(store, &id).unwrap().machine.pending;
    assert_eq!(pending, [insert("B:", "disk-2"), insert("A:", "disk-3")]);
    machine::set_pending_insert(store, &id, "B:", None).unwrap();
    assert_eq!(
        machine::get(store, &id).unwrap().machine.pending,
        [insert("A:", "disk-3")]
    );
    assert!(machine::set_pending_insert(store, &id, "C:", None).is_err());
    assert!(machine::set_pending_insert(store, &id, "A:", Some(insert("B:", "disk-1"))).is_err());
}

/// Import the fixture ROMs `files` (vendor paths) as firmware packages of the
/// store, as Import ROM folder… does; `None` when a fixture is missing.
fn import_roms(fixture: &Fixture, files: &[&str]) -> Option<Vec<Vec<u8>>> {
    let folder = fixture.path.join("roms");
    fs::create_dir_all(&folder).unwrap();
    let mut roms = Vec::new();
    for (i, file) in files.iter().enumerate() {
        let bytes = fixtures::fixture(file)?;
        fs::write(folder.join(format!("rom-{i}.bin")), &bytes).unwrap();
        roms.push(bytes);
    }
    let catalogue = pc_image::firmware::catalogue();
    let scan = pc_image::firmware_import::scan(&[folder], catalogue).unwrap();
    assert_eq!(scan.candidates.len(), files.len(), "{:?}", scan.issues);
    for candidate in &scan.candidates {
        pc_image::firmware_import::publish(&fixture.store, candidate, &[], catalogue).unwrap();
    }
    Some(roms)
}
/// The machine's tables with the system BIOS bound to `bytes` instead.
fn with_bios(machine: &Table, bytes: &[u8]) -> Table {
    let mut machine = machine.clone();
    let sha256 = image_store::Digest::of(bytes).hex().to_owned();
    let bios = machine["firmware"]["bios"].as_table().unwrap().clone();
    let mut bios = bios;
    bios.insert(
        "image".into(),
        format!("assets/firmware-{}.bin", &sha256[..16]).into(),
    );
    bios.insert("sha256".into(), sha256.into());
    machine
        .get_mut("firmware")
        .and_then(Value::as_table_mut)
        .unwrap()
        .insert("bios".into(), bios.into());
    machine
}
fn bios_bytes(machine: &assembly::MachineConfig) -> Vec<u8> {
    machine
        .images
        .iter()
        .find_map(|image| match &image.source {
            assembly::ImageSource::Inline(bytes) if image.base + bytes.len() as u64 == 0x100000 => {
                Some(bytes.clone())
            }
            _ => None,
        })
        .unwrap()
}

#[test]
fn a_firmware_change_applies_at_the_next_start_and_keeps_or_resets_the_cmos() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let Some(roms) = import_roms(
        &fixture,
        &[
            "kiselev-bios/bios-xi8088-atreset.bin",
            "unemu-at-bios/bios-unemu-at.bin",
        ],
    ) else {
        return;
    };
    let (atreset, open_at) = (&roms[0], &roms[1]);
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Firmware"), None).unwrap();
    // The guest's own SETUP choice, kept in the machine's CMOS journal.
    fixture.changed_from(&id, "test:source", 0x5A);
    let before = cmos(&fixture, &id);
    assert_eq!(before[32], 0x5A);
    let image_tables = machine::settings(store, &id).unwrap().image;

    // A ROM that no package of the store holds cannot be chosen.
    let error = machine::change_settings(store, &id, with_bios(&image_tables, &[0; 32768]), false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no firmware package"), "{error}");

    // The same CMOS layout (Kiselev Xi 8088 to its AT-reset build): the
    // machine's CMOS stays, and the new ROM runs from the next start.
    let change =
        machine::change_settings(store, &id, with_bios(&image_tables, atreset), true).unwrap();
    assert!(!change.cmos_reset);
    assert_eq!(cmos(&fixture, &id), before);
    assert!(machine::get(store, &id).unwrap().machine.settings.is_some());
    {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        assert_eq!(&bios_bytes(&started.session.machine), atreset);
    }
    // The image never changes.
    let resolved = pc_image::resolve(store, "test:source", &fixture.path.join("s")).unwrap();
    assert_ne!(&bios_bytes(&resolved), atreset);

    // Another layout (the open AT BIOS): the CMOS is reset to the bytes the
    // configuration derives, as on a new machine, and the guest's choice is
    // gone.
    let change =
        machine::change_settings(store, &id, with_bios(&image_tables, open_at), true).unwrap();
    assert!(change.cmos_reset);
    assert_eq!(change.cmos_layout, Some(assembly::CmosLayout::OpenAtBios));
    let reset = cmos(&fixture, &id);
    assert_ne!(reset, before);
    let derived = {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        assert_eq!(&bios_bytes(&started.session.machine), open_at);
        assembly::derive_cmos_config(&started.session.machine).unwrap()
    };
    // Configuration RAM is the derived bytes; the clock, the status
    // registers and the shutdown byte stay the RTC's own.
    assert_eq!(reset[0x10..], derived[0x10..]);
    assert_eq!(reset[0x0E], derived[0x0E]);
    assert_eq!(reset[..0x0E], before[..0x0E]);
    assert_ne!(reset[32], 0x5A);

    // Save as new image… binds the chosen ROM in the new image.
    machine::commit(store, &id, "test:open-at", None, None, true).unwrap();
    let saved = pc_image::resolve(store, "test:open-at", &fixture.path.join("t")).unwrap();
    assert_eq!(&bios_bytes(&saved), open_at);

    // Back to the image's own firmware: the overlay goes, and the CMOS is
    // reset again for the image's layout.
    let change = machine::change_settings(store, &id, image_tables, true).unwrap();
    assert!(change.cmos_reset);
    assert_eq!(
        change.cmos_layout,
        Some(assembly::CmosLayout::KiselevXi8088)
    );
    assert!(machine::get(store, &id).unwrap().machine.settings.is_none());
}

#[test]
fn a_stored_cmos_copied_at_every_start_is_reset_for_another_layout() {
    let Some(fixture) = Fixture::without_cmos_journal() else {
        return;
    };
    let Some(roms) = import_roms(&fixture, &["unemu-at-bios/bios-unemu-at.bin"]) else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Stored"), None).unwrap();
    let tables = machine::settings(store, &id).unwrap().image;
    let change = machine::change_settings(store, &id, with_bios(&tables, &roms[0]), true).unwrap();
    assert!(change.cmos_reset);
    assert_eq!(change.cmos_layout, Some(assembly::CmosLayout::OpenAtBios));
    let nvram = |machine: &assembly::MachineConfig| {
        machine
            .devices
            .iter()
            .find_map(|d| match &d.kind {
                assembly::DeviceKind::Rtc(rtc) => match rtc.nvram {
                    assembly::NvramSource::Image(bytes) => Some(bytes),
                    _ => None,
                },
                _ => None,
            })
            .unwrap()
    };
    for _ in 0..2 {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        let bytes = nvram(&started.session.machine);
        let derived = assembly::derive_cmos_config(&started.session.machine).unwrap();
        assert_eq!(bytes[0x10..], derived[0x10..], "every start");
    }
    // A later change within the layout keeps resetting from the image's bytes.
    let change = machine::change_settings(
        store,
        &id,
        with_extended(&with_bios(&tables, &roms[0]), 2048),
        true,
    )
    .unwrap();
    assert!(!change.cmos_reset, "the machine's layout stays");
    // The bytes it reports are those every start now copies: the image's
    // stored bytes reset for the open AT BIOS, not recomputed SETUP fields.
    let owner = store.workspace(&id).unwrap();
    let started = start(&owner, None);
    let bytes = nvram(&started.session.machine);
    let stored = [0u8; 64];
    let changed: Vec<usize> = (0..64).filter(|&i| bytes[i] != stored[i]).collect();
    let reported: Vec<usize> = change.cmos_changes.iter().map(|c| c.offset).collect();
    assert_eq!(reported, changed);
    assert!(change.cmos_changes.iter().all(|c| c.new == bytes[c.offset]));
}

/// The firmware package of `bytes` in the store: its tag and manifest digest.
fn package_of(store: &Store, bytes: &[u8]) -> (String, image_store::Digest) {
    let sha256 = image_store::Digest::of(bytes);
    pc_image::create::packages(store)
        .unwrap()
        .into_iter()
        .find(|p| {
            p.config
                .roms
                .iter()
                .any(|r| r.blob.as_ref() == Some(&sha256))
        })
        .map(|p| (p.reference, p.descriptor.digest))
        .unwrap()
}

#[test]
fn a_machine_keeps_its_chosen_firmware_after_the_package_loses_its_tag() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let Some(roms) = import_roms(&fixture, &["unemu-at-bios/bios-unemu-at.bin"]) else {
        return;
    };
    let open_at = &roms[0];
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Chosen"), None).unwrap();
    let image_tables = machine::settings(store, &id).unwrap().image;
    machine::change_settings(store, &id, with_bios(&image_tables, open_at), true).unwrap();
    // The record keeps the binding; the sheet's tables never show it.
    let record = machine::get(store, &id).unwrap().machine.settings.unwrap();
    assert!(record.contains_key(pc_image::revision::BOUND_FIRMWARE));
    let blobs = pc_image::revision::bound_blobs(&record);
    assert_eq!(blobs.len(), 1);
    assert_eq!(
        blobs[0].decoded_digest().unwrap(),
        image_store::Digest::of(open_at)
    );
    let shown = machine::settings(store, &id).unwrap().machine;
    assert!(!shown.contains_key(pc_image::revision::BOUND_FIRMWARE));

    // Delete the package's only tag, as Delete tag… in the Firmware
    // collection does; the blobs stay in the store.
    let (tag, package) = package_of(store, open_at);
    store.remove_tag(&tag, &package).unwrap();
    assert!(pc_image::create::packages(store)
        .unwrap()
        .iter()
        .all(|p| p.descriptor.digest != package));

    // Start still binds the chosen ROM by digest.
    {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        assert_eq!(&bios_bytes(&started.session.machine), open_at);
    }
    // Save as new image… carries it, and names the package it came from.
    machine::commit(store, &id, "test:chosen", None, None, true).unwrap();
    let saved = pc_image::resolve(store, "test:chosen", &fixture.path.join("t")).unwrap();
    assert_eq!(&bios_bytes(&saved), open_at);
    let annotations = store
        .resolve("test:chosen", &pc_image::PcProvider)
        .unwrap()
        .manifest
        .annotations;
    assert!(
        annotations["unemu.firmware.sources"].contains(&package.to_string()),
        "{annotations:?}"
    );
    // Settings… OK with the sheet's tables (no bindings in them) works.
    machine::change_settings(
        store,
        &id,
        with_extended(&with_bios(&image_tables, open_at), 2048),
        true,
    )
    .unwrap();
    {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        assert_eq!(&bios_bytes(&started.session.machine), open_at);
    }
    // Clearing goes back to the image's firmware.
    let change = machine::clear_settings(store, &id).unwrap();
    assert!(change.cmos_reset, "back to the Kiselev layout");
    assert!(machine::get(store, &id).unwrap().machine.settings.is_none());
}

#[test]
fn settings_that_no_longer_bind_can_still_be_cleared_with_a_cmos_reset() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let Some(roms) = import_roms(&fixture, &["unemu-at-bios/bios-unemu-at.bin"]) else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Orphan"), None).unwrap();
    let image_tables = machine::settings(store, &id).unwrap().image;
    machine::change_settings(store, &id, with_bios(&image_tables, &roms[0]), true).unwrap();
    // A record without its bindings, whose package then loses its tag: its
    // tables name a ROM nothing binds any more.
    store
        .edit_machines(|machines| {
            let machine = machines.get_mut(&id).unwrap();
            if let Some(settings) = &mut machine.settings {
                settings.remove(pc_image::revision::BOUND_FIRMWARE);
            }
            Ok(())
        })
        .unwrap();
    let (tag, package) = package_of(store, &roms[0]);
    store.remove_tag(&tag, &package).unwrap();
    let error = machine::prepare(&store.workspace(&id).unwrap())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("no firmware package"), "{error}");
    // The image's own firmware always comes back, and the CMOS of a layout
    // nobody can tell now is reset.
    let change = machine::change_settings(store, &id, image_tables, true).unwrap();
    assert!(change.cmos_reset);
    assert!(machine::get(store, &id).unwrap().machine.settings.is_none());
    let owner = store.workspace(&id).unwrap();
    start(&owner, None);
}

#[test]
fn an_interrupted_cmos_reset_is_finished_at_the_next_start() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let Some(roms) = import_roms(&fixture, &["unemu-at-bios/bios-unemu-at.bin"]) else {
        return;
    };
    let store = &fixture.store;
    let id = machine::create_from_tag(store, "test:source", Some("Crash"), None).unwrap();
    fixture.changed_from(&id, "test:source", 0x5A);
    let image_tables = machine::settings(store, &id).unwrap().image;
    // The process dies once the record holds the new BIOS, before the CMOS
    // journal is reset.
    let crashed = machine::change_settings_with_hook(
        store,
        &id,
        with_bios(&image_tables, &roms[0]),
        true,
        |step| match step {
            machine::SettingsStep::Recorded => Err(pc_image::Error::Invalid("crash".into())),
        },
    );
    assert!(crashed.is_err());
    assert!(machine::get(store, &id).unwrap().machine.settings.is_some());
    assert_eq!(cmos(&fixture, &id)[32], 0x5A, "the old layout's bytes");
    assert!(machine::cmos_reset_pending(store, &id).unwrap());
    // Start finishes the reset before the new BIOS boots.
    let derived = {
        let owner = store.workspace(&id).unwrap();
        let started = start(&owner, None);
        assert_eq!(bios_bytes(&started.session.machine), roms[0]);
        assembly::derive_cmos_config(&started.session.machine).unwrap()
    };
    let bytes = cmos(&fixture, &id);
    assert_eq!(bytes[0x10..], derived[0x10..]);
    assert_ne!(bytes[32], 0x5A);
    assert!(!machine::cmos_reset_pending(store, &id).unwrap());
}
