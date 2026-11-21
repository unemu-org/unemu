// SPDX-License-Identifier: AGPL-3.0-or-later
//! Create from files…: the media picker's second view, as revision 2 of the
//! approved media-picker mock-up draws it; machine-images §5.2.
//!
//! The view lists host files and folders, lays them out on a diskette of a
//! format the drive takes (Microsoft FAT Specification, through
//! [`media::fat12`]; IBM Personal Computer AT Technical Reference, diskette
//! formats) and says live whether they fit one disk, need a split across
//! several, or hold a file too large for any disk. Only names, kinds and
//! lengths are read before Create, on the library worker; the window then
//! lays them out again for every edit of the top folder, label or format
//! without reading the host. Create and insert hands the shell a
//! [`CreateOrder`]; the library worker builds the images with the same
//! builder as the command line and stores them as the machine's own disks
//! ([`pc_media::created_media`]).

use crate::media_picker::{
    drive_name, format_name, group_box, hint, hint_under, key_cap, notice, pane, segment, Outcome,
    Picker, When, PANE_GAP, SMALL_SIZE,
};
use egui::{RichText, Stroke};
use media::fat12::{volume_label, BuildError, FloppyFormat, HostItem, Plan, ShortName, Survey};
use pc_media::created_media::{self, CreateOrder};
use std::path::{Path, PathBuf};
use ui_kit::pane::{good_colour, group_colour};

/// One host file or folder of the list.
#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    /// Its names and sizes, once the library worker read them.
    pub read: Option<Result<HostItem, String>>,
    /// Whether the shell asked the worker to read it.
    asked: bool,
}

impl Item {
    fn name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}

/// What the files come to on one disk of the chosen format.
#[derive(Clone, Debug)]
pub(crate) enum Fit {
    /// Nothing added yet.
    Empty,
    /// The worker still reads names and sizes.
    Reading,
    /// A top folder or label DOS refuses.
    Name(String),
    /// Something else that blocks Create: a host item that could not be
    /// read, a folder path too deep for DOS, every file skipped.
    Problem(String),
    Fits,
    /// A file larger than an empty disk of the format.
    TooLarge {
        name: String,
        bytes: u64,
    },
    /// Too big for one disk.
    Over {
        /// The split across several disks of the format.
        split: Result<Box<Plan>, String>,
        /// Data-area bytes needed beyond one disk.
        beyond: u64,
        /// Root entries needed and the format's, when the root overflows.
        root: Option<(usize, usize)>,
        /// A larger format the drive takes that holds everything.
        larger: Option<FloppyFormat>,
    },
}

/// The layout of the current list, and its fit.
#[derive(Clone, Debug)]
pub(crate) struct Outlook {
    pub survey: Option<Survey>,
    pub fit: Fit,
}

impl Outlook {
    /// Disks Create makes: one when it fits, the split's otherwise, none
    /// when it cannot.
    pub fn disks(&self) -> usize {
        match &self.fit {
            Fit::Fits => 1,
            Fit::Over {
                split: Ok(plan), ..
            } => plan.disks.len(),
            _ => 0,
        }
    }
    /// Why Create cannot run, in one phrase.
    pub fn problem(&self, format: FloppyFormat) -> Option<String> {
        match &self.fit {
            Fit::Empty => Some("Add files or a folder".into()),
            Fit::Reading => Some("Reading names and sizes".into()),
            Fit::Name(why) | Fit::Problem(why) => Some(why.clone()),
            Fit::TooLarge { name, .. } => {
                Some(format!("{name} is too large for any {format} disk"))
            }
            Fit::Over {
                split: Err(why), ..
            } => Some(why.clone()),
            Fit::Fits | Fit::Over { .. } => None,
        }
    }
}

/// The Create view's state: the list, the disk's form and its outlook.
#[derive(Clone, Debug)]
pub struct Create {
    pub items: Vec<Item>,
    pub top: String,
    top_touched: bool,
    pub label: String,
    label_touched: bool,
    pub format: Option<FloppyFormat>,
    pub journal: bool,
    pub keep: bool,
    keep_touched: bool,
    /// Counts changes of the list and the form, so the outlook is made once
    /// per change.
    revision: u64,
    outlook: Option<(u64, FloppyFormat, Vec<FloppyFormat>, Outlook)>,
}

impl Default for Create {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            top: String::new(),
            top_touched: false,
            label: String::new(),
            label_touched: false,
            format: None,
            journal: true,
            keep: false,
            keep_touched: false,
            revision: 0,
            outlook: None,
        }
    }
}

/// The formats a drive takes, its own first.
pub(crate) fn formats(drive: assembly::FloppyGeometry) -> Vec<FloppyFormat> {
    let own = created_media::format_of(drive);
    own.into_iter()
        .chain(
            drive
                .accepted_media()
                .into_iter()
                .filter_map(created_media::format_of)
                .filter(|format| Some(*format) != own),
        )
        .collect()
}

/// `n` with thousands separated by commas, as the mock-up writes bytes.
pub(crate) fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn human(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} bytes", grouped(bytes))
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Why DOS would not keep a host name as it stands, as the DOS names
/// section lists it.
fn rename_reason(host: &str, dos_path: &str) -> String {
    let short = dos_path.rsplit('\\').next().unwrap_or(dos_path);
    let (stem, extension) = match host.rfind('.').filter(|&at| at > 0) {
        Some(at) => (&host[..at], &host[at + 1..]),
        None => (host, ""),
    };
    let mut why = Vec::new();
    if host.contains(char::is_whitespace) {
        why.push("space".to_owned());
    }
    if stem.contains('.') {
        why.push("more than one dot".to_owned());
    }
    let mut refused: Vec<char> = host
        .chars()
        .filter(|c| {
            !c.is_whitespace()
                && *c != '.'
                && !(c.is_ascii_alphanumeric() || "!#$%&'()-@^_`{}~".contains(*c))
        })
        .collect();
    refused.dedup();
    if !refused.is_empty() {
        let shown: Vec<String> = refused.iter().map(char::to_string).collect();
        why.push(format!("{} not allowed", shown.join(" ")));
    }
    if stem.chars().filter(|c| *c != '.').count() > 8 {
        why.push("name over 8 characters".to_owned());
    }
    if extension.chars().count() > 3 {
        why.push("extension over 3 characters".to_owned());
    }
    if why.is_empty() && short.contains('~') {
        why.push("same name as another file".to_owned());
    }
    if why.is_empty() {
        why.push("not a DOS name".to_owned());
    }
    why.join(", ")
}

