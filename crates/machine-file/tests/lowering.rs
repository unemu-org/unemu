// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine-file lowering integration tests, see the design § Validation and presets.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use assembly::{ConfigError, DeviceId, DeviceKind, ImageSource, NvramSource, RamRegion, WriteMode};
use machine_file::{lower, parse_toml, ImageReader, MachineFile, Origin, Origins};

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

const MACHINE: &str = include_str!("../../../machines/at-freedos.toml");
const MINIMAL: &str =
    "schema = 1\n[board]\nmodel = 'ibm-5170'\n[firmware.bios]\nimage = 'bios.bin'\n";

#[derive(Default)]
struct Images {
    files: BTreeMap<PathBuf, Vec<u8>>,
    reads: Vec<PathBuf>,
}

impl Images {
    fn rom(file: &Path) -> Self {
        let mut images = Self::default();
        images
            .files
            .insert(file.with_file_name("bios.bin"), vec![0xF4; 0x8000]);
        images
    }
}

impl ImageReader for Images {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.reads.push(path.into());
        self.files.remove(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "missing synthetic image or duplicate read",
            )
        })
    }
}

fn run(input: &str) -> Result<assembly::MachineConfig, machine_file::LowerError> {
    let file = Path::new("test-machines/custom.toml");
    lower(
        &parse_toml(file, input).unwrap(),
        &Origins::from_toml(file, input).unwrap(),
        &mut Images::rom(file),
    )
}

fn synthetic_machine() -> (MachineFile, Origins, Images, Vec<u8>, Vec<u8>) {
    let file = Path::new("machines/at-freedos.toml");
    let mut document = parse_toml(file, MACHINE).unwrap();
    document
        .firmware
        .as_mut()
        .unwrap()
        .bios
        .as_mut()
        .unwrap()
        .sha256 = None;
    document
        .card
        .as_mut()
        .unwrap()
        .last_mut()
        .unwrap()
        .drives
        .as_mut()
        .unwrap()[0]
        .sha256 = None;
    let rom = vec![0xF4; 0x8000];
    let floppy = vec![0; 1_474_560];
    let mut images = Images::default();
    images.files.insert(
        "machines/../vendor/kiselev-bios/bios-xi8088.bin".into(),
        rom.clone(),
    );
    images.files.insert(
        "machines/../vendor/freedos/freedos.img".into(),
        floppy.clone(),
    );
    (
        document,
        Origins::from_toml(file, MACHINE).unwrap(),
        images,
        rom,
        floppy,
    )
}

#[test]
fn synthetic_round_trip_equals_the_whole_rust_preset() {
    let (document, origins, mut images, rom, floppy) = synthetic_machine();
    let config = lower(&document, &origins, &mut images).unwrap();
    let mut expected = assembly::bios(rom, Some(floppy));
    expected.display_switch = assembly::DisplaySwitch::Colour;
    assert_eq!(config, expected);
    assert_eq!(images.reads.len(), 2);
    assert!(images.files.is_empty());
    assert!(config
        .images
        .iter()
        .all(|image| matches!(image.source, ImageSource::Inline(_))));
    assert!(config
        .drives
        .iter()
        .all(|drive| matches!(drive.image, ImageSource::Inline(_))));
}

#[test]
fn repository_fixture_round_trip_checks_pinned_hashes() {
    let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let Some(floppy) = fixtures::fixture("freedos/freedos.img") else {
        return;
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml");
    assert_eq!(
        machine_file::load(path).unwrap(),
        assembly::bios(rom, Some(floppy))
    );
}

#[test]
fn sha256_mismatch_reports_both_hashes() {
    use sha2::{Digest, Sha256};
    let expected = "0".repeat(64);
    let actual = format!("{:x}", Sha256::digest(vec![0xF4; 0x8000]));
    let error = run(&format!("{MINIMAL}sha256 = '{expected}'\n")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("expected {expected}, actual {actual}")),
        "{error}"
    );
    assert!(error.to_string().contains("firmware.bios.image"));
}

#[test]
fn validation_errors_name_the_card_file_and_position() {
    let input = format!("{MINIMAL}\n[[card]]\nid = 'console'\nkind = 'uart-16550'\nirq = 8\n");
    let error = run(&input).unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::BoardCardIrq {
            device: DeviceId::Uart,
            line: 8,
            ..
        })
    ));
    assert_eq!(error.field, "card[console]");
    assert_eq!(error.origin.file, Path::new("test-machines/custom.toml"));
    assert_eq!(error.origin.line, Some(7), "{error}");
    let rendered = error.to_string();
    assert!(rendered.contains("custom.toml:"));
    assert!(rendered.contains("card[console]"));
}

