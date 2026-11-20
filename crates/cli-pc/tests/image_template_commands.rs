// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image template build` from the open fixtures: the template machines
//! only, reproducible digests, dry runs, the push guard of the official
//! namespace, a POST of the open-at template, and `image media import`
//! reproducing the installation sets the unemu-media repository publishes;
//! machine-images §5.5. Nothing here reaches a network registry.
#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

use image_registry::{guard::Redistribution, Reference, RegistryEntry, DEFAULT_REGISTRY};
use image_store::Store;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

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

/// The open firmware fixtures the templates need, the only fixtures
/// `image template build` reads, or `None` when they are not fetched.
fn fetched() -> Option<()> {
    for source in &pc_image::template::FIRMWARE {
        fixtures::fixture(source.rom)?;
        fixtures::fixture(source.source_archive)?;
    }
    Some(())
}

/// The local tags of both templates, in build order.
const TEMPLATE_TAGS: [&str; 4] = [
    "open-at:1.2.0",
    "open-at:latest",
    "open-at-games:1.2.0",
    "open-at-games:latest",
];

fn run(args: &[&dyn AsRef<std::ffi::OsStr>]) -> (u8, String, String) {
    let args: Vec<OsString> = args.iter().map(|a| a.as_ref().to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run_with_firmware_sources(
        &args,
        &mut out,
        &mut err,
        &cli_pc::command::FirmwareSources::default(),
    );
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn build(store: &Path, dry_run: bool) -> (u8, String, String) {
    let vendor = vendor();
    let mut args: Vec<&dyn AsRef<std::ffi::OsStr>> = vec![
        &"image",
        &"template",
        &"build",
        &"--store",
        &store,
        &"--vendor",
        &vendor,
    ];
    if dry_run {
        args.push(&"--dry-run");
    }
    run(&args)
}

/// `(tag, digest, kind, outcome)` of each tag line.
fn rows(output: &str) -> Vec<(String, String, String, String)> {
    output
        .lines()
        .filter(|line| line.contains("\tsha256:"))
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 4, "{line}");
            (
                fields[0].into(),
                fields[1].into(),
                fields[2].into(),
                fields[3].into(),
            )
        })
        .collect()
}

