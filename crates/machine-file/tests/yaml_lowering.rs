// SPDX-License-Identifier: AGPL-3.0-or-later
//! YAML lowering, path and syntax-equivalence integration tests.

use machine_file::{lower, parse_toml, parse_yaml, ImageReader, MachineFile, Origins};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

struct Reader<F>(F);
impl<F: FnMut(&Path) -> io::Result<Vec<u8>>> ImageReader for Reader<F> {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        (self.0)(path)
    }
}

const MINIMAL: &str =
    "schema: 1\nboard: {model: ibm-5170}\nfirmware:\n  bios:\n    image: bios.bin\n";

fn run(input: &str) -> Result<assembly::MachineConfig, machine_file::LowerError> {
    let file = Path::new("machines/test.yaml");
    let mut reader = Reader(|_path: &Path| Ok(vec![0xF4; 0x8000]));
    lower(
        &parse_yaml(file, input).unwrap(),
        &Origins::from_yaml(file, input).unwrap(),
        &mut reader,
    )
}

#[test]
fn at_adapter_yaml_selects_register_ownership_and_keeps_ata_as_default() {
    for (setting, model) in [
        ("", assembly::AtaControllerModel::Ata),
        ("controller_model: ata", assembly::AtaControllerModel::Ata),
        (
            "controller_model: at-adapter",
            assembly::AtaControllerModel::AtAdapter,
        ),
    ] {
        let config = run(&format!(
            "{MINIMAL}    provides: [fixed-disk]\ncard:\n  - kind: ide\n    {setting}\n"
        ))
        .unwrap();
        let ide = config
            .devices
            .iter()
            .find(|device| device.id == assembly::DeviceId::Ide)
            .unwrap();
        assert!(matches!(ide.kind, assembly::DeviceKind::Ide(cfg) if cfg.model == model));
    }
}

fn twins(directory: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut pairs = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            pairs.extend(twins(&path)?);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "yaml" || ext == "yml")
        {
            let toml = path.with_extension("toml");
            if !toml.is_file() {
                return Err(format!("{} has no .toml twin", path.display()));
            }
            let other = path.with_extension(if path.extension().unwrap() == "yaml" {
                "yml"
            } else {
                "yaml"
            });
            if other.is_file() {
                return Err(format!("{} has both .yaml and .yml twins", toml.display()));
            }
            pairs.push((toml, path));
        }
    }
    Ok(pairs)
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines")
}

fn repository_twins() -> Vec<(PathBuf, PathBuf)> {
    let pairs = twins(&repository_root()).unwrap();
    assert!(!pairs.is_empty(), "at least the FreeDOS pair must exist");
    pairs
}

fn unpin(document: &mut MachineFile) {
    if let Some(firmware) = &mut document.firmware {
        if let Some(bios) = &mut firmware.bios {
            bios.sha256 = None;
        }
        for rom in firmware.option_roms.iter_mut().flatten() {
            rom.sha256 = None;
        }
    }
    if let Some(rtc) = document.board.as_mut().and_then(|board| board.rtc.as_mut()) {
        if let Some(machine_file::schema::v1::Nvram::Image(image)) = &mut rtc.nvram {
            image.sha256 = None;
        }
    }
    for card in document.card.iter_mut().flatten() {
        for drive in card.drives.iter_mut().flatten() {
            drive.sha256 = None;
        }
    }
}

fn check_synthetic_twins(root: &Path) -> Result<(), String> {
    for (toml, yaml) in twins(root)? {
        let toml_input = std::fs::read_to_string(&toml).map_err(|error| error.to_string())?;
        let yaml_input = std::fs::read_to_string(&yaml).map_err(|error| error.to_string())?;
        let mut a = parse_toml(&toml, &toml_input).map_err(|error| error.to_string())?;
        let mut b = parse_yaml(&yaml, &yaml_input).map_err(|error| error.to_string())?;
        unpin(&mut a);
        unpin(&mut b);
        let mut dummy = Reader(|path: &Path| {
            Ok(if path.extension().is_some_and(|ext| ext == "img") {
                vec![0; 1_474_560]
            } else {
                vec![0xF4; 0x8000]
            })
        });
        let a = lower(&a, &Origins::new(&toml), &mut dummy).map_err(|error| error.to_string())?;
        let b = lower(&b, &Origins::new(&yaml), &mut dummy).map_err(|error| error.to_string())?;
        if a != b {
            return Err(format!(
                "{} and {} lower to different configs",
                toml.display(),
                yaml.display()
            ));
        }
        if toml.file_stem().unwrap() == "at-freedos" {
            let mut expected = assembly::bios(vec![0xF4; 0x8000], Some(vec![0; 1_474_560]));
            expected.display_switch = assembly::DisplaySwitch::Colour;
            assert_eq!(b, expected);
        }
    }
    Ok(())
}

