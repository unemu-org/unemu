// SPDX-License-Identifier: AGPL-3.0-or-later
//! Independent release requirements of published artifacts; machine-images §5.6.
//!
//! `unemu.min-app-version` names the selected platform core's release, and
//! `unemu.min-frontend-version` names the composing application's release.
//! Both are manifest-only SemVer normal versions, read leniently before strict
//! codecs or blob downloads. Application composition injects the two levels;
//! the store neither links a core nor infers an application version from its package.
//!
//! Shared envelope, routing, compression and package rows belong to the front end.
//! Providers supply core-owned rows and their own package-format rows separately.
//! Metadata schemas and title fields raise neither runnable requirement. Core
//! baseline 0.0.1 and the frozen front-end extraction baseline 0.0.3 keep existing
//! manifests byte-identical. Higher inherited floors stay within their own domain.

use crate::{
    artifact::NEUTRAL_TYPES, firmware, invalid, media_set, media_type, Error, Manifest, Result,
    MAX_SMALL_BLOB,
};
use serde::Deserialize;
use std::{fmt, str::FromStr};

/// Manifest annotation naming the oldest core release for `unemu.platform`.
pub const MIN_APP_VERSION: &str = "unemu.min-app-version";
/// Manifest-only minimum front-end release; independent of the core release.
pub const MIN_FRONTEND_VERSION: &str = "unemu.min-frontend-version";
/// The name of the PC core in compatibility diagnostics.
pub const PROGRAM: &str = "unemupc";

/// A release version: SemVer 2.0.0 §2 normal version `MAJOR.MINOR.PATCH`,
/// ordered by §11 precedence (major, then minor, then patch).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl AppVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl FromStr for AppVersion {
    type Err = Error;

    /// Three dot-separated non-negative integers without leading zeros
    /// (SemVer 2.0.0 §2); a pre-release or build part is refused.
    fn from_str(text: &str) -> Result<Self> {
        let number = |part: &str| {
            (!part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part == "0" || !part.starts_with('0')))
            .then(|| part.parse::<u64>().ok())
            .flatten()
        };
        let mut parts = text.split('.');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(major), Some(minor), Some(patch), None) => {
                match (number(major), number(minor), number(patch)) {
                    (Some(major), Some(minor), Some(patch)) => Ok(Self::new(major, minor, patch)),
                    _ => Err(not_a_release(text)),
                }
            }
            _ => Err(not_a_release(text)),
        }
    }
}

fn not_a_release(text: &str) -> Error {
    invalid(format!(
        "{text:?} is not a release version (MAJOR.MINOR.PATCH, no pre-release or build part)"
    ))
}

impl fmt::Display for AppVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Release 0.0.1, the first one. Every feature it reads is the baseline, and
/// an artifact that needs nothing newer carries no annotation.
pub const BASELINE: AppVersion = AppVersion::new(0, 0, 1);
/// The next front-end release. Rows use literal historical releases so their
/// meaning survives later releases; this constant advances with the application.
pub const NEXT_RELEASE: AppVersion = AppVersion::new(0, 0, 5);
/// The newest shared front-end row. Composition also includes the PC provider
/// and recipe parser's front-end maxima, independently of every core table.
pub const NEWEST_ROW: AppVersion = AppVersion::new(0, 0, 3);
/// The frozen pre-extraction release line. It is never the store package version.
pub const EXTRACTION_BASELINE: AppVersion = AppVersion::new(0, 0, 3);

/// A release domain; a requirement is always compared within its own domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Core,
    Frontend,
}
impl Component {
    pub fn annotation(self) -> &'static str {
        match self {
            Self::Core => MIN_APP_VERSION,
            Self::Frontend => MIN_FRONTEND_VERSION,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Core => PROGRAM,
            Self::Frontend => "front end",
        }
    }
    pub fn baseline(self) -> AppVersion {
        match self {
            Self::Core => BASELINE,
            Self::Frontend => EXTRACTION_BASELINE,
        }
    }
}

/// Injected release levels for one selected platform, including development rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Levels {
    pub core: Option<AppVersion>,
    pub frontend: AppVersion,
}
impl Default for Levels {
    fn default() -> Self {
        Self {
            core: None,
            frontend: EXTRACTION_BASELINE,
        }
    }
}
impl Levels {
    pub fn new(
        core: Option<(AppVersion, AppVersion)>,
        frontend: AppVersion,
        frontend_row: AppVersion,
    ) -> Self {
        Self {
            core: core.map(|(version, row)| version.max(row)),
            frontend: frontend.max(frontend_row),
        }
    }
    pub fn level(self, component: Component) -> Option<AppVersion> {
        match component {
            Component::Core => self.core,
            Component::Frontend => Some(self.frontend),
        }
    }
}

/// Release descriptions supplied by application composition, keyed by platform.
#[derive(Clone, Debug, Default)]
pub struct Compatibility {
    pub frontend: Option<AppVersion>,
    pub frontend_row: AppVersion,
    pub cores: Vec<(String, AppVersion, AppVersion)>,
}
impl Default for AppVersion {
    fn default() -> Self {
        BASELINE
    }
}
impl Compatibility {
    pub fn new(frontend: AppVersion, newest_row: AppVersion) -> Self {
        Self {
            frontend: Some(frontend),
            frontend_row: newest_row,
            cores: Vec::new(),
        }
    }
    pub fn with_core(
        mut self,
        platform: &str,
        version: AppVersion,
        newest_row: AppVersion,
    ) -> Self {
        self.cores.push((platform.into(), version, newest_row));
        self
    }
    pub fn levels(&self, platform: &str) -> Levels {
        let core = self
            .cores
            .iter()
            .find(|(id, _, _)| crate::platform::same(platform, id))
            .map(|(_, version, row)| (*version, *row));
        Levels::new(
            core,
            self.frontend.unwrap_or(EXTRACTION_BASELINE),
            self.frontend_row,
        )
    }
    pub fn manifest_platform(bytes: &[u8]) -> Option<String> {
        #[derive(Deserialize)]
        struct Routing {
            #[serde(default)]
            annotations: std::collections::BTreeMap<String, serde_json::Value>,
        }
        (bytes.len() as u64 <= MAX_SMALL_BLOB)
            .then(|| serde_json::from_slice::<Routing>(bytes).ok())
            .flatten()
            .and_then(|routing| {
                routing
                    .annotations
                    .get("unemu.platform")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
    }
    pub fn manifest_levels(&self, bytes: &[u8]) -> Levels {
        self.levels(
            Self::manifest_platform(bytes)
                .as_deref()
                .unwrap_or_default(),
        )
    }
    pub fn check(&self, bytes: &[u8], reference: &str) -> Result<()> {
        let levels = self.manifest_levels(bytes);
        Requirements::read(bytes).check(reference, levels)?;
        if self.frontend.is_some() && levels.core.is_none() {
            if let Some(platform) = Self::manifest_platform(bytes) {
                return Err(invalid(format!(
                    "{reference}: no core available for platform {platform}"
                )));
            }
        }
        Ok(())
    }
}

/// The independently computed floors of one artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Releases {
    pub core: AppVersion,
    pub frontend: AppVersion,
}
impl Default for Releases {
    fn default() -> Self {
        Self {
            core: BASELINE,
            frontend: BASELINE,
        }
    }
}

/// Both manifest-only requirements, read before any strict codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirements {
    pub core: Requirement,
    pub frontend: Requirement,
}
impl Requirements {
    pub fn read(bytes: &[u8]) -> Self {
        Self {
            core: Requirement::read_key(bytes, MIN_APP_VERSION),
            frontend: Requirement::read_key(bytes, MIN_FRONTEND_VERSION),
        }
    }
    pub fn of(manifest: &Manifest) -> Self {
        Self {
            core: Requirement::of(manifest),
            frontend: Requirement::of_text(
                manifest
                    .annotations
                    .get(MIN_FRONTEND_VERSION)
                    .map(String::as_str),
            ),
        }
    }
    pub fn unmet_reason(&self, levels: Levels) -> Option<String> {
        self.core
            .unmet_reason(Component::Core, levels.core)
            .or_else(|| {
                self.frontend
                    .unmet_reason(Component::Frontend, Some(levels.frontend))
            })
    }
    pub fn check(&self, reference: &str, levels: Levels) -> Result<()> {
        self.core.check(reference, Component::Core, levels.core)?;
        self.frontend
            .check(reference, Component::Frontend, Some(levels.frontend))
    }
    pub fn releases(&self) -> Result<Releases> {
        Ok(Releases {
            core: self.core.release()?,
            frontend: self.frontend.release()?,
        })
    }
}