impl Create {
    /// Add host files and folders; one already listed changes nothing.
    /// Returns how many were new.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> usize {
        let mut added = 0;
        for path in paths {
            if self.items.iter().any(|item| item.path == path) {
                continue;
            }
            self.items.push(Item {
                path,
                read: None,
                asked: false,
            });
            added += 1;
        }
        if added > 0 {
            self.revision += 1;
        }
        added
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
            self.revision += 1;
        }
    }

    /// The items not yet handed to the worker; they count as asked now.
    pub fn take_reads(&mut self) -> Vec<PathBuf> {
        self.items
            .iter_mut()
            .filter(|item| !item.asked)
            .map(|item| {
                item.asked = true;
                item.path.clone()
            })
            .collect()
    }

    /// The worker's names and sizes of `path`, or why it could not read it.
    pub fn read_done(&mut self, path: &Path, result: Result<HostItem, String>) {
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.path == path && item.read.is_none())
        {
            item.read = Some(result);
            self.revision += 1;
        }
    }

    pub fn set_top(&mut self, text: &str) {
        let text = text.to_uppercase();
        if text != self.top {
            self.top = text;
            self.top_touched = true;
            self.revision += 1;
        }
    }

    pub fn set_label(&mut self, text: &str) {
        let text = text.to_uppercase();
        if text != self.label {
            self.label = text;
            self.label_touched = true;
            self.revision += 1;
        }
    }

    pub fn set_format(&mut self, format: FloppyFormat) {
        self.format = Some(format);
    }

    pub fn set_keep(&mut self, keep: bool) {
        self.keep = keep;
        self.keep_touched = true;
    }

    /// The only item when it is a folder that has been read.
    fn alone(&self) -> Option<&Item> {
        match self.items.as_slice() {
            [item] if matches!(&item.read, Some(Ok(read)) if read.is_folder()) => Some(item),
            _ => None,
        }
    }

    /// Fill in what the user has not set, and lay the list out again when
    /// anything it depends on changed: the format the drive takes (its own
    /// unless another it takes was chosen), the top folder (a single added
    /// folder's name cut to 8.3, else none), the label (the top folder) and
    /// keeping in the library (on for a split, off for one disk).
    pub fn settle(&mut self, accepted: &[FloppyFormat], journal_available: bool) {
        if !self.format.is_some_and(|f| accepted.contains(&f)) {
            self.format = accepted.first().copied();
        }
        if !journal_available {
            self.journal = false;
        }
        if !self.top_touched {
            let top = self
                .alone()
                .and_then(|item| item.path.file_name())
                .map(|name| ShortName::from_host(name).to_string())
                .unwrap_or_default();
            if top != self.top {
                self.top = top;
                self.revision += 1;
            }
        }
        if !self.label_touched {
            let label: String = self.top.chars().filter(|c| *c != '.').take(11).collect();
            if label != self.label {
                self.label = label;
                self.revision += 1;
            }
        }
        let Some(format) = self.format else {
            return;
        };
        let fresh = self.outlook.as_ref().is_none_or(|(revision, f, a, _)| {
            *revision != self.revision || *f != format || a != accepted
        });
        if fresh {
            let outlook = self.compute(format, accepted);
            self.outlook = Some((self.revision, format, accepted.to_vec(), outlook));
        }
        if !self.keep_touched {
            self.keep = self.outlook().is_some_and(|o| o.disks() > 1);
        }
    }

    /// The outlook [`Self::settle`] made last.
    pub(crate) fn outlook(&self) -> Option<&Outlook> {
        self.outlook.as_ref().map(|(.., outlook)| outlook)
    }

    fn top_name(&self) -> Result<Option<ShortName>, String> {
        let top = self.top.trim();
        if top.is_empty() {
            return Ok(None);
        }
        ShortName::parse(top)
            .map(Some)
            .ok_or_else(|| format!("The top folder {top} is not an 8.3 name"))
    }

    fn label_bytes(&self) -> Result<Option<[u8; 11]>, String> {
        let label = self.label.trim_end();
        if label.is_empty() {
            return Ok(None);
        }
        if label.len() > 11 {
            return Err("The volume label has more than 11 characters".into());
        }
        volume_label(label)
            .map(Some)
            .map_err(|_| "The volume label has a character DOS does not allow".into())
    }

    fn compute(&self, format: FloppyFormat, accepted: &[FloppyFormat]) -> Outlook {
        let fit = |fit| Outlook { survey: None, fit };
        if self.items.is_empty() {
            return fit(Fit::Empty);
        }
        let mut items = Vec::new();
        for item in &self.items {
            match &item.read {
                None => return fit(Fit::Reading),
                Some(Err(why)) => return fit(Fit::Problem(why.clone())),
                Some(Ok(read)) => items.push(read.clone()),
            }
        }
        let top = match self.top_name() {
            Ok(top) => top,
            Err(why) => return fit(Fit::Name(why)),
        };
        let label = match self.label_bytes() {
            Ok(label) => label,
            Err(why) => return fit(Fit::Name(why)),
        };
        let survey = match Survey::new(&items, top, format, label) {
            Ok(survey) => survey,
            Err(error) => return fit(Fit::Problem(error.to_string())),
        };
        let fit = if survey.files() == 0 {
            Fit::Problem("Nothing to copy: every file was skipped".into())
        } else {
            match survey.plan() {
                Ok(_) => Fit::Fits,
                Err(BuildError::FileTooLarge {
                    dos_path, bytes, ..
                }) => Fit::TooLarge {
                    name: dos_path.rsplit('\\').next().unwrap_or_default().to_owned(),
                    bytes,
                },
                Err(BuildError::DoesNotFit { .. } | BuildError::RootFull { .. }) => {
                    let larger = accepted.iter().copied().find(|other| {
                        other.data_bytes() > format.data_bytes()
                            && Survey::new(&items, top, *other, label)
                                .is_ok_and(|survey| survey.plan().is_ok())
                    });
                    let capacity = format.root_entries();
                    Fit::Over {
                        split: survey.plan_split().map(Box::new).map_err(|e| e.to_string()),
                        beyond: survey.needed().saturating_sub(format.data_bytes()),
                        root: (survey.root_entries() > capacity)
                            .then_some((survey.root_entries(), capacity)),
                        larger,
                    }
                }
                Err(error) => Fit::Problem(error.to_string()),
            }
        };
        Outlook {
            survey: Some(survey),
            fit,
        }
    }

    /// What the disks are called together: the top folder without its
    /// extension, else the label, else FILES.
    pub fn title(&self) -> String {
        let top = self.top.trim();
        let top = top.split('.').next().unwrap_or_default();
        if !top.is_empty() {
            top.to_owned()
        } else if !self.label.trim().is_empty() {
            self.label.trim().to_owned()
        } else {
            "FILES".into()
        }
    }

    /// The tag a kept set gets: the title in lower case with hyphens as the
    /// work of a user-supplied set (`private/media/<platform>/<work>`),
    /// release 1.0, first revision (recipes.md §8.3).
    pub fn tag(&self) -> String {
        let mut slug = String::new();
        for c in self.title().chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug = slug.trim_end_matches('-');
        format!(
            "{}:{}",
            pc_image::media_repository(
                pc_image::PRIVATE_NAMESPACE,
                if slug.is_empty() { "files" } else { slug }
            ),
            image_store::tag::revision(Some("1.0"), 1)
        )
    }

    /// What Create and insert asks the shell for, when it can run.
    pub fn order(&self, unit: u8) -> Option<CreateOrder> {
        let outlook = self.outlook()?;
        let format = self.format?;
        if outlook.problem(format).is_some() {
            return None;
        }
        let items = self
            .items
            .iter()
            .map(|item| item.read.clone()?.ok())
            .collect::<Option<Vec<_>>>()?;
        Some(CreateOrder {
            unit,
            items,
            top: self.top_name().ok()?,
            format,
            label: self.label_bytes().ok()?,
            split: outlook.disks() > 1,
            journal: self.journal,
            keep: self.keep.then(|| (self.tag(), self.title())),
            title: self.title(),
        })
    }
}