struct TempTree(PathBuf);
impl TempTree {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "machine-file-lowering-{}-{}",
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
fn changed_kiselev_bios_reports_pin_origin_and_defers_catalogue_rejection_to_assembly() {
    use sha2::{Digest, Sha256};

    let Some(mut rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let pin = format!("{:x}", Sha256::digest(&rom));
    rom[0] ^= 1;
    let actual = format!("{:x}", Sha256::digest(&rom));
    let tree = TempTree::new();
    let path = std::fs::canonicalize(&tree.0).unwrap().join("machine.toml");
    let image = path.with_file_name("bios.bin");
    std::fs::write(&image, &rom).unwrap();
    std::fs::write(&path, format!("{MINIMAL}sha256 = '{pin}'\n")).unwrap();
    let machine_file::LoadError::Lower(error) = machine_file::load(&path).unwrap_err() else {
        panic!("image pin must fail during lowering");
    };
    assert_eq!(error.origin.file, path);
    assert_eq!(error.origin.line, Some(5));
    assert_eq!(error.field, "firmware.bios.image");
    assert!(error.validation.is_none());
    assert_eq!(
        error.message,
        format!(
            "sha256 mismatch for {}: expected {pin}, actual {actual}; pin set at {}:6",
            image.display(),
            path.display()
        )
    );
    assert_eq!(
        error.to_string(),
        format!(
            "{}:5: firmware.bios.image: {}",
            path.display(),
            error.message
        )
    );

    std::fs::write(&path, MINIMAL).unwrap();
    let config = machine_file::load(&path).unwrap();
    assert_eq!(config.images[0].source, ImageSource::Inline(rom));
    let error = match assembly::assemble(&config) {
        Err(assembly::AssemblyError::Config(error)) => error,
        _ => panic!("assembly must reject the unlisted image"),
    };
    assert_eq!(
        error,
        ConfigError::FirmwareNotListed {
            sha256: actual.clone(),
            board: "ibm-5170",
            known: "kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh), kiselev-xi8088-1.0.2-atreset (open, vendor/kiselev-bios/fetch.sh), unemu-at-bios-1.0.2-unemu.3 (open, vendor/unemu-at-bios/fetch.sh), unemu-at-bios-1.0.2-unemu.2 (open, vendor/unemu-at-bios/fetch.sh at unemupc98 bcb375e6410b18266bd5f99533624b768d067867), ibm-5170-bios-1984-01-10 (user-supplied: U27 6181028 even, U47 6181029 odd or U17 6181024 even@0, U37 6181026 odd@0, U27 6181025 even@32768, U47 6181027 odd@32768; see vendor/user-roms/README.md), ibm-5170-bios-1985-06-10 (user-supplied: U27 6480090 even, U47 6480091 odd; see vendor/user-roms/README.md), ibm-5170-bios-1985-11-15 (user-supplied: U27 61X9266 even, U47 61X9265 odd; see vendor/user-roms/README.md), landmark-rom-post-at-1.2.0 (user-supplied: U27 27256 even, U47 27256 odd; see vendor/user-roms/README.md)".into(),
        }
    );
    assert_eq!(error.to_string(), format!("bios image (sha256 {actual}) is not a known image for board \"ibm-5170\"; known: kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh), kiselev-xi8088-1.0.2-atreset (open, vendor/kiselev-bios/fetch.sh), unemu-at-bios-1.0.2-unemu.3 (open, vendor/unemu-at-bios/fetch.sh), unemu-at-bios-1.0.2-unemu.2 (open, vendor/unemu-at-bios/fetch.sh at unemupc98 bcb375e6410b18266bd5f99533624b768d067867), ibm-5170-bios-1984-01-10 (user-supplied: U27 6181028 even, U47 6181029 odd or U17 6181024 even@0, U37 6181026 odd@0, U27 6181025 even@32768, U47 6181027 odd@32768; see vendor/user-roms/README.md), ibm-5170-bios-1985-06-10 (user-supplied: U27 6480090 even, U47 6480091 odd; see vendor/user-roms/README.md), ibm-5170-bios-1985-11-15 (user-supplied: U27 61X9266 even, U47 61X9265 odd; see vendor/user-roms/README.md), landmark-rom-post-at-1.2.0 (user-supplied: U27 27256 even, U47 27256 odd; see vendor/user-roms/README.md)"));
}

#[test]
fn relative_image_paths_use_the_machine_file_directory() {
    let tree = TempTree::new();
    let directory = tree.0.join("machines");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("custom.toml");
    let rom = vec![0xF4; 0x8000];
    std::fs::write(&path, MINIMAL).unwrap();
    std::fs::write(directory.join("bios.bin"), &rom).unwrap();
    assert_ne!(std::env::current_dir().unwrap(), directory);
    let config = machine_file::load(path).unwrap();
    assert_eq!(config.images[0].source, ImageSource::Inline(rom));
}

#[test]
fn individual_field_origins_resolve_images_before_any_future_merge() {
    let file = Path::new("child/custom.toml");
    let mut origins = Origins::new(file);
    origins.insert(
        "firmware.bios.image",
        Origin {
            file: "parent/base.toml".into(),
            line: Some(9),
        },
    );
    let mut images = Images::rom(Path::new("parent/base.toml"));
    let config = lower(&parse_toml(file, MINIMAL).unwrap(), &origins, &mut images).unwrap();
    assert_eq!(images.reads, [Path::new("parent/bios.bin")]);
    assert!(matches!(config.images[0].source, ImageSource::Inline(_)));
}

#[test]
fn nvram_journal_requires_an_explicit_path() {
    let nvram = run(&format!(
        "{MINIMAL}[board.rtc]\nnvram={{image='cmos.bin',write='journal'}}\n"
    ))
    .unwrap_err();
    assert_eq!(nvram.field, "board.rtc.nvram.journal");
    assert_eq!(nvram.message, "required field is missing");
}

#[test]
fn card_defaults_overrides_and_file_order_are_preserved() {
    for (port, irq) in [(0x3F8, 4), (0x3E8, 4), (0x2F8, 3), (0x2E8, 3)] {
        let config = run(&format!(
            "{MINIMAL}[[card]]\nkind='uart-16550'\nport={port}\n"
        ))
        .unwrap();
        let uart = config
            .devices
            .iter()
            .find(|device| device.id == DeviceId::Uart)
            .unwrap();
        assert_eq!(uart.irq, Some(irq));
        assert_eq!(
            uart.kind,
            DeviceKind::Uart(assembly::UartCfg {
                base: port,
                mouse: false
            })
        );
        assert_eq!(uart.ports, [assembly::PortWindow::new(port, port + 7)]);
    }
    let config = run(&format!("{MINIMAL}[[card]]\nkind='fdc-765'\nirq='none'\ndma='none'\n[[card]]\nkind='uart-16550'\nport=0x300\nirq=5\n[[card]]\nkind='vga-text'\n[[card]]\nkind='mda'\n")).unwrap();
    assert_eq!(
        config
            .devices
            .iter()
            .filter(
                |device| [DeviceId::Fdc, DeviceId::Uart, DeviceId::Vga, DeviceId::Mda]
                    .contains(&device.id)
            )
            .map(|device| device.id)
            .collect::<Vec<_>>(),
        [DeviceId::Fdc, DeviceId::Uart, DeviceId::Vga, DeviceId::Mda]
    );
    let fdc = config
        .devices
        .iter()
        .find(|device| device.id == DeviceId::Fdc)
        .unwrap();
    let uart = config
        .devices
        .iter()
        .find(|device| device.id == DeviceId::Uart)
        .unwrap();
    assert_eq!(fdc.irq, None);
    assert_eq!(fdc.dma, None);
    assert_eq!(fdc.ports, assembly::FdcCfg::port_windows(false));
    assert_eq!(uart.irq, Some(5));
    assert_eq!(
        config
            .memory
            .mmio
            .iter()
            .map(|window| window.device)
            .collect::<Vec<_>>(),
        [DeviceId::Mda, DeviceId::Vga]
    );
}

#[test]
fn invalid_card_fields_and_duplicate_roles_are_rejected() {
    for fields in [
        "kind='mda'\nport=0x3B0",
        "kind='vga-text'\nirq='none'",
        "kind='uart-16550'\ndma='none'",
        "kind='mda'\ndrives=[]",
        "kind='fdc-765'\nport=0x370",
        "kind='uart-16550'\nport=0x300",
        "kind='mda'\nremove=false",
    ] {
        assert!(
            run(&format!("{MINIMAL}[[card]]\n{fields}\n")).is_err(),
            "{fields}"
        );
    }
    let error = run(&format!("{MINIMAL}[[card]]\nid='first'\nkind='uart-16550'\n[[card]]\nid='second'\nkind='uart-16550'\nport=0x2F8\n")).unwrap_err();
    assert_eq!(
        error.validation.as_deref(),
        Some(&ConfigError::DuplicateDeviceId(DeviceId::Uart))
    );
    assert!(error.to_string().contains("card[first]"));
    assert!(error.to_string().contains("card[second]"));
}

#[test]
fn firmware_open_bus_starts_after_configured_extended_ram() {
    use assembly::test_support::{Cpu, CpuStop, DmaAccess};
    let mut config = run(&format!("{MINIMAL}[memory]\nextended_kib=1024\n")).unwrap();
    assert_eq!(config.memory.ram.last().unwrap().end(), 0x200000);
    let ImageSource::Inline(rom) = &mut config.images[0].source else {
        panic!("inline BIOS required")
    };
    // FFFF:0010 reaches installed RAM without address-size overrides.
    let program = [
        0xFA, 0xB8, 0xFF, 0xFF, 0x8E, 0xD8, 0xC6, 0x06, 0x10, 0, 0x5A, 0xA0, 0x10, 0, 0x31, 0xDB,
        0x8E, 0xDB, 0xA2, 0, 5, 0xF4,
    ];
    rom[..program.len()].copy_from_slice(&program);
    rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
    let mut cpu = assembly::test_support::mapped_cpu(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert_eq!(cpu.run(Some(100)).unwrap(), CpuStop::Halt);
    let mut value = [0];
    cpu.read_mem(0x500, &mut value).unwrap();
    assert_eq!(value, [0x5A]);
    // Host writes populate ROM too, so use the guest DMA seam for high stores.
    for (address, expected) in [(0x1FFFFFu32, 0x5A), (0x200000, 0xFF), (0xFDFFFF, 0xFF)] {
        assert_eq!(cpu.guest_dma_write(address, 0x5A), DmaAccess::Complete(()));
        cpu.read_mem(u64::from(address), &mut value).unwrap();
        assert_eq!(value, [expected]);
    }
}

#[test]
fn extended_memory_limit_reserves_the_board_alias() {
    for value in [14208, 14272, 15232] {
        let config = run(&format!("{MINIMAL}[memory]\nextended_kib={value}\n")).unwrap();
        assert_eq!(
            config.memory.ram.last().unwrap().end(),
            0x100000 + value * 1024
        );
    }
}

#[test]
fn extended_memory_above_board_limit_has_exact_diagnostic() {
    for value in [15296, 15360] {
        let result = run(&format!("{MINIMAL}[memory]\nextended_kib={value}\n"));
        assert!(result.is_err(), "extended_kib={value} must be rejected");
        let error = result.unwrap_err();
        assert_eq!(error.field, "memory.extended_kib");
        assert_eq!(
            error.message,
            "must be a multiple of 64 KiB, at most 15232 KiB on ibm-5170"
        );
    }
}

#[test]
fn full_extended_memory_map_touches_the_high_rom_alias() {
    use assembly::test_support::{Cpu, DmaAccess};
    let config = run(&format!("{MINIMAL}[memory]\nextended_kib=15232\n")).unwrap();
    assert_eq!(config.memory.ram.last().unwrap().end(), 0xFE0000);
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    let mut regions = assembly::test_support::cpu_mapping_regions(&config, options).unwrap();
    regions.sort_unstable();
    assert_eq!(
        regions,
        vec![
            (0, 0xC0000, "ram"),
            (0xC0000, 0xF8000, "rom"),
            (0xF8000, 0x100000, "rom"),
            (0x100000, 0xFE0000, "ram"),
            (0xFE0000, 0x1000000, "rom"),
        ]
    );
    assert!(regions.windows(2).all(|pair| pair[0].1 == pair[1].0));
    let mut cpu = assembly::test_support::mapped_cpu(&config, options).unwrap();
    // High physical stores use byte DMA; the host loader would overwrite ROM.
    assert_eq!(cpu.guest_dma_write(0xFDFFFF, 0x5A), DmaAccess::Complete(()));
    assert_eq!(cpu.guest_dma_write(0xFE0000, 0x5A), DmaAccess::Complete(()));
    let mut last_ram = [0];
    cpu.read_mem(0xFDFFFF, &mut last_ram).unwrap();
    assert_eq!(last_ram, [0x5A]);
    let mut low = [0xFF];
    let mut high = [0xFF];
    cpu.read_mem(0xE0000, &mut low).unwrap();
    cpu.read_mem(0xFE0000, &mut high).unwrap();
    assert_eq!(high, low);
}

#[test]
fn ram_rule_reserves_adapter_space_and_keeps_the_conventional_gap() {
    let file = Path::new("test-machines/custom.toml");
    let input = format!("{MINIMAL}[memory]\nconventional_kib=512\nextended_kib=128\n[cpu]\nmodel='80286'\nrate='3/2'\n[[card]]\nkind='mda'\n");
    let mut images = Images::rom(file);
    let config = lower(
        &parse_toml(file, &input).unwrap(),
        &Origins::from_toml(file, &input).unwrap(),
        &mut images,
    )
    .unwrap();
    assert_eq!(config.memory.address_space_bytes, 0x120000);
    assert_eq!(
        config.memory.ram,
        [
            RamRegion::new(0, 0x80000),
            RamRegion::new(0xA0000, 0x10000),
            RamRegion::new(0xB8000, 0x8000),
            RamRegion::new(0x100000, 0x20000)
        ]
    );
    assert_eq!(config.clock.retired_per_pit_clock, 3);
    assert_eq!(config.clock.retired_per_pit_clock_denominator, 2);
    for memory in [
        "conventional_kib=641",
        "conventional_kib=15",
        "extended_kib=15424",
        "extended_kib=63",
    ] {
        assert!(
            run(&format!("{MINIMAL}[memory]\n{memory}\n")).is_err(),
            "{memory}"
        );
    }
}

#[test]
fn post_port_overrides_update_the_board_dma_decode() {
    for port in ["'none'", "0x190"] {
        let input = MINIMAL.replace(
            "model = 'ibm-5170'",
            &format!("model = 'ibm-5170'\npost_port={port}"),
        );
        let config = run(&input).unwrap();
        for mut expected in assembly::BoardModel::Ibm5170
            .motherboard_devices()
            .into_iter()
            .filter(|device| matches!(device.id, DeviceId::Dma | DeviceId::DmaWord))
        {
            if expected.id == DeviceId::Dma {
                expected.ports[1].start = 0x80;
            }
            let actual = config
                .devices
                .iter()
                .find(|device| device.id == expected.id)
                .unwrap();
            assert_eq!(actual, &expected);
        }
        let post = config.devices.iter().find(|d| d.id == DeviceId::Post);
        if port == "'none'" {
            assert!(post.is_none());
        } else {
            let post = post.unwrap();
            assert_eq!(post.ports, [assembly::PortWindow::single(0x190)]);
            assert_eq!(
                post.kind,
                DeviceKind::Post(assembly::PostPortCfg { port: 0x190 })
            );
        }
    }
}

#[test]
fn rtc_and_nvram_are_lowered_without_repairing_image_bytes() {
    let file = Path::new("test-machines/custom.toml");
    let input =
        format!("{MINIMAL}[board.rtc]\nstart='2024-02-29T12:34:56'\nnvram={{image='cmos.bin'}}\n");
    let mut images = Images::rom(file);
    images
        .files
        .insert(file.with_file_name("cmos.bin"), (0..64).collect());
    let config = lower(
        &parse_toml(file, &input).unwrap(),
        &Origins::from_toml(file, &input).unwrap(),
        &mut images,
    )
    .unwrap();
    let DeviceKind::Rtc(ref rtc) = config
        .devices
        .iter()
        .find(|d| d.id == DeviceId::Rtc)
        .unwrap()
        .kind
    else {
        panic!("RTC kind")
    };
    assert_eq!(rtc.start.to_string(), "2024-02-29T12:34:56");
    assert_eq!(
        rtc.nvram,
        NvramSource::Image(std::array::from_fn(|i| i as u8))
    );
    let config = run(&format!("{MINIMAL}[board.rtc]\nnvram='blank'\n")).unwrap();
    assert!(config.devices.iter().any(|d| matches!(
        d.kind,
        DeviceKind::Rtc(assembly::CmosCfg {
            nvram: NvramSource::Blank,
            ..
        })
    )));
    for start in [
        "2023-02-29T00:00:00",
        "2000-01-01T24:00:00",
        "1979-01-01T00:00:00",
    ] {
        let error = run(&format!("{MINIMAL}[board.rtc]\nstart='{start}'\n")).unwrap_err();
        assert!(
            matches!(
                error.validation.as_deref(),
                Some(ConfigError::InvalidRtcStart { .. })
            ),
            "{error}"
        );
        assert_eq!(error.field, "board.rtc");
    }
    assert!(run(&format!("{MINIMAL}[board.rtc]\nstart='2024-2-29'\n"))
        .unwrap_err()
        .message
        .contains("YYYY-MM-DD"));
    assert!(run(&format!(
        "{MINIMAL}[board.rtc]\nnvram={{image='cmos.bin',write='readonly'}}\n"
    ))
    .unwrap_err()
    .message
    .contains("readonly NVRAM is unsupported: the MC146818 has no write-protect input; use write = \"volatile\""));
    let mut images = Images::rom(file);
    images
        .files
        .insert(file.with_file_name("cmos.bin"), vec![0; 63]);
    let error = lower(
        &parse_toml(file, &input).unwrap(),
        &Origins::new(file),
        &mut images,
    )
    .unwrap_err();
    assert!(error.message.contains("64 bytes, got 63"));
}

#[test]
fn drives_sort_by_unit_and_retain_controller_geometry_and_write_mode() {
    let file = Path::new("test-machines/custom.toml");
    let input = format!("{MINIMAL}provides=['floppy']\n[[card]]\nid='diskette'\nkind='fdc-765'\ndma=1\nirq=5\n[[card.drives]]\nunit=1\ntype='360K'\nimage='small.img'\nwrite='readonly'\n[[card.drives]]\nunit=0\ntype='720K'\nimage='medium.img'\n");
    let mut images = Images::rom(file);
    images
        .files
        .insert(file.with_file_name("small.img"), vec![0; 368_640]);
    images
        .files
        .insert(file.with_file_name("medium.img"), vec![0; 737_280]);
    let config = lower(
        &parse_toml(file, &input).unwrap(),
        &Origins::from_toml(file, &input).unwrap(),
        &mut images,
    )
    .unwrap();
    assert_eq!(
        config
            .drives
            .iter()
            .map(|drive| (
                drive.controller,
                drive.unit,
                drive.kind.image_bytes(),
                drive.write.clone()
            ))
            .collect::<Vec<_>>(),
        [
            (DeviceId::Fdc, 0, 737_280, WriteMode::Volatile),
            (DeviceId::Fdc, 1, 368_640, WriteMode::ReadOnly)
        ]
    );
    let fdc = config.devices.last().unwrap();
    assert_eq!(fdc.irq, Some(5));
    assert_eq!(fdc.dma.unwrap().channel, 1);
}

#[test]
fn drive_and_image_validation_keep_the_defining_origin() {
    let (mut document, origins, mut images, _, _) = synthetic_machine();
    images
        .files
        .insert("machines/../vendor/freedos/freedos.img".into(), vec![0; 7]);
    let error = lower(&document, &origins, &mut images).unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::ImageGeometryMismatch { drive_index: 0, .. })
    ));
    assert_eq!(error.field, "card[fdc-765].drives[unit=0]");
    assert_eq!(error.origin.file, Path::new("machines/at-freedos.toml"));
    let drive_line = MACHINE[..MACHINE.find("[[card.drives]]").unwrap()]
        .bytes()
        .filter(|&byte| byte == b'\n')
        .count()
        + 1;
    assert_eq!(error.origin.line, Some(drive_line));
    let mut images = synthetic_machine().2;
    document
        .firmware
        .as_mut()
        .unwrap()
        .bios
        .as_mut()
        .unwrap()
        .provides = None;
    let error = lower(&document, &origins, &mut images).unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::NoProvider {
            device: DeviceId::Fdc,
            ..
        })
    ));
    assert_eq!(error.field, "card[fdc-765]");
}

