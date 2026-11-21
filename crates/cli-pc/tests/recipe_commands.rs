// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu recipe check` and `unemu recipe show` (recipes.md §6.11, §6.15,
//! §6.16): their output and exit codes on the recipe corpus and on usage
//! errors. Uses synthetic draft files and reads the corpus only.

use std::ffi::OsString;

const CORPUS: &str = "../../backlog/docs/design/recipe-corpus";
const TEMPLATE_RECIPES: &str = "../pc-image/data/recipes";

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
        "freedos-1.4-livecd",
        "kenslab-2.01",
        "azuramon",
        "windows-3.0",
        "ibm-5170-bios",
    ]
    .map(|f| format!("{CORPUS}/{f}"));
    let mut args = vec!["recipe", "check"];
    args.extend(folders.iter().map(String::as_str));
    args.push(TEMPLATE_RECIPES);
    let (code, output, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.ends_with("14 files checked, 0 findings\n"),
        "{output}"
    );
    assert!(output.contains(&format!(
        "{CORPUS}/kenslab-2.01/kenslab-2.01.media.toml: ok, media recipe; required inputs: labfull, licenses-lf\n"
    )));
}

#[test]
fn the_whole_corpus_has_one_finding_per_recipe_of_a_platform_without_provider() {
    let (code, output, _) = upcli(&["recipe", "check", CORPUS, TEMPLATE_RECIPES]);
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
        output.ends_with("20 files checked, 5 findings\n"),
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
                "usage: unemu recipe check FILE|FOLDER... [--deny-drift]\n       unemu recipe show FILE\n"
            ),
            "{errors}"
        );
    }
    let (code, _, errors) = upcli(&["recipe", "check", "no-such.media.toml"]);
    assert_eq!(code, 2);
    assert!(
        errors.starts_with("unemu recipe check: no-such.media.toml: "),
        "{errors}"
    );
}

#[test]
fn the_usage_names_the_command() {
    let (code, output, _) = upcli(&[]);
    assert_eq!(code, 0);
    assert!(output.contains("unemu recipe check FILE|FOLDER... [--deny-drift]"));
    assert!(output.contains("unemu recipe show FILE"));
}

/// Ken's Labyrinth 2.01 now has a lock: show prints its pinned record,
/// requirements and work ids, with labels in their merged order.
#[test]
fn show_prints_the_pinned_corpus_record() {
    let recipe = format!("{CORPUS}/kenslab-2.01/kenslab-2.01.media.toml");
    let (code, output, errors) = upcli(&["recipe", "show", &recipe]);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let description = "Ken's Labyrinth 2.01, the full version of Ken Silverman's first-person shooter, published by Epic MegaGames in 1993: the aliens of the planet Zogar have taken your dog Sparky, and you go through their labyrinth in three episodes of ten levels. One 3.5-inch 1.44M diskette holds all 19 files of the author's LABFULL.ZIP, unmodified: the game (KEN.EXE); KENSBFIX.EXE, the author's 2008 build that fixes the 'Digitized Sound' crash and replaces KEN.EXE when Digitized Sound is enabled; SETUP.EXE; the game data; the manual (HELPME.DOC); the hint manual (HINTS.TXT); and four bonus saved games. Copy them to a hard disk, run SETUP, then KEN. Released by the author as freeware, with Epic MegaGames' permission, on 16 November 1999: it may be copied freely, but not sold, and only with all of its files (README.TXT, the set's licence text).";
    let synopsis = "Ken Silverman's 3D arcade game of 1993, with 256-colour graphics, music and digital sound, in three episodes: The Search for Sparky, Sparky's Revenge and Find the Way Home.";
    let requirements = "IBM AT-Compatible computer (386 or 486 highly recommended); VGA Graphics; 512K of total memory (at least 365K available); About 1 megabyte of hard disk space";
    let digest = "sha256:1e34b66a682475182c0092cba2679c8da64e65160f91159a5c9ef3f944a40395";
    assert_eq!(
        output,
        format!(
            "unemu/media/ibm-pc/kenslab:2.01-r1\t{digest}\tibm-pc\n\
             # locked: the record of {digest}, which a build writes; the work file's edits wait for a new revision\n\
             title\t\"Ken's Labyrinth\"\n\
             publisher\t\"Epic MegaGames\"\n\
             developer\t\"Ken Silverman\"\n\
             developer\t\"Andy Cotter\"\n\
             year\t1993\n\
             genre\t\"first-person shooter\"\n\
             platform\t\"DOS\"\n\
             description\t{description:?}\n\
             version\t\"2.01\"\n\
             edition\t\"Full Version\"\n\
             requirements\t{requirements:?}\n\
             need\tcpu\t286+\n\
             need\tinput\tjoystick,mouse,none\n\
             need\tmemory\t512K\n\
             need\tsound\tsb,adlib,none\n\
             need\tvideo\tvga\n\
             work\t\"kenslab\"\n\
             synopsis\t{synopsis:?}\n\
             work-id\tigdb\t\"kens-labyrinth\"\n\
             work-id\tmobygames\t\"kens-labyrinth\"\n\
             work-id\twikidata\t\"Q3814453\"\n\
             set-label\t\"game\"\n\
             set-label\t\"first-person-shooter\"\n\
             set-label\t\"freeware\"\n"
        )
    );
}

