// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware collection of the desktop library; machine-images §§5.3 and 7.
//! Rows, facets, chip layout and "Used by" come from cached package summaries,
//! so listing and selection never read ROM bytes. ROM-folder scans and imports
//! run on the library worker with the matching, refusals and idempotence of
//! `image firmware import`.
//!
//! Split BIOS sets follow the IBM Personal Computer AT Technical Reference,
//! Section 1, System Board ROM: an even and an odd chip interleave into one
//! logical ROM, so a chip's offset counts assembled bytes.

use super::firmware_metadata::{
    layer_text, provenance_lines, Catalogue, Draft, EditDialog, Facts, Fields, PackageDetails,
    Rights, Verdict,
};
use super::{human_size, Filter, ImageRow, LibraryView, Listed, Query};
use assembly::firmware_discovery::DiscoveryControl;
use image_store::{
    firmware::Rom,
    license, media_type,
    summary::{FirmwareSummary, ImageSummary, SummaryCache},
    Descriptor, Digest, Store,
};
use pc_image::firmware_import::{self, Candidate, Statement};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Tree facet groups of the Firmware collection.
pub(super) const FACETS: [&str; 4] = ["Role", "Board", "Machine class", "Redistribution"];
pub(super) fn is_facet(kind: &str) -> bool {
    FACETS.contains(&kind)
}
pub(super) fn facet_label(kind: &str, value: &str) -> String {
    match kind {
        "Role" => role_label(value).into(),
        "Machine class" => value.to_uppercase(),
        "Redistribution" => redistribution_label(value == OPEN).into(),
        _ => value.into(),
    }
}
pub(super) fn role_label(role: &str) -> &str {
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

/// A ROM layer, stored raw or as a `+zstd` frame (machine-images §2).
fn is_rom(descriptor: &Descriptor) -> bool {
    descriptor.decoded_media_type() == media_type::FIRMWARE
}
/// The raw identity that configs and ROM pins use, whether stored raw or compressed.
fn decoded_identity(descriptor: &Descriptor) -> Option<Digest> {
    descriptor.decoded_digest().ok()
}
fn pin(sha256: &str) -> Option<Digest> {
    format!("sha256:{sha256}").parse().ok()
}

/// Package redistribution class from the ROM descriptors' annotations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Redistribution {
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
    fn value(&self) -> &'static str {
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
pub(super) struct FirmwareRow {
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
const NO_SOURCE_BADGE: &str = "⚠ No source · private only";
const WORKER_UNAVAILABLE: &str = "Image store worker not available";
impl FirmwareRow {
    fn firmware(&self) -> Option<&FirmwareSummary> {
        self.image.summary.as_ref()?.firmware.as_ref()
    }
    pub fn same(&self, other: &Self) -> bool {
        self.image.same(&other.image)
    }
}

/// Raw firmware identities of local machine images, mapped to their tags.
#[derive(Default)]
pub(super) struct MachineRoms(BTreeMap<Digest, BTreeSet<String>>);
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
    fn users(&self, firmware: &FirmwareSummary) -> Vec<String> {
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

pub(super) fn add_facets(facets: &mut BTreeMap<String, BTreeSet<String>>, summary: &ImageSummary) {
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
pub(super) fn rows(
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

pub(super) const COLUMNS: [&str; 9] = [
    "Name",
    "Roles",
    "Chips",
    "Vendor",
    "Version / date",
    "Fits",
    "Catalogue id",
    "Redistribution",
    "Size",
];
const WIDTHS: [f32; 9] = [210.0, 110.0, 90.0, 100.0, 160.0, 120.0, 170.0, 150.0, 80.0];

fn roles(f: &FirmwareSummary) -> String {
    f.roles
        .iter()
        .map(|r| role_label(r))
        .collect::<Vec<_>>()
        .join(", ")
}
fn chips(rom: &Rom) -> String {
    if rom.parts.is_empty() {
        "single image".into()
    } else {
        format!("{} chips", rom.parts.len())
    }
}
fn version_date(f: &FirmwareSummary) -> String {
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
pub(super) fn short(hash: &str) -> String {
    if hash.len() > 12 {
        format!("{}…{}", &hash[..8], &hash[hash.len() - 4..])
    } else {
        hash.into()
    }
}

/// A package row's icon and its warning, before and after the name.
pub(super) fn lead(row: &FirmwareRow) -> super::Lead {
    super::Lead {
        icon: Some(crate::shell_icons::Icon::Firmware),
        warning: row.image.failure.is_some(),
        ..super::Lead::default()
    }
}

pub(super) fn cells(row: &FirmwareRow) -> [String; 9] {
    let name = row.image.name().to_owned();
    let Some(f) = row.firmware() else {
        let mut cells: [String; 9] = Default::default();
        cells[0] = name;
        cells[8] = if row.image.failure.is_some() {
            "Unavailable"
        } else {
            "Pending"
        }
        .into();
        return cells;
    };
    [
        name,
        roles(f),
        f.roms.iter().map(chips).collect::<Vec<_>>().join(" + "),
        f.vendor.clone().unwrap_or_else(|| "Unknown".into()),
        version_date(f),
        fits(f),
        f.catalogue_ids.join(", "),
        if row.source_missing {
            format!("{} {NO_SOURCE_BADGE}", row.redistribution.label())
        } else if !f.sources.is_empty() {
            format!("{} · with source archive", row.redistribution.label())
        } else {
            row.redistribution.label()
        },
        row.image
            .summary
            .as_ref()
            .map(|s| human_size(s.stored_size))
            .unwrap_or_default(),
    ]
}
fn sort_value(row: &FirmwareRow, column: usize) -> String {
    match column {
        8 => format!(
            "{:020}",
            row.image.summary.as_ref().map_or(0, |s| s.stored_size)
        ),
        column => cells(row)
            .get(column)
            .cloned()
            .unwrap_or_default()
            .to_lowercase(),
    }
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_lowercase().chain(chars).collect::<String>()
    })
}
fn assembly_hint(rom: &Rom) -> String {
    let role = lower_first(role_label(&rom.role));
    if rom.parts.is_empty() {
        format!(
            "One {role} · {} bytes · SHA-256 {}.",
            rom.size,
            short(&rom.sha256)
        )
    } else {
        format!(
            "One logical {role} · {} assembled bytes · SHA-256 {}. Each pair interleaves \
             even/odd bytes; offsets are in assembled bytes.",
            rom.size,
            short(&rom.sha256)
        )
    }
}
fn chip_box(
    ui: &mut egui::Ui,
    title: String,
    placement: String,
    size: u64,
    sha256: &str,
    license: &str,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.strong(title);
            ui.label(placement);
            ui.small(format!("{size} bytes"));
            ui.monospace(short(sha256)).on_hover_text(sha256);
            ui.small(license);
        });
    });
}

/// What the package pane asks the library view to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneAction {
    None,
    OpenReadme,
    EditMetadata,
    /// Edit metadata… with the source archive picker.
    AddSource,
}

/// The licence of the layer holding the raw identity `digest`, if open.
fn layer_license(summary: &ImageSummary, digest: &Digest) -> Option<String> {
    summary
        .storage_blobs
        .iter()
        .find(|d| decoded_identity(d).as_ref() == Some(digest))
        .and_then(license::open_license)
        .map(|l| l.as_str().to_owned())
}
fn readme_layer(summary: &ImageSummary) -> Option<&Descriptor> {
    summary
        .storage_blobs
        .iter()
        .find(|d| d.decoded_media_type() == media_type::MARKDOWN)
}
/// The push guard's notice for a copyleft open ROM without its source layer.
pub(super) fn guard_notice(license: &str) -> String {
    format!(
        "⚠ Push guard: private registries only. {license} is a copyleft licence and this \
         package has no source archive layer. Image › Push… refuses unemu/ and every other \
         non-private registry, for this package and for any machine image that embeds its ROM. \
         Private registries and private exports still work."
    )
}

