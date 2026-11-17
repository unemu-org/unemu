// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cross-file inheritance, source provenance and embedded-preset acceptance tests.

use machine_file::{load, load_with_reader, lower, resolve, ImageReader, LoadError};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

const BASE: &str =
    "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n";
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "machine-file-extends-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn write(&self, name: &str, input: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, input).unwrap();
        path
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Reader<F>(F);
impl<F: FnMut(&Path) -> io::Result<Vec<u8>>> ImageReader for Reader<F> {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.0(path)
    }
}
fn message(error: LoadError) -> machine_file::LowerError {
    let LoadError::Lower(error) = error else {
        panic!("expected resolution error: {error}")
    };
    error
}

#[test]
fn cycle_error_prints_chain_and_extends_source_line() {
    let tree = Tree::new();
    let a = tree.write("a.toml", "schema=1\nextends='b.yaml'\n");
    let b = tree.write("b.yaml", "schema: 1\nextends: a.toml\n");
    let e = message(resolve(&a).unwrap_err());
    assert_eq!(
        e.message,
        format!("inheritance cycle: {} → b.yaml → a.toml", a.display())
    );
    assert_eq!(e.field, "extends");
    assert_eq!(e.origin.file, b);
    assert_eq!(e.origin.line, Some(2));
}

#[test]
fn eight_layers_are_allowed_and_ninth_fails_at_parent_reference() {
    let tree = Tree::new();
    tree.write("7.toml", BASE);
    for i in (0..7).rev() {
        tree.write(
            &format!("{i}.toml"),
            &format!("schema=1\nextends='{}.toml'\n", i + 1),
        );
    }
    resolve(tree.0.join("0.toml")).unwrap();
    tree.write("extra.toml", "schema=1\nextends='0.toml'\n");
    let e = message(resolve(tree.0.join("extra.toml")).unwrap_err());
    assert_eq!(e.message, "inheritance depth exceeds limit of 8 layers");
    assert_eq!(e.origin.file, tree.0.join("6.toml"));
    assert_eq!(e.origin.line, Some(2));
}

#[cfg(unix)]
#[test]
fn cycles_use_canonical_identity_but_keep_spelled_names() {
    let tree = Tree::new();
    let root = tree.write("root.toml", "schema=1\nextends='alias.toml'\n");
    std::os::unix::fs::symlink("root.toml", tree.0.join("alias.toml")).unwrap();
    let e = message(resolve(&root).unwrap_err());
    assert_eq!(
        e.message,
        format!("inheritance cycle: {} → alias.toml", root.display())
    );
}

#[test]
fn yaml_extends_toml_and_toml_extends_yaml_root_first() {
    let tree = Tree::new();
    tree.write(
        "base.toml",
        &format!("{BASE}[memory]\nconventional_kib=512\nextended_kib=64\n"),
    );
    tree.write(
        "middle.yaml",
        "schema: 1\nextends: base.toml\nmemory: {extended_kib: 128}\ncpu: {rate: 2}\n",
    );
    let root = tree.write(
        "root.toml",
        "schema=1\nextends='middle.yaml'\n[cpu]\nrate=3\n",
    );
    let (d, o) = resolve(&root).unwrap();
    let ram = d.memory.as_ref().unwrap();
    assert_eq!(
        (ram.conventional_kib, ram.extended_kib),
        (Some(512), Some(128))
    );
    assert_eq!(d.cpu.as_ref().unwrap().rate.unwrap().steps.get(), 3);
    assert_eq!(
        o.get("memory.extended_kib").file,
        tree.0.join("middle.yaml")
    );
    assert_eq!(o.get("memory.extended_kib").line, Some(3));
    assert_eq!(
        o.get("memory.conventional_kib").file,
        tree.0.join("base.toml")
    );
    assert_eq!(o.get("cpu.rate").file, root);
    assert_eq!(o.get("cpu.rate").line, Some(4));
    let mut reader = Reader(|_: &Path| Ok(vec![0xF4; 0x8000]));
    let config = load_with_reader(tree.0.join("root.toml"), &mut reader).unwrap();
    assert_eq!(config.clock.retired_per_pit_clock, 3);
}

#[test]
fn every_layer_must_declare_its_schema_before_merging() {
    let tree = Tree::new();
    let parent = tree.write("parent.yaml", "board: {model: ibm-5170}\n");
    let child = tree.write("child.toml", "schema=1\nextends='parent.yaml'\n");
    let LoadError::Parse(e) = resolve(child).unwrap_err() else {
        panic!()
    };
    assert_eq!(e.path(), parent);
    assert!(e.message().contains("missing field `schema`"));
}

