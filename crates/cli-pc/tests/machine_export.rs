// SPDX-License-Identifier: AGPL-3.0-or-later
//! Export a stopped library machine's current synthetic disk through the CLI.

#[allow(dead_code)]
#[path = "../../pc-image/tests/support/workspace.rs"]
mod support;

#[path = "support/export.rs"]
mod export_support;
use export_support::Export;
use image_store::Digest;
use journal_files::SectorSource;
use pc_image::machine;
use std::ffi::OsString;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn machine(f: &support::Fixture) -> String {
    machine::create_from_tag(&f.store, "test:source", Some("export"), None).unwrap()
}

fn files(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, bool)> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            for (path, value) in files(&entry.path()) {
                result.insert(PathBuf::from(entry.file_name()).join(path), value);
            }
        } else {
            result.insert(
                PathBuf::from(entry.file_name()),
                (
                    fs::read(entry.path()).unwrap(),
                    entry.metadata().unwrap().permissions().readonly(),
                ),
            );
        }
    }
    result
}

fn exported(f: &support::Fixture, id: &str, slot: &str, name: &str) -> (PathBuf, String) {
    let directory = f.path.join(name);
    let options = [
        "--format",
        "raw",
        "--slot",
        slot,
        "--to",
        directory.to_str().unwrap(),
    ];
    let (code, out, err) = run(f, id, &options);
    assert_eq!(code, 0, "{err}");
    let name = out.lines().next().unwrap().split('\t').next().unwrap();
    (directory.join(name), out)
}

fn run(f: &support::Fixture, id: &str, options: &[&str]) -> (u8, String, String) {
    let mut args: Vec<OsString> = ["image", "machine", "export", id]
        .into_iter()
        .chain(options.iter().copied())
        .map(Into::into)
        .collect();
    args.push("--store".into());
    args.push(f.store.root().into());
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(&args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

#[test]
fn lists_current_media() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id =
        pc_image::machine::create_from_tag(&f.store, "test:source", Some("export"), None).unwrap();
    let (code, out, err) = run(&f, &id, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("format:raw\tRaw disk image\t.img\tslot\tavailable"),
        "{out}"
    );
    assert!(out.contains("format:machine\tPC machine file\t.toml\tmachine\tavailable"));
    assert!(out.contains("A:\tfloppy\t368640\t5.25-inch 360K"));
    assert!(out.contains("C:\tfixed\t16384\tCHS"));
}

#[test]
fn exports_unchanged_raw_and_prints_its_identity_without_changing_the_store() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let before = files(f.store.root());
    let (path, out) = exported(&f, &id, "C:", "out.img");
    let raw = fs::read(&path).unwrap();
    assert_eq!(raw, fs::read(f.path.join("fixed.img")).unwrap());
    assert!(
        out.starts_with(&format!("slot-2.img\tbytes:16384\t{}\n", Digest::of(&raw))),
        "{out}"
    );
    assert!(out.contains("note:"));
    assert_eq!(before, files(f.store.root()));
}

#[test]
fn journal_export_matches_sector_source_and_restored_writes_are_not_changes() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    f.changed(&id, 0x42);
    let raw = fs::read(f.path.join("fixed.img")).unwrap();
    let journal_path = f.store.workspace_path(&id).unwrap().join("media");
    let config = f
        .store
        .resolve("test:source", &pc_image::PcProvider)
        .unwrap();
    let config: toml::Table = std::str::from_utf8(&config.config)
        .unwrap()
        .parse()
        .unwrap();
    let medium = config["slots"].as_array().unwrap().last().unwrap()["inserted"]
        .as_str()
        .unwrap();
    let journal_path = journal_path.join(format!("{medium}.upfj"));
    let verified = journal_files::VerifiedBase::from_bytes(&raw).unwrap();
    let scan = journal_files::scan_overlay_reader(
        &mut fs::File::open(&journal_path).unwrap(),
        &verified,
        raw.len() as u64,
    )
    .unwrap();
    let overlay = scan.overlay.unwrap();
    let expected: Vec<u8> = (0..32)
        .flat_map(|lba| overlay.read_sector(lba).unwrap())
        .collect();
    let before = files(f.store.root());
    let (path, _) = exported(&f, &id, "C:", "current.img");
    assert_eq!(fs::read(path).unwrap(), expected);
    assert_eq!(before, files(f.store.root()));
    let (code, out, err) = run(&f, &id, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("C:\tfixed\t16384\tCHS"));
    let mut journal = journal_files::JournalMedia::open(
        &journal_path,
        &raw,
        raw.len() as u64,
        Default::default(),
    )
    .unwrap();
    journal.write_batch(&[(0, [0; 512])]);
    journal.finish().unwrap();
    let (code, out, err) = run(&f, &id, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("C:\tfixed\t16384\tCHS"));
}

