// SPDX-License-Identifier: AGPL-3.0-or-later
//! The launch document over content in memory: its encoding, its tables,
//! and what `check` and `lower` open and refuse.

use super::*;
use std::cell::RefCell;
use unemu_core_api::{Content, ErrorKind};

use crate::fixtures;

/// Content in memory by SHA-256, with each open counted.
struct Counting {
    blobs: BTreeMap<[u8; 32], Vec<u8>>,
    opened: RefCell<Vec<([u8; 32], u64)>>,
}
impl Counting {
    fn new<'a>(blobs: impl IntoIterator<Item = &'a [u8]>) -> Self {
        Self {
            blobs: blobs
                .into_iter()
                .map(|bytes| (Digest::of(bytes).sha256(), bytes.to_vec()))
                .collect(),
            opened: RefCell::default(),
        }
    }
    fn opened(&self) -> BTreeSet<([u8; 32], u64)> {
        self.opened.borrow().iter().copied().collect()
    }
}
impl ContentSource for Counting {
    fn open(
        &self,
        sha256: &[u8; 32],
        size: u64,
    ) -> std::result::Result<Content, unemu_core_api::Error> {
        self.opened.borrow_mut().push((*sha256, size));
        self.blobs
            .get(sha256)
            .filter(|bytes| bytes.len() as u64 == size)
            .map(|bytes| Content::Bytes(bytes.clone()))
            .ok_or_else(|| unemu_core_api::Error::new(ErrorKind::Content, "not in memory"))
    }
}

