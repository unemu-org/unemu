// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless File › New machine… wizard; machine-images §§2, 5.3 and 6.
//!
//! The wizard keeps the user's selections and writes them as a draft machine
//! document. Every option, limit and why-disabled reason comes from the
//! capabilities API over that document (the same one the Settings sheet uses):
//! scalar fields use its choices, structural choices such as a card or a disk
//! are candidate documents run through the same validator. Firmware fit comes
//! from package fit claims and the catalogue entries the packages name. The
//! approved design is `backlog/docs/design/mockups/new-machine.html`.

use assembly::capabilities::{self as caps, Choice, Domain, Draft, Severity, Value};
use image_store::{firmware::Rom, Digest, Store};
use pc_image::create::{
    self, Chs, Ems, Hardware, NewImage, Package, PackageRom, Sb16, Serial, Video,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

pub(crate) const STEPS: [&str; 7] = [
    "Machine class",
    "Motherboard",
    "CPU and memory",
    "Video, sound and ports",
    "Drives",
    "Name and tag",
    "Summary",
];
pub(crate) const MOTHERBOARD: usize = 1;
pub(crate) const CPU_MEMORY: usize = 2;
pub(crate) const DEVICES: usize = 3;
pub(crate) const DRIVES: usize = 4;
pub(crate) const NAME: usize = 5;
pub(crate) const SUMMARY: usize = 6;

/// A firmware picker: which package role it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Bios,
    Video,
    OptionRom,
}
impl Slot {
    fn role(self) -> &'static str {
        match self {
            Self::Bios => "system-bios",
            Self::Video => "video-bios",
            Self::OptionRom => "option-rom",
        }
    }
    pub fn index(self) -> usize {
        self as usize
    }
}

/// A selected ROM: its package manifest digest and logical ROM id.
pub(crate) type RomRef = (Digest, String);

/// One package ROM offered by a picker, with the reasons it does not fit.
#[derive(Clone, Debug)]
pub(crate) struct Offer {
    pub rom: RomRef,
    pub name: String,
    pub vendor: String,
    pub detail: String,
    pub layout: String,
    pub open: bool,
    pub reasons: Vec<String>,
}

/// The selections that determine the machine document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub class: &'static str,
    pub hardware: Hardware,
    pub bios: Option<RomRef>,
    pub video_bios: Option<RomRef>,
    pub option_rom: Option<RomRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagLevel {
    Ok,
    Warn,
    Bad,
}
/// The result of checking the repository name and tag against the store rules.
#[derive(Clone, Debug)]
pub(crate) struct TagCheck {
    pub level: TagLevel,
    pub text: String,
    /// A suggested free version tag after a refusal.
    pub fix: Option<String>,
    /// The digest an existing movable tag names; Create asks before moving it.
    pub moving: Option<Digest>,
}

/// A prospective manifest, or why the ROM bytes refuse the machine.
pub(crate) type Preview = Result<Rc<image_store::Manifest>, String>;

#[derive(Default)]
struct Cache {
    revision: u64,
    draft: Option<Result<Rc<Draft>, String>>,
    errors: Option<Rc<Vec<caps::Diagnostic>>>,
    /// The manifest Create would write, from the real ROM bytes.
    preview: Option<Option<Preview>>,
    choices: BTreeMap<String, Rc<Vec<Choice>>>,
    candidates: BTreeMap<String, Option<String>>,
    fits: BTreeMap<String, Vec<String>>,
    board: Option<BoardVerdicts>,
}
/// The bare board's System BIOS choices and its own validator errors.
type BoardVerdicts = (Rc<Vec<Choice>>, Rc<Vec<caps::Diagnostic>>);

/// ROM bytes read once from the store, for previews on the UI thread. The
/// store still verifies every byte again when Create runs.
struct Roms<'a> {
    store: &'a Store,
    cache: &'a RefCell<BTreeMap<Digest, Vec<u8>>>,
}
impl image_store::BlobSource for Roms<'_> {
    fn read_verified(
        &self,
        descriptor: &image_store::Descriptor,
        limit: u64,
    ) -> image_store::Result<Vec<u8>> {
        if let Some(bytes) = self.cache.borrow().get(&descriptor.digest) {
            return Ok(bytes.clone());
        }
        let bytes = self.store.read_verified(descriptor, limit)?;
        self.cache
            .borrow_mut()
            .insert(descriptor.digest.clone(), bytes.clone());
        Ok(bytes)
    }
}

pub(crate) struct Wizard {
    pub step: usize,
    pub visited: usize,
    selection: Selection,
    revision: u64,
    video_touched: bool,
    pub filter: String,
    pub show_incompatible: [bool; 3],
    pub repo: String,
    repo_edited: bool,
    pub tag: String,
    pub title: String,
    /// Firmware packages of the local store; `None` while they are read.
    pub packages: Option<Vec<Package>>,
    /// Every local tag, for the tag rules.
    pub tags: Vec<(String, Digest)>,
    /// Package ROM blobs some local manifest marks user-supplied. The most
    /// restrictive mark for a digest wins, exactly as in the push guard.
    pub restricted: BTreeSet<Digest>,
    /// Every local manifest's marks, for the prospective image's closure.
    pub marks: image_registry::guard::LocalMarks,
    /// The local store, read for ROM bytes when previewing an image.
    pub store: Option<Store>,
    roms: RefCell<BTreeMap<Digest, Vec<u8>>>,
    /// Descriptor defaults for re-enabling the Sound Blaster and EMS cards.
    pub sb16_default: Sb16,
    pub ems_default: Ems,
    pub custom: [bool; 2],
    pub error: Option<String>,
    pub busy: bool,
    /// The user confirmed moving this tag from the named digest.
    pub confirmed_move: Option<Digest>,
    /// The Move tag? confirmation is showing.
    pub move_prompt: bool,
    /// Import ROM folder… is possible now (a stopped machine, an idle store).
    pub can_import_roms: bool,
    cache: RefCell<Cache>,
}

fn errors_in(diagnostics: &[caps::Diagnostic]) -> Vec<caps::Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .cloned()
        .collect()
}
fn default_of(draft: &Draft, path: &str) -> Option<u32> {
    match caps::describe(draft, path)?.default? {
        Value::Integer(n) => Some(n),
        _ => None,
    }
}

impl Wizard {
    pub fn new() -> Self {
        let board = assembly::BoardModel::Ibm5170;
        let speed = create::speeds(board).first().copied();
        // Defaults come from the capabilities descriptors of an AT board.
        let draft = Draft::for_board(board);
        let mut probe = draft.clone();
        probe.add_card("sb16".into(), assembly::descriptors::CardKind::Sb16);
        probe.add_card(
            "lotech-ems".into(),
            assembly::descriptors::CardKind::LotechEms,
        );
        let sb16 = Sb16 {
            base: default_of(&probe, "card[sb16].base").unwrap_or(0x220) as u16,
            irq: default_of(&probe, "card[sb16].irq").unwrap_or(5) as u8,
            dma8: default_of(&probe, "card[sb16].dma8").unwrap_or(1) as u8,
            dma16: default_of(&probe, "card[sb16].dma16").unwrap_or(5) as u8,
        };
        let ems = Ems {
            port: default_of(&probe, "card[lotech-ems].port").unwrap_or(0x260) as u16,
            // The frame has no descriptor default; the open AT profile uses D000h
            // and `sanitize` moves it when the validator refuses it.
            frame: 0xD000,
            size_kib: default_of(&probe, "card[lotech-ems].size_kib").unwrap_or(2048),
        };
        let hardware = Hardware {
            board,
            rate: speed.map_or((8, 13), |s| (s.steps, s.pit_clocks)),
            fpu: false,
            conventional_kib: default_of(&draft, "memory.conventional_kib").unwrap_or(640),
            // 1 MiB keeps the POST memory test short.
            extended_kib: 1024,
            video: Video::Vga,
            sound: Some(sb16),
            serial: Serial::Mouse,
            ems: Some(ems),
            floppies: [Some("1.2M".into()), None],
            ide_port: assembly::descriptors::IDE_PORTS[0],
            disks: [Some(create::BLANK_DISKS[0]), None],
        };
        Self {
            step: 0,
            visited: 0,
            selection: Selection {
                class: "at",
                hardware,
                bios: None,
                video_bios: None,
                option_rom: None,
            },
            revision: 1,
            video_touched: false,
            filter: String::new(),
            show_incompatible: [false; 3],
            repo: "open-at".into(),
            repo_edited: false,
            tag: "1.0.0".into(),
            title: String::new(),
            packages: None,
            tags: Vec::new(),
            restricted: BTreeSet::new(),
            marks: Default::default(),
            store: None,
            roms: RefCell::new(BTreeMap::new()),
            sb16_default: sb16,
            ems_default: ems,
            custom: [false; 2],
            error: None,
            busy: false,
            confirmed_move: None,
            move_prompt: false,
            can_import_roms: true,
            cache: RefCell::new(Cache::default()),
        }
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }
    pub fn hardware(&self) -> &Hardware {
        &self.selection.hardware
    }
    /// Change the selections; every derived option is recomputed afterwards.
    pub fn edit(&mut self, change: impl FnOnce(&mut Selection)) {
        let before = self.selection.clone();
        change(&mut self.selection);
        if self.selection != before {
            self.revision += 1;
            self.confirmed_move = None;
        }
    }
    pub fn set_repo(&mut self, repo: String) {
        self.repo = repo;
        self.repo_edited = true;
        self.confirmed_move = None;
    }
    pub fn set_tag(&mut self, tag: String) {
        self.tag = tag;
        self.confirmed_move = None;
    }

