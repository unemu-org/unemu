// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware package licences, source archives, descriptive metadata, the
//! generated README and metadata revisions; machine-images §5.3. Synthetic
//! ROMs and archives only, except the machine-image test, which needs the
//! open Xi8088 BIOS fixture.

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

use image_store::{
    firmware::{ArchiveFormat, Config, Provenance, ProvenanceKind, Validator},
    license, media_type as mt,
    summary::{Facets, SummaryCache},
    BlobSource, Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB,
};
use pc_image::{
    firmware::PcFirmwareRules,
    firmware_import::{self, Attachment, Candidate, ReadmeChoice, Statement},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "firmware-metadata-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn store(&self, name: &str) -> Store {
        Store::open(self.0.join(name)).unwrap()
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn leaked(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
const OPEN_ROM: u8 = 0x41;
const VENDOR_ROM: u8 = 0x42;
const EVEN_CHIP: u8 = 0x51;
const ODD_CHIP: u8 = 0x52;
/// The source archive the synthetic fetch script would build for the open BIOS.
fn pinned() -> Vec<u8> {
    tar(6000)
}
/// One open copyleft system BIOS with its pinned source archive, one
/// proprietary single-image BIOS and one proprietary two-chip BIOS, as the
/// real catalogue lists the Kiselev BIOS and IBM BIOSes.
fn catalogue() -> Vec<assembly::FirmwareEntry> {
    let entry = |id, sha256: Digest, distribution| assembly::FirmwareEntry {
        id,
        sha256: leaked(sha256.hex().into()),
        size: 32768,
        distribution,
        ..assembly::KISELEV_XI8088
    };
    let chip = |label, lane, byte| assembly::KnownPart {
        label,
        part_number: "synthetic",
        lane,
        offset: 0,
        size: 16384,
        sha256: leaked(Digest::of(&[byte; 16384]).hex().into()),
    };
    let chips: &'static [assembly::KnownPart] = Box::leak(Box::new([
        chip("U27", assembly::Lane::Even, EVEN_CHIP),
        chip("U47", assembly::Lane::Odd, ODD_CHIP),
    ]));
    vec![
        entry(
            "synthetic-open-bios",
            Digest::of(&[OPEN_ROM; 32768]),
            assembly::FirmwareDistribution::Open {
                fetch: "synthetic/fetch.sh",
                license: "GPL-3.0-or-later",
                provenance: "synthetic/bios at a fixed revision",
                source_archive_sha256: Some(leaked(Digest::of(&pinned()).hex().into())),
            },
        ),
        entry(
            "synthetic-vendor-bios",
            Digest::of(&[VENDOR_ROM; 32768]),
            assembly::FirmwareDistribution::UserSupplied,
        ),
        assembly::FirmwareEntry {
            parts: Box::leak(Box::new([chips])),
            ..entry(
                "synthetic-vendor-chips",
                Digest::of(&[EVEN_CHIP, ODD_CHIP].repeat(16384)),
                assembly::FirmwareDistribution::UserSupplied,
            )
        },
    ]
}
/// A POSIX tar signature at offset 257 followed by `size` bytes in total.
fn tar(size: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; size.max(512)];
    bytes[..10].copy_from_slice(b"src/a.asm\0");
    bytes[257..263].copy_from_slice(b"ustar\0");
    for (i, b) in bytes.iter_mut().enumerate().skip(512) {
        *b = (i % 61) as u8;
    }
    bytes
}
fn scanned(t: &Temp, byte: u8, cat: &[assembly::FirmwareEntry]) -> Candidate {
    let folder = t.0.join(format!("roms-{byte:02x}"));
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("rom.bin"), [byte; 32768]).unwrap();
    let mut scan = firmware_import::scan(&[folder], cat).unwrap();
    assert_eq!(scan.candidates.len(), 1, "{:?}", scan.issues);
    scan.candidates.remove(0)
}
fn unlisted(t: &Temp, name: &str, cat: &[assembly::FirmwareEntry]) -> Candidate {
    let rom = t.file(&format!("{name}.bin"), &[0x55; 8192]);
    firmware_import::unlisted(&rom, name, "option-rom", cat).unwrap()
}
fn resolve(store: &Store, tag: &str, cat: &[assembly::FirmwareEntry]) -> (Manifest, Config) {
    let image = store
        .resolve(tag, &Validator(&PcFirmwareRules(cat)))
        .unwrap();
    (image.manifest, Config::decode(&image.config).unwrap())
}
fn layer<'a>(manifest: &'a Manifest, kind: &str) -> &'a Descriptor {
    manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == kind)
        .unwrap_or_else(|| panic!("no {kind} layer"))
}
fn class(d: &Descriptor) -> Option<String> {
    license::open_license(d).map(|l| l.as_str().to_owned())
}
fn readme(store: &Store, manifest: &Manifest) -> String {
    let d = layer(manifest, mt::MARKDOWN);
    String::from_utf8(store.read_verified(d, MAX_SMALL_BLOB).unwrap()).unwrap()
}
fn open(license: &str) -> Statement {
    Statement {
        open: true,
        license: Some(license.into()),
        ..Statement::default()
    }
}

fn with_source(path: PathBuf) -> Statement {
    Statement {
        source: Some(path),
        ..Statement::default()
    }
}