// ---- rendering -------------------------------------------------------------

/// The Next disk keys of a drive.
fn next_key(unit: u8) -> &'static str {
    if unit == 0 {
        "Ctrl+Alt+F"
    } else {
        "Ctrl+Alt+Shift+F"
    }
}

/// The footer's line: what Create and insert makes, where it goes and when;
/// or why it cannot run.
pub(crate) fn summary(picker: &Picker) -> Result<String, String> {
    let create = &picker.create;
    let (Some(drive), Some(format), Some(outlook)) =
        (picker.drive(), create.format, create.outlook())
    else {
        return Err("This machine has no floppy drive.".into());
    };
    if let Some(note) = &picker.machine().create_unavailable {
        return Err(note.clone());
    }
    if let Some(problem) = outlook.problem(format) {
        return Err(format!("{problem}."));
    }
    let machine = picker.machine();
    let disks = outlook.disks();
    let files = outlook.survey.as_ref().map_or(0, Survey::files);
    let what = if disks > 1 {
        format!("{disks} disks of {format}")
    } else {
        format!("a {format} disk")
    };
    let when = match machine.when {
        When::Now => " — now.",
        When::Halted if drive.unit == 0 => " — now; then Ctrl+Alt+Del boots from it.",
        When::Halted => " — now.",
        When::AtStart => " — at the next Start.",
    };
    let name = drive_name(drive.unit);
    let all = if disks > 1 {
        format!(" All {disks} join {name}'s catalogue.")
    } else {
        String::new()
    };
    Ok(format!(
        "Create {what} from {} and insert {} into {name} of {}{when}{all}",
        plural(files, "file"),
        if disks > 1 { "disk 1" } else { "it" },
        machine.name
    ))
}

/// Create and insert's label.
pub(crate) fn create_label(create: &Create) -> String {
    match create.outlook().map_or(0, Outlook::disks) {
        n if n > 1 => format!("Create {n} disks and insert"),
        _ => "Create and insert".into(),
    }
}

/// The Create view's keys, as the mock-up's footer lists them.
pub(crate) fn key_hints(ui: &mut egui::Ui) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let text = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).small().weak());
        };
        text(ui, "Drop files and folders on the list ·");
        key_cap(ui, "Alt+A");
        key_cap(ui, "Alt+B");
        text(ui, "drive ·");
        key_cap(ui, "Esc");
        text(ui, "closes");
    });
}

/// The body of the Create view: the files and folders beside the disk, in
/// the search view's fixed height.
pub(crate) fn body(
    ui: &mut egui::Ui,
    picker: &mut Picker,
    classic: bool,
    height: f32,
) -> Option<Outcome> {
    let accepted = picker
        .drive()
        .map_or_else(Vec::new, |d| formats(d.geometry));
    let journal = picker.machine().journal_unavailable.is_none();
    picker.create.settle(&accepted, journal);
    ui.add_space(2.0);
    let width = ui.available_width();
    let half = ((width - PANE_GAP) / 2.0).max(0.0);
    let (_, rect) = ui.allocate_space(egui::vec2(width, height));
    let at = |left: f32| {
        egui::Rect::from_min_size(egui::pos2(left, rect.top()), egui::vec2(half, height))
    };
    let mut outcome = None;
    let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    pane(ui, at(rect.left()), classic, "create-files", 8, |ui| {
        files(ui, picker, hovering, &mut outcome)
    });
    pane(
        ui,
        at(rect.left() + half + PANE_GAP),
        classic,
        "create-disk",
        8,
        |ui| disk(ui, picker, classic, &accepted),
    );
    outcome
}

/// The left pane: Add files…, Add folder…, the list, then the DOS names and
/// the skipped entries.
fn files(ui: &mut egui::Ui, picker: &mut Picker, hovering: bool, outcome: &mut Option<Outcome>) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Add files…").clicked() {
            *outcome = Some(Outcome::AddFiles);
        }
        let folder = ui.button("Add folder…");
        if std::mem::take(&mut picker.focus_add) {
            folder.request_focus();
        }
        if folder.clicked() {
            *outcome = Some(Outcome::AddFolder);
        }
        hint(ui, "or drop files and folders on this list");
    });
    ui.add_space(6.0);
    let create = &mut picker.create;
    if create.items.is_empty() {
        drop_zone(ui, hovering);
    } else {
        item_table(ui, create);
    }
    let survey = create.outlook().and_then(|o| o.survey.clone());
    let renames = survey.as_ref().map_or(&[][..], Survey::renames);
    ui.add_space(8.0);
    let roots: Vec<PathBuf> = create.items.iter().map(|item| item.path.clone()).collect();
    egui::CollapsingHeader::new(format!(
        "DOS names — {}",
        if renames.is_empty() {
            "none changed beyond upper case".to_owned()
        } else {
            format!("{} changed beyond upper case", renames.len())
        }
    ))
    .id_salt("create-dos-names")
    .show(ui, |ui| {
        if !renames.is_empty() {
            let widths = shares(ui, &[0.38, 0.32, 0.30]);
            table_row(ui, &widths, &[false; 3], false, |column, ui| {
                ui.label(RichText::new(["On this computer", "On the disk", "Why"][column]).weak());
            });
            for (row, rename) in renames.iter().enumerate() {
                let host = relative(&rename.host, &roots);
                let name = rename
                    .host
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                table_row(ui, &widths, &[false; 3], row % 2 == 1, |column, ui| {
                    match column {
                        0 => ui.label(&host).on_hover_text(&host),
                        1 => ui.label(RichText::new(format!("\\{}", rename.dos_path)).monospace()),
                        _ => ui.label(
                            RichText::new(rename_reason(&name, &rename.dos_path))
                                .small()
                                .weak(),
                        ),
                    };
                });
            }
        }
        hint(
            ui,
            "DOS reads 8.3 names only: up to 8 characters, a dot and up to 3 more. Names are upper case on the disk, and no long names are written.",
        );
    });
    let skipped = survey.as_ref().map_or(&[][..], Survey::skipped);
    egui::CollapsingHeader::new(format!(
        "Skipped — {}",
        if skipped.is_empty() {
            "none".to_owned()
        } else {
            plural(skipped.len(), "item")
        }
    ))
    .id_salt("create-skipped")
    .show(ui, |ui| {
        for entry in skipped {
            let host = relative(&entry.host, &roots);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&host).monospace());
                hint(ui, format!("· {}", entry.reason));
            });
        }
        hint(
            ui,
            "Host junk stays off the disk: .DS_Store files, AppleDouble files (._ and a name) and symbolic links.",
        );
    });
}

