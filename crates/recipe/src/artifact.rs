// SPDX-License-Identifier: AGPL-3.0-or-later
//! Recipes as registry artifacts (recipes §7): the bundle of a
//! pinned recipe, the codec that reads it back, the guard that decides what
//! may be published, and the plan of a registry row from the manifest and
//! config alone.
//!
//! **The artifact** (§7.1). The manifest's `artifactType` is
//! [`media_type::RECIPE_MEDIA`] or [`media_type::RECIPE_FIRMWARE`], one per
//! kind. Its config is the recipe file itself ([`media_type::RECIPE_CONFIG`]),
//! so the config digest is the recipe's digest. Its layers, in this order:
//! the work file the recipe names ([`media_type::RECIPE_WORK`]), each
//! bundled input in recipe order ([`media_type::RECIPE_INPUT`]) and each
//! file of the lock in name order ([`media_type::RECIPE_LOCK`]). Every
//! descriptor, the config's too, carries its file name in
//! `org.opencontainers.image.title`, as the folder beside the recipe holds
//! it: `NAME.media.toml`, `WORK.work.toml`, the input's `name`, and
//! `NAME.media.toml.lock/sha256/<hex>` for a lock file. A pull writes each
//! part at its title and gets back the folder the recipe was pinned in, so
//! the pulled recipe plans and builds with no other file. Every part is at
//! most 1 MiB. The manifest's annotations are `unemu.platform`,
//! `unemu.recipe.output` (`name:tag`), `unemu.recipe.output.digest`, the
//! title and description for registry web pages, independent core and front-end
//! minimum releases ([`crate::compat`]), and
//! `org.opencontainers.image.licenses = "CC0-1.0"`.
//!
//! **Reading** fails closed ([`Artifact::from_parts`], [`inspect`]): the
//! envelope's types and sizes, every part's digest, the titles, and then
//! the parts against the recipe: the work file it names, the bundled inputs
//! it lists with their hashes and sizes and the files of its lock, nothing
//! more and nothing less; finally the static checks of the recipe laid out
//! with its parts, rule 8 included, which opens the lock.
//!
//! **The guard** ([`guard`], recipes §6.13, §7.3)
//! adds what publishing needs: a bundled input is text; a recipe without
//! `[license]` names no file under 256 bytes unless the entry says
//! `disclosable = true`; the repository is the publisher's namespace,
//! `recipes/` and the output's path after its first component, and the tag
//! is the output's; the annotations say what the recipe says; the marker is
//! at least what this release computes. That a tag is replaced only by a
//! recipe pinning the same output digest is checked against the registry by
//! the client (`image_registry::recipe`).

use crate::model::{Document, InputForm, Kind, Recipe};
use crate::plan::{Plan, Sources};
use crate::{lock, Providers, Report};
use image_store::compat::{self, AppVersion, Levels, Requirement, Requirements};
use image_store::{media_type, Descriptor, Digest, Manifest};
use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// The descriptor annotation naming a part's file.
pub const TITLE: &str = "org.opencontainers.image.title";
/// The manifest annotation with the output's description.
pub const DESCRIPTION: &str = "org.opencontainers.image.description";
/// The manifest annotation with the artifact's licence.
pub const LICENSES: &str = "org.opencontainers.image.licenses";
/// The recipe artifact's licence: a recipe, its bundled texts and its lock
/// are our own text (recipes §6.6).
pub const LICENSE: &str = "CC0-1.0";
/// The manifest annotation naming the output, `name:tag`.
pub const OUTPUT: &str = "unemu.recipe.output";
/// The manifest annotation with the output's pinned manifest digest.
pub const OUTPUT_DIGEST: &str = "unemu.recipe.output.digest";
/// The manifest annotation naming the platform.
pub const PLATFORM: &str = "unemu.platform";

/// The artifact type of a recipe of `kind`.
pub fn artifact_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Media => media_type::RECIPE_MEDIA,
        Kind::Firmware => media_type::RECIPE_FIRMWARE,
    }
}

/// The kind of a recipe artifact type; `None` for any other type.
pub fn kind_of(artifact_type: &str) -> Option<Kind> {
    match artifact_type {
        media_type::RECIPE_MEDIA => Some(Kind::Media),
        media_type::RECIPE_FIRMWARE => Some(Kind::Firmware),
        _ => None,
    }
}

/// The file name a recipe of `kind` has: `NAME.media.toml` or
/// `NAME.firmware.toml`.
fn suffix(kind: Kind) -> &'static str {
    match kind {
        Kind::Media => ".media.toml",
        Kind::Firmware => ".firmware.toml",
    }
}

