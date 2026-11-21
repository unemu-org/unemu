// SPDX-License-Identifier: AGPL-3.0-or-later
//! Explorer-style local image library; machine-images §§2.1, 3 and 7.
//! Store jobs own I/O and PC readiness, launch and publication adapters.
//! The tree, facets and virtualized rows consume platform-neutral summaries.
//! Firmware packages (§5.3) and installation media sets (§5.2) form their own
//! collections, see [`firmware`] and [`install_media`]; a firmware package's
//! metadata and licences are edited with `firmware_metadata`. The
//! configured registries follow the Library group as Remotes, see
//! [`remotes`].

pub mod firmware;
pub(crate) mod firmware_metadata;
pub(crate) mod hardware;
pub mod install_media;
#[cfg(test)]
mod layout_tests;
pub mod machines;
#[cfg(test)]
mod needs_newer_tests;
pub(crate) mod pane;
#[cfg(test)]
mod push_tests;
pub mod remotes;

use crate::{
    commands::Command,
    layers_dialog::LayersDialog,
    registry_transfer::Accounts,
    remote_browse::{BrowseEvent, BrowseRequest},
    remote_pull::{Drive, Mode},
};
use image_store::{media_type as mt, Descriptor, Digest, Store};
use pc_library::machine_files::LibraryEntry;
use remotes::{RemotesAction, RemotesContext};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
};
use ui_kit::shell_icons::Icon;

type Result<T> = std::result::Result<T, String>;
#[cfg(test)]
const REBUILD_BATCH: usize = 8;

pub fn default_store_path() -> Result<PathBuf> {
    image_store::location::default_store_root().map_err(|e| e.to_string())
}
#[cfg(test)]
use crate::library_worker::regular_workspace;
use crate::library_worker::{regular_name, Event, Finish, Recovery, Worker, WorkspaceKind};
pub use pc_library::library_worker::Request;
// The image rows, their sources and the shared texts are the library
// model's; the frame and its collections keep naming them here.
#[cfg(test)]
pub(crate) use library_model::listing::source;
pub(crate) use library_model::listing::{human_size, source_cell, source_hint, ImageRow};
// The worker, which deletes tags and lists snapshots, is the PC library's
// too; the frame's tests still call both here.
#[cfg(test)]
pub(super) use library_model::listing::delete_tag;
// The query the frame sends the worker and the snapshot it answers with
// are the PC library's.
#[cfg(test)]
pub(super) use pc_library::snapshot::snapshot;
use pc_library::snapshot::{image_rights, redistribution, Filter, ViewRow};
pub(super) use pc_library::snapshot::{Query, Snapshot};
// The list tables are the neutral library views'; the frame and its
// collections keep naming them here.
#[cfg(test)]
use library_ui::table::test_support::{CHILD_INDENT, NAME_ICON_ROOM, SCROLL_ROOM};
use library_ui::table::{
    draw_cells, draw_cells_with_hints, fit_columns, group_row, sort_header, table_width, Cell,
    Columns, Lead,
};

/// The Images list's columns, as the machines mock-up lists images, with the
/// remotes mock-up's Source column after the name.
const IMAGE_COLUMNS: Columns<7> = Columns {
    labels: [
        "Name",
        "Source",
        "Tags",
        "Digest",
        "Machines",
        "Redistribution",
        "Size",
    ],
    widths: [260.0, 140.0, 220.0, 120.0, 80.0, 110.0, 90.0],
    cells: [
        Cell::Text,
        Cell::Text,
        Cell::Mono,
        Cell::Mono,
        Cell::Number,
        Cell::Badge,
        Cell::Number,
    ],
    samples: [
        "",
        "local",
        "",
        "00000000…0000",
        "99",
        "user-supplied",
        "999.9 MiB",
    ],
};
/// The store cannot count machines, so the list does not sort by them.
const IMAGE_MACHINES_COLUMN: usize = 4;
/// The details pane's first height, and the least either side of the split
/// keeps: a few list rows with their header and footer, a few pane lines.
/// On a window 1280 px wide or wider the first height shows the whole pane
/// of a machine or an image without hints or warnings, its actions and the
/// note under them included; a longer pane, such as one with a newer
/// firmware hint, scrolls with its scroll bar shown.
const DETAILS_HEIGHT: f32 = 460.0;
const DETAILS_MIN_HEIGHT: f32 = 120.0;
const LIST_MIN_HEIGHT: f32 = 160.0;
/// The mock-ups' gap between the library's panes and around them.
const PANE_GAP: i8 = 3;
/// The tree pane, on the field colour as the mock-ups' `.pane`. It keeps no
/// gap on its right: the list's and the details pane's gaps lie there.
const TREE_PANE: pane::Pane = pane::Pane {
    ground: pane::Ground::Field,
    gap: egui::Margin {
        left: PANE_GAP,
        right: 0,
        top: PANE_GAP,
        bottom: PANE_GAP,
    },
};
/// The list pane, on the field colour as the mock-ups' `.pane`. Its gap and
/// the details pane's make the mock-ups' 6 px split between them.
const LIST_PANE: pane::Pane = pane::Pane {
    ground: pane::Ground::Field,
    gap: egui::Margin::same(PANE_GAP),
};
/// The details pane, on the second face colour as the mock-ups' `.details`.
const DETAILS_PANE: pane::Pane = pane::Pane {
    ground: pane::Ground::Face2,
    gap: egui::Margin::same(PANE_GAP),
};
/// The superseded firmware an image binds, read from its library summary.
fn image_newer(image: &ImageRow) -> Vec<crate::firmware_choices::Newer> {
    image
        .summary
        .as_ref()
        .map(|s| crate::firmware_choices::newer_of(&crate::firmware_choices::in_summary(s)))
        .unwrap_or_default()
}
/// An image row's cells; `machines` counts the machines made from it. A
/// superseded firmware puts the quiet "↑ newer BIOS" badge after the name.
fn row_cells(row: &ViewRow, machines: usize) -> [String; 7] {
    let image = &row.image;
    let summary = image.summary.as_ref();
    let mut name = image.name().to_owned();
    if !image_newer(image).is_empty() {
        name.push_str("  ");
        name.push_str(crate::firmware_choices::BADGE);
    }
    [
        name,
        source_cell(&image.descriptor),
        image.references(),
        pane::short_digest(image.descriptor.digest.hex()),
        machines.to_string(),
        summary
            .map(|s| redistribution(s).to_owned())
            .unwrap_or_default(),
        summary
            .map(|s| human_size(s.stored_size))
            .unwrap_or_else(|| "Pending".into()),
    ]
}

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
        Filter::Machines => Some(Icon::Machine),
        Filter::All => Some(Icon::Image),
        Filter::MachineFiles => Some(Icon::MachineFile),
        Filter::Media => Some(Icon::Media),
        Filter::Firmware => Some(Icon::Firmware),
        _ => None,
    }
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

