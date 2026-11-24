// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only ROM-folder matching, tag-last package import and metadata
//! revisions; machine-images §5.3. Exact hashes identify each chip; a
//! source's file name, without its folder, is only the name its layer
//! exports to (recipes.md §3.10). Every layer carries an SPDX licence next
//! to its redistribution mark; the generated README and the config are open
//! only when every other layer is.

use crate::firmware::{self, Fit, PcFirmwareRules};
use crate::{invalid, Result};
use image_store::{
    export_name,
    firmware::{ArchiveFormat, Config, Part, Provenance, Readme, Rom, Source, Validator},
    license::{self, Expression},
    media_set::{Extra, ExtraKind, MediaFormats},
    media_type as mt, BlobSource, Descriptor, Digest, Manifest, Store,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Issue {
    pub source: PathBuf,
    pub reason: String,
}
/// One package to import. Publication derives the package's rights from the
/// catalogue and the bytes again, so a hand-built candidate, or one with its
/// `catalogue_id` cleared, cannot open what the catalogue lists as proprietary.
#[derive(Debug)]
pub struct Candidate {
    pub catalogue_id: String,
    pub config: Config,
    pub sources: Vec<PathBuf>,
    pub distribution: assembly::FirmwareDistribution,
}
/// The local tag an import gives the package of the catalogue entry
/// `catalogue_id`: its first revision in
/// `private/firmware/<platform>/<catalogue id>` (recipes.md §8.3). A local
/// import is never named under `unemu/`, open or not, so it never takes the
/// official name a pull of the published package writes; the official
/// builders (`template::build_firmware`) write those, and a push may name
/// any target the guard allows.
pub fn package_tag(catalogue_id: &str) -> String {
    format!(
        "{}:{}",
        crate::firmware_repository(crate::PRIVATE_NAMESPACE, catalogue_id),
        image_store::tag::revision(None, 1)
    )
}

/// The local tag an import of the package of `catalogue_id` checks and
/// writes in `store`: [`package_tag`], unless only the first revision under
/// `unemu/` exists, as a pull or a template build wrote it. One catalogue
/// entry has one first revision here: [`existing`] reports the same ROM and
/// document bytes under it as already imported, whatever its licences, and
/// other bytes as a conflict; an edit publishes a new revision.
pub fn import_tag(store: &Store, catalogue_id: &str) -> Result<String> {
    let own = package_tag(catalogue_id);
    let present = |tag: &str| match store.tag_descriptor(tag) {
        Ok(_) => Ok(true),
        Err(image_store::Error::TagNotFound(_)) => Ok(false),
        Err(error) => Err(error),
    };
    if present(&own)? {
        return Ok(own);
    }
    let official = format!(
        "{}:{}",
        crate::firmware_repository(crate::OPEN_NAMESPACE, catalogue_id),
        image_store::tag::revision(None, 1)
    );
    if present(&official)? {
        return Ok(official);
    }
    Ok(own)
}

impl Candidate {
    /// The local tag an import of this candidate writes into a store that
    /// holds no package of its catalogue entry ([`package_tag`]).
    pub fn tag(&self) -> String {
        package_tag(&self.catalogue_id)
    }
    /// Another catalogue entry pins the same logical ROM, so the folder alone
    /// cannot choose its identity: the user must select one entry.
    pub fn is_ambiguous(&self, catalogue: &[assembly::FirmwareEntry]) -> bool {
        self.config.roms.first().is_some_and(|rom| {
            catalogue.iter().any(|entry| {
                entry.id != self.catalogue_id
                    && entry.sha256 == rom.sha256
                    && entry.size == rom.size
            })
        })
    }
}
#[derive(Debug, Default)]
pub struct Scan {
    pub candidates: Vec<Candidate>,
    pub issues: Vec<Issue>,
    pub size_mismatches: usize,
    pub hash_mismatches: usize,
}
#[derive(Clone, Debug)]
pub struct Attachment {
    pub path: PathBuf,
    pub dos_name: Option<String>,
}

/// Host enumeration limits prevent an accidental large tree from becoming a ROM scan.
#[derive(Clone, Copy, Debug)]
pub struct ScanLimits {
    pub depth: usize,
    pub entries: usize,
}
impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            depth: 32,
            entries: 10_000,
        }
    }
}
struct Walk {
    visited: BTreeSet<PathBuf>,
    files: Vec<PathBuf>,
    issues: Vec<Issue>,
    limits: ScanLimits,
    entries: usize,
    limited: bool,
}
impl Walk {
    fn limit(&mut self, path: &Path, reason: &str) {
        if !self.limited {
            let maximum = if reason == "entry count" {
                self.limits.entries
            } else {
                self.limits.depth
            };
            self.issues.push(Issue { source: path.into(), reason: format!("scan limit reached ({reason}, maximum {maximum}); narrow the folder or supply explicit files") });
        }
        self.limited = true;
    }
    fn admit(&mut self, path: &Path) -> bool {
        if self.entries >= self.limits.entries {
            self.limit(path, "entry count");
            return false;
        }
        self.entries += 1;
        true
    }
    fn visit(&mut self, path: &Path, boundary: &Path, depth: usize) {
        let result = (|| -> std::io::Result<()> {
            let canonical = path.canonicalize()?;
            if !canonical.starts_with(boundary) {
                return Err(std::io::Error::other("symlink leaves import root"));
            }
            if !self.visited.insert(canonical) {
                return Ok(());
            }
            let metadata = fs::metadata(path)?;
            if metadata.is_dir() {
                if depth > self.limits.depth {
                    self.limit(path, "recursion depth");
                    return Ok(());
                }
                let mut children = Vec::new();
                for entry in fs::read_dir(path)? {
                    if !self.admit(path) {
                        return Ok(());
                    }
                    match entry {
                        Ok(entry) => children.push(entry.path()),
                        Err(e) => self.issues.push(Issue {
                            source: path.into(),
                            reason: format!("unreadable directory entry: {}", e.kind()),
                        }),
                    }
                }
                children.sort();
                for child in children {
                    self.visit(&child, boundary, depth + 1);
                    if self.limited {
                        break;
                    }
                }
            } else if metadata.is_file() {
                self.files.push(path.into());
            } else {
                return Err(std::io::Error::other("expected regular file"));
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.issues.push(Issue {
                source: path.into(),
                reason: format!("unreadable input: {e}"),
            });
        }
    }
}
/// Stable file order and full hash/size matching; no source or store is modified.
pub fn scan(inputs: &[PathBuf], catalogue: &[assembly::FirmwareEntry]) -> Result<Scan> {
    scan_with_limits(inputs, catalogue, ScanLimits::default())
}
/// A limited enumeration publishes no candidates, so filesystem listing order
/// cannot choose an arbitrary partial subset of a large tree.
pub fn scan_with_limits(
    inputs: &[PathBuf],
    catalogue: &[assembly::FirmwareEntry],
    limits: ScanLimits,
) -> Result<Scan> {
    let mut walk = Walk {
        visited: BTreeSet::new(),
        files: Vec::new(),
        issues: Vec::new(),
        entries: 0,
        limited: false,
        limits,
    };
    let mut inputs = inputs.to_vec();
    inputs.sort();
    for path in inputs {
        if !walk.admit(&path) {
            break;
        }
        match path.canonicalize() {
            Ok(boundary) => walk.visit(&path, &boundary, 0),
            Err(e) => walk.issues.push(Issue {
                source: path,
                reason: format!("unreadable input: {}", e.kind()),
            }),
        }
        if walk.limited {
            break;
        }
    }
    let mut scan = Scan {
        issues: walk.issues,
        ..Scan::default()
    };
    if walk.limited {
        return Ok(scan);
    }
    let mut files = walk.files;
    files.sort();
    let sizes: BTreeSet<_> = catalogue
        .iter()
        .flat_map(|e| {
            std::iter::once(e.size).chain(e.parts.iter().flat_map(|p| p.iter().map(|p| p.size)))
        })
        .collect();
    let mut found = BTreeMap::<(String, u64), PathBuf>::new();
    for path in files {
        let result = (|| -> Result<()> {
            let stat = fs::metadata(&path)?;
            if !sizes.contains(&stat.len()) {
                scan.size_mismatches += 1;
                return Ok(());
            }
            let hash = machine_file::firmware_discovery::hash_firmware_file(&path)
                .map_err(|e| invalid(format!("unreadable ROM: {e}")))?;
            let matches = catalogue.iter().any(|e| {
                (e.sha256 == hash && e.size == stat.len())
                    || e.parts
                        .iter()
                        .flat_map(|l| l.iter())
                        .any(|p| p.sha256 == hash && p.size == stat.len())
            });
            if !matches {
                scan.hash_mismatches += 1;
                return Ok(());
            }
            found.entry((hash, stat.len())).or_insert(path.clone());
            Ok(())
        })();
        if let Err(e) = result {
            scan.issues.push(Issue {
                source: path,
                reason: e.to_string(),
            });
        }
    }
    for entry in catalogue {
        let mut choices = Vec::new();
        if let Some(path) = found.get(&(entry.sha256.into(), entry.size)) {
            choices.push((
                Some(format!("sha256:{}", entry.sha256).parse()?),
                Vec::new(),
                vec![path.clone()],
            ));
        }
        for layout in entry.parts {
            let present = layout
                .iter()
                .filter(|p| found.contains_key(&(p.sha256.into(), p.size)))
                .count();
            if present == 0 {
                continue;
            }
            if present != layout.len() {
                let missing = layout
                    .iter()
                    .filter(|p| !found.contains_key(&(p.sha256.into(), p.size)))
                    .map(|p| p.label)
                    .collect::<Vec<_>>()
                    .join(", ");
                scan.issues.push(Issue {
                    source: PathBuf::from(entry.id),
                    reason: format!(
                        "{present} of {} chips present, missing {missing}",
                        layout.len()
                    ),
                });
                continue;
            }
            let mut parts = Vec::new();
            let mut sources = Vec::new();
            for p in *layout {
                let path = found
                    .get(&(p.sha256.into(), p.size))
                    .ok_or_else(|| invalid("missing matched chip"))?;
                sources.push(path.clone());
                parts.push(Part {
                    blob: format!("sha256:{}", p.sha256).parse()?,
                    sha256: p.sha256.into(),
                    size: p.size,
                    position: Some(p.label.into()),
                    part_number: Some(p.part_number.into()),
                    lane: p.lane.to_string(),
                    offset: p.offset,
                });
            }
            choices.push((None, parts, sources));
        }
        if choices.len() > 1 {
            scan.issues.push(Issue {
                source: entry.id.into(),
                reason: "ambiguous complete layouts; supply files for exactly one layout".into(),
            });
            continue;
        }
        if let Some((blob, parts, sources)) = choices.pop() {
            let rom = Rom {
                id: firmware::role(entry.kind).into(),
                role: firmware::role(entry.kind).into(),
                catalogue_id: Some(entry.id.into()),
                sha256: entry.sha256.into(),
                size: entry.size,
                blob,
                position: None,
                part_number: None,
                requires: sorted(entry.requires.iter().map(|p| firmware::device_kind(*p))),
                parts,
            };
            let fit = catalogue_fit(entry);
            let provenance = match entry.distribution {
                assembly::FirmwareDistribution::Open { provenance, .. } => Some(Provenance {
                    kind: None,
                    from: provenance.into(),
                    revision: None,
                    url: None,
                    date: None,
                }),
                assembly::FirmwareDistribution::UserSupplied => None,
            };
            let config = Config {
                schema: 1,
                platform: crate::PLATFORM.into(),
                name: entry.id.into(),
                vendor: None,
                version: None,
                date: None,
                notes: None,
                description: None,
                labels: Vec::new(),
                fits: fit.table()?,
                provenance,
                roms: vec![rom],
                sources: Vec::new(),
                extras: Vec::new(),
                readme: None,
            };
            let candidate = Candidate {
                catalogue_id: entry.id.into(),
                config,
                sources,
                distribution: entry.distribution,
            };
            let descriptors = candidate
                .config
                .payloads()
                .into_iter()
                .map(|(hash, size, kind)| Descriptor::new(kind, hash, size))
                .collect::<Vec<_>>();
            let source = Files(&candidate);
            if let Err(e) = image_store::firmware::FirmwareRules::validate(
                &PcFirmwareRules(catalogue),
                &candidate.config,
                &source,
                &descriptors,
            ) {
                scan.issues.push(Issue {
                    source: entry.id.into(),
                    reason: format!("invalid complete set: {e}"),
                });
            } else {
                scan.candidates.push(candidate);
            }
        }
    }
    Ok(scan)
}
/// Lists in a config are kept sorted, so equal sets have one spelling.
fn sorted<'a>(items: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut items: Vec<String> = items.map(str::to_owned).collect();
    items.sort();
    items
}
struct Files<'a>(&'a Candidate);
impl BlobSource for Files<'_> {
    fn read_verified(&self, d: &Descriptor, limit: u64) -> image_store::Result<Vec<u8>> {
        let payloads = self.0.config.payloads();
        let index = payloads
            .iter()
            .position(|(hash, _, _)| hash == &d.digest)
            .ok_or_else(|| image_store::Error::Invalid("missing source".into()))?;
        let path = self
            .0
            .sources
            .get(index)
            .ok_or_else(|| image_store::Error::Invalid("missing source".into()))?;
        use std::io::Read;
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit
            || bytes.len() as u64 != d.size
            || Digest::of(&bytes) != d.digest
        {
            return Err(image_store::Error::Invalid(
                "ROM changed during import preview".into(),
            ));
        }
        Ok(bytes)
    }
}
/// Descriptive fields and licence statements a user supplies at import or in
/// an edit; machine-images §5.3. `None` keeps the catalogue prefill (import)
/// or the package's current value (edit). The program never guesses `open`:
/// it comes from the catalogue or from `open` with a named licence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Statement {
    pub title: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub vendor: Option<String>,
    pub version: Option<String>,
    pub date: Option<String>,
    pub labels: Option<Vec<String>>,
    pub boards: Option<Vec<String>>,
    pub machine_classes: Option<Vec<String>>,
    /// Device kinds an unlisted ROM needs; catalogue ROMs take the catalogue's.
    pub requires: Option<Vec<String>>,
    pub provenance: Option<Provenance>,
    /// The user states that the ROM images are open under `license`.
    pub open: bool,
    pub license: Option<String>,
    /// The user marks the ROM images user-supplied, even open catalogue firmware.
    pub user_supplied: bool,
    /// A zip or tar archive holding the ROM images' corresponding source.
    pub source: Option<PathBuf>,
    /// The licence of a source archive other than the one the project pins
    /// for a catalogue ROM; with `open`, it marks that archive open.
    pub source_license: Option<String>,
    /// Licences of the attachments, by position; `None` keeps one user-supplied.
    pub extra_licenses: Vec<Option<String>>,
    /// What happens to the README layer.
    pub readme: ReadmeChoice,
    /// A minimum release above what the compatibility tables compute, for
    /// behaviour they cannot see (machine-images §5.6); never lower. An edit
    /// keeps the edited package's own as a floor.
    pub min_app_version: Option<image_store::compat::AppVersion>,
}

