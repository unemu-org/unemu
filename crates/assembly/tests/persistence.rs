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
    out(&mut program, 0x3F7, 2);
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
    for count in [512, 17, 1024, 1024 + 17] {
        let temp = Temp::new();
        let mut journal = config(&temp, true, count);
        let mut guest = vec![0xFA, 0xFC];
        out(&mut guest, 0x3F2, 0x1C);
        out(&mut guest, 0x3F7, 2);
        dma(&mut guest, true, count);
        // EOT beyond the physical track forces an abnormal result after a committed sector.
        for value in [5, 0, 0, 0, 9, 2, 10, 0x1B, 0xFF] {
            out(&mut guest, 0x3F5, value);
        }
        guest.extend([0xBF, 0, 0x10]);
        for _ in 0..7 {
            guest.extend([0xEC, 0xAA]);
        }
        dma(&mut guest, true, count);
        for value in [5, 0, 0, 0, 1, 2, 4, 0x1B, 0xFF] {
            out(&mut guest, 0x3F5, value);
        }
        guest.extend([0xBF, 0x10, 0x10]);
        for _ in 0..7 {
            guest.extend([0xEC, 0xAA]);
        }
        // Capture the registers outside CpuReg through guest stores, including FLAGS.
        guest.extend([
            0x9C, 0x8F, 0x06, 0x20, 0x10, 0x8C, 0x1E, 0x22, 0x10, 0x8C, 0x06, 0x24, 0x10, 0x8C,
            0x16, 0x26, 0x10, 0xF4,
        ]);
        journal.images[0].source = ImageSource::Inline(guest);
        let payload: Vec<_> = (0..count).map(|i| (i as u8) ^ 0xC7).collect();
        journal.images[1].source = ImageSource::Inline(payload.clone());
        let mut volatile = journal.clone();
        volatile.drives[0].write = WriteMode::Volatile;
        let observation = ObservationPolicy {
            trace: device::TracePolicy::Ring { capacity: 4096 },
            ..Default::default()
        };
        let mut a = assemble_with_observation(&journal, &observation).unwrap();
        let mut b = assemble_with_observation(&volatile, &observation).unwrap();
        let mut halted = false;
        for cap in (1..=2000).step_by(7) {
            let policy = RunPolicy::natural(Some(cap), None);
            let left = a.run_session(&policy).unwrap();
            let right = b.run_session(&policy).unwrap();
            assert_eq!(left, right);
            assert_same_guest_state(&mut a, &mut b);
            if left == machine::RunOutcome::Halted {
                halted = true;
                break;
            }
        }
        assert!(halted, "guest must complete all commands");
        let mut result = [0; 7];
        a.machine.cpu().read_mem(0x1000, &mut result).unwrap();
        if count > 512 {
            assert_eq!(
                &result[..2],
                &[0x40, 4],
                "invalid next sector after committed data"
            );
        } else {
            assert_eq!(&result[..2], &[0, 0]);
        }
        a.machine.cpu().read_mem(0x1010, &mut result).unwrap();
        assert_eq!(&result[..2], &[0, 0], "multi-sector/partial terminal count");
        let image = controller_image(&mut a);
        assert_eq!(image, controller_image(&mut b));
        let mut expected = vec![0; image.len()];
        expected[..payload.len()].copy_from_slice(&payload);
        let last_sector = 8 * 512;
        expected[last_sector..last_sector + usize::from(count.min(512))]
            .copy_from_slice(&payload[..usize::from(count.min(512))]);
        assert_eq!(
            image, expected,
            "complete image, including untouched sectors and padding"
        );
        a.finish_persistence().unwrap();
        drop(a);
        let base = std::fs::read(temp.path("base.img")).unwrap();
        let replay = media::scan(
            &std::fs::read(temp.path("disk.upfj")).unwrap(),
            &base,
            base.len() as u64,
        );
        assert_eq!(replay.report.status, media::Status::Valid);
        assert_eq!(replay.image.as_deref(), Some(image.as_slice()));
        let reloaded = media::JournalMedia::open(
            &temp.path("disk.upfj"),
            &base,
            base.len() as u64,
            media::SinkStatus::default(),
        )
        .unwrap();
        assert_eq!(reloaded.image(), image);
        assert_eq!(std::fs::read(temp.path("base.img")).unwrap(), base);
    }
}

