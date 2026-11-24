// SPDX-License-Identifier: AGPL-3.0-or-later
//! Verified package facts and consented template acquisition, using invented bytes.

use image_store::{firmware, media_type as mt, BlobSource, Digest, Manifest, Store};
use pc_image::{firmware_import, recipe::template_facts};
use recipe::template::UsePlan;
use std::fs;
use unemu_core_api::{BindingKind, ContentReader, ErrorKind};

fn package(
    store: &Store,
    root: &std::path::Path,
    role: assembly::FirmwareKind,
) -> (UsePlan, Vec<assembly::FirmwareEntry>, Vec<u8>) {
    let (id, template, byte) = match role {
        assembly::FirmwareKind::SystemBios => (
            "synthetic-at-bios",
            *pc_image::firmware::catalogue()
                .iter()
                .find(|e| e.id == "unemu-at-bios-1.0.2-unemu.3")
                .unwrap(),
            0x11,
        ),
        assembly::FirmwareKind::VideoBios => (
            "synthetic-video-bios",
            assembly::FirmwareEntry::VGABIOS_0_9D_UNEMU_1_STD,
            0x22,
        ),
        assembly::FirmwareKind::OptionRom => (
            "synthetic-xtide",
            assembly::FirmwareEntry::XUB_R638_XTP_ATA1F0,
            0x33,
        ),
    };
    let bytes = vec![byte; 8192];
    let catalogue = vec![assembly::FirmwareEntry {
        id,
        kind: role,
        size: bytes.len() as u64,
        sha256: Box::leak(Digest::of(&bytes).hex().to_owned().into_boxed_str()),
        parts: &[],
        distribution: assembly::FirmwareDistribution::Open {
            fetch: "fixture",
            license: "MIT",
            provenance: "synthetic open package",
            source_archive_sha256: None,
        },
        ..template
    }];
    let path = root.join(format!("{id}.bin"));
    fs::write(&path, &bytes).unwrap();
    let scan = firmware_import::scan(&[path], &catalogue).unwrap();
    assert_eq!(scan.candidates.len(), 1);
    let descriptor = firmware_import::publish(store, &scan.candidates[0], &[], &catalogue).unwrap();
    (
        UsePlan {
            id: id.into(),
            image: format!("unemu/firmware/ibm-pc/{id}:r1"),
            digest: descriptor.digest.to_string(),
            present: true,
        },
        catalogue,
        bytes,
    )
}

