// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installation-set import shared by `image media import` and the desktop
//! library; machine-images §5.2. The user's confirmed order is authoritative:
//! array position defines disk order, never file names or labels. Sources are
//! copied by the store's streaming, full-SHA-256 ingestion path and stay
//! unchanged; host paths never enter the config, metadata or annotations.
//! Floppy geometry follows the IBM Personal Computer AT Technical Reference,
//! Diskette Drive Adapter, through [`crate::media_set`].

use crate::media_set::{self as pc, PcMediaFormats};
use image_store::{
    codec,
    media_set::{valid_text, Config, Extra, ExtraKind, MediaFormats, Medium, Validator},
    media_type as mt,
    metadata::{Metadata, Title},
    Descriptor, Digest, Manifest, Store,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fmt, fs,
    path::{Path, PathBuf},
};

/// A refusal reads as `image media import` reports it: the failing source or
/// extra first, then the cause chain.
#[derive(Debug)]
pub struct Refusal(String);
impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Refusal {}
impl From<image_store::Error> for Refusal {
    fn from(error: image_store::Error) -> Self {
        Self(chain(&error))
    }
}
pub type Result<T> = std::result::Result<T, Refusal>;

fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
trait Context<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T>;
}
impl<T, E: std::error::Error> Context<T> for std::result::Result<T, E> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| Refusal(format!("{}: {}", what(), chain(&e))))
    }
}
fn ensure(condition: bool, message: impl FnOnce() -> String) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Refusal(message()))
    }
}

/// One medium in confirmed order. `None` keeps the command line's defaults:
/// the file name as label and the floppy format detected from the exact size.
#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub label: Option<String>,
    pub format: Option<String>,
}
/// A loose file stored with the set; `None` derives the kind from the extension.
#[derive(Clone, Debug)]
pub struct ExtraSource {
    pub path: PathBuf,
    pub dos_name: Option<String>,
    pub kind: Option<ExtraKind>,
}
/// Everything one import publishes under `tag`.
#[derive(Clone, Debug)]
pub struct Request {
    pub tag: String,
    /// The set's one catalogue title; its id is always `title`.
    pub title: Title,
    pub labels: Vec<String>,
    pub media: Vec<Source>,
    pub extras: Vec<ExtraSource>,
    pub compress: bool,
}

/// A folder's candidate images in stable file-name order, or the refusal of
/// the folder, with the hidden files skipped before either was decided.
#[derive(Debug)]
pub struct Listing {
    pub hidden: Vec<OsString>,
    pub files: Result<Vec<PathBuf>>,
}

/// Hidden files are skipped and reported. Anything else that is not a regular
/// media candidate refuses the whole folder, so a stray file never becomes a
/// disk; an explicit file list is the way to import unusual names.
pub fn list_folder(directory: &Path) -> Listing {
    let mut hidden = Vec::new();
    let files = list_candidates(directory, &mut hidden);
    Listing { hidden, files }
}
fn list_candidates(directory: &Path, hidden: &mut Vec<OsString>) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(directory)
        .context(|| format!("directory {:?}", directory))?
        .collect::<std::io::Result<Vec<_>>>()
        .context(|| format!("directory {:?}", directory))?;
    entries.sort_by_key(|e| e.file_name());
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.path();
        if entry.file_name().as_encoded_bytes().starts_with(b".") {
            hidden.push(entry.file_name());
            continue;
        }
        let stat = fs::metadata(&path).context(|| format!("source {:?}", path))?;
        ensure(stat.is_file(), || {
            format!("source {:?}: expected a regular file", path)
        })?;
        ensure(
            matches!(extension(&path).as_str(), "img" | "ima" | "iso")
                || pc::detect(stat.len()).is_some(),
            || {
                format!(
                    "source {:?}: not a media candidate; use an explicit file list",
                    path
                )
            },
        )?;
        files.push(path);
    }
    Ok(files)
}

fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