/// An AT on the open Kiselev BIOS with a journalled 360K floppy `a`, a
/// read-only 360K floppy `b` in a catalogue of the second drive, and a CMOS
/// image in `cmos_mode`; its blobs (BIOS, `a`, `b`, CMOS). `None` when the
/// BIOS fixture is missing.
fn fixture(cmos_mode: &str) -> Option<(Launch, [Vec<u8>; 4])> {
    let bios = fixtures::fixture("kiselev-bios/bios-xi8088.bin")?;
    let a = vec![0xF6; 368_640];
    let b = vec![0xE5; 368_640];
    let cmos = vec![0; 64];
    let [db, da, dd, dc] = [&bios, &a, &b, &cmos].map(|bytes| Digest::of(bytes));
    let text = format!(
        r#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[machine.board.rtc.nvram]
image = "assets/cmos.bin"
[machine.firmware.bios]
image = "assets/bios.bin"
sha256 = "{bios}"
[[machine.card]]
kind = "fdc-765"
[[machine.card.drives]]
unit = 0
type = "360K"
[[machine.card.drives]]
unit = 1
type = "360K"
[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "{db}"
[[media]]
id = "a"
source = "assets/a.img"
head = "{da}"
policy = "journal"
geometry = {{ cylinders = 40, heads = 2, sectors = 9, sector_bytes = 512 }}
[[media]]
id = "b"
source = "assets/b.img"
head = "{dd}"
name = "Disk B"
policy = "readonly"
geometry = {{ cylinders = 40, heads = 2, sectors = 9, sector_bytes = 512 }}
[[slots]]
controller = "fdc-765"
unit = 0
catalogue = ["a"]
catalogue_mode = false
inserted = "a"
[[slots]]
controller = "fdc-765"
unit = 1
catalogue = ["b"]
catalogue_mode = true
inserted = "b"
[nvram]
mode = "{cmos_mode}"
source = "assets/cmos.bin"
head = "{dc}"
"#,
        bios = db.hex(),
    );
    let config = Config::decode(text.parse().unwrap()).unwrap();
    let blobs = [bios, a, b, cmos];
    let content = blobs
        .iter()
        .map(|bytes| ContentRow {
            digest: Digest::of(bytes),
            sha256: Digest::of(bytes).sha256(),
            size: bytes.len() as u64,
        })
        .collect();
    let mut records = vec![RecordRow {
        medium: "a".into(),
        name: "a.upfj".into(),
        keep_generations: 3,
    }];
    if cmos_mode == "journal" {
        records.push(RecordRow {
            medium: "cmos".into(),
            name: "cmos.upfj".into(),
            keep_generations: 7,
        });
    }
    Some((
        Launch {
            config,
            content,
            records,
        },
        blobs,
    ))
}

fn source(blobs: &[Vec<u8>; 4]) -> Counting {
    Counting::new(blobs.iter().map(Vec::as_slice))
}

fn flat(bytes: &[u8]) -> ([u8; 32], u64) {
    (Digest::of(bytes).sha256(), bytes.len() as u64)
}

/// A launch document with invented digests: its encoding is pinned, and
/// what `decode` reads back encodes to the same bytes.
#[test]
fn a_document_round_trips_byte_for_byte_and_its_encoding_is_pinned() {
    let digest = |n: u8| Digest::from_sha256([n; 32]);
    let text = format!(
        r#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "{}"
[[media]]
id = "c"
source = "assets/c.img"
head = "{}"
policy = "journal"
geometry = {{ cylinders = 20, heads = 2, sectors = 17, sector_bytes = 512 }}
[[slots]]
controller = "ide"
unit = 0
catalogue = ["c"]
catalogue_mode = false
inserted = "c"
"#,
        digest(1),
        digest(2),
    );
    let launch = Launch {
        config: Config::decode(text.parse().unwrap()).unwrap(),
        content: vec![
            ContentRow {
                digest: digest(1),
                sha256: [1; 32],
                size: 65_536,
            },
            ContentRow {
                digest: digest(2),
                sha256: [0xab; 32],
                size: 348_160,
            },
        ],
        records: vec![RecordRow {
            medium: "c".into(),
            name: "c.upfj".into(),
            keep_generations: 10,
        }],
    };
    let bytes = encode(&launch).unwrap();
    let ones = "01".repeat(32);
    let abs = "ab".repeat(32);
    let expected = format!(
        r#"schema = 1
platform = "ibm-pc"

[machine]
schema = 1

[machine.board]
model = "ibm-5170"

[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "sha256:{ones}"

[[media]]
id = "c"
source = "assets/c.img"
head = "sha256:{abs2}"
policy = "journal"

[media.geometry]
cylinders = 20
heads = 2
sectors = 17
sector_bytes = 512

[[slots]]
controller = "ide"
unit = 0
catalogue = ["c"]
catalogue_mode = false
inserted = "c"

[[content]]
digest = "sha256:{ones}"
sha256 = "{ones}"
size = 65536

[[content]]
digest = "sha256:{abs2}"
sha256 = "{abs}"
size = 348160

[[record]]
medium = "c"
name = "c.upfj"
keep_generations = 10
"#,
        abs2 = "02".repeat(32),
    );
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(encode(&decoded).unwrap(), bytes);
    assert_eq!(decoded.content, launch.content);
    assert_eq!(decoded.records, launch.records);
}

/// `decode` reads the form alone, and refuses a document too long to be
/// one, a malformed SHA-256 and a field the format does not have.
#[test]
fn decode_refuses_what_is_not_a_launch_document() {
    let refusal = |bytes: &[u8]| decode(bytes).unwrap_err().to_string();
    assert_eq!(
        refusal(&vec![b' '; MAX_DOCUMENT_BYTES + 1]),
        "invalid PC image: launch document exceeds size limit of 2097152 bytes"
    );
    let base = "schema = 1\nplatform = \"ibm-pc\"\n[machine]\nschema = 1\n";
    let mut longest = base.as_bytes().to_vec();
    longest.resize(MAX_DOCUMENT_BYTES, b' ');
    assert!(decode(&longest).is_ok());
    let mut oversized = decode(base.as_bytes()).unwrap();
    oversized
        .config
        .machine
        .insert("description".into(), "x".repeat(MAX_DOCUMENT_BYTES).into());
    assert_eq!(
        encode(&oversized).unwrap_err().to_string(),
        "invalid PC image: launch document exceeds size limit of 2097152 bytes"
    );
    let row = |sha256: &str| {
        format!(
            "{base}[[content]]\ndigest = \"sha256:{}\"\nsha256 = \"{sha256}\"\nsize = 1\n",
            "00".repeat(32)
        )
    };
    assert!(decode(row(&"00".repeat(32)).as_bytes()).is_ok());
    for bad in [
        "00".repeat(31),
        "AB".repeat(32),
        format!("sha256:{}", "00".repeat(32)),
    ] {
        assert_eq!(
            refusal(row(&bad).as_bytes()),
            "invalid PC image: content sha256 must be 64 lowercase hex digits"
        );
    }
    assert!(refusal(format!("{base}[[extra]]\n").as_bytes()).contains("unknown field `extra`"));
}

/// `check` lists the content a machine of the document opens: what `lower`
/// reads (the ROM and the CMOS image) and what it names for the machine to
/// open (the floppy bases, and the CMOS image again for its journal), and
/// nothing else. `lower` never opens a floppy.
#[test]
fn check_lists_exactly_what_lower_opens() {
    for mode in ["journal", "image"] {
        let Some((launch, blobs)) = fixture(mode) else {
            return;
        };
        let [bios, a, b, cmos] = &blobs;
        let content = source(&blobs);
        let listed = check(&launch, assembly::AssemblyOptions::default()).unwrap();
        let lowered = lower(&launch, &content, assembly::AssemblyOptions::default()).unwrap();
        assert_eq!(content.opened(), BTreeSet::from([flat(bios), flat(cmos)]));
        let named: BTreeSet<_> = lowered.bases.values().copied().collect();
        let mut expected = BTreeSet::from([flat(a), flat(b)]);
        if mode == "journal" {
            expected.insert(flat(cmos));
        }
        assert_eq!(named, expected, "{mode}");
        let opens: BTreeSet<_> = content.opened().union(&named).copied().collect();
        let listed_set: BTreeSet<_> = listed.iter().map(|c| (c.sha256, c.size)).collect();
        assert_eq!(listed_set, opens, "{mode}");
        assert_eq!(listed.len(), 4);
        assert_eq!(
            listed.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            ["firmware", "floppy", "floppy", "CMOS"]
        );
    }
}

/// Distinct heads can name identical flat bytes. The contract lists that
/// content once, while both logical bases still have their own keys.
#[test]
fn check_deduplicates_flat_content_without_losing_base_keys() {
    let Some((mut launch, blobs)) = fixture("image") else {
        return;
    };
    let shared = launch.content[1].clone();
    launch.content[2].sha256 = shared.sha256;
    launch.content[2].size = shared.size;
    let listed = check(&launch, assembly::AssemblyOptions::default()).unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(
        listed.iter().filter(|r| r.sha256 == shared.sha256).count(),
        1
    );
    let content = source(&blobs);
    let lowered = lower(&launch, &content, assembly::AssemblyOptions::default()).unwrap();
    assert_eq!(lowered.bases.len(), 2);
    assert_eq!(
        lowered.bases[&PathBuf::from("assets/a.img")],
        flat(&blobs[1])
    );
    assert_eq!(
        lowered.bases[&PathBuf::from("assets/b.img")],
        flat(&blobs[1])
    );
    assert_eq!(
        content.opened(),
        BTreeSet::from([flat(&blobs[0]), flat(&blobs[3])])
    );
}

/// A disk row can reach the largest ATA CHS capacity without allocating or
/// opening its bytes. Increasing both the row and its matching geometry
/// cannot evade assembly's drive limit.
#[test]
fn disk_rows_are_bounded_by_geometry_without_reading_disk_content() {
    let Some((mut launch, mut blobs)) = fixture("image") else {
        return;
    };
    launch.config.media.truncate(1);
    launch.config.slots.truncate(1);
    launch.config.slots[0].controller = "ide".into();
    launch.content.remove(2);
    // Geometry validation needs a BIOS that declares the fixed-disk service;
    // the floppy fixture's BIOS does not. No guest instructions run here.
    blobs[0].fill(0xff);
    let bios = Digest::of(&blobs[0]);
    launch.config.firmware[0].blob = bios;
    launch.content[0].digest = bios;
    launch.content[0].sha256 = bios.sha256();
    launch.config.machine["firmware"]["bios"]
        .as_table_mut()
        .unwrap()
        .insert("sha256".into(), bios.hex().into());
    launch.config.machine["firmware"]["bios"]
        .as_table_mut()
        .unwrap()
        .insert("provides".into(), vec!["floppy", "fixed-disk"].into());
    let set_geometry = |launch: &mut Launch, cylinders: u32| {
        launch.config.media[0].geometry = image::Geometry {
            cylinders,
            heads: 16,
            sectors: 63,
            sector_bytes: 512,
        };
        launch.content[1].size = u64::from(cylinders) * 16 * 63 * 512;
        let hardware: toml::Table = format!(
            "[[card]]\nkind = \"ide\"\n[[card.drives]]\nchannel = \"master\"\ngeometry = {{ cylinders = {cylinders}, heads = 16, sectors = 63 }}\n"
        )
        .parse()
        .unwrap();
        launch
            .config
            .machine
            .insert("card".into(), hardware["card"].clone());
    };
    set_geometry(&mut launch, 16_383);
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    let listed = check(&launch, options).unwrap();
    assert_eq!(listed[1].size, 8_455_200_768);
    let content = source(&blobs);
    let lowered = lower(&launch, &content, options).unwrap();
    assert_eq!(
        lowered.bases[&PathBuf::from("assets/a.img")].1,
        listed[1].size
    );
    assert_eq!(
        content.opened(),
        BTreeSet::from([flat(&blobs[0]), flat(&blobs[3])])
    );

    set_geometry(&mut launch, 16_384);
    let checked = check(&launch, options).unwrap_err().to_string();
    let content = source(&blobs);
    let lowered = lower(&launch, &content, options).unwrap_err().to_string();
    assert_eq!(checked, lowered);
    assert!(checked.contains("invalid ATA geometry"), "{checked}");
    assert!(!content
        .opened()
        .contains(&(launch.content[1].sha256, launch.content[1].size)));

    launch.content[1].size = u64::MAX;
    let content = source(&blobs);
    assert_eq!(
        check(&launch, options).unwrap_err().to_string(),
        "invalid PC image: medium geometry differs from payload length"
    );
    assert_eq!(
        lower(&launch, &content, options).unwrap_err().to_string(),
        "invalid PC image: medium geometry differs from payload length"
    );
    assert!(content.opened().is_empty());
}

/// Every key the machine names is in `bases` or `records`; each journal
/// keeps what its record row says; a journalled CMOS is an NVRAM journal
/// over its image. The machine is the codec's otherwise.
#[test]
fn lower_names_every_key_and_keeps_each_record_retention() {
    let Some((mut launch, blobs)) = fixture("journal") else {
        return;
    };
    launch.records[0].name = "kept-a-floppy".into();
    let lowered = lower(
        &launch,
        &source(&blobs),
        assembly::AssemblyOptions::default(),
    )
    .unwrap();
    let machine = &lowered.machine;
    let [_, a, b, cmos] = &blobs;
    assert_eq!(
        lowered.bases,
        BTreeMap::from([
            ("assets/a.img".into(), flat(a)),
            ("assets/b.img".into(), flat(b)),
            ("assets/cmos.bin".into(), flat(cmos)),
        ])
    );
    assert_eq!(
        lowered.records,
        BTreeMap::from([
            ("a.upfj".into(), "kept-a-floppy".to_string()),
            ("cmos.upfj".into(), "cmos.upfj".to_string()),
        ])
    );
    let drive = &machine.drives[0];
    assert_eq!(
        drive.image,
        assembly::ImageSource::File("assets/a.img".into())
    );
    assert_eq!(
        drive.write,
        assembly::WriteMode::Journal {
            path: "a.upfj".into(),
            keep_generations: 3
        }
    );
    let list = machine.drives[1].media.as_ref().unwrap();
    assert_eq!(
        list.entries[0].image,
        assembly::ImageSource::File("assets/b.img".into())
    );
    let nvram = machine
        .devices
        .iter()
        .find_map(|d| match &d.kind {
            assembly::DeviceKind::Rtc(rtc) => Some(rtc.nvram.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        nvram,
        assembly::NvramSource::Journal {
            image: "assets/cmos.bin".into(),
            expected_sha256: flat(cmos).0,
            journal: "cmos.upfj".into(),
            keep_generations: 7,
        }
    );

    // The codec's machine, from the stored config alone, is the same but for
    // the retention it does not know and the CMOS it copies.
    let rows = launch
        .content
        .iter()
        .map(|row| {
            let flat = image::ContentRow {
                sha256: row.sha256,
                size: row.size,
            };
            (row.digest, flat)
        })
        .collect();
    let mut codec = image::lower(
        launch.config.clone(),
        &rows,
        &source(&blobs),
        true,
        assembly::AssemblyOptions::default(),
    )
    .unwrap()
    .machine;
    codec.drives[0].write = drive.write.clone();
    for device in &mut codec.devices {
        if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
            assert_eq!(
                rtc.nvram,
                assembly::NvramSource::Image(cmos.as_slice().try_into().unwrap())
            );
            rtc.nvram = nvram.clone();
        }
    }
    assert_eq!(&codec, machine);
}

/// A document with a missing, doubled or unused row of either table, or a
/// row that breaks a rule of its table, is refused by `check` and by `lower`
/// with the same words, before any content is opened.
#[test]
fn check_and_lower_refuse_bad_tables_alike_before_opening_anything() {
    let Some((launch, blobs)) = fixture("journal") else {
        return;
    };
    let [bios, a, _, cmos] = &blobs;
    let (bios, a) = (Digest::of(bios), Digest::of(a));
    let cmos = Digest::of(cmos);
    let edit = |change: &dyn Fn(&mut Launch)| {
        let mut launch = launch.clone();
        change(&mut launch);
        launch
    };
    let row = |launch: &Launch, digest: Digest| {
        launch
            .content
            .iter()
            .position(|row| row.digest == digest)
            .unwrap()
    };
    let cases: Vec<(Launch, String)> = vec![
        (
            edit(&|l| {
                let i = row(l, bios);
                l.content.remove(i);
            }),
            format!("missing content row for {bios}"),
        ),
        (
            edit(&|l| {
                let i = row(l, a);
                l.content.remove(i);
            }),
            format!("missing content row for {a}"),
        ),
        (
            edit(&|l| {
                let i = row(l, cmos);
                l.content.remove(i);
            }),
            format!("missing content row for {cmos}"),
        ),
        (
            edit(&|l| {
                let i = row(l, a);
                l.content.push(l.content[i].clone());
            }),
            format!("doubled content row for {a}"),
        ),
        (
            edit(&|l| {
                l.content.push(ContentRow {
                    digest: Digest::from_sha256([9; 32]),
                    sha256: [9; 32],
                    size: 512,
                });
            }),
            format!("unused content row for {}", Digest::from_sha256([9; 32])),
        ),
        (
            edit(&|l| {
                let i = row(l, bios);
                l.content[i].size = MAX_FIRMWARE_BYTES + 1;
            }),
            "payload exceeds PC size limit".into(),
        ),
        (
            edit(&|l| {
                let i = row(l, cmos);
                l.content[i].size = 128;
            }),
            "CMOS must contain exactly 64 bytes".into(),
        ),
        (
            edit(&|l| {
                l.records.remove(0);
            }),
            "missing record row for a".into(),
        ),
        (
            edit(&|l| {
                l.records.pop();
            }),
            "missing record row for cmos".into(),
        ),
        (
            edit(&|l| {
                let first = l.records[0].clone();
                l.records.push(first);
            }),
            "doubled record row for a".into(),
        ),
        (
            edit(&|l| {
                l.records.push(RecordRow {
                    medium: "b".into(),
                    name: "b.upfj".into(),
                    keep_generations: 1,
                });
            }),
            "unused record row for b".into(),
        ),
        (
            edit(&|l| {
                l.records[0].name = "c.upfj".into();
            }),
            r#"the journal of a cannot be the record "c.upfj": it is a.upfj or a kept record name"#
                .into(),
        ),
        (
            edit(&|l| {
                l.records[0].name = "kept-".into();
            }),
            r#"the journal of a cannot be the record "kept-": it is a.upfj or a kept record name"#
                .into(),
        ),
        (
            edit(&|l| {
                l.records[0].name = "kept-one".into();
                l.records[1].name = "kept-one".into();
            }),
            "record kept-one holds more than one journal".into(),
        ),
        (
            edit(&|l| {
                l.records[1].keep_generations = 0;
            }),
            "the journal of cmos must keep at least one generation".into(),
        ),
        (
            edit(&|l| {
                l.config.platform = "ibm-pc-at".into();
            }),
            "unsupported PC config envelope".into(),
        ),
        (
            edit(&|l| {
                l.config.slots[1].catalogue = vec!["z".into()];
                l.config.slots[1].inserted = Some("z".into());
            }),
            "unknown catalogue medium".into(),
        ),
    ];
    for (launch, words) in cases {
        let words = format!("invalid PC image: {words}");
        let checked = check(&launch, assembly::AssemblyOptions::default())
            .unwrap_err()
            .to_string();
        let content = source(&blobs);
        let lowered = lower(&launch, &content, assembly::AssemblyOptions::default())
            .unwrap_err()
            .to_string();
        assert_eq!(checked, words);
        assert_eq!(lowered, words);
        assert!(content.opened().is_empty(), "{words}");
    }
}

/// What the lowering refuses in the hardware tables, `check` refuses with
/// the same words, from the metadata mode and `MachineConfig::validate`.
#[test]
fn check_and_lower_refuse_bad_hardware_alike() {
    let Some((launch, blobs)) = fixture("image") else {
        return;
    };
    let with = |table: &str, key: &str, value: toml::Value| {
        let mut launch = launch.clone();
        let machine = &mut launch.config.machine;
        machine
            .entry(table)
            .or_insert_with(|| toml::Table::new().into())
            .as_table_mut()
            .unwrap()
            .insert(key.into(), value);
        launch
    };
    let mut no_bios = launch.clone();
    no_bios.config.machine.remove("firmware");
    no_bios.config.firmware.clear();
    no_bios.content.remove(0);
    let cases = [
        with("memory", "conventional_kib", 650.into()),
        with("memory", "extended_kib", 100.into()),
        with("cpu", "model", "8086".into()),
        no_bios,
    ];
    for launch in cases {
        let checked = check(&launch, assembly::AssemblyOptions::default())
            .unwrap_err()
            .to_string();
        let lowered = lower(
            &launch,
            &source(&blobs),
            assembly::AssemblyOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(checked, lowered);
    }
}

#[test]
fn decode_refuses_unknown_versions() {
    for text in [
        "schema = 2\nplatform = \"ibm-pc\"\n[machine]\nschema = 1\n",
        "schema = 1\nplatform = \"ibm-pc\"\n[machine]\nschema = 2\n",
    ] {
        assert!(decode(text.as_bytes()).is_err(), "{text}");
    }
}

#[test]
fn decode_refuses_unknown_fields_in_owned_tables() {
    let base = "schema = 1\nplatform = \"ibm-pc\"\n[machine]\nschema = 1\n";
    let digest = "00".repeat(32);
    for text in [
        format!("extra = true\n{base}"),
        format!("{base}[[content]]\ndigest = \"sha256:{digest}\"\nsha256 = \"{digest}\"\nsize = 1\nextra = true\n"),
        format!("{base}[[record]]\nmedium = \"cmos\"\nname = \"cmos.upfj\"\nkeep_generations = 1\nextra = true\n"),
    ] {
        let error = decode(text.as_bytes()).unwrap_err().to_string();
        assert!(error.contains("unknown field `extra`"), "{error}");
    }
}

#[test]
fn decode_uses_the_stored_config_hardware_rules() {
    for hardware in [
        "schema = 1\nextra = true\n",
        "schema = 1\n[board]\nmodel = \"ibm-5170\"\nextra = true\n",
        "schema = 1\n[cpu]\nextra = true\n",
        "schema = 1\n[firmware.bios]\nimage = \"assets/bios.bin\"\nextra = true\n",
    ] {
        let text = format!(
            "schema = 1\nplatform = \"ibm-pc\"\n[machine]\n{}",
            hardware.replace("[", "[machine.")
        );
        let error = decode(text.as_bytes()).unwrap_err().to_string();
        let stored = Config::decode(text.parse().unwrap())
            .unwrap_err()
            .to_string();
        assert_eq!(error, stored);
        assert!(error.contains("unknown field `extra`"), "{error}");
    }
}

fn overlapping_option_roms_fixture() -> Option<(Launch, Counting)> {
    let (mut launch, blobs) = fixture("image")?;
    let mut rom = vec![0; 8192];
    rom[..3].copy_from_slice(&[0x55, 0xaa, 16]);
    rom[8191] = 0xf1;
    let digest = Digest::of(&rom);
    let mut roms = Vec::new();
    for index in 0..2 {
        let name = format!("assets/option{index}.bin");
        roms.push(toml::Value::Table(toml::Table::from_iter([
            ("image".into(), name.clone().into()),
            ("sha256".into(), digest.hex().into()),
            ("base".into(), 0xC8000.into()),
        ])));
        launch.config.firmware.push(Firmware {
            key: format!("firmware.option_roms[{index}]"),
            source: name,
            blob: digest,
            source_archive: None,
        });
    }
    launch.config.machine["firmware"]
        .as_table_mut()
        .unwrap()
        .insert("option_roms".into(), roms.into());
    launch.content.push(ContentRow {
        digest,
        sha256: digest.sha256(),
        size: rom.len() as u64,
    });
    let content = Counting::new(blobs.iter().map(Vec::as_slice).chain([rom.as_slice()]));
    Some((launch, content))
}

#[test]
fn check_and_lower_refuse_invalid_rtc_before_overlapping_option_roms() {
    let Some((mut launch, content)) = overlapping_option_roms_fixture() else {
        return;
    };
    launch.config.machine["board"]["rtc"]
        .as_table_mut()
        .unwrap()
        .insert("start".into(), "2000-02-30T00:00:00".into());
    let options = assembly::AssemblyOptions::default();
    let lowered = lower(&launch, &content, options).unwrap_err().to_string();
    assert!(lowered.contains("invalid RTC start"), "{lowered}");
    assert_eq!(check(&launch, options).unwrap_err().to_string(), lowered);
}

#[test]
fn check_and_lower_refuse_overlapping_option_roms_with_the_same_sentence() {
    let Some((mut launch, content)) = overlapping_option_roms_fixture() else {
        return;
    };
    let options = assembly::AssemblyOptions::default();
    let lowered = lower(&launch, &content, options).unwrap_err().to_string();
    assert!(lowered.contains("overlap"), "{lowered}");
    assert_eq!(check(&launch, options).unwrap_err().to_string(), lowered);

    // A length must not make unread video firmware look identified.
    launch.config.machine["firmware"]["option_roms"][0]["base"] = 0xC0000.into();
    assert!(check(&launch, options).is_ok());
}