#[test]
fn the_build_is_reproducible_all_open_for_unemu_and_its_dry_run_writes_nothing() {
    if fetched().is_none() {
        return;
    }
    let t = Temp::new("template-cli");
    let (code, first, errors) = build(&t.0.join("a"), false);
    assert_eq!(code, 0, "{errors}");
    assert!(first.ends_with("guard:allowed\n"), "{first}");
    let first = rows(&first);
    let tags: Vec<_> = first.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(tags, TEMPLATE_TAGS);
    assert!(
        first.iter().all(|r| r.2 == "machine" && r.3 == "created"),
        "{first:?}"
    );
    // The firmware packages the templates bind stay in the store under the
    // tags `image pull` writes for the published packages, without a line.
    let store = Store::open_existing(t.0.join("a")).unwrap();
    for source in &pc_image::template::FIRMWARE {
        let tag = pc_image::template::local_tag(
            &source.repository(),
            pc_image::template::FIRMWARE_REVISION,
        );
        assert!(tag.ends_with(":r1"), "{tag}");
        assert!(store.tag_descriptor(&tag).is_ok(), "{tag}");
    }
    drop(store);
    let digests = |rows: &[(String, String, String, String)]| {
        rows.iter()
            .map(|r| (r.0.clone(), r.1.clone(), r.2.clone()))
            .collect::<Vec<_>>()
    };

    // Again into the same store: every tag is in place. Into another store:
    // the same digests.
    let (code, again, errors) = build(&t.0.join("a"), false);
    assert_eq!(code, 0, "{errors}");
    let again = rows(&again);
    assert_eq!(digests(&again), digests(&first));
    assert!(again.iter().all(|r| r.3 == "present"), "{again:?}");
    let (code, other, errors) = build(&t.0.join("b"), false);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(digests(&rows(&other)), digests(&first));

    // The dry run compares with the store and writes neither it nor a new one.
    let before = fs::read(t.0.join("a/images/index.json")).unwrap();
    let (code, dry, errors) = build(&t.0.join("a"), true);
    assert_eq!(code, 0, "{errors}");
    assert!(dry.ends_with("dry run; no blobs or tag written\n"), "{dry}");
    let dry = rows(&dry);
    assert_eq!(digests(&dry), digests(&first));
    assert!(dry.iter().all(|r| r.3 == "present"), "{dry:?}");
    assert_eq!(fs::read(t.0.join("a/images/index.json")).unwrap(), before);
    let (code, fresh, errors) = build(&t.0.join("missing"), true);
    assert_eq!(code, 0, "{errors}");
    assert!(rows(&fresh).iter().all(|r| r.3 == "new"));
    assert!(!t.0.join("missing").exists());

    // `image push` of each tag to the official namespace would pass the guard:
    // every blob open, every copyleft ROM and medium with its source archive.
    let store = Store::open_existing(t.0.join("a")).unwrap();
    let entry = RegistryEntry::builtin(DEFAULT_REGISTRY);
    for (tag, digest, _, _) in &first {
        let target = Reference::parse(tag, DEFAULT_REGISTRY).unwrap();
        assert_eq!(target.namespace(), "unemu", "{tag}");
        // The plan both programs make, built here as platforms::push_plan
        // builds it: every local mark plus the firmware catalogue's
        // restrictions.
        let plan = image_registry::push::plan_with_catalogue(
            &store,
            tag,
            target,
            Some(&entry),
            |manifest| pc_image::firmware::restricted(manifest, pc_image::firmware::catalogue()),
        )
        .unwrap();
        assert_eq!(plan.image().digest.to_string(), *digest);
        assert_eq!(plan.verdict(), &Ok(()), "{tag}");
        assert!(plan
            .closure()
            .iter()
            .all(|c| c.effective == Redistribution::Open));
    }
}

#[test]
fn a_store_that_records_an_open_rom_as_user_supplied_refuses_the_build() {
    if fetched().is_none() {
        return;
    }
    let t = Temp::new("template-cli-refused");
    let store = t.0.join("store");
    // Imported without its source archive, the copyleft BIOS stays
    // user-supplied, and that mark follows its bytes into every image.
    let roms = t.0.join("roms");
    fs::create_dir_all(&roms).unwrap();
    fs::copy(
        vendor().join(pc_image::template::FIRMWARE[0].rom),
        roms.join("bios.bin"),
    )
    .unwrap();
    let (code, output, errors) = run(&[
        &"image",
        &"firmware",
        &"import",
        &roms,
        &"--store",
        &store,
        &"--create",
        &"--for",
        &"unemu-at-bios-1.0.2-unemu.3",
    ]);
    assert_eq!(code, 0, "{errors}");
    assert!(output.contains("kept user-supplied"), "{output}");
    let index = fs::read(store.join("images/index.json")).unwrap();
    // The dry run judges with the marks of the store it reports on as well.
    // The template that binds the ROM is refused, not its package: the
    // package is a local copy, never pushed by this command.
    for dry_run in [true, false] {
        let (code, output, errors) = build(&store, dry_run);
        assert_ne!(code, 0, "{output}");
        assert!(
            errors.contains("unemu/open-at:1.2.0 refused")
                && errors.contains("every blob must be open"),
            "{errors}"
        );
        assert!(!output.contains("guard:allowed"), "{output}");
        assert!(!output.contains("\tsha256:"), "{output}");
        if dry_run {
            assert_eq!(fs::read(store.join("images/index.json")).unwrap(), index);
        }
    }
    let store = Store::open_existing(&store).unwrap();
    for tag in TEMPLATE_TAGS {
        assert!(store.tag_descriptor(tag).is_err(), "{tag}");
    }
    // The user's own import and the three local packages the build made
    // before the template was refused; they stay.
    let mut tags: Vec<_> = store
        .list_tags(None, 10)
        .unwrap()
        .tags
        .into_iter()
        .map(|t| t.name)
        .collect();
    tags.sort_unstable();
    assert_eq!(
        tags,
        [
            "private/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1",
            "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1",
            "unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1",
            "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1",
        ]
    );
}