/// The exact byte length of a medium source, refused unless it is a regular file.
pub fn source_size(path: &Path) -> Result<u64> {
    regular_size(path, "source")
}
/// The exact byte length of an extra, refused unless it is a regular file.
pub fn extra_size(path: &Path) -> Result<u64> {
    regular_size(path, "extra")
}
fn regular_size(path: &Path, what: &str) -> Result<u64> {
    let stat = fs::metadata(path).context(|| format!("{what} {:?}", path))?;
    ensure(stat.is_file(), || {
        format!("{what} {:?}: expected a regular file", path)
    })?;
    Ok(stat.len())
}

/// Every format a PC installation set may declare, floppies first.
pub fn format_names() -> Vec<&'static str> {
    assembly::descriptors::FLOPPY_TYPES
        .iter()
        .map(|f| f.name)
        .chain(["iso-2048"])
        .collect()
}
/// A proposal for review, never a silent choice: the floppy format of the exact
/// size, or ISO-2048 for an `.iso` file of whole 2048-byte sectors.
pub fn propose_format(path: &Path, size: u64) -> Option<&'static str> {
    pc::detect(size).or_else(|| {
        (extension(path) == "iso" && size > 0 && size.is_multiple_of(2048)).then_some("iso-2048")
    })
}
/// The kind the command line gives an extra with this file name.
pub fn extra_kind(path: &Path) -> ExtraKind {
    match extension(path).as_str() {
        "txt" | "pdf" | "md" | "doc" => ExtraKind::Document,
        "zip" | "7z" | "arj" | "rar" => ExtraKind::Archive,
        "exe" | "com" | "bat" => ExtraKind::Executable,
        _ => ExtraKind::Other,
    }
}

/// A validated import: config with placeholder blobs, metadata and the payload
/// list, before any store object is written.
#[derive(Clone, Debug)]
pub struct Plan {
    tag: String,
    title: String,
    config: Config,
    metadata: Metadata,
    media: Vec<(PathBuf, &'static str, u64)>,
    extras: Vec<PathBuf>,
    compress: bool,
}
impl Plan {
    /// Media and extras in publication order; blobs are placeholders until import.
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn tag(&self) -> &str {
        &self.tag
    }
    /// Each medium's source beside its config entry, in disk order.
    pub fn media(&self) -> impl Iterator<Item = (&Path, &Medium)> {
        self.media
            .iter()
            .map(|(path, _, _)| path.as_path())
            .zip(&self.config.media)
    }
}

/// Validate every source, label, format, extra name and the metadata without
/// touching a store, so a refused import leaves no blobs behind.
pub fn plan(request: &Request) -> Result<Plan> {
    ensure(valid_text(&request.title.title), || {
        format!("invalid title {:?}", request.title.title)
    })?;
    let empty = Digest::of(b"");
    let mut config = Config {
        schema: 1,
        platform: PcMediaFormats.platform().into(),
        media: Vec::new(),
        extras: Vec::new(),
    };
    let mut media = Vec::new();
    for (index, source) in request.media.iter().enumerate() {
        let path = &source.path;
        let size = source_size(path)?;
        let name = source
            .format
            .as_deref()
            .or_else(|| pc::detect(size))
            .ok_or_else(|| {
                Refusal(format!(
                    "source {:?}: unknown media size; use --format N=FORMAT",
                    path
                ))
            })?;
        let format = pc::format(name).context(|| format!("source {:?}", path))?;
        let kind = PcMediaFormats
            .validate_format(&format, size)
            .context(|| format!("source {:?}", path))?;
        let label = source.label.clone().unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        ensure(valid_text(&label), || {
            format!("source {:?}: invalid label {:?}", path, label)
        })?;
        config.media.push(Medium {
            id: format!("disk-{}", index + 1),
            label,
            format,
            blob: empty.clone(),
        });
        media.push((path.clone(), kind, size));
    }
    for extra in &request.extras {
        let path = &extra.path;
        let size = extra_size(path)?;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Refusal(format!("extra {:?}: filename must be UTF-8", path)))?
            .to_owned();
        ensure(valid_text(&filename), || {
            format!("extra {:?}: invalid filename", path)
        })?;
        if let Some(name) = &extra.dos_name {
            PcMediaFormats
                .validate_name(name)
                .context(|| format!("extra {:?}", path))?;
        }
        config.extras.push(Extra {
            blob: empty.clone(),
            filename,
            size,
            kind: extra.kind.unwrap_or_else(|| extra_kind(path)),
            dos_name: extra.dos_name.clone(),
        });
    }
    config.encode().context(|| {
        format!(
            "source list {:?}, extras {:?}",
            request.media.iter().map(|s| &s.path).collect::<Vec<_>>(),
            request
                .extras
                .iter()
                .map(|e| (&e.path, &e.dos_name))
                .collect::<Vec<_>>()
        )
    })?;
    let mut metadata = Metadata::new(&config.platform);
    metadata.labels.clone_from(&request.labels);
    metadata.titles.push(Title {
        id: "title".into(),
        ..request.title.clone()
    });
    metadata.encode(&[])?;
    Ok(Plan {
        tag: request.tag.clone(),
        title: request.title.title.clone(),
        config,
        metadata,
        media,
        extras: request.extras.iter().map(|e| e.path.clone()).collect(),
        compress: request.compress,
    })
}