/// A host path as the lists name it: from the added item it belongs to,
/// `azuramon/maps/+bonus.map`, or its own name.
fn relative(host: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .find_map(|root| {
            let rest = host.strip_prefix(root).ok()?;
            let mut parts = vec![root.file_name()?.to_string_lossy().into_owned()];
            parts.extend(
                rest.components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned()),
            );
            Some(parts.join("/"))
        })
        .unwrap_or_else(|| {
            host.file_name().map_or_else(
                || host.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
}

/// Column widths from shares of the width the table has.
fn shares(ui: &egui::Ui, shares: &[f32]) -> Vec<f32> {
    let width = ui.available_width();
    shares.iter().map(|share| (width * share).floor()).collect()
}

/// One row of a table with fixed column widths: each cell keeps its width,
/// cuts a long line short with … and right-aligns where `right` says;
/// `stripe` shades the row, as every second row of the mock-up's tables.
fn table_row(
    ui: &mut egui::Ui,
    widths: &[f32],
    right: &[bool],
    stripe: bool,
    mut cell: impl FnMut(usize, &mut egui::Ui),
) {
    let background = ui.painter().add(egui::Shape::Noop);
    let row = ui
        .horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (column, width) in widths.iter().enumerate() {
                let align = if right.get(column).copied().unwrap_or(false) {
                    egui::Align::Max
                } else {
                    egui::Align::Min
                };
                ui.allocate_ui_with_layout(
                    egui::vec2(*width, 0.0),
                    egui::Layout::top_down(align),
                    |ui| {
                        ui.set_width(*width);
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(4, 2))
                            .show(ui, |ui| {
                                ui.set_width((*width - 8.0).max(0.0));
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                cell(column, ui);
                            });
                    },
                );
            }
        })
        .response
        .rect;
    if stripe {
        ui.painter().set(
            background,
            egui::epaint::RectShape::filled(row, 0.0, ui.visuals().faint_bg_color),
        );
    }
}

/// The empty list: a dashed drop zone.
fn drop_zone(ui: &mut egui::Ui, hovering: bool) {
    let response = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 26))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Drop files and folders here").weak());
                hint(
                    ui,
                    "or use Add files… and Add folder…. Nothing on this computer is read beyond names and sizes until Create.",
                );
            });
        })
        .response;
    let colour = if hovering {
        ui.visuals().selection.bg_fill
    } else {
        ui.visuals().weak_text_color()
    };
    let rect = response.rect.shrink(0.5);
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    ui.painter().extend(egui::Shape::dashed_line(
        &corners,
        Stroke::new(1.0, colour),
        4.0,
        3.0,
    ));
}

/// The list: what each item is, where it lands, what it holds and its size,
/// with × to remove it; then the total.
fn item_table(ui: &mut egui::Ui, create: &mut Create) {
    let landing: Vec<String> = create
        .outlook()
        .and_then(|o| o.survey.as_ref())
        .map(|s| s.landing().to_vec())
        .unwrap_or_default();
    let widths = shares(ui, &[0.31, 0.25, 0.25, 0.13, 0.06]);
    let right = [false, false, false, true, false];
    let mut remove = None;
    table_row(ui, &widths, &right, false, |column, ui| {
        let head = ["Added", "On the disk", "Contents", "Size", ""][column];
        ui.label(RichText::new(head).weak());
    });
    ui.separator();
    for (index, item) in create.items.iter().enumerate() {
        let name = item.name();
        let folder = matches!(&item.read, Some(Ok(read)) if read.is_folder());
        let within = item
            .path
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        table_row(
            ui,
            &widths,
            &right,
            index % 2 == 1,
            |column, ui| match column {
                0 => {
                    ui.label(&name)
                        .on_hover_text(item.path.display().to_string());
                    let kind = match &item.read {
                        Some(Ok(_)) if folder => "folder",
                        Some(Ok(_)) => "file",
                        _ => "item",
                    };
                    let place = if within.is_empty() {
                        kind.to_owned()
                    } else {
                        format!("{kind} in {within}")
                    };
                    ui.label(RichText::new(place).small().weak());
                }
                1 => {
                    ui.label(
                        RichText::new(landing.get(index).cloned().unwrap_or_default()).monospace(),
                    );
                }
                2 => match &item.read {
                    None => {
                        ui.label(RichText::new("reading…").weak());
                    }
                    Some(Err(why)) => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(why).small().color(ui.visuals().warn_fg_color),
                            )
                            .wrap(),
                        );
                    }
                    Some(Ok(read)) => {
                        ui.label(if read.is_folder() {
                            let mut text = plural(read.files(), "file");
                            if read.folders() > 0 {
                                text.push_str(&format!(" in {}", plural(read.folders(), "folder")));
                            }
                            text
                        } else {
                            "file".into()
                        });
                        if read.skipped() > 0 {
                            ui.label(
                                RichText::new(format!("{} skipped", read.skipped()))
                                    .small()
                                    .weak(),
                            );
                        }
                    }
                },
                3 => {
                    if let Some(Ok(read)) = &item.read {
                        ui.label(human(read.bytes()))
                            .on_hover_text(format!("{} bytes", grouped(read.bytes())));
                    }
                }
                _ => {
                    let label = format!("Remove {name}");
                    let response = ui
                        .add(egui::Button::new(RichText::new("×").strong()).frame(false))
                        .on_hover_text("Remove");
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, &label)
                    });
                    if response.clicked() {
                        remove = Some(index);
                    }
                }
            },
        );
    }
    let read: Vec<&HostItem> = create
        .items
        .iter()
        .filter_map(|item| item.read.as_ref()?.as_ref().ok())
        .collect();
    let files: usize = read.iter().map(|r| r.files()).sum();
    // A folder added alone is the top folder; others count themselves.
    let alone = create.items.len() == 1;
    let folders: usize = read
        .iter()
        .map(|r| r.folders() + usize::from(r.is_folder() && !alone))
        .sum();
    let bytes: u64 = read.iter().map(|r| r.bytes()).sum();
    ui.separator();
    table_row(ui, &widths, &right, false, |column, ui| match column {
        0 => {
            ui.label(RichText::new("Total").strong());
        }
        2 => {
            let mut text = plural(files, "file");
            if folders > 0 {
                text.push_str(&format!(" in {}", plural(folders, "folder")));
            }
            ui.label(RichText::new(text).strong());
        }
        3 => {
            ui.label(RichText::new(human(bytes)).strong())
                .on_hover_text(format!("{} bytes", grouped(bytes)));
        }
        _ => {}
    });
    if let Some(index) = remove {
        create.remove(index);
    }
}

