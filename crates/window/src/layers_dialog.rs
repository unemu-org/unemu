// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whole-machine disk-layer dialog; machine-images §§4 and 7. It follows the
//! approved main-window mock-up's Disk layers dialog
//! (`backlog/docs/design/mockups/main-window.html`, `#dlg-layers`): one
//! sunken tree in the code font holds each fixed disk's chain from its base
//! to the working changes, which take the selection highlight, then the
//! floppy catalogues, the firmware and the settings; under it the save
//! notice, and one footer row.
//!
//! Drives go by the letters the status bar's chips give them: A: and B: for
//! the diskette units, C: and D: for the IDE master and slave. Media go by
//! name with their write modes, firmware by the names and open or
//! user-supplied marks the image's details pane gives it, layers by kind
//! with a short digest. The check's controller ids, medium ids and
//! configuration keys stay inside.
//!
//! Publication and discard are requested only after an explicit review step.
//! Save as new image… (Keep as new image… for Gaming mode) opens the one
//! Save as new image… form (`backlog/docs/design/mockups/save-as-image.html`):
//! a saved layer always becomes a new image with a name and a tag. Disk
//! layers' earlier Save as layer… and New image from here… opened that same
//! form, the second with the option to flatten disk chains; the one button
//! offers that option, unchecked.

use crate::{
    library_view::{
        hardware::{self, FirmwareFact},
        human_size, pane,
    },
    library_worker::{CheckedImage, WorkspaceKind},
    media_picker::format_name,
    save_image::{self, SaveAsImage, TagLevel, Target},
};
use assembly::{DriveKind, FloppyGeometry};
use egui::{text::LayoutJob, RichText, TextFormat};
use image_store::Digest;
use pc_image::inspection::{ImageDetails, MediumDetails};

/// The mock-up's wide dialog.
const WIDTH: f32 = 680.0;
/// The mock-up's tree text size (`.tree`).
const TREE_SIZE: f32 = 11.5;
/// Room between tree lines, so a line takes the mock-up's 1.6 line height.
const TREE_GAP: f32 = 5.0;
/// Room between the tree's outline and its text: the mock-up's 8 px.
const TREE_PADDING: i8 = 8;
/// The tree scrolls beyond this height.
const TREE_HEIGHT: f32 = 340.0;
/// The mock-up's save notice.
const NOTICE: &str = "Saving a layer stops the machine first. The image keeps its settings, firmware and every drive together; a disk on its own is not an image.";

/// How a part of a tree line reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Body,
    /// The quiet storage line.
    Muted,
    /// The working changes: the mock-up's highlighted current line (`.cur`).
    Current,
}

/// One line of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    /// A drive's heading or a member of its chain, in parts.
    Chain(Vec<(String, Tone)>),
    /// The blank line between the chains and the rest.
    Gap,
    /// A label and its value; a long value wraps beside the label column.
    Fact(String, Vec<(String, Tone)>),
}

/// A drive of the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drive {
    Floppy {
        unit: u8,
        format: Option<FloppyGeometry>,
    },
    Fixed {
        unit: u8,
        bytes: Option<u64>,
    },
}

impl Drive {
    /// Position among A:, B:, C: and D:, as the status bar's chips count.
    fn slot(self) -> u8 {
        match self {
            Self::Floppy { unit, .. } => unit,
            Self::Fixed { unit, .. } => unit.saturating_add(2),
        }
    }

    /// The drive by letter and type: `A: 1.2M`, `C: IDE master (20.4 MiB)`.
    /// A unit past Z: has no letter and goes by its number.
    fn title(self) -> String {
        let (first, unit) = match self {
            Self::Floppy { unit, .. } => (b'A', unit),
            Self::Fixed { unit, .. } => (b'C', unit),
        };
        let letter = hardware::letter(first, unit).unwrap_or_else(|| format!("unit {unit}"));
        match self {
            Self::Floppy { format, .. } => match format {
                Some(format) => format!("{letter} {}", format_name(format)),
                None => letter,
            },
            Self::Fixed { unit, bytes } => {
                let role = crate::drive_chips::role(unit);
                match bytes {
                    Some(bytes) => format!("{letter} IDE {role} ({})", human_size(bytes)),
                    None => format!("{letter} IDE {role}"),
                }
            }
        }
    }
}

/// A drive's catalogue as the check lists it,
/// `card:unit catalogue: id, id · inserted: id`, where `empty` stands for no
/// medium in the drive. Medium ids are lowercase letters, digits and dashes,
/// so the separators never occur inside one; but `empty` is itself a valid
/// id, and the check's line is the only public account of the slot. When a
/// catalogue holds a medium with that id, the line cannot tell whether it is
/// in the drive, and no medium is marked rather than one guessed.
struct Slot<'a> {
    key: &'a str,
    catalogue: Vec<&'a str>,
    inserted: Option<&'a str>,
}

fn slots(details: &ImageDetails) -> Vec<Slot<'_>> {
    details
        .slots
        .iter()
        .filter_map(|line| {
            let (key, rest) = line.split_once(" catalogue: ")?;
            let (catalogue, inserted) = rest.rsplit_once("· inserted: ")?;
            let catalogue: Vec<&str> = catalogue
                .trim()
                .split(", ")
                .filter(|id| !id.is_empty())
                .collect();
            let inserted = Some(inserted).filter(|id| *id != "empty" && catalogue.contains(id));
            Some(Slot {
                key,
                catalogue,
                inserted,
            })
        })
        .collect()
}

/// The image's drives by the key the check names each with (`card:unit`):
/// from the draft, with each drive's type and size; any other key by its
/// card's kind, as the library's panes letter them: an IDE card's units
/// from C:, every other card's from A:. A card without an id goes by its
/// kind.
fn drives(details: &ImageDetails, checked: Option<&CheckedImage>) -> Vec<(String, Drive)> {
    let mut drives = Vec::new();
    if let Some(draft) = checked.and_then(|c| c.draft.as_ref()) {
        for drive in &draft.config.drives {
            let key = format!(
                "{}:{}",
                hardware::controller_name(draft, drive.controller),
                drive.unit
            );
            let drive = match drive.kind {
                DriveKind::Floppy { geometry } => Drive::Floppy {
                    unit: drive.unit,
                    format: Some(geometry),
                },
                DriveKind::HardDisk { geometry } => Drive::Fixed {
                    unit: drive.unit,
                    bytes: Some(geometry.image_bytes()),
                },
            };
            drives.push((key, drive));
        }
    }
    let cards = checked.map_or(&[][..], |c| c.cards.as_slice());
    let keys = slots(details)
        .into_iter()
        .map(|slot| slot.key)
        .chain(details.media.iter().map(|m| m.drive.as_str()));
    for key in keys {
        if drives.iter().any(|(known, _)| known == key) {
            continue;
        }
        let Some((card, unit)) = key.rsplit_once(':') else {
            continue;
        };
        let Ok(unit) = unit.parse::<u8>() else {
            continue;
        };
        let ide = match cards.iter().find(|(id, _)| id == card) {
            Some((_, kind)) => kind == "ide",
            None => card == "ide",
        };
        let drive = if ide {
            Drive::Fixed { unit, bytes: None }
        } else {
            Drive::Floppy { unit, format: None }
        };
        drives.push((key.to_owned(), drive));
    }
    drives
}

/// A medium's write mode in the words of the status bar's chips.
fn mode(policy: pc_image::Policy) -> &'static str {
    match policy {
        pc_image::Policy::Readonly => "read-only",
        pc_image::Policy::Journal => "journal",
        pc_image::Policy::Volatile => "volatile",
    }
}

/// A medium's own name, if it has one other than its id.
fn own_name(medium: &MediumDetails) -> Option<&str> {
    Some(medium.name.as_str()).filter(|name| !name.is_empty() && *name != medium.id)
}

/// A medium's name, or its place among its drive's media, as the library's
/// panes name it.
fn medium_name(details: &ImageDetails, medium: &MediumDetails) -> String {
    let index = details
        .media
        .iter()
        .filter(|m| m.drive == medium.drive)
        .position(|m| m.id == medium.id)
        .unwrap_or(0);
    hardware::medium_name(index, medium)
}

