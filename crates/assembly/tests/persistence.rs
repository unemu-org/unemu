// SPDX-License-Identifier: AGPL-3.0-or-later
//! Assembly/session persistence acceptance; NEC µPD765 WRITE/READ DATA (N=2)
//! and Intel 8237A single-transfer mode. See persistent-media.md §Tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use assembly::{
    assemble, assemble_with_observation, AssembledMachine, DeviceId, ImageSource, MachineConfig,
    ObservationPolicy, RunPolicy, WriteMode,
};
use cpu::{Cpu, CpuReg};
use media::{ClosingReason, Frame, Header, Record};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "assembly-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn entries(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        paths.sort();
        paths
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn out(program: &mut Vec<u8>, port: u16, value: u8) {
    program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
}
fn dma(program: &mut Vec<u8>, write: bool, count: u16) {
    for (port, value) in [
        (0x0A, 6),
        (0x0C, 0),
        (0x0B, if write { 0x4A } else { 0x46 }),
        (4, 0),
        (4, if write { 6 } else { 10 }),
        (0x81, 0),
        (5, (count - 1) as u8),
        (5, ((count - 1) >> 8) as u8),
        (0x0A, 2),
    ] {
        out(program, port, value);
    }
}
fn command(program: &mut Vec<u8>, write: bool, result: u16) {
    for value in [if write { 5 } else { 6 }, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
        out(program, 0x3F5, value);
    }
    program.extend([0xBF, result as u8, (result >> 8) as u8]);
    for _ in 0..7 {
        program.extend([0xEC, 0xAA]);
    }
}
fn program(write: bool, count: u16) -> Vec<u8> {
    let mut program = vec![0xFA, 0xFC];
    out(&mut program, 0x3F2, 0x1C);
    if write {
        dma(&mut program, true, count);
        command(&mut program, true, 0x1000);
    }
    dma(&mut program, false, 512);
    command(&mut program, false, 0x1010);
    program.push(0xF4);
    program
}
fn pattern() -> Vec<u8> {
    (0..512).map(|i| (i as u8) ^ 0xC7).collect()
}
fn config(temp: &Temp, write: bool, count: u16) -> MachineConfig {
    let mut config = assembly::at_fdc_write(program(write, count), pattern(), 0x600);
    let base = temp.path("base.img");
    if !base.exists() {
        std::fs::write(&base, vec![0; config.drives[0].kind.image_bytes() as usize]).unwrap();
    }
    config.drives[0].image = ImageSource::File(base);
    config.drives[0].write = WriteMode::journal(temp.path("disk.upfj"));
    config
        .devices
        .iter_mut()
        .find(|d| d.id == DeviceId::Fdc)
        .unwrap()
        .irq = Some(6);
    config
}
fn run(assembled: &mut AssembledMachine) {
    assert_eq!(
        assembled
            .run_session(&RunPolicy::natural(Some(10_000), None))
            .unwrap(),
        machine::RunOutcome::Halted
    );
}
fn readback(assembled: &AssembledMachine) -> Vec<u8> {
    let mut bytes = vec![0; 512];
    assembled.machine.cpu().read_mem(0xA00, &mut bytes).unwrap();
    bytes
}
fn report(path: &Path, base: &Path) -> media::VerificationReport {
    let bytes = std::fs::read(base).unwrap();
    media::scan(&std::fs::read(path).unwrap(), &bytes, bytes.len() as u64).report
}

#[test]
fn guest_write_survives_fresh_assembly_including_partial_sector_and_interruption() {
    for count in [512, 17] {
        for finish in [true, false] {
            let temp = Temp::new();
            let first = config(&temp, true, count);
            let base = std::fs::read(temp.path("base.img")).unwrap();
            let mut assembled = assemble(&first).unwrap();
            assert_eq!(assembled.persistence.len(), 1);
            run(&mut assembled);
            let mut expected = pattern();
            expected[count as usize..].fill(0);
            assert_eq!(readback(&assembled), expected);
            if finish {
                assembled.finish_persistence().unwrap();
            }
            drop(assembled);
            let unmounted = report(&temp.path("disk.upfj"), &temp.path("base.img"));
            assert_eq!(unmounted.unsealed_tip.is_some(), !finish);
            let mut second = assemble(&config(&temp, false, count)).unwrap();
            if !finish {
                assert!(second.persistence[0].mount_report().unsealed_tip.is_some());
            }
            run(&mut second);
            assert_eq!(readback(&second), expected);
            second.finish_persistence().unwrap();
            drop(second);
            let closed = report(&temp.path("disk.upfj"), &temp.path("base.img"));
            assert_eq!(
                closed.generations[0].reason,
                if finish {
                    ClosingReason::SessionEnd
                } else {
                    ClosingReason::Recovered
                }
            );
            assert_eq!(std::fs::read(temp.path("base.img")).unwrap(), base);
        }
    }
}

