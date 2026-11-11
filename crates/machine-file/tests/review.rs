// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end regressions for machine-files § extends, integrity and input limits.

use machine_file::{load_with_reader, lower, resolve, ImageReader, LoadError};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

const BASE: &str =
    "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n";
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "machine-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, name: &str, input: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, input).unwrap();
        path
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[derive(Default)]
struct Images {
    paths: Vec<PathBuf>,
}
fn bytes(path: &Path) -> Vec<u8> {
    let name = path.file_name().unwrap().to_str().unwrap();
    let length = if name.contains("nvram") {
        64
    } else if name.contains("option") {
        2048
    } else if name.ends_with(".img") {
        1_474_560
    } else {
        0x8000
    };
    let mut bytes = vec![if name.contains("new") { 0xFC } else { 0xF4 }; length];
    if name.contains("option") {
        bytes[..3].copy_from_slice(&[0x55, 0xAA, 4]);
        bytes[length - 1] = 0;
        bytes[length - 1] =
            0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    }
    bytes
}
impl ImageReader for Images {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.paths.push(path.into());
        Ok(bytes(path))
    }
}
fn machine(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../machines")
            .join(name),
    )
    .unwrap()
    .lines()
    .filter(|line| !line.trim_start().starts_with("sha256"))
    .collect::<Vec<_>>()
    .join("\n")
}
fn lower_error(error: LoadError) -> machine_file::LowerError {
    let LoadError::Lower(error) = error else {
        panic!("expected lower error: {error}")
    };
    error
}

#[test]
fn mentioning_reference_uart_without_drives_lowers() {
    let tree = Tree::new();
    tree.write("at-freedos.toml", &machine("at-freedos.toml"));
    let child = tree.write(
        "child.toml",
        "schema=1\nextends='at-freedos.toml'\n[[card]]\nid='uart-16550'\n",
    );
    load_with_reader(child, &mut Images::default()).unwrap();
}
#[test]
fn mentioning_preset_mda_without_drives_lowers() {
    let tree = Tree::new();
    let child = tree.write("child.toml", "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[card]]\nid='mda'\n");
    load_with_reader(child, &mut Images::default()).unwrap();
}
#[test]
fn uart_irq_override_over_preset_example_lowers_and_takes_effect() {
    let tree = Tree::new();
    tree.write(
        "at-freedos-from-preset.toml",
        &machine("at-freedos-from-preset.toml")
            .replace("[firmware.bios]", "[firmware.bios]\nprovides=['floppy']"),
    );
    let child = tree.write(
        "child.toml",
        "schema=1\nextends='at-freedos-from-preset.toml'\n[[card]]\nid='uart-16550'\nirq=4\n",
    );
    let config = load_with_reader(child, &mut Images::default()).unwrap();
    let uart = config
        .devices
        .iter()
        .find(|d| d.id == assembly::DeviceId::Uart)
        .unwrap();
    assert_eq!(uart.irq, Some(4));
}
#[test]
fn every_supported_card_kind_merges_then_lowers_without_invented_drives() {
    let tree = Tree::new();
    for kind in ["mda", "vga-text", "uart-16550", "fdc-765", "ide"] {
        tree.write(
            "parent.toml",
            &format!(
                "{}[[card]]\nkind='{kind}'\n",
                BASE.replace("provides=['floppy']", "provides=['floppy','fixed-disk']")
            ),
        );
        let child = tree.write(
            "child.toml",
            &format!("schema=1\nextends='parent.toml'\n[[card]]\nid='{kind}'\n"),
        );
        let (document, origins) = resolve(child).unwrap();
        assert!(
            document.card.as_ref().unwrap()[0].drives.is_none(),
            "{kind}"
        );
        lower(&document, &origins, &mut Images::default()).unwrap();
    }
}
#[test]
fn inherited_ide_card_and_drive_merge_then_lower() {
    let tree = Tree::new();
    let parent = format!("{}[[card]]\nkind='ide'\nport=0x170\n[[card.drives]]\nchannel='master'\nimage='hard-disk.bin'\ngeometry={{cylinders=8,heads=2,sectors=4}}\nwrite='volatile'\n", BASE.replace("provides=['floppy']", "provides=['floppy','fixed-disk']"));
    tree.write("parent.toml", &parent);
    let child = tree.write("child.toml", "schema=1\nextends='parent.toml'\n[[card]]\nid='ide'\nirq=15\n[[card.drives]]\nchannel='master'\nwrite='readonly'\n");
    let config = load_with_reader(child, &mut Images::default()).unwrap();
    let card = config
        .devices
        .iter()
        .find(|card| card.id == assembly::DeviceId::Ide)
        .unwrap();
    assert_eq!(
        card.kind,
        assembly::DeviceKind::Ide(assembly::IdeCfg {
            base: 0x170,
            control: 0x376
        })
    );
    assert_eq!(card.irq, Some(15));
    assert_eq!(config.drives[0].controller, assembly::DeviceId::Ide);
    assert_eq!(config.drives[0].unit, 0);
    assert_eq!(config.drives[0].write, assembly::WriteMode::ReadOnly);
    assert!(
        matches!(&config.drives[0].kind, assembly::DriveKind::HardDisk { geometry } if geometry.cylinders == 8 && geometry.heads == 2 && geometry.sectors_per_track == 4)
    );
}

