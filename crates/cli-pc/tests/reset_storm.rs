// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu run --machine` on a guest that shuts down again right after every
//! reset: each reset takes three steps of virtual time, so the run's cap
//! (20,000,000 steps by default) would take over 6 million resets to reach. The
//! run stops with the guest-stop exit code after the storm's 64th reset,
//! names it on its `stopped:` line and prints the CPU stop report at the
//! last shutdown (commands.md, Build / run / test;
//! `backlog/docs/architecture.md` § Core boundary and host timing). The system ROM is
//! `assembly::test_support::reset_storm_rom`, the
//! project's own bytes.

use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn run_stops_a_guest_that_resets_again_and_again_with_a_report() {
    let temp = machine_file::test_support::TempDir::new("reset-storm");
    let rom = assembly::test_support::reset_storm_rom();
    let rom_path = temp.path().join("storm.bin");
    std::fs::write(&rom_path, &rom).unwrap();
    let machine = temp.path().join("storm.toml");
    std::fs::write(
        &machine,
        format!(
            "schema = 1\n[board]\nmodel = \"ibm-5170\"\n[firmware.bios]\n\
             image = \"storm.bin\"\nsha256 = \"{}\"\n[[card]]\nkind = \"mda\"\n",
            assembly::test_support::RESET_STORM_ROM_SHA256
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
            "stopped: cpu fault: the machine resets again and again: 64 resets within 192 \
             steps, the last shutdown at f800:0007"
        ),
        "{errors}"
    );
    assert_eq!(lines.next(), Some("cpu stop report"), "{errors}");
    for line in [
        "cs:ip    f800:0007 (linear 0f8007)\n",
        "mode     real, cpl 0, msw fff0, a20 on\n",
        "idtr     base 000000 limit 0000\n",
        "code     cc f4 00 00 00 00 00 00 00 00 00 00 00 00 00 00\n",
        "decoded  int3\n",
    ] {
        assert!(errors.contains(line), "{line:?} missing from\n{errors}");
    }
}