fn assert_same_guest_state(a: &mut AssembledMachine, b: &mut AssembledMachine) {
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
        a.machine.cpu().has_pending_irq(),
        b.machine.cpu().has_pending_irq()
    );
    assert_eq!(
        a.machine.cpu().completed_steps(),
        b.machine.cpu().completed_steps()
    );
    assert_eq!(a.machine.cpu().retired(), b.machine.cpu().retired());
    assert_eq!(a.machine.virtual_time(), b.machine.virtual_time());
    assert_eq!(a.machine.clock(), b.machine.clock());
    assert_eq!(a.machine.bus().open_bus_trace_dropped(), 0);
    assert_eq!(b.machine.bus().open_bus_trace_dropped(), 0);
    assert_eq!(
        a.machine.bus_mut().take_open_bus_trace(),
        b.machine.bus_mut().take_open_bus_trace()
    );
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
        assert_eq!(a.machine.bus().device(index).unwrap().trace_dropped(), 0);
        assert_eq!(b.machine.bus().device(other).unwrap().trace_dropped(), 0);
        assert_eq!(
            a.machine.bus_mut().device_mut(index).unwrap().take_trace(),
            b.machine.bus_mut().device_mut(other).unwrap().take_trace()
        );
    }
}

fn controller_image(assembled: &mut AssembledMachine) -> Vec<u8> {
    let index = assembled.device_index[&DeviceId::Fdc];
    let fdc = assembled.machine.bus_mut().device_mut(index).unwrap();
    let saved = fdc.save();
    fdc.reset();
    fdc.port_write(0x3F2, 1, 0x1C).unwrap();
    fdc.port_write(0x3F7, 1, 2).unwrap();
    let mut image = Vec::new();
    for cylinder in 0u8..40 {
        for head in 0..2 {
            for value in [6, head << 2, cylinder, head, 1, 2, 9, 0x1B, 0xFF] {
                fdc.port_write(0x3F5, 1, value.into()).unwrap();
            }
            let request = fdc.take_dma_request().unwrap();
            let device::DmaOp::ToMemory(bytes) = request.op else {
                panic!("READ DATA must supply media bytes");
            };
            assert_eq!(bytes.len(), 9 * 512);
            fdc.dma_complete(device::DmaOutcome::ToMemory {
                transferred: bytes.len(),
                terminal_count: true,
            });
            image.extend(bytes);
            for _ in 0..7 {
                fdc.port_read(0x3F5, 1).unwrap();
            }
        }
    }
    fdc.restore(&saved).unwrap();
    image
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
                bytes_per_sector: 256,
            },
        },
    ] {
        invalid = original.clone();
        invalid.drives[0].kind = kind;
        assert!(invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("512-byte sectors"));
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
    assembled.persistence[0]
        .checkpoint(&mut assembled.machine)
        .unwrap();
    assert_eq!(
        report(&temp.path("disk.upfj"), &temp.path("base.img")).generations[0].reason,
        ClosingReason::Checkpoint
    );
    assembled.finish_persistence().unwrap();
    assembled.finish_persistence().unwrap();
    assert!(assembled.persistence[0]
        .checkpoint(&mut assembled.machine)
        .is_err());
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
    assert!(assembled.persistence[0]
        .checkpoint(&mut assembled.machine)
        .is_err());
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
    // Keep the unsupported-opcode host error separate from the 80286's
    // in-guest exception delivery, as in the frontend error-path fixtures.
    config.devices.retain(|device| {
        !matches!(
            device.kind,
            assembly::DeviceKind::Keyboard(_) | assembly::DeviceKind::SysCtrlPortA
        )
    });
    let ImageSource::Inline(program) = &mut config.images[0].source else {
        unreachable!()
    };
    program.pop();
    program.extend([0x0F, 0x05]);
    let mut assembled = assemble(&config).unwrap();
    let error = assembled
        .run_session(&RunPolicy::natural(Some(10_000), None))
        .unwrap_err();
    assert!(matches!(
        error,
        assembly::RunError::Machine(machine::MachineError::Cpu(
            cpu::CpuError::UnsupportedInstruction { .. }
        ))
    ));
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
        assert!(error.contains("reserved sidecar path"), "{error}");
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
            error.contains("drive 0") && error.contains("reserved sidecar path"),
            "{error}"
        );
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
        assert!(error.contains("drive 0"), "{error}");
        assert!(error.contains("reserved sidecar path"), "{error}");
        assert!(assemble(&candidate).is_err());
        assert_eq!(temp.entries(), before);
    }
}

