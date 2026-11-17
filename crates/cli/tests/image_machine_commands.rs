// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image machine` mirrors the desktop's machine actions on the command line;
//! machine-images §3.2. The open Xi8088 BIOS and a journaled 360K floppy make
//! a small image; the runs are capped at a few instructions.

use image_store::machine::PendingInsert;
use pc_image::machine;
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "support/fixtures.rs"]
mod fixtures;

struct RemoveDir(PathBuf);
impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn run(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli::command::run(args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}
fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn machine_commands_parse_their_arguments_and_options() {
    for valid in [
        vec!["image", "machine", "ls", "--store", "s"],
        vec!["image", "machine", "new", "a:1", "--store", "s"],
        vec![
            "image",
            "machine",
            "new",
            "a:1",
            "--name",
            "DOS games",
            "--store",
            "s",
        ],
        vec![
            "image", "machine", "rename", "dos", "New name", "--store", "s",
        ],
        vec![
            "image",
            "machine",
            "duplicate",
            "dos",
            "--store",
            "s",
            "--name",
            "Copy",
        ],
        vec!["image", "machine", "reset", "dos", "--store", "s", "--yes"],
        vec!["image", "machine", "rm", "dos", "--store", "s"],
        vec!["image", "machine", "settings", "dos", "--store", "s"],
        vec![
            "image",
            "machine",
            "settings",
            "dos",
            "--store",
            "s",
            "--set",
            "memory.extended_kib=1024",
            "--set",
            "cpu.rate=8/13",
            "--dry-run",
        ],
        vec![
            "image", "machine", "settings", "dos", "--store", "s", "--clear",
        ],
        vec![
            "image", "machine", "commit", "dos", "--store", "s", "--tag", "a:2",
        ],
        vec![
            "image",
            "machine",
            "commit",
            "dos",
            "--store",
            "s",
            "--tag",
            "a:2",
            "--title",
            "Saved",
            "--keep-on-old-image",
        ],
        vec![
            "image",
            "machine",
            "run",
            "dos",
            "--store",
            "s",
            "--max-steps",
            "5",
        ],
    ] {
        assert!(cli::command::parse(&args(&valid)).is_ok(), "{valid:?}");
    }
    for invalid in [
        vec!["image", "machine"],
        vec!["image", "machine", "ls"],
        vec!["image", "machine", "start", "dos", "--store", "s"],
        vec!["image", "machine", "new", "--store", "s"],
        vec!["image", "machine", "rename", "dos", "--store", "s"],
        vec!["image", "machine", "reset", "--store", "s"],
        vec!["image", "machine", "ls", "--store", "s", "--yes"],
        vec!["image", "machine", "commit", "dos", "--store", "s"],
        vec![
            "image",
            "machine",
            "run",
            "dos",
            "--store",
            "s",
            "--max-steps",
            "0",
        ],
        vec![
            "image", "machine", "settings", "dos", "--store", "s", "--set", "=1",
        ],
        vec![
            "image", "machine", "settings", "dos", "--store", "s", "--set", "memory",
        ],
        vec![
            "image", "machine", "settings", "dos", "--store", "s", "--clear", "--set", "a.b=1",
        ],
        vec![
            "image",
            "machine",
            "new",
            "a:1",
            "--store",
            "s",
            "--keep-on-old-image",
        ],
    ] {
        assert!(cli::command::parse(&args(&invalid)).is_err(), "{invalid:?}");
    }
}

struct Lab {
    root: PathBuf,
    _cleanup: RemoveDir,
}
impl Lab {
    fn new() -> Option<Self> {
        let bios = fixtures::fixture("kiselev-bios/bios-xi8088.bin")?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "machine-command-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let cleanup = RemoveDir(root.clone());
        std::fs::write(root.join("bios.bin"), bios).unwrap();
        std::fs::write(root.join("disk.img"), vec![0; 360 * 1024]).unwrap();
        let file = root.join("machine.toml");
        std::fs::write(
            &file,
            "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='disk.img'\nwrite='journal'\njournal='source.upfj'\n",
        )
        .unwrap();
        let lab = Self {
            root,
            _cleanup: cleanup,
        };
        lab.ok(&[
            "image",
            "import",
            file.to_str().unwrap(),
            "--tag",
            "test:base",
            "--create",
        ]);
        Some(lab)
    }
    fn run(&self, list: &[&str]) -> (u8, String, String) {
        let mut list = args(list);
        list.extend(["--store".into(), self.root.clone().into_os_string()]);
        run(&list)
    }
    fn ok(&self, list: &[&str]) -> String {
        let (code, output, errors) = self.run(list);
        assert_eq!(code, 0, "{list:?}: {errors}");
        output
    }
    fn store(&self) -> image_store::Store {
        image_store::Store::open_existing(&self.root).unwrap()
    }
    fn row(&self, id: &str) -> Vec<String> {
        self.ok(&["image", "machine", "ls"])
            .lines()
            .find(|l| l.starts_with(&format!("{id}\t")))
            .unwrap_or_else(|| panic!("no machine {id}"))
            .split('\t')
            .map(str::to_owned)
            .collect()
    }
    fn journals(&self, id: &str) -> usize {
        std::fs::read_dir(self.root.join("workspaces").join(id).join("media"))
            .map(|entries| {
                entries
                    .filter(|e| {
                        e.as_ref()
                            .unwrap()
                            .path()
                            .extension()
                            .is_some_and(|x| x == "upfj")
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

/// Write one sector into a machine's floppy journal, as a guest would.
fn write_floppy(store: &image_store::Store, id: &str, value: u8) {
    let owner = machine::own(store, id).unwrap();
    let started = machine::prepare(&owner).unwrap();
    let medium = &started.session.machine.drives[0]
        .media
        .as_ref()
        .unwrap()
        .entries[0];
    let assembly::WriteMode::Journal { path, .. } = &medium.write else {
        panic!("journaled floppy expected")
    };
    let mut journal =
        media::JournalMedia::open(path, &[0; 360 * 1024], 360 * 1024, Default::default()).unwrap();
    journal.write_batch(&[(0, [value; 512])]);
    journal.finish().unwrap();
}

#[test]
fn every_machine_command_mirrors_its_desktop_action() {
    let Some(lab) = Lab::new() else {
        return;
    };
    // A workspace named on the command line becomes a machine of that name.
    lab.ok(&[
        "image",
        "run",
        "test:base",
        "--workspace",
        "play",
        "--max-steps",
        "1",
    ]);
    let (code, listed, errors) = lab.run(&["image", "machine", "ls"]);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(errors, "migrated:play\n");
    assert!(
        listed.starts_with("play\tplay\ttest:base\tsha256:"),
        "{listed}"
    );
    let (_, _, errors) = lab.run(&["image", "machine", "ls"]);
    assert!(errors.is_empty(), "migration runs once: {errors}");

    let created = lab.ok(&[
        "image",
        "machine",
        "new",
        "test:base",
        "--name",
        "DOS games",
    ]);
    assert_eq!(created, "dos-games\tDOS games\n");
    let row = lab.row("dos-games");
    assert_eq!(row[4], "Never started");
    assert_eq!(row[5], "last-run:never");
    assert_eq!(
        lab.ok(&["image", "machine", "new", "test:base"]),
        "test-base\ttest:base\n"
    );
    let (code, _, errors) = lab.run(&[
        "image",
        "machine",
        "new",
        "test:base",
        "--name",
        "dos GAMES",
    ]);
    assert_ne!(code, 0);
    assert!(errors.contains("exists already"), "{errors}");

    lab.ok(&["image", "machine", "rename", "dos-games", "Keen"]);
    assert_eq!(lab.row("dos-games")[1], "Keen");

    // Settings change this machine only, from its next start.
    let planned = lab.ok(&[
        "image",
        "machine",
        "settings",
        "dos-games",
        "--set",
        "memory.extended_kib=1024",
        "--dry-run",
    ]);
    assert!(planned.contains("+extended_kib = 1024"), "{planned}");
    assert_eq!(lab.row("dos-games")[4], "Never started");
    lab.ok(&[
        "image",
        "machine",
        "settings",
        "dos-games",
        "--set",
        "memory.extended_kib=1024",
    ]);
    assert_eq!(lab.row("dos-games")[4], "Changed: settings");
    let shown = lab.ok(&["image", "machine", "settings", "dos-games"]);
    assert!(
        shown.contains("[memory]\n") && shown.contains("+extended_kib = 1024"),
        "{shown}"
    );
    // --clear drops the overlay entirely.
    lab.ok(&["image", "machine", "settings", "dos-games", "--clear"]);
    assert_eq!(lab.row("dos-games")[4], "Never started");
    assert_eq!(lab.ok(&["image", "machine", "settings", "dos-games"]), "");
    lab.ok(&[
        "image",
        "machine",
        "settings",
        "dos-games",
        "--set",
        "memory.extended_kib=1024",
    ]);
    // The older workspace commands send a machine to the machine commands.
    for (old, hint) in [
        (
            vec![
                "image",
                "publish",
                "--from-workspace",
                "dos-games",
                "--tag",
                "x:1",
            ],
            "image machine commit dos-games",
        ),
        (
            vec!["image", "workspace", "discard", "dos-games", "--yes"],
            "image machine reset dos-games",
        ),
    ] {
        let (code, _, errors) = lab.run(&old);
        assert_ne!(code, 0);
        assert!(errors.contains(hint), "{errors}");
    }
    assert_eq!(lab.row("dos-games")[4], "Changed: settings");

    let output = lab.ok(&["image", "machine", "run", "dos-games", "--max-steps", "1"]);
    assert!(output.starts_with("pinned-image:sha256:"), "{output}");
    assert!(output.contains("\nmachine:dos-games\t"), "{output}");
    assert!(!lab.row("dos-games")[5].ends_with("never"));
    write_floppy(&lab.store(), "dos-games", 0x5A);
    assert_eq!(lab.row("dos-games")[4], "Changed: A:, settings");

    let copied = lab.ok(&["image", "machine", "duplicate", "dos-games"]);
    assert_eq!(copied, "keen-2\tKeen (2)\n");
    assert_eq!(lab.row("keen-2")[4], "Changed: A:, settings");

    // Save as new image… moves the machine onto the new image by default.
    let committed = lab.ok(&[
        "image",
        "machine",
        "commit",
        "dos-games",
        "--tag",
        "keen:1.0.0",
    ]);
    let mut lines = committed.lines();
    let digest = lines
        .next()
        .unwrap()
        .strip_prefix("keen:1.0.0\t")
        .unwrap()
        .to_owned();
    assert_eq!(
        lines.next().unwrap(),
        format!("machine:dos-games\tpinned-image:{digest}")
    );
    let row = lab.row("dos-games");
    assert_eq!(
        (row[2].as_str(), row[3].as_str()),
        ("keen:1.0.0", digest.as_str())
    );
    assert_eq!(row[4], "Same as image");
    // Keep on the old image: the copy keeps its changes and its image.
    let kept = lab.ok(&[
        "image",
        "machine",
        "commit",
        "keen-2",
        "--tag",
        "keen:1.1.0",
        "--keep-on-old-image",
    ]);
    assert!(!kept.contains(&digest));
    let row = lab.row("keen-2");
    assert_eq!(
        (row[2].as_str(), row[4].as_str()),
        ("test:base", "Changed: A:, settings")
    );

    // Reset and delete ask for --yes; the image always stays.
    let (code, shown, errors) = lab.run(&["image", "machine", "reset", "keen-2"]);
    assert_ne!(code, 0);
    assert!(errors.contains("--yes") && shown.starts_with("keen-2\t"));
    assert_eq!(lab.journals("keen-2"), 1);
    lab.ok(&["image", "machine", "reset", "keen-2", "--yes"]);
    assert_eq!(lab.journals("keen-2"), 0);
    assert_eq!(lab.row("keen-2")[4], "Never started");
    assert_ne!(lab.run(&["image", "machine", "rm", "test-base"]).0, 0);
    lab.ok(&["image", "machine", "rm", "test-base", "--yes"]);
    assert!(!lab.ok(&["image", "machine", "ls"]).contains("test-base\t"));
    assert!(lab.ok(&["image", "ls"]).contains("test:base\t"));

    // The older command still runs a machine's workspace, as that machine.
    let output = lab.ok(&[
        "image",
        "run",
        "test:base",
        "--workspace",
        "play",
        "--max-steps",
        "1",
    ]);
    assert!(output.contains("\nworkspace:play\t"), "{output}");
    assert!(!lab.row("play")[5].ends_with("never"));
    let (code, _, errors) = lab.run(&[
        "image",
        "run",
        "keen:1.0.0",
        "--workspace",
        "keen-2",
        "--max-steps",
        "1",
    ]);
    assert_ne!(code, 0);
    assert!(errors.contains("workspace pins"), "{errors}");
}

#[test]
fn a_pending_insert_goes_into_its_drive_at_the_next_start_and_is_consumed() {
    let Some(lab) = Lab::new() else {
        return;
    };
    std::fs::write(lab.root.join("setup.img"), vec![0xE5; 360 * 1024]).unwrap();
    lab.ok(&[
        "image",
        "media",
        "import",
        lab.root.join("setup.img").to_str().unwrap(),
        "--title",
        "Setup disks",
        "--tag",
        "setup:r1",
    ]);
    lab.ok(&[
        "image",
        "machine",
        "new",
        "test:base",
        "--name",
        "Installer",
    ]);
    let store = lab.store();
    let set = store.tag_descriptor("setup:r1").unwrap();
    let insert = PendingInsert {
        drive: "A:".into(),
        set: set.clone(),
        reference: "setup:r1".into(),
        medium: "disk-1".into(),
        all: false,
        journal: true,
    };
    let fixed_disk = PendingInsert {
        drive: "C:".into(),
        ..insert.clone()
    };
    assert!(machine::set_pending_inserts(&store, "installer", vec![fixed_disk]).is_err());
    machine::set_pending_inserts(&store, "installer", vec![insert.clone()]).unwrap();
    assert!(lab
        .row("installer")
        .contains(&"insert-pending:A:".to_owned()));
    let journal = lab
        .root
        .join("workspaces/installer/media")
        .join(format!("{}-disk-1.upfj", set.digest.hex()));
    {
        let owner = machine::own(&store, "installer").unwrap();
        let started = machine::prepare(&owner).unwrap();
        assert_eq!(started.inserted, std::slice::from_ref(&insert));
        let list = started.session.machine.drives[0].media.as_ref().unwrap();
        assert!(!list.empty);
        assert_eq!(list.entries.len(), 2, "the set's disk, then the image's");
        let first = &list.entries[0];
        let assembly::WriteMode::Journal { path, .. } = &first.write else {
            panic!("journaled insert expected")
        };
        assert_eq!(
            path,
            &owner
                .path()
                .join("media")
                .join(format!("{}-disk-1.upfj", set.digest.hex()))
        );
        let assembly::ImageSource::File(file) = &first.image else {
            panic!("store file expected")
        };
        assert!(started.session.bases.contains_key(file));
        // Nothing is consumed until the machine has assembled.
        assert!(lab
            .row("installer")
            .contains(&"insert-pending:A:".to_owned()));
        machine::confirm(&owner, &started.inserted, None).unwrap();
    }
    assert!(!lab
        .row("installer")
        .iter()
        .any(|c| c.starts_with("insert-pending")));
    machine::set_pending_inserts(&store, "installer", vec![insert]).unwrap();
    // An assembly failure (here: the insert's journal path is a directory)
    // keeps the pending insert and the last run.
    std::fs::create_dir_all(&journal).unwrap();
    let (code, output, errors) =
        lab.run(&["image", "machine", "run", "installer", "--max-steps", "1"]);
    assert_ne!(code, 0, "{output}");
    assert!(!errors.is_empty());
    let row = lab.row("installer");
    assert!(row.contains(&"insert-pending:A:".to_owned()), "{row:?}");
    assert_eq!(row[5], "last-run:never");
    std::fs::remove_dir(&journal).unwrap();
    let output = lab.ok(&["image", "machine", "run", "installer", "--max-steps", "1"]);
    assert!(output.contains("\ninserted:A:\tdisk-1\n"), "{output}");
    assert!(machine::get(&store, "installer")
        .unwrap()
        .machine
        .pending
        .is_empty());
    assert!(!lab.row("installer")[5].ends_with("never"));
    // Writes to an inserted disk show in the state, and Save as new image
    // warns that they are not part of the image.
    let mut writer = media::JournalMedia::open(
        &journal,
        &[0xE5; 360 * 1024],
        360 * 1024,
        Default::default(),
    )
    .unwrap();
    writer.write_batch(&[(0, [1; 512])]);
    writer.finish().unwrap();
    assert_eq!(lab.row("installer")[4], "Changed: inserted media");
    let listing = machine::get(&store, "installer").unwrap();
    assert_eq!(listing.changes[0].drive, machine::INSERTED);
    assert_eq!(listing.changes[0].sectors, Some(1));
    let (code, _, errors) = lab.run(&[
        "image",
        "machine",
        "commit",
        "installer",
        "--tag",
        "installer:1.0.0",
        "--keep-on-old-image",
    ]);
    assert_eq!(code, 0, "{errors}");
    assert!(
        errors.contains("warning: writes to inserted media are not saved into the image"),
        "{errors}"
    );
}

#[test]
fn a_damaged_workspace_is_reported_and_reserved_ids_are_never_handed_out() {
    let Some(lab) = Lab::new() else {
        return;
    };
    {
        let store = lab.store();
        let broken = store.workspace("broken").unwrap();
        std::fs::write(broken.path().join("session.toml"), "not a session").unwrap();
    }
    let (code, _, errors) = lab.run(&["image", "machine", "ls"]);
    assert_eq!(code, 0, "{errors}");
    assert!(errors.starts_with("migration-skipped:broken\t"), "{errors}");
    for (name, id) in [
        ("Fresh install", "m-fresh-install"),
        ("Regular", "m-regular"),
        ("Regular!", "m-regular-2"),
        ("Gaming rig", "m-gaming-rig"),
        ("Disposable", "m-disposable"),
    ] {
        assert_eq!(
            lab.ok(&["image", "machine", "new", "test:base", "--name", name]),
            format!("{id}\t{name}\n")
        );
    }
}

/// A machine whose image stores its CMOS: `image machine settings` says when
/// a system BIOS of another CMOS layout resets it.
#[test]
fn settings_say_when_another_bios_layout_resets_the_cmos() {
    let (Some(bios), Some(open_at)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("unemu-at-bios/bios-unemu-at.bin"),
    ) else {
        return;
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "machine-cmos-reset-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("roms")).unwrap();
    let _cleanup = RemoveDir(root.clone());
    std::fs::write(root.join("bios.bin"), bios).unwrap();
    std::fs::write(root.join("roms/open-at.bin"), &open_at).unwrap();
    std::fs::write(root.join("cmos.bin"), [0; 64]).unwrap();
    let file = root.join("machine.toml");
    std::fs::write(
        &file,
        "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='source-cmos.upfj'\n",
    )
    .unwrap();
    let store_root = root.join("store");
    let ok = |list: &[&str]| {
        let mut list = args(list);
        list.extend(["--store".into(), store_root.clone().into_os_string()]);
        let (code, output, errors) = run(&list);
        assert_eq!(code, 0, "{list:?}: {errors}");
        output
    };
    ok(&[
        "image",
        "import",
        file.to_str().unwrap(),
        "--tag",
        "test:cmos",
        "--create",
    ]);
    let store = image_store::Store::open_existing(&store_root).unwrap();
    let catalogue = pc_image::firmware::catalogue();
    let scan = pc_image::firmware_import::scan(&[root.join("roms")], catalogue).unwrap();
    pc_image::firmware_import::publish(&store, &scan.candidates[0], &[], catalogue).unwrap();
    ok(&["image", "machine", "new", "test:cmos", "--name", "Chosen"]);
    let sha256 = image_store::Digest::of(&open_at).hex().to_owned();
    let output = ok(&[
        "image",
        "machine",
        "settings",
        "chosen",
        "--set",
        &format!("firmware.bios.sha256={sha256}"),
        "--set",
        &format!("firmware.bios.image=assets/firmware-{}.bin", &sha256[..16]),
    ]);
    assert!(output.contains("cmos-reset:open AT BIOS"), "{output}");
    // Another setting within the layout keeps the CMOS.
    let output = ok(&[
        "image",
        "machine",
        "settings",
        "chosen",
        "--set",
        "memory.extended_kib=1024",
    ]);
    assert!(!output.contains("cmos-reset:"), "{output}");
}

/// An image a newer release tagged in a store both share: `machine new` and
/// `run` refuse it with the release it needs before its config is decoded,
/// and the listing shows the requirement. Without the marker the same image
/// is refused only by the decoder, after its config is read.
#[test]
fn an_image_that_needs_a_newer_release_is_refused_before_its_config_is_read() {
    use image_store::{compat, BlobSource, Descriptor, Manifest, MAX_SMALL_BLOB};
    let Some(lab) = Lab::new() else {
        return;
    };
    let store = lab.store();
    let base = store.tag_descriptor("test:base").unwrap();
    let manifest = Manifest::decode_with_types(
        &store.read_verified(&base, MAX_SMALL_BLOB).unwrap(),
        pc_image::PAYLOAD_TYPES,
    )
    .unwrap();
    let mut config = b"future_option = 1\n".to_vec();
    config.extend(
        store
            .read_verified(&manifest.config, MAX_SMALL_BLOB)
            .unwrap(),
    );
    let blob = store.import(config.as_slice(), None).unwrap();
    let newer = |marker: Option<&str>| {
        let mut newer = manifest.clone();
        newer.config = Descriptor {
            digest: blob.digest.clone(),
            size: blob.size,
            ..manifest.config.clone()
        };
        if let Some(marker) = marker {
            newer
                .annotations
                .insert(compat::MIN_APP_VERSION.into(), marker.into());
        }
        store
            .import_manifest_with_types(&newer, pc_image::PAYLOAD_TYPES)
            .unwrap()
    };
    store
        .tag_as_newer_release("newer:1.0.0", &newer(Some("99.0.0")))
        .unwrap();
    store
        .tag_as_newer_release("unmarked:1.0.0", &newer(None))
        .unwrap();
    let sentence = format!(
        "newer:1.0.0 needs unemupc 99.0.0 or newer (this is {})",
        compat::level()
    );
    for command in [
        &["image", "machine", "new", "newer:1.0.0"][..],
        &[
            "image",
            "run",
            "newer:1.0.0",
            "--workspace",
            "newer",
            "--max-steps",
            "1",
        ],
    ] {
        let (code, _, errors) = lab.run(command);
        assert_ne!(code, 0, "{command:?}");
        assert!(errors.contains(&sentence), "{command:?}: {errors}");
    }
    assert!(lab.ok(&["image", "machine", "ls"]).is_empty());
    // A machine a newer release made of the image shows the reason it cannot
    // start here.
    let newer_image = store.tag_descriptor("newer:1.0.0").unwrap();
    store
        .edit_machines(|machines| {
            machines.insert(
                "newer",
                image_store::machine::Machine::new(
                    "Newer",
                    Descriptor::new(
                        image_store::media_type::MANIFEST,
                        newer_image.digest,
                        newer_image.size,
                    ),
                    "newer:1.0.0",
                    image_store::machine::Origin::New,
                ),
            )
        })
        .unwrap();
    let machines = lab.ok(&["image", "machine", "ls"]);
    let line = machines.lines().find(|l| l.starts_with("newer\t")).unwrap();
    assert!(
        line.ends_with("\tmin-app-version:99.0.0\tNeeds unemupc 99.0.0 or newer"),
        "{machines}"
    );
    // Its config is not read: no decoder message.
    assert!(line.contains("needs unemupc 99.0.0 or newer"), "{line}");
    assert!(!line.contains("future_option"), "{line}");
    let listed = lab.ok(&["image", "ls"]);
    let line = |tag: &str| {
        listed
            .lines()
            .find(|l| l.starts_with(&format!("{tag}\t")))
            .unwrap()
            .to_owned()
    };
    assert!(
        line("newer:1.0.0").ends_with("\tmin-app-version:99.0.0\tNeeds unemupc 99.0.0 or newer"),
        "{listed}"
    );
    // An image written now names the `ibm-pc` platform, which 0.0.3 first
    // reads; this release meets that.
    assert!(
        line("test:base").ends_with("\tmin-app-version:0.0.3"),
        "{listed}"
    );

    let (code, _, errors) = lab.run(&[
        "image",
        "run",
        "unmarked:1.0.0",
        "--workspace",
        "unmarked",
        "--max-steps",
        "1",
    ]);
    assert_ne!(code, 0);
    assert!(errors.contains("unknown field `future_option`"), "{errors}");
    assert!(!errors.contains("needs unemupc"), "{errors}");
    // New machine from image… still makes a machine of it, as before.
    lab.ok(&["image", "machine", "new", "unmarked:1.0.0"]);
}

/// `image import --min-app-version` raises the image's minimum release, never
/// below what this release computes and never above this release. Every image
/// written now declares 0.0.3, the first release that reads its platform id,
/// and this release reads nothing newer, so the statement cannot raise an
/// image above what it computes until a row for 0.0.4 exists; it is checked
/// at 0.0.3, and the refusals on both sides.
#[test]
fn import_states_a_minimum_release_only_upwards() {
    use image_store::compat::{level, PACKAGE};
    let Some(lab) = Lab::new() else {
        return;
    };
    let file = lab.root.join("machine.toml");
    let file = file.to_str().unwrap();
    let raised = level().to_string();
    let newer =
        format!("--min-app-version is newer than {raised}, the newest release this unemupc reads");
    lab.ok(&[
        "image",
        "import",
        file,
        "--tag",
        "test:raised",
        "--min-app-version",
        &raised,
    ]);
    let listed = lab.ok(&["image", "ls"]);
    let line = listed
        .lines()
        .find(|l| l.starts_with("test:raised\t"))
        .unwrap();
    assert!(
        line.ends_with(&format!("\tmin-app-version:{raised}")),
        "{listed}"
    );
    for (stated, words) in [
        // Refused when the command line is read, before anything is copied.
        (
            "0.0.0",
            vec!["--min-app-version is lower than 0.0.1, the first release"],
        ),
        // Refused when the image is planned: below what the image needs.
        (
            "0.0.2",
            vec!["the stated minimum version 0.0.2 is lower than 0.0.3"],
        ),
        ("99.0.0", vec![newer.as_str()]),
    ] {
        let (code, _, errors) = lab.run(&[
            "image",
            "import",
            file,
            "--tag",
            "test:refused",
            "--min-app-version",
            stated,
        ]);
        assert_ne!(code, 0, "{stated}");
        for word in words {
            assert!(errors.contains(word), "{stated}: {errors}");
        }
    }
    assert!(!lab.ok(&["image", "ls"]).contains("test:refused"));
    // A machine shows what its image needs.
    lab.ok(&["image", "machine", "new", "test:raised", "--name", "Raised"]);
    let machines = lab.ok(&["image", "machine", "ls"]);
    let line = machines
        .lines()
        .find(|l| l.starts_with("raised\t"))
        .unwrap();
    assert!(
        line.ends_with(&format!("\tmin-app-version:{raised}")),
        "{machines}"
    );
    for bad in ["1.0", "1.0.0-rc.1", "v1.0.0"] {
        let (code, _, errors) = lab.run(&[
            "image",
            "import",
            file,
            "--tag",
            "test:bad",
            "--min-app-version",
            bad,
        ]);
        assert_ne!(code, 0);
        assert!(errors.contains("--min-app-version expects"), "{errors}");
    }
    assert!(level() >= PACKAGE);
}
