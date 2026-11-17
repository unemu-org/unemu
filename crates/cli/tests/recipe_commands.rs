// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli recipe check` (recipes.md §6.11, §6.15): its output and exit codes
//! on the recipe corpus and on usage errors. Reads files only.

use std::ffi::OsString;

const CORPUS: &str = "../../backlog/docs/design/recipe-corpus";

fn upcli(args: &[&str]) -> (u8, String, String) {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli::command::run(&args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

#[test]
fn the_ibm_pc_recipes_of_the_corpus_pass_with_exit_0() {
    let folders = [
        "freedos-1.4-floppy",
        "freedos-1.4-livecd",
        "kenslab-2.01",
        "azuramon",
        "windows-3.0",
        "unemu-at-bios",
        "ibm-5170-bios",
    ]
    .map(|f| format!("{CORPUS}/{f}"));
    let mut args = vec!["recipe", "check"];
    args.extend(folders.iter().map(String::as_str));
    let (code, output, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.ends_with("12 files checked, 0 findings\n"),
        "{output}"
    );
    assert!(output.contains(&format!(
        "{CORPUS}/kenslab-2.01/kenslab-2.01.media.toml: ok, media recipe; required inputs: labfull, licenses-lf\n"
    )));
}

#[test]
fn the_whole_corpus_has_one_finding_per_recipe_of_a_platform_without_provider() {
    let (code, output, _) = upcli(&["recipe", "check", CORPUS]);
    assert_eq!(code, 1);
    let findings: Vec<&str> = output.lines().filter(|l| l.contains(": error[")).collect();
    assert_eq!(findings.len(), 5, "{output}");
    assert_eq!(
        findings[0],
        format!("{CORPUS}/amiga-adf/amdemo.media.toml:9:12: error[6.11.7] media.platform: no provider of platform `amiga` in this build; it has ibm-pc")
    );
    assert!(findings[1..]
        .iter()
        .all(|f| f.contains("/contra-nes/") && f.contains("platform `nes`")));
    assert!(
        output.ends_with("18 files checked, 5 findings\n"),
        "{output}"
    );
}

#[test]
fn usage_errors_and_unreadable_files_exit_2() {
    for args in [
        &["recipe"][..],
        &["recipe", "check"],
        &["recipe", "show", "x.media.toml"],
        &["recipe", "check", "--json", CORPUS],
    ] {
        let (code, output, errors) = upcli(args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(output, "");
        assert!(
            errors.starts_with("usage: upcli recipe check FILE|FOLDER..."),
            "{errors}"
        );
    }
    let (code, _, errors) = upcli(&["recipe", "check", "no-such.media.toml"]);
    assert_eq!(code, 2);
    assert!(
        errors.starts_with("upcli recipe check: no-such.media.toml: "),
        "{errors}"
    );
}

#[test]
fn the_usage_names_the_command() {
    let (code, output, _) = upcli(&[]);
    assert_eq!(code, 0);
    assert!(output.contains("upcli recipe check FILE|FOLDER..."));
}
