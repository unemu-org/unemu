// SPDX-License-Identifier: AGPL-3.0-or-later
//! TOML/YAML primary-display switch parsing and hardware validation.
//! See machine-files.md §Board and machine-config.md §Primary display.

use assembly::{ConfigError, DisplaySwitch, PrimaryDisplay};
use machine_file::{lower, parse_toml, parse_yaml, ImageReader, Origins, YamlOrigins};
use std::{io, path::Path};

struct Images;
impl ImageReader for Images {
    fn read(&mut self, _: &Path) -> io::Result<Vec<u8>> {
        Ok(vec![0xF4; 32768])
    }
}

fn twin(switch: Option<&str>, cards: &[&str]) -> (String, String) {
    let mut toml = "schema=1\n[board]\nmodel='ibm-5170'\n".to_string();
    let mut yaml = "schema: 1\nboard:\n  model: ibm-5170\n".to_string();
    if let Some(switch) = switch {
        toml.push_str(&format!("display_switch='{switch}'\n"));
        yaml.push_str(&format!("  display_switch: {switch}\n"));
    }
    toml.push_str("[firmware.bios]\nimage='bios.bin'\n");
    yaml.push_str("firmware:\n  bios:\n    image: bios.bin\n");
    if !cards.is_empty() {
        yaml.push_str("card:\n");
    }
    for card in cards {
        toml.push_str(&format!("[[card]]\nkind='{card}'\n"));
        yaml.push_str(&format!("  - kind: {card}\n"));
    }
    (toml, yaml)
}

#[test]
fn primary_display_switch_toml_yaml_twins_lower_identically() {
    for (switch, cards, expected, video) in [
        (None, vec!["mda"], PrimaryDisplay::Monochrome, 0x30),
        (None, vec!["vga-text"], PrimaryDisplay::Colour80, 0x20),
        (
            None,
            vec!["mda", "vga-text"],
            PrimaryDisplay::Monochrome,
            0x30,
        ),
        (
            Some("auto"),
            vec!["vga-text", "mda"],
            PrimaryDisplay::Monochrome,
            0x30,
        ),
        (
            Some("mono"),
            vec!["mda", "vga-text"],
            PrimaryDisplay::Monochrome,
            0x30,
        ),
        (
            Some("colour"),
            vec!["mda", "vga-text"],
            PrimaryDisplay::Colour80,
            0x20,
        ),
        (None, vec![], PrimaryDisplay::None, 0),
        (Some("auto"), vec![], PrimaryDisplay::None, 0),
    ] {
        let (toml, yaml) = twin(switch, &cards);
        let t = lower(
            &parse_toml("machine.toml", &toml).unwrap(),
            &Origins::from_toml("machine.toml", &toml).unwrap(),
            &mut Images,
        )
        .unwrap();
        let y = lower(
            &parse_yaml("machine.yaml", &yaml).unwrap(),
            &Origins::from_yaml("machine.yaml", &yaml).unwrap(),
            &mut Images,
        )
        .unwrap();
        assert_eq!(t, y);
        assert_eq!(t.primary_display().unwrap(), expected);
        assert_eq!(
            assembly::derive_cmos_config(&t).unwrap()[0x14] & 0x30,
            video
        );
    }
}

#[test]
fn primary_display_switch_missing_card_errors_point_to_the_switch_value() {
    for (switch, cards, requested) in [
        ("mono", vec!["vga-text"], DisplaySwitch::Mono),
        ("colour", vec!["mda"], DisplaySwitch::Colour),
        ("mono", vec![], DisplaySwitch::Mono),
        ("colour", vec![], DisplaySwitch::Colour),
    ] {
        let (toml, yaml) = twin(Some(switch), &cards);
        let t = lower(
            &parse_toml("machine.toml", &toml).unwrap(),
            &Origins::from_toml("machine.toml", &toml).unwrap(),
            &mut Images,
        )
        .unwrap_err();
        let y = lower(
            &parse_yaml("machine.yaml", &yaml).unwrap(),
            &Origins::from_yaml("machine.yaml", &yaml).unwrap(),
            &mut Images,
        )
        .unwrap_err();
        for error in [t, y] {
            let expected = match requested {
                DisplaySwitch::Mono => "display switch mono requires an installed MDA card",
                DisplaySwitch::Colour => {
                    "display switch colour requires an installed colour card (vga-text / vga)"
                }
                DisplaySwitch::Auto => unreachable!(),
            };
            assert_eq!(error.message, expected);
            assert_eq!(error.field, "board.display_switch");
            assert_eq!(error.origin.line, Some(4));
            assert_eq!(
                error.validation.as_deref(),
                Some(&ConfigError::DisplaySwitchUnavailable { switch: requested })
            );
        }
    }
}

