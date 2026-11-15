// SPDX-License-Identifier: AGPL-3.0-or-later
//! Official template builds from the open fixtures: determinism, the guard
//! seam, and machines that validate and lower; machine-images §5.5.
use image_store::{media_type as mt, BlobSource, ConfigValidator, Manifest, Store, MAX_SMALL_BLOB};
use pc_image::create::{NewImage, PackageRom};
use pc_image::template::{self, Built, Kind, Outcome, VERSION};
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "../../cli/tests/support/fixtures.rs"]
mod fixtures;

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn vendor() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor")
}

/// Whether the fetched ROMs and source archives of `sources` are present; the
/// first missing one skips the test unless the fixture gate requires it.
fn present(sources: &[template::FirmwareSource]) -> bool {
    sources.iter().all(|source| {
        [source.rom, source.source_archive]
            .into_iter()
            .all(|relative| fixtures::fixture(relative).is_some())
    })
}

fn allow(_: &str, _: &Manifest) -> Result<(), String> {
    Ok(())
}

/// Every image's repository and digest with its local tags.
fn identities(built: &[Built]) -> Vec<(String, String, Vec<String>)> {
    built
        .iter()
        .map(|b| {
            (
                b.repository.clone(),
                b.descriptor.digest.to_string(),
                b.tags.iter().map(|(tag, _)| tag.clone()).collect(),
            )
        })
        .collect()
}

#[test]
fn two_builds_from_different_fixture_copies_give_identical_images() {
    if !present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-determinism");
    // Only the files the build reads, at another path.
    let copy = t.0.join("fixtures");
    for source in &template::FIRMWARE {
        for relative in [source.rom, source.source_archive] {
            let to = copy.join(relative);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(vendor().join(relative), to).unwrap();
        }
    }
    let first = Store::open(t.0.join("first")).unwrap();
    let second = Store::open(t.0.join("second")).unwrap();
    let a = template::build(&first, &vendor(), &allow, &mut |_| {}).unwrap();
    let b = template::build(&second, &copy, &allow, &mut |_| {}).unwrap();
    assert_eq!(identities(&a), identities(&b));
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(
            first.read_verified(&x.descriptor, MAX_SMALL_BLOB).unwrap(),
            second.read_verified(&y.descriptor, MAX_SMALL_BLOB).unwrap()
        );
    }
    let names: Vec<_> = a.iter().map(|b| (b.kind, b.repository.as_str())).collect();
    assert_eq!(
        names,
        [
            (
                Kind::Firmware,
                "unemu/firmware/kiselev-xi8088-1.0.2-atreset"
            ),
            (Kind::Firmware, "unemu/firmware/vgabios-0.8a-unemu.1-std"),
            (Kind::Firmware, "unemu/firmware/xub-r638-xtp-ata1f0"),
            (Kind::Machine, "unemu/open-at"),
            (Kind::Machine, "unemu/open-at-games"),
        ]
    );
    for built in &a {
        let tags: Vec<_> = built.tags.iter().map(|(tag, _)| tag.as_str()).collect();
        assert_eq!(
            tags,
            [
                template::local_tag(&built.repository, VERSION),
                template::local_tag(&built.repository, "latest")
            ]
        );
        assert!(built.tags.iter().all(|(_, o)| *o == Outcome::Created));
        for (tag, _) in &built.tags {
            assert_eq!(
                first.tag_descriptor(tag).unwrap().digest,
                built.descriptor.digest
            );
        }
    }
    assert_eq!(a[3].tags[0].0, "open-at:1.0.1");
    // A rebuild into the same store finds every tag in place.
    let again = template::build(&first, &vendor(), &allow, &mut |_| {}).unwrap();
    assert_eq!(identities(&again), identities(&a));
    assert!(again
        .iter()
        .flat_map(|b| &b.tags)
        .all(|(_, o)| *o == Outcome::Present));
}