/// A standard drive's size: 1.2M and 360K drives take 5.25-inch disks,
/// 1.44M and 720K drives 3.5-inch ones.
fn inches(drive: assembly::FloppyGeometry) -> &'static str {
    if drive == assembly::FloppyGeometry::HD_1_2 || drive == assembly::FloppyGeometry::DD_360K {
        "5.25-inch "
    } else if drive == assembly::FloppyGeometry::HD_1_44
        || drive == assembly::FloppyGeometry::DD_720K
    {
        "3.5-inch "
    } else {
        ""
    }
}

/// One row of the disk's form: the label in the left column, the control
/// and its hint in the right one.
fn form_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui), note: &str) {
    ui.horizontal_top(|ui| {
        let height = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(84.0, height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(84.0);
                ui.label(label);
            },
        );
        ui.vertical(|ui| {
            add(ui);
            if !note.is_empty() {
                hint(ui, note);
            }
        });
    });
}

/// The right pane: top folder, format and label; the space on the disk and
/// the fit; the split; the write setting and keeping in the library.
fn disk(ui: &mut egui::Ui, picker: &mut Picker, classic: bool, accepted: &[FloppyFormat]) {
    let machine = picker.machine().name.clone();
    let Some(drive) = picker.drive().cloned() else {
        hint(ui, "This machine has no floppy drive.");
        return;
    };
    let unit = drive.unit;
    let name = drive_name(unit);
    let create = &mut picker.create;
    let mut top = create.top.clone();
    form_row(
        ui,
        "Top folder",
        |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut top)
                    .id_salt("create-top")
                    .char_limit(12)
                    .desired_width(160.0)
                    .hint_text("none: the root"),
            );
        },
        "Everything added goes into this folder on the disk; a single added folder becomes it. Empty puts everything at the root.",
    );
    create.set_top(&top);
    let mut chosen = None;
    let current = create.format;
    form_row(
        ui,
        "Format",
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for format in accepted {
                    let text = crate::media_picker::label_and_meta(
                        ui,
                        format.name(),
                        current == Some(*format),
                        &format!(" {} bytes free", grouped(format.data_bytes())),
                    );
                    if segment(ui, current == Some(*format), text, classic).clicked() {
                        chosen = Some(*format);
                    }
                }
            });
        },
        &format!(
            "{name} is a {}{} drive: it takes {} disks.",
            inches(drive.geometry),
            format_name(drive.geometry),
            accepted
                .iter()
                .map(|f| f.name())
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    );
    if let Some(format) = chosen {
        create.set_format(format);
    }
    let mut label = create.label.clone();
    form_row(
        ui,
        "Volume label",
        |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut label)
                    .id_salt("create-label")
                    .char_limit(11)
                    .desired_width(160.0)
                    .hint_text("none"),
            );
        },
        "Up to 11 characters; DIR and VOL show it.",
    );
    create.set_label(&label);
    let journal = picker.machine().journal_unavailable.is_none();
    let create = &mut picker.create;
    create.settle(accepted, journal);
    let Some(format) = create.format else {
        return;
    };
    let Some(outlook) = create.outlook().cloned() else {
        return;
    };
    ui.add_space(8.0);
    ui.label(RichText::new("Space on the disk").strong());
    meter(ui, &outlook, format, !create.label.trim().is_empty());
    let mut switch = None;
    fit_state(ui, classic, &outlook, format, &mut switch);
    if let Some(format) = switch {
        create.set_format(format);
    }
    split(ui, classic, &outlook, format, &machine, unit, &create.top);
    group_box(ui, "When a program writes to the disk", classic, |ui| {
        let unavailable = picker.machine().journal_unavailable.clone();
        let create = &mut picker.create;
        ui.add_enabled_ui(unavailable.is_none(), |ui| {
            ui.radio_value(
                &mut create.journal,
                true,
                format!("Keep the changes in {machine}"),
            );
        });
        hint_under(
            ui,
            unavailable.unwrap_or_else(|| {
                "Saved with the machine. The created image itself never changes.".into()
            }),
        );
        ui.radio_value(&mut create.journal, false, "Write-protected");
        hint_under(ui, "A program that tries to write gets an error.");
    });
    let create = &mut picker.create;
    ui.add_space(4.0);
    let mut keep = create.keep;
    if ui
        .checkbox(
            &mut keep,
            "Also keep in the library as an installation media set",
        )
        .clicked()
    {
        create.set_keep(keep);
    }
    let disks = outlook.disks();
    let many = disks > 1;
    hint_under(
        ui,
        format!(
            "Adds {} to Installation media as {}, titled {}, so every machine's Insert… lists it. Left off, {} to {machine} alone and {} removed with it.",
            if many {
                format!("the {disks} disks, as one set,")
            } else {
                "the disk".into()
            },
            create.tag(),
            create.title(),
            if many { "the disks belong" } else { "the disk belongs" },
            if many { "are" } else { "is" },
        ),
    );
    ui.add_space(4.0);
    hint(
        ui,
        "The same files with the same names, top folder, label and format give the same image bytes, so the same SHA-256, in whatever order they were added. Host times are not kept: every file and folder gets one fixed date, and nothing random, such as a volume serial number, goes in.",
    );
}

