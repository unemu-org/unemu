// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu image export` (recipes.md §3.10, decision-37, §19.2 B11): every
//! layer of a set or package as a plain file under its export file name,
//! the config and manifest as stored, `EXPORT-VERSION` and `SHA256SUMS`;
//! with `--files`, the trees of the set's FAT12 disks merged into `files`,
//! with the entries' times. `scripts/export-layout.sh`, which uses no
//! upcli, writes the same files from the store's OCI Image Layout: the
//! tests run both on the same stored artifact, a synthetic set, a synthetic
//! firmware package and the FreeDOS 1.4 1.44M set, and compare the trees
//! byte for byte, times of the unpacked files included. The script needs
//! jq, sha256sum, zstd and mtools; without one of them that comparison is
//! skipped with a line saying so.
//!
//! The user-ROM test builds Azuramoon from the user's own folder, exports
//! it with `--files` and compares the tree with the folder; it prints only
//! a verdict.

use media::fat12::{plan_volumes, DosTime, Entry, FloppyFormat, Order, Volume};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

#[path = "../../pc-image/tests/support/frontend.rs"]
mod frontend;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/export-layout.sh"
);
const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../backlog/docs/design/recipe-corpus"
);

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "image-export-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
    fn store(&self) -> String {
        self.path("store").to_str().unwrap().to_owned()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

fn export(t: &Temp, tag: &str, to: &str, files: bool) -> (u8, String, String) {
    let to = t.path(to);
    let store = t.store();
    let mut args = vec!["image", "export", tag, "--to", to.to_str().unwrap()];
    if files {
        args.push("--files");
    }
    args.extend(["--store", &store]);
    upcli(&args)
}

/// The tools the script needs, or the first one missing.
fn script_tools() -> Result<(), &'static str> {
    for tool in ["jq", "sha256sum", "zstd", "mcopy", "mdir"] {
        let found = std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()));
        if !found {
            return Err(tool);
        }
    }
    Ok(())
}

/// Run the script into `to` and return its standard output, or `None` with
/// a line when a tool it needs is missing.
fn script(t: &Temp, tag: &str, to: &str, files: bool) -> Option<String> {
    if let Err(tool) = script_tools() {
        // The devShell has every tool, so the gate, which requires the
        // fixtures, requires them too.
        let message =
            format!("SKIPPED: scripts/export-layout.sh needs {tool}, which is not on PATH");
        assert!(
            std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").as_deref()
                != Some(std::ffi::OsStr::new("1")),
            "{message}"
        );
        println!("{message}");
        return None;
    }
    let mut command = std::process::Command::new("/bin/sh");
    command.arg(SCRIPT);
    if files {
        command.arg("--files");
    }
    let output = command
        .args([&t.path("store/images"), Path::new(tag), &t.path(to)])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8(output.stdout).unwrap())
}

/// What a tree holds: each path below `root`, `/`-separated, with a file's
/// bytes (`None` for a folder) and, for what lies under `files`, its
/// modification time in seconds.
type Tree = BTreeMap<String, (Option<Vec<u8>>, Option<u64>)>;

fn tree(root: &Path) -> Tree {
    fn walk(root: &Path, at: &Path, out: &mut Tree) {
        let mut entries: Vec<_> = fs::read_dir(at)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            let meta = fs::symlink_metadata(&path).unwrap();
            let time = relative.starts_with("files/").then(|| {
                meta.modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            });
            if meta.is_dir() {
                out.insert(relative, (None, time));
                walk(root, &path, out);
            } else {
                out.insert(relative, (Some(fs::read(&path).unwrap()), time));
            }
        }
    }
    let mut out = Tree::new();
    walk(root, root, &mut out);
    out
}

/// Compare two trees and name the first difference.
fn same_trees(a: &Tree, b: &Tree) {
    let names = |t: &Tree| t.keys().cloned().collect::<Vec<_>>();
    assert_eq!(names(a), names(b), "the trees hold other names");
    for (path, (bytes, time)) in a {
        let (other, other_time) = &b[path];
        assert!(bytes == other, "{path}: other bytes");
        assert_eq!(time, other_time, "{path}: another time");
    }
}