#[test]
fn the_guard_sees_each_image_before_its_tags_and_a_refusal_tags_nothing_of_it() {
    if !present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-guard");
    let store = Store::open(t.0.join("store")).unwrap();
    let seen = RefCell::new(Vec::new());
    let guard = |reference: &str, manifest: &Manifest| {
        let (repository, tag) = reference.rsplit_once(':').unwrap();
        assert_eq!(tag, VERSION);
        assert!(store
            .tag_descriptor(&template::local_tag(repository, tag))
            .is_err());
        assert!(!manifest.layers.is_empty());
        seen.borrow_mut().push(reference.to_owned());
        if repository == "unemu/open-at-games" {
            Err("synthetic refusal".to_owned())
        } else {
            Ok(())
        }
    };
    let error = template::build(&store, &vendor(), &guard, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("unemu/open-at-games:1.0.1 refused: synthetic refusal"),
        "{error}"
    );
    assert_eq!(
        *seen.borrow(),
        [
            "unemu/firmware/kiselev-xi8088-1.0.2-atreset:1.0.1",
            "unemu/firmware/vgabios-0.8a-unemu.1-std:1.0.1",
            "unemu/firmware/xub-r638-xtp-ata1f0:1.0.1",
            "unemu/open-at:1.0.1",
            "unemu/open-at-games:1.0.1",
        ]
    );
    assert!(store.tag_descriptor("open-at:1.0.1").is_ok());
    for tag in ["open-at-games:1.0.1", "open-at-games:latest"] {
        assert!(store.tag_descriptor(tag).is_err(), "{tag}");
    }
}

#[test]
fn a_missing_fixture_or_another_source_archive_names_the_fetch_script() {
    let t = Temp::new("template-fixtures");
    let store = Store::open(t.0.join("store")).unwrap();
    let error = template::build(&store, &t.0.join("empty"), &allow, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("missing fixture") && error.contains("vendor/kiselev-bios/fetch.sh"),
        "{error}"
    );
    let kiselev = &template::FIRMWARE[0];
    if !present(std::slice::from_ref(kiselev)) {
        return;
    }
    let copy = t.0.join("fixtures");
    for relative in [kiselev.rom, kiselev.source_archive] {
        let to = copy.join(relative);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(vendor().join(relative), to).unwrap();
    }
    let mut archive = vec![0_u8; 8192];
    archive[257..263].copy_from_slice(b"ustar\0");
    fs::write(copy.join(kiselev.source_archive), archive).unwrap();
    let error = template::build(&store, &copy, &allow, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("not the source archive the catalogue pins"),
        "{error}"
    );
    assert!(store.list_tags(None, 10).unwrap().tags.is_empty());
}

