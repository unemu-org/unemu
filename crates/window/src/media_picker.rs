// SPDX-License-Identifier: AGPL-3.0-or-later
//! The media picker: one Insert… per floppy drive, as the approved
//! media-picker mock-up draws it; machine-images §5.2 and §7.
//!
//! The picker chooses a target (the machine, its drive and when the disk goes
//! in), a source (the disks already in the machine's catalogues, or the
//! installation media sets of this library) and a disk. A set fits a drive by
//! the accepted-media rule of the drive type (`FloppyGeometry::accepted_media`:
//! a 1.44M drive takes 1.44M and 720K disks, a 1.2M drive 1.2M and 360K);
//! sets that do not fit hide behind one line. Search takes free words and
//! `field:value` words with the ranking tiers of the planned remote catalogue.
//! The picker itself reads nothing: the shell inserts what it returns, through
//! the existing media path for a running machine, or as a pending insert of a
//! stopped machine in the store.

use crate::library_view::pane::{good_colour, group_colour, shadow_colour, warn_ground};
use assembly::FloppyGeometry;
use egui::{Align, Color32, Layout, RichText, Stroke, TextStyle};
use image_store::{Descriptor, Digest};
use std::collections::{BTreeMap, BTreeSet};

/// One disk of a set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Disk {
    pub id: String,
    pub label: String,
    /// `None` for an optical medium.
    pub format: Option<FloppyGeometry>,
}

/// An installation media set of this library, as the picker lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SetEntry {
    pub reference: String,
    pub image: Descriptor,
    pub title: String,
    pub publisher: String,
    pub year: Option<i64>,
    pub genre: String,
    pub platform: String,
    pub labels: Vec<String>,
    pub disks: Vec<Disk>,
    pub extras: usize,
    pub open: bool,
    /// Why no drive takes the set: damaged, optical or of an unknown format.
    pub problem: Option<String>,
}

pub(crate) fn format_name(format: FloppyGeometry) -> String {
    crate::media_dialog::geometry_name(format)
}

impl SetEntry {
    /// The format of the set's disks, as "2 × 1.2M" names it.
    pub fn format(&self) -> Option<FloppyGeometry> {
        self.disks.iter().find_map(|disk| disk.format)
    }
    pub fn disks_label(&self) -> String {
        format!(
            "{} × {}",
            self.disks.len(),
            self.format().map_or_else(|| "CD".to_owned(), format_name)
        )
    }
    /// Whether a drive of this type takes every disk of the set.
    pub fn fits(&self, drive: FloppyGeometry) -> bool {
        let accepted = drive.accepted_media();
        self.problem.is_none()
            && !self.disks.is_empty()
            && self
                .disks
                .iter()
                .all(|disk| disk.format.is_some_and(|f| accepted.contains(&f)))
    }
    fn words(&self) -> Vec<String> {
        format!(
            "{} {} {} {} {} {} {}",
            self.title,
            self.publisher,
            self.genre,
            self.platform,
            self.year.map(|y| y.to_string()).unwrap_or_default(),
            self.labels.join(" "),
            self.reference
        )
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '.'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
    }
    /// The facet values of the set: Format, Platform, Genre, Year,
    /// Publisher and Labels.
    fn facet(&self, name: &str) -> Vec<String> {
        match name {
            "Format" => self.format().map(format_name).into_iter().collect(),
            "Platform" => vec![self.platform.clone()],
            "Genre" => vec![self.genre.clone()],
            "Year" => self.year.map(|y| y.to_string()).into_iter().collect(),
            "Publisher" => vec![self.publisher.clone()],
            "Labels" => self.labels.clone(),
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
    }
}

/// The picker's facets, in the mock-up's order.
pub(crate) const FACETS: [&str; 6] = ["Format", "Platform", "Genre", "Year", "Publisher", "Labels"];
/// Every floppy format the Format facet lists, marked where the drive does
/// not take it.
const FORMATS: [&str; 4] = ["1.44M", "720K", "1.2M", "360K"];

/// Search words: free words, and `field:value` words.
pub(crate) fn tokens(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// Whether a set matches every search word. A free word matches the start of
/// any word of the set; `format:`, `publisher:`, `genre:`, `platform:`,
/// `label:` and `year:` (one year or `from..to`) match that field; an empty
/// value matches everything.
pub(crate) fn matches(set: &SetEntry, tokens: &[String]) -> bool {
    let words = set.words();
    tokens.iter().all(|token| match token.split_once(':') {
        Some((field, value)) if !field.is_empty() => {
            if value.is_empty() {
                return true;
            }
            match field {
                "format" => set
                    .format()
                    .is_some_and(|f| format_name(f).to_lowercase() == value),
                "publisher" => set.publisher.to_lowercase().contains(value),
                "genre" => set.genre.to_lowercase().contains(value),
                "platform" => set
                    .platform
                    .to_lowercase()
                    .replace(char::is_whitespace, "")
                    .contains(value),
                "label" => set.labels.iter().any(|l| l.to_lowercase().contains(value)),
                "year" => match value.split_once("..") {
                    Some((from, to)) => set.year.is_some_and(|year| {
                        from.parse().is_ok_and(|from: i64| year >= from)
                            && to.parse().is_ok_and(|to: i64| year <= to)
                    }),
                    None => set.year.is_some_and(|year| year.to_string() == value),
                },
                _ => true,
            }
        }
        _ => words.iter().any(|word| word.starts_with(token.as_str())),
    })
}

/// The ranking tier of a match: the title first, then the publisher, then
/// the rest. Without free words every set ranks alike and lists by title.
pub(crate) fn rank(set: &SetEntry, tokens: &[String]) -> u8 {
    let words: Vec<&str> = tokens
        .iter()
        .filter(|t| !t.contains(':'))
        .map(String::as_str)
        .collect();
    if words.is_empty() {
        return 9;
    }
    let title = set.title.to_lowercase();
    let title_words: Vec<&str> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if title == words.join(" ") {
        0
    } else if title.starts_with(words[0]) {
        1
    } else if words
        .iter()
        .all(|w| title_words.iter().any(|t| t.starts_with(w)))
    {
        2
    } else if words
        .iter()
        .all(|w| set.publisher.to_lowercase().contains(w))
    {
        3
    } else {
        4
    }
}

/// A disk already in a drive's catalogue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogueDisk {
    pub name: String,
    pub format: FloppyGeometry,
    /// How writes go, in plain words.
    pub write: String,
    pub inserted: bool,
    /// Its place in the drive's catalogue.
    pub index: usize,
    /// The set and the disk's position in it, for a library disk.
    pub set: Option<(Digest, usize)>,
}

/// A floppy drive of the target machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Drive {
    pub unit: u8,
    pub geometry: FloppyGeometry,
    /// What the drive holds now, or the disk waiting for it.
    pub holds: String,
    pub catalogue: Vec<CatalogueDisk>,
    /// The set and disk the drive holds, or will at Start, for the next-disk
    /// default.
    pub current: Option<(Digest, usize)>,
    /// Why disks of the catalogue cannot be chosen here, if so.
    pub catalogue_note: Option<String>,
}

/// When a disk goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum When {
    Now,
    /// The machine halted: the disk goes in now and Ctrl+Alt+Del boots it.
    Halted,
    /// A stopped machine takes the disk at its next Start.
    AtStart,
}

/// A machine the picker can insert into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// A stopped machine of the library, whose pending inserts the store
    /// keeps; `None` for the running machine or a stopped machine file.
    pub id: Option<String>,
    pub name: String,
    pub when: When,
    pub drives: Vec<Drive>,
    /// Why writes cannot be kept in this target, which then takes disks
    /// write-protected only.
    pub journal_unavailable: Option<String>,
    /// Why Create from files… cannot put disks into this target.
    pub create_unavailable: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The disks in this machine's catalogues.
    Machine,
    /// The installation media sets of this library.
    Library,
}

/// The chosen row: a set, or a disk of a drive's catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    Set(usize),
    Entry(u8, usize),
}

/// What Insert, Browse… or Cancel asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A stopped machine's disk for its next Start: the drive's pending
    /// insert, replacing an earlier one.
    Pending {
        id: String,
        insert: image_store::machine::PendingInsert,
    },
    /// A set's disk into a drive now (a running machine) or at Start (a
    /// machine file); `first` goes in, the others join when `all`.
    Set {
        unit: u8,
        set: Descriptor,
        reference: String,
        first: usize,
        all: bool,
        journal: bool,
    },
    /// A disk already in the drive's catalogue.
    Entry {
        unit: u8,
        index: usize,
    },
    /// Browse for an image file…: today's file flow for this drive.
    Browse(u8),
    /// Add files… of Create from files…: the system dialog for several
    /// files; the picker stays open.
    AddFiles,
    /// Add folder… of Create from files…: the system folder dialog; the
    /// picker stays open.
    AddFolder,
    /// Create and insert: build the disks and put them into the drive.
    Create(Box<crate::created_media::CreateOrder>),
    Cancel,
}

/// The picker's two views: searching for a disk, or creating one from host
/// files and folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Search,
    Create,
}

pub(crate) struct Picker {
    pub targets: Vec<Target>,
    pub target: usize,
    pub unit: u8,
    pub source: Source,
    pub query: String,
    pub facets: BTreeMap<String, BTreeSet<String>>,
    pub show_misfits: bool,
    pub choice: Option<Choice>,
    pub disk: usize,
    disk_touched: bool,
    pub all: bool,
    pub journal: bool,
    pub sets: Vec<SetEntry>,
    pub remote_preview: bool,
    /// The configured registries, each with "default" or "private" when it
    /// is, as the From row greys them until after the MVP.
    pub remotes: Vec<(String, String)>,
    focus: bool,
    pub mode: Mode,
    /// Create from files…: kept while the view switches back to search.
    pub create: crate::media_create::Create,
    /// The Create view just opened: Add folder… takes the focus, as the
    /// mock-up does.
    pub(crate) focus_add: bool,
    /// The list scrolls the chosen row into view once, as it does when a
    /// drive's All N disks… opens the picker on the disk in the drive;
    /// another source drops the request.
    pub(crate) reveal: bool,
}

/// The result rows: the sets that fit the drive, then those that do not.
pub(crate) struct Results {
    pub fit: Vec<usize>,
    pub misfit: Vec<usize>,
}

impl Picker {
    /// The picker on `unit` of target `target`, with `set` chosen when given.
    pub fn new(
        targets: Vec<Target>,
        target: usize,
        unit: u8,
        sets: Vec<SetEntry>,
        set: Option<&Digest>,
    ) -> Self {
        let mut picker = Self {
            targets,
            target,
            unit,
            source: Source::Library,
            query: String::new(),
            facets: BTreeMap::new(),
            show_misfits: false,
            choice: None,
            disk: 0,
            disk_touched: false,
            all: true,
            journal: true,
            sets,
            remote_preview: false,
            remotes: Vec::new(),
            focus: true,
            mode: Mode::Search,
            create: crate::media_create::Create::default(),
            focus_add: false,
            reveal: false,
        };
        if !picker.machine().drives.iter().any(|d| d.unit == unit) {
            picker.unit = picker.machine().drives.first().map_or(0, |d| d.unit);
        }
        picker.journal = picker.machine().journal_unavailable.is_none();
        if let Some(index) =
            set.and_then(|digest| picker.sets.iter().position(|s| s.image.digest == *digest))
        {
            picker.choice = Some(Choice::Set(index));
            if picker
                .drive()
                .is_some_and(|d| !picker.sets[index].fits(d.geometry))
            {
                picker.show_misfits = true;
            }
        }
        picker.keep_choice();
        picker
    }
    /// Create from files…: the Create view on the same target.
    pub fn open_create(&mut self) {
        self.mode = Mode::Create;
        self.focus_add = true;
    }
    /// ‹ Back to search, with the search and the added items kept.
    pub fn back_to_search(&mut self) {
        self.mode = Mode::Search;
        self.focus = true;
    }
    pub fn machine(&self) -> &Target {
        &self.targets[self.target.min(self.targets.len().saturating_sub(1))]
    }
    pub fn drive(&self) -> Option<&Drive> {
        self.machine().drives.iter().find(|d| d.unit == self.unit)
    }
    fn facet_ok(&self, set: &SetEntry, except: Option<&str>) -> bool {
        self.facets.iter().all(|(name, values)| {
            Some(name.as_str()) == except
                || values.is_empty()
                || set.facet(name).iter().any(|v| values.contains(v))
        })
    }
    pub fn results(&self) -> Results {
        let tokens = tokens(&self.query);
        let mut all: Vec<usize> = (0..self.sets.len())
            .filter(|&i| matches(&self.sets[i], &tokens) && self.facet_ok(&self.sets[i], None))
            .collect();
        all.sort_by_cached_key(|&i| {
            (
                rank(&self.sets[i], &tokens),
                self.sets[i].title.to_lowercase(),
            )
        });
        let geometry = self.drive().map(|d| d.geometry);
        let (fit, misfit) = all
            .into_iter()
            .partition(|&i| geometry.is_some_and(|g| self.sets[i].fits(g)));
        Results { fit, misfit }
    }
    /// A facet's values with their counts over the current result, and
    /// whether the drive takes that format.
    pub fn facet_values(&self, name: &str) -> Vec<(String, usize, bool)> {
        let tokens = tokens(&self.query);
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for set in &self.sets {
            if matches(set, &tokens) && self.facet_ok(set, Some(name)) {
                for value in set.facet(name) {
                    *counts.entry(value).or_default() += 1;
                }
            }
        }
        let accepted: Vec<String> = self
            .drive()
            .map(|d| {
                d.geometry
                    .accepted_media()
                    .into_iter()
                    .map(format_name)
                    .collect()
            })
            .unwrap_or_default();
        let values: Vec<String> = if name == "Format" {
            FORMATS.iter().map(|f| (*f).to_owned()).collect()
        } else {
            counts.keys().cloned().collect()
        };
        values
            .into_iter()
            .map(|value| {
                let fits = name != "Format" || accepted.contains(&value);
                let count = counts.get(&value).copied().unwrap_or(0);
                (value, count, fits)
            })
            .collect()
    }
    /// The rows the list shows now, in order, for the arrow keys.
    fn rows(&self) -> Vec<Choice> {
        match self.source {
            Source::Machine => self
                .machine()
                .drives
                .iter()
                .flat_map(|d| {
                    d.catalogue
                        .iter()
                        .map(move |e| Choice::Entry(d.unit, e.index))
                })
                .collect(),
            Source::Library => {
                let results = self.results();
                let mut rows: Vec<Choice> = results.fit.iter().map(|&i| Choice::Set(i)).collect();
                if self.show_misfits {
                    rows.extend(results.misfit.iter().map(|&i| Choice::Set(i)));
                }
                rows
            }
        }
    }
    /// Keep a valid choice after the list changed: the first that fits.
    fn keep_choice(&mut self) {
        let rows = self.rows();
        if self.choice.is_some_and(|c| rows.contains(&c)) {
            return;
        }
        self.choice = rows.first().copied();
        self.disk_touched = false;
    }
    pub fn set_query(&mut self, query: String) {
        if query != self.query {
            self.query = query;
            self.disk_touched = false;
            self.keep_choice();
        }
    }
    pub fn toggle_facet(&mut self, name: &str, value: &str) {
        let values = self.facets.entry(name.to_owned()).or_default();
        if !values.remove(value) {
            values.insert(value.to_owned());
        }
        self.keep_choice();
    }
    pub fn set_source(&mut self, source: Source) {
        self.source = source;
        self.reveal = false;
        self.choice = None;
        self.keep_choice();
    }
    pub fn switch_drive(&mut self, unit: u8) {
        if self.machine().drives.iter().any(|d| d.unit == unit) {
            self.unit = unit;
            self.disk_touched = false;
            self.keep_choice();
        }
    }
    pub fn switch_machine(&mut self, target: usize) {
        if target < self.targets.len() && target != self.target {
            self.target = target;
            if self.drive().is_none() {
                self.unit = self.machine().drives.first().map_or(0, |d| d.unit);
            }
            self.journal = self.machine().journal_unavailable.is_none();
            self.disk_touched = false;
            self.keep_choice();
        }
    }
    pub fn choose(&mut self, choice: Choice) {
        self.choice = Some(choice);
        self.disk_touched = false;
    }
    pub fn move_choice(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let at = self
            .choice
            .and_then(|c| rows.iter().position(|r| *r == c))
            .map_or(0, |i| i as isize + delta);
        self.choose(rows[at.clamp(0, rows.len() as isize - 1) as usize]);
    }
    pub fn pick_disk(&mut self, disk: usize) {
        self.disk = disk;
        self.disk_touched = true;
    }
    /// Disk 1, or the next disk when the drive holds one of the same set.
    pub fn default_disk(&self, set: usize) -> usize {
        let set = &self.sets[set];
        match self.drive().and_then(|d| d.current.as_ref()) {
            Some((digest, disk)) if *digest == set.image.digest && disk + 1 < set.disks.len() => {
                disk + 1
            }
            _ => 0,
        }
    }
    /// The disk Insert puts in: the chosen one, else the default.
    pub fn chosen_disk(&self) -> usize {
        match self.choice {
            Some(Choice::Set(set))
                if self.disk_touched && self.disk < self.sets[set].disks.len() =>
            {
                self.disk
            }
            Some(Choice::Set(set)) => self.default_disk(set),
            _ => 0,
        }
    }
    fn entry(&self, unit: u8, index: usize) -> Option<&CatalogueDisk> {
        self.machine()
            .drives
            .iter()
            .find(|d| d.unit == unit)?
            .catalogue
            .iter()
            .find(|e| e.index == index)
    }
    /// Why Insert cannot run now, in one short phrase.
    pub fn problem(&self) -> Option<String> {
        let drive = self.drive()?;
        let name = drive_name(drive.unit);
        match self.choice {
            None => Some("Choose a set".into()),
            Some(Choice::Set(set)) => {
                let set = &self.sets[set];
                if let Some(problem) = &set.problem {
                    Some(problem.clone())
                } else if set.fits(drive.geometry) {
                    None
                } else {
                    Some(format!(
                        "Doesn't fit {name} ({})",
                        format_name(drive.geometry)
                    ))
                }
            }
            Some(Choice::Entry(unit, index)) => {
                let entry = self.entry(unit, index)?;
                if let Some(note) = &drive.catalogue_note {
                    Some(note.clone())
                } else if unit != drive.unit {
                    // A drive's catalogue disk goes into that drive.
                    Some(format!(
                        "In {}'s catalogue — switch to {} (Alt+{})",
                        drive_name(unit),
                        drive_name(unit),
                        char::from(b'A' + unit)
                    ))
                } else if drive.geometry.accepted_media().contains(&entry.format) {
                    None
                } else {
                    Some(format!("Doesn't fit {name}"))
                }
            }
        }
    }
    /// The line above Insert: what goes where, and when.
    pub fn summary(&self) -> String {
        if let Some(problem) = self.problem() {
            return format!("{problem}.");
        }
        let Some(drive) = self.drive() else {
            return "This machine has no floppy drive.".into();
        };
        let what = match self.choice {
            Some(Choice::Set(index)) => {
                let set = &self.sets[index];
                // The disk as its radio button names it, without the number.
                let chosen = self.chosen_disk();
                let disk = disk_name(&set.disks[chosen].label, chosen + 1);
                if set.disks.len() > 1 && self.all {
                    format!(
                        "{} — {disk} (all {} disks join the catalogue)",
                        set.title,
                        set.disks.len()
                    )
                } else {
                    format!("{} — {disk}", set.title)
                }
            }
            Some(Choice::Entry(unit, index)) => self
                .entry(unit, index)
                .map_or_else(String::new, |e| e.name.clone()),
            None => String::new(),
        };
        let machine = self.machine();
        let what = if matches!(self.choice, Some(Choice::Set(_)))
            && (!self.journal || machine.journal_unavailable.is_some())
        {
            format!("{what}, write-protected,")
        } else {
            what
        };
        format!(
            "Insert {what} into {} of {}{}",
            drive_name(drive.unit),
            machine.name,
            match machine.when {
                When::Now => " — now.",
                When::Halted => " — now; then Ctrl+Alt+Del boots from it.",
                When::AtStart => " — at the next Start.",
            }
        )
    }
    /// Insert: what the shell does with the choice.
    pub fn insert(&self) -> Option<Outcome> {
        if self.problem().is_some() {
            return None;
        }
        let unit = self.drive()?.unit;
        match self.choice? {
            Choice::Set(index) => {
                let set = &self.sets[index];
                let first = self.chosen_disk();
                let all = self.all && set.disks.len() > 1;
                Some(match &self.machine().id {
                    Some(id) => Outcome::Pending {
                        id: id.clone(),
                        insert: image_store::machine::PendingInsert {
                            drive: format!("{}:", char::from(b'A' + unit)),
                            set: set.image.clone(),
                            reference: set.reference.clone(),
                            medium: set.disks[first].id.clone(),
                            all,
                            journal: self.journal && self.machine().journal_unavailable.is_none(),
                        },
                    },
                    None => Outcome::Set {
                        unit,
                        set: set.image.clone(),
                        reference: set.reference.clone(),
                        first,
                        all,
                        journal: self.journal && self.machine().journal_unavailable.is_none(),
                    },
                })
            }
            Choice::Entry(from, index) => {
                let entry = self.entry(from, index)?;
                (from == unit).then_some(Outcome::Entry {
                    unit,
                    index: entry.index,
                })
            }
        }
    }
    /// The drive of the same machine that takes a set that does not fit
    /// the chosen one.
    pub fn other_drive(&self, set: &SetEntry) -> Option<&Drive> {
        self.machine()
            .drives
            .iter()
            .find(|d| d.unit != self.unit && set.fits(d.geometry))
    }
}

