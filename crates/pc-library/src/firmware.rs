// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Firmware collection's model; machine-images §§5.3 and 7. Rows,
//! facets, labels and "Used by" come from cached package summaries, so
//! listing and selection never read ROM bytes. ROM-folder scans and imports
//! run on the library worker with the matching, refusals and idempotence of
//! `image firmware import`; the desktop draws them.
//!
//! Split BIOS sets follow the IBM Personal Computer AT Technical Reference,
//! Section 1, System Board ROM: an even and an odd chip interleave into one
//! logical ROM, so a chip's offset counts assembled bytes.

use crate::firmware_metadata::{Catalogue, Facts, Fields, Verdict};
use crate::snapshot::{Filter, Query};
use image_store::{
    license, media_type,
    summary::{FirmwareSummary, ImageSummary, SummaryCache},
    Descriptor, Digest, Store,
};
use library_model::listing::{source_cell, ImageRow, Listed};
use machine_file::firmware_discovery::DiscoveryControl;
use pc_image::firmware_import::{self, Candidate, Statement};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;

/// A ROM layer, stored raw or as a `+zstd` frame (machine-images §2).
pub fn is_rom(descriptor: &Descriptor) -> bool {
    descriptor.decoded_media_type() == media_type::FIRMWARE
}

/// Tree facet groups of the Firmware collection.
pub const FACETS: [&str; 4] = ["Role", "Board", "Machine class", "Redistribution"];
pub fn is_facet(kind: &str) -> bool {
    FACETS.contains(&kind)
}
pub fn facet_label(kind: &str, value: &str) -> String {
    match kind {
        "Role" => role_label(value).into(),
        "Machine class" => value.to_uppercase(),
        "Redistribution" => redistribution_label(value == OPEN).into(),
        _ => value.into(),
    }
}
pub fn role_label(role: &str) -> &str {
    match role {
        "system-bios" => "System BIOS",
        "option-rom" => "Option ROM",
        "video-bios" => "Video BIOS",
        "keyboard-controller" => "Keyboard controller",
        "other" => "Other",
        _ => role,
    }
}

const OPEN: &str = "open";
const USER_SUPPLIED: &str = "user-supplied";
fn redistribution_label(open: bool) -> &'static str {
    if open {
        "Open"
    } else {
        "User-supplied"
    }
}

/// The raw identity that configs and ROM pins use, whether stored raw or compressed.
pub fn decoded_identity(descriptor: &Descriptor) -> Option<Digest> {
    descriptor.decoded_digest().ok()
}
pub fn pin(sha256: &str) -> Option<Digest> {
    format!("sha256:{sha256}").parse().ok()
}

/// Package redistribution class from the ROM descriptors' annotations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Redistribution {
    pub open: bool,
    pub licenses: Vec<String>,
}
impl Redistribution {
    /// The most restrictive ROM wins: one user-supplied chip keeps the whole
    /// package private, and a missing annotation means user-supplied.
    pub fn of(summary: &ImageSummary) -> Self {
        Self::of_payloads(summary, is_rom)
    }
    /// The same rule over the payload layers `payload` selects. As for the
    /// push guard, an open mark counts only with a valid SPDX licence.
    pub fn of_payloads(summary: &ImageSummary, payload: fn(&Descriptor) -> bool) -> Self {
        let licenses: Vec<_> = summary
            .storage_blobs
            .iter()
            .filter(|d| payload(d))
            .map(image_store::license::open_license)
            .collect();
        let open = !licenses.is_empty() && licenses.iter().all(Option::is_some);
        let mut licenses: Vec<String> = licenses
            .into_iter()
            .flatten()
            .filter(|_| open)
            .map(|license| license.as_str().to_owned())
            .collect();
        licenses.sort();
        licenses.dedup();
        Self { open, licenses }
    }
    pub fn value(&self) -> &'static str {
        if self.open {
            OPEN
        } else {
            USER_SUPPLIED
        }
    }
    pub fn label(&self) -> String {
        if self.open && !self.licenses.is_empty() {
            format!("Open · {}", self.licenses.join(", "))
        } else {
            redistribution_label(self.open).into()
        }
    }
}

