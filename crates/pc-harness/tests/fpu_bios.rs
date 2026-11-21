// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM PC/AT optional coprocessor detection with the open Kiselev BIOS.
//! See `backlog/docs/design/machine-config.md` §CPU model selection.

use assembly::{bios, FpuChip};

#[path = "support/fixtures.rs"]
mod fixtures;

#[test]
#[ignore = "requires the open BIOS fixture and runs its coprocessor detection twice"]
fn kiselev_empty_socket_finishes_the_coprocessor_probe() {
    let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let mut screens = Vec::new();
    for fpu in [FpuChip::I80287, FpuChip::None] {
        let mut config = bios(rom.clone(), Some(pc_harness::bootable_floppy_image()));
        config.cpu.fpu = fpu;
        let policy = assembly::ObservationPolicy {
            routes: vec![assembly::ObservationRequest {
                device: assembly::DeviceId::Mda,
                output: assembly::OutputKind::TextScanout,
                byte_limit: 0,
            }],
            ..Default::default()
        };
        let mut assembled = machine_file::assemble_with_observation(&config, &policy).unwrap();
        assert_eq!(
            assembled
                .run_session(&assembly::RunPolicy::natural(
                    Some(pc_harness::BIOS_GOLDEN_MAX_RETIRED),
                    Some(pc_harness::BIOS_IDLE_LIMIT),
                ))
                .unwrap(),
            machine::RunOutcome::RetiredCapReached
        );
        let video::Frame::Text(grid) = assembled
            .capture_text(assembly::DeviceId::Mda)
            .unwrap()
            .frame
        else {
            panic!("text frame required")
        };
        let screen = grid.rows_as_strings();
        eprintln!("FPU {fpu:?}:\n{}", screen.join("\n"));
        screens.push(screen);
    }
    // The generic present engine already fails this BIOS's 03FFh control-word
    // comparison, so both policies retain its existing "Absent" report.
    assert_eq!(screens[0], screens[1]);
    let report = screens[0]
        .iter()
        .find(|line| line.starts_with("Mathematics Co-processor:"))
        .expect("coprocessor report");
    assert_eq!(report, "Mathematics Co-processor:   Absent");
    eprintln!("Coprocessor report: {report:?} -> {report:?}");
}
