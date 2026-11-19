// SPDX-License-Identifier: AGPL-3.0-or-later
//! Draft builds of recipes through the `ibm-pc` provider and the importers
//! (recipes.md §3.1, §6.9, §6.12, §6.14).
//!
//! - The open firmware recipes of the corpus build the packages
//!   `image firmware import` makes from the same fixtures, as the template
//!   build imports them with the metadata flags: the same configs byte for
//!   byte, and the same manifests.
//! - The three FreeDOS recipes build sets whose configs equal byte for byte
//!   those the importer makes from the same disks and extras as `image media
//!   import` states them; the edition archive is not needed, as its disks
//!   are found loose.
//! - Nothing is tagged when an input is missing or a step refuses.
//! - A build is a pure function of its inputs: two builds into two stores,
//!   with the network refused and the library files' host times changed in
//!   between, give the same artifact.
//! - Ken's Labyrinth and Azuramoon build from the user's own files
//!   (`user_rom_` tests, ignored; skipped without the files unless
//!   `UNEMUPC_REQUIRE_USER_ROMS=1`), and Ken's disk is the one the FAT
//!   builder's golden pins.

use image_store::metadata::Metadata;
use image_store::{ConfigValidator, Digest, Manifest, Store};
use pc_image::recipe::PcRecipes;
use pc_image::template;
use recipe::build::{build_path, Fetcher, Outcome, Request};
use recipe::library::Library;
use recipe::provider::Provider;
use recipe::store_index::StoreIndex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

const CORPUS: &str = "../../backlog/docs/design/recipe-corpus";

const PROVIDERS: [&dyn Provider; 1] = [&PcRecipes];

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "recipe-build-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
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

fn corpus(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(CORPUS)
        .join(relative)
}

fn allow(_: &str, _: &Manifest, _: &[u8]) -> Result<(), String> {
    Ok(())
}

/// A fetcher that fails the test: these builds must not reach the network.
struct NoNetwork;
impl Fetcher for NoNetwork {
    fn fetch(&self, url: &str, _: u64, _: &mut dyn std::io::Write) -> Result<(), String> {
        panic!("the build tried to download {url}");
    }
}

/// Build `recipe` into `store` from the libraries named, with `--fetch`
/// consent given to a fetcher that refuses, and the lines it logged.
fn build(
    recipe: &Path,
    store: &Store,
    libraries: &[PathBuf],
    keep_as: Option<&str>,
) -> (Outcome, Vec<String>) {
    build_with(recipe, store, libraries, keep_as, Some(&NoNetwork))
}

/// [`build`] with the fetcher given; without one (no `--fetch` consent) an
/// input that is not found makes the outcome `Missing`.
fn build_with(
    recipe: &Path,
    store: &Store,
    libraries: &[PathBuf],
    keep_as: Option<&str>,
    fetcher: Option<&dyn Fetcher>,
) -> (Outcome, Vec<String>) {
    let index = StoreIndex::load(store).unwrap();
    let mut libraries: Vec<Library> = libraries
        .iter()
        .map(|dir| Library::scan(dir, None).unwrap())
        .collect();
    // Content found in the store is read through the validators of the
    // `ibm-pc` artifacts.
    let media = image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats);
    let rules = pc_image::firmware::PcFirmwareRules(pc_image::firmware::catalogue());
    let firmware = image_store::firmware::Validator(&rules);
    let content = recipe::build::StoreContent::new(store, vec![&media, &firmware]);
    let mut lines = Vec::new();
    let outcome = build_path(
        Request {
            path: recipe,
            providers: recipe::Providers(&PROVIDERS),
            store,
            index: Some(&index),
            store_read: Some(&content),
            libraries: &mut libraries,
            fetcher,
            keep_as,
            find_outputs: true,
        },
        &mut |line| lines.push(line.to_owned()),
    )
    .unwrap();
    (outcome, lines)
}

fn built(outcome: Outcome) -> recipe::build::Report {
    match outcome {
        Outcome::Built(report) => *report,
        other => panic!("not built: {other:#?}"),
    }
}

fn tags(store: &Store) -> Vec<String> {
    store
        .list_tags(None, 1000)
        .unwrap()
        .tags
        .into_iter()
        .map(|tag| tag.name)
        .collect()
}