fn dos(text: &str) -> DosTime {
    text.parse().unwrap()
}

/// Two 720K disks a split built, in listed order with times, a label and a
/// serial, and a blank 360K image that is no FAT volume.
fn synthetic_disks(t: &Temp) -> (Vec<Entry>, Vec<PathBuf>) {
    let entries = vec![
        Entry::file("README.TXT", b"Read me.\r\n".to_vec()).at(dos("1994-05-06T07:08:10")),
        Entry::file("GAME/PART1.BIN", vec![1; 300_000]).at(dos("1995-01-02T03:04:06")),
        Entry::file("GAME/PART2.BIN", vec![2; 300_000]),
        Entry::file("GAME/DATA/PART3.BIN", vec![3; 300_000]).at(dos("2026-11-19T12:34:56")),
        Entry::folder("EMPTY").at(dos("2000-02-29T23:59:58")),
    ];
    let mut volume = Volume::new(FloppyFormat::Dd720);
    volume.order = Order::Listed;
    volume.label = Some(*b"SYNTH      ");
    volume.serial = 0x0BAD_F00D;
    volume.time = dos("1999-12-31T23:59:58");
    volume.volumes = Some(2);
    let images = plan_volumes(entries.clone(), &volume)
        .unwrap()
        .images()
        .unwrap();
    fs::create_dir_all(t.path("disks")).unwrap();
    let mut paths = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let path = t.path(&format!("disks/DISK{}.IMG", index + 1));
        fs::write(&path, image).unwrap();
        paths.push(path);
    }
    let blank = t.path("disks/BLANK.IMG");
    fs::write(&blank, vec![0u8; 368_640]).unwrap();
    paths.push(blank);
    (entries, paths)
}

