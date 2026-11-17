// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installation-set import shared by `image media import` and the desktop
//! library; machine-images §5.2. The user's confirmed order is authoritative:
//! array position defines disk order, never file names or labels. Sources are
//! copied by the store's streaming, full-SHA-256 ingestion path and stay
//! unchanged; host paths never enter the config, metadata or annotations.
//! Floppy geometry follows the IBM Personal Computer AT Technical Reference,
//! Diskette Drive Adapter, through [`crate::media_set`].
//!
//! A set is user-supplied unless the import states otherwise
//! ([`OpenStatement`]): its licence and the extra holding the licence's text,
//! either the vendor's terms as one SPDX `LicenseRef-`, or an SPDX expression
//! together with the extra holding the set's corresponding source. An open
//! set marks every medium and extra open under that licence, each linking the
//! terms text's digest in `unemu.terms` and the source archive's, when there
//! is one, in `unemu.source`; its generated config and metadata are then open
//! under `CC0-1.0`, as a firmware package's are when every other layer is
//! open.

use crate::media_set::{self as pc, PcMediaFormats};
use image_store::{
    codec,
    compat::AppVersion,
    license::{self, Expression},
    media_set::{valid_text, Config, Extra, ExtraKind, MediaFormats, Medium, Validator},
    media_type as mt,
    metadata::{Metadata, Title, SOFTWARE_PLATFORMS},
    Descriptor, Digest, Manifest, Store,
};
use std::{
    collections::{BTreeMap, BTreeSet},
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
/// The user's statement that a set may be redistributed under the stated
/// licence. The program never guesses it from file names or contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenStatement {
    /// The vendor's terms as one SPDX `LicenseRef-<name>`, or an SPDX
    /// licence expression, which needs `source`.
    pub license: String,
    /// The file name of the extra that holds the licence's text.
    pub terms: String,
    /// The file name of the extra that holds the set's corresponding source.
    pub source: Option<String>,
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
        "zip" | "7z" | "arj" | "rar" | "tar" | "tgz" | "gz" => ExtraKind::Archive,
        "exe" | "com" | "bat" => ExtraKind::Executable,
        _ => ExtraKind::Other,
    }
}

