// SPDX-License-Identifier: AGPL-3.0-or-later
//! The PC platform id is `ibm-pc`, and `ibm-pc-at`, the id releases 0.0.1
//! and 0.0.2 wrote, stays a read-only alias; machine-images §1 (Platform
//! neutrality) and §5.6. A machine image, an installation set and a firmware
//! package as those releases wrote them are read, listed, pulled from a
//! registry, run and combined with artifacts that name `ibm-pc`; a store from
//! before keeps its bytes; every writer writes `ibm-pc` and declares release
//! 0.0.3, also when it derives from an artifact that names the retired id;
//! and an id no provider reads stays refused. Only the fetched open firmware
//! fixtures and synthetic disks are used.

#[path = "../../pc-image/tests/support/open_image.rs"]
mod open_image;

use image_registry::{
    credentials::Anonymous,
    pull,
    test_support::{MockRegistry, Policy},
    Client, Reference,
};
use image_store::{
    compat::{AppVersion, Requirement, MIN_APP_VERSION},
    firmware::Validator as FirmwareValidator,
    machine::PendingInsert,
    media_set::Validator as MediaValidator,
    platform::with_platform_id,
    summary::SummaryCache,
    BlobSource, ConfigValidator, Descriptor, Manifest, Store, MAX_SMALL_BLOB,
};
use open_image::Open;
use pc_image::{
    create::{self, PackageRom},
    firmware::PcFirmwareRules,
    firmware_import::{self, Statement},
    machine,
    media_set::PcMediaFormats,
    revision, PcProvider, PAYLOAD_TYPES,
};
use std::{collections::BTreeMap, ffi::OsString, path::Path};

const RETIRED: &str = "ibm-pc-at";
const CURRENT: &str = "ibm-pc";
/// The first release that reads `ibm-pc`.
const RENAMED_IN: AppVersion = AppVersion::new(0, 0, 3);

fn run(store: &Path, list: &[&str]) -> String {
    let mut args: Vec<OsString> = list.iter().map(OsString::from).collect();
    args.extend(["--store".into(), store.as_os_str().to_owned()]);
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli::command::run(&args, &mut output, &mut errors);
    let errors = String::from_utf8(errors).unwrap();
    assert_eq!(code, 0, "{list:?}: {errors}");
    String::from_utf8(output).unwrap()
}

fn firmware_rules() -> PcFirmwareRules<'static> {
    PcFirmwareRules(pc_image::firmware::catalogue())
}

/// The decoded manifest and config of `image`, decoded with `types`.
fn read(
    store: &Store,
    image: &Descriptor,
    types: &[image_store::PayloadType],
) -> (Manifest, String) {
    let manifest =
        Manifest::decode_with_types(&store.read_verified(image, MAX_SMALL_BLOB).unwrap(), types)
            .unwrap();
    let config = String::from_utf8(
        store
            .read_verified(&manifest.config, MAX_SMALL_BLOB)
            .unwrap(),
    )
    .unwrap();
    (manifest, config)
}

/// The platform the metadata layer of `manifest` names.
fn metadata_platform(store: &Store, manifest: &Manifest) -> String {
    let layer = manifest
        .layers
        .iter()
        .find(|l| l.media_type == image_store::media_type::METADATA)
        .unwrap();
    let bytes = store.read_verified(layer, MAX_SMALL_BLOB).unwrap();
    image_store::metadata::Metadata::decode(&bytes, &manifest.layers)
        .unwrap()
        .platform
}

/// `image` names `ibm-pc` in its manifest, config and metadata, and declares
/// 0.0.3.
fn assert_written_now(store: &Store, image: &Descriptor, types: &[image_store::PayloadType]) {
    let (manifest, config) = read(store, image, types);
    assert_eq!(manifest.platform().unwrap(), CURRENT, "{image:?}");
    assert!(
        config.contains("platform = \"ibm-pc\"\n"),
        "{image:?}: {config}"
    );
    if manifest
        .layers
        .iter()
        .any(|l| l.media_type == image_store::media_type::METADATA)
    {
        assert_eq!(metadata_platform(store, &manifest), CURRENT, "{image:?}");
    }
    assert_eq!(
        Requirement::of(&manifest),
        Requirement::Release(RENAMED_IN),
        "{image:?}"
    );
}

