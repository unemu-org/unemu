// SPDX-License-Identifier: AGPL-3.0-or-later
//! Export a stopped library machine's current synthetic disk through the CLI.

#[allow(dead_code)]
#[path = "../../pc-image/tests/support/workspace.rs"]
mod support;

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
    let path = f.path.join(name);
    let options = ["--slot", slot, "--to", path.to_str().unwrap()];
    let (code, out, err) = run(f, id, &options);
    assert_eq!(code, 0, "{err}");
    (path, out)
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
    assert_eq!(
        out,
        "A:\tfloppy\t368640\tjournal-changed:no\tselection:configured\tmedium:one.img\nC:\tfixed\t16384\tjournal-changed:no\tselection:configured\tmedium:C: image\n"
    );
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
    assert_eq!(
        out,
        format!("{}\tbytes:16384\t{}\n", path.display(), Digest::of(&raw))
    );
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
    assert!(out.contains("C:\tfixed\t16384\tjournal-changed:yes"));
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
    assert!(out.contains("C:\tfixed\t16384\tjournal-changed:no"));
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
    let target = f.path.join("out.img");
    for options in [
        vec![],
        vec!["--slot", "C:", "--to", target.to_str().unwrap()],
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
    let target = f.path.join("out.img");
    for options in [
        vec![],
        vec!["--slot", "C:", "--to", target.to_str().unwrap()],
    ] {
        let (code, _, err) = run(&f, &id, &options);
        assert_ne!(code, 0);
        assert!(err.contains("needs recovery"), "{err}");
    }
    assert!(!target.exists());
    assert_eq!(before, files(f.store.root()));
}

#[test]
fn refuses_existing_outputs_including_dangling_symlinks() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let path = f.path.join("out.img");
    fs::write(&path, b"keep").unwrap();
    let (code, _, err) = run(&f, &id, &["--slot", "C:", "--to", path.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(fs::read(&path).unwrap(), b"keep");
    #[cfg(unix)]
    {
        fs::remove_file(&path).unwrap();
        let absent = f.path.join("absent");
        std::os::unix::fs::symlink(&absent, &path).unwrap();
        let (code, _, err) = run(&f, &id, &["--slot", "C:", "--to", path.to_str().unwrap()]);
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
    let path = f.path.join("partial.img");
    let before = files(f.store.root());
    let error = snapshot
        .export_with_hook(&f.store, "A:", &path, |written| {
            assert_eq!(written, 64 * 1024);
            assert!(!path.exists());
            let partial = fs::read_dir(&f.path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|entry| {
                    entry
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".partial.img.unemu-export-")
                })
                .expect("export temp file");
            assert_eq!(fs::metadata(partial).unwrap().len(), written);
            Err(std::io::Error::other("injected output failure"))
        })
        .err()
        .unwrap();
    assert!(error.to_string().contains("injected output failure"));
    assert!(!path.exists());
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
        assert!(out.contains("C:\tfixed\t16384\tjournal-changed:yes"));
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
    snapshot
        .export(&f.store, "C:", &f.path.join("latest.img"))
        .unwrap();
    assert_eq!(before.unwrap(), files(f.store.root()));
}

#[test]
fn streaming_failure_preserves_a_replacement_at_the_output_path() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let snapshot = machine::export::open(&f.store, &id).unwrap();
    let path = f.path.join("partial.img");
    let before = files(f.store.root());
    let error = snapshot
        .export_with_hook(&f.store, "A:", &path, |written| {
            assert_eq!(written, 64 * 1024);
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
fn publish_refuses_a_destination_created_after_the_last_write() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let snapshot = machine::export::open(&f.store, &id).unwrap();
    let output = f.path.join("output");
    fs::create_dir(&output).unwrap();
    let path = output.join("late.img");
    let before = files(f.store.root());
    let error = snapshot
        .export_with_hook(&f.store, "A:", &path, |written| {
            if written == 360 * 1024 {
                fs::write(&path, b"late destination must survive")?;
            }
            Ok(())
        })
        .err()
        .expect("publish must refuse the late destination");
    assert!(error.to_string().contains("exist"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), b"late destination must survive");
    assert_eq!(
        fs::read_dir(&output).unwrap().count(),
        1,
        "temp file leaked"
    );
    assert_eq!(before, files(f.store.root()));
}

#[test]
fn rejects_unknown_ids_slots_and_incomplete_options() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine(&f);
    let target = f.path.join("out.img");
    let (code, _, err) = run(&f, "missing", &[]);
    assert_ne!(code, 0);
    assert!(err.contains("unemu image machine ls"), "{err}");
    let (code, _, err) = run(
        &f,
        &id,
        &["--slot", "unknown", "--to", target.to_str().unwrap()],
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
    let target = f.store.root().join("out.img");
    let (code, _, err) = run(&f, &id, &["--slot", "C:", "--to", target.to_str().unwrap()]);
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
    assert!(
        out.contains("B:\tfloppy\t0\tjournal-changed:no\tselection:empty\tmedium:(none)"),
        "{out}"
    );
    let target = f.path.join("out.img");
    let (code, _, err) = run(&f, &id, &["--slot", "B:", "--to", target.to_str().unwrap()]);
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
    let target = f.path.join("out.img");
    let (code, _, err) = run(&f, &id, &["--slot", "C:", "--to", target.to_str().unwrap()]);
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
    assert!(
        out.contains(
            "A:\tfloppy\t368640\tjournal-changed:yes\tselection:pending-insert\tmedium:setup.img"
        ),
        "{out}"
    );
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
    assert!(
        out.contains("selection:configured\tmedium:one.img"),
        "{out}"
    );
    let (path, _) = exported(&f, &id, "A:", "configured.img");
    assert_eq!(
        fs::read(path).unwrap(),
        fs::read(f.path.join("one.img")).unwrap()
    );
    assert_eq!(before, files(f.store.root()));
}
