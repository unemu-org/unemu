// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu recipe conformance` on a machine recipe: the open-at 1.3.0
//! recipe `image template config` emits, built and pinned as its author
//! does without `--fetch`, conforms in a scratch store of its own, from the
//! libraries and the package recipes named with it. Reads the open
//! firmware fixtures; skipped without them.
use pc_harness::test_support::fixtures;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

/// What `recipe pin` writes into the open-at 1.3.0 recipe.
const OPEN_AT: &str = "sha256:04bad34426efdd67e2bec1fb7eb33cdd54ea96e1b0e8e52550e5622e6211ecd2";

/// The package recipes the open-at uses pin, each folder with its file.
const PACKAGES: [(&str, &str); 3] = [
    ("unemu-at-bios", "unemu-at-bios-1.0.2-unemu.3.firmware.toml"),
    ("vgabios", "vgabios-0.9d-unemu.1-std.firmware.toml"),
    ("xtide-universal-bios", "xub-r638-xtp-ata1f0.firmware.toml"),
];

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
        let path = std::env::temp_dir().join(format!(
            "machine-conformance-cli-{name}-{}",
            std::process::id()
        ));
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

fn package_folder(folder: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../pc-image/data/recipes")
        .join(folder)
}

/// `recipe COMMAND PATHS... --config --store STORE --library ...`, each
/// firmware fixture folder a library.
fn words(t: &Temp, command: &str, paths: &[PathBuf], store: &str) -> Vec<OsString> {
    let vendor = pc_harness::test_support::root().join("vendor");
    let mut args: Vec<OsString> = vec!["recipe".into(), command.into()];
    args.extend(paths.iter().map(|p| p.into()));
    args.extend([
        "--config".into(),
        t.path("preferences.toml").into(),
        "--store".into(),
        t.path(store).into(),
    ]);
    for (folder, _) in PACKAGES {
        args.extend(["--library".into(), vendor.join(folder).into()]);
    }
    args
}

fn run(t: &Temp, command: &str, paths: &[PathBuf], store: &str) -> (u8, String, String) {
    upcli(&words(t, command, paths, store))
}

fn fetched() -> bool {
    pc_image::template::FIRMWARE.iter().all(|source| {
        fixtures::fixture(source.rom).is_some()
            && fixtures::fixture(source.source_archive).is_some()
    })
}

#[test]
fn recipe_conformance_builds_a_pinned_open_at_from_the_libraries_and_its_package_recipes() {
    if !fetched() {
        return;
    }
    let t = Temp::new("open-at");
    let recipes = t.path("recipes");
    let (code, _, errors) = upcli(&[
        "image".into(),
        "template".into(),
        "config".into(),
        "--out".into(),
        recipes.clone().into(),
    ]);
    assert_eq!((code, errors.as_str()), (0, ""));
    // The author's store gets the packages from their recipes and the
    // libraries, then the machine is pinned there.
    for (folder, file) in PACKAGES {
        let (code, output, errors) =
            run(&t, "build", &[package_folder(folder).join(file)], "author");
        assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    }
    let machine = recipes.join("open-at.machine.toml");
    let (code, output, errors) = run(&t, "pin", std::slice::from_ref(&machine), "author");
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.contains(&format!("digest {OPEN_AT} written into")),
        "{output}"
    );

    // Named before its package recipes, the machine still conforms; the
    // scratch store is gone afterwards.
    let mut paths = vec![machine.clone()];
    paths.extend(PACKAGES.iter().map(|(folder, _)| package_folder(folder)));
    let (code, output, errors) = run(&t, "conformance", &paths, "conformance");
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.starts_with(&format!(
            "{}: conforms: unemu/machines/ibm-pc/open-at:1.3.0\t{OPEN_AT}\n",
            machine.display()
        )),
        "{output}"
    );
    assert!(
        output.ends_with("4 pinned recipes: 4 conform, 0 differ, 0 not built; 0 drafts skipped\n"),
        "{output}"
    );
    assert_eq!(
        fs::read_dir(t.path("conformance/cache/recipe-conformance"))
            .unwrap()
            .count(),
        0
    );

    // The machine alone finds no package in its scratch store: not built.
    let (code, output, _) = run(&t, "conformance", std::slice::from_ref(&machine), "alone");
    assert_eq!(code, 1, "{output}");
    assert_eq!(
        output,
        format!(
            "{}: not built: missing 3 of 4 inputs\n\
             1 pinned recipe: 0 conform, 0 differ, 1 not built; 0 drafts skipped\n",
            machine.display()
        )
    );
}