/// `image` as release 0.0.2 wrote it, tagged `tag`.
fn retired(
    store: &Store,
    image: &Descriptor,
    types: &[image_store::PayloadType],
    tag: &str,
    validator: &dyn ConfigValidator,
) -> Descriptor {
    let old = with_platform_id(store, image, types, RETIRED).unwrap();
    store.update_tag(tag, &old, None, validator).unwrap();
    let (manifest, config) = read(store, &old, types);
    assert_eq!(manifest.platform().unwrap(), RETIRED);
    assert!(config.contains("platform = \"ibm-pc-at\"\n"), "{config}");
    assert!(!manifest.annotations.contains_key(MIN_APP_VERSION));
    old
}

/// A blank 1.2M diskette, which the open image's A: drive takes.
fn diskette(dir: &Path) -> String {
    let path = dir.join("setup.img");
    std::fs::write(&path, vec![0xE5; 1_228_800]).unwrap();
    path.to_str().unwrap().to_owned()
}

/// Every artifact of the store, with what a release before the rename wrote:
/// the open packages and a machine bound to them as written now, an
/// installation set imported from a diskette, and their `ibm-pc-at` copies.
struct Lab {
    open: Open,
    packages: Vec<(String, Descriptor)>,
    new_machine: Descriptor,
    old_machine: Descriptor,
    new_set: Descriptor,
    old_set: Descriptor,
}

impl Lab {
    fn new() -> Option<Self> {
        let open = Open::new()?;
        let store = &open.store;
        let rules = firmware_rules();
        let mut packages = Vec::new();
        for package in &open.packages {
            assert_written_now(store, &package.descriptor, &[]);
            let id = package.config.roms[0].catalogue_id.clone().unwrap();
            let tag = format!("old/{id}:1");
            let old = retired(
                store,
                &package.descriptor,
                &[],
                &tag,
                &FirmwareValidator(&rules),
            );
            packages.push((tag, old));
        }
        let new_machine =
            create::create(store, &open.image([None, None]), "lab:new", None).unwrap();
        assert_written_now(store, &new_machine, PAYLOAD_TYPES);
        let old_machine = retired(store, &new_machine, PAYLOAD_TYPES, "lab:old", &PcProvider);
        let disk = diskette(&open.path);
        run(
            store.root(),
            &[
                "image", "media", "import", &disk, "--title", "Setup", "--tag", "set:new",
            ],
        );
        let new_set = store.tag_descriptor("set:new").unwrap();
        assert_written_now(store, &new_set, &[]);
        let old_set = retired(
            store,
            &new_set,
            &[],
            "set:old",
            &MediaValidator(&PcMediaFormats),
        );
        Some(Self {
            open,
            packages,
            new_machine,
            old_machine,
            new_set,
            old_set,
        })
    }

    fn store(&self) -> &Store {
        &self.open.store
    }
}

#[test]
fn artifacts_that_name_the_retired_id_are_read_and_listed() {
    let Some(lab) = Lab::new() else {
        return;
    };
    let store = lab.store();
    let rules = firmware_rules();
    // Read by every provider, and the old machine by the desktop's checks.
    store.resolve("lab:old", &PcProvider).unwrap();
    store
        .resolve("set:old", &MediaValidator(&PcMediaFormats))
        .unwrap();
    for (tag, _) in &lab.packages {
        store.resolve(tag, &FirmwareValidator(&rules)).unwrap();
    }
    revision::Source::open(store, &lab.old_machine).unwrap();
    pc_image::desktop::check(store, &lab.old_machine, None, || false).unwrap();

    // Listed: summaries without failures, and the old packages offered to
    // New machine.
    let mut cache = SummaryCache::open(store).unwrap();
    while !cache.pending().is_empty() {
        cache.rebuild(store, 64, &[], &[]).unwrap();
    }
    let old_images = std::iter::once(&lab.old_machine)
        .chain(std::iter::once(&lab.old_set))
        .chain(lab.packages.iter().map(|(_, d)| d));
    for image in old_images {
        assert!(cache.failure(&image.digest).is_none(), "{image:?}");
        let summary = cache.summary(&image.digest).unwrap();
        assert_eq!(summary.platform, RETIRED);
        assert!(pc_image::is_pc(&summary.platform));
        assert_eq!(
            pc_image::platform_name(&summary.platform),
            "IBM PC compatible"
        );
    }
    let offered: Vec<String> = create::packages(store)
        .unwrap()
        .into_iter()
        .map(|p| p.reference)
        .collect();
    for (tag, _) in &lab.packages {
        assert!(offered.contains(tag), "{tag}: {offered:?}");
    }
    let root = store.root();
    assert!(run(root, &["image", "ls"]).contains("lab:old\t"));
    let sets = run(root, &["image", "media", "ls"]);
    assert!(sets.contains("set:old\t"), "{sets}");
    let firmware = run(root, &["image", "firmware", "ls"]);
    for (tag, _) in &lab.packages {
        assert!(firmware.contains(&format!("{tag}\t")), "{firmware}");
    }
    // Machine-readable output names the platform by its current id.
    let shown = run(root, &["image", "media", "show", "set:old"]);
    assert!(
        shown.starts_with(&format!("set:old\t{}\t{CURRENT}\n", lab.old_set.digest)),
        "{shown}"
    );
}