/// The mock-up's short digest: the first and the last four hex digits.
fn short(hex: &str) -> String {
    match (hex.get(..4), hex.get(hex.len().saturating_sub(4)..)) {
        (Some(head), Some(tail)) if hex.len() > 8 => format!("{head}…{tail}"),
        _ => hex.to_owned(),
    }
}

fn sectors(count: usize) -> String {
    if count == 1 {
        "1 sector".into()
    } else {
        format!("{count} sectors")
    }
}

/// A medium's chain, from its drive's heading through its base and layers
/// to the working changes. The digests line up in one column, and the
/// working changes' state under them.
fn chain(
    lines: &mut Vec<Line>,
    details: &ImageDetails,
    medium: &MediumDetails,
    drive: Option<Drive>,
) {
    let head = match (drive, own_name(medium)) {
        (Some(drive), Some(name)) => format!("{} — {name}", drive.title()),
        (Some(drive), None) => drive.title(),
        (None, _) => medium_name(details, medium),
    };
    lines.push(Line::Chain(vec![(
        format!("{head} · {}", mode(medium.policy)),
        Tone::Body,
    )]));
    let indent = |depth: usize| format!(" {}└─ ", "    ".repeat(depth));
    let members: Vec<(String, String, String, String)> = medium
        .chain
        .iter()
        .enumerate()
        .map(|(depth, member)| {
            let kind = if depth == 0 { "base" } else { "layer" };
            let (label, size, mut tail) = match member.blank_bytes {
                Some(bytes) => (
                    "  blank",
                    human_size(bytes),
                    format!(" ({} stored)", human_size(member.size)),
                ),
                None => ("", human_size(member.size), String::new()),
            };
            if member.shared_by > 1 {
                tail.push_str(&format!("  shared by {} images", member.shared_by));
            }
            (
                format!("{}{kind}{label}", indent(depth)),
                short(member.digest.hex()),
                size,
                tail,
            )
        })
        .collect();
    let working = indent(medium.chain.len());
    let current = "working changes";
    let column = members
        .iter()
        .map(|(head, ..)| head.chars().count())
        .chain([working.chars().count() + current.len()])
        .max()
        .unwrap_or(0);
    let size_column = members
        .iter()
        .map(|(_, _, size, _)| size.chars().count())
        .max()
        .unwrap_or(0);
    for (head, digest, size, tail) in members {
        lines.push(Line::Chain(vec![(
            format!("{head:<column$}  {digest}  {size:>size_column$}{tail}"),
            Tone::Body,
        )]));
    }
    let state = match medium.working_sectors {
        Some(0) => "no changes".to_owned(),
        Some(count) => format!("{}   not saved", sectors(count)),
        None => "size unknown   not saved".to_owned(),
    };
    let current_column = column - working.chars().count();
    lines.push(Line::Chain(vec![
        (working, Tone::Body),
        (
            format!("{current:<current_column$}  {state}"),
            Tone::Current,
        ),
    ]));
}

/// A floppy drive's catalogue line: each medium by name with its write
/// mode, the one in the drive marked, and unsaved changes highlighted.
fn catalogue(details: &ImageDetails, slots: &[Slot<'_>], key: &str, drive: Drive) -> Line {
    let slot = slots.iter().find(|slot| slot.key == key);
    let media: Vec<&MediumDetails> = match slot {
        Some(slot) => slot
            .catalogue
            .iter()
            .filter_map(|id| details.media.iter().find(|m| m.id == *id))
            .collect(),
        None => details.media.iter().filter(|m| m.drive == key).collect(),
    };
    let mut value = Vec::new();
    for (index, medium) in media.iter().enumerate() {
        if index > 0 {
            value.push((" · ".to_owned(), Tone::Body));
        }
        let inserted = slot.and_then(|slot| slot.inserted) == Some(medium.id.as_str());
        value.push((
            format!(
                "{} ({}{})",
                medium_name(details, medium),
                mode(medium.policy),
                if inserted { ", in the drive" } else { "" }
            ),
            Tone::Body,
        ));
        if let Some(count) = medium.working_sectors.filter(|count| *count > 0) {
            value.push((" ".to_owned(), Tone::Body));
            value.push((format!("{} not saved", sectors(count)), Tone::Current));
        }
    }
    if value.is_empty() {
        value.push(("empty".to_owned(), Tone::Body));
    }
    Line::Fact(format!("{} catalogue", drive.title()), value)
}

/// The firmware line: each ROM by the name the details pane gives it (the
/// catalogue's, a library package's, else the start of its digest), with the
/// open or user-supplied mark, once for all when they share it.
fn firmware(roms: &[FirmwareFact]) -> Line {
    let mark = |open: bool| if open { "open" } else { "user-supplied" };
    let shared = roms
        .first()
        .and_then(|rom| rom.open)
        .filter(|first| roms.iter().all(|rom| rom.open == Some(*first)));
    let value = match shared {
        _ if roms.is_empty() => "none".to_owned(),
        Some(open) => format!(
            "{}  ({}{})",
            roms.iter()
                .map(|rom| rom.name.as_str())
                .collect::<Vec<_>>()
                .join(" · "),
            mark(open),
            if open { "" } else { ", by hash" }
        ),
        None => roms
            .iter()
            .map(|rom| match rom.open {
                Some(open) => format!("{} ({})", rom.name, mark(open)),
                None => rom.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(" · "),
    };
    Line::Fact("Firmware".into(), vec![(value, Tone::Body)])
}

/// What the tree shows: the chains of the fixed disks by letter and of
/// every medium in no floppy drive (CMOS among them), then the floppy
/// catalogues by letter, the firmware, the settings and, quietly, what the
/// image takes in the store.
fn tree(
    details: &ImageDetails,
    checked: Option<&CheckedImage>,
    roms: &[FirmwareFact],
    source: &SourceImage,
) -> Vec<Line> {
    let drives = drives(details, checked);
    let slots = slots(details);
    let drive_of = |key: &str| {
        drives
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, d)| *d)
    };
    let mut lines = Vec::new();
    let mut chained: Vec<(u8, &MediumDetails, Option<Drive>)> = details
        .media
        .iter()
        .filter_map(|medium| match drive_of(&medium.drive) {
            Some(Drive::Floppy { .. }) => None,
            Some(drive) => Some((drive.slot(), medium, Some(drive))),
            None => Some((u8::MAX, medium, None)),
        })
        .collect();
    chained.sort_by_key(|(slot, ..)| *slot);
    for (_, medium, drive) in chained {
        chain(&mut lines, details, medium, drive);
    }
    if !lines.is_empty() {
        lines.push(Line::Gap);
    }
    let mut floppies: Vec<(&str, Drive)> = drives
        .iter()
        .filter(|(_, drive)| matches!(drive, Drive::Floppy { .. }))
        .map(|(key, drive)| (key.as_str(), *drive))
        .collect();
    floppies.sort_by_key(|(_, drive)| drive.slot());
    for (key, drive) in floppies {
        lines.push(catalogue(details, &slots, key, drive));
    }
    lines.push(firmware(roms));
    lines.push(Line::Fact(
        "Settings".into(),
        vec![(
            if source.reference.is_empty() {
                "from the pinned image".to_owned()
            } else {
                format!("from {}", source.reference)
            },
            Tone::Body,
        )],
    ));
    lines.push(Line::Fact(
        "Storage".into(),
        vec![(
            format!(
                "stored {} · exclusive {} · dedup savings {}",
                human_size(details.stored_bytes),
                human_size(details.exclusive_bytes),
                human_size(details.dedup_bytes)
            ),
            Tone::Muted,
        )],
    ));
    lines
}

/// Draws the tree in its sunken box, in the code font: the chains line by
/// line, the facts with their labels in one column.
fn tree_box(ui: &mut egui::Ui, lines: &[Line], classic: bool) {
    let font = egui::FontId::monospace(TREE_SIZE);
    let visuals = ui.visuals().clone();
    let format = |tone: Tone| {
        let mut format = TextFormat::simple(font.clone(), visuals.text_color());
        match tone {
            Tone::Body => (),
            Tone::Muted => format.color = visuals.weak_text_color(),
            Tone::Current => {
                format.color = visuals.selection.stroke.color;
                format.background = visuals.selection.bg_fill;
            }
        }
        format
    };
    let job = |parts: &[(String, Tone)]| {
        let mut job = LayoutJob::default();
        for (text, tone) in parts {
            job.append(text, 0.0, format(*tone));
        }
        job
    };
    // The facts' labels share one column, two characters past the widest.
    let column = lines
        .iter()
        .filter_map(|line| match line {
            Line::Fact(label, _) => Some(label.chars().count()),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 2;
    pane::sunken(ui, classic, TREE_PADDING, |ui| {
        ui.set_width(ui.available_width());
        // The tree takes its whole height at once, up to the most it shows:
        // left to the room the modal measured the frame before, it would
        // grow over several frames when the media arrive.
        egui::ScrollArea::vertical()
            .max_height(TREE_HEIGHT)
            .min_scrolled_height(TREE_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, TREE_GAP);
                for line in lines {
                    match line {
                        Line::Chain(parts) => {
                            ui.add(egui::Label::new(job(parts)).wrap());
                        }
                        Line::Gap => {
                            ui.add_space(TREE_SIZE);
                        }
                        Line::Fact(label, value) => {
                            let tone = value.first().map_or(Tone::Body, |(_, tone)| *tone);
                            let label = format!("{label:<column$}");
                            ui.horizontal_top(|ui| {
                                ui.add(egui::Label::new(job(&[(label, tone)])).extend());
                                ui.add(egui::Label::new(job(value)).wrap());
                            });
                        }
                    }
                }
            });
    });
}

/// The mock-up's notice: its warning colour on its warning ground
/// (`.notice`), with the skin's corners.
fn notice(ui: &mut egui::Ui, text: &str, classic: bool) {
    let ground = pane::warn_ground(ui.visuals());
    egui::Frame::new()
        .fill(ground)
        .corner_radius(if classic { 0 } else { 6 })
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let colour = ui.visuals().warn_fg_color;
            ui.add(egui::Label::new(RichText::new(text).color(colour)).wrap());
        });
}

