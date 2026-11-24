// SPDX-License-Identifier: AGPL-3.0-or-later
//! The lock of a pinned recipe (recipes §3.1, §3.8, §6.16, §7.2): the
//! output's manifest, config and metadata, byte for byte, as the author's
//! program wrote them when the recipe was pinned.
//!
//! **Why.** A set's digest depends on how one release spells its manifest,
//! config and metadata and which annotations it writes. A later release
//! must still build the digest a recipe was pinned to, so it never writes
//! those three again: it builds the payload blobs, checks each against the
//! locked manifest, and publishes the locked bytes unchanged. Only the
//! payload bytes, which the steps make, bind for good (recipes §9).
//!
//! **Where and how.** Beside `NAME.media.toml` the lock is the folder
//! `NAME.media.toml.lock`, holding `sha256/<hex>` files, each named by the
//! lower-case SHA-256 of its bytes, as the store names its blobs: the
//! manifest, its config, and every other layer of the manifest that no step
//! makes (the title metadata of a set, the generated README of a firmware
//! package). The recipe's `digest` names the manifest and the manifest's
//! descriptors name the rest, so the recipe file alone pins every byte.
//! The files are exactly the bytes the store holds, so writing a lock again
//! from the same artifact writes the same files. The lock holds no payload
//! byte, so it is as publishable as the recipe.
//!
//! **Checks**, in the order of recipes §3.8:
//!
//! 1. [`open`]: the recipe's `digest` is the SHA-256 of a lock file, and
//!    every lock file holds the bytes its name says;
//! 2. [`open`]: the manifest's config descriptor, and every layer it does
//!    not take from a step, name lock files of the size they state, and the
//!    lock holds nothing the manifest does not name;
//! 3. [`compare`]: the lock's decoded fields agree with the recipe, field
//!    by field. A title field that comes from the work file and differs is
//!    drift, not an error: the lock wins until a new revision is pinned;
//! 4. [`verify`]: every payload the steps made has its descriptor's decoded
//!    SHA-256 and size, and encodes to its stored SHA-256 and size;
//! 5. [`verify`]: every layer of the manifest is a payload a step made or a
//!    lock file, and no output is left without a descriptor.
//!
//! Points 1 to 3 are part of `recipe check` (rule 8), so a broken lock stops
//! a build before any input is read. Points 4 and 5 run after the steps and
//! before anything is written; [`store`] then writes the payloads, the lock
//! files and the manifest into the store, and the platform's publisher tags
//! the locked manifest ([`crate::build::Publisher::tag_locked`]).

