// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Installation media collection's model; machine-images §§5.2 and 7.
//! Rows, facets, the disk list and extras come from cached set summaries, so
//! listing and selection never read a medium. Import installation media…
//! lists, checks and publishes on the library worker through
//! `pc_image::media_import`, the code path of `image media import`, with its
//! refusals and an idempotent re-import; the desktop draws it.
//!
//! Floppy formats are the PC provider's (IBM Personal Computer AT Technical
//! Reference, Diskette Drive Adapter): a set is offered to a drive only when
//! the drive type takes every disk, by the shared rule
//! (`FloppyGeometry::accepted_media`), so a 1.44M drive takes a set mixing
//! 1.44M and 720K disks.

use crate::firmware::Redistribution;
use crate::snapshot::{Filter, Query};
use assembly::FloppyGeometry;
use image_store::{
    media_set::Medium,
    media_type,
    summary::{ImageSummary, MediaSetSummary, SummaryCache},
    Descriptor, Store,
};
use library_model::listing::{source, ImageRow, Listed};
use pc_image::media_import;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

/// Tree facet groups of the Installation media collection.
pub const FACETS: [&str; 5] = ["Format", "Labels", "Genre", "Platform", "Year"];
pub fn facet_label(kind: &str, value: &str) -> String {
    match kind {
        "Format" => format_label(value),
        _ => value.into(),
    }
}

pub const OPTICAL: &str = "iso-2048";
pub const UNKNOWN: &str = "unknown format";

fn format_label(name: &str) -> String {
    match name {
        OPTICAL => "Optical · ISO-2048".into(),
        UNKNOWN => "Unknown format".into(),
        name => format!("Floppy · {name}"),
    }
}
fn short_format(name: &str) -> &str {
    if name == OPTICAL {
        "ISO-2048"
    } else {
        name
    }
}
/// The provider's name of a medium's format; other platforms stay unnamed.
pub fn format_name(summary: &ImageSummary, medium: &Medium) -> String {
    if !pc_image::is_pc(&summary.platform) {
        return UNKNOWN.into();
    }
    pc_image::media_set::format_name(&medium.format).map_or_else(|_| UNKNOWN.into(), str::to_owned)
}
fn formats(summary: &ImageSummary) -> Vec<String> {
    summary
        .media_set
        .iter()
        .flat_map(|set| set.media.iter().map(|m| format_name(summary, m)))
        .collect()
}
/// Disk counts per format, in first-use order: "4 x 1.44M".
pub fn disks_label(formats: &[String]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for name in formats {
        match counts.iter_mut().find(|(n, _)| n == name) {
            Some((_, count)) => *count += 1,
            None => counts.push((name, 1)),
        }
    }
    counts
        .iter()
        .map(|(name, count)| format!("{count} x {}", short_format(name)))
        .collect::<Vec<_>>()
        .join(" + ")
}
fn is_set_payload(descriptor: &Descriptor) -> bool {
    matches!(
        descriptor.decoded_media_type(),
        media_type::FLOPPY | media_type::OPTICAL | media_type::EXTRA
    )
}

/// One installation set in the Installation media collection, with every tag
/// of its repository that names it.
#[derive(Clone, Debug)]
pub struct MediaRow {
    pub image: ImageRow,
    pub redistribution: Redistribution,
}
impl MediaRow {
    pub fn set(&self) -> Option<&MediaSetSummary> {
        self.image.summary.as_ref()?.media_set.as_ref()
    }
    pub fn same(&self, other: &Self) -> bool {
        self.image.same(&other.image)
    }
}

/// Whether a set can go into a drive of the running machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Every disk fits this floppy unit of the running machine.
    Ready(u8),
    /// No machine runs; the set needs a drive of this format.
    NoMachine(String),
    Unavailable(String),
}
impl Availability {
    fn row_text(&self) -> String {
        match self {
            Self::Ready(unit) => format!("Ready to insert into {}:", drive_letter(*unit)),
            Self::NoMachine(format) => format!("Fits a {format} drive"),
            Self::Unavailable(reason) => reason.clone(),
        }
    }
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Ready(_) => None,
            Self::NoMachine(format) => Some(format!(
                "No machine is running. Insert into… puts this set into a {format} drive of a \
                 machine for its next Start."
            )),
            Self::Unavailable(reason) => Some(reason.clone()),
        }
    }
}
fn drive_letter(unit: u8) -> &'static str {
    if unit == 0 {
        "A"
    } else {
        "B"
    }
}
/// Drive-independent refusals first (platform, optical, unknown formats), then
/// the drive check the Insert dialog applies, unit by unit: the set is Ready
/// on the first drive whose type takes every disk.
pub fn availability(summary: &ImageSummary, drives: &[(u8, FloppyGeometry)]) -> Availability {
    let Some(set) = &summary.media_set else {
        return Availability::Unavailable("Reading set summary…".into());
    };
    if !pc_image::is_pc(&summary.platform) {
        return Availability::Unavailable(format!(
            "Platform {} is unavailable for this drive",
            summary.platform
        ));
    }
    let names = formats(summary);
    let Some(first) = names.first() else {
        return Availability::Unavailable("The set lists no media".into());
    };
    if names.iter().any(|n| n == OPTICAL) {
        return Availability::Unavailable("CD-ROM emulation not available yet".into());
    }
    if names.iter().any(|n| n == UNKNOWN) {
        return Availability::Unavailable("Unknown floppy geometry".into());
    }
    let mut reason = None;
    for &(unit, geometry) in drives {
        match pc_media::media_dialog::media_compatibility(
            &summary.platform,
            &set.media,
            geometry,
            unit,
        ) {
            None => return Availability::Ready(unit),
            Some(why) => {
                reason.get_or_insert(why);
            }
        }
    }
    match reason {
        Some(reason) => Availability::Unavailable(reason),
        None => Availability::NoMachine(first.clone()),
    }
}