fn import_set(t: &Temp, tag: &str, disks: &[PathBuf], extra: Option<&Path>) {
    let store = t.store();
    let mut args: Vec<String> = ["image", "media", "import"].map(str::to_owned).to_vec();
    args.extend(disks.iter().map(|p| p.to_str().unwrap().to_owned()));
    args.extend(
        [
            "--title",
            "Export test",
            "--tag",
            tag,
            "--store",
            &store,
            "--create",
        ]
        .map(str::to_owned),
    );
    if let Some(extra) = extra {
        args.extend(["--extra".into(), extra.to_str().unwrap().to_owned()]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, _, errors) = upcli(&args);
    assert_eq!(code, 0, "{errors}");
}

fn sums(dir: &Path, names: &[&str]) -> String {
    let mut names = names.to_vec();
    names.sort();
    names
        .iter()
        .map(|name| {
            let digest = image_store::Digest::of(&fs::read(dir.join(name)).unwrap());
            format!("{}  {name}\n", digest.hex())
        })
        .collect()
}

#[test]
fn a_set_exports_every_layer_and_its_disks_files_as_the_script_does() {
    let t = Temp::new("set");
    let (entries, disks) = synthetic_disks(&t);
    fs::write(t.path("notes.txt"), b"Synthetic notes").unwrap();
    import_set(&t, "export-test:r1", &disks, Some(&t.path("notes.txt")));

    let (code, output, errors) = export(&t, "export-test:r1", "out", true);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let store = image_store::Store::open_existing(t.path("store")).unwrap();
    let descriptor = store.tag_descriptor("export-test:r1").unwrap();
    assert_eq!(
        output,
        format!(
            "not read\tBLANK.IMG\tnot a FAT12 volume: no boot sector signature\n\
             exported export-test:r1\t{}\t9 files\n\
             files\t4 files\t3 folders\t2 volumes\n",
            descriptor.digest
        )
    );
    let out = t.path("out");
    let mut names: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "BLANK.IMG",
            "DISK1.IMG",
            "DISK2.IMG",
            "EXPORT-VERSION",
            "SHA256SUMS",
            "config.toml",
            "files",
            "manifest.json",
            "metadata.json",
            "notes.txt"
        ]
    );
    // Unmodified media and extras are the inputs byte for byte; config and
    // manifest are the stored bytes.
    for disk in &disks {
        let name = disk.file_name().unwrap();
        assert_eq!(fs::read(out.join(name)).unwrap(), fs::read(disk).unwrap());
    }
    assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"Synthetic notes");
    use image_store::BlobSource;
    let manifest = store.read_verified(&descriptor, 1 << 20).unwrap();
    assert_eq!(fs::read(out.join("manifest.json")).unwrap(), manifest);
    let manifest = image_store::Manifest::decode_for_summary(&manifest).unwrap();
    assert_eq!(
        fs::read(out.join("config.toml")).unwrap(),
        store.read_verified(&manifest.config, 1 << 20).unwrap()
    );
    assert_eq!(fs::read(out.join("EXPORT-VERSION")).unwrap(), b"1\n");
    assert_eq!(
        fs::read_to_string(out.join("SHA256SUMS")).unwrap(),
        sums(
            &out,
            &[
                "BLANK.IMG",
                "DISK1.IMG",
                "DISK2.IMG",
                "EXPORT-VERSION",
                "config.toml",
                "manifest.json",
                "metadata.json",
                "notes.txt"
            ]
        )
    );
    // The two disks' trees make up the tree that was split, with each
    // entry's time, or the step's, as UTC.
    let exported = tree(&out);
    let default = dos("1999-12-31T23:59:58").unix_seconds();
    for (path, time) in [
        ("files/README.TXT", "1994-05-06T07:08:10"),
        ("files/GAME/PART1.BIN", "1995-01-02T03:04:06"),
        ("files/GAME/DATA/PART3.BIN", "2026-11-19T12:34:56"),
        ("files/EMPTY", "2000-02-29T23:59:58"),
    ] {
        assert_eq!(exported[path].1, Some(dos(time).unix_seconds()), "{path}");
    }
    for path in ["files/GAME", "files/GAME/DATA", "files/GAME/PART2.BIN"] {
        assert_eq!(exported[path].1, Some(default), "{path}");
    }
    for entry in &entries {
        let path = format!("files/{}", entry.path);
        assert!(exported[&path].0 == entry.contents, "{path}");
    }
    let unpacked = exported.keys().filter(|p| p.starts_with("files/")).count();
    assert_eq!(unpacked, 7);

    // The script writes the same files from the store's OCI Image Layout.
    if let Some(output) = script(&t, "export-test:r1", "by-script", true) {
        assert_eq!(output, "not read\tBLANK.IMG\n");
        same_trees(&exported, &tree(&t.path("by-script")));
        // And sha256sum -c accepts the list.
        let check = std::process::Command::new("sha256sum")
            .args(["-c", "--quiet", "SHA256SUMS"])
            .current_dir(&out)
            .status()
            .unwrap();
        assert!(check.success());
    }

    // The same export again finds every file in place and writes nothing.
    let (code, again, errors) = export(&t, "export-test:r1", "out", true);
    assert_eq!((code, errors.as_str(), again), (0, "", output));
    same_trees(&exported, &tree(&out));
}

