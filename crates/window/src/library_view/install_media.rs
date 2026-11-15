// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installation media collection of the desktop library; machine-images §§5.2
//! and 7. Rows, facets, the disk list and extras come from cached set
//! summaries, so listing and selection never read a medium. Import
//! installation media… lists, checks and publishes on the library worker
//! through `pc_image::media_import`, the code path of `image media import`,
//! with its refusals and an idempotent re-import.
//!
//! Floppy formats are the PC provider's (IBM Personal Computer AT Technical
//! Reference, Diskette Drive Adapter): a set is offered to a drive only when
//! every disk has that drive's exact geometry. Insertion itself goes through
//! the drive's existing Insert dialog, which verifies every disk again.

use super::{firmware::Redistribution, human_size, Filter, ImageRow, LibraryView, Query};
use assembly::FloppyGeometry;
use image_store::{
    media_set::{valid_text, ExtraKind, MediaFormats, Medium},
    media_type,
    summary::{ImageSummary, MediaSetSummary, SummaryCache},
    Descriptor, Store, Tag,
};
use pc_image::{media_import, media_set::PcMediaFormats};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

/// Tree facet groups of the Installation media collection.
pub(super) const FACETS: [&str; 5] = ["Format", "Labels", "Genre", "Platform", "Year"];
pub(super) fn facet_label(kind: &str, value: &str) -> String {
    match kind {
        "Format" => format_label(value),
        _ => value.into(),
    }
}

