// SPDX-License-Identifier: AGPL-3.0-or-later
//! Raw PC runs belong to the standalone core; core-contract.md §12.9.

#[test]
fn raw_commands_are_unknown_and_print_frontend_usage() {
    let mut usage = Vec::new();
    assert_eq!(
        upcli::command::run(&[], &mut usage, &mut std::io::sink()),
        0
    );
    for args in [
        vec!["machine", "check", "missing.toml"],
        vec!["machine", "check", "missing.toml", "--rom-dir", "roms"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_unemu"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let mut expected = usage.clone();
        expected.extend_from_slice(b"unknown or extra arguments\n");
        assert_eq!(output.stderr, expected, "{args:?}");
    }
    for args in [
        vec!["--boot"],
        vec!["--bios", "missing.bin"],
        vec!["--allow-unlisted-firmware", "--bios", "missing.bin"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_unemu"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("expected --machine"));
    }
}
