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
    assert!(
        error.contains("config") || error.contains("TOML"),
        "{error}"
    );
    assert_eq!(target.list_tags(None, 100).unwrap().tags.len(), 0);
}

#[path = "support/template.rs"]
mod template_support;
use template_support::template_recipe;

#[test]
fn portable_template_builds_deterministically_and_opens_starts_reopens_and_exports() {
    use recipe::{
        build::{Outcome, Request},
        template::{build_path, compose, prepare, Prepared},
        Providers,
    };
    use unemu_core_api::{test_support::check_prepared, Core, RunLimit};
    use unemu_pc_core::PcCore;
    for source in &pc_image::template::FIRMWARE {
        for relative in [source.rom, source.source_archive] {
            if pc_harness::test_support::fixtures::fixture(relative).is_none() {
                return;
            }
        }
    }
    let temp = image_store::test_support::TempDir::new("portable-template");
    let mut digests = Vec::new();
    for index in 0..2 {
        let root = temp.path().join(format!("build-{index}"));
        fs::create_dir_all(&root).unwrap();
        let store = Store::open(root.join("store")).unwrap().with_compatibility(
            pc_image::compat::compatibility(pc_image::compat::NEXT_RELEASE),
        );
        let path = template_recipe(&store, &root);
        let providers = [&pc_image::recipe::PcRecipes as &dyn recipe::provider::Provider];
        let request = || Request {
            path: &path,
            providers: Providers(&providers),
            store: &store,
            index: None,
            store_read: None,
            libraries: &mut [],
            fetcher: None,
            keep_as: None,
            find_outputs: false,
        };
        let core_for = |_: &str| {
            Some(if index == 0 {
                &PcCore as &'static dyn Core
            } else {
                &PermutedPcCore as &'static dyn Core
            })
        };
        let Prepared::Ready {
            plan,
            mut facts,
            config,
        } = prepare(request(), &core_for, None, &mut |_| {}).unwrap()
        else {
            panic!("missing template inputs");
        };
        if index == 1 {
            facts.uses.reverse();
        }
        let document = unemu_core_api::ConfigDocument::new(pc_config::launch::MEDIA_TYPE, &config);
        let (prepared, _) = compose(
            &PcCore as &dyn Core,
            "ibm-pc",
            document,
            &plan.disks,
            &facts,
        )
        .unwrap();
        check_prepared(&PcCore as &dyn Core, "ibm-pc", &prepared).unwrap();
        let result = build_path(request(), &core_for, None, &mut |_| {}).unwrap();
        let Outcome::Built(report) = result else {
            panic!("{result:?}");
        };
        let image = store
            .resolve(&report.reference, &pc_image::PcProvider)
            .unwrap();
        let Outcome::Built(again) = build_path(request(), &core_for, None, &mut |_| {}).unwrap()
        else {
            panic!();
        };
        assert_eq!(
            again.published.descriptor.digest,
            report.published.descriptor.digest
        );
        assert!(!again.published.created);
        let source =
            pc_image::revision::Source::open(&store, &report.published.descriptor).unwrap();
        let mut settings = source.machine().clone();
        settings
            .get_mut("memory")
            .unwrap()
            .as_table_mut()
            .unwrap()
            .insert("extended_kib".into(), 2048.into());
        let revised = pc_image::revision::plan(&store, &source, settings).unwrap();
        let revision = pc_image::revision::publish(
            &store,
            &revised,
            "private/machines/ibm-pc/revised:1.3.0",
            None,
        )
        .unwrap();
        store
            .resolve_descriptor(&revision, &pc_image::PcProvider)
            .unwrap();
        assert_eq!(image.config, prepared.config.bytes);
        assert_eq!(
            image.manifest.config.media_type,
            pc_config::launch::MEDIA_TYPE
        );
        assert!(image
            .manifest
            .layers
            .windows(2)
            .all(|d| d[0].digest < d[1].digest));
        assert_eq!(
            image
                .manifest
                .layers
                .iter()
                .filter(|d| d.decoded_media_type() == mt::TAR)
                .count(),
            3
        );
        assert!(!image
            .manifest
            .layers
            .iter()
            .any(|d| d.decoded_media_type() == pc_image::DISK_ZERO));
        let portable = pc_config::launch::decode_portable(&image.config).unwrap();
        assert!(portable.config.media[0].head.is_none());
        assert_eq!(
            portable.config.media[0].zero.as_ref().unwrap().size,
            1048576
        );
        assert_eq!(
            portable
                .config
                .slots
                .iter()
                .filter(|s| s.inserted.is_none())
                .count(),
            2
        );
        let id = pc_image::machine::create(
            &store,
            &report.published.descriptor,
            &report.reference,
            Some("Portable"),
            None,
        )
        .unwrap();
        for cycle in 0..2 {
            let owner = pc_image::machine::own(&store, &id).unwrap();
            let prepared =
                pc_image::launch::prepare_machine(&owner, &PcCore, "Portable".into()).unwrap();
            let mut machine = core_launch::start(&prepared.prepared).unwrap();
            machine.run(RunLimit::steps(32)).unwrap();
            machine.finish().unwrap();
            drop(machine);
            drop(owner);
            if cycle == 0 {
                let path = store.workspace_path(&id).unwrap().join("media/system.upfj");
                let raw = vec![0; 1048576];
                let mut journal = journal_files::JournalMedia::open(
                    &path,
                    &raw,
                    raw.len() as u64,
                    Default::default(),
                )
                .unwrap();
                journal.write_batch(&[(0, [0x5a; 512])]);
                journal.finish().unwrap();
                let path = store.workspace_path(&id).unwrap().join("media/data.upfj");
                let mut journal = journal_files::JournalMedia::open(
                    &path,
                    &raw,
                    raw.len() as u64,
                    Default::default(),
                )
                .unwrap();
                journal.write_batch(&[(0, [0x6b; 512])]);
                journal.finish().unwrap();
            }
        }
        assert!(store
            .workspace_path(&id)
            .unwrap()
            .join("media/system.upfj")
            .is_file());
        pc_image::inspection::inspect(&store, &report.published.descriptor, Some(&id)).unwrap();
        pc_image::desktop::check(&store, &report.published.descriptor, Some(&id), || false)
            .unwrap();
        let snapshot = pc_image::machine::export::open(&store, &id).unwrap();
        let reader = snapshot.current(&store, "C:").unwrap();
        assert_eq!(reader.size(), 1048576);
        let mut bytes = [1; 512];
        reader.read_at(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0x5a; 512]);
        assert!(
            snapshot
                .media(&store)
                .unwrap()
                .iter()
                .filter(|m| m.bytes == 0)
                .count()
                >= 2
        );
        let descriptions = PcCore
            .describe_media("ibm-pc", snapshot.document())
            .unwrap();
        let slot = descriptions
            .iter()
            .find(|m| m.slot.label == "C:")
            .unwrap()
            .slot
            .id;
        let media = [unemu_core_api::ExportMedium::new(
            slot,
            reader.size(),
            reader.as_ref(),
        )];
        let mut sink = unemu_core_api::test_support::MemoryExportSink::new();
        let exported = PcCore
            .export(
                "ibm-pc",
                snapshot.document(),
                unemu_core_api::ExportRequest::new("raw", &media),
                &mut sink,
            )
            .unwrap();
        let outputs = sink.outputs();
        assert_eq!(outputs.len(), 1);
        let output = outputs.values().next().unwrap();
        assert_eq!(output.len(), 1048576);
        assert_eq!(&output[..512], &[0x5a; 512]);
        assert!(output[512..].iter().all(|b| *b == 0));
        let hash: String = exported.outputs[0]
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hash, Digest::of(output).hex());
        drop(reader);
        drop(snapshot);
        let saved = pc_image::machine::commit(
            &store,
            &id,
            "private/machines/ibm-pc/saved:1.3.0",
            None,
            None,
            false,
        )
        .unwrap();
        let saved_image = store
            .resolve_descriptor(&saved, &pc_image::PcProvider)
            .unwrap();
        assert_eq!(
            saved_image.manifest.config.media_type,
            pc_config::launch::MEDIA_TYPE
        );
        let snapshot = pc_image::machine::export::open(&store, &id).unwrap();
        let reader = snapshot.current(&store, "C:").unwrap();
        reader.read_at(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0x5a; 512]);
        let data = snapshot.current(&store, "D:").unwrap();
        data.read_at(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0x6b; 512]);
        assert_eq!(
            image.config,
            store
                .resolve_descriptor(&report.published.descriptor, &pc_image::PcProvider)
                .unwrap()
                .config
        );
        digests.push(report.published.descriptor.digest);
    }
    assert_eq!(digests[0], digests[1]);
}

