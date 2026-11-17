// SPDX-License-Identifier: AGPL-3.0-or-later
//! Builder tests read every image back through the independent reader.

use super::reader::Volume;
use super::*;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct Scratch(PathBuf);

impl Scratch {
    pub(super) fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "unemupc-fat12-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(super) fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    pub(super) fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    pub(super) fn folder(&self, relative: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pattern(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn request(sources: Vec<PathBuf>, format: FloppyFormat) -> Request {
    Request {
        sources,
        top: None,
        format,
        label: None,
        exclude: Vec::new(),
    }
}

fn names(hosts: &[&str]) -> Vec<(String, bool)> {
    let hosts: Vec<&OsStr> = hosts.iter().map(OsStr::new).collect();
    names::assign(&hosts)
        .unwrap()
        .into_iter()
        .map(|(name, renamed)| (name.to_string(), renamed))
        .collect()
}

/// SHA-256 of blank images, `(format, labelled, today, before)`. `before`
/// is what the command-line formatter wrote before it moved into this module
/// (MEASURED from the previous release's code path), with a boot sector that
/// only halted. The pins were taken again once, when that halt loop gave way
/// to the data-disk boot code: an image with its code area put back to the
/// halt loop still hashes to `before`, so nothing outside the boot code
/// changed.
const BLANK_SHA256: [(FloppyFormat, bool, &str, &str); 8] = [
    (
        FloppyFormat::Hd144,
        false,
        "ad5f8c958565c134af3610e34a5d30fe641869a9593fefad597a1b3249890cee",
        "1b5a59272b41b2250b168d0a36d434ea1e3b93d17e29549ed161770c75460a51",
    ),
    (
        FloppyFormat::Hd144,
        true,
        "2791cb26479fa6a313a1ef8e8af3501139c7e53095e0649637cea70427fdd05e",
        "ce1cde123d7654476c1701c67ebd6d4aa2a2ebb2283fad0f4b3c65c2b35acb30",
    ),
    (
        FloppyFormat::Hd12,
        false,
        "7bb3e03fbb64d303832e01f48db57734b1c5e9ae74faf269203e70ea8778e061",
        "aa811c1ebee755d42b74b195cba4607ce78f3839fa916195e2e71a8388f795ac",
    ),
    (
        FloppyFormat::Hd12,
        true,
        "a5c61e6a375f08bc99dc110a0dc257e771b2f1acbdfbd30e41ae2cf4cdbc6443",
        "b4fa9598d115f73fa63ed7f668dded56c738b80d23402b0489c0f3c78554a11d",
    ),
    (
        FloppyFormat::Dd720,
        false,
        "ab982731bf2232aa554224b642916f962b28dbbddad24a6588d2c3bbc10124c9",
        "5a0ef43713bcbd4101bbf430f8600dab868c0292b512e72a11a7c65a82cec27a",
    ),
    (
        FloppyFormat::Dd720,
        true,
        "896aaead449eddb56a113dd998a15ca55afeb873c15df9bae4502e6edce479ee",
        "3f21c59e2d4d5ae30582544e57bb9d5c0ec9e8e97df343b378bbd6df8a07e017",
    ),
    (
        FloppyFormat::Dd360,
        false,
        "36fb37973a5e2ade4d1f6f5ae1a666acaf1d537dec213693e47dd4f88ca2182c",
        "42b2f137eabbcec61ade48304df9dc088bbf331df0bc2ff7a3a1dcea7828c1ef",
    ),
    (
        FloppyFormat::Dd360,
        true,
        "d43ecb1d4c0bba5345b0b51733ed01e5534ebceee79fa0030866adfefa8322f9",
        "ab694193a5ceda0c07acddff3491564405467c53243baf5030ce1581ca161136",
    ),
];

