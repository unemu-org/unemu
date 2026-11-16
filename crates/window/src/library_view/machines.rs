// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Machines collection: named, local, mutable instances of images, as the
//! approved machines mock-up draws them; machine-images §3.2 and §7.
//!
//! The worker lists machines through `pc_image::machine` and runs every change
//! there; this module draws the list, the details pane and the dialogs, and
//! turns the user's choices into worker jobs. Names are checked here as the
//! store checks them (1–64 characters, unique ignoring case), so a dialog
//! says why before the store refuses.

use super::{
    hardware::{self, DriveFact, ImageFacts},
    human_size, pane, Filter, ImageRow, Lead, LibraryView,
};
use crate::{
    library_worker::{MachineJob, Recovery, Request},
    save_image::{self, SaveAsImage, TagLevel, Target},
    shell_icons::Icon,
    shell_model::Command,
};
use image_store::{machine::unique_among, summary::ImageSummary};
use std::collections::{BTreeMap, BTreeSet};

/// A machine as the library lists it.
#[derive(Clone, Debug)]
pub(crate) struct MachineRow {
    pub listing: pc_image::machine::Listing,
    /// The image's library summary: its catalogue titles and labels.
    pub summary: Option<ImageSummary>,
    /// The image's title, for search and the details pane.
    pub title: Option<String>,
    /// The floppy drives the machine runs with, by unit.
    pub drives: Vec<(u8, assembly::FloppyGeometry)>,
}

fn repository(reference: &str) -> &str {
    reference
        .split('@')
        .next()
        .unwrap_or_default()
        .rsplit_once(':')
        .map_or(reference, |(repo, _)| repo)
}

impl MachineRow {
    pub fn id(&self) -> &str {
        &self.listing.id
    }
    pub fn name(&self) -> &str {
        &self.listing.machine.name
    }
    /// The tag every image action of the machine uses: the tag it was made
    /// from while that still names its image, else another tag that does,
    /// else the repository at the digest.
    pub fn reference(&self) -> String {
        let source = &self.listing.machine.source;
        if self.listing.tags.iter().any(|tag| tag == source) {
            source.clone()
        } else if let Some(tag) = self.listing.tags.first() {
            tag.clone()
        } else {
            let repo = repository(source);
            format!(
                "{}@{}",
                if repo.is_empty() { "machine" } else { repo },
                self.listing.machine.image.digest
            )
        }
    }
    /// The source image as the Source image column names it.
    pub fn source_label(&self) -> String {
        if self.listing.detached() {
            let repo = repository(&self.listing.machine.source);
            format!("{} (no tag)", if repo.is_empty() { "image" } else { repo })
        } else {
            self.reference()
        }
    }
    /// The image's row, for readiness checks and the image actions.
    pub fn image_row(&self) -> ImageRow {
        ImageRow {
            reference: self.reference(),
            aliases: Vec::new(),
            descriptor: self.listing.machine.image.clone(),
            summary: self.summary.clone(),
            failure: None,
        }
    }
    /// The machines mock-up's badges after the source image: the tag the
    /// machine came from moved, or no tag names its image any more.
    pub fn source_badges(&self) -> Vec<String> {
        let mut badges = Vec::new();
        if self.listing.moved_to.is_some() {
            let tag = self
                .listing
                .machine
                .source
                .rsplit_once(':')
                .map_or("tag", |(_, tag)| tag);
            badges.push(format!("⚑ {tag} moved"));
        }
        if self.listing.detached() {
            badges.push("detached".into());
        }
        badges
    }
    pub fn state_text(&self) -> String {
        let mut text = self.listing.state.to_string();
        for insert in &self.listing.machine.pending {
            text.push_str(&format!(" · {} insert pending", insert.drive));
        }
        text
    }
    pub fn last_run(&self) -> &str {
        self.listing.machine.last_run.as_deref().unwrap_or("Never")
    }
    fn searched(&self, text: &str) -> bool {
        let text = text.trim().to_lowercase();
        text.is_empty()
            || format!(
                "{} {} {} {} {}",
                self.name(),
                self.id(),
                self.source_label(),
                self.title.as_deref().unwrap_or_default(),
                self.listing.machine.labels.join(" ")
            )
            .to_lowercase()
            .contains(&text)
    }
    /// The values a facet of the Machines collection takes for this machine:
    /// its own labels, else its image's catalogue metadata.
    fn facet_values(&self, kind: &str) -> Vec<String> {
        if kind == "Labels" {
            return self.listing.machine.labels.clone();
        }
        let titles = self.summary.iter().flat_map(|s| s.titles.iter());
        titles
            .flat_map(|t| match kind {
                "Genre" => t.genres.clone(),
                "Platform" => t.platform.iter().cloned().collect(),
                "Year" => t.year.map(|y| y.to_string()).into_iter().collect(),
                "Developer" => t.developers.clone(),
                "Publisher" => t.publishers.clone(),
                "Series" => t.series.iter().cloned().collect(),
                _ => Vec::new(),
            })
            .collect()
    }
    fn lead(&self) -> Lead {
        Lead {
            icon: Some(Icon::Machine),
            child: false,
            warning: self.listing.problem.is_some(),
            favourite: self.listing.machine.favorite,
        }
    }
    fn cells(&self) -> [String; 5] {
        let mut source = format!(
            "{} {}",
            self.source_label(),
            &self.listing.machine.image.digest.hex()[..8]
        );
        for badge in self.source_badges() {
            source.push_str(" · ");
            source.push_str(&badge);
        }
        [
            self.name().to_owned(),
            source,
            self.state_text(),
            self.last_run().to_owned(),
            human_size(self.listing.change_bytes),
        ]
    }
}

/// The facets of the Machines collection, in the tree's order.
pub(super) const FACETS: [&str; 7] = [
    "Labels",
    "Platform",
    "Genre",
    "Year",
    "Developer",
    "Publisher",
    "Series",
];

pub(super) fn facets(rows: &[MachineRow]) -> BTreeMap<String, BTreeSet<String>> {
    let mut facets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        for kind in FACETS {
            facets
                .entry(kind.into())
                .or_default()
                .extend(row.facet_values(kind));
        }
    }
    facets
}

/// The machines a filter and a search show, sorted by a column. Recently
/// played always lists the last run first.
pub(super) fn visible<'a>(
    rows: &'a [MachineRow],
    filter: &Filter,
    text: &str,
    sort: usize,
    descending: bool,
) -> Vec<&'a MachineRow> {
    let mut shown: Vec<&MachineRow> = rows
        .iter()
        .filter(|row| match filter {
            Filter::Recent => row.listing.machine.last_run.is_some(),
            Filter::Favorites => row.listing.machine.favorite,
            Filter::MachineFacet(kind, value) => row.facet_values(kind).contains(value),
            _ => true,
        })
        .filter(|row| row.searched(text))
        .collect();
    let (sort, descending) = if *filter == Filter::Recent {
        (3, true)
    } else {
        (sort, descending)
    };
    shown.sort_by_cached_key(|row| match sort {
        1 => row.source_label().to_lowercase(),
        2 => row.state_text().to_lowercase(),
        3 => row.listing.machine.last_run.clone().unwrap_or_default(),
        4 => format!("{:020}", row.listing.change_bytes),
        _ => row.name().to_lowercase(),
    });
    if descending {
        shown.reverse();
    }
    shown
}

const COLUMNS: super::Columns<5> = super::Columns {
    labels: ["Name", "Source image", "State", "Last run", "Changes"],
    widths: [220.0, 300.0, 240.0, 150.0, 90.0],
    cells: [
        super::Cell::Text,
        super::Cell::Mono,
        super::Cell::Text,
        super::Cell::Text,
        super::Cell::Number,
    ],
    samples: ["", "", "", "", "999.9 MiB"],
};

/// Why a machine name cannot be used: the store's rules, checked before it
/// refuses. `taken` lists the other machines' names.
pub(crate) fn name_problem<'a>(
    value: &str,
    taken: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let name = value.trim();
    if name.is_empty() {
        return Some("Type a name.".into());
    }
    let count = name.chars().count();
    if count > image_store::machine::MAX_NAME_CHARS {
        return Some(format!("At most 64 characters ({count} now)."));
    }
    if let Err(error) = image_store::machine::validate_name(name) {
        return Some(error.to_string());
    }
    let fold = |s: &str| s.to_uppercase().to_lowercase();
    taken
        .into_iter()
        .find(|other| fold(other) == fold(name))
        .map(|other| {
            format!(
                "A machine named \u{201c}{other}\u{201d} exists already. Names are unique, ignoring case."
            )
        })
}