/// The config and metadata bytes of the image under `tag`.
fn stored(store: &Store, tag: &str, validator: &dyn ConfigValidator) -> (Vec<u8>, Metadata) {
    let image = store.resolve(tag, validator).unwrap();
    let metadata = Metadata::decode(&image.metadata, &image.manifest.layers).unwrap();
    (image.config, metadata)
}

const FIRMWARE: [&str; 3] = [
    "unemu-at-bios/unemu-at-bios-1.0.2-unemu.3.firmware.toml",
    "vgabios/vgabios-0.9d-unemu.1-std.firmware.toml",
    "xtide-universal-bios/xub-r638-xtp-ata1f0.firmware.toml",
];

fn firmware_present() -> bool {
    template::FIRMWARE.iter().all(|source| {
        [source.rom, source.source_archive]
            .into_iter()
            .all(|relative| fixtures::fixture(relative).is_some())
    })
}

#[test]
fn the_open_firmware_recipes_build_the_packages_the_importer_makes() {
    if !firmware_present() {
        return;
    }
    let t = Temp::new("firmware");
    let from_recipes = Store::open(t.path("recipes")).unwrap();
    let imported = Store::open(t.path("imported")).unwrap();
    let packages = template::build_firmware(&imported, &vendor(), &allow, &mut |_| {}).unwrap();
    let catalogue = pc_image::firmware::catalogue();
    let validator =
        image_store::firmware::Validator(&pc_image::firmware::PcFirmwareRules(catalogue));
    for (file, package) in FIRMWARE.iter().zip(&packages) {
        let (outcome, lines) = build(&corpus(file), &from_recipes, &[vendor()], None);
        assert!(lines.is_empty(), "nothing to fetch: {lines:?}");
        let report = built(outcome);
        let tag = &package.tags[0].0;
        assert_eq!(&report.reference, tag);
        let recipe_config = from_recipes.resolve(tag, &validator).unwrap().config;
        let import_config = imported.resolve(tag, &validator).unwrap().config;
        assert_eq!(
            String::from_utf8(recipe_config).unwrap(),
            String::from_utf8(import_config).unwrap(),
            "{file}"
        );
        // More than the config: the same package.
        assert_eq!(
            report.published.descriptor.digest, package.descriptor.digest,
            "{file}"
        );
        assert!(report.published.created);
    }
    // A second build finds every package already built.
    for file in FIRMWARE {
        let report = built(build(&corpus(file), &from_recipes, &[vendor()], None).0);
        assert!(!report.published.created, "{file}");
    }
}

const FREEDOS: [&str; 3] = [
    "freedos-1.4-floppy/freedos-1.4.media.toml",
    "freedos-1.4-floppy/freedos-1.4-1.2m.media.toml",
    "freedos-1.4-floppy/freedos-1.4-720k.media.toml",
];

fn media_present() -> bool {
    template::media_files()
        .into_iter()
        .all(|file| fixtures::fixture(file.path).is_some())
        && fixtures::fixture("freedos/floppy-1.4-licenses.txt").is_some()
}