/// What a manifest declares about the release it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requirement {
    /// No annotation: any release reads it.
    None,
    Release(AppVersion),
    /// An annotation that is not a release version; it counts as unmet.
    Invalid(String),
}

impl Requirement {
    /// Read the marker from manifest bytes without validating anything else,
    /// so a future envelope, artifact type or field still yields it. Bytes
    /// that are not a bounded JSON object give no requirement; the strict
    /// decode that follows refuses them.
    pub fn read(manifest: &[u8]) -> Self {
        Self::read_key(manifest, MIN_APP_VERSION)
    }

    fn read_key(manifest: &[u8], key: &str) -> Self {
        #[derive(Deserialize)]
        struct Lenient {
            #[serde(default)]
            annotations: Option<serde_json::Value>,
        }
        if manifest.len() as u64 > MAX_SMALL_BLOB {
            return Self::None;
        }
        match serde_json::from_slice::<Lenient>(manifest) {
            Ok(Lenient {
                annotations: Some(serde_json::Value::Object(annotations)),
            }) => match annotations.get(key) {
                None => Self::None,
                Some(serde_json::Value::String(text)) => Self::parse(text),
                Some(other) => Self::Invalid(other.to_string()),
            },
            _ => Self::None,
        }
    }

    /// The marker of a decoded manifest.
    pub fn of(manifest: &Manifest) -> Self {
        manifest
            .annotations
            .get(MIN_APP_VERSION)
            .map_or(Self::None, |text| Self::parse(text))
    }

    /// The requirement an annotation value states, as a summary keeps it:
    /// none without a value, unmet when the value is not a release version.
    pub fn of_text(text: Option<&str>) -> Self {
        text.map_or(Self::None, Self::parse)
    }

    fn parse(text: &str) -> Self {
        text.parse()
            .map_or_else(|_| Self::Invalid(text.into()), Self::Release)
    }

    /// The annotation value as it was read, for a summary to keep.
    pub fn text(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Release(release) => Some(release.to_string()),
            Self::Invalid(text) => Some(text.clone()),
        }
    }

    /// "Needs unemupc X or newer" when this build does not read the
    /// artifact, as every list that disables it says.
    pub fn unmet_reason(&self, component: Component, level: Option<AppVersion>) -> Option<String> {
        (!level.is_some_and(|level| self.met_by(level)) && !matches!(self, Self::None)).then(|| {
            needs_component(
                component,
                match self {
                    Self::Release(required) => Some(*required),
                    _ => None,
                },
            )
        })
    }

    /// Whether a release at `level` reads the artifact: the requirement is
    /// no higher than the level. A value that does not parse is unmet.
    pub fn met_by(&self, level: AppVersion) -> bool {
        match self {
            Self::None => true,
            Self::Release(required) => *required <= level,
            Self::Invalid(_) => false,
        }
    }

    /// Refuse an artifact this build does not read, naming `reference`.
    pub fn check(
        &self,
        reference: &str,
        component: Component,
        level: Option<AppVersion>,
    ) -> Result<()> {
        if matches!(self, Self::None) || level.is_some_and(|level| self.met_by(level)) {
            return Ok(());
        }
        Err(Error::NeedsNewer {
            reference: reference.into(),
            component,
            required: match self {
                Self::Release(required) => Some(*required),
                _ => None,
            },
            level: level.unwrap_or(BASELINE),
        })
    }

    /// The release declared, [`BASELINE`] when there is none.
    pub fn release(&self) -> Result<AppVersion> {
        match self {
            Self::None => Ok(BASELINE),
            Self::Release(release) => Ok(*release),
            Self::Invalid(text) => Err(not_a_release(text)),
        }
    }
}

/// The reason an unmet requirement gives: "Needs unemupc X or newer", or
/// "Needs a newer unemupc" for a value that does not parse.
pub fn needs(required: Option<AppVersion>) -> String {
    needs_component(Component::Core, required)
}

pub fn needs_component(component: Component, required: Option<AppVersion>) -> String {
    let name = component.name();
    match required {
        Some(required) => format!("Needs {name} {required} or newer"),
        None => format!("Needs a newer {name}"),
    }
}

pub(crate) fn needs_newer(
    component: &Component,
    required: &Option<AppVersion>,
    level: &AppVersion,
) -> String {
    let name = component.name();
    match required {
        Some(required) => format!("needs {name} {required} or newer (this is {level})"),
        None => format!("needs a newer {name} (this is {level})"),
    }
}

/// Stamp each domain independently, preserving inherited floors and omitting its baseline.
pub fn stamp(
    manifest: &mut Manifest,
    required: Releases,
    stated: Option<AppVersion>,
    levels: Levels,
) -> Result<()> {
    let inherited = Requirement::of(manifest).release()?;
    let core = raised(
        required.core.max(inherited),
        stated,
        Component::Core,
        levels.core,
    )?;
    floor(manifest, Releases { core, ..required }, levels)
}

pub fn raised(
    required: AppVersion,
    stated: Option<AppVersion>,
    component: Component,
    level: Option<AppVersion>,
) -> Result<AppVersion> {
    within_level(required, component, level)?;
    let Some(stated) = stated else {
        return Ok(required);
    };
    if stated < required {
        return Err(invalid(format!("the stated minimum version {stated} is lower than {required}, the oldest {} that reads this artifact", component.name())));
    }
    within_level(stated, component, level)?;
    Ok(stated)
}

/// Carry floors only while the encoded content is retained. Repackaged ROMs carry only core floors.
pub fn carry(manifest: &mut Manifest, source: &Manifest, levels: Levels) -> Result<()> {
    floor(manifest, Requirements::of(source).releases()?, levels)
}

pub fn floor(manifest: &mut Manifest, releases: Releases, levels: Levels) -> Result<()> {
    let own = Requirements::of(manifest).releases()?;
    let floors = [
        (Component::Core, own.core.max(releases.core)),
        (Component::Frontend, own.frontend.max(releases.frontend)),
    ];
    for (component, required) in floors {
        within_level(required, component, levels.level(component))?;
    }
    for (component, required) in floors {
        set(manifest, component, required);
    }
    Ok(())
}

