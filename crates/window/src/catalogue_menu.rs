// SPDX-License-Identifier: AGPL-3.0-or-later
//! A floppy drive's Catalogue submenu, under the drive in the Media menu and
//! in its drive chip's menu, as revision 3 of the approved media-picker
//! mock-up draws it; machine-images §7.
//!
//! The submenu lists the drive's catalogue in order: the same disks, with the
//! same names, as the media picker's This machine source, and choosing one
//! does what Insert there does with it. A running or halted machine takes the
//! disk now; a stopped one takes it at its next Start, and choosing the disk
//! already in its drive takes a waiting disk out again. A menu never scrolls,
//! so past [`CAP`] disks the submenu lists a window around the disk in the
//! drive, or the one waiting for Start, and All N disks… opens the picker.
//!
//! This module also owns the one short name a disk has wherever a drive
//! names it: the drive chip, the Media menu's drive line, This machine in the
//! picker and the submenu.

use crate::media_dialog::{CatalogueEntry, FloppySlot};
use crate::media_picker::{drive_name, CatalogueDisk, SetEntry};
use crate::shell_model::Command;
use egui::containers::menu::{MenuState, SubMenu, SubMenuButton};
use egui::{FocusDirection, Key, Modifiers, RichText};
use image_store::Digest;

/// The most disks the submenu lists: a menu never scrolls.
pub(crate) const CAP: usize = 12;
/// Disks listed before the one in the drive when the catalogue is longer
/// than [`CAP`]; the others come after it, since programs ask for the next
/// disk.
const BEFORE: usize = 2;
/// The mark of the disk in the drive.
pub(crate) const IN_DRIVE_MARK: &str = "●";
/// The submenu's narrowest and widest, as the mock-up's submenu sets them.
const MIN_WIDTH: f32 = 250.0;
const MAX_WIDTH: f32 = 360.0;

/// A disk of an installation media set: what its short name is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SetDisk<'a> {
    pub title: &'a str,
    /// The disk's place in the set, from 1.
    pub number: usize,
    pub count: usize,
    pub label: &'a str,
}

/// The disk `medium` of `set`, when the set has it. The set's title leaves
/// out its edition in parentheses, as the picker sets the two apart.
pub(crate) fn set_disk<'a>(set: &'a SetEntry, medium: &str) -> Option<SetDisk<'a>> {
    let at = set.disks.iter().position(|disk| disk.id == medium)?;
    Some(SetDisk {
        title: crate::media_picker::title_parts(&set.title).0,
        number: at + 1,
        count: set.disks.len(),
        label: &set.disks[at].label,
    })
}

/// A disk's short name. A disk of a set is the set's title and its place,
/// "AZURAMON 3 of 10", then its own name after a dash when that says more
/// than its number, "FreeDOS 1 of 2 — Boot disk"; a title that ends in a
/// number takes a middle dot, so "Win 3.1 · 1 of 7" never reads as a disk
/// numbered 3.11. A one-disk set is its title alone. Any other disk, such
/// as an image file chosen with Browse…, keeps `name`.
pub(crate) fn short_name(disk: Option<SetDisk<'_>>, name: &str) -> String {
    let Some(disk) = disk else {
        return name.to_owned();
    };
    let title = disk.title;
    if disk.count <= 1 {
        return title.to_owned();
    }
    let separator = if title.ends_with(|c: char| c.is_ascii_digit()) {
        " · "
    } else {
        " "
    };
    let mut short = format!("{title}{separator}{} of {}", disk.number, disk.count);
    if let Some(own) = own_name(disk.label, disk.number) {
        short.push_str(" — ");
        short.push_str(&own);
    }
    short
}

/// What the label of disk `number` names it, without what only repeats its
/// place: "Disk 1 (Setup)" is "Setup", "Disk 1 of 8 - packages
/// (x86DSK01.img)" is "Packages", and "Disk 2", "Disk 2 of 7", "Disk #2",
/// "Diskette 2", "Disk2" or "2/7" of disk 2 name nothing more. A file name in
/// parentheses at the end is no name of the disk's own; a number that is not
/// the disk's place stays, as it says something.
pub(crate) fn own_name(label: &str, number: usize) -> Option<String> {
    let (label, _) = crate::media_picker::label_without_file(label);
    let label = label.trim();
    let Some(rest) = after_place(label, number) else {
        return (!label.is_empty()).then(|| label.to_owned());
    };
    let mut rest = rest
        .trim_start_matches(|c: char| c.is_whitespace() || "-–—:,".contains(c))
        .trim_end();
    if let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        rest = inner.trim();
    }
    if rest.is_empty() {
        return None;
    }
    // What follows a place in lower case starts with a capital.
    let first = rest.split_whitespace().next().unwrap_or_default();
    if first.chars().all(|c| c.is_ascii_lowercase()) {
        let mut chars = rest.chars();
        return chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str());
    }
    Some(rest.to_owned())
}