#[test]
fn incomplete_or_unmerged_documents_and_image_io_fail_clearly() {
    assert_eq!(run("schema=1\n").unwrap_err().field, "board.model");
    assert_eq!(
        run("schema=1\n[board]\nmodel='ibm-5170'\n")
            .unwrap_err()
            .field,
        "firmware.bios"
    );
    let error = run(&format!("extends='parent.toml'\n{MINIMAL}")).unwrap_err();
    assert_eq!(error.field, "extends");
    let error = run(&MINIMAL.replace("bios.bin", "missing.bin")).unwrap_err();
    assert!(error
        .message
        .contains("cannot read image test-machines/missing.bin"));
    assert!(error.message.contains("missing synthetic image"));
    let error = run(&format!(
        "{MINIMAL}[[card]]\nkind='uart-16550'\nport=65535\nirq='none'\n"
    ))
    .unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::UartPortWindowOutOfSpace { .. })
    ));
}

#[test]
fn duplicate_card_ids_report_both_origins() {
    for cards in [
        "[[card]]\nid='x'\nkind='mda'\n[[card]]\nid='x'\nkind='vga-text'\n",
        "[[card]]\nid='mda'\nkind='vga-text'\n[[card]]\nkind='mda'\n",
        "[[card]]\nkind='mda'\n[[card]]\nkind='mda'\n",
    ] {
        let input = format!("{MINIMAL}{cards}");
        let file = Path::new("child/custom.toml");
        let document = parse_toml(file, &input).unwrap();
        let mut origins = Origins::from_toml(file, &input).unwrap();
        origins.insert(
            "card[0]",
            Origin {
                file: "parent/base.toml".into(),
                line: Some(12),
            },
        );
        origins.insert(
            "card[1]",
            Origin {
                file: file.into(),
                line: Some(25),
            },
        );
        let error = lower(&document, &origins, &mut Images::rom(file)).unwrap_err();
        assert!(error.validation.is_none());
        assert!(error.message.contains("duplicate card id"), "{error}");
        assert!(error.to_string().contains("parent/base.toml:12"), "{error}");
        assert!(
            error.to_string().contains("child/custom.toml:25"),
            "{error}"
        );
    }
}

