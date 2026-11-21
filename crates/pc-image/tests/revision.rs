// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings on an image and Save as new image…: configuration-only revisions
//! share every disk, firmware and media blob, CMOS bytes follow what SETUP
//! writes, tags follow the store rules and descriptive annotations survive.
//! Open firmware and synthetic disks only; machine-images §§2, 4 and 7.

use image_store::{BlobSource, Descriptor, Digest, Store};
use journal_files::SectorSource;
use pc_image::{
    create::Chs,
    revision::{self, Source},
    workspace, PcProvider, CMOS,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use toml::{Table, Value};
#[path = "support/open_image.rs"]
mod open_image;
use open_image::{fixtures, Open};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// An imported machine whose stored CMOS holds a running clock and options.
struct Stored {
    path: PathBuf,
    store: Store,
    image: Descriptor,
    cmos: [u8; 64],
}
impl Drop for Stored {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn checksum(bytes: &[u8; 64]) -> [u8; 2] {
    bytes[0x10..=0x2D]
        .iter()
        .fold(0u16, |sum, &b| sum.wrapping_add(u16::from(b)))
        .to_be_bytes()
}

/// `None` when a fetched ROM fixture is missing, which skips the test.
fn stored() -> Option<Stored> {
    let mut roms = Vec::new();
    for (from, to) in [
        ("kiselev-bios/bios-xi8088.bin", "bios.bin"),
        (
            "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
            "xtide.bin",
        ),
    ] {
        roms.push((fixtures::fixture(from)?, to));
    }
    let path = std::env::temp_dir().join(format!(
        "image-revision-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    for (bytes, to) in roms {
        fs::write(path.join(to), bytes).unwrap();
    }
    fs::write(path.join("fixed.img"), vec![0; 32 * 512]).unwrap();
    fs::write(path.join("one.img"), vec![0; 360 * 1024]).unwrap();
    // Clock 12:34:56 on 1990-06-15, status registers, A: 360K, one drive, a
    // monochrome display, other option bytes and a valid checksum. The base memory says 512 KiB
    // where the machine derives 640: a field no edit below touches stays as
    // stored.
    let mut cmos = [0u8; 64];
    cmos[..10].copy_from_slice(&[0x56, 0, 0x34, 0, 0x12, 0, 0x06, 0x15, 0x06, 0x90]);
    cmos[0x0A] = 0x26;
    cmos[0x0B] = 0x02;
    cmos[0x0D] = 0x80;
    // A shutdown status byte POST left behind (IBM AT Technical Reference,
    // CMOS RAM Address Map, 0Fh): RTC state no revision may rewrite.
    cmos[0x0F] = 0x0A;
    cmos[0x10] = 0x10;
    cmos[0x11] = 0x5A;
    cmos[0x13] = 0x33;
    cmos[0x14] = 0x31;
    cmos[0x15..=0x16].copy_from_slice(&512u16.to_le_bytes());
    cmos[0x17..=0x18].copy_from_slice(&1024u16.to_le_bytes());
    cmos[0x30..=0x31].copy_from_slice(&1024u16.to_le_bytes());
    cmos[0x32] = 0x19;
    let sum = checksum(&cmos);
    cmos[0x2E..=0x2F].copy_from_slice(&sum);
    fs::write(path.join("cmos.bin"), cmos).unwrap();
    let file = path.join("machine.toml");
    fs::write(&file, "schema=1\nextends='preset:ibm-5170'\n[memory]\nextended_kib=1024\n[firmware.bios]\nimage='bios.bin'\n[[firmware.option_roms]]\nimage='xtide.bin'\nbase=0xC8000\nprovides=['fixed-disk']\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='source-cmos.upfj'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='one.img'\nwrite='journal'\njournal='source-one.upfj'\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=32}\nimage='fixed.img'\nwrite='journal'\njournal='source-fixed.upfj'\nkeep_generations=3\n").unwrap();
    let store = Store::open(path.join("store")).unwrap();
    let image = pc_image::import_machine(
        &store,
        &file,
        "stored:1.0.0",
        &pc_image::ImportOptions {
            no_compress: true,
            ..Default::default()
        },
    )
    .unwrap()
    .manifest;
    Some(Stored {
        path,
        store,
        image,
        cmos,
    })
}

fn table<'a>(machine: &'a mut Table, key: &str) -> &'a mut Table {
    machine.get_mut(key).and_then(Value::as_table_mut).unwrap()
}
fn floppy_drives(machine: &mut Table) -> &mut Vec<Value> {
    machine["card"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c.get("kind").and_then(Value::as_str) == Some("fdc-765"))
        .unwrap()
        .get_mut("drives")
        .and_then(Value::as_array_mut)
        .unwrap()
}
fn with_extended(source: &Source, kib: i64) -> Table {
    let mut machine = source.machine().clone();
    table(&mut machine, "memory").insert("extended_kib".into(), kib.into());
    machine
}
fn non_cmos(layers: &[Descriptor]) -> Vec<Digest> {
    layers
        .iter()
        .filter(|d| d.media_type != CMOS)
        .map(|d| d.digest.clone())
        .collect()
}
fn offsets(changes: &[revision::CmosChange]) -> Vec<usize> {
    changes.iter().map(|c| c.offset).collect()
}
fn cmos_blob(store: &Store, tag: &str) -> Vec<u8> {
    let image = store.resolve(tag, &PcProvider).unwrap();
    let d = image
        .manifest
        .layers
        .iter()
        .find(|d| d.media_type == CMOS)
        .unwrap();
    store.read_verified(d, 64).unwrap()
}

#[test]
fn memory_revision_shares_every_blob_and_rewrites_only_memory_bytes_and_checksum() {
    let Some(fixture) = stored() else {
        return;
    };
    let source = Source::open(&fixture.store, &fixture.image).unwrap();
    assert!(source.stores_cmos());
    let plan = revision::plan(&fixture.store, &source, with_extended(&source, 4096)).unwrap();
    // Only the configuration and the CMOS image are new.
    assert_ne!(plan.machine.0, plan.machine.1);
    let old = fixture
        .store
        .resolve_descriptor(&fixture.image, &PcProvider)
        .unwrap()
        .manifest;
    assert_eq!(non_cmos(&plan.manifest().layers), non_cmos(&old.layers));
    assert_eq!(plan.added.len(), 1);
    assert_eq!(plan.added[0].media_type, CMOS);
    assert!(plan
        .diff
        .contains(&revision::DiffLine::Added("extended_kib = 4096".into())));
    // SETUP writes the expansion memory size at 17h–18h, POST records it at
    // 30h–31h, and the checksum follows; 4096 KiB is 00 10h.
    let mut expected = fixture.cmos;
    expected[0x17..=0x18].copy_from_slice(&4096u16.to_le_bytes());
    expected[0x30..=0x31].copy_from_slice(&4096u16.to_le_bytes());
    let sum = checksum(&expected);
    expected[0x2E..=0x2F].copy_from_slice(&sum);
    let changed: Vec<_> = (0..64)
        .filter(|&i| expected[i] != fixture.cmos[i])
        .collect();
    assert_eq!(offsets(&plan.cmos_changes), changed);
    assert!(changed.iter().all(|i| [0x18, 0x31, 0x2E, 0x2F].contains(i)));
    assert!(changed.contains(&0x18) && changed.contains(&0x31));

    let revised = revision::publish(&fixture.store, &plan, "stored:1.1.0", None).unwrap();
    let bytes = cmos_blob(&fixture.store, "stored:1.1.0");
    assert_eq!(bytes, expected);
    assert_eq!(&bytes[..0x0E], &fixture.cmos[..0x0E], "the clock is kept");
    // The source and its tag are untouched; both images resolve.
    assert_eq!(
        fixture.store.tag_descriptor("stored:1.0.0").unwrap().digest,
        fixture.image.digest
    );
    assert_eq!(cmos_blob(&fixture.store, "stored:1.0.0"), fixture.cmos);
    let image = fixture.store.resolve("stored:1.1.0", &PcProvider).unwrap();
    assert_eq!(image.image.digest, revised.digest);
    assert_eq!(
        image.manifest.annotations[workspace::BASE_DIGEST],
        fixture.image.digest.to_string()
    );
    let machine = pc_image::workspace::prepare_start(
        &fixture.store.workspace("inspect-0").unwrap(),
        "stored:1.1.0",
        false,
    )
    .unwrap()
    .machine;
    assert_eq!(
        machine.memory.address_space_bytes,
        0x100000 + 4096 * 1024,
        "the revision lowers to the new memory size"
    );
}

#[test]
fn drive_revision_rewrites_only_diskette_and_equipment_bytes_and_checksum() {
    let Some(fixture) = stored() else {
        return;
    };
    let source = Source::open(&fixture.store, &fixture.image).unwrap();
    let mut machine = source.machine().clone();
    let mut drive = Table::new();
    drive.insert("unit".into(), 1.into());
    drive.insert("type".into(), "720K".into());
    floppy_drives(&mut machine).push(drive.into());
    let plan = revision::plan(&fixture.store, &source, machine).unwrap();
    // 720K is diskette type 3 (low nibble of 10h, drive B:); two drives set
    // equipment bits 7–6 to 01.
    let mut expected = fixture.cmos;
    expected[0x10] = expected[0x10] & 0xF0 | 0x03;
    expected[0x14] = expected[0x14] & !0xC1 | 0x41;
    let sum = checksum(&expected);
    expected[0x2E..=0x2F].copy_from_slice(&sum);
    let changed: Vec<_> = (0..64)
        .filter(|&i| expected[i] != fixture.cmos[i])
        .collect();
    assert_eq!(offsets(&plan.cmos_changes), changed);
    assert!(changed.iter().all(|i| [0x10, 0x14, 0x2E, 0x2F].contains(i)));
    assert!(changed.contains(&0x10) && changed.contains(&0x14));
    revision::publish(&fixture.store, &plan, "stored:two-drives", None).unwrap();
    let bytes = cmos_blob(&fixture.store, "stored:two-drives");
    assert_eq!(bytes, expected);
    assert_eq!(&bytes[..0x0E], &fixture.cmos[..0x0E], "the clock is kept");
    let machine = pc_image::workspace::prepare_start(
        &fixture.store.workspace("inspect-1").unwrap(),
        "stored:two-drives",
        false,
    )
    .unwrap()
    .machine;
    let floppies = machine
        .drives
        .iter()
        .filter(|d| matches!(d.kind, assembly::DriveKind::Floppy { .. }))
        .count();
    assert_eq!(floppies, 2);
    // The catalogue of A: stays with A:, the new B: starts empty.
    let a = machine
        .drives
        .iter()
        .find(|d| d.unit == 0 && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
        .unwrap();
    assert_eq!(a.media.as_ref().unwrap().entries.len(), 1);
    let b = machine.drives.iter().find(|d| d.unit == 1).unwrap();
    assert!(b.media.as_ref().unwrap().entries.is_empty());
}

#[test]
fn invalid_drafts_and_stranded_catalogue_media_cannot_publish() {
    let Some(fixture) = stored() else {
        return;
    };
    let source = Source::open(&fixture.store, &fixture.image).unwrap();
    assert!(revision::plan(&fixture.store, &source, with_extended(&source, 99_999)).is_err());
    let mut machine = source.machine().clone();
    table(&mut machine, "memory").insert("conventional_kib".into(), 700.into());
    assert!(revision::plan(&fixture.store, &source, machine).is_err());
    // A: holds a 360K medium; a 1.2M drive cannot take it.
    let mut machine = source.machine().clone();
    floppy_drives(&mut machine)[0]
        .as_table_mut()
        .unwrap()
        .insert("type".into(), "1.2M".into());
    let error = revision::plan(&fixture.store, &source, machine)
        .unwrap_err()
        .to_string();
    assert!(error.contains("catalogue"), "{error}");
    // Removing A: would silently drop its medium from the image.
    let mut machine = source.machine().clone();
    floppy_drives(&mut machine).clear();
    let error = revision::plan(&fixture.store, &source, machine)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot be removed"), "{error}");
    let error = revision::plan(&fixture.store, &source, source.machine().clone())
        .unwrap_err()
        .to_string();
    assert!(error.contains("unchanged"), "{error}");
}

#[test]
fn version_tags_never_move_and_other_tags_move_only_from_the_confirmed_digest() {
    let Some(fixture) = stored() else {
        return;
    };
    let source = Source::open(&fixture.store, &fixture.image).unwrap();
    let first = revision::plan(&fixture.store, &source, with_extended(&source, 2048)).unwrap();
    let second = revision::plan(&fixture.store, &source, with_extended(&source, 4096)).unwrap();
    let error = revision::publish(&fixture.store, &first, "stored:1.0.0", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("version tags never move"), "{error}");
    let error = revision::publish(
        &fixture.store,
        &first,
        "stored:1.0.0",
        Some(&fixture.image.digest),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("version tags never move"), "{error}");
    assert_eq!(
        fixture.store.tag_descriptor("stored:1.0.0").unwrap().digest,
        fixture.image.digest
    );
    let a = revision::publish(&fixture.store, &first, "stored:latest", None).unwrap();
    assert!(matches!(
        revision::publish(&fixture.store, &second, "stored:latest", None),
        Err(pc_image::Error::Store(image_store::Error::Conflict { current: Some(d) })) if d == a.digest
    ));
    let b = revision::publish(&fixture.store, &second, "stored:latest", Some(&a.digest)).unwrap();
    assert_eq!(
        fixture
            .store
            .tag_descriptor("stored:latest")
            .unwrap()
            .digest,
        b.digest
    );
    // The image the tag left stays reachable by its digest.
    fixture.store.resolve_descriptor(&a, &PcProvider).unwrap();
}

#[test]
fn display_and_coprocessor_changes_rewrite_only_their_equipment_bits() {
    let Some(fixture) = stored() else {
        return;
    };
    let source = Source::open(&fixture.store, &fixture.image).unwrap();
    // The preset fits a monochrome adapter; without it there is no display.
    let no_display = {
        let mut machine = source.machine().clone();
        machine["card"]
            .as_array_mut()
            .unwrap()
            .retain(|card| card.get("kind").and_then(Value::as_str) != Some("mda"));
        machine
    };
    let fpu = {
        let mut machine = source.machine().clone();
        table(&mut machine, "cpu").insert("fpu".into(), "80287".into());
        machine
    };
    // Equipment byte bits 5–4 are the primary display (11b monochrome, 00b
    // none), bit 1 the math coprocessor; nothing else in 14h or elsewhere moves.
    for (machine, mask, bits) in [(no_display, 0x30u8, 0x00u8), (fpu, 0x02, 0x02)] {
        let plan = revision::plan(&fixture.store, &source, machine).unwrap();
        let mut expected = fixture.cmos;
        expected[0x14] = expected[0x14] & !mask | bits;
        let sum = checksum(&expected);
        expected[0x2E..=0x2F].copy_from_slice(&sum);
        let changed: Vec<_> = (0..64)
            .filter(|&i| expected[i] != fixture.cmos[i])
            .collect();
        assert!(changed.contains(&0x14));
        assert_eq!(offsets(&plan.cmos_changes), changed);
        assert!(changed.iter().all(|i| [0x14, 0x2E, 0x2F].contains(i)));
    }
}

fn class(manifest: &image_store::Manifest, marks: &image_registry::guard::LocalMarks) -> bool {
    image_registry::guard::closure(manifest, marks)
        .iter()
        .all(|entry| entry.effective == image_registry::guard::Redistribution::Open)
}

#[test]
fn a_revision_keeps_every_mark_and_the_redistribution_class_of_its_source() {
    let Some(stored) = stored() else {
        return;
    };
    let Some(open) = Open::new() else {
        return;
    };
    let created = pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), None]),
        "lab:1.0.0",
        None,
    )
    .unwrap();
    for (store, image, tag, open_class) in [
        (&stored.store, &stored.image, "stored:1.1.0", false),
        (&open.store, &created, "lab:1.1.0", true),
    ] {
        let source = Source::open(store, image).unwrap();
        let old = store
            .resolve_descriptor(image, &PcProvider)
            .unwrap()
            .manifest;
        let plan = revision::plan(store, &source, with_extended(&source, 2048)).unwrap();
        let manifest = plan.manifest();
        assert_eq!(manifest.config.annotations["unemu.redistribution"], "open");
        for layer in &manifest.layers {
            match old.layers.iter().find(|d| d.digest == layer.digest) {
                Some(before) => assert_eq!(layer.annotations, before.annotations, "{tag}"),
                None => {
                    assert_eq!(layer.media_type, CMOS);
                    assert_eq!(layer.annotations["unemu.redistribution"], "user-supplied");
                }
            }
        }
        revision::publish(store, &plan, tag, None).unwrap();
        let marks = image_registry::guard::LocalMarks::scan(store).unwrap();
        let new = store.resolve(tag, &PcProvider).unwrap().manifest;
        assert_eq!(class(&old, &marks), open_class, "{tag}");
        assert_eq!(class(&new, &marks), class(&old, &marks), "{tag}");
    }
}