/// `label` after a leading place of disk `number`: "Disk 2", "Disk #2",
/// "Diskette 2" or "Disk2", each with " of 7" or "/7" after it, or a bare
/// "2 of 7" or "2/7". `None` when the label does not start with that place;
/// a bare number without a count, as "2 Players", is a name.
fn after_place(label: &str, number: usize) -> Option<&str> {
    let mut rest = label;
    let word = ["diskette", "disk"].into_iter().find_map(|word| {
        let head = rest.get(..word.len())?;
        head.eq_ignore_ascii_case(word).then(|| &rest[word.len()..])
    });
    if let Some(after) = word {
        let after = after.trim_start();
        rest = after.strip_prefix('#').unwrap_or(after).trim_start();
    }
    let (place, after) = leading_number(rest)?;
    if place != number {
        return None;
    }
    rest = after;
    let tail = rest.trim_start();
    let count = tail
        .get(..2)
        .filter(|of| of.eq_ignore_ascii_case("of") && tail[2..].starts_with(' '))
        .map(|_| tail[2..].trim_start())
        .or_else(|| tail.strip_prefix('/').map(str::trim_start))
        .and_then(leading_number);
    match count {
        Some((_, after)) => rest = after,
        None if word.is_none() => return None,
        None => {}
    }
    (!rest.starts_with(|c: char| c.is_alphanumeric())).then_some(rest)
}

/// The decimal number `text` starts with, and what follows it.
fn leading_number(text: &str) -> Option<(usize, &str)> {
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let number = text.get(..digits)?.parse().ok()?;
    Some((number, &text[digits..]))
}

/// A catalogue entry's short name, `set` being the library set its source
/// names when the library has it.
pub(crate) fn entry_name(entry: &CatalogueEntry, set: Option<&SetEntry>) -> String {
    let own = if entry.medium.name.is_empty() {
        "image"
    } else {
        entry.medium.name.as_str()
    };
    let disk = entry
        .source
        .as_ref()
        .zip(set)
        .and_then(|(source, set)| set_disk(set, &source.medium_id));
    short_name(disk, own)
}

/// How writes to a disk go, in plain words, for the machine `machine`.
fn write_words(write: &assembly::WriteMode, machine: &str) -> String {
    match write {
        assembly::WriteMode::ReadOnly => "write-protected".to_owned(),
        assembly::WriteMode::Journal { .. } => format!("changes kept in {machine}"),
        assembly::WriteMode::Volatile => "changes lost at Stop".to_owned(),
    }
}

/// A drive's catalogue as the picker's This machine lists it and the
/// Catalogue submenu shows it: every disk that can go in, in order, with its
/// short name. `now` is the catalogue index of the disk in the drive while
/// the machine runs; `find` looks a library set up by its digest.
pub(crate) fn slot_catalogue(
    slot: &FloppySlot,
    now: Option<usize>,
    machine: &str,
    find: &dyn Fn(&Digest) -> Option<SetEntry>,
) -> Vec<CatalogueDisk> {
    let mut sets = Sets::new(find);
    slot.entries
        .iter()
        .filter(|entry| entry.reason.is_none())
        .filter_map(|entry| {
            let index = entry.index?;
            Some(sets.disk(entry, index, slot.geometry, now == Some(index), machine))
        })
        .collect()
}

/// The disks one Insert put into a stopped drive for its next Start, as the
/// Catalogue submenu lists them while the drive's own disks are not known:
/// by their place among them, each with its short name.
pub(crate) fn waiting_catalogue(
    entries: &[CatalogueEntry],
    geometry: assembly::FloppyGeometry,
    machine: &str,
    find: &dyn Fn(&Digest) -> Option<SetEntry>,
) -> Vec<CatalogueDisk> {
    let mut sets = Sets::new(find);
    entries
        .iter()
        .enumerate()
        .map(|(at, entry)| sets.disk(entry, at, geometry, false, machine))
        .collect()
}

/// The library sets a list of disks names, each looked up once however many
/// of its disks the list has.
struct Sets<'a> {
    find: &'a dyn Fn(&Digest) -> Option<SetEntry>,
    found: Vec<(Digest, Option<SetEntry>)>,
}

impl<'a> Sets<'a> {
    fn new(find: &'a dyn Fn(&Digest) -> Option<SetEntry>) -> Self {
        Self {
            find,
            found: Vec::new(),
        }
    }

    fn set(&mut self, digest: &Digest) -> Option<&SetEntry> {
        let at = match self.found.iter().position(|(found, _)| found == digest) {
            Some(at) => at,
            None => {
                self.found.push((digest.clone(), (self.find)(digest)));
                self.found.len() - 1
            }
        };
        self.found[at].1.as_ref()
    }

    /// `entry` as a listed disk at `index`.
    fn disk(
        &mut self,
        entry: &CatalogueEntry,
        index: usize,
        geometry: assembly::FloppyGeometry,
        inserted: bool,
        machine: &str,
    ) -> CatalogueDisk {
        let set = entry
            .source
            .as_ref()
            .and_then(|source| self.set(&source.set));
        let place = entry.source.as_ref().zip(set).and_then(|(source, set)| {
            let disk = set.disks.iter().position(|d| d.id == source.medium_id)?;
            Some((source.set.clone(), disk))
        });
        CatalogueDisk {
            name: entry_name(entry, set),
            format: entry.geometry.unwrap_or(geometry),
            write: write_words(&entry.medium.write, machine),
            inserted,
            index,
            set: place,
        }
    }
}