#[test]
fn validation_diagnostics_start_with_the_card_or_image() {
    let error = run(&format!(
        "{MINIMAL}[[card]]\nid='com'\nkind='uart-16550'\nport=0x40\nirq='none'\n"
    ))
    .unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::PortOverlap { .. })
    ));
    assert_eq!(error.field, "card[com]");
    assert!(
        error
            .message
            .contains("; test-machines/custom.toml:2: board"),
        "{error}"
    );
    let file = Path::new("test-machines/custom.toml");
    let input = format!("{MINIMAL}base=0xB0000\n[[card]]\nkind='mda'\n");
    let mut images = Images::rom(file);
    images
        .files
        .insert(file.with_file_name("bios.bin"), vec![0xF4; 0x50000]);
    let error = lower(
        &parse_toml(file, &input).unwrap(),
        &Origins::from_toml(file, &input).unwrap(),
        &mut images,
    )
    .unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::ImageOverlapsMmio { image_index: 0, .. })
    ));
    assert_eq!(error.field, "firmware.bios");
    assert!(error.message.contains("card[mda]"), "{error}");

    let error = run(&format!("{MINIMAL}base=0xF0000\n")).unwrap_err();
    assert_eq!(
        error.validation.as_deref(),
        Some(&ConfigError::BiosPlacement {
            base: 0xF0000,
            size: 32768
        })
    );
    assert_eq!(error.field, "firmware.bios.image");
    assert_eq!(error.origin.file, file);
    assert_eq!(error.origin.line, Some(4));
    assert_eq!(
        error.message,
        "system BIOS of 32 KiB at 0xf0000 must have a positive size that is a multiple of 8 KiB and end at 0x100000"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_machine_paths_use_the_target_directory() {
    let tree = TempTree::new();
    let target = tree.0.join("target");
    let aliases = tree.0.join("aliases");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&aliases).unwrap();
    let file = target.join("custom.yaml");
    let alias = aliases.join("custom.toml");
    let rom = vec![0xF4; 0x8000];
    std::fs::write(&file, MINIMAL).unwrap();
    std::fs::write(target.join("bios.bin"), &rom).unwrap();
    std::fs::write(aliases.join("bios.bin"), vec![0; 0x8000]).unwrap();
    std::os::unix::fs::symlink("../target/custom.yaml", &alias).unwrap();
    assert_eq!(
        machine_file::load(&alias).unwrap().images[0].source,
        ImageSource::Inline(rom)
    );
    std::fs::write(
        &file,
        format!("{MINIMAL}[[card]]\nkind='uart-16550'\nirq=8\n"),
    )
    .unwrap();
    let machine_file::LoadError::Lower(error) = machine_file::load(&alias).unwrap_err() else {
        panic!("expected validation error")
    };
    assert_eq!(error.origin.file, alias);
}