/// Record that a layer takes `name`, and with it `name.zst`, ignoring case.
fn occupy(taken: &mut BTreeSet<String>, name: &str) {
    taken.insert(export_name::key(name));
    taken.insert(export_name::key(&format!("{name}{}", export_name::ZST)));
}
/// Whether neither `name` nor `name.zst` is taken, ignoring case.
fn free(taken: &BTreeSet<String>, name: &str) -> bool {
    !taken.contains(&export_name::key(name))
        && !taken.contains(&export_name::key(&format!("{name}{}", export_name::ZST)))
}

/// The README a statement asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReadmeChoice {
    /// Generate it at import. In an edit, keep a README written by hand and
    /// generate any other again.
    #[default]
    Keep,
    /// Generate it from the fields and licences, replacing one written by hand.
    Generate,
    /// This text, written by hand, stored with LF line ends.
    Text(String),
}

/// README layer annotation that marks a README written by hand, so an edit
/// keeps it and tells it apart from generated text.
pub const README_ORIGIN: &str = "unemu.readme";
pub const HAND_WRITTEN: &str = "hand-written";
/// Whether a README layer holds text the user wrote by hand.
pub fn hand_written_readme(descriptor: &Descriptor) -> bool {
    descriptor
        .annotations
        .get(README_ORIGIN)
        .map(String::as_str)
        == Some(HAND_WRITTEN)
}

/// Where a planned layer's bytes come from.
enum Payload {
    File(PathBuf),
    Memory(Vec<u8>),
    /// Already in the store, as part of the package being edited.
    Stored,
}
struct Plan {
    manifest: Manifest,
    config: Vec<u8>,
    /// The config `config` encodes, and the README text the plan stores.
    decoded: Config,
    readme: String,
    /// One entry per manifest layer, in layer order.
    payloads: Vec<(Payload, Descriptor)>,
    /// What the user should know, such as why open firmware stays private.
    notes: Vec<String>,
}
struct Archive {
    payload: Payload,
    descriptor: Descriptor,
    filename: String,
    format: ArchiveFormat,
    size: u64,
    /// The class an edited package's archive already has.
    current: Option<Expression>,
}