/// What waits for a stopped machine's next Start in the drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// A disk of the list, by its index there.
    Entry(usize),
    /// A disk that joins the catalogue at Start, by its short name.
    Joins(String),
}

/// The line under a list of disks waiting for Start: the drive's own disks
/// come with the image and join them then.
pub(crate) const OWN_DISKS_AT_START: &str = "The image's own disks join the catalogue then.";

/// One drive's Catalogue submenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Catalogue {
    pub unit: u8,
    /// The drive's catalogue in order, as This machine lists it; or, while
    /// [`Self::unknown`] says why, the disks waiting for the next Start.
    pub disks: Vec<CatalogueDisk>,
    /// The catalogue index of the disk in the drive: now while the machine
    /// runs, at power-on while it is stopped.
    pub in_drive: Option<usize>,
    pub pending: Option<Pending>,
    /// Why the drive's own disks are not known before Start, when they are
    /// not: those of a stopped library machine, or of an image, join it only
    /// then.
    pub unknown: Option<String>,
    /// The disks waiting for Start are the whole catalogue the stopped
    /// library machine kept at its last Stop, the image's own disks among
    /// them, so no line says that those join later.
    pub kept: bool,
}

/// What a disk row says at its right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    None,
    /// The disk is in the drive.
    InDrive,
    /// The disk goes in at the next Start.
    AtStart,
}

/// One line of the submenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Heading(String),
    Disk {
        index: usize,
        name: String,
        /// The disk is in the drive: the mark column shows [`IN_DRIVE_MARK`].
        here: bool,
        mark: Mark,
    },
    /// A line under the disks: a disk waiting for Start that is not in the
    /// catalogue yet, or the drive's own disks that join it then.
    Note(String),
    /// All N disks…, past [`CAP`].
    All(usize),
}

impl Catalogue {
    /// The item's label and tooltip while it has nothing to choose: one
    /// disk, or none; or, while the drive's own disks are not known, no disk
    /// waiting for Start.
    pub fn disabled(&self) -> Option<(String, String)> {
        let drive = drive_name(self.unit);
        if let Some(why) = &self.unknown {
            return self
                .disks
                .is_empty()
                .then(|| ("Catalogue · known at Start".into(), why.clone()));
        }
        match self.disks.len() {
            0 => Some((
                "Catalogue · empty".into(),
                format!(
                    "{drive}'s catalogue is empty. Insert… and Create from files… add disks to it."
                ),
            )),
            1 => Some((
                "Catalogue · one disk".into(),
                format!("Only one disk in {drive}'s catalogue. Insert… adds more."),
            )),
            _ => None,
        }
    }

    fn position(&self, index: usize) -> Option<usize> {
        self.disks.iter().position(|disk| disk.index == index)
    }

    fn pending_index(&self) -> Option<usize> {
        match self.pending {
            Some(Pending::Entry(index)) => Some(index),
            _ => None,
        }
    }

    /// The disks listed, as positions in [`Self::disks`]: all of them up to
    /// [`CAP`]; past it, [`CAP`] of them from [`BEFORE`] ahead of the disk
    /// waiting for Start, else the disk in the drive, else the first.
    pub fn window(&self) -> std::ops::Range<usize> {
        let count = self.disks.len();
        if count <= CAP {
            return 0..count;
        }
        let at = self
            .pending_index()
            .and_then(|index| self.position(index))
            .or_else(|| self.in_drive.and_then(|index| self.position(index)))
            .unwrap_or(0);
        let from = at.saturating_sub(BEFORE).min(count - CAP);
        from..from + CAP
    }

    /// The submenu's lines, top to bottom.
    pub fn rows(&self) -> Vec<Row> {
        let drive = drive_name(self.unit);
        let count = self.disks.len();
        let window = self.window();
        let capped = window.len() < count;
        let mut rows = vec![Row::Heading(match (capped, self.unknown.is_some()) {
            (true, false) => format!(
                "Disks {}–{} of {count} in {drive}'s catalogue",
                window.start + 1,
                window.end
            ),
            (false, false) => format!("{drive}'s catalogue, in order · {count} disks"),
            (true, true) => format!(
                "Disks {}–{} of {count} for {drive} at the next Start",
                window.start + 1,
                window.end
            ),
            (false, true) if count == 1 => format!("For {drive} at the next Start · 1 disk"),
            (false, true) => format!("For {drive} at the next Start, in order · {count} disks"),
        })];
        for disk in &self.disks[window] {
            let here = self.in_drive == Some(disk.index);
            rows.push(Row::Disk {
                index: disk.index,
                name: disk.name.clone(),
                here,
                mark: if self.pending_index() == Some(disk.index) {
                    Mark::AtStart
                } else if here {
                    Mark::InDrive
                } else {
                    Mark::None
                },
            });
        }
        if let Some(Pending::Joins(name)) = &self.pending {
            rows.push(Row::Note(format!(
                "At Start: {name}, which joins the catalogue then."
            )));
        }
        if self.unknown.is_some() && !self.kept {
            rows.push(Row::Note(OWN_DISKS_AT_START.into()));
        }
        if capped {
            rows.push(Row::All(count));
        }
        rows
    }

    /// The disk a keyboard opening puts the focus on: the disk in the drive,
    /// else the one waiting for Start, when the submenu lists it.
    pub fn focus(&self) -> Option<usize> {
        let index = self.in_drive.or_else(|| self.pending_index())?;
        self.window()
            .contains(&self.position(index)?)
            .then_some(index)
    }
}