#[test]
fn portable_stored_configs_refuse_session_records_bad_receipts_and_legacy_zero_disks() {
    use recipe::{
        build::{Outcome, Request},
        Providers,
    };
    for source in &pc_image::template::FIRMWARE {
        for relative in [source.rom, source.source_archive] {
            if pc_harness::test_support::fixtures::fixture(relative).is_none() {
                return;
            }
        }
    }
    let temp = image_store::test_support::TempDir::new("portable-refusals");
    let store = Store::open(temp.path().join("store"))
        .unwrap()
        .with_compatibility(pc_image::compat::compatibility(
            pc_image::compat::NEXT_RELEASE,
        ));
    let path = template_recipe(&store, temp.path());
    let providers = [&pc_image::recipe::PcRecipes as &dyn recipe::provider::Provider];
    let result = recipe::template::build_path(
        Request {
            path: &path,
            providers: Providers(&providers),
            store: &store,
            index: None,
            store_read: None,
            libraries: &mut [],
            fetcher: None,
            keep_as: None,
            find_outputs: false,
        },
        &|_| Some(&unemu_pc_core::PcCore),
        None,
        &mut |_| {},
    )
    .unwrap();
    let Outcome::Built(report) = result else {
        panic!("{result:?}");
    };
    let image = store
        .resolve(&report.reference, &pc_image::PcProvider)
        .unwrap();
    let original = pc_config::launch::decode_portable(&image.config).unwrap();
    for (case, expected) in [
        ("kept", "session-kept"),
        ("receipt", "receipts differ"),
        ("legacy", "unknown field"),
    ] {
        let mut launch = original.clone();
        if case == "kept" {
            launch.records[0].name = "kept-system.upfj".into();
        }
        if case == "receipt" {
            launch.content[0].sha256 = [0x99; 32];
        }
        let bytes = if case == "legacy" {
            toml::to_string(&launch.config).unwrap().into_bytes()
        } else {
            pc_config::launch::encode(&launch).unwrap()
        };
        let blob = store.import(bytes.as_slice(), None).unwrap();
        let mut manifest = image.manifest.clone();
        manifest.config = image_store::Descriptor::new(
            if case == "legacy" {
                mt::CONFIG
            } else {
                mt::PC_LAUNCH_CONFIG
            },
            blob.digest,
            blob.size,
        );
        let descriptor = store
            .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
            .unwrap();
        let error = store
            .resolve_descriptor(&descriptor, &pc_image::PcProvider)
            .unwrap_err()
            .to_string();
        if case == "legacy" {
            assert!(error.contains("zero disks without"), "{error}");
        } else {
            assert!(error.contains(expected), "{error}");
        }
    }
    assert_eq!(
        store.tag_descriptor(&report.reference).unwrap().digest,
        image.image.digest
    );
}