#[test]
fn a_catalogue_open_rom_is_open_with_its_pinned_source_archive_and_prefilled_metadata() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = scanned(&t, OPEN_ROM, &cat);
    let statement = with_source(t.file("synthetic-src.tar", &pinned()));
    let preview =
        firmware_import::preview_manifest_with(&candidate, &[], &statement, &cat).unwrap();
    assert!(preview.notes.is_empty(), "{:?}", preview.notes);
    let d = firmware_import::publish_with(&store, &candidate, &[], &statement, &cat).unwrap();
    let (manifest, config) = resolve(&store, &candidate.tag(), &cat);
    assert_eq!(d.digest, Digest::of(&manifest.encode().unwrap()));
    assert_eq!(preview.manifest, manifest);
    // Catalogue lists are kept sorted.
    assert_eq!(
        config.roms[0].requires,
        ["kbc-8042", "pic-cascade", "rtc-cmos"]
    );
    let provenance = config.provenance.as_ref().unwrap();
    assert_eq!(provenance.from, "synthetic/bios at a fixed revision");
    assert_eq!(provenance.kind, None, "the catalogue does not say how");
    assert_eq!(
        class(layer(&manifest, mt::FIRMWARE)).as_deref(),
        Some("GPL-3.0-or-later")
    );
    // The pinned archive is open under the catalogue licence without a statement.
    assert_eq!(
        class(layer(&manifest, mt::TAR)).as_deref(),
        Some("GPL-3.0-or-later")
    );
    assert_eq!(
        class(layer(&manifest, mt::MARKDOWN)).as_deref(),
        Some("CC0-1.0")
    );
    assert_eq!(class(&manifest.config).as_deref(), Some("CC0-1.0"));
    assert_eq!(
        manifest.annotations["org.opencontainers.image.licenses"],
        "GPL-3.0-or-later AND CC0-1.0"
    );
    assert_eq!(
        manifest.annotations["org.opencontainers.image.title"],
        "synthetic-open-bios"
    );
    assert!(!manifest
        .annotations
        .contains_key("org.opencontainers.image.created"));
    let text = readme(&store, &manifest);
    assert!(text.contains("- ROM images: GPL-3.0-or-later, open (redistributable).\n"));
    assert!(text.contains("Origin: synthetic/bios at a fixed revision\n"));
    assert!(text.contains("- Requires: kbc-8042, pic-cascade, rtc-cmos\n"));
    // The catalogue's licence is not open to restatement; restating it changes nothing.
    let other = t.store("other");
    let restated = |license: &str| Statement {
        source: statement.source.clone(),
        ..open(license)
    };
    let err = firmware_import::publish_with(&other, &candidate, &[], &restated("MIT"), &cat)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("records this ROM's licence as GPL-3.0-or-later"),
        "{err}"
    );
    let same =
        firmware_import::publish_with(&other, &candidate, &[], &restated("GPL-3.0-or-later"), &cat)
            .unwrap();
    assert_eq!(same.digest, d.digest);
}

#[test]
fn a_catalogue_open_copyleft_rom_without_its_open_source_stays_user_supplied() {
    let t = Temp::new();
    let cat = catalogue();
    let candidate = scanned(&t, OPEN_ROM, &cat);
    // No archive: user-supplied, with a note naming the script that builds it.
    let bare = firmware_import::preview_manifest_with(&candidate, &[], &Statement::default(), &cat)
        .unwrap();
    assert!(bare.notes[0].starts_with("kept user-supplied: GPL-3.0-or-later is copyleft"));
    assert!(bare.notes[0].contains("add the archive synthetic/fetch.sh builds with --source"));
    let store = t.store("store");
    firmware_import::publish(&store, &candidate, &[], &cat).unwrap();
    let (manifest, _) = resolve(&store, &candidate.tag(), &cat);
    assert!(std::iter::once(&manifest.config)
        .chain(&manifest.layers)
        .all(
            |d| d.annotations[license::REDISTRIBUTION] == "user-supplied"
                && !d.annotations.contains_key(license::LICENSE)
        ));
    // An explicit open statement that cannot hold is refused, not downgraded.
    let err = firmware_import::publish_with(
        &t.store("explicit"),
        &candidate,
        &[],
        &Statement {
            open: true,
            ..Statement::default()
        },
        &cat,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("copyleft"), "{err}");
    // Another archive than the pinned one needs the user's licence statement.
    let foreign = t.file("other-src.tar", &tar(7000));
    let other = firmware_import::preview_manifest_with(
        &candidate,
        &[],
        &with_source(foreign.clone()),
        &cat,
    )
    .unwrap();
    assert!(
        other.notes[0].contains("not the one synthetic/fetch.sh builds"),
        "{:?}",
        other.notes
    );
    assert!(class(layer(&other.manifest, mt::TAR)).is_none());
    assert!(class(layer(&other.manifest, mt::FIRMWARE)).is_none());
    let stated = Statement {
        source: Some(foreign),
        source_license: Some("GPL-3.0-or-later".into()),
        open: true,
        ..Statement::default()
    };
    let stated = firmware_import::preview_manifest_with(&candidate, &[], &stated, &cat).unwrap();
    assert_eq!(
        class(layer(&stated.manifest, mt::TAR)).as_deref(),
        Some("GPL-3.0-or-later")
    );
    assert_eq!(
        class(layer(&stated.manifest, mt::FIRMWARE)).as_deref(),
        Some("GPL-3.0-or-later")
    );
    // The user may keep open firmware private; the pinned archive stays open.
    let kept = Statement {
        user_supplied: true,
        source: Some(t.file("pinned-src.tar", &pinned())),
        ..Statement::default()
    };
    let kept = firmware_import::preview_manifest_with(&candidate, &[], &kept, &cat).unwrap();
    assert!(class(layer(&kept.manifest, mt::FIRMWARE)).is_none());
    assert!(class(&kept.manifest.config).is_none());
}

#[test]
fn a_proprietary_catalogue_rom_can_never_be_marked_open() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = scanned(&t, VENDOR_ROM, &cat);
    let err = firmware_import::publish_with(&store, &candidate, &[], &open("MIT"), &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("proprietary"), "{err}");
    assert!(store.list_tags(None, 10).unwrap().tags.is_empty());
    firmware_import::publish(&store, &candidate, &[], &cat).unwrap();
    let (manifest, config) = resolve(&store, &candidate.tag(), &cat);
    assert!(config.provenance.is_none());
    assert!(class(layer(&manifest, mt::FIRMWARE)).is_none());
    assert!(class(&manifest.config).is_none());
    assert!(!manifest
        .annotations
        .contains_key("org.opencontainers.image.licenses"));
    let err = firmware_import::edit(&store, &candidate.tag(), "firmware/x:2", &open("MIT"), &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("proprietary"), "{err}");
}

