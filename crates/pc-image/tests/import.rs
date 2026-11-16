// SPDX-License-Identifier: AGPL-3.0-or-later
//! Open-fixture and synthetic journal coverage; machine-images §§2 and 8.

use image_store::{BlobSource, ConfigValidator, Digest, Store};
use pc_image::{import_machine, resolve, ImportOptions, PcProvider, RecoveryChoice};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pc-image-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> Store {
        Store::open(self.0.join("store")).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn recovery(id: &str, choice: RecoveryChoice) -> ImportOptions {
    ImportOptions {
        recovery: [(id.to_owned(), choice)].into(),
        ..Default::default()
    }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
/// A machine file on the fetched open BIOS; `None` when that fixture is missing.
fn fixture(temp: &Temp, extra: &str) -> Option<PathBuf> {
    let bios = fixtures::fixture("kiselev-bios/bios-xi8088.bin")?;
    fs::write(temp.0.join("bios.bin"), bios).unwrap();
    let path = temp.0.join("machine.toml");
    fs::write(
        &path,
        format!("schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n{extra}"),
    )
    .unwrap();
    Some(path)
}
fn floppy(journal: bool) -> String {
    format!("\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='disk.img'\nwrite='{}'\n{}", if journal { "journal" } else { "volatile" }, if journal { "journal='disk.upfj'\nkeep_generations=3" } else { "" })
}
fn make_journal(temp: &Temp, cmos: bool, closed: bool) -> Vec<u8> {
    let base = vec![0; if cmos { 64 } else { 360 * 1024 }];
    fs::write(
        temp.0.join(if cmos { "cmos.bin" } else { "disk.img" }),
        &base,
    )
    .unwrap();
    let path = temp.0.join(if cmos { "cmos.upfj" } else { "disk.upfj" });
    let mut journal =
        media::JournalMedia::open(&path, &base, base.len() as u64, Default::default()).unwrap();
    let mut sector = [0; 512];
    if cmos {
        sector[32] = 0xA7;
    } else {
        sector.fill(0xA7);
    }
    journal.write_batch(&[(0, sector)]);
    if closed {
        journal.finish().unwrap();
    } else {
        drop(journal);
    }
    base
}
fn first_bytes(store: &Store, tag: &str, kind: &str) -> Vec<u8> {
    let image = store.resolve(tag, &PcProvider).unwrap();
    let d = image
        .manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == kind)
        .unwrap();
    store.read_verified(d, 16 * 1024 * 1024).unwrap()
}
#[test]
fn clean_journal_freezes_generation_and_preserves_originals() {
    let temp = Temp::new();
    let base = make_journal(&temp, false, true);
    let Some(path) = fixture(&temp, &floppy(true)) else {
        return;
    };
    let before = fs::read(temp.0.join("disk.upfj")).unwrap();
    let source = fs::read(&path).unwrap();
    let store = temp.store();
    let record = import_machine(&store, &path, "test:clean", &ImportOptions::default()).unwrap();
    assert_eq!(record.generations[0].generation, Some(1));
    assert!(!record.generations[0].recovered);
    assert_eq!(
        &first_bytes(&store, "test:clean", image_store::media_type::FLOPPY)[..512],
        &[0xA7; 512]
    );
    assert_eq!(fs::read(temp.0.join("disk.img")).unwrap(), base);
    assert_eq!(fs::read(temp.0.join("disk.upfj")).unwrap(), before);
    assert_eq!(fs::read(path).unwrap(), source);
}
#[test]
fn interrupted_tail_requires_choice_and_recovers_only_private_copy() {
    let temp = Temp::new();
    let base = make_journal(&temp, false, false);
    let Some(path) = fixture(&temp, &floppy(true)) else {
        return;
    };
    let journal = temp.0.join("disk.upfj");
    let mut bytes = fs::read(&journal).unwrap();
    bytes.extend([0x55; 17]);
    fs::write(&journal, &bytes).unwrap();
    let store = temp.store();
    assert!(
        import_machine(&store, &path, "test:no-choice", &ImportOptions::default())
            .unwrap_err()
            .to_string()
            .contains("explicit recovery choice")
    );
    assert!(store.list_tags(None, 100).unwrap().tags.is_empty());
    let record = import_machine(
        &store,
        &path,
        "test:recover",
        &recovery("medium-0", RecoveryChoice::Recover),
    )
    .unwrap();
    assert_eq!(record.generations[0].generation, Some(1));
    assert!(record.generations[0].recovered);
    assert_eq!(
        &first_bytes(&store, "test:recover", image_store::media_type::FLOPPY)[..512],
        &[0xA7; 512]
    );
    let closed = import_machine(
        &store,
        &path,
        "test:base",
        &recovery("medium-0", RecoveryChoice::Generation(0)),
    )
    .unwrap();
    assert_eq!(closed.generations[0].generation, Some(0));
    assert_eq!(
        first_bytes(&store, "test:base", image_store::media_type::FLOPPY),
        base
    );
    assert_eq!(fs::read(&journal).unwrap(), bytes);
    assert_eq!(fs::read(temp.0.join("disk.img")).unwrap(), base);
    assert!(fs::read_dir(store.root().join("staging"))
        .unwrap()
        .next()
        .is_none());
}
#[test]
fn interior_corruption_and_wrong_original_base_block_import() {
    let temp = Temp::new();
    let mut base = make_journal(&temp, false, true);
    let Some(path) = fixture(&temp, &floppy(true)) else {
        return;
    };
    let journal = temp.0.join("disk.upfj");
    let original = fs::read(&journal).unwrap();
    let mut damaged = original.clone();
    damaged[media::HEADER_LEN + 100] ^= 1;
    fs::write(&journal, &damaged).unwrap();
    let store = temp.store();
    assert!(import_machine(
        &store,
        &path,
        "test:corrupt",
        &recovery("medium-0", RecoveryChoice::Recover)
    )
    .unwrap_err()
    .to_string()
    .contains("refused"));
    assert_eq!(fs::read(&journal).unwrap(), damaged);
    fs::write(&journal, &original).unwrap();
    base[0] ^= 1;
    fs::write(temp.0.join("disk.img"), &base).unwrap();
    assert!(
        import_machine(&store, &path, "test:wrong-base", &ImportOptions::default())
            .unwrap_err()
            .to_string()
            .contains("BaseDigest")
    );
    assert!(store.list_tags(None, 100).unwrap().tags.is_empty());
}
#[test]
fn cmos_freezes_and_resolves_with_session_local_journal() {
    let temp = Temp::new();
    let base = make_journal(&temp, true, true);
    let Some(path) = fixture(
        &temp,
        "\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='cmos.upfj'\n",
    ) else {
        return;
    };
    let original = fs::read(temp.0.join("cmos.upfj")).unwrap();
    let store = temp.store();
    let record = import_machine(&store, &path, "test:cmos", &ImportOptions::default()).unwrap();
    assert_eq!(record.generations[0].medium, "cmos");
    assert_eq!(record.generations[0].generation, Some(1));
    let bytes = first_bytes(&store, "test:cmos", pc_image::CMOS);
    assert_eq!(bytes.len(), 64);
    assert_eq!(bytes[32], 0xA7);
    let config = resolve(&store, "test:cmos", &temp.0.join("session")).unwrap();
    assert!(config.devices.iter().any(|d| matches!(&d.kind, assembly::DeviceKind::Rtc(assembly::CmosCfg { nvram: assembly::NvramSource::Journal { journal, .. }, .. }) if journal == &temp.0.join("session/cmos.upfj"))));
    assert_eq!(fs::read(temp.0.join("cmos.bin")).unwrap(), base);
    assert_eq!(fs::read(temp.0.join("cmos.upfj")).unwrap(), original);
}
#[test]
fn deterministic_import_deduplicates_and_is_self_contained() {
    let temp = Temp::new();
    fs::write(temp.0.join("disk.img"), vec![0; 360 * 1024]).unwrap();
    let Some(path) = fixture(&temp, &floppy(false)) else {
        return;
    };
    let store = temp.store();
    let a = import_machine(&store, &path, "test:a", &ImportOptions::default()).unwrap();
    let count = fs::read_dir(store.root().join("images/blobs/sha256"))
        .unwrap()
        .count();
    let b = import_machine(&store, &path, "test:b", &ImportOptions::default()).unwrap();
    assert_eq!(a.manifest.digest, b.manifest.digest);
    assert_eq!(
        count,
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count()
    );
    fs::remove_file(temp.0.join("bios.bin")).unwrap();
    fs::remove_file(temp.0.join("disk.img")).unwrap();
    fs::remove_file(path).unwrap();
    let config = resolve(&store, "test:a", &temp.0.join("session")).unwrap();
    let mut assembled = assembly::assemble(&config).unwrap();
    assembled.finish_persistence().unwrap();
    assert!(import_machine(
        &store,
        &temp.0.join("missing.toml"),
        "test:a",
        &ImportOptions::default()
    )
    .is_err());
}
#[test]
fn missing_inputs_are_listed_together() {
    let temp = Temp::new();
    let Some(path) = fixture(&temp, &floppy(false)) else {
        return;
    };
    fs::remove_file(temp.0.join("bios.bin")).unwrap();
    let error = import_machine(
        &temp.store(),
        &path,
        "test:missing",
        &ImportOptions::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("missing inputs"));
    assert!(error.contains("bios.bin"));
    assert!(error.contains("disk.img"));
}
fn normalize(mut c: assembly::MachineConfig) -> assembly::MachineConfig {
    fn source(image: &mut assembly::ImageSource, pin: &mut Option<[u8; 32]>) {
        if let assembly::ImageSource::File(path) = image {
            assert!(
                pin.is_some(),
                "file-backed media must retain a full SHA-256 pin"
            );
            *image = assembly::ImageSource::Inline(fs::read(path).unwrap());
        }
        if let assembly::ImageSource::Inline(bytes) = image {
            let digest = Digest::of(bytes);
            let hash: [u8; 32] = (0..32)
                .map(|i| u8::from_str_radix(&digest.hex()[i * 2..i * 2 + 2], 16).unwrap())
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            if let Some(pin) = pin {
                assert_eq!(*pin, hash);
            } else {
                *pin = Some(hash);
            }
        }
    }
    for drive in &mut c.drives {
        source(&mut drive.image, &mut drive.expected_sha256);
        if let Some(list) = &mut drive.media {
            for m in &mut list.entries {
                source(&mut m.image, &mut m.expected_sha256);
            }
        }
    }
    c
}
/// The fetched fixtures a shipped machine file's firmware and drives name,
/// relative to `vendor/`; shipped files sit next to it, in `machines/`.
fn vendor_fixtures(doc: &machine_file::MachineFile) -> Vec<String> {
    let firmware = doc
        .firmware
        .iter()
        .flat_map(|f| f.bios.iter().chain(f.option_roms.iter().flatten()))
        .flat_map(|rom| {
            rom.image
                .iter()
                .chain(rom.parts.iter().flatten().map(|p| &p.image))
        });
    let drives = doc
        .card
        .iter()
        .flatten()
        .flat_map(|c| c.drives.iter().flatten())
        .flat_map(|d| {
            d.image
                .iter()
                .chain(d.media.iter().flatten().map(|m| &m.image))
        });
    firmware
        .chain(drives)
        .filter_map(|image| image.strip_prefix("../vendor/"))
        .map(str::to_owned)
        .collect()
}
#[test]
fn shipped_open_machine_files_round_trip_effective_assembly_input() {
    let temp = Temp::new();
    let store = temp.store();
    let mut tested = Vec::new();
    let mut files = fs::read_dir(root().join("machines"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    files.sort();
    for path in files {
        if !matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("toml" | "yaml")
        ) {
            continue;
        }
        let (doc, _) = machine_file::resolve(&path).unwrap();
        let user_roms = doc
            .firmware
            .iter()
            .flat_map(|f| f.bios.iter().chain(f.option_roms.iter().flatten()))
            .any(|rom| {
                rom.image
                    .as_deref()
                    .is_some_and(|s| s.contains("user-roms"))
                    || rom
                        .parts
                        .iter()
                        .flatten()
                        .any(|p| p.image.contains("user-roms"))
            });
        let local_media = doc
            .card
            .iter()
            .flatten()
            .flat_map(|c| c.drives.iter().flatten())
            .any(|d| {
                d.image.as_deref().is_some_and(|s| s.starts_with("local/"))
                    || d.media
                        .iter()
                        .flatten()
                        .any(|m| m.image.starts_with("local/"))
            });
        if user_roms || local_media {
            eprintln!(
                "skipping machine requiring user inputs: {}",
                path.file_name().unwrap().to_string_lossy()
            );
            continue;
        }
        // Every open machine must round-trip, so one missing fetched fixture
        // skips the whole test rather than leaving fewer machines tested.
        if !vendor_fixtures(&doc)
            .iter()
            .all(|relative| fixtures::fixture(relative).is_some())
        {
            return;
        }
        let original = machine_file::load(&path).unwrap();
        let tag = format!("shipped:t{}", tested.len());
        import_machine(&store, &path, &tag, &ImportOptions::default()).unwrap();
        assert_eq!(
            normalize(original),
            normalize(resolve(&store, &tag, &temp.0.join("session")).unwrap()),
            "{}",
            path.display()
        );
        eprintln!(
            "round-tripped open fixture: {}",
            path.file_name().unwrap().to_string_lossy()
        );
        tested.push(path);
    }
    assert!(
        tested.len() >= 3,
        "the open FreeDOS TOML, YAML and preset files must be tested"
    );
}
#[test]
fn short_open_bios_checkpoint_matches_machine_file() {
    use cpu::Cpu;
    if fixtures::fixture("kiselev-bios/bios-xi8088.bin").is_none()
        || fixtures::fixture("freedos/freedos.img").is_none()
    {
        return;
    }
    let temp = Temp::new();
    let store = temp.store();
    let path = root().join("machines/at-freedos.toml");
    import_machine(&store, &path, "test:checkpoint", &ImportOptions::default()).unwrap();
    let original = machine_file::load(&path).unwrap();
    let imported = resolve(&store, "test:checkpoint", &temp.0.join("session")).unwrap();
    let checkpoint = |c: &assembly::MachineConfig| {
        let mut m = assembly::assemble(c).unwrap();
        let outcome = m
            .run_session(&assembly::RunPolicy::natural(Some(10_000), None))
            .unwrap();
        let mut memory = vec![0; 65536];
        m.machine.cpu().read_mem(0, &mut memory).unwrap();
        (
            outcome,
            m.machine.cpu().retired(),
            m.machine.virtual_time(),
            Digest::of(&memory),
        )
    };
    assert!(
        include_str!("../../../Cargo.lock").contains("ae12d43e7ec21dbed3e6c128a63c566a428a3054")
    );
    let reference = checkpoint(&original);
    assert_eq!(reference.0, machine::RunOutcome::RetiredCapReached);
    assert_eq!((reference.1, reference.2), (97, 10_000));
    assert_eq!(
        reference.3.to_string(),
        "sha256:c7862c33ecb65b0e7a067d159a52c6c48c439509cb584ebcc4952cffdc78948a"
    );
    assert_eq!(reference, checkpoint(&imported));
}
#[test]
fn provider_rejects_paths_unknown_fields_geometry_and_unbound_content() {
    let temp = Temp::new();
    fs::write(temp.0.join("disk.img"), vec![0; 360 * 1024]).unwrap();
    let Some(path) = fixture(&temp, &floppy(false)) else {
        return;
    };
    let store = temp.store();
    import_machine(&store, &path, "test:valid", &ImportOptions::default()).unwrap();
    let image = store.resolve("test:valid", &PcProvider).unwrap();
    let table: toml::Table = std::str::from_utf8(&image.config).unwrap().parse().unwrap();
    for mutate in 0..5 {
        let mut invalid = table.clone();
        match mutate {
            0 => {
                invalid.insert("unknown".into(), true.into());
            }
            1 => invalid["firmware"].as_array_mut().unwrap()[0]["source"] = "../outside.bin".into(),
            2 => invalid["media"].as_array_mut().unwrap()[0]["geometry"]["heads"] = 1.into(),
            3 => invalid["slots"].as_array_mut().unwrap()[0]["unit"] = 1.into(),
            _ => {
                invalid["slots"].as_array_mut().unwrap()[0]["catalogue"] =
                    Vec::<toml::Value>::new().into();
                invalid["slots"].as_array_mut().unwrap()[0]
                    .as_table_mut()
                    .unwrap()
                    .remove("inserted");
            }
        }
        assert!(
            PcProvider
                .validate(&invalid, &image.manifest.layers, &store)
                .is_err(),
            "mutation {mutate}"
        );
    }
    let mut shared = table.clone();
    shared["media"].as_array_mut().unwrap()[0]["policy"] = "readonly".into();
    let fdc = shared["machine"]["card"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|card| card["kind"].as_str() == Some("fdc-765"))
        .unwrap();
    let mut second_drive = fdc["drives"].as_array().unwrap()[0].clone();
    second_drive["unit"] = 1.into();
    fdc["drives"].as_array_mut().unwrap().push(second_drive);
    let mut second_slot = shared["slots"].as_array().unwrap()[0].clone();
    second_slot["unit"] = 1.into();
    shared["slots"].as_array_mut().unwrap().push(second_slot);
    assert!(PcProvider
        .validate(&shared, &image.manifest.layers, &store)
        .is_ok());
    shared["media"].as_array_mut().unwrap()[0]["policy"] = "journal".into();
    assert!(PcProvider
        .validate(&shared, &image.manifest.layers, &store)
        .is_err());
}

#[test]
fn catalogue_order_empty_slots_policies_and_unused_media_survive() {
    let temp = Temp::new();
    fs::write(temp.0.join("first.img"), vec![0x11; 360 * 1024]).unwrap();
    fs::write(temp.0.join("second.img"), vec![0x22; 360 * 1024]).unwrap();
    let Some(path) = fixture(&temp, "\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=1\ntype='360K'\nempty=true\n[[card.drives.media]]\nimage='second.img'\nwrite='readonly'\n[[card.drives.media]]\nimage='first.img'\nwrite='volatile'\n[[card.drives]]\nunit=0\ntype='360K'\nempty=true\nmedia=[]\n") else {
        return;
    };
    let store = temp.store();
    let record =
        import_machine(&store, &path, "test:catalogue", &ImportOptions::default()).unwrap();
    assert_eq!(record.generations.len(), 2);
    let original = machine_file::load(&path).unwrap();
    let imported = resolve(&store, "test:catalogue", &temp.0.join("session")).unwrap();
    assert_eq!(normalize(original), normalize(imported));
    let image = store.resolve("test:catalogue", &PcProvider).unwrap();
    assert_eq!(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| d.decoded_media_type() == image_store::media_type::FLOPPY)
            .count(),
        2
    );
}
#[test]
fn inheritance_and_symlink_origins_are_used_before_flattening() {
    let temp = Temp::new();
    fs::create_dir(temp.0.join("parent")).unwrap();
    fs::create_dir(temp.0.join("child")).unwrap();
    let Some(base) = fixture(&temp, "") else {
        return;
    };
    fs::rename(&base, temp.0.join("parent/base.toml")).unwrap();
    fs::rename(temp.0.join("bios.bin"), temp.0.join("parent/bios.bin")).unwrap();
    let child = temp.0.join("child/machine.toml");
    fs::write(&child, "schema=1\nextends='../parent/base.toml'\n[cpu]\nrate='3/7'\nfpu='80287'\n[board]\nmouse=true\n").unwrap();
    #[cfg(unix)]
    let child = {
        let link = temp.0.join("alias.toml");
        std::os::unix::fs::symlink(&child, &link).unwrap();
        link
    };
    let store = temp.store();
    import_machine(&store, &child, "test:origins", &ImportOptions::default()).unwrap();
    assert_eq!(
        machine_file::load(&child).unwrap(),
        resolve(&store, "test:origins", &temp.0.join("session")).unwrap()
    );
    let image = store.resolve("test:origins", &PcProvider).unwrap();
    let text = std::str::from_utf8(&image.config).unwrap();
    assert!(!text.contains(&temp.0.to_string_lossy().to_string()));
    assert!(!text.contains("extends"));
    assert!(!text.contains("base.toml"));
}
#[test]
fn volatile_cmos_and_interrupted_cmos_keep_exact_bytes() {
    let temp = Temp::new();
    let base = make_journal(&temp, true, false);
    let Some(path) = fixture(
        &temp,
        "\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='cmos.upfj'\n",
    ) else {
        return;
    };
    let store = temp.store();
    assert!(import_machine(
        &store,
        &path,
        "test:no-cmos-choice",
        &ImportOptions::default()
    )
    .unwrap_err()
    .to_string()
    .contains("explicit recovery choice"));
    let record = import_machine(
        &store,
        &path,
        "test:recovered-cmos",
        &recovery("cmos", RecoveryChoice::Recover),
    )
    .unwrap();
    assert_eq!(record.generations[0].generation, Some(1));
    assert!(record.generations[0].recovered);
    assert_eq!(
        first_bytes(&store, "test:recovered-cmos", pc_image::CMOS)[32],
        0xA7
    );
    let Some(path) = fixture(
        &temp,
        "\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='volatile'\n",
    ) else {
        return;
    };
    import_machine(
        &store,
        &path,
        "test:volatile-cmos",
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(
        first_bytes(&store, "test:volatile-cmos", pc_image::CMOS),
        base
    );
}
#[test]
fn retained_journal_history_and_readonly_source_are_untouched() {
    let temp = Temp::new();
    let base = make_journal(&temp, false, true);
    let journal = temp.0.join("disk.upfj");
    let mut writer =
        media::JournalMedia::open(&journal, &base, base.len() as u64, Default::default())
            .unwrap()
            .with_retention(1);
    writer.write_batch(&[(0, [0x5C; 512])]);
    writer.finish().unwrap();
    let before = fs::read(&journal).unwrap();
    let Some(path) = fixture(&temp, &floppy(true)) else {
        return;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&journal, fs::Permissions::from_mode(0o444)).unwrap();
    }
    let store = temp.store();
    let record = import_machine(&store, &path, "test:retained", &ImportOptions::default()).unwrap();
    assert_eq!(record.generations[0].generation, Some(2));
    assert_eq!(
        &first_bytes(&store, "test:retained", image_store::media_type::FLOPPY)[..512],
        &[0x5C; 512]
    );
    assert!(import_machine(
        &store,
        &path,
        "test:unavailable",
        &recovery("medium-0", RecoveryChoice::Generation(0))
    )
    .unwrap_err()
    .to_string()
    .contains("retained closed generation"));
    assert_eq!(fs::read(&journal).unwrap(), before);
}

#[test]
fn unlisted_firmware_is_rejected_even_without_a_source_pin() {
    let temp = Temp::new();
    let Some(path) = fixture(&temp, "") else {
        return;
    };
    let bios = temp.0.join("bios.bin");
    let mut bytes = fs::read(&bios).unwrap();
    bytes[0] ^= 1;
    fs::write(&bios, &bytes).unwrap();
    let store = temp.store();
    let error = import_machine(&store, &path, "test:unlisted", &ImportOptions::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("is not a known image"), "{error}");
    assert!(store.list_tags(None, 100).unwrap().tags.is_empty());
}
#[test]
fn split_open_firmware_preserves_chips_lanes_offsets_and_assembled_pin() {
    let temp = Temp::new();
    let Some(bios) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let mut parts = Vec::new();
    for (part, chunk) in bios.chunks(16384).enumerate() {
        for (lane, name) in [(0, "even"), (1, "odd")] {
            let bytes = chunk
                .iter()
                .skip(lane)
                .step_by(2)
                .copied()
                .collect::<Vec<_>>();
            let image = format!("{name}-{part}.bin");
            fs::write(temp.0.join(&image), &bytes).unwrap();
            parts.push(format!(
                "{{image='{image}',lane='{name}',offset={},sha256='{}'}}",
                part * 16384,
                Digest::of(&bytes).hex()
            ));
        }
    }
    let path = temp.0.join("machine.toml");
    fs::write(
        &path,
        format!(
            "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nsha256='{}'\nparts=[{}]\n",
            Digest::of(&bios).hex(),
            parts.join(",")
        ),
    )
    .unwrap();
    let store = temp.store();
    import_machine(&store, &path, "test:split", &ImportOptions::default()).unwrap();
    assert_eq!(
        machine_file::load(&path).unwrap(),
        resolve(&store, "test:split", &temp.0.join("session")).unwrap()
    );
    let image = store.resolve("test:split", &PcProvider).unwrap();
    assert_eq!(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| d.decoded_media_type() == image_store::media_type::FIRMWARE)
            .count(),
        4
    );
    let text = std::str::from_utf8(&image.config).unwrap();
    assert!(text.contains(Digest::of(&bios).hex()));
}