    /// Install the store's packages and tags. The BIOS defaults are applied
    /// only when no chosen BIOS exists (first load, or it disappeared); a
    /// refresh keeps the user's video BIOS, option ROM and name.
    pub fn set_store(&mut self, choices: crate::library_worker::StoreChoices) {
        let (packages, tags, marks) = choices;
        // Compressed ROMs are keyed by their decoded identity, as package
        // configs name them and as the push guard keys its marks.
        self.restricted = packages
            .iter()
            .flat_map(|p| image_registry::guard::closure(&p.manifest, &marks))
            .filter(|c| c.effective == image_registry::guard::Redistribution::UserSupplied)
            .filter_map(|c| c.descriptor.decoded_digest().ok())
            .collect();
        self.packages = Some(packages);
        self.tags = tags;
        self.marks = marks;
        self.revision += 1;
        if self.bios_rom().is_none() {
            self.choose_board(self.selection.hardware.board);
        }
    }

    pub fn go(&mut self, step: usize) {
        self.step = step.min(STEPS.len() - 1);
        self.visited = self.visited.max(self.step);
    }

    fn refresh(&self) {
        let mut cache = self.cache.borrow_mut();
        if cache.revision != self.revision {
            *cache = Cache {
                revision: self.revision,
                ..Cache::default()
            };
        }
    }

    // ---- package lookups -------------------------------------------------

    fn package(&self, digest: &Digest) -> Option<&Package> {
        self.packages
            .iter()
            .flatten()
            .find(|p| &p.descriptor.digest == digest)
    }
    fn package_rom(&self, rom: &Option<RomRef>) -> Option<PackageRom> {
        let (digest, id) = rom.as_ref()?;
        let package = self.package(digest)?;
        let rom = package.config.roms.iter().find(|r| &r.id == id)?;
        Some(PackageRom::new(package, rom))
    }
    pub fn bios_rom(&self) -> Option<PackageRom> {
        self.package_rom(&self.selection.bios)
    }
    pub fn video_rom(&self) -> Option<PackageRom> {
        if self.selection.hardware.video != Video::Vga {
            return None;
        }
        self.package_rom(&self.selection.video_bios)
    }
    pub fn option_rom(&self) -> Option<PackageRom> {
        self.package_rom(&self.selection.option_rom)
    }

    /// The image the selections describe. Without a BIOS no firmware is bound.
    fn image_of(&self, selection: &Selection) -> NewImage {
        let mut firmware = Vec::new();
        if let Some(bios) = self.package_rom(&selection.bios) {
            firmware.push(bios);
            if selection.hardware.video == Video::Vga {
                firmware.extend(self.package_rom(&selection.video_bios));
            }
            firmware.extend(self.package_rom(&selection.option_rom));
        }
        NewImage {
            hardware: selection.hardware.clone(),
            firmware,
            title: Some(self.title.trim().to_owned()).filter(|t| !t.is_empty()),
        }
    }
    pub fn image(&self) -> NewImage {
        self.image_of(&self.selection)
    }

    // ---- capabilities ----------------------------------------------------

    fn describe(image: &NewImage) -> Result<Draft, String> {
        let mut document = image.document().map_err(|e| e.to_string())?;
        // A new image's floppy drives start with empty catalogues; describing
        // them so keeps drive-type choices independent of any medium size.
        for card in document
            .get_mut("card")
            .and_then(toml::Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if card.get("kind").and_then(toml::Value::as_str) != Some("fdc-765") {
                continue;
            }
            for drive in card
                .get_mut("drives")
                .and_then(toml::Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(drive) = drive.as_table_mut() {
                    drive.insert("media".into(), toml::Value::Array(Vec::new()));
                    drive.insert("empty".into(), true.into());
                }
            }
        }
        let text = toml::to_string(&document).map_err(|e| e.to_string())?;
        let file =
            machine_file::parse_toml("new-machine.toml", &text).map_err(|e| e.to_string())?;
        file.describe(&machine_file::Origins::new("new-machine.toml"))
            .map_err(|e| e.to_string())
    }
    fn errors_of(image: &NewImage) -> Result<Vec<caps::Diagnostic>, String> {
        let draft = Self::describe(image)?;
        Ok(caps::validate_draft(&draft)
            .diagnostics
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect())
    }
    /// The capabilities draft of the current selections.
    pub fn draft(&self) -> Result<Rc<Draft>, String> {
        self.refresh();
        let mut cache = self.cache.borrow_mut();
        cache
            .draft
            .get_or_insert_with(|| Self::describe(&self.image()).map(Rc::new))
            .clone()
    }
    /// Validator errors of the current selections.
    pub fn errors(&self) -> Rc<Vec<caps::Diagnostic>> {
        self.refresh();
        if let Some(errors) = self.cache.borrow().errors.clone() {
            return errors;
        }
        let errors = Rc::new(match self.draft() {
            Ok(draft) => caps::validate_draft(&draft)
                .diagnostics
                .into_iter()
                .filter(|d| d.severity == Severity::Error)
                .collect(),
            Err(_) => Vec::new(),
        });
        self.cache.borrow_mut().errors = Some(errors.clone());
        errors
    }
    /// The API's choices for a field of the current draft, with reasons.
    pub fn choices(&self, path: &str) -> Rc<Vec<Choice>> {
        self.refresh();
        if let Some(choices) = self.cache.borrow().choices.get(path) {
            return choices.clone();
        }
        let choices = Rc::new(
            self.draft()
                .map(|draft| caps::choices(&draft, path))
                .unwrap_or_default(),
        );
        self.cache
            .borrow_mut()
            .choices
            .insert(path.into(), choices.clone());
        choices
    }
    /// The integer domain of a field, for limits shown beside numeric inputs.
    pub fn domain(&self, path: &str) -> Option<assembly::descriptors::IntegerDomain> {
        match caps::describe(&*self.draft().ok()?, path)?.domain {
            Domain::Integer(domain) => Some(domain),
            Domain::Values(_) => None,
        }
    }
    /// Why a structural change would be refused: the first validator error the
    /// candidate document has that the current one lacks. `None` means allowed.
    pub fn candidate(&self, key: &str, change: impl FnOnce(&mut Selection)) -> Option<String> {
        self.refresh();
        if let Some(reason) = self.cache.borrow().candidates.get(key) {
            return reason.clone();
        }
        let mut selection = self.selection.clone();
        change(&mut selection);
        let reason = if selection == self.selection {
            None
        } else {
            let baseline = self.errors();
            let image = self.image_of(&selection);
            match Self::errors_of(&image) {
                Ok(errors) => errors
                    .into_iter()
                    .find(|d| !baseline.contains(d))
                    .map(|d| d.reason)
                    .or_else(|| self.byte_reason(&image)),
                Err(error) => Some(error),
            }
        };
        self.cache
            .borrow_mut()
            .candidates
            .insert(key.into(), reason.clone());
        reason
    }