const SMALL: Chs = Chs::new(615, 4, 17);

fn annotations(store: &Store, tag: &str) -> std::collections::BTreeMap<String, String> {
    store
        .resolve(tag, &PcProvider)
        .unwrap()
        .manifest
        .annotations
}

#[test]
fn settings_revision_of_a_new_image_keeps_its_title_and_firmware_sources() {
    let Some(open) = Open::new() else {
        return;
    };
    let created = pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), None]),
        "lab:1.0.0",
        None,
    )
    .unwrap();
    let before = annotations(&open.store, "lab:1.0.0");
    assert_eq!(before[workspace::TITLE], "Open AT workbench");
    assert!(before.contains_key("unemu.firmware.sources"));
    let source = Source::open(&open.store, &created).unwrap();
    assert!(!source.stores_cmos());
    let plan = revision::plan(&open.store, &source, with_extended(&source, 2048)).unwrap();
    // Without a stored CMOS image Start derives it; the review still shows
    // which bytes change: expansion memory and the checksum.
    assert!(!plan.cmos_stored);
    assert!(offsets(&plan.cmos_changes)
        .iter()
        .all(|i| [0x17, 0x18, 0x2E, 0x2F].contains(i)));
    assert!(plan.added.is_empty(), "only the configuration is new");
    let old = open
        .store
        .resolve("lab:1.0.0", &PcProvider)
        .unwrap()
        .manifest;
    assert_eq!(plan.manifest().layers, old.layers);
    revision::publish(&open.store, &plan, "lab:1.1.0", None).unwrap();
    let after = annotations(&open.store, "lab:1.1.0");
    for key in [workspace::TITLE, "unemu.firmware.sources", "unemu.platform"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    assert_eq!(after[workspace::BASE_DIGEST], created.digest.to_string());
}