#[test]
fn every_repository_twin_lowers_equally_without_fixtures() {
    assert!(!repository_twins().is_empty());
    check_synthetic_twins(&repository_root()).unwrap();
}

#[test]
fn twin_discovery_rejects_orphan_yaml_files_recursively() {
    let tree = TempTree::new();
    let nested = tree.0.join("nested");
    std::fs::create_dir(&nested).unwrap();
    for ext in ["yaml", "yml"] {
        let file = nested.join(format!("orphan.{ext}"));
        std::fs::write(&file, "invalid machine").unwrap();
        assert!(check_synthetic_twins(&tree.0)
            .unwrap_err()
            .contains("has no .toml twin"));
        std::fs::remove_file(file).unwrap();
    }
}

#[test]
fn twin_discovery_compares_yml_and_allows_toml_only_files() {
    let tree = TempTree::new();
    std::fs::write(
        tree.0.join("test.toml"),
        "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n",
    )
    .unwrap();
    check_synthetic_twins(&tree.0).unwrap();
    let yaml = tree.0.join("test.yml");
    std::fs::write(
        &yaml,
        format!("{MINIMAL}memory: {{conventional_kib: 512}}\n"),
    )
    .unwrap();
    assert!(check_synthetic_twins(&tree.0)
        .unwrap_err()
        .contains("lower to different configs"));
    std::fs::write(yaml, MINIMAL).unwrap();
    check_synthetic_twins(&tree.0).unwrap();
}

#[test]
fn twin_discovery_rejects_two_yaml_spellings_for_one_stem() {
    let tree = TempTree::new();
    std::fs::write(
        tree.0.join("test.toml"),
        "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n",
    )
    .unwrap();
    for ext in ["yaml", "yml"] {
        std::fs::write(tree.0.join(format!("test.{ext}")), MINIMAL).unwrap();
    }
    assert!(check_synthetic_twins(&tree.0)
        .unwrap_err()
        .contains("both .yaml and .yml twins"));
}

#[test]
fn every_repository_twin_loads_equally_with_pinned_fixtures() {
    let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let Some(floppy) = fixtures::fixture("freedos/freedos.img") else {
        return;
    };
    for (toml, yaml) in repository_twins() {
        let a = machine_file::load(&toml).unwrap();
        let b = machine_file::load(&yaml).unwrap();
        assert_eq!(a, b);
        if toml.file_stem().unwrap() == "at-freedos" {
            assert_eq!(b, assembly::bios(rom.clone(), Some(floppy.clone())));
        }
    }
}

#[test]
fn hash_mismatch_reports_both_hashes_and_the_defining_origin() {
    use sha2::{Digest, Sha256};
    let pin = "0".repeat(64);
    let error = run(&format!("{MINIMAL}    sha256: '{pin}'\n")).unwrap_err();
    assert_eq!(error.field, "firmware.bios.image");
    assert_eq!(error.origin.line, Some(5));
    assert!(error.message.contains(&format!(
        "expected {pin}, actual {:x}",
        Sha256::digest(vec![0xF4; 0x8000])
    )));
}

#[test]
fn missing_fields_and_validation_use_the_same_labels_as_toml() {
    let error = run("schema: 1\n").unwrap_err();
    assert_eq!(error.field, "board.model");
    assert_eq!(error.message, "required field is missing");
    let error = run(&format!(
        "{MINIMAL}card:\n  - id: console\n    kind: uart-16550\n    irq: 8\n"
    ))
    .unwrap_err();
    assert_eq!(error.field, "card[console]");
    assert_eq!(error.origin.line, Some(7));
    assert!(error.validation.is_some());
}

#[test]
fn duplicate_card_ids_report_both_origins() {
    for cards in [
        "  - kind: mda\n  - kind: mda\n",
        "  - {id: console, kind: mda}\n  - {id: console, kind: vga-text}\n",
        "  - {id: mda, kind: vga-text}\n  - kind: mda\n",
    ] {
        let error = run(&format!("{MINIMAL}card:\n{cards}")).unwrap_err();
        assert!(error.message.contains("duplicate card id"), "{error}");
        assert!(error.to_string().contains("test.yaml:7"), "{error}");
        assert!(error.to_string().contains("test.yaml:8"), "{error}");
    }
}