#[test]
fn exports_layered_head_then_applies_its_workspace_journal() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    f.changed(&id, 0x23);
    machine::commit(&f.store, &id, "test:layered", None, None, false).unwrap();
    let (path, _) = exported(&f, &id, "C:", "layer.img");
    let head = fs::read(path).unwrap();
    assert_eq!(&head[..512], &[0x23; 512]);
    assert_eq!(&head[512..], &vec![0; 31 * 512]);
    pc_image::compress_image(&f.store, "test:layered", "test:compressed").unwrap();
    let compressed =
        machine::create_from_tag(&f.store, "test:compressed", Some("compressed"), None).unwrap();
    let before = files(f.store.root());
    let (path, _) = exported(&f, &compressed, "C:", "compressed.img");
    assert_eq!(fs::read(path).unwrap(), head);
    assert_eq!(before, files(f.store.root()));
    let owner = machine::own(&f.store, &id).unwrap();
    let prepared = machine::prepare(&owner).unwrap();
    let drive = prepared.session.machine.drives.last().unwrap();
    let assembly::WriteMode::Journal { path, .. } = &drive.write else {
        panic!("journal expected")
    };
    let mut journal =
        journal_files::JournalMedia::open(path, &head, head.len() as u64, Default::default())
            .unwrap();
    journal.write_batch(&[(1, [0x91; 512])]);
    journal.finish().unwrap();
    drop(owner);
    let base = journal_files::VerifiedBase::from_bytes(&head).unwrap();
    let scan = journal_files::scan_overlay_reader(
        &mut fs::File::open(path).unwrap(),
        &base,
        head.len() as u64,
    )
    .unwrap();
    let overlay = scan.overlay.unwrap();
    let expected: Vec<_> = (0..32)
        .flat_map(|lba| overlay.read_sector(lba).unwrap())
        .collect();
    let before = files(f.store.root());
    let (path, _) = exported(&f, &id, "C:", "layer-current.img");
    assert_eq!(fs::read(path).unwrap(), expected);
    assert_eq!(before, files(f.store.root()));
}

#[test]
fn refuses_running_machine_and_holds_the_lock_until_export_finishes() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let owner = machine::own(&f.store, &id).unwrap();
    let target = f.path.join("slot-2.img");
    for options in [
        vec![],
        vec![
            "--format",
            "raw",
            "--slot",
            "C:",
            "--to",
            f.path.to_str().unwrap(),
        ],
    ] {
        let (code, _, err) = run(&f, &id, &options);
        assert_ne!(code, 0);
        assert!(err.contains("is running; stop it first"), "{err}");
    }
    assert!(!target.exists());
    drop(owner);
    let snapshot = machine::export::open(&f.store, &id).unwrap();
    assert!(f.store.workspace(&id).is_err());
    drop(snapshot);
    assert!(f.store.workspace(&id).is_ok());
}

#[test]
fn refuses_recovery_without_modifying_the_journal_or_creating_outputs() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    f.changed(&id, 2);
    let config = f
        .store
        .resolve("test:source", &pc_image::PcProvider)
        .unwrap();
    let config: toml::Table = std::str::from_utf8(&config.config)
        .unwrap()
        .parse()
        .unwrap();
    let medium = config["slots"].as_array().unwrap().last().unwrap()["inserted"]
        .as_str()
        .unwrap();
    let journal = f
        .store
        .workspace_path(&id)
        .unwrap()
        .join(format!("media/{medium}.upfj"));
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(b"torn frame")
        .unwrap();
    let before = files(f.store.root());
    let target = f.path.join("slot-2.img");
    let (code, _, err) = run(&f, &id, &[]);
    assert_eq!(
        code, 0,
        "config-only listing must not read the journal: {err}"
    );
    let (code, _, err) = run(
        &f,
        &id,
        &[
            "--format",
            "raw",
            "--slot",
            "C:",
            "--to",
            f.path.to_str().unwrap(),
        ],
    );
    assert_ne!(code, 0);
    assert!(err.contains("needs recovery"), "{err}");
    assert!(!target.exists());
    assert_eq!(before, files(f.store.root()));
}