#[test]
fn both_templates_post_with_the_open_bios_and_xtide_finds_their_blank_disk() {
    if fetched().is_none() {
        return;
    }
    let t = Temp::new("template-cli-post");
    let store = Store::open(t.0.join("store")).unwrap();
    cli_pc::image::build_templates(&store, &vendor()).unwrap();
    for tag in ["open-at:1.2.0", "open-at-games:1.2.0"] {
        post(&store, tag);
    }
}

/// Run `tag`'s POST to the boot loader, then let it try A: and C:.
fn post(store: &Store, tag: &str) {
    let owner = store.workspace(&tag.replace([':', '.'], "-")).unwrap();
    let prepared = pc_image::workspace::prepare_start(&owner, tag, false).unwrap();
    let launch =
        pc_image::launch::from_session(prepared, &unemu_pc_core::PcCore, "Template POST".into())
            .unwrap();
    let mut pc = core_launch::start_with(&launch.prepared, |request| {
        unemu_pc_core::PcCore.create_machine(request)
    })
    .unwrap();
    let machine = pc.assembled();
    let checkpoints = device::ByteCapture::new(4096);
    let post_index = machine.device_index[&assembly::DeviceId::Post];
    machine
        .machine
        .bus_mut()
        .device_mut(post_index)
        .unwrap()
        .byte_output()
        .unwrap()
        .bind_sink(Box::new(checkpoints.clone()));
    let mut post = Vec::new();
    for step in 1..=60u64 {
        machine
            .run_session(&assembly::RunPolicy::natural(Some(step * 1_000_000), None))
            .unwrap();
        post.extend(checkpoints.drain().unwrap());
        if post.contains(&0x00) {
            // INT 19h tries the empty A:, then C: through XTIDE.
            let at = {
                use cpu::Cpu;
                machine.machine.cpu().retired()
            };
            machine
                .run_session(&assembly::RunPolicy::natural(Some(at + 30_000_000), None))
                .unwrap();
            break;
        }
    }
    // The open BIOS checkpoint stream through the option-ROM scan (40h–43h,
    // XTIDE at C8000h) to the boot loader (Kiselev errno.inc).
    assert_eq!(
        post,
        [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x11, 0x20, 0x21, 0x22,
            0x24, 0x25, 0x30, 0x31, 0x40, 0x41, 0x42, 0x43, 0x00
        ],
        "{tag}"
    );
    let capture = machine.capture_text(assembly::DeviceId::Vga).unwrap();
    let video::Frame::Text(grid) = &capture.frame else {
        panic!("expected VGA text")
    };
    let rows = grid.rows_as_strings();
    let row = |text: &str| rows.iter().any(|row| row.contains(text));
    assert!(
        row("Master at 1F0h: UNEMU ATA-2 PIO DISK"),
        "{tag}: {rows:#?}"
    );
    assert!(row("Booting C"), "{tag}: {rows:#?}");
    assert!(row("Boot sector not found"), "{tag}: {rows:#?}");
    machine.finish_persistence().unwrap();
}

/// The open-at template with another title, from the store's packages.
fn variant(store: &Store, title: &str) -> pc_image::create::NewImage {
    let packages = pc_image::create::packages(store).unwrap();
    let firmware = pc_image::template::FIRMWARE
        .iter()
        .map(|source| {
            packages
                .iter()
                .find_map(|p| {
                    p.config
                        .roms
                        .iter()
                        .find(|r| r.catalogue_id.as_deref() == Some(source.catalogue_id))
                        .map(|r| pc_image::create::PackageRom::new(p, r))
                })
                .unwrap()
        })
        .collect();
    pc_image::create::NewImage {
        hardware: pc_image::template::TEMPLATES[0].hardware().unwrap(),
        firmware,
        title: Some(title.into()),
        description: None,
    }
}

