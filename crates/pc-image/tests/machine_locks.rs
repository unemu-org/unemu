// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine recipe locks and publication at the store boundary.
use image_store::{BlobSource, Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB};
use recipe::build::{Outcome, Request};
use std::{collections::BTreeSet, fs, path::Path};

#[path = "support/template.rs"]
mod support;

fn fixtures() -> bool {
    pc_image::template::FIRMWARE.iter().all(|source| {
        [source.rom, source.source_archive]
            .iter()
            .all(|path| pc_harness::test_support::fixtures::fixture(path).is_some())
    })
}
fn store(path: &Path) -> Store {
    Store::open(path)
        .unwrap()
        .with_compatibility(pc_image::compat::compatibility(
            pc_image::compat::NEXT_RELEASE,
        ))
}
fn build(path: &Path, store: &Store) -> Result<Box<recipe::build::Report>, String> {
    match recipe::template::build_path(
        Request {
            path,
            providers: recipe::Providers(&[&pc_image::recipe::PcRecipes]),
            store,
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
    ) {
        Ok(Outcome::Built(report)) => Ok(report),
        other => Err(format!("{other:?}")),
    }
}
fn manifest(store: &Store, descriptor: &Descriptor) -> Manifest {
    Manifest::decode_for_summary(&store.read_verified(descriptor, MAX_SMALL_BLOB).unwrap()).unwrap()
}
fn copy_packages(source: &Store, target: &Store) {
    for tag in source.list_tags(None, 100).unwrap().tags {
        let m = manifest(source, &tag.manifest);
        if m.artifact_type() != image_store::media_type::FIRMWARE {
            continue;
        }
        for d in std::iter::once(&tag.manifest)
            .chain(std::iter::once(&m.config))
            .chain(&m.layers)
        {
            let bytes = fs::read(source.blob_file_path(&d.digest, d.size).unwrap()).unwrap();
            target.import(bytes.as_slice(), Some(&d.digest)).unwrap();
        }
    }
}
fn pin(path: &Path, source: &Store) -> (Descriptor, Vec<Vec<u8>>) {
    let built = build(path, source).unwrap();
    let d = built.published.descriptor;
    let files = recipe::lock::files_of(source, &d, &BTreeSet::new()).unwrap();
    assert_eq!(
        files.len(),
        3,
        "machine locks must never carry package payloads"
    );
    recipe::lock::write(&recipe::lock::dir(path), &files).unwrap();
    let text = fs::read_to_string(path).unwrap().replace(
        "version = \"1.3.0\"",
        &format!("version = \"1.3.0\"\ndigest = \"{}\"", d.digest),
    );
    fs::write(path, text).unwrap();
    (d, files)
}

#[test]
fn machine_three_file_lock_round_trip_bundle_and_lost_versus_broken_lock() {
    if !fixtures() {
        return;
    }
    let temp = image_store::test_support::TempDir::new("machine-locks");
    let source = store(&temp.path().join("source"));
    let path = support::template_recipe(&source, temp.path());
    let (d, files) = pin(&path, &source);
    let target = store(&temp.path().join("target"));
    copy_packages(&source, &target);
    let rebuilt = build(&path, &target).unwrap();
    assert!(rebuilt.locked);
    assert_eq!(rebuilt.published.descriptor, d);
    assert_eq!(
        recipe::lock::files_of(&target, &d, &BTreeSet::new()).unwrap(),
        files
    );
    let providers = recipe::Providers(&[&pc_image::recipe::PcRecipes]);
    let artifact = recipe::artifact::bundle_with_levels(
        &path,
        providers,
        pc_image::compat::levels(pc_image::compat::NEXT_RELEASE),
    )
    .unwrap();
    assert_eq!(
        artifact.manifest.artifact_type(),
        "application/vnd.unemu.recipe.machine.v1"
    );
    assert_eq!(
        artifact
            .manifest
            .layers
            .iter()
            .filter(|d| d.media_type == image_store::media_type::RECIPE_LOCK)
            .count(),
        3
    );
    let pulled = temp.path().join("pulled");
    let inspected = recipe::artifact::inspect(&artifact, providers, &pulled).unwrap();
    assert_eq!(
        build(&inspected.path, &target)
            .unwrap()
            .published
            .descriptor,
        d
    );
    let scratch = temp.path().join("guard");
    assert!(recipe::artifact::guard(
        &artifact,
        providers,
        "private/recipes/machines/ibm-pc/bound",
        "latest",
        &scratch
    )
    .is_err());
    recipe::artifact::guard(
        &artifact,
        providers,
        "private/recipes/machines/ibm-pc/bound",
        "1.3.0",
        &temp.path().join("guard-valid"),
    )
    .unwrap();
    for reserved in ["media", "firmware", "machines", "recipes"] {
        assert!(recipe::artifact::guard(
            &artifact,
            providers,
            &format!("private/{reserved}/recipes/machines/ibm-pc/bound"),
            "1.3.0",
            &temp.path().join(format!("guard-{reserved}"))
        )
        .is_err());
    }
    fs::remove_dir_all(recipe::lock::dir(&path)).unwrap();
    let lost = build(&path, &target).unwrap();
    assert!(lost.lock_rebuilt);
    assert_eq!(lost.published.descriptor, d);
    fs::create_dir_all(recipe::lock::dir(&path)).unwrap();
    let broken = build(&path, &target).unwrap_err();
    assert!(broken.contains("sha256"), "{broken}");
}

fn relock(
    path: &Path,
    old: &Digest,
    m: &Manifest,
    files: &[Vec<u8>],
    replace: Option<(&Digest, Vec<u8>)>,
    noncanonical: bool,
) -> Digest {
    let mut bytes = m.encode_with_types(pc_image::PAYLOAD_TYPES).unwrap();
    if noncanonical {
        bytes.push(b'\n');
    }
    let digest = Digest::of(&bytes);
    let mut updated = vec![bytes];
    for bytes in files.iter().skip(1) {
        if let Some((old, new)) = &replace {
            if Digest::of(bytes) == **old {
                updated.push(new.clone());
                continue;
            }
        }
        updated.push(bytes.clone());
    }
    recipe::lock::write(&recipe::lock::dir(path), &updated).unwrap();
    let text = fs::read_to_string(path)
        .unwrap()
        .replace(&old.to_string(), &digest.to_string());
    fs::write(path, text).unwrap();
    digest
}

#[test]
fn machine_lock_refuses_each_changed_part_and_never_tags_a_failure() {
    if !fixtures() {
        return;
    }
    let temp = image_store::test_support::TempDir::new("machine-lock-changes");
    let source = store(&temp.path().join("source"));
    let path = support::template_recipe(&source, temp.path());
    let (d, files) = pin(&path, &source);
    let original = fs::read_to_string(&path).unwrap();
    for part in ["config", "metadata", "descriptor", "closure"] {
        fs::write(&path, &original).unwrap();
        let mut m = manifest(&source, &d);
        let replacement = match part {
            "config" | "metadata" => {
                let descriptor = if part == "config" {
                    &mut m.config
                } else {
                    m.layers
                        .iter_mut()
                        .find(|d| d.media_type == image_store::media_type::METADATA)
                        .unwrap()
                };
                let old = descriptor.digest.clone();
                let mut bytes = source.read_verified(descriptor, MAX_SMALL_BLOB).unwrap();
                bytes.push(b'\n');
                descriptor.digest = Digest::of(&bytes);
                descriptor.size = bytes.len() as u64;
                Some((old, bytes))
            }
            "descriptor" => {
                m.layers[0]
                    .annotations
                    .insert("org.opencontainers.image.title".into(), "changed".into());
                None
            }
            "closure" => {
                let index = m
                    .layers
                    .iter()
                    .position(|d| d.decoded_media_type() == image_store::media_type::MARKDOWN)
                    .unwrap();
                m.layers.remove(index);
                None
            }
            _ => unreachable!(),
        };
        relock(
            &path,
            &d.digest,
            &m,
            &files,
            replacement
                .as_ref()
                .map(|(digest, bytes)| (digest, bytes.clone())),
            false,
        );
        let target = store(&temp.path().join(part));
        copy_packages(&source, &target);
        let error = build(&path, &target).unwrap_err();
        assert!(error.contains("lock"), "{part}: {error}");
        assert!(
            target
                .tag_descriptor("private/machines/ibm-pc/bound:1.3.0")
                .is_err(),
            "{part}"
        );
    }
    fs::write(&path, &original).unwrap();
    fs::remove_dir_all(recipe::lock::dir(&path)).unwrap();
    fs::write(
        &path,
        original.replace(&d.digest.to_string(), &format!("sha256:{}", "0".repeat(64))),
    )
    .unwrap();
    let target = store(&temp.path().join("lost-mismatch"));
    copy_packages(&source, &target);
    assert!(build(&path, &target)
        .unwrap_err()
        .contains("needs its lock"));
    assert!(target
        .tag_descriptor("private/machines/ibm-pc/bound:1.3.0")
        .is_err());
}

#[test]
fn machine_locked_order_and_manifest_bytes_win_without_restamping() {
    if !fixtures() {
        return;
    }
    let temp = image_store::test_support::TempDir::new("machine-lock-order");
    let source = store(&temp.path().join("source"));
    let path = support::template_recipe(&source, temp.path());
    let (d, files) = pin(&path, &source);
    let mut m = manifest(&source, &d);
    m.layers.reverse();
    let digest = relock(&path, &d.digest, &m, &files, None, true);
    let target = store(&temp.path().join("target"));
    copy_packages(&source, &target);
    let built = build(&path, &target).unwrap();
    assert_eq!(built.published.descriptor.digest, digest);
    assert_eq!(manifest(&target, &built.published.descriptor), m);
    assert_eq!(
        target
            .read_verified(&built.published.descriptor, MAX_SMALL_BLOB)
            .unwrap(),
        recipe::lock::read(&recipe::lock::dir(&path)).unwrap()[digest.hex()]
    );
    assert_eq!(
        recipe::lock::read(&recipe::lock::dir(&path)).unwrap().len(),
        3
    );
    let mut blocks = fs::read_to_string(&path)
        .unwrap()
        .split("\n[use.")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let head = blocks.remove(0);
    blocks.reverse();
    fs::write(
        &path,
        head + &blocks
            .into_iter()
            .map(|b| "\n[use.".to_owned() + &b)
            .collect::<String>(),
    )
    .unwrap();
    assert_eq!(
        build(&path, &target).unwrap().published.descriptor.digest,
        digest
    );
    let artifact = recipe::artifact::bundle_with_levels(
        &path,
        recipe::Providers(&[&pc_image::recipe::PcRecipes]),
        pc_image::compat::levels(pc_image::compat::NEXT_RELEASE),
    )
    .unwrap();
    assert!(artifact.parts().any(|(d, bytes)| d.media_type
        == image_store::media_type::RECIPE_LOCK
        && bytes.ends_with(b"}\n")));
}

#[test]
fn official_machine_guard_retains_imported_licenses_and_refuses_before_tagging() {
    if !fixtures() {
        return;
    }
    let temp = image_store::test_support::TempDir::new("machine-guard");
    let source = store(&temp.path().join("source"));
    let path = support::template_recipe(&source, temp.path());
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("private/machines/", "unemu/team/machines/");
    fs::write(&path, &text).unwrap();
    let d = build(&path, &source).unwrap().published.descriptor;
    let original = manifest(&source, &d);
    let recipe: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    {
        for usage in recipe["use"].as_table().unwrap().values() {
            let package = recipe::template::descriptor(
                &source,
                &usage["digest"].as_str().unwrap().parse().unwrap(),
            )
            .unwrap()
            .unwrap();
            let package = manifest(&source, &package);
            for imported in &original.layers {
                if let Some(expected) = package.layers.iter().find(|p| p.digest == imported.digest)
                {
                    assert_eq!(
                        imported, expected,
                        "imported rights must not be replaced by the generated-content license"
                    );
                }
            }
        }
    }
    for fault in ["closed", "source"] {
        let mut changed = original.clone();
        let rom = changed
            .layers
            .iter_mut()
            .find(|d| {
                d.decoded_media_type() == image_store::media_type::FIRMWARE
                    && image_store::license::open_license(d)
                        .unwrap()
                        .needs_source(image_store::license::ACTIVE)
            })
            .unwrap();
        if fault == "closed" {
            rom.annotations.remove(image_store::license::LICENSE);
        } else {
            let source: Digest = rom
                .annotations
                .remove(image_store::license::SOURCE)
                .unwrap()
                .parse()
                .unwrap();
            changed.layers.retain(|d| !d.names(&source));
        }
        let reference = format!("unemu/team/machines/ibm-pc/{fault}:1.3.0");
        let error = pc_image::recipe::template_facts::tag(&source, &reference, &changed)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(if fault == "closed" {
                "every blob"
            } else {
                "source archives"
            }),
            "{error}"
        );
        assert!(source.tag_descriptor(&reference).is_err());
    }
    let mut changed = original.clone();
    changed.annotations.insert(
        "org.opencontainers.image.title".into(),
        "Another title".into(),
    );
    assert!(pc_image::recipe::template_facts::tag(
        &source,
        "unemu/team/machines/ibm-pc/bound:1.3.0",
        &changed
    )
    .is_err());
    assert_eq!(
        source
            .tag_descriptor("unemu/team/machines/ibm-pc/bound:1.3.0")
            .unwrap()
            .digest,
        d.digest
    );
}