#[test]
fn show_marks_only_the_fields_inherited_from_a_draft_work_file() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temp = Temp(std::env::temp_dir().join(format!("recipe-show-draft-{}", std::process::id())));
    std::fs::create_dir_all(&temp.0).unwrap();
    std::fs::write(
        temp.0.join("draft.work.toml"),
        r#"recipe = "1.0"
[work]
id = "draft"
[title]
title = "Lantern Maze"
publishers = ["Work Publisher"]
developers = ["Lantern Studio"]
genres = ["puzzle"]
software_platform = "DOS"
labels = ["game"]
synopsis = "Find a path through a maze of lanterns."
[ids]
wikidata = "Q1"
[needs]
cpu = "286+"
memory = "512K"
"#,
    )
    .unwrap();
    let recipe = temp.0.join("draft.media.toml");
    std::fs::write(
        &recipe,
        r#"recipe = "1.0"
work = "draft"
[media]
name = "private/media/ibm-pc/draft"
platform = "ibm-pc"
release = "1.0"
revision = 1
[title]
publishers = ["Release Publisher"]
description = "A synthetic release for the show command."
labels = ["game", "demo"]
[needs]
memory = "640K"
[input.disk]
name = "DISK.IMG"
size = 737280
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
[[medium]]
id = "disk-1"
label = "Disk 1"
format = "720K"
from = "disk"
"#,
    )
    .unwrap();
    let (code, output, errors) = upcli(&["recipe", "show", recipe.to_str().unwrap()]);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert_eq!(
        output,
        "private/media/ibm-pc/draft:1.0-r1\tdraft\tibm-pc\n\
         title\t\"Lantern Maze\"\t# from draft.work.toml\n\
         publisher\t\"Release Publisher\"\n\
         developer\t\"Lantern Studio\"\t# from draft.work.toml\n\
         genre\t\"puzzle\"\t# from draft.work.toml\n\
         platform\t\"DOS\"\t# from draft.work.toml\n\
         description\t\"A synthetic release for the show command.\"\n\
         need\tcpu\t286+\t# from draft.work.toml\n\
         need\tmemory\t640K\n\
         work\t\"draft\"\n\
         synopsis\t\"Find a path through a maze of lanterns.\"\t# from draft.work.toml\n\
         work-id\twikidata\t\"Q1\"\t# from draft.work.toml\n\
         set-label\t\"game\"\t# from draft.work.toml\n\
         set-label\t\"demo\"\n"
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
    let firmware =
        format!("{TEMPLATE_RECIPES}/unemu-at-bios/unemu-at-bios-1.0.2-unemu.3.firmware.toml");
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
        assert_eq!(errors, format!("unemu recipe show: {path}: {message}\n"));
    }
    let (code, _, errors) = upcli(&["recipe", "show", "no-such.media.toml"]);
    assert_eq!(code, 2);
    assert!(
        errors.starts_with("unemu recipe show: no-such.media.toml: "),
        "{errors}"
    );
}