#[test]
fn the_dry_run_names_a_version_conflict_and_what_latest_does() {
    if fetched().is_none() {
        return;
    }
    let t = Temp::new("template-cli-states");
    let state = |output: &str, tag: &str| {
        rows(output)
            .into_iter()
            .find(|r| r.0 == tag)
            .map(|r| r.3)
            .unwrap_or_else(|| panic!("{tag}: {output}"))
    };

    // open-at:1.2.0 names another image before the first build.
    let path = t.0.join("conflict");
    let store = Store::open(&path).unwrap();
    // Only the packages the templates bind: every template is refused.
    let firmware_only =
        |_: &str, _: &image_store::Manifest, _: &[u8]| Err("machines later".to_owned());
    assert!(pc_image::template::build(&store, &vendor(), &firmware_only, &mut |_| {}).is_err());
    let other = pc_image::create::create(
        &store,
        &variant(&store, "Another AT"),
        "open-at:1.2.0",
        None,
    )
    .unwrap();
    let (code, dry, errors) = build(&path, true);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(
        state(&dry, "open-at:1.2.0"),
        format!("conflict:{}", other.digest)
    );
    assert_eq!(state(&dry, "open-at:latest"), "new");
    // The report names the templates alone, never the packages they bind.
    let reported: Vec<_> = rows(&dry).into_iter().map(|r| r.0).collect();
    assert_eq!(reported, TEMPLATE_TAGS);
    let (code, _, errors) = build(&path, false);
    assert_ne!(code, 0);
    assert!(errors.contains("a version tag never moves"), "{errors}");

    // latest names a lower version, then a higher one.
    let path = t.0.join("aliases");
    let (code, _, errors) = build(&path, false);
    assert_eq!(code, 0, "{errors}");
    let store = Store::open_existing(&path).unwrap();
    let point = |to: &image_store::Descriptor| {
        let current = store.tag_descriptor("open-at:latest").unwrap().digest;
        store
            .update_tag("open-at:latest", to, Some(&current), &pc_image::PcProvider)
            .unwrap();
    };
    let lower =
        pc_image::create::create(&store, &variant(&store, "Older AT"), "open-at:0.9.0", None)
            .unwrap();
    point(&lower);
    let (code, dry, errors) = build(&path, true);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(
        state(&dry, "open-at:latest"),
        format!("moves:{}", lower.digest)
    );
    assert_eq!(state(&dry, "open-at:1.2.0"), "present");
    let higher =
        pc_image::create::create(&store, &variant(&store, "Newer AT"), "open-at:2.0.0", None)
            .unwrap();
    point(&higher);
    let (code, dry, errors) = build(&path, true);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(
        state(&dry, "open-at:latest"),
        format!("kept:{}", higher.digest)
    );
    let (code, output, errors) = build(&path, false);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(
        state(&output, "open-at:latest"),
        format!("kept:{}", higher.digest)
    );
    assert_eq!(
        store.tag_descriptor("open-at:latest").unwrap().digest,
        higher.digest
    );

    // A package tag of the store names another package: the build refuses
    // before any template, and so does the dry run, which reports no package.
    let xub = pc_image::template::local_tag(
        &pc_image::template::FIRMWARE[2].repository(),
        pc_image::template::FIRMWARE_REVISION,
    );
    let bios = pc_image::template::local_tag(
        &pc_image::template::FIRMWARE[0].repository(),
        pc_image::template::FIRMWARE_REVISION,
    );
    let ours = store.tag_descriptor(&xub).unwrap();
    let another = store.tag_descriptor(&bios).unwrap();
    // A revision tag never moves, so the other package takes the tag after
    // it was deleted, in a store of an earlier release that kept no record
    // of deleted revisions.
    store.remove_tag(&xub, &ours.digest).unwrap();
    fs::remove_file(path.join("local/retired-version-tags.toml")).unwrap();
    store
        .update_tag(
            &xub,
            &another,
            None,
            &image_store::firmware::Validator(&pc_image::firmware::PcFirmwareRules(
                pc_image::firmware::catalogue(),
            )),
        )
        .unwrap();
    let index = fs::read(path.join("images/index.json")).unwrap();
    let conflict = format!(
        "{xub} already names {}, not {}",
        another.digest, ours.digest
    );
    for dry_run in [true, false] {
        let (code, output, errors) = build(&path, dry_run);
        assert_ne!(code, 0, "dry run {dry_run}: {output}");
        assert!(
            errors.contains(&conflict) && errors.contains("remove or rename that local tag"),
            "dry run {dry_run}: {errors}"
        );
        assert!(!output.contains("\tsha256:"), "dry run {dry_run}: {output}");
        assert!(
            !output.contains("guard:allowed"),
            "dry run {dry_run}: {output}"
        );
        assert_eq!(fs::read(path.join("images/index.json")).unwrap(), index);
    }
}

