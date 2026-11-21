// SPDX-License-Identifier: AGPL-3.0-or-later
//! Recipes as registry artifacts (recipes.md §7): a pinned
//! recipe pushed to the in-process mock registry and pulled into an empty
//! folder plans and builds with no other file to its pinned digest; the
//! recipe guard's refusals; the registry row judged from manifest and
//! config alone; a recipe a newer release wrote refused before any blob of
//! its bundle is fetched; and `unemu.build.recipe` on the tags a recipe
//! build writes. No test touches the network. Synthetic files only.

use image_registry::credentials::Anonymous;
use image_registry::recipe::{self as remote, RowState};
use image_registry::test_support::{MockRegistry, Policy};
use image_registry::{Client, Error, Reference};
use image_store::compat::{AppVersion, Requirement, MIN_APP_VERSION};
use image_store::{Digest, Manifest, Store};
use recipe::artifact::{self, Artifact, Judged, Refusal};
use recipe::compat::{Table, TABLE};
use recipe::library::Library;
use recipe::provider::Provider;
use recipe::Providers;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const PROVIDERS: [&dyn Provider; 1] = [&pc_image::recipe::PcRecipes];

fn providers() -> Providers<'static> {
    Providers(&PROVIDERS)
}

fn upcli(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

fn os(words: &[&dyn AsRef<std::ffi::OsStr>]) -> Vec<OsString> {
    words.iter().map(|w| w.as_ref().to_owned()).collect()
}

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("recipe-registry-{name}-{}", std::process::id()));
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

fn sha(bytes: &[u8]) -> String {
    Digest::of(bytes).hex().to_owned()
}

const WORK: &str =
    "recipe = \"1.0\"\n\n[work]\nid = \"shared\"\n\n[title]\ntitle = \"Shared\"\nsynopsis = \"A work.\"\n";

/// Notes of 300 bytes: a bundled text over the disclosable limit.
fn notes() -> Vec<u8> {
    let mut text = "These notes travel with the recipe.\n"
        .repeat(9)
        .into_bytes();
    text.truncate(300);
    text
}

fn data(seed: usize) -> Vec<u8> {
    (0..737_280).map(|i| ((i + seed) % 251) as u8).collect()
}

/// The preferences: the mock registry, plain HTTP on loopback, no
/// libraries.
fn preferences(t: &Temp, registry: &MockRegistry) -> PathBuf {
    let path = t.path("preferences.toml");
    fs::write(
        &path,
        format!(
            "[[registries]]\nhost = \"{}\"\nprivate = false\nplain_http = true\n",
            registry.host()
        ),
    )
    .unwrap();
    path
}

/// A media recipe in `t/<folder>` with a work file, a bundled text input
/// `NOTES.TXT` made into an extra by a `text` step, and a 720K image from
/// `t/lib`, pinned into `t/<folder>-store`. `extra` is added to the input
/// table of the notes. Returns the recipe file and the pinned digest.
fn pinned(
    t: &Temp,
    registry: &MockRegistry,
    folder: &str,
    notes: &[u8],
    extra: &str,
    seed: usize,
) -> (PathBuf, String) {
    let lib = t.path("lib");
    let dir = t.path(folder);
    fs::create_dir_all(&lib).unwrap();
    fs::create_dir_all(&dir).unwrap();
    let data = data(seed);
    fs::write(lib.join(format!("DATA{seed}.BIN")), &data).unwrap();
    fs::write(dir.join("NOTES.TXT"), notes).unwrap();
    fs::write(dir.join("shared.work.toml"), WORK).unwrap();
    let text = format!(
        "recipe = \"1.0\"\nwork = \"shared\"\n\n[media]\nname = \"private/media/ibm-pc/shared\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n\
         [title]\nyear = 1990\n\n\
         [input.notes]\nname = \"NOTES.TXT\"\nsize = {}\nsha256 = \"{}\"\nbundle = true\n{extra}\n\
         [input.data]\nname = \"DATA{seed}.BIN\"\nsize = {}\nsha256 = \"{}\"\n\n\
         [[step]]\nid = \"readme\"\ntext = \"notes\"\neol = \"crlf\"\n\n\
         [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"720K\"\nfrom = \"data\"\n\n\
         [[extra]]\nid = \"readme\"\nfile_name = \"README.TXT\"\nkind = \"document\"\nfrom = \"readme\"\n",
        notes.len(),
        sha(notes),
        data.len(),
        sha(&data),
    );
    pin_text(t, registry, folder, &text)
}

/// Write `text` as `t/<folder>/shared.media.toml` and pin it into
/// `t/<folder>-store` with the library `t/lib`.
fn pin_text(t: &Temp, registry: &MockRegistry, folder: &str, text: &str) -> (PathBuf, String) {
    let lib = t.path("lib");
    let dir = t.path(folder);
    fs::create_dir_all(&dir).unwrap();
    let recipe = dir.join("shared.media.toml");
    fs::write(&recipe, text).unwrap();
    let (code, output, errors) = upcli(&os(&[
        &"recipe",
        &"pin",
        &recipe,
        &"--config",
        &preferences(t, registry),
        &"--store",
        &t.path(&format!("{folder}-store")),
        &"--library",
        &lib,
    ]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let digest = fs::read_to_string(&recipe)
        .unwrap()
        .lines()
        .find_map(|l| {
            l.strip_prefix("digest = \"")?
                .strip_suffix('"')
                .map(str::to_owned)
        })
        .unwrap();
    (recipe, digest)
}

fn target(registry: &MockRegistry, path: &str) -> Reference {
    Reference::parse(&format!("{}/{path}", registry.host()), "unused.example").unwrap()
}

fn open_policy() -> Policy {
    Policy {
        anonymous_read: vec!["*".into()],
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    }
}

const PLACE: &str = "unemu/recipes/media/ibm-pc/shared:r1";

/// Push `artifact` to `place` on the mock registry, staging in `t/stage`.
fn push(
    t: &Temp,
    registry: &MockRegistry,
    artifact: &Artifact,
    place: &str,
) -> image_registry::Result<image_registry::push::PushReport> {
    let store = Store::open(t.path("stage")).unwrap();
    let entry = registry.entry(false);
    let client = Client::new(&entry, Box::new(Anonymous)).unwrap();
    remote::push(
        &store,
        artifact,
        &target(registry, place),
        Some(&entry),
        providers(),
        &client,
        &image_registry::push::PushOptions::default(),
        &mut |_, _| {},
    )
}

fn files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.strip_prefix(dir).unwrap().display().to_string());
            }
        }
    }
    out.sort();
    out
}