#[test]
fn an_export_overwrites_nothing_and_writes_nothing_before_it_has_checked_every_name() {
    let t = Temp::new("refuse");
    let (_, disks) = synthetic_disks(&t);
    fs::write(t.path("notes.txt"), b"Synthetic notes").unwrap();
    import_set(&t, "export-test:r1", &disks, Some(&t.path("notes.txt")));
    let (code, _, errors) = export(&t, "export-test:r1", "out", true);
    assert_eq!(code, 0, "{errors}");
    let out = t.path("out");

    // A file with other bytes refuses the export; a file removed before is
    // not written again, so nothing was written.
    fs::write(out.join("notes.txt"), b"changed").unwrap();
    fs::remove_file(out.join("EXPORT-VERSION")).unwrap();
    fs::remove_file(out.join("files/README.TXT")).unwrap();
    let (code, output, errors) = export(&t, "export-test:r1", "out", true);
    assert_eq!(
        (code, output.as_str(), errors.as_str()),
        (
            1,
            "",
            "notes.txt is already there with other bytes; the export overwrites nothing\n"
        )
    );
    assert!(!out.join("EXPORT-VERSION").exists());
    assert!(!out.join("files/README.TXT").exists());
    assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"changed");
    fs::write(out.join("notes.txt"), b"Synthetic notes").unwrap();

    // The same for a file of the tree, a folder where a file goes, and a
    // link.
    fs::write(out.join("files/GAME/PART2.BIN"), b"changed").unwrap();
    let (code, _, errors) = export(&t, "export-test:r1", "out", true);
    assert_eq!(code, 1);
    assert_eq!(
        errors,
        "files/GAME/PART2.BIN is already there with other bytes; the export overwrites nothing\n"
    );
    // Without --files the tree is not looked at.
    let (code, _, errors) = export(&t, "export-test:r1", "out", false);
    assert_eq!((code, errors.as_str()), (0, ""));
    assert!(out.join("EXPORT-VERSION").exists());
    fs::remove_file(out.join("config.toml")).unwrap();
    fs::create_dir(out.join("config.toml")).unwrap();
    let (code, _, errors) = export(&t, "export-test:r1", "out", false);
    assert_eq!(
        (code, errors.as_str()),
        (
            1,
            "config.toml is a folder where the export writes a file\n"
        )
    );
    fs::remove_dir(out.join("config.toml")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(t.path("notes.txt"), out.join("config.toml")).unwrap();
        let (code, _, errors) = export(&t, "export-test:r1", "out", false);
        assert_eq!(
            (code, errors.as_str()),
            (
                1,
                "config.toml is a symbolic link; the export writes no file through one\n"
            )
        );
    }
}

#[test]
fn files_that_two_disks_hold_with_other_bytes_refuse_files_but_not_the_export() {
    let t = Temp::new("clash");
    fs::create_dir_all(t.path("disks")).unwrap();
    let mut disks = Vec::new();
    for (index, bytes) in [b"one".as_slice(), b"two".as_slice()].iter().enumerate() {
        let image = plan_volumes(
            vec![
                Entry::file("SAME.TXT", b"same".to_vec()),
                Entry::file("CLASH.TXT", bytes.to_vec()),
            ],
            &Volume::new(FloppyFormat::Dd360),
        )
        .unwrap()
        .images()
        .unwrap()
        .remove(0);
        let path = t.path(&format!("disks/D{}.IMG", index + 1));
        fs::write(&path, image).unwrap();
        disks.push(path);
    }
    import_set(&t, "clash:r1", &disks, None);
    let (code, output, errors) = export(&t, "clash:r1", "out", true);
    assert_eq!(
        (code, output.as_str(), errors.as_str()),
        (
            1,
            "",
            "--files: D1.IMG and D2.IMG both hold CLASH.TXT with other bytes; export without --files\n"
        )
    );
    assert!(!t.path("out").exists());
    let (code, _, errors) = export(&t, "clash:r1", "out", false);
    assert_eq!((code, errors.as_str()), (0, ""));
}

