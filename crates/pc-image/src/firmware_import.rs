// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only ROM-folder matching, tag-last package import and metadata
//! revisions; machine-images §5.3. Source names are local diagnostics only;
//! exact hashes identify each chip. Every layer carries an SPDX licence next
//! to its redistribution mark; the generated README and the config are open
//! only when every other layer is.

use crate::firmware::{self, Fit, PcFirmwareRules};
use crate::{invalid, Result};
use image_store::{
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
impl Candidate {
    pub fn tag(&self) -> String {
        format!("firmware/{}:1", self.catalogue_id)
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
            let hash = assembly::firmware_discovery::hash_firmware_file(&path)
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
            let fit = Fit {
                boards: sorted(entry.boards.iter().copied()),
                machine_classes: if entry.boards.contains(&"ibm-5170") {
                    vec!["at".into()]
                } else {
                    Vec::new()
                },
            };
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
                platform: "ibm-pc-at".into(),
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
fn file_name(path: &Path) -> Result<String> {
    Ok(path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("file name must be UTF-8"))?
        .to_owned())
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
            platform: "ibm-pc-at".into(),
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
    if let Some(catalogue) = catalogue {
        let locked = config.roms.iter().any(|rom| {
            firmware::proprietary(catalogue, &rom.sha256, rom.size).is_some()
                || rom
                    .parts
                    .iter()
                    .any(|p| firmware::proprietary(catalogue, &p.sha256, p.size).is_some())
        });
        if locked {
            return Ok(Some(assembly::FirmwareDistribution::UserSupplied));
        }
    }
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

/// Apply the descriptive part of a statement to a config.
fn apply(config: &mut Config, statement: &Statement) -> Result<()> {
    if let Some(title) = &statement.title {
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
            *field = value.clone();
        }
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
    if let Some(provenance) = &statement.provenance {
        config.provenance = Some(provenance.clone());
    }
    Ok(())
}

const TITLE: &str = "org.opencontainers.image.title";
fn titled(descriptor: &mut Descriptor, name: &str) {
    descriptor.annotations.insert(TITLE.into(), name.into());
}

/// Mark every layer, generate the README, encode the config and order the
/// layers by the config's payload traversal.
fn assemble(
    mut config: Config,
    roms: Vec<(Payload, Descriptor)>,
    source: Option<(Archive, Option<Expression>)>,
    extras: Vec<(Payload, Descriptor, Option<Expression>)>,
    rom_license: Option<Expression>,
    notes: Vec<String>,
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
        titled(&mut d, &archive.filename);
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
        titled(&mut d, &extra.filename);
        marks.push(license);
        pool.push((payload, d));
    }
    let all_open = rom_license.is_some()
        && (config.sources.is_empty() || source_license.is_some())
        && marks.iter().all(Option::is_some);
    let text = crate::firmware_readme::generate(
        &config,
        &crate::firmware_readme::Marks {
            roms: rom_license.as_ref(),
            source: source_license.as_ref(),
            extras: &marks,
            all_open,
        },
    );
    let metadata = all_open
        .then(|| Expression::parse(firmware::METADATA_LICENSE))
        .transpose()?;
    let bytes = text.into_bytes();
    let mut readme = Descriptor::new(mt::MARKDOWN, Digest::of(&bytes), bytes.len() as u64);
    license::mark(&mut readme, metadata.as_ref());
    titled(&mut readme, "README.md");
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
    let mut manifest = Manifest::new_firmware(
        &config.platform,
        config_descriptor,
        payloads.iter().map(|(_, d)| d.clone()).collect(),
    )?;
    annotate(&mut manifest, &config, all_open);
    Ok(Plan {
        manifest,
        config: encoded,
        payloads,
        notes,
    })
}

/// Standard OCI annotations where they fit; there is no creation time, because
/// none is supplied and the host clock never enters a package.
fn annotate(manifest: &mut Manifest, config: &Config, all_open: bool) {
    let annotations = &mut manifest.annotations;
    annotations.insert(TITLE.into(), config.name.clone());
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
    Ok(plan(candidate, attachments, &Statement::default(), None)?.manifest)
}
/// What an import with a statement would publish.
#[derive(Debug)]
pub struct Preview {
    pub manifest: Manifest,
    /// What the user should know, such as why open firmware stays private.
    pub notes: Vec<String>,
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
    let plan = plan(candidate, attachments, statement, Some(catalogue))?;
    validate(&plan, catalogue, None)?;
    Ok(Preview {
        manifest: plan.manifest,
        notes: plan.notes,
    })
}

fn plan(
    candidate: &Candidate,
    attachments: &[Attachment],
    statement: &Statement,
    catalogue: Option<&[assembly::FirmwareEntry]>,
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
        roms.push((
            Payload::File(path.clone()),
            payload(path, kind, &digest, size)?,
        ));
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
            assembly::firmware_discovery::hash_firmware_file(&extra.path)?
        )
        .parse()?;
        let extension = extra
            .path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let kind = match extension.as_str() {
            "txt" | "pdf" | "md" | "doc" => ExtraKind::Document,
            "zip" | "7z" | "arj" | "rar" => ExtraKind::Archive,
            "exe" | "com" | "bat" => ExtraKind::Executable,
            _ => ExtraKind::Other,
        };
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
    apply(&mut config, statement)?;
    let known = known(&config, catalogue, Some(candidate.distribution))?;
    let default = match known {
        Some(assembly::FirmwareDistribution::Open { license, .. }) => {
            Some(Expression::parse(license)?)
        }
        _ => None,
    };
    decide(
        config, roms, source, extras, known, default, statement, catalogue,
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
    assemble(config, roms, source, extras, rom_license, notes)
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

/// The package a tag already names for this import, verified: the identical
/// package, or one written before packages carried licences and a README
/// that holds the same ROM and document bytes. Any other package under the
/// tag conflicts before anything is copied.
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
            let older = !image
                .manifest
                .layers
                .iter()
                .any(|d| d.decoded_media_type() == mt::MARKDOWN);
            if existing.digest == digest
                || older && bound_layers(&image.manifest)? == bound_layers(manifest)?
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
/// package's metadata is [`edit`]'s job.
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
        &candidate.tag(),
        None,
    )
}
/// [`publish_with`] under a chosen local tag instead of `firmware/<id>:1`,
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
    let plan = plan(candidate, attachments, statement, Some(catalogue))?;
    validate(&plan, catalogue, None)?;
    unjudged(judged, &Digest::of(&plan.manifest.encode()?))?;
    if let Some(existing) = existing(store, tag, &plan.manifest, catalogue)? {
        // An earlier package with the same ROMs is kept as it is.
        unjudged(judged, &existing.digest)?;
        return Ok(existing);
    }
    store_plan(store, plan, tag, catalogue, judged)
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

/// The tag after `name:N`, for a tag whose version is a plain revision number.
pub fn next_revision(tag: &str) -> Option<String> {
    let (name, revision) = tag.rsplit_once(':')?;
    if revision.is_empty()
        || revision.starts_with('0')
        || !revision.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(format!(
        "{name}:{}",
        revision.parse::<u64>().ok()?.checked_add(1)?
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
    if !statement.extra_licenses.is_empty() {
        return Err(invalid("an edit keeps the package's documents as they are"));
    }
    store.check_new_tag(target)?;
    let image = store.resolve(tag, &Validator(&PcFirmwareRules(catalogue)))?;
    let mut config = Config::decode(&image.config)?;
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
    let mut roms = Vec::new();
    for (digest, _, kind) in config.payloads() {
        if kind == mt::FIRMWARE {
            roms.push((Payload::Stored, layer(&digest)?));
        }
    }
    // The most restrictive ROM mark is the package's current ROM licence.
    let current = roms
        .iter()
        .map(|(_, d)| license::open_license(d))
        .collect::<Option<Vec<_>>>()
        .and_then(|l| l.into_iter().next());
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
    apply(&mut config, statement)?;
    let known = known(&config, Some(catalogue), None)?;
    let plan = decide(
        config,
        roms,
        source,
        extras,
        known,
        current,
        statement,
        Some(catalogue),
    )?;
    validate(&plan, catalogue, Some(store))?;
    if Digest::of(&plan.manifest.encode()?) == image.image.digest {
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
        assert_eq!(license::ACTIVE, license::Policy::ACCEPTED);
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
}