/// One firmware package in the Firmware collection, with every tag of its
/// repository that names it.
#[derive(Clone, Debug)]
pub struct FirmwareRow {
    pub image: ImageRow,
    pub redistribution: Redistribution,
    /// Local machine tags whose own closure holds every blob of one of these ROMs.
    pub used_by: Vec<String>,
    /// An open copyleft ROM without its source archive layer: the push guard
    /// keeps the package in private registries (`license::source_missing`).
    pub source_missing: bool,
}
/// Push-guard rule 4 from the summary's own layers.
fn source_missing(summary: &ImageSummary) -> bool {
    summary
        .storage_blobs
        .iter()
        .filter(|d| is_rom(d))
        .any(|d| license::source_missing(d, &summary.storage_blobs))
}
pub const NO_SOURCE_BADGE: &str = "⚠ No source · private only";

impl FirmwareRow {
    pub fn firmware(&self) -> Option<&FirmwareSummary> {
        self.image.summary.as_ref()?.firmware.as_ref()
    }
    pub fn same(&self, other: &Self) -> bool {
        self.image.same(&other.image)
    }
}

/// Raw firmware identities of local machine images, mapped to their tags.
#[derive(Default)]
pub struct MachineRoms(BTreeMap<Digest, BTreeSet<String>>);
impl MachineRoms {
    pub fn add(&mut self, reference: &str, summary: &ImageSummary) {
        for identity in summary
            .storage_blobs
            .iter()
            .filter(|d| is_rom(d))
            .filter_map(decoded_identity)
        {
            self.0.entry(identity).or_default().insert(reference.into());
        }
    }
    /// A machine uses a logical ROM when it embeds every chip, or the already
    /// assembled image. Both sides compare raw identities (the ROM and chip
    /// pins), so compressed storage on either side does not hide a match.
    pub fn users(&self, firmware: &FirmwareSummary) -> Vec<String> {
        let mut users = BTreeSet::new();
        for rom in &firmware.roms {
            if let Some(found) = pin(&rom.sha256).and_then(|d| self.0.get(&d)) {
                users.extend(found.iter().cloned());
            }
            let mut common: Option<BTreeSet<String>> = None;
            for part in &rom.parts {
                let found = pin(&part.sha256)
                    .and_then(|d| self.0.get(&d).cloned())
                    .unwrap_or_default();
                common = Some(match common {
                    None => found,
                    Some(previous) => previous.intersection(&found).cloned().collect(),
                });
            }
            users.extend(common.unwrap_or_default());
        }
        users.into_iter().collect()
    }
}

pub fn add_facets(facets: &mut BTreeMap<String, BTreeSet<String>>, summary: &ImageSummary) {
    let Some(f) = &summary.firmware else {
        return;
    };
    for (kind, values) in [
        ("Role", &f.roles),
        ("Board", &f.boards),
        ("Machine class", &f.machine_classes),
    ] {
        facets
            .entry(kind.into())
            .or_default()
            .extend(values.iter().cloned());
    }
    facets
        .entry("Redistribution".into())
        .or_default()
        .insert(Redistribution::of(summary).value().into());
}

/// Firmware rows for the current search and facet. `matches` is the summary
/// query result for the text and the role/board/class facets.
pub fn rows(
    cache: &SummaryCache,
    packages: &[&Listed],
    matches: &BTreeSet<String>,
    q: &Query,
    machines: &MachineRoms,
) -> Vec<FirmwareRow> {
    let mut rows = Vec::new();
    for package in packages {
        let summary = cache.summary(&package.tag.manifest.digest);
        let matched = match summary {
            Some(_) => package.matched(matches),
            None => q.filter == Filter::Firmware && package.named(&q.text),
        };
        let redistribution = summary.map(Redistribution::of).unwrap_or_default();
        let facet_match = match &q.filter {
            Filter::Facet(kind, value) if kind == "Redistribution" => {
                redistribution.value() == value
            }
            _ => true,
        };
        if !matched || !facet_match {
            continue;
        }
        rows.push(FirmwareRow {
            used_by: summary
                .and_then(|s| s.firmware.as_ref())
                .map(|f| machines.users(f))
                .unwrap_or_default(),
            source_missing: summary.is_some_and(source_missing),
            redistribution,
            image: package.row(cache),
        });
    }
    rows.sort_by_cached_key(|row| sort_value(row, q.firmware_sort));
    if q.firmware_descending {
        rows.reverse();
    }
    rows
}

