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

use assembly::FloppyGeometry;
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
    Cancel,
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
    focus: bool,
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
            focus: true,
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
                let disk = &set.disks[self.chosen_disk()];
                if set.disks.len() > 1 && self.all {
                    format!(
                        "{} — {} (all {} disks join the catalogue)",
                        set.title,
                        disk.label,
                        set.disks.len()
                    )
                } else {
                    format!("{} — {}", set.title, disk.label)
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

// ---- rendering -------------------------------------------------------------

fn small(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}

/// A cover placeholder: the set's short title on a dark tile, as the mock-up
/// draws it until covers come with the registry catalogue.
fn cover(ui: &mut egui::Ui, title: &str, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 2.0, egui::Color32::from_rgb(0x2b, 0x3a, 0x55));
    let short: String = title.chars().take(10).collect();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        short,
        egui::FontId::proportional(9.0),
        egui::Color32::WHITE,
    );
}

/// The height of the picker's body: the facets, results and details columns.
const BODY_HEIGHT: f32 = 300.0;

/// One column of the body, `width` wide, scrolling inside the body's height.
fn column(ui: &mut egui::Ui, width: f32, id: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.set_width(width);
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(BODY_HEIGHT)
            .show(ui, content);
    });
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
    let enter_inserts = !popup && (focused.is_none() || focused == Some(search));
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
            !popup && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            !popup && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            enter_inserts && input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            alt_a,
            alt_b,
            typed,
        )
    });
    // Typing searches wherever the focus is, as the mock-up moves it back.
    if typed && focused != Some(search) && !popup && picker.source == Source::Library {
        picker.focus = true;
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
        ui.set_width(760.0);
        let title = format!("Insert disk — {machine_name}");
        if classic {
            crate::shell::gradient_title(ui, &title);
        } else {
            ui.heading(title);
        }
        target_row(ui, picker);
        ui.separator();
        source_row(ui, picker);
        ui.horizontal(|ui| {
            let mut query = picker.query.clone();
            let response = ui.add_enabled(
                picker.source == Source::Library,
                egui::TextEdit::singleline(&mut query)
                    .id(search)
                    .desired_width(420.0)
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
            let chips: Vec<(String, String)> = picker
                .facets
                .iter()
                .flat_map(|(name, values)| values.iter().map(move |v| (name.clone(), v.clone())))
                .collect();
            for (name, value) in chips {
                if ui.small_button(format!("{name}: {value} ×")).clicked() {
                    picker.toggle_facet(&name, &value);
                }
            }
        });
        // The facets, results and details share one fixed height, as the
        // mock-up's body does, and each scrolls inside it. A vertical
        // separator takes all the height on offer, and a modal offers the
        // height it measured last frame: in a row without a bound the
        // separators would grow the modal by its footer's height every frame.
        let body = egui::vec2(ui.available_width(), BODY_HEIGHT);
        ui.allocate_ui_with_layout(body, egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.set_min_height(BODY_HEIGHT);
            column(ui, 170.0, "picker-facets", |ui| facets(ui, picker));
            ui.separator();
            column(ui, 300.0, "picker-results", |ui| {
                results(ui, picker, &mut double);
            });
            ui.separator();
            column(ui, 250.0, "picker-details", |ui| details(ui, picker));
        });
        ui.separator();
        let problem = picker.problem();
        let summary = picker.summary();
        if problem.is_some() {
            ui.colored_label(ui.visuals().warn_fg_color, summary);
        } else {
            ui.label(summary);
        }
        small(
            ui,
            "Type to search · ↑↓ choose · Enter inserts · Alt+A Alt+B drive · Esc closes",
        );
        ui.horizontal(|ui| {
            let unit = picker.unit;
            if ui.button("Browse for an image file…").clicked() {
                outcome = Some(Outcome::Browse(unit));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    outcome = Some(Outcome::Cancel);
                }
                if ui
                    .add_enabled(problem.is_none(), egui::Button::new("Insert"))
                    .clicked()
                {
                    outcome = picker.insert();
                }
            });
        });
        // Escape closes an open drop-down first, then the picker.
        if !egui::Popup::is_any_open(ui.ctx()) && ui.input_mut(crate::shell::escape_pressed) {
            outcome = Some(Outcome::Cancel);
        }
    });
    // A double-click on a row inserts it, as Enter does.
    if double && outcome.is_none() {
        outcome = picker.insert();
    }
    outcome
}

