// SPDX-License-Identifier: AGPL-3.0-or-later
//! Source-located adapter ROM diagnostics; firmware design §6, rules 11–14.

use assembly::ConfigError;
use machine_file::{lower, parse_toml, parse_yaml, ImageReader, Origins, YamlOrigins};
use sha2::{Digest, Sha256};
use std::{io, path::Path};

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

fn rom() -> Vec<u8> {
    let mut bytes = vec![0; 2048];
    bytes[..4].copy_from_slice(&[0x55, 0xAA, 4, 0xCB]);
    bytes[2047] = 0x32;
    bytes
}

struct Images(Vec<u8>);
impl ImageReader for Images {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        Ok(if path.ends_with("bios.bin") {
            vec![0; 32768]
        } else {
            self.0.clone()
        })
    }
}

fn load(
    bytes: Vec<u8>,
    base: u32,
    yaml: bool,
    tail: &str,
) -> Result<assembly::MachineConfig, machine_file::LowerError> {
    let input = if yaml {
        format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios: {{image: bios.bin}}\n  option_roms:\n    - image: option.bin\n      base: {base}\n{tail}")
    } else {
        format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[firmware.option_roms]]\nimage='option.bin'\nbase={base}\n{tail}")
    };
    let file = Path::new(if yaml { "roms.yaml" } else { "roms.toml" });
    let (document, origins) = if yaml {
        (
            parse_yaml(file, &input).unwrap(),
            Origins::from_yaml(file, &input).unwrap(),
        )
    } else {
        (
            parse_toml(file, &input).unwrap(),
            Origins::from_toml(file, &input).unwrap(),
        )
    };
    lower(&document, &origins, &mut Images(bytes))
}

#[test]
fn option_rom_format_errors_retain_the_file_line_and_exact_assembly_message() {
    for yaml in [false, true] {
        for (bytes, base, message, field) in [
            (rom(), 0xC8001, "option ROM at 0xC8001 must be 2 KiB aligned", "firmware.option_roms[0].base"),
            (rom(), 0xC7800, "system BIOS unlisted has no known video-ROM scan at C0000h-C7FFFh", "firmware.option_roms[0]"),
            (vec![0; 2048], 0xC8000, "option ROM at 0xC8000: expected 55AAh signature and a length byte", "firmware.option_roms[0]"),
            ({ let mut bytes = rom(); bytes[2] = 3; bytes }, 0xC8000, "option ROM at 0xC8000: length byte declares 1536 bytes, image has 2048 (must match and be nonzero)", "firmware.option_roms[0]"),
            ({ let mut bytes = rom(); bytes[100] = 0xB0; bytes }, 0xC8000, "option ROM at 0xC8000: byte sum is 0xB0, must be 0 (pad and checksum the image)", "firmware.option_roms[0]"),
        ] {
            let error = load(bytes, base, yaml, "").unwrap_err();
            assert_eq!(error.message, message);
            assert_eq!(error.field, field);
            assert_eq!(error.origin.line, Some(6));
            assert_eq!(error.origin.file, Path::new(if yaml { "roms.yaml" } else { "roms.toml" }));
            assert_eq!(error.validation.as_ref().unwrap().to_string(), message);
        }
    }
}

#[test]
fn unlisted_option_rom_has_its_own_origin_and_known_build_list() {
    for yaml in [false, true] {
        let error = load(rom(), 0xC8000, yaml, "").unwrap_err();
        let hash = format!("{:x}", Sha256::digest(rom()));
        assert_eq!(error.message, format!("option ROM image (sha256 {hash}) is not a known image for board \"ibm-5170\"; known: ibm-vga-bios-1987-32k (user-supplied), vgabios-0.9d-unemu.1-std (open, vendor/vgabios/fetch.sh), vgabios-0.8a-unemu.1-std (open, vendor/vgabios/fetch.sh at unemupc 4f73f3d06e120b42a93bbbc0f857f3fb9a1cb0ea), xub-r638-at (open, vendor/xtide-universal-bios/fetch.sh), xub-r638-xtp-ata1f0 (open, vendor/xtide-universal-bios/fetch.sh)"));
        assert_eq!(error.field, "firmware.option_roms[0]");
        assert_eq!(error.origin.line, Some(6));
        assert!(matches!(
            error.validation.as_deref(),
            Some(ConfigError::OptionRomNotListed { image_index: 1, .. })
        ));
    }
}