#[test]
fn assembly_passes_retention_and_reserves_retention_temporary_names() {
    let temp = Temp::new();
    let mut candidate = config(&temp, false, 512);
    let path = temp.path("disk.upfj");
    candidate.drives[0].write = WriteMode::Journal {
        path: path.clone(),
        keep_generations: 2,
    };
    let base = std::fs::read(temp.path("base.img")).unwrap();
    let mut machine = assemble(&candidate).unwrap();
    for _ in 0..4 {
        machine.persistence[0]
            .checkpoint(&mut machine.machine)
            .unwrap();
    }
    machine.finish_persistence().unwrap();
    drop(machine);
    let result = media::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
    assert_eq!(result.report.status, media::Status::Valid);
    assert_eq!(result.report.header.unwrap().anchor_generation, 4);
    assert_eq!(
        result
            .report
            .generations
            .iter()
            .map(|g| g.id)
            .collect::<Vec<_>>(),
        [4, 5]
    );
    assert_eq!(result.image.unwrap(), base);
    assert_eq!(std::fs::read(temp.path("base.img")).unwrap(), base);
    for marker in ["retain", "export"] {
        candidate.drives[0].write =
            WriteMode::journal(temp.path(&format!("disk.upfj.{marker}-1.tmp")));
        assert!(candidate.validate().is_err());
        assert!(assemble(&candidate).is_err());
    }
    candidate.drives[0].write = WriteMode::journal(&path);
    let reserved = temp.path("disk.upfj.retain-1.tmp");
    std::fs::write(&reserved, &base).unwrap();
    candidate.drives[0].image = ImageSource::File(reserved);
    assert!(candidate.validate().is_err());
}

#[cfg(feature = "test-support")]
#[test]
fn finish_attempts_every_drive_and_returns_the_first_real_error() {
    for latched in [false, true] {
        for both_fail in [false, true] {
            let temp = Temp::new();
            let mut config = config(&temp, false, 512);
            let mut second = config.drives[0].clone();
            second.unit = 1;
            second.write = WriteMode::journal(temp.path("second.upfj"));
            config.drives.push(second);
            let mut machine = assemble(&config).unwrap();
            machine.persistence[0].fail_output();
            if latched {
                assert!(machine.persistence[0]
                    .checkpoint(&mut machine.machine)
                    .is_err());
            }
            if both_fail {
                machine.persistence[1].fail_output();
            }
            assert_eq!(machine.finish_persistence().unwrap_err().drive_index, 0);
            if both_fail {
                assert!(machine.persistence[1].status().is_some());
            } else {
                let report = report(&temp.path("second.upfj"), &temp.path("base.img"));
                assert_eq!(report.generations.len(), 1);
                assert_eq!(report.generations[0].reason, ClosingReason::SessionEnd);
            }
        }
    }
}

#[test]
fn retention_reserved_inputs_are_refused_even_for_other_or_volatile_journals() {
    for mode in [
        WriteMode::Volatile,
        WriteMode::ReadOnly,
        WriteMode::journal("unused.upfj"),
    ] {
        let temp = Temp::new();
        let mut config = config(&temp, false, 512);
        let reserved = temp.path("other.upfj.retain-7.tmp");
        std::fs::rename(temp.path("base.img"), &reserved).unwrap();
        config.drives[0].image = ImageSource::File(reserved.clone());
        config.drives[0].write = mode;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("retention-temporary"));
        assert!(assemble(&config).is_err());
        assert!(reserved.exists());
    }
}

#[test]
fn reserved_input_errors_name_the_owning_drive_and_firmware_image() {
    let temp = Temp::new();
    let mut drives = config(&temp, false, 512);
    drives.drives[0].write = WriteMode::Volatile;
    let mut second = drives.drives[0].clone();
    second.unit = 1;
    let reserved = temp.path("other.upfj.retain-7.tmp");
    std::fs::copy(temp.path("base.img"), &reserved).unwrap();
    second.image = ImageSource::File(reserved.clone());
    drives.drives.push(second);
    let error = drives.validate().unwrap_err();
    assert!(
        matches!(&error,assembly::ConfigError::DriveInput {drive_index:1,path,..} if path==&reserved)
    );
    assert_eq!(
        error.to_string(),
        format!(
            "drive 1 input {}: uses a reserved retention-temporary name",
            reserved.display()
        )
    );
    assert!(
        !error.to_string().contains("drive 0")
            && !error.to_string().contains("journal configuration")
    );
    for (kind, role) in [
        (assembly::ImageKind::Rom, "firmware"),
        (assembly::ImageKind::RamLoad, "RAM-load"),
    ] {
        let mut images = config(&temp, false, 512);
        images.drives[0].write = WriteMode::Volatile;
        let index = images.images.len();
        images.images.push(assembly::LoadImage {
            base: 0x2000,
            source: ImageSource::File(reserved.clone()),
            kind,
            provides: vec![],
        });
        let error = images.validate().unwrap_err();
        assert!(
            matches!(&error,assembly::ConfigError::ImageInput {image_index,role:r,path,..} if *image_index==index && *r==role && path==&reserved)
        );
        assert_eq!(
            error.to_string(),
            format!(
                "{role} image {}: uses a reserved retention-temporary name",
                reserved.display()
            )
        );
        assert!(
            !error.to_string().contains("drive 0")
                && !error.to_string().contains("journal configuration")
        );
    }
}