#[test]
fn volatile_and_readonly_create_no_outputs_and_restart_from_base() {
    for mode in [WriteMode::Volatile, WriteMode::ReadOnly] {
        let temp = Temp::new();
        let mut config = config(&temp, true, 512);
        config.drives[0].write = mode.clone();
        let before = temp.entries();
        let base = std::fs::read(temp.path("base.img")).unwrap();
        let mut assembled = assemble(&config).unwrap();
        assert!(assembled.persistence.is_empty());
        run(&mut assembled);
        assert_eq!(
            readback(&assembled),
            if mode == WriteMode::Volatile {
                pattern()
            } else {
                vec![0; 512]
            }
        );
        if mode == WriteMode::ReadOnly {
            let mut status = [0; 7];
            assembled
                .machine
                .cpu()
                .read_mem(0x1000, &mut status)
                .unwrap();
            assert_eq!(&status[..2], &[0x40, 2]);
        }
        assembled.finish_persistence().unwrap();
        drop(assembled);
        config.images[0].source = ImageSource::Inline(program(false, 512));
        let mut second = assemble(&config).unwrap();
        run(&mut second);
        assert_eq!(readback(&second), vec![0; 512]);
        drop(second);
        assert_eq!(temp.entries(), before);
        assert_eq!(std::fs::read(temp.path("base.img")).unwrap(), base);
    }
}

#[test]
fn journal_and_volatile_have_identical_guest_state_at_every_boundary() {
    for count in [512, 17] {
        let temp = Temp::new();
        let journal = config(&temp, true, count);
        let mut volatile = journal.clone();
        volatile.drives[0].write = WriteMode::Volatile;
        let observation = ObservationPolicy {
            trace: device::TracePolicy::Ring { capacity: 4096 },
            ..Default::default()
        };
        let mut a = assemble_with_observation(&journal, &observation).unwrap();
        let mut b = assemble_with_observation(&volatile, &observation).unwrap();
        for cap in (1..=1000).step_by(7) {
            let policy = RunPolicy::natural(Some(cap), None);
            let left = a.run_session(&policy).unwrap();
            let right = b.run_session(&policy).unwrap();
            assert_eq!(left, right);
            let mut ma = vec![0; 0x10000];
            let mut mb = ma.clone();
            a.machine.cpu().read_mem(0, &mut ma).unwrap();
            b.machine.cpu().read_mem(0, &mut mb).unwrap();
            assert_eq!(ma, mb);
            for reg in [
                CpuReg::Rax,
                CpuReg::Rbx,
                CpuReg::Rcx,
                CpuReg::Rdx,
                CpuReg::Rsi,
                CpuReg::Rdi,
                CpuReg::Rsp,
                CpuReg::Rbp,
                CpuReg::Rip,
            ] {
                assert_eq!(a.machine.cpu().reg(reg), b.machine.cpu().reg(reg));
            }
            assert_eq!(
                a.machine.cpu().code_segment(),
                b.machine.cpu().code_segment()
            );
            assert_eq!(
                a.machine.cpu().interrupts_enabled(),
                b.machine.cpu().interrupts_enabled()
            );
            assert_eq!(a.machine.cpu().irq_ready(), b.machine.cpu().irq_ready());
            assert_eq!(
                a.machine.cpu().completed_steps(),
                b.machine.cpu().completed_steps()
            );
            assert_eq!(a.machine.cpu().retired(), b.machine.cpu().retired());
            assert_eq!(a.machine.virtual_time(), b.machine.virtual_time());
            assert_eq!(a.machine.clock(), b.machine.clock());
            for (&id, &index) in &a.device_index {
                let other = b.device_index[&id];
                assert_eq!(
                    a.machine.bus().device_irq_line(index),
                    b.machine.bus().device_irq_line(other)
                );
                assert_eq!(
                    a.machine.bus().device(index).unwrap().save(),
                    b.machine.bus().device(other).unwrap().save()
                );
                assert_eq!(
                    a.machine.bus_mut().device_mut(index).unwrap().take_trace(),
                    b.machine.bus_mut().device_mut(other).unwrap().take_trace()
                );
            }
            if left == machine::RunOutcome::Halted {
                break;
            }
        }
        assert_eq!(readback(&a), readback(&b));
        assert_ne!(readback(&a), vec![0; 512]);
        a.finish_persistence().unwrap();
    }
}

