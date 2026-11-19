// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli run --machine` on a guest stuck delivering exceptions: its handler
//! faults at its first instruction, so no instruction retires and the run's
//! cap, which counts retired instructions, is never reached. The run stops
//! with the guest-stop exit code, names the stall on its `stopped:` line and
//! prints the CPU stop report (commands.md, Build / run / test;
//! `backlog/docs/design/observation.md` §CPU stop report). The system ROM is
//! `assembly::test_support::delivery_stall_rom`, the project's own bytes.

use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn run_stops_a_guest_stuck_in_exception_delivery_with_a_report() {
    let temp = assembly::test_support::TempDir::new("delivery-stall");
    let rom = assembly::test_support::delivery_stall_rom();
    let rom_path = temp.path().join("stall.bin");
    std::fs::write(&rom_path, &rom).unwrap();
    let machine = temp.path().join("stall.toml");
    std::fs::write(
        &machine,
        format!(
            "schema = 1\n[board]\nmodel = \"ibm-5170\"\n[firmware.bios]\n\
             image = \"stall.bin\"\nsha256 = \"{}\"\n[[card]]\nkind = \"mda\"\n",
            assembly::test_support::fixture_sha256(&rom)
        ),
    )
    .unwrap();
    let args: Vec<OsString> = vec![
        "run".into(),
        "--machine".into(),
        machine.into_os_string(),
        "--allow-unlisted-firmware".into(),
        "--no-store".into(),
    ];
    // A run that never returns fails the test at the deadline instead of
    // hanging it.
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let code = cli_pc::command::run(&args, &mut output, &mut errors);
        sender.send((code, errors)).unwrap();
    });
    let (code, errors) = receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("the run returns");
    let errors = String::from_utf8(errors).unwrap();
    assert_eq!(code, 1, "{errors}");
    // The unlisted ROM's warning comes first.
    let mut lines = errors
        .lines()
        .skip_while(|line| line.starts_with("warning: "));
    assert_eq!(
        lines.next(),
        Some(
            "stopped: cpu fault: the processor is stuck delivering exceptions: 11264 \
             deliveries without an instruction, the last to 0008:0040"
        ),
        "{errors}"
    );
    assert_eq!(lines.next(), Some("cpu stop report"), "{errors}");
    for line in [
        "cs:ip    0008:0040 (linear 0f8040)\n",
        "mode     protected, cpl 0, msw fff1, a20 on\n",
        "code     8e d8 f4 00 00 00 00 00 00 00 00 00 00 00 00 00\n",
    ] {
        assert!(errors.contains(line), "{line:?} missing from\n{errors}");
    }
}
