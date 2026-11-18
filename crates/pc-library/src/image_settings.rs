// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless Settings on an image: edit a stopped machine image's hardware as a
//! draft and publish it as a new image revision; machine-images §§6–7 and
//! `backlog/docs/design/mockups/save-as-image.html`.
//!
//! The draft is the image's flattened hardware tables. Every option, limit and
//! reason comes from the capabilities API over those tables; each edit is
//! written into them at once and described again, so what the review shows and
//! Publish writes is exactly what was validated. Disks and media are never
//! edited here: the revision binds them again by digest. Firmware is chosen
//! among the catalogue entries that fit the board
//! (`backlog/docs/design/mockups/firmware-settings.html`): a choice writes the
//! ROM table of a firmware package in the local store, which the revision or
//! the machine binds by digest.
//!
//! The model never touches the store: the library worker reads the source
//! image and checks the revision, and the model only describes and edits.

use crate::firmware_choices::{self as fw, Slot};
use assembly::capabilities::{self as caps, Draft, Value};
use assembly::{CmosLayout, FirmwareEntry};
use image_store::Digest;
use library_model::{listing::ImageRow, save_image::SaveAsImage};
use pc_image::create::{Package, PackageRom};
use pc_image::revision::{Plan, Source};
use std::{cell::RefCell, collections::BTreeMap};
use toml::Table;

/// The review step: the revision the worker checks and the name it is
/// published under. `plan` is `None` until the worker answers.
pub struct Review {
    pub plan: Option<Result<Plan, String>>,
    pub save: SaveAsImage,
    pub error: Option<String>,
}

/// What the worker needs to check a revision; `revision` matches the answer
/// to the draft it was asked for.
pub struct PlanRequest {
    pub revision: u64,
    pub machine: Table,
}

/// Diskette choices per drive position: (type, reason it is unavailable).
type FloppyChoices = Vec<(Option<String>, Option<String>)>;

/// A catalogue entry offered for a firmware slot.
#[derive(Clone, Debug)]
pub struct FirmwareChoice {
    pub entry: &'static FirmwareEntry,
    /// The ROM of a firmware package in the local store with these bytes.
    pub rom: Option<PackageRom>,
    /// Every blob open (from its package, else the catalogue's distribution).
    pub open: bool,
    /// Why it cannot be chosen now; `None` when it can, and for the current one.
    pub reason: Option<String>,
}

/// What a system BIOS change does to the CMOS that the current BIOS wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmosLine {
    /// The same layout: the CMOS stays as it is.
    Same(Option<CmosLayout>),
    /// Another layout, but the image stores no CMOS: Start derives it from
    /// the configuration every time, so nothing is reset.
    Derived(Option<CmosLayout>),
    /// Another layout, or a BIOS the catalogue does not list (`None`), whose
    /// layout nobody knows: the stored CMOS is reset to the bytes derived from
    /// the configuration (a machine at its next start, a revision when
    /// published).
    Reset {
        from: Option<&'static FirmwareEntry>,
        to: Option<&'static FirmwareEntry>,
    },
}

pub struct ImageSettings {
    pub row: ImageRow,
    pub source: Source,
    pub machine: Table,
    pub draft: Draft,
    original: Vec<caps::Field>,
    pub tab: caps::Group,
    pub read_only: bool,
    pub error: Option<String>,
    choices: BTreeMap<String, Vec<caps::Choice>>,
    pub floppies: RefCell<BTreeMap<u8, FloppyChoices>>,
    pub review: Option<Review>,
    /// Counts committed edits, so a late check of an older draft is ignored.
    revision: u64,
    /// Every local tag, as the worker read them with the source.
    tags: Vec<(String, Digest)>,
    /// Cancel was asked with unsaved edits: confirm before discarding them.
    pub discard_prompt: bool,
    /// Settings… on a machine: its id and name. The sheet then keeps the
    /// tables as that machine's settings instead of publishing a revision.
    machine_of: Option<(String, String)>,
    /// The tables the machine keeps now; edits since are unsaved.
    applied: Table,
    /// OK pressed: close once the store has kept the tables.
    pub close_after_apply: bool,
    /// Firmware packages of the local store; `None` until the worker reads them.
    packages: Option<Vec<Package>>,
    /// "Switch to …": the firmware choice to make once the packages are read.
    pending_switch: Option<(Slot, String)>,
}

