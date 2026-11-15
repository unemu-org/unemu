// SPDX-License-Identifier: AGPL-3.0-or-later
//! Explorer-style local image library; machine-images §§2.1, 3 and 7.
//! Store jobs own I/O and PC readiness, launch and publication adapters.
//! The tree, facets and virtualized rows consume platform-neutral summaries.
//! Firmware packages (§5.3) and installation media sets (§5.2) form their own
//! collections, see [`firmware`] and [`install_media`]; a firmware package's
//! metadata and licences are edited with [`firmware_metadata`].

pub(crate) mod firmware;
pub(crate) mod firmware_metadata;
pub(crate) mod install_media;

use crate::{
    layers_dialog::LayersDialog,
    shell_icons::Icon,
    shell_model::{Command, LibraryEntry},
};
use image_store::{
    summary::{ArtifactKind, Facets, ImageSummary, SummaryCache},
    Descriptor, Digest, Store, Tag,
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
    Media,
    /// A facet of the Installation media collection; its kinds share names
    /// with machine facets, so the collection is part of the filter.
    MediaFacet(String, String),
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
    /// The Installation media collection and its facets list sets instead of machines.
    fn is_media(&self) -> bool {
        matches!(self, Self::Media | Self::MediaFacet(_, _))
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
    /// So does the Installation media list.
    media_sort: usize,
    media_descending: bool,
}
#[derive(Clone, Debug)]
pub(super) struct ImageRow {
    /// The tag every action on the row uses.
    pub reference: String,
    /// The other tags of this repository that name the same manifest. They
    /// share the row, so a version tag and its `latest` alias are listed once.
    pub aliases: Vec<String>,
    pub descriptor: Descriptor,
    pub summary: Option<ImageSummary>,
    pub failure: Option<String>,
}
impl ImageRow {
    /// Rows are one per manifest of a repository, whichever tag leads them.
    pub fn same(&self, other: &Self) -> bool {
        self.repository() == other.repository() && self.descriptor.digest == other.descriptor.digest
    }
    pub fn has_tag(&self, reference: &str) -> bool {
        self.reference == reference || self.aliases.iter().any(|a| a == reference)
    }
    /// Every tag of the row, the acting one first: "1.1.0, latest".
    pub fn tags(&self) -> String {
        tags(&self.reference, &self.aliases)
    }
    /// The repository with every tag of the row: "open-at:1.1.0, latest".
    pub fn references(&self) -> String {
        format!("{}:{}", self.repository(), self.tags())
    }
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
fn tags(reference: &str, aliases: &[String]) -> String {
    std::iter::once(reference)
        .chain(aliases.iter().map(String::as_str))
        .map(|r| r.rsplit_once(':').map_or(r, |(_, tag)| tag))
        .collect::<Vec<_>>()
        .join(", ")
}
/// One manifest of one repository as the library lists it, with every tag
/// there that names it.
pub(super) struct Listed<'a> {
    pub tag: &'a Tag,
    pub aliases: Vec<String>,
}
impl Listed<'_> {
    fn names(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.tag.name).chain(&self.aliases)
    }
    /// The summary search matched one of the row's tags.
    pub fn matched(&self, matches: &BTreeSet<String>) -> bool {
        self.names().any(|name| matches.contains(name))
    }
    /// Before its summary exists a row matches the search by its tag names.
    pub fn named(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.names().any(|name| name.to_lowercase().contains(&text))
    }
    fn references(&self) -> String {
        let name = &self.tag.name;
        let repository = name.rsplit_once(':').map_or(name.as_str(), |(r, _)| r);
        format!("{repository}:{}", tags(name, &self.aliases))
    }
    pub fn row(&self, cache: &SummaryCache) -> ImageRow {
        let digest = &self.tag.manifest.digest;
        ImageRow {
            reference: self.tag.name.clone(),
            aliases: self.aliases.clone(),
            descriptor: self.tag.manifest.clone(),
            summary: cache.summary(digest).cloned(),
            failure: cache.failure(digest).map(|f| f.reason.clone()),
        }
    }
}
/// Tags of one repository that name the same manifest, such as a version tag
/// and its `latest` alias, form one row. A version tag leads its row because
/// it names that manifest forever (machine-images §3); the other tags follow
/// by name. Rows keep the order of their first tag in `tags`.
pub(super) fn listed(tags: &[Tag]) -> Vec<Listed<'_>> {
    let mut rows: Vec<Vec<&Tag>> = Vec::new();
    let mut index: BTreeMap<(&str, &Digest), usize> = BTreeMap::new();
    for tag in tags {
        let repository = tag
            .name
            .rsplit_once(':')
            .map_or(tag.name.as_str(), |(r, _)| r);
        match index.entry((repository, &tag.manifest.digest)) {
            std::collections::btree_map::Entry::Occupied(row) => rows[*row.get()].push(tag),
            std::collections::btree_map::Entry::Vacant(row) => {
                row.insert(rows.len());
                rows.push(vec![tag]);
            }
        }
    }
    let alias = |tag: &Tag| {
        !tag.name
            .rsplit_once(':')
            .is_some_and(|(_, version)| Store::is_version_tag(version))
    };
    rows.into_iter()
        .map(|mut tags| {
            tags.sort_by(|a, b| (alias(a), &a.name).cmp(&(alias(b), &b.name)));
            Listed {
                tag: tags[0],
                aliases: tags[1..].iter().map(|t| t.name.clone()).collect(),
            }
        })
        .collect()
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
    media_rows: Vec<install_media::MediaRow>,
    media_total: usize,
    /// Set summaries still being built; `pending` counts every artifact.
    media_pending: usize,
    /// Kept apart from machine facets, whose kinds have the same names.
    media_facets: BTreeMap<String, BTreeSet<String>>,
    /// The worker read the store, so an empty library is really empty.
    read: bool,
}