    /// What only the ROM bytes decide, such as CMOS-typed fixed disks: the
    /// candidate's error when the current selection has none or another one.
    fn byte_reason(&self, image: &NewImage) -> Option<String> {
        let store = self.store.as_ref()?;
        if image.firmware.is_empty() {
            return None;
        }
        let roms = Roms {
            store,
            cache: &self.roms,
        };
        let error = pc_image::create::check(image, &roms).err()?.to_string();
        let current = match self.preview() {
            Some(Err(current)) => Some(current),
            _ => None,
        };
        (current.as_deref() != Some(error.as_str())).then_some(error)
    }
    /// The manifest Create would write for the current selections, or `None`
    /// without a store or a system BIOS.
    pub fn preview(&self) -> Option<Preview> {
        self.refresh();
        if let Some(preview) = self.cache.borrow().preview.clone() {
            return preview;
        }
        let image = self.image();
        let preview = self
            .store
            .as_ref()
            .filter(|_| !image.firmware.is_empty())
            .map(|store| {
                let roms = Roms {
                    store,
                    cache: &self.roms,
                };
                pc_image::create::preview(&image, &roms)
                    .map(Rc::new)
                    .map_err(|e| e.to_string())
            });
        self.cache.borrow_mut().preview = Some(preview.clone());
        preview
    }

    // ---- firmware pickers -------------------------------------------------