/// Describe hardware tables through the same lowering as a machine file.
pub fn describe(machine: &Table) -> Result<Draft, String> {
    let text = toml::to_string(&Source::describable(machine)).map_err(|e| e.to_string())?;
    let file = machine_file::parse_toml("image.toml", &text).map_err(|e| e.to_string())?;
    file.describe(&machine_file::Origins::new("image.toml"))
        .map_err(|e| e.to_string())
}

fn toml_value(value: &Value) -> toml::Value {
    match value {
        Value::Integer(value) => i64::from(*value).into(),
        Value::Name(name) | Value::FileName(name) => name.clone().into(),
        Value::Disconnected => "none".into(),
    }
}

fn section<'a>(machine: &'a mut Table, name: &str) -> Result<&'a mut Table, String> {
    machine
        .entry(name)
        .or_insert_with(|| toml::Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| format!("[{name}] must be a table"))
}

/// A card's file identity: its id, else its kind, as the draft names it.
fn card_name(card: &toml::Value) -> Option<&str> {
    card.get("id").or_else(|| card.get("kind"))?.as_str()
}

fn cards(machine: &mut Table) -> Result<&mut Vec<toml::Value>, String> {
    machine
        .entry("card")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| "cards must be an array".to_owned())
}

fn card<'a>(machine: &'a mut Table, name: &str) -> Result<&'a mut Table, String> {
    cards(machine)?
        .iter_mut()
        .find(|card| card_name(card) == Some(name))
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| format!("no card {name} in this image"))
}

/// Write the draft's pending edits into the hardware tables. Field paths are
/// the capabilities API's; their file spelling is the machine-file schema.
fn apply(machine: &mut Table, draft: &Draft) -> Result<(), String> {
    // A card is installed or removed before its fields are written: the
    // draft records the defaults it picks for a new card ahead of the card.
    let mut edits: Vec<_> = draft.edits.iter().collect();
    edits.sort_by_key(|edit| !edit.field_path.ends_with(".enabled"));
    for edit in edits {
        let (path, value) = (edit.field_path.as_str(), &edit.value);
        if path.starts_with("cpu.rate_") {
            let clock = caps::validate_draft(draft).config.clock;
            section(machine, "cpu")?.insert(
                "rate".into(),
                format!(
                    "{}/{}",
                    clock.retired_per_pit_clock, clock.retired_per_pit_clock_denominator
                )
                .into(),
            );
        } else if path.starts_with("firmware.") {
            return Err("Firmware is chosen on the Firmware tab, among the catalogue entries that fit the board.".into());
        } else if let Some(tail) = path.strip_prefix("card[") {
            let (name, suffix) = tail.split_once("].").ok_or("invalid card field")?;
            if suffix == "enabled" {
                let present = cards(machine)?
                    .iter()
                    .any(|card| card_name(card) == Some(name));
                if *value == Value::Name("none".into()) {
                    cards(machine)?.retain(|card| card_name(card) != Some(name));
                } else if !present {
                    cards(machine)?.push(
                        Table::from_iter([("kind".to_owned(), toml::Value::from(name))]).into(),
                    );
                }
            } else if let Some(drive) = suffix.strip_prefix("drives[unit=") {
                let (unit, key) = drive.split_once("].").ok_or("invalid drive field")?;
                let unit: i64 = unit.parse().map_err(|_| "invalid drive unit")?;
                let drive = card(machine, name)?
                    .get_mut("drives")
                    .and_then(toml::Value::as_array_mut)
                    .and_then(|drives| {
                        drives
                            .iter_mut()
                            .find(|d| d.get("unit").and_then(toml::Value::as_integer) == Some(unit))
                    })
                    .and_then(toml::Value::as_table_mut)
                    .ok_or("no such drive in this image")?;
                drive.insert(key.into(), toml_value(value));
            } else {
                card(machine, name)?.insert(suffix.into(), toml_value(value));
            }
        } else if let Some((table, key)) = path.split_once('.') {
            if !matches!(table, "board" | "cpu" | "memory") {
                return Err(format!("{path} cannot change here"));
            }
            section(machine, table)?.insert(key.into(), toml_value(value));
        } else {
            return Err(format!("{path} cannot change here"));
        }
    }
    Ok(())
}

