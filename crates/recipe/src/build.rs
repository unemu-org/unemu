// SPDX-License-Identifier: AGPL-3.0-or-later
//! The draft build of a recipe (recipes §3.1, §6.12, §6.14): the plan, the
//! inputs read and checked, the steps run, the outputs handed to the
//! platform's importers, which tag the artifact last.
//!
//! **Phases.** [`build_path`] runs them in this order, and each phase only
//! sees what the one before it hands on:
//!
//! 1. *Check and plan*: the static checks, then [`crate::plan::plan`], which
//!    finds every input in the store, the download cache, beside the recipe
//!    or in a library, without the network.
//! 2. *Fetch*, only with the user's consent ([`Fetcher`], `--fetch`): each
//!    URL of a required file input the plan reports missing. Inputs without
//!    URLs are looked for by hash after downloads. The download list is printed
//!    first. A download stops at the pinned size and is kept in the
//!    download cache under its SHA-256 only when its size and hash are the
//!    ones pinned; the plan then runs again.
//! 3. *Gather* ([`gather`]): every value the plan found is read and checked
//!    against its size and SHA-256 again, wherever it was found.
//! 4. *Steps* ([`run`]): a pure function of the gathered bytes, the recipe
//!    and the platform's volume profile. It holds no store, library,
//!    network or clock handle, so a step cannot reach any; values are
//!    visited in recipe order and kept in sorted maps.
//! 5. *Publish* ([`stage`], [`Publisher`]): the outputs are written to a
//!    staging folder of the store's cache under their file names and handed
//!    to the platform's importers, the same plan-and-publish functions
//!    `image media import` and `image firmware import` end in. The importer
//!    copies and hashes every file, writes the manifest and tags it last.
//!
//! Nothing is tagged when an input is missing, a fetch fails, a value read
//! is not the one pinned, or a step or the importer refuses. A recipe
//! without a `digest` is a draft: its result is tagged and its digest
//! printed, and nothing is compared with a pin.
//!
//! **A pinned recipe** (one with a `digest`, recipes §3.1, §3.8) runs the
//! same phases up to the steps; then, instead of the importers, its outputs
//! are checked against the lock ([`crate::lock::verify`]) and the lock's
//! manifest, config and metadata are published unchanged
//! ([`crate::lock::store`], [`Publisher::tag_locked`]). So a later build
//! of the program, whose writers spell those files differently, still
//! publishes the pinned digest. A mismatch tags nothing ([`Outcome::Mismatch`]).
//!
//! Before the steps, a pinned build looks every output a step makes up by
//! the payload its lock names, in the store, the download cache and the
//! libraries ([`crate::plan::plan_pinned`]); a found output is read and
//! checked by its SHA-256 like any input, and the steps and inputs only it
//! needed are not run or read (recipes §3.4, "found instead of made").
//!
//! **A lost lock** (recipes §7.2): a pinned recipe with nothing at its
//! lock's path is built as a draft with this build's writers, into a
//! scratch store. When that gives the pinned digest, the files it wrote
//! stand in for the lock and the build goes on as a pinned one; otherwise
//! it says the recipe needs its lock and tags nothing. A lock that is
//! there and broken is not lost: the checks refuse it.
//!
//! **Limits of this version.** Values are held in memory while the steps
//! run. Attachments (recipes §3.9) are not built: their manifests need a
//! `subject` the manifest codec does not have yet; a build says so and
//! builds the artifact without them.

use crate::library::Library;
use crate::merge::Merged;
use crate::model::{Input, InputForm, Kind, Op, Recipe, RomSource};
use crate::plan::{Form, Found, InputPlan, Plan, Sources, Status};
use crate::provider::{Provider, VolumeKeys};
use crate::steps::{self, Bytes, MemberSpec, TreeFile, Value};
use crate::store_index::StoreIndex;
use crate::Providers;
use image_store::metadata::Title;
use image_store::{Descriptor, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The folder of the store's cache that holds downloaded inputs, each named
/// by its lower-case SHA-256.
pub const DOWNLOADS: &str = "cache/downloads";

/// The folder of the store's cache where a build writes its outputs for the
/// importers; each build has its own folder under it and removes it.
pub const STAGING: &str = "cache/recipe-build";

/// Reads content the store holds by its decoded SHA-256 (recipes §3.4,
/// source 1): the program's platforms know how to resolve an image.
pub trait StoreRead {
    /// The decoded bytes of `sha256` from an image under one of `tags`.
    fn read(&self, tags: &[String], size: u64, sha256: &str) -> io::Result<Vec<u8>>;
}

/// Store content read through the validators of the platforms compiled in:
/// the image of a tag is resolved once, which verifies it, and content is
/// streamed from it.
pub struct StoreContent<'a> {
    store: &'a Store,
    validators: Vec<&'a dyn image_store::ConfigValidator>,
    resolved: std::cell::RefCell<BTreeMap<String, std::rc::Rc<image_store::ResolvedImage>>>,
}

impl<'a> StoreContent<'a> {
    pub fn new(store: &'a Store, validators: Vec<&'a dyn image_store::ConfigValidator>) -> Self {
        Self {
            store,
            validators,
            resolved: Default::default(),
        }
    }

    /// The image of `tag`, resolved with the validator of its artifact type
    /// and platform.
    fn image(&self, tag: &str) -> image_store::Result<std::rc::Rc<image_store::ResolvedImage>> {
        use image_store::BlobSource as _;
        if let Some(image) = self.resolved.borrow().get(tag) {
            return Ok(image.clone());
        }
        let descriptor = self.store.tag_descriptor(tag)?;
        let manifest = image_store::Manifest::decode_for_summary(
            &self
                .store
                .read_verified(&descriptor, image_store::MAX_SMALL_BLOB)?,
        )?;
        let family = manifest.platform()?;
        let validator = self
            .validators
            .iter()
            .find(|v| {
                v.artifact_type() == manifest.artifact_type()
                    && image_store::platform::same(v.platform(), family)
            })
            .ok_or_else(|| {
                image_store::Error::Invalid(format!(
                    "no provider of {} on platform {family}",
                    manifest.artifact_type()
                ))
            })?;
        let image = std::rc::Rc::new(self.store.resolve(tag, *validator)?);
        self.resolved
            .borrow_mut()
            .insert(tag.to_owned(), image.clone());
        Ok(image)
    }
}

impl StoreRead for StoreContent<'_> {
    fn read(&self, tags: &[String], size: u64, sha256: &str) -> io::Result<Vec<u8>> {
        use std::io::Read as _;
        let digest: image_store::Digest = format!("sha256:{sha256}")
            .parse()
            .map_err(|e: image_store::Error| io::Error::other(e.to_string()))?;
        let mut last = None;
        for tag in tags {
            let attempt = (|| -> image_store::Result<Vec<u8>> {
                let mut bytes = Vec::new();
                self.image(tag)?
                    .content_reader(&digest)?
                    .take(size.saturating_add(1))
                    .read_to_end(&mut bytes)?;
                Ok(bytes)
            })();
            match attempt {
                Ok(bytes) => return Ok(bytes),
                Err(e) => last = Some(format!("{tag}: {e}")),
            }
        }
        Err(io::Error::other(
            last.unwrap_or_else(|| "no image holds it".into()),
        ))
    }
}

/// Downloads one URL with the user's consent (recipes §6.13): `https`
/// only, redirects that stay on `https`, at most `size` bytes handed to
/// `sink`. The build checks the size and SHA-256 afterwards.
pub trait Fetcher {
    fn fetch(&self, url: &str, size: u64, sink: &mut dyn Write) -> Result<(), String>;
}

/// Hands a staged artifact to the platform's importers.
pub trait Publisher {
    /// Publish `artifact` into `store` under `artifact.reference`, tag last.
    /// A tag that already names exactly this artifact is reported as
    /// present, not written again; any refusal is in the importer's words.
    fn publish(&self, store: &Store, artifact: &Staged) -> Result<Published, String>;

    /// Tag the locked manifest of a pinned build, already stored with its
    /// whole closure ([`crate::lock::store`]), as `reference`, with the
    /// platform's validator of its artifact type ([`crate::lock::tag`]).
    /// The publisher writes no manifest, config or metadata of its own.
    fn tag_locked(
        &self,
        store: &Store,
        reference: &str,
        manifest: &Descriptor,
    ) -> Result<Published, String> {
        let _ = (store, reference, manifest);
        Err("this build cannot publish a locked artifact of this platform".into())
    }
}