#[test]
fn the_freedos_recipes_build_the_configs_the_importer_makes() {
    if !media_present() {
        return;
    }
    let t = Temp::new("freedos");
    let from_recipes = Store::open(t.path("recipes")).unwrap();
    let imported = Store::open(t.path("imported")).unwrap();
    let sets = template::build_media(&imported, &vendor(), &allow, &mut |_| {}).unwrap();
    let validator = image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats);
    for (file, set) in FREEDOS.iter().zip(&sets) {
        // The licence index, our own text with LF line ends, lies in the
        // vendor folder; the disks and the source archive below it. The
        // edition archive is nowhere: its disks are found loose. The
        // recipes are pinned, and the extra the `text` step makes, the
        // index with DOS line ends, lies there too as the template set's
        // `floppy-1.4/LICENSES.TXT`: it is found by its locked payload
        // instead of made, so the LF text is not needed (recipes §3.4).
        let (outcome, lines) = build(
            &corpus(file),
            &from_recipes,
            &[vendor().join("freedos")],
            None,
        );
        assert!(lines.is_empty(), "nothing to fetch: {lines:?}");
        let report = built(outcome);
        assert_eq!(
            report
                .kept
                .iter()
                .map(|k| (k.input.as_str(), k.how.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    "edition",
                    format!("not needed: its {} members were found", set_disks(file)).as_str()
                ),
                (
                    "licenses-lf",
                    "not needed: the outputs made from it were found"
                ),
                ("source", "kept as stored"),
            ]
        );
        // In the library for the first set, in the store for the others.
        let rendered = report.render();
        let line = rendered
            .lines()
            .find(|l| l.starts_with("  extra licenses (LICENSES.TXT): found "))
            .unwrap_or_else(|| panic!("{rendered}"));
        assert!(line.ends_with(", not made"), "{line}");
        let tag = &set.tags[0].0;
        assert_eq!(&report.reference, tag);
        let (recipe_config, recipe_metadata) = stored(&from_recipes, tag, &validator);
        let (import_config, import_metadata) = stored(&imported, tag, &validator);
        assert_eq!(
            String::from_utf8(recipe_config).unwrap(),
            String::from_utf8(import_config).unwrap(),
            "{file}"
        );
        // One title id for both writers: the work, the repository's last
        // component.
        assert_eq!(recipe_metadata.titles[0].id, "freedos");
        assert_eq!(import_metadata.titles[0].id, "freedos");
        // The recipe's record is the merge with the work file, handed
        // through unchanged.
        let title = &recipe_metadata.titles[0];
        assert_eq!(title.title, "FreeDOS");
        assert_eq!(title.work_id.as_deref(), Some("freedos"));
        assert!(title.synopsis.is_some());
    }
}

fn set_disks(file: &str) -> usize {
    match file {
        f if f.ends_with("720k.media.toml") => 12,
        f if f.ends_with("1.2m.media.toml") => 8,
        _ => 7,
    }
}

const HEAD: &str = "recipe = \"0.1\"\n\n[media]\nname = \"private/media/ibm-pc/synthetic\"\nplatform = \"ibm-pc\"\nrelease = \"1.0\"\nrevision = 1\n\n[title]\ntitle = \"Synthetic\"\n\n";

fn sha(bytes: &[u8]) -> String {
    Digest::of(bytes).hex().to_owned()
}