#[cfg(feature = "test-support")]
#[test]
fn uncertain_checkpoint_latches_disabled_appends_and_stops_the_next_boundary() {
    let temp = Temp::new();
    let mut cfg = config(&temp, true, 512);
    let path = temp.path("disk.upfj");
    cfg.drives[0].write = WriteMode::Journal {
        path: path.clone(),
        keep_generations: 1,
    };
    let mut machine = assemble(&cfg).unwrap();
    machine.persistence[0]
        .checkpoint(&mut machine.machine)
        .unwrap();
    machine.persistence[0].fail_retention("publication directory sync");
    machine.persistence[0]
        .checkpoint(&mut machine.machine)
        .unwrap();
    assert!(
        machine.persistence[0]
            .maintenance_failure()
            .unwrap()
            .uncertain
    );
    assert!(
        machine.persistence[0].check().is_err(),
        "disabled appends must be latched immediately"
    );
    let journal = std::fs::read(&path).unwrap();
    // Exercise an already-running chunk directly; the checked frontend refuses
    // to begin another chunk once the host persistence latch is set.
    assert_eq!(
        machine.machine.run_bounded(Some(10_000)).unwrap(),
        machine::RunOutcome::Halted
    );
    assert_eq!(readback(&machine), pattern());
    let mut result = [0; 7];
    machine.machine.cpu().read_mem(0x1000, &mut result).unwrap();
    assert_eq!(result[1] & 2, 0);
    assert_eq!(std::fs::read(&path).unwrap(), journal);
    assert!(machine.check_persistence().is_err());
    let retired = machine.machine.cpu().retired();
    let time = machine.machine.virtual_time();
    assert!(matches!(
        machine.run_session(&RunPolicy::natural(Some(20_000), None)),
        Err(assembly::RunError::Persistence(_))
    ));
    assert_eq!(machine.machine.cpu().retired(), retired);
    assert_eq!(machine.machine.virtual_time(), time);
}

#[cfg(unix)]
#[test]
fn assembly_protects_the_inode_of_the_single_loaded_base_read() {
    let temp = Temp::new();
    let config = config(&temp, false, 512);
    let base = temp.path("base.img");
    let alias = temp.path("disk.upfj.retain-0.tmp");
    let orphan = temp.path("disk.upfj.retain-1.tmp");
    let bytes = std::fs::read(&base).unwrap();
    std::fs::hard_link(&base, &alias).unwrap();
    std::fs::write(&orphan, &bytes).unwrap();
    let machine = assemble(&config).unwrap();
    assert_eq!(std::fs::read(&base).unwrap(), bytes);
    assert_eq!(std::fs::read(&alias).unwrap(), bytes);
    assert!(!orphan.exists());
    drop(machine);
}