#[test]
fn template_machines_validate_lower_and_describe_themselves() {
    if !present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-lowering");
    let store = Store::open(t.0.join("store")).unwrap();
    template::build(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    for (tag, games) in [("open-at:1.0.1", false), ("open-at-games:latest", true)] {
        let resolved = store.resolve(tag, &pc_image::PcProvider).unwrap();
        let config: toml::Table = std::str::from_utf8(&resolved.config)
            .unwrap()
            .parse()
            .unwrap();
        pc_image::PcProvider
            .validate(&config, &resolved.manifest.layers, &store)
            .unwrap();
        let annotations = &resolved.manifest.annotations;
        let template = &template::TEMPLATES[usize::from(games)];
        assert_eq!(
            annotations["org.opencontainers.image.title"],
            template.title
        );
        assert_eq!(
            annotations["org.opencontainers.image.description"],
            template.description
        );
        // Every ROM travels with its source archive; no medium is bound.
        let count = |kind: &str| {
            resolved
                .manifest
                .layers
                .iter()
                .filter(|d| d.decoded_media_type() == kind)
                .count()
        };
        assert_eq!(count(mt::FIRMWARE), 3);
        assert_eq!(count(mt::TAR), 3);
        assert_eq!(count(mt::FLOPPY), 0);

        let machine =
            pc_image::resolve(&store, tag, &t.0.join(format!("session-{games}"))).unwrap();
        let has = |wanted: fn(&assembly::DeviceKind) -> bool| {
            machine.devices.iter().any(|d| wanted(&d.kind))
        };
        assert_eq!(has(|k| matches!(k, assembly::DeviceKind::Sb16(_))), games);
        assert_eq!(
            has(|k| matches!(k, assembly::DeviceKind::LotechEms(_))),
            games
        );
        assert_eq!(
            has(|k| matches!(k, assembly::DeviceKind::Uart(u) if u.mouse)),
            games
        );
        assert!(has(|k| matches!(k, assembly::DeviceKind::VgaRegisters(_))));
        let disks: Vec<_> = machine
            .drives
            .iter()
            .filter_map(|d| match d.kind {
                assembly::DriveKind::HardDisk { geometry } => Some((
                    geometry.cylinders,
                    geometry.heads,
                    geometry.sectors_per_track,
                )),
                _ => None,
            })
            .collect();
        assert_eq!(disks, [(1024, 16, 63)]);
        assert_eq!(
            machine
                .drives
                .iter()
                .filter(|d| matches!(d.kind, assembly::DriveKind::Floppy { .. }))
                .count(),
            1
        );
    }
}

/// The digests of this template generation. Any change to the templates, or
/// to what they are built from, changes them; version tags never move, so a
/// changed template needs a new `pc_image::template::VERSION` and new pins.
const PINNED: [(&str, &str); 5] = [
    (
        "unemu/firmware/kiselev-xi8088-1.0.2-atreset",
        "sha256:202af98f8fff46f5a392d2103ea56bdbe6c43d88f428951f18b72c2fa95df7da",
    ),
    (
        "unemu/firmware/vgabios-0.8a-unemu.1-std",
        "sha256:78d390d182157865e57c576671753c3e81ee4d04bce521ee29e6508ae7ae49c4",
    ),
    (
        "unemu/firmware/xub-r638-xtp-ata1f0",
        "sha256:e4dfecc8f9b7197a69c3b6064667a795d7b0ac16c56979ddb3eac778d5b9b332",
    ),
    (
        "unemu/open-at",
        "sha256:85e095b004ca09369d95becc047e9ad53287ffdd79b5e10e576f25dbb0646592",
    ),
    (
        "unemu/open-at-games",
        "sha256:1d092620ee9207776067aa2fcee8560df3d281979aa0c566016f882afb25576f",
    ),
];

#[test]
fn the_published_generation_keeps_its_pinned_digests() {
    if !present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-pins");
    let store = Store::open(t.0.join("store")).unwrap();
    let built = template::build(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    let digests: Vec<_> = built
        .iter()
        .map(|b| (b.repository.clone(), b.descriptor.digest.to_string()))
        .collect();
    let pinned: Vec<_> = PINNED
        .iter()
        .map(|(r, d)| (r.to_string(), d.to_string()))
        .collect();
    assert_eq!(
        digests, pinned,
        "the templates changed: version {VERSION} is published with the pinned digests and its tags never move; bump pc_image::template::VERSION, then update these pins"
    );
}

/// The open-at template with another title, from the store's packages.
fn variant(store: &Store, title: &str) -> NewImage {
    let packages = pc_image::create::packages(store).unwrap();
    let firmware = template::FIRMWARE
        .iter()
        .map(|source| {
            packages
                .iter()
                .find_map(|p| {
                    p.config
                        .roms
                        .iter()
                        .find(|r| r.catalogue_id.as_deref() == Some(source.catalogue_id))
                        .map(|r| PackageRom::new(p, r))
                })
                .unwrap()
        })
        .collect();
    NewImage {
        hardware: template::TEMPLATES[0].hardware().unwrap(),
        firmware,
        title: Some(title.into()),
        description: None,
    }
}

fn alias_outcome(built: &[Built], repository: &str) -> Outcome {
    built
        .iter()
        .find(|b| b.repository == repository)
        .unwrap()
        .tags[1]
        .1
        .clone()
}

#[test]
fn a_version_tag_that_names_another_image_refuses_the_build() {
    if !present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-version-conflict");
    let store = Store::open(t.0.join("store")).unwrap();
    let firmware_only = |reference: &str, _: &Manifest| {
        if reference.starts_with("unemu/firmware/") {
            Ok(())
        } else {
            Err("machines later".to_owned())
        }
    };
    assert!(template::build(&store, &vendor(), &firmware_only, &mut |_| {}).is_err());
    let other = pc_image::create::create(
        &store,
        &variant(&store, "Another AT"),
        "open-at:1.0.1",
        None,
    )
    .unwrap();
    let error = template::build(&store, &vendor(), &allow, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("open-at:1.0.1 already names {}", other.digest))
            && error.contains("a version tag never moves"),
        "{error}"
    );
    assert_eq!(
        store.tag_descriptor("open-at:1.0.1").unwrap().digest,
        other.digest
    );
    assert!(store.tag_descriptor("open-at:latest").is_err());
}

#[test]
fn latest_moves_only_away_from_lower_versions_of_its_repository() {
    if !present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-alias");
    let store = Store::open(t.0.join("store")).unwrap();
    let built = template::build(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    let ours = built[3].descriptor.clone();
    let games = built[4].descriptor.clone();
    let point = |to: &image_store::Descriptor| {
        let current = store.tag_descriptor("open-at:latest").unwrap().digest;
        store
            .update_tag("open-at:latest", to, Some(&current), &pc_image::PcProvider)
            .unwrap();
    };
    let rebuild = || template::build(&store, &vendor(), &allow, &mut |_| {}).unwrap();

    // A higher version keeps latest.
    let higher =
        pc_image::create::create(&store, &variant(&store, "Newer AT"), "open-at:2.0.0", None)
            .unwrap();
    point(&higher);
    assert_eq!(
        alias_outcome(&rebuild(), "unemu/open-at"),
        Outcome::Kept(higher.digest.clone())
    );
    assert_eq!(
        store.tag_descriptor("open-at:latest").unwrap().digest,
        higher.digest
    );

    // So does an image no version of the repository names.
    point(&games);
    assert_eq!(
        alias_outcome(&rebuild(), "unemu/open-at"),
        Outcome::Kept(games.digest.clone())
    );

    // A lower version gives way to this one.
    let lower =
        pc_image::create::create(&store, &variant(&store, "Older AT"), "open-at:0.9.0", None)
            .unwrap();
    point(&lower);
    assert_eq!(
        alias_outcome(&rebuild(), "unemu/open-at"),
        Outcome::Moved(lower.digest.clone())
    );
    assert_eq!(
        store.tag_descriptor("open-at:latest").unwrap().digest,
        ours.digest
    );
    assert_eq!(alias_outcome(&rebuild(), "unemu/open-at"), Outcome::Present);
}

#[test]
fn only_the_judged_manifest_is_tagged() {
    if !present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-judged");
    let store = Store::open(t.0.join("store")).unwrap();
    template::build(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    let wrong = image_store::Digest::of(b"another manifest");
    let image = variant(&store, "Judged AT");
    let error = pc_image::create::create_judged(&store, &image, "judged:1.0.0", None, &wrong)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("not the checked {wrong}")),
        "{error}"
    );
    assert!(store.tag_descriptor("judged:1.0.0").is_err());
    let previewed = pc_image::create::preview(&image, &store).unwrap();
    let right = image_store::Digest::of(
        &previewed
            .encode_with_types(pc_image::PAYLOAD_TYPES)
            .unwrap(),
    );
    let created =
        pc_image::create::create_judged(&store, &image, "judged:1.0.0", None, &right).unwrap();
    assert_eq!(created.digest, right);

    let kiselev = &template::FIRMWARE[0];
    let catalogue = pc_image::firmware::catalogue();
    let scan = pc_image::firmware_import::scan(&[vendor().join(kiselev.rom)], catalogue).unwrap();
    let statement = pc_image::firmware_import::Statement {
        source: Some(vendor().join(kiselev.source_archive)),
        ..Default::default()
    };
    let publish = |tag: &str, judged: &image_store::Digest| {
        pc_image::firmware_import::publish_as(
            &store,
            &scan.candidates[0],
            &[],
            &statement,
            catalogue,
            tag,
            Some(judged),
        )
    };
    let error = publish("judged/bios:1", &wrong).unwrap_err().to_string();
    assert!(
        error.contains(&format!("not the checked {wrong}")),
        "{error}"
    );
    assert!(store.tag_descriptor("judged/bios:1").is_err());
    // The template's own package, under its version tag, is the judged one.
    let package = store
        .tag_descriptor("unemu/firmware/kiselev-xi8088-1.0.2-atreset:1.0.1")
        .unwrap();
    assert_eq!(
        publish("judged/bios:1", &package.digest).unwrap().digest,
        package.digest
    );
}