fn within_level(
    required: AppVersion,
    component: Component,
    level: Option<AppVersion>,
) -> Result<()> {
    if required <= component.baseline() {
        return Ok(());
    }
    if level.is_some_and(|level| required <= level) {
        return Ok(());
    }
    Err(invalid(format!("minimum {} version {required} is newer than this build's level {}; check the component's newest compatibility row", component.name(), level.unwrap_or(BASELINE))))
}

fn set(manifest: &mut Manifest, component: Component, release: AppVersion) {
    let key = component.annotation();
    if release > component.baseline() {
        manifest.annotations.insert(key.into(), release.to_string());
    } else {
        manifest.annotations.remove(key);
    }
}

/// One table row: a feature and the first release that reads it.
pub type Row = (&'static str, AppVersion);

/// Values of the descriptor annotation `unemu.redistribution`; the codec
/// accepts exactly these.
pub const REDISTRIBUTION: &[Row] = &[("open", BASELINE), ("user-supplied", BASELINE)];

/// The newest release the descriptors' `unemu.redistribution` values need.
fn redistribution(manifest: &Manifest) -> Result<AppVersion> {
    let mut release = BASELINE;
    for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
        if let Some(value) = descriptor.annotations.get(crate::license::REDISTRIBUTION) {
            release = release.max(lookup(&[REDISTRIBUTION], value).ok_or_else(|| {
                invalid(format!("no compatibility row for redistribution {value}"))
            })?);
        }
    }
    Ok(release)
}

/// Platform ids of the alias table ([`crate::platform::ALIASES`]): a retired
/// id is read since the release that first wrote it, the id that replaced it
/// first by the release that renamed it, because older releases compare ids
/// exactly. Every other id is its provider's to rate.
pub const PLATFORMS: &[Row] = &[
    ("ibm-pc-at", BASELINE),
    ("ibm-pc", AppVersion::new(0, 0, 3)),
];

/// The release that first reads a platform id: its row in [`PLATFORMS`], the
/// baseline for an id the alias table does not name.
pub fn platform(id: &str) -> AppVersion {
    lookup(&[PLATFORMS], id).unwrap_or(BASELINE)
}

/// The release the `platform` value of a config table needs; a missing or
/// non-string value is the codec's to refuse.
pub fn config_platform(config: &toml::Table) -> AppVersion {
    config
        .get("platform")
        .and_then(toml::Value::as_str)
        .map_or(BASELINE, platform)
}

/// The manifest artifact types the envelope reads. Recipe artifacts
/// (recipes.md §7.1) are first read by the release after 0.0.2.
pub fn artifact(kind: &str) -> Option<AppVersion> {
    match kind {
        media_type::MACHINE | media_type::MEDIA_SET | media_type::FIRMWARE => Some(BASELINE),
        media_type::RECIPE_MEDIA | media_type::RECIPE_FIRMWARE => Some(AppVersion::new(0, 0, 3)),
        _ => None,
    }
}

/// The config and layer media types of a recipe artifact (recipes.md
/// §7.1), which only a recipe envelope reads.
pub const RECIPE_TYPES: &[Row] = &[
    (media_type::RECIPE_CONFIG, AppVersion::new(0, 0, 3)),
    (media_type::RECIPE_WORK, AppVersion::new(0, 0, 3)),
    (media_type::RECIPE_INPUT, AppVersion::new(0, 0, 3)),
    (media_type::RECIPE_LOCK, AppVersion::new(0, 0, 3)),
];

/// Compressed payload types the installation-set and firmware-package
/// envelopes declare ([`crate::codec::MEDIA_PAYLOAD_TYPES`],
/// [`crate::codec::FIRMWARE_PAYLOAD_TYPES`]).
const COMPRESSED: &[Row] = &[
    ("application/vnd.unemu.media.floppy.v1+zstd", BASELINE),
    ("application/vnd.unemu.media.optical.v1+zstd", BASELINE),
    ("application/vnd.unemu.media.extra.v1+zstd", BASELINE),
    ("application/vnd.unemu.firmware.v1+zstd", BASELINE),
    ("application/x-tar+zstd", BASELINE),
];

/// A payload media type the shared envelope reads, raw or compressed.
pub fn payload(kind: &str) -> Option<AppVersion> {
    NEUTRAL_TYPES
        .iter()
        .find(|(t, _, _)| *t == kind)
        .map(|(_, _, since)| *since)
        .or_else(|| lookup(&[COMPRESSED, RECIPE_TYPES], kind))
}

/// The row named `key` in any of `rows`.
pub fn lookup(rows: &[&[Row]], key: &str) -> Option<AppVersion> {
    rows.iter()
        .flat_map(|rows| rows.iter())
        .find(|(name, _)| *name == key)
        .map(|(_, since)| *since)
}

/// Keys of an installation-set config (machine-images §5.2), as dotted paths
/// through tables and arrays of tables. The `media.format` table is the
/// provider's.
pub const MEDIA_SET_KEYS: &[Row] = &[
    ("schema", BASELINE),
    ("platform", BASELINE),
    ("media", BASELINE),
    ("media.id", BASELINE),
    ("media.label", BASELINE),
    ("media.format", BASELINE),
    ("media.blob", BASELINE),
];
/// Keys of the extras an installation set or a firmware package carries.
pub const EXTRA_KEYS: &[Row] = &[
    ("extras", BASELINE),
    ("extras.blob", BASELINE),
    ("extras.filename", BASELINE),
    ("extras.size", BASELINE),
    ("extras.kind", BASELINE),
    ("extras.dos_name", BASELINE),
];
/// Keys of a firmware-package config (machine-images §5.3). The `fits`
/// table is the provider's.
pub const FIRMWARE_KEYS: &[Row] = &[
    ("schema", BASELINE),
    ("platform", BASELINE),
    ("name", BASELINE),
    ("vendor", BASELINE),
    ("version", BASELINE),
    ("date", BASELINE),
    ("notes", BASELINE),
    ("description", BASELINE),
    ("labels", BASELINE),
    ("fits", BASELINE),
    ("provenance", BASELINE),
    ("provenance.kind", BASELINE),
    ("provenance.from", BASELINE),
    ("provenance.revision", BASELINE),
    ("provenance.url", BASELINE),
    ("provenance.date", BASELINE),
    ("roms", BASELINE),
    ("roms.id", BASELINE),
    ("roms.role", BASELINE),
    ("roms.catalogue_id", BASELINE),
    ("roms.sha256", BASELINE),
    ("roms.size", BASELINE),
    ("roms.blob", BASELINE),
    ("roms.position", BASELINE),
    ("roms.part_number", BASELINE),
    ("roms.requires", BASELINE),
    ("roms.parts", BASELINE),
    ("roms.parts.blob", BASELINE),
    ("roms.parts.sha256", BASELINE),
    ("roms.parts.size", BASELINE),
    ("roms.parts.position", BASELINE),
    ("roms.parts.part_number", BASELINE),
    ("roms.parts.lane", BASELINE),
    ("roms.parts.offset", BASELINE),
    ("sources", BASELINE),
    ("sources.blob", BASELINE),
    ("sources.filename", BASELINE),
    ("sources.size", BASELINE),
    ("sources.format", BASELINE),
    ("sources.roms", BASELINE),
    ("readme", BASELINE),
    ("readme.blob", BASELINE),
    ("readme.size", BASELINE),
];