/// The stored descriptor of one package payload: a ROM, extra or tar source
/// archive of at least 4 KiB is a deterministic `+zstd` frame, so the package
/// identity is known before anything is written and equals what publication
/// stores. A source that no longer has the scanned bytes is refused, naming it.
fn payload(path: &Path, kind: &str, digest: &Digest, size: u64) -> Result<Descriptor> {
    let changed =
        |reason: String| invalid(format!("input {digest} changed since the scan: {reason}"));
    let d = fs::File::open(path)
        .map_err(image_store::Error::from)
        .and_then(|file| {
            image_store::codec::describe(
                file,
                kind,
                size,
                image_store::codec::FIRMWARE_PAYLOAD_TYPES,
            )
        })
        .map_err(|e| changed(e.to_string()))?;
    let actual = d.decoded_digest()?;
    if &actual != digest {
        return Err(changed(format!("it now reads as {actual}")));
    }
    Ok(d)
}
/// The name a ROM image, source archive or document is exported under: its
/// file name as given, without the folder, refused unless it is a valid
/// input name ([`export_name::input_problem`]).
fn file_name(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("file name must be UTF-8"))?;
    if let Some(problem) = export_name::input_problem(name) {
        return Err(invalid(format!(
            "{path:?}: file name {name:?} {problem}; rename the file"
        )));
    }
    Ok(name.to_owned())
}
/// A zip or POSIX tar archive, recognized by its signature (PKWARE APPNOTE
/// §4.3.7 local file header or §4.3.16 end record; POSIX.1-2001 ustar magic
/// at offset 257), never by its name. Only format and SHA-256 are checked:
/// the user states that it is the corresponding source.
fn archive(path: &Path) -> Result<Archive> {
    use std::io::Read;
    let stat = fs::metadata(path)?;
    if !stat.is_file() || stat.len() == 0 {
        return Err(invalid("source archive must be a nonempty regular file"));
    }
    let mut head = Vec::new();
    fs::File::open(path)?.take(262).read_to_end(&mut head)?;
    let format = if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        ArchiveFormat::Zip
    } else if head.get(257..262) == Some(b"ustar") {
        ArchiveFormat::Tar
    } else {
        return Err(invalid("source archive must be a zip or POSIX tar archive"));
    };
    let descriptor = image_store::codec::describe(
        fs::File::open(path)?,
        format.media_type(),
        stat.len(),
        image_store::codec::FIRMWARE_PAYLOAD_TYPES,
    )?;
    Ok(Archive {
        payload: Payload::File(path.into()),
        descriptor,
        filename: file_name(path)?,
        format,
        size: stat.len(),
        current: None,
    })
}