struct TempTree(PathBuf);
impl TempTree {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "machine-file-yaml-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempTree {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn relative_and_absolute_images_resolve_from_the_machine_directory() {
    let tree = TempTree::new();
    let directory = tree.0.join("machines");
    std::fs::create_dir(&directory).unwrap();
    let rom = vec![0xF4; 0x8000];
    let image = directory.join("bios.bin");
    std::fs::write(&image, &rom).unwrap();
    for extension in ["yaml", "yml"] {
        let file = directory.join(format!("test.{extension}"));
        for input in [
            MINIMAL.to_owned(),
            MINIMAL.replace("bios.bin", image.to_str().unwrap()),
        ] {
            std::fs::write(&file, input).unwrap();
            assert_eq!(
                machine_file::load(&file).unwrap().images[0].source,
                assembly::ImageSource::Inline(rom.clone())
            );
        }
    }
}

#[test]
fn unsupported_extensions_list_all_accepted_extensions_before_io() {
    for file in ["machine", "machine.json", "machine.YAML"] {
        let error = machine_file::load(file).unwrap_err().to_string();
        assert!(error.starts_with(&format!("{file}:")));
        assert!(error.contains(".toml, .yaml or .yml"));
    }
}

#[cfg(unix)]
#[test]
fn symlinks_select_syntax_by_supplied_extension_and_images_by_target_directory() {
    let tree = TempTree::new();
    let target = tree.0.join("target");
    let aliases = tree.0.join("aliases");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&aliases).unwrap();
    std::fs::write(target.join("bios.bin"), vec![0xF4; 0x8000]).unwrap();
    for (extension, target_name, input) in [
        ("yaml", "yaml-content.toml", MINIMAL),
        ("yml", "yaml-content.toml", MINIMAL),
        (
            "toml",
            "toml-content.yaml",
            "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n",
        ),
    ] {
        std::fs::write(target.join(target_name), input).unwrap();
        let alias = aliases.join(format!("test.{extension}"));
        std::os::unix::fs::symlink(target.join(target_name), &alias).unwrap();
        machine_file::load(&alias).unwrap();
        // A directory alias also preserves a caller's relative diagnostic path.
        let current = std::env::current_dir().unwrap();
        let mut relative = PathBuf::new();
        for _ in current.ancestors().skip(1) {
            relative.push("..");
        }
        relative.push(alias.strip_prefix("/").unwrap());
        for invalid in ["schema: 1\nunknown: true\n", "schema: 1\n", MINIMAL] {
            if extension == "toml" {
                continue;
            }
            std::fs::write(target.join(target_name), invalid).unwrap();
            if invalid != MINIMAL {
                let error = machine_file::load(&relative).unwrap_err().to_string();
                assert!(
                    error.starts_with(&format!("{}:", relative.display())),
                    "{error}"
                );
            }
            if invalid == MINIMAL {
                // The target image exists; force an image failure for this case.
                let mut missing = Reader(|_path: &Path| {
                    Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "missing synthetic image",
                    ))
                });
                let error = machine_file::load_with_reader(&relative, &mut missing)
                    .unwrap_err()
                    .to_string();
                assert!(error.starts_with(&format!("{}:", relative.display())));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn symlink_image_errors_show_spelled_and_resolved_paths() {
    use sha2::{Digest, Sha256};
    let tree = TempTree::new();
    let target = tree.0.join("target");
    let aliases = tree.0.join("aliases");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&aliases).unwrap();
    let file = target.join("test.yaml");
    let alias = aliases.join("test.yml");
    std::fs::write(&file, MINIMAL).unwrap();
    let spelled = aliases.join("bios.bin");
    std::fs::write(&spelled, vec![0; 0x8000]).unwrap();
    std::os::unix::fs::symlink(&file, &alias).unwrap();
    let resolved = target.canonicalize().unwrap().join("bios.bin");
    let description = format!("{} (resolved to {})", spelled.display(), resolved.display());
    let machine_file::LoadError::Lower(error) = machine_file::load(&alias).unwrap_err() else {
        panic!("expected image read error")
    };
    assert_eq!(error.origin.file, alias);
    assert!(
        error
            .message
            .starts_with(&format!("cannot read image {description}:")),
        "{error}"
    );
    let rom = vec![0xF4; 0x8000];
    std::fs::write(&resolved, &rom).unwrap();
    let pin = "0".repeat(64);
    std::fs::write(&file, format!("{MINIMAL}    sha256: '{pin}'\n")).unwrap();
    let machine_file::LoadError::Lower(error) = machine_file::load(&alias).unwrap_err() else {
        panic!("expected image integrity error")
    };
    assert_eq!(error.origin.file, alias);
    assert_eq!(
        error.message,
        format!(
            "sha256 mismatch for {description}: expected {pin}, actual {:x}; pin set at {}:6",
            Sha256::digest(&rom),
            alias.display()
        )
    );
}