/// What a publication did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub descriptor: Descriptor,
    /// `false`: the tag already named this artifact.
    pub created: bool,
}

/// The values the plan found, read and checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gathered {
    /// Each input the artifact needs, by id.
    pub inputs: BTreeMap<String, Value>,
    /// Members of `unpack` steps found instead of made, by step and path.
    pub members: BTreeMap<(String, String), Bytes>,
    /// Outputs of a pinned recipe found instead of made, by kind (`medium`,
    /// `rom`, `extra`), place among the outputs of that kind, and file.
    pub outputs: BTreeMap<(&'static str, usize, usize), Bytes>,
}

/// One medium the steps made, before it is staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltMedium {
    pub id: String,
    pub label: String,
    pub format: String,
    pub file_name: String,
    pub bytes: Bytes,
}

/// One chip or whole ROM: the name of the value it came from and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltFile {
    pub name: String,
    pub bytes: Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltRom {
    pub id: String,
    pub role: String,
    pub catalogue_id: Option<String>,
    /// One file for a ROM stored whole, one per chip for `parts`.
    pub files: Vec<BuiltFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltExtra {
    pub id: String,
    pub file_name: String,
    pub kind: Option<String>,
    pub dos_name: Option<String>,
    pub bytes: Bytes,
}

/// What the steps made: the outputs, in recipe order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Built {
    pub media: Vec<BuiltMedium>,
    pub roms: Vec<BuiltRom>,
    pub extras: Vec<BuiltExtra>,
    /// Archives an `unpack` step opened, by input id, and how many of their
    /// file members the recipe did not take.
    pub unused_members: BTreeMap<String, usize>,
}

/// A staged file: where it lies, under the file name the artifact gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedFile {
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedMedium {
    pub id: String,
    pub label: String,
    pub format: String,
    pub file: StagedFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedRom {
    pub id: String,
    pub role: String,
    pub catalogue_id: Option<String>,
    pub files: Vec<StagedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedExtra {
    pub id: String,
    pub kind: Option<String>,
    pub dos_name: Option<String>,
    /// The file is named the extra's `file_name`.
    pub file: StagedFile,
}

/// `[license]`, with the extras it names by their file names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedLicense {
    pub expression: String,
    pub terms: Option<String>,
    pub source: Option<String>,
}

/// `[package]` of a Firmwarefile (recipes §6.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StagedPackage {
    pub title: String,
    pub vendor: Option<String>,
    pub version: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
    /// `None`: the recipe states none.
    pub labels: Option<Vec<String>>,
    pub boards: Option<Vec<String>>,
    pub machine_classes: Option<Vec<String>>,
}

/// An artifact ready for the importers: every output a file in the staging
/// folder, and the description the recipe and its work file state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub kind: Kind,
    /// `<repository>:<tag>`, the reference the importer tags.
    pub reference: String,
    pub media: Vec<StagedMedium>,
    pub roms: Vec<StagedRom>,
    pub extras: Vec<StagedExtra>,
    pub license: Option<StagedLicense>,
    /// A Mediafile's merged title record and set labels (recipes §6.16),
    /// handed through unchanged.
    pub title: Option<Title>,
    pub labels: Vec<String>,
    pub package: Option<StagedPackage>,
    /// `min_app_version` of the identity table.
    pub min_app_version: Option<String>,
}

/// Read every value the plan found and the artifact needs, and check each
/// against its size and SHA-256. Content found in several places is read
/// once.
pub fn gather(
    recipe: &Recipe,
    plan: &Plan,
    libraries: &[Library],
    store: Option<&dyn StoreRead>,
) -> Result<Gathered, String> {
    let mut read = Reader {
        libraries,
        store,
        cache: BTreeMap::new(),
    };
    let mut gathered = Gathered::default();
    for input in &plan.inputs {
        if input.optional || input.status != Status::Ready {
            continue;
        }
        let Some(model) = recipe.inputs.iter().find(|i| i.id.value == input.id) else {
            continue;
        };
        let value = match &input.form {
            Form::File(value) => Value::File(
                read.value(value.found.as_ref(), value.size, &value.sha256)
                    .map_err(|e| format!("input {}: {e}", input.id))?,
            ),
            Form::Files(files) => {
                let times = file_times(model);
                let mut tree = Vec::new();
                for (path, value) in files {
                    let bytes = read
                        .value(value.found.as_ref(), value.size, &value.sha256)
                        .map_err(|e| format!("input {}, file {path}: {e}", input.id))?;
                    tree.push(TreeFile {
                        path: path.clone(),
                        bytes,
                        time: times.get(path.as_str()).copied().flatten(),
                    });
                }
                Value::Tree(tree)
            }
        };
        gathered.inputs.insert(input.id.clone(), value);
    }
    for member in &plan.members {
        if member.value.found.is_none() {
            continue;
        }
        let bytes = read
            .value(
                member.value.found.as_ref(),
                member.value.size,
                &member.value.sha256,
            )
            .map_err(|e| format!("member {}/{}: {e}", member.step, member.path))?;
        gathered
            .members
            .insert((member.step.clone(), member.path.clone()), bytes);
    }
    for payload in &plan.payloads {
        if payload.value.found.is_none() {
            continue;
        }
        let bytes = read
            .value(
                payload.value.found.as_ref(),
                payload.value.size,
                &payload.value.sha256,
            )
            .map_err(|e| format!("{}: {e}", payload.what()))?;
        gathered
            .outputs
            .insert((payload.kind, payload.index, payload.part), bytes);
    }
    Ok(gathered)
}

fn file_times(input: &Input) -> BTreeMap<&str, Option<crate::model::LocalTime>> {
    match &input.form {
        InputForm::FileSet { files } => files
            .iter()
            .map(|f| (f.path.value.as_str(), f.time.as_ref().map(|t| t.value)))
            .collect(),
        _ => BTreeMap::new(),
    }
}

struct Reader<'a> {
    libraries: &'a [Library],
    store: Option<&'a dyn StoreRead>,
    cache: BTreeMap<String, Bytes>,
}

impl Reader<'_> {
    /// The bytes of a found value, checked against its size and SHA-256.
    fn value(&mut self, found: Option<&Found>, size: u64, sha256: &str) -> Result<Bytes, String> {
        if let Some(bytes) = self.cache.get(sha256) {
            return Ok(bytes.clone());
        }
        let found = found.ok_or("not found")?;
        let bytes = match found {
            Found::Store { tags } => self
                .store
                .ok_or("no store to read it from")?
                .read(tags, size, sha256),
            Found::Download { path } | Found::Bundle { path } => std::fs::read(path),
            Found::Library { library, place } => self
                .libraries
                .iter()
                .find(|l| &l.named == library)
                .ok_or_else(|| format!("library {} is not open", library.display()))?
                .read(place),
        }
        .map_err(|e| format!("{found}: {e}"))?;
        if bytes.len() as u64 != size || steps::sha256(&bytes) != sha256 {
            return Err(format!(
                "{found}: no longer holds the {size} bytes of sha256 {sha256}; it changed since it was found"
            ));
        }
        let bytes = Arc::new(bytes);
        self.cache.insert(sha256.to_owned(), bytes.clone());
        Ok(bytes)
    }
}