fn floppy_controller(machine: &mut Table) -> Result<&mut Table, String> {
    cards(machine)?
        .iter_mut()
        .find(|card| card.get("kind").and_then(toml::Value::as_str) == Some("fdc-765"))
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| "this image has no diskette controller".to_owned())
}

impl ImageSettings {
    /// Settings over a source image the worker read. `read_only` while the
    /// image runs.
    pub fn new(
        row: ImageRow,
        source: Source,
        tags: Vec<(String, Digest)>,
        read_only: bool,
    ) -> Result<Self, String> {
        let machine = source.machine().clone();
        let applied = machine.clone();
        let draft = describe(&machine)?;
        let original = caps::fields(&draft);
        Ok(Self {
            row,
            source,
            machine,
            draft,
            original,
            tab: caps::Group::Memory,
            read_only,
            error: None,
            choices: BTreeMap::new(),
            floppies: RefCell::default(),
            review: None,
            revision: 0,
            tags,
            discard_prompt: false,
            applied,
            machine_of: None,
            close_after_apply: false,
            packages: None,
            pending_switch: None,
        })
    }

    /// The sheet of Settings… on a machine, over the tables it runs with.
    /// Differences stay marked against its image.
    pub fn for_machine(mut self, id: String, name: String, machine: Table) -> Self {
        self.machine_of = Some((id, name));
        self.applied = machine.clone();
        if machine != self.machine {
            self.commit(machine);
        }
        self
    }
    pub fn machine_id(&self) -> Option<&str> {
        self.machine_of.as_ref().map(|(id, _)| id.as_str())
    }
    pub fn machine_name(&self) -> Option<&str> {
        self.machine_of.as_ref().map(|(_, name)| name.as_str())
    }
    /// Edits to a machine's settings that the store does not keep yet.
    pub fn unsaved(&self) -> bool {
        self.machine != self.applied
    }
    /// Whether OK or Apply may keep the draft as the machine's settings.
    pub fn can_apply(&self) -> bool {
        !self.read_only && self.error.is_none() && self.unsaved() && !self.validation().has_errors()
    }
    /// The machine and tables OK (`close`) or Apply keeps.
    pub fn apply_request(&mut self, close: bool) -> Option<(String, Table)> {
        if !self.can_apply() {
            return None;
        }
        self.close_after_apply = close;
        Some((self.machine_id()?.to_owned(), self.machine.clone()))
    }
    /// The store kept the draft.
    pub fn applied(&mut self) {
        self.applied = self.machine.clone();
    }