#[test]
fn rights_follow_the_bytes_even_for_a_crafted_candidate() {
    let t = Temp::new();
    let cat = catalogue();
    let mut candidate = scanned(&t, VENDOR_ROM, &cat);
    // Clearing the catalogue id and claiming open firmware does not help.
    candidate.config.roms[0].catalogue_id = None;
    candidate.distribution = assembly::FirmwareDistribution::Open {
        fetch: "crafted/fetch.sh",
        license: "MIT",
        provenance: "crafted",
        source_archive_sha256: None,
    };
    let store = t.store("store");
    let err = firmware_import::publish_with(&store, &candidate, &[], &open("MIT"), &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("proprietary"), "{err}");
    firmware_import::publish(&store, &candidate, &[], &cat).unwrap();
    let (manifest, _) = resolve(&store, &candidate.tag(), &cat);
    assert!(class(layer(&manifest, mt::FIRMWARE)).is_none());
    // One chip of a proprietary layout is proprietary too, as a ROM or a document.
    let chip = t.file("chip.bin", &[EVEN_CHIP; 16384]);
    let err = firmware_import::unlisted(&chip, "chip", "option-rom", &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("synthetic-vendor-chips"), "{err}");
    let host = unlisted(&t, "documented", &cat);
    let extras = [Attachment {
        path: chip,
        dos_name: None,
    }];
    let licensed = Statement {
        extra_licenses: vec![Some("MIT".into())],
        ..Statement::default()
    };
    let err = firmware_import::publish_with(&t.store("doc"), &host, &extras, &licensed, &cat)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("synthetic-vendor-chips") && err.contains("proprietary"),
        "{err}"
    );
    firmware_import::publish(&t.store("doc-private"), &host, &extras, &cat).unwrap();
    // A catalogue id is never an unlisted package id.
    let rom = t.file("named.bin", &[0x77; 16]);
    let err = firmware_import::unlisted(&rom, "synthetic-open-bios", "option-rom", &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("catalogue id"), "{err}");
}

#[test]
fn an_edit_cannot_open_bytes_a_newer_catalogue_lists_as_proprietary() {
    let t = Temp::new();
    // Imported where the catalogue does not know the ROM.
    let candidate = unlisted(&t, "later-listed", &[]);
    let store = t.store("store");
    firmware_import::publish_with(&store, &candidate, &[], &open("MIT"), &[]).unwrap();
    let (manifest, _) = resolve(&store, "firmware/later-listed:1", &[]);
    assert_eq!(
        class(layer(&manifest, mt::FIRMWARE)).as_deref(),
        Some("MIT")
    );
    let newer = vec![assembly::FirmwareEntry {
        id: "synthetic-later-listed",
        sha256: leaked(Digest::of(&[0x55; 8192]).hex().into()),
        size: 8192,
        kind: assembly::FirmwareKind::OptionRom,
        distribution: assembly::FirmwareDistribution::UserSupplied,
        ..assembly::KISELEV_XI8088
    }];
    let err = firmware_import::edit(
        &store,
        "firmware/later-listed:1",
        "firmware/later-listed:2",
        &open("MIT"),
        &newer,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("proprietary"), "{err}");
    let relabelled = Statement {
        labels: Some(vec!["listed".into()]),
        ..Statement::default()
    };
    firmware_import::edit(
        &store,
        "firmware/later-listed:1",
        "firmware/later-listed:2",
        &relabelled,
        &newer,
    )
    .unwrap();
    let (manifest, _) = resolve(&store, "firmware/later-listed:2", &newer);
    assert!(
        class(layer(&manifest, mt::FIRMWARE)).is_none(),
        "locked by bytes"
    );
    assert!(class(&manifest.config).is_none());
}

#[test]
fn an_unlisted_rom_is_user_supplied_unless_the_user_names_its_licence() {
    let t = Temp::new();
    let cat = catalogue();
    let candidate = unlisted(&t, "plain-rom", &cat);
    let store = t.store("store");
    for (statement, reason) in [
        (
            Statement {
                open: true,
                ..Statement::default()
            },
            "--license",
        ),
        (
            Statement {
                license: Some("MIT".into()),
                ..Statement::default()
            },
            "add --open",
        ),
        (open("Made-Up-1.0"), "unknown SPDX licence identifier"),
        (open("GPL-2.0-only"), "copyleft"),
        (
            Statement {
                source: Some(t.file("unstated-src.tar", &tar(600))),
                ..open("GPL-2.0-only")
            },
            "--source-license",
        ),
    ] {
        let err = firmware_import::publish_with(&store, &candidate, &[], &statement, &cat)
            .unwrap_err()
            .to_string();
        assert!(err.contains(reason), "{statement:?}: {err}");
    }
    assert!(store.list_tags(None, 10).unwrap().tags.is_empty());
    firmware_import::publish(&store, &candidate, &[], &cat).unwrap();
    let (manifest, config) = resolve(&store, "firmware/plain-rom:1", &cat);
    assert!(config.roms[0].catalogue_id.is_none());
    assert!(class(layer(&manifest, mt::FIRMWARE)).is_none());
    // The README of a minimal package, byte for byte.
    let expected = format!(
        "# plain-rom\n\n- Platform: ibm-pc-at\n- Vendor: not recorded\n- Version: not recorded\n\
         - Date: not recorded\n\n## Fits\n\n- Boards: unspecified\n- Machine classes: unspecified\n\
         - Not in the firmware catalogue: the fit and role claims are the user's and unverified.\n\n\
         ## ROM images\n\n### option-rom (Option ROM)\n\n- Size: 8192 bytes\n- SHA-256: {}\n\n\
         ## Provenance\n\nNot recorded.\n\n## Licence\n\n- ROM images: user-supplied. No \
         redistribution right is recorded; keep this package in private stores and registries.\n\
         - This README and the package config: user-supplied, like the package.\n",
        Digest::of(&[0x55; 8192]).hex()
    );
    assert_eq!(readme(&store, &manifest), expected);
    let named = t.store("named");
    firmware_import::publish_with(&named, &candidate, &[], &open("MIT"), &cat).unwrap();
    let (manifest, _) = resolve(&named, "firmware/plain-rom:1", &cat);
    assert_eq!(
        class(layer(&manifest, mt::FIRMWARE)).as_deref(),
        Some("MIT")
    );
    assert_eq!(class(&manifest.config).as_deref(), Some("CC0-1.0"));
    // A ROM the catalogue knows is imported through the catalogue.
    let known = t.file("known.bin", &[OPEN_ROM; 32768]);
    let err = firmware_import::unlisted(&known, "renamed", "system-bios", &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("--for synthetic-open-bios"), "{err}");
    let rom = t.file("bad-role.bin", &[1; 16]);
    assert!(firmware_import::unlisted(&rom, "bad-role", "bios", &cat).is_err());
    assert!(firmware_import::unlisted(&rom, "Bad Id", "option-rom", &cat).is_err());
}