fn image_layer(kind: &str, image: Option<&str>, pin: Option<&str>) -> String {
    let header = match kind {
        "bios" => "[firmware.bios]\n".to_owned(),
        "option" => "[firmware]\noption_roms=[{base=0xC8000,".to_owned(),
        "nvram" => "[board.rtc.nvram]\n".to_owned(),
        "drive" => "[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.44M'\n".to_owned(),
        _ => unreachable!(),
    };
    let delimiter = if kind == "option" { "," } else { "\n" };
    let mut result = header;
    if let Some(image) = image {
        result.push_str(&format!("image='{image}'{delimiter}"));
    }
    if let Some(pin) = pin {
        result.push_str(&format!("sha256='{pin}'{delimiter}"));
    }
    if kind == "option" {
        result.push_str("}]\n");
    }
    result
}
fn image_name(kind: &str, new: bool) -> String {
    format!(
        "{}-{kind}.{}",
        if new { "new" } else { "old" },
        if kind == "drive" { "img" } else { "bin" }
    )
}
fn pinned_parent(kind: &str) -> String {
    let name = image_name(kind, false);
    let pin = format!("{:x}", Sha256::digest(bytes(Path::new(&name))));
    let base: String = if kind == "bios" {
        "schema=1\n[board]\nmodel='ibm-5170'\n".into()
    } else {
        BASE.into()
    };
    let layer = image_layer(kind, Some(&name), Some(&pin));
    let layer = if kind == "drive" {
        layer.replace("id='fdc-765'\n", "id='fdc-765'\nkind='fdc-765'\n")
    } else {
        layer
    };
    format!("{base}{layer}")
}
#[test]
fn image_override_without_pin_loads_for_each_image_family() {
    let tree = Tree::new();
    for kind in ["bios", "option", "nvram", "drive"] {
        tree.write("parent.toml", &pinned_parent(kind));
        let name = image_name(kind, true);
        let child = tree.write(
            "child.toml",
            &format!(
                "schema=1\nextends='parent.toml'\n{}",
                image_layer(kind, Some(&name), None)
            ),
        );
        let mut images = Images::default();
        machine_file::load_with_reader_and_options(
            child,
            &mut images,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: kind == "option",
            },
        )
        .unwrap();
        assert!(
            images
                .paths
                .iter()
                .any(|p| p.file_name().unwrap() == name.as_str()),
            "{kind}"
        );
    }
}
#[test]
fn image_override_uses_its_own_pin_and_reports_the_pin_line() {
    let tree = Tree::new();
    for kind in ["bios", "option", "nvram", "drive"] {
        tree.write("parent.toml", &pinned_parent(kind));
        let name = image_name(kind, true);
        let valid = format!("{:x}", Sha256::digest(bytes(Path::new(&name))));
        let child = tree.write(
            "child.toml",
            &format!(
                "schema=1\nextends='parent.toml'\n{}",
                image_layer(kind, Some(&name), Some(&valid))
            ),
        );
        machine_file::load_with_reader_and_options(
            &child,
            &mut Images::default(),
            assembly::AssemblyOptions {
                allow_unlisted_firmware: kind == "option",
            },
        )
        .unwrap();
        let input = format!(
            "schema=1\nextends='parent.toml'\n{}",
            image_layer(kind, Some(&name), Some(&"0".repeat(64)))
        );
        tree.write("child.toml", &input);
        let line = input.lines().position(|l| l.contains("sha256")).unwrap() + 1;
        let error = lower_error(load_with_reader(&child, &mut Images::default()).unwrap_err());
        assert!(
            error.message.starts_with("sha256 mismatch for"),
            "{kind}: {error}"
        );
        assert!(
            error
                .message
                .contains(&format!("pin set at {}:{line}", child.display())),
            "{kind}: {error}"
        );
        assert!(error.message.contains(&format!("actual {valid}")));
    }
}
#[test]
fn repinning_an_inherited_image_uses_the_new_pins_origin() {
    let tree = Tree::new();
    for kind in ["bios", "nvram", "drive"] {
        tree.write("parent.toml", &pinned_parent(kind));
        let name = image_name(kind, false);
        let pin = format!("{:x}", Sha256::digest(bytes(Path::new(&name))));
        let input = format!(
            "schema=1\nextends='parent.toml'\n{}",
            image_layer(kind, None, Some(&pin))
        );
        let child = tree.write("child.toml", &input);
        load_with_reader(&child, &mut Images::default()).unwrap();
        let input = input.replace(&pin, &"0".repeat(64));
        tree.write("child.toml", &input);
        let line = input.lines().position(|l| l.contains("sha256")).unwrap() + 1;
        let (d, o) = resolve(&child).unwrap();
        let error = lower(&d, &o, &mut Images::default()).unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("pin set at {}:{line}", child.display())),
            "{kind}: {error}"
        );
        assert!(
            error.message.contains("old-"),
            "the image still comes from the parent"
        );
    }
}

