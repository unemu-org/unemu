// SPDX-License-Identifier: AGPL-3.0-or-later
//! Recipe artifacts on a registry (recipes.md §7): push, pull
//! and the registry row of the Recipes group, over the same client and
//! transfer code as machine images, media sets and firmware packages.
//!
//! **Push** ([`push()`]): the target's registry rules of [`crate::guard`]
//! (a configured registry, a listed namespace) with every part counted as
//! open, as a recipe, its bundled texts and its lock are our own CC0 text;
//! then the recipe guard ([`recipe::artifact::guard`]), which inspects the
//! artifact laid out in a scratch folder; then the tag rule of recipes §7.3:
//! a recipe tag that names another manifest moves only to a recipe that
//! pins the same output digest (`unemu.recipe.output.digest`), without
//! `--move`, and is refused otherwise, whatever the options say. A published
//! 1.0 recipe keeps its grammar at that output tag; a newer version needs a new output tag. The parts
//! are staged in the store as untagged blobs and uploaded by the push code
//! of [`crate::push`], which publishes the manifest last after checking
//! that the tag did not change meanwhile.
//!
//! **Pull** ([`pull()`]): the manifest first, whose `unemu.min-app-version`
//! is checked before anything else is fetched, so a recipe a newer release
//! wrote is refused with the release it needs and no blob of its bundle is
//! requested; then the strict envelope, the config and every part, each at
//! most 1 MiB and verified, the codec of [`recipe::artifact`] and
//! [`recipe::artifact::inspect`] in a scratch folder; only then are the
//! parts written into the folder named, where the recipe plans and builds
//! with no other file.
//!
//! **A row** ([`row`]): the manifest, then the config, and nothing else;
//! the plan of [`recipe::artifact::judge`] says what the user's sources
//! hold. A recipe a newer release wrote is judged from its manifest alone.

use crate::{
    guard::{self, ClosureEntry, MarkSource, Redistribution},
    push::{self, BlobOutcome, PushOptions, PushPlan, PushProgress, PushReport, TagOutcome},
    Client, Error, Reference, RegistryEntry, Result,
};
use ::recipe::artifact::{self, Artifact, Judged};
use ::recipe::plan::Sources;
use ::recipe::Providers;
use image_store::{media_type, Descriptor, Digest, Manifest, Store};
use std::path::{Path, PathBuf};

/// The parts of `artifact` as a closure in which every blob is open: the
/// registry rules of [`guard::check`] then decide by the target alone.
fn closure(artifact: &Artifact) -> Vec<ClosureEntry> {
    std::iter::once(("config", &artifact.manifest.config))
        .chain(artifact.manifest.layers.iter().map(|d| ("layer", d)))
        .map(|(role, descriptor)| ClosureEntry {
            role,
            descriptor: descriptor.clone(),
            mark: Redistribution::Open,
            effective: Redistribution::Open,
            source: MarkSource::ThisImage,
        })
        .collect()
}

/// A private scratch folder for one inspection, removed with everything in
/// it when it is dropped: a new hidden folder under `base`, created only if
/// no such folder exists (never one another process made), readable by
/// this user only.
struct Scratch(PathBuf);

