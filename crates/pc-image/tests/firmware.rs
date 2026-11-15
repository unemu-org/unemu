// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic chip/package integration and open-fixture provenance; machine-images §5.3.

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

use image_store::{
    firmware::{Config, Validator},
    media_type as mt,
    summary::{ArtifactKind, Facets, SummaryCache},
    BlobSource, Descriptor, Digest, Manifest, Store,
};
use pc_image::{
    firmware::{Fit, PcFirmwareRules},
    firmware_import::{self, Attachment},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "firmware-package-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
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
fn leaked(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
fn catalogue() -> Vec<assembly::FirmwareEntry> {
    let even = vec![0x31; 16384];
    let odd = vec![0x72; 16384];
    let assembled = [0x31, 0x72].repeat(16384);
    let parts: &'static [assembly::KnownPart] = Box::leak(
        vec![
            assembly::KnownPart {
                label: "U27",
                part_number: "test-even",
                lane: assembly::Lane::Even,
                offset: 0,
                size: even.len() as u64,
                sha256: leaked(Digest::of(&even).hex().into()),
            },
            assembly::KnownPart {
                label: "U47",
                part_number: "test-odd",
                lane: assembly::Lane::Odd,
                offset: 0,
                size: odd.len() as u64,
                sha256: leaked(Digest::of(&odd).hex().into()),
            },
        ]
        .into_boxed_slice(),
    );
    vec![assembly::FirmwareEntry {
        id: "synthetic-two-chip-bios",
        sha256: leaked(Digest::of(&assembled).hex().into()),
        size: assembled.len() as u64,
        parts: Box::leak(vec![parts].into_boxed_slice()),
        distribution: assembly::FirmwareDistribution::UserSupplied,
        ..assembly::KISELEV_XI8088
    }]
}
fn chips(temp: &Temp) -> PathBuf {
    let root = temp.0.join("roms");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), vec![0x31; 16384]).unwrap();
    fs::write(root.join("b"), vec![0x72; 16384]).unwrap();
    root
}
#[test]
fn firmware_import_assembles_chips_reports_incomplete_unmatched_and_preserves_sources() {
    let t = Temp::new();
    let cat = catalogue();
    let root = chips(&t);
    fs::write(root.join("unmatched"), vec![0x99; 16384]).unwrap();
    fs::write(root.join("wrong-size"), b"not a ROM").unwrap();
    let before = [
        fs::read(root.join("a")).unwrap(),
        fs::read(root.join("b")).unwrap(),
    ];
    let scan = firmware_import::scan(std::slice::from_ref(&root), &cat).unwrap();
    assert_eq!(scan.candidates.len(), 1);
    assert!(scan.issues.is_empty());
    assert_eq!(scan.size_mismatches, 1);
    assert_eq!(scan.hash_mismatches, 1);
    let store = t.store();
    let extra = t.0.join("manual.txt");
    fs::write(&extra, b"synthetic manual").unwrap();
    let d = firmware_import::publish(
        &store,
        &scan.candidates[0],
        &[Attachment {
            path: extra.clone(),
            dos_name: Some("MANUAL.TXT".into()),
        }],
        &cat,
    )
    .unwrap();
    let image = store
        .resolve(
            "firmware/synthetic-two-chip-bios:1",
            &Validator(&PcFirmwareRules(&cat)),
        )
        .unwrap();
    let config = Config::decode(&image.config).unwrap();
    assert_eq!(config.roms[0].parts.len(), 2);
    assert_eq!(config.extras.len(), 1);
    assert_eq!(image.manifest.layers.len(), 3);
    assert_eq!(
        machine_file::firmware_package::reconstructed(
            &config.roms[0],
            &store,
            &image.manifest.layers
        )
        .unwrap(),
        [0x31, 0x72].repeat(16384)
    );
    assert!(image
        .manifest
        .layers
        .iter()
        .chain(std::iter::once(&image.manifest.config))
        .all(|d| d.annotations["unemu.redistribution"] == "user-supplied"));
    assert_eq!(fs::read(root.join("a")).unwrap(), before[0]);
    assert_eq!(fs::read(root.join("b")).unwrap(), before[1]);
    assert_eq!(fs::read(extra).unwrap(), b"synthetic manual");
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.rebuild(&store, 64, &[], &[]).unwrap(), 1);
    let row = cache.summary(&d.digest).unwrap();
    assert_eq!(row.kind, ArtifactKind::Firmware);
    assert!(row.machine.is_none());
    // A firmware row records its whole storage set like any other artifact, so
    // it is not re-queued as pending and counts toward shared storage totals.
    let mut stored = image
        .manifest
        .layers
        .iter()
        .chain([&image.manifest.config, &d])
        .map(|b| b.digest.clone())
        .collect::<Vec<_>>();
    stored.sort();
    let mut recorded = row
        .storage_blobs
        .iter()
        .map(|b| b.digest.clone())
        .collect::<Vec<_>>();
    recorded.sort();
    assert_eq!(recorded, stored);
    assert!(cache.pending().is_empty());
    let (total, _) = cache.storage_totals().unwrap();
    assert_eq!(total, row.stored_size);
    let f = Facets {
        firmware_roles: vec!["system-bios".into()],
        boards: vec!["ibm-5170".into()],
        machine_classes: vec!["at".into()],
        catalogue_ids: vec![cat[0].id.into()],
        ..Default::default()
    };
    assert_eq!(cache.query("synthetic", &f).len(), 1);
    fs::remove_file(root.join("b")).unwrap();
    let scan = firmware_import::scan(&[root.clone(), t.0.join("missing-file")], &cat).unwrap();
    assert!(scan.candidates.is_empty());
    assert!(scan
        .issues
        .iter()
        .any(|i| i.reason == "1 of 2 chips present, missing U47"));
    assert!(scan.issues.iter().any(|i| i.reason.contains("unreadable")));
}
#[test]
fn firmware_import_determinism_assembled_pin_and_catalogue_layout_are_enforced() {
    let t = Temp::new();
    let cat = catalogue();
    let root = chips(&t);
    let first = firmware_import::scan(std::slice::from_ref(&root), &cat).unwrap();
    let reversed = firmware_import::scan(&[root.join("b"), root.join("a")], &cat).unwrap();
    assert_eq!(
        first.candidates[0].config.encode().unwrap(),
        reversed.candidates[0].config.encode().unwrap()
    );
    let a = t.store();
    let b = Store::open(t.0.join("second-store")).unwrap();
    let d = firmware_import::publish(&a, &first.candidates[0], &[], &cat).unwrap();
    assert_eq!(
        d,
        firmware_import::publish(&b, &reversed.candidates[0], &[], &cat).unwrap()
    );
    let mut wrong = cat.clone();
    wrong[0].sha256 = leaked("00".repeat(32));
    let rejected = firmware_import::scan(std::slice::from_ref(&root), &wrong).unwrap();
    assert!(rejected.candidates.is_empty());
    assert!(rejected
        .issues
        .iter()
        .any(|i| i.reason.contains("assembled firmware identity")));
    let image = a
        .resolve(
            "firmware/synthetic-two-chip-bios:1",
            &Validator(&PcFirmwareRules(&cat)),
        )
        .unwrap();
    let mut bad = Config::decode(&image.config).unwrap();
    bad.roms[0].parts.swap(0, 1);
    use image_store::firmware::FirmwareRules;
    assert!(PcFirmwareRules(&cat)
        .validate(&bad, &a, &image.manifest.layers)
        .is_ok());
    bad.roms[0].parts[0].position = Some("U27".into());
    assert!(PcFirmwareRules(&cat)
        .validate(&bad, &a, &image.manifest.layers)
        .is_err());
    bad = Config::decode(&image.config).unwrap();
    bad.fits = Fit {
        boards: vec!["unsupported-board".into()],
        machine_classes: Vec::new(),
    }
    .table()
    .unwrap();
    assert!(PcFirmwareRules(&cat)
        .validate(&bad, &a, &image.manifest.layers)
        .is_err());
    let blob = &image.manifest.layers[0];
    let path = a.blob_file_path(&blob.digest, blob.size).unwrap();
    fs::write(path, vec![0; blob.size as usize]).unwrap();
    assert!(a
        .resolve(
            "firmware/synthetic-two-chip-bios:1",
            &Validator(&PcFirmwareRules(&cat))
        )
        .is_err());
}
fn unlisted_package(store: &Store, root: &Path) -> (Descriptor, Vec<assembly::FirmwareEntry>) {
    let cat = catalogue();
    let mut scan = firmware_import::scan(&[root.to_path_buf()], &cat).unwrap();
    scan.candidates[0].config.roms[0].catalogue_id = None;
    (
        firmware_import::publish(store, &scan.candidates[0], &[], &cat).unwrap(),
        cat,
    )
}
#[test]
fn firmware_store_first_resolves_without_folders_rechecks_pins_and_blocks_tampering() {
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let (d, cat) = unlisted_package(&store, &root);
    let machine = t.0.join("machine.toml");
    fs::write(&machine, format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nsha256='{}'\nparts=[{{image='absent-even',lane='even'}},{{image='absent-odd',lane='odd'}}]\n[[card]]\nkind='mda'\n", cat[0].sha256)).unwrap();
    fs::remove_dir_all(&root).unwrap();
    let control = assembly::firmware_discovery::DiscoveryControl::default();
    let prepared =
        assembly::firmware_discovery::with_discovery_session(None, false, &control, || {
            machine_file::prepare_for_assembly_with_firmware_store(
                &machine,
                &[],
                &mut Vec::new(),
                Some(&store),
            )
        })
        .unwrap();
    assert_eq!(prepared.firmware_sources(), &[d.digest]);
    assert_eq!(control.snapshot().folder_listings, 0);
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    let loaded = prepared.load(options).unwrap();
    assert_eq!(
        loaded.config.images[0].source,
        assembly::ImageSource::Inline([0x31, 0x72].repeat(16384))
    );
    let (doc, _) = prepared.resolved();
    let part = &doc
        .firmware
        .as_ref()
        .unwrap()
        .bios
        .as_ref()
        .unwrap()
        .parts
        .as_ref()
        .unwrap()[0];
    fs::write(&part.image, vec![0; 16384]).unwrap();
    assert!(prepared.load(options).is_err());
    let root = chips(&t);
    let roots = [assembly::firmware_discovery::FirmwareRoot::new(
        root,
        assembly::firmware_discovery::RootOrigin::CommandLine,
    )];
    assert!(machine_file::prepare_for_assembly_with_firmware_store(
        &machine,
        &roots,
        &mut Vec::new(),
        Some(&store)
    )
    .is_err());
}
#[test]
fn firmware_store_miss_falls_back_to_pinned_folder_and_unpinned_path_is_not_guessed() {
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let cat = catalogue();
    let bytes = [0x31, 0x72].repeat(16384);
    fs::write(root.join("assembled"), &bytes).unwrap();
    let machine = t.0.join("machine.toml");
    fs::write(
        &machine,
        format!(
            "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='absent'\nsha256='{}'\n",
            cat[0].sha256
        ),
    )
    .unwrap();
    let roots = [assembly::firmware_discovery::FirmwareRoot::new(
        &root,
        assembly::firmware_discovery::RootOrigin::CommandLine,
    )];
    let p = machine_file::prepare_for_assembly_with_firmware_store(
        &machine,
        &roots,
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(p.firmware_sources().is_empty());
    assert!(p
        .resolved()
        .0
        .firmware
        .as_ref()
        .unwrap()
        .bios
        .as_ref()
        .unwrap()
        .image
        .as_ref()
        .unwrap()
        .ends_with("assembled"));
    fs::remove_file(root.join("assembled")).unwrap();
    unlisted_package(&store, &root);
    fs::write(
        &machine,
        "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='absent'\n",
    )
    .unwrap();
    let p = machine_file::prepare_for_assembly_with_firmware_store(
        &machine,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(p.firmware_sources().is_empty());
    assert_eq!(
        p.resolved()
            .0
            .firmware
            .as_ref()
            .unwrap()
            .bios
            .as_ref()
            .unwrap()
            .image
            .as_deref(),
        Some("absent")
    );
}
#[test]
fn firmware_machine_image_records_open_package_provenance_and_owns_rom_closure() {
    let t = Temp::new();
    let store = t.store();
    let Some(bytes) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let fixture = t.0.join("open.bin");
    fs::write(&fixture, bytes).unwrap();
    let scan = firmware_import::scan(&[fixture], pc_image::firmware::catalogue()).unwrap();
    assert_eq!(scan.candidates.len(), 1);
    let d = firmware_import::publish(
        &store,
        &scan.candidates[0],
        &[],
        pc_image::firmware::catalogue(),
    )
    .unwrap();
    let machine = t.0.join("machine.toml");
    fs::write(
        &machine,
        format!(
            "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='absent'\nsha256='{}'\n",
            assembly::KISELEV_XI8088.sha256
        ),
    )
    .unwrap();
    pc_image::import_machine(&store, &machine, "test:1", &Default::default()).unwrap();
    let image = store.resolve("test:1", &pc_image::PcProvider).unwrap();
    assert_eq!(
        image.manifest.annotations["unemu.firmware.sources"],
        format!("[\"{}\"]", d.digest)
    );
    let firmware = image
        .manifest
        .layers
        .iter()
        .find(|l| l.media_type == mt::FIRMWARE)
        .unwrap();
    let manifest = Manifest::decode(
        &store
            .read_verified(&d, image_store::MAX_SMALL_BLOB)
            .unwrap(),
    )
    .unwrap();
    assert!(manifest.layers.iter().any(|l| l.digest == firmware.digest));
    store
        .remove_tag(&scan.candidates[0].tag(), &d.digest)
        .unwrap();
    assert!(store.resolve("test:1", &pc_image::PcProvider).is_ok());
}

#[test]
fn firmware_package_rejects_missing_unreferenced_payloads_and_unknown_roles() {
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let cat = catalogue();
    let scan = firmware_import::scan(&[root], &cat).unwrap();
    firmware_import::publish(&store, &scan.candidates[0], &[], &cat).unwrap();
    let validator = Validator(&PcFirmwareRules(&cat));
    let image = store
        .resolve(&scan.candidates[0].tag(), &validator)
        .unwrap();
    let mut missing = image.manifest.clone();
    missing.layers.pop();
    let d = store.import_manifest(&missing).unwrap();
    assert!(store
        .update_tag("test:missing", &d, None, &validator)
        .is_err());
    assert!(store.tag_descriptor("test:missing").is_err());
    let extra = store.import(b"unreferenced".as_slice(), None).unwrap();
    let mut unreferenced = image.manifest.clone();
    unreferenced
        .layers
        .push(Descriptor::new(mt::EXTRA, extra.digest, extra.size));
    let d = store.import_manifest(&unreferenced).unwrap();
    assert!(store
        .update_tag("test:unreferenced", &d, None, &validator)
        .is_err());
    assert!(store.tag_descriptor("test:unreferenced").is_err());
    let mut config = Config::decode(&image.config).unwrap();
    config.roms[0].role = "mystery".into();
    let bytes = config.encode().unwrap();
    let blob = store.import(bytes.as_slice(), None).unwrap();
    let mut unknown = image.manifest.clone();
    unknown.config = Descriptor::new(mt::FIRMWARE_CONFIG, blob.digest, blob.size);
    let d = store.import_manifest(&unknown).unwrap();
    assert!(store
        .update_tag("test:unknown-role", &d, None, &validator)
        .is_err());
    config.roms[0].catalogue_id = None;
    let bytes = [0x31, 0x72].repeat(16384);
    let blob = store.import(bytes.as_slice(), None).unwrap();
    config.roms[0].blob = Some(blob.digest.clone());
    config.roms[0].parts.clear();
    config.roms[0].role = "keyboard-controller".into();
    let reserved_layers = [Descriptor::new(mt::FIRMWARE, blob.digest, blob.size)];
    config.fits = Fit {
        boards: vec!["ibm-5160".into()],
        machine_classes: vec!["xt".into()],
    }
    .table()
    .unwrap();
    use image_store::firmware::FirmwareRules;
    assert!(
        PcFirmwareRules(&cat)
            .validate(&config, &store, &reserved_layers)
            .is_ok(),
        "reserved hardware remains storable"
    );
}
#[test]
fn firmware_store_hit_preserves_declared_chip_order_and_never_lists_fallback_roots() {
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let (_, cat) = unlisted_package(&store, &root);
    let layout = cat[0].parts[0];
    let path = t.0.join("machine.toml");
    fs::write(&path,format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nsha256='{}'\nparts=[{{image='absent-odd',lane='odd',sha256='{}'}},{{image='absent-even',lane='even',sha256='{}'}}]\n",cat[0].sha256,layout[1].sha256,layout[0].sha256)).unwrap();
    fs::remove_dir_all(root).unwrap();
    let roots = [assembly::firmware_discovery::FirmwareRoot::new(
        t.0.join("offline-folder"),
        assembly::firmware_discovery::RootOrigin::CommandLine,
    )];
    let control = assembly::firmware_discovery::DiscoveryControl::default();
    let mut warnings = Vec::new();
    let prepared =
        assembly::firmware_discovery::with_discovery_session(None, false, &control, || {
            machine_file::prepare_for_assembly_with_firmware_store(
                &path,
                &roots,
                &mut warnings,
                Some(&store),
            )
        })
        .unwrap();
    let parts = prepared
        .resolved()
        .0
        .firmware
        .as_ref()
        .unwrap()
        .bios
        .as_ref()
        .unwrap()
        .parts
        .as_ref()
        .unwrap();
    assert_eq!(parts[0].lane, machine_file::schema::v1::Lane::Odd);
    assert_eq!(parts[1].lane, machine_file::schema::v1::Lane::Even);
    assert_eq!(parts[0].sha256.as_ref().unwrap().as_str(), layout[1].sha256);
    assert_eq!(control.snapshot().folder_listings, 0);
    assert!(warnings.is_empty());
    assert!(prepared
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        })
        .is_ok());
}

#[test]
fn firmware_store_skips_foreign_provider_packages_with_matching_pins() {
    struct Foreign;
    impl image_store::firmware::FirmwareRules for Foreign {
        fn platform(&self) -> &str {
            "synthetic-foreign"
        }
        fn validate(
            &self,
            _: &Config,
            _: &dyn BlobSource,
            _: &[Descriptor],
        ) -> image_store::Result<()> {
            Ok(())
        }
        fn validate_name(&self, _: &str) -> image_store::Result<()> {
            Ok(())
        }
    }
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let (pc, cat) = unlisted_package(&store, &root);
    let image = store
        .resolve(
            "firmware/synthetic-two-chip-bios:1",
            &Validator(&PcFirmwareRules(&cat)),
        )
        .unwrap();
    let mut config = Config::decode(&image.config).unwrap();
    config.platform = "synthetic-foreign".into();
    config.fits = toml::Table::from_iter([("family".into(), "synthetic".into())]);
    let bytes = config.encode().unwrap();
    let blob = store.import(bytes.as_slice(), None).unwrap();
    let foreign = Manifest::new_firmware(
        &config.platform,
        Descriptor::new(mt::FIRMWARE_CONFIG, blob.digest, blob.size),
        image.manifest.layers.clone(),
    )
    .unwrap();
    let foreign = store.import_manifest(&foreign).unwrap();
    store
        .update_tag("foreign:1", &foreign, None, &Validator(&Foreign))
        .unwrap();
    store
        .remove_tag("firmware/synthetic-two-chip-bios:1", &pc.digest)
        .unwrap();
    fs::write(t.0.join("declared.bin"), [0x31, 0x72].repeat(16384)).unwrap();
    let machine = t.0.join("machine.toml");
    fs::write(&machine,format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='declared.bin'\nsha256='{}'\n",cat[0].sha256)).unwrap();
    let prepared = machine_file::prepare_for_assembly_with_firmware_store(
        &machine,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(prepared.firmware_sources().is_empty());
    assert!(prepared
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        })
        .is_ok());
}

fn machine_text(rom: &str) -> String {
    format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\n{rom}\n[[card]]\nkind='mda'\n")
}
#[test]
fn partial_chip_pins_never_replace_an_unpinned_explicit_image() {
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let (_, cat) = unlisted_package(&store, &root);
    fs::write(t.0.join("even.bin"), vec![0x31; 16384]).unwrap();
    fs::write(t.0.join("patched.bin"), vec![0x99; 16384]).unwrap();
    let path = t.0.join("partial.toml");
    fs::write(&path, machine_text(&format!("parts=[{{image='even.bin',lane='even',sha256='{}'}},{{image='patched.bin',lane='odd'}}]", cat[0].parts[0][0].sha256))).unwrap();
    let prepared = machine_file::prepare_for_assembly_with_firmware_store(
        &path,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(prepared.firmware_sources().is_empty());
    let rom = prepared
        .resolved()
        .0
        .firmware
        .as_ref()
        .unwrap()
        .bios
        .as_ref()
        .unwrap();
    assert_eq!(rom.parts.as_ref().unwrap()[1].image, "patched.bin");
    let loaded = prepared
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        })
        .unwrap();
    assert_eq!(
        loaded.config.images[0].source,
        assembly::ImageSource::Inline([0x31, 0x99].repeat(16384))
    );
    fs::write(&path, machine_text(&format!("parts=[{{image='absent-even',lane='even',sha256='{}'}},{{image='absent-odd',lane='odd',sha256='{}'}}]", cat[0].parts[0][0].sha256, cat[0].parts[0][1].sha256))).unwrap();
    let pinned = machine_file::prepare_for_assembly_with_firmware_store(
        &path,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert_eq!(pinned.firmware_sources().len(), 1);
    assert!(pinned
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        })
        .is_ok());
}
#[test]
fn store_chip_matching_is_bijective_and_preserves_scalar_declarations() {
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let (_, cat) = unlisted_package(&store, &root);
    let path = t.0.join("layout.toml");
    fs::write(
        &path,
        machine_text(&format!(
            "sha256='{}'\nparts=[{{image='first',lane='even'}},{{image='second',lane='even'}}]",
            cat[0].sha256
        )),
    )
    .unwrap();
    let duplicate = machine_file::prepare_for_assembly_with_firmware_store(
        &path,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(duplicate.firmware_sources().is_empty());
    fs::write(t.0.join("declared.bin"), [0x31, 0x72].repeat(16384)).unwrap();
    fs::write(
        &path,
        machine_text(&format!("image='declared.bin'\nsha256='{}'", cat[0].sha256)),
    )
    .unwrap();
    let scalar = machine_file::prepare_for_assembly_with_firmware_store(
        &path,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(scalar.firmware_sources().is_empty());
    let rom = scalar
        .resolved()
        .0
        .firmware
        .as_ref()
        .unwrap()
        .bios
        .as_ref()
        .unwrap();
    assert_eq!(rom.image.as_deref(), Some("declared.bin"));
    assert!(rom.parts.is_none());
}
#[test]
fn provider_refuses_parts_for_every_non_system_role() {
    use image_store::firmware::FirmwareRules;
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let cat = catalogue();
    let mut scan = firmware_import::scan(&[root], &cat).unwrap();
    scan.candidates[0].config.roms[0].catalogue_id = None;
    for role in ["option-rom", "video-bios", "keyboard-controller", "other"] {
        let mut candidate = firmware_import::scan(&scan.candidates[0].sources, &cat)
            .unwrap()
            .candidates
            .remove(0);
        candidate.config.roms[0].catalogue_id = None;
        candidate.config.roms[0].role = role.into();
        assert!(
            firmware_import::publish(&store, &candidate, &[], &cat).is_err(),
            "{role}"
        );
        let descriptors = candidate
            .config
            .payloads()
            .into_iter()
            .map(|(hash, size, kind)| Descriptor::new(kind, hash, size))
            .collect::<Vec<_>>();
        assert!(PcFirmwareRules(&cat)
            .validate(&candidate.config, &store, &descriptors)
            .is_err());
        assert!(store.tag_descriptor(&candidate.tag()).is_err());
    }
}
fn alternative_catalogue() -> (Vec<assembly::FirmwareEntry>, Vec<Vec<u8>>, Vec<u8>) {
    let bytes = (1..=4).map(|i| vec![i; 16384]).collect::<Vec<_>>();
    let make = |index: usize, offset, lane, label| assembly::KnownPart {
        label,
        part_number: "synthetic",
        lane,
        offset,
        size: bytes[index].len() as u64,
        sha256: leaked(Digest::of(&bytes[index]).hex().into()),
    };
    let four: &'static [assembly::KnownPart] = Box::leak(
        vec![
            make(0, 0, assembly::Lane::Even, "U17"),
            make(1, 0, assembly::Lane::Odd, "U37"),
            make(2, 32768, assembly::Lane::Even, "U27"),
            make(3, 32768, assembly::Lane::Odd, "U47"),
        ]
        .into_boxed_slice(),
    );
    let even = [bytes[0].as_slice(), bytes[2].as_slice()].concat();
    let odd = [bytes[1].as_slice(), bytes[3].as_slice()].concat();
    let two: &'static [assembly::KnownPart] = Box::leak(
        vec![
            assembly::KnownPart {
                label: "U27",
                part_number: "synthetic",
                lane: assembly::Lane::Even,
                offset: 0,
                size: 32768,
                sha256: leaked(Digest::of(&even).hex().into()),
            },
            assembly::KnownPart {
                label: "U47",
                part_number: "synthetic",
                lane: assembly::Lane::Odd,
                offset: 0,
                size: 32768,
                sha256: leaked(Digest::of(&odd).hex().into()),
            },
        ]
        .into_boxed_slice(),
    );
    let assembled = [[1, 2].repeat(16384), [3, 4].repeat(16384)].concat();
    let entry = assembly::FirmwareEntry {
        id: "synthetic-layout-bios",
        size: 65536,
        sha256: leaked(Digest::of(&assembled).hex().into()),
        parts: Box::leak(vec![two, four].into_boxed_slice()),
        distribution: assembly::FirmwareDistribution::UserSupplied,
        ..assembly::KISELEV_XI8088
    };
    (vec![entry], bytes, assembled)
}
#[test]
fn four_chip_offsets_ambiguity_and_assembled_input_are_distinct_choices() {
    let t = Temp::new();
    let (cat, bytes, assembled) = alternative_catalogue();
    let root = t.0.join("four");
    fs::create_dir(&root).unwrap();
    for (i, b) in bytes.iter().enumerate() {
        fs::write(root.join(format!("chip-{i}")), b).unwrap();
    }
    let four = firmware_import::scan(std::slice::from_ref(&root), &cat).unwrap();
    assert_eq!(four.candidates.len(), 1);
    assert_eq!(
        four.candidates[0].config.roms[0]
            .parts
            .iter()
            .map(|p| p.offset)
            .collect::<Vec<_>>(),
        vec![0, 0, 32768, 32768]
    );
    let store = t.store();
    let d = firmware_import::publish(&store, &four.candidates[0], &[], &cat).unwrap();
    let resolved = store
        .resolve_descriptor(&d, &Validator(&PcFirmwareRules(&cat)))
        .unwrap();
    let config = Config::decode(&resolved.config).unwrap();
    assert_eq!(
        machine_file::firmware_package::reconstructed(
            &config.roms[0],
            &store,
            &resolved.manifest.layers
        )
        .unwrap(),
        assembled
    );
    fs::write(root.join("assembled"), &assembled).unwrap();
    let ambiguous = firmware_import::scan(std::slice::from_ref(&root), &cat).unwrap();
    assert!(ambiguous.candidates.is_empty());
    assert!(ambiguous
        .issues
        .iter()
        .any(|i| i.reason.contains("ambiguous complete layouts")));
    fs::remove_file(root.join("assembled")).unwrap();
    fs::write(
        root.join("two-even"),
        [bytes[0].as_slice(), bytes[2].as_slice()].concat(),
    )
    .unwrap();
    fs::write(
        root.join("two-odd"),
        [bytes[1].as_slice(), bytes[3].as_slice()].concat(),
    )
    .unwrap();
    let ambiguous = firmware_import::scan(&[root], &cat).unwrap();
    assert!(ambiguous.candidates.is_empty());
    assert!(ambiguous
        .issues
        .iter()
        .any(|i| i.reason.contains("ambiguous complete layouts")));
}
#[test]
fn store_selection_uses_the_declared_two_or_four_chip_layout() {
    let t = Temp::new();
    let (cat, bytes, assembled) = alternative_catalogue();
    let store = t.store();
    let paths = bytes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let p = t.0.join(format!("chip-{i}"));
            fs::write(&p, b).unwrap();
            p
        })
        .collect::<Vec<_>>();
    let mut four = firmware_import::scan(&paths, &cat)
        .unwrap()
        .candidates
        .remove(0);
    four.config.roms[0].catalogue_id = None;
    four.catalogue_id = "layout-four".into();
    let four_digest = firmware_import::publish(&store, &four, &[], &cat).unwrap();
    let two = [t.0.join("even"), t.0.join("odd")];
    fs::write(&two[0], [bytes[0].as_slice(), bytes[2].as_slice()].concat()).unwrap();
    fs::write(&two[1], [bytes[1].as_slice(), bytes[3].as_slice()].concat()).unwrap();
    let mut candidate = firmware_import::scan(&two, &cat)
        .unwrap()
        .candidates
        .remove(0);
    candidate.config.roms[0].catalogue_id = None;
    candidate.catalogue_id = "layout-two".into();
    let two_digest = firmware_import::publish(&store, &candidate, &[], &cat).unwrap();
    let path = t.0.join("machine.toml");
    for (parts,expected) in [("[{image='a',lane='even',offset=0},{image='b',lane='odd',offset=0},{image='c',lane='even',offset=32768},{image='d',lane='odd',offset=32768}]",four_digest.digest),("[{image='e',lane='even'},{image='o',lane='odd'}]",two_digest.digest)] {
        fs::write(&path,machine_text(&format!("sha256='{}'\nparts={parts}",cat[0].sha256))).unwrap();
        let p=machine_file::prepare_for_assembly_with_firmware_store(&path,&[],&mut Vec::new(),Some(&store)).unwrap();assert_eq!(p.firmware_sources(),&[expected]);
        let loaded=p.load(assembly::AssemblyOptions{allow_unlisted_firmware:true}).unwrap();assert_eq!(loaded.config.images[0].source,assembly::ImageSource::Inline(assembled.clone()));
    }
    fs::write(t.0.join("assembled.bin"), &assembled).unwrap();
    fs::write(
        &path,
        machine_text(&format!(
            "image='assembled.bin'\nsha256='{}'",
            cat[0].sha256
        )),
    )
    .unwrap();
    let p = machine_file::prepare_for_assembly_with_firmware_store(
        &path,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(p.firmware_sources().is_empty());
    assert!(p
        .resolved()
        .0
        .firmware
        .as_ref()
        .unwrap()
        .bios
        .as_ref()
        .unwrap()
        .parts
        .is_none());
}
#[test]
fn unavailable_store_open_and_summary_cache_warn_and_fall_back() {
    let t = Temp::new();
    let cat = catalogue();
    let roms = t.0.join("roms");
    fs::create_dir(&roms).unwrap();
    fs::write(roms.join("assembled"), [0x31, 0x72].repeat(16384)).unwrap();
    let machine = t.0.join("machine.toml");
    fs::write(
        &machine,
        machine_text(&format!("image='absent'\nsha256='{}'", cat[0].sha256)),
    )
    .unwrap();
    let roots = [assembly::firmware_discovery::FirmwareRoot::new(
        roms,
        assembly::firmware_discovery::RootOrigin::CommandLine,
    )];
    // A default root where no store has been created yet is an ordinary miss.
    let mut warnings = Vec::new();
    let p = machine_file::prepare_for_assembly_with_store_root(
        &machine,
        &roots,
        &mut warnings,
        Some(&t.0.join("no-store-yet")),
    )
    .unwrap();
    assert!(p.firmware_sources().is_empty());
    assert!(
        !warnings.iter().any(|w| w.contains("store")),
        "{warnings:?}"
    );
    assert!(!t.0.join("no-store-yet").exists());
    assert!(p
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        })
        .is_ok());
    let bad = t.0.join("not-a-store");
    fs::write(&bad, b"not a directory").unwrap();
    let mut warnings = Vec::new();
    let p = machine_file::prepare_for_assembly_with_store_root(
        &machine,
        &roots,
        &mut warnings,
        Some(&bad),
    )
    .unwrap();
    assert!(p.firmware_sources().is_empty());
    assert!(warnings
        .iter()
        .any(|w| w.contains("firmware store unavailable")));
    assert!(p
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        })
        .is_ok());
    let store = t.store();
    fs::create_dir_all(store.root().join("cache")).unwrap();
    fs::write(
        store.root().join("cache/summaries"),
        b"not a cache directory",
    )
    .unwrap();
    let mut warnings = Vec::new();
    let p = machine_file::prepare_for_assembly_with_firmware_store(
        &machine,
        &roots,
        &mut warnings,
        Some(&store),
    )
    .unwrap();
    assert!(p.firmware_sources().is_empty());
    assert!(warnings
        .iter()
        .any(|w| w.contains("store index unavailable")));
    assert!(p
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        })
        .is_ok());
}
#[test]
fn scan_limits_bound_enumeration_and_summarize_non_rom_files() {
    use firmware_import::{scan_with_limits, ScanLimits};
    let t = Temp::new();
    let cat = catalogue();
    let root = chips(&t);
    for i in 0..20 {
        fs::write(root.join(format!("text-{i}")), b"not a ROM").unwrap();
    }
    let scan = firmware_import::scan(std::slice::from_ref(&root), &cat).unwrap();
    assert_eq!(scan.size_mismatches, 20);
    assert!(scan.issues.is_empty());
    assert_eq!(scan.candidates.len(), 1);
    let capped = scan_with_limits(
        std::slice::from_ref(&root),
        &cat,
        ScanLimits {
            depth: 32,
            entries: 5,
        },
    )
    .unwrap();
    assert!(capped.candidates.is_empty());
    assert_eq!(capped.issues.len(), 1);
    assert!(capped.issues[0].reason.contains("entry count"));
    let deep = root.join("one/two/three");
    fs::create_dir_all(&deep).unwrap();
    let capped = scan_with_limits(
        &[root],
        &cat,
        ScanLimits {
            depth: 1,
            entries: 100,
        },
    )
    .unwrap();
    assert!(capped.candidates.is_empty());
    assert!(capped
        .issues
        .iter()
        .any(|i| i.reason.contains("recursion depth")));
}
#[test]
fn reimport_reuses_identical_packages_but_rejects_changed_attachments() {
    let t = Temp::new();
    let cat = catalogue();
    let root = chips(&t);
    let store = t.store();
    let scan = firmware_import::scan(&[root], &cat).unwrap();
    let manual = t.0.join("manual.txt");
    fs::write(&manual, b"first").unwrap();
    let extras = [Attachment {
        path: manual.clone(),
        dos_name: None,
    }];
    let first = firmware_import::publish(&store, &scan.candidates[0], &extras, &cat).unwrap();
    assert_eq!(
        first,
        firmware_import::publish(&store, &scan.candidates[0], &extras, &cat).unwrap()
    );
    let manifest = firmware_import::preview_manifest(&scan.candidates[0], &extras).unwrap();
    assert!(
        firmware_import::already_imported(&store, &scan.candidates[0].tag(), &manifest, &cat)
            .unwrap()
    );
    fs::write(manual, b"changed").unwrap();
    assert!(firmware_import::publish(&store, &scan.candidates[0], &extras, &cat).is_err());
    assert_eq!(
        store
            .tag_descriptor(&scan.candidates[0].tag())
            .unwrap()
            .digest,
        first.digest
    );
}