/// The library's empty state while it holds no machine image: where the
/// first one comes from. Nothing contacts a registry until Get templates…
/// shows the default registry's templates in Remotes.
fn empty_library(ui: &mut egui::Ui, commands: &mut Vec<Command>) {
    ui.group(|ui| {
        // The get-templates mock-up's empty card: its picture beside the text.
        ui.horizontal_top(|ui| {
            ui_kit::shell_icons::show(ui, Icon::EmptyLibrary);
            ui.vertical(|ui| empty_library_text(ui, commands));
        });
    });
}
fn empty_library_text(ui: &mut egui::Ui, commands: &mut Vec<Command>) {
    ui.strong("No machine images yet");
    ui.add(
        egui::Label::new(
            "Get templates… lists the ready-made open machines of the default registry under Remotes, to pull: open firmware with its source and a blank disk, no operating system.",
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

/// What a selected image's pane asks for.
enum ImageAction {
    Command(Command),
    Favourite(bool),
    Labels,
    /// Show a machine made from the image, by its id.
    Open(String),
    /// "Switch to …": Settings… with that firmware choice made.
    Switch(crate::firmware_choices::Slot, &'static str),
    /// Select another image: the newer tag the hint links to.
    ShowImage(Box<ImageRow>),
}

/// The dry runs of every local image, or why they could not be read, with
/// the library's tags they were read for.
type LibraryPlans = (
    Vec<(String, Digest)>,
    std::result::Result<Vec<crate::registry_transfer::PlannedImage>, String>,
);

pub struct LibraryView {
    pub root: Option<PathBuf>,
    worker: Option<Worker>,
    snapshot: Arc<Snapshot>,
    query: Query,
    pub selected: Option<ImageRow>,
    pub readiness: Option<Result<pc_image::inspection::ImageDetails>>,
    /// What the last check read from the image's manifest and configuration,
    /// by digest: the drives a floppy inserted before Start goes into, and
    /// the names and annotations the details panes show.
    checked: Option<(Digest, Box<crate::library_worker::CheckedImage>)>,
    /// The selected image's hardware, drives and firmware as the details
    /// panes name them, by digest; made once per check and snapshot.
    facts: Option<(Digest, hardware::ImageFacts)>,
    pub active: Option<(ImageRow, String, WorkspaceKind)>,
    pub layers: Option<LayersDialog>,
    pub busy: bool,
    pending_operation: bool,
    pub status: Option<String>,
    pub error: Option<String>,
    pub launched: Option<crate::library_worker::DesktopLaunch>,
    /// The pending inserts of the machine's list that the launched session
    /// put into its drives.
    pub launched_inserts: Vec<image_store::machine::PendingInsert>,
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
    /// Whether the window wears the classic skin, which sinks the panes into
    /// a bevel; the modern skin outlines them with a flat line.
    pub classic: bool,
    /// File › New machine… while it is open.
    pub new_machine: Option<crate::new_machine::Wizard>,
    /// The wizard asked for Import ROM folder…; the shell picks the folder.
    pub firmware_import_wanted: bool,
    /// A created image was selected; the next frame drops the machine-file selection.
    clear_file_selection: bool,
    /// Settings on an image while it is open.
    pub image_settings: Option<crate::image_settings::ImageSettings>,
    /// Delete tag… of the selected firmware package, apart from an image's.
    delete_package: Option<ImageRow>,
    /// Show in library of a pulled installation media set: the tag the
    /// Installation media collection selects once it lists it.
    media_reselect: Option<String>,
    /// "Switch to …": the firmware choice the next Settings sheet opens with.
    settings_switch: Option<(crate::firmware_choices::Slot, String)>,
    /// Push to… of one library item while it is open.
    pub push: Option<crate::registry_transfer::PushDialog>,
    /// Push to… of several items, or Push here… of a registry or
    /// namespace, while it is open.
    pub push_batch: Option<library_model::push_batch::PushBatch>,
    /// Several items of Images, Installation media or Firmware selected
    /// with Ctrl (⌘ on macOS) or Shift, by tag, in the order they were
    /// added; empty, or one tag alone, while the single selection stands.
    pub library_multi: Vec<String>,
    /// Where Shift selects from: the row clicked last, by tag.
    library_anchor: Option<String>,
    /// The dry run of every local image, for a namespace pane's count of
    /// what could go there, with the library's tags it was read for: an
    /// answer, a failure included, stands until those tags change, so a
    /// failure is not asked again every frame. `plans_wanted` is the
    /// request awaited, with the tags it reads.
    push_plans: Option<LibraryPlans>,
    plans_wanted: Option<(u64, Vec<(String, Digest)>)>,
    /// What the namespace pane's verdicts read of Preferences › Registries,
    /// read once until Preferences close, never every frame.
    here_rules: Option<library_model::push_destination::Rules>,
    /// The counts of the shown node's pane, until the node, the registries
    /// or the dry runs change.
    here_cache: Option<(
        library_model::push_destination::Place,
        u64,
        usize,
        remotes::HereCounts,
    )>,
    /// Registries… of a node's pane: the shell opens Preferences.
    pub registries_wanted: bool,
    /// Show in Remotes of a finished push: the tag the shell shows there.
    pub remote_wanted: Option<image_registry::Reference>,
    /// Image › Pull… while it is open.
    pub pull: Option<crate::registry_transfer::PullDialog>,
    /// Pull… of tags selected in Remotes while it is open.
    pub remote_pull: Option<crate::remote_pull::RemotePull>,
    /// Log in… of a registry in Remotes: the registry whose login the shell
    /// opens in Preferences › Registries.
    pub login_wanted: Option<String>,
    /// Every machine in the store, as the worker last listed them.
    machines: Arc<Vec<machines::MachineRow>>,
    /// The selected machine's id; `selected` then holds its image's row.
    pub machine: Option<String>,
    /// The selected machine's settings that differ from its image, by id;
    /// `None` inside while the worker reads them.
    machine_settings: Option<(String, Option<Result<Vec<machines::SettingDiff>>>)>,
    /// A Machines dialog while it is open.
    machine_dialog: Option<machines::Dialog>,
    /// The labels field of the selected machine.
    machine_labels: String,
    /// The one-time notice of desktop sessions that became machines.
    migration_notice: Option<pc_image::machine::Migration>,
    /// The machine that runs now: its id and name.
    pub active_machine: Option<(String, String)>,
    /// Counts machine listings, so derived text is computed once per listing.
    machines_revision: u64,
    /// The default name of a new machine of an image, for its details pane:
    /// the image digest and listing it was computed for.
    name_hint: Option<(Digest, u64, String)>,
    /// The machine Start on an image just made: floppies inserted into that
    /// image before Start go into it, and into no other machine.
    pub started_from_image: Option<String>,
    /// Insert into… of the Installation media collection: the machine, the
    /// drive and the set the shell opens the media picker on.
    pub picker_wanted: Option<(String, u8, Digest)>,
    /// Names and sizes of host files and folders the worker read for Create
    /// from files…, for the shell to hand to the picker.
    pub host_reads: Vec<(PathBuf, std::result::Result<media::fat12::HostItem, String>)>,
    /// What the worker made of Create and insert, for the shell.
    pub disks_created: Option<std::result::Result<Box<crate::created_media::Created>, String>>,
    /// What the last Start left out, for the status bar once it runs.
    pub start_notes: Vec<String>,
    /// The drives the launched session took back their kept catalogue in,
    /// for the shell once the machine has assembled.
    pub start_catalogue: Vec<crate::kept_catalogue::RestoredDrive>,
    /// The Remotes group: the registries, the node shown and its rows.
    pub remote: remotes::RemotesView,
    /// What the registry dialogs and Remotes read of the compiled-in
    /// platforms, made once.
    pub compatibility: image_store::compat::Compatibility,
    pub(crate) platform_facts: library_model::PlatformFacts,
}
impl LibraryView {
    pub fn with_compatibility(mut self, compatibility: image_store::compat::Compatibility) -> Self {
        self.compatibility = compatibility;
        self
    }
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            worker: None,
            snapshot: Arc::default(),
            // The library opens on Machines.
            query: Query {
                filter: Filter::Machines,
                ..Query::default()
            },
            selected: None,
            readiness: None,
            checked: None,
            facts: None,
            active: None,
            layers: None,
            busy: false,
            pending_operation: false,
            status: None,
            error: None,
            launched: None,
            launched_inserts: Vec::new(),
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
            classic: true,
            new_machine: None,
            firmware_import_wanted: false,
            settings_switch: None,
            delete_package: None,
            clear_file_selection: false,
            image_settings: None,
            push: None,
            push_batch: None,
            library_multi: Vec::new(),
            library_anchor: None,
            push_plans: None,
            plans_wanted: None,
            here_rules: None,
            here_cache: None,
            registries_wanted: false,
            remote_wanted: None,
            pull: None,
            remote_pull: None,
            media_reselect: None,
            login_wanted: None,
            machines: Arc::default(),
            machine: None,
            machine_settings: None,
            machine_dialog: None,
            machine_labels: String::new(),
            migration_notice: None,
            active_machine: None,
            machines_revision: 0,
            name_hint: None,
            started_from_image: None,
            picker_wanted: None,
            host_reads: Vec::new(),
            disks_created: None,
            start_notes: Vec::new(),
            start_catalogue: Vec::new(),
            remote: remotes::RemotesView::new(),
            compatibility: pc_image::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ),
            platform_facts: crate::pc_desktop::PcDesktop::facts(),
        }
    }
    /// Whether the worker runs, to read host files and build disks for
    /// Create from files….
    pub fn can_read_host(&self) -> bool {
        self.worker.is_some() && !self.worker_dead
    }
    /// Every installation media set of the library, as the media picker lists them.
    pub fn picker_sets(&self) -> Vec<crate::media_picker::SetEntry> {
        self.snapshot
            .all_sets
            .iter()
            .filter_map(install_media::picker_set)
            .collect()
    }
    /// The set of manifest `digest` as the media picker lists it, without
    /// listing every other set: drive names look one up every frame.
    pub fn picker_set(&self, digest: &Digest) -> Option<crate::media_picker::SetEntry> {
        self.snapshot
            .all_sets
            .iter()
            .find(|row| row.image.descriptor.digest == *digest)
            .and_then(install_media::picker_set)
    }
    /// The library sources of the disks `insert` put into its drive at
    /// Start, each with its label, its place in the set and the set's count,
    /// in the order they lead the drive's catalogue then: the chosen disk,
    /// then the set's others when all join.
    pub fn inserted_sources(
        &self,
        insert: &image_store::machine::PendingInsert,
    ) -> Vec<(
        image_store::media_set::CatalogueSource,
        String,
        usize,
        usize,
    )> {
        let Some(set) = self
            .snapshot
            .all_sets
            .iter()
            .find(|row| row.image.descriptor.digest == insert.set.digest)
            .and_then(|row| row.image.summary.as_ref()?.media_set.as_ref())
        else {
            return Vec::new();
        };
        let Some(chosen) = set.media.iter().position(|m| m.id == insert.medium) else {
            return Vec::new();
        };
        let others = (0..set.media.len()).filter(|at| insert.all && *at != chosen);
        std::iter::once(chosen)
            .chain(others)
            .map(|at| {
                let medium = &set.media[at];
                (
                    image_store::media_set::CatalogueSource {
                        set: insert.set.digest.clone(),
                        medium_id: medium.id.clone(),
                        blob: medium.blob.clone(),
                    },
                    medium.label.clone(),
                    at,
                    set.media.len(),
                )
            })
            .collect()
    }
    /// The name Start gives a new machine of `row`'s image, computed once per
    /// image and machine listing rather than every frame.
    fn default_machine_name(&mut self, row: &ImageRow) -> String {
        let fresh = |(digest, revision, _): &(Digest, u64, String)| {
            *digest == row.descriptor.digest && *revision == self.machines_revision
        };
        if !self.name_hint.as_ref().is_some_and(fresh) {
            let name = image_store::machine::unique_among(
                row.name(),
                self.machines.iter().map(machines::MachineRow::name),
            );
            self.name_hint = Some((row.descriptor.digest.clone(), self.machines_revision, name));
        }
        self.name_hint
            .as_ref()
            .map(|(_, _, name)| name.clone())
            .unwrap_or_default()
    }
    /// Every machine of the store, as the worker last listed them.
    pub fn machine_rows(&self) -> Arc<Vec<machines::MachineRow>> {
        self.machines.clone()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn synthetic(root: PathBuf) -> Self {
        let mut view = Self::new(Some(root.clone()));
        view.worker = Some(Worker::synthetic(root));
        view
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.worker.is_none() && !self.worker_dead {
            if let Some(root) = self.root.clone() {
                let ctx = ctx.clone();
                self.worker = Some(Worker::spawn_with_compatibility(
                    root,
                    std::sync::Arc::new(move || ctx.request_repaint()),
                    self.compatibility.clone(),
                ));
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
            let message = self.error.clone().unwrap_or_default();
            self.fail_remotes(&message);
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
        if let Some(dialog) = &mut self.push_batch {
            dialog.fail(&message);
        }
        self.plans_wanted = None;
        if let Some(dialog) = &mut self.pull {
            dialog.fail(&message);
        }
        if let Some(dialog) = &mut self.remote_pull {
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
                        // Its records too: a push adds where it went.
                        selected.descriptor.clone_from(&current.image.descriptor);
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
                if let Some(reference) = self.media_reselect.clone() {
                    if let Some(row) = snapshot
                        .media_rows
                        .iter()
                        .find(|r| r.image.has_tag(&reference))
                    {
                        self.media_reselect = None;
                        self.media_selected = Some(row.clone());
                    }
                }
                let tags_changed = self.snapshot.tags != snapshot.tags;
                self.snapshot = snapshot;
                self.refresh_facts();
                if tags_changed {
                    self.remote_library_changed();
                }
            }
            Event::HostRead(path, result) => self.host_reads.push((path, *result)),
            Event::DisksCreated(result) => self.disks_created = Some(result),
            Event::StartNotes(notes) => self.start_notes = notes,
            Event::StartCatalogue(drives) => self.start_catalogue = drives,
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
                if self.new_machine.is_some() || self.image_settings.is_some() {
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
                    self.checked = machine.map(|checked| (id.clone(), checked));
                    self.refresh_facts();
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
            Event::Launched(session, row, name, kind, inserted) => {
                let row = *row;
                self.active_machine = (kind == WorkspaceKind::Machine).then(|| {
                    let title = self
                        .machine_row(&name)
                        .map_or_else(|| name.clone(), |m| m.name().to_owned());
                    (name.clone(), title)
                });
                if kind == WorkspaceKind::Machine {
                    self.machine = Some(name.clone());
                }
                self.selected = Some(row.clone());
                self.active = Some((row, name, kind));
                self.launched = Some(*session);
                self.launched_inserts = inserted;
                self.pending_operation = false;
                self.busy = false;
            }
            Event::Finished => {
                self.busy = self.pending_operation;
                self.active = None;
                self.active_machine = None;
                self.started_from_image = None;
                if !self.pending_operation {
                    self.status = Some("Ready".into());
                }
            }
            Event::Layers(name, result, source, checked, summary) => {
                self.busy = false;
                if let Some(dialog) = &mut self.layers {
                    // The drives and ROMs go by the names the details pane
                    // gives them; a failed check keeps what the dialog had.
                    if let Ok(details) = &result {
                        let facts = hardware::ImageFacts::new(
                            details,
                            checked.as_deref(),
                            summary.as_deref(),
                            &self.snapshot.rom_names,
                        );
                        dialog.set_image(checked.map(|checked| *checked), facts.firmware);
                    }
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
                self.settings_opened(opened);
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
                // A deleted package's pane goes; the list follows with the
                // next snapshot.
                if self
                    .firmware_selected
                    .as_ref()
                    .is_some_and(|row| row.image.reference == reference)
                {
                    self.firmware_selected = None;
                    self.firmware_package = None;
                }
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
                // The worker's record of a stopped gaming copy names the
                // machine it was made from and its title; the question asked
                // when it stopped takes them.
                if let Some(decision) = &mut self.decision {
                    if let Some(row) = rows.iter().find(|row| row.name == decision.name) {
                        *decision = row.clone();
                    }
                }
                self.recoveries = rows;
            }
            // Package refreshes never touch a busy flag: a Create may still run.
            Event::Packages(result) => {
                if let Some(wizard) = &mut self.new_machine {
                    match result {
                        Ok(choices) => wizard.set_store(choices),
                        Err(error) => wizard.error = Some(error),
                    }
                } else if let Some(settings) = &mut self.image_settings {
                    match result {
                        Ok((packages, _, _)) => settings.set_packages(packages),
                        Err(error) => settings.error = Some(error),
                    }
                }
            }
            Event::Created(Ok(row)) => {
                // File › New machine… makes a machine from the image too,
                // unless the user opted out.
                let machine = self
                    .new_machine
                    .as_ref()
                    .filter(|w| w.busy)
                    .and_then(|w| w.machine_choice());
                if self.new_machine.as_ref().is_some_and(|w| w.busy) {
                    self.new_machine = None;
                }
                self.status = Some(format!("Created {}", row.reference));
                self.show_created((*row).clone());
                if let Some(name) = machine {
                    self.busy = true;
                    self.send(Request::Machine(
                        crate::library_worker::MachineJob::Create {
                            image: row,
                            name,
                            start: false,
                        },
                    ));
                }
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
                        // Pull… from Remotes reports its own pulls.
                        crate::registry_transfer::TransferEvent::Pulled { id, .. } => {
                            self.pull.is_none()
                                && !self.remote_pull.as_ref().is_some_and(|d| d.owns(*id))
                        }
                        _ => self.pull.is_none(),
                    };
                    if closed {
                        self.status = Some(outcome.into());
                    }
                }
                self.on_push_event(&event);
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
                let mut insert = None;
                if let Some(dialog) = &mut self.remote_pull {
                    let running = dialog.running();
                    dialog.on_event(&event);
                    let pulled = dialog.pulled();
                    if running && !dialog.running() && !pulled.is_empty() {
                        self.status = Some(format!("Pulled {}", pulled.join(", ")));
                    }
                    insert = dialog.take_insert();
                }
                // Pull and insert…: the set's first disk waits in the drive.
                if let Some((machine, insert)) = insert {
                    let drive = insert.drive.clone();
                    self.set_pending(&machine, &drive, Some(insert));
                }
                if let Some(request) = follow_up {
                    self.send(Request::Transfer(request));
                }
            }
            Event::Machines(Ok(rows)) => {
                self.machines = rows;
                self.machines_revision += 1;
                // A machine that is gone (deleted elsewhere) is no Start target.
                if let Some(id) = self.machine.clone() {
                    match self.machine_row(&id).map(machines::MachineRow::image_row) {
                        Some(row) => {
                            if let Some(selected) = &mut self.selected {
                                if selected.descriptor.digest != row.descriptor.digest {
                                    // Save as new image… moved it onto a new image.
                                    *selected = row;
                                    self.readiness = None;
                                    self.recheck();
                                } else {
                                    selected.reference = row.reference;
                                    // A listing can carry the summary the
                                    // selection lacked; the pane names its
                                    // firmware from it.
                                    if row.summary.is_some() {
                                        selected.summary = row.summary;
                                    }
                                }
                            }
                            self.refresh_facts();
                        }
                        None if !self.busy => {
                            self.machine = None;
                            self.selected = None;
                            self.readiness = None;
                        }
                        None => (),
                    }
                }
            }
            Event::Machines(Err(error)) => self.error = Some(error),
            Event::Migrated(migration) => self.migration_notice = Some(migration),
            Event::MachineNote(message) => self.status = Some(message),
            Event::MachineDone {
                message,
                select,
                start,
            } => {
                self.busy = false;
                self.status = Some(message);
                // Only the Save dialog waits for its machine's reply; another
                // job's reply never closes a dialog opened meanwhile.
                if let (Some(machines::Dialog::Save(dialog)), Some((id, _))) =
                    (&self.machine_dialog, &select)
                {
                    if dialog.id == *id {
                        self.machine_dialog = None;
                    }
                }
                if let Some((id, row)) = select {
                    self.show_machine(id.clone(), *row);
                    if start {
                        self.started_from_image = Some(id);
                        self.pending_launch = self
                            .selected
                            .as_ref()
                            .map(|row| (row.descriptor.digest.clone(), WorkspaceKind::Machine));
                    }
                } else if let Some(id) = self.machine.clone() {
                    // Settings, a reset or new labels change the details pane.
                    self.machine_settings = Some((id.clone(), None));
                    self.send(Request::Machine(
                        crate::library_worker::MachineJob::ReadSettings(id),
                    ));
                    self.recheck();
                }
            }
            Event::MachineSettings(id, result) => {
                if let Some((wanted, diffs)) = &mut self.machine_settings {
                    if *wanted == id {
                        *diffs = Some(
                            result
                                .map_err(|e| e.to_string())
                                .and_then(|settings| machines::setting_diffs(&settings)),
                        );
                    }
                }
            }
            Event::MachineSettingsOpened(result) => {
                self.busy = false;
                let opened = result.and_then(|opened| {
                    let (id, name, row, source, machine, tags) = *opened;
                    let running = self.machine_running(&id);
                    crate::image_settings::ImageSettings::new(row, source, tags, running)
                        .map(|settings| settings.for_machine(id, name, machine))
                });
                self.settings_opened(opened);
            }
            Event::MachineSettingsApplied(result) => {
                self.busy = false;
                match result {
                    Ok(change) => {
                        let name = self
                            .image_settings
                            .as_ref()
                            .and_then(|s| s.machine_name())
                            .unwrap_or("the machine")
                            .to_owned();
                        let mut status = format!(
                            "Settings of \u{201c}{name}\u{201d} saved; they apply at its next start"
                        );
                        if change.cmos_reset {
                            status.push_str(&format!(
                                ", with its CMOS reset for the {} layout",
                                change
                                    .cmos_layout
                                    .map_or("new BIOS's", assembly::CmosLayout::label)
                            ));
                        } else if !change.cmos_changes.is_empty() {
                            status.push_str("; its CMOS bytes were recomputed");
                        }
                        status.push_str(". C: and its disk layers are unchanged");
                        if let Some(note) = &change.cmos_note {
                            status.push_str("; ");
                            status.push_str(note);
                        }
                        self.status = Some(status);
                        if let Some(settings) = &mut self.image_settings {
                            settings.applied();
                            if settings.close_after_apply {
                                self.image_settings = None;
                            }
                        }
                        if let Some(id) = self.machine.clone() {
                            self.machine_settings = Some((id.clone(), None));
                            self.send(Request::Machine(
                                crate::library_worker::MachineJob::ReadSettings(id),
                            ));
                        }
                    }
                    Err(error) => match &mut self.image_settings {
                        Some(settings) => settings.error = Some(error),
                        None => self.error = Some(error),
                    },
                }
            }
            Event::CommitRefused(refused) => {
                self.busy = false;
                self.commit_refused(refused.error, refused.tags);
            }
            Event::Browse(event) => self.on_browse(event),
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
        // The worker answers one request at a time: a slow registry listing
        // must not hold a machine's start behind it.
        if request.launches() {
            self.stop_remotes();
        }
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
        self.push_batch = None;
        self.pull = None;
        self.remote_pull = None;
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown();
        }
        self.worker_dead = true;
    }
    /// Give Remotes the logins of Preferences › Registries; registries
    /// already open read their list from these preferences again.
    pub fn attach_remotes(&mut self, accounts: Accounts) {
        self.here_rules = None;
        self.here_cache = None;
        self.remote.attach_remotes(accounts);
    }
    /// A registry's node is shown instead of a library collection.
    pub fn remote_shown(&self) -> bool {
        self.remote.remote_shown()
    }
    fn on_browse(&mut self, event: BrowseEvent) {
        self.remote.on_browse(event);
    }
    /// Stop every registry's listing and reads, as before every start of a
    /// machine: the worker answers one request at a time, so a slow registry
    /// must not hold the start behind it.
    pub fn stop_remotes(&mut self) {
        self.remote.stop_remotes();
    }
    /// The worker is gone: every running registry job ends with `message`.
    fn fail_remotes(&mut self, message: &str) {
        self.remote.fail_remotes(message);
    }
    /// The library's tags changed: Remotes compares every read tag with them
    /// again.
    fn remote_library_changed(&mut self) {
        let actions = self.remote.remote_library_changed();
        self.apply_remotes(actions, &mut None);
    }
    /// Preferences closed: the registries are read again, and the one whose
    /// Log in… opened them lists again.
    fn remote_preferences_closed(&mut self) {
        let actions = self.remote.remote_preferences_closed();
        self.apply_remotes(actions, &mut None);
    }
    /// F5 and the toolbar's Refresh: list the shown registry again.
    pub fn refresh_remote(&mut self) {
        let actions = self.remote.refresh_remote();
        self.apply_remotes(actions, &mut None);
    }
    /// File › Get templates… and the empty library's button: Remotes › the
    /// default registry › `unemu/` › Machine images, listed first if this
    /// session has not.
    pub fn show_templates(&mut self, accounts: Accounts, selected_file: &mut Option<usize>) {
        let actions = self.remote.show_templates(accounts);
        self.apply_remotes(actions, selected_file);
    }
    /// Show a remote node. It replaces the library's selection, so Start
    /// never targets something no longer shown, and lists its registry the
    /// first time in this session.
    fn select_remote(&mut self, node: remotes::Node, selected_file: &mut Option<usize>) {
        self.library_multi.clear();
        self.selected = None;
        self.machine = None;
        self.machine_settings = None;
        self.readiness = None;
        self.pending_launch = None;
        self.firmware_selected = None;
        self.media_selected = None;
        *selected_file = None;
        let actions = self.remote.select_remote(node);
        self.apply_remotes(actions, selected_file);
    }
    /// The Remotes group under Library in the tree.
    fn remote_tree(&mut self, ui: &mut egui::Ui, selected_file: &mut Option<usize>) {
        let actions = self.remote.remote_tree(ui);
        self.apply_remotes(actions, selected_file);
    }
    /// The list of the shown remote node.
    fn remote_list(&mut self, ui: &mut egui::Ui) {
        let drives = self.stopped_drives();
        let context = RemotesContext {
            facts: &self.platform_facts,
            classic: self.classic,
            tags: &self.snapshot.tags,
            drives: &drives,
            here: None,
        };
        let actions = self.remote.remote_list(ui, &context);
        self.apply_remotes(actions, &mut None);
    }
    /// What the list asked for, as if it had been drawn and clicked.
    #[cfg(test)]
    fn apply_list(&mut self, host: &str, answer: remotes::ListAnswer) {
        let drives = self.stopped_drives();
        let context = RemotesContext {
            facts: &self.platform_facts,
            classic: self.classic,
            tags: &self.snapshot.tags,
            drives: &drives,
            here: None,
        };
        let actions = self.remote.apply_list(host, answer, &context);
        self.apply_remotes(actions, &mut None);
    }
    /// Whether Pull… has something to pull: a selected tag it can pull, or
    /// of several, one it does not leave out.
    pub fn can_pull_remote(&self) -> bool {
        // The shell asks every frame; whether a tag can be pulled does not
        // depend on the drives, so they are not listed here.
        let context = RemotesContext {
            facts: &self.platform_facts,
            classic: self.classic,
            tags: &self.snapshot.tags,
            drives: &[],
            here: None,
        };
        self.remote.can_pull_remote(&context)
    }
    /// Pull…, Pull and create machine… or Pull and insert… of the selected
    /// tags: the dialog opens, and pulls once the user confirms.
    pub fn begin_remote_pull(&mut self, mode: Mode) {
        let drives = self.stopped_drives();
        let context = RemotesContext {
            facts: &self.platform_facts,
            classic: self.classic,
            tags: &self.snapshot.tags,
            drives: &drives,
            here: None,
        };
        self.remote
            .begin_remote_pull(mode, &context, &mut self.remote_pull);
    }
    /// The details pane while a remote node is shown.
    fn remote_details(&mut self, ui: &mut egui::Ui) {
        let drives = self.stopped_drives();
        let here = self.here_counts();
        let context = RemotesContext {
            facts: &self.platform_facts,
            classic: self.classic,
            tags: &self.snapshot.tags,
            drives: &drives,
            here,
        };
        let actions = self.remote.remote_details(ui, &context);
        self.apply_remotes(actions, &mut None);
    }
    /// Contents… of a remote tag, while it is open.
    fn remote_contents(&mut self, ctx: &egui::Context, classic: bool) {
        if self.remote.contents.is_none() {
            return;
        }
        let drives = self.stopped_drives();
        let context = RemotesContext {
            facts: &self.platform_facts,
            classic,
            tags: &self.snapshot.tags,
            drives: &drives,
            here: None,
        };
        self.remote.remote_contents(ctx, &context);
    }
    /// Send a browsing request to the worker; a worker that is gone ends it
    /// at once with the reason, so the registry says why it lists nothing.
    fn send_browse(&mut self, request: BrowseRequest) {
        let ended = remotes::unsent(&request);
        self.send(Request::Browse(request));
        if self.worker_dead {
            let message = self
                .error
                .clone()
                .unwrap_or_else(|| "Image store worker stopped".into());
            self.on_browse(ended(message));
        }
    }
    /// What Remotes asked of the library, in its order. Only the tree and
    /// File › Get templates… show a node, which clears `selected_file`.
    fn apply_remotes(&mut self, actions: Vec<RemotesAction>, selected_file: &mut Option<usize>) {
        for action in actions {
            match action {
                RemotesAction::Browse(request) => self.send_browse(request),
                RemotesAction::Select(node) => self.select_remote(node, selected_file),
                RemotesAction::ShowInLibrary {
                    tag,
                    artifact_type,
                    image,
                } => self.show_in_library(&tag, &artifact_type, image),
                RemotesAction::NewMachine { tag, image, name } => {
                    self.new_machine_from(&tag, image, &name)
                }
                RemotesAction::BeginPull(mode) => self.begin_remote_pull(mode),
                RemotesAction::LogIn(host) => self.login_wanted = Some(host),
                RemotesAction::PushHere(place) => self.begin_push_here(place),
                RemotesAction::Registries => self.registries_wanted = true,
                RemotesAction::Insert { machine, unit, set } => {
                    self.picker_wanted = Some((machine, unit, set))
                }
            }
        }
    }
    /// The floppy drives of the library's machines that are stopped: every
    /// one but the machine that runs.
    fn stopped_drives(&self) -> Vec<Drive> {
        let running = self.active_machine.as_ref().map(|(id, _)| id.as_str());
        self.machines
            .iter()
            .filter(|machine| Some(machine.id()) != running)
            .flat_map(|machine: &machines::MachineRow| {
                machine.drives.iter().map(|&(unit, geometry)| {
                    let letter = crate::media_picker::drive_name(unit);
                    let pending = machine
                        .listing
                        .machine
                        .pending
                        .iter()
                        .find(|p| p.drive == letter)
                        .map(|p| self.pending_label(p));
                    let created = machine
                        .created
                        .iter()
                        .find(|c| c.drive == letter)
                        .map(|c| c.title.clone());
                    crate::remote_pull::pc_drive(
                        machine.id().to_owned(),
                        machine.name().to_owned(),
                        unit,
                        geometry,
                        pending.or(created),
                    )
                })
            })
            .collect()
    }

    /// Select `tag` in the library collection of its artifact type: an image
    /// at once, a set or a package once the library lists it.
    fn show_in_library(&mut self, tag: &str, artifact_type: &str, image: Descriptor) {
        match artifact_type {
            mt::MEDIA_SET | mt::FIRMWARE => {
                self.remote.node = None;
                self.selected = None;
                self.machine = None;
                self.machine_settings = None;
                self.readiness = None;
                self.pending_launch = None;
                self.clear_file_selection = true;
                self.query.text.clear();
                if artifact_type == mt::FIRMWARE {
                    self.query.filter = Filter::Firmware;
                    self.media_selected = None;
                    self.firmware_reselect = Some(tag.to_owned());
                } else {
                    self.query.filter = Filter::Media;
                    self.firmware_selected = None;
                    self.media_reselect = Some(tag.to_owned());
                }
                self.send(Request::Query(self.query.clone()));
            }
            _ => self.select_image(ImageRow {
                reference: tag.to_owned(),
                aliases: Vec::new(),
                descriptor: image,
                summary: None,
                failure: None,
            }),
        }
    }

    /// New machine from image… on a pulled image, selected in Images.
    fn new_machine_from(&mut self, tag: &str, image: Descriptor, name: &str) {
        let row = ImageRow {
            reference: tag.to_owned(),
            aliases: Vec::new(),
            descriptor: image,
            summary: None,
            failure: None,
        };
        self.select_image(row.clone());
        self.new_machine_named(row, name);
    }

    /// The machine of image `digest`, once a check has read it.
    pub fn selected_machine(&self, digest: &Digest) -> Option<&assembly::MachineConfig> {
        self.checked
            .as_ref()
            .filter(|(id, _)| id == digest)
            .and_then(|(_, checked)| checked.draft.as_ref())
            .map(|draft| &draft.config)
    }
    /// Name the selected image's hardware, drives and firmware again, after a
    /// check or a snapshot that may name its firmware packages.
    fn refresh_facts(&mut self) {
        let (Some(row), Some(Ok(details))) = (&self.selected, &self.readiness) else {
            return;
        };
        let digest = row.descriptor.digest.clone();
        let facts = hardware::ImageFacts::new(
            details,
            self.checked_image(),
            row.summary.as_ref(),
            &self.snapshot.rom_names,
        );
        self.facts = Some((digest, facts));
    }
    /// What the selected image's check read from its manifest and
    /// configuration.
    pub(super) fn checked_image(&self) -> Option<&crate::library_worker::CheckedImage> {
        let digest = &self.selected.as_ref()?.descriptor.digest;
        self.checked
            .as_ref()
            .filter(|(id, _)| id == digest)
            .map(|(_, checked)| checked.as_ref())
    }
    /// The selected image's facts, while its check stands.
    pub(super) fn image_facts(&self) -> Option<&hardware::ImageFacts> {
        let digest = &self.selected.as_ref()?.descriptor.digest;
        if !matches!(self.readiness, Some(Ok(_))) {
            return None;
        }
        self.facts
            .as_ref()
            .filter(|(id, _)| id == digest)
            .map(|(_, facts)| facts)
    }
    pub fn ready(&self) -> bool {
        !self.busy && matches!(self.readiness, Some(Ok(_)))
    }
    /// "Needs unemupc X or newer" when this release does not read the
    /// selection's image: the selected machine's, else the selected image's
    /// (machine-images §5.6). Nothing that reads its config is offered.
    pub fn selection_needs_newer(&self) -> Option<String> {
        if self.machine.is_some() {
            if let Some(reason) = self
                .selected_machine_row()
                .and_then(|row| row.needs_newer.clone())
            {
                return Some(reason);
            }
        }
        self.selected.as_ref()?.summary.as_ref()?.needs_newer()
    }
    /// The reason for what Push… would preselect: the selected firmware
    /// package in its collection, else the selection.
    pub fn target_needs_newer(&self) -> Option<String> {
        if self.query.filter.is_firmware() {
            return self
                .firmware_selected
                .as_ref()?
                .image
                .summary
                .as_ref()?
                .needs_newer();
        }
        self.selection_needs_newer()
    }
    /// Refuse an action that reads the config of a stopped selection this
    /// release does not read, saying why. A running session is always one
    /// this release reads.
    fn refused_as_newer(&mut self, reason: Option<String>) -> bool {
        match reason.filter(|_| self.active.is_none()) {
            Some(reason) => {
                self.error = Some(reason);
                true
            }
            None => false,
        }
    }
    /// Start (or Gaming mode, with a disposable `kind`) the selection: the
    /// selected machine, or the selected image, which Start runs in a
    /// machine of its own.
    pub fn launch(&mut self, kind: WorkspaceKind) {
        // Several rows selected: a command of one item takes none.
        if self.several_selected() {
            return;
        }
        if self.busy {
            return;
        }
        if let Some(reason) = self.selection_needs_newer() {
            self.error = Some(reason);
            return;
        }
        // Before the readiness check too: it waits behind a registry listing.
        self.stop_remotes();
        if let Some(row) = self.selected.clone() {
            if self.machine.is_none() && !kind.disposable() {
                self.start_image(row);
                return;
            }
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
            match self.machine.clone() {
                Some(id) => self.send(Request::LaunchMachine(id, kind.disposable())),
                None => self.send(Request::Launch(row, kind, None)),
            }
        }
    }
    pub fn resume(&mut self, row: ImageRow, name: String, kind: WorkspaceKind) {
        self.stop_remotes();
        self.pending_operation = true;
        self.busy = true;
        self.status = Some("Reopening image workspace…".into());
        if kind == WorkspaceKind::Machine {
            self.send(Request::LaunchMachine(name, false));
        } else {
            self.send(Request::Launch(row, kind, Some(name)));
        }
    }
    pub fn cancel_operation(&mut self) {
        self.pending_operation = false;
        self.busy = false;
    }
    /// Hand the stopped session's floppy catalogues to the worker, ahead of
    /// [`Self::finish_with_failure`]: a library machine keeps them for its
    /// next Start.
    pub fn keep_catalogue(&mut self, drives: Vec<crate::kept_catalogue::StoppedDrive>) {
        if self.active.is_some() {
            self.send(Request::KeepCatalogue(drives));
        }
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
                    machine: None,
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
    /// Check the selection again: a machine with its own journals, an image
    /// as stored.
    pub fn recheck(&mut self) {
        if let Some(row) = self.selected.clone() {
            // An image this release does not read is not checked: the
            // reason is known from its manifest, and the check would only
            // refuse it.
            if let Some(reason) = self.selection_needs_newer() {
                self.readiness = Some(Err(reason));
                return;
            }
            self.readiness = None;
            self.send(Request::Check(row.clone(), self.machine.clone()));
        }
    }
    pub fn open_layers(&mut self) {
        // Several rows selected: a command of one item takes none.
        if self.several_selected() {
            return;
        }
        if self.refused_as_newer(self.selection_needs_newer()) {
            return;
        }
        let machine = self
            .selected_machine_row()
            .map(|m| (m.image_row(), m.id().to_owned(), m.name().to_owned()));
        let target = self.active.clone().or_else(|| match &machine {
            Some((row, id, _)) => Some((row.clone(), id.clone(), WorkspaceKind::Machine)),
            None => self.selected.clone().map(|row| {
                let name = regular_name(&row.descriptor.digest);
                (row, name, WorkspaceKind::Regular)
            }),
        });
        let title = match (&self.active_machine, &machine, &self.active) {
            (Some((_, name)), _, _) => name.clone(),
            (None, Some((_, _, name)), None) => name.clone(),
            _ => target
                .as_ref()
                .map_or("Machine", |(row, _, _)| row.name())
                .to_owned(),
        };
        self.layers = Some(LayersDialog::for_workspace(
            target.as_ref().map(|(_, name, _)| name.clone()),
            target
                .as_ref()
                .map_or(WorkspaceKind::Regular, |(_, _, kind)| *kind),
            &title,
        ));
        if let Some((row, name, _)) = target {
            self.busy = true;
            self.send(Request::Layers(row, name));
        }
    }
    /// Machine › Save as new image…: of the running or selected machine, its
    /// own form with what happens to the machine afterwards; otherwise the
    /// Save as new image… form Disk layers opens.
    pub fn begin_save_image(&mut self) {
        // Several rows selected: a command of one item takes none.
        if self.several_selected() {
            return;
        }
        if self.refused_as_newer(self.selection_needs_newer()) {
            return;
        }
        let machine = match (&self.active_machine, &self.active) {
            (Some((id, _)), _) => Some(id.clone()),
            (None, None) => self.machine.clone(),
            (None, Some(_)) => None,
        };
        if let Some(id) = machine {
            self.begin_save_machine(&id);
            return;
        }
        self.open_layers();
        if let Some(dialog) = &mut self.layers {
            dialog.begin_save(false);
        }
    }
    /// Settings… on the selected machine changes that machine; on an image it
    /// opens the sheet of a new image revision. The worker reads either, and
    /// the sheet opens read-only while it runs.
    pub fn open_settings(&mut self) {
        // Several rows selected: a command of one item takes none.
        if self.several_selected() {
            return;
        }
        if self.refused_as_newer(self.selection_needs_newer()) {
            return;
        }
        if let Some(id) = self.machine.clone() {
            self.send(Request::Machine(
                crate::library_worker::MachineJob::OpenSettings(id),
            ));
            return;
        }
        self.open_image_settings();
    }
    /// "Switch to …" in a details pane: Settings… on the selected machine or
    /// image, on the Firmware tab with that choice already made. Nothing
    /// changes until OK, or Review changes… and Publish revision.
    pub fn switch_firmware(&mut self, slot: crate::firmware_choices::Slot, id: &str) {
        self.settings_switch = Some((slot, id.to_owned()));
        self.open_settings();
    }
    /// A Settings sheet the worker read: it takes a waiting "Switch to …"
    /// and reads the store's firmware packages for its Firmware tab.
    fn settings_opened(
        &mut self,
        opened: std::result::Result<crate::image_settings::ImageSettings, String>,
    ) {
        let switch = self.settings_switch.take();
        match opened {
            Ok(mut settings) => {
                if let Some((slot, id)) = switch {
                    settings.switch_to(slot, &id);
                }
                self.image_settings = Some(settings);
                self.send(Request::FirmwarePackages);
            }
            Err(error) => self.error = Some(error),
        }
    }
    /// Image › Settings…: a new revision of the selected image, or of the
    /// selected machine's source image.
    pub fn open_image_settings(&mut self) {
        // Several rows selected: a command of one item takes none.
        if self.several_selected() {
            return;
        }
        // The selected machine's source image is the image it runs.
        if self.refused_as_newer(self.selection_needs_newer()) {
            return;
        }
        if let Some(row) = self.selected.clone() {
            self.send(Request::OpenSettings(row));
        }
    }
    /// Select the selected machine's source image in the Images collection.
    pub fn show_source_image(&mut self) {
        let Some(row) = self
            .selected_machine_row()
            .map(machines::MachineRow::image_row)
        else {
            return;
        };
        self.select_image(row);
    }
    /// Select an image in the Images collection.
    fn select_image(&mut self, row: ImageRow) {
        self.machine = None;
        self.machine_settings = None;
        self.remote.node = None;
        self.query.filter = Filter::All;
        self.query.text.clear();
        self.query.expanded.insert(row.repository().to_owned());
        self.selected = Some(row);
        self.readiness = None;
        self.send(Request::Query(self.query.clone()));
        self.recheck();
    }
    /// Select a created image in the machine view with no filter, search or
    /// collapsed group hiding it, so Start never targets a hidden machine.
    fn show_created(&mut self, row: ImageRow) {
        self.machine = None;
        self.machine_settings = None;
        self.remote.node = None;
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
        wizard.machine_names = self.machines.iter().map(|m| m.name().to_owned()).collect();
        wizard.store = self
            .root
            .as_ref()
            .and_then(|root| Store::open_existing(root).ok())
            .map(|store| store.with_compatibility(self.compatibility.clone()));
        self.new_machine = Some(wizard);
        self.send(Request::FirmwarePackages);
        if self.worker_dead {
            if let Some(wizard) = &mut self.new_machine {
                wizard.error = self.error.clone();
            }
        }
    }
    /// The library items Push to… takes: the several selected with Ctrl or
    /// Shift, else the one selected, in Images, Installation media or
    /// Firmware. Machines are never pushed, and nothing is taken while a
    /// registry's node is shown.
    pub fn push_selection(&self) -> Vec<ImageRow> {
        if self.remote.node.is_some() {
            return Vec::new();
        }
        let filter = &self.query.filter;
        let rows: Vec<&ImageRow> = if filter.is_firmware() {
            self.snapshot
                .firmware_rows
                .iter()
                .map(|r| &r.image)
                .collect()
        } else if filter.is_media() {
            self.snapshot.media_rows.iter().map(|r| &r.image).collect()
        } else if *filter == Filter::MachineFiles {
            return Vec::new();
        } else {
            self.snapshot
                .rows
                .iter()
                .filter(|r| r.group.is_none())
                .map(|r| &r.image)
                .collect()
        };
        if self.library_multi.len() > 1 {
            return self
                .library_multi
                .iter()
                .filter_map(|tag| {
                    rows.iter()
                        .find(|r| r.reference == *tag)
                        .map(|r| (*r).clone())
                })
                .collect();
        }
        let single = if filter.is_firmware() {
            self.firmware_selected.as_ref().map(|r| r.image.clone())
        } else if filter.is_media() {
            self.media_selected.as_ref().map(|r| r.image.clone())
        } else if self.machine.is_none() {
            self.selected.clone()
        } else {
            None
        };
        single.into_iter().collect()
    }

    /// Push to… has something to push: library items selected, none of
    /// them an image this release does not read.
    pub fn can_push_to(&self) -> bool {
        let selection = self.push_selection();
        !selection.is_empty()
            && selection
                .iter()
                .all(|row| row.summary.as_ref().and_then(|s| s.needs_newer()).is_none())
    }

    /// Image › Push to…, the toolbar's Push… and the Push to… buttons: one
    /// selected item opens its dialog, several open one list; the worker
    /// builds the dry runs. Refused, with nothing opened, while nothing
    /// pushable is selected.
    pub fn begin_push(&mut self, accounts: crate::registry_transfer::Accounts) {
        if self.refused_as_newer(self.target_needs_newer()) {
            return;
        }
        let selection = self.push_selection();
        if !self.can_push_to() {
            return;
        }
        let listing = self.remote_listing();
        self.push = None;
        self.push_batch = None;
        match selection.as_slice() {
            [one] => {
                let (dialog, request) = crate::registry_transfer::PushDialog::open(
                    accounts,
                    self.platform_facts.clone(),
                    Some(one.reference.clone()),
                );
                self.push = Some(dialog.with_listing(listing));
                self.send(Request::Transfer(request));
            }
            several => {
                let tags = several.iter().map(|row| row.reference.clone()).collect();
                let (batch, request) = library_model::push_batch::PushBatch::open_to(
                    accounts,
                    self.platform_facts.clone(),
                    listing,
                    tags,
                );
                self.push_batch = Some(batch);
                self.send(Request::Transfer(request));
            }
        }
    }

    /// A click on a row of Images, Installation media or Firmware, by its
    /// tag, with the modifiers held: alone it selects the row; with Ctrl
    /// (⌘ on macOS) it adds the row to the selection or takes it out; with
    /// Shift it selects the rows between the last click and this one.
    /// `current` is the row selected alone so far, `rows` the list's tags
    /// from the top. Returns the tag to select alone, or `None` while
    /// several stay selected.
    pub(super) fn pick_library(
        &mut self,
        tag: &str,
        current: Option<String>,
        modifiers: egui::Modifiers,
        rows: &[String],
    ) -> Option<String> {
        if modifiers.command {
            if self.library_multi.len() < 2 {
                self.library_multi = current.into_iter().collect();
            }
            match self.library_multi.iter().position(|t| t == tag) {
                Some(at) => {
                    self.library_multi.remove(at);
                }
                None => self.library_multi.push(tag.into()),
            }
            self.library_anchor = Some(tag.into());
        } else if let (true, Some(anchor)) = (modifiers.shift, self.library_anchor.clone()) {
            let at = |t: &str| rows.iter().position(|r| r == t);
            self.library_multi = match (at(&anchor), at(tag)) {
                (Some(a), Some(b)) => rows[a.min(b)..=a.max(b)].to_vec(),
                _ => vec![tag.into()],
            };
        } else {
            self.library_multi.clear();
            self.library_anchor = Some(tag.into());
        }
        if self.library_multi.len() < 2 {
            let alone = self.library_multi.pop();
            self.library_multi.clear();
            return Some(alone.unwrap_or_else(|| tag.into()));
        }
        None
    }

    /// Several rows of Images, Installation media or Firmware are selected
    /// with Ctrl or Shift: only Push to… takes them, and every command of a
    /// single item is disabled and refused.
    pub fn several_selected(&self) -> bool {
        self.library_multi.len() > 1 && self.remote.node.is_none()
    }

    /// The row is one of several selected.
    pub(super) fn picked(&self, tag: &str) -> bool {
        self.library_multi.len() > 1 && self.library_multi.iter().any(|t| t == tag)
    }

    /// The place Push here… of the shown Remotes node pushes to.
    pub fn push_place(&self) -> Option<library_model::push_destination::Place> {
        let node = self.remote.node.as_ref()?;
        self.remote.node_place(node)
    }

    /// Push here… applies: a registry or namespace node is shown, and its
    /// registry answers.
    pub fn can_push_here(&self) -> bool {
        self.push_place()
            .is_some_and(|place| self.remote.push_blocked(&place.host).is_none())
    }

    /// Image › Push here…, the toolbar's Push… on a Remotes node and the
    /// Push here… button of a node's pane: the library listed for `place`.
    /// Refused while its registry does not answer.
    pub fn begin_push_here(&mut self, place: library_model::push_destination::Place) {
        if self.remote.push_blocked(&place.host).is_some() {
            return;
        }
        let Some(accounts) = self.remote.model.as_ref().map(|m| m.accounts().clone()) else {
            return;
        };
        let listing = self.remote_listing();
        self.push = None;
        let (batch, request) = library_model::push_batch::PushBatch::open_here(
            accounts,
            self.platform_facts.clone(),
            listing,
            place,
        );
        self.push_batch = Some(batch);
        self.send(Request::Transfer(request));
    }

    /// What this session's Remotes listed, for the checks before a push.
    fn remote_listing(&self) -> crate::registry_transfer::Listing {
        self.remote
            .model
            .as_ref()
            .map(|m| m.listing())
            .unwrap_or_default()
    }

    /// A push's answers that reach past its dialog: a published tag shows
    /// in Remotes at once, and the library's dry runs feed a namespace's
    /// pane.
    fn on_push_event(&mut self, event: &crate::registry_transfer::TransferEvent) {
        use crate::registry_transfer::TransferEvent;
        match event {
            TransferEvent::Landed { landed, .. } => {
                if let Some(model) = &mut self.remote.model {
                    model.landed(landed);
                }
            }
            TransferEvent::PlannedAll { id, result } => {
                let mine = self
                    .plans_wanted
                    .take_if(|wanted| wanted.0 == *id)
                    .map(|(_, tags)| tags);
                let batch = self.push_batch.as_ref().is_some_and(|b| b.owns(*id));
                if let Some(tags) = mine.or_else(|| batch.then(|| self.snapshot.tags.clone())) {
                    self.push_plans = Some((tags, result.clone()));
                    self.here_cache = None;
                }
            }
            _ => {}
        }
        if let Some(batch) = &mut self.push_batch {
            let running = batch.running();
            batch.on_event(event);
            if running && !batch.running() {
                let pushed: Vec<String> = batch.landed.iter().map(ToString::to_string).collect();
                self.status = Some(if pushed.is_empty() {
                    "Nothing was pushed.".into()
                } else {
                    format!("Pushed {}", pushed.join(", "))
                });
            }
        }
    }

    /// How many library items could go to the shown node, for its pane:
    /// worked out from the library's dry runs, read once on the worker,
    /// and the registries list, read again only when the node, the
    /// registries or the dry runs change.
    fn here_counts(&mut self) -> Option<remotes::HereCounts> {
        let node = self.remote.node.clone()?;
        node.namespace.as_ref()?;
        let place = self.remote.node_place(&node)?;
        // Asked again only when the library's tags change: an answer, a
        // failure included, stands until then.
        let stale = self
            .push_plans
            .as_ref()
            .is_none_or(|(tags, _)| *tags != self.snapshot.tags);
        if stale && self.plans_wanted.is_none() && self.snapshot.read && !self.worker_dead {
            let id = crate::registry_transfer::next_id();
            self.plans_wanted = Some((id, self.snapshot.tags.clone()));
            self.send(Request::Transfer(
                crate::registry_transfer::TransferRequest::PlanAll { id },
            ));
        }
        let plans = self.push_plans.as_ref()?.1.as_ref().ok()?;
        let model = self.remote.model.as_ref()?;
        let revision = model.revision();
        if let Some((cached, at, count, counts)) = &self.here_cache {
            if *cached == place && *at == revision && *count == plans.len() {
                return Some(*counts);
            }
        }
        // Preferences are read once; the listing is this session's memory.
        let mut rules = self
            .here_rules
            .get_or_insert_with(|| model.accounts().rules(Default::default()))
            .clone();
        rules.listing = model.listing();
        let mut counts = remotes::HereCounts {
            can: 0,
            same: 0,
            total: plans.len(),
        };
        for (image, plan) in plans {
            let target = library_model::push_destination::here_destination(image, &place);
            let evaluation = rules.evaluate(plan.as_ref().ok(), None, &target.to_string());
            match evaluation.verdict {
                crate::registry_transfer::Verdict::Same => counts.same += 1,
                crate::registry_transfer::Verdict::Ready
                | crate::registry_transfer::Verdict::Login
                | crate::registry_transfer::Verdict::Move { .. } => counts.can += 1,
                _ => {}
            }
        }
        self.here_cache = Some((place, revision, plans.len(), counts));
        Some(counts)
    }

    /// Show in Remotes of a finished push: the registry's node of the tag's
    /// namespace, with the tag selected.
    pub fn show_remote(
        &mut self,
        target: &image_registry::Reference,
        selected_file: &mut Option<usize>,
    ) {
        let Some(tag) = target.tag.clone() else {
            return;
        };
        let namespace = target
            .repository
            .split_once('/')
            .map(|(first, _)| first.to_owned());
        let node = remotes::Node {
            host: target.host.clone(),
            namespace: Some(namespace.clone()),
            group: None,
        };
        self.remote.open.insert(target.host.clone());
        self.select_remote(node, selected_file);
        self.remote.selected = Some((target.host.clone(), target.repository.clone(), tag.clone()));
        if let Some(model) = &mut self.remote.model {
            if let Some(request) = model.select(&target.host, &target.repository, &tag) {
                self.send_browse(request);
            }
        }
        // A registry this session listed shows the row the push added; one
        // it did not lists now, and shows the tag once its listing has it.
        let listed = self
            .remote
            .model
            .as_ref()
            .and_then(|m| m.registry(&target.host))
            .is_some_and(|v| v.state == crate::remote_browse::State::Listed || v.stale());
        self.status = Some(if listed {
            format!(
                "Showing {target} in Remotes. The row came from the pushed manifest; no request was made."
            )
        } else {
            format!(
                "Showing {target} in Remotes: {} is listed now, as it was not in this session.",
                target.host
            )
        });
    }

    /// The kind of transfer a dialog runs now, for the question before quitting.
    pub fn running_transfer(&self) -> Option<&'static str> {
        if self.push.as_ref().is_some_and(|d| d.running())
            || self.push_batch.as_ref().is_some_and(|d| d.running())
        {
            Some("push")
        } else if self.pull.as_ref().is_some_and(|d| d.running())
            || self.remote_pull.as_ref().is_some_and(|d| d.running())
        {
            Some("pull")
        } else {
            None
        }
    }
    /// Image › Pull by reference…: a reference into the local store.
    pub fn begin_pull(&mut self, accounts: crate::registry_transfer::Accounts) {
        self.pull = Some(crate::registry_transfer::PullDialog::open(
            accounts,
            self.platform_facts.clone(),
        ));
    }
    /// Preferences closed: a registry whose Log in… opened them lists
    /// again, and an open push judges its destinations anew, as after a
    /// login there.
    pub fn preferences_closed(&mut self) {
        self.remote_preferences_closed();
        let listing = self.remote_listing();
        if let Some(dialog) = &mut self.push {
            dialog.reload(listing.clone());
        }
        if let Some(dialog) = &mut self.push_batch {
            dialog.reload(listing);
        }
        self.here_cache = None;
        self.here_rules = None;
    }
    pub fn begin_import(&mut self, path: &Path) {
        self.import = Some((path.to_string_lossy().into_owned(), String::new()));
    }
    /// Delete tag… of the selected image.
    pub fn begin_delete(&mut self) {
        // Several rows selected: a command of one item takes none.
        if self.several_selected() {
            return;
        }
        if self.machine.is_some() {
            return;
        }
        self.delete_package = None;
        if self.query.filter.is_firmware() {
            self.delete_package = self
                .firmware_selected
                .as_ref()
                .filter(|_| self.active.is_none())
                .map(|row| row.image.clone());
            self.delete = self
                .delete_package
                .as_ref()
                .map(|row| row.reference.clone());
            return;
        }
        self.delete = self
            .selected
            .as_ref()
            .filter(|_| self.active.is_none())
            .map(|row| row.reference.clone());
    }
    /// Delete tag… applies to the selected firmware package now.
    pub fn can_delete_package(&self) -> bool {
        self.query.filter.is_firmware()
            && self.firmware_selected.is_some()
            && self.active.is_none()
            && !self.busy
    }
    /// The machines whose own settings choose a ROM of a firmware package:
    /// they keep the ROM by digest, so they use the package as images do.
    fn package_machines(&self, row: &firmware::FirmwareRow) -> Vec<String> {
        let Some(f) = row.image.summary.as_ref().and_then(|s| s.firmware.as_ref()) else {
            return Vec::new();
        };
        self.machines
            .iter()
            .filter(|m| {
                m.listing.machine.settings.as_ref().is_some_and(|settings| {
                    let hashes = crate::firmware_choices::rom_hashes(settings);
                    f.roms.iter().any(|rom| hashes.contains(&rom.sha256))
                })
            })
            .map(|m| m.name().to_owned())
            .collect()
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
            || self.push_batch.is_some()
            || self.pull.is_some()
            || self.remote_pull.is_some()
            || self.remote.contents.is_some()
            || self.machine_dialog.is_some()
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
        self.push_batch = None;
        self.pull = None;
        self.remote_pull = None;
        self.remote.contents = None;
        self.machine_dialog = None;
    }
    /// Entering the Firmware or Installation media collection drops the machine
    /// selection, so Start never targets a machine that is no longer shown;
    /// Machines and Images each keep only their own kind of selection.
    fn set_filter(&mut self, filter: Filter, selected_file: &mut Option<usize>) {
        // A selection of several stays within its collection.
        if filter != self.query.filter {
            self.library_multi.clear();
        }
        let leaves_machines = self.query.filter.is_machines() != filter.is_machines();
        if filter.is_firmware() || filter.is_media() || leaves_machines {
            self.selected = None;
            self.machine = None;
            self.machine_settings = None;
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
        self.remote.node = None;
        self.query.filter = filter;
    }
    /// The machines mock-up's warning banner for unfinished gaming sessions:
    /// nothing is discarded until the user chooses.
    fn unfinished_sessions(&mut self, ui: &mut egui::Ui, selected_file: &mut Option<usize>) {
        if self.recoveries.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            ui.colored_label(ui.visuals().warn_fg_color, "⚠");
            ui.strong("Unfinished sessions");
        });
        let recoveries = self.recoveries.clone();
        for record in recoveries {
            ui.horizontal(|ui| {
                ui.label(&record.title);
                if ui.button("Keep as new machine…").clicked() {
                    self.begin_keep_copy(record.clone());
                }
                for (label, keep) in [("Keep as new image…", true), ("Discard…", false)] {
                    if ui.button(label).clicked() {
                        *selected_file = None;
                        self.machine = None;
                        self.selected = Some(record.row());
                        let mut dialog = LayersDialog::for_workspace(
                            Some(record.name.clone()),
                            record.kind,
                            &record.title,
                        );
                        if keep {
                            dialog.begin_keep();
                        } else {
                            dialog.begin_discard();
                        }
                        self.layers = Some(dialog);
                        self.busy = true;
                        self.send(Request::Layers(record.row(), record.name.clone()));
                    }
                }
            });
        }
    }
    /// The facets of a collection, under it in the tree.
    fn tree_facets(
        &mut self,
        ui: &mut egui::Ui,
        collection: &Filter,
        selected_file: &mut Option<usize>,
    ) -> bool {
        let mut changed = false;
        let firmware_mode = collection.is_firmware();
        let media_mode = collection.is_media();
        let machine_mode = collection.is_machines();
        let kinds: &[&str] = if firmware_mode {
            &firmware::FACETS
        } else if media_mode {
            &install_media::FACETS
        } else if machine_mode {
            &machines::FACETS
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
        } else if machine_mode {
            Filter::MachineFacet
        } else {
            Filter::Facet
        };
        let snapshot = self.snapshot.clone();
        let machine_facets;
        let facets = if media_mode {
            &snapshot.media_facets
        } else if machine_mode {
            machine_facets = machines::facets(&self.machines);
            &machine_facets
        } else {
            &snapshot.facets
        };
        ui.indent(("facets", machine_mode, media_mode, firmware_mode), |ui| {
            for kind in kinds {
                let picked = egui::CollapsingHeader::new(*kind)
                    .id_salt(("facet-group", media_mode, machine_mode, *kind))
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
        changed
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
        if let Some(target) = self.remote_wanted.take() {
            self.show_remote(&target, selected_file);
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
        let classic = self.classic;
        pane::backdrop(ui);
        egui::Panel::left("library-tree")
            .exact_size(230.0)
            .show_separator_line(false)
            .frame(TREE_PANE.frame())
            .show(ui, |ui| {
                TREE_PANE.paint(ui, classic);
                // A tree taller than its pane scrolls, as the mock-ups' panes do.
                egui::ScrollArea::vertical()
                    .id_salt("library-tree")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.strong("Library");
                        let machines = self.machines.clone();
                        let recent = machines
                            .iter()
                            .filter(|m| m.listing.machine.last_run.is_some())
                            .count();
                        let favourites = machines
                            .iter()
                            .filter(|m| m.listing.machine.favorite)
                            .count();
                        // The machines mock-up's one Library group, in its order;
                        // each collection's facets sit under it while it is selected.
                        for (label, filter, indent) in [
                            (format!("Machines ({})", machines.len()), Filter::Machines, false),
                            (format!("Recently played ({recent})"), Filter::Recent, true),
                            (format!("Favourites ({favourites})"), Filter::Favorites, true),
                            (format!("Images ({})", self.snapshot.total), Filter::All, false),
                            (
                                format!("Installation media ({})", self.snapshot.media_total),
                                Filter::Media,
                                false,
                            ),
                            (
                                format!("Firmware ({})", self.snapshot.firmware_total),
                                Filter::Firmware,
                                false,
                            ),
                            (format!("Machine files ({})", files.len()), Filter::MachineFiles, false),
                        ] {
                            // Collections without an icon keep the icon's room, so
                            // the names line up as in the machines mock-up's tree.
                            let mark = match tree_icon(&filter) {
                                Some(icon) => ui_kit::shell_icons::atom(icon),
                                None => ui_kit::shell_icons::blank(),
                            };
                            let current = self.remote.node.is_none() && self.query.filter == filter;
                            let picked = ui
                                .horizontal(|ui| {
                                    if indent {
                                        ui.add_space(16.0);
                                    }
                                    ui.selectable_label(current, (mark, label.as_str()))
                                        .clicked()
                                })
                                .inner;
                            if picked {
                                self.set_filter(filter.clone(), selected_file);
                                changed = true;
                            }
                            let current = &self.query.filter;
                            let here = self.remote.node.is_none() && match filter {
                                Filter::Favorites => current.is_machines(),
                                Filter::All => {
                                    matches!(current, Filter::All)
                                        || matches!(current, Filter::Facet(kind, _) if !firmware::is_facet(kind))
                                }
                                Filter::Media => current.is_media(),
                                Filter::Firmware => current.is_firmware(),
                                _ => false,
                            };
                            if here {
                                changed |= self.tree_facets(ui, &filter, selected_file);
                            }
                        }
                        self.remote_tree(ui, selected_file);
                    });
            });
        // The split between the list and the details pane drags, as in the
        // library mock-up; egui keeps the chosen height for the session. The
        // pane fills its height, so it scrolls only when its content is taller.
        let details_max = (ui.available_height() - LIST_MIN_HEIGHT).max(DETAILS_MIN_HEIGHT);
        egui::Panel::bottom("library-details")
            .resizable(true)
            .show_separator_line(false)
            .default_size(DETAILS_HEIGHT)
            .size_range(DETAILS_MIN_HEIGHT..=details_max)
            .frame(DETAILS_PANE.frame())
            .show(ui, |ui| {
                DETAILS_PANE.paint(ui, classic);
                // A pane taller than its area keeps its scroll bar in view, so
                // what lies below, the note under the actions included, reads
                // as further down rather than cut off.
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                egui::ScrollArea::vertical()
                    .id_salt("selected-image-details")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.details_pane(ui, files, selected_file, &mut commands);
                    });
            });
        let panel = egui::CentralPanel::default().frame(LIST_PANE.frame());
        panel.show(ui, |ui| {
            LIST_PANE.paint(ui, classic);
            if self.remote.node.is_some() {
                self.remote_list(ui);
                return;
            }
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
                let search = ui.text_edit_singleline(&mut self.query.text);
                changed |= search.changed();
                // The classic skin outlines no resting field, and the
                // field's own colour is the pane's ground: the mock-ups'
                // sunken field keeps it in view.
                if classic {
                    pane::inset(ui, search.rect, true);
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
            if self.query.filter.is_machines() {
                self.machine_banners(ui);
                self.unfinished_sessions(ui, selected_file);
                if let Some(warning) = &self.snapshot.warning {
                    ui.label(warning);
                }
                self.rows_built = 0;
                changed |= self.machine_list(ui, &mut commands);
                return;
            }
            self.unfinished_sessions(ui, selected_file);
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
            let widths = fit_columns(ui, &IMAGE_COLUMNS);
            let machines = self.machines.clone();
            let made = |digest: &Digest| {
                machines
                    .iter()
                    .filter(|m| m.listing.machine.image.digest == *digest)
                    .count()
            };
            let snapshot = self.snapshot.clone();
            egui::ScrollArea::horizontal()
                .id_salt("library-columns")
                .show(ui, |ui| {
                    ui.set_min_width(table_width(ui, &widths));
                    changed |= sort_header(
                        ui,
                        &IMAGE_COLUMNS.labels,
                        &widths,
                        &[IMAGE_MACHINES_COLUMN],
                        classic,
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
                                            String::new(),
                                            "Machine file".into(),
                                            String::new(),
                                            String::new(),
                                            String::new(),
                                            String::new(),
                                        ];
                                        let (clicked, double) = draw_cells(
                                            ui,
                                            &cells,
                                            &widths,
                                            &[Cell::Text; 7],
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
                                        snapshot.rows.get(index - visible_files.len())
                                    {
                                        if let Some(count) = row.group {
                                            let repo = row.image.repository().to_owned();
                                            if group_row(
                                                ui,
                                                self.query.expanded.contains(&repo),
                                                row.image.name(),
                                                &format!("{repo} · {count} images"),
                                                table_width(ui, &widths),
                                            ) {
                                                if !self.query.expanded.remove(&repo) {
                                                    self.query.expanded.insert(repo);
                                                }
                                                changed = true;
                                            }
                                        } else {
                                            let image = &row.image;
                                            let summary = image.summary.as_ref();
                                            let cells =
                                                row_cells(row, made(&image.descriptor.digest));
                                            let several = self.library_multi.len() > 1;
                                            let selected = if several {
                                                self.picked(&image.reference)
                                            } else {
                                                self.selected
                                                    .as_ref()
                                                    .is_some_and(|r| r.same(image))
                                            };
                                            // An image this release does not
                                            // read says why on its name (§5.6).
                                            let hints = [
                                                summary.and_then(|s| s.needs_newer()).or_else(
                                                    || {
                                                        crate::firmware_choices::badge_tooltip(
                                                            &image_newer(image),
                                                        )
                                                    },
                                                ),
                                                Some(source_hint(&image.descriptor)),
                                            ];
                                            let (clicked, double) = draw_cells_with_hints(
                                                ui,
                                                &cells,
                                                &widths,
                                                &IMAGE_COLUMNS.cells,
                                                selected,
                                                index % 2 == 0,
                                                image_lead(row),
                                                &hints,
                                            );
                                            let alone = if clicked || double {
                                                let modifiers = ui.input(|i| i.modifiers);
                                                let rows: Vec<String> = snapshot
                                                    .rows
                                                    .iter()
                                                    .filter(|r| r.group.is_none())
                                                    .map(|r| r.image.reference.clone())
                                                    .collect();
                                                let current = self
                                                    .selected
                                                    .as_ref()
                                                    .filter(|_| self.machine.is_none())
                                                    .map(|r| r.reference.clone());
                                                self.pick_library(
                                                    &image.reference,
                                                    current,
                                                    modifiers,
                                                    &rows,
                                                )
                                            } else {
                                                None
                                            };
                                            let image = match &alone {
                                                Some(tag) if *tag != image.reference => snapshot
                                                    .rows
                                                    .iter()
                                                    .find(|r| r.image.reference == *tag)
                                                    .map_or(image, |r| &r.image),
                                                _ => image,
                                            };
                                            let summary = image.summary.as_ref();
                                            let mine = self
                                                .selected
                                                .as_ref()
                                                .is_some_and(|r| r.same(image));
                                            if alone.is_some() && (!mine || several) {
                                                self.pending_launch = None;
                                                self.machine = None;
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
    /// The details pane of the selection: a firmware package, an
    /// installation set, a machine, an image or a machine file.
    pub(super) fn details_pane(
        &mut self,
        ui: &mut egui::Ui,
        files: &[LibraryEntry],
        selected_file: &mut Option<usize>,
        commands: &mut Vec<Command>,
    ) {
        pane::style(ui);
        if self.remote.node.is_some() {
            self.remote_details(ui);
        } else if self.library_multi.len() > 1 && !self.query.filter.is_machines() {
            self.several_details(ui, commands);
        } else if self.query.filter.is_firmware() {
            match self.firmware_selected.clone() {
                Some(row) => {
                    let machines = self.package_machines(&row);
                    let action = firmware::details(
                        ui,
                        &row,
                        self.package_details(),
                        self.can_edit_firmware(),
                        &machines,
                    );
                    self.pane_action(&row, action);
                    self.push_section(ui, &row.image, commands);
                }
                None => {
                    ui.label("Select a firmware package for details.");
                }
            }
        } else if self.query.filter.is_media() {
            match &self.media_selected {
                Some(row) => {
                    match install_media::details(ui, row, &self.running_drives, &self.machines) {
                        Some(install_media::SetAction::Insert(unit)) => {
                            commands.push(Command::InsertSet(unit))
                        }
                        Some(install_media::SetAction::InsertInto(machine, unit)) => {
                            self.picker_wanted =
                                Some((machine, unit, row.image.descriptor.digest.clone()));
                        }
                        None => (),
                    }
                    let image = row.image.clone();
                    self.push_section(ui, &image, commands);
                }
                None => {
                    ui.label("Select an installation set for details.");
                }
            }
        } else if self.query.filter.is_machines() {
            self.machine_details(ui, commands);
        } else if let Some(row) = self.selected.take() {
            self.image_details(ui, row, commands);
        } else if let Some(entry) = selected_file.and_then(|i| files.get(i)) {
            pane::title(ui, Icon::MachineFile, &entry.name, false);
            pane::facts(ui, "machine-file-facts", |ui| {
                for (name, value) in &entry.details {
                    pane::fact_text(ui, name, value.as_str());
                }
            });
            pane::text(ui, entry.ems.as_str());
            if let Some(warning) = &entry.warning {
                pane::text(
                    ui,
                    egui::RichText::new(warning).color(ui.visuals().warn_fg_color),
                );
                pane::meta(ui, entry.hint());
            }
            pane::actions(ui, |ui| {
                if ui
                    .add_enabled(
                        entry.checked && entry.warning.is_none(),
                        pane::primary(ui, "Start"),
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
        } else {
            ui.label("Select an image tag or machine file for details.");
        }
    }

    /// The pane of several items selected with Ctrl or Shift.
    fn several_details(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let rows = self.push_selection();
        let items: Vec<library_ui::push_view::Selected> = rows
            .iter()
            .map(|row| library_ui::push_view::Selected {
                image: crate::registry_transfer::LocalImage::of(&row.reference, &row.descriptor),
                source: source_cell(&row.descriptor),
                size: row.summary.as_ref().map(|s| s.stored_size),
                open: row.summary.as_ref().map(|s| image_rights(s).open),
            })
            .collect();
        match library_ui::push_view::several_items(ui, &items) {
            Some(library_ui::push_view::SeveralAction::PushTo) => commands.push(Command::Push),
            Some(library_ui::push_view::SeveralAction::Clear) => self.library_multi.clear(),
            None => {}
        }
    }

    /// The Push to… part of a set's or package's pane: where it came from
    /// and went, where Push to… sends it, the registries it is on, and the
    /// Push to… button.
    fn push_section(&mut self, ui: &mut egui::Ui, row: &ImageRow, commands: &mut Vec<Command>) {
        let image = crate::registry_transfer::LocalImage::of(&row.reference, &row.descriptor);
        pane::facts(ui, "push-facts", |ui| {
            library_ui::push_view::item_facts(ui, &image, &row.descriptor);
        });
        let listing = self.remote_listing();
        if let Some(target) =
            library_ui::push_view::on_registries(ui, &image, &row.descriptor, &listing)
        {
            self.remote_wanted = Some(target);
        }
        let reads = row.summary.as_ref().and_then(|s| s.needs_newer()).is_none();
        pane::actions(ui, |ui| {
            if ui
                .add_enabled(reads, egui::Button::new("Push to…"))
                .clicked()
            {
                commands.push(Command::Push);
            }
        });
    }

    /// Carry out what a selected image's pane asked for.
    fn image_pane_action(
        &mut self,
        row: &ImageRow,
        action: ImageAction,
        commands: &mut Vec<Command>,
    ) {
        let favourite = row.summary.as_ref().is_some_and(|s| s.favorite);
        let labels = || {
            row.summary
                .as_ref()
                .map(|s| s.labels.clone())
                .unwrap_or_default()
        };
        match action {
            ImageAction::Command(command) => commands.push(command),
            ImageAction::Favourite(favourite) => self.send(Request::Edit(
                row.descriptor.digest.clone(),
                favourite,
                labels(),
            )),
            ImageAction::Labels => {
                let labels = self
                    .labels
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                self.send(Request::Edit(
                    row.descriptor.digest.clone(),
                    favourite,
                    labels,
                ));
            }
            ImageAction::Open(id) => {
                if let Some(image) = self.machine_row(&id).map(machines::MachineRow::image_row) {
                    self.show_machine(id, image);
                }
            }
            ImageAction::Switch(slot, id) => self.switch_firmware(slot, id),
            ImageAction::ShowImage(image) => self.select_image(*image),
        }
    }

    /// A newer tag of the image's repository, listed now, that already binds
    /// `by`: the hint links to it, so new machines can start from it.
    /// The hint names the image by its newest version tag, and links to the
    /// image of the newest version among several. The search covers every
    /// image of the store, so a closed group, a search or a facet never hides
    /// the link.
    fn newer_tag(&self, image: &ImageRow, by: &assembly::FirmwareEntry) -> Option<ImageRow> {
        let best = |row: &ImageRow| {
            std::iter::once(&row.reference)
                .chain(&row.aliases)
                .filter_map(|reference| {
                    let tag = reference.rsplit_once(':')?.1;
                    Some((pc_image::template::precedence(tag), reference.clone()))
                })
                .max()
        };
        self.snapshot
            .replacements
            .iter()
            .filter(|r| {
                r.repository() == image.repository()
                    && r.descriptor.digest != image.descriptor.digest
                    && r.summary.as_ref().is_some_and(|s| {
                        crate::firmware_choices::in_summary(s)
                            .iter()
                            .any(|(_, e)| e.id == by.id)
                    })
            })
            .filter_map(|r| Some((best(r)?, r)))
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|((_, reference), row)| ImageRow {
                reference,
                ..row.clone()
            })
    }

    /// The details pane of a selected image, as the machines mock-up draws
    /// it: the kind tile beside the title, tags, digest and size and the
    /// description, then hardware, drives and the image's facts, in the first
    /// column; its firmware and the machines made from it beside them from
    /// the top of the pane; then the actions in one row and the note under
    /// them. `row` is the selection, taken for the frame
    /// and put back before the pane's actions run.
    fn image_details(&mut self, ui: &mut egui::Ui, row: ImageRow, commands: &mut Vec<Command>) {
        let name_hint = self.default_machine_name(&row);
        let summary = row.summary.as_ref();
        let rights = summary.map(image_rights);
        let ready = self.ready();
        let digest = &row.descriptor.digest;
        // The pane reads these while it edits the labels field; they go back
        // before anything the pane asks for runs.
        let readiness = self.readiness.take();
        let facts = self.facts.take();
        let checked = self.checked.take();
        let image_facts = facts
            .as_ref()
            .filter(|(id, _)| id == digest && matches!(readiness, Some(Ok(_))))
            .map(|(_, facts)| facts);
        let checked_image = checked
            .as_ref()
            .filter(|(id, _)| id == digest)
            .map(|(_, checked)| checked.as_ref());
        let base = checked_image.and_then(|c| c.base.as_ref()).map(|base| {
            self.snapshot
                .tags
                .iter()
                .find(|(_, d)| d == base)
                .map_or_else(
                    || format!("sha256:{}", pane::short_digest(base.hex())),
                    |(tag, _)| tag.clone(),
                )
        });
        let made: Vec<(String, String, String)> = self
            .machines_of(digest)
            .into_iter()
            .map(|m| {
                (
                    m.id().to_owned(),
                    m.name().to_owned(),
                    format!("— {} · last run {}", m.listing.state, m.last_run()),
                )
            })
            .collect();
        let mut actions = Vec::new();
        let mut open = None;
        let tile = pane::Tile {
            title: row.name(),
            kind: "IMAGE",
            user_supplied: rights.as_ref().is_some_and(|r| !r.open),
        };
        // The newer tag each superseded firmware's hint links to, read before
        // the pane borrows the labels field. The hints sit under the head;
        // what they ask for runs first among the pane's actions.
        let newer: Vec<_> = image_newer(&row)
            .into_iter()
            .map(|newer| {
                let tag = self.newer_tag(&row, newer.by);
                (newer, tag)
            })
            .collect();
        let mut hinted = Vec::new();
        let local_image = crate::registry_transfer::LocalImage::of(&row.reference, &row.descriptor);
        let listing = self.remote_listing();
        let mut shown_remote = None;
        let labels = &mut self.labels;
        pane::split(
            ui,
            0.55,
            |ui| {
                pane::head(ui, &tile, |ui| {
                    pane::title(
                        ui,
                        Icon::Image,
                        row.name(),
                        summary.is_some_and(|s| s.favorite),
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.label(pane::mono(row.references()));
                        ui.label(pane::mono(format!(
                            "sha256:{}",
                            pane::short_digest(digest.hex())
                        )));
                        if let Some(summary) = summary {
                            ui.label(egui::RichText::new(human_size(summary.stored_size)).weak());
                        }
                    });
                    if let Some(description) = checked_image.and_then(|c| c.description.as_deref())
                    {
                        pane::text(ui, description);
                    }
                    match &readiness {
                        Some(Ok(_)) => {
                            pane::meta(ui, "Ready · firmware verified");
                        }
                        Some(Err(reason)) => {
                            pane::text(
                                ui,
                                egui::RichText::new(reason).color(ui.visuals().warn_fg_color),
                            );
                        }
                        None => {
                            pane::meta(ui, "Checking image…");
                        }
                    }
                });
                for (newer, tag) in newer {
                    let hint = pane::newer_hint(
                        ui,
                        &newer,
                        "Switching publishes a new image revision under a new tag; this image and its machines stay as they are.",
                        tag.as_ref().map(|t| t.reference.as_str()),
                    );
                    if hint.switch {
                        hinted.push(ImageAction::Switch(newer.slot, newer.by.id));
                    }
                    if let (true, Some(tag)) = (hint.newer_tag, tag) {
                        hinted.push(ImageAction::ShowImage(Box::new(tag)));
                    }
                }
            },
            |ui| {
                pane::facts(ui, "image-facts", |ui| {
                    pane::fact(ui, "Hardware", |ui| match image_facts {
                        Some(facts) => {
                            pane::text(ui, facts.hardware.as_str());
                        }
                        None => {
                            pane::meta(ui, "Checking image…");
                        }
                    });
                    pane::fact(ui, "Drives", |ui| match image_facts {
                        Some(facts) => {
                            for drive in &facts.drives {
                                ui.horizontal_wrapped(|ui| {
                                    ui.strong(&drive.drive);
                                    ui.add(egui::Label::new(&drive.described).wrap());
                                });
                            }
                        }
                        None => {
                            pane::meta(ui, "Checking image…");
                        }
                    });
                    if let Some(title) = summary.and_then(|s| s.titles.first()) {
                        for (label, value) in [
                            ("Platform", title.platform.clone().unwrap_or_default()),
                            ("Genre", title.genres.join(", ")),
                            ("Developer", title.developers.join(", ")),
                            ("Publisher", title.publishers.join(", ")),
                            (
                                "Year",
                                title.year.map(|y| y.to_string()).unwrap_or_default(),
                            ),
                            ("Series", title.series.clone().unwrap_or_default()),
                        ] {
                            if !value.is_empty() {
                                pane::fact_text(ui, label, value);
                            }
                        }
                    }
                    if let Some(summary) = summary {
                        if !summary.titles.is_empty() {
                            let titles: Vec<&str> =
                                summary.titles.iter().map(|t| t.title.as_str()).collect();
                            pane::fact_text(ui, "Titles", titles.join(", "));
                        }
                    }
                    pane::fact(ui, "Base image", |ui| match &base {
                        Some(base) => {
                            pane::text(ui, pane::mono(base.as_str()));
                        }
                        None => {
                            pane::text(ui, "—");
                        }
                    });
                    if let Some(Ok(details)) = &readiness {
                        pane::fact_text(
                            ui,
                            "Storage",
                            format!(
                                "{} only in this image · {} shared with other images",
                                human_size(details.exclusive_bytes),
                                human_size(details.dedup_bytes)
                            ),
                        );
                    }
                    if let Some(rights) = &rights {
                        pane::fact(ui, "Redistribution", |ui| {
                            ui.horizontal_wrapped(|ui| pane::rights(ui, rights.open));
                        });
                    }
                    library_ui::push_view::item_facts(ui, &local_image, &row.descriptor);
                    if let Some(summary) = summary {
                        pane::fact(ui, "Labels", |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let mut favourite = summary.favorite;
                                if ui.checkbox(&mut favourite, "Favourite").changed() {
                                    actions.push(ImageAction::Favourite(favourite));
                                }
                                // A fixed id keeps the field's focus while the
                                // pane's columns stack or part as the window
                                // is resized.
                                ui.add(
                                    egui::TextEdit::singleline(labels)
                                        .id(egui::Id::unique("image-labels"))
                                        .hint_text("labels, comma separated")
                                        .desired_width(ui.available_width().clamp(80.0, 180.0)),
                                );
                                if ui.button("Apply labels").clicked() {
                                    actions.push(ImageAction::Labels);
                                }
                            });
                        });
                    }
                });
            },
            |ui| {
                pane::firmware_section(ui, image_facts, None);
                if let Some(target) = library_ui::push_view::on_registries(
                    ui,
                    &local_image,
                    &row.descriptor,
                    &listing,
                ) {
                    shown_remote = Some(target);
                }
                pane::section(ui, format!("Machines from this image ({})", made.len()));
                if made.is_empty() {
                    pane::meta(
                        ui,
                        format!(
                            "None yet. Start creates one named \u{201c}{name_hint}\u{201d} and starts it; New machine from image… asks for the name first."
                        ),
                    );
                }
                for (id, name, state) in &made {
                    ui.horizontal_wrapped(|ui| {
                        if ui.link(name).clicked() {
                            open = Some(ImageAction::Open(id.clone()));
                        }
                        ui.label(egui::RichText::new(state).weak());
                    });
                }
                pane::meta(
                    ui,
                    "Machines keep this image by its digest, also after its tags move or are deleted.",
                );
            },
        );
        actions.splice(0..0, hinted);
        actions.extend(open);
        let count = made.len();
        // An image this release does not read keeps only what needs no
        // config: its details and Delete tag… (§5.6).
        let needs = summary.and_then(|s| s.needs_newer());
        let reads = needs.is_none();
        pane::actions(ui, |ui| {
            for (label, enabled, command, hover) in [
                (
                    if count > 0 { "Start…" } else { "Start" },
                    ready,
                    Command::Start,
                    "",
                ),
                ("New machine from image…", reads, Command::NewFromImage, ""),
                ("Gaming mode", ready, Command::GamingMode, ""),
                (
                    "Settings…",
                    reads,
                    Command::Settings,
                    "Changes create a new image revision; disks are shared.",
                ),
                ("Layers…", reads, Command::Layers, ""),
                ("Push to…", reads, Command::Push, ""),
                ("Delete tag…", true, Command::DeleteTag, ""),
            ] {
                let button = if command == Command::Start {
                    pane::primary(ui, label)
                } else {
                    egui::Button::new(label)
                };
                let mut response = ui.add_enabled(enabled, button);
                if let (false, Some(reason)) = (enabled, &needs) {
                    response = response.on_disabled_hover_text(reason);
                }
                if !hover.is_empty() {
                    response = response.on_hover_text(hover);
                }
                if response.clicked() {
                    actions.push(ImageAction::Command(command));
                }
            }
        });
        pane::meta(
            ui,
            "Settings… on an image publishes a new image revision; machines from this image keep running this one. To change one machine only, use Settings… on that machine.",
        );
        self.readiness = readiness;
        self.facts = facts;
        self.checked = checked;
        self.selected = Some(row);
        if let Some(target) = shown_remote {
            self.remote_wanted = Some(target);
        }
        if actions.is_empty() {
            return;
        }
        if let Some(row) = self.selected.clone() {
            for action in actions {
                self.image_pane_action(&row, action, commands);
            }
        }
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
            settings.read_only = match settings.machine_id() {
                Some(id) => self
                    .active_machine
                    .as_ref()
                    .is_some_and(|(active, _)| active == id),
                None => self.active.as_ref().is_some_and(|(active, _, _)| {
                    active.descriptor.digest == settings.row.descriptor.digest
                }),
            };
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
                    // The Firmware library's import opens over the sheet; the
                    // sheet rereads the packages once it imported.
                    crate::settings_sheet::ImageAction::ImportRoms => {
                        self.firmware_import_wanted = true
                    }
                    crate::settings_sheet::ImageAction::Apply(close) => {
                        match settings.apply_request(close) {
                            Some((id, table)) => {
                                self.busy = true;
                                requests.push(Request::Machine(
                                    crate::library_worker::MachineJob::ApplySettings(
                                        id,
                                        Box::new(table),
                                    ),
                                ));
                            }
                            None if close && !settings.unsaved() => self.image_settings = None,
                            None => (),
                        }
                    }
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
                if classic { ui_kit::skin::gradient_title(ui, "Import image"); } else { ui.heading("Import image"); } ui.label("Machine file"); ui.text_edit_singleline(path); ui.label("New repository:tag"); ui.text_edit_singleline(tag);
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
            let target = self
                .delete_package
                .clone()
                .or_else(|| self.selected.clone());
            let package_users = self
                .firmware_selected
                .clone()
                .filter(|_| self.delete_package.is_some())
                .map(|row| (row.used_by.clone(), self.package_machines(&row)));
            egui::Modal::new(egui::Id::unique("delete-tag")).show(ctx, |ui| {
                if classic {
                    ui_kit::skin::gradient_title(ui, "Delete tag?");
                } else {
                    ui.heading("Delete tag?");
                }
                if let Some(row) = &target {
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
                    if let Some((images, machines)) = &package_users {
                        for name in machines {
                            ui.add(
                                egui::Label::new(format!(
                                    "The machine \u{201c}{name}\u{201d} chooses this firmware in its own settings. It keeps the ROM by digest, so it still starts, saves and resets without this tag."
                                ))
                                .wrap(),
                            );
                        }
                        if !images.is_empty() {
                            ui.add(
                                egui::Label::new(format!(
                                    "Images that embed these ROMs keep them: {}.",
                                    images.join(", ")
                                ))
                                .wrap(),
                            );
                        }
                    }
                    let machines = self.machines_of(&row.descriptor.digest).len();
                    if machines > 0 {
                        ui.add(
                            egui::Label::new(format!(
                                "Its {machines} machine{} the image by its digest. With no tag left it shows as detached and stays in the store for them.",
                                if machines == 1 { " keeps" } else { "s keep" }
                            ))
                            .wrap(),
                        );
                    }
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
            if close {
                self.delete_package = None;
            }
        }
        if self.layers.is_none() {
            if let Some(record) = self
                .decision
                .clone()
                .filter(|_| self.machine_dialog.is_none())
            {
                let mut keep = false;
                let mut keep_machine = false;
                let mut discard = false;
                egui::Modal::new(egui::Id::unique("gaming-stop")).show(ctx,|ui| {
                    ui.heading(format!("Gaming mode ended — {}", record.title));
                    ui.label("Discard removes the copy. Keep as new machine… gives it a name in Machines. Keep as new image… freezes it into a new image, then removes the copy.");
                    ui.horizontal(|ui| {
                        discard=ui.add_enabled(!self.busy,egui::Button::new("Discard")).clicked();
                        keep_machine=ui.add_enabled(!self.busy,egui::Button::new("Keep as new machine…")).clicked();
                        keep=ui.add_enabled(!self.busy,egui::Button::new("Keep as new image…")).clicked();
                    });
                });
                if keep_machine {
                    self.begin_keep_copy(record.clone());
                }
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
        self.machine_modals(ctx, classic, &mut requests);
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
        use crate::registry_transfer_view::{
            show_pull, show_push, show_push_batch, show_remote_pull, Action,
        };
        let mut shown = None;
        if let Some(dialog) = &mut self.push {
            match show_push(ctx, dialog, classic) {
                Action::Send(request) => requests.push(Request::Transfer(request)),
                Action::SendAll(list) => requests.extend(list.into_iter().map(Request::Transfer)),
                Action::Close => self.push = None,
                Action::LogIn(host) => self.login_wanted = Some(host),
                Action::ShowInRemotes(target) => {
                    self.push = None;
                    shown = Some(target);
                }
                Action::None | Action::ShowPulled | Action::NewMachine => (),
            }
        }
        if let Some(dialog) = &mut self.push_batch {
            match show_push_batch(ctx, dialog, classic) {
                Action::Send(request) => requests.push(Request::Transfer(request)),
                Action::SendAll(list) => requests.extend(list.into_iter().map(Request::Transfer)),
                Action::Close => self.push_batch = None,
                Action::LogIn(host) => self.login_wanted = Some(host),
                Action::ShowInRemotes(target) => {
                    self.push_batch = None;
                    shown = Some(target);
                }
                Action::None | Action::ShowPulled | Action::NewMachine => (),
            }
        }
        if let Some(target) = shown {
            self.remote_wanted = Some(target);
        }
        if let Some(dialog) = &mut self.pull {
            match show_pull(ctx, dialog, classic) {
                Action::Send(request) => requests.push(Request::Transfer(request)),
                Action::SendAll(list) => requests.extend(list.into_iter().map(Request::Transfer)),
                Action::Close => self.pull = None,
                Action::None
                | Action::ShowPulled
                | Action::NewMachine
                | Action::LogIn(_)
                | Action::ShowInRemotes(_) => (),
            }
        }
        if let Some(dialog) = &mut self.remote_pull {
            match show_remote_pull(ctx, dialog, classic) {
                Action::Send(request) => requests.push(Request::Transfer(request)),
                Action::SendAll(list) => requests.extend(list.into_iter().map(Request::Transfer)),
                Action::Close => self.remote_pull = None,
                Action::ShowPulled => {
                    if let Some(dialog) = self.remote_pull.take() {
                        let item = &dialog.items[0];
                        if let Some(digest) = item.source.digest.clone() {
                            let image = Descriptor::new(
                                image_store::media_type::MANIFEST,
                                digest,
                                item.manifest_size,
                            );
                            self.show_in_library(&item.local_tag, &item.artifact_type, image);
                        }
                    }
                }
                Action::NewMachine => {
                    if let Some((tag, image, name)) = self
                        .remote_pull
                        .take()
                        .and_then(|dialog| dialog.created_image())
                    {
                        self.new_machine_from(&tag, image, &name);
                    }
                }
                Action::None | Action::LogIn(_) | Action::ShowInRemotes(_) => (),
            }
        }
        self.remote_contents(ctx, classic);
    }
    /// Apply worker events until `done` holds; tests only, the frame loop polls.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pump_until(&mut self, mut done: impl FnMut(&Self) -> bool) {
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn pump_queued(&mut self) {
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

    #[test]
    fn regression_new_machine_store_keeps_library_compatibility() {
        use image_store::compat::{AppVersion as V, Compatibility};
        let temp = machine_file::test_support::TempDir::new("wizard-compatibility");
        let store = Store::open(temp.path().join("store")).unwrap();
        let runtime = Compatibility::new(V::new(0, 0, 9), V::new(0, 0, 1)).with_core(
            "ibm-pc",
            V::new(0, 0, 7),
            V::new(0, 0, 1),
        );
        let mut view = LibraryView::new(None).with_compatibility(runtime.clone());
        view.root = Some(store.root().to_path_buf());
        view.begin_new_machine();
        let reopened = view.new_machine.as_ref().unwrap().store.as_ref().unwrap();
        assert_eq!(reopened.levels("ibm-pc"), runtime.levels("ibm-pc"));
        assert!(image_store::BlobSource::compatibility_configured(reopened));
    }
    use image_store::summary::{ArtifactKind, SummaryCache};
    use image_store::{
        media_type as mt,
        metadata::{Metadata, Title},
        BlobSource, ConfigValidator, Manifest,
    };
    use std::collections::BTreeMap;

    pub(super) struct Provider;
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
    ) -> (machine_file::test_support::TempDir, Store, Vec<Descriptor>) {
        let temp = machine_file::test_support::TempDir::new("desktop-library");
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
            "Get templates… lists the ready-made open machines of the default registry under Remotes, to pull: open firmware with its source and a blank disk, no operating system.",
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
    fn firmware_and_media_only() -> (machine_file::test_support::TempDir, Store) {
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
        let config = image_store::firmware::Config::decode(format!("schema=1\nplatform='ibm-pc'\nname='Synthetic firmware'\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=32768\nsha256='{}'\nblob='{}'\n", digest.hex(), digest).as_bytes()).unwrap();
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
        library.query.filter = Filter::All;
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
            filter: Filter::Facet("Labels".into(), "collection".into()),
            ..Default::default()
        };
        let result = snapshot(&cache, &query);
        assert_eq!(result.matched, 2);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].group, Some(2));
        query.expanded.insert("synthetic-0".into());
        assert_eq!(snapshot(&cache, &query).rows.len(), 3);
        // Recently played and Favourites list machines, never image rows.
        for filter in [Filter::Recent, Filter::Favorites] {
            query.filter = filter;
            assert_eq!(snapshot(&cache, &query).matched, 0);
        }
        query.filter = Filter::All;
        query.sort = 2;
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
        let config = image_store::firmware::Config::decode(format!("schema=1\nplatform='ibm-pc'\nname='Synthetic firmware'\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=32768\nsha256='{}'\nblob='{}'\n", digest.hex(), digest).as_bytes()).unwrap();
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
        // Tags an earlier release could write, beside the import's revision.
        tag_again(&store, "private/firmware/ibm-pc/synthetic:latest", &package);
        tag_again(&store, "private/firmware/ibm-pc/synthetic:1.1.0", &package);
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
            .map(|row| (row.group, row.child, row_cells(row, 0)[2].clone()))
            .collect();
        // The header counts the repository's two images, one per manifest,
        // over its three tags.
        assert_eq!(
            rows,
            [
                (Some(2), false, String::from("dos-lab:1.0.0")),
                (None, true, String::from("dos-lab:1.0.0")),
                (None, true, String::from("dos-lab:2.0.0, latest")),
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
        assert_eq!(package.reference, "private/firmware/ibm-pc/synthetic:1.1.0");
        assert_eq!(package.tags(), "1.1.0, r1, latest");

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
            install_media::cells(&media.media_rows[0], &[])[2],
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
        // Too narrow for the package's three columns side by side, so the
        // pane at its first height is shorter than the package.
        let size = egui::vec2(1000.0, 1000.0);
        let mut time = 0.0;
        let mut draw = |view: &mut LibraryView, events: Vec<egui::Event>| {
            time += 0.05;
            let output = frame(&ctx, size, time, events, |ui| {
                view.show(ui, &[], &mut selected_file);
            });
            texts(&output)
        };
        // The pane's second line, under the package name, and its last facts line.
        let top =
            "System BIOS · local package · private/firmware/ibm-pc/synthetic-two-chip-bios:r1";
        let last = "Edit metadata… creates a new package revision; ROM blobs unchanged. A machine \
                    embeds its ROM blobs and records the source package digest. Source folders \
                    are not needed after import.";
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
            default - margin - 300.0,
        );
        let mut shown = draw(&mut view, vec![]);
        let taller = y(&shown, top).expect("the pane still shows the package");
        assert!(
            (default - taller - 300.0).abs() < 2.0,
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
        library.query.filter = Filter::All;
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
            .import(&b"schema=99\nplatform='ibm-pc'\n[machine]\n"[..], None)
            .unwrap();
        let metadata = Metadata::new("ibm-pc").encode(&[]).unwrap();
        let metadata = store.import(metadata.as_slice(), None).unwrap();
        let manifest = Manifest::new(
            "ibm-pc",
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
        let temp = machine_file::test_support::TempDir::new("desktop-launch-transfer");
        let path = temp.path().join("disk.img");
        std::fs::write(&path, [0u8; 512]).unwrap();
        let make = || pc_image::workspace::PreparedSession {
            launch: None,
            machine: assembly::bios(vec![0xf4; 32768], None),
            image: Digest::of(b"synthetic image"),
            bases: BTreeMap::from([(
                path.clone(),
                journal_files::VerifiedBase::open(&path).unwrap(),
            )]),
            zero: BTreeMap::new(),
        };
        let prepared = make();
        let before = journal_files::data_passes(512);
        let transfer = prepared.into_transfer().unwrap();
        assert_eq!(journal_files::data_passes(512), before);
        assert!(std::thread::spawn(move || {
            let before = journal_files::data_passes(512);
            let ready = transfer.restore().is_ok();
            assert_eq!(journal_files::data_passes(512), before);
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
        use journal_files::SectorSource;
        let temp = machine_file::test_support::TempDir::new("verified-inode-swap");
        let dir = temp.path().join("current");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("base.img");
        std::fs::write(&path, [7; 512]).unwrap();
        let base = journal_files::VerifiedBase::open(&path).unwrap();
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
            launch: None,
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
        let temp = machine_file::test_support::TempDir::new("bad-library-root");
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
        let config = image_store::firmware::Config::decode(format!("schema=1\nplatform='ibm-pc'\nname='Synthetic firmware'\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=32768\nsha256='{}'\nblob='{}'\n", digest.hex(), digest).as_bytes()).unwrap();
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
        let worker = Worker::spawn(firmware.store.root().to_owned(), std::sync::Arc::new(|| {}));
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
        wizard.set_title("Open AT workbench".into());
        wizard.go(crate::new_machine::SUMMARY);
        assert!(wizard.private_notice().is_none());
        let (image, reference, expected) = wizard.request().unwrap();
        assert_eq!(
            (reference.as_str(), &expected),
            ("open-at-workbench:1.0.0", &None)
        );
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
        let launch =
            pc_image::launch::from_session(prepared, &unemu_pc_core::PcCore, "Test".into())
                .unwrap();
        let mut pc = core_launch::start_with(&launch.prepared, |request| {
            unemu_pc_core::PcCore.create_machine(request)
        })
        .unwrap();
        let machine = pc.assembled();
        let checkpoints = device::ByteCapture::new(4096);
        let post_index = machine.device_index[&assembly::DeviceId::Post];
        machine
            .machine
            .bus_mut()
            .device_mut(post_index)
            .unwrap()
            .byte_output()
            .unwrap()
            .bind_sink(Box::new(checkpoints.clone()));
        let mut post = Vec::new();
        for step in 1..=60u64 {
            machine
                .run_session(&assembly::RunPolicy::natural(Some(step * 1_000_000), None))
                .unwrap();
            post.extend(checkpoints.drain().unwrap());
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
        let worker = Worker::spawn(firmware.store.root().to_owned(), std::sync::Arc::new(|| {}));
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
                "schema=1\nplatform='ibm-pc'\nname='Synthetic two-chip BIOS'\n[fits]\nboards=['ibm-5170']\nmachine_classes=['at']\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=65536\nsha256='{}'\n[[roms.parts]]\nblob='{e}'\nsha256='{}'\nsize=32768\nposition='U27'\nlane='even'\n[[roms.parts]]\nblob='{o}'\nsha256='{}'\nsize=32768\nposition='U47'\nlane='odd'\n",
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
            pc_image::workspace::prepare_start(
                &firmware.store.workspace("inspect-0").unwrap(),
                "chips:1.0.0",
                false,
            )
            .map(|p| p.machine)
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
        let worker = Worker::spawn(firmware.store.root().to_owned(), std::sync::Arc::new(|| {}));
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
            let Event::Layers(name, details, source, checked, summary) =
                worker_events(&worker, &|e| matches!(e, Event::Layers(..)))
            else {
                unreachable!()
            };
            // What Disk layers names the drives and ROMs with.
            let checked = checked.expect("the image's configuration");
            assert!(checked.draft.is_some());
            let facts = hardware::ImageFacts::new(
                details.as_ref().unwrap(),
                Some(&checked),
                summary.as_deref(),
                &BTreeMap::new(),
            );
            assert!(!facts.firmware.is_empty(), "{facts:?}");
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
        // open-at is the official template's name, so the suggestion is a
        // local revision; a version of your own choice is taken as typed.
        assert_eq!(review.save.reference(), "open-at:1.0.0-local.1");
        review.save.set_tag("1.1.0".into());
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
        assert_eq!(review.save.reference(), "open-at:1.0.0-local.1");
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

    /// A revision never moves a tag, `latest` included; a new tag another
    /// program takes before Publish reaches the store is refused, and the
    /// review asks again with the store's tags.
    #[test]
    fn a_revision_never_moves_a_tag_and_a_tag_taken_meanwhile_is_asked_again() {
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
        assert_eq!(review.save.check().level, crate::save_image::TagLevel::Bad);
        assert!(review.save.submit().is_none());
        assert!(
            review.save.confirm_move().is_none(),
            "no Move tag? question"
        );
        review.save.set_tag("1.1.0".into());
        let target = review.save.submit().unwrap();
        assert_eq!(target.expected, None);
        let plan = review.plan.clone().unwrap().unwrap();
        // Another program takes open-at:1.1.0 before Publish reaches the store.
        let other = pc_image::create::create(
            &firmware.store,
            &crate::image_settings::tests::open_new_image(&firmware, 2048),
            "open-at:1.1.0",
            None,
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
            Some(crate::save_image::Prompt::Refuse(other.digest.clone())),
            "the refusal names the image the tag names now"
        );
        // A tag that is no version, taken meanwhile, is refused the same way.
        let settings = view.image_settings.as_mut().unwrap();
        let review = settings.review.as_mut().unwrap();
        review.save.dismiss();
        review.save.set_tag("workbench".into());
        let target = review.save.submit().unwrap();
        let plan = review.plan.clone().unwrap().unwrap();
        let taken = pc_image::create::create(
            &firmware.store,
            &crate::image_settings::tests::open_new_image(&firmware, 3072),
            "open-at:workbench",
            None,
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
        assert_eq!(review.save.check().level, crate::save_image::TagLevel::Bad);
        assert!(!matches!(
            review.save.prompt,
            Some(crate::save_image::Prompt::Move(_))
        ));
        for (tag, digest) in [
            ("open-at:1.1.0", &other.digest),
            ("open-at:latest", &row.descriptor.digest),
            ("open-at:workbench", &taken.digest),
        ] {
            assert_eq!(
                &firmware.store.tag_descriptor(tag).unwrap().digest,
                digest,
                "{tag} never moved"
            );
        }
    }

    /// A machine whose own settings choose a package's ROM is one of the
    /// package's users: its pane lists it, and Delete tag… names it.
    #[test]
    fn a_machine_choosing_a_package_rom_is_named_in_used_by_and_delete() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(true) else {
            return;
        };
        let cache = firmware::tests::warm(&firmware.store);
        let query = Query {
            filter: Filter::Firmware,
            ..Query::default()
        };
        let snap = snapshot(&cache, &query);
        let kiselev = crate::firmware_choices::entry("kiselev-xi8088-1.0.2").unwrap();
        let package = snap
            .firmware_rows
            .iter()
            .find(|r| {
                r.image
                    .summary
                    .as_ref()
                    .and_then(|s| s.firmware.as_ref())
                    .is_some_and(|f| f.roms.iter().any(|rom| rom.sha256 == kiselev.sha256))
            })
            .unwrap()
            .clone();
        let mut machine = machines::tests::row("chooser", "Chooser", "unused", &["open-at:1.0.0"]);
        machine.listing.machine.settings = Some(
            toml::from_str(&format!(
                "[firmware.bios]\nimage='assets/firmware-0.bin'\nsha256='{}'\n",
                kiselev.sha256
            ))
            .unwrap(),
        );
        let mut view = LibraryView::new(None);
        view.set_machines(vec![machine]);
        view.query = query;
        view.snapshot = Arc::new(snap);
        view.firmware_selected = Some(package.clone());
        assert_eq!(view.package_machines(&package), ["Chooser"]);
        let ctx = egui::Context::default();
        let draw = |view: &mut LibraryView, time: f64| {
            let output =
                firmware::tests::frame(&ctx, egui::vec2(1400.0, 1200.0), time, vec![], |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let mut commands = Vec::new();
                        view.details_pane(ui, &[], &mut None, &mut commands);
                    });
                    let _ = view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
                });
            firmware::tests::texts(&output)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>()
        };
        let shown = draw(&mut view, 0.0);
        assert!(
            shown
                .iter()
                .any(|t| t == "Chooser — its own firmware choice"),
            "{shown:?}"
        );
        assert!(view.can_delete_package());
        view.begin_delete();
        assert_eq!(
            view.delete.as_deref(),
            Some(package.image.reference.as_str())
        );
        let mut shown = Vec::new();
        for frame in 1..20 {
            shown = draw(&mut view, f64::from(frame) * 0.05);
        }
        assert!(
            shown.iter().any(|t| t.starts_with(
                "The machine \u{201c}Chooser\u{201d} chooses this firmware in its own settings."
            )),
            "{shown:?}"
        );
        assert!(shown
            .iter()
            .any(|t| t == &format!("Deletes {}", package.image.reference)));
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
        // The desktop session made before machines existed is a machine now,
        // and its unfinished save came with it.
        until_view(&mut view, |view| !view.machines.is_empty());
        let machine = view.machines[0].clone();
        assert!(machine.listing.publication_pending);
        assert_eq!(machine.image_row().descriptor.digest, row.descriptor.digest);
        let name = machine.id().to_owned();
        view.show_machine(name.clone(), machine.image_row());
        // Save as new image… offers to finish it as it was chosen.
        view.begin_save_image();
        assert!(matches!(
            view.machine_dialog,
            Some(machines::Dialog::Finish { .. })
        ));
        view.machine_dialog = None;
        // So does Disk layers' save form, whose save of a machine finishes it.
        view.open_layers();
        if let Some(dialog) = &mut view.layers {
            dialog.begin_save(false);
        }
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
    pub(super) fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
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
                let visuals = ui_kit::skin::skin_visuals(classic, dark);
                let fill = visuals.selection.bg_fill;
                let ctx = egui::Context::default();
                ctx.set_visuals(visuals);
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    draw_cells(
                        ui,
                        &cells,
                        &[80.0; 9],
                        &[Cell::Text; 9],
                        true,
                        false,
                        Lead::default(),
                    );
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
        ui_kit::skin::configure_context(&ctx);
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
        assert_eq!(tree_icon(&Filter::Machines), Some(Icon::Machine));
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
        assert_eq!(row_cells(&row, 0)[0], "games:1");
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
            draw_cells(
                ui,
                &cells,
                &[200.0; 9],
                &[Cell::Text; 9],
                false,
                false,
                lead,
            );
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
        assert_eq!(sections[2].format.color, ui_kit::shell_icons::STAR_GOLD);
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
            draw_cells(ui, &cells, &[200.0; 9], &[Cell::Text; 9], true, false, lead);
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