/// Run the steps the outputs need and collect the outputs, in recipe order.
/// A pure function of its arguments: the steps get bytes, keys and the
/// provider's volume profile, nothing else.
pub fn run(recipe: &Recipe, gathered: &Gathered, provider: &dyn Provider) -> Result<Built, String> {
    let mut runner = Runner {
        recipe,
        gathered,
        provider,
        steps: BTreeMap::new(),
        unused: BTreeMap::new(),
    };
    let text = |v: &Option<crate::model::At<String>>| v.as_ref().map(|v| v.value.clone());
    let mut built = Built::default();
    // An output found instead of made is taken as found; the steps run only
    // for the others.
    let found = |kind: &'static str, index: usize, part: usize| {
        gathered.outputs.get(&(kind, index, part)).cloned()
    };
    for (index, medium) in recipe.media.iter().enumerate() {
        let id = text(&medium.id).unwrap_or_default();
        let format = text(&medium.format).unwrap_or_default();
        let from = text(&medium.from).unwrap_or_default();
        let bytes = match found("medium", index, 0) {
            Some(bytes) => bytes,
            None => runner
                .file(&from)
                .map_err(|e| format!("medium {id}: {e}"))?,
        };
        let file_name =
            text(&medium.file_name).unwrap_or_else(|| provider.media().file_name(&id, &format));
        built.media.push(BuiltMedium {
            label: text(&medium.label).unwrap_or_else(|| id.clone()),
            id,
            format,
            file_name,
            bytes,
        });
    }
    for (index, rom) in recipe.roms.iter().enumerate() {
        let id = text(&rom.id).unwrap_or_default();
        let references: Vec<String> = match &rom.source {
            RomSource::File(from) => text(from).into_iter().collect(),
            RomSource::Parts { parts, .. } => parts.iter().filter_map(|p| text(&p.from)).collect(),
        };
        let mut files = Vec::new();
        for (part, reference) in references.into_iter().enumerate() {
            let bytes = match found("rom", index, part) {
                Some(bytes) => bytes,
                None => runner
                    .file(&reference)
                    .map_err(|e| format!("rom {id}: {e}"))?,
            };
            files.push(BuiltFile {
                name: runner.name_of(&reference),
                bytes,
            });
        }
        built.roms.push(BuiltRom {
            role: text(&rom.role).unwrap_or_default(),
            catalogue_id: text(&rom.catalogue_id),
            id,
            files,
        });
    }
    for (index, extra) in recipe.extras.iter().enumerate() {
        let id = text(&extra.id).unwrap_or_default();
        let from = text(&extra.from).unwrap_or_default();
        let bytes = match found("extra", index, 0) {
            Some(bytes) => bytes,
            None => runner.file(&from).map_err(|e| format!("extra {id}: {e}"))?,
        };
        built.extras.push(BuiltExtra {
            file_name: text(&extra.file_name).unwrap_or_else(|| id.clone()),
            kind: text(&extra.kind),
            dos_name: text(&extra.dos_name),
            id,
            bytes,
        });
    }
    built.unused_members = runner.unused;
    Ok(built)
}

struct Runner<'a> {
    recipe: &'a Recipe,
    gathered: &'a Gathered,
    provider: &'a dyn Provider,
    steps: BTreeMap<String, Value>,
    unused: BTreeMap<String, usize>,
}

impl Runner<'_> {
    /// The value a reference names: an input or a step's result, or a
    /// member of either.
    fn value(&mut self, reference: &str) -> Result<Value, String> {
        let (name, member) = match reference.split_once('/') {
            Some((name, member)) => (name, Some(member)),
            None => (reference, None),
        };
        let whole = if let Some(value) = self.gathered.inputs.get(name) {
            value.clone()
        } else if self.recipe.inputs.iter().any(|i| i.id.value == name) {
            return Err(format!("the input {name} was not found"));
        } else {
            self.step(name)?
        };
        match member {
            None => Ok(whole),
            Some(path) => match whole {
                Value::Tree(files) => files
                    .into_iter()
                    .find(|f| f.path == path)
                    .map(|f| Value::File(f.bytes))
                    .ok_or_else(|| format!("{reference}: {name} holds no {path}")),
                Value::File(_) => Err(format!("{reference}: {name} is a file, not a tree")),
            },
        }
    }

    fn file(&mut self, reference: &str) -> Result<Bytes, String> {
        match self.value(reference)? {
            Value::File(bytes) => Ok(bytes),
            Value::Tree(_) => Err(format!("{reference} is a tree, where a file is wanted")),
        }
    }

    fn tree(&mut self, reference: &str) -> Result<Vec<TreeFile>, String> {
        match self.value(reference)? {
            Value::Tree(files) => Ok(files),
            Value::File(_) => Err(format!("{reference} is a file, where a tree is wanted")),
        }
    }

    /// The name a value goes by when an importer needs a file name for it:
    /// an input's canonical name, the last part of a member's path, or a
    /// step's id.
    fn name_of(&self, reference: &str) -> String {
        if let Some((_, path)) = reference.split_once('/') {
            return path.rsplit('/').next().unwrap_or(path).to_owned();
        }
        self.recipe
            .inputs
            .iter()
            .find(|i| i.id.value == reference)
            .and_then(|i| i.name.as_ref())
            .map_or_else(|| reference.to_owned(), |n| n.value.clone())
    }

    /// The result of the step `id`, run once.
    fn step(&mut self, id: &str) -> Result<Value, String> {
        if let Some(value) = self.steps.get(id) {
            return Ok(value.clone());
        }
        let step = self
            .recipe
            .steps
            .iter()
            .find(|s| s.id.as_ref().is_some_and(|i| i.value == id))
            .ok_or_else(|| format!("no input or step is named {id}"))?;
        let operand = |v: &Option<crate::model::At<String>>| {
            v.as_ref()
                .map(|v| v.value.clone())
                .ok_or_else(|| format!("step {id} has no operand"))
        };
        let value = match &step.op {
            Op::Unpack {
                archive,
                members,
                complete,
            } => {
                let archive_ref = operand(archive)?;
                let found: BTreeMap<String, Bytes> = self
                    .gathered
                    .members
                    .iter()
                    .filter(|((step, _), _)| step == id)
                    .map(|((_, path), bytes)| (path.clone(), bytes.clone()))
                    .collect();
                let specs: Vec<MemberSpec> = members
                    .iter()
                    .map(|m| MemberSpec {
                        path: m.path.value.clone(),
                        size: m.size.as_ref().map(|s| s.value),
                        sha256: m.sha256.as_ref().map(|s| s.value.clone()),
                        time: m.time.as_ref().map(|t| t.value),
                    })
                    .collect();
                // The archive is opened only when a member is still missing
                // and the plan therefore found the archive.
                let input = archive_ref.split('/').next().unwrap_or_default();
                let opened = if specs.iter().all(|m| found.contains_key(&m.path))
                    || !self.archive_available(&archive_ref)
                {
                    None
                } else {
                    Some(self.file(&archive_ref)?)
                };
                let tree = steps::unpack(
                    opened.as_deref().map(Vec::as_slice),
                    &specs,
                    &found,
                    *complete,
                )
                .map_err(|e| format!("step {id}: {e}"))?;
                if let Some(bytes) = &opened {
                    let held = crate::zip::Archive::open(bytes)
                        .map(|a| a.entries.len())
                        .unwrap_or(0);
                    *self.unused.entry(input.to_owned()).or_default() +=
                        held.saturating_sub(specs.len());
                }
                Value::Tree(tree)
            }
            Op::Text { text } => {
                let bytes = self.file(&operand(text)?)?;
                Value::File(Arc::new(
                    steps::text(&bytes).map_err(|e| format!("step {id}: {e}"))?,
                ))
            }
            Op::Fat {
                tree,
                placements,
                format,
                label,
                serial,
                order,
                time,
                volumes,
            } => {
                let files = match placements {
                    Some(placements) => placements
                        .iter()
                        .map(|p| {
                            Ok(TreeFile {
                                path: operand(&p.dos_name)?,
                                bytes: self.file(&operand(&p.from)?)?,
                                time: p.time.as_ref().map(|t| t.value),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?,
                    None => self.tree(&operand(tree)?)?,
                };
                let format = operand(format)?;
                let profile = self
                    .provider
                    .media()
                    .volume_profile(&format)
                    .ok_or_else(|| format!("step {id}: the format {format} holds no FAT volume"))?;
                let keys = VolumeKeys {
                    label: label.as_ref().map(|l| l.value.clone()),
                    serial: serial
                        .as_ref()
                        .map(|s| u32::try_from(s.value))
                        .transpose()
                        .map_err(|_| format!("step {id}: the serial is not a 32-bit number"))?,
                    listed: order.as_ref().is_some_and(|o| o.value == "listed"),
                    time: time.as_ref().map(|t| t.value),
                    volumes: volumes
                        .as_ref()
                        .map(|v| usize::try_from(v.value))
                        .transpose()
                        .map_err(|_| format!("step {id}: no such number of volumes"))?,
                };
                steps::fat(&files, &keys, profile).map_err(|e| format!("step {id}: {e}"))?
            }
            Op::Unknown { .. } => return Err(format!("step {id} has no verb")),
        };
        self.steps.insert(id.to_owned(), value.clone());
        Ok(value)
    }

    /// Whether the archive an `unpack` step names was gathered: the plan
    /// leaves it out when every member an output reads was found.
    fn archive_available(&self, reference: &str) -> bool {
        let name = reference.split('/').next().unwrap_or(reference);
        self.gathered.inputs.contains_key(name)
            || self
                .recipe
                .steps
                .iter()
                .any(|s| s.id.as_ref().is_some_and(|i| i.value == name))
    }
}

/// The tag a recipe's output takes: a machine's SemVer version, or
/// `[<release>-]r<revision>` for media and firmware.
pub fn tag(recipe: &Recipe) -> String {
    if recipe.kind == Kind::Machine {
        return recipe
            .identity
            .version
            .as_ref()
            .map(|v| v.value.clone())
            .unwrap_or_default();
    }
    let revision = recipe.identity.revision.as_ref().map_or(0, |r| r.value);
    match &recipe.identity.release {
        Some(release) => format!("{}-r{revision}", release.value),
        None => format!("r{revision}"),
    }
}

/// Write the outputs into `dir`, each under the file name the artifact
/// gives it, in a folder of its own so that no two names meet.
pub fn stage(
    recipe: &Recipe,
    merged: Option<&Merged>,
    built: &Built,
    reference: &str,
    dir: &Path,
) -> io::Result<Staged> {
    let write = |folder: String, name: &str, bytes: &[u8]| -> io::Result<StagedFile> {
        if let Some(problem) = file_name_problem(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the file name {name:?} {problem}"),
            ));
        }
        let folder = dir.join(folder);
        std::fs::create_dir_all(&folder)?;
        let path = folder.join(name);
        std::fs::write(&path, bytes)?;
        Ok(StagedFile {
            path,
            size: bytes.len() as u64,
        })
    };
    let mut staged = Staged {
        kind: recipe.kind,
        reference: reference.to_owned(),
        media: Vec::new(),
        roms: Vec::new(),
        extras: Vec::new(),
        license: None,
        title: merged.map(Merged::title),
        labels: merged
            .map(|m| m.labels.iter().map(|l| l.value.clone()).collect())
            .unwrap_or_default(),
        package: None,
        min_app_version: recipe
            .identity
            .min_app_version
            .as_ref()
            .map(|v| v.value.clone()),
    };
    for (index, medium) in built.media.iter().enumerate() {
        staged.media.push(StagedMedium {
            id: medium.id.clone(),
            label: medium.label.clone(),
            format: medium.format.clone(),
            file: write(
                format!("medium-{}", index + 1),
                &medium.file_name,
                &medium.bytes,
            )?,
        });
    }
    for (index, rom) in built.roms.iter().enumerate() {
        let mut files = Vec::new();
        for (part, file) in rom.files.iter().enumerate() {
            files.push(write(
                format!("rom-{}-{}", index + 1, part + 1),
                &file.name,
                &file.bytes,
            )?);
        }
        staged.roms.push(StagedRom {
            id: rom.id.clone(),
            role: rom.role.clone(),
            catalogue_id: rom.catalogue_id.clone(),
            files,
        });
    }
    for (index, extra) in built.extras.iter().enumerate() {
        staged.extras.push(StagedExtra {
            id: extra.id.clone(),
            kind: extra.kind.clone(),
            dos_name: extra.dos_name.clone(),
            file: write(
                format!("extra-{}", index + 1),
                &extra.file_name,
                &extra.bytes,
            )?,
        });
    }
    let extra_name = |id: &Option<crate::model::At<String>>| {
        id.as_ref().and_then(|id| {
            built
                .extras
                .iter()
                .find(|e| e.id == id.value)
                .map(|e| e.file_name.clone())
        })
    };
    staged.license = recipe.license.as_ref().and_then(|license| {
        Some(StagedLicense {
            expression: license.expression.as_ref()?.value.clone(),
            terms: extra_name(&license.terms),
            source: extra_name(&license.source),
        })
    });
    staged.package = recipe.package.as_ref().map(|package| {
        let text = |v: &Option<crate::model::At<String>>| v.as_ref().map(|v| v.value.clone());
        let list = |v: &[crate::model::At<String>]| {
            (!v.is_empty()).then(|| v.iter().map(|v| v.value.clone()).collect())
        };
        StagedPackage {
            title: text(&package.title).unwrap_or_default(),
            vendor: text(&package.vendor),
            version: text(&package.version),
            date: text(&package.date),
            description: text(&package.description),
            notes: text(&package.notes),
            labels: list(&package.labels),
            boards: list(&package.boards),
            machine_classes: list(&package.machine_classes),
        }
    });
    Ok(staged)
}

/// Why `name` cannot be a staged file's name, or `None`: it must be one
/// path component on every host, so that joining it to the staging folder
/// can name nothing outside it (no separator of any host, no drive or
/// stream prefix such as `C:x`, not `.` or `..`, no NUL).
pub fn file_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("is empty");
    }
    if name == "." || name == ".." {
        return Some("names a folder");
    }
    if name.contains(['/', '\\']) {
        return Some("holds a path separator");
    }
    if name.contains(':') {
        return Some("holds a colon, a drive or stream prefix on some hosts");
    }
    if name.contains('\0') {
        return Some("holds a NUL byte");
    }
    None
}