#[test]
fn refuses_existing_outputs_including_dangling_symlinks() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let path = f.path.join("slot-2.img");
    fs::write(&path, b"keep").unwrap();
    let (code, _, err) = run(
        &f,
        &id,
        &[
            "--format",
            "raw",
            "--slot",
            "C:",
            "--to",
            f.path.to_str().unwrap(),
        ],
    );
    assert_ne!(code, 0);
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(fs::read(&path).unwrap(), b"keep");
    #[cfg(unix)]
    {
        fs::remove_file(&path).unwrap();
        let absent = f.path.join("absent");
        std::os::unix::fs::symlink(&absent, &path).unwrap();
        let (code, _, err) = run(
            &f,
            &id,
            &[
                "--format",
                "raw",
                "--slot",
                "C:",
                "--to",
                f.path.to_str().unwrap(),
            ],
        );
        assert_ne!(code, 0);
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(fs::read_link(&path).unwrap(), absent);
        assert!(!absent.exists());
    }
}

#[test]
fn removes_partial_output_when_streaming_fails() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let snapshot = machine::export::open(&f.store, &id).unwrap();
    let path = f.path.join("slot-0.img");
    let before = files(f.store.root());
    let error = snapshot
        .export_with_hook(&f.store, "A:", &f.path, |written| {
            assert!(written > 0);
            assert!(!path.exists());
            let partial = fs::read_dir(&f.path)
                .unwrap()
                .map(|entry| entry.unwrap())
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".slot-0.img.unemu-export-")
                })
                .unwrap();
            assert_eq!(partial.metadata().unwrap().len(), written);
            Err(std::io::Error::other("injected output failure"))
        })
        .unwrap_err();
    assert!(error.to_string().contains("injected output failure"));
    assert!(!path.exists());
    assert!(!fs::read_dir(&f.path).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));
    assert_eq!(before, files(f.store.root()));
    let (_, _, err) = run(&f, &id, &[]);
    assert!(err.is_empty(), "{err}");
}

#[test]
fn compacted_journal_exports_match_random_access_run_reads() {
    for kind in ["raw", "layered", "compressed"] {
        let Some(f) = support::Fixture::new() else {
            return;
        };
        let mut id = machine(&f);
        if kind != "raw" {
            f.changed(&id, 0x23);
            machine::commit(&f.store, &id, "test:layered", None, None, false).unwrap();
            if kind == "compressed" {
                pc_image::compress_image(&f.store, "test:layered", "test:compressed").unwrap();
                id = machine::create_from_tag(&f.store, "test:compressed", None, None).unwrap();
            }
        }
        let owner = machine::own(&f.store, &id).unwrap();
        let prepared = machine::prepare(&owner).unwrap();
        let drive = prepared.session.machine.drives.last().unwrap();
        let assembly::ImageSource::File(base_path) = &drive.image else {
            panic!("file-backed disk expected")
        };
        let assembly::WriteMode::Journal { path, .. } = &drive.write else {
            panic!("journal expected")
        };
        let base = journal_files::VerifiedBase::open(base_path).unwrap();
        let mut journal = journal_files::JournalMedia::open_verified(
            path,
            &base,
            base.byte_len(),
            Default::default(),
        )
        .unwrap()
        .with_retention(2);
        journal.write_batch(&[(7, [0x71; 512]), (22, [0x22; 512])]);
        journal.checkpoint().unwrap();
        for value in 1..=5 {
            journal.write_batch(&[(20, [value; 512])]);
            journal.checkpoint().unwrap();
        }
        journal.finish().unwrap();
        let scan = journal_files::scan_overlay_reader(
            &mut fs::File::open(path).unwrap(),
            &base,
            base.byte_len(),
        )
        .unwrap();
        assert!(!scan.report.needs_recovery());
        assert!(scan.report.header.unwrap().anchor_generation > 0);
        let overlay = scan.overlay.unwrap();
        let expected: Vec<_> = (0..base.byte_len() / 512)
            .flat_map(|lba| overlay.read_sector(lba).unwrap())
            .collect();
        assert_eq!(&expected[7 * 512..8 * 512], &[0x71; 512]);
        drop(owner);
        let before = files(f.store.root());
        let (code, out, err) = run(&f, &id, &[]);
        assert_eq!(code, 0, "{kind}: {err}");
        assert!(out.contains("C:\tfixed\t16384\tCHS"));
        let (path, _) = exported(&f, &id, "C:", "compacted.img");
        assert_eq!(fs::read(path).unwrap(), expected, "{kind}");
        assert_eq!(before, files(f.store.root()));
    }
}

