// SPDX-License-Identifier: AGPL-3.0-or-later
//! Production timing regressions, Intel 80286 Programmer's Reference Manual,
//! string instructions and interrupt recognition (§6 of the scheduler design).

#[test]
fn binary_usage_names_the_remaining_demo_and_dos_goldens() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_unemupc"))
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
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_unemupc"))
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