/// The verified decoded bytes a resolved image holds for this SHA-256.
fn medium_bytes(image: &image_store::ResolvedImage, sha256: [u8; 32]) -> Vec<u8> {
    use std::io::Read;
    let hex: String = sha256.iter().map(|b| format!("{b:02x}")).collect();
    let mut bytes = Vec::new();
    image
        .content_reader(&format!("sha256:{hex}").parse().unwrap())
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
fn catalogue_journals(temp: &Temp, absent_first: bool) -> Option<PathBuf> {
    for (name, value) in [("first", 0x11), ("second", 0x22)] {
        let base = vec![0; 360 * 1024];
        fs::write(temp.0.join(format!("{name}.img")), &base).unwrap();
        if name == "first" && absent_first {
            continue;
        }
        let journal_path = temp.0.join(format!("{name}.upfj"));
        let mut journal =
            media::JournalMedia::open(&journal_path, &base, base.len() as u64, Default::default())
                .unwrap();
        journal.write_batch(&[(0, [value; 512])]);
        journal.checkpoint().unwrap();
        journal.write_batch(&[(0, [value + 1; 512])]);
        if name == "first" {
            journal.finish().unwrap();
        } else {
            drop(journal);
        }
    }
    fixture(temp,"\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='first.img'\nwrite='journal'\njournal='first.upfj'\n[[card.drives.media]]\nimage='second.img'\nwrite='journal'\njournal='second.upfj'\n")
}
#[test]
fn recovery_choices_are_independent_and_absent_journals_use_zero() {
    for absent_first in [false, true] {
        let temp = Temp::new();
        let Some(path) = catalogue_journals(&temp, absent_first) else {
            return;
        };
        let store = temp.store();
        let error = import_machine(
            &store,
            &path,
            "test:missing-choice",
            &ImportOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("medium-1") && error.contains("recoverable generations"),
            "{error}"
        );
        let mut options = recovery("medium-1", RecoveryChoice::Generation(1));
        if !absent_first {
            options
                .recovery
                .insert("medium-0".into(), RecoveryChoice::Generation(0));
        }
        let record = import_machine(&store, &path, "test:independent", &options).unwrap();
        assert_eq!(record.generations[0].generation, Some(0));
        assert_eq!(record.generations[1].generation, Some(1));
        let config = resolve(&store, "test:independent", &temp.0.join("session")).unwrap();
        let entries = &config.drives[0].media.as_ref().unwrap().entries;
        let image = store.resolve("test:independent", &PcProvider).unwrap();
        let bytes = |entry: &assembly::FloppyMedium| {
            let assembly::ImageSource::File(path) = &entry.image else {
                panic!("journal media name a file-shaped base")
            };
            // A compressed floppy stays in memory; its key is never a file.
            assert!(!path.exists());
            medium_bytes(&image, entry.expected_sha256.unwrap())
        };
        assert_eq!(&bytes(&entries[0])[..512], &[0; 512]);
        assert_eq!(&bytes(&entries[1])[..512], &[0x22; 512]);
        let tip_options = recovery("medium-1", RecoveryChoice::Recover);
        let record = import_machine(&store, &path, "test:tip", &tip_options).unwrap();
        assert_eq!(
            record.generations[0].generation,
            Some(if absent_first { 0 } else { 2 })
        );
        assert_eq!(record.generations[1].generation, Some(2));
        assert!(record.generations[1].recovered);
    }
}
#[test]
fn pulled_medium_ids_cannot_escape_or_collide_with_cmos() {
    let temp = Temp::new();
    fs::write(temp.0.join("disk.img"), vec![0; 360 * 1024]).unwrap();
    let Some(path) = fixture(&temp, &floppy(false)) else {
        return;
    };
    let store = temp.store();
    import_machine(&store, &path, "test:ids", &ImportOptions::default()).unwrap();
    let image = store.resolve("test:ids", &PcProvider).unwrap();
    let table: toml::Table = std::str::from_utf8(&image.config).unwrap().parse().unwrap();
    for id in [
        "../../x",
        "/abs",
        "cmos",
        "medium/0",
        "",
        "UPPER",
        "012345678901234567890123456789012",
    ] {
        let mut bad = table.clone();
        bad["media"].as_array_mut().unwrap()[0]["id"] = id.into();
        bad["slots"].as_array_mut().unwrap()[0]["catalogue"] = vec![toml::Value::from(id)].into();
        bad["slots"].as_array_mut().unwrap()[0]["inserted"] = id.into();
        assert!(
            PcProvider
                .validate(&bad, &image.manifest.layers, &store)
                .is_err(),
            "{id}"
        );
    }
}
#[test]
fn snapshot_import_preserves_source_directory_and_sweeps_abandoned_staging() {
    let source = Temp::new();
    make_journal(&source, false, true);
    let Some(path) = fixture(&source, &floppy(true)) else {
        return;
    };
    fs::remove_file(source.0.join("disk.upfj.lock")).unwrap();
    let directory = || {
        let mut names = fs::read_dir(&source.0)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    let before = directory();
    let destination = Temp::new();
    let store = destination.store();
    let abandoned = store.root().join("staging/pc-import-v1-abandoned");
    fs::create_dir(&abandoned).unwrap();
    fs::write(abandoned.join("private.upfj"), b"partial").unwrap();
    import_machine(&store, &path, "test:snapshot", &ImportOptions::default()).unwrap();
    assert_eq!(directory(), before);
    assert!(!abandoned.exists());
    assert!(!source.0.join("disk.upfj.lock").exists());
}
#[test]
fn tag_preflight_and_local_records_precede_publication() {
    let temp = Temp::new();
    fs::write(temp.0.join("disk.img"), vec![0; 360 * 1024]).unwrap();
    let Some(path) = fixture(&temp, &floppy(false)) else {
        return;
    };
    let store = temp.store();
    let record = import_machine(&store, &path, "test:occupied", &ImportOptions::default()).unwrap();
    let saved = pc_image::ImportRecord::read(&store, &record.manifest.digest)
        .unwrap()
        .unwrap();
    assert_eq!(saved.generations[0].policy, pc_image::Policy::Volatile);
    assert_eq!(saved.generations[0].generation, None);
    let local = store
        .root()
        .join("local/imports")
        .join(format!("{}.toml", record.manifest.digest.hex()));
    let text = fs::read_to_string(local).unwrap();
    assert!(text.contains("policy = \"volatile\""));
    assert!(!text.contains("generation ="));
    let count = fs::read_dir(store.root().join("images/blobs/sha256"))
        .unwrap()
        .count();
    let absent = temp.0.join("absent.toml");
    assert!(
        import_machine(&store, &absent, "test:occupied", &ImportOptions::default())
            .unwrap_err()
            .to_string()
            .contains("tag conflict")
    );
    assert!(
        import_machine(&store, &absent, "invalid", &ImportOptions::default())
            .unwrap_err()
            .to_string()
            .contains("repository:tag")
    );
    assert_eq!(
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count(),
        count
    );
}
#[test]
fn import_uses_firmware_roots_and_public_origin_paths() {
    let temp = Temp::new();
    let Some(path) = fixture(&temp, "") else {
        return;
    };
    let roms = temp.0.join("roms");
    fs::create_dir(&roms).unwrap();
    fs::rename(temp.0.join("bios.bin"), roms.join("relocated.bin")).unwrap();
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("sha256='1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8'\n");
    fs::write(&path, text).unwrap();
    let options = ImportOptions {
        rom_dirs: vec![roms],
        ..Default::default()
    };
    let store = temp.store();
    import_machine(&store, &path, "test:discovered", &options).unwrap();
    resolve(&store, "test:discovered", &temp.0.join("session")).unwrap();
    assert_eq!(
        machine_file::Origins::new("preset:ibm-5170")
            .image_path("firmware.bios.image", "assets/system.bin"),
        PathBuf::from("assets/system.bin")
    );
}
#[test]
fn sb16_ems_and_materialized_defaults_round_trip() {
    for extra in ["\n[[card]]\nkind='sb16'\nbase=0x240\nirq=7\ndma8=3\ndma16=6\n[[card]]\nkind='lotech-ems'\nport=0x268\nframe=0xD000\nsize_kib=1536\n", "\n[[card]]\nkind='sb16'\n[[card]]\nkind='lotech-ems'\nframe=0xD000\n"] {
        let temp=Temp::new();let Some(path)=fixture(&temp,extra) else { return; };let store=temp.store();
        let original=machine_file::load(&path).unwrap();import_machine(&store,&path,"test:cards",&ImportOptions::default()).unwrap();
        assert_eq!(original,resolve(&store,"test:cards",&temp.0.join("session")).unwrap());
        let image=store.resolve("test:cards",&PcProvider).unwrap();let table:toml::Table=std::str::from_utf8(&image.config).unwrap().parse().unwrap();
        assert!(table["machine"]["cpu"].get("rate").is_some());assert!(table["machine"]["cpu"].get("fpu").is_some());
        assert!(table["machine"]["board"]["rtc"].get("start").is_some());assert!(table["machine"]["memory"].get("extended_kib").is_some());
        for card in table["machine"]["card"].as_array().unwrap() {
            match card["kind"].as_str().unwrap(){"sb16"=>{for key in ["base","irq","dma8","dma16"]{assert!(card.get(key).is_some());}},"lotech-ems"=>{for key in ["port","frame","size_kib"]{assert!(card.get(key).is_some());}},_=>{}}
        }
    }
}
#[test]
fn journaled_ide_resolves_file_backed_and_runs_short_checkpoint() {
    let temp = Temp::new();
    let base = vec![0; 8 * 2 * 4 * 512];
    fs::write(temp.0.join("ide.img"), &base).unwrap();
    let mut journal = media::JournalMedia::open(
        &temp.0.join("ide.upfj"),
        &base,
        base.len() as u64,
        Default::default(),
    )
    .unwrap();
    journal.write_batch(&[(1, [0xA7; 512])]);
    journal.finish().unwrap();
    let Some(xtide) = fixtures::fixture("xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin") else {
        return;
    };
    fs::write(temp.0.join("xtide.bin"), xtide).unwrap();
    let Some(path) = fixture(&temp,"\n[[firmware.option_roms]]\nimage='xtide.bin'\nbase=0xC8000\nprovides=['fixed-disk']\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=8,heads=2,sectors=4}\nimage='ide.img'\nwrite='journal'\njournal='ide.upfj'\n") else {
        return;
    };
    let original_journal = fs::read(temp.0.join("ide.upfj")).unwrap();
    let store = temp.store();
    let record = import_machine(&store, &path, "test:ide", &ImportOptions::default()).unwrap();
    assert_eq!(record.generations[0].generation, Some(1));
    let session = temp.0.join("session");
    fs::create_dir(&session).unwrap();
    let config = resolve(&store, "test:ide", &session).unwrap();
    assert!(config.drives[0].media.is_none());
    assert!(matches!(
        config.drives[0].image,
        assembly::ImageSource::File(_)
    ));
    assert!(config.drives[0].expected_sha256.is_some());
    assert!(matches!(
        config.drives[0].write,
        assembly::WriteMode::Journal { .. }
    ));
    let assembly::ImageSource::File(frozen) = &config.drives[0].image else {
        panic!("file-backed disk expected")
    };
    assert_eq!(&fs::read(frozen).unwrap()[512..1024], &[0xA7; 512]);
    let mut machine = assembly::assemble(&config).unwrap();
    assert_eq!(
        machine
            .run_session(&assembly::RunPolicy::natural(Some(10_000), None))
            .unwrap(),
        machine::RunOutcome::RetiredCapReached
    );
    machine.finish_persistence().unwrap();
    assert_eq!(fs::read(temp.0.join("ide.upfj")).unwrap(), original_journal);
}

#[test]
fn compression_preserves_machine_identity_and_resolves_mixed_payloads_without_sources() {
    use image_store::media_type as mt;
    let temp = Temp::new();
    let Some(path) = fixture(&temp, &floppy(false)) else {
        return;
    };
    fs::write(temp.0.join("disk.img"), vec![0x29; 360 * 1024]).unwrap();
    let store = temp.store();
    import_machine(
        &store,
        &path,
        "test:raw",
        &ImportOptions {
            no_compress: true,
            ..Default::default()
        },
    )
    .unwrap();
    import_machine(&store, &path, "test:default", &ImportOptions::default()).unwrap();
    let raw = store.resolve("test:raw", &PcProvider).unwrap();
    let compressed = store.resolve("test:default", &PcProvider).unwrap();
    assert_eq!(
        raw.manifest.config.digest,
        compressed.manifest.config.digest
    );
    assert_eq!(raw.config, compressed.config);
    assert!(raw.manifest.layers.iter().all(|d| !d.is_compressed()));
    assert!(compressed
        .manifest
        .layers
        .iter()
        .filter(|d| matches!(d.decoded_media_type(), mt::FIRMWARE | mt::FLOPPY))
        .all(|d| d.is_compressed()));
    let target = store
        .compress_image("test:raw", "test:compressed", &PcProvider)
        .unwrap();
    assert_eq!(target.digest, compressed.image.digest);
    let mut mixed = raw.manifest.clone();
    for d in &mut mixed.layers {
        if d.media_type == mt::FIRMWARE {
            *d = store
                .compress_payload(d, PcProvider.payload_types())
                .unwrap();
        }
    }
    let descriptor = store
        .import_manifest_with_types(&mixed, PcProvider.payload_types())
        .unwrap();
    store
        .update_tag("test:mixed", &descriptor, None, &PcProvider)
        .unwrap();
    fs::remove_file(temp.0.join("bios.bin")).unwrap();
    fs::remove_file(temp.0.join("disk.img")).unwrap();
    fs::remove_file(path).unwrap();
    for tag in ["test:raw", "test:default", "test:compressed", "test:mixed"] {
        let machine = resolve(&store, tag, &temp.0.join(tag.replace(':', "-"))).unwrap();
        assert_eq!(machine.drives[0].kind.image_bytes(), 360 * 1024);
    }
    let owner = store.workspace("compressed").unwrap();
    pc_image::workspace::prepare_start(&owner, "test:default", true).unwrap();
}

/// Every regular file under `root` with its length and modification time, so a
/// comparison proves an operation wrote nothing.
fn tree(root: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                files.push((entry.path(), metadata.len(), metadata.modified().unwrap()));
            }
        }
    }
    files.sort();
    files
}
/// Decoded payload and flattened-head files in the cache, without lock files.
fn decoded_files(store: &Store) -> Vec<String> {
    let mut names: Vec<_> = match fs::read_dir(store.root().join("cache/materialized")) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.ends_with(".lock"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

#[test]
fn compressed_import_and_resolution_write_nothing_and_a_mount_decodes_only_the_hard_disk() {
    use image_store::media_type as mt;
    let temp = Temp::new();
    // A near-blank fixed disk, a floppy and two ROMs, all compressible.
    let mut disk = vec![0; 8 * 2 * 4 * 512];
    disk[512..1024].fill(0xA7);
    fs::write(temp.0.join("ide.img"), &disk).unwrap();
    fs::write(temp.0.join("disk.img"), vec![0x29; 360 * 1024]).unwrap();
    let Some(xtide) = fixtures::fixture("xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin") else {
        return;
    };
    fs::write(temp.0.join("xtide.bin"), xtide).unwrap();
    let Some(path) = fixture(
        &temp,
        &format!("\n[[firmware.option_roms]]\nimage='xtide.bin'\nbase=0xC8000\nprovides=['fixed-disk']\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={{cylinders=8,heads=2,sectors=4}}\nimage='ide.img'\nwrite='journal'\njournal='ide.upfj'\n{}", floppy(true)),
    ) else {
        return;
    };
    let store = temp.store();
    let record =
        import_machine(&store, &path, "test:compressed", &ImportOptions::default()).unwrap();
    // Import verified every payload by streaming; the cache was never touched.
    assert!(!store.root().join("cache/materialized").exists());
    let image = store.resolve("test:compressed", &PcProvider).unwrap();
    for kind in [mt::FIRMWARE, mt::FLOPPY, mt::DISK_BASE] {
        assert!(image
            .manifest
            .layers
            .iter()
            .filter(|d| d.decoded_media_type() == kind)
            .all(|d| d.is_compressed()));
    }
    // The push guard reads the redistribution mark on stored descriptors: the
    // user's payloads are user-supplied, the generated config and metadata open.
    assert!(image
        .manifest
        .layers
        .iter()
        .chain(std::iter::once(&image.manifest.config))
        .all(|d| d.annotations["unemu.redistribution"] == generated_mark(d)));
    let before = tree(store.root());
    store.resolve("test:compressed", &PcProvider).unwrap();
    pc_image::inspection::inspect(&store, &record.manifest, None).unwrap();
    pc_image::desktop::check(&store, &record.manifest, None, || false).unwrap();
    assert_eq!(tree(store.root()), before);
    // A mount decodes the hard disk only; the floppy and ROMs stay in memory.
    let owner = store.workspace("mount").unwrap();
    let prepared = pc_image::workspace::prepare_start(&owner, "test:compressed", true).unwrap();
    let base = image
        .manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_BASE)
        .unwrap();
    assert_eq!(
        decoded_files(&store),
        [base.decoded_digest().unwrap().hex()]
    );
    let floppy = prepared
        .machine
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::Floppy { .. }))
        .unwrap();
    let assembly::ImageSource::File(key) = &floppy.image else {
        panic!("journal media name a file-shaped base")
    };
    assert!(!key.exists());
    assert_eq!(
        media::SectorSource::read_sector(&prepared.bases[key], 0).unwrap(),
        [0x29; 512]
    );
    // In-memory bases cross to the execution thread and run with the disk.
    let prepared = prepared.into_transfer().unwrap().restore().unwrap();
    let mut machine = assembly::assemble_with_verified_bases(
        &prepared.machine,
        &assembly::ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        &prepared.bases,
    )
    .unwrap();
    machine
        .run_session(&assembly::RunPolicy::natural(Some(10_000), None))
        .unwrap();
    machine.finish_persistence().unwrap();
    assert_eq!(decoded_files(&store).len(), 1);
}

