// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli recipe check` and `upcli recipe show` (recipes.md §6.11, §6.15,
//! §6.16): their output and exit codes on the recipe corpus and on usage
//! errors. Reads files only.

use std::ffi::OsString;

const CORPUS: &str = "../../backlog/docs/design/recipe-corpus";

fn upcli(args: &[&str]) -> (u8, String, String) {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(&args, &mut output, &mut errors);
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
        &["recipe", "check", "--json", CORPUS],
        &["recipe", "show"],
        &["recipe", "show", "a.media.toml", "b.media.toml"],
        &["recipe", "show", "--json"],
        &["recipe", "list", CORPUS],
    ] {
        let (code, output, errors) = upcli(args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(output, "");
        assert!(
            errors.starts_with(
                "usage: upcli recipe check FILE|FOLDER...\n       upcli recipe show FILE\n"
            ),
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
    assert!(output.contains("upcli recipe show FILE"));
}

/// Ken's Labyrinth 2.01 merged with its work file: the shared fields carry
/// a comment naming the work file, the release's own do not, and the
/// labels are the work's two, then the release's own.
#[test]
fn show_prints_the_merged_record_and_marks_what_came_from_the_work_file() {
    let recipe = format!("{CORPUS}/kenslab-2.01/kenslab-2.01.media.toml");
    let (code, output, errors) = upcli(&["recipe", "show", &recipe]);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let description = "Ken's Labyrinth 2.01, the full version of Ken Silverman's first-person shooter, published by Epic MegaGames in 1993: the aliens of the planet Zogar have taken your dog Sparky, and you go through their labyrinth in three episodes of ten levels. One 3.5-inch 1.44M diskette holds all 19 files of the author's LABFULL.ZIP, unmodified: the game (KEN.EXE); KENSBFIX.EXE, the author's 2008 build that fixes the 'Digitized Sound' crash and replaces KEN.EXE when Digitized Sound is enabled; SETUP.EXE; the game data; the manual (HELPME.DOC); the hint manual (HINTS.TXT); and four bonus saved games. Copy them to a hard disk, run SETUP, then KEN. Released by the author as freeware, with Epic MegaGames' permission, on 16 November 1999: it may be copied freely, but not sold, and only with all of its files (README.TXT, the set's licence text).";
    let synopsis = "Ken Silverman's first-person shooter of 1993: the aliens of the planet Zogar have taken your dog Sparky, and you go through their labyrinth in three episodes of ten levels.";
    let work = "\t# from kenslab.work.toml";
    assert_eq!(
        output,
        format!(
            "unemu/media/ibm-pc/kenslab:2.01-r1\tdraft\tibm-pc\n\
             title\t\"Ken's Labyrinth\"{work}\n\
             publisher\t\"Epic MegaGames\"{work}\n\
             developer\t\"Ken Silverman\"{work}\n\
             developer\t\"Andy Cotter\"{work}\n\
             year\t1993\n\
             genre\t\"first-person shooter\"{work}\n\
             platform\t\"DOS\"{work}\n\
             description\t{description:?}\n\
             version\t\"2.01\"\n\
             edition\t\"Full Version\"\n\
             need\tcpu\t286+{work}\n\
             need\tsound\tsb,adlib,pc-speaker,none{work}\n\
             need\tvideo\tvga{work}\n\
             work\t\"kenslab\"\n\
             synopsis\t{synopsis:?}{work}\n\
             set-label\t\"game\"{work}\n\
             set-label\t\"first-person-shooter\"{work}\n\
             set-label\t\"freeware\"\n"
        )
    );
}

/// A recipe that does not pass the checks shows their report, exit 1: the
/// NES recipes have no provider in this build.
#[test]
fn show_refuses_a_recipe_with_findings_with_the_checks_report() {
    let recipe = format!("{CORPUS}/contra-nes/contra-europe.media.toml");
    let (code, output, errors) = upcli(&["recipe", "show", &recipe]);
    assert_eq!((code, errors.as_str()), (1, ""));
    assert_eq!(
        output,
        format!(
            "{recipe}:10:12: error[6.11.7] media.platform: no provider of platform `nes` in this build; it has ibm-pc\n\
             {recipe}: 1 finding, media recipe; required inputs: cart\n\
             1 file checked, 1 finding\n"
        )
    );
}

/// A file without a title record, a folder or a file that cannot be read
/// is exit 2.
#[test]
fn show_takes_one_readable_media_recipe() {
    let firmware = format!("{CORPUS}/unemu-at-bios/unemu-at-bios-1.0.2-unemu.3.firmware.toml");
    let work = format!("{CORPUS}/kenslab-2.01/kenslab.work.toml");
    let folder = format!("{CORPUS}/kenslab-2.01");
    for (path, message) in [
        (
            firmware.as_str(),
            "a firmware recipe; show prints the title record of a media recipe",
        ),
        (
            work.as_str(),
            "a work file; show prints the title record of a media recipe",
        ),
        (folder.as_str(), "a folder; show takes one recipe file"),
    ] {
        let (code, output, errors) = upcli(&["recipe", "show", path]);
        assert_eq!((code, output.as_str()), (2, ""), "{path}");
        assert_eq!(errors, format!("upcli recipe show: {path}: {message}\n"));
    }
    let (code, _, errors) = upcli(&["recipe", "show", "no-such.media.toml"]);
    assert_eq!(code, 2);
    assert!(
        errors.starts_with("upcli recipe show: no-such.media.toml: "),
        "{errors}"
    );
}
