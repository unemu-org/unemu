// SPDX-License-Identifier: AGPL-3.0-or-later
//! File-backed assembly regression tests; persistent-media.md § Integration.
use super::*;
use crate::FileMachineExt;
use assembly::*;
use assembly::{FloppyMediaList, FloppyMedium, ImageSource, WriteMode};

#[test]
fn append_floppy_verifies_pins_and_paths_before_changing_the_catalogue() {
    use sha2::{Digest, Sha256};
    let temp = assembly::test_support::TempDir::new("append-floppy");
    let path = temp.path().join("extra.img");
    let base = vec![0x33; 1_474_560];
    std::fs::write(&path, &base).unwrap();
    let mut config = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; base.len()]);
    let mut assembled = crate::assemble(&config).unwrap();
    let mut medium = FloppyMedium {
        image: ImageSource::File(path.clone()),
        expected_sha256: Some([0; 32]),
        write: WriteMode::ReadOnly,
        name: "extra.img".into(),
    };
    let original = assembled.floppy_name(DeviceId::Fdc, 0);
    assert!(assembled
        .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
        .unwrap_err()
        .to_string()
        .contains("SHA-256"));
    assert_eq!(assembled.floppy_names[&(DeviceId::Fdc, 0)].len(), 1);
    medium.expected_sha256 = Some(Sha256::digest(&base).into());
    medium.write = WriteMode::journal(&path);
    assert!(assembled
        .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
        .unwrap_err()
        .to_string()
        .contains("aliases"));
    medium.write = WriteMode::ReadOnly;
    assert_eq!(
        assembled
            .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
            .unwrap(),
        1
    );
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), original);
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(1))
        .unwrap();
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "extra.img");
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
        .unwrap();
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Next)
        .unwrap();
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), original);
    assert_eq!(std::fs::read(path).unwrap(), base);
    assembled.finish_persistence().unwrap();
}

#[test]
fn append_floppy_takes_any_medium_the_drive_type_takes() {
    let temp = assembly::test_support::TempDir::new("append-mixed-density");
    let mut config = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; 1_474_560]);
    let mut assembled = crate::assemble(&config).unwrap();
    let medium = |name: &str, bytes: Vec<u8>| {
        let path = temp.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        FloppyMedium {
            image: ImageSource::File(path),
            expected_sha256: None,
            write: WriteMode::ReadOnly,
            name: name.into(),
        }
    };
    let refused = assembled
        .append_floppy(
            DeviceId::Fdc,
            0,
            &mut config,
            &medium("hd1200.img", vec![0x12; 1_228_800]),
        )
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("a 1.44M drive takes 1.44M (1474560 bytes) or 720K (737280 bytes) media"),
        "{refused}"
    );
    let mut sector = vec![0x72; 737_280];
    sector[512..1024].fill(0x27);
    let index = assembled
        .append_floppy(DeviceId::Fdc, 0, &mut config, &medium("dd720.img", sector))
        .unwrap();
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(index))
        .unwrap();
    // The 720K medium answers at 250 kbps with its own nine-sector layout.
    let fdc_index = assembled.device_index[&DeviceId::Fdc];
    let fdc = assembled.machine.bus_mut().device_mut(fdc_index).unwrap();
    fdc.port_write(0x3f2, 1, 0x1c).unwrap();
    for rate in [0, 2] {
        fdc.port_write(0x3f7, 1, rate).unwrap();
        for byte in [6, 0, 0, 0, 2, 2, 2, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        let request = fdc.take_dma_request();
        if rate == 0 {
            assert!(request.is_none());
        } else {
            let device::DmaOp::ToMemory(bytes) = request.unwrap().op else {
                panic!("READ DATA");
            };
            assert_eq!(bytes, vec![0x27; 512]);
            fdc.dma_complete(device::DmaOutcome::ToMemory {
                transferred: 512,
                terminal_count: true,
            });
        }
        let result: Vec<u32> = (0..7).map(|_| fdc.port_read(0x3f5, 1).unwrap()).collect();
        assert_eq!(result[1], if rate == 0 { 1 } else { 0 });
    }
    assembled.finish_persistence().unwrap();
}