#[test]
fn template_facts_for_three_open_packages_are_deterministic_and_package_local() {
    let temp = image_store::test_support::TempDir::new("template-facts");
    let store = Store::open(temp.path().join("store"))
        .unwrap()
        .with_compatibility(pc_image::compat::compatibility(
            pc_image::compat::NEXT_RELEASE,
        ));
    for kind in [
        assembly::FirmwareKind::SystemBios,
        assembly::FirmwareKind::VideoBios,
        assembly::FirmwareKind::OptionRom,
    ] {
        let (usage, catalogue, bytes) = package(&store, temp.path(), kind);
        let first = template_facts::firmware_use(&store, &usage, &catalogue).unwrap();
        let second = template_facts::firmware_use(&store, &usage, &catalogue).unwrap();
        assert_eq!(first.facts, second.facts);
        assert_eq!(first.facts.manifest_digest, usage.digest);
        assert_eq!(first.facts.id, usage.id);
        assert_eq!(first.facts.items.len(), 1);
        let item = &first.facts.items[0];
        assert_eq!(item.kind, BindingKind::FirmwareRom);
        assert_eq!(item.id, pc_image::firmware::role(kind));
        assert_eq!(item.roles, [pc_image::firmware::role(kind)]);
        assert_eq!(item.size, bytes.len() as u64);
        let content = first.content.open(&item.sha256, item.size).unwrap();
        let mut actual = vec![0; item.size as usize];
        content.read_at(0, &mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert_eq!(Digest::of(&actual).hex(), catalogue[0].sha256);
        assert_eq!(
            first
                .content
                .open(&item.sha256, item.size + 1)
                .unwrap_err()
                .kind,
            ErrorKind::Content
        );
        assert_eq!(
            first.content.open(&[0; 32], item.size).unwrap_err().kind,
            ErrorKind::Content
        );

        // Changing this package's declaration must never import a role from
        // another catalogue entry with the same content identity.
        let descriptor = recipe::template::descriptor(&store, &usage.digest.parse().unwrap())
            .unwrap()
            .unwrap();
        let mut manifest =
            Manifest::decode_for_summary(&store.read_compatible_manifest(&descriptor).unwrap())
                .unwrap();
        let mut config = firmware::Config::decode(
            &store
                .read_verified(&manifest.config, image_store::MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        config.roms[0].catalogue_id = None;
        config.roms[0].role = "other".into();
        let config_bytes = config.encode().unwrap();
        let blob = store.import(config_bytes.as_slice(), None).unwrap();
        manifest.config = image_store::Descriptor::new(mt::FIRMWARE_CONFIG, blob.digest, blob.size);
        let changed = store
            .import_manifest_with_types(&manifest, image_store::codec::FIRMWARE_PAYLOAD_TYPES)
            .unwrap();
        let changed_use = UsePlan {
            digest: changed.digest.to_string(),
            ..usage
        };
        let changed_facts = template_facts::firmware_use(&store, &changed_use, &catalogue).unwrap();
        assert_eq!(changed_facts.facts.items[0].roles, ["other"]);
    }
}

#[test]
fn template_facts_refuse_wrong_kind_corrupt_manifest_and_corrupt_payload() {
    let temp = image_store::test_support::TempDir::new("template-facts-refusals");
    let store = Store::open(temp.path().join("store"))
        .unwrap()
        .with_compatibility(pc_image::compat::compatibility(
            pc_image::compat::NEXT_RELEASE,
        ));
    let (usage, catalogue, _) = package(&store, temp.path(), assembly::FirmwareKind::VideoBios);
    let wrong = UsePlan {
        image: "unemu/media/ibm-pc/disks:1-r1".into(),
        ..usage.clone()
    };
    assert!(template_facts::firmware_use(&store, &wrong, &catalogue)
        .err()
        .unwrap()
        .contains("wrong artifact type"));
    let descriptor = recipe::template::descriptor(&store, &usage.digest.parse().unwrap())
        .unwrap()
        .unwrap();
    let manifest =
        Manifest::decode_for_summary(&store.read_compatible_manifest(&descriptor).unwrap())
            .unwrap();
    let blob = &manifest.layers[0];
    let path = store.blob_file_path(&blob.digest, blob.size).unwrap();
    let original = fs::read(&path).unwrap();
    fs::write(&path, vec![0; original.len()]).unwrap();
    assert!(template_facts::firmware_use(&store, &usage, &catalogue).is_err());
    fs::write(&path, original).unwrap();
    let path = store
        .blob_file_path(&descriptor.digest, descriptor.size)
        .unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(path, bytes).unwrap();
    assert!(template_facts::firmware_use(&store, &usage, &catalogue)
        .err()
        .unwrap()
        .contains("digest mismatch"));
}

fn media_package(store: &Store, marker: u8) -> (image_store::Descriptor, Manifest, Vec<u8>) {
    let payload = vec![marker; 737_280];
    let blob = store.import(payload.as_slice(), None).unwrap();
    let medium = image_store::Descriptor::new(mt::FLOPPY, blob.digest, blob.size);
    let config = image_store::media_set::Config {
        schema: 1,
        platform: "ibm-pc".into(),
        media: vec![image_store::media_set::Medium {
            id: "disk-1".into(),
            label: "Disk 1".into(),
            format: pc_image::media_set::format("720K").unwrap(),
            blob: medium.digest.clone(),
        }],
        extras: Vec::new(),
    }
    .encode()
    .unwrap();
    let blob = store.import(config.as_slice(), None).unwrap();
    let config = image_store::Descriptor::new(mt::MEDIA_SET_CONFIG, blob.digest, blob.size);
    let blob = store
        .import(
            br#"{"schema":1,"platform":"ibm-pc","titles":[]}"#.as_slice(),
            None,
        )
        .unwrap();
    let metadata = image_store::Descriptor::new(mt::METADATA, blob.digest, blob.size);
    let manifest = Manifest::new_media_set("ibm-pc", config, vec![medium, metadata]).unwrap();
    let descriptor = store
        .import_manifest_with_types(&manifest, image_store::codec::MEDIA_PAYLOAD_TYPES)
        .unwrap();
    (descriptor, manifest, payload)
}

#[test]
fn template_media_facts_and_plan_use_the_manifest_digest_without_tags() {
    use recipe::provider::Provider;
    let temp = image_store::test_support::TempDir::new("template-media");
    let store = Store::open(temp.path().join("store"))
        .unwrap()
        .with_compatibility(pc_image::compat::compatibility(
            pc_image::compat::NEXT_RELEASE,
        ));
    let (descriptor, _, bytes) = media_package(&store, 42);
    let usage = UsePlan {
        id: "dos".into(),
        image: "private/media/ibm-pc/dos:1-r1".into(),
        digest: descriptor.digest.to_string(),
        present: true,
    };
    let verified = pc_image::recipe::PcRecipes
        .template_use(&store, &usage)
        .unwrap();
    assert_eq!(verified.facts.items.len(), 1);
    let item = &verified.facts.items[0];
    assert_eq!(item.id, "disk-1");
    assert_eq!(item.kind, BindingKind::Medium);
    assert!(item.roles.is_empty());
    let source = verified.content.open(&item.sha256, item.size).unwrap();
    let mut actual = vec![0; bytes.len()];
    source.read_at(0, &mut actual).unwrap();
    assert_eq!(actual, bytes);
    let path = temp.path().join("media.machine.toml");
    let config = b"config bytes are opaque to preparation";
    fs::write(temp.path().join("config.toml"), config).unwrap();
    fs::write(&path, format!("recipe='1.1'\n[machine]\nname='private/machines/ibm-pc/media'\nplatform='ibm-pc'\nversion='1.3.0'\n[template]\nconfig='config'\n[input.config]\nname='config.toml'\nsize={}\nsha256='{}'\nbundle=true\n[use.dos]\nimage='{}'\ndigest='{}'\n", config.len(), Digest::of(config).hex(),usage.image,usage.digest)).unwrap();
    let index = recipe::store_index::StoreIndex::load(&store).unwrap();
    let plan = recipe::plan_path(
        &path,
        recipe::Providers(&[&pc_image::recipe::PcRecipes]),
        recipe::plan::Sources {
            store: Some(&index),
            downloads: None,
            libraries: &mut [],
        },
    )
    .unwrap();
    let recipe::Planned::Plan(plan) = plan else {
        panic!("{plan:?}");
    };
    assert!(plan.complete(), "{}", plan.render());
    assert!(plan.uses[0].present);
    assert_eq!(plan.json()["uses"][0]["status"], "present");
    assert!(plan.render().contains("present in the store"));
    assert_eq!(plan.outputs.len(), 1);
    assert_eq!(plan.outputs[0].from, ["config", "use.dos"]);
    let prepared = recipe::template::prepare(
        recipe::build::Request {
            path: &path,
            providers: recipe::Providers(&[&pc_image::recipe::PcRecipes]),
            store: &store,
            index: Some(&index),
            store_read: None,
            libraries: &mut [],
            fetcher: None,
            keep_as: None,
            find_outputs: true,
        },
        &|_| Some(&unemu_pc_core::PcCore),
        None,
        &mut |_| {},
    )
    .unwrap();
    let recipe::template::Prepared::Ready {
        plan,
        facts,
        config: actual_config,
    } = prepared
    else {
        panic!("expected facts");
    };
    assert_eq!(actual_config.as_slice(), config);
    assert!(plan.complete());
    assert_eq!(facts.uses, [verified.facts]);
    assert_eq!(store.list_tags(None, 100).unwrap().tags.len(), 0);
    let manifest_path = store
        .blob_file_path(&descriptor.digest, descriptor.size)
        .unwrap();
    let mut bytes = fs::read(&manifest_path).unwrap();
    bytes[0] ^= 1;
    fs::write(manifest_path, bytes).unwrap();
    let refused = recipe::template::prepare(
        recipe::build::Request {
            path: &path,
            providers: recipe::Providers(&[&pc_image::recipe::PcRecipes]),
            store: &store,
            index: None,
            store_read: None,
            libraries: &mut [],
            fetcher: None,
            keep_as: None,
            find_outputs: true,
        },
        &|_| Some(&unemu_pc_core::PcCore),
        None,
        &mut |_| {},
    );
    assert!(refused.err().unwrap().contains("digest mismatch"));
}

#[test]
fn template_prepare_acquires_in_plan_order_only_with_consent_and_never_builds() {
    use recipe::{
        build::{Fetcher, Outcome, Request},
        provider::Provider,
        template::{Acquire, Prepared},
    };
    use std::cell::RefCell;
    struct NoUrls;
    impl Fetcher for NoUrls {
        fn fetch(&self, _: &str, _: u64, _: &mut dyn std::io::Write) -> Result<(), String> {
            panic!("bundled config needs no download")
        }
    }
    struct CopyPackages<'a> {
        store: &'a Store,
        calls: RefCell<Vec<String>>,
    }
    impl Acquire for CopyPackages<'_> {
        fn acquire(&self, target: &Store, usage: &UsePlan, platform: &str) -> Result<(), String> {
            assert_eq!(platform, "ibm-pc");
            self.calls.borrow_mut().push(usage.id.clone());
            let descriptor =
                recipe::template::descriptor(self.store, &usage.digest.parse().unwrap())
                    .unwrap()
                    .unwrap();
            let bytes = self.store.read_compatible_manifest(&descriptor).unwrap();
            let manifest = Manifest::decode_for_summary(&bytes).unwrap();
            for blob in std::iter::once(&manifest.config).chain(&manifest.layers) {
                let bytes = self
                    .store
                    .read_verified(blob, image_store::MAX_SMALL_BLOB)
                    .unwrap();
                target.import(bytes.as_slice(), Some(&blob.digest)).unwrap();
            }
            target
                .import(bytes.as_slice(), Some(&descriptor.digest))
                .unwrap();
            Ok(())
        }
    }
    let temp = image_store::test_support::TempDir::new("template-acquisition");
    let compatibility = pc_image::compat::compatibility(pc_image::compat::NEXT_RELEASE);
    let source = Store::open(temp.path().join("source"))
        .unwrap()
        .with_compatibility(compatibility.clone());
    let target = Store::open(temp.path().join("target"))
        .unwrap()
        .with_compatibility(compatibility);
    let (first, _, _) = media_package(&source, 41);
    let (second, _, _) = media_package(&source, 42);
    let path = temp.path().join("acquire.machine.toml");
    let config = b"unchanged opaque template";
    fs::write(temp.path().join("config.toml"), config).unwrap();
    fs::write(&path,format!("recipe='1.1'\n[machine]\nname='private/machines/ibm-pc/acquire'\nplatform='ibm-pc'\nversion='1.3.0'\n[template]\nconfig='config'\n[input.config]\nname='config.toml'\nsize={}\nsha256='{}'\nbundle=true\n[use.z]\nimage='private/media/ibm-pc/z:1-r1'\ndigest='{}'\n[use.a]\nimage='private/media/ibm-pc/a:1-r1'\ndigest='{}'\n",config.len(),Digest::of(config).hex(),second.digest,first.digest)).unwrap();
    let acquire = CopyPackages {
        store: &source,
        calls: RefCell::new(Vec::new()),
    };
    let providers: &[&dyn Provider] = &[&pc_image::recipe::PcRecipes];
    let run = |fetcher| {
        recipe::template::prepare(
            Request {
                path: &path,
                providers: recipe::Providers(providers),
                store: &target,
                index: None,
                store_read: None,
                libraries: &mut [],
                fetcher,
                keep_as: None,
                find_outputs: true,
            },
            &|_| Some(&unemu_pc_core::PcCore),
            Some(&acquire),
            &mut |_| {},
        )
        .unwrap()
    };
    let Prepared::Missing(plan) = run(None) else {
        panic!("expected missing uses without consent");
    };
    assert_eq!(plan.uses.len(), 2);
    assert!(acquire.calls.borrow().is_empty());
    let order: Vec<_> = plan.uses.iter().map(|u| u.id.clone()).collect();
    let Prepared::Ready {
        plan,
        facts,
        config: actual_config,
    } = run(Some(&NoUrls))
    else {
        panic!("expected acquired facts");
    };
    assert!(plan.complete());
    assert_eq!(*acquire.calls.borrow(), order);
    assert_eq!(actual_config.as_slice(), config);
    assert_eq!(facts.uses.len(), 2);
    assert_eq!(
        facts.uses.iter().map(|u| u.id.clone()).collect::<Vec<_>>(),
        order
    );
    let outcome = recipe::template::build_path(
        Request {
            path: &path,
            providers: recipe::Providers(providers),
            store: &target,
            index: None,
            store_read: None,
            libraries: &mut [],
            fetcher: None,
            keep_as: None,
            find_outputs: true,
        },
        &|_| Some(&unemu_pc_core::PcCore),
        None,
        &mut |_| {},
    )
    .unwrap();
    let Outcome::Failed { plan, error } = outcome else {
        panic!("expected construction refusal");
    };
    assert!(plan.complete());
    assert_eq!(error, "machine templates are not built yet");
    assert_eq!(target.list_tags(None, 100).unwrap().tags.len(), 0);
}