/// A validated import: config with placeholder blobs, metadata and the payload
/// list, before any store object is written.
#[derive(Clone, Debug)]
pub struct Plan {
    tag: String,
    /// The manifest's descriptive annotations, read from the title record.
    annotations: BTreeMap<String, String>,
    config: Config,
    metadata: Metadata,
    media: Vec<(PathBuf, &'static str, u64)>,
    extras: Vec<PathBuf>,
    compress: bool,
    /// The stated licence and the extras with its text and the source.
    open: Option<Open>,
    /// A minimum release stated above what the tables compute.
    min_app_version: Option<AppVersion>,
}
/// A checked [`OpenStatement`]: the licence and the positions of the
/// terms-text and source-archive extras.
#[derive(Clone, Debug)]
struct Open {
    license: Expression,
    terms: usize,
    source: Option<usize>,
}
impl Plan {
    /// Media and extras in publication order; blobs are placeholders until import.
    pub fn config(&self) -> &Config {
        &self.config
    }
    /// The licence of an open set and the extra holding its terms text;
    /// `None` for a user-supplied set.
    pub fn open(&self) -> Option<(&Expression, &Extra)> {
        self.open
            .as_ref()
            .map(|open| (&open.license, &self.config.extras[open.terms]))
    }
    /// The extra holding an open set's corresponding source, when one is
    /// stated.
    pub fn source(&self) -> Option<&Extra> {
        let index = self.open.as_ref()?.source?;
        Some(&self.config.extras[index])
    }
    pub fn tag(&self) -> &str {
        &self.tag
    }
    /// State a minimum release above what the compatibility tables compute
    /// for the set, for behaviour they cannot see (machine-images §5.6). A
    /// value below the computed one is refused, naming both.
    pub fn raise_min_app_version(&mut self, stated: AppVersion) -> Result<()> {
        let required = crate::compat::media_set_config(&self.config)?;
        image_store::compat::raised(required, Some(stated))?;
        self.min_app_version = Some(stated);
        Ok(())
    }
    /// The minimum release stated for the set, if any.
    pub fn min_app_version(&self) -> Option<AppVersion> {
        self.min_app_version
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
/// touching a store, so a refused import leaves no blobs behind. The set is
/// user-supplied.
pub fn plan(request: &Request) -> Result<Plan> {
    plan_with(request, None)
}

/// [`plan`] with the user's statement that the set is open. The licence is
/// one `LicenseRef-<name>`, the vendor's own terms, or an SPDX expression
/// stated together with the extra holding the set's corresponding source, the
/// source an open-source licence may oblige. The terms text and the source
/// archive must each be exactly one of the request's extras, named by file
/// name; the source archive is an archive extra other than the terms text.
pub fn plan_with(request: &Request, open: Option<&OpenStatement>) -> Result<Plan> {
    ensure(valid_text(&request.title.title), || {
        format!("invalid title {:?}", request.title.title)
    })?;
    check_catalogue(&request.title)?;
    // Set labels obey the same text rule as every other label, whichever
    // front end built the request.
    for label in &request.labels {
        ensure(valid_text(label), || format!("invalid set label {label:?}"))?;
    }
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
    let open = open.map(|open| open_statement(open, &config)).transpose()?;
    let mut metadata = Metadata::new(&config.platform);
    metadata.labels.clone_from(&request.labels);
    metadata.titles.push(Title {
        id: "title".into(),
        ..request.title.clone()
    });
    metadata.encode(&[])?;
    Ok(Plan {
        tag: request.tag.clone(),
        annotations: describe(&request.title),
        config,
        metadata,
        media,
        extras: request.extras.iter().map(|e| e.path.clone()).collect(),
        compress: request.compress,
        open,
        min_app_version: None,
    })
}

/// The set-level catalogue fields the library's import dialog can state: a
/// release year from 1 to 9999 and a software platform from its canonical
/// choices. The metadata schema keeps other platforms readable for sets made
/// elsewhere, but an import here never introduces one.
fn check_catalogue(title: &Title) -> Result<()> {
    if let Some(year) = title.release_year {
        ensure((1..=9999).contains(&year), || {
            format!("release year {year}: expected a whole number from 1 to 9999")
        })?;
    }
    if let Some(platform) = &title.platform {
        ensure(SOFTWARE_PLATFORMS.contains(&platform.as_str()), || {
            let choices: Vec<String> = SOFTWARE_PLATFORMS
                .iter()
                .map(|p| format!("{p:?}"))
                .collect();
            format!(
                "software platform {platform:?}: expected one of {}",
                choices.join(", ")
            )
        })?;
    }
    Ok(())
}

/// The stated licence, checked, and the positions of the terms-text and
/// source-archive extras.
fn open_statement(open: &OpenStatement, config: &Config) -> Result<Open> {
    let license =
        Expression::parse(&open.license).context(|| format!("licence {:?}", open.license))?;
    ensure(license.is_reference() || open.source.is_some(), || {
        format!(
            "licence {:?}: an installation set is open only under the vendor's own terms, \
             stated as one LicenseRef-<name>, or under an SPDX licence expression together \
             with the extra that holds its corresponding source",
            open.license
        )
    })?;
    let terms = named_extra(config, "terms text", &open.terms)?;
    let source = open
        .source
        .as_deref()
        .map(|name| named_extra(config, "source archive", name))
        .transpose()?;
    if let Some(index) = source {
        // The guard sees only that the link names an extra; its shape is
        // checked here, where the extra's kind is known.
        ensure(index != terms, || {
            format!(
                "source archive {:?}: the terms text cannot be the source archive",
                config.extras[index].filename
            )
        })?;
        ensure(config.extras[index].kind == ExtraKind::Archive, || {
            format!(
                "source archive {:?}: expected an archive extra (zip, tar, 7z, …)",
                config.extras[index].filename
            )
        })?;
    }
    Ok(Open {
        license,
        terms,
        source,
    })
}
/// The position of the one extra whose file name is `name`.
fn named_extra(config: &Config, what: &str, name: &str) -> Result<usize> {
    let named: Vec<usize> = (0..config.extras.len())
        .filter(|&i| config.extras[i].filename == name)
        .collect();
    let extras = || {
        config
            .extras
            .iter()
            .map(|e| e.filename.as_str())
            .collect::<Vec<_>>()
    };
    match named[..] {
        [index] => Ok(index),
        [] => Err(Refusal(format!(
            "{what} {name:?}: not the file name of an extra of the set; extras: {:?}",
            extras()
        ))),
        _ => Err(Refusal(format!(
            "{what} {name:?}: names {} extras of the set; give the {what} a file name of its own",
            named.len()
        ))),
    }
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

/// What an open set's content layers carry: the licence and the decoded
/// digests of the terms text and, when stated, of the source archive.
#[derive(Clone, Copy)]
struct Marks<'a> {
    license: &'a Expression,
    terms: &'a Digest,
    source: Option<&'a Digest>,
}
/// The marks of one medium or extra: open under the stated licence and linked
/// to the terms text and the source archive, or user-supplied.
fn content(mut d: Descriptor, open: Option<Marks<'_>>) -> Descriptor {
    license::mark(&mut d, open.map(|marks| marks.license));
    let link = |d: &mut Descriptor, key: &str, digest: Option<&Digest>| match digest {
        Some(digest) => d.annotations.insert(key.into(), digest.to_string()),
        None => d.annotations.remove(key),
    };
    link(&mut d, license::TERMS, open.map(|marks| marks.terms));
    link(&mut d, license::SOURCE, open.and_then(|marks| marks.source));
    d
}
/// The marks of the config and metadata, which this program generates: open
/// under the generated-metadata licence only when every medium and extra is.
fn generated(mut d: Descriptor, open: bool) -> Descriptor {
    if open {
        return crate::generated(d);
    }
    license::mark(&mut d, None);
    d
}
/// The licence, terms link and source link a layer of the planned set
/// carries.
fn expected_marks(d: &Descriptor, open: Option<Marks<'_>>) -> Result<ExpectedMarks> {
    let generated = d.media_type == mt::MEDIA_SET_CONFIG || d.decoded_media_type() == mt::METADATA;
    Ok(match open {
        None => (None, None, None),
        Some(_) if generated => (
            Some(Expression::parse(crate::firmware::METADATA_LICENSE)?),
            None,
            None,
        ),
        Some(marks) => (
            Some(marks.license.clone()),
            Some(marks.terms.to_string()),
            marks.source.map(Digest::to_string),
        ),
    })
}
type ExpectedMarks = (Option<Expression>, Option<String>, Option<String>);

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
const DESCRIPTION: &str = "org.opencontainers.image.description";
const VENDOR: &str = "org.opencontainers.image.vendor";
const AUTHORS: &str = "org.opencontainers.image.authors";
const LICENSES: &str = "org.opencontainers.image.licenses";
/// The manifest annotations [`describe`] derives from the title record.
const DESCRIPTIVE: [&str; 4] = [TITLE, DESCRIPTION, VENDOR, AUTHORS];

/// The OCI image-spec's pre-defined annotations a registry browser shows,
/// filled from the set's title record: its title, its description, the
/// publishers as vendor (the distributing organisation) and the developers as
/// authors, each list joined with ", " in the order stated. A field the record
/// leaves empty adds nothing, so a set without catalogue fields keeps its
/// manifest bytes. There is no creation time, because an import has no clock,
/// and no URL, because the title record has no field for one.
fn describe(title: &Title) -> BTreeMap<String, String> {
    let joined = |values: &[String]| {
        values
            .iter()
            .filter(|v| !v.trim().is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    [
        (TITLE, title.title.clone()),
        (DESCRIPTION, title.description.clone().unwrap_or_default()),
        (VENDOR, joined(&title.publishers)),
        (AUTHORS, joined(&title.developers)),
    ]
    .into_iter()
    .filter(|(_, value)| !value.trim().is_empty())
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

/// Copy and fully hash each source in order, then publish the tag last. When
/// the tag already names a set, nothing is written: the same media, labels,
/// formats, extras, metadata, descriptive annotations and marks are already
/// imported, anything else is a tag conflict. `cancelled` is checked before
/// each file and a cancelled import writes no tag. `progress` receives files
/// done and the file total.
pub fn publish(
    store: &Store,
    plan: &Plan,
    progress: &dyn Fn(usize, usize),
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome> {
    publish_checked(store, plan, progress, cancelled, &|_, _| Ok(()))
}

/// What [`publish_checked`] asks before a new set is stored: its finished
/// manifest and config bytes; an error refuses the set in its own words.
pub type Check<'a> = &'a dyn Fn(&Manifest, &[u8]) -> std::result::Result<(), String>;

/// [`publish`], handing a new set's finished manifest and its config bytes to
/// `check` before the manifest is stored and the tag written: a refusal, reported as `check`
/// words it, writes neither, and the payloads already copied stay
/// unreferenced until a sweep. A tag that already names the set is reported
/// without a check, as [`publish`] reports it.
pub fn publish_checked(
    store: &Store,
    plan: &Plan,
    progress: &dyn Fn(usize, usize),
    cancelled: &dyn Fn() -> bool,
    check: Check<'_>,
) -> Result<Outcome> {
    match store.tag_descriptor(&plan.tag) {
        Ok(existing) => return compare_existing(store, plan, &existing, progress, cancelled),
        Err(image_store::Error::TagNotFound(_)) => store.check_new_tag(&plan.tag)?,
        Err(error) => return Err(error.into()),
    }
    // A new set takes a revision tag, whose release alias the store writes
    // with it; refused here before any medium is copied.
    if let Some(problem) = image_store::tag::new_image_problem(&plan.tag, false) {
        return Err(Refusal(problem));
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
        layers.push(blob);
    }
    for (index, path) in plan.extras.iter().enumerate() {
        let size = config.extras[index].size;
        let blob = payload(plan.media.len() + index, "extra", path, mt::EXTRA, size)?;
        config.extras[index].blob = blob.decoded_digest()?;
        layers.push(blob);
    }
    progress(total, total);
    let open = plan.open.as_ref().map(|open| {
        (
            &open.license,
            config.extras[open.terms].blob.clone(),
            open.source.map(|index| config.extras[index].blob.clone()),
        )
    });
    let marks = open.as_ref().map(|(license, terms, source)| Marks {
        license,
        terms,
        source: source.as_ref(),
    });
    let layers: Vec<Descriptor> = layers.into_iter().map(|d| content(d, marks)).collect();
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
        Ok(generated(
            Descriptor::new(kind, blob.digest, blob.size),
            open.is_some(),
        ))
    };
    let bytes = plan.metadata.encode(&unique)?;
    unique.push(small(mt::METADATA, &bytes)?);
    let cfg = small(mt::MEDIA_SET_CONFIG, &config.encode()?)?;
    let mut manifest = Manifest::new_media_set(&config.platform, cfg, unique)?;
    manifest.annotations.extend(plan.annotations.clone());
    if open.is_some() {
        let licenses: Vec<Expression> = manifest
            .layers
            .iter()
            .filter_map(license::open_license)
            .collect();
        if let Some(all) = Expression::all(&licenses) {
            manifest
                .annotations
                .insert(LICENSES.into(), all.as_str().into());
        }
    }
    let config_bytes = config.encode()?;
    crate::compat::stamp(
        &mut manifest,
        &config_bytes,
        image_store::compat::BASELINE,
        plan.min_app_version,
    )?;
    check(&manifest, &config_bytes).map_err(Refusal)?;
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
/// descriptive annotations, disk count, labels, formats, extras or licence
/// marks differ, is refused before a source is read. Only then is each
/// source's raw SHA-256 compared with the stored config's decoded digests, so
/// how the set was stored (compressed or not) never decides the outcome.
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
    // The descriptive annotations are part of what the tag names: a set stored
    // without the ones this import writes (published before they existed, say)
    // is refused like one with another title, never reported as this set.
    // So is the minimum release, which an import computes again and may raise.
    let release = image_store::compat::raised(
        crate::compat::media_set_config(&stored)?,
        plan.min_app_version,
    )?;
    if layout != plan.config
        || resolved.metadata != plan.metadata.encode(&[])?
        || DESCRIPTIVE
            .iter()
            .any(|&key| resolved.manifest.annotations.get(key) != plan.annotations.get(key))
        || image_store::compat::Requirement::of(&resolved.manifest).release()? != release
    {
        return Err(different());
    }
    // The same bytes under other marks are another set: rights are part of
    // what the tag names.
    let open = plan.open.as_ref().map(|open| Marks {
        license: &open.license,
        terms: &stored.extras[open.terms].blob,
        source: open.source.map(|index| &stored.extras[index].blob),
    });
    for d in std::iter::once(&resolved.manifest.config).chain(&resolved.manifest.layers) {
        let (license, terms, source) = expected_marks(d, open)?;
        if license::open_license(d) != license
            || d.annotations.get(license::TERMS) != terms.as_ref()
            || d.annotations.get(license::SOURCE) != source.as_ref()
        {
            return Err(different());
        }
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
            tag: "test:set-r1".into(),
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
    fn an_invalid_set_label_is_refused_before_anything_is_read() {
        let temp = Temp::new("set-label");
        let a = temp.0.join("a.img");
        floppy(&a, 1);
        let mut request = request(&[a], "Synthetic set");
        request.labels = vec!["freedos".into(), "line\nbreak".into()];
        let error = plan(&request).unwrap_err().to_string();
        assert!(error.contains("invalid set label"), "{error}");
        request.labels = vec!["freedos".into(), "x".repeat(129)];
        assert!(plan(&request).is_err());
        request.labels = vec!["freedos".into(), "open-source".into()];
        assert!(plan(&request).is_ok());
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
                "tag conflict: test:set-r1 already names a different installation set ({}); \
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
            .starts_with("tag conflict: test:set-r1"));
        assert_eq!(hashed, 0);
        assert_eq!((blobs(), staging()), (before, staged));
        assert_eq!(
            store.tag_descriptor("test:set-r1").unwrap().digest,
            stored.descriptor().digest
        );

        // The other way round: a compressed set matches a raw re-import.
        let mut tagged = compressed.clone();
        tagged.tag = "test:compressed-r1".into();
        let stored = publish(&store, &plan(&tagged).unwrap(), &|_, _| {}, &|| false).unwrap();
        tagged.compress = false;
        assert_eq!(
            attempt(&tagged).0.unwrap(),
            Outcome::AlreadyImported(stored.descriptor().clone())
        );
    }

    /// A set written now names the `ibm-pc` platform, which release 0.0.3
    /// first reads, so it declares 0.0.3; a stated minimum release below the
    /// computed one is refused naming both, and a re-import states the same
    /// marker. This build reads nothing newer than 0.0.3, so no statement can
    /// raise a PC set above it; the shared tables' tests raise one.
    #[test]
    fn a_stated_minimum_release_raises_the_marker_and_never_lowers_it() {
        use image_store::compat::{level, Requirement, BASELINE, PACKAGE};
        let computed = AppVersion::new(0, 0, 3);
        let temp = Temp::new("min-app-version");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 7);
        let store = Store::open(temp.0.join("store")).unwrap();
        let read = |d: &Descriptor| {
            use image_store::BlobSource;
            Manifest::decode(&store.read_verified(d, image_store::MAX_SMALL_BLOB).unwrap()).unwrap()
        };
        let mut plain = request(std::slice::from_ref(&disk), "Plain set");
        plain.tag = "test:plain-r1".into();
        let stored = publish(&store, &plan(&plain).unwrap(), &|_, _| {}, &|| false).unwrap();
        assert_eq!(
            Requirement::of(&read(stored.descriptor())),
            Requirement::Release(computed)
        );

        let mut raised = plan(&request(std::slice::from_ref(&disk), "Raised set")).unwrap();
        for lower in [AppVersion::new(0, 0, 0), BASELINE, PACKAGE] {
            let refusal = raised.raise_min_app_version(lower).unwrap_err().to_string();
            assert!(
                refusal.contains(&lower.to_string()) && refusal.contains(&computed.to_string()),
                "{refusal}"
            );
        }
        raised.raise_min_app_version(level()).unwrap();
        let stored = publish(&store, &raised, &|_, _| {}, &|| false).unwrap();
        assert_eq!(
            Requirement::of(&read(stored.descriptor())),
            Requirement::Release(level())
        );
        // The marker is part of what the tag names: the same request without
        // a statement computes the same one.
        let same = plan(&request(std::slice::from_ref(&disk), "Raised set")).unwrap();
        assert_eq!(
            publish(&store, &same, &|_, _| {}, &|| false).unwrap(),
            Outcome::AlreadyImported(stored.descriptor().clone())
        );
        assert_eq!(
            publish(&store, &raised, &|_, _| {}, &|| false).unwrap(),
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
        assert!(store.tag_descriptor("test:set-r1").is_err());
    }

    fn statement(license: &str, terms: &str) -> OpenStatement {
        OpenStatement {
            license: license.into(),
            terms: terms.into(),
            source: None,
        }
    }
    fn extra(path: &Path) -> ExtraSource {
        ExtraSource {
            path: path.into(),
            dos_name: None,
            kind: None,
        }
    }

    #[test]
    fn an_open_statement_needs_one_licence_reference_and_one_extra_as_terms_text() {
        let temp = Temp::new("open-refusals");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 1);
        let terms = temp.0.join("VENDOR.TXT");
        fs::write(&terms, b"Synthetic vendor terms").unwrap();
        fs::create_dir(temp.0.join("copy")).unwrap();
        let copy = temp.0.join("copy/VENDOR.TXT");
        fs::write(&copy, b"Another synthetic text").unwrap();
        let mut request = request(&[disk], "Synthetic shareware");
        request.extras.push(extra(&terms));
        let refusal = |request: &Request, license: &str, name: &str| {
            plan_with(request, Some(&statement(license, name)))
                .unwrap_err()
                .to_string()
        };
        let open = plan_with(
            &request,
            Some(&statement("LicenseRef-synthetic", "VENDOR.TXT")),
        )
        .unwrap();
        assert_eq!(
            open.open().map(|(l, e)| (l.as_str(), e.filename.as_str())),
            Some(("LicenseRef-synthetic", "VENDOR.TXT"))
        );
        for (license, reason) in [
            ("not a licence", "unknown SPDX licence identifier"),
            ("LicenseRef-", "unknown SPDX licence identifier"),
            ("MIT", "the vendor's own terms"),
            ("GPL-2.0-only", "the vendor's own terms"),
            ("LicenseRef-a OR MIT", "the vendor's own terms"),
        ] {
            let text = refusal(&request, license, "VENDOR.TXT");
            assert!(
                text.contains(reason) && text.contains(&format!("{license:?}")),
                "{text}"
            );
        }
        let text = refusal(&request, "LicenseRef-synthetic", "MANUAL.TXT");
        assert!(
            text.contains("not the file name of an extra") && text.contains("VENDOR.TXT"),
            "{text}"
        );
        request.extras.push(extra(&copy));
        let text = refusal(&request, "LicenseRef-synthetic", "VENDOR.TXT");
        assert!(text.contains("names 2 extras"), "{text}");
        // Without a statement the set is user-supplied, whatever the extras are called.
        assert!(plan(&request).unwrap().open().is_none());
    }

    #[test]
    fn an_open_set_marks_every_medium_and_extra_and_links_its_terms_text() {
        let temp = Temp::new("open-marks");
        let (a, b) = (temp.0.join("a.img"), temp.0.join("b.img"));
        floppy(&a, 1);
        floppy(&b, 2);
        // Large enough to be stored compressed: the marks travel on `+zstd` too.
        let manual = temp.0.join("MANUAL.TXT");
        fs::write(&manual, vec![b'm'; 5000]).unwrap();
        // A terms text above the 4 KiB threshold is stored `+zstd` as well;
        // the links name its decoded digest, not the stored one.
        let terms = temp.0.join("VENDOR.TXT");
        fs::write(&terms, "Synthetic vendor terms. ".repeat(200)).unwrap();
        let store = Store::open(temp.0.join("store")).unwrap();
        let mut shareware = request(&[a, b], "Synthetic shareware");
        shareware.tag = "test:open-r1".into();
        shareware.extras = vec![extra(&manual), extra(&terms)];
        let vendor = statement("LicenseRef-synthetic-shareware", "VENDOR.TXT");
        let publish_with = |request: &Request, open: Option<&OpenStatement>| {
            publish(
                &store,
                &plan_with(request, open).unwrap(),
                &|_, _| {},
                &|| false,
            )
        };
        let resolve = |outcome: &Outcome| {
            store
                .resolve_descriptor(outcome.descriptor(), &Validator(&PcMediaFormats))
                .unwrap()
        };
        let open = resolve(&publish_with(&shareware, Some(&vendor)).unwrap());
        let config = Config::decode(&open.config).unwrap();
        let link = config.extras[1].blob.to_string();
        assert_eq!(open.manifest.layers.len(), 5);
        let text = open
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&config.extras[1].blob))
            .unwrap();
        assert!(text.is_compressed() && text.digest.to_string() != link);
        for d in &open.manifest.layers {
            let licence = license::open_license(d).unwrap();
            if d.media_type == mt::METADATA {
                assert_eq!(licence.as_str(), "CC0-1.0");
                assert!(!d.annotations.contains_key(license::TERMS));
            } else {
                assert_eq!(licence.as_str(), "LicenseRef-synthetic-shareware");
                assert_eq!(d.annotations[license::TERMS], link);
            }
        }
        let cfg = &open.manifest.config;
        assert_eq!(license::open_license(cfg).unwrap().as_str(), "CC0-1.0");
        assert!(!cfg.annotations.contains_key(license::TERMS));
        assert_eq!(
            open.manifest.annotations[LICENSES],
            "LicenseRef-synthetic-shareware AND CC0-1.0"
        );
        let closure: Vec<Descriptor> = std::iter::once(cfg.clone())
            .chain(open.manifest.layers.clone())
            .collect();
        assert!(license::terms_unlinked(&closure).is_empty());

        // The same files without the statement are user-supplied throughout.
        let mut private = shareware.clone();
        private.tag = "test:private-r1".into();
        let user = resolve(&publish_with(&private, None).unwrap());
        for d in std::iter::once(&user.manifest.config).chain(&user.manifest.layers) {
            assert_eq!(
                d.annotations[license::REDISTRIBUTION],
                license::USER_SUPPLIED
            );
            assert!(!d.annotations.contains_key(license::LICENSE));
            assert!(!d.annotations.contains_key(license::TERMS));
        }
        assert!(!user.manifest.annotations.contains_key(LICENSES));

        // The marks are part of the set: the same bytes under other marks,
        // another licence, or another extra as terms text are a tag conflict.
        let mut swapped = shareware.clone();
        swapped.tag = "test:private-r1".into();
        private.tag = "test:open-r1".into();
        let manual_terms = statement("LicenseRef-synthetic-shareware", "MANUAL.TXT");
        let other_licence = statement("LicenseRef-other-terms", "VENDOR.TXT");
        for (request, open) in [
            (&swapped, Some(&vendor)),
            (&private, None),
            (&shareware, Some(&manual_terms)),
            (&shareware, Some(&other_licence)),
        ] {
            let refusal = publish_with(request, open).unwrap_err().to_string();
            assert!(refusal.starts_with("tag conflict"), "{refusal}");
        }
        assert!(matches!(
            publish_with(&shareware, Some(&vendor)).unwrap(),
            Outcome::AlreadyImported(d) if d.digest == open.image.digest
        ));
    }

    fn with_source(license: &str, terms: &str, source: &str) -> OpenStatement {
        OpenStatement {
            source: Some(source.into()),
            ..statement(license, terms)
        }
    }

    #[test]
    fn an_spdx_licence_opens_a_set_only_with_one_extra_named_as_its_source() {
        let temp = Temp::new("open-spdx-refusals");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 1);
        let (copying, archive) = (temp.0.join("COPYING"), temp.0.join("SRC.TAR"));
        fs::write(&copying, b"Synthetic licence text").unwrap();
        fs::write(&archive, b"Synthetic source archive").unwrap();
        let mut request = request(&[disk], "Synthetic free software");
        request.extras = vec![extra(&copying), extra(&archive)];
        let refusal = |request: &Request, open: OpenStatement| {
            plan_with(request, Some(&open)).unwrap_err().to_string()
        };
        // An SPDX expression needs the source archive, copyleft or not.
        for license in ["GPL-2.0-only", "MIT", "LicenseRef-a OR MIT"] {
            let text = refusal(&request, statement(license, "COPYING"));
            assert!(
                text.contains("the vendor's own terms") && text.contains("corresponding source"),
                "{text}"
            );
        }
        for license in [
            "GPL-2.0-only",
            "GPL-2.0-or-later AND MIT AND LicenseRef-other",
            "MIT",
        ] {
            let plan =
                plan_with(&request, Some(&with_source(license, "COPYING", "SRC.TAR"))).unwrap();
            assert_eq!(
                plan.open().map(|(l, e)| (l.as_str(), e.filename.as_str())),
                Some((license, "COPYING"))
            );
            assert_eq!(plan.source().map(|e| e.filename.as_str()), Some("SRC.TAR"));
        }
        let text = refusal(
            &request,
            with_source("GPL-2.0-only", "COPYING", "SOURCES.ZIP"),
        );
        assert!(
            text.starts_with("source archive \"SOURCES.ZIP\": not the file name of an extra")
                && text.contains("SRC.TAR"),
            "{text}"
        );
        fs::create_dir(temp.0.join("copy")).unwrap();
        let copy = temp.0.join("copy/SRC.TAR");
        fs::write(&copy, b"Another synthetic archive").unwrap();
        request.extras.push(extra(&copy));
        let text = refusal(&request, with_source("GPL-2.0-only", "COPYING", "SRC.TAR"));
        assert!(text.contains("names 2 extras"), "{text}");
        request.extras.pop();
        // The source archive is an archive extra, never the terms text.
        let text = refusal(&request, with_source("GPL-2.0-only", "SRC.TAR", "SRC.TAR"));
        assert!(
            text.contains("the terms text cannot be the source archive"),
            "{text}"
        );
        let text = refusal(&request, with_source("GPL-2.0-only", "SRC.TAR", "COPYING"));
        assert!(text.contains("expected an archive extra"), "{text}");
        // A licence reference may name a source archive too.
        let plan = plan_with(
            &request,
            Some(&with_source("LicenseRef-synthetic", "COPYING", "SRC.TAR")),
        )
        .unwrap();
        assert!(plan.source().is_some());
        assert!(plan_with(
            &request,
            Some(&statement("LicenseRef-synthetic", "COPYING"))
        )
        .unwrap()
        .source()
        .is_none());
    }

    #[test]
    fn an_open_copyleft_set_links_every_medium_and_extra_to_its_source_archive() {
        let temp = Temp::new("open-copyleft");
        let (a, b) = (temp.0.join("a.img"), temp.0.join("b.img"));
        floppy(&a, 1);
        floppy(&b, 2);
        let copying = temp.0.join("COPYING");
        fs::write(&copying, "Synthetic licence text. ".repeat(300)).unwrap();
        // Above the compression threshold: the links name decoded digests.
        let archive = temp.0.join("SRC.TAR");
        fs::write(&archive, vec![b's'; 9000]).unwrap();
        let store = Store::open(temp.0.join("store")).unwrap();
        let mut set = request(&[a, b], "Synthetic free software");
        set.tag = "test:gpl-r1".into();
        set.extras = vec![extra(&copying), extra(&archive)];
        let gpl = with_source("GPL-2.0-or-later AND MIT", "COPYING", "SRC.TAR");
        let publish_with = |request: &Request, open: &OpenStatement| {
            publish(
                &store,
                &plan_with(request, Some(open)).unwrap(),
                &|_, _| {},
                &|| false,
            )
        };
        let outcome = publish_with(&set, &gpl).unwrap();
        let open = store
            .resolve_descriptor(outcome.descriptor(), &Validator(&PcMediaFormats))
            .unwrap();
        let config = Config::decode(&open.config).unwrap();
        let (terms, source) = (
            config.extras[0].blob.to_string(),
            config.extras[1].blob.to_string(),
        );
        let stored = open
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&config.extras[1].blob))
            .unwrap();
        assert!(stored.is_compressed() && stored.digest.to_string() != source);
        for d in &open.manifest.layers {
            if d.media_type == mt::METADATA {
                assert!(!d.annotations.contains_key(license::SOURCE));
                continue;
            }
            assert_eq!(
                license::open_license(d).unwrap().as_str(),
                "GPL-2.0-or-later AND MIT"
            );
            assert_eq!(d.annotations[license::TERMS], terms);
            assert_eq!(d.annotations[license::SOURCE], source);
        }
        assert!(!open
            .manifest
            .config
            .annotations
            .contains_key(license::SOURCE));
        assert_eq!(
            open.manifest.annotations[LICENSES],
            "GPL-2.0-or-later AND MIT AND CC0-1.0"
        );
        // Rules 4 and 5 find everything they need in the closure.
        let closure: Vec<Descriptor> = std::iter::once(open.manifest.config.clone())
            .chain(open.manifest.layers.clone())
            .collect();
        assert!(closure
            .iter()
            .all(|d| !license::source_missing(d, &closure)));
        assert!(license::terms_unlinked(&closure).is_empty());
        // Without the archive the media are refused by rule 4.
        let without: Vec<Descriptor> = closure
            .iter()
            .filter(|d| !d.names(&config.extras[1].blob))
            .cloned()
            .collect();
        assert_eq!(
            without
                .iter()
                .filter(|d| license::source_missing(d, &without))
                .count(),
            3
        );
        // The source link is part of the set: the same files under a licence
        // reference without one are a tag conflict.
        let reference = statement("LicenseRef-synthetic", "COPYING");
        let refusal = publish_with(&set, &reference).unwrap_err().to_string();
        assert!(refusal.starts_with("tag conflict"), "{refusal}");
        assert!(matches!(
            publish_with(&set, &gpl).unwrap(),
            Outcome::AlreadyImported(d) if d.digest == open.image.digest
        ));
    }