#[test]
fn drive_lowering_errors_name_the_unit_and_defining_origin() {
    let file = Path::new("test-machines/custom.toml");
    let input = format!("{MINIMAL}provides=['floppy']\n[[card]]\nid='diskette'\nkind='fdc-765'\n[[card.drives]]\nunit=1\ntype='360K'\nimage='one.img'\n[[card.drives]]\nunit=0\ntype='720K'\n");
    let origins = Origins::from_toml(file, &input).unwrap();
    let error = lower(
        &parse_toml(file, &input).unwrap(),
        &origins,
        &mut Images::rom(file),
    )
    .unwrap_err();
    assert_eq!(error.field, "card[diskette].drives[unit=0].image");
    assert_eq!(&error.origin, origins.get("card[0].drives[1]"));
    assert_ne!(error.origin.line, origins.get("card[0].drives[0]").line);
}

#[test]
fn non_journal_writes_reject_journal_options() {
    let file = Path::new("test-machines/custom.toml");
    for option in ["journal='disk.journal'", "keep_generations=2"] {
        let input = format!("{MINIMAL}provides=['floppy']\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.44M'\nimage='disk.img'\nwrite='volatile'\n{option}\n");
        let mut images = Images::rom(file);
        images
            .files
            .insert(file.with_file_name("disk.img"), vec![0; 1_474_560]);
        let error = lower(
            &parse_toml(file, &input).unwrap(),
            &Origins::from_toml(file, &input).unwrap(),
            &mut images,
        )
        .unwrap_err();
        assert!(
            error
                .message
                .contains("only accepted with write = \"journal\""),
            "{error}"
        );
        assert_eq!(error.field, "card[fdc-765].drives[unit=0].write");
    }
}