/// What a recipe artifact or its push was refused for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The recipe does not pass its checks; the rendered report.
    Findings(String),
    /// A recipe without `digest`: it has no output digest to publish.
    Draft { file: String },
    /// A bundled input beside the recipe that is not the file the recipe
    /// names: another size or hash.
    Bundled {
        file: String,
        size: u64,
        sha256: String,
    },
    /// A file that cannot be read.
    Unreadable(String),
    /// The artifact is not a recipe artifact this release reads: its
    /// envelope, a digest, a title or another structural rule.
    Codec(String),
    /// A part that is none of the recipe's work file, bundled inputs and
    /// lock files.
    ExtraPart { title: String, media_type: String },
    /// A part the recipe names that the artifact lacks.
    MissingPart { title: String },
    /// A bundled input that is not text.
    NotText { title: String },
    /// A recipe without `[license]` that names a file under 256 bytes not
    /// marked `disclosable` (recipes §6.13).
    Disclosable { input: String, size: u64 },
    /// The target repository or tag is not the one the recipe's output
    /// gives it (recipes §7.3).
    Name { target: String, wanted: String },
    /// The manifest's marker is missing or below what the tables compute.
    MinAppVersion {
        declared: String,
        required: AppVersion,
    },
    /// An annotation that does not say what the recipe says.
    Annotation {
        key: &'static str,
        found: Option<String>,
        wanted: String,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Findings(report) => write!(f, "the recipe does not pass its checks:\n{report}"),
            Self::Draft { file } => write!(
                f,
                "{file} is a draft, without digest; pin it first (unemu recipe pin), as a recipe artifact names its output's digest"
            ),
            Self::Bundled { file, size, sha256 } => write!(
                f,
                "{file}: not the {size} bytes of sha256 {sha256} the recipe names for its bundled input"
            ),
            Self::Unreadable(why) => f.write_str(why),
            Self::Codec(why) => write!(f, "not a recipe artifact this release reads: {why}"),
            Self::ExtraPart { title, media_type } => write!(
                f,
                "the artifact holds {title} ({media_type}), which is none of the recipe's work file, bundled inputs and lock files; a recipe artifact holds those and nothing else"
            ),
            Self::MissingPart { title } => {
                write!(f, "the recipe names {title}, which the artifact does not hold")
            }
            Self::NotText { title } => write!(
                f,
                "the bundled input {title} is not text (UTF-8 without NUL); only our own text is bundled"
            ),
            Self::Disclosable { input, size } => write!(
                f,
                "the recipe has no [license] and names {input} of {size} bytes, under {}: its hash gives the file away; say disclosable = true on the entry if it may be published all the same",
                crate::DISCLOSABLE_BELOW
            ),
            Self::Name { target, wanted } => write!(
                f,
                "{target} is not where this recipe is published: its output names it {wanted} (the publisher's namespace, recipes/, the output's path after its first component and the output's tag)"
            ),
            Self::MinAppVersion { declared, required } => write!(
                f,
                "the recipe needs {} {required} or newer to build by this release's tables, but its {} declares {declared}; bundle it again with this release",
                compat::PROGRAM,
                compat::MIN_APP_VERSION
            ),
            Self::Annotation { key, found, wanted } => write!(
                f,
                "the annotation {key} is {}, the recipe gives {wanted:?}",
                found.as_ref().map_or("missing".to_owned(), |v| format!("{v:?}"))
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// A recipe artifact held in memory; every part is at most 1 MiB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub manifest: Manifest,
    /// The manifest's canonical bytes, whose SHA-256 is [`Self::digest`].
    pub manifest_bytes: Vec<u8>,
    pub digest: Digest,
    /// The recipe file.
    pub config: Vec<u8>,
    /// Each layer's bytes, in the manifest's order.
    pub layers: Vec<Vec<u8>>,
}

/// What a layer of a recipe artifact is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Work,
    Input,
    Lock,
}

impl Role {
    fn of(media_type: &str) -> Option<Self> {
        match media_type {
            media_type::RECIPE_WORK => Some(Self::Work),
            media_type::RECIPE_INPUT => Some(Self::Input),
            media_type::RECIPE_LOCK => Some(Self::Lock),
            _ => None,
        }
    }
}

fn title(descriptor: &Descriptor) -> Option<&str> {
    descriptor.annotations.get(TITLE).map(String::as_str)
}

/// Why `name` cannot be a part's file name: one path component on every
/// host ([`crate::build::file_name_problem`]), not hidden, at most 255
/// bytes.
fn name_problem(name: &str) -> Option<&'static str> {
    crate::build::file_name_problem(name).or(if name.starts_with('.') {
        Some("is hidden")
    } else if name.len() > 255 {
        Some("is longer than 255 bytes")
    } else {
        None
    })
}

impl Artifact {
    /// The manifest's descriptor.
    pub fn descriptor(&self) -> Descriptor {
        Descriptor::new(
            media_type::MANIFEST,
            self.digest.clone(),
            self.manifest_bytes.len() as u64,
        )
    }

    /// The recipe's file name, its config's title.
    pub fn recipe_file(&self) -> &str {
        title(&self.manifest.config).unwrap_or_default()
    }

    /// The output, `name:tag`, as the manifest names it.
    pub fn output(&self) -> Option<&str> {
        self.manifest.annotations.get(OUTPUT).map(String::as_str)
    }

    /// The output's pinned digest, as the manifest names it.
    pub fn output_digest(&self) -> Option<&str> {
        self.manifest
            .annotations
            .get(OUTPUT_DIGEST)
            .map(String::as_str)
    }

    /// Each layer with its bytes.
    pub fn parts(&self) -> impl Iterator<Item = (&Descriptor, &[u8])> {
        self.manifest
            .layers
            .iter()
            .zip(self.layers.iter().map(Vec::as_slice))
    }

    /// Read an artifact from its manifest bytes, config bytes and layer
    /// bytes in the manifest's order, as a registry served them: the
    /// envelope strictly, every part's digest and size, and the titles
    /// ([`structure`]). Nothing is written.
    pub fn from_parts(
        manifest_bytes: Vec<u8>,
        config: Vec<u8>,
        layers: Vec<Vec<u8>>,
    ) -> Result<Self, Refusal> {
        Self::from_parts_with_levels(manifest_bytes, config, layers, historical_levels())
    }

