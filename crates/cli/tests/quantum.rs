// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cooperative yields keep virtual-time caps reachable even without guest I/O (§6).

use std::process::Command;
use std::time::{Duration, Instant};

use assembly::{assemble, at_boot, ClockConfig};
use cpu::{Cpu, CpuStop, X86JitCpu};
use machine::RunOutcome;

// A subprocess lets the parent kill and reap an unbounded CPU run on regression;
// a detached watchdog thread would leave the spinning guest alive in the suite.
fn with_watchdog(name: &str, run: impl FnOnce()) {
    const CHILD_TEST: &str = "UNEMUPC98_QUANTUM_CHILD_TEST";
    if std::env::var(CHILD_TEST).as_deref() == Ok(name) {
        run();
        return;
    }

    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", name, "--nocapture"])
        .env(CHILD_TEST, name)
        .spawn()
        .expect("spawn bounded-run test");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("poll bounded-run test") {
            assert!(status.success(), "{name} failed: {status}");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill unresponsive guest");
            child.wait().expect("reap unresponsive guest");
            panic!("{name} did not return within 10 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn real16_spin_reaches_cap_with_default_clock() {
    with_watchdog("real16_spin_reaches_cap_with_default_clock", || {
        // A two-instruction loop; the self-jump case has its own test below.
        let config = at_boot(vec![0xFB, 0x90, 0xEB, 0xFD]); // STI; NOP; JMP NOP
        let mut assembled = assemble(&config).expect("assemble Real16 machine");
        assert_eq!(
            assembled.machine.run_bounded(Some(32)).expect("run"),
            RunOutcome::RetiredCapReached
        );
        let retired = assembled.machine.cpu().retired();
        assert!(retired >= 32);
        // Each loop block retires two instructions, plus the initial STI.
        assert!(retired <= 32 + 2 * config.clock.quantum.expect("finite quantum") + 1);
        assert_eq!(assembled.machine.virtual_time(), retired);
    });
}

#[test]
fn real16_self_jump_idle_advances_virtual_time() {
    with_watchdog("real16_self_jump_idle_advances_virtual_time", || {
        // `sti; jmp $` is how a real-mode program idles while it waits for an
        // interrupt. Each iteration of the self-jump must count as a retired
        // instruction, otherwise virtual time freezes, no device ticks, and the
        // run never reaches its cap.
        let config = at_boot(vec![0xFB, 0xEB, 0xFE]); // STI; JMP $
        let mut assembled = assemble(&config).expect("assemble Real16 machine");
        assert_eq!(
            assembled.machine.run_bounded(Some(1_000)).expect("run"),
            RunOutcome::RetiredCapReached
        );
        assert!(assembled.machine.virtual_time() >= 1_000);
    });
}

#[test]
fn long64_spin_yields_with_serial_quantum() {
    with_watchdog("long64_spin_yields_with_serial_quantum", || {
        let mut cpu =
            X86JitCpu::new(cli::RAM_BYTES, cli::ENTRY, &[0xEB, 0xFE]).expect("Long64 JMP $");
        assert_eq!(
            cpu.run(cli::QUANTUM).expect("run"),
            CpuStop::BudgetExhausted
        );
    });
}

#[test]
fn default_quanta_match_the_bios_block_budget() {
    assert_eq!(ClockConfig::default().quantum, Some(65_536));
    assert_eq!(cli::QUANTUM, Some(65_536));
}