/// A package's catalogue status, as the firmware-settings mock-up's Status
/// column says it: a superseded release names the release that replaces
/// it; a package of several ROMs is Tested only when every ROM is.
fn status(f: &FirmwareSummary) -> String {
    let entries: Vec<&assembly::FirmwareEntry> = f
        .catalogue_ids
        .iter()
        .filter_map(|id| crate::firmware_choices::entry(id))
        .collect();
    if let Some((by, _)) = entries
        .iter()
        .find_map(|e| crate::firmware_choices::replacement(e))
    {
        return format!("Superseded by {}", crate::firmware_choices::release(by));
    }
    if entries.is_empty() {
        "Not catalogued".into()
    } else if entries
        .iter()
        .all(|e| e.status == assembly::FirmwareStatus::Tested)
    {
        "Tested".into()
    } else {
        "Listed".into()
    }
}

pub fn roles(f: &FirmwareSummary) -> String {
    f.roles
        .iter()
        .map(|r| role_label(r))
        .collect::<Vec<_>>()
        .join(", ")
}
pub fn version_date(f: &FirmwareSummary) -> String {
    format!(
        "{} / {}",
        f.version.as_deref().unwrap_or("Version unknown"),
        f.date.as_deref().unwrap_or("date unknown")
    )
}
fn fits(f: &FirmwareSummary) -> String {
    let classes: Vec<_> = f.machine_classes.iter().map(|c| c.to_uppercase()).collect();
    let parts: Vec<_> = [f.boards.join(", "), classes.join(", ")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        "Unspecified".into()
    } else {
        parts.join(" · ")
    }
}

/// A package's cells; a package without its summary says why in the role
/// column.
pub fn cells(row: &FirmwareRow) -> [String; 7] {
    let name = row.image.name().to_owned();
    let source = source_cell(&row.image.descriptor);
    // A package this release does not read says so in its Status (§5.6).
    let needs = row.image.summary.as_ref().and_then(|s| s.needs_newer());
    let Some(f) = row.firmware() else {
        if let Some(reason) = needs {
            let mut cells: [String; 7] = Default::default();
            cells[0] = name;
            cells[1] = source;
            cells[3] = reason;
            return cells;
        }
        let mut cells: [String; 7] = Default::default();
        cells[0] = name;
        cells[1] = source;
        cells[2] = if row.image.failure.is_some() {
            "Unavailable"
        } else {
            "Pending"
        }
        .into();
        return cells;
    };
    [
        name,
        source,
        roles(f),
        needs.unwrap_or_else(|| status(f)),
        fits(f),
        version_date(f),
        if row.source_missing {
            format!("{} {NO_SOURCE_BADGE}", row.redistribution.label())
        } else if !f.sources.is_empty() {
            format!("{} · with source archive", row.redistribution.label())
        } else {
            row.redistribution.label()
        },
    ]
}
fn sort_value(row: &FirmwareRow, column: usize) -> String {
    cells(row)
        .get(column)
        .cloned()
        .unwrap_or_default()
        .to_lowercase()
}