#[test]
fn compressing_a_machine_keeps_its_journal_retention() {
    let temp = Temp::new();
    fs::write(temp.0.join("disk.img"), vec![0x31; 360 * 1024]).unwrap();
    let Some(path) = fixture(&temp, &floppy(true)) else {
        return;
    };
    let store = temp.store();
    let raw = import_machine(
        &store,
        &path,
        "test:raw",
        &ImportOptions {
            no_compress: true,
            ..Default::default()
        },
    )
    .unwrap();
    let compressed = pc_image::compress_image(&store, "test:raw", "test:small").unwrap();
    assert_ne!(compressed.digest, raw.manifest.digest);
    let image = store.resolve("test:small", &PcProvider).unwrap();
    assert!(image.manifest.layers.iter().any(|d| d.is_compressed()));
    assert!(image
        .manifest
        .layers
        .iter()
        .all(|d| d.annotations["unemu.redistribution"] == generated_mark(d)));
    let record = pc_image::ImportRecord::read(&store, &compressed.digest)
        .unwrap()
        .unwrap();
    assert_eq!(record.manifest, compressed);
    assert_eq!(record.generations, raw.generations);
    assert_eq!(record.generations[0].keep_generations, Some(3));
    let owner = store.workspace("small").unwrap();
    let prepared = pc_image::workspace::prepare_start(&owner, "test:small", true).unwrap();
    assert!(matches!(
        prepared.machine.drives[0].write,
        assembly::WriteMode::Journal {
            keep_generations: 3,
            ..
        }
    ));
}