/// The Catalogue item of a drive's menu and its submenu; `None` while the
/// drive takes no disk. A chosen disk, or All N disks…, goes to `commands`.
///
/// The submenu opens beside the menu on the side with room. In a menu that
/// opens upward, a drive chip's menu on the status bar, it grows upward from
/// its item and leaves the status bar clear; elsewhere it grows down.
///
/// The submenu opens on hover or click. From the keyboard, → or Enter on
/// the item opens it on the disk in the drive, ↑↓ choose, Enter takes the
/// disk, and ← or Escape go back to the item; the menu's own Escape then
/// closes the menu.
pub(crate) fn item(
    ui: &mut egui::Ui,
    catalogue: Option<&Catalogue>,
    upward: bool,
    commands: &mut Vec<Command>,
) {
    let mark = ui_kit::shell_icons::mark;
    let arrow = || SubMenuButton::arrow_atom(None);
    let Some(catalogue) = catalogue.filter(|catalogue| catalogue.disabled().is_none()) else {
        let (label, why) = catalogue
            .and_then(Catalogue::disabled)
            .map_or(("Catalogue".to_owned(), None), |(label, why)| {
                (label, Some(why))
            });
        let response = ui.add_enabled(
            false,
            egui::Button::new((mark(""), label)).right_text(arrow()),
        );
        if let Some(why) = why {
            response.on_disabled_hover_text(why);
        }
        return;
    };
    let unit = catalogue.unit;
    let drive = drive_name(unit);
    let mut focus = None;
    // The item as egui's submenu button draws it, highlighted while open.
    let submenu_id = SubMenu::id_from_widget_id(ui.next_auto_id());
    let open = MenuState::from_ui(ui, |state, _| state.open_item == Some(submenu_id));
    let inactive = ui.style().visuals.widgets.inactive;
    if open {
        ui.style_mut().visuals.widgets.inactive = ui.style().visuals.widgets.open;
    }
    let response = ui.add(egui::Button::new((mark(""), "Catalogue")).right_text(arrow()));
    ui.style_mut().visuals.widgets.inactive = inactive;
    // egui places a submenu with its top at its item. Growing upward, the
    // place it is given ends at the item's bottom instead, as tall as the
    // submenu was last frame, so its last line sits level with the item.
    let mut anchor = response.clone();
    if let Some(height) = upward
        .then(|| egui::AreaState::load(ui.ctx(), submenu_id)?.size)
        .flatten()
        .map(|size| size.y)
    {
        let margins = egui::Frame::menu(ui.style()).total_margin().sum().y;
        anchor.interact_rect.min.y =
            (response.interact_rect.max.y + margins - height).min(response.interact_rect.min.y);
    }
    // The disk in the drive takes the focus a frame after → or Enter: egui
    // gives it to the submenu's first line then, so the request waits for
    // the next frame, ahead of every line, and lapses after it.
    let in_drive = submenu_id.with("focus the disk in the drive");
    let pass = ui.ctx().cumulative_pass_nr();
    let submenu = SubMenu::new().show(ui, &anchor, |ui| {
        let wanted = ui.data_mut(|data| {
            let wanted = data.get_temp::<(egui::Id, u64)>(in_drive);
            data.remove::<(egui::Id, u64)>(in_drive);
            wanted
        });
        if let Some((id, _)) = wanted.filter(|(_, asked)| asked + 1 == pass) {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        ui.set_min_width(MIN_WIDTH);
        ui.set_max_width(MAX_WIDTH);
        let lines = catalogue.rows();
        // Growing upward, the submenu's place comes from last frame's height:
        // other lines than then are measured again before anything shows.
        let kinds: Vec<u8> = lines
            .iter()
            .map(|line| match line {
                Row::Heading(_) => 0,
                Row::Disk { .. } => 1,
                Row::Note(_) => 2,
                Row::All(_) => 3,
            })
            .collect();
        let measured = submenu_id.with("lines");
        let before = ui.data_mut(|data| data.get_temp::<Vec<u8>>(measured));
        if upward && !ui.is_sizing_pass() && before.is_some_and(|before| before != kinds) {
            ui.ctx()
                .request_discard("the Catalogue submenu measured its lines again");
        }
        ui.data_mut(|data| data.insert_temp(measured, kinds));
        let mut rows = Vec::new();
        for row in lines {
            match row {
                Row::Heading(text) | Row::Note(text) => {
                    ui.add_enabled(false, egui::Label::new(RichText::new(text).small()));
                }
                Row::Disk {
                    index,
                    name,
                    here,
                    mark: right,
                } => {
                    let button = egui::Button::new((
                        mark(if here { IN_DRIVE_MARK } else { "" }),
                        name.as_str(),
                    ));
                    let button = match right {
                        Mark::None => button,
                        Mark::InDrive => {
                            button.shortcut_text(RichText::new(format!("in {drive}")).small())
                        }
                        Mark::AtStart => button.right_text(
                            RichText::new("at Start")
                                .small()
                                .color(ui.visuals().warn_fg_color),
                        ),
                    };
                    let response = ui.add(button);
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::MenuItemRadio, true, here, &name)
                    });
                    if catalogue.focus() == Some(index) {
                        focus = Some(response.id);
                    }
                    rows.push(response.id);
                    if response.clicked() {
                        commands.push(Command::CatalogueDisk(unit, index));
                        ui.close();
                    }
                }
                Row::All(count) => {
                    ui.separator();
                    let response = ui
                        .add(
                            egui::Button::new((mark(""), format!("All {count} disks…")))
                                .shortcut_text(RichText::new("Insert… › This machine").small()),
                        )
                        .on_hover_text(format!(
                            "Opens Insert… on This machine, with the disk in {drive} selected"
                        ));
                    rows.push(response.id);
                    if response.clicked() {
                        commands.push(Command::CatalogueAll(unit));
                        ui.close();
                    }
                }
            }
        }
        // Escape inside the submenu goes back to its item, as ← does, and
        // leaves the menu open; the menu's own Escape closes the menu.
        let inside = ui.memory(|memory| rows.iter().any(|id| memory.had_focus_last_frame(*id)));
        if inside && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
            ui.memory_mut(|memory| memory.move_focus(FocusDirection::Left));
        }
    });
    // → or Enter opens the submenu on the disk in the drive rather than on
    // its first line, so → ↓ Enter puts in the next disk.
    let keyed = response.has_focus()
        && ui.input(|input| {
            input.key_pressed(Key::ArrowRight)
                || input.key_pressed(Key::Enter)
                || input.key_pressed(Key::Space)
        });
    if let (true, Some(_), Some(focus)) = (keyed, submenu, focus) {
        ui.data_mut(|data| data.insert_temp(in_drive, (focus, pass)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_picker::Disk;
    use assembly::FloppyGeometry;

    fn set(title: &str, labels: &[&str]) -> SetEntry {
        SetEntry {
            reference: "sample:1".into(),
            image: image_store::Descriptor::new(
                image_store::media_type::MANIFEST,
                Digest::of(title.as_bytes()),
                1,
            ),
            title: title.into(),
            publisher: String::new(),
            year: None,
            genre: String::new(),
            platform: "DOS".into(),
            labels: Vec::new(),
            disks: labels
                .iter()
                .enumerate()
                .map(|(at, label)| Disk {
                    id: format!("disk{}", at + 1),
                    label: (*label).into(),
                    format: Some(FloppyGeometry::HD_1_44),
                })
                .collect(),
            extras: 0,
            open: true,
            problem: None,
        }
    }

    fn names(set: &SetEntry) -> Vec<String> {
        set.disks
            .iter()
            .map(|disk| short_name(set_disk(set, &disk.id), "ignored"))
            .collect()
    }

    /// A disk of a set is the set's title and its place, with its own name
    /// after a dash only when it says more than its number.
    #[test]
    fn a_disk_of_a_set_is_its_title_and_place() {
        let azuramon: Vec<String> = (1..=10).map(|n| format!("Disk {n}")).collect();
        let labels: Vec<&str> = azuramon.iter().map(String::as_str).collect();
        assert_eq!(
            names(&set("AZURAMON", &labels))[2..4],
            ["AZURAMON 3 of 10", "AZURAMON 4 of 10"]
        );
        assert_eq!(
            names(&set("FreeDOS", &["Boot disk", "disk 2 of 2"])),
            ["FreeDOS 1 of 2 — Boot disk", "FreeDOS 2 of 2"]
        );
        assert_eq!(
            names(&set(
                "FreeDOS 1.4 (Floppy Edition, 1.2M)",
                &[
                    "Disk 1 of 8 - packages (x86DSK01.img)",
                    "Disk 2 of 8 (x86DSK02.img)"
                ]
            )),
            ["FreeDOS 1.4 · 1 of 2 — Packages", "FreeDOS 1.4 · 2 of 2"]
        );
        assert_eq!(
            names(&set("Lemmings", &["Disk 1", "Disk 2: levels"])),
            ["Lemmings 1 of 2", "Lemmings 2 of 2 — Levels"]
        );
    }

    /// A title that ends in a number takes a middle dot before the place.
    #[test]
    fn a_title_that_ends_in_a_number_takes_a_middle_dot() {
        let win = set(
            "Win 3.1",
            &[
                "Disk 1 (Setup)",
                "Disk 2",
                "Disk 3",
                "Disk 4",
                "Disk 5",
                "Disk 6",
                "Disk 7",
            ],
        );
        let names = names(&win);
        assert_eq!(names[0], "Win 3.1 · 1 of 7 — Setup");
        assert_eq!(names[6], "Win 3.1 · 7 of 7");
        assert_eq!(
            short_name(
                Some(SetDisk {
                    title: "Ultima 6",
                    number: 2,
                    count: 4,
                    label: "Disk B"
                }),
                ""
            ),
            "Ultima 6 · 2 of 4 — Disk B"
        );
    }

    /// A one-disk set is its title alone; a disk of no set, such as a
    /// browsed image file, keeps its own name.
    #[test]
    fn a_one_disk_set_is_its_title_and_a_browsed_file_its_name() {
        assert_eq!(
            names(&set("Keen 4", &["Shareware v1.4 installation"])),
            ["Keen 4"]
        );
        assert_eq!(short_name(None, "game.img"), "game.img");
        assert_eq!(short_name(None, "AZURAMON 3 of 10"), "AZURAMON 3 of 10");
    }

    /// A label that only repeats the disk's place, however it is spelled,
    /// adds nothing after the dash; another number, or a bare number that is
    /// no place, is the disk's own name.
    #[test]
    fn a_place_in_any_spelling_names_nothing_more() {
        for label in [
            "Disk 1",
            "disk 1 of 4",
            "DISK 1 OF 4",
            "Disk #1",
            "Disk # 1",
            "Diskette 1",
            "Disk1",
            "1/4",
            "1 / 4",
            "1 of 4",
            "Disk 1/4",
            "Disk 1 (x86DSK01.img)",
        ] {
            assert_eq!(own_name(label, 1), None, "{label}");
        }
        assert_eq!(own_name("Disk 1/4 - tools", 1).as_deref(), Some("Tools"));
        assert_eq!(own_name("Diskette 1: Setup", 1).as_deref(), Some("Setup"));
        assert_eq!(own_name("Disk 3", 1).as_deref(), Some("Disk 3"));
        assert_eq!(own_name("2 Players", 2).as_deref(), Some("2 Players"));
        assert_eq!(own_name("Disk 1x", 1).as_deref(), Some("Disk 1x"));
        assert_eq!(own_name("Diskettes", 1).as_deref(), Some("Diskettes"));
        assert_eq!(
            names(&set("Ultima", &["Disk #1", "Diskette 2", "Disk3", "4/4"])),
            [
                "Ultima 1 of 4",
                "Ultima 2 of 4",
                "Ultima 3 of 4",
                "Ultima 4 of 4"
            ]
        );
        // The picker's disk buttons follow the same rule.
        assert_eq!(crate::media_picker::disk_name("Disk 1 (Setup)", 1), "Setup");
        assert_eq!(
            crate::media_picker::disk_name("Diskette 2", 2),
            "Diskette 2"
        );
    }

    /// A library set's title leaves out its edition; a title Create from
    /// files… was given keeps its parenthesis, which names no edition.
    #[test]
    fn only_a_library_title_leaves_out_its_edition() {
        assert_eq!(
            names(&set(
                "Lemmings (Floppy Edition, 720K)",
                &["Disk 1", "Disk 2"]
            )),
            ["Lemmings 1 of 2", "Lemmings 2 of 2"]
        );
        assert_eq!(
            crate::created_media::disk_name("GAME (1993)", 0, 2),
            "GAME (1993) 1 of 2"
        );
        assert_eq!(
            crate::created_media::disk_name("GAME (1993)", 0, 1),
            "GAME (1993)"
        );
    }

    /// While the drive's own disks are known only at Start, the submenu
    /// lists the disks waiting for it, the chosen one "at Start" and focused,
    /// says under them that the image's own disks join then, and is enabled
    /// for one waiting disk; with none it says when the disks are known.
    #[test]
    fn disks_waiting_for_start_are_listed_while_the_drive_is_unknown() {
        let mut waiting = catalogue(3, None, Some(Pending::Entry(1)));
        waiting.unknown = Some("Known at Start.".into());
        assert_eq!(waiting.disabled(), None);
        let rows = waiting.rows();
        assert_eq!(
            rows[0],
            Row::Heading("For B: at the next Start, in order · 3 disks".into())
        );
        assert_eq!(
            rows[2],
            Row::Disk {
                index: 1,
                name: "AZURAMON 2 of 18".into(),
                here: false,
                mark: Mark::AtStart
            }
        );
        assert_eq!(rows.last(), Some(&Row::Note(OWN_DISKS_AT_START.into())));
        assert_eq!(waiting.focus(), Some(1));
        let mut one = catalogue(1, None, Some(Pending::Entry(0)));
        one.unknown = Some("Known at Start.".into());
        assert_eq!(one.disabled(), None);
        assert_eq!(
            one.rows()[0],
            Row::Heading("For B: at the next Start · 1 disk".into())
        );
    }

    fn entry(
        name: &str,
        index: Option<usize>,
        source: Option<image_store::media_set::CatalogueSource>,
    ) -> CatalogueEntry {
        CatalogueEntry {
            source,
            receipt: None,
            readonly_bytes: None,
            medium: assembly::FloppyMedium {
                image: assembly::ImageSource::File(format!("/media/{name}").into()),
                expected_sha256: None,
                write: assembly::WriteMode::ReadOnly,
                name: name.into(),
            },
            geometry: Some(FloppyGeometry::HD_1_44),
            reason: None,
            index,
            declared: false,
            alternates: Vec::new(),
            workspace_journal: None,
            origin: None,
            verified: None,
            reference: None,
        }
    }

    /// This machine and the submenu list the disks that can go in, in
    /// catalogue order: a library disk by its set and place, any other disk
    /// by its own name, a nameless one as "image". A disk the drive refuses
    /// and one not in the catalogue yet are left out.
    #[test]
    fn the_catalogue_names_a_library_disk_by_its_set() {
        let win = set("Win 3.1", &["Disk 1 (Setup)", "Disk 2"]);
        let source = |medium: &str| image_store::media_set::CatalogueSource {
            set: win.image.digest.clone(),
            medium_id: medium.into(),
            blob: Digest::of(medium.as_bytes()),
        };
        let mut refused = entry("bad.img", Some(3), None);
        refused.reason = Some("damaged".into());
        let slot = FloppySlot {
            geometry: FloppyGeometry::HD_1_44,
            entries: vec![
                entry("game.img", Some(0), None),
                entry("Setup", Some(1), Some(source("disk1"))),
                entry("", Some(2), None),
                refused,
                entry("waiting.img", None, None),
                entry("Disk 2", Some(4), Some(source("disk2"))),
            ],
            journal_retention: 1,
        };
        let find = |digest: &Digest| (*digest == win.image.digest).then(|| win.clone());
        let disks = slot_catalogue(&slot, Some(1), "DOS games", &find);
        let names: Vec<&str> = disks.iter().map(|disk| disk.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "game.img",
                "Win 3.1 · 1 of 2 — Setup",
                "image",
                "Win 3.1 · 2 of 2"
            ]
        );
        assert_eq!(
            disks.iter().map(|disk| disk.index).collect::<Vec<_>>(),
            [0, 1, 2, 4]
        );
        assert_eq!(
            disks.iter().map(|disk| disk.inserted).collect::<Vec<_>>(),
            [false, true, false, false]
        );
        assert_eq!(disks[1].set, Some((win.image.digest.clone(), 0)));
        assert_eq!(disks[0].write, "write-protected");
        // A set the library no longer lists: its disks keep their own names.
        let gone = slot_catalogue(&slot, None, "DOS games", &|_| None);
        assert_eq!(gone[1].name, "Setup");
        assert_eq!(gone[1].set, None);
    }

    /// One frame of a drive menu opened from a button at `anchor` in an
    /// 800 × 600 window, the way a drive chip opens its menu: every text and
    /// where it is.
    fn menu_frame(
        ctx: &egui::Context,
        anchor: egui::Rect,
        upward: bool,
        catalogue: &Catalogue,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let response = ui.put(anchor, egui::Button::new("Drive"));
            egui::Popup::menu(&response).show(|ui| {
                let _ = ui.button("Insert…");
                item(ui, Some(catalogue), upward, &mut Vec::new());
                let _ = ui.button("Eject");
            });
        });
        output.textures_delta.clear();
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        text
    }

    fn at(text: &[(String, egui::Rect)], label: &str) -> Option<egui::Rect> {
        text.iter()
            .find(|(shown, _)| shown == label)
            .map(|(_, rect)| *rect)
    }

    /// The submenu opens beside its menu on the side with room: to the left
    /// at the window's right edge. In a menu that opens upward, as a drive
    /// chip's does on the status bar, it grows upward from its item, also
    /// where there would be room below. Its place and size hold from frame
    /// to frame.
    #[test]
    fn the_submenu_opens_on_the_side_with_room_and_keeps_its_size() {
        let catalogue = catalogue(5, Some(0), None);
        let heading = "B:'s catalogue, in order · 5 disks";
        for (corner, upward, left) in [
            (egui::pos2(20.0, 20.0), false, false),
            (egui::pos2(700.0, 20.0), false, true),
            (egui::pos2(20.0, 330.0), true, false),
            (egui::pos2(20.0, 572.0), true, false),
            (egui::pos2(700.0, 572.0), true, true),
        ] {
            let ctx = egui::Context::default();
            let anchor = egui::Rect::from_min_size(corner, egui::vec2(80.0, 20.0));
            let press = |pressed| egui::Event::PointerButton {
                pos: anchor.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            menu_frame(&ctx, anchor, upward, &catalogue, Vec::new());
            menu_frame(
                &ctx,
                anchor,
                upward,
                &catalogue,
                vec![egui::Event::PointerMoved(anchor.center()), press(true)],
            );
            menu_frame(&ctx, anchor, upward, &catalogue, vec![press(false)]);
            let menu = menu_frame(&ctx, anchor, upward, &catalogue, Vec::new());
            let opener = at(&menu, "Catalogue").expect("the Catalogue item");
            let mut frames = Vec::new();
            for _ in 0..8 {
                let text = menu_frame(
                    &ctx,
                    anchor,
                    upward,
                    &catalogue,
                    vec![egui::Event::PointerMoved(opener.center())],
                );
                frames.push((
                    at(&text, heading),
                    at(&text, "AZURAMON 5 of 18"),
                    at(&text, "Catalogue"),
                ));
            }
            let what = format!("menu at {corner:?}");
            let (Some(top), Some(bottom), Some(item)) = frames[2] else {
                panic!("{what}: the submenu is not shown: {frames:?}");
            };
            assert!(
                frames[2..].iter().all(|frame| *frame == frames[2]),
                "{what}: the submenu moves or grows: {frames:?}"
            );
            if left {
                assert!(top.right() < item.left(), "{what}: {top:?} {item:?}");
            } else {
                assert!(top.left() > item.right(), "{what}: {top:?} {item:?}");
            }
            if upward {
                assert!(top.top() < item.top() - 40.0, "{what}: {top:?} {item:?}");
                assert!(
                    bottom.bottom() <= item.bottom() + 6.0,
                    "{what}: {bottom:?} {item:?}"
                );
            } else {
                assert!(
                    (top.top() - item.top()).abs() < 12.0,
                    "{what}: {top:?} {item:?}"
                );
            }
            let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            assert!(
                window.contains_rect(top) && window.contains_rect(bottom),
                "{what}"
            );
        }
    }

    fn disk(index: usize) -> CatalogueDisk {
        CatalogueDisk {
            name: format!("AZURAMON {} of 18", index + 1),
            format: FloppyGeometry::HD_1_44,
            write: "write-protected".into(),
            inserted: false,
            index,
            set: None,
        }
    }

    fn catalogue(count: usize, in_drive: Option<usize>, pending: Option<Pending>) -> Catalogue {
        Catalogue {
            unit: 1,
            disks: (0..count).map(disk).collect(),
            in_drive,
            pending,
            unknown: None,
            kept: false,
        }
    }

    fn listed(catalogue: &Catalogue) -> Vec<usize> {
        catalogue
            .rows()
            .into_iter()
            .filter_map(|row| match row {
                Row::Disk { index, .. } => Some(index),
                _ => None,
            })
            .collect()
    }

    /// Up to twelve disks are all listed. Past twelve, twelve are: two
    /// before the disk waiting for Start, else the disk in the drive, and
    /// nine after, and All N disks… follows.
    #[test]
    fn a_long_catalogue_lists_twelve_disks_around_the_current_one() {
        let one = catalogue(1, Some(0), None);
        assert_eq!(one.window(), 0..1);
        let twelve = catalogue(12, Some(11), None);
        assert_eq!(twelve.window(), 0..12);
        assert!(!twelve.rows().iter().any(|row| matches!(row, Row::All(_))));
        assert_eq!(
            twelve.rows()[0],
            Row::Heading("B:'s catalogue, in order · 12 disks".into())
        );
        let thirteen = catalogue(13, Some(5), None);
        assert_eq!(listed(&thirteen), (1..13).collect::<Vec<_>>());
        assert_eq!(thirteen.rows().last(), Some(&Row::All(13)));
        assert_eq!(
            thirteen.rows()[0],
            Row::Heading("Disks 2–13 of 13 in B:'s catalogue".into())
        );
        for (in_drive, from) in [
            (None, 0),
            (Some(0), 0),
            (Some(1), 0),
            (Some(3), 1),
            (Some(8), 6),
            (Some(17), 6),
        ] {
            let long = catalogue(18, in_drive, None);
            assert_eq!(long.window(), from..from + CAP, "in the drive {in_drive:?}");
            assert_eq!(long.rows().len(), 1 + CAP + 1);
        }
        // The disk waiting for Start leads the window, not the one in the drive.
        let pending = catalogue(18, Some(2), Some(Pending::Entry(10)));
        assert_eq!(pending.window(), 6..18);
        assert_eq!(listed(&pending), (6..18).collect::<Vec<_>>());
        assert_eq!(pending.focus(), None, "the disk in the drive is not listed");
        let near = catalogue(18, Some(2), Some(Pending::Entry(4)));
        assert_eq!(near.window(), 2..14);
        assert_eq!(near.focus(), Some(2));
    }

    /// ● and "in B:" mark the disk in the drive; the disk waiting for Start
    /// says "at Start"; one that joins the catalogue then is named under
    /// the list.
    #[test]
    fn marks_name_the_disk_in_the_drive_and_the_one_for_start() {
        let rows = catalogue(3, Some(0), Some(Pending::Entry(2))).rows();
        assert_eq!(
            rows[1..],
            [
                Row::Disk {
                    index: 0,
                    name: "AZURAMON 1 of 18".into(),
                    here: true,
                    mark: Mark::InDrive
                },
                Row::Disk {
                    index: 1,
                    name: "AZURAMON 2 of 18".into(),
                    here: false,
                    mark: Mark::None
                },
                Row::Disk {
                    index: 2,
                    name: "AZURAMON 3 of 18".into(),
                    here: false,
                    mark: Mark::AtStart
                },
            ]
        );
        let rows = catalogue(2, None, Some(Pending::Joins("Keen 4".into()))).rows();
        assert_eq!(
            rows.last(),
            Some(&Row::Note(
                "At Start: Keen 4, which joins the catalogue then.".into()
            ))
        );
    }

    /// With one disk or none the item is disabled and says why; a drive
    /// whose disks are not known yet says so in the tooltip instead.
    #[test]
    fn one_disk_or_none_disables_the_item_with_the_reason() {
        assert_eq!(
            catalogue(0, None, None).disabled(),
            Some((
                "Catalogue · empty".into(),
                "B:'s catalogue is empty. Insert… and Create from files… add disks to it.".into()
            ))
        );
        assert_eq!(
            catalogue(1, Some(0), None).disabled(),
            Some((
                "Catalogue · one disk".into(),
                "Only one disk in B:'s catalogue. Insert… adds more.".into()
            ))
        );
        assert_eq!(catalogue(2, None, None).disabled(), None);
        // Disks the drive gets only at Start are not "empty".
        let mut unknown = catalogue(0, None, None);
        unknown.unknown = Some("Not known until Start.".into());
        assert_eq!(
            unknown.disabled(),
            Some((
                "Catalogue · known at Start".into(),
                "Not known until Start.".into()
            ))
        );
    }
}