/// Write sectors to C: through the workspace journal of a prepared session.
fn write_c(prepared: &workspace::PreparedSession, sectors: &[(u64, [u8; 512])]) {
    let drive = prepared
        .machine
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    let assembly::ImageSource::File(path) = &drive.image else {
        panic!("expected a named base")
    };
    let assembly::WriteMode::Journal { path: journal, .. } = &drive.write else {
        panic!("expected a journal")
    };
    let base = prepared.bases[path].clone();
    let mut media = journal_files::JournalMedia::open_verified_with_metadata(
        journal,
        &base,
        base.byte_len(),
        Default::default(),
        None,
    )
    .unwrap();
    media.write_batch(sectors);
    media.finish().unwrap();
}

#[test]
fn a_save_without_changes_is_an_alias_of_the_same_image() {
    let Some(open) = Open::new() else {
        return;
    };
    let created = pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), None]),
        "lab:1.0.0",
        None,
    )
    .unwrap();
    let owner = open.store.workspace("idle").unwrap();
    drop(workspace::prepare_start(&owner, "lab:1.0.0", false).unwrap());
    let alias = workspace::publish_as(&owner, "lab:latest", None, false, None, |_| Ok(())).unwrap();
    assert_eq!(alias.digest, created.digest);
    let annotations = annotations(&open.store, "lab:latest");
    assert_eq!(annotations, self::annotations(&open.store, "lab:1.0.0"));
    assert!(!annotations.contains_key(workspace::BASE_DIGEST));
}