#[test]
fn a_copyleft_rom_is_open_only_with_its_source_archive_which_it_links() {
    let t = Temp::new();
    let cat = catalogue();
    let candidate = unlisted(&t, "copyleft-rom", &cat);
    let source = t.file("copyleft-src.tar", &tar(16 * 1024));
    let store = t.store("store");
    let statement = Statement {
        source: Some(source.clone()),
        source_license: Some("GPL-2.0-only".into()),
        ..open("GPL-2.0-only")
    };
    firmware_import::publish_with(&store, &candidate, &[], &statement, &cat).unwrap();
    let (manifest, config) = resolve(&store, "firmware/copyleft-rom:1", &cat);
    let archive = layer(&manifest, mt::TAR);
    // A source archive of at least 4 KiB is a compressible payload.
    assert_eq!(archive.media_type, "application/x-tar+zstd");
    assert_eq!(archive.decoded_size().unwrap(), 16 * 1024);
    assert_eq!(class(archive).as_deref(), Some("GPL-2.0-only"));
    assert_eq!(
        archive.annotations["org.opencontainers.image.title"],
        "copyleft-src.tar"
    );
    let rom = layer(&manifest, mt::FIRMWARE);
    assert_eq!(
        rom.annotations[license::SOURCE],
        archive.decoded_digest().unwrap().to_string()
    );
    assert_eq!(config.sources.len(), 1);
    assert_eq!(config.sources[0].format, ArchiveFormat::Tar);
    assert_eq!(config.sources[0].roms, ["option-rom"]);
    assert_eq!(config.sources[0].filename, "copyleft-src.tar");
    let kinds: Vec<_> = manifest
        .layers
        .iter()
        .map(|d| d.decoded_media_type())
        .collect();
    assert_eq!(kinds, [mt::FIRMWARE, mt::TAR, mt::MARKDOWN]);
    let text = readme(&store, &manifest);
    assert!(text.contains("- Source archive copyleft-src.tar (tar, 16384 bytes, SHA-256 "));
    assert!(text.contains("the corresponding source of option-rom; GPL-2.0-only, open."));
    // A zip archive is already compressed and stays raw; other files are refused.
    let zip = t.file(
        "copyleft-src.zip",
        &[b"PK\x03\x04".as_slice(), &[7; 8000]].concat(),
    );
    let zipped = t.store("zipped");
    let statement = Statement {
        source: Some(zip),
        source_license: Some("LGPL-2.1-or-later".into()),
        ..open("LGPL-2.1-or-later")
    };
    firmware_import::publish_with(&zipped, &candidate, &[], &statement, &cat).unwrap();
    let (manifest, _) = resolve(&zipped, "firmware/copyleft-rom:1", &cat);
    assert_eq!(layer(&manifest, mt::ZIP).media_type, mt::ZIP);
    let text = t.file("not-an-archive.tar", &[b'x'; 600]);
    let statement = Statement {
        source: Some(text),
        source_license: Some("GPL-2.0-only".into()),
        ..open("GPL-2.0-only")
    };
    let err = firmware_import::publish_with(&t.store("refused"), &candidate, &[], &statement, &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("zip or POSIX tar"), "{err}");
}

#[test]
fn the_config_and_readme_are_open_only_when_every_other_layer_is() {
    let t = Temp::new();
    let cat = catalogue();
    let candidate = unlisted(&t, "documented-rom", &cat);
    let manual = t.file("manual.txt", b"synthetic manual");
    let extras = [Attachment {
        path: manual,
        dos_name: None,
    }];
    let store = t.store("store");
    firmware_import::publish_with(&store, &candidate, &extras, &open("MIT"), &cat).unwrap();
    let (manifest, _) = resolve(&store, "firmware/documented-rom:1", &cat);
    assert_eq!(
        class(layer(&manifest, mt::FIRMWARE)).as_deref(),
        Some("MIT")
    );
    assert!(class(layer(&manifest, mt::EXTRA)).is_none());
    assert!(class(layer(&manifest, mt::MARKDOWN)).is_none());
    assert!(class(&manifest.config).is_none());
    assert!(!manifest
        .annotations
        .contains_key("org.opencontainers.image.licenses"));
    assert!(readme(&store, &manifest)
        .contains("- Document manual.txt: no licence recorded, user-supplied.\n"));
    let licensed = t.store("licensed");
    let statement = Statement {
        extra_licenses: vec![Some("CC-BY-4.0".into())],
        ..open("MIT")
    };
    firmware_import::publish_with(&licensed, &candidate, &extras, &statement, &cat).unwrap();
    let (manifest, _) = resolve(&licensed, "firmware/documented-rom:1", &cat);
    assert_eq!(
        class(layer(&manifest, mt::EXTRA)).as_deref(),
        Some("CC-BY-4.0")
    );
    assert_eq!(class(&manifest.config).as_deref(), Some("CC0-1.0"));
    assert_eq!(
        manifest.annotations["org.opencontainers.image.licenses"],
        "MIT AND CC-BY-4.0 AND CC0-1.0"
    );
    // The package licence joins the parsed layer licences, so several long
    // document licences do not run into the length limit of one statement.
    let long = |n: usize| format!("LicenseRef-{}-{n}", "d".repeat(180));
    let documents: Vec<Attachment> = (0..3)
        .map(|n| Attachment {
            path: t.file(&format!("doc-{n}.txt"), format!("document {n}").as_bytes()),
            dos_name: None,
        })
        .collect();
    let statement = Statement {
        extra_licenses: (0..3).map(|n| Some(long(n))).collect(),
        ..open("MIT")
    };
    let preview =
        firmware_import::preview_manifest_with(&candidate, &documents, &statement, &cat).unwrap();
    let joined = &preview.manifest.annotations["org.opencontainers.image.licenses"];
    assert!(joined.len() > 256);
    assert_eq!(
        joined,
        &format!(
            "MIT AND {} AND {} AND {} AND CC0-1.0",
            long(0),
            long(1),
            long(2)
        )
    );
}

