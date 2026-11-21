// SPDX-License-Identifier: AGPL-3.0-or-later
//! Production timing regressions, Intel 80286 Programmer's Reference Manual,
//! string instructions and interrupt recognition (§6 of the scheduler design).

use assembly::at_boot;
use cpu::Cpu;
use machine::RunOutcome;
use machine_file::assemble;

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
