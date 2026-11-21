// SPDX-License-Identifier: AGPL-3.0-or-later
//! File-backed assembly regression tests; persistent-media.md § Integration.
use crate::assemble::HostValidate;
use crate::FileMount;
use assembly::test_support::*;
use assembly::*;
use device::{SectorMedia, SectorWrite};
use journal_files::{JournalMedia, SinkStatus, VerifiedBase};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};
trait TestPersistence {
    fn prepare_removable(
        drive_index: usize,
        path: &std::path::Path,
        base: VerifiedBase,
        keep: u32,
    ) -> (Self, Box<dyn device::SectorMedia>)
    where
        Self: Sized;
}
impl TestPersistence for PersistenceHandle {
    /// A removable medium whose journal at host `path` opens at insert.
    fn prepare_removable(
        drive_index: usize,
        path: &Path,
        base: VerifiedBase,
        keep_generations: u32,
    ) -> (Self, Box<dyn SectorMedia>) {
        Self::prepare_mounted(
            drive_index,
            Rc::new(FileMount::new()),
            path,
            (*base).clone().into(),
            keep_generations,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpu::Cpu;

    #[test]
    fn floppy_media_journal_mock_bus_seals_releases_lock_and_reopens_overlay() {
        use device::{Device, DmaOp, DmaOutcome};
        use devices::{upd765::Drive, FloppyGeometry, Upd765};
        let mut machine = crate::assemble(&assembly::at_boot(vec![0xf4]))
            .unwrap()
            .machine;
        let temp = crate::test_support::TempDir::new("floppy-eject-journal");
        let path = temp.path().join("disk.upfj");
        let base = vec![0x11; 1_474_560];
        let (mut handle, media) = PersistenceHandle::prepare_removable(
            0,
            &path,
            VerifiedBase::from_bytes(&base).unwrap(),
            10,
        );
        assert!(!path.exists(), "inactive journal must not be opened");
        handle.check_running().unwrap();
        handle.insert().unwrap();
        let mut fdc = Upd765::with_drives([
            Some(Drive::with_media(FloppyGeometry::HD1440, media)),
            None,
            None,
            None,
        ]);
        fdc.port_write(0x3f2, 1, 0x1c).unwrap();
        let mut bus = testkit::MockBus::new(Box::new(fdc));
        for value in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            bus.port_write(0x3f5, 1, value).unwrap();
        }
        assert!(matches!(
            bus.device_mut().take_dma_request().unwrap().op,
            DmaOp::FromMemory { len: 512 }
        ));
        bus.device_mut().dma_complete(DmaOutcome::FromMemory {
            bytes: vec![0x77; 512],
            terminal_count: true,
        });
        for _ in 0..7 {
            bus.port_read(0x3f5, 1).unwrap();
        }
        handle.eject(&mut machine).unwrap();
        bus.device_mut()
            .as_any_mut()
            .unwrap()
            .downcast_mut::<Upd765>()
            .unwrap()
            .eject(0)
            .unwrap();
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(
            scan.report.generations[0].reason,
            journal::ClosingReason::SessionEnd
        );
        assert_eq!(&scan.image.unwrap()[..512], &[0x77; 512]);
        let other =
            JournalMedia::open(&path, &base, base.len() as u64, SinkStatus::default()).unwrap();
        drop(other);
        handle.check_running().unwrap();
        handle.insert().unwrap();
        bus.device_mut()
            .as_any_mut()
            .unwrap()
            .downcast_mut::<Upd765>()
            .unwrap()
            .insert(0, 0)
            .unwrap();
        for value in [6, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            bus.port_write(0x3f5, 1, value).unwrap();
        }
        let DmaOp::ToMemory(bytes) = bus.device_mut().take_dma_request().unwrap().op else {
            panic!("READ DATA");
        };
        assert_eq!(bytes, vec![0x77; 512]);
        bus.device_mut().dma_complete(DmaOutcome::ToMemory {
            transferred: 512,
            terminal_count: true,
        });
        handle.finish(&mut machine).unwrap();
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert_eq!(scan.report.generations.len(), 2);
        assert_eq!(base, vec![0x11; 1_474_560]);
    }

    #[test]
    fn floppy_media_reinsertion_preserves_retention_warning_from_eject() {
        let mut machine = crate::assemble(&assembly::at_boot(vec![0xf4]))
            .unwrap()
            .machine;
        let temp = crate::test_support::TempDir::new("removable-retention-warning");
        let path = temp.path().join("disk.upfj");
        let base = vec![0; 512];
        let (mut handle, mut media) = PersistenceHandle::prepare_removable(
            0,
            &path,
            VerifiedBase::from_bytes(&base).unwrap(),
            1,
        );
        handle.insert().unwrap();
        media.write_batch(&[SectorWrite {
            lba: 0,
            bytes: [0x11; 512],
        }]);
        handle.eject(&mut machine).unwrap();
        handle.insert().unwrap();
        media.write_batch(&[SectorWrite {
            lba: 0,
            bytes: [0x22; 512],
        }]);
        handle.fail_retention("temporary write");
        handle.eject(&mut machine).unwrap();
        let warning = handle
            .maintenance_failure()
            .expect("retention warning latched on eject");
        handle.insert().unwrap();
        assert_eq!(
            handle.maintenance_failure(),
            Some(warning.clone()),
            "reinsertion must retain the warning until the frontend can print it"
        );
        assert_eq!(media.read_sector(0), [0x22; 512]);
        handle.eject(&mut machine).unwrap();
        assert_eq!(handle.maintenance_failure(), Some(warning));
    }

    #[test]
    fn floppy_media_journal_failed_eject_never_writes_a_clean_close() {
        let mut machine = crate::assemble(&assembly::at_boot(vec![0xf4]))
            .unwrap()
            .machine;
        let temp = crate::test_support::TempDir::new("floppy-eject-error");
        let path = temp.path().join("disk.upfj");
        let base = vec![0; 1_474_560];
        let (mut handle, mut media) = PersistenceHandle::prepare_removable(
            0,
            &path,
            VerifiedBase::from_bytes(&base).unwrap(),
            10,
        );
        handle.insert().unwrap();
        handle.fail_output();
        media.write_batch(&[SectorWrite {
            lba: 0,
            bytes: [0x77; 512],
        }]);
        assert!(handle.eject(&mut machine).is_err());
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert!(scan.report.generations.is_empty());
        assert_eq!(media.read_sector(0), [0x77; 512]);
    }

    fn nvram_config(dir: &Path, program: Vec<u8>) -> (MachineConfig, [u8; 64], PathBuf) {
        use sha2::{Digest, Sha256};
        let base = std::array::from_fn(|i| i as u8);
        let image = dir.join("cmos.bin");
        let journal = dir.join("cmos.upfj");
        std::fs::write(&image, base).unwrap();
        let mut config = assembly::at_rtc(program, 100, 10);
        let rtc = config
            .devices
            .iter_mut()
            .find(|d| matches!(d.kind, assembly::DeviceKind::Rtc(_)))
            .unwrap();
        let assembly::DeviceKind::Rtc(c) = &mut rtc.kind else {
            unreachable!()
        };
        c.nvram = assembly::NvramSource::Journal {
            image,
            expected_sha256: Sha256::digest(base).into(),
            journal: journal.clone(),
            keep_generations: 2,
        };
        (config, base, journal)
    }

    #[cfg(unix)]
    #[test]
    fn logical_validation_ignores_host_paths_but_checks_journal_keys_and_retention() {
        let temp = crate::test_support::TempDir::new("logical-validation");
        let (mut config, _, journal) = nvram_config(temp.path(), vec![0xf4]);
        std::os::unix::fs::symlink("absent", &journal).unwrap();
        assert!(config.validate_host().is_err());
        config.validate_logical().unwrap();

        let rtc = config
            .devices
            .iter_mut()
            .find_map(|d| match &mut d.kind {
                assembly::DeviceKind::Rtc(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let assembly::NvramSource::Journal {
            keep_generations, ..
        } = &mut rtc.nvram
        else {
            unreachable!()
        };
        *keep_generations = 0;
        assert!(config
            .validate_logical()
            .unwrap_err()
            .to_string()
            .contains("keep_generations must be positive"));
        let (nvram, _, _) = nvram_config(temp.path(), vec![0xf4]);
        let rtc = nvram
            .devices
            .iter()
            .find(|d| matches!(d.kind, assembly::DeviceKind::Rtc(_)))
            .unwrap()
            .clone();
        let mut config = assembly::at_fdc_write(vec![0xf4], vec![0; 512], 0x1000);
        config
            .devices
            .retain(|d| !matches!(d.kind, assembly::DeviceKind::Rtc(_)));
        config.devices.push(rtc);
        config.drives[0].image = ImageSource::File(temp.path().join("disk.img"));
        config.drives[0].write = WriteMode::journal(&journal);
        let error = config.validate_logical().unwrap_err().to_string();
        assert!(error.contains("also another medium's journal"), "{error}");
    }

    fn write_cmos(machine: &mut assembly::AssembledMachine, index: u8, value: u8) {
        machine
            .machine
            .bus_mut()
            .port_write(0x70, 1, u32::from(index))
            .unwrap();
        machine
            .machine
            .bus_mut()
            .port_write(0x71, 1, u32::from(value))
            .unwrap();
    }

    fn read_cmos(machine: &mut assembly::AssembledMachine, index: u8) -> u8 {
        machine
            .machine
            .bus_mut()
            .port_write(0x70, 1, u32::from(index))
            .unwrap();
        machine.machine.bus_mut().port_read(0x71, 1).unwrap() as u8
    }

    #[test]
    fn nvram_run_stops_and_prompt_polls_do_not_append() {
        let temp = crate::test_support::TempDir::new("nvram-polls");
        let (config, base, path) = nvram_config(temp.path(), vec![0x90, 0xEB, 0xFD]);
        let mut machine = crate::assemble_with_observation(
            &config,
            &assembly::ObservationPolicy {
                routes: vec![assembly::ObservationRequest {
                    device: assembly::DeviceId::Vga,
                    output: assembly::OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..Default::default()
            },
        )
        .unwrap();
        write_cmos(&mut machine, 0x10, 0xA5);
        machine
            .run_session(&assembly::RunPolicy::natural(Some(5), None))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            journal::HEADER_LEN as u64
        );
        let mut policy = assembly::RunPolicy::natural(Some(20), None);
        policy.stop = assembly::StopCondition::PromptThen {
            console: assembly::ConsoleProbe {
                device: assembly::DeviceId::Vga,
            },
            needle: "never present".into(),
            poll_step: 2,
            then_budget: 0,
        };
        machine.run_session(&policy).unwrap();
        machine.check_persistence().unwrap();
        machine.persistence[0]
            .checkpoint(&mut machine.machine)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            journal::HEADER_LEN as u64
        );
        machine.finish_persistence().unwrap();
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(
            scan.report.generations[0].reason,
            journal::ClosingReason::SessionEnd
        );
        assert_eq!(scan.report.generations[0].commands, 1);
    }

    #[test]
    fn nvram_finish_coalesces_replays_and_closes_one_session_generation() {
        let temp = crate::test_support::TempDir::new("nvram-session");
        let (config, base, path) = nvram_config(
            temp.path(),
            vec![0xFA, 0xB0, 0x0E, 0xE6, 0x70, 0xB0, 0xA5, 0xE6, 0x71, 0xF4],
        );
        let mut first = crate::assemble(&config).unwrap();
        assert_eq!(read_cmos(&mut first, 0x0B), 2);
        first
            .run_session(&assembly::RunPolicy::natural(Some(100), None))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            journal::HEADER_LEN as u64
        );
        first.finish_persistence().unwrap();
        let length = std::fs::metadata(&path).unwrap().len();
        first.finish_persistence().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
        assert_eq!(
            first.persistence[0]
                .checkpoint(&mut first.machine)
                .unwrap_err()
                .source
                .message,
            "session already finished"
        );
        drop(first);
        let mut second = crate::assemble(&config).unwrap();
        assert_eq!(read_cmos(&mut second, 14), 0xA5);
        assert_eq!(read_cmos(&mut second, 63), base[63]);
        assert_eq!(read_cmos(&mut second, 0x0B), 2);
        for index in 0..14 {
            write_cmos(&mut second, index, 0x12);
        }
        write_cmos(&mut second, 63, 0x55);
        write_cmos(&mut second, 63, base[63]);
        write_cmos(&mut second, 15, 9);
        write_cmos(&mut second, 16, 0x67);
        write_cmos(&mut second, 50, 0x21);
        second.finish_persistence().unwrap();
        drop(second);
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 2);
        assert!(scan
            .report
            .generations
            .iter()
            .all(|g| g.reason == journal::ClosingReason::SessionEnd && g.commands == 1));
        let image = scan.image.unwrap();
        assert_eq!(&image[..14], &base[..14]);
        assert_eq!(image[15], base[15]);
        assert_eq!(image[16], 0x67);
        assert_eq!(image[50], 0x21);
        let mut third = crate::assemble(&config).unwrap();
        assert_eq!(read_cmos(&mut third, 15), base[15]);
        assert_eq!(read_cmos(&mut third, 50), 0x21);
        assert_eq!(read_cmos(&mut third, 0), 0);
        third.finish_persistence().unwrap();
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
        let last = scan.report.generations.last().unwrap();
        assert_eq!(last.reason, journal::ClosingReason::SessionEnd);
        assert_eq!(
            last.commands, 0,
            "unchanged session has only its end marker"
        );
    }

    fn reset_config(config: &mut MachineConfig, reset_program: Vec<u8>) {
        config.memory.address_space_bytes = 0x100000;
        config.images.push(assembly::LoadImage {
            base: 0xFFFF0,
            source: ImageSource::Inline(reset_program),
            kind: assembly::ImageKind::Rom,
            provides: vec![],
        });
    }

    #[test]
    fn nvram_shutdown_only_reset_never_appends_ram() {
        for index in [0x0F, 0x10] {
            let temp = crate::test_support::TempDir::new("nvram-shutdown");
            let (mut config, base, path) = nvram_config(
                temp.path(),
                vec![
                    0xFA, 0xB0, index, 0xE6, 0x70, 0xB0, 9, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64,
                ],
            );
            reset_config(&mut config, vec![0xF4]);
            let mut machine = crate::assemble(&config).unwrap();
            machine
                .run_session(&assembly::RunPolicy::natural(Some(100), None))
                .unwrap();
            assert_eq!(
                read_cmos(&mut machine, index),
                9,
                "CPU reset preserves live CMOS"
            );
            let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
            assert!(scan.report.generations.is_empty());
            if index == 15 {
                assert!(scan.report.unsealed_tip.is_none());
                assert_eq!(
                    std::fs::metadata(&path).unwrap().len(),
                    journal::HEADER_LEN as u64
                );
            } else {
                assert_eq!(scan.report.unsealed_tip.as_ref().unwrap().commands, 1);
                assert_eq!(scan.image.as_ref().unwrap()[16], 9);
                assert_eq!(scan.image.as_ref().unwrap()[15], base[15]);
            }
            machine.finish_persistence().unwrap();
            let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
            assert_eq!(scan.report.generations.len(), 1);
            assert_eq!(
                scan.report.generations[0].reason,
                journal::ClosingReason::SessionEnd
            );
            assert_eq!(scan.report.generations[0].commands, u64::from(index == 16));
        }
    }

    #[test]
    fn nvram_execution_error_leaves_reset_records_open_for_recovery() {
        let temp = crate::test_support::TempDir::new("nvram-error");
        let (mut config, base, path) = nvram_config(
            temp.path(),
            vec![
                0xFA, 0xB0, 0x10, 0xE6, 0x70, 0xB0, 9, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64,
            ],
        );
        // A processor exception now vectors in the guest. Fetching beyond the
        // rig's physical map still tests host-error recovery after CPU reset.
        reset_config(&mut config, vec![0xEA, 0xFF, 0xFF, 0xFF, 0xFF]);
        let mut machine = crate::assemble(&config).unwrap();
        let error = machine
            .run_session(&assembly::RunPolicy::natural(Some(100), None))
            .unwrap_err();
        assert!(matches!(
            error,
            assembly::RunError::Machine(machine::MachineError::Cpu(cpu::CpuError::Unmapped {
                addr: 0x10FFEF
            }))
        ));
        drop(machine);
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert!(scan.report.generations.is_empty());
        assert_eq!(scan.report.unsealed_tip.as_ref().unwrap().commands, 1);
        let mut next = crate::assemble(&config).unwrap();
        assert!(next.persistence[0].mount_report().unsealed_tip.is_some());
        assert_eq!(read_cmos(&mut next, 16), 9);
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(
            scan.report.generations[0].reason,
            journal::ClosingReason::Recovered
        );
        next.finish_persistence().unwrap();
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 2);
        assert_eq!(
            scan.report.generations[1].reason,
            journal::ClosingReason::SessionEnd
        );
    }

    #[test]
    fn nvram_resets_share_a_generation_and_retention_waits_for_finish() {
        let temp = crate::test_support::TempDir::new("nvram-retention");
        let (mut config, base, path) = nvram_config(temp.path(), vec![0xF4]);
        for device in &mut config.devices {
            if let assembly::DeviceKind::Rtc(assembly::CmosCfg {
                nvram:
                    assembly::NvramSource::Journal {
                        keep_generations, ..
                    },
                ..
            }) = &mut device.kind
            {
                *keep_generations = 1;
            }
        }
        let mut first = crate::assemble(&config).unwrap();
        write_cmos(&mut first, 16, 0x55);
        first.finish_persistence().unwrap();
        drop(first);
        let mut second = crate::assemble(&config).unwrap();
        for value in [0x66, 0x77] {
            write_cmos(&mut second, 16, value);
            second.before_cpu_reset().unwrap();
            let before = std::fs::read(&path).unwrap();
            second.before_cpu_reset().unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "unchanged reset adds no record"
            );
            let scan = journal::scan(&before, &base, 64);
            assert_eq!(scan.report.header.unwrap().anchor_generation, 0);
            assert_eq!(scan.report.generations.len(), 1);
            assert_eq!(scan.report.unsealed_tip.unwrap().generation, 2);
        }
        second.finish_persistence().unwrap();
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(scan.report.header.unwrap().anchor_generation, 2);
        assert_eq!(scan.image.unwrap()[16], 0x77);
        let error = second.persistence[0]
            .checkpoint(&mut second.machine)
            .unwrap_err();
        assert_eq!(error.source.operation, "checkpoint");
        assert_eq!(error.source.message, "session already finished");
    }