/// The fit meter: the files' own bytes, then cluster rounding and folders
/// lighter, over the free bytes of a freshly formatted disk.
fn meter(ui: &mut egui::Ui, outlook: &Outlook, format: FloppyFormat, label: bool) {
    let capacity = format.data_bytes();
    let survey = outlook.survey.as_ref();
    let used = survey.map_or(0, Survey::needed);
    let files = survey.map_or(0, Survey::file_bytes);
    let over = used > capacity;
    let width = ui.available_width().max(40.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 14.0), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::ProgressIndicator,
            true,
            format!("Space on one {format} disk"),
        )
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, visuals.text_edit_bg_color());
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, ui_kit::pane::shadow_colour(visuals)),
            egui::StrokeKind::Inside,
        );
        let inner = rect.shrink(2.0);
        let share = |bytes: u64| inner.width() * (bytes.min(capacity) as f32 / capacity as f32);
        let colour = if over {
            visuals.warn_fg_color
        } else {
            visuals.selection.bg_fill
        };
        let all = egui::Rect::from_min_size(inner.min, egui::vec2(share(used), inner.height()));
        let own = egui::Rect::from_min_size(inner.min, egui::vec2(share(files), inner.height()));
        painter.rect_filled(all, 0.0, colour.gamma_multiply(0.45));
        painter.rect_filled(own, 0.0, colour);
    }
    let Some(survey) = survey else {
        hint(
            ui,
            format!("One {format} disk holds {} bytes.", grouped(capacity)),
        );
        return;
    };
    if over {
        ui.label(
            RichText::new(format!(
                "{} bytes needed; one {format} disk holds {}",
                grouped(used),
                grouped(capacity)
            ))
            .strong()
            .color(ui.visuals().warn_fg_color),
        );
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(
                RichText::new(format!(
                    "{} of {} bytes used",
                    grouped(used),
                    grouped(capacity)
                ))
                .strong(),
            );
            ui.label(format!(" · {} free", grouped(capacity - used)));
        });
    }
    hint(
        ui,
        format!(
            "Files {} bytes, {} in whole {}-byte clusters · {} {} bytes · root folder {} of {} entries{}",
            grouped(survey.file_bytes()),
            grouped(survey.file_space()),
            grouped(format.cluster_bytes()),
            plural(survey.folders(), "folder"),
            grouped(survey.folder_space()),
            survey.root_entries(),
            format.root_entries(),
            if label { " (the label is one)" } else { "" }
        ),
    );
}

/// The live state under the meter.
fn fit_state(
    ui: &mut egui::Ui,
    classic: bool,
    outlook: &Outlook,
    format: FloppyFormat,
    switch: &mut Option<FloppyFormat>,
) {
    ui.add_space(2.0);
    let capacity = format.data_bytes();
    match &outlook.fit {
        Fit::Empty => hint(ui, "Add files or a folder to see whether they fit."),
        Fit::Reading => hint(ui, "Reading names and sizes…"),
        Fit::Name(why) => notice(ui, classic, |ui, warn| {
            ui.add(egui::Label::new(RichText::new(format!(
                "{why}. A DOS name has up to 8 characters, then a dot and up to 3 more: letters, digits and ! # $ % & ' ( ) - @ ^ _ ` {{ }} ~."
            )).color(warn)).wrap());
        }),
        Fit::Problem(why) => notice(ui, classic, |ui, warn| {
            ui.add(egui::Label::new(RichText::new(format!("{why}.")).color(warn)).wrap());
        }),
        Fit::TooLarge { name, bytes } => notice(ui, classic, |ui, warn| {
            ui.add(egui::Label::new(RichText::new(format!(
                "{name} ({} bytes) is too large for any {format} disk: an empty one holds {} bytes. A file never spans two disks, so a split cannot take it either.",
                grouped(*bytes),
                grouped(capacity)
            )).color(warn)).wrap());
        }),
        Fit::Fits => {
            ui.label(
                RichText::new(format!("Fits one {format} disk."))
                    .strong()
                    .color(good_colour(ui.visuals())),
            );
        }
        Fit::Over {
            beyond,
            root,
            larger,
            ..
        } => notice(ui, classic, |ui, warn| {
            let mut why = Vec::new();
            if *beyond > 0 {
                why.push(format!(
                    "it needs {} bytes more than the disk holds",
                    grouped(*beyond)
                ));
            }
            if let Some((needed, capacity)) = root {
                why.push(format!(
                    "its root folder needs {needed} entries and the disk has {capacity}; a top folder needs only one"
                ));
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(
                    RichText::new(format!(
                        "Does not fit one {format} disk: {}.",
                        why.join(", and ")
                    ))
                    .color(warn),
                );
                if let Some(larger) = larger {
                    ui.label(RichText::new(format!(" It fits one {larger} disk: ")).color(warn));
                    if ui.link(format!("use {larger}")).clicked() {
                        *switch = Some(*larger);
                    }
                    ui.label(RichText::new(".").color(warn));
                }
            });
        }),
    }
}

