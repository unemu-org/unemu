// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM Personal Computer AT Technical Reference, System Board, Keyboard
//! Controller, Input-Port Bit Definitions; Real-Time Clock/CMOS RAM Information,
//! Equipment Byte (Hex 14). See machine-config.md §Primary display.

use assembly::*;

fn config(mono: bool, colour: bool, video_rom: bool, switch: DisplaySwitch) -> MachineConfig {
    let mut config = bios(vec![0xF4; 32768], None);
    config.display_switch = switch;
    config.devices.retain(|device| match device.kind {
        DeviceKind::Mda(_) => mono,
        DeviceKind::Vga(_) => colour,
        _ => true,
    });
    config.memory.mmio.retain(|window| match window.device {
        DeviceId::Mda => mono,
        DeviceId::Vga => colour,
        _ => true,
    });
    if video_rom {
        let mut bytes = vec![0; 2048];
        bytes[..3].copy_from_slice(&[0x55, 0xAA, 4]);
        bytes[3] = 0xFD;
        config.images.push(LoadImage {
            base: 0xC0000,
            source: ImageSource::Inline(bytes),
            kind: ImageKind::Rom,
            provides: vec![],
        });
    }
    config
}

fn checksum(seed: &[u8]) {
    let sum: u16 = seed[0x10..=0x2D].iter().map(|&byte| u16::from(byte)).sum();
    assert_eq!(u16::from_be_bytes([seed[0x2E], seed[0x2F]]), sum);
}

