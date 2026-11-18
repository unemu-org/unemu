// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pinned recipes and their locks through the `ibm-pc` provider
//! (recipes.md §3.1, §3.8, §6.16, §7.2, §9).
//!
//! - A recipe pinned by one build of the program builds the same manifest
//!   digest with another build whose manifest writer is changed. The other
//!   build is simulated by a publisher that writes the manifest its own way
//!   (annotations in another order, one more annotation, other whitespace):
//!   its drafts get another digest, its builds of the pinned recipe the
//!   pinned one, byte for byte, because a pinned build publishes the lock's
//!   bytes and never calls a writer.
//! - A changed work file is reported as drift by the checks, by the build
//!   and by `show`, and does not change what a pinned build publishes.
//! - A lock mismatch tags nothing: a payload a step now makes differently
//!   (with `--keep-as`, kept only under the tag asked for), a field the
//!   recipe changed since the pin, a lock file that was edited, a digest
//!   the lock does not hold.
//! - The conformance harness builds the corpus's pinned recipes from the
//!   open fixtures and compares the digests (skipped without the fixtures
//!   unless `UNEMUPC_REQUIRE_FIXTURES=1`).

use image_store::{media_set, BlobSource as _, Descriptor, Digest, Manifest, Store};
use pc_image::recipe::PcRecipes;
use pc_image::recipe_build::PcPublisher;
use recipe::build::{build_path, Outcome, Published, Publisher, Request, Staged};
use recipe::library::Library;
use recipe::pin::{pin_path, Conformed, Pinned};
use recipe::provider::{FirmwareProfile, MediaProfile, Provider};
use recipe::store_index::StoreIndex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "recipe-lock-{name}-{}-{}",
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

/// The `ibm-pc` provider of another build of the program: everything as
/// [`PcRecipes`] has it but the manifest writer of its publisher.
struct OtherBuild;

impl Provider for OtherBuild {
    fn platform(&self) -> &str {
        PcRecipes.platform()
    }
    fn media(&self) -> &dyn MediaProfile {
        PcRecipes.media()
    }
    fn firmware(&self) -> &dyn FirmwareProfile {
        PcRecipes.firmware()
    }
    fn needs(&self) -> Option<&dyn image_store::needs::Vocabulary> {
        PcRecipes.needs()
    }
    fn software_platforms(&self) -> &[&str] {
        PcRecipes.software_platforms()
    }
    fn work_id_catalogues(&self) -> &[&str] {
        PcRecipes.work_id_catalogues()
    }
    fn publisher(&self) -> Option<&dyn Publisher> {
        Some(&OtherWriter)
    }
}

/// A publisher whose manifest writer differs from this build's: it puts an
/// annotation of its own first in the manifest's annotations, out of the
/// sorted order this build writes, and breaks the lines. The importers
/// make the payloads, config and metadata, in a scratch store; this writer
/// then writes the manifest its own way and tags it.
struct OtherWriter;

/// The manifest bytes the other build writes for this build's `bytes`: the
/// same decoded manifest plus one annotation, spelled differently.
fn other_manifest(bytes: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).unwrap();
    // The manifest's own annotations are its last field.
    let at = text.rfind("\"annotations\":{").unwrap() + "\"annotations\":{".len();
    let text = format!(
        "{}\"org.example.writer\":\"another release\",{}",
        &text[..at],
        &text[at..]
    );
    text.replace(",\"layers\":", ",\n  \"layers\": ")
        .into_bytes()
}

impl Publisher for OtherWriter {
    fn publish(&self, store: &Store, artifact: &Staged) -> Result<Published, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let scratch_root = store.root().join(format!(
            "cache/other-writer-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let scratch = Store::open(&scratch_root).map_err(|e| e.to_string())?;
        let made = PcPublisher.publish(&scratch, artifact)?;
        let bytes = scratch
            .read_verified(&made.descriptor, image_store::MAX_SMALL_BLOB)
            .map_err(|e| e.to_string())?;
        let manifest = Manifest::decode_for_summary(&bytes).map_err(|e| e.to_string())?;
        for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
            let mut blob = Vec::new();
            std::io::Read::read_to_end(
                &mut scratch
                    .open_blob(&descriptor.digest, descriptor.size)
                    .map_err(|e| e.to_string())?,
                &mut blob,
            )
            .map_err(|e| e.to_string())?;
            store
                .import(blob.as_slice(), Some(&descriptor.digest))
                .map_err(|e| e.to_string())?;
        }
        let written = other_manifest(&bytes);
        let blob = store
            .import(written.as_slice(), None)
            .map_err(|e| e.to_string())?;
        let descriptor = Descriptor::new(image_store::media_type::MANIFEST, blob.digest, blob.size);
        let _ = fs::remove_dir_all(&scratch_root);
        recipe::lock::tag(
            store,
            &artifact.reference,
            &descriptor,
            &media_set::Validator(&pc_image::media_set::PcMediaFormats),
        )
    }

    fn tag_locked(
        &self,
        store: &Store,
        reference: &str,
        manifest: &Descriptor,
    ) -> Result<Published, String> {
        PcPublisher.tag_locked(store, reference, manifest)
    }
}

const THIS: [&dyn Provider; 1] = [&PcRecipes];
const OTHER: [&dyn Provider; 1] = [&OtherBuild];