fn described(source: PathBuf) -> Statement {
    Statement {
        title: Some("Synthetic option ROM r7".into()),
        description: Some("An option ROM for a synthetic disk adapter.".into()),
        notes: Some("Scans the adapter twice.\nNeeds the primary channel.".into()),
        vendor: Some("Example project".into()),
        version: Some("r7".into()),
        date: Some("2026-06-09".into()),
        labels: Some(vec!["xt-ide".into(), "open-source".into()]),
        boards: Some(vec!["ibm-5170".into()]),
        machine_classes: Some(vec!["at".into()]),
        requires: Some(vec!["ata-primary".into()]),
        provenance: Some(Provenance {
            kind: Some(ProvenanceKind::Built),
            from: "example/option-rom with no patches".into(),
            revision: Some("r7".into()),
            url: Some("https://example.org/option-rom".into()),
            date: Some("2026-06-10".into()),
        }),
        source: Some(source),
        source_license: Some("MPL-2.0".into()),
        ..open("MPL-2.0")
    }
}

#[test]
fn manifests_and_readmes_are_byte_identical_across_imports() {
    let t = Temp::new();
    let cat = catalogue();
    let candidate = unlisted(&t, "described-rom", &cat);
    let statement = described(t.file("described-src.tar", &tar(6000)));
    let manual = t.file("notes.txt", b"build notes");
    let extras = [Attachment {
        path: manual,
        dos_name: Some("NOTES.TXT".into()),
    }];
    let preview =
        firmware_import::preview_manifest_with(&candidate, &extras, &statement, &cat).unwrap();
    let mut packages = Vec::new();
    for name in ["first", "second"] {
        let store = t.store(name);
        let d =
            firmware_import::publish_with(&store, &candidate, &extras, &statement, &cat).unwrap();
        let (manifest, _) = resolve(&store, "firmware/described-rom:1", &cat);
        packages.push((d, manifest.encode().unwrap(), readme(&store, &manifest)));
    }
    assert_eq!(packages[0], packages[1]);
    assert_eq!(
        packages[0].1,
        preview.manifest.encode().unwrap(),
        "known before writing"
    );
    let (_, bytes, text) = &packages[0];
    let manifest = Manifest::decode(bytes).unwrap();
    for (key, value) in [
        ("org.opencontainers.image.title", "Synthetic option ROM r7"),
        (
            "org.opencontainers.image.description",
            "An option ROM for a synthetic disk adapter.",
        ),
        ("org.opencontainers.image.vendor", "Example project"),
        ("org.opencontainers.image.version", "r7"),
        (
            "org.opencontainers.image.source",
            "https://example.org/option-rom",
        ),
        ("org.opencontainers.image.revision", "r7"),
    ] {
        assert_eq!(manifest.annotations[key], value, "{key}");
    }
    for line in [
        "# Synthetic option ROM r7\n\nAn option ROM for a synthetic disk adapter.\n",
        "- Labels: open-source, xt-ide\n",
        "- Boards: ibm-5170\n- Machine classes: at\n",
        "- Requires: ata-primary\n",
        "## Provenance\n\nBuilt from example/option-rom with no patches\n\n- Revision: r7\n",
        "- Date: 2026-06-10\n",
        "## Notes and known quirks\n\nScans the adapter twice.\nNeeds the primary channel.\n",
        "- ROM images: MPL-2.0, open (redistributable).\n",
        "- Document notes.txt: no licence recorded, user-supplied.\n",
    ] {
        assert!(text.contains(line), "{line:?} missing from\n{text}");
    }
    assert!(!text.contains('\r'));
}

#[test]
fn an_edit_publishes_a_new_revision_with_the_same_rom_blobs() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = unlisted(&t, "edited-rom", &cat);
    let first = firmware_import::publish(&store, &candidate, &[], &cat).unwrap();
    assert_eq!(
        firmware_import::next_revision("firmware/edited-rom:1").as_deref(),
        Some("firmware/edited-rom:2")
    );
    for tag in ["firmware/edited-rom:latest", "x:01", "x:"] {
        assert_eq!(firmware_import::next_revision(tag), None, "{tag}");
    }
    let statement = Statement {
        labels: Some(vec!["edited".into()]),
        source: Some(t.file("edited-src.tar", &tar(5000))),
        source_license: Some("GPL-2.0-or-later".into()),
        ..open("GPL-2.0-or-later")
    };
    let second = firmware_import::edit(
        &store,
        "firmware/edited-rom:1",
        "firmware/edited-rom:2",
        &statement,
        &cat,
    )
    .unwrap();
    assert!(second.notes.is_empty());
    assert_ne!(first.digest, second.descriptor.digest);
    assert_eq!(
        store
            .tag_descriptor("firmware/edited-rom:1")
            .unwrap()
            .digest,
        first.digest,
        "a package is immutable"
    );
    let (old, _) = resolve(&store, "firmware/edited-rom:1", &cat);
    let (new, config) = resolve(&store, "firmware/edited-rom:2", &cat);
    assert_eq!(
        layer(&old, mt::FIRMWARE).digest,
        layer(&new, mt::FIRMWARE).digest,
        "the same stored ROM blob"
    );
    assert_eq!(config.labels, ["edited"]);
    assert_eq!(
        class(layer(&new, mt::FIRMWARE)).as_deref(),
        Some("GPL-2.0-or-later")
    );
    assert_eq!(config.sources.len(), 1);
    assert!(readme(&store, &new).contains("- Labels: edited\n"));
    // A third revision keeps the source archive and licence without restating them.
    let third = Statement {
        title: Some("Edited ROM".into()),
        ..Statement::default()
    };
    firmware_import::edit(
        &store,
        "firmware/edited-rom:2",
        "firmware/edited-rom:3",
        &third,
        &cat,
    )
    .unwrap();
    let (newest, config) = resolve(&store, "firmware/edited-rom:3", &cat);
    assert_eq!(layer(&newest, mt::TAR).digest, layer(&new, mt::TAR).digest);
    assert_eq!(
        class(layer(&newest, mt::FIRMWARE)).as_deref(),
        Some("GPL-2.0-or-later")
    );
    assert_eq!(config.name, "Edited ROM");
    for (target, statement, reason) in [
        ("firmware/edited-rom:4", third.clone(), "changes nothing"),
        (
            "firmware/edited-rom:1",
            Statement {
                title: Some("Other".into()),
                ..Statement::default()
            },
            "exists",
        ),
    ] {
        let err = firmware_import::edit(&store, "firmware/edited-rom:3", target, &statement, &cat)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(reason) || err.contains("conflict"),
            "{target}: {err}"
        );
    }
}