#[test]
fn oversized_files_and_parse_inputs_have_an_exact_path_error() {
    let tree = Tree::new();
    for ext in ["toml", "yaml", "yml"] {
        let input = format!(
            "{}\n{}",
            if ext == "toml" {
                "schema=1"
            } else {
                "schema: 1"
            },
            " ".repeat(1_048_576)
        );
        let path = tree.write(&format!("large.{ext}"), &input);
        let LoadError::Parse(error) = resolve(&path).unwrap_err() else {
            panic!()
        };
        assert_eq!(error.path(), path);
        assert_eq!(
            error.message(),
            "machine file exceeds size limit of 1048576 bytes"
        );
        let result = if ext == "toml" {
            machine_file::parse_toml(&path, &input)
        } else {
            machine_file::parse_yaml(&path, &input)
        };
        assert_eq!(
            result.unwrap_err().message(),
            "machine file exceeds size limit of 1048576 bytes"
        );
    }
}
#[test]
fn unreadable_parent_names_the_child_extends_origin() {
    let tree = Tree::new();
    std::fs::create_dir(tree.0.join("dir.toml")).unwrap();
    let child = tree.write("child.toml", "schema=1\nextends='dir.toml'\n");
    let error = lower_error(resolve(&child).unwrap_err());
    assert_eq!(error.origin.file, child);
    assert_eq!(error.origin.line, Some(2));
    assert_eq!(error.field, "extends");
    assert!(error.message.starts_with(&format!(
        "cannot read parent {}:",
        tree.0.join("dir.toml").display()
    )));
}
// Windows forbids ':' in file names; exercise this spelling on Unix.
#[cfg(unix)]
#[test]
fn literal_preset_prefixed_filenames_load_from_filesystem() {
    if std::env::var_os("MACHINE_LITERAL_PROBE").is_some() {
        load_with_reader("preset:literal.toml", &mut Images::default()).unwrap();
        return;
    }
    let tree = Tree::new();
    let literal = tree.write("preset:literal.toml", BASE);
    load_with_reader(literal, &mut Images::default()).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "literal_preset_prefixed_filenames_load_from_filesystem",
        ])
        .current_dir(&tree.0)
        .env("MACHINE_LITERAL_PROBE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn caller_names_are_files_and_only_extends_interprets_preset_names() {
    let tree = Tree::new();
    let LoadError::Parse(error) = resolve("preset:ibm-5170").unwrap_err() else {
        panic!()
    };
    assert_eq!(error.path(), Path::new("preset:ibm-5170"));
    assert_eq!(
        error.message(),
        "expected a machine file with extension .toml, .yaml or .yml"
    );
    let child = tree.write(
        "child.toml",
        "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n",
    );
    load_with_reader(child, &mut Images::default()).unwrap();
}