fn target_row(ui: &mut egui::Ui, picker: &mut Picker) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Into");
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
                    format!(
                        "{} {} · {}",
                        drive_name(d.unit),
                        format_name(d.geometry),
                        d.holds
                    ),
                )
            })
            .collect();
        for (unit, text) in drives {
            if ui
                .selectable_label(picker.unit == unit, text)
                .on_hover_text(format!("Alt+{}", char::from(b'A' + unit)))
                .clicked()
            {
                picker.switch_drive(unit);
            }
        }
        let when = match picker.machine().when {
            When::Now => "Running: goes in now",
            When::Halted => "Halted: goes in now, then Ctrl+Alt+Del boots from it",
            When::AtStart => "Stopped: goes in at the next Start",
        };
        ui.strong(when);
    });
}

fn source_row(ui: &mut egui::Ui, picker: &mut Picker) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("From");
        let in_machine: usize = picker
            .machine()
            .drives
            .iter()
            .map(|d| d.catalogue.len())
            .sum();
        if ui
            .selectable_label(
                picker.source == Source::Machine,
                format!("This machine ({in_machine})"),
            )
            .clicked()
        {
            picker.set_source(Source::Machine);
        }
        if ui
            .selectable_label(
                picker.source == Source::Library,
                format!("This library ({})", picker.sets.len()),
            )
            .clicked()
        {
            picker.set_source(Source::Library);
        }
        ui.add_enabled(
            false,
            egui::Button::selectable(false, "Remotes · after the MVP"),
        )
        .on_disabled_hover_text("Remotes come after the MVP");
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

fn facets(ui: &mut egui::Ui, picker: &mut Picker) {
    if picker.source != Source::Library {
        small(ui, "Filters apply to This library.");
        return;
    }
    ui.strong("Filters");
    let unit = drive_name(picker.unit);
    let mut toggled = None;
    for name in FACETS {
        let values = picker.facet_values(name);
        egui::CollapsingHeader::new(name)
            .id_salt(("picker-facet", name))
            .default_open(matches!(name, "Format" | "Genre"))
            .show(ui, |ui| {
                if values.is_empty() {
                    ui.weak("none here");
                }
                for (value, count, fits) in values {
                    let mut on = picker.facets.get(name).is_some_and(|v| v.contains(&value));
                    let label = if fits {
                        format!("{value} ({count})")
                    } else {
                        format!("{value} · doesn't fit {unit} ({count})")
                    };
                    if ui
                        .add_enabled(count > 0 || on, egui::Checkbox::new(&mut on, label))
                        .changed()
                    {
                        toggled = Some((name, value));
                    }
                }
            });
    }
    if let Some((name, value)) = toggled {
        picker.toggle_facet(name, &value);
    }
}

