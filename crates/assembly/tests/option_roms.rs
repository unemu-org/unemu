// SPDX-License-Identifier: AGPL-3.0-or-later
//! Adapter ROM placement and execution: IBM Personal Computer AT Technical
//! Reference, System BIOS, "Adapters with System-Accessible ROM Modules".
//! See `backlog/docs/design/firmware-and-machine-classes.md` §6.

use assembly::*;
use cpu::Cpu;
use device::ByteStreamKind;

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

fn rom() -> Vec<u8> {
    let mut bytes = vec![0; 2048];
    bytes[..3].copy_from_slice(&[0x55, 0xAA, 4]);
    bytes[3] = 0xCB;
    checksum(&mut bytes);
    bytes
}

fn checksum(bytes: &mut [u8]) {
    let last = bytes.len() - 1;
    bytes[last] = 0;
    bytes[last] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
}

fn config() -> MachineConfig {
    let mut config = bios(vec![0; 32768], Some(vec![0; 1_474_560]));
    add(&mut config, 0xC8000, rom());
    config
}

fn add(config: &mut MachineConfig, base: u64, bytes: Vec<u8>) {
    config.images.push(LoadImage {
        base,
        source: ImageSource::Inline(bytes),
        kind: ImageKind::Rom,
        provides: vec![],
    });
}

fn error(config: &MachineConfig, allow: bool) -> ConfigError {
    match assemble_with_options(
        config,
        AssemblyOptions {
            allow_unlisted_firmware: allow,
        },
    ) {
        Err(AssemblyError::Config(error)) => error,
        _ => panic!("invalid adapter firmware must fail before execution"),
    }
}

#[test]
fn option_rom_alignment_and_scan_window_exact_messages() {
    let mut config = config();
    config.images[1].base = 0xC8001;
    assert_eq!(
        error(&config, true).to_string(),
        "option ROM at 0xC8001 must be 2 KiB aligned"
    );
    for base in [0xC0000, 0xC7800, 0xF8000] {
        config.images[1].base = base;
        // Avoid overlap with the system BIOS in the end-boundary case.
        if base == 0xF8000 {
            config.images[0].base = 0xFA000;
            config.images[0].source = ImageSource::Inline(vec![0; 24576]);
        }
        assert_eq!(error(&config, true).to_string(), format!("option ROM at {base:#X} is outside the scan window 0xC8000..0xF8000 of \"kiselev-xi8088-1.0.2\""));
    }
    let mut config = self::config();
    config.images[1].base = 0xF7800;
    config.images[1].source = ImageSource::Inline(vec![0; 4096]);
    // Move the BIOS so image overlap does not mask the window-end check.
    config.images[0].base = 0xFA000;
    config.images[0].source = ImageSource::Inline(vec![0; 24576]);
    assert!(matches!(
        error(&config, true),
        ConfigError::OptionRomOutsideScanWindow { .. }
    ));
}

#[test]
fn option_rom_signature_length_and_checksum_exact_messages() {
    for bytes in [vec![], vec![0x55, 0xAA], vec![0; 2048]] {
        let mut config = config();
        config.images[1].source = ImageSource::Inline(bytes);
        assert_eq!(
            error(&config, true).to_string(),
            "option ROM at 0xC8000: expected 55AAh signature and a length byte"
        );
    }
    let mut blank = config();
    blank.images[1].source = ImageSource::Blank { size: 2048 };
    assert_eq!(
        error(&blank, true).to_string(),
        "option ROM at 0xC8000: expected 55AAh signature and a length byte"
    );
    for (length, size) in [(0, 2048), (3, 2048), (5, 2048), (4, 4096)] {
        let mut config = config();
        let mut bytes = vec![0; size];
        bytes[..3].copy_from_slice(&[0x55, 0xAA, length]);
        checksum(&mut bytes);
        config.images[1].source = ImageSource::Inline(bytes);
        assert_eq!(error(&config, true).to_string(), format!("option ROM at 0xC8000: length byte declares {} bytes, image has {size} (must match and be nonzero)", u64::from(length)*512));
    }
    let mut config = config();
    let ImageSource::Inline(bytes) = &mut config.images[1].source else {
        unreachable!()
    };
    bytes[100] = 0xB0;
    assert_eq!(
        error(&config, true).to_string(),
        "option ROM at 0xC8000: byte sum is 0xB0, must be 0 (pad and checksum the image)"
    );
}