    pub fn from_parts_with_levels(
        manifest_bytes: Vec<u8>,
        config: Vec<u8>,
        layers: Vec<Vec<u8>>,
        levels: Levels,
    ) -> Result<Self, Refusal> {
        let codec = |e: image_store::Error| Refusal::Codec(e.to_string());
        Requirements::read(&manifest_bytes)
            .check("the recipe artifact", levels)
            .map_err(codec)?;
        let manifest = Manifest::decode(&manifest_bytes).map_err(codec)?;
        if !image_store::is_recipe(manifest.artifact_type()) {
            return Err(Refusal::Codec(format!(
                "its artifact type is {}",
                manifest.artifact_type()
            )));
        }
        if layers.len() != manifest.layers.len() {
            return Err(Refusal::Codec("a layer's bytes are missing".into()));
        }
        let matches = |descriptor: &Descriptor, bytes: &[u8]| {
            if bytes.len() as u64 != descriptor.size || Digest::of(bytes) != descriptor.digest {
                Err(Refusal::Codec(format!(
                    "the bytes of {} are not the ones its descriptor names",
                    title(descriptor).unwrap_or(&descriptor.digest.to_string())
                )))
            } else {
                Ok(())
            }
        };
        matches(&manifest.config, &config)?;
        for (descriptor, bytes) in manifest.layers.iter().zip(&layers) {
            matches(descriptor, bytes)?;
        }
        structure(&manifest)?;
        Ok(Self {
            digest: Digest::of(&manifest_bytes),
            manifest,
            manifest_bytes,
            config,
            layers,
        })
    }

    /// Write every part at its title under `dir`, the folder the recipe was
    /// pinned in, and give the recipe file's path. The titles are checked
    /// first ([`structure`]). A file already there with the same bytes is
    /// kept; one with other bytes, a folder at a file's place, or a
    /// symbolic link at any path below `dir` that a part goes through, is
    /// refused before anything is written. When a write fails, what this
    /// call wrote is removed again, so `dir` is as it was.
    pub fn unpack(&self, dir: &Path) -> io::Result<PathBuf> {
        let invalid = |why: String| io::Error::new(io::ErrorKind::InvalidData, why);
        structure(&self.manifest).map_err(|e| invalid(e.to_string()))?;
        let titled = std::iter::once((self.recipe_file(), self.config.as_slice())).chain(
            self.parts()
                .map(|(d, bytes)| (title(d).unwrap_or_default(), bytes)),
        );
        let mut write = Vec::new();
        for (title, bytes) in titled {
            let path = dir.join(title);
            // Every component below `dir`, the file's own included.
            let mut at = dir.to_path_buf();
            for component in Path::new(title).components() {
                at.push(component);
                match std::fs::symlink_metadata(&at) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(invalid(format!(
                            "{} is a symbolic link; a pull writes no file through one",
                            at.display()
                        )))
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                    Err(e) => {
                        return Err(io::Error::new(e.kind(), format!("{}: {e}", at.display())))
                    }
                }
            }
            match std::fs::symlink_metadata(&path) {
                Ok(meta) => {
                    let same = meta.is_file()
                        && meta.len() == bytes.len() as u64
                        && std::fs::read(&path).is_ok_and(|existing| existing == bytes);
                    if !same {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "{} is there with other bytes; pull into another folder",
                                path.display()
                            ),
                        ));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => write.push((path, bytes)),
                Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
            }
        }
        let mut made: Vec<PathBuf> = Vec::new();
        let mut folders: Vec<PathBuf> = Vec::new();
        let result = (|| -> io::Result<()> {
            for (path, bytes) in &write {
                if let Some(parent) = path.parent() {
                    let mut missing = Vec::new();
                    let mut up = parent;
                    while !up.exists() {
                        missing.push(up.to_path_buf());
                        match up.parent() {
                            Some(next) => up = next,
                            None => break,
                        }
                    }
                    for folder in missing.into_iter().rev() {
                        std::fs::create_dir(&folder)?;
                        folders.push(folder);
                    }
                }
                let mut staged = path.as_os_str().to_owned();
                staged.push(format!(".pull-{}", std::process::id()));
                let staged = PathBuf::from(staged);
                made.push(staged.clone());
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&staged)?;
                io::Write::write_all(&mut file, bytes)?;
                file.sync_all()?;
                std::fs::rename(&staged, path)?;
                made.pop();
                made.push(path.clone());
            }
            Ok(())
        })();
        if let Err(error) = result {
            for file in made.iter().rev() {
                let _ = std::fs::remove_file(file);
            }
            for folder in folders.iter().rev() {
                let _ = std::fs::remove_dir(folder);
            }
            return Err(error);
        }
        Ok(dir.join(self.recipe_file()))
    }

    /// The bundled inputs the manifest lists, `(name, size, sha256)`.
    pub fn listed(&self) -> Vec<(String, u64, String)> {
        listed(&self.manifest)
    }
}

/// The bundled inputs a recipe manifest lists, `(name, size, sha256)`,
/// read from its descriptors alone.
pub fn listed(manifest: &Manifest) -> Vec<(String, u64, String)> {
    manifest
        .layers
        .iter()
        .filter(|d| d.media_type == media_type::RECIPE_INPUT)
        .filter_map(|d| Some((title(d)?.to_owned(), d.size, d.digest.hex().to_owned())))
        .collect()
}