/// The workspace id the store would give a new machine of this name.
pub(super) fn id_for<'a>(name: &str, ids: impl IntoIterator<Item = &'a str> + Clone) -> String {
    let slug = image_store::machine::slug(if name.trim().is_empty() {
        "machine"
    } else {
        name
    });
    let taken = |id: &str| ids.clone().into_iter().any(|other| other == id);
    if !taken(&slug) {
        return slug;
    }
    (2u32..)
        .map(|n| format!("{slug}-{n}"))
        .find(|id| !taken(id))
        .unwrap_or(slug)
}

/// What a name dialog makes or changes.
#[derive(Clone)]
pub(super) enum NamePurpose {
    /// New machine from image…, and New machine in the Start… chooser.
    New(Box<ImageRow>),
    Rename(String),
    Duplicate(String),
    /// Keep as new machine… of a Gaming-mode copy.
    Keep(Box<Recovery>),
}

/// The one name prompt of the machines mock-up.
pub(super) struct NameDialog {
    pub purpose: NamePurpose,
    pub title: String,
    pub intro: String,
    pub value: String,
    pub ok: &'static str,
    /// New machine from image…'s "Start it now".
    pub start: Option<bool>,
    pub foot: Option<String>,
}

/// One choice of the Start… chooser on an image with machines.
pub(super) struct StartChoice {
    pub id: String,
    pub name: String,
    pub hint: String,
}

pub(super) struct StartDialog {
    pub image: Box<ImageRow>,
    pub choices: Vec<StartChoice>,
    /// The chosen machine; `None` is New machine.
    pub pick: Option<String>,
    pub new_name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfirmKind {
    Reset,
    Delete,
}

pub(super) struct ConfirmDialog {
    pub kind: ConfirmKind,
    pub id: String,
    pub name: String,
    pub lines: Vec<String>,
    pub image: String,
    pub changes: u64,
    pub shared: bool,
}

/// Save as new image… of a machine.
pub(super) struct SaveDialog {
    pub id: String,
    pub name: String,
    pub form: SaveAsImage,
    /// Keep the machine on its old image with its changes.
    pub keep: bool,
    pub lines: Vec<String>,
    pub changes: u64,
    pub running: bool,
    pub error: Option<String>,
}

pub(super) enum Dialog {
    Name(NameDialog),
    Start(StartDialog),
    Confirm(ConfirmDialog),
    Save(Box<SaveDialog>),
    /// An interrupted Save as new image…, offered to be finished.
    Finish {
        id: String,
        name: String,
    },
}

/// The changes a reset or a delete removes, as the mock-up lists them.
fn change_lines(row: &MachineRow) -> Vec<String> {
    let mut lines: Vec<String> = row
        .listing
        .changes
        .iter()
        .map(|change| {
            let drive = if change.drive == pc_image::machine::INSERTED {
                "Inserted media".to_owned()
            } else {
                change.drive.clone()
            };
            match change.sectors {
                Some(n) => format!("{drive} — {n} sectors written"),
                None => format!("{drive} — changes of unknown size"),
            }
        })
        .collect();
    if let pc_image::machine::State::Changed {
        cmos: true,
        settings,
        ..
    } = &row.listing.state
    {
        lines.push("CMOS — changed bytes".into());
        if *settings {
            lines.push("Settings — back to the image's".into());
        }
    } else if let pc_image::machine::State::Changed { settings: true, .. } = &row.listing.state {
        lines.push("Settings — back to the image's".into());
    }
    for insert in &row.listing.machine.pending {
        lines.push(format!(
            "Pending insert — {} in {}",
            insert.medium, insert.drive
        ));
    }
    lines
}

/// What the details pane asks for.
enum Action {
    Command(Command),
    Favorite(bool),
    Labels,
    ShowImage,
}

/// A row of "Settings that differ from the image": the setting, this
/// machine's value and the image's.
pub(crate) type SettingDiff = (String, String, String);

/// The settings of a machine that differ from its image, by the property
/// sheet's field names.
pub(super) fn setting_diffs(
    settings: &pc_image::machine::Settings,
) -> Result<Vec<SettingDiff>, String> {
    use assembly::capabilities as caps;
    let image = crate::image_settings::describe(&settings.image)?;
    let machine = crate::image_settings::describe(&settings.machine)?;
    let before = caps::fields(&image);
    Ok(caps::fields(&machine)
        .into_iter()
        .filter_map(|field| {
            let old = before.iter().find(|old| old.path == field.path);
            if old.is_some_and(|old| old.value == field.value) {
                return None;
            }
            Some((
                field.label.clone(),
                caps::value_label(&machine, &field.path, &field.value),
                old.map_or_else(
                    || "—".to_owned(),
                    |old| caps::value_label(&image, &old.path, &old.value),
                ),
            ))
        })
        .collect())
}

impl LibraryView {
    pub(super) fn machine_row(&self, id: &str) -> Option<&MachineRow> {
        self.machines.iter().find(|row| row.id() == id)
    }
    /// The selected machine, once the worker has listed it.
    pub fn selected_machine_row(&self) -> Option<&MachineRow> {
        self.machine.as_deref().and_then(|id| self.machine_row(id))
    }
    /// The machines made from an image, the last run first.
    pub(super) fn machines_of(&self, digest: &image_store::Digest) -> Vec<&MachineRow> {
        let mut rows: Vec<&MachineRow> = self
            .machines
            .iter()
            .filter(|row| row.listing.machine.image.digest == *digest)
            .collect();
        rows.sort_by(|a, b| {
            b.listing
                .machine
                .last_run
                .cmp(&a.listing.machine.last_run)
                .then_with(|| a.name().cmp(b.name()))
        });
        rows
    }
    fn names(&self) -> impl Iterator<Item = &str> + Clone {
        self.machines.iter().map(MachineRow::name)
    }
    /// Whether the machine runs now.
    pub(super) fn machine_running(&self, id: &str) -> bool {
        self.active_machine
            .as_ref()
            .is_some_and(|(active, _)| active == id)
            || self
                .machine_row(id)
                .is_some_and(|row| row.listing.state == pc_image::machine::State::Running)
    }

    /// Select a machine: its image's row becomes `selected`, so readiness,
    /// floppies and the Image menu follow its source image.
    pub(super) fn select_machine(&mut self, id: String, row: ImageRow) {
        self.pending_launch = None;
        self.started_from_image = None;
        self.machine_labels = self
            .machine_row(&id)
            .map(|row| row.listing.machine.labels.join(", "))
            .unwrap_or_default();
        self.machine_settings = Some((id.clone(), None));
        self.send(Request::Machine(MachineJob::ReadSettings(id.clone())));
        self.machine = Some(id);
        self.selected = Some(row);
        self.readiness = None;
        self.recheck();
    }

    /// Show the Machines collection with `id` selected and visible.
    pub(crate) fn show_machine(&mut self, id: String, row: ImageRow) {
        if !self.query.filter.is_machines() {
            self.query.filter = Filter::Machines;
        }
        self.query.text.clear();
        self.firmware_selected = None;
        self.media_selected = None;
        self.clear_file_selection = true;
        self.select_machine(id, row);
        self.send(Request::Query(self.query.clone()));
    }

    /// Tests: the machines as a listing would give them.
    #[cfg(test)]
    pub(crate) fn set_machines(&mut self, rows: Vec<MachineRow>) {
        self.machines = std::sync::Arc::new(rows);
        self.machines_revision += 1;
    }

    /// Tests: a snapshot of `cache`, as the worker sends one.
    #[cfg(test)]
    pub(crate) fn use_snapshot(&mut self, cache: &image_store::summary::SummaryCache) {
        self.snapshot = std::sync::Arc::new(super::snapshot(cache, &self.query));
    }

