// SPDX-License-Identifier: AGPL-3.0-or-later
//! The diagnostic report and its code trail change nothing the guest does
//! (`backlog/docs/design/observation.md` §Diagnostic report of a running
//! machine).
//!
//! The open-BIOS FreeDOS boot of `freedos_boot_golden` runs twice on the
//! reference `at-freedos` machine, polling for FreeCOM's date prompt in the
//! same one-million-step chunks as `pc_harness::run_freedos_config`: once as
//! the golden runs it, and once with the code trail enabled and a full
//! diagnostic report taken at every poll boundary. Both runs must stop at the
//! same retired count and virtual time, show the golden's MDA screen, emit the
//! same POST stream and leave every device with the same snapshot. The
//! wall time of each run is printed for the record.
//!
//! Marked `#[ignore]` because it boots FreeDOS twice.

use assembly::{
    AssembledMachine, DeviceId, ObservationPolicy, ObservationRequest, OutputKind, RunPolicy,
};
use video::Frame;
#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/session.rs"]
#[allow(dead_code)]
mod session;

struct Boot {
    retired: u64,
    virtual_time: u64,
    mda: String,
    post: Vec<u8>,
    devices: Vec<Vec<u8>>,
    reports: usize,
    trail_samples: u64,
    elapsed: std::time::Duration,
}

fn assembled() -> AssembledMachine {
    let config = machine_file::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml"),
    )
    .unwrap();
    let mut policy = ObservationPolicy {
        trace: device::TracePolicy::Off,
        ..Default::default()
    };
    for device in [DeviceId::Vga, DeviceId::Mda] {
        policy.routes.push(ObservationRequest {
            device,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        });
    }
    for (device, kind) in [
        (DeviceId::Uart, device::ByteStreamKind::SerialTx),
        (DeviceId::Post, device::ByteStreamKind::PostCheckpoints),
    ] {
        policy.routes.push(ObservationRequest {
            device,
            output: OutputKind::Bytes(kind),
            byte_limit: 1024 * 1024,
        });
    }
    assembly::assemble_with_observation(&config, &policy).unwrap()
}

fn mda(assembled: &AssembledMachine) -> String {
    let Frame::Text(grid) = assembled.capture_text(DeviceId::Mda).unwrap().frame else {
        panic!("MDA text required")
    };
    grid.to_dump()
}

/// Boot to the date prompt as `run_freedos_config` polls for it; with
/// `diagnose`, enable the trail and take a report at every poll boundary.
fn boot(diagnose: bool) -> Boot {
    let mut assembled = assembled();
    if diagnose {
        assembled.machine.enable_code_trail();
    }
    let started = std::time::Instant::now();
    let mut reports = 0;
    loop {
        let target = assembled.machine.virtual_time() + pc_harness::DOS_INTERACTIVE_POLL_STEP;
        let cap = target.min(pc_harness::BIOS_MAX_RETIRED);
        let outcome = assembled
            .run_session(&RunPolicy::natural(
                Some(cap),
                Some(pc_harness::BIOS_IDLE_LIMIT),
            ))
            .unwrap();
        if diagnose {
            let report = assembled.diagnostic_report("running").to_string();
            assert!(
                report.contains("\ntrail    cs:ip after the last "),
                "{report}"
            );
            reports += 1;
        }
        if outcome == machine::RunOutcome::Halted
            || mda(&assembled).contains(pc_harness::DOS_DATE_PROMPT)
            || cap >= pc_harness::BIOS_MAX_RETIRED
        {
            break;
        }
    }
    let elapsed = started.elapsed();
    let capture = assembled.capture_text(DeviceId::Vga).unwrap();
    let post = assembled
        .observation
        .drain_bytes(DeviceId::Post, device::ByteStreamKind::PostCheckpoints)
        .unwrap();
    let mda = mda(&assembled);
    let trail_samples = assembled
        .machine
        .code_trail()
        .map_or(0, machine::CodeTrail::samples);
    let bus = assembled.machine.bus_mut();
    bus.settle();
    let devices = (0..)
        .map_while(|index| bus.device(index).map(|device| device.save()))
        .collect();
    Boot {
        retired: capture.retired,
        virtual_time: capture.virtual_time,
        mda,
        post,
        devices,
        reports,
        trail_samples,
        elapsed,
    }
}

#[ignore = "slow: boots FreeDOS twice on the open BIOS"]
#[test]
fn the_code_trail_and_reports_leave_the_freedos_boot_golden_unchanged() {
    let (Some(_rom), Some(_img)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
    ) else {
        return;
    };
    let plain = boot(false);
    let diagnosed = boot(true);
    eprintln!(
        "plain boot {:?}; with the trail and {} reports {:?} ({} trail samples)",
        plain.elapsed, diagnosed.reports, diagnosed.elapsed, diagnosed.trail_samples
    );
    // The numbers of `freedos_boot_golden`.
    for run in [&plain, &diagnosed] {
        assert_eq!((run.retired, run.virtual_time), (8_800_473, 10_000_000));
        assert_eq!(session::screen_hash(&run.mda), 0x83ffbc2844f28889);
        assert_eq!(session::logical_hash(&run.mda), 0xc46f0c147c881837);
    }
    assert_eq!(plain.post, diagnosed.post);
    assert_eq!(plain.devices, diagnosed.devices);
    assert_eq!(diagnosed.reports, 10);
    assert!(diagnosed.trail_samples > 0);
}