#[test]
fn legacy_option_chip_package_does_not_replace_a_scalar_option_rom() {
    struct Legacy;
    impl image_store::firmware::FirmwareRules for Legacy {
        fn platform(&self) -> &str {
            "ibm-pc-at"
        }
        fn validate(
            &self,
            _: &Config,
            _: &dyn BlobSource,
            _: &[Descriptor],
        ) -> image_store::Result<()> {
            Ok(())
        }
        fn validate_name(&self, _: &str) -> image_store::Result<()> {
            Ok(())
        }
    }
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let (d, cat) = unlisted_package(&store, &root);
    let image = store
        .resolve_descriptor(&d, &Validator(&PcFirmwareRules(&cat)))
        .unwrap();
    let mut config = Config::decode(&image.config).unwrap();
    config.roms[0].role = "option-rom".into();
    let bytes = config.encode().unwrap();
    let blob = store.import(bytes.as_slice(), None).unwrap();
    let legacy = Manifest::new_firmware(
        &config.platform,
        Descriptor::new(mt::FIRMWARE_CONFIG, blob.digest, blob.size),
        image.manifest.layers,
    )
    .unwrap();
    let legacy = store.import_manifest(&legacy).unwrap();
    store
        .update_tag("legacy:option", &legacy, None, &Validator(&Legacy))
        .unwrap();
    fs::write(t.0.join("declared.bin"), [0x31, 0x72].repeat(16384)).unwrap();
    let path = t.0.join("machine.toml");
    fs::write(
        &path,
        format!(
            "{}\n[[firmware.option_roms]]\nimage='declared.bin'\nsha256='{}'\nbase=0xC0000\n",
            machine_text("image='declared.bin'"),
            cat[0].sha256
        ),
    )
    .unwrap();
    let prepared = machine_file::prepare_for_assembly_with_firmware_store(
        &path,
        &[],
        &mut Vec::new(),
        Some(&store),
    )
    .unwrap();
    assert!(prepared.firmware_sources().is_empty());
    let option = &prepared
        .resolved()
        .0
        .firmware
        .as_ref()
        .unwrap()
        .option_roms
        .as_ref()
        .unwrap()[0];
    assert!(option.parts.is_none());
    assert_eq!(option.image.as_deref(), Some("declared.bin"));
}
#[cfg(unix)]
#[test]
fn preselection_lease_and_cache_save_failures_are_misses() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new();
    let root = chips(&t);
    let store = t.store();
    let (_, cat) = unlisted_package(&store, &root);
    let machine = t.0.join("machine.toml");
    fs::write(t.0.join("declared.bin"), [0x31, 0x72].repeat(16384)).unwrap();
    fs::write(
        &machine,
        machine_text(&format!("image='declared.bin'\nsha256='{}'", cat[0].sha256)),
    )
    .unwrap();
    let cache = store.root().join("cache/summaries");
    fs::create_dir_all(&cache).unwrap();
    let permissions = fs::metadata(&cache).unwrap().permissions();
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o555)).unwrap();
    // Root, as in a Linux CI container, may write to a read-only directory.
    let probe = cache.join("permission-probe");
    let denied = fs::write(&probe, []).is_err();
    let _ = fs::remove_file(&probe);
    let mut warnings = Vec::new();
    let result = machine_file::prepare_for_assembly_with_firmware_store(
        &machine,
        &[],
        &mut warnings,
        Some(&store),
    );
    fs::set_permissions(&cache, permissions).unwrap();
    let p = result.unwrap();
    if denied {
        assert!(p.firmware_sources().is_empty());
        assert!(warnings
            .iter()
            .any(|w| w.contains("store index unavailable")));
    } else {
        eprintln!("skipped cache-save failure checks: this host writes to a read-only directory (for example, as root)");
    }
    assert!(p
        .load(assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        })
        .is_ok());
    let lease = store.root().join("locks/store.lock");
    fs::remove_file(&lease).unwrap();
    fs::create_dir(&lease).unwrap();
    let mut warnings = Vec::new();
    let p = machine_file::prepare_for_assembly_with_firmware_store(
        &machine,
        &[],
        &mut warnings,
        Some(&store),
    )
    .unwrap();
    assert!(p.firmware_sources().is_empty());
    assert!(warnings
        .iter()
        .any(|w| w.contains("store index unavailable")));
}
#[test]
fn split_package_presence_cannot_change_a_scalar_machine_image_digest() {
    let Some(bytes) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let t = Temp::new();
    let store = t.store();
    fs::write(t.0.join("declared.bin"), &bytes).unwrap();
    let machine = t.0.join("machine.toml");
    fs::write(&machine,format!("schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='declared.bin'\nsha256='{}'\n",assembly::KISELEV_XI8088.sha256)).unwrap();
    let before =
        pc_image::import_machine(&store, &machine, "before:1", &Default::default()).unwrap();
    for lanes in [2, 4] {
        let mut parts = Vec::new();
        let mut paths = Vec::new();
        for index in 0..lanes {
            let offset = (index / 2) * (bytes.len() / lanes * 2);
            let lane = index % 2;
            let data = bytes[offset..offset + bytes.len() / lanes * 2]
                .iter()
                .skip(lane)
                .step_by(2)
                .copied()
                .collect::<Vec<_>>();
            let path = t.0.join(format!("layout-{lanes}-{index}"));
            fs::write(&path, &data).unwrap();
            paths.push(path);
            let digest = Digest::of(&data);
            parts.push(image_store::firmware::Part {
                blob: digest.clone(),
                sha256: digest.hex().into(),
                size: data.len() as u64,
                position: None,
                part_number: None,
                lane: if lane == 0 { "even" } else { "odd" }.into(),
                offset: offset as u64,
            });
        }
        let config = Config {
            schema: 1,
            platform: "ibm-pc-at".into(),
            name: "Synthetic split representation".into(),
            vendor: None,
            version: None,
            date: None,
            notes: None,
            fits: toml::Table::new(),
            roms: vec![image_store::firmware::Rom {
                id: "system-bios".into(),
                role: "system-bios".into(),
                catalogue_id: None,
                sha256: assembly::KISELEV_XI8088.sha256.into(),
                size: bytes.len() as u64,
                blob: None,
                position: None,
                part_number: None,
                parts,
            }],
            extras: Vec::new(),
        };
        let candidate = firmware_import::Candidate {
            catalogue_id: format!("open-split-{lanes}"),
            config,
            sources: paths,
            distribution: assembly::FirmwareDistribution::UserSupplied,
        };
        firmware_import::publish(&store, &candidate, &[], pc_image::firmware::catalogue()).unwrap();
    }
    let after = pc_image::import_machine(&store, &machine, "after:1", &Default::default()).unwrap();
    assert_eq!(before.manifest.digest, after.manifest.digest);
}