    #[test]
    fn a_check_refusal_stores_no_manifest_and_writes_no_tag() {
        let temp = Temp::new("checked");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 3);
        let store = Store::open(temp.0.join("store")).unwrap();
        let plan = plan(&request(&[disk], "Checked set")).unwrap();
        let seen = std::cell::RefCell::new(None);
        let refusal = publish_checked(&store, &plan, &|_, _| {}, &|| false, &|manifest, _| {
            *seen.borrow_mut() = Some(manifest.clone());
            Err("synthetic refusal".into())
        })
        .unwrap_err()
        .to_string();
        assert_eq!(refusal, "synthetic refusal");
        let checked = seen.into_inner().expect("the check saw the manifest");
        assert_eq!(checked.layers.len(), 2);
        assert!(store.tag_descriptor("test:set-r1").is_err());
        let digest = Digest::of(&checked.encode().unwrap());
        assert!(!store
            .root()
            .join("images/blobs/sha256")
            .join(digest.hex())
            .exists());
        // The same plan passes once the check allows it.
        let published =
            publish_checked(&store, &plan, &|_, _| {}, &|| false, &|_, _| Ok(())).unwrap();
        assert_eq!(published.descriptor().digest, digest);
    }

    #[test]
    fn a_set_imported_without_a_statement_keeps_its_manifest_bytes() {
        // Measured with the import as it was before open statements existed:
        // the default marks, and so every set digest, are unchanged. The
        // rename of the platform id to `ibm-pc` moved it once.
        let temp = Temp::new("pinned");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 0x5a);
        let manual = temp.0.join("MANUAL.TXT");
        fs::write(&manual, b"Synthetic manual").unwrap();
        let store = Store::open(temp.0.join("store")).unwrap();
        let mut set = request(&[disk], "Pinned set");
        set.extras.push(ExtraSource {
            path: manual,
            dos_name: Some("MANUAL.TXT".into()),
            kind: None,
        });
        set.compress = false;
        let published = publish(&store, &plan(&set).unwrap(), &|_, _| {}, &|| false).unwrap();
        assert_eq!(
            published.descriptor().digest.to_string(),
            "sha256:6e6804aba2a89f2c56d6b8758b9abb575b04c272eaca415c63d5ef3d7df2f57e"
        );
        // The test support that makes artifacts naming the retired id gives
        // this set as releases before the rename wrote it, byte for byte: the
        // pin of those releases.
        let old = image_store::platform::with_platform_id(
            &store,
            published.descriptor(),
            &[],
            image_store::platform::ALIASES[0].0,
        )
        .unwrap();
        assert_eq!(
            old.digest.to_string(),
            "sha256:852615e645eac3a3bccfb7d3a30b026e4cec4bbb7fc5ee72f06ba2808b671dcf"
        );
    }

    /// The manifest's OCI image-spec annotations, the ones registry browsers read.
    fn oci(manifest: &Manifest) -> BTreeMap<&str, &str> {
        manifest
            .annotations
            .iter()
            .filter(|(key, _)| key.starts_with("org.opencontainers.image."))
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    #[test]
    fn the_title_record_fills_the_standard_oci_annotations_in_stated_order() {
        let temp = Temp::new("annotations");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 1);
        let store = Store::open(temp.0.join("store")).unwrap();
        let mut set = request(&[disk], "Catalogued set");
        set.title.description = Some("A synthetic set.\nSecond line.".into());
        set.title.publishers = vec!["Second Publisher".into(), "First Publisher".into()];
        set.title.developers = vec!["Synthetic Studio".into(), "Another Studio".into()];
        set.title.release_year = Some(1991);
        set.title.genres = vec!["puzzle".into()];
        set.title.platform = Some("DOS".into());
        let import = || publish(&store, &plan(&set).unwrap(), &|_, _| {}, &|| false);
        let published = import().unwrap();
        let resolved = store
            .resolve_descriptor(published.descriptor(), &Validator(&PcMediaFormats))
            .unwrap();
        // Lists keep the order stated, never sorted; nothing else is added,
        // in particular no creation time.
        assert_eq!(
            oci(&resolved.manifest),
            BTreeMap::from([
                (TITLE, "Catalogued set"),
                (DESCRIPTION, "A synthetic set.\nSecond line."),
                (VENDOR, "Second Publisher, First Publisher"),
                (AUTHORS, "Synthetic Studio, Another Studio"),
            ])
        );
        // An identical re-import writes the identical manifest and is the same set.
        assert_eq!(
            import().unwrap(),
            Outcome::AlreadyImported(published.descriptor().clone())
        );
        // The same media and metadata under a manifest without the annotations,
        // as an import made before they existed stored it, is another set.
        let mut bare = resolved.manifest.clone();
        for key in [DESCRIPTION, VENDOR, AUTHORS] {
            bare.annotations.remove(key);
        }
        let bare = store.import_manifest(&bare).unwrap();
        store
            .remove_tag("test:set-r1", &published.descriptor().digest)
            .unwrap();
        // That earlier import's store kept no record of deleted revisions.
        fs::remove_file(store.root().join("local/retired-version-tags.toml")).unwrap();
        store
            .update_tag("test:set-r1", &bare, None, &Validator(&PcMediaFormats))
            .unwrap();
        let refusal = import().unwrap_err().to_string();
        assert!(
            refusal.starts_with("tag conflict: test:set-r1"),
            "{refusal}"
        );
    }

    #[test]
    fn empty_catalogue_fields_add_no_annotation_and_blank_list_entries_are_skipped() {
        let temp = Temp::new("no-annotations");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 1);
        let annotations = |edit: &dyn Fn(&mut Title)| {
            let mut set = request(std::slice::from_ref(&disk), "Plain set");
            edit(&mut set.title);
            plan(&set).unwrap().annotations
        };
        let title_only = BTreeMap::from([(TITLE.to_owned(), "Plain set".to_owned())]);
        assert_eq!(annotations(&|_| {}), title_only);
        // Catalogue fields the annotations do not carry add nothing either.
        assert_eq!(
            annotations(&|t| {
                t.release_year = Some(1991);
                t.genres = vec!["puzzle".into()];
                t.platform = Some("DOS".into());
            }),
            title_only
        );
        assert_eq!(
            annotations(&|t| {
                t.description = Some(" \n".into());
                t.publishers = vec![String::new(), " ".into()];
            }),
            title_only
        );
        assert_eq!(
            annotations(&|t| t.developers = vec![" ".into(), "Studio".into(), String::new()]),
            BTreeMap::from([
                (TITLE.to_owned(), "Plain set".to_owned()),
                (AUTHORS.to_owned(), "Studio".to_owned()),
            ])
        );
    }

    #[test]
    fn a_release_year_and_software_platform_follow_the_import_dialog_rules() {
        let temp = Temp::new("catalogue");
        let disk = temp.0.join("disk.img");
        floppy(&disk, 1);
        let with = |year: Option<i64>, platform: Option<&str>| {
            let mut request = request(std::slice::from_ref(&disk), "Catalogued set");
            request.title.release_year = year;
            request.title.platform = platform.map(str::to_owned);
            plan(&request).map(|plan| plan.metadata.titles[0].clone())
        };
        // Every choice the dialog offers, and its year bounds, are accepted as stated.
        for platform in image_store::metadata::SOFTWARE_PLATFORMS {
            let title = with(Some(1), Some(platform)).unwrap();
            assert_eq!(title.platform.as_deref(), Some(*platform));
        }
        assert_eq!(with(Some(9999), None).unwrap().release_year, Some(9999));
        for year in [0, -1, 10_000, i64::MAX] {
            assert_eq!(
                with(Some(year), Some("DOS")).unwrap_err().to_string(),
                format!("release year {year}: expected a whole number from 1 to 9999")
            );
        }
        for platform in ["dos", "Amiga", "", " DOS"] {
            let refusal = with(Some(1991), Some(platform)).unwrap_err().to_string();
            assert!(
                refusal.starts_with(&format!(
                    "software platform {platform:?}: expected one of \"DOS\", \"Windows 3.x\""
                )) && refusal.ends_with("\"other\""),
                "{refusal}"
            );
        }
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
        assert_eq!(extra_kind(Path::new("SOURCE.TAR")), ExtraKind::Archive);
        assert_eq!(extra_kind(Path::new("fix.exe")), ExtraKind::Executable);
        assert_eq!(extra_kind(Path::new("art.png")), ExtraKind::Other);
    }
}
