// SPDX-License-Identifier: AGPL-3.0-or-later
//! Blank fixed disks as zero bases: identity without data, reads, journals,
//! sparse layers, flattening, redistribution and sizes; machine-images §§2, 4.

use image_store::{media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest, Store};
use media::SectorSource;
use pc_image::{create::Chs, workspace, PcProvider, DISK_ZERO};
#[path = "support/open_image.rs"]
mod open_image;
use open_image::Open;

const SMALL: Chs = Chs::new(615, 4, 17);

fn layers(store: &Store, tag: &str) -> Vec<Descriptor> {
    store.resolve(tag, &PcProvider).unwrap().manifest.layers
}
fn zero(layers: &[Descriptor]) -> Descriptor {
    let found: Vec<_> = layers
        .iter()
        .filter(|d| d.media_type == DISK_ZERO)
        .cloned()
        .collect();
    assert_eq!(found.len(), 1);
    found[0].clone()
}
/// The verified reader and journal path of the IDE master.
fn fixed_disk(
    prepared: &workspace::PreparedSession,
) -> (media::VerifiedBase, std::path::PathBuf, std::path::PathBuf) {
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
    (prepared.bases[path].clone(), path.clone(), journal.clone())
}
fn write(prepared: &workspace::PreparedSession, sectors: &[(u64, [u8; 512])]) {
    let (base, _, journal) = fixed_disk(prepared);
    let mut media = media::JournalMedia::open_verified_with_metadata(
        &journal,
        &base,
        base.byte_len(),
        Default::default(),
        None,
    )
    .unwrap();
    media.write_batch(sectors);
    media.finish().unwrap();
}
fn no_materialized_disk(store: &Store) {
    assert!(!store.root().join("cache/materialized").exists());
    assert!(!store.root().join("cache/unmaterialized").exists());
}

#[test]
fn blank_disk_resolves_from_a_descriptor_and_reads_zeros() {
    let Some(open) = Open::new() else {
        return;
    };
    let image = pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), None]),
        "lab:1.0.0",
        None,
    )
    .unwrap();
    let layers = layers(&open.store, "lab:1.0.0");
    let zero = zero(&layers);
    assert!(zero.size < 256, "{}", zero.size);
    assert_eq!(zero.annotations["unemu.redistribution"], "open");
    let owner = open.store.workspace("play").unwrap();
    let prepared = workspace::prepare_start(&owner, "lab:1.0.0", false).unwrap();
    assert_eq!(prepared.image, image.digest);
    let (base, path, _) = fixed_disk(&prepared);
    assert_eq!(base.byte_len(), SMALL.bytes());
    let identity: String = base
        .identity()
        .base_digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(identity, pc_image::zero::zero_digest(SMALL.bytes()).hex());
    for lba in [0, 1, 41_819] {
        assert_eq!(base.read_sector(lba).unwrap(), [0; 512]);
    }
    assert!(!path.exists(), "a zero base is never written to the host");
    no_materialized_disk(&open.store);
    // The drive pins the decoded identity, the SHA-256 of that many zero bytes.
    let drive = prepared
        .machine
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    assert_eq!(drive.expected_sha256, Some(base.identity().base_digest));
    assert_eq!(
        Digest::of(&vec![0; SMALL.bytes() as usize]),
        pc_image::zero::zero_digest(SMALL.bytes())
    );
    // A thread transfer keeps the zero-rooted head without any file.
    let restored = prepared.into_transfer().unwrap().restore().unwrap();
    let (base, _, _) = fixed_disk(&restored);
    assert_eq!(base.read_sector(7).unwrap(), [0; 512]);
}