    #[test]
    fn nvram_tail_damage_is_reported_and_base_pin_precedes_outputs() {
        use std::io::Write;
        let temp = crate::test_support::TempDir::new("nvram-tail");
        let (config, base, path) = nvram_config(temp.path(), vec![0xF4]);
        let mut machine = crate::assemble(&config).unwrap();
        write_cmos(&mut machine, 63, 0x99);
        machine.finish_persistence().unwrap();
        drop(machine);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0x55; 27])
            .unwrap();
        let mut machine = crate::assemble(&config).unwrap();
        assert_eq!(machine.persistence[0].mount_report().discarded_bytes, 27);
        assert_eq!(read_cmos(&mut machine, 63), 0x99);
        machine.finish_persistence().unwrap();
        drop(machine);
        let mut changed = base;
        changed[0] ^= 1;
        std::fs::write(temp.path().join("cmos.bin"), changed).unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = crate::assemble(&config).err().unwrap().to_string();
        assert!(error.contains("NVRAM: image sha256 mismatch"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(&path).unwrap();
        let lock = path.with_file_name("cmos.upfj.lock");
        std::fs::remove_file(&lock).unwrap();
        let error = crate::assemble(&config).err().unwrap().to_string();
        assert!(error.contains("NVRAM: image sha256 mismatch"), "{error}");
        assert!(!path.exists());
        assert!(!lock.exists());
    }

    #[test]
    fn nvram_output_failure_stops_before_cpu_reset() {
        let temp = crate::test_support::TempDir::new("nvram-reset");
        let program = vec![
            0xFA, 0xB0, 0x3F, 0xE6, 0x70, 0xB0, 0xA5, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64, 0xF4,
        ];
        let (mut config, base, path) = nvram_config(temp.path(), program);
        config.memory.address_space_bytes = 0x100000;
        config.images.push(assembly::LoadImage {
            base: 0xFFFF0,
            source: ImageSource::Inline(vec![0xF4]),
            kind: assembly::ImageKind::Rom,
            provides: vec![],
        });
        for fail in [true, false] {
            let mut machine = crate::assemble(&config).unwrap();
            if fail {
                machine.persistence[0].fail_output();
            }
            let result = machine.run_session(&assembly::RunPolicy::natural(Some(100), None));
            if fail {
                assert!(matches!(result, Err(assembly::RunError::Persistence(_))));
                assert_eq!(machine.machine.cpu().code_segment(), 0);
                assert!(machine
                    .check_persistence()
                    .unwrap_err()
                    .to_string()
                    .starts_with("NVRAM persistence:"));
            } else {
                assert_eq!(result.unwrap(), machine::RunOutcome::Halted);
                assert_eq!(machine.machine.cpu().code_segment(), 0xF000);
                let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, 64);
                assert_eq!(
                    scan.report.generations.len(),
                    0,
                    "reset boundary must keep the session open"
                );
                assert_eq!(scan.report.unsealed_tip.as_ref().unwrap().commands, 1);
                assert_eq!(scan.image.as_ref().unwrap()[63], 0xA5);
                machine.finish_persistence().unwrap();
            }
        }
    }

    #[test]
    #[should_panic(expected = "journal ATA controller must exist and expose Ata")]
    fn ide_journal_invalid_controller_index_is_an_assembly_invariant() {
        let temp = crate::test_support::TempDir::new("ide-controller-invariant");
        let (config, _, _) = nvram_config(temp.path(), vec![0xF4]);
        let mut machine = crate::assemble(&config).unwrap();
        let controller = machine.device_index[&assembly::DeviceId::Rtc];
        flush_ata_with_invalid_controller(&mut machine, controller);
    }

    #[test]
    fn ide_journal_sink_failure_keeps_guest_writes_and_recovers_unsealed_prefix() {
        let temp = crate::test_support::TempDir::new("ide-sink-failure");
        let base = vec![0; 32768];
        let base_path = temp.path().join("base.img");
        let path = temp.path().join("ide.upfj");
        std::fs::write(&base_path, &base).unwrap();
        let mut config = assembly::at_boot(vec![0xFA, 0xF4]);
        let cfg = assembly::IdeCfg {
            model: assembly::AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        config.devices.push(
            assembly::DeviceConfig::new(
                assembly::DeviceId::Ide,
                assembly::DeviceKind::Ide(cfg),
                cfg.port_windows(),
            )
            .with_irq(cfg.irq()),
        );
        config.drives.push(assembly::DriveConfig {
            media: None,
            controller: assembly::DeviceId::Ide,
            unit: 0,
            kind: assembly::DriveKind::HardDisk {
                geometry: assembly::HardDiskGeometry {
                    cylinders: 8,
                    heads: 2,
                    sectors_per_track: 4,
                    bytes_per_sector: 512,
                },
            },
            image: ImageSource::File(base_path.clone()),
            expected_sha256: None,
            write: WriteMode::journal(&path),
        });
        let mut machine = crate::assemble(&config).unwrap();
        for (lba, value) in [(1, 0x1234), (2, 0xABCD)] {
            if lba == 2 {
                machine.persistence[0].fail_output();
            }
            let bus = machine.machine.bus_mut();
            for (port, byte) in [
                (0x1F6, 0xE0),
                (0x1F2, 1),
                (0x1F3, lba),
                (0x1F4, 0),
                (0x1F5, 0),
                (0x1F7, 0x30),
            ] {
                bus.port_write(port, 1, byte).unwrap();
            }
            for _ in 0..256 {
                bus.port_write(0x1F0, 2, value).unwrap();
            }
            assert_eq!(bus.port_read(0x1F7, 1).unwrap(), 0x50);
            if lba == 1 {
                machine.check_persistence().unwrap();
            }
        }
        let index = machine.device_index[&assembly::DeviceId::Ide];
        let before = machine.machine.bus().device(index).unwrap().save();
        assert!(machine
            .check_persistence()
            .unwrap_err()
            .to_string()
            .starts_with("drive 0 persistence:"));
        let device = machine.machine.bus_mut().device_mut(index).unwrap();
        assert_eq!(device.save(), before);
        let ata = device
            .as_any_mut()
            .unwrap()
            .downcast_mut::<devices::Ata>()
            .unwrap();
        assert_eq!(
            &ata.drive(0).unwrap().image()[1024..1536],
            &0xABCDu16.to_le_bytes().repeat(256)
        );
        assert!(machine.finish_persistence().is_err());
        drop(machine);
        let scan = journal::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert!(scan.report.generations.is_empty());
        assert_eq!(scan.report.unsealed_tip.unwrap().commands, 1);
        assert_eq!(
            &scan.image.unwrap()[512..1024],
            &0x1234u16.to_le_bytes().repeat(256)
        );
        let mut fresh = crate::assemble(&config).unwrap();
        assert!(fresh.persistence[0].mount_report().unsealed_tip.is_some());
        fresh.finish_persistence().unwrap();
        assert_eq!(std::fs::read(base_path).unwrap(), base);
    }

    #[test]
    fn failing_sink_preserves_guest_prefix_and_stops_at_frontend_boundary() {
        let temp = crate::test_support::TempDir::new("journal-test");
        let path = temp.path();
        let out = |program: &mut Vec<u8>, port: u16, value: u8| {
            program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
        };
        let mut program = vec![0xFA, 0xFC];
        out(&mut program, 0x3F2, 0x1C);
        out(&mut program, 0x3F7, 2);
        for (operation, write) in [true, true, false].into_iter().enumerate() {
            for (port, value) in [
                (0x0A, 6),
                (0x0C, 0),
                (0x0B, if write { 0x4A } else { 0x46 }),
                (4, 0),
                (
                    4,
                    if !write {
                        10
                    } else if operation == 0 {
                        6
                    } else {
                        8
                    },
                ),
                (0x81, 0),
                (5, 0xFF),
                (5, 1),
                (0x0A, 2),
            ] {
                out(&mut program, port, value);
            }
            for value in [if write { 5 } else { 6 }, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
                out(&mut program, 0x3F5, value);
            }
            program.extend([0xBF, 0, 0x10 + operation as u8]);
            for _ in 0..7 {
                program.extend([0xEC, 0xAA]);
            }
        }
        program.push(0xF4);
        let pattern = vec![0xA7; 512];
        let mut config = assembly::at_fdc_write(program, pattern.clone(), 0x600);
        let base = path.join("base.img");
        let journal_path = path.join("disk.upfj");
        std::fs::write(&base, vec![0; config.drives[0].kind.image_bytes() as usize]).unwrap();
        config.drives[0].image = ImageSource::File(base);
        let second_pattern = vec![0x5C; 512];
        let mut second_image = config.images[1].clone();
        second_image.base = 0x800;
        second_image.source = ImageSource::Inline(second_pattern.clone());
        config.images.push(second_image);
        let mut volatile = crate::assemble(&config).unwrap();
        config.drives[0].write = WriteMode::journal(&journal_path);
        let mut failing = crate::assemble(&config).unwrap();
        failing.persistence[0].fail_output();
        let policy = assembly::RunPolicy::natural(Some(10_000), None);
        assert_eq!(
            volatile.run_session(&policy).unwrap(),
            machine::RunOutcome::Halted
        );
        assert!(matches!(
            failing.run_session(&policy),
            Err(assembly::RunError::Persistence(_))
        ));
        assert_eq!(failing.persistence[0].status().unwrap().operation, "write");
        let mut actual = vec![0; 0x10000];
        let mut expected = actual.clone();
        failing.machine.cpu().read_mem(0, &mut actual).unwrap();
        volatile.machine.cpu().read_mem(0, &mut expected).unwrap();
        assert_eq!(
            actual[0x1101] & 2,
            0,
            "the second WRITE DATA must not become write-protected"
        );
        assert_eq!(&actual[0x1100..0x1107], &expected[0x1100..0x1107]);
        assert!(
            actual[0xA00..0xC00].iter().all(|byte| *byte == 0x5C),
            "the second WRITE DATA must replace the first pattern after the host error"
        );
        assert_ne!(&actual[0xA00..0xC00], pattern);
        assert_eq!(actual, expected);
        assert_eq!(
            failing.machine.virtual_time(),
            volatile.machine.virtual_time()
        );
        assert_eq!(
            failing.machine.cpu().retired(),
            volatile.machine.cpu().retired()
        );
        assert_eq!(failing.machine.clock(), volatile.machine.clock());
        let before = failing.machine.virtual_time();
        assert!(failing
            .run_session(&assembly::RunPolicy::natural(Some(20_000), None))
            .is_err());
        assert_eq!(failing.machine.virtual_time(), before);
        assert!(failing.finish_persistence().is_err());
        drop(failing);
        drop(volatile);
        assert_eq!(
            std::fs::metadata(journal_path).unwrap().len(),
            journal::HEADER_LEN as u64
        );
    }
    #[test]
    fn retention_failures_preserve_controller_status_dma_irq_and_virtual_time() {
        for operation in [
            "temporary write",
            "temporary sync",
            "replace",
            "publication directory sync",
        ] {
            let temp = crate::test_support::TempDir::new("retention-test");
            let mut program = vec![0xFA, 0xFC];
            let out = |p: &mut Vec<u8>, port: u16, value: u8| {
                p.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
            };
            out(&mut program, 0x3F2, 0x1C);
            out(&mut program, 0x3F7, 2);
            for (port, value) in [
                (0x0A, 6),
                (0x0C, 0),
                (0x0B, 0x4A),
                (4, 0),
                (4, 6),
                (0x81, 0),
                (5, 0xFF),
                (5, 1),
                (0x0A, 2),
            ] {
                out(&mut program, port, value);
            }
            for value in [5, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
                out(&mut program, 0x3F5, value);
            }
            program.extend([0xBF, 0, 0x10]);
            for _ in 0..7 {
                program.extend([0xEC, 0xAA]);
            }
            program.push(0xF4);
            let mut config = assembly::at_fdc_write(program, vec![0xA7; 512], 0x600);
            let base = temp.path().join("base.img");
            let bytes = vec![0; config.drives[0].kind.image_bytes() as usize];
            std::fs::write(&base, &bytes).unwrap();
            config.drives[0].image = ImageSource::File(base.clone());
            let path = temp.path().join("disk.upfj");
            config.drives[0].write = WriteMode::Journal {
                path: path.clone(),
                keep_generations: 1,
            };
            let mut machine = crate::assemble(&config).unwrap();
            machine.persistence[0]
                .checkpoint(&mut machine.machine)
                .unwrap();
            machine.persistence[0].fail_retention(operation);
            machine
                .run_session(&assembly::RunPolicy::natural(Some(10_000), None))
                .unwrap();
            let mut before = vec![0; 0x10000];
            machine.machine.cpu().read_mem(0, &mut before).unwrap();
            let time = machine.machine.virtual_time();
            let retired = machine.machine.cpu().retired();
            let devices: Vec<_> = machine
                .device_index
                .values()
                .map(|&index| {
                    (
                        index,
                        machine.machine.bus().device(index).unwrap().save(),
                        machine.machine.bus().device_irq_line(index),
                    )
                })
                .collect();
            machine.persistence[0]
                .checkpoint(&mut machine.machine)
                .unwrap();
            let failure = machine.persistence[0].maintenance_failure().unwrap();
            assert_eq!(machine.persistence[0].status().is_some(), failure.uncertain);
            assert_eq!(machine.check_persistence().is_err(), failure.uncertain);
            assert_eq!(
                failure.uncertain,
                matches!(operation, "replace" | "publication directory sync")
            );
            let mut after = before.clone();
            machine.machine.cpu().read_mem(0, &mut after).unwrap();
            assert_eq!(after, before);
            assert_eq!(after[0x1001] & 2, 0);
            assert_eq!(machine.machine.virtual_time(), time);
            assert_eq!(machine.machine.cpu().retired(), retired);
            for (index, snapshot, irq) in devices {
                assert_eq!(
                    machine.machine.bus().device(index).unwrap().save(),
                    snapshot
                );
                assert_eq!(machine.machine.bus().device_irq_line(index), irq);
            }
            assert_eq!(std::fs::read(&base).unwrap(), bytes);
            let scan = journal::scan(&std::fs::read(&path).unwrap(), &bytes, bytes.len() as u64);
            assert_eq!(scan.report.status, journal::Status::Valid);
            assert!(!scan.report.needs_recovery());
            assert_eq!(&scan.image.unwrap()[..512], &[0xA7; 512]);
        }
    }
}
