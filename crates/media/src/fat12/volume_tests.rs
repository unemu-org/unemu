// SPDX-License-Identifier: AGPL-3.0-or-later
//! Volumes from files in memory: the keys of a recipe's `fat` step. Images
//! are read back through the independent test reader.

use super::reader::Volume as Reader;
use super::*;
use std::path::PathBuf;

fn bytes(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|i| (i as u8).wrapping_mul(13).wrapping_add(seed))
        .collect()
}

fn time(text: &str) -> DosTime {
    text.parse().unwrap()
}

fn images(entries: Vec<Entry>, volume: &Volume) -> Vec<Vec<u8>> {
    plan_volumes(entries, volume).unwrap().images().unwrap()
}

fn word(image: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([image[at], image[at + 1]])
}

/// The root entries' names, a label included, in slot order.
fn root_names(image: &[u8], format: FloppyFormat) -> Vec<String> {
    (0..format.root_entries())
        .map(|slot| format.root_start() + slot * ENTRY)
        .take_while(|&at| image[at] != 0)
        .map(|at| String::from_utf8_lossy(&image[at..at + 11]).into_owned())
        .collect()
}

/// SHA-256 of an image, for the pins that freeze each addition's bytes.
fn sha256(image: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(image)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn error(entries: Vec<Entry>, volume: &Volume) -> BuildError {
    plan_volumes(entries, volume).unwrap_err()
}

#[test]
fn listed_order_keeps_the_given_order_of_entries_and_clusters() {
    let entries = || {
        vec![
            Entry::file("SETUP.EXE", bytes(700, 1)),
            Entry::file("DATA/ZED.DAT", bytes(600, 2)),
            Entry::file("DATA/ALPHA.DAT", bytes(100, 3)),
            Entry::file("AUTORUN.BAT", bytes(30, 4)),
        ]
    };
    let format = FloppyFormat::Hd144;
    let listed = Volume {
        order: Order::Listed,
        ..Volume::new(format)
    };
    let image = &images(entries(), &listed)[0];
    assert_eq!(
        root_names(image, format),
        ["SETUP   EXE", "DATA       ", "AUTORUN BAT"]
    );
    let reader = Reader::open(image).unwrap();
    reader.check().unwrap();
    let data = reader.list("DATA").unwrap();
    assert_eq!(
        data.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        [".", "..", "ZED.DAT", "ALPHA.DAT"]
    );
    // Clusters follow the listed tree in pre-order: SETUP.EXE takes 2 and 3,
    // DATA 4, ZED.DAT 5 and 6, ALPHA.DAT 7, AUTORUN.BAT 8.
    let first = |path: &str| {
        let (folder, name) = path.rsplit_once('\\').unwrap_or(("", path));
        reader
            .list(folder)
            .unwrap()
            .into_iter()
            .find(|e| e.name == name)
            .unwrap()
            .first_cluster
    };
    assert_eq!(
        [
            "SETUP.EXE",
            "DATA",
            "DATA\\ZED.DAT",
            "DATA\\ALPHA.DAT",
            "AUTORUN.BAT"
        ]
        .map(first),
        [2, 4, 5, 7, 8]
    );
    assert_eq!(reader.read("DATA\\ZED.DAT").unwrap(), bytes(600, 2));
    // MEASURED when listed order was added; the bytes a recipe freezes.
    assert_eq!(
        sha256(image),
        "e259b33b40016edecff8ee3b84dba778fb9c18fbc8a3e08a65c1eb83ebc5a605"
    );

    let named = &images(entries(), &Volume::new(format))[0];
    assert_eq!(
        root_names(named, format),
        ["AUTORUN BAT", "SETUP   EXE", "DATA       "],
        "name order: files first, then folders"
    );
    assert_ne!(image, named);
}

#[test]
fn name_order_gives_the_bytes_the_host_builder_gives() {
    let scratch = super::tests::Scratch::new("volume-host");
    // Host names in mixed case, a folder that sorts before the files, and
    // `a-b.txt` before `a.txt` by host bytes but after it as stored
    // (`A-B     TXT` against `A       TXT`).
    let files = [
        ("Game.exe", "GAME.EXE", bytes(3000, 2)),
        ("README.TXT", "README.TXT", bytes(700, 1)),
        ("a-b.txt", "A-B.TXT", bytes(40, 5)),
        ("a.txt", "A.TXT", bytes(50, 6)),
        ("AAA/level1.dat", "AAA/LEVEL1.DAT", bytes(5000, 3)),
        ("AAA/maps/m1.map", "AAA/MAPS/M1.MAP", bytes(10, 4)),
        ("AAA/empty.txt", "AAA/EMPTY.TXT", Vec::new()),
    ];
    for (host, _, contents) in &files {
        scratch.file(host, contents);
    }
    scratch.folder("docs");
    let label = Some(volume_label("SAME").unwrap());
    for format in FloppyFormat::ALL {
        let request = Request {
            sources: ["Game.exe", "README.TXT", "a-b.txt", "a.txt", "AAA", "docs"]
                .map(|name| scratch.path(name))
                .to_vec(),
            top: None,
            format,
            label,
            exclude: Vec::new(),
        };
        let (host, _) = build(&request).unwrap();
        // Given in another order, as stored: name order sorts them as the
        // host scan does.
        let mut entries: Vec<Entry> = files
            .iter()
            .rev()
            .map(|(_, stored, contents)| Entry::file(*stored, contents.clone()))
            .collect();
        entries.push(Entry::folder("DOCS"));
        let memory = images(
            entries,
            &Volume {
                label,
                ..Volume::new(format)
            },
        );
        assert_eq!(memory, std::slice::from_ref(&host), "{format}");
        assert_eq!(
            root_names(&host, format),
            [
                "SAME       ",
                "A       TXT",
                "A-B     TXT",
                "GAME    EXE",
                "README  TXT",
                "AAA        ",
                "DOCS       "
            ],
            "{format}"
        );
    }
}

#[test]
fn entry_times_fill_creation_access_and_write_fields() {
    let format = FloppyFormat::Dd720;
    let volume = Volume {
        time: time("2008-07-03T00:00:00"),
        order: Order::Listed,
        ..Volume::new(format)
    };
    let image = &images(
        vec![
            Entry::file("BOARDS.KZP", b"maze".to_vec()).at(time("1993-03-27T13:58:22")),
            Entry::file("GAME/KEN.EXE", b"MZ".to_vec()),
            Entry::folder("SAVES").at(time("2107-12-31T23:59:58")),
        ],
        &volume,
    )[0];
    // 1993-03-27 is (13 << 9) | (3 << 5) | 27 = 1A7Bh; 13:58:22 is
    // (13 << 11) | (58 << 5) | 11 = 6F4Bh. 2008-07-03 is 38E3h, midnight 0.
    // 2107-12-31 is FF9Fh; 23:59:58 is BF7Dh.
    let fields = |entry: &[u8]| [14, 16, 18, 22, 24].map(|at| word(entry, at));
    let root = format.root_start();
    let slot = |n: usize| &image[root + n * ENTRY..root + (n + 1) * ENTRY];
    assert_eq!(&slot(0)[..11], b"BOARDS  KZP");
    assert_eq!(fields(slot(0)), [0x6F4B, 0x1A7B, 0x1A7B, 0x6F4B, 0x1A7B]);
    assert_eq!(slot(0)[13], 0, "no tenths: the seconds are even");
    assert_eq!(&slot(1)[..11], b"GAME       ");
    assert_eq!(fields(slot(1)), [0, 0x38E3, 0x38E3, 0, 0x38E3]);
    assert_eq!(&slot(2)[..11], b"SAVES      ");
    assert_eq!(fields(slot(2)), [0xBF7D, 0xFF9F, 0xFF9F, 0xBF7D, 0xFF9F]);
    // MEASURED when entry times were added.
    assert_eq!(
        sha256(image),
        "16a9c1b9ec170e4d322710d4f8620c4a0c6e3bd65e3b4a44021f79f879933cae"
    );
    let reader = Reader::open(image).unwrap();
    reader.check().unwrap();
    for (folder, own) in [("GAME", 0x38E3), ("SAVES", 0xFF9F)] {
        for entry in reader.list(folder).unwrap() {
            // `.` and `..` take the folder's own time.
            let expected = if entry.name.starts_with('.') {
                own
            } else {
                0x38E3
            };
            assert_eq!(word(&entry.raw, 24), expected, "{folder} {}", entry.name);
        }
    }
}

#[test]
fn times_are_checked_not_rounded() {
    assert_eq!(DosTime::EPOCH, time("1980-01-01T00:00:00"));
    assert_eq!(
        time("2000-02-29T12:30:58").to_string(),
        "2000-02-29T12:30:58"
    );
    assert!(DosTime::new(2096, 2, 29, 0, 0, 0).is_ok());
    for refused in [
        "1979-12-31T23:59:58",
        "2108-01-01T00:00:00",
        "1993-03-27T13:58:23",
        "1981-02-29T00:00:00",
        "2100-02-29T00:00:00",
        "1993-04-31T00:00:00",
        "1993-13-01T00:00:00",
        "1993-03-27T24:00:00",
        "1993-03-27 13:58:22",
        "1993-03-27T13:58",
        "1993-03-27T13:58:22Z",
    ] {
        assert!(refused.parse::<DosTime>().is_err(), "{refused}");
    }
}

#[test]
fn serial_and_label_with_its_time_reach_the_boot_sector_and_the_root() {
    let format = FloppyFormat::Hd144;
    let volume = Volume {
        label: Some(volume_label("KENSLAB").unwrap()),
        serial: 0x2008_0703,
        time: time("2008-07-03T16:58:22"),
        ..Volume::new(format)
    };
    let image = &images(vec![Entry::file("KEN.EXE", b"MZ".to_vec())], &volume)[0];
    assert_eq!(&image[39..43], &[0x03, 0x07, 0x08, 0x20]);
    assert_eq!(&image[43..54], b"KENSLAB    ");
    let root = &image[format.root_start()..format.root_start() + ENTRY];
    assert_eq!(&root[..12], b"KENSLAB    \x08");
    // 2008-07-03 is 38E3h; 16:58:22 is (16 << 11) | (58 << 5) | 11 = 874Bh.
    for (at, expected) in [
        (14, 0x874B),
        (16, 0x38E3),
        (18, 0x38E3),
        (22, 0x874B),
        (24, 0x38E3),
    ] {
        assert_eq!(word(root, at), expected, "offset {at}");
    }
    assert_eq!(&root[26..32], &[0; 6], "a label has no cluster or length");
    let reader = Reader::open(image).unwrap();
    reader.check().unwrap();
    assert_eq!(reader.label().as_deref(), Some("KENSLAB"));
    // MEASURED when the chosen serial and the label's time were added.
    assert_eq!(
        sha256(image),
        "09fb019e0f2d31cc28ae95043eedb122da076e936fd660fd9627cf89524435f8"
    );

    // Without a choice: the fixed serial, `NO NAME` and no label entry.
    let plain = &images(
        vec![Entry::file("KEN.EXE", b"MZ".to_vec())],
        &Volume::new(format),
    )[0];
    assert_eq!(&plain[39..43], &DEFAULT_SERIAL.to_le_bytes());
    assert_eq!(&plain[43..54], b"NO NAME    ");
    assert_eq!(root_names(plain, format), ["KEN     EXE"]);
}

#[test]
fn bad_names_are_refused_with_the_rule_never_renamed() {
    let volume = Volume::new(FloppyFormat::Hd144);
    for (path, part, fault) in [
        ("LONGFILENAME.TXT", "LONGFILENAME.TXT", NameFault::Stem(12)),
        ("GAME/README.TEXT", "README.TEXT", NameFault::Extension(4)),
        ("A B.TXT", "A B.TXT", NameFault::Character(' ')),
        (
            "DATA/\u{C4}.TXT",
            "\u{C4}.TXT",
            NameFault::Character('\u{C4}'),
        ),
        ("WILD*.TXT", "WILD*.TXT", NameFault::Character('*')),
        ("GAME\\KEN.EXE", "GAME\\KEN.EXE", NameFault::Character('\\')),
        ("A.B.C", "A.B.C", NameFault::Periods),
        (".PROFILE", ".PROFILE", NameFault::Periods),
        ("NOTES.", "NOTES.", NameFault::Periods),
        ("..", "..", NameFault::Periods),
        ("GAME//KEN.EXE", "", NameFault::Empty),
        ("/KEN.EXE", "", NameFault::Empty),
        ("CON.TXT", "CON.TXT", NameFault::Device),
        ("LPT1", "LPT1", NameFault::Device),
    ] {
        match error(vec![Entry::file(path, b"x".to_vec())], &volume) {
            BuildError::BadName {
                path: named,
                name,
                fault: found,
            } => assert_eq!((named.as_str(), name.as_str(), found), (path, part, fault)),
            other => panic!("{path}: {other}"),
        }
    }
    assert_eq!(
        error(
            vec![Entry::file("DATA/LONGFILENAME.TXT", b"x".to_vec())],
            &volume
        )
        .to_string(),
        "DATA/LONGFILENAME.TXT: \"LONGFILENAME.TXT\" is not a DOS 8.3 name: the name has 12 \
         characters before the extension; at most 8 are allowed; names are refused, never renamed"
    );
    assert_eq!(
        error(vec![Entry::file("A B.TXT", b"x".to_vec())], &volume).to_string(),
        "A B.TXT: \"A B.TXT\" is not a DOS 8.3 name: the character ' ' is not allowed; names \
         are refused, never renamed"
    );

    // A recipe states the name as it is on the disk: lower case is refused
    // (host files, by contrast, are upper-cased by the host scan).
    for (path, part, letter) in [
        ("game/KEN.EXE", "game", 'g'),
        ("GAME/Ken.exe", "Ken.exe", 'e'),
    ] {
        match error(vec![Entry::file(path, b"MZ".to_vec())], &volume) {
            BuildError::BadName { name, fault, .. } => {
                assert_eq!((name.as_str(), fault), (part, NameFault::LowerCase(letter)))
            }
            other => panic!("{path}: {other}"),
        }
    }
    assert_eq!(
        error(vec![Entry::file("readme.txt", b"x".to_vec())], &volume).to_string(),
        "readme.txt: \"readme.txt\" is not a DOS 8.3 name: the lower-case letter 'r' is not \
         allowed: a directory entry stores upper case, so write the name as it is on the disk; \
         names are refused, never renamed"
    );
    let image = &images(vec![Entry::file("GAME/KEN.EXE", b"MZ".to_vec())], &volume)[0];
    assert_eq!(
        Reader::open(image).unwrap().read("GAME\\KEN.EXE").unwrap(),
        b"MZ"
    );

    for entries in [
        vec![
            Entry::file("KEN.EXE", b"1".to_vec()),
            Entry::file("KEN.EXE", b"2".to_vec()),
        ],
        vec![
            Entry::file("GAME", b"1".to_vec()),
            Entry::file("GAME/KEN.EXE", b"2".to_vec()),
        ],
        vec![
            Entry::file("GAME/KEN.EXE", b"1".to_vec()),
            Entry::file("GAME", b"2".to_vec()),
        ],
        vec![Entry::folder("GAME"), Entry::folder("GAME")],
        vec![Entry::file("GAME", b"1".to_vec()), Entry::folder("GAME")],
    ] {
        let path = entries[1].path.clone();
        let refused = error(entries, &volume);
        assert!(
            matches!(&refused, BuildError::NameClash { path: p } if *p == path),
            "{refused}"
        );
        assert!(refused
            .to_string()
            .contains("named twice, or as both a file and a folder"));
    }
    // A folder seen above a file may still be listed once, for its time.
    assert!(plan_volumes(
        vec![
            Entry::file("GAME/KEN.EXE", b"1".to_vec()),
            Entry::folder("GAME")
        ],
        &volume
    )
    .is_ok());
}

#[test]
fn folders_deeper_than_dos_allows_are_refused() {
    let volume = Volume::new(FloppyFormat::Hd144);
    // DOS keeps a current directory in 64 characters counting the leading
    // backslash: six folders of eight characters and one of nine
    // (`ABCDEF.AB`), with six separators, make exactly 64; one character
    // more makes 65.
    fn levels(last: &str) -> Vec<&str> {
        let mut parts = vec!["ABCDEFGH"; 6];
        parts.push(last);
        parts
    }
    let fits = levels("ABCDEF.AB").join("/");
    assert_eq!(1 + fits.len(), 64);
    assert!(plan_volumes(
        vec![Entry::file(format!("{fits}/F.TXT"), b"x".to_vec())],
        &volume
    )
    .is_ok());
    let deep = levels("ABCDEFG.AB").join("/");
    match error(
        vec![Entry::file(format!("{deep}/F.TXT"), b"x".to_vec())],
        &volume,
    ) {
        BuildError::PathTooLong {
            host,
            length,
            dos_path,
        } => {
            assert_eq!(host, PathBuf::from(&deep));
            assert_eq!(length, 65);
            assert_eq!(dos_path, format!("\\{}", levels("ABCDEFG.AB").join("\\")));
        }
        other => panic!("{other}"),
    }
}

/// Three files of 150 KiB: a 360K volume holds two of them.
fn three_parts() -> Vec<Entry> {
    (1..=3)
        .map(|n| Entry::file(format!("PART{n}.DAT"), bytes(150 * 1024, n as u8)))
        .collect()
}

#[test]
fn a_split_fills_exactly_the_volumes_asked_for_or_names_the_number() {
    let format = FloppyFormat::Dd360;
    let split = |volumes| Volume {
        label: Some(volume_label("PARTS").unwrap()),
        volumes: Some(volumes),
        ..Volume::new(format)
    };
    let plan = plan_volumes(three_parts(), &split(2)).unwrap();
    assert_eq!(
        plan.disks.iter().map(|d| d.files.len()).collect::<Vec<_>>(),
        [2, 1]
    );
    let images = plan.images().unwrap();
    for (index, image) in images.iter().enumerate() {
        let reader = Reader::open(image).unwrap();
        reader.check().unwrap();
        assert_eq!(reader.label(), Some(format!("PARTS {}", index + 1)));
    }
    assert_eq!(
        Reader::open(&images[1]).unwrap().read("PART3.DAT").unwrap(),
        bytes(150 * 1024, 3)
    );
    // MEASURED when the exact split was added: each volume.
    assert_eq!(
        images.iter().map(|image| sha256(image)).collect::<Vec<_>>(),
        [
            "f8a0037aadc73a582e9b3619921960968a365bbc09a6a4ebdde01155323008ad",
            "31892b2e999121eaed6c392eaf06c0b05be7749fe8854a393acc655e8397f534"
        ]
    );

    for requested in [3, 5] {
        let refused = error(three_parts(), &split(requested));
        assert!(
            matches!(refused, BuildError::WrongVolumes { requested: r, needed: 2, .. } if r == requested),
            "{refused}"
        );
        assert_eq!(
            refused.to_string(),
            format!("the files need 2 360K volumes in folder order, not {requested}; ask for 2")
        );
    }
    assert_eq!(
        error(vec![Entry::file("SMALL.TXT", b"x".to_vec())], &split(2)).to_string(),
        "the files fit on one 360K volume, not 2; build one volume instead of a split"
    );
    for requested in [0, 1] {
        assert!(matches!(
            error(three_parts(), &split(requested)),
            BuildError::VolumeCount { requested: r } if r == requested
        ));
    }
    // Without a split the misfit is reported as for host files.
    assert!(matches!(
        error(three_parts(), &Volume::new(format)),
        BuildError::DoesNotFit { .. }
    ));
}

#[test]
fn the_same_entries_build_the_same_bytes() {
    let volume = Volume {
        label: Some(volume_label("TWICE").unwrap()),
        serial: 0x1234_5678,
        order: Order::Listed,
        time: time("1999-11-16T15:09:04"),
        volumes: Some(2),
        ..Volume::new(FloppyFormat::Dd360)
    };
    assert_eq!(
        images(three_parts(), &volume),
        images(three_parts(), &volume)
    );
    let single = Volume {
        volumes: None,
        ..volume
    };
    let entries = || {
        vec![
            Entry::file("B.TXT", b"b".to_vec()),
            Entry::file("A/C.TXT", b"c".to_vec()),
        ]
    };
    assert_eq!(images(entries(), &single), images(entries(), &single));
}

#[test]
fn an_empty_tree_is_a_formatted_volume() {
    let format = FloppyFormat::Hd12;
    let label = Some(volume_label("BLANK").unwrap());
    let image = images(
        Vec::new(),
        &Volume {
            label,
            ..Volume::new(format)
        },
    );
    let mut blank = format_metadata(format, label);
    blank.resize(format.image_bytes() as usize, 0);
    assert_eq!(image, [blank]);
}

/// The volume path a recipe's `fat` step runs ([`plan_volumes`], then
/// `Plan::images`) names no clock, network, environment, process or thread,
/// and its own files read no host file. `tree.rs` and `layout.rs` also hold
/// the host-file builder of `media create`, which reads folders and writes
/// images; the volume path builds its tree from entries in memory and lays
/// the images out in memory, so those two are checked for the clock and the
/// rest only.
#[test]
fn the_volume_path_reaches_no_clock_network_or_host() {
    let sources = [
        ("volume.rs", include_str!("volume.rs"), true),
        ("split.rs", include_str!("split.rs"), true),
        ("names.rs", include_str!("names.rs"), true),
        ("time.rs", include_str!("time.rs"), true),
        ("boot.rs", include_str!("boot.rs"), true),
        ("tree.rs", include_str!("tree.rs"), false),
        ("layout.rs", include_str!("layout.rs"), false),
    ];
    for (name, source, no_files) in sources {
        let code = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "std::time",
            "Instant",
            "SystemTime",
            "std::net",
            "std::env",
            "std::process",
            "std::thread",
        ] {
            assert!(!code.contains(forbidden), "{name} names {forbidden}");
        }
        if no_files {
            for forbidden in ["std::fs", "fs::"] {
                assert!(!code.contains(forbidden), "{name} names {forbidden}");
            }
        }
    }
}
