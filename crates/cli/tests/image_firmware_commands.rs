// SPDX-License-Identifier: AGPL-3.0-or-later
//! Open-fixture firmware CLI imports, dry runs and inspection; machine-images §5.3.
#[path = "support/fixtures.rs"]
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
    let sources = cli::command::FirmwareSources {
        store_root: Some(root.join("store")),
        index: Some(root.join("rom-index")),
    };
    let code = cli::command::run_with_firmware_sources(&args, &mut out, &mut err, &sources);
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
    let tag = format!("firmware/{}:1", assembly::KISELEV_XI8088.id);
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
    assert_eq!(cli::command::run(&generic, &mut listing, &mut errors), 0);
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