struct PermutedPcCore;
impl unemu_core_api::Core for PermutedPcCore {
    fn description(&self) -> &unemu_core_api::CoreDescription {
        unemu_pc_core::PcCore.description()
    }
    fn binding_slots(
        &self,
        platform: &str,
        config: unemu_core_api::ConfigDocument<'_>,
    ) -> Result<Vec<unemu_core_api::BindingSlot>, unemu_core_api::Error> {
        unemu_pc_core::PcCore.binding_slots(platform, config)
    }
    fn compose(
        &self,
        platform: &str,
        config: unemu_core_api::ConfigDocument<'_>,
        facts: unemu_core_api::BindingFacts<'_>,
    ) -> Result<unemu_core_api::PreparedConfig, unemu_core_api::Error> {
        let mut uses = facts.uses.to_vec();
        uses.reverse();
        for usage in &mut uses {
            usage.items.reverse();
            for item in &mut usage.items {
                item.roles.reverse();
            }
        }
        unemu_pc_core::PcCore.compose(
            platform,
            config,
            unemu_core_api::BindingFacts::new(&uses, facts.content),
        )
    }
    fn validate(
        &self,
        platform: &str,
        config: unemu_core_api::ConfigDocument<'_>,
    ) -> Result<unemu_core_api::Validated, unemu_core_api::Error> {
        unemu_pc_core::PcCore.validate(platform, config)
    }
    fn create(
        &self,
        request: &unemu_core_api::CreateRequest<'_>,
    ) -> Result<Box<dyn unemu_core_api::Machine>, unemu_core_api::Error> {
        unemu_pc_core::PcCore.create(request)
    }
}

