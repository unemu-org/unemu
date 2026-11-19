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
//! `--move`, and is refused otherwise, whatever the options say. The parts
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
use image_store::{compat::Requirement, media_type, Descriptor, Digest, Manifest, Store};
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

/// What a pull wrote.
#[derive(Debug)]
pub struct Pulled {
    pub artifact: Artifact,
    /// The recipe file in the folder pulled into.
    pub recipe: PathBuf,
}

/// The manifest at `source`, refused before anything else is fetched when
/// it needs a newer release, and decoded as a recipe artifact.
fn recipe_manifest(client: &Client, source: &Reference) -> Result<(Vec<u8>, Manifest)> {
    let fetched = client
        .manifest(&source.repository, &source.manifest_reference())?
        .ok_or_else(|| Error::NotFound(source.to_string()))?;
    Requirement::read(&fetched.bytes).check(&source.to_string())?;
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
    if source.host != client.host() {
        return Err(Error::Config("client and source registry differ".into()));
    }
    let (manifest_bytes, manifest) = recipe_manifest(client, source)?;
    let repository = source.repository.as_str();
    let config = client.small_blob(repository, &manifest.config)?;
    let mut layers = Vec::new();
    for layer in &manifest.layers {
        layers.push(client.small_blob(repository, layer)?);
    }
    let artifact = Artifact::from_parts(manifest_bytes, config, layers)?;
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
    let repository = reference.repository.as_str();
    let fetched = client
        .manifest(repository, &reference.manifest_reference())?
        .ok_or_else(|| Error::NotFound(reference.to_string()))?;
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
    if let Some(reason) = artifact::needs(&fetched.bytes) {
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