    /// The table of the Machines collection with its sort header.
    pub(super) fn machine_list(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) -> bool {
        let mut changed = false;
        let machines = self.machines.clone();
        // With no image at all, the library's empty state says where the
        // first one comes from.
        if machines.is_empty() && self.snapshot.read && self.snapshot.total == 0 {
            super::empty_library(ui, commands);
            return false;
        }
        let shown = visible(
            &machines,
            &self.query.filter,
            &self.query.text,
            self.query.machine_sort,
            self.query.machine_descending,
        );
        let widths = super::fit_columns(ui, &COLUMNS);
        egui::ScrollArea::horizontal()
            .id_salt("machine-columns")
            .show(ui, |ui| {
                ui.set_min_width(super::table_width(ui, &widths));
                if super::sort_header(
                    ui,
                    &COLUMNS.labels,
                    &widths,
                    &[],
                    &mut self.query.machine_sort,
                    &mut self.query.machine_descending,
                ) {
                    changed = true;
                }
                if shown.is_empty() {
                    ui.label(if machines.is_empty() {
                        "No machines yet. Select an image and choose New machine from image… or Start, or get one with File › Get templates…."
                    } else {
                        "No machines match. Clear the search or pick another collection."
                    });
                }
                egui::ScrollArea::vertical()
                    .id_salt("machine-list")
                    .max_height((ui.available_height() - 26.0).max(80.0))
                    .show_rows(ui, 20.0, shown.len(), |ui, range| {
                        for index in range {
                            self.rows_built += 1;
                            let row = shown[index];
                            let selected = self.machine.as_deref() == Some(row.id());
                            let (clicked, double) = super::draw_cells(
                                ui,
                                &row.cells(),
                                &widths,
                                &COLUMNS.cells,
                                selected,
                                index % 2 == 0,
                                row.lead(),
                            );
                            if (clicked || double) && !selected {
                                self.select_machine(row.id().to_owned(), row.image_row());
                            }
                            if double {
                                commands.push(Command::Start);
                            }
                        }
                    });
            });
        let running = machines
            .iter()
            .filter(|row| row.listing.state == pc_image::machine::State::Running)
            .count();
        ui.label(format!(
            "{} of {} machines{} · changes {}",
            shown.len(),
            machines.len(),
            if running > 0 {
                format!(" · {running} running")
            } else {
                String::new()
            },
            human_size(machines.iter().map(|row| row.listing.change_bytes).sum())
        ));
        changed
    }