/// Too big for one disk: the split with its plan and the XCOPY recipe, and
/// Copy into C:, greyed.
fn split(
    ui: &mut egui::Ui,
    classic: bool,
    outlook: &Outlook,
    format: FloppyFormat,
    machine: &str,
    unit: u8,
    top: &str,
) {
    let (plan, too_large) = match &outlook.fit {
        Fit::Over {
            split: Ok(plan), ..
        } => (Some(plan), None),
        Fit::TooLarge { name, .. } => (None, Some(name)),
        _ => return,
    };
    let name = drive_name(unit);
    let top = top.trim();
    group_box(ui, "Too big for one disk", classic, |ui| {
        if let Some(plan) = plan {
            let n = plan.disks.len();
            ui.add(egui::RadioButton::new(
                true,
                RichText::new(format!("Split across {n} disks of {format}")).strong(),
            ));
            hint_under(
                ui,
                if top.is_empty() {
                    "Each file stays whole, in folder order. Every disk keeps the folders its files need.".to_owned()
                } else {
                    format!("Each file stays whole, in folder order. Every disk keeps \\{top}\\ and the folders its files need.")
                },
            );
            let relative = |path: &str| -> String {
                match path.strip_prefix(&format!("{top}\\")) {
                    Some(rest) if !top.is_empty() => rest.to_owned(),
                    _ => path.to_owned(),
                }
            };
            let widths = shares(ui, &[0.09, 0.22, 0.10, 0.17, 0.42]);
            let right = [false, false, true, true, false];
            table_row(ui, &widths, &right, false, |column, ui| {
                let head = ["Disk", "Label", "Files", "Bytes used", "First – last file"][column];
                ui.label(RichText::new(head).weak());
            });
            for (index, disk) in plan.disks.iter().enumerate() {
                let label = disk.label.map_or_else(
                    || "—".to_owned(),
                    |label| String::from_utf8_lossy(label.trim_ascii_end()).into_owned(),
                );
                let first = disk
                    .files
                    .first()
                    .map(|f| relative(&f.dos_path))
                    .unwrap_or_default();
                let last = disk
                    .files
                    .last()
                    .map(|f| relative(&f.dos_path))
                    .unwrap_or_default();
                let span = format!("{first} – {last}");
                table_row(ui, &widths, &right, index % 2 == 1, |column, ui| {
                    match column {
                        0 => ui.label(RichText::new((index + 1).to_string()).strong()),
                        1 => ui.label(&label),
                        2 => ui.label(disk.files.len().to_string()),
                        3 => ui.label(grouped(disk.used_bytes)),
                        _ => ui
                            .label(RichText::new(&span).monospace().size(SMALL_SIZE))
                            .on_hover_text(&span),
                    };
                });
            }
            hint(
                ui,
                format!(
                    "Each disk holds up to {} bytes. The {n} disks join {name}'s catalogue in this order and disk 1 goes in; when the game or you need the next one, Media › Next disk ({}) swaps it.",
                    grouped(format.data_bytes()),
                    next_key(unit)
                ),
            );
            ui.add_space(4.0);
            ui.label("To rebuild the tree on C:, run at the DOS prompt with each disk in turn:");
            let destination = format!("C:\\{}", if top.is_empty() { "GAME" } else { top });
            let command = format!("XCOPY {name}\\{top} {destination} /S /E");
            egui::Frame::new()
                .fill(ui.visuals().text_edit_bg_color())
                .stroke(Stroke::new(1.0, group_colour(ui.visuals())))
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(command).monospace());
                });
            hint(
                ui,
                format!(
                    "On the first disk XCOPY may ask whether {destination} is a file or a directory: answer D. Then {} and the same command for the next disk.{}",
                    next_key(unit),
                    if top.is_empty() {
                        " Without a top folder, the command copies each disk's root; GAME stands for any folder name."
                    } else {
                        ""
                    }
                ),
            );
        } else if let Some(file) = too_large {
            ui.add_enabled(
                false,
                egui::RadioButton::new(false, format!("Split across several {format} disks")),
            );
            hint_under(
                ui,
                format!("Cannot help: {file} alone is larger than a disk."),
            );
        }
        ui.add_space(4.0);
        ui.add_enabled_ui(false, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::RadioButton::new(false, "Copy into C:"));
                ui.label(RichText::new("later").italics().small().weak());
            });
        });
        hint_under(
            ui,
            format!(
                "Writes the files into {machine}'s fixed disk C: as a new disk layer: no floppies and no XCOPY. A separate, bigger feature: unemu would write the fixed disk's own file system from outside the guest."
            ),
        );
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn grouped_bytes_and_rename_reasons_read_as_the_mock_up_writes_them() {
        assert_eq!(grouped(1_457_664), "1,457,664");
        assert_eq!(grouped(512), "512");
        assert_eq!(human(431), "431 bytes");
        assert_eq!(human(48_120), "47.0 KiB");
        assert_eq!(
            rename_reason("Azuramon Manual.txt", "AZURAMON\\AZURAM~1.TXT"),
            "space, name over 8 characters"
        );
        assert_eq!(
            rename_reason("+bonus.map", "MAPS\\_BONUS.MAP"),
            "+ not allowed"
        );
        assert_eq!(
            rename_reason("credits.html", "CREDITS.HTM"),
            "extension over 3 characters"
        );
    }

    /// Host files under a temporary folder: each path relative to it with
    /// its length.
    pub(crate) fn host(tag: &str, files: &[(&str, usize)]) -> machine_file::test_support::TempDir {
        let temp = machine_file::test_support::TempDir::new(tag);
        for (relative, length) in files {
            let path = temp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![0x41; *length]).unwrap();
        }
        temp
    }

    /// The list with `paths` added and read, as the library worker reads them.
    pub(crate) fn listed(paths: &[PathBuf]) -> Create {
        let mut create = Create::default();
        create.add(paths.iter().cloned());
        for path in create.take_reads() {
            let read = HostItem::read(&path).map_err(|e| e.to_string());
            create.read_done(&path, read);
        }
        create
    }

    fn drive(name: &str) -> assembly::FloppyGeometry {
        assembly::descriptors::FLOPPY_TYPES
            .iter()
            .find(|kind| kind.name == name)
            .unwrap()
            .geometry
    }

    /// A single added folder names the top folder, cut to 8.3, and the
    /// label follows it until either is edited; edits stick. Adding the same
    /// item again changes nothing, and × takes it out.
    #[test]
    fn a_folder_sets_the_defaults_edits_stick_and_twice_changes_nothing() {
        let temp = host(
            "create-defaults",
            &[
                ("nightfall/nightfal.exe", 5000),
                ("nightfall/readme.txt", 400),
                ("loose.txt", 10),
            ],
        );
        let folder = temp.path().join("nightfall");
        let mut create = Create::default();
        assert_eq!(create.add([folder.clone()]), 1);
        assert_eq!(create.take_reads(), std::slice::from_ref(&folder));
        assert!(create.take_reads().is_empty(), "each item is read once");
        let accepted = formats(drive("1.44M"));
        create.settle(&accepted, true);
        assert!(matches!(create.outlook().unwrap().fit, Fit::Reading));
        assert_eq!(create.top, "", "no default before the folder is read");
        create.read_done(&folder, HostItem::read(&folder).map_err(|e| e.to_string()));
        create.settle(&accepted, true);
        assert_eq!(
            (create.top.as_str(), create.label.as_str()),
            ("NIGHTFAL", "NIGHTFAL")
        );
        assert_eq!(create.format, Some(FloppyFormat::Hd144));
        assert_eq!(create.tag(), "private/media/ibm-pc/nightfal:1.0-r1");
        assert_eq!(create.add([folder.clone()]), 0);
        assert_eq!(create.items.len(), 1);
        // The label follows the top folder until it is edited.
        create.set_top("night.v2");
        create.settle(&accepted, true);
        assert_eq!(
            (create.top.as_str(), create.label.as_str()),
            ("NIGHT.V2", "NIGHTV2")
        );
        assert_eq!(create.title(), "NIGHT");
        create.set_label("my game");
        create.set_top("ngt");
        create.settle(&accepted, true);
        assert_eq!(
            (create.top.as_str(), create.label.as_str()),
            ("NGT", "MY GAME")
        );
        // A second item keeps the edited top folder.
        create.add([temp.path().join("loose.txt")]);
        for path in create.take_reads() {
            create.read_done(&path, HostItem::read(&path).map_err(|e| e.to_string()));
        }
        create.settle(&accepted, true);
        assert_eq!(create.top, "NGT");
        let survey = create.outlook().unwrap().survey.clone().unwrap();
        assert_eq!(survey.landing(), ["\\NGT\\NIGHTFAL\\", "\\NGT\\LOOSE.TXT"]);
        create.remove(1);
        create.remove(0);
        create.settle(&accepted, true);
        assert!(create.items.is_empty());
        assert!(matches!(create.outlook().unwrap().fit, Fit::Empty));
        // Two items untouched: everything at the root.
        let mut two = listed(&[temp.path().join("loose.txt"), folder]);
        two.settle(&accepted, true);
        assert_eq!((two.top.as_str(), two.label.as_str()), ("", ""));
        assert_eq!(two.title(), "FILES");
    }

    /// The live states: fits one disk; does not fit, with the bytes beyond
    /// one disk and a larger format the drive takes; a file too large for
    /// any disk, which leaves nothing to create.
    #[test]
    fn the_fit_follows_the_files_and_the_format() {
        let temp = host(
            "create-fit",
            &[
                ("small/a.dat", 100_000),
                ("galaxia/a.dat", 400_000),
                ("galaxia/b.dat", 400_000),
                ("big/whole.bin", 1_500_000),
            ],
        );
        let accepted = formats(drive("1.44M"));
        assert_eq!(accepted, [FloppyFormat::Hd144, FloppyFormat::Dd720]);
        let mut small = listed(&[temp.path().join("small")]);
        small.settle(&accepted, true);
        let outlook = small.outlook().unwrap();
        assert!(matches!(outlook.fit, Fit::Fits), "{:?}", outlook.fit);
        assert_eq!(outlook.disks(), 1);
        assert!(
            !small.keep,
            "one disk is not kept in the library by default"
        );
        assert!(small.order(1).is_some_and(|order| !order.split));

        let mut galaxia = listed(&[temp.path().join("galaxia")]);
        galaxia.set_format(FloppyFormat::Dd720);
        galaxia.settle(&accepted, true);
        let survey = galaxia.outlook().unwrap().survey.clone().unwrap();
        match &galaxia.outlook().unwrap().fit {
            Fit::Over {
                beyond,
                larger,
                split: Ok(plan),
                root: None,
            } => {
                assert_eq!(*beyond, survey.needed() - FloppyFormat::Dd720.data_bytes());
                assert!(*beyond > 0);
                assert_eq!(*larger, Some(FloppyFormat::Hd144));
                assert_eq!(plan.disks.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        assert!(galaxia.keep, "a split is kept in the library by default");
        assert_eq!(create_label(&galaxia), "Create 2 disks and insert");
        galaxia.set_format(FloppyFormat::Hd144);
        galaxia.settle(&accepted, true);
        assert!(matches!(galaxia.outlook().unwrap().fit, Fit::Fits));
        assert!(!galaxia.keep, "back on one disk, the default follows");

        let mut big = listed(&[temp.path().join("big")]);
        big.settle(&accepted, true);
        match &big.outlook().unwrap().fit {
            Fit::TooLarge { name, bytes } => {
                assert_eq!((name.as_str(), *bytes), ("WHOLE.BIN", 1_500_000))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            big.outlook()
                .unwrap()
                .problem(FloppyFormat::Hd144)
                .as_deref(),
            Some("WHOLE.BIN is too large for any 1.44M disk")
        );
        assert_eq!(big.order(1), None, "Create is disabled");
        // A top folder DOS refuses blocks Create with the reason.
        small.set_top("two words");
        small.settle(&accepted, true);
        assert!(
            matches!(&small.outlook().unwrap().fit, Fit::Name(why) if why.contains("TWO WORDS"))
        );
        assert_eq!(small.order(1), None);
    }

    /// A split lists each disk: its numbered label, its files, its bytes,
    /// its first and last file; the user's choice of keeping in the library
    /// sticks whatever the plan.
    #[test]
    fn a_split_plans_every_disk_and_the_keep_choice_sticks() {
        let temp = host(
            "create-split",
            &[
                ("azura/level1/a.dat", 500_000),
                ("azura/level1/b.dat", 500_000),
                ("azura/level2/c.dat", 500_000),
                ("azura/readme.txt", 1_000),
            ],
        );
        let accepted = formats(drive("1.44M"));
        let mut create = listed(&[temp.path().join("azura")]);
        create.settle(&accepted, true);
        let Fit::Over {
            split: Ok(plan), ..
        } = &create.outlook().unwrap().fit
        else {
            panic!("{:?}", create.outlook().unwrap().fit)
        };
        let labels: Vec<String> = plan
            .disks
            .iter()
            .map(|disk| String::from_utf8_lossy(disk.label.unwrap().trim_ascii_end()).into_owned())
            .collect();
        assert_eq!(labels, ["AZURA 1", "AZURA 2"]);
        // Folder order: the folder's own README goes on disk 1, before its
        // subfolders' files.
        let paths: Vec<Vec<&str>> = plan
            .disks
            .iter()
            .map(|disk| disk.files.iter().map(|f| f.dos_path.as_str()).collect())
            .collect();
        assert_eq!(
            paths,
            [
                vec![
                    "AZURA\\README.TXT",
                    "AZURA\\LEVEL1\\A.DAT",
                    "AZURA\\LEVEL1\\B.DAT"
                ],
                vec!["AZURA\\LEVEL2\\C.DAT"],
            ]
        );
        assert!(create.keep);
        create.set_keep(false);
        create.settle(&accepted, true);
        assert!(!create.keep, "the user's choice sticks");
        let order = create.order(1).unwrap();
        assert!(order.split && order.keep.is_none() && order.journal);
        create.set_keep(true);
        let order = create.order(1).unwrap();
        assert_eq!(
            order.keep,
            Some((
                "private/media/ibm-pc/azura:1.0-r1".to_owned(),
                "AZURA".to_owned()
            ))
        );
        // Without a journal the disks go in write-protected.
        create.settle(&accepted, false);
        assert!(!create.order(1).unwrap().journal);
    }
}