/// The flow of recipes §7 through `unemu`: push, pull into an empty folder,
/// plan and build there into an empty store with the library alone; the
/// build gives the pinned digest and its tag records the recipe it was
/// built from, the artifact's config digest.
#[test]
fn a_pushed_recipe_pulled_into_an_empty_folder_plans_and_builds_to_its_pin() {
    let t = Temp::new("flow");
    let registry = MockRegistry::start(open_policy());
    let (recipe, digest) = pinned(&t, &registry, "author", &notes(), "", 0);
    let prefs = preferences(&t, &registry);
    let place = format!("{}/{PLACE}", registry.host());

    // The author's pin records the pinned text on the tag it built.
    let author = Store::open(t.path("author-store")).unwrap();
    let tags = author.list_tags(None, 10).unwrap().tags;
    let built = tags
        .iter()
        .find(|t| t.name == "private/media/ibm-pc/shared:r1")
        .unwrap();
    assert_eq!(
        built.built_from_recipe(),
        Some(Digest::of(&fs::read(&recipe).unwrap()))
    );

    let (code, output, errors) = upcli(&os(&[
        &"recipe",
        &"push",
        &recipe,
        &place,
        &"--config",
        &prefs,
        &"--dry-run",
        &"--store",
        &t.path("stage"),
    ]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.contains(&format!(
            "output:private/media/ibm-pc/shared:r1\t{digest}\n"
        )),
        "{output}"
    );
    assert!(
        output.contains("\tshared.media.toml.lock/sha256/"),
        "{output}"
    );
    assert!(output.ends_with("guard:allowed\n"), "{output}");
    assert!(
        registry.log().is_empty(),
        "a dry run sends nothing: {:?}",
        registry.log()
    );

    let (code, output, errors) = upcli(&os(&[
        &"recipe",
        &"push",
        &recipe,
        &place,
        &"--config",
        &prefs,
        &"--store",
        &t.path("stage"),
    ]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output
            .lines()
            .last()
            .unwrap()
            .starts_with(&format!("pushed\t{place}\tsha256:")),
        "{output}"
    );
    // The same artifact again changes nothing.
    let (code, output, _) = upcli(&os(&[
        &"recipe",
        &"push",
        &recipe,
        &place,
        &"--config",
        &prefs,
        &"--store",
        &t.path("stage"),
    ]));
    assert_eq!(code, 0);
    assert!(
        output.starts_with(&format!("unchanged\t{place}\t")),
        "{output}"
    );

    let pulled = t.path("pulled");
    let (code, output, errors) = upcli(&os(&[
        &"recipe",
        &"pull",
        &place,
        &"--to",
        &pulled,
        &"--config",
        &prefs,
    ]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.ends_with(&format!(
            "\t{}\n",
            pulled.join("shared.media.toml").display()
        )),
        "{output}"
    );
    let laid_out = files(&pulled);
    assert_eq!(laid_out.len(), 6, "{laid_out:?}");
    assert_eq!(
        laid_out,
        files(&t.path("author")),
        "the folder the recipe was pinned in"
    );
    for file in &laid_out {
        assert_eq!(
            fs::read(pulled.join(file)).unwrap(),
            fs::read(t.path("author").join(file)).unwrap(),
            "{file}"
        );
    }

    // An empty store and the library: the bundled notes are found beside
    // the pulled recipe, the image in the library.
    let store = t.path("empty");
    Store::open(&store).unwrap();
    let pulled_recipe = pulled.join("shared.media.toml");
    let (code, output, errors) = upcli(&os(&[
        &"recipe",
        &"plan",
        &pulled_recipe,
        &"--config",
        &prefs,
        &"--store",
        &store,
        &"--library",
        &t.path("lib"),
    ]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.contains(&format!(
            "beside the recipe: {}",
            pulled.join("NOTES.TXT").display()
        )),
        "{output}"
    );
    let (code, output, errors) = upcli(&os(&[
        &"recipe",
        &"build",
        &pulled_recipe,
        &"--config",
        &prefs,
        &"--store",
        &store,
        &"--library",
        &t.path("lib"),
    ]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.ends_with(&format!(
            "built private/media/ibm-pc/shared:r1\t{digest}\tlocked\n"
        )),
        "{output}"
    );
    let store = Store::open_existing(&store).unwrap();
    let tag = store
        .list_tags(None, 10)
        .unwrap()
        .tags
        .into_iter()
        .find(|t| t.name == "private/media/ibm-pc/shared:r1")
        .unwrap();
    assert_eq!(tag.manifest.digest.to_string(), digest);
    let config = Digest::of(&fs::read(&pulled_recipe).unwrap());
    assert_eq!(tag.built_from_recipe(), Some(config.clone()));
    let artifact = artifact::bundle(&recipe, providers()).unwrap();
    assert_eq!(
        artifact.manifest.config.digest, config,
        "the recipe's digest is the config's"
    );

    // A second pull into the same folder keeps it; one into a folder that
    // holds another file of a part's name is refused and writes nothing.
    let (code, _, errors) = upcli(&os(&[
        &"recipe",
        &"pull",
        &place,
        &"--to",
        &pulled,
        &"--config",
        &prefs,
    ]));
    assert_eq!((code, errors.as_str()), (0, ""));
    let other = t.path("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("NOTES.TXT"), b"mine").unwrap();
    let (code, _, errors) = upcli(&os(&[
        &"recipe",
        &"pull",
        &place,
        &"--to",
        &other,
        &"--config",
        &prefs,
    ]));
    assert_eq!(code, 1);
    assert!(errors.contains("is there with other bytes"), "{errors}");
    assert_eq!(files(&other), ["NOTES.TXT"]);
}

/// The manifest's annotations and parts, in the order of recipes §7.1.
#[test]
fn the_artifact_holds_the_recipe_its_work_file_bundled_inputs_and_lock() {
    let t = Temp::new("layout");
    let registry = MockRegistry::start(open_policy());
    let (recipe, digest) = pinned(&t, &registry, "author", &notes(), "", 0);
    let artifact = artifact::bundle(&recipe, providers()).unwrap();
    let manifest = &artifact.manifest;
    assert_eq!(
        manifest.artifact_type(),
        image_store::media_type::RECIPE_MEDIA
    );
    assert_eq!(
        manifest.config.media_type,
        "application/vnd.unemu.recipe.v1+toml"
    );
    let annotations: Vec<(&str, &str)> = manifest
        .annotations
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        annotations,
        [
            ("org.opencontainers.image.description", "A work."),
            ("org.opencontainers.image.licenses", "CC0-1.0"),
            ("org.opencontainers.image.title", "Shared"),
            ("unemu.min-app-version", "0.0.3"),
            ("unemu.platform", "ibm-pc"),
            ("unemu.recipe.output", "private/media/ibm-pc/shared:r1"),
            ("unemu.recipe.output.digest", digest.as_str()),
        ]
    );
    let parts: Vec<(&str, &str)> = std::iter::once(&manifest.config)
        .chain(&manifest.layers)
        .map(|d| {
            (
                d.media_type.as_str(),
                d.annotations[artifact::TITLE].as_str(),
            )
        })
        .collect();
    let lock: Vec<String> = fs::read_dir(recipe::lock::dir(&recipe).join("sha256"))
        .unwrap()
        .map(|e| {
            format!(
                "shared.media.toml.lock/sha256/{}",
                e.unwrap().file_name().to_str().unwrap()
            )
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut expected = vec![
        ("application/vnd.unemu.recipe.v1+toml", "shared.media.toml"),
        (
            "application/vnd.unemu.recipe.work.v1+toml",
            "shared.work.toml",
        ),
        ("application/vnd.unemu.recipe.input.v1", "NOTES.TXT"),
    ];
    expected.extend(
        lock.iter()
            .map(|l| ("application/vnd.unemu.recipe.lock.v1", l.as_str())),
    );
    assert_eq!(parts, expected);
    // The bundle is deterministic: the same folder gives the same digest.
    assert_eq!(
        artifact::bundle(&recipe, providers()).unwrap().digest,
        artifact.digest
    );
    // A pinned recipe without its lock fails its checks (rule 8), and a
    // draft has no output digest to publish.
    fs::rename(recipe::lock::dir(&recipe), t.path("lock-aside")).unwrap();
    assert!(matches!(
        artifact::bundle(&recipe, providers()),
        Err(Refusal::Findings(report)) if report.contains("6.11.8")
    ));
    let text = fs::read_to_string(&recipe).unwrap();
    let draft: String = text
        .lines()
        .filter(|l| !l.starts_with("digest = "))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&recipe, draft).unwrap();
    assert_eq!(
        artifact::bundle(&recipe, providers()),
        Err(Refusal::Draft {
            file: recipe.display().to_string()
        })
    );
}

/// An artifact rebuilt with other layers, as a tampered or hand-made one.
fn with_layers(artifact: &Artifact, layers: Vec<(image_store::Descriptor, Vec<u8>)>) -> Artifact {
    let mut manifest = Manifest::new_recipe(
        artifact.manifest.artifact_type(),
        "ibm-pc",
        artifact.manifest.config.clone(),
        layers.iter().map(|(d, _)| d.clone()).collect(),
    )
    .unwrap();
    manifest.annotations = artifact.manifest.annotations.clone();
    Artifact::from_parts(
        manifest.encode().unwrap(),
        artifact.config.clone(),
        layers.into_iter().map(|(_, b)| b).collect(),
    )
    .unwrap()
}

fn part(kind: &str, title: &str, bytes: &[u8]) -> (image_store::Descriptor, Vec<u8>) {
    let mut descriptor = image_store::Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
    descriptor
        .annotations
        .insert(artifact::TITLE.into(), title.into());
    (descriptor, bytes.to_vec())
}

fn refusal(result: image_registry::Result<impl std::fmt::Debug>) -> Refusal {
    match result {
        Err(Error::Recipe(refusal)) => refusal,
        other => panic!("{other:?}"),
    }
}

/// Each refusal of the recipe guard: a part that is none of the recipe's,
/// a part it names that is missing, a bundled input that is not text, a
/// recipe without `[license]` naming a file under 256 bytes not marked
/// disclosable, another place than the output's; and the same recipe passes.
#[test]
fn the_recipe_guard_refuses_each_thing_it_must() {
    let t = Temp::new("guard");
    let registry = MockRegistry::start(open_policy());
    let (recipe, _) = pinned(&t, &registry, "author", &notes(), "", 0);
    let artifact = artifact::bundle(&recipe, providers()).unwrap();
    let layers: Vec<_> = artifact
        .parts()
        .map(|(d, b)| (d.clone(), b.to_vec()))
        .collect();

    let mut extra = layers.clone();
    extra.insert(
        2,
        part(
            image_store::media_type::RECIPE_INPUT,
            "EXTRA.TXT",
            b"not in the recipe",
        ),
    );
    let refused = refusal(push(&t, &registry, &with_layers(&artifact, extra), PLACE));
    assert_eq!(
        refused,
        Refusal::ExtraPart {
            title: "EXTRA.TXT".into(),
            media_type: image_store::media_type::RECIPE_INPUT.into()
        }
    );
    // A bundled input whose bytes are not the ones the recipe names.
    let mut other = layers.clone();
    other[1] = part(
        image_store::media_type::RECIPE_INPUT,
        "NOTES.TXT",
        b"other notes",
    );
    assert!(matches!(
        refusal(push(&t, &registry, &with_layers(&artifact, other), PLACE)),
        Refusal::ExtraPart { .. }
    ));
    let mut missing = layers.clone();
    missing.remove(0);
    assert_eq!(
        refusal(push(&t, &registry, &with_layers(&artifact, missing), PLACE)),
        Refusal::MissingPart {
            title: "shared.work.toml".into()
        }
    );
    // A lock file the lock does not need fails rule 8 when laid out.
    let mut stray = layers.clone();
    let bytes = b"a stray file";
    stray.push(part(
        image_store::media_type::RECIPE_LOCK,
        &format!("shared.media.toml.lock/sha256/{}", sha(bytes)),
        bytes,
    ));
    let found = refusal(push(&t, &registry, &with_layers(&artifact, stray), PLACE));
    assert!(
        matches!(&found, Refusal::Findings(r) if r.contains("6.11.8")),
        "{found}"
    );
    assert_eq!(
        refusal(push(
            &t,
            &registry,
            &artifact,
            "unemu/recipes/media/ibm-pc/other:r1"
        )),
        Refusal::Name {
            target: "unemu/recipes/media/ibm-pc/other:r1".into(),
            wanted: "unemu/recipes/media/ibm-pc/shared:r1".into()
        }
    );
    assert!(
        registry.log().is_empty(),
        "nothing reached the registry: {:?}",
        registry.log()
    );

    // Not text: a NUL in the bundled input.
    let mut binary = notes();
    binary[10] = 0;
    let (binary, _) = pinned(&t, &registry, "binary", &binary, "", 1);
    assert_eq!(
        refusal(push(
            &t,
            &registry,
            &artifact::bundle(&binary, providers()).unwrap(),
            PLACE
        )),
        Refusal::NotText {
            title: "NOTES.TXT".into()
        }
    );
    // A tiny file in a recipe without [license], then the same marked.
    let (tiny, _) = pinned(&t, &registry, "tiny", b"serial 1234\n", "", 2);
    assert_eq!(
        refusal(push(
            &t,
            &registry,
            &artifact::bundle(&tiny, providers()).unwrap(),
            PLACE
        )),
        Refusal::Disclosable {
            input: "input.notes".into(),
            size: 12
        }
    );
    let (marked, _) = pinned(
        &t,
        &registry,
        "marked",
        b"serial 1234\n",
        "disclosable = true\n",
        3,
    );
    assert!(push(
        &t,
        &registry,
        &artifact::bundle(&marked, providers()).unwrap(),
        PLACE
    )
    .is_ok());
    assert!(push(
        &t,
        &registry,
        &artifact,
        "unemu/recipes/media/ibm-pc/shared:r2"
    )
    .is_err());
}

/// Recipes §7.3: a recipe tag moves only to a recipe that pins the same
/// output digest, without --move; another output digest is refused even
/// with --move, and the registry keeps the old recipe.
#[test]
fn a_recipe_tag_is_replaced_only_by_a_recipe_of_the_same_output_digest() {
    let t = Temp::new("replace");
    let registry = MockRegistry::start(open_policy());
    let (recipe, digest) = pinned(&t, &registry, "author", &notes(), "", 0);
    let first = artifact::bundle(&recipe, providers()).unwrap();
    push(&t, &registry, &first, PLACE).unwrap();

    // A better recipe of the same output: a URL added to the data input.
    let text = fs::read_to_string(&recipe).unwrap().replace(
        "[input.data]\n",
        "[input.data]\nurls = [\"https://example.org/DATA0.BIN\"]\n",
    );
    fs::write(&recipe, text).unwrap();
    let better = artifact::bundle(&recipe, providers()).unwrap();
    assert_ne!(better.digest, first.digest);
    assert_eq!(better.output_digest(), Some(digest.as_str()));
    let report = push(&t, &registry, &better, PLACE).unwrap();
    assert_eq!(
        report.tag,
        image_registry::push::TagOutcome::Moved {
            from: first.digest.clone()
        }
    );
    let repository = "unemu/recipes/media/ibm-pc/shared";
    assert_eq!(
        registry.tag_digest(repository, "r1"),
        Some(better.digest.clone())
    );

    // Another output under the same name and tag.
    let (other, other_digest) = pinned(&t, &registry, "other", &notes(), "", 5);
    assert_ne!(other_digest, digest);
    let other = artifact::bundle(&other, providers()).unwrap();
    let store = Store::open(t.path("stage")).unwrap();
    let entry = registry.entry(false);
    let client = Client::new(&entry, Box::new(Anonymous)).unwrap();
    let refused = remote::push(
        &store,
        &other,
        &target(&registry, PLACE),
        Some(&entry),
        providers(),
        &client,
        &image_registry::push::PushOptions {
            allow_move: true,
            ..Default::default()
        },
        &mut |_, _| {},
    );
    match refused {
        Err(Error::RecipeTagTaken {
            current,
            output,
            wanted,
            ..
        }) => {
            assert_eq!(current, better.digest);
            assert_eq!(output.as_deref(), Some(digest.as_str()));
            assert_eq!(wanted, other_digest);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(registry.tag_digest(repository, "r1"), Some(better.digest));
}

fn requests(registry: &MockRegistry) -> Vec<String> {
    registry.log()
}

/// A registry row is judged from the manifest and the config alone: two
/// requests, no part of the bundle; the bundled input counts as found in
/// the artifact. `unemu recipe plan REF` prints that plan.
#[test]
fn a_registry_row_is_judged_from_manifest_and_config_alone() {
    let t = Temp::new("row");
    let registry = MockRegistry::start(open_policy());
    let (recipe, _) = pinned(&t, &registry, "author", &notes(), "", 0);
    let artifact = artifact::bundle(&recipe, providers()).unwrap();
    push(&t, &registry, &artifact, PLACE).unwrap();
    registry.clear_log();

    let client = Client::new(&registry.entry(false), Box::new(Anonymous)).unwrap();
    let mut libraries = vec![Library::scan(&t.path("lib"), None).unwrap()];
    let row = remote::row(
        &client,
        &target(&registry, PLACE),
        providers(),
        recipe::plan::Sources {
            store: None,
            downloads: None,
            libraries: &mut libraries,
        },
    )
    .unwrap();
    assert_eq!(
        requests(&registry),
        [
            "GET /v2/unemu/recipes/media/ibm-pc/shared/manifests/r1".to_owned(),
            format!(
                "GET /v2/unemu/recipes/media/ibm-pc/shared/blobs/{}",
                artifact.manifest.config.digest
            ),
        ]
    );
    assert_eq!(
        row.output.as_deref(),
        Some("private/media/ibm-pc/shared:r1")
    );
    let RowState::Judged(Judged::Plan(plan)) = row.state else {
        panic!("{:?}", row.state)
    };
    assert!(plan.complete(), "{}", plan.render());
    assert!(
        plan.render().contains("beside the recipe: NOTES.TXT"),
        "{}",
        plan.render()
    );

    // Without the library the row says what is missing.
    registry.clear_log();
    let prefs = preferences(&t, &registry);
    let place = format!("{}/{PLACE}", registry.host());
    let (code, output, errors) = upcli(&os(&[&"recipe", &"plan", &place, &"--config", &prefs]));
    assert_eq!((code, errors.as_str()), (1, ""), "{output}");
    assert!(output.contains("DATA0.BIN"), "{output}");
    assert_eq!(requests(&registry).len(), 2, "{:?}", requests(&registry));
    let (code, output, errors) = upcli(&os(&[
        &"recipe",
        &"plan",
        &place,
        &"--config",
        &prefs,
        &"--library",
        &t.path("lib"),
    ]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
}

/// Recipes a newer release wrote, rated by its rows: a newer recipe
/// version, a newer step kind, and an output in a newer media format (the
/// output's own marker). Each is refused with the release it needs, by
/// pull and by a row, before any blob of the bundle is fetched.
#[test]
fn a_newer_recipe_is_refused_before_any_blob_of_its_bundle_is_fetched() {
    let t = Temp::new("newer");
    let registry = MockRegistry::start(open_policy());
    let (recipe, _) = pinned(&t, &registry, "author", &notes(), "", 0);
    let artifact = artifact::bundle(&recipe, providers()).unwrap();
    push(&t, &registry, &artifact, PLACE).unwrap();
    let inspected = artifact::inspect(&artifact, providers(), &t.path("inspect")).unwrap();
    let newer = AppVersion::new(0, 0, 4);
    assert!(
        newer > image_store::compat::level(),
        "a release this one is not"
    );
    let next = AppVersion::new(0, 0, 3);
    let versions: &[image_store::compat::Row] = &[("1.0", newer)];
    let steps: &[image_store::compat::Row] = &[("unpack", next), ("fat", next), ("text", newer)];
    let cases = [
        ("version", Table { versions, ..TABLE }, Requirement::None),
        ("step", Table { steps, ..TABLE }, Requirement::None),
        ("format", TABLE, Requirement::Release(newer)),
    ];
    let client = Client::new(&registry.entry(false), Box::new(Anonymous)).unwrap();
    let repository = "unemu/recipes/media/ibm-pc/shared";
    for (kind, table, output) in cases {
        let required = recipe::compat::required(&inspected.recipe, &output, &table).unwrap();
        assert_eq!(required, newer, "{kind}");
        let mut manifest = artifact.manifest.clone();
        manifest
            .annotations
            .insert(MIN_APP_VERSION.into(), required.to_string());
        let tag = format!("r1-{kind}");
        registry.seed_manifest(repository, &tag, &manifest.encode().unwrap());
        registry.clear_log();
        let source = target(&registry, &format!("{repository}:{tag}"));
        let refused = remote::pull(&client, &source, &t.path(kind), providers());
        match refused {
            Err(error @ Error::Store(image_store::Error::NeedsNewer { .. })) => assert!(
                error.to_string().contains("needs unemupc 0.0.4 or newer"),
                "{error}"
            ),
            other => panic!("{kind}: {other:?}"),
        }
        let row = remote::row(
            &client,
            &source,
            providers(),
            recipe::plan::Sources {
                store: None,
                downloads: None,
                libraries: &mut [],
            },
        )
        .unwrap();
        assert_eq!(
            row.state,
            RowState::NeedsNewer("Needs unemupc 0.0.4 or newer".into()),
            "{kind}"
        );
        assert_eq!(
            requests(&registry),
            [
                format!("GET /v2/{repository}/manifests/{tag}"),
                format!("GET /v2/{repository}/manifests/{tag}"),
            ],
            "{kind}: the manifest only"
        );
        assert!(!t.path(kind).exists(), "{kind}: nothing written");
    }
}

/// CRC-32 (IEEE 802.3), as a zip directory states it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A zip archive of one stored member, 1993-03-27 13:58:22.
fn zip(name: &str, data: &[u8]) -> Vec<u8> {
    let (time, date): (u16, u16) = ((13 << 11) | (58 << 5) | 11, (13 << 9) | (3 << 5) | 27);
    let crc = crc32(data);
    let mut out = Vec::new();
    let header = |out: &mut Vec<u8>, central: bool| {
        out.extend_from_slice(if central {
            b"PK\x01\x02"
        } else {
            b"PK\x03\x04"
        });
        if central {
            out.extend_from_slice(&20u16.to_le_bytes());
        }
        for value in [20u16, 0, 0, time, date] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        if central {
            for value in [0u16, 0, 0] {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out.extend_from_slice(name.as_bytes());
    };
    header(&mut out, false);
    out.extend_from_slice(data);
    let directory = out.len();
    header(&mut out, true);
    let size = out.len() - directory;
    out.extend_from_slice(b"PK\x05\x06");
    for value in [0u16, 0, 1, 1] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(directory as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A recipe whose `unpack` step lists a member of 9 bytes, the serial
/// number of recipes §6.13, made into an extra; `marked` says
/// `disclosable = true` on the member, `licensed` adds an open statement
/// with its terms text.
fn member_recipe(
    t: &Temp,
    registry: &MockRegistry,
    folder: &str,
    marked: bool,
    licensed: bool,
) -> PathBuf {
    let serial = b"SN 12345\n";
    let archive = zip("SERIAL.TXT", serial);
    let data = data(7);
    let terms = "You may share this game unchanged.\n".repeat(10);
    let lib = t.path("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("GAME.ZIP"), &archive).unwrap();
    fs::write(lib.join("DATA7.BIN"), &data).unwrap();
    fs::write(lib.join("TERMS.TXT"), &terms).unwrap();
    let (license, terms_input, terms_extra) = if licensed {
        (
            "[license]\nexpression = \"LicenseRef-Shareware\"\nterms = \"terms\"\n\n".to_owned(),
            format!(
                "[input.terms]\nname = \"TERMS.TXT\"\nsize = {}\nsha256 = \"{}\"\n\n",
                terms.len(),
                sha(terms.as_bytes())
            ),
            "\n[[extra]]\nid = \"terms\"\nfile_name = \"TERMS.TXT\"\nkind = \"document\"\nfrom = \"terms\"\n",
        )
    } else {
        (String::new(), String::new(), "")
    };
    let text = format!(
        "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/shared\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n\
         [title]\ntitle = \"Shared\"\n\n{license}\
         [input.game]\nname = \"GAME.ZIP\"\nsize = {}\nsha256 = \"{}\"\ndisclosable = true\n\n\
         [input.data]\nname = \"DATA7.BIN\"\nsize = {}\nsha256 = \"{}\"\n\n{terms_input}\
         [[step]]\nid = \"files\"\nunpack = \"game\"\nformat = \"zip\"\ncomplete = true\nmembers = [\n  {{ path = \"SERIAL.TXT\", size = 9, sha256 = \"{}\"{} }},\n]\n\n\
         [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"720K\"\nfrom = \"data\"\n\n\
         [[extra]]\nid = \"serial\"\nfile_name = \"SERIAL.TXT\"\nkind = \"document\"\nfrom = \"files/SERIAL.TXT\"\n{terms_extra}",
        archive.len(),
        sha(&archive),
        data.len(),
        sha(&data),
        sha(serial),
        if marked { ", disclosable = true" } else { "" },
    );
    pin_text(t, registry, folder, &text).0
}

/// Recipes §6.13 for members (the maintainer, 2026-11-19): a member of 9
/// bytes in a recipe without `[license]` is refused unless it says
/// `disclosable = true`; a recipe with `[license]` may publish it. The key
/// on a member of 256 bytes or more is a finding, as on an input.
#[test]
fn a_tiny_member_is_published_only_when_marked_or_licensed() {
    let t = Temp::new("member");
    let registry = MockRegistry::start(open_policy());
    let unmarked = member_recipe(&t, &registry, "unmarked", false, false);
    assert_eq!(
        refusal(push(
            &t,
            &registry,
            &artifact::bundle(&unmarked, providers()).unwrap(),
            PLACE
        )),
        Refusal::Disclosable {
            input: "step files member SERIAL.TXT".into(),
            size: 9
        }
    );
    let marked = member_recipe(&t, &registry, "marked", true, false);
    push(
        &t,
        &registry,
        &artifact::bundle(&marked, providers()).unwrap(),
        PLACE,
    )
    .unwrap();
    let licensed = member_recipe(&t, &registry, "licensed", false, true);
    let artifact = artifact::bundle(&licensed, providers()).unwrap();
    // Another output than the marked one: pushed into another namespace.
    push(
        &t,
        &registry,
        &artifact,
        "other/recipes/media/ibm-pc/shared:r1",
    )
    .unwrap();
    let big = fs::read_to_string(&marked)
        .unwrap()
        .replace("size = 9, ", "size = 300, ");
    fs::write(&marked, big).unwrap();
    let report = recipe::check_paths(&[marked], providers()).unwrap();
    assert!(
        report.findings.iter().any(|f| f
            .message
            .contains("disclosable is for a file under 256 bytes; this one is 300")),
        "{}",
        report.render()
    );
}

/// Bundled parts never share bytes: two bundled inputs with the same bytes
/// or name, or one with the bytes of the work file, are findings of the
/// checks, not a failed push.
#[test]
fn identical_bundled_parts_are_a_finding_of_the_checks() {
    let t = Temp::new("identical");
    let dir = t.path("r");
    fs::create_dir_all(&dir).unwrap();
    let notes = notes();
    fs::write(dir.join("NOTES.TXT"), &notes).unwrap();
    fs::write(dir.join("COPY.TXT"), &notes).unwrap();
    fs::write(dir.join("shared.work.toml"), WORK).unwrap();
    let data = data(0);
    let recipe = |inputs: &str| {
        format!(
            "recipe = \"1.0\"\nwork = \"shared\"\n\n[media]\nname = \"private/media/ibm-pc/shared\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n\
             {inputs}\
             [input.data]\nname = \"DATA0.BIN\"\nsize = {}\nsha256 = \"{}\"\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"720K\"\nfrom = \"data\"\n\n\
             [[extra]]\nid = \"a\"\nfile_name = \"A.TXT\"\nkind = \"document\"\nfrom = \"a\"\n\n\
             [[extra]]\nid = \"b\"\nfile_name = \"B.TXT\"\nkind = \"document\"\nfrom = \"b\"\n",
            data.len(),
            sha(&data)
        )
    };
    let input = |id: &str, name: &str, bytes: &[u8]| {
        format!(
            "[input.{id}]\nname = \"{name}\"\nsize = {}\nsha256 = \"{}\"\nbundle = true\n\n",
            bytes.len(),
            sha(bytes)
        )
    };
    let findings = |text: String| -> Vec<String> {
        let path = dir.join("shared.media.toml");
        fs::write(&path, text).unwrap();
        recipe::check_paths(&[path], providers())
            .unwrap()
            .findings
            .iter()
            .map(|f| f.message.clone())
            .collect()
    };
    let same_bytes = findings(recipe(&format!(
        "{}{}",
        input("a", "NOTES.TXT", &notes),
        input("b", "COPY.TXT", &notes)
    )));
    assert!(
        same_bytes
            .iter()
            .any(|m| m.starts_with("input.a is bundled with the same bytes")),
        "{same_bytes:?}"
    );
    let other = b"other notes, long enough to be no secret at all".repeat(6);
    fs::write(dir.join("OTHER.TXT"), &other).unwrap();
    let same_name = findings(recipe(&format!(
        "{}{}",
        input("a", "NOTES.TXT", &notes),
        input("b", "NOTES.TXT", &other)
    )));
    assert!(
        same_name
            .iter()
            .any(|m| m.contains("is bundled as NOTES.TXT too")),
        "{same_name:?}"
    );
    let work = findings(recipe(&format!(
        "{}{}",
        input("a", "W.TXT", WORK.as_bytes()),
        input("b", "NOTES.TXT", &notes)
    )));
    assert!(
        work.iter()
            .any(|m| m.contains("has the bytes of the work file shared.work.toml")),
        "{work:?}"
    );
    let clean = findings(recipe(&format!(
        "{}{}",
        input("a", "NOTES.TXT", &notes),
        input("b", "OTHER.TXT", &other)
    )));
    assert!(clean.is_empty(), "{clean:?}");
}

/// An artifact with `edit` applied to its manifest annotations.
fn reannotated(
    artifact: &Artifact,
    edit: impl FnOnce(&mut std::collections::BTreeMap<String, String>),
) -> Artifact {
    let mut manifest = artifact.manifest.clone();
    edit(&mut manifest.annotations);
    Artifact::from_parts(
        manifest.encode().unwrap(),
        artifact.config.clone(),
        artifact.layers.clone(),
    )
    .unwrap()
}

/// The guard's remaining refusals: a marker missing or below the computed
/// one, an annotation that does not say what the recipe says, a bundled
/// file whose bytes are not the pinned ones (exit 1), and a tag that names
/// a manifest which is no recipe.
#[test]
fn the_guard_refuses_markers_annotations_and_a_tag_of_another_kind() {
    let t = Temp::new("guard-more");
    let registry = MockRegistry::start(open_policy());
    let (recipe, digest) = pinned(&t, &registry, "author", &notes(), "", 0);
    let artifact = artifact::bundle(&recipe, providers()).unwrap();
    let next = AppVersion::new(0, 0, 3);
    let missing = reannotated(&artifact, |a| {
        a.remove(MIN_APP_VERSION);
    });
    assert_eq!(
        refusal(push(&t, &registry, &missing, PLACE)),
        Refusal::MinAppVersion {
            declared: "none".into(),
            required: next
        }
    );
    let lower = reannotated(&artifact, |a| {
        a.insert(MIN_APP_VERSION.into(), "0.0.2".into());
    });
    assert_eq!(
        refusal(push(&t, &registry, &lower, PLACE)),
        Refusal::MinAppVersion {
            declared: "0.0.2".into(),
            required: next
        }
    );
    let output = reannotated(&artifact, |a| {
        a.insert(
            artifact::OUTPUT.into(),
            "private/media/ibm-pc/other:r1".into(),
        );
    });
    assert_eq!(
        refusal(push(&t, &registry, &output, PLACE)),
        Refusal::Annotation {
            key: artifact::OUTPUT,
            found: Some("private/media/ibm-pc/other:r1".into()),
            wanted: "private/media/ibm-pc/shared:r1".into()
        }
    );
    let pinned_digest = reannotated(&artifact, |a| {
        a.remove(artifact::OUTPUT_DIGEST);
    });
    assert_eq!(
        refusal(push(&t, &registry, &pinned_digest, PLACE)),
        Refusal::Annotation {
            key: artifact::OUTPUT_DIGEST,
            found: None,
            wanted: digest.clone()
        }
    );
    assert!(registry.log().is_empty(), "{:?}", registry.log());

    // A firmware package already under the recipe's tag.
    let config = b"schema = 1\n";
    let rom = b"rom";
    let package = Manifest::new_firmware(
        "ibm-pc",
        image_store::Descriptor::new(
            image_store::media_type::FIRMWARE_CONFIG,
            Digest::of(config),
            config.len() as u64,
        ),
        vec![image_store::Descriptor::new(
            image_store::media_type::FIRMWARE,
            Digest::of(rom),
            rom.len() as u64,
        )],
    )
    .unwrap()
    .encode()
    .unwrap();
    let current = registry.seed_manifest("unemu/recipes/media/ibm-pc/shared", "r1", &package);
    match push(&t, &registry, &artifact, PLACE) {
        Err(Error::RecipeTagTaken {
            current: named,
            output: None,
            wanted,
            ..
        }) => {
            assert_eq!(named, current);
            assert_eq!(wanted, digest);
        }
        other => panic!("{other:?}"),
    }

    // A bundled file changed since the pin: refused, exit 1.
    fs::write(recipe.with_file_name("NOTES.TXT"), b"other notes").unwrap();
    let prefs = preferences(&t, &registry);
    let place = format!("{}/{PLACE}", registry.host());
    let (code, _, errors) = upcli(&os(&[
        &"recipe",
        &"push",
        &recipe,
        &place,
        &"--config",
        &prefs,
        &"--store",
        &t.path("stage"),
        &"--dry-run",
    ]));
    assert_eq!(code, 1, "{errors}");
    assert!(
        errors.contains("the recipe names for its bundled input"),
        "{errors}"
    );
}

/// `Artifact::unpack` writes nothing through a symbolic link below the
/// folder, nothing over a file of another size, and when a write fails it
/// removes what it wrote, so the folder is as it was.
#[cfg(unix)]
#[test]
fn unpack_refuses_links_and_leaves_the_folder_as_it_was_on_failure() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new("unpack");
    let registry = MockRegistry::start(open_policy());
    let (recipe, _) = pinned(&t, &registry, "author", &notes(), "", 0);
    let artifact = artifact::bundle(&recipe, providers()).unwrap();

    let linked = t.path("linked");
    fs::create_dir_all(&linked).unwrap();
    fs::create_dir_all(t.path("elsewhere")).unwrap();
    std::os::unix::fs::symlink(t.path("elsewhere"), linked.join("shared.media.toml.lock")).unwrap();
    let error = artifact.unpack(&linked).unwrap_err();
    assert!(error.to_string().contains("is a symbolic link"), "{error}");
    assert_eq!(files(&linked), Vec::<String>::new());
    assert_eq!(files(&t.path("elsewhere")), Vec::<String>::new());

    let other = t.path("other-size");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("NOTES.TXT"), b"short").unwrap();
    assert_eq!(
        artifact.unpack(&other).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(files(&other), ["NOTES.TXT"]);

    // The lock folder cannot be written: the recipe, work file and notes
    // written before it are removed again.
    let locked = t.path("read-only");
    let sha256 = locked.join("shared.media.toml.lock/sha256");
    fs::create_dir_all(&sha256).unwrap();
    fs::set_permissions(&sha256, fs::Permissions::from_mode(0o500)).unwrap();
    assert!(artifact.unpack(&locked).is_err());
    fs::set_permissions(&sha256, fs::Permissions::from_mode(0o700)).unwrap();
    let left: Vec<_> = fs::read_dir(&locked)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["shared.media.toml.lock"]);
    assert_eq!(files(&locked), Vec::<String>::new());
}

/// `recipe plan` takes a missing path for a registry reference only when it
/// parses as one; any other missing path cannot be read (exit 2).
#[test]
fn a_missing_path_that_is_no_reference_is_a_file_that_cannot_be_read() {
    let t = Temp::new("plan-ref");
    for operand in [r"C:\dir\recipe", "./dir/x@y"] {
        let (code, output, errors) = upcli(&os(&[
            &"recipe",
            &"plan",
            &operand,
            &"--config",
            &t.path("preferences.toml"),
        ]));
        assert_eq!((code, output.as_str()), (2, ""), "{operand}: {errors}");
        assert!(errors.starts_with("unemu recipe plan: "), "{errors}");
        assert!(!errors.contains("registry"), "{operand}: {errors}");
    }
}
