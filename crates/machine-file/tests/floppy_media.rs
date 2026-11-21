// SPDX-License-Identifier: AGPL-3.0-or-later
//! Removable AT floppy catalogues; machine-files.md § Drives.

use assembly::{DeviceId, FloppyAction, ImageSource, WriteMode};
use std::path::Path;

fn files() -> machine_file::test_support::TempDir {
    let temp = machine_file::test_support::TempDir::new("floppy-catalogue");
    std::fs::write(temp.path().join("bios.bin"), vec![0xf4; 32768]).unwrap();
    std::fs::write(temp.path().join("one.img"), vec![0x11; 368640]).unwrap();
    std::fs::write(temp.path().join("two.img"), vec![0x22; 368640]).unwrap();
    temp
}

fn toml(drive: &str) -> String {
    format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n{drive}\n")
}

fn load(
    dir: &Path,
    name: &str,
    text: &str,
) -> Result<assembly::MachineConfig, machine_file::LoadError> {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    machine_file::load(path)
}

#[test]
fn floppy_media_toml_yaml_and_single_image_are_supported() {
    let temp = files();
    let spec = "media=[{image='one.img',write='readonly'},{image='two.img',write='journal',journal='two.upfj',keep_generations=2}]\nempty=true";
    let a = load(temp.path(), "machine.toml", &toml(spec)).unwrap();
    let yaml = "schema: 1\nboard: {model: ibm-5170}\nfirmware:\n  bios: {image: bios.bin, provides: [floppy]}\ncard:\n  - kind: fdc-765\n    drives:\n      - unit: 0\n        type: 360K\n        empty: true\n        media:\n          - {image: one.img, write: readonly}\n          - {image: two.img, write: journal, journal: two.upfj, keep_generations: 2}\n";
    let b = load(temp.path(), "machine.yaml", yaml).unwrap();
    assert_eq!(a, b);
    let list = a.drives[0].media.as_ref().unwrap();
    assert!(list.empty);
    assert_eq!(list.entries.len(), 2);
    assert_eq!(list.entries[0].write, WriteMode::ReadOnly);
    assert_eq!(
        list.entries[1].write,
        WriteMode::Journal {
            path: temp.path().canonicalize().unwrap().join("two.upfj"),
            keep_generations: 2
        }
    );
    let legacy = load(
        temp.path(),
        "single.toml",
        &toml("image='one.img'\nwrite='readonly'"),
    )
    .unwrap();
    assert!(legacy.drives[0].media.is_none());
    assert_eq!(legacy.drives[0].write, WriteMode::ReadOnly);
    assert_eq!(
        legacy.drives[0].image,
        ImageSource::Inline(vec![0x11; 368640])
    );
}

#[test]
fn floppy_media_geometry_conflicts_and_ambiguous_forms_are_refused() {
    let temp = files();
    std::fs::write(temp.path().join("two.img"), vec![0; 1228800]).unwrap();
    for drive in [
        "media=[{image='one.img'},{image='two.img'}]",
        "image='one.img'\nmedia=[{image='one.img'}]",
        "write='readonly'\nmedia=[{image='one.img'}]",
        "media=[]",
    ] {
        assert!(
            load(temp.path(), "bad.toml", &toml(drive)).is_err(),
            "{drive}"
        );
    }
    let yaml = "schema: 1\nboard: {model: ibm-5170}\nfirmware:\n  bios: {image: bios.bin, provides: [floppy]}\ncard:\n  - kind: fdc-765\n    drives:\n      - unit: 0\n        type: 360K\n        media: [{image: two.img}]\n";
    let error = load(temp.path(), "bad.yaml", yaml).unwrap_err().to_string();
    assert!(
        error.contains("media[0]")
            && error.contains(
                "medium image is 1228800 bytes but a 360K drive takes 360K (368640 bytes) media"
            ),
        "{error}"
    );
    assert!(load(temp.path(), "empty.toml", &toml("empty=true")).is_ok());
}

/// Machine-file text for one floppy drive of `drive_type` on unit 0.
fn typed(drive_type: &str, drive: &str) -> String {
    toml(drive).replace("type='360K'", &format!("type='{drive_type}'"))
}