/// The build reads the firmware fixtures alone: the installation sets are
/// not its images any more, so their fixtures are neither checked nor
/// needed. Without a firmware fixture it stops and names the fetch script.
#[test]
fn the_build_needs_only_the_firmware_fixtures() {
    if fetched().is_none() {
        return;
    }
    let t = Temp::new("template-cli-firmware-only");
    let copy = t.0.join("vendor");
    for source in &pc_image::template::FIRMWARE {
        for relative in [source.rom, source.source_archive] {
            let to = copy.join(relative);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(vendor().join(relative), to).unwrap();
        }
    }
    let empty = t.0.join("empty");
    let store = t.0.join("store");
    for dry_run in [true, false] {
        let with = |vendor: &Path| {
            let mut args: Vec<&dyn AsRef<std::ffi::OsStr>> = vec![
                &"image",
                &"template",
                &"build",
                &"--store",
                &store,
                &"--vendor",
                &vendor,
            ];
            if dry_run {
                args.push(&"--dry-run");
            }
            run(&args)
        };
        let (code, output, errors) = with(&copy);
        assert_eq!(code, 0, "{errors}");
        let tags: Vec<_> = rows(&output).into_iter().map(|r| r.0).collect();
        assert_eq!(tags, TEMPLATE_TAGS, "{output}");
        assert_eq!(store.exists(), !dry_run);

        let (code, output, errors) = with(&empty);
        assert_ne!(code, 0, "{output}");
        assert!(
            errors.contains("missing fixture") && errors.contains("vendor/unemu-at-bios/fetch.sh"),
            "{errors}"
        );
        assert!(!errors.contains("freedos"), "{errors}");
        assert!(!output.contains("\tsha256:"), "{output}");
    }
}

/// The fixtures of every installation set, or `None` when they are not
/// fetched.
fn media_fetched() -> Option<()> {
    for file in pc_image::template::media_files() {
        fixtures::fixture(file.path)?;
    }
    Some(())
}