use crate::build::Built;
use crate::merge::{Merged, Origin, Sourced};
use crate::model::{At, Kind, Recipe, RomSource};
use crate::provider::Provider;
use crate::steps::Bytes;
use image_store::codec;
use image_store::{
    export_name, firmware, media_set, media_type, metadata, Descriptor, Digest, Manifest,
    PayloadType, Store, MAX_SMALL_BLOB,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// The suffix of a lock folder: `NAME.media.toml.lock`.
pub const SUFFIX: &str = ".lock";

/// The folder inside a lock that holds its files, named after the hash.
pub const FILES: &str = "sha256";

/// The lock folder of the recipe at `recipe`.
pub fn dir(recipe: &Path) -> PathBuf {
    let mut name = OsString::from(recipe.as_os_str());
    name.push(SUFFIX);
    PathBuf::from(name)
}

fn hex(bytes: &[u8]) -> String {
    Digest::of(bytes).hex().to_owned()
}

fn is_hex(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Read every file of the lock at `dir`, each checked against its name.
/// Hidden entries (a desktop's `.DS_Store`) are skipped; any other entry
/// that is not `sha256/<hex>` is an error, as is a file over 1 MiB, the
/// limit of a manifest, config or metadata blob.
pub fn read(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let shown = dir.display();
    let entries = |folder: &Path| -> Result<Vec<(String, PathBuf)>, String> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))? {
            let entry = entry.map_err(|e| format!("{}: {e}", folder.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(format!(
                    "{}: a name that is not UTF-8: {name:?}",
                    folder.display()
                ));
            };
            if !name.starts_with('.') {
                out.push((name.to_owned(), entry.path()));
            }
        }
        out.sort();
        Ok(out)
    };
    let mut files = BTreeMap::new();
    for (name, path) in entries(dir)? {
        if name != FILES || !path.is_dir() {
            return Err(format!(
                "{shown}: holds {name}; a lock holds only {FILES}/<hex> files"
            ));
        }
    }
    let folder = dir.join(FILES);
    if !folder.is_dir() {
        return Err(format!("{shown}: no {FILES} folder"));
    }
    for (name, path) in entries(&folder)? {
        if !is_hex(&name) || !path.is_file() {
            return Err(format!(
                "{shown}/{FILES}/{name}: not a file named by a lower-case SHA-256"
            ));
        }
        let size = std::fs::metadata(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        if size > MAX_SMALL_BLOB {
            return Err(format!(
                "{shown}/{FILES}/{name}: {size} bytes; a lock file is at most 1 MiB"
            ));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let actual = hex(&bytes);
        if actual != name {
            return Err(format!(
                "{shown}/{FILES}/{name}: holds bytes of sha256 {actual}, not the ones its name says"
            ));
        }
        files.insert(name, bytes);
    }
    Ok(files)
}

/// Write a lock of `files` at `dir`, replacing any lock there. The new lock
/// is written whole beside it and renamed into place, so a reader never sees
/// a mix of the two; between moving the old lock aside and renaming the new
/// one in there is briefly no lock, which a reader reports as missing.
pub fn write(dir: &Path, files: &[Vec<u8>]) -> io::Result<()> {
    let sibling = |what: &str| {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut name = OsString::from(dir.as_os_str());
        name.push(format!(
            ".{what}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        PathBuf::from(name)
    };
    let staged = sibling("new");
    let result = (|| {
        std::fs::create_dir_all(staged.join(FILES))?;
        for bytes in files {
            std::fs::write(staged.join(FILES).join(hex(bytes)), bytes)?;
        }
        if dir.exists() {
            let old = sibling("old");
            std::fs::rename(dir, &old)?;
            std::fs::rename(&staged, dir)?;
            std::fs::remove_dir_all(&old)
        } else {
            std::fs::rename(&staged, dir)
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staged);
    }
    result
}

/// A lock that passed points 1 and 2 of recipes §3.8.
#[derive(Clone, Debug)]
pub struct Locked {
    pub digest: Digest,
    pub manifest_bytes: Vec<u8>,
    pub manifest: Manifest,
    pub config: Vec<u8>,
    /// The bytes of the manifest's title metadata layer, when it has one.
    pub metadata: Option<Vec<u8>>,
    /// Every lock file but the manifest, by digest.
    pub blobs: BTreeMap<Digest, Vec<u8>>,
}

/// Points 1 and 2: read the lock at `dir` and check it against the
/// recipe's `digest`.
pub fn open(digest: &str, dir: &Path) -> Result<Locked, String> {
    let files = read(dir)?;
    check(digest, files, &dir.display().to_string())
}

/// Points 1 and 2 for lock files held in memory, by the lower-case SHA-256
/// of their bytes; `shown` names them in a message.
fn check(
    digest: &str,
    mut files: BTreeMap<String, Vec<u8>>,
    shown: &str,
) -> Result<Locked, String> {
    let wanted: Digest = digest
        .parse()
        .map_err(|e| format!("the digest {digest}: {e}"))?;
    let manifest_bytes = files.remove(wanted.hex()).ok_or_else(|| {
        format!(
            "{shown}: no {FILES}/{}; the lock does not hold the manifest the digest names",
            wanted.hex()
        )
    })?;
    let manifest = Manifest::decode_for_summary(&manifest_bytes)
        .map_err(|e| format!("the locked manifest {wanted}: {e}"))?;
    let blobs: BTreeMap<Digest, Vec<u8>> = files
        .into_values()
        .map(|bytes| (Digest::of(&bytes), bytes))
        .collect();
    let take = |descriptor: &Descriptor, what: &str| -> Result<Vec<u8>, String> {
        match blobs.get(&descriptor.digest) {
            Some(bytes) if bytes.len() as u64 == descriptor.size => Ok(bytes.clone()),
            Some(bytes) => Err(format!(
                "the locked manifest names its {what} {} of {} bytes; the lock's file has {}",
                descriptor.digest,
                descriptor.size,
                bytes.len()
            )),
            None => Err(format!(
                "the locked manifest names its {what} {}, which the lock does not hold",
                descriptor.digest
            )),
        }
    };
    let config = take(&manifest.config, "config")?;
    let metadata = match manifest
        .layers
        .iter()
        .find(|l| l.media_type == media_type::METADATA)
    {
        Some(layer) => Some(take(layer, "metadata")?),
        None => None,
    };
    // The lock holds the text the writers generate and no payload byte:
    // the config, the metadata and a package's README, found by the
    // manifest's structure, never by which files the lock happens to hold.
    let generated = generated_layers(&manifest, &config);
    if let Some(stray) = blobs.keys().find(|d| !generated.contains(d)) {
        let named = manifest.layers.iter().any(|l| &l.digest == stray);
        return Err(format!(
            "{shown}: holds {FILES}/{}, which {}; a lock holds the config, the metadata and a package's README, and no payload byte",
            stray.hex(),
            if named {
                "is a payload layer of the locked manifest"
            } else {
                "the locked manifest does not name"
            }
        ));
    }
    Ok(Locked {
        digest: wanted,
        manifest_bytes,
        manifest,
        config,
        metadata,
        blobs,
    })
}

/// The files a lock of the artifact `manifest` in `store` holds: the
/// manifest, its config and every layer whose decoded SHA-256 is not among
/// `made`, the outputs the steps made (lower-case hex), as the store holds
/// them. A recipe is pinned with them ([`crate::pin::pin_path`]), and a
/// build whose lock is lost rebuilds them (recipes §7.2).
pub fn files_of(
    store: &Store,
    manifest: &Descriptor,
    made: &BTreeSet<String>,
) -> Result<Vec<Vec<u8>>, String> {
    use image_store::BlobSource as _;
    let manifest_bytes = store
        .read_verified(manifest, MAX_SMALL_BLOB)
        .map_err(|e| format!("the built manifest {}: {e}", manifest.digest))?;
    let decoded = Manifest::decode_for_summary(&manifest_bytes)
        .map_err(|e| format!("the built manifest {}: {e}", manifest.digest))?;
    let mut files = vec![manifest_bytes];
    let mut written = vec![&decoded.config];
    for layer in &decoded.layers {
        let is_made = layer.decoded_digest().is_ok_and(|d| made.contains(d.hex()));
        if !is_made {
            written.push(layer);
        }
    }
    for descriptor in written {
        let bytes = store
            .read_verified(descriptor, MAX_SMALL_BLOB)
            .map_err(|e| {
                format!(
                    "{} ({}), which no step made: {e}; a lock holds files of at most 1 MiB",
                    descriptor.digest, descriptor.media_type
                )
            })?;
        files.push(bytes);
    }
    files.dedup();
    Ok(files)
}

/// Points 1 and 2 for lock files a build rebuilt with its own writers, as
/// [`files_of`] gives them, against the recipe's `digest`.
pub fn rebuilt(digest: &str, files: &[Vec<u8>]) -> Result<Locked, String> {
    let files = files
        .iter()
        .map(|bytes| (hex(bytes), bytes.clone()))
        .collect();
    check(digest, files, "the rebuilt lock")
}

/// The payload the lock names for each output a step makes, found through
/// the locked config as [`verify`] matches them: media in order, extras by
/// file name, ROMs and their chips in order. An output the lock names no
/// layer of is left out; the build then makes it and [`verify`] reports it.
pub fn payloads(
    recipe: &Recipe,
    locked: &Locked,
    provider: &dyn Provider,
) -> Vec<crate::plan::LockedOutput> {
    let text = |v: &Option<At<String>>| v.as_ref().map(|v| v.value.clone());
    let mut out = Vec::new();
    let push = |out: &mut Vec<crate::plan::LockedOutput>,
                (kind, index, part): (&'static str, usize, usize),
                name: String,
                blob: &Digest| {
        if let Some(size) = layer(&locked.manifest, blob).and_then(|d| d.decoded_size().ok()) {
            out.push(crate::plan::LockedOutput {
                kind,
                index,
                part,
                name,
                size,
                sha256: blob.hex().to_owned(),
            });
        }
    };
    let extra_name = |extra: &crate::model::Extra| {
        text(&extra.file_name)
            .or_else(|| text(&extra.id))
            .unwrap_or_default()
    };
    match recipe.kind {
        Kind::Media => {
            let Ok(config) = media_set::Config::decode(&locked.config) else {
                return out;
            };
            for (index, (medium, entry)) in recipe.media.iter().zip(&config.media).enumerate() {
                let id = text(&medium.id).unwrap_or_default();
                let format = text(&medium.format).unwrap_or_default();
                let name = text(&medium.file_name)
                    .unwrap_or_else(|| provider.media().file_name(&id, &format));
                push(&mut out, ("medium", index, 0), name, &entry.blob);
            }
            for (index, extra) in recipe.extras.iter().enumerate() {
                let name = extra_name(extra);
                if let Some(entry) = config.extras.iter().find(|e| e.filename == name) {
                    push(&mut out, ("extra", index, 0), name, &entry.blob);
                }
            }
        }
        Kind::Firmware => {
            let Ok(config) = firmware::Config::decode(&locked.config) else {
                return out;
            };
            for (index, (rom, entry)) in recipe.roms.iter().zip(&config.roms).enumerate() {
                let references: Vec<String> = match &rom.source {
                    RomSource::File(from) => text(from).into_iter().collect(),
                    RomSource::Parts { parts, .. } => {
                        parts.iter().filter_map(|p| text(&p.from)).collect()
                    }
                };
                let blobs: Vec<&Digest> = match &entry.blob {
                    Some(blob) => vec![blob],
                    None => entry.parts.iter().map(|p| &p.blob).collect(),
                };
                for (part, (reference, blob)) in references.iter().zip(blobs).enumerate() {
                    let name = reference.rsplit('/').next().unwrap_or(reference).to_owned();
                    push(&mut out, ("rom", index, part), name, blob);
                }
            }
            for (index, extra) in recipe.extras.iter().enumerate() {
                let name = extra_name(extra);
                let blob = config
                    .extras
                    .iter()
                    .find(|e| e.filename == name)
                    .map(|e| &e.blob)
                    .or_else(|| {
                        config
                            .sources
                            .iter()
                            .find(|s| s.filename == name)
                            .map(|s| &s.blob)
                    });
                if let Some(blob) = blob {
                    push(&mut out, ("extra", index, 0), name, blob);
                }
            }
        }
    }
    out
}

/// The stored digests of the descriptors the writers generate, by the
/// manifest's structure: its config, its metadata layer and, for a firmware
/// package, the layer its config names as the README. A config that does
/// not decode names no README.
fn generated_layers(manifest: &Manifest, config: &[u8]) -> BTreeSet<Digest> {
    let readme = (manifest.artifact_type() == media_type::FIRMWARE)
        .then(|| firmware::Config::decode(config).ok())
        .flatten()
        .and_then(|c| c.readme)
        .map(|r| r.blob);
    std::iter::once(manifest.config.digest.clone())
        .chain(
            manifest
                .layers
                .iter()
                .filter(|l| {
                    l.media_type == media_type::METADATA
                        || readme
                            .as_ref()
                            .is_some_and(|r| l.media_type == media_type::MARKDOWN && l.names(r))
                })
                .map(|l| l.digest.clone()),
        )
        .collect()
}

/// One field where the lock and the recipe, or its work file, differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    /// The field, in the recipe's terms: `medium[2].label`, `title.synopsis`.
    pub field: String,
    /// The lock's value, as text.
    pub locked: String,
    /// Today's value, as text.
    pub now: String,
    /// The file that states today's value: `the recipe`, or the work file's
    /// name.
    pub from: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: the lock has {}, {} has {}",
            self.field, self.locked, self.from, self.now
        )
    }
}

/// Point 3: what disagrees. `errors` refuse the recipe; `drift` lists the
/// work file's fields that changed since the pin, which the lock overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    pub errors: Vec<Difference>,
    pub drift: Vec<Difference>,
}

const RECIPE: &str = "the recipe";

fn text(v: Option<&str>) -> String {
    v.map_or_else(|| "nothing".to_owned(), |v| format!("{v:?}"))
}

fn list(v: &[String]) -> String {
    if v.is_empty() {
        "nothing".to_owned()
    } else {
        format!("{v:?}")
    }
}

fn stated(v: &Option<At<String>>) -> Option<&str> {
    v.as_ref().map(|v| v.value.as_str())
}

struct Comparer<'a> {
    out: Comparison,
    work_file: &'a str,
}

impl Comparer<'_> {
    /// A difference of a value the recipe states.
    fn error(&mut self, field: impl Into<String>, locked: String, now: String) {
        self.out.errors.push(Difference {
            field: field.into(),
            locked,
            now,
            from: RECIPE.into(),
        });
    }

    /// Compare two values the recipe states.
    fn same(&mut self, field: impl Into<String>, locked: String, now: String) {
        if locked != now {
            self.error(field, locked, now);
        }
    }

    /// Compare a value of the merged title record: a difference is drift
    /// only when today's value is the work file's, which the lock overrides
    /// until a new revision; a value the recipe states, or one that nothing
    /// states any more, is an error.
    fn merged(
        &mut self,
        field: impl Into<String>,
        locked: String,
        now: String,
        from: Option<Origin>,
    ) {
        if locked == now {
            return;
        }
        let field = field.into();
        if from != Some(Origin::Work) {
            self.error(field, locked, now);
        } else {
            self.out.drift.push(Difference {
                field,
                locked,
                now,
                from: self.work_file.to_owned(),
            });
        }
    }
}