#[test]
fn export_reads_settings_only_after_acquiring_the_workspace_lock() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let mut settings = machine::settings(&f.store, &id).unwrap().machine;
    let controller = settings["card"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c["kind"].as_str() == Some("fdc-765"))
        .unwrap();
    let drives = controller
        .get_mut("drives")
        .unwrap()
        .as_array_mut()
        .unwrap();
    let mut second = drives[0].clone();
    second
        .as_table_mut()
        .unwrap()
        .insert("unit".into(), 1.into());
    drives.push(second);
    let mut before = None;
    let owner = machine::own(&f.store, &id).unwrap();
    let snapshot = machine::export::open_with_hook(&f.store, &id, || {
        f.store.edit_machines(|machines| {
            machines.get_mut(&id).unwrap().settings = Some(settings);
            Ok(())
        })?;
        drop(owner);
        before = Some(files(f.store.root()));
        Ok(())
    })
    .unwrap();
    let rows = snapshot.media(&f.store).unwrap();
    assert!(rows.iter().any(|row| row.slot == "B:"), "stale settings");
    assert!(machine::own(&f.store, &id).is_err());
    snapshot.export(&f.store, "C:", &f.path).unwrap();
    assert_eq!(before.unwrap(), files(f.store.root()));
}

#[test]
fn streaming_failure_preserves_a_final_created_by_another_party() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let snapshot = machine::export::open(&f.store, &id).unwrap();
    let path = f.path.join("slot-0.img");
    let before = files(f.store.root());
    let error = snapshot
        .export_with_hook(&f.store, "A:", &f.path, |written| {
            assert!(written > 0);
            assert!(!path.exists());
            fs::write(&path, b"replacement must survive").unwrap();
            Err(std::io::Error::other("injected output failure"))
        })
        .err()
        .unwrap();
    assert!(path.exists(), "cleanup deleted the replacement");
    assert_eq!(fs::read(&path).unwrap(), b"replacement must survive");
    assert!(error.to_string().contains("injected output failure"));
    assert_eq!(before, files(f.store.root()));
}

#[test]
fn repeat_export_refuses_completed_output_without_overwriting() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let (path, _) = exported(&f, &id, "A:", "output");
    let before = fs::read(&path).unwrap();
    let (code, _, err) = run(
        &f,
        &id,
        &[
            "--format",
            "raw",
            "--slot",
            "A:",
            "--to",
            path.parent().unwrap().to_str().unwrap(),
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("already exists"));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn rejects_unknown_ids_slots_and_incomplete_options() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let target = f.path.join("slot-2.img");
    let (code, _, err) = run(&f, "missing", &[]);
    assert_ne!(code, 0);
    assert!(err.contains("unemu image machine ls"), "{err}");
    let (code, _, err) = run(
        &f,
        &id,
        &[
            "--format",
            "raw",
            "--slot",
            "unknown",
            "--to",
            f.path.to_str().unwrap(),
        ],
    );
    assert_ne!(code, 0);
    assert!(
        err.contains("no slot unknown; omit --slot to list media"),
        "{err}"
    );
    assert!(!target.exists());
    for options in [
        vec!["--slot", "C:"],
        vec!["--to", "out.img"],
        vec!["--slot", "C:", "--slot", "C:", "--to", "out.img"],
    ] {
        let (code, _, _) = run(&f, &id, &options);
        assert_eq!(code, 2);
    }
    let args: Vec<OsString> = ["image", "machine", "export", "--store", "s"]
        .into_iter()
        .map(Into::into)
        .collect();
    assert!(cli_pc::command::parse(&args)
        .unwrap_err()
        .contains("unemu image machine ls"));
}

