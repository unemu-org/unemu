// SPDX-License-Identifier: AGPL-3.0-or-later
//! Descriptive metadata and licence statements of a firmware package, shared
//! by the Import ROM folder… review and Edit metadata…; machine-images §5.3.
//!
//! The editor holds the user's statement only. Every verdict it shows (the
//! licence of each layer, whether the README and the config are open, the
//! README text, and why an import or a revision is refused) is the library's
//! own preview of that statement, which the library worker computes from the
//! ROM and archive bytes. The program never marks a ROM open on its own: open
//! comes from the firmware catalogue or from a licence the user names, and
//! the library refuses open for bytes the catalogue lists as proprietary.

use super::firmware::{role_label, short};
use image_store::{
    firmware::{Config, Provenance, ProvenanceKind, Validator},
    license::{self, Expression},
    media_type, Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB,
};
use pc_image::{
    firmware::{PcFirmwareRules, BOARDS, DEVICE_KINDS, MACHINE_CLASSES},
    firmware_import::{self, Preview, ReadmeChoice, Statement},
};
use std::path::PathBuf;

/// The user's redistribution choice for the ROM images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rights {
    UserSupplied,
    Open,
}

/// What the firmware catalogue says about a package's ROMs, as the library
/// reads it: by bytes first, then by the listed ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Catalogue {
    /// Listed open under `license`; a copyleft licence still needs the source.
    Open { id: String, license: String },
    /// Listed, or matched by its bytes, as proprietary: never open.
    Proprietary { id: String },
    /// No catalogue entry: role, fits and licence are the user's claims.
    Unlisted,
}
impl Catalogue {
    pub fn of(config: &Config, catalogue: &[assembly::FirmwareEntry]) -> Result<Self, String> {
        let ids = config
            .roms
            .iter()
            .filter_map(|r| r.catalogue_id.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let locked = firmware_import::locked_by(config, catalogue);
        Ok(
            match firmware_import::catalogue_rights(config, catalogue).map_err(|e| e.to_string())? {
                Some(assembly::FirmwareDistribution::Open { license, .. }) => Self::Open {
                    id: ids,
                    license: license.into(),
                },
                Some(assembly::FirmwareDistribution::UserSupplied) => Self::Proprietary {
                    id: locked.map_or(ids, |entry| entry.id.into()),
                },
                None => Self::Unlisted,
            },
        )
    }
    fn listed(&self) -> bool {
        !matches!(self, Self::Unlisted)
    }
}

/// An SPDX licence the user names: an identifier from the parser's list, or
/// a free expression, which the parser validates as it is typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LicenseChoice {
    pub text: String,
    /// The free field is shown, for an expression the list does not hold.
    pub free: bool,
}
const OTHER_EXPRESSION: &str = "Other SPDX expression…";
impl LicenseChoice {
    pub fn of(text: Option<&str>) -> Self {
        let text = text.unwrap_or_default().to_owned();
        Self {
            free: !text.is_empty() && !license::identifiers().contains(&text.as_str()),
            text,
        }
    }
    pub fn stated(&self) -> Option<String> {
        let text = self.text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
    /// The parser's refusal of the stated expression, if any.
    pub fn problem(&self) -> Option<String> {
        self.stated()
            .and_then(|text| Expression::parse(&text).err().map(|e| e.to_string()))
    }
    fn expression(&self) -> Option<Expression> {
        self.stated().and_then(|text| Expression::parse(&text).ok())
    }
    fn picker(
        &mut self,
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash + std::fmt::Debug,
        none: &str,
    ) {
        let shown = if self.free {
            OTHER_EXPRESSION.to_owned()
        } else if self.text.is_empty() {
            none.to_owned()
        } else {
            self.text.clone()
        };
        egui::ComboBox::from_id_salt(salt)
            .selected_text(shown)
            .width(220.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(!self.free && self.text.is_empty(), none)
                    .clicked()
                {
                    *self = Self::default();
                }
                let (copyleft, other): (Vec<&str>, Vec<&str>) =
                    license::identifiers().iter().partition(|id| {
                        Expression::parse(id).is_ok_and(|e| e.needs_source(license::ACTIVE))
                    });
                for (group, ids) in [
                    ("Copyleft — needs a source archive", copyleft),
                    ("Other licences", other),
                ] {
                    ui.weak(group);
                    for id in ids {
                        if ui
                            .selectable_label(!self.free && self.text == id, id)
                            .clicked()
                        {
                            *self = Self::of(Some(id));
                        }
                    }
                }
                if ui.selectable_label(self.free, OTHER_EXPRESSION).clicked() {
                    self.free = true;
                }
            });
        if self.free {
            ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .hint_text("e.g. MIT OR Apache-2.0")
                    .desired_width(220.0),
            );
            if let Some(problem) = self.problem() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "Not a valid SPDX expression: {problem}. Use identifiers from the SPDX \
                         licence list (or LicenseRef-…) joined by AND, OR or WITH."
                    ),
                );
            }
        }
    }
}

/// How the bytes were obtained. Revision and address come from the command
/// line and are kept as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProvenanceFields {
    pub kind: Option<ProvenanceKind>,
    pub from: String,
    pub date: String,
    revision: Option<String>,
    url: Option<String>,
}
impl ProvenanceFields {
    fn of(provenance: Option<&Provenance>) -> Self {
        provenance.map_or_else(Self::default, |p| Self {
            kind: p.kind,
            from: p.from.clone(),
            date: p.date.clone().unwrap_or_default(),
            revision: p.revision.clone(),
            url: p.url.clone(),
        })
    }
    /// Blank fields clear: the library drops an empty provenance. Blank
    /// details with no kind also drop the revision and address the dialog
    /// does not show, so clearing what is shown clears the provenance.
    fn statement(&self) -> Provenance {
        let cleared = self.kind.is_none() && self.from.trim().is_empty();
        Provenance {
            kind: self.kind,
            from: self.from.trim().to_owned(),
            revision: self.revision.clone().filter(|_| !cleared),
            url: self.url.clone().filter(|_| !cleared),
            date: Some(self.date.trim().to_owned()),
        }
    }
}
fn provenance_label(kind: Option<ProvenanceKind>) -> &'static str {
    match kind {
        Some(ProvenanceKind::Dumped) => "Dumped from a board",
        Some(ProvenanceKind::Downloaded) => "Downloaded from",
        Some(ProvenanceKind::Built) => "Built from a revision",
        None => "Not stated",
    }
}
fn provenance_hint(kind: Option<ProvenanceKind>) -> &'static str {
    match kind {
        Some(ProvenanceKind::Dumped) => "Board, chip positions and markings, reader",
        Some(ProvenanceKind::Downloaded) => "Named source and file",
        Some(ProvenanceKind::Built) => "Repository, revision and patches",
        None => "How the bytes were obtained",
    }
}
/// The pane's provenance lines: source, details and date.
pub(crate) fn provenance_lines(provenance: Option<&Provenance>) -> [String; 3] {
    match provenance {
        None => ["Not recorded".into(), "—".into(), "Unknown".into()],
        Some(p) => [
            provenance_label(p.kind).into(),
            p.from.clone(),
            p.date.clone().unwrap_or_else(|| "Unknown".into()),
        ],
    }
}

/// A source archive as a package or a preview holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceFile {
    pub filename: String,
    pub format: String,
    pub size: u64,
    pub license: Option<String>,
}

/// Facts about the package that the editor does not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Facts {
    pub catalogue: Catalogue,
    /// Boards and classes a catalogue ROM may narrow; `None` when unlisted.
    pub catalogue_fit: Option<(Vec<String>, Vec<String>)>,
    /// The source archive the package already holds (Edit metadata… only).
    pub stored_source: Option<SourceFile>,
    /// Required devices are claims of one unlisted ROM, else catalogue facts.
    pub single_rom: bool,
}

/// The fields the user edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fields {
    pub title: String,
    pub description: String,
    pub notes: String,
    pub labels: Vec<String>,
    pub boards: Vec<String>,
    pub classes: Vec<String>,
    pub requires: Vec<String>,
    pub provenance: ProvenanceFields,
    pub rights: Rights,
    /// The ROM images' licence when open and not catalogue-open.
    pub license: LicenseChoice,
    /// A newly chosen source archive.
    pub source: Option<PathBuf>,
    /// The archive's own licence statement, needed unless the archive is the
    /// one the project pins for a catalogue ROM or keeps the licence it has.
    pub source_license: LicenseChoice,
    /// The README text once edited by hand.
    pub readme: Option<String>,
}
impl Fields {
    /// An import's catalogue prefill: catalogue-open firmware is preset open
    /// with the catalogue licence, everything else user-supplied.
    pub fn prefill(config: &Config, facts: &Facts) -> Self {
        let mut fields = Self::of(config, None);
        if matches!(facts.catalogue, Catalogue::Open { .. }) {
            fields.rights = Rights::Open;
        }
        fields
    }
    /// A stored package's current values: open when its ROM images carry
    /// the open `license`; a catalogue-open ROM takes the catalogue's.
    pub fn stored(config: &Config, facts: &Facts, license: Option<&str>) -> Self {
        let mut fields = Self::of(config, license);
        if facts.catalogue.listed() {
            fields.license = LicenseChoice::default();
        }
        fields
    }
    /// A user-supplied ROM needs no source archive, so a newly chosen one and
    /// its licence go with the open choice instead of staying hidden.
    pub fn set_rights(&mut self, rights: Rights) {
        self.rights = rights;
        if rights == Rights::UserSupplied {
            self.source = None;
            self.source_license = LicenseChoice::default();
        }
    }
    /// A config's fields, with the ROM images' current redistribution.
    pub fn of(config: &Config, license: Option<&str>) -> Self {
        let fit = pc_image::firmware::Fit::decode(&config.fits).unwrap_or_default();
        let mut requires: Vec<String> = config
            .roms
            .iter()
            .flat_map(|r| r.requires.iter().cloned())
            .collect();
        requires.sort();
        requires.dedup();
        Self {
            title: config.name.clone(),
            description: config.description.clone().unwrap_or_default(),
            notes: config.notes.clone().unwrap_or_default(),
            labels: config.labels.clone(),
            boards: fit.boards,
            classes: fit.machine_classes,
            requires,
            provenance: ProvenanceFields::of(config.provenance.as_ref()),
            rights: if license.is_some() {
                Rights::Open
            } else {
                Rights::UserSupplied
            },
            license: LicenseChoice::of(license),
            source: None,
            source_license: LicenseChoice::default(),
            readme: None,
        }
    }
}