#[test]
fn a_machine_that_names_the_retired_id_runs_and_combines_with_current_artifacts() {
    let Some(lab) = Lab::new() else {
        return;
    };
    let store = lab.store();
    let root = store.root();
    // An `ibm-pc-at` machine bound to `ibm-pc` firmware packages runs.
    let old = machine::create_from_tag(store, "lab:old", Some("Old"), None).unwrap();
    let output = run(root, &["image", "machine", "run", &old, "--max-steps", "1"]);
    assert!(!output.is_empty());
    run(
        root,
        &[
            "image",
            "run",
            "lab:old",
            "--workspace",
            "old-run",
            "--new",
            "--max-steps",
            "1",
        ],
    );
    // An `ibm-pc` machine takes an `ibm-pc-at` installation set.
    let new = machine::create_from_tag(store, "lab:new", Some("New"), None).unwrap();
    machine::set_pending_inserts(
        store,
        &new,
        vec![PendingInsert {
            drive: "A:".into(),
            set: lab.old_set.clone(),
            reference: "set:old".into(),
            medium: "disk-1".into(),
            all: false,
            journal: true,
        }],
    )
    .unwrap();
    let output = run(root, &["image", "machine", "run", &new, "--max-steps", "1"]);
    assert!(output.contains("\ninserted:A:\tdisk-1\n"), "{output}");

    // An `ibm-pc` machine made from `ibm-pc-at` firmware packages, as New
    // machine… makes it.
    let packages = create::packages(store).unwrap();
    let mut image = lab.open.image([None, None]);
    for rom in &mut image.firmware {
        let id = rom.rom().unwrap().catalogue_id.clone().unwrap();
        let package = packages
            .iter()
            .find(|p| p.reference == format!("old/{id}:1"))
            .unwrap();
        let entry = package
            .config
            .roms
            .iter()
            .find(|r| r.catalogue_id.as_deref() == Some(id.as_str()))
            .unwrap();
        *rom = PackageRom::new(package, entry);
    }
    let made = create::create(store, &image, "lab:old-firmware", None).unwrap();
    assert_written_now(store, &made, PAYLOAD_TYPES);
    let (manifest, _) = read(store, &made, PAYLOAD_TYPES);
    for (_, package) in &lab.packages {
        assert!(
            manifest.annotations["unemu.firmware.sources"].contains(&package.digest.to_string()),
            "{:?}",
            manifest.annotations
        );
    }
    let made = machine::create_from_tag(store, "lab:old-firmware", None, None).unwrap();
    run(
        root,
        &["image", "machine", "run", &made, "--max-steps", "1"],
    );
}

/// Each artifact a release before the rename wrote, pulled from a registry,
/// is the very same artifact in the store it lands in.
#[test]
fn artifacts_that_name_the_retired_id_are_pulled_unchanged() {
    let Some(lab) = Lab::new() else {
        return;
    };
    let source = lab.store();
    let registry = MockRegistry::start(Policy {
        anonymous_read: vec!["*".into()],
        ..Policy::default()
    });
    let mut artifacts = vec![
        ("lab/old", lab.old_machine.clone(), PAYLOAD_TYPES),
        ("lab/set", lab.old_set.clone(), &[][..]),
    ];
    for (_, package) in &lab.packages {
        artifacts.push(("lab/firmware", package.clone(), &[][..]));
    }
    let target = Store::open(lab.open.path.join("pulled")).unwrap();
    let rules = firmware_rules();
    let media = MediaValidator(&PcMediaFormats);
    let firmware = FirmwareValidator(&rules);
    let providers: [&dyn ConfigValidator; 3] = [&PcProvider, &media, &firmware];
    let client = Client::new(&registry.entry(false), Box::new(Anonymous)).unwrap();
    for (index, (repository, image, types)) in artifacts.into_iter().enumerate() {
        let (manifest, _) = read(source, &image, types);
        // The registry holds the stored bytes, compressed where the store
        // compressed them.
        for blob in std::iter::once(&manifest.config).chain(&manifest.layers) {
            let path = source.blob_file_path(&blob.digest, blob.size).unwrap();
            registry.seed_blob(repository, &std::fs::read(path).unwrap());
        }
        let tag = format!("v{index}");
        registry.seed_manifest(
            repository,
            &tag,
            &source.read_verified(&image, MAX_SMALL_BLOB).unwrap(),
        );
        let reference = Reference::parse(
            &format!("{}/{repository}:{tag}", registry.host()),
            "unused.example",
        )
        .unwrap();
        let local = format!("pulled/{index}:1");
        let report = pull::pull(&target, &client, &reference, &local, &providers, &|_| {}).unwrap();
        assert_eq!(report.image.digest, image.digest);
        assert_eq!(target.tag_descriptor(&local).unwrap().digest, image.digest);
    }
    target.resolve("pulled/0:1", &PcProvider).unwrap();
    target.resolve("pulled/1:1", &media).unwrap();
    target.resolve("pulled/2:1", &firmware).unwrap();
}

