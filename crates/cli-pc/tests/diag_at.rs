// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu run --machine --diag-at N` writes the machine's diagnostic report
//! to stderr when the run reaches step N, then runs on to its natural stop
//! (commands.md, Build / run / test; `backlog/docs/architecture.md` § Core boundary
//! and host timing). The system ROM is
//! `assembly::test_support::keyboard_masked_halt_rom`, the project's own
//! bytes: it masks IRQ1, disables the 8042's keyboard interface and halts
//! with interrupts disabled.

use std::ffi::OsString;

fn run(extra: &[&str]) -> (u8, String) {
    let temp = machine_file::test_support::TempDir::new("diag-at");
    let rom = assembly::test_support::keyboard_masked_halt_rom();
    std::fs::write(temp.path().join("halt.bin"), &rom).unwrap();
    let machine = temp.path().join("halt.toml");
    std::fs::write(
        &machine,
        format!(
            "schema = 1\n[board]\nmodel = \"ibm-5170\"\n[firmware.bios]\n\
             image = \"halt.bin\"\nsha256 = \"{}\"\n[[card]]\nkind = \"mda\"\n",
            assembly::test_support::fixture_sha256(&rom)
        ),
    )
    .unwrap();
    let mut args: Vec<OsString> = vec![
        "run".into(),
        "--machine".into(),
        machine.into_os_string(),
        "--allow-unlisted-firmware".into(),
        "--no-store".into(),
    ];
    args.extend(extra.iter().map(OsString::from));
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(&args, &mut output, &mut errors);
    (code, String::from_utf8(errors).unwrap())
}

#[test]
fn diag_at_reports_the_running_machine_then_the_run_goes_on() {
    let (code, errors) = run(&["--diag-at", "10"]);
    assert_eq!(code, 0, "{errors}");
    let report = errors
        .find("machine diagnostic report\n")
        .expect("the report is written");
    let stopped = errors
        .find("stopped: ")
        .expect("the run goes on to its stop");
    assert!(report < stopped, "{errors}");
    for line in [
        "retired  10 instructions, 10 steps\n",
        "status   running\n",
        "halted   no\n",
        // After ten instructions the guest has programmed the 8259 but not
        // yet masked IRQ1.
        "pic      master irr 00 isr 00 imr 00 base 08",
        // The core serves the PIC ports while the CPU runs, so a write to
        // them is not a trail boundary: the one sample is where the run
        // stopped for the report.
        "trail    f800:0012\n",
    ] {
        assert!(errors.contains(line), "{line:?} missing from\n{errors}");
    }
    assert!(errors[stopped..].starts_with("stopped: halt"), "{errors}");

    // Without --diag-at the run prints no report and stops the same way.
    let (plain_code, plain) = run(&[]);
    assert_eq!(plain_code, code);
    assert!(!plain.contains("machine diagnostic report"), "{plain}");
    assert_eq!(
        plain.lines().find(|line| line.starts_with("stopped: ")),
        errors.lines().find(|line| line.starts_with("stopped: "))
    );
}

/// The last step the run reaches is reported too: `--diag-at` equal to
/// `--max-steps` writes the report, then the run stops at its budget.
#[test]
fn diag_at_the_step_budget_still_reports() {
    let (code, errors) = run(&["--max-steps", "12", "--diag-at", "12"]);
    assert_eq!(code, 0, "{errors}");
    assert!(
        errors.contains("machine diagnostic report\nretired  12 instructions, 12 steps\n"),
        "{errors}"
    );
    let stopped = errors.lines().find(|line| line.starts_with("stopped: "));
    assert_eq!(stopped, Some("stopped: budget exhausted"), "{errors}");
    // One step past the budget is never reached.
    let (_, beyond) = run(&["--max-steps", "12", "--diag-at", "13"]);
    assert!(!beyond.contains("machine diagnostic report"), "{beyond}");
}