/// The selected-package pane, drawn from the summary, plus what the worker
/// read for it (`package`: the README text and the guard's Config and Push
/// lines). A firmware package has no launch action, so it never offers Start.
pub(super) fn details(
    ui: &mut egui::Ui,
    row: &FirmwareRow,
    package: Option<&Result<PackageDetails, String>>,
    can_edit: bool,
) -> PaneAction {
    let mut action = PaneAction::None;
    let image = &row.image;
    let (Some(f), Some(summary)) = (row.firmware(), image.summary.as_ref()) else {
        ui.strong(image.name());
        ui.label(image.references());
        ui.label(
            image
                .failure
                .clone()
                .unwrap_or_else(|| "Reading package summary…".into()),
        );
        return action;
    };
    let license = row.redistribution.licenses.join(", ");
    let warn = ui.visuals().warn_fg_color;
    ui.strong(image.name());
    ui.label(format!(
        "{} · local package · {}",
        roles(f),
        image.references()
    ));
    match &f.description {
        Some(description) => {
            ui.add(egui::Label::new(description).wrap());
        }
        None => {
            ui.weak("No description.");
        }
    }
    ui.horizontal_wrapped(|ui| {
        if f.labels.is_empty() {
            ui.weak("No labels.");
        }
        for label in &f.labels {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(4, 1))
                .show(ui, |ui| ui.monospace(label));
        }
    });
    if row.source_missing {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.colored_label(warn, guard_notice(&license));
            if ui
                .add_enabled(can_edit, egui::Button::new("Add source archive…"))
                .clicked()
            {
                action = PaneAction::AddSource;
            }
        });
    }
    let checked = |line: fn(&PackageDetails) -> String| match package {
        None => "Checking…".to_owned(),
        Some(Err(error)) => error.clone(),
        Some(Ok(details)) => line(details),
    };
    ui.columns(3, |columns| {
        let facts = &mut columns[0];
        egui::Grid::new("firmware-facts")
            .num_columns(2)
            .show(facts, |ui| {
                let digest = image.descriptor.digest.hex();
                let redistribution = if row.source_missing {
                    format!("{} {NO_SOURCE_BADGE}", row.redistribution.label())
                } else {
                    row.redistribution.label()
                };
                for (label, value, hover) in [
                    (
                        "Vendor",
                        f.vendor.clone().unwrap_or_else(|| "Unknown".into()),
                        None,
                    ),
                    ("Version / date", version_date(f), None),
                    ("Catalogue id", f.catalogue_ids.join(", "), None),
                    ("Package SHA-256", short(digest), Some(digest)),
                    ("Redistribution", redistribution, None),
                    (
                        "Config",
                        checked(|d| {
                            if d.config_open {
                                "Open — every layer is open".into()
                            } else {
                                "User-supplied — follows its user-supplied layers".into()
                            }
                        }),
                        None,
                    ),
                    ("Push", checked(super::firmware_metadata::push_text), None),
                    ("Stored", human_size(summary.stored_size), None),
                ] {
                    ui.weak(label);
                    let response = ui.label(value);
                    if let Some(hover) = hover {
                        response.on_hover_text(hover);
                    }
                    ui.end_row();
                }
            });
        facts.strong("Provenance");
        egui::Grid::new("firmware-provenance")
            .num_columns(2)
            .show(facts, |ui| {
                let [source, from, date] = provenance_lines(f.provenance.as_ref());
                for (label, value) in [("Source", source), ("Details", from), ("Date", date)] {
                    ui.weak(label);
                    ui.add(egui::Label::new(value).wrap());
                    ui.end_row();
                }
            });
        facts.horizontal_wrapped(|ui| {
            ui.add_enabled(false, egui::Button::new("Use in machine…"))
                .on_disabled_hover_text(
                    "Binding a package into a machine draft is not available yet.",
                );
            ui.add_enabled(false, egui::Button::new("Export"))
                .on_disabled_hover_text("Package export is not available yet.");
            if ui
                .add_enabled(can_edit, egui::Button::new("Edit metadata…"))
                .clicked()
            {
                action = PaneAction::EditMetadata;
            }
        });
        facts.weak("Edit metadata… creates a new package revision; ROM blobs unchanged.");
        facts.weak(
            "A machine embeds its ROM blobs and records the source package digest. \
             Source folders are not needed after import.",
        );

        let layout = &mut columns[1];
        layout.strong("Chip layout");
        for rom in &f.roms {
            layout.horizontal_wrapped(|ui| {
                if rom.parts.is_empty() {
                    let title = match (&rom.position, &rom.part_number) {
                        (Some(position), Some(number)) => format!("{position} · {number}"),
                        (Some(label), None) | (None, Some(label)) => label.clone(),
                        (None, None) => "Single image".into(),
                    };
                    let license = rom
                        .blob
                        .as_ref()
                        .and_then(|blob| layer_license(summary, blob));
                    chip_box(
                        ui,
                        title,
                        role_label(&rom.role).into(),
                        rom.size,
                        &rom.sha256,
                        &layer_text(license.as_deref()),
                    );
                }
                for part in &rom.parts {
                    let title = [&part.position, &part.part_number]
                        .into_iter()
                        .flatten()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    chip_box(
                        ui,
                        if title.is_empty() {
                            "Chip".into()
                        } else {
                            title
                        },
                        format!(
                            "{} · {} @ {}",
                            role_label(&rom.role),
                            part.lane,
                            part.offset
                        ),
                        part.size,
                        &part.sha256,
                        &layer_text(layer_license(summary, &part.blob).as_deref()),
                    );
                }
            });
            layout.add(egui::Label::new(egui::RichText::new(assembly_hint(rom)).weak()).wrap());
        }
        layout.strong("Fits — boards, machine classes and required devices");
        if f.boards.is_empty() && f.machine_classes.is_empty() {
            layout.label("Unspecified (not universal)");
        }
        for board in &f.boards {
            layout.label(format!("Board: {board}"));
        }
        for class in &f.machine_classes {
            layout.label(format!("Machine class: {}", class.to_uppercase()));
        }
        let mut requires: Vec<&String> = f.roms.iter().flat_map(|r| &r.requires).collect();
        requires.sort();
        requires.dedup();
        for device in requires {
            layout.label(format!("Requires: {device}"));
        }
        layout.strong("Used by — machines in this library");
        if row.used_by.is_empty() {
            layout.label("No local machine image embeds these ROMs.");
        }
        for reference in &row.used_by {
            layout.label(reference);
        }

        let extras = &mut columns[2];
        extras.strong("Documents (extras)");
        if f.extras.is_empty() {
            extras.label("No documents attached.");
        }
        for extra in &f.extras {
            extras.label(&extra.filename);
            extras.small(format!(
                "{} · {} · {}",
                super::firmware_metadata::extra_kind(extra.kind),
                layer_text(layer_license(summary, &extra.blob).as_deref()),
                human_size(extra.size)
            ));
        }
        extras.strong("Source archive");
        match f.sources.first() {
            Some(source) => {
                extras.label(&source.filename);
                extras.small(format!(
                    "Source archive · {} · {} · {}",
                    source.format.as_str(),
                    layer_text(layer_license(summary, &source.blob).as_deref()),
                    human_size(source.size)
                ));
            }
            None if row.source_missing => {
                extras.label("No source archive");
                extras.colored_label(
                    warn,
                    format!("Required to publish {license} firmware outside private registries."),
                );
                if extras
                    .add_enabled(can_edit, egui::Button::new("Add source archive…"))
                    .clicked()
                {
                    action = PaneAction::AddSource;
                }
            }
            None if row.redistribution.open => {
                extras.weak(format!("None — optional for {license}."));
            }
            None => {
                extras.weak("None — not needed for user-supplied firmware.");
            }
        }
        extras.strong("README");
        let text = match package {
            Some(Ok(details)) => details.readme.as_deref(),
            _ => None,
        };
        match (&f.readme, readme_layer(summary)) {
            (Some(readme), layer) => {
                extras.label("README.md");
                extras.small(format!(
                    "README · UTF-8 Markdown · {} · {}",
                    layer_text(
                        layer
                            .and_then(license::open_license)
                            .map(|l| l.as_str().to_owned())
                            .as_deref()
                    ),
                    human_size(readme.size)
                ));
                if extras
                    .add_enabled(text.is_some(), egui::Button::new("Open README"))
                    .clicked()
                {
                    action = PaneAction::OpenReadme;
                }
                if let Some(text) = text {
                    let mut lines: Vec<&str> = text.lines().take(9).collect();
                    lines.push("…");
                    extras.add(
                        egui::Label::new(egui::RichText::new(lines.join("\n")).monospace().small())
                            .wrap(),
                    );
                }
            }
            (None, _) => {
                extras.label("No README layer");
                extras.small(
                    "Imported by an earlier version, before README and source layers existed. \
                     Edit metadata… adds one in a new revision.",
                );
                extras.add_enabled(false, egui::Button::new("Open README"));
            }
        }
    });
    if let Some(reason) = &image.failure {
        ui.colored_label(warn, reason);
    }
    action
}

/// Which statement a picked source archive belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SourceTarget {
    /// A set of the Import ROM folder… review, by catalogue id, and the
    /// scanned folder the picker opens in.
    Import(String, PathBuf),
    /// The package in Edit metadata….
    Edit,
}

