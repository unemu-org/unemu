// SPDX-License-Identifier: AGPL-3.0-or-later
//! File-loaded floppy persistence through NEC µPD765 WRITE/READ DATA (N=2)
//! and Intel 8237A single-transfer mode; see machine-files.md § Drives and extends.

use assembly::{assemble_with_options, AssemblyOptions, ConfigError, WriteMode};
use machine_file::{load, LoadError};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

const BASE: &str = "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='base.img'\n";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "machine-file-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let temp = Self(path.canonicalize().unwrap());
        temp.write("bios.bin", vec![0xF4; 0x8000]);
        temp.write("base.img", vec![0; 368_640]);
        temp
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn error(path: &Path) -> machine_file::LowerError {
    let LoadError::Lower(error) = load(path).unwrap_err() else {
        panic!("expected lowering error")
    };
    error
}
fn out(program: &mut Vec<u8>, port: u16, value: u8) {
    program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
}
fn transfer(program: &mut Vec<u8>, write: bool, result: u16) {
    for (port, value) in [
        (0x0A, 6),
        (0x0C, 0),
        (0x0B, if write { 0x4A } else { 0x46 }),
        (4, 0),
        (4, if write { 6 } else { 10 }),
        (0x81, 0),
        (5, 0xFF),
        (5, 1),
        (0x0A, 2),
    ] {
        out(program, port, value);
    }
    for value in [if write { 5 } else { 6 }, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
        out(program, 0x3F5, value);
    }
    program.extend([0xBF, result as u8, (result >> 8) as u8]);
    for _ in 0..7 {
        program.extend([0xEC, 0xAA]);
    }
}
fn bios(write: bool) -> Vec<u8> {
    let mut program = vec![
        0xFA, 0xFC, 0x31, 0xC0, 0x8E, 0xD8, 0x8E, 0xC0, 0x8E, 0xD0, 0xBC, 0, 0x70,
    ];
    out(&mut program, 0x3F2, 0x1C);
    if write {
        program.extend([0xBF, 0, 6, 0xB9, 0, 2, 0xB0, 0xC7, 0xF3, 0xAA]);
        transfer(&mut program, true, 0x1000);
    }
    program.extend([0xBF, 0, 10, 0xB9, 0, 2, 0xB0, 0xFF, 0xF3, 0xAA]);
    program.extend([0xBF, 0x10, 0x10, 0xB9, 7, 0, 0xB0, 0xFF, 0xF3, 0xAA]);
    transfer(&mut program, false, 0x1010);
    program.push(0xF4);
    let mut rom = vec![0xF4; 0x8000];
    rom[..program.len()].copy_from_slice(&program);
    rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
    rom
}
fn acceptance(yaml: bool) {
    let temp = Temp::new();
    let path = if yaml {
        temp.write("machine.yaml", "schema: 1\nboard: {model: ibm-5170}\nfirmware:\n  bios: {image: bios.bin, provides: [floppy]}\ncard:\n  - kind: fdc-765\n    drives:\n      - unit: 0\n        type: 360K\n        image: base.img\n        write: journal\n        journal: disk.upfj\n")
    } else {
        temp.write(
            "machine.toml",
            format!("{BASE}write='journal'\njournal='disk.upfj'\n"),
        )
    };
    let original = std::fs::read(temp.0.join("base.img")).unwrap();
    for write in [true, false] {
        temp.write("bios.bin", bios(write));
        let config = load(&path).unwrap();
        assert_eq!(
            config.drives[0].write,
            WriteMode::journal(temp.0.join("disk.upfj"))
        );
        let mut assembled = assemble_with_options(
            &config,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assert_eq!(assembled.persistence.len(), 1);
        assert_eq!(
            assembled
                .run_session(&assembly::RunPolicy::natural(Some(10_000), None))
                .unwrap(),
            machine::RunOutcome::Halted
        );
        let mut bytes = [0; 512];
        assembled.machine.cpu().read_mem(0xA00, &mut bytes).unwrap();
        assert_eq!(bytes, [0xC7; 512]);
        for result in if write {
            vec![0x1000, 0x1010]
        } else {
            vec![0x1010]
        } {
            let mut status = [0; 7];
            assembled
                .machine
                .cpu()
                .read_mem(result, &mut status)
                .unwrap();
            assert_eq!(&status[..3], &[0, 0, 0]);
        }
        assembled.finish_persistence().unwrap();
        drop(assembled);
        assert_eq!(std::fs::read(temp.0.join("base.img")).unwrap(), original);
    }
    assert!(temp.0.join("disk.upfj").is_file());
}

#[test]
fn toml_guest_write_replays_after_reload_without_modifying_base() {
    acceptance(false);
}
#[test]
fn yaml_guest_write_replays_after_reload_without_modifying_base() {
    acceptance(true);
}

#[test]
fn journal_options_require_journal_write_mode() {
    let temp = Temp::new();
    for mode in ["", "write='volatile'\n", "write='readonly'\n"] {
        for option in ["journal='disk.upfj'\n", "keep_generations=2\n"] {
            let path = temp.write("machine.toml", format!("{BASE}{mode}{option}"));
            let e = error(&path);
            assert_eq!(e.field, "card[fdc-765].drives[unit=0].write");
            assert_eq!(
                e.message,
                "journal and keep_generations are only accepted with write = \"journal\""
            );
        }
    }
}
#[test]
fn journal_write_requires_an_explicit_nonempty_path() {
    let temp = Temp::new();
    let path = temp.write("machine.toml", format!("{BASE}write='journal'\n"));
    let e = error(&path);
    assert_eq!(e.field, "card[fdc-765].drives[unit=0].journal");
    assert_eq!(e.origin.file, path);
    assert_eq!(e.origin.line, Some(9));
    assert_eq!(e.message, "required field is missing");
    let path = temp.write(
        "machine.toml",
        format!("{BASE}write='journal'\njournal=''\n"),
    );
    assert_eq!(error(&path).message, "an explicit journal path is required");
}
#[test]
fn zero_retention_is_rejected_with_its_source_line() {
    let temp = Temp::new();
    let path = temp.write(
        "machine.toml",
        format!("{BASE}write='journal'\njournal='disk.upfj'\nkeep_generations=0\n"),
    );
    let LoadError::Parse(e) = load(&path).unwrap_err() else {
        panic!("expected parse error")
    };
    assert_eq!(e.path(), path);
    assert!(e.message().contains("nonzero u32"), "{e}");
    assert!(e.to_string().contains("line 15"), "{e}");
}
#[test]
fn journal_image_alias_is_an_assembly_error_at_the_drive_origin() {
    let temp = Temp::new();
    let path = temp.write(
        "machine.toml",
        format!("{BASE}write='journal'\njournal='./base.img'\n"),
    );
    let e = error(&path);
    assert_eq!(e.field, "card[fdc-765].drives[unit=0]");
    assert_eq!(e.origin.file, path);
    assert_eq!(e.origin.line, Some(9));
    assert!(matches!(
        *e.validation.unwrap(),
        ConfigError::Journal { drive_index: 0, .. }
    ));
    assert!(
        e.message.starts_with("card[fdc-765].drives[unit=0] "),
        "{}",
        e.message
    );
    assert!(e.message.contains("aliases mounted input"), "{}", e.message);
}
#[test]
fn inherited_journal_and_child_journal_use_their_own_layer_directories() {
    let temp = Temp::new();
    temp.write("parent/base.img", vec![0; 368_640]);
    temp.write("parent/bios.bin", vec![0xF4; 0x8000]);
    temp.write(
        "parent/base.toml",
        format!("{BASE}write='journal'\njournal='disk.upfj'\nkeep_generations=7\n"),
    );
    let child = temp.write(
        "child/machine.toml",
        "schema=1\nextends='../parent/base.toml'\n",
    );
    let config = load(&child).unwrap();
    assert_eq!(
        config.drives[0].write,
        WriteMode::Journal {
            path: temp.0.join("parent/disk.upfj"),
            keep_generations: 7
        }
    );
    temp.write("child/machine.toml", "schema=1\nextends='../parent/base.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\njournal='child.upfj'\nkeep_generations=3\n");
    assert_eq!(
        load(&child).unwrap().drives[0].write,
        WriteMode::Journal {
            path: temp.0.join("child/child.upfj"),
            keep_generations: 3
        }
    );
}
#[cfg(unix)]
#[test]
fn symlinked_layer_uses_canonical_directory_and_spelled_diagnostics() {
    let temp = Temp::new();
    let target = temp.write(
        "target/machine.toml",
        format!("{BASE}write='journal'\njournal='disk.upfj'\n"),
    );
    temp.write("target/base.img", vec![0; 368_640]);
    temp.write("target/bios.bin", vec![0xF4; 0x8000]);
    let alias = temp.0.join("alias.toml");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    assert_eq!(
        load(&alias).unwrap().drives[0].write,
        WriteMode::journal(temp.0.join("target/disk.upfj"))
    );
    temp.write(
        "target/machine.toml",
        format!("{BASE}write='journal'\njournal='base.img'\n"),
    );
    let e = error(&alias);
    assert_eq!(e.origin.file, alias);
    assert_eq!(e.origin.line, Some(9));
    assert!(e.message.contains("aliases mounted input"));
}
#[test]
fn child_can_switch_an_inherited_drive_to_journal() {
    let temp = Temp::new();
    temp.write("base.toml", BASE);
    let child = temp.write("child.toml", "schema=1\nextends='base.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nwrite='journal'\njournal='disk.upfj'\n");
    assert_eq!(
        load(child).unwrap().drives[0].write,
        WriteMode::journal(temp.0.join("disk.upfj"))
    );
}
#[test]
fn child_image_override_drops_journal_pin_write_and_retention() {
    let temp = Temp::new();
    temp.write(
        "parent.toml",
        format!(
            "{BASE}sha256='{}'\nwrite='journal'\njournal='disk.upfj'\nkeep_generations=7\n",
            "0".repeat(64)
        ),
    );
    let child_text = "schema=1\nextends='parent.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nimage='new.img'\n";
    temp.write("new.img", vec![0; 368_640]);
    let child = temp.write("child.toml", child_text);
    let (doc, _) = machine_file::resolve(&child).unwrap();
    let drive = &doc.card.as_ref().unwrap()[0].drives.as_ref().unwrap()[0];
    assert!(drive.journal.is_none());
    assert!(drive.sha256.is_none());
    assert!(drive.keep_generations.is_none());
    assert!(drive.write.is_none());
    assert_eq!(load(&child).unwrap().drives[0].write, WriteMode::Volatile);
    temp.write("child.toml", format!("{child_text}write='journal'\n"));
    let e = error(&child);
    assert_eq!(e.field, "card[fdc-765].drives[unit=0].journal");
    assert_eq!(e.origin.file, child);
    assert_eq!(e.origin.line, Some(5));
    assert_eq!(e.message, "required field is missing");
    temp.write(
        "child.toml",
        format!("{child_text}write='journal'\njournal='new.upfj'\n"),
    );
    assert_eq!(
        load(&child).unwrap().drives[0].write,
        WriteMode::journal(temp.0.join("new.upfj"))
    );
}
fn entries(path: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    entries
}
#[test]
fn repository_fixture_drives_remain_volatile_without_creating_sidecars() {
    let Some(_) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let Some(_) = fixtures::fixture("freedos/freedos.img") else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dirs = [
        root.join("vendor/kiselev-bios"),
        root.join("vendor/freedos"),
    ];
    let before = dirs.each_ref().map(|dir| entries(dir));
    let image = std::fs::read(dirs[1].join("freedos.img")).unwrap();
    for name in ["at-freedos.toml", "at-freedos-from-preset.toml"] {
        let config = load(root.join("machines").join(name)).unwrap();
        assert!(!config.drives.is_empty());
        assert!(config
            .drives
            .iter()
            .all(|drive| drive.write == WriteMode::Volatile));
        let mut assembled = assembly::assemble(&config).unwrap();
        assert!(assembled.persistence.is_empty());
        assembled.finish_persistence().unwrap();
    }
    assert_eq!(dirs.each_ref().map(|dir| entries(dir)), before);
    assert_eq!(std::fs::read(dirs[1].join("freedos.img")).unwrap(), image);
}

#[test]
fn journal_bases_still_check_hash_pins_and_geometry_before_mounting() {
    let temp = Temp::new();
    let path = temp.write(
        "machine.toml",
        format!(
            "{BASE}write='journal'\njournal='disk.upfj'\nsha256='{}'\n",
            "0".repeat(64)
        ),
    );
    assert!(error(&path).message.contains("sha256 mismatch"));
    temp.write(
        "machine.toml",
        format!("{BASE}write='journal'\njournal='disk.upfj'\n"),
    );
    temp.write("base.img", [0; 512]);
    assert!(matches!(
        *error(&path).validation.unwrap(),
        ConfigError::ImageGeometryMismatch {
            expected: 368_640,
            actual: 512,
            ..
        }
    ));
    assert!(!temp.0.join("disk.upfj").exists());
}

fn changed_base_is_refused(existing_journal: bool, explicit_pin: bool) {
    let temp = Temp::new();
    let original = vec![0; 368_640];
    let expected = format!("{:x}", Sha256::digest(&original));
    let pin = if explicit_pin {
        format!("sha256='{expected}'\n")
    } else {
        String::new()
    };
    let path = temp.write(
        "machine.toml",
        format!("{BASE}write='journal'\njournal='disk.upfj'\n{pin}"),
    );
    let config = load(&path).unwrap();
    let journal = temp.0.join("disk.upfj");
    let before = if existing_journal {
        let mut assembled = assemble_with_options(
            &config,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assembled.finish_persistence().unwrap();
        drop(assembled);
        Some(std::fs::read(&journal).unwrap())
    } else {
        None
    };
    let changed = vec![0x55; 368_640];
    temp.write("base.img", &changed);
    let actual = format!("{:x}", Sha256::digest(&changed));
    let failure = match assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    ) {
        Ok(_) => panic!("changed base was mounted"),
        Err(error) => error,
    };
    let message = format!(
        "invalid configuration: drive 0 image sha256 mismatch: expected {expected}, actual {actual}"
    );
    assert_eq!(failure.to_string(), message);
    match before {
        Some(bytes) => assert_eq!(std::fs::read(&journal).unwrap(), bytes),
        None => {
            assert!(!journal.exists());
            assert!(!temp.0.join("disk.upfj.lock").exists());
        }
    }
}

#[test]
fn pinned_base_rewritten_after_load_is_refused_before_journal_creation() {
    changed_base_is_refused(false, true);
}

#[test]
fn existing_journal_with_changed_base_reports_pin_mismatch_before_binding() {
    changed_base_is_refused(true, true);
}

#[test]
fn unpinned_base_rewritten_after_load_is_also_refused() {
    changed_base_is_refused(false, false);
}

#[test]
fn synthetic_reader_journal_is_refused_and_volatile_guest_reads_verified_bytes() {
    struct Synthetic;
    impl machine_file::ImageReader for Synthetic {
        fn read(&mut self, path: &Path) -> std::io::Result<Vec<u8>> {
            if path.file_name().unwrap() == "base.img" {
                Ok(vec![0; 368_640])
            } else {
                Ok(bios(false))
            }
        }
    }
    let temp = Temp::new();
    temp.write("base.img", vec![0x55; 368_640]);
    let pin = format!("{:x}", Sha256::digest(vec![0; 368_640]));
    let path = temp.write("machine.toml", format!("{BASE}sha256='{pin}'\n"));
    let config = machine_file::load_with_reader(&path, &mut Synthetic).unwrap();
    let mut assembled = assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert_eq!(
        assembled
            .run_session(&assembly::RunPolicy::natural(Some(10_000), None))
            .unwrap(),
        machine::RunOutcome::Halted
    );
    let mut sector = [0xFF; 512];
    assembled
        .machine
        .cpu()
        .read_mem(0xA00, &mut sector)
        .unwrap();
    assert_eq!(sector, [0; 512]);
    let mut status = [0xFF; 7];
    assembled
        .machine
        .cpu()
        .read_mem(0x1010, &mut status)
        .unwrap();
    assert_eq!(&status[..3], &[0, 0, 0]);
    temp.write(
        "machine.toml",
        format!("{BASE}sha256='{pin}'\nwrite='journal'\njournal='disk.upfj'\n"),
    );
    let failure = match machine_file::load_with_reader(&path, &mut Synthetic) {
        Err(LoadError::Lower(error)) => error,
        _ => panic!("synthetic journal base was accepted"),
    };
    assert_eq!(failure.field, "card[fdc-765].drives[unit=0].image");
    assert_eq!(failure.origin.file, path);
    assert_eq!(failure.origin.line, Some(12));
    assert_eq!(
        failure.message,
        format!(
            "cannot read image {}: journal mode requires a file-backed image reader",
            temp.0.join("base.img").display()
        )
    );
    assert!(!temp.0.join("disk.upfj").exists());
    assert!(!temp.0.join("disk.upfj.lock").exists());
}

#[test]
fn ide_journals_are_still_refused_by_assembly_validation() {
    let temp = Temp::new();
    temp.write("base.img", [0; 512]);
    let input = "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['fixed-disk']\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=1}\nimage='base.img'\nwrite='journal'\njournal='disk.upfj'\n";
    let path = temp.write("machine.toml", input);
    let e = error(&path);
    assert_eq!(e.origin.file, path);
    assert_eq!(e.origin.line, Some(9));
    assert!(matches!(
        *e.validation.unwrap(),
        ConfigError::Journal { drive_index: 0, .. }
    ));
    assert_eq!(e.message, "card[ide].drives[channel=master] journal configuration: journal mode requires a floppy with 512-byte sectors");
}

fn disable_inherited_journal(mode: &str, expected: WriteMode) {
    let temp = Temp::new();
    let parent = temp.write(
        "parent.toml",
        format!("{BASE}write='journal'\njournal='disk.upfj'\nkeep_generations=7\n"),
    );
    let child = temp.write("child.toml", format!("schema=1\nextends='parent.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nwrite='{mode}'\n"));
    let (doc, origins) = machine_file::resolve(&child).unwrap();
    let drive = &doc.card.as_ref().unwrap()[0].drives.as_ref().unwrap()[0];
    assert!(drive.journal.is_none());
    assert!(drive.keep_generations.is_none());
    assert_ne!(origins.get("card[0].drives[0].journal").file, parent);
    assert_ne!(
        origins.get("card[0].drives[0].keep_generations").file,
        parent
    );
    assert_eq!(load(&child).unwrap().drives[0].write, expected);
    assert!(!temp.0.join("disk.upfj").exists());
    for option in ["journal='own.upfj'", "keep_generations=2"] {
        temp.write("child.toml", format!("schema=1\nextends='parent.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nwrite='{mode}'\n{option}\n"));
        let e = error(&child);
        assert_eq!(e.origin.file, child);
        assert_eq!(
            e.message,
            "journal and keep_generations are only accepted with write = \"journal\""
        );
    }
}

#[test]
fn child_can_disable_journaling_with_volatile_without_repeating_image() {
    disable_inherited_journal("volatile", WriteMode::Volatile);
}

#[test]
fn child_can_disable_journaling_with_readonly_without_repeating_image() {
    disable_inherited_journal("readonly", WriteMode::ReadOnly);
}

fn preserve_inherited_write_on_image_override(mode: &str, expected: WriteMode) {
    let temp = Temp::new();
    let parent = temp.write("parent.toml", format!("{BASE}write='{mode}'\n"));
    temp.write("new.img", vec![0; 368_640]);
    let child = temp.write("child.toml", "schema=1\nextends='parent.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nimage='new.img'\n");
    let (doc, origins) = machine_file::resolve(&child).unwrap();
    let drive = &doc.card.as_ref().unwrap()[0].drives.as_ref().unwrap()[0];
    assert!(drive.write.is_some());
    assert_eq!(origins.get("card[0].drives[0].write").file, parent);
    assert_eq!(origins.get("card[0].drives[0].write").line, Some(13));
    assert_eq!(load(&child).unwrap().drives[0].write, expected);
}

#[test]
fn image_override_preserves_inherited_readonly() {
    preserve_inherited_write_on_image_override("readonly", WriteMode::ReadOnly);
}

#[test]
fn image_override_preserves_inherited_volatile() {
    preserve_inherited_write_on_image_override("volatile", WriteMode::Volatile);
}

fn assert_source_alias(path: &Path, drive_line: usize) {
    let e = error(path);
    assert_eq!(e.origin.file, path);
    assert_eq!(e.origin.line, Some(drive_line));
    assert_eq!(e.field, "card[fdc-765].drives[unit=0]");
    assert!(matches!(
        *e.validation.unwrap(),
        ConfigError::Journal { drive_index: 0, .. }
    ));
    assert!(e.message.contains("aliases source input"), "{}", e.message);
}

fn journal_aliases_source(source: &str, kind: &str) {
    let temp = Temp::new();
    let path = temp.write("machine.toml", BASE);
    let journal = match kind {
        "direct" => source,
        "canonical" => {
            #[cfg(unix)]
            std::os::unix::fs::symlink(temp.0.join(source), temp.0.join("alias.upfj")).unwrap();
            "alias.upfj"
        }
        "hardlink" => {
            std::fs::hard_link(temp.0.join(source), temp.0.join("alias.upfj")).unwrap();
            "alias.upfj"
        }
        _ => panic!("unknown alias kind"),
    };
    temp.write(
        "machine.toml",
        format!("{BASE}write='journal'\njournal='{journal}'\n"),
    );
    let before = std::fs::read(temp.0.join(source)).unwrap();
    let listing = entries(&temp.0);
    assert_source_alias(&path, 9);
    assert_eq!(std::fs::read(temp.0.join(source)).unwrap(), before);
    assert_eq!(entries(&temp.0), listing);
}

#[test]
fn journal_cannot_alias_bios_image() {
    journal_aliases_source("bios.bin", "direct");
}

#[test]
fn journal_cannot_alias_entry_machine_file() {
    journal_aliases_source("machine.toml", "direct");
}

#[cfg(unix)]
#[test]
fn journal_cannot_alias_bios_through_symlink() {
    journal_aliases_source("bios.bin", "canonical");
}

#[cfg(unix)]
#[test]
fn journal_cannot_alias_bios_through_hardlink() {
    journal_aliases_source("bios.bin", "hardlink");
}

#[cfg(unix)]
#[test]
fn journal_cannot_alias_entry_machine_through_hardlink() {
    journal_aliases_source("machine.toml", "hardlink");
}

#[test]
fn journal_cannot_alias_parent_layer_file() {
    let temp = Temp::new();
    temp.write("parent.toml", BASE);
    let child = temp.write("child.toml", "schema=1\nextends='parent.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nwrite='journal'\njournal='parent.toml'\n");
    assert_source_alias(&child, 5);
}

#[test]
fn journal_cannot_alias_layer_without_surviving_fields() {
    let temp = Temp::new();
    temp.write("parent.toml", BASE);
    temp.write("middle.toml", "schema=1\nextends='parent.toml'\n");
    let child = temp.write("child.toml", "schema=1\nextends='middle.toml'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nwrite='journal'\njournal='middle.toml'\n");
    assert_source_alias(&child, 5);
}

#[test]
fn journal_cannot_alias_other_volatile_drive_image() {
    let temp = Temp::new();
    temp.write("other.img", vec![0; 368_640]);
    let path = temp.write("machine.toml", format!("{BASE}write='journal'\njournal='other.img'\n[[card.drives]]\nunit=1\ntype='360K'\nimage='other.img'\n"));
    assert_source_alias(&path, 9);
}

#[test]
fn journal_cannot_alias_nvram_image() {
    let temp = Temp::new();
    temp.write("cmos.bin", [0; 64]);
    let path = temp.write(
        "machine.toml",
        format!(
            "{BASE}write='journal'\njournal='cmos.bin'\n[board.rtc]\nnvram={{image='cmos.bin'}}\n"
        ),
    );
    assert_source_alias(&path, 9);
}

#[test]
fn journal_cannot_name_directory() {
    let temp = Temp::new();
    std::fs::create_dir(temp.0.join("journal-dir")).unwrap();
    let path = temp.write(
        "machine.toml",
        format!("{BASE}write='journal'\njournal='journal-dir'\n"),
    );
    let e = error(&path);
    assert_eq!(e.origin.file, path);
    assert_eq!(e.origin.line, Some(9));
    assert_eq!(e.field, "card[fdc-765].drives[unit=0]");
    assert!(e.message.contains("journal path"), "{}", e.message);
    assert!(e.message.contains("is a directory"), "{}", e.message);
    assert!(!temp.0.join("journal-dir.lock").exists());
}

#[test]
fn retention_temporary_image_names_are_refused_for_every_write_mode() {
    let temp = Temp::new();
    temp.write("base.retain-1.tmp", vec![0; 368_640]);
    temp.write("bios.retain-1.tmp", vec![0xF4; 0x8000]);
    for mode in ["volatile", "readonly", "journal"] {
        for image in ["base.img", "bios.bin"] {
            let reserved = if image == "base.img" {
                "base.retain-1.tmp"
            } else {
                "bios.retain-1.tmp"
            };
            let path = temp.write(
                "machine.toml",
                format!(
                    "{}write='{mode}'\n{}",
                    BASE.replace(image, reserved),
                    if mode == "journal" {
                        "journal='disk.upfj'\n"
                    } else {
                        ""
                    }
                ),
            );
            let e = error(&path);
            assert_eq!(e.origin.file, path);
            assert_eq!(
                e.origin.line,
                Some(if image == "base.img" { 12 } else { 5 })
            );
            assert_eq!(
                e.field,
                if image == "base.img" {
                    "card[fdc-765].drives[unit=0].image"
                } else {
                    "firmware.bios.image"
                }
            );
            assert!(
                e.message.contains("reserved retention-temporary name"),
                "{e}"
            );
            assert!(e.message.contains(reserved), "{e}");
        }
    }
    assert!(!temp.0.join("disk.upfj").exists());
    assert!(!temp.0.join("disk.upfj.lock").exists());
}

#[cfg(unix)]
#[test]
fn resolved_retention_temporary_image_names_are_refused() {
    let temp = Temp::new();
    temp.write("bios.retain-1.tmp", vec![0xF4; 0x8000]);
    std::fs::remove_file(temp.0.join("bios.bin")).unwrap();
    std::os::unix::fs::symlink("bios.retain-1.tmp", temp.0.join("bios.bin")).unwrap();
    let path = temp.write("machine.toml", BASE);
    let e = error(&path);
    assert_eq!(e.origin.file, path);
    assert_eq!(e.origin.line, Some(5));
    assert_eq!(e.field, "firmware.bios.image");
    assert!(
        e.message.contains("reserved retention-temporary name"),
        "{e}"
    );
}

#[test]
fn journal_replacement_temporary_names_remain_reserved() {
    let temp = Temp::new();
    for marker in ["create", "retain", "export"] {
        let path = temp.write(
            "machine.toml",
            format!("{BASE}write='journal'\njournal='disk.upfj.{marker}-1.tmp'\n"),
        );
        let e = error(&path);
        assert_eq!(e.origin.file, path);
        assert_eq!(e.origin.line, Some(9));
        assert_eq!(e.field, "card[fdc-765].drives[unit=0]");
        assert!(matches!(
            *e.validation.unwrap(),
            ConfigError::Journal { drive_index: 0, .. }
        ));
        assert!(e.message.contains("reserved"), "{}", e.message);
    }
}

#[test]
fn journal_temporary_names_are_reserved_for_inline_inputs_and_layers() {
    let temp = Temp::new();
    for marker in ["create", "retain"] {
        let source = format!("disk.upfj.{marker}-1.tmp");
        temp.write(&source, vec![0xF4; 0x8000]);
        let path = temp.write(
            "machine.toml",
            format!(
                "{}write='journal'\njournal='disk.upfj'\n",
                BASE.replace("bios.bin", &source)
            ),
        );
        let e = error(&path);
        assert_eq!(e.origin.file, path);
        assert!(e.message.contains("reserv"), "{e}");
        temp.write(&source, BASE);
        let layer = format!("disk.upfj.{marker}-1.tmp.toml");
        std::fs::rename(temp.0.join(&source), temp.0.join(&layer)).unwrap();
        let path = temp.write("machine.toml", format!("schema=1\nextends='{layer}'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\nwrite='journal'\njournal='disk.upfj'\n"));
        if marker == "create" {
            // Only the journal's exact decimal temporary namespace is reserved.
            load(&path).unwrap();
        } else {
            let e = error(&path);
            assert_eq!(e.origin.file, path);
            assert_eq!(e.origin.line, Some(5));
            assert!(e.message.contains("reserv"), "{e}");
        }
    }
    assert!(!temp.0.join("disk.upfj.lock").exists());
}

#[test]
fn journal_cannot_alias_an_option_rom_input() {
    for yaml in [false, true] {
        for alias in ["direct", "hardlink"] {
            let temp = Temp::new();
            let mut rom = vec![0; 2048];
            rom[..4].copy_from_slice(&[0x55, 0xAA, 4, 0xCB]);
            rom[2047] = 0x32;
            let option = temp.write("option.bin", &rom);
            let journal = if alias == "direct" {
                "option.bin"
            } else {
                std::fs::hard_link(&option, temp.0.join("alias.upfj")).unwrap();
                "alias.upfj"
            };
            let (name, input) = if yaml {
                ("machine.yaml", format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios: {{image: bios.bin, provides: [floppy]}}\n  option_roms: [{{image: option.bin, base: 0xC8000}}]\ncard:\n  - kind: fdc-765\n    drives:\n      - unit: 0\n        type: 360K\n        image: base.img\n        write: journal\n        journal: {journal}\n"))
            } else {
                let base = BASE.replace(
                    "[firmware.bios]",
                    "[firmware]\noption_roms=[{image='option.bin',base=0xC8000}]\n[firmware.bios]",
                );
                (
                    "machine.toml",
                    format!("{base}write='journal'\njournal='{journal}'\n"),
                )
            };
            let drive_line = input
                .lines()
                .position(|line| {
                    if yaml {
                        line.contains("- unit:")
                    } else {
                        line == "[[card.drives]]"
                    }
                })
                .unwrap()
                + 1;
            let path = temp.write(name, input);
            let listing = entries(&temp.0);
            for allow_unlisted_firmware in [false, true] {
                let LoadError::Lower(error) = machine_file::load_with_options(
                    &path,
                    AssemblyOptions {
                        allow_unlisted_firmware,
                    },
                )
                .unwrap_err() else {
                    panic!("expected a source-located journal alias error");
                };
                assert_eq!(error.origin.file, path);
                assert_eq!(error.origin.line, Some(drive_line));
                assert_eq!(error.field, "card[fdc-765].drives[unit=0]");
                assert!(matches!(
                    error.validation.as_deref(),
                    Some(ConfigError::Journal { drive_index: 0, .. })
                ));
                assert_eq!(error.message, format!("card[fdc-765].drives[unit=0] journal configuration: journal {} aliases source input {}", temp.0.join(journal).display(), option.display()));
                assert_eq!(std::fs::read(&option).unwrap(), rom);
                assert_eq!(entries(&temp.0), listing);
            }
        }
    }
}