/// What a publication found under the plan's tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Imported(Descriptor),
    /// The tag already names exactly this set; nothing was written.
    AlreadyImported(Descriptor),
}
impl Outcome {
    pub fn descriptor(&self) -> &Descriptor {
        match self {
            Self::Imported(d) | Self::AlreadyImported(d) => d,
        }
    }
}

fn user_supplied(mut d: Descriptor) -> Descriptor {
    d.annotations
        .insert("unemu.redistribution".into(), "user-supplied".into());
    d
}

fn conflict(tag: &str, what: &str) -> Refusal {
    Refusal(format!(
        "tag conflict: {tag} already names {what}; choose another tag"
    ))
}
fn cancel_check(cancelled: &dyn Fn() -> bool, what: &str, path: &Path) -> Result<()> {
    ensure(!cancelled(), || {
        format!(
            "import cancelled before {what} {:?}; no tag was written",
            path
        )
    })
}
const TITLE: &str = "org.opencontainers.image.title";

/// Copy and fully hash each source in order, then publish the tag last. When
/// the tag already names a set, nothing is written: the same media, labels,
/// formats, extras and metadata are already imported, anything else is a tag
/// conflict. `cancelled` is checked before each file and a cancelled import
/// writes no tag. `progress` receives files done and the file total.
pub fn publish(
    store: &Store,
    plan: &Plan,
    progress: &dyn Fn(usize, usize),
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome> {
    match store.tag_descriptor(&plan.tag) {
        Ok(existing) => return compare_existing(store, plan, &existing, progress, cancelled),
        Err(image_store::Error::TagNotFound(_)) => store.check_new_tag(&plan.tag)?,
        Err(error) => return Err(error.into()),
    }
    let codecs = if plan.compress {
        codec::MEDIA_PAYLOAD_TYPES
    } else {
        &[]
    };
    let total = plan.media.len() + plan.extras.len();
    let payload =
        |done: usize, what: &str, path: &Path, kind: &str, size: u64| -> Result<Descriptor> {
            cancel_check(cancelled, what, path)?;
            progress(done, total);
            let file = fs::File::open(path).context(|| format!("{what} {:?}", path))?;
            let blob = store
                .import_payload(file, kind, size, None, codecs)
                .context(|| format!("{what} {:?}", path))?;
            ensure(blob.decoded_size()? == size, || {
                format!("{what} {:?}: length changed during import", path)
            })?;
            Ok(blob)
        };
    let mut config = plan.config.clone();
    let mut layers = Vec::new();
    for (index, (path, kind, size)) in plan.media.iter().enumerate() {
        let blob = payload(index, "source", path, kind, *size)?;
        config.media[index].blob = blob.decoded_digest()?;
        layers.push(user_supplied(blob));
    }
    for (index, path) in plan.extras.iter().enumerate() {
        let size = config.extras[index].size;
        let blob = payload(plan.media.len() + index, "extra", path, mt::EXTRA, size)?;
        config.extras[index].blob = blob.decoded_digest()?;
        layers.push(user_supplied(blob));
    }
    progress(total, total);
    let mut seen = BTreeSet::new();
    let mut unique = Vec::<Descriptor>::new();
    for d in layers {
        if seen.insert(d.digest.clone()) {
            unique.push(d);
        } else {
            let paths: Vec<_> = plan.media.iter().map(|(path, _, _)| path).collect();
            ensure(
                unique.iter().any(|p| {
                    p.digest == d.digest && p.media_type == d.media_type && p.size == d.size
                }),
                || {
                    format!(
                        "source list {:?}: identical bytes used with conflicting payload types",
                        paths
                    )
                },
            )?;
        }
    }
    let small = |kind: &str, bytes: &[u8]| -> Result<Descriptor> {
        let blob = store.import(bytes, None)?;
        Ok(user_supplied(Descriptor::new(kind, blob.digest, blob.size)))
    };
    let bytes = plan.metadata.encode(&unique)?;
    unique.push(small(mt::METADATA, &bytes)?);
    let cfg = small(mt::MEDIA_SET_CONFIG, &config.encode()?)?;
    let mut manifest = Manifest::new_media_set(&config.platform, cfg, unique)?;
    manifest
        .annotations
        .insert(TITLE.into(), plan.title.clone());
    let manifest = store.import_manifest(&manifest)?;
    match store.update_tag(&plan.tag, &manifest, None, &Validator(&PcMediaFormats)) {
        Ok(()) => Ok(Outcome::Imported(manifest)),
        // Another import of the same set published the tag first.
        Err(image_store::Error::Conflict {
            current: Some(current),
        }) if current == manifest.digest => Ok(Outcome::AlreadyImported(manifest)),
        Err(image_store::Error::Conflict {
            current: Some(current),
        }) => Err(conflict(
            &plan.tag,
            &format!("a different installation set ({current})"),
        )),
        Err(error) => Err(error.into()),
    }
}

/// An existing tag is compared without writing anything. The stored set is
/// resolved first; anything but an installation set, or a set whose metadata,
/// title, disk count, labels, formats or extras differ, is refused before a
/// source is read. Only then is each source's raw SHA-256 compared with the
/// stored config's decoded digests, so how the set was stored (compressed or
/// not) never decides the outcome.
fn compare_existing(
    store: &Store,
    plan: &Plan,
    existing: &Descriptor,
    progress: &dyn Fn(usize, usize),
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome> {
    use image_store::BlobSource;
    let manifest =
        Manifest::decode_for_summary(&store.read_verified(existing, image_store::MAX_SMALL_BLOB)?)?;
    let kind = match manifest.artifact_type() {
        mt::MEDIA_SET => None,
        mt::MACHINE => Some("a machine image"),
        mt::FIRMWARE => Some("a firmware package"),
        _ => Some("another kind of artifact"),
    };
    if let Some(kind) = kind {
        return Err(conflict(
            &plan.tag,
            &format!("{kind}, not an installation set"),
        ));
    }
    let resolved = store.resolve_descriptor(existing, &Validator(&PcMediaFormats))?;
    let stored = Config::decode(&resolved.config)?;
    let different = || {
        conflict(
            &plan.tag,
            &format!("a different installation set ({})", existing.digest),
        )
    };
    let mut layout = stored.clone();
    let placeholder = Digest::of(b"");
    for medium in &mut layout.media {
        medium.blob.clone_from(&placeholder);
    }
    for extra in &mut layout.extras {
        extra.blob.clone_from(&placeholder);
    }
    if layout != plan.config
        || resolved.metadata != plan.metadata.encode(&[])?
        || resolved.manifest.annotations.get(TITLE) != Some(&plan.title)
    {
        return Err(different());
    }
    let total = plan.media.len() + plan.extras.len();
    let sources = plan
        .media
        .iter()
        .map(|(path, kind, size)| ("source", path, *kind, *size))
        .chain(
            plan.extras
                .iter()
                .zip(&plan.config.extras)
                .map(|(path, extra)| ("extra", path, mt::EXTRA, extra.size)),
        );
    let wanted = stored
        .media
        .iter()
        .map(|m| &m.blob)
        .chain(stored.extras.iter().map(|e| &e.blob));
    for (done, ((what, path, kind, size), wanted)) in sources.zip(wanted).enumerate() {
        cancel_check(cancelled, what, path)?;
        progress(done, total);
        let file = fs::File::open(path).context(|| format!("{what} {:?}", path))?;
        let raw =
            codec::describe(file, kind, size, &[]).context(|| format!("{what} {:?}", path))?;
        if &raw.digest != wanted {
            return Err(different());
        }
    }
    progress(total, total);
    Ok(Outcome::AlreadyImported(Descriptor::new(
        mt::MANIFEST,
        existing.digest.clone(),
        existing.size,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pc-image-media-import-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn floppy(path: &Path, byte: u8) {
        fs::write(path, vec![byte; 368_640]).unwrap();
    }
    fn request(paths: &[PathBuf], title: &str) -> Request {
        Request {
            tag: "test:set".into(),
            title: Title {
                title: title.into(),
                ..Title::default()
            },
            labels: Vec::new(),
            media: paths
                .iter()
                .map(|path| Source {
                    path: path.clone(),
                    label: None,
                    format: None,
                })
                .collect(),
            extras: Vec::new(),
            compress: true,
        }
    }

    #[test]
    fn identical_reimport_is_already_imported_and_a_different_set_conflicts_without_writes() {
        let temp = Temp::new("idempotent");
        let (a, b) = (temp.0.join("a.img"), temp.0.join("b.img"));
        floppy(&a, 1);
        floppy(&b, 2);
        let store = Store::open(temp.0.join("store")).unwrap();
        let first = request(&[a.clone(), b.clone()], "Synthetic set");
        let imported = publish(&store, &plan(&first).unwrap(), &|_, _| {}, &|| false).unwrap();
        assert!(matches!(imported, Outcome::Imported(_)));
        let blobs = || {
            fs::read_dir(store.root().join("images/blobs/sha256"))
                .unwrap()
                .count()
        };
        let before = blobs();
        let again = publish(&store, &plan(&first).unwrap(), &|_, _| {}, &|| false).unwrap();
        assert_eq!(
            again,
            Outcome::AlreadyImported(imported.descriptor().clone())
        );
        // Another order is another set: the tag refuses it and nothing is stored.
        let reordered = request(&[b, a], "Synthetic set");
        let refusal = publish(&store, &plan(&reordered).unwrap(), &|_, _| {}, &|| false)
            .unwrap_err()
            .to_string();
        assert!(refusal.contains("tag conflict"), "{refusal}");
        assert_eq!(blobs(), before);
    }

    fn entries(path: &Path) -> usize {
        fs::read_dir(path).map_or(0, |d| d.count())
    }

    #[test]
    fn an_existing_tag_compares_content_however_the_set_was_stored_and_never_writes() {
        let temp = Temp::new("existing");
        let (a, b) = (temp.0.join("a.img"), temp.0.join("b.img"));
        floppy(&a, 1);
        floppy(&b, 2);
        let manual = temp.0.join("manual.txt");
        fs::write(&manual, b"synthetic manual").unwrap();
        let store = Store::open(temp.0.join("store")).unwrap();
        let mut raw = request(&[a.clone(), b.clone()], "Stored raw");
        raw.compress = false;
        let stored = publish(&store, &plan(&raw).unwrap(), &|_, _| {}, &|| false).unwrap();
        let resolved = store
            .resolve_descriptor(stored.descriptor(), &Validator(&PcMediaFormats))
            .unwrap();
        assert!(resolved.manifest.layers.iter().all(|d| !d.is_compressed()));
        let blobs = || entries(&store.root().join("images/blobs/sha256"));
        let staging = || entries(&store.root().join("staging"));
        let (before, staged) = (blobs(), staging());
        let attempt = |request: &Request| {
            let hashed = std::cell::Cell::new(0);
            let result = publish(
                &store,
                &plan(request).unwrap(),
                &|done, total| {
                    if done < total {
                        hashed.set(hashed.get() + 1);
                    }
                },
                &|| false,
            );
            (result, hashed.get())
        };
        // A compressing re-import of the same content is the same set.
        let mut compressed = raw.clone();
        compressed.compress = true;
        let (result, hashed) = attempt(&compressed);
        assert_eq!(
            result.unwrap(),
            Outcome::AlreadyImported(stored.descriptor().clone())
        );
        assert_eq!(hashed, 2, "every source was hashed");
        // Different disk bytes under the same layout are refused after hashing.
        floppy(&b, 3);
        let (result, hashed) = attempt(&compressed);
        let refusal = result.unwrap_err().to_string();
        assert_eq!(
            refusal,
            format!(
                "tag conflict: test:set already names a different installation set ({}); \
                 choose another tag",
                stored.descriptor().digest
            )
        );
        assert_eq!(hashed, 2, "the changed second disk was hashed");
        // A new extra differs in layout: refused before any source is read.
        floppy(&b, 2);
        let mut extra = compressed.clone();
        extra.extras.push(ExtraSource {
            path: manual,
            dos_name: Some("MANUAL.TXT".into()),
            kind: None,
        });
        let (result, hashed) = attempt(&extra);
        assert!(result
            .unwrap_err()
            .to_string()
            .starts_with("tag conflict: test:set"));
        assert_eq!(hashed, 0);
        assert_eq!((blobs(), staging()), (before, staged));
        assert_eq!(
            store.tag_descriptor("test:set").unwrap().digest,
            stored.descriptor().digest
        );

        // The other way round: a compressed set matches a raw re-import.
        let mut tagged = compressed.clone();
        tagged.tag = "test:compressed".into();
        let stored = publish(&store, &plan(&tagged).unwrap(), &|_, _| {}, &|| false).unwrap();
        tagged.compress = false;
        assert_eq!(
            attempt(&tagged).0.unwrap(),
            Outcome::AlreadyImported(stored.descriptor().clone())
        );
    }

    #[test]
    fn cancelling_between_files_publishes_no_tag_and_reports_progress() {
        let temp = Temp::new("cancel");
        let paths: Vec<_> = (0..3).map(|i| temp.0.join(format!("{i}.img"))).collect();
        for (i, path) in paths.iter().enumerate() {
            floppy(path, i as u8);
        }
        let store = Store::open(temp.0.join("store")).unwrap();
        let plan = plan(&request(&paths, "Cancelled set")).unwrap();
        let seen = std::cell::RefCell::new(Vec::new());
        let refusal = publish(
            &store,
            &plan,
            &|done, total| seen.borrow_mut().push((done, total)),
            &|| seen.borrow().len() == 2,
        )
        .unwrap_err()
        .to_string();
        assert!(
            refusal.contains("import cancelled before source"),
            "{refusal}"
        );
        assert!(refusal.contains("2.img"), "{refusal}");
        assert_eq!(*seen.borrow(), [(0, 3), (1, 3)]);
        assert!(store.tag_descriptor("test:set").is_err());
    }

    #[test]
    fn proposals_and_kinds_follow_size_and_extension() {
        assert_eq!(propose_format(Path::new("a.img"), 1_474_560), Some("1.44M"));
        assert_eq!(propose_format(Path::new("a.iso"), 4096), Some("iso-2048"));
        assert_eq!(propose_format(Path::new("a.img"), 4096), None);
        assert_eq!(propose_format(Path::new("a.iso"), 2049), None);
        assert_eq!(
            format_names(),
            ["1.44M", "1.2M", "720K", "360K", "iso-2048"]
        );
        assert_eq!(extra_kind(Path::new("Manual.PDF")), ExtraKind::Document);
        assert_eq!(extra_kind(Path::new("patch.zip")), ExtraKind::Archive);
        assert_eq!(extra_kind(Path::new("fix.exe")), ExtraKind::Executable);
        assert_eq!(extra_kind(Path::new("art.png")), ExtraKind::Other);
    }
}