#[test]
fn option_rom_overlap_is_independent_of_image_order() {
    for base in [0xC8000, 0xC8400, 0xF8000] {
        let mut config = config();
        add(&mut config, base, rom());
        let (a, b) = if base == 0xF8000 { (0, 2) } else { (1, 2) };
        assert_eq!(
            error(&config, true).to_string(),
            format!("images {a} and {b} overlap at {base:#x}")
        );
    }
}

#[test]
fn option_roms_are_ordered_by_address_and_do_not_displace_the_bios() {
    let mut config = config();
    add(&mut config, 0xD0000, rom());
    add(&mut config, 0xF7800, rom());
    config.images.reverse();
    for _ in 0..config.images.len() {
        let assembled = assemble_with_options(
            &config,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assert_eq!(
            assembled
                .option_roms
                .iter()
                .map(|image| image.base)
                .collect::<Vec<_>>(),
            [0xC8000, 0xD0000, 0xF7800]
        );
        assert_eq!(assembled.unlisted_firmware.len(), 4);
        config.images.rotate_left(1);
    }
}

#[test]
fn adapter_space_has_no_ram_and_ignores_guest_writes() {
    let mut config = config();
    assert!(config
        .memory
        .ram
        .iter()
        .all(|ram| ram.end() <= 0xC0000 || ram.base >= 0xF8000));
    let ImageSource::Inline(bytes) = &mut config.images[0].source else {
        unreachable!()
    };
    // DS=C800; write into both a populated ROM and the empty scan space.
    let code = [
        0xFA, 0xB8, 0x00, 0xC8, 0x8E, 0xD8, 0xC6, 0x06, 0, 0, 0xFF, 0xC6, 0x06, 0, 0x10, 0xFF, 0xF4,
    ];
    // Far jump from the 16-byte reset stub to room for the whole program.
    bytes[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
    bytes[..code.len()].copy_from_slice(&code);
    let mut assembled = assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert_eq!(
        assembled.machine.run_bounded(Some(100)).unwrap(),
        machine::RunOutcome::Halted
    );
    for (address, expected) in [(0xC8000, 0x55), (0xC9000, 0)] {
        let mut byte = [0];
        assembled
            .machine
            .cpu()
            .read_mem(address, &mut byte)
            .unwrap();
        assert_eq!(byte, [expected]);
    }
}

#[test]
fn adapter_ram_and_ram_load_cannot_bypass_the_catalogue() {
    for base in [0xC0000, 0xC8000, 0xF7800] {
        let mut config = config();
        config.memory.ram.push(RamRegion::new(base, 2048));
        assert_eq!(
            error(&config, true).to_string(),
            format!("board \"ibm-5170\" adapter ROM space overlaps ram at {base:#x}")
        );
    }
    let mut config = config();
    config.images[1].kind = ImageKind::RamLoad;
    assert_eq!(
        error(&config, true).to_string(),
        "ram-load image 1 covers unmapped ram at 0xc8000"
    );
}

fn ide(config: &mut MachineConfig) {
    let cfg = IdeCfg {
        base: 0x1F0,
        control: 0x3F6,
    };
    config.devices.push(
        DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(cfg), cfg.port_windows()).with_irq(14),
    );
    config
        .devices
        .iter_mut()
        .find(|device| device.id == DeviceId::Fdc)
        .unwrap()
        .ports = FdcCfg::port_windows(true);
}

#[test]
fn unlisted_junk_rom_cannot_grant_a_strict_fixed_disk_provider() {
    let Some(bios) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let mut config = self::config();
    config.images[0].source = ImageSource::Inline(bios);
    ide(&mut config);
    config.images[1].provides = vec![FirmwareService::FixedDisk];
    assert!(matches!(
        error(&config, false),
        ConfigError::OptionRomNotListed { image_index: 1, .. }
    ));
    let assembled = assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert_eq!(assembled.unlisted_firmware.len(), 1);
    assert_eq!(
        assembled.option_roms[0].provides,
        [FirmwareService::FixedDisk]
    );
}

#[test]
fn listed_xub_requires_its_card_and_owns_its_services() {
    let Some(xub) = fixtures::fixture("xtide-universal-bios/ide_at-r638-8k.bin") else {
        return;
    };
    let Some(bios) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let mut config = self::config();
    config.images[0].source = ImageSource::Inline(bios);
    config.images[1].source = ImageSource::Inline(xub);
    assert_eq!(
        error(&config, false).to_string(),
        "option ROM \"xub-r638-at\" drives ATA at 0x1F0/IRQ14, but no ide card decodes 0x1F0"
    );
    ide(&mut config);
    let assembled = assemble(&config).unwrap();
    assert!(assembled.unlisted_firmware.is_empty());
    assert_eq!(
        assembled.option_roms[0].provides,
        [FirmwareService::FixedDisk]
    );
    for allow in [false, true] {
        config.images[1].provides = vec![FirmwareService::Floppy];
        assert_eq!(error(&config, allow).to_string(), "firmware \"xub-r638-at\" declares provides [\"floppy\"]; catalogue provides [\"fixed-disk\"]");
    }
}

#[test]
#[ignore = "requires fetched Kiselev BIOS and completes POST"]
fn kiselev_post_calls_a_synthetic_option_rom() {
    let Some(bios) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let mut config = assembly::bios(bios, Some(vec![0; 1_474_560]));
    let mut bytes = rom();
    // Preserve DS; write a marker at 0000:0500 and emit E7h, then return far.
    let code = [
        0x1E, 0x31, 0xC0, 0x8E, 0xD8, 0xC6, 0x06, 0, 5, 0xA7, 0xB0, 0xE7, 0xE6, 0x80, 0x1F, 0xCB,
    ];
    bytes[3..3 + code.len()].copy_from_slice(&code);
    checksum(&mut bytes);
    add(&mut config, 0xC8000, bytes);
    // Only this inline test ROM is unlisted; the production BIOS remains pinned.
    let policy = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Post,
            output: OutputKind::Bytes(ByteStreamKind::PostCheckpoints),
            byte_limit: 256,
        }],
        ..ObservationPolicy::default()
    };
    let mut assembled = assemble_with_observation_and_options(
        &config,
        &policy,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert_eq!(assembled.unlisted_firmware.len(), 1);
    let mut post = Vec::new();
    for cap in (100_000..=30_000_000).step_by(100_000) {
        assembled.machine.run_bounded(Some(cap)).unwrap();
        post.extend(
            assembled
                .observation
                .drain_bytes(DeviceId::Post, ByteStreamKind::PostCheckpoints)
                .unwrap(),
        );
        if post.contains(&0x43) {
            break;
        }
    }
    assert!(
        post.windows(5)
            .any(|codes| codes == [0x40, 0x41, 0xE7, 0x42, 0x43]),
        "POST: {post:02X?}"
    );
    let mut marker = [0];
    assembled
        .machine
        .cpu()
        .read_mem(0x500, &mut marker)
        .unwrap();
    assert_eq!(marker, [0xA7]);
}