/// The staging folder of one build, removed when the build is done, also
/// when it unwinds from a panic.
struct StagingFolder(PathBuf);

impl Drop for StagingFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How old a stray `.partial-*` download is before a build removes it.
const STALE_PARTIAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Remove what builds that did not finish left behind (commands.md,
/// Building recipes): staging folders and lost-lock scratch stores of
/// processes that no longer run, and
/// partial downloads older than a day. Errors are ignored: this is
/// housekeeping, and a folder another build still uses is never named.
fn sweep(store: &Store) {
    for folder in [STAGING, LOST_LOCK] {
        let Ok(entries) = std::fs::read_dir(store.root().join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let pid = name
                .to_str()
                .and_then(|n| n.split('-').next())
                .and_then(|n| n.parse::<u32>().ok());
            if pid.is_some_and(|pid| pid != std::process::id() && !alive(pid)) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(store.root().join(DOWNLOADS)) {
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > STALE_PARTIAL);
            if stale && entry.file_name().to_string_lossy().starts_with(".partial-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Whether a process of this id runs. Elsewhere than Unix it is taken to
/// run, and its staging folder stays until a build of that id removes it.
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists and may be
        // signalled; nothing is sent.
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// A name no other staging folder or partial download of this process
/// takes: the process id, then a counter.
fn unique(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{prefix}{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// What a build is asked to do.
pub struct Request<'a> {
    pub path: &'a Path,
    pub providers: Providers<'a>,
    pub store: &'a Store,
    /// The store's index of decoded digests, source 1 of the search.
    pub index: Option<&'a StoreIndex>,
    pub store_read: Option<&'a dyn StoreRead>,
    pub libraries: &'a mut [Library],
    /// `Some`: the user consents to downloads (`--fetch`).
    pub fetcher: Option<&'a dyn Fetcher>,
    /// Tag the result under this tag of the recipe's repository instead of
    /// the recipe's own tag (`--keep-as`).
    pub keep_as: Option<&'a str>,
    /// Look a pinned recipe's outputs up by their locked payloads before
    /// running the steps (recipes §3.4). `recipe build` does; the
    /// conformance harness does not, as it proves that the steps still make
    /// the bytes they were pinned with.
    pub find_outputs: bool,
}

/// How each input of a built artifact is kept (recipes §3.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub input: String,
    pub how: String,
}

/// A finished build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub plan: Plan,
    pub reference: String,
    pub published: Published,
    pub kept: Vec<Kept>,
    /// Attachments the recipe declares, not built in this version.
    pub attachments: Vec<String>,
    /// The recipe is pinned: the lock's manifest, config and metadata were
    /// published unchanged after every payload matched it.
    pub locked: bool,
    /// The work file's fields that differ from the lock (recipes §6.16);
    /// the lock's values were published.
    pub drift: Vec<crate::lock::Difference>,
    /// The lower-case SHA-256 of every output the steps made or found: the
    /// payloads, which a lock never holds.
    pub outputs: BTreeSet<String>,
    /// The recipe is pinned and its lock was lost: this build's writers
    /// made the pinned digest, and their files stood in for the lock
    /// (recipes §7.2).
    pub lock_rebuilt: bool,
}

/// What [`build_path`] gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The file does not pass the checks.
    Refused(crate::Report),
    /// The file is a work file.
    NotRecipe,
    /// An input the artifact needs is missing after any fetch: nothing was
    /// built or tagged.
    Missing(Box<Plan>),
    /// A fetch, a read, a step or the importer failed: nothing was tagged.
    Failed {
        plan: Box<Plan>,
        error: String,
    },
    /// A pinned recipe whose payloads do not match its lock (recipes §3.8,
    /// points 4 and 5): nothing was tagged under the recipe's tag. With
    /// `--keep-as` the result this build's writers make is kept under that
    /// tag.
    Mismatch {
        plan: Box<Plan>,
        mismatches: Vec<String>,
        kept: Option<(String, Published)>,
    },
    Built(Box<Report>),
}

/// Build the recipe at `request.path`: check, plan, fetch with consent,
/// gather, run the steps and publish through the platform's importer.
/// `log` receives the lines a person follows the build by: what will be
/// downloaded, before it is. An I/O error is a recipe or work file that
/// cannot be read.
pub fn build_path(request: Request<'_>, log: &mut dyn FnMut(&str)) -> io::Result<Outcome> {
    let (recipe, work, text) = match crate::load(request.path, request.providers)? {
        crate::Opened::Refused(report) => return Ok(Outcome::Refused(report)),
        crate::Opened::Work => return Ok(Outcome::NotRecipe),
        crate::Opened::Recipe(recipe, work, bytes) => (*recipe, work, bytes),
    };
    // Composing a machine needs a core, which only the template build has.
    if recipe.kind == Kind::Machine {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "machine recipes are built by the template build, which composes them with a core",
        ));
    }
    let file = request.path.display().to_string();
    // The digest of the recipe text this build reads, recorded on the tag
    // it writes (`unemu.build.recipe`, recipes §7.3).
    let recipe_digest = image_store::Digest::of(&text);
    let family = recipe
        .identity
        .platform
        .as_ref()
        .map(|p| p.value.clone())
        .unwrap_or_default();
    let downloads = request.store.root().join(DOWNLOADS);
    // Content the build cannot read from the store is not found there.
    let index = request.index.filter(|_| request.store_read.is_some());
    sweep(request.store);
    let failed = |plan: Plan, error: String| {
        Ok(Outcome::Failed {
            plan: Box::new(plan),
            error,
        })
    };
    let provider = request.providers.get(&family);
    // A pinned recipe's lock, opened before the plan so that the outputs it
    // names are looked for first; `None` for a draft and a lost lock.
    let lock_dir = crate::lock::dir(request.path);
    let lost = recipe.identity.digest.is_some() && crate::lock_lost(request.path);
    let locked = match &recipe.identity.digest {
        Some(digest) if !lost => Some(crate::lock::open(&digest.value, &lock_dir)),
        _ => None,
    };
    let wanted = match (&locked, provider) {
        (Some(Ok(locked)), Some(provider)) if request.find_outputs => {
            crate::lock::payloads(&recipe, locked, provider)
        }
        _ => Vec::new(),
    };
    let mut plan = crate::plan::plan_pinned(
        &recipe,
        &file,
        request.path.parent(),
        Sources {
            store: index,
            downloads: Some(&downloads),
            libraries: &mut *request.libraries,
        },
        &wanted,
    );
    plan.lock_lost = lost.then(|| lock_dir.clone());
    let Some(provider) = provider else {
        return failed(
            plan,
            format!("no provider of platform {family} in this build"),
        );
    };
    let locked = match locked.transpose() {
        Ok(locked) => locked,
        Err(error) => return failed(plan, error),
    };
    let Some(publisher) = provider.publisher() else {
        return failed(
            plan,
            format!("this build checks recipes of platform {family} but cannot build them"),
        );
    };
    if !plan.complete() {
        let Some(fetcher) = request.fetcher else {
            return Ok(Outcome::Missing(Box::new(plan)));
        };
        if let Err(error) = fetch_missing(&plan, &downloads, fetcher, log) {
            return match error {
                FetchError::Cannot => Ok(Outcome::Missing(Box::new(plan))),
                FetchError::Failed(error) => failed(plan, error),
            };
        }
        plan = crate::plan::plan_pinned(
            &recipe,
            &file,
            request.path.parent(),
            Sources {
                store: index,
                downloads: Some(&downloads),
                libraries: &mut *request.libraries,
            },
            &wanted,
        );
        plan.lock_lost = lost.then(|| lock_dir.clone());
        if !plan.complete() {
            return Ok(Outcome::Missing(Box::new(plan)));
        }
    }
    let own_tag = tag(&recipe);
    let repository = recipe
        .identity
        .name
        .as_ref()
        .map(|n| n.value.clone())
        .unwrap_or_default();
    let reference = match request.keep_as {
        None => format!("{repository}:{own_tag}"),
        Some(kept) if kept.contains([':', '/', '@']) || kept.is_empty() => {
            return failed(
                plan,
                format!("--keep-as {kept}: give a tag of the recipe's repository, such as {own_tag}-mine, without the repository"),
            )
        }
        Some(kept) if kept == own_tag => {
            return failed(
                plan,
                format!("--keep-as {kept}: that is the recipe's own tag; a kept result takes another"),
            )
        }
        Some(kept) => format!("{repository}:{kept}"),
    };
    let merged = match recipe.kind {
        Kind::Media => match crate::merge::merge(&recipe, work.as_deref()) {
            Some(merged) => Some(merged),
            None => return failed(plan, "the recipe states no title".into()),
        },
        Kind::Firmware | Kind::Machine => None,
    };
    let gathered = match gather(&recipe, &plan, request.libraries, request.store_read) {
        Ok(gathered) => gathered,
        Err(error) => return failed(plan, error),
    };
    let built = match run(&recipe, &gathered, provider) {
        Ok(built) => built,
        Err(error) => return failed(plan, error),
    };
    let outputs = outputs(&built);
    let kept = kept(&recipe, &plan, &built);
    let attachments = recipe
        .attachments
        .iter()
        .filter_map(|a| a.id.as_ref().map(|id| id.value.clone()))
        .collect();
    let mut drift = Vec::new();
    let published = match &recipe.identity.digest {
        // A draft: the outputs go through the platform's importers.
        None => match draft(
            request.store,
            publisher,
            &recipe,
            merged.as_ref(),
            &built,
            &reference,
        ) {
            Ok(published) => published,
            Err(error) => return failed(plan, error),
        },
        // Pinned: the payloads are checked against the lock and the lock's
        // bytes are published; no writer of this build is used.
        Some(digest) => {
            let locked = match locked {
                Some(locked) => locked,
                // A lost lock: this build's writers make the files, and
                // they stand in for the lock only when they give the
                // pinned digest (recipes §7.2).
                None => match rebuild_lock(
                    request.store,
                    publisher,
                    &recipe,
                    merged.as_ref(),
                    &built,
                    &reference,
                    &digest.value,
                    &outputs,
                ) {
                    Ok(locked) => locked,
                    Err(error) => {
                        return failed(
                            plan,
                            format!("{file}: the lock {} is lost; {error}", lock_dir.display()),
                        )
                    }
                },
            };
            // The checks compared the lock already; the files may have
            // changed since, so a disagreement found now refuses too.
            let comparison =
                crate::lock::compare(&recipe, merged.as_ref(), &locked, Some(provider));
            if !comparison.errors.is_empty() {
                let errors: Vec<String> =
                    comparison.errors.iter().map(ToString::to_string).collect();
                return failed(
                    plan,
                    format!("the lock disagrees with the recipe: {}", errors.join("; ")),
                );
            }
            drift = comparison.drift;
            match crate::lock::verify(&recipe, &locked, &built) {
                Ok(payloads) => {
                    let result = crate::lock::store(request.store, &locked, &payloads).and_then(
                        |manifest| publisher.tag_locked(request.store, &reference, &manifest),
                    );
                    match result {
                        Ok(published) => published,
                        Err(error) => return failed(plan, error),
                    }
                }
                Err(mismatches) => {
                    let kept = match request.keep_as {
                        Some(_) => match draft(
                            request.store,
                            publisher,
                            &recipe,
                            merged.as_ref(),
                            &built,
                            &reference,
                        ) {
                            Ok(published) => {
                                record(request.store, &reference, &published, &recipe_digest, log);
                                Some((reference, published))
                            }
                            Err(error) => return failed(plan, error),
                        },
                        None => None,
                    };
                    return Ok(Outcome::Mismatch {
                        plan: Box::new(plan),
                        mismatches,
                        kept,
                    });
                }
            }
        }
    };
    record(request.store, &reference, &published, &recipe_digest, log);
    Ok(Outcome::Built(Box::new(Report {
        plan,
        reference,
        published,
        kept,
        attachments,
        locked: recipe.identity.digest.is_some(),
        drift,
        outputs,
        lock_rebuilt: lost,
    })))
}

/// Record on the tag a build created that the recipe text of digest
/// `recipe` built it ([`image_store::BUILD_RECIPE`]); a tag that was there
/// already is left as it is. The record is metadata of the index: when it
/// cannot be written the tag stands and the log says so.
pub(crate) fn record(
    store: &Store,
    reference: &str,
    published: &Published,
    recipe: &image_store::Digest,
    log: &mut dyn FnMut(&str),
) {
    if !published.created {
        return;
    }
    if let Err(error) = store.record_build_recipe(reference, &published.descriptor.digest, recipe) {
        log(&format!(
            "warning: {reference}: the recipe that built it is not recorded: {error}"
        ));
    }
}

/// Where a build whose lock is lost makes its draft, in the store's cache,
/// one folder per build, removed when it is done.
pub const LOST_LOCK: &str = "cache/recipe-lost-lock";

/// Make the files of a lost lock with this build's writers (recipes §7.2):
/// the draft of `built` goes into a scratch store; when its manifest has
/// the pinned `digest`, its manifest, config and generated layers are the
/// lock, checked as an opened one is. Otherwise the error names both
/// digests: the recipe needs its lock.
#[allow(clippy::too_many_arguments)]
fn rebuild_lock(
    store: &Store,
    publisher: &dyn Publisher,
    recipe: &Recipe,
    merged: Option<&Merged>,
    built: &Built,
    reference: &str,
    digest: &str,
    outputs: &BTreeSet<String>,
) -> Result<crate::lock::Locked, String> {
    let folder = StagingFolder(store.root().join(LOST_LOCK).join(unique("")));
    let scratch = Store::open(&folder.0)
        .map_err(|e| format!("a scratch store in {}: {e}", folder.0.display()))?;
    let made = draft(&scratch, publisher, recipe, merged, built, reference)?;
    if made.descriptor.digest.to_string() != digest {
        return Err(format!(
            "this build makes {}, the recipe pins {digest}: the recipe needs its lock; restore it from where the recipe was published",
            made.descriptor.digest
        ));
    }
    let files = crate::lock::files_of(&scratch, &made.descriptor, outputs)?;
    crate::lock::rebuilt(digest, &files)
}

/// The lower-case SHA-256 of every output the steps made.
fn outputs(built: &Built) -> BTreeSet<String> {
    built
        .media
        .iter()
        .map(|m| &m.bytes)
        .chain(
            built
                .roms
                .iter()
                .flat_map(|r| r.files.iter().map(|f| &f.bytes)),
        )
        .chain(built.extras.iter().map(|e| &e.bytes))
        .map(|bytes| steps::sha256(bytes))
        .collect()
}

/// Stage the outputs and hand them to the platform's importers, which
/// write the config, metadata and manifest with this build's writers into
/// `store` and tag `reference` last.
fn draft(
    store: &Store,
    publisher: &dyn Publisher,
    recipe: &Recipe,
    merged: Option<&Merged>,
    built: &Built,
    reference: &str,
) -> Result<Published, String> {
    let staging = StagingFolder(store.root().join(STAGING).join(unique("")));
    stage(recipe, merged, built, reference, &staging.0)
        .map_err(|e| format!("staging the outputs in {}: {e}", staging.0.display()))
        .and_then(|staged| publisher.publish(store, &staged))
}

pub(crate) enum FetchError {
    /// No missing required file input has a URL.
    Cannot,
    Failed(String),
}

/// Download missing required file inputs with URLs into the download cache,
/// once per hash and size, after printing what will be downloaded. A download
/// is kept only when its size and SHA-256 are the pinned ones.
pub(crate) fn fetch_missing(
    plan: &Plan,
    downloads: &Path,
    fetcher: &dyn Fetcher,
    log: &mut dyn FnMut(&str),
) -> Result<(), FetchError> {
    let missing: Vec<&InputPlan> = plan
        .inputs
        .iter()
        .filter(|i| !i.optional && matches!(i.status, Status::Missing(_)))
        .collect();
    let mut wanted = Vec::new();
    let mut seen = BTreeSet::new();
    for input in &missing {
        match &input.form {
            Form::File(value) if !input.urls.is_empty() => {
                if seen.insert((&value.sha256, value.size)) {
                    wanted.push((*input, value));
                }
            }
            _ => {
                log(&format!(
                    "fetch: input {} has no URL; will look for it by hash after downloads",
                    input.id
                ));
            }
        }
    }
    if wanted.is_empty() {
        log("fetch: nothing could be downloaded: no missing required file input has a URL");
        return Err(FetchError::Cannot);
    }
    let total: u64 = wanted.iter().map(|(_, v)| v.size).sum();
    log(&format!(
        "fetch: downloading {} input{} ({total} bytes), as --fetch allows:",
        wanted.len(),
        if wanted.len() == 1 { "" } else { "s" }
    ));
    for (input, value) in &wanted {
        log(&format!(
            "  {} ({}): {} bytes, sha256 {}, from {}",
            input.id,
            input.name,
            value.size,
            value.sha256,
            input.urls.join(" or ")
        ));
    }
    std::fs::create_dir_all(downloads).map_err(|e| {
        FetchError::Failed(format!("the download cache {}: {e}", downloads.display()))
    })?;
    for (input, value) in wanted {
        let target = downloads.join(&value.sha256);
        // Each download has a partial file of its own, named by the process
        // and a counter, so builds that fetch the same input at once, in
        // this process or another, never write one file; whichever renames
        // last leaves the same verified bytes in the cache.
        let partial = downloads.join(unique(&format!(".partial-{}-", value.sha256)));
        let mut errors = Vec::new();
        let mut done = false;
        for url in &input.urls {
            match download(fetcher, url, value.size, &value.sha256, &partial) {
                Ok(()) => {
                    std::fs::rename(&partial, &target)
                        .map_err(|e| FetchError::Failed(format!("{}: {e}", target.display())))?;
                    log(&format!("  {}: fetched from {url}", input.id));
                    done = true;
                    break;
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&partial);
                    log(&format!("  {}: {url}: {error}", input.id));
                    errors.push(format!("{url}: {error}"));
                }
            }
        }
        if !done {
            return Err(FetchError::Failed(format!(
                "input {} ({}) could not be fetched: {}",
                input.id,
                input.name,
                errors.join("; ")
            )));
        }
    }
    Ok(())
}