#[test]
fn journal_config_rejects_incompatible_sources_geometry_and_retention() {
    let temp = Temp::new();
    let original = config(&temp, true, 512);
    assert_eq!(
        original.drives[0].write,
        WriteMode::Journal {
            path: temp.path("disk.upfj"),
            keep_generations: 10
        }
    );
    for image in [
        ImageSource::Inline(vec![0; 368640]),
        ImageSource::Blank { size: 368640 },
    ] {
        let mut config = original.clone();
        config.drives[0].image = image;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("file-backed"));
    }
    let mut invalid = original.clone();
    invalid.drives[0].write = WriteMode::Journal {
        path: temp.path("disk.upfj"),
        keep_generations: 0,
    };
    assert!(invalid
        .validate()
        .unwrap_err()
        .to_string()
        .contains("positive"));
    invalid.drives[0].write = WriteMode::journal("");
    assert!(invalid
        .validate()
        .unwrap_err()
        .to_string()
        .contains("explicit"));
    for kind in [
        assembly::DriveKind::Floppy {
            geometry: assembly::FloppyGeometry {
                bytes_per_sector: 256,
                ..assembly::FloppyGeometry::DD_360K
            },
        },
        assembly::DriveKind::HardDisk {
            geometry: assembly::HardDiskGeometry {
                cylinders: 1,
                heads: 1,
                sectors_per_track: 1,
                bytes_per_sector: 512,
            },
        },
    ] {
        invalid = original.clone();
        invalid.drives[0].kind = kind;
        assert!(invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("floppy with 512-byte sectors"));
    }
    assert_eq!(temp.entries(), vec![temp.path("base.img")]);
}

#[test]
fn journal_and_lock_outputs_cannot_alias_any_mounted_input_or_output() {
    let temp = Temp::new();
    let original = config(&temp, true, 512);
    let assert_rejected = |config: &MachineConfig| {
        let before = temp.entries();
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("aliases"));
        let error = match assemble(config) {
            Ok(_) => panic!("alias mounted"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("drive") && error.contains("aliases"),
            "{error}"
        );
        assert_eq!(temp.entries(), before);
    };
    let mut candidate = original.clone();
    candidate.drives[0].write = WriteMode::journal(temp.path("base.img"));
    assert_rejected(&candidate);
    std::fs::hard_link(temp.path("base.img"), temp.path("hard.upfj")).unwrap();
    candidate.drives[0].write = WriteMode::journal(temp.path("hard.upfj"));
    assert_rejected(&candidate);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path("base.img"), temp.path("sym.upfj")).unwrap();
        candidate.drives[0].write = WriteMode::journal(temp.path("sym.upfj"));
        assert_rejected(&candidate);
    }
    candidate = original.clone();
    candidate.images[1].source = ImageSource::File(temp.path("base.img"));
    candidate.images[1].base = 0;
    candidate.drives[0].image = ImageSource::Inline(vec![0; 368640]);
    candidate.drives[0].write = WriteMode::Volatile;
    let mut second = original.drives[0].clone();
    second.unit = 1;
    second.write = WriteMode::journal(temp.path("hard.upfj"));
    candidate.drives.push(second);
    // Keep the load image valid; the alias must be caught before resolution.
    assert_rejected(&candidate);
    candidate = original.clone();
    let mut second = candidate.drives[0].clone();
    second.unit = 1;
    candidate.drives.push(second);
    assert_rejected(&candidate);
    candidate.drives[1].write = WriteMode::journal(temp.path("other.upfj"));
    std::fs::write(temp.path("disk.upfj"), [0; 128]).unwrap();
    std::fs::hard_link(temp.path("disk.upfj"), temp.path("other.upfj")).unwrap();
    assert_rejected(&candidate);
    candidate = original.clone();
    std::fs::hard_link(temp.path("base.img"), temp.path("disk.upfj.lock")).unwrap();
    assert_rejected(&candidate);
}