#[test]
fn a_damaged_raw_base_is_refused_and_its_receipt_rejected() {
    let temp = Temp::new();
    fs::write(temp.0.join("disk.img"), vec![0x52; 360 * 1024]).unwrap();
    let Some(path) = fixture(&temp, &floppy(false)) else {
        return;
    };
    let store = temp.store();
    import_machine(
        &store,
        &path,
        "test:raw",
        &ImportOptions {
            no_compress: true,
            ..Default::default()
        },
    )
    .unwrap();
    let image = store.resolve("test:raw", &PcProvider).unwrap();
    let floppy = image
        .manifest
        .layers
        .iter()
        .find(|d| d.media_type == image_store::media_type::FLOPPY)
        .unwrap();
    let blob = store.blob_file_path(&floppy.digest, floppy.size).unwrap();
    image.verified_file(&floppy.digest).unwrap();
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&blob, bytes).unwrap();
    // A receipt taken before the damage refuses the changed file.
    assert!(image.verified_file(&floppy.digest).is_err());
    assert!(store.resolved_file_path(&image, &floppy.digest).is_err());
    assert!(matches!(
        store.resolve("test:raw", &PcProvider),
        Err(image_store::Error::DigestMismatch { .. })
    ));
}
/// Import marks its generated configuration and metadata open and every
/// payload from the user's files user-supplied.
fn generated_mark(d: &image_store::Descriptor) -> &'static str {
    use image_store::media_type::{CONFIG, METADATA};
    if matches!(d.media_type.as_str(), CONFIG | METADATA) {
        "open"
    } else {
        "user-supplied"
    }
}