    fn class_name(id: &str) -> String {
        create::MACHINE_CLASSES
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| id.to_owned(), |c| c.name.to_owned())
    }
    fn board_name(id: &str) -> String {
        caps::value_label(
            &Draft::for_board(assembly::BoardModel::Ibm5170),
            "board.model",
            &Value::Name(id.into()),
        )
    }
    pub fn board_label(&self) -> String {
        Self::board_name(self.selection.hardware.board.id())
    }
    /// Reasons a package ROM does not fit the slot; `None` if it has another
    /// role. Verdicts are cached per revision: pages redraw every frame.
    fn fit(&self, package: &Package, rom: &Rom, slot: Slot) -> Option<Vec<String>> {
        if rom.role != slot.role() {
            return None;
        }
        self.refresh();
        let key = format!("{slot:?}:{}:{}", package.descriptor.digest, rom.id);
        if let Some(reasons) = self.cache.borrow().fits.get(&key) {
            return Some(reasons.clone());
        }
        let reasons = self.fit_uncached(package, rom, slot);
        self.cache.borrow_mut().fits.insert(key, reasons.clone());
        Some(reasons)
    }
    fn fit_uncached(&self, package: &Package, rom: &Rom, slot: Slot) -> Vec<String> {
        let mut reasons = Vec::new();
        let fit = package.fit();
        let board = self.selection.hardware.board.id();
        let class = create::class_of(self.selection.hardware.board).map(|c| c.id);
        if let Some(other) = fit.machine_classes.first() {
            if !fit
                .machine_classes
                .iter()
                .any(|c| Some(c.as_str()) == class)
            {
                reasons.push(format!(
                    "Wrong class: built for {}.",
                    Self::class_name(other)
                ));
                return reasons;
            }
        }
        if slot == Slot::Bios && fit.boards.is_empty() {
            reasons.push("Names no board in its fit metadata. A class alone does not establish fit: add the board to its fit claims in the Firmware library.".into());
        } else if !fit.boards.is_empty() && !fit.boards.iter().any(|b| b == board) {
            reasons.push(format!(
                "Wrong board: it fits the {}.",
                fit.boards
                    .iter()
                    .map(|b| Self::board_name(b))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ));
        }
        if rom.catalogue_id.is_none() {
            reasons.push("Not in the firmware catalogue, so its requirements are unknown: unlisted firmware never fits an image.".into());
        }
        if let (true, Some(id)) = (reasons.is_empty(), &rom.catalogue_id) {
            reasons.extend(self.requirement(
                slot,
                id,
                (package.descriptor.digest.clone(), rom.id.clone()),
            ));
        }
        reasons
    }
    /// The validator's verdict on a package ROM in a firmware slot. A system
    /// BIOS is checked against the bare board through the API's choices, so
    /// the cards and disks it would need later do not count against it. A
    /// video BIOS or option ROM is checked as a candidate machine with that ROM
    /// (an option ROM fits the disk adapter with it); only errors the ROM
    /// introduces count, never a problem the machine already has.
    fn requirement(&self, slot: Slot, id: &str, rom: RomRef) -> Option<String> {
        match slot {
            Slot::Bios => {
                let value = Value::Name(id.into());
                let (choices, baseline) = self.board_choices();
                match choices.iter().find(|c| c.value == value) {
                    Some(choice) => errors_in(&choice.diagnostics)
                        .into_iter()
                        .find(|d| !baseline.contains(d))
                        .map(|d| d.reason),
                    None => Some(format!(
                        "Its catalogue entry is not a system BIOS for the {}.",
                        self.board_label()
                    )),
                }
            }
            Slot::Video | Slot::OptionRom => {
                self.bios_rom()?;
                let key = format!("rom:{slot:?}:{}:{}", rom.0, rom.1);
                self.candidate(&key, |s| match slot {
                    Slot::Video => s.video_bios = Some(rom),
                    _ => s.option_rom = Some(rom),
                })
            }
        }
    }
    /// System BIOS choices of the bare board and its own errors, cached per
    /// revision.
    fn board_choices(&self) -> BoardVerdicts {
        self.refresh();
        if let Some(cached) = self.cache.borrow().board.clone() {
            return cached;
        }
        let draft = Draft::for_board(self.selection.hardware.board);
        let cached = (
            Rc::new(caps::choices(&draft, "firmware.bios")),
            Rc::new(errors_in(&caps::validate_draft(&draft).diagnostics)),
        );
        self.cache.borrow_mut().board = Some(cached.clone());
        cached
    }
    /// Fitting and incompatible offers of a picker; the BIOS list follows the
    /// text filter.
    pub fn offers(&self, slot: Slot) -> (Vec<Offer>, Vec<Offer>) {
        let query = if slot == Slot::Bios {
            self.filter.trim().to_lowercase()
        } else {
            String::new()
        };
        self.offers_matching(slot, &query)
    }
    /// Number of fitting packages, regardless of the text filter.
    pub fn fitting(&self, slot: Slot) -> usize {
        self.offers_matching(slot, "").0.len()
    }
    fn offers_matching(&self, slot: Slot, query: &str) -> (Vec<Offer>, Vec<Offer>) {
        let mut fits = Vec::new();
        let mut misfits = Vec::new();
        for package in self.packages.iter().flatten() {
            for rom in &package.config.roms {
                let Some(reasons) = self.fit(package, rom, slot) else {
                    continue;
                };
                let open = self.rom_open(package, rom);
                let config = &package.config;
                let vendor = config
                    .vendor
                    .clone()
                    .unwrap_or_else(|| "Unknown vendor".into());
                let detail = format!(
                    "{} · {} · local store · {}",
                    config
                        .version
                        .as_ref()
                        .map_or_else(|| "Version unknown".into(), |v| format!("Version {v}")),
                    config.date.clone().unwrap_or_else(|| "date unknown".into()),
                    package.reference
                );
                let chips = if rom.parts.is_empty() {
                    format!("Single image · {} KiB", rom.size / 1024)
                } else {
                    rom.parts
                        .iter()
                        .map(|p| {
                            format!(
                                "{} {}",
                                p.position.clone().unwrap_or_else(|| "chip".into()),
                                p.lane
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" + ")
                };
                let layout = format!(
                    "{chips} · SHA-256 {}…{}",
                    &rom.sha256[..8],
                    &rom.sha256[rom.sha256.len() - 4..]
                );
                let offer = Offer {
                    rom: (package.descriptor.digest.clone(), rom.id.clone()),
                    name: config.name.clone(),
                    vendor,
                    detail,
                    layout,
                    open,
                    reasons,
                };
                let text = format!(
                    "{} {} {} {} {}",
                    offer.name,
                    offer.vendor,
                    offer.detail,
                    if open { "open" } else { "user-supplied" },
                    rom.catalogue_id.clone().unwrap_or_default()
                )
                .to_lowercase();
                if !query.is_empty() && !text.contains(query) {
                    continue;
                }
                if offer.reasons.is_empty() {
                    fits.push(offer);
                } else {
                    misfits.push(offer);
                }
            }
        }
        let key = |o: &Offer| (o.vendor.clone(), !o.open, o.name.clone(), o.rom.clone());
        fits.sort_by_key(key);
        misfits.sort_by_key(key);
        (fits, misfits)
    }
    fn default_offer(&self, slot: Slot, prefer_vendor: Option<&str>) -> Option<RomRef> {
        let (fits, _) = self.offers_matching(slot, "");
        fits.iter()
            .min_by_key(|o| (Some(o.vendor.as_str()) != prefer_vendor, !o.open))
            .map(|o| o.rom.clone())
    }

    // ---- choices with consequences -----------------------------------------

    /// Changing the board picks a fitting BIOS again and resets what it decides.
    pub fn choose_board(&mut self, board: assembly::BoardModel) {
        self.edit(|s| s.hardware.board = board);
        if let Some(speed) = create::speeds(board).into_iter().find(|s| s.calibrated) {
            self.edit(|s| s.hardware.rate = (speed.steps, speed.pit_clocks));
        }
        self.filter.clear();
        let bios = self.default_offer(Slot::Bios, None);
        self.edit(|s| s.bios = bios);
        self.apply_bios();
    }
    /// Choosing a BIOS resets the video BIOS (unless picked), the disk option
    /// ROM and the drive choices it decides.
    pub fn choose_bios(&mut self, rom: RomRef) {
        self.edit(|s| s.bios = Some(rom));
        self.apply_bios();
    }
    pub fn choose_video_bios(&mut self, rom: RomRef) {
        self.video_touched = true;
        self.edit(|s| s.video_bios = Some(rom));
    }
    pub fn choose_option_rom(&mut self, rom: Option<RomRef>) {
        self.edit(|s| s.option_rom = rom);
        self.sanitize();
    }
    fn apply_bios(&mut self) {
        let bios = self.bios_rom();
        let vendor = bios.as_ref().and_then(|b| b.package.config.vendor.clone());
        if !self.video_touched {
            let video = self.default_offer(Slot::Video, vendor.as_deref());
            self.edit(|s| s.video_bios = video);
        }
        // The catalogue states whether the system BIOS serves fixed disks;
        // without that service the disks need an adapter option ROM.
        let serves_disks = bios
            .as_ref()
            .and_then(PackageRom::entry)
            .is_some_and(|e| e.provides.contains(&assembly::FirmwareService::FixedDisk));
        let option = if serves_disks {
            None
        } else {
            self.default_offer(Slot::OptionRom, None)
        };
        self.edit(|s| s.option_rom = option);
        if !self.repo_edited {
            self.repo = match &bios {
                Some(b) if self.open(b) => "open-at".into(),
                Some(b) => format!(
                    "{}-at",
                    b.package
                        .config
                        .vendor
                        .as_deref()
                        .and_then(|v| v.split_whitespace().next())
                        .unwrap_or("custom")
                        .to_lowercase()
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .collect::<String>()
                ),
                None => "at".into(),
            };
        }
        self.sanitize();
    }
    /// Replace selections the validator now refuses by the first allowed one.
    pub fn sanitize(&mut self) {
        let speeds = create::speeds(self.selection.hardware.board);
        if !speeds
            .iter()
            .any(|s| (s.steps, s.pit_clocks) == self.selection.hardware.rate)
        {
            if let Some(speed) = speeds.first() {
                self.edit(|s| s.hardware.rate = (speed.steps, speed.pit_clocks));
            }
        }
        for unit in 0..2 {
            let path = format!("card[fdc-765].drives[unit={unit}].type");
            let Some(current) = self.selection.hardware.floppies[unit].clone() else {
                continue;
            };
            let choices = self.choices(&path);
            if choices
                .iter()
                .any(|c| c.value == Value::Name(current.clone()) && !c.enabled)
            {
                let replacement = choices
                    .iter()
                    .find(|c| c.enabled)
                    .and_then(|c| match &c.value {
                        Value::Name(name) => Some(name.clone()),
                        _ => None,
                    });
                self.edit(|s| {
                    s.hardware.floppies[unit] = if unit == 0 { replacement } else { None }
                });
            }
        }
        if let Some(ems) = self.selection.hardware.ems {
            let choices = self.choices("card[lotech-ems].frame");
            let refused = choices
                .iter()
                .any(|c| c.value == Value::Integer(u32::from(ems.frame)) && !c.enabled);
            if let Some(Value::Integer(frame)) = choices
                .iter()
                .find(|c| c.enabled)
                .map(|c| c.value.clone())
                .filter(|_| refused)
            {
                self.edit(|s| {
                    if let Some(ems) = &mut s.hardware.ems {
                        ems.frame = frame as u16;
                    }
                });
            }
        }
        if self.selection.hardware.ide_port != assembly::descriptors::IDE_PORTS[0] {
            let port = Value::Integer(u32::from(self.selection.hardware.ide_port));
            if self
                .choices("card[ide].port")
                .iter()
                .any(|c| c.value == port && !c.enabled)
            {
                self.edit(|s| s.hardware.ide_port = assembly::descriptors::IDE_PORTS[0]);
            }
        }
    }

    // ---- steps -------------------------------------------------------------

    fn step_of(&self, paths: &[String]) -> usize {
        let video_first = self.video_rom().is_some();
        for path in paths {
            if path.starts_with("firmware.bios") || path.starts_with("board.") {
                return MOTHERBOARD;
            }
            if path.starts_with("cpu.")
                || path.starts_with("memory.")
                || path.contains("lotech-ems")
            {
                return CPU_MEMORY;
            }
            if path.starts_with("firmware.video")
                || (video_first && path.starts_with("firmware.option_roms[0]"))
                || ["card[vga", "card[mda", "card[sb16", "card[uart"]
                    .iter()
                    .any(|p| path.starts_with(p))
            {
                return DEVICES;
            }
            if path.starts_with("firmware.option_roms")
                || path.starts_with("card[fdc")
                || path.starts_with("card[ide")
            {
                return DRIVES;
            }
        }
        DRIVES
    }
    /// Problems that block Next on their step and Create on the last one.
    pub fn problems(&self) -> Vec<(usize, String)> {
        let mut list = Vec::new();
        if self.packages.is_none() {
            list.push((
                MOTHERBOARD,
                "Reading firmware packages from the store…".into(),
            ));
        } else if self.bios_rom().is_none() {
            list.push((
                MOTHERBOARD,
                if self.fitting(Slot::Bios) == 0 {
                    format!(
                        "No system BIOS fits the {}: import your ROM folder or get a package from a registry.",
                        self.board_label()
                    )
                } else {
                    format!("Choose a system BIOS for the {}.", self.board_label())
                },
            ));
        }
        if self.selection.hardware.video == Video::Vga
            && self.bios_rom().is_some()
            && self.video_rom().is_none()
        {
            list.push((
                DEVICES,
                "VGA needs a video BIOS: choose one under Video BIOS, or pick VGA text only or MDA."
                    .into(),
            ));
        }
        if self.bios_rom().is_some() {
            match self.draft() {
                Err(error) => list.push((DRIVES, error)),
                Ok(_) => {
                    let errors = self.errors();
                    for d in errors.iter() {
                        list.push((self.step_of(&d.field_paths), d.reason.clone()));
                    }
                    // What only the ROM bytes decide, such as CMOS disk types.
                    if errors.is_empty() {
                        if let Some(Err(error)) = self.preview() {
                            list.push((DRIVES, error));
                        }
                    }
                }
            }
        }
        let check = self.tag_check();
        if check.level == TagLevel::Bad {
            list.push((NAME, check.text));
        }
        list.sort_by_key(|(step, _)| *step);
        list
    }
    // ---- name and tag ------------------------------------------------------

    pub fn reference(&self) -> String {
        format!("{}:{}", self.repo.trim(), self.tag.trim())
    }
    fn existing(&self, reference: &str) -> Option<&Digest> {
        self.tags
            .iter()
            .find(|(name, _)| name == reference)
            .map(|(_, d)| d)
    }
    /// The next free minor version of a repository, or 1.0.0.
    pub fn next_version(&self, repo: &str) -> String {
        let prefix = format!("{repo}:");
        let top = self
            .tags
            .iter()
            .filter_map(|(name, _)| name.strip_prefix(&prefix))
            .filter(|tag| image_registry::reference::is_version_tag(tag))
            .filter_map(|tag| {
                let core = tag.split(['-', '+']).next()?;
                let mut parts = core.split('.').map(|n| n.parse::<u64>().ok());
                Some((parts.next()??, parts.next()??, parts.next()??))
            })
            .max();
        match top {
            Some((major, minor, _)) => format!("{major}.{}.0", minor + 1),
            None => "1.0.0".into(),
        }
    }
    /// A descriptive tag of the chosen hardware, such as `6mhz-1mb-2mbems`.
    pub fn hardware_tag(&self) -> String {
        let h = &self.selection.hardware;
        let mhz = create::speeds(h.board)
            .into_iter()
            .find(|s| (s.steps, s.pit_clocks) == h.rate)
            .map_or_else(
                || format!("{}-{}", h.rate.0, h.rate.1),
                |s| format!("{}mhz", s.mhz),
            );
        let size = |kib: u32| {
            if kib >= 1024 && kib.is_multiple_of(1024) {
                format!("{}mb", kib / 1024)
            } else {
                format!("{kib}k")
            }
        };
        let ems = h
            .ems
            .map(|e| format!("-{}ems", size(e.size_kib)))
            .unwrap_or_default();
        format!("{mhz}-{}{ems}", size(h.extended_kib))
    }
    /// Store and registry tag rules: the OCI grammar, version tags never move,
    /// other tags move only after a confirmation.
    pub fn tag_check(&self) -> TagCheck {
        let bad = |text: String| TagCheck {
            level: TagLevel::Bad,
            text,
            fix: None,
            moving: None,
        };
        let repo = self.repo.trim();
        let tag = self.tag.trim();
        if Store::validate_tag_name(&format!("{repo}:latest")).is_err() {
            return bad("Repository names use lowercase letters and digits, separated by . _ or - characters.".into());
        }
        if tag.contains('+') {
            return bad(
                "Tags cannot contain \"+\": a registry tag allows only letters, digits and . _ -."
                    .into(),
            );
        }
        let reference = self.reference();
        if Store::validate_tag_name(&reference).is_err() {
            return bad("Tags start with a letter, digit or _ and use only letters, digits and . _ - (at most 128 characters).".into());
        }
        let version = image_registry::reference::is_version_tag(tag);
        match self.existing(&reference) {
            Some(digest) if version => TagCheck {
                level: TagLevel::Bad,
                text: format!(
                    "{reference} already exists ({}). Version tags never move, so this name cannot be reused.",
                    short(digest)
                ),
                fix: Some(self.next_version(repo)),
                moving: None,
            },
            Some(digest) => TagCheck {
                level: TagLevel::Warn,
                text: format!(
                    "{reference} already names {}. Create will ask before moving it; the old image stays reachable by its digest{}.",
                    short(digest),
                    {
                        let others: Vec<_> = self
                            .tags
                            .iter()
                            .filter(|(name, d)| d == digest && name != &reference)
                            .map(|(name, _)| name.as_str())
                            .collect();
                        if others.is_empty() {
                            String::new()
                        } else {
                            format!(" and by {}", others.join(", "))
                        }
                    }
                ),
                fix: None,
                moving: Some(digest.clone()),
            },
            None => TagCheck {
                level: TagLevel::Ok,
                text: format!(
                    "{reference} is new. {}",
                    if version {
                        "As a version tag it will never move."
                    } else {
                        "This tag may move to a newer image later, after a confirmation."
                    }
                ),
                fix: None,
                moving: None,
            },
        }
    }

    // ---- redistribution and summary ----------------------------------------

    /// Whether every blob of a package ROM is open, locally and everywhere else.
    fn rom_open(&self, package: &Package, rom: &Rom) -> bool {
        package.open(rom)
            && rom
                .blob
                .iter()
                .chain(rom.parts.iter().map(|p| &p.blob))
                .all(|digest| !self.restricted.contains(digest))
    }
    pub fn open(&self, rom: &PackageRom) -> bool {
        rom.rom().is_ok_and(|r| self.rom_open(&rom.package, r))
    }
    /// What makes the image user-supplied: the push guard's own rule over the
    /// whole prospective closure (configuration, metadata, zero bases and
    /// ROMs), with the most restrictive mark any local manifest records.
    /// Without a preview, the chosen package ROMs alone decide.
    pub fn user_supplied(&self) -> Vec<String> {
        let image = self.image();
        let mut names: Vec<String> = Vec::new();
        let mut add = |name: String| {
            if !names.contains(&name) {
                names.push(name);
            }
        };
        let Some(Ok(manifest)) = self.preview() else {
            for rom in image.firmware.iter().filter(|r| !self.open(r)) {
                add(rom.package.config.name.clone());
            }
            return names;
        };
        for entry in image_registry::guard::closure(&manifest, &self.marks) {
            if entry.effective != image_registry::guard::Redistribution::UserSupplied {
                continue;
            }
            let digest = &entry
                .descriptor
                .decoded_digest()
                .unwrap_or_else(|_| entry.descriptor.digest.clone());
            let package = image.firmware.iter().find(|rom| {
                rom.rom().is_ok_and(|r| {
                    r.blob.as_ref() == Some(digest) || r.parts.iter().any(|p| &p.blob == digest)
                })
            });
            add(match package {
                Some(rom) => rom.package.config.name.clone(),
                None => match entry.descriptor.media_type.as_str() {
                    image_store::media_type::CONFIG => "the generated configuration".into(),
                    image_store::media_type::METADATA => "the generated metadata".into(),
                    pc_image::DISK_ZERO => "a blank disk".into(),
                    other => other.into(),
                },
            });
        }
        names
    }
    pub fn private_notice(&self) -> Option<String> {
        let users = self.user_supplied();
        (!users.is_empty()).then(|| {
            format!(
                "User-supplied firmware ({}): this image stays in your private store and can be pushed only to a private registry, never to unemu/.",
                users.join(", ")
            )
        })
    }
    pub fn full_reference(&self) -> String {
        let users = self.user_supplied();
        format!(
            "If pushed to the default registry later: {}/{}/{}{}",
            image_registry::DEFAULT_REGISTRY,
            if users.is_empty() {
                image_registry::reference::OFFICIAL_NAMESPACE
            } else {
                "private"
            },
            self.reference(),
            if users.is_empty() {
                String::new()
            } else {
                format!(
                    " — private namespace only, because {} is user-supplied.",
                    users.join(" and ")
                )
            }
        )
    }
    fn drive_line(chs: Option<Chs>) -> String {
        chs.map_or_else(
            || "none".into(),
            |c| {
                format!(
                    "new blank disk · {} · {}/{}/{} · zero base, nothing stored until written",
                    crate::library_view::human_size(c.bytes()),
                    c.cylinders,
                    c.heads,
                    c.sectors
                )
            },
        )
    }
    pub fn summary(&self) -> Vec<(&'static str, String)> {
        let h = &self.selection.hardware;
        let draft = self.draft().ok();
        let label = |path: &str, value: Value| {
            draft
                .as_deref()
                .map_or_else(String::new, |d| caps::value_label(d, path, &value))
        };
        let speed = create::speeds(h.board)
            .into_iter()
            .find(|s| (s.steps, s.pit_clocks) == h.rate)
            .map_or_else(String::new, |s| format!("{} MHz, ", s.mhz));
        let mut rows = vec![
            (
                "Image",
                format!(
                    "{}{} · new · local store",
                    self.reference(),
                    if self.title.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", self.title.trim())
                    }
                ),
            ),
            (
                "Machine",
                format!(
                    "AT 286 · {} · Intel 80286 · {speed}{}/{} steps per PIT clock · {}",
                    self.board_label(),
                    h.rate.0,
                    h.rate.1,
                    if h.fpu { "80287" } else { "no coprocessor" }
                ),
            ),
            (
                "Memory",
                format!(
                    "{} KiB conventional · {} KiB extended{}",
                    h.conventional_kib,
                    h.extended_kib,
                    h.ems.map_or_else(String::new, |e| format!(
                        " · Lo-tech EMS {} KiB at {:X}h, frame {:X}h",
                        e.size_kib, e.port, e.frame
                    ))
                ),
            ),
            (
                "Video",
                label("card[vga].kind", Value::Name(h.video.card().into())),
            ),
            (
                "Sound",
                h.sound.map_or_else(
                    || "None".into(),
                    |s| {
                        format!(
                            "Sound Blaster 16 · {:X}h · IRQ {} · DMA {}/{}",
                            s.base, s.irq, s.dma8, s.dma16
                        )
                    },
                ),
            ),
            (
                "COM1",
                match h.serial {
                    Serial::Mouse => "16550 UART · Microsoft serial mouse",
                    Serial::Empty => "16550 UART · nothing attached",
                    Serial::None => "not installed",
                }
                .into(),
            ),
        ];
        let rom_line = |rom: Option<PackageRom>| {
            rom.map(|r| {
                let open = self.open(&r);
                format!(
                    "{} [{}]",
                    r.package.config.name,
                    if open { "open" } else { "user-supplied" }
                )
            })
        };
        rows.push((
            "BIOS",
            rom_line(self.bios_rom()).unwrap_or_else(|| "none chosen".into()),
        ));
        if let Some(line) = rom_line(self.video_rom()) {
            rows.push(("Video BIOS", line));
        }
        if let Some(line) = rom_line(self.option_rom()) {
            rows.push(("Option ROM", format!("{line} · disk controller")));
        }
        for (unit, name) in [(0, "A:"), (1, "B:")] {
            rows.push((
                name,
                h.floppies[unit].as_ref().map_or_else(
                    || "not installed".into(),
                    |kind| {
                        format!(
                            "{} · empty catalogue",
                            label(
                                "card[fdc-765].drives[unit=0].type",
                                Value::Name(kind.clone())
                            )
                        )
                    },
                ),
            ));
        }
        rows.push(("C:", Self::drive_line(h.disks[0])));
        rows.push(("D:", Self::drive_line(h.disks[1])));
        rows.push((
            "CMOS",
            "derived from this configuration at first Start".into(),
        ));
        rows
    }

    // ---- create --------------------------------------------------------------

    /// The image to create, its reference and the digest a confirmed move
    /// replaces. Refuses while any step has a problem.
    pub fn request(&self) -> Result<(NewImage, String, Option<Digest>), String> {
        if let Some((step, problem)) = self.problems().into_iter().next() {
            return Err(format!(
                "Fix step {} ({}) first: {problem}",
                step + 1,
                STEPS[step]
            ));
        }
        let check = self.tag_check();
        if let Some(current) = check.moving {
            if self.confirmed_move.as_ref() != Some(&current) {
                return Err("Confirm moving the tag first.".into());
            }
            return Ok((self.image(), self.reference(), Some(current)));
        }
        Ok((self.image(), self.reference(), None))
    }
}

