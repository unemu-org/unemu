// SPDX-License-Identifier: AGPL-3.0-or-later
//! Official recipe inputs and machine equivalence across config encodings.
use image_store::{
    media_type as mt, BlobSource, Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB,
};
use pc_image::template::{self, inputs};
use std::{collections::BTreeMap, fs};
use toml::{Table, Value};

use unemu_core_api::{
    BindingFacts, BindingSlot, ConfigDocument, Core, CoreDescription, CreateRequest, Error,
    PreparedConfig,
};

struct ConstructionOnly;
impl Core for ConstructionOnly {
    fn description(&self) -> &CoreDescription {
        unemu_pc_core::PcCore.description()
    }
    fn create(&self, _: &CreateRequest<'_>) -> Result<Box<dyn unemu_core_api::Machine>, Error> {
        panic!("recipe construction must not create or execute a guest")
    }
    fn binding_slots(
        &self,
        platform: &str,
        config: ConfigDocument<'_>,
    ) -> Result<Vec<BindingSlot>, Error> {
        unemu_pc_core::PcCore.binding_slots(platform, config)
    }
    fn compose(
        &self,
        platform: &str,
        config: ConfigDocument<'_>,
        facts: BindingFacts<'_>,
    ) -> Result<PreparedConfig, Error> {
        unemu_pc_core::PcCore.compose(platform, config, facts)
    }
}

#[derive(Debug, PartialEq)]
struct Machine {
    hardware: Table,
    firmware: Vec<(String, String, Digest, Option<Digest>)>,
    media: Vec<Value>,
    slots: Vec<Value>,
    nvram: Option<Value>,
    archival_rights: BTreeMap<Digest, Descriptor>,
}

fn document(store: &Store, descriptor: &Descriptor) -> (Manifest, Vec<u8>) {
    let manifest =
        Manifest::decode_for_summary(&store.read_compatible_manifest(descriptor).unwrap()).unwrap();
    let bytes = store
        .read_verified(&manifest.config, MAX_SMALL_BLOB)
        .unwrap();
    (manifest, bytes)
}