#[test]
fn option_rom_bases_require_two_kib_alignment() {
    let file = Path::new("test-machines/custom.toml");
    let input = format!("{MINIMAL}[[firmware.option_roms]]\nimage='option.bin'\nbase=0xC8001\n");
    let mut images = Images::rom(file);
    images
        .files
        .insert(file.with_file_name("option.bin"), vec![0; 0x2000]);
    let error = lower(
        &parse_toml(file, &input).unwrap(),
        &Origins::from_toml(file, &input).unwrap(),
        &mut images,
    )
    .unwrap_err();
    assert_eq!(error.field, "firmware.option_roms[0].base");
    assert!(error.message.contains("2 KiB aligned"));
}

#[test]
fn option_rom_validation_retains_each_image_origin() {
    let file = Path::new("test-machines/custom.toml");
    let input = format!("{MINIMAL}[[firmware.option_roms]]\nimage='first.bin'\nbase=0xC8000\n[[firmware.option_roms]]\nimage='second.bin'\nbase=0xC8000\n");
    let mut origins = Origins::from_toml(file, &input).unwrap();
    origins.insert(
        "firmware.option_roms[0]",
        Origin {
            file: "roms/first.toml".into(),
            line: Some(11),
        },
    );
    origins.insert(
        "firmware.option_roms[1]",
        Origin {
            file: "roms/second.toml".into(),
            line: Some(22),
        },
    );
    let mut images = Images::rom(file);
    images.files.insert("roms/first.bin".into(), vec![0; 2048]);
    images.files.insert("roms/second.bin".into(), vec![0; 2048]);
    let error = lower(&parse_toml(file, &input).unwrap(), &origins, &mut images).unwrap_err();
    assert_eq!(
        error.validation.as_deref(),
        Some(&ConfigError::ImageOverlap {
            a: 1,
            b: 2,
            addr: 0xC8000
        })
    );
    assert_eq!(error.field, "firmware.option_roms[0]");
    assert!(
        error
            .message
            .starts_with("firmware.option_roms[0] and firmware.option_roms[1] overlap at 0xc8000"),
        "{error}"
    );
    assert_eq!(&error.origin, origins.get("firmware.option_roms[0]"));
    assert!(
        error
            .to_string()
            .contains("roms/second.toml:22: firmware.option_roms[1]"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn relative_machine_paths_stay_relative_in_diagnostics() {
    let tree = TempTree::new();
    let target = tree.0.join("real-source");
    let aliases = tree.0.join("spelled-source");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink("real-source", &aliases).unwrap();
    let file = aliases.join("custom.toml");
    let current = std::env::current_dir().unwrap();
    let mut relative = PathBuf::new();
    for _ in current.ancestors().skip(1) {
        relative.push("..");
    }
    relative.push(file.strip_prefix("/").unwrap());
    assert!(relative.is_relative());
    for input in [
        "schema=1\nunknown=true\n".to_owned(),
        "schema=1\n".to_owned(),
        MINIMAL.to_owned(),
    ] {
        std::fs::write(&file, input).unwrap();
        let diagnostic = machine_file::load(&relative).unwrap_err();
        let reported_file = match &diagnostic {
            machine_file::LoadError::Parse(error) => error.path(),
            machine_file::LoadError::Lower(error) => &error.origin.file,
        };
        let error = diagnostic.to_string();
        assert!(
            error.starts_with(&format!("{}:", relative.display())),
            "{error}"
        );
        assert_eq!(reported_file, &relative);
        assert!(
            reported_file.ends_with("spelled-source/custom.toml"),
            "{error}"
        );
    }
}

#[test]
fn missing_card_kind_does_not_synthesize_a_duplicate_id() {
    let error = run(&format!(
        "{MINIMAL}[[card]]\nid='1'\nkind='mda'\n[[card]]\n"
    ))
    .unwrap_err();
    assert_eq!(error.field, "card #1.kind");
    assert_eq!(error.message, "required field is missing");
    assert!(error.validation.is_none());
}

#[test]
fn duplicate_drive_messages_use_unit_labels_after_sorting() {
    let file = Path::new("test-machines/custom.toml");
    let input = format!("{MINIMAL}provides=['floppy']\n[[card]]\nid='diskette'\nkind='fdc-765'\n[[card.drives]]\nunit=1\ntype='360K'\nimage='one-a.img'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='zero.img'\n[[card.drives]]\nunit=1\ntype='360K'\nimage='one-b.img'\n");
    let origins = Origins::from_toml(file, &input).unwrap();
    let mut images = Images::rom(file);
    for name in ["one-a.img", "zero.img", "one-b.img"] {
        images
            .files
            .insert(file.with_file_name(name), vec![0; 368_640]);
    }
    let error = lower(&parse_toml(file, &input).unwrap(), &origins, &mut images).unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::DuplicateDriveUnit {
            a: 1,
            b: 2,
            unit: 1,
            ..
        })
    ));
    assert_eq!(&error.origin, origins.get("card[0].drives[0]"));
    let first = format!(
        "card[diskette].drives[unit=1] ({})",
        origins.get("card[0].drives[0]")
    );
    let second = format!(
        "card[diskette].drives[unit=1] ({})",
        origins.get("card[0].drives[2]")
    );
    assert_ne!(first, second);
    assert!(
        error.message.starts_with(&format!(
            "{first} and {second} bind to controller Fdc unit 1"
        )),
        "{error}"
    );
    assert!(!error.message.contains("drives 1 and 2"), "{error}");
    assert!(
        error.message.contains(&format!(
            "{}: card[diskette].drives[unit=1]",
            origins.get("card[0].drives[2]")
        )),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn symlink_image_errors_show_the_spelled_and_resolved_paths() {
    use sha2::{Digest, Sha256};
    let tree = TempTree::new();
    let target = tree.0.join("target");
    let aliases = tree.0.join("aliases");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&aliases).unwrap();
    let file = target.join("custom.toml");
    let alias = aliases.join("custom.toml");
    std::fs::write(&file, MINIMAL).unwrap();
    let spelled = aliases.join("bios.bin");
    std::fs::write(&spelled, vec![0; 0x8000]).unwrap();
    std::os::unix::fs::symlink("../target/custom.toml", &alias).unwrap();
    let resolved = target.canonicalize().unwrap().join("bios.bin");
    assert!(spelled.is_file());
    assert!(!resolved.exists());
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
    std::fs::write(&file, format!("{MINIMAL}sha256='{pin}'\n")).unwrap();
    let machine_file::LoadError::Lower(error) = machine_file::load(&alias).unwrap_err() else {
        panic!("expected image integrity error")
    };
    assert_eq!(
        error.message,
        format!(
            "sha256 mismatch for {description}: expected {pin}, actual {:x}; pin set at {}:6",
            Sha256::digest(&rom),
            alias.display()
        )
    );
}

const IDE: &str = "[[card]]\nid='disk'\nkind='ide'\n[[card.drives]]\nchannel='master'\nimage='disk.img'\ngeometry={cylinders=8,heads=2,sectors=4}\nwrite='readonly'\n";

#[test]
fn at_adapter_empty_ibm5170_card_echoes_through_assembled_bus_and_preserves_fdc() {
    let input = format!("{MINIMAL}provides=['fixed-disk']\n[[card]]\nkind='ide'\ncontroller_model='at-adapter'\n[[card]]\nkind='fdc-765'\n");
    let config = ide_lower(&input).unwrap();
    assert_eq!(config.board, assembly::BoardModel::Ibm5170);
    assert!(config.drives.is_empty());
    let mut assembled = assembly::assemble_with_options(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let bus = assembled.machine.bus_mut();
    bus.port_read(0x3F1, 1).unwrap();
    assert_eq!(bus.port_read(0x5F7, 1).unwrap(), 0xFF);
    let mut idle = false;
    for _ in 0..16 {
        if bus.port_read(0x1F7, 1).unwrap() & 0x80 == 0 {
            idle = true;
            break;
        }
    }
    assert!(idle, "adapter remained busy during the register probe");
    for pattern in [0x55, 0xAA] {
        bus.port_write(0x1F4, 1, pattern).unwrap();
        assert_eq!(bus.port_read(0x1F4, 1).unwrap(), pattern);
    }
    assert_eq!(bus.port_read(0x1F7, 1).unwrap(), 0);
    assert_eq!(bus.port_read(0x1F1, 1).unwrap(), 1);
    assert_eq!(bus.port_read(0x3F6, 1).unwrap(), 0xFF);
    assert_eq!(bus.port_read(0x3F4, 1).unwrap(), 0x80);
    assert_eq!(bus.port_read(0x3F7, 1).unwrap(), 0);
}

#[test]
fn at_adapter_model_is_ide_only_and_unknown_models_keep_source_positions() {
    let error = ide_lower(&format!(
        "{MINIMAL}[[card]]\nkind='fdc-765'\ncontroller_model='at-adapter'\n"
    ))
    .unwrap_err();
    assert_eq!(error.field, "card[fdc-765].controller_model");
    assert!(error.message.contains("only supported by ide"));
    assert_eq!(error.origin.line, Some(6));
    let file = Path::new("model.toml");
    let error = parse_toml(
        file,
        "schema=1\n[[card]]\nkind='ide'\ncontroller_model='unknown'\n",
    )
    .unwrap_err();
    assert!(error.message().contains("unknown variant"));
}

fn ide_lower(input: &str) -> Result<assembly::MachineConfig, machine_file::LowerError> {
    let file = Path::new("machines/ide-test.toml");
    let mut images = Images::rom(file);
    images
        .files
        .insert("machines/disk.img".into(), vec![0xAA; 32768]);
    images
        .files
        .insert("machines/slave.img".into(), vec![0x55; 16384]);
    lower(
        &parse_toml(file, input).unwrap(),
        &Origins::from_toml(file, input).unwrap(),
        &mut images,
    )
}

#[test]
fn ide_lowering_binds_geometry_media_and_splits_fdc_for_primary_only() {
    for (port, control, irq) in [(0x1F0, 0x3F6, 14), (0x170, 0x376, 15)] {
        let input = format!(
            "{MINIMAL}provides=['fixed-disk']\n{}[[card]]\nkind='fdc-765'\n",
            IDE.replace("kind='ide'", &format!("kind='ide'\nport={port}"))
        );
        let config = ide_lower(&input).unwrap();
        let ide = config
            .devices
            .iter()
            .find(|device| device.id == DeviceId::Ide)
            .unwrap();
        assert_eq!(
            ide.kind,
            DeviceKind::Ide(assembly::IdeCfg {
                model: assembly::AtaControllerModel::Ata,
                base: port,
                control
            })
        );
        assert_eq!(ide.irq, Some(irq));
        assert_eq!(ide.dma, None);
        assert_eq!(config.drives[0].controller, DeviceId::Ide);
        assert_eq!(config.drives[0].unit, 0);
        assert_eq!(
            config.drives[0].kind,
            assembly::DriveKind::HardDisk {
                geometry: assembly::HardDiskGeometry {
                    cylinders: 8,
                    heads: 2,
                    sectors_per_track: 4,
                    bytes_per_sector: 512
                }
            }
        );
        assert_eq!(
            config.drives[0].image,
            ImageSource::Inline(vec![0xAA; 32768])
        );
        assert_eq!(config.drives[0].write, WriteMode::ReadOnly);
        assert_eq!(
            config.devices.last().unwrap().ports,
            assembly::FdcCfg::port_windows(port == 0x1F0)
        );
    }
}

#[test]
fn ide_no_provider_reports_the_card_file_and_line() {
    let error = ide_lower(&format!("{MINIMAL}[[card]]\nkind='ide'\n")).unwrap_err();
    assert_eq!(error.to_string(), "machines/ide-test.toml:6: card[ide]: device Ide needs firmware service fixed-disk; no ROM provides it");
    assert_eq!(
        error.validation.as_deref(),
        Some(&ConfigError::NoProvider {
            device: DeviceId::Ide,
            service: assembly::FirmwareService::FixedDisk
        })
    );
}

#[test]
fn ide_geometry_and_sha256_errors_keep_drive_origins() {
    let input = format!("{MINIMAL}provides=['fixed-disk']\n{IDE}");
    let error = ide_lower(&input.replace("cylinders=8", "cylinders=0")).unwrap_err();
    assert_eq!(error.to_string(), "machines/ide-test.toml:10: card[disk].drives[channel=master]: card[disk].drives[channel=master] has invalid ATA geometry: expected cylinders 1..=16383, heads 1..=16, sectors 1..=63 and 512-byte sectors");
    let error = ide_lower(&input.replace("cylinders=8", "cylinders=7")).unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::ImageGeometryMismatch { .. })
    ));
    assert_eq!(error.origin.line, Some(10));
    let error = ide_lower(&input.replace(
        "image='disk.img'",
        &format!("image='disk.img'\nsha256='{}'", "0".repeat(64)),
    ))
    .unwrap_err();
    assert_eq!(error.origin.line, Some(10));
    assert_eq!(error.field, "card[disk].drives[channel=master].image");
    assert!(error
        .message
        .contains(&format!("expected {}, actual", "0".repeat(64))));
}