impl Scratch {
    fn new(base: &Path) -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::fs::create_dir_all(base).map_err(image_store::Error::from)?;
        loop {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let path = base.join(format!(
                ".recipe-scratch-{}-{}-{nanos:08x}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(image_store::Error::from(e).into()),
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The folder of a store's cache where a recipe push inspects its artifact.
pub const PUSH_SCRATCH: &str = "cache/recipe-push";

/// Check `artifact` for a push to `target` without the network: the
/// registry rules, then the recipe guard in a scratch folder of the
/// store's cache. [`push()`] runs the same checks again.
pub fn check(
    store: &Store,
    artifact: &Artifact,
    target: &Reference,
    entry: Option<&RegistryEntry>,
    providers: Providers<'_>,
) -> Result<()> {
    let tag = match (&target.tag, &target.digest) {
        (Some(tag), None) => tag,
        _ => {
            return Err(Error::Reference(format!(
                "{target}: a recipe is pushed to the tag of its output, not by digest"
            )))
        }
    };
    guard::check(entry, target, &closure(artifact))?;
    let scratch = Scratch::new(&store.root().join(PUSH_SCRATCH))?;
    artifact::guard(artifact, providers, &target.repository, tag, &scratch.0)?;
    Ok(())
}

/// Push `artifact` to `target` (recipes §7.3). The checks of [`check`]
/// run first; the parts are staged in `store` as untagged blobs. A tag
/// that names this artifact is [`TagOutcome::Unchanged`]; one that names a
/// recipe of the same output digest moves ([`TagOutcome::Moved`]), without
/// `options.allow_move`; any other is refused with
/// [`Error::RecipeTagTaken`].
#[allow(clippy::too_many_arguments)]
pub fn push(
    store: &Store,
    artifact: &Artifact,
    target: &Reference,
    entry: Option<&RegistryEntry>,
    providers: Providers<'_>,
    client: &Client,
    options: &PushOptions,
    progress: &mut dyn FnMut(&Descriptor, BlobOutcome),
) -> Result<PushReport> {
    if target.host != client.host() {
        return Err(Error::Config("client and target registry differ".into()));
    }
    if options.chunk_size < push::MIN_CHUNK_SIZE {
        return Err(Error::Config(format!(
            "chunk size must be at least {} bytes",
            push::MIN_CHUNK_SIZE
        )));
    }
    check(store, artifact, target, entry, providers)?;
    for (descriptor, bytes) in
        std::iter::once((&artifact.manifest.config, &artifact.config[..])).chain(artifact.parts())
    {
        store.import(bytes, Some(&descriptor.digest))?;
    }
    store.import(artifact.manifest_bytes.as_slice(), Some(&artifact.digest))?;
    let repository = target.repository.as_str();
    let reference = target.manifest_reference();
    let current = client.manifest(repository, &reference)?;
    let wanted = artifact.output_digest().unwrap_or_default().to_owned();
    let tag = match &current {
        Some(current) if current.digest == artifact.digest => {
            return Ok(PushReport {
                image: artifact.descriptor(),
                blobs: Vec::new(),
                tag: TagOutcome::Unchanged,
            })
        }
        Some(current) => {
            let output = Manifest::decode_for_summary(&current.bytes)
                .ok()
                .filter(|m| image_store::is_recipe(m.artifact_type()))
                .and_then(|m| m.annotations.get(artifact::OUTPUT_DIGEST).cloned());
            if output.as_deref() != Some(wanted.as_str()) {
                return Err(Error::RecipeTagTaken {
                    tag: reference,
                    current: current.digest.clone(),
                    output,
                    wanted,
                });
            }
            if recipe_version(&artifact.config).as_deref() != Some("1.0") {
                store
                    .compatibility()
                    .check(&current.bytes, &target.to_string())?;
                let current_manifest = Manifest::decode(&current.bytes)?;
                let config = client.small_blob(repository, &current_manifest.config)?;
                if recipe_version(&config).as_deref() == Some("1.0") {
                    return Err(Error::Config(
                        "a published recipe 1.0 cannot be replaced by a newer recipe version at the same output tag; publish a new output version or revision".into(),
                    ));
                }
            }
            TagOutcome::Moved {
                from: current.digest.clone(),
            }
        }
        None => TagOutcome::Created,
    };
    let plan = PushPlan {
        target: target.clone(),
        entry: entry.cloned(),
        image: artifact.descriptor(),
        manifest: artifact.manifest.clone(),
        manifest_bytes: artifact.manifest_bytes.clone(),
        closure: closure(artifact),
        verdict: Ok(()),
    };
    let mut report = |event: PushProgress<'_>| {
        if let PushProgress::Blob(descriptor, outcome) = event {
            progress(descriptor, outcome);
        }
    };
    let mut watch = push::Watch {
        progress: &mut report,
        cancelled: &|| false,
    };
    let preflight = current.map(|c| c.digest);
    push::finish(store, &plan, client, options, &mut watch, tag, preflight)
}

fn recipe_version(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let table: toml::Table = toml::from_str(text).ok()?;
    table.get("recipe")?.as_str().map(str::to_owned)
}

/// What a pull wrote.
#[derive(Debug)]
pub struct Pulled {
    pub artifact: Artifact,
    /// The recipe file in the folder pulled into.
    pub recipe: PathBuf,
}

/// The manifest at `source`, refused before anything else is fetched when
/// it needs a newer release, and decoded as a recipe artifact.
fn recipe_manifest(
    client: &Client,
    source: &Reference,
    compatibility: &image_store::compat::Compatibility,
) -> Result<(Vec<u8>, Manifest)> {
    if source.tag.as_deref() == Some("latest") {
        return Err(Error::Reference(
            "recipe artifacts have no latest tag".into(),
        ));
    }
    let fetched = client
        .manifest(&source.repository, &source.manifest_reference())?
        .ok_or_else(|| Error::NotFound(source.to_string()))?;
    compatibility.check(&fetched.bytes, &source.to_string())?;
    recipe_manifest_from(source, fetched)
}

/// Pull the recipe at `source` into `dir` (recipes §7.1, §7.2): every
/// part is fetched and verified, the artifact is inspected in a private
/// hidden scratch folder inside `dir`, removed afterwards, and only then
/// written into `dir`, where the recipe plans and builds with no other
/// file. A file already in `dir` with other bytes is refused before
/// anything is written there; a `dir` this pull created is removed again
/// when the pull fails.
pub fn pull(
    client: &Client,
    source: &Reference,
    dir: &Path,
    providers: Providers<'_>,
) -> Result<Pulled> {
    pull_with_compatibility(
        client,
        source,
        dir,
        providers,
        &historical_compatibility(providers),
    )
}

pub fn pull_with_compatibility(
    client: &Client,
    source: &Reference,
    dir: &Path,
    providers: Providers<'_>,
    compatibility: &image_store::compat::Compatibility,
) -> Result<Pulled> {
    if source.host != client.host() {
        return Err(Error::Config("client and source registry differ".into()));
    }
    let (manifest_bytes, manifest) = recipe_manifest(client, source, compatibility)?;
    let repository = source.repository.as_str();
    let config = client.small_blob(repository, &manifest.config)?;
    let mut layers = Vec::new();
    for layer in &manifest.layers {
        layers.push(client.small_blob(repository, layer)?);
    }
    let artifact = Artifact::from_parts_with_levels(
        manifest_bytes.clone(),
        config,
        layers,
        compatibility.manifest_levels(&manifest_bytes),
    )?;
    let created = !dir.exists();
    let result = (|| -> Result<PathBuf> {
        {
            let scratch = Scratch::new(dir)?;
            artifact::inspect(&artifact, providers, &scratch.0)?;
        }
        Ok(artifact.unpack(dir).map_err(image_store::Error::from)?)
    })();
    match result {
        Ok(recipe) => Ok(Pulled { artifact, recipe }),
        Err(error) => {
            if created {
                let _ = std::fs::remove_dir(dir);
            }
            Err(error)
        }
    }
}

/// A registry row of the Recipes group (recipes §7.4), judged from the
/// manifest and the config alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub reference: String,
    /// The manifest's digest.
    pub digest: Digest,
    /// The output, `name:tag`, and its pinned digest, as the manifest names
    /// them.
    pub output: Option<String>,
    pub output_digest: Option<String>,
    pub state: RowState,
}

/// What a row says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    /// "Needs unemupc X or newer": judged from the manifest alone.
    NeedsNewer(String),
    /// The recipe's plan, or its findings.
    Judged(Judged),
}

/// Judge the recipe at `reference` for a row: its manifest, then its
/// config (both at most 1 MiB), planned against `sources`. No part of its
/// bundle is fetched; a recipe that needs a newer release is not fetched
/// beyond its manifest.
pub fn row(
    client: &Client,
    reference: &Reference,
    providers: Providers<'_>,
    sources: Sources<'_>,
) -> Result<Row> {
    row_with_compatibility(
        client,
        reference,
        providers,
        sources,
        &historical_compatibility(providers),
    )
}

pub fn row_with_compatibility(
    client: &Client,
    reference: &Reference,
    providers: Providers<'_>,
    sources: Sources<'_>,
    compatibility: &image_store::compat::Compatibility,
) -> Result<Row> {
    let repository = reference.repository.as_str();
    let fetched = client
        .manifest(repository, &reference.manifest_reference())?
        .ok_or_else(|| Error::NotFound(reference.to_string()))?;
    let needs = artifact::needs(
        &fetched.bytes,
        compatibility.manifest_levels(&fetched.bytes),
    );
    if needs.is_none() {
        compatibility.check(&fetched.bytes, &reference.to_string())?;
    }
    let lenient = Manifest::decode_for_summary(&fetched.bytes).ok();
    let annotation = |key: &str| {
        lenient
            .as_ref()
            .and_then(|m| m.annotations.get(key).cloned())
    };
    let mut row = Row {
        reference: reference.to_string(),
        digest: fetched.digest.clone(),
        output: annotation(artifact::OUTPUT),
        output_digest: annotation(artifact::OUTPUT_DIGEST),
        state: RowState::NeedsNewer(String::new()),
    };
    if let Some(reason) = needs {
        row.state = RowState::NeedsNewer(reason);
        return Ok(row);
    }
    let (_, manifest) = recipe_manifest_from(reference, fetched)?;
    let config = client.small_blob(repository, &manifest.config)?;
    let judged = artifact::judge(
        &manifest,
        &config,
        &reference.to_string(),
        providers,
        sources,
    )?;
    row.state = RowState::Judged(judged);
    Ok(row)
}

/// [`recipe_manifest`] for a manifest fetched already.
fn recipe_manifest_from(
    source: &Reference,
    fetched: crate::client::FetchedManifest,
) -> Result<(Vec<u8>, Manifest)> {
    if fetched
        .media_type
        .as_deref()
        .is_some_and(|t| t != media_type::MANIFEST)
    {
        return Err(Error::Protocol(format!(
            "unsupported manifest media type {}",
            fetched.media_type.unwrap_or_default()
        )));
    }
    let envelope = Manifest::decode_for_summary(&fetched.bytes)?;
    if !image_store::is_recipe(envelope.artifact_type()) {
        return Err(Error::Protocol(format!(
            "{source} is a {}, not a recipe; image pull fetches it",
            envelope.artifact_type()
        )));
    }
    let manifest = Manifest::decode(&fetched.bytes)?;
    Ok((fetched.bytes, manifest))
}

fn historical_compatibility(providers: Providers<'_>) -> image_store::compat::Compatibility {
    use image_store::compat::{Compatibility, BASELINE, EXTRACTION_BASELINE};
    providers.platforms().into_iter().fold(
        Compatibility::new(EXTRACTION_BASELINE, BASELINE),
        |runtime, platform| runtime.with_core(platform, EXTRACTION_BASELINE, BASELINE),
    )
}

#[cfg(test)]
mod machine_tests {
    use super::*;
    use crate::http::{Body, Method, Response, Transport};
    use ::recipe::provider::{
        CatalogueEntry, FirmwareProfile, MediaProfile, Provider, VolumeProfile,
    };
    use image_store::compat::{Compatibility, BASELINE, NEXT_RELEASE};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    struct Empty;
    impl Provider for Empty {
        fn platform(&self) -> &str {
            "synthetic"
        }
        fn media(&self) -> &dyn MediaProfile {
            self
        }
        fn firmware(&self) -> &dyn FirmwareProfile {
            self
        }
        fn needs(&self) -> Option<&dyn image_store::needs::Vocabulary> {
            None
        }
        fn software_platforms(&self) -> &[&str] {
            &[]
        }
        fn work_id_catalogues(&self) -> &[&str] {
            &[]
        }
    }
    impl MediaProfile for Empty {
        fn formats(&self) -> &[&str] {
            &["720K"]
        }
        fn format_name(&self, _: &toml::Table) -> Option<String> {
            Some("720K".into())
        }
        fn size_fault(&self, _: &str, _: u64) -> Option<String> {
            None
        }
        fn volume_profile(&self, _: &str) -> Option<&dyn VolumeProfile> {
            None
        }
        fn dos_name_fault(&self, _: &str) -> Option<String> {
            None
        }
    }
    impl FirmwareProfile for Empty {
        fn roles(&self) -> &[&str] {
            &[]
        }
        fn lanes(&self) -> &[&str] {
            &[]
        }
        fn boards(&self) -> &[&str] {
            &[]
        }
        fn machine_classes(&self) -> &[&str] {
            &[]
        }
        fn entry(&self, _: &str) -> Option<CatalogueEntry> {
            None
        }
        fn identify(&self, _: &str, _: u64) -> Option<(String, CatalogueEntry)> {
            None
        }
    }
    #[derive(Default)]
    struct Remote {
        manifest: Option<Vec<u8>>,
        blobs: BTreeMap<String, Vec<u8>>,
        requests: Vec<String>,
    }
    struct Memory(Arc<Mutex<Remote>>);
    impl Transport for Memory {
        fn send(
            &self,
            method: Method,
            url: &str,
            _: &[(&str, String)],
            body: &Body<'_>,
        ) -> std::io::Result<Response> {
            let mut remote = self.0.lock().unwrap();
            remote.requests.push(format!("{} {url}", method.as_str()));
            if url.contains("/manifests/") {
                if method == Method::Put {
                    let Body::Bytes(bytes) = body else {
                        panic!("manifest body must be bytes")
                    };
                    remote.manifest = Some(bytes.to_vec());
                    let mut response = Response::manifest_bytes(url, Vec::new());
                    response.status = 201;
                    return Ok(response);
                }
                if method == Method::Head {
                    let bytes = remote.manifest.as_deref().unwrap_or_default();
                    let mut response = Response::manifest_head(url, &Digest::of(bytes));
                    if remote.manifest.is_none() {
                        response.status = 404;
                    }
                    return Ok(response);
                }
                assert_eq!(method, Method::Get);
                let mut response =
                    Response::manifest_bytes(url, remote.manifest.clone().unwrap_or_default());
                if remote.manifest.is_none() {
                    response.status = 404;
                }
                return Ok(response);
            }
            let digest = url.rsplit('/').next().unwrap();
            let bytes = remote
                .blobs
                .get(digest)
                .expect("only bundled parts may be requested");
            if method == Method::Head {
                return Ok(Response::manifest_head(url, &digest.parse().unwrap()));
            }
            assert_eq!(method, Method::Get);
            Ok(Response::manifest_bytes(url, bytes.clone()))
        }
    }