pub(crate) fn short(digest: &Digest) -> String {
    format!("sha256:{}…{}", &digest.hex()[..8], &digest.hex()[60..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_test_support::FirmwareStore;

    fn wizard(store: &FirmwareStore) -> Wizard {
        let mut wizard = Wizard::new();
        wizard.store = Some(store.store.clone());
        wizard.set_store(store.choices());
        wizard
    }
    fn offer_names(offers: &[Offer]) -> Vec<&str> {
        offers.iter().map(|o| o.name.as_str()).collect()
    }
    fn enabled(wizard: &Wizard, path: &str, value: Value) -> Option<bool> {
        wizard
            .choices(path)
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.enabled)
    }
    fn reason(wizard: &Wizard, path: &str, value: Value) -> String {
        wizard
            .choices(path)
            .iter()
            .find(|c| c.value == value)
            .and_then(|c| c.reason.clone())
            .unwrap_or_default()
    }

    #[test]
    fn machine_class_lists_the_closed_seven_with_only_the_at_available() {
        assert_eq!(create::MACHINE_CLASSES.len(), 7);
        let available: Vec<_> = create::MACHINE_CLASSES
            .iter()
            .filter(|c| !c.boards.is_empty())
            .map(|c| c.id)
            .collect();
        assert_eq!(available, ["at"]);
        let wizard = Wizard::new();
        assert_eq!(wizard.selection().class, "at");
        assert!(wizard
            .problems()
            .iter()
            .any(|(step, _)| *step == MOTHERBOARD));
    }

    #[test]
    fn motherboard_offers_fitting_bios_packages_and_explains_the_others() {
        let store = FirmwareStore::new(true);
        let mut wizard = wizard(&store);
        let (fits, misfits) = wizard.offers(Slot::Bios);
        assert_eq!(
            offer_names(&fits),
            [
                "Kiselev AT BIOS (private copy)",
                "kiselev-xi8088-1.0.2-atreset"
            ]
        );
        // The default is the open package, so the image stays redistributable.
        let open = fits.iter().find(|o| o.open).unwrap();
        assert_eq!(wizard.selection().bios.as_ref(), Some(&open.rom));
        assert_eq!(wizard.repo, "open-at");
        let reasons = |name: &str| {
            misfits
                .iter()
                .find(|o| o.name == name)
                .unwrap()
                .reasons
                .join(" ")
        };
        assert!(reasons("Synthetic XT BIOS").contains("Wrong class: built for PC/XT, 8-bit ISA."));
        let unlisted = reasons("Unlisted 286 BIOS");
        assert!(unlisted.contains("Names no board"), "{unlisted}");
        assert!(
            unlisted.contains("Not in the firmware catalogue"),
            "{unlisted}"
        );
        // The video BIOS and option ROM pickers never offer a system BIOS.
        assert!(!offer_names(&wizard.offers(Slot::Video).0).contains(&"Synthetic XT BIOS"));
        wizard.filter = "private".into();
        assert_eq!(
            offer_names(&wizard.offers(Slot::Bios).0),
            ["Kiselev AT BIOS (private copy)"]
        );
        assert_eq!(wizard.fitting(Slot::Bios), 2);
        // The open BIOS has no fixed-disk service, so XTIDE is chosen for the disks.
        assert!(wizard
            .option_rom()
            .is_some_and(|r| r.entry().is_some_and(|e| e.id == "xub-r638-xtp-ata1f0")));
        assert_eq!(
            wizard.video_rom().and_then(|r| r.entry()).map(|e| e.id),
            Some("vgabios-0.8a-unemu.1-std")
        );
        assert!(wizard.problems().is_empty(), "{:?}", wizard.problems());
    }

    #[test]
    fn an_empty_store_shows_the_empty_state_and_blocks_the_motherboard_step() {
        let mut wizard = Wizard::new();
        wizard.set_store(Default::default());
        assert_eq!(wizard.fitting(Slot::Bios), 0);
        let problems = wizard.problems();
        assert_eq!(problems[0].0, MOTHERBOARD);
        assert!(
            problems[0]
                .1
                .contains("No system BIOS fits the IBM 5170 (AT)"),
            "{problems:?}"
        );
        assert!(wizard.request().is_err());
    }

    #[test]
    fn cpu_and_memory_options_and_limits_come_from_the_capabilities_api() {
        let store = FirmwareStore::new(false);
        let mut wizard = wizard(&store);
        let speeds = create::speeds(assembly::BoardModel::Ibm5170);
        assert_eq!(
            speeds
                .iter()
                .map(|s| (s.mhz, s.steps, s.pit_clocks))
                .collect::<Vec<_>>(),
            [(6, 8, 13), (8, 32, 39)]
        );
        assert_eq!(wizard.hardware().rate, (8, 13));
        let fpu: Vec<_> = wizard
            .choices("cpu.fpu")
            .iter()
            .map(|c| c.value.clone())
            .collect();
        assert_eq!(
            fpu,
            [Value::Name("none".into()), Value::Name("80287".into())]
        );
        assert_eq!(wizard.choices("memory.conventional_kib").len(), 41);
        let extended = wizard.domain("memory.extended_kib").unwrap();
        assert_eq!(
            (extended.max, extended.step),
            (assembly::IBM5170_MAX_EXTENDED_KIB, 64)
        );
        // The EMS frame follows the validator: E000h is system ROM space and
        // C000h overlaps the video BIOS of the chosen VGA card.
        let frame = "card[lotech-ems].frame";
        assert_eq!(enabled(&wizard, frame, Value::Integer(0xD000)), Some(true));
        assert_eq!(enabled(&wizard, frame, Value::Integer(0xE000)), Some(false));
        assert_eq!(enabled(&wizard, frame, Value::Integer(0xC000)), Some(false));
        assert!(!reason(&wizard, frame, Value::Integer(0xC000)).is_empty());
        // Without the video BIOS and XTIDE at C8000h the C000h frame is free.
        wizard.edit(|s| {
            s.hardware.video = Video::VgaText;
            s.option_rom = None;
            s.hardware.disks = [None, None];
        });
        assert_eq!(enabled(&wizard, frame, Value::Integer(0xC000)), Some(true));
    }

    #[test]
    fn device_options_and_video_bios_fit_follow_the_validator() {
        let store = FirmwareStore::new(false);
        let mut wizard = wizard(&store);
        // The EMS board decodes 260h, so the Sound Blaster cannot use it.
        assert_eq!(
            enabled(&wizard, "card[sb16].base", Value::Integer(0x260)),
            Some(false)
        );
        assert!(!reason(&wizard, "card[sb16].base", Value::Integer(0x260)).is_empty());
        assert_eq!(
            enabled(&wizard, "card[sb16].base", Value::Integer(0x240)),
            Some(true)
        );
        assert_eq!(
            wizard.candidate("video-text", |s| s.hardware.video = Video::VgaText),
            None
        );
        assert_eq!(
            wizard.candidate("video-mda", |s| s.hardware.video = Video::Mda),
            None
        );
        // The open VGA BIOS states the register-level VGA it needs; it fits.
        let (fits, _) = wizard.offers(Slot::Video);
        assert_eq!(offer_names(&fits), ["vgabios-0.8a-unemu.1-std"]);
        wizard.edit(|s| s.video_bios = None);
        assert!(wizard
            .problems()
            .iter()
            .any(|(step, text)| *step == DEVICES && text.starts_with("VGA needs a video BIOS")));
        wizard.edit(|s| s.hardware.video = Video::Mda);
        assert!(!wizard.problems().iter().any(|(step, _)| *step == DEVICES));
    }

    #[test]
    fn drives_follow_firmware_services_and_option_rom_requirements() {
        let store = FirmwareStore::new(false);
        let mut wizard = wizard(&store);
        assert_eq!(wizard.hardware().disks[0], Some(create::BLANK_DISKS[0]));
        assert!(
            enabled(
                &wizard,
                "card[fdc-765].drives[unit=0].type",
                Value::Name("1.44M".into())
            ) == Some(true)
        );
        // Without XTIDE the Kiselev BIOS has no fixed-disk service.
        assert!(wizard
            .candidate("no-xtide", |s| s.option_rom = None)
            .is_some());
        wizard.choose_option_rom(None);
        assert!(wizard.problems().iter().any(|(step, _)| *step == DRIVES));
        let xtide = wizard.offers(Slot::OptionRom).0[0].rom.clone();
        wizard.choose_option_rom(Some(xtide));
        assert!(wizard.problems().is_empty(), "{:?}", wizard.problems());
        // A slave needs a master.
        assert!(wizard
            .candidate("slave-only", |s| s.hardware.disks =
                [None, Some(create::BLANK_DISKS[0])])
            .is_some());
        // XTIDE r638 is built for the primary channel; the API says why.
        assert_eq!(
            enabled(&wizard, "card[ide].port", Value::Integer(0x170)),
            Some(false)
        );
        wizard.edit(|s| {
            s.option_rom = None;
            s.hardware.ide_port = 0x170;
            s.hardware.disks = [None, None];
        });
        let (fits, misfits) = wizard.offers(Slot::OptionRom);
        assert!(fits.is_empty());
        assert!(!misfits[0].reasons.is_empty());
    }

    #[test]
    fn tags_follow_the_grammar_and_version_tags_never_move() {
        let store = FirmwareStore::new(false);
        let mut wizard = wizard(&store);
        let image = Digest::of(b"existing");
        wizard.tags.push(("open-at:1.0.0".into(), image.clone()));
        wizard.tags.push(("open-at:latest".into(), image.clone()));
        let check = wizard.tag_check();
        assert_eq!(check.level, TagLevel::Bad);
        assert!(check.text.contains("Version tags never move"));
        assert_eq!(check.fix.as_deref(), Some("1.1.0"));
        assert!(wizard.request().is_err());
        for (tag, text) in [
            ("1.0.0+build.5", "cannot contain \"+\""),
            (".hidden", "Tags start with a letter"),
            (&"x".repeat(129), "at most 128"),
        ] {
            wizard.set_tag(tag.into());
            let check = wizard.tag_check();
            assert_eq!(check.level, TagLevel::Bad, "{tag}");
            assert!(check.text.contains(text), "{tag}: {}", check.text);
        }
        wizard.set_repo("Open AT".into());
        assert!(wizard.tag_check().text.starts_with("Repository names"));
        wizard.set_repo("open-at".into());
        wizard.set_tag("latest".into());
        let check = wizard.tag_check();
        assert_eq!(check.level, TagLevel::Warn);
        assert_eq!(check.moving, Some(image.clone()));
        assert!(wizard.request().is_err(), "a move needs confirmation");
        wizard.confirmed_move = Some(image.clone());
        let (_, reference, expected) = wizard.request().unwrap();
        assert_eq!(
            (reference.as_str(), expected),
            ("open-at:latest", Some(image))
        );
        wizard.set_tag(wizard.hardware_tag());
        assert_eq!(wizard.tag, "6mhz-1mb-2mbems");
        assert_eq!(wizard.tag_check().level, TagLevel::Ok);
        wizard.set_tag("1.1.0".into());
        assert!(wizard.tag_check().text.contains("will never move"));
    }

    #[test]
    fn a_user_supplied_bios_makes_the_image_private() {
        let store = FirmwareStore::new(true);
        let mut wizard = wizard(&store);
        assert_eq!(wizard.private_notice(), None);
        assert!(wizard
            .full_reference()
            .contains("registry.unemu.dev/unemu/open-at:1.0.0"));
        let private = wizard
            .offers(Slot::Bios)
            .0
            .into_iter()
            .find(|o| !o.open)
            .unwrap();
        wizard.choose_bios(private.rom);
        let notice = wizard.private_notice().unwrap();
        assert!(
            notice.contains("Kiselev AT BIOS (private copy)"),
            "{notice}"
        );
        assert!(notice.contains("never to unemu/"));
        assert!(wizard.full_reference().contains("/private/"));
        assert_eq!(wizard.repo, "synthetic-at");
        let summary = wizard.summary();
        let bios = summary.iter().find(|(k, _)| *k == "BIOS").unwrap();
        assert!(bios.1.ends_with("[user-supplied]"));
    }

    #[test]
    fn a_blob_another_manifest_marks_user_supplied_is_user_supplied_here() {
        let store = FirmwareStore::new(false);
        let wizard = wizard(&store);
        assert_eq!(wizard.private_notice(), None);
        // Another package records the open VGA BIOS bytes as user-supplied;
        // the most restrictive local mark wins, as in the push guard.
        let bytes = std::fs::read(store.temp.path().join("roms/vga.bin")).unwrap();
        let digest = Digest::of(&bytes);
        let config = image_store::firmware::Config::decode(
            format!(
                "schema=1\nplatform='ibm-pc-at'\nname='VGA BIOS copy'\n[fits]\nboards=['ibm-5170']\n[[roms]]\nid='video-bios'\nrole='video-bios'\ncatalogue_id='vgabios-0.8a-unemu.1-std'\nsize=32768\nsha256='{}'\nblob='{digest}'\n",
                digest.hex()
            )
            .as_bytes(),
        )
        .unwrap();
        let candidate = pc_image::firmware_import::Candidate {
            catalogue_id: "vga-copy".into(),
            config,
            sources: vec![store.temp.path().join("roms/vga.bin")],
            distribution: assembly::FirmwareDistribution::UserSupplied,
        };
        pc_image::firmware_import::publish(
            &store.store,
            &candidate,
            &[],
            pc_image::firmware::catalogue(),
        )
        .unwrap();
        let wizard = {
            let mut w = Wizard::new();
            w.set_store(store.choices());
            w
        };
        assert!(wizard.restricted.contains(&digest));
        assert!(wizard.offers(Slot::Video).0.iter().all(|o| !o.open));
        let notice = wizard.private_notice().unwrap();
        assert!(
            notice.contains("vgabios-0.8a-unemu.1-std") || notice.contains("VGA BIOS copy"),
            "{notice}"
        );
    }

    #[test]
    fn a_package_refresh_keeps_the_users_choices_unless_the_bios_disappeared() {
        let store = FirmwareStore::new(false);
        let mut wizard = wizard(&store);
        let bios = wizard.selection().bios.clone();
        wizard.choose_option_rom(None);
        wizard.set_repo("lab".into());
        wizard.set_store(store.choices());
        assert_eq!(wizard.selection().bios, bios);
        assert_eq!(wizard.selection().option_rom, None);
        assert_eq!(wizard.repo, "lab");
        // Without the chosen BIOS the defaults apply again; the name stays.
        let (mut packages, tags, marks) = store.choices();
        packages.retain(|p| Some(&p.descriptor.digest) != bios.as_ref().map(|b| &b.0));
        wizard.set_store((packages, tags, marks));
        assert_eq!(wizard.selection().bios, None);
        wizard.set_store(store.choices());
        assert_eq!(wizard.selection().bios, bios);
        assert!(wizard.option_rom().is_some(), "XTIDE is the default again");
        assert_eq!(wizard.repo, "lab");
        // A preview from the real ROM bytes backs the summary.
        assert!(matches!(wizard.preview(), Some(Ok(_))));
    }

    #[test]
    fn what_only_the_rom_bytes_decide_becomes_a_candidate_reason() {
        let store = FirmwareStore::new(false);
        // The open VGA BIOS package lists fine, but its ROM blob is gone.
        let vga = store
            .choices()
            .0
            .into_iter()
            .find(|p| p.config.name == "vgabios-0.8a-unemu.1-std")
            .unwrap();
        // The stored blob may be a compressed frame named by its own digest.
        let decoded = vga.config.roms[0].blob.clone().unwrap();
        let blob = vga
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&decoded))
            .unwrap()
            .digest
            .clone();
        std::fs::remove_file(
            store
                .store
                .root()
                .join("images/blobs/sha256")
                .join(blob.hex()),
        )
        .unwrap();
        let wizard = wizard(&store);
        let (fits, misfits) = wizard.offers(Slot::Video);
        assert!(fits.is_empty());
        let reasons = misfits[0].reasons.join(" ");
        assert!(reasons.contains(blob.hex()), "{reasons}");
        // The draft alone cannot see it: the capabilities API allows the ROM.
        let draft = wizard.draft().unwrap();
        assert!(caps::choices(&draft, "firmware.video")
            .iter()
            .any(|c| c.value == Value::Name("vgabios-0.8a-unemu.1-std".into()) && c.enabled));
    }

    /// IBM Personal Computer AT Technical Reference, Fixed Disk Type Byte: the
    /// June 1985 BIOS serves only its CMOS disk types, so a geometry without a
    /// type is refused per option, before Create.
    #[test]
    #[ignore = "requires separately supplied IBM 5170 BIOS chips"]
    fn user_rom_cmos_typed_bios_refuses_untyped_geometries_per_option() {
        let catalogue = assembly::BoardModel::Ibm5170.firmware_catalogue();
        let found = assembly::require_user_firmware!("ibm-5170-bios-1985-06-10", catalogue);
        let store = FirmwareStore::new(false);
        let roms = store.temp.path().join("ibm");
        std::fs::create_dir(&roms).unwrap();
        let paths = match &found.source {
            assembly::ImageSource::Interleaved(parts) => {
                parts.iter().map(|p| p.path.clone()).collect::<Vec<_>>()
            }
            assembly::ImageSource::File(path) => vec![path.clone()],
            _ => panic!("discovery returns file-backed firmware"),
        };
        for (index, path) in paths.iter().enumerate() {
            found.expect_redacted(
                std::fs::copy(path, roms.join(format!("chip-{index}.bin"))),
                "copying firmware",
            );
        }
        let scan = pc_image::firmware_import::scan(&[roms], catalogue).unwrap();
        for candidate in &scan.candidates {
            pc_image::firmware_import::publish(&store.store, candidate, &[], catalogue).unwrap();
        }
        let mut wizard = wizard(&store);
        let ibm = wizard
            .offers(Slot::Bios)
            .0
            .into_iter()
            .find(|o| !o.open)
            .expect("the IBM package fits the IBM 5170");
        wizard.choose_bios(ibm.rom);
        // The IBM BIOS serves fixed disks itself, so no option ROM is chosen.
        assert_eq!(wizard.selection().option_rom, None);
        wizard.edit(|s| s.hardware.disks = [Some(create::Chs::new(615, 4, 17)), None]);
        assert!(
            wizard.problems().iter().all(|(step, _)| *step != DRIVES),
            "{:?}",
            wizard.problems()
        );
        let typed = wizard.candidate("typed", |s| {
            s.hardware.disks[0] = Some(create::Chs::new(940, 8, 17))
        });
        assert_eq!(typed, None, "type 4 is listed");
        let untyped = wizard.candidate("untyped", |s| {
            s.hardware.disks[0] = Some(create::Chs::new(1024, 16, 63))
        });
        assert!(untyped.is_some(), "1024/16/63 has no CMOS type");
        wizard.edit(|s| s.hardware.disks[0] = Some(create::Chs::new(614, 4, 17)));
        assert!(wizard.problems().iter().any(|(step, _)| *step == DRIVES));
    }

    #[test]
    fn every_step_renders_without_a_window() {
        let store = FirmwareStore::new(true);
        let mut wizard = wizard(&store);
        let ctx = egui::Context::default();
        for step in 0..STEPS.len() {
            wizard.go(step);
            for classic in [true, false] {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    assert!(matches!(
                        crate::new_machine_view::show(ui.ctx(), &mut wizard, classic),
                        crate::new_machine_view::Action::None
                    ));
                });
                output.textures_delta.clear();
            }
        }
        let mut empty = Wizard::new();
        empty.set_store(Default::default());
        empty.go(MOTHERBOARD);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = crate::new_machine_view::show(ui.ctx(), &mut empty, true);
        });
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Import ROM folder"))));
        output.textures_delta.clear();
    }
}