pub(crate) fn drive_name(unit: u8) -> String {
    format!("{}:", char::from(b'A' + unit))
}

/// The registries the From row greys: the default registry first, then the
/// other configured ones, each marked default or private where it is.
pub(crate) fn remotes(
    config: &image_registry::config::RegistryConfig,
    default: &str,
) -> Vec<(String, String)> {
    let mut remotes = vec![(default.to_owned(), "default".to_owned())];
    for entry in &config.registries {
        if entry.host != default {
            let note = if entry.private { "private" } else { "" };
            remotes.push((entry.host.clone(), note.to_owned()));
        }
    }
    remotes
}

// ---- rendering -------------------------------------------------------------

/// The picker's outer size in a big window: the mock-up's dialog.
const SIZE: egui::Vec2 = egui::vec2(1040.0, 640.0);
/// The narrowest the picker lays out its content. In a narrower window the
/// content scrolls sideways inside the window rather than squeezing.
const MIN_WIDTH: f32 = 720.0;
/// Room kept between the picker and each edge of the window's content area,
/// as the mock-up's dialog keeps 12 px from the page.
const GUTTER: f32 = 12.0;
/// The room the separator above the footer takes.
const SEPARATOR: f32 = 6.0;
/// The room under the footer, inside the picker's frame.
const BOTTOM: i8 = 4;
/// The least height of the body. In a shorter window the picker's content
/// scrolls inside the window instead.
const BODY_MIN: f32 = 240.0;
/// The facets pane's width, as the mock-up's body grid sets it.
const FACETS_WIDTH: f32 = 178.0;
/// The details pane's width, as the mock-up's body grid sets it; the
/// results pane takes the rest.
const DETAILS_WIDTH: f32 = 300.0;
/// The widest a drive's toggle button grows; what the drive holds is cut
/// short beyond it.
const SEGMENT_WIDTH: f32 = 260.0;
/// The mock-up's gap between the body's panes.
pub(crate) const PANE_GAP: f32 = 3.0;
/// The mock-up's 11 px meta text beside its 12 px body text.
pub(crate) const SMALL_SIZE: f32 = 11.0;
/// A result row: the mock-up's 36 × 48 cover with 6 px above and below.
const ROW_HEIGHT: f32 = 60.0;
/// The cover of a result row.
const ROW_COVER: egui::Vec2 = egui::vec2(36.0, 48.0);
/// The details pane's cover: the mock-up's 60 px wide 3:4 tile.
const DETAILS_COVER: egui::Vec2 = egui::vec2(60.0, 80.0);
/// The cover placeholder's colour at its top; it fades to near black.
const COVER_COLOUR: Color32 = Color32::from_rgb(0x2b, 0x3a, 0x55);
/// A catalogue disk's cover: the mock-up's plain grey tile.
const ENTRY_COVER_COLOUR: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);

/// The mock-ups' hover colour (`hover`) under a result row.
fn hover_colour(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(47, 58, 77)
    } else {
        Color32::from_rgb(223, 230, 242)
    }
}

/// Muted small text, wrapped: the mock-up's hints.
pub(crate) fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text.into()).small().weak()).wrap());
}

/// A hint under a check box or radio button, in line with its text.
pub(crate) fn hint_under(ui: &mut egui::Ui, text: impl Into<String>) {
    let spacing = ui.spacing();
    let indent = (spacing.icon_width + spacing.icon_spacing - spacing.item_spacing.x).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(indent);
        hint(ui, text);
    });
}

/// A label in the body style, strong when `strong`, followed by meta text
/// in the small muted style, as one text.
pub(crate) fn label_and_meta(
    ui: &egui::Ui,
    label: &str,
    strong: bool,
    meta: &str,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut label = RichText::new(label);
    if strong {
        label = label.strong();
    }
    let font = egui::FontSelection::Default;
    label.append_to(&mut job, ui.style(), font.clone(), Align::Center);
    RichText::new(meta)
        .small()
        .weak()
        .append_to(&mut job, ui.style(), font, Align::Center);
    job
}

/// A title and the edition in parentheses after it, as the mock-up sets
/// them apart: "FreeDOS 1.4 (Floppy Edition, 1.2M)" is the title
/// "FreeDOS 1.4" and the edition "(Floppy Edition, 1.2M)". The edition is
/// the outermost parenthesis that ends the title.
pub(crate) fn title_parts(title: &str) -> (&str, &str) {
    if !title.ends_with(')') {
        return (title, "");
    }
    let mut depth = 0usize;
    for (at, c) in title.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    let head = title[..at].trim_end();
                    return if head.is_empty() || head.len() == at {
                        (title, "")
                    } else {
                        (head, &title[at..])
                    };
                }
            }
            _ => (),
        }
    }
    (title, "")
}

/// Whether `name` is a file name: a stem, a dot and an extension of one to
/// four letters and digits that starts with a letter, with no spaces.
fn is_file_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, extension)) => {
            !stem.is_empty()
                && !name.contains(char::is_whitespace)
                && (1..=4).contains(&extension.len())
                && extension.chars().all(|c| c.is_ascii_alphanumeric())
                && extension.starts_with(|c: char| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// A disk's label without a file name in parentheses at its end, and that
/// file name: "Disk 1 of 8 - packages (x86DSK01.img)" is "Disk 1 of 8 -
/// packages" from "x86DSK01.img". Other parentheses, as "(1.2M)" or
/// "(v2.0)", stay in the label.
pub(crate) fn label_without_file(label: &str) -> (&str, Option<&str>) {
    if let Some(rest) = label.strip_suffix(')') {
        if let Some(at) = rest.rfind(" (") {
            let file = &rest[at + 2..];
            if at > 0 && is_file_name(file) {
                return (&label[..at], Some(file));
            }
        }
    }
    (label, None)
}

/// A disk by its label, as its radio button names it after its number and
/// the summary names it: what the label says beyond the disk's place, by
/// the one rule the drives' short names use, else the label without its
/// file name.
pub(crate) fn disk_name(label: &str, number: usize) -> String {
    crate::catalogue_menu::own_name(label, number)
        .unwrap_or_else(|| label_without_file(label).0.to_owned())
}

/// A cover's title in lines no wider than `width`: whole words packed onto
/// each line, at most `rows` lines. A word wider than the cover keeps a
/// line of its own, unbroken; the cover's clip cuts it.
fn cover_lines(
    painter: &egui::Painter,
    text: &str,
    font: &egui::FontId,
    width: f32,
    rows: usize,
) -> Vec<String> {
    let fits = |line: &str| {
        painter
            .layout_no_wrap(line.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
            <= width
    };
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        if let Some(last) = lines.last_mut() {
            let longer = format!("{last} {word}");
            if fits(&longer) {
                *last = longer;
                continue;
            }
        }
        if lines.len() == rows {
            break;
        }
        lines.push(word.to_owned());
    }
    lines
}

/// A cover placeholder: the set's short title on a dark tile, as the mock-up
/// draws it until covers come with the registry catalogue. The title breaks
/// between words into at most three centred lines; a line wider than the
/// tile starts at its left edge, so its start stays readable, and the tile
/// clips it.
fn paint_cover(painter: &egui::Painter, rect: egui::Rect, text: &str, top: Color32, size: f32) {
    let painter = painter.with_clip_rect(rect);
    let bottom = Color32::from_rgb(0x0b, 0x0b, 0x0b);
    let mut mesh = egui::Mesh::default();
    for (pos, colour) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), bottom),
        (rect.left_bottom(), bottom),
    ] {
        mesh.colored_vertex(pos, colour);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    let font = egui::FontId::monospace(size);
    let room = rect.width() - 4.0;
    let galleys: Vec<_> = cover_lines(&painter, text, &font, room, 3)
        .into_iter()
        .map(|line| painter.layout_no_wrap(line, font.clone(), Color32::WHITE))
        .collect();
    let height: f32 = galleys.iter().map(|g| g.size().y).sum();
    let mut y = (rect.center().y - height / 2.0).max(rect.top() + 2.0);
    for galley in galleys {
        let width = galley.size().x;
        let x = if width > room {
            rect.left() + 2.0
        } else {
            rect.center().x - width / 2.0
        };
        let line_height = galley.size().y;
        painter.galley(egui::pos2(x, y), galley, Color32::WHITE);
        y += line_height;
    }
}

/// The picker's text sizes and control outlines: the mock-up's 11 px meta
/// text, and an outline on resting buttons, boxes and fields where the skin
/// draws none, so every button reads as a framed button and every check
/// box as a box, as in the library's panes.
fn style(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    if let Some(font) = style.text_styles.get_mut(&TextStyle::Small) {
        font.size = SMALL_SIZE;
    }
    let visuals = &mut style.visuals;
    if visuals.widgets.inactive.bg_stroke.is_empty() {
        let outline = visuals
            .widgets
            .noninteractive
            .fg_stroke
            .color
            .gamma_multiply(0.45);
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, outline);
    }
}

