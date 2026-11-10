// SPDX-License-Identifier: AGPL-3.0-or-later
//! ATA PIO through assembled hardware and the real-mode CPU.
//!
//! ATA-2 §§6.2, 7 IDENTIFY DEVICE and Intel 80286 Programmer's Reference
//! Manual, INS/REP; see `backlog/docs/design/machine-config.md`
//! §The fixed-disk (IDE/HDD) attachment seam.

use assembly::*;

fn machine(base: u16, code: Vec<u8>) -> MachineConfig {
    let mut config = at_boot(code);
    let cfg = IdeCfg {
        base,
        control: base + 0x206,
    };
    config.devices.push(
        DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(cfg), cfg.port_windows())
            .with_irq(cfg.irq()),
    );
    for (unit, cylinders, fill, write) in [
        (0, 8, 0xAA, WriteMode::ReadOnly),
        (1, 4, 0x55, WriteMode::Volatile),
    ] {
        let geometry = HardDiskGeometry {
            cylinders,
            heads: 2,
            sectors_per_track: 4,
            bytes_per_sector: 512,
        };
        config.drives.push(DriveConfig {
            controller: DeviceId::Ide,
            unit,
            kind: DriveKind::HardDisk { geometry },
            image: ImageSource::Inline(vec![fill; geometry.image_bytes() as usize]),
            expected_sha256: None,
            write,
        });
    }
    config
}

#[test]
fn assembled_ide_identify_transfers_256_words_with_rep_insw() {
    for base in [0x1F0u16, 0x170] {
        // Clear ES, select master, issue IDENTIFY, then transfer all 256 words.
        let mut code = vec![0xFA, 0xFC, 0x31, 0xC0, 0x8E, 0xC0, 0xBA];
        code.extend_from_slice(&(base + 6).to_le_bytes());
        code.extend_from_slice(&[0xB0, 0xA0, 0xEE, 0x42, 0xB0, 0xEC, 0xEE, 0xBA]);
        code.extend_from_slice(&base.to_le_bytes());
        code.extend_from_slice(&[0xBF, 0x00, 0x20, 0xB9, 0x00, 0x01, 0xF3, 0x6D, 0xF4]);
        let mut assembled = assemble(&machine(base, code)).unwrap();
        assembled.machine.run_bounded(Some(400)).unwrap();
        let mut bytes = [0; 512];
        assembled
            .machine
            .cpu()
            .read_mem(0x2000, &mut bytes)
            .unwrap();
        let words: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(
            (words[0], words[1], words[3], words[6], words[60]),
            (0x40, 8, 2, 4, 64)
        );
        let model: Vec<_> = words[27..47]
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect();
        assert_eq!(
            String::from_utf8(model).unwrap(),
            "UNEMU ATA-2 PIO DISK                    "
        );
        assert_eq!(words[255], 0);
        assert_eq!(
            assembled.machine.bus_mut().port_read(base, 2).unwrap(),
            0xFFFF
        );
    }
}

#[test]
fn assembled_ide_binds_both_drives_and_enforces_readonly() {
    let mut assembled = assemble(&machine(0x1F0, vec![0xFA, 0xF4])).unwrap();
    let bus = assembled.machine.bus_mut();
    for (head, cylinders, fill) in [(0xE0, 8, 0xAA), (0xF0, 4, 0x55)] {
        bus.port_write(0x1F6, 1, head).unwrap();
        bus.port_write(0x1F7, 1, 0xEC).unwrap();
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0x40);
        assert_eq!(bus.port_read(0x1F0, 2).unwrap(), cylinders);
        for _ in 2..256 {
            bus.port_read(0x1F0, 2).unwrap();
        }
        bus.port_write(0x1F2, 1, 1).unwrap();
        for port in 0x1F3..=0x1F5 {
            bus.port_write(port, 1, 0).unwrap();
        }
        bus.port_write(0x1F7, 1, 0x30).unwrap();
        if head == 0xE0 {
            assert_eq!(bus.port_read(0x1F7, 1).unwrap(), 0x51);
            assert_eq!(bus.port_read(0x1F1, 1).unwrap(), 4);
        }
        for _ in 0..256 {
            bus.port_write(0x1F0, 2, 0x1234).unwrap();
        }
        bus.port_write(0x1F3, 1, 0).unwrap();
        bus.port_write(0x1F2, 1, 1).unwrap();
        bus.port_write(0x1F7, 1, 0x20).unwrap();
        let expected = if head == 0xE0 { fill * 0x101 } else { 0x1234 };
        for _ in 0..256 {
            assert_eq!(bus.port_read(0x1F0, 2).unwrap(), expected);
        }
    }
}