/// A package as the program wrote it before licences and READMEs: the ROM
/// marked open without a licence, no README layer, no descriptive fields.
fn old_package(
    store: &Store,
    candidate: &Candidate,
    cat: &[assembly::FirmwareEntry],
) -> Descriptor {
    let rom = fs::read(&candidate.sources[0]).unwrap();
    let blob = store.import(rom.as_slice(), None).unwrap();
    let mut layer = Descriptor::new(mt::FIRMWARE, blob.digest.clone(), blob.size);
    layer
        .annotations
        .insert(license::REDISTRIBUTION.into(), "open".into());
    let mut config = candidate.config.clone();
    config.roms[0].requires.clear();
    config.provenance = None;
    let text = config.encode().unwrap();
    let stored = store.import(text.as_slice(), None).unwrap();
    let mut config = Descriptor::new(mt::FIRMWARE_CONFIG, stored.digest, stored.size);
    config
        .annotations
        .insert(license::REDISTRIBUTION.into(), "user-supplied".into());
    let manifest = Manifest::new_firmware("ibm-pc-at", config, vec![layer]).unwrap();
    let d = store.import_manifest(&manifest).unwrap();
    store
        .update_tag(
            &candidate.tag(),
            &d,
            None,
            &Validator(&PcFirmwareRules(cat)),
        )
        .unwrap();
    d
}

#[test]
fn packages_written_before_licences_stay_readable_and_user_supplied() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = scanned(&t, OPEN_ROM, &cat);
    let old = old_package(&store, &candidate, &cat);
    let (manifest, config) = resolve(&store, &candidate.tag(), &cat);
    assert!(config.readme.is_none() && config.sources.is_empty());
    assert!(
        class(layer(&manifest, mt::FIRMWARE)).is_none(),
        "open without a licence"
    );
    let mut cache = SummaryCache::open(&store).unwrap();
    while !cache.pending().is_empty() {
        cache.rebuild(&store, 64, &[], &[]).unwrap();
    }
    let summary = cache
        .summary(&old.digest)
        .unwrap()
        .firmware
        .clone()
        .unwrap();
    assert!(!summary.open);
    assert!(summary.licenses.is_empty());
    // Importing the folder again finds the earlier revision and keeps it.
    let preview = firmware_import::preview_manifest(&candidate, &[]).unwrap();
    assert!(
        class(layer(&preview, mt::FIRMWARE)).is_none(),
        "no source, kept private"
    );
    assert_eq!(
        firmware_import::existing(&store, &candidate.tag(), &preview, &cat).unwrap(),
        Some(Descriptor::new(mt::MANIFEST, old.digest.clone(), old.size))
    );
    assert_eq!(
        firmware_import::publish(&store, &candidate, &[], &cat)
            .unwrap()
            .digest,
        old.digest
    );
    // An edit brings it up to date; the catalogue supplies the licence.
    let restated = Statement {
        open: true,
        ..with_source(t.file("synthetic-src.tar", &pinned()))
    };
    let target = "firmware/synthetic-open-bios:2";
    firmware_import::edit(&store, &candidate.tag(), target, &restated, &cat).unwrap();
    let (manifest, _) = resolve(&store, target, &cat);
    assert_eq!(
        class(layer(&manifest, mt::FIRMWARE)).as_deref(),
        Some("GPL-3.0-or-later")
    );
    assert!(manifest.layers.iter().any(|d| d.media_type == mt::MARKDOWN));
    // A changed new package under a tag is still a conflict.
    let other = t.store("other");
    firmware_import::publish(&other, &candidate, &[], &cat).unwrap();
    let manual = t.file("added.txt", b"added later");
    let extras = [Attachment {
        path: manual,
        dos_name: None,
    }];
    assert!(firmware_import::publish(&other, &candidate, &extras, &cat).is_err());
}

#[test]
fn summaries_expose_licences_labels_and_their_facets() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = scanned(&t, OPEN_ROM, &cat);
    let labelled = Statement {
        labels: Some(vec!["synthetic".into(), "open-source".into()]),
        description: Some("Searchable description".into()),
        ..with_source(t.file("synthetic-src.tar", &pinned()))
    };
    let open_package =
        firmware_import::publish_with(&store, &candidate, &[], &labelled, &cat).unwrap();
    let private = unlisted(&t, "private-rom", &cat);
    firmware_import::publish(&store, &private, &[], &cat).unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    while !cache.pending().is_empty() {
        cache.rebuild(&store, 64, &[], &[]).unwrap();
    }
    let row = cache.summary(&open_package.digest).unwrap();
    let firmware = row.firmware.as_ref().unwrap();
    assert_eq!(firmware.licenses, ["GPL-3.0-or-later"]);
    assert!(firmware.open);
    assert_eq!(firmware.labels, ["open-source", "synthetic"]);
    assert_eq!(
        firmware.description.as_deref(),
        Some("Searchable description")
    );
    assert!(firmware.readme.is_some());
    assert_eq!(row.labels, ["open-source", "synthetic"]);
    let by_licence = Facets {
        licenses: vec!["GPL-3.0-or-later".into()],
        ..Facets::default()
    };
    let by_label = Facets {
        labels: vec!["open-source".into()],
        ..Facets::default()
    };
    for facets in [by_licence, by_label] {
        let found = cache.query("", &facets);
        assert_eq!(found.len(), 1, "{facets:?}");
        assert_eq!(found[0].image, open_package.digest);
    }
    assert_eq!(cache.query("searchable", &Facets::default()).len(), 1);
    assert_eq!(cache.query("", &Facets::default()).len(), 2);
}