/// The structural rules of a recipe manifest: its config and every layer
/// have a title that is a file name; the recipe's is `NAME.media.toml` or
/// `NAME.firmware.toml` by the artifact's kind; at most one work file,
/// named `WORK.work.toml`; a lock file at `NAME.lock/sha256/<its hex>`;
/// no title twice.
pub fn structure(manifest: &Manifest) -> Result<(), Refusal> {
    let codec = |why: String| Err(Refusal::Codec(why));
    let Some(kind) = kind_of(manifest.artifact_type()) else {
        return codec(format!("its artifact type is {}", manifest.artifact_type()));
    };
    let Some(recipe) = title(&manifest.config) else {
        return codec(format!("its config has no {TITLE}"));
    };
    if let Some(problem) = name_problem(recipe) {
        return codec(format!("the recipe's file name {recipe:?} {problem}"));
    }
    if !recipe.ends_with(suffix(kind)) || recipe.len() == suffix(kind).len() {
        return codec(format!(
            "the recipe's file name {recipe:?} does not end in {}",
            suffix(kind)
        ));
    }
    let lock = format!("{recipe}{}/{}/", lock::SUFFIX, lock::FILES);
    let mut titles = BTreeSet::from([recipe.to_owned()]);
    let mut works = 0;
    for layer in &manifest.layers {
        let Some(name) = title(layer) else {
            return codec(format!("the layer {} has no {TITLE}", layer.digest));
        };
        if !titles.insert(name.to_owned()) {
            return codec(format!("two parts are named {name}"));
        }
        match Role::of(&layer.media_type) {
            Some(Role::Lock) => {
                if name != format!("{lock}{}", layer.digest.hex()) {
                    return codec(format!(
                        "the lock file {} is named {name:?}, not {lock}{}",
                        layer.digest,
                        layer.digest.hex()
                    ));
                }
            }
            Some(role) => {
                if let Some(problem) = name_problem(name) {
                    return codec(format!("the part's file name {name:?} {problem}"));
                }
                if role == Role::Work {
                    works += 1;
                    if !name.ends_with(".work.toml") || works > 1 {
                        return codec(format!(
                            "{name}: a recipe artifact holds one work file, WORK.work.toml"
                        ));
                    }
                }
            }
            None => {
                return codec(format!("the layer {name} is a {}", layer.media_type));
            }
        }
    }
    Ok(())
}

/// A recipe file's name and bytes, and every part beside it the artifact
/// carries.
struct Gathered {
    file: String,
    config: Vec<u8>,
    parts: Vec<(Descriptor, Vec<u8>)>,
}

fn read(path: &Path) -> Result<Vec<u8>, Refusal> {
    std::fs::read(path).map_err(|e| Refusal::Unreadable(format!("{}: {e}", path.display())))
}

/// The artifact of the pinned recipe at `path` (recipes §7.1): the recipe
/// must pass its checks with its lock beside it; its work file, bundled
/// inputs and lock files are read from its folder, each bundled input
/// checked against its size and hash. Nothing is written.
pub fn bundle(path: &Path, providers: Providers<'_>) -> Result<Artifact, Refusal> {
    bundle_with_levels(path, providers, historical_levels())
}