#[test]
fn writes_journal_then_save_seals_only_written_sectors_and_flatten_stays_sparse() {
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
    let zero_digest = zero(&layers(&open.store, "lab:1.0.0")).digest;
    let owner = open.store.workspace("install").unwrap();
    let prepared = workspace::prepare_start(&owner, "lab:1.0.0", false).unwrap();
    write(&prepared, &[(0, [0x55; 512]), (1000, [0xAA; 512])]);
    drop(prepared);
    workspace::publish(&owner, "lab-dos:1.0.0", None).unwrap();
    let saved = layers(&open.store, "lab-dos:1.0.0");
    assert_eq!(zero(&saved).digest, zero_digest, "the zero base is shared");
    let pack = saved
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_SECTORS)
        .unwrap();
    assert_eq!(
        pack.decoded_size().unwrap(),
        2 * media::layer::RECORD_LEN,
        "only written sectors"
    );
    let layer = saved
        .iter()
        .find(|d| d.media_type == mt::DISK_LAYER)
        .unwrap();
    let layer =
        media::layer::Descriptor::decode(&open.store.read_verified(layer, 4096).unwrap()).unwrap();
    assert_eq!(layer.parent, zero_digest.to_string());
    assert_eq!(layer.base, zero_digest.to_string());

    // The saved head reads straight from its chain; nothing is materialized.
    let owner = open.store.workspace("again").unwrap();
    let prepared = workspace::prepare_start(&owner, "lab-dos:1.0.0", false).unwrap();
    let (base, _, _) = fixed_disk(&prepared);
    assert_eq!(base.read_sector(0).unwrap(), [0x55; 512]);
    assert_eq!(base.read_sector(1000).unwrap(), [0xAA; 512]);
    assert_eq!(base.read_sector(999).unwrap(), [0; 512]);
    no_materialized_disk(&open.store);
    let restored = prepared.into_transfer().unwrap().restore().unwrap();
    assert_eq!(
        fixed_disk(&restored).0.read_sector(1000).unwrap(),
        [0xAA; 512]
    );
    drop(restored);

    // Flattening a zero-rooted chain gives one layer over the zero base.
    write(
        &workspace::prepare_start(&owner, "lab-dos:1.0.0", false).unwrap(),
        &[(2, [0x11; 512])],
    );
    workspace::publish_with_options(&owner, "lab-flat:1.0.0", None, true, |_| Ok(())).unwrap();
    let flat = layers(&open.store, "lab-flat:1.0.0");
    assert_eq!(
        flat.iter()
            .filter(|d| d.media_type == mt::DISK_LAYER)
            .count(),
        1
    );
    assert!(!flat.iter().any(|d| d.decoded_media_type() == mt::DISK_BASE));
    let pack = flat
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_SECTORS)
        .unwrap();
    assert_eq!(pack.decoded_size().unwrap(), 3 * media::layer::RECORD_LEN);
    let layer = flat
        .iter()
        .find(|d| d.media_type == mt::DISK_LAYER)
        .unwrap();
    let layer =
        media::layer::Descriptor::decode(&open.store.read_verified(layer, 4096).unwrap()).unwrap();
    assert_eq!(layer.parent, zero_digest.to_string());

    // Writing zeros back makes the flattened head the zero base itself.
    let owner = open.store.workspace("erase").unwrap();
    write(
        &workspace::prepare_start(&owner, "lab-flat:1.0.0", false).unwrap(),
        &[(0, [0; 512]), (2, [0; 512]), (1000, [0; 512])],
    );
    workspace::publish_with_options(&owner, "lab-erased:1.0.0", None, true, |_| Ok(())).unwrap();
    let erased = layers(&open.store, "lab-erased:1.0.0");
    assert_eq!(zero(&erased).digest, zero_digest);
    assert!(!erased
        .iter()
        .any(|d| matches!(d.decoded_media_type(), mt::DISK_LAYER | mt::DISK_SECTORS)));
    no_materialized_disk(&open.store);
}

