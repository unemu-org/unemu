// SPDX-License-Identifier: AGPL-3.0-or-later
//! A machine needs at least what the firmware packages it binds declare
//! (machine-images §5.6): New machine… and Save as new image… fold each
//! package's `unemu.min-app-version` into the image's. Every PC artifact
//! written now names the platform `ibm-pc`, which release 0.0.3 first reads,
//! and this build reads nothing newer, so no package this build can read
//! lifts a new machine above 0.0.3 yet: the fold shows on a package that
//! declares a release this build does not meet, which every writer refuses,
//! naming the package. Only the fetched open fixtures are used.
#[path = "support/open_image.rs"]
mod open_image;

use image_store::{
    compat::{level, AppVersion, Requirement, MIN_APP_VERSION},
    BlobSource, Descriptor, Manifest, MAX_SMALL_BLOB,
};
use pc_image::{
    create::{self, PackageRom},
    firmware_import::{self, Statement},
    machine, revision, PcProvider, PAYLOAD_TYPES,
};

const BIOS: &str = "unemu-at-bios-1.0.2-unemu.3";
/// The first release that reads the platform id `ibm-pc`.
const RENAMED_IN: AppVersion = AppVersion::new(0, 0, 3);

fn requirement(store: &image_store::Store, image: &Descriptor) -> Requirement {
    let bytes = store.read_verified(image, MAX_SMALL_BLOB).unwrap();
    Requirement::of(&Manifest::decode_with_types(&bytes, PAYLOAD_TYPES).unwrap())
}

#[test]
fn a_new_machine_inherits_the_minimum_version_of_its_firmware_packages() {
    let Some(open) = open_image::Open::new() else {
        return;
    };
    let store = &open.store;
    let catalogue = pc_image::firmware::catalogue();
    let plain = create::create(store, &open.image([None, None]), "lab:plain", None).unwrap();
    assert_eq!(requirement(store, &plain), Requirement::Release(RENAMED_IN));

    // An edit states the newest release this build reads: a new revision.
    let raised_tag = format!("private/firmware/ibm-pc/{BIOS}:r2");
    firmware_import::edit(
        store,
        &format!("private/firmware/ibm-pc/{BIOS}:r1"),
        &raised_tag,
        &Statement {
            title: Some("Raised BIOS".into()),
            min_app_version: Some(level()),
            ..Statement::default()
        },
        catalogue,
    )
    .unwrap();
    let packages = create::packages(store).unwrap();
    let raised = packages
        .iter()
        .find(|p| p.reference == raised_tag)
        .expect("the raised revision is listed");
    assert_eq!(
        Requirement::of(&raised.manifest),
        Requirement::Release(level())
    );
    assert_eq!(
        pc_image::compat::packages_floor(std::iter::once(raised)).unwrap(),
        level()
    );
    let rom = raised
        .config
        .roms
        .iter()
        .find(|r| r.catalogue_id.as_deref() == Some(BIOS))
        .unwrap();
    let mut image = open.image([None, None]);
    image.firmware[0] = PackageRom::new(raised, rom);
    let preview = create::preview(&image, store).unwrap();
    assert_eq!(Requirement::of(&preview), Requirement::Release(level()));
    let machine = create::create(store, &image, "lab:raised", None).unwrap();
    assert_eq!(requirement(store, &machine), Requirement::Release(level()));
    // The same hardware with the unraised package needs what its platform id
    // needs.
    assert_eq!(
        Requirement::of(&create::preview(&open.image([None, None]), store).unwrap()),
        Requirement::Release(RENAMED_IN)
    );
}

/// A copy of the BIOS package that declares `marker`, tagged `tag` as a newer
/// release would tag it in a store both share.
fn marked(open: &open_image::Open, marker: &str, tag: &str) -> Descriptor {
    let package = open
        .packages
        .iter()
        .find(|p| p.reference == format!("private/firmware/ibm-pc/{BIOS}:r1"))
        .unwrap();
    let mut manifest = package.manifest.clone();
    manifest
        .annotations
        .insert(MIN_APP_VERSION.into(), marker.into());
    let copy = open.store.import_manifest(&manifest).unwrap();
    open.store.tag_as_newer_release(tag, &copy).unwrap();
    copy
}

/// A package that declares a release this build does not meet, a newer one
/// or one that does not parse, is folded into every machine that binds it,
/// so New machine… and both kinds of Save refuse the machine, naming the
/// package.
#[test]
fn a_machine_binding_a_package_this_release_cannot_read_is_refused() {
    let Some(open) = open_image::Open::new() else {
        return;
    };
    let store = &open.store;
    let plain = create::create(store, &open.image([None, None]), "lab:plain", None).unwrap();
    for (marker, needs) in [
        ("99.0.0", "needs unemupc 99.0.0 or newer"),
        ("soon", "needs a newer unemupc"),
    ] {
        let tag = format!("private/firmware/ibm-pc/marked-{marker}:r1");
        let package = marked(&open, marker, &tag);
        let refused = |error: pc_image::Error| {
            let error = error.to_string();
            assert!(
                error.contains(&tag) && error.contains(needs),
                "{marker}: {error}"
            );
        };
        // New machine… with the package's BIOS.
        let packages = create::packages(store).unwrap();
        let listed = packages.iter().find(|p| p.reference == tag).unwrap();
        let rom = listed
            .config
            .roms
            .iter()
            .find(|r| r.catalogue_id.as_deref() == Some(BIOS))
            .unwrap();
        let mut image = open.image([None, None]);
        image.firmware[0] = PackageRom::new(listed, rom);
        refused(create::preview(&image, store).unwrap_err());
        refused(create::create(store, &image, "lab:refused", None).unwrap_err());
        assert!(store.tag_descriptor("lab:refused").is_err());

        // An image whose firmware came from that package: Save as new
        // image… and a machine's Save.
        let bytes = store.read_verified(&plain, MAX_SMALL_BLOB).unwrap();
        let mut manifest = Manifest::decode_with_types(&bytes, PAYLOAD_TYPES).unwrap();
        let original = open
            .packages
            .iter()
            .find(|p| p.reference == format!("private/firmware/ibm-pc/{BIOS}:r1"))
            .unwrap();
        let sources = manifest.annotations["unemu.firmware.sources"].replace(
            &original.descriptor.digest.to_string(),
            &package.digest.to_string(),
        );
        manifest
            .annotations
            .insert("unemu.firmware.sources".into(), sources);
        let bound = store
            .import_manifest_with_types(&manifest, PAYLOAD_TYPES)
            .unwrap();
        let bound_tag = format!("lab:bound-{marker}");
        store
            .update_tag(&bound_tag, &bound, None, &PcProvider)
            .unwrap();
        let source = revision::Source::open(store, &bound).unwrap();
        let mut settings = source.machine().clone();
        settings
            .get_mut("memory")
            .and_then(toml::Value::as_table_mut)
            .unwrap()
            .insert("extended_kib".into(), 2048.into());
        refused(revision::plan(store, &source, settings).unwrap_err());
        let id = machine::create_from_tag(store, &bound_tag, None, None).unwrap();
        refused(machine::commit(store, &id, "lab:saved", None, None, false).unwrap_err());
        assert!(store.tag_descriptor("lab:saved").is_err());
    }
}