/// Bind recipe parser features to the caller's front-end level.
pub fn bundle_with_levels(
    path: &Path,
    providers: Providers<'_>,
    levels: Levels,
) -> Result<Artifact, Refusal> {
    let unreadable = |e: io::Error| Refusal::Unreadable(e.to_string());
    let report = crate::check_paths(&[path.to_path_buf()], providers).map_err(unreadable)?;
    if !report.is_clean() {
        return Err(Refusal::Findings(report.render()));
    }
    let (recipe, work, text) = match crate::load(path, providers).map_err(unreadable)? {
        crate::Opened::Recipe(recipe, work, bytes) => (recipe, work, bytes),
        crate::Opened::Work => {
            return Err(Refusal::Findings(format!(
                "{}: a work file is published with the recipes that name it",
                path.display()
            )))
        }
        crate::Opened::Refused(report) => return Err(Refusal::Findings(report.render())),
    };
    let file = path.display().to_string();
    let Some(digest) = &recipe.identity.digest else {
        return Err(Refusal::Draft { file });
    };
    // The checks refused a pinned recipe without its lock folder (rule 8).
    let lock_dir = lock::dir(path);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Refusal::Codec(format!("{file}: the file name is not UTF-8")))?
        .to_owned();
    let folder = path.parent().unwrap_or(Path::new(""));
    let mut parts = Vec::new();
    let part = |kind: &str, title: &str, bytes: Vec<u8>| {
        let mut descriptor = Descriptor::new(kind, Digest::of(&bytes), bytes.len() as u64);
        descriptor.annotations.insert(TITLE.into(), title.into());
        (descriptor, bytes)
    };
    if let Some(id) = &recipe.work {
        let work_file = format!("{}.work.toml", id.value);
        let bytes = read(&folder.join(&work_file))?;
        parts.push(part(media_type::RECIPE_WORK, &work_file, bytes));
    }
    for (input, size, sha256) in bundled(&recipe)? {
        let bytes = read(&folder.join(&input))?;
        if bytes.len() as u64 != size || Digest::of(&bytes).hex() != sha256 {
            return Err(Refusal::Bundled {
                file: folder.join(&input).display().to_string(),
                size,
                sha256,
            });
        }
        parts.push(part(media_type::RECIPE_INPUT, &input, bytes));
    }
    let lock_files = lock::read(&lock_dir).map_err(Refusal::Unreadable)?;
    for (hex, bytes) in lock_files {
        let title = format!("{name}{}/{}/{hex}", lock::SUFFIX, lock::FILES);
        parts.push(part(media_type::RECIPE_LOCK, &title, bytes));
    }
    let locked = lock::open(&digest.value, &lock_dir).map_err(Refusal::Unreadable)?;
    let required = requirements(&recipe, &locked.manifest, providers).map_err(Refusal::Codec)?;
    let (title_text, description) = match recipe.kind {
        Kind::Media => {
            let merged = crate::merge::merge(&recipe, work.as_deref());
            (
                merged.as_ref().map(|m| m.title.value.clone()),
                merged.and_then(|m| m.description.map(|d| d.value)),
            )
        }
        Kind::Firmware => {
            let package = recipe.package.as_ref();
            (
                package
                    .and_then(|p| p.title.as_ref())
                    .map(|t| t.value.clone()),
                package
                    .and_then(|p| p.description.as_ref())
                    .map(|d| d.value.clone()),
            )
        }
    };
    let gathered = Gathered {
        file: name,
        config: text,
        parts,
    };
    let mut manifest = assemble(&recipe, &gathered)?;
    let annotations = &mut manifest.annotations;
    annotations.insert(OUTPUT.into(), output(&recipe));
    annotations.insert(OUTPUT_DIGEST.into(), digest.value.clone());
    annotations.insert(LICENSES.into(), LICENSE.into());
    if let Some(title) = title_text {
        annotations.insert("org.opencontainers.image.title".into(), title);
    }
    if let Some(description) = description {
        annotations.insert(DESCRIPTION.into(), description);
    }
    compat::stamp(&mut manifest, required, None, levels)
        .map_err(|e| Refusal::Codec(e.to_string()))?;
    let manifest_bytes = manifest
        .encode()
        .map_err(|e| Refusal::Codec(e.to_string()))?;
    let layers = gathered.parts.into_iter().map(|(_, bytes)| bytes).collect();
    Artifact::from_parts_with_levels(manifest_bytes, gathered.config, layers, levels)
}

/// The manifest of the parts, without the annotations the bundle adds.
fn assemble(recipe: &Recipe, gathered: &Gathered) -> Result<Manifest, Refusal> {
    let mut config = Descriptor::new(
        media_type::RECIPE_CONFIG,
        Digest::of(&gathered.config),
        gathered.config.len() as u64,
    );
    config
        .annotations
        .insert(TITLE.into(), gathered.file.clone());
    let platform = recipe
        .identity
        .platform
        .as_ref()
        .map(|p| image_store::platform::canonical(&p.value).to_owned())
        .unwrap_or_default();
    Manifest::new_recipe(
        artifact_type(recipe.kind),
        &platform,
        config,
        gathered.parts.iter().map(|(d, _)| d.clone()).collect(),
    )
    .map_err(|e| Refusal::Codec(e.to_string()))
}

/// The output's reference, `name:tag`.
pub fn output(recipe: &Recipe) -> String {
    let name = recipe
        .identity
        .name
        .as_ref()
        .map(|n| n.value.as_str())
        .unwrap_or_default();
    format!("{name}:{}", crate::build::tag(recipe))
}

/// The bundled inputs of `recipe`, `(name, size, sha256)`: single files
/// with `bundle = true`, each named by one path component.
fn bundled(recipe: &Recipe) -> Result<Vec<(String, u64, String)>, Refusal> {
    let mut out = Vec::new();
    for input in &recipe.inputs {
        if !input.bundle.as_ref().is_some_and(|b| b.value) {
            continue;
        }
        let (
            InputForm::File {
                size: Some(size),
                sha256: Some(sha256),
            },
            Some(name),
        ) = (&input.form, &input.name)
        else {
            return Err(Refusal::Codec(format!(
                "the bundled input {} is not a single file with size and sha256",
                input.id.value
            )));
        };
        if let Some(problem) = name_problem(&name.value) {
            return Err(Refusal::Codec(format!(
                "the bundled input {}'s name {:?} {problem}",
                input.id.value, name.value
            )));
        }
        out.push((name.value.clone(), size.value, sha256.value.clone()));
    }
    Ok(out)
}

/// What [`inspect`] found in an artifact that passes.
#[derive(Debug)]
pub struct Inspected {
    pub recipe: Box<Recipe>,
    /// The recipe file, laid out with its parts in the folder given.
    pub path: PathBuf,
}