#[test]
fn guard_summary_and_inspection_see_a_small_open_blank_disk() {
    let Some(open) = Open::new() else {
        return;
    };
    let image = pc_image::create::create(
        &open.store,
        &open.image([Some(Chs::new(1024, 16, 63)), None]),
        "open-at:1.0.0",
        None,
    )
    .unwrap();
    let manifest = open
        .store
        .resolve("open-at:1.0.0", &PcProvider)
        .unwrap()
        .manifest;
    let marks = image_registry::guard::LocalMarks::scan(&open.store).unwrap();
    let closure = image_registry::guard::closure(&manifest, &marks);
    assert!(closure
        .iter()
        .all(|c| c.effective == image_registry::guard::Redistribution::Open));
    let official =
        image_registry::Reference::parse("open-at:1.0.0", image_registry::DEFAULT_REGISTRY)
            .unwrap();
    assert_eq!(
        image_registry::guard::check(
            Some(&image_registry::RegistryEntry::builtin(
                image_registry::DEFAULT_REGISTRY
            )),
            &official,
            &closure
        ),
        Ok(())
    );
    let mut cache = image_store::summary::SummaryCache::open(&open.store).unwrap();
    while !cache.pending().is_empty() {
        cache
            .rebuild(&open.store, 64, &[], pc_image::PAYLOAD_TYPES)
            .unwrap();
    }
    let summary = cache.summary(&image.digest).unwrap();
    assert_eq!(summary.name, "Open AT workbench");
    // Each copyleft ROM travels with its package's source archive, linked.
    let archives: Vec<_> = manifest
        .layers
        .iter()
        .filter(|d| d.decoded_media_type() == mt::TAR)
        .collect();
    assert_eq!(archives.len(), 3, "Kiselev, VGA BIOS and XTIDE sources");
    assert!(manifest
        .layers
        .iter()
        .all(|d| !image_store::license::source_missing(d, &manifest.layers)));
    // Firmware and source blobs are shared with their packages; the disk adds
    // a descriptor.
    let firmware: u64 = manifest
        .layers
        .iter()
        .filter(|d| matches!(d.decoded_media_type(), mt::FIRMWARE | mt::TAR))
        .map(|d| d.size)
        .sum();
    assert!(
        summary.stored_size - firmware < 8 * 1024,
        "{} stored besides firmware",
        summary.stored_size - firmware
    );
    for details in [
        pc_image::inspection::inspect(&open.store, &image, None).unwrap(),
        pc_image::desktop::check(&open.store, &image, None, || false).unwrap(),
    ] {
        let disk = details
            .media
            .iter()
            .find(|m| m.chain.first().is_some_and(|c| c.blank_bytes.is_some()))
            .unwrap();
        assert_eq!(disk.chain.len(), 1);
        assert_eq!(disk.chain[0].blank_bytes, Some(1024 * 16 * 63 * 512));
        assert!(disk.chain[0].size < 256);
    }
}

#[test]
fn a_blank_disk_mounts_only_through_a_journal() {
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
    let image = open.store.resolve("lab:1.0.0", &PcProvider).unwrap();
    let text = std::str::from_utf8(&image.config).unwrap();
    assert!(text.contains("policy = \"journal\""));
    for policy in ["volatile", "readonly"] {
        let table: toml::Table = text
            .replace("policy = \"journal\"", &format!("policy = \"{policy}\""))
            .parse()
            .unwrap();
        let error = PcProvider
            .validate(&table, &image.manifest.layers, &open.store)
            .unwrap_err();
        assert!(
            error.to_string().contains("only through a journal"),
            "{error}"
        );
    }
    // A zero base whose geometry differs from the drive's is refused.
    let table: toml::Table = text
        .replace("cylinders = 615", "cylinders = 614")
        .parse()
        .unwrap();
    assert!(PcProvider
        .validate(&table, &image.manifest.layers, &open.store)
        .is_err());
}