#[test]
fn cycle_chain_uses_exactly_the_written_names() {
    if std::env::var_os("MACHINE_CYCLE_PROBE").is_some() {
        let error = lower_error(resolve("a.toml").unwrap_err());
        assert_eq!(error.message, "inheritance cycle: a.toml → b.yaml → a.toml");
        return;
    }
    let tree = Tree::new();
    tree.write("a.toml", "schema=1\nextends='b.yaml'\n");
    tree.write("b.yaml", "schema: 1\nextends: a.toml\n");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "cycle_chain_uses_exactly_the_written_names"])
        .current_dir(&tree.0)
        .env("MACHINE_CYCLE_PROBE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn exact_size_limit_is_accepted_for_both_syntaxes() {
    let tree = Tree::new();
    for (name, prefix) in [
        ("edge.toml", BASE),
        (
            "edge.yaml",
            "schema: 1\nboard: {model: ibm-5170}\nfirmware: {bios: {image: bios.bin}}\n",
        ),
    ] {
        let input = format!("{prefix}{}", " ".repeat(1_048_576 - prefix.len()));
        let file = tree.write(name, &input);
        load_with_reader(file, &mut Images::default()).unwrap();
    }
}

fn assert_relative(document: &machine_file::MachineFile, file: &Path) {
    let mut paths = Vec::new();
    if let Some(parent) = &document.extends {
        if !parent.starts_with("preset:") {
            paths.push(parent.as_str());
        }
    }
    if let Some(firmware) = &document.firmware {
        for rom in firmware
            .bios
            .iter()
            .chain(firmware.option_roms.iter().flatten())
        {
            if let Some(image) = &rom.image {
                paths.push(image);
            }
        }
    }
    if let Some(machine_file::schema::v1::Nvram::Image(image)) = document
        .board
        .as_ref()
        .and_then(|b| b.rtc.as_ref())
        .and_then(|r| r.nvram.as_ref())
    {
        if let Some(path) = &image.image {
            paths.push(path);
        }
        if let Some(path) = &image.journal {
            paths.push(path);
        }
    }
    for card in document.card.iter().flatten() {
        for drive in card.drives.iter().flatten() {
            if let Some(path) = &drive.image {
                paths.push(path);
            }
            if let Some(path) = &drive.journal {
                paths.push(path);
            }
        }
    }
    for path in paths {
        assert!(
            !Path::new(path).is_absolute(),
            "{} contains absolute path {path}",
            file.display()
        );
    }
}
fn machine_paths(directory: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(machine_paths(&path));
        } else if path
            .extension()
            .is_some_and(|e| e == "toml" || e == "yaml" || e == "yml")
        {
            result.push(path);
        }
    }
    result.sort();
    result
}
#[test]
fn every_repository_machine_uses_relative_paths() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines");
    let paths = machine_paths(&root);
    assert!(paths.len() >= 4);
    for path in paths {
        let input = std::fs::read_to_string(&path).unwrap();
        let document = if path.extension().unwrap() == "toml" {
            machine_file::parse_toml(&path, &input)
        } else {
            machine_file::parse_yaml(&path, &input)
        }
        .unwrap();
        assert_relative(&document, &path);
    }
}

