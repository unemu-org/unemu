// SPDX-License-Identifier: AGPL-3.0-or-later
//! Official template builds from the open fixtures: determinism, the guard
//! seam, machines that validate and lower, and the local builds of the
//! firmware packages and installation sets, pinned to the digests the
//! unemu-media repository publishes; machine-images §5.5.
use image_store::{media_type as mt, BlobSource, ConfigValidator, Manifest, Store, MAX_SMALL_BLOB};
use pc_image::create::{NewImage, PackageRom};
use pc_image::template::{self, Built, Kind, Outcome, FIRMWARE_REVISION, VERSION};
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "../../pc-harness/tests/support/fixtures.rs"]
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
fn firmware_present(sources: &[template::FirmwareSource]) -> bool {
    sources.iter().all(|source| {
        [source.rom, source.source_archive]
            .into_iter()
            .all(|relative| fixtures::fixture(relative).is_some())
    })
}

/// Every fixture file the installation sets read: their disks, then the
/// extras they share.
fn media_files() -> Vec<&'static str> {
    template::media_files()
        .into_iter()
        .map(|file| file.path)
        .collect()
}

/// Whether every fixture of the installation sets is present, as
/// [`firmware_present`].
fn media_present() -> bool {
    media_files()
        .into_iter()
        .all(|relative| fixtures::fixture(relative).is_some())
}