    /// The migration notice and the unfinished gaming sessions above the list.
    pub(super) fn machine_banners(&mut self, ui: &mut egui::Ui) {
        if let Some(migration) = &self.migration_notice {
            let names: Vec<String> = migration
                .adopted
                .iter()
                .map(|id| {
                    self.machine_row(id)
                        .map_or_else(|| id.clone(), |row| row.name().to_owned())
                })
                .collect();
            let mut dismiss = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    crate::shell_icons::show(ui, Icon::Info);
                    ui.vertical(|ui| {
                        if !names.is_empty() {
                            ui.strong("Your desktop sessions are machines now.");
                            ui.add(
                                egui::Label::new(format!(
                                    "Each image you had started got a machine named after the image, with its changes kept: {}. Rename… them any time.",
                                    names.join(", ")
                                ))
                                .wrap(),
                            );
                        }
                        for (name, reason) in &migration.skipped {
                            ui.add(
                                egui::Label::new(format!(
                                    "{name} stays as it is for now: {reason}"
                                ))
                                .wrap(),
                            );
                        }
                    });
                    dismiss = ui.button("OK").clicked();
                });
            });
            if dismiss {
                self.migration_notice = None;
            }
        }
    }

    /// The details pane of the selected machine, as the machines mock-up
    /// draws it: the kind tile beside the name, the facts and the labels,
    /// media per drive beside the settings, firmware and hardware, then the
    /// actions in one row and the note under them.
    pub(super) fn machine_details(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let Some(row) = self.selected_machine_row().cloned() else {
            ui.label(if self.machine.is_some() {
                "Reading the machine…"
            } else {
                "Select a machine or an image for its details."
            });
            return;
        };
        let mut actions = Vec::new();
        let facts = self.image_facts().cloned();
        let readiness = self.readiness.clone();
        let running = self.machine_running(row.id());
        let machine = &row.listing.machine;
        let user_supplied = row
            .summary
            .as_ref()
            .is_some_and(|s| !super::image_rights(s).open);
        let tile = pane::Tile {
            title: row.title.as_deref().unwrap_or(row.name()),
            kind: "MACHINE",
            user_supplied,
        };
        pane::head(ui, &tile, |ui| {
            pane::title(ui, Icon::Machine, row.name(), machine.favorite);
            let created = match (&machine.created, machine.origin) {
                (Some(at), image_store::machine::Origin::Duplicate) => {
                    format!(" · created {at} as a duplicate")
                }
                (Some(at), _) => format!(" · created {at}"),
                (None, image_store::machine::Origin::Migration) => " · created at migration".into(),
                (None, _) => String::new(),
            };
            pane::meta(
                ui,
                format!(
                    "Machine · workspace id {} (command line: image machine run {}){created}",
                    row.id(),
                    row.id()
                ),
            );
            if machine.origin == image_store::machine::Origin::Migration {
                pane::meta(
                    ui,
                    "Made from this image's earlier desktop session when machines arrived; its changes were kept.",
                );
            }
            if let Some(problem) = &row.listing.problem {
                pane::text(
                    ui,
                    egui::RichText::new(format!("⚠ {problem}")).color(ui.visuals().warn_fg_color),
                );
            }
            pane::facts(ui, "machine-facts", |ui| {
                pane::fact(ui, "Source image", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.link(row.source_label()).clicked() {
                            actions.push(Action::ShowImage);
                        }
                        ui.label(format!("— {}", row.title.as_deref().unwrap_or("untitled")));
                        ui.label(pane::mono(format!(
                            "sha256:{}",
                            pane::short_digest(machine.image.digest.hex())
                        )));
                        for badge in row.source_badges() {
                            ui.colored_label(ui.visuals().warn_fg_color, badge);
                        }
                    });
                });
                pane::fact_text(ui, "State", row.state_text());
                pane::fact_text(ui, "Last run", row.last_run());
                pane::fact_text(
                    ui,
                    "Changes",
                    format!(
                        "{} in journals beside the image",
                        human_size(row.listing.change_bytes)
                    ),
                );
                pane::fact(ui, "Labels", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let mut favourite = machine.favorite;
                        if ui.checkbox(&mut favourite, "Favourite").changed() {
                            actions.push(Action::Favorite(favourite));
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.machine_labels)
                                .hint_text("labels, comma separated")
                                .desired_width(ui.available_width().clamp(80.0, 180.0)),
                        );
                        if ui.button("Apply labels").clicked() {
                            actions.push(Action::Labels);
                        }
                    });
                });
            });
        });
        if row.listing.moved_to.is_some() {
            pane::text(
                ui,
                egui::RichText::new(format!(
                    "Created from {}, which now names another image. This machine stays on sha256:{}: a machine never follows a tag. For the newer image, start a new machine from it.",
                    machine.source,
                    pane::short_digest(machine.image.digest.hex())
                ))
                .color(ui.visuals().warn_fg_color),
            );
        }
        if row.listing.detached() {
            pane::text(
                ui,
                egui::RichText::new(
                    "No tag names this machine's image any more (detached). The image stays in the store for this machine; Save as new image… gives its state a name again.",
                )
                .color(ui.visuals().warn_fg_color),
            );
        }
        pane::split(
            ui,
            0.55,
            |ui| {
                pane::section(ui, "Media per drive");
                self.media_per_drive(ui, &row, facts.as_ref(), &readiness);
            },
            |ui| {
                pane::section(ui, "Settings that differ from the image");
                match self
                    .machine_settings
                    .as_ref()
                    .filter(|(id, _)| id == row.id())
                    .and_then(|(_, diffs)| diffs.as_ref())
                {
                    None => {
                        pane::meta(ui, "Reading settings…");
                    }
                    Some(Err(error)) => {
                        pane::text(ui, error.as_str());
                    }
                    Some(Ok(diffs)) if diffs.is_empty() => {
                        pane::meta(ui, "None — this machine uses the image's settings.");
                    }
                    Some(Ok(diffs)) => {
                        let rows: Vec<pane::Row> = diffs
                            .iter()
                            .map(|(setting, mine, image)| pane::Row {
                                head: setting.clone(),
                                cells: vec![
                                    vec![(mine.clone(), pane::Tone::Body)],
                                    vec![(image.clone(), pane::Tone::Body)],
                                ],
                                changed: true,
                            })
                            .collect();
                        pane::table(
                            ui,
                            "machine-settings-diff",
                            &["Setting", "This machine", "Image"],
                            &rows,
                        );
                        pane::meta(ui, "Applied at every start of this machine. Save as new image… writes them into the new image's configuration.");
                    }
                }
                pane::firmware_section(ui, facts.as_ref(), true);
                pane::section(ui, "Hardware, from the image");
                match (&readiness, &facts) {
                    (Some(Ok(_)), Some(facts)) => {
                        pane::text(ui, facts.hardware.as_str());
                    }
                    (Some(Err(reason)), _) => {
                        pane::text(ui, reason.as_str());
                    }
                    _ => {
                        pane::meta(ui, "Checking image…");
                    }
                }
            },
        );
        let ready = self.ready();
        let stopped = !running;
        pane::actions(ui, |ui| {
            for (label, enabled, command, reason) in [
                ("Start", stopped && ready, Command::Start, ""),
                ("Gaming mode", stopped && ready, Command::GamingMode, ""),
                ("Settings…", true, Command::Settings, ""),
                (
                    "Insert media…",
                    stopped && !row.drives.is_empty(),
                    Command::Insert(0),
                    "",
                ),
                ("Save as new image…", true, Command::SaveImage, ""),
                ("Disk layers…", true, Command::Layers, ""),
                (
                    "Duplicate…",
                    stopped,
                    Command::Duplicate,
                    "Stop the machine first.",
                ),
                ("Rename…", true, Command::Rename, ""),
                (
                    "Reset to image…",
                    stopped,
                    Command::ResetMachine,
                    "Stop the machine first.",
                ),
                (
                    "Delete machine…",
                    stopped,
                    Command::DeleteMachine,
                    "Stop the machine first.",
                ),
            ] {
                let button = if command == Command::Start {
                    pane::primary(ui, label)
                } else {
                    egui::Button::new(label)
                };
                let response = ui.add_enabled(enabled, button);
                if !reason.is_empty() {
                    response.clone().on_disabled_hover_text(reason);
                }
                if response.clicked() {
                    actions.push(Action::Command(command));
                }
            }
        });
        pane::meta(
            ui,
            "Gaming mode plays a disposable copy of this machine as it is now; this machine does not change. Reset to image… throws its changes away and keeps the machine; Delete machine… removes both, never the image.",
        );
        for action in actions {
            match action {
                Action::Command(command) => commands.push(command),
                Action::Favorite(favourite) => self.send(Request::Machine(MachineJob::Favorite(
                    row.id().to_owned(),
                    favourite,
                ))),
                Action::Labels => {
                    let labels = self
                        .machine_labels
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    self.send(Request::Machine(MachineJob::Labels(
                        row.id().to_owned(),
                        labels,
                    )));
                }
                Action::ShowImage => commands.push(Command::ShowSource),
            }
        }
    }

    /// The image's drives by letter beside what this machine changed in
    /// them: the mock-up's Media per drive table, CMOS last.
    fn media_per_drive(
        &self,
        ui: &mut egui::Ui,
        row: &MachineRow,
        facts: Option<&ImageFacts>,
        readiness: &Option<Result<pc_image::inspection::ImageDetails, String>>,
    ) {
        let Some(facts) = facts else {
            match readiness {
                Some(Err(reason)) => pane::text(ui, reason.as_str()),
                _ => pane::meta(ui, "Checking image…"),
            };
            return;
        };
        let rows = drive_rows(&facts.drives, row, |insert| self.pending_label(insert));
        pane::table(
            ui,
            "machine-media",
            &["Drive", "In the image", "In this machine"],
            &rows,
        );
    }

    // ---- actions -----------------------------------------------------------

    /// Start on an image: a machine named after it, started at once, when it
    /// has none; otherwise the Start… chooser, the last run preselected.
    pub(super) fn start_image(&mut self, row: ImageRow) {
        let machines: Vec<StartChoice> = self
            .machines_of(&row.descriptor.digest)
            .into_iter()
            .map(|m| StartChoice {
                id: m.id().to_owned(),
                name: m.name().to_owned(),
                hint: format!(
                    "{} · last run {} · {} of changes",
                    m.state_text(),
                    m.last_run(),
                    human_size(m.listing.change_bytes)
                ),
            })
            .collect();
        if machines.is_empty() {
            self.busy = true;
            self.status = Some("Creating a machine from the image…".into());
            self.send(Request::Machine(MachineJob::Create {
                image: Box::new(row),
                name: None,
                start: true,
            }));
            return;
        }
        let new_name = unique_among(row.name(), self.names());
        self.machine_dialog = Some(Dialog::Start(StartDialog {
            pick: Some(machines[0].id.clone()),
            choices: machines,
            new_name,
            image: Box::new(row),
        }));
    }

    /// The toolbar's Start: Start… on an image with machines, which asks.
    pub fn start_label(&self) -> &'static str {
        match (&self.machine, &self.selected) {
            (None, Some(row)) if !self.machines_of(&row.descriptor.digest).is_empty() => "Start…",
            _ => "Start",
        }
    }

    /// Image › Start…: the selected image, or the selected machine's source.
    pub fn start_selected_image(&mut self) {
        if self.busy {
            return;
        }
        if let Some(row) = self.selected.clone() {
            self.start_image(row);
        }
    }

    /// Image › Gaming mode: a disposable copy of the image as stored, also
    /// while one of its machines is selected.
    pub fn image_gaming(&mut self) {
        if self.busy {
            return;
        }
        let Some(row) = self.selected.clone() else {
            return;
        };
        // The image's own copy goes straight to the worker, which verifies
        // the image as it prepares it: a readiness check of the selected
        // machine neither delays it nor turns it into a copy of the machine.
        self.pending_launch = None;
        self.busy = true;
        self.status = Some("Verifying image and opening workspace…".into());
        self.send(Request::Launch(
            row,
            crate::library_worker::WorkspaceKind::Disposable,
            None,
        ));
    }

    /// Set or clear the disk drive `drive` of a stopped machine gets at its
    /// next Start. The row shows it at once; the store changes that drive
    /// only, on its record as it is then.
    pub fn set_pending(
        &mut self,
        id: &str,
        drive: &str,
        insert: Option<image_store::machine::PendingInsert>,
    ) {
        let mut rows = (*self.machines).clone();
        if let Some(row) = rows.iter_mut().find(|row| row.id() == id) {
            let pending = &mut row.listing.machine.pending;
            pending.retain(|p| p.drive != drive);
            pending.extend(insert.clone());
            self.machines = std::sync::Arc::new(rows);
            self.machines_revision += 1;
        }
        self.send(Request::Machine(MachineJob::SetPending {
            id: id.to_owned(),
            drive: drive.to_owned(),
            insert: insert.map(Box::new),
        }));
    }

    /// A pending disk as the details pane, the chips and the picker name it:
    /// the set's title and the disk's label, as the library lists the set.
    pub fn pending_label(&self, insert: &image_store::machine::PendingInsert) -> String {
        let Some(set) = self
            .picker_sets()
            .into_iter()
            .find(|set| set.image.digest == insert.set.digest)
        else {
            return pending_name(insert);
        };
        let disk = set
            .disks
            .iter()
            .find(|disk| disk.id == insert.medium)
            .map_or(insert.medium.as_str(), |disk| disk.label.as_str())
            .to_owned();
        if insert.all && set.disks.len() > 1 {
            format!(
                "{} — {disk} (all {} disks join the catalogue)",
                set.title,
                set.disks.len()
            )
        } else {
            format!("{} — {disk}", set.title)
        }
    }

    /// Image › Layers…: the image's own chains, without a machine's changes.
    pub fn open_image_layers(&mut self) {
        let machine = self.machine.take();
        let active = self.active.take();
        self.open_layers();
        self.active = active;
        self.machine = machine;
    }

    pub fn begin_new_from_image(&mut self) {
        let Some(row) = self.selected.clone() else {
            return;
        };
        let value = unique_among(row.name(), self.names());
        self.machine_dialog = Some(Dialog::Name(NameDialog {
            title: "New machine from image".into(),
            intro: format!(
                "From {} sha256:{} — {}",
                row.reference,
                &row.descriptor.digest.hex()[..12],
                row.name()
            ),
            value,
            ok: "Create",
            start: Some(true),
            foot: Some("A new machine costs nothing until it writes: only the sectors, floppies, CMOS bytes and settings it changes are stored, in journals beside the image. The image itself never changes.".into()),
            purpose: NamePurpose::New(Box::new(row)),
        }));
    }

    pub fn begin_rename(&mut self) {
        let Some(row) = self.selected_machine_row() else {
            return;
        };
        self.machine_dialog = Some(Dialog::Name(NameDialog {
            title: format!("Rename — {}", row.name()),
            intro: format!("Machine from {}.", row.reference()),
            value: row.name().to_owned(),
            ok: "Rename",
            start: None,
            foot: None,
            purpose: NamePurpose::Rename(row.id().to_owned()),
        }));
    }

    pub fn begin_duplicate(&mut self) {
        let Some(row) = self.selected_machine_row() else {
            return;
        };
        let value = unique_among(&format!("{} (copy)", row.name()), self.names());
        let row = row.clone();
        self.machine_dialog = Some(Dialog::Name(NameDialog {
            title: format!("Duplicate — {}", row.name()),
            intro: format!(
                "The copy starts with this machine's changes ({} copied), settings, pending insert and labels, on the same image {}. Afterwards the two change independently.",
                human_size(row.listing.change_bytes),
                row.reference()
            ),
            value,
            ok: "Duplicate",
            start: None,
            foot: None,
            purpose: NamePurpose::Duplicate(row.id().to_owned()),
        }));
    }

    pub fn begin_keep_copy(&mut self, record: Recovery) {
        let value = unique_among(&record.title, self.names());
        self.machine_dialog = Some(Dialog::Name(NameDialog {
            title: "Keep as new machine".into(),
            intro: format!(
                "The gaming copy becomes a machine of its own, on {}, with its changes. Nothing is copied: the disposable workspace is renamed.",
                record.reference
            ),
            value,
            ok: "Keep",
            start: None,
            foot: record
                .machine
                .as_deref()
                .and_then(|id| self.machine_row(id))
                .map(|m| format!("{} stays as it was before Gaming mode.", m.name())),
            purpose: NamePurpose::Keep(Box::new(record)),
        }));
    }

    pub fn begin_reset(&mut self) {
        self.begin_confirm(ConfirmKind::Reset);
    }

    pub fn begin_delete_machine(&mut self) {
        self.begin_confirm(ConfirmKind::Delete);
    }

    fn begin_confirm(&mut self, kind: ConfirmKind) {
        let Some(row) = self.selected_machine_row() else {
            return;
        };
        let shared = self.machines_of(&row.listing.machine.image.digest).len() > 1;
        self.machine_dialog = Some(Dialog::Confirm(ConfirmDialog {
            kind,
            id: row.id().to_owned(),
            name: row.name().to_owned(),
            lines: change_lines(row),
            image: row.reference(),
            changes: row.listing.change_bytes,
            shared,
        }));
    }

    /// Save as new image… of the selected (or running) machine.
    pub(super) fn begin_save_machine(&mut self, id: &str) {
        let Some(row) = self.machine_row(id).cloned() else {
            return;
        };
        // An unfinished save is finished as it was chosen (moving onto the
        // new image, or keeping the changes on the old one), never redone.
        if row.listing.publication_pending {
            self.machine_dialog = Some(Dialog::Finish {
                id: row.id().to_owned(),
                name: row.name().to_owned(),
            });
            return;
        }
        let tags = self.snapshot.tags.clone();
        let mut form = SaveAsImage::new(
            &row.reference(),
            row.listing.machine.image.digest.clone(),
            tags,
        )
        .with_title(Some(row.name()));
        let repo = image_store::machine::slug(row.name());
        form.set_repo(repo);
        let mut lines = change_lines(&row);
        if lines.is_empty() {
            lines.push("No changes — the new tag names the source manifest itself".into());
        }
        self.machine_dialog = Some(Dialog::Save(Box::new(SaveDialog {
            id: row.id().to_owned(),
            name: row.name().to_owned(),
            form,
            keep: false,
            lines,
            changes: row.listing.change_bytes,
            running: self.machine_running(row.id()),
            error: None,
        })));
    }

    // ---- dialogs -----------------------------------------------------------

    /// Draws the open Machines dialog; requests go to the worker.
    pub(super) fn machine_modals(
        &mut self,
        ctx: &egui::Context,
        classic: bool,
        requests: &mut Vec<Request>,
    ) {
        let Some(dialog) = self.machine_dialog.take() else {
            return;
        };
        let title = |ui: &mut egui::Ui, text: &str| {
            if classic {
                crate::shell::gradient_title(ui, text);
            } else {
                ui.heading(text);
            }
        };
        let names: Vec<String> = self.names().map(str::to_owned).collect();
        let ids: Vec<String> = self.machines.iter().map(|m| m.id().to_owned()).collect();
        let kept = match dialog {
            Dialog::Name(mut dialog) => {
                let exclude = match &dialog.purpose {
                    NamePurpose::Rename(id) => Some(id.clone()),
                    _ => None,
                };
                let taken: Vec<&str> = self
                    .machines
                    .iter()
                    .filter(|m| Some(m.id()) != exclude.as_deref())
                    .map(MachineRow::name)
                    .collect();
                let mut done = None;
                let mut close = false;
                egui::Modal::new(egui::Id::unique("machine-name")).show(ctx, |ui| {
                    ui.set_width(460.0);
                    title(ui, &dialog.title);
                    ui.add(egui::Label::new(&dialog.intro).wrap());
                    ui.label("Machine name");
                    let response = ui.text_edit_singleline(&mut dialog.value);
                    let problem = name_problem(&dialog.value, taken.iter().copied());
                    match &exclude {
                        Some(id) => ui.label(
                            egui::RichText::new(format!(
                                "Only the name changes. The workspace id stays {id} for the command line."
                            ))
                            .small(),
                        ),
                        None => ui.label(
                            egui::RichText::new(format!(
                                "1–64 characters, unique among your machines. Workspace id for the command line: {}.",
                                id_for(&dialog.value, ids.iter().map(String::as_str))
                            ))
                            .small(),
                        ),
                    };
                    if let Some(problem) = &problem {
                        ui.colored_label(ui.visuals().warn_fg_color, problem);
                    }
                    if let Some(start) = &mut dialog.start {
                        ui.checkbox(start, "Start it now");
                    }
                    if let Some(foot) = &dialog.foot {
                        ui.add(egui::Label::new(egui::RichText::new(foot).small()).wrap());
                    }
                    ui.separator();
                    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        let ok = if dialog.start == Some(true) {
                            format!("{} and start", dialog.ok)
                        } else {
                            dialog.ok.to_owned()
                        };
                        if (ui.add_enabled(problem.is_none(), egui::Button::new(ok)).clicked()
                            || enter)
                            && problem.is_none()
                        {
                            done = Some(dialog.value.trim().to_owned());
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                    close |= ui.input_mut(crate::shell::escape_pressed);
                });
                match done {
                    Some(name) => {
                        let start = dialog.start == Some(true);
                        requests.push(Request::Machine(match dialog.purpose {
                            NamePurpose::New(image) => MachineJob::Create {
                                image,
                                name: Some(name),
                                start,
                            },
                            NamePurpose::Rename(id) => MachineJob::Rename(id, name),
                            NamePurpose::Duplicate(id) => MachineJob::Duplicate(id, name),
                            NamePurpose::Keep(record) => {
                                self.decision = None;
                                MachineJob::KeepCopy {
                                    workspace: record.name.clone(),
                                    name,
                                }
                            }
                        }));
                        self.busy = true;
                        None
                    }
                    None if close => None,
                    None => Some(Dialog::Name(dialog)),
                }
            }
            Dialog::Start(mut dialog) => {
                let mut go = false;
                let mut close = false;
                let problem = dialog
                    .pick
                    .is_none()
                    .then(|| name_problem(&dialog.new_name, names.iter().map(String::as_str)))
                    .flatten();
                egui::Modal::new(egui::Id::unique("start-image")).show(ctx, |ui| {
                    ui.set_width(480.0);
                    title(ui, &format!("Start — {}", dialog.image.reference));
                    ui.label(format!(
                        "This image has {} machine{}. Start one of them, or a new machine from the image.",
                        dialog.choices.len(),
                        if dialog.choices.len() == 1 { "" } else { "s" }
                    ));
                    for choice in &dialog.choices {
                        ui.radio_value(&mut dialog.pick, Some(choice.id.clone()), egui::RichText::new(&choice.name).strong());
                        ui.label(egui::RichText::new(&choice.hint).small());
                    }
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut dialog.pick, None, egui::RichText::new("New machine").strong());
                        let response = ui.text_edit_singleline(&mut dialog.new_name);
                        if response.gained_focus() {
                            dialog.pick = None;
                        }
                    });
                    ui.label(egui::RichText::new("New machine starts from the image as stored.").small());
                    if let Some(problem) = &problem {
                        ui.colored_label(ui.visuals().warn_fg_color, problem);
                    }
                    ui.label(egui::RichText::new("Gaming mode (Image menu) plays a disposable copy of the image instead.").small());
                    ui.separator();
                    ui.horizontal(|ui| {
                        go = ui.add_enabled(problem.is_none(), egui::Button::new("Start")).clicked();
                        close = ui.button("Cancel").clicked();
                    });
                    close |= ui.input_mut(crate::shell::escape_pressed);
                });
                if go {
                    match dialog.pick {
                        Some(id) => {
                            let row = self
                                .machine_row(&id)
                                .map_or_else(|| (*dialog.image).clone(), MachineRow::image_row);
                            self.show_machine(id, row);
                            self.pending_launch = self.selected.as_ref().map(|row| {
                                (
                                    row.descriptor.digest.clone(),
                                    crate::library_worker::WorkspaceKind::Machine,
                                )
                            });
                        }
                        None => {
                            self.busy = true;
                            requests.push(Request::Machine(MachineJob::Create {
                                image: dialog.image,
                                name: Some(dialog.new_name.trim().to_owned()),
                                start: true,
                            }));
                        }
                    }
                    None
                } else if close {
                    None
                } else {
                    Some(Dialog::Start(dialog))
                }
            }
            Dialog::Confirm(dialog) => {
                let mut ok = false;
                let mut close = false;
                let (heading, button) = match dialog.kind {
                    ConfirmKind::Reset => (format!("Reset to image — {}", dialog.name), "Reset"),
                    ConfirmKind::Delete => (
                        format!("Delete machine — {}", dialog.name),
                        "Delete machine",
                    ),
                };
                egui::Modal::new(egui::Id::unique("machine-confirm")).show(ctx, |ui| {
                    ui.set_width(460.0);
                    title(ui, &heading);
                    if dialog.kind == ConfirmKind::Reset && dialog.lines.is_empty() {
                        ui.label(format!("Nothing to reset: {} is the same as its image.", dialog.name));
                    } else {
                        ui.label(match dialog.kind {
                            ConfirmKind::Reset => format!(
                                "Return {} to {} exactly as stored. These changes are removed:",
                                dialog.name, dialog.image
                            ),
                            ConfirmKind::Delete => format!(
                                "Delete {} and its changes ({})?",
                                dialog.name,
                                human_size(dialog.changes)
                            ),
                        });
                        for line in &dialog.lines {
                            ui.label(format!("• {line}"));
                        }
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(match dialog.kind {
                                ConfirmKind::Reset => "The machine keeps its name, favourite, labels and last run. To keep the changes, choose Save as new image… first. Disk layers › Discard changes opens this same question.".to_owned(),
                                ConfirmKind::Delete => format!(
                                    "The image {} stays in the library{}. To keep the changes, choose Save as new image… first.",
                                    dialog.image,
                                    if dialog.shared { ", with its other machines" } else { "" }
                                ),
                            })
                            .small(),
                        )
                        .wrap(),
                    );
                    ui.separator();
                    ui.horizontal(|ui| {
                        let nothing = dialog.kind == ConfirmKind::Reset && dialog.lines.is_empty();
                        if nothing {
                            close = ui.button("OK").clicked();
                        } else {
                            ok = ui.button(button).clicked();
                            close = ui.button("Cancel").clicked();
                        }
                    });
                    close |= ui.input_mut(crate::shell::escape_pressed);
                });
                if ok {
                    self.busy = true;
                    requests.push(Request::Machine(match dialog.kind {
                        ConfirmKind::Reset => MachineJob::Reset(dialog.id.clone()),
                        ConfirmKind::Delete => {
                            self.machine = None;
                            self.selected = None;
                            self.readiness = None;
                            MachineJob::Delete(dialog.id.clone())
                        }
                    }));
                    None
                } else if close {
                    None
                } else {
                    Some(Dialog::Confirm(dialog))
                }
            }
            Dialog::Finish { id, name } => {
                let mut finish = false;
                let mut close = false;
                egui::Modal::new(egui::Id::unique("finish-save")).show(ctx, |ui| {
                    ui.set_width(460.0);
                    title(ui, &format!("Save as new image — {name}"));
                    ui.add(
                        egui::Label::new(format!(
                            "An earlier Save as new image… of {name} did not finish. The machine cannot start until it does. Finishing keeps every change it was saving and does what that save chose: move onto the new image, or keep the changes here."
                        ))
                        .wrap(),
                    );
                    ui.separator();
                    ui.horizontal(|ui| {
                        finish = ui
                            .add_enabled(!self.busy, egui::Button::new("Finish saving"))
                            .clicked();
                        close = ui.button("Cancel").clicked();
                    });
                    close |= ui.input_mut(crate::shell::escape_pressed);
                });
                if finish {
                    self.busy = true;
                    requests.push(Request::Machine(MachineJob::FinishCommit(id)));
                    None
                } else if close {
                    None
                } else {
                    Some(Dialog::Finish { id, name })
                }
            }
            Dialog::Save(mut dialog) => {
                let mut save = false;
                let mut close = false;
                egui::Modal::new(egui::Id::unique("save-machine")).show(ctx, |ui| {
                    ui.set_width(640.0);
                    title(ui, &format!("Save as new image — {}", dialog.name));
                    ui.label(format!(
                        "From the machine {} · its image {} sha256:{}",
                        dialog.name,
                        dialog.form.source,
                        save_image::short(&dialog.form.source_digest)
                    ));
                    if dialog.running {
                        ui.label("Saving stops the machine first: every drive finishes its writes, then the changes are frozen into the new image.");
                    }
                    ui.separator();
                    ui.strong("Name");
                    save_image::form(ui, &mut dialog.form);
                    ui.label(
                        egui::RichText::new(format!(
                            "Defaults to the machine name. A new name starts at 1.0.0; use {} for a new version of the source image instead.",
                            dialog.form.source.rsplit_once(':').map_or(dialog.form.source.as_str(), |(r, _)| r)
                        ))
                        .small(),
                    );
                    ui.separator();
                    ui.strong("What is saved");
                    for line in &dialog.lines {
                        ui.label(format!("• {line}"));
                    }
                    ui.label(egui::RichText::new("Everything else (firmware, unchanged disks, media blobs) is shared with the source image by digest.").small());
                    ui.separator();
                    ui.strong("After saving, this machine…");
                    ui.radio_value(
                        &mut dialog.keep,
                        false,
                        "moves onto the new image and starts again with fresh, empty journals. Its state becomes Same as image; the changes are stored once, in the image. (Default.)",
                    );
                    ui.radio_value(
                        &mut dialog.keep,
                        true,
                        format!(
                            "stays as it is on {} with its changes. The new image stands alone; start it with New machine from image….{}",
                            dialog.form.source,
                            if dialog.changes > 0 {
                                format!(" (Its changes are then stored twice: {} more.)", human_size(dialog.changes))
                            } else {
                                String::new()
                            }
                        ),
                    );
                    if dialog.changes > 0 {
                        ui.add(egui::Label::new(egui::RichText::new("New disk layers count as user-supplied until you mark them open, so this image can be pushed only to a private registry.").small()).wrap());
                    }
                    if let Some(error) = &dialog.error {
                        ui.add(egui::Label::new(error).wrap());
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        let check = dialog.form.check();
                        let ready = !self.busy && (check.level != TagLevel::Bad || check.taken.is_some());
                        save = ui.add_enabled(ready, egui::Button::new("Save")).clicked();
                        close = ui.button("Cancel").clicked();
                    });
                });
                let mut target: Option<Target> =
                    save_image::prompts(ctx, &mut dialog.form, classic);
                if target.is_none()
                    && dialog.form.prompt.is_none()
                    && ctx.input_mut(crate::shell::escape_pressed)
                {
                    close = true;
                }
                if save {
                    target = dialog.form.submit();
                }
                if let Some(target) = target {
                    self.busy = true;
                    self.status = Some("Saving the machine as a new image…".into());
                    requests.push(Request::Machine(MachineJob::Commit {
                        id: dialog.id.clone(),
                        target,
                        keep: dialog.keep,
                    }));
                    // The dialog stays until the store answers, so a refusal
                    // asks again with the current tags.
                    Some(Dialog::Save(dialog))
                } else if close {
                    None
                } else {
                    Some(Dialog::Save(dialog))
                }
            }
        };
        if self.machine_dialog.is_none() {
            self.machine_dialog = kept;
        }
    }

    /// A refused Save as new image… asks again with the store's tags.
    pub(super) fn commit_refused(
        &mut self,
        error: String,
        tags: Vec<(String, image_store::Digest)>,
    ) {
        match &mut self.machine_dialog {
            Some(Dialog::Save(dialog)) => {
                dialog.error = Some(error);
                dialog.form.refresh(tags);
            }
            _ => self.error = Some(error),
        }
    }
}