/// A ROM-folder scan as the review dialog shows it; source paths stay local.
#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    pub candidates: Vec<CandidateView>,
    /// Catalogue sets that cannot become packages, with the scanner's reason.
    pub refused: Vec<(String, String)>,
    /// Unreadable inputs and scan limits, by folder-relative name.
    pub issues: Vec<(String, String)>,
    pub unmatched: Option<String>,
    pub cancelled: bool,
}
#[derive(Clone, Debug)]
pub struct CandidateView {
    pub catalogue_id: String,
    pub tag: String,
    pub name: String,
    pub chips: Vec<String>,
    /// Logical ROM identity; candidates sharing one need an explicit choice.
    pub identity: (String, u64),
    pub redistribution: String,
    pub state: CandidateState,
    /// The catalogue's verdict and the prefilled fields the details editor
    /// starts from.
    pub facts: Facts,
    pub fields: Fields,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateState {
    New,
    AlreadyImported,
    /// Another catalogue entry has the same ROM identity.
    Ambiguous,
    Refused(String),
}
impl CandidateView {
    pub fn result(&self) -> String {
        let present = self.chips.len();
        let found = if present > 1 {
            format!("{present} of {present} chips present")
        } else {
            "Complete image present".into()
        };
        match &self.state {
            CandidateState::New => found,
            CandidateState::AlreadyImported => "Already imported".into(),
            CandidateState::Ambiguous => {
                format!("{found}; ambiguous catalogue identities, pick one")
            }
            CandidateState::Refused(reason) => reason.clone(),
        }
    }
    pub fn importable(&self) -> bool {
        matches!(self.state, CandidateState::New | CandidateState::Ambiguous)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Imported(String),
    AlreadyImported(String),
    Failed(String, String),
}
impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Imported(tag) => write!(f, "Imported {tag}"),
            Self::AlreadyImported(tag) => write!(f, "Already imported {tag}"),
            Self::Failed(tag, reason) => write!(f, "{tag}: {reason}"),
        }
    }
}

/// Whether the store holds the candidate's ROMs under the tag an import
/// writes, and that tag.
fn existing(
    store: &Store,
    candidate: &Candidate,
    catalogue: &[assembly::FirmwareEntry],
) -> pc_image::Result<(bool, String)> {
    let manifest = firmware_import::preview_manifest(candidate, &[])?;
    let tag = firmware_import::import_tag(store, &candidate.catalogue_id)?;
    Ok((
        firmware_import::already_imported(store, &tag, &manifest, catalogue)?,
        tag,
    ))
}