/// One row of the layers table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayerRow {
    pub name: String,
    pub kind: String,
    pub license: Option<String>,
    /// The package config, whose class follows every other layer.
    pub config: bool,
}
/// `GPL-3.0-or-later · open`, or the user-supplied wording.
pub(crate) fn layer_text(license: Option<&str>) -> String {
    match license {
        Some(license) => format!("{license} · open"),
        None => "No licence recorded · user-supplied".into(),
    }
}
fn class(d: &Descriptor) -> Option<String> {
    license::open_license(d).map(|l| l.as_str().to_owned())
}
/// The name a ROM or chip layer carries in the chip layout.
fn rom_layer_name(config: &Config, digest: &Digest) -> String {
    for rom in &config.roms {
        if rom.blob.as_ref() == Some(digest) {
            return [&rom.position, &rom.part_number]
                .into_iter()
                .flatten()
                .cloned()
                .reduce(|a, b| format!("{a} · {b}"))
                .unwrap_or_else(|| format!("Single image · {}", role_label(&rom.role)));
        }
        if let Some(part) = rom.parts.iter().find(|p| &p.blob == digest) {
            return [&part.position, &part.part_number]
                .into_iter()
                .flatten()
                .cloned()
                .reduce(|a, b| format!("{a} · {b}"))
                .unwrap_or_else(|| "Chip".into());
        }
    }
    short(digest.hex())
}

/// The library's verdict on one statement, taken from its preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub readme: String,
    pub notes: Vec<String>,
    /// Every layer in manifest order, then the config.
    pub layers: Vec<LayerRow>,
    /// The ROM images' licence, `None` when user-supplied.
    pub rom_license: Option<String>,
    pub source: Option<SourceFile>,
    pub config_open: bool,
    /// An edit with this statement would publish the very same package.
    pub unchanged: bool,
}
impl Verdict {
    pub fn of(preview: &Preview) -> Self {
        let config = &preview.config;
        let manifest = &preview.manifest;
        let mut layers = Vec::new();
        let mut rom_license = None;
        let mut source = None;
        for d in &manifest.layers {
            let digest = d.decoded_digest().unwrap_or_else(|_| d.digest.clone());
            let license = class(d);
            let (name, kind) = match d.decoded_media_type() {
                media_type::FIRMWARE => {
                    rom_license = license.clone();
                    (rom_layer_name(config, &digest), "ROM image".to_owned())
                }
                media_type::TAR | media_type::ZIP => {
                    let s = config.sources.iter().find(|s| s.blob == digest);
                    let name = s.map_or_else(|| short(digest.hex()), |s| s.filename.clone());
                    if let Some(s) = s {
                        source = Some(SourceFile {
                            filename: s.filename.clone(),
                            format: s.format.as_str().into(),
                            size: s.size,
                            license: license.clone(),
                        });
                    }
                    let format = s.map_or("archive", |s| s.format.as_str());
                    (name, format!("Source archive · {format}"))
                }
                media_type::MARKDOWN => ("README.md".into(), "README · UTF-8 Markdown".into()),
                _ => {
                    let extra = config.extras.iter().find(|e| e.blob == digest);
                    (
                        extra.map_or_else(|| short(digest.hex()), |e| e.filename.clone()),
                        extra.map_or("Document".into(), |e| extra_kind(e.kind).into()),
                    )
                }
            };
            layers.push(LayerRow {
                name,
                kind,
                license,
                config: false,
            });
        }
        let config_open = license::open_license(&manifest.config).is_some();
        layers.push(LayerRow {
            name: "Config".into(),
            kind: "Package metadata".into(),
            license: class(&manifest.config),
            config: true,
        });
        Self {
            readme: preview.readme.clone(),
            notes: preview.notes.clone(),
            layers,
            rom_license,
            source,
            config_open,
            unchanged: false,
        }
    }
}
pub(crate) fn extra_kind(kind: image_store::media_set::ExtraKind) -> &'static str {
    use image_store::media_set::ExtraKind;
    match kind {
        ExtraKind::Document => "Document",
        ExtraKind::Archive => "Archive",
        ExtraKind::Executable => "Executable",
        ExtraKind::Other => "Other",
    }
}

/// The library's reasons name command-line flags; the dialogs name the
/// controls that make the same statement.
pub(crate) fn gui_reason(reason: &str) -> String {
    let reason = ["invalid PC image: ", "invalid image: "]
        .iter()
        .find_map(|prefix| reason.strip_prefix(prefix))
        .unwrap_or(reason);
    let mut text = reason.to_owned();
    for (flag, control) in [
        (" with --open --source-license SPDX", ""),
        (
            " with --source and --source-license SPDX",
            " with Source archive… and state its licence",
        ),
        (" with --source-license SPDX", ""),
        (" with --source", " with Source archive…"),
        ("--open needs --license SPDX", "Open needs an SPDX licence"),
        ("; add --open", "; choose Open"),
        (" (--source)", " (Source archive…)"),
    ] {
        text = text.replace(flag, control);
    }
    text
}

/// End a reason with a stop, unless it already ends with one or an ellipsis.
pub(crate) fn stop(text: &str) -> String {
    if text.ends_with(['.', '…']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}
/// A reason on its own line: capitalised and ended with a stop.
pub(crate) fn sentence(text: &str) -> String {
    match text.get(..1) {
        Some(first) => stop(&format!("{}{}", first.to_uppercase(), &text[1..])),
        None => String::new(),
    }
}

/// The user's statement the mock-up places with the source archive choice.
pub(crate) const SOURCE_STATEMENT: &str = "You state that this archive is the corresponding \
    source. The program checks that it is a readable zip or tar archive and records its \
    SHA-256; it does not build it or compare it with the ROM.";

/// What the editor asks its dialog to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorAction {
    None,
    /// Run the host file picker for the source archive.
    PickSource,
}