#[test]
fn appended_journal_keeps_guest_writes_separate_and_reopens_the_overlay() {
    let temp = assembly::test_support::TempDir::new("append-journal");
    let path = temp.path().join("extra.img");
    let journal = temp.path().join("extra.upfj");
    let base = vec![0x33; 1_474_560];
    std::fs::write(&path, &base).unwrap();
    let mut config = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; base.len()]);
    let mut assembled = crate::assemble(&config).unwrap();
    let medium = FloppyMedium {
        image: ImageSource::File(path.clone()),
        expected_sha256: None,
        write: WriteMode::journal(&journal),
        name: "extra.img".into(),
    };
    let index = assembled
        .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
        .unwrap();
    assert!(!journal.exists());
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(index))
        .unwrap();
    let fdc_index = assembled.device_index[&DeviceId::Fdc];
    let fdc = assembled.machine.bus_mut().device_mut(fdc_index).unwrap();
    fdc.port_write(0x3f2, 1, 0x1c).unwrap();
    for byte in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
        fdc.port_write(0x3f5, 1, byte).unwrap();
    }
    assert!(fdc.take_dma_request().is_some());
    fdc.dma_complete(device::DmaOutcome::FromMemory {
        bytes: vec![0x77; 512],
        terminal_count: true,
    });
    for _ in 0..7 {
        fdc.port_read(0x3f5, 1).unwrap();
    }
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), base);
    let scan = journal::scan(&std::fs::read(&journal).unwrap(), &base, base.len() as u64);
    assert_eq!(&scan.image.unwrap()[..512], &[0x77; 512]);
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(index))
        .unwrap();
    let fdc = assembled.machine.bus_mut().device_mut(fdc_index).unwrap();
    for byte in [6, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
        fdc.port_write(0x3f5, 1, byte).unwrap();
    }
    let device::DmaOp::ToMemory(bytes) = fdc.take_dma_request().unwrap().op else {
        panic!("READ DATA");
    };
    assert_eq!(bytes, vec![0x77; 512]);
    fdc.dma_complete(device::DmaOutcome::ToMemory {
        transferred: 512,
        terminal_count: true,
    });
    assembled.finish_persistence().unwrap();
}

#[test]
fn floppy_media_schedule_is_ordered_and_applies_only_at_the_requested_boundary() {
    let mut config = assembly::floppy_boot(vec![0xfa, 0xeb, 0xfe], vec![0; 1_474_560]);
    config.drives[0].media = Some(FloppyMediaList {
        empty: false,
        entries: [0x11, 0x22]
            .into_iter()
            .map(|byte| FloppyMedium {
                image: ImageSource::Inline(vec![byte; 1_474_560]),
                expected_sha256: None,
                write: WriteMode::ReadOnly,
                name: format!("{byte}.img"),
            })
            .collect(),
        inserted: 0,
    });
    let mut assembled = crate::assemble(&config).unwrap();
    let event = FloppyEvent {
        at: 20,
        controller: DeviceId::Fdc,
        unit: 0,
        action: FloppyAction::Next,
    };
    assembled.run_floppy_schedule(19, None, &[]).unwrap();
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "17.img");
    assembled.run_floppy_schedule(20, None, &[event]).unwrap();
    assert_eq!(assembled.machine.virtual_time(), 20);
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "34.img");
    assert_eq!(
        assembled.machine.bus_mut().port_read(0x3f7, 1).unwrap(),
        0x80
    );
    assert!(assembled
        .run_floppy_schedule(30, None, &[event, FloppyEvent { at: 19, ..event }])
        .is_err());
    assert_eq!(assembled.machine.virtual_time(), 20);
    assembled
        .run_floppy_schedule(
            20,
            None,
            &[
                FloppyEvent {
                    action: FloppyAction::Eject,
                    ..event
                },
                FloppyEvent {
                    action: FloppyAction::Next,
                    ..event
                },
            ],
        )
        .unwrap();
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "17.img");
    assembled.finish_persistence().unwrap();
}
/// A catalogue may start with any entry in the drive: the guest sees it
/// from power-on with the change line clear, its journal opens as the
/// first entry's would, and an entry past the catalogue is refused.
#[test]
fn a_catalogue_can_start_with_any_entry_in_the_drive() {
    let temp = assembly::test_support::TempDir::new("floppy-power-on");
    let base_path = temp.path().join("base.img");
    let journal_path = temp.path().join("second.upfj");
    std::fs::write(&base_path, vec![0x22; 1_474_560]).unwrap();
    let mut config = assembly::floppy_boot(vec![0xfa, 0xeb, 0xfe], vec![0; 1_474_560]);
    let list = |inserted| FloppyMediaList {
        empty: false,
        entries: vec![
            FloppyMedium {
                image: ImageSource::Inline(vec![0x11; 1_474_560]),
                expected_sha256: None,
                write: WriteMode::ReadOnly,
                name: "first.img".into(),
            },
            FloppyMedium {
                image: ImageSource::File(base_path.clone()),
                expected_sha256: None,
                write: WriteMode::journal(&journal_path),
                name: "second.img".into(),
            },
        ],
        inserted,
    };
    config.drives[0].media = Some(list(2));
    assert!(crate::assemble(&config).is_err(), "no third entry");
    config.drives[0].media = Some(list(1));
    let mut assembled = crate::assemble(&config).unwrap();
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "second.img");
    assert!(journal_path.exists(), "the disk in the drive is open");
    assert_eq!(assembled.machine.bus_mut().port_read(0x3f7, 1).unwrap(), 0);
    assembled.finish_persistence().unwrap();
}