#[test]
fn ide_lowering_orders_channels_and_reports_duplicate_positions() {
    let input = format!("{MINIMAL}provides=['fixed-disk']\n[[card]]\nid='disk'\nkind='ide'\n[[card.drives]]\nchannel='slave'\nimage='slave.img'\ngeometry={{cylinders=4,heads=2,sectors=4}}\n[[card.drives]]\nchannel='master'\nimage='disk.img'\ngeometry={{cylinders=8,heads=2,sectors=4}}\nwrite='readonly'\n");
    let config = ide_lower(&input).unwrap();
    assert_eq!(
        config
            .drives
            .iter()
            .map(|drive| drive.unit)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(config.drives[0].write, WriteMode::ReadOnly);
    assert_eq!(config.drives[1].write, WriteMode::Volatile);
    assert_eq!(
        config.drives[1].image,
        ImageSource::Inline(vec![0x55; 16384])
    );
    let error = ide_lower(&input.replace("channel='slave'", "channel='master'")).unwrap_err();
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::DuplicateDriveUnit { unit: 0, .. })
    ));
    assert!(error
        .message
        .contains("channel=master] (machines/ide-test.toml:10)"));
    assert!(error
        .message
        .contains("channel=master] (machines/ide-test.toml:14)"));
}

#[test]
fn catalogued_bios_cannot_claim_fixed_disk_services_in_a_file() {
    let Some(bytes) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let input = format!(
        "{MINIMAL}sha256='{}'\nprovides=['floppy','fixed-disk']\n[[card]]\nkind='ide'\n",
        assembly::KISELEV_XI8088.sha256
    );
    let file = Path::new("machines/catalogue-test.toml");
    let mut images = Images::default();
    images.files.insert("machines/bios.bin".into(), bytes);
    let error = lower(
        &parse_toml(file, &input).unwrap(),
        &Origins::from_toml(file, &input).unwrap(),
        &mut images,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "machines/catalogue-test.toml:4: firmware.bios.provides: firmware \"kiselev-xi8088-1.0.2\" declares provides [\"fixed-disk\", \"floppy\"]; catalogue provides [\"floppy\"]");
    assert!(matches!(
        error.validation.as_deref(),
        Some(ConfigError::FirmwareProvidesMismatch { image_index: 0, .. })
    ));
}