/// The newest release any key of `config` needs. Each key is named by its
/// dotted path through tables and arrays of tables and must have a row in
/// `rows`; a key without one is refused, because no release is known to read
/// it. The tables at `foreign` paths belong to another table's owner and are
/// not entered.
pub fn keys(config: &toml::Table, rows: &[&[Row]], foreign: &[&str]) -> Result<AppVersion> {
    fn walk(
        table: &toml::Table,
        prefix: &str,
        rows: &[&[Row]],
        foreign: &[&str],
        release: &mut AppVersion,
    ) -> Result<()> {
        for (key, value) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            let since = lookup(rows, &path)
                .ok_or_else(|| invalid(format!("no compatibility row for config key {path}")))?;
            *release = (*release).max(since);
            if foreign.contains(&path.as_str()) {
                continue;
            }
            match value {
                toml::Value::Table(table) => walk(table, &path, rows, foreign, release)?,
                toml::Value::Array(items) => {
                    for table in items.iter().filter_map(toml::Value::as_table) {
                        walk(table, &path, rows, foreign, release)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut release = BASELINE;
    walk(config, "", rows, foreign, &mut release)?;
    Ok(release)
}

/// The release that first reads an extra's kind.
pub fn extra_kind(kind: media_set::ExtraKind) -> AppVersion {
    use media_set::ExtraKind::*;
    match kind {
        Document | Archive | Executable | Other => BASELINE,
    }
}

/// The release that first reads a firmware package's provenance kind.
pub fn provenance_kind(kind: firmware::ProvenanceKind) -> AppVersion {
    use firmware::ProvenanceKind::*;
    match kind {
        Dumped | Downloaded | Built => BASELINE,
    }
}

/// The release that first reads a source archive's format.
pub fn archive_format(format: firmware::ArchiveFormat) -> AppVersion {
    use firmware::ArchiveFormat::*;
    match format {
        Tar | Zip => BASELINE,
    }
}

/// Metadata schemas (machine-images §2.1), by number: the release that first
/// reads each one in full. A release shows a schema it does not read
/// read-only and still runs the artifact, so these rows never raise the
/// marker; they decide which schema a writer needs ([`metadata_schema_for`])
/// and which releases show a catalogue read-only. Release 0.0.2 reads only
/// schema 1.
pub const METADATA_SCHEMAS: &[Row] = &[("1", BASELINE), ("2", AppVersion::new(0, 0, 3))];

/// Fields of a metadata title record: the release that first reads each. A
/// writer uses the oldest schema whose release reads every field a title
/// sets, so metadata without the newer fields keeps schema 1 and its bytes.
pub const TITLE_FIELDS: &[Row] = &[
    ("id", BASELINE),
    ("title", BASELINE),
    ("sort_title", BASELINE),
    ("series", BASELINE),
    ("genres", BASELINE),
    ("developers", BASELINE),
    ("publishers", BASELINE),
    ("platform", BASELINE),
    ("release_year", BASELINE),
    ("players", BASELINE),
    ("languages", BASELINE),
    ("regions", BASELINE),
    ("description", BASELINE),
    ("version", BASELINE),
    ("edition", BASELINE),
    ("source_media", BASELINE),
    ("tags", BASELINE),
    ("cover", BASELINE),
    ("screenshots", BASELINE),
    ("launch", BASELINE),
    ("needs", AppVersion::new(0, 0, 3)),
    ("requirements", AppVersion::new(0, 0, 3)),
    ("work_id", AppVersion::new(0, 0, 3)),
    ("synopsis", AppVersion::new(0, 0, 3)),
    ("work_ids", AppVersion::new(0, 0, 3)),
];

/// The release that first reads metadata `schema` in full; `None` for a
/// schema no row names, which this build shows read-only.
pub fn metadata_schema(schema: u32) -> Option<AppVersion> {
    lookup(&[METADATA_SCHEMAS], &schema.to_string())
}

/// The oldest metadata schema whose release reads every one of `fields`,
/// the keys of title records. A field without a row is refused, because no
/// schema is known to hold it.
pub fn metadata_schema_for<'a>(fields: impl IntoIterator<Item = &'a str>) -> Result<u32> {
    let mut release = BASELINE;
    for field in fields {
        release = release.max(
            lookup(&[TITLE_FIELDS], field)
                .ok_or_else(|| invalid(format!("no compatibility row for title field {field}")))?,
        );
    }
    METADATA_SCHEMAS
        .iter()
        .find(|(_, since)| *since >= release)
        .and_then(|(schema, _)| schema.parse().ok())
        .ok_or_else(|| invalid(format!("no metadata schema is read by {release}")))
}

/// The newest release the shared vocabulary of a set's config needs.
pub fn media_set_values(config: &media_set::Config) -> AppVersion {
    config
        .extras
        .iter()
        .map(|e| extra_kind(e.kind))
        .fold(BASELINE, AppVersion::max)
}

/// The newest release the shared vocabulary of a package's config needs.
pub fn firmware_values(config: &firmware::Config) -> AppVersion {
    let extras = config.extras.iter().map(|e| extra_kind(e.kind));
    let sources = config.sources.iter().map(|s| archive_format(s.format));
    let provenance = config
        .provenance
        .iter()
        .filter_map(|p| p.kind)
        .map(provenance_kind);
    extras
        .chain(sources)
        .chain(provenance)
        .fold(BASELINE, AppVersion::max)
}

/// The newest release the manifest's artifact type, its platform and every
/// payload media type need; `provider` holds the rows of provider-declared
/// types. A media type without a row is refused.
pub fn envelope(manifest: &Manifest, provider: &[Row]) -> Result<AppVersion> {
    let mut release = artifact(manifest.artifact_type())
        .ok_or_else(|| {
            invalid(format!(
                "no compatibility row for artifact type {}",
                manifest.artifact_type()
            ))
        })?
        .max(platform(manifest.platform()?));
    for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
        let kind = descriptor.media_type.as_str();
        release = release.max(
            payload(kind)
                .or_else(|| lookup(&[provider], kind))
                .ok_or_else(|| invalid(format!("no compatibility row for media type {kind}")))?,
        );
    }
    Ok(release.max(redistribution(manifest)?))
}

/// What the shared rows alone say an artifact needs: its envelope and
/// platform, and for an installation set or a firmware package the keys and
/// values of its config that the store owns. Provider-declared payload types
/// and the provider's tables are left to the provider, which overrides
/// [`crate::ConfigValidator::required_release`] with its own rows.
pub fn shared_frontend_release(manifest: &Manifest, config: &[u8]) -> Result<AppVersion> {
    let mut release = artifact(manifest.artifact_type())
        .ok_or_else(|| {
            invalid(format!(
                "no compatibility row for artifact type {}",
                manifest.artifact_type()
            ))
        })?
        .max(platform(manifest.platform()?));
    for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
        if let Some(since) = payload(&descriptor.media_type) {
            release = release.max(since);
        }
    }
    release = release.max(redistribution(manifest)?);
    // Every artifact's config is TOML with the routing `platform` key.
    let table: toml::Table =
        toml::from_str(std::str::from_utf8(config).map_err(|e| invalid(e.to_string()))?)
            .map_err(|e| invalid(e.to_string()))?;
    release = release.max(config_platform(&table));
    match manifest.artifact_type() {
        media_type::MEDIA_SET => {
            release = release
                .max(keys(
                    &table,
                    &[MEDIA_SET_KEYS, EXTRA_KEYS],
                    &["media.format"],
                )?)
                .max(media_set_values(&media_set::Config::decode(config)?));
        }
        media_type::FIRMWARE => {
            release = release
                .max(keys(&table, &[FIRMWARE_KEYS, EXTRA_KEYS], &["fits"])?)
                .max(firmware_values(&firmware::Config::decode(config)?));
        }
        _ => {}
    }
    Ok(release)
}

/// The field names a serde-derived struct deserializes, for the tests that
/// keep each config table's rows complete.
#[cfg(any(test, feature = "test-support"))]
pub fn struct_fields<T: serde::de::DeserializeOwned>() -> &'static [&'static str] {
    use serde::de::{Deserializer, Visitor};
    #[derive(Debug)]
    struct Fields(&'static [&'static str]);
    impl fmt::Display for Fields {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("struct fields")
        }
    }
    impl std::error::Error for Fields {}
    impl serde::de::Error for Fields {
        fn custom<M: fmt::Display>(_: M) -> Self {
            Self(&[])
        }
    }
    struct Probe;
    impl<'de> Deserializer<'de> for Probe {
        type Error = Fields;
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> std::result::Result<V::Value, Fields> {
            Err(Fields(&[]))
        }
        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> std::result::Result<V::Value, Fields> {
            Err(Fields(fields))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map enum identifier ignored_any
        }
    }
    match T::deserialize(Probe) {
        Err(Fields(fields)) => fields,
        Ok(_) => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PACKAGE: AppVersion = EXTRACTION_BASELINE;
    fn level() -> AppVersion {
        EXTRACTION_BASELINE
    }
    fn levels() -> Levels {
        Levels::new(Some((level(), BASELINE)), level(), NEWEST_ROW)
    }

    use crate::{Descriptor, Digest};

    fn stamp(manifest: &mut Manifest, core: AppVersion, stated: Option<AppVersion>) -> Result<()> {
        super::stamp(
            manifest,
            Releases {
                core,
                ..Releases::default()
            },
            stated,
            levels(),
        )
    }
    fn floor(manifest: &mut Manifest, core: AppVersion) -> Result<()> {
        super::floor(
            manifest,
            Releases {
                core,
                ..Releases::default()
            },
            levels(),
        )
    }
    fn carry(manifest: &mut Manifest, source: &Manifest) -> Result<()> {
        super::carry(manifest, source, levels())
    }
    fn set(manifest: &mut Manifest, core: AppVersion) {
        super::set(manifest, Component::Core, core)
    }
    fn machine() -> Manifest {
        let config = Descriptor::new(media_type::CONFIG, Digest::of(b"config"), 6);
        let metadata = Descriptor::new(media_type::METADATA, Digest::of(b"metadata"), 8);
        Manifest::new("synthetic", config, vec![metadata]).unwrap()
    }

    #[test]
    fn release_versions_parse_as_semver_normal_versions_only() {
        assert_eq!("0.0.1".parse::<AppVersion>().unwrap(), BASELINE);
        assert_eq!(
            "10.20.300".parse::<AppVersion>().unwrap(),
            AppVersion::new(10, 20, 300)
        );
        assert_eq!(AppVersion::new(1, 2, 3).to_string(), "1.2.3");
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-rc.1",
            "1.2.3+build",
            "v1.2.3",
            " 1.2.3",
            "1..3",
            "-1.2.3",
            "1.2.x",
            "99999999999999999999.0.0",
        ] {
            assert!(bad.parse::<AppVersion>().is_err(), "{bad:?}");
        }
        assert!(AppVersion::new(0, 0, 10) > AppVersion::new(0, 0, 9));
        assert!(AppVersion::new(0, 1, 0) > AppVersion::new(0, 0, 99));
        assert!(AppVersion::new(1, 0, 0) > AppVersion::new(0, 99, 99));
    }

    #[test]
    fn a_requirement_is_met_up_to_this_release_level_and_unparseable_is_unmet() {
        let at = AppVersion::new(0, 0, 2);
        assert!(Requirement::None.met_by(at));
        assert!(Requirement::Release(at).met_by(at));
        assert!(Requirement::Release(BASELINE).met_by(at));
        assert!(!Requirement::Release(AppVersion::new(0, 0, 3)).met_by(at));
        assert!(!Requirement::Invalid("next".into()).met_by(at));
        assert!(!Requirement::Invalid("next".into()).met_by(AppVersion::new(999, 0, 0)));
        // A development build whose tables name a newer release reads it.
        assert!(level() >= PACKAGE && level() >= NEWEST_ROW);
        let this = level();
        assert!(Requirement::Release(this)
            .check("x:1", Component::Core, Some(level()))
            .is_ok());
        let higher = AppVersion::new(this.major, this.minor, this.patch + 1);
        assert_eq!(
            Requirement::Release(higher)
                .check("x:1", Component::Core, Some(level()))
                .unwrap_err()
                .to_string(),
            format!("x:1 needs unemupc {higher} or newer (this is {this})")
        );
        assert_eq!(
            Requirement::Invalid("soon".into())
                .check("x:1", Component::Core, Some(level()))
                .unwrap_err()
                .to_string(),
            format!("x:1 needs a newer unemupc (this is {this})")
        );
        assert_eq!(
            needs(Some(higher)),
            format!("Needs unemupc {higher} or newer")
        );
        assert_eq!(needs(None), "Needs a newer unemupc");
        // What a summary keeps and the reason its lists give.
        for (text, requirement) in [
            (None, Requirement::None),
            (Some("0.0.1"), Requirement::Release(BASELINE)),
            (Some("soon"), Requirement::Invalid("soon".into())),
        ] {
            assert_eq!(Requirement::of_text(text), requirement);
            assert_eq!(requirement.text().as_deref(), text);
        }
        assert_eq!(
            Requirement::Release(this).unmet_reason(Component::Core, Some(level())),
            None
        );
        assert_eq!(
            Requirement::None.unmet_reason(Component::Core, Some(level())),
            None
        );
        assert_eq!(
            Requirement::Release(higher).unmet_reason(Component::Core, Some(level())),
            Some(format!("Needs unemupc {higher} or newer"))
        );
        assert_eq!(
            Requirement::Invalid("soon".into())
                .unmet_reason(Component::Core, Some(level()))
                .as_deref(),
            Some("Needs a newer unemupc")
        );
    }

    #[test]
    fn the_marker_is_read_leniently_before_any_strict_decode() {
        assert_eq!(
            Requirement::read(&machine().encode().unwrap()),
            Requirement::None
        );
        let mut marked = machine();
        marked
            .annotations
            .insert(MIN_APP_VERSION.into(), "99.0.0".into());
        let bytes = marked.encode().unwrap();
        assert_eq!(
            Requirement::read(&bytes),
            Requirement::Release(AppVersion::new(99, 0, 0))
        );
        assert_eq!(
            Requirement::of(&Manifest::decode(&bytes).unwrap()),
            Requirement::read(&bytes)
        );
        // A future envelope: an unknown artifact type and top-level field.
        let future = br#"{"schemaVersion":3,"artifactType":"application/vnd.unemu.machine.v9","future":{"x":1},"annotations":{"unemu.platform":"ibm-pc","unemu.min-app-version":"7.1.0"}}"#;
        assert!(Manifest::decode(future).is_err());
        assert_eq!(
            Requirement::read(future),
            Requirement::Release(AppVersion::new(7, 1, 0))
        );
        for (bytes, expected) in [
            (
                &br#"{"annotations":{"unemu.min-app-version":"1.0"}}"#[..],
                Requirement::Invalid("1.0".into()),
            ),
            (
                br#"{"annotations":{"unemu.min-app-version":3}}"#,
                Requirement::Invalid("3".into()),
            ),
            (br#"{"annotations":{"other":"1.0.0"}}"#, Requirement::None),
            (br#"not json"#, Requirement::None),
            (br#"[1,2]"#, Requirement::None),
        ] {
            assert_eq!(Requirement::read(bytes), expected);
        }
    }

    #[test]
    fn writers_leave_the_baseline_out_and_a_statement_only_raises() {
        let plain = machine();
        let mut stamped = machine();
        stamp(&mut stamped, BASELINE, None).unwrap();
        assert_eq!(stamped.encode().unwrap(), plain.encode().unwrap());
        stamp(&mut stamped, BASELINE, Some(BASELINE)).unwrap();
        assert_eq!(stamped.encode().unwrap(), plain.encode().unwrap());

        // A build whose tables rate an artifact 0.0.3 writes 0.0.3. A row
        // above this build's level is the build's own mistake.
        let mut next = machine();
        set(&mut next, AppVersion::new(0, 0, 3));
        assert_eq!(next.annotations[MIN_APP_VERSION], "0.0.3");
        let beyond = AppVersion::new(level().major, level().minor, level().patch + 1);
        let mistake = stamp(&mut machine(), beyond, None).unwrap_err().to_string();
        assert!(mistake.contains("newest compatibility row"), "{mistake}");
        let mistake = floor(&mut machine(), beyond).unwrap_err().to_string();
        assert!(mistake.contains("newest compatibility row"), "{mistake}");

        let mut raised = machine();
        stamp(&mut raised, BASELINE, Some(PACKAGE)).unwrap();
        assert_eq!(Requirement::of(&raised), Requirement::Release(PACKAGE));
        let lower = stamp(&mut machine(), PACKAGE, Some(BASELINE))
            .unwrap_err()
            .to_string();
        assert!(
            lower.contains("is lower than")
                && lower.contains(&BASELINE.to_string())
                && lower.contains(&PACKAGE.to_string()),
            "{lower}"
        );
        let above = AppVersion::new(level().major + 1, 0, 0);
        let newer = stamp(&mut machine(), BASELINE, Some(above))
            .unwrap_err()
            .to_string();
        assert!(newer.contains(&above.to_string()), "{newer}");

        // A derived artifact keeps its source's marker as a floor.
        let mut derived = machine();
        carry(&mut derived, &raised).unwrap();
        assert_eq!(Requirement::of(&derived), Requirement::of(&raised));
        carry(&mut derived, &plain).unwrap();
        assert_eq!(Requirement::of(&derived), Requirement::of(&raised));
    }

    /// Rows name literal versions; the constant only says what a new row
    /// takes, so the release commit must move it past the new version.
    #[test]
    fn next_release_is_above_the_package_version() {
        assert!(
            NEXT_RELEASE > PACKAGE,
            "release {PACKAGE} is out: raise NEXT_RELEASE past it"
        );
    }

    /// A machine image, a set and a package that carry a marker no release
    /// reads yet and an unknown annotation decode in strict, display and
    /// provider modes: the property releases 0.0.1 and 0.0.2 rely on.
    #[test]
    fn marked_artifacts_decode_in_every_mode() {
        let config = |kind: &str| Descriptor::new(kind, Digest::of(kind.as_bytes()), 6);
        let metadata = Descriptor::new(media_type::METADATA, Digest::of(b"metadata"), 8);
        let artifacts = [
            (
                Manifest::new(
                    "synthetic",
                    config(media_type::CONFIG),
                    vec![metadata.clone()],
                )
                .unwrap(),
                &[crate::PayloadType {
                    media_type: "application/vnd.unemu.future.v1",
                    max_size: 1,
                }][..],
            ),
            (
                Manifest::new_media_set(
                    "synthetic",
                    config(media_type::MEDIA_SET_CONFIG),
                    vec![
                        Descriptor::new(media_type::FLOPPY, Digest::of(b"disk"), 4),
                        metadata.clone(),
                    ],
                )
                .unwrap(),
                crate::codec::MEDIA_PAYLOAD_TYPES,
            ),
            (
                Manifest::new_firmware(
                    "synthetic",
                    config(media_type::FIRMWARE_CONFIG),
                    vec![Descriptor::new(media_type::FIRMWARE, Digest::of(b"rom"), 3)],
                )
                .unwrap(),
                crate::codec::FIRMWARE_PAYLOAD_TYPES,
            ),
        ];
        for (mut manifest, types) in artifacts {
            manifest
                .annotations
                .insert(MIN_APP_VERSION.into(), "99.0.0".into());
            manifest
                .annotations
                .insert("unemu.future-annotation".into(), "anything".into());
            manifest
                .config
                .annotations
                .insert("unemu.future-annotation".into(), "anything".into());
            let bytes = manifest.encode().unwrap();
            assert_eq!(Manifest::decode(&bytes).unwrap(), manifest);
            assert_eq!(Manifest::decode_for_summary(&bytes).unwrap(), manifest);
            assert_eq!(
                Manifest::decode_with_types(&bytes, types).unwrap(),
                manifest
            );
            assert_eq!(
                Requirement::read(&bytes),
                Requirement::Release(AppVersion::new(99, 0, 0))
            );
        }
    }

    /// The id that replaced a retired one is first read by the release that
    /// renamed it, 0.0.3 for `ibm-pc`, whether the manifest or the config
    /// names it; the retired id and any other id need nothing newer than the
    /// baseline from the shared rows. Every id of the alias table has a row.
    #[test]
    fn a_renamed_platform_id_is_rated_by_the_release_that_renamed_it() {
        for (retired, current) in crate::platform::ALIASES {
            assert_eq!(lookup(&[PLATFORMS], retired), Some(BASELINE), "{retired}");
            assert!(lookup(&[PLATFORMS], current).is_some(), "{current}");
        }
        assert_eq!(platform("ibm-pc"), AppVersion::new(0, 0, 3));
        assert_eq!(platform("ibm-pc-at"), BASELINE);
        assert_eq!(platform("synthetic"), BASELINE);
        let rate = |manifest_platform: &str, config_platform: &str| {
            let config = format!("schema = 1\nplatform = \"{config_platform}\"\n");
            let metadata = Descriptor::new(media_type::METADATA, Digest::of(b"metadata"), 8);
            let manifest = Manifest::new(
                manifest_platform,
                Descriptor::new(
                    media_type::CONFIG,
                    Digest::of(config.as_bytes()),
                    config.len() as u64,
                ),
                vec![metadata],
            )
            .unwrap();
            (
                envelope(&manifest, &[]).unwrap(),
                shared_frontend_release(&manifest, config.as_bytes()).unwrap(),
            )
        };
        let next = AppVersion::new(0, 0, 3);
        assert_eq!(rate("ibm-pc", "ibm-pc"), (next, next));
        assert_eq!(rate("ibm-pc-at", "ibm-pc"), (BASELINE, next));
        assert_eq!(rate("ibm-pc", "ibm-pc-at"), (next, next));
        assert_eq!(rate("ibm-pc-at", "ibm-pc-at"), (BASELINE, BASELINE));
        assert_eq!(rate("synthetic", "synthetic"), (BASELINE, BASELINE));
        assert!(level() >= next, "this build reads what it writes");
    }

    /// The codec accepts exactly the redistribution values that have rows.
    #[test]
    fn redistribution_values_have_rows() {
        for value in ["open", "user-supplied", "public"] {
            let mut manifest = machine();
            manifest
                .config
                .annotations
                .insert(crate::license::REDISTRIBUTION.into(), value.into());
            let row = lookup(&[REDISTRIBUTION], value);
            assert_eq!(manifest.encode().is_ok(), row.is_some(), "{value}");
            if row.is_some() {
                assert_eq!(envelope(&manifest, &[]).unwrap(), BASELINE);
            }
        }
    }

    #[test]
    fn config_keys_take_the_newest_row_and_refuse_a_key_without_one() {
        const ROWS: &[Row] = &[
            ("schema", BASELINE),
            ("media", BASELINE),
            ("media.id", BASELINE),
            ("media.future", NEXT_RELEASE),
            ("opaque", BASELINE),
        ];
        let table = |text: &str| -> toml::Table { toml::from_str(text).unwrap() };
        assert_eq!(
            keys(&table("schema = 1\n[[media]]\nid = 'a'\n"), &[ROWS], &[]).unwrap(),
            BASELINE
        );
        assert_eq!(
            keys(
                &table("schema = 1\n[[media]]\nid = 'a'\n[[media]]\nid = 'b'\nfuture = 1\n"),
                &[ROWS],
                &[]
            )
            .unwrap(),
            NEXT_RELEASE
        );
        let error = keys(&table("[[media]]\nother = 1\n"), &[ROWS], &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("media.other"), "{error}");
        assert!(keys(&table("[opaque]\nanything = 1\n"), &[ROWS], &["opaque"]).is_ok());
        assert!(keys(&table("[opaque]\nanything = 1\n"), &[ROWS], &[]).is_err());
    }

    /// Every field of the shared config structs has a row, and every row
    /// names a field, so a new optional key cannot ship without one.
    #[test]
    fn every_shared_config_key_has_a_row() {
        fn check(rows: &[&[Row]], prefix: &str, fields: &[&str]) -> Vec<String> {
            let mut paths = Vec::new();
            for field in fields {
                let path = if prefix.is_empty() {
                    (*field).to_owned()
                } else {
                    format!("{prefix}.{field}")
                };
                assert!(lookup(rows, &path).is_some(), "no row for {path}");
                paths.push(path);
            }
            paths
        }
        let mut set = Vec::new();
        set.extend(check(
            &[MEDIA_SET_KEYS, EXTRA_KEYS],
            "",
            struct_fields::<media_set::Config>(),
        ));
        set.extend(check(
            &[MEDIA_SET_KEYS],
            "media",
            struct_fields::<media_set::Medium>(),
        ));
        set.extend(check(
            &[EXTRA_KEYS],
            "extras",
            struct_fields::<media_set::Extra>(),
        ));
        let mut package = Vec::new();
        for (prefix, fields) in [
            ("", struct_fields::<firmware::Config>()),
            ("roms", struct_fields::<firmware::Rom>()),
            ("roms.parts", struct_fields::<firmware::Part>()),
            ("provenance", struct_fields::<firmware::Provenance>()),
            ("sources", struct_fields::<firmware::Source>()),
            ("readme", struct_fields::<firmware::Readme>()),
            ("extras", struct_fields::<media_set::Extra>()),
        ] {
            package.extend(check(&[FIRMWARE_KEYS, EXTRA_KEYS], prefix, fields));
        }
        for (rows, paths) in [
            (&[MEDIA_SET_KEYS, EXTRA_KEYS][..], &set),
            (&[FIRMWARE_KEYS, EXTRA_KEYS][..], &package),
        ] {
            for (row, _) in rows.iter().flat_map(|rows| rows.iter()) {
                assert!(paths.iter().any(|p| p == row), "row {row} names no field");
            }
        }
    }

    /// Every field of a title record has a row and every row names a field,
    /// so a new catalogue field cannot ship without deciding its schema.
    #[test]
    fn every_title_field_has_a_row() {
        let fields = struct_fields::<crate::metadata::Title>();
        for field in fields {
            assert!(
                lookup(&[TITLE_FIELDS], field).is_some(),
                "no row for {field}"
            );
        }
        for (row, _) in TITLE_FIELDS {
            assert!(fields.contains(row), "row {row} names no field");
        }
        assert_eq!(fields.len(), TITLE_FIELDS.len());
    }

    /// Schema 2 is the next release's: 0.0.2 shows it read-only, this build
    /// reads it, and a title needs it only for a field schema 1 lacks.
    #[test]
    fn metadata_schema_two_is_read_from_0_0_3_and_needed_only_by_its_fields() {
        let previous = AppVersion::new(0, 0, 2);
        assert_eq!(metadata_schema(1), Some(BASELINE));
        assert_eq!(metadata_schema(2), Some(AppVersion::new(0, 0, 3)));
        assert_eq!(metadata_schema(crate::metadata::SCHEMA + 1), None);
        assert!(Requirement::Release(metadata_schema(1).unwrap()).met_by(previous));
        assert!(!Requirement::Release(metadata_schema(2).unwrap()).met_by(previous));
        assert!(
            Requirement::Release(metadata_schema(crate::metadata::SCHEMA).unwrap()).met_by(level())
        );
        assert_eq!(metadata_schema_for([]).unwrap(), 1);
        for (field, since) in TITLE_FIELDS {
            let schema = metadata_schema_for(["id", "title", field]).unwrap();
            assert_eq!(schema, if *since == BASELINE { 1 } else { 2 }, "{field}");
        }
        assert_eq!(
            metadata_schema_for(["title", "needs", "work_ids"]).unwrap(),
            2
        );
        assert_eq!(
            metadata_schema_for(["title", "future"])
                .unwrap_err()
                .to_string(),
            "invalid image: no compatibility row for title field future"
        );
    }

    #[test]
    fn every_shared_payload_and_artifact_type_has_a_row() {
        for kind in crate::codec::MEDIA_PAYLOAD_TYPES
            .iter()
            .chain(crate::codec::FIRMWARE_PAYLOAD_TYPES)
        {
            assert!(payload(kind.media_type).is_some(), "{}", kind.media_type);
        }
        for kind in [
            media_type::MACHINE,
            media_type::MEDIA_SET,
            media_type::FIRMWARE,
        ] {
            assert!(artifact(kind).is_some(), "{kind}");
        }
        assert!(payload("application/vnd.unemu.future.v1").is_none());
        let rows = NEUTRAL_TYPES
            .iter()
            .map(|(_, _, since)| *since)
            .chain(COMPRESSED.iter().map(|(_, since)| *since))
            .chain(
                [
                    MEDIA_SET_KEYS,
                    EXTRA_KEYS,
                    FIRMWARE_KEYS,
                    REDISTRIBUTION,
                    PLATFORMS,
                    METADATA_SCHEMAS,
                    TITLE_FIELDS,
                ]
                .iter()
                .flat_map(|rows| rows.iter().map(|(_, since)| *since)),
            )
            .chain({
                use firmware::{ArchiveFormat, ProvenanceKind};
                use media_set::ExtraKind;
                let kinds = [
                    ExtraKind::Document,
                    ExtraKind::Archive,
                    ExtraKind::Executable,
                    ExtraKind::Other,
                ]
                .map(extra_kind);
                let provenance = [
                    ProvenanceKind::Dumped,
                    ProvenanceKind::Downloaded,
                    ProvenanceKind::Built,
                ]
                .map(provenance_kind);
                let formats = [ArchiveFormat::Tar, ArchiveFormat::Zip].map(archive_format);
                kinds.into_iter().chain(provenance).chain(formats)
            });
        for since in rows {
            assert!(since <= NEWEST_ROW, "a row names {since}; raise NEWEST_ROW");
        }
    }

    /// Resolving an image a newer release tagged in a shared store refuses
    /// it with the release it needs before the config is decoded; without
    /// the marker the decoder's own refusal stays.
    #[test]
    fn resolve_refuses_an_unmet_image_before_decoding_its_config() {
        struct Strict;
        impl crate::ConfigValidator for Strict {
            fn platform(&self) -> &str {
                "synthetic"
            }
            fn validate(
                &self,
                table: &toml::Table,
                _: &[Descriptor],
                _: &dyn crate::BlobSource,
            ) -> Result<Vec<Digest>> {
                match table
                    .keys()
                    .find(|k| !matches!(k.as_str(), "schema" | "platform"))
                {
                    Some(key) => Err(invalid(format!("unknown field `{key}`"))),
                    None => Ok(Vec::new()),
                }
            }
        }
        let root = std::env::temp_dir().join(format!(
            "image-store-compat-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let store = crate::Store::open(&root).unwrap().with_compatibility(
            Compatibility::new(level(), NEWEST_ROW).with_core("synthetic", level(), BASELINE),
        );
        let put = |kind: &str, bytes: &[u8]| {
            let blob = store.import(bytes, None).unwrap();
            Descriptor::new(kind, blob.digest, blob.size)
        };
        let config = put(
            media_type::CONFIG,
            b"schema = 1\nplatform = \"synthetic\"\nfuture_option = 1\n",
        );
        let metadata = put(
            media_type::METADATA,
            br#"{"schema":1,"platform":"synthetic","titles":[]}"#,
        );
        let image = |marker: Option<&str>| {
            let mut manifest =
                Manifest::new("synthetic", config.clone(), vec![metadata.clone()]).unwrap();
            if let Some(marker) = marker {
                manifest
                    .annotations
                    .insert(MIN_APP_VERSION.into(), marker.into());
            }
            store.import_manifest(&manifest).unwrap()
        };
        let marked = image(Some("99.0.0"));
        // This release refuses to tag what it cannot validate.
        let refused = store
            .update_tag("newer:1", &marked, None, &Strict)
            .unwrap_err();
        assert!(matches!(refused, Error::NeedsNewer { .. }), "{refused}");
        store.tag_as_newer_release("newer:1", &marked).unwrap();
        let refused = store.resolve("newer:1", &Strict).unwrap_err().to_string();
        assert_eq!(
            refused,
            format!(
                "newer:1 needs unemupc 99.0.0 or newer (this is {})",
                level()
            )
        );
        // A machine keeps its image by digest alone; the refusal of its
        // Start still names the tag that names the image.
        let bare = Descriptor::new(media_type::MANIFEST, marked.digest.clone(), marked.size);
        assert_eq!(
            store
                .resolve_descriptor(&bare, &Strict)
                .unwrap_err()
                .to_string(),
            refused
        );
        let unmarked = image(None);
        store.tag_as_newer_release("unmarked:1", &unmarked).unwrap();
        let refused = store
            .resolve("unmarked:1", &Strict)
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("unknown field `future_option`"),
            "{refused}"
        );
        let invalid = image(Some("1.0"));
        let refused = store
            .resolve_descriptor(&invalid, &Strict)
            .unwrap_err()
            .to_string();
        assert_eq!(
            refused,
            format!(
                "{} needs a newer unemupc (this is {})",
                invalid.digest,
                level()
            )
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Compression rates the manifest it writes with the provider's tables
    /// and keeps the higher of that and the source's marker.
    #[test]
    fn compression_rates_the_manifest_it_writes() {
        use crate::ConfigValidator as _;
        struct Rated;
        impl crate::ConfigValidator for Rated {
            fn platform(&self) -> &str {
                "synthetic"
            }
            fn payload_types(&self) -> &[crate::PayloadType] {
                &[crate::PayloadType {
                    media_type: "application/vnd.unemu.firmware.v1+zstd",
                    max_size: 1 << 20,
                }]
            }
            fn validate(
                &self,
                table: &toml::Table,
                _: &[Descriptor],
                _: &dyn crate::BlobSource,
            ) -> Result<Vec<Digest>> {
                table["blobs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| v.as_str().unwrap_or_default().parse())
                    .collect()
            }
            fn required_release(&self, _: &Manifest, _: &[u8]) -> Result<AppVersion> {
                Ok(PACKAGE)
            }
        }
        let root = std::env::temp_dir().join(format!(
            "image-store-compat-compress-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let store = crate::Store::open(&root).unwrap().with_compatibility(
            Compatibility::new(level(), NEWEST_ROW).with_core("synthetic", level(), BASELINE),
        );
        let put = |kind: &str, bytes: &[u8]| {
            let blob = store.import(bytes, None).unwrap();
            Descriptor::new(kind, blob.digest, blob.size)
        };
        let rom = put(media_type::FIRMWARE, &[0; 8192]);
        let config = put(
            media_type::CONFIG,
            format!(
                "schema = 1\nplatform = \"synthetic\"\nblobs = [\"{}\"]\n",
                rom.digest
            )
            .as_bytes(),
        );
        let metadata = put(
            media_type::METADATA,
            br#"{"schema":1,"platform":"synthetic","titles":[]}"#,
        );
        let manifest = Manifest::new_with_types(
            "synthetic",
            config,
            vec![rom, metadata],
            Rated.payload_types(),
        )
        .unwrap();
        let source = store
            .import_manifest_with_types(&manifest, Rated.payload_types())
            .unwrap();
        let compressed = store.compress_manifest(&source, &Rated).unwrap();
        let bytes = crate::BlobSource::read_verified(&store, &compressed, MAX_SMALL_BLOB).unwrap();
        let written = Manifest::decode_with_types(&bytes, Rated.payload_types()).unwrap();
        assert!(written.layers[0].is_compressed());
        assert_eq!(Requirement::of(&written), Requirement::Release(PACKAGE));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn shared_rows_rate_a_set_and_refuse_an_unknown_artifact_type() {
        let config = media_set::Config {
            schema: 1,
            platform: "synthetic".into(),
            media: vec![media_set::Medium {
                id: "disk-1".into(),
                label: "Disk 1".into(),
                format: toml::Table::from_iter([("kind".into(), "provider-owned".into())]),
                blob: Digest::of(b"disk"),
            }],
            extras: Vec::new(),
        };
        let bytes = config.encode().unwrap();
        let manifest = Manifest::new_media_set(
            "synthetic",
            Descriptor::new(
                media_type::MEDIA_SET_CONFIG,
                Digest::of(&bytes),
                bytes.len() as u64,
            ),
            vec![
                Descriptor::new(media_type::FLOPPY, Digest::of(b"disk"), 4),
                Descriptor::new(media_type::METADATA, Digest::of(b"metadata"), 8),
            ],
        )
        .unwrap();
        assert_eq!(
            shared_frontend_release(&manifest, &bytes).unwrap(),
            BASELINE
        );
        assert_eq!(envelope(&manifest, &[]).unwrap(), BASELINE);
        let mut future = machine();
        future.layers.push(Descriptor::new(
            "application/vnd.unemu.future.v1",
            Digest::of(b"future"),
            6,
        ));
        assert!(envelope(&future, &[]).is_err());
        assert_eq!(
            envelope(
                &future,
                &[("application/vnd.unemu.future.v1", NEXT_RELEASE)]
            )
            .unwrap(),
            NEXT_RELEASE
        );
    }
}