#[test]
fn an_imported_machine_file_does_not_make_a_created_open_image_private() {
    use image_registry::guard::{self, Redistribution};
    let Some(open) = Open::new() else {
        return;
    };
    // A machine file with another open BIOS build: import marks the user's
    // ROM user-supplied and its generated configuration and metadata open.
    let Some(bios) = open_image::fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    std::fs::write(open.path.join("xi8088.bin"), bios).unwrap();
    let file = open.path.join("imported.toml");
    std::fs::write(
        &file,
        "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='xi8088.bin'\n",
    )
    .unwrap();
    let imported =
        pc_image::import_machine(&open.store, &file, "imported:1.0.0", &Default::default())
            .unwrap()
            .manifest;
    let created = pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), None]),
        "open-at:1.0.0",
        None,
    )
    .unwrap();
    let marks = guard::LocalMarks::scan(&open.store).unwrap();
    let manifest = |d: &Descriptor| {
        image_store::Manifest::decode_for_summary(&open.store.read_verified(d, 1 << 20).unwrap())
            .unwrap()
    };
    let closure = guard::closure(&manifest(&created), &marks);
    assert!(
        closure.iter().all(|c| c.effective == Redistribution::Open),
        "{closure:?}"
    );
    let official =
        image_registry::Reference::parse("open-at:1.0.0", image_registry::DEFAULT_REGISTRY)
            .unwrap();
    assert_eq!(
        guard::check(
            Some(&image_registry::RegistryEntry::builtin(
                image_registry::DEFAULT_REGISTRY
            )),
            &official,
            &closure
        ),
        Ok(())
    );
    for entry in guard::closure(&manifest(&imported), &marks) {
        let expected = match entry.descriptor.media_type.as_str() {
            mt::CONFIG | mt::METADATA => Redistribution::Open,
            _ => Redistribution::UserSupplied,
        };
        assert_eq!(entry.effective, expected, "{entry}");
    }
}

