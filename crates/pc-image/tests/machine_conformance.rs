// SPDX-License-Identifier: AGPL-3.0-or-later
//! The conformance harness on machine recipes: a pinned open-at 1.3.0
//! recipe, as `image template config` emits it, is built into a scratch
//! store of its own from the libraries alone, its uses made there by the
//! package recipes named with it.
use image_store::{Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB};
use recipe::library::Library;
use recipe::pin::{Conformance, Conformed, Pinned};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// What `recipe pin` writes into the open-at 1.3.0 recipe.
const OPEN_AT: &str = "sha256:04bad34426efdd67e2bec1fb7eb33cdd54ea96e1b0e8e52550e5622e6211ecd2";

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

/// The folders of the package recipes the open-at uses pin.
fn packages() -> Vec<PathBuf> {
    ["unemu-at-bios", "vgabios", "xtide-universal-bios"]
        .into_iter()
        .map(|folder| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data/recipes")
                .join(folder)
        })
        .collect()
}

/// The fixture folders of the three open firmware packages.
fn firmware_library() -> Vec<Library> {
    let vendor = pc_harness::test_support::root().join("vendor");
    ["unemu-at-bios", "vgabios", "xtide-universal-bios"]
        .into_iter()
        .map(|folder| Library::scan(&vendor.join(folder), None).unwrap())
        .collect()
}

/// The emitted open-at 1.3.0 recipe, pinned in an author store that holds
/// its packages: the recipe's path and the pinned manifest.
fn pinned_open_at(root: &Path) -> (PathBuf, Store, Descriptor) {
    let author = store(&root.join("author"));
    pc_image::template::build_firmware(
        &author,
        &[pc_harness::test_support::root().join("vendor")],
        &|_, _, _| Ok(()),
        &mut |_| {},
    )
    .unwrap();
    let recipes = root.join("recipes");
    fs::create_dir_all(&recipes).unwrap();
    for document in pc_image::template::inputs::emit().unwrap() {
        fs::write(recipes.join(document.name), document.bytes).unwrap();
    }
    let path = recipes.join("open-at.machine.toml");
    let pinned = recipe::template::pin_path(
        recipe::build::Request {
            path: &path,
            providers: recipe::Providers(&[&pc_image::recipe::PcRecipes]),
            store: &author,
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
    let Pinned::Pinned { report, digest, .. } = pinned else {
        panic!("{pinned:#?}")
    };
    assert_eq!(digest, OPEN_AT);
    (path, author, report.published.descriptor)
}

fn conform(paths: &[PathBuf], scratch: &Store, libraries: &mut [Library]) -> Vec<Conformance> {
    recipe::pin::conformance(
        paths,
        recipe::Providers(&[&pc_image::recipe::PcRecipes]),
        &|_| Some(&unemu_pc_core::PcCore),
        scratch,
        libraries,
    )
    .unwrap()
}

/// Named before its packages, the machine recipe still finds them: they
/// are built into the scratch store first, from the library, never
/// downloaded and never read from another store.
#[test]
fn a_pinned_open_at_conforms_from_the_libraries_and_its_package_recipes() {
    if !fixtures() {
        return;
    }
    let temp = image_store::test_support::TempDir::new("machine-conformance");
    let (machine, _author, _) = pinned_open_at(temp.path());
    let scratch = store(&temp.path().join("scratch"));
    let mut paths = vec![machine.clone()];
    paths.extend(packages());
    let results = conform(&paths, &scratch, &mut firmware_library());
    assert_eq!(
        results[0],
        Conformance {
            file: machine.display().to_string(),
            result: Conformed::Conforms {
                reference: "unemu/machines/ibm-pc/open-at:1.3.0".into(),
                digest: OPEN_AT.into(),
            },
        }
    );
    let (text, all) = recipe::pin::render(&results);
    assert!(all, "{text}");
    assert!(
        text.ends_with("4 pinned recipes: 4 conform, 0 differ, 0 not built; 0 drafts skipped\n"),
        "{text}"
    );
}

/// A lock and digest that name another manifest than the one the recipe
/// composes: the run says it differs, as for a package whose payloads
/// disagree with its lock.
#[test]
fn a_machine_recipe_pinned_to_another_manifest_differs() {
    if !fixtures() {
        return;
    }
    let temp = image_store::test_support::TempDir::new("machine-conformance-differs");
    let (machine, author, pinned) = pinned_open_at(temp.path());
    let mut manifest = Manifest::decode_for_summary(
        &image_store::BlobSource::read_verified(&author, &pinned, MAX_SMALL_BLOB).unwrap(),
    )
    .unwrap();
    manifest.layers[0]
        .annotations
        .insert("org.opencontainers.image.title".into(), "changed".into());
    let bytes = manifest.encode_with_types(pc_image::PAYLOAD_TYPES).unwrap();
    let other = Digest::of(&bytes);
    let lock = recipe::lock::dir(&machine);
    let mut files: Vec<Vec<u8>> = recipe::lock::read(&lock)
        .unwrap()
        .into_iter()
        .filter(|(name, _)| name != pinned.digest.hex())
        .map(|(_, bytes)| bytes)
        .collect();
    files.insert(0, bytes);
    fs::remove_dir_all(&lock).unwrap();
    recipe::lock::write(&lock, &files).unwrap();
    let text = fs::read_to_string(&machine)
        .unwrap()
        .replace(OPEN_AT, &other.to_string());
    fs::write(&machine, text).unwrap();

    let scratch = store(&temp.path().join("scratch"));
    let mut paths = vec![machine];
    paths.extend(packages());
    let results = conform(&paths, &scratch, &mut firmware_library());
    match &results[0].result {
        Conformed::Differs(mismatches) => assert!(
            mismatches
                .iter()
                .any(|m| m.contains("the machine lock disagrees")),
            "{mismatches:#?}"
        ),
        other => panic!("{other:#?}"),
    }
    let (text, all) = recipe::pin::render(&results);
    assert!(!all);
    assert!(
        text.ends_with("4 pinned recipes: 3 conform, 1 differ, 0 not built; 0 drafts skipped\n"),
        "{text}"
    );
}

/// Without the firmware files in the libraries, neither the packages nor
/// the machine are built; the machine's line is its plan's summary, its
/// three uses missing.
#[test]
fn a_machine_recipe_whose_uses_the_libraries_lack_is_not_built() {
    if !fixtures() {
        return;
    }
    let temp = image_store::test_support::TempDir::new("machine-conformance-missing");
    let (machine, _author, _) = pinned_open_at(temp.path());
    let empty = temp.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    for named_packages in [false, true] {
        let scratch = store(&temp.path().join(format!("scratch-{named_packages}")));
        let mut paths = vec![machine.clone()];
        if named_packages {
            paths.extend(packages());
        }
        let results = conform(
            &paths,
            &scratch,
            &mut [Library::scan(&empty, None).unwrap()],
        );
        assert_eq!(
            results[0].result,
            Conformed::Missing("missing 3 of 4 inputs".into()),
            "{results:#?}"
        );
        let (text, all) = recipe::pin::render(&results);
        assert!(!all);
        assert!(
            text.starts_with(&format!(
                "{}: not built: missing 3 of 4 inputs\n",
                machine.display()
            )),
            "{text}"
        );
    }
}