#[test]
fn a_firmware_package_exports_its_roms_and_readme_as_the_script_does() {
    let t = Temp::new("firmware");
    let rom = t.path("option.bin");
    fs::write(&rom, [0x66; 8192]).unwrap();
    let store = t.store();
    let (code, _, errors) = upcli(&[
        "image",
        "firmware",
        "import",
        rom.to_str().unwrap(),
        "--unlisted",
        "opt-rom",
        "--role",
        "option-rom",
        "--store",
        &store,
        "--create",
    ]);
    assert_eq!(code, 0, "{errors}");
    let tag = "private/firmware/ibm-pc/opt-rom:r1";
    let (code, output, errors) = export(&t, tag, "out", false);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(output.ends_with("\t6 files\n"), "{output}");
    let exported = tree(&t.path("out"));
    assert_eq!(
        exported.keys().collect::<Vec<_>>(),
        [
            "EXPORT-VERSION",
            "README.md",
            "SHA256SUMS",
            "config.toml",
            "manifest.json",
            "option.bin"
        ]
    );
    assert!(exported["option.bin"].0.as_deref() == Some(&[0x66; 8192][..]));
    // A package has no disks: --files adds nothing.
    let (code, output, _) = export(&t, tag, "with-files", true);
    assert_eq!(code, 0);
    assert!(
        output.ends_with("files\t0 files\t0 folders\t0 volumes\n"),
        "{output}"
    );
    if script(&t, tag, "by-script", false).is_some() {
        same_trees(&exported, &tree(&t.path("by-script")));
    }
}

/// The FreeDOS 1.4 1.44M set, an open fixture of real disks with long
/// names and hidden files: the export gives the disks back byte for byte,
/// and the command and the script write the same files and trees.
#[test]
fn the_freedos_set_exports_as_the_script_exports_it() {
    let dir = frontend::vendor().join("freedos/floppy-1.4/144m");
    let names = [
        "x86BOOT.img",
        "x86DSK01.img",
        "x86DSK02.img",
        "x86DSK03.img",
        "x86DSK04.img",
        "x86DSK05.img",
        "x86DSK06.img",
    ];
    for name in names {
        if frontend::fixture(&format!("freedos/floppy-1.4/144m/{name}")).is_none() {
            return;
        }
    }
    let t = Temp::new("freedos");
    let disks: Vec<PathBuf> = names.iter().map(|name| dir.join(name)).collect();
    import_set(&t, "freedos-export:r1", &disks, None);
    let (code, output, errors) = export(&t, "freedos-export:r1", "out", true);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(output.contains("\t7 volumes\n"), "{output}");
    for disk in &disks {
        let name = disk.file_name().unwrap();
        assert!(fs::read(t.path("out").join(name)).unwrap() == fs::read(disk).unwrap());
    }
    let exported = tree(&t.path("out"));
    // Long names are the files' names.
    assert!(exported.contains_key("files/freedos/bin/attrib.com"));
    assert!(exported.contains_key("files/KERNEL.SYS"));
    if let Some(output) = script(&t, "freedos-export:r1", "by-script", true) {
        assert_eq!(output, "");
        same_trees(&exported, &tree(&t.path("by-script")));
    }
}

/// Run an mtools command with the host's configuration left out and times
/// read as UTC.
fn mtools(tool: &str, args: &[&std::ffi::OsStr]) {
    let status = std::process::Command::new(tool)
        .args(args)
        .env("MTOOLSRC", "/dev/null")
        .env("MTOOLS_SKIP_CHECK", "1")
        .env("TZ", "UTC0")
        .status()
        .unwrap();
    assert!(status.success(), "{tool} {args:?}");
}