    #[test]
    fn machine_recipe_bundle_push_pull_inspect_and_tag_guards_round_trip() {
        let temp = image_store::test_support::TempDir::new("machine-recipe-registry");
        let runtime = Compatibility::new(NEXT_RELEASE, NEXT_RELEASE).with_core(
            "synthetic",
            BASELINE,
            BASELINE,
        );
        let store = Store::open(temp.path().join("store"))
            .unwrap()
            .with_compatibility(runtime.clone());
        let marked = |kind: &str, bytes: &[u8]| {
            let mut d = Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
            image_store::license::mark(
                &mut d,
                Some(&image_store::license::Expression::parse("CC0-1.0").unwrap()),
            );
            d
        };
        let input = b"bound=1\n";
        let config = b"schema=1\n";
        let metadata = image_store::metadata::Metadata::new("synthetic")
            .encode(&[])
            .unwrap();
        let mut output = Manifest::new(
            "synthetic",
            marked(media_type::CONFIG, config),
            vec![marked(media_type::METADATA, &metadata)],
        )
        .unwrap();
        output
            .annotations
            .insert("unemu.template.uses".into(), String::new());
        let manifest = output.encode().unwrap();
        let digest = Digest::of(&manifest);
        let path = temp.path().join("demo.machine.toml");
        let recipe = format!("recipe='1.1'\n[machine]\nname='unemu/team/machines/synthetic/demo'\nplatform='synthetic'\nversion='1.0.0'\ndigest='{digest}'\n[template]\nconfig='config'\n[license]\nexpression='CC0-1.0'\n[input.config]\nname='config.toml'\nsize={}\nsha256='{}'\nbundle=true\n", input.len(), Digest::of(input).hex());
        std::fs::write(&path, recipe).unwrap();
        std::fs::write(temp.path().join("config.toml"), input).unwrap();
        ::recipe::lock::write(
            &::recipe::lock::dir(&path),
            &[manifest, config.to_vec(), metadata],
        )
        .unwrap();
        let providers = Providers(&[&Empty]);
        let artifact =
            artifact::bundle_with_levels(&path, providers, runtime.levels("synthetic")).unwrap();
        let target = Reference::parse(
            "registry.example/unemu/team/recipes/machines/synthetic/demo:1.0.0",
            "registry.example",
        )
        .unwrap();
        let entry = RegistryEntry::builtin("registry.example");
        let remote = Arc::new(Mutex::new(Remote::default()));
        for (d, bytes) in std::iter::once((&artifact.manifest.config, artifact.config.as_slice()))
            .chain(artifact.parts())
        {
            remote
                .lock()
                .unwrap()
                .blobs
                .insert(d.digest.to_string(), bytes.to_vec());
        }
        let client =
            Client::for_test_transport("registry.example", Box::new(Memory(remote.clone())));
        let report = push(
            &store,
            &artifact,
            &target,
            Some(&entry),
            providers,
            &client,
            &PushOptions::default(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(report.tag, TagOutcome::Created);
        assert_eq!(
            remote.lock().unwrap().manifest.as_ref().unwrap(),
            &artifact.manifest_bytes
        );
        let pulled = pull_with_compatibility(
            &client,
            &target,
            &temp.path().join("pull"),
            providers,
            &runtime,
        )
        .unwrap();
        assert_eq!(pulled.artifact, artifact);
        let repeated = push(
            &store,
            &artifact,
            &target,
            Some(&entry),
            providers,
            &client,
            &PushOptions::default(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(repeated.tag, TagOutcome::Unchanged);
        let mut changed = artifact.manifest.clone();
        changed.annotations.insert(
            artifact::OUTPUT_DIGEST.into(),
            format!("sha256:{}", "0".repeat(64)),
        );
        remote.lock().unwrap().manifest = Some(changed.encode().unwrap());
        let before = remote.lock().unwrap().requests.len();
        let error = push(
            &store,
            &artifact,
            &target,
            Some(&entry),
            providers,
            &client,
            &PushOptions {
                allow_move: true,
                ..PushOptions::default()
            },
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(matches!(error, Error::RecipeTagTaken { .. }));
        assert_eq!(remote.lock().unwrap().requests.len(), before + 1);
        let latest = Reference::parse(
            "registry.example/unemu/team/recipes/machines/synthetic/demo:latest",
            "registry.example",
        )
        .unwrap();
        let before = remote.lock().unwrap().requests.len();
        assert!(push(
            &store,
            &artifact,
            &latest,
            Some(&entry),
            providers,
            &client,
            &PushOptions::default(),
            &mut |_, _| {}
        )
        .is_err());
        assert_eq!(remote.lock().unwrap().requests.len(), before);
    }
    #[test]
    fn a_published_recipe_keeps_version_one_zero_at_its_existing_output_tag() {
        let temp = image_store::test_support::TempDir::new("recipe-version-guard");
        let runtime = Compatibility::new(NEXT_RELEASE, NEXT_RELEASE).with_core(
            "synthetic",
            BASELINE,
            BASELINE,
        );
        let store = Store::open(temp.path().join("store"))
            .unwrap()
            .with_compatibility(runtime.clone());
        let payload = b"x";
        let mut medium = Descriptor::new(media_type::FLOPPY, Digest::of(payload), 1);
        medium
            .annotations
            .insert(image_store::export_name::TITLE.into(), "disk.txt".into());
        let config = image_store::media_set::Config {
            schema: 1,
            platform: "synthetic".into(),
            media: vec![image_store::media_set::Medium {
                id: "disk-1".into(),
                label: "Disk 1".into(),
                format: toml::from_str("kind='synthetic'").unwrap(),
                blob: medium.digest.clone(),
            }],
            extras: Vec::new(),
        }
        .encode()
        .unwrap();
        let mut metadata = image_store::metadata::Metadata::new("synthetic");
        metadata
            .titles
            .push(serde_json::from_str(r#"{"id":"demo","title":"Demo"}"#).unwrap());
        let metadata = metadata.encode(&[]).unwrap();
        let output = Manifest::new_media_set(
            "synthetic",
            Descriptor::new(
                media_type::MEDIA_SET_CONFIG,
                Digest::of(&config),
                config.len() as u64,
            ),
            vec![
                medium,
                Descriptor::new(
                    media_type::METADATA,
                    Digest::of(&metadata),
                    metadata.len() as u64,
                ),
            ],
        )
        .unwrap();
        let manifest = output.encode().unwrap();
        let digest = Digest::of(&manifest);
        let path = temp.path().join("demo.media.toml");
        let text = format!("recipe='1.0'\n[media]\nname='team/media/synthetic/demo'\nplatform='synthetic'\nrevision=1\ndigest='{digest}'\n[title]\ntitle='Demo'\n[input.disk]\nname='disk.txt'\nsize=1\nsha256='{}'\nbundle=true\ndisclosable=true\n[[medium]]\nid='disk-1'\nlabel='Disk 1'\nformat='720K'\nfile_name='disk.txt'\nfrom='disk'\n", Digest::of(payload).hex());
        std::fs::write(&path, &text).unwrap();
        std::fs::write(temp.path().join("disk.txt"), payload).unwrap();
        ::recipe::lock::write(&::recipe::lock::dir(&path), &[manifest, config, metadata]).unwrap();
        let providers = Providers(&[&Empty]);
        let old =
            artifact::bundle_with_levels(&path, providers, runtime.levels("synthetic")).unwrap();
        std::fs::write(&path, text.replace("recipe='1.0'", "recipe='1.1'")).unwrap();
        let new =
            artifact::bundle_with_levels(&path, providers, runtime.levels("synthetic")).unwrap();
        assert_eq!(old.output_digest(), new.output_digest());
        let remote = Arc::new(Mutex::new(Remote::default()));
        remote.lock().unwrap().manifest = Some(old.manifest_bytes.clone());
        for artifact in [&old, &new] {
            for (d, bytes) in
                std::iter::once((&artifact.manifest.config, artifact.config.as_slice()))
                    .chain(artifact.parts())
            {
                remote
                    .lock()
                    .unwrap()
                    .blobs
                    .insert(d.digest.to_string(), bytes.to_vec());
            }
        }
        let client =
            Client::for_test_transport("registry.example", Box::new(Memory(remote.clone())));
        let entry = RegistryEntry::builtin("registry.example");
        let target = Reference::parse(
            "registry.example/team/recipes/media/synthetic/demo:r1",
            "registry.example",
        )
        .unwrap();
        for key in [
            image_store::compat::MIN_APP_VERSION,
            image_store::compat::MIN_FRONTEND_VERSION,
        ] {
            let mut future = old.manifest.clone();
            future.annotations.insert(key.into(), "99.0.0".into());
            remote.lock().unwrap().manifest = Some(future.encode().unwrap());
            remote.lock().unwrap().requests.clear();
            let error = push(
                &store,
                &new,
                &target,
                Some(&entry),
                providers,
                &client,
                &PushOptions::default(),
                &mut |_, _| {},
            )
            .unwrap_err();
            assert!(error.to_string().contains("99.0.0"), "{error}");
            assert_eq!(remote.lock().unwrap().requests.len(), 1);
        }
        remote.lock().unwrap().manifest = Some(old.manifest_bytes.clone());
        remote.lock().unwrap().requests.clear();
        let error = push(
            &store,
            &new,
            &target,
            Some(&entry),
            providers,
            &client,
            &PushOptions {
                allow_move: true,
                ..PushOptions::default()
            },
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("published recipe 1.0"),
            "{error}"
        );
        assert_eq!(
            remote.lock().unwrap().manifest.as_ref().unwrap(),
            &old.manifest_bytes
        );
        assert!(!remote
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|request| request.starts_with("PUT ")));
        assert_eq!(remote.lock().unwrap().requests.len(), 2);
        remote.lock().unwrap().manifest = Some(new.manifest_bytes.clone());
        std::fs::write(
            &path,
            "# own publication note\n".to_owned() + &text.replace("recipe='1.0'", "recipe='1.1'"),
        )
        .unwrap();
        let updated =
            artifact::bundle_with_levels(&path, providers, runtime.levels("synthetic")).unwrap();
        remote.lock().unwrap().blobs.insert(
            updated.manifest.config.digest.to_string(),
            updated.config.clone(),
        );
        let report = push(
            &store,
            &updated,
            &target,
            Some(&entry),
            providers,
            &client,
            &PushOptions::default(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(report.tag, TagOutcome::Moved { from: new.digest });
    }
}