#[test]
fn a_machine_image_embeds_the_source_layer_of_an_open_catalogue_rom() {
    let Some(bytes) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let Some(archive) = fixtures::fixture("kiselev-bios/kiselev-bios-a4af458-atreset-src.tar")
    else {
        return;
    };
    let t = Temp::new();
    let folder = t.0.join("roms");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("bios.bin"), bytes).unwrap();
    let catalogue = pc_image::firmware::catalogue();
    let scan = firmware_import::scan(&[folder], catalogue).unwrap();
    let candidate = &scan.candidates[0];
    let machine = t.file(
        "machine.toml",
        format!(
            "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='absent'\nsha256='{}'\n",
            assembly::KISELEV_XI8088.sha256
        )
        .as_bytes(),
    );
    // The archive the Kiselev fetch script builds is the one the catalogue pins.
    let source = t.file("kiselev-bios-a4af458-atreset-src.tar", &archive);
    let with = t.store("with-source");
    let statement = with_source(source);
    firmware_import::publish_with(&with, candidate, &[], &statement, catalogue).unwrap();
    let (package, _) = resolve(&with, &candidate.tag(), catalogue);
    pc_image::import_machine(&with, &machine, "lab:1", &Default::default()).unwrap();
    let image = with.resolve("lab:1", &pc_image::PcProvider).unwrap();
    let archive = layer(&image.manifest, mt::TAR);
    assert_eq!(
        archive.digest,
        layer(&package, mt::TAR).digest,
        "a shared blob"
    );
    assert_eq!(class(archive).as_deref(), Some("GPL-3.0-or-later"));
    let rom = layer(&image.manifest, mt::FIRMWARE);
    assert_eq!(class(rom).as_deref(), Some("GPL-3.0-or-later"));
    assert_eq!(
        rom.annotations[license::SOURCE],
        archive.decoded_digest().unwrap().to_string()
    );
    let config = String::from_utf8(image.config.clone()).unwrap();
    assert!(config.contains(&format!(
        "source_archive = \"{}\"",
        archive.decoded_digest().unwrap()
    )));
    // Without a source archive in the package, the machine keeps the ROM private.
    let without = t.store("without-source");
    firmware_import::publish(&without, candidate, &[], catalogue).unwrap();
    pc_image::import_machine(&without, &machine, "lab:1", &Default::default()).unwrap();
    let image = without.resolve("lab:1", &pc_image::PcProvider).unwrap();
    assert!(class(layer(&image.manifest, mt::FIRMWARE)).is_none());
    assert!(!image
        .manifest
        .layers
        .iter()
        .any(|d| d.decoded_media_type() == mt::TAR));
}

#[test]
fn a_hand_written_readme_and_cleared_fields_are_what_the_preview_shows_and_the_import_stores() {
    let t = Temp::new();
    let cat = catalogue();
    let candidate = scanned(&t, OPEN_ROM, &cat);
    assert!(candidate.config.provenance.is_some(), "catalogue prefill");
    let source = t.file("synthetic-src.tar", &pinned());
    let generated = with_source(source.clone());
    let written = Statement {
        readme: ReadmeChoice::Text("# Mine\r\n\r\nWritten by hand.\r".into()),
        // An empty provenance clears the catalogue's prefill.
        provenance: Some(Provenance {
            kind: None,
            from: "  ".into(),
            revision: Some(String::new()),
            url: None,
            date: Some(String::new()),
        }),
        description: Some("A described BIOS.".into()),
        ..with_source(source)
    };
    for (name, statement) in [("generated", &generated), ("written", &written)] {
        let store = t.store(name);
        let preview =
            firmware_import::preview_manifest_with(&candidate, &[], statement, &cat).unwrap();
        firmware_import::publish_with(&store, &candidate, &[], statement, &cat).unwrap();
        let (manifest, config) = resolve(&store, &candidate.tag(), &cat);
        assert_eq!(preview.manifest, manifest, "{name}");
        assert_eq!(preview.config, config, "{name}");
        let stored = readme(&store, &manifest);
        assert_eq!(preview.readme, stored, "{name}");
        let regenerated = firmware_import::generated_readme(&config, &manifest);
        assert_eq!(
            firmware_import::hand_written_readme(layer(&manifest, mt::MARKDOWN)),
            name == "written"
        );
        if name == "generated" {
            assert_eq!(regenerated, stored, "the generator alone wrote it");
            assert!(config.provenance.is_some());
        } else {
            assert_eq!(stored, "# Mine\n\nWritten by hand.\n");
            assert_ne!(regenerated, stored, "a hand-written README is told apart");
            assert!(config.provenance.is_none());
            assert_eq!(config.description.as_deref(), Some("A described BIOS."));
        }
    }
    for (statement, reason) in [
        (
            Statement {
                title: Some(" ".into()),
                ..Statement::default()
            },
            "the title must be one line",
        ),
        (
            Statement {
                readme: ReadmeChoice::Text(" \r\n".into()),
                ..Statement::default()
            },
            "the README must not be empty",
        ),
        (
            Statement {
                date: Some("1985-13-01".into()),
                ..Statement::default()
            },
            "the date must be a calendar date",
        ),
        (
            Statement {
                provenance: Some(Provenance {
                    kind: Some(ProvenanceKind::Dumped),
                    from: String::new(),
                    revision: None,
                    url: None,
                    date: None,
                }),
                ..Statement::default()
            },
            "the provenance needs its details",
        ),
        (
            Statement {
                provenance: Some(Provenance {
                    kind: None,
                    from: "a board".into(),
                    revision: None,
                    url: None,
                    date: Some("2026-02-30".into()),
                }),
                ..Statement::default()
            },
            "the provenance date must be a calendar date",
        ),
    ] {
        let err = firmware_import::preview_manifest_with(&candidate, &[], &statement, &cat)
            .unwrap_err()
            .to_string();
        assert!(err.contains(reason), "{reason}: {err}");
    }
}

