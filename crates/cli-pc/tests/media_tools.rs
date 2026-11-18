// SPDX-License-Identifier: AGPL-3.0-or-later
//! The journal tools' usage errors on the command line with the PC platform
//! compiled in, where `--boot` still runs; persistent-media.md § Offline
//! commands. The journal tools themselves are tested with the neutral
//! command line, in the `cli-core` package.

use cli_pc::command::run;
use std::ffi::OsString;

#[test]
fn usage_errors_are_two_and_boot_still_runs() {
    for arguments in [
        vec!["--unknown"],
        vec!["--boot", "extra"],
        vec!["media"],
        vec!["media", "commit"],
        vec!["media", "compact"],
        vec!["media", "verify"],
        vec!["media", "verify", "--base"],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--repair-tail",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "0",
        ],
        vec![
            "media",
            "generations",
            "--base",
            "a",
            "--journal",
            "b",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "generations",
            "--base",
            "a",
            "--journal",
            "b",
            "--repair-tail",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "-1",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "+1",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "18446744073709551616",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "0",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--base",
            "b",
            "--journal",
            "c",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--repair-tail",
            "--repair-tail",
            "--backup",
            "c",
        ],
    ] {
        let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run(&args, &mut out, &mut err), 2, "{arguments:?}");
        assert!(out.is_empty());
        assert!(String::from_utf8(err).unwrap().contains("usage:"));
    }
    assert_eq!(
        cli_pc::parse(&["--boot".into()]),
        Some(Ok(cli_pc::PcCommand::Boot))
    );
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(run(&["--boot".into()], &mut out, &mut err), 0);
    assert_eq!(out, b".....");
    assert!(err.is_empty());
    out.clear();
    assert_eq!(run(&[], &mut out, &mut err), 0);
    assert!(String::from_utf8(out).unwrap().contains("media verify"));
}