impl LibraryView {
    /// The ROM-folder import shares the single library worker, which stays
    /// occupied for the whole of a running image session.
    pub fn can_import_firmware(&self) -> bool {
        self.worker.is_some()
            && !self.worker_dead
            && !self.busy
            && self.active.is_none()
            && self.firmware_import.is_none()
    }
    /// Edit metadata… reads, checks and publishes on the library worker.
    pub fn can_edit_firmware(&self) -> bool {
        self.worker.is_some()
            && !self.worker_dead
            && !self.busy
            && self.active.is_none()
            && self.firmware_edit.is_none()
            && self
                .firmware_selected
                .as_ref()
                .is_some_and(|row| row.firmware().is_some())
    }
    /// A stopped worker never answers, so a waiting import or metadata
    /// dialog stops waiting and offers Close.
    pub(super) fn fail_firmware_import(&mut self) {
        if let Some(dialog) = self
            .firmware_import
            .as_mut()
            .filter(|d| d.outcomes.is_none() && d.error.is_none())
        {
            dialog.failed("Image store worker stopped; nothing more was imported".into());
        }
        if let Some(dialog) = &mut self.firmware_edit {
            dialog.stop("Image store worker stopped; no new revision was published".into());
        }
        if let Some((_, details @ None)) = &mut self.firmware_package {
            *details = Some(Err("Image store worker stopped".into()));
        }
    }
    /// Start the scan of an already picked folder; hashing reuses `index`, the
    /// persistent ROM hash index, as the command-line import does.
    pub fn begin_firmware_import(&mut self, folder: PathBuf, index: Option<PathBuf>) {
        let control = DiscoveryControl::default();
        self.firmware_import = Some(ImportDialog::new(folder.clone(), control.clone()));
        self.busy = true;
        self.status = Some("Scanning ROM folder…".into());
        self.send(super::Request::ScanFirmware(folder, index, control));
    }
    /// The import request for the reviewed picks, if the dialog allows one.
    pub(super) fn import_picks(&mut self) -> Option<super::Request> {
        let dialog = self.firmware_import.as_mut()?;
        if !dialog.can_import() {
            return None;
        }
        dialog.importing = true;
        self.busy = true;
        self.status = Some("Importing firmware packages…".into());
        Some(super::Request::ImportFirmware(dialog.statements()))
    }
    /// Edit metadata… on the selected package; with `pick_source`, the
    /// source archive picker follows once the package is read.
    pub fn begin_firmware_edit(&mut self, pick_source: bool) {
        if !self.can_edit_firmware() {
            return;
        }
        let Some(row) = self.firmware_selected.clone() else {
            return;
        };
        self.firmware_edit = Some(EditDialog::new(
            row.image.reference.clone(),
            row.image.name().into(),
            pick_source,
        ));
        self.send(super::Request::OpenFirmwareEdit(row.image.reference));
    }
    /// The host picker's answer for a source archive.
    pub fn firmware_source_picked(&mut self, target: SourceTarget, path: Option<PathBuf>) {
        let Some(path) = path else {
            return;
        };
        let draft = match target {
            SourceTarget::Import(id, _) => self
                .firmware_import
                .as_mut()
                .and_then(|d| d.drafts.get_mut(&id)),
            SourceTarget::Edit => self.firmware_edit.as_mut().and_then(|d| d.draft.as_mut()),
        };
        if let Some(draft) = draft {
            draft.fields.source = Some(path);
            draft.revision += 1;
        }
        self.request_firmware_previews();
    }
    /// Ask the worker for the library's verdict on every statement that
    /// changed since its last preview, one preview per statement at a time.
    pub(super) fn request_firmware_previews(&mut self) {
        if self.worker.is_none() || self.worker_dead || self.active.is_some() {
            return;
        }
        let mut requests = Vec::new();
        if let Some(dialog) = self.firmware_import.as_mut().filter(|d| d.reviewing()) {
            for (id, draft) in &mut dialog.drafts {
                if let Some((revision, statement)) = draft.wants_preview() {
                    requests.push(super::Request::PreviewFirmware(
                        id.clone(),
                        revision,
                        Box::new(statement),
                    ));
                }
            }
        }
        if let Some(dialog) = self
            .firmware_edit
            .as_mut()
            .filter(|d| !d.publishing && !d.stopped)
        {
            if let (Some(target), Some(draft)) = (dialog.target.clone(), dialog.draft.as_mut()) {
                if let Some((revision, statement)) = draft.wants_preview() {
                    requests.push(super::Request::PreviewFirmwareEdit(
                        dialog.tag.clone(),
                        target,
                        revision,
                        Box::new(statement),
                    ));
                }
            }
        }
        for request in requests {
            self.send(request);
        }
    }
    /// Publish new revision, if the editor's statement was accepted.
    pub(super) fn publish_firmware_edit(&mut self) -> Option<super::Request> {
        let dialog = self.firmware_edit.as_mut()?;
        if !dialog.footer().0 {
            return None;
        }
        let draft = dialog.draft.as_ref()?;
        dialog.publishing = true;
        dialog.error = None;
        self.busy = true;
        self.status = Some("Publishing the new package revision…".into());
        Some(super::Request::PublishFirmwareEdit(
            dialog.tag.clone(),
            dialog.target.clone()?,
            Box::new(draft.statement()),
        ))
    }
    /// Selecting a package drops any machine selection: with neither an image
    /// tag nor a machine file selected the shell has nothing to Start.
    pub(super) fn select_firmware(&mut self, row: &FirmwareRow, selected_file: &mut Option<usize>) {
        self.selected = None;
        self.readiness = None;
        self.pending_launch = None;
        *selected_file = None;
        self.show_package(row.clone());
    }
    /// Select a package and ask the worker for what its pane reads from the store.
    pub(super) fn show_package(&mut self, row: FirmwareRow) {
        let digest = row.image.descriptor.digest.clone();
        let reference = row.image.reference.clone();
        self.firmware_selected = Some(row);
        if self.worker.is_none() || self.worker_dead || self.active.is_some() {
            self.firmware_package = Some((digest, Some(Err(WORKER_UNAVAILABLE.into()))));
            return;
        }
        self.firmware_package = Some((digest.clone(), None));
        self.send(super::Request::ReadFirmwarePackage(reference, digest));
    }
    /// Open README, Edit metadata… and Add source archive… from the pane.
    pub(super) fn pane_action(&mut self, row: &FirmwareRow, action: PaneAction) {
        match action {
            PaneAction::None => (),
            PaneAction::OpenReadme => {
                if let Some(Ok(PackageDetails {
                    readme: Some(text), ..
                })) = self.package_details()
                {
                    self.firmware_readme = Some(super::firmware_metadata::ReadmeDialog {
                        name: row.image.name().into(),
                        text: text.clone(),
                    });
                }
            }
            PaneAction::EditMetadata => self.begin_firmware_edit(false),
            PaneAction::AddSource => self.begin_firmware_edit(true),
        }
    }
    /// Read the selected package's pane again, for marks that may have changed.
    pub(super) fn refresh_package(&mut self) {
        if let Some(row) = self.firmware_selected.clone() {
            self.show_package(row);
        }
    }
    /// A reading refused while the worker was busy with a session is asked
    /// again once the worker is free.
    pub(super) fn retry_package(&mut self) {
        let unavailable = matches!(
            &self.firmware_package,
            Some((_, Some(Err(error)))) if error == WORKER_UNAVAILABLE
        );
        if unavailable && self.worker.is_some() && !self.worker_dead && self.active.is_none() {
            self.refresh_package();
        }
    }
    /// The worker's reading of the selected package, once it arrived.
    pub(super) fn package_details(&self) -> Option<&Result<PackageDetails, String>> {
        let row = self.firmware_selected.as_ref()?;
        self.firmware_package
            .as_ref()
            .filter(|(digest, _)| digest == &row.image.descriptor.digest)
            .and_then(|(_, details)| details.as_ref())
    }
    pub(super) fn firmware_list(
        &mut self,
        ui: &mut egui::Ui,
        selected_file: &mut Option<usize>,
    ) -> bool {
        let mut changed = false;
        let snapshot = self.snapshot.clone();
        self.rows_built = 0;
        egui::ScrollArea::horizontal()
            .id_salt("firmware-columns")
            .show(ui, |ui| {
                ui.set_min_width(WIDTHS.iter().sum::<f32>() + 64.0);
                changed |= super::sort_header(
                    ui,
                    &COLUMNS,
                    &WIDTHS,
                    &mut self.query.firmware_sort,
                    &mut self.query.firmware_descending,
                );
                egui::ScrollArea::vertical()
                    .id_salt("firmware-list")
                    .max_height((ui.available_height() - 26.0).max(80.0))
                    .show_rows(ui, 20.0, snapshot.firmware_rows.len(), |ui, range| {
                        for index in range {
                            self.rows_built += 1;
                            let row = &snapshot.firmware_rows[index];
                            let selected = self
                                .firmware_selected
                                .as_ref()
                                .is_some_and(|current| current.same(row));
                            // A double click only selects: packages have no launch action.
                            let (clicked, double) = super::draw_cells(
                                ui,
                                &cells(row),
                                &WIDTHS,
                                selected,
                                index % 2 == 0,
                                lead(row),
                            );
                            if (clicked || double) && !selected {
                                self.select_firmware(row, selected_file);
                            }
                        }
                    });
            });
        if snapshot.firmware_rows.is_empty() {
            ui.label(if snapshot.firmware_total == 0 {
                "No firmware packages yet. Import ROM folder… copies complete ROM sets into the store."
            } else {
                "No packages match this search."
            });
        }
        changed
    }
}