fn request<'a>(
    recipe: &'a Path,
    providers: &'a [&'a dyn Provider],
    store: &'a Store,
    index: &'a StoreIndex,
    libraries: &'a mut [Library],
    keep_as: Option<&'a str>,
) -> Request<'a> {
    Request {
        path: recipe,
        providers: recipe::Providers(providers),
        store,
        index: Some(index),
        store_read: None,
        libraries,
        fetcher: None,
        keep_as,
    }
}

fn build(
    recipe: &Path,
    providers: &[&dyn Provider],
    store: &Store,
    lib: &Path,
    keep_as: Option<&str>,
) -> Outcome {
    let index = StoreIndex::load(store).unwrap();
    let mut libraries = vec![Library::scan(lib, None).unwrap()];
    build_path(
        request(recipe, providers, store, &index, &mut libraries, keep_as),
        &mut |line| panic!("nothing to log: {line}"),
    )
    .unwrap()
}

fn built(outcome: Outcome) -> recipe::build::Report {
    match outcome {
        Outcome::Built(report) => *report,
        other => panic!("not built: {other:#?}"),
    }
}

/// Pin `recipe` into `store` with this build; the pinned digest.
fn pin(recipe: &Path, store: &Store, lib: &Path) -> Digest {
    let index = StoreIndex::load(store).unwrap();
    let mut libraries = vec![Library::scan(lib, None).unwrap()];
    match pin_path(
        request(recipe, &THIS, store, &index, &mut libraries, None),
        &mut |line| panic!("nothing to log: {line}"),
    )
    .unwrap()
    {
        Pinned::Pinned { digest, files, .. } => {
            // The manifest, the config and the title metadata.
            assert_eq!(files, 3);
            digest.parse().unwrap()
        }
        other => panic!("not pinned: {other:#?}"),
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

fn blobs(store: &Store) -> usize {
    fs::read_dir(store.root().join("images/blobs/sha256")).map_or(0, |entries| entries.count())
}

fn sha(bytes: &[u8]) -> String {
    Digest::of(bytes).hex().to_owned()
}

/// A stored zip archive of `members` (PKWARE APPNOTE §4.3.7 and §4.3.12),
/// every member dated 1993-03-27 13:58:22.
fn zip(members: &[(&str, &[u8])]) -> Vec<u8> {
    let (time, date): (u16, u16) = ((13 << 11) | (58 << 5) | 11, (13 << 9) | (3 << 5) | 27);
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
    for (name, bytes) in members {
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

const WORK: &str = "recipe = \"0.1\"\n\n[work]\nid = \"synthetic\"\n\n[title]\ntitle = \"Synthetic\"\npublishers = [\"Example Software\"]\nlabels = [\"game\"]\nsynopsis = \"A synthetic game.\"\n";

/// A draft media recipe in `t/recipes` of three steps (members unpacked
/// from an archive, built into a 1.44M disk, and a text with DOS line
/// ends as an extra) whose title comes partly from its work file; its
/// inputs in `t/lib`.
fn synthetic(t: &Temp) -> (PathBuf, PathBuf) {
    let lib = t.path("lib");
    let folder = t.path("recipes");
    fs::create_dir_all(&lib).unwrap();
    fs::create_dir_all(&folder).unwrap();
    let archive = zip(&[("A.EXE", b"first member"), ("DATA/B.DAT", b"second member")]);
    let notes = b"line one\nline two\n";
    fs::write(lib.join("GAME.ZIP"), &archive).unwrap();
    fs::write(lib.join("notes.txt"), notes).unwrap();
    let text = format!(
        "recipe = \"0.1\"\nwork = \"synthetic\"\n\n[media]\nname = \"private/media/ibm-pc/synthetic\"\nplatform = \"ibm-pc\"\nrelease = \"1.0\"\nrevision = 1\n\n\
         [title]\nyear = 1993\ndescription = \"The first release, on one disk.\"\nlabels = [\"freeware\"]\n\n\
         [input.archive]\nname = \"GAME.ZIP\"\nsize = {}\nsha256 = \"{}\"\n\n\
         [input.notes]\nname = \"notes.txt\"\nsize = {}\nsha256 = \"{}\"\n\n\
         [[step]]\nid = \"files\"\nunpack = \"archive\"\nformat = \"zip\"\nmembers = [\n\
           {{ path = \"A.EXE\", time = 1993-03-27T13:58:22 }},\n\
           {{ path = \"DATA/B.DAT\", time = 1993-03-27T13:58:22 }},\n]\n\n\
         [[step]]\nid = \"disk\"\nfat = \"files\"\nformat = \"1.44M\"\nlabel = \"SYNTHETIC\"\n\n\
         [[step]]\nid = \"readme\"\ntext = \"notes\"\neol = \"crlf\"\n\n\
         [[medium]]\nid = \"game-disk\"\nlabel = \"The game\"\nformat = \"1.44M\"\nfrom = \"disk\"\n\n\
         [[extra]]\nid = \"readme\"\nfile_name = \"README.TXT\"\nkind = \"document\"\ndos_name = \"README.TXT\"\nfrom = \"readme\"\n",
        archive.len(),
        sha(&archive),
        notes.len(),
        sha(notes),
    );
    let recipe = folder.join("synthetic.media.toml");
    fs::write(&recipe, text).unwrap();
    fs::write(folder.join("synthetic.work.toml"), WORK).unwrap();
    (recipe, lib)
}

/// A copy of the folder of `recipe` without its lock and with the digest
/// line removed: the same recipe as a draft.
fn as_draft(t: &Temp, recipe: &Path, name: &str) -> PathBuf {
    let folder = t.path(name);
    fs::create_dir_all(&folder).unwrap();
    let text = fs::read_to_string(recipe).unwrap();
    let draft: String = text
        .lines()
        .filter(|l| !l.starts_with("digest = "))
        .map(|l| format!("{l}\n"))
        .collect();
    let copy = folder.join(recipe.file_name().unwrap());
    fs::write(&copy, draft).unwrap();
    let work = recipe.with_file_name("synthetic.work.toml");
    fs::copy(&work, folder.join("synthetic.work.toml")).unwrap();
    copy
}

fn lock_file(recipe: &Path, digest: &Digest) -> Vec<u8> {
    fs::read(recipe::lock::dir(recipe).join("sha256").join(digest.hex())).unwrap()
}

#[test]
fn a_pinned_recipe_builds_its_digest_with_another_builds_manifest_writer() {
    let t = Temp::new("writer");
    let (recipe, lib) = synthetic(&t);
    let draft = as_draft(&t, &recipe, "draft");
    let pinned = pin(&recipe, &Store::open(t.path("author")).unwrap(), &lib);
    let text = fs::read_to_string(&recipe).unwrap();
    assert!(
        text.contains(&format!("revision = 1\ndigest = \"{pinned}\"\n")),
        "{text}"
    );

    // The other build's writer is a real change: its draft of the same
    // recipe has another manifest digest, while the config and the
    // metadata it holds are this build's, blob for blob.
    let other = Store::open(t.path("other-draft")).unwrap();
    let report = built(build(&draft, &OTHER, &other, &lib, None));
    assert!(!report.locked);
    let drafted = report.published.descriptor.digest.clone();
    assert_ne!(drafted, pinned);
    let locked_manifest = Manifest::decode_for_summary(&lock_file(&recipe, &pinned)).unwrap();
    let drafted_manifest = Manifest::decode_for_summary(
        &other
            .read_verified(&report.published.descriptor, image_store::MAX_SMALL_BLOB)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(drafted_manifest.config, locked_manifest.config);
    assert_eq!(drafted_manifest.layers, locked_manifest.layers);
    assert_eq!(
        drafted_manifest
            .annotations
            .get("org.example.writer")
            .map(String::as_str),
        Some("another release")
    );

    // The pinned recipe, built by the other build: the pinned digest, and
    // the manifest the store holds is the lock's, byte for byte.
    let store = Store::open(t.path("other-pinned")).unwrap();
    let report = built(build(&recipe, &OTHER, &store, &lib, None));
    assert!(report.locked);
    assert!(report.drift.is_empty());
    assert_eq!(report.published.descriptor.digest, pinned);
    assert_eq!(report.reference, "private/media/ibm-pc/synthetic:1.0-r1");
    assert_eq!(
        store
            .read_verified(&report.published.descriptor, image_store::MAX_SMALL_BLOB)
            .unwrap(),
        lock_file(&recipe, &pinned)
    );
    // The image resolves: every payload the steps made is in the store.
    let validator = media_set::Validator(&pc_image::media_set::PcMediaFormats);
    store.resolve(&report.reference, &validator).unwrap();
    assert!(report.render().ends_with(&format!(
        "built private/media/ibm-pc/synthetic:1.0-r1\t{pinned}\tlocked\n"
    )));
    // Built again: present, not written again.
    let again = built(build(&recipe, &OTHER, &store, &lib, None));
    assert!(!again.published.created);

    // A pin is made once per revision.
    let index = StoreIndex::load(&store).unwrap();
    let mut libraries = vec![Library::scan(&lib, None).unwrap()];
    match pin_path(
        request(&recipe, &THIS, &store, &index, &mut libraries, None),
        &mut |_| {},
    )
    .unwrap()
    {
        Pinned::Already(digest) => assert_eq!(digest, pinned.to_string()),
        other => panic!("{other:#?}"),
    }
}

#[test]
fn a_changed_work_file_is_reported_and_does_not_change_the_result() {
    let t = Temp::new("work");
    let (recipe, lib) = synthetic(&t);
    let pinned = pin(&recipe, &Store::open(t.path("author")).unwrap(), &lib);
    let work = recipe.with_file_name("synthetic.work.toml");
    fs::write(
        &work,
        WORK.replace("A synthetic game.", "A synthetic game, described again.")
            .replace("Example Software", "Example Publishing")
            .replace("[\"game\"]", "[\"game\", \"puzzle\"]"),
    )
    .unwrap();

    // The checks pass and list the drift, field by field.
    let report =
        recipe::check_paths(std::slice::from_ref(&recipe), recipe::Providers(&THIS)).unwrap();
    assert!(report.is_clean(), "{}", report.render());
    let drift: Vec<String> = report.files[0]
        .drift
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        drift,
        [
            "title.synopsis: the lock has \"A synthetic game.\", synthetic.work.toml has \"A synthetic game, described again.\"",
            "title.publishers: the lock has [\"Example Software\"], synthetic.work.toml has [\"Example Publishing\"]",
            "title.labels: the lock has [\"game\", \"freeware\"], synthetic.work.toml has [\"game\", \"puzzle\", \"freeware\"]",
        ]
    );
    assert!(report.render().contains(
        "  drift: title.synopsis: the lock has \"A synthetic game.\", synthetic.work.toml has \"A synthetic game, described again.\"; a build writes the lock's value until a new revision is pinned\n"
    ));

    // The build reports the same drift and publishes the pinned digest:
    // the metadata in the store is the lock's.
    let store = Store::open(t.path("store")).unwrap();
    let report = built(build(&recipe, &THIS, &store, &lib, None));
    assert_eq!(report.published.descriptor.digest, pinned);
    assert_eq!(
        report
            .drift
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        drift
    );
    let validator = media_set::Validator(&pc_image::media_set::PcMediaFormats);
    let image = store.resolve(&report.reference, &validator).unwrap();
    let metadata =
        image_store::metadata::Metadata::decode(&image.metadata, &image.manifest.layers).unwrap();
    assert_eq!(
        metadata.titles[0].synopsis.as_deref(),
        Some("A synthetic game.")
    );
    assert_eq!(metadata.titles[0].publishers, ["Example Software"]);

    // `show` prints the lock's record, not today's merge.
    match recipe::show_path(&recipe, recipe::Providers(&THIS)).unwrap() {
        recipe::Showing::Locked {
            record,
            drift: shown,
        } => {
            assert_eq!(record.synopsis.as_deref(), Some("A synthetic game."));
            assert_eq!(record.title.from, recipe::merge::Origin::Lock);
            assert_eq!(shown.len(), 3);
        }
        other => panic!("{other:#?}"),
    }

    // The same recipe as a draft takes the work file as it is today, so
    // the work file's change does reach a draft: the lock is what keeps
    // the pinned result.
    let draft = as_draft(&t, &recipe, "draft");
    let drafted = built(build(
        &draft,
        &THIS,
        &Store::open(t.path("draft-store")).unwrap(),
        &lib,
        None,
    ));
    assert_ne!(drafted.published.descriptor.digest, pinned);
}

#[test]
fn a_lock_mismatch_tags_nothing() {
    let t = Temp::new("mismatch");
    let (recipe, lib) = synthetic(&t);
    let pinned = pin(&recipe, &Store::open(t.path("author")).unwrap(), &lib);
    let original = fs::read_to_string(&recipe).unwrap();

    // A step that makes other bytes: another volume label. The checks pass
    // (the label is the step's, not a field of the artifact), the payload
    // check names the medium and both hashes, and nothing is written.
    fs::write(
        &recipe,
        original.replace("label = \"SYNTHETIC\"", "label = \"CHANGED\""),
    )
    .unwrap();
    let store = Store::open(t.path("payload")).unwrap();
    match build(&recipe, &THIS, &store, &lib, None) {
        Outcome::Mismatch {
            mismatches, kept, ..
        } => {
            assert_eq!(mismatches.len(), 1, "{mismatches:?}");
            assert!(
                mismatches[0]
                    .starts_with("medium game-disk (game-disk.img): the lock expects sha256:"),
                "{}",
                mismatches[0]
            );
            assert!(mismatches[0].contains("the steps made sha256:"));
            assert!(kept.is_none());
        }
        other => panic!("{other:#?}"),
    }
    assert!(tags(&store).is_empty());
    assert_eq!(blobs(&store), 0);
    // With --keep-as the differing result is kept under that tag only.
    match build(&recipe, &THIS, &store, &lib, Some("mine-r1")) {
        Outcome::Mismatch {
            kept: Some((reference, published)),
            ..
        } => {
            assert_eq!(reference, "private/media/ibm-pc/synthetic:mine-r1");
            assert_ne!(published.descriptor.digest, pinned);
        }
        other => panic!("{other:#?}"),
    }
    assert!(!tags(&store)
        .iter()
        .any(|t| t.ends_with(":1.0-r1") || t.ends_with(":1.0")));
    assert!(tags(&store).contains(&"private/media/ibm-pc/synthetic:mine-r1".to_owned()));

    // A field the recipe changed since the pin: a finding of rule 8 at the
    // digest, and the build refuses before reading any input.
    fs::write(
        &recipe,
        original.replace("label = \"The game\"", "label = \"Another label\""),
    )
    .unwrap();
    let report =
        recipe::check_paths(std::slice::from_ref(&recipe), recipe::Providers(&THIS)).unwrap();
    let findings: Vec<String> = report.findings.iter().map(|f| f.message.clone()).collect();
    assert_eq!(
        findings,
        ["the lock disagrees with the recipe: medium[1].label: the lock has \"The game\", the recipe has \"Another label\""]
    );
    assert_eq!(report.findings[0].rule, recipe::Rule::Lock);
    assert_eq!(report.findings[0].path, "media.digest");
    let store = Store::open(t.path("field")).unwrap();
    assert!(matches!(
        build(&recipe, &THIS, &store, &lib, None),
        Outcome::Refused(_)
    ));
    assert!(tags(&store).is_empty());

    // A field of the title record the recipe states.
    fs::write(&recipe, original.replace("year = 1993", "year = 1994")).unwrap();
    let report =
        recipe::check_paths(std::slice::from_ref(&recipe), recipe::Providers(&THIS)).unwrap();
    assert_eq!(
        report.findings[0].message,
        "the lock disagrees with the recipe: title.year: the lock has 1993, the recipe has 1994"
    );

    // A lock file edited after the pin.
    fs::write(&recipe, &original).unwrap();
    let manifest = Manifest::decode_for_summary(&lock_file(&recipe, &pinned)).unwrap();
    let config = recipe::lock::dir(&recipe)
        .join("sha256")
        .join(manifest.config.digest.hex());
    let bytes = fs::read(&config).unwrap();
    fs::write(&config, [&bytes[..], b"\n"].concat()).unwrap();
    let report =
        recipe::check_paths(std::slice::from_ref(&recipe), recipe::Providers(&THIS)).unwrap();
    assert!(
        report.findings[0]
            .message
            .ends_with("not the ones its name says"),
        "{}",
        report.render()
    );
    let store = Store::open(t.path("tampered")).unwrap();
    assert!(matches!(
        build(&recipe, &THIS, &store, &lib, None),
        Outcome::Refused(_)
    ));
    assert!(tags(&store).is_empty());
    fs::write(&config, &bytes).unwrap();

    // A digest the lock does not hold.
    let other = format!("sha256:{}", "1".repeat(64));
    fs::write(&recipe, original.replace(&pinned.to_string(), &other)).unwrap();
    let report =
        recipe::check_paths(std::slice::from_ref(&recipe), recipe::Providers(&THIS)).unwrap();
    assert!(
        report.findings[0]
            .message
            .contains("the lock does not hold the manifest the digest names"),
        "{}",
        report.render()
    );

    // Restored, the recipe builds its digest again.
    fs::write(&recipe, &original).unwrap();
    let store = Store::open(t.path("restored")).unwrap();
    assert_eq!(
        built(build(&recipe, &THIS, &store, &lib, None))
            .published
            .descriptor
            .digest,
        pinned
    );
}

fn corpus(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../backlog/docs/design/recipe-corpus")
        .join(relative)
}

fn vendor() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor")
}

/// The conformance harness on the corpus: the pinned open recipes, built
/// from the fixtures alone into a scratch store, give the digests they pin.
#[test]
fn the_pinned_corpus_builds_the_digests_it_pins() {
    let present = pc_image::template::media_files()
        .into_iter()
        .all(|file| fixtures::fixture(file.path).is_some())
        && fixtures::fixture("freedos/floppy-1.4-licenses.txt").is_some()
        && pc_image::template::FIRMWARE.iter().all(|source| {
            [source.rom, source.source_archive]
                .into_iter()
                .all(|relative| fixtures::fixture(relative).is_some())
        });
    if !present {
        return;
    }
    let t = Temp::new("conformance");
    let scratch = Store::open(t.path("scratch")).unwrap();
    let mut libraries = vec![
        Library::scan(&vendor().join("freedos"), None).unwrap(),
        Library::scan(&vendor(), None).unwrap(),
    ];
    let results = recipe::pin::conformance(
        &[
            corpus("freedos-1.4-floppy"),
            corpus("unemu-at-bios"),
            corpus("vgabios"),
            corpus("xtide-universal-bios"),
        ],
        recipe::Providers(&THIS),
        &scratch,
        &mut libraries,
    )
    .unwrap();
    let conforming: Vec<&str> = results
        .iter()
        .filter_map(|r| match &r.result {
            Conformed::Conforms { reference, .. } => Some(reference.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        conforming,
        [
            "unemu/media/ibm-pc/freedos:1.4-1.2m-r1",
            "unemu/media/ibm-pc/freedos:1.4-720k-r1",
            "unemu/media/ibm-pc/freedos:1.4-r1",
            "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1",
            "unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1",
            "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1",
        ],
        "{results:#?}"
    );
    let (text, all) = recipe::pin::render(&results);
    assert!(all, "{text}");
    assert!(
        text.ends_with("6 pinned recipes: 6 conform, 0 differ, 0 not built; 0 drafts skipped\n")
    );
}

/// The digest the pinned `recipe` states, as hex.
fn pinned_hex(recipe: &Path) -> String {
    fs::read_to_string(recipe)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("digest = \"sha256:"))
        .unwrap()
        .trim_end_matches('"')
        .to_owned()
}

/// Replace the lock file `old` of `recipe` with `bytes`, named by their
/// hash as a careful forger would; the new name.
fn swap_lock_file(recipe: &Path, old: &str, bytes: &[u8]) -> String {
    let folder = recipe::lock::dir(recipe).join("sha256");
    fs::remove_file(folder.join(old)).unwrap();
    let new = sha(bytes);
    fs::write(folder.join(&new), bytes).unwrap();
    new
}

/// Edit the locked manifest of `recipe`, re-hash it and point the recipe's
/// digest at the result: a lock that passes points 1 and 2.
fn tamper_manifest(recipe: &Path, edit: impl Fn(&str) -> String) {
    let old = pinned_hex(recipe);
    let manifest = fs::read_to_string(recipe::lock::dir(recipe).join("sha256").join(&old)).unwrap();
    let edited = edit(&manifest);
    assert_ne!(edited, manifest, "the edit changed nothing");
    let new = swap_lock_file(recipe, &old, edited.as_bytes());
    let text = fs::read_to_string(recipe).unwrap();
    fs::write(recipe, text.replace(&old, &new)).unwrap();
}

/// The findings of the checks on `recipe`; each must be rule 8's.
fn lock_findings(recipe: &Path) -> Vec<String> {
    let report = recipe::check_paths(&[recipe.to_path_buf()], recipe::Providers(&THIS)).unwrap();
    for finding in &report.findings {
        assert_eq!(finding.rule, recipe::Rule::Lock, "{finding}");
    }
    report
        .findings
        .iter()
        .map(|f| {
            f.message
                .strip_prefix("the lock disagrees with the recipe: ")
                .unwrap_or(&f.message)
                .to_owned()
        })
        .collect()
}

/// Mark every user-supplied descriptor of a manifest open under MIT.
fn mark_open(manifest: &str) -> String {
    assert!(
        manifest
            .matches("\"unemu.redistribution\":\"user-supplied\"")
            .count()
            >= 3
    );
    manifest.replace(
        "\"unemu.redistribution\":\"user-supplied\"",
        "\"unemu.license\":\"MIT\",\"unemu.redistribution\":\"open\"",
    )
}

/// The reviewer's forgery on a set: a recipe with no `[license]`, its lock
/// edited to mark every layer open and re-hashed. Rule 8 refuses it, so the
/// build tags nothing and the push guard never sees the marks.
#[test]
fn a_lock_that_marks_a_user_supplied_set_open_is_refused() {
    let t = Temp::new("open-set");
    let (recipe, lib) = synthetic(&t);
    pin(&recipe, &Store::open(t.path("author")).unwrap(), &lib);
    assert!(lock_findings(&recipe).is_empty());
    tamper_manifest(&recipe, mark_open);
    let findings = lock_findings(&recipe);
    assert!(
        findings.contains(
            &"config.license: the lock has open under MIT, the recipe has user-supplied: the recipe states no [license]"
                .to_owned()
        ),
        "{findings:#?}"
    );
    assert!(
        findings.contains(
            &"layer[1] (game-disk.img).license: the lock has open under MIT, the recipe has user-supplied: the recipe states no [license]"
                .to_owned()
        ),
        "{findings:#?}"
    );
    let store = Store::open(t.path("store")).unwrap();
    assert!(matches!(
        build(&recipe, &THIS, &store, &lib, None),
        Outcome::Refused(_)
    ));
    assert!(tags(&store).is_empty());
}

/// A field the recipe or its work file no longer states, and a default the
/// recipe leaves to the importer: errors, not drift.
#[test]
fn a_field_removed_since_the_pin_is_an_error() {
    let t = Temp::new("removed");
    let (recipe, lib) = synthetic(&t);
    pin(&recipe, &Store::open(t.path("author")).unwrap(), &lib);
    let original = fs::read_to_string(&recipe).unwrap();
    fs::write(&recipe, original.replace("year = 1993\n", "")).unwrap();
    assert_eq!(
        lock_findings(&recipe),
        ["title.year: the lock has 1993, the recipe has nothing"]
    );
    fs::write(&recipe, original.replace("dos_name = \"README.TXT\"\n", "")).unwrap();
    assert_eq!(
        lock_findings(&recipe),
        ["extra[1].dos_name: the lock has \"README.TXT\", the recipe has nothing"]
    );
    fs::write(&recipe, &original).unwrap();
    // The work file no longer states the synopsis: nothing does, so the
    // lock's value is not drift.
    let work = recipe.with_file_name("synthetic.work.toml");
    fs::write(
        &work,
        WORK.replace("synopsis = \"A synthetic game.\"\n", ""),
    )
    .unwrap();
    assert_eq!(
        lock_findings(&recipe),
        ["title.synopsis: the lock has \"A synthetic game.\", the recipe has nothing"]
    );
}

/// Each media format of the provider is named back from the table a set's
/// config stores for it, and the generated text's licence is the
/// provider's.
#[test]
fn formats_are_compared_by_name_and_generated_text_has_the_providers_licence() {
    for format in PcRecipes.formats() {
        let table = pc_image::media_set::format(format).unwrap();
        assert_eq!(
            PcRecipes.format_name(&table).as_deref(),
            Some(*format),
            "{format}"
        );
    }
    assert_eq!(
        recipe::lock::GENERATED_LICENSE,
        pc_image::firmware::METADATA_LICENSE
    );
}

fn vgabios_present() -> bool {
    fixtures::fixture("vgabios/vgabios-std.bin").is_some()
        && fixtures::fixture("vgabios/vgabios-0.9d-unemu.1-src.tar").is_some()
}

/// The corpus's open VGA BIOS recipe as a draft in `t/recipes`.
fn vgabios_draft(t: &Temp) -> PathBuf {
    let folder = t.path("recipes");
    fs::create_dir_all(&folder).unwrap();
    let text: String = fs::read_to_string(corpus("vgabios/vgabios-0.9d-unemu.1-std.firmware.toml"))
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with("digest = "))
        .map(|l| format!("{l}\n"))
        .collect();
    let recipe = folder.join("vgabios.firmware.toml");
    fs::write(&recipe, text).unwrap();
    recipe
}

/// The reviewer's forgery on a firmware package: a user's own copy of a
/// catalogue ROM, without `[license]`, its lock marked open.
#[test]
fn a_lock_that_marks_a_user_supplied_package_open_is_refused() {
    if !vgabios_present() {
        return;
    }
    let t = Temp::new("open-package");
    let folder = t.path("recipes");
    fs::create_dir_all(&folder).unwrap();
    let bytes = fixtures::fixture("vgabios/vgabios-std.bin").unwrap();
    let recipe = folder.join("mine.firmware.toml");
    fs::write(
        &recipe,
        format!(
            "recipe = \"0.1\"\n\n[firmware]\nname = \"private/firmware/ibm-pc/vgabios-0.9d-unemu.1-std\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n\
             [package]\ntitle = \"A user's VGA BIOS\"\n\n\
             [input.rom]\nname = \"vgabios-std.bin\"\nsize = {}\nsha256 = \"{}\"\n\n\
             [[rom]]\nid = \"video-bios\"\nrole = \"video-bios\"\ncatalogue_id = \"vgabios-0.9d-unemu.1-std\"\nfrom = \"rom\"\n",
            bytes.len(),
            sha(&bytes)
        ),
    )
    .unwrap();
    pin(
        &recipe,
        &Store::open(t.path("author")).unwrap(),
        &vendor().join("vgabios"),
    );
    assert!(lock_findings(&recipe).is_empty());
    tamper_manifest(&recipe, mark_open);
    let findings = lock_findings(&recipe);
    assert!(
        findings.contains(
            &"layer[1] (vgabios-std.bin).license: the lock has open under MIT, the recipe has user-supplied: the recipe states no [license]"
                .to_owned()
        ),
        "{findings:#?}"
    );
    // The README's text states the marks: the forged ones are not the ones
    // it was generated from.
    assert!(
        findings.iter().any(|f| f.starts_with("README: ")),
        "{findings:#?}"
    );
    let store = Store::open(t.path("store")).unwrap();
    let lib = vendor().join("vgabios");
    assert!(matches!(
        build(&recipe, &THIS, &store, &lib, None),
        Outcome::Refused(_)
    ));
    assert!(tags(&store).is_empty());
}

/// An open package's lock may not be less restrictive than its recipe: a
/// ROM relicensed, a README edited, a stated field removed.
#[test]
fn an_open_packages_lock_is_held_to_its_recipe() {
    if !vgabios_present() {
        return;
    }
    let t = Temp::new("open-vgabios");
    let recipe = vgabios_draft(&t);
    let lib = vendor().join("vgabios");
    pin(&recipe, &Store::open(t.path("author")).unwrap(), &lib);
    assert!(lock_findings(&recipe).is_empty());
    let lock = recipe::lock::dir(&recipe);
    let saved_lock: Vec<(PathBuf, Vec<u8>)> = fs::read_dir(lock.join("sha256"))
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let saved_recipe = fs::read_to_string(&recipe).unwrap();
    let restore = || {
        let _ = fs::remove_dir_all(lock.join("sha256"));
        fs::create_dir_all(lock.join("sha256")).unwrap();
        for (path, bytes) in &saved_lock {
            fs::write(path, bytes).unwrap();
        }
        fs::write(&recipe, &saved_recipe).unwrap();
    };

    // The ROM and its source archive relicensed under a permissive licence.
    tamper_manifest(&recipe, |m| {
        m.replace(
            "\"unemu.license\":\"LGPL-2.1-or-later\"",
            "\"unemu.license\":\"MIT\"",
        )
    });
    let findings = lock_findings(&recipe);
    assert!(
        findings.contains(
            &"layer[1] (vgabios-std.bin).license: the lock has open under MIT, the recipe has open under LGPL-2.1-or-later at most"
                .to_owned()
        ),
        "{findings:#?}"
    );
    restore();

    // The ROM's source link pointed elsewhere.
    tamper_manifest(&recipe, |m| {
        m.replacen(
            "\"unemu.source\":\"sha256:0f0890b6",
            "\"unemu.source\":\"sha256:1f0890b6",
            1,
        )
    });
    assert!(
        lock_findings(&recipe).iter().any(|f| f.starts_with(
            "layer[1] (vgabios-std.bin).source: the lock has a link to sha256:1f0890b6"
        )),
        "{:#?}",
        lock_findings(&recipe)
    );
    restore();

    // A README edited, the config and the manifest re-hashed to name it.
    let manifest_hex = pinned_hex(&recipe);
    let manifest_text = fs::read_to_string(lock.join("sha256").join(&manifest_hex)).unwrap();
    let manifest = Manifest::decode_for_summary(manifest_text.as_bytes()).unwrap();
    let config_hex = manifest.config.digest.hex().to_owned();
    let config_text = fs::read_to_string(lock.join("sha256").join(&config_hex)).unwrap();
    let config = image_store::firmware::Config::decode(config_text.as_bytes()).unwrap();
    let readme = config.readme.clone().unwrap();
    let readme_hex = readme.blob.hex().to_owned();
    let mut readme_text = fs::read(lock.join("sha256").join(&readme_hex)).unwrap();
    readme_text.extend_from_slice(b"\nAlso redistributable under MIT.\n");
    let new_readme = swap_lock_file(&recipe, &readme_hex, &readme_text);
    let new_config_text = config_text.replace(&readme_hex, &new_readme).replace(
        &format!("size = {}\n", readme.size),
        &format!("size = {}\n", readme_text.len()),
    );
    let new_config = swap_lock_file(&recipe, &config_hex, new_config_text.as_bytes());
    tamper_manifest(&recipe, |m| {
        m.replace(
            &format!(
                "\"digest\":\"sha256:{readme_hex}\",\"size\":{}",
                readme.size
            ),
            &format!(
                "\"digest\":\"sha256:{new_readme}\",\"size\":{}",
                readme_text.len()
            ),
        )
        .replace(
            &format!(
                "\"digest\":\"sha256:{config_hex}\",\"size\":{}",
                manifest.config.size
            ),
            &format!(
                "\"digest\":\"sha256:{new_config}\",\"size\":{}",
                new_config_text.len()
            ),
        )
    });
    let findings = lock_findings(&recipe);
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert!(
        findings[0].starts_with("README: the lock has a text other than the generated one"),
        "{findings:#?}"
    );
    restore();

    // A stated field removed from the recipe: the lock's value is an error.
    fs::write(
        &recipe,
        saved_recipe.replace(
            "vendor = \"unemu (fork of the LGPL VGABIOS project)\"\n",
            "",
        ),
    )
    .unwrap();
    assert_eq!(
        lock_findings(&recipe),
        ["package.vendor: the lock has \"unemu (fork of the LGPL VGABIOS project)\", the recipe has nothing"]
    );
    restore();
    assert!(lock_findings(&recipe).is_empty());
}

/// The re-reviewer's forgery on an open package: the stored bytes of the
/// ROM and its source archive copied into the lock, both relabelled with
/// the generated text's licence `CC0-1.0`, the manifest's licence list
/// dropped, everything re-hashed. A lock holds no payload byte, so the
/// lock is refused before its marks are read, and nothing is tagged.
#[test]
fn a_lock_that_holds_a_payload_is_refused() {
    if !vgabios_present() {
        return;
    }
    let t = Temp::new("payload-in-lock");
    let recipe = vgabios_draft(&t);
    let lib = vendor().join("vgabios");
    let author = Store::open(t.path("author")).unwrap();
    pin(&recipe, &author, &lib);
    let lock = recipe::lock::dir(&recipe).join("sha256");
    let manifest =
        Manifest::decode_for_summary(&fs::read(lock.join(pinned_hex(&recipe))).unwrap()).unwrap();
    for layer in &manifest.layers {
        if layer.media_type == image_store::media_type::MARKDOWN {
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut author.open_blob(&layer.digest, layer.size).unwrap(),
            &mut bytes,
        )
        .unwrap();
        fs::write(lock.join(layer.digest.hex()), bytes).unwrap();
    }
    tamper_manifest(&recipe, |m| {
        m.replace(
            "\"unemu.license\":\"LGPL-2.1-or-later\"",
            "\"unemu.license\":\"CC0-1.0\"",
        )
        .replace(
            "\"org.opencontainers.image.licenses\":\"LGPL-2.1-or-later AND CC0-1.0\",",
            "",
        )
    });
    let findings = lock_findings(&recipe);
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert!(
        findings[0].contains("which is a payload layer of the locked manifest; a lock holds the config, the metadata and a package's README, and no payload byte"),
        "{findings:#?}"
    );
    let store = Store::open(t.path("store")).unwrap();
    assert!(matches!(
        build(&recipe, &THIS, &store, &lib, None),
        Outcome::Refused(_)
    ));
    assert!(tags(&store).is_empty());
}

/// What the importer copies from the catalogue, a recipe never states: a
/// lock that changes it is refused, so no text of its own reaches the
/// generated config or README.
#[test]
fn a_lock_that_changes_the_catalogues_provenance_is_refused() {
    if !vgabios_present() {
        return;
    }
    let t = Temp::new("provenance");
    let recipe = vgabios_draft(&t);
    pin(
        &recipe,
        &Store::open(t.path("author")).unwrap(),
        &vendor().join("vgabios"),
    );
    let lock = recipe::lock::dir(&recipe).join("sha256");
    let manifest_hex = pinned_hex(&recipe);
    let manifest =
        Manifest::decode_for_summary(&fs::read(lock.join(&manifest_hex)).unwrap()).unwrap();
    let config_hex = manifest.config.digest.hex().to_owned();
    let config_text = fs::read_to_string(lock.join(&config_hex)).unwrap();
    let at = config_text.find("[provenance]\nfrom = \"").unwrap() + "[provenance]\nfrom = \"".len();
    let edited = format!(
        "{}Also free to use under MIT. {}",
        &config_text[..at],
        &config_text[at..]
    );
    let new_config = swap_lock_file(&recipe, &config_hex, edited.as_bytes());
    tamper_manifest(&recipe, |m| {
        m.replace(
            &format!(
                "\"digest\":\"sha256:{config_hex}\",\"size\":{}",
                manifest.config.size
            ),
            &format!(
                "\"digest\":\"sha256:{new_config}\",\"size\":{}",
                edited.len()
            ),
        )
    });
    let findings = lock_findings(&recipe);
    assert!(
        findings.iter().any(|f| f.starts_with(
            "package.provenance: the lock has \"from \\\"Also free to use under MIT."
        )),
        "{findings:#?}"
    );
}