/// A disk mtools wrote, as Linux and Windows NT write names: an all lower
/// case name as an 8.3 entry with the case bits of byte 12, a mixed-case
/// name as a long name, in a folder. The command and the script give the
/// same names, bytes and times.
#[test]
fn a_disk_mtools_wrote_exports_with_its_host_names_as_the_script_does() {
    if script_tools().is_err() {
        script(&Temp::new("mtools-skip"), "-", "-", false);
        return;
    }
    let t = Temp::new("mtools");
    let image = t.path("MTOOLS.IMG");
    let os = |s: &Path| s.as_os_str().to_owned();
    mtools(
        "mformat",
        &[
            "-C".as_ref(),
            "-f".as_ref(),
            "1440".as_ref(),
            "-v".as_ref(),
            "MTOOLS".as_ref(),
            "-i".as_ref(),
            &os(&image),
            "::".as_ref(),
        ],
    );
    fs::write(t.path("readme.txt"), b"lower case").unwrap();
    fs::write(t.path("ReadMe.Long.Txt"), b"mixed case").unwrap();
    fs::write(t.path("data.bin"), [7u8; 3000]).unwrap();
    mtools("mmd", &["-i".as_ref(), &os(&image), "::docs".as_ref()]);
    for (file, to) in [
        ("readme.txt", "::"),
        ("ReadMe.Long.Txt", "::"),
        ("data.bin", "::docs"),
    ] {
        mtools(
            "mcopy",
            &["-i".as_ref(), &os(&image), &os(&t.path(file)), to.as_ref()],
        );
    }
    import_set(&t, "mtools:r1", &[image], None);
    let (code, output, errors) = export(&t, "mtools:r1", "out", true);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let exported = tree(&t.path("out"));
    for path in [
        "files/readme.txt",
        "files/ReadMe.Long.Txt",
        "files/docs/data.bin",
    ] {
        assert!(exported.contains_key(path), "{path}: {:?}", exported.keys());
    }
    script(&t, "mtools:r1", "by-script", true).unwrap();
    same_trees(&exported, &tree(&t.path("by-script")));
}

/// Two long names that differ only in Unicode composition: two files on a
/// host that keeps names as their bytes, one on a host that normalizes them
/// (macOS). There the export stops when the second meets the first and
/// removes everything it wrote.
#[test]
fn names_one_host_takes_as_one_file_leave_nothing_behind() {
    let t = Temp::new("composed");
    let mut image = plan_volumes(
        vec![
            Entry::file("A.TXT", b"composed".to_vec()),
            Entry::file("B.TXT", b"decomposed".to_vec()),
        ],
        &Volume::new(FloppyFormat::Dd360),
    )
    .unwrap()
    .images()
    .unwrap()
    .remove(0);
    // 360K: one reserved sector and two FATs of two sectors; the root's
    // first entries are A.TXT and B.TXT. Each gets a long name before it.
    let root = 5 * 512;
    let short: Vec<[u8; 32]> = (0..2)
        .map(|i| image[root + i * 32..root + i * 32 + 32].try_into().unwrap())
        .collect();
    let long = |name: &str, short: &[u8; 32]| {
        let checksum = short[..11]
            .iter()
            .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte));
        let mut part = [0xFFu8; 32];
        part[0] = 0x41;
        part[11] = 0x0F;
        part[12] = 0;
        part[13] = checksum;
        part[26..28].copy_from_slice(&[0, 0]);
        let units: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let slots = (1..11)
            .step_by(2)
            .chain((14..26).step_by(2))
            .chain((28..32).step_by(2));
        for (slot, unit) in slots.zip(units) {
            part[slot..slot + 2].copy_from_slice(&unit.to_le_bytes());
        }
        part
    };
    let entries = [
        long("caf\u{e9}.txt", &short[0]),
        short[0],
        long("cafe\u{301}.txt", &short[1]),
        short[1],
    ];
    for (index, entry) in entries.iter().enumerate() {
        image[root + index * 32..root + index * 32 + 32].copy_from_slice(entry);
    }
    let disk = t.path("NAMES.IMG");
    fs::write(&disk, &image).unwrap();
    import_set(&t, "names:r1", &[disk], None);
    let (code, _, errors) = export(&t, "names:r1", "out", true);
    // The file systems of macOS take the two names as one.
    #[cfg(target_os = "macos")]
    assert_eq!(code, 1, "{errors}");
    if code == 0 {
        // The host keeps both names.
        let exported = tree(&t.path("out"));
        assert!(exported["files/caf\u{e9}.txt"].0.as_deref() == Some(&b"composed"[..]));
        assert!(exported["files/cafe\u{301}.txt"].0.as_deref() == Some(&b"decomposed"[..]));
    } else {
        assert_eq!(code, 1);
        assert!(
            errors.contains("is already there with other bytes"),
            "{errors}"
        );
        assert!(!t.path("out").exists(), "the export left files behind");
    }
}

