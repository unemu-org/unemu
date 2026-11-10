// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM Personal Computer AT Technical Reference, Real-Time Clock/CMOS RAM
//! Information: provider selection must preserve existing NVRAM seeds.

use assembly::*;
use device::Device;
use devices::{CmosConfig, CmosRtc};

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

#[test]
fn provider_cmos_existing_presets_keep_every_seed_byte() {
    for (name, config, floppy_types, equipment, base_memory_kb) in [
        ("boot", at_boot(vec![0xF4]), 0, 0, 64),
        ("keyboard", at_keyboard(vec![0xF4]), 0, 0, 64),
        ("rtc", at_rtc(vec![0xF4], 100, 10), 0, 0, 64),
        (
            "fdc-write",
            at_fdc_write(vec![0xF4], vec![0; 512], 0x8000),
            0x10,
            1,
            64,
        ),
        ("vga", at_vga_text(vec![0xF4]), 0, 0, 640),
        ("test386", test386(vec![0; 65536]), 0, 0, 640),
        (
            "floppy-boot",
            floppy_boot(vec![0xF4], vec![0; 1_474_560]),
            0x40,
            1,
            64,
        ),
        ("bios-no-media", bios(vec![0; 32768], None), 0x40, 1, 640),
        (
            "bios-freedos",
            bios(vec![0; 32768], Some(vec![0; 1_474_560])),
            0x40,
            1,
            640,
        ),
    ] {
        let legacy = CmosRtc::new().with_config(CmosConfig {
            floppy_types,
            hard_disk_types: 0,
            equipment,
            base_memory_kb,
            extended_memory_kb: 0,
        });
        let current = CmosRtc::new().with_nvram(derive_cmos_config(&config).unwrap());
        assert_eq!(
            current.save(),
            legacy.save(),
            "{name}: all 64 backing bytes and live RTC state"
        );
    }
}

#[test]
fn provider_cmos_xub_disk_keeps_system_bios_types_zero() {
    let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let Some(xub) = fixtures::fixture("xtide-universal-bios/ide_at-r638-8k.bin") else {
        return;
    };
    let mut config = bios(rom, Some(vec![0; 1_474_560]));
    let baseline = derive_cmos_config(&config).unwrap();
    let ide = IdeCfg {
        base: 0x1F0,
        control: 0x3F6,
    };
    config.devices.push(
        DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(ide), ide.port_windows()).with_irq(14),
    );
    config
        .devices
        .iter_mut()
        .find(|device| device.id == DeviceId::Fdc)
        .unwrap()
        .ports = FdcCfg::port_windows(true);
    let geometry = HardDiskGeometry {
        cylinders: 615,
        heads: 4,
        sectors_per_track: 17,
        bytes_per_sector: 512,
    };
    config.drives.push(DriveConfig {
        kind: DriveKind::HardDisk { geometry },
        image: ImageSource::Blank {
            size: geometry.image_bytes(),
        },
        expected_sha256: None,
        controller: DeviceId::Ide,
        unit: 0,
        write: WriteMode::Volatile,
    });
    config.images.push(LoadImage {
        base: 0xC8000,
        source: ImageSource::Inline(xub),
        kind: ImageKind::Rom,
        provides: vec![],
    });
    assert_eq!(derive_cmos_config(&config).unwrap(), baseline);
    let mut assembled = assemble(&config).unwrap();
    assert_eq!(
        assembled.option_roms[0].provides,
        [FirmwareService::FixedDisk]
    );
    for register in [0x12, 0x19, 0x1A] {
        let bus = assembled.machine.bus_mut();
        bus.port_write(0x70, 1, register).unwrap();
        assert_eq!(bus.port_read(0x71, 1).unwrap(), 0);
    }
}

#[test]
fn provider_cmos_catalogue_capability_is_explicit() {
    assert_eq!(
        FirmwareCapability::CmosDriveTypes.to_string(),
        "cmos-drive-types"
    );
    assert!(KISELEV_XI8088.capabilities.is_empty());
    assert!(FirmwareEntry::XUB_R638_AT.capabilities.is_empty());
}