#[test]
fn primary_display_switch_rejects_unknown_names_and_wrong_types_in_both_formats() {
    let (toml, yaml) = twin(Some("auto"), &[]);
    assert!(parse_toml("machine.toml", &toml).is_ok());
    assert!(parse_yaml("machine.yaml", &yaml).is_ok());
    for value in ["'color'", "'mda'", "'MONO'", "true", "42", "[]", "{}"] {
        let toml = format!("schema=1\n[board]\ndisplay_switch={value}\n");
        let yaml = format!("schema: 1\nboard:\n  display_switch: {value}\n");
        for error in [
            parse_toml("machine.toml", &toml).unwrap_err(),
            parse_yaml("machine.yaml", &yaml).unwrap_err(),
        ] {
            let message = error.to_string();
            assert!(message.contains("display_switch"), "{message}");
            if value == "'color'" {
                assert!(message.contains("use `colour`"), "{message}");
            }
        }
    }
}

#[test]
fn kiselev_machine_files_explicitly_select_colour_and_neutral_preset_keeps_auto() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines");
    for name in [
        "at-freedos.toml",
        "at-freedos.yaml",
        "at-freedos-from-preset.toml",
    ] {
        let path = root.join(name);
        let text = std::fs::read_to_string(&path).unwrap();
        let document = if name.ends_with("yaml") {
            parse_yaml(&path, &text).unwrap()
        } else {
            parse_toml(&path, &text).unwrap()
        };
        assert_eq!(
            document.board.unwrap().display_switch,
            Some(machine_file::schema::v1::DisplaySwitch::Colour),
            "{name} must declare the firmware compatibility switch explicitly"
        );
        let (resolved, _) = machine_file::resolve(&path).unwrap();
        assert_eq!(
            resolved.board.unwrap().display_switch,
            Some(machine_file::schema::v1::DisplaySwitch::Colour)
        );
    }
    let neutral = root.join("presets/ibm-5170.toml");
    let text = std::fs::read_to_string(&neutral).unwrap();
    assert_eq!(
        parse_toml(&neutral, &text)
            .unwrap()
            .board
            .unwrap()
            .display_switch,
        None
    );
}

/// The open AT BIOS reads the switch with the IBM polarity, so its machine
/// files name the console they want: MDA on the inherited dual-card board,
/// the only (VGA) card on the game profile.
#[test]
fn open_at_bios_machine_files_select_their_console_with_ibm_polarity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines");
    for (name, switch) in [
        ("at-ide.toml", machine_file::schema::v1::DisplaySwitch::Mono),
        (
            "at-games-open.toml",
            machine_file::schema::v1::DisplaySwitch::Colour,
        ),
    ] {
        let (resolved, _) = machine_file::resolve(root.join(name)).unwrap();
        assert!(
            resolved
                .firmware
                .unwrap()
                .bios
                .unwrap()
                .image
                .unwrap()
                .ends_with("vendor/unemu-at-bios/bios-unemu-at.bin"),
            "{name}"
        );
        assert_eq!(
            resolved.board.unwrap().display_switch,
            Some(switch),
            "{name}"
        );
    }
}

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

#[test]
fn kiselev_machine_file_assembly_reads_display_switch_from_input_port() {
    for fixture in ["kiselev-bios/bios-xi8088.bin", "freedos/freedos.img"] {
        if fixtures::fixture(fixture).is_none() {
            return;
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml");
    let loaded =
        machine_file::load_for_assembly(path, assembly::AssemblyOptions::default()).unwrap();
    assert_eq!(loaded.config.display_switch, DisplaySwitch::Colour);
    let mut assembled = loaded
        .assemble(&assembly::ObservationPolicy::default())
        .unwrap();
    assembled
        .machine
        .bus_mut()
        .port_write(0x64, 1, 0xC0)
        .unwrap();
    assert_eq!(
        assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
        0xA0
    );
}