#[test]
fn nested_origins_retain_card_drive_rtc_and_option_rom_lines() {
    let input = "schema: 1\nboard:\n  model: ibm-5170\n  rtc:\n    nvram: derived\ncpu: {model: 80286}\nmemory: {conventional_kib: 640}\nfirmware:\n  bios: {image: bios.bin}\n  option_roms:\n    - {image: option.bin, base: 0xC8000}\ncard:\n  - kind: fdc-765\n    drives:\n      - {unit: 1}\n      - {unit: 0}\n";
    let origins = Origins::from_yaml("origin.yaml", input).unwrap();
    for (field, line) in [
        ("board", 3),
        ("board.rtc", 5),
        ("cpu", 6),
        ("memory", 7),
        ("firmware.bios", 9),
        ("firmware.option_roms[0]", 11),
        ("card[0]", 13),
        ("card[0].drives[0]", 15),
        ("card[0].drives[1]", 16),
    ] {
        assert_eq!(origins.get(field).line, Some(line), "{field}");
    }
    let error = run(&format!(
        "{MINIMAL}card:\n  - kind: fdc-765\n    drives:\n      - {{unit: 1, type: 360K}}\n"
    ))
    .unwrap_err();
    assert_eq!(error.field, "card[fdc-765].drives[unit=1].image");
    assert_eq!(error.origin.line, Some(9));
}

#[test]
fn board_mouse_yaml_and_toml_lower_to_identical_hardware() {
    for mouse in [false, true] {
        let yaml = MINIMAL.replace(
            "board: {model: ibm-5170}",
            &format!("board: {{model: ibm-5170, mouse: {mouse}}}"),
        );
        let toml = format!("schema=1\n[board]\nmodel='ibm-5170'\nmouse={mouse}\n[firmware.bios]\nimage='bios.bin'\n");
        let file = Path::new("machines/test.toml");
        let config = lower(
            &parse_toml(file, &toml).unwrap(),
            &Origins::from_toml(file, &toml).unwrap(),
            &mut Reader(|_: &Path| Ok(vec![0xF4; 0x8000])),
        )
        .unwrap();
        assert_eq!(run(&yaml).unwrap(), config);
        assert!(config
            .devices
            .iter()
            .any(|d| matches!(&d.kind, assembly::DeviceKind::Keyboard(cfg) if cfg.mouse == mouse)));
    }
    let config = run(MINIMAL).unwrap();
    assert!(config
        .devices
        .iter()
        .any(|d| matches!(&d.kind, assembly::DeviceKind::Keyboard(cfg) if !cfg.mouse)));
}

#[test]
fn board_mouse_requires_a_boolean_in_both_syntaxes() {
    for value in ["1", "'true'", "yes"] {
        assert!(parse_yaml(
            "machine.yaml",
            &format!("schema: 1\nboard: {{mouse: {value}}}\n")
        )
        .is_err());
    }
    for value in ["1", "'true'", "'yes'"] {
        assert!(parse_toml(
            "machine.toml",
            &format!("schema=1\n[board]\nmouse={value}\n")
        )
        .is_err());
    }
    for value in [true, false] {
        let toml = parse_toml(
            "machine.toml",
            &format!("schema=1\n[board]\nmouse={value}\n"),
        )
        .unwrap();
        let yaml = parse_yaml(
            "machine.yaml",
            &format!("schema: 1\nboard: {{mouse: {value}}}\n"),
        )
        .unwrap();
        assert_eq!(toml.board.unwrap().mouse, Some(value));
        assert_eq!(yaml.board.unwrap().mouse, Some(value));
    }
}

#[test]
fn mouse_irq_conflict_names_the_yaml_mouse_field() {
    let input = MINIMAL.replace(
        "board: {model: ibm-5170}",
        "board: {model: ibm-5170, mouse: true}",
    ) + "card: [{id: serial, kind: uart-16550, irq: 12}]\n";
    let Err(error) = run(&input) else {
        panic!("expected IRQ12 conflict")
    };
    assert_eq!(
        error.validation.as_deref(),
        Some(&assembly::ConfigError::IrqConflict {
            a: assembly::DeviceId::Keyboard,
            b: assembly::DeviceId::Uart,
            line: 12,
        })
    );
    assert_eq!(error.field, "card[serial]");
    assert!(error.message.contains("board.mouse"), "{error}");
    assert!(error.message.contains("test.yaml:2"), "{error}");
}