/// Read an artifact against its recipe (the codec's second half): the
/// recipe text passes the checks that need no other file; the parts are
/// exactly its work file, its bundled inputs with their sizes and hashes,
/// and its lock files; laid out in `scratch`, an empty folder, the recipe
/// passes every check, rule 8 included, which opens the lock and refuses
/// any file it does not need. `scratch` keeps the layout.
pub fn inspect(
    artifact: &Artifact,
    providers: Providers<'_>,
    scratch: &Path,
) -> Result<Inspected, Refusal> {
    structure(&artifact.manifest)?;
    let file = artifact.recipe_file().to_owned();
    let recipe = crate::check_held(&file, artifact.config.clone(), providers)
        .map_err(|report| Refusal::Findings(report.render()))?;
    if Some(recipe.kind) != kind_of(artifact.manifest.artifact_type()) {
        return Err(Refusal::Codec(format!(
            "a {} recipe in an artifact of type {}",
            recipe.kind.key(),
            artifact.manifest.artifact_type()
        )));
    }
    let work = recipe
        .work
        .as_ref()
        .map(|id| format!("{}.work.toml", id.value));
    let inputs = bundled(&recipe)?;
    let mut wanted: BTreeSet<String> = work.iter().cloned().collect();
    wanted.extend(inputs.iter().map(|(name, _, _)| name.clone()));
    for (descriptor, _) in artifact.parts() {
        let name = title(descriptor).unwrap_or_default();
        let fits = match Role::of(&descriptor.media_type) {
            Some(Role::Work) => work.as_deref() == Some(name),
            Some(Role::Input) => inputs.iter().any(|(n, size, sha256)| {
                n == name && *size == descriptor.size && sha256 == descriptor.digest.hex()
            }),
            // Rule 8 below opens the lock: a lock file it does not need
            // is refused there.
            Some(Role::Lock) => true,
            None => false,
        };
        if !fits {
            return Err(Refusal::ExtraPart {
                title: name.to_owned(),
                media_type: descriptor.media_type.clone(),
            });
        }
        wanted.remove(name);
    }
    if let Some(title) = wanted.into_iter().next() {
        return Err(Refusal::MissingPart { title });
    }
    let path = artifact
        .unpack(scratch)
        .map_err(|e| Refusal::Unreadable(e.to_string()))?;
    let report: Report = crate::check_paths(std::slice::from_ref(&path), providers)
        .map_err(|e| Refusal::Unreadable(e.to_string()))?;
    if !report.is_clean() {
        return Err(Refusal::Findings(report.render()));
    }
    match crate::document(&path).map_err(|e| Refusal::Unreadable(e.to_string()))? {
        Some(Document::Recipe(recipe)) => Ok(Inspected { recipe, path }),
        _ => Err(Refusal::Codec(format!("{file} holds no recipe"))),
    }
}

/// Whether `bytes` are text a recipe may bundle: UTF-8 without NUL.
fn is_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| !text.contains('\0'))
}

/// The push guard of recipes (recipes §6.13, §7.3), for a push of `artifact` to `repository:tag`: [`inspect`] in
/// `scratch`, then a bundled input is text, a recipe without `[license]`
/// names no file under 256 bytes unless it says `disclosable = true`, the
/// target is the recipe's place, the annotations say what the recipe says
/// and the marker is at least what this release computes.
pub fn guard(
    artifact: &Artifact,
    providers: Providers<'_>,
    repository: &str,
    tag: &str,
    scratch: &Path,
) -> Result<(), Refusal> {
    let Inspected { recipe, path } = inspect(artifact, providers, scratch)?;
    for (descriptor, bytes) in artifact.parts() {
        if descriptor.media_type == media_type::RECIPE_INPUT && !is_text(bytes) {
            return Err(Refusal::NotText {
                title: title(descriptor).unwrap_or_default().to_owned(),
            });
        }
    }
    if recipe.license.is_none() {
        disclosable(&recipe)?;
    }
    let output = output(&recipe);
    let (name, own_tag) = output.rsplit_once(':').unwrap_or((&output, ""));
    let namespace = repository.split('/').next().unwrap_or_default();
    let rest = name.split_once('/').map_or("", |(_, rest)| rest);
    let wanted = format!("{namespace}/recipes/{rest}:{own_tag}");
    if format!("{repository}:{tag}") != wanted {
        return Err(Refusal::Name {
            target: format!("{repository}:{tag}"),
            wanted,
        });
    }
    let annotation = |key: &'static str, wanted: String| -> Result<(), Refusal> {
        let found = artifact.manifest.annotations.get(key);
        if found != Some(&wanted) {
            return Err(Refusal::Annotation {
                key,
                found: found.cloned(),
                wanted,
            });
        }
        Ok(())
    };
    let digest = recipe
        .identity
        .digest
        .as_ref()
        .map(|d| d.value.clone())
        .unwrap_or_default();
    annotation(OUTPUT, output.clone())?;
    annotation(OUTPUT_DIGEST, digest.clone())?;
    annotation(LICENSES, LICENSE.into())?;
    let platform = recipe
        .identity
        .platform
        .as_ref()
        .map(|p| image_store::platform::canonical(&p.value).to_owned())
        .unwrap_or_default();
    annotation(PLATFORM, platform)?;
    let locked = lock::open(&digest, &lock::dir(&path)).map_err(Refusal::Unreadable)?;
    let required = requirements(&recipe, &locked.manifest, providers).map_err(Refusal::Codec)?;
    for (component, requirement, floor) in [
        (
            compat::Component::Core,
            Requirements::of(&artifact.manifest).core,
            required.core,
        ),
        (
            compat::Component::Frontend,
            Requirements::of(&artifact.manifest).frontend,
            required.frontend,
        ),
    ] {
        let enough = match &requirement {
            Requirement::None => floor <= component.baseline(),
            Requirement::Release(release) => *release >= floor,
            Requirement::Invalid(_) => false,
        };
        if !enough {
            if component == compat::Component::Core {
                return Err(Refusal::MinAppVersion {
                    declared: requirement.text().unwrap_or_else(|| "none".into()),
                    required: floor,
                });
            }
            return Err(Refusal::Codec(format!(
                "{} declares {}, needs {} {floor} or newer",
                component.annotation(),
                requirement.text().unwrap_or_else(|| "none".into()),
                component.name()
            )));
        }
    }
    Ok(())
}