#[test]
fn missing_parent_and_unknown_preset_name_the_reference() {
    let tree = Tree::new();
    let child = tree.write("child.toml", "schema=1\nextends='absent.toml'\n");
    let e = message(resolve(&child).unwrap_err());
    assert_eq!(e.origin.file, child);
    assert_eq!(e.origin.line, Some(2));
    assert_eq!(
        e.message,
        format!(
            "parent file not found: {}",
            tree.0.join("absent.toml").display()
        )
    );
    tree.write("child.toml", "schema=1\nextends='preset:unknown'\n");
    let e = message(resolve(&child).unwrap_err());
    assert_eq!(
        e.message,
        "unknown preset `unknown`; known presets: ibm-5170"
    );
    assert_eq!(e.origin.file, child);
    assert_eq!(e.origin.line, Some(2));
}

#[test]
fn absolute_parent_paths_are_rejected() {
    let tree = Tree::new();
    let parent = tree.write("parent.toml", BASE);
    let child = tree.write(
        "child.toml",
        &format!("schema=1\nextends='{}'\n", parent.display()),
    );
    assert_eq!(
        message(resolve(child).unwrap_err()).message,
        "parent must be a relative path or preset:<name>"
    );
}

#[test]
fn completeness_lists_all_required_fields_and_the_chain_before_image_io() {
    let tree = Tree::new();
    let parent = tree.write(
        "parent.toml",
        "schema=1\n[board]\npost_port=128\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\n",
    );
    let child = tree.write(
        "child.yaml",
        "schema: 1\nextends: parent.toml\nfirmware:\n  bios: {provides: [floppy]}\n",
    );
    let mut reader = Reader(|_: &Path| -> io::Result<Vec<u8>> {
        panic!("incomplete machines must fail before image reads")
    });
    let e = message(load_with_reader(&child, &mut reader).unwrap_err());
    assert_eq!(e.field, "board.model");
    assert_eq!(e.origin.file, parent);
    assert_eq!(e.origin.line, Some(2));
    assert_eq!(e.message, format!("required fields are missing: board.model ({}:2), firmware.bios.image ({}:4), card[0].drives[0].image ({}:6), card[0].drives[0].type ({}:6); inheritance chain: parent.toml → {}", parent.display(), child.display(), parent.display(), parent.display(), child.display()));
}

#[test]
fn completeness_covers_nvram_option_rom_and_ide_geometry() {
    let tree = Tree::new();
    let path = tree.write("incomplete.toml", "schema=1\n[board]\nmodel='ibm-5170'\n[board.rtc]\nnvram={write='volatile'}\n[firmware]\noption_roms=[{}]\n[firmware.bios]\nimage='bios.bin'\n[[card]]\nkind='ide'\n[[card.drives]]\ngeometry={heads=4}\n");
    let e = message(resolve(&path).unwrap_err());
    for field in [
        "board.rtc.nvram.image",
        "firmware.option_roms[0].image",
        "firmware.option_roms[0].base",
        "card[0].drives[0].image",
        "card[0].drives[0].channel",
        "card[0].drives[0].geometry.cylinders",
        "card[0].drives[0].geometry.sectors",
    ] {
        assert!(e.message.contains(field), "{field}: {e}");
    }
    assert!(!e.message.contains("geometry.heads"));
}

#[test]
fn relative_images_use_the_defining_layer_even_after_child_overrides() {
    let tree = Tree::new();
    tree.write("base/parent.toml", &format!("{BASE}[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.44M'\nimage='parent.img'\n[[card.drives]]\nunit=1\ntype='1.44M'\nimage='inherited.img'\n"));
    let child = tree.write("child/machine.yaml", "schema: 1\nextends: ../base/parent.toml\ncard:\n  - id: fdc-765\n    drives:\n      - unit: 0\n        image: child.img\n        write: readonly\n");
    let parent = tree.0.join("child/../base/parent.toml");
    let (d, o) = resolve(&child).unwrap();
    assert_eq!(o.get("firmware.bios.image").file, parent);
    assert_eq!(o.get("card[0].drives[0].image").file, child);
    assert_eq!(o.get("card[0].drives[0].image").line, Some(7));
    assert_eq!(o.get("card[0].drives[0].type").file, parent);
    let mut paths = Vec::new();
    let mut reader = Reader(|path: &Path| {
        paths.push(path.to_path_buf());
        Ok(if path.extension().unwrap() == "img" {
            vec![0; 1_474_560]
        } else {
            vec![0xF4; 0x8000]
        })
    });
    lower(&d, &o, &mut reader).unwrap();
    assert_eq!(
        paths,
        [
            tree.0.join("base/bios.bin"),
            tree.0.join("child/child.img"),
            tree.0.join("base/inherited.img")
        ]
    );
}

