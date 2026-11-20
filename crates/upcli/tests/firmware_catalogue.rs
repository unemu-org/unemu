// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware policy at the CLI boundary; see firmware-and-machine-classes.md §6.

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

#[test]
fn legacy_post_runner_checks_the_catalogue_without_changing_the_bare_rig() {
    let Some(mut rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let real = pc_harness::run_bios_with_cap(&rom, 0);
    assert!(!matches!(
        real.stop,
        pc_harness::StopReason::ConfigFailed { .. }
    ));
    rom[0] ^= 1;
    let changed = pc_harness::run_bios_with_cap(&rom, 0);
    match changed.stop {
        pc_harness::StopReason::ConfigFailed { detail } => {
            assert!(detail.contains("is not a known image for board \"ibm-5170\""));
            assert!(detail.contains("kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh)"));
        }
        stop => panic!("changed BIOS reached execution: {stop:?}"),
    }
}
