// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu recipe pin`, and the commands on a pinned recipe: `recipe show`,
//! `recipe build` and `recipe conformance` (recipes.md §3.1, §3.8, §6.15,
//! §6.16, §9): their output and exit codes. Every test names its own
//! preferences file and stores. Synthetic files only.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

fn upcli(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("recipe-pin-cli-{name}-{}", std::process::id()));
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
    image_store::Digest::of(bytes).hex().to_owned()
}

const WORK: &str = "recipe = \"1.0\"\n\n[work]\nid = \"pinned\"\n\n[title]\ntitle = \"Pinned\"\nsynopsis = \"A work.\"\n";

/// A media recipe of a 720K disk image taken unmodified and a readme, with
/// a work file; the inputs in `t/lib`.
fn recipe(t: &Temp) -> (PathBuf, PathBuf) {
    let data: Vec<u8> = (0..737_280).map(|i| (i % 251) as u8).collect();
    let readme = b"Read me first.\r\n";
    let lib = t.path("lib");
    let folder = t.path("recipes");
    fs::create_dir_all(&lib).unwrap();
    fs::create_dir_all(&folder).unwrap();
    fs::write(lib.join("DATA.BIN"), &data).unwrap();
    fs::write(lib.join("README.TXT"), readme).unwrap();
    let text = format!(
        "recipe = \"1.0\"\nwork = \"pinned\"\n\n[media]\nname = \"private/media/ibm-pc/pinned\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n\
         [title]\nyear = 1990\n\n\
         [input.readme]\nname = \"README.TXT\"\nsize = {}\nsha256 = \"{}\"\n\n\
         [input.data]\nname = \"DATA.BIN\"\nsize = {}\nsha256 = \"{}\"\n\n\
         [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"720K\"\nfrom = \"data\"\n\n\
         [[extra]]\nid = \"readme\"\nfile_name = \"README.TXT\"\nkind = \"document\"\nfrom = \"readme\"\n",
        readme.len(),
        sha(readme),
        data.len(),
        sha(&data),
    );
    let path = folder.join("pinned.media.toml");
    fs::write(&path, text).unwrap();
    fs::write(folder.join("pinned.work.toml"), WORK).unwrap();
    (path, lib)
}

fn words(t: &Temp, command: &str, path: &Path, store: &str, lib: &Path) -> Vec<OsString> {
    vec![
        "recipe".into(),
        command.into(),
        path.into(),
        "--config".into(),
        t.path("preferences.toml").into(),
        "--store".into(),
        t.path(store).into(),
        "--library".into(),
        lib.into(),
    ]
}