/// A single-image ROM without a catalogue entry, added explicitly with its
/// role. Its fit and role are user claims; it stays unlisted, and importing it
/// never grants the run-only permission for unlisted firmware.
pub fn unlisted(
    path: &Path,
    id: &str,
    role: &str,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Candidate> {
    use std::io::Read;
    if !image_store::media_set::valid_id(id) {
        return Err(invalid(
            "package id must be lowercase letters, digits, '.', '_' or '-'",
        ));
    }
    if !firmware::ROLES.contains(&role) {
        return Err(invalid(format!(
            "unknown ROM role {role:?}; known: {}",
            firmware::ROLES.join(", ")
        )));
    }
    if catalogue.iter().any(|e| e.id == id) {
        return Err(invalid(format!(
            "{id} is a firmware catalogue id; an unlisted package needs another id"
        )));
    }
    let stat = fs::metadata(path)?;
    if !stat.is_file() || stat.len() == 0 || stat.len() > 1024 * 1024 {
        return Err(invalid(
            "an unlisted ROM is one regular file of 1 byte to 1 MiB",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let digest = Digest::of(&bytes);
    if let Some(entry) = catalogue.iter().find(|e| {
        (e.sha256 == digest.hex() && e.size == bytes.len() as u64)
            || e.parts
                .iter()
                .flat_map(|l| l.iter())
                .any(|p| p.sha256 == digest.hex() && p.size == bytes.len() as u64)
    }) {
        return Err(invalid(format!(
            "the ROM matches firmware catalogue entry {}; import it with --for {} instead",
            entry.id, entry.id
        )));
    }
    Ok(Candidate {
        catalogue_id: id.into(),
        config: Config {
            schema: 1,
            platform: crate::PLATFORM.into(),
            name: id.into(),
            vendor: None,
            version: None,
            date: None,
            notes: None,
            description: None,
            labels: Vec::new(),
            fits: toml::Table::new(),
            provenance: None,
            roms: vec![Rom {
                id: role.into(),
                role: role.into(),
                catalogue_id: None,
                sha256: digest.hex().into(),
                size: bytes.len() as u64,
                blob: Some(digest),
                position: None,
                part_number: None,
                requires: Vec::new(),
                parts: Vec::new(),
            }],
            sources: Vec::new(),
            extras: Vec::new(),
            readme: None,
        },
        sources: vec![path.into()],
        distribution: assembly::FirmwareDistribution::UserSupplied,
    })
}

/// What the firmware catalogue says about the package's ROMs, by bytes first:
/// a ROM or chip whose SHA-256 and length match a proprietary catalogue image
/// or chip is user-supplied whatever its `catalogue_id` says. Otherwise the
/// listed ids decide: user-supplied when any is proprietary, open with one
/// licence when all are open, nothing when a ROM is unlisted. Without a
/// catalogue (a preview only) the candidate's own classification stands in.
fn known(
    config: &Config,
    catalogue: Option<&[assembly::FirmwareEntry]>,
    fallback: Option<assembly::FirmwareDistribution>,
) -> Result<Option<assembly::FirmwareDistribution>> {
    if catalogue.is_some_and(|catalogue| locked_by(config, catalogue).is_some()) {
        return Ok(Some(assembly::FirmwareDistribution::UserSupplied));
    }
    let fitted = catalogue
        .map(|entries| Fit::decode(&config.fits)?.catalogue(entries))
        .transpose()?;
    let catalogue = fitted.as_deref();
    let mut listed = Vec::new();
    for id in config.roms.iter().filter_map(|r| r.catalogue_id.as_deref()) {
        listed.push(match catalogue {
            Some(catalogue) => {
                catalogue
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or_else(|| invalid(format!("unknown firmware catalogue id {id}")))?
                    .distribution
            }
            None => fallback.unwrap_or(assembly::FirmwareDistribution::UserSupplied),
        });
    }
    if listed.contains(&assembly::FirmwareDistribution::UserSupplied) {
        return Ok(Some(assembly::FirmwareDistribution::UserSupplied));
    }
    if listed.len() != config.roms.len() {
        return Ok(None);
    }
    let first = listed.first().copied();
    if listed.iter().any(|d| match (d, first) {
        (
            assembly::FirmwareDistribution::Open { license: a, .. },
            Some(assembly::FirmwareDistribution::Open { license: b, .. }),
        ) => *a != b,
        _ => false,
    }) {
        return Err(invalid(
            "the ROM images of one package must share one licence",
        ));
    }
    Ok(first)
}

/// The proprietary catalogue entry whose bytes a ROM or chip of the package
/// has: such a package is user-supplied whatever its ids or marks say.
pub fn locked_by<'a>(
    config: &Config,
    catalogue: &'a [assembly::FirmwareEntry],
) -> Option<&'a assembly::FirmwareEntry> {
    config.roms.iter().find_map(|rom| {
        firmware::proprietary(catalogue, &rom.sha256, rom.size).or_else(|| {
            rom.parts
                .iter()
                .find_map(|p| firmware::proprietary(catalogue, &p.sha256, p.size))
        })
    })
}

/// A source archive's own class: the licence the user states for it, the
/// catalogue licence when it is the very archive the project pins for the
/// catalogue ROM, or, in an edit, the class it already has. Bytes the
/// catalogue lists as proprietary are never open.
fn archive_license(
    archive: &Archive,
    known: Option<assembly::FirmwareDistribution>,
    statement: &Statement,
    catalogue: Option<&[assembly::FirmwareEntry]>,
) -> Result<Option<Expression>> {
    let digest = archive.descriptor.decoded_digest()?;
    let locked = catalogue.and_then(|c| firmware::proprietary(c, digest.hex(), archive.size));
    if let Some(stated) = &statement.source_license {
        if !statement.open {
            return Err(invalid(
                "a source licence statement marks the archive open; add --open",
            ));
        }
        if let Some(entry) = locked {
            return Err(invalid(format!(
                "the source archive has the bytes of {}, which the firmware catalogue lists as proprietary",
                entry.id
            )));
        }
        return Ok(Some(Expression::parse(stated)?));
    }
    if locked.is_some() {
        return Ok(None);
    }
    if let Some(assembly::FirmwareDistribution::Open {
        license,
        source_archive_sha256: Some(pin),
        ..
    }) = known
    {
        if pin == digest.hex() {
            return Ok(Some(Expression::parse(license)?));
        }
    }
    Ok(archive.current.clone())
}

/// The ROM images' licence after the statement, and notes for the user.
/// `default` is the import's catalogue licence or the edited package's
/// current one; `source` is `None` without an archive, else whether the
/// archive is open. Refuses `open` for proprietary catalogue firmware, `open`
/// without a licence, and an explicit copyleft `open` without an open source
/// archive. A catalogue-open copyleft ROM without one is kept user-supplied
/// with a note naming the fetch script that builds its archive.
fn rom_license(
    known: Option<assembly::FirmwareDistribution>,
    default: Option<Expression>,
    statement: &Statement,
    source: Option<bool>,
    policy: license::Policy,
) -> Result<(Option<Expression>, Vec<String>)> {
    if statement.open && statement.user_supplied {
        return Err(invalid("open and user-supplied exclude each other"));
    }
    if statement.license.is_some() && !statement.open {
        return Err(invalid(
            "a licence statement marks the ROM images open; add --open",
        ));
    }
    let stated = statement
        .license
        .as_deref()
        .map(Expression::parse)
        .transpose()?;
    let open_source = source == Some(true);
    let without_source = |license: &Expression, fetch: Option<&str>| {
        let how = match (source, fetch) {
            (Some(_), Some(fetch)) => format!(
                "the source archive is not the one {fetch} builds; state its licence with --open --source-license SPDX"
            ),
            (Some(_), None) => "state the source archive's licence with --source-license SPDX".into(),
            (None, Some(fetch)) => format!("add the archive {fetch} builds with --source"),
            (None, None) => {
                "add the source archive with --source and --source-license SPDX".into()
            }
        };
        format!(
            "{license} is copyleft, so the ROM images are open only together with an open source archive: {how}"
        )
    };
    match known {
        Some(assembly::FirmwareDistribution::UserSupplied) => {
            if statement.open {
                return Err(invalid(
                    "the firmware catalogue lists this ROM as proprietary (user-supplied); it cannot be marked open",
                ));
            }
            Ok((None, Vec::new()))
        }
        Some(assembly::FirmwareDistribution::Open { license, fetch, .. }) => {
            if statement.user_supplied || !(statement.open || default.is_some()) {
                return Ok((None, Vec::new()));
            }
            let catalogue = Expression::parse(license)?;
            if stated.as_ref().is_some_and(|s| s != &catalogue) {
                return Err(invalid(format!(
                    "the firmware catalogue records this ROM's licence as {catalogue}"
                )));
            }
            if policy.catalogue_copyleft_needs_source
                && catalogue.needs_source(policy)
                && !open_source
            {
                let reason = without_source(&catalogue, Some(fetch));
                if statement.open {
                    return Err(invalid(reason));
                }
                return Ok((None, vec![format!("kept user-supplied: {reason}")]));
            }
            Ok((Some(catalogue), Vec::new()))
        }
        None => {
            if statement.user_supplied {
                return Ok((None, Vec::new()));
            }
            let license = if statement.open {
                Some(stated.ok_or_else(|| {
                    invalid("--open needs --license SPDX: a licence is stated, never guessed")
                })?)
            } else {
                default
            };
            if let Some(license) = &license {
                if license.needs_source(policy) && !open_source {
                    return Err(invalid(without_source(license, None)));
                }
            }
            Ok((license, Vec::new()))
        }
    }
}

/// `None` for an empty or blank text, which clears an optional field.
fn stated(text: &Option<String>) -> Option<String> {
    text.clone().filter(|t| !t.trim().is_empty())
}

/// Apply the descriptive part of a statement to a config.
fn apply(
    config: &mut Config,
    statement: &Statement,
    catalogue: Option<&[assembly::FirmwareEntry]>,
) -> Result<()> {
    if let Some(title) = &statement.title {
        if !image_store::media_set::valid_text(title) {
            return Err(invalid("the title must be one line of 1 to 128 characters"));
        }
        config.name = title.clone();
    }
    for (field, value) in [
        (&mut config.description, &statement.description),
        (&mut config.notes, &statement.notes),
        (&mut config.vendor, &statement.vendor),
        (&mut config.version, &statement.version),
        (&mut config.date, &statement.date),
    ] {
        if value.is_some() {
            *field = stated(value);
        }
    }
    if config
        .date
        .as_deref()
        .is_some_and(|d| !image_store::firmware::valid_date(d))
    {
        return Err(invalid("the date must be a calendar date, YYYY-MM-DD"));
    }
    let sorted = |items: &Vec<String>| {
        let mut items = items.clone();
        items.sort();
        items
    };
    if let Some(labels) = &statement.labels {
        config.labels = sorted(labels);
    }
    if statement.boards.is_some() || statement.machine_classes.is_some() {
        let mut fit = Fit::decode(&config.fits)?;
        if let Some(boards) = &statement.boards {
            fit.boards = sorted(boards);
        }
        if let Some(classes) = &statement.machine_classes {
            fit.machine_classes = sorted(classes);
        }
        // Metadata edits may narrow a catalogue claim even though package checks accept a union of boards.
        if let Some(catalogue) = catalogue {
            let entries = fit.catalogue(catalogue)?;
            for id in config
                .roms
                .iter()
                .filter_map(|rom| rom.catalogue_id.as_deref())
            {
                if let Some(entry) = entries.iter().find(|entry| entry.id == id) {
                    let allowed = catalogue_fit(entry);
                    if fit
                        .boards
                        .iter()
                        .any(|board| !allowed.boards.contains(board))
                        || fit
                            .machine_classes
                            .iter()
                            .any(|class| !allowed.machine_classes.contains(class))
                    {
                        return Err(invalid("firmware catalogue fit may only be narrowed"));
                    }
                }
            }
        }
        config.fits = fit.table()?;
    }
    if let Some(requires) = &statement.requires {
        if config.roms.iter().any(|r| r.catalogue_id.is_some()) {
            return Err(invalid(
                "a catalogue ROM's required devices come from the firmware catalogue",
            ));
        }
        if config.roms.len() != 1 {
            return Err(invalid("required devices need a package with one ROM"));
        }
        if let Some(kind) = requires
            .iter()
            .find(|k| !firmware::DEVICE_KINDS.contains(&k.as_str()))
        {
            return Err(invalid(format!(
                "unknown device kind {kind:?}; known: {}",
                firmware::DEVICE_KINDS.join(", ")
            )));
        }
        config.roms[0].requires = sorted(requires);
    }
    if let Some(p) = &statement.provenance {
        let p = Provenance {
            kind: p.kind,
            from: p.from.clone(),
            revision: stated(&p.revision),
            url: stated(&p.url),
            date: stated(&p.date),
        };
        let blank = p.from.trim().is_empty();
        config.provenance = if blank
            && p.kind.is_none()
            && p.revision.is_none()
            && p.url.is_none()
            && p.date.is_none()
        {
            None
        } else if blank {
            return Err(invalid(
                "the provenance needs its details: where the bytes came from",
            ));
        } else if p
            .date
            .as_deref()
            .is_some_and(|d| !image_store::firmware::valid_date(d))
        {
            return Err(invalid(
                "the provenance date must be a calendar date, YYYY-MM-DD",
            ));
        } else {
            Some(p)
        };
    }
    Ok(())
}

/// A README written by hand, with CR LF and lone CR line ends turned into
/// LF, as the generated text has.
fn hand_written(text: &str) -> Result<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.trim().is_empty() {
        return Err(invalid(
            "the README must not be empty; regenerate it from the fields instead",
        ));
    }
    if text.len() as u64 > image_store::MAX_SMALL_BLOB {
        return Err(invalid("the README must be at most 1 MiB"));
    }
    Ok(text)
}

/// Mark every layer, generate the README (or take the one written by hand),
/// encode the config and order the layers by the config's payload traversal.
fn assemble(
    mut config: Config,
    roms: Vec<(Payload, Descriptor)>,
    source: Option<(Archive, Option<Expression>)>,
    extras: Vec<(Payload, Descriptor, Option<Expression>)>,
    rom_license: Option<Expression>,
    notes: Vec<String>,
    readme: Option<&str>,
) -> Result<Plan> {
    config.readme = None;
    config.sources = Vec::new();
    let mut pool = Vec::new();
    let link = match &source {
        Some((archive, _)) => Some(archive.descriptor.decoded_digest()?),
        None => None,
    };
    let source_license = source.as_ref().and_then(|(_, l)| l.clone());
    for (payload, mut d) in roms {
        license::mark(&mut d, rom_license.as_ref());
        d.annotations.remove("unemu.provenance");
        match &link {
            Some(link) => d
                .annotations
                .insert(license::SOURCE.into(), link.to_string()),
            None => d.annotations.remove(license::SOURCE),
        };
        pool.push((payload, d));
    }
    if let (Some((archive, class)), Some(link)) = (source, link) {
        let mut d = archive.descriptor;
        license::mark(&mut d, class.as_ref());
        export_name::assign(&mut d, &archive.filename);
        config.sources.push(Source {
            blob: link,
            filename: archive.filename,
            size: archive.size,
            format: archive.format,
            roms: config.roms.iter().map(|r| r.id.clone()).collect(),
        });
        pool.push((archive.payload, d));
    }
    let mut marks = Vec::new();
    for ((payload, mut d, license), extra) in extras.into_iter().zip(&config.extras) {
        license::mark(&mut d, license.as_ref());
        export_name::assign(&mut d, &extra.filename);
        marks.push(license);
        pool.push((payload, d));
    }
    let all_open = rom_license.is_some()
        && (config.sources.is_empty() || source_license.is_some())
        && marks.iter().all(Option::is_some);
    let hand = readme.map(hand_written).transpose()?;
    let text = match &hand {
        Some(text) => text.clone(),
        None => crate::firmware_readme::generate(
            &config,
            &crate::firmware_readme::Marks {
                roms: rom_license.as_ref(),
                source: source_license.as_ref(),
                extras: &marks,
                all_open,
            },
        ),
    };
    let metadata = all_open
        .then(|| Expression::parse(firmware::METADATA_LICENSE))
        .transpose()?;
    let bytes = text.clone().into_bytes();
    let mut readme = Descriptor::new(mt::MARKDOWN, Digest::of(&bytes), bytes.len() as u64);
    license::mark(&mut readme, metadata.as_ref());
    export_name::assign(&mut readme, export_name::README);
    if hand.is_some() {
        readme
            .annotations
            .insert(README_ORIGIN.into(), HAND_WRITTEN.into());
    }
    config.readme = Some(Readme {
        blob: readme.digest.clone(),
        size: readme.size,
    });
    pool.push((Payload::Memory(bytes), readme));
    let encoded = config.encode()?;
    let mut config_descriptor = Descriptor::new(
        mt::FIRMWARE_CONFIG,
        Digest::of(&encoded),
        encoded.len() as u64,
    );
    license::mark(&mut config_descriptor, metadata.as_ref());
    let mut payloads: Vec<(Payload, Descriptor)> = Vec::new();
    for (digest, _, _) in config.payloads() {
        if payloads.iter().any(|(_, d)| d.names(&digest)) {
            continue;
        }
        let index = pool
            .iter()
            .position(|(_, d)| d.names(&digest))
            .ok_or_else(|| invalid("package payload missing from the plan"))?;
        payloads.push(pool.swap_remove(index));
    }
    // Every layer exports to its own file: a ROM image, the source archive or
    // a document named like another layer, or like README.md, is refused,
    // naming both inputs.
    let layers: Vec<Descriptor> = payloads.iter().map(|(_, d)| d.clone()).collect();
    export_name::check_with(&layers, &|index| match &payloads[index] {
        (Payload::File(path), _) => format!("{path:?}"),
        (Payload::Memory(_), _) => "the README".into(),
        (Payload::Stored, d) => format!(
            "the stored layer {:?}",
            export_name::of(d).unwrap_or(&d.digest.to_string())
        ),
    })?;
    let mut manifest = Manifest::new_firmware(&config.platform, config_descriptor, layers)?;
    annotate(&mut manifest, &config, all_open);
    Ok(Plan {
        manifest,
        config: encoded,
        decoded: config,
        readme: text,
        payloads,
        notes,
    })
}

/// A stored package's ROM licence: the most restrictive ROM mark, so one
/// user-supplied chip keeps the whole package's ROM images user-supplied.
pub fn current_rom_license(config: &Config, manifest: &Manifest) -> Option<Expression> {
    config
        .payloads()
        .iter()
        .filter(|(_, _, kind)| *kind == mt::FIRMWARE)
        .map(|(digest, _, _)| {
            manifest
                .layers
                .iter()
                .find(|d| d.names(digest))
                .and_then(license::open_license)
        })
        .collect::<Option<Vec<_>>>()
        .and_then(|l| l.into_iter().next())
}

/// The kind a document attached to a package gets from its file name.
pub fn extra_kind(path: &Path) -> ExtraKind {
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "pdf" | "md" | "doc" => ExtraKind::Document,
        "zip" | "7z" | "arj" | "rar" => ExtraKind::Archive,
        "exe" | "com" | "bat" => ExtraKind::Executable,
        _ => ExtraKind::Other,
    }
}