/// Draw the picker; returns what the user chose to do.
pub(crate) fn show(ctx: &egui::Context, picker: &mut Picker, classic: bool) -> Option<Outcome> {
    let mut outcome = None;
    // The picker's keys: ↑↓ choose, Enter inserts, Alt+A / Alt+B switch the
    // drive, Escape closes. Typing goes to the search field.
    let search = egui::Id::unique("media-picker-search");
    // Enter inserts from the search field or the list, never from a focused
    // button (Cancel, Browse…) or an open drop-down.
    let focused = ctx.memory(|m| m.focused());
    let popup = egui::Popup::is_any_open(ctx);
    // The Create view has no list to walk and no Insert: arrows, Enter and
    // typing stay with its fields.
    let creating = picker.mode == Mode::Create;
    let enter_inserts = !creating && !popup && (focused.is_none() || focused == Some(search));
    let (up, down, enter, alt_a, alt_b, typed) = ctx.input_mut(|input| {
        let alt_a = input.consume_key(egui::Modifiers::ALT, egui::Key::A);
        let alt_b = input.consume_key(egui::Modifiers::ALT, egui::Key::B);
        // Alt+A and Alt+B switch the drive; the character the chord also
        // types (a, å, ∫ …) never reaches the search field.
        if alt_a || alt_b {
            input.events.retain(|e| !matches!(e, egui::Event::Text(_)));
        }
        let typed = input
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::Text(_)));
        (
            !creating && !popup && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            !creating && !popup && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            enter_inserts && input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            alt_a,
            alt_b,
            typed,
        )
    });
    // Typing searches wherever the focus is, as the mock-up moves it back.
    if typed && focused != Some(search) && !popup && !creating && picker.source == Source::Library {
        picker.focus = true;
    }
    // Files and folders dropped while the Create view shows join its list.
    if creating {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .filter(|path| !path.as_os_str().is_empty())
                .collect()
        });
        picker.create.add(dropped);
    }
    if up {
        picker.move_choice(-1);
    }
    if down {
        picker.move_choice(1);
    }
    if alt_a {
        picker.switch_drive(0);
    }
    if alt_b {
        picker.switch_drive(1);
    }
    if enter {
        outcome = picker.insert();
    }
    let machine_name = picker.machine().name.clone();
    let mut double = false;
    egui::Modal::new(egui::Id::unique("media-picker")).show(ctx, |ui| {
        style(ui);
        // The mock-up's size, inside the window's content area with room
        // around it. The body takes what the rows above it, laid out this
        // pass at that width, and the footer leave, never what was laid out
        // the frame before: a modal lays its content out inside the size it
        // measured last frame, and a body sized from that would grow the
        // picker frame by frame. Below the least size the content keeps its
        // size and scrolls inside the window. The scroll area is offered that
        // whole size, as the modal offers only last frame's size, and egui's
        // small default size on the first frame.
        let margin = egui::Frame::popup(ui.style()).total_margin().sum();
        let room = ui.ctx().content_rect().size() - egui::Vec2::splat(2.0 * GUTTER);
        let view = (room.min(SIZE) - margin).max(egui::Vec2::ZERO);
        let width = view.x.max(MIN_WIDTH - margin.x);
        egui::ScrollArea::both()
            .id_salt("media-picker-scroll")
            .max_width(view.x)
            .max_height(view.y)
            .min_scrolled_width(view.x)
            .min_scrolled_height(view.y)
            .show(ui, |ui| {
                ui.set_width(width);
                let top = ui.cursor().top();
                let title = if creating {
                    format!("Create disk from files — {machine_name}")
                } else {
                    format!("Insert disk — {machine_name}")
                };
                if classic {
                    ui_kit::skin::gradient_title(ui, &title);
                } else {
                    ui.heading(title);
                }
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 4,
                        bottom: BOTTOM,
                    })
                    .show(ui, |ui| {
                        target_row(ui, picker, classic);
                        if creating {
                            create_from_row(ui);
                        } else {
                            source_row(ui, picker, classic);
                            search_row(ui, picker, search);
                        }
                        let above = ui.cursor().top() - top;
                        let body_height = (view.y - above - footer_height(ui)).max(BODY_MIN);
                        if creating {
                            if let Some(chosen) =
                                crate::media_create::body(ui, picker, classic, body_height)
                            {
                                outcome = Some(chosen);
                            }
                        } else {
                            body(ui, picker, classic, body_height, &mut double);
                        }
                        if let Some(chosen) = footer(ui, picker) {
                            outcome = Some(chosen);
                        }
                    });
            });
        // Escape closes an open drop-down first, then the picker.
        if !egui::Popup::is_any_open(ui.ctx()) && ui.input_mut(ui_kit::skin::escape_pressed) {
            outcome = Some(Outcome::Cancel);
        }
    });
    // A double-click on a row inserts it, as Enter does.
    if double && outcome.is_none() {
        outcome = picker.insert();
    }
    outcome
}

/// A row's muted label, Into or From, in the mock-up's 50 px column.
fn row_label(ui: &mut egui::Ui, text: &str) {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(50.0, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(50.0);
            ui.label(RichText::new(text).weak());
        },
    );
}

/// One button of the mock-up's segmented control (`.seg`): framed at rest,
/// and sunken on the field colour when chosen.
pub(crate) fn segment(
    ui: &mut egui::Ui,
    chosen: bool,
    text: impl Into<egui::WidgetText>,
    classic: bool,
) -> egui::Response {
    let mut button = egui::Button::new(text);
    if chosen {
        button = button.fill(ui.visuals().text_edit_bg_color());
    }
    let response = ui.add(button);
    if chosen && classic && ui.is_rect_visible(response.rect) {
        ui_kit::skin::bevel(ui, response.rect, true);
    }
    response
}

/// The mock-up's Into row: the machine, its drives as toggle buttons, and
/// when the disk goes in.
fn target_row(ui: &mut egui::Ui, picker: &mut Picker, classic: bool) {
    ui.horizontal_wrapped(|ui| {
        row_label(ui, "Into");
        let stopped = picker.machine().when == When::AtStart && picker.machine().id.is_some();
        let current = picker.target;
        let label = |t: &Target| match t.when {
            When::Now => format!("{} (running)", t.name),
            When::Halted => format!("{} (halted)", t.name),
            When::AtStart => format!("{} (stopped)", t.name),
        };
        let mut chosen = current;
        ui.add_enabled_ui(stopped && picker.targets.len() > 1, |ui| {
            egui::ComboBox::from_id_salt("picker-machine")
                .selected_text(label(picker.machine()))
                .show_ui(ui, |ui| {
                    for (index, target) in picker.targets.iter().enumerate() {
                        ui.selectable_value(&mut chosen, index, label(target));
                    }
                });
        })
        .response
        .on_disabled_hover_text(if stopped {
            "The only machine with a floppy drive"
        } else {
            "The running machine"
        });
        picker.switch_machine(chosen);
        let drives: Vec<(u8, String)> = picker
            .machine()
            .drives
            .iter()
            .map(|d| {
                (
                    d.unit,
                    format!(" {} · {}", format_name(d.geometry), d.holds),
                )
            })
            .collect();
        ui.scope(|ui| {
            // The drives sit side by side as one segmented control, each on
            // one line: what a drive holds is cut short when long, and its
            // tooltip says it whole.
            ui.spacing_mut().item_spacing.x = 0.0;
            for (unit, rest) in drives {
                let chosen = picker.unit == unit;
                let name = drive_name(unit);
                let mut text = label_and_meta(ui, &name, chosen, &rest);
                text.wrap = egui::text::TextWrapping {
                    max_width: SEGMENT_WIDTH,
                    max_rows: 1,
                    break_anywhere: true,
                    overflow_character: Some('…'),
                };
                let text = ui.painter().layout_job(text);
                let key = format!("Alt+{}", char::from(b'A' + unit));
                let tip = if text.elided {
                    format!("{name}{rest}\n{key}")
                } else {
                    key
                };
                if segment(ui, chosen, text, classic)
                    .on_hover_text(tip)
                    .clicked()
                {
                    picker.switch_drive(unit);
                }
            }
        });
        let when = picker.machine().when;
        let (text, colour) = match when {
            When::Now => ("Running: goes in now", good_colour(ui.visuals())),
            When::Halted => (
                "Halted: goes in now, then Ctrl+Alt+Del boots from it",
                good_colour(ui.visuals()),
            ),
            When::AtStart => (
                "Stopped: goes in at the next Start",
                ui.visuals().weak_text_color(),
            ),
        };
        ui.label(RichText::new(text).small().color(colour));
    });
}

/// The Create view's From row: files and folders of this computer.
fn create_from_row(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        row_label(ui, "From");
        ui.label(RichText::new("Files and folders on this computer").strong());
    });
}

/// The mock-up's From row: the sources as tabs, the remotes greyed until
/// after the MVP, and the remote preview link.
fn source_row(ui: &mut egui::Ui, picker: &mut Picker, classic: bool) {
    ui.horizontal_wrapped(|ui| {
        row_label(ui, "From");
        let in_machine: usize = picker
            .machine()
            .drives
            .iter()
            .map(|d| d.catalogue.len())
            .sum();
        let sources = [
            (Source::Machine, "This machine", in_machine),
            (Source::Library, "This library", picker.sets.len()),
        ];
        ui.scope(|ui| {
            // The sources sit side by side as one segmented control.
            ui.spacing_mut().item_spacing.x = 0.0;
            for (source, name, count) in sources {
                let chosen = picker.source == source;
                let text = label_and_meta(ui, name, chosen, &format!(" ({count})"));
                if segment(ui, chosen, text, classic).clicked() {
                    picker.set_source(source);
                }
            }
            // The configured registries, greyed until after the MVP.
            let remotes = if picker.remotes.is_empty() {
                vec![("Remotes".to_owned(), String::new())]
            } else {
                picker.remotes.clone()
            };
            for (host, note) in remotes {
                let meta = if note.is_empty() {
                    " · after the MVP".to_owned()
                } else {
                    format!(" {note} · after the MVP")
                };
                let text = label_and_meta(ui, &host, false, &meta);
                ui.add_enabled(false, egui::Button::new(text))
                    .on_disabled_hover_text("Remotes come after the MVP");
            }
        });
        if ui
            .link(if picker.remote_preview {
                "Hide the remote preview"
            } else {
                "What will a remote result look like?"
            })
            .clicked()
        {
            picker.remote_preview = !picker.remote_preview;
        }
    });
}

/// The search field across the picker, with a chip per chosen facet value
/// after it. The field takes the row but for the chips, which keeps it first
/// for Tab; the chips' width comes from the pass before, and when it changes,
/// egui lays the frame out again.
fn search_row(ui: &mut egui::Ui, picker: &mut Picker, search: egui::Id) {
    let chips: Vec<(String, String)> = picker
        .facets
        .iter()
        .flat_map(|(name, values)| values.iter().map(move |v| (name.clone(), v.clone())))
        .collect();
    let id = egui::Id::unique("media-picker-chips-width");
    let known = ui.data(|data| data.get_temp::<f32>(id)).unwrap_or(0.0);
    ui.horizontal(|ui| {
        let mut query = picker.query.clone();
        let response = ui.add_enabled(
            picker.source == Source::Library,
            egui::TextEdit::singleline(&mut query)
                .id(search)
                .margin(egui::Margin::symmetric(8, 4))
                .desired_width((ui.available_width() - known).max(120.0))
                .hint_text(if picker.source == Source::Library {
                    "Search title, publisher, year…  (publisher:apogee, format:720k)"
                } else {
                    "Search works on This library"
                }),
        );
        if std::mem::take(&mut picker.focus) {
            response.request_focus();
        }
        picker.set_query(query);
        let mut width = 0.0;
        for (name, value) in chips {
            let chip = ui.small_button(format!("{name}: {value} ×"));
            width += chip.rect.width() + ui.spacing().item_spacing.x;
            if chip.clicked() {
                picker.toggle_facet(&name, &value);
            }
        }
        if (known - width).abs() > 0.5 {
            ui.data_mut(|data| data.insert_temp(id, width));
            ui.ctx()
                .request_discard("the media picker measured its facet chips");
        }
    });
}

/// The body: the facets, results and details panes side by side, each on
/// the field colour inside the inset outline, as the library's panes, and
/// each scrolling inside the body's fixed height.
fn body(ui: &mut egui::Ui, picker: &mut Picker, classic: bool, height: f32, double: &mut bool) {
    ui.add_space(2.0);
    let width = ui.available_width();
    let results_width = (width - FACETS_WIDTH - DETAILS_WIDTH - 2.0 * PANE_GAP).max(0.0);
    let (_, rect) = ui.allocate_space(egui::vec2(width, height));
    let at = |left: f32, width: f32| {
        egui::Rect::from_min_size(egui::pos2(left, rect.top()), egui::vec2(width, height))
    };
    let results_left = rect.left() + FACETS_WIDTH + PANE_GAP;
    let details_left = results_left + results_width + PANE_GAP;
    pane(
        ui,
        at(rect.left(), FACETS_WIDTH),
        classic,
        "picker-facets",
        4,
        |ui| facets(ui, picker),
    );
    pane(
        ui,
        at(results_left, results_width),
        classic,
        "picker-results",
        0,
        |ui| results(ui, picker, double),
    );
    pane(
        ui,
        at(details_left, DETAILS_WIDTH),
        classic,
        "picker-details",
        8,
        |ui| details(ui, picker, classic),
    );
}

/// One pane of the body at `rect`: the field colour inside the inset
/// outline, `padding` in from it, and what `add` draws scrolling inside.
pub(crate) fn pane(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    classic: bool,
    id: &str,
    padding: i8,
    add: impl FnOnce(&mut egui::Ui),
) {
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    crate::library_view::pane::sunken(&mut ui, classic, padding, |ui| {
        let inner = ui.available_size();
        ui.set_min_size(inner);
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(inner.y)
            .auto_shrink([false, false])
            .show(ui, add);
    });
}

/// The facets: flat sections under the mock-up's small uppercase headings,
/// one line per value with its check box and its count.
fn facets(ui: &mut egui::Ui, picker: &mut Picker) {
    if picker.source != Source::Library {
        hint(ui, "Filters apply to This library.");
        return;
    }
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    let unit = drive_name(picker.unit);
    let mut toggled = None;
    for (index, name) in FACETS.into_iter().enumerate() {
        if index > 0 {
            ui.add_space(6.0);
        }
        ui.label(
            RichText::new(name.to_uppercase())
                .small()
                .color(ui.visuals().weak_text_color())
                .extra_letter_spacing(0.3),
        );
        let values = picker.facet_values(name);
        if values.is_empty() {
            hint(ui, "none here");
        }
        for (value, count, fits) in values {
            let mut on = picker.facets.get(name).is_some_and(|v| v.contains(&value));
            let label = if fits {
                RichText::new(value.clone())
            } else {
                RichText::new(format!("{value} · doesn't fit {unit}")).weak()
            };
            let enabled = count > 0 || on;
            if facet_value(ui, &mut on, enabled, label, count) {
                toggled = Some((name, value));
            }
        }
    }
    if let Some((name, value)) = toggled {
        picker.toggle_facet(name, &value);
    }
}

/// One facet value on one line: its check box with the value, cut short
/// when long, and the count right-aligned in muted text. True when the box
/// was toggled.
fn facet_value(
    ui: &mut egui::Ui,
    on: &mut bool,
    enabled: bool,
    label: RichText,
    count: usize,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let count = RichText::new(count.to_string()).small().weak();
        let count_width = egui::WidgetText::from(count.clone())
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                TextStyle::Small,
            )
            .size()
            .x;
        let room = (ui.available_width() - count_width - ui.spacing().item_spacing.x).max(0.0);
        ui.scope(|ui| {
            ui.set_max_width(room);
            changed = ui
                .add_enabled(enabled, egui::Checkbox::new(on, label))
                .changed();
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(count);
        });
    });
    changed
}

/// What a result row shows: the mock-up's cover, title and subtitle at the
/// left, and its meta on the first line at the right.
struct RowView<'a> {
    /// The row's name for assistive technology: the whole title.
    name: &'a str,
    cover: &'a str,
    cover_colour: Color32,
    title: &'a str,
    edition: &'a str,
    subtitle: String,
    meta: Vec<Meta>,
    selected: bool,
    misfit: bool,
}

/// One item of a result row's meta.
enum Meta {
    /// The disks, as "2 × 1.2M", in strong text.
    Disks(String),
    /// Plain text, as "1 extra".
    Text(String),
    /// A framed mark: the licence (open or private), or the drives whose
    /// catalogues hold the set.
    Badge(String, Option<bool>),
}

