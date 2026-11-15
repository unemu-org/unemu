// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless Settings on an image: edit a stopped machine image's hardware as a
//! draft and publish it as a new image revision; machine-images §§6–7 and
//! `backlog/docs/design/mockups/save-as-image.html`.
//!
//! The draft is the image's flattened hardware tables. Every option, limit and
//! reason comes from the capabilities API over those tables; each edit is
//! written into them at once and described again, so what the review shows and
//! Publish writes is exactly what was validated. Disks, firmware and media are
//! never edited here: the revision binds them again by digest.
//!
//! The model never touches the store: the library worker reads the source
//! image and checks the revision, and the model only describes and edits.

use crate::{library_view::ImageRow, save_image::SaveAsImage};
use assembly::capabilities::{self as caps, Draft, Value};
use image_store::Digest;
use pc_image::revision::{Plan, Source};
use std::{cell::RefCell, collections::BTreeMap};
use toml::Table;

/// The review step: the revision the worker checks and the name it is
/// published under. `plan` is `None` until the worker answers.
pub(crate) struct Review {
    pub plan: Option<Result<Plan, String>>,
    pub save: SaveAsImage,
    pub error: Option<String>,
}

/// What the worker needs to check a revision; `revision` matches the answer
/// to the draft it was asked for.
pub(crate) struct PlanRequest {
    pub revision: u64,
    pub machine: Table,
}

/// Diskette choices per drive position: (type, reason it is unavailable).
type FloppyChoices = Vec<(Option<String>, Option<String>)>;

pub(crate) struct ImageSettings {
    pub row: ImageRow,
    source: Source,
    machine: Table,
    pub draft: Draft,
    original: Vec<caps::Field>,
    pub tab: caps::Group,
    pub read_only: bool,
    pub error: Option<String>,
    choices: BTreeMap<String, Vec<caps::Choice>>,
    floppies: RefCell<BTreeMap<u8, FloppyChoices>>,
    pub review: Option<Review>,
    /// Counts committed edits, so a late check of an older draft is ignored.
    revision: u64,
    /// Every local tag, as the worker read them with the source.
    tags: Vec<(String, Digest)>,
    /// Cancel was asked with unsaved edits: confirm before discarding them.
    pub discard_prompt: bool,
}