/// The `image media import` command line that imports one installation set
/// into `store`, with the software recipes' metadata: the disks, the title and tag,
/// each disk's label and format, the extras with their DOS names, the
/// catalogue record with its version, edition and set labels, and the open
/// statement. Each extra's kind comes from its extension.
fn media_import_args(source: &pc_image::template::MediaSource, store: &Path) -> Vec<OsString> {
    let release = source.release;
    let mut args: Vec<OsString> = vec!["image".into(), "media".into(), "import".into()];
    args.extend(source.disks.iter().map(|d| vendor().join(d.path).into()));
    let mut flag = |name: &str, value: OsString| args.extend([name.into(), value]);
    let tag = pc_image::template::local_tag(&source.repository(), source.tag);
    flag("--title", release.title.into());
    flag("--tag", tag.into());
    for (index, disk) in source.disks.iter().enumerate() {
        flag("--label", format!("{}={}", index + 1, disk.name).into());
        flag(
            "--format",
            format!("{}={}", index + 1, source.format).into(),
        );
    }
    for (file, _) in release.extras {
        let mut extra = vendor().join(file.path).into_os_string();
        extra.push(":");
        extra.push(file.name);
        flag("--extra", extra);
    }
    flag("--publisher", release.publisher.into());
    flag("--developer", release.publisher.into());
    flag("--year", release.year.to_string().into());
    for genre in release.genres {
        flag("--genre", genre.into());
    }
    flag("--platform", "DOS".into());
    flag("--description", source.description.into());
    flag("--version", release.release.into());
    flag("--edition", release.edition.into());
    flag("--work", "freedos".into());
    flag("--work-id", "wikidata=Q473751".into());
    flag(
        "--synopsis",
        "An open source DOS-compatible operating system, by the FreeDOS Project.".into(),
    );
    flag("--need", "video=vga,ega".into());
    flag(
        "--requirements",
        "This edition should run on any hardware that can run FreeDOS and has EGA or better graphics".into(),
    );
    for label in release.labels {
        flag("--set-label", label.into());
    }
    flag("--license", release.license.into());
    flag("--license-text", release.terms.into());
    flag("--source", release.source.into());
    flag("--store", store.into());
    args.extend(["--open".into(), "--create".into()]);
    args
}

fn run_args(args: &[OsString]) -> (u8, String, String) {
    run(&args
        .iter()
        .map(|a| a as &dyn AsRef<std::ffi::OsStr>)
        .collect::<Vec<_>>())
}

/// The digest an import printed on its last line, after its tag.
fn imported_digest(output: &str, tag: &str) -> String {
    let last = output.lines().last().unwrap_or_default();
    let digest = last.strip_prefix(&format!("{tag}\t"));
    digest.unwrap_or_else(|| panic!("{output}")).to_owned()
}

/// A media recipe imports the official installation sets with the command
/// line alone: each set it imports is byte for byte the set the local build
/// gives, so the same manifest digest, which the pc-image tests pin to the
/// published one. Without the set labels, version and edition it would be
/// another set.
#[test]
fn the_media_import_command_reproduces_each_installation_set() {
    if media_fetched().is_none() {
        return;
    }
    let t = Temp::new("template-media-import");
    let store = Store::open(t.0.join("built")).unwrap();
    let allow = |_: &str, _: &image_store::Manifest, _: &[u8]| Ok(());
    let built = pc_image::template::build_media(&store, &vendor(), &allow, &mut |_| {}).unwrap();
    assert_eq!(built.len(), pc_image::template::MEDIA.len());
    for (source, set) in pc_image::template::MEDIA.iter().zip(&built) {
        let tag = pc_image::template::local_tag(&source.repository(), source.tag);
        assert_eq!(set.tags[0].0, tag);
        let args = media_import_args(source, &t.0.join("imported"));
        let (code, output, errors) = run_args(&args);
        assert_eq!(code, 0, "{tag}: {errors}");
        assert_eq!(
            imported_digest(&output, &tag),
            set.descriptor.digest.to_string(),
            "{tag}: {output}"
        );
    }

    let source = &pc_image::template::MEDIA[0];
    let tag = pc_image::template::local_tag(&source.repository(), source.tag);
    let mut args = media_import_args(source, &t.0.join("without"));
    let mut index = 0;
    while index < args.len() {
        if ["--set-label", "--version", "--edition"].contains(&args[index].to_str().unwrap()) {
            args.drain(index..index + 2);
        } else {
            index += 1;
        }
    }
    let (code, output, errors) = run_args(&args);
    assert_eq!(code, 0, "{errors}");
    assert_ne!(
        imported_digest(&output, &tag),
        built[0].descriptor.digest.to_string(),
        "{output}"
    );
}