/// Worker side of the scan: match the folder against the catalogue without
/// touching the store, then check each complete set against existing tags.
pub fn scan_folder(
    store: &Store,
    folder: &Path,
    index: Option<&Path>,
    control: &DiscoveryControl,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<(ScanReport, Vec<Candidate>)> {
    let scan =
        machine_file::firmware_discovery::with_discovery_session(index, false, control, || {
            firmware_import::scan(&[folder.to_owned()], catalogue)
        })
        .map_err(|e| e.to_string())?;
    if control.is_cancelled() {
        return Ok((
            ScanReport {
                cancelled: true,
                ..Default::default()
            },
            Vec::new(),
        ));
    }
    let mut report = ScanReport::default();
    let unmatched = scan.size_mismatches + scan.hash_mismatches;
    if unmatched > 0 {
        report.unmatched = Some(format!(
            "unmatched: {unmatched} files ({} size mismatches, {} SHA-256 mismatches)",
            scan.size_mismatches, scan.hash_mismatches
        ));
    }
    for issue in scan.issues {
        if catalogue
            .iter()
            .any(|e| issue.source.as_path() == Path::new(e.id))
        {
            report
                .refused
                .push((issue.source.display().to_string(), issue.reason));
        } else {
            let source = issue
                .source
                .strip_prefix(folder)
                .unwrap_or(&issue.source)
                .display()
                .to_string();
            report.issues.push((source, issue.reason));
        }
    }
    for candidate in &scan.candidates {
        let rom = &candidate.config.roms[0];
        let ambiguous = catalogue.iter().any(|entry| {
            entry.id != candidate.catalogue_id
                && entry.sha256 == rom.sha256
                && entry.size == rom.size
        });
        let catalogue_kind = Catalogue::of(&candidate.config, catalogue);
        let existing = existing(store, candidate, catalogue);
        let tag = existing
            .as_ref()
            .map(|(_, tag)| tag.clone())
            .unwrap_or_default();
        let state = match (existing, &catalogue_kind) {
            (Err(error), _) => CandidateState::Refused(error.to_string()),
            (_, Err(error)) => CandidateState::Refused(error.clone()),
            (Ok((true, _)), _) => CandidateState::AlreadyImported,
            (Ok((false, _)), _) if ambiguous => CandidateState::Ambiguous,
            (Ok((false, _)), _) => CandidateState::New,
        };
        let name = |path: &PathBuf| {
            path.strip_prefix(folder)
                .unwrap_or(path)
                .display()
                .to_string()
        };
        let chips = if rom.parts.is_empty() {
            candidate
                .sources
                .iter()
                .map(|source| format!("{} → single image", name(source)))
                .collect()
        } else {
            rom.parts
                .iter()
                .zip(&candidate.sources)
                .map(|(part, source)| {
                    format!(
                        "{} → {} {} @ {}",
                        name(source),
                        part.position.as_deref().unwrap_or("chip"),
                        part.lane,
                        part.offset
                    )
                })
                .collect()
        };
        let fit = pc_image::firmware::Fit::decode(&candidate.config.fits).unwrap_or_default();
        let facts = Facts {
            catalogue: catalogue_kind.unwrap_or(Catalogue::Unlisted),
            catalogue_fit: Some((fit.boards, fit.machine_classes)),
            stored_source: None,
            single_rom: candidate.config.roms.len() == 1,
        };
        report.candidates.push(CandidateView {
            catalogue_id: candidate.catalogue_id.clone(),
            tag,
            name: candidate.config.name.clone(),
            chips,
            identity: (rom.sha256.clone(), rom.size),
            redistribution: match candidate.distribution {
                assembly::FirmwareDistribution::Open { license, .. } => format!("Open · {license}"),
                assembly::FirmwareDistribution::UserSupplied => "User-supplied".into(),
            },
            state,
            fields: Fields::prefill(&candidate.config, &facts),
            facts,
        });
    }
    Ok((report, scan.candidates))
}

/// Worker side of a set's preview: what Import selected would publish for
/// the set with this statement, or the library's reason for refusing it.
pub fn preview_set(
    candidates: &[Candidate],
    id: &str,
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Verdict> {
    let candidate = candidates
        .iter()
        .find(|c| c.catalogue_id == id)
        .ok_or("not in the last scan; scan the folder again")?;
    firmware_import::preview_manifest_with(candidate, &[], statement, catalogue)
        .map(|preview| Verdict::of(&preview))
        .map_err(|e| e.to_string())
}

/// Worker side of Import selected: verify, copy and publish each chosen set
/// with its statement, reporting an identical existing package as already
/// imported. A failure refuses only its own set, unlike the command line,
/// which stops the run.
pub fn import_selected(
    store: &Store,
    candidates: &[Candidate],
    selected: &[(String, Statement)],
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Vec<Outcome>> {
    if candidates.is_empty() {
        return Err("no reviewed scan to import from; scan the folder again".into());
    }
    Ok(selected
        .iter()
        .map(|(id, statement)| {
            let Some(candidate) = candidates.iter().find(|c| &c.catalogue_id == id) else {
                return Outcome::Failed(
                    id.clone(),
                    "not in the last scan; scan the folder again".into(),
                );
            };
            let tag = match firmware_import::import_tag(store, &candidate.catalogue_id) {
                Ok(tag) => tag,
                Err(error) => return Outcome::Failed(id.clone(), error.to_string()),
            };
            match existing(store, candidate, catalogue) {
                Ok((true, _)) => Outcome::AlreadyImported(tag),
                Ok((false, _)) => {
                    match firmware_import::publish_with(store, candidate, &[], statement, catalogue)
                    {
                        Ok(_) => Outcome::Imported(tag),
                        Err(error) => Outcome::Failed(tag, error.to_string()),
                    }
                }
                Err(error) => Outcome::Failed(tag, error.to_string()),
            }
        })
        .collect())
}

/// What the desktop's tests of the Firmware collection compare with.
#[cfg(feature = "test-support")]
pub mod test_support {
    /// The Redistribution facet's value of a user-supplied package.
    pub const USER_SUPPLIED: &str = super::USER_SUPPLIED;
}