/// A result row across the results pane, as the mock-up's `.res`: hovered
/// and selected rows take the hover and selection colours across the whole
/// row, and a row that does not fit is faded. `fit` draws the line under the
/// title of a row that does not fit. A click or double-click anywhere on the
/// row answers through the returned response.
fn result_row(
    ui: &mut egui::Ui,
    view: &RowView<'_>,
    fit: Option<&mut dyn FnMut(&mut egui::Ui)>,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, view.selected, view.name)
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals().clone();
    let mut painter = ui.painter().clone();
    if view.misfit {
        painter.multiply_opacity(if view.selected { 0.8 } else { 0.55 });
    }
    if view.selected {
        painter.rect_filled(rect, 0.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, hover_colour(&visuals));
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, visuals.panel_fill),
    );
    let inner = rect.shrink2(egui::vec2(8.0, 6.0));
    let cover = egui::Rect::from_min_size(inner.min, ROW_COVER);
    paint_cover(&painter, cover, view.cover, view.cover_colour, 10.0);
    let selected_ink = visuals.selection.stroke.color;
    let (ink, strong, muted) = if view.selected {
        (selected_ink, selected_ink, selected_ink)
    } else {
        (
            visuals.text_color(),
            visuals.strong_text_color(),
            visuals.weak_text_color(),
        )
    };
    let small = TextStyle::Small.resolve(ui.style());
    let body = TextStyle::Body.resolve(ui.style());
    // The meta, right to left along the first line.
    let mut right = inner.right();
    let line = egui::Rect::from_min_size(inner.min, egui::vec2(inner.width(), 16.0));
    for meta in view.meta.iter().rev() {
        let (text, colour, framed) = match meta {
            Meta::Disks(text) => (text, strong, false),
            Meta::Text(text) => (text, ink, false),
            Meta::Badge(text, open) => (
                text,
                match open {
                    Some(true) => good_colour(&visuals),
                    Some(false) => visuals.warn_fg_color,
                    None => visuals.text_color(),
                },
                true,
            ),
        };
        let galley = painter.layout_no_wrap(text.clone(), small.clone(), colour);
        let pad = if framed { 4.0 } else { 0.0 };
        let size = galley.size() + egui::vec2(2.0 * pad, 0.0);
        let at = egui::Rect::from_min_size(
            egui::pos2(right - size.x, line.center().y - size.y / 2.0),
            size,
        );
        if framed {
            painter.rect_filled(at, 0.0, visuals.panel_fill);
            painter.rect_stroke(
                at,
                0.0,
                Stroke::new(1.0, shadow_colour(&visuals)),
                egui::StrokeKind::Inside,
            );
        }
        painter.galley(at.min + egui::vec2(pad, 0.0), galley, colour);
        right = at.left() - 5.0;
    }
    // The title and the subtitle, cut short before the meta.
    let left = cover.right() + 10.0;
    let text_width = (right - 5.0 - left).max(0.0);
    let one_line = |job: &mut egui::text::LayoutJob| {
        job.wrap = egui::text::TextWrapping {
            max_width: text_width,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
    };
    let mut title = egui::text::LayoutJob::default();
    title.append(
        view.title,
        0.0,
        egui::TextFormat::simple(body.clone(), strong),
    );
    if !view.edition.is_empty() {
        title.append(
            view.edition,
            4.0,
            egui::TextFormat::simple(body, if view.selected { ink } else { muted }),
        );
    }
    one_line(&mut title);
    let title = painter.layout_job(title);
    let subtitle_top = inner.top() + title.size().y + 1.0;
    painter.galley(egui::pos2(left, inner.top()), title, ink);
    let mut subtitle = egui::text::LayoutJob::single_section(
        view.subtitle.clone(),
        egui::TextFormat::simple(small, muted),
    );
    one_line(&mut subtitle);
    let subtitle = painter.layout_job(subtitle);
    let fit_top = subtitle_top + subtitle.size().y + 2.0;
    painter.galley(egui::pos2(left, subtitle_top), subtitle, muted);
    if let Some(fit) = fit {
        let area = egui::Rect::from_min_max(egui::pos2(left, fit_top), inner.right_bottom());
        let mut line = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(response.id)
                .max_rect(area)
                .layout(Layout::left_to_right(Align::Min)),
        );
        if view.misfit {
            line.multiply_opacity(if view.selected { 0.8 } else { 0.55 });
        }
        if view.selected {
            // Muted text on the selection would all but vanish.
            line.visuals_mut().override_text_color = Some(ink);
            line.visuals_mut().weak_text_color = Some(ink);
        }
        line.spacing_mut().item_spacing.x = 4.0;
        fit(&mut line);
    }
    response
}

/// A group row of the results, as the mock-up's `.pk-group`: small muted
/// text on the second face colour, across the pane.
fn group_row(ui: &mut egui::Ui, text: impl Into<String>) {
    egui::Frame::new()
        .fill(group_colour(ui.visuals()))
        .inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 6,
            bottom: 3,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text.into()).small().strong());
        });
}

/// Muted text in the results pane, with room around it.
fn padded(ui: &mut egui::Ui, padding: i8, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(padding))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// The result list; `insert` is set when a row is double-clicked.
fn results(ui: &mut egui::Ui, picker: &mut Picker, insert: &mut bool) {
    let spacing = ui.spacing().item_spacing;
    // The rows sit edge to edge, each with a rule under it.
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut chosen = None;
    let mut switch = None;
    let mut toggle = false;
    if picker.source == Source::Machine {
        let drives = picker.machine().drives.clone();
        let target = picker.drive().map(|d| (d.unit, d.geometry));
        for drive in &drives {
            group_row(
                ui,
                format!(
                    "{} {} — catalogue",
                    drive_name(drive.unit),
                    format_name(drive.geometry)
                ),
            );
            if drive.catalogue.is_empty() {
                padded(ui, 8, |ui| {
                    ui.weak("Empty");
                });
            }
            for entry in &drive.catalogue {
                let fits = target.is_some_and(|(_, g)| g.accepted_media().contains(&entry.format));
                let choice = Choice::Entry(drive.unit, entry.index);
                let name = drive_name(drive.unit);
                let view = RowView {
                    name: &entry.name,
                    cover: &name,
                    cover_colour: ENTRY_COVER_COLOUR,
                    title: &entry.name,
                    edition: "",
                    subtitle: format!(
                        "{} · {}{}",
                        format_name(entry.format),
                        entry.write,
                        if entry.inserted {
                            format!(" · in {name} now")
                        } else {
                            String::new()
                        }
                    ),
                    meta: Vec::new(),
                    selected: picker.choice == Some(choice),
                    misfit: !fits,
                };
                let unit = target.map_or(0, |(unit, _)| unit);
                let mut line = |ui: &mut egui::Ui| {
                    ui.label(
                        RichText::new(format!("Doesn't fit {}", drive_name(unit)))
                            .small()
                            .weak(),
                    );
                };
                let response = result_row(
                    ui,
                    &view,
                    if fits {
                        None
                    } else {
                        Some(&mut line as &mut dyn FnMut(&mut egui::Ui))
                    },
                );
                // Scrolled to once it is laid out where it shows: a pass that
                // only sizes the picker does not use the request up.
                if view.selected && picker.reveal && ui.is_visible() && !ui.is_sizing_pass() {
                    response.scroll_to_me(Some(Align::Center));
                    picker.reveal = false;
                }
                if response.clicked() {
                    chosen = Some(choice);
                }
                if response.double_clicked() {
                    chosen = Some(choice);
                    *insert = true;
                }
            }
            if let Some(note) = &drive.catalogue_note {
                padded(ui, 8, |ui| hint(ui, note.clone()));
            }
        }
    } else {
        let results = picker.results();
        let drive = picker.drive().cloned();
        for &index in &results.fit {
            let response = set_row(ui, picker, index, false, &mut switch);
            if response.clicked() {
                chosen = Some(Choice::Set(index));
            }
            if response.double_clicked() {
                chosen = Some(Choice::Set(index));
                *insert = true;
            }
        }
        if results.fit.is_empty() {
            padded(ui, 18, |ui| {
                ui.weak(match &drive {
                    Some(d) if !results.misfit.is_empty() => format!(
                        "Nothing that fits {} ({}) matches.",
                        drive_name(d.unit),
                        format_name(d.geometry)
                    ),
                    _ => "Nothing matches. Clear the search or a filter.".to_owned(),
                });
            });
        }
        if !results.misfit.is_empty() {
            if let Some(d) = &drive {
                padded(ui, 8, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} don't fit {} ({}: takes {})",
                                results.misfit.len(),
                                drive_name(d.unit),
                                format_name(d.geometry),
                                d.geometry
                                    .accepted_media()
                                    .into_iter()
                                    .map(format_name)
                                    .collect::<Vec<_>>()
                                    .join(" and ")
                            ))
                            .weak(),
                        );
                        toggle = ui
                            .link(if picker.show_misfits { "Hide" } else { "Show" })
                            .clicked();
                    });
                });
            }
            if picker.show_misfits {
                for &index in &results.misfit {
                    let response = set_row(ui, picker, index, true, &mut switch);
                    if response.clicked() {
                        chosen = Some(Choice::Set(index));
                    }
                }
            }
        }
        if picker.remote_preview {
            padded(ui, 8, |ui| {
                ui.spacing_mut().item_spacing = spacing;
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.weak("From a remote · preview — after the MVP");
                        ui.add_enabled_ui(false, |ui| {
                            ui.label(
                                "A set not in this library: its download size and Pull and insert.",
                            );
                            let _ = ui.button("Pull and insert");
                        });
                        hint(ui, "Pull and insert pulls the set by digest into this library, verifies every byte, then inserts it like a local set. A private remote first asks for a login.");
                    });
            });
        }
    }
    if toggle {
        picker.show_misfits = !picker.show_misfits;
    }
    if let Some(unit) = switch {
        picker.switch_drive(unit);
    }
    if let Some(choice) = chosen {
        picker.choose(choice);
    }
}

/// A set's result row; a set that does not fit says under its title where
/// it would, and its switch link sets `switch`.
fn set_row(
    ui: &mut egui::Ui,
    picker: &Picker,
    index: usize,
    misfit: bool,
    switch: &mut Option<u8>,
) -> egui::Response {
    let set = &picker.sets[index];
    let in_drives: Vec<String> = picker
        .machine()
        .drives
        .iter()
        .filter(|d| {
            d.catalogue
                .iter()
                .any(|e| e.set.as_ref().is_some_and(|(s, _)| *s == set.image.digest))
        })
        .map(|d| drive_name(d.unit))
        .collect();
    let (title, edition) = title_parts(&set.title);
    let year = set.year.map(|y| y.to_string()).unwrap_or_default();
    let mut meta = vec![
        Meta::Disks(set.disks_label()),
        Meta::Text(match set.extras {
            0 => "no extras".to_owned(),
            1 => "1 extra".to_owned(),
            n => format!("{n} extras"),
        }),
        Meta::Badge(
            if set.open { "open" } else { "private" }.to_owned(),
            Some(set.open),
        ),
    ];
    if !in_drives.is_empty() {
        meta.push(Meta::Badge(format!("in {}", in_drives.join(", ")), None));
    }
    let view = RowView {
        name: &set.title,
        cover: title,
        cover_colour: COVER_COLOUR,
        title,
        edition,
        subtitle: [set.publisher.as_str(), year.as_str(), set.genre.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        meta,
        selected: picker.choice == Some(Choice::Set(index)),
        misfit,
    };
    let other = picker.other_drive(set).map(|d| (d.unit, d.geometry));
    let name = picker.machine().name.clone();
    let problem = set.problem.clone();
    let format = set.format().map_or_else(|| "CD".to_owned(), format_name);
    let mut line = |ui: &mut egui::Ui| match (other, &problem) {
        (_, Some(problem)) => {
            ui.label(RichText::new(problem.clone()).small().weak());
        }
        (Some((unit, geometry)), None) => {
            ui.label(
                RichText::new(format!(
                    "Fits {} ({}) —",
                    drive_name(unit),
                    format_name(geometry)
                ))
                .small()
                .weak(),
            );
            if ui
                .link(
                    RichText::new(format!(
                        "switch to {} or Alt+{}",
                        drive_name(unit),
                        char::from(b'A' + unit)
                    ))
                    .small(),
                )
                .clicked()
            {
                *switch = Some(unit);
            }
        }
        (None, None) => {
            ui.label(
                RichText::new(format!("No drive in {name} takes {format}"))
                    .small()
                    .weak(),
            );
        }
    };
    result_row(
        ui,
        &view,
        if misfit {
            Some(&mut line as &mut dyn FnMut(&mut egui::Ui))
        } else {
            None
        },
    )
}

/// A group box, as the mock-up's fieldset: a thin outline in the shadow
/// colour with its legend set into the top edge.
pub(crate) fn group_box(
    ui: &mut egui::Ui,
    legend: &str,
    classic: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    let galley = egui::WidgetText::from(legend).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        ui.available_width() - 24.0,
        TextStyle::Body,
    );
    let half = (galley.size().y / 2.0).round();
    ui.add_space(4.0 + half);
    let frame = egui::Frame::new()
        .stroke(Stroke::new(1.0, shadow_colour(ui.visuals())))
        .corner_radius(if classic { 0 } else { 6 })
        .inner_margin(egui::Margin {
            left: 10,
            right: 8,
            top: half as i8 + 4,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    let rect = frame.response.rect;
    let at = egui::pos2(rect.left() + 8.0, rect.top() - half);
    let behind = egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(3.0, 0.0));
    let painter = ui.painter();
    painter.rect_filled(behind, 0.0, ui.visuals().text_edit_bg_color());
    painter.galley(at, galley, ui.visuals().text_color());
}

/// A notice on the mock-up's warning ground, with the skin's corners.
/// `add` gets the warning colour for its text; a link in it keeps the link
/// colour.
pub(crate) fn notice(ui: &mut egui::Ui, classic: bool, add: impl FnOnce(&mut egui::Ui, Color32)) {
    let visuals = ui.visuals().clone();
    egui::Frame::new()
        .fill(warn_ground(&visuals))
        .corner_radius(if classic { 0 } else { 6 })
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui, visuals.warn_fg_color);
        });
}

/// The details pane's head: the cover, the title, and the reference with
/// the disks and the licence mark.
fn details_head(ui: &mut egui::Ui, set: &SetEntry) {
    let (title, edition) = title_parts(&set.title);
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(DETAILS_COVER, egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            paint_cover(ui.painter(), rect, title, COVER_COLOUR, 11.0);
        }
        ui.add_space(2.0);
        ui.vertical(|ui| {
            let mut job = egui::text::LayoutJob::default();
            let font = egui::FontSelection::Default;
            RichText::new(title).size(13.0).strong().append_to(
                &mut job,
                ui.style(),
                font.clone(),
                Align::Min,
            );
            if !edition.is_empty() {
                RichText::new(format!(" {edition}"))
                    .size(13.0)
                    .weak()
                    .append_to(&mut job, ui.style(), font, Align::Min);
            }
            ui.add(egui::Label::new(job).wrap());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(
                    RichText::new(&set.reference)
                        .monospace()
                        .size(SMALL_SIZE)
                        .weak(),
                );
                ui.label(
                    RichText::new(format!(" · {} · ", set.disks_label()))
                        .small()
                        .weak(),
                );
                let colour = if set.open {
                    good_colour(ui.visuals())
                } else {
                    ui.visuals().warn_fg_color
                };
                // One allocation, so the mark moves to the next line whole
                // rather than wrapping inside itself.
                let text = if set.open { "open" } else { "private" };
                let galley = ui.painter().layout_no_wrap(
                    text.to_owned(),
                    TextStyle::Small.resolve(ui.style()),
                    colour,
                );
                let (rect, _) = ui.allocate_exact_size(
                    galley.size() + egui::vec2(8.0, 0.0),
                    egui::Sense::hover(),
                );
                let painter = ui.painter();
                painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0, shadow_colour(ui.visuals())),
                    egui::StrokeKind::Inside,
                );
                painter.galley(rect.min + egui::vec2(4.0, 0.0), galley, colour);
            });
        });
    });
}