fn facets_for(filter: &Filter) -> Facets {
    let mut f = Facets::default();
    if let Filter::Facet(kind, value) | Filter::MediaFacet(kind, value) = filter {
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
    // Counts are of rows, so a manifest under several tags counts once.
    let listed = listed(cache.tags());
    let mut result = Snapshot {
        storage: cache.storage_totals(),
        total: listed.len(),
        warning: cache.library_error().map(str::to_owned),
        read: true,
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
    let media_mode = q.filter.is_media();
    let mut packages = Vec::new();
    let mut sets = Vec::new();
    let pending: BTreeSet<_> = cache
        .pending()
        .into_iter()
        .map(|tag| tag.name.as_str())
        .collect();
    let mut machine_roms = firmware::MachineRoms::default();
    for row in &listed {
        let tag = row.tag;
        let s = cache.summary(&tag.manifest.digest);
        result.pending += usize::from(pending.contains(tag.name.as_str()));
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
                packages.push(row);
            } else {
                result.media_total += 1;
                result.media_pending += usize::from(pending.contains(tag.name.as_str()));
                if let Some(s) = s {
                    install_media::add_facets(&mut result.media_facets, s);
                }
                sets.push(row);
            }
            continue;
        }
        if let Some(s) = s.filter(|_| firmware_mode) {
            machine_roms.add(&row.references(), s);
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
        if firmware_mode || media_mode {
            continue;
        }
        let text_match = if s.is_some() {
            row.matched(&matches)
        } else {
            row.named(&q.text)
        };
        let filter_match = match &q.filter {
            Filter::MachineFiles => false,
            Filter::Favorites => s.is_some_and(|s| s.favorite),
            Filter::Recent => s.is_some_and(|s| s.last_run.is_some()),
            Filter::Facet(_, _) => row.matched(&matches),
            _ => true,
        };
        if !text_match || !filter_match {
            continue;
        }
        let image = row.row(cache);
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
    if media_mode {
        result.media_rows = install_media::rows(cache, &sets, &matches, q);
        result.matched = result.media_rows.len();
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
            // The header counts tags; its children are one per manifest.
            let tags = images
                .iter()
                .map(|image| 1 + image.aliases.len())
                .sum::<usize>();
            result.rows.push(ViewRow {
                image: first,
                group: Some(tags),
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
/// The details pane's first height, and the least either side of the split
/// keeps: a few list rows with their header and footer, a few pane lines.
const DETAILS_HEIGHT: f32 = 232.0;
const DETAILS_MIN_HEIGHT: f32 = 120.0;
const LIST_MIN_HEIGHT: f32 = 160.0;
fn row_cells(row: &ViewRow) -> [String; 9] {
    let image = &row.image;
    let summary = image.summary.as_ref();
    let title = summary.and_then(|s| s.titles.first());
    [
        image.name().to_owned(),
        image.tags(),
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
/// What a list row's name cell shows around the name, as the machines
/// mock-up draws it: the row kind's icon before the name, a warning and a
/// favourite star after it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Lead {
    pub icon: Option<Icon>,
    /// A row inside an expanded repository group, set in under its header.
    pub child: bool,
    /// The row cannot be used as it is: unreadable, or its firmware is missing.
    pub warning: bool,
    pub favourite: bool,
}

/// The warning badge after a name, as the main-window mock-up marks a
/// machine that cannot start.
const WARNING_BADGE: &str = " ⚠";
const FAVOURITE_BADGE: &str = " ★";
/// The set-in of a group's child rows.
const CHILD_INDENT: f32 = 16.0;
/// The icon's 16 points, its 2-point margin and the mock-up's 5-point gap
/// (rounded) before the name.
const NAME_ICON_ROOM: f32 = 21.0;

fn image_lead(row: &ViewRow) -> Lead {
    Lead {
        icon: Some(Icon::Image),
        child: row.child,
        warning: row.image.failure.is_some(),
        favourite: row.image.summary.as_ref().is_some_and(|s| s.favorite),
    }
}

/// The collections in the tree that the machines mock-up marks with an icon.
fn tree_icon(filter: &Filter) -> Option<Icon> {
    match filter {
        Filter::All => Some(Icon::Image),
        Filter::MachineFiles => Some(Icon::MachineFile),
        Filter::Media => Some(Icon::Media),
        Filter::Firmware => Some(Icon::Firmware),
        _ => None,
    }
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
    lead: Lead,
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
    // On the selection fill, text takes the selection stroke colour, as
    // egui's own selectable items do; every skin keeps that pair legible.
    let selected_text = selected.then(|| ui.visuals().selection.stroke.color);
    egui::Frame::new().fill(fill).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (column, (text, width)) in cells.iter().zip(widths.iter().copied()).enumerate() {
                let response = if column == 0 {
                    name_cell(ui, text, width, selected_text, lead)
                } else {
                    let rich = egui::RichText::new(text);
                    let rich = match selected_text {
                        Some(color) => rich.color(color),
                        None => rich,
                    };
                    ui.add_sized(
                        [width, 20.0],
                        egui::Label::new(rich)
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                };
                let response = response.on_hover_text(text);
                clicked |= response.clicked();
                double |= response.double_clicked();
            }
        });
    });
    (clicked, double)
}

/// The name cell as the machines mock-up draws it: left-aligned, the row's
/// icon (after a group's indent), the name, then its badges, truncated
/// together to the column. It is painted rather than a centred label, because
/// a centred egui label drops the space the icon and the indent need.
fn name_cell(
    ui: &mut egui::Ui,
    text: &str,
    width: f32,
    selected_text: Option<egui::Color32>,
    lead: Lead,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), text));
    // The colour an interactive label's text takes; the icon shares it.
    let ink = selected_text.unwrap_or_else(|| ui.style().interact(&response).text_color());
    let indent = if lead.child { CHILD_INDENT } else { 0.0 };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let mut left = rect.left() + indent;
    if let Some(icon) = lead.icon {
        let at = egui::Rect::from_min_size(
            egui::pos2(left + 2.0, rect.center().y - 8.0),
            egui::Vec2::splat(16.0),
        );
        crate::shell_icons::paint(&painter, at, icon, ink, ui.visuals());
        left += NAME_ICON_ROOM;
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat::simple(font.clone(), ink));
    for (on, badge, color) in [
        (lead.warning, WARNING_BADGE, ui.visuals().warn_fg_color),
        (
            lead.favourite,
            FAVOURITE_BADGE,
            crate::shell_icons::STAR_GOLD,
        ),
    ] {
        if on {
            job.append(
                badge,
                0.0,
                egui::TextFormat::simple(font.clone(), selected_text.unwrap_or(color)),
            );
        }
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - left).max(0.0));
    let galley = ui.painter().layout_job(job);
    painter.galley(
        egui::pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    response
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
/// The library's empty state while it holds no machine image: where the
/// first one comes from. Nothing contacts a registry until Get templates….
fn empty_library(ui: &mut egui::Ui, commands: &mut Vec<Command>) {
    ui.group(|ui| {
        // The get-templates mock-up's empty card: its picture beside the text.
        ui.horizontal_top(|ui| {
            crate::shell_icons::show(ui, Icon::EmptyLibrary);
            ui.vertical(|ui| empty_library_text(ui, commands));
        });
    });
}
fn empty_library_text(ui: &mut egui::Ui, commands: &mut Vec<Command>) {
    ui.strong("No machine images yet");
    ui.add(
        egui::Label::new(
            "Get templates… downloads ready-made open machines from the default registry: open firmware with its source and a blank disk, no operating system.",
        )
        .wrap(),
    );
    if ui.button("Get templates…").clicked() {
        commands.push(Command::GetTemplates);
    }
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "File › New machine… builds one from firmware you imported, and File › Import image… adds an image file. Nothing is downloaded until you choose.",
            )
            .small(),
        )
        .wrap(),
    );
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
    make: fn(String, String) -> Filter,
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
                let filter = make(kind.into(), value.clone());
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
    /// The machine of the image a check last read, by digest: the drives a
    /// floppy inserted before Start goes into.
    checked_machine: Option<(Digest, Box<assembly::MachineConfig>)>,
    pub active: Option<(ImageRow, String, WorkspaceKind)>,
    pub layers: Option<LayersDialog>,
    pub busy: bool,
    pending_operation: bool,
    pub status: Option<String>,
    pub error: Option<String>,
    pub launched: Option<pc_image::workspace::SessionTransfer>,
    import: Option<(String, String)>,
    /// Delete tag… while it is open, with the tag it would remove.
    delete: Option<String>,
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
    /// Edit metadata… while it is open.
    firmware_edit: Option<firmware_metadata::EditDialog>,
    /// Open README while it is open.
    firmware_readme: Option<firmware_metadata::ReadmeDialog>,
    /// A dialog asked for the source archive picker; the shell runs it.
    pub firmware_source_pick: Option<firmware::SourceTarget>,
    /// What the worker read for the selected package, by package digest;
    /// `None` inside while the reading is under way.
    firmware_package: Option<(Digest, Option<Result<firmware_metadata::PackageDetails>>)>,
    /// A published revision to select once a snapshot lists it.
    firmware_reselect: Option<String>,
    /// Kept apart from `selected` too: a set is never a Start target.
    media_selected: Option<install_media::MediaRow>,
    media_import: Option<install_media::ImportDialog>,
    /// Import installation media… asked for a host picker; the shell runs it.
    pub media_pick: Option<install_media::Pick>,
    /// Floppy units of the running machine, for Insert on a selected set.
    pub running_drives: Vec<(u8, assembly::FloppyGeometry)>,
    /// File › New machine… while it is open.
    pub new_machine: Option<crate::new_machine::Wizard>,
    /// The wizard asked for Import ROM folder…; the shell picks the folder.
    pub firmware_import_wanted: bool,
    /// A created image was selected; the next frame drops the machine-file selection.
    clear_file_selection: bool,
    /// Settings on an image while it is open.
    pub image_settings: Option<crate::image_settings::ImageSettings>,
    /// Image › Push… while it is open.
    pub push: Option<crate::registry_transfer::PushDialog>,
    /// Image › Pull… while it is open.
    pub pull: Option<crate::registry_transfer::PullDialog>,
    /// File › Get templates… while it is open.
    pub templates: Option<crate::templates::TemplatesDialog>,
    /// Log in… of Get templates…: the registry whose login the shell opens in
    /// Preferences › Registries.
    pub login_wanted: Option<String>,
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
            checked_machine: None,
            active: None,
            layers: None,
            busy: false,
            pending_operation: false,
            status: None,
            error: None,
            launched: None,
            import: None,
            delete: None,
            labels: String::new(),
            rows_built: 0,
            recoveries: Vec::new(),
            decision: None,
            firmware_error: false,
            pending_launch: None,
            worker_dead: false,
            firmware_selected: None,
            firmware_import: None,
            firmware_edit: None,
            firmware_readme: None,
            firmware_source_pick: None,
            firmware_package: None,
            firmware_reselect: None,
            media_selected: None,
            media_import: None,
            media_pick: None,
            running_drives: Vec::new(),
            new_machine: None,
            firmware_import_wanted: false,
            clear_file_selection: false,
            image_settings: None,
            push: None,
            pull: None,
            templates: None,
            login_wanted: None,
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
            self.on_event(event);
        }
        self.request_firmware_previews();
        self.retry_package();
        if disconnected && !self.worker_dead {
            self.busy = false;
            self.pending_operation = false;
            self.worker_dead = true;
            self.fail_firmware_import();
            self.fail_media_import();
            if self.error.is_none() {
                self.error = Some(
                    "Image store worker stopped; choose the Image store folder to retry".into(),
                );
            }
            self.fail_transfers();
        }
    }
    /// Push… and Pull… wait for the worker; once it is gone they end.
    fn fail_transfers(&mut self) {
        let message = self
            .error
            .clone()
            .unwrap_or_else(|| "Image store worker stopped".into());
        if let Some(dialog) = &mut self.push {
            dialog.fail(&message);
        }
        if let Some(dialog) = &mut self.pull {
            dialog.fail(&message);
        }
        if let Some(dialog) = &mut self.templates {
            dialog.fail(&message);
        }
    }
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Snapshot(snapshot) => {
                if let Some(selected) = &mut self.selected {
                    if let Some(current) = snapshot.rows.iter().find(|r| r.image.same(selected)) {
                        if current.image.summary.is_some() {
                            selected.summary = current.image.summary.clone();
                        }
                        // Tags may have come or gone; the row still acts on its lead tag.
                        selected.reference.clone_from(&current.image.reference);
                        selected.aliases.clone_from(&current.image.aliases);
                    }
                }
                if let Some(selected) = &mut self.firmware_selected {
                    if let Some(current) = snapshot.firmware_rows.iter().find(|r| r.same(selected))
                    {
                        *selected = current.clone();
                    }
                }
                if let Some(reference) = self.firmware_reselect.clone() {
                    if let Some(row) = snapshot
                        .firmware_rows
                        .iter()
                        .find(|r| r.image.has_tag(&reference) && r.image.summary.is_some())
                    {
                        self.firmware_reselect = None;
                        self.show_package(row.clone());
                    }
                }
                if let Some(selected) = &mut self.media_selected {
                    if let Some(current) = snapshot.media_rows.iter().find(|r| r.same(selected)) {
                        *selected = current.clone();
                    }
                }
                self.snapshot = snapshot;
            }
            Event::MediaSources(pick, result) => {
                self.busy = false;
                if let Some(dialog) = &mut self.media_import {
                    dialog.sources(pick, result);
                }
            }
            Event::MediaImported(result) => {
                self.busy = false;
                if let Ok(outcome) = &result {
                    self.status = Some(outcome.to_string());
                }
                match &mut self.media_import {
                    Some(dialog) => dialog.finished(result),
                    None => self.error = result.err(),
                }
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
            Event::FirmwarePreviewed(id, revision, result) => {
                if let Some(draft) = self
                    .firmware_import
                    .as_mut()
                    .and_then(|d| d.drafts.get_mut(&id))
                {
                    draft.previewed(revision, result.map(|v| *v));
                }
            }
            Event::FirmwareEditOpened(result) => {
                if let Some(dialog) = &mut self.firmware_edit {
                    let pick = dialog.pick_source && result.is_ok();
                    dialog.opened(result.map(|source| *source));
                    if pick {
                        self.firmware_source_pick = Some(firmware::SourceTarget::Edit);
                    }
                }
            }
            Event::FirmwareEditPreviewed(revision, result) => {
                if let Some(draft) = self.firmware_edit.as_mut().and_then(|d| d.draft.as_mut()) {
                    draft.previewed(revision, result.map(|v| *v));
                }
            }
            Event::FirmwareEdited(source, result) => {
                self.busy = false;
                if let Ok((target, notes)) = &result {
                    let mut status = format!("Published {target}; {source} is unchanged");
                    for note in notes {
                        status.push_str("; ");
                        status.push_str(note);
                    }
                    self.status = Some(status);
                    self.firmware_edit = None;
                    self.firmware_reselect = Some(target.clone());
                } else if let Some(dialog) = &mut self.firmware_edit {
                    dialog.published(&result);
                } else {
                    self.error = result.err();
                }
            }
            Event::FirmwarePackage(digest, result) => {
                if let Some((wanted, details)) = &mut self.firmware_package {
                    if *wanted == digest {
                        *details = Some(result.map(|d| *d));
                    }
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
                // An import can change the local marks the pane's Push line reads.
                self.refresh_package();
                if self.new_machine.is_some() {
                    self.send(Request::FirmwarePackages);
                }
            }
            Event::Checked(id, result, machine) => {
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|r| r.descriptor.digest == id)
                {
                    let ready = result.is_ok();
                    self.readiness = Some(result);
                    self.checked_machine = machine.map(|machine| (id.clone(), machine));
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
            Event::Layers(name, result, source) => {
                self.busy = false;
                if let Some(dialog) = &mut self.layers {
                    dialog.set_workspace(name);
                    dialog.set_details(result);
                    dialog.set_source(*source);
                }
            }
            Event::PublishRefused(refused) => {
                self.busy = false;
                self.pending_operation = false;
                match &mut self.layers {
                    Some(dialog) => dialog.refused(refused.error, refused.tags),
                    None => self.error = Some(refused.error),
                }
            }
            Event::SettingsOpened(result) => {
                self.busy = false;
                let opened = result.and_then(|opened| {
                    let (row, source, tags) = *opened;
                    let running = self.active.as_ref().is_some_and(|(active, _, _)| {
                        active.descriptor.digest == row.descriptor.digest
                    });
                    crate::image_settings::ImageSettings::new(row, source, tags, running)
                });
                match opened {
                    Ok(settings) => self.image_settings = Some(settings),
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Planned(revision, plan, tags) => {
                if let Some(settings) = &mut self.image_settings {
                    settings.planned(revision, plan.map(|plan| *plan), tags);
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
                self.refresh_package();
                // The row stays while another tag still names its manifest.
                if let Some(row) = &mut self.selected {
                    row.aliases.retain(|alias| *alias != reference);
                    if row.reference == reference {
                        if row.aliases.is_empty() {
                            self.selected = None;
                            self.readiness = None;
                        } else {
                            row.reference = row.aliases.remove(0);
                        }
                    }
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
            // Package refreshes never touch a busy flag: a Create may still run.
            Event::Packages(result) => {
                if let Some(wizard) = &mut self.new_machine {
                    match result {
                        Ok(choices) => wizard.set_store(choices),
                        Err(error) => wizard.error = Some(error),
                    }
                }
            }
            Event::Created(Ok(row)) => {
                if self.new_machine.as_ref().is_some_and(|w| w.busy) {
                    self.new_machine = None;
                }
                self.status = Some(format!("Created {}", row.reference));
                self.show_created(*row);
            }
            Event::Revised(Ok(row)) => {
                self.busy = false;
                self.image_settings = None;
                self.status = Some(format!(
                    "Published {}; the source image is unchanged",
                    row.reference
                ));
                self.show_created(*row);
            }
            Event::Revised(Err(refused)) => {
                self.busy = false;
                match &mut self.image_settings {
                    Some(settings) if settings.review.is_some() => {
                        settings.refused(refused.error, refused.tags)
                    }
                    _ => self.error = Some(refused.error),
                }
            }
            Event::Created(Err(error)) => match &mut self.new_machine {
                Some(wizard) => {
                    wizard.busy = false;
                    wizard.error = Some(error);
                }
                None => self.error = Some(error),
            },
            Event::FirmwareError(error) => {
                self.busy = false;
                self.firmware_error = true;
                self.error = Some(error);
            }
            Event::Transfer(event) => {
                if let Some(outcome) = crate::registry_transfer::closed_outcome(&event) {
                    let closed = match &event {
                        crate::registry_transfer::TransferEvent::Pushed { .. } => {
                            self.push.is_none()
                        }
                        // Get templates… reports its own pulls.
                        crate::registry_transfer::TransferEvent::Pulled { id, .. } => {
                            self.pull.is_none()
                                && !self.templates.as_ref().is_some_and(|d| d.owns(*id))
                        }
                        _ => self.pull.is_none(),
                    };
                    if closed {
                        self.status = Some(outcome.into());
                    }
                }
                let follow_up = self
                    .push
                    .as_mut()
                    .and_then(|dialog| dialog.on_event(&event));
                if let Some(dialog) = &mut self.pull {
                    dialog.on_event(&event);
                    if let crate::registry_transfer::PullState::Finished {
                        pulled: Some(tag), ..
                    } = &dialog.state
                    {
                        self.status = Some(format!("Pulled {tag}"));
                    }
                }
                if let Some(dialog) = &mut self.templates {
                    let running = dialog.running();
                    dialog.on_event(&event);
                    let pulled = dialog.pulled();
                    if running && !dialog.running() && !pulled.is_empty() {
                        self.status = Some(format!("Got {}", pulled.join(", ")));
                    }
                }
                if let Some(request) = follow_up {
                    self.send(Request::Transfer(request));
                }
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
            self.fail_transfers();
            self.fail_media_import();
        }
    }
    pub fn shutdown(&mut self) {
        // Dropping a transfer dialog cancels its transfer at the next check.
        self.push = None;
        self.pull = None;
        self.templates = None;
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown();
        }
        self.worker_dead = true;
    }
    /// The machine of image `digest`, once a check has read it.
    pub fn selected_machine(&self, digest: &Digest) -> Option<&assembly::MachineConfig> {
        self.checked_machine
            .as_ref()
            .filter(|(id, _)| id == digest)
            .map(|(_, machine)| machine.as_ref())
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
    pub(super) fn recheck(&mut self) {
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
    /// Machine › Save as new image…: Disk layers' Save as layer… form.
    pub fn begin_save_image(&mut self) {
        self.open_layers();
        if let Some(dialog) = &mut self.layers {
            dialog.begin_save(false);
        }
    }
    /// Settings… on the selected image; the worker reads it and the sheet
    /// opens read-only while the image runs.
    pub fn open_settings(&mut self) {
        if let Some(row) = self.selected.clone() {
            self.send(Request::OpenSettings(row));
        }
    }
    /// Select a created image in the machine view with no filter, search or
    /// collapsed group hiding it, so Start never targets a hidden machine.
    fn show_created(&mut self, row: ImageRow) {
        self.query.filter = Filter::All;
        self.query.text.clear();
        self.query.expanded.insert(row.repository().to_owned());
        self.firmware_selected = None;
        self.media_selected = None;
        self.pending_launch = None;
        self.clear_file_selection = true;
        self.selected = Some(row);
        self.readiness = None;
        self.send(Request::Query(self.query.clone()));
        self.recheck();
    }
    /// File › New machine…: the wizard reads firmware packages and tags first.
    pub fn begin_new_machine(&mut self) {
        let mut wizard = crate::new_machine::Wizard::new();
        wizard.store = self
            .root
            .as_ref()
            .and_then(|root| Store::open_existing(root).ok());
        self.new_machine = Some(wizard);
        self.send(Request::FirmwarePackages);
        if self.worker_dead {
            if let Some(wizard) = &mut self.new_machine {
                wizard.error = self.error.clone();
            }
        }
    }
    /// Image › Push…: the selected image (machine or firmware package), or
    /// the first local image; the worker builds the dry run.
    pub fn begin_push(&mut self, accounts: crate::registry_transfer::Accounts) {
        let selected = if self.query.filter.is_firmware() {
            self.firmware_selected
                .as_ref()
                .map(|row| row.image.reference.clone())
        } else {
            self.selected.as_ref().map(|row| row.reference.clone())
        };
        let (dialog, request) = crate::registry_transfer::PushDialog::open(accounts, selected);
        self.push = Some(dialog);
        self.send(Request::Transfer(request));
    }
    /// The kind of transfer a dialog runs now, for the question before quitting.
    pub fn running_transfer(&self) -> Option<&'static str> {
        if self.push.as_ref().is_some_and(|d| d.running()) {
            Some("push")
        } else if self.pull.as_ref().is_some_and(|d| d.running())
            || self.templates.as_ref().is_some_and(|d| d.running())
        {
            Some("pull")
        } else {
            None
        }
    }
    /// Image › Pull…: a reference into the local store.
    pub fn begin_pull(&mut self, accounts: crate::registry_transfer::Accounts) {
        self.pull = Some(crate::registry_transfer::PullDialog::open(accounts));
    }
    /// File › Get templates…: the official templates of the default registry,
    /// listed by the worker.
    pub fn begin_templates(&mut self, accounts: crate::registry_transfer::Accounts) {
        let (dialog, request) = crate::templates::TemplatesDialog::open(accounts);
        self.templates = Some(dialog);
        if let Some(request) = request {
            self.send(Request::Transfer(request));
        }
    }
    /// Preferences closed: Get templates… lists again when its Log in…
    /// opened them.
    pub fn preferences_closed(&mut self) {
        if let Some(request) = self.templates.as_mut().and_then(|d| d.after_preferences()) {
            self.send(Request::Transfer(request));
        }
    }
    pub fn begin_import(&mut self, path: &Path) {
        self.import = Some((path.to_string_lossy().into_owned(), String::new()));
    }
    pub fn begin_delete(&mut self) {
        self.delete = self
            .selected
            .as_ref()
            .filter(|_| self.active.is_none())
            .map(|row| row.reference.clone());
    }
    pub fn has_dialog(&self) -> bool {
        self.layers.is_some()
            || self.image_settings.is_some()
            || self.import.is_some()
            || self.delete.is_some()
            || self.decision.is_some()
            || self.firmware_import.is_some()
            || self.firmware_edit.is_some()
            || self.firmware_readme.is_some()
            || self.media_import.is_some()
            || self.new_machine.is_some()
            || self.push.is_some()
            || self.pull.is_some()
            || self.templates.is_some()
    }
    /// Close Disk layers, Import image, Delete tag and Import ROM folder for an
    /// exit; a running scan is cancelled. The Gaming keep-or-discard question
    /// stays: the exit waits for that decision.
    pub fn close_dialogs(&mut self) {
        self.layers = None;
        self.image_settings = None;
        self.import = None;
        self.delete = None;
        if let Some(dialog) = self.firmware_import.take() {
            dialog.control.cancel();
        }
        self.firmware_edit = None;
        self.firmware_readme = None;
        if let Some(dialog) = self.media_import.take() {
            dialog.control.cancel();
        }
        self.new_machine = None;
        self.push = None;
        self.pull = None;
        self.templates = None;
    }
    /// Entering the Firmware or Installation media collection drops the machine
    /// selection, so Start never targets a machine that is no longer shown.
    fn set_filter(&mut self, filter: Filter, selected_file: &mut Option<usize>) {
        if filter.is_firmware() || filter.is_media() {
            self.selected = None;
            self.readiness = None;
            self.pending_launch = None;
            *selected_file = None;
        }
        if !filter.is_firmware() {
            self.firmware_selected = None;
        }
        if !filter.is_media() {
            self.media_selected = None;
        }
        self.query.filter = filter;
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        files: &[LibraryEntry],
        selected_file: &mut Option<usize>,
    ) -> Vec<Command> {
        if std::mem::take(&mut self.clear_file_selection) {
            *selected_file = None;
        }
        ui.add_enabled_ui(!self.busy, |ui| {
            self.show_contents(ui, files, selected_file)
        })
        .inner
    }
    fn show_contents(
        &mut self,
        ui: &mut egui::Ui,
        files: &[LibraryEntry],
        selected_file: &mut Option<usize>,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let mut changed = false;
        egui::Panel::left("library-tree")
            .exact_size(230.0)
            .show(ui, |ui| {
                ui.strong("Library");
                let firmware_label = format!("Firmware ({})", self.snapshot.firmware_total);
                let media_label = format!("Installation media ({})", self.snapshot.media_total);
                for (label, filter) in [
                    ("All images", Filter::All),
                    ("Recently played", Filter::Recent),
                    ("Favourites", Filter::Favorites),
                    ("Machine files", Filter::MachineFiles),
                    (media_label.as_str(), Filter::Media),
                    (firmware_label.as_str(), Filter::Firmware),
                ] {
                    // Collections without an icon keep the icon's room, so
                    // the names line up as in the machines mock-up's tree.
                    let mark = match tree_icon(&filter) {
                        Some(icon) => crate::shell_icons::atom(icon),
                        None => crate::shell_icons::blank(),
                    };
                    if ui
                        .selectable_label(self.query.filter == filter, (mark, label))
                        .clicked()
                    {
                        self.set_filter(filter, selected_file);
                        changed = true;
                    }
                }
                let firmware_mode = self.query.filter.is_firmware();
                let media_mode = self.query.filter.is_media();
                let kinds: &[&str] = if firmware_mode {
                    &firmware::FACETS
                } else if media_mode {
                    &install_media::FACETS
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
                } else if media_mode {
                    install_media::facet_label
                } else {
                    |_, value| value.to_owned()
                };
                let make: fn(String, String) -> Filter = if media_mode {
                    Filter::MediaFacet
                } else {
                    Filter::Facet
                };
                let snapshot = self.snapshot.clone();
                let facets = if media_mode {
                    &snapshot.media_facets
                } else {
                    &snapshot.facets
                };
                for kind in kinds {
                    let picked = egui::CollapsingHeader::new(*kind)
                        .id_salt(("facet-group", media_mode, *kind))
                        .default_open(
                            (firmware_mode && *kind != "Board")
                                || (media_mode && matches!(*kind, "Format" | "Labels")),
                        )
                        .show(ui, |ui| {
                            facets.get(*kind).and_then(|values| {
                                facet_list(ui, kind, values, &self.query.filter, label, make).0
                            })
                        })
                        .body_returned
                        .flatten();
                    if let Some(filter) = picked {
                        self.set_filter(filter, selected_file);
                        changed = true;
                    }
                }
            });
        // The split between the list and the details pane drags, as in the
        // library mock-up; egui keeps the chosen height for the session. The
        // pane fills its height, so it scrolls only when its content is taller.
        let details_max = (ui.available_height() - LIST_MIN_HEIGHT).max(DETAILS_MIN_HEIGHT);
        egui::Panel::bottom("library-details")
            .resizable(true)
            .default_size(DETAILS_HEIGHT)
            .size_range(DETAILS_MIN_HEIGHT..=details_max)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("selected-image-details")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.query.filter.is_firmware() {
                            match self.firmware_selected.clone() {
                                Some(row) => {
                                    let action = firmware::details(
                                        ui,
                                        &row,
                                        self.package_details(),
                                        self.can_edit_firmware(),
                                    );
                                    self.pane_action(&row, action);
                                }
                                None => {
                                    ui.label("Select a firmware package for details.");
                                }
                            }
                        } else if self.query.filter.is_media() {
                            match &self.media_selected {
                                Some(row) => {
                                    if let Some(unit) =
                                        install_media::details(ui, row, &self.running_drives)
                                    {
                                        commands.push(Command::InsertSet(unit));
                                    }
                                }
                                None => {
                                    ui.label("Select an installation set for details.");
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
                                columns[0].label(row.references());
                                columns[0].monospace(&row.descriptor.digest.hex()[..12]);
                                let meta = &mut columns[1];
                                // The machines mock-up heads an image's details
                                // with its icon, and a favourite with its star.
                                meta.horizontal(|ui| {
                                    crate::shell_icons::show(ui, Icon::Image);
                                    ui.strong(row.name());
                                    if row.summary.as_ref().is_some_and(|s| s.favorite) {
                                        ui.colored_label(crate::shell_icons::STAR_GOLD, "★");
                                    }
                                });
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
                                                .map(|member| match member.blank_bytes {
                                                    Some(bytes) => {
                                                        format!("zero base {}", human_size(bytes))
                                                    }
                                                    None => member.digest.hex()[..8].to_owned(),
                                                })
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
                                    if ui
                                        .button("Settings…")
                                        .on_hover_text("Changes create a new image revision; disks are shared.")
                                        .clicked()
                                    {
                                        commands.push(Command::Settings);
                                    }
                                    if ui.button("Layers…").clicked() {
                                        commands.push(Command::Layers);
                                    }
                                    if ui.button("Push…").clicked() {
                                        commands.push(Command::Push);
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
            let media_mode = self.query.filter.is_media();
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
                if media_mode {
                    if ui
                        .add_enabled(
                            self.can_import_media(),
                            egui::Button::new("Import installation media…"),
                        )
                        .clicked()
                    {
                        commands.push(Command::ImportMedia);
                    }
                    ui.add_enabled(false, egui::Button::new("Make transfer media…"))
                        .on_disabled_hover_text(
                            "Making transfer media from extras is not available yet.",
                        );
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
                let private = self
                    .snapshot
                    .firmware_rows
                    .iter()
                    .filter(|r| r.source_missing)
                    .count();
                ui.label(format!(
                    "{} of {} packages · {} pending{}",
                    self.snapshot.matched,
                    self.snapshot.firmware_total,
                    self.snapshot.pending,
                    if private > 0 {
                        format!(" · {private} private only")
                    } else {
                        String::new()
                    }
                ));
                return;
            }
            if media_mode {
                if let Some(warning) = &self.snapshot.warning {
                    ui.label(warning);
                }
                changed |= self.media_list(ui, selected_file);
                ui.label(format!(
                    "{} of {} sets · {} pending",
                    self.snapshot.matched, self.snapshot.media_total, self.snapshot.media_pending
                ));
                return;
            }
            if !self.recoveries.is_empty() {
                // The machines mock-up's warning banner for an unfinished session.
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, "⚠");
                    ui.strong("Unfinished sessions");
                });
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
            let empty = self.snapshot.read && self.snapshot.total == 0;
            if empty {
                empty_library(ui, &mut commands);
            }
            // Machine files from disk list only under Machine files; All
            // images lists stored images alone.
            let files_mode = self.query.filter == Filter::MachineFiles;
            let visible_files: Vec<_> = if files_mode {
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
            // The empty state replaces the table unless machine files are listed.
            if empty && visible_files.is_empty() {
                return;
            }
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
                                            entry.name.clone(),
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
                                            Lead {
                                                icon: Some(Icon::MachineFile),
                                                warning: entry.has_problem(),
                                                ..Lead::default()
                                            },
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
                                                Lead::default(),
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
                                            let selected = self
                                                .selected
                                                .as_ref()
                                                .is_some_and(|r| r.same(image));
                                            let (clicked, double) = draw_cells(
                                                ui,
                                                &cells,
                                                &COLUMN_WIDTHS,
                                                selected,
                                                index % 2 == 0,
                                                image_lead(row),
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
            if files_mode {
                ui.label(format!(
                    "{} of {} machine files",
                    visible_files.len(),
                    files.len()
                ));
            } else {
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
            }
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
        let can_import = !running && self.can_import_firmware();
        if let Some(wizard) = &mut self.new_machine {
            wizard.can_import_roms = can_import;
            match crate::new_machine_view::show(ctx, wizard, classic) {
                crate::new_machine_view::Action::None => (),
                crate::new_machine_view::Action::Cancel => self.new_machine = None,
                crate::new_machine_view::Action::Create => match wizard.request() {
                    Ok((image, reference, expected)) => {
                        wizard.busy = true;
                        wizard.error = None;
                        requests.push(Request::CreateImage(Box::new(image), reference, expected));
                    }
                    Err(error) => wizard.error = Some(error),
                },
                // The library's Import ROM folder… flow; the shell picks the
                // folder, and the wizard rereads packages once it imported.
                crate::new_machine_view::Action::ImportRoms => self.firmware_import_wanted = true,
            }
            self.firmware_modal(ctx, classic, &mut requests);
            return requests;
        }
        if let Some(settings) = &mut self.image_settings {
            settings.read_only = self.active.as_ref().is_some_and(|(active, _, _)| {
                active.descriptor.digest == settings.row.descriptor.digest
            });
            if settings.review.is_some() {
                match crate::settings_sheet::show_review(ctx, settings, self.busy, classic) {
                    crate::settings_sheet::ReviewAction::None => (),
                    crate::settings_sheet::ReviewAction::Back => settings.review = None,
                    crate::settings_sheet::ReviewAction::Cancel => self.image_settings = None,
                    crate::settings_sheet::ReviewAction::Publish(plan, target) => {
                        self.busy = true;
                        self.status = Some("Publishing the new image revision…".into());
                        requests.push(Request::PublishRevision(plan, target));
                    }
                }
            } else {
                match crate::settings_sheet::show_image(ctx, settings, classic) {
                    crate::settings_sheet::ImageAction::None => (),
                    crate::settings_sheet::ImageAction::Cancel => self.image_settings = None,
                    crate::settings_sheet::ImageAction::Review => {
                        let request = settings.begin_review();
                        requests.push(Request::PlanRevision(
                            settings.row.clone(),
                            request.revision,
                            Box::new(request.machine),
                        ));
                    }
                }
            }
            self.firmware_modal(ctx, classic, &mut requests);
            return requests;
        }
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
        if let Some(mut chosen) = self.delete.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("delete-tag")).show(ctx, |ui| {
                if classic {
                    crate::shell::gradient_title(ui, "Delete tag?");
                } else {
                    ui.heading("Delete tag?");
                }
                if let Some(row) = &self.selected {
                    let tags: Vec<&String> = std::iter::once(&row.reference)
                        .chain(&row.aliases)
                        .collect();
                    if !tags.contains(&&chosen) {
                        chosen.clone_from(&row.reference);
                    }
                    // A row lists every tag of its manifest; the user picks one.
                    if tags.len() > 1 {
                        for tag in &tags {
                            ui.radio_value(&mut chosen, (*tag).clone(), tag.as_str());
                        }
                    }
                    ui.label(format!("Deletes {chosen}"));
                    let kept: Vec<&str> = tags
                        .iter()
                        .filter(|tag| ***tag != chosen)
                        .map(|tag| tag.as_str())
                        .collect();
                    ui.label(if kept.is_empty() {
                        "Only this tag is removed. Shared bases and layers stay in the store."
                            .to_owned()
                    } else {
                        format!(
                            "Only this tag is removed; the image keeps {}. Shared bases and \
                             layers stay in the store.",
                            kept.join(", ")
                        )
                    });
                    if ui.button("Delete tag").clicked() {
                        requests.push(Request::Delete(ImageRow {
                            reference: chosen.clone(),
                            ..row.clone()
                        }));
                        self.busy = true;
                        close = true;
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
            self.delete = (!close).then_some(chosen);
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
        self.transfer_modals(ctx, classic, &mut requests);
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
                crate::layers_dialog::Action::Publish(name, target) => {
                    self.pending_operation = true;
                    self.busy = true;
                    self.status = Some("Publishing whole machine…".into());
                    if let Some(row) = self
                        .active
                        .as_ref()
                        .map(|(r, _, _)| r)
                        .or(self.selected.as_ref())
                    {
                        requests.push(Request::Publish(row.clone(), name, target));
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
        self.firmware_modal(ctx, classic, &mut requests);
        self.media_modal(ctx, classic, &mut requests);
        requests
    }
    fn transfer_modals(&mut self, ctx: &egui::Context, classic: bool, requests: &mut Vec<Request>) {
        use crate::registry_transfer_view::{show_pull, show_push, show_templates, Action};
        if let Some(dialog) = &mut self.push {
            match show_push(ctx, dialog, classic) {
                Action::Send(request) => requests.push(Request::Transfer(request)),
                Action::SendAll(list) => requests.extend(list.into_iter().map(Request::Transfer)),
                Action::Close => self.push = None,
                Action::None | Action::LogIn(_) => (),
            }
        }
        if let Some(dialog) = &mut self.pull {
            match show_pull(ctx, dialog, classic) {
                Action::Send(request) => requests.push(Request::Transfer(request)),
                Action::SendAll(list) => requests.extend(list.into_iter().map(Request::Transfer)),
                Action::Close => self.pull = None,
                Action::None | Action::LogIn(_) => (),
            }
        }
        if let Some(dialog) = &mut self.templates {
            match show_templates(ctx, dialog, classic) {
                Action::Send(request) => requests.push(Request::Transfer(request)),
                Action::SendAll(list) => requests.extend(list.into_iter().map(Request::Transfer)),
                Action::Close => self.templates = None,
                Action::LogIn(host) => self.login_wanted = Some(host),
                Action::None => (),
            }
        }
    }
    /// Apply worker events until `done` holds; tests only, the frame loop polls.
    #[cfg(test)]
    pub(crate) fn pump_until(&mut self, mut done: impl FnMut(&Self) -> bool) {
        while !done(self) {
            let event = self
                .worker
                .as_ref()
                .expect("a library worker")
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .expect("worker notification");
            if let Event::Error(error) = &event {
                panic!("worker: {error}");
            }
            self.on_event(event);
        }
    }
    /// Apply the events already queued, without waiting; tests only.
    #[cfg(test)]
    pub(crate) fn pump_queued(&mut self) {
        while let Some(event) = self
            .worker
            .as_ref()
            .and_then(|worker| worker.receive.try_recv().ok())
        {
            if let Event::Error(error) = &event {
                panic!("worker: {error}");
            }
            self.on_event(event);
        }
    }
    fn firmware_modal(&mut self, ctx: &egui::Context, classic: bool, requests: &mut Vec<Request>) {
        if let Some(dialog) = &mut self.firmware_import {
            match dialog.show(ctx, classic) {
                firmware::DialogAction::Import => requests.extend(self.import_picks()),
                firmware::DialogAction::Close => {
                    if let Some(dialog) = self.firmware_import.take() {
                        dialog.control.cancel();
                    }
                }
                firmware::DialogAction::PickSource(id) => {
                    self.firmware_source_pick =
                        Some(firmware::SourceTarget::Import(id, dialog.folder.clone()));
                }
                firmware::DialogAction::None => (),
            }
        }
        if let Some(dialog) = &mut self.firmware_edit {
            match dialog.show(ctx, classic) {
                firmware_metadata::EditAction::Publish => {
                    requests.extend(self.publish_firmware_edit())
                }
                firmware_metadata::EditAction::PickSource => {
                    self.firmware_source_pick = Some(firmware::SourceTarget::Edit);
                }
                firmware_metadata::EditAction::Close => self.firmware_edit = None,
                firmware_metadata::EditAction::None => (),
            }
        }
        if self
            .firmware_readme
            .as_ref()
            .is_some_and(|dialog| dialog.show(ctx, classic))
        {
            self.firmware_readme = None;
        }
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
    fn an_empty_library_offers_get_templates_once_the_store_is_read() {
        let ctx = egui::Context::default();
        let frame = |library: &mut LibraryView, events: Vec<egui::Event>| {
            let mut selected = None;
            let mut commands = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| commands = library.show(ui, &[], &mut selected),
            );
            output.textures_delta.clear();
            let mut button = None;
            let mut texts = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    if text.galley.text() == "Get templates…" {
                        button = Some(text.visual_bounding_rect().center());
                    }
                    texts.push(text.galley.text().to_owned());
                }
            }
            (commands, button, texts)
        };
        // Before the worker's first snapshot nothing says the library is empty.
        let mut library = LibraryView::new(None);
        assert_eq!(frame(&mut library, vec![]).1, None);
        library.snapshot = Arc::new(Snapshot {
            read: true,
            ..Default::default()
        });
        let (_, button, texts) = frame(&mut library, vec![]);
        let at = button.expect("the empty state shows Get templates…");
        for wanted in [
            "No machine images yet",
            "Get templates… downloads ready-made open machines from the default registry: open firmware with its source and a blank disk, no operating system.",
            "File › New machine… builds one from firmware you imported, and File › Import image… adds an image file. Nothing is downloaded until you choose.",
        ] {
            assert!(texts.iter().any(|t| t == wanted), "{wanted}: {texts:?}");
        }
        // The card replaces the empty table.
        assert!(!texts.iter().any(|t| t == "Last run"), "{texts:?}");
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(
            &mut library,
            vec![egui::Event::PointerMoved(at), click(true)],
        );
        let (commands, _, _) = frame(&mut library, vec![click(false)]);
        assert_eq!(commands, [Command::GetTemplates]);
        // A library with machine images has no empty state, only its table.
        let (_temp, store, _) = catalogue(1);
        library.snapshot = Arc::new(snapshot(&warm(&store), &Query::default()));
        let (_, button, texts) = frame(&mut library, vec![]);
        assert_eq!(button, None);
        assert!(texts.iter().any(|t| t == "Last run"), "{texts:?}");
        // Firmware packages and installation media are not machine images, so
        // a library holding only them still offers Get templates….
        let (_temp, store) = firmware_and_media_only();
        let result = snapshot(&warm(&store), &Query::default());
        assert_eq!(
            (result.total, result.firmware_total, result.media_total),
            (0, 1, 1)
        );
        library.snapshot = Arc::new(result);
        assert!(frame(&mut library, vec![]).1.is_some());
    }
    /// A store with one firmware package and one media set, and no machine image.
    fn firmware_and_media_only() -> (assembly::test_support::TempDir, Store) {
        let (temp, store, images) = catalogue(1);
        let machine = Manifest::decode_for_summary(
            &store
                .read_verified(&images[0], image_store::MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        let bytes = image_store::media_set::Config {
            schema: 1,
            platform: "synthetic".into(),
            media: vec![image_store::media_set::Medium {
                id: "disk-1".into(),
                label: "Disk 1".into(),
                format: toml::Table::new(),
                blob: Digest::of(b"disk"),
            }],
            extras: Vec::new(),
        }
        .encode()
        .unwrap();
        let blob = store.import(bytes.as_slice(), None).unwrap();
        let config = Descriptor::new(mt::MEDIA_SET_CONFIG, blob.digest, blob.size);
        let set = Manifest::new_media_set("synthetic", config, machine.layers).unwrap();
        let set = store.import_manifest(&set).unwrap();
        let index = store.root().join("images/index.json");
        let text = std::fs::read_to_string(&index).unwrap();
        let prefix = text.trim_end().strip_suffix("]}").unwrap();
        std::fs::write(&index,format!("{prefix},{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"installation:one\"}}}}]}}",mt::MANIFEST,set.digest,set.size)).unwrap();
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
        pc_image::firmware_import::publish(
            &store,
            &candidate,
            &[],
            pc_image::firmware::catalogue(),
        )
        .unwrap();
        store
            .remove_tag("synthetic-0:one", &images[0].digest)
            .unwrap();
        (temp, store)
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
                library.show(ui, &[], &mut selected);
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
                    library.show(ui, &[], &mut selected);
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
        let (_temp, store, images) = catalogue(3);
        // A second revision of synthetic-0: another manifest under another tag.
        store
            .remove_tag("synthetic-2:one", &images[2].digest)
            .unwrap();
        store
            .update_tag("synthetic-0:two", &images[2], None, &Provider)
            .unwrap();
        for image in [&images[0], &images[2]] {
            store
                .edit_library(&image.digest, |entry| {
                    entry.favorite = true;
                    entry.labels = vec!["collection".into()];
                    entry.last_run = Some("123".into());
                    Ok(())
                })
                .unwrap();
        }
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
    /// Appends a tag naming `manifest` to the store index, as a pull of a
    /// version tag and its alias leaves the index.
    fn tag_again(store: &Store, name: &str, manifest: &Descriptor) {
        let index = store.root().join("images/index.json");
        let text = std::fs::read_to_string(&index).unwrap();
        let prefix = text.trim_end().strip_suffix("]}").unwrap();
        let artifact = manifest
            .artifact_type
            .as_deref()
            .map_or_else(String::new, |t| format!(",\"artifactType\":\"{t}\""));
        std::fs::write(&index, format!("{prefix},{{\"mediaType\":\"{}\"{artifact},\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"{name}\"}}}}]}}", mt::MANIFEST, manifest.digest, manifest.size)).unwrap();
    }
    #[test]
    fn a_version_tag_and_its_alias_are_one_row_in_every_collection() {
        let (temp, store, images) = catalogue(2);
        // dos-lab:1.0.0 is one revision; 2.0.0 and its latest alias are another.
        for (tag, image) in [
            ("dos-lab:1.0.0", 0),
            ("dos-lab:latest", 1),
            ("dos-lab:2.0.0", 1),
        ] {
            store
                .update_tag(tag, &images[image], None, &Provider)
                .unwrap();
        }
        for (index, image) in images.iter().enumerate() {
            store
                .remove_tag(&format!("synthetic-{index}:one"), &image.digest)
                .unwrap();
        }
        // A firmware package under its import tag, a version tag and latest.
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
        tag_again(&store, "firmware/synthetic:latest", &package);
        tag_again(&store, "firmware/synthetic:1.1.0", &package);
        // An installation set under a version tag and latest.
        let machine = Manifest::decode_for_summary(
            &store
                .read_verified(&images[0], image_store::MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        let bytes = image_store::media_set::Config {
            schema: 1,
            platform: "synthetic".into(),
            media: vec![image_store::media_set::Medium {
                id: "disk-1".into(),
                label: "Disk 1".into(),
                format: toml::Table::new(),
                blob: Digest::of(b"disk"),
            }],
            extras: Vec::new(),
        }
        .encode()
        .unwrap();
        let blob = store.import(bytes.as_slice(), None).unwrap();
        let config = Descriptor::new(mt::MEDIA_SET_CONFIG, blob.digest, blob.size);
        let set = Manifest::new_media_set("synthetic", config, machine.layers).unwrap();
        let set = store.import_manifest(&set).unwrap();
        tag_again(&store, "installation:latest", &set);
        tag_again(&store, "installation:1.0.0", &set);
        // Eight tags, four manifests: pending counts manifests too.
        let cold = SummaryCache::open(&store).unwrap();
        assert_eq!(cold.tags().len(), 8);
        assert_eq!(snapshot(&cold, &Query::default()).pending, 4);
        let cache = warm(&store);

        let mut query = Query::default();
        query.expanded.insert("dos-lab".into());
        let machines = snapshot(&cache, &query);
        assert_eq!((machines.total, machines.matched), (2, 2));
        let rows: Vec<_> = machines
            .rows
            .iter()
            .map(|row| (row.group, row.child, row_cells(row)[1].clone()))
            .collect();
        // The header counts the repository's three tags over two rows.
        assert_eq!(
            rows,
            [
                (Some(3), false, String::from("1.0.0")),
                (None, true, String::from("1.0.0")),
                (None, true, String::from("2.0.0, latest")),
            ]
        );
        let revision = &machines.rows[2].image;
        assert_eq!(revision.reference, "dos-lab:2.0.0");
        assert_eq!(revision.aliases, ["dos-lab:latest"]);
        assert_eq!(revision.references(), "dos-lab:2.0.0, latest");
        // A search for the alias finds its row, which still acts on the version tag.
        let found = snapshot(
            &cache,
            &Query {
                text: "latest".into(),
                ..Default::default()
            },
        );
        assert_eq!(found.matched, 1);
        assert_eq!(found.rows[0].image.reference, "dos-lab:2.0.0");

        let firmware = snapshot(
            &cache,
            &Query {
                filter: Filter::Firmware,
                ..Default::default()
            },
        );
        assert_eq!((firmware.firmware_total, firmware.matched), (1, 1));
        assert_eq!(firmware.firmware_rows.len(), 1);
        let package = &firmware.firmware_rows[0].image;
        assert_eq!(package.reference, "firmware/synthetic:1.1.0");
        assert_eq!(package.tags(), "1.1.0, 1, latest");

        let media = snapshot(
            &cache,
            &Query {
                filter: Filter::Media,
                ..Default::default()
            },
        );
        assert_eq!((media.media_total, media.matched), (1, 1));
        assert_eq!(media.media_rows.len(), 1);
        assert_eq!(media.media_rows[0].image.reference, "installation:1.0.0");
        assert_eq!(
            install_media::cells(&media.media_rows[0], &[])[1],
            "1.0.0, latest"
        );
    }
    #[test]
    fn delete_tag_names_the_tag_offers_the_others_and_the_row_stays() {
        use firmware::tests::{frame, texts};
        let (_temp, store, images) = catalogue(1);
        for tag in ["synthetic-0:latest", "synthetic-0:1.0.0"] {
            store.update_tag(tag, &images[0], None, &Provider).unwrap();
        }
        store
            .remove_tag("synthetic-0:one", &images[0].digest)
            .unwrap();
        let result = snapshot(&warm(&store), &Query::default());
        assert_eq!((result.total, result.rows.len()), (1, 1));
        let row = result.rows[0].image.clone();
        let mut view = LibraryView::new(None);
        view.snapshot = Arc::new(result);
        view.selected = Some(row.clone());
        view.begin_delete();
        assert_eq!(view.delete.as_deref(), Some("synthetic-0:1.0.0"));

        let ctx = egui::Context::default();
        let mut requests = Vec::new();
        let mut time = 0.0;
        let mut draw = |view: &mut LibraryView, events: Vec<egui::Event>| {
            time += 0.05;
            let output = frame(&ctx, egui::vec2(1200.0, 900.0), time, events, |ui| {
                requests.extend(view.modals(&ui.ctx().clone(), None, Vec::new(), false, false));
            });
            texts(&output)
        };
        // A modal sizes itself over its first frames.
        let mut shown = draw(&mut view, vec![]);
        for _ in 0..20 {
            shown = draw(&mut view, vec![]);
        }
        let at = |shown: &[(String, egui::Rect)], text: &str| {
            shown
                .iter()
                .find(|(t, _)| t == text)
                .map(|(_, rect)| rect.center())
                .unwrap_or_else(|| panic!("{text}: {shown:?}"))
        };
        at(&shown, "Deletes synthetic-0:1.0.0");
        at(&shown, "Only this tag is removed; the image keeps synthetic-0:latest. Shared bases and layers stay in the store.");
        let mut click = |view: &mut LibraryView, at: egui::Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            draw(view, vec![egui::Event::PointerMoved(at)]);
            draw(view, vec![button(true)]);
            draw(view, vec![button(false)]);
            draw(view, vec![])
        };
        let shown = click(&mut view, at(&shown, "synthetic-0:latest"));
        at(&shown, "Deletes synthetic-0:latest");
        click(&mut view, at(&shown, "Delete tag"));
        assert!(view.delete.is_none() && view.busy);
        let [Request::Delete(chosen)] = requests.as_slice() else {
            panic!("one Delete request");
        };
        assert_eq!(chosen.reference, "synthetic-0:latest");
        assert_eq!(chosen.descriptor.digest, images[0].digest);

        // The worker removes the alias; the row and its selection stay.
        delete_tag(&store, chosen).unwrap();
        view.on_event(Event::Deleted(chosen.reference.clone()));
        let after = snapshot(&warm(&store), &Query::default());
        assert_eq!((after.total, after.rows.len()), (1, 1));
        assert_eq!(after.rows[0].image.tags(), "1.0.0");
        view.on_event(Event::Snapshot(Arc::new(after)));
        let selected = view.selected.as_ref().expect("selection survives");
        assert_eq!(
            (selected.reference.as_str(), selected.aliases.len()),
            ("synthetic-0:1.0.0", 0)
        );
        // Deleting the lead tag of a row with an alias keeps it selected
        // under the alias.
        view.selected = Some(row);
        view.on_event(Event::Deleted("synthetic-0:1.0.0".into()));
        let selected = view.selected.as_ref().expect("the alias keeps the row");
        assert_eq!(
            (selected.reference.as_str(), selected.aliases.len()),
            ("synthetic-0:latest", 0)
        );
    }
    #[test]
    fn the_details_split_drags_keeps_its_height_and_shows_the_whole_pane() {
        use firmware::tests::{frame, texts};
        let (_f, cache) = firmware::tests::library();
        let mut view = LibraryView::new(None);
        let mut selected_file = None;
        view.select_first_package(&cache, &mut selected_file);
        let ctx = egui::Context::default();
        let size = egui::vec2(1400.0, 1000.0);
        let mut time = 0.0;
        let mut draw = |view: &mut LibraryView, events: Vec<egui::Event>| {
            time += 0.05;
            let output = frame(&ctx, size, time, events, |ui| {
                view.show(ui, &[], &mut selected_file);
            });
            texts(&output)
        };
        // The pane's second line, under the package name, and its last facts line.
        let top = "System BIOS · local package · firmware/synthetic-two-chip-bios:1";
        let last = "Edit metadata… creates a new package revision; ROM blobs unchanged.";
        let y = |shown: &[(String, egui::Rect)], text: &str| {
            shown
                .iter()
                .find(|(t, _)| t == text)
                .map(|(_, rect)| rect.min.y)
        };
        let mut shown = draw(&mut view, vec![]);
        for _ in 0..3 {
            shown = draw(&mut view, vec![]);
        }
        let default = y(&shown, top).expect("the pane shows the package");
        // The default pane is too short for the package, so its end scrolls.
        assert!(y(&shown, last).is_none(), "{shown:?}");
        type Draw<'a> =
            dyn FnMut(&mut LibraryView, Vec<egui::Event>) -> Vec<(String, egui::Rect)> + 'a;
        let drag = |draw: &mut Draw, view: &mut LibraryView, from: f32, to: f32| {
            let at = |y| egui::pos2(800.0, y);
            let button = |y, pressed| egui::Event::PointerButton {
                pos: at(y),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            draw(view, vec![egui::Event::PointerMoved(at(from))]);
            draw(view, vec![button(from, true)]);
            draw(view, vec![egui::Event::PointerMoved(at((from + to) / 2.0))]);
            draw(view, vec![egui::Event::PointerMoved(at(to))]);
            draw(view, vec![button(to, false)]);
            draw(view, vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))])
        };
        // The split sits on the pane's top edge, the frame margin and the
        // package name above that line.
        let margin = default - (size.y - DETAILS_HEIGHT);
        assert!(margin > 0.0 && margin < 40.0, "{default}");
        drag(
            &mut draw,
            &mut view,
            default - margin,
            default - margin - 400.0,
        );
        let mut shown = draw(&mut view, vec![]);
        let taller = y(&shown, top).expect("the pane still shows the package");
        assert!(
            (default - taller - 400.0).abs() < 2.0,
            "{default} → {taller}"
        );
        assert!(y(&shown, last).is_some(), "the taller pane shows it all");
        // The height stays over frames and over a refreshed list.
        for _ in 0..5 {
            draw(&mut view, vec![]);
        }
        view.snapshot = Arc::new(snapshot(&cache, &view.query));
        shown = draw(&mut view, vec![]);
        assert_eq!(y(&shown, top), Some(taller));
        assert!(y(&shown, last).is_some());
        // Dragging further keeps a few list rows above the split, and down
        // keeps a few pane lines.
        drag(&mut draw, &mut view, taller - margin, 20.0);
        let shown = draw(&mut view, vec![]);
        let highest = y(&shown, top).unwrap();
        assert!(
            (highest - margin - LIST_MIN_HEIGHT).abs() < 2.0,
            "{highest}"
        );
        assert!(shown
            .iter()
            .any(|(t, r)| t.starts_with("Name") && r.max.y < highest));
        drag(&mut draw, &mut view, highest - margin, size.y - 5.0);
        let shown = draw(&mut view, vec![]);
        let lowest = y(&shown, top).unwrap();
        assert!(
            (lowest - margin - (size.y - DETAILS_MIN_HEIGHT)).abs() < 2.0,
            "{lowest}"
        );
    }
    #[test]
    fn all_images_lists_stored_images_and_machine_files_list_alone() {
        let (_temp, store, _) = catalogue(2);
        let cache = warm(&store);
        let files = [
            LibraryEntry::pending("/synthetic/alpha-machine.toml".into()),
            LibraryEntry::pending("/synthetic/beta-machine.toml".into()),
        ];
        let mut library = LibraryView::new(None);
        let ctx = egui::Context::default();
        let shown = |library: &mut LibraryView| {
            library.snapshot = Arc::new(snapshot(&cache, &library.query));
            let mut selected = None;
            let output =
                firmware::tests::frame(&ctx, egui::vec2(1200.0, 700.0), 0.0, vec![], |ui| {
                    library.show(ui, &files, &mut selected);
                });
            firmware::tests::texts(&output)
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
        };
        let all = shown(&mut library);
        for image in ["Synthetic 0", "Synthetic 1"] {
            assert!(all.iter().any(|t| t == image), "{image}: {all:?}");
        }
        assert!(
            !all.iter()
                .any(|t| t.contains("-machine") || t == "Machine file"),
            "{all:?}"
        );
        assert!(
            all.iter().any(|t| t.starts_with("2 of 2 images")),
            "{all:?}"
        );
        let mut selected = None;
        library.set_filter(Filter::MachineFiles, &mut selected);
        let machine_files = shown(&mut library);
        for file in ["alpha-machine", "beta-machine"] {
            assert!(
                machine_files.iter().any(|t| t == file),
                "{file}: {machine_files:?}"
            );
        }
        assert!(
            !machine_files.iter().any(|t| t.starts_with("Synthetic ")),
            "{machine_files:?}"
        );
        assert!(
            machine_files.iter().any(|t| t == "2 of 2 machine files"),
            "{machine_files:?}"
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
                aliases: Vec::new(),
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
            aliases: Vec::new(),
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
            zero: BTreeMap::new(),
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
            zero: BTreeMap::new(),
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
                    aliases: Vec::new(),
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
            built = facet_list(
                ui,
                "Labels",
                &values,
                &Filter::All,
                |_, v| v.into(),
                Filter::Facet,
            )
            .1;
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
        for _ in crate::test_wait::rounds() {
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
        for _ in crate::test_wait::rounds() {
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
        let bytes = image_store::media_set::Config {
            schema: 1,
            platform: "synthetic".into(),
            media: vec![image_store::media_set::Medium {
                id: "disk-1".into(),
                label: "Disk 1".into(),
                format: toml::Table::new(),
                blob: Digest::of(b"disk"),
            }],
            extras: Vec::new(),
        }
        .encode()
        .unwrap();
        let blob = store.import(bytes.as_slice(), None).unwrap();
        let config = Descriptor::new(mt::MEDIA_SET_CONFIG, blob.digest, blob.size);
        let set = Manifest::new_media_set("synthetic", config.clone(), machine.layers).unwrap();
        let set = store.import_manifest(&set).unwrap();
        let index = store.root().join("images/index.json");
        let text = std::fs::read_to_string(&index).unwrap();
        let prefix = text.trim_end().strip_suffix("]}").unwrap();
        std::fs::write(&index,format!("{prefix},{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"installation:one\"}}}}]}}",mt::MANIFEST,set.digest,set.size)).unwrap();
        let cache = warm(&store);
        let summary = cache.summary(&set.digest).unwrap();
        assert_eq!(summary.kind, image_store::summary::ArtifactKind::MediaSet);
        assert!(summary.machine.is_none());
        assert_eq!(summary.config, config.digest);
        assert_eq!(summary.media_set.as_ref().unwrap().media[0].label, "Disk 1");
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

    #[test]
    fn new_machine_creates_an_image_that_lists_validates_and_boots_the_open_bios() {
        use image_store::ConfigValidator;
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let worker = Worker::spawn(firmware.store.root().to_owned(), egui::Context::default());
        let next = |wanted: &dyn Fn(&Event) -> bool| loop {
            match worker.receive.recv_timeout(crate::test_wait::HANG_GUARD) {
                Ok(Event::Error(error)) => panic!("worker: {error}"),
                Ok(event) if wanted(&event) => break event,
                Ok(_) => continue,
                Err(error) => panic!("no worker event: {error}"),
            }
        };
        worker.send.send(Request::FirmwarePackages).unwrap();
        let Event::Packages(choices) = next(&|e| matches!(e, Event::Packages(_))) else {
            unreachable!()
        };
        let mut wizard = crate::new_machine::Wizard::new();
        wizard.set_store(choices.unwrap());
        wizard.edit(|s| s.hardware.disks[0] = Some(pc_image::create::Chs::new(1024, 16, 63)));
        wizard.title = "Open AT workbench".into();
        wizard.go(crate::new_machine::SUMMARY);
        assert!(wizard.private_notice().is_none());
        let (image, reference, expected) = wizard.request().unwrap();
        assert_eq!((reference.as_str(), &expected), ("open-at:1.0.0", &None));
        worker
            .send
            .send(Request::CreateImage(
                Box::new(image),
                reference.clone(),
                expected,
            ))
            .unwrap();
        let Event::Created(Ok(row)) = next(&|e| matches!(e, Event::Created(_))) else {
            panic!("Create failed")
        };
        // The new image appears in the library under its reference and title.
        next(&|e| {
            matches!(e, Event::Snapshot(snapshot) if snapshot.rows.iter().any(|r| {
                r.image.reference == reference
                    && r.image.summary.as_ref().is_some_and(|s| s.name == "Open AT workbench")
            }))
        });
        // It validates as a complete image, and creating it again is refused.
        let resolved = firmware
            .store
            .resolve(&reference, &pc_image::PcProvider)
            .unwrap();
        assert_eq!(resolved.image.digest, row.descriptor.digest);
        let config: toml::Table = std::str::from_utf8(&resolved.config)
            .unwrap()
            .parse()
            .unwrap();
        pc_image::PcProvider
            .validate(&config, &resolved.manifest.layers, &firmware.store)
            .unwrap();
        drop(worker);
        assert!(
            pc_image::create::create(&firmware.store, &wizard.image(), &reference, None).is_err()
        );
        // Start assembles the machine and the open BIOS runs its POST.
        let owner = firmware.store.workspace("first-start").unwrap();
        let prepared = pc_image::workspace::prepare_start(&owner, &reference, false).unwrap();
        let policy = assembly::ObservationPolicy {
            trace: device::TracePolicy::Off,
            routes: vec![
                assembly::ObservationRequest {
                    device: assembly::DeviceId::Post,
                    output: assembly::OutputKind::Bytes(device::ByteStreamKind::PostCheckpoints),
                    byte_limit: 4096,
                },
                assembly::ObservationRequest {
                    device: assembly::DeviceId::Vga,
                    output: assembly::OutputKind::TextScanout,
                    byte_limit: 0,
                },
            ],
            ..Default::default()
        };
        let mut machine = assembly::assemble_with_verified_bases(
            &prepared.machine,
            &policy,
            assembly::AssemblyOptions::default(),
            &prepared.bases,
        )
        .unwrap();
        let mut post = Vec::new();
        for step in 1..=60u64 {
            machine
                .run_session(&assembly::RunPolicy::natural(Some(step * 1_000_000), None))
                .unwrap();
            post.extend(
                machine
                    .observation
                    .drain_bytes(
                        assembly::DeviceId::Post,
                        device::ByteStreamKind::PostCheckpoints,
                    )
                    .unwrap(),
            );
            if post.contains(&0x00) {
                // Let INT 19h try A: (empty) and then C: through XTIDE.
                let at = {
                    use cpu::Cpu;
                    machine.machine.cpu().retired()
                };
                machine
                    .run_session(&assembly::RunPolicy::natural(Some(at + 30_000_000), None))
                    .unwrap();
                break;
            }
        }
        let capture = machine.capture_text(assembly::DeviceId::Vga).unwrap();
        let video::Frame::Text(grid) = &capture.frame else {
            panic!("expected VGA text")
        };
        // The open BIOS golden's checkpoint stream, through the option-ROM scan
        // (40h–43h, XTIDE at C8000h) to the boot loader (Kiselev errno.inc).
        assert_eq!(
            post,
            [
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x11, 0x20, 0x21, 0x22,
                0x24, 0x25, 0x30, 0x31, 0x40, 0x41, 0x42, 0x43, 0x00
            ]
        );
        // XTIDE identified the blank C: and the boot read its zero sector 0
        // through ATA, which has no boot signature.
        let rows = grid.rows_as_strings();
        let row = |text: &str| rows.iter().any(|row| row.contains(text));
        assert!(row("Master at 1F0h: UNEMU ATA-2 PIO DISK"), "{rows:#?}");
        assert!(row("Booting C"), "{rows:#?}");
        assert!(row("Boot sector not found"), "{rows:#?}");
        machine.finish_persistence().unwrap();
        assert!(firmware.temp.path().exists());
    }

    fn worker_events(worker: &Worker, wanted: &dyn Fn(&Event) -> bool) -> Event {
        loop {
            match worker.receive.recv_timeout(crate::test_wait::HANG_GUARD) {
                Ok(Event::Error(error)) => panic!("worker: {error}"),
                Ok(event) if wanted(&event) => return event,
                Ok(_) => continue,
                Err(error) => panic!("no worker event: {error}"),
            }
        }
    }

    #[test]
    fn new_machine_requests_are_served_while_a_session_runs() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let mut wizard = crate::new_machine::Wizard::new();
        wizard.set_store(firmware.choices());
        let (image, _, _) = wizard.request().unwrap();
        let first =
            pc_image::create::create(&firmware.store, &image, "open-at:1.0.0", None).unwrap();
        let worker = Worker::spawn(firmware.store.root().to_owned(), egui::Context::default());
        worker_events(&worker, &|e| matches!(e, Event::Recoveries(_)));
        let row = ImageRow {
            reference: "open-at:1.0.0".into(),
            aliases: Vec::new(),
            descriptor: first,
            summary: None,
            failure: None,
        };
        worker
            .send
            .send(Request::Launch(row, WorkspaceKind::Regular, None))
            .unwrap();
        worker_events(&worker, &|e| matches!(e, Event::Launched(..)));
        // The session loop answers New machine… instead of dropping it.
        worker.send.send(Request::FirmwarePackages).unwrap();
        let Event::Packages(Ok((packages, _, _))) =
            worker_events(&worker, &|e| matches!(e, Event::Packages(_)))
        else {
            panic!("package listing failed")
        };
        assert_eq!(packages.len(), 3);
        worker
            .send
            .send(Request::CreateImage(
                Box::new(image),
                "open-at:1.1.0".into(),
                None,
            ))
            .unwrap();
        let Event::Created(Ok(created)) =
            worker_events(&worker, &|e| matches!(e, Event::Created(_)))
        else {
            panic!("Create failed during a session")
        };
        assert_eq!(created.reference, "open-at:1.1.0");
        firmware
            .store
            .resolve("open-at:1.1.0", &pc_image::PcProvider)
            .unwrap();
        worker.send.send(Request::Finish(Finish::Retain)).unwrap();
        worker_events(&worker, &|e| matches!(e, Event::Finished));
    }

    #[test]
    fn create_results_and_package_refreshes_touch_only_the_wizard() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let mut view = LibraryView::synthetic(firmware.store.root().to_owned());
        view.query.filter = Filter::Firmware;
        view.query.text = "kiselev".into();
        // A Create is in flight when an Import ROM folder… refresh arrives.
        view.begin_new_machine();
        view.new_machine.as_mut().unwrap().busy = true;
        view.on_event(Event::Packages(Ok(firmware.choices())));
        let wizard = view.new_machine.as_ref().unwrap();
        assert!(wizard.busy && wizard.packages.is_some());
        // A generic job error goes to the library, never into the wizard.
        view.on_event(Event::Error("an unrelated job failed".into()));
        let wizard = view.new_machine.as_ref().unwrap();
        assert!(wizard.busy && wizard.error.is_none());
        assert!(view.error.is_some());
        let row = ImageRow {
            reference: "open-at:1.0.0".into(),
            aliases: Vec::new(),
            descriptor: Descriptor::new(mt::MANIFEST, Digest::of(b"created"), 7),
            summary: None,
            failure: None,
        };
        view.on_event(Event::Created(Ok(Box::new(row.clone()))));
        assert!(view.new_machine.is_none());
        // The created row is visible and selected in the machine view.
        assert_eq!(view.query.filter, Filter::All);
        assert!(view.query.text.is_empty());
        assert!(view.query.expanded.contains("open-at"));
        assert_eq!(
            view.selected.as_ref().map(|r| r.reference.as_str()),
            Some("open-at:1.0.0")
        );
        assert!(view.clear_file_selection);
        // A Create finishing after the wizard closed leaves another job's busy flag.
        view.busy = true;
        view.on_event(Event::Created(Ok(Box::new(row))));
        assert!(view.busy);
        // A refused Create lands in the open wizard.
        view.busy = false;
        view.begin_new_machine();
        view.new_machine.as_mut().unwrap().busy = true;
        view.on_event(Event::Created(Err("refused".into())));
        let wizard = view.new_machine.as_ref().unwrap();
        assert!(!wizard.busy);
        assert_eq!(wizard.error.as_deref(), Some("refused"));
    }

    #[test]
    fn create_binds_a_two_chip_system_bios_by_part() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let dir = firmware.temp.path().join("chips");
        std::fs::create_dir(&dir).unwrap();
        let even = vec![0xF4; 32768];
        let odd = vec![0x90; 32768];
        std::fs::write(dir.join("even.bin"), &even).unwrap();
        std::fs::write(dir.join("odd.bin"), &odd).unwrap();
        let assembled: Vec<u8> = even.iter().zip(&odd).flat_map(|(a, b)| [*a, *b]).collect();
        let (e, o, a) = (Digest::of(&even), Digest::of(&odd), Digest::of(&assembled));
        let config = image_store::firmware::Config::decode(
            format!(
                "schema=1\nplatform='ibm-pc-at'\nname='Synthetic two-chip BIOS'\n[fits]\nboards=['ibm-5170']\nmachine_classes=['at']\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=65536\nsha256='{}'\n[[roms.parts]]\nblob='{e}'\nsha256='{}'\nsize=32768\nposition='U27'\nlane='even'\n[[roms.parts]]\nblob='{o}'\nsha256='{}'\nsize=32768\nposition='U47'\nlane='odd'\n",
                a.hex(),
                e.hex(),
                o.hex()
            )
            .as_bytes(),
        )
        .unwrap();
        let candidate = pc_image::firmware_import::Candidate {
            catalogue_id: "two-chip".into(),
            config,
            sources: vec![dir.join("even.bin"), dir.join("odd.bin")],
            distribution: assembly::FirmwareDistribution::UserSupplied,
        };
        pc_image::firmware_import::publish(
            &firmware.store,
            &candidate,
            &[],
            pc_image::firmware::catalogue(),
        )
        .unwrap();
        let package = pc_image::create::packages(&firmware.store)
            .unwrap()
            .into_iter()
            .find(|p| p.config.name == "Synthetic two-chip BIOS")
            .unwrap();
        let image = pc_image::create::NewImage {
            hardware: pc_image::create::Hardware {
                board: assembly::BoardModel::Ibm5170,
                rate: (8, 13),
                fpu: false,
                conventional_kib: 640,
                extended_kib: 0,
                video: pc_image::create::Video::Mda,
                sound: None,
                serial: pc_image::create::Serial::None,
                ems: None,
                floppies: [None, None],
                ide_port: 0x1F0,
                disks: [None, None],
            },
            firmware: vec![pc_image::create::PackageRom::new(
                &package,
                &package.config.roms[0],
            )],
            title: None,
            description: None,
        };
        let created = pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::create::create(&firmware.store, &image, "chips:1.0.0", None)
        })
        .unwrap();
        let resolved = pc_image::test_support::with_synthetic_firmware(|| {
            firmware.store.resolve("chips:1.0.0", &pc_image::PcProvider)
        })
        .unwrap();
        assert_eq!(resolved.image.digest, created.digest);
        let config: toml::Table = std::str::from_utf8(&resolved.config)
            .unwrap()
            .parse()
            .unwrap();
        let bindings: Vec<_> = config["firmware"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| (b["key"].as_str().unwrap(), b["blob"].as_str().unwrap()))
            .collect();
        assert_eq!(
            bindings,
            [
                ("firmware.bios.parts[0]", e.to_string().as_str()),
                ("firmware.bios.parts[1]", o.to_string().as_str())
            ]
        );
        let bios = &config["machine"]["firmware"]["bios"];
        assert_eq!(bios["sha256"].as_str(), Some(a.hex()));
        assert_eq!(bios["parts"].as_array().unwrap().len(), 2);
        // The machine assembles the two chips into the BIOS it pins.
        let machine = pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::resolve(
                &firmware.store,
                "chips:1.0.0",
                &firmware.temp.path().join("session"),
            )
        })
        .unwrap();
        assert!(machine
            .images
            .iter()
            .any(|image| image.source == assembly::ImageSource::Inline(assembled.clone())));
    }

    fn until_view(view: &mut LibraryView, done: impl Fn(&LibraryView) -> bool) {
        let ctx = egui::Context::default();
        for _ in crate::test_wait::rounds() {
            view.poll(&ctx);
            if done(view) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the library worker did not finish: {:?}", view.error);
    }

    /// The settings sheet as the worker opens it, after `open_settings`.
    fn settings_opened(view: &mut LibraryView) {
        view.image_settings = None;
        view.open_settings();
        until_view(view, |view| view.image_settings.is_some());
    }
    /// Review changes… as the sheet sends it, waiting for the worker's check.
    fn review_checked(view: &mut LibraryView) {
        let settings = view.image_settings.as_mut().unwrap();
        let request = settings.begin_review();
        let row = settings.row.clone();
        view.send(Request::PlanRevision(
            row,
            request.revision,
            Box::new(request.machine),
        ));
        until_view(view, |view| {
            view.image_settings
                .as_ref()
                .and_then(|s| s.review.as_ref())
                .is_some_and(|r| r.plan.is_some())
        });
    }

    #[test]
    fn save_as_new_image_from_layers_keeps_title_and_firmware_sources() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&firmware);
        let before = firmware
            .store
            .resolve("open-at:1.0.0", &pc_image::PcProvider)
            .unwrap()
            .manifest
            .annotations;
        let worker = Worker::spawn(firmware.store.root().to_owned(), egui::Context::default());
        worker_events(&worker, &|e| matches!(e, Event::Recoveries(_)));
        let mut row = row;
        for (tag, title, base) in [
            ("open-at:1.1.0", None, "open-at:1.0.0"),
            (
                "open-at-dos:1.0.0",
                Some("Open AT with FreeDOS"),
                "open-at:1.1.0",
            ),
        ] {
            worker
                .send
                .send(Request::Layers(
                    row.clone(),
                    regular_name(&row.descriptor.digest),
                ))
                .unwrap();
            let Event::Layers(name, details, source) =
                worker_events(&worker, &|e| matches!(e, Event::Layers(..)))
            else {
                unreachable!()
            };
            // After the first save the library selects the saved image, whose
            // workspace the form then starts from.
            assert_eq!(source.reference, base);
            assert!(source.title.is_some());
            assert!(source.tags.iter().any(|(t, _)| t == "open-at:1.0.0"));
            let crate::layers_dialog::Action::Publish(workspace, target) =
                crate::layers_dialog::tests::reviewed_publication(
                    &name,
                    WorkspaceKind::Regular,
                    details.unwrap(),
                    *source,
                    tag,
                    false,
                    title,
                )
            else {
                panic!("no publication")
            };
            // A save without changes would alias its source; write a sector.
            let owner = firmware.store.workspace(&workspace).unwrap();
            let prepared = pc_image::workspace::prepare_start(&owner, base, false).unwrap();
            crate::image_settings::tests::write_c(&prepared, 1);
            drop(prepared);
            drop(owner);
            worker
                .send
                .send(Request::Publish(row.clone(), workspace, target))
                .unwrap();
            let Event::Published(saved) =
                worker_events(&worker, &|e| matches!(e, Event::Published(_)))
            else {
                unreachable!()
            };
            row = *saved;
            let after = firmware
                .store
                .resolve(tag, &pc_image::PcProvider)
                .unwrap()
                .manifest
                .annotations;
            assert_eq!(
                after[pc_image::workspace::TITLE],
                title.unwrap_or("Open AT workbench"),
                "{tag}"
            );
            assert_eq!(
                after["unemu.firmware.sources"], before["unemu.firmware.sources"],
                "{tag}"
            );
            assert!(
                after.contains_key(pc_image::workspace::BASE_DIGEST),
                "{tag}"
            );
        }
    }

    #[test]
    fn settings_revision_publishes_a_new_tag_and_leaves_the_source() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&firmware);
        let mut view = LibraryView::synthetic(firmware.store.root().to_owned());
        view.selected = Some(row.clone());
        settings_opened(&mut view);
        let settings = view.image_settings.as_mut().unwrap();
        assert!(!settings.read_only);
        settings.set(
            "memory.extended_kib",
            assembly::capabilities::Value::Integer(2048),
        );
        review_checked(&mut view);
        let review = view
            .image_settings
            .as_mut()
            .unwrap()
            .review
            .as_mut()
            .unwrap();
        let target = review.save.submit().unwrap();
        assert_eq!(target.reference, "open-at:1.1.0");
        let plan = review.plan.clone().unwrap().unwrap();
        view.send(Request::PublishRevision(Box::new(plan), target));
        until_view(&mut view, |view| view.image_settings.is_none());
        assert_eq!(
            view.selected.as_ref().map(|r| r.reference.as_str()),
            Some("open-at:1.1.0")
        );
        assert_eq!(
            firmware
                .store
                .tag_descriptor("open-at:1.0.0")
                .unwrap()
                .digest,
            row.descriptor.digest
        );
        let old = firmware
            .store
            .resolve("open-at:1.0.0", &pc_image::PcProvider)
            .unwrap()
            .manifest;
        let new = firmware
            .store
            .resolve("open-at:1.1.0", &pc_image::PcProvider)
            .unwrap()
            .manifest;
        assert_eq!(
            new.layers, old.layers,
            "every disk, ROM and metadata blob is shared"
        );
        assert_ne!(new.config.digest, old.config.digest);
        // A second revision cannot take the version tag the first one took.
        view.selected = Some(row);
        settings_opened(&mut view);
        view.image_settings.as_mut().unwrap().set(
            "memory.extended_kib",
            assembly::capabilities::Value::Integer(4096),
        );
        review_checked(&mut view);
        let review = view
            .image_settings
            .as_mut()
            .unwrap()
            .review
            .as_mut()
            .unwrap();
        assert_eq!(review.save.reference(), "open-at:1.2.0");
        review.save.set_tag("1.1.0".into());
        assert!(review.save.submit().is_none());
        assert!(matches!(
            review.save.prompt,
            Some(crate::save_image::Prompt::Refuse(_))
        ));
        // Running images open read-only.
        let running = view.selected.clone().unwrap();
        view.active = Some((running, "regular".into(), WorkspaceKind::Regular));
        settings_opened(&mut view);
        assert!(view.image_settings.as_ref().unwrap().read_only);
    }

    #[test]
    fn settings_review_and_save_never_open_the_store_on_the_ui_thread() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&firmware);
        let mut view = LibraryView::synthetic(firmware.store.root().to_owned());
        // The view's own root points nowhere: only the worker reaches the store.
        view.root = Some(firmware.temp.path().join("not-a-store"));
        view.selected = Some(row);
        settings_opened(&mut view);
        view.image_settings.as_mut().unwrap().set(
            "memory.extended_kib",
            assembly::capabilities::Value::Integer(2048),
        );
        review_checked(&mut view);
        let review = view
            .image_settings
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap();
        assert!(matches!(review.plan, Some(Ok(_))));
        assert!(review.save.tags.iter().any(|(t, _)| t == "open-at:1.0.0"));
        view.image_settings = None;
        view.begin_save_image();
        until_view(&mut view, |view| {
            view.layers.as_ref().is_some_and(|d| d.save.is_some())
        });
        let form = view.layers.as_ref().unwrap().save.as_ref().unwrap();
        assert_eq!(form.source, "open-at:1.0.0");
        assert_eq!(form.title.as_deref(), Some("Open AT workbench"));
    }

    #[test]
    fn a_refused_revision_asks_again_with_the_digest_another_program_moved_the_tag_to() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&firmware);
        firmware
            .store
            .update_tag(
                "open-at:latest",
                &row.descriptor,
                None,
                &pc_image::PcProvider,
            )
            .unwrap();
        let mut view = LibraryView::synthetic(firmware.store.root().to_owned());
        view.selected = Some(row.clone());
        settings_opened(&mut view);
        view.image_settings.as_mut().unwrap().set(
            "memory.extended_kib",
            assembly::capabilities::Value::Integer(2048),
        );
        review_checked(&mut view);
        let settings = view.image_settings.as_mut().unwrap();
        let review = settings.review.as_mut().unwrap();
        review.save.set_tag("latest".into());
        assert!(review.save.submit().is_none());
        let target = review.save.confirm_move().unwrap();
        assert_eq!(target.expected, Some(row.descriptor.digest.clone()));
        let plan = review.plan.clone().unwrap().unwrap();
        // Another program moves latest before Publish reaches the store.
        let other = pc_image::create::create(
            &firmware.store,
            &crate::image_settings::tests::open_new_image(&firmware, 2048),
            "open-at:other",
            None,
        )
        .unwrap();
        firmware
            .store
            .update_tag(
                "open-at:latest",
                &other,
                Some(&row.descriptor.digest),
                &pc_image::PcProvider,
            )
            .unwrap();
        view.busy = true;
        view.send(Request::PublishRevision(Box::new(plan), target));
        until_view(&mut view, |view| !view.busy);
        let review = view
            .image_settings
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap();
        assert!(review.error.is_some());
        assert_eq!(
            review.save.prompt,
            Some(crate::save_image::Prompt::Move(other.digest.clone())),
            "the question returns with the current digest"
        );
        assert_eq!(
            firmware
                .store
                .tag_descriptor("open-at:latest")
                .unwrap()
                .digest,
            other.digest
        );
    }

    #[test]
    fn an_interrupted_save_is_offered_and_finished_with_its_own_tag() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&firmware);
        let name = regular_name(&row.descriptor.digest);
        let owner = firmware.store.workspace(&name).unwrap();
        let prepared = pc_image::workspace::prepare_start(&owner, "open-at:1.0.0", false).unwrap();
        crate::image_settings::tests::write_c(&prepared, 7);
        drop(prepared);
        assert!(pc_image::workspace::publish_with_hook(
            &owner,
            "open-at-dos:1.0.0",
            None,
            |step| {
                if step == pc_image::workspace::PublicationStep::IntentStored {
                    Err(pc_image::Error::Invalid("injected interruption".into()))
                } else {
                    Ok(())
                }
            }
        )
        .is_err());
        drop(owner);
        let mut view = LibraryView::synthetic(firmware.store.root().to_owned());
        view.selected = Some(row);
        view.begin_save_image();
        until_view(&mut view, |view| {
            view.layers.as_ref().is_some_and(|d| d.save.is_some())
        });
        let dialog = view.layers.as_ref().unwrap();
        let Some(crate::layers_dialog::Action::Publish(workspace, target)) =
            crate::layers_dialog::tests::finish(dialog)
        else {
            panic!("no pending save offered")
        };
        assert_eq!(target.reference, "open-at-dos:1.0.0");
        assert_eq!(target.expected, None);
        view.busy = true;
        view.send(Request::Publish(
            view.selected.clone().unwrap(),
            workspace,
            target,
        ));
        until_view(&mut view, |view| !view.busy);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(firmware.store.tag_descriptor("open-at-dos:1.0.0").is_ok());
        assert_eq!(
            pc_image::workspace::pending_publication_at(&firmware.store, &name).unwrap(),
            None
        );
    }

    #[test]
    fn image_settings_and_its_review_render_without_a_window() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&firmware);
        let mut settings = crate::image_settings::tests::opened(&firmware, row, false);
        settings.set(
            "memory.extended_kib",
            assembly::capabilities::Value::Integer(2048),
        );
        let ctx = egui::Context::default();
        let texts = |output: &egui::FullOutput| -> Vec<String> {
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        };
        for tab in assembly::capabilities::Group::ALL {
            settings.tab = tab;
            for frame in 0..2 {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    assert!(matches!(
                        crate::settings_sheet::show_image(ui.ctx(), &mut settings, true),
                        crate::settings_sheet::ImageAction::None
                    ));
                });
                output.textures_delta.clear();
                if tab == assembly::capabilities::Group::Memory && frame == 1 {
                    let texts = texts(&output);
                    assert!(
                        texts
                            .iter()
                            .any(|t| t.contains("1 change — a new revision on review")),
                        "{texts:?}"
                    );
                    assert!(texts.iter().any(|t| t == "Was: 1024"), "{texts:?}");
                }
            }
        }
        crate::image_settings::tests::reviewed(&mut settings, &firmware.store);
        crate::library_test_support::settled_modal(
            egui::Id::unique("image-revision"),
            egui::vec2(1040.0, 700.0),
            |ctx| {
                let _ = crate::settings_sheet::show_review(ctx, &mut settings, false, true);
            },
        );
        let mut last = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(matches!(
                    crate::settings_sheet::show_review(ui.ctx(), &mut settings, false, true),
                    crate::settings_sheet::ReviewAction::None
                ));
            });
            output.textures_delta.clear();
            last = texts(&output);
        }
        for text in [
            "Publish new image revision",
            "Hardware changes — flattened machine configuration",
            "CMOS bytes recomputed, as SETUP would",
            "Image and media bindings",
        ] {
            assert!(last.iter().any(|t| t.contains(text)), "{text}: {last:?}");
        }
        // Escape on the sheet with edits asks before discarding them.
        settings.review = None;
        let escape = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(escape.clone(), |ui| {
            assert!(matches!(
                crate::settings_sheet::show_image(ui.ctx(), &mut settings, true),
                crate::settings_sheet::ImageAction::None
            ));
        });
        output.textures_delta.clear();
        assert!(settings.discard_prompt);
        let mut output = ctx.run_ui(escape, |ui| {
            assert!(matches!(
                crate::settings_sheet::show_image(ui.ctx(), &mut settings, true),
                crate::settings_sheet::ImageAction::None
            ));
        });
        output.textures_delta.clear();
        assert!(!settings.discard_prompt, "Escape keeps editing");
        assert!(settings.dirty());
    }

    /// WCAG 2.x contrast ratio between two opaque sRGB colours.
    fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
        let luminance = |c: egui::Color32| {
            let channel = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn selected_rows_and_items_are_legible_in_every_skin_and_theme() {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let section = text.galley.job.sections[0].format.color;
                    let color = text.override_text_color.unwrap_or(
                        if section == egui::Color32::PLACEHOLDER {
                            text.fallback_color
                        } else {
                            section
                        },
                    );
                    out.push((text.galley.text().to_owned(), color));
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => (),
            }
        }
        let cells: [String; 9] = std::array::from_fn(|i| format!("cell {i}"));
        for classic in [false, true] {
            for dark in [false, true] {
                let visuals = crate::shell::skin_visuals(classic, dark);
                let fill = visuals.selection.bg_fill;
                let ctx = egui::Context::default();
                ctx.set_visuals(visuals);
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    draw_cells(ui, &cells, &[80.0; 9], true, false, Lead::default());
                    ui.add(egui::Button::selectable(true, "selected item"));
                });
                output.textures_delta.clear();
                let mut texts = Vec::new();
                for clipped in &output.shapes {
                    walk(&clipped.shape, &mut texts);
                }
                assert_eq!(texts.len(), 10, "{texts:?}");
                for (text, color) in texts {
                    let ratio = contrast(color, fill);
                    assert!(
                        ratio >= 4.5,
                        "classic {classic}, dark {dark}: {text:?} in {color:?} on {fill:?} is {ratio:.2}:1"
                    );
                }
            }
        }
    }

    /// Every shape `run` paints in the light theme, flattened.
    fn painted(run: impl FnMut(&mut egui::Ui)) -> Vec<egui::Shape> {
        fn flatten(shape: egui::Shape, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| flatten(s, out)),
                shape => out.push(shape),
            }
        }
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
        ctx.set_visuals(egui::Visuals::light());
        let mut output = ctx.run_ui(egui::RawInput::default(), run);
        output.textures_delta.clear();
        let mut shapes = Vec::new();
        for clipped in output.shapes {
            flatten(clipped.shape, &mut shapes);
        }
        shapes
    }

    #[test]
    fn the_tree_marks_the_collections_the_machines_mockup_marks() {
        assert_eq!(tree_icon(&Filter::All), Some(Icon::Image));
        assert_eq!(tree_icon(&Filter::MachineFiles), Some(Icon::MachineFile));
        assert_eq!(tree_icon(&Filter::Media), Some(Icon::Media));
        assert_eq!(tree_icon(&Filter::Firmware), Some(Icon::Firmware));
        assert_eq!(tree_icon(&Filter::Recent), None);
        assert_eq!(tree_icon(&Filter::Favorites), None);
    }

    #[test]
    fn image_rows_lead_with_the_image_icon_and_flag_unreadable_images() {
        let image = ImageRow {
            reference: "games:1".into(),
            aliases: Vec::new(),
            descriptor: Descriptor::new(mt::MANIFEST, Digest::of(b"games"), 7),
            summary: None,
            failure: Some("missing blob".into()),
        };
        let row = ViewRow {
            image,
            group: None,
            child: true,
        };
        assert_eq!(
            image_lead(&row),
            Lead {
                icon: Some(Icon::Image),
                child: true,
                warning: true,
                favourite: false,
            }
        );
        // The name cell carries the name alone; the icon and badges are drawn.
        assert_eq!(row_cells(&row)[0], "games:1");
    }

    /// The name cell draws the icon before the name, set in under a group,
    /// and the warning and favourite badges after it, in the mock-up's colours.
    #[test]
    fn the_name_cell_draws_its_icon_and_badges() {
        let cells: [String; 9] = std::array::from_fn(|i| format!("cell {i}"));
        let lead = Lead {
            icon: Some(Icon::Image),
            child: true,
            warning: true,
            favourite: true,
        };
        let visuals = egui::Visuals::light();
        let shapes = painted(|ui| {
            draw_cells(ui, &cells, &[200.0; 9], false, false, lead);
        });
        let name = shapes
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text().starts_with("cell 0") => Some(text),
                _ => None,
            })
            .expect("the name cell");
        assert_eq!(name.galley.text(), "cell 0 ⚠ ★");
        let sections = &name.galley.job.sections;
        assert_eq!(sections[1].format.color, visuals.warn_fg_color);
        assert_eq!(sections[2].format.color, crate::shell_icons::STAR_GOLD);
        // Left-aligned: the name starts after the group indent and the icon.
        assert_eq!(name.pos.x, CHILD_INDENT + NAME_ICON_ROOM);
        let icon: Vec<_> = shapes
            .iter()
            .filter(|shape| !matches!(shape, egui::Shape::Text(_)))
            .map(|shape| shape.visual_bounding_rect())
            .filter(|rect| rect.is_positive() && rect.max.x <= name.pos.x)
            .collect();
        // The image icon's three outlines, in the space before the name.
        assert_eq!(icon.len(), 3, "{icon:?}");
        assert!(
            icon.iter()
                .all(|rect| rect.min.x >= CHILD_INDENT && rect.width() <= 17.0),
            "{icon:?}"
        );
        // Selected, the icon and badges turn to the selection text colour.
        let shapes = painted(|ui| {
            draw_cells(ui, &cells, &[200.0; 9], true, false, lead);
        });
        let selected = egui::Visuals::light().selection.stroke.color;
        let name = shapes
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text().starts_with("cell 0") => Some(text),
                _ => None,
            })
            .unwrap();
        assert!(name
            .galley
            .job
            .sections
            .iter()
            .all(|section| section.format.color == selected));
    }

    #[test]
    fn the_empty_library_shows_the_get_templates_picture() {
        let visuals = egui::Visuals::light();
        let shapes = painted(|ui| empty_library(ui, &mut Vec::new()));
        // The picture's screen: a 28×19 rectangle in the selection colour.
        assert!(shapes.iter().any(|shape| matches!(
            shape,
            egui::Shape::Rect(rect)
                if rect.fill == visuals.selection.bg_fill
                    && rect.rect.size() == egui::vec2(28.0, 19.0)
        )));
        assert!(shapes.iter().any(|shape| matches!(
            shape,
            egui::Shape::Text(text) if text.galley.text() == "No machine images yet"
        )));
    }
}