/// The README the program generates for a stored package from its config
/// and its layers' marks, as an import or edit with no hand-written README
/// would write it; a stored README that differs was written by hand.
pub fn generated_readme(config: &Config, manifest: &Manifest) -> String {
    let class = |digest: &Digest| {
        manifest
            .layers
            .iter()
            .find(|d| d.names(digest))
            .and_then(license::open_license)
    };
    let roms = current_rom_license(config, manifest);
    let source = config.sources.first().and_then(|s| class(&s.blob));
    let extras: Vec<Option<Expression>> = config.extras.iter().map(|e| class(&e.blob)).collect();
    let all_open = roms.is_some()
        && (config.sources.is_empty() || source.is_some())
        && extras.iter().all(Option::is_some);
    crate::firmware_readme::generate(
        config,
        &crate::firmware_readme::Marks {
            roms: roms.as_ref(),
            source: source.as_ref(),
            extras: &extras,
            all_open,
        },
    )
}

/// Standard OCI annotations where they fit; there is no creation time, because
/// none is supplied and the host clock never enters a package.
fn annotate(manifest: &mut Manifest, config: &Config, all_open: bool) {
    let annotations = &mut manifest.annotations;
    annotations.insert(export_name::TITLE.into(), config.name.clone());
    for (key, value) in [
        ("org.opencontainers.image.description", &config.description),
        ("org.opencontainers.image.vendor", &config.vendor),
        ("org.opencontainers.image.version", &config.version),
        (
            "org.opencontainers.image.source",
            &config.provenance.as_ref().and_then(|p| p.url.clone()),
        ),
        (
            "org.opencontainers.image.revision",
            &config.provenance.as_ref().and_then(|p| p.revision.clone()),
        ),
    ] {
        if let Some(value) = value {
            annotations.insert(key.into(), value.clone());
        }
    }
    if all_open {
        let licenses: Vec<Expression> = manifest
            .layers
            .iter()
            .filter_map(license::open_license)
            .collect();
        if let Some(all) = Expression::all(&licenses) {
            manifest.annotations.insert(
                "org.opencontainers.image.licenses".into(),
                all.as_str().into(),
            );
        }
    }
}

/// Reads a plan's payloads for provider validation before anything is stored.
struct PlanBlobs<'a> {
    payloads: &'a [(Payload, Descriptor)],
    store: Option<&'a Store>,
}
impl BlobSource for PlanBlobs<'_> {
    fn read_verified(&self, d: &Descriptor, limit: u64) -> image_store::Result<Vec<u8>> {
        use std::io::Read;
        let missing = || image_store::Error::Invalid("payload missing from the plan".into());
        let (payload, _) = self
            .payloads
            .iter()
            .find(|(_, p)| p.digest == d.digest)
            .ok_or_else(missing)?;
        let size = d.decoded_size()?;
        if size > limit {
            return Err(image_store::Error::Invalid(
                "payload exceeds its read limit".into(),
            ));
        }
        let bytes = match payload {
            Payload::File(path) => {
                let mut bytes = Vec::new();
                fs::File::open(path)?
                    .take(limit + 1)
                    .read_to_end(&mut bytes)?;
                bytes
            }
            Payload::Memory(bytes) => bytes.clone(),
            Payload::Stored => return self.store.ok_or_else(missing)?.read_verified(d, limit),
        };
        if bytes.len() as u64 != size || Digest::of(&bytes) != d.decoded_digest()? {
            return Err(image_store::Error::Invalid(
                "input changed during import preview".into(),
            ));
        }
        Ok(bytes)
    }
}
fn validate(
    plan: &Plan,
    catalogue: &[assembly::FirmwareEntry],
    store: Option<&Store>,
) -> Result<()> {
    let table: toml::Table = std::str::from_utf8(&plan.config)
        .map_err(|e| invalid(e.to_string()))?
        .parse()
        .map_err(|e: toml::de::Error| invalid(e.to_string()))?;
    image_store::ConfigValidator::validate(
        &Validator(&PcFirmwareRules(catalogue)),
        &table,
        &plan.manifest.layers,
        &PlanBlobs {
            payloads: &plan.payloads,
            store,
        },
    )?;
    Ok(())
}