const PLATFORM: &str = "ibm-pc-at";
const OPTICAL: &str = "iso-2048";
const UNKNOWN: &str = "unknown format";

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
fn format_name(summary: &ImageSummary, medium: &Medium) -> String {
    if summary.platform != PLATFORM {
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
fn format_detail(summary: &ImageSummary, medium: &Medium) -> String {
    let name = format_name(summary, medium);
    if name == OPTICAL {
        return "Optical · ISO-2048, 2048-byte sectors".into();
    }
    match pc_image::media_set::geometry(&medium.format) {
        Ok(Some(g)) if summary.platform == PLATFORM => format!(
            "Floppy · {name}, {} cylinders / {} heads / {} sectors",
            g.cylinders, g.heads, g.sectors_per_track
        ),
        _ => name,
    }
}
/// Disk counts per format, in first-use order: "4 x 1.44M".
fn disks_label(formats: &[String]) -> String {
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
fn kind_label(kind: ExtraKind) -> &'static str {
    match kind {
        ExtraKind::Document => "Document",
        ExtraKind::Archive => "Archive",
        ExtraKind::Executable => "Executable",
        ExtraKind::Other => "Other",
    }
}

/// One installation set tag in the Installation media collection.
#[derive(Clone, Debug)]
pub(super) struct MediaRow {
    pub image: ImageRow,
    pub redistribution: Redistribution,
}
impl MediaRow {
    fn set(&self) -> Option<&MediaSetSummary> {
        self.image.summary.as_ref()?.media_set.as_ref()
    }
    pub fn same(&self, other: &Self) -> bool {
        self.image.reference == other.image.reference
            && self.image.descriptor.digest == other.image.descriptor.digest
    }
}

/// Whether a set can go into a drive of the running machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Availability {
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
    fn reason(&self) -> Option<String> {
        match self {
            Self::Ready(_) => None,
            Self::NoMachine(format) => Some(format!(
                "No machine is running. Start one with a {format} drive, then Media › Insert › \
                 From library… loads this set into the drive's catalogue in order."
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
/// Drive-independent refusals first (platform, optical, mixed formats), then
/// the exact drive check the Insert dialog applies, unit by unit.
pub(crate) fn availability(
    summary: &ImageSummary,
    drives: &[(u8, FloppyGeometry)],
) -> Availability {
    let Some(set) = &summary.media_set else {
        return Availability::Unavailable("Reading set summary…".into());
    };
    if summary.platform != PLATFORM {
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
    if names.iter().any(|n| n != first) {
        return Availability::Unavailable(
            "Disks of several formats; no single drive takes the whole set".into(),
        );
    }
    let mut reason = None;
    for &(unit, geometry) in drives {
        match crate::media_dialog::media_compatibility(
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

pub(super) fn add_facets(facets: &mut BTreeMap<String, BTreeSet<String>>, summary: &ImageSummary) {
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
pub(super) fn rows(
    cache: &SummaryCache,
    sets: &[&Tag],
    matches: &BTreeSet<String>,
    q: &Query,
) -> Vec<MediaRow> {
    let mut rows = Vec::new();
    for tag in sets {
        let summary = cache.summary(&tag.manifest.digest);
        let matched = match summary {
            Some(_) => matches.contains(&tag.name),
            None => {
                q.filter == Filter::Media
                    && tag.name.to_lowercase().contains(&q.text.to_lowercase())
            }
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
            image: ImageRow {
                reference: tag.name.clone(),
                descriptor: tag.manifest.clone(),
                summary: summary.cloned(),
                failure: cache
                    .failure(&tag.manifest.digest)
                    .map(|f| f.reason.clone()),
            },
        });
    }
    rows.sort_by_cached_key(|row| sort_value(row, q.media_sort));
    if q.media_descending {
        rows.reverse();
    }
    rows
}

pub(super) const COLUMNS: [&str; 9] = [
    "Set",
    "Tag",
    "Platform",
    "Disks",
    "Extras",
    "Size stored / decoded",
    "Redistribution",
    "Labels",
    "Availability",
];
const WIDTHS: [f32; 9] = [240.0, 70.0, 80.0, 110.0, 70.0, 170.0, 120.0, 110.0, 240.0];

/// "Example Adventure (4 x 1.44M)", as rows and the details pane name a set.
pub(super) fn set_name(row: &MediaRow) -> String {
    match row.image.summary.as_ref().filter(|_| row.set().is_some()) {
        Some(summary) => format!("{} ({})", row.image.name(), disks_label(&formats(summary))),
        None => row.image.name().into(),
    }
}
fn extras_label(count: usize) -> String {
    match count {
        0 => "—".into(),
        1 => "1 file".into(),
        n => format!("{n} files"),
    }
}
fn sizes(summary: &ImageSummary) -> String {
    format!(
        "{} / {}",
        human_size(summary.stored_size),
        human_size(summary.decoded_size)
    )
}

pub(super) fn cells(row: &MediaRow, drives: &[(u8, FloppyGeometry)]) -> [String; 9] {
    let failed = if row.image.failure.is_some() {
        "[!] "
    } else {
        ""
    };
    let (Some(summary), Some(set)) = (row.image.summary.as_ref(), row.set()) else {
        let mut cells: [String; 9] = Default::default();
        cells[0] = format!("{failed}{}", row.image.name());
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
        format!("{failed}{}", set_name(row)),
        row.image
            .reference
            .rsplit(':')
            .next()
            .unwrap_or_default()
            .into(),
        title.and_then(|t| t.platform.clone()).unwrap_or_default(),
        disks_label(&formats(summary)),
        extras_label(set.extras.len()),
        sizes(summary),
        row.redistribution.label(),
        summary.labels.join(", "),
        availability(summary, drives).row_text(),
    ]
}
/// The worker sorts without the window's running drives, so availability
/// sorts by its stopped-machine text.
fn sort_value(row: &MediaRow, column: usize) -> String {
    let set = row.set();
    match column {
        // Counts sort as numbers, then by the format text for equal counts.
        3 => format!(
            "{:020} {}",
            set.map_or(0, |s| s.media.len()),
            cells(row, &[])[3].to_lowercase()
        ),
        4 => format!("{:020}", set.map_or(0, |s| s.extras.len())),
        5 => format!(
            "{:020}",
            row.image.summary.as_ref().map_or(0, |s| s.stored_size)
        ),
        column => cells(row, &[])
            .get(column)
            .cloned()
            .unwrap_or_default()
            .to_lowercase(),
    }
}

/// The selected-set pane, drawn from the summary alone. A set has no launch
/// action, so the pane never offers Start. Returns the floppy unit whose Insert
/// was pressed.
pub(super) fn details(
    ui: &mut egui::Ui,
    row: &MediaRow,
    drives: &[(u8, FloppyGeometry)],
) -> Option<u8> {
    let image = &row.image;
    let (Some(summary), Some(set)) = (image.summary.as_ref(), row.set()) else {
        ui.strong(image.name());
        ui.label(&image.reference);
        ui.label(
            image
                .failure
                .clone()
                .unwrap_or_else(|| "Reading set summary…".into()),
        );
        return None;
    };
    let mut insert = None;
    let available = availability(summary, drives);
    let title = summary.titles.first();
    let unknown = || "Unknown".to_owned();
    ui.columns(3, |columns| {
        let facts = &mut columns[0];
        facts.strong(set_name(row));
        facts.label(format!("{} · private library", image.reference));
        egui::Grid::new("media-set-facts")
            .num_columns(2)
            .show(facts, |ui| {
                let digest = image.descriptor.digest.hex();
                let machine = if summary.platform == PLATFORM {
                    "PC AT"
                } else {
                    summary.platform.as_str()
                };
                let redistribution = if row.redistribution.open {
                    row.redistribution.label()
                } else {
                    format!("{} · private store and export", row.redistribution.label())
                };
                for (label, value, hover) in [
                    (
                        "Platform",
                        format!(
                            "{machine} / {}",
                            title
                                .and_then(|t| t.platform.clone())
                                .unwrap_or_else(unknown)
                        ),
                        None,
                    ),
                    (
                        "Publisher",
                        title
                            .map(|t| t.publishers.join(", "))
                            .filter(|p| !p.is_empty())
                            .unwrap_or_else(unknown),
                        None,
                    ),
                    (
                        "Year / genre",
                        format!(
                            "{} / {}",
                            title
                                .and_then(|t| t.year)
                                .map_or_else(unknown, |y| y.to_string()),
                            title
                                .map(|t| t.genres.join(", "))
                                .filter(|g| !g.is_empty())
                                .unwrap_or_else(unknown)
                        ),
                        None,
                    ),
                    ("Redistribution", redistribution, None),
                    ("Stored / decoded", sizes(summary), None),
                    ("Set SHA-256", super::firmware::short(digest), Some(digest)),
                ] {
                    ui.weak(label);
                    let response = ui.label(value);
                    if let Some(hover) = hover {
                        response.on_hover_text(hover);
                    }
                    ui.end_row();
                }
            });
        facts.horizontal_wrapped(|ui| {
            let response = ui.add_enabled(
                matches!(available, Availability::Ready(_)),
                egui::Button::new("Insert"),
            );
            if let Some(reason) = available.reason() {
                response.clone().on_disabled_hover_text(reason);
            }
            if response.clicked() {
                if let Availability::Ready(unit) = available {
                    insert = Some(unit);
                }
            }
            ui.add_enabled(false, egui::Button::new("Add to machine…"))
                .on_disabled_hover_text("Adding a set to a machine image is not available yet.");
            ui.add_enabled(false, egui::Button::new("Make transfer media…"))
                .on_disabled_hover_text(if set.extras.is_empty() {
                    "Make transfer media: this set has no extras."
                } else {
                    "Making transfer media from extras is not available yet."
                });
            ui.add_enabled(false, egui::Button::new("Export"))
                .on_disabled_hover_text("Set export is not available yet.");
        });
        facts.weak(
            "Stored media and extras stay read-only. Installer writes use the selected \
             workspace's journal.",
        );

        let media = &mut columns[1];
        media.strong("Media in catalogue order");
        for (index, medium) in set.media.iter().enumerate() {
            media.label(format!("{}. {}", index + 1, medium.label));
            let hex = medium.blob.hex();
            media
                .small(format!(
                    "{} · SHA-256 {}",
                    format_detail(summary, medium),
                    super::firmware::short(hex)
                ))
                .on_hover_text(hex);
        }
        if let Availability::Unavailable(reason) = &available {
            media.colored_label(media.visuals().warn_fg_color, reason);
        }

        let extras = &mut columns[2];
        extras.strong("Extras");
        if set.extras.is_empty() {
            extras.label("No extras in this set.");
        }
        for extra in &set.extras {
            let blob = summary
                .storage_blobs
                .iter()
                .find(|d| d.decoded_digest().ok().as_ref() == Some(&extra.blob));
            let rights = blob
                .and_then(image_store::license::open_license)
                .map_or_else(|| "user-supplied".into(), |l| format!("open · {l}"));
            let dos = extra.dos_name.as_ref().map_or_else(
                || "DOS name assigned on transfer".into(),
                |name| format!("DOS name {name}"),
            );
            extras.label(&extra.filename);
            extras.small(format!(
                "{} · {} · {dos} · {rights}",
                kind_label(extra.kind),
                human_size(extra.size)
            ));
            extras.horizontal(|ui| {
                if extra.kind == ExtraKind::Document {
                    ui.add_enabled(false, egui::Button::new("Open"))
                        .on_disabled_hover_text(
                            "Opening documents on the host is not available yet.",
                        );
                } else {
                    ui.add_enabled(false, egui::Button::new("Export…"))
                        .on_disabled_hover_text(
                            "Saving extras is not available yet. Archives and executables are \
                             never opened or run on the host.",
                        );
                }
            });
        }
    });
    if let Some(reason) = &image.failure {
        ui.colored_label(ui.visuals().warn_fg_color, reason);
    }
    insert
}

impl LibraryView {
    /// The import shares the single library worker, which stays occupied for
    /// the whole of a running image session.
    pub fn can_import_media(&self) -> bool {
        self.worker.is_some()
            && !self.worker_dead
            && !self.busy
            && self.active.is_none()
            && self.media_import.is_none()
    }
    /// A stopped worker never answers, so a waiting import dialog stops
    /// waiting and offers Close.
    pub(super) fn fail_media_import(&mut self) {
        if let Some(dialog) = self.media_import.as_mut() {
            dialog.stopped("Image store worker stopped; nothing more was imported".into());
        }
    }
    /// File › Import installation media…: an empty review; picking comes next.
    pub fn begin_media_import(&mut self) {
        self.media_import = Some(ImportDialog::default());
    }
    /// List picked files on the worker, so the window never touches them.
    pub fn media_picked(&mut self, pick: Pick, paths: Vec<PathBuf>) {
        let Some(dialog) = self.media_import.as_mut() else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        dialog.waiting = Some(pick);
        dialog.error = None;
        self.busy = true;
        self.send(super::Request::PreviewMedia(pick, paths));
    }
    /// The installation set whose Insert the shell routes into a drive.
    pub fn selected_set(&self) -> Option<image_store::Digest> {
        self.media_selected
            .as_ref()
            .map(|row| row.image.descriptor.digest.clone())
    }
    /// Selecting a set drops any machine selection: with neither an image tag
    /// nor a machine file selected the shell has nothing to Start.
    pub(super) fn select_media(&mut self, row: &MediaRow, selected_file: &mut Option<usize>) {
        self.media_selected = Some(row.clone());
        self.firmware_selected = None;
        self.selected = None;
        self.readiness = None;
        self.pending_launch = None;
        *selected_file = None;
    }
    pub(super) fn media_list(
        &mut self,
        ui: &mut egui::Ui,
        selected_file: &mut Option<usize>,
    ) -> bool {
        let mut changed = false;
        let snapshot = self.snapshot.clone();
        let drives = self.running_drives.clone();
        self.rows_built = 0;
        egui::ScrollArea::horizontal()
            .id_salt("media-set-columns")
            .show(ui, |ui| {
                ui.set_min_width(WIDTHS.iter().sum::<f32>() + 64.0);
                changed |= super::sort_header(
                    ui,
                    &COLUMNS,
                    &WIDTHS,
                    &mut self.query.media_sort,
                    &mut self.query.media_descending,
                );
                egui::ScrollArea::vertical()
                    .id_salt("media-set-list")
                    .max_height((ui.available_height() - 26.0).max(80.0))
                    .show_rows(ui, 20.0, snapshot.media_rows.len(), |ui, range| {
                        for index in range {
                            self.rows_built += 1;
                            let row = &snapshot.media_rows[index];
                            let selected = self
                                .media_selected
                                .as_ref()
                                .is_some_and(|current| current.same(row));
                            // A double click only selects: sets have no launch action.
                            let (clicked, double) = super::draw_cells(
                                ui,
                                &cells(row, &drives),
                                &WIDTHS,
                                selected,
                                index % 2 == 0,
                            );
                            if (clicked || double) && !selected {
                                self.select_media(row, selected_file);
                            }
                        }
                    });
            });
        if snapshot.media_rows.is_empty() {
            ui.label(if snapshot.media_total == 0 {
                "No installation sets yet. Import installation media… copies floppy or CD \
                 images into the store."
            } else {
                "No sets match this search."
            });
        }
        changed
    }
    pub(super) fn media_modal(
        &mut self,
        ctx: &egui::Context,
        classic: bool,
        requests: &mut Vec<super::Request>,
    ) {
        let Some(dialog) = &mut self.media_import else {
            return;
        };
        match dialog.show(ctx, classic) {
            DialogAction::None => (),
            DialogAction::Pick(pick) => self.media_pick = Some(pick),
            DialogAction::Import => requests.extend(self.import_media_request()),
            DialogAction::CancelImport => dialog.control.cancel(),
            DialogAction::Close => {
                if let Some(dialog) = self.media_import.take() {
                    dialog.control.cancel();
                }
            }
        }
    }
    /// The import request for the reviewed set, if the dialog allows one.
    pub(super) fn import_media_request(&mut self) -> Option<super::Request> {
        let dialog = self.media_import.as_mut()?;
        if !dialog.can_import() {
            return None;
        }
        let request = match dialog.request() {
            Ok(request) => request,
            Err(error) => {
                dialog.error = Some(error);
                return None;
            }
        };
        dialog.control = Control::default();
        dialog.importing = true;
        dialog.error = None;
        self.busy = true;
        self.status = Some("Importing installation media…".into());
        Some(super::Request::ImportMedia(
            Box::new(request),
            dialog.control.clone(),
        ))
    }
}

/// Cancel and progress shared between the dialog and the worker's import.
#[derive(Clone, Debug, Default)]
pub(crate) struct Control(Arc<ControlState>);
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
pub(crate) enum Pick {
    Files,
    Folder,
    Extras,
}
/// Picked files with their exact sizes; source paths stay local hints.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sources {
    pub files: Vec<(PathBuf, u64)>,
    /// Hidden files a folder listing skipped, as the command line reports them.
    pub notes: Vec<String>,
}
/// Worker side of a pick: a folder lists by the command line's rule, files
/// keep the order they were picked in, and every source must be a regular file.
pub(crate) fn inspect(pick: Pick, paths: &[PathBuf]) -> Result<Sources, String> {
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
pub(crate) enum Outcome {
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
pub(crate) fn import(
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

/// One picked medium as the review lists it.
#[derive(Clone, Debug)]
pub(crate) struct DiskDraft {
    pub path: PathBuf,
    pub size: u64,
    pub detected: Option<&'static str>,
    /// The detected format until the user overrides it.
    pub format: Option<&'static str>,
    pub label: String,
}
/// One picked extra; an empty DOS name is assigned on transfer.
#[derive(Clone, Debug)]
pub(crate) struct ExtraDraft {
    pub path: PathBuf,
    pub size: u64,
    pub kind: ExtraKind,
    pub dos_name: String,
}
fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
/// An uppercase 8.3 proposal from a host name, or nothing when none is valid.
fn suggest_dos(path: &Path) -> String {
    let upper = file_name(path).to_uppercase();
    let (stem, extension) = upper.rsplit_once('.').unwrap_or((upper.as_str(), ""));
    let keep = |text: &str, length: usize| -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .take(length)
            .collect()
    };
    let (stem, extension) = (keep(stem, 8), keep(extension, 3));
    let name = if extension.is_empty() {
        stem
    } else {
        format!("{stem}.{extension}")
    };
    if PcMediaFormats.validate_name(&name).is_ok() {
        name
    } else {
        String::new()
    }
}
/// A local tag from the title: lowercase words joined by hyphens, version 1.0.
fn default_tag(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(64).collect();
    let slug = slug.trim_end_matches('-');
    format!(
        "{}:1.0",
        if slug.is_empty() {
            "installation-media"
        } else {
            slug
        }
    )
}
fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn format_choice(format: Option<&str>) -> String {
    format.map_or_else(
        || "Choose format…".into(),
        |name| match name {
            OPTICAL => "Optical · ISO-2048".into(),
            name => assembly::descriptors::FLOPPY_TYPES
                .iter()
                .find(|f| f.name == name)
                .map_or_else(
                    || name.to_owned(),
                    |f| {
                        format!(
                            "Floppy · {name} ({}/{}/{})",
                            f.geometry.cylinders, f.geometry.heads, f.geometry.sectors_per_track
                        )
                    },
                ),
        },
    )
}

/// Import installation media… review: ordered disks with labels and formats,
/// extras with kinds and DOS names, title and metadata, then the outcome.
pub(super) struct ImportDialog {
    pub disks: Vec<DiskDraft>,
    pub extras: Vec<ExtraDraft>,
    pub title: String,
    pub tag: String,
    tag_edited: bool,
    pub software_platform: String,
    pub publisher: String,
    pub year: String,
    pub genre: String,
    pub labels: String,
    pub description: String,
    pub notes: Vec<String>,
    /// A pick is being listed on the worker.
    pub waiting: Option<Pick>,
    pub importing: bool,
    pub control: Control,
    pub outcome: Option<Outcome>,
    pub error: Option<String>,
    /// The worker stopped; only Close remains.
    pub worker_stopped: bool,
}
impl Default for ImportDialog {
    fn default() -> Self {
        Self {
            disks: Vec::new(),
            extras: Vec::new(),
            title: String::new(),
            tag: default_tag(""),
            tag_edited: false,
            software_platform: "DOS".into(),
            publisher: String::new(),
            year: String::new(),
            genre: String::new(),
            labels: String::new(),
            description: String::new(),
            notes: Vec::new(),
            waiting: None,
            importing: false,
            control: Control::default(),
            outcome: None,
            error: None,
            worker_stopped: false,
        }
    }
}
pub(super) enum DialogAction {
    None,
    /// The shell runs the host picker and hands the paths back.
    Pick(Pick),
    Import,
    CancelImport,
    Close,
}
impl ImportDialog {
    /// A new pick of media replaces the list; extras add to theirs.
    pub fn sources(&mut self, pick: Pick, result: Result<Sources, String>) {
        self.waiting = None;
        let sources = match result {
            Ok(sources) => sources,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.outcome = None;
        if pick == Pick::Extras {
            for (path, size) in sources.files {
                let mut dos_name = suggest_dos(&path);
                if self.extras.iter().any(|e| e.dos_name == dos_name) {
                    dos_name.clear();
                }
                self.extras.push(ExtraDraft {
                    kind: media_import::extra_kind(&path),
                    path,
                    size,
                    dos_name,
                });
            }
            return;
        }
        self.notes = sources.notes;
        self.disks = sources
            .files
            .into_iter()
            .map(|(path, size)| {
                let detected = media_import::propose_format(&path, size);
                let name = file_name(&path);
                let label = name
                    .rsplit_once('.')
                    .map_or(name.as_str(), |(stem, _)| stem)
                    .to_owned();
                DiskDraft {
                    label: if label.is_empty() { name } else { label },
                    path,
                    size,
                    detected,
                    format: detected,
                }
            })
            .collect();
    }
    pub fn set_title(&mut self, title: String) {
        self.title = title;
        if !self.tag_edited {
            self.tag = default_tag(&self.title);
        }
    }
    pub fn set_tag(&mut self, tag: String) {
        self.tag = tag;
        self.tag_edited = true;
    }
    /// Swap a disk with its neighbour; order is what the set publishes.
    pub fn move_disk(&mut self, index: usize, up: bool) {
        let other = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1).filter(|i| *i < self.disks.len())
        };
        if let Some(other) = other.filter(|_| index < self.disks.len()) {
            self.disks.swap(index, other);
        }
    }
    fn disk_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.disks.is_empty() {
            problems.push("Pick image files or a folder.".into());
        }
        for (index, disk) in self.disks.iter().enumerate() {
            let number = index + 1;
            let name = file_name(&disk.path);
            match disk.format {
                None => problems.push(format!(
                    "Disk {number} ({name}): {} bytes match no known format; choose one.",
                    disk.size
                )),
                Some(format) => {
                    if let Err(error) = pc_image::media_set::format(format)
                        .and_then(|table| PcMediaFormats.validate_format(&table, disk.size))
                    {
                        problems.push(format!(
                            "Disk {number} ({name}, {} bytes): {error}.",
                            disk.size
                        ));
                    }
                }
            }
            if !valid_text(&disk.label) {
                problems.push(format!(
                    "Disk {number}: enter a label of at most 128 characters."
                ));
            }
        }
        problems
    }
    fn extra_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen = BTreeMap::new();
        for (index, extra) in self.extras.iter().enumerate() {
            let dos = extra.dos_name.trim();
            if dos.is_empty() {
                continue;
            }
            if PcMediaFormats.validate_name(dos).is_err() {
                problems.push(format!(
                    "Row {}: “{dos}” is not a DOS 8.3 name (up to 8 characters, optionally a \
                     dot and up to 3 more; uppercase, no spaces).",
                    index + 1
                ));
            }
            if let Some(first) = seen.insert(dos.to_owned(), index) {
                problems.push(format!(
                    "Rows {} and {} both use {dos}.",
                    first + 1,
                    index + 1
                ));
            }
        }
        problems
    }
    fn metadata_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !valid_text(&self.title) {
            problems.push("Enter a title of at most 128 characters.".into());
        }
        if let Err(error) = Store::validate_tag_name(self.tag.trim()) {
            problems.push(format!("Tag {:?}: {error}.", self.tag.trim()));
        }
        if !self.year.trim().is_empty() && self.year().is_none() {
            problems.push("Year must be a whole number from 1 to 9999.".into());
        }
        if !list(&self.labels).iter().all(|l| valid_text(l)) {
            problems.push("Labels must each be at most 128 characters.".into());
        }
        problems
    }
    fn year(&self) -> Option<i64> {
        self.year
            .trim()
            .parse()
            .ok()
            .filter(|y| (1..=9999).contains(y))
    }
    pub fn problems(&self) -> Vec<String> {
        let mut problems = self.disk_problems();
        problems.extend(self.extra_problems());
        problems.extend(self.metadata_problems());
        problems
    }
    pub fn can_import(&self) -> bool {
        !self.importing
            && !self.worker_stopped
            && self.waiting.is_none()
            && self.outcome.is_none()
            && self.problems().is_empty()
    }
    /// The shared import request; the worker validates it again.
    pub fn request(&self) -> Result<media_import::Request, String> {
        if let Some(problem) = self.problems().into_iter().next() {
            return Err(problem);
        }
        let description = self.description.trim();
        Ok(media_import::Request {
            tag: self.tag.trim().into(),
            title: image_store::metadata::Title {
                id: "title".into(),
                title: self.title.trim().into(),
                platform: Some(self.software_platform.clone()).filter(|p| !p.is_empty()),
                publishers: list(&self.publisher),
                genres: list(&self.genre),
                release_year: self.year(),
                description: (!description.is_empty()).then(|| description.to_owned()),
                ..Default::default()
            },
            labels: list(&self.labels),
            media: self
                .disks
                .iter()
                .map(|disk| media_import::Source {
                    path: disk.path.clone(),
                    label: Some(disk.label.trim().into()),
                    format: disk.format.map(str::to_owned),
                })
                .collect(),
            extras: self
                .extras
                .iter()
                .map(|extra| media_import::ExtraSource {
                    path: extra.path.clone(),
                    dos_name: Some(extra.dos_name.trim().to_owned()).filter(|n| !n.is_empty()),
                    kind: Some(extra.kind),
                })
                .collect(),
            compress: true,
        })
    }
    pub fn finished(&mut self, result: Result<Outcome, String>) {
        self.importing = false;
        match result {
            Ok(outcome) => self.outcome = Some(outcome),
            Err(error) => self.error = Some(error),
        }
    }
    fn stopped(&mut self, error: String) {
        self.importing = false;
        self.waiting = None;
        self.worker_stopped = true;
        self.error = Some(error);
    }
    fn editable(&self) -> bool {
        !self.importing && !self.worker_stopped && self.outcome.is_none()
    }
    fn disk_table(&mut self, ui: &mut egui::Ui) {
        let mut moved = None;
        egui::Grid::new("import-media-disks")
            .num_columns(5)
            .striped(true)
            .show(ui, |ui| {
                for heading in ["#", "File / size", "Label", "Format override", "Order"] {
                    ui.strong(heading);
                }
                ui.end_row();
                let count = self.disks.len();
                for (index, disk) in self.disks.iter_mut().enumerate() {
                    ui.label((index + 1).to_string());
                    ui.vertical(|ui| {
                        ui.label(file_name(&disk.path));
                        ui.small(format!("{} bytes", disk.size));
                        ui.small(format!("Detected: {}", format_choice(disk.detected)));
                    });
                    ui.add(egui::TextEdit::singleline(&mut disk.label).desired_width(200.0));
                    egui::ComboBox::from_id_salt(("import-media-format", index))
                        .selected_text(format_choice(disk.format))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut disk.format, None, format_choice(None));
                            for name in media_import::format_names() {
                                ui.selectable_value(
                                    &mut disk.format,
                                    Some(name),
                                    format_choice(Some(name)),
                                );
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(index > 0, egui::Button::new("↑")).clicked() {
                            moved = Some((index, true));
                        }
                        if ui
                            .add_enabled(index + 1 < count, egui::Button::new("↓"))
                            .clicked()
                        {
                            moved = Some((index, false));
                        }
                    });
                    ui.end_row();
                }
            });
        if let Some((index, up)) = moved {
            self.move_disk(index, up);
        }
    }
    fn extra_table(&mut self, ui: &mut egui::Ui) {
        let mut removed = None;
        egui::Grid::new("import-media-extras")
            .num_columns(5)
            .striped(true)
            .show(ui, |ui| {
                for heading in ["#", "File / size", "Kind", "DOS 8.3 name", "Remove"] {
                    ui.strong(heading);
                }
                ui.end_row();
                for (index, extra) in self.extras.iter_mut().enumerate() {
                    ui.label((index + 1).to_string());
                    ui.vertical(|ui| {
                        ui.label(file_name(&extra.path));
                        ui.small(format!("{} bytes", extra.size));
                    });
                    egui::ComboBox::from_id_salt(("import-media-kind", index))
                        .selected_text(kind_label(extra.kind))
                        .show_ui(ui, |ui| {
                            for kind in [
                                ExtraKind::Document,
                                ExtraKind::Archive,
                                ExtraKind::Executable,
                                ExtraKind::Other,
                            ] {
                                ui.selectable_value(&mut extra.kind, kind, kind_label(kind));
                            }
                        });
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut extra.dos_name)
                                .hint_text("assigned")
                                .char_limit(12)
                                .desired_width(110.0),
                        )
                        .changed()
                    {
                        extra.dos_name = extra.dos_name.to_uppercase();
                    }
                    if ui.button("Remove").clicked() {
                        removed = Some(index);
                    }
                    ui.end_row();
                }
                if self.extras.is_empty() {
                    ui.label("");
                    ui.label("No extras. Manuals, read-me files and patches can be added here.");
                    ui.end_row();
                }
            });
        if let Some(index) = removed {
            self.extras.remove(index);
        }
    }
    fn metadata_form(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("import-media-metadata")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Title");
                let mut title = self.title.clone();
                if ui.text_edit_singleline(&mut title).changed() {
                    self.set_title(title);
                }
                ui.end_row();
                ui.label("Tag");
                let mut tag = self.tag.clone();
                if ui.text_edit_singleline(&mut tag).changed() {
                    self.set_tag(tag);
                }
                ui.end_row();
                ui.label("Machine platform");
                ui.label("PC AT");
                ui.end_row();
                ui.label("Software platform");
                egui::ComboBox::from_id_salt("import-media-software-platform")
                    .selected_text(self.software_platform.clone())
                    .show_ui(ui, |ui| {
                        for platform in image_store::metadata::SOFTWARE_PLATFORMS {
                            ui.selectable_value(
                                &mut self.software_platform,
                                (*platform).to_owned(),
                                *platform,
                            );
                        }
                    });
                ui.end_row();
                for (label, value, hint) in [
                    ("Publisher", &mut self.publisher, "Unknown"),
                    ("Year", &mut self.year, "1994"),
                    ("Genre", &mut self.genre, "Adventure"),
                    ("Labels", &mut self.labels, "comma separated"),
                ] {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(value).hint_text(hint));
                    ui.end_row();
                }
                ui.label("Description");
                ui.add(egui::TextEdit::multiline(&mut self.description).desired_rows(2));
                ui.end_row();
            });
    }
    pub fn show(&mut self, ctx: &egui::Context, classic: bool) -> DialogAction {
        let mut action = DialogAction::None;
        egui::Modal::new(egui::Id::unique("import-installation-media")).show(ctx, |ui| {
            ui.set_max_width(760.0);
            if classic {
                crate::shell::gradient_title(ui, "Import installation media…");
            } else {
                ui.heading("Import installation media…");
            }
            egui::ScrollArea::vertical()
                .id_salt("import-media-review")
                .max_height((ctx.content_rect().height() - 220.0).max(240.0))
                .show(ui, |ui| self.review(ui, &mut action));
            if self.importing {
                let (done, total) = self.control.progress();
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if self.control.is_cancelled() {
                        "Cancelling after the current file…".to_owned()
                    } else {
                        format!("Copying and hashing — {done} of {total} files")
                    });
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
            if let Some(outcome) = &self.outcome {
                ui.label(outcome.to_string());
            }
            ui.horizontal(|ui| {
                if self.outcome.is_some() || self.worker_stopped {
                    if ui.button("Close").clicked() {
                        action = DialogAction::Close;
                    }
                    return;
                }
                if ui
                    .add_enabled(
                        self.can_import(),
                        egui::Button::new("Import to private library"),
                    )
                    .clicked()
                {
                    action = DialogAction::Import;
                }
                if self.importing {
                    if ui
                        .add_enabled(
                            !self.control.is_cancelled(),
                            egui::Button::new("Cancel import"),
                        )
                        .clicked()
                    {
                        action = DialogAction::CancelImport;
                    }
                } else if ui.button("Cancel").clicked() {
                    action = DialogAction::Close;
                }
            });
        });
        action
    }
    fn review(&mut self, ui: &mut egui::Ui, action: &mut DialogAction) {
        let editable = self.editable() && self.waiting.is_none();
        ui.label("Pick image files or a folder, then confirm order, labels and formats.");
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Pick files…").clicked() {
                    *action = DialogAction::Pick(Pick::Files);
                }
                if ui.button("Pick folder…").clicked() {
                    *action = DialogAction::Pick(Pick::Folder);
                }
            });
            ui.weak(
                "Folder candidates are sorted by file name; a file list keeps the order it was \
                 picked in. Listing reads names and sizes only.",
            );
            if self.waiting.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Listing picked files…");
                });
            }
            for note in &self.notes {
                ui.small(note);
            }
            self.disk_table(ui);
            let disk_problems = self.disk_problems();
            if !disk_problems.is_empty() && !self.disks.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Choose a format matching the exact byte size and a nonempty label for \
                     every file. No resizing or padding is performed.",
                );
            }
            for problem in &disk_problems {
                ui.small(problem);
            }
            ui.separator();
            ui.strong("Extras — loose files such as manuals and patches");
            if ui.button("Add files…").clicked() {
                *action = DialogAction::Pick(Pick::Extras);
            }
            self.extra_table(ui);
            ui.weak(
                "The kind comes from the file extension and can be changed. A DOS name is \
                 optional: an empty one is assigned, without collisions, when a transfer \
                 medium is made.",
            );
            for problem in self.extra_problems() {
                ui.colored_label(ui.visuals().warn_fg_color, problem);
            }
            ui.separator();
            ui.strong("Title and metadata");
            self.metadata_form(ui);
            for problem in self.metadata_problems() {
                ui.colored_label(ui.visuals().warn_fg_color, problem);
            }
        });
        ui.separator();
        if !self.disks.is_empty() {
            ui.label(format!(
                "Preview: {} as {} in this order, with {} extra(s).",
                self.tag.trim(),
                disks_label(
                    &self
                        .disks
                        .iter()
                        .map(|d| d.format.unwrap_or(UNKNOWN).to_owned())
                        .collect::<Vec<_>>()
                ),
                self.extras.len()
            ));
        }
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Redistribution: user-supplied by default, for every medium and every extra on its \
             own. The import copies and fully hashes every source into the private store; \
             source files stay unchanged. Optical sets can be stored; insertion stays disabled.",
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library_view::{snapshot, Snapshot};
    use image_store::Digest;
    use std::{fs, sync::Arc, time::Duration};

    pub(crate) const FLOPPY_1440: usize = 1_474_560;

    /// A raw floppy image of one repeated byte, at an exact standard size.
    fn disk(path: &Path, size: usize, byte: u8) {
        fs::write(path, vec![byte; size]).unwrap();
    }
    fn import_cli(store: &Path, args: &[&str]) {
        let mut command: Vec<std::ffi::OsString> = ["image", "media", "import"]
            .into_iter()
            .chain(args.iter().copied())
            .map(Into::into)
            .collect();
        command.extend([
            "--store".into(),
            store.as_os_str().to_owned(),
            "--create".into(),
        ]);
        let mut errors = Vec::new();
        assert_eq!(
            cli::command::run(&command, &mut std::io::sink(), &mut errors),
            0,
            "{}",
            String::from_utf8_lossy(&errors)
        );
    }
    pub(crate) fn warm(store: &Store) -> SummaryCache {
        let mut cache = SummaryCache::open(store).unwrap();
        while !cache.pending().is_empty() {
            cache.rebuild(store, 8, &[], &[]).unwrap();
        }
        cache
    }
    /// Three sets beside the fixture's machine: a two-disk 1.44M set with a
    /// manual and a patch, a 1.2M set and an optical set.
    pub(crate) fn library() -> (crate::library_test_support::Fixture, SummaryCache) {
        let f = crate::library_test_support::Fixture::new();
        let root = f.temp.path();
        let store = f.store.root().to_owned();
        let (one, two) = (root.join("setup.img"), root.join("world.img"));
        disk(&one, FLOPPY_1440, 1);
        disk(&two, FLOPPY_1440, 2);
        fs::write(root.join("Manual.txt"), b"synthetic manual").unwrap();
        fs::write(root.join("patch.zip"), b"synthetic patch").unwrap();
        let manual = format!("{}:MANUAL.TXT", root.join("Manual.txt").display());
        import_cli(
            &store,
            &[
                one.to_str().unwrap(),
                two.to_str().unwrap(),
                "--title",
                "Example Adventure",
                "--tag",
                "example-adventure:1.0",
                "--label",
                "1=Disk 1 of 2 - Setup",
                "--label",
                "2=Disk 2 of 2 - World",
                "--extra",
                &manual,
                "--extra",
                root.join("patch.zip").to_str().unwrap(),
            ],
        );
        let atlas = root.join("atlas.img");
        disk(&atlas, 1_228_800, 3);
        import_cli(
            &store,
            &[
                atlas.to_str().unwrap(),
                "--title",
                "Sample Atlas",
                "--tag",
                "sample-atlas:1.0",
            ],
        );
        let disc = root.join("archive.iso");
        disk(&disc, 8192, 4);
        import_cli(
            &store,
            &[
                disc.to_str().unwrap(),
                "--format",
                "1=iso-2048",
                "--title",
                "Moonlit Archive",
                "--tag",
                "moonlit-archive:1.0",
            ],
        );
        let cache = warm(&f.store);
        (f, cache)
    }
    fn query(filter: Filter, text: &str) -> Query {
        Query {
            text: text.into(),
            filter,
            ..Default::default()
        }
    }
    fn names(result: &Snapshot) -> Vec<String> {
        result
            .media_rows
            .iter()
            .map(|r| r.image.name().to_owned())
            .collect()
    }
    pub(crate) fn texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.text().to_owned(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => (),
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }
    fn frame(
        ctx: &egui::Context,
        size: egui::Vec2,
        time: f64,
        events: Vec<egui::Event>,
        draw: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(time),
                events,
                ..Default::default()
            },
            draw,
        );
        output.textures_delta.clear();
        output
    }
    /// One pointer click at `at`, spread over frames as a user's would be.
    fn click(ctx: &egui::Context, at: egui::Pos2, mut draw: impl FnMut(&mut egui::Ui)) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        for (time, events) in [
            (1.0, vec![egui::Event::PointerMoved(at)]),
            (1.02, vec![button(true)]),
            (1.04, vec![button(false)]),
            (1.06, vec![]),
        ] {
            frame(ctx, egui::vec2(1400.0, 1000.0), time, events, &mut draw);
        }
    }
    fn poll_until(view: &mut LibraryView, mut done: impl FnMut(&LibraryView) -> bool) {
        let ctx = egui::Context::default();
        for _ in 0..5000 {
            view.poll(&ctx);
            if done(view) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("worker did not finish; error {:?}", view.error);
    }
    const DRIVE_A: [(u8, FloppyGeometry); 1] = [(0, FloppyGeometry::HD_1_44)];
    impl LibraryView {
        /// Enter the Installation media collection and select a set, as a click does.
        pub(crate) fn select_set(
            &mut self,
            cache: &SummaryCache,
            reference: &str,
            selected_file: &mut Option<usize>,
        ) {
            self.set_filter(Filter::Media, selected_file);
            self.snapshot = Arc::new(snapshot(cache, &self.query));
            let row = self
                .snapshot
                .media_rows
                .iter()
                .find(|r| r.image.reference == reference)
                .unwrap()
                .clone();
            self.select_media(&row, selected_file);
        }
    }

    #[test]
    fn media_rows_facets_and_search_come_from_summaries_without_blob_reads() {
        let (f, cache) = library();
        // No blob, medium or manifest is readable any more.
        fs::rename(
            f.store.root().join("images/blobs"),
            f.store.root().join("hidden-blobs"),
        )
        .unwrap();
        let result = snapshot(&cache, &query(Filter::Media, ""));
        assert_eq!(result.media_total, 3);
        assert_eq!(result.matched, 3);
        assert!(result.rows.is_empty() && result.firmware_rows.is_empty());
        let row = |tag: &str| {
            result
                .media_rows
                .iter()
                .find(|r| r.image.reference == tag)
                .unwrap()
        };
        let adventure = cells(row("example-adventure:1.0"), &[]);
        assert_eq!(
            adventure[..5],
            [
                "Example Adventure (2 x 1.44M)",
                "1.0",
                "",
                "2 x 1.44M",
                "2 files",
            ]
        );
        assert!(adventure[5].contains(" / 2.8 MiB"), "{}", adventure[5]);
        assert_eq!(adventure[6], "User-supplied");
        assert_eq!(adventure[8], "Fits a 1.44M drive");
        assert_eq!(
            cells(row("example-adventure:1.0"), &DRIVE_A)[8],
            "Ready to insert into A:"
        );
        let atlas = cells(row("sample-atlas:1.0"), &DRIVE_A);
        assert_eq!(atlas[0], "Sample Atlas (1 x 1.2M)");
        assert_eq!(atlas[4], "—");
        assert_eq!(atlas[8], "1.2M disks; drive A: is 1.44M");
        let disc = cells(row("moonlit-archive:1.0"), &DRIVE_A);
        assert_eq!(disc[3], "1 x ISO-2048");
        assert_eq!(disc[8], "CD-ROM emulation not available yet");
        assert_eq!(
            result.media_facets["Format"],
            BTreeSet::from(["1.2M".into(), "1.44M".into(), "iso-2048".into()])
        );
        assert_eq!(facet_label("Format", "iso-2048"), "Optical · ISO-2048");
        assert_eq!(facet_label("Format", "1.2M"), "Floppy · 1.2M");
        for (value, expected) in [
            ("1.44M", vec!["Example Adventure"]),
            ("iso-2048", vec!["Moonlit Archive"]),
            ("720K", vec![]),
        ] {
            let filter = Filter::MediaFacet("Format".into(), value.into());
            assert!(filter.is_media());
            assert_eq!(names(&snapshot(&cache, &query(filter, ""))), expected);
        }
        for (text, expected) in [
            ("adventure", vec!["Example Adventure"]),
            ("atlas", vec!["Sample Atlas"]),
            // Disk labels and extra names are searchable too.
            ("world", vec!["Example Adventure"]),
            ("patch", vec!["Example Adventure"]),
            ("missing", vec![]),
        ] {
            assert_eq!(
                names(&snapshot(&cache, &query(Filter::Media, text))),
                expected,
                "{text}"
            );
        }
        let mut sorted = query(Filter::Media, "");
        sorted.media_sort = 0;
        assert_eq!(names(&snapshot(&cache, &sorted))[0], "Example Adventure");
        sorted.media_descending = true;
        assert_eq!(names(&snapshot(&cache, &sorted))[0], "Sample Atlas");
        // The media sort leaves the machine and firmware sorts alone.
        assert_eq!((sorted.sort, sorted.firmware_sort), (0, 0));
        // The machine collection keeps its own rows and only counts sets.
        let machines = snapshot(&cache, &Query::default());
        assert!(machines.media_rows.is_empty());
        assert_eq!(machines.media_total, 3);
        assert_eq!(machines.rows.len(), 1);
        assert_eq!(machines.rows[0].image.reference, "test:source");
    }

    #[test]
    fn warm_set_summaries_survive_a_cache_file_round_trip() {
        let (f, warm) = library();
        let cache = SummaryCache::open(&f.store).unwrap();
        assert!(cache.pending().is_empty(), "{:?}", cache.pending());
        let mut sets = 0;
        for tag in cache.tags() {
            let summary = cache.summary(&tag.manifest.digest).unwrap();
            if summary.kind == image_store::summary::ArtifactKind::MediaSet {
                sets += 1;
                let set = summary.media_set.as_ref().expect("layout read back");
                assert_eq!(
                    Some(set),
                    warm.summary(&tag.manifest.digest)
                        .unwrap()
                        .media_set
                        .as_ref()
                );
                assert!(!formats(summary).iter().any(|n| n == UNKNOWN));
            } else {
                assert!(summary.media_set.is_none());
            }
        }
        assert_eq!(sets, 3);
    }

    #[test]
    fn open_set_payloads_count_only_with_a_valid_licence() {
        let (_f, cache) = library();
        let result = snapshot(&cache, &query(Filter::Media, "adventure"));
        let mut row = result.media_rows[0].clone();
        let mark = |row: &mut MediaRow, license: Option<&str>| {
            let summary = row.image.summary.as_mut().unwrap();
            for d in summary
                .storage_blobs
                .iter_mut()
                .filter(|d| is_set_payload(d))
            {
                d.annotations
                    .insert("unemu.redistribution".into(), "open".into());
                match license {
                    Some(license) => d.annotations.insert("unemu.license".into(), license.into()),
                    None => d.annotations.remove("unemu.license"),
                };
            }
            row.redistribution =
                Redistribution::of_payloads(row.image.summary.as_ref().unwrap(), is_set_payload);
        };
        let pane = |row: &MediaRow| {
            let ctx = egui::Context::default();
            let output = frame(&ctx, egui::vec2(1400.0, 1000.0), 0.0, vec![], |ui| {
                egui::CentralPanel::default().show(ui, |ui| details(ui, row, &[]));
            });
            texts(&output)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>()
        };
        mark(&mut row, Some("MIT"));
        assert_eq!(cells(&row, &[])[6], "Open · MIT");
        let shown = pane(&row);
        assert!(shown
            .iter()
            .any(|t| t == "Document · 16 B · DOS name MANUAL.TXT · open · MIT"));
        assert!(shown.iter().any(|t| t == "Open · MIT"), "{shown:?}");
        // An open mark without a licence, or with an invalid one, is user-supplied,
        // as the push guard counts it.
        for license in [None, Some("not a licence")] {
            mark(&mut row, license);
            assert_eq!(cells(&row, &[])[6], "User-supplied", "{license:?}");
            let shown = pane(&row);
            assert!(
                shown
                    .iter()
                    .any(|t| t == "Document · 16 B · DOS name MANUAL.TXT · user-supplied"),
                "{shown:?}"
            );
        }
    }

    #[test]
    fn an_existing_machine_tag_is_refused_before_any_source_is_read() {
        let (f, _) = library();
        let source = f.temp.path().join("setup.img");
        let request = media_import::Request {
            tag: "test:source".into(),
            title: image_store::metadata::Title {
                title: "Clash".into(),
                ..Default::default()
            },
            labels: Vec::new(),
            media: vec![media_import::Source {
                path: source,
                label: None,
                format: None,
            }],
            extras: Vec::new(),
            compress: true,
        };
        let read = std::cell::Cell::new(false);
        let refusal = media_import::publish(
            &f.store,
            &media_import::plan(&request).unwrap(),
            &|_, _| read.set(true),
            &|| false,
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            refusal,
            "tag conflict: test:source already names a machine image, not an installation \
             set; choose another tag"
        );
        assert!(!read.get());
    }

    #[test]
    fn set_labels_facet_uses_library_labels() {
        let (f, _) = library();
        let tag = f.store.tag_descriptor("sample-atlas:1.0").unwrap();
        f.store
            .edit_library(&tag.digest, |entry| {
                entry.labels = vec!["To install".into()];
                Ok(())
            })
            .unwrap();
        let cache = warm(&f.store);
        let result = snapshot(&cache, &query(Filter::Media, ""));
        assert_eq!(
            result.media_facets["Labels"],
            BTreeSet::from(["To install".into()])
        );
        let filter = Filter::MediaFacet("Labels".into(), "To install".into());
        assert_eq!(
            names(&snapshot(&cache, &query(filter, ""))),
            ["Sample Atlas"]
        );
    }

    #[test]
    fn set_summaries_cached_before_the_disk_list_are_rebuilt() {
        let (f, _) = library();
        // Rewrite each set row as an older build wrote it, without the layout.
        let path = f.store.root().join("cache/summaries/index.json");
        let mut text = fs::read_to_string(&path).unwrap();
        let mut stripped = 0;
        while let Some(start) = text.find(",\"media_set\":{") {
            let (mut depth, mut quoted, mut escaped) = (0i32, false, false);
            let body = start + ",\"media_set\":".len();
            let end = body
                + text[body..]
                    .char_indices()
                    .find(|&(_, c)| {
                        match c {
                            _ if escaped => escaped = false,
                            '\\' if quoted => escaped = true,
                            '"' => quoted = !quoted,
                            '{' | '[' if !quoted => depth += 1,
                            '}' | ']' if !quoted => depth -= 1,
                            _ => (),
                        }
                        depth == 0
                    })
                    .unwrap()
                    .0
                + 1;
            text.replace_range(start..end, "");
            stripped += 1;
        }
        assert_eq!(stripped, 3);
        fs::write(&path, text).unwrap();
        let mut cache = SummaryCache::open(&f.store).unwrap();
        assert_eq!(cache.pending().len(), 3);
        while !cache.pending().is_empty() {
            cache.rebuild(&f.store, 8, &[], &[]).unwrap();
        }
        let result = snapshot(&cache, &query(Filter::Media, "adventure"));
        let set = result.media_rows[0].set().unwrap();
        assert_eq!(set.media[1].label, "Disk 2 of 2 - World");
        assert_eq!(set.extras[0].dos_name.as_deref(), Some("MANUAL.TXT"));
    }

    #[test]
    fn set_pane_lists_disks_extras_and_disabled_actions_and_never_offers_start() {
        let (f, cache) = library();
        let result = Arc::new(snapshot(&cache, &query(Filter::Media, "")));
        let ctx = egui::Context::default();
        let size = egui::vec2(1400.0, 1000.0);
        let mut shown = Vec::new();
        for row in &result.media_rows {
            let output = frame(&ctx, size, 0.0, vec![], |ui| {
                egui::CentralPanel::default().show(ui, |ui| details(ui, row, &DRIVE_A));
            });
            shown.extend(texts(&output).into_iter().map(|(t, _)| t));
        }
        let first = f.temp.path().join("setup.img");
        let first = super::super::firmware::short(Digest::of(&fs::read(first).unwrap()).hex());
        for expected in [
            "Example Adventure (2 x 1.44M)",
            "example-adventure:1.0 · private library",
            "PC AT / Unknown",
            "User-supplied · private store and export",
            "1. Disk 1 of 2 - Setup",
            "2. Disk 2 of 2 - World",
            &format!("Floppy · 1.44M, 80 cylinders / 2 heads / 18 sectors · SHA-256 {first}"),
            "Manual.txt",
            "Document · 16 B · DOS name MANUAL.TXT · user-supplied",
            "patch.zip",
            "Archive · 15 B · DOS name assigned on transfer · user-supplied",
            "No extras in this set.",
            "Optical · ISO-2048, 2048-byte sectors · SHA-256 ",
            "CD-ROM emulation not available yet",
            "1.2M disks; drive A: is 1.44M",
            "Insert",
            "Add to machine…",
            "Make transfer media…",
            "Export",
            "Open",
            "Export…",
        ] {
            assert!(
                shown
                    .iter()
                    .any(|t| t == expected || (expected.ends_with(' ') && t.starts_with(expected))),
                "missing {expected:?} in {shown:?}"
            );
        }
        assert!(!shown.iter().any(|t| t == "Start"));

        // The whole library view: a machine selection is dropped on entering
        // the collection, and nothing it draws there is a Start action.
        let machines = snapshot(&cache, &Query::default());
        let mut view = LibraryView::new(None);
        view.selected = Some(machines.rows[0].image.clone());
        view.snapshot = Arc::new(machines);
        let mut file = Some(0);
        view.set_filter(Filter::Media, &mut file);
        assert!(view.selected.is_none() && file.is_none());
        view.snapshot = result.clone();
        view.media_selected = Some(result.media_rows[0].clone());
        let mut commands = Vec::new();
        let output = frame(&ctx, size, 0.0, vec![], |ui| {
            commands = view.show(ui, &[], &mut file, None);
        });
        let drawn = texts(&output);
        for expected in [
            "Installation media (3)",
            "Import installation media…",
            "Format",
        ] {
            assert!(drawn.iter().any(|(t, _)| t == expected), "{expected}");
        }
        assert!(!drawn.iter().any(|(t, _)| t == "Start"));
        assert!(commands.is_empty());
        // Leaving the collection drops the set selection.
        view.set_filter(Filter::All, &mut file);
        assert!(view.media_selected.is_none());
    }

    #[test]
    fn insert_is_offered_only_for_a_matching_running_drive_and_returns_its_unit() {
        let (_f, cache) = library();
        let result = snapshot(&cache, &query(Filter::Media, ""));
        let summary = |tag: &str| {
            result
                .media_rows
                .iter()
                .find(|r| r.image.reference == tag)
                .unwrap()
                .image
                .summary
                .clone()
                .unwrap()
        };
        let adventure = summary("example-adventure:1.0");
        assert_eq!(
            availability(&adventure, &[]),
            Availability::NoMachine("1.44M".into())
        );
        assert_eq!(availability(&adventure, &DRIVE_A), Availability::Ready(0));
        let both = [(0, FloppyGeometry::HD_1_2), (1, FloppyGeometry::HD_1_44)];
        assert_eq!(availability(&adventure, &both), Availability::Ready(1));
        assert_eq!(
            availability(&summary("moonlit-archive:1.0"), &DRIVE_A),
            Availability::Unavailable("CD-ROM emulation not available yet".into())
        );
        // Clicking Insert names the drive; without a running drive it is disabled.
        let row = result
            .media_rows
            .iter()
            .find(|r| r.image.reference == "example-adventure:1.0")
            .unwrap();
        for (drives, expected) in [(&DRIVE_A[..], Some(0)), (&[][..], None)] {
            let ctx = egui::Context::default();
            let output = frame(&ctx, egui::vec2(1400.0, 1000.0), 0.0, vec![], |ui| {
                egui::CentralPanel::default().show(ui, |ui| details(ui, row, drives));
            });
            let (_, rect) = texts(&output)
                .into_iter()
                .find(|(t, _)| t == "Insert")
                .unwrap();
            let mut pressed = None;
            click(&ctx, rect.center(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    pressed = pressed.or(details(ui, row, drives));
                });
            });
            assert_eq!(pressed, expected);
        }
    }

    #[test]
    fn double_clicking_a_set_row_selects_it_and_issues_no_start() {
        let (_f, cache) = library();
        let ctx = egui::Context::default();
        let size = egui::vec2(1400.0, 800.0);
        let mut view = LibraryView::new(None);
        let machines = snapshot(&cache, &Query::default());
        view.selected = Some(machines.rows[0].image.clone());
        view.query.filter = Filter::Media;
        view.snapshot = Arc::new(snapshot(&cache, &query(Filter::Media, "")));
        let mut file = Some(0);
        let mut commands = Vec::new();
        let first = frame(&ctx, size, 0.0, vec![], |ui| {
            commands.extend(view.show(ui, &[], &mut file, None));
        });
        let target = view.snapshot.media_rows[0].clone();
        let (_, rect) = texts(&first)
            .into_iter()
            .find(|(t, _)| *t == set_name(&target))
            .expect("set row drawn");
        let at = rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        for (time, events) in [
            (1.0, vec![egui::Event::PointerMoved(at)]),
            (1.02, vec![button(true)]),
            (1.04, vec![button(false)]),
            (1.06, vec![button(true)]),
            (1.08, vec![button(false)]),
            (1.1, vec![]),
        ] {
            frame(&ctx, size, time, events, |ui| {
                commands.extend(view.show(ui, &[], &mut file, None));
            });
        }
        assert!(view
            .media_selected
            .as_ref()
            .is_some_and(|r| r.same(&target)));
        assert!(view.selected.is_none() && file.is_none());
        assert!(
            !commands.contains(&crate::shell_model::Command::Start),
            "{commands:?}"
        );
    }

    fn sources(files: &[(&Path, u64)]) -> Result<Sources, String> {
        Ok(Sources {
            files: files.iter().map(|(p, s)| (p.to_path_buf(), *s)).collect(),
            notes: Vec::new(),
        })
    }

    #[test]
    fn import_dialog_orders_disks_overrides_formats_and_checks_labels_and_extras() {
        let mut dialog = ImportDialog::default();
        dialog.sources(
            Pick::Files,
            sources(&[
                (Path::new("/in/disk2.img"), FLOPPY_1440 as u64),
                (Path::new("/in/disk1.img"), FLOPPY_1440 as u64),
                (Path::new("/in/odd.bin"), 4096),
                (Path::new("/in/cd.iso"), 8192),
            ]),
        );
        let labels = |d: &ImportDialog| d.disks.iter().map(|d| d.label.clone()).collect::<Vec<_>>();
        assert_eq!(labels(&dialog), ["disk2", "disk1", "odd", "cd"]);
        assert_eq!(
            dialog.disks.iter().map(|d| d.detected).collect::<Vec<_>>(),
            [Some("1.44M"), Some("1.44M"), None, Some("iso-2048")]
        );
        // The file list keeps its order until the user moves a disk.
        dialog.move_disk(1, true);
        assert_eq!(labels(&dialog), ["disk1", "disk2", "odd", "cd"]);
        dialog.move_disk(0, true);
        dialog.move_disk(3, false);
        assert_eq!(labels(&dialog), ["disk1", "disk2", "odd", "cd"]);
        dialog.set_title("Example Adventure!".into());
        assert_eq!(dialog.tag, "example-adventure:1.0");
        let problems = dialog.problems();
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("Disk 3 (odd.bin): 4096 bytes match no known format")),
            "{problems:?}"
        );
        assert!(!dialog.can_import());
        // An override must still agree with the exact size.
        dialog.disks[2].format = Some("720K");
        assert!(dialog
            .problems()
            .iter()
            .any(|p| p.contains("media size differs from format")));
        dialog.disks[2].format = Some("iso-2048");
        dialog.disks[1].label = " ".into();
        assert!(dialog
            .problems()
            .iter()
            .any(|p| p.starts_with("Disk 2: enter a label")));
        dialog.disks[1].label = "Disk 2 of 4 - World".into();
        assert!(dialog.problems().is_empty(), "{:?}", dialog.problems());
        assert!(dialog.can_import());

        dialog.sources(
            Pick::Extras,
            sources(&[
                (Path::new("/in/Manual.pdf"), 10),
                (Path::new("/in/manual.pdf"), 10),
                (Path::new("/in/Setup fix.exe"), 20),
            ]),
        );
        assert_eq!(
            dialog
                .extras
                .iter()
                .map(|e| (e.kind, e.dos_name.as_str()))
                .collect::<Vec<_>>(),
            [
                (ExtraKind::Document, "MANUAL.PDF"),
                (ExtraKind::Document, ""),
                (ExtraKind::Executable, "SETUPFIX.EXE"),
            ]
        );
        dialog.extras[1].dos_name = "MANUAL.PDF".into();
        assert!(dialog
            .problems()
            .contains(&"Rows 1 and 2 both use MANUAL.PDF.".into()));
        dialog.extras[1].dos_name = "LONGNAME99.PDF".into();
        assert!(dialog
            .problems()
            .iter()
            .any(|p| p.starts_with("Row 2: “LONGNAME99.PDF” is not a DOS 8.3 name")));
        dialog.extras[1].dos_name.clear();
        dialog.extras[2].kind = ExtraKind::Other;
        dialog.year = "19x4".into();
        assert!(dialog
            .problems()
            .iter()
            .any(|p| p.starts_with("Year must be")));
        dialog.year = "1994".into();
        dialog.labels = "To install, Adventure".into();
        dialog.description = "A synthetic set.".into();
        dialog.set_tag("custom:2".into());
        dialog.set_title("Another title".into());
        assert_eq!(dialog.tag, "custom:2", "an edited tag stays");
        let request = dialog.request().unwrap();
        assert_eq!(request.tag, "custom:2");
        assert_eq!(request.title.title, "Another title");
        assert_eq!(request.title.release_year, Some(1994));
        assert_eq!(request.title.platform.as_deref(), Some("DOS"));
        assert_eq!(
            request.title.description.as_deref(),
            Some("A synthetic set.")
        );
        assert_eq!(request.labels, ["To install", "Adventure"]);
        assert_eq!(
            request
                .media
                .iter()
                .map(|m| (m.label.clone().unwrap(), m.format.clone().unwrap()))
                .collect::<Vec<_>>(),
            [
                ("disk1".to_owned(), "1.44M".to_owned()),
                ("Disk 2 of 4 - World".into(), "1.44M".into()),
                ("odd".into(), "iso-2048".into()),
                ("cd".into(), "iso-2048".into()),
            ]
        );
        assert_eq!(
            request
                .extras
                .iter()
                .map(|e| (e.dos_name.clone(), e.kind))
                .collect::<Vec<_>>(),
            [
                (Some("MANUAL.PDF".into()), Some(ExtraKind::Document)),
                (None, Some(ExtraKind::Document)),
                (Some("SETUPFIX.EXE".into()), Some(ExtraKind::Other)),
            ]
        );
        // A new pick of media replaces the disks and keeps the extras.
        dialog.sources(Pick::Files, sources(&[(Path::new("/in/x.img"), 368_640)]));
        assert_eq!(labels(&dialog), ["x"]);
        assert_eq!(dialog.extras.len(), 3);
        // A refused pick keeps the reviewed list and shows the refusal.
        dialog.sources(
            Pick::Folder,
            Err(
                "source \"/in/notes.txt\": not a media candidate; use an explicit file list".into(),
            ),
        );
        assert_eq!(labels(&dialog), ["x"]);
        assert!(dialog
            .error
            .as_deref()
            .unwrap()
            .contains("explicit file list"));
    }

    fn import_view(root: &Path) -> LibraryView {
        let mut view = LibraryView::synthetic(root.join("store"));
        poll_until(&mut view, |v| v.snapshot.storage.is_some());
        view
    }
    /// Pick through the worker and wait for the listing.
    fn pick(view: &mut LibraryView, pick: Pick, paths: Vec<PathBuf>) {
        view.media_picked(pick, paths);
        poll_until(view, |v| {
            v.media_import.as_ref().is_some_and(|d| d.waiting.is_none())
        });
        assert!(!view.busy);
    }
    fn run_import(view: &mut LibraryView) -> Result<Outcome, String> {
        let request = view.import_media_request().expect("import allowed");
        assert!(view.busy && !view.can_import_media());
        view.send(request);
        poll_until(view, |v| {
            v.media_import.as_ref().is_some_and(|d| !d.importing)
        });
        assert!(!view.busy);
        let dialog = view.media_import.as_ref().unwrap();
        match (&dialog.outcome, &dialog.error) {
            (Some(outcome), _) => Ok(outcome.clone()),
            (None, Some(error)) => Err(error.clone()),
            _ => panic!("no import result"),
        }
    }

    #[test]
    fn folder_import_runs_on_the_worker_like_the_cli_and_reimport_is_already_imported() {
        let temp = assembly::test_support::TempDir::new("media-import-flow");
        let folder = temp.path().join("disks");
        fs::create_dir(&folder).unwrap();
        disk(&folder.join("b.img"), FLOPPY_1440, 2);
        disk(&folder.join("a.img"), FLOPPY_1440, 1);
        fs::write(folder.join(".DS_Store"), b"hidden").unwrap();
        let manual = temp.path().join("Readme.txt");
        fs::write(&manual, b"synthetic readme").unwrap();
        let mut view = import_view(temp.path());
        view.set_filter(Filter::Media, &mut None);
        view.send(super::super::Request::Query(view.query.clone()));
        assert!(view.can_import_media());
        view.begin_media_import();
        assert!(!view.can_import_media());
        pick(&mut view, Pick::Folder, vec![folder.clone()]);
        let dialog = view.media_import.as_mut().unwrap();
        assert_eq!(dialog.notes, ["skipped hidden file: \".DS_Store\""]);
        assert_eq!(
            dialog
                .disks
                .iter()
                .map(|d| d.label.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"],
            "a folder lists by file name"
        );
        dialog.move_disk(0, false);
        dialog.disks[0].label = "Disk 1 - Setup".into();
        dialog.set_title("Folder Set".into());
        dialog.labels = "To install".into();
        pick(&mut view, Pick::Extras, vec![manual.clone()]);
        assert_eq!(
            run_import(&mut view),
            Ok(Outcome::Imported("folder-set:1.0".into()))
        );
        assert_eq!(view.status.as_deref(), Some("Imported folder-set:1.0"));
        // The same set the command line would publish from the same choices.
        let store = Store::open_existing(temp.path().join("store")).unwrap();
        let image = store
            .resolve(
                "folder-set:1.0",
                &image_store::media_set::Validator(&PcMediaFormats),
            )
            .unwrap();
        let config = image_store::media_set::Config::decode(&image.config).unwrap();
        assert_eq!(
            config
                .media
                .iter()
                .map(|m| m.label.as_str())
                .collect::<Vec<_>>(),
            ["Disk 1 - Setup", "a"]
        );
        assert_eq!(config.media[0].blob, Digest::of(&vec![2; FLOPPY_1440]));
        assert_eq!(config.extras[0].dos_name.as_deref(), Some("README.TXT"));
        assert_eq!(
            fs::read(folder.join("b.img")).unwrap(),
            vec![2; FLOPPY_1440]
        );
        // The new set appears as a row once its summary is built.
        poll_until(&mut view, |v| {
            v.snapshot.media_rows.len() == 1 && v.snapshot.media_rows[0].set().is_some()
        });
        assert_eq!(view.snapshot.media_rows[0].image.name(), "Folder Set");
        assert_eq!(
            view.snapshot.media_facets["Labels"],
            BTreeSet::from(["To install".into()])
        );
        // Importing the same review again is idempotent.
        let dialog = view.media_import.as_mut().unwrap();
        assert!(!dialog.can_import(), "an imported review only closes");
        dialog.outcome = None;
        assert_eq!(
            run_import(&mut view),
            Ok(Outcome::AlreadyImported("folder-set:1.0".into()))
        );
        // A different set under the same tag is refused as the CLI refuses it.
        let dialog = view.media_import.as_mut().unwrap();
        dialog.outcome = None;
        dialog.move_disk(0, false);
        let refusal = run_import(&mut view).unwrap_err();
        assert!(refusal.contains("tag conflict"), "{refusal}");
        view.shutdown();
    }

    #[test]
    fn refusals_read_as_the_cli_gives_them_and_cancel_stops_between_files() {
        let temp = assembly::test_support::TempDir::new("media-import-refusals");
        let folder = temp.path().join("mixed");
        fs::create_dir(&folder).unwrap();
        disk(&folder.join("a.img"), FLOPPY_1440, 1);
        fs::write(folder.join("notes.txt"), b"not a disk").unwrap();
        let mut view = import_view(temp.path());
        view.begin_media_import();
        pick(&mut view, Pick::Folder, vec![folder.clone()]);
        let error = view.media_import.as_ref().unwrap().error.clone().unwrap();
        assert_eq!(
            error,
            format!(
                "source {:?}: not a media candidate; use an explicit file list",
                folder.join("notes.txt")
            )
        );
        assert!(view.media_import.as_ref().unwrap().disks.is_empty());
        // A source that shrinks after review is refused by the shared plan.
        let files: Vec<_> = (0..3).map(|i| folder.join(format!("{i}.img"))).collect();
        for (i, path) in files.iter().enumerate() {
            disk(path, FLOPPY_1440, i as u8 + 10);
        }
        pick(&mut view, Pick::Files, files.clone());
        view.media_import
            .as_mut()
            .unwrap()
            .set_title("Refused".into());
        fs::write(&files[1], [0; 4096]).unwrap();
        let refusal = run_import(&mut view).unwrap_err();
        assert_eq!(
            refusal,
            format!(
                "source {:?}: invalid image: media size differs from format",
                files[1]
            )
        );
        disk(&files[1], FLOPPY_1440, 11);
        // Cancelling before the first file publishes nothing.
        let request = view.import_media_request().unwrap();
        view.media_import.as_ref().unwrap().control.cancel();
        view.send(request);
        poll_until(&mut view, |v| {
            v.media_import.as_ref().is_some_and(|d| !d.importing)
        });
        let dialog = view.media_import.as_ref().unwrap();
        assert!(dialog
            .error
            .as_deref()
            .unwrap()
            .starts_with("import cancelled before source"));
        assert!(
            dialog.can_import(),
            "a cancelled review can be imported again"
        );
        let store = Store::open_existing(temp.path().join("store")).unwrap();
        assert!(store.tag_descriptor("refused:1.0").is_err());
        view.shutdown();
    }

    fn assert_failed_and_closable(view: &mut LibraryView) {
        let dialog = view.media_import.as_ref().expect("dialog stays open");
        assert!(dialog.error.is_some() && dialog.worker_stopped && !dialog.importing);
        assert!(!view.busy && !view.can_import_media());
        let ctx = egui::Context::default();
        // A modal sizes itself over its first frames; wait until it stops moving.
        let mut drawn: Vec<(String, egui::Rect)> = Vec::new();
        for step in 0..20 {
            let previous = drawn.iter().find(|(t, _)| t == "Close").map(|(_, r)| *r);
            drawn = texts(&frame(
                &ctx,
                egui::vec2(1400.0, 1000.0),
                f64::from(step) * 0.05,
                vec![],
                |ui| {
                    view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
                },
            ));
            let current = drawn.iter().find(|(t, _)| t == "Close").map(|(_, r)| *r);
            if current.is_some() && current == previous {
                break;
            }
        }
        assert!(!drawn.iter().any(|(t, _)| t == "Import to private library"));
        let (_, close) = drawn
            .into_iter()
            .find(|(t, _)| t == "Close")
            .expect("Close offered");
        click(&ctx, close.center(), |ui| {
            view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
        });
        assert!(view.media_import.is_none());
    }

    #[test]
    fn import_dialog_fails_and_closes_when_the_worker_is_missing_or_dies() {
        let temp = assembly::test_support::TempDir::new("media-dead-worker");
        let source = temp.path().join("a.img");
        disk(&source, FLOPPY_1440, 1);
        assert!(!LibraryView::new(None).can_import_media());
        // The request cannot reach a worker whose thread has already ended.
        let mut view = import_view(&temp.path().join("stopped"));
        let mut worker = view.worker.take().unwrap();
        worker.shutdown();
        view.worker = Some(worker);
        view.begin_media_import();
        view.media_picked(Pick::Files, vec![source.clone()]);
        assert!(view.worker_dead);
        assert_failed_and_closable(&mut view);
        // The worker dies inside the import.
        let root = temp.path().join("crash");
        let mut view = LibraryView::new(Some(root.join("store")));
        view.worker = Some(crate::library_worker::Worker::media_crash(
            root.join("store"),
        ));
        poll_until(&mut view, |v| v.snapshot.storage.is_some());
        view.begin_media_import();
        pick(&mut view, Pick::Files, vec![source]);
        view.media_import
            .as_mut()
            .unwrap()
            .set_title("Crash".into());
        let request = view.import_media_request().unwrap();
        view.send(request);
        poll_until(&mut view, |v| v.worker_dead);
        assert_failed_and_closable(&mut view);
    }

    #[test]
    fn rows_without_a_summary_say_pending_or_unavailable() {
        let row = |failure: Option<&str>| MediaRow {
            image: ImageRow {
                reference: "synthetic-set:1.0".into(),
                descriptor: Descriptor::new(media_type::MANIFEST, Digest::of(b"set"), 7),
                summary: None,
                failure: failure.map(Into::into),
            },
            redistribution: Redistribution::default(),
        };
        assert_eq!(cells(&row(None), &[])[5], "Pending");
        let failed = cells(&row(Some("missing blob")), &[]);
        assert_eq!(failed[5], "Unavailable");
        assert!(failed[0].starts_with("[!] "));
    }
}