#[test]
fn damaged_journal_refuses_mount_with_drive_and_reason_without_overwriting_it() {
    let temp = Temp::new();
    let config = config(&temp, false, 512);
    std::fs::write(temp.path("disk.upfj"), b"damaged header").unwrap();
    let error = match assemble(&config) {
        Ok(_) => panic!("damaged journal mounted"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("drive 0 journal mount"), "{error}");
    assert!(error.contains("HeaderLength"), "{error}");
    assert_eq!(
        std::fs::read(temp.path("disk.upfj")).unwrap(),
        b"damaged header"
    );
}

#[test]
fn checkpoint_closes_a_generation_and_finish_is_explicit_and_idempotent() {
    let temp = Temp::new();
    let config = config(&temp, true, 512);
    let mut assembled = assemble(&config).unwrap();
    run(&mut assembled);
    assembled.persistence[0].checkpoint().unwrap();
    assert_eq!(
        report(&temp.path("disk.upfj"), &temp.path("base.img")).generations[0].reason,
        ClosingReason::Checkpoint
    );
    assembled.finish_persistence().unwrap();
    assembled.finish_persistence().unwrap();
    assert!(assembled.persistence[0].checkpoint().is_err());
    assert!(assembled
        .run_session(&RunPolicy::natural(Some(20_000), None))
        .is_err());
    assert_eq!(
        report(&temp.path("disk.upfj"), &temp.path("base.img"))
            .generations
            .len(),
        2
    );
}

#[test]
fn latched_host_failure_prevents_frontend_boundary_from_advancing_or_sealing() {
    let temp = Temp::new();
    let config = config(&temp, false, 512);
    let base = std::fs::read(temp.path("base.img")).unwrap();
    // A valid exhausted lineage makes checkpoint fail deterministically without
    // host permissions, process-global resource limits or filesystem quotas.
    let generation = u64::MAX - 1;
    let mut bytes = Header::new(&base, generation)
        .unwrap()
        .encode()
        .unwrap()
        .to_vec();
    for (sequence, record) in [
        (1, Record::CommandEnd { sectors: 0 }),
        (
            2,
            Record::GenerationEnd {
                reason: ClosingReason::RetentionAnchor,
            },
        ),
    ] {
        bytes.extend(
            Frame {
                sequence,
                generation,
                record,
            }
            .encode()
            .unwrap(),
        );
    }
    std::fs::write(temp.path("disk.upfj"), &bytes).unwrap();
    let mut assembled = assemble(&config).unwrap();
    assert!(assembled.persistence[0].checkpoint().is_err());
    assert!(assembled.persistence[0].status().is_some());
    let before = assembled.machine.virtual_time();
    let error = assembled
        .run_session(&RunPolicy::natural(Some(100), None))
        .unwrap_err();
    assert!(error.to_string().contains("drive 0 persistence"));
    assert_eq!(assembled.machine.virtual_time(), before);
    assert!(assembled.finish_persistence().is_err());
    assert_eq!(std::fs::read(temp.path("disk.upfj")).unwrap(), bytes);
}

#[test]
fn execution_error_keeps_commands_unsealed() {
    let temp = Temp::new();
    let mut config = config(&temp, true, 512);
    let ImageSource::Inline(program) = &mut config.images[0].source else {
        unreachable!()
    };
    program.pop();
    program.extend([0x0F, 0x05]);
    let mut assembled = assemble(&config).unwrap();
    assert!(assembled
        .run_session(&RunPolicy::natural(Some(10_000), None))
        .is_err());
    drop(assembled);
    let report = report(&temp.path("disk.upfj"), &temp.path("base.img"));
    assert!(report.unsealed_tip.is_some());
    assert!(report.generations.is_empty());
}

#[test]
fn creation_temp_namespace_rejects_mounted_inputs() {
    for load_image in [false, true] {
        let temp = Temp::new();
        let mut candidate = config(&temp, false, 512);
        let journal = temp.path("disk.upfj");
        let collision = temp.path("disk.upfj.create-7.tmp");
        if load_image {
            std::fs::write(&collision, vec![0; 512]).unwrap();
            candidate.images[1].source = ImageSource::File(collision.clone());
        } else {
            std::fs::rename(temp.path("base.img"), &collision).unwrap();
            candidate.drives[0].image = ImageSource::File(collision.clone());
        }
        let before = temp.entries();
        let error = candidate.validate().unwrap_err().to_string();
        assert!(error.contains("drive 0"), "{error}");
        assert!(error.contains(&journal.display().to_string()), "{error}");
        assert!(error.contains(&collision.display().to_string()), "{error}");
        assert!(error.contains("creation-temporary"), "{error}");
        assert!(assemble(&candidate).is_err());
        assert_eq!(temp.entries(), before);
        assert_eq!(
            std::fs::read(&collision).unwrap(),
            vec![0; if load_image { 512 } else { 368640 }]
        );
    }
}

#[test]
fn journal_config_refuses_creation_temp_names_in_either_drive_order() {
    for index in [0, 1] {
        let temp = Temp::new();
        let mut candidate = config(&temp, false, 512);
        let mut second = candidate.drives[0].clone();
        second.unit = 1;
        second.write = WriteMode::journal(temp.path("other.upfj"));
        candidate.drives.push(second);
        let path = temp.path("j.upfj.create-7.tmp");
        candidate.drives[index].write = WriteMode::journal(&path);
        let before = temp.entries();
        let error = candidate.validate().unwrap_err().to_string();
        assert!(error.contains(&format!("drive {index}")), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("creation-temporary"), "{error}");
        assert!(assemble(&candidate).is_err());
        assert_eq!(temp.entries(), before);
    }
}

#[cfg(unix)]
#[test]
fn reserved_symlink_input_names_are_refused_for_bases_and_load_images() {
    for load_image in [false, true] {
        let temp = Temp::new();
        let mut candidate = config(&temp, false, 512);
        let target = temp.path("unrelated.img");
        std::fs::write(&target, vec![0; if load_image { 512 } else { 368640 }]).unwrap();
        let link = temp.path("disk.upfj.create-7.tmp");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        if load_image {
            candidate.images[1].source = ImageSource::File(link.clone());
        } else {
            candidate.drives[0].image = ImageSource::File(link.clone());
        }
        let before = temp.entries();
        let error = candidate.validate().unwrap_err().to_string();
        assert!(
            error.contains("drive 0") && error.contains("creation-temporary"),
            "{error}"
        );
        assert!(error.contains(&link.display().to_string()), "{error}");
        assert!(
            error.contains(&temp.path("disk.upfj").display().to_string()),
            "{error}"
        );
        assert!(assemble(&candidate).is_err());
        assert_eq!(temp.entries(), before);
        assert_eq!(std::fs::read_link(link).unwrap(), target);
    }
}

#[test]
fn invalid_volatile_bases_keep_their_image_read_errors_with_an_unrelated_journal() {
    for case in ["missing", "not-a-directory", "symlink-loop"] {
        let temp = Temp::new();
        let mut candidate = config(&temp, false, 512);
        let mut second = candidate.drives[0].clone();
        second.unit = 1;
        candidate.drives[0].write = WriteMode::Volatile;
        let input = match case {
            "missing" => temp.path("missing").join("base.img"),
            "not-a-directory" => {
                std::fs::write(temp.path("file"), b"not a directory").unwrap();
                temp.path("file").join("base.img")
            }
            _ => {
                #[cfg(unix)]
                {
                    let link = temp.path("loop.img");
                    std::os::unix::fs::symlink(&link, &link).unwrap();
                    link
                }
                #[cfg(not(unix))]
                continue;
            }
        };
        candidate.drives[0].image = ImageSource::File(input.clone());
        second.write = WriteMode::Volatile;
        candidate.drives.push(second);
        let before = temp.entries();
        let without = match assemble(&candidate) {
            Ok(_) => panic!("invalid base mounted"),
            Err(error) => error.to_string(),
        };
        candidate.drives[1].write = WriteMode::journal(temp.path("other.upfj"));
        candidate.validate().unwrap();
        let with = match assemble(&candidate) {
            Ok(_) => panic!("invalid base mounted"),
            Err(error) => error.to_string(),
        };
        assert_eq!(with, without, "{case}");
        assert!(with.contains("cannot read image file"), "{with}");
        assert!(with.contains(&input.display().to_string()), "{with}");
        assert_eq!(temp.entries(), before);
    }
}

#[cfg(unix)]
#[test]
fn dangling_journal_alias_is_rejected_before_any_mount_or_lock() {
    let temp = Temp::new();
    let mut candidate = config(&temp, false, 512);
    let target = temp.path("a.upfj");
    let link = temp.path("link.upfj");
    candidate.drives[0].write = WriteMode::journal(&target);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut second = candidate.drives[0].clone();
    second.unit = 1;
    second.write = WriteMode::journal(&link);
    candidate.drives.push(second);
    let before = temp.entries();
    let error = candidate.validate().unwrap_err().to_string();
    assert!(error.contains("drive 1"), "{error}");
    assert!(error.contains("journal path"), "{error}");
    assert!(error.contains(&link.display().to_string()), "{error}");
    assert!(assemble(&candidate).is_err());
    assert_eq!(temp.entries(), before);
    assert!(!target.exists());
}

#[test]
fn journal_normalization_errors_name_the_path() {
    let temp = Temp::new();
    let mut candidate = config(&temp, false, 512);
    std::fs::write(temp.path("file"), b"not a directory").unwrap();
    let path = temp.path("file").join("disk.upfj");
    candidate.drives[0].write = WriteMode::journal(&path);
    let error = candidate.validate().unwrap_err().to_string();
    assert!(
        error.contains("drive 0") && error.contains("journal path"),
        "{error}"
    );
    assert!(error.contains(&path.display().to_string()), "{error}");
}

#[test]
fn invalid_observation_policies_never_create_or_recover_journals() {
    for recover in [false, true] {
        let temp = Temp::new();
        let candidate = config(&temp, true, 512);
        if recover {
            let mut assembled = assemble(&candidate).unwrap();
            run(&mut assembled);
            drop(assembled);
        }
        let journal = temp.path("disk.upfj");
        let before = temp.entries();
        let bytes = std::fs::read(&journal).ok();
        let request = assembly::ObservationRequest {
            device: DeviceId::Fdc,
            output: assembly::OutputKind::TextScanout,
            byte_limit: 0,
        };
        for routes in [
            vec![request],
            vec![assembly::ObservationRequest {
                device: DeviceId::Coprocessor,
                ..request
            }],
            vec![
                assembly::ObservationRequest {
                    device: DeviceId::Uart,
                    output: assembly::OutputKind::Bytes(device::ByteStreamKind::SerialTx),
                    byte_limit: 1
                };
                2
            ],
        ] {
            assert!(assemble_with_observation(
                &candidate,
                &ObservationPolicy {
                    routes,
                    ..Default::default()
                }
            )
            .is_err());
            assert_eq!(temp.entries(), before);
            assert_eq!(std::fs::read(&journal).ok(), bytes);
        }
    }
}

#[cfg(unix)]
#[test]
fn journal_config_refuses_creation_temp_names_through_symlinks() {
    for reserved_link in [false, true] {
        let temp = Temp::new();
        let mut candidate = config(&temp, false, 512);
        let normal = temp.path("other.upfj");
        let reserved = temp.path("disk.upfj.create-7.tmp");
        let path = if reserved_link {
            std::fs::write(&normal, b"foreign file").unwrap();
            std::os::unix::fs::symlink(&normal, &reserved).unwrap();
            &reserved
        } else {
            std::fs::write(&reserved, b"foreign file").unwrap();
            std::os::unix::fs::symlink(&reserved, &normal).unwrap();
            &normal
        };
        candidate.drives[0].write = WriteMode::journal(path);
        let before = temp.entries();
        let error = candidate.validate().unwrap_err().to_string();
        assert!(
            error.contains("drive 0") && error.contains("creation-temporary"),
            "{error}"
        );
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(assemble(&candidate).is_err());
        assert_eq!(temp.entries(), before);
        assert_eq!(std::fs::read(path).unwrap(), b"foreign file");
    }
}

#[test]
fn nested_creation_names_are_refused_by_assembly() {
    for name in ["disk.upfj.create-x.upfj", "disk.create-", ".create-hidden"] {
        let temp = Temp::new();
        let mut candidate = config(&temp, false, 512);
        let path = temp.path(name);
        candidate.drives[0].write = WriteMode::journal(&path);
        let before = temp.entries();
        let error = candidate.validate().unwrap_err().to_string();
        assert!(error.contains("drive 0") && error.contains(name), "{error}");
        assert!(error.contains("creation-temporary"), "{error}");
        assert!(assemble(&candidate).is_err());
        assert_eq!(temp.entries(), before);
    }
}