/// The details pane: the chosen set's head, the disk to insert, the
/// catalogue choice and the write setting; or the chosen catalogue disk.
fn details(ui: &mut egui::Ui, picker: &mut Picker, classic: bool) {
    let Some(choice) = picker.choice else {
        hint(ui, "Choose a set.");
        return;
    };
    let unit = picker.unit;
    let name = picker.machine().name.clone();
    match choice {
        Choice::Entry(from, index) => {
            let Some(entry) = picker.entry(from, index).cloned() else {
                return;
            };
            ui.label(RichText::new(&entry.name).size(13.0).strong());
            hint(
                ui,
                format!(
                    "In {}'s catalogue · {}",
                    drive_name(from),
                    format_name(entry.format)
                ),
            );
            if picker.problem().is_none() {
                ui.add(
                    egui::Label::new(format!(
                        "Insert puts it into {}. It keeps its own write setting: {}.",
                        drive_name(unit),
                        entry.write
                    ))
                    .wrap(),
                );
            }
            hint(
                ui,
                "This machine lists the disks already in its drives' catalogues, the quick way to swap disks.",
            );
        }
        Choice::Set(index) => {
            let set = picker.sets[index].clone();
            details_head(ui, &set);
            let Some(drive) = picker.drive().cloned() else {
                return;
            };
            ui.add_space(4.0);
            if !set.fits(drive.geometry) {
                let accepted: Vec<String> = drive
                    .geometry
                    .accepted_media()
                    .into_iter()
                    .map(format_name)
                    .collect();
                let other = picker.other_drive(&set).map(|d| (d.unit, d.geometry));
                let mut switch = None;
                notice(ui, classic, |ui, warn| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "Doesn't fit {}, which takes {} disks.",
                                drive_name(unit),
                                accepted.join(" and ")
                            ))
                            .color(warn),
                        )
                        .wrap(),
                    );
                    match other {
                        Some((other_unit, geometry)) => {
                            if ui
                                .link(format!(
                                    "Switch to {} ({})",
                                    drive_name(other_unit),
                                    format_name(geometry)
                                ))
                                .clicked()
                            {
                                switch = Some(other_unit);
                            }
                        }
                        None => {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!(
                                        "No drive in {name} takes {}.",
                                        set.format().map_or_else(|| "CD".to_owned(), format_name)
                                    ))
                                    .color(warn),
                                )
                                .wrap(),
                            );
                        }
                    }
                });
                if let Some(other_unit) = switch {
                    picker.switch_drive(other_unit);
                }
                return;
            }
            let chosen = picker.chosen_disk();
            let mut picked = None;
            group_box(ui, "Disk to insert", classic, |ui| {
                // One line per disk, its name cut short when long: the
                // radio buttons stay in one column.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for (i, d) in set.disks.iter().enumerate() {
                    let format = d.format.map_or_else(|| "CD".to_owned(), format_name);
                    let shown = format!("{}. {}", i + 1, disk_name(&d.label, i + 1));
                    let text = label_and_meta(ui, &shown, false, &format!(" · {format}"));
                    let response = ui.add(egui::RadioButton::new(chosen == i, text));
                    let response = if label_without_file(&d.label).1.is_some()
                        || !shown.ends_with(d.label.as_str())
                    {
                        response.on_hover_text(&d.label)
                    } else {
                        response
                    };
                    if response.clicked() {
                        picked = Some(i);
                    }
                }
            });
            if let Some(i) = picked {
                picker.pick_disk(i);
            }
            if set.disks.len() > 1 {
                ui.add_space(4.0);
                ui.checkbox(
                    &mut picker.all,
                    format!(
                        "Put all {} disks in {}'s catalogue, in order",
                        set.disks.len(),
                        drive_name(unit)
                    ),
                );
                hint_under(
                    ui,
                    format!(
                        "When a program asks for the next disk, use Media › Next disk ({}).",
                        if unit == 0 {
                            "Ctrl+Alt+F"
                        } else {
                            "Ctrl+Alt+Shift+F"
                        }
                    ),
                );
            }
            if drive
                .catalogue
                .iter()
                .any(|e| e.set.as_ref().is_some_and(|(s, _)| *s == set.image.digest))
            {
                hint(
                    ui,
                    format!(
                        "Already in {}'s catalogue; those disks keep their own write setting.",
                        drive_name(unit)
                    ),
                );
            }
            let unavailable = picker.machine().journal_unavailable.clone();
            group_box(ui, "When a program writes to the disk", classic, |ui| {
                ui.add_enabled_ui(unavailable.is_none(), |ui| {
                    ui.radio_value(
                        &mut picker.journal,
                        true,
                        format!("Keep the changes in {name}"),
                    );
                });
                match &unavailable {
                    Some(reason) => hint_under(ui, reason.clone()),
                    None => hint_under(
                        ui,
                        "Saved with the machine. The disk in the library never changes.",
                    ),
                }
                ui.radio_value(&mut picker.journal, false, "Write-protected");
                hint_under(
                    ui,
                    "Like a disk with its write-protect tab set: a program that tries to write gets an error.",
                );
            });
            if set.extras > 0 {
                ui.add_space(4.0);
                hint(
                    ui,
                    "Extras stay in the library; open them from the set's details.",
                );
            }
        }
    }
}

/// A key cap, as the mock-up's `kbd`: the key in small monospace on the
/// field colour, in a thin outline.
pub(crate) fn key_cap(ui: &mut egui::Ui, key: &str) {
    egui::Frame::new()
        .fill(ui.visuals().text_edit_bg_color())
        .stroke(Stroke::new(1.0, shadow_colour(ui.visuals())))
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(3, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(key).monospace().size(10.5));
        });
}

/// The height the footer takes under the body: the separator, the summary
/// line and the row of keys and buttons, the room between them, and the
/// room under them. It follows from the style alone, so the body's height
/// never waits on the footer's layout.
fn footer_height(ui: &egui::Ui) -> f32 {
    let spacing = ui.spacing();
    let line = spacing.interact_size.y;
    let buttons =
        line.max(ui.text_style_height(&TextStyle::Button) + 2.0 * spacing.button_padding.y);
    3.0 * spacing.item_spacing.y + SEPARATOR + line + buttons + f32::from(BOTTOM)
}

/// The picker's keys, as the mock-up's footer lists them with key caps.
fn key_hints(ui: &mut egui::Ui) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let text = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).small().weak());
        };
        text(ui, "Type to search ·");
        key_cap(ui, "↑");
        key_cap(ui, "↓");
        text(ui, "choose ·");
        key_cap(ui, "Enter");
        text(ui, "inserts ·");
        key_cap(ui, "Alt+A");
        key_cap(ui, "Alt+B");
        text(ui, "drive ·");
        key_cap(ui, "Esc");
        text(ui, "closes");
    });
}