#[test]
fn disks_of_one_geometry_share_one_zero_base_and_keep_separate_journals() {
    let Some(open) = Open::new() else {
        return;
    };
    pc_image::create::create(
        &open.store,
        &open.image([Some(SMALL), Some(SMALL)]),
        "two:1.0.0",
        None,
    )
    .unwrap();
    // `zero` asserts exactly one zero base descriptor for both disks.
    let shared = zero(&layers(&open.store, "two:1.0.0")).digest;
    let owner = open.store.workspace("two").unwrap();
    let prepared = workspace::prepare_start(&owner, "two:1.0.0", false).unwrap();
    let disks: Vec<_> = prepared
        .machine
        .drives
        .iter()
        .filter(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .map(|d| {
            let assembly::ImageSource::File(path) = &d.image else {
                panic!("expected a named base")
            };
            let assembly::WriteMode::Journal { path: journal, .. } = &d.write else {
                panic!("expected a journal")
            };
            (prepared.bases[path].clone(), journal.clone())
        })
        .collect();
    assert_eq!(disks.len(), 2);
    assert_ne!(disks[0].1, disks[1].1, "each disk has its own journal");
    let mut media = media::JournalMedia::open_verified_with_metadata(
        &disks[0].1,
        &disks[0].0,
        disks[0].0.byte_len(),
        Default::default(),
        None,
    )
    .unwrap();
    media.write_batch(&[(0, [0x77; 512])]);
    media.finish().unwrap();
    drop(prepared);
    workspace::publish(&owner, "two-written:1.0.0", None).unwrap();
    let image = open
        .store
        .resolve("two-written:1.0.0", &PcProvider)
        .unwrap();
    let config: toml::Table = std::str::from_utf8(&image.config).unwrap().parse().unwrap();
    let heads: Vec<_> = config["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["head"].as_str().unwrap().to_owned())
        .collect();
    assert_ne!(heads[0], shared.to_string(), "C: has a layer now");
    assert_eq!(heads[1], shared.to_string(), "D: is still the zero base");
    let owner = open.store.workspace("two-again").unwrap();
    let prepared = workspace::prepare_start(&owner, "two-written:1.0.0", false).unwrap();
    let (c, _, _) = fixed_disk(&prepared);
    assert_eq!(c.read_sector(0).unwrap(), [0x77; 512]);
}

#[test]
fn the_created_configuration_lowers_like_the_equivalent_machine_file() {
    let Some(open) = Open::new() else {
        return;
    };
    let image = open.image([Some(SMALL), None]);
    pc_image::create::create(&open.store, &image, "created:1.0.0", None).unwrap();
    // The same document as a machine file: real ROM paths, an all-zero raw
    // disk on a journal and an empty floppy catalogue.
    let mut document = image.document().unwrap();
    let roms = ["bios.bin", "vga.bin", "xtide.bin"];
    let firmware = document["firmware"].as_table_mut().unwrap();
    firmware["bios"].as_table_mut().unwrap().insert(
        "image".into(),
        open.path
            .join("roms")
            .join(roms[0])
            .display()
            .to_string()
            .into(),
    );
    for (rom, name) in firmware["option_roms"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(&roms[1..])
    {
        rom.as_table_mut().unwrap().insert(
            "image".into(),
            open.path
                .join("roms")
                .join(name)
                .display()
                .to_string()
                .into(),
        );
    }
    std::fs::write(open.path.join("blank.img"), vec![0; SMALL.bytes() as usize]).unwrap();
    for card in document["card"].as_array_mut().unwrap() {
        let kind = card["kind"].as_str().unwrap().to_owned();
        for drive in card
            .get_mut("drives")
            .and_then(toml::Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let drive = drive.as_table_mut().unwrap();
            if kind == "ide" {
                drive.insert("image".into(), "blank.img".into());
                drive.insert("write".into(), "journal".into());
                drive.insert("journal".into(), "blank.upfj".into());
            } else {
                drive.insert("media".into(), toml::Value::Array(Vec::new()));
                drive.insert("empty".into(), true.into());
            }
        }
    }
    let file = open.path.join("equivalent.toml");
    std::fs::write(&file, toml::to_string(&document).unwrap()).unwrap();
    pc_image::import_machine(&open.store, &file, "imported:1.0.0", &Default::default()).unwrap();
    let normalize = |mut c: assembly::MachineConfig| {
        for drive in &mut c.drives {
            drive.image = assembly::ImageSource::Blank {
                size: drive.kind.image_bytes(),
            };
            if let assembly::WriteMode::Journal { path, .. } = &mut drive.write {
                *path = "journal".into();
            }
        }
        c
    };
    let created = pc_image::resolve(&open.store, "created:1.0.0", &open.path.join("a")).unwrap();
    let imported = pc_image::resolve(&open.store, "imported:1.0.0", &open.path.join("b")).unwrap();
    // Identities agree too: the zero base pins the SHA-256 of the raw zeros.
    assert_eq!(
        created
            .drives
            .iter()
            .map(|d| d.expected_sha256)
            .collect::<Vec<_>>(),
        imported
            .drives
            .iter()
            .map(|d| d.expected_sha256)
            .collect::<Vec<_>>()
    );
    assert_eq!(normalize(created), normalize(imported));
}

#[test]
fn a_compressed_sector_pack_over_a_zero_base_mounts_without_any_decoded_file() {
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
    let owner = open.store.workspace("install").unwrap();
    let prepared = workspace::prepare_start(&owner, "lab:1.0.0", false).unwrap();
    // Sixteen sectors make a pack over 4 KiB, which publication compresses.
    let sectors: Vec<_> = (0..16u64)
        .map(|lba| (lba * 3, [lba as u8 + 1; 512]))
        .collect();
    write(&prepared, &sectors);
    drop(prepared);
    workspace::publish(&owner, "lab-dos:1.0.0", None).unwrap();
    let saved = layers(&open.store, "lab-dos:1.0.0");
    let pack = saved
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_SECTORS)
        .unwrap();
    assert!(pack.is_compressed(), "{pack:?}");
    assert!(!zero(&saved).is_compressed());
    // Resolution streams the chain over its zero base and writes nothing.
    no_materialized_disk(&open.store);
    let owner = open.store.workspace("play").unwrap();
    let prepared = workspace::prepare_start(&owner, "lab-dos:1.0.0", false).unwrap();
    let (base, _, _) = fixed_disk(&prepared);
    for (lba, bytes) in &sectors {
        assert_eq!(base.read_sector(*lba).unwrap(), *bytes);
    }
    assert_eq!(base.read_sector(1).unwrap(), [0; 512]);
    no_materialized_disk(&open.store);
    let restored = prepared.into_transfer().unwrap().restore().unwrap();
    assert_eq!(fixed_disk(&restored).0.read_sector(45).unwrap(), [16; 512]);
    drop(restored);
    // Flatten keeps one sparse layer over the same zero base.
    workspace::publish_with_options(&owner, "lab-flat:1.0.0", None, true, |_| Ok(())).unwrap();
    let flat = layers(&open.store, "lab-flat:1.0.0");
    let pack = flat
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_SECTORS)
        .unwrap();
    assert_eq!(pack.decoded_size().unwrap(), 16 * media::layer::RECORD_LEN);
    assert_eq!(zero(&flat).digest, zero(&saved).digest);
    no_materialized_disk(&open.store);
}

/// The push guard's verdict for `tag` in the official namespace.
fn official(store: &Store, tag: &str) -> Result<(), image_registry::guard::Refusal> {
    let manifest = store.resolve(tag, &PcProvider).unwrap().manifest;
    let marks = image_registry::guard::LocalMarks::scan(store).unwrap();
    let closure = image_registry::guard::closure(&manifest, &marks);
    let target = image_registry::Reference::parse(tag, image_registry::DEFAULT_REGISTRY).unwrap();
    image_registry::guard::check(
        Some(&image_registry::RegistryEntry::builtin(
            image_registry::DEFAULT_REGISTRY,
        )),
        &target,
        &closure,
    )
}

#[test]
fn a_copyleft_rom_without_its_source_archive_keeps_a_new_image_private() {
    let Some(open) = Open::new() else {
        return;
    };
    // The same open AT BIOS, from a package another program wrote: open
    // under the GPL, but without its source archive.
    let original = open.rom("unemu-at-bios-1.0.2-unemu.3");
    let package = &original.package;
    let mut config = package.config.clone();
    config.sources.clear();
    let text = config.encode().unwrap();
    let blob = open.store.import(text.as_slice(), None).unwrap();
    let mut descriptor = Descriptor::new(mt::FIRMWARE_CONFIG, blob.digest, blob.size);
    descriptor.annotations = package.manifest.config.annotations.clone();
    let layers = package
        .manifest
        .layers
        .iter()
        .filter(|d| d.decoded_media_type() != mt::TAR)
        .map(|d| {
            let mut d = d.clone();
            d.annotations.remove(image_store::license::SOURCE);
            d
        })
        .collect();
    let crafted = image_store::Manifest::new_firmware("ibm-pc", descriptor, layers).unwrap();
    let crafted = open.store.import_manifest(&crafted).unwrap();
    let rules = pc_image::firmware::PcFirmwareRules(pc_image::firmware::catalogue());
    open.store
        .update_tag(
            "private/firmware/ibm-pc/foreign-at-bios:r1",
            &crafted,
            None,
            &image_store::firmware::Validator(&rules),
        )
        .unwrap();
    let foreign = pc_image::create::packages(&open.store)
        .unwrap()
        .into_iter()
        .find(|p| p.descriptor.digest == crafted.digest)
        .unwrap();
    let mut image = open.image([None, None]);
    image.firmware[0] = pc_image::create::PackageRom::new(&foreign, &foreign.config.roms[0]);
    let preview = pc_image::create::preview(&image, &open.store).unwrap();
    pc_image::create::create(&open.store, &image, "foreign-at:1.0.0", None).unwrap();
    let created = open.store.resolve("foreign-at:1.0.0", &PcProvider).unwrap();
    assert_eq!(
        preview, created.manifest,
        "the preview is what Create writes"
    );
    let bios = foreign.config.roms[0].blob.clone().unwrap();
    let rom = created
        .manifest
        .layers
        .iter()
        .find(|d| d.names(&bios))
        .unwrap();
    assert!(image_store::license::source_missing(
        rom,
        &created.manifest.layers
    ));
    assert!(matches!(
        official(&open.store, "foreign-at:1.0.0"),
        Err(image_registry::guard::Refusal::CopyleftWithoutSource { ref blobs, .. }) if blobs[0].digest == rom.digest
    ));
    // The same ROM from the package with its source: the image may enter unemu/.
    pc_image::create::create(
        &open.store,
        &open.image([None, None]),
        "open-at:1.0.0",
        None,
    )
    .unwrap();
    assert_eq!(official(&open.store, "open-at:1.0.0"), Ok(()));
}