/// A machine image keeps each floppy medium's own format: a 1.44M drive whose
/// catalogue lists a 720K and a 1.44M disk, and a 1.2M drive with a single
/// 360K image, publish, resolve, assemble and republish from a session. The
/// rule stays the drive type's accepted media: a 1.2M disk in the 1.44M
/// catalogue is still refused.
#[test]
fn mixed_density_floppy_media_round_trip_through_publish_and_resolve() {
    let temp = Temp::new();
    let drives = "\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.2M'\nimage='old-360.img'\nwrite='readonly'\n[[card.drives]]\nunit=1\ntype='1.44M'\nmedia=[{image='game-720.img',write='readonly'},{image='disk-1440.img',write='readonly'}]\n";
    let Some(path) = fixture(&temp, drives) else {
        return;
    };
    let images = [
        ("old-360.img", 368_640, 0x36),
        ("game-720.img", 737_280, 0x72),
        ("disk-1440.img", 1_474_560, 0x14),
        ("hd-1200.img", 1_228_800, 0x12),
    ];
    for (name, size, byte) in images {
        fs::write(temp.0.join(name), vec![byte; size]).unwrap();
    }
    let store = temp.store();
    import_machine(&store, &path, "test:mixed", &ImportOptions::default()).unwrap();
    let check = |machine: &assembly::MachineConfig| {
        let floppy = |unit| {
            machine
                .drives
                .iter()
                .find(|d| d.controller == assembly::DeviceId::Fdc && d.unit == unit)
                .unwrap()
        };
        let bytes = |source: &assembly::ImageSource| match source {
            assembly::ImageSource::File(path) => fs::read(path).unwrap(),
            assembly::ImageSource::Inline(bytes) => bytes.clone(),
            other => panic!("unexpected source {other:?}"),
        };
        let a = floppy(0);
        assert_eq!(
            a.kind,
            assembly::DriveKind::Floppy {
                geometry: assembly::FloppyGeometry::HD_1_2
            }
        );
        assert_eq!(bytes(&a.image), vec![0x36; 368_640]);
        let b = floppy(1);
        assert_eq!(
            b.kind,
            assembly::DriveKind::Floppy {
                geometry: assembly::FloppyGeometry::HD_1_44
            }
        );
        let list = b.media.as_ref().unwrap();
        assert_eq!(list.entries.len(), 2);
        assert_eq!(bytes(&list.entries[0].image), vec![0x72; 737_280]);
        assert_eq!(bytes(&list.entries[1].image), vec![0x14; 1_474_560]);
        let _assembled = assembly::assemble_with_options(
            machine,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
    };
    check(&resolve(&store, "test:mixed", &temp.0.join("session-a")).unwrap());
    let owner = store.workspace("mixed").unwrap();
    let prepared = pc_image::workspace::prepare_start(&owner, "test:mixed", true).unwrap();
    check(&prepared.machine);
    pc_image::workspace::publish(&owner, "test:republished", None).unwrap();
    drop(owner);
    check(&resolve(&store, "test:republished", &temp.0.join("session-b")).unwrap());

    let refused = fs::read_to_string(&path)
        .unwrap()
        .replace("image='disk-1440.img'", "image='hd-1200.img'");
    fs::write(&path, refused).unwrap();
    let error = import_machine(&store, &path, "test:refused", &ImportOptions::default())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("a 1.44M drive takes 1.44M (1474560 bytes) or 720K (737280 bytes) media"),
        "{error}"
    );
}
