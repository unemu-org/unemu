// SPDX-License-Identifier: AGPL-3.0-or-later
//! Raw PC runs belong to the standalone core; core-contract.md §12.9.

#[test]
fn raw_commands_are_unknown_and_print_frontend_usage() {
    let usage = std::process::Command::new(env!("CARGO_BIN_EXE_upcli"))
        .output()
        .unwrap();
    for args in [
        vec!["--boot"],
        vec!["--bios", "missing.bin"],
        vec!["machine", "check", "missing.toml"],
        vec!["--allow-unlisted-firmware", "--bios", "missing.bin"],
        vec!["machine", "check", "missing.toml", "--rom-dir", "roms"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_upcli"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let mut expected = usage.stdout.clone();
        expected.extend_from_slice(b"unknown or extra arguments\n");
        assert_eq!(output.stderr, expected, "{args:?}");
    }
}