/// The user-ROM roots, as the other user-file tests take them.
fn roots() -> Vec<PathBuf> {
    machine_file::firmware_discovery::default_search_roots()
        .into_iter()
        .filter(|root| root.is_dir())
        .collect()
}

/// The first file below the roots of `size` bytes whose SHA-256 is
/// `sha256` and whose name is `name`, symbolic links followed.
fn find_file(name: &str, size: u64, sha256: &str) -> Option<PathBuf> {
    fn walk(
        dir: &Path,
        seen: &mut std::collections::BTreeSet<PathBuf>,
        found: &mut dyn FnMut(&Path) -> bool,
    ) -> Option<PathBuf> {
        if !seen.insert(dir.canonicalize().ok()?) {
            return None;
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for path in entries {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if let Some(hit) = walk(&path, seen, found) {
                    return Some(hit);
                }
            } else if meta.is_file() && found(&path) {
                return Some(path);
            }
        }
        None
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut found = |path: &Path| {
        path.file_name().is_some_and(|n| n == name)
            && fs::metadata(path).is_ok_and(|m| m.len() == size)
            && fs::read(path).is_ok_and(|bytes| image_store::Digest::of(&bytes).hex() == sha256)
    };
    roots()
        .iter()
        .find_map(|root| walk(root, &mut seen, &mut found))
}

/// Every file below `root` by its `/`-separated path, hidden entries (a
/// name starting with a dot) left out, as scaffold leaves them out.
fn user_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
                out.insert(relative.replace('\\', "/"), fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// recipes.md §19.2 B11: Azuramoon built from the user's folder and
/// exported with `--files` gives the folder back, every file byte for byte
/// under its own path, nothing more. Only a verdict is printed.
#[test]
#[ignore = "needs the user's own copy of the AZURAMON folder; see vendor/user-roms/README.md"]
fn user_rom_azuramoon_exported_with_files_is_the_users_folder() {
    let Some(com) = find_file(
        "AZURAMON.COM",
        768,
        "f9ed092b86c64bcf3e3ab001fd23ee170e7a0072174c112d0c23322ca84825e7",
    ) else {
        let message = "SKIPPED: user-supplied files of AZURAMON not found (set UNEMUPC_USER_ROMS_DIR or see vendor/user-roms/README.md)";
        assert!(
            std::env::var_os("UNEMUPC_REQUIRE_USER_ROMS").as_deref()
                != Some(std::ffi::OsStr::new("1")),
            "{message}"
        );
        println!("{message}");
        return;
    };
    let t = Temp::new("azuramoon");
    let recipe = format!("{CORPUS}/azuramon/azuramoon-2026.media.toml");
    let config = t.path("preferences.toml");
    let store = t.store();
    let mut args = vec![
        "recipe".to_owned(),
        "build".into(),
        recipe,
        "--config".into(),
        config.to_str().unwrap().to_owned(),
        "--store".into(),
        store,
    ];
    for root in roots() {
        args.extend(["--library".into(), root.to_str().unwrap().to_owned()]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, output, _) = upcli(&args);
    assert_eq!(code, 0, "the Azuramoon recipe did not build");
    let reference = "private/media/ibm-pc/azuramoon:2026-r1";
    assert!(
        output.contains(&format!("built {reference}\t")),
        "not tagged"
    );
    let (code, output, _) = export(&t, reference, "out", true);
    assert_eq!(code, 0, "the export failed");
    assert!(output.ends_with("\t10 volumes\n"), "not ten volumes");
    let exported: BTreeMap<String, Vec<u8>> = tree(&t.path("out/files"))
        .into_iter()
        .filter_map(|(path, (bytes, _))| Some((path, bytes?)))
        .collect();
    let expected = user_files(com.parent().unwrap());
    assert_eq!(
        expected.len(),
        561,
        "the folder holds other files than the recipe's"
    );
    assert!(
        exported == expected,
        "the exported tree differs from the user's folder"
    );
}