#[test]
fn direct_lowering_retains_the_pin_line_for_both_syntaxes() {
    let pin = "0".repeat(64);
    for (file, input, yaml) in [
        ("direct.toml", format!("{BASE}sha256='{pin}'\n"), false),
        ("direct.yaml", format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios:\n    image: bios.bin\n    sha256: '{pin}'\n"), true),
    ] {
        let (document, origins) = if yaml {
            (machine_file::parse_yaml(file, &input).unwrap(), machine_file::Origins::from_yaml(file, &input).unwrap())
        } else {
            (machine_file::parse_toml(file, &input).unwrap(), machine_file::Origins::from_toml(file, &input).unwrap())
        };
        let error = lower(&document, &origins, &mut Images::default()).unwrap_err();
        assert_eq!(error.pin_origin.as_deref(), Some(origins.get("firmware.bios.sha256")));
        assert!(error.message.ends_with(&format!("pin set at {file}:{}", if yaml { 6 } else { 7 })), "{error}");
    }
}

#[test]
fn omitting_image_and_pin_keeps_the_inherited_pin_and_its_origin() {
    let tree = Tree::new();
    let parent = tree.write(
        "parent.toml",
        &format!("{BASE}sha256='{}'\n", "0".repeat(64)),
    );
    let child = tree.write(
        "child.toml",
        "schema=1\nextends='parent.toml'\n[firmware.bios]\nbase=0xF8000\n",
    );
    let error = lower_error(load_with_reader(child, &mut Images::default()).unwrap_err());
    assert!(error
        .message
        .ends_with(&format!("pin set at {}:7", parent.display())));
}

#[test]
fn unresolved_extends_error_points_to_the_resolution_api() {
    let input = "schema=1\nextends='parent.toml'\n";
    let document = machine_file::parse_toml("child.toml", input).unwrap();
    let error = lower(
        &document,
        &machine_file::Origins::new("child.toml"),
        &mut Images::default(),
    )
    .unwrap_err();
    assert_eq!(
        error.message,
        "unresolved extends; call resolve before lowering"
    );
}

#[test]
fn oversized_utf8_is_rejected_before_decoding_a_partial_character() {
    let tree = Tree::new();
    let input = format!("{BASE}{}𝄞", " ".repeat(1_048_575 - BASE.len()));
    let path = tree.write("utf8.toml", &input);
    let LoadError::Parse(error) = resolve(&path).unwrap_err() else {
        panic!()
    };
    assert_eq!(error.path(), path);
    assert_eq!(
        error.message(),
        "machine file exceeds size limit of 1048576 bytes"
    );
}

#[test]
fn wrong_extension_parent_names_the_child_extends_origin() {
    let tree = Tree::new();
    let parent = tree.write("base.tml", BASE);
    let child = tree.write("child.toml", "schema=1\nextends='base.tml'\n");
    let error = lower_error(resolve(&child).unwrap_err());
    assert_eq!(error.origin.file, child);
    assert_eq!(error.origin.line, Some(2));
    assert_eq!(error.field, "extends");
    assert_eq!(
        error.message,
        format!(
            "parent {}: expected a machine file with extension .toml, .yaml or .yml",
            parent.display()
        )
    );
}

#[test]
fn oversized_parent_names_the_child_extends_origin() {
    let tree = Tree::new();
    for extension in ["toml", "yaml"] {
        let parent = tree.write(&format!("large.{extension}"), &" ".repeat(1_048_577));
        let child = tree.write(
            "child.toml",
            &format!("schema=1\nextends='large.{extension}'\n"),
        );
        let error = lower_error(resolve(&child).unwrap_err());
        assert_eq!(error.origin.file, child);
        assert_eq!(error.origin.line, Some(2));
        assert_eq!(error.field, "extends");
        assert_eq!(
            error.message,
            format!(
                "parent {}: machine file exceeds size limit of 1048576 bytes",
                parent.display()
            )
        );
    }
}

#[test]
fn completeness_chain_preserves_spelling_and_origins_keep_io_paths() {
    let tree = Tree::new();
    std::fs::create_dir(tree.0.join("entry")).unwrap();
    std::fs::create_dir(tree.0.join("sub")).unwrap();
    tree.write("sub/b.yaml", "schema: 1\nboard:\n  post_port: 128\n");
    let child = tree.write("entry/child.toml", "schema=1\nextends='../sub/b.yaml'\n");
    let error = lower_error(resolve(&child).unwrap_err());
    assert_eq!(error.origin.file, tree.0.join("entry/../sub/b.yaml"));
    assert_eq!(error.origin.line, Some(3));
    assert!(
        error.message.ends_with(&format!(
            "inheritance chain: ../sub/b.yaml → {}",
            child.display()
        )),
        "{error}"
    );
}

#[test]
fn option_nvram_and_drive_pins_retain_exact_diagnostics_in_both_syntaxes() {
    let pin = "0".repeat(64);
    let tree = Tree::new();
    for name in ["bios.bin", "option.bin", "nvram.bin", "floppy.img"] {
        std::fs::write(tree.0.join(name), bytes(Path::new(name))).unwrap();
    }
    let yaml_base = "schema: 1\nboard: {model: ibm-5170}\nfirmware:\n  bios: {image: bios.bin, provides: [floppy]}\n";
    for (label, image, toml, toml_line, yaml, yaml_line) in [
        (
            "firmware.option_roms[0]",
            "option.bin",
            format!("{BASE}[[firmware.option_roms]]\nbase=0xC8000\nimage='option.bin'\nsha256='{pin}'\n"),
            10,
            format!("{yaml_base}  option_roms:\n    - base: 819200\n      image: option.bin\n      sha256: '{pin}'\n"),
            8,
        ),
        (
            "board.rtc.nvram",
            "nvram.bin",
            format!("{BASE}[board.rtc.nvram]\nimage='nvram.bin'\nsha256='{pin}'\n"),
            9,
            format!("schema: 1\nboard:\n  model: ibm-5170\n  rtc:\n    nvram:\n      image: nvram.bin\n      sha256: '{pin}'\nfirmware:\n  bios: {{image: bios.bin}}\n"),
            7,
        ),
        (
            "card[storage].drives[unit=1]",
            "floppy.img",
            format!("{BASE}[[card]]\nkind='uart-16550'\n[[card]]\nid='storage'\nkind='fdc-765'\n[[card.drives]]\nunit=1\ntype='1.44M'\nimage='floppy.img'\nsha256='{pin}'\n"),
            16,
            format!("{yaml_base}card:\n  - kind: uart-16550\n  - id: storage\n    kind: fdc-765\n    drives:\n      - unit: 1\n        type: 1.44M\n        image: floppy.img\n        sha256: '{pin}'\n"),
            13,
        ),
    ] {
        for (ext, input, line) in [("toml", toml, toml_line), ("yaml", yaml, yaml_line)] {
            let path = tree.write(&format!("machine.{ext}"), &input);
            let (document, origins) = resolve(&path).unwrap();
            let expected_origin = machine_file::Origin { file: path.clone(), line: Some(line) };
            let expected = format!(
                "sha256 mismatch for {}: expected {pin}, actual {:x}; pin set at {expected_origin}",
                tree.0.join(image).display(),
                Sha256::digest(bytes(Path::new(image))),
            );
            let error = lower(&document, &origins, &mut Images::default()).unwrap_err();
            assert_eq!(error.field, format!("{label}.image"));
            assert_eq!(error.pin_origin.as_deref(), Some(&expected_origin));
            assert_eq!(error.message, expected);
            let error = lower_error(
                machine_file::load_for_assembly(&path, assembly::AssemblyOptions::default())
                    .err()
                    .unwrap(),
            );
            assert_eq!(error.pin_origin.as_deref(), Some(&expected_origin));
            assert_eq!(error.message, expected);
        }
    }
}

#[test]
fn inherited_pins_keep_their_source_separate_from_overridden_images_in_both_syntaxes() {
    let tree = Tree::new();
    let pin = "0".repeat(64);
    for ext in ["toml", "yaml"] {
        for kind in ["bios", "nvram", "drive"] {
            let name = image_name(kind, true);
            let (base, image_layer, pin_layer, child_layer) = if ext == "toml" {
                (
                    pinned_parent(kind),
                    format!(
                        "schema=1\nextends='base.toml'\n{}",
                        image_layer(kind, Some(&name), None)
                    ),
                    format!(
                        "schema=1\nextends='image.toml'\n{}",
                        image_layer(kind, None, Some(&pin))
                    ),
                    "schema=1\nextends='pin.toml'\n[memory]\nextended_kib=1024\n".to_owned(),
                )
            } else {
                let base = match kind {
                    "bios" => format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios: {{image: old-bios.bin, sha256: '{pin}'}}\n"),
                    "nvram" => format!("schema: 1\nboard:\n  model: ibm-5170\n  rtc:\n    nvram: {{image: old-nvram.bin, sha256: '{pin}'}}\nfirmware:\n  bios: {{image: bios.bin}}\n"),
                    "drive" => format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios: {{image: bios.bin, provides: [floppy]}}\ncard:\n  - id: storage\n    kind: fdc-765\n    drives:\n      - {{unit: 0, type: 1.44M, image: old-drive.img, sha256: '{pin}'}}\n"),
                    _ => unreachable!(),
                };
                let layer = |value: &str| match kind {
                    "bios" => format!("firmware:\n  bios: {{{value}}}\n"),
                    "nvram" => format!("board:\n  rtc:\n    nvram: {{{value}}}\n"),
                    "drive" => format!(
                        "card:\n  - id: storage\n    drives:\n      - {{unit: 0, {value}}}\n"
                    ),
                    _ => unreachable!(),
                };
                (
                    base,
                    format!(
                        "schema: 1\nextends: base.yaml\n{}",
                        layer(&format!("image: {name}"))
                    ),
                    format!(
                        "schema: 1\nextends: image.yaml\n{}",
                        layer(&format!("sha256: '{pin}'"))
                    ),
                    "schema: 1\nextends: pin.yaml\nmemory: {extended_kib: 1024}\n".to_owned(),
                )
            };
            tree.write(&format!("base.{ext}"), &base);
            let image_file = tree.write(&format!("image.{ext}"), &image_layer);
            let (document, _) = resolve(&image_file).unwrap();
            let image_pin = match kind {
                "bios" => {
                    &document
                        .firmware
                        .as_ref()
                        .unwrap()
                        .bios
                        .as_ref()
                        .unwrap()
                        .sha256
                }
                "nvram" => {
                    let machine_file::schema::v1::Nvram::Image(image) = document
                        .board
                        .as_ref()
                        .unwrap()
                        .rtc
                        .as_ref()
                        .unwrap()
                        .nvram
                        .as_ref()
                        .unwrap()
                    else {
                        panic!("expected NVRAM image")
                    };
                    &image.sha256
                }
                "drive" => &document.card.as_ref().unwrap()[0].drives.as_ref().unwrap()[0].sha256,
                _ => unreachable!(),
            };
            assert!(
                image_pin.is_none(),
                "an overriding image clears the earlier pin"
            );
            let pin_file = tree.write(&format!("pin.{ext}"), &pin_layer);
            let child = tree.write(&format!("child.{ext}"), &child_layer);
            let (document, origins) = resolve(&child).unwrap();
            let error = lower(&document, &origins, &mut Images::default()).unwrap_err();
            let line = pin_layer
                .lines()
                .position(|line| line.contains("sha256"))
                .unwrap()
                + 1;
            let expected_origin = machine_file::Origin {
                file: pin_file,
                line: Some(line),
            };
            assert_eq!(error.origin.file, image_file);
            assert_eq!(error.pin_origin.as_deref(), Some(&expected_origin));
            assert_eq!(
                error.message,
                format!(
                "sha256 mismatch for {}: expected {pin}, actual {:x}; pin set at {expected_origin}",
                tree.0.join(&name).display(),
                Sha256::digest(bytes(Path::new(&name))),
            )
            );
        }
    }
}

fn out_of_order_drive_pin_documents(pin: &str) -> [(&'static str, String, usize); 2] {
    let valid = format!("{:x}", Sha256::digest(bytes(Path::new("one.img"))));
    [
        (
            "toml",
            format!("{BASE}[[card]]\nid='storage'\nkind='fdc-765'\n[[card.drives]]\nunit=1\ntype='1.44M'\nimage='one.img'\nsha256='{valid}'\n[[card.drives]]\nunit=0\ntype='1.44M'\nimage='zero.img'\nsha256='{pin}'\n"),
            19,
        ),
        (
            "yaml",
            format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios: {{image: bios.bin, provides: [floppy]}}\ncard:\n  - id: storage\n    kind: fdc-765\n    drives:\n      - unit: 1\n        type: 1.44M\n        image: one.img\n        sha256: '{valid}'\n      - unit: 0\n        type: 1.44M\n        image: zero.img\n        sha256: '{pin}'\n"),
            16,
        ),
    ]
}

#[test]
fn sorted_drive_pin_mismatch_uses_the_document_origin_in_both_syntaxes() {
    let tree = Tree::new();
    let pin = "0".repeat(64);
    for (ext, input, line) in out_of_order_drive_pin_documents(&pin) {
        let file = tree.write(&format!("sorted.{ext}"), &input);
        let (document, origins) = resolve(&file).unwrap();
        let drives = document.card.as_ref().unwrap()[0].drives.as_ref().unwrap();
        assert_eq!(
            drives.iter().map(|drive| drive.unit).collect::<Vec<_>>(),
            vec![Some(1), Some(0)]
        );
        let mut images = Images::default();
        let error = lower(&document, &origins, &mut images).unwrap_err();
        let pin_origin = machine_file::Origin {
            file,
            line: Some(line),
        };
        assert_eq!(error.field, "card[storage].drives[unit=0].image");
        assert_eq!(error.pin_origin.as_deref(), Some(&pin_origin));
        assert_eq!(
            error.message,
            format!(
                "sha256 mismatch for {}: expected {pin}, actual {:x}; pin set at {pin_origin}",
                tree.0.join("zero.img").display(),
                Sha256::digest(bytes(Path::new("zero.img"))),
            )
        );
        assert_eq!(
            images.paths,
            vec![tree.0.join("bios.bin"), tree.0.join("zero.img")]
        );
    }
}

#[test]
fn drive_pin_mismatch_keeps_its_parent_origin_after_removal_in_both_syntaxes() {
    let tree = Tree::new();
    let pin = "0".repeat(64);
    for (ext, input, line) in out_of_order_drive_pin_documents(&pin) {
        let parent = tree.write(&format!("parent.{ext}"), &input);
        let child = if ext == "toml" {
            "schema=1\nextends='parent.toml'\n[[card]]\nid='storage'\n[[card.drives]]\nunit=1\nremove=true\n"
        } else {
            "schema: 1\nextends: parent.yaml\ncard:\n  - id: storage\n    drives:\n      - {unit: 1, remove: true}\n"
        };
        let file = tree.write(&format!("child.{ext}"), child);
        let (document, origins) = resolve(&file).unwrap();
        let drives = document.card.as_ref().unwrap()[0].drives.as_ref().unwrap();
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].unit, Some(0));
        let error = lower(&document, &origins, &mut Images::default()).unwrap_err();
        let pin_origin = machine_file::Origin {
            file: parent.clone(),
            line: Some(line),
        };
        assert_eq!(error.field, "card[storage].drives[unit=0].image");
        assert_eq!(error.origin.file, parent);
        assert_eq!(error.pin_origin.as_deref(), Some(&pin_origin));
        assert_eq!(
            error.message,
            format!(
                "sha256 mismatch for {}: expected {pin}, actual {:x}; pin set at {pin_origin}",
                tree.0.join("zero.img").display(),
                Sha256::digest(bytes(Path::new("zero.img"))),
            )
        );
    }
}