#[test]
fn uses_the_executables_default_store_and_rejects_store_destinations() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let args: Vec<OsString> = ["image", "machine", "export", &id]
        .into_iter()
        .map(Into::into)
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run_with_firmware_sources(
        &args,
        &mut out,
        &mut err,
        &cli_pc::command::FirmwareSources {
            store_root: Some(f.store.root().to_owned()),
            ..Default::default()
        },
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    assert!(String::from_utf8(out).unwrap().contains("C:\tfixed\t16384"));
    let target = f.store.root().join("slot-2.img");
    let (code, _, err) = run(
        &f,
        &id,
        &[
            "--format",
            "raw",
            "--slot",
            "C:",
            "--to",
            target.parent().unwrap().to_str().unwrap(),
        ],
    );
    assert_ne!(code, 0);
    assert!(err.contains("outside the read-only store"), "{err}");
    assert!(!target.exists());
}

#[test]
fn lists_empty_slots_and_refuses_to_export_them() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let mut settings = machine::settings(&f.store, &id).unwrap().machine;
    let cards = settings.get_mut("card").unwrap().as_array_mut().unwrap();
    let controller = cards
        .iter_mut()
        .find(|c| c["kind"].as_str() == Some("fdc-765"))
        .unwrap();
    let drives = controller
        .get_mut("drives")
        .unwrap()
        .as_array_mut()
        .unwrap();
    let mut empty = drives[0].clone();
    empty
        .as_table_mut()
        .unwrap()
        .insert("unit".into(), 1.into());
    drives.push(empty);
    machine::change_settings(&f.store, &id, settings, true).unwrap();
    let (code, out, err) = run(&f, &id, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("B:\tfloppy\t"), "{out}");
    let target = f.path.join("slot-2.img");
    let (code, _, err) = run(
        &f,
        &id,
        &[
            "--format",
            "raw",
            "--slot",
            "B:",
            "--to",
            f.path.to_str().unwrap(),
        ],
    );
    assert_ne!(code, 0);
    assert!(err.contains("slot B: is empty"), "{err}");
    assert!(!target.exists());
}

#[test]
fn refuses_a_damaged_base_without_writing_output() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let digest = Digest::of(&fs::read(f.path.join("fixed.img")).unwrap());
    let blob = f.store.blob_file_path(&digest, 16384).unwrap();
    let mut damaged = fs::read(&blob).unwrap();
    damaged[700] ^= 1;
    fs::write(&blob, damaged).unwrap();
    let target = f.path.join("slot-2.img");
    let (code, _, err) = run(
        &f,
        &id,
        &[
            "--format",
            "raw",
            "--slot",
            "C:",
            "--to",
            target.parent().unwrap().to_str().unwrap(),
        ],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("digest mismatch") || err.contains("sha256"),
        "{err}"
    );
    assert!(!target.exists());
}