pub fn add_facets(facets: &mut BTreeMap<String, BTreeSet<String>>, summary: &ImageSummary) {
    facets
        .entry("Format".into())
        .or_default()
        .extend(formats(summary));
    facets
        .entry("Labels".into())
        .or_default()
        .extend(summary.labels.iter().cloned());
    for t in &summary.titles {
        for (kind, values) in [
            ("Genre", t.genres.clone()),
            ("Platform", t.platform.iter().cloned().collect()),
            ("Year", t.year.map(|y| y.to_string()).into_iter().collect()),
        ] {
            facets.entry(kind.into()).or_default().extend(values);
        }
    }
}

/// Set rows for the current search and facet. `matches` is the summary query
/// result for the text and the label, genre, platform and year facets.
pub fn rows(
    cache: &SummaryCache,
    sets: &[&Listed],
    matches: &BTreeSet<String>,
    q: &Query,
) -> Vec<MediaRow> {
    let mut rows = Vec::new();
    for set in sets {
        let summary = cache.summary(&set.tag.manifest.digest);
        let matched = match summary {
            Some(_) => set.matched(matches),
            None => q.filter == Filter::Media && set.named(&q.text),
        };
        let facet_match = match &q.filter {
            Filter::MediaFacet(kind, value) if kind == "Format" => {
                summary.is_some_and(|s| formats(s).contains(value))
            }
            _ => true,
        };
        if !matched || !facet_match {
            continue;
        }
        rows.push(MediaRow {
            redistribution: summary
                .map(|s| Redistribution::of_payloads(s, is_set_payload))
                .unwrap_or_default(),
            image: set.row(cache),
        });
    }
    rows.sort_by_cached_key(|row| sort_value(row, q.media_sort));
    if q.media_descending {
        rows.reverse();
    }
    rows
}

/// Every set row, for the media picker and the drives a set fits.
pub fn all_rows(cache: &SummaryCache, sets: &[&Listed]) -> Vec<MediaRow> {
    sets.iter()
        .map(|set| {
            let summary = cache.summary(&set.tag.manifest.digest);
            MediaRow {
                redistribution: summary
                    .map(|s| Redistribution::of_payloads(s, is_set_payload))
                    .unwrap_or_default(),
                image: set.row(cache),
            }
        })
        .collect()
}

/// A set as the media picker lists it, once its summary is read.
pub fn picker_set(row: &MediaRow) -> Option<pc_media::media_picker::SetEntry> {
    use pc_media::media_picker::{Disk, SetEntry};
    let summary = row.image.summary.as_ref()?;
    let set = summary.media_set.as_ref()?;
    let title = summary.titles.first();
    let mut problem = row.image.failure.clone();
    let disks: Vec<Disk> = set
        .media
        .iter()
        .map(|medium| {
            let format = match pc_image::media_set::geometry(&medium.format) {
                Ok(Some(format)) => Some(format),
                Ok(None) => {
                    problem.get_or_insert("CD-ROM emulation not available yet".into());
                    None
                }
                Err(error) => {
                    problem.get_or_insert(error.to_string());
                    None
                }
            };
            Disk {
                id: medium.id.clone(),
                label: medium.label.clone(),
                format,
            }
        })
        .collect();
    if !pc_image::is_pc(&summary.platform) {
        problem.get_or_insert(format!("Platform {} is unavailable", summary.platform));
    }
    Some(SetEntry {
        reference: row.image.reference.clone(),
        image: row.image.descriptor.clone(),
        title: row.image.name().to_owned(),
        publisher: title.map(|t| t.publishers.join(", ")).unwrap_or_default(),
        year: title.and_then(|t| t.year),
        genre: title.map(|t| t.genres.join(", ")).unwrap_or_default(),
        platform: title.and_then(|t| t.platform.clone()).unwrap_or_default(),
        labels: summary.labels.clone(),
        disks,
        extras: set.extras.len(),
        open: row.redistribution.open,
        problem,
    })
}

/// "Example Adventure (4 x 1.44M)", as rows and the details pane name a set.
pub fn set_name(row: &MediaRow) -> String {
    match row.image.summary.as_ref().filter(|_| row.set().is_some()) {
        Some(summary) => format!("{} ({})", row.image.name(), disks_label(&formats(summary))),
        None => row.image.name().into(),
    }
}