#[test]
fn recipe_pin_writes_the_lock_and_the_digest_then_show_build_and_conformance_use_them() {
    let t = Temp::new("flow");
    let (recipe, lib) = recipe(&t);

    let (code, output, errors) = upcli(&words(&t, "pin", &recipe, "author", &lib));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let lines: Vec<&str> = output.lines().collect();
    let digest = lines[2]
        .strip_prefix("built private/media/ibm-pc/pinned:r1\t")
        .and_then(|rest| rest.strip_suffix("\tdraft"))
        .unwrap_or_else(|| panic!("{output}"))
        .to_owned();
    let lock = recipe::lock::dir(&recipe);
    assert_eq!(
        lines[3],
        format!(
            "pinned private/media/ibm-pc/pinned:r1: digest {digest} written into {}, lock {} (3 files)",
            recipe.display(),
            lock.display()
        )
    );
    assert!(fs::read_to_string(&recipe)
        .unwrap()
        .contains(&format!("revision = 1\ndigest = \"{digest}\"\n")));
    assert_eq!(fs::read_dir(lock.join("sha256")).unwrap().count(), 3);

    // A second pin is refused: an artifact that changes is a new revision.
    let (code, _, errors) = upcli(&words(&t, "pin", &recipe, "author", &lib));
    assert_eq!(code, 1);
    assert!(
        errors.contains(&format!("already pinned to {digest}")),
        "{errors}"
    );

    // `show` prints the lock's record, marked as such, and the drift of a
    // work file edited since the pin.
    fs::write(
        recipe.with_file_name("pinned.work.toml"),
        WORK.replace("A work.", "A work, described again."),
    )
    .unwrap();
    let show: Vec<OsString> = vec!["recipe".into(), "show".into(), recipe.clone().into()];
    let (code, output, errors) = upcli(&show);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert_eq!(
        output,
        format!(
            "private/media/ibm-pc/pinned:r1\t{digest}\tibm-pc\n\
             # locked: the record of {digest}, which a build writes; the work file's edits wait for a new revision\n\
             # drift: title.description: the lock has \"A work.\", pinned.work.toml has \"A work, described again.\"\n\
             # drift: title.synopsis: the lock has \"A work.\", pinned.work.toml has \"A work, described again.\"\n\
             title\t\"Pinned\"\n\
             year\t1990\n\
             description\t\"A work.\"\n\
             work\t\"pinned\"\n\
             synopsis\t\"A work.\"\n"
        )
    );

    // A build in another store publishes the pinned digest, and says the
    // lock's values were published.
    let (code, output, errors) = upcli(&words(&t, "build", &recipe, "other", &lib));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(output.contains("  drift: title.synopsis: the lock has \"A work.\", pinned.work.toml has \"A work, described again.\"; the lock's value was published"), "{output}");
    assert!(
        output.ends_with(&format!(
            "built private/media/ibm-pc/pinned:r1\t{digest}\tlocked\n"
        )),
        "{output}"
    );

    // The conformance harness: the pinned recipe conforms, from the library
    // alone; the scratch store is gone afterwards.
    let (code, output, errors) = upcli(&words(
        &t,
        "conformance",
        recipe.parent().unwrap(),
        "conformance",
        &lib,
    ));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert_eq!(
        output,
        format!(
            "{}: conforms: private/media/ibm-pc/pinned:r1\t{digest}\n\
             1 pinned recipe: 1 conform, 0 differ, 0 not built; 0 drafts skipped\n",
            recipe.display()
        )
    );
    assert_eq!(
        fs::read_dir(t.path("conformance/cache/recipe-conformance"))
            .unwrap()
            .count(),
        0
    );

    // Without the inputs, it is not built and the exit status says so.
    let empty = t.path("empty");
    fs::create_dir_all(&empty).unwrap();
    let (code, output, _) = upcli(&words(&t, "conformance", &recipe, "conformance", &empty));
    assert_eq!(code, 1);
    assert!(
        output.ends_with("1 pinned recipe: 0 conform, 0 differ, 1 not built; 0 drafts skipped\n"),
        "{output}"
    );
}

#[test]
fn recipe_pin_and_conformance_usage_errors_exit_2() {
    let t = Temp::new("usage");
    let (recipe, lib) = recipe(&t);
    let mut keep = words(&t, "pin", &recipe, "store", &lib);
    keep.extend(["--keep-as".into(), "mine-r1".into()]);
    for args in [
        keep,
        vec!["recipe".into(), "pin".into()],
        vec!["recipe".into(), "conformance".into()],
        vec!["recipe".into(), "conformance".into(), "--json".into()],
    ] {
        let (code, _, errors) = upcli(&args);
        assert_eq!(code, 2, "{args:?}");
        assert!(errors.contains("unemu recipe pin FILE"), "{errors}");
    }
}

#[test]
fn recipe_conformance_that_finds_no_pinned_recipe_exits_1() {
    let t = Temp::new("nothing-pinned");
    let (recipe, lib) = recipe(&t);
    let (code, output, errors) = upcli(&words(
        &t,
        "conformance",
        recipe.parent().unwrap(),
        "store",
        &lib,
    ));
    assert_eq!(code, 1, "{output}");
    assert!(
        output.ends_with("0 pinned recipes: 0 conform, 0 differ, 0 not built; 1 draft skipped\n"),
        "{output}"
    );
    assert_eq!(
        errors,
        "unemu recipe conformance: no pinned recipe found in the files and folders named\n"
    );
}