fn neutral(store: &Store, descriptor: &Descriptor) -> Machine {
    let (manifest, bytes) = document(store, descriptor);
    let mut config = if manifest.config.media_type == mt::PC_LAUNCH_CONFIG {
        pc_config::launch::decode_portable(&bytes).unwrap().config
    } else {
        pc_config::image::Config::decode(
            toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap(),
        )
        .unwrap()
    };
    let mut firmware = Vec::new();
    let mut archival_rights = BTreeMap::new();
    pc_config::image::firmware_images::<pc_config::image::Error>(
        &mut config.machine,
        |key, rom| {
            let binding = config.firmware.iter().find(|f| f.key == key).unwrap();
            let digest: Digest = binding.blob.to_string().parse().unwrap();
            let layer = manifest.layers.iter().find(|d| d.names(&digest)).unwrap();
            let bytes = store.read_verified(layer, MAX_SMALL_BLOB).unwrap();
            assert_eq!(Digest::of(&bytes), digest);
            let entry = pc_image::firmware::catalogue()
                .iter()
                .find(|e| e.sha256 == digest.hex())
                .unwrap();
            let source = layer
                .annotations
                .get(image_store::license::SOURCE)
                .map(|s| s.parse::<Digest>().unwrap());
            if let Some(link) = binding.source_archive {
                assert_eq!(Some(link.to_string().parse().unwrap()), source);
            }
            firmware.push((
                key.into(),
                pc_image::firmware::role(entry.kind).into(),
                digest.clone(),
                source.clone(),
            ));
            let mut pending = vec![digest.clone()];
            while let Some(id) = pending.pop() {
                if archival_rights.contains_key(&id) {
                    continue;
                }
                let d = manifest.layers.iter().find(|d| d.names(&id)).unwrap();
                store.read_verified(d, d.decoded_size().unwrap()).unwrap();
                for key in [image_store::license::SOURCE, image_store::license::TERMS] {
                    if let Some(link) = d.annotations.get(key) {
                        pending.push(link.parse().unwrap());
                    }
                }
                archival_rights.insert(id, d.clone());
            }
            rom.insert("image".into(), digest.to_string().into());
            Ok(())
        },
    )
    .unwrap();
    let mut media = Vec::new();
    for (index, medium) in config.media.iter().enumerate() {
        let size = if let Some(zero) = &medium.zero {
            zero.size
        } else {
            let head: Digest = medium.head.unwrap().to_string().parse().unwrap();
            let d = manifest.layers.iter().find(|d| d.names(&head)).unwrap();
            assert_eq!(d.media_type, pc_image::zero::MEDIA_TYPE);
            let zero =
                pc_image::zero::ZeroBase::decode(&store.read_verified(d, MAX_SMALL_BLOB).unwrap())
                    .unwrap();
            assert_eq!(zero.geometry.cylinders, medium.geometry.cylinders);
            assert_eq!(zero.geometry.heads, medium.geometry.heads);
            assert_eq!(zero.geometry.sectors, medium.geometry.sectors);
            assert_eq!(zero.geometry.sector_bytes, medium.geometry.sector_bytes);
            zero.byte_len()
        };
        assert_eq!(
            size,
            u64::from(medium.geometry.cylinders)
                * u64::from(medium.geometry.heads)
                * u64::from(medium.geometry.sectors)
                * u64::from(medium.geometry.sector_bytes)
        );
        let mut value = Value::try_from(medium).unwrap();
        let table = value.as_table_mut().unwrap();
        for key in ["source", "head", "zero"] {
            table.remove(key);
        }
        table.insert("id".into(), (index as i64).into());
        table.insert("size".into(), (size as i64).into());
        media.push(value);
    }
    let slots = config
        .slots
        .iter()
        .map(|slot| {
            let mut value = Value::try_from(slot).unwrap();
            let table = value.as_table_mut().unwrap();
            let id = |name: &str| {
                Value::Integer(config.media.iter().position(|m| m.id == name).unwrap() as i64)
            };
            table.insert(
                "catalogue".into(),
                slot.catalogue
                    .iter()
                    .map(|s| id(s))
                    .collect::<Vec<_>>()
                    .into(),
            );
            if let Some(inserted) = &slot.inserted {
                table.insert("inserted".into(), id(inserted));
            }
            value
        })
        .collect();
    Machine {
        hardware: config.machine,
        firmware,
        media,
        slots,
        nvram: config.nvram.as_ref().map(|n| Value::try_from(n).unwrap()),
        archival_rights,
    }
}

#[test]
fn emitted_documents_are_pinned_complete_drafts() {
    let first = inputs::emit().unwrap();
    assert_eq!(first, inputs::emit().unwrap());
    for pair in first.as_chunks::<2>().0 {
        let config: Table = toml::from_str(std::str::from_utf8(&pair[0].bytes).unwrap()).unwrap();
        assert_eq!(config["schema"].as_integer(), Some(1));
        assert_eq!(config["machine"]["schema"].as_integer(), Some(1));
        let slots = ConstructionOnly
            .binding_slots(
                pc_image::PLATFORM,
                ConfigDocument::new(pair[0].media_type, &pair[0].bytes),
            )
            .unwrap();
        assert_eq!(slots.len(), 4);
        assert_eq!(
            slots
                .iter()
                .filter(|s| s.kind == unemu_core_api::BindingKind::ZeroDisk)
                .count(),
            1
        );
        let recipe: Table = toml::from_str(std::str::from_utf8(&pair[1].bytes).unwrap()).unwrap();
        assert_eq!(recipe["recipe"].as_str(), Some("1.1"));
        assert_eq!(recipe["machine"]["version"].as_str(), Some("1.3.0"));
        assert!(recipe["machine"].get("digest").is_none());
        assert_eq!(
            recipe["input"]["config"]["name"].as_str(),
            Some(pair[0].name.as_str())
        );
        assert_eq!(
            recipe["input"]["config"]["size"].as_integer(),
            Some(pair[0].bytes.len() as i64)
        );
        assert_eq!(
            recipe["input"]["config"]["sha256"].as_str(),
            Some(Digest::of(&pair[0].bytes).hex())
        );
    }
    let expected = [
        "c4411aa29a385d3a91efea1ae3b911f3c906069c5aff0d8f612cd9b77eccc002",
        "6f254e83e6275152e9532f5ad704667ecd7b4cbbf1af48a8b0a383a044acc583",
        "0a83a4be46e2744ebec4f955fbb6ccb4ce23673eb9f977145e2d347f8bd992da",
        "927981a11b83d378248eae71d77ab2b39e4923bcb3d8b48a56e33b5769b170a0",
    ];
    for (document, expected) in first.iter().zip(expected) {
        println!(
            "{} {} {} {}",
            document.name,
            document.bytes.len(),
            Digest::of(&document.bytes),
            document.media_type
        );
        assert_eq!(
            Digest::of(&document.bytes).hex(),
            expected,
            "{}",
            document.name
        );
    }
}