#[test]
fn listed_xub_errors_and_catalogue_services_survive_lowering() {
    let Some(bytes) = fixtures::fixture("xtide-universal-bios/ide_at-r638-8k.bin") else {
        return;
    };
    for yaml in [false, true] {
        let error = load(bytes.clone(), 0xC8000, yaml, "").unwrap_err();
        assert_eq!(
            error.message,
            "option ROM \"xub-r638-at\" drives ATA at 0x1F0/IRQ14, but no ide card decodes 0x1F0"
        );
        assert_eq!(error.field, "firmware.option_roms[0]");
        assert_eq!(error.origin.line, Some(6));
        let tail = if yaml {
            "card: [{kind: ide}]\n"
        } else {
            "[[card]]\nkind='ide'\n"
        };
        let config = load(bytes.clone(), 0xC8000, yaml, tail).unwrap();
        assert_eq!(
            config.images[1].provides,
            vec![assembly::FirmwareService::FixedDisk]
        );
        assert!(config
            .memory
            .ram
            .iter()
            .all(|ram| ram.end() <= 0xC0000 || ram.base >= 0xF8000));
        let tail = if yaml {
            "      provides: [floppy]\ncard: [{kind: ide}]\n"
        } else {
            "provides=['floppy']\n[[card]]\nkind='ide'\n"
        };
        let error = load(bytes.clone(), 0xC8000, yaml, tail).unwrap_err();
        assert_eq!(error.field, "firmware.option_roms[0].provides");
        assert_eq!(error.origin.line, Some(6));
        assert!(matches!(
            error.validation.as_deref(),
            Some(ConfigError::FirmwareProvidesMismatch { image_index: 1, .. })
        ));
    }
}

#[test]
fn misplaced_declared_bios_does_not_name_the_higher_option_rom() {
    let input = "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nbase=0xC0000\n[[firmware.option_roms]]\nimage='option.bin'\nbase=0xD0000\n";
    let file = Path::new("misplaced.toml");
    let error = lower(
        &parse_toml(file, input).unwrap(),
        &Origins::from_toml(file, input).unwrap(),
        &mut Images(rom()),
    )
    .unwrap_err();
    assert_eq!(
        error.validation.as_deref(),
        Some(&ConfigError::BiosPlacement {
            base: 0xC0000,
            size: 32768
        })
    );
    assert_eq!(error.message, "system BIOS of 32 KiB at 0xc0000 must have a positive size that is a multiple of 8 KiB and end at 0x100000");
    assert_eq!(error.field, "firmware.bios.image");
    assert_eq!(error.origin.file, file);
    assert_eq!(error.origin.line, Some(4));
}

#[test]
fn machine_file_escape_retains_the_junk_rom_origin_and_reaches_assembly() {
    struct File(std::path::PathBuf);
    impl Drop for File {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let file =
        File(std::env::temp_dir().join(format!("option-rom-escape-{}.toml", std::process::id())));
    let input = "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[firmware.option_roms]]\nimage='option.bin'\nbase=0xC8000\nprovides=['fixed-disk']\n[[card]]\nkind='ide'\n";
    std::fs::write(&file.0, input).unwrap();
    let machine_file::LoadError::Lower(error) =
        machine_file::load_with_reader(&file.0, &mut Images(rom())).unwrap_err()
    else {
        panic!("expected source-located catalogue error")
    };
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::OptionRomNotListed { image_index: 1, .. })
    ));
    assert_eq!(error.field, "firmware.option_roms[0]");
    assert_eq!(error.origin.file, file.0);
    assert_eq!(error.origin.line, Some(6));
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    let config =
        machine_file::load_with_reader_and_options(&file.0, &mut Images(rom()), options).unwrap();
    let assembled = assembly::assemble_with_options(&config, options).unwrap();
    assert_eq!(
        assembled.option_roms[0].provides,
        [assembly::FirmwareService::FixedDisk]
    );
    assert_eq!(assembled.unlisted_firmware.len(), 2);
}