fn pin_then_drift(t: &Temp) -> (PathBuf, PathBuf) {
    let (path, lib) = recipe(t);
    let (code, output, errors) = upcli(&words(t, "pin", &path, "author", &lib));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    fs::write(
        path.with_file_name("pinned.work.toml"),
        WORK.replace("A work.", "A changed work."),
    )
    .unwrap();
    (path, lib)
}

#[test]
fn recipe_check_deny_drift_exits_1_but_default_still_warns() {
    let t = Temp::new("check-drift");
    let (path, _) = pin_then_drift(&t);
    let mut args = vec!["recipe".into(), "check".into(), path.clone().into()];
    let (code, warning, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (0, ""), "{warning}");
    assert!(warning.contains("drift: title.synopsis:"), "{warning}");
    args.push("--deny-drift".into());
    let (code, output, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (1, ""), "{output}");
    assert_eq!(output, warning);

    fs::write(path.with_file_name("pinned.work.toml"), WORK).unwrap();
    assert_eq!(upcli(&args).0, 0);
    fs::remove_file(&path).unwrap();
    assert_eq!(upcli(&args).0, 2);
}

#[test]
fn recipe_build_deny_drift_exits_1_before_reading_inputs_or_publishing() {
    let t = Temp::new("build-drift");
    let (path, lib) = pin_then_drift(&t);
    let mut args = words(&t, "build", &path, "refused", &lib);
    args.extend(["--deny-drift".into(), "--keep-as".into(), "mine-r1".into()]);
    let (code, output, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (1, ""), "{output}");
    assert!(output.contains("drift: title.synopsis:"), "{output}");
    assert!(!output.contains("built private/"), "{output}");
    assert!(!t.path("refused").exists());

    fs::remove_dir_all(&lib).unwrap();
    assert_eq!(upcli(&args), (code, output, errors));
    fs::write(path.with_file_name("pinned.work.toml"), WORK).unwrap();
    fs::create_dir_all(&lib).unwrap();
    assert_eq!(upcli(&args).0, 1);

    let pinned = fs::read_to_string(&path).unwrap();
    recipe(&t);
    fs::write(&path, pinned).unwrap();
    let (code, output, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
}

#[test]
fn recipe_conformance_with_drafts_exits_1_unless_allowed() {
    let t = Temp::new("draft-policy");
    let (path, lib) = recipe(&t);
    let draft = fs::read_to_string(&path).unwrap();
    assert_eq!(upcli(&words(&t, "pin", &path, "author", &lib)).0, 0);
    let draft_path = path.with_file_name("draft.media.toml");
    fs::write(&draft_path, draft).unwrap();
    let mut args = words(&t, "conformance", path.parent().unwrap(), "check", &lib);
    let (code, output, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (1, ""), "{output}");
    assert!(output.contains("draft, not built"), "{output}");
    assert!(
        output.ends_with("1 pinned recipe: 1 conform, 0 differ, 0 not built; 1 draft skipped\n"),
        "{output}"
    );
    args.push("--allow-drafts".into());
    let (code, allowed, errors) = upcli(&args);
    assert_eq!((code, errors.as_str()), (0, ""), "{allowed}");
    assert_eq!(allowed, output);

    fs::remove_dir_all(&lib).unwrap();
    fs::create_dir_all(&lib).unwrap();
    assert_eq!(upcli(&args).0, 1);
    fs::remove_file(path).unwrap();
    assert_eq!(upcli(&args).0, 1);
}

#[test]
fn recipe_policy_flags_reject_duplicates_and_missing_operands() {
    for args in [
        vec!["check", "--deny-drift"],
        vec!["build", "--deny-drift"],
        vec!["conformance", "--allow-drafts"],
        vec!["check", "FILE", "--deny-drift", "--deny-drift"],
        vec!["build", "FILE", "--deny-drift", "--deny-drift"],
        vec!["conformance", "FILE", "--allow-drafts", "--allow-drafts"],
        vec!["pin", "FILE", "--deny-drift"],
    ] {
        let args = std::iter::once("recipe")
            .chain(args)
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert_eq!(upcli(&args).0, 2, "{args:?}");
    }
}