/// The result list; `insert` is set when a row is double-clicked.
fn results(ui: &mut egui::Ui, picker: &mut Picker, insert: &mut bool) {
    let mut chosen = None;
    let mut switch = None;
    let mut toggle = false;
    if picker.source == Source::Machine {
        let drives = picker.machine().drives.clone();
        let geometry = picker.drive().map(|d| d.geometry);
        for drive in &drives {
            ui.strong(format!(
                "{} {} — catalogue",
                drive_name(drive.unit),
                format_name(drive.geometry)
            ));
            if drive.catalogue.is_empty() {
                ui.weak("Empty");
            }
            for entry in &drive.catalogue {
                let fits = geometry.is_some_and(|g| g.accepted_media().contains(&entry.format));
                let selected = picker.choice == Some(Choice::Entry(drive.unit, entry.index));
                let text = format!(
                    "{} · {} · {}{}{}",
                    entry.name,
                    format_name(entry.format),
                    entry.write,
                    if entry.inserted {
                        format!(" · in {} now", drive_name(drive.unit))
                    } else {
                        String::new()
                    },
                    if fits {
                        String::new()
                    } else {
                        format!(" — doesn't fit {}", drive_name(picker.unit))
                    }
                );
                let response = ui.selectable_label(selected, text);
                if response.clicked() {
                    chosen = Some(Choice::Entry(drive.unit, entry.index));
                }
                if response.double_clicked() {
                    chosen = Some(Choice::Entry(drive.unit, entry.index));
                    *insert = true;
                }
            }
            if let Some(note) = &drive.catalogue_note {
                small(ui, note.clone());
            }
        }
        small(ui, "This machine lists the disks already in its drives' catalogues, the quick way to swap disks.");
    } else {
        let results = picker.results();
        let drive = picker.drive().cloned();
        for &index in &results.fit {
            let response = set_row(ui, picker, index, false);
            if response.clicked() {
                chosen = Some(Choice::Set(index));
            }
            if response.double_clicked() {
                chosen = Some(Choice::Set(index));
                *insert = true;
            }
        }
        if results.fit.is_empty() {
            ui.weak(match &drive {
                Some(d) if !results.misfit.is_empty() => format!(
                    "Nothing that fits {} ({}) matches.",
                    drive_name(d.unit),
                    format_name(d.geometry)
                ),
                _ => "Nothing matches. Clear the search or a filter.".to_owned(),
            });
        }
        if !results.misfit.is_empty() {
            if let Some(d) = &drive {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
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
                    ));
                    toggle = ui
                        .link(if picker.show_misfits { "Hide" } else { "Show" })
                        .clicked();
                });
            }
            if picker.show_misfits {
                for &index in &results.misfit {
                    let response = set_row(ui, picker, index, true);
                    if response.clicked() {
                        chosen = Some(Choice::Set(index));
                    }
                    let set = &picker.sets[index];
                    match (picker.other_drive(set), &set.problem) {
                        (_, Some(problem)) => small(ui, problem.clone()),
                        (Some(other), None) => {
                            let unit = other.unit;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Fits {} ({}) —",
                                        drive_name(unit),
                                        format_name(other.geometry)
                                    ))
                                    .small(),
                                );
                                if ui
                                    .link(format!(
                                        "switch to {} or Alt+{}",
                                        drive_name(unit),
                                        char::from(b'A' + unit)
                                    ))
                                    .clicked()
                                {
                                    switch = Some(unit);
                                }
                            });
                        }
                        (None, None) => small(
                            ui,
                            format!(
                                "No drive in {} takes {}",
                                picker.machine().name,
                                set.format().map_or_else(|| "CD".to_owned(), format_name)
                            ),
                        ),
                    }
                }
            }
        }
        if picker.remote_preview {
            ui.separator();
            ui.weak("From a remote · preview — after the MVP");
            ui.add_enabled_ui(false, |ui| {
                ui.label("A set not in this library: its download size and Pull and insert.");
                let _ = ui.button("Pull and insert");
            });
            small(ui, "Pull and insert pulls the set by digest into this library, verifies every byte, then inserts it like a local set. A private remote first asks for a login.");
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

fn set_row(ui: &mut egui::Ui, picker: &Picker, index: usize, misfit: bool) -> egui::Response {
    let set = &picker.sets[index];
    let selected = picker.choice == Some(Choice::Set(index));
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
    let response = ui
        .horizontal(|ui| {
            cover(ui, &set.title, egui::vec2(28.0, 36.0));
            ui.vertical(|ui| {
                let mut title = egui::RichText::new(&set.title).strong();
                if misfit {
                    title = title.weak();
                }
                ui.label(title);
                let year = set.year.map(|y| y.to_string()).unwrap_or_default();
                ui.label(
                    egui::RichText::new(
                        [set.publisher.as_str(), year.as_str(), set.genre.as_str()]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · "),
                    )
                    .small(),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {} · {}{}",
                        set.disks_label(),
                        match set.extras {
                            0 => "no extras".to_owned(),
                            1 => "1 extra".to_owned(),
                            n => format!("{n} extras"),
                        },
                        if set.open { "open" } else { "private" },
                        if in_drives.is_empty() {
                            String::new()
                        } else {
                            format!(" · in {}", in_drives.join(", "))
                        }
                    ))
                    .small(),
                );
            });
        })
        .response;
    let response = response.interact(egui::Sense::click());
    if selected {
        ui.painter().rect_stroke(
            response.rect,
            2.0,
            egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn details(ui: &mut egui::Ui, picker: &mut Picker) {
    let Some(choice) = picker.choice else {
        small(ui, "Choose a set.");
        return;
    };
    let unit = picker.unit;
    let name = picker.machine().name.clone();
    match choice {
        Choice::Entry(from, index) => {
            let Some(entry) = picker.entry(from, index).cloned() else {
                return;
            };
            ui.strong(&entry.name);
            small(
                ui,
                format!(
                    "In {}'s catalogue · {}",
                    drive_name(from),
                    format_name(entry.format)
                ),
            );
            if picker.problem().is_none() {
                ui.label(format!(
                    "Insert puts it into {}. It keeps its own write setting: {}.",
                    drive_name(unit),
                    entry.write
                ));
            }
        }
        Choice::Set(index) => {
            let set = picker.sets[index].clone();
            ui.horizontal(|ui| {
                cover(ui, &set.title, egui::vec2(44.0, 56.0));
                ui.vertical(|ui| {
                    ui.strong(&set.title);
                    small(
                        ui,
                        format!(
                            "{} · {} · {}",
                            set.reference,
                            set.disks_label(),
                            if set.open { "open" } else { "private" }
                        ),
                    );
                });
            });
            let Some(drive) = picker.drive().cloned() else {
                return;
            };
            if !set.fits(drive.geometry) {
                let accepted: Vec<String> = drive
                    .geometry
                    .accepted_media()
                    .into_iter()
                    .map(format_name)
                    .collect();
                ui.add(
                    egui::Label::new(format!(
                        "Doesn't fit {}: it takes {} disks.",
                        drive_name(unit),
                        accepted.join(" and ")
                    ))
                    .wrap(),
                );
                if let Some(other) = picker.other_drive(&set) {
                    let other_unit = other.unit;
                    if ui
                        .link(format!(
                            "Switch to {} ({})",
                            drive_name(other_unit),
                            format_name(other.geometry)
                        ))
                        .clicked()
                    {
                        picker.switch_drive(other_unit);
                    }
                } else {
                    small(
                        ui,
                        format!(
                            "No drive in {name} takes {}.",
                            set.format().map_or_else(|| "CD".to_owned(), format_name)
                        ),
                    );
                }
                return;
            }
            ui.strong("Disk to insert");
            let mut disk = picker.chosen_disk();
            for (i, d) in set.disks.iter().enumerate() {
                if ui
                    .radio_value(
                        &mut disk,
                        i,
                        format!(
                            "{}. {} · {}",
                            i + 1,
                            d.label,
                            d.format.map_or_else(|| "CD".to_owned(), format_name)
                        ),
                    )
                    .clicked()
                {
                    picker.pick_disk(i);
                }
            }
            if set.disks.len() > 1 {
                ui.checkbox(
                    &mut picker.all,
                    format!(
                        "Put all {} disks in {}'s catalogue, in order",
                        set.disks.len(),
                        drive_name(unit)
                    ),
                );
                small(
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
                small(
                    ui,
                    format!(
                        "Already in {}'s catalogue; those disks keep their own write setting.",
                        drive_name(unit)
                    ),
                );
            }
            ui.strong("When a program writes to the disk");
            let unavailable = picker.machine().journal_unavailable.clone();
            ui.add_enabled_ui(unavailable.is_none(), |ui| {
                ui.radio_value(
                    &mut picker.journal,
                    true,
                    format!("Keep the changes in {name}"),
                );
            });
            match unavailable {
                Some(reason) => small(ui, reason),
                None => small(
                    ui,
                    "Saved with the machine. The disk in the library never changes.",
                ),
            }
            ui.radio_value(&mut picker.journal, false, "Write-protected");
            small(ui, "Like a disk with its write-protect tab set: a program that tries to write gets an error.");
            if set.extras > 0 {
                small(
                    ui,
                    "Extras stay in the library; open them from the set's details.",
                );
            }
        }
    }
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

    /// The picker draws without a window, in every source.
    #[test]
    fn the_picker_renders_headlessly() {
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
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
        crate::shell::configure_context(&ctx);
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
}
