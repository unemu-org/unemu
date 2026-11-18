// SPDX-License-Identifier: AGPL-3.0-or-later
//! Descriptive metadata and licence statements of a firmware package, shared
//! by the Import ROM folder… review and Edit metadata…; machine-images §5.3.
//! This is the model; the desktop draws the editor.
//!
//! The editor holds the user's statement only. Every verdict it shows (the
//! licence of each layer, whether the README and the config are open, the
//! README text, and why an import or a revision is refused) is the library's
//! own preview of that statement, which the library worker computes from the
//! ROM and archive bytes. The program never marks a ROM open on its own: open
//! comes from the firmware catalogue or from a licence the user names, and
//! the library refuses open for bytes the catalogue lists as proprietary.

use crate::firmware::role_label;
use image_store::{
    firmware::{Config, Provenance, ProvenanceKind, Validator},
    license::{self, Expression},
    media_type, Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB,
};
use library_model::listing::short;
use pc_image::{
    firmware::PcFirmwareRules,
    firmware_import::{self, Preview, ReadmeChoice, Statement},
};
use std::path::PathBuf;

/// The user's redistribution choice for the ROM images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rights {
    UserSupplied,
    Open,
}

/// What the firmware catalogue says about a package's ROMs, as the library
/// reads it: by bytes first, then by the listed ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Catalogue {
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
    pub fn listed(&self) -> bool {
        !matches!(self, Self::Unlisted)
    }
}

/// An SPDX licence the user names: an identifier from the parser's list, or
/// a free expression, which the parser validates as it is typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LicenseChoice {
    pub text: String,
    /// The free field is shown, for an expression the list does not hold.
    pub free: bool,
}

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
    pub fn expression(&self) -> Option<Expression> {
        self.stated().and_then(|text| Expression::parse(&text).ok())
    }
}

/// How the bytes were obtained. Revision and address come from the command
/// line and are kept as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProvenanceFields {
    pub kind: Option<ProvenanceKind>,
    pub from: String,
    pub date: String,
    pub revision: Option<String>,
    pub url: Option<String>,
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
    pub fn statement(&self) -> Provenance {
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
pub fn provenance_label(kind: Option<ProvenanceKind>) -> &'static str {
    match kind {
        Some(ProvenanceKind::Dumped) => "Dumped from a board",
        Some(ProvenanceKind::Downloaded) => "Downloaded from",
        Some(ProvenanceKind::Built) => "Built from a revision",
        None => "Not stated",
    }
}
/// The pane's provenance lines: source, details and date.
pub fn provenance_lines(provenance: Option<&Provenance>) -> [String; 3] {
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
pub struct SourceFile {
    pub filename: String,
    pub format: String,
    pub size: u64,
    pub license: Option<String>,
}

/// Facts about the package that the editor does not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
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
pub struct Fields {
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
pub struct LayerRow {
    pub name: String,
    pub kind: String,
    pub license: Option<String>,
    /// The package config, whose class follows every other layer.
    pub config: bool,
}
/// `GPL-3.0-or-later · open`, or the user-supplied wording.
pub fn layer_text(license: Option<&str>) -> String {
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
pub struct Verdict {
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
pub fn extra_kind(kind: image_store::media_set::ExtraKind) -> &'static str {
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
pub fn gui_reason(reason: &str) -> String {
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
pub fn stop(text: &str) -> String {
    if text.ends_with(['.', '…']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}
/// A reason on its own line: capitalised and ended with a stop.
pub fn sentence(text: &str) -> String {
    match text.get(..1) {
        Some(first) => stop(&format!("{}{}", first.to_uppercase(), &text[1..])),
        None => String::new(),
    }
}

/// One package's statement under edit, with the library's latest verdict.
#[derive(Clone, Debug)]
pub struct Draft {
    pub facts: Facts,
    initial: Fields,
    pub fields: Fields,
    /// Raised with every change; a verdict answers one revision.
    pub revision: u64,
    verdict: Option<(u64, Result<Verdict, String>)>,
    /// The newest accepted verdict, shown while a newer one is pending or refused.
    pub last_ok: Option<Verdict>,
    in_flight: Option<u64>,
    pub label: String,
    pub label_problem: Option<String>,
    pub readme_buffer: String,
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
    pub fn chosen_license(&self) -> Option<Expression> {
        if self.fields.rights != Rights::Open {
            return None;
        }
        match &self.facts.catalogue {
            Catalogue::Open { license, .. } => Expression::parse(license).ok(),
            _ => self.fields.license.expression(),
        }
    }
    pub fn narrowed(&self) -> bool {
        self.facts
            .catalogue_fit
            .as_ref()
            .is_some_and(|(boards, classes)| {
                &self.fields.boards != boards || &self.fields.classes != classes
            })
    }
}

/// The pane's reading of a selected package that needs the store: its
/// README text and where the push guard lets it go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDetails {
    pub readme: Option<String>,
    /// The guard's effective class of the config.
    pub config_open: bool,
    pub push: Push,
    /// An open ROM whose licence obliges the source travels with it.
    pub source_layer: bool,
}
pub use image_registry::guard::Reach as Push;
/// The pane's Push line.
pub fn push_text(details: &PackageDetails) -> String {
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
pub fn read_package(
    store: &Store,
    tag: &str,
    digest: &Digest,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<PackageDetails, String> {
    use image_registry::guard;
    let plan = image_registry::push::plan_with_catalogue(
        store,
        tag,
        library_model::registry_transfer::dry_run_target(),
        None,
        |manifest| pc_image::firmware::restricted(manifest, catalogue),
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
pub struct EditSource {
    /// The new revision's tag, `:N` to `:N+1` as the command line names it.
    pub target: String,
    pub name: String,
    pub draft: Draft,
}

/// Read a stored package for Edit metadata…: its fields, the current class
/// of its ROM images and source archive, and its README when written by hand.
pub fn open_edit(
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
pub fn preview_edit(
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
pub fn publish_edit(
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

/// What the desktop's tests of the statement editor compare with.
#[cfg(feature = "test-support")]
pub mod test_support {
    use image_store::Descriptor;

    /// A layer's open licence, as the layers table names it.
    pub fn class(d: &Descriptor) -> Option<String> {
        super::class(d)
    }
}