/// A ROM-folder scan as the review dialog shows it; source paths stay local.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScanReport {
    pub candidates: Vec<CandidateView>,
    /// Catalogue sets that cannot become packages, with the scanner's reason.
    pub refused: Vec<(String, String)>,
    /// Unreadable inputs and scan limits, by folder-relative name.
    pub issues: Vec<(String, String)>,
    pub unmatched: Option<String>,
    pub cancelled: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct CandidateView {
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
pub(crate) enum CandidateState {
    New,
    AlreadyImported,
    /// Another catalogue entry has the same ROM identity.
    Ambiguous,
    Refused(String),
}
impl CandidateView {
    fn result(&self) -> String {
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
    fn importable(&self) -> bool {
        matches!(self.state, CandidateState::New | CandidateState::Ambiguous)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
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

fn existing(
    store: &Store,
    candidate: &Candidate,
    catalogue: &[assembly::FirmwareEntry],
) -> pc_image::Result<bool> {
    let manifest = firmware_import::preview_manifest(candidate, &[])?;
    firmware_import::already_imported(store, &candidate.tag(), &manifest, catalogue)
}

/// Worker side of the scan: match the folder against the catalogue without
/// touching the store, then check each complete set against existing tags.
pub(crate) fn scan_folder(
    store: &Store,
    folder: &Path,
    index: Option<&Path>,
    control: &DiscoveryControl,
    catalogue: &[assembly::FirmwareEntry],
) -> super::Result<(ScanReport, Vec<Candidate>)> {
    let scan = assembly::firmware_discovery::with_discovery_session(index, false, control, || {
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
        let state = match (existing(store, candidate, catalogue), &catalogue_kind) {
            (Err(error), _) => CandidateState::Refused(error.to_string()),
            (_, Err(error)) => CandidateState::Refused(error.clone()),
            (Ok(true), _) => CandidateState::AlreadyImported,
            (Ok(false), _) if ambiguous => CandidateState::Ambiguous,
            (Ok(false), _) => CandidateState::New,
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
            tag: candidate.tag(),
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
pub(crate) fn preview_set(
    candidates: &[Candidate],
    id: &str,
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> super::Result<Verdict> {
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
pub(crate) fn import_selected(
    store: &Store,
    candidates: &[Candidate],
    selected: &[(String, Statement)],
    catalogue: &[assembly::FirmwareEntry],
) -> super::Result<Vec<Outcome>> {
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
            let tag = candidate.tag();
            match existing(store, candidate, catalogue) {
                Ok(true) => Outcome::AlreadyImported(tag),
                Ok(false) => {
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

/// Import ROM folder… review: scan progress, matched and refused sets with
/// each set's redistribution and details, unmatched files, then the import
/// outcomes. Cancel and Cancel scan stop a running scan; once Import selected
/// is sent, the import runs to its end.
pub(super) struct ImportDialog {
    pub folder: PathBuf,
    pub control: DiscoveryControl,
    pub report: Option<ScanReport>,
    pub picks: BTreeSet<String>,
    /// The statement of every set that can be imported, by catalogue id.
    pub drafts: BTreeMap<String, Draft>,
    /// The set whose details the editor shows.
    pub current: Option<String>,
    pub outcomes: Option<Vec<Outcome>>,
    pub importing: bool,
    pub error: Option<String>,
}
pub(super) enum DialogAction {
    None,
    Import,
    Close,
    /// Run the source archive picker for this set.
    PickSource(String),
}
impl ImportDialog {
    fn new(folder: PathBuf, control: DiscoveryControl) -> Self {
        Self {
            folder,
            control,
            report: None,
            picks: BTreeSet::new(),
            drafts: BTreeMap::new(),
            current: None,
            outcomes: None,
            importing: false,
            error: None,
        }
    }
    pub fn scanning(&self) -> bool {
        self.report.is_none() && self.error.is_none()
    }
    pub fn scanned(&mut self, report: ScanReport) {
        self.picks = report
            .candidates
            .iter()
            .filter(|c| c.state == CandidateState::New)
            .map(|c| c.catalogue_id.clone())
            .collect();
        self.drafts = report
            .candidates
            .iter()
            .filter(|c| c.importable())
            .map(|c| {
                (
                    c.catalogue_id.clone(),
                    Draft::new(c.facts.clone(), c.fields.clone()),
                )
            })
            .collect();
        self.current = self
            .picks
            .iter()
            .next()
            .or(self.drafts.keys().next())
            .cloned();
        self.report = Some(report);
    }
    pub fn finished(&mut self, outcomes: Vec<Outcome>) {
        self.importing = false;
        self.outcomes = Some(outcomes);
    }
    pub fn failed(&mut self, error: String) {
        self.importing = false;
        for draft in self.drafts.values_mut() {
            draft.stopped();
        }
        self.error = Some(error);
    }
    /// The sets can still be reviewed and changed.
    pub fn reviewing(&self) -> bool {
        self.report.as_ref().is_some_and(|r| !r.cancelled)
            && self.outcomes.is_none()
            && self.error.is_none()
            && !self.importing
    }
    /// Two picked sets with one ROM identity would publish the same bytes
    /// under two catalogue ids; the user must choose one, as `--for` does.
    pub fn conflict(&self) -> Option<String> {
        let report = self.report.as_ref()?;
        let mut seen = BTreeMap::new();
        for c in report
            .candidates
            .iter()
            .filter(|c| self.picks.contains(&c.catalogue_id))
        {
            if let Some(other) = seen.insert(&c.identity, &c.catalogue_id) {
                return Some(format!(
                    "{other} and {} are the same ROM; pick one catalogue identity",
                    c.catalogue_id
                ));
            }
        }
        None
    }
    /// Why Import selected is disabled: per picked set, the library's reason
    /// for refusing its statement, or that its check is still running.
    pub fn blockers(&self) -> Vec<String> {
        let mut blockers = Vec::new();
        if self.picks.is_empty() {
            blockers.push("no set is picked".into());
        }
        if let Some(conflict) = self.conflict() {
            blockers.push(conflict);
        }
        for id in &self.picks {
            let problem = match self.drafts.get(id) {
                Some(draft) => draft.problem(),
                None => Some("not importable".into()),
            };
            if let Some(problem) = problem {
                let title = self
                    .drafts
                    .get(id)
                    .map_or(id.as_str(), |d| d.fields.title.as_str());
                blockers.push(format!("{title}: {problem}"));
            }
        }
        blockers
    }
    pub fn can_import(&self) -> bool {
        self.reviewing() && self.blockers().is_empty()
    }
    /// Each picked set with the statement the library accepted for it.
    pub fn statements(&self) -> Vec<(String, Statement)> {
        self.picks
            .iter()
            .filter_map(|id| Some((id.clone(), self.drafts.get(id)?.statement())))
            .collect()
    }
    fn rights_cell(&mut self, ui: &mut egui::Ui, c: &CandidateView, editable: bool) {
        let picked = self.picks.contains(&c.catalogue_id);
        let Some(draft) = self.drafts.get_mut(&c.catalogue_id) else {
            ui.label(&c.redistribution);
            return;
        };
        let label = |rights: Rights| match (rights, &draft.facts.catalogue) {
            (Rights::UserSupplied, _) => "User-supplied".to_owned(),
            (Rights::Open, Catalogue::Open { license, .. }) => format!("Open · {license}"),
            (Rights::Open, _) => "Open — state a licence…".to_owned(),
        };
        let proprietary = matches!(draft.facts.catalogue, Catalogue::Proprietary { .. });
        let before = draft.fields.rights;
        let mut rights = before;
        ui.vertical(|ui| {
            ui.add_enabled_ui(editable && !proprietary, |ui| {
                egui::ComboBox::from_id_salt(("firmware-rights", &c.catalogue_id))
                    .selected_text(label(rights))
                    .show_ui(ui, |ui| {
                        for choice in [Rights::UserSupplied, Rights::Open] {
                            ui.selectable_value(&mut rights, choice, label(choice));
                        }
                    });
            });
            match &draft.facts.catalogue {
                Catalogue::Proprietary { id } => {
                    ui.small(format!("Catalogue: proprietary ({id})"));
                }
                Catalogue::Open { license, .. } if rights == Rights::Open => {
                    ui.small(format!("Licence from the catalogue: {license}"));
                }
                Catalogue::Open { license, .. } => {
                    ui.small(format!("Catalogue: open · {license}"));
                }
                Catalogue::Unlisted => {
                    if let Some(license) = draft.fields.license.stated() {
                        ui.small(format!("Stated by you: {license}"));
                    }
                }
            }
            if let (Rights::Open, Some(path)) = (rights, &draft.fields.source) {
                ui.small(format!(
                    "Source: {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
            }
            if let Some(problem) = draft.problem().filter(|_| picked) {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    super::firmware_metadata::sentence(&problem),
                );
            }
        });
        if rights != before {
            draft.fields.set_rights(rights);
            draft.revision += 1;
            self.current = Some(c.catalogue_id.clone());
        }
    }
    pub fn show(&mut self, ctx: &egui::Context, classic: bool) -> DialogAction {
        let mut action = DialogAction::None;
        egui::Modal::new(egui::Id::unique("import-rom-folder")).show(ctx, |ui| {
            ui.set_max_width(980.0);
            if classic {
                crate::shell::gradient_title(ui, "Import ROM folder…");
            } else {
                ui.heading("Import ROM folder…");
            }
            ui.label(format!("Folder: {}", self.folder.display()));
            ui.weak(
                "Match full SHA-256 and size against the catalogue, then assemble complete \
                 logical BIOS sets. Socket placement comes from the matching layout. \
                 Descriptions, fits and licences are prefilled from the catalogue where it has \
                 an entry.",
            );
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
            // Scrolled content takes the full height at once; offered the rest of
            // last frame's dialog, it would grow the modal frame by frame.
            let height = (ctx.content_rect().height() - 260.0).max(240.0);
            egui::ScrollArea::vertical()
                .id_salt("import-rom-folder-review")
                .max_height(height)
                .min_scrolled_height(height)
                .show(ui, |ui| self.review(ui, ctx, &mut action));
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "User-supplied by default. A set is open only when the catalogue lists it as \
                 open or you state its SPDX licence, and a copyleft set also needs its source \
                 archive. Only selected complete sets are copied. Source files remain unchanged.",
            );
            if self.importing {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Verifying and copying selected packages…");
                });
            }
            for outcome in self.outcomes.iter().flatten() {
                ui.label(outcome.to_string());
            }
            ui.horizontal_wrapped(|ui| {
                if self.outcomes.is_some() || self.error.is_some() {
                    if ui.button("Close").clicked() {
                        action = DialogAction::Close;
                    }
                    return;
                }
                if self.reviewing() {
                    let blockers = self.blockers();
                    ui.weak(if blockers.is_empty() {
                        let count = self.picks.len();
                        format!(
                            "{count} {} ready to import.",
                            if count == 1 { "set" } else { "sets" }
                        )
                    } else {
                        format!(
                            "Import disabled — {}",
                            super::firmware_metadata::stop(&blockers.join("; "))
                        )
                    });
                }
                if ui
                    .add_enabled(self.can_import(), egui::Button::new("Import selected"))
                    .clicked()
                {
                    action = DialogAction::Import;
                }
                if ui
                    .add_enabled(!self.importing, egui::Button::new("Cancel"))
                    .clicked()
                {
                    action = DialogAction::Close;
                }
            });
            if self.drafts.values().any(Draft::waiting) {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        });
        action
    }
    fn review(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, action: &mut DialogAction) {
        let editable = self.reviewing();
        // Taken out while drawing, so the rows can change the drafts beside it.
        let Some(report) = self.report.take() else {
            if self.scanning() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "Listing and hashing — {} files read",
                        self.control.snapshot().files_read
                    ));
                });
                if ui.button("Cancel scan").clicked() {
                    self.control.cancel();
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            return;
        };
        if !report.cancelled {
            self.review_report(ui, &report, editable, action);
        } else {
            ui.label("Scan cancelled; nothing was imported.");
        }
        self.report = Some(report);
    }
    fn review_report(
        &mut self,
        ui: &mut egui::Ui,
        report: &ScanReport,
        editable: bool,
        action: &mut DialogAction,
    ) {
        ui.label(format!(
            "Scan complete — {} matched, {} refused, {} other issues",
            report.candidates.len(),
            report.refused.len(),
            report.issues.len()
        ));
        ui.strong("Matched packages — assembled from parts");
        egui::Grid::new("firmware-candidate-grid")
            .num_columns(5)
            .striped(true)
            .show(ui, |ui| {
                for heading in [
                    "Pick",
                    "Package / parts",
                    "Result",
                    "Redistribution",
                    "Details",
                ] {
                    ui.strong(heading);
                }
                ui.end_row();
                for c in &report.candidates {
                    let mut picked = self.picks.contains(&c.catalogue_id);
                    if ui
                        .add_enabled(
                            editable && c.importable(),
                            egui::Checkbox::without_text(&mut picked),
                        )
                        .changed()
                    {
                        if picked {
                            self.picks.insert(c.catalogue_id.clone());
                        } else {
                            self.picks.remove(&c.catalogue_id);
                        }
                    }
                    ui.vertical(|ui| {
                        let title = self
                            .drafts
                            .get(&c.catalogue_id)
                            .map_or(c.name.as_str(), |d| d.fields.title.as_str());
                        ui.label(title);
                        for chip in &c.chips {
                            ui.small(chip);
                        }
                    });
                    ui.vertical(|ui| {
                        ui.label(c.result());
                        ui.small(format!(
                            "SHA-256 {} · {} bytes",
                            short(&c.identity.0),
                            c.identity.1
                        ));
                        ui.small(format!("Tag {}", c.tag));
                    });
                    self.rights_cell(ui, c, editable);
                    let current = self.current.as_deref() == Some(c.catalogue_id.as_str());
                    if ui
                        .add_enabled(
                            !matches!(c.state, CandidateState::Refused(_)),
                            egui::Button::new("Details").selected(current),
                        )
                        .clicked()
                    {
                        self.current = Some(c.catalogue_id.clone());
                    }
                    ui.end_row();
                }
                for (id, reason) in &report.refused {
                    ui.add_enabled(false, egui::Checkbox::without_text(&mut false));
                    ui.label(id);
                    ui.label(format!("{reason}. Import disabled."));
                    ui.label("—");
                    ui.label("");
                    ui.end_row();
                }
            });
        if let Some(current) = self.current.clone() {
            let candidate = report.candidates.iter().find(|c| c.catalogue_id == current);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                let title = self
                    .drafts
                    .get(&current)
                    .map(|d| d.fields.title.clone())
                    .or(candidate.map(|c| c.name.clone()))
                    .unwrap_or_default();
                ui.strong(format!("Set details — {title}"));
                match (self.drafts.get_mut(&current), candidate) {
                    (Some(draft), _) => {
                        ui.weak(format!(
                            "Prefilled from the catalogue entry {current} and the matched chip \
                             layout. Edits go into the package config and its README.md."
                        ));
                        ui.add_enabled_ui(editable, |ui| {
                            if draft.show(ui, &format!("firmware-set-{current}"))
                                == super::firmware_metadata::EditorAction::PickSource
                            {
                                *action = DialogAction::PickSource(current.clone());
                            }
                        });
                    }
                    (None, Some(c)) if c.state == CandidateState::AlreadyImported => {
                        ui.label(format!(
                            "Already in the store as {}. Nothing is imported again. Change its \
                             metadata in the library with Edit metadata…, which publishes a new \
                             revision.",
                            c.tag
                        ));
                    }
                    _ => (),
                }
            });
        }
        ui.strong("Unmatched files — excluded");
        if report.unmatched.is_none() && report.issues.is_empty() {
            ui.label("None");
        }
        if let Some(unmatched) = &report.unmatched {
            ui.label(unmatched);
        }
        for (source, reason) in &report.issues {
            ui.label(format!("{source}: {reason}"));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library_test_support::Fixture;
    use crate::library_view::{snapshot, LibraryView, Snapshot};
    use image_store::Digest;
    use std::{fs, sync::Arc, time::Duration};

    pub(crate) fn leaked(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }
    pub(crate) fn hash(bytes: &[u8]) -> &'static str {
        leaked(Digest::of(bytes).hex().into())
    }
    fn two_chip(id: &'static str, even: u8, odd: u8) -> assembly::FirmwareEntry {
        let part = |label, part_number, lane, byte: u8| assembly::KnownPart {
            label,
            part_number,
            lane,
            offset: 0,
            size: 16384,
            sha256: hash(&[byte; 16384]),
        };
        let layout: &'static [assembly::KnownPart] = Box::leak(Box::new([
            part("U27", "test-even", assembly::Lane::Even, even),
            part("U47", "test-odd", assembly::Lane::Odd, odd),
        ]));
        assembly::FirmwareEntry {
            id,
            sha256: hash(&[even, odd].repeat(16384)),
            size: 32768,
            parts: Box::leak(Box::new([layout])),
            distribution: assembly::FirmwareDistribution::UserSupplied,
            ..assembly::KISELEV_XI8088
        }
    }
    pub(crate) fn single(id: &'static str, byte: u8, open: bool) -> assembly::FirmwareEntry {
        assembly::FirmwareEntry {
            id,
            sha256: hash(&[byte; 32768]),
            size: 32768,
            distribution: if open {
                assembly::FirmwareDistribution::Open {
                    fetch: "synthetic/fetch.sh",
                    license: "MIT",
                    provenance: "synthetic build",
                    source_archive_sha256: None,
                }
            } else {
                assembly::FirmwareDistribution::UserSupplied
            },
            ..assembly::KISELEV_XI8088
        }
    }
    /// A POSIX tar archive (ustar magic at offset 257) of `size` bytes, `fill`
    /// after the header, as the synthetic copyleft BIOS's source.
    pub(crate) fn tar(size: usize, fill: u8) -> Vec<u8> {
        let mut bytes = vec![fill; size.max(512)];
        bytes[..257].fill(0);
        bytes[..10].copy_from_slice(b"src/a.asm\0");
        bytes[257..263].copy_from_slice(b"ustar\0");
        bytes
    }
    /// The archive the synthetic catalogue pins for its copyleft BIOS.
    pub(crate) fn pinned_tar() -> Vec<u8> {
        tar(6000, 0x5a)
    }
    /// The open image has the bytes of the synthetic machine BIOS, so that
    /// machine is a "Used by" entry of the open package.
    pub(crate) fn synthetic_catalogue() -> &'static [assembly::FirmwareEntry] {
        static CATALOGUE: std::sync::OnceLock<Vec<assembly::FirmwareEntry>> =
            std::sync::OnceLock::new();
        CATALOGUE.get_or_init(|| {
            vec![
                two_chip("synthetic-two-chip-bios", 0x31, 0x72),
                two_chip("synthetic-incomplete-bios", 0x41, 0x82),
                single("synthetic-open-bios", 0xf4, true),
                assembly::FirmwareEntry {
                    distribution: assembly::FirmwareDistribution::Open {
                        fetch: "synthetic/copyleft-fetch.sh",
                        license: "GPL-3.0-or-later",
                        provenance: "synthetic copyleft build",
                        source_archive_sha256: Some(hash(&pinned_tar())),
                    },
                    ..single("synthetic-copyleft-bios", 0xc5, true)
                },
            ]
        })
    }
    pub(crate) fn chip_folder(root: &Path) -> PathBuf {
        let folder = root.join("roms");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a"), [0x31; 16384]).unwrap();
        fs::write(folder.join("b"), [0x72; 16384]).unwrap();
        folder
    }
    pub(crate) fn warm(store: &Store) -> SummaryCache {
        let mut cache = SummaryCache::open(store).unwrap();
        while !cache.pending().is_empty() {
            cache.rebuild(store, 8, &[], &[]).unwrap();
        }
        cache
    }
    /// The open single-image package alone, for shell tests.
    pub(crate) fn publish_open_package(f: &Fixture) {
        let folder = f.temp.path().join("open-rom");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("open.bin"), [0xf4; 32768]).unwrap();
        let cat = synthetic_catalogue();
        let scan = firmware_import::scan(&[folder], cat).unwrap();
        firmware_import::publish(&f.store, &scan.candidates[0], &[], cat).unwrap();
    }
    impl LibraryView {
        /// Enter the Firmware collection and select its first package, as a click does.
        pub(crate) fn select_first_package(
            &mut self,
            cache: &SummaryCache,
            selected_file: &mut Option<usize>,
        ) {
            self.set_filter(Filter::Firmware, selected_file);
            self.snapshot = Arc::new(snapshot(cache, &self.query));
            let row = self.snapshot.firmware_rows[0].clone();
            self.select_firmware(&row, selected_file);
        }
    }
    /// A synthetic machine plus a described two-chip package with a document
    /// and an open single-image package holding the machine's BIOS bytes.
    pub(crate) fn library() -> (Fixture, SummaryCache) {
        let f = Fixture::new();
        let folder = chip_folder(f.temp.path());
        fs::write(folder.join("open.bin"), [0xf4; 32768]).unwrap();
        let cat = synthetic_catalogue();
        let mut scan = firmware_import::scan(&[folder], cat).unwrap();
        assert_eq!(scan.candidates.len(), 2, "{:?}", scan.issues);
        let manual = f.temp.path().join("manual.txt");
        fs::write(&manual, b"synthetic manual").unwrap();
        for candidate in &mut scan.candidates {
            let extras = if candidate.catalogue_id == "synthetic-two-chip-bios" {
                candidate.config.name = "Synthetic two-chip BIOS".into();
                candidate.config.vendor = Some("Example vendor".into());
                candidate.config.date = Some("1985-06-10".into());
                vec![firmware_import::Attachment {
                    path: manual.clone(),
                    dos_name: None,
                }]
            } else {
                Vec::new()
            };
            firmware_import::publish(&f.store, candidate, &extras, cat).unwrap();
        }
        let cache = warm(&f.store);
        (f, cache)
    }
    pub(in crate::library_view) fn query(filter: Filter, text: &str) -> Query {
        Query {
            text: text.into(),
            filter,
            ..Default::default()
        }
    }
    fn names(result: &Snapshot) -> Vec<String> {
        result
            .firmware_rows
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
    pub(crate) fn frame(
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

    #[test]
    fn firmware_rows_show_package_fields_facets_and_search_without_blob_reads() {
        let (f, cache) = library();
        // Rows, facets and Used by come from summaries alone: no blob, ROM or
        // manifest is readable any more.
        fs::rename(
            f.store.root().join("images/blobs"),
            f.store.root().join("hidden-blobs"),
        )
        .unwrap();
        let result = snapshot(&cache, &query(Filter::Firmware, ""));
        assert_eq!(result.firmware_total, 2);
        assert!(result.rows.is_empty());
        assert_eq!(result.matched, 2);
        let row = |id: &str| {
            result
                .firmware_rows
                .iter()
                .find(|r| r.image.reference == format!("firmware/{id}:1"))
                .unwrap()
        };
        let split = row("synthetic-two-chip-bios");
        let cells = cells(split);
        assert_eq!(
            cells[..8],
            [
                "Synthetic two-chip BIOS",
                "System BIOS",
                "2 chips",
                "Example vendor",
                "Version unknown / 1985-06-10",
                "ibm-5170 · AT",
                "synthetic-two-chip-bios",
                "User-supplied",
            ]
        );
        assert!(cells[8].ends_with("KiB"), "{}", cells[8]);
        assert!(split.used_by.is_empty());
        let open = row("synthetic-open-bios");
        let cells = super::cells(open);
        assert_eq!(cells[2], "single image");
        assert_eq!(cells[3], "Unknown");
        assert_eq!(cells[4], "Version unknown / date unknown");
        assert_eq!(cells[7], "Open · MIT");
        assert_eq!(open.used_by, ["test:source"]);
        assert_eq!(
            result.facets["Role"],
            BTreeSet::from(["system-bios".into()])
        );
        assert_eq!(result.facets["Board"], BTreeSet::from(["ibm-5170".into()]));
        assert_eq!(
            result.facets["Machine class"],
            BTreeSet::from(["at".into()])
        );
        assert_eq!(
            result.facets["Redistribution"],
            BTreeSet::from(["open".into(), "user-supplied".into()])
        );
        for (kind, value, expected) in [
            ("Redistribution", "open", vec!["synthetic-open-bios"]),
            (
                "Redistribution",
                "user-supplied",
                vec!["Synthetic two-chip BIOS"],
            ),
            (
                "Role",
                "system-bios",
                vec!["Synthetic two-chip BIOS", "synthetic-open-bios"],
            ),
            ("Role", "video-bios", vec![]),
            (
                "Board",
                "ibm-5170",
                vec!["Synthetic two-chip BIOS", "synthetic-open-bios"],
            ),
            ("Machine class", "xt", vec![]),
        ] {
            let filter = Filter::Facet(kind.into(), value.into());
            assert!(filter.is_firmware());
            assert_eq!(
                names(&snapshot(&cache, &query(filter, ""))),
                expected,
                "{kind}={value}"
            );
        }
        for (text, expected) in [
            ("u47", vec!["Synthetic two-chip BIOS"]),
            ("example vendor", vec!["Synthetic two-chip BIOS"]),
            ("synthetic-open", vec!["synthetic-open-bios"]),
            ("missing", vec![]),
        ] {
            assert_eq!(
                names(&snapshot(&cache, &query(Filter::Firmware, text))),
                expected,
                "{text}"
            );
        }
        let mut sorted = query(Filter::Firmware, "");
        sorted.firmware_sort = 7;
        assert_eq!(names(&snapshot(&cache, &sorted))[0], "synthetic-open-bios");
        sorted.firmware_descending = true;
        assert_eq!(
            names(&snapshot(&cache, &sorted))[0],
            "Synthetic two-chip BIOS"
        );
        // The firmware sort leaves the machine list's own sort alone.
        assert_eq!((sorted.sort, sorted.descending), (0, false));
        // The machine collection keeps its own rows and only counts packages.
        let machines = snapshot(&cache, &Query::default());
        assert!(machines.firmware_rows.is_empty());
        assert_eq!(machines.firmware_total, 2);
        assert_eq!(machines.rows.len(), 1);
        assert_eq!(machines.rows[0].image.reference, "test:source");
    }

    #[test]
    fn firmware_summaries_cached_before_chip_details_are_rebuilt() {
        let (f, _) = library();
        // Rewrite each firmware row as an older build wrote it: the fields from
        // "vendor" onward are serialized last, up to the row's closing brace.
        let path = f.store.root().join("cache/summaries/index.json");
        let mut text = fs::read_to_string(&path).unwrap();
        let mut stripped = 0;
        while let Some(start) = text.find(",\"vendor\":") {
            let (mut depth, mut quoted, mut escaped) = (0i32, false, false);
            let end = start
                + text[start..]
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
                        depth < 0
                    })
                    .unwrap()
                    .0;
            text.replace_range(start..end, "");
            stripped += 1;
        }
        assert_eq!(stripped, 2);
        fs::write(&path, text).unwrap();
        let mut cache = SummaryCache::open(&f.store).unwrap();
        assert_eq!(cache.pending().len(), 2);
        while !cache.pending().is_empty() {
            cache.rebuild(&f.store, 8, &[], &[]).unwrap();
        }
        let result = snapshot(&cache, &query(Filter::Firmware, "u27"));
        let firmware = result.firmware_rows[0].firmware().unwrap();
        assert_eq!(firmware.vendor.as_deref(), Some("Example vendor"));
        assert_eq!(firmware.roms[0].parts.len(), 2);
        assert_eq!(firmware.extras[0].filename, "manual.txt");
    }

    #[test]
    fn package_pane_lists_chips_documents_and_used_by_and_never_offers_start() {
        let (f, cache) = library();
        let result = Arc::new(snapshot(&cache, &query(Filter::Firmware, "")));
        let ctx = egui::Context::default();
        let size = egui::vec2(1400.0, 1000.0);
        let mut shown = Vec::new();
        for row in &result.firmware_rows {
            let output = frame(&ctx, size, 0.0, vec![], |ui| {
                egui::CentralPanel::default().show(ui, |ui| details(ui, row, None, false));
            });
            shown.extend(texts(&output).into_iter().map(|(t, _)| t));
        }
        let even = short(Digest::of(&[0x31; 16384]).hex());
        for expected in [
            "U27 · test-even",
            "U47 · test-odd",
            "System BIOS · even @ 0",
            "System BIOS · odd @ 0",
            "16384 bytes",
            even.as_str(),
            "Single image",
            "manual.txt",
            "Document · No licence recorded · user-supplied · 16 B",
            "MIT · open",
            "No licence recorded · user-supplied",
            "Checking…",
            "No documents attached.",
            "Board: ibm-5170",
            "Machine class: AT",
            "test:source",
            "No local machine image embeds these ROMs.",
            "Open · MIT",
            "Use in machine…",
        ] {
            assert!(
                shown.iter().any(|t| t == expected),
                "missing {expected:?} in {shown:?}"
            );
        }
        assert!(shown
            .iter()
            .any(|t| t.starts_with("One logical system BIOS · 32768 assembled bytes")));
        assert!(!shown.iter().any(|t| t == "Start"));

        // The whole library view: a machine selection is dropped on entering the
        // collection, and nothing it draws there is a Start action.
        let machines = snapshot(&cache, &Query::default());
        let mut view = LibraryView::new(None);
        view.selected = Some(machines.rows[0].image.clone());
        view.snapshot = Arc::new(machines);
        let mut file = Some(0);
        let draw = |view: &mut LibraryView, file: &mut Option<usize>| {
            let mut commands = Vec::new();
            let output = frame(&ctx, size, 0.0, vec![], |ui| {
                commands = view.show(ui, &[], file);
            });
            (texts(&output), commands)
        };
        let (machine_texts, _) = draw(&mut view, &mut None);
        assert!(
            machine_texts.iter().any(|(t, _)| t == "Start"),
            "the probe sees a machine Start"
        );
        view.set_filter(Filter::Firmware, &mut file);
        assert!(view.selected.is_none());
        assert!(file.is_none());
        view.snapshot = result.clone();
        view.firmware_selected = Some(result.firmware_rows[0].clone());
        let (firmware_texts, commands) = draw(&mut view, &mut file);
        assert!(firmware_texts
            .iter()
            .any(|(t, _)| t == "Import ROM folder…"));
        assert!(firmware_texts.iter().any(|(t, _)| t == "Firmware (2)"));
        assert!(!firmware_texts.iter().any(|(t, _)| t == "Start"));
        assert!(commands.is_empty());
        drop(f);
    }

    #[test]
    fn double_clicking_a_package_row_selects_it_and_issues_no_start() {
        let (_f, cache) = library();
        let ctx = egui::Context::default();
        let size = egui::vec2(1400.0, 800.0);
        let mut view = LibraryView::new(None);
        let machines = snapshot(&cache, &Query::default());
        view.selected = Some(machines.rows[0].image.clone());
        view.query.filter = Filter::Firmware;
        view.snapshot = Arc::new(snapshot(&cache, &query(Filter::Firmware, "")));
        let mut file = Some(0);
        let mut commands = Vec::new();
        let first = frame(&ctx, size, 0.0, vec![], |ui| {
            commands.extend(view.show(ui, &[], &mut file));
        });
        let target = view.snapshot.firmware_rows[0].clone();
        let (_, rect) = texts(&first)
            .into_iter()
            .find(|(t, _)| t == target.image.name())
            .expect("package row drawn");
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
                commands.extend(view.show(ui, &[], &mut file));
            });
        }
        assert!(view
            .firmware_selected
            .as_ref()
            .is_some_and(|r| r.same(&target)));
        assert!(view.selected.is_none());
        assert!(file.is_none());
        assert!(
            !commands.contains(&crate::shell_model::Command::Start),
            "{commands:?}"
        );
    }

    pub(crate) fn poll_until(view: &mut LibraryView, mut done: impl FnMut(&LibraryView) -> bool) {
        let ctx = egui::Context::default();
        for _ in crate::test_wait::rounds() {
            view.poll(&ctx);
            if done(view) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("worker did not finish; error {:?}", view.error);
    }

    #[test]
    fn rom_folder_import_runs_on_the_worker_reports_refusals_and_refreshes_rows() {
        let temp = assembly::test_support::TempDir::new("firmware-import-flow");
        let folder = chip_folder(temp.path());
        fs::write(folder.join("c"), [0x41; 16384]).unwrap();
        fs::write(folder.join("junk.bin"), [0x99; 16384]).unwrap();
        fs::write(folder.join("note.txt"), b"not a ROM").unwrap();
        let mut view = LibraryView::synthetic(temp.path().join("store"));
        poll_until(&mut view, |v| v.snapshot.storage.is_some());
        view.set_filter(Filter::Firmware, &mut None);
        view.send(super::super::Request::Query(view.query.clone()));
        assert!(view.can_import_firmware());
        view.begin_firmware_import(folder.clone(), None);
        assert!(view.busy && !view.can_import_firmware());
        poll_until(&mut view, |v| {
            v.firmware_import
                .as_ref()
                .is_some_and(|d| d.report.is_some())
        });
        assert!(!view.busy);
        let dialog = view.firmware_import.as_ref().unwrap();
        let report = dialog.report.as_ref().unwrap();
        assert_eq!(report.candidates.len(), 1);
        let candidate = &report.candidates[0];
        assert_eq!(candidate.state, CandidateState::New);
        assert_eq!(candidate.tag, "firmware/synthetic-two-chip-bios:1");
        assert_eq!(candidate.chips, ["a → U27 even @ 0", "b → U47 odd @ 0"]);
        assert_eq!(candidate.result(), "2 of 2 chips present");
        assert_eq!(
            report.refused,
            [(
                "synthetic-incomplete-bios".to_owned(),
                "1 of 2 chips present, missing U47".to_owned()
            )]
        );
        assert_eq!(
            report.unmatched.as_deref(),
            Some("unmatched: 2 files (1 size mismatches, 1 SHA-256 mismatches)")
        );
        assert_eq!(
            dialog.picks,
            BTreeSet::from(["synthetic-two-chip-bios".into()])
        );
        // Import selected waits for the library's verdict on the statement.
        assert!(!dialog.can_import());
        poll_until(&mut view, |v| {
            v.firmware_import.as_ref().is_some_and(|d| d.can_import())
        });
        let request = view.import_picks().expect("import allowed");
        view.send(request);
        poll_until(&mut view, |v| {
            v.firmware_import
                .as_ref()
                .is_some_and(|d| d.outcomes.is_some())
        });
        assert_eq!(
            view.firmware_import.as_ref().unwrap().outcomes,
            Some(vec![Outcome::Imported(
                "firmware/synthetic-two-chip-bios:1".into()
            )])
        );
        poll_until(&mut view, |v| {
            v.snapshot.firmware_rows.len() == 1
                && v.snapshot.firmware_rows[0].image.summary.is_some()
        });
        assert_eq!(cells(&view.snapshot.firmware_rows[0])[2], "2 chips");
        // Re-importing the same folder is idempotent.
        view.firmware_import = None;
        view.begin_firmware_import(folder, None);
        poll_until(&mut view, |v| {
            v.firmware_import
                .as_ref()
                .is_some_and(|d| d.report.is_some())
        });
        let dialog = view.firmware_import.as_ref().unwrap();
        let candidate = &dialog.report.as_ref().unwrap().candidates[0];
        assert_eq!(candidate.state, CandidateState::AlreadyImported);
        assert_eq!(candidate.result(), "Already imported");
        assert!(dialog.picks.is_empty() && !dialog.can_import());
        view.shutdown();
    }

    #[test]
    fn ambiguous_identities_changed_sources_and_cancelled_scans_are_refused() {
        let temp = assembly::test_support::TempDir::new("firmware-import-refusals");
        let store = Store::open(temp.path().join("store")).unwrap();
        let cat = [
            single("synthetic-first", 0x11, false),
            single("synthetic-alias", 0x11, false),
            two_chip("synthetic-two-chip-bios", 0x31, 0x72),
        ];
        let folder = chip_folder(temp.path());
        fs::write(folder.join("shared.bin"), [0x11; 32768]).unwrap();
        let control = DiscoveryControl::default();
        let (report, candidates) = scan_folder(&store, &folder, None, &control, &cat).unwrap();
        let state = |id: &str| {
            report
                .candidates
                .iter()
                .find(|c| c.catalogue_id == id)
                .unwrap()
                .state
                .clone()
        };
        assert_eq!(state("synthetic-first"), CandidateState::Ambiguous);
        assert_eq!(state("synthetic-alias"), CandidateState::Ambiguous);
        assert_eq!(state("synthetic-two-chip-bios"), CandidateState::New);
        let mut dialog = ImportDialog::new(folder.clone(), control);
        dialog.scanned(report);
        crate::library_test_support::settled_modal(
            egui::Id::unique("import-rom-folder"),
            egui::vec2(1040.0, 700.0),
            |ctx| {
                let _ = dialog.show(ctx, true);
            },
        );
        assert_eq!(
            dialog.picks,
            BTreeSet::from(["synthetic-two-chip-bios".into()])
        );
        dialog.picks.insert("synthetic-first".into());
        dialog.picks.insert("synthetic-alias".into());
        assert!(dialog
            .conflict()
            .unwrap()
            .contains("pick one catalogue identity"));
        assert!(!dialog.can_import());
        dialog.picks.remove("synthetic-alias");
        assert!(dialog.conflict().is_none());
        // Only the library's checks of the picked statements are still missing.
        let blockers = dialog.blockers();
        assert_eq!(blockers.len(), 2, "{blockers:?}");
        assert!(
            blockers.iter().all(|b| b.ends_with(": checking…")),
            "{blockers:?}"
        );

        // A chip that changes after the scan is refused, and no tag appears.
        // The import re-reads it to compute the package identity, so the
        // refusal names the scanned chip and the bytes it now holds.
        fs::write(folder.join("a"), [0x30; 16384]).unwrap();
        let outcomes = import_selected(
            &store,
            &candidates,
            &[
                ("synthetic-two-chip-bios".into(), Statement::default()),
                ("synthetic-first".into(), Statement::default()),
            ],
            &cat,
        )
        .unwrap();
        let Outcome::Failed(tag, reason) = &outcomes[0] else {
            panic!("{outcomes:?}")
        };
        assert_eq!(tag, "firmware/synthetic-two-chip-bios:1");
        assert!(
            reason.contains("changed since the scan")
                && reason.contains(&Digest::of(&[0x31; 16384]).to_string())
                && reason.contains(&Digest::of(&[0x30; 16384]).to_string()),
            "{reason}"
        );
        assert_eq!(
            outcomes[1],
            Outcome::Imported("firmware/synthetic-first:1".into())
        );
        assert!(store
            .tag_descriptor("firmware/synthetic-two-chip-bios:1")
            .is_err());
        assert_eq!(
            import_selected(
                &store,
                &candidates,
                &[("synthetic-first".into(), Statement::default())],
                &cat
            )
            .unwrap(),
            [Outcome::AlreadyImported(
                "firmware/synthetic-first:1".into()
            )]
        );

        let cancelled = DiscoveryControl::default();
        cancelled.cancel();
        let (report, candidates) = scan_folder(&store, &folder, None, &cancelled, &cat).unwrap();
        assert!(report.cancelled && report.candidates.is_empty() && candidates.is_empty());
    }
    /// One pointer click at `at`, spread over frames as a user's would be.
    pub(crate) fn click(ctx: &egui::Context, at: egui::Pos2, mut draw: impl FnMut(&mut egui::Ui)) {
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
            frame(ctx, egui::vec2(1200.0, 900.0), time, events, &mut draw);
        }
    }
    fn assert_failed_and_closable(view: &mut LibraryView) {
        let dialog = view.firmware_import.as_ref().expect("dialog stays open");
        assert!(dialog.error.is_some() && !dialog.scanning() && !dialog.importing);
        assert!(!view.busy && !view.can_import_firmware());
        let ctx = egui::Context::default();
        // A modal sizes itself over its first frames; wait until it stops moving.
        let mut drawn: Vec<(String, egui::Rect)> = Vec::new();
        for step in 0..20 {
            let previous = drawn.iter().find(|(t, _)| t == "Close").map(|(_, r)| *r);
            drawn = texts(&frame(
                &ctx,
                egui::vec2(1200.0, 900.0),
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
        assert!(!drawn.iter().any(|(t, _)| t == "Import selected"));
        let (_, close) = drawn
            .into_iter()
            .find(|(t, _)| t == "Close")
            .expect("Close offered");
        click(&ctx, close.center(), |ui| {
            view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
        });
        assert!(view.firmware_import.is_none());
    }

    #[test]
    fn import_dialog_fails_and_closes_when_the_worker_is_missing_or_dies() {
        let temp = assembly::test_support::TempDir::new("firmware-dead-worker");
        let folder = chip_folder(temp.path());
        assert!(!LibraryView::new(None).can_import_firmware());
        // The request cannot reach a worker whose thread has already ended.
        let mut view = LibraryView::synthetic(temp.path().join("stopped"));
        poll_until(&mut view, |v| v.snapshot.storage.is_some());
        let mut worker = view.worker.take().unwrap();
        worker.shutdown();
        view.worker = Some(worker);
        assert!(view.can_import_firmware());
        view.begin_firmware_import(folder.clone(), None);
        assert!(view.worker_dead);
        assert_failed_and_closable(&mut view);
        // The worker dies inside the scan, inside the preview of the set's
        // statement, then inside the import after an accepted preview.
        for survive in [0, 1, 2] {
            let root = temp.path().join(format!("crash-{survive}"));
            let mut view = LibraryView::new(Some(root.clone()));
            view.worker = Some(crate::library_worker::Worker::firmware_crash(root, survive));
            poll_until(&mut view, |v| v.snapshot.storage.is_some());
            view.begin_firmware_import(folder.clone(), None);
            if survive == 2 {
                poll_until(&mut view, |v| {
                    v.firmware_import.as_ref().is_some_and(|d| d.can_import())
                });
                let request = view.import_picks().expect("import allowed");
                view.send(request);
            }
            poll_until(&mut view, |v| v.worker_dead);
            assert_failed_and_closable(&mut view);
        }
    }

    #[test]
    fn unrelated_worker_errors_do_not_reach_the_import_dialog() {
        let temp = assembly::test_support::TempDir::new("firmware-unrelated-error");
        let folder = chip_folder(temp.path());
        let mut view = LibraryView::synthetic(temp.path().join("store"));
        poll_until(&mut view, |v| v.snapshot.storage.is_some());
        // Deleting a tag that does not exist fails while the scan is queued.
        view.send(super::super::Request::Delete(ImageRow {
            reference: "missing:tag".into(),
            aliases: Vec::new(),
            descriptor: Descriptor::new(media_type::MANIFEST, Digest::of(b"missing"), 7),
            summary: None,
            failure: None,
        }));
        view.begin_firmware_import(folder, None);
        poll_until(&mut view, |v| {
            v.firmware_import
                .as_ref()
                .is_some_and(|d| d.report.is_some() || d.error.is_some())
        });
        let dialog = view.firmware_import.as_ref().unwrap();
        assert!(dialog.error.is_none(), "{:?}", dialog.error);
        assert_eq!(dialog.report.as_ref().unwrap().candidates.len(), 1);
        assert!(
            view.error.is_some(),
            "the edit error is reported on its own"
        );
        view.shutdown();
    }

    #[test]
    fn scans_list_refused_sets_apart_from_unreadable_files() {
        let temp = assembly::test_support::TempDir::new("firmware-scan-classes");
        let store = Store::open(temp.path().join("store")).unwrap();
        let folder = chip_folder(temp.path());
        fs::write(folder.join("c"), [0x41; 16384]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(temp.path().join("gone"), folder.join("dangling")).unwrap();
        let control = DiscoveryControl::default();
        let (report, _) =
            scan_folder(&store, &folder, None, &control, synthetic_catalogue()).unwrap();
        assert_eq!(
            report.refused,
            [(
                "synthetic-incomplete-bios".to_owned(),
                "1 of 2 chips present, missing U47".to_owned()
            )]
        );
        #[cfg(unix)]
        {
            assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
            assert_eq!(report.issues[0].0, "dangling");
            assert!(report.issues[0].1.starts_with("unreadable input"));
        }
        assert_eq!(report.candidates.len(), 1);
    }

    // Persistence is disabled on Windows, which has no portable change time.
    #[cfg(not(windows))]
    #[test]
    fn a_second_scan_reads_no_rom_through_the_persistent_index() {
        let temp = assembly::test_support::TempDir::new("firmware-index-reuse");
        let store = Store::open(temp.path().join("store")).unwrap();
        let folder = chip_folder(temp.path());
        // A hash of a file changed within two seconds never enters the index.
        std::thread::sleep(Duration::from_millis(2200));
        let index = temp.path().join("firmware-index");
        let reads = || {
            let control = DiscoveryControl::default();
            let (report, _) = scan_folder(
                &store,
                &folder,
                Some(index.as_path()),
                &control,
                synthetic_catalogue(),
            )
            .unwrap();
            assert_eq!(report.candidates.len(), 1);
            control.snapshot().files_read
        };
        assert_eq!(reads(), 2);
        assert_eq!(reads(), 0);
    }

    #[test]
    fn thousands_of_packages_build_only_visible_rows() {
        let (_f, cache) = library();
        let one = snapshot(&cache, &query(Filter::Firmware, ""));
        let mut many = Snapshot::default();
        many.firmware_rows = (0..2000)
            .map(|i| {
                let mut row = one.firmware_rows[0].clone();
                row.image.reference = format!("firmware/synthetic-{i}:1");
                row
            })
            .collect();
        many.firmware_total = many.firmware_rows.len();
        let mut view = LibraryView::new(None);
        view.query.filter = Filter::Firmware;
        view.snapshot = Arc::new(many);
        let ctx = egui::Context::default();
        for (time, delta) in [(0.0, 0.0), (1.0, -30000.0), (2.0, 30000.0)] {
            let events = vec![
                egui::Event::PointerMoved(egui::pos2(600.0, 200.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    delta: egui::vec2(0.0, delta),
                    modifiers: egui::Modifiers::default(),
                },
            ];
            frame(&ctx, egui::vec2(1400.0, 700.0), time, events, |ui| {
                view.show(ui, &[], &mut None);
            });
            assert!(
                view.rows_built > 0 && view.rows_built < 40,
                "built {} rows",
                view.rows_built
            );
        }
    }

    #[test]
    fn open_roms_count_only_with_a_valid_licence() {
        let (_f, cache) = library();
        let tag = cache
            .tags()
            .iter()
            .find(|t| t.name == "firmware/synthetic-open-bios:1")
            .unwrap();
        let open = cache.summary(&tag.manifest.digest).unwrap().clone();
        assert_eq!(Redistribution::of(&open).label(), "Open · MIT");
        let with = |edit: &dyn Fn(&mut Descriptor)| {
            let mut summary = open.clone();
            summary
                .storage_blobs
                .iter_mut()
                .filter(|d| is_rom(d))
                .for_each(edit);
            Redistribution::of(&summary)
        };
        type Edit<'a> = &'a dyn Fn(&mut Descriptor);
        let cases: [(&str, Edit); 3] = [
            ("no licence", &|d| {
                d.annotations.remove("unemu.license");
            }),
            ("invalid licence", &|d| {
                d.annotations
                    .insert("unemu.license".into(), "not a licence".into());
            }),
            ("user-supplied mark", &|d| {
                d.annotations
                    .insert("unemu.redistribution".into(), "user-supplied".into());
            }),
        ];
        for (case, edit) in cases {
            let redistribution = with(edit);
            assert!(!redistribution.open, "{case}");
            assert_eq!(redistribution.label(), "User-supplied", "{case}");
            assert_eq!(redistribution.value(), USER_SUPPLIED, "{case}");
        }
        let two = with(&|d: &mut Descriptor| {
            d.annotations
                .insert("unemu.license".into(), "GPL-3.0-or-later".into());
        });
        assert_eq!(two.label(), "Open · GPL-3.0-or-later");
    }

    #[test]
    fn rows_without_a_summary_say_pending_or_unavailable() {
        let row = |failure: Option<&str>| FirmwareRow {
            image: ImageRow {
                reference: "firmware/synthetic:1".into(),
                aliases: Vec::new(),
                descriptor: Descriptor::new(media_type::MANIFEST, Digest::of(b"package"), 7),
                summary: None,
                failure: failure.map(Into::into),
            },
            redistribution: Redistribution::default(),
            used_by: Vec::new(),
            source_missing: false,
        };
        assert_eq!(cells(&row(None))[8], "Pending");
        assert!(!lead(&row(None)).warning);
        let failed = cells(&row(Some("missing blob")));
        assert_eq!(failed[8], "Unavailable");
        assert_eq!(failed[0], "firmware/synthetic:1");
        let failed = lead(&row(Some("missing blob")));
        assert!(failed.warning);
        assert_eq!(failed.icon, Some(crate::shell_icons::Icon::Firmware));
    }

    #[test]
    fn compressed_rom_layers_match_by_decoded_identity() {
        let (_f, cache) = library();
        let summary = |reference: &str| {
            let tag = cache.tags().iter().find(|t| t.name == reference).unwrap();
            cache.summary(&tag.manifest.digest).unwrap().clone()
        };
        // Stored bytes are compressed: the digest names them, the annotations the raw ROM.
        let compress = |mut summary: ImageSummary| {
            for blob in summary
                .storage_blobs
                .iter_mut()
                .filter(|d| d.media_type == media_type::FIRMWARE)
            {
                blob.annotations.insert(
                    image_store::codec::DECODED_DIGEST.into(),
                    blob.digest.to_string(),
                );
                blob.annotations.insert(
                    image_store::codec::DECODED_SIZE.into(),
                    blob.size.to_string(),
                );
                blob.digest = Digest::of(format!("compressed {}", blob.digest).as_bytes());
                blob.media_type.push_str("+zstd");
            }
            summary
        };
        let open = compress(summary("firmware/synthetic-open-bios:1"));
        let split = summary("firmware/synthetic-two-chip-bios:1");
        assert!(open
            .storage_blobs
            .iter()
            .any(|d| d.media_type.ends_with("+zstd")));
        assert_eq!(Redistribution::of(&open).label(), "Open · MIT");
        let machine = summary("test:source");
        for (raw_machine, packaged) in [(true, &open), (false, &open)] {
            let mut machines = MachineRoms::default();
            let held = if raw_machine {
                machine.clone()
            } else {
                compress(machine.clone())
            };
            machines.add("test:source", &held);
            assert_eq!(
                machines.users(packaged.firmware.as_ref().unwrap()),
                ["test:source"]
            );
        }
        // A machine holding both chips compressed uses the split package; one chip is not enough.
        let chips = |bytes: &[u8]| {
            let mut held = machine.clone();
            held.storage_blobs = bytes
                .iter()
                .map(|&b| Descriptor::new(media_type::FIRMWARE, Digest::of(&[b; 16384]), 16384))
                .collect();
            compress(held)
        };
        for (bytes, expected) in [
            (&[0x31, 0x72][..], vec!["chips:one"]),
            (&[0x31][..], vec![]),
        ] {
            let mut machines = MachineRoms::default();
            machines.add("chips:one", &chips(bytes));
            assert_eq!(machines.users(split.firmware.as_ref().unwrap()), expected);
        }
    }
}