/// Bytes in a journal sector: a 512-byte disk sector.
const SECTOR_BYTES: u64 = 512;

/// A count with its thousands grouped by a no-break space, as the mock-ups
/// write sector counts: "61 030".
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(digit);
    }
    out
}

/// The Media per drive rows of a machine: each drive of its image by
/// letter, what this machine wrote there and the disk waiting for its next
/// Start; media inserted from installation sets follow the image's drives.
fn drive_rows(
    drives: &[DriveFact],
    row: &MachineRow,
    pending: impl Fn(&image_store::machine::PendingInsert) -> String,
) -> Vec<pane::Row> {
    let written = |drive: &str| -> Vec<String> {
        row.listing
            .changes
            .iter()
            .filter(|change| change.drive == drive)
            .map(|change| {
                change.sectors.map_or_else(
                    || "changes of unknown size".into(),
                    |n| {
                        format!(
                            "{} sectors written · {}",
                            grouped(n),
                            human_size(n as u64 * SECTOR_BYTES)
                        )
                    },
                )
            })
            .collect()
    };
    let pending_lines = |drive: &str| -> Vec<(String, pane::Tone)> {
        row.listing
            .machine
            .pending
            .iter()
            .filter(|insert| insert.drive == drive)
            .map(|insert| {
                (
                    format!(
                        "Pending: {} — in the drive at the next Start",
                        pending(insert)
                    ),
                    pane::Tone::Pending,
                )
            })
            .collect()
    };
    let cmos_changed = matches!(
        row.listing.state,
        pc_image::machine::State::Changed { cmos: true, .. }
    );
    let mut rows: Vec<pane::Row> = drives
        .iter()
        .map(|drive| {
            let lines = written(&drive.drive);
            let cmos = drive.drive == hardware::CMOS && cmos_changed;
            let mut cell = if !lines.is_empty() {
                vec![
                    ("Written by this machine".to_owned(), pane::Tone::Body),
                    (lines.join(", "), pane::Tone::Change),
                ]
            } else if cmos {
                vec![
                    ("Changed by this machine".to_owned(), pane::Tone::Body),
                    (
                        "CMOS bytes differ from the image".to_owned(),
                        pane::Tone::Change,
                    ),
                ]
            } else {
                vec![("As in the image".to_owned(), pane::Tone::Muted)]
            };
            cell.extend(pending_lines(&drive.drive));
            pane::Row {
                head: drive.drive.clone(),
                cells: vec![vec![(drive.described.clone(), pane::Tone::Body)], cell],
                changed: !lines.is_empty() || cmos,
            }
        })
        .collect();
    let inserted = written(pc_image::machine::INSERTED);
    if !inserted.is_empty() {
        rows.push(pane::Row {
            head: "Inserted".into(),
            cells: vec![
                vec![("Media from installation sets".to_owned(), pane::Tone::Body)],
                vec![
                    ("Written by this machine".to_owned(), pane::Tone::Body),
                    (inserted.join(", "), pane::Tone::Change),
                ],
            ],
            changed: true,
        });
    }
    // A pending disk for a drive the image does not list still shows.
    for insert in &row.listing.machine.pending {
        if !drives.iter().any(|drive| drive.drive == insert.drive) {
            rows.push(pane::Row {
                head: insert.drive.clone(),
                cells: vec![
                    vec![("Not in the image".to_owned(), pane::Tone::Muted)],
                    pending_lines(&insert.drive),
                ],
                changed: false,
            });
        }
    }
    rows
}