#[test]
fn floppy_media_runtime_eject_releases_journal_and_reinsertion_replays_writes() {
    let temp = assembly::test_support::TempDir::new("floppy-runtime");
    let base_path = temp.path().join("base.img");
    let journal_path = temp.path().join("disk.upfj");
    let base = vec![0x11; 1_474_560];
    std::fs::write(&base_path, &base).unwrap();
    let mut config = assembly::floppy_boot(vec![0xfa, 0xeb, 0xfe], base.clone());
    config.drives[0].media = Some(FloppyMediaList {
        empty: false,
        entries: vec![
            FloppyMedium {
                image: ImageSource::Inline(base.clone()),
                expected_sha256: None,
                write: WriteMode::ReadOnly,
                name: "first.img".into(),
            },
            FloppyMedium {
                image: ImageSource::File(base_path.clone()),
                expected_sha256: None,
                write: WriteMode::journal(&journal_path),
                name: "second.img".into(),
            },
        ],
        inserted: 0,
    });
    let mut assembled = crate::assemble(&config).unwrap();
    assert!(!journal_path.exists());
    let index = assembled.device_index[&DeviceId::Fdc];
    let boot_snapshot = assembled.machine.bus().device(index).unwrap().save();
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Next)
        .unwrap();
    assert!(journal_path.exists());
    let fdc = assembled.machine.bus_mut().device_mut(index).unwrap();
    let journal_snapshot = fdc.save();
    fdc.restore(&journal_snapshot).unwrap();
    let error = fdc.restore(&boot_snapshot).unwrap_err();
    assert!(error.to_string().contains("journaled medium"));
    assert_eq!(fdc.save(), journal_snapshot);
    assert_eq!(
        std::fs::read(&journal_path).unwrap().len(),
        journal::HEADER_LEN
    );
    fdc.port_write(0x3f2, 1, 0x1c).unwrap();
    for byte in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
        fdc.port_write(0x3f5, 1, byte).unwrap();
    }
    assert!(fdc.take_dma_request().is_some());
    fdc.dma_complete(device::DmaOutcome::FromMemory {
        bytes: vec![0x77; 512],
        terminal_count: true,
    });
    for _ in 0..7 {
        fdc.port_read(0x3f5, 1).unwrap();
    }
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
        .unwrap();
    assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "empty");
    let scan = journal::scan(
        &std::fs::read(&journal_path).unwrap(),
        &base,
        base.len() as u64,
    );
    assert_eq!(scan.report.generations.len(), 1);
    assert_eq!(&scan.image.unwrap()[..512], &[0x77; 512]);
    assembled.run_floppy_schedule(10, None, &[]).unwrap();
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(1))
        .unwrap();
    let fdc = assembled.machine.bus_mut().device_mut(index).unwrap();
    for byte in [6, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
        fdc.port_write(0x3f5, 1, byte).unwrap();
    }
    let device::DmaOp::ToMemory(bytes) = fdc.take_dma_request().unwrap().op else {
        panic!("READ DATA");
    };
    assert_eq!(bytes, vec![0x77; 512]);
    fdc.dma_complete(device::DmaOutcome::ToMemory {
        transferred: 512,
        terminal_count: true,
    });
    assembled.finish_persistence().unwrap();
    let scan = journal::scan(
        &std::fs::read(&journal_path).unwrap(),
        &base,
        base.len() as u64,
    );
    assert_eq!(scan.report.generations.len(), 2);
    assert_eq!(std::fs::read(base_path).unwrap(), base);
}