#[test]
fn catalogued_bios_file_uses_omitted_empty_or_equal_services() {
    let Some(bytes) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    for provides in ["", "provides=[]\n", "provides=['floppy']\n"] {
        let input = format!("{MINIMAL}sha256='{}'\n{provides}[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='floppy.img'\n", assembly::KISELEV_XI8088.sha256);
        let file = Path::new("machines/catalogue-test.toml");
        let mut images = Images::default();
        images
            .files
            .insert("machines/bios.bin".into(), bytes.clone());
        images
            .files
            .insert("machines/floppy.img".into(), vec![0; 368640]);
        let config = lower(
            &parse_toml(file, &input).unwrap(),
            &Origins::from_toml(file, &input).unwrap(),
            &mut images,
        )
        .unwrap();
        assert!(assembly::assemble(&config)
            .unwrap()
            .unlisted_firmware
            .is_empty());
    }
}

#[test]
fn board_mouse_is_opt_in_and_lowers_into_the_8042() {
    for (setting, expected) in [
        ("", false),
        ("mouse=false\n", false),
        ("mouse=true\n", true),
    ] {
        let input = MINIMAL.replace(
            "model = 'ibm-5170'\n",
            &format!("model = 'ibm-5170'\n{setting}"),
        );
        let config = run(&input).unwrap();
        let keyboard = config
            .devices
            .iter()
            .find(|d| d.id == DeviceId::Keyboard)
            .unwrap();
        assert!(matches!(&keyboard.kind, DeviceKind::Keyboard(cfg) if cfg.mouse == expected));
        assert_eq!(keyboard.irq, Some(1));
    }
}

#[test]
fn mouse_irq_conflict_names_the_mouse_field_and_its_origin() {
    let input = MINIMAL.replace("model = 'ibm-5170'\n", "model = 'ibm-5170'\nmouse = true\n")
        + "\n[[card]]\nid='serial'\nkind='uart-16550'\nirq=12\n";
    let Err(error) = run(&input) else {
        panic!("expected IRQ12 conflict")
    };
    assert_eq!(
        error.validation.as_deref(),
        Some(&ConfigError::IrqConflict {
            a: DeviceId::Keyboard,
            b: DeviceId::Uart,
            line: 12,
        })
    );
    assert_eq!(error.field, "card[serial]");
    assert!(error.message.contains("board.mouse"), "{error}");
    assert!(error.message.contains("custom.toml:4"), "{error}");
}

#[test]
fn serial_mouse_is_absent_by_default_and_none_explicitly_disables_it() {
    for setting in ["", "mouse='none'\n"] {
        let config = run(&format!("{MINIMAL}[[card]]\nkind='uart-16550'\n{setting}")).unwrap();
        assert!(config
            .devices
            .iter()
            .any(|d| matches!(&d.kind, DeviceKind::Uart(c) if !c.mouse)));
    }
}