fn assert_display(config: &MachineConfig, primary: PrimaryDisplay, straps: u32, video: u8) {
    assert_eq!(config.primary_display().unwrap(), primary);
    let derived = derive_cmos_config(config).unwrap();
    assert_eq!(derived[0x14], 1 | video, "floppy inventory is preserved");
    checksum(&derived);
    let extended = derive_cmos_layout(config).unwrap();
    assert_eq!(&extended[..64], derived);
    let mut assembled = assemble_with_options(
        config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let bus = assembled.machine.bus_mut();
    bus.port_write(0x64, 1, 0xC0).unwrap();
    assert_eq!(bus.port_read(0x60, 1).unwrap(), straps);
    bus.port_write(0x64, 1, 0xD0).unwrap();
    assert_eq!(bus.port_read(0x60, 1).unwrap(), 3);
    let actual: Vec<u8> = (0x10..=0x2F)
        .map(|index| {
            bus.port_write(0x70, 1, index).unwrap();
            bus.port_read(0x71, 1).unwrap() as u8
        })
        .collect();
    assert_eq!(actual, derived[0x10..=0x2F]);
}

#[test]
fn primary_display_mda_only() {
    assert_display(
        &config(true, false, false, DisplaySwitch::Auto),
        PrimaryDisplay::Monochrome,
        0xE0,
        0x30,
    );
}

#[test]
fn primary_display_colour_text_only() {
    assert_display(
        &config(false, true, false, DisplaySwitch::Auto),
        PrimaryDisplay::Colour80,
        0xA0,
        0x20,
    );
}

#[test]
fn primary_display_video_rom_only() {
    let mut config = config(false, true, true, DisplaySwitch::Auto);
    let vga = config
        .devices
        .iter_mut()
        .find(|device| device.id == DeviceId::Vga)
        .unwrap();
    vga.kind = DeviceKind::VgaRegisters(VgaCfg {});
    vga.ports = VgaCfg::port_windows();
    let aperture = config
        .memory
        .mmio
        .iter_mut()
        .find(|window| window.device == DeviceId::Vga)
        .unwrap();
    aperture.base = 0xA0000;
    aperture.size = 0x20000;
    config.memory.ram = vec![RamRegion::new(0, 0xA0000)];
    assert_display(&config, PrimaryDisplay::OwnBios, 0xA0, 0);
    config.images.pop();
    assert_display(&config, PrimaryDisplay::Colour80, 0xA0, 0x20);
}

#[test]
fn primary_display_dual_cards_default_to_mono_in_either_order() {
    let mut config = config(true, true, false, DisplaySwitch::Auto);
    assert_display(&config, PrimaryDisplay::Monochrome, 0xE0, 0x30);
    let mda = config
        .devices
        .iter()
        .position(|d| d.id == DeviceId::Mda)
        .unwrap();
    let colour = config
        .devices
        .iter()
        .position(|d| d.id == DeviceId::Vga)
        .unwrap();
    config.devices.swap(mda, colour);
    assert_display(&config, PrimaryDisplay::Monochrome, 0xE0, 0x30);
    config.drives.clear();
    let seed = derive_cmos_config(&config).unwrap();
    assert_eq!(seed[0x14], 0x30);
    checksum(&seed);
}

#[test]
fn primary_display_dual_cards_follow_the_switch() {
    let mut config = config(true, true, false, DisplaySwitch::Colour);
    assert_display(&config, PrimaryDisplay::Colour80, 0xA0, 0x20);
    config.display_switch = DisplaySwitch::Mono;
    assert_display(&config, PrimaryDisplay::Monochrome, 0xE0, 0x30);
}

#[test]
fn primary_display_without_cards_keeps_unassigned_video_code() {
    assert_display(
        &config(false, false, false, DisplaySwitch::Auto),
        PrimaryDisplay::None,
        0xA0,
        0,
    );
}

#[test]
fn primary_display_ignores_non_video_option_roms() {
    let mut config = config(false, true, true, DisplaySwitch::Auto);
    config.images.last_mut().unwrap().base = 0xC8000;
    assert_eq!(config.primary_display().unwrap(), PrimaryDisplay::Colour80);
    assert_eq!(derive_cmos_config(&config).unwrap()[0x14] & 0x30, 0x20);
}

#[test]
fn primary_display_explicit_nvram_is_not_rewritten() {
    let mut config = config(true, false, false, DisplaySwitch::Auto);
    let mut seed = [0; 64];
    for (index, byte) in seed.iter_mut().enumerate().skip(0x0E) {
        *byte = index as u8;
    }
    let rtc = config
        .devices
        .iter_mut()
        .find(|device| device.id == DeviceId::Rtc)
        .unwrap();
    let DeviceKind::Rtc(rtc) = &mut rtc.kind else {
        unreachable!()
    };
    rtc.nvram = NvramSource::Image(seed);
    let mut assembled = assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let bus = assembled.machine.bus_mut();
    bus.port_write(0x64, 1, 0xC0).unwrap();
    assert_eq!(bus.port_read(0x60, 1).unwrap(), 0xE0);
    for (index, expected) in seed.iter().enumerate().skip(0x0E) {
        bus.port_write(0x70, 1, index as u32).unwrap();
        assert_eq!(bus.port_read(0x71, 1).unwrap() as u8, *expected);
    }
}

#[test]
fn primary_display_switch_rejects_missing_adapter_classes() {
    for (mono, colour, switch) in [
        (false, true, DisplaySwitch::Mono),
        (true, false, DisplaySwitch::Colour),
        (false, false, DisplaySwitch::Mono),
        (false, false, DisplaySwitch::Colour),
    ] {
        assert_eq!(
            config(mono, colour, false, switch).validate(),
            Err(ConfigError::DisplaySwitchUnavailable { switch })
        );
    }
}

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

#[test]
fn kiselev_presets_explicitly_select_colour_with_a0_input_and_colour_cmos() {
    for image in [
        "kiselev-bios/bios-xi8088.bin",
        "kiselev-bios/bios-xi8088-atreset.bin",
    ] {
        let Some(rom) = fixtures::fixture(image) else {
            return;
        };
        for floppy in [None, Some(vec![0; 1_474_560])] {
            let config = bios(rom.clone(), floppy);
            assert_eq!(config.display_switch, DisplaySwitch::Colour, "{image}");
            assert_display(&config, PrimaryDisplay::Colour80, 0xA0, 0x20);
        }
    }
    for floppy in [None, Some(vec![0; 1_474_560])] {
        assert_eq!(
            bios(vec![0xF4; 32768], floppy).display_switch,
            DisplaySwitch::Auto,
            "other firmware retains automatic board selection"
        );
    }
}

#[test]
fn primary_display_dual_cards_with_video_rom_select_own_bios() {
    let mut config = config(true, true, true, DisplaySwitch::Colour);
    config.memory.ram.retain(|region| region.base < 0xC0000);
    assert_display(&config, PrimaryDisplay::OwnBios, 0xA0, 0);
    config.display_switch = DisplaySwitch::Mono;
    assert_display(&config, PrimaryDisplay::Monochrome, 0xE0, 0x30);
}