#[test]
fn floppy_media_eject_keeps_in_flight_ide_sectors_buffered_until_finish() {
    use assembly::{
        AtaControllerModel, DeviceConfig, DeviceKind, DriveConfig, DriveKind, HardDiskGeometry,
        IdeCfg,
    };
    let temp = assembly::test_support::TempDir::new("floppy-ide-boundary");
    let floppy_base = vec![0; assembly::FloppyGeometry::DD_360K.image_bytes() as usize];
    let ide_base = vec![0x11; 32768];
    let floppy_image = temp.path().join("floppy.img");
    let ide_image = temp.path().join("ide.img");
    let floppy_journal = temp.path().join("floppy.upfj");
    let ide_journal = temp.path().join("ide.upfj");
    std::fs::write(&floppy_image, &floppy_base).unwrap();
    std::fs::write(&ide_image, &ide_base).unwrap();
    let mut config = assembly::at_fdc_write(vec![0xfa, 0xf4], vec![0; 512], 0x600);
    config.drives[0].image = ImageSource::File(floppy_image);
    config.drives[0].write = WriteMode::journal(&floppy_journal);
    let ide = IdeCfg {
        model: AtaControllerModel::Ata,
        base: 0x170,
        control: 0x376,
    };
    config.devices.push(
        DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(ide), ide.port_windows())
            .with_irq(ide.irq()),
    );
    config.drives.push(DriveConfig {
        media: None,
        controller: DeviceId::Ide,
        unit: 0,
        kind: DriveKind::HardDisk {
            geometry: HardDiskGeometry {
                cylinders: 8,
                heads: 2,
                sectors_per_track: 4,
                bytes_per_sector: 512,
            },
        },
        image: ImageSource::File(ide_image.clone()),
        expected_sha256: None,
        write: WriteMode::journal(&ide_journal),
    });
    let mut assembled = crate::assemble(&config).unwrap();
    let bus = assembled.machine.bus_mut();
    for (port, value) in [
        (0x176, 0xe0),
        (0x172, 2),
        (0x173, 0),
        (0x174, 0),
        (0x175, 0),
        (0x177, 0x30),
    ] {
        bus.port_write(port, 1, value).unwrap();
    }
    for _ in 0..256 {
        bus.port_write(0x170, 2, 0x1234).unwrap();
    }
    for _ in 0..17 {
        bus.port_write(0x170, 2, 0x5678).unwrap();
    }
    let ide_index = assembled.device_index[&DeviceId::Ide];
    let state = assembled.machine.bus().device(ide_index).unwrap().save();
    let before = std::fs::read(&ide_journal).unwrap();
    assert_eq!(before.len(), journal::HEADER_LEN);
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
        .unwrap();
    assert_eq!(std::fs::read(&ide_journal).unwrap(), before);
    assert_eq!(
        assembled.machine.bus().device(ide_index).unwrap().save(),
        state
    );
    let floppy = journal::scan(
        &std::fs::read(&floppy_journal).unwrap(),
        &floppy_base,
        floppy_base.len() as u64,
    );
    assert_eq!(floppy.report.generations.len(), 1);
    assert_eq!(
        floppy.report.generations[0].reason,
        journal::ClosingReason::SessionEnd
    );
    assert_eq!(assembled.machine.virtual_time(), 0);
    assembled.finish_persistence().unwrap();
    let ide = journal::scan(
        &std::fs::read(&ide_journal).unwrap(),
        &ide_base,
        ide_base.len() as u64,
    );
    assert_eq!(ide.report.generations.len(), 1);
    let mut expected = ide_base.clone();
    expected[..512].copy_from_slice(&0x1234u16.to_le_bytes().repeat(256));
    assert_eq!(ide.image.unwrap(), expected);
    assert_eq!(std::fs::read(ide_image).unwrap(), ide_base);
}