/// The footer: what goes where and when, right-aligned on one line and cut
/// short when long; then the keys, and the framed buttons at the right in the
/// mock-up's order for the eye and for Tab: Browse for an image file…,
/// Create from files…, Insert (the default) and Cancel; in the Create view
/// ‹ Back to search, Create and insert (the default) and Cancel. The
/// buttons' width comes from the pass before; when it changes, egui lays the
/// frame out again.
fn footer(ui: &mut egui::Ui, picker: &mut Picker) -> Option<Outcome> {
    let mut outcome = None;
    ui.add(egui::Separator::default().spacing(SEPARATOR));
    let creating = picker.mode == Mode::Create;
    let (summary, problem) = if creating {
        match crate::media_create::summary(picker) {
            Ok(text) => (text, None),
            Err(why) => (why.clone(), Some(why)),
        }
    } else {
        (picker.summary(), picker.problem())
    };
    let summary = RichText::new(summary);
    let summary = if problem.is_some() {
        summary.color(ui.visuals().warn_fg_color)
    } else {
        summary.strong()
    };
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        Layout::right_to_left(Align::Center),
        |ui| {
            ui.add(egui::Label::new(summary).truncate());
        },
    );
    let id = egui::Id::unique(if creating {
        "media-picker-create-footer-buttons"
    } else {
        "media-picker-footer-buttons"
    });
    let known = ui.data(|data| data.get_temp::<f32>(id));
    ui.horizontal(|ui| {
        if creating {
            crate::media_create::key_hints(ui);
        } else {
            key_hints(ui);
        }
        // Unmeasured, the buttons follow the keys, so they never push the
        // picker wider while egui measures them.
        ui.add_space(known.map_or(0.0, |width| (ui.available_width() - width).max(0.0)));
        let unit = picker.unit;
        let first = if creating {
            let back = ui.button("‹ Back to search");
            if back.clicked() {
                picker.back_to_search();
            }
            let label = crate::media_create::create_label(&picker.create);
            let create = crate::library_view::pane::primary(ui, &label);
            if ui.add_enabled(problem.is_none(), create).clicked() {
                outcome = picker
                    .create
                    .order(unit)
                    .map(|order| Outcome::Create(Box::new(order)));
            }
            back
        } else {
            let browse = ui.button("Browse for an image file…");
            if browse.clicked() {
                outcome = Some(Outcome::Browse(unit));
            }
            if ui.button("Create from files…").clicked() {
                picker.open_create();
            }
            let insert = crate::library_view::pane::primary(ui, "Insert");
            if ui.add_enabled(problem.is_none(), insert).clicked() {
                outcome = picker.insert();
            }
            browse
        };
        let cancel = ui.button("Cancel");
        if cancel.clicked() {
            outcome = Some(Outcome::Cancel);
        }
        let width = first.rect.union(cancel.rect).width();
        if known.is_none_or(|known| (known - width).abs() > 0.5) {
            ui.data_mut(|data| data.insert_temp(id, width));
            ui.ctx()
                .request_discard("the media picker footer measured its buttons");
        }
    });
    outcome
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn format(name: &str) -> FloppyGeometry {
        assembly::descriptors::FLOPPY_TYPES
            .iter()
            .find(|kind| kind.name == name)
            .unwrap()
            .geometry
    }
    #[allow(non_snake_case)]
    fn M12() -> FloppyGeometry {
        format("1.2M")
    }
    #[allow(non_snake_case)]
    fn M144() -> FloppyGeometry {
        format("1.44M")
    }
    #[allow(non_snake_case)]
    fn K720() -> FloppyGeometry {
        format("720K")
    }

    pub(crate) fn set(
        title: &str,
        publisher: &str,
        year: i64,
        genre: &str,
        format: FloppyGeometry,
        disks: usize,
    ) -> SetEntry {
        SetEntry {
            reference: format!("{}:1.0", title.to_lowercase().replace(' ', "-")),
            image: Descriptor::new(
                image_store::media_type::MANIFEST,
                Digest::of(title.as_bytes()),
                1,
            ),
            title: title.into(),
            publisher: publisher.into(),
            year: Some(year),
            genre: genre.into(),
            platform: "DOS".into(),
            labels: vec!["shareware".into()],
            disks: (0..disks)
                .map(|i| Disk {
                    id: format!("disk-{}", i + 1),
                    label: format!("Disk {}", i + 1),
                    format: Some(format),
                })
                .collect(),
            extras: 1,
            open: true,
            problem: None,
        }
    }

    fn library() -> Vec<SetEntry> {
        vec![
            set(
                "Commander Keen 4",
                "Apogee Software",
                1991,
                "platformer",
                M144(),
                1,
            ),
            set(
                "Duke Nukem II",
                "Apogee Software",
                1993,
                "platformer",
                M144(),
                1,
            ),
            set(
                "FreeDOS 1.3",
                "The FreeDOS Project",
                2022,
                "operating system",
                M12(),
                2,
            ),
            set("Lemmings", "Psygnosis", 1991, "puzzle", K720(), 2),
        ]
    }

    pub(crate) fn machine(id: Option<&str>, when: When) -> Target {
        Target {
            id: id.map(str::to_owned),
            name: "DOS games".into(),
            when,
            journal_unavailable: None,
            create_unavailable: None,
            drives: vec![
                Drive {
                    unit: 0,
                    geometry: M12(),
                    holds: "empty".into(),
                    catalogue: Vec::new(),
                    current: None,
                    catalogue_note: None,
                },
                Drive {
                    unit: 1,
                    geometry: M144(),
                    holds: "empty".into(),
                    catalogue: Vec::new(),
                    current: None,
                    catalogue_note: None,
                },
            ],
        }
    }

    /// A set's disk 1 with every disk, journaled: the picker's default.
    pub(crate) fn set_outcome(unit: u8) -> Outcome {
        Outcome::Set {
            unit,
            set: Descriptor::new(image_store::media_type::MANIFEST, Digest::of(b"set"), 1),
            reference: "set:1".into(),
            first: 0,
            all: true,
            journal: true,
        }
    }

    fn titles(picker: &Picker, rows: &[usize]) -> Vec<String> {
        rows.iter().map(|&i| picker.sets[i].title.clone()).collect()
    }

    /// A 1.2M drive takes 1.2M and 360K; a 1.44M drive 1.44M and 720K. The
    /// rest hide behind one line and say where they would fit.
    #[test]
    fn sets_fit_a_drive_by_its_accepted_media() {
        let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
        let results = picker.results();
        assert_eq!(titles(&picker, &results.fit), ["FreeDOS 1.3"]);
        assert_eq!(
            titles(&picker, &results.misfit),
            ["Commander Keen 4", "Duke Nukem II", "Lemmings"]
        );
        let keen = picker.sets[0].clone();
        assert_eq!(picker.other_drive(&keen).map(|d| d.unit), Some(1));
        picker.switch_drive(1);
        let results = picker.results();
        assert_eq!(
            titles(&picker, &results.fit),
            ["Commander Keen 4", "Duke Nukem II", "Lemmings"]
        );
        // The Format facet lists every format, marking the ones B: refuses.
        let formats = picker.facet_values("Format");
        assert_eq!(
            formats,
            [
                ("1.44M".to_owned(), 2, true),
                ("720K".to_owned(), 1, true),
                ("1.2M".to_owned(), 1, false),
                ("360K".to_owned(), 0, false),
            ]
        );
    }

    /// Free words match word starts and rank the title first; field:value
    /// words filter by field, years by range.
    #[test]
    fn search_takes_free_words_and_field_values() {
        let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 1, library(), None);
        let found = |picker: &mut Picker, query: &str| {
            picker.set_query(query.into());
            let results = picker.results();
            let mut all = results.fit;
            all.extend(results.misfit);
            titles(picker, &all)
        };
        assert_eq!(found(&mut picker, "keen"), ["Commander Keen 4"]);
        assert_eq!(
            found(&mut picker, "publisher:apogee"),
            ["Commander Keen 4", "Duke Nukem II"]
        );
        assert_eq!(found(&mut picker, "format:720k"), ["Lemmings"]);
        assert_eq!(
            found(&mut picker, "year:1991..1992"),
            ["Commander Keen 4", "Lemmings"]
        );
        assert_eq!(
            found(&mut picker, "genre:"),
            [
                "Commander Keen 4",
                "Duke Nukem II",
                "Lemmings",
                "FreeDOS 1.3"
            ]
        );
        // The title tier ranks before the publisher tier.
        assert_eq!(
            found(&mut picker, "apogee"),
            ["Commander Keen 4", "Duke Nukem II"]
        );
        let lemmings = &picker.sets[3];
        assert_eq!(rank(lemmings, &tokens("lemmings")), 0);
        assert_eq!(rank(lemmings, &tokens("psygnosis")), 3);
        // A facet narrows the result; its counts follow the result.
        picker.set_query(String::new());
        picker.toggle_facet("Genre", "puzzle");
        assert_eq!(titles(&picker, &picker.results().fit), ["Lemmings"]);
        assert_eq!(
            picker
                .facet_values("Genre")
                .iter()
                .find(|v| v.0 == "platformer")
                .map(|v| v.1),
            Some(2)
        );
    }

    /// Disk 1 by default, the next disk when the drive holds one of the set;
    /// the summary says when it goes in.
    #[test]
    fn the_disk_and_when_follow_the_drive() {
        let mut target = machine(Some("dos-games"), When::AtStart);
        let freedos = library()[2].image.digest.clone();
        target.drives[0].current = Some((freedos.clone(), 0));
        let mut picker = Picker::new(vec![target], 0, 0, library(), Some(&freedos));
        assert_eq!(picker.choice, Some(Choice::Set(2)));
        assert_eq!(picker.chosen_disk(), 1);
        assert_eq!(
            picker.summary(),
            "Insert FreeDOS 1.3 — Disk 2 (all 2 disks join the catalogue) into A: of DOS games — at the next Start."
        );
        picker.pick_disk(0);
        picker.all = false;
        picker.journal = false;
        assert_eq!(
            picker.insert(),
            Some(Outcome::Pending {
                id: "dos-games".into(),
                insert: image_store::machine::PendingInsert {
                    drive: "A:".into(),
                    set: picker.sets[2].image.clone(),
                    reference: picker.sets[2].reference.clone(),
                    medium: "disk-1".into(),
                    all: false,
                    journal: false,
                },
            })
        );
        // A set that does not fit is shown with its reason and cannot go in.
        let keen = picker.sets[0].image.digest.clone();
        let picker = Picker::new(
            vec![machine(None, When::Halted)],
            0,
            0,
            library(),
            Some(&keen),
        );
        assert!(picker.show_misfits);
        assert_eq!(picker.problem().as_deref(), Some("Doesn't fit A: (1.2M)"));
        assert_eq!(picker.insert(), None);
        let mut picker = picker;
        picker.switch_drive(1);
        assert!(picker
            .summary()
            .ends_with("into B: of DOS games — now; then Ctrl+Alt+Del boots from it."));
        assert!(matches!(
            picker.insert(),
            Some(Outcome::Set {
                unit: 1,
                first: 0,
                ..
            })
        ));
    }

    /// The arrow keys walk the rows that are shown; This machine lists the
    /// catalogue disks.
    #[test]
    fn arrows_walk_the_shown_rows_and_this_machine_lists_catalogues() {
        let mut target = machine(None, When::Now);
        target.drives[0].catalogue = vec![CatalogueDisk {
            name: "Blank work disk".into(),
            format: M12(),
            write: "changes kept in DOS games".into(),
            inserted: true,
            index: 3,
            set: None,
        }];
        let mut picker = Picker::new(vec![target], 0, 1, library(), None);
        assert_eq!(picker.choice, Some(Choice::Set(0)));
        picker.move_choice(1);
        assert_eq!(picker.choice, Some(Choice::Set(1)));
        picker.move_choice(5);
        assert_eq!(picker.choice, Some(Choice::Set(3)));
        picker.set_source(Source::Machine);
        assert_eq!(picker.choice, Some(Choice::Entry(0, 3)));
        assert_eq!(
            picker.problem().as_deref(),
            Some("In A:'s catalogue — switch to A: (Alt+A)")
        );
        picker.switch_drive(0);
        assert_eq!(picker.insert(), Some(Outcome::Entry { unit: 0, index: 3 }));
    }

    /// All N disks… of a drive's Catalogue submenu opens This machine on
    /// the disk in the drive: the list scrolls that row into view, where a
    /// long catalogue would otherwise leave it below the pane.
    #[test]
    fn reveal_scrolls_the_chosen_catalogue_disk_into_view() {
        let mut target = machine(None, When::Now);
        target.drives[0].catalogue = (0..18)
            .map(|index| CatalogueDisk {
                name: format!("AZURAMON {} of 18", index + 1),
                format: M12(),
                write: "changes kept in DOS games".into(),
                inserted: index == 15,
                index,
                set: None,
            })
            .collect();
        // The row lies inside the list's visible part, which clips the rows'
        // format lines; the details pane names the disk too. The list
        // scrolls over a few frames.
        let shows = |picker: &mut Picker| {
            let last = shown_frames(picker, BIG, true, false, 40)
                .pop()
                .expect("frames");
            let list = last
                .texts
                .iter()
                .find(|t| t.text == "1.2M · changes kept in DOS games")
                .expect("a row of the list")
                .clip;
            let row = last.widget(egui::accesskit::Role::ListBoxOption, "AZURAMON 16 of 18");
            list.contains_rect(row.rect)
        };
        let mut picker = Picker::new(vec![target.clone()], 0, 0, library(), None);
        picker.set_source(Source::Machine);
        picker.choose(Choice::Entry(0, 15));
        assert!(
            !shows(&mut picker),
            "without reveal the row stays below the pane"
        );
        let mut picker = Picker::new(vec![target], 0, 0, library(), None);
        picker.set_source(Source::Machine);
        picker.choose(Choice::Entry(0, 15));
        picker.reveal = true;
        assert!(shows(&mut picker));
        assert!(
            !picker.reveal,
            "the request ends once the row is scrolled to"
        );
        // Another source drops a request the list has not used yet.
        picker.reveal = true;
        picker.set_source(Source::Library);
        assert!(!picker.reveal);
    }

    /// The picker draws without a window, in every source.
    #[test]
    fn the_picker_renders_headlessly() {
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        let mut picker = Picker::new(
            vec![
                machine(Some("dos-games"), When::AtStart),
                machine(None, When::AtStart),
            ],
            0,
            0,
            library(),
            None,
        );
        picker.show_misfits = true;
        picker.remote_preview = true;
        for source in [Source::Library, Source::Machine] {
            picker.set_source(source);
            let mut outcome = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                outcome = show(ui.ctx(), &mut picker, true);
            });
            output.textures_delta.clear();
            assert_eq!(outcome, None);
        }
    }

    /// The picker keeps one size frame after frame, in both sources, with
    /// more sets than its list shows and every list part open, so its rows
    /// stay where the pointer is and Insert stays on screen.
    #[test]
    fn the_picker_keeps_its_size_frame_after_frame() {
        let mut sets = library();
        for n in 0..20 {
            sets.push(set(
                &format!("Shareware {n}"),
                "Apogee Software",
                1990 + n,
                "platformer",
                M144(),
                1,
            ));
        }
        let mut target = machine(None, When::Now);
        target.drives[1].catalogue = vec![CatalogueDisk {
            name: "Blank work disk".into(),
            format: M144(),
            write: "changes kept in DOS games".into(),
            inserted: true,
            index: 0,
            set: None,
        }];
        for source in [Source::Library, Source::Machine] {
            for classic in [true, false] {
                let mut picker = Picker::new(vec![target.clone()], 0, 1, sets.clone(), None);
                picker.show_misfits = true;
                picker.remote_preview = true;
                picker.set_source(source);
                crate::library_test_support::settled_modal(
                    egui::Id::unique("media-picker"),
                    egui::vec2(1040.0, 700.0),
                    |ctx| assert_eq!(show(ctx, &mut picker, classic), None),
                );
            }
        }
    }

    /// A disk of the other drive's catalogue is refused with where it
    /// belongs, never offered as an Insert that does nothing.
    #[test]
    fn a_catalogue_disk_of_the_other_drive_says_to_switch() {
        let mut target = machine(None, When::Now);
        target.drives[1].geometry = M12();
        target.drives[0].catalogue = vec![CatalogueDisk {
            name: "Work disk".into(),
            format: M12(),
            write: "write-protected".into(),
            inserted: false,
            index: 2,
            set: None,
        }];
        let mut picker = Picker::new(vec![target], 0, 1, library(), None);
        picker.set_source(Source::Machine);
        assert_eq!(picker.choice, Some(Choice::Entry(0, 2)));
        assert_eq!(
            picker.problem().as_deref(),
            Some("In A:'s catalogue — switch to A: (Alt+A)")
        );
        assert_eq!(picker.insert(), None);
        assert!(picker.summary().starts_with("In A:'s catalogue"));
        picker.switch_drive(0);
        assert_eq!(picker.insert(), Some(Outcome::Entry { unit: 0, index: 2 }));
    }

    /// A target that cannot keep writes offers the write-protected setting
    /// only, says so, and inserts write-protected.
    #[test]
    fn a_target_without_a_journal_inserts_write_protected_and_says_so() {
        let mut target = machine(None, When::AtStart);
        target.journal_unavailable = Some("Before Start, write-protected.".into());
        let freedos = library()[2].image.digest.clone();
        let picker = Picker::new(vec![target], 0, 0, library(), Some(&freedos));
        assert!(!picker.journal);
        assert!(
            picker.summary().contains(", write-protected, into A:"),
            "{}",
            picker.summary()
        );
        assert!(matches!(
            picker.insert(),
            Some(Outcome::Set { journal: false, .. })
        ));
    }

    fn frame(
        ctx: &egui::Context,
        picker: &mut Picker,
        events: Vec<egui::Event>,
    ) -> Option<Outcome> {
        let mut outcome = None;
        for events in [Vec::new(), events] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| outcome = show(ui.ctx(), picker, true).or(outcome.take()),
            );
            output.textures_delta.clear();
        }
        outcome
    }

    /// Alt+B switches the drive without typing into the search field (the
    /// windowing layer also sends the chord's character as text); Enter in
    /// the search field inserts; plain typing searches.
    #[test]
    fn the_picker_keys_switch_drives_insert_and_search() {
        let key = |key: egui::Key, modifiers: egui::Modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
        let outcome = frame(
            &ctx,
            &mut picker,
            vec![
                key(egui::Key::B, egui::Modifiers::ALT),
                egui::Event::Text("∫".into()),
            ],
        );
        assert_eq!(outcome, None);
        assert_eq!(picker.unit, 1);
        assert_eq!(picker.query, "");
        let outcome = frame(&ctx, &mut picker, vec![egui::Event::Text("le".into())]);
        assert_eq!(outcome, None);
        assert_eq!(picker.query, "le");
        assert_eq!(
            picker
                .choice
                .map(|c| match c {
                    Choice::Set(i) => picker.sets[i].title.clone(),
                    Choice::Entry(..) => String::new(),
                })
                .as_deref(),
            Some("Lemmings")
        );
        let outcome = frame(
            &ctx,
            &mut picker,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(
            matches!(outcome, Some(Outcome::Set { unit: 1, .. })),
            "{outcome:?}"
        );
    }

    /// A widget as egui reported it to AccessKit.
    struct Widget {
        role: egui::accesskit::Role,
        label: String,
        rect: egui::Rect,
    }

    /// A painted text: where its glyphs are, the clip it was painted under,
    /// and its first section's font size and colour.
    struct Text {
        text: String,
        /// Each row's characters, as the galley broke them.
        rows: Vec<String>,
        glyphs: egui::Rect,
        clip: egui::Rect,
        size: f32,
        monospace: bool,
        colour: egui::Color32,
    }

    /// What one frame of the picker showed: the modal's area, its widgets,
    /// and every painted text and rectangle with the clip it was painted under.
    struct Shown {
        modal: egui::Rect,
        widgets: Vec<Widget>,
        texts: Vec<Text>,
        rects: Vec<(egui::epaint::RectShape, egui::Rect)>,
    }

    impl Shown {
        fn of(output: &egui::FullOutput, modal: egui::Rect) -> Self {
            let widgets = output
                .platform_output
                .accesskit_update
                .as_ref()
                .map(|update| {
                    update
                        .nodes
                        .iter()
                        .filter_map(|(_, node)| {
                            let area = node.bounds()?;
                            Some(Widget {
                                role: node.role(),
                                label: node.label().or(node.value()).unwrap_or_default().to_owned(),
                                rect: egui::Rect::from_min_max(
                                    egui::pos2(area.x0 as f32, area.y0 as f32),
                                    egui::pos2(area.x1 as f32, area.y1 as f32),
                                ),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            fn walk(shape: &egui::Shape, clip: egui::Rect, shown: &mut Shown) {
                match shape {
                    egui::Shape::Text(text) => {
                        let glyphs = text
                            .galley
                            .rows
                            .iter()
                            .filter(|row| !row.glyphs.is_empty())
                            .map(|row| row.rect_without_leading_space())
                            .reduce(|a, b| a.union(b))
                            .unwrap_or(text.galley.rect)
                            .translate(text.pos.to_vec2());
                        let format = text.galley.job.sections.first().map(|s| &s.format);
                        shown.texts.push(Text {
                            text: text.galley.text().to_owned(),
                            rows: text
                                .galley
                                .rows
                                .iter()
                                .map(|row| row.glyphs.iter().map(|g| g.chr).collect())
                                .collect(),
                            glyphs,
                            clip,
                            size: format.map_or(0.0, |f| f.font_id.size),
                            monospace: format
                                .is_some_and(|f| f.font_id.family == egui::FontFamily::Monospace),
                            colour: format.map_or(egui::Color32::TRANSPARENT, |f| f.color),
                        });
                    }
                    egui::Shape::Rect(rect) => shown.rects.push((rect.clone(), clip)),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, clip, shown);
                        }
                    }
                    _ => (),
                }
            }
            let mut shown = Self {
                modal,
                widgets,
                texts: Vec::new(),
                rects: Vec::new(),
            };
            for clipped in &output.shapes {
                walk(&clipped.shape, clipped.clip_rect, &mut shown);
            }
            shown
        }
        fn widgets(&self, role: egui::accesskit::Role) -> impl Iterator<Item = &Widget> {
            self.widgets.iter().filter(move |w| w.role == role)
        }
        fn widget(&self, role: egui::accesskit::Role, label: &str) -> &Widget {
            self.widgets(role)
                .find(|w| w.label == label)
                .unwrap_or_else(|| {
                    let labels: Vec<_> = self.widgets(role).map(|w| &w.label).collect();
                    panic!("no {role:?} {label:?} in {labels:?}")
                })
        }
    }

    /// The picker in a window `size` large, in one skin and theme, for
    /// `count` frames, each as it was shown.
    fn shown_frames(
        picker: &mut Picker,
        size: egui::Vec2,
        classic: bool,
        dark: bool,
        count: usize,
    ) -> Vec<Shown> {
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        ctx.set_visuals(ui_kit::skin::skin_visuals(classic, dark));
        ctx.enable_accesskit();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        (0..count)
            .map(|_| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(viewport),
                        ..Default::default()
                    },
                    |ui| assert_eq!(show(ui.ctx(), picker, classic), None),
                );
                output.textures_delta.clear();
                let modal = ctx
                    .memory(|m| m.area_rect(egui::Id::unique("media-picker")))
                    .expect("the picker is shown");
                Shown::of(&output, modal)
            })
            .collect()
    }

    /// The picker as its fifth frame shows it.
    fn shown(picker: &mut Picker, size: egui::Vec2, classic: bool, dark: bool) -> Shown {
        shown_frames(picker, size, classic, dark, 5)
            .pop()
            .expect("frames")
    }

    /// Every skin and theme: classic and modern, light and dark.
    const LOOKS: [(bool, bool); 4] = [(true, false), (true, true), (false, false), (false, true)];

    /// A big window, where the picker takes the mock-up's size.
    const BIG: egui::Vec2 = egui::vec2(1600.0, 1000.0);

    /// In a big window the picker is the mock-up's 1040 wide and about 640
    /// tall, with the search field across it; in a smaller one it stays
    /// inside the window's content area, with room around it.
    #[test]
    fn the_picker_takes_the_mock_ups_size_inside_the_window() {
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
            let big = shown(&mut picker, BIG, classic, dark);
            assert!(
                (big.modal.width() - 1040.0).abs() <= 1.0,
                "{what}: {:?}",
                big.modal
            );
            assert!(
                (big.modal.height() - 640.0).abs() <= 24.0,
                "{what}: {:?}",
                big.modal
            );
            let search = big
                .widgets(egui::accesskit::Role::TextInput)
                .next()
                .expect("the search field");
            assert!(
                search.rect.width() >= big.modal.width() - 40.0,
                "{what}: the search field is {:?} in {:?}",
                search.rect,
                big.modal
            );
            for size in [egui::vec2(900.0, 600.0), egui::vec2(700.0, 460.0)] {
                let small = shown(&mut picker, size, classic, dark);
                let area = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                assert!(
                    area.shrink(8.0).contains_rect(small.modal),
                    "{what}: {:?} in a {size:?} window",
                    small.modal
                );
            }
        }
    }

    /// The facets, results and details panes sit on the field colour and
    /// share one fixed height, the same frame after frame from the first
    /// one shown: nothing grows. The modal fades in over its first frames,
    /// so the colour is checked once it has.
    #[test]
    fn the_body_panes_keep_one_height_on_the_field() {
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let field = ui_kit::skin::skin_visuals(classic, dark).text_edit_bg_color();
            let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
            picker.show_misfits = true;
            let frames = shown_frames(&mut picker, BIG, classic, dark, 12);
            let panes = |frame: &Shown| -> Vec<(egui::Rect, egui::Color32)> {
                frame
                    .rects
                    .iter()
                    .filter(|(r, _)| {
                        r.fill.a() > 0
                            && r.rect.height() > 200.0
                            && r.rect.width() < frame.modal.width() - 40.0
                    })
                    .map(|(r, _)| (r.rect, r.fill))
                    .collect()
            };
            let mut heights = Vec::new();
            for frame in &frames[1..] {
                let panes = panes(frame);
                assert_eq!(panes.len(), 3, "{what}: {panes:?}");
                assert!(
                    panes.iter().all(|(p, _)| p.height() == panes[0].0.height()),
                    "{what}: {panes:?}"
                );
                heights.push(panes[0].0.height());
            }
            assert!(
                heights.iter().all(|h| *h == heights[0]),
                "{what}: {heights:?}"
            );
            assert!(heights[0] >= 440.0, "{what}: {heights:?}");
            let last = panes(frames.last().expect("frames"));
            assert!(
                last.iter().all(|(_, fill)| *fill == field),
                "{what}: {last:?} on {field:?}"
            );
        }
    }

    /// Every facet value is one line: a check box with the value, the
    /// count right-aligned beside it; formats the drive does not take say
    /// so. The facets are flat sections under uppercase headings.
    #[test]
    fn facet_values_are_single_lines_with_a_check_box() {
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
            let shown = shown(&mut picker, BIG, classic, dark);
            for heading in ["FORMAT", "PLATFORM", "GENRE", "YEAR", "PUBLISHER"] {
                assert!(
                    shown.texts.iter().any(|t| t.text == heading),
                    "{what}: {heading}"
                );
            }
            for name in ["Format", "Platform", "Genre", "Year", "Publisher"] {
                for (value, count, fits) in picker.facet_values(name) {
                    let label = if fits {
                        value.clone()
                    } else {
                        format!("{value} · doesn't fit A:")
                    };
                    let check = shown.widget(egui::accesskit::Role::CheckBox, &label);
                    assert!(
                        check.rect.height() <= 22.0,
                        "{what}: {label} is {:?}",
                        check.rect
                    );
                    let count = count.to_string();
                    assert!(
                        shown.texts.iter().any(|t| t.text == count
                            && (t.glyphs.center().y - check.rect.center().y).abs() < 3.0
                            && t.glyphs.left() > check.rect.left() + 100.0),
                        "{what}: no count {count} beside {label} at {:?}",
                        check.rect
                    );
                }
            }
        }
    }

    /// A result row's cover placeholder keeps its text inside the cover: a
    /// long title is clipped, never painted over the row, and its lines
    /// break between words, never inside one.
    #[test]
    fn a_result_cover_clips_its_text_inside_the_cover() {
        let mut sets = library();
        sets.push(set(
            "Supercalifragilistic Expedition",
            "Apogee Software",
            1992,
            "platformer",
            M12(),
            1,
        ));
        let words: BTreeSet<&str> = sets
            .iter()
            .flat_map(|s| s.title.split_whitespace())
            .collect();
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, sets.clone(), None);
            picker.show_misfits = true;
            let shown = shown(&mut picker, BIG, classic, dark);
            let covers: Vec<&Text> = shown
                .texts
                .iter()
                .filter(|t| {
                    t.monospace
                        && t.size <= 10.5
                        && t.colour == egui::Color32::WHITE
                        && !t.text.is_empty()
                })
                .collect();
            for cover in &covers {
                assert!(
                    cover.clip.width() <= 36.5 && cover.clip.height() <= 48.5,
                    "{what}: {:?} painted under {:?}",
                    cover.text,
                    cover.clip
                );
                for row in &cover.rows {
                    assert!(
                        row.split_whitespace().all(|word| words.contains(word)),
                        "{what}: {:?} breaks a word in {:?}",
                        row,
                        cover.rows
                    );
                }
            }
            assert!(covers.len() >= 6, "{what}: {} cover lines", covers.len());
            // A word wider than the cover keeps its line, cut by the clip.
            assert!(
                covers.iter().any(|t| t.text == "Supercalifragilistic"),
                "{what}"
            );
        }
    }

    /// The disks to insert are compact radios in one column, one line each,
    /// named by their label without the file name; with two disks every
    /// part of the details pane shows without scrolling.
    #[test]
    fn disk_radios_share_one_column_one_line_each() {
        let mut floppy = set(
            "FreeDOS 1.4 (Floppy Edition, 1.2M)",
            "The FreeDOS Project",
            2025,
            "operating system",
            M12(),
            8,
        );
        for (i, disk) in floppy.disks.iter_mut().enumerate() {
            disk.label = if i == 0 {
                "Disk 1 of 8 - boot disk and installer (x86BOOT.img)".into()
            } else {
                format!("Disk {} of 8 - packages (x86DSK0{i}.img)", i + 1)
            };
        }
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let digest = floppy.image.digest.clone();
            let mut picker = Picker::new(
                vec![machine(None, When::Now)],
                0,
                0,
                vec![floppy.clone()],
                Some(&digest),
            );
            let eight = shown(&mut picker, BIG, classic, dark);
            let mut radios: Vec<&Widget> = eight
                .widgets(egui::accesskit::Role::RadioButton)
                .filter(|w| w.label.starts_with(|c: char| c.is_ascii_digit()))
                .collect();
            radios.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
            assert_eq!(radios.len(), 8, "{what}");
            assert!(
                radios[0].label.starts_with("1. Boot disk and installer"),
                "{what}: {}",
                radios[0].label
            );
            for radio in &radios {
                assert!(
                    (radio.rect.left() - radios[0].rect.left()).abs() < 0.5,
                    "{what}: {} at {:?}",
                    radio.label,
                    radio.rect
                );
                assert!(radio.rect.height() <= 22.0, "{what}: {:?}", radio.rect);
                assert!(!radio.label.contains(".img"), "{what}: {}", radio.label);
            }
            // The summary names the disk as its radio button does.
            assert_eq!(
                picker.summary(),
                "Insert FreeDOS 1.4 (Floppy Edition, 1.2M) — Boot disk and installer (all 8 disks join the catalogue) into A: of DOS games — now.",
                "{what}"
            );
            // Two disks: the write setting and the extras line show unscrolled.
            let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
            let two = shown(&mut picker, BIG, classic, dark);
            let details = two
                .rects
                .iter()
                .filter(|(r, _)| {
                    r.fill.a() > 0
                        && r.rect.height() > 200.0
                        && r.rect.width() < two.modal.width() - 40.0
                })
                .map(|(r, _)| r.rect)
                .max_by(|a, b| a.left().total_cmp(&b.left()))
                .expect("the details pane");
            for label in ["Write-protected", "Keep the changes in DOS games"] {
                let radio = two.widget(egui::accesskit::Role::RadioButton, label);
                assert!(
                    details.shrink(2.0).contains_rect(radio.rect),
                    "{what}: {label} at {:?} outside the details pane {details:?}",
                    radio.rect
                );
            }
            let extras = two
                .texts
                .iter()
                .find(|t| t.text.starts_with("Extras stay in the library"))
                .unwrap_or_else(|| panic!("{what}: no extras line"));
            assert!(
                extras.clip.contains_rect(extras.glyphs),
                "{what}: the extras line is cut: {:?} under {:?}",
                extras.glyphs,
                extras.clip
            );
        }
    }

    /// The footer: the summary right-aligned on one line, then framed
    /// buttons at the right, Browse for an image file…, Insert and Cancel.
    #[test]
    fn the_footer_buttons_are_framed_buttons_at_the_right() {
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
            let shown = shown(&mut picker, BIG, classic, dark);
            let [browse, insert, cancel] = ["Browse for an image file…", "Insert", "Cancel"]
                .map(|label| shown.widget(egui::accesskit::Role::Button, label).rect);
            assert!(
                browse.right() < insert.left() && insert.right() < cancel.left(),
                "{what}: {browse:?} {insert:?} {cancel:?}"
            );
            for rect in [browse, insert] {
                assert!((rect.center().y - cancel.center().y).abs() < 1.0, "{what}");
            }
            assert!(browse.left() > shown.modal.center().x, "{what}: {browse:?}");
            assert!(
                shown.modal.right() - cancel.right() < 24.0,
                "{what}: {cancel:?}"
            );
            let window = ui_kit::skin::skin_visuals(classic, dark).window_fill;
            for (label, rect) in [("Browse", browse), ("Insert", insert), ("Cancel", cancel)] {
                assert!(
                    shown.rects.iter().any(|(r, _)| {
                        (r.rect.min - rect.min).length() <= 2.0
                            && (r.rect.max - rect.max).length() <= 2.0
                            && ((r.stroke.width >= 1.0 && r.stroke.color.a() > 0)
                                || (r.fill != window && r.fill.a() > 0))
                    }),
                    "{what}: {label} has no frame at {rect:?}"
                );
            }
            let summary = shown
                .texts
                .iter()
                .find(|t| t.text.starts_with("Insert FreeDOS 1.3"))
                .unwrap_or_else(|| panic!("{what}: no summary"));
            assert!(
                summary.glyphs.height() <= 20.0,
                "{what}: {:?}",
                summary.glyphs
            );
            assert!(
                (summary.glyphs.right() - cancel.right()).abs() <= 6.0,
                "{what}: the summary ends at {:?}, Cancel at {cancel:?}",
                summary.glyphs
            );
            assert!(summary.glyphs.bottom() <= cancel.top(), "{what}");
        }
    }

    /// When the Into row wraps, for a long machine name or a drive holding a
    /// disk with a long name, the body gives up the room: the footer stays
    /// inside the picker. What a drive holds is cut to one line.
    #[test]
    fn the_footer_stays_inside_when_the_top_rows_wrap() {
        for (classic, dark) in LOOKS {
            for (name, holds) in [
                (
                    "Open AT 286 for games",
                    "Commander Keen 4: Secret of the Oracle — Disk 1 (shareware 1.4)",
                ),
                (
                    "Open AT 286 for games with a long name",
                    "Keen 4 — Shareware v1.4 installation",
                ),
            ] {
                for size in [BIG, egui::vec2(1040.0, 700.0)] {
                    let what = format!("classic {classic}, dark {dark}, {name:?} at {size:?}");
                    let mut target = machine(None, When::Halted);
                    target.name = name.into();
                    target.drives[1].holds = holds.into();
                    let mut picker = Picker::new(vec![target], 0, 0, library(), None);
                    let shown = shown(&mut picker, size, classic, dark);
                    let cancel = shown.widget(egui::accesskit::Role::Button, "Cancel").rect;
                    assert!(
                        shown.modal.contains_rect(cancel),
                        "{what}: Cancel at {cancel:?} in {:?}",
                        shown.modal
                    );
                    let area = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                    assert!(area.contains_rect(shown.modal), "{what}: {:?}", shown.modal);
                    let b = shown
                        .widgets(egui::accesskit::Role::Button)
                        .find(|w| w.label.starts_with("B:"))
                        .expect("the B: toggle");
                    assert!(b.rect.height() <= 22.0, "{what}: B: is {:?}", b.rect);
                    assert!(
                        b.rect.width() <= SEGMENT_WIDTH + 20.0,
                        "{what}: B: is {:?}",
                        b.rect
                    );
                }
            }
        }
    }

    /// A disk label loses only a real file name at its end; a disk's name
    /// drops a "Disk N of M -" that repeats its number and capitalises a
    /// first word in lower case only; an edition is the outermost trailing
    /// parenthesis.
    #[test]
    fn labels_lose_only_a_file_name_and_a_repeated_number() {
        assert_eq!(
            label_without_file("Disk 1 of 8 - boot disk and installer (x86BOOT.img)"),
            ("Disk 1 of 8 - boot disk and installer", Some("x86BOOT.img"))
        );
        assert_eq!(
            label_without_file("Docs (README.TXT)"),
            ("Docs", Some("README.TXT"))
        );
        for kept in [
            "Boot disk (1.2M)",
            "Install (v2.0)",
            "Tools (.img)",
            "Setup (disk.image1)",
            "Data (my disk.img)",
            "Plain",
        ] {
            assert_eq!(label_without_file(kept), (kept, None), "{kept}");
        }
        assert_eq!(
            disk_name("Disk 1 of 8 - boot disk and installer (x86BOOT.img)", 1),
            "Boot disk and installer"
        );
        assert_eq!(
            disk_name("Disk 3 of 8 - packages (x86DSK02.img)", 3),
            "Packages"
        );
        assert_eq!(disk_name("Disk 2 of 3: x86 tools", 2), "x86 tools");
        assert_eq!(disk_name("Disk 2 of 3 - dBase files", 2), "dBase files");
        assert_eq!(disk_name("Disk 2 of 3 - über disk", 2), "über disk");
        assert_eq!(disk_name("Disk 2", 2), "Disk 2");
        assert_eq!(disk_name("Disk 2 of 3 - tools", 1), "Disk 2 of 3 - tools");
        assert_eq!(disk_name("Boot disk (1.2M)", 1), "Boot disk (1.2M)");
        assert_eq!(
            title_parts("FreeDOS 1.4 (Floppy Edition, 1.2M)"),
            ("FreeDOS 1.4", "(Floppy Edition, 1.2M)")
        );
        assert_eq!(title_parts("Foo (a (b))"), ("Foo", "(a (b))"));
        assert_eq!(title_parts("Foo(bar)"), ("Foo(bar)", ""));
        assert_eq!(title_parts("(bar)"), ("(bar)", ""));
        assert_eq!(title_parts("Commander Keen 4"), ("Commander Keen 4", ""));
    }

    /// One context driven by a pointer, frame by frame, in one skin and theme.
    struct Pointer {
        ctx: egui::Context,
        time: f64,
        classic: bool,
    }

    impl Pointer {
        fn new(classic: bool, dark: bool) -> Self {
            let ctx = egui::Context::default();
            ui_kit::skin::configure_context(&ctx);
            ctx.set_visuals(ui_kit::skin::skin_visuals(classic, dark));
            ctx.enable_accesskit();
            Self {
                ctx,
                time: 0.0,
                classic,
            }
        }
        fn frame(
            &mut self,
            picker: &mut Picker,
            events: Vec<egui::Event>,
        ) -> (Option<Outcome>, Shown) {
            self.time += 1.0 / 60.0;
            let mut outcome = None;
            let classic = self.classic;
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, BIG)),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                },
                |ui| outcome = show(ui.ctx(), picker, classic),
            );
            output.textures_delta.clear();
            let modal = self
                .ctx
                .memory(|m| m.area_rect(egui::Id::unique("media-picker")))
                .expect("the picker is shown");
            (outcome, Shown::of(&output, modal))
        }
        /// Clicks `count` times on the widget `label` of `role`, as the
        /// picker shows it once settled, and moves the pointer away;
        /// returns what the picker asked for meanwhile.
        fn click(
            &mut self,
            picker: &mut Picker,
            role: egui::accesskit::Role,
            label: &str,
            count: usize,
        ) -> Option<Outcome> {
            let mut shown = None;
            for _ in 0..5 {
                shown = Some(self.frame(picker, Vec::new()).1);
            }
            let at = shown.expect("frames").widget(role, label).rect.center();
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.time += 1.0;
            let mut outcome = self.frame(picker, vec![egui::Event::PointerMoved(at)]).0;
            for _ in 0..count {
                for pressed in [true, false] {
                    outcome = outcome.or(self.frame(picker, vec![button(pressed)]).0);
                }
            }
            let away = egui::Event::PointerMoved(egui::pos2(1.0, 1.0));
            outcome.or(self.frame(picker, vec![away]).0)
        }
    }

    /// Every control of the picker answers a click, in every skin and
    /// theme: rows (a double-click inserts), facet boxes and their chips,
    /// Show, the switch link of a set that fits the other drive, the disk
    /// radios, the catalogue box, the write radios, the drive toggles, the
    /// sources, the remote preview link, Insert, Browse… and Cancel.
    #[test]
    fn every_control_answers_a_click() {
        use egui::accesskit::Role;
        let on_b = || Picker::new(vec![machine(None, When::Now)], 0, 1, library(), None);
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let mut run = Pointer::new(classic, dark);
            let mut picker = on_b();
            assert_eq!(
                run.click(&mut picker, Role::ListBoxOption, "Duke Nukem II", 1),
                None
            );
            assert_eq!(picker.choice, Some(Choice::Set(1)), "{what}");
            let outcome = run.click(&mut picker, Role::ListBoxOption, "Commander Keen 4", 2);
            assert!(
                matches!(outcome, Some(Outcome::Set { unit: 1, .. })),
                "{what}: {outcome:?}"
            );
            let mut picker = on_b();
            run.click(&mut picker, Role::CheckBox, "platformer", 1);
            assert!(
                picker
                    .facets
                    .get("Genre")
                    .is_some_and(|v| v.contains("platformer")),
                "{what}"
            );
            run.click(&mut picker, Role::Button, "Genre: platformer ×", 1);
            assert!(
                picker.facets.get("Genre").is_none_or(|v| v.is_empty()),
                "{what}"
            );
            run.click(&mut picker, Role::Link, "Show", 1);
            assert!(picker.show_misfits, "{what}");
            run.click(&mut picker, Role::Link, "switch to A: or Alt+A", 1);
            assert_eq!(picker.unit, 0, "{what}");
            run.click(&mut picker, Role::ListBoxOption, "FreeDOS 1.3", 1);
            run.click(&mut picker, Role::RadioButton, "2. Disk 2 · 1.2M", 1);
            assert_eq!(picker.chosen_disk(), 1, "{what}");
            run.click(
                &mut picker,
                Role::CheckBox,
                "Put all 2 disks in A:'s catalogue, in order",
                1,
            );
            assert!(!picker.all, "{what}");
            run.click(&mut picker, Role::RadioButton, "Write-protected", 1);
            assert!(!picker.journal, "{what}");
            run.click(
                &mut picker,
                Role::RadioButton,
                "Keep the changes in DOS games",
                1,
            );
            assert!(picker.journal, "{what}");
            run.click(&mut picker, Role::Button, "B: 1.44M · empty", 1);
            assert_eq!(picker.unit, 1, "{what}");
            run.click(&mut picker, Role::Button, "A: 1.2M · empty", 1);
            assert_eq!(picker.unit, 0, "{what}");
            // Switching the drive went back to the default disk.
            assert_eq!(picker.chosen_disk(), 0, "{what}");
            run.click(&mut picker, Role::RadioButton, "2. Disk 2 · 1.2M", 1);
            let outcome = run.click(&mut picker, Role::Button, "Insert", 1);
            assert!(
                matches!(
                    outcome,
                    Some(Outcome::Set {
                        unit: 0,
                        first: 1,
                        all: false,
                        journal: true,
                        ..
                    })
                ),
                "{what}: {outcome:?}"
            );
            let outcome = run.click(&mut picker, Role::Button, "Browse for an image file…", 1);
            assert_eq!(outcome, Some(Outcome::Browse(0)), "{what}");
            let outcome = run.click(&mut picker, Role::Button, "Cancel", 1);
            assert_eq!(outcome, Some(Outcome::Cancel), "{what}");
            run.click(&mut picker, Role::Button, "This machine (0)", 1);
            assert_eq!(picker.source, Source::Machine, "{what}");
            run.click(&mut picker, Role::Button, "This library (4)", 1);
            assert_eq!(picker.source, Source::Library, "{what}");
            run.click(
                &mut picker,
                Role::Link,
                "What will a remote result look like?",
                1,
            );
            assert!(picker.remote_preview, "{what}");
        }
    }

    /// The picker on B: of a running DOS games with `paths` in its Create
    /// view, read.
    fn creating(paths: &[std::path::PathBuf]) -> Picker {
        let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 1, library(), None);
        picker.open_create();
        picker.create = crate::media_create::tests::listed(paths);
        picker
    }

    /// Create from files… in the footer turns the picker into its Create
    /// view on the same drive; ‹ Back to search returns with the search and
    /// the added items kept.
    #[test]
    fn create_from_files_opens_the_create_view_and_back_keeps_both() {
        use egui::accesskit::Role;
        let temp =
            crate::media_create::tests::host("picker-create-open", &[("game/play.exe", 900)]);
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let mut run = Pointer::new(classic, dark);
            let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 1, library(), None);
            picker.set_query("keen".into());
            assert_eq!(
                run.click(&mut picker, Role::Button, "Create from files…", 1),
                None
            );
            assert_eq!(picker.mode, Mode::Create, "{what}");
            assert_eq!(picker.unit, 1, "{what}");
            let (_, shown) = run.frame(&mut picker, Vec::new());
            assert!(
                shown
                    .texts
                    .iter()
                    .any(|t| t.text == "Create disk from files — DOS games"),
                "{what}"
            );
            assert!(shown
                .texts
                .iter()
                .any(|t| t.text == "Files and folders on this computer"));
            assert!(
                shown.widgets(Role::TextInput).all(|w| w.label != "keen"),
                "{what}: no search field"
            );
            picker.create.add([temp.path().join("game")]);
            run.click(&mut picker, Role::Button, "‹ Back to search", 1);
            assert_eq!(picker.mode, Mode::Search, "{what}");
            assert_eq!(picker.query, "keen", "{what}");
            assert_eq!(picker.create.items.len(), 1, "{what}");
        }
    }

    /// The Create view opens with Add folder… focused, as the mock-up's
    /// does: Enter asks for the folder dialog.
    #[test]
    fn the_create_view_opens_with_add_folder_focused() {
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 1, library(), None);
        picker.open_create();
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(
            frame(&ctx, &mut picker, vec![enter]),
            Some(Outcome::AddFolder)
        );
    }

    /// The Create view keeps the picker's size frame after frame, empty,
    /// with a long list and with a split, its two panes sharing the body's
    /// fixed height.
    #[test]
    fn the_create_view_keeps_the_pickers_size_and_body_height() {
        let mut files: Vec<(String, usize)> = (0..120)
            .map(|n| (format!("azuramon/gfx/tile{n:03}.pcx"), 30_000))
            .collect();
        files.push(("azuramon/Azuramon Manual.txt".into(), 48_120));
        files.push(("azuramon/.DS_Store".into(), 6_148));
        let files: Vec<(&str, usize)> = files.iter().map(|(p, n)| (p.as_str(), *n)).collect();
        let temp = crate::media_create::tests::host("picker-create-size", &files);
        for paths in [Vec::new(), vec![temp.path().join("azuramon")]] {
            for (classic, dark) in LOOKS {
                let what = format!("classic {classic}, dark {dark}, {} items", paths.len());
                let mut picker = creating(&paths);
                let search = {
                    let mut search =
                        Picker::new(vec![machine(None, When::Now)], 0, 1, library(), None);
                    shown(&mut search, BIG, classic, dark).modal
                };
                let frames = shown_frames(&mut picker, BIG, classic, dark, 12);
                let panes = |frame: &Shown| -> Vec<egui::Rect> {
                    frame
                        .rects
                        .iter()
                        .filter(|(r, _)| {
                            r.fill.a() > 0
                                && r.rect.height() > 200.0
                                && r.rect.width() < frame.modal.width() - 40.0
                        })
                        .map(|(r, _)| r.rect)
                        .collect()
                };
                let mut heights = Vec::new();
                for frame in &frames[1..] {
                    assert_eq!(
                        frame.modal, frames[1].modal,
                        "{what}: the picker moved or grew"
                    );
                    let panes = panes(frame);
                    assert_eq!(panes.len(), 2, "{what}: {panes:?}");
                    assert!(
                        panes.iter().all(|p| p.height() == panes[0].height()),
                        "{what}"
                    );
                    heights.push(panes[0].height());
                }
                assert!(
                    heights.iter().all(|h| *h == heights[0]),
                    "{what}: {heights:?}"
                );
                assert!(heights[0] >= 440.0, "{what}: {heights:?}");
                let modal = frames.last().unwrap().modal;
                assert!(
                    (modal.width() - search.width()).abs() <= 1.0,
                    "{what}: {modal:?} {search:?}"
                );
                assert!(
                    (modal.height() - search.height()).abs() <= 1.0,
                    "{what}: {modal:?} {search:?}"
                );
                let cancel = frames
                    .last()
                    .unwrap()
                    .widget(egui::accesskit::Role::Button, "Cancel")
                    .rect;
                assert!(modal.contains_rect(cancel), "{what}");
            }
        }
    }

    /// The footer says what Create and insert makes and where it goes, or
    /// why it cannot run; a split names its disks on the button.
    #[test]
    fn the_create_footer_says_what_goes_where() {
        let temp = crate::media_create::tests::host(
            "picker-create-footer",
            &[
                ("small/a.exe", 1000),
                ("small/b.dat", 1000),
                ("galaxia/a.dat", 900_000),
                ("galaxia/b.dat", 900_000),
                ("big/whole.bin", 1_500_000),
            ],
        );
        let mut picker = creating(&[temp.path().join("small")]);
        let frame = shown(&mut picker, BIG, true, false);
        assert_eq!(
            crate::media_create::summary(&picker).unwrap(),
            "Create a 1.44M disk from 2 files and insert it into B: of DOS games — now."
        );
        frame.widget(egui::accesskit::Role::Button, "Create and insert");
        assert!(frame.texts.iter().any(|t| t.text == "Fits one 1.44M disk."));
        let mut picker = creating(&[temp.path().join("galaxia")]);
        let frame = shown(&mut picker, BIG, true, false);
        assert_eq!(
            crate::media_create::summary(&picker).unwrap(),
            "Create 2 disks of 1.44M from 2 files and insert disk 1 into B: of DOS games — now. All 2 join B:'s catalogue."
        );
        frame.widget(egui::accesskit::Role::Button, "Create 2 disks and insert");
        frame.widget(
            egui::accesskit::Role::RadioButton,
            "Split across 2 disks of 1.44M",
        );
        assert!(frame
            .texts
            .iter()
            .any(|t| t.text == "XCOPY B:\\GALAXIA C:\\GALAXIA /S /E"));
        let mut picker = creating(&[temp.path().join("big")]);
        shown(&mut picker, BIG, true, false);
        assert_eq!(
            crate::media_create::summary(&picker).unwrap_err(),
            "WHOLE.BIN is too large for any 1.44M disk."
        );
    }

    /// Every control of the Create view answers a click, in every skin and
    /// theme: Add files…, Add folder…, ×, the formats and the larger-format
    /// link, the write radios, keeping in the library, Create and insert and
    /// Cancel; a file too large for any disk leaves Create doing nothing.
    #[test]
    fn every_create_control_answers_a_click() {
        use egui::accesskit::Role;
        let temp = crate::media_create::tests::host(
            "picker-create-clicks",
            &[
                ("nightfall/nightfal.exe", 400_000),
                ("nightfall/intro.fli", 500_000),
                ("loose.txt", 10),
                ("big/whole.bin", 1_500_000),
            ],
        );
        let folder = temp.path().join("nightfall");
        for (classic, dark) in LOOKS {
            let what = format!("classic {classic}, dark {dark}");
            let mut run = Pointer::new(classic, dark);
            let mut picker = creating(&[folder.clone(), temp.path().join("loose.txt")]);
            assert_eq!(
                run.click(&mut picker, Role::Button, "Add files…", 1),
                Some(Outcome::AddFiles),
                "{what}"
            );
            assert_eq!(
                run.click(&mut picker, Role::Button, "Add folder…", 1),
                Some(Outcome::AddFolder),
                "{what}"
            );
            run.click(&mut picker, Role::Button, "Remove loose.txt", 1);
            assert_eq!(picker.create.items.len(), 1, "{what}");
            assert_eq!(
                picker.create.top, "NIGHTFAL",
                "{what}: a folder alone is the top folder"
            );
            run.click(&mut picker, Role::Button, "720K 730,112 bytes free", 1);
            assert_eq!(
                picker.create.format,
                Some(media::fat12::FloppyFormat::Dd720),
                "{what}"
            );
            run.click(&mut picker, Role::Link, "use 1.44M", 1);
            assert_eq!(
                picker.create.format,
                Some(media::fat12::FloppyFormat::Hd144),
                "{what}"
            );
            run.click(&mut picker, Role::RadioButton, "Write-protected", 1);
            assert!(!picker.create.journal, "{what}");
            run.click(
                &mut picker,
                Role::RadioButton,
                "Keep the changes in DOS games",
                1,
            );
            assert!(picker.create.journal, "{what}");
            run.click(
                &mut picker,
                Role::CheckBox,
                "Also keep in the library as an installation media set",
                1,
            );
            assert!(picker.create.keep, "{what}");
            let outcome = run.click(&mut picker, Role::Button, "Create and insert", 1);
            let Some(Outcome::Create(order)) = outcome else {
                panic!("{what}: {outcome:?}")
            };
            assert_eq!(order.unit, 1, "{what}");
            assert_eq!(
                order.top.map(|t| t.to_string()).as_deref(),
                Some("NIGHTFAL")
            );
            assert!(!order.split && order.journal, "{what}");
            assert_eq!(
                order.keep,
                Some((
                    "private/media/ibm-pc/nightfal:1.0-r1".to_owned(),
                    "NIGHTFAL".to_owned()
                ))
            );
            assert_eq!(
                run.click(&mut picker, Role::Button, "Cancel", 1),
                Some(Outcome::Cancel),
                "{what}"
            );
            let mut picker = creating(&[temp.path().join("big")]);
            assert_eq!(
                run.click(&mut picker, Role::Button, "Create and insert", 1),
                None,
                "{what}"
            );
        }
    }

    /// A file dropped on the Create view joins its list once; the search
    /// view takes no drops.
    #[test]
    fn dropped_files_join_the_create_list() {
        #[derive(Debug)]
        struct Dropped(std::path::PathBuf);
        impl egui::DroppedFile for Dropped {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                Err("not read".into())
            }
        }
        let temp = crate::media_create::tests::host("picker-create-drop", &[("tq.exe", 100)]);
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        let drop = |picker: &mut Picker| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    dropped_files: vec![std::sync::Arc::new(Dropped(temp.path().join("tq.exe")))],
                    ..Default::default()
                },
                |ui| {
                    show(ui.ctx(), picker, true);
                },
            );
            output.textures_delta.clear();
        };
        let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 1, library(), None);
        drop(&mut picker);
        assert!(picker.create.items.is_empty());
        picker.open_create();
        drop(&mut picker);
        drop(&mut picker);
        assert_eq!(picker.create.items.len(), 1);
        assert_eq!(picker.create.items[0].path, temp.path().join("tq.exe"));
    }

    /// The Create view's texts stay inside their panes across the width, in
    /// every skin and theme, with long names and a split: a table cuts a
    /// long cell short rather than running past the pane.
    #[test]
    fn the_create_view_keeps_its_texts_inside_the_panes() {
        let mut files: Vec<(String, usize)> = (0..40)
            .map(|n| (format!("azuramon/gfx/tile{n:03}.pcx"), 30_000))
            .collect();
        files.push((
            "azuramon/Azuramon Manual for the level editor.txt".into(),
            48_120,
        ));
        files.push(("azuramon/level editor.exe".into(), 228_416));
        files.push(("azuramon/.DS_Store".into(), 6_148));
        files.push((
            "azuramon/a rather long folder name/deeper still/+bonus.map".into(),
            8_812,
        ));
        files.push(("A loose file with a very long name indeed.txt".into(), 10));
        let files: Vec<(&str, usize)> = files.iter().map(|(p, n)| (p.as_str(), *n)).collect();
        let temp = crate::media_create::tests::host("picker-create-inside", &files);
        for (classic, dark) in LOOKS {
            for paths in [
                vec![temp.path().join("azuramon")],
                vec![
                    temp.path().join("azuramon"),
                    temp.path()
                        .join("A loose file with a very long name indeed.txt"),
                ],
            ] {
                let what = format!("classic {classic}, dark {dark}, {} items", paths.len());
                let mut picker = creating(&paths);
                picker.create.set_format(media::fat12::FloppyFormat::Dd720);
                let frame = shown(&mut picker, BIG, classic, dark);
                assert!(
                    frame
                        .texts
                        .iter()
                        .any(|t| t.text.starts_with("Split across")),
                    "{what}: no split"
                );
                for text in &frame.texts {
                    assert!(
                        text.glyphs.left() >= text.clip.left() - 0.5
                            && text.glyphs.right() <= text.clip.right() + 0.5,
                        "{what}: {:?} at {:?} runs past {:?}",
                        text.text,
                        text.glyphs,
                        text.clip
                    );
                }
            }
        }
    }

    /// The From row greys a tab per configured registry, the default first.
    #[test]
    fn the_from_row_greys_each_configured_registry() {
        let config = image_registry::config::RegistryConfig::from_toml(
            "[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n",
        )
        .unwrap();
        let mut picker = Picker::new(vec![machine(None, When::Now)], 0, 0, library(), None);
        picker.remotes = remotes(&config, "registry.example");
        assert_eq!(
            picker.remotes,
            [
                ("registry.example".to_owned(), "default".to_owned()),
                ("nas.example:5000".to_owned(), "private".to_owned()),
            ]
        );
        for (classic, dark) in LOOKS {
            let shown = shown(&mut picker, BIG, classic, dark);
            for label in [
                "registry.example default · after the MVP",
                "nas.example:5000 private · after the MVP",
            ] {
                shown.widget(egui::accesskit::Role::Button, label);
            }
        }
    }
}