/// Compute the immutable package identity without writing any store objects.
pub fn preview_manifest(candidate: &Candidate, attachments: &[Attachment]) -> Result<Manifest> {
    Ok(plan(
        candidate,
        attachments,
        &Statement::default(),
        None,
        crate::compat::levels(image_store::compat::EXTRACTION_BASELINE),
    )?
    .manifest)
}
/// What an import or edit with a statement would publish.
#[derive(Debug)]
pub struct Preview {
    pub manifest: Manifest,
    /// What the user should know, such as why open firmware stays private.
    pub notes: Vec<String>,
    /// The config and the README text it would store, for a frontend to show.
    pub config: Config,
    pub readme: String,
}
impl From<Plan> for Preview {
    fn from(plan: Plan) -> Self {
        Self {
            manifest: plan.manifest,
            notes: plan.notes,
            config: plan.decoded,
            readme: plan.readme,
        }
    }
}
/// The package identity an import with this statement publishes, checked by
/// the provider against `catalogue` before anything is written. Rights come
/// from the catalogue and the bytes, never from the candidate's own fields.
pub fn preview_manifest_with(
    candidate: &Candidate,
    attachments: &[Attachment],
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Preview> {
    preview_manifest_at(
        candidate,
        attachments,
        statement,
        catalogue,
        crate::compat::levels(image_store::compat::EXTRACTION_BASELINE),
    )
}

pub fn preview_manifest_at(
    candidate: &Candidate,
    attachments: &[Attachment],
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
    levels: image_store::compat::Levels,
) -> Result<Preview> {
    let plan = plan(candidate, attachments, statement, Some(catalogue), levels)?;
    validate(&plan, catalogue, None)?;
    Ok(plan.into())
}

fn plan(
    candidate: &Candidate,
    attachments: &[Attachment],
    statement: &Statement,
    catalogue: Option<&[assembly::FirmwareEntry]>,
    levels: image_store::compat::Levels,
) -> Result<Plan> {
    let mut config = candidate.config.clone();
    let bindings = config.payloads();
    if bindings.len() != candidate.sources.len() {
        return Err(invalid("firmware source count differs from bindings"));
    }
    if statement.extra_licenses.len() > attachments.len() {
        return Err(invalid("more document licences than documents"));
    }
    let mut roms = Vec::new();
    for ((digest, size, kind), path) in bindings.into_iter().zip(&candidate.sources) {
        let mut d = payload(path, kind, &digest, size)?;
        export_name::assign(&mut d, &file_name(path)?);
        roms.push((Payload::File(path.clone()), d));
    }
    let mut extras = Vec::new();
    for (index, extra) in attachments.iter().enumerate() {
        if let Some(name) = &extra.dos_name {
            crate::media_set::PcMediaFormats.validate_name(name)?;
        }
        let stat = fs::metadata(&extra.path)?;
        if !stat.is_file() {
            return Err(invalid("extra must be a regular file"));
        }
        let filename = file_name(&extra.path)?;
        let digest: Digest = format!(
            "sha256:{}",
            machine_file::firmware_discovery::hash_firmware_file(&extra.path)?
        )
        .parse()?;
        let kind = extra_kind(&extra.path);
        config.extras.push(Extra {
            blob: digest.clone(),
            filename,
            size: stat.len(),
            kind,
            dos_name: extra.dos_name.clone(),
        });
        let license = statement
            .extra_licenses
            .get(index)
            .cloned()
            .flatten()
            .map(|l| Expression::parse(&l))
            .transpose()?;
        if let Some(entry) = catalogue
            .filter(|_| license.is_some())
            .and_then(|c| firmware::proprietary(c, digest.hex(), stat.len()))
        {
            return Err(invalid(format!(
                "document {:?} has the bytes of {}, which the firmware catalogue lists as proprietary; it cannot be marked open",
                extra.path.file_name().unwrap_or_default(),
                entry.id
            )));
        }
        extras.push((
            Payload::File(extra.path.clone()),
            payload(&extra.path, mt::EXTRA, &digest, stat.len())?,
            license,
        ));
    }
    let source = statement.source.as_deref().map(archive).transpose()?;
    apply(&mut config, statement, catalogue)?;
    let known = known(&config, catalogue, Some(candidate.distribution))?;
    let default = match known {
        Some(assembly::FirmwareDistribution::Open { license, .. }) => {
            Some(Expression::parse(license)?)
        }
        _ => None,
    };
    let readme = match &statement.readme {
        ReadmeChoice::Text(text) => Some(text.as_str()),
        ReadmeChoice::Keep | ReadmeChoice::Generate => None,
    };
    decide(
        config, roms, source, extras, known, default, statement, catalogue, readme, levels,
    )
}

/// Classify the source archive, then the ROM images, and assemble the plan.
#[allow(clippy::too_many_arguments)]
fn decide(
    config: Config,
    roms: Vec<(Payload, Descriptor)>,
    source: Option<Archive>,
    extras: Vec<(Payload, Descriptor, Option<Expression>)>,
    known: Option<assembly::FirmwareDistribution>,
    default: Option<Expression>,
    statement: &Statement,
    catalogue: Option<&[assembly::FirmwareEntry]>,
    readme: Option<&str>,
    levels: image_store::compat::Levels,
) -> Result<Plan> {
    if statement.source_license.is_some() && source.is_none() {
        return Err(invalid(
            "a source licence needs a source archive (--source)",
        ));
    }
    let source = match source {
        Some(archive) => {
            let class = archive_license(&archive, known, statement, catalogue)?;
            Some((archive, class))
        }
        None => None,
    };
    let (rom_license, notes) = rom_license(
        known,
        default,
        statement,
        source.as_ref().map(|(_, class)| class.is_some()),
        license::ACTIVE,
    )?;
    let mut plan = assemble(config, roms, source, extras, rom_license, notes, readme)?;
    crate::compat::stamp(
        &mut plan.manifest,
        &plan.config,
        image_store::compat::BASELINE,
        statement.min_app_version,
        levels,
    )?;
    Ok(plan)
}

/// The ROM and document layers' decoded identities in manifest order.
fn bound_layers(manifest: &Manifest) -> Result<Vec<(String, Digest, u64)>> {
    manifest
        .layers
        .iter()
        .filter(|d| matches!(d.decoded_media_type(), mt::FIRMWARE | mt::EXTRA))
        .map(|d| {
            Ok((
                d.decoded_media_type().to_owned(),
                d.decoded_digest()?,
                d.decoded_size()?,
            ))
        })
        .collect()
}

/// The package a tag already names for this import, verified: one that holds
/// the same ROM and document bytes, whatever its metadata, licences, source
/// archive or README (an earlier version wrote it, or an import with another
/// statement). Any other package under the tag conflicts before anything is
/// copied. Changing an imported package's metadata is [`edit`]'s job.
pub fn existing(
    store: &Store,
    tag: &str,
    manifest: &Manifest,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Option<Descriptor>> {
    let digest = Digest::of(&manifest.encode()?);
    match store.tag_descriptor(tag) {
        Ok(existing) => {
            let image =
                store.resolve_descriptor(&existing, &Validator(&PcFirmwareRules(catalogue)))?;
            if existing.digest == digest
                || bound_layers(&image.manifest)? == bound_layers(manifest)?
            {
                return Ok(Some(Descriptor::new(
                    mt::MANIFEST,
                    existing.digest,
                    existing.size,
                )));
            }
            Err(image_store::Error::Conflict {
                current: Some(existing.digest),
            }
            .into())
        }
        Err(image_store::Error::TagNotFound(_)) => {
            store.check_new_tag(tag)?;
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}
/// Check only a selected, unambiguous candidate; see [`existing`].
pub fn already_imported(
    store: &Store,
    tag: &str,
    manifest: &Manifest,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<bool> {
    Ok(existing(store, tag, manifest, catalogue)?.is_some())
}

/// Copy a plan's new payloads, its config and manifest, then publish `tag`
/// by compare-and-swap from absent. Stored payloads are already present.
/// With `judged`, the stored manifest must have that digest before the tag
/// is written.
fn store_plan(
    store: &Store,
    plan: Plan,
    tag: &str,
    catalogue: &[assembly::FirmwareEntry],
    judged: Option<&Digest>,
) -> Result<Descriptor> {
    for (payload, descriptor) in &plan.payloads {
        let kind = descriptor.decoded_media_type();
        let size = descriptor.decoded_size()?;
        let expected = descriptor.decoded_digest()?;
        let stored = match payload {
            Payload::File(path) => store.import_payload(
                fs::File::open(path)?,
                kind,
                size,
                Some(&expected),
                image_store::codec::FIRMWARE_PAYLOAD_TYPES,
            )?,
            Payload::Memory(bytes) => store.import_payload(
                bytes.as_slice(),
                kind,
                size,
                Some(&expected),
                image_store::codec::FIRMWARE_PAYLOAD_TYPES,
            )?,
            Payload::Stored => continue,
        };
        if stored.digest != descriptor.digest || stored.size != descriptor.size {
            return Err(invalid("source changed during import"));
        }
    }
    store.import(plan.config.as_slice(), Some(&plan.manifest.config.digest))?;
    let d = store.import_manifest(&plan.manifest)?;
    unjudged(judged, &d.digest)?;
    let validator = Validator(&PcFirmwareRules(catalogue));
    match store.update_tag(tag, &d, None, &validator) {
        Ok(()) => (),
        Err(image_store::Error::Conflict {
            current: Some(current),
        }) if current == d.digest => {
            store.resolve_descriptor(&d, &validator)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(d)
}

/// Pre-check the target before copying, then verify the closure and publish by
/// CAS, with the catalogue's prefill and no further statement.
pub fn publish(
    store: &Store,
    candidate: &Candidate,
    attachments: &[Attachment],
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Descriptor> {
    publish_with(
        store,
        candidate,
        attachments,
        &Statement::default(),
        catalogue,
    )
}
/// [`publish`] with the user's descriptive fields and licence statement. A tag
/// that already holds these ROMs is reused as it is: changing an imported
/// package's metadata is [`edit`]'s job, so a statement that would change it
/// is refused rather than dropped.
pub fn publish_with(
    store: &Store,
    candidate: &Candidate,
    attachments: &[Attachment],
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Descriptor> {
    publish_as(
        store,
        candidate,
        attachments,
        statement,
        catalogue,
        &import_tag(store, &candidate.catalogue_id)?,
        None,
    )
}
/// [`publish_with`] under a chosen local tag instead of [`package_tag`],
/// such as the version tag of an official template package. With `judged`,
/// only a package with that manifest digest is tagged or reused: the one a
/// caller checked, for example against a push guard.
pub fn publish_as(
    store: &Store,
    candidate: &Candidate,
    attachments: &[Attachment],
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
    tag: &str,
    judged: Option<&Digest>,
) -> Result<Descriptor> {
    let plan = plan(
        candidate,
        attachments,
        statement,
        Some(catalogue),
        crate::compat::source_levels(store),
    )?;
    validate(&plan, catalogue, None)?;
    let digest = Digest::of(&plan.manifest.encode()?);
    unjudged(judged, &digest)?;
    if let Some(existing) = existing(store, tag, &plan.manifest, catalogue)? {
        if existing.digest != digest && *statement != Statement::default() {
            let stored =
                Manifest::decode(&store.read_verified(&existing, image_store::MAX_SMALL_BLOB)?)?;
            if unnamed(&stored) == unnamed(&plan.manifest) {
                let names: Vec<&str> = stored.layers.iter().filter_map(export_name::of).collect();
                return Err(invalid(format!(
                    "{tag} already holds this package with the same metadata, only under other \
                     file names ({}); a package keeps the names it was imported with: rename \
                     the files to match, or import without the statement to keep it",
                    if names.is_empty() {
                        "none, as earlier releases stored packages".to_owned()
                    } else {
                        names.join(", ")
                    }
                )));
            }
            return Err(invalid(format!(
                "{tag} already holds these ROMs with other metadata, licences, minimum \
                 version or file names; an edit (image firmware edit, or Edit metadata… in the \
                 library) publishes them as a new revision"
            )));
        }
        // An earlier package with the same ROMs is kept as it is.
        unjudged(judged, &existing.digest)?;
        return Ok(existing);
    }
    store_plan(store, plan, tag, catalogue, judged)
}
/// `manifest` without its layers' export file names.
fn unnamed(manifest: &Manifest) -> Manifest {
    let mut manifest = manifest.clone();
    for layer in &mut manifest.layers {
        layer.annotations.remove(export_name::TITLE);
    }
    manifest
}
/// Refuse a package other than the one the caller judged.
fn unjudged(judged: Option<&Digest>, digest: &Digest) -> Result<()> {
    match judged {
        Some(judged) if judged != digest => Err(invalid(format!(
            "the package is {digest}, not the checked {judged}"
        ))),
        _ => Ok(()),
    }
}

/// The tag after `name:TAG`: the next revision of a revision tag
/// (`name:r1` gives `name:r2`, `name:2x32k-r1` gives `name:2x32k-r2`), and
/// `name:rN+1` for a plain number `N`, the first packaging an earlier
/// release numbered `1`. `None` for any other tag.
pub fn next_revision(tag: &str) -> Option<String> {
    let (name, revision) = tag.rsplit_once(':')?;
    if let Some(next) = image_store::tag::next_revision(revision) {
        return Some(format!("{name}:{next}"));
    }
    if revision.is_empty()
        || revision.starts_with('0')
        || !revision.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(format!(
        "{name}:{}",
        image_store::tag::revision(None, revision.parse::<u64>().ok()?.checked_add(1)?)
    ))
}

/// Publish a new revision of the package `tag` under the new tag `target`,
/// with the statement applied: same ROM and document blobs, a new config and
/// README, and new marks. A package is immutable, so `tag` keeps naming the
/// old revision. A new `source` replaces the package's source archive.
pub fn edit(
    store: &Store,
    tag: &str,
    target: &str,
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Revision> {
    let (plan, unchanged) = edit_plan(store, tag, target, statement, catalogue)?;
    if unchanged {
        return Err(invalid(
            "the edit changes nothing; the package stays as it is",
        ));
    }
    let notes = plan.notes.clone();
    Ok(Revision {
        descriptor: store_plan(store, plan, target, catalogue, None)?,
        notes,
    })
}

/// What [`edit`] with this statement would publish under `target`, checked
/// as the edit checks it, without writing anything.
#[derive(Debug)]
pub struct EditPreview {
    pub preview: Preview,
    /// The statement changes nothing, so an edit would refuse it.
    pub unchanged: bool,
}
/// The dry run of [`edit`].
pub fn preview_edit(
    store: &Store,
    tag: &str,
    target: &str,
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<EditPreview> {
    let (plan, unchanged) = edit_plan(store, tag, target, statement, catalogue)?;
    Ok(EditPreview {
        preview: plan.into(),
        unchanged,
    })
}

/// The firmware catalogue's verdict on a package's ROMs, by bytes first as
/// import and edit decide it: `UserSupplied` when any ROM or chip has the
/// bytes of a proprietary entry or a listed id is proprietary, `Open` when
/// every ROM is listed open, `None` when a ROM is unlisted.
pub fn catalogue_rights(
    config: &Config,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Option<assembly::FirmwareDistribution>> {
    known(config, Some(catalogue), None)
}

/// The fits the catalogue lists for an entry, which a package may narrow:
/// its boards, and the AT machine class for an IBM 5170 board.
pub fn catalogue_fit(entry: &assembly::FirmwareEntry) -> Fit {
    Fit {
        boards: sorted(entry.boards.iter().copied()),
        machine_classes: if entry.boards.contains(&"ibm-5170") {
            vec!["at".into()]
        } else {
            Vec::new()
        },
    }
}

/// An edit's plan, and whether it would publish the very same package.
fn edit_plan(
    store: &Store,
    tag: &str,
    target: &str,
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<(Plan, bool)> {
    if !statement.extra_licenses.is_empty() {
        return Err(invalid("an edit keeps the package's documents as they are"));
    }
    store.check_new_tag(target)?;
    let image = store.resolve(tag, &Validator(&PcFirmwareRules(catalogue)))?;
    let declared = image_store::compat::Requirement::of(&image.manifest).release()?;
    if let Some(stated) = statement
        .min_app_version
        .filter(|stated| *stated < declared)
    {
        return Err(invalid(format!(
            "{tag} already declares {declared}; a stated minimum version ({stated}) only raises it"
        )));
    }
    let mut config = Config::decode(&image.config)?;
    // A new revision names the platform by its current id, also when the
    // package it edits names the retired one.
    config.platform = crate::PLATFORM.into();
    if config.sources.len() > 1 {
        return Err(invalid(
            "a package with several source archives cannot be edited",
        ));
    }
    let layer = |digest: &Digest| {
        image
            .manifest
            .layers
            .iter()
            .find(|d| d.names(digest))
            .cloned()
            .ok_or_else(|| invalid("package layer missing"))
    };
    // A ROM layer keeps the file name it was imported under. A package
    // written before ROM layers had names gets `<rom id>.bin`, or
    // `<rom id>-<n>.bin` for its n-th chip, since the original name is gone.
    // When that is no valid input name or another layer's, it gets
    // `<rom id>-rom.bin` (`<rom id>-rom-<n>.bin`); failing both, the edit is
    // refused.
    let mut blobs = Vec::new();
    for rom in &config.roms {
        blobs.extend(rom.blob.iter().map(|blob| (rom, None, blob)));
        blobs.extend(
            (rom.parts.iter().enumerate()).map(|(chip, part)| (rom, Some(chip), &part.blob)),
        );
    }
    let stored = blobs
        .iter()
        .map(|(_, _, digest)| layer(digest))
        .collect::<Result<Vec<_>>>()?;
    let kept = |d: &Descriptor| {
        export_name::decoded(d)
            .or(export_name::of(d))
            .map(str::to_owned)
    };
    let mut taken = BTreeSet::new();
    let new_source = statement.source.as_deref().map(file_name).transpose()?;
    let names = (config.extras.iter().map(|e| e.filename.clone()))
        .chain(config.sources.iter().map(|s| s.filename.clone()))
        .chain(new_source)
        .chain([export_name::README.to_owned()])
        .chain(stored.iter().filter_map(kept));
    for name in names {
        occupy(&mut taken, &name);
    }
    let mut roms = Vec::new();
    for ((rom, chip, _), mut d) in blobs.into_iter().zip(stored) {
        let name = match kept(&d) {
            Some(name) => name,
            None => {
                let chip = chip
                    .map(|chip| format!("-{}", chip + 1))
                    .unwrap_or_default();
                let choices = [
                    format!("{}{chip}.bin", rom.id),
                    format!("{}-rom{chip}.bin", rom.id),
                ];
                let name = (choices.iter())
                    .find(|name| export_name::input_problem(name).is_none() && free(&taken, name))
                    .cloned()
                    .ok_or_else(|| {
                        invalid(format!(
                            "ROM {}: the package stores it without a file name, and neither {:?} \
                             nor {:?} can name it; import the ROM again from its file",
                            rom.id, choices[0], choices[1]
                        ))
                    })?;
                occupy(&mut taken, &name);
                name
            }
        };
        export_name::assign(&mut d, &name);
        roms.push((Payload::Stored, d));
    }
    let current = current_rom_license(&config, &image.manifest);
    // A README written by hand stays unless the statement replaces it.
    let kept = match &statement.readme {
        ReadmeChoice::Keep => match image
            .manifest
            .layers
            .iter()
            .find(|d| d.decoded_media_type() == mt::MARKDOWN && hand_written_readme(d))
        {
            Some(d) => Some(
                String::from_utf8(store.read_verified(d, image_store::MAX_SMALL_BLOB)?)
                    .map_err(|_| invalid("the README layer is not UTF-8 text"))?,
            ),
            None => None,
        },
        ReadmeChoice::Generate => None,
        ReadmeChoice::Text(text) => Some(text.clone()),
    };
    let source = match (&statement.source, config.sources.first()) {
        (Some(path), _) => Some(archive(path)?),
        (None, Some(existing)) => {
            let descriptor = layer(&existing.blob)?;
            Some(Archive {
                payload: Payload::Stored,
                current: license::open_license(&descriptor),
                descriptor,
                filename: existing.filename.clone(),
                format: existing.format,
                size: existing.size,
            })
        }
        (None, None) => None,
    };
    let mut extras = Vec::new();
    for extra in &config.extras {
        let d = layer(&extra.blob)?;
        let license = license::open_license(&d);
        extras.push((Payload::Stored, d, license));
    }
    apply(&mut config, statement, Some(catalogue))?;
    let known = known(&config, Some(catalogue), None)?;
    let mut plan = decide(
        config,
        roms,
        source,
        extras,
        known,
        current,
        statement,
        Some(catalogue),
        kept.as_deref(),
        crate::compat::source_levels(store),
    )?;
    image_store::compat::carry(
        &mut plan.manifest,
        &image.manifest,
        crate::compat::source_levels(store),
    )?;
    validate(&plan, catalogue, Some(store))?;
    let unchanged = Digest::of(&plan.manifest.encode()?) == image.image.digest;
    Ok((plan, unchanged))
}

/// A published package revision.
#[derive(Debug)]
pub struct Revision {
    pub descriptor: Descriptor,
    /// What the user should know, such as why open firmware stays private.
    pub notes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    const OPEN: assembly::FirmwareDistribution = assembly::FirmwareDistribution::Open {
        fetch: "synthetic/fetch.sh",
        license: "GPL-3.0-or-later",
        provenance: "synthetic build",
        source_archive_sha256: None,
    };
    /// `source`: `None` without an archive, else whether the archive is open.
    fn decide(
        known: Option<assembly::FirmwareDistribution>,
        statement: &Statement,
        source: Option<bool>,
        policy: license::Policy,
    ) -> Result<(Option<String>, Vec<String>)> {
        let default = match known {
            Some(assembly::FirmwareDistribution::Open { license, .. }) => {
                Some(Expression::parse(license).unwrap())
            }
            _ => None,
        };
        rom_license(known, default, statement, source, policy)
            .map(|(l, notes)| (l.map(|l| l.as_str().to_owned()), notes))
    }
    #[test]
    fn a_catalogue_open_copyleft_rom_without_open_source_stays_user_supplied() {
        let none = Statement::default();
        assert_eq!(license::ACTIVE, license::Policy::STRICT);
        for source in [None, Some(false)] {
            let (license, notes) = decide(Some(OPEN), &none, source, license::ACTIVE).unwrap();
            assert_eq!(license, None);
            assert!(notes[0].contains("synthetic/fetch.sh"), "{notes:?}");
        }
        assert!(notes_free(decide(
            Some(OPEN),
            &none,
            Some(true),
            license::ACTIVE
        )));
        // An explicit open statement that cannot hold is refused, not downgraded.
        let stated = Statement {
            open: true,
            ..Statement::default()
        };
        let err = decide(Some(OPEN), &stated, None, license::ACTIVE).unwrap_err();
        assert!(err.to_string().contains("copyleft"), "{err}");
        // The switch still reads the decision's earlier, looser rule.
        let loose = license::Policy {
            catalogue_copyleft_needs_source: false,
            ..license::ACTIVE
        };
        assert_eq!(
            decide(Some(OPEN), &none, None, loose).unwrap().0.as_deref(),
            Some("GPL-3.0-or-later")
        );
        let downgrade = Statement {
            user_supplied: true,
            ..Statement::default()
        };
        assert_eq!(
            decide(Some(OPEN), &downgrade, Some(true), license::ACTIVE)
                .unwrap()
                .0,
            None
        );
        // An offered permissive alternative needs no source unless strict.
        let either = Statement {
            open: true,
            license: Some("MIT OR GPL-2.0-only".into()),
            ..Statement::default()
        };
        assert!(decide(None, &either, None, license::Policy::ACCEPTED).is_ok());
        assert!(decide(None, &either, None, license::Policy::STRICT).is_err());
    }
    fn notes_free(result: Result<(Option<String>, Vec<String>)>) -> bool {
        result.is_ok_and(|(license, notes)| license.is_some() && notes.is_empty())
    }

    /// A package written now names the `ibm-pc` platform, which release
    /// 0.0.3 first reads, so it declares 0.0.3; a stated minimum release
    /// below the computed one is refused, and an edit keeps the edited
    /// package's marker and never lowers it.
    #[test]
    fn a_stated_minimum_release_marks_a_package_and_an_edit_keeps_it() {
        use crate::compat::level;
        use image_store::compat::{AppVersion, Requirement, BASELINE};
        let computed = AppVersion::new(0, 0, 3);
        let root = std::env::temp_dir().join(format!(
            "pc-image-firmware-min-app-version-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let rom = root.join("rom.bin");
        fs::write(&rom, [0x5a; 8192]).unwrap();
        let store = Store::open(root.join("store")).unwrap().with_compatibility(
            crate::compat::compatibility(image_store::compat::EXTRACTION_BASELINE),
        );
        let candidate = unlisted(&rom, "marked-rom", "option-rom", &[]).unwrap();
        let requirement = |tag: &str| {
            let image = store
                .resolve(tag, &Validator(&PcFirmwareRules(&[])))
                .unwrap();
            Requirement::of(&image.manifest)
        };
        let stated = |version| Statement {
            min_app_version: Some(version),
            ..Statement::default()
        };
        publish_as(
            &store,
            &candidate,
            &[],
            &Statement::default(),
            &[],
            "firmware/plain:r1",
            None,
        )
        .unwrap();
        assert_eq!(
            requirement("firmware/plain:r1"),
            Requirement::Release(computed)
        );
        for lower in [AppVersion::new(0, 0, 0), BASELINE, AppVersion::new(0, 0, 2)] {
            let refusal = publish_as(
                &store,
                &candidate,
                &[],
                &stated(lower),
                &[],
                "firmware/lower:r1",
                None,
            )
            .unwrap_err()
            .to_string();
            assert!(
                refusal.contains(&lower.to_string()) && refusal.contains(&computed.to_string()),
                "{refusal}"
            );
        }
        publish_as(
            &store,
            &candidate,
            &[],
            &stated(level()),
            &[],
            "firmware/raised:r1",
            None,
        )
        .unwrap();
        assert_eq!(
            requirement("firmware/raised:r1"),
            Requirement::Release(level())
        );
        let renamed = Statement {
            title: Some("Renamed ROM".into()),
            ..Statement::default()
        };
        edit(
            &store,
            "firmware/raised:r1",
            "firmware/raised:r2",
            &renamed,
            &[],
        )
        .unwrap();
        assert_eq!(
            requirement("firmware/raised:r2"),
            Requirement::Release(level())
        );
        // An edit may raise the value, never lower it.
        let refusal = edit(
            &store,
            "firmware/raised:r2",
            "firmware/raised:r3",
            &stated(BASELINE),
            &[],
        )
        .unwrap_err()
        .to_string();
        assert!(refusal.contains("only raises"), "{refusal}");
        // The package already declares what it computes, so stating it
        // again changes nothing.
        let refusal = edit(
            &store,
            "firmware/plain:r1",
            "firmware/plain:r2",
            &stated(computed),
            &[],
        )
        .unwrap_err()
        .to_string();
        assert!(refusal.contains("changes nothing"), "{refusal}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_next_revision_counts_up_from_revision_tags_and_old_numbers() {
        for (tag, next) in [
            (
                "private/firmware/ibm-pc/pkg:r1",
                Some("private/firmware/ibm-pc/pkg:r2"),
            ),
            ("pkg:r9", Some("pkg:r10")),
            ("pkg:2x32k-r1", Some("pkg:2x32k-r2")),
            // A package an earlier release imported as `:1`.
            ("firmware/pkg:1", Some("firmware/pkg:r2")),
            ("pkg:9", Some("pkg:r10")),
            ("pkg:2x32k", None),
            ("pkg:latest", None),
            ("pkg:01", None),
            ("pkg", None),
        ] {
            assert_eq!(next_revision(tag).as_deref(), next, "{tag}");
        }
    }

    #[test]
    fn a_local_import_is_named_under_private_open_or_not() {
        let root =
            std::env::temp_dir().join(format!("pc-image-package-tag-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let rom = root.join("rom.bin");
        fs::write(&rom, [0x5a; 8192]).unwrap();
        let candidate = unlisted(&rom, "synthetic-rom", "option-rom", &[]).unwrap();
        let tag = "private/firmware/ibm-pc/synthetic-rom:r1";
        assert_eq!(candidate.tag(), tag);
        let open = Statement {
            open: true,
            license: Some("MIT".into()),
            ..Statement::default()
        };
        // Open or not, a local import never takes the official name.
        let opened = Store::open(root.join("opened"))
            .unwrap()
            .with_compatibility(crate::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
        let open_package = publish_with(&opened, &candidate, &[], &open, &[]).unwrap();
        assert_eq!(
            opened.tag_descriptor(tag).unwrap().digest,
            open_package.digest
        );
        // The official revision the store already holds is the one an
        // import checks: the same ROMs there are already imported.
        let official = Store::open(root.join("official"))
            .unwrap()
            .with_compatibility(crate::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
        let unemu = "unemu/firmware/ibm-pc/synthetic-rom:r1";
        let pulled = publish_as(&official, &candidate, &[], &open, &[], unemu, None).unwrap();
        assert_eq!(import_tag(&official, "synthetic-rom").unwrap(), unemu);
        assert_eq!(publish(&official, &candidate, &[], &[]).unwrap(), pulled);
        assert!(official.tag_descriptor(tag).is_err());
        // A package's revision tag never moves, not even from the confirmed
        // digest, and no package is tagged latest.
        let store = Store::open(root.join("store")).unwrap().with_compatibility(
            crate::compat::compatibility(image_store::compat::EXTRACTION_BASELINE),
        );
        let first = publish(&store, &candidate, &[], &[]).unwrap();
        assert_eq!(store.tag_descriptor(tag).unwrap().digest, first.digest);
        let validator = Validator(&PcFirmwareRules(&[]));
        let other = publish_as(
            &store,
            &candidate,
            &[],
            &open,
            &[],
            "private/firmware/ibm-pc/other:r1",
            None,
        )
        .unwrap();
        let error = store
            .update_tag(tag, &other, Some(&first.digest), &validator)
            .unwrap_err()
            .to_string();
        assert!(error.contains("never move"), "{error}");
        let error = store
            .update_tag(
                "private/firmware/ibm-pc/synthetic-rom:latest",
                &first,
                None,
                &validator,
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("no latest"), "{error}");
        assert_eq!(store.tag_descriptor(tag).unwrap().digest, first.digest);
        fs::remove_dir_all(&root).unwrap();
    }
}