#[test]
fn adapter_mmio_hole_routes_guest_reads_and_writes() {
    use device::{Device, DeviceError, VClock};
    use std::{cell::RefCell, rc::Rc};
    struct AdapterRam {
        byte: u8,
        accesses: Rc<RefCell<Vec<(&'static str, u64)>>>,
    }
    impl Device for AdapterRam {
        fn name(&self) -> &'static str {
            "adapter-ram-test"
        }
        fn port_read(&mut self, _: u16, _: u8) -> Result<u32, DeviceError> {
            unreachable!()
        }
        fn port_write(&mut self, _: u16, _: u8, _: u32) -> Result<(), DeviceError> {
            unreachable!()
        }
        fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
            assert_eq!(size, 1);
            self.accesses.borrow_mut().push(("read", addr));
            Ok(u64::from(self.byte))
        }
        fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
            assert_eq!(size, 1);
            self.accesses.borrow_mut().push(("write", addr));
            self.byte = value as u8;
            Ok(())
        }
        fn tick(&mut self, _: VClock) {}
        fn irq_pending(&self) -> bool {
            false
        }
        fn reset(&mut self) {
            self.byte = 0;
        }
        fn save(&self) -> Vec<u8> {
            vec![self.byte]
        }
        fn restore(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
            self.byte = bytes[0];
            Ok(())
        }
    }
    // Include touching and out-of-order holes to exercise interval merging.
    for windows in [
        vec![(0xD0000, 0x800)],
        vec![(0xD0800, 0x800), (0xD0000, 0x800)],
    ] {
        let mut config = config();
        for (base, size) in windows {
            config
                .memory
                .mmio
                .push(MmioWindow::new(base, size, DeviceId::Vga));
        }
        let ImageSource::Inline(bytes) = &mut config.images[0].source else {
            unreachable!()
        };
        bytes[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
        let code = [
            0xFA, 0xB8, 0, 0xD0, 0x8E, 0xD8, 0xC6, 0x06, 0, 0, 0xA7, 0xA0, 0, 0, 0xF4,
        ];
        bytes[..code.len()].copy_from_slice(&code);
        let mut assembled = assemble_with_options(
            &config,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        let accesses = Rc::new(RefCell::new(Vec::new()));
        *assembled
            .machine
            .bus_mut()
            .device_mut(assembled.device_index[&DeviceId::Vga])
            .unwrap() = Box::new(AdapterRam {
            byte: 0,
            accesses: accesses.clone(),
        });
        assert_eq!(
            assembled.machine.run_bounded(Some(100)).unwrap(),
            machine::RunOutcome::Halted
        );
        assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rax) & 0xFF, 0xA7);
        assert_eq!(*accesses.borrow(), [("write", 0xD0000), ("read", 0xD0000)]);
    }
}