#[test]
fn pending_insert_export_matches_what_machine_run_will_mount() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let raw = vec![0xE5; 360 * 1024];
    let other = f.path.join("other.img");
    fs::write(&other, vec![0xA1; raw.len()]).unwrap();
    let disk = f.path.join("setup.img");
    fs::write(&disk, &raw).unwrap();
    let args: Vec<OsString> = [
        "image",
        "media",
        "import",
        other.to_str().unwrap(),
        disk.to_str().unwrap(),
        "--title",
        "Setup disks",
        "--tag",
        "setup:r1",
        "--store",
        f.store.root().to_str().unwrap(),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        cli_pc::command::run(&args, &mut out, &mut err),
        0,
        "{}",
        String::from_utf8_lossy(&err)
    );
    let set = f.store.tag_descriptor("setup:r1").unwrap();
    let insert = image_store::machine::PendingInsert {
        drive: "A:".into(),
        set,
        reference: "setup:r1".into(),
        medium: "disk-2".into(),
        all: true,
        journal: true,
    };
    machine::set_pending_insert(&f.store, &id, "A:", Some(insert.clone())).unwrap();
    let owner = machine::own(&f.store, &id).unwrap();
    let prepared = machine::prepare(&owner).unwrap();
    let drive = &prepared.session.machine.drives[0];
    let list = drive.media.as_ref().unwrap();
    let selected = &list.entries[list.inserted];
    let assembly::ImageSource::File(base) = &selected.image else {
        panic!("file-backed pending medium expected")
    };
    assert_eq!(fs::read(base).unwrap(), raw);
    let assembly::WriteMode::Journal { path, .. } = &selected.write else {
        panic!("journal expected")
    };
    let mut journal =
        journal_files::JournalMedia::open(path, &raw, raw.len() as u64, Default::default())
            .unwrap();
    journal.write_batch(&[(2, [0x72; 512])]);
    journal.finish().unwrap();
    let base = journal_files::VerifiedBase::from_bytes(&raw).unwrap();
    let overlay = journal_files::scan_overlay_reader(
        &mut fs::File::open(path).unwrap(),
        &base,
        raw.len() as u64,
    )
    .unwrap()
    .overlay
    .unwrap();
    let expected: Vec<_> = (0..raw.len() as u64 / 512)
        .flat_map(|lba| overlay.read_sector(lba).unwrap())
        .collect();
    drop(owner);
    let before = files(f.store.root());
    let (code, out, err) = run(&f, &id, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("A:\tfloppy\t368640\t5.25-inch 360K"), "{out}");
    let (path, _) = exported(&f, &id, "A:", "pending.img");
    assert_eq!(fs::read(path).unwrap(), expected);
    assert_eq!(before, files(f.store.root()));
    assert_eq!(
        machine::get(&f.store, &id).unwrap().machine.pending,
        vec![insert.clone()]
    );
    let readonly = image_store::machine::PendingInsert {
        journal: false,
        ..insert
    };
    machine::set_pending_insert(&f.store, &id, "A:", Some(readonly)).unwrap();
    let (path, _) = exported(&f, &id, "A:", "readonly-pending.img");
    assert_eq!(
        fs::read(path).unwrap(),
        raw,
        "a read-only next run must ignore an old journal"
    );
}

#[test]
fn desktop_kept_selection_is_ignored_as_it_is_by_machine_run() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let owner = machine::own(&f.store, &id).unwrap();
    let folder = owner.path().join("media/created");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("catalogue.toml"), "schema=1\n[[drive]]\ndrive='A:'\nin_drive=1\n[[drive.disk]]\nkind='own'\nname='one.img'\n[[drive.disk]]\nkind='own'\nname='two.img'\n").unwrap();
    let prepared = machine::prepare(&owner).unwrap();
    let list = prepared.session.machine.drives[0].media.as_ref().unwrap();
    assert_eq!(list.entries[list.inserted].name, "one.img");
    drop(owner);
    let before = files(f.store.root());
    let (code, out, err) = run(&f, &id, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("A:\tfloppy\t368640"), "{out}");
    let (path, _) = exported(&f, &id, "A:", "configured.img");
    assert_eq!(
        fs::read(path).unwrap(),
        fs::read(f.path.join("one.img")).unwrap()
    );
    assert_eq!(before, files(f.store.root()));
}

