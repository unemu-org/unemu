// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli recipe plan` (recipes.md §3.4, §3.5, §6.15): where each input of
//! a recipe is found, from the store, a library of the preferences or one
//! named with `--library`; a near miss; the JSON form; the exit codes. Every
//! test names its own preferences file and store, so the user's are never
//! read. Synthetic files only, except the user-ROM test of AZURAMON, which
//! looks for the user's copy of the game folder as the volume goldens do
//! (crates/pc-harness/tests/user_media_volumes.rs).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const AZURAMOON: &str =
    "../../backlog/docs/design/recipe-corpus/azuramon/azuramoon-2026.media.toml";

fn upcli(args: &[&OsString]) -> (u8, String, String) {
    let args: Vec<OsString> = args.iter().map(|a| (*a).clone()).collect();
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(&args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("recipe-plan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sha(bytes: &[u8]) -> String {
    image_store::Digest::of(bytes)
        .to_string()
        .trim_start_matches("sha256:")
        .to_owned()
}

/// `upcli recipe plan` with the temporary preferences file and store, plus
/// `extra` words.
fn plan(temp: &Temp, recipe: &Path, extra: &[&str]) -> (u8, String, String) {
    let mut words: Vec<OsString> = vec!["recipe".into(), "plan".into(), recipe.into()];
    words.extend(["--config".into(), temp.path("preferences.toml").into()]);
    words.extend(["--store".into(), temp.path("store").into()]);
    words.extend(extra.iter().map(OsString::from));
    upcli(&words.iter().collect::<Vec<_>>())
}

const HEAD: &str = "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/synthetic\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n[title]\ntitle = \"Synthetic\"\n\n";

/// A recipe of one game folder of two files, built into one 1.44M disk.
fn game_recipe(temp: &Temp, run: &[u8], data: &[u8]) -> PathBuf {
    let text = format!(
        "{HEAD}[input.game]\nname = \"GAME\"\nfiles = [\n\
         {{ path = \"RUN.COM\", size = {}, sha256 = \"{}\" }},\n\
         {{ path = \"DATA/LEVEL.DAT\", size = {}, sha256 = \"{}\" }},\n]\n\n\
         [[step]]\nid = \"disk\"\nfat = \"game\"\nformat = \"1.44M\"\n\n\
         [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1 of 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n",
        run.len(),
        sha(run),
        data.len(),
        sha(data)
    );
    let path = temp.path("synthetic.media.toml");
    fs::write(&path, text).unwrap();
    path
}

fn store(temp: &Temp) {
    image_store::Store::open(temp.path("store")).unwrap();
}

#[test]
fn a_library_named_on_the_command_line_finds_every_file_and_exits_0() {
    let temp = Temp::new("found");
    store(&temp);
    let lib = temp.path("lib");
    fs::create_dir_all(lib.join("copy/DATA")).unwrap();
    fs::write(lib.join("copy/RUN.COM"), b"run").unwrap();
    fs::write(lib.join("copy/DATA/LEVEL.DAT"), b"level data").unwrap();
    fs::write(lib.join("unrelated.bin"), b"of another size").unwrap();
    let recipe = game_recipe(&temp, b"run", b"level data");
    let (code, output, errors) = plan(&temp, &recipe, &["--library", lib.to_str().unwrap()]);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let lib_text = lib.display();
    assert!(output.contains(&format!(
        "library {lib_text}: 3 files, 0 zip archives (0 members), 0 catalogue rows; hashed 2, kept 0, dropped 0\n"
    )), "{output}");
    assert!(
        output.contains(&format!(
            "  game (GAME): found 2 of 2 files, read from 2 places (library {lib_text}: 2)\n"
        )),
        "{output}"
    );
    assert!(output.contains("  disk: fat game: runs\n"), "{output}");
    assert!(
        output.contains("  medium disk-1 \"Disk 1 of 1\": can build\n"),
        "{output}"
    );
    assert!(
        output.ends_with("can build: all 1 input found\n"),
        "{output}"
    );

    // The hashes were kept in the store's cache: a second plan hashes
    // nothing.
    let (code, output, _) = plan(&temp, &recipe, &["--library", lib.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(
        output.contains("; hashed 0, kept 2, dropped 0\n"),
        "{output}"
    );
}

#[test]
fn a_near_miss_is_reported_the_input_is_missing_and_the_plan_exits_1() {
    let temp = Temp::new("near");
    store(&temp);
    let lib = temp.path("lib");
    fs::create_dir_all(&lib).unwrap();
    // Another version of RUN.COM: same name, another size.
    fs::write(lib.join("run.com"), b"run, version 2").unwrap();
    fs::write(lib.join("LEVEL.DAT"), b"level data").unwrap();
    // The library comes from the preferences this time.
    fs::write(
        temp.path("preferences.toml"),
        format!("libraries = [{:?}]\n", lib.display().to_string()),
    )
    .unwrap();
    let recipe = game_recipe(&temp, b"run", b"level data");
    let (code, output, errors) = plan(&temp, &recipe, &[]);
    assert_eq!((code, errors.as_str()), (1, ""), "{output}");
    let lib_text = lib.display();
    assert!(
        output.contains(&format!(
        "  game (GAME): missing some: 1 of 2 files, read from 1 places (library {lib_text}: 1)\n\
         \x20   missing RUN.COM: 3 bytes, sha256 {}\n\
         \x20     near miss (same name, another size): library {lib_text}: run.com, 14 bytes\n\
         \x20   used for: Disk 1 of 1\n",
        sha(b"run")
    )),
        "{output}"
    );
    assert!(
        output.contains("  disk: fat game: blocked: missing game\n"),
        "{output}"
    );
    assert!(output.ends_with("missing 1 of 1 input\n"), "{output}");

    let (code, json, _) = plan(&temp, &recipe, &["--json"]);
    assert_eq!(code, 1);
    for line in [
        "  \"complete\": false,",
        "  \"summary\": \"missing 1 of 1 input\"",
        "              \"kind\": \"name-other-size\",",
        "          \"found\": null,",
    ] {
        assert!(json.lines().any(|l| l == line), "{line}\n{json}");
    }
}

/// A blank 1.44M disk imported into the store as `blank:r1` (compressed,
/// as an import is by default), the same bytes in a library, and a recipe
/// that names them.
fn blank_in_store_and_library(temp: &Temp) -> (PathBuf, PathBuf) {
    let disk = temp.path("disk.img");
    let (code, _, errors) = upcli(&[
        &"media".into(),
        &"create".into(),
        &"floppy".into(),
        &"--type".into(),
        &"1.44M".into(),
        &"--format".into(),
        &disk.clone().into_os_string(),
    ]);
    assert_eq!(code, 0, "{errors}");
    let (code, _, errors) = upcli(&[
        &"image".into(),
        &"media".into(),
        &"import".into(),
        &disk.clone().into_os_string(),
        &"--title".into(),
        &"Blank".into(),
        &"--tag".into(),
        &"blank:r1".into(),
        &"--store".into(),
        &temp.path("store").into_os_string(),
        &"--create".into(),
    ]);
    assert_eq!(code, 0, "{errors}");
    let bytes = fs::read(&disk).unwrap();
    let lib = temp.path("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::copy(&disk, lib.join("disk.img")).unwrap();
    let text = format!(
        "{HEAD}[input.disk]\nname = \"disk.img\"\nsize = {}\nsha256 = \"{}\"\n\n\
         [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n",
        bytes.len(),
        sha(&bytes)
    );
    let recipe = temp.path("blank.media.toml");
    fs::write(&recipe, text).unwrap();
    (recipe, lib)
}

#[test]
fn an_input_already_in_the_store_is_found_there_first() {
    let temp = Temp::new("store");
    let (recipe, lib) = blank_in_store_and_library(&temp);
    let (code, output, errors) = plan(&temp, &recipe, &["--library", lib.to_str().unwrap()]);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.contains("  disk (disk.img): found in the store (blank:r1)\n"),
        "{output}"
    );
    assert!(
        output.contains("; hashed 0, kept 0, dropped 0\n"),
        "{output}"
    );
    // The index of decoded digests is a cache in the store.
    assert!(temp.path("store/cache/decoded/index.json").is_file());
}

#[test]
fn a_tag_whose_manifest_cannot_be_read_is_a_warning_and_the_plan_goes_on() {
    let temp = Temp::new("broken-tag");
    let (recipe, lib) = blank_in_store_and_library(&temp);
    let store = image_store::Store::open_existing(temp.path("store")).unwrap();
    let manifest = store.tag_descriptor("blank:r1").unwrap().digest.to_string();
    let blob = temp.path(&format!(
        "store/images/blobs/sha256/{}",
        manifest.trim_start_matches("sha256:")
    ));
    let mut permissions = fs::metadata(&blob).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&blob, permissions).unwrap();
    fs::write(&blob, b"not the manifest").unwrap();
    let (code, output, errors) = plan(&temp, &recipe, &["--library", lib.to_str().unwrap()]);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.contains(
            "  warning: the image of blank:r1 is left out: its manifest cannot be read: "
        ),
        "{output}"
    );
    assert!(
        output.contains("  disk (disk.img): found in library "),
        "{output}"
    );
}

#[test]
fn a_recipe_with_findings_prints_the_checks_and_usage_errors_exit_2() {
    let temp = Temp::new("usage");
    store(&temp);
    let recipe = temp.path("bad.media.toml");
    fs::write(&recipe, HEAD.replace("revision = 1", "revision = \"one\"")).unwrap();
    let (code, output, _) = plan(&temp, &recipe, &[]);
    assert_eq!(code, 1);
    assert!(output.contains("error[6.11.1] media.revision"), "{output}");
    let (code, json, _) = plan(&temp, &recipe, &["--json"]);
    assert_eq!(code, 1);
    assert!(json.contains("\"summary\": \"refused: "), "{json}");

    let (code, _, errors) = plan(
        &temp,
        &recipe,
        &["--library", temp.path("absent").to_str().unwrap()],
    );
    assert_eq!(code, 2);
    assert!(errors.starts_with("upcli recipe plan: "), "{errors}");
    for words in [
        &["recipe", "plan"][..],
        &["recipe", "plan", "a.media.toml", "b.media.toml"],
        &["recipe", "plan", "a.media.toml", "--bogus"],
        &["recipe", "plan", "a.media.toml", "--library"],
        &["recipe", "plan", "a.media.toml", "--json", "--json"],
    ] {
        let words: Vec<OsString> = words.iter().map(OsString::from).collect();
        let (code, output, errors) = upcli(&words.iter().collect::<Vec<_>>());
        assert_eq!((code, output.as_str()), (2, ""), "{words:?}");
        assert!(
            errors.contains("upcli recipe plan FILE|REF [--json]"),
            "{errors}"
        );
    }
}

/// AZURAMON's recipe lists 561 files with 204 distinct contents. With the
/// user's copy of the game folder in a library, the plan finds all 561
/// entries from 204 files, one per content, hashing each of them once; a
/// second plan hashes nothing. The user-ROM roots (`vendor/user-roms`, then
/// `UNEMUPC_USER_ROMS_DIR`) are the libraries.
#[test]
#[ignore = "needs the user's own copy of AZURAMON; see vendor/user-roms/README.md"]
fn user_rom_azuramoon_plan_finds_561_entries_from_204_files() {
    let temp = Temp::new("azuramoon");
    store(&temp);
    let roots: Vec<PathBuf> = machine_file::firmware_discovery::default_search_roots()
        .into_iter()
        .filter(|root| root.is_dir())
        .collect();
    let mut extra = Vec::new();
    for root in &roots {
        extra.extend(["--library", root.to_str().unwrap()]);
    }
    let (code, json, errors) = plan(
        &temp,
        Path::new(AZURAMOON),
        &[&["--json"][..], &extra].concat(),
    );
    let found = json
        .lines()
        .find_map(|l| l.trim().strip_prefix("\"found_files\": "))
        .and_then(|n| n.trim_end_matches(',').parse::<usize>().ok())
        .unwrap_or(0);
    if found != 561 {
        let message = format!(
            "SKIPPED: user-supplied files of Azuramoon not found ({found} of 561 files; set UNEMUPC_USER_ROMS_DIR or see vendor/user-roms/README.md)"
        );
        assert!(
            std::env::var_os("UNEMUPC_REQUIRE_USER_ROMS").as_deref()
                != Some(std::ffi::OsStr::new("1")),
            "{message}"
        );
        println!("{message}");
        return;
    }
    assert_eq!((code, errors.as_str()), (0, ""));
    let number = |key: &str| -> Vec<usize> {
        json.lines()
            .filter_map(|l| l.trim().strip_prefix(&format!("\"{key}\": ")))
            .filter_map(|n| n.trim_end_matches(',').parse().ok())
            .collect()
    };
    assert_eq!(number("places"), [204], "one file per distinct content");
    let hashed: usize = number("hashed").iter().sum();
    assert_eq!(hashed, 204, "each content hashed once, nothing else");
    // Only the counts above are printed: nothing about the user's files.
    let (code, json, _) = plan(
        &temp,
        Path::new(AZURAMOON),
        &[&["--json"][..], &extra].concat(),
    );
    assert_eq!(code, 0);
    let again: usize = json
        .lines()
        .filter_map(|l| l.trim().strip_prefix("\"hashed\": "))
        .filter_map(|n| n.trim_end_matches(',').parse::<usize>().ok())
        .sum();
    assert_eq!(again, 0, "the second plan reuses the index");
}