#[test]
fn image_errors_after_merging_name_the_correct_layer_and_line() {
    let tree = Tree::new();
    let parent = tree.write("parent.toml", BASE);
    let child = tree.write(
        "child.yaml",
        "schema: 1\nextends: parent.toml\nfirmware:\n  bios:\n    base: 0xF8000\n",
    );
    let mut reader = Reader(|_: &Path| {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "synthetic missing image",
        ))
    });
    let e = message(load_with_reader(&child, &mut reader).unwrap_err());
    assert_eq!(e.field, "firmware.bios.image");
    assert_eq!(e.origin.file, parent);
    assert_eq!(e.origin.line, Some(5));
    assert!(e.message.contains("synthetic missing image"));
    tree.write(
        "child.yaml",
        "schema: 1\nextends: parent.toml\nfirmware:\n  bios:\n    image: new.bin\n",
    );
    let e = message(load_with_reader(&child, &mut reader).unwrap_err());
    assert_eq!(e.origin.file, child);
    assert_eq!(e.origin.line, Some(5));
}

#[cfg(unix)]
#[test]
fn symlink_layers_use_target_directories_for_parents_and_images() {
    let tree = Tree::new();
    tree.write(
        "target/parent.yaml",
        "schema: 1\nboard: {model: ibm-5170}\nfirmware: {bios: {image: bios.bin}}\n",
    );
    tree.write("target/content.yaml", "schema=1\nextends='parent.yaml'\n");
    tree.write("alias/parent.yaml", "invalid decoy");
    let alias = tree.0.join("alias/machine.toml");
    std::os::unix::fs::symlink("../target/content.yaml", &alias).unwrap();
    std::fs::write(tree.0.join("target/bios.bin"), vec![0xF4; 0x8000]).unwrap();
    load(&alias).unwrap();
    // An inherited image itself may come from a symlink-spelled parent.
    tree.write("target/parent.toml", BASE);
    std::os::unix::fs::symlink("../target/parent.toml", tree.0.join("alias/linked.toml")).unwrap();
    let child = tree.write("alias/child.toml", "schema=1\nextends='linked.toml'\n");
    load(&child).unwrap();
    std::fs::remove_file(tree.0.join("target/bios.bin")).unwrap();
    let e = message(load(child).unwrap_err());
    assert_eq!(e.origin.file, tree.0.join("alias/linked.toml"));
    assert!(e.message.contains(&format!(
        "{} (resolved to {})",
        tree.0.join("alias/bios.bin").display(),
        tree.0.join("target/bios.bin").display()
    )));
}

fn unpin(d: &mut machine_file::MachineFile) {
    d.firmware.as_mut().unwrap().bios.as_mut().unwrap().sha256 = None;
    for card in d.card.iter_mut().flatten() {
        for drive in card.drives.iter_mut().flatten() {
            drive.sha256 = None;
        }
    }
}
fn machines() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines")
}

#[test]
fn preset_example_equals_freedos_without_fixtures() {
    let (mut a, oa) = resolve(machines().join("at-freedos.toml")).unwrap();
    let (mut b, ob) = resolve(machines().join("at-freedos-from-preset.toml")).unwrap();
    unpin(&mut a);
    unpin(&mut b);
    b.firmware.as_mut().unwrap().bios.as_mut().unwrap().provides = a
        .firmware
        .as_ref()
        .unwrap()
        .bios
        .as_ref()
        .unwrap()
        .provides
        .clone();
    let mut reader = Reader(|path: &Path| {
        Ok(if path.extension().unwrap() == "img" {
            vec![0; 1_474_560]
        } else {
            vec![0xF4; 0x8000]
        })
    });
    let a = lower(&a, &oa, &mut reader).unwrap();
    let b = lower(&b, &ob, &mut reader).unwrap();
    assert_eq!(a, b);
    let mut expected = assembly::bios(vec![0xF4; 0x8000], Some(vec![0; 1_474_560]));
    expected.display_switch = assembly::DisplaySwitch::Colour;
    assert_eq!(b, expected);
}

#[test]
fn preset_example_equals_freedos_with_pinned_fixtures() {
    let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let Some(floppy) = fixtures::fixture("freedos/freedos.img") else {
        return;
    };
    let a = load(machines().join("at-freedos.toml")).unwrap();
    let b = load(machines().join("at-freedos-from-preset.toml")).unwrap();
    assert_eq!(a, b);
    assert_eq!(b, assembly::bios(rom, Some(floppy)));
}

#[test]
fn completeness_reports_uart_irq_for_a_port_without_a_default() {
    let tree = Tree::new();
    let file = tree.write(
        "uart.toml",
        &format!("{BASE}[[card]]\nkind='uart-16550'\nport=0x200\n"),
    );
    let e = message(resolve(file).unwrap_err());
    assert_eq!(e.field, "card[0].irq");
    assert!(e.message.contains("card[0].irq"));
}