#[test]
fn machine_core_inheritance_and_preparation_frontend_floors_are_independent() {
    if !fixtures() {
        return;
    }
    use image_store::compat::{
        AppVersion as V, Compatibility, Requirement, Requirements, MIN_APP_VERSION,
        MIN_FRONTEND_VERSION,
    };
    let temp = image_store::test_support::TempDir::new("machine-floors");
    let source = Store::open(temp.path().join("store"))
        .unwrap()
        .with_compatibility(
            Compatibility::new(V::new(0, 0, 9), pc_image::compat::NEXT_RELEASE).with_core(
                "ibm-pc",
                V::new(0, 0, 8),
                pc_image::compat::core_newest_row(),
            ),
        );
    let path = support::template_recipe(&source, temp.path());
    let mut text = fs::read_to_string(&path).unwrap();
    let parsed: toml::Table = toml::from_str(&text).unwrap();
    let old: Digest = parsed["use"]["bios"]["digest"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let old_descriptor = recipe::template::descriptor(&source, &old)
        .unwrap()
        .unwrap();
    let mut package = manifest(&source, &old_descriptor);
    package
        .annotations
        .insert(MIN_APP_VERSION.into(), "0.0.7".into());
    package
        .annotations
        .insert(MIN_FRONTEND_VERSION.into(), "0.0.9".into());
    let changed = source
        .import_manifest_with_types(&package, image_store::codec::FIRMWARE_PAYLOAD_TYPES)
        .unwrap();
    text = text.replace(&old.to_string(), &changed.digest.to_string());
    fs::write(&path, &text).unwrap();
    let built = build(&path, &source).unwrap();
    let floors = Requirements::of(&manifest(&source, &built.published.descriptor));
    assert_eq!(floors.core, Requirement::Release(V::new(0, 0, 7)));
    assert_eq!(
        floors.frontend,
        Requirement::Release(pc_image::compat::NEXT_RELEASE)
    );
    let pinned = pin(&path, &source);
    let artifact = recipe::artifact::bundle_with_levels(
        &path,
        recipe::Providers(&[&pc_image::recipe::PcRecipes]),
        source.compatibility_levels("ibm-pc"),
    )
    .unwrap();
    assert_eq!(Requirements::of(&artifact.manifest), floors);
    assert_eq!(pinned.0, built.published.descriptor);
    fs::write(
        &path,
        text.replace(
            "version = \"1.3.0\"",
            "version = \"1.3.0\"\nmin_app_version = \"0.0.8\"",
        )
        .replace("/bound\"", "/raised\""),
    )
    .unwrap();
    let raised = build(&path, &source).unwrap();
    let raised = Requirements::of(&manifest(&source, &raised.published.descriptor));
    assert_eq!(raised.core, Requirement::Release(V::new(0, 0, 8)));
    assert_eq!(raised.frontend, floors.frontend);
    fs::write(
        &path,
        text.replace(
            "version = \"1.3.0\"",
            "version = \"1.3.0\"\nmin_app_version = \"0.0.4\"",
        )
        .replace("/bound\"", "/lowered\""),
    )
    .unwrap();
    assert!(build(&path, &source).unwrap_err().contains("lower than"));
    assert!(source
        .tag_descriptor("private/machines/ibm-pc/lowered:1.3.0")
        .is_err());
}

#[test]
fn unsupported_machine_preparation_requirements_stop_before_package_acquisition() {
    use image_store::compat::{AppVersion as V, Compatibility};
    struct NoFetch;
    impl recipe::build::Fetcher for NoFetch {
        fn fetch(&self, _: &str, _: u64, _: &mut dyn std::io::Write) -> Result<(), String> {
            panic!("unexpected input request")
        }
    }
    impl recipe::template::Acquire for NoFetch {
        fn acquire(&self, _: &Store, _: &recipe::template::UsePlan, _: &str) -> Result<(), String> {
            panic!("unexpected package request")
        }
    }
    let temp = image_store::test_support::TempDir::new("preparation-requirements");
    fs::write(temp.path().join("config.toml"), b"opaque config").unwrap();
    for (name, runtime, stated, reason) in [
        (
            "core",
            pc_image::compat::compatibility(pc_image::compat::NEXT_RELEASE),
            "min_app_version='99.0.0'",
            "unemupc",
        ),
        (
            "frontend",
            Compatibility::new(V::new(0, 0, 3), V::new(0, 0, 1)).with_core(
                "ibm-pc",
                V::new(0, 0, 9),
                V::new(0, 0, 1),
            ),
            "",
            "front end",
        ),
    ] {
        let store = Store::open(temp.path().join(name))
            .unwrap()
            .with_compatibility(runtime);
        let path = temp.path().join(format!("{name}.machine.toml"));
        fs::write(&path, format!("recipe='1.1'\n[machine]\nname='private/machines/ibm-pc/test'\nplatform='ibm-pc'\nversion='1.0.0'\n{stated}\n[template]\nconfig='config'\n[input.config]\nname='config.toml'\nsize=13\nsha256='{}'\nbundle=true\n[use.bios]\nimage='private/firmware/ibm-pc/bios:r1'\ndigest='sha256:{}'\n", Digest::of(b"opaque config").hex(), "0".repeat(64))).unwrap();
        let error = recipe::template::prepare(
            Request {
                path: &path,
                providers: recipe::Providers(&[&pc_image::recipe::PcRecipes]),
                store: &store,
                index: None,
                store_read: None,
                libraries: &mut [],
                fetcher: Some(&NoFetch),
                keep_as: None,
                find_outputs: false,
            },
            &|_| Some(&unemu_pc_core::PcCore),
            Some(&NoFetch),
            &mut |_| {},
        )
        .err()
        .expect("unsupported requirement");
        assert!(error.contains(reason), "{error}");
        assert!(store.list_tags(None, 100).unwrap().tags.is_empty());
    }
}