#[test]
fn an_edit_preview_is_the_revision_the_edit_publishes_and_can_clear_fields() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = unlisted(&t, "previewed-rom", &cat);
    let first = Statement {
        description: Some("First words.".into()),
        notes: Some("A quirk.".into()),
        ..Statement::default()
    };
    firmware_import::publish_with(&store, &candidate, &[], &first, &cat).unwrap();
    let tag = "firmware/previewed-rom:1";
    let target = "firmware/previewed-rom:2";
    let same =
        firmware_import::preview_edit(&store, tag, target, &Statement::default(), &cat).unwrap();
    assert!(same.unchanged);
    let cleared = Statement {
        description: Some(String::new()),
        labels: Some(vec!["edited".into()]),
        ..Statement::default()
    };
    let preview = firmware_import::preview_edit(&store, tag, target, &cleared, &cat).unwrap();
    assert!(!preview.unchanged);
    assert!(preview.preview.config.description.is_none(), "cleared");
    assert_eq!(preview.preview.config.notes.as_deref(), Some("A quirk."));
    assert!(
        store.tag_descriptor(target).is_err(),
        "a preview writes nothing"
    );
    let revision = firmware_import::edit(&store, tag, target, &cleared, &cat).unwrap();
    let (old, _) = resolve(&store, tag, &cat);
    let (new, config) = resolve(&store, target, &cat);
    assert_eq!(
        revision.descriptor.digest,
        Digest::of(&preview.preview.manifest.encode().unwrap())
    );
    assert_eq!(preview.preview.config, config);
    assert_eq!(preview.preview.readme, readme(&store, &new));
    assert_eq!(
        layer(&old, mt::FIRMWARE).digest,
        layer(&new, mt::FIRMWARE).digest
    );
    // The new tag is checked as the edit checks it.
    let err = firmware_import::preview_edit(&store, tag, target, &cleared, &cat)
        .unwrap_err()
        .to_string();
    assert!(err.contains("exists") || err.contains("conflict"), "{err}");
}

#[test]
fn catalogue_rights_and_fits_are_the_import_rules_own() {
    let t = Temp::new();
    let cat = catalogue();
    let rights =
        |candidate: &Candidate| firmware_import::catalogue_rights(&candidate.config, &cat).unwrap();
    assert!(matches!(
        rights(&scanned(&t, OPEN_ROM, &cat)),
        Some(assembly::FirmwareDistribution::Open {
            license: "GPL-3.0-or-later",
            ..
        })
    ));
    assert_eq!(
        rights(&scanned(&t, VENDOR_ROM, &cat)),
        Some(assembly::FirmwareDistribution::UserSupplied)
    );
    assert_eq!(rights(&unlisted(&t, "rights-rom", &cat)), None);
    // Bytes decide: a crafted id cannot open a proprietary ROM.
    let mut crafted = scanned(&t, VENDOR_ROM, &cat);
    crafted.config.roms[0].catalogue_id = Some("synthetic-open-bios".into());
    assert_eq!(
        rights(&crafted),
        Some(assembly::FirmwareDistribution::UserSupplied)
    );
    let fit = firmware_import::catalogue_fit(&cat[0]);
    assert_eq!(fit.boards, ["ibm-5170"]);
    assert_eq!(fit.machine_classes, ["at"]);
    assert_eq!(
        pc_image::firmware::Fit::decode(&scanned(&t, OPEN_ROM, &cat).config.fits).unwrap(),
        fit
    );
}

#[test]
fn a_tag_holding_the_same_roms_is_already_imported_whatever_its_metadata() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = scanned(&t, OPEN_ROM, &cat);
    // The first import states a source archive and a title.
    let stated = Statement {
        title: Some("Described open BIOS".into()),
        ..with_source(t.file("synthetic-src.tar", &pinned()))
    };
    let first = firmware_import::publish_with(&store, &candidate, &[], &stated, &cat).unwrap();
    // A rescan previews the plain catalogue prefill: another manifest, the same ROMs.
    let plain = firmware_import::preview_manifest(&candidate, &[]).unwrap();
    assert_ne!(Digest::of(&plain.encode().unwrap()), first.digest);
    assert_eq!(
        firmware_import::existing(&store, &candidate.tag(), &plain, &cat).unwrap(),
        Some(first.clone())
    );
    assert!(firmware_import::already_imported(&store, &candidate.tag(), &plain, &cat).unwrap());
    // A plain re-import keeps the package; a statement that would change it
    // is refused, naming the edit, instead of being dropped.
    assert_eq!(
        firmware_import::publish(&store, &candidate, &[], &cat)
            .unwrap()
            .digest,
        first.digest
    );
    let other = Statement {
        title: Some("Another title".into()),
        ..Statement::default()
    };
    let err = firmware_import::publish_with(&store, &candidate, &[], &other, &cat)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("already holds these ROMs") && err.contains("image firmware edit"),
        "{err}"
    );
    // Other document bytes under the tag are still a conflict.
    let extras = [Attachment {
        path: t.file("added.txt", b"added later"),
        dos_name: None,
    }];
    assert!(firmware_import::publish(&store, &candidate, &extras, &cat).is_err());
    assert_eq!(
        store.tag_descriptor(&candidate.tag()).unwrap().digest,
        first.digest
    );
}

#[test]
fn an_edit_keeps_a_hand_written_readme_unless_asked_to_generate_it() {
    let t = Temp::new();
    let cat = catalogue();
    let store = t.store("store");
    let candidate = unlisted(&t, "readme-rom", &cat);
    let written = Statement {
        readme: ReadmeChoice::Text("# Mine\n\nWritten by hand.\n".into()),
        ..Statement::default()
    };
    firmware_import::publish_with(&store, &candidate, &[], &written, &cat).unwrap();
    let edit = |from: &str, to: &str, statement: Statement| {
        firmware_import::edit(&store, from, to, &statement, &cat).unwrap();
        let (manifest, config) = resolve(&store, to, &cat);
        let text = readme(&store, &manifest);
        let hand = firmware_import::hand_written_readme(layer(&manifest, mt::MARKDOWN));
        (
            text,
            hand,
            firmware_import::generated_readme(&config, &manifest),
        )
    };
    // Another field changes; the README written by hand stays as it is.
    let (text, hand, _) = edit(
        "firmware/readme-rom:1",
        "firmware/readme-rom:2",
        Statement {
            notes: Some("A quirk.".into()),
            ..Statement::default()
        },
    );
    assert_eq!(text, "# Mine\n\nWritten by hand.\n");
    assert!(hand);
    // Asked to, the edit generates it again, and later edits follow the fields.
    let (text, hand, generated) = edit(
        "firmware/readme-rom:2",
        "firmware/readme-rom:3",
        Statement {
            readme: ReadmeChoice::Generate,
            ..Statement::default()
        },
    );
    assert!(!hand);
    assert_eq!(text, generated);
    assert!(text.contains("## Notes and known quirks\n\nA quirk.\n"));
    let (text, hand, generated) = edit(
        "firmware/readme-rom:3",
        "firmware/readme-rom:4",
        Statement {
            notes: Some("Another quirk.".into()),
            ..Statement::default()
        },
    );
    assert!(!hand);
    assert_eq!(text, generated);
    assert!(text.contains("Another quirk."));
}