/// A stored zip archive of `members`, each with the DOS date and time
/// words given (PKWARE APPNOTE §4.3.7 and §4.3.12).
fn zip(members: &[(&str, &[u8], u16, u16)]) -> Vec<u8> {
    let crc = |bytes: &[u8]| {
        let mut c = !0u32;
        for &b in bytes {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    };
    let (mut out, mut directory) = (Vec::new(), Vec::new());
    for (name, bytes, time, date) in members {
        let offset = out.len() as u32;
        let common = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&time.to_le_bytes());
            v.extend_from_slice(&date.to_le_bytes());
            v.extend_from_slice(&crc(bytes).to_le_bytes());
            v.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            v.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(bytes);
        directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        directory.extend_from_slice(&20u16.to_le_bytes());
        common(&mut directory);
        directory.extend_from_slice(&[0; 10]);
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&directory);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A recipe with all three steps: two members unpacked from an archive with
/// the archive's times, a game folder as a file set, both built into one
/// 1.44M disk in listed order, and a text with DOS line ends as an extra.
/// The files lie in `lib`.
fn three_steps(t: &Temp, lib: &Path) -> PathBuf {
    let (a, b) = (b"first member".as_slice(), b"second member".as_slice());
    // 1993-03-27 13:58:22.
    let (time, date) = ((13 << 11) | (58 << 5) | 11, (13 << 9) | (3 << 5) | 27);
    let archive = zip(&[("A.EXE", a, time, date), ("DATA/B.DAT", b, time, date)]);
    let run = b"run the game";
    let notes = b"line one\nline two\n";
    fs::create_dir_all(lib.join("game")).unwrap();
    fs::write(lib.join("GAME.ZIP"), &archive).unwrap();
    fs::write(lib.join("game/RUN.COM"), run).unwrap();
    fs::write(lib.join("notes.txt"), notes).unwrap();
    let text = format!(
        "{HEAD}[input.archive]\nname = \"GAME.ZIP\"\nsize = {}\nsha256 = \"{}\"\nurls = [\"https://example.org/GAME.ZIP\"]\n\n\
         [input.game]\nname = \"GAME\"\nfiles = [\n  {{ path = \"RUN.COM\", size = {}, sha256 = \"{}\", time = 1999-12-31T23:59:58 }},\n]\n\n\
         [input.notes]\nname = \"notes.txt\"\nsize = {}\nsha256 = \"{}\"\n\n\
         [[step]]\nid = \"files\"\nunpack = \"archive\"\nformat = \"zip\"\ncomplete = true\nmembers = [\n\
           {{ path = \"A.EXE\", time = 1993-03-27T13:58:22 }},\n\
           {{ path = \"DATA/B.DAT\" }},\n]\n\n\
         [[step]]\nid = \"disk\"\nfat = \"files\"\nformat = \"1.44M\"\nlabel = \"SYNTHETIC\"\nserial = 0x12345678\norder = \"listed\"\ntime = 2001-02-03T04:05:06\n\n\
         [[step]]\nid = \"folder\"\nfat = \"game\"\nformat = \"720K\"\n\n\
         [[step]]\nid = \"readme\"\ntext = \"notes\"\neol = \"crlf\"\n\n\
         [[medium]]\nid = \"game-disk\"\nlabel = \"The game\"\nformat = \"1.44M\"\nfrom = \"disk\"\n\n\
         [[medium]]\nid = \"run-disk\"\nlabel = \"The runner\"\nformat = \"720K\"\nfile_name = \"RUNNER.IMG\"\nfrom = \"folder\"\n\n\
         [[extra]]\nid = \"readme\"\nfile_name = \"README.TXT\"\nkind = \"document\"\ndos_name = \"README.TXT\"\nfrom = \"readme\"\n",
        archive.len(),
        sha(&archive),
        run.len(),
        sha(run),
        notes.len(),
        sha(notes),
    );
    let path = t.path("synthetic.media.toml");
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_build_runs_the_three_steps_and_hands_the_outputs_to_the_importer() {
    let t = Temp::new("steps");
    let lib = t.path("lib");
    let recipe = three_steps(&t, &lib);
    let store = Store::open(t.path("store")).unwrap();
    let report = built(build(&recipe, &store, std::slice::from_ref(&lib), None).0);
    assert_eq!(report.reference, "private/media/ibm-pc/synthetic:1.0-r1");
    assert_eq!(
        report
            .kept
            .iter()
            .map(|k| format!("{}: {}", k.input, k.how))
            .collect::<Vec<_>>(),
        [
            "archive: kept as files",
            "game: kept as files",
            "notes: kept as text with DOS line ends"
        ]
    );
    let validator = image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats);
    let image = store.resolve(&report.reference, &validator).unwrap();
    let config = image_store::media_set::Config::decode(&image.config).unwrap();
    assert_eq!(
        config
            .media
            .iter()
            .map(|m| (m.id.as_str(), m.label.as_str()))
            .collect::<Vec<_>>(),
        [("game-disk", "The game"), ("run-disk", "The runner")]
    );
    let metadata = Metadata::decode(&image.metadata, &image.manifest.layers).unwrap();
    assert_eq!(metadata.titles[0].id, "synthetic");
    let content = |digest: &Digest| {
        use std::io::Read;
        let mut bytes = Vec::new();
        image
            .content_reader(digest)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    };
    // The disk holds the members in listed order, with the stated time and
    // the step's time for the folder; the label and serial as stated.
    let disk = content(&config.media[0].blob);
    let volume = media::fat12::reader::Volume::open(&disk).unwrap();
    volume.check().unwrap();
    assert_eq!(volume.label().as_deref(), Some("SYNTHETIC"));
    assert_eq!(volume.read("A.EXE").unwrap(), b"first member");
    assert_eq!(volume.read("DATA\\B.DAT").unwrap(), b"second member");
    let root = volume.list("").unwrap();
    let a = root.iter().find(|e| e.name == "A.EXE").unwrap();
    // Last-write date and time words of the directory entry.
    let word = |at: usize| u16::from_le_bytes([a.raw[at], a.raw[at + 1]]);
    assert_eq!(
        (word(24), word(22)),
        ((13 << 9) | (3 << 5) | 27, (13 << 11) | (58 << 5) | 11)
    );
    assert_eq!(&disk[39..43], &0x1234_5678u32.to_le_bytes());
    let runner = content(&config.media[1].blob);
    let runner = media::fat12::reader::Volume::open(&runner).unwrap();
    assert_eq!(runner.read("RUN.COM").unwrap(), b"run the game");
    assert_eq!(content(&config.extras[0].blob), b"line one\r\nline two\r\n");
    assert_eq!(config.extras[0].filename, "README.TXT");
}

#[test]
fn a_build_is_a_pure_function_of_its_inputs() {
    let t = Temp::new("pure");
    let lib = t.path("lib");
    let recipe = three_steps(&t, &lib);
    let first = Store::open(t.path("first")).unwrap();
    let second = Store::open(t.path("second")).unwrap();
    // The network is refused (the fetcher panics), and the host times of
    // every library file change between the builds: neither reaches the
    // result.
    let a = built(build(&recipe, &first, std::slice::from_ref(&lib), None).0);
    for file in ["GAME.ZIP", "game/RUN.COM", "notes.txt"] {
        let handle = fs::File::options()
            .write(true)
            .open(lib.join(file))
            .unwrap();
        handle
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000))
            .unwrap();
    }
    let b = built(build(&recipe, &second, std::slice::from_ref(&lib), None).0);
    assert_eq!(a.published.descriptor, b.published.descriptor);
    assert_eq!(tags(&first), tags(&second));
}