/// A store a release before the rename wrote loads, lists and resolves with
/// its index and every blob unchanged.
#[test]
fn a_store_from_before_the_rename_is_read_without_a_rewrite() {
    let Some(lab) = Lab::new() else {
        return;
    };
    let store = lab.store();
    // Only the retired copies stay tagged, as in a store of release 0.0.2.
    for tag in ["lab:new", "set:new"] {
        store
            .remove_tag(tag, &store.tag_descriptor(tag).unwrap().digest)
            .unwrap();
    }
    for package in &lab.open.packages {
        store
            .remove_tag(&package.reference, &package.descriptor.digest)
            .unwrap();
    }
    let root = store.root().to_owned();
    let snapshot = || {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.join("images")];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        files
    };
    let before = snapshot();
    let store = Store::open_existing(&root).unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    while !cache.pending().is_empty() {
        cache.rebuild(&store, 64, &[], &[]).unwrap();
    }
    let rules = firmware_rules();
    let tags = store.list_tags(None, 100).unwrap().tags;
    assert_eq!(tags.len(), 2 + lab.packages.len());
    for tag in &tags {
        assert!(
            cache.failure(&tag.manifest.digest).is_none(),
            "{}",
            tag.name
        );
        let validator: &dyn ConfigValidator = match tag.name.as_str() {
            "lab:old" => &PcProvider,
            "set:old" => &MediaValidator(&PcMediaFormats),
            _ => &FirmwareValidator(&rules),
        };
        let resolved = store.resolve(&tag.name, validator).unwrap();
        assert_eq!(resolved.manifest.platform().unwrap(), RETIRED);
    }
    assert_eq!(create::packages(&store).unwrap().len(), lab.packages.len());
    pc_image::desktop::check(&store, &lab.old_machine, None, || false).unwrap();
    assert_eq!(snapshot(), before, "the store's images were rewritten");
}

/// Every writer that derives from an artifact naming the retired id writes
/// `ibm-pc` and declares 0.0.3: Save as new image, a machine's Save, compress,
/// a catalogue edit and a firmware package edit. The machine digest of a
/// compressed or re-catalogued image stays its source's.
#[test]
fn writers_deriving_from_the_retired_id_write_the_current_one() {
    let Some(lab) = Lab::new() else {
        return;
    };
    let store = lab.store();
    // Save as new image… with another memory size.
    let source = revision::Source::open(store, &lab.old_machine).unwrap();
    let mut settings = source.machine().clone();
    settings
        .get_mut("memory")
        .and_then(toml::Value::as_table_mut)
        .unwrap()
        .insert("extended_kib".into(), 2048.into());
    let plan = revision::plan(store, &source, settings).unwrap();
    let revised = revision::publish(store, &plan, "lab:revised", None).unwrap();
    assert_written_now(store, &revised, PAYLOAD_TYPES);
    // A machine's Save as new image.
    let id = machine::create_from_tag(store, "lab:old", None, None).unwrap();
    let saved = machine::commit(store, &id, "lab:saved", None, None, false).unwrap();
    assert_written_now(store, &saved, PAYLOAD_TYPES);
    // Compress keeps the config, so the machine stays the same machine; the
    // manifest and the metadata it writes name the current id.
    let compressed = pc_image::compress_image(store, "lab:old", "lab:compressed").unwrap();
    let (manifest, _) = read(store, &compressed, PAYLOAD_TYPES);
    assert_eq!(manifest.platform().unwrap(), CURRENT);
    assert_eq!(metadata_platform(store, &manifest), CURRENT);
    assert_eq!(Requirement::of(&manifest), Requirement::Release(RENAMED_IN));
    let (old, _) = read(store, &lab.old_machine, PAYLOAD_TYPES);
    assert_eq!(manifest.config.digest, old.config.digest);
    store.resolve("lab:compressed", &PcProvider).unwrap();
    let media = MediaValidator(&PcMediaFormats);
    let set = store
        .compress_image("set:old", "set:compressed", &media)
        .unwrap();
    let (manifest, _) = read(store, &set, &[]);
    assert_eq!(manifest.platform().unwrap(), CURRENT);
    assert_eq!(metadata_platform(store, &manifest), CURRENT);
    assert_eq!(Requirement::of(&manifest), Requirement::Release(RENAMED_IN));
    // A catalogue edit writes new metadata and a new manifest, never a new
    // config.
    store
        .edit_library(&lab.old_machine.digest, |entry| {
            entry.catalogue.labels = Some(vec!["edited".into()]);
            Ok(())
        })
        .unwrap();
    let edited = store
        .publish_overlay(&lab.old_machine, "lab:edited", &PcProvider)
        .unwrap();
    let resolved = store.resolve("lab:edited", &PcProvider).unwrap();
    assert_eq!(resolved.manifest.platform().unwrap(), CURRENT);
    assert_eq!(
        Requirement::of(&resolved.manifest),
        Requirement::Release(RENAMED_IN)
    );
    assert_eq!(resolved.manifest.config.digest, old.config.digest);
    let metadata =
        image_store::metadata::Metadata::decode(&resolved.metadata, &resolved.manifest.layers)
            .unwrap();
    assert_eq!(metadata.platform, CURRENT);
    assert_ne!(edited.digest, lab.old_machine.digest);
    // A firmware package edit: a new revision of the package.
    let (tag, _) = &lab.packages[0];
    let edit = firmware_import::edit(
        store,
        tag,
        "old/edited:2",
        &Statement {
            title: Some("Edited package".into()),
            ..Statement::default()
        },
        pc_image::firmware::catalogue(),
    )
    .unwrap();
    assert_written_now(store, &edit.descriptor, &[]);
}

