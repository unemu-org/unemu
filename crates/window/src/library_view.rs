// SPDX-License-Identifier: AGPL-3.0-or-later
//! Explorer-style local image library; machine-images §§2.1, 3 and 7.
//! Store jobs own I/O and PC readiness, launch and publication adapters.
//! The tree, facets and virtualized rows consume platform-neutral summaries.
//! Firmware packages form their own collection (§5.3), see [`firmware`].

pub(crate) mod firmware;

use crate::{
    layers_dialog::LayersDialog,
    shell_model::{Command, LibraryEntry},
};
use image_store::{
    summary::{ArtifactKind, Facets, ImageSummary, SummaryCache},
    Descriptor, Digest, Store,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
};

type Result<T> = std::result::Result<T, String>;
#[cfg(test)]
const REBUILD_BATCH: usize = 8;

pub(super) fn default_store_path() -> Result<PathBuf> {
    image_store::location::default_store_root().map_err(|e| e.to_string())
}
#[cfg(test)]
use crate::library_worker::regular_workspace;
pub(super) use crate::library_worker::Request;
use crate::library_worker::{regular_name, Event, Finish, Recovery, Worker, WorkspaceKind};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Recent,
    Favorites,
    MachineFiles,
    Firmware,
    Facet(String, String),
}
impl Filter {
    /// The Firmware collection and its facets list packages instead of machines.
    fn is_firmware(&self) -> bool {
        match self {
            Self::Firmware => true,
            Self::Facet(kind, _) => firmware::is_facet(kind),
            _ => false,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub(super) struct Query {
    text: String,
    filter: Filter,
    sort: usize,
    descending: bool,
    expanded: BTreeSet<String>,
    /// The Firmware list has other columns, so it keeps its own sort.
    firmware_sort: usize,
    firmware_descending: bool,
}
#[derive(Clone, Debug)]
pub(super) struct ImageRow {
    pub reference: String,
    pub descriptor: Descriptor,
    pub summary: Option<ImageSummary>,
    pub failure: Option<String>,
}
impl ImageRow {
    pub fn name(&self) -> &str {
        self.summary
            .as_ref()
            .and_then(|s| {
                if s.name.is_empty() {
                    s.titles.first().map(|t| t.title.as_str())
                } else {
                    Some(s.name.as_str())
                }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.reference)
    }
    fn repository(&self) -> &str {
        self.reference
            .rsplit_once(':')
            .map_or(self.reference.as_str(), |(r, _)| r)
    }
}
#[derive(Clone)]
struct ViewRow {
    image: ImageRow,
    group: Option<usize>,
    child: bool,
}
#[derive(Default)]
pub(super) struct Snapshot {
    rows: Vec<ViewRow>,
    facets: BTreeMap<String, BTreeSet<String>>,
    total: usize,
    matched: usize,
    pending: usize,
    nominal_bytes: u64,
    storage: Option<(u64, u64)>,
    warning: Option<String>,
    firmware_rows: Vec<firmware::FirmwareRow>,
    firmware_total: usize,
}

fn facets_for(filter: &Filter) -> Facets {
    let mut f = Facets::default();
    if let Filter::Facet(kind, value) = filter {
        match kind.as_str() {
            "Genre" => f.genres.push(value.clone()),
            "Platform" => f.platforms.push(value.clone()),
            "Year" => {
                if let Ok(y) = value.parse() {
                    f.years.push(y);
                }
            }
            "Developer" => f.developers.push(value.clone()),
            "Publisher" => f.publishers.push(value.clone()),
            "Series" => f.series.push(value.clone()),
            "Labels" => f.labels.push(value.clone()),
            "Role" => f.firmware_roles.push(value.clone()),
            "Board" => f.boards.push(value.clone()),
            "Machine class" => f.machine_classes.push(value.clone()),
            _ => (),
        }
    }
    f
}
fn sort_value(row: &ImageRow, column: usize) -> String {
    let s = row.summary.as_ref();
    let title = s.and_then(|s| s.titles.first());
    match column {
        1 => row.reference.clone(),
        2 => title.and_then(|t| t.platform.clone()).unwrap_or_default(),
        3 => title.map(|t| t.genres.join(", ")).unwrap_or_default(),
        4 => format!("{:020}", title.and_then(|t| t.year).unwrap_or_default()),
        5 => title.map(|t| t.developers.join(", ")).unwrap_or_default(),
        6 => s.and_then(|s| s.last_run.clone()).unwrap_or_default(),
        7 => format!("{:020}", s.map_or(0, |s| s.stored_size)),
        8 => s.map(|s| s.labels.join(", ")).unwrap_or_default(),
        _ => row.name().to_lowercase(),
    }
}
pub(super) fn snapshot(cache: &SummaryCache, q: &Query) -> Snapshot {
    let mut result = Snapshot {
        storage: cache.storage_totals(),
        total: cache.tags().len(),
        pending: cache.pending().len(),
        warning: cache.library_error().map(str::to_owned),
        ..Default::default()
    };
    let matches: BTreeSet<_> = cache
        .query(&q.text, &facets_for(&q.filter))
        .into_iter()
        .map(|r| r.reference)
        .collect();
    let mut groups: BTreeMap<String, Vec<ImageRow>> = BTreeMap::new();
    let mut counted = BTreeSet::new();
    let firmware_mode = q.filter.is_firmware();
    let mut packages = Vec::new();
    let mut machine_roms = firmware::MachineRoms::default();
    for tag in cache.tags() {
        let s = cache.summary(&tag.manifest.digest);
        let kind = s.map_or_else(
            || match tag.manifest.artifact_type.as_deref() {
                Some(image_store::media_type::MEDIA_SET) => ArtifactKind::MediaSet,
                Some(image_store::media_type::FIRMWARE) => ArtifactKind::Firmware,
                _ => ArtifactKind::Machine,
            },
            |summary| summary.kind,
        );
        // Media sets and firmware packages have no launch action; they belong
        // to their own collections, not to the machine rows.
        if kind != ArtifactKind::Machine {
            result.total -= 1;
            if kind == ArtifactKind::Firmware {
                result.firmware_total += 1;
                if let Some(s) = s {
                    firmware::add_facets(&mut result.facets, s);
                }
                packages.push(tag);
            }
            continue;
        }
        if let Some(s) = s.filter(|_| firmware_mode) {
            machine_roms.add(&tag.name, s);
        }
        if let Some(s) = s {
            if counted.insert(s.image.clone()) {
                result.nominal_bytes += s.stored_size;
            }
            for label in &s.labels {
                result
                    .facets
                    .entry("Labels".into())
                    .or_default()
                    .insert(label.clone());
            }
            for t in &s.titles {
                for (kind, values) in [
                    ("Genre", t.genres.clone()),
                    ("Platform", t.platform.iter().cloned().collect()),
                    ("Year", t.year.map(|y| y.to_string()).into_iter().collect()),
                    ("Developer", t.developers.clone()),
                    ("Publisher", t.publishers.clone()),
                    ("Series", t.series.iter().cloned().collect()),
                ] {
                    result.facets.entry(kind.into()).or_default().extend(values);
                }
            }
        }
        if firmware_mode {
            continue;
        }
        let text_match = if s.is_some() {
            matches.contains(&tag.name)
        } else {
            q.text.is_empty() || tag.name.to_lowercase().contains(&q.text.to_lowercase())
        };
        let filter_match = match &q.filter {
            Filter::MachineFiles => false,
            Filter::Favorites => s.is_some_and(|s| s.favorite),
            Filter::Recent => s.is_some_and(|s| s.last_run.is_some()),
            Filter::Facet(_, _) => matches.contains(&tag.name),
            _ => true,
        };
        if !text_match || !filter_match {
            continue;
        }
        let image = ImageRow {
            reference: tag.name.clone(),
            descriptor: tag.manifest.clone(),
            summary: s.cloned(),
            failure: cache
                .failure(&tag.manifest.digest)
                .map(|f| f.reason.clone()),
        };
        groups
            .entry(image.repository().into())
            .or_default()
            .push(image);
        result.matched += 1;
    }
    if firmware_mode {
        result.firmware_rows = firmware::rows(cache, &packages, &matches, q, &machine_roms);
        result.matched = result.firmware_rows.len();
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    for images in &mut groups {
        images.sort_by_cached_key(|row| sort_value(row, q.sort));
        if q.descending {
            images.reverse();
        }
    }
    groups.sort_by_cached_key(|rows| sort_value(&rows[0], q.sort));
    if q.descending {
        groups.reverse();
    }
    for images in groups {
        let count = images.len();
        if count > 1 {
            let first = images[0].clone();
            let expanded = q.expanded.contains(first.repository());
            result.rows.push(ViewRow {
                image: first,
                group: Some(count),
                child: false,
            });
            if !expanded {
                continue;
            }
        }
        result.rows.extend(images.into_iter().map(|image| ViewRow {
            image,
            group: None,
            child: count > 1,
        }));
    }
    result
}

const COLUMN_WIDTHS: [f32; 9] = [210.0, 90.0, 100.0, 120.0, 56.0, 130.0, 110.0, 100.0, 140.0];
fn row_cells(row: &ViewRow) -> [String; 9] {
    let image = &row.image;
    let summary = image.summary.as_ref();
    let title = summary.and_then(|s| s.titles.first());
    [
        format!(
            "{}{}{}",
            if row.child { "    " } else { "" },
            if image.failure.is_some() { "[!] " } else { "" },
            image.name()
        ),
        image
            .reference
            .rsplit(':')
            .next()
            .unwrap_or_default()
            .into(),
        title.and_then(|t| t.platform.clone()).unwrap_or_default(),
        title.map(|t| t.genres.join(", ")).unwrap_or_default(),
        title
            .and_then(|t| t.year)
            .map(|y| y.to_string())
            .unwrap_or_default(),
        title.map(|t| t.developers.join(", ")).unwrap_or_default(),
        summary
            .and_then(|s| s.last_run.clone())
            .unwrap_or_else(|| "Never".into()),
        summary
            .map(|s| human_size(s.stored_size))
            .unwrap_or_else(|| "Pending".into()),
        summary.map(|s| s.labels.join(", ")).unwrap_or_default(),
    ]
}
/// Sortable column headers; a click on the sorted column reverses it.
fn sort_header(
    ui: &mut egui::Ui,
    labels: &[&str; 9],
    widths: &[f32; 9],
    sort: &mut usize,
    descending: &mut bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (column, label) in labels.iter().enumerate() {
            let label = if *sort == column {
                format!("{label} {}", if *descending { "▼" } else { "▲" })
            } else {
                (*label).into()
            };
            if ui
                .add_sized([widths[column], 22.0], egui::Button::new(label))
                .clicked()
            {
                if *sort == column {
                    *descending = !*descending;
                } else {
                    *sort = column;
                    *descending = false;
                }
                changed = true;
            }
        }
    });
    changed
}
fn draw_cells(
    ui: &mut egui::Ui,
    cells: &[String; 9],
    widths: &[f32; 9],
    selected: bool,
    striped: bool,
) -> (bool, bool) {
    let mut clicked = false;
    let mut double = false;
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else if striped {
        ui.visuals().faint_bg_color
    } else {
        egui::Color32::TRANSPARENT
    };
    egui::Frame::new().fill(fill).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (text, width) in cells.iter().zip(widths.iter().copied()) {
                let response = ui
                    .add_sized(
                        [width, 20.0],
                        egui::Label::new(text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(text);
                clicked |= response.clicked();
                double |= response.double_clicked();
            }
        });
    });
    (clicked, double)
}

pub(super) fn delete_tag(store: &Store, row: &ImageRow) -> Result<()> {
    store
        .remove_tag(&row.reference, &row.descriptor.digest)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
fn check_image(
    store: &Store,
    row: &ImageRow,
    name: Option<&str>,
) -> Result<pc_image::inspection::ImageDetails> {
    if let Some(reason) = &row.failure {
        return Err(reason.clone());
    }
    pc_image::desktop::check(store, &row.descriptor, name, || false).map_err(|e| e.to_string())
}

pub(super) fn host_last_run() -> String {
    // This timestamp is local catalogue metadata and never enters virtual time.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| "Unknown".into(), |d| last_run_utc(d.as_secs()))
}
pub(super) fn human_size(bytes: u64) -> String {
    for (unit, divisor) in [
        ("GiB", 1u64 << 30),
        ("MiB", 1u64 << 20),
        ("KiB", 1u64 << 10),
    ] {
        if bytes >= divisor {
            return format!("{:.1} {unit}", bytes as f64 / divisor as f64);
        }
    }
    format!("{bytes} B")
}
fn facet_list(
    ui: &mut egui::Ui,
    kind: &str,
    values: &BTreeSet<String>,
    current: &Filter,
    label: fn(&str, &str) -> String,
) -> (Option<Filter>, usize) {
    let values: Vec<_> = values.iter().collect();
    let mut selected = None;
    let mut built = 0;
    egui::ScrollArea::vertical()
        .id_salt(("facet", kind))
        .max_height(160.0)
        .show_rows(ui, 20.0, values.len(), |ui, range| {
            for index in range {
                built += 1;
                let value = values[index];
                let filter = Filter::Facet(kind.into(), value.clone());
                if ui
                    .selectable_label(current == &filter, label(kind, value))
                    .clicked()
                {
                    selected = Some(filter);
                }
            }
        });
    (selected, built)
}
fn last_run_utc(seconds: u64) -> String {
    let days = i128::from(seconds / 86400) + 719468;
    let era = days / 146097;
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = march_month + if march_month < 10 { 3 } else { -9 };
    year += i128::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        (seconds / 3600) % 24,
        (seconds / 60) % 60
    )
}

pub(super) struct LibraryView {
    pub root: Option<PathBuf>,
    worker: Option<Worker>,
    snapshot: Arc<Snapshot>,
    query: Query,
    pub selected: Option<ImageRow>,
    pub readiness: Option<Result<pc_image::inspection::ImageDetails>>,
    pub active: Option<(ImageRow, String, WorkspaceKind)>,
    pub layers: Option<LayersDialog>,
    pub busy: bool,
    pending_operation: bool,
    pub status: Option<String>,
    pub error: Option<String>,
    pub launched: Option<pc_image::workspace::SessionTransfer>,
    import: Option<(String, String)>,
    delete: bool,
    labels: String,
    pub rows_built: usize,
    pub recoveries: Vec<Recovery>,
    pub decision: Option<Recovery>,
    pub firmware_error: bool,
    pending_launch: Option<(Digest, WorkspaceKind)>,
    worker_dead: bool,
    /// Kept apart from `selected`: a package is never a Start target.
    firmware_selected: Option<firmware::FirmwareRow>,
    firmware_import: Option<firmware::ImportDialog>,
}
impl LibraryView {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            worker: None,
            snapshot: Arc::default(),
            query: Query::default(),
            selected: None,
            readiness: None,
            active: None,
            layers: None,
            busy: false,
            pending_operation: false,
            status: None,
            error: None,
            launched: None,
            import: None,
            delete: false,
            labels: String::new(),
            rows_built: 0,
            recoveries: Vec::new(),
            decision: None,
            firmware_error: false,
            pending_launch: None,
            worker_dead: false,
            firmware_selected: None,
            firmware_import: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn synthetic(root: PathBuf) -> Self {
        let mut view = Self::new(Some(root.clone()));
        view.worker = Some(Worker::synthetic(root));
        view
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.worker.is_none() && !self.worker_dead {
            if let Some(root) = self.root.clone() {
                self.worker = Some(Worker::spawn(root, ctx.clone()));
            }
        }
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(worker) = &self.worker {
            loop {
                match worker.receive.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        for event in events {
            match event {
                Event::Snapshot(snapshot) => {
                    if let Some(selected) = &mut self.selected {
                        if let Some(current) = snapshot.rows.iter().find(|r| {
                            r.image.reference == selected.reference
                                && r.image.descriptor.digest == selected.descriptor.digest
                        }) {
                            if current.image.summary.is_some() {
                                selected.summary = current.image.summary.clone();
                            }
                        }
                    }
                    if let Some(selected) = &mut self.firmware_selected {
                        if let Some(current) =
                            snapshot.firmware_rows.iter().find(|r| r.same(selected))
                        {
                            *selected = current.clone();
                        }
                    }
                    self.snapshot = snapshot;
                }
                Event::FirmwareScanned(report) => {
                    self.busy = false;
                    self.status = Some(if report.cancelled {
                        "ROM folder scan cancelled".into()
                    } else {
                        "ROM folder scanned".into()
                    });
                    if let Some(dialog) = &mut self.firmware_import {
                        dialog.scanned(*report);
                    }
                }
                Event::FirmwareFailed(error) => {
                    self.busy = false;
                    match &mut self.firmware_import {
                        Some(dialog) => dialog.failed(error),
                        None => self.error = Some(error),
                    }
                }
                Event::FirmwareImported(outcomes) => {
                    self.busy = false;
                    self.status = Some(
                        outcomes
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; "),
                    );
                    if let Some(dialog) = &mut self.firmware_import {
                        dialog.finished(outcomes);
                    }
                }
                Event::Checked(id, result) => {
                    if self
                        .selected
                        .as_ref()
                        .is_some_and(|r| r.descriptor.digest == id)
                    {
                        let ready = result.is_ok();
                        self.readiness = Some(result);
                        if self
                            .pending_launch
                            .as_ref()
                            .is_some_and(|(wanted, _)| wanted == &id)
                        {
                            let mode = self.pending_launch.take().map(|(_, mode)| mode);
                            if ready {
                                if let Some(mode) = mode {
                                    self.launch(mode);
                                }
                            }
                        }
                    }
                }
                Event::Launched(session, row, name, kind) => {
                    let row = *row;
                    self.selected = Some(row.clone());
                    self.active = Some((row, name, kind));
                    self.launched = Some(*session);
                    self.pending_operation = false;
                    self.busy = false;
                }
                Event::Finished => {
                    self.busy = self.pending_operation;
                    self.active = None;
                    if !self.pending_operation {
                        self.status = Some("Ready".into());
                    }
                }
                Event::Layers(name, result) => {
                    self.busy = false;
                    if let Some(dialog) = &mut self.layers {
                        dialog.set_workspace(name);
                        dialog.set_details(result);
                    }
                }
                Event::Published(row) => {
                    self.busy = false;
                    self.pending_operation = false;
                    self.status = Some(format!("Published {}", row.reference));
                    self.selected = Some(*row);
                    self.layers = None;
                    self.decision = None;
                    self.labels.clear();
                    self.recheck();
                }
                Event::Deleted(reference) => {
                    self.busy = false;
                    if self
                        .selected
                        .as_ref()
                        .is_some_and(|row| row.reference == reference)
                    {
                        self.selected = None;
                        self.readiness = None;
                    }
                    self.status = Some("Tag deleted; shared blobs retained".into());
                }
                Event::Done(message) => {
                    self.decision = None;
                    self.busy = false;
                    self.pending_operation = false;
                    self.status = Some(message);
                    if let Some(dialog) = &mut self.layers {
                        dialog.completed();
                    }
                    self.recheck();
                    if self.layers.is_some() {
                        self.open_layers();
                    }
                }
                Event::Recoveries(rows) => {
                    self.recoveries = rows;
                }
                Event::FirmwareError(error) => {
                    self.busy = false;
                    self.firmware_error = true;
                    self.error = Some(error);
                }
                Event::Error(error) => {
                    self.busy = false;
                    self.pending_operation = false;
                    self.firmware_error = false;
                    self.error = Some(error.clone());
                    if let Some(dialog) = &mut self.layers {
                        dialog.error = Some(error);
                    }
                }
            }
        }
        if disconnected && !self.worker_dead {
            self.busy = false;
            self.pending_operation = false;
            self.worker_dead = true;
            self.fail_firmware_import();
            if self.error.is_none() {
                self.error = Some(
                    "Image store worker stopped; choose the Image store folder to retry".into(),
                );
            }
        }
    }
    pub fn send(&mut self, request: Request) {
        let result = if let Some(worker) = &self.worker {
            worker
                .cancel
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            worker.send.send(request).is_ok()
        } else {
            false
        };
        if !result {
            self.busy = false;
            self.pending_operation = false;
            self.worker_dead = true;
            self.error =
                Some("Image store worker stopped; choose the Image store folder to retry".into());
            self.fail_firmware_import();
        }
    }
    pub fn shutdown(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown();
        }
        self.worker_dead = true;
    }
    pub fn ready(&self) -> bool {
        !self.busy && matches!(self.readiness, Some(Ok(_)))
    }
    pub fn launch(&mut self, kind: WorkspaceKind) {
        if self.busy {
            return;
        }
        if let Some(row) = self.selected.clone() {
            if self.readiness.is_none() {
                self.pending_launch = Some((row.descriptor.digest.clone(), kind));
                self.recheck();
                return;
            }
            if !self.ready() {
                self.error = self
                    .readiness
                    .as_ref()
                    .and_then(|r| r.as_ref().err().cloned());
                return;
            }
            self.busy = true;
            self.status = Some("Verifying image and opening workspace…".into());
            self.send(Request::Launch(row, kind, None));
        }
    }
    pub fn resume(&mut self, row: ImageRow, name: String, kind: WorkspaceKind) {
        self.pending_operation = true;
        self.busy = true;
        self.status = Some("Reopening image workspace…".into());
        self.send(Request::Launch(row, kind, Some(name)));
    }
    pub fn cancel_operation(&mut self) {
        self.pending_operation = false;
        self.busy = false;
    }
    pub fn finish_with_failure(&mut self, retain: bool, failed: bool) {
        if let Some((row, name, kind)) = self.active.clone() {
            if kind.disposable() && !retain {
                self.decision = Some(Recovery {
                    schema: 1,
                    kind,
                    reference: row.reference.clone(),
                    image: row.descriptor.clone(),
                    title: row.name().into(),
                    recovery: true,
                    name: name.clone(),
                });
            }
            self.busy = true;
            let outcome = if failed {
                Finish::Failed
            } else if retain || !kind.disposable() {
                Finish::Retain
            } else {
                Finish::Deferred
            };
            self.send(Request::Finish(outcome));
        }
    }
    pub fn needs_decision(&self) -> bool {
        self.decision.is_some()
    }
    fn recheck(&mut self) {
        if let Some(row) = self.selected.clone() {
            self.readiness = None;
            self.send(Request::Check(row.clone(), None));
        }
    }
    pub fn open_layers(&mut self) {
        let target = self.active.clone().or_else(|| {
            self.selected.clone().map(|row| {
                let name = regular_name(&row.descriptor.digest);
                (row, name, WorkspaceKind::Regular)
            })
        });
        self.layers = Some(LayersDialog::for_workspace(
            target.as_ref().map(|(_, name, _)| name.clone()),
            target
                .as_ref()
                .map_or(WorkspaceKind::Regular, |(_, _, kind)| *kind),
            target.as_ref().map_or("Machine", |(row, _, _)| row.name()),
        ));
        if let Some((row, name, _)) = target {
            self.busy = true;
            self.send(Request::Layers(row, name));
        }
    }
    pub fn begin_import(&mut self, path: &Path) {
        self.import = Some((path.to_string_lossy().into_owned(), String::new()));
    }
    pub fn begin_delete(&mut self) {
        self.delete = self.selected.is_some() && self.active.is_none();
    }
    pub fn has_dialog(&self) -> bool {
        self.layers.is_some()
            || self.import.is_some()
            || self.delete
            || self.decision.is_some()
            || self.firmware_import.is_some()
    }
    /// Close Disk layers, Import image, Delete tag and Import ROM folder for an
    /// exit; a running scan is cancelled. The Gaming keep-or-discard question
    /// stays: the exit waits for that decision.
    pub fn close_dialogs(&mut self) {
        self.layers = None;
        self.import = None;
        self.delete = false;
        if let Some(dialog) = self.firmware_import.take() {
            dialog.control.cancel();
        }
    }
    /// Entering the Firmware collection drops the machine selection, so Start
    /// never targets a machine that is no longer shown.
    fn set_filter(&mut self, filter: Filter, selected_file: &mut Option<usize>) {
        if filter.is_firmware() && !self.query.filter.is_firmware() {
            self.selected = None;
            self.readiness = None;
            self.pending_launch = None;
            *selected_file = None;
        }
        if !filter.is_firmware() {
            self.firmware_selected = None;
        }
        self.query.filter = filter;
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        files: &[LibraryEntry],
        selected_file: &mut Option<usize>,
        branding: Option<(&mut crate::logo::Logo, bool, bool)>,
    ) -> Vec<Command> {
        ui.add_enabled_ui(!self.busy, |ui| {
            self.show_contents(ui, files, selected_file, branding)
        })
        .inner
    }
    fn show_contents(
        &mut self,
        ui: &mut egui::Ui,
        files: &[LibraryEntry],
        selected_file: &mut Option<usize>,
        branding: Option<(&mut crate::logo::Logo, bool, bool)>,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let mut changed = false;
        egui::Panel::left("library-tree")
            .exact_size(230.0)
            .show(ui, |ui| {
                ui.strong("Library");
                let firmware_label = format!("Firmware ({})", self.snapshot.firmware_total);
                for (label, filter) in [
                    ("All images", Filter::All),
                    ("Recently played", Filter::Recent),
                    ("Favourites", Filter::Favorites),
                    ("Machine files", Filter::MachineFiles),
                    (firmware_label.as_str(), Filter::Firmware),
                ] {
                    if ui
                        .selectable_label(self.query.filter == filter, label)
                        .clicked()
                    {
                        self.set_filter(filter, selected_file);
                        changed = true;
                    }
                }
                let firmware_mode = self.query.filter.is_firmware();
                let kinds: &[&str] = if firmware_mode {
                    &firmware::FACETS
                } else {
                    &[
                        "Labels",
                        "Genre",
                        "Platform",
                        "Year",
                        "Developer",
                        "Publisher",
                        "Series",
                    ]
                };
                let label: fn(&str, &str) -> String = if firmware_mode {
                    firmware::facet_label
                } else {
                    |_, value| value.to_owned()
                };
                let snapshot = self.snapshot.clone();
                for kind in kinds {
                    let picked = egui::CollapsingHeader::new(*kind)
                        .default_open(firmware_mode && *kind != "Board")
                        .show(ui, |ui| {
                            snapshot.facets.get(*kind).and_then(|values| {
                                facet_list(ui, kind, values, &self.query.filter, label).0
                            })
                        })
                        .body_returned
                        .flatten();
                    if let Some(filter) = picked {
                        self.set_filter(filter, selected_file);
                        changed = true;
                    }
                }
                ui.separator();

                if let Some((logo, reduced, visible)) = branding {
                    ui.collapsing("unemupc98", |ui| logo.welcome(ui, reduced, visible));
                }
            });
        egui::Panel::bottom("library-details")
            .exact_size(232.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("selected-image-details")
                    .show(ui, |ui| {
                        if self.query.filter.is_firmware() {
                            match &self.firmware_selected {
                                Some(row) => firmware::details(ui, row),
                                None => {
                                    ui.label("Select a firmware package for details.");
                                }
                            }
                        } else if let Some(row) = self.selected.clone() {
                            ui.columns(3, |columns| {
                                egui::Frame::new()
                                    .fill(egui::Color32::from_rgb(28, 60, 116))
                                    .inner_margin(12.0)
                                    .show(&mut columns[0], |ui| {
                                        ui.set_min_height(120.0);
                                        ui.label(
                                            egui::RichText::new(row.name())
                                                .strong()
                                                .color(egui::Color32::WHITE),
                                        );
                                    });
                                columns[0].label(&row.reference);
                                columns[0].monospace(&row.descriptor.digest.hex()[..12]);
                                let meta = &mut columns[1];
                                meta.strong(row.name());
                                if let Some(summary) = &row.summary {
                                    if let Some(title) = summary.titles.first() {
                                        for (label, value) in [
                                            (
                                                "Platform",
                                                title
                                                    .platform
                                                    .clone()
                                                    .unwrap_or_else(|| summary.platform.clone()),
                                            ),
                                            ("Genre", title.genres.join(", ")),
                                            ("Developer", title.developers.join(", ")),
                                            ("Publisher", title.publishers.join(", ")),
                                            (
                                                "Year",
                                                title
                                                    .year
                                                    .map(|y| y.to_string())
                                                    .unwrap_or_default(),
                                            ),
                                            ("Series", title.series.clone().unwrap_or_default()),
                                        ] {
                                            if !value.is_empty() {
                                                meta.label(format!("{label}: {value}"));
                                            }
                                        }
                                    }
                                    meta.label(format!(
                                        "Stored: {}",
                                        human_size(summary.stored_size)
                                    ));
                                }
                                match &self.readiness {
                                    Some(Ok(details)) => {
                                        meta.label("Ready · firmware verified");
                                        for hardware in &details.hardware {
                                            meta.label(hardware);
                                        }
                                        for medium in details.media.iter().take(3) {
                                            let chain = medium
                                                .chain
                                                .iter()
                                                .map(|member| &member.digest.hex()[..8])
                                                .collect::<Vec<_>>()
                                                .join(" → ");
                                            meta.label(format!(
                                                "{}: {chain} → working",
                                                medium.drive
                                            ));
                                        }
                                        meta.label(format!(
                                            "Exclusive: {} · dedup savings: {}",
                                            human_size(details.exclusive_bytes),
                                            human_size(details.dedup_bytes)
                                        ));
                                    }
                                    Some(Err(reason)) => {
                                        meta.add(egui::Label::new(reason).wrap());
                                    }
                                    None => {
                                        meta.label("Checking image…");
                                    }
                                }
                                let actions = &mut columns[2];
                                actions.strong("Titles in this image");
                                if let Some(summary) = &row.summary {
                                    egui::ScrollArea::vertical()
                                        .id_salt("image-titles")
                                        .max_height(70.0)
                                        .show_rows(
                                            actions,
                                            20.0,
                                            summary.titles.len(),
                                            |ui, range| {
                                                for i in range {
                                                    ui.label(&summary.titles[i].title);
                                                }
                                            },
                                        );
                                    let mut favorite = summary.favorite;
                                    if actions.checkbox(&mut favorite, "Favourite").changed() {
                                        self.send(Request::Edit(
                                            row.descriptor.digest.clone(),
                                            favorite,
                                            summary.labels.clone(),
                                        ));
                                    }
                                    actions.label("Labels (comma separated)");
                                    actions.text_edit_singleline(&mut self.labels);
                                    if actions.button("Apply labels").clicked() {
                                        self.send(Request::Edit(
                                            row.descriptor.digest.clone(),
                                            favorite,
                                            self.labels
                                                .split(',')
                                                .map(str::trim)
                                                .filter(|s| !s.is_empty())
                                                .map(str::to_owned)
                                                .collect(),
                                        ));
                                    }
                                }
                                actions.horizontal_wrapped(|ui| {
                                    if ui
                                        .add_enabled(self.ready(), egui::Button::new("Start"))
                                        .clicked()
                                    {
                                        commands.push(Command::Start);
                                    }
                                    ui.add_enabled(false, egui::Button::new("Settings…"))
                                        .on_hover_text(
                                            "Image settings editing is not available yet.",
                                        );
                                    if ui.button("Layers…").clicked() {
                                        commands.push(Command::Layers);
                                    }
                                });
                            });
                        } else if let Some(entry) = selected_file.and_then(|i| files.get(i)) {
                            ui.columns(2, |columns| {
                                columns[0].strong(&entry.name);
                                for (name, value) in &entry.details {
                                    columns[0].label(format!("{name}: {value}"));
                                }
                                columns[0].label(&entry.ems);
                                if let Some(warning) = &entry.warning {
                                    columns[1].add(egui::Label::new(warning).wrap());
                                    columns[1].label(entry.hint());
                                }
                                columns[1].horizontal_wrapped(|ui| {
                                    if ui
                                        .add_enabled(
                                            entry.checked && entry.warning.is_none(),
                                            egui::Button::new("Start"),
                                        )
                                        .clicked()
                                    {
                                        commands.push(Command::Start);
                                    }
                                    if ui.button("Settings…").clicked() {
                                        commands.push(Command::Settings);
                                    }
                                    if ui.button("Layers…").clicked() {
                                        commands.push(Command::Layers);
                                    }
                                    if entry.hint().contains("Create blank disk")
                                        && ui.button("Create blank disk…").clicked()
                                    {
                                        commands.push(Command::CreateBlank);
                                    }
                                });
                            });
                        } else {
                            ui.label("Select an image tag or machine file for details.");
                        }
                    });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            let firmware_mode = self.query.filter.is_firmware();
            ui.horizontal(|ui| {
                if firmware_mode
                    && ui
                        .add_enabled(
                            self.can_import_firmware(),
                            egui::Button::new("Import ROM folder…"),
                        )
                        .clicked()
                {
                    commands.push(Command::ImportFirmware);
                }
                ui.label("Search:");
                if ui.text_edit_singleline(&mut self.query.text).changed() {
                    changed = true;
                }
            });
            if firmware_mode {
                if let Some(warning) = &self.snapshot.warning {
                    ui.label(warning);
                }
                changed |= self.firmware_list(ui, selected_file);
                ui.label(format!(
                    "{} of {} packages · {} pending",
                    self.snapshot.matched, self.snapshot.firmware_total, self.snapshot.pending
                ));
                return;
            }
            if !self.recoveries.is_empty() {
                ui.strong("Unfinished sessions");
                let recoveries = self.recoveries.clone();
                for record in recoveries {
                    ui.horizontal(|ui| {
                        ui.label(&record.title);
                        if ui.button("Keep as new image…").clicked() {
                            *selected_file = None;
                            self.selected = Some(record.row());
                            self.layers = Some(LayersDialog::for_workspace(
                                Some(record.name.clone()),
                                record.kind,
                                &record.title,
                            ));
                            if let Some(dialog) = &mut self.layers {
                                dialog.begin_keep();
                            }
                            self.busy = true;
                            self.send(Request::Layers(record.row(), record.name.clone()));
                        }
                        if ui.button("Discard…").clicked() {
                            *selected_file = None;
                            self.selected = Some(record.row());
                            self.layers = Some(LayersDialog::for_workspace(
                                Some(record.name.clone()),
                                record.kind,
                                &record.title,
                            ));
                            if let Some(dialog) = &mut self.layers {
                                dialog.begin_discard();
                            }
                            self.busy = true;
                            self.send(Request::Layers(record.row(), record.name.clone()));
                        }
                    });
                }
            }
            if let Some(warning) = &self.snapshot.warning {
                ui.label(warning);
            }
            let visible_files: Vec<_> =
                if matches!(self.query.filter, Filter::All | Filter::MachineFiles) {
                    let search = self.query.text.to_lowercase();
                    let mut visible: Vec<_> = files
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| {
                            format!("{} {}", f.name, f.path.display())
                                .to_lowercase()
                                .contains(&search)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    visible.sort_by_key(|i| files[*i].name.to_lowercase());
                    if self.query.descending {
                        visible.reverse();
                    }
                    visible
                } else {
                    Vec::new()
                };
            self.rows_built = 0;
            egui::ScrollArea::horizontal()
                .id_salt("library-columns")
                .show(ui, |ui| {
                    ui.set_min_width(COLUMN_WIDTHS.iter().sum::<f32>() + 64.0);
                    changed |= sort_header(
                        ui,
                        &[
                            "Name",
                            "Tag",
                            "Platform",
                            "Genre",
                            "Year",
                            "Developer",
                            "Last run",
                            "Size",
                            "Labels",
                        ],
                        &COLUMN_WIDTHS,
                        &mut self.query.sort,
                        &mut self.query.descending,
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("image-list")
                        .max_height((ui.available_height() - 26.0).max(80.0))
                        .show_rows(
                            ui,
                            20.0,
                            visible_files.len() + self.snapshot.rows.len(),
                            |ui, range| {
                                for index in range {
                                    self.rows_built += 1;
                                    if let Some(&i) = visible_files.get(index) {
                                        let entry = &files[i];
                                        let cells = [
                                            format!(
                                                "{}{}",
                                                if entry.has_problem() { "[!] " } else { "" },
                                                entry.name
                                            ),
                                            "Machine file".into(),
                                            String::new(),
                                            String::new(),
                                            String::new(),
                                            String::new(),
                                            "Never".into(),
                                            String::new(),
                                            String::new(),
                                        ];
                                        let (clicked, double) = draw_cells(
                                            ui,
                                            &cells,
                                            &COLUMN_WIDTHS,
                                            self.selected.is_none() && *selected_file == Some(i),
                                            index % 2 == 0,
                                        );
                                        if clicked || double {
                                            self.selected = None;
                                            self.readiness = None;
                                            *selected_file = Some(i);
                                        }
                                        if double {
                                            commands.push(Command::Start);
                                        }
                                    } else if let Some(row) =
                                        self.snapshot.rows.get(index - visible_files.len())
                                    {
                                        if let Some(count) = row.group {
                                            let repo = row.image.repository().to_owned();
                                            let mut cells = row_cells(row);
                                            cells[0] = format!(
                                                "{} {} ({count} tags)",
                                                if self.query.expanded.contains(&repo) {
                                                    "−"
                                                } else {
                                                    "+"
                                                },
                                                row.image.name()
                                            );
                                            if draw_cells(
                                                ui,
                                                &cells,
                                                &COLUMN_WIDTHS,
                                                false,
                                                index % 2 == 0,
                                            )
                                            .0
                                            {
                                                if !self.query.expanded.remove(&repo) {
                                                    self.query.expanded.insert(repo);
                                                }
                                                changed = true;
                                            }
                                        } else {
                                            let image = &row.image;
                                            let summary = image.summary.as_ref();
                                            let cells = row_cells(row);
                                            let selected =
                                                self.selected.as_ref().is_some_and(|r| {
                                                    r.reference == image.reference
                                                        && r.descriptor.digest
                                                            == image.descriptor.digest
                                                });
                                            let (clicked, double) = draw_cells(
                                                ui,
                                                &cells,
                                                &COLUMN_WIDTHS,
                                                selected,
                                                index % 2 == 0,
                                            );
                                            if (clicked || double) && !selected {
                                                self.pending_launch = None;
                                                self.selected = Some(image.clone());
                                                *selected_file = None;
                                                self.labels = summary
                                                    .map(|s| s.labels.join(", "))
                                                    .unwrap_or_default();
                                                self.readiness = image.failure.clone().map(Err);
                                                if self.readiness.is_none() {
                                                    self.recheck();
                                                }
                                            }
                                            if double {
                                                commands.push(Command::Start);
                                            }
                                        }
                                    }
                                }
                            },
                        );
                });
            ui.label(format!(
                "{} of {} images · {} pending · {}",
                self.snapshot.matched,
                self.snapshot.total,
                self.snapshot.pending,
                self.snapshot
                    .storage
                    .map(|(total, saved)| format!(
                        "{} distinct · {} dedup savings",
                        human_size(total),
                        human_size(saved)
                    ))
                    .unwrap_or_else(|| "Rebuilding storage totals…".into())
            ));
        });
        if changed {
            self.send(Request::Query(self.query.clone()));
        }
        commands
    }
    pub fn modals(
        &mut self,
        ctx: &egui::Context,
        selected_path: Option<&Path>,
        roots: Vec<PathBuf>,
        classic: bool,
        running: bool,
    ) -> Vec<Request> {
        let mut requests = Vec::new();
        if let Some((path, tag)) = &mut self.import {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("import-image")).show(ctx, |ui| {
                if classic { crate::shell::gradient_title(ui, "Import image"); } else { ui.heading("Import image"); } ui.label("Machine file"); ui.text_edit_singleline(path); ui.label("New repository:tag"); ui.text_edit_singleline(tag);
                ui.label("Firmware is located using the configured ROM folders. Missing firmware: Tools > Locate firmware…");
                ui.horizontal(|ui| { if ui.add_enabled(!self.busy && !tag.trim().is_empty(), egui::Button::new("Import")).clicked() { requests.push(Request::Import(PathBuf::from(path.trim()), tag.trim().into(), roots)); self.busy = true; self.status = Some("Importing machine image…".into()); close = true; }
                    if ui.button("Cancel").clicked() { close = true; } });
            });
            if close {
                self.import = None;
            }
        }
        if self.delete {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("delete-tag")).show(ctx, |ui| {
                if classic {
                    crate::shell::gradient_title(ui, "Delete tag?");
                } else {
                    ui.heading("Delete tag?");
                }
                if let Some(row) = &self.selected {
                    ui.label(&row.reference);
                    ui.label(
                        "Only this tag is removed. Shared bases and layers stay in the store.",
                    );
                    if ui.button("Delete tag").clicked() {
                        requests.push(Request::Delete(row.clone()));
                        self.busy = true;
                        close = true;
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
            if close {
                self.delete = false;
            }
        }
        if self.layers.is_none() {
            if let Some(record) = self.decision.clone() {
                let mut keep = false;
                let mut discard = false;
                egui::Modal::new(egui::Id::unique("gaming-stop")).show(ctx,|ui| {
                    ui.heading("Keep this gaming session?");ui.label(&record.title);
                    ui.label("Keep as a new image, or discard the isolated changes. The regular workspace is unchanged.");
                    ui.horizontal(|ui| {
                        keep=ui.add_enabled(!self.busy,egui::Button::new("Keep as new image…")).clicked();
                        discard=ui.add_enabled(!self.busy,egui::Button::new("Discard")).clicked();
                    });
                });
                if keep {
                    self.selected = Some(record.row());
                    let mut dialog = LayersDialog::for_workspace(
                        Some(record.name.clone()),
                        record.kind,
                        &record.title,
                    );
                    dialog.begin_keep();
                    self.layers = Some(dialog);
                    self.busy = true;
                    requests.push(Request::Layers(record.row(), record.name.clone()));
                }
                if discard {
                    self.busy = true;
                    requests.push(Request::Discard(record.name));
                }
            }
        }
        if let Some(dialog) = &mut self.layers {
            let action = dialog.show(ctx, self.busy, classic);
            match action {
                crate::layers_dialog::Action::Close => self.layers = None,
                crate::layers_dialog::Action::Import => {
                    self.layers = None;
                    if let Some(path) = selected_path.filter(|_| !running) {
                        self.begin_import(path);
                    }
                }
                crate::layers_dialog::Action::Publish(name, tag, flatten) => {
                    self.pending_operation = true;
                    self.busy = true;
                    self.status = Some("Publishing whole machine…".into());
                    if let Some(row) = self
                        .active
                        .as_ref()
                        .map(|(r, _, _)| r)
                        .or(self.selected.as_ref())
                    {
                        requests.push(Request::Publish(row.clone(), name, tag, flatten));
                    }
                }
                crate::layers_dialog::Action::Discard(name) => {
                    self.pending_operation = true;
                    self.busy = true;
                    self.status = Some("Discarding workspace changes…".into());
                    requests.push(Request::Discard(name));
                }
                crate::layers_dialog::Action::None => (),
            }
        }
        if let Some(dialog) = &mut self.firmware_import {
            match dialog.show(ctx, classic) {
                firmware::DialogAction::Import => requests.extend(self.import_picks()),
                firmware::DialogAction::Close => {
                    if let Some(dialog) = self.firmware_import.take() {
                        dialog.control.cancel();
                    }
                }
                firmware::DialogAction::None => (),
            }
        }
        requests
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use image_store::{
        media_type as mt,
        metadata::{Metadata, Title},
        BlobSource, ConfigValidator, Manifest,
    };

    struct Provider;
    impl ConfigValidator for Provider {
        fn platform(&self) -> &str {
            "synthetic"
        }
        fn validate(
            &self,
            _: &toml::Table,
            descriptors: &[Descriptor],
            _: &dyn BlobSource,
        ) -> image_store::Result<Vec<Digest>> {
            Ok(descriptors
                .iter()
                .filter(|d| d.media_type == mt::DISK_BASE)
                .map(|d| d.digest.clone())
                .collect())
        }
        fn payload_types(&self) -> &[image_store::PayloadType] {
            pc_image::PAYLOAD_TYPES
        }
    }
    pub(crate) fn catalogue(
        count: usize,
    ) -> (assembly::test_support::TempDir, Store, Vec<Descriptor>) {
        let temp = assembly::test_support::TempDir::new("desktop-library");
        let store = Store::open(temp.path().join("store")).unwrap();
        let blob = store
            .import(&b"schema=1\nplatform='synthetic'\n"[..], None)
            .unwrap();
        let config = Descriptor::new(mt::CONFIG, blob.digest, blob.size);
        let directory = store.root().join("images/blobs/sha256");
        let mut images = Vec::new();
        let mut index = Vec::new();
        for i in 0..count {
            let mut metadata = Metadata::new("synthetic");
            metadata.titles = vec![Title {
                id: "one".into(),
                title: format!("Synthetic {i}"),
                genres: vec!["Adventure".into()],
                platform: Some("DOS".into()),
                release_year: Some(1991),
                developers: vec!["Example studio".into()],
                ..Default::default()
            }];
            let bytes = metadata.encode(&[]).unwrap();
            let digest = Digest::of(&bytes);
            std::fs::write(directory.join(digest.hex()), &bytes).unwrap();
            let meta = Descriptor::new(mt::METADATA, digest, bytes.len() as u64);
            let manifest = Manifest::new("synthetic", config.clone(), vec![meta]).unwrap();
            let bytes = manifest.encode().unwrap();
            let digest = Digest::of(&bytes);
            std::fs::write(directory.join(digest.hex()), &bytes).unwrap();
            images.push(Descriptor::new(
                mt::MANIFEST,
                digest.clone(),
                bytes.len() as u64,
            ));
            index.push(format!("{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"synthetic-{i}:one\"}}}}", mt::MANIFEST, digest, bytes.len()));
        }
        std::fs::write(
            store.root().join("images/index.json"),
            format!(
                "{{\"schemaVersion\":2,\"mediaType\":\"{}\",\"manifests\":[{}]}}",
                mt::INDEX,
                index.join(",")
            ),
        )
        .unwrap();
        (temp, store, images)
    }
    fn warm(store: &Store) -> SummaryCache {
        let mut cache = SummaryCache::open(store).unwrap();
        while !cache.pending().is_empty() {
            assert!(cache.rebuild(store, REBUILD_BATCH, &[], &[]).unwrap() <= REBUILD_BATCH);
        }
        cache
    }
    #[test]
    fn thousand_images_use_no_manifests_on_warm_start_or_facet_search() {
        let (_temp, store, _) = catalogue(1000);
        warm(&store);
        std::fs::rename(
            store.root().join("images/blobs"),
            store.root().join("unavailable-blobs"),
        )
        .unwrap();
        let cache = SummaryCache::open(&store).unwrap();
        assert!(cache.pending().is_empty());
        let query = Query {
            text: "synthetic 999".into(),
            filter: Filter::Facet("Genre".into(), "Adventure".into()),
            ..Default::default()
        };
        let result = snapshot(&cache, &query);
        assert_eq!(result.total, 1000);
        assert_eq!(result.matched, 1);
        assert_eq!(result.rows[0].image.reference, "synthetic-999:one");
        assert_eq!(result.facets["Platform"], BTreeSet::from(["DOS".into()]));
    }
    #[test]
    fn thousand_images_build_only_visible_rows_per_frame() {
        let (_temp, store, _) = catalogue(1000);
        let cache = warm(&store);
        let mut library = LibraryView::new(None);
        library.snapshot = Arc::new(snapshot(&cache, &Query::default()));
        let ctx = egui::Context::default();
        let mut selected = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                library.show(ui, &[], &mut selected, None);
            },
        );
        output.textures_delta.clear();
        assert!(library.rows_built > 0);
        assert!(library.rows_built < 40, "built {} rows", library.rows_built);
        for direction in [-1.0, 1.0] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 600.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(egui::pos2(500.0, 250.0)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Unknown,
                            delta: egui::vec2(0.0, direction * 12000.0),
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    library.show(ui, &[], &mut selected, None);
                },
            );
            output.textures_delta.clear();
            assert!(
                library.rows_built > 0 && library.rows_built < 40,
                "built {} rows while scrolling",
                library.rows_built
            );
        }
    }
    #[test]
    fn repository_groups_expand_and_overlay_fields_filter_without_payload_reads() {
        let (_temp, store, images) = catalogue(2);
        store
            .update_tag("synthetic-0:two", &images[0], None, &Provider)
            .unwrap();
        store
            .edit_library(&images[0].digest, |entry| {
                entry.favorite = true;
                entry.labels = vec!["collection".into()];
                entry.last_run = Some("123".into());
                Ok(())
            })
            .unwrap();
        let cache = warm(&store);
        let mut query = Query {
            filter: Filter::Favorites,
            ..Default::default()
        };
        let result = snapshot(&cache, &query);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].group, Some(2));
        query.expanded.insert("synthetic-0".into());
        assert_eq!(snapshot(&cache, &query).rows.len(), 3);
        query.filter = Filter::Facet("Labels".into(), "collection".into());
        assert_eq!(snapshot(&cache, &query).matched, 2);
        query.filter = Filter::Recent;
        assert_eq!(snapshot(&cache, &query).matched, 2);
        query.filter = Filter::All;
        query.sort = 1;
        query.descending = true;
        assert_eq!(
            snapshot(&cache, &query).rows[0].image.reference,
            "synthetic-1:one"
        );
    }
    #[test]
    fn deleting_one_tag_keeps_shared_bases_and_other_tags() {
        let (_temp, store, images) = catalogue(1);
        let blob = store.import(&[0u8; 512][..], None).unwrap();
        let original = store.resolve("synthetic-0:one", &Provider).unwrap();
        let mut layers = original.manifest.layers;
        layers.insert(
            0,
            Descriptor::new(mt::DISK_BASE, blob.digest.clone(), blob.size),
        );
        let mut manifest = Manifest::new("synthetic", original.manifest.config, layers).unwrap();
        let image = store
            .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
            .unwrap();
        store
            .update_tag(
                "synthetic-0:one",
                &image,
                Some(&images[0].digest),
                &Provider,
            )
            .unwrap();
        manifest.annotations.insert(
            "org.opencontainers.image.title".into(),
            "Another image sharing the base".into(),
        );
        let shared = store
            .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
            .unwrap();
        assert_ne!(shared.digest, image.digest);
        store
            .update_tag("shared:two", &shared, None, &Provider)
            .unwrap();
        delete_tag(
            &store,
            &ImageRow {
                reference: "synthetic-0:one".into(),
                descriptor: image.clone(),
                summary: None,
                failure: None,
            },
        )
        .unwrap();
        assert!(store.tag_descriptor("synthetic-0:one").is_err());
        assert_eq!(
            store.tag_descriptor("shared:two").unwrap().digest,
            shared.digest
        );
        store.resolve("shared:two", &Provider).unwrap();
        store.verify(&blob.digest, blob.size).unwrap();
    }
    #[test]
    fn pinned_preparation_uses_the_selected_digest_after_a_tag_moves() {
        let (_temp, store, images) = catalogue(2);
        store
            .update_tag(
                "synthetic-0:one",
                &images[1],
                Some(&images[0].digest),
                &Provider,
            )
            .unwrap();
        std::fs::remove_file(
            store
                .root()
                .join("images/blobs/sha256")
                .join(images[0].digest.hex()),
        )
        .unwrap();
        let owner = store.workspace("pinned-selection").unwrap();
        let error = pc_image::workspace::prepare_pinned(&owner, &images[0], false)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(&images[0].digest.to_string()), "{error}");
        assert!(!error.contains(&images[1].digest.to_string()), "{error}");
        assert!(owner.read("session.toml").unwrap().is_none());
    }
    #[test]
    fn readiness_blocks_unsupported_platform_missing_blob_and_new_schema() {
        let (_temp, store, images) = catalogue(1);
        let cache = warm(&store);
        let row = snapshot(&cache, &Query::default()).rows[0].image.clone();
        assert!(check_image(&store, &row, None).is_err());
        let future_config = store
            .import(&b"schema=99\nplatform='ibm-pc-at'\n[machine]\n"[..], None)
            .unwrap();
        let metadata = Metadata::new("ibm-pc-at").encode(&[]).unwrap();
        let metadata = store.import(metadata.as_slice(), None).unwrap();
        let manifest = Manifest::new(
            "ibm-pc-at",
            Descriptor::new(mt::CONFIG, future_config.digest, future_config.size),
            vec![Descriptor::new(
                mt::METADATA,
                metadata.digest,
                metadata.size,
            )],
        )
        .unwrap();
        let future = ImageRow {
            reference: "future:one".into(),
            descriptor: store.import_manifest(&manifest).unwrap(),
            summary: None,
            failure: None,
        };
        let reason = check_image(&store, &future, None).unwrap_err();
        assert!(reason.contains("version or platform mismatch"), "{reason}");
        std::fs::remove_file(
            store
                .root()
                .join("images/blobs/sha256")
                .join(images[0].digest.hex()),
        )
        .unwrap();
        assert!(check_image(&store, &row, None)
            .unwrap_err()
            .contains("missing"));
    }
    #[test]
    fn verified_launch_handles_transfer_without_payload_reads_and_detect_changes() {
        let temp = assembly::test_support::TempDir::new("desktop-launch-transfer");
        let path = temp.path().join("disk.img");
        std::fs::write(&path, [0u8; 512]).unwrap();
        let make = || pc_image::workspace::PreparedSession {
            machine: assembly::bios(vec![0xf4; 32768], None),
            image: Digest::of(b"synthetic image"),
            bases: BTreeMap::from([(path.clone(), media::VerifiedBase::open(&path).unwrap())]),
        };
        let prepared = make();
        let before = media::data_passes(512);
        let transfer = prepared.into_transfer().unwrap();
        assert_eq!(media::data_passes(512), before);
        assert!(std::thread::spawn(move || {
            let before = media::data_passes(512);
            let ready = transfer.restore().is_ok();
            assert_eq!(media::data_passes(512), before);
            ready
        })
        .join()
        .unwrap());
        let transfer = make().into_transfer().unwrap();
        std::fs::write(&path, [1u8; 1024]).unwrap();
        assert!(transfer.restore().is_err());
    }
    #[test]
    fn verified_transfer_uses_original_handle_after_same_length_and_mtime_inode_swap() {
        use media::SectorSource;
        let temp = assembly::test_support::TempDir::new("verified-inode-swap");
        let dir = temp.path().join("current");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("base.img");
        std::fs::write(&path, [7; 512]).unwrap();
        let base = media::VerifiedBase::open(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::rename(&dir, temp.path().join("original")).unwrap();
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(&path, [9; 512]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let prepared = pc_image::workspace::PreparedSession {
            machine: assembly::bios(vec![0xf4; 32768], None),
            image: Digest::of(b"synthetic"),
            bases: BTreeMap::from([(path.clone(), base)]),
        };
        let restored = prepared.into_transfer().unwrap().restore().unwrap();
        assert_eq!(restored.bases[&path].read_sector(0).unwrap(), [7; 512]);
        assert_eq!(std::fs::read(path).unwrap(), [9; 512]);
    }
    #[test]
    fn published_regular_workspace_is_found_by_its_new_pin() {
        let (_temp, store, images) = catalogue(2);
        let name = regular_name(&images[0].digest);
        let owner = store.workspace(&name).unwrap();
        let session = format!(
            "schema=1\nmachine='{}'\n[image]\n{}",
            Digest::of(b"machine"),
            toml::to_string(&images[1]).unwrap()
        );
        owner.write("session.toml", session.as_bytes()).unwrap();
        assert_eq!(regular_workspace(&store, &images[1].digest).unwrap(), name);
        assert_ne!(regular_workspace(&store, &images[0].digest).unwrap(), name);
    }
    #[test]
    fn platform_default_store_paths_and_preference_round_trip() {
        assert_eq!(last_run_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(last_run_utc(951782400), "2000-02-29 00:00 UTC");
        assert_eq!(
            default_store_path().unwrap(),
            image_store::location::default_store_root().unwrap()
        );
        let mut preferences = crate::preferences::Preferences::default();
        preferences.store_root = Some("/synthetic-store".into());
        let restored: crate::preferences::Preferences =
            toml::from_str(&toml::to_string(&preferences).unwrap()).unwrap();
        assert_eq!(restored, preferences);
    }
    #[test]
    fn groups_are_ordered_by_their_first_sorted_child() {
        let (_temp, store, images) = catalogue(10);
        for (index, image) in images.iter().enumerate() {
            delete_tag(
                &store,
                &ImageRow {
                    reference: format!("synthetic-{index}:one"),
                    descriptor: image.clone(),
                    summary: None,
                    failure: None,
                },
            )
            .unwrap();
        }
        for (tag, index) in [("alpha:first", 0), ("alpha:last", 9), ("middle:only", 5)] {
            store
                .update_tag(tag, &images[index], None, &Provider)
                .unwrap();
        }
        let cache = warm(&store);
        let result = snapshot(
            &cache,
            &Query {
                descending: true,
                ..Default::default()
            },
        );
        assert_eq!(result.rows[0].image.reference, "alpha:last");
    }
    #[test]
    fn thousand_facet_values_build_only_visible_rows() {
        let ctx = egui::Context::default();
        let values = (0..1000).map(|i| format!("Label {i}")).collect();
        let mut built = 0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            built = facet_list(ui, "Labels", &values, &Filter::All, |_, v| v.into()).1;
        });
        output.textures_delta.clear();
        assert!(built > 0 && built < 20, "built {built}");
    }
    #[test]
    fn failed_store_open_and_dead_sends_clear_busy_and_allow_folder_replacement() {
        let temp = assembly::test_support::TempDir::new("bad-library-root");
        let file = temp.path().join("file");
        std::fs::write(&file, b"not a directory").unwrap();
        let mut view = LibraryView::new(Some(file));
        view.busy = true;
        let ctx = egui::Context::default();
        for _ in 0..500 {
            view.poll(&ctx);
            if view.worker_dead {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(view.worker_dead);
        assert!(!view.busy);
        assert!(view.error.take().is_some());
        view.poll(&ctx);
        assert!(
            view.error.is_none(),
            "a dead worker must not reopen the error dialog every frame"
        );
        view.busy = true;
        view.send(Request::Query(Query::default()));
        assert!(!view.busy);
        view.shutdown();
        let mut replacement = LibraryView::new(Some(temp.path().join("valid")));
        replacement.poll(&ctx);
        for _ in 0..500 {
            replacement.poll(&ctx);
            if replacement.snapshot.storage.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!replacement.worker_dead);
        replacement.shutdown();
    }
    #[test]
    fn warm_storage_totals_count_shared_blobs_once_and_show_savings() {
        let (_temp, store, _) = catalogue(2);
        let cache = warm(&store);
        let nominal: u64 = cache
            .tags()
            .iter()
            .map(|tag| cache.summary(&tag.manifest.digest).unwrap().stored_size)
            .sum();
        let (distinct, saved) = cache.storage_totals().unwrap();
        assert!(saved > 0);
        assert_eq!(distinct + saved, nominal);
        std::fs::rename(
            store.root().join("images/blobs"),
            store.root().join("hidden-blobs"),
        )
        .unwrap();
        assert_eq!(
            snapshot(&cache, &Query::default()).storage,
            Some((distinct, saved))
        );
    }
    #[test]
    fn media_sets_stay_out_of_machine_launch_rows() {
        let (_temp, store, images) = catalogue(1);
        let machine = Manifest::decode_for_summary(
            &store
                .read_verified(&images[0], image_store::MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        let config = Descriptor::new(
            mt::MEDIA_SET_CONFIG,
            machine.config.digest.clone(),
            machine.config.size,
        );
        let set = Manifest::new_media_set("synthetic", config, machine.layers).unwrap();
        let set = store.import_manifest(&set).unwrap();
        let index = store.root().join("images/index.json");
        let text = std::fs::read_to_string(&index).unwrap();
        let prefix = text.trim_end().strip_suffix("]}").unwrap();
        std::fs::write(&index,format!("{prefix},{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"installation:one\"}}}}]}}",mt::MANIFEST,set.digest,set.size)).unwrap();
        let cache = warm(&store);
        let summary = cache.summary(&set.digest).unwrap();
        assert_eq!(summary.kind, image_store::summary::ArtifactKind::MediaSet);
        assert!(summary.machine.is_none());
        assert_eq!(summary.config, machine.config.digest);
        assert!(!summary.storage_blobs.is_empty());
        std::fs::rename(
            store.root().join("images/blobs"),
            store.root().join("hidden-blobs"),
        )
        .unwrap();
        let result = snapshot(&cache, &Query::default());
        assert_eq!(result.total, 1);
        assert_eq!(result.matched, 1);
        assert_eq!(result.rows[0].image.descriptor.digest, images[0].digest);
    }
    #[test]
    fn firmware_packages_stay_out_of_machine_launch_rows() {
        let (temp, store, images) = catalogue(1);
        let bytes = vec![0xf4; 32768];
        let source = temp.path().join("synthetic.bin");
        std::fs::write(&source, &bytes).unwrap();
        let digest = Digest::of(&bytes);
        let config = image_store::firmware::Config::decode(format!("schema=1\nplatform='ibm-pc-at'\nname='Synthetic firmware'\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=32768\nsha256='{}'\nblob='{}'\n", digest.hex(), digest).as_bytes()).unwrap();
        let candidate = pc_image::firmware_import::Candidate {
            catalogue_id: "synthetic".into(),
            config,
            sources: vec![source],
            distribution: assembly::FirmwareDistribution::UserSupplied,
        };
        let package = pc_image::firmware_import::publish(
            &store,
            &candidate,
            &[],
            pc_image::firmware::catalogue(),
        )
        .unwrap();
        let cache = warm(&store);
        let summary = cache.summary(&package.digest).unwrap();
        assert_eq!(summary.kind, ArtifactKind::Firmware);
        assert!(!summary.storage_blobs.is_empty());
        assert!(cache.pending().is_empty());
        let result = snapshot(&cache, &Query::default());
        assert_eq!(result.total, 1);
        assert_eq!(result.matched, 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].image.descriptor.digest, images[0].digest);
    }
}