/// The image with its boot code replaced by the earlier halt loop
/// (`cli; hlt; jmp $-1`) and zeros up to the signature.
fn with_halt_loop(image: &[u8]) -> Vec<u8> {
    let mut image = image.to_vec();
    image[62..510].fill(0);
    image[62..66].copy_from_slice(&[0xFA, 0xF4, 0xEB, 0xFD]);
    image
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn blank_volumes_are_byte_identical_to_the_previous_formatter() {
    let scratch = Scratch::new("blank");
    let empty = scratch.file("empty.txt", b"");
    for (format, labelled, expected, before) in BLANK_SHA256 {
        let label = labelled.then(|| volume_label("test disk").unwrap());
        let mut blank = format_metadata(format, label);
        blank.resize(format.image_bytes() as usize, 0);
        assert_eq!(sha256(&blank), expected, "{format} labelled={labelled}");
        assert_eq!(sha256(&with_halt_loop(&blank)), before, "{format}");
        // The builder starts from the same bytes: an empty file adds only its
        // root entry, and no cluster.
        let (mut built, plan) = build(&Request {
            label,
            ..request(vec![empty.clone()], format)
        })
        .unwrap();
        let slot = format.root_start() + usize::from(labelled) * ENTRY;
        assert_eq!(&built[slot..slot + 11], b"EMPTY   TXT");
        built[slot..slot + ENTRY].fill(0);
        assert_eq!(built, blank, "{format}");
        assert_eq!(plan.disks[0].free_bytes, format.data_bytes());
        assert!(matches!(
            build(&Request {
                label,
                ..request(Vec::new(), format)
            }),
            Err(BuildError::NothingToAdd)
        ));
    }
}

#[test]
fn format_table_matches_the_at_diskette_formats() {
    for (format, sectors, data_bytes) in [
        (FloppyFormat::Hd144, 2880, 2847 * 512),
        (FloppyFormat::Hd12, 2400, 2371 * 512),
        (FloppyFormat::Dd720, 1440, 713 * 1024),
        (FloppyFormat::Dd360, 720, 354 * 1024),
    ] {
        assert_eq!(format.total_sectors(), sectors);
        assert_eq!(format.image_bytes(), u64::from(sectors) * 512);
        assert_eq!(format.data_bytes(), data_bytes);
        assert_eq!(FloppyFormat::from_name(format.name()), Some(format));
    }
    assert_eq!(FloppyFormat::from_name("2.88M"), None);
}

#[test]
fn every_format_reads_back_files_and_folders() {
    let scratch = Scratch::new("formats");
    let files = [
        ("readme.txt", pattern(700, 1)),
        ("GAME.EXE", pattern(3000, 2)),
        ("data/level1.dat", pattern(5000, 3)),
        ("data/maps/m1.map", pattern(10, 4)),
        ("data/empty.txt", Vec::new()),
    ];
    for (relative, bytes) in &files {
        scratch.file(relative, bytes);
    }
    scratch.folder("docs");
    let sources = ["readme.txt", "GAME.EXE", "data", "docs"].map(|name| scratch.path(name));
    for format in FloppyFormat::ALL {
        let (image, plan) = build(&Request {
            label: Some(volume_label("TEST DISK").unwrap()),
            ..request(sources.to_vec(), format)
        })
        .unwrap();
        assert_eq!(image.len() as u64, format.image_bytes());
        let layout = format.layout();
        let word = |at: usize| u16::from_le_bytes([image[at], image[at + 1]]);
        assert_eq!(word(11), 512);
        assert_eq!(image[13], layout.cluster_sectors);
        assert_eq!((word(14), image[16]), (1, 2));
        assert_eq!(word(17), layout.root_entries);
        assert_eq!(u32::from(word(19)), format.total_sectors());
        assert_eq!(image[21], layout.media);
        assert_eq!(word(22), layout.fat_sectors);
        assert_eq!((word(24), word(26)), (layout.sectors_per_track, 2));
        assert_eq!(&image[43..54], b"TEST DISK  ");

        let volume = Volume::open(&image).unwrap();
        volume.check().unwrap();
        assert_eq!(volume.label().as_deref(), Some("TEST DISK"));
        let root: Vec<_> = volume
            .list("")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        // Files first, then folders, each by the bytes of the stored names.
        assert_eq!(root, ["GAME.EXE", "README.TXT", "DATA", "DOCS"], "{format}");
        assert_eq!(volume.list("DOCS").unwrap().len(), 2, "only . and ..");
        for (relative, bytes) in &files {
            let path = relative.to_ascii_uppercase().replace('/', "\\");
            assert_eq!(&volume.read(&path).unwrap(), bytes, "{format} {path}");
        }
        let disk = &plan.disks[0];
        assert_eq!(
            disk.files
                .iter()
                .map(|file| file.dos_path.as_str())
                .collect::<Vec<_>>(),
            // Folder order: the root's own files, then each folder's.
            [
                "GAME.EXE",
                "README.TXT",
                "DATA\\EMPTY.TXT",
                "DATA\\LEVEL1.DAT",
                "DATA\\MAPS\\M1.MAP",
            ]
        );
        assert_eq!(disk.files[3].host, scratch.path("data/level1.dat"));
        assert_eq!(disk.files[3].bytes, 5000);
        assert_eq!(disk.free_bytes as usize, volume.free_bytes());
        assert_eq!(disk.used_bytes + disk.free_bytes, format.data_bytes());
        assert!(plan.renames.is_empty(), "case changes are not renames");
        assert!(plan.skipped.is_empty());
    }
}

#[test]
fn folders_start_with_dot_entries_grow_by_clusters_and_use_fixed_dates() {
    let scratch = Scratch::new("folders");
    for index in 0..20 {
        scratch.file(&format!("many/f{index:02}.txt"), &[b'a' + index]);
    }
    scratch.file("many/deep/inner.txt", b"inner");
    let (image, _) = build(&request(vec![scratch.path("many")], FloppyFormat::Hd144)).unwrap();
    let volume = Volume::open(&image).unwrap();
    volume.check().unwrap();
    let many = volume
        .list("")
        .unwrap()
        .into_iter()
        .find(|e| e.name == "MANY")
        .unwrap();
    assert_eq!(many.attributes, 0x10);
    assert_eq!(many.size, 0);
    // 23 entries of 32 bytes need two 512-byte clusters.
    assert_eq!(volume.chain(many.first_cluster).unwrap().len(), 2);
    let entries = volume.list("MANY").unwrap();
    assert_eq!(entries.len(), 23);
    assert_eq!(
        (entries[0].name.as_str(), entries[0].first_cluster),
        (".", many.first_cluster)
    );
    assert_eq!(
        (entries[1].name.as_str(), entries[1].first_cluster),
        ("..", 0)
    );
    let deep = volume.list("MANY\\DEEP").unwrap();
    assert_eq!(deep[1].first_cluster, many.first_cluster);
    assert_eq!(volume.read("MANY\\DEEP\\INNER.TXT").unwrap(), b"inner");
    for (_, entry) in volume.walk().unwrap() {
        assert!(matches!(entry.attributes, 0x00 | 0x10), "{entry:?}");
        assert_eq!(
            &entry.raw[12..16],
            &[0; 4],
            "reserved byte and creation time"
        );
        assert_eq!(&entry.raw[16..20], &[0x21, 0, 0x21, 0], "1980-01-01");
        assert_eq!(&entry.raw[20..24], &[0; 4], "high cluster and write time");
        assert_eq!(&entry.raw[24..26], &[0x21, 0], "1980-01-01");
    }
}

#[test]
fn short_names_follow_the_basis_rules() {
    for (host, expected, renamed) in [
        ("readme.txt", "README.TXT", false),
        ("GAME.EXE", "GAME.EXE", false),
        ("MiXeD.TxT", "MIXED.TXT", false),
        ("ab~1.txt", "AB~1.TXT", false),
        ("long file name.txt", "LONG_FIL.TXT", true),
        ("verylongname.extension", "VERYLONG.EXT", true),
        ("archive.tar.gz", "ARCHIVET.GZ", true),
        ("x.y.z", "XY.Z", true),
        (".hidden", "HIDDEN", true),
        ("a+b=c[1].dat", "A_B_C_1_.DAT", true),
        ("caf\u{e9}.txt", "CAFE.TXT", true),
        ("\u{c4}bc.txt", "ABC.TXT", true),
        ("A\u{308}bc.txt", "ABC.TXT", true),
        ("\u{142}\u{f3}d\u{17a}", "_ODZ", true),
        ("\u{c6}sop", "_SOP", true),
        ("\u{301}", "_", true),
        ("a\tb", "A_B", true),
        ("noext.", "NOEXT", true),
        ("...", "_", true),
        ("con.txt", "CON_.TXT", true),
        ("lpt1", "LPT1_", true),
    ] {
        assert_eq!(names(&[host]), [(expected.to_owned(), renamed)], "{host:?}");
    }
}

#[test]
fn collisions_take_numeric_tails_in_sorted_order() {
    // An exact name keeps its name even though a converted one sorts first.
    assert_eq!(
        names(&["long file one.txt", "long file two.txt", "long_fil.txt"]),
        [
            ("LONG_F~1.TXT".to_owned(), true),
            ("LONG_F~2.TXT".to_owned(), true),
            ("LONG_FIL.TXT".to_owned(), false),
        ]
    );
    assert_eq!(
        names(&["README.TXT", "readme.txt"]),
        [
            ("README.TXT".to_owned(), false),
            ("README~1.TXT".to_owned(), true)
        ]
    );
    assert_eq!(
        names(&["ABCDEF~1.TXT", "abcdefghij.txt", "abcdefghik.txt"]),
        [
            ("ABCDEF~1.TXT".to_owned(), false),
            ("ABCDEFGH.TXT".to_owned(), true),
            ("ABCDEF~2.TXT".to_owned(), true),
        ]
    );
    let hosts: Vec<String> = (0..12).map(|n| format!("document {n:02}.txt")).collect();
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let assigned = names(&hosts);
    assert_eq!(assigned[0].0, "DOCUMENT.TXT");
    assert_eq!(assigned[1].0, "DOCUME~1.TXT");
    assert_eq!(assigned[9].0, "DOCUME~9.TXT");
    assert_eq!(assigned[10].0, "DOCUM~10.TXT");
    assert_eq!(assigned[11].0, "DOCUM~11.TXT");
}

#[test]
fn renames_are_reported_as_host_path_to_dos_path() {
    let scratch = Scratch::new("renames");
    scratch.file("My Game/long file one.txt", b"1");
    scratch.file("My Game/long file two.txt", b"2");
    scratch.file("My Game/LONG_FIL.TXT", b"3");
    scratch.file("My Game/play.bat", b"4");
    let (image, plan) =
        build(&request(vec![scratch.path("My Game")], FloppyFormat::Dd360)).unwrap();
    let renames: Vec<_> = plan
        .renames
        .iter()
        .map(|rename| (rename.host.clone(), rename.dos_path.as_str()))
        .collect();
    assert_eq!(
        renames,
        [
            (scratch.path("My Game"), "MY_GAME"),
            (
                scratch.path("My Game/long file one.txt"),
                "MY_GAME\\LONG_F~1.TXT"
            ),
            (
                scratch.path("My Game/long file two.txt"),
                "MY_GAME\\LONG_F~2.TXT"
            ),
        ]
    );
    let volume = Volume::open(&image).unwrap();
    assert_eq!(volume.read("MY_GAME\\LONG_FIL.TXT").unwrap(), b"3");
    assert_eq!(volume.read("MY_GAME\\LONG_F~2.TXT").unwrap(), b"2");
}

#[test]
fn top_folder_holds_every_source() {
    let scratch = Scratch::new("top");
    let sources = vec![
        scratch.file("loose/game.exe", b"exe"),
        scratch.file("other/data.dat", b"dat"),
    ];
    let (image, plan) = build(&Request {
        top: ShortName::parse("mygame"),
        ..request(sources, FloppyFormat::Hd12)
    })
    .unwrap();
    let volume = Volume::open(&image).unwrap();
    volume.check().unwrap();
    let paths: Vec<_> = volume.walk().unwrap().into_iter().map(|(p, _)| p).collect();
    assert_eq!(paths, ["MYGAME", "MYGAME\\DATA.DAT", "MYGAME\\GAME.EXE"]);
    assert!(plan.renames.is_empty());
}

#[test]
fn typed_short_names_are_checked_not_converted() {
    for (text, expected) in [
        ("AZURAMON", Some("AZURAMON")),
        ("game", Some("GAME")),
        ("game.v1", Some("GAME.V1")),
        ("A~1", Some("A~1")),
        ("toolongname", None),
        ("a.b.c", None),
        ("a.", None),
        (".a", None),
        ("", None),
        ("bad/x", None),
        ("two words", None),
        ("con", None),
        ("com1.txt", None),
    ] {
        assert_eq!(
            ShortName::parse(text)
                .map(|name| name.to_string())
                .as_deref(),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn junk_and_symlinks_inside_folders_are_skipped_with_reasons() {
    let scratch = Scratch::new("junk");
    scratch.file("game/GAME.EXE", b"game");
    for junk in [".DS_Store", "._GAME.EXE", "Thumbs.db", "DESKTOP.INI"] {
        scratch.file(&format!("game/{junk}"), b"junk");
    }
    scratch.file("game/__MACOSX/._GAME.EXE", b"junk");
    let mut expected = Vec::new();
    // Host-name byte order within the folder.
    for junk in [
        ".DS_Store",
        "._GAME.EXE",
        "DESKTOP.INI",
        "Thumbs.db",
        "__MACOSX",
    ] {
        expected.push((scratch.path(&format!("game/{junk}")), SkipReason::HostJunk));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("GAME.EXE", scratch.path("game/link.exe")).unwrap();
        symlink("..", scratch.path("game/up")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(scratch.path("game/pipe"))
            .status()
            .unwrap()
            .success());
        expected.extend([
            (scratch.path("game/link.exe"), SkipReason::SymbolicLink),
            (scratch.path("game/pipe"), SkipReason::NotRegular),
            (scratch.path("game/up"), SkipReason::SymbolicLink),
        ]);
    }
    let (image, plan) = build(&request(vec![scratch.path("game")], FloppyFormat::Hd144)).unwrap();
    let skipped: Vec<_> = plan
        .skipped
        .iter()
        .map(|skip| (skip.host.clone(), skip.reason))
        .collect();
    assert_eq!(skipped, expected);
    let volume = Volume::open(&image).unwrap();
    let paths: Vec<_> = volume.walk().unwrap().into_iter().map(|(p, _)| p).collect();
    assert_eq!(paths, ["GAME", "GAME\\GAME.EXE"]);
}

#[test]
fn named_sources_are_refused_rather_than_skipped() {
    let scratch = Scratch::new("named");
    let game = scratch.file("game/GAME.EXE", b"game");
    let junk = scratch.file(".DS_Store", b"junk");
    let mut refused = vec![(junk, SkipReason::HostJunk, "is host metadata")];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("game", scratch.path("linked")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(scratch.path("pipe"))
            .status()
            .unwrap()
            .success());
        refused.push((
            scratch.path("linked"),
            SkipReason::SymbolicLink,
            "is a symbolic link; name its target",
        ));
        refused.push((
            scratch.path("pipe"),
            SkipReason::NotRegular,
            "is not a regular",
        ));
    }
    for (source, expected, message) in refused {
        let error = plan(&request(
            vec![game.clone(), source.clone()],
            FloppyFormat::Hd144,
        ))
        .unwrap_err();
        assert!(
            matches!(&error, BuildError::Refused { path, reason } if *path == source && *reason == expected),
            "{error:?}"
        );
        assert!(error.to_string().contains(message), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn sources_naming_the_same_host_entry_are_refused() {
    let scratch = Scratch::new("same");
    scratch.file("g/one.txt", b"1");
    let g = scratch.path("g");
    for again in [g.clone(), scratch.path("g/../g"), scratch.path("./g")] {
        let error = plan(&request(
            vec![g.clone(), again.clone()],
            FloppyFormat::Hd144,
        ))
        .unwrap_err();
        assert!(
            matches!(&error, BuildError::SameSource { first, second } if *first == g && *second == again),
            "{error:?}"
        );
        let message = error.to_string();
        assert!(message.contains(&g.display().to_string()), "{message}");
    }
    // A case-insensitive host file system reaches one folder by two spellings.
    if scratch.path("G").exists() {
        assert!(matches!(
            plan(&request(vec![g, scratch.path("G")], FloppyFormat::Hd144)),
            Err(BuildError::SameSource { .. })
        ));
    }
}

#[test]
fn folder_paths_longer_than_dos_allows_are_refused() {
    let scratch = Scratch::new("deep");
    // Six folders of nine characters (\DDDDDDDD) and a last one of ten
    // (\EEEEEEE.F) make exactly 64 characters.
    let deep = "dddddddd/".repeat(6);
    scratch.file(&format!("{deep}eeeeeee.f/file.txt"), b"deep");
    let first = scratch.path("dddddddd");
    let (image, _) = build(&request(vec![first.clone()], FloppyFormat::Hd144)).unwrap();
    let volume = Volume::open(&image).unwrap();
    let path = "DDDDDDDD\\".repeat(6) + "EEEEEEE.F";
    assert_eq!(1 + path.len(), 64);
    assert_eq!(volume.read(&format!("{path}\\FILE.TXT")).unwrap(), b"deep");
    // The top folder counts: \T makes the last folder 66 characters.
    let error = plan(&Request {
        top: ShortName::parse("T"),
        ..request(vec![first.clone()], FloppyFormat::Hd144)
    })
    .unwrap_err();
    match &error {
        BuildError::PathTooLong {
            host,
            dos_path,
            length,
        } => {
            assert_eq!(*host, scratch.path(&format!("{deep}eeeeeee.f")));
            assert_eq!(
                (dos_path.as_str(), *length),
                (format!("\\T\\{path}").as_str(), 66)
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(error.to_string().contains("at most 64"), "{error}");
}

#[test]
fn excluded_outputs_inside_a_source_tree_are_skipped() {
    let scratch = Scratch::new("exclude");
    scratch.file("game/GAME.EXE", b"game");
    let output = scratch.file("game/game.img", b"previous image");
    let plan = plan(&Request {
        exclude: vec![output.clone(), scratch.path("game/missing.img")],
        ..request(vec![scratch.path("game")], FloppyFormat::Hd144)
    })
    .unwrap();
    assert_eq!(
        plan.skipped,
        [Skipped {
            host: output,
            reason: SkipReason::Output
        }]
    );
    assert_eq!(plan.disks[0].files.len(), 1);
}

#[cfg(unix)]
#[test]
fn a_pipe_swapped_in_after_planning_is_refused_without_blocking() {
    let scratch = Scratch::new("pipe");
    let file = scratch.file("data.bin", b"data");
    let plan = plan(&request(vec![file.clone()], FloppyFormat::Hd144)).unwrap();
    fs::remove_file(&file).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .arg(&file)
        .status()
        .unwrap()
        .success());
    // A blocking open would wait for a writer forever; fail instead of hanging.
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || send.send(plan.image(0)).unwrap());
    let result = receive
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("building blocked on the pipe");
    assert!(matches!(
        result,
        Err(BuildError::Changed { path }) if path == file
    ));
}

#[test]
fn output_ignores_input_order_and_host_times() {
    let scratch = Scratch::new("determinism");
    let mut files = Vec::new();
    for (index, relative) in [
        "b/zeta.txt",
        "b/alpha.txt",
        "b/Mid.TXT",
        "a/one.bin",
        "a/sub/two.bin",
        "root.txt",
    ]
    .iter()
    .enumerate()
    {
        files.push(scratch.file(relative, &pattern(600 * (index + 1), index as u8)));
    }
    let mut sources = vec![
        scratch.path("b"),
        scratch.path("a"),
        scratch.path("root.txt"),
    ];
    let first = build(&request(sources.clone(), FloppyFormat::Dd720))
        .unwrap()
        .0;
    for (index, file) in files.iter().enumerate() {
        let time =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_007 * index as u64);
        fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }
    sources.reverse();
    let second = build(&request(sources.clone(), FloppyFormat::Dd720))
        .unwrap()
        .0;
    sources.swap(0, 1);
    let third = build(&request(sources, FloppyFormat::Dd720)).unwrap().0;
    assert!(
        first == second && second == third,
        "same inputs, same bytes"
    );
}

#[test]
fn exact_fit_and_one_cluster_short() {
    let scratch = Scratch::new("fit");
    let format = FloppyFormat::Dd360;
    let cluster = format.cluster_bytes() as usize;
    let data = format.data_bytes() as usize;
    let a = scratch.file("a.bin", &pattern(data - cluster, 1));
    let b = scratch.file("b.bin", &pattern(cluster, 2));
    let (image, plan) = build(&request(vec![a.clone(), b.clone()], format)).unwrap();
    assert_eq!(plan.disks[0].free_bytes, 0);
    let volume = Volume::open(&image).unwrap();
    volume.check().unwrap();
    assert_eq!(volume.free_bytes(), 0);
    assert_eq!(volume.read("B.BIN").unwrap(), pattern(cluster, 2));

    // One byte more costs a whole cluster.
    fs::write(&b, pattern(cluster + 1, 2)).unwrap();
    match build(&request(vec![a.clone(), b], format)) {
        Err(BuildError::DoesNotFit {
            needed,
            capacity,
            shortfall,
            ..
        }) => assert_eq!(
            (needed, capacity, shortfall),
            (format.data_bytes() + 1024, format.data_bytes(), 1024)
        ),
        other => panic!("{other:?}"),
    }
    // A one-byte file in a folder needs the folder's cluster too.
    scratch.file("sub/c.bin", b"c");
    let error = build(&request(vec![a, scratch.path("sub")], format)).unwrap_err();
    assert!(
        matches!(
            error,
            BuildError::DoesNotFit {
                shortfall: 1024,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("1024 bytes short"), "{error}");
}

#[test]
fn a_file_larger_than_the_data_area_is_named() {
    let scratch = Scratch::new("large");
    let format = FloppyFormat::Hd144;
    let whole = scratch.file("whole.bin", &pattern(format.data_bytes() as usize, 7));
    let (image, _) = build(&request(vec![whole.clone()], format)).unwrap();
    assert_eq!(
        Volume::open(&image)
            .unwrap()
            .read("WHOLE.BIN")
            .unwrap()
            .len(),
        1_457_664
    );
    let over = scratch.file(
        "big/over.bin",
        &pattern(format.data_bytes() as usize + 1, 7),
    );
    type Planner = fn(&Request) -> Result<Plan, BuildError>;
    let planners: [Planner; 2] = [plan, plan_split];
    for plan in planners {
        match plan(&request(vec![whole.clone(), scratch.path("big")], format)) {
            Err(error @ BuildError::FileTooLarge { .. }) => {
                let BuildError::FileTooLarge {
                    host,
                    dos_path,
                    bytes,
                    room,
                    ..
                } = &error
                else {
                    unreachable!()
                };
                assert_eq!((host, dos_path.as_str()), (&over, "BIG\\OVER.BIN"));
                assert_eq!(*bytes, 1_457_665);
                assert_eq!(*room, 1_457_664 - 512, "less the folder's cluster");
                assert!(error.to_string().contains("over.bin"), "{error}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn the_fixed_root_directory_overflows_into_an_error() {
    let scratch = Scratch::new("root");
    let format = FloppyFormat::Hd144;
    let sources: Vec<_> = (0..225)
        .map(|index| scratch.file(&format!("f{index:03}"), b""))
        .collect();
    assert!(plan(&request(sources[..224].to_vec(), format)).is_ok());
    let labelled = Request {
        label: Some(volume_label("FULL").unwrap()),
        ..request(sources[..224].to_vec(), format)
    };
    assert!(matches!(
        plan(&labelled),
        Err(BuildError::RootFull {
            needed: 225,
            capacity: 224,
            ..
        })
    ));
    assert!(matches!(
        plan(&request(sources.clone(), format)),
        Err(BuildError::RootFull { .. })
    ));
    let under_top = Request {
        top: ShortName::parse("ALL"),
        ..request(sources.clone(), format)
    };
    let (image, _) = build(&under_top).unwrap();
    assert_eq!(
        Volume::open(&image).unwrap().list("ALL").unwrap().len(),
        227
    );
    // A split starts a new disk when the root is full.
    let split = plan_split(&request(sources, format)).unwrap();
    assert_eq!(split.disks.len(), 2);
    assert_eq!(split.disks[1].files.len(), 1);
}

#[test]
fn split_sets_carry_the_full_path_on_every_disk() {
    let scratch = Scratch::new("split");
    let mut expected = Vec::new();
    for (index, relative) in [
        "azura/level1/a.dat",
        "azura/level1/b.dat",
        "azura/level1/c.dat",
        "azura/level2/a.dat",
        "azura/level2/b.dat",
        "azura/level2/c.dat",
    ]
    .iter()
    .enumerate()
    {
        let bytes = pattern(150_000, index as u8);
        scratch.file(relative, &bytes);
        expected.push((relative.to_ascii_uppercase().replace('/', "\\"), bytes));
    }
    scratch.file("azura/readme.txt", b"read me");
    expected.push(("AZURA\\README.TXT".into(), b"read me".to_vec()));
    scratch.folder("azura/saves");
    let format = FloppyFormat::Dd720;
    let (images, plan) = build_split(&Request {
        label: Some(volume_label("AZURA").unwrap()),
        ..request(vec![scratch.path("azura")], format)
    })
    .unwrap();
    // 147 clusters per file: the one-cluster README and four of them fit
    // beside three folders, the sixth does not.
    let counts: Vec<_> = plan.disks.iter().map(|disk| disk.files.len()).collect();
    assert_eq!(counts, [5, 2]);
    let mut found = Vec::new();
    for (index, (image, disk)) in images.iter().zip(&plan.disks).enumerate() {
        let label = format!("AZURA {}", index + 1);
        assert_eq!(disk.label, Some(volume_label(&label).unwrap()));
        let volume = Volume::open(image).unwrap();
        volume.check().unwrap();
        assert_eq!(volume.label(), Some(label));
        assert_eq!(volume.free_bytes() as u64, disk.free_bytes);
        assert_eq!(
            volume.list("").unwrap().len(),
            1,
            "only the top folder at the root"
        );
        for (path, entry) in volume.walk().unwrap() {
            assert!(path == "AZURA" || path.starts_with("AZURA\\"), "{path}");
            if !entry.is_dir() {
                found.push((path.clone(), volume.read(&path).unwrap()));
            }
        }
        for file in &disk.files {
            assert_eq!(
                volume.read(&file.dos_path).unwrap().len() as u64,
                file.bytes
            );
        }
    }
    found.sort();
    expected.sort();
    assert_eq!(found, expected, "every file exactly once");
    let in_order: Vec<_> = plan
        .disks
        .iter()
        .flat_map(|disk| disk.files.iter().map(|file| file.dos_path.clone()))
        .collect();
    assert_eq!(
        in_order[0], "AZURA\\README.TXT",
        "a folder's own files first"
    );
    assert_eq!(
        in_order[4], "AZURA\\LEVEL2\\A.DAT",
        "greedy in folder order"
    );
    let saves = Volume::open(&images[1])
        .unwrap()
        .list("AZURA\\SAVES")
        .unwrap();
    assert_eq!(saves.len(), 2, "an empty folder still travels");
}

#[test]
fn split_refuses_a_file_too_large_for_one_disk_and_keeps_one_disk_plain() {
    let scratch = Scratch::new("refuse");
    let format = FloppyFormat::Dd360;
    scratch.file("big/huge.bin", &pattern(format.data_bytes() as usize, 1));
    let error = plan_split(&request(vec![scratch.path("big")], format)).unwrap_err();
    assert!(
        matches!(&error, BuildError::FileTooLarge { dos_path, room, .. }
            if dos_path == "BIG\\HUGE.BIN" && *room == format.data_bytes() - 1024),
        "{error:?}"
    );
    let small = scratch.file("small.txt", b"small");
    let plan = plan_split(&Request {
        label: Some(volume_label("GAME").unwrap()),
        ..request(vec![small], format)
    })
    .unwrap();
    assert_eq!(plan.disks.len(), 1);
    assert_eq!(plan.disks[0].label, Some(volume_label("GAME").unwrap()));
}

#[test]
fn numbered_labels_fit_eleven_characters() {
    for (label, disk, expected) in [
        ("AZURAMON", 10, "AZURAMON 10"),
        ("QUIDPROQUO", 1, "QUIDPROQU 1"),
        ("TEST DISK", 2, "TEST DISK 2"),
        ("ABCDEFGH IJ", 3, "ABCDEFGH 3"),
    ] {
        assert_eq!(
            split::numbered(volume_label(label).unwrap(), disk),
            volume_label(expected).unwrap(),
            "{label} {disk}"
        );
    }
}

#[test]
fn unusable_sources_are_errors_and_changed_files_are_caught() {
    let scratch = Scratch::new("errors");
    assert!(matches!(
        plan(&request(vec![PathBuf::from("..")], FloppyFormat::Hd144)),
        Err(BuildError::Unnamed { .. })
    ));
    let missing = scratch.path("missing");
    match plan(&request(vec![missing.clone()], FloppyFormat::Hd144)) {
        Err(BuildError::Io { path, .. }) => assert_eq!(path, missing),
        other => panic!("{other:?}"),
    }
    let file = scratch.file("grow.txt", b"short");
    let plan = plan(&request(vec![file.clone()], FloppyFormat::Hd144)).unwrap();
    fs::write(&file, b"longer now").unwrap();
    assert!(matches!(
        plan.image(0),
        Err(BuildError::Changed { path }) if path == file
    ));
}

#[test]
fn volume_labels_reject_punctuation_and_overlength() {
    assert_eq!(&volume_label("Test Disk").unwrap(), b"TEST DISK  ");
    for bad in ["", " X", "TWELVE CHARS", "a/b", "caf\u{e9}"] {
        assert!(volume_label(bad).is_err(), "{bad:?}");
    }
}

fn read_items(paths: &[PathBuf]) -> Vec<HostItem> {
    paths
        .iter()
        .map(|path| HostItem::read(path).unwrap())
        .collect()
}

#[test]
fn host_items_count_their_tree_and_what_was_left_out() {
    let scratch = Scratch::new("items");
    scratch.file("game/GAME.EXE", &pattern(1000, 1));
    scratch.file("game/data/LEVEL1.DAT", &pattern(300, 2));
    scratch.file("game/data/.DS_Store", b"junk");
    scratch.folder("game/saves");
    scratch.file("game/._GAME.EXE", b"junk");
    let folder = HostItem::read(&scratch.path("game")).unwrap();
    assert!(folder.is_folder());
    assert_eq!(folder.host(), scratch.path("game"));
    assert_eq!(
        (
            folder.files(),
            folder.folders(),
            folder.bytes(),
            folder.skipped()
        ),
        (2, 2, 1300, 2)
    );
    let file = HostItem::read(&scratch.path("game/GAME.EXE")).unwrap();
    assert!(!file.is_folder());
    assert_eq!((file.files(), file.folders(), file.bytes()), (1, 0, 1000));
    // A named source that cannot be copied is refused, as a request's is.
    assert!(matches!(
        HostItem::read(&scratch.path("game/._GAME.EXE")),
        Err(BuildError::Refused {
            reason: SkipReason::HostJunk,
            ..
        })
    ));
}

#[test]
fn a_survey_plans_and_builds_what_a_request_builds() {
    let scratch = Scratch::new("survey-same");
    scratch.file("game/PLAY.EXE", &pattern(40_000, 3));
    scratch.file("game/Level Data/one.dat", &pattern(9_000, 4));
    let loose = scratch.file("Read Me.txt", &pattern(700, 5));
    let sources = vec![scratch.path("game"), loose];
    let top = ShortName::parse("mygame");
    let label = Some(volume_label("MYGAME").unwrap());
    for format in FloppyFormat::ALL {
        let (image, plan) = build(&Request {
            top,
            label,
            ..request(sources.clone(), format)
        })
        .unwrap();
        let mut items = read_items(&sources);
        let survey = Survey::new(&items, top, format, label).unwrap();
        let surveyed = survey.plan().unwrap();
        assert_eq!(surveyed.image(0).unwrap(), image, "{format}");
        assert_eq!(surveyed.renames, plan.renames);
        assert_eq!(survey.needed(), plan.disks[0].used_bytes, "{format}");
        assert_eq!(
            survey.landing(),
            ["\\MYGAME\\GAME\\", "\\MYGAME\\READ_ME.TXT"]
        );
        // The order the items were added in does not matter.
        items.reverse();
        let reversed = Survey::new(&items, top, format, label).unwrap();
        assert_eq!(reversed.plan().unwrap().image(0).unwrap(), image);
        assert_eq!(
            reversed.landing(),
            ["\\MYGAME\\READ_ME.TXT", "\\MYGAME\\GAME\\"]
        );
    }
}

#[test]
fn a_folder_added_alone_is_the_top_folder() {
    let scratch = Scratch::new("survey-alone");
    scratch.file("nightfall/nightfal.exe", &pattern(5_000, 6));
    scratch.file("nightfall/readme.txt", &pattern(512, 7));
    scratch.file("nightfall/.DS_Store", b"junk");
    let items = read_items(&[scratch.path("nightfall")]);
    let top = ShortName::from_host(OsStr::new("nightfall"));
    assert_eq!(top.to_string(), "NIGHTFAL");
    let survey = Survey::new(&items, Some(top), FloppyFormat::Hd144, None).unwrap();
    assert_eq!(survey.landing(), ["\\NIGHTFAL\\"]);
    assert_eq!(survey.skipped().len(), 1);
    assert_eq!(
        survey.skipped()[0].host,
        scratch.path("nightfall/.DS_Store")
    );
    let image = survey.plan().unwrap().image(0).unwrap();
    let volume = Volume::open(&image).unwrap();
    let paths: Vec<_> = volume.walk().unwrap().into_iter().map(|(p, _)| p).collect();
    assert_eq!(
        paths,
        ["NIGHTFAL", "NIGHTFAL\\NIGHTFAL.EXE", "NIGHTFAL\\README.TXT"]
    );
    // The command line makes the same disk from the folder alone, which it
    // puts at \NIGHTFAL\ by its own name.
    assert_eq!(
        build(&request(
            vec![scratch.path("nightfall")],
            FloppyFormat::Hd144
        ))
        .unwrap()
        .0,
        image
    );
    // Without a top folder its members go to the root.
    let root = Survey::new(&items, None, FloppyFormat::Hd144, None).unwrap();
    assert_eq!(root.landing(), ["\\"]);
    assert_eq!((root.folders(), root.root_entries()), (0, 2));
    let image = root.plan().unwrap().image(0).unwrap();
    let paths: Vec<_> = Volume::open(&image)
        .unwrap()
        .walk()
        .unwrap()
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(paths, ["NIGHTFAL.EXE", "README.TXT"]);
}

#[test]
fn a_survey_counts_space_as_dos_does_and_plans_the_split() {
    let scratch = Scratch::new("survey-space");
    let format = FloppyFormat::Dd720;
    let cluster = format.cluster_bytes();
    scratch.file("big/a.dat", &pattern(400_000, 1));
    scratch.file("big/b.dat", &pattern(400_000, 2));
    scratch.file("big/c.txt", &pattern(10, 3));
    let items = read_items(&[scratch.path("big")]);
    let label = Some(volume_label("BIG").unwrap());
    let survey = Survey::new(&items, ShortName::parse("big"), format, label).unwrap();
    assert_eq!((survey.files(), survey.folders()), (3, 1));
    assert_eq!(survey.file_bytes(), 800_010);
    assert_eq!(
        survey.file_space(),
        2 * 400_000u64.div_ceil(cluster) * cluster + cluster
    );
    assert_eq!(survey.folder_space(), cluster);
    assert_eq!(survey.root_entries(), 2, "the folder and the label");
    match survey.plan() {
        Err(BuildError::DoesNotFit { needed, .. }) => assert_eq!(needed, survey.needed()),
        other => panic!("{other:?}"),
    }
    let split = survey.plan_split().unwrap();
    assert_eq!(split.disks.len(), 2);
    assert_eq!(split.disks[0].label, Some(volume_label("BIG 1").unwrap()));
    assert_eq!(split.disks[1].files[0].dos_path, "BIG\\B.DAT");
    // One file larger than a disk is named for every way of laying it out.
    scratch.file(
        "huge/whole.bin",
        &pattern(format.data_bytes() as usize + 1, 8),
    );
    let huge = read_items(&[scratch.path("huge")]);
    let survey = Survey::new(&huge, None, format, None).unwrap();
    assert!(matches!(
        survey.plan(),
        Err(BuildError::FileTooLarge { .. })
    ));
    assert!(matches!(
        survey.plan_split(),
        Err(BuildError::FileTooLarge { .. })
    ));
    // Two items that reach one host entry are refused, as named sources are.
    #[cfg(unix)]
    {
        let twice = read_items(&[scratch.path("big/a.dat"), scratch.path("big/../big/a.dat")]);
        assert!(matches!(
            Survey::new(&twice, None, format, None),
            Err(BuildError::SameSource { .. })
        ));
    }
    assert!(matches!(
        Survey::new(&[], None, format, None),
        Err(BuildError::NothingToAdd)
    ));
}

#[test]
fn a_split_puts_a_folders_own_files_before_its_subfolders() {
    let scratch = Scratch::new("split-order");
    // AAA sorts before the loose files, which still go first.
    for (relative, length) in [
        ("game/AAA/one.dat", 300_000),
        ("game/AAA/two.dat", 300_000),
        ("game/AAA/deep/three.dat", 300_000),
        ("game/INSTALL.BAT", 100),
        ("game/README.TXT", 2_000),
        ("game/SETUP.EXE", 40_000),
        ("game/ZZZ/last.dat", 300_000),
    ] {
        scratch.file(relative, &pattern(length, length as u8));
    }
    let format = FloppyFormat::Dd720;
    let request = Request {
        top: None,
        ..request(vec![scratch.path("game")], format)
    };
    let (images, split) = build_split(&request).unwrap();
    let files: Vec<Vec<&str>> = split
        .disks
        .iter()
        .map(|disk| disk.files.iter().map(|f| f.dos_path.as_str()).collect())
        .collect();
    assert_eq!(
        files,
        [
            vec![
                "GAME\\INSTALL.BAT",
                "GAME\\README.TXT",
                "GAME\\SETUP.EXE",
                "GAME\\AAA\\ONE.DAT",
                "GAME\\AAA\\TWO.DAT",
            ],
            vec!["GAME\\AAA\\DEEP\\THREE.DAT", "GAME\\ZZZ\\LAST.DAT"],
        ]
    );
    // On each disk a folder's directory entries are its files, then its
    // folders: the order the split takes them in.
    let listed: Vec<String> = Volume::open(&images[0])
        .unwrap()
        .list("GAME")
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(
        listed,
        [".", "..", "INSTALL.BAT", "README.TXT", "SETUP.EXE", "AAA"]
    );
    // One disk lists its files in the same order.
    let one = plan(&Request {
        format: FloppyFormat::Hd144,
        ..request
    })
    .unwrap();
    assert_eq!(one.disks[0].files[0].dos_path, "GAME\\INSTALL.BAT");
    assert_eq!(
        one.disks[0].files.last().unwrap().dos_path,
        "GAME\\ZZZ\\LAST.DAT"
    );
}