#[test]
fn adapter_mmio_cannot_overlap_an_option_rom_or_the_bios() {
    for (base, image_index) in [(0xC8000, 1), (0xF8000, 0)] {
        let mut config = config();
        config
            .memory
            .mmio
            .push(MmioWindow::new(base, 0x800, DeviceId::Vga));
        assert_eq!(
            error(&config, true),
            ConfigError::ImageOverlapsMmio {
                image_index,
                device: DeviceId::Vga,
                addr: base
            }
        );
        assert_eq!(
            error(&config, true).to_string(),
            format!("image {image_index} overlaps mmio for Vga at {base:#x}")
        );
    }
}

#[test]
fn option_rom_alignment_rejects_half_and_quarter_steps_without_overlap() {
    for base in [0xC8200, 0xC8400] {
        let mut config = config();
        config.images[1].base = base;
        assert_eq!(
            error(&config, true),
            ConfigError::OptionRomMisaligned {
                image_index: 1,
                base
            }
        );
        assert_eq!(
            error(&config, true).to_string(),
            format!("option ROM at {base:#X} must be 2 KiB aligned")
        );
    }
    let mut config = config();
    config.images[1].base = 0xC8800;
    assert!(assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true
        }
    )
    .is_ok());
}

#[test]
fn eight_kib_valid_unknown_rom_is_not_xub() {
    let mut bytes = vec![0; 8192];
    bytes[..4].copy_from_slice(&[0x55, 0xAA, 16, 0xCB]);
    checksum(&mut bytes);
    assert_eq!(bytes.len(), 8192);
    assert_eq!(&bytes[..3], &[0x55, 0xAA, 16]);
    assert_eq!(
        bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        0
    );
    let mut config = config();
    config.images[1].source = ImageSource::Inline(bytes);
    // No ATA card: confusing size/header with XUB identity would report a card
    // mismatch or grant XUB's fixed-disk service under the escape.
    let error = check_option_rom_catalogue(&config, AssemblyOptions::default()).unwrap_err();
    assert!(matches!(
        error,
        ConfigError::OptionRomNotListed { image_index: 1, .. }
    ));
    assert_eq!(
        check_option_rom_catalogue(
            &config,
            AssemblyOptions {
                allow_unlisted_firmware: true
            }
        )
        .unwrap()
        .len(),
        1
    );
    let assembled = assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert!(assembled.option_roms[0].provides.is_empty());
}

#[test]
fn no_rom_ending_at_one_mib_cannot_select_the_highest_option_rom() {
    let mut config = config();
    config.system_bios_image = None;
    config.images[0].base = 0xC0000;
    config.images[1].base = 0xD0000;
    for allow in [false, true] {
        assert_eq!(
            error(&config, allow),
            ConfigError::MissingSystemBios { board: "ibm-5170" }
        );
    }
}

#[test]
fn file_backed_system_bios_is_identified_after_resolution() {
    let Some(_) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let mut config = config();
    config.images[0].source = ImageSource::File(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/kiselev-bios/bios-xi8088.bin"),
    );
    config.images.reverse();
    config.system_bios_image = Some(1);
    assert_eq!(config.validate(), Ok(()));
    assert!(assemble_with_options(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true
        }
    )
    .is_ok());
}