#[test]
fn machine_file_retains_the_declared_system_bios_index() {
    let input = "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n";
    let file = Path::new("declared-bios.toml");
    let config = lower(
        &parse_toml(file, input).unwrap(),
        &Origins::from_toml(file, input).unwrap(),
        &mut Images(rom()),
    )
    .unwrap();
    assert_eq!(config.system_bios_image, Some(0));
    assert_eq!(config.images[0].base, 0xF8000);
}

#[test]
fn video_container_toml_yaml_hash_full_file_and_map_only_suffix() {
    let mut bytes = vec![0; 8192];
    let mut video = vec![0; 24576];
    video[..4].copy_from_slice(&[0x55, 0xAA, 0x30, 0xCB]);
    video[24575] = 0u8.wrapping_sub(video.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
    bytes.extend(video);
    let pin = format!("{:x}", Sha256::digest(&bytes));
    for yaml in [false, true] {
        let text = if yaml {
            format!("schema: 1\nboard: {{model: ibm-5170}}\nfirmware:\n  bios: {{image: bios.bin}}\n  option_roms:\n    - {{image: video.bin, base: 0xC8000, offset: 8192, sha256: '{pin}'}}\ncard: [{{kind: vga}}]\n")
        } else {
            format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[firmware.option_roms]]\nimage='video.bin'\nbase=0xC8000\noffset=8192\nsha256='{pin}'\n[[card]]\nkind='vga'\n")
        };
        let path = Path::new(if yaml { "video.yaml" } else { "video.toml" });
        let document = if yaml {
            parse_yaml(path, &text).unwrap()
        } else {
            parse_toml(path, &text).unwrap()
        };
        let origins = if yaml {
            Origins::from_yaml(path, &text).unwrap()
        } else {
            Origins::from_toml(path, &text).unwrap()
        };
        let options = assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        };
        let config = machine_file::lower_with_options(
            &document,
            &origins,
            &mut Images(bytes.clone()),
            options,
        )
        .unwrap();
        assert_eq!(config.images[1].source.known_len(), Some(24576));
        assert!(matches!(
            config.images[1].source,
            assembly::ImageSource::RomContainer { offset: 8192, .. }
        ));
        let regions = assembly::test_support::cpu_mapping_regions(&config, options).unwrap();
        assert!(regions.contains(&(0xC8000, 0xCE000, "rom")));
        let mut corrupt = bytes.clone();
        corrupt[0] = 1;
        let error =
            machine_file::lower_with_options(&document, &origins, &mut Images(corrupt), options)
                .unwrap_err();
        assert!(
            error.message.contains("sha256 mismatch"),
            "unmapped prefix is still pinned"
        );
    }
}

#[test]
fn video_container_offsets_reject_empty_suffix_and_system_bios_slicing() {
    for (field, text, expected) in [
        (
            "firmware.option_roms[0].offset",
            "[[firmware.option_roms]]\nimage='video.bin'\nbase=0xC0000\noffset=2048\n",
            "offset must leave a nonempty ROM",
        ),
        (
            "firmware.bios.offset",
            "offset=0\n",
            "offset is only supported by option ROM images",
        ),
    ] {
        let input = format!(
            "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n{text}"
        );
        let path = Path::new("video.toml");
        let error = machine_file::lower_with_options(
            &parse_toml(path, &input).unwrap(),
            &Origins::from_toml(path, &input).unwrap(),
            &mut Images(rom()),
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap_err();
        assert_eq!(error.field, field);
        assert_eq!(error.message, expected);
    }
}