/// A pending insert as the details pane and the drive chips name it.
pub(crate) fn pending_name(insert: &image_store::machine::PendingInsert) -> String {
    let set = if insert.reference.is_empty() {
        "a set".to_owned()
    } else {
        insert.reference.clone()
    };
    if insert.all {
        format!("{set} {} (all disks join the catalogue)", insert.medium)
    } else {
        format!("{set} {}", insert.medium)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image_store::{media_type as mt, Descriptor, Digest};
    use pc_image::machine::{Listing, State};
    use std::sync::Arc;

    pub(crate) fn image(name: &str) -> Descriptor {
        Descriptor::new(mt::MANIFEST, Digest::of(name.as_bytes()), 1)
    }

    pub(crate) fn row(id: &str, name: &str, image_name: &str, tags: &[&str]) -> MachineRow {
        let source = tags.first().map_or("games:1.0.0", |t| t).to_owned();
        let machine = image_store::machine::Machine::new(
            name,
            image(image_name),
            &source,
            image_store::machine::Origin::New,
        );
        MachineRow {
            listing: Listing {
                id: id.into(),
                machine,
                state: State::NeverStarted,
                changes: Vec::new(),
                change_bytes: 0,
                tags: tags.iter().map(|t| (*t).to_owned()).collect(),
                moved_to: None,
                publication_pending: false,
                problem: None,
            },
            summary: None,
            title: Some("Games".into()),
            drives: vec![(0, assembly::descriptors::FLOPPY_TYPES[0].geometry)],
        }
    }

    /// Names follow the store's rules: 1–64 characters, unique ignoring
    /// case; the default adds " (2)", and the id hint is a free slug.
    #[test]
    fn names_are_checked_as_the_store_checks_them() {
        let taken = ["DOS games", "Win 3.1 test"];
        assert_eq!(name_problem("  ", taken).as_deref(), Some("Type a name."));
        assert!(name_problem("dos GAMES", taken)
            .unwrap()
            .contains("\u{201c}DOS games\u{201d} exists already"));
        assert!(name_problem(&"x".repeat(65), taken)
            .unwrap()
            .starts_with("At most 64 characters (65 now)"));
        assert_eq!(name_problem(" Keen ", taken), None);
        assert_eq!(unique_among("DOS games", taken), "DOS games (2)");
        assert_eq!(unique_among("Keen", taken), "Keen");
        assert_eq!(id_for("DOS games", ["dos-games"]), "dos-games-2");
        assert_eq!(id_for("", ["dos-games"]), "machine");
    }

    /// The State column, the insert-pending badge and the source badges.
    #[test]
    fn rows_show_state_pending_inserts_and_tag_badges() {
        let mut machine = row("dos", "DOS", "a", &["games:1.0.0", "games:latest"]);
        machine.listing.state = State::Changed {
            drives: vec!["A:".into(), "C:".into()],
            inserted: false,
            cmos: true,
            settings: true,
        };
        machine
            .listing
            .machine
            .pending
            .push(image_store::machine::PendingInsert {
                drive: "A:".into(),
                set: image("set"),
                reference: "keen4:1.4".into(),
                medium: "disk-1".into(),
                all: false,
                journal: true,
            });
        machine.listing.change_bytes = 2048;
        let cells = machine.cells();
        assert_eq!(cells[0], "DOS");
        assert!(cells[1].starts_with("games:1.0.0 "), "{}", cells[1]);
        assert_eq!(
            cells[2],
            "Changed: A:, C:, CMOS, settings · A: insert pending"
        );
        assert_eq!(cells[3], "Never");
        assert_eq!(cells[4], "2.0 KiB");
        assert_eq!(machine.lead().icon, Some(Icon::Machine));
        // The tag it came from moved; then no tag names its image.
        machine.listing.moved_to = Some(image("b"));
        assert_eq!(machine.source_badges(), ["⚑ 1.0.0 moved"]);
        machine.listing.tags.clear();
        assert_eq!(machine.source_label(), "games (no tag)");
        assert_eq!(machine.source_badges(), ["⚑ 1.0.0 moved", "detached"]);
        assert!(machine.reference().starts_with("games@sha256:"));
    }

    /// Recently played lists machines run before, the last run first;
    /// Favourites and the label facet filter; search finds the image title.
    #[test]
    fn collections_filter_and_sort_machines() {
        let mut a = row("a", "Alpha", "a", &["games:1"]);
        let mut b = row("b", "Beta", "a", &["games:1"]);
        let c = row("c", "Gamma", "a", &["games:1"]);
        a.listing.machine.last_run = Some("2026-11-14 10:00 UTC".into());
        b.listing.machine.last_run = Some("2026-11-15 09:00 UTC".into());
        b.listing.machine.favorite = true;
        a.listing.machine.labels = vec!["testing".into()];
        let rows = vec![a, b, c];
        let names = |filter: Filter, text: &str| -> Vec<String> {
            visible(&rows, &filter, text, 0, false)
                .into_iter()
                .map(|r| r.name().to_owned())
                .collect()
        };
        assert_eq!(names(Filter::Machines, ""), ["Alpha", "Beta", "Gamma"]);
        assert_eq!(names(Filter::Recent, ""), ["Beta", "Alpha"]);
        assert_eq!(names(Filter::Favorites, ""), ["Beta"]);
        assert_eq!(
            names(Filter::MachineFacet("Labels".into(), "testing".into()), ""),
            ["Alpha"]
        );
        assert_eq!(names(Filter::Machines, "gamm"), ["Gamma"]);
        assert_eq!(names(Filter::Machines, "games"), ["Alpha", "Beta", "Gamma"]);
        assert_eq!(
            visible(&rows, &Filter::Machines, "", 0, true)[0].name(),
            "Gamma"
        );
        assert_eq!(
            facets(&rows)["Labels"].iter().collect::<Vec<_>>(),
            ["testing"]
        );
    }

    fn drawn(view: &mut LibraryView) -> (Vec<String>, Vec<Command>) {
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
        let mut commands = Vec::new();
        let mut selected = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| commands = view.show(ui, &[], &mut selected),
        );
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        (texts, commands)
    }

    /// The library opens on Machines: the Library group in the mock-up's
    /// order, the machine's row and its details pane with its actions.
    #[test]
    fn the_library_opens_on_machines_with_the_mockup_tree_and_details() {
        let mut view = LibraryView::new(None);
        let mut machine = row("dos-games", "DOS games", "a", &["games:1.0.0"]);
        machine.listing.machine.favorite = true;
        view.machines = Arc::new(vec![machine.clone()]);
        view.migration_notice = Some(pc_image::machine::Migration {
            adopted: vec!["dos-games".into()],
            skipped: Vec::new(),
        });
        let (texts, _) = drawn(&mut view);
        let at = |label: &str| {
            texts
                .iter()
                .position(|t| t == label)
                .unwrap_or_else(|| panic!("{label}: {texts:?}"))
        };
        let order = [
            at("Library"),
            at("Machines (1)"),
            at("Recently played (0)"),
            at("Favourites (1)"),
            at("Images (0)"),
            at("Installation media (0)"),
            at("Firmware (0)"),
            at("Machine files (0)"),
            at("Remotes"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{texts:?}");
        assert!(texts
            .iter()
            .any(|t| t == "Your desktop sessions are machines now."));
        assert!(texts.iter().any(|t| t.starts_with("DOS games")));
        assert!(texts.iter().any(|t| t == "Never started"));
        // Selected, the details pane names its id, source and actions.
        view.machine = Some("dos-games".into());
        view.selected = Some(machine.image_row());
        let (texts, _) = drawn(&mut view);
        for wanted in [
            "Source image",
            "Media per drive",
            "Settings that differ from the image",
            "Gaming mode",
            "Insert media…",
            "Save as new image…",
            "Disk layers…",
            "Duplicate…",
            "Rename…",
            "Reset to image…",
            "Delete machine…",
        ] {
            assert!(texts.iter().any(|t| t == wanted), "{wanted}: {texts:?}");
        }
        assert!(texts
            .iter()
            .any(|t| t.starts_with("Machine · workspace id dos-games")));
    }

    /// Start on an image with no machine makes one named after it and
    /// starts it; with machines it asks which, the last run first.
    #[test]
    fn start_on_an_image_creates_a_machine_or_asks_which() {
        let temp = assembly::test_support::TempDir::new("start-on-image");
        let mut view = LibraryView::synthetic(temp.path().join("store"));
        let image_row = row("x", "X", "a", &["games:1.0.0"]).image_row();
        view.start_image(image_row.clone());
        assert!(view.busy, "a machine is being created");
        assert!(view.machine_dialog.is_none());
        view.shutdown();
        view.busy = false;
        let mut old = row("old", "Old", "a", &["games:1.0.0"]);
        old.listing.machine.last_run = Some("2026-11-01 10:00 UTC".into());
        let mut new = row("new", "New", "a", &["games:1.0.0"]);
        new.listing.machine.last_run = Some("2026-11-15 10:00 UTC".into());
        view.machines = Arc::new(vec![old, new, row("other", "Other", "b", &["x:1"])]);
        view.start_image(image_row);
        let Some(Dialog::Start(dialog)) = &view.machine_dialog else {
            panic!("the Start… chooser");
        };
        assert_eq!(
            dialog
                .choices
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
        assert_eq!(dialog.pick.as_deref(), Some("new"));
        assert_eq!(dialog.new_name, "games:1.0.0");
    }

    fn record(machine: Option<&str>, title: &str) -> Recovery {
        Recovery {
            schema: 1,
            kind: crate::library_worker::WorkspaceKind::Disposable,
            reference: "games:1.0.0".into(),
            image: image("a"),
            title: title.into(),
            recovery: true,
            machine: machine.map(str::to_owned),
            name: "disposable-1".into(),
        }
    }

    /// The question asked when a gaming copy stops takes the worker's record
    /// of it, so Keep as new machine… names and keeps the machine's copy.
    #[test]
    fn the_gaming_question_takes_the_copys_machine_from_the_worker() {
        let mut view = LibraryView::new(None);
        view.decision = Some(record(None, "games:1.0.0"));
        view.on_event(crate::library_worker::Event::Recoveries(vec![record(
            Some("dos"),
            "DOS (gaming copy)",
        )]));
        let decision = view.decision.clone().unwrap();
        assert_eq!(decision.machine.as_deref(), Some("dos"));
        view.begin_keep_copy(decision);
        let Some(Dialog::Name(dialog)) = &view.machine_dialog else {
            panic!("the name dialog");
        };
        assert_eq!(dialog.value, "DOS (gaming copy)");
    }

    /// Image › Gaming mode plays the image's own copy at once, never waiting
    /// for (and then following) the selected machine's readiness check.
    #[test]
    fn image_gaming_mode_plays_the_image_without_the_machines_check() {
        let temp = assembly::test_support::TempDir::new("image-gaming");
        let mut view = LibraryView::synthetic(temp.path().join("store"));
        let machine = row("dos", "DOS", "a", &["games:1.0.0"]);
        view.set_machines(vec![machine.clone()]);
        view.machine = Some("dos".into());
        view.selected = Some(machine.image_row());
        view.readiness = None;
        view.image_gaming();
        assert!(view.busy);
        assert!(
            view.pending_launch.is_none(),
            "no launch waits for the machine's check"
        );
        assert_eq!(view.machine.as_deref(), Some("dos"));
        view.shutdown();
    }

    /// Another job's reply never closes a dialog opened meanwhile; Save as
    /// new image… closes on its own machine's reply. An unfinished save is
    /// offered to be finished, not redone.
    #[test]
    fn replies_close_only_their_own_dialog_and_an_unfinished_save_is_finished() {
        let mut view = LibraryView::new(None);
        let mut machine = row("dos", "DOS", "a", &["games:1.0.0"]);
        view.set_machines(vec![machine.clone()]);
        view.machine = Some("dos".into());
        view.selected = Some(machine.image_row());
        view.begin_rename();
        let done = |id: &str| crate::library_worker::Event::MachineDone {
            message: "done".into(),
            select: Some((id.to_owned(), Box::new(machine.image_row()))),
            start: false,
        };
        view.on_event(crate::library_worker::Event::MachineNote(
            "Labels saved".into(),
        ));
        view.on_event(done("other"));
        assert!(matches!(view.machine_dialog, Some(Dialog::Name(_))));
        view.machine_dialog = None;
        view.machine = Some("dos".into());
        view.begin_save_machine("dos");
        assert!(matches!(view.machine_dialog, Some(Dialog::Save(_))));
        view.on_event(done("dos"));
        assert!(view.machine_dialog.is_none());
        machine.listing.publication_pending = true;
        view.set_machines(vec![machine]);
        view.begin_save_machine("dos");
        assert!(matches!(view.machine_dialog, Some(Dialog::Finish { .. })));
    }

    /// The default name a details pane shows is computed once per image and
    /// listing, and is a valid free name however long the image's name.
    #[test]
    fn the_default_name_is_valid_free_and_computed_once() {
        let mut view = LibraryView::new(None);
        let long = "registry.example.org/team/dos-collection-with-a-long-name:1.0.0-preview";
        let mut taken = row("x", &"r".repeat(64), "b", &["x:1"]);
        taken.listing.machine.name = long.chars().take(64).collect();
        view.set_machines(vec![taken]);
        let mut image_row = row("y", "Y", "a", &[long]).image_row();
        image_row.reference = long.into();
        let name = view.default_machine_name(&image_row);
        assert!(image_store::machine::validate_name(&name).is_ok(), "{name}");
        assert!(name.ends_with(" (2)"), "{name}");
        let revision = view.machines_revision;
        assert_eq!(view.default_machine_name(&image_row), name);
        assert_eq!(view.machines_revision, revision);
        assert!(view
            .name_hint
            .as_ref()
            .is_some_and(|(_, r, _)| *r == revision));
    }
}