#[test]
fn official_recipes_build_twice_and_equal_the_historical_machines() {
    for source in template::FIRMWARE {
        for path in [source.rom, source.source_archive] {
            if pc_harness::test_support::fixtures::fixture(path).is_none() {
                return;
            }
        }
    }
    let temp = image_store::test_support::TempDir::new("official-template-recipes");
    let mut results = Vec::new();
    for attempt in 0..2 {
        let root = temp.path().join(format!("attempt-{attempt}"));
        fs::create_dir_all(&root).unwrap();
        let store = Store::open(root.join("store")).unwrap().with_compatibility(
            pc_image::compat::compatibility(pc_image::compat::NEXT_RELEASE),
        );
        let historical = template::build(
            &store,
            &[pc_harness::test_support::root().join("vendor")],
            &|_, _, _| Ok(()),
            &mut |_| {},
        )
        .unwrap();
        for document in inputs::emit().unwrap() {
            fs::write(root.join(document.name), document.bytes).unwrap();
        }
        let mut digests = Vec::new();
        for (template, old) in template::TEMPLATES.iter().zip(&historical) {
            let path = root.join(format!("{}.machine.toml", template.name));
            let providers = [&pc_image::recipe::PcRecipes as &dyn recipe::provider::Provider];
            let result = recipe::template::build_path(
                recipe::build::Request {
                    path: &path,
                    providers: recipe::Providers(&providers),
                    store: &store,
                    index: None,
                    store_read: None,
                    libraries: &mut [],
                    fetcher: None,
                    keep_as: None,
                    find_outputs: true,
                },
                &|_| Some(&ConstructionOnly),
                None,
                &mut |_| {},
            )
            .unwrap();
            let recipe::build::Outcome::Built(report) = result else {
                panic!("{result:?}")
            };
            let new = &report.published.descriptor;
            println!("{} attempt={attempt} {}", template.name, new.digest);
            assert_ne!(new.digest, old.descriptor.digest);
            let expected = neutral(&store, &old.descriptor);
            assert_eq!(expected.firmware.len(), 3);
            assert_eq!(expected.media.len(), 1);
            assert_eq!(expected.slots.len(), 3);
            assert_eq!(expected.archival_rights.len(), 6);
            assert_eq!(expected, neutral(&store, new));
            let (manifest, _) = document(&store, new);
            assert_eq!(manifest.config.media_type, mt::PC_LAUNCH_CONFIG);
            assert_eq!(
                manifest.annotations[image_store::compat::MIN_FRONTEND_VERSION],
                pc_image::compat::NEXT_RELEASE.to_string()
            );
            assert!(manifest
                .layers
                .windows(2)
                .all(|d| d[0].digest < d[1].digest));
            digests.push(new.digest.clone());
        }
        results.push(digests);
    }
    assert_eq!(results[0], results[1]);
}
