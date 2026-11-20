// SPDX-License-Identifier: AGPL-3.0-or-later
//! The standalone timer demo's command-line contract; core-contract.md §12.9.

use std::{ffi::OsString, process::Command};

fn run(args: &[OsString], out: &mut Vec<u8>, err: &mut Vec<u8>) -> u8 {
    let output = Command::new(env!("CARGO_BIN_EXE_unemupc"))
        .args(args)
        .output()
        .unwrap();
    out.extend(output.stdout);
    err.extend(output.stderr);
    output.status.code().unwrap().try_into().unwrap()
}

#[test]
fn usage_errors_are_two_and_boot_still_runs() {
    let arguments = ["--boot", "extra"];
    let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(run(&args, &mut out, &mut err), 2, "{arguments:?}");
    assert!(out.is_empty());
    assert!(String::from_utf8(err).unwrap().contains("usage:"));
    assert_eq!(
        unemupc::parse(&["--boot".into()]),
        Some(Ok(unemupc::Command::Boot))
    );
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(run(&["--boot".into()], &mut out, &mut err), 0);
    assert_eq!(out, b".....");
    assert!(err.is_empty());
}