#[test]
fn floppy_drive_types_take_their_lower_density_media() {
    let temp = files();
    std::fs::write(temp.path().join("dd720.img"), vec![0x72; 737_280]).unwrap();
    std::fs::write(temp.path().join("hd1440.img"), vec![0x14; 1_474_560]).unwrap();
    std::fs::write(temp.path().join("hd1200.img"), vec![0x12; 1_228_800]).unwrap();
    // Accepted: the drive's own format and its lower-density medium, in one catalogue.
    for (drive_type, images) in [
        ("1.44M", "media=[{image='dd720.img'},{image='hd1440.img'}]"),
        ("1.2M", "media=[{image='one.img'},{image='hd1200.img'}]"),
        ("1.44M", "image='dd720.img'"),
        ("1.2M", "image='one.img'"),
    ] {
        let config = load(temp.path(), "machine.toml", &typed(drive_type, images))
            .unwrap_or_else(|error| panic!("{drive_type} {images}: {error}"));
        assert_eq!(
            assembly::derive_cmos_config(&config).unwrap()[0x10] >> 4,
            if drive_type == "1.44M" { 4 } else { 2 },
            "CMOS 10h names the drive type, not the medium"
        );
        let _assembled = machine_file::assemble_with_options(
            &config,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
    }
    // Refused, naming the drive type and the media it takes.
    for (drive_type, images, needs) in [
        (
            "1.44M",
            "media=[{image='dd720.img'},{image='hd1200.img'}]",
            "medium image is 1228800 bytes but a 1.44M drive takes 1.44M (1474560 bytes) or 720K (737280 bytes) media",
        ),
        (
            "720K",
            "media=[{image='hd1440.img'}]",
            "medium image is 1474560 bytes but a 720K drive takes 720K (737280 bytes) media",
        ),
        (
            "1.2M",
            "media=[{image='dd720.img'}]",
            "medium image is 737280 bytes but a 1.2M drive takes 1.2M (1228800 bytes) or 360K (368640 bytes) media",
        ),
        (
            "1.44M",
            "image='one.img'",
            "image is 368640 bytes but a 1.44M drive takes 1.44M (1474560 bytes) or 720K (737280 bytes) media",
        ),
    ] {
        let error = load(temp.path(), "machine.toml", &typed(drive_type, images))
            .unwrap_err()
            .to_string();
        assert!(error.contains(needs), "{drive_type} {images}: {error}");
    }
}

#[test]
fn floppy_media_all_bases_are_revalidated_before_any_journal_opens() {
    let temp = files();
    let spec = "media=[{image='one.img',write='journal',journal='one.upfj'},{image='two.img'}]";
    let config = load(temp.path(), "machine.toml", &toml(spec)).unwrap();
    std::fs::write(temp.path().join("two.img"), vec![0x99; 368640]).unwrap();
    let error = machine_file::assemble_with_options(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("sha256"));
    assert!(!temp.path().join("one.upfj").exists());
}

#[test]
fn floppy_media_paths_cannot_alias_inactive_inputs_or_journals() {
    let temp = files();
    assert!(load(
        temp.path(),
        "valid.toml",
        &toml("media=[{image='one.img',write='journal',journal='one.upfj'},{image='two.img'}]")
    )
    .is_ok());
    for spec in [
        "media=[{image='one.img',write='journal',journal='two.img'},{image='two.img'}]",
        "media=[{image='one.img',write='journal',journal='both.upfj'},{image='two.img',write='journal',journal='both.upfj'}]",
    ] {
        assert!(load(temp.path(), "bad.toml", &toml(spec)).is_err());
    }
}

#[test]
fn floppy_media_empty_start_and_next_assert_change_line() {
    let temp = files();
    let config = load(
        temp.path(),
        "machine.toml",
        &toml("empty=true\nmedia=[{image='one.img'},{image='two.img'}]"),
    )
    .unwrap();
    let mut machine = machine_file::assemble_with_options(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert_eq!(machine.floppy_name(DeviceId::Fdc, 0), "empty");
    assert_eq!(machine.machine.bus_mut().port_read(0x3f7, 1).unwrap(), 0x80);
    for name in ["one.img", "two.img", "one.img"] {
        machine
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Next)
            .unwrap();
        assert_eq!(machine.floppy_name(DeviceId::Fdc, 0), name);
        assert_eq!(machine.machine.bus_mut().port_read(0x3f7, 1).unwrap(), 0x80);
    }
    machine.finish_persistence().unwrap();
}

#[test]
fn floppy_media_replaces_an_inherited_catalogue_with_layer_relative_paths() {
    let temp = files();
    let base = toml("empty=true\nmedia=[{image='one.img'},{image='two.img'}]");
    load(temp.path(), "base.toml", &base).unwrap();
    std::fs::create_dir(temp.path().join("child")).unwrap();
    std::fs::write(temp.path().join("child/three.img"), vec![0x33; 368640]).unwrap();
    let child = "schema=1\nextends='../base.toml'\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\nmedia=[{image='three.img',write='readonly'}]\n";
    let config = load(&temp.path().join("child"), "child.toml", child).unwrap();
    let list = config.drives[0].media.as_ref().unwrap();
    assert!(
        !list.empty,
        "a child media list starts inserted unless it restates empty"
    );
    assert_eq!(list.entries.len(), 1);
    assert_eq!(
        list.entries[0].image,
        ImageSource::File(temp.path().join("child/three.img").canonicalize().unwrap())
    );
    let explicit_empty = load(
        &temp.path().join("child"),
        "empty.toml",
        &format!("{child}empty=true\n"),
    )
    .unwrap();
    assert!(explicit_empty.drives[0].media.as_ref().unwrap().empty);
    let single = "schema=1\nextends='base.toml'\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\nimage='one.img'\n";
    assert!(load(temp.path(), "single.toml", single).unwrap().drives[0]
        .media
        .is_none());
}

#[test]
fn floppy_media_check_opens_no_writers_and_legacy_window_labels_keep_file_names() {
    let temp = files();
    let spec = "media=[{image='one.img',write='journal',journal='one.upfj'},{image='two.img',write='journal',journal='two.upfj'}]";
    let path = temp.path().join("machine.toml");
    std::fs::write(&path, toml(spec)).unwrap();
    let loaded = machine_file::load_for_assembly(
        &path,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    loaded.check().unwrap();
    for name in ["one.upfj", "one.upfj.lock", "two.upfj", "two.upfj.lock"] {
        assert!(!temp.path().join(name).exists(), "{name}");
    }
    std::fs::write(&path, toml("image='one.img'")).unwrap();
    let loaded = machine_file::load_for_assembly(
        &path,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let mut machine = loaded
        .assemble(&assembly::ObservationPolicy::default())
        .unwrap();
    assert_eq!(machine.floppy_name(DeviceId::Fdc, 0), "one.img");
    machine.finish_persistence().unwrap();
}
