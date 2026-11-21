// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu recipe scaffold` and `unemu recipe inputs` (recipes.md §6.15,
//! §19.2 B9): the tables scaffold writes for a zip archive, a folder and a
//! plain file, byte for byte; that they parse back and pass `recipe check`
//! once the author adds the rest; the inputs of that recipe as a DAT, a CSV
//! and a checksum file, byte for byte, and `sha256sum -c` accepting the
//! last against the files. Synthetic files only, except the two user-ROM
//! tests, which scaffold the user's own LABFULL.ZIP and AZURAMON folder and
//! compare with the corpus's tables; they look for the files as the volume
//! goldens do (crates/pc-harness/tests/user_media_volumes.rs) and print
//! nothing about them.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const CORPUS: &str = "../../backlog/docs/design/recipe-corpus";

fn upcli(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

fn words(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("recipe-scaffold-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xedb8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The MS-DOS time and date fields of a zip header (PKWARE APPNOTE
/// §4.4.6).
fn dos(year: u16, month: u16, day: u16, hour: u16, minute: u16, second: u16) -> (u16, u16) {
    (
        (hour << 11) | (minute << 5) | (second / 2),
        ((year - 1980) << 9) | (month << 5) | day,
    )
}

/// A zip archive of stored members, in order, each with its DOS time.
fn zip(entries: &[(&str, &[u8], (u16, u16))]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for (name, bytes, (time, date)) in entries {
        let offset = out.len() as u32;
        let mut common = Vec::new();
        common.extend_from_slice(&20u16.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        common.extend_from_slice(&time.to_le_bytes());
        common.extend_from_slice(&date.to_le_bytes());
        common.extend_from_slice(&crc32(bytes).to_le_bytes());
        common.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        common.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        common.extend_from_slice(&(name.len() as u16).to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&common);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(bytes);
        directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        directory.extend_from_slice(&20u16.to_le_bytes());
        directory.extend_from_slice(&common);
        directory.extend_from_slice(&[0; 10]);
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&directory);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The author's files: a zip whose order is not the name order and which
/// holds a folder entry, a folder tree with a hidden file, and a plain
/// file.
fn fixture(temp: &Temp) {
    temp.write(
        "files/GAME.ZIP",
        &zip(&[
            ("README.TXT", b"Read me.\n", dos(1993, 3, 27, 13, 58, 22)),
            ("DATA/", b"", dos(1994, 12, 31, 0, 0, 0)),
            (
                "DATA/LEVEL.DAT",
                b"level data",
                dos(1994, 12, 31, 23, 59, 58),
            ),
            ("A.EXE", b"MZ program", dos(1980, 1, 1, 0, 0, 0)),
        ]),
    );
    for (path, bytes) in [
        ("files/SET/B.TXT", &b"bee\n"[..]),
        ("files/SET/A.TXT", b"a\n"),
        ("files/SET/Z.BIN", b"\x00\x01\x02"),
        ("files/SET/SUB/DEEP/D.DAT", b"deep"),
        ("files/SET/SUB/C.DAT", b"see"),
        ("files/SET/ALPHA/E.TXT", b"e\n"),
        ("files/SET/.hidden", b"host metadata"),
        ("files/NOTES.TXT", b"Notes.\n"),
    ] {
        temp.write(path, bytes);
    }
}

// SHA-256 of the archive's bytes as [`zip`] writes them: 423 bytes, four
// local headers and the central directory. The members' are of their bytes,
// measured with Python's hashlib.
const ZIP_SHA: &str = "89cd77b95c37c44993bb4192279315e55ef5cea95856be17480e1b5aa99cf58c";
const ZIP_SIZE: u64 = 423;

/// What scaffold prints for the fixture, named `GAME.ZIP SET NOTES.TXT`.
fn expected_tables() -> String {
    format!(
        r#"[input.game]
name = "GAME.ZIP"
size = {ZIP_SIZE}
sha256 = "{ZIP_SHA}"

[input.set]
name = "SET"
files = [
    {{ path = "A.TXT", size = 2, sha256 = "{A}" }},
    {{ path = "B.TXT", size = 4, sha256 = "{B}" }},
    {{ path = "Z.BIN", size = 3, sha256 = "{Z}" }},
    {{ path = "ALPHA/E.TXT", size = 2, sha256 = "{E}" }},
    {{ path = "SUB/C.DAT", size = 3, sha256 = "{C}" }},
    {{ path = "SUB/DEEP/D.DAT", size = 4, sha256 = "{D}" }},
]

[input.notes]
name = "NOTES.TXT"
size = 7
sha256 = "{NOTES}"

[[step]]
id = "game-files"
unpack = "game"
format = "zip"
complete = true
members = [
    {{ path = "README.TXT", size = 9, sha256 = "{README}", time = 1993-03-27T13:58:22 }},
    {{ path = "DATA/LEVEL.DAT", size = 10, sha256 = "{LEVEL}", time = 1994-12-31T23:59:58 }},
    {{ path = "A.EXE", size = 10, sha256 = "{EXE}", time = 1980-01-01T00:00:00 }},
]
"#
    )
}

const A: &str = "87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7";
const B: &str = "c150e5a8a604acebd8d15bd7bf8ea96b2874bdcc91dee6319977d353251283b0";
const C: &str = "aa9e9b5c907d50feb410f2a84e81ab72c5ae6724d57ccc53650f9361d33dc734";
const D: &str = "74611c1d6455b534323a21f8133a6f43dc3a8188e7b946f96dcc28dde932fcb2";
const E: &str = "a2bbdb2de53523b8099b37013f251546f3d65dbe7a0774fa41af0a4176992fd4";
const Z: &str = "ae4b3280e56e2faf83f414a6e3dabe9d5fbe18976544c05fed121accb85b53fc";
const NOTES: &str = "8bcc07e3af5963927125230b5cbe9472ed79adbcd37b09082eba58d8ae50ac7d";
const README: &str = "0378767b3aa93349bb79cdd0ba46bd5979bb7adfb8cbcfe2404097479935be50";
const LEVEL: &str = "c37c487062d18e20916003f0500c1081547e40b0380868f001affff4663916c6";
const EXE: &str = "8b06c5fc916e99e35b74afa2bf2fdcd079cc33d0f7e8e84859c69c529e4748d1";

fn scaffold(temp: &Temp) -> (u8, String, String) {
    let files = temp.path("files");
    let mut args = words(&["recipe", "scaffold"]);
    args.extend(
        ["GAME.ZIP", "SET", "NOTES.TXT"]
            .iter()
            .map(|n| OsString::from(files.join(n))),
    );
    upcli(&args)
}

/// The scaffold's tables with what the author adds: the identity, a title
/// and the outputs that use every value.
fn recipe_text(tables: &str) -> String {
    format!(
        r#"recipe = "1.0"

[media]
name = "private/media/ibm-pc/scaffold-test"
platform = "ibm-pc"
release = "1.0"
revision = 1

[title]
title = "Scaffold test"

{tables}
[[step]]
id = "disk"
fat = "game-files"
format = "1.44M"

[[step]]
id = "set-disk"
fat = "set"
format = "1.44M"

[[medium]]
id = "disk-1"
label = "Disk 1 of 2"
format = "1.44M"
from = "disk"

[[medium]]
id = "disk-2"
label = "Disk 2 of 2"
format = "1.44M"
from = "set-disk"

[[extra]]
id = "notes"
file_name = "NOTES.TXT"
kind = "document"
from = "notes"
"#
    )
}

#[test]
fn scaffold_writes_the_tables_byte_for_byte_and_they_pass_the_checks() {
    let temp = Temp::new("tables");
    fixture(&temp);
    let (code, output, errors) = scaffold(&temp);
    assert_eq!((code, errors.as_str()), (0, ""));
    assert_eq!(output, expected_tables());
    // No host path is written: only the names given and paths below them.
    assert!(!output.contains(temp.0.to_str().unwrap()));
    // The same files give the same text.
    assert_eq!(scaffold(&temp).1, output);

    let recipe = temp.path("scaffold-test.media.toml");
    fs::write(&recipe, recipe_text(&output)).unwrap();
    let (code, report, errors) = upcli(&["recipe".into(), "check".into(), recipe.clone().into()]);
    assert_eq!((code, errors.as_str()), (0, ""), "{report}");
    assert!(report.ends_with("1 file checked, 0 findings\n"), "{report}");
}

fn inputs(recipe: &Path, format: &str) -> (u8, String, String) {
    upcli(&[
        "recipe".into(),
        "inputs".into(),
        recipe.into(),
        "--format".into(),
        format.into(),
    ])
}

#[test]
fn inputs_lists_the_recipe_in_three_forms_and_sha256sum_accepts_the_files() {
    let temp = Temp::new("inputs");
    fixture(&temp);
    let recipe = temp.path("scaffold-test.media.toml");
    fs::write(&recipe, recipe_text(&expected_tables())).unwrap();

    // Inputs by id (game, notes, set), a file set's entries in its order.
    let (code, sums, errors) = inputs(&recipe, "sha256sums");
    assert_eq!((code, errors.as_str()), (0, ""));
    assert_eq!(
        sums,
        format!(
            "{ZIP_SHA}  GAME.ZIP\n{NOTES}  NOTES.TXT\n{A}  SET/A.TXT\n{B}  SET/B.TXT\n{Z}  SET/Z.BIN\n{E}  SET/ALPHA/E.TXT\n{C}  SET/SUB/C.DAT\n{D}  SET/SUB/DEEP/D.DAT\n"
        )
    );
    let (code, csv, _) = inputs(&recipe, "csv");
    assert_eq!(code, 0);
    assert_eq!(
        csv,
        format!(
            "path,size,sha256,crc32,sha1,md5\nGAME.ZIP,{ZIP_SIZE},{ZIP_SHA},,,\nNOTES.TXT,7,{NOTES},,,\nSET/A.TXT,2,{A},,,\nSET/B.TXT,4,{B},,,\nSET/Z.BIN,3,{Z},,,\nSET/ALPHA/E.TXT,2,{E},,,\nSET/SUB/C.DAT,3,{C},,,\nSET/SUB/DEEP/D.DAT,4,{D},,,\n"
        )
    );
    let (code, dat, _) = inputs(&recipe, "dat");
    assert_eq!(code, 0);
    assert_eq!(
        dat,
        format!(
            r#"<?xml version="1.0"?>
<!DOCTYPE datafile PUBLIC "-//Logiqx//DTD ROM Management Datafile//EN" "http://www.logiqx.com/Dats/datafile.dtd">
<datafile>
	<header>
		<name>private/media/ibm-pc/scaffold-test</name>
		<description>private/media/ibm-pc/scaffold-test:1.0-r1</description>
		<version>1.0-r1</version>
		<author>unemu recipe inputs</author>
	</header>
	<game name="scaffold-test-1.0-r1">
		<description>private/media/ibm-pc/scaffold-test:1.0-r1</description>
		<rom name="GAME.ZIP" size="{ZIP_SIZE}" sha256="{ZIP_SHA}"/>
		<rom name="NOTES.TXT" size="7" sha256="{NOTES}"/>
		<rom name="SET/A.TXT" size="2" sha256="{A}"/>
		<rom name="SET/B.TXT" size="4" sha256="{B}"/>
		<rom name="SET/Z.BIN" size="3" sha256="{Z}"/>
		<rom name="SET/ALPHA/E.TXT" size="2" sha256="{E}"/>
		<rom name="SET/SUB/C.DAT" size="3" sha256="{C}"/>
		<rom name="SET/SUB/DEEP/D.DAT" size="4" sha256="{D}"/>
	</game>
</datafile>
"#
        )
    );

    // The checksum file names the author's folder as it is: sha256sum -c,
    // run in it, accepts every line (coreutils is in the devShell).
    let list = temp.path("SHA256SUMS");
    fs::write(&list, &sums).unwrap();
    let run = std::process::Command::new("sha256sum")
        .arg("-c")
        .arg(&list)
        .current_dir(temp.path("files"))
        .output()
        .expect("sha256sum (coreutils) is on the PATH of the devShell");
    let said = String::from_utf8(run.stdout).unwrap();
    assert!(run.status.success(), "{said}");
    assert_eq!(
        said.lines().filter(|l| l.ends_with(": OK")).count(),
        8,
        "{said}"
    );
    // A changed file is caught.
    temp.write("files/SET/A.TXT", b"A\n");
    let run = std::process::Command::new("sha256sum")
        .arg("-c")
        .arg(&list)
        .current_dir(temp.path("files"))
        .output()
        .unwrap();
    assert!(!run.status.success());
}

#[test]
fn what_a_recipe_cannot_hold_exits_1_and_usage_exits_2() {
    let temp = Temp::new("refused");
    temp.write(
        "UP.ZIP",
        &zip(&[("../A.TXT", b"x", dos(1990, 1, 1, 0, 0, 0))]),
    );
    let (code, output, errors) = upcli(&[
        "recipe".into(),
        "scaffold".into(),
        temp.path("UP.ZIP").into(),
    ]);
    assert_eq!((code, output.as_str()), (1, ""));
    assert!(
        errors.starts_with("unemu recipe scaffold: ") && errors.contains("a `..` path component"),
        "{errors}"
    );
    let (code, output, errors) = upcli(&[
        "recipe".into(),
        "scaffold".into(),
        temp.path("absent").into(),
    ]);
    assert_eq!((code, output.as_str()), (2, ""));
    assert!(errors.starts_with("unemu recipe scaffold: "), "{errors}");

    // A recipe with findings: the check's report, exit 1.
    let recipe = temp.path("bad.media.toml");
    fs::write(&recipe, "recipe = \"1.0\"\n").unwrap();
    let (code, output, _) = inputs(&recipe, "csv");
    assert_eq!(code, 1);
    assert!(output.contains(": error["), "{output}");
    // Two inputs at one path with other bytes: one folder cannot hold both.
    let clash = temp.path("clash.media.toml");
    fs::write(
        &clash,
        format!(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/clash\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n[title]\ntitle = \"Clash\"\n\n\
             [input.a]\nname = \"X.BIN\"\nsize = 2\nsha256 = \"{A}\"\n\n[input.b]\nname = \"X.BIN\"\nsize = 4\nsha256 = \"{B}\"\n\n\
             [input.disk]\nname = \"D.IMG\"\nsize = 1474560\nsha256 = \"{Z}\"\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n\n\
             [[extra]]\nid = \"one\"\nfile_name = \"ONE.BIN\"\nkind = \"other\"\nfrom = \"a\"\n\n\
             [[extra]]\nid = \"two\"\nfile_name = \"TWO.BIN\"\nkind = \"other\"\nfrom = \"b\"\n"
        ),
    )
    .unwrap();
    let (code, report, _) = upcli(&["recipe".into(), "check".into(), clash.clone().into()]);
    assert_eq!(code, 0, "{report}");
    let (code, output, errors) = inputs(&clash, "sha256sums");
    assert_eq!((code, output.as_str()), (1, ""));
    assert!(
        errors.contains("the inputs `a` and `b` both lie at X.BIN with other bytes"),
        "{errors}"
    );
    // A work file has no inputs.
    let (code, _, errors) = inputs(
        Path::new(&format!("{CORPUS}/kenslab-2.01/kenslab.work.toml")),
        "dat",
    );
    assert_eq!(code, 2);
    assert!(errors.contains("a work file"), "{errors}");

    for line in [
        &["recipe", "scaffold"][..],
        &["recipe", "scaffold", "--json", "a.zip"],
        &["recipe", "inputs", "a.media.toml"],
        &["recipe", "inputs", "--format", "dat"],
        &["recipe", "inputs", "a.media.toml", "--format", "xml"],
        &["recipe", "inputs", "a.media.toml", "--format"],
        &[
            "recipe",
            "inputs",
            "a.media.toml",
            "b.media.toml",
            "--format",
            "dat",
        ],
        &[
            "recipe",
            "inputs",
            "a.media.toml",
            "--format",
            "dat",
            "--format",
            "csv",
        ],
    ] {
        let (code, output, errors) = upcli(&words(line));
        assert_eq!((code, output.as_str()), (2, ""), "{line:?}");
        assert!(
            errors.contains("unemu recipe scaffold FILE|FOLDER...")
                && errors.contains("unemu recipe inputs FILE --format dat|csv|sha256sums"),
            "{errors}"
        );
    }
}

/// A folder's order is the FAT builder's `order = "name"` order, byte for
/// byte: the same paths, built into a volume by `media::fat12` and read
/// back, list in the order scaffold writes, here for names whose bytes
/// below `.` order them otherwise than their 11-byte directory field.
#[test]
fn a_file_set_lists_each_folder_in_the_fat_builders_name_order() {
    let temp = Temp::new("fat-order");
    let names = ["A-B.TXT", "A.TXT", "AB", "AB-C.TXT", "AB.TXT"];
    for name in names {
        temp.write(&format!("TREE/{name}"), name.as_bytes());
        temp.write(&format!("TREE/D-1/{name}"), name.as_bytes());
        temp.write(&format!("TREE/D/{name}"), name.as_bytes());
    }
    let (code, output, errors) =
        upcli(&["recipe".into(), "scaffold".into(), temp.path("TREE").into()]);
    assert_eq!((code, errors.as_str()), (0, ""));
    let listed: Vec<String> = output
        .lines()
        .filter_map(|l| l.strip_prefix("    { path = \""))
        .map(|l| l[..l.find('"').unwrap()].to_owned())
        .collect();
    let entries: Vec<media::fat12::Entry> = listed
        .iter()
        .map(|path| media::fat12::Entry::file(path.clone(), path.as_bytes()))
        .collect();
    let volume = media::fat12::Volume::new(media::fat12::FloppyFormat::Hd144);
    let image = media::fat12::plan_volumes(entries, &volume)
        .unwrap()
        .image(0)
        .unwrap();
    let reader = media::fat12::reader::Volume::open(&image).unwrap();
    let mut built = Vec::new();
    for (folder, prefix) in [("", ""), ("D", "D/"), ("D-1", "D-1/")] {
        let (files, folders): (Vec<_>, Vec<_>) = reader
            .list(folder)
            .unwrap()
            .into_iter()
            .filter(|e| e.name != "." && e.name != "..")
            .partition(|e| !e.is_dir());
        built.extend(files.iter().map(|e| format!("{prefix}{}", e.name)));
        if folder.is_empty() {
            assert_eq!(
                folders.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                ["D", "D-1"],
                "folders after files, by the field"
            );
        }
    }
    assert_eq!(listed, built);
    assert_eq!(
        &listed[..5],
        ["A.TXT", "A-B.TXT", "AB", "AB.TXT", "AB-C.TXT"]
    );
}

/// The user-ROM roots, as the other user-file tests take them.
fn roots() -> Vec<PathBuf> {
    machine_file::firmware_discovery::default_search_roots()
        .into_iter()
        .filter(|root| root.is_dir())
        .collect()
}

/// Every file below the roots whose size is `size` and SHA-256 `sha256`,
/// symbolic links followed, each folder visited once.
fn find_file(size: u64, sha256: &str) -> Option<PathBuf> {
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
        fs::metadata(path).is_ok_and(|m| m.len() == size)
            && fs::read(path).is_ok_and(|bytes| {
                image_store::Digest::of(&bytes).to_string() == format!("sha256:{sha256}")
            })
    };
    roots()
        .iter()
        .find_map(|root| walk(root, &mut seen, &mut found))
}

/// Skip with the user-ROM line, or fail when the user's files are required.
fn skip(what: &str) {
    let message = format!(
        "SKIPPED: user-supplied files of {what} not found (set UNEMUPC_USER_ROMS_DIR or see vendor/user-roms/README.md)"
    );
    assert!(
        std::env::var_os("UNEMUPC_REQUIRE_USER_ROMS").as_deref() != Some(std::ffi::OsStr::new("1")),
        "{message}"
    );
    println!("{message}");
}

/// The lines of the corpus file from the line `first` up to the next blank
/// line, each ending in a line feed.
fn block(text: &str, first: &str) -> String {
    let start = text
        .lines()
        .position(|l| l == first)
        .unwrap_or_else(|| panic!("no line {first:?}"));
    text.lines()
        .skip(start)
        .take_while(|l| !l.is_empty())
        .map(|l| format!("{l}\n"))
        .collect()
}

/// Scaffold one operand, named `name` in a scratch folder by a symbolic
/// link to the user's file or folder, as the author names it.
fn scaffold_as(found: &Path, name: &str) -> String {
    let temp = Temp::new(&format!("user-{}", name.to_ascii_lowercase()));
    let named = temp.path(name);
    #[cfg(unix)]
    std::os::unix::fs::symlink(found, &named).unwrap();
    #[cfg(not(unix))]
    fs::copy(found, &named).unwrap();
    let (code, output, errors) = upcli(&["recipe".into(), "scaffold".into(), named.into()]);
    assert_eq!(code, 0, "scaffold failed: {}", errors.len());
    output
}

/// recipes.md §19.2 B9: scaffolding LABFULL.ZIP gives the corpus's input
/// table and member list. The corpus's measured parts are compared: its
/// `urls`, which the author adds, are left out of the input table, and the
/// step's id (`files`) is the author's name for what scaffold calls
/// `labfull-files`.
#[test]
#[ignore = "needs the user's own copy of LABFULL.ZIP; see vendor/user-roms/README.md"]
fn user_rom_scaffold_of_labfull_zip_gives_the_corpus_tables() {
    let corpus =
        fs::read_to_string(format!("{CORPUS}/kenslab-2.01/kenslab-2.01.media.toml")).unwrap();
    let Some(found) = find_file(
        940_242,
        "643aa0ca8d1a7d2c88afecd952da00f3b912be8b83c4c1d2f4ab9b50491fadfd",
    ) else {
        return skip("LABFULL.ZIP");
    };
    let mut input = String::new();
    let mut in_urls = false;
    for line in block(&corpus, "[input.labfull]").lines() {
        if line.starts_with("urls = [") {
            in_urls = true;
        } else if in_urls {
            in_urls = line != "]";
        } else {
            input.push_str(&format!("{line}\n"));
        }
    }
    let step = block(&corpus, "[[step]]").replace("id = \"files\"", "id = \"labfull-files\"");
    let expected = format!("{input}\n{step}");
    let actual = scaffold_as(&found, "LABFULL.ZIP");
    // Only a verdict is printed, never the tables of the user's file.
    assert!(
        actual == expected,
        "the scaffold of LABFULL.ZIP differs from the corpus"
    );
}

/// recipes.md §19.2 B9: scaffolding the AZURAMON folder gives the corpus's
/// file set, 561 entries, byte for byte. The input's id (`game`) is the
/// author's name for what scaffold calls `azuramon`. The folder is the one
/// that holds the game's AZURAMON.COM, found by its size and hash.
#[test]
#[ignore = "needs the user's own copy of the AZURAMON folder; see vendor/user-roms/README.md"]
fn user_rom_scaffold_of_the_azuramon_folder_gives_the_corpus_table() {
    let corpus =
        fs::read_to_string(format!("{CORPUS}/azuramon/azuramoon-2026.media.toml")).unwrap();
    let Some(com) = find_file(
        768,
        "f9ed092b86c64bcf3e3ab001fd23ee170e7a0072174c112d0c23322ca84825e7",
    )
    .filter(|path| path.file_name().is_some_and(|n| n == "AZURAMON.COM")) else {
        return skip("AZURAMON");
    };
    let expected = block(&corpus, "[input.game]").replace("[input.game]", "[input.azuramon]");
    assert_eq!(
        expected.lines().filter(|l| l.contains("sha256")).count(),
        561
    );
    let actual = scaffold_as(com.parent().unwrap(), "AZURAMON");
    assert!(
        actual == expected,
        "the scaffold of AZURAMON differs from the corpus"
    );
}