/// One package's statement under edit, with the library's latest verdict.
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub facts: Facts,
    initial: Fields,
    pub fields: Fields,
    /// Raised with every change; a verdict answers one revision.
    pub revision: u64,
    verdict: Option<(u64, Result<Verdict, String>)>,
    /// The newest accepted verdict, shown while a newer one is pending or refused.
    last_ok: Option<Verdict>,
    in_flight: Option<u64>,
    label: String,
    label_problem: Option<String>,
    readme_buffer: String,
}
impl Draft {
    pub fn new(facts: Facts, fields: Fields) -> Self {
        Self {
            facts,
            initial: fields.clone(),
            fields,
            revision: 0,
            verdict: None,
            last_ok: None,
            in_flight: None,
            label: String::new(),
            label_problem: None,
            readme_buffer: String::new(),
        }
    }
    /// The statement an import or an edit applies: descriptive fields only
    /// where they changed, so `None` keeps the prefill or the current value,
    /// and the redistribution choice always explicit, so a choice that
    /// cannot hold is refused rather than quietly downgraded.
    pub fn statement(&self) -> Statement {
        let (f, i) = (&self.fields, &self.initial);
        let open = f.rights == Rights::Open;
        let archive = f.source.is_some() || self.facts.stored_source.is_some();
        let changed = |now: &String, then: &String| (now != then).then(|| now.clone());
        Statement {
            title: (f.title != i.title).then(|| f.title.trim().to_owned()),
            description: changed(&f.description, &i.description),
            notes: changed(&f.notes, &i.notes),
            labels: (f.labels != i.labels).then(|| f.labels.clone()),
            boards: (f.boards != i.boards).then(|| f.boards.clone()),
            machine_classes: (f.classes != i.classes).then(|| f.classes.clone()),
            requires: (f.requires != i.requires).then(|| f.requires.clone()),
            provenance: (f.provenance != i.provenance).then(|| f.provenance.statement()),
            open,
            license: (open && !matches!(self.facts.catalogue, Catalogue::Open { .. }))
                .then(|| f.license.stated())
                .flatten(),
            user_supplied: !open,
            source: f.source.clone(),
            source_license: (open && archive)
                .then(|| f.source_license.stated())
                .flatten(),
            // Regenerate from fields after a hand-written README asks the
            // library to generate it; otherwise a stored one would be kept.
            readme: match (&f.readme, &i.readme) {
                (Some(text), _) => ReadmeChoice::Text(text.clone()),
                (None, Some(_)) => ReadmeChoice::Generate,
                (None, None) => ReadmeChoice::Keep,
            },
            ..Statement::default()
        }
    }
    /// The fields that differ from where the editor started.
    pub fn changes(&self) -> Vec<&'static str> {
        let (f, i) = (&self.fields, &self.initial);
        [
            ("title", f.title != i.title),
            ("description", f.description != i.description),
            ("notes", f.notes != i.notes),
            ("labels", f.labels != i.labels),
            ("boards", f.boards != i.boards),
            ("machine classes", f.classes != i.classes),
            ("required devices", f.requires != i.requires),
            ("provenance", f.provenance != i.provenance),
            ("redistribution", f.rights != i.rights),
            ("licence", f.license != i.license),
            ("source archive", f.source.is_some()),
            ("archive licence", f.source_license != i.source_license),
            ("README", f.readme != i.readme),
        ]
        .into_iter()
        .filter_map(|(name, changed)| changed.then_some(name))
        .collect()
    }
    /// The statement to preview next, once per revision and one at a time.
    pub fn wants_preview(&mut self) -> Option<(u64, Statement)> {
        if self.in_flight.is_some() || self.current().is_some() {
            return None;
        }
        self.in_flight = Some(self.revision);
        Some((self.revision, self.statement()))
    }
    pub fn previewed(&mut self, revision: u64, result: Result<Verdict, String>) {
        if self.in_flight == Some(revision) {
            self.in_flight = None;
        }
        if revision > self.revision {
            return;
        }
        if let Ok(verdict) = &result {
            self.last_ok = Some(verdict.clone());
        }
        self.verdict = Some((revision, result));
    }
    /// The worker stopped: no answer is coming.
    pub fn stopped(&mut self) {
        self.in_flight = None;
    }
    pub fn waiting(&self) -> bool {
        self.in_flight.is_some()
    }
    /// The verdict on the statement as it stands now.
    pub fn current(&self) -> Option<&Result<Verdict, String>> {
        self.verdict
            .as_ref()
            .filter(|(revision, _)| *revision == self.revision)
            .map(|(_, verdict)| verdict)
    }
    /// The verdict the library gave on the statement as it stands, if accepted.
    pub fn accepted(&self) -> Option<&Verdict> {
        self.current().and_then(|v| v.as_ref().ok())
    }
    /// Why the statement cannot be published now, in the dialog's words.
    pub fn problem(&self) -> Option<String> {
        match self.current() {
            None => Some("checking…".into()),
            Some(Ok(_)) => None,
            Some(Err(reason)) => Some(gui_reason(reason)),
        }
    }
    /// The ROM licence the user's choice names: the catalogue's, or the stated one.
    fn chosen_license(&self) -> Option<Expression> {
        if self.fields.rights != Rights::Open {
            return None;
        }
        match &self.facts.catalogue {
            Catalogue::Open { license, .. } => Expression::parse(license).ok(),
            _ => self.fields.license.expression(),
        }
    }
    fn narrowed(&self) -> bool {
        self.facts
            .catalogue_fit
            .as_ref()
            .is_some_and(|(boards, classes)| {
                &self.fields.boards != boards || &self.fields.classes != classes
            })
    }

    /// Draw the editor; any change raises the revision for a new preview.
    pub fn show(&mut self, ui: &mut egui::Ui, salt: &str) -> EditorAction {
        let before = self.fields.clone();
        let mut action = EditorAction::None;
        ui.columns(2, |columns| {
            self.describe(&mut columns[0], salt);
            self.rights(&mut columns[1], salt, &mut action);
        });
        self.readme(ui, salt);
        if before.rights == Rights::Open && self.fields.rights == Rights::UserSupplied {
            self.fields.set_rights(Rights::UserSupplied);
        }
        if self.fields != before {
            self.revision += 1;
        }
        action
    }
    fn describe(&mut self, ui: &mut egui::Ui, salt: &str) {
        let listed = self.facts.catalogue.listed();
        let fields = &mut self.fields;
        egui::CollapsingHeader::new("Title and description")
            .id_salt((salt, "title"))
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new((salt, "title-grid"))
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Title");
                        ui.add(egui::TextEdit::singleline(&mut fields.title).desired_width(260.0));
                        ui.end_row();
                        ui.label("Description");
                        ui.add(
                            egui::TextEdit::multiline(&mut fields.description)
                                .desired_rows(3)
                                .desired_width(260.0),
                        );
                        ui.end_row();
                        ui.label("Notes and known quirks");
                        ui.add(
                            egui::TextEdit::multiline(&mut fields.notes)
                                .desired_rows(2)
                                .desired_width(260.0),
                        );
                        ui.end_row();
                    });
                ui.weak(
                    "Also written as org.opencontainers.image.title; the description as \
                     …image.description.",
                );
            });
        egui::CollapsingHeader::new("Labels")
            .id_salt((salt, "labels"))
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    remove_chips(ui, &mut fields.labels, true);
                    let input = ui.add(
                        egui::TextEdit::singleline(&mut self.label)
                            .hint_text("new label")
                            .desired_width(120.0),
                    );
                    let entered =
                        input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Add").clicked() || entered {
                        self.label_problem = add_label(&mut fields.labels, &self.label);
                        if self.label_problem.is_none() {
                            self.label.clear();
                        }
                    }
                });
                match &self.label_problem {
                    Some(problem) => {
                        ui.colored_label(ui.visuals().warn_fg_color, problem);
                    }
                    None => {
                        ui.weak(
                            "Lower-case letters, digits, '.', '_' and '-', for example ami, \
                             turbo or xt-ide.",
                        );
                    }
                }
            });
        let narrowed = self.narrowed();
        let fields = &mut self.fields;
        egui::CollapsingHeader::new("Fits")
            .id_salt((salt, "fits"))
            .default_open(true)
            .show(ui, |ui| {
                for (title, items, vocabulary, removable, addable) in [
                    ("Boards", &mut fields.boards, BOARDS, true, !listed),
                    (
                        "Machine classes",
                        &mut fields.classes,
                        MACHINE_CLASSES,
                        true,
                        !listed,
                    ),
                    (
                        "Required devices",
                        &mut fields.requires,
                        DEVICE_KINDS,
                        !listed && self.facts.single_rom,
                        !listed && self.facts.single_rom,
                    ),
                ] {
                    ui.weak(title);
                    ui.horizontal_wrapped(|ui| {
                        if items.is_empty() {
                            ui.weak(if title == "Required devices" {
                                "None recorded"
                            } else {
                                "Unspecified (not universal)"
                            });
                        }
                        remove_chips(ui, items, removable);
                        if addable {
                            let spare: Vec<&str> = vocabulary
                                .iter()
                                .copied()
                                .filter(|v| !items.iter().any(|i| i == v))
                                .collect();
                            if !spare.is_empty() {
                                egui::ComboBox::from_id_salt((salt, title, "add"))
                                    .selected_text("Add…")
                                    .show_ui(ui, |ui| {
                                        for value in spare {
                                            if ui.selectable_label(false, value).clicked() {
                                                items.push(value.into());
                                                items.sort();
                                            }
                                        }
                                    });
                            }
                        }
                    });
                }
                match &self.facts.catalogue {
                    Catalogue::Unlisted => ui.weak(
                        "Your claims. They stay unverified until a provider checks them and \
                         never enable Use on their own.",
                    ),
                    Catalogue::Open { id, .. } | Catalogue::Proprietary { id } => ui.weak(format!(
                        "From the catalogue entry {id}. You can narrow boards and classes but \
                         not widen them; required devices are facts of the ROM."
                    )),
                };
                if narrowed && ui.button("Restore catalogue fits").clicked() {
                    if let Some((boards, classes)) = &self.facts.catalogue_fit {
                        fields.boards = boards.clone();
                        fields.classes = classes.clone();
                    }
                }
            });
    }
    fn rights(&mut self, ui: &mut egui::Ui, salt: &str, action: &mut EditorAction) {
        let fields = &mut self.fields;
        egui::CollapsingHeader::new("Provenance")
            .id_salt((salt, "provenance"))
            .default_open(true)
            .show(ui, |ui| {
                let p = &mut fields.provenance;
                ui.horizontal_wrapped(|ui| {
                    for kind in [
                        Some(ProvenanceKind::Dumped),
                        Some(ProvenanceKind::Downloaded),
                        Some(ProvenanceKind::Built),
                        None,
                    ] {
                        ui.radio_value(&mut p.kind, kind, provenance_label(kind));
                    }
                });
                egui::Grid::new((salt, "provenance-grid"))
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Details");
                        ui.add(
                            egui::TextEdit::singleline(&mut p.from)
                                .hint_text(provenance_hint(p.kind))
                                .desired_width(260.0),
                        );
                        ui.end_row();
                        ui.label("Date");
                        ui.add(
                            egui::TextEdit::singleline(&mut p.date)
                                .hint_text("YYYY-MM-DD")
                                .desired_width(110.0),
                        );
                        ui.end_row();
                    });
                ui.weak("When the bytes were obtained. Blank means unknown; the clock never fills it in.");
            });
        let chosen = self.chosen_license();
        // The ROM licence of an accepted verdict whose open source layer meets it.
        let confirmed = self.accepted().and_then(|v| {
            let rom = v.rom_license.clone()?;
            let needs = Expression::parse(&rom).ok()?.needs_source(license::ACTIVE);
            (needs && v.source.as_ref()?.license.is_some()).then_some(rom)
        });
        let stored = self.facts.stored_source.clone();
        let catalogue = self.facts.catalogue.clone();
        let fields = &mut self.fields;
        egui::CollapsingHeader::new("Redistribution and licence")
            .id_salt((salt, "rights"))
            .default_open(true)
            .show(ui, |ui| {
                let proprietary = matches!(catalogue, Catalogue::Proprietary { .. });
                if proprietary {
                    fields.rights = Rights::UserSupplied;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(
                        &mut fields.rights,
                        Rights::UserSupplied,
                        "User-supplied (private only)",
                    );
                    ui.add_enabled_ui(!proprietary, |ui| {
                        ui.radio_value(&mut fields.rights, Rights::Open, "Open (redistributable)");
                    });
                });
                let open = fields.rights == Rights::Open;
                match &catalogue {
                    Catalogue::Proprietary { id } => {
                        ui.weak(format!(
                            "The catalogue lists this ROM as proprietary ({id}), so it stays \
                             user-supplied."
                        ));
                    }
                    Catalogue::Open { license, .. } if open => {
                        ui.weak(format!("Licence from the catalogue: {license}."));
                    }
                    Catalogue::Open { license, .. } => {
                        ui.weak(format!(
                            "The catalogue lists it as open under {license}; keeping it \
                             user-supplied is always allowed."
                        ));
                    }
                    Catalogue::Unlisted if open => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("SPDX licence");
                            fields.license.picker(
                                ui,
                                (salt, "rom-licence"),
                                "— Choose a licence —",
                            );
                        });
                        ui.weak(
                            "Open needs your explicit statement of the licence. The program \
                             never marks a ROM open on its own.",
                        );
                    }
                    Catalogue::Unlisted => {
                        ui.weak(
                            "Not in the catalogue, so it is user-supplied unless you state its \
                             licence: choose Open, then an SPDX licence.",
                        );
                    }
                }
                if !open {
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label("Source archive:");
                    match (&fields.source, &stored) {
                        (Some(path), _) => {
                            ui.strong(
                                path.file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            );
                        }
                        (None, Some(stored)) => {
                            ui.strong(&stored.filename);
                            ui.weak(format!(
                                "{} · {}",
                                stored.format,
                                super::human_size(stored.size)
                            ));
                        }
                        (None, None) => {
                            ui.weak("none");
                        }
                    }
                    if ui.button("Source archive…").clicked() {
                        *action = EditorAction::PickSource;
                    }
                    if fields.source.is_some() && ui.button("Remove").clicked() {
                        fields.source = None;
                    }
                });
                ui.weak(SOURCE_STATEMENT);
                if fields.source.is_some() || stored.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Archive licence");
                        fields
                            .source_license
                            .picker(ui, (salt, "source-licence"), "Not stated");
                    });
                    ui.weak(
                        "Not stated keeps the licence the archive has or, for the archive the \
                         project pins for a catalogue ROM, the catalogue licence. State the \
                         licence of any other archive.",
                    );
                }
                if let Some(license) = &chosen {
                    let attached = fields.source.is_some() || stored.is_some();
                    match (license.needs_source(license::ACTIVE), attached, &confirmed) {
                        // Only the library's accepted verdict confirms the source.
                        (true, _, Some(rom)) => {
                            ui.colored_label(
                                egui::Color32::from_rgb(31, 107, 51),
                                format!("✓ Corresponding source attached, as {rom} requires."),
                            );
                        }
                        (true, false, None) => {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!(
                                    "Required: {license} is a copyleft licence. Choose the \
                                     archive, or keep the set user-supplied."
                                ),
                            );
                        }
                        (true, true, None) => (),
                        (false, ..) => {
                            ui.weak(format!("Optional for {license}."));
                        }
                    }
                }
            });
        egui::CollapsingHeader::new("Layers and licences")
            .id_salt((salt, "layers"))
            .default_open(true)
            .show(ui, |ui| match &self.last_ok {
                None => {
                    ui.weak("Checking the layers…");
                }
                Some(verdict) => {
                    egui::Grid::new((salt, "layer-grid"))
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            for heading in ["Layer", "Kind", "Licence · redistribution"] {
                                ui.strong(heading);
                            }
                            ui.end_row();
                            for row in &verdict.layers {
                                ui.label(&row.name);
                                ui.label(&row.kind);
                                ui.label(if row.config {
                                    if row.license.is_some() {
                                        "Open — every layer is open".to_owned()
                                    } else {
                                        "User-supplied — follows its layers".to_owned()
                                    }
                                } else {
                                    layer_text(row.license.as_deref())
                                });
                                ui.end_row();
                            }
                        });
                    ui.weak(if verdict.config_open {
                        "Every ROM, document and source layer is open, so README.md (CC0-1.0) \
                         and the config are open too."
                    } else {
                        "At least one layer is user-supplied, so README.md and the config are \
                         user-supplied too."
                    });
                }
            });
    }
    fn readme(&mut self, ui: &mut egui::Ui, salt: &str) {
        ui.strong("README.md — generated from these fields");
        let edited = self.fields.readme.is_some();
        if let (false, Some(verdict)) = (edited, &self.last_ok) {
            self.readme_buffer.clone_from(&verdict.readme);
        }
        let ready = edited || self.last_ok.is_some();
        let response = ui.add_enabled(
            ready,
            egui::TextEdit::multiline(&mut self.readme_buffer)
                .id_salt((salt, "readme"))
                .code_editor()
                .desired_rows(10)
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.fields.readme = Some(self.readme_buffer.clone());
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(edited, egui::Button::new("Regenerate from fields"))
                .clicked()
            {
                self.fields.readme = None;
            }
            ui.weak(if edited {
                "Edited by hand: field changes no longer update it."
            } else {
                "Follows the fields until you edit it. Stored as its own plain UTF-8 Markdown \
                 layer."
            });
        });
        if self.last_ok.as_ref().is_some_and(|v| v.config_open) {
            ui.weak("Every layer is open, so this README is published under CC0-1.0.");
        }
    }
}