#[test]
fn portable_template_retains_referenced_media_bytes_and_the_packages_terms() {
    use recipe::{
        build::{Outcome, Request},
        Providers,
    };
    use unemu_core_api::{Core, RunLimit};
    for source in &pc_image::template::FIRMWARE {
        for relative in [source.rom, source.source_archive] {
            if pc_harness::test_support::fixtures::fixture(relative).is_none() {
                return;
            }
        }
    }
    let temp = image_store::test_support::TempDir::new("portable-media");
    let store = Store::open(temp.path().join("store"))
        .unwrap()
        .with_compatibility(pc_image::compat::compatibility(
            pc_image::compat::NEXT_RELEASE,
        ));
    let path = template_recipe(&store, temp.path());
    let (_, mut manifest, bytes) = media_package(&store, 42);
    let terms = b"Synthetic media terms.\n".repeat(256);
    let mut terms_descriptor = store
        .import_payload(
            terms.as_slice(),
            mt::EXTRA,
            terms.len() as u64,
            None,
            image_store::codec::MEDIA_PAYLOAD_TYPES,
        )
        .unwrap();
    assert!(terms_descriptor.is_compressed());
    let terms_digest = terms_descriptor.decoded_digest().unwrap();
    image_store::license::mark(
        &mut terms_descriptor,
        Some(&image_store::license::Expression::parse("CC0-1.0").unwrap()),
    );
    manifest.layers[0]
        .annotations
        .insert(image_store::license::TERMS.into(), terms_digest.to_string());
    let mut config = image_store::media_set::Config::decode(
        &store
            .read_verified(&manifest.config, image_store::MAX_SMALL_BLOB)
            .unwrap(),
    )
    .unwrap();
    config.extras.push(image_store::media_set::Extra {
        blob: terms_digest,
        filename: "TERMS.TXT".into(),
        size: terms.len() as u64,
        kind: image_store::media_set::ExtraKind::Document,
        dos_name: Some("TERMS.TXT".into()),
    });
    manifest.layers.insert(1, terms_descriptor.clone());
    let config = config.encode().unwrap();
    let blob = store.import(config.as_slice(), None).unwrap();
    manifest.config = image_store::Descriptor::new(mt::MEDIA_SET_CONFIG, blob.digest, blob.size);
    let package = store
        .import_manifest_with_types(&manifest, image_store::codec::MEDIA_PAYLOAD_TYPES)
        .unwrap();
    let old = template_support::bound_config();
    let text = std::str::from_utf8(&old).unwrap();
    let (before, after) = text.rsplit_once("empty = true").unwrap();
    let new = format!("{before}inserted = {{ use = \"dos\", medium = \"disk-1\" }}{after}");
    fs::write(temp.path().join("bound.toml"), &new).unwrap();
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(Digest::of(&old).hex(), Digest::of(new.as_bytes()).hex())
        .replace(
            &format!("size = {}\n", old.len()),
            &format!("size = {}\n", new.len()),
        );
    fs::write(
        &path,
        format!(
            "{text}\n[use.dos]\nimage = \"private/media/ibm-pc/disks:1-r1\"\ndigest = \"{}\"\n",
            package.digest
        ),
    )
    .unwrap();
    let providers = [&pc_image::recipe::PcRecipes as &dyn recipe::provider::Provider];
    let result = recipe::template::build_path(
        Request {
            path: &path,
            providers: Providers(&providers),
            store: &store,
            index: None,
            store_read: None,
            libraries: &mut [],
            fetcher: None,
            keep_as: None,
            find_outputs: false,
        },
        &|_| Some(&unemu_pc_core::PcCore as &'static dyn Core),
        None,
        &mut |_| {},
    )
    .unwrap();
    let Outcome::Built(report) = result else {
        panic!("{result:?}");
    };
    let image = store
        .resolve(&report.reference, &pc_image::PcProvider)
        .unwrap();
    assert!(image.manifest.layers.contains(&manifest.layers[0]));
    assert!(image.manifest.layers.contains(&terms_descriptor));
    assert!(image.manifest.annotations["unemu.template.uses"].contains(&package.digest.to_string()));
    let id = pc_image::machine::create(
        &store,
        &report.published.descriptor,
        &report.reference,
        Some("Media"),
        None,
    )
    .unwrap();
    let owner = pc_image::machine::own(&store, &id).unwrap();
    let prepared =
        pc_image::launch::prepare_machine(&owner, &unemu_pc_core::PcCore, "Media".into()).unwrap();
    let mut machine = core_launch::start(&prepared.prepared).unwrap();
    machine.run(RunLimit::steps(8)).unwrap();
    machine.finish().unwrap();
    drop(machine);
    drop(owner);
    let snapshot = pc_image::machine::export::open(&store, &id).unwrap();
    let reader = snapshot.current(&store, "B:").unwrap();
    assert_eq!(reader.size(), bytes.len() as u64);
    let mut actual = [0; 512];
    reader.read_at(0, &mut actual).unwrap();
    assert_eq!(actual, [42; 512]);
    drop(reader);
    drop(snapshot);
    let mut other = manifest.clone();
    image_store::license::mark(
        &mut other.layers[0],
        Some(&image_store::license::Expression::parse("MIT").unwrap()),
    );
    let other = store
        .import_manifest_with_types(&other, image_store::codec::MEDIA_PAYLOAD_TYPES)
        .unwrap();
    let conflicting = new.replace("type = \"1.2M\"", "type = \"1.44M\"").replacen(
        "empty = true",
        "inserted = { use = \"other\", medium = \"disk-1\" }",
        1,
    );
    let conflicting = conflicting.replace("inserted = {", "write = \"readonly\"\ninserted = {");
    fs::write(temp.path().join("bound.toml"), &conflicting).unwrap();
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(
            Digest::of(new.as_bytes()).hex(),
            Digest::of(conflicting.as_bytes()).hex(),
        )
        .replace(
            &format!("size = {}\n", new.len()),
            &format!("size = {}\n", conflicting.len()),
        )
        .replace("version = \"1.3.0\"", "version = \"1.3.1\"");
    fs::write(
        &path,
        format!(
            "{text}\n[use.other]\nimage = \"private/media/ibm-pc/other:1-r1\"\ndigest = \"{}\"\n",
            other.digest
        ),
    )
    .unwrap();
    let result = recipe::template::build_path(
        Request {
            path: &path,
            providers: Providers(&providers),
            store: &store,
            index: None,
            store_read: None,
            libraries: &mut [],
            fetcher: None,
            keep_as: None,
            find_outputs: false,
        },
        &|_| Some(&unemu_pc_core::PcCore as &'static dyn Core),
        None,
        &mut |_| {},
    )
    .unwrap();
    let Outcome::Failed { error, .. } = result else {
        panic!("{result:?}");
    };
    assert!(error.contains("conflicting descriptors"), "{error}");
    assert!(store
        .tag_descriptor("private/machines/ibm-pc/bound:1.3.1")
        .is_err());
}