#[test]
fn contract_formats_export_current_bytes_and_loadable_machine() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let (code, listed, err) = run(&f, &id, &[]);
    assert_eq!(code, 0, "{err}");
    for format in ["raw", "vhd-fixed", "machine"] {
        assert!(listed.contains(&format!("format:{format}\t")), "{listed}");
        let dir = f.path.join(format);
        fs::create_dir(&dir).unwrap();
        let mut options = vec!["--format", format, "--to", dir.to_str().unwrap()];
        if format != "machine" {
            options.extend(["--slot", "C:"]);
        }
        let (code, out, err) = run(&f, &id, &options);
        assert_eq!(code, 0, "{format}: {err}");
        assert!(out.contains("sha256:"), "{out}");
        let paths: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let raw = fs::read(f.path.join("fixed.img")).unwrap();
        if format == "raw" {
            assert_eq!(fs::read(&paths[0]).unwrap(), raw);
        }
        if format == "vhd-fixed" {
            let vhd = fs::read(&paths[0]).unwrap();
            assert_eq!(vhd.len(), raw.len() + 512);
            assert_eq!(&vhd[..raw.len()], raw);
            assert_eq!(&vhd[raw.len()..raw.len() + 8], b"conectix");
        }
        if format == "machine" {
            let document = paths
                .iter()
                .find(|p| p.extension().is_some_and(|e| e == "toml"))
                .unwrap();
            struct Reader<'a>(&'a support::Fixture);
            impl machine_file::ImageReader for Reader<'_> {
                fn read(&mut self, path: &Path) -> std::io::Result<Vec<u8>> {
                    if path.is_file() {
                        return fs::read(path);
                    }
                    let digest: Digest =
                        format!("sha256:{}", path.file_name().unwrap().to_str().unwrap())
                            .parse()
                            .unwrap();
                    for name in ["bios.bin", "xtide.bin"] {
                        let bytes = fs::read(self.0.path.join(name))?;
                        if Digest::of(&bytes) == digest {
                            return Ok(bytes);
                        }
                    }
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "unbound firmware",
                    ))
                }
            }
            let loaded = machine_file::load_with_reader(document, &mut Reader(&f)).unwrap();
            assert_eq!(loaded.drives.len(), 2);
            let text = fs::read_to_string(document).unwrap();
            for path in paths
                .iter()
                .filter(|p| p.extension().is_some_and(|e| e == "img"))
            {
                assert!(text.contains(path.file_name().unwrap().to_str().unwrap()));
            }
            for drive in &loaded.drives {
                let image = drive
                    .media
                    .as_ref()
                    .map(|media| &media.entries[media.inserted].image)
                    .unwrap_or(&drive.image);
                let bytes = match image {
                    assembly::ImageSource::File(path) => fs::read(path).unwrap(),
                    assembly::ImageSource::Inline(bytes) => bytes.clone(),
                    _ => panic!("exported medium must have current bytes"),
                };
                assert!(paths
                    .iter()
                    .filter(|p| p.extension().is_some_and(|e| e == "img"))
                    .any(|p| fs::read(p).unwrap() == bytes));
            }
            assert!(paths.len() >= 3);
        }
        assert!(out.contains("note:"), "{out}");
        let snapshot = machine::export::open(&f.store, &id).unwrap();
        let source = export_support::Source {
            snapshot: &snapshot,
            store: &f.store,
        };
        let expected_dir = f.path.join(format!("{format}-receipts"));
        fs::create_dir(&expected_dir).unwrap();
        let mut sink = cli_core::image::machine_export::FileSink::new(&expected_dir).unwrap();
        let report = cli_core::image::machine_export::export(
            &unemu_pc_core::PcCore,
            pc_image::PLATFORM,
            &source,
            format,
            if format == "machine" {
                None
            } else {
                Some("C:")
            },
            &mut sink,
        )
        .unwrap();
        for note in &report.notes {
            assert!(
                out.contains(&format!("note:{note}\n")),
                "missing note: {note}"
            );
        }
        let mut expected = Vec::new();
        cli_core::image::machine_export::print(&report, &mut expected).unwrap();
        assert_eq!(out.as_bytes(), expected);
    }
}

#[test]
fn current_reader_handles_unaligned_reads_and_detects_changed_verified_base() {
    use unemu_core_api::ErrorKind;
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    f.changed(&id, 0x37);
    let snapshot = machine::export::open(&f.store, &id).unwrap();
    let reader = snapshot.current(&f.store, "C:").unwrap();
    let mut bytes = [0; 514];
    reader.read_at(1, &mut bytes).unwrap();
    assert_eq!(&bytes[..511], &[0x37; 511]);
    assert_eq!(&bytes[511..], &[0; 3]);
    assert_eq!(
        reader.read_at(reader.size(), &mut [0]).unwrap_err().kind,
        ErrorKind::Usage
    );
    let digest = Digest::of(&fs::read(f.path.join("fixed.img")).unwrap());
    let path = f.store.blob_file_path(&digest, reader.size()).unwrap();
    let mut changed = fs::read(&path).unwrap();
    changed[700] ^= 1;
    fs::write(path, changed).unwrap();
    assert_eq!(
        reader.read_at(0, &mut bytes).unwrap_err().kind,
        ErrorKind::Content
    );
}
