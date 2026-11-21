// SPDX-License-Identifier: AGPL-3.0-or-later
//! Open-fixture firmware CLI imports, dry runs and inspection; machine-images §5.3.
#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "firmware-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn run(root: &Path, args: &[&str]) -> (u8, String, String) {
    let mut args = ["image", "firmware"]
        .into_iter()
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect::<Vec<_>>();
    args.extend(["--store".into(), root.join("store").into_os_string()]);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let sources = cli_pc::command::FirmwareSources {
        store_root: Some(root.join("store")),
        index: Some(root.join("rom-index")),
        ..Default::default()
    };
    let code = cli_pc::command::run_with_firmware_sources(&args, &mut out, &mut err, &sources);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}
#[test]
fn firmware_cli_dry_run_import_extras_inspection_and_tag_conflicts() {
    let t = Temp::new();
    let folder = t.0.join("roms");
    fs::create_dir(&folder).unwrap();
    let Some(bytes) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    fs::write(folder.join("renamed.bin"), bytes).unwrap();
    fs::write(t.0.join("rom-index"), b"dry-run sentinel").unwrap();
    fs::write(folder.join("unmatched"), b"synthetic").unwrap();
    let before = fs::read(folder.join("renamed.bin")).unwrap();
    let args = ["import", folder.to_str().unwrap(), "--create", "--dry-run"];
    let (code, out, err) = run(&t.0, &args);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("dry run"));
    assert_eq!(
        fs::read(t.0.join("rom-index")).unwrap(),
        b"dry-run sentinel"
    );
    assert!(out.contains("unmatched"));
    assert!(!t.0.join("store").exists());
    fs::create_dir(t.0.join("store")).unwrap();
    let (code, out, err) = run(&t.0, &args);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("dry run"));
    assert_eq!(
        fs::read(t.0.join("rom-index")).unwrap(),
        b"dry-run sentinel"
    );
    assert!(!t.0.join("store/store.toml").exists());
    fs::remove_dir(t.0.join("store")).unwrap();
    let extra = t.0.join("manual.txt");
    fs::write(&extra, b"synthetic manual").unwrap();
    let extra_arg = format!("{}:MANUAL.TXT", extra.display());
    let args = [
        "import",
        folder.to_str().unwrap(),
        "--create",
        "--for",
        assembly::KISELEV_XI8088.id,
        "--extra",
        extra_arg.as_str(),
    ];
    let (code, out, err) = run(&t.0, &args);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("imported"));
    let tag = format!("private/firmware/ibm-pc/{}:r1", assembly::KISELEV_XI8088.id);
    let (code, out, err) = run(&t.0, &["ls"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&tag));
    let mut listing = Vec::new();
    let mut errors = Vec::new();
    let generic = [
        OsString::from("image"),
        OsString::from("ls"),
        OsString::from("--store"),
        t.0.join("store").into_os_string(),
    ];
    assert_eq!(cli_pc::command::run(&generic, &mut listing, &mut errors), 0);
    let listing = String::from_utf8(listing).unwrap();
    // The package's ROM is stored as a frame; nothing is decoded to disk.
    let row = listing
        .lines()
        .find(|l| l.contains("\tfirmware\t"))
        .unwrap_or_else(|| panic!("{listing:?}"));
    let size = |key: &str| -> u64 {
        row.split('\t')
            .find_map(|f| f.strip_prefix(key))
            .unwrap()
            .parse()
            .unwrap()
    };
    assert!(size("stored:") < size("decoded:"), "{row}");
    assert!(listing.ends_with("\nmaterialized:0\n"), "{listing:?}");
    let (code, out, err) = run(&t.0, &["show", &tag]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("MANUAL") || out.contains("manual.txt"));
    assert!(out.contains("system-bios"));
    let (code, out, err) = run(&t.0, &args);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("already imported"), "{out}");
    assert_eq!(fs::read(folder.join("renamed.bin")).unwrap(), before);
    assert_eq!(fs::read(extra).unwrap(), b"synthetic manual");
}
#[test]
fn firmware_cli_rejects_missing_inputs_ambiguous_extra_targets_and_unknown_options() {
    let t = Temp::new();
    for args in [
        &["import"][..],
        &["show"],
        &["import", "rom", "--extra", "manual.txt"],
        &["import", "rom", "--for", "unknown"],
        &["import", "rom", "--rescan", "--rescan"],
        &["ls", "--dry-run"],
        &["show", "x:1", "--rescan"],
    ] {
        let (code, _, _) = run(&t.0, args);
        assert_ne!(code, 0, "{args:?}");
    }
    assert!(!t.0.join("store").exists());
}
/// A POSIX tar signature at offset 257 in `size` bytes.
fn tar(size: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; size];
    bytes[257..263].copy_from_slice(b"ustar\0");
    for (i, b) in bytes.iter_mut().enumerate().skip(512) {
        *b = (i % 53) as u8;
    }
    bytes
}
/// The `layer KIND DIGEST CLASS LICENCE` lines of `image firmware show`.
fn layers(show: &str, kind: &str) -> Vec<String> {
    show.lines()
        .filter(|l| l.starts_with(&format!("layer\t{kind}\t")))
        .map(str::to_owned)
        .collect()
}
#[test]
fn firmware_cli_states_licences_sources_and_metadata_and_edits_revisions() {
    let t = Temp::new();
    let rom = t.0.join("option.bin");
    fs::write(&rom, [0x66; 8192]).unwrap();
    let source = t.0.join("option-src.tar");
    fs::write(&source, tar(9000)).unwrap();
    let rom = rom.to_str().unwrap();
    let source = source.to_str().unwrap();
    let base = [
        "import",
        rom,
        "--create",
        "--unlisted",
        "opt-rom",
        "--role",
        "option-rom",
    ];
    let with = |extra: &[&'static str]| -> Vec<&str> {
        base.iter().copied().chain(extra.iter().copied()).collect()
    };
    // Open is a statement with a named licence; copyleft needs its source.
    let (code, _, err) = run(&t.0, &with(&["--open"]));
    assert_eq!(code, 1);
    assert!(err.contains("--license"), "{err}");
    let (code, _, err) = run(&t.0, &with(&["--open", "--license", "GPL-2.0-only"]));
    assert_eq!(code, 1);
    assert!(err.contains("copyleft"), "{err}");
    let mut args = with(&[
        "--open",
        "--license",
        "GPL-2.0-only",
        "--title",
        "Synthetic option ROM",
        "--description",
        "An adapter ROM for the CLI test.",
        "--label",
        "open-source",
        "--label",
        "synthetic",
        "--board",
        "ibm-5170",
        "--class",
        "at",
        "--requires",
        "ata-primary",
        "--built-from",
        "example/option-rom",
        "--revision",
        "r9",
        "--provenance-date",
        "2026-06-09",
    ]);
    args.extend(["--source", source]);
    // Any archive but one the project pins needs its own licence statement.
    let (code, _, err) = run(&t.0, &args);
    assert_eq!(code, 1);
    assert!(err.contains("--source-license"), "{err}");
    args.extend(["--source-license", "GPL-2.0-only"]);
    let (code, out, err) = run(&t.0, &args);
    assert_eq!(code, 0, "{err}");
    let digest = out
        .lines()
        .find_map(|l| l.strip_prefix("imported\tprivate/firmware/ibm-pc/opt-rom:r1\t"))
        .unwrap_or_else(|| panic!("{out}"))
        .to_owned();
    let (code, show, err) = run(&t.0, &["show", "private/firmware/ibm-pc/opt-rom:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(show.starts_with(&format!(
        "private/firmware/ibm-pc/opt-rom:r1\t{digest}\t\"Synthetic option ROM\"\n"
    )));
    for line in [
        "requires\toption-rom\t[\"ata-primary\"]\n",
        "labels\t[\"open-source\", \"synthetic\"]\n",
        "description\t\"An adapter ROM for the CLI test.\"\n",
        "provenance\tbuilt\t\"example/option-rom\"\trevision:r9\turl:-\tdate:2026-06-09\n",
        "source\t\"option-src.tar\"\ttar\t9000\t",
        "push\tany configured registry, including unemu/\n",
    ] {
        assert!(show.contains(line), "{line:?} missing from\n{show}");
    }
    let rom_layer = layers(&show, "rom");
    assert_eq!(rom_layer.len(), 1);
    assert!(rom_layer[0].ends_with("\topen\tGPL-2.0-only"), "{show}");
    assert!(layers(&show, "source")[0].ends_with("\topen\tGPL-2.0-only"));
    assert!(layers(&show, "readme")[0].ends_with("\topen\tCC0-1.0"));
    assert!(layers(&show, "config")[0].ends_with("\topen\tCC0-1.0"));
    // An edit is a new revision of the same ROM blob.
    let (code, out, err) = run(
        &t.0,
        &[
            "edit",
            "private/firmware/ibm-pc/opt-rom:r1",
            "--label",
            "edited",
            "--notes",
            "A quirk.",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with(
            "edited\tprivate/firmware/ibm-pc/opt-rom:r1\tprivate/firmware/ibm-pc/opt-rom:r2\tsha256:"
        ),
        "{out}"
    );
    let (code, edited, err) = run(&t.0, &["show", "private/firmware/ibm-pc/opt-rom:r2"]);
    assert_eq!(code, 0, "{err}");
    assert!(edited.contains("labels\t[\"edited\"]\n"), "{edited}");
    assert!(edited.contains("notes\t\"A quirk.\"\n"), "{edited}");
    assert_eq!(layers(&edited, "rom"), rom_layer);
    assert_eq!(layers(&edited, "source"), layers(&show, "source"));
    assert_ne!(layers(&edited, "readme"), layers(&show, "readme"));
    let (code, unchanged, _) = run(&t.0, &["show", "private/firmware/ibm-pc/opt-rom:r1"]);
    assert_eq!(code, 0);
    assert_eq!(unchanged, show, "the first revision is immutable");
    // Keeping a package private drops every licence mark.
    let (code, _, err) = run(
        &t.0,
        &[
            "edit",
            "private/firmware/ibm-pc/opt-rom:r2",
            "--to",
            "private/opt-rom:r1",
            "--user-supplied",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (_, private, _) = run(&t.0, &["show", "private/opt-rom:r1"]);
    assert!(
        private.contains("push\tprivate registries outside unemu/ only: user-supplied layers\n")
    );
    assert!(layers(&private, "rom")[0].ends_with("\tuser-supplied\t-"));
}
#[test]
fn firmware_cli_refuses_incomplete_licence_and_metadata_options() {
    let t = Temp::new();
    for args in [
        &[
            "import",
            "rom",
            "--unlisted",
            "x",
            "--role",
            "option-rom",
            "--license",
            "MIT",
        ][..],
        &[
            "import",
            "rom",
            "--unlisted",
            "x",
            "--role",
            "option-rom",
            "--open",
            "--user-supplied",
        ],
        &["import", "rom", "--unlisted", "x"],
        &["import", "rom", "--role", "option-rom"],
        &[
            "import",
            "a",
            "b",
            "--unlisted",
            "x",
            "--role",
            "option-rom",
        ],
        &["import", "rom", "--title", "Needs one package"],
        &[
            "import",
            "rom",
            "--for",
            "kiselev-xi8088-1.0.2",
            "--extra-license",
            "MIT",
        ],
        &[
            "import",
            "rom",
            "--for",
            "kiselev-xi8088-1.0.2",
            "--revision",
            "r1",
        ],
        &[
            "import",
            "rom",
            "--for",
            "kiselev-xi8088-1.0.2",
            "--built-from",
            "a",
            "--dumped-from",
            "b",
        ],
        &["edit", "private/firmware/ibm-pc/x:r1"],
        &["edit", "--title", "No tag"],
        &["ls", "--open"],
        &["show", "x:1", "--label", "a"],
        &[
            "import",
            "rom",
            "--unlisted",
            "x",
            "--role",
            "option-rom",
            "--source-license",
            "MIT",
        ],
        &[
            "import",
            "rom",
            "--unlisted",
            "x",
            "--role",
            "option-rom",
            "--open",
            "--source-license",
            "MIT",
        ],
    ] {
        let (code, _, err) = run(&t.0, args);
        assert_eq!(code, 2, "{args:?}: {err}");
    }
    for args in [
        &[
            "import",
            "rom",
            "--unlisted",
            "x",
            "--role",
            "option-rom",
            "--open",
            "--open",
        ][..],
        &[
            "edit",
            "private/firmware/ibm-pc/x:r1",
            "--user-supplied",
            "--user-supplied",
        ],
        &["import", "rom", "--create", "--create"],
    ] {
        let (code, _, err) = run(&t.0, args);
        assert_eq!(code, 2, "{args:?}");
        assert!(
            err.ends_with("duplicate firmware option\n"),
            "{args:?}: {err}"
        );
    }
    assert!(!t.0.join("store").exists());
}