fn origin<T>(v: &Option<Sourced<T>>) -> Option<Origin> {
    v.as_ref().map(|s| s.from)
}

/// Point 3: compare the lock's decoded fields with the recipe and, for a
/// Mediafile, with today's merge of its title record (`merged`). The
/// provider, when there is one, gives the media formats' config tables and
/// default file names.
pub fn compare(
    recipe: &Recipe,
    merged: Option<&Merged>,
    locked: &Locked,
    provider: Option<&dyn Provider>,
) -> Comparison {
    let mut c = Comparer {
        out: Comparison::default(),
        work_file: merged
            .and_then(|m| m.work_file.as_deref())
            .unwrap_or("the work file"),
    };
    let artifact_type = match recipe.kind {
        Kind::Media => media_type::MEDIA_SET,
        Kind::Firmware => media_type::FIRMWARE,
    };
    c.same(
        "artifact type",
        locked.manifest.artifact_type().to_owned(),
        artifact_type.to_owned(),
    );
    let platform = stated(&recipe.identity.platform).unwrap_or_default();
    let locked_platform = locked.manifest.platform().unwrap_or_default();
    if !image_store::platform::same(locked_platform, platform) {
        c.error(
            format!("{}.platform", recipe.kind.key()),
            locked_platform.to_owned(),
            platform.to_owned(),
        );
    }
    match recipe.kind {
        Kind::Media => match media_set::Config::decode(&locked.config) {
            Ok(config) => {
                media(&mut c, recipe, locked, &config, provider);
                marks(&mut c, recipe, locked, MarksConfig::Media(&config));
                match (&locked.metadata, merged) {
                    (Some(bytes), Some(merged)) => {
                        match metadata::Metadata::decode(bytes, &locked.manifest.layers) {
                            Ok(metadata) => title(&mut c, merged, &metadata),
                            Err(e) => c.error(
                                "title",
                                format!("unreadable metadata ({e})"),
                                String::new(),
                            ),
                        }
                    }
                    (None, _) => c.error("title", "no metadata".into(), "a title record".into()),
                    (_, None) => {}
                }
            }
            Err(e) => c.error(
                "config",
                format!("an unreadable set config ({e})"),
                String::new(),
            ),
        },
        Kind::Firmware => match firmware::Config::decode(&locked.config) {
            Ok(config) => {
                package(&mut c, recipe, locked, &config, provider);
                marks(&mut c, recipe, locked, MarksConfig::Firmware(&config));
            }
            Err(e) => c.error(
                "config",
                format!("an unreadable firmware config ({e})"),
                String::new(),
            ),
        },
    }
    c.out
}

/// The layer whose decoded content is `blob`, never the metadata.
fn layer<'m>(manifest: &'m Manifest, blob: &Digest) -> Option<&'m Descriptor> {
    manifest
        .layers
        .iter()
        .find(|l| l.media_type != media_type::METADATA && l.names(blob))
}

fn media(
    c: &mut Comparer<'_>,
    recipe: &Recipe,
    locked: &Locked,
    config: &media_set::Config,
    provider: Option<&dyn Provider>,
) {
    c.same(
        "medium",
        format!("{} media", config.media.len()),
        format!("{} media", recipe.media.len()),
    );
    for (index, (medium, locked_medium)) in recipe.media.iter().zip(&config.media).enumerate() {
        let at = format!("medium[{}]", index + 1);
        let id = stated(&medium.id).unwrap_or_default();
        let format = stated(&medium.format).unwrap_or_default();
        c.same(
            format!("{at}.id"),
            text(Some(&locked_medium.id)),
            text(Some(id)),
        );
        c.same(
            format!("{at}.label"),
            text(Some(&locked_medium.label)),
            text(Some(stated(&medium.label).unwrap_or(id))),
        );
        // Compared by name, a decoded field: the provider names the format
        // the locked config's table describes.
        if let Some(provider) = provider {
            let name = provider.media().format_name(&locked_medium.format);
            c.same(
                format!("{at}.format"),
                text(name.as_deref()),
                text(Some(format)),
            );
        }
        let file_name = stated(&medium.file_name)
            .map(str::to_owned)
            .or_else(|| provider.map(|p| p.media().file_name(id, format)));
        if let Some(file_name) = file_name {
            let locked_name = layer(&locked.manifest, &locked_medium.blob)
                .and_then(export_name::decoded)
                .map(str::to_owned);
            c.same(
                format!("{at}.file_name"),
                text(locked_name.as_deref()),
                text(Some(&file_name)),
            );
        }
    }
    extras(c, recipe, &config.extras, None, provider);
}