/// One download into `partial`, checked against the pinned size and hash.
fn download(
    fetcher: &dyn Fetcher,
    url: &str,
    size: u64,
    sha256: &str,
    partial: &Path,
) -> Result<(), String> {
    use sha2::Digest as _;
    struct Checked<W> {
        out: W,
        sha: sha2::Sha256,
        written: u64,
        limit: u64,
    }
    impl<W: Write> Write for Checked<W> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.written += bytes.len() as u64;
            if self.written > self.limit {
                return Err(io::Error::other(format!(
                    "more than the pinned {} bytes",
                    self.limit
                )));
            }
            self.sha.update(bytes);
            self.out.write_all(bytes)?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.out.flush()
        }
    }
    let file = std::fs::File::create(partial).map_err(|e| e.to_string())?;
    let mut checked = Checked {
        out: io::BufWriter::new(file),
        sha: sha2::Sha256::new(),
        written: 0,
        limit: size,
    };
    fetcher.fetch(url, size, &mut checked)?;
    checked.flush().map_err(|e| e.to_string())?;
    if checked.written != size {
        return Err(format!("{} bytes, not the pinned {size}", checked.written));
    }
    let got = format!("{:x}", checked.sha.finalize());
    if got != sha256 {
        return Err(format!("sha256 {got}, not the pinned {sha256}"));
    }
    let file = checked
        .out
        .into_inner()
        .map_err(|e| e.error().to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

/// How each input the plan saw is kept in the artifact (recipes §3.10), and
/// in which of its files, by the names `image export` writes them under:
/// `kept as stored in NAME` when its bytes are a layer unchanged; `kept as
/// files in NAME` when a `fat` step built them into the volumes of these
/// media, whose files `image export --files` writes back with the recipe's
/// names and times; `kept as text with DOS line ends in NAME` after a
/// `text` step. A way through an `unpack` step keeps the archive's members
/// (`members kept as …`); unless the archive is also kept as stored, `not
/// kept: the archive itself` follows, with `N members unused` for the file
/// members the recipe did not take. An input the plan did not need says
/// why.
fn kept(recipe: &Recipe, plan: &Plan, built: &Built) -> Vec<Kept> {
    /// How an input reaches an output (the way, and whether through an
    /// `unpack` step, so as its members) and the outputs' names, in output
    /// order.
    type Ways = BTreeMap<(&'static str, bool), Vec<String>>;
    let mut how: BTreeMap<String, Ways> = BTreeMap::new();
    let steps: BTreeMap<&str, &Op> = recipe
        .steps
        .iter()
        .filter_map(|s| Some((s.id.as_ref()?.value.as_str(), &s.op)))
        .collect();
    struct Trace<'s, 'r> {
        steps: &'s BTreeMap<&'r str, &'r Op>,
        how: &'s mut BTreeMap<String, Ways>,
    }
    impl Trace<'_, '_> {
        fn trace(
            &mut self,
            reference: &str,
            mut through: &'static str,
            mut members: bool,
            out: &str,
            depth: usize,
        ) {
            let name = reference.split('/').next().unwrap_or(reference);
            let Some(op) = self.steps.get(name) else {
                let names = self
                    .how
                    .entry(name.to_owned())
                    .or_default()
                    .entry((through, members))
                    .or_default();
                if !names.iter().any(|n| n == out) {
                    names.push(out.to_owned());
                }
                return;
            };
            if depth > 64 {
                return;
            }
            let operand = match op {
                Op::Unpack { archive, .. } => {
                    members = true;
                    archive.as_ref()
                }
                Op::Fat {
                    tree, placements, ..
                } => {
                    through = "kept as files";
                    for from in placements.iter().flatten().filter_map(|p| p.from.as_ref()) {
                        self.trace(&from.value, through, members, out, depth + 1);
                    }
                    tree.as_ref()
                }
                Op::Text { text } => {
                    through = "kept as text with DOS line ends";
                    text.as_ref()
                }
                Op::Unknown { .. } => None,
            };
            if let Some(operand) = operand {
                self.trace(&operand.value.clone(), through, members, out, depth + 1);
            }
        }
    }
    let mut tracer = Trace {
        steps: &steps,
        how: &mut how,
    };
    let text = |v: &Option<crate::model::At<String>>| v.as_ref().map(|v| v.value.clone());
    // Each output's reference and the name its file is exported under.
    let mut outputs: Vec<(String, String)> = Vec::new();
    for (medium, made) in recipe.media.iter().zip(&built.media) {
        outputs.extend(text(&medium.from).map(|from| (from, made.file_name.clone())));
    }
    for (rom, made) in recipe.roms.iter().zip(&built.roms) {
        let references: Vec<String> = match &rom.source {
            RomSource::File(from) => text(from).into_iter().collect(),
            RomSource::Parts { parts, .. } => parts.iter().filter_map(|p| text(&p.from)).collect(),
        };
        for (reference, file) in references.into_iter().zip(&made.files) {
            outputs.push((reference, file.name.clone()));
        }
    }
    for (extra, made) in recipe.extras.iter().zip(&built.extras) {
        outputs.extend(text(&extra.from).map(|from| (from, made.file_name.clone())));
    }
    for (reference, out) in &outputs {
        tracer.trace(reference, "kept as stored", false, out, 0);
    }
    plan.inputs
        .iter()
        .filter(|input| !input.optional)
        .map(|input| {
            let empty = Ways::new();
            let ways = how.get(&input.id).unwrap_or(&empty);
            let mut parts: Vec<String> = match &input.status {
                Status::NotNeeded(why) => vec![format!("not needed: {why}")],
                _ => ways
                    .iter()
                    .map(|((way, members), names)| {
                        let members = if *members { "members " } else { "" };
                        format!("{members}{way} in {}", names.join(", "))
                    })
                    .collect(),
            };
            let unpacked = ways.keys().any(|&(_, members)| members);
            let itself = ways.contains_key(&("kept as stored", false));
            if !matches!(input.status, Status::NotNeeded(_)) && unpacked && !itself {
                let mut lost = vec!["the archive itself".to_owned()];
                if let Some(&unused) = built.unused_members.get(&input.id) {
                    if unused > 0 {
                        lost.push(format!(
                            "{unused} member{} unused",
                            if unused == 1 { "" } else { "s" }
                        ));
                    }
                }
                parts.push(format!("not kept: {}", lost.join(", ")));
            }
            Kept {
                input: input.id.clone(),
                how: parts.join("; "),
            }
        })
        .collect()
}

impl Report {
    /// The text `unemu recipe build` prints after a build.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for kept in &self.kept {
            out.push_str(&format!("  {}: {}\n", kept.input, kept.how));
        }
        for payload in &self.plan.payloads {
            if let Some(found) = &payload.value.found {
                out.push_str(&format!("  {}: found {found}, not made\n", payload.what()));
            }
        }
        if self.lock_rebuilt {
            out.push_str(&format!(
                "  lock: lost; this build's writers made the pinned digest {}\n",
                self.published.descriptor.digest
            ));
        }
        for attachment in &self.attachments {
            out.push_str(&format!(
                "  attachment {attachment}: not built; this version builds no attachments\n"
            ));
        }
        for drift in &self.drift {
            out.push_str(&format!(
                "  drift: {drift}; the lock's value was published, a new revision takes the change up\n"
            ));
        }
        out.push_str(&format!(
            "{} {}\t{}\t{}\n",
            if self.published.created {
                "built"
            } else {
                "already built"
            },
            self.reference,
            self.published.descriptor.digest,
            if self.locked { "locked" } else { "draft" }
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Document;
    use crate::provider::{
        CatalogueEntry, FirmwareProfile, MediaProfile, VolumeFile, VolumeProfile,
    };

    /// A provider whose volumes are a listing of their files: enough to
    /// see what the `fat` step hands the profile.
    struct Listing;
    impl Provider for Listing {
        fn platform(&self) -> &str {
            "test"
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
    impl MediaProfile for Listing {
        fn formats(&self) -> &[&str] {
            &["disk"]
        }
        fn size_fault(&self, _: &str, _: u64) -> Option<String> {
            None
        }
        fn volume_profile(&self, _: &str) -> Option<&dyn VolumeProfile> {
            Some(self)
        }
        fn dos_name_fault(&self, _: &str) -> Option<String> {
            None
        }
    }
    impl VolumeProfile for Listing {
        fn path_fault(&self, _: &str) -> Option<String> {
            None
        }
        fn label_fault(&self, _: &str) -> Option<String> {
            None
        }
        fn build(
            &self,
            files: &[VolumeFile<'_>],
            keys: &VolumeKeys,
        ) -> Result<Vec<Vec<u8>>, String> {
            let mut image = format!("{keys:?}\n").into_bytes();
            for file in files {
                image.extend_from_slice(format!("{} {:?} ", file.path, file.time).as_bytes());
                image.extend_from_slice(file.bytes);
                image.push(b'\n');
            }
            Ok(vec![image])
        }
    }
    impl FirmwareProfile for Listing {
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

    fn recipe(text: &str) -> Recipe {
        match crate::parse_text(text) {
            Some(Document::Recipe(recipe)) => *recipe,
            _ => panic!("not a recipe:\n{text}"),
        }
    }

    const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn bytes(b: &[u8]) -> Bytes {
        Arc::new(b.to_vec())
    }

    #[test]
    fn machine_template_version_is_the_output_tag() {
        let text = "recipe = \"1.1\"\n[machine]\nname = \"unemu/machines/ibm-pc/open-at\"\nplatform = \"ibm-pc\"\nversion = \"1.3.0\"\n[template]\nconfig = \"config\"\n";
        let crate::model::Document::Recipe(recipe) = crate::parse_text(text).unwrap() else {
            panic!("expected a recipe");
        };
        assert_eq!(tag(&recipe), "1.3.0");
        assert_eq!(
            crate::artifact::output(&recipe),
            "unemu/machines/ibm-pc/open-at:1.3.0"
        );
    }

    /// The steps run on the gathered bytes alone: the same bytes give the
    /// same outputs, the members found stand for the archive, the file
    /// set's stated times reach the volume and the text gets DOS line ends.
    #[test]
    fn running_the_steps_is_a_pure_function_of_the_gathered_bytes() {
        let recipe = recipe(&format!(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/test/x\"\nplatform = \"test\"\nrevision = 1\n\n[title]\ntitle = \"X\"\n\n\
             [input.archive]\nname = \"X.ZIP\"\nsize = 1\nsha256 = \"{ZERO}\"\n\n\
             [input.game]\nname = \"GAME\"\nfiles = [{{ path = \"RUN.COM\", size = 3, sha256 = \"{ZERO}\", time = 1999-12-31T23:59:58 }}]\n\n\
             [input.notes]\nname = \"notes\"\nsize = 1\nsha256 = \"{ZERO}\"\n\n\
             [[step]]\nid = \"files\"\nunpack = \"archive\"\nformat = \"zip\"\nmembers = [{{ path = \"A.EXE\", size = 1, sha256 = \"{ZERO}\" }}]\n\n\
             [[step]]\nid = \"disk\"\nfat = \"game\"\nformat = \"disk\"\nlabel = \"L\"\norder = \"listed\"\n\n\
             [[step]]\nid = \"readme\"\ntext = \"notes\"\neol = \"crlf\"\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk\"\nformat = \"disk\"\nfrom = \"disk\"\n\n\
             [[extra]]\nid = \"a\"\nfile_name = \"A.EXE\"\nfrom = \"files/A.EXE\"\n\n\
             [[extra]]\nid = \"readme\"\nfile_name = \"README.TXT\"\nfrom = \"readme\"\n"
        ));
        let time = crate::model::LocalTime {
            year: 1999,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 58,
        };
        let gathered = Gathered {
            inputs: BTreeMap::from([
                (
                    "game".to_owned(),
                    Value::Tree(vec![TreeFile {
                        path: "RUN.COM".into(),
                        bytes: bytes(b"run"),
                        time: Some(time),
                    }]),
                ),
                ("notes".to_owned(), Value::File(bytes(b"a\nb\n"))),
            ]),
            members: BTreeMap::from([(("files".to_owned(), "A.EXE".to_owned()), bytes(b"a"))]),
            outputs: BTreeMap::new(),
        };
        let first = run(&recipe, &gathered, &Listing).unwrap();
        assert_eq!(first, run(&recipe, &gathered, &Listing).unwrap());
        assert_eq!(first.media[0].file_name, "disk-1");
        assert_eq!(
            String::from_utf8(first.media[0].bytes.to_vec()).unwrap(),
            format!(
                "{:?}\nRUN.COM {:?} run\n",
                VolumeKeys {
                    label: Some("L".into()),
                    listed: true,
                    ..VolumeKeys::default()
                },
                Some(time)
            )
        );
        assert_eq!(first.extras[0].bytes.as_slice(), b"a");
        assert_eq!(first.extras[1].bytes.as_slice(), b"a\r\nb\r\n");
        assert!(
            first.unused_members.is_empty(),
            "the archive was not opened"
        );
    }

    /// Two fetches of one missing input at once, as two builds sharing a
    /// store make them: each writes a partial file of its own, both finish,
    /// and the cache holds the pinned bytes and no partial file.
    #[test]
    fn two_fetches_of_one_input_at_once_do_not_share_a_partial_file() {
        use std::sync::Barrier;
        let bytes = b"downloaded bytes".to_vec();
        let recipe = recipe(&format!(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/test/x\"\nplatform = \"test\"\nrevision = 1\n\n[title]\ntitle = \"X\"\n\n\
             [input.disk]\nname = \"DISK.IMG\"\nsize = {}\nsha256 = \"{}\"\nurls = [\"https://example.org/DISK.IMG\"]\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk\"\nformat = \"disk\"\nfrom = \"disk\"\n",
            bytes.len(),
            steps::sha256(&bytes)
        ));
        let plan = crate::plan::plan(
            &recipe,
            "x.media.toml",
            None,
            Sources {
                store: None,
                downloads: None,
                libraries: &mut [],
            },
        );
        assert!(!plan.complete());
        struct Halves<'a> {
            bytes: &'a [u8],
            barrier: &'a Barrier,
        }
        impl Fetcher for Halves<'_> {
            fn fetch(&self, _: &str, _: u64, sink: &mut dyn Write) -> Result<(), String> {
                let half = self.bytes.len() / 2;
                sink.write_all(&self.bytes[..half])
                    .map_err(|e| e.to_string())?;
                sink.flush().map_err(|e| e.to_string())?;
                // Both downloads are under way before either finishes.
                self.barrier.wait();
                sink.write_all(&self.bytes[half..])
                    .map_err(|e| e.to_string())
            }
        }
        let dir = std::env::temp_dir().join(format!("recipe-fetch-twice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let barrier = Barrier::new(2);
        let fetcher = Halves {
            bytes: &bytes,
            barrier: &barrier,
        };
        let results: Vec<bool> = std::thread::scope(|scope| {
            let runs: Vec<_> = (0..2)
                .map(|_| scope.spawn(|| fetch_missing(&plan, &dir, &fetcher, &mut |_| {}).is_ok()))
                .collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect()
        });
        assert_eq!(results, [true, true]);
        assert_eq!(
            std::fs::read(dir.join(steps::sha256(&bytes))).unwrap(),
            bytes
        );
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, [steps::sha256(&bytes)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A staged file's name is one path component on every host.
    #[test]
    fn a_staged_file_name_cannot_leave_the_staging_folder() {
        for (name, problem) in [
            ("", "is empty"),
            ("..", "names a folder"),
            ("../x", "holds a path separator"),
            ("..\\x", "holds a path separator"),
            (
                "C:x",
                "holds a colon, a drive or stream prefix on some hosts",
            ),
            (
                "A.TXT:stream",
                "holds a colon, a drive or stream prefix on some hosts",
            ),
        ] {
            assert_eq!(file_name_problem(name), Some(problem), "{name:?}");
        }
        assert_eq!(file_name_problem("x86BOOT.img"), None);
        let recipe = recipe(&format!(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/test/x\"\nplatform = \"test\"\nrevision = 1\n\n[title]\ntitle = \"X\"\n\n\
             [input.disk]\nname = \"D\"\nsize = 1\nsha256 = \"{ZERO}\"\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk\"\nformat = \"disk\"\nfrom = \"disk\"\n"
        ));
        let dir = std::env::temp_dir().join(format!("recipe-stage-{}", std::process::id()));
        let built = Built {
            media: vec![BuiltMedium {
                id: "disk-1".into(),
                label: "Disk".into(),
                format: "disk".into(),
                file_name: "C:..\\..\\escape.img".into(),
                bytes: bytes(b"x"),
            }],
            ..Built::default()
        };
        let error = stage(&recipe, None, &built, "x:r1", &dir).unwrap_err();
        assert!(
            error.to_string().contains("holds a path separator"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The step code and the runner name no clock, network, host file,
    /// environment, process or thread interface: a step can reach none of
    /// them, only the bytes and keys it is given.
    #[test]
    fn the_steps_reach_no_clock_network_or_host() {
        let steps = include_str!("steps.rs");
        let steps = &steps[..steps.find("#[cfg(test)]").unwrap()];
        let build = include_str!("build.rs");
        let runner = &build[build.find("pub fn run(").unwrap()
            ..build.find("/// The tag a recipe's output takes").unwrap()];
        for (name, source) in [("steps.rs", steps), ("build.rs run", runner)] {
            for forbidden in [
                "std::time",
                "Instant",
                "SystemTime",
                "std::net",
                "std::fs",
                "std::env",
                "std::process",
                "std::thread",
                "Fetcher",
                "StoreRead",
                "Library",
            ] {
                assert!(!source.contains(forbidden), "{name} names {forbidden}");
            }
        }
        // The zip reader the `unpack` step opens an archive with: no clock,
        // network, environment, process or thread anywhere in it, and no
        // file in the parts an archive held in memory goes through (the
        // directory, the decoder and `Archive`); its file functions serve
        // the libraries' scan, before any step.
        let zip = include_str!("zip.rs");
        let zip = &zip[..zip.find("#[cfg(test)]").unwrap()];
        for forbidden in [
            "std::time",
            "Instant",
            "SystemTime",
            "std::net",
            "std::env",
            "std::process",
            "std::thread",
        ] {
            assert!(!zip.contains(forbidden), "zip.rs names {forbidden}");
        }
        let in_memory = [
            &zip[zip.find("fn directory<").unwrap()
                ..zip.find("/// The SHA-256 of a member's bytes").unwrap()],
            &zip[zip.find("fn decode<").unwrap()..],
        ];
        for part in in_memory {
            for forbidden in ["File", "fs::", "Path"] {
                assert!(
                    !part.contains(forbidden),
                    "zip.rs's in-memory reading names {forbidden}"
                );
            }
        }
    }
}
