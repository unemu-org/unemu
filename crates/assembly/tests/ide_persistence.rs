// SPDX-License-Identifier: AGPL-3.0-or-later
//! ATA-2 §7 WRITE SECTORS / WRITE MULTIPLE through assembled journal media.
//! See persistent-media.md §Integration for whole-sector boundary flushing.

use assembly::*;
use cpu::Cpu;
use device::Device;
use media::offline::{Access, JournalTool};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Disk {
    root: PathBuf,
    base: Vec<u8>,
}
impl Disk {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ide-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let base: Vec<_> = (0..32768).map(|i| (i ^ (i >> 9)) as u8).collect();
        std::fs::write(root.join("base.img"), &base).unwrap();
        Self { root, base }
    }
    fn config(&self) -> MachineConfig {
        let mut config = at_boot(vec![0xFA, 0xF4]);
        let cfg = IdeCfg {
            model: AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        config.devices.push(
            DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(cfg), cfg.port_windows())
                .with_irq(cfg.irq()),
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
            image: ImageSource::File(self.root.join("base.img")),
            expected_sha256: None,
            write: WriteMode::journal(self.root.join("disk.upfj")),
        });
        config
    }
    fn scan(&self) -> media::Scan {
        media::scan(
            &std::fs::read(self.root.join("disk.upfj")).unwrap(),
            &self.base,
            self.base.len() as u64,
        )
    }
    fn tool(&self) -> JournalTool {
        JournalTool::open(
            &self.root.join("base.img"),
            &self.root.join("disk.upfj"),
            Access::Mutate,
        )
        .unwrap()
    }
}
impl Drop for Disk {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn ata(machine: &mut AssembledMachine) -> &mut devices::Ata {
    let index = machine.device_index[&DeviceId::Ide];
    machine
        .machine
        .bus_mut()
        .device_mut(index)
        .unwrap()
        .as_any_mut()
        .unwrap()
        .downcast_mut()
        .unwrap()
}
fn command(machine: &mut AssembledMachine, lba: u8, count: u8, opcode: u8) {
    let bus = machine.machine.bus_mut();
    for (port, byte) in [
        (0x1F6, 0xE0),
        (0x1F2, count),
        (0x1F3, lba),
        (0x1F4, 0),
        (0x1F5, 0),
        (0x1F7, opcode),
    ] {
        bus.port_write(port, 1, byte.into()).unwrap();
    }
}
fn words(machine: &mut AssembledMachine, value: u16, count: usize) {
    for _ in 0..count {
        machine
            .machine
            .bus_mut()
            .port_write(0x1F0, 2, value.into())
            .unwrap();
    }
}

#[test]
fn ide_journal_records_single_and_multiple_commands_and_replays_every_byte() {
    for opcode in [0x30, 0x31, 0xC5] {
        let disk = Disk::new();
        let mut machine = assemble(&disk.config()).unwrap();
        assert_eq!(machine.persistence.len(), 1);
        assert_eq!(machine.persistence[0].keep_generations, 10);
        if opcode == 0xC5 {
            command(&mut machine, 0, 2, 0xC6);
        }
        command(&mut machine, 3, 1, opcode);
        words(&mut machine, 0x1234, 256);
        command(&mut machine, 5, 3, opcode);
        for value in [0xABCD, 0x5678, 0x9ABC] {
            words(&mut machine, value, 256);
        }
        let scan = disk.scan();
        assert_eq!(scan.report.unsealed_tip.unwrap().commands, 2);
        let mut expected = disk.base.clone();
        for (lba, value) in [(3, 0x1234u16), (5, 0xABCD), (6, 0x5678), (7, 0x9ABC)] {
            expected[lba * 512..(lba + 1) * 512].copy_from_slice(&value.to_le_bytes().repeat(256));
        }
        assert_eq!(scan.image.unwrap(), expected);
        machine.finish_persistence().unwrap();
        drop(machine);
        let mut fresh = assemble(&disk.config()).unwrap();
        assert_eq!(ata(&mut fresh).drive(0).unwrap().image(), expected);
        fresh.finish_persistence().unwrap();
        assert_eq!(
            std::fs::read(disk.root.join("base.img")).unwrap(),
            disk.base
        );
    }
}

#[test]
fn ide_journal_interrupted_command_flushes_only_at_explicit_boundaries() {
    for boundary in ["run", "finish", "snapshot", "reset"] {
        let disk = Disk::new();
        let mut machine = assemble(&disk.config()).unwrap();
        command(&mut machine, 4, 3, 0x30);
        words(&mut machine, 0xA55A, 256);
        words(&mut machine, 0xFFFF, 17);
        assert!(disk.scan().report.unsealed_tip.is_none());
        let state = ata(&mut machine).save();
        match boundary {
            "finish" => machine.finish_persistence().unwrap(),
            "snapshot" => machine.flush_persistence().unwrap(),
            "reset" => machine.before_cpu_reset().unwrap(),
            _ => {
                machine
                    .run_session(&RunPolicy::natural(Some(10), None))
                    .unwrap();
            }
        }
        let after = ata(&mut machine).save();
        // Running advances the trace clock, but must preserve all PIO state.
        assert_eq!(&after[..8], &state[..8]);
        assert_eq!(&after[16..], &state[16..]);
        let scan = disk.scan();
        if boundary == "run" {
            assert!(scan.report.unsealed_tip.is_none());
            assert_eq!(scan.image.unwrap(), disk.base);
            machine.finish_persistence().unwrap();
            continue;
        }
        assert_eq!(
            &scan.image.as_ref().unwrap()[4 * 512..5 * 512],
            &0xA55Au16.to_le_bytes().repeat(256)
        );
        assert_eq!(
            &scan.image.as_ref().unwrap()[5 * 512..],
            &disk.base[5 * 512..]
        );
        if boundary != "finish" {
            assert_eq!(scan.report.unsealed_tip.unwrap().commands, 1);
        }
        drop(machine);
        let mut fresh = assemble(&disk.config()).unwrap();
        assert_eq!(
            ata(&mut fresh).drive(0).unwrap().image(),
            scan.image.unwrap()
        );
        fresh.finish_persistence().unwrap();
    }
}

#[test]
fn ide_journal_explicit_snapshot_flush_preserves_partial_buffer_and_command_continuation() {
    let disk = Disk::new();
    let mut machine = assemble(&disk.config()).unwrap();
    command(&mut machine, 4, 2, 0x30);
    words(&mut machine, 0x1234, 256);
    words(&mut machine, 0xABCD, 17);
    let before = ata(&mut machine).save();
    machine.flush_persistence().unwrap();
    assert_eq!(ata(&mut machine).save(), before);
    let journal = std::fs::read(disk.root.join("disk.upfj")).unwrap();
    machine.flush_persistence().unwrap();
    assert_eq!(std::fs::read(disk.root.join("disk.upfj")).unwrap(), journal);
    words(&mut machine, 0x5678, 239);
    let mut expected = disk.base.clone();
    expected[4 * 512..5 * 512].copy_from_slice(&0x1234u16.to_le_bytes().repeat(256));
    expected[5 * 512..5 * 512 + 34].copy_from_slice(&0xABCDu16.to_le_bytes().repeat(17));
    expected[5 * 512 + 34..6 * 512].copy_from_slice(&0x5678u16.to_le_bytes().repeat(239));
    assert_eq!(disk.scan().report.unsealed_tip.unwrap().commands, 2);
    machine.finish_persistence().unwrap();
    drop(machine);
    let mut fresh = assemble(&disk.config()).unwrap();
    assert_eq!(ata(&mut fresh).drive(0).unwrap().image(), expected);
    fresh.finish_persistence().unwrap();
}

#[test]
fn ide_journal_execution_error_leaves_completed_sectors_recoverable() {
    let disk = Disk::new();
    let mut config = disk.config();
    config.images[0].source = ImageSource::Inline(vec![0xEA, 0xFF, 0xFF, 0xFF, 0xFF]);
    let mut machine = assemble(&config).unwrap();
    command(&mut machine, 4, 2, 0x30);
    words(&mut machine, 0x1234, 256);
    words(&mut machine, 0xABCD, 17);
    assert!(machine
        .run_session(&RunPolicy::natural(Some(10), None))
        .is_err());
    let scan = disk.scan();
    assert!(scan.report.generations.is_empty());
    assert_eq!(scan.report.unsealed_tip.unwrap().commands, 1);
    let mut expected = disk.base.clone();
    expected[4 * 512..5 * 512].copy_from_slice(&0x1234u16.to_le_bytes().repeat(256));
    assert_eq!(scan.image.unwrap(), expected);
    drop(machine);
    let mut fresh = assemble(&disk.config()).unwrap();
    assert_eq!(ata(&mut fresh).drive(0).unwrap().image(), expected);
    assert_eq!(
        disk.scan().report.generations[0].reason,
        media::ClosingReason::Recovered
    );
    fresh.finish_persistence().unwrap();
}

fn pio_workload(disk: &Disk, fault: bool) -> MachineConfig {
    let mut config = disk.config();
    let mut code = vec![0xFA, 0xFC];
    for (port, value) in [
        (0x1F6u16, 0xE0),
        (0x1F2, 3),
        (0x1F3, 4),
        (0x1F4, 0),
        (0x1F5, 0),
        (0x1F7, 0x30),
    ] {
        code.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
    }
    // REP OUTSW crosses sector boundaries without any host-side port writes.
    code.extend([0xBE, 0, 0x20, 0xBA, 0xF0, 1, 0xB9, 0x80, 1, 0xF3, 0x6F]);
    if fault {
        // The ATA data register rejects a byte cycle, producing a run error
        // after exactly one whole sector and half of the next.
        code.push(0xEC);
    } else {
        code.extend([0xBA, 0xF7, 1, 0xEC, 0xA2, 0, 0x30]);
        code.extend([0xBA, 0xF0, 1, 0xB9, 0x80, 1, 0xF3, 0x6F, 0xF4]);
    }
    config.images[0].source = ImageSource::Inline(code);
    config.images.push(LoadImage {
        base: 0x2000,
        kind: ImageKind::RamLoad,
        provides: vec![],
        source: ImageSource::Inline((0..1536).map(|i| (i ^ (i >> 8)) as u8).collect()),
    });
    config
}

#[test]
fn ide_journal_guest_command_grouping_is_independent_of_host_cap_splits() {
    let mut journals = Vec::new();
    for step in [1, 17, 257, 10_000] {
        let disk = Disk::new();
        let mut machine = assemble(&pio_workload(&disk, false)).unwrap();
        loop {
            let cap = machine.machine.virtual_time() + step;
            let outcome = machine
                .run_session(&RunPolicy::natural(Some(cap), None))
                .unwrap();
            machine.check_persistence().unwrap();
            if outcome == machine::RunOutcome::Halted {
                break;
            }
            assert!(cap < 10_000);
        }
        machine.finish_persistence().unwrap();
        journals.push(std::fs::read(disk.root.join("disk.upfj")).unwrap());
    }
    assert!(
        journals.windows(2).all(|pair| pair[0] == pair[1]),
        "the same guest workload must produce byte-identical journals for all host cap splits"
    );
}

#[test]
fn ide_journal_guest_outsw_error_preserves_exactly_one_completed_sector() {
    let disk = Disk::new();
    let mut machine = assemble(&pio_workload(&disk, true)).unwrap();
    let result = machine.run_session(&RunPolicy::natural(Some(10_000), None));
    assert!(machine.machine.cpu().retired() > 0);
    assert!(matches!(
        result,
        Err(RunError::Machine(machine::MachineError::Bus(
            bus::BusError::Device(device::DeviceError::UnmappedRegister { offset: 0x1F0, .. })
        )))
    ));
    let scan = disk.scan();
    assert!(scan.report.generations.is_empty());
    assert_eq!(scan.report.unsealed_tip.unwrap().commands, 1);
    let mut expected = disk.base.clone();
    expected[4 * 512..5 * 512]
        .copy_from_slice(&(0..512).map(|i| (i ^ (i >> 8)) as u8).collect::<Vec<_>>());
    assert_eq!(scan.image.unwrap(), expected);
    drop(machine);
    let mut fresh = assemble(&disk.config()).unwrap();
    assert_eq!(ata(&mut fresh).drive(0).unwrap().image(), expected);
    fresh.finish_persistence().unwrap();
}

/// ATA-2 §8.4 WRITE SECTORS and IBM PC AT Technical Reference, Keyboard
/// Controller pulse commands: a CPU-only reset must persist whole sectors.
#[test]
fn ide_journal_guest_cpu_reset_preserves_exactly_one_completed_sector() {
    let disk = Disk::new();
    let mut config = pio_workload(&disk, true);
    let ImageSource::Inline(code) = &mut config.images[0].source else {
        panic!("PIO workload must be inline guest code");
    };
    assert_eq!(code.pop(), Some(0xEC));
    // Replace the fault after 1.5 sectors with an 8042 reset request. The
    // following marker must remain untouched when execution restarts in ROM.
    code.extend([0xB0, 0xFE, 0xE6, 0x64, 0xC6, 0x06, 0, 0x30, 0xEE, 0xF4]);
    config.memory.address_space_bytes = 0x100000;
    config.images.push(LoadImage {
        base: 0xFFFF0,
        kind: ImageKind::Rom,
        provides: vec![],
        source: ImageSource::Inline(vec![0xB0, 0x66, 0xA2, 1, 0x30, 0xF4]),
    });
    let mut machine = assemble(&config).unwrap();
    assert_eq!(
        machine
            .run_session(&RunPolicy::natural(Some(10_000), None))
            .unwrap(),
        machine::RunOutcome::Halted
    );
    assert_eq!(machine.machine.cpu().code_segment(), 0xF000);
    let mut markers = [0; 2];
    machine
        .machine
        .cpu()
        .read_mem(0x3000, &mut markers)
        .unwrap();
    assert_eq!(markers, [0, 0x66]);

    // An explicit flush or finish here would mask a missing reset flush.
    assert_eq!(
        std::fs::metadata(disk.root.join("disk.upfj"))
            .unwrap()
            .len(),
        (media::HEADER_LEN + 2 * media::FRAME_LEN) as u64,
        "reset must append one sector and its command end, without sealing the session"
    );
    let scan = disk.scan();
    assert!(scan.report.generations.is_empty());
    assert_eq!(scan.report.unsealed_tip.unwrap().commands, 1);
    let mut expected = disk.base.clone();
    expected[4 * 512..5 * 512]
        .copy_from_slice(&(0..512).map(|i| (i ^ (i >> 8)) as u8).collect::<Vec<_>>());
    assert_eq!(scan.image.unwrap(), expected);
    drop(machine);

    let mut fresh = assemble(&disk.config()).unwrap();
    command(&mut fresh, 4, 3, 0x20);
    let mut replayed = Vec::new();
    for _ in 0..3 * 256 {
        let word = fresh.machine.bus_mut().port_read(0x1F0, 2).unwrap() as u16;
        replayed.extend(word.to_le_bytes());
    }
    assert_eq!(replayed, expected[4 * 512..7 * 512]);
    assert_eq!(fresh.machine.bus_mut().port_read(0x1F7, 1).unwrap(), 0x50);
    fresh.finish_persistence().unwrap();
    assert_eq!(
        std::fs::read(disk.root.join("base.img")).unwrap(),
        disk.base
    );
}

fn direct_handle_close(checkpoint: bool) {
    let disk = Disk::new();
    let mut machine = assemble(&disk.config()).unwrap();
    command(&mut machine, 4, 2, 0x30);
    words(&mut machine, 0x1234, 256);
    words(&mut machine, 0xABCD, 128);
    if checkpoint {
        machine.persistence[0]
            .checkpoint(&mut machine.machine)
            .unwrap();
    } else {
        machine.persistence[0].finish(&mut machine.machine).unwrap();
    }
    let scan = disk.scan();
    assert_eq!(scan.report.generations[0].commands, 1);
    let mut expected = disk.base.clone();
    expected[4 * 512..5 * 512].copy_from_slice(&0x1234u16.to_le_bytes().repeat(256));
    assert_eq!(scan.image.unwrap(), expected);
}

#[test]
fn ide_journal_direct_handle_finish_flushes_the_completed_prefix() {
    direct_handle_close(false);
}

#[test]
fn ide_journal_direct_handle_checkpoint_flushes_the_completed_prefix() {
    direct_handle_close(true);
}

#[test]
fn ide_journal_retention_keeps_a_replayable_anchor() {
    let disk = Disk::new();
    let mut config = disk.config();
    config.drives[0].write = WriteMode::Journal {
        path: disk.root.join("disk.upfj"),
        keep_generations: 1,
    };
    for value in [0x1234, 0x5678] {
        let mut machine = assemble(&config).unwrap();
        command(&mut machine, 63, 1, 0x30);
        words(&mut machine, value, 256);
        machine.finish_persistence().unwrap();
    }
    let scan = disk.scan();
    assert_eq!(scan.report.header.unwrap().anchor_generation, 2);
    assert_eq!(scan.report.generations.len(), 1);
    let mut expected = disk.base.clone();
    expected[63 * 512..].copy_from_slice(&0x5678u16.to_le_bytes().repeat(256));
    assert_eq!(scan.image.unwrap(), expected);
    let mut fresh = assemble(&config).unwrap();
    assert_eq!(ata(&mut fresh).drive(0).unwrap().image(), expected);
    fresh.finish_persistence().unwrap();
}

#[test]
fn ide_journal_rollback_and_commit_use_image_capacity() {
    let disk = Disk::new();
    for value in [0x1234, 0x5678] {
        let mut machine = assemble(&disk.config()).unwrap();
        command(&mut machine, 63, 1, 0x30);
        words(&mut machine, value, 256);
        machine.finish_persistence().unwrap();
    }
    let mut expected = disk.base.clone();
    expected[63 * 512..].copy_from_slice(&0x1234u16.to_le_bytes().repeat(256));
    let mut tool = disk.tool();
    assert_eq!(tool.verify().unwrap().report.generations.len(), 2);
    let output = disk.root.join("export.img");
    tool.commit(1, &output).unwrap();
    assert_eq!(std::fs::read(output).unwrap(), expected);
    tool.rollback(1, &disk.root.join("backup.upfj")).unwrap();
    assert_eq!(
        tool.verify()
            .unwrap()
            .overlay
            .unwrap()
            .materialize()
            .unwrap(),
        expected
    );
    drop(tool);
    let mut machine = assemble(&disk.config()).unwrap();
    assert_eq!(ata(&mut machine).drive(0).unwrap().image(), expected);
    machine.finish_persistence().unwrap();
    assert_eq!(
        std::fs::read(disk.root.join("base.img")).unwrap(),
        disk.base
    );
}

#[test]
#[cfg(feature = "test-support")]
fn ide_journal_guest_outsw_state_matches_volatile_and_failing_sink_at_small_caps() {
    for step in [1, 17, 257] {
        let disk = Disk::new();
        let failing_disk = Disk::new();
        let mut config = pio_workload(&disk, false);
        config.drives[0].write = WriteMode::Volatile;
        let mut volatile = assemble(&config).unwrap();
        let mut journal = assemble(&pio_workload(&disk, false)).unwrap();
        let mut failing = assemble(&pio_workload(&failing_disk, false)).unwrap();
        failing.persistence[0].fail_output();
        loop {
            let cap = volatile.machine.virtual_time() + step;
            volatile
                .run_session(&RunPolicy::natural(Some(cap), None))
                .unwrap();
            journal
                .run_session(&RunPolicy::natural(Some(cap), None))
                .unwrap();
            let failed = failing.run_session(&RunPolicy::natural(Some(cap), None));
            let mut expected = vec![0; 0x10000];
            volatile.machine.cpu().read_mem(0, &mut expected).unwrap();
            let state = ata(&mut volatile).save();
            for machine in [&mut journal, &mut failing] {
                let mut actual = vec![0; 0x10000];
                machine.machine.cpu().read_mem(0, &mut actual).unwrap();
                assert_eq!(actual, expected);
                assert_eq!(ata(machine).save(), state);
                assert_eq!(
                    machine.machine.cpu().retired(),
                    volatile.machine.cpu().retired()
                );
                assert_eq!(
                    machine.machine.virtual_time(),
                    volatile.machine.virtual_time()
                );
                assert_eq!(machine.machine.clock(), volatile.machine.clock());
            }
            if failed.is_err() {
                assert!(matches!(failed, Err(RunError::Persistence(_))));
                assert_eq!(
                    ata(&mut failing).port_read(0x3F6, 1).unwrap(),
                    0x50,
                    "sink failure must be observed only after command completion"
                );
                assert_eq!(disk.scan().report.unsealed_tip.unwrap().commands, 1);
                break;
            }
            assert!(
                disk.scan().report.unsealed_tip.is_none(),
                "host caps must not submit an active command"
            );
            assert!(cap < 10_000);
        }
        journal.finish_persistence().unwrap();
        assert!(failing.finish_persistence().is_err());
    }
}

#[test]
fn base_mutation_stops_the_frontend_before_another_run_and_after_finish() {
    for finish_first in [false, true] {
        let disk = Disk::new();
        let mut machine = assemble(&disk.config()).unwrap();
        if finish_first {
            machine.finish_persistence().unwrap();
        }
        std::fs::write(disk.root.join("base.img"), vec![0xA5; disk.base.len()]).unwrap();
        let error = machine.check_persistence().unwrap_err();
        assert!(error
            .to_string()
            .contains("base file changed after verification"));
    }
}