/// Chips with a remove button each, or read-only chips.
fn remove_chips(ui: &mut egui::Ui, items: &mut Vec<String>, removable: bool) {
    let mut removed = None;
    for (index, item) in items.iter().enumerate() {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(4, 1))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.monospace(item);
                    if removable
                        && ui
                            .small_button("×")
                            .on_hover_text(format!("Remove {item}"))
                            .clicked()
                    {
                        removed = Some(index);
                    }
                });
            });
    }
    if let Some(index) = removed {
        items.remove(index);
    }
}
/// Add a label in the media id grammar the config requires; the reason when refused.
fn add_label(labels: &mut Vec<String>, text: &str) -> Option<String> {
    let label = text.trim();
    if label.is_empty() {
        return Some("Type a label first.".into());
    }
    if !image_store::media_set::valid_id(label) {
        return Some(format!(
            "“{label}” is not valid: use lower-case letters, digits, '.', '_' and '-', starting \
             with a letter or digit, up to 64 characters."
        ));
    }
    if labels.iter().any(|l| l == label) {
        return Some(format!("“{label}” is already listed."));
    }
    labels.push(label.into());
    labels.sort();
    None
}

/// The pane's reading of a selected package that needs the store: its
/// README text and where the push guard lets it go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageDetails {
    pub readme: Option<String>,
    /// The guard's effective class of the config.
    pub config_open: bool,
    pub push: Push,
    /// An open ROM whose licence obliges the source travels with it.
    pub source_layer: bool,
}
pub(crate) use image_registry::guard::Reach as Push;
/// The pane's Push line.
pub(crate) fn push_text(details: &PackageDetails) -> String {
    match details.push {
        Push::Anywhere if details.source_layer => {
            "unemu/ and private registries · source layer present".into()
        }
        Push::Anywhere => "unemu/ and private registries".into(),
        Push::UserSupplied => "Private registries only: user-supplied layers".into(),
        Push::CopyleftWithoutSource => {
            "Private registries only: copyleft ROM without its source layer".into()
        }
    }
}

/// Worker side of the pane: the command line's push plan of the tag (every
/// local mark and the catalogue's), and the README layer's text.
pub(crate) fn read_package(
    store: &Store,
    tag: &str,
    digest: &Digest,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<PackageDetails, String> {
    use image_registry::guard;
    let plan = cli::push_plan(
        store,
        tag,
        crate::registry_transfer::dry_run_target(),
        None,
        catalogue,
    )
    .map_err(|e| e.to_string())?;
    if &plan.image().digest != digest {
        return Err("the tag now names another package; select it again".into());
    }
    let closure = plan.closure();
    let config_open = closure
        .iter()
        .find(|c| c.descriptor.media_type == media_type::FIRMWARE_CONFIG)
        .is_some_and(|c| c.effective == guard::Redistribution::Open);
    let source_layer = closure.iter().any(|c| {
        c.descriptor.decoded_media_type() == media_type::FIRMWARE
            && license::open_license(&c.descriptor).is_some_and(|l| l.needs_source(license::ACTIVE))
    });
    let readme = match closure
        .iter()
        .find(|c| c.descriptor.decoded_media_type() == media_type::MARKDOWN)
    {
        Some(c) => Some(
            String::from_utf8(
                image_store::BlobSource::read_verified(store, &c.descriptor, MAX_SMALL_BLOB)
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|_| "the README layer is not UTF-8 text".to_owned())?,
        ),
        None => None,
    };
    Ok(PackageDetails {
        readme,
        config_open,
        source_layer,
        // The same helper and order of reasons as `image firmware show`.
        push: guard::reach(closure),
    })
}

/// What Edit metadata… starts from, read on the worker.
#[derive(Clone, Debug)]
pub(crate) struct EditSource {
    /// The new revision's tag, `:N` to `:N+1` as the command line names it.
    pub target: String,
    pub name: String,
    pub draft: Draft,
}

/// Read a stored package for Edit metadata…: its fields, the current class
/// of its ROM images and source archive, and its README when written by hand.
pub(crate) fn open_edit(
    store: &Store,
    tag: &str,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<EditSource, String> {
    let target = firmware_import::next_revision(tag).ok_or_else(|| {
        format!("{tag} has no numeric revision, so no new revision can follow it")
    })?;
    if store.tag_descriptor(&target).is_ok() {
        return Err(format!(
            "{target} already exists; edit the newest revision of the package"
        ));
    }
    let image = store
        .resolve(tag, &Validator(&PcFirmwareRules(catalogue)))
        .map_err(|e| e.to_string())?;
    let config = Config::decode(&image.config).map_err(|e| e.to_string())?;
    let manifest: &Manifest = &image.manifest;
    let layer = |digest: &Digest| manifest.layers.iter().find(|d| d.names(digest));
    let rom_license =
        firmware_import::current_rom_license(&config, manifest).map(|l| l.as_str().to_owned());
    let catalogue_kind = Catalogue::of(&config, catalogue)?;
    let ids: Vec<&str> = config
        .roms
        .iter()
        .filter_map(|r| r.catalogue_id.as_deref())
        .collect();
    let catalogue_fit = match ids.as_slice() {
        [id] => catalogue.iter().find(|e| e.id == *id).map(|entry| {
            let fit = firmware_import::catalogue_fit(entry);
            (fit.boards, fit.machine_classes)
        }),
        _ => None,
    };
    let stored_source = config.sources.first().map(|s| SourceFile {
        filename: s.filename.clone(),
        format: s.format.as_str().into(),
        size: s.size,
        license: layer(&s.blob).and_then(class),
    });
    // Only a README written by hand starts as edited; the library keeps it.
    let readme = match manifest.layers.iter().find(|d| {
        d.decoded_media_type() == media_type::MARKDOWN && firmware_import::hand_written_readme(d)
    }) {
        Some(d) => Some(
            String::from_utf8(
                image_store::BlobSource::read_verified(store, d, MAX_SMALL_BLOB)
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|_| "the README layer is not UTF-8 text".to_owned())?,
        ),
        None => None,
    };
    let facts = Facts {
        catalogue: catalogue_kind,
        catalogue_fit,
        stored_source,
        single_rom: config.roms.len() == 1,
    };
    let mut fields = Fields::stored(&config, &facts, rom_license.as_deref());
    fields.readme = readme;
    Ok(EditSource {
        target,
        name: config.name.clone(),
        draft: Draft::new(facts, fields),
    })
}
/// Worker side of a preview of Edit metadata….
pub(crate) fn preview_edit(
    store: &Store,
    tag: &str,
    target: &str,
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Verdict, String> {
    let preview = firmware_import::preview_edit(store, tag, target, statement, catalogue)
        .map_err(|e| e.to_string())?;
    Ok(Verdict {
        unchanged: preview.unchanged,
        ..Verdict::of(&preview.preview)
    })
}
/// Worker side of Publish new revision: the new tag and the library's notes.
pub(crate) fn publish_edit(
    store: &Store,
    tag: &str,
    target: &str,
    statement: &Statement,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<(String, Vec<String>), String> {
    let revision = firmware_import::edit(store, tag, target, statement, catalogue)
        .map_err(|e| e.to_string())?;
    Ok((target.into(), revision.notes))
}

/// Edit metadata…: the shared editor over a stored package. Publishing makes
/// a new revision with the same ROM blobs; the edited tag never changes.
pub(crate) struct EditDialog {
    pub tag: String,
    pub name: String,
    pub target: Option<String>,
    pub draft: Option<Draft>,
    /// Opening or publishing failed, or the worker stopped.
    pub error: Option<String>,
    /// The worker stopped: nothing more can be read, checked or published.
    pub stopped: bool,
    pub publishing: bool,
    /// Run the source archive picker once the package is read.
    pub pick_source: bool,
}
pub(crate) enum EditAction {
    None,
    PickSource,
    Publish,
    Close,
}
impl EditDialog {
    pub fn new(tag: String, name: String, pick_source: bool) -> Self {
        Self {
            tag,
            name,
            target: None,
            draft: None,
            error: None,
            stopped: false,
            publishing: false,
            pick_source,
        }
    }
    pub fn opened(&mut self, result: Result<EditSource, String>) {
        match result {
            Ok(source) => {
                self.name = source.name;
                self.target = Some(source.target);
                self.draft = Some(source.draft);
            }
            Err(error) => self.error = Some(error),
        }
    }
    /// Waiting for the worker: reading the package, a preview or the publication.
    pub fn waiting(&self) -> bool {
        !self.stopped
            && ((self.draft.is_none() && self.error.is_none())
                || self.publishing
                || self.draft.as_ref().is_some_and(Draft::waiting))
    }
    /// The worker is gone: the dialog stops waiting and offers Close.
    pub fn stop(&mut self, error: String) {
        self.publishing = false;
        self.stopped = true;
        if let Some(draft) = &mut self.draft {
            draft.stopped();
        }
        self.error = Some(error);
    }
    pub fn published(&mut self, result: &Result<(String, Vec<String>), String>) {
        self.publishing = false;
        if let Err(error) = result {
            self.error = Some(error.clone());
        }
    }
    /// Why Publish new revision is disabled, or what it would change.
    pub fn footer(&self) -> (bool, String) {
        let Some(draft) = &self.draft else {
            return (false, "Reading the package…".into());
        };
        if self.publishing {
            return (false, "Publishing the new revision…".into());
        }
        if self.stopped {
            return (
                false,
                "Publish disabled — the image store worker stopped.".into(),
            );
        }
        match draft.problem() {
            Some(problem) => (false, format!("Publish disabled — {}", stop(&problem))),
            None if draft.accepted().is_some_and(|v| v.unchanged) => {
                (false, "No changes yet.".into())
            }
            None if draft.changes().is_empty() => (
                true,
                "No field changed; the new revision brings the package up to the current rules \
                 with a new config and README."
                    .into(),
            ),
            None => (true, format!("Changed: {}.", draft.changes().join(", "))),
        }
    }
    pub fn show(&mut self, ctx: &egui::Context, classic: bool) -> EditAction {
        let mut action = EditAction::None;
        let title = format!("Edit metadata — {}", self.name);
        egui::Modal::new(egui::Id::unique("firmware-edit-metadata")).show(ctx, |ui| {
            ui.set_max_width(960.0);
            if classic {
                crate::shell::gradient_title(ui, &title);
            } else {
                ui.heading(&title);
            }
            let next = self.target.clone().unwrap_or_else(|| "a new tag".into());
            ui.group(|ui| {
                // The firmware mock-up's note banner: ⓘ beside its text.
                ui.horizontal_top(|ui| {
                    crate::shell_icons::show(ui, crate::shell_icons::Icon::Info);
                    ui.vertical(|ui| {
                        ui.strong(
                            "Changes create a new package revision; ROM blobs are unchanged.",
                        );
                        ui.label(format!(
                            "{} itself never changes. Its ROM, document and source blobs are \
                             reused by digest; the new revision gets a new config and README.md \
                             and is published as {next}. Machines that recorded the old package \
                             digest keep it.",
                            self.tag
                        ));
                    });
                });
            });
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().warn_fg_color, gui_reason(error));
            }
            match &mut self.draft {
                None if self.error.is_none() => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the package…");
                    });
                }
                None => (),
                Some(draft) => {
                    ui.add_enabled_ui(!self.publishing, |ui| {
                        // Scrolled content takes the full height at once; offered the rest of
                        // last frame's dialog, it would grow the modal frame by frame.
                        let height = (ctx.content_rect().height() - 240.0).max(240.0);
                        egui::ScrollArea::vertical()
                            .id_salt("firmware-edit-editor")
                            .max_height(height)
                            .min_scrolled_height(height)
                            .show(ui, |ui| {
                                if draft.show(ui, "firmware-edit") == EditorAction::PickSource {
                                    action = EditAction::PickSource;
                                }
                            });
                    });
                }
            }
            let (enabled, why) = self.footer();
            ui.horizontal_wrapped(|ui| {
                if self.stopped || (self.draft.is_none() && self.error.is_some()) {
                    if ui.button("Close").clicked() {
                        action = EditAction::Close;
                    }
                    return;
                }
                ui.weak(why);
                if ui
                    .add_enabled(enabled, egui::Button::new("Publish new revision"))
                    .clicked()
                {
                    action = EditAction::Publish;
                }
                if ui
                    .add_enabled(!self.publishing, egui::Button::new("Cancel"))
                    .clicked()
                {
                    action = EditAction::Close;
                }
            });
            if self.waiting() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        });
        action
    }
}