#[test]
fn assembled_ide_identify_interrupt_reaches_the_cascaded_pic() {
    for (base, vector, slave_mask) in [(0x1F0u16, 0x76u64, 0xBFu8), (0x170, 0x77, 0x7F)] {
        let mut code = vec![
            0xFA, 0xFC, 0x31, 0xC0, 0x8E, 0xD8, 0x8E, 0xC0, 0x8E, 0xD0, 0xBC, 0x00, 0x70,
        ];
        // Intel 8259A datasheet, ICW1–4: cascaded 8086 mode, slave vectors 70h–77h.
        for (port, value) in [
            (0x20, 0x11),
            (0xA0, 0x11),
            (0x21, 0x08),
            (0xA1, 0x70),
            (0x21, 0x04),
            (0xA1, 0x02),
            (0x21, 0x01),
            (0xA1, 0x01),
            (0x21, 0xFB),
            (0xA1, slave_mask),
        ] {
            code.extend_from_slice(&[0xB0, value, 0xE6, port]);
        }
        code.push(0xBA);
        code.extend_from_slice(&(base + 0x206).to_le_bytes());
        code.extend_from_slice(&[0xB0, 0, 0xEE, 0xBA]);
        code.extend_from_slice(&(base + 6).to_le_bytes());
        code.extend_from_slice(&[
            0xB0, 0xA0, 0xEE, 0x42, 0xFB, 0x90, 0xA0, 0x00, 0x30, 0xA2, 0x01, 0x30, 0xB0, 0xEC,
            0xEE, 0xBA,
        ]);
        code.extend_from_slice(&base.to_le_bytes());
        code.extend_from_slice(&[0xBF, 0x00, 0x20, 0xB9, 0x00, 0x01, 0xF3, 0x6D, 0xFA, 0xF4]);
        let mut config = machine(base, code);
        let mut handler = vec![0x50, 0x52, 0xFE, 0x06, 0x00, 0x30, 0xBA];
        handler.extend_from_slice(&(base + 7).to_le_bytes());
        handler.extend_from_slice(&[0xEC, 0xB0, 0x20, 0xE6, 0xA0, 0xE6, 0x20, 0x5A, 0x58, 0xCF]);
        config.images.push(LoadImage {
            base: vector * 4,
            source: ImageSource::Inline(vec![0x00, 0x10, 0, 0]),
            kind: ImageKind::RamLoad,
            provides: Vec::new(),
        });
        config.images.push(LoadImage {
            base: 0x1000,
            source: ImageSource::Inline(handler),
            kind: ImageKind::RamLoad,
            provides: Vec::new(),
        });
        let mut assembled = assemble(&config).unwrap();
        assembled.machine.run_bounded(Some(500)).unwrap();
        let mut markers = [0; 2];
        assembled
            .machine
            .cpu()
            .read_mem(0x3000, &mut markers)
            .unwrap();
        assert_eq!(
            markers,
            [1, 0],
            "IRQ vector {vector:#x} must run exactly once after IDENTIFY"
        );
        let mut identify = [0; 4];
        assembled
            .machine
            .cpu()
            .read_mem(0x2000, &mut identify)
            .unwrap();
        assert_eq!(identify, [0x40, 0, 8, 0]);
    }
}

#[test]
fn assembled_ide_byte_data_cycle_stops_with_unmapped_register() {
    for base in [0x1F0u16, 0x170] {
        let mut code = vec![0xFA, 0xBA];
        code.extend_from_slice(&base.to_le_bytes());
        code.extend_from_slice(&[0xEC, 0xF4]);
        let mut assembled = assemble(&machine(base, code)).unwrap();
        assert!(
            matches!(assembled.machine.run_bounded(Some(10)), Err(machine::MachineError::Bus(bus::BusError::Device(device::DeviceError::UnmappedRegister { kind: "port", offset }))) if offset == u64::from(base))
        );
    }
}

#[test]
fn assembled_ide_host_reaches_and_flushes_a_session_end_write() {
    use device::Device;

    let mut code = vec![0xFA];
    for (offset, value) in [(6u16, 0xE0u8), (2, 3), (3, 2), (4, 0), (5, 0), (7, 0x30)] {
        code.push(0xBA);
        code.extend_from_slice(&(0x1F0 + offset).to_le_bytes());
        code.extend_from_slice(&[0xB0, value, 0xEE]);
    }
    code.extend_from_slice(&[
        0xBA, 0xF0, 0x01, 0xB8, 0x55, 0x55, 0xB9, 0x00, 0x01, 0xEF, 0xE2, 0xFD, 0xB8, 0x34, 0x12,
        0xEF, 0xF4,
    ]);
    let mut config = machine(0x1F0, code);
    config.drives[0].write = WriteMode::Volatile;
    let mut assembled = assemble(&config).unwrap();
    assembled.machine.run_bounded(Some(1000)).unwrap();
    let index = assembled.device_index[&DeviceId::Ide];
    let ata = assembled
        .machine
        .bus_mut()
        .device_mut(index)
        .unwrap()
        .as_any_mut()
        .expect("ATA must expose its host-side downcast")
        .downcast_mut::<devices::Ata>()
        .expect("IDE must assemble as ATA");
    assert_eq!(ata.port_read(0x3F6, 1).unwrap(), 0x58);
    let before = ata.save();
    ata.flush_pending();
    assert_eq!(ata.save(), before);
    let image = ata.drive(0).unwrap().image();
    assert_eq!(&image[2 * 512..3 * 512], &[0x55; 512]);
    assert_eq!(&image[3 * 512..4 * 512], &[0xAA; 512]);
    ata.flush_pending();
    assert_eq!(ata.save(), before);
}