pub(super) enum Action {
    None,
    Close,
    Import,
    Publish(String, Target),
    Discard(String),
}
/// The image the workspace's session pins: its reference, digest and title,
/// read by the library worker.
#[derive(Clone, Debug, Default)]
pub(super) struct SourceImage {
    pub reference: String,
    pub digest: Option<Digest>,
    pub title: Option<String>,
    /// Every local tag, for the tag rules.
    pub tags: Vec<(String, Digest)>,
    /// An unfinished save of this workspace: its tag and expected digest.
    pub pending: Option<(String, Option<Digest>)>,
}
pub(super) struct LayersDialog {
    workspace: Option<String>,
    kind: WorkspaceKind,
    title: String,
    details: Option<ImageDetails>,
    /// What the check read of the image's configuration: its drives.
    checked: Option<CheckedImage>,
    /// The image's ROMs as the details pane names and marks them.
    firmware: Vec<FirmwareFact>,
    discard: bool,
    source: SourceImage,
    /// The Save as new image… form while it is open.
    pub save: Option<SaveAsImage>,
    /// Open the form once the worker has read the source image.
    save_wanted: Option<bool>,
    pub error: Option<String>,
}
impl LayersDialog {
    pub fn new(workspace: Option<String>) -> Self {
        Self {
            workspace,
            kind: WorkspaceKind::Regular,
            title: "Disk layers — Machine".into(),
            details: None,
            checked: None,
            firmware: Vec::new(),
            discard: false,
            source: SourceImage::default(),
            save: None,
            save_wanted: None,
            error: None,
        }
    }
    pub fn for_workspace(workspace: Option<String>, kind: WorkspaceKind, name: &str) -> Self {
        let mut dialog = Self::new(workspace);
        dialog.kind = kind;
        dialog.title = format!("Disk layers — {name}");
        dialog
    }
    pub fn set_machine_name(&mut self, name: &str) {
        self.title = format!("Disk layers — {name}");
    }
    pub fn set_source(&mut self, source: SourceImage) {
        self.source = source;
        if let Some(from_here) = self.save_wanted.take() {
            self.begin_save(from_here);
        }
    }
    /// Refresh the form's tags after the store refused a save and ask again.
    pub fn refused(&mut self, error: String, tags: Vec<(String, Digest)>) {
        self.error = Some(error);
        self.source.tags = tags.clone();
        if let Some(form) = &mut self.save {
            form.refresh(tags);
        }
    }
    /// Open the Save as new image… form; `from_here` offers flattening. Before
    /// the source image is known, the form opens when it arrives.
    pub fn begin_save(&mut self, from_here: bool) {
        let Some(digest) = self.source.digest.clone() else {
            self.save_wanted = Some(from_here);
            return;
        };
        let form = SaveAsImage::new(&self.source.reference, digest, self.source.tags.clone())
            .with_title(self.source.title.as_deref());
        self.save = Some(if from_here { form.with_flatten() } else { form });
        self.discard = false;
    }
    pub fn begin_keep(&mut self) {
        self.begin_save(false);
    }
    pub fn begin_discard(&mut self) {
        self.discard = true;
        self.save = None;
    }
    fn affected(&self) -> Vec<&pc_image::inspection::MediumDetails> {
        self.details
            .as_ref()
            .into_iter()
            .flat_map(|d| &d.media)
            .filter(|m| m.policy != pc_image::Policy::Readonly && m.working_sectors != Some(0))
            .collect()
    }
    pub fn set_workspace(&mut self, name: String) {
        self.workspace = Some(name);
    }
    pub fn set_details(&mut self, details: Result<ImageDetails, String>) {
        match details {
            Ok(details) => self.details = Some(details),
            Err(e) => self.error = Some(e),
        }
    }
    /// What names the image's drives and ROMs: the check's reading of its
    /// configuration, and its ROMs as the details pane names them.
    pub fn set_image(&mut self, checked: Option<CheckedImage>, firmware: Vec<FirmwareFact>) {
        self.checked = checked;
        self.firmware = firmware;
    }
    pub fn completed(&mut self) {
        self.discard = false;
        self.save = None;
        self.save_wanted = None;
        self.error = None;
    }
    /// Finish an interrupted save with its own tag and expected digest, as a
    /// repeated publication does; nothing else can run until it finishes.
    fn finish_pending(&self) -> Option<Action> {
        let (reference, expected) = self.source.pending.clone()?;
        Some(Action::Publish(
            self.workspace.clone()?,
            Target {
                reference,
                expected,
                title: None,
                flatten: false,
            },
        ))
    }
    fn pending_notice(&self, ui: &mut egui::Ui, busy: bool) -> Option<Action> {
        let (tag, _) = self.source.pending.as_ref()?;
        ui.add(egui::Label::new(format!("An earlier save to {tag} did not finish. The machine cannot start until it does; finishing keeps every change it was saving.")).wrap());
        ui.add_enabled(!busy, egui::Button::new(format!("Finish saving {tag}")))
            .clicked()
            .then(|| self.finish_pending())
            .flatten()
    }
    /// Save: the form's tag rules decide; the media must be inspected first.
    fn publication(&mut self) -> Option<Action> {
        self.details.as_ref()?;
        let name = self.workspace.clone()?;
        let target = self.save.as_mut()?.submit()?;
        Some(Action::Publish(name, target))
    }
    fn discard(&self) -> Option<Action> {
        (self.discard && (self.details.is_some() || self.error.is_some()))
            .then(|| {
                self.workspace
                    .as_ref()
                    .map(|name| Action::Discard(name.clone()))
            })
            .flatten()
    }
    /// An affected medium in the discard question: its drive by letter and
    /// type, and its name.
    fn medium_title(&self, medium: &MediumDetails) -> String {
        let Some(details) = &self.details else {
            return hardware::medium_name(0, medium);
        };
        let name = medium_name(details, medium);
        match drives(details, self.checked.as_ref())
            .into_iter()
            .find(|(key, _)| *key == medium.drive)
        {
            Some((_, drive)) => format!("{} — {name}", drive.title()),
            None => name,
        }
    }
    /// The mock-up's footer: Save as new image… (the default), Discard
    /// changes and Close in one row at the right, in that order for the eye
    /// and for Tab. The row is only as tall as its buttons: a modal lays out
    /// inside the size it measured the frame before, so a row taking all the
    /// height on offer would carry that height over. The buttons' width comes
    /// from the pass before; when it changes, egui lays the frame out again.
    fn footer(&mut self, ui: &mut egui::Ui, busy: bool, action: &mut Action) {
        let id = egui::Id::unique("disk-layers-footer-width");
        let known = ui.data(|data| data.get_temp::<f32>(id));
        ui.horizontal(|ui| {
            // Unmeasured, the buttons start at the left, so they never push
            // the dialog wider while egui measures it.
            ui.add_space(known.map_or(0.0, |width| (ui.available_width() - width).max(0.0)));
            let mut buttons = Vec::new();
            if self.workspace.is_some() {
                let ready = !busy && self.details.is_some();
                let disposable = self.kind.disposable();
                let label = if disposable {
                    "Keep as new image…"
                } else {
                    "Save as new image…"
                };
                let save = pane::primary(ui, label);
                let save = ui.add_enabled(ready && self.source.digest.is_some(), save);
                if save.clicked() {
                    // Flattening stays an option of the form, unchecked;
                    // Gaming mode's keep never offered it.
                    self.begin_save(!disposable);
                }
                let discard = ui.add_enabled(ready, egui::Button::new("Discard changes"));
                if discard.clicked() {
                    self.begin_discard();
                }
                buttons.extend([save.rect, discard.rect]);
            }
            let close = ui.button("Close");
            if close.clicked() {
                *action = Action::Close;
            }
            buttons.push(close.rect);
            let width = buttons
                .iter()
                .copied()
                .reduce(|a, b| a.union(b))
                .map_or(0.0, |rect| rect.width());
            if known.is_none_or(|known| (known - width).abs() > 0.5) {
                ui.data_mut(|data| data.insert_temp(id, width));
                ui.ctx()
                    .request_discard("the Disk layers footer measured its buttons");
            }
        });
    }
    pub fn show(&mut self, ctx: &egui::Context, busy: bool, classic: bool) -> Action {
        if self.save.is_some() {
            return self.show_save(ctx, busy, classic);
        }
        let mut action = Action::None;
        egui::Modal::new(egui::Id::unique("disk-layers")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            if classic {
                ui_kit::skin::gradient_title(ui, &self.title);
            } else {
                ui.heading(&self.title);
            }
            ui.add_space(6.0);
            if self.workspace.is_none() {
                ui.label("Disk layers need a machine image. Import a machine file to keep settings, firmware and every drive together.");
                if ui.button("File > Import image…").clicked() {
                    action = Action::Import;
                }
            } else {
                let lines = match &self.details {
                    Some(details) => tree(
                        details,
                        self.checked.as_ref(),
                        &self.firmware,
                        &self.source,
                    ),
                    None => vec![Line::Chain(vec![("Loading chains…".into(), Tone::Body)])],
                };
                tree_box(ui, &lines, classic);
                ui.add_space(8.0);
                notice(ui, NOTICE, classic);
                if let Some(a) = self.pending_notice(ui, busy) {
                    action = a;
                }
                if self.discard {
                    ui.add_space(6.0);
                    ui.strong("Discard changes to the affected media?");
                    if self.details.is_none() {
                        ui.label("Media inspection failed. This discards every writable medium and CMOS in this workspace.");
                    }
                    for medium in self.affected() {
                        ui.label(self.medium_title(medium));
                    }
                    ui.label("The workspace's CMOS, settings and all media changes are removed. Immutable bases remain.");
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, egui::Button::new("Confirm discard")).clicked() {
                            if let Some(a) = self.discard() {
                                action = a;
                            }
                        }
                        if ui.button("Cancel action").clicked() {
                            self.discard = false;
                        }
                    });
                }
            }
            if let Some(e) = &self.error {
                ui.add(egui::Label::new(e).wrap());
            }
            ui.add_space(8.0);
            self.footer(ui, busy, &mut action);
        });
        action
    }

    /// The Save as new image… dialog: name and tag, what is saved, the base
    /// image, and the redistribution note for new disk layers.
    fn show_save(&mut self, ctx: &egui::Context, busy: bool, classic: bool) -> Action {
        let mut action = Action::None;
        let mut cancel = false;
        let mut save = false;
        let workspace = self.workspace.clone().unwrap_or_default();
        let changed: Vec<_> = self
            .affected()
            .into_iter()
            .map(|medium| (self.medium_title(medium), medium.clone()))
            .collect();
        let loaded = self.details.is_some();
        let pending = self.source.pending.as_ref().map(|(tag, _)| tag.clone());
        let mut finish = false;
        let Some(form) = &mut self.save else {
            return action;
        };
        egui::Modal::new(egui::Id::unique("save-as-image")).show(ctx, |ui| {
            ui.set_width(640.0);
            if classic {
                ui_kit::skin::gradient_title(ui, "Save as new image");
            } else {
                ui.heading("Save as new image");
            }
            ui.label(format!(
                "From {} {} · workspace {workspace}",
                form.source,
                save_image::short(&form.source_digest)
            ));
            ui.label("Saving stops the machine first: every drive finishes its writes, then the changes are frozen into the new image.");
            if let Some(tag) = &pending {
                ui.add(egui::Label::new(format!("An earlier save to {tag} did not finish. The machine cannot start until it does; finishing keeps every change it was saving.")).wrap());
                finish = ui
                    .add_enabled(!busy, egui::Button::new(format!("Finish saving {tag}")))
                    .clicked();
            }
            ui.separator();
            ui.strong("Name");
            save_image::form(ui, form);
            ui.separator();
            ui.strong("What is saved");
            if !loaded {
                ui.label("Loading chains…");
            }
            for (title, medium) in &changed {
                let layered = medium.chain.iter().any(|m| m.blank_bytes.is_some()) || medium.chain.len() > 1;
                ui.label(format!(
                    "{title} — {} — {}",
                    medium.working_sectors.map_or_else(
                        || "changes of unknown size".into(),
                        |n| format!("{n} sectors written")
                    ),
                    if layered { "a new layer on its chain" } else { "frozen into a new blob" }
                ));
            }
            if loaded && changed.is_empty() {
                ui.label("No drive has unsaved changes: every disk and medium is shared.");
            }
            ui.label("Hardware settings unchanged; firmware and unchanged media are shared with the source image.");
            ui.separator();
            ui.strong("Base image (FROM chain)");
            ui.monospace(format!(
                "image  {}  {}\n        └─ {}  this save",
                form.source,
                save_image::short(&form.source_digest),
                form.reference()
            ));
            ui.add(egui::Label::new(egui::RichText::new(format!("The new image records {} as its base for provenance. Running it never needs the base image or its tag.", form.source)).small()).wrap());
            if !changed.is_empty() {
                ui.add(egui::Label::new("New disk layers count as user-supplied until you mark them open, so this image can be pushed only to a private registry. Its base stays as it is.").wrap());
            }
            ui.add(egui::Label::new(egui::RichText::new("After saving, this workspace runs the new image with fresh, empty journals.").small()).wrap());
            if let Some(e) = &self.error {
                ui.add(egui::Label::new(e).wrap());
            }
            ui.separator();
            ui.horizontal(|ui| {
                let check = form.check();
                let ready = !busy && loaded && (check.level != TagLevel::Bad || check.taken.is_some());
                if ui.add_enabled(ready, egui::Button::new("Save")).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if let Some(target) = save_image::prompts(ctx, form, classic) {
            if let Some(name) = self.workspace.clone() {
                action = Action::Publish(name, target);
            }
        } else if form.prompt.is_none() && ctx.input_mut(ui_kit::skin::escape_pressed) {
            cancel = true;
        }
        if save {
            if let Some(a) = self.publication() {
                action = a;
            }
        }
        if finish {
            if let Some(a) = self.finish_pending() {
                action = a;
            }
        }
        if cancel {
            self.save = None;
        }
        action
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn details() -> ImageDetails {
        ImageDetails {
            media: vec![],
            slots: vec![],
            firmware: vec![],
            hardware: vec![],
            stored_bytes: 0,
            exclusive_bytes: 0,
            dedup_bytes: 0,
        }
    }
    pub(crate) fn source(reference: &str, digest: &Digest) -> SourceImage {
        SourceImage {
            reference: reference.into(),
            digest: Some(digest.clone()),
            title: Some("Example machine".into()),
            tags: vec![(reference.into(), digest.clone())],
            pending: None,
        }
    }
    #[test]
    fn save_as_layer_and_new_image_from_here_open_the_same_form() {
        let mut dialog = LayersDialog::new(Some("gaming-synthetic".into()));
        dialog.set_source(source("example:1.0.0", &Digest::of(b"example")));
        dialog.begin_save(false);
        assert!(
            dialog.publication().is_none(),
            "media are not inspected yet"
        );
        dialog.set_details(Ok(details()));
        let form = dialog.save.as_mut().unwrap();
        assert_eq!(form.reference(), "example:1.1.0");
        assert_eq!(form.title.as_deref(), Some("Example machine"));
        assert_eq!(form.flatten, None);
        form.set_tag("saved".into());
        assert!(matches!(
            dialog.publication(),
            Some(Action::Publish(name, Target { reference, expected: None, title: None, flatten: false }))
                if name == "gaming-synthetic" && reference == "example:saved"
        ));
        dialog.begin_save(true);
        let form = dialog.save.as_mut().unwrap();
        form.flatten = Some(true);
        form.title = Some("Renamed".into());
        assert!(matches!(
            dialog.publication(),
            Some(Action::Publish(_, Target { flatten: true, title: Some(title), .. })) if title == "Renamed"
        ));
        // A taken version tag is refused, never published.
        dialog.save.as_mut().unwrap().set_tag("1.0.0".into());
        assert!(dialog.publication().is_none());
        assert!(matches!(
            dialog.save.as_ref().unwrap().prompt,
            Some(save_image::Prompt::Refuse(_))
        ));
        dialog.completed();
        assert!(dialog.save.is_none());
        assert!(dialog.publication().is_none());
    }
    #[test]
    fn the_form_waits_for_its_source_and_offers_to_finish_a_pending_save() {
        let mut dialog = LayersDialog::new(Some("regular-1".into()));
        dialog.begin_keep();
        assert!(dialog.save.is_none(), "the source is not read yet");
        let mut source = source("example:1.0.0", &Digest::of(b"example"));
        source.pending = Some(("example:saved".into(), Some(Digest::of(b"old"))));
        dialog.set_source(source);
        assert!(
            dialog.save.is_some(),
            "the form opens once the source arrives"
        );
        assert!(matches!(
            dialog.finish_pending(),
            Some(Action::Publish(name, Target { reference, expected: Some(d), title: None, flatten: false }))
                if name == "regular-1" && reference == "example:saved" && d == Digest::of(b"old")
        ));
    }
    #[test]
    fn discard_requires_details_and_explicit_affected_media_confirmation() {
        let mut dialog = LayersDialog::new(Some("gaming-synthetic".into()));
        assert!(dialog.discard().is_none());
        dialog.set_details(Ok(details()));
        assert!(dialog.discard().is_none());
        dialog.begin_discard();
        assert!(
            matches!(dialog.discard(), Some(Action::Discard(name)) if name == "gaming-synthetic")
        );
        dialog.discard = false;
        assert!(dialog.discard().is_none());
        let mut file_dialog = LayersDialog::new(None);
        assert!(file_dialog.publication().is_none());
        assert!(file_dialog.discard().is_none());
    }
    /// Finish saving <tag>, as its button asks.
    pub(crate) fn finish(dialog: &LayersDialog) -> Option<Action> {
        dialog.finish_pending()
    }
    /// The publication a user requests through the Save as new image… form.
    pub(crate) fn reviewed_publication(
        name: &str,
        kind: WorkspaceKind,
        details: ImageDetails,
        source: SourceImage,
        tag: &str,
        flatten: bool,
        title: Option<&str>,
    ) -> Action {
        let mut dialog = LayersDialog::for_workspace(Some(name.into()), kind, "Synthetic machine");
        dialog.set_source(source);
        dialog.set_details(Ok(details));
        dialog.begin_save(flatten);
        let form = dialog.save.as_mut().unwrap();
        let (repo, tag) = tag.split_once(':').unwrap();
        form.set_repo(repo.into());
        form.set_tag(tag.into());
        if let Some(title) = title {
            form.title = Some(title.into());
        }
        if let Some(choice) = &mut form.flatten {
            *choice = flatten;
        }
        dialog.publication().unwrap()
    }
    #[test]
    fn the_save_form_renders_without_a_window() {
        let mut dialog = LayersDialog::for_workspace(
            Some("regular-1".into()),
            WorkspaceKind::Regular,
            "Synthetic machine",
        );
        dialog.set_source(source("example:1.0.0", &Digest::of(b"example")));
        dialog.set_details(Ok(details()));
        dialog.begin_save(true);
        let ctx = egui::Context::default();
        let mut frame = || {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(matches!(dialog.show(ui.ctx(), false, true), Action::None));
            });
            output.textures_delta.clear();
            output
        };
        let _ = frame();
        let output = frame();
        for text in [
            "Save as new image",
            "Base image (FROM chain)",
            "Flatten disk chains into new bases",
        ] {
            assert!(
                output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(t) if t.galley.text().contains(text))),
                "{text}"
            );
        }
    }
    #[test]
    fn affected_media_excludes_readonly_and_unchanged_writable_media() {
        let mut details = details();
        for (id, policy, sectors) in [
            ("readonly", pc_image::Policy::Readonly, Some(3)),
            ("unchanged", pc_image::Policy::Journal, Some(0)),
            ("changed", pc_image::Policy::Journal, Some(1)),
            ("volatile", pc_image::Policy::Volatile, None),
        ] {
            details.media.push(pc_image::inspection::MediumDetails {
                id: id.into(),
                drive: id.into(),
                name: id.into(),
                policy,
                chain: vec![],
                working_sectors: sectors,
            });
        }
        let mut dialog = LayersDialog::for_workspace(
            Some("disposable".into()),
            WorkspaceKind::Disposable,
            "Synthetic machine",
        );
        dialog.set_details(Ok(details));
        assert_eq!(dialog.title, "Disk layers — Synthetic machine");
        assert_eq!(
            dialog
                .affected()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["changed", "volatile"]
        );
    }

    /// The mock-ups' shadow colour (`lo`), light and dark.
    const LO: [egui::Color32; 2] = [
        egui::Color32::from_rgb(141, 138, 131),
        egui::Color32::from_rgb(31, 32, 35),
    ];
    /// The mock-ups' dark shadow colour (`dk`), light and dark.
    const DK: [egui::Color32; 2] = [
        egui::Color32::from_rgb(63, 61, 57),
        egui::Color32::from_rgb(15, 16, 17),
    ];

    /// A dialog showing `row`'s check as the worker reads it, with the first
    /// fixed disk holding three written sectors on a base two images share.
    fn shown(store: &image_store::Store, row: &crate::library_view::ImageRow) -> LayersDialog {
        let crate::library_worker::Event::Checked(_, Ok(mut details), Some(checked)) =
            crate::library_worker::checked(store, row, None)
        else {
            panic!("the test image checks")
        };
        let ide: Vec<&str> = checked
            .cards
            .iter()
            .filter(|(_, kind)| kind == "ide")
            .map(|(id, _)| id.as_str())
            .collect();
        let disk = details
            .media
            .iter_mut()
            .find(|m| {
                m.drive
                    .split_once(':')
                    .is_some_and(|(card, _)| ide.contains(&card))
            })
            .expect("a fixed disk");
        disk.working_sectors = Some(3);
        disk.chain[0].shared_by = 2;
        let mut dialog = LayersDialog::for_workspace(
            Some("regular-1".into()),
            WorkspaceKind::Regular,
            "Synthetic machine",
        );
        // The ROMs as the library names them when the event arrives: the
        // check's configuration and the image's summary.
        let summary = crate::library_view::firmware::tests::warm(store)
            .summary(&row.descriptor.digest)
            .cloned();
        let facts = hardware::ImageFacts::new(
            &details,
            Some(&checked),
            summary.as_ref(),
            &std::collections::BTreeMap::new(),
        );
        dialog.set_image(Some(*checked), facts.firmware);
        dialog.set_details(Ok(details));
        dialog.set_source(source(&row.reference, &row.descriptor.digest));
        dialog
    }

    /// Every shape `dialog` paints once its modal has settled, in a skin and
    /// a theme, flattened, with the visuals it was painted in.
    fn paint(
        dialog: &mut LayersDialog,
        classic: bool,
        dark: bool,
    ) -> (Vec<egui::Shape>, egui::Visuals) {
        fn flatten(shape: egui::Shape, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| flatten(s, out)),
                shape => out.push(shape),
            }
        }
        let visuals = ui_kit::skin::skin_visuals(classic, dark);
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        ctx.set_visuals(visuals.clone());
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0));
        let mut shapes = Vec::new();
        // egui measures a new modal in an invisible first frame, then fades
        // it in; a second between frames ends the fade.
        for frame in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(f64::from(frame)),
                    ..Default::default()
                },
                |ui| {
                    assert!(matches!(
                        dialog.show(ui.ctx(), false, classic),
                        Action::None
                    ));
                },
            );
            output.textures_delta.clear();
            shapes.clear();
            for clipped in output.shapes {
                flatten(clipped.shape, &mut shapes);
            }
        }
        (shapes, visuals)
    }

    /// The painted texts, each with the rectangle of its galley.
    fn texts(shapes: &[egui::Shape]) -> Vec<(&egui::epaint::TextShape, egui::Rect)> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some((
                    text,
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect()
    }

    /// Every painted text, one per line.
    fn all_text(shapes: &[egui::Shape]) -> String {
        texts(shapes)
            .iter()
            .map(|(text, _)| text.galley.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// No controller id, medium id or configuration key in `shown`.
    fn assert_no_internal_ids(shown: &str, details: &ImageDetails, what: &str) {
        for raw in [
            "ide:",
            "fdc-765",
            "firmware.bios",
            "firmware.option_roms",
            "medium-",
            "inserted:",
        ] {
            assert!(!shown.contains(raw), "{what}: {raw:?} in {shown}");
        }
        for medium in &details.media {
            if medium.drive.contains(':') {
                assert!(
                    !shown.contains(&medium.drive),
                    "{what}: {:?} in {shown}",
                    medium.drive
                );
            }
        }
    }

    /// The open template image's tree names its drives by letter, type and
    /// size, its media by name with their write modes, and its firmware by
    /// the catalogue's names with the open mark, as the approved mock-up
    /// does, in both skins and both themes. Neither the tree nor the discard
    /// question shows a controller id, a medium id or a configuration key.
    #[test]
    fn the_tree_names_drives_media_and_firmware_as_the_mockup_does() {
        let Some(store) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&store);
        let mut dialog = shown(&store.store, &row);
        let details = dialog.details.clone().unwrap();
        // The catalogue's names, as the details pane shows them.
        let roms = [
            "unemu AT BIOS 1.0.2-unemu.3",
            "unemu VGA BIOS 0.9d-unemu.1 (std)",
            "XTIDE Universal BIOS r638-xtp-ata1f0",
        ];
        let disk = human_size(615 * 4 * 17 * 512);
        for classic in [false, true] {
            for dark in [false, true] {
                let what = format!("classic {classic}, dark {dark}");
                dialog.discard = false;
                let (shapes, _) = paint(&mut dialog, classic, dark);
                let shown = all_text(&shapes);
                for expected in [
                    format!("C: IDE master ({disk})"),
                    "A: 1.2M catalogue".into(),
                    "empty".into(),
                    "└─ base  blank".into(),
                    "shared by 2 images".into(),
                    "working changes".into(),
                    "3 sectors".into(),
                    "Firmware".into(),
                    "(open)".into(),
                    "Settings".into(),
                    "from open-at:1.0.0".into(),
                ] {
                    assert!(shown.contains(&expected), "{what}: {expected:?} in {shown}");
                }
                for rom in roms {
                    assert!(shown.contains(rom), "{what}: {rom:?} in {shown}");
                }
                assert_no_internal_ids(&shown, &details, &what);
                dialog.begin_discard();
                let (shapes, _) = paint(&mut dialog, classic, dark);
                let asked = all_text(&shapes);
                assert!(
                    asked.contains("Discard changes to the affected media?"),
                    "{what}"
                );
                assert!(asked.contains("C: IDE master"), "{what}: {asked}");
                assert_no_internal_ids(&asked, &details, &what);
            }
        }
    }

    /// The working changes read as the mock-up's highlighted current line
    /// (`.cur`): the selection fill behind them and the selected text colour
    /// on it, as every skin and theme keeps that pair legible.
    #[test]
    fn the_working_changes_take_the_selection_highlight() {
        let f = crate::library_test_support::Fixture::new();
        let mut dialog = shown(&f.store, &f.row);
        for classic in [false, true] {
            for dark in [false, true] {
                let what = format!("classic {classic}, dark {dark}");
                let (shapes, visuals) = paint(&mut dialog, classic, dark);
                let (line, _) = texts(&shapes)
                    .into_iter()
                    .find(|(t, _)| t.galley.text().contains("working changes"))
                    .unwrap_or_else(|| panic!("{what}: no working changes"));
                let text = line.galley.text();
                for part in ["working changes", "3 sectors", "not saved"] {
                    let at = text
                        .find(part)
                        .unwrap_or_else(|| panic!("{what}: {part:?} in {text:?}"));
                    let section = line
                        .galley
                        .job
                        .sections
                        .iter()
                        .find(|s| s.byte_range.contains(&egui::epaint::text::ByteIndex(at)))
                        .unwrap();
                    assert_eq!(
                        (section.format.background, section.format.color),
                        (visuals.selection.bg_fill, visuals.selection.stroke.color),
                        "{what}: {part:?} in {text:?}"
                    );
                }
            }
        }
    }

    /// The footer is the mock-up's one row: Save as new image… first, as the
    /// default button in the strong text colour, then Discard changes and
    /// Close. Save as layer… and New image from here… are that one button.
    #[test]
    fn the_footer_is_one_row_with_save_as_new_image_first() {
        let f = crate::library_test_support::Fixture::new();
        let mut dialog = shown(&f.store, &f.row);
        for classic in [false, true] {
            for dark in [false, true] {
                let what = format!("classic {classic}, dark {dark}");
                let (shapes, visuals) = paint(&mut dialog, classic, dark);
                let painted = texts(&shapes);
                let at = |label: &str| {
                    painted
                        .iter()
                        .find(|(t, _)| t.galley.text() == label)
                        .unwrap_or_else(|| panic!("{what}: {label:?} not painted"))
                };
                let (save_text, save) = at("Save as new image…");
                let (close_text, close) = at("Close");
                let (_, discard) = at("Discard changes");
                for (label, rect) in [("Discard changes", discard), ("Close", close)] {
                    assert!(
                        (rect.center().y - save.center().y).abs() < 1.0,
                        "{what}: {label} is not on the Save as new image… row"
                    );
                }
                assert!(
                    save.right() < discard.left() && discard.right() < close.left(),
                    "{what}: {save:?} {discard:?} {close:?}"
                );
                assert_eq!(
                    save_text.galley.job.sections[0].format.color,
                    visuals.strong_text_color(),
                    "{what}: the default button"
                );
                assert_ne!(
                    close_text.galley.job.sections[0].format.color,
                    visuals.strong_text_color(),
                    "{what}: Close is not the default"
                );
                for gone in ["Save as layer…", "New image from here…"] {
                    assert!(
                        painted.iter().all(|(t, _)| t.galley.text() != gone),
                        "{what}: {gone}"
                    );
                }
            }
        }
    }

    /// The tree sits in one sunken box on the field colour inside the
    /// mock-ups' inset outline: the classic skin's two-pixel bevel, shadow
    /// and dark shadow along its top, the modern skin's 1 px line in the
    /// shadow colour, in both themes.
    #[test]
    fn the_tree_sits_in_one_sunken_box() {
        let f = crate::library_test_support::Fixture::new();
        let mut dialog = shown(&f.store, &f.row);
        for classic in [false, true] {
            for dark in [false, true] {
                let what = format!("classic {classic}, dark {dark}");
                let theme = usize::from(dark);
                let (shapes, visuals) = paint(&mut dialog, classic, dark);
                let painted = texts(&shapes);
                let lines: Vec<egui::Rect> = [
                    "C: IDE master",
                    "└─ base",
                    "working changes",
                    "Firmware",
                    "Settings",
                ]
                .iter()
                .map(|part| {
                    painted
                        .iter()
                        .find(|(t, _)| t.galley.text().contains(part))
                        .map(|(_, rect)| *rect)
                        .unwrap_or_else(|| panic!("{what}: {part:?} not painted"))
                })
                .collect();
                let field = visuals.text_edit_bg_color();
                let boxes: Vec<egui::Rect> = shapes
                    .iter()
                    .filter_map(|shape| match shape {
                        egui::Shape::Rect(r)
                            if r.fill == field
                                && lines.iter().all(|line| r.rect.contains_rect(*line)) =>
                        {
                            Some(r.rect)
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(boxes.len(), 1, "{what}: {boxes:?}");
                let rect = boxes[0];
                if classic {
                    let edge = |ring: egui::Rect, colour: egui::Color32| {
                        let (a, b) = (ring.left_top(), ring.right_top());
                        shapes.iter().any(|shape| {
                            matches!(shape, egui::Shape::LineSegment { points, stroke }
                                if stroke.color == colour
                                    && points[0].distance(a) < 0.01
                                    && points[1].distance(b) < 0.01)
                        })
                    };
                    assert!(edge(rect.shrink(0.5), LO[theme]), "{what}: outer bevel");
                    assert!(edge(rect.shrink(1.5), DK[theme]), "{what}: inner bevel");
                } else {
                    assert!(
                        shapes
                            .iter()
                            .any(|shape| matches!(shape, egui::Shape::Rect(r)
                            if r.rect == rect
                                && r.fill == egui::Color32::TRANSPARENT
                                && r.stroke == egui::Stroke::new(1.0, LO[theme]))),
                        "{what}: no 1 px shadow-coloured outline"
                    );
                }
            }
        }
    }

    /// Save as new image… opens the one Save as new image… form with the
    /// option to flatten disk chains, unchecked: it saves what Save as
    /// layer… saved and still offers what New image from here… offered.
    /// Gaming mode's Keep as new image… opens the form without the option.
    #[test]
    fn save_as_new_image_opens_the_one_form_with_the_flatten_option() {
        let f = crate::library_test_support::Fixture::new();
        for (kind, label, flatten) in [
            (WorkspaceKind::Regular, "Save as new image…", Some(false)),
            (WorkspaceKind::Disposable, "Keep as new image…", None),
        ] {
            let mut dialog = shown(&f.store, &f.row);
            dialog.kind = kind;
            let pointer = crate::library_test_support::Pointer::new(egui::vec2(1024.0, 768.0));
            let mut draw = |ui: &mut egui::Ui| {
                dialog.show(ui.ctx(), false, false);
            };
            let mut painted = Vec::new();
            for _ in 0..3 {
                painted = pointer.frame(vec![], &mut draw);
            }
            let at = crate::library_test_support::painted_at(&painted, label).center();
            pointer.click(at, &mut draw);
            let form = dialog
                .save
                .as_ref()
                .unwrap_or_else(|| panic!("{label} opens the form"));
            assert_eq!(form.flatten, flatten, "{label}");
        }
    }

    /// The dialog keeps one size frame after frame in both skins, with the
    /// media read and while they load: a modal lays out inside the size it
    /// measured the frame before, so a row taking all the height on offer
    /// would carry that height over. When the media arrive, the dialog takes
    /// its new size in the next frame rather than growing over several.
    #[test]
    fn the_dialog_keeps_its_size_frame_after_frame() {
        let f = crate::library_test_support::Fixture::new();
        for classic in [false, true] {
            let full = shown(&f.store, &f.row);
            let mut dialog = LayersDialog::for_workspace(
                Some("regular-1".into()),
                WorkspaceKind::Regular,
                "Synthetic machine",
            );
            let ctx = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0));
            let mut rects = Vec::new();
            for frame in 0..8 {
                if frame == 3 {
                    dialog.set_image(full.checked.clone(), full.firmware.clone());
                    dialog.set_details(Ok(full.details.clone().unwrap()));
                }
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        time: Some(f64::from(frame)),
                        ..Default::default()
                    },
                    |ui| {
                        ui.ctx()
                            .set_visuals(ui_kit::skin::skin_visuals(classic, false));
                        dialog.show(ui.ctx(), false, classic);
                    },
                );
                output.textures_delta.clear();
                rects.push(ctx.memory(|m| m.area_rect(egui::Id::unique("disk-layers"))));
            }
            // egui centres a modal by the size it measured the frame before,
            // so the dialog moves once, a frame after it grows.
            let heights: Vec<f32> = rects.iter().flatten().map(|r| r.height()).collect();
            assert!(
                heights[3..].iter().all(|h| (h - heights[3]).abs() < 0.5)
                    && rects[4..].iter().all(|rect| *rect == rects[4]),
                "classic {classic}: {rects:?}"
            );
            assert!(
                heights[3] > heights[2],
                "classic {classic}: the media did not show"
            );
            for loaded in [true, false] {
                let mut dialog = if loaded {
                    shown(&f.store, &f.row)
                } else {
                    LayersDialog::for_workspace(
                        Some("regular-1".into()),
                        WorkspaceKind::Regular,
                        "Synthetic machine",
                    )
                };
                crate::library_test_support::settled_modal(
                    egui::Id::unique("disk-layers"),
                    egui::vec2(1024.0, 768.0),
                    |ctx| {
                        ctx.set_visuals(ui_kit::skin::skin_visuals(classic, false));
                        assert!(matches!(dialog.show(ctx, false, classic), Action::None));
                    },
                );
            }
        }
    }

    /// The Save as new image… form lists what is saved by drive letter and
    /// medium name, as the tree does, never by controller or medium id.
    #[test]
    fn the_save_form_names_what_is_saved_by_drive() {
        let f = crate::library_test_support::Fixture::new();
        let mut dialog = shown(&f.store, &f.row);
        let details = dialog.details.clone().unwrap();
        dialog.begin_save(true);
        for classic in [false, true] {
            let what = format!("classic {classic}");
            let (shapes, _) = paint(&mut dialog, classic, false);
            let shown = all_text(&shapes);
            assert!(shown.contains("What is saved"), "{what}: {shown}");
            assert!(shown.contains("C: IDE master"), "{what}: {shown}");
            assert_no_internal_ids(&shown, &details, &what);
        }
    }

    /// A journalled medium of `drive` with one base of 2 KiB.
    fn medium(id: &str, drive: &str, name: &str, working: Option<usize>) -> MediumDetails {
        MediumDetails {
            id: id.into(),
            drive: drive.into(),
            name: name.into(),
            policy: pc_image::Policy::Journal,
            chain: vec![pc_image::inspection::ChainMember {
                digest: Digest::of(id.as_bytes()),
                size: 2048,
                shared_by: 1,
                blank_bytes: None,
            }],
            working_sectors: working,
        }
    }
    fn image(media: Vec<MediumDetails>, slots: &[&str], firmware: &[&str]) -> ImageDetails {
        ImageDetails {
            media,
            slots: slots.iter().map(|s| s.to_string()).collect(),
            firmware: firmware.iter().map(|s| s.to_string()).collect(),
            hardware: vec![],
            stored_bytes: 4096,
            exclusive_bytes: 3072,
            dedup_bytes: 1024,
        }
    }
    /// What a check reads of an image whose hardware tables have these
    /// cards, as an image stores them.
    fn checked_of(cards: &str) -> CheckedImage {
        let machine = format!(
            "schema = 1\n[board]\nmodel = \"ibm-5170\"\n[cpu]\nmodel = \"80286\"\nrate = \"8/13\"\n[memory]\nconventional_kib = 640\nextended_kib = 0\n[firmware.bios]\nbase = 1015808\nimage = \"assets/firmware-0.bin\"\nprovides = [\"floppy\", \"fixed-disk\"]\nsha256 = \"{}\"\n[[card]]\nkind = \"vga\"\n{cards}",
            Digest::of(b"bios").hex()
        );
        let table: toml::Table = toml::from_str(&machine).unwrap();
        CheckedImage {
            draft: Some(crate::image_settings::describe(&table).unwrap()),
            ..CheckedImage::default()
        }
    }
    fn rom(name: &str, open: Option<bool>) -> FirmwareFact {
        FirmwareFact {
            role: "System BIOS".into(),
            name: name.into(),
            open,
        }
    }
    /// The tree as text, a line per line, a fact's label and value two
    /// spaces apart; the highlighted parts in brackets.
    fn text(lines: &[Line]) -> Vec<String> {
        let parts = |parts: &[(String, Tone)]| {
            parts
                .iter()
                .map(|(text, tone)| match tone {
                    Tone::Current => format!("[{text}]"),
                    _ => text.clone(),
                })
                .collect::<String>()
        };
        lines
            .iter()
            .map(|line| match line {
                Line::Chain(chain) => parts(chain),
                Line::Gap => String::new(),
                Line::Fact(label, value) => format!("{label}  {}", parts(value)),
            })
            .collect()
    }
    /// A one-member chain's base line and working changes, as `chain` lines
    /// them up.
    fn base(id: &str, working: &str) -> [String; 2] {
        [
            format!(
                "{:<23}  {}  2.0 KiB",
                " └─ base",
                short(Digest::of(id.as_bytes()).hex())
            ),
            format!("     └─ [working changes  {working}]"),
        ]
    }
    fn settings() -> [String; 2] {
        [
            "Settings  from at:1.0.0".into(),
            "Storage  stored 4.0 KiB · exclusive 3.0 KiB · dedup savings 1.0 KiB".into(),
        ]
    }

    /// The tree of each kind of machine, line by line: two IDE disks with
    /// CMOS and user-supplied firmware; a slave without a master, lettered
    /// D: as the status bar's chips letter it; A: and a 1.44M B: with no
    /// fixed disk and mixed firmware marks; and an image read without its
    /// configuration, whose drives go by their cards' kinds.
    #[test]
    fn the_tree_reads_each_kind_of_machine_as_the_mockup_does() {
        assert_eq!(short("0123456789abcdef"), "0123…cdef");
        let source = source("at:1.0.0", &Digest::of(b"at"));
        let fact = |label: &str, value: &str| format!("{label}  {value}");

        // Two IDE disks, CMOS, no floppy drive, user-supplied firmware.
        let checked = checked_of(
            "[[card]]\nkind = \"fdc-765\"\nirq = 6\ndma = 2\n[[card]]\nkind = \"ide\"\ncontroller_model = \"ata\"\nirq = 14\nport = 496\n[[card.drives]]\nchannel = \"master\"\ngeometry = { cylinders = 615, heads = 4, sectors = 17 }\n[[card.drives]]\nchannel = \"slave\"\ngeometry = { cylinders = 306, heads = 4, sectors = 17 }\n",
        );
        let mut cmos = medium("cmos", "CMOS", "CMOS settings", Some(1));
        cmos.chain[0].size = 64;
        let details = image(
            vec![
                medium("medium-0", "ide:0", "medium-0", Some(3)),
                medium("medium-1", "ide:1", "games", Some(0)),
                cmos,
            ],
            &[
                "ide:0 catalogue: medium-0 · inserted: medium-0",
                "ide:1 catalogue: medium-1 · inserted: medium-1",
            ],
            &[],
        );
        let roms = [
            rom("IBM 5170 BIOS", Some(false)),
            rom("IBM VGA BIOS", Some(false)),
        ];
        let [c, c_work] = base("medium-0", "3 sectors   not saved");
        let [d, d_work] = base("medium-1", "no changes");
        let [settings, storage] = settings();
        assert_eq!(
            text(&tree(&details, Some(&checked), &roms, &source)),
            [
                "C: IDE master (20.4 MiB) · journal".to_owned(),
                c,
                c_work,
                "D: IDE slave (10.2 MiB) — games · journal".into(),
                d,
                d_work,
                "CMOS settings · journal".into(),
                format!(
                    "{:<23}  {}  64 B",
                    " └─ base",
                    short(Digest::of(b"cmos").hex())
                ),
                "     └─ [working changes  1 sector   not saved]".into(),
                String::new(),
                fact(
                    "Firmware",
                    "IBM 5170 BIOS · IBM VGA BIOS  (user-supplied, by hash)"
                ),
                settings.clone(),
                storage.clone(),
            ]
        );

        // A slave without a master, read without a draft: D:, by its card.
        let checked = CheckedImage {
            cards: vec![("ide".into(), "ide".into())],
            ..CheckedImage::default()
        };
        let details = image(
            vec![medium("medium-0", "ide:1", "medium-0", Some(0))],
            &["ide:1 catalogue: medium-0 · inserted: medium-0"],
            &[],
        );
        let lines = text(&tree(&details, Some(&checked), &[], &source));
        assert_eq!(lines[0], "D: IDE slave · journal");
        assert_eq!(lines[4], fact("Firmware", "none"));

        // A: and a 1.44M B:, no fixed disk; B: holds a medium whose id is
        // `empty`, so the check's line cannot say which medium is in B:.
        let checked = checked_of(
            "[[card]]\nkind = \"fdc-765\"\nirq = 6\ndma = 2\n[[card.drives]]\ntype = \"1.2M\"\nunit = 0\n[[card.drives]]\ntype = \"1.44M\"\nunit = 1\n",
        );
        let mut utils = medium("medium-1", "fdc-765:1", "utils", Some(0));
        utils.policy = pc_image::Policy::Readonly;
        let details = image(
            vec![
                medium("medium-0", "fdc-765:0", "freedos-1200", Some(0)),
                utils,
                medium("empty", "fdc-765:1", "transfer", Some(12)),
            ],
            &[
                "fdc-765:0 catalogue: medium-0 · inserted: medium-0",
                "fdc-765:1 catalogue: medium-1, empty · inserted: empty",
            ],
            &[],
        );
        let roms = [
            rom("Open AT BIOS", Some(true)),
            rom("IBM VGA BIOS", Some(false)),
            rom("ROM 0123456789ab", None),
        ];
        assert_eq!(
            text(&tree(&details, Some(&checked), &roms, &source)),
            [
                fact("A: 1.2M catalogue", "freedos-1200 (journal, in the drive)"),
                fact(
                    "B: 1.44M catalogue",
                    "utils (read-only) · transfer (journal) [12 sectors not saved]"
                ),
                fact(
                    "Firmware",
                    "Open AT BIOS (open) · IBM VGA BIOS (user-supplied) · ROM 0123456789ab"
                ),
                settings.clone(),
                storage.clone(),
            ]
        );

        // No configuration at all: the cards' usual ids and the check's
        // digest starts, as the details pane names an image it cannot read.
        let details = image(
            vec![
                medium("medium-0", "ide:0", "medium-0", Some(0)),
                medium("medium-1", "fdc-765:0", "medium-1", Some(0)),
            ],
            &["fdc-765:0 catalogue: medium-1 · inserted: medium-1"],
            &["firmware.bios · 0123456789ab"],
        );
        let roms = hardware::ImageFacts::new(&details, None, None, &Default::default()).firmware;
        let [c, c_work] = base("medium-0", "no changes");
        assert_eq!(
            text(&tree(&details, None, &roms, &source)),
            [
                "C: IDE master · journal".to_owned(),
                c,
                c_work,
                String::new(),
                fact("A: catalogue", "disk 1 (journal, in the drive)"),
                fact("Firmware", "ROM 0123456789ab"),
                settings,
                storage,
            ]
        );
    }

    /// Tab reaches the footer's buttons in the order they read: Save as new
    /// image…, Discard changes, Close.
    #[test]
    fn tab_reaches_the_footer_in_reading_order() {
        let f = crate::library_test_support::Fixture::new();
        let key = |key: egui::Key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // What Space does after `tabs` presses of Tab from a fresh dialog.
        let reached = |tabs: usize| {
            let mut dialog = shown(&f.store, &f.row);
            let pointer = crate::library_test_support::Pointer::new(egui::vec2(1024.0, 768.0));
            let mut closed = false;
            let mut draw = |ui: &mut egui::Ui| {
                closed |= matches!(dialog.show(ui.ctx(), false, false), Action::Close);
            };
            for _ in 0..3 {
                pointer.frame(vec![], &mut draw);
            }
            for _ in 0..tabs {
                pointer.frame(vec![key(egui::Key::Tab)], &mut draw);
            }
            pointer.frame(vec![key(egui::Key::Space)], &mut draw);
            pointer.frame(vec![], &mut draw);
            if dialog.save.is_some() {
                Some("Save as new image…")
            } else if dialog.discard {
                Some("Discard changes")
            } else if closed {
                Some("Close")
            } else {
                None
            }
        };
        let mut order: Vec<&str> = Vec::new();
        for tabs in 1..40 {
            if let Some(button) = reached(tabs) {
                if order.last() != Some(&button) {
                    order.push(button);
                }
                if button == "Close" {
                    break;
                }
            }
        }
        assert_eq!(order, ["Save as new image…", "Discard changes", "Close"]);
    }
}