/// The `ibm-pc` side of the `fat` step, `FatVolume::build`, names no clock,
/// network, environment, process, thread or host file; the FAT12 builder's
/// volume path behind it is checked the same way by `media`'s own tests
/// (`fat12::volume_tests`).
#[test]
fn the_pc_volume_build_reaches_no_clock_network_or_host() {
    let code = include_str!("../src/recipe.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    for forbidden in [
        "std::time",
        "Instant",
        "SystemTime",
        "std::net",
        "std::env",
        "std::process",
        "std::thread",
        "std::fs",
        "fs::",
    ] {
        assert!(!code.contains(forbidden), "recipe.rs names {forbidden}");
    }
}

#[test]
fn nothing_is_tagged_when_an_input_is_missing_or_a_step_refuses() {
    let t = Temp::new("missing");
    let lib = t.path("lib");
    let recipe = three_steps(&t, &lib);
    let store = Store::open(t.path("store")).unwrap();
    // Without the archive, the plan reports it missing and no step runs;
    // no consent to fetch is used for a plan that cannot complete without
    // a download (the fetcher here would panic on any request).
    fs::remove_file(lib.join("GAME.ZIP")).unwrap();
    let index = StoreIndex::load(&store).unwrap();
    let mut libraries = vec![Library::scan(&lib, None).unwrap()];
    let outcome = build_path(
        Request {
            path: &recipe,
            providers: recipe::Providers(&PROVIDERS),
            store: &store,
            index: Some(&index),
            store_read: None,
            libraries: &mut libraries,
            fetcher: None,
            keep_as: None,
            find_outputs: true,
        },
        &mut |line| panic!("nothing to log: {line}"),
    )
    .unwrap();
    let Outcome::Missing(plan) = outcome else {
        panic!("{outcome:#?}");
    };
    assert_eq!(plan.summary(), "missing 1 of 3 inputs");
    assert!(tags(&store).is_empty());

    // A text with CR LF already in it: the step refuses, nothing is tagged.
    let t = Temp::new("refused");
    let lib = t.path("lib");
    let recipe = three_steps(&t, &lib);
    let notes = b"line one\r\n";
    fs::write(lib.join("notes.txt"), notes).unwrap();
    let text = fs::read_to_string(&recipe).unwrap().replace(
        &format!("size = 18\nsha256 = \"{}\"", sha(b"line one\nline two\n")),
        &format!("size = {}\nsha256 = \"{}\"", notes.len(), sha(notes)),
    );
    fs::write(&recipe, text).unwrap();
    let store = Store::open(t.path("store")).unwrap();
    match build(&recipe, &store, &[lib], None).0 {
        Outcome::Failed { error, .. } => {
            assert!(error.contains("step readme: a CR at byte 8"), "{error}")
        }
        other => panic!("{other:#?}"),
    }
    assert!(tags(&store).is_empty());

    // The FreeDOS disks alone: the licence index and the source archive
    // are missing, and nothing is tagged.
    if !media_present() {
        return;
    }
    let store = Store::open(t.path("freedos")).unwrap();
    let (outcome, _) = build(
        &corpus(FREEDOS[0]),
        &store,
        &[vendor().join("freedos/floppy-1.4/144m")],
        None,
    );
    let Outcome::Missing(plan) = outcome else {
        panic!("{outcome:#?}");
    };
    assert_eq!(plan.summary(), "missing 2 of 3 inputs");
    assert!(tags(&store).is_empty());
}