/// A set's cells; a set without its summary says why in the availability
/// column.
pub fn cells(row: &MediaRow, drives: &[(u8, FloppyGeometry)]) -> [String; 6] {
    let source = source(&row.image.descriptor).text();
    let (Some(summary), Some(_)) = (row.image.summary.as_ref(), row.set()) else {
        let mut cells: [String; 6] = Default::default();
        cells[0] = row.image.name().to_owned();
        cells[1] = source;
        cells[5] = if row.image.failure.is_some() {
            "Unavailable"
        } else {
            "Pending"
        }
        .into();
        return cells;
    };
    let title = summary.titles.first();
    [
        set_name(row),
        source,
        row.image.tags(),
        title.and_then(|t| t.platform.clone()).unwrap_or_default(),
        disks_label(&formats(summary)),
        availability(summary, drives).row_text(),
    ]
}
/// The worker sorts without the window's running drives, so availability
/// sorts by its stopped-machine text.
fn sort_value(row: &MediaRow, column: usize) -> String {
    let set = row.set();
    match column {
        // Counts sort as numbers, then by the format text for equal counts.
        4 => format!(
            "{:020} {}",
            set.map_or(0, |s| s.media.len()),
            cells(row, &[])[4].to_lowercase()
        ),
        column => cells(row, &[])
            .get(column)
            .cloned()
            .unwrap_or_default()
            .to_lowercase(),
    }
}

/// Cancel and progress shared between the dialog and the worker's import.
#[derive(Clone, Debug, Default)]
pub struct Control(Arc<ControlState>);
#[derive(Debug, Default)]
struct ControlState {
    cancelled: AtomicBool,
    done: AtomicUsize,
    total: AtomicUsize,
}
impl Control {
    /// Stop before the next file; files already copied stay unreferenced.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }
    /// Files done and the file total of the running import.
    pub fn progress(&self) -> (usize, usize) {
        (
            self.0.done.load(Ordering::Relaxed),
            self.0.total.load(Ordering::Relaxed),
        )
    }
    fn report(&self, done: usize, total: usize) {
        self.0.total.store(total, Ordering::Relaxed);
        self.0.done.store(done, Ordering::Relaxed);
    }
}

/// What a picker returned: media files in their order, a media folder, or extras.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Files,
    Folder,
    Extras,
}
/// Picked files with their exact sizes; source paths stay local hints.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub files: Vec<(PathBuf, u64)>,
    /// Hidden files a folder listing skipped, as the command line reports them.
    pub notes: Vec<String>,
}
/// Worker side of a pick: a folder lists by the command line's rule, files
/// keep the order they were picked in, and every source must be a regular file.
pub fn inspect(pick: Pick, paths: &[PathBuf]) -> Result<Sources, String> {
    let mut sources = Sources::default();
    let paths = match (pick, paths) {
        (Pick::Folder, [folder]) => {
            let listing = media_import::list_folder(folder);
            sources.notes = listing
                .hidden
                .iter()
                .map(|name| format!("skipped hidden file: {name:?}"))
                .collect();
            let files = listing.files.map_err(|e| e.to_string())?;
            if files.is_empty() {
                return Err(format!("source list {paths:?}: no media files"));
            }
            files
        }
        _ => paths.to_vec(),
    };
    for path in paths {
        let size = if pick == Pick::Extras {
            media_import::extra_size(&path)
        } else {
            media_import::source_size(&path)
        }
        .map_err(|e| e.to_string())?;
        sources.files.push((path, size));
    }
    Ok(sources)
}

/// The result of one import, as the dialog and status line report it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Imported(String),
    AlreadyImported(String),
}
impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Imported(tag) => write!(f, "Imported {tag}"),
            Self::AlreadyImported(tag) => write!(f, "Already imported {tag}"),
        }
    }
}
/// Worker side of Import to private library: the plan and publication of
/// `image media import`, stopping between files when the dialog cancels.
pub fn import(
    store: &Store,
    request: &media_import::Request,
    control: &Control,
) -> Result<Outcome, String> {
    let plan = media_import::plan(request).map_err(|e| e.to_string())?;
    let tag = plan.tag().to_owned();
    match media_import::publish(
        store,
        &plan,
        &|done, total| control.report(done, total),
        &|| control.is_cancelled(),
    ) {
        Ok(media_import::Outcome::Imported(_)) => Ok(Outcome::Imported(tag)),
        Ok(media_import::Outcome::AlreadyImported(_)) => Ok(Outcome::AlreadyImported(tag)),
        Err(error) => Err(error.to_string()),
    }
}

/// What the desktop's tests of the Installation media collection compare
/// with.
#[cfg(feature = "test-support")]
pub mod test_support {
    use image_store::{summary::ImageSummary, Descriptor};

    /// The format names of a set's media, in order.
    pub fn formats(summary: &ImageSummary) -> Vec<String> {
        super::formats(summary)
    }

    /// Whether a layer is a medium or an extra of a set.
    pub fn is_set_payload(descriptor: &Descriptor) -> bool {
        super::is_set_payload(descriptor)
    }
}