    fn commit(&mut self, machine: Table) {
        match describe(&machine) {
            Ok(draft) => {
                self.machine = machine;
                self.draft = draft;
                self.choices.clear();
                self.floppies.borrow_mut().clear();
                self.review = None;
                self.error = None;
                self.revision += 1;
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// One capabilities edit, written into the hardware tables at once.
    pub fn set(&mut self, path: &str, value: Value) {
        if self.read_only {
            return;
        }
        let mut draft = self.draft.clone();
        draft.set(path, value);
        let mut machine = self.machine.clone();
        match apply(&mut machine, &draft) {
            Ok(()) => self.commit(machine),
            Err(error) => self.error = Some(error),
        }
    }

    pub fn choices(&mut self, path: &str) -> &[caps::Choice] {
        self.choices
            .entry(path.into())
            .or_insert_with(|| caps::choices(&self.draft, path))
    }

    pub fn validation(&self) -> caps::Validation {
        caps::validate_draft(&self.draft)
    }

    /// Fields whose value differs from the image, with the image's value.
    pub fn changes(&self) -> Vec<(caps::Field, Option<Value>)> {
        caps::fields(&self.draft)
            .into_iter()
            .filter_map(|field| {
                let old = self.original.iter().find(|old| old.path == field.path);
                (old.map(|old| &old.value) != Some(&field.value))
                    .then(|| (field, old.map(|old| old.value.clone())))
            })
            .collect()
    }

    pub fn dirty(&self) -> bool {
        &self.machine != self.source.machine()
    }

    /// Whether Review changes… may open: stopped, changed and valid.
    pub fn can_review(&self) -> bool {
        !self.read_only && self.error.is_none() && self.dirty() && !self.validation().has_errors()
    }

    /// The diskette type of `unit`, or `None` without a drive.
    pub fn floppy(&self, unit: u8) -> Option<String> {
        self.draft
            .config
            .drives
            .iter()
            .find_map(|drive| match drive.kind {
                assembly::DriveKind::Floppy { geometry }
                    if drive.controller == assembly::DeviceId::Fdc && drive.unit == unit =>
                {
                    assembly::descriptors::FLOPPY_TYPES
                        .iter()
                        .find(|kind| kind.geometry == geometry)
                        .map(|kind| kind.name.to_owned())
                }
                _ => None,
            })
    }

    fn with_floppy(&self, unit: u8, kind: Option<&str>) -> Result<Table, String> {
        let mut machine = self.machine.clone();
        let fdc = floppy_controller(&mut machine)?;
        let drives = fdc
            .entry("drives")
            .or_insert_with(|| toml::Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or("drives must be an array")?;
        let unit_of = |d: &toml::Value| d.get("unit").and_then(toml::Value::as_integer);
        drives.retain(|d| unit_of(d) != Some(i64::from(unit)));
        if let Some(kind) = kind {
            drives.push(
                Table::from_iter([
                    ("unit".to_owned(), toml::Value::from(i64::from(unit))),
                    ("type".to_owned(), kind.into()),
                ])
                .into(),
            );
            drives.sort_by_key(|d| unit_of(d).unwrap_or(i64::MAX));
        }
        if drives.is_empty() {
            fdc.remove("drives");
        }
        Ok(machine)
    }

    /// Choices for a diskette drive position: not installed, or each drive
    /// type with the capabilities API's reason when it is unavailable.
    pub fn floppy_choices(&self, unit: u8) -> FloppyChoices {
        if let Some(choices) = self.floppies.borrow().get(&unit) {
            return choices.clone();
        }
        let choices = self.floppy_choices_uncached(unit);
        self.floppies.borrow_mut().insert(unit, choices.clone());
        choices
    }
    fn floppy_choices_uncached(&self, unit: u8) -> FloppyChoices {
        let mut choices = vec![(None, None)];
        let candidate = self
            .with_floppy(unit, Some(assembly::descriptors::FLOPPY_TYPES[0].name))
            .and_then(|machine| describe(&machine));
        let Ok(candidate) = candidate else {
            return choices;
        };
        let path = format!("card[fdc-765].drives[unit={unit}].type");
        let path = caps::fields(&candidate)
            .into_iter()
            .find(|field| {
                field.path.ends_with(&format!(".drives[unit={unit}].type"))
                    && field.group == caps::Group::Storage
            })
            .map_or(path, |field| field.path);
        for choice in caps::choices(&candidate, &path) {
            if let Value::Name(name) = choice.value {
                choices.push((
                    Some(name),
                    (!choice.enabled).then_some(choice.reason).flatten(),
                ));
            }
        }
        choices
    }

    /// Install, change or remove a diskette drive. A drive added here starts
    /// with an empty media catalogue; a removed drive's catalogue leaves the image.
    pub fn set_floppy(&mut self, unit: u8, kind: Option<&str>) {
        if self.read_only {
            return;
        }
        match self.with_floppy(unit, kind) {
            Ok(machine) => self.commit(machine),
            Err(error) => self.error = Some(error),
        }
    }

    /// The source is an official template: a repository of the official
    /// namespace, pulled into the store under its template name, with the
    /// template's title, from whichever registry (its short form decides).
    /// `unemu/` is read-only for the user, so a revision of it is written to
    /// the local store only.
    pub fn official_template(&self) -> bool {
        let repo = image_registry::reference::short_name(self.row.repository());
        pc_image::template::TEMPLATES.iter().any(|t| t.name == repo)
    }
    /// The release a local revision of the source takes its suffix after:
    /// the source's own version tag, else the repository's newest version
    /// (a source tagged only `latest`), else 1.0.0, each without an earlier
    /// `-local.N`.
    fn source_version(&self) -> String {
        let prefix = format!("{}:", self.row.repository());
        let release = |tag: &str| -> String {
            match tag.split_once("-local.") {
                Some((release, n)) if n.chars().all(|c| c.is_ascii_digit()) => release.into(),
                _ => tag.into(),
            }
        };
        let versions = |own: bool| {
            self.tags
                .iter()
                .filter(move |(_, digest)| !own || *digest == self.row.descriptor.digest)
                .filter_map(|(name, _)| name.strip_prefix(&prefix))
                .filter(|tag| image_store::Store::is_version_tag(tag))
                .map(release)
                .max_by_key(|tag| pc_image::template::precedence(tag))
        };
        versions(true)
            .or_else(|| versions(false))
            .unwrap_or_else(|| "1.0.0".into())
    }

    /// Open the review step; the worker checks the revision with the real ROM
    /// bytes and answers through [`Self::planned`]. The name is a new tag:
    /// the next minor version, or for an official template its version with
    /// a local suffix.
    pub fn begin_review(&mut self) -> PlanRequest {
        let official = self.official_template().then(|| self.source_version());
        self.review = Some(Review {
            plan: None,
            save: SaveAsImage::new(
                &self.row.reference,
                self.row.descriptor.digest.clone(),
                self.tags.clone(),
            )
            .publishing()
            .official(official),
            error: None,
        });
        PlanRequest {
            revision: self.revision,
            machine: self.machine.clone(),
        }
    }

    /// The worker's check of the draft at `revision`, with fresh local tags.
    pub fn planned(
        &mut self,
        revision: u64,
        plan: Result<Plan, String>,
        tags: Vec<(String, Digest)>,
    ) {
        if revision != self.revision {
            return;
        }
        self.tags = tags.clone();
        if let Some(review) = &mut self.review {
            review.plan = Some(plan);
            review.save.tags = tags;
        }
    }

    /// The store refused the publication: ask again with its current tags.
    pub fn refused(&mut self, error: String, tags: Vec<(String, Digest)>) {
        self.tags = tags.clone();
        if let Some(review) = &mut self.review {
            review.error = Some(error);
            review.save.refresh(tags);
        }
    }

    /// Cancel, Escape or ×: with unsaved edits, ask first. Returns whether
    /// the sheet may close now.
    pub fn request_close(&mut self) -> bool {
        let unsaved = if self.machine_of.is_some() {
            self.unsaved()
        } else {
            self.dirty()
        };
        if unsaved && !self.read_only {
            self.discard_prompt = true;
            false
        } else {
            true
        }
    }

    /// Whether the source image's own tag can move (it is not a version).
    pub fn source_tag_moves(&self) -> bool {
        self.row
            .reference
            .rsplit_once(':')
            .is_some_and(|(_, tag)| !image_store::Store::is_version_tag(tag))
    }

    /// Whether the source stores a CMOS image (otherwise Start derives it).
    pub fn stores_cmos(&self) -> bool {
        self.source.stores_cmos()
    }

    // ---- firmware ----------------------------------------------------------

    /// The local store's firmware packages, as the worker read them. A
    /// "Switch to …" waiting for them is made now.
    pub fn set_packages(&mut self, packages: Vec<Package>) {
        self.packages = Some(packages);
        self.choices.clear();
        self.try_switch();
    }
    pub fn packages_read(&self) -> bool {
        self.packages.is_some()
    }

    /// "Switch to …" from a details pane: the Firmware tab with that choice
    /// made, so the CMOS line and any conflict show before OK.
    pub fn switch_to(&mut self, slot: Slot, id: &str) {
        self.tab = caps::Group::Firmware;
        self.pending_switch = Some((slot, id.to_owned()));
        self.try_switch();
    }
    /// Make a waiting "Switch to …" once its ROM can be bound: at once, or
    /// after Import ROM folder… brought it into the store, when its error goes.
    fn try_switch(&mut self) {
        let Some((slot, id)) = self.pending_switch.clone() else {
            return;
        };
        if self.packages.is_none() {
            return;
        }
        if self.firmware_available(slot, &id) {
            self.pending_switch = None;
            self.error = None;
            self.set_firmware(slot, &id);
        } else {
            self.error = Some(format!(
                "{} is not in your store: Import ROM folder… adds it, and the switch is made then.",
                fw::entry(&id).map_or_else(|| id.clone(), fw::name)
            ));
        }
    }
    /// Whether an entry can be bound now: the image's or the applied tables
    /// hold it, or a firmware package of the store does.
    fn firmware_available(&self, slot: Slot, id: &str) -> bool {
        [self.source.machine(), &self.applied]
            .into_iter()
            .any(|tables| fw::current(tables, slot).is_some_and(|e| e.id == id))
            || fw::entry(id).is_some_and(|entry| {
                fw::package_rom(self.packages.as_deref().unwrap_or_default(), entry, slot).is_some()
            })
    }

    /// The draft's hardware tables.
    pub fn machine(&self) -> &Table {
        &self.machine
    }
    /// The board's name as the capabilities API labels it.
    pub fn board_label(&self) -> String {
        let board = self.draft.config.board;
        caps::value_label(&self.draft, "board.model", &Value::Name(board.id().into()))
    }

    /// The entry the draft binds in a slot.
    pub fn firmware(&self, slot: Slot) -> Option<&'static FirmwareEntry> {
        fw::current(&self.machine, slot)
    }
    /// The entry the image binds in a slot.
    pub fn image_firmware(&self, slot: Slot) -> Option<&'static FirmwareEntry> {
        fw::current(self.source.machine(), slot)
    }
    /// The tables whose system BIOS wrote the CMOS: the machine's applied
    /// tables, or the image's.
    fn cmos_writer(&self) -> &Table {
        if self.machine_of.is_some() {
            &self.applied
        } else {
            self.source.machine()
        }
    }

    /// Whether the machine's video card has a BIOS of its own: a
    /// register-level VGA. A text-only card is driven by the system BIOS.
    pub fn has_video_bios(&self) -> bool {
        self.firmware(Slot::Video).is_some()
            || self
                .draft
                .config
                .devices
                .iter()
                .any(|d| matches!(d.kind, assembly::DeviceKind::VgaRegisters(_)))
    }

    /// The capabilities path of a slot in this draft.
    fn firmware_path(&self, slot: Slot) -> String {
        match slot {
            Slot::System => "firmware.bios".into(),
            Slot::Video => caps::canonical_field_path(&self.draft, "firmware.video"),
        }
    }

    /// The catalogue entries that fit the board for a slot, each with the
    /// package ROM of the store that holds it and why it cannot be chosen
    /// now. System BIOS entries come grouped by family (one CMOS layout per
    /// family); a superseded entry follows the releases of its family.
    pub fn firmware_choices(&mut self, slot: Slot) -> Vec<FirmwareChoice> {
        let board = self.draft.config.board;
        let current = self.firmware(slot).map(|e| e.id);
        let path = self.firmware_path(slot);
        let verdicts = self.choices(&path).to_vec();
        let packages = self.packages.as_deref().unwrap_or_default();
        let mut list: Vec<(usize, FirmwareChoice)> = fw::catalogue()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == slot.kind() && e.boards.contains(&board.id()))
            .map(|(index, entry)| {
                let rom = fw::package_rom(packages, entry, slot);
                let open = rom
                    .as_ref()
                    .map_or_else(|| fw::catalogue_open(entry), fw::rom_open);
                let reason = if Some(entry.id) == current {
                    None
                } else if self.packages.is_none() {
                    Some("the firmware packages of your store are being read.".into())
                } else if !self.firmware_available(slot, entry.id) {
                    // Not when the image's or the applied tables hold the entry:
                    // set_firmware binds it from them, so no package is needed.
                    Some(if fw::catalogue_open(entry) {
                        "not in your store: Import ROM folder… adds it from a folder you choose.".to_owned()
                    } else {
                        "not in your store; user-supplied ROMs are never downloaded, so Import ROM folder… adds your own dump.".to_owned()
                    })
                } else {
                    verdicts
                        .iter()
                        .find(|c| c.value == Value::Name(entry.id.into()))
                        .filter(|c| !c.enabled)
                        .and_then(|c| c.reason.clone())
                };
                (
                    index,
                    FirmwareChoice {
                        entry,
                        rom,
                        open,
                        reason,
                    },
                )
            })
            .collect();
        list.sort_by_key(|(index, c)| {
            (c.entry.cmos_layout, c.entry.superseded_by.is_some(), *index)
        });
        list.into_iter().map(|(_, c)| c).collect()
    }

    /// Choose a catalogue entry for a slot. The image's (or, on a machine,
    /// the applied) table comes back as it was; another entry takes the ROM
    /// table of its package in the store.
    pub fn set_firmware(&mut self, slot: Slot, id: &str) {
        if self.read_only {
            return;
        }
        let kept = [self.source.machine(), &self.applied]
            .into_iter()
            .find_map(|tables| {
                fw::rom_table(tables, slot)
                    .filter(|_| fw::current(tables, slot).is_some_and(|e| e.id == id))
                    .cloned()
            });
        let table = match kept {
            Some(table) => Ok(table),
            None => {
                let rom = fw::entry(id).and_then(|entry| {
                    fw::package_rom(self.packages.as_deref().unwrap_or_default(), entry, slot)
                });
                match rom {
                    Some(rom) => fw::package_table(&rom),
                    None => Err(format!(
                        "{} is not in your store; import its ROM folder first.",
                        fw::entry(id).map_or_else(|| id.to_owned(), fw::name)
                    )),
                }
            }
        };
        let mut machine = self.machine.clone();
        match table.and_then(|table| fw::replace(&mut machine, slot, table)) {
            Ok(()) => self.commit(machine),
            Err(error) => self.error = Some(error),
        }
    }

    /// What the draft's system BIOS does to the CMOS the current one wrote;
    /// `None` while it is the same BIOS.
    pub fn cmos_line(&self) -> Option<CmosLine> {
        let sha256 = |tables: &Table| {
            fw::rom_table(tables, Slot::System)
                .and_then(|rom| rom.get("sha256"))
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        };
        let writer = self.cmos_writer();
        if sha256(writer) == sha256(&self.machine) {
            return None;
        }
        let (from, to) = (
            fw::current(writer, Slot::System),
            self.firmware(Slot::System),
        );
        let layout = |entry: Option<&FirmwareEntry>| entry.and_then(|e| e.cmos_layout);
        // An unlisted BIOS keeps CMOS in a layout nobody knows: as pc-image
        // does, only two catalogued BIOSes of one layout keep it.
        let same = from.is_some() && to.is_some() && layout(from) == layout(to);
        Some(if same {
            CmosLine::Same(layout(to))
        } else if !self.stores_cmos() {
            CmosLine::Derived(layout(to))
        } else {
            CmosLine::Reset { from, to }
        })
    }

    /// User-supplied firmware the draft takes that the image does not have:
    /// a later image of it (Save as new image… or this revision) can be
    /// pushed only to a private namespace.
    pub fn new_user_supplied(&self) -> Vec<&'static FirmwareEntry> {
        let packages = self.packages.as_deref().unwrap_or_default();
        Slot::ALL
            .into_iter()
            .filter_map(|slot| {
                let entry = self.firmware(slot)?;
                let open = fw::package_rom(packages, entry, slot)
                    .as_ref()
                    .map_or_else(|| fw::catalogue_open(entry), fw::rom_open);
                (!open && self.image_firmware(slot).is_none_or(|e| e.id != entry.id))
                    .then_some(entry)
            })
            .collect()
    }
}