#[test]
fn a_pending_publication_finishes_with_its_own_tag() {
    let Some(open) = Open::new() else {
        return;
    };
    pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), None]),
        "lab:1.0.0",
        None,
    )
    .unwrap();
    let owner = open.store.workspace("crash").unwrap();
    let prepared = workspace::prepare_start(&owner, "lab:1.0.0", false).unwrap();
    write_c(&prepared, &[(0, [0x33; 512])]);
    drop(prepared);
    assert!(
        workspace::publish_with_hook(&owner, "lab-dos:1.0.0", None, |step| {
            if step == workspace::PublicationStep::IntentStored {
                Err(pc_image::Error::Invalid("injected interruption".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert_eq!(
        workspace::pending_publication_at(&open.store, "crash").unwrap(),
        Some(("lab-dos:1.0.0".into(), None))
    );
    let finished =
        workspace::publish_as(&owner, "lab-dos:1.0.0", None, false, None, |_| Ok(())).unwrap();
    assert_eq!(
        workspace::pending_publication_at(&open.store, "crash").unwrap(),
        None
    );
    assert_eq!(
        open.store.tag_descriptor("lab-dos:1.0.0").unwrap().digest,
        finished.digest
    );
}

#[test]
fn save_as_new_image_keeps_title_and_firmware_sources_and_can_retitle() {
    let Some(open) = Open::new() else {
        return;
    };
    pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), None]),
        "lab:1.0.0",
        None,
    )
    .unwrap();
    // A source that carries annotations of its own build, such as its
    // creation time; a copy describes another build and drops them.
    let mut manifest = open
        .store
        .resolve("lab:1.0.0", &PcProvider)
        .unwrap()
        .manifest;
    for key in [
        "org.opencontainers.image.created",
        "org.opencontainers.image.version",
    ] {
        manifest.annotations.insert(key.into(), "2026-11-15".into());
    }
    let created = open
        .store
        .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
        .unwrap();
    open.store
        .update_tag("lab:annotated", &created, None, &PcProvider)
        .unwrap();
    let before = annotations(&open.store, "lab:annotated");
    let owner = open.store.workspace("install").unwrap();
    let prepared = workspace::prepare_start(&owner, "lab:annotated", false).unwrap();
    write_c(&prepared, &[(0, [0x55; 512])]);
    drop(prepared);
    let saved =
        workspace::publish_as(&owner, "lab-dos:1.0.0", None, false, None, |_| Ok(())).unwrap();
    let after = annotations(&open.store, "lab-dos:1.0.0");
    for key in [workspace::TITLE, "unemu.firmware.sources", "unemu.platform"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    assert_eq!(after[workspace::BASE_DIGEST], created.digest.to_string());
    assert!(!after.contains_key("org.opencontainers.image.created"));
    assert!(!after.contains_key("org.opencontainers.image.version"));
    // The workspace now runs the saved image; a later save may retitle it.
    let prepared = workspace::prepare_start(&owner, "lab-dos:1.0.0", false).unwrap();
    write_c(&prepared, &[(1, [0xAA; 512])]);
    drop(prepared);
    workspace::publish_as(
        &owner,
        "lab-dos:1.1.0",
        None,
        false,
        Some("Open AT with FreeDOS"),
        |_| Ok(()),
    )
    .unwrap();
    let retitled = annotations(&open.store, "lab-dos:1.1.0");
    assert_eq!(retitled[workspace::TITLE], "Open AT with FreeDOS");
    assert_eq!(
        retitled["unemu.firmware.sources"],
        before["unemu.firmware.sources"]
    );
    assert_eq!(retitled[workspace::BASE_DIGEST], saved.digest.to_string());
    // A version tag never moves, not even from the confirmed digest.
    let prepared = workspace::prepare_start(&owner, "lab-dos:1.1.0", false).unwrap();
    write_c(&prepared, &[(2, [0x11; 512])]);
    drop(prepared);
    let current = open.store.tag_descriptor("lab-dos:1.1.0").unwrap().digest;
    let error = workspace::publish_as(&owner, "lab-dos:1.1.0", Some(&current), false, None, |_| {
        Ok(())
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("version tags never move"), "{error}");
}

/// Import the fixture ROMs `files` (vendor paths) as firmware packages, with
/// the source archive `sources` names for an open copyleft one.
fn import_roms(fixture: &Stored, files: &[(&str, Option<&str>)]) -> Option<Vec<Vec<u8>>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let catalogue = pc_image::firmware::catalogue();
    let mut roms = Vec::new();
    for (i, (file, source)) in files.iter().enumerate() {
        let bytes = fixtures::fixture(file)?;
        if let Some(source) = source {
            fixtures::fixture(source)?;
        }
        let folder = fixture.path.join(format!("roms-{i}"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("rom.bin"), &bytes).unwrap();
        let scan = pc_image::firmware_import::scan(&[folder], catalogue).unwrap();
        let statement = pc_image::firmware_import::Statement {
            source: source.map(|s| root.join("vendor").join(s)),
            ..Default::default()
        };
        pc_image::firmware_import::publish_with(
            &fixture.store,
            &scan.candidates[0],
            &[],
            &statement,
            catalogue,
        )
        .unwrap();
        roms.push(bytes);
    }
    Some(roms)
}
fn with_bios(source: &Source, bytes: &[u8]) -> Table {
    let mut machine = source.machine().clone();
    let sha256 = Digest::of(bytes).hex().to_owned();
    let bios = table(table(&mut machine, "firmware"), "bios");
    bios.insert(
        "image".into(),
        format!("assets/firmware-{}.bin", &sha256[..16]).into(),
    );
    bios.insert("sha256".into(), sha256.into());
    machine
}

#[test]
fn a_firmware_revision_binds_the_package_rom_and_resets_cmos_only_across_layouts() {
    let Some(fixture) = stored() else {
        return;
    };
    let Some(roms) = import_roms(
        &fixture,
        &[
            ("kiselev-bios/bios-xi8088-atreset.bin", None),
            (
                "unemu-at-bios/bios-unemu-at.bin",
                Some("unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar"),
            ),
        ],
    ) else {
        return;
    };
    let source = Source::open(&fixture.store, &fixture.image).unwrap();
    let firmware = |plan: &revision::Plan| -> Vec<Digest> {
        plan.added
            .iter()
            .filter(|d| {
                d.media_type != CMOS && d.decoded_media_type() != image_store::media_type::CONFIG
            })
            .map(|d| d.digest.clone())
            .collect()
    };

    // The same layout: the stored CMOS stays byte for byte.
    let plan = revision::plan(&fixture.store, &source, with_bios(&source, &roms[0])).unwrap();
    assert!(!plan.cmos_reset);
    assert!(plan.cmos_changes.is_empty());
    assert_eq!(firmware(&plan).len(), 1, "the new ROM blob");
    revision::publish(&fixture.store, &plan, "stored:1.1.0", None).unwrap();
    assert_eq!(cmos_blob(&fixture.store, "stored:1.1.0"), fixture.cmos);

    // Another layout: the revision stores the bytes its configuration
    // derives, and the open ROM travels with its source archive.
    let plan = revision::plan(&fixture.store, &source, with_bios(&source, &roms[1])).unwrap();
    assert!(plan.cmos_reset);
    assert_eq!(firmware(&plan).len(), 2, "the ROM and its source archive");
    revision::publish(&fixture.store, &plan, "stored:2.0.0", None).unwrap();
    let machine = pc_image::workspace::prepare_start(
        &fixture.store.workspace("inspect-2").unwrap(),
        "stored:2.0.0",
        false,
    )
    .unwrap()
    .machine;
    let bios = machine
        .images
        .iter()
        .find_map(|image| match &image.source {
            assembly::ImageSource::Inline(bytes) if image.base + bytes.len() as u64 == 0x100000 => {
                Some(bytes.clone())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(bios, roms[1]);
    let derived = assembly::derive_cmos_config(&machine).unwrap();
    let bytes = cmos_blob(&fixture.store, "stored:2.0.0");
    assert_eq!(bytes[0x10..], derived[0x10..]);
    assert_eq!(&bytes[..0x0E], &fixture.cmos[..0x0E], "the clock is kept");
    assert_eq!(bytes[0x0F], 0x0A, "the shutdown byte is kept");
    // The new revision names the package its new ROM came from.
    let sources = annotations(&fixture.store, "stored:2.0.0")
        .remove("unemu.firmware.sources")
        .unwrap();
    let package = pc_image::create::packages(&fixture.store)
        .unwrap()
        .into_iter()
        .find(|p| {
            p.config
                .roms
                .iter()
                .any(|r| r.blob.as_ref() == Some(&Digest::of(&roms[1])))
        })
        .unwrap();
    assert!(
        sources.contains(&package.descriptor.digest.to_string()),
        "{sources}"
    );
    // The source and its tag never change.
    assert_eq!(cmos_blob(&fixture.store, "stored:1.0.0"), fixture.cmos);
    assert_eq!(
        fixture.store.tag_descriptor("stored:1.0.0").unwrap().digest,
        fixture.image.digest
    );
}