/// Open README: the package's README layer, shown by the program itself.
pub(crate) struct ReadmeDialog {
    pub name: String,
    pub text: String,
}
impl ReadmeDialog {
    /// `true` once closed.
    pub fn show(&self, ctx: &egui::Context, classic: bool) -> bool {
        let mut close = false;
        let title = format!("README.md — {}", self.name);
        egui::Modal::new(egui::Id::unique("firmware-readme")).show(ctx, |ui| {
            ui.set_max_width(760.0);
            if classic {
                crate::shell::gradient_title(ui, &title);
            } else {
                ui.heading(&title);
            }
            ui.weak(
                "Shown by the program from the package's README.md layer, plain UTF-8 Markdown. \
                 No host application is needed, and any text tool can read the layer after \
                 export.",
            );
            // Scrolled content takes the full height at once; offered the rest of
            // last frame's dialog, it would grow the modal frame by frame.
            let height = (ctx.content_rect().height() - 200.0).max(200.0);
            egui::ScrollArea::vertical()
                .id_salt("firmware-readme-text")
                .max_height(height)
                .min_scrolled_height(height)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&self.text).monospace()).wrap());
                });
            if ui.button("Close").clicked() {
                close = true;
            }
        });
        close
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_view::firmware::tests::{
        chip_folder, click, frame, library, pinned_tar, poll_until, query, synthetic_catalogue,
        tar, texts, warm,
    };
    use crate::library_view::firmware::{
        cells, details, import_selected, scan_folder, CandidateState, Outcome, PaneAction,
        SourceTarget,
    };
    use crate::library_view::{snapshot, Filter, LibraryView};
    use assembly::{firmware_discovery::DiscoveryControl, test_support::TempDir};
    use std::fs;

    const OPEN: &str = "synthetic-open-bios";
    const OPEN_TAG: &str = "firmware/synthetic-open-bios:1";
    const COPYLEFT: &str = "synthetic-copyleft-bios";
    const VENDOR: &str = "synthetic-two-chip-bios";

    fn draft<'a>(view: &'a LibraryView, id: &str) -> &'a Draft {
        &view.firmware_import.as_ref().unwrap().drafts[id]
    }
    fn change(view: &mut LibraryView, id: &str, edit: impl FnOnce(&mut Fields)) {
        let draft = view
            .firmware_import
            .as_mut()
            .unwrap()
            .drafts
            .get_mut(id)
            .unwrap();
        edit(&mut draft.fields);
        draft.revision += 1;
    }
    /// Every set's statement has the library's verdict.
    fn settled(view: &LibraryView) -> bool {
        view.firmware_import
            .as_ref()
            .is_some_and(|d| d.report.is_some() && d.drafts.values().all(|d| d.current().is_some()))
    }
    fn edit_settled(view: &LibraryView) -> bool {
        view.firmware_edit
            .as_ref()
            .and_then(|d| d.draft.as_ref())
            .is_some_and(|d| d.current().is_some())
    }
    fn edit_draft(view: &mut LibraryView) -> &mut Draft {
        view.firmware_edit.as_mut().unwrap().draft.as_mut().unwrap()
    }
    fn import_and_wait(view: &mut LibraryView) -> Vec<Outcome> {
        let request = view.import_picks().expect("import allowed");
        view.send(request);
        poll_until(view, |v| {
            v.firmware_import
                .as_ref()
                .is_some_and(|d| d.outcomes.is_some())
        });
        view.firmware_import
            .as_ref()
            .unwrap()
            .outcomes
            .clone()
            .unwrap()
    }
    fn resolve(store: &Store, tag: &str) -> (Manifest, Config) {
        let image = store
            .resolve(tag, &Validator(&PcFirmwareRules(synthetic_catalogue())))
            .unwrap();
        (image.manifest, Config::decode(&image.config).unwrap())
    }
    fn layer<'a>(manifest: &'a Manifest, kind: &str) -> &'a Descriptor {
        manifest
            .layers
            .iter()
            .find(|d| d.decoded_media_type() == kind)
            .unwrap_or_else(|| panic!("no {kind} layer"))
    }
    fn readme_text(store: &Store, manifest: &Manifest) -> String {
        String::from_utf8(
            image_store::BlobSource::read_verified(
                store,
                layer(manifest, media_type::MARKDOWN),
                MAX_SMALL_BLOB,
            )
            .unwrap(),
        )
        .unwrap()
    }
    /// The library view in the Firmware collection with `tag`'s row selected.
    fn select(view: &mut LibraryView, tag: &str) {
        poll_until(view, |v| v.snapshot.storage.is_some());
        view.set_filter(Filter::Firmware, &mut None);
        view.send(super::super::Request::Query(view.query.clone()));
        poll_until(view, |v| {
            v.snapshot
                .firmware_rows
                .iter()
                .any(|r| r.image.reference == tag && r.image.summary.is_some())
        });
        let row = view
            .snapshot
            .firmware_rows
            .iter()
            .find(|r| r.image.reference == tag)
            .unwrap()
            .clone();
        view.show_package(row);
    }
    fn shown(output: &egui::FullOutput) -> Vec<String> {
        texts(output).into_iter().map(|(t, _)| t).collect()
    }
    /// The texts a set's editor draws on its own.
    fn editor_texts(view: &LibraryView, id: &str) -> Vec<String> {
        let mut draft = draft(view, id).clone();
        shown(&frame(
            &egui::Context::default(),
            egui::vec2(1600.0, 1600.0),
            0.0,
            vec![],
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| draft.show(ui, "test-editor"));
            },
        ))
    }

    #[test]
    fn redistribution_choices_follow_the_catalogue_and_the_library_verdicts() {
        let temp = TempDir::new("firmware-rights-review");
        let folder = chip_folder(temp.path());
        fs::write(folder.join("open.bin"), [0xf4; 32768]).unwrap();
        fs::write(folder.join("copyleft.bin"), [0xc5; 32768]).unwrap();
        let pinned = temp.path().join("pinned-src.tar");
        fs::write(&pinned, pinned_tar()).unwrap();
        let other = temp.path().join("other-src.tar");
        fs::write(&other, tar(7000, 0x33)).unwrap();
        let mut view = LibraryView::synthetic(temp.path().join("store"));
        poll_until(&mut view, |v| v.snapshot.storage.is_some());
        view.begin_firmware_import(folder.clone(), None);
        poll_until(&mut view, settled);

        // A ROM the catalogue lists as open arrives open with its licence.
        let open = draft(&view, OPEN);
        assert_eq!(
            open.facts.catalogue,
            Catalogue::Open {
                id: OPEN.into(),
                license: "MIT".into()
            }
        );
        assert_eq!(open.fields.rights, Rights::Open);
        let verdict = open
            .accepted()
            .expect("a permissive licence needs no source");
        assert_eq!(verdict.rom_license.as_deref(), Some("MIT"));
        assert!(verdict.config_open);
        assert_eq!(
            verdict.layers.last().map(|l| l.license.as_deref()),
            Some(Some("CC0-1.0"))
        );
        // Proprietary sets stay user-supplied.
        let vendor = draft(&view, VENDOR);
        assert!(matches!(
            vendor.facts.catalogue,
            Catalogue::Proprietary { .. }
        ));
        assert_eq!(vendor.fields.rights, Rights::UserSupplied);
        assert!(vendor
            .accepted()
            .is_some_and(|v| v.rom_license.is_none() && !v.config_open));
        // A copyleft set is refused until its source archive is chosen, in
        // the dialog's own words.
        let problem = draft(&view, COPYLEFT).problem().unwrap();
        assert!(
            problem.contains("GPL-3.0-or-later is copyleft")
                && problem.contains("synthetic/copyleft-fetch.sh builds with Source archive…"),
            "{problem}"
        );
        assert!(!problem.contains("--"), "{problem}");
        let dialog = view.firmware_import.as_ref().unwrap();
        assert!(!dialog.can_import());
        assert!(dialog
            .blockers()
            .iter()
            .any(|b| b.starts_with("synthetic-copyleft-bios: GPL-3.0-or-later is copyleft")));

        // The dialog draws the choice, the reason and the set's details.
        let ctx = egui::Context::default();
        let mut drawn = Vec::new();
        for step in 0..6 {
            drawn = shown(&frame(
                &ctx,
                egui::vec2(1600.0, 4000.0),
                f64::from(step) * 0.05,
                vec![],
                |ui| {
                    view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
                },
            ));
        }
        for expected in [
            "Redistribution",
            "Open · MIT",
            "Open · GPL-3.0-or-later",
            "User-supplied",
            "Catalogue: proprietary (synthetic-two-chip-bios)",
        ] {
            assert!(
                drawn.iter().any(|t| t == expected),
                "missing {expected:?} in {drawn:?}"
            );
        }
        assert!(drawn.iter().any(|t| t
            .starts_with("Import disabled — synthetic-copyleft-bios: GPL-3.0")
            && t.ends_with("with Source archive…")));
        let mut copyleft = draft(&view, COPYLEFT).clone();
        let editor = shown(&frame(
            &ctx,
            egui::vec2(1600.0, 1600.0),
            0.0,
            vec![],
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| copyleft.show(ui, "test-editor"));
            },
        ));
        for expected in [
            "README.md — generated from these fields",
            "Source archive…",
            "Licence from the catalogue: GPL-3.0-or-later.",
            "Required: GPL-3.0-or-later is a copyleft licence. Choose the archive, or keep the \
             set user-supplied.",
            "From the catalogue entry synthetic-copyleft-bios. You can narrow boards and classes \
             but not widen them; required devices are facts of the ROM.",
            "Open (redistributable)",
            "Board",
            SOURCE_STATEMENT,
        ] {
            assert!(
                editor.iter().any(|t| t.starts_with(expected)),
                "missing {expected:?} in {editor:?}"
            );
        }

        // Open for proprietary bytes is refused by the library, with its reason.
        change(&mut view, VENDOR, |f| f.rights = Rights::Open);
        poll_until(&mut view, settled);
        let refused = draft(&view, VENDOR).problem().unwrap();
        assert!(
            refused.contains("lists this ROM as proprietary")
                && refused.contains("cannot be marked open"),
            "{refused}"
        );
        change(&mut view, VENDOR, |f| f.rights = Rights::UserSupplied);

        // The archive the project pins is open under the catalogue licence.
        view.firmware_source_picked(
            SourceTarget::Import(COPYLEFT.into(), folder.clone()),
            Some(pinned),
        );
        poll_until(&mut view, settled);
        let verdict = draft(&view, COPYLEFT)
            .accepted()
            .cloned()
            .expect("the pinned archive");
        assert_eq!(verdict.rom_license.as_deref(), Some("GPL-3.0-or-later"));
        assert_eq!(
            verdict.source,
            Some(SourceFile {
                filename: "pinned-src.tar".into(),
                format: "tar".into(),
                size: 6000,
                license: Some("GPL-3.0-or-later".into()),
            })
        );
        // Any other archive needs its own licence statement.
        view.firmware_source_picked(
            SourceTarget::Import(COPYLEFT.into(), folder.clone()),
            Some(other),
        );
        poll_until(&mut view, settled);
        let problem = draft(&view, COPYLEFT).problem().unwrap();
        assert!(
            problem
                .contains("is not the one synthetic/copyleft-fetch.sh builds; state its licence"),
            "{problem}"
        );
        assert!(!problem.contains("--"), "{problem}");
        // Only the library's accepted verdict confirms the source.
        const CONFIRMED: &str = "✓ Corresponding source attached, as GPL-3.0-or-later requires.";
        assert!(!editor_texts(&view, COPYLEFT).iter().any(|t| t == CONFIRMED));
        change(&mut view, COPYLEFT, |f| {
            f.source_license = LicenseChoice::of(Some("GPL-3.0-or-later"))
        });
        poll_until(&mut view, settled);
        assert!(editor_texts(&view, COPYLEFT).iter().any(|t| t == CONFIRMED));
        assert!(view.firmware_import.as_ref().unwrap().can_import());
        assert_eq!(
            import_and_wait(&mut view),
            [
                Outcome::Imported("firmware/synthetic-copyleft-bios:1".into()),
                Outcome::Imported(OPEN_TAG.into()),
                Outcome::Imported("firmware/synthetic-two-chip-bios:1".into()),
            ]
        );
        // Scanned again, every set is already imported, whatever statement
        // made it: the tags hold the same ROM bytes.
        view.firmware_import = None;
        view.begin_firmware_import(folder.clone(), None);
        poll_until(&mut view, |v| {
            v.firmware_import
                .as_ref()
                .is_some_and(|d| d.report.is_some())
        });
        let dialog = view.firmware_import.as_mut().unwrap();
        let states: Vec<_> = dialog
            .report
            .as_ref()
            .unwrap()
            .candidates
            .iter()
            .map(|c| (c.catalogue_id.clone(), c.state.clone()))
            .collect();
        assert_eq!(
            states,
            [VENDOR, OPEN, COPYLEFT].map(|id| (id.to_owned(), CandidateState::AlreadyImported))
        );
        assert!(dialog.picks.is_empty() && dialog.drafts.is_empty());
        dialog.current = Some(COPYLEFT.into());
        let drawn = shown(&frame(
            &ctx,
            egui::vec2(1600.0, 4000.0),
            1.0,
            vec![],
            |ui| {
                view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
            },
        ));
        assert!(
            drawn.iter().any(|t| t.starts_with(
                "Already in the store as firmware/synthetic-copyleft-bios:1. Nothing is imported \
                 again."
            )),
            "{drawn:?}"
        );
        view.shutdown();
        let store = Store::open(temp.path().join("store")).unwrap();
        let (manifest, config) = resolve(&store, "firmware/synthetic-copyleft-bios:1");
        let rom = layer(&manifest, media_type::FIRMWARE);
        assert_eq!(class(rom).as_deref(), Some("GPL-3.0-or-later"));
        assert!(!license::source_missing(rom, &manifest.layers));
        assert_eq!(config.sources[0].filename, "other-src.tar");
        let (manifest, _) = resolve(&store, "firmware/synthetic-two-chip-bios:1");
        assert!(class(layer(&manifest, media_type::FIRMWARE)).is_none());
    }

    #[test]
    fn spdx_choices_are_validated_by_the_parser_and_reasons_name_the_dialog_controls() {
        for (text, valid) in [
            ("MIT OR Apache-2.0", true),
            ("GPL-2.0-only WITH Classpath-exception-2.0", true),
            ("LicenseRef-vendor-1", true),
            ("mit", false),
            ("MIT OR Foo-1.0", false),
            ("GPL-2.0", false),
            ("MIT AND", false),
        ] {
            let choice = LicenseChoice {
                text: text.into(),
                free: true,
            };
            assert_eq!(choice.problem().is_none(), valid, "{text}");
        }
        let problem = LicenseChoice::of(Some("MIT OR Foo-1.0")).problem().unwrap();
        assert!(problem.contains("Foo-1.0"), "{problem}");
        assert!(!LicenseChoice::of(Some("MIT")).free);
        assert!(LicenseChoice::of(Some("MIT AND Zlib")).free);
        assert_eq!(LicenseChoice::default().problem(), None);

        let temp = TempDir::new("firmware-spdx");
        let rom = temp.path().join("rom.bin");
        fs::write(&rom, [0x66; 8192]).unwrap();
        let candidate = firmware_import::unlisted(&rom, "spdx-rom", "option-rom", &[]).unwrap();
        let facts = Facts {
            catalogue: Catalogue::of(&candidate.config, &[]).unwrap(),
            catalogue_fit: None,
            stored_source: None,
            single_rom: true,
        };
        assert_eq!(facts.catalogue, Catalogue::Unlisted);
        let mut draft = Draft::new(facts.clone(), Fields::prefill(&candidate.config, &facts));
        assert_eq!(draft.fields.rights, Rights::UserSupplied);
        let statement = draft.statement();
        assert!(statement.user_supplied && !statement.open && statement.license.is_none());
        let reason = |draft: &Draft| {
            firmware_import::preview_manifest_with(&candidate, &[], &draft.statement(), &[])
                .map(|_| ())
                .map_err(|e| gui_reason(&e.to_string()))
        };
        draft.fields.rights = Rights::Open;
        let r = reason(&draft).unwrap_err();
        assert!(r.starts_with("Open needs an SPDX licence"), "{r}");
        draft.fields.license = LicenseChoice {
            text: " MIT OR Apache-2.0 ".into(),
            free: true,
        };
        assert_eq!(
            draft.statement().license.as_deref(),
            Some("MIT OR Apache-2.0")
        );
        assert!(reason(&draft).is_ok());
        draft.fields.license = LicenseChoice::of(Some("GPL-2.0-only"));
        let r = reason(&draft).unwrap_err();
        assert!(
            r.contains("add the source archive with Source archive… and state its licence"),
            "{r}"
        );
        let source = temp.path().join("src.tar");
        fs::write(&source, tar(5000, 1)).unwrap();
        draft.fields.source = Some(source);
        let r = reason(&draft).unwrap_err();
        assert!(
            r.contains("state the source archive's licence") && !r.contains("--"),
            "{r}"
        );
        draft.fields.source_license = LicenseChoice::of(Some("GPL-2.0-only"));
        assert!(reason(&draft).is_ok());
        // A user-supplied choice sends no licence statement at all.
        draft.fields.rights = Rights::UserSupplied;
        let statement = draft.statement();
        assert!(statement.license.is_none() && statement.source_license.is_none());
    }

    #[test]
    fn the_details_editor_writes_its_fields_into_the_imported_package() {
        let temp = TempDir::new("firmware-details-round-trip");
        let store = Store::open(temp.path().join("store")).unwrap();
        let folder = chip_folder(temp.path());
        let cat = synthetic_catalogue();
        let control = DiscoveryControl::default();
        let (report, candidates) = scan_folder(&store, &folder, None, &control, cat).unwrap();
        let view = &report.candidates[0];
        assert_eq!(
            (view.catalogue_id.as_str(), &view.state),
            (VENDOR, &CandidateState::New)
        );
        let mut draft = Draft::new(view.facts.clone(), view.fields.clone());
        assert_eq!(
            draft.statement(),
            Statement {
                user_supplied: true,
                ..Statement::default()
            },
            "the prefill alone states nothing"
        );
        let f = &mut draft.fields;
        f.title = "  Example two-chip BIOS ".into();
        f.description = "Two interleaved chips.".into();
        f.notes = "Even chip first.\nOdd chip second.".into();
        assert_eq!(add_label(&mut f.labels, "original"), None);
        assert_eq!(add_label(&mut f.labels, " at "), None);
        assert!(add_label(&mut f.labels, "Not Valid")
            .unwrap()
            .contains("is not valid"));
        assert!(add_label(&mut f.labels, "at")
            .unwrap()
            .contains("already listed"));
        f.classes.clear();
        f.provenance.kind = Some(ProvenanceKind::Dumped);
        f.provenance.from = "A synthetic board, chips U27 and U47".into();
        f.provenance.date = "2026-10-04".into();
        assert!(draft.narrowed());
        assert_eq!(
            draft.changes(),
            [
                "title",
                "description",
                "notes",
                "labels",
                "machine classes",
                "provenance"
            ]
        );
        let statement = draft.statement();
        assert!(statement.boards.is_none() && statement.requires.is_none());
        assert!(statement.vendor.is_none() && statement.readme == ReadmeChoice::Keep);
        // Widening a catalogue fit is the library's refusal, not the editor's.
        let mut widened = draft.clone();
        widened.fields.boards.push("generic-386-isa".into());
        assert!(firmware_import::preview_manifest_with(
            &candidates[0],
            &[],
            &widened.statement(),
            cat
        )
        .is_err());
        let outcomes =
            import_selected(&store, &candidates, &[(VENDOR.into(), statement)], cat).unwrap();
        assert_eq!(
            outcomes,
            [Outcome::Imported(
                "firmware/synthetic-two-chip-bios:1".into()
            )]
        );
        let (manifest, config) = resolve(&store, "firmware/synthetic-two-chip-bios:1");
        assert_eq!(config.name, "Example two-chip BIOS");
        assert_eq!(
            config.description.as_deref(),
            Some("Two interleaved chips.")
        );
        assert_eq!(
            config.notes.as_deref(),
            Some("Even chip first.\nOdd chip second.")
        );
        assert_eq!(config.labels, ["at", "original"]);
        let fit = pc_image::firmware::Fit::decode(&config.fits).unwrap();
        assert_eq!(fit.boards, ["ibm-5170"]);
        assert!(fit.machine_classes.is_empty());
        let p = config.provenance.clone().unwrap();
        assert_eq!(
            (p.kind, p.from.as_str(), p.date.as_deref()),
            (
                Some(ProvenanceKind::Dumped),
                "A synthetic board, chips U27 and U47",
                Some("2026-10-04")
            )
        );
        let readme = readme_text(&store, &manifest);
        assert!(readme.starts_with("# Example two-chip BIOS\n\nTwo interleaved chips.\n"));
        // The stored package reads back as Edit metadata…'s starting point.
        let source = open_edit(&store, "firmware/synthetic-two-chip-bios:1", cat).unwrap();
        assert_eq!(source.target, "firmware/synthetic-two-chip-bios:2");
        let stored = &source.draft.fields;
        assert_eq!(stored.title, "Example two-chip BIOS");
        assert_eq!(stored.labels, ["at", "original"]);
        assert!(stored.classes.is_empty());
        assert_eq!(stored.provenance.date, "2026-10-04");
        assert_eq!(stored.rights, Rights::UserSupplied);
        assert!(stored.readme.is_none(), "the generator wrote it");
        assert!(source.draft.narrowed());
    }

    #[test]
    fn the_readme_preview_is_the_library_text_and_a_hand_edit_is_stored_as_typed() {
        for hand in [false, true] {
            let temp = TempDir::new("firmware-readme-preview");
            let folder = temp.path().join("roms");
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("open.bin"), [0xf4; 32768]).unwrap();
            let mut view = LibraryView::synthetic(temp.path().join("store"));
            poll_until(&mut view, |v| v.snapshot.storage.is_some());
            view.begin_firmware_import(folder, None);
            poll_until(&mut view, settled);
            let generated = draft(&view, OPEN).accepted().unwrap().readme.clone();
            assert!(
                generated.starts_with("# synthetic-open-bios\n"),
                "{generated}"
            );
            assert!(generated.contains("- ROM images: MIT, open (redistributable).\n"));
            if hand {
                change(&mut view, OPEN, |f| {
                    f.readme = Some("# Mine\r\nWritten by hand.\r\n".into())
                });
                poll_until(&mut view, settled);
            }
            let expected = draft(&view, OPEN).accepted().unwrap().readme.clone();
            if hand {
                assert_eq!(expected, "# Mine\nWritten by hand.\n");
                assert_eq!(draft(&view, OPEN).changes(), ["README"]);
            }
            assert_eq!(
                import_and_wait(&mut view),
                [Outcome::Imported(OPEN_TAG.into())]
            );
            view.shutdown();
            let store = Store::open(temp.path().join("store")).unwrap();
            let (manifest, config) = resolve(&store, OPEN_TAG);
            let stored = readme_text(&store, &manifest);
            assert_eq!(stored, expected);
            assert_eq!(
                firmware_import::generated_readme(&config, &manifest) == stored,
                !hand
            );
            let source = open_edit(&store, OPEN_TAG, synthetic_catalogue()).unwrap();
            assert_eq!(
                source.draft.fields.readme.is_some(),
                hand,
                "Edit metadata… starts from a hand-written README"
            );
            // Another field changes; a hand-written README stays, a generated
            // one follows the fields.
            let mut draft = source.draft;
            assert_eq!(
                draft.statement().readme,
                if hand {
                    ReadmeChoice::Text(expected.clone())
                } else {
                    ReadmeChoice::Keep
                }
            );
            draft.fields.notes = "A quirk.".into();
            firmware_import::edit(
                &store,
                OPEN_TAG,
                &source.target,
                &draft.statement(),
                synthetic_catalogue(),
            )
            .unwrap();
            let (manifest, _) = resolve(&store, &source.target);
            let edited = readme_text(&store, &manifest);
            assert_eq!(edited == expected, hand, "{edited}");
            assert_eq!(edited.contains("A quirk."), !hand);
            // Regenerate from fields asks the library for generated text.
            draft.fields.readme = None;
            assert_eq!(
                draft.statement().readme,
                if hand {
                    ReadmeChoice::Generate
                } else {
                    ReadmeChoice::Keep
                }
            );
        }
    }

    #[test]
    fn edit_metadata_publishes_a_new_revision_with_the_same_rom_blobs() {
        let (f, _) = library();
        let first = f.store.tag_descriptor(OPEN_TAG).unwrap();
        let mut view = LibraryView::synthetic(f.store.root().to_owned());
        select(&mut view, OPEN_TAG);
        poll_until(&mut view, |v| v.package_details().is_some());
        let details = view.package_details().unwrap().clone().unwrap();
        // The synthetic machine image holds the same BIOS bytes marked
        // user-supplied, and the most restrictive local mark wins.
        assert_eq!(details.push, Push::UserSupplied);
        assert!(details.config_open);
        assert!(details
            .readme
            .as_deref()
            .unwrap()
            .starts_with("# synthetic-open-bios\n"));
        // Open README shows the stored text in the program.
        let row = view.firmware_selected.clone().unwrap();
        view.pane_action(&row, PaneAction::OpenReadme);
        assert_eq!(
            view.firmware_readme.as_ref().map(|d| d.text.as_str()),
            details.readme.as_deref()
        );
        crate::library_test_support::settled_modal(
            egui::Id::unique("firmware-readme"),
            egui::vec2(1040.0, 700.0),
            |ctx| {
                view.modals(ctx, None, Vec::new(), false, false);
            },
        );
        view.firmware_readme = None;

        assert!(view.can_edit_firmware());
        view.pane_action(&row, PaneAction::EditMetadata);
        poll_until(&mut view, edit_settled);
        crate::library_test_support::settled_modal(
            egui::Id::unique("firmware-edit-metadata"),
            egui::vec2(1040.0, 700.0),
            |ctx| {
                view.modals(ctx, None, Vec::new(), false, false);
            },
        );
        let dialog = view.firmware_edit.as_ref().unwrap();
        assert_eq!(
            dialog.target.as_deref(),
            Some("firmware/synthetic-open-bios:2")
        );
        assert_eq!(dialog.footer(), (false, "No changes yet.".into()));
        let draft = dialog.draft.as_ref().unwrap();
        assert_eq!(draft.fields.rights, Rights::Open);
        assert!(draft.fields.readme.is_none());
        edit_draft(&mut view).fields.description = "An edited description.".into();
        edit_draft(&mut view).revision += 1;
        poll_until(&mut view, edit_settled);
        assert_eq!(
            view.firmware_edit.as_ref().unwrap().footer(),
            (true, "Changed: description.".into())
        );
        let request = view.publish_firmware_edit().expect("publish allowed");
        view.send(request);
        poll_until(&mut view, |v| v.firmware_edit.is_none());
        assert_eq!(
            view.status.as_deref(),
            Some("Published firmware/synthetic-open-bios:2; firmware/synthetic-open-bios:1 is unchanged")
        );
        poll_until(&mut view, |v| {
            v.firmware_selected
                .as_ref()
                .is_some_and(|r| r.image.reference == "firmware/synthetic-open-bios:2")
        });
        view.shutdown();
        assert_eq!(
            f.store.tag_descriptor(OPEN_TAG).unwrap().digest,
            first.digest,
            "the edited tag never changes"
        );
        let (old, _) = resolve(&f.store, OPEN_TAG);
        let (new, config) = resolve(&f.store, "firmware/synthetic-open-bios:2");
        assert_eq!(
            layer(&old, media_type::FIRMWARE).digest,
            layer(&new, media_type::FIRMWARE).digest,
            "the same stored ROM blob"
        );
        assert_eq!(
            config.description.as_deref(),
            Some("An edited description.")
        );
        assert_eq!(
            class(layer(&new, media_type::FIRMWARE)).as_deref(),
            Some("MIT")
        );
        // An older revision whose successor exists is not edited again.
        let err = open_edit(&f.store, OPEN_TAG, synthetic_catalogue()).unwrap_err();
        assert_eq!(
            err,
            "firmware/synthetic-open-bios:2 already exists; edit the newest revision of the package"
        );
    }

    #[test]
    fn every_refusal_the_editor_can_meet_names_the_dialog_controls() {
        let temp = TempDir::new("firmware-reason-words");
        let cat = synthetic_catalogue();
        let rom = temp.path().join("rom.bin");
        fs::write(&rom, [0x66; 8192]).unwrap();
        let unlisted = firmware_import::unlisted(&rom, "words-rom", "option-rom", cat).unwrap();
        let folder = temp.path().join("roms");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("copyleft.bin"), [0xc5; 32768]).unwrap();
        let copyleft = firmware_import::scan(&[folder], cat)
            .unwrap()
            .candidates
            .remove(0);
        let archive = temp.path().join("other-src.tar");
        fs::write(&archive, tar(7000, 0x33)).unwrap();
        let open = |license: Option<&str>| Statement {
            open: true,
            license: license.map(Into::into),
            ..Statement::default()
        };
        let sourced = |statement: Statement| Statement {
            source: Some(archive.clone()),
            ..statement
        };
        let gpl = Some("GPL-2.0-only");
        for (candidate, statement, words) in [
            (&unlisted, open(None), "Open needs an SPDX licence"),
            (
                &unlisted,
                Statement {
                    license: Some("MIT".into()),
                    ..Statement::default()
                },
                "choose Open",
            ),
            (
                &unlisted,
                open(gpl),
                "with Source archive… and state its licence",
            ),
            (
                &unlisted,
                sourced(open(gpl)),
                "state the source archive's licence",
            ),
            (&copyleft, open(None), "builds with Source archive…"),
            (&copyleft, sourced(open(None)), "builds; state its licence"),
            (
                &unlisted,
                sourced(Statement {
                    source_license: Some("MIT".into()),
                    ..Statement::default()
                }),
                "choose Open",
            ),
            (
                &unlisted,
                Statement {
                    source_license: Some("MIT".into()),
                    ..open(Some("MIT"))
                },
                "(Source archive…)",
            ),
        ] {
            let raw = firmware_import::preview_manifest_with(candidate, &[], &statement, cat)
                .unwrap_err()
                .to_string();
            assert!(raw.contains("--"), "{raw}");
            let shown = gui_reason(&raw);
            assert!(!shown.contains("--"), "{raw} -> {shown}");
            assert!(shown.contains(words), "{words:?} not in {shown}");
        }
    }

    #[test]
    fn clearing_the_shown_provenance_clears_it_and_user_supplied_drops_the_archive() {
        let provenance = ProvenanceFields {
            kind: None,
            from: " ".into(),
            date: String::new(),
            revision: Some("r9".into()),
            url: Some("https://example.org/rom".into()),
        };
        let stated = provenance.statement();
        assert!(stated.revision.is_none() && stated.url.is_none());
        let kept = ProvenanceFields {
            from: "A board".into(),
            ..provenance
        }
        .statement();
        assert_eq!(kept.revision.as_deref(), Some("r9"));
        let temp = TempDir::new("firmware-rights-switch");
        let rom = temp.path().join("rom.bin");
        fs::write(&rom, [0x66; 8192]).unwrap();
        let candidate = firmware_import::unlisted(&rom, "switch-rom", "option-rom", &[]).unwrap();
        let mut fields = Fields::of(&candidate.config, Some("GPL-2.0-only"));
        fields.source = Some(temp.path().join("src.tar"));
        fields.source_license = LicenseChoice::of(Some("GPL-2.0-only"));
        fields.set_rights(Rights::UserSupplied);
        assert!(fields.source.is_none() && fields.source_license == LicenseChoice::default());
    }

    /// A package as an earlier version could write it: the ROM open under a
    /// copyleft licence with no source archive layer.
    fn crafted_copyleft_package(store: &Store, root: &std::path::Path) -> &'static str {
        let rom = root.join("legacy.bin");
        fs::write(&rom, [0x67; 8192]).unwrap();
        let candidate = firmware_import::unlisted(&rom, "legacy-rom", "option-rom", &[]).unwrap();
        let open = Statement {
            open: true,
            license: Some("MIT".into()),
            ..Statement::default()
        };
        firmware_import::publish_with(store, &candidate, &[], &open, &[]).unwrap();
        let (mut manifest, _) = resolve(store, "firmware/legacy-rom:1");
        let gpl = Expression::parse("GPL-2.0-only").unwrap();
        for d in manifest
            .layers
            .iter_mut()
            .filter(|d| d.decoded_media_type() == media_type::FIRMWARE)
        {
            license::mark(d, Some(&gpl));
        }
        let d = store.import_manifest(&manifest).unwrap();
        let tag = "firmware/crafted-rom:1";
        store
            .update_tag(tag, &d, None, &Validator(&PcFirmwareRules(&[])))
            .unwrap();
        tag
    }

    #[test]
    fn a_copyleft_open_rom_without_its_source_shows_the_push_badge_notice_and_needs_the_source() {
        let temp = TempDir::new("firmware-push-badge");
        let store = Store::open(temp.path().join("store")).unwrap();
        let tag = crafted_copyleft_package(&store, temp.path());
        let cache = warm(&store);
        let result = snapshot(&cache, &query(Filter::Firmware, ""));
        let row = |tag: &str| {
            result
                .firmware_rows
                .iter()
                .find(|r| r.image.reference == tag)
                .unwrap()
                .clone()
        };
        let crafted = row(tag);
        assert!(crafted.source_missing);
        assert!(!row("firmware/legacy-rom:1").source_missing);
        assert_eq!(
            cells(&crafted)[7],
            "Open · GPL-2.0-only ⚠ No source · private only"
        );
        let read = read_package(&store, tag, &crafted.image.descriptor.digest, &[]).unwrap();
        assert_eq!(read.push, Push::CopyleftWithoutSource);
        let ctx = egui::Context::default();
        let mut action = PaneAction::None;
        let drawn = shown(&frame(
            &ctx,
            egui::vec2(1800.0, 1400.0),
            0.0,
            vec![],
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    action = details(ui, &crafted, Some(&Ok(read.clone())), true);
                });
            },
        ));
        for expected in [
            super::super::firmware::guard_notice("GPL-2.0-only"),
            "Add source archive…".into(),
            "Private registries only: copyleft ROM without its source layer".into(),
            "No source archive".into(),
            "Required to publish GPL-2.0-only firmware outside private registries.".into(),
            "Open · GPL-2.0-only ⚠ No source · private only".into(),
            "Edit metadata…".into(),
        ] {
            assert!(
                drawn.contains(&expected),
                "missing {expected:?} in {drawn:?}"
            );
        }
        assert_eq!(action, PaneAction::None);

        // Add source archive… is Edit metadata… with the source picker; the
        // new revision must meet the current rules.
        drop(store);
        let mut view = LibraryView::synthetic(temp.path().join("store"));
        select(&mut view, tag);
        let selected = view.firmware_selected.clone().unwrap();
        view.pane_action(&selected, PaneAction::AddSource);
        poll_until(&mut view, edit_settled);
        assert_eq!(view.firmware_source_pick.take(), Some(SourceTarget::Edit));
        let problem = edit_draft(&mut view).problem().unwrap();
        assert!(
            problem.contains("GPL-2.0-only is copyleft")
                && problem.contains("with Source archive… and state its licence"),
            "{problem}"
        );
        let source = temp.path().join("legacy-src.zip");
        let mut zip = b"PK\x03\x04".to_vec();
        zip.extend([0x21; 300]);
        fs::write(&source, zip).unwrap();
        view.firmware_source_picked(SourceTarget::Edit, Some(source));
        poll_until(&mut view, edit_settled);
        let problem = edit_draft(&mut view).problem().unwrap();
        assert!(
            problem.contains("state the source archive's licence"),
            "{problem}"
        );
        let draft = edit_draft(&mut view);
        draft.fields.source_license = LicenseChoice::of(Some("GPL-2.0-only"));
        draft.revision += 1;
        poll_until(&mut view, edit_settled);
        let verdict = edit_draft(&mut view).accepted().cloned().unwrap();
        assert_eq!(
            verdict
                .source
                .as_ref()
                .map(|s| (s.format.as_str(), s.license.as_deref())),
            Some(("zip", Some("GPL-2.0-only")))
        );
        let request = view.publish_firmware_edit().expect("publish allowed");
        view.send(request);
        poll_until(&mut view, |v| v.firmware_edit.is_none());
        view.shutdown();
        let store = Store::open(temp.path().join("store")).unwrap();
        let (manifest, _) = resolve(&store, "firmware/crafted-rom:2");
        assert!(!license::source_missing(
            layer(&manifest, media_type::FIRMWARE),
            &manifest.layers
        ));
        let cache = warm(&store);
        let result = snapshot(&cache, &query(Filter::Firmware, "crafted"));
        let revised = result
            .firmware_rows
            .iter()
            .find(|r| r.image.reference == "firmware/crafted-rom:2")
            .unwrap();
        assert!(!revised.source_missing);
        assert_eq!(
            cells(revised)[7],
            "Open · GPL-2.0-only · with source archive"
        );
        let read = read_package(
            &store,
            "firmware/crafted-rom:2",
            &revised.image.descriptor.digest,
            &[],
        )
        .unwrap();
        assert_eq!(
            push_text(&read),
            "unemu/ and private registries · source layer present"
        );
    }

    /// Draw the library's modals until Close stops moving, then click it.
    fn close_by_click(view: &mut LibraryView) {
        let ctx = egui::Context::default();
        let mut drawn: Vec<(String, egui::Rect)> = Vec::new();
        for step in 0..20 {
            let previous = drawn.iter().find(|(t, _)| t == "Close").map(|(_, r)| *r);
            // The size `click` draws at, so the button stays where it was found.
            drawn = texts(&frame(
                &ctx,
                egui::vec2(1200.0, 900.0),
                f64::from(step) * 0.05,
                vec![],
                |ui| {
                    view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
                },
            ));
            let current = drawn.iter().find(|(t, _)| t == "Close").map(|(_, r)| *r);
            if current.is_some() && current == previous {
                break;
            }
        }
        assert!(!drawn.iter().any(|(t, _)| t == "Publish new revision"));
        let (_, close) = drawn
            .into_iter()
            .find(|(t, _)| t == "Close")
            .expect("Close offered");
        click(&ctx, close.center(), |ui| {
            view.modals(&ui.ctx().clone(), None, Vec::new(), false, false);
        });
    }

    #[test]
    fn edit_metadata_fails_cleanly_when_the_worker_dies() {
        // The worker dies reading the package, in the first preview, then in
        // the publication after two accepted previews.
        for survive in [1, 2, 4] {
            let (f, _) = library();
            let root = f.store.root().to_owned();
            let mut view = LibraryView::new(Some(root.clone()));
            view.worker = Some(crate::library_worker::Worker::firmware_crash(root, survive));
            select(&mut view, OPEN_TAG);
            poll_until(&mut view, |v| v.package_details().is_some());
            assert!(view.package_details().unwrap().is_ok());
            view.begin_firmware_edit(false);
            if survive == 4 {
                poll_until(&mut view, edit_settled);
                edit_draft(&mut view).fields.notes = "A quirk.".into();
                edit_draft(&mut view).revision += 1;
                poll_until(&mut view, edit_settled);
                let request = view.publish_firmware_edit().expect("publish allowed");
                view.send(request);
            }
            poll_until(&mut view, |v| v.worker_dead);
            let dialog = view.firmware_edit.as_ref().expect("the dialog stays open");
            assert!(dialog.stopped && !dialog.waiting() && !dialog.publishing);
            assert!(
                dialog.error.as_deref().unwrap().contains("worker stopped"),
                "{:?}",
                dialog.error
            );
            assert!(!view.busy && !view.can_edit_firmware());
            close_by_click(&mut view);
            assert!(view.firmware_edit.is_none(), "survive {survive}");
            assert!(
                f.store
                    .tag_descriptor("firmware/synthetic-open-bios:2")
                    .is_err(),
                "nothing was published"
            );
        }
    }
}