fn allow(_: &str, _: &Manifest, _: &[u8]) -> Result<(), String> {
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
    if !firmware_present(&template::FIRMWARE) {
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
            (Kind::Machine, "unemu/open-at"),
            (Kind::Machine, "unemu/open-at-games"),
        ]
    );
    // The packages the templates bind are local copies of the published
    // ones, under their published tags.
    for (source, (_, digest)) in template::FIRMWARE.iter().zip(&FIRMWARE_PINNED) {
        let tag = template::local_tag(&source.repository(), FIRMWARE_REVISION);
        assert_eq!(
            first.tag_descriptor(&tag).unwrap().digest.to_string(),
            *digest
        );
    }
    for built in &a {
        let tags: Vec<_> = built.tags.iter().map(|(tag, _)| tag.as_str()).collect();
        assert_eq!(built.version, VERSION);
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
    assert_eq!(a[0].tags[0].0, "open-at:1.2.0");
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
    if !firmware_present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-guard");
    let store = Store::open(t.0.join("store")).unwrap();
    let seen = RefCell::new(Vec::new());
    let guard = |reference: &str, manifest: &Manifest, _: &[u8]| {
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
        error.contains("unemu/open-at-games:1.2.0 refused: synthetic refusal"),
        "{error}"
    );
    // Only the templates: the packages they bind are not images of the build.
    assert_eq!(
        *seen.borrow(),
        ["unemu/open-at:1.2.0", "unemu/open-at-games:1.2.0"]
    );
    assert!(store.tag_descriptor("open-at:1.2.0").is_ok());
    for tag in ["open-at-games:1.2.0", "open-at-games:latest"] {
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
        error.contains("missing fixture") && error.contains("vendor/unemu-at-bios/fetch.sh"),
        "{error}"
    );
    let bios = &template::FIRMWARE[0];
    if !firmware_present(std::slice::from_ref(bios)) {
        return;
    }
    let copy = t.0.join("fixtures");
    for relative in [bios.rom, bios.source_archive] {
        let to = copy.join(relative);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(vendor().join(relative), to).unwrap();
    }
    let mut archive = vec![0_u8; 8192];
    archive[257..263].copy_from_slice(b"ustar\0");
    fs::write(copy.join(bios.source_archive), archive).unwrap();
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
    if !firmware_present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-lowering");
    let store = Store::open(t.0.join("store")).unwrap();
    template::build(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    for (tag, games) in [("open-at:1.2.0", false), ("open-at-games:latest", true)] {
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
            2
        );
        // B: is an empty 3.5-inch 1.44M drive; A: stays the 1.2M drive.
        let floppies: Vec<_> = machine
            .drives
            .iter()
            .filter_map(|d| match d.kind {
                assembly::DriveKind::Floppy { geometry } => Some((
                    d.unit,
                    geometry,
                    d.media
                        .as_ref()
                        .map(|list| (list.entries.len(), list.empty)),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            floppies,
            [
                (0, assembly::FloppyGeometry::HD_1_2, Some((0, true))),
                (1, assembly::FloppyGeometry::HD_1_44, Some((0, true))),
            ]
        );
    }
}

/// The digests of this template generation. Any change to the templates, or
/// to what they are built from, changes them; version tags never move, so a
/// changed template needs a new `pc_image::template::VERSION` and new pins.
/// Two exceptions moved every pin of this file under the same tags, because
/// the artifacts published before the first public release are replaced, not
/// revised: the rename of the platform id to `ibm-pc`, and the export file
/// names on every layer of the firmware packages. A template copies the
/// packages' ROM and source layers with their names, and its manifest names
/// the package digests in `unemu.firmware.sources`, so any change to a
/// package moves these pins too.
const PINNED: [(&str, &str); 2] = [
    (
        "unemu/open-at",
        "sha256:750981c254e3107c46808f43c109ed2ca155f00204f3ccc0635d57c2ad3fd363",
    ),
    (
        "unemu/open-at-games",
        "sha256:01028f9cd51b45053ee70b38af5be29f591537fc719e48f7c8f5572c7ce23a1f",
    ),
];

/// The firmware packages as the unemu-media repository builds them
/// (`recipes/*/firmware.toml`) for the push that replaces the published ones
/// under the names with the platform in the path and the revision tag: the
/// names change no manifest byte, but the export file name now on every
/// layer does, so the packages read back from the registry before that push
/// have other digests. The local build must give these digests: a pin that
/// fails means the two repositories no longer build the same package, and a
/// package's revision tag never moves.
const FIRMWARE_PINNED: [(&str, &str); 3] = [
    (
        "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1",
        "sha256:e3c226ae2cb8935ffacf9fb806124d1c1219388c8af4f0fb60ef5a96fbd86c58",
    ),
    (
        "unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1",
        "sha256:179a11dbd20741e1e3775388ed42bdf9de2b62e9136c0467271d34271b769d57",
    ),
    (
        "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1",
        "sha256:7d42e80e6297cadbd2c08c1c26a19a4a68c33c9651aceec2c98053bf81f90547",
    ),
];

#[test]
fn the_published_generation_keeps_its_pinned_digests() {
    if !firmware_present(&template::FIRMWARE) {
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

/// Each local package build gives the published package under its published
/// tag, and from another copy of the fixtures the same package.
#[test]
fn the_firmware_packages_are_the_published_ones() {
    if !firmware_present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-firmware-pins");
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
    let seen = RefCell::new(Vec::new());
    let record = |reference: &str, _: &Manifest, _: &[u8]| {
        seen.borrow_mut().push(reference.to_owned());
        Ok(())
    };
    let a = template::build_firmware(&first, &vendor(), &record, &mut |_| {}).unwrap();
    let b = template::build_firmware(&second, &copy, &allow, &mut |_| {}).unwrap();
    assert_eq!(identities(&a), identities(&b));
    let judged: Vec<_> = FIRMWARE_PINNED.iter().map(|(tag, _)| *tag).collect();
    assert_eq!(*seen.borrow(), judged, "the guard judges each package");
    let tagged: Vec<_> = a
        .iter()
        .map(|built| {
            assert_eq!(built.kind, Kind::Firmware);
            assert_eq!(built.version, FIRMWARE_REVISION);
            assert_eq!(built.tags.len(), 1, "a package has no alias");
            assert_eq!(built.tags[0].1, Outcome::Created);
            (built.tags[0].0.clone(), built.descriptor.digest.to_string())
        })
        .collect();
    let pinned: Vec<_> = FIRMWARE_PINNED
        .iter()
        .map(|(tag, digest)| (tag.to_string(), digest.to_string()))
        .collect();
    assert_eq!(
        tagged, pinned,
        "the local firmware packages are no longer the published ones"
    );
    for (tag, digest) in &pinned {
        assert_eq!(
            first.tag_descriptor(tag).unwrap().digest.to_string(),
            *digest
        );
    }
    let again = template::build_firmware(&first, &vendor(), &allow, &mut |_| {}).unwrap();
    assert_eq!(identities(&again), identities(&a));
    assert!(again
        .iter()
        .flat_map(|b| &b.tags)
        .all(|(_, o)| *o == Outcome::Present));
}

/// The system BIOS package names the fork and its original author, so the
/// generated README credits Sergey Kiselev outside the source layer too.
#[test]
fn the_bios_package_readme_names_the_fork_and_its_original_author() {
    if !firmware_present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-readme");
    let store = Store::open(t.0.join("store")).unwrap();
    let built = template::build_firmware(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    let bios = &built[0];
    assert_eq!(
        bios.repository,
        template::FIRMWARE[0].repository(),
        "the system BIOS package comes first"
    );
    let manifest = Manifest::decode(
        &store
            .read_verified(&bios.descriptor, MAX_SMALL_BLOB)
            .unwrap(),
    )
    .unwrap();
    let readme = manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == mt::MARKDOWN)
        .expect("generated README layer");
    let text = String::from_utf8(store.read_verified(readme, MAX_SMALL_BLOB).unwrap()).unwrap();
    let entry = pc_image::firmware::catalogue()
        .iter()
        .find(|entry| entry.id == template::FIRMWARE[0].catalogue_id)
        .unwrap();
    let assembly::FirmwareDistribution::Open { provenance, .. } = entry.distribution else {
        panic!("the template BIOS is open firmware");
    };
    assert!(text.contains(&format!("Origin: {provenance}\n")), "{text}");
    assert!(
        text.contains("fork of skiselev/8088_bios 1.0.2 by Sergey Kiselev"),
        "{text}"
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
    if !firmware_present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-version-conflict");
    let store = Store::open(t.0.join("store")).unwrap();
    // Only the packages the templates bind: every template is refused.
    let firmware_only = |_: &str, _: &Manifest, _: &[u8]| Err("machines later".to_owned());
    assert!(template::build(&store, &vendor(), &firmware_only, &mut |_| {}).is_err());
    let other = pc_image::create::create(
        &store,
        &variant(&store, "Another AT"),
        "open-at:1.2.0",
        None,
    )
    .unwrap();
    let error = template::build(&store, &vendor(), &allow, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("open-at:1.2.0 already names {}", other.digest))
            && error.contains("a version tag never moves"),
        "{error}"
    );
    assert_eq!(
        store.tag_descriptor("open-at:1.2.0").unwrap().digest,
        other.digest
    );
    assert!(store.tag_descriptor("open-at:latest").is_err());
}

#[test]
fn latest_moves_only_away_from_lower_versions_of_its_repository() {
    if !firmware_present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-alias");
    let store = Store::open(t.0.join("store")).unwrap();
    let built = template::build(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    let ours = built[0].descriptor.clone();
    let games = built[1].descriptor.clone();
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
    if !firmware_present(&template::FIRMWARE) {
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

    let bios = &template::FIRMWARE[0];
    let catalogue = pc_image::firmware::catalogue();
    let scan = pc_image::firmware_import::scan(&[vendor().join(bios.rom)], catalogue).unwrap();
    let statement = bios.statement(vendor().join(bios.source_archive));
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
    let error = publish("judged/bios:r1", &wrong).unwrap_err().to_string();
    assert!(
        error.contains(&format!("not the checked {wrong}")),
        "{error}"
    );
    assert!(store.tag_descriptor("judged/bios:r1").is_err());
    // The template's own package, under its published tag, is the judged one.
    let package = store
        .tag_descriptor("unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1")
        .unwrap();
    assert_eq!(
        publish("judged/bios:r1", &package.digest).unwrap().digest,
        package.digest
    );
}

/// Each firmware package carries its title, description, vendor, version and
/// labels in its config, and the standard OCI annotations repeat the first
/// four, so library and registry views need not fall back to the catalogue id.
#[test]
fn each_firmware_package_states_its_title_description_vendor_and_version() {
    if !firmware_present(&template::FIRMWARE) {
        return;
    }
    let t = Temp::new("template-metadata");
    let store = Store::open(t.0.join("store")).unwrap();
    let built = template::build_firmware(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    assert!(template::FIRMWARE[0].description.contains("Sergey Kiselev"));
    for (source, package) in template::FIRMWARE.iter().zip(&built) {
        assert_eq!(package.repository, source.repository());
        let manifest = Manifest::decode(
            &store
                .read_verified(&package.descriptor, MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        let config = image_store::firmware::Config::decode(
            &store
                .read_verified(&manifest.config, MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        assert_ne!(config.name, source.catalogue_id);
        assert_eq!(config.name, source.title);
        assert_eq!(config.description.as_deref(), Some(source.description));
        assert_eq!(config.vendor.as_deref(), Some(source.vendor));
        assert_eq!(config.version.as_deref(), Some(source.version));
        let mut labels = source.labels.to_vec();
        labels.sort_unstable();
        assert_eq!(config.labels, labels, "{}", source.catalogue_id);
        for (key, value) in [
            ("title", source.title),
            ("description", source.description),
            ("vendor", source.vendor),
            ("version", source.version),
        ] {
            assert_eq!(
                manifest
                    .annotations
                    .get(&format!("org.opencontainers.image.{key}"))
                    .map(String::as_str),
                Some(value),
                "{key} of {}",
                source.catalogue_id
            );
        }
    }
}

/// Copy every fixture of the installation sets to `to`.
fn copy_media_fixtures(to: &std::path::Path) {
    for relative in media_files() {
        let target = to.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(vendor().join(relative), target).unwrap();
    }
}

/// Each installation set holds the edition's disks unmodified and in
/// installer order, the licence index and the source archive, with the
/// catalogue record and the registry browser's
/// annotations. Every medium and extra is open under the release's licence
/// and links the licence index and the source archive, so the official
/// namespace's rules 4 and 5 pass.
#[test]
fn the_installation_sets_carry_the_edition_its_licence_index_and_its_source() {
    use image_store::{
        license,
        media_set::{Config, ExtraKind, Validator},
        metadata::Metadata,
    };
    use pc_image::media_set::{format_name, PcMediaFormats};
    if !media_present() {
        return;
    }
    let t = Temp::new("template-media");
    let store = Store::open(t.0.join("store")).unwrap();
    let release = &template::FREEDOS_1_4;
    let built = template::build_media(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    for (source, set) in template::MEDIA.iter().zip(&built) {
        assert_eq!(
            (set.kind, set.repository.as_str()),
            (Kind::Media, source.repository().as_str())
        );
        let resolved = store
            .resolve_descriptor(&set.descriptor, &Validator(&PcMediaFormats))
            .unwrap();
        let config = Config::decode(&resolved.config).unwrap();
        // The disks, byte for byte the edition's, in installer order.
        assert_eq!(config.media.len(), source.disks.len());
        for (medium, disk) in config.media.iter().zip(source.disks) {
            assert_eq!(medium.label, disk.name);
            assert_eq!(format_name(&medium.format).unwrap(), source.format);
            assert_eq!(medium.blob.hex(), disk.sha256);
        }
        let extras: Vec<_> = config
            .extras
            .iter()
            .map(|e| {
                (
                    e.filename.as_str(),
                    e.dos_name.as_deref(),
                    e.kind,
                    e.blob.hex(),
                )
            })
            .collect();
        let expected: Vec<_> = release
            .extras
            .iter()
            .map(|(file, kind)| {
                (
                    file.path.rsplit('/').next().unwrap(),
                    Some(file.name),
                    *kind,
                    file.sha256,
                )
            })
            .collect();
        assert_eq!(extras, expected);
        assert_eq!(extras.last().unwrap().2, ExtraKind::Archive);
        // Open under the release's licence, linked to its text and source.
        let terms = config
            .extras
            .iter()
            .find(|e| e.filename == release.terms)
            .unwrap();
        let archive = config
            .extras
            .iter()
            .find(|e| e.filename == release.source)
            .unwrap();
        for d in &resolved.manifest.layers {
            if d.decoded_media_type() == mt::METADATA {
                continue;
            }
            assert_eq!(license::open_license(d).unwrap().as_str(), release.license);
            assert_eq!(d.annotations[license::TERMS], terms.blob.to_string());
            assert_eq!(d.annotations[license::SOURCE], archive.blob.to_string());
        }
        let closure: Vec<_> = std::iter::once(resolved.manifest.config.clone())
            .chain(resolved.manifest.layers.clone())
            .collect();
        assert!(closure.iter().all(|d| license::open_license(d).is_some()));
        assert!(closure
            .iter()
            .all(|d| !license::source_missing(d, &closure)));
        assert!(license::terms_unlinked(&closure).is_empty());
        // The catalogue record and the registry browser's annotations: the
        // work's title, with the release and edition in fields of their own.
        let metadata = Metadata::decode(&resolved.metadata, &resolved.manifest.layers).unwrap();
        let title = &metadata.titles[0];
        assert_eq!(title.title, "FreeDOS");
        assert_eq!(title.title, release.title);
        assert_eq!(title.publishers, [release.publisher]);
        assert_eq!(title.release_year, Some(2025));
        assert_eq!(title.platform.as_deref(), Some("DOS"));
        assert_eq!(title.genres, ["operating system"]);
        assert_eq!(title.version.as_deref(), Some("1.4"));
        assert_eq!(title.edition.as_deref(), Some("Floppy Edition"));
        assert_eq!(metadata.labels, release.labels);
        let annotations = &resolved.manifest.annotations;
        for (key, value) in [
            ("title", release.title),
            ("description", source.description),
            ("vendor", release.publisher),
            ("authors", release.publisher),
        ] {
            assert_eq!(
                annotations[&format!("org.opencontainers.image.{key}")],
                value,
                "{key}"
            );
        }
        assert_eq!(
            annotations["org.opencontainers.image.licenses"],
            format!("{} AND CC0-1.0", release.license)
        );
    }
}

/// The guard judges every set before it is tagged, and a refused set leaves
/// no tag of it behind.
#[test]
fn the_guard_judges_each_installation_set_before_its_tags() {
    if !media_present() {
        return;
    }
    let t = Temp::new("template-media-guard");
    let store = Store::open(t.0.join("store")).unwrap();
    let seen = RefCell::new(Vec::new());
    let guard = |reference: &str, manifest: &Manifest, _: &[u8]| {
        let (repository, tag) = reference.rsplit_once(':').unwrap();
        assert_eq!(repository, "unemu/media/ibm-pc/freedos");
        assert_eq!(manifest.artifact_type(), mt::MEDIA_SET);
        assert!(store
            .tag_descriptor(&template::local_tag(repository, tag))
            .is_err());
        seen.borrow_mut().push(reference.to_owned());
        if tag == "1.4-1.2m-r1" {
            Err("synthetic refusal".to_owned())
        } else {
            Ok(())
        }
    };
    let error = template::build_media(&store, &vendor(), &guard, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("unemu/media/ibm-pc/freedos:1.4-1.2m-r1 refused: synthetic refusal"),
        "{error}"
    );
    assert_eq!(
        *seen.borrow(),
        [
            "unemu/media/ibm-pc/freedos:1.4-r1",
            "unemu/media/ibm-pc/freedos:1.4-1.2m-r1"
        ]
    );
    for tag in [
        "unemu/media/ibm-pc/freedos:1.4-r1",
        "unemu/media/ibm-pc/freedos:1.4",
    ] {
        assert!(store.tag_descriptor(tag).is_ok(), "{tag}");
    }
    for tag in [
        "unemu/media/ibm-pc/freedos:1.4-1.2m-r1",
        "unemu/media/ibm-pc/freedos:1.4-1.2m",
        "unemu/media/ibm-pc/freedos:latest",
    ] {
        assert!(store.tag_descriptor(tag).is_err(), "{tag}");
    }
}

#[test]
fn another_file_at_a_media_fixture_path_is_refused_before_any_set_is_tagged() {
    if !media_present() {
        return;
    }
    let t = Temp::new("template-media-fixture");
    let copy = t.0.join("fixtures");
    copy_media_fixtures(&copy);
    // A boot disk with one changed byte is not the edition's.
    let boot = copy.join(template::MEDIA[0].disks[0].path);
    let mut bytes = fs::read(&boot).unwrap();
    bytes[0x200] ^= 1;
    fs::write(&boot, bytes).unwrap();
    let store = Store::open(t.0.join("store")).unwrap();
    let error = template::build_media(&store, &copy, &allow, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("x86BOOT.img is not the file vendor/freedos/fetch-floppy-1.4.sh pins"),
        "{error}"
    );
    let media_tags = |store: &Store| {
        store
            .list_tags(None, 100)
            .unwrap()
            .tags
            .into_iter()
            .filter(|t| t.name.starts_with("unemu/media/"))
            .count()
    };
    assert_eq!(media_tags(&store), 0);
    fs::remove_file(&boot).unwrap();
    let error = template::build_media(&store, &copy, &allow, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("missing fixture") && error.contains("fetch-floppy-1.4.sh"),
        "{error}"
    );
    assert_eq!(media_tags(&store), 0);
}

/// The installation sets as the unemu-media repository builds them
/// (`recipes/freedos-1.4-floppy/media.toml`) for the push that replaces the
/// published ones under the revision tags with the platform in the path: the
/// names change no manifest byte, but the export file name now on every
/// layer does, so the sets read back from the registry before that push have
/// other digests. The local build must give these digests: a pin that fails
/// means the two repositories no longer build the same set, and a set's
/// revision tag never moves.
const MEDIA_PINNED: [(&str, &str); 3] = [
    (
        "unemu/media/ibm-pc/freedos:1.4-r1",
        "sha256:545f287b5967093633426446465a48900b59d2049112666c2a4d509a0856de1f",
    ),
    (
        "unemu/media/ibm-pc/freedos:1.4-1.2m-r1",
        "sha256:53e81e4e4ef207feba6b32745157d1b1c8c2e6cbb4e9f69ce013ccee3e6e5924",
    ),
    (
        "unemu/media/ibm-pc/freedos:1.4-720k-r1",
        "sha256:29c2d9d224ec41aae0b3b5efe228d53e41c3d4bffd1a912cbc6534c5cfc4d5dc",
    ),
];

#[test]
fn two_media_builds_from_different_fixture_copies_give_the_published_sets() {
    if !media_present() {
        return;
    }
    let t = Temp::new("template-media-pins");
    let copy = t.0.join("fixtures");
    copy_media_fixtures(&copy);
    let first = Store::open(t.0.join("first")).unwrap();
    let second = Store::open(t.0.join("second")).unwrap();
    let a = template::build_media(&first, &vendor(), &allow, &mut |_| {}).unwrap();
    let b = template::build_media(&second, &copy, &allow, &mut |_| {}).unwrap();
    assert_eq!(identities(&a), identities(&b));
    for (built, source) in a.iter().zip(&template::MEDIA) {
        assert_eq!(built.version, source.tag);
        // The revision, then the release alias the store moved with it.
        assert_eq!(
            built.tags,
            [
                (
                    template::local_tag(&source.repository(), source.tag),
                    Outcome::Created
                ),
                (
                    template::local_tag(&source.repository(), source.alias()),
                    Outcome::Created
                ),
            ]
        );
        assert_eq!(
            first.tag_descriptor(&built.tags[1].0).unwrap().digest,
            built.descriptor.digest
        );
    }
    let tagged: Vec<_> = a
        .iter()
        .map(|b| (b.tags[0].0.clone(), b.descriptor.digest.to_string()))
        .collect();
    let pinned: Vec<_> = MEDIA_PINNED
        .iter()
        .map(|(tag, digest)| (tag.to_string(), digest.to_string()))
        .collect();
    assert_eq!(
        tagged, pinned,
        "the local installation sets are no longer the published ones"
    );
    let again = template::build_media(&first, &vendor(), &allow, &mut |_| {}).unwrap();
    assert_eq!(identities(&again), identities(&a));
    assert!(again
        .iter()
        .flat_map(|b| &b.tags)
        .all(|(_, o)| *o == Outcome::Present));
}

/// A minimal ORAS-style pull out of the local store: each layer's stored
/// bytes written into the empty folder `to` under the layer's
/// `org.opencontainers.image.title`, the way `oras pull` names the files it
/// pulls. A layer without a name, a name that is not one plain file name and
/// a name another layer already took, ignoring case as macOS and Windows do,
/// fail the pull. Returns the names in manifest order.
fn oras_pull(store: &Store, image: &image_store::Descriptor, to: &std::path::Path) -> Vec<String> {
    use std::io::Write;
    let manifest =
        Manifest::decode_for_summary(&store.read_verified(image, MAX_SMALL_BLOB).unwrap()).unwrap();
    image_store::export_name::check(&manifest.layers).unwrap();
    fs::create_dir_all(to).unwrap();
    let mut names: Vec<String> = Vec::new();
    for layer in &manifest.layers {
        let name = layer
            .annotations
            .get("org.opencontainers.image.title")
            .unwrap_or_else(|| panic!("layer {} has no file name", layer.digest));
        assert!(
            !matches!(name.as_str(), "" | "." | "..") && !name.contains(['/', '\\']),
            "{name:?} is not a plain file name"
        );
        assert!(
            names
                .iter()
                .all(|n| n.to_lowercase() != name.to_lowercase()),
            "{name:?} names two layers"
        );
        let bytes = fs::read(store.blob_file_path(&layer.digest, layer.size).unwrap()).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to.join(name))
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert_eq!(
            image_store::Digest::of(&fs::read(to.join(name)).unwrap()),
            layer.digest
        );
        names.push(name.clone());
    }
    names
}

/// The file name of a fixture path, without its folders.
fn file_name(relative: &str) -> String {
    std::path::Path::new(relative)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

/// What `zstd -d` does to a pulled `NAME.zst` in `folder`: writes `NAME`
/// beside it, the frame's decoded bytes, and returns them. The frame is
/// decoded from the pulled file's own bytes, imported into an empty store
/// for the layer `layer` they are.
fn zstd_d(folder: &std::path::Path, name: &str, layer: &image_store::Descriptor) -> Vec<u8> {
    let decoded_name = name
        .strip_suffix(".zst")
        .expect("a compressed layer's name ends in .zst");
    let scratch = Store::open(folder.join(".decode")).unwrap();
    scratch
        .import(
            fs::File::open(folder.join(name)).unwrap(),
            Some(&layer.digest),
        )
        .unwrap();
    let bytes = scratch.read_verified(layer, 1 << 30).unwrap();
    fs::remove_dir_all(folder.join(".decode")).unwrap();
    fs::write(folder.join(decoded_name), &bytes).unwrap();
    bytes
}

/// Every layer of each package and set names its file, so a generic OCI
/// client writes the fixtures' own file names: the ROM image, its source
/// archive and the README of a package; the disks, the licence index, the
/// source archive and the metadata of a set. A compressed layer's name has
/// `.zst` after the fixture's, and decoding the pulled file gives the
/// fixture byte for byte under its own name.
#[test]
fn an_oras_style_pull_writes_each_layer_under_its_fixture_file_name() {
    if !firmware_present(&template::FIRMWARE) || !media_present() {
        return;
    }
    let t = Temp::new("template-oras-pull");
    let store = Store::open(t.0.join("store")).unwrap();
    let pulled = t.0.join("pulled");
    // Pull one artifact, decode every `.zst` file as `zstd -d` would, and
    // compare each file with the fixture of its name; returns the pulled
    // names.
    let pull_and_decode = |image: &image_store::Descriptor, folder: &str, fixtures: &[&str]| {
        let to = pulled.join(folder);
        let names = oras_pull(&store, image, &to);
        let manifest =
            Manifest::decode_for_summary(&store.read_verified(image, MAX_SMALL_BLOB).unwrap())
                .unwrap();
        for ((layer, name), relative) in manifest.layers.iter().zip(&names).zip(fixtures) {
            let fixture = fs::read(vendor().join(relative)).unwrap();
            let file = file_name(relative);
            if layer.is_compressed() {
                assert_eq!(*name, format!("{file}.zst"));
                assert_eq!(zstd_d(&to, name, layer), fixture, "{relative}");
            } else {
                assert_eq!(*name, file);
            }
            assert_eq!(fs::read(to.join(&file)).unwrap(), fixture, "{relative}");
        }
        names
    };
    let packages = template::build_firmware(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    for (built, source) in packages.iter().zip(&template::FIRMWARE) {
        let names = pull_and_decode(
            &built.descriptor,
            source.catalogue_id,
            &[source.rom, source.source_archive],
        );
        assert_eq!(
            names,
            [
                format!("{}.zst", file_name(source.rom)),
                format!("{}.zst", file_name(source.source_archive)),
                "README.md".into()
            ]
        );
    }
    let sets = template::build_media(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    for (built, source) in sets.iter().zip(&template::MEDIA) {
        let files: Vec<&str> = source
            .disks
            .iter()
            .chain(source.release.extras.iter().map(|(file, _)| file))
            .map(|file| file.path)
            .collect();
        let names = pull_and_decode(&built.descriptor, source.tag, &files);
        let mut expected: Vec<String> = files
            .iter()
            .map(|path| format!("{}.zst", file_name(path)))
            .collect();
        expected.push("metadata.json".into());
        assert_eq!(names, expected);
        assert_eq!(names[0], "x86BOOT.img.zst");
    }
}
