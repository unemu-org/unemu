// SPDX-License-Identifier: AGPL-3.0-or-later
//! The shared frontend routes command words without starting a window;
//! core-contract.md §12.13 Q1.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unemu"))
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap()
}

#[test]
fn command_words_reach_the_composed_cli_and_keep_its_exit_codes() {
    for args in [
        vec!["image"],
        vec!["media", "create"],
        vec!["recipe"],
        vec!["library"],
        vec!["run"],
        vec!["unknown-command"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage: unemu"));
    }
}

#[test]
fn version_and_about_share_the_desktop_notice_without_opening_a_window() {
    let version = run(&["--version"]);
    let about = run(&["--about"]);
    assert!(version.status.success());
    assert!(about.status.success());
    assert_eq!(version.stdout, about.stdout);
    assert!(version.stderr.is_empty());
    assert!(about.stderr.is_empty());
    let text = String::from_utf8(version.stdout).unwrap();
    assert!(text.starts_with(concat!("unemu ", env!("CARGO_PKG_VERSION"), "\n")));
    for notice in [
        "core unemupc",
        "core unemunes",
        "AGPL-3.0-or-later",
        "Nuked-OPL3",
        "Ubuntu Font",
    ] {
        assert!(text.contains(notice), "missing {notice}");
    }
}

#[test]
fn media_creation_runs_the_composed_command_line_without_a_display() {
    let temp = assembly::test_support::TempDir::new("unemu-media-dispatch");
    let root = temp.path();
    let disk = root.join("disk.img");
    let output = run(&[
        "media",
        "create",
        "floppy",
        "--type",
        "360K",
        disk.to_str().unwrap(),
        "--format",
    ]);
    let length = std::fs::metadata(&disk).map(|metadata| metadata.len());
    assert!(output.status.success(), "{output:?}");
    assert_eq!(length.unwrap(), 368_640);
}

#[test]
fn help_lists_both_the_window_and_command_words() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("unemu [--machine FILE]"), "{text}");
    assert!(text.contains("unemu image ls"), "{text}");
}

#[test]
fn image_media_and_recipe_commands_work_without_a_window() {
    let temp = assembly::test_support::TempDir::new("unemu-dispatch");
    let root = temp.path();
    let store = root.join("store");
    image_store::Store::open(&store).unwrap();
    let listed = run(&["image", "ls", "--store", store.to_str().unwrap()]);
    assert!(listed.status.success(), "{listed:?}");
    assert!(listed.stderr.is_empty());
    let disk = root.join("blank.img");
    let created = run(&[
        "media",
        "create",
        "floppy",
        "--type",
        "360K",
        disk.to_str().unwrap(),
        "--format",
    ]);
    assert!(created.status.success(), "{created:?}");
    assert_eq!(std::fs::metadata(&disk).unwrap().len(), 368_640);
    let missing = root.join("missing.media.toml");
    let checked = run(&["recipe", "check", missing.to_str().unwrap()]);
    assert_eq!(checked.status.code(), Some(2), "{checked:?}");
    assert!(String::from_utf8_lossy(&checked.stderr).contains("missing.media.toml"));
}
