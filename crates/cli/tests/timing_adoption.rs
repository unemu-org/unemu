// SPDX-License-Identifier: AGPL-3.0-or-later
//! Production timing regressions, Intel 80286 Programmer's Reference Manual,
//! string instructions and interrupt recognition (§6 of the scheduler design).

use assembly::{assemble, at_boot};
use cpu::Cpu;
use machine::RunOutcome;

#[test]
fn default_machine_counts_rep_iterations_and_stops_at_exact_cap() {
    // MOV CX, 100; REP STOSB; CLI; HLT. A short cap must stop inside REP.
    let config = at_boot(vec![0xB9, 100, 0, 0xF3, 0xAA, 0xFA, 0xF4]);
    let mut assembled = assemble(&config).unwrap();
    assert_eq!(
        assembled.machine.run_bounded(Some(7)).unwrap(),
        RunOutcome::RetiredCapReached
    );
    assert_eq!(assembled.machine.virtual_time(), 7);
    assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rcx), 94);
    assert_eq!(assembled.machine.cpu().retired(), 1);
}

#[test]
fn binary_usage_names_the_remaining_demo_and_dos_goldens() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_upcli"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let usage = String::from_utf8(output.stdout).unwrap();
    assert!(usage.contains("--boot (Real16 timer demo)"));
    assert!(usage.contains("crates/pc-harness/tests/*dos*golden.rs"));
}

#[test]
fn unknown_or_extra_arguments_fail_with_usage_on_stderr() {
    for args in [
        vec!["--unknown"],
        vec!["image.bin"],
        vec!["--boot", "extra"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_upcli"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("usage:"));
    }
}
