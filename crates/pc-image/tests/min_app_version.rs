// SPDX-License-Identifier: AGPL-3.0-or-later
//! A machine needs at least what the firmware packages it binds declare
//! (machine-images §5.6): New machine… and Save as new image… fold each
//! package's `unemu.min-app-version` into the image's. Only the fetched open
//! fixtures are used.
#[path = "support/open_image.rs"]
mod open_image;

use image_store::{
    compat::{Requirement, PACKAGE},
    BlobSource, Descriptor, Manifest, MAX_SMALL_BLOB,
};
use pc_image::{
    create::{self, PackageRom},
    firmware_import::{self, Statement},
    PAYLOAD_TYPES,
};

const BIOS: &str = "unemu-at-bios-1.0.2-unemu.3";

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
    assert_eq!(requirement(store, &plain), Requirement::None);

    // An edit raises the BIOS package's minimum version: a new revision.
    let raised_tag = format!("firmware/{BIOS}:2");
    firmware_import::edit(
        store,
        &format!("firmware/{BIOS}:1"),
        &raised_tag,
        &Statement {
            min_app_version: Some(PACKAGE),
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
        Requirement::Release(PACKAGE)
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
    assert_eq!(Requirement::of(&preview), Requirement::Release(PACKAGE));
    let machine = create::create(store, &image, "lab:raised", None).unwrap();
    assert_eq!(requirement(store, &machine), Requirement::Release(PACKAGE));
    // The same hardware with the unraised package needs nothing.
    assert_eq!(
        Requirement::of(&create::preview(&open.image([None, None]), store).unwrap()),
        Requirement::None
    );
}