/// Describe hardware tables through the same lowering as a machine file.
fn describe(machine: &Table) -> Result<Draft, String> {
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
            return Err("Firmware stays as the image stores it; Settings on an image changes hardware only.".into());
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
        })
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

    /// Open the review step; the worker checks the revision with the real ROM
    /// bytes and answers through [`Self::planned`].
    pub fn begin_review(&mut self) -> PlanRequest {
        self.review = Some(Review {
            plan: None,
            save: SaveAsImage::new(
                &self.row.reference,
                self.row.descriptor.digest.clone(),
                self.tags.clone(),
            )
            .publishing(),
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
        if self.dirty() && !self.read_only {
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
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library_test_support::FirmwareStore;
    use pc_image::create::{Chs, Hardware, NewImage, PackageRom, Serial, Video};

    /// A New machine… image of the open fixtures: Kiselev AT BIOS, open VGA
    /// BIOS, XTIDE, a blank C:, drive A: 1.2M and a title.
    pub(crate) fn open_new_image(store: &FirmwareStore, extended_kib: u32) -> NewImage {
        let packages = pc_image::create::packages(&store.store).unwrap();
        let rom = |id: &str| {
            packages
                .iter()
                .find_map(|p| {
                    p.config
                        .roms
                        .iter()
                        .find(|r| r.catalogue_id.as_deref() == Some(id))
                        .map(|r| PackageRom::new(p, r))
                })
                .unwrap()
        };
        NewImage {
            hardware: Hardware {
                board: assembly::BoardModel::Ibm5170,
                rate: (8, 13),
                fpu: false,
                conventional_kib: 640,
                extended_kib,
                video: Video::Vga,
                sound: None,
                serial: Serial::Empty,
                ems: None,
                floppies: [Some("1.2M".into()), None],
                ide_port: 0x1F0,
                disks: [Some(Chs::new(615, 4, 17)), None],
            },
            firmware: vec![
                rom("kiselev-xi8088-1.0.2-atreset"),
                rom("vgabios-0.8a-unemu.1-std"),
                rom("xub-r638-xtp-ata1f0"),
            ],
            title: Some("Open AT workbench".into()),
            description: None,
        }
    }
    /// That image with 1 MiB extended memory, created as open-at:1.0.0.
    pub(crate) fn open_image(store: &FirmwareStore) -> ImageRow {
        let image = open_new_image(store, 1024);
        let descriptor =
            pc_image::create::create(&store.store, &image, "open-at:1.0.0", None).unwrap();
        ImageRow {
            reference: "open-at:1.0.0".into(),
            descriptor,
            summary: None,
            failure: None,
        }
    }

    /// Write `value` into sector 0 of C: through the session's journal.
    pub(crate) fn write_c(prepared: &pc_image::workspace::PreparedSession, value: u8) {
        let drive = prepared
            .machine
            .drives
            .iter()
            .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
            .unwrap();
        let assembly::ImageSource::File(path) = &drive.image else {
            panic!("expected a named base")
        };
        let assembly::WriteMode::Journal { path: journal, .. } = &drive.write else {
            panic!("expected a journal")
        };
        let base = prepared.bases[path].clone();
        let mut media = media::JournalMedia::open_verified_with_metadata(
            journal,
            &base,
            media::SectorSource::byte_len(&base),
            Default::default(),
            None,
        )
        .unwrap();
        media.write_batch(&[(0, [value; 512])]);
        media.finish().unwrap();
    }

    /// Settings as the worker opens them: source and tags read from the store.
    pub(crate) fn opened(store: &FirmwareStore, row: ImageRow, read_only: bool) -> ImageSettings {
        let source = Source::open(&store.store, &row.descriptor).unwrap();
        let tags = crate::save_image::local_tags(&store.store).unwrap();
        ImageSettings::new(row, source, tags, read_only).unwrap()
    }
    /// The review as the worker answers it.
    pub(crate) fn reviewed(model: &mut ImageSettings, store: &image_store::Store) {
        let request = model.begin_review();
        let plan = pc_image::revision::plan(store, &model.source, request.machine)
            .map_err(|e| e.to_string());
        let tags = crate::save_image::local_tags(store).unwrap();
        model.planned(request.revision, plan, tags);
    }

    #[test]
    fn closing_with_edits_asks_first_and_a_stale_check_is_ignored() {
        let store = FirmwareStore::new(false);
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        assert!(model.request_close(), "nothing to lose");
        model.set("memory.extended_kib", Value::Integer(2048));
        assert!(!model.request_close());
        assert!(model.discard_prompt);
        // A check of an older draft never lands in a newer review.
        let stale = model.begin_review();
        model.set("memory.extended_kib", Value::Integer(4096));
        let request = model.begin_review();
        assert_ne!(stale.revision, request.revision);
        model.planned(stale.revision, Err("old".into()), Vec::new());
        assert!(model.review.as_ref().unwrap().plan.is_none());
        model.planned(request.revision, Err("current".into()), Vec::new());
        assert!(model.review.as_ref().unwrap().plan.is_some());
        // Diskette choices are computed once per draft.
        let first = model.floppy_choices(1);
        assert_eq!(model.floppies.borrow().len(), 1);
        assert_eq!(model.floppy_choices(1), first);
        model.set_floppy(1, Some("720K"));
        assert!(model.floppies.borrow().is_empty());
    }

    #[test]
    fn edits_go_through_the_capabilities_api_into_the_hardware_tables() {
        let store = FirmwareStore::new(false);
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        assert!(!model.dirty() && !model.can_review());
        model.set("memory.extended_kib", Value::Integer(4096));
        assert!(model.dirty() && model.can_review());
        assert_eq!(
            model.machine["memory"]["extended_kib"].as_integer(),
            Some(4096)
        );
        let changes = model.changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0.path, "memory.extended_kib");
        assert_eq!(changes[0].1, Some(Value::Integer(1024)));
        // Installing the Sound Blaster adds the card; its defaults come from lowering.
        model.set("card[sb16].enabled", Value::Name("installed".into()));
        assert!(caps::fields(&model.draft)
            .iter()
            .any(|f| f.path == "card[sb16].base" && f.value == Value::Integer(0x220)));
        model.set("card[sb16].enabled", Value::Name("none".into()));
        // Reverting the memory leaves nothing to review.
        model.set("memory.extended_kib", Value::Integer(1024));
        assert!(!model.dirty());
        // Firmware is not edited here.
        model.set("firmware.bios", Value::Disconnected);
        assert!(model.error.as_deref().unwrap().contains("Firmware"));
    }

    #[test]
    fn drive_b_can_be_installed_and_offers_only_valid_types() {
        let store = FirmwareStore::new(false);
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        assert_eq!(model.floppy(0).as_deref(), Some("1.2M"));
        assert_eq!(model.floppy(1), None);
        let choices = model.floppy_choices(1);
        assert_eq!(choices[0], (None, None));
        assert!(choices
            .iter()
            .any(|(kind, reason)| kind.as_deref() == Some("720K") && reason.is_none()));
        model.set_floppy(1, Some("720K"));
        assert_eq!(model.floppy(1).as_deref(), Some("720K"));
        assert!(model.can_review());
        model.set_floppy(1, None);
        assert!(!model.dirty());
    }

    #[test]
    fn running_images_are_read_only_and_invalid_drafts_cannot_review() {
        let store = FirmwareStore::new(false);
        let row = open_image(&store);
        let mut model = opened(&store, row.clone(), true);
        model.set("memory.extended_kib", Value::Integer(4096));
        assert!(!model.dirty() && !model.can_review());
        let mut model = opened(&store, row, false);
        // A resource conflict stays in the draft with the validator's reason.
        model.set("card[lotech-ems].enabled", Value::Name("installed".into()));
        model.set("card[sb16].enabled", Value::Name("installed".into()));
        let port = caps::fields(&model.draft)
            .into_iter()
            .find(|f| f.path.ends_with("lotech-ems].port"))
            .unwrap()
            .value;
        model.set("card[sb16].base", port);
        assert!(model.dirty());
        assert!(model.validation().has_errors());
        assert!(!model.can_review());
    }

    #[test]
    fn review_shows_the_configuration_diff_shared_blobs_and_suggests_the_next_version() {
        let store = FirmwareStore::new(false);
        let row = open_image(&store);
        let mut model = opened(&store, row.clone(), false);
        model.set("memory.extended_kib", Value::Integer(2048));
        reviewed(&mut model, &store.store);
        let review = model.review.as_ref().unwrap();
        let plan = review.plan.as_ref().unwrap().as_ref().unwrap();
        assert!(plan.diff.contains(&pc_image::revision::DiffLine::Added(
            "extended_kib = 2048".into()
        )));
        assert!(
            plan.added.is_empty(),
            "disks, firmware and metadata are shared"
        );
        assert_ne!(plan.machine.0, plan.machine.1);
        assert_eq!(review.save.reference(), "open-at:1.1.0");
        // Another edit drops the review: it described the old draft.
        model.set("memory.extended_kib", Value::Integer(4096));
        assert!(model.review.is_none());
    }
}