/// A build starts by removing what unfinished builds left: the staging
/// folder of a process that no longer runs and partial downloads older than
/// a day. A fresh partial download, maybe another build's, stays; the
/// build's own staging folder is gone when it ends.
#[test]
fn a_build_sweeps_stale_staging_folders_and_partial_downloads() {
    let t = Temp::new("sweep");
    let lib = t.path("lib");
    let recipe = three_steps(&t, &lib);
    let store = Store::open(t.path("store")).unwrap();
    let staging = store.root().join(recipe::build::STAGING);
    let downloads = store.root().join(recipe::build::DOWNLOADS);
    fs::create_dir_all(&downloads).unwrap();
    // No process has this id: above every system's largest.
    fs::create_dir_all(staging.join("2000000000-0/medium-1")).unwrap();
    let lost_lock = store.root().join(recipe::build::LOST_LOCK);
    fs::create_dir_all(lost_lock.join("2000000000-0/images")).unwrap();
    let old = downloads.join(".partial-old-1-0");
    let fresh = downloads.join(".partial-fresh-1-0");
    fs::write(&old, b"old").unwrap();
    fs::write(&fresh, b"fresh").unwrap();
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 3600))
        .unwrap();
    built(build(&recipe, &store, std::slice::from_ref(&lib), None).0);
    #[cfg(unix)]
    assert_eq!(fs::read_dir(&staging).unwrap().count(), 0, "nothing left");
    #[cfg(unix)]
    assert_eq!(fs::read_dir(&lost_lock).unwrap().count(), 0, "nothing left");
    assert!(!old.exists());
    assert!(fresh.exists());
}

#[test]
fn keep_as_tags_the_result_in_the_recipes_repository_under_another_tag() {
    let t = Temp::new("keep");
    let lib = t.path("lib");
    let recipe = three_steps(&t, &lib);
    let store = Store::open(t.path("store")).unwrap();
    let report = built(
        build(
            &recipe,
            &store,
            std::slice::from_ref(&lib),
            Some("1.0-mine-r1"),
        )
        .0,
    );
    assert_eq!(
        report.reference,
        "private/media/ibm-pc/synthetic:1.0-mine-r1"
    );
    assert!(!tags(&store).contains(&"private/media/ibm-pc/synthetic:1.0-r1".to_owned()));
    for refused in ["1.0-r1", "other/repo:1.0-r1", ""] {
        match build(&recipe, &store, std::slice::from_ref(&lib), Some(refused)).0 {
            Outcome::Failed { error, .. } => assert!(error.starts_with("--keep-as"), "{error}"),
            other => panic!("{refused}: {other:#?}"),
        }
    }
}

/// The user-ROM roots of the volume goldens: `vendor/user-roms`, then
/// `UNEMUPC_USER_ROMS_DIR`, those that exist.
fn user_roots() -> Vec<PathBuf> {
    std::iter::once(vendor().join("user-roms"))
        .chain(std::env::var_os("UNEMUPC_USER_ROMS_DIR").map(PathBuf::from))
        .filter(|root| root.is_dir())
        .collect()
}

/// The build of a recipe from the user's files, or `None` with one SKIPPED
/// line when they are not all there (a failure under
/// `UNEMUPC_REQUIRE_USER_ROMS=1`). The build has no `--fetch` consent: a
/// recipe that names download URLs for an input (Ken's LABFULL.ZIP) must
/// report it missing, never reach for the network, where the user's files
/// are absent (CI).
fn build_from_user_files(
    title: &str,
    recipe: &Path,
    store: &Store,
    extra: &[PathBuf],
) -> Option<recipe::build::Report> {
    let mut libraries = user_roots();
    libraries.extend(extra.iter().cloned());
    match build_with(recipe, store, &libraries, None, None).0 {
        Outcome::Built(report) => Some(*report),
        Outcome::Missing(plan) => {
            let message = format!(
                "SKIPPED: user-supplied files of {title} not found ({}; set UNEMUPC_USER_ROMS_DIR or see vendor/user-roms/README.md)",
                plan.summary()
            );
            assert!(
                std::env::var_os("UNEMUPC_REQUIRE_USER_ROMS").as_deref()
                    != Some(std::ffi::OsStr::new("1")),
                "{message}"
            );
            println!("{message}");
            None
        }
        other => panic!("{title}: {other:#?}"),
    }
}