/// The extras in order: file name, kind and DOS name, each against the
/// importer's default when the recipe states none (the kind the provider
/// gives the file name, no DOS name). `skip` is the extra a firmware
/// package keeps as its source archive rather than as an extra.
fn extras(
    c: &mut Comparer<'_>,
    recipe: &Recipe,
    locked: &[media_set::Extra],
    skip: Option<&str>,
    provider: Option<&dyn Provider>,
) {
    let mine: Vec<(usize, &crate::model::Extra)> = recipe
        .extras
        .iter()
        .enumerate()
        .filter(|(_, e)| skip.is_none() || stated(&e.id) != skip)
        .collect();
    c.same(
        "extra",
        format!("{} extras", locked.len()),
        format!("{} extras", mine.len()),
    );
    for ((index, extra), locked) in mine.into_iter().zip(locked) {
        let at = format!("extra[{}]", index + 1);
        let id = stated(&extra.id).unwrap_or_default();
        let file_name = stated(&extra.file_name).unwrap_or(id);
        c.same(
            format!("{at}.file_name"),
            text(Some(&locked.filename)),
            text(Some(file_name)),
        );
        let kind = stated(&extra.kind)
            .map(str::to_owned)
            .or_else(|| provider.and_then(|p| p.media().extra_kind(recipe.kind, file_name)));
        if let Some(kind) = kind {
            let locked_kind = serde_json::to_value(locked.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            c.same(
                format!("{at}.kind"),
                text(Some(&locked_kind)),
                text(Some(&kind)),
            );
        }
        c.same(
            format!("{at}.dos_name"),
            text(locked.dos_name.as_deref()),
            text(stated(&extra.dos_name)),
        );
    }
}

/// The catalogue id the importer gives a ROM the recipe names no id for:
/// the catalogue entry its bytes belong to, when the recipe states them.
fn identified(recipe: &Recipe, rom: &crate::model::Rom, provider: &dyn Provider) -> Option<String> {
    let RomSource::File(Some(from)) = &rom.source else {
        return None;
    };
    let input = recipe.inputs.iter().find(|i| i.id.value == from.value)?;
    let crate::model::InputForm::File {
        size: Some(size),
        sha256: Some(sha256),
    } = &input.form
    else {
        return None;
    };
    let firmware = provider.firmware();
    let scoped = firmware.for_package(recipe.package.as_ref());
    scoped
        .as_deref()
        .unwrap_or(firmware)
        .identify(&sha256.value, size.value)
        .map(|(id, _)| id)
}

fn sorted(list: &[At<String>]) -> Vec<String> {
    let mut out: Vec<String> = list.iter().map(|v| v.value.clone()).collect();
    out.sort();
    out
}

fn package(
    c: &mut Comparer<'_>,
    recipe: &Recipe,
    locked: &Locked,
    config: &firmware::Config,
    provider: Option<&dyn Provider>,
) {
    c.same(
        "rom",
        format!("{} ROMs", config.roms.len()),
        format!("{} ROMs", recipe.roms.len()),
    );
    for (index, (rom, locked_rom)) in recipe.roms.iter().zip(&config.roms).enumerate() {
        let at = format!("rom[{}]", index + 1);
        c.same(
            format!("{at}.id"),
            text(Some(&locked_rom.id)),
            text(stated(&rom.id)),
        );
        c.same(
            format!("{at}.role"),
            text(Some(&locked_rom.role)),
            text(stated(&rom.role)),
        );
        let catalogue_id = stated(&rom.catalogue_id)
            .map(str::to_owned)
            .or_else(|| provider.and_then(|p| identified(recipe, rom, p)));
        if catalogue_id.is_some() || provider.is_some() {
            c.same(
                format!("{at}.catalogue_id"),
                text(locked_rom.catalogue_id.as_deref()),
                text(catalogue_id.as_deref()),
            );
        }
        let parts: &[crate::model::Part] = match &rom.source {
            RomSource::File(_) => &[],
            RomSource::Parts { parts, .. } => parts,
        };
        c.same(
            format!("{at}.parts"),
            format!("{} chips", locked_rom.parts.len()),
            format!("{} chips", parts.len()),
        );
        for (chip, (part, locked_part)) in parts.iter().zip(&locked_rom.parts).enumerate() {
            let at = format!("{at}.parts[{}]", chip + 1);
            for (key, locked, mine) in [
                ("position", &locked_part.position, &part.position),
                ("part_number", &locked_part.part_number, &part.part_number),
            ] {
                if let Some(mine) = stated(mine) {
                    c.same(
                        format!("{at}.{key}"),
                        text(locked.as_deref()),
                        text(Some(mine)),
                    );
                }
            }
            if let Some(lane) = stated(&part.lane) {
                c.same(
                    format!("{at}.lane"),
                    text(Some(&locked_part.lane)),
                    text(Some(lane)),
                );
            }
            if let Some(offset) = &part.offset {
                c.same(
                    format!("{at}.offset"),
                    locked_part.offset.to_string(),
                    offset.value.to_string(),
                );
            }
        }
    }
    let source = recipe
        .license
        .as_ref()
        .and_then(|l| stated(&l.source))
        .map(str::to_owned);
    let source_name = source.as_deref().and_then(|id| {
        recipe
            .extras
            .iter()
            .find(|e| stated(&e.id) == Some(id))
            .map(|e| stated(&e.file_name).unwrap_or(id).to_owned())
    });
    c.same(
        "license.source",
        list(
            &config
                .sources
                .iter()
                .map(|s| s.filename.clone())
                .collect::<Vec<_>>(),
        ),
        list(&source_name.into_iter().collect::<Vec<_>>()),
    );
    extras(c, recipe, &config.extras, source.as_deref(), provider);
    if let Some(package) = &recipe.package {
        if let Some(title) = stated(&package.title) {
            c.same("package.title", text(Some(&config.name)), text(Some(title)));
        }
        // A field the recipe leaves out is one the importer leaves empty.
        for (key, locked, mine) in [
            ("vendor", &config.vendor, &package.vendor),
            ("version", &config.version, &package.version),
            ("date", &config.date, &package.date),
            ("description", &config.description, &package.description),
            ("notes", &config.notes, &package.notes),
        ] {
            c.same(
                format!("package.{key}"),
                text(locked.as_deref()),
                text(stated(mine)),
            );
        }
        let mut labels = config.labels.clone();
        labels.sort();
        c.same(
            "package.labels",
            list(&labels),
            list(&sorted(&package.labels)),
        );
        // Boards and machine classes the recipe leaves out are the
        // catalogue's; the ones it states replace them.
        for (key, mine) in [
            ("boards", &package.boards),
            ("machine_classes", &package.machine_classes),
        ] {
            if mine.is_empty() {
                continue;
            }
            let mut fits: Vec<String> = config
                .fits
                .get(key)
                .and_then(toml::Value::as_array)
                .map(|v| {
                    v.iter()
                        .filter_map(|i| i.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            fits.sort();
            c.same(format!("package.{key}"), list(&fits), list(&sorted(mine)));
        }
    }
    // The README is text the importer generates from the config and the
    // layers' marks; a recipe build writes nothing else.
    if let Some(readme) = &config.readme {
        let bytes = locked.blobs.get(&readme.blob).map(Vec::as_slice);
        let generated = provider.and_then(|p| p.firmware().readme(config, &locked.manifest));
        match (bytes, generated) {
            (Some(bytes), Some(generated)) if bytes != generated.as_bytes() => c.error(
                "README",
                format!(
                    "a text other than the generated one ({} bytes)",
                    bytes.len()
                ),
                format!(
                    "the README generated from the lock's config and marks ({} bytes)",
                    generated.len()
                ),
            ),
            (None, _) => c.error("README", "no file".into(), "the generated README".into()),
            _ => {}
        }
    } else {
        c.error(
            "README",
            "none".into(),
            "the README the importer generates".into(),
        );
    }
    if let Some(provider) = provider {
        let firmware = provider.firmware();
        let scoped = firmware.for_package(recipe.package.as_ref());
        catalogue_fields(c, config, scoped.as_deref().unwrap_or(firmware));
    }
}

/// What the importer copies from the firmware catalogue into a package's
/// config, which a recipe never states: each ROM's required devices and the
/// package's provenance, the entry's text for open firmware and none for
/// user-supplied. A lock that differs would carry text of its own into the
/// generated config and README.
fn catalogue_fields(
    c: &mut Comparer<'_>,
    config: &firmware::Config,
    firmware: &dyn crate::provider::FirmwareProfile,
) {
    let entry = |rom: &firmware::Rom| {
        rom.catalogue_id
            .as_deref()
            .and_then(|id| firmware.entry(id))
    };
    for (index, rom) in config.roms.iter().enumerate() {
        let mut requires = rom.requires.clone();
        requires.sort();
        let want = entry(rom).map(|e| e.requires).unwrap_or_default();
        c.same(
            format!("rom[{}].requires", index + 1),
            list(&requires),
            list(&want),
        );
    }
    let provenance = |p: &firmware::Provenance| {
        format!(
            "from {:?}, kind {:?}, revision {:?}, url {:?}, date {:?}",
            p.from,
            p.kind.map(firmware::ProvenanceKind::as_str),
            p.revision,
            p.url,
            p.date
        )
    };
    let want = config
        .roms
        .first()
        .and_then(entry)
        .and_then(|e| e.provenance)
        .map(|from| {
            provenance(&firmware::Provenance {
                kind: None,
                from,
                revision: None,
                url: None,
                date: None,
            })
        });
    c.same(
        "package.provenance",
        text(config.provenance.as_ref().map(provenance).as_deref()),
        text(want.as_deref()),
    );
}

/// The licence this program gives the text it generates for an artifact
/// whose every payload is open: the config, the title metadata and a
/// firmware package's README. It is the platform providers' too (a
/// `pc-image` test keeps the two equal).
pub const GENERATED_LICENSE: &str = "CC0-1.0";

/// The locked config, for the licence marks: where the recipe's terms and
/// source extras and a package's ROMs are.
enum MarksConfig<'a> {
    Media(&'a media_set::Config),
    Firmware(&'a firmware::Config),
}

/// The redistribution marks the lock carries, against what the recipe
/// states (machine-images §5, recipes §6.6). The push guard trusts these
/// marks, so a lock may be more restrictive than its recipe, never less:
///
/// - with no `[license]`, no descriptor is open, no descriptor links terms
///   or a source archive, and the manifest names no licences;
/// - with `[license]`, an open payload layer is open under exactly the
///   recipe's expression, and an open layer only the lock holds (the config,
///   the metadata, a package's README: text this program generates) under
///   [`GENERATED_LICENSE`]; a terms or source link names the decoded digest
///   of the recipe's own terms or source extra, and an open payload of a set
///   (a package's ROM: for its source) carries the links the recipe states;
///   the manifest's licence list is the one its open layers give;
/// - an `open` mark without a valid licence is refused anywhere.
fn marks(c: &mut Comparer<'_>, recipe: &Recipe, locked: &Locked, config: MarksConfig<'_>) {
    use image_store::license::{self, Expression};
    let stated_license = recipe.license.as_ref();
    let expression = stated_license.and_then(|l| stated(&l.expression));
    let extra_blob = |id: Option<&str>| -> Option<Digest> {
        let id = id?;
        let extra = recipe.extras.iter().find(|e| stated(&e.id) == Some(id))?;
        let name = stated(&extra.file_name).unwrap_or(id);
        let extras = match &config {
            MarksConfig::Media(config) => &config.extras,
            MarksConfig::Firmware(config) => &config.extras,
        };
        extras
            .iter()
            .find(|e| e.filename == name)
            .map(|e| e.blob.clone())
            .or_else(|| match &config {
                MarksConfig::Firmware(config) => config
                    .sources
                    .iter()
                    .find(|s| s.filename == name)
                    .map(|s| s.blob.clone()),
                MarksConfig::Media(_) => None,
            })
    };
    let terms = extra_blob(stated_license.and_then(|l| stated(&l.terms)));
    let source = extra_blob(stated_license.and_then(|l| stated(&l.source)));
    let roms: BTreeSet<Digest> = match &config {
        MarksConfig::Firmware(config) => config
            .roms
            .iter()
            .flat_map(|r| {
                r.blob
                    .iter()
                    .cloned()
                    .chain(r.parts.iter().map(|p| p.blob.clone()))
            })
            .collect(),
        MarksConfig::Media(_) => BTreeSet::new(),
    };
    let media = matches!(config, MarksConfig::Media(_));
    let manifest = &locked.manifest;
    let generated = generated_layers(manifest, &locked.config);
    let named = std::iter::once(("config".to_owned(), &manifest.config, true)).chain(
        manifest.layers.iter().enumerate().map(|(index, layer)| {
            let name = match export_name::decoded(layer) {
                Some(file) => format!("layer[{}] ({file})", index + 1),
                None => format!("layer[{}]", index + 1),
            };
            (name, layer, generated.contains(&layer.digest))
        }),
    );
    for (name, descriptor, generated) in named {
        let claim = license::open_license(descriptor);
        let marked_open = descriptor
            .annotations
            .get(license::REDISTRIBUTION)
            .is_some_and(|v| v == license::OPEN);
        let field = format!("{name}.license");
        match (expression, &claim) {
            (_, None) if marked_open => c.error(
                field.clone(),
                "marked open without a valid licence".into(),
                "user-supplied".into(),
            ),
            (_, None) => {}
            (None, Some(open)) => c.error(
                field.clone(),
                format!("open under {}", open.as_str()),
                "user-supplied: the recipe states no [license]".into(),
            ),
            (Some(mine), Some(open)) => {
                let want = if generated { GENERATED_LICENSE } else { mine };
                if open.as_str() != want {
                    c.error(
                        field.clone(),
                        format!("open under {}", open.as_str()),
                        format!("open under {want} at most"),
                    );
                }
            }
        }
        let decoded = descriptor.decoded_digest().ok();
        let payload_open = expression.is_some() && claim.is_some() && !generated;
        for (key, want, what) in [
            (license::TERMS, &terms, "terms"),
            (license::SOURCE, &source, "source"),
        ] {
            let required = payload_open
                && (media
                    || (key == license::SOURCE
                        && decoded.as_ref().is_some_and(|d| roms.contains(d))));
            match (descriptor.annotations.get(key), want) {
                (Some(link), Some(want)) if *link == want.to_string() => {}
                (Some(link), want) => c.error(
                    format!("{name}.{what}"),
                    format!("a link to {link}"),
                    match want {
                        Some(want) => format!("a link to the recipe's {what} extra, {want}"),
                        None => format!("no link: the recipe states no {what}"),
                    },
                ),
                (None, Some(want)) if required => c.error(
                    format!("{name}.{what}"),
                    "no link".into(),
                    format!("a link to the recipe's {what} extra, {want}"),
                ),
                (None, _) => {}
            }
        }
    }
    // The manifest's own annotations: the licence list of its open layers,
    // and none of the descriptor marks, which no importer writes there.
    const LICENSES: &str = "org.opencontainers.image.licenses";
    for key in [
        license::REDISTRIBUTION,
        license::LICENSE,
        license::TERMS,
        license::SOURCE,
    ] {
        if let Some(value) = manifest.annotations.get(key) {
            c.error(
                format!("manifest.{key}"),
                format!("{value:?}"),
                "nothing: a mark belongs to a descriptor".into(),
            );
        }
    }
    let open: Vec<Expression> = manifest
        .layers
        .iter()
        .filter_map(license::open_license)
        .collect();
    let want = expression
        .and_then(|_| Expression::all(&open))
        .map(|all| all.as_str().to_owned());
    if let Some(have) = manifest.annotations.get(LICENSES) {
        if Some(have) != want.as_ref() {
            c.error(
                format!("manifest.{LICENSES}"),
                format!("{have:?}"),
                text(want.as_deref()),
            );
        }
    }
}

/// The title record and the set labels, field by field.
fn title(c: &mut Comparer<'_>, merged: &Merged, locked: &metadata::Metadata) {
    let [record] = locked.titles.as_slice() else {
        c.error(
            "title",
            format!("{} title records", locked.titles.len()),
            "one".into(),
        );
        return;
    };
    let now = merged.title();
    let recipe = Some(Origin::Recipe);
    let one = |v: &Option<String>| text(v.as_deref());
    c.merged(
        "title.id",
        text(Some(&record.id)),
        text(Some(&now.id)),
        recipe,
    );
    c.merged(
        "title.title",
        text(Some(&record.title)),
        text(Some(&now.title)),
        Some(merged.title.from),
    );
    for (key, locked, today, from) in [
        (
            "sort_title",
            &record.sort_title,
            &now.sort_title,
            origin(&merged.sort_title),
        ),
        (
            "series",
            &record.series,
            &now.series,
            origin(&merged.series),
        ),
        (
            "software_platform",
            &record.platform,
            &now.platform,
            origin(&merged.software_platform),
        ),
        (
            "description",
            &record.description,
            &now.description,
            origin(&merged.description),
        ),
        ("version", &record.version, &now.version, recipe),
        ("edition", &record.edition, &now.edition, recipe),
        (
            "source_media",
            &record.source_media,
            &now.source_media,
            recipe,
        ),
        (
            "requirements",
            &record.requirements,
            &now.requirements,
            recipe,
        ),
        (
            "synopsis",
            &record.synopsis,
            &now.synopsis,
            merged.synopsis.as_ref().map(|_| Origin::Work),
        ),
    ] {
        c.merged(format!("title.{key}"), one(locked), one(today), from);
    }
    c.merged(
        "title.work",
        one(&record.work_id),
        one(&now.work_id),
        recipe,
    );
    c.merged(
        "title.year",
        record
            .release_year
            .map_or_else(|| "nothing".into(), |y| y.to_string()),
        now.release_year
            .map_or_else(|| "nothing".into(), |y| y.to_string()),
        recipe,
    );
    let players = |p: &Option<metadata::Players>| {
        p.as_ref()
            .map_or_else(|| "nothing".into(), |p| format!("{} to {}", p.min, p.max))
    };
    c.merged(
        "title.players",
        players(&record.players),
        players(&now.players),
        origin(&merged.players),
    );
    for (key, locked, today, from) in [
        (
            "publishers",
            &record.publishers,
            &now.publishers,
            origin(&merged.publishers),
        ),
        (
            "developers",
            &record.developers,
            &now.developers,
            origin(&merged.developers),
        ),
        (
            "genres",
            &record.genres,
            &now.genres,
            origin(&merged.genres),
        ),
        ("languages", &record.languages, &now.languages, recipe),
        ("regions", &record.regions, &now.regions, recipe),
    ] {
        c.merged(format!("title.{key}"), list(locked), list(today), from);
    }
    let kinds: BTreeSet<&String> = record.needs.keys().chain(now.needs.keys()).collect();
    for kind in kinds {
        let from = merged
            .needs
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, v)| v.from);
        c.merged(
            format!("needs.{kind}"),
            list(record.needs.get(kind).map_or(&[][..], Vec::as_slice)),
            list(now.needs.get(kind).map_or(&[][..], Vec::as_slice)),
            from,
        );
    }
    let catalogues: BTreeSet<&String> = record.work_ids.keys().chain(now.work_ids.keys()).collect();
    for catalogue in catalogues {
        let from = merged
            .ids
            .iter()
            .find(|(k, _)| k == catalogue)
            .map(|(_, v)| v.from);
        c.merged(
            format!("ids.{catalogue}"),
            text(record.work_ids.get(catalogue).map(String::as_str)),
            text(now.work_ids.get(catalogue).map(String::as_str)),
            from,
        );
    }
    // Labels are the union of the work file's and the recipe's: an error
    // when a label the recipe itself states is not in the lock, drift when
    // the work file states labels and the rest of the difference is its.
    let today: Vec<String> = merged.labels.iter().map(|l| l.value.clone()).collect();
    if today != locked.labels {
        let missing = merged
            .labels
            .iter()
            .any(|l| l.from == Origin::Recipe && !locked.labels.contains(&l.value));
        c.merged(
            "title.labels",
            list(&locked.labels),
            list(&today),
            if missing {
                Some(Origin::Recipe)
            } else if merged.labels.iter().any(|l| l.from == Origin::Work) {
                Some(Origin::Work)
            } else {
                None
            },
        );
    }
    for (key, present) in [
        ("tags", !record.tags.is_empty()),
        ("cover", record.cover.is_some()),
        ("screenshots", !record.screenshots.is_empty()),
        ("launch", record.launch.is_some()),
    ] {
        if present {
            c.error(format!("title.{key}"), "a value".into(), "nothing".into());
        }
    }
}

/// One payload of the locked manifest and the bytes a step made for it.
#[derive(Clone, Debug)]
pub struct Payload {
    /// The output, in the recipe's terms: `medium disk-1 (x86BOOT.img)`.
    pub what: String,
    pub descriptor: Descriptor,
    pub bytes: Bytes,
}

/// The payload types this build encodes, to look a locked `+zstd`
/// descriptor's encoding up by its media type.
fn payload_type(media_type: &str) -> Option<PayloadType> {
    codec::MEDIA_PAYLOAD_TYPES
        .iter()
        .chain(codec::FIRMWARE_PAYLOAD_TYPES)
        .find(|t| t.media_type == media_type)
        .copied()
}

/// The payload types a descriptor's stored encoding needs: none for a raw
/// blob, its own `+zstd` type otherwise.
fn codecs(descriptor: &Descriptor) -> Result<Vec<PayloadType>, String> {
    if !descriptor.is_compressed() {
        return Ok(Vec::new());
    }
    payload_type(&descriptor.media_type)
        .map(|t| vec![t])
        .ok_or_else(|| {
            format!(
                "the encoding {} is not one this build writes",
                descriptor.media_type
            )
        })
}

/// Points 4 and 5: match every output the steps made with its descriptor in
/// the locked manifest, by the locked config, and check its decoded and
/// stored SHA-256 and size. Writes nothing. `Err` lists every mismatch, each
/// naming the output, the expected and the actual hash.
pub fn verify(
    recipe: &Recipe,
    locked: &Locked,
    built: &Built,
) -> Result<Vec<Payload>, Vec<String>> {
    let mut errors = Vec::new();
    // Each output with the decoded digest the locked config gives it.
    let mut wanted: Vec<(String, Option<Digest>, Bytes)> = Vec::new();
    match recipe.kind {
        Kind::Media => match media_set::Config::decode(&locked.config) {
            Ok(config) => {
                if config.media.len() != built.media.len() {
                    errors.push(format!(
                        "the lock has {} media, the steps made {}",
                        config.media.len(),
                        built.media.len()
                    ));
                }
                for (medium, locked) in built.media.iter().zip(&config.media) {
                    wanted.push((
                        format!("medium {} ({})", medium.id, medium.file_name),
                        Some(locked.blob.clone()),
                        medium.bytes.clone(),
                    ));
                }
                for extra in &built.extras {
                    let blob = config
                        .extras
                        .iter()
                        .find(|e| e.filename == extra.file_name)
                        .map(|e| e.blob.clone());
                    wanted.push((
                        format!("extra {} ({})", extra.id, extra.file_name),
                        blob,
                        extra.bytes.clone(),
                    ));
                }
            }
            Err(e) => errors.push(format!("the locked set config: {e}")),
        },
        Kind::Firmware => match firmware::Config::decode(&locked.config) {
            Ok(config) => {
                if config.roms.len() != built.roms.len() {
                    errors.push(format!(
                        "the lock has {} ROMs, the steps made {}",
                        config.roms.len(),
                        built.roms.len()
                    ));
                }
                for (rom, locked) in built.roms.iter().zip(&config.roms) {
                    let blobs: Vec<Digest> = match &locked.blob {
                        Some(blob) => vec![blob.clone()],
                        None => locked.parts.iter().map(|p| p.blob.clone()).collect(),
                    };
                    if blobs.len() != rom.files.len() {
                        errors.push(format!(
                            "rom {}: the lock has {} files, the steps made {}",
                            rom.id,
                            blobs.len(),
                            rom.files.len()
                        ));
                    }
                    for (file, blob) in rom.files.iter().zip(blobs) {
                        wanted.push((
                            format!("rom {} ({})", rom.id, file.name),
                            Some(blob),
                            file.bytes.clone(),
                        ));
                    }
                }
                for extra in &built.extras {
                    let blob = config
                        .extras
                        .iter()
                        .find(|e| e.filename == extra.file_name)
                        .map(|e| e.blob.clone())
                        .or_else(|| {
                            config
                                .sources
                                .iter()
                                .find(|s| s.filename == extra.file_name)
                                .map(|s| s.blob.clone())
                        });
                    wanted.push((
                        format!("extra {} ({})", extra.id, extra.file_name),
                        blob,
                        extra.bytes.clone(),
                    ));
                }
            }
            Err(e) => errors.push(format!("the locked firmware config: {e}")),
        },
    }
    let mut payloads: Vec<Payload> = Vec::new();
    // Layers an output stands for, matched or not: a mismatch is reported
    // once, at its output.
    let mut claimed: BTreeSet<Digest> = BTreeSet::new();
    for (what, blob, bytes) in wanted {
        let Some(blob) = blob else {
            errors.push(format!("{what}: the locked config names no such file"));
            continue;
        };
        let Some(descriptor) = layer(&locked.manifest, &blob) else {
            errors.push(format!(
                "{what}: the locked manifest has no layer of content {blob}"
            ));
            continue;
        };
        claimed.insert(descriptor.digest.clone());
        let expected_size = descriptor.decoded_size().unwrap_or(u64::MAX);
        let actual = Digest::of(&bytes);
        if actual != blob || bytes.len() as u64 != expected_size {
            errors.push(format!(
                "{what}: the lock expects {blob} ({expected_size} bytes), the steps made {actual} ({} bytes); \
                 every input matched its SHA-256, so a step no longer writes the bytes it wrote when the recipe was pinned: please report it",
                bytes.len()
            ));
            continue;
        }
        let stored = codecs(descriptor).and_then(|codecs| {
            codec::describe(
                bytes.as_slice(),
                descriptor.decoded_media_type(),
                bytes.len() as u64,
                &codecs,
            )
            .map_err(|e| e.to_string())
        });
        match stored {
            Ok(stored) if stored.digest == descriptor.digest && stored.size == descriptor.size => {}
            Ok(stored) => errors.push(format!(
                "{what}: the lock stores it as {} ({} bytes), this build encodes {} ({} bytes)",
                descriptor.digest, descriptor.size, stored.digest, stored.size
            )),
            Err(e) => errors.push(format!("{what}: {e}")),
        }
        if !payloads
            .iter()
            .any(|p| p.descriptor.digest == descriptor.digest)
        {
            payloads.push(Payload {
                what,
                descriptor: descriptor.clone(),
                bytes,
            });
        }
    }
    for layer in &locked.manifest.layers {
        if !claimed.contains(&layer.digest) && !locked.blobs.contains_key(&layer.digest) {
            errors.push(format!(
                "the locked manifest's layer {} ({}) is neither made by a step nor held by the lock",
                layer.digest, layer.media_type
            ));
        }
    }
    if errors.is_empty() {
        Ok(payloads)
    } else {
        Err(errors)
    }
}

/// Write a verified lock's artifact into `store`: the payloads, encoded as
/// their descriptors say, every lock file and the manifest, without a tag.
/// Each write checks its digest; the store keeps a blob it already holds.
pub fn store(store: &Store, locked: &Locked, payloads: &[Payload]) -> Result<Descriptor, String> {
    for payload in payloads {
        let descriptor = &payload.descriptor;
        let written = store
            .import_payload(
                payload.bytes.as_slice(),
                descriptor.decoded_media_type(),
                payload.bytes.len() as u64,
                Some(&Digest::of(&payload.bytes)),
                &codecs(descriptor)?,
            )
            .map_err(|e| format!("{}: {e}", payload.what))?;
        if written.digest != descriptor.digest {
            return Err(format!(
                "{}: stored as {}, the lock says {}",
                payload.what, written.digest, descriptor.digest
            ));
        }
    }
    for (digest, bytes) in &locked.blobs {
        store
            .import(bytes.as_slice(), Some(digest))
            .map_err(|e| format!("the lock file {}: {e}", digest.hex()))?;
    }
    store
        .import(locked.manifest_bytes.as_slice(), Some(&locked.digest))
        .map_err(|e| format!("the locked manifest {}: {e}", locked.digest))?;
    Ok(Descriptor::new(
        media_type::MANIFEST,
        locked.digest.clone(),
        locked.manifest_bytes.len() as u64,
    ))
}

/// Tag `manifest`, already stored with its whole closure, as `reference`
/// with the platform's `validator`, which resolves it and so verifies every
/// blob once more. A tag that already names it is reported as present; one
/// that names another image is refused, as a revision tag never moves.
pub fn tag(
    store: &Store,
    reference: &str,
    manifest: &Descriptor,
    validator: &dyn image_store::ConfigValidator,
) -> Result<crate::build::Published, String> {
    let present = |descriptor: &Descriptor| crate::build::Published {
        descriptor: descriptor.clone(),
        created: false,
    };
    match store.tag_descriptor(reference) {
        Ok(existing) if existing.digest == manifest.digest => return Ok(present(manifest)),
        Ok(existing) => {
            return Err(format!(
                "{reference} already names {}; a revision tag never moves",
                existing.digest
            ))
        }
        Err(image_store::Error::TagNotFound(_)) => {}
        Err(e) => return Err(e.to_string()),
    }
    store.check_new_tag(reference).map_err(|e| e.to_string())?;
    let package = validator.artifact_type() == media_type::FIRMWARE;
    if let Some(problem) = image_store::tag::new_image_problem(reference, package) {
        return Err(problem);
    }
    match store.update_tag(reference, manifest, None, validator) {
        Ok(()) => Ok(crate::build::Published {
            descriptor: manifest.clone(),
            created: true,
        }),
        Err(image_store::Error::Conflict {
            current: Some(current),
        }) if current == manifest.digest => Ok(present(manifest)),
        Err(e) => Err(e.to_string()),
    }
}

/// The text of a recipe file with `digest = "<digest>"` on the line after
/// `revision`, in the file's own line ends. `revision` is the byte span of
/// the revision's value.
pub fn with_digest(text: &str, revision: &std::ops::Range<usize>, digest: &str) -> String {
    let end = text[revision.end..]
        .find('\n')
        .map_or(text.len(), |n| revision.end + n + 1);
    let crlf = text[..end].ends_with("\r\n");
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(text.len() + 80);
    out.push_str(&text[..end]);
    if !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&format!("digest = {digest:?}{eol}"));
    out.push_str(&text[end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("recipe-lock-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_lock_folder_sits_beside_the_recipe() {
        assert_eq!(
            dir(Path::new("a/b/x.media.toml")),
            PathBuf::from("a/b/x.media.toml.lock")
        );
    }

    #[test]
    fn a_lock_is_written_as_files_named_by_their_hash_and_read_back() {
        let t = Temp::new("rw");
        let lock = t.0.join("x.media.toml.lock");
        let files = vec![b"manifest".to_vec(), b"config".to_vec()];
        write(&lock, &files).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(lock.join(FILES))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        let mut expected = vec![hex(b"manifest"), hex(b"config")];
        expected.sort();
        assert_eq!(names, expected);
        let read = read(&lock).unwrap();
        assert_eq!(read.get(&hex(b"config")).unwrap(), b"config");
        // Written again with other files: the old lock is replaced whole and
        // nothing is left beside it.
        write(&lock, &[b"other".to_vec()]).unwrap();
        assert_eq!(super::read(&lock).unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 1);
    }

    #[test]
    fn a_lock_file_whose_bytes_are_not_its_name_is_refused() {
        let t = Temp::new("tamper");
        let lock = t.0.join("x.media.toml.lock");
        write(&lock, &[b"config".to_vec()]).unwrap();
        std::fs::write(lock.join(FILES).join(hex(b"config")), b"changed").unwrap();
        let error = read(&lock).unwrap_err();
        assert!(error.contains("not the ones its name says"), "{error}");
        std::fs::write(lock.join("notes.txt"), b"x").unwrap();
        assert!(read(&lock).unwrap_err().contains("holds notes.txt"));
    }

    /// A payload layer is never classed as generated text, even when a
    /// forged lock holds its bytes: an open recipe's medium relabelled
    /// `CC0-1.0`, the generated text's licence, is still held to the
    /// recipe's expression.
    #[test]
    fn a_payload_whose_bytes_a_lock_holds_is_still_a_payload() {
        let Some(crate::model::Document::Recipe(recipe)) = crate::parse_text(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/x\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n[title]\ntitle = \"X\"\n\n[license]\nexpression = \"MIT\"\n\n[input.disk]\nname = \"x.img\"\nsize = 4\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n\n[[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n",
        ) else {
            panic!("the recipe does not read");
        };
        let disk = b"disk".to_vec();
        let config = media_set::Config {
            schema: 1,
            platform: "ibm-pc".into(),
            media: vec![media_set::Medium {
                id: "disk-1".into(),
                label: "Disk 1".into(),
                format: toml::Table::new(),
                blob: Digest::of(&disk),
            }],
            extras: Vec::new(),
        }
        .encode()
        .unwrap();
        let mut layer = Descriptor::new(media_type::FLOPPY, Digest::of(&disk), 4);
        image_store::license::mark(
            &mut layer,
            Some(&image_store::license::Expression::parse(GENERATED_LICENSE).unwrap()),
        );
        let manifest = Manifest::new_media_set(
            "ibm-pc",
            Descriptor::new(
                media_type::MEDIA_SET_CONFIG,
                Digest::of(&config),
                config.len() as u64,
            ),
            vec![
                layer,
                Descriptor::new(media_type::METADATA, Digest::of(b"{}"), 2),
            ],
        )
        .unwrap();
        let manifest_bytes = manifest.encode().unwrap();
        let locked = Locked {
            digest: Digest::of(&manifest_bytes),
            manifest_bytes,
            manifest,
            config: config.clone(),
            metadata: None,
            blobs: [(Digest::of(&config), config), (Digest::of(&disk), disk)]
                .into_iter()
                .collect(),
        };
        let errors: Vec<String> = compare(&recipe, None, &locked, None)
            .errors
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(
            errors.contains(
                &"layer[1].license: the lock has open under CC0-1.0, the recipe has open under MIT at most"
                    .to_owned()
            ),
            "{errors:#?}"
        );
    }

    #[test]
    fn the_digest_goes_on_the_line_after_the_revision_in_the_files_line_ends() {
        let text = "[media]\nname = \"x\"\nrevision = 1 # first\n\n[title]\n";
        let at = text.find("1 #").unwrap();
        assert_eq!(
            with_digest(text, &(at..at + 1), "sha256:ab"),
            "[media]\nname = \"x\"\nrevision = 1 # first\ndigest = \"sha256:ab\"\n\n[title]\n"
        );
        let crlf = text.replace('\n', "\r\n");
        let at = crlf.find("1 #").unwrap();
        assert_eq!(
            with_digest(&crlf, &(at..at + 1), "sha256:ab"),
            "[media]\r\nname = \"x\"\r\nrevision = 1 # first\r\ndigest = \"sha256:ab\"\r\n\r\n[title]\r\n"
        );
        let last = "[media]\nrevision = 2";
        assert_eq!(
            with_digest(last, &(19..20), "sha256:cd"),
            "[media]\nrevision = 2\ndigest = \"sha256:cd\"\n"
        );
    }
}