#[test]
fn ide_drive_pin_mismatch_retains_its_exact_origin_in_both_syntaxes() {
    let tree = Tree::new();
    let pin = "0".repeat(64);
    for (ext, input, line) in [
        (
            "toml",
            format!("{BASE}[[card]]\nid='disk'\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={{cylinders=8,heads=2,sectors=4}}\nimage='disk.img'\nsha256='{pin}'\n"),
            14,
        ),
        (
            "yaml",
            format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios: {{image: bios.bin}}\ncard:\n  - id: disk\n    kind: ide\n    drives:\n      - channel: master\n        geometry: {{cylinders: 8, heads: 2, sectors: 4}}\n        image: disk.img\n        sha256: '{pin}'\n"),
            12,
        ),
    ] {
        let file = tree.write(&format!("ide.{ext}"), &input);
        let (document, origins) = resolve(&file).unwrap();
        let error = lower(&document, &origins, &mut Images::default()).unwrap_err();
        let pin_origin = machine_file::Origin { file, line: Some(line) };
        assert_eq!(error.field, "card[disk].drives[channel=master].image");
        assert_eq!(error.pin_origin.as_deref(), Some(&pin_origin));
        assert_eq!(error.message, format!(
            "sha256 mismatch for {}: expected {pin}, actual {:x}; pin set at {pin_origin}",
            tree.0.join("disk.img").display(),
            Sha256::digest(bytes(Path::new("disk.img"))),
        ));
    }
}