#[test]
#[ignore = "user-supplied files: Ken's Labyrinth 2.01"]
fn user_rom_kenslab_builds_from_the_users_files_and_its_disk_is_the_pinned_one() {
    // The corpus does not carry unemu-media's licence index, a bundled
    // input; the copy here states a text of our own in its place. The
    // disk does not depend on it.
    let t = Temp::new("kenslab");
    let folder = corpus("kenslab-2.01");
    let licences = b"Ken's Labyrinth: README.TXT states its licence.\n";
    let text = fs::read_to_string(folder.join("kenslab-2.01.media.toml"))
        .unwrap()
        .replace(
            "size = 3442\nsha256 = \"b4b52913919b8f481289fe0beef2a76a812b4014e6a2b09357caa2aec91f4ff7\"",
            &format!("size = {}\nsha256 = \"{}\"", licences.len(), sha(licences)),
        );
    let recipe = t.path("kenslab-2.01.media.toml");
    fs::write(&recipe, text).unwrap();
    fs::copy(
        folder.join("kenslab.work.toml"),
        t.path("kenslab.work.toml"),
    )
    .unwrap();
    fs::write(t.path("licenses.txt"), licences).unwrap();
    let store = Store::open(t.path("store")).unwrap();
    let Some(report) = build_from_user_files("Ken's Labyrinth 2.01", &recipe, &store, &[]) else {
        return;
    };
    assert_eq!(report.reference, "unemu/media/ibm-pc/kenslab:2.01-r1");
    let validator = image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats);
    let image = store.resolve(&report.reference, &validator).unwrap();
    let config = image_store::media_set::Config::decode(&image.config).unwrap();
    // The disk the FAT builder's golden pins from the author's LABFULL.ZIP
    // (crates/pc-harness/tests/user_media_volumes.rs).
    assert_eq!(
        config.media[0].blob.hex(),
        "16992a33eef5a71080dc6f0df526a2d723a4acb2f1dfca6b7d7032a7dbb80062"
    );
    let metadata = Metadata::decode(&image.metadata, &image.manifest.layers).unwrap();
    assert_eq!(metadata.titles[0].id, "kenslab");
    assert_eq!(metadata.titles[0].title, "Ken's Labyrinth");
    // A second build from the same files is the same artifact.
    let again = Store::open(t.path("again")).unwrap();
    let second = build_from_user_files("Ken's Labyrinth 2.01", &recipe, &again, &[]).unwrap();
    assert_eq!(second.published.descriptor, report.published.descriptor);
}

#[test]
#[ignore = "user-supplied files: Azuramoon"]
fn user_rom_azuramoon_builds_its_ten_disks_from_the_users_folder() {
    let t = Temp::new("azuramoon");
    let store = Store::open(t.path("store")).unwrap();
    let recipe = corpus("azuramon/azuramoon-2026.media.toml");
    let Some(report) = build_from_user_files("Azuramoon", &recipe, &store, &[]) else {
        return;
    };
    assert_eq!(report.reference, "private/media/ibm-pc/azuramoon:2026-r1");
    let validator = image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats);
    let image = store.resolve(&report.reference, &validator).unwrap();
    let config = image_store::media_set::Config::decode(&image.config).unwrap();
    assert_eq!(config.media.len(), 10);
    let again = Store::open(t.path("again")).unwrap();
    let second = build_from_user_files("Azuramoon", &recipe, &again, &[]).unwrap();
    assert_eq!(second.published.descriptor, report.published.descriptor);
    println!(
        "azuramoon: {}",
        config
            .media
            .iter()
            .map(|m| m.blob.hex().to_owned())
            .collect::<Vec<_>>()
            .join(" ")
    );
}