#[test]
fn rollback_then_reappend_is_visible_through_fresh_guest_dma_reads() {
    use media::offline::{Access, JournalTool};

    let temp = Temp::new();
    for value in [0x11, 0x22, 0x33] {
        let mut config = config(&temp, true, 512);
        config.images[1].source = ImageSource::Inline(vec![value; 512]);
        let mut assembled = assemble(&config).unwrap();
        run(&mut assembled);
        assert_eq!(readback(&assembled), vec![value; 512]);
        assembled.finish_persistence().unwrap();
    }
    let original = std::fs::read(temp.path("disk.upfj")).unwrap();
    {
        let mut tool = JournalTool::open(
            &temp.path("base.img"),
            &temp.path("disk.upfj"),
            Access::Mutate,
        )
        .unwrap();
        tool.rollback(1, &temp.path("backup.upfj")).unwrap();
    }
    assert_eq!(std::fs::read(temp.path("backup.upfj")).unwrap(), original);
    let mut reader = assemble(&config(&temp, false, 512)).unwrap();
    run(&mut reader);
    assert_eq!(readback(&reader), vec![0x11; 512]);
    drop(reader);
    let mut next = config(&temp, true, 512);
    next.images[1].source = ImageSource::Inline(vec![0x55; 512]);
    let mut writer = assemble(&next).unwrap();
    run(&mut writer);
    writer.finish_persistence().unwrap();
    drop(writer);
    let mut reader = assemble(&config(&temp, false, 512)).unwrap();
    run(&mut reader);
    assert_eq!(readback(&reader), vec![0x55; 512]);
    let mut expected = vec![0; 368640];
    expected[..512].fill(0x55);
    assert_eq!(controller_image(&mut reader), expected);
    let closed = report(&temp.path("disk.upfj"), &temp.path("base.img"));
    assert_eq!(
        closed.generations.iter().map(|g| g.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        std::fs::read(temp.path("base.img")).unwrap(),
        vec![0; 368640]
    );
}

#[cfg(feature = "test-support")]
#[test]
fn failed_output_preserves_complete_guest_prefix_and_final_media() {
    let temp = Temp::new();
    let mut journal = config(&temp, true, 17);
    let mut guest = program(true, 17);
    guest.pop();
    guest.extend(program(true, 512));
    journal.images[0].source = ImageSource::Inline(guest);
    let mut volatile = journal.clone();
    volatile.drives[0].write = WriteMode::Volatile;
    let observation = ObservationPolicy {
        trace: device::TracePolicy::Ring { capacity: 4096 },
        ..Default::default()
    };
    let mut a = assemble_with_observation(&journal, &observation).unwrap();
    let mut b = assemble_with_observation(&volatile, &observation).unwrap();
    a.persistence[0].fail_output();
    let policy = RunPolicy::natural(Some(10_000), None);
    assert!(matches!(
        a.run_session(&policy),
        Err(assembly::RunError::Persistence(_))
    ));
    assert_eq!(b.run_session(&policy).unwrap(), machine::RunOutcome::Halted);
    assert_same_guest_state(&mut a, &mut b);
    assert_eq!(
        readback(&a),
        pattern(),
        "post-error command must still replace padded data"
    );
    assert_eq!(controller_image(&mut a), controller_image(&mut b));
    let before = a.machine.virtual_time();
    assert!(a
        .run_session(&RunPolicy::natural(Some(20_000), None))
        .is_err());
    assert_eq!(a.machine.virtual_time(), before);
    assert!(a.finish_persistence().is_err());
    assert_eq!(
        std::fs::metadata(temp.path("disk.upfj")).unwrap().len(),
        media::HEADER_LEN as u64
    );
}

#[test]
fn identical_guest_sessions_produce_byte_identical_journals() {
    let first = Temp::new();
    let second = Temp::new();
    for count in [512, 17] {
        let journals = [&first, &second].map(|temp| {
            let mut assembled = assemble(&config(temp, true, count)).unwrap();
            run(&mut assembled);
            let mut expected = pattern();
            expected[usize::from(count)..].fill(0);
            assert_eq!(readback(&assembled), expected);
            assembled.persistence[0]
                .checkpoint(&mut assembled.machine)
                .unwrap();
            assembled.finish_persistence().unwrap();
            drop(assembled);
            std::fs::read(temp.path("disk.upfj")).unwrap()
        });
        assert_eq!(
            journals[0], journals[1],
            "journal bytes must not depend on the session's directory"
        );
    }
    assert_eq!(
        std::fs::read(first.path("base.img")).unwrap(),
        std::fs::read(second.path("base.img")).unwrap()
    );
}

#[test]
fn reserved_journal_sidecars_are_usage_errors_before_assembly_opens_outputs() {
    let temp = Temp::new();
    let mut machine = config(&temp, false, 512);
    for name in [
        "disk.lock",
        "disk_LOCK",
        "disk.CREATE-0.TMP",
        "disk.RETAIN-1.TMP",
    ] {
        let path = temp.path(name);
        machine.drives[0].write = WriteMode::journal(&path);
        let error = machine.validate().unwrap_err();
        assert!(
            matches!(error, assembly::ConfigError::Journal { ref message, .. } if message == "reserved sidecar path")
        );
        assert!(assemble(&machine)
            .err()
            .unwrap()
            .to_string()
            .contains("reserved sidecar path"));
        assert!(!path.exists());
        assert!(!temp.path(&format!("{name}.lock")).exists());
    }
    for name in [
        format!("{}.upfj", "a".repeat(64)),
        "games-install-1.upfj".into(),
    ] {
        let path = temp.path(&name);
        machine.drives[0].write = WriteMode::journal(&path);
        machine.validate().unwrap();
        assemble(&machine).unwrap().finish_persistence().unwrap();
    }
}