/// An id that is neither `ibm-pc` nor its alias is no PC artifact, as before
/// the rename: no provider reads it and New machine does not offer it.
#[test]
fn an_unknown_platform_id_stays_refused() {
    let Some(lab) = Lab::new() else {
        return;
    };
    let store = lab.store();
    let rules = firmware_rules();
    let unknown = "ibm-pc-xt";
    let machine = with_platform_id(store, &lab.new_machine, PAYLOAD_TYPES, unknown).unwrap();
    let error = store.resolve_descriptor(&machine, &PcProvider).unwrap_err();
    assert!(error.to_string().contains("platform mismatch"), "{error}");
    let set = with_platform_id(store, &lab.new_set, &[], unknown).unwrap();
    assert!(store
        .resolve_descriptor(&set, &MediaValidator(&PcMediaFormats))
        .is_err());
    let (_, package) = &lab.packages[0];
    let package = with_platform_id(store, package, &[], unknown).unwrap();
    assert!(store
        .resolve_descriptor(&package, &FirmwareValidator(&rules))
        .is_err());
    assert!(store
        .update_tag("unknown:1", &package, None, &FirmwareValidator(&rules))
        .is_err());
    let (manifest, _) = read(store, &machine, PAYLOAD_TYPES);
    let media = MediaValidator(&PcMediaFormats);
    let providers: [&dyn ConfigValidator; 2] = [&PcProvider, &media];
    assert!(image_registry::select_provider(&providers, &manifest).is_err());
    assert!(!pc_image::is_pc(unknown));
    assert_eq!(pc_image::platform_name(unknown), unknown);
}

/// The official builders write the current id: the template machines with
/// the firmware packages they bind and, when their fixtures are fetched, the
/// installation sets.
#[test]
fn template_builds_write_the_current_id() {
    let Some(lab) = Lab::new() else {
        return;
    };
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor");
    let store = Store::open(lab.open.path.join("templates")).unwrap();
    let allow = |_: &str, _: &Manifest, _: &[u8]| -> Result<(), String> { Ok(()) };
    let built = pc_image::template::build(&store, &vendor, &allow, &mut |_| {}).unwrap();
    assert_eq!(built.len(), pc_image::template::TEMPLATES.len());
    for template in &built {
        assert_written_now(&store, &template.descriptor, PAYLOAD_TYPES);
    }
    for package in create::packages(&store).unwrap() {
        assert_written_now(&store, &package.descriptor, &[]);
    }
    if pc_image::template::check_media_fixtures(&vendor).is_ok() {
        let sets = pc_image::template::build_media(&store, &vendor, &allow, &mut |_| {}).unwrap();
        for set in &sets {
            assert_written_now(&store, &set.descriptor, &[]);
        }
    }
}