/// Recipes §6.13: a file input, a file-set entry or a member of an
/// `unpack` step under 256 bytes must say `disclosable = true` in a recipe
/// without `[license]`: each states a hash that gives a tiny file away.
fn disclosable(recipe: &Recipe) -> Result<(), Refusal> {
    let marked = |flag: &Option<crate::model::At<bool>>| flag.as_ref().is_some_and(|f| f.value);
    for input in &recipe.inputs {
        match &input.form {
            InputForm::File {
                size: Some(size), ..
            } if size.value < crate::DISCLOSABLE_BELOW && !marked(&input.disclosable) => {
                return Err(Refusal::Disclosable {
                    input: format!("input.{}", input.id.value),
                    size: size.value,
                });
            }
            InputForm::FileSet { files } => {
                for entry in files {
                    if let Some(size) = &entry.size {
                        if size.value < crate::DISCLOSABLE_BELOW && !marked(&entry.disclosable) {
                            return Err(Refusal::Disclosable {
                                input: format!("input.{} {}", input.id.value, entry.path.value),
                                size: size.value,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for step in &recipe.steps {
        let (crate::model::Op::Unpack { members, .. }, Some(id)) = (&step.op, &step.id) else {
            continue;
        };
        for member in members {
            if let Some(size) = &member.size {
                if size.value < crate::DISCLOSABLE_BELOW && !marked(&member.disclosable) {
                    return Err(Refusal::Disclosable {
                        input: format!("step {} member {}", id.value, member.path.value),
                        size: size.value,
                    });
                }
            }
        }
    }
    Ok(())
}

/// What [`judge`] gives for a registry row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Judged {
    /// The plan of the recipe against the sources, its bundled inputs
    /// found in the artifact.
    Plan(Box<Plan>),
    /// The recipe text does not pass the checks that need no other file.
    Refused(Report),
}

/// Judge a registry row from a recipe manifest and its config alone (recipes
/// §7.1, R13): the recipe text is checked without its work file and lock,
/// which travel with it, and planned against `sources`; a bundled input
/// counts as found when the manifest lists it with the recipe's size and
/// hash. `shown` names the recipe in the report. The caller has read the
/// manifest's marker first.
pub fn judge(
    manifest: &Manifest,
    config: &[u8],
    shown: &str,
    providers: Providers<'_>,
    sources: Sources<'_>,
) -> Result<Judged, Refusal> {
    structure(manifest)?;
    if Digest::of(config) != manifest.config.digest || config.len() as u64 != manifest.config.size {
        return Err(Refusal::Codec(
            "the config is not the one the manifest names".into(),
        ));
    }
    let file = title(&manifest.config).unwrap_or_default();
    let recipe = match crate::check_held(file, config.to_vec(), providers) {
        Ok(recipe) => recipe,
        Err(report) => return Ok(Judged::Refused(report)),
    };
    let plan = crate::plan::plan_listed(&recipe, shown, &listed(manifest), sources);
    Ok(Judged::Plan(Box::new(plan)))
}

/// The release a manifest's marker asks for, for a row's "Needs unemupc X
/// or newer"; `None` when this release reads it.
pub fn needs(manifest_bytes: &[u8], levels: Levels) -> Option<String> {
    Requirements::read(manifest_bytes).unmet_reason(levels)
}

fn requirements(
    recipe: &Recipe,
    output: &Manifest,
    providers: Providers<'_>,
) -> Result<compat::Releases, String> {
    let mut required =
        crate::compat::requirements(recipe, &Requirements::of(output), &crate::compat::TABLE)?;
    let platform = recipe
        .identity
        .platform
        .as_ref()
        .map(|platform| image_store::platform::canonical(&platform.value))
        .ok_or("the recipe states no platform")?;
    let provider = providers
        .get(platform)
        .ok_or_else(|| format!("no provider for platform {platform}"))?;
    required.core = required.core.max(provider.core_platform_release(platform)?);
    Ok(required)
}

fn historical_levels() -> Levels {
    Levels::new(
        Some((compat::EXTRACTION_BASELINE, compat::BASELINE)),
        compat::EXTRACTION_BASELINE,
        compat::NEWEST_ROW,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(kind: &str, title: Option<&str>, bytes: &[u8]) -> Descriptor {
        let mut d = Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
        if let Some(title) = title {
            d.annotations.insert(TITLE.into(), title.into());
        }
        d
    }

    const RECIPE: &[u8] = b"recipe = \"1.0\"\n";

    /// Read back an artifact of `kind` whose config is titled `config` and
    /// whose layers are `(media type, title, bytes)`.
    fn read(
        kind: &str,
        config: Option<&str>,
        layers: &[(&str, Option<&str>, &[u8])],
    ) -> Result<Artifact, Refusal> {
        let manifest = Manifest::new_recipe(
            kind,
            "ibm-pc",
            descriptor(media_type::RECIPE_CONFIG, config, RECIPE),
            layers
                .iter()
                .map(|(k, t, b)| descriptor(k, *t, b))
                .collect(),
        )
        .unwrap();
        Artifact::from_parts(
            manifest.encode().unwrap(),
            RECIPE.to_vec(),
            layers.iter().map(|(_, _, b)| b.to_vec()).collect(),
        )
    }

    fn codec(result: Result<Artifact, Refusal>) -> String {
        match result {
            Err(Refusal::Codec(why)) => why,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_codec_refuses_every_title_that_is_not_the_folder_of_a_recipe() {
        let media = media_type::RECIPE_MEDIA;
        let input = media_type::RECIPE_INPUT;
        let ok = Some("x.media.toml");
        assert!(read(media, ok, &[(input, Some("NOTES.TXT"), b"n")]).is_ok());
        // Titles missing, on the config or a layer.
        assert!(codec(read(media, None, &[])).contains("config has no"));
        assert!(codec(read(media, ok, &[(input, None, b"n")])).contains("has no"));
        // Titles that are no single plain file name.
        for bad in ["../x", "/abs", r"a\b", "C:x", ".hidden", ""] {
            let why = codec(read(media, ok, &[(input, Some(bad), b"n")]));
            assert!(why.contains("file name"), "{bad}: {why}");
        }
        // Duplicates, and a part named as the recipe.
        let why = codec(read(
            media,
            ok,
            &[(input, Some("A.TXT"), b"a"), (input, Some("A.TXT"), b"b")],
        ));
        assert!(why.contains("two parts are named A.TXT"), "{why}");
        let why = codec(read(media, ok, &[(input, Some("x.media.toml"), b"a")]));
        assert!(why.contains("two parts are named x.media.toml"), "{why}");
        // A lock file named by another hash, or outside the lock.
        let lock = media_type::RECIPE_LOCK;
        let own = format!("x.media.toml.lock/sha256/{}", Digest::of(b"l").hex());
        assert!(read(media, ok, &[(lock, Some(&own), b"l")]).is_ok());
        let other = format!("x.media.toml.lock/sha256/{}", Digest::of(b"m").hex());
        for bad in [other.as_str(), "y.media.toml.lock/sha256/x", "LOCK"] {
            let why = codec(read(media, ok, &[(lock, Some(bad), b"l")]));
            assert!(why.contains("the lock file"), "{bad}: {why}");
        }
        // The recipe's name and the artifact's kind; work files.
        for (kind, name) in [
            (media, "x.firmware.toml"),
            (media_type::RECIPE_FIRMWARE, "x.media.toml"),
            (media, "x.toml"),
        ] {
            let why = codec(read(kind, Some(name), &[]));
            assert!(why.contains("does not end in"), "{name}: {why}");
        }
        assert!(codec(read(media, Some(".media.toml"), &[])).contains("is hidden"));
        let work = media_type::RECIPE_WORK;
        assert!(codec(read(media, ok, &[(work, Some("w.toml"), b"w")])).contains("one work file"));
        assert!(codec(read(
            media,
            ok,
            &[
                (work, Some("a.work.toml"), b"a"),
                (work, Some("b.work.toml"), b"b")
            ]
        ))
        .contains("one work file"));
    }

    #[test]
    fn the_codec_refuses_bytes_that_are_not_the_descriptors() {
        let manifest = Manifest::new_recipe(
            media_type::RECIPE_MEDIA,
            "ibm-pc",
            descriptor(media_type::RECIPE_CONFIG, Some("x.media.toml"), RECIPE),
            vec![descriptor(
                media_type::RECIPE_INPUT,
                Some("N.TXT"),
                b"notes",
            )],
        )
        .unwrap();
        let bytes = manifest.encode().unwrap();
        let attempt = |config: &[u8], layers: Vec<Vec<u8>>| {
            codec(Artifact::from_parts(bytes.clone(), config.to_vec(), layers))
        };
        assert!(
            attempt(b"recipe = \"0.2\"\n", vec![b"notes".to_vec()]).contains("are not the ones")
        );
        assert!(attempt(RECIPE, vec![b"Notes".to_vec()]).contains("are not the ones"));
        assert!(attempt(RECIPE, vec![b"notes!".to_vec()]).contains("are not the ones"));
        assert!(attempt(RECIPE, vec![]).contains("missing"));
        // Another artifact type than a recipe's.
        let mut other = String::from_utf8(bytes.clone()).unwrap();
        other = other.replace(media_type::RECIPE_MEDIA, media_type::MEDIA_SET);
        assert!(matches!(
            Artifact::from_parts(other.into_bytes(), RECIPE.to_vec(), vec![b"notes".to_vec()]),
            Err(Refusal::Codec(_))
        ));
    }

    /// `unpack` checks the titles itself, so an artifact assembled without
    /// the codec writes nothing outside its folder.
    #[test]
    fn unpack_checks_the_titles_itself() {
        let config = descriptor(media_type::RECIPE_CONFIG, Some("x.media.toml"), RECIPE);
        let layer = descriptor(media_type::RECIPE_INPUT, Some("../escape"), b"n");
        let manifest =
            Manifest::new_recipe(media_type::RECIPE_MEDIA, "ibm-pc", config, vec![layer]).unwrap();
        let artifact = Artifact {
            manifest_bytes: manifest.encode().unwrap(),
            digest: Digest::of(b""),
            manifest,
            config: RECIPE.to_vec(),
            layers: vec![b"n".to_vec()],
        };
        let dir = std::env::temp_dir().join(format!("recipe-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let error = artifact.unpack(&dir.join("inside")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!dir.exists());
    }
}
