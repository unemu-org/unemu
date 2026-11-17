// SPDX-License-Identifier: AGPL-3.0-or-later
//! Compatibility of published images: the oldest release that reads and runs
//! an artifact; machine-images §5.6.
//!
//! The manifest annotation [`MIN_APP_VERSION`] names that release as a
//! Semantic Versioning 2.0.0 normal version (§2: `MAJOR.MINOR.PATCH`, with no
//! pre-release or build part). It is a hint that lets a release refuse early,
//! with a reason a person understands, never an authority: every codec stays
//! strict and fails closed. No annotation means no requirement, so an artifact
//! that older releases already read keeps its bytes and digest. The marker is
//! not part of any config, so a corrected marker never changes a machine's
//! identity.
//!
//! A per-feature table records, for every feature a reader reads, the first
//! release that reads it: artifact and payload media types, config keys,
//! vocabulary values, firmware catalogue entries and accepted ranges. This
//! module holds the shared rows (the envelope's media types and the keys and
//! values of installation-set and firmware-package configs); the platform
//! provider holds its own. Release 0.0.1 is the baseline: everything it reads
//! has [`BASELINE`]. Writers stamp the maximum over what an artifact uses
//! ([`stamp`]) and leave the annotation out at the baseline. Readers check the
//! marker leniently before any strict decode ([`Requirement::read`]), so even
//! a future envelope gets the message.

use crate::{
    artifact::NEUTRAL_TYPES, firmware, invalid, media_set, media_type, Error, Manifest, Result,
    MAX_SMALL_BLOB,
};
use serde::Deserialize;
use std::{fmt, str::FromStr};

/// Manifest annotation: the oldest release of the program that provides the
/// manifest's `unemu.platform` and reads and runs the artifact.
pub const MIN_APP_VERSION: &str = "unemu.min-app-version";
/// The program the version numbers name. The desktop program and the command
/// line share one version.
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
/// The release after the newest published one (0.0.2): the version a row for
/// a feature that release does not read takes. A row writes it as a literal,
/// `AppVersion::new(0, 0, 3)`, never as this constant, so it keeps naming 0.0.3
/// after 0.0.3 ships. The release commit raises this constant past the new
/// package version; a test refuses a value that is not above it.
pub const NEXT_RELEASE: AppVersion = AppVersion::new(0, 0, 3);
/// The newest version any row of this build's tables names, shared or
/// provider-owned. A provider's tests pin it, so a development build that
/// adds a row for the next release reads what it writes.
pub const NEWEST_ROW: AppVersion = BASELINE;
/// This build's package version.
pub const PACKAGE: AppVersion = AppVersion::new(
    decimal(env!("CARGO_PKG_VERSION_MAJOR")),
    decimal(env!("CARGO_PKG_VERSION_MINOR")),
    decimal(env!("CARGO_PKG_VERSION_PATCH")),
);

const fn decimal(text: &str) -> u64 {
    let bytes = text.as_bytes();
    let mut value = 0;
    let mut index = 0;
    while index < bytes.len() {
        value = value * 10 + (bytes[index] - b'0') as u64;
        index += 1;
    }
    value
}

/// The release level this build reads: its package version, or the newest
/// version its tables name when that is higher.
pub fn level() -> AppVersion {
    if NEWEST_ROW > PACKAGE {
        NEWEST_ROW
    } else {
        PACKAGE
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
            }) => match annotations.get(MIN_APP_VERSION) {
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

    fn parse(text: &str) -> Self {
        text.parse()
            .map_or_else(|_| Self::Invalid(text.into()), Self::Release)
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
    pub fn check(&self, reference: &str) -> Result<()> {
        let level = level();
        if self.met_by(level) {
            return Ok(());
        }
        Err(Error::NeedsNewer {
            reference: reference.into(),
            required: match self {
                Self::Release(required) => Some(*required),
                _ => None,
            },
            level,
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
    match required {
        Some(required) => format!("Needs {PROGRAM} {required} or newer"),
        None => format!("Needs a newer {PROGRAM}"),
    }
}

/// The message of [`Error::NeedsNewer`] after the reference.
pub(crate) fn needs_newer(required: &Option<AppVersion>, level: &AppVersion) -> String {
    match required {
        Some(required) => format!("needs {PROGRAM} {required} or newer (this is {level})"),
        None => format!("needs a newer {PROGRAM} (this is {level})"),
    }
}

/// Write the marker: `required` from the tables and from what the artifact
/// inherits, raised to `stated` when a recipe or `--min-app-version` states a
/// higher one, and left out at the baseline. A stated value below `required`
/// is refused, naming both, and so is one above this build's level, which
/// could not read the artifact then.
pub fn stamp(
    manifest: &mut Manifest,
    required: AppVersion,
    stated: Option<AppVersion>,
) -> Result<()> {
    set(manifest, raised(required, stated)?);
    Ok(())
}

/// The marker [`stamp`] writes: `required` raised to `stated`. A stated value
/// below `required` is refused, naming both, and so is one above this build's
/// level.
pub fn raised(required: AppVersion, stated: Option<AppVersion>) -> Result<AppVersion> {
    within_level(required)?;
    let Some(stated) = stated else {
        return Ok(required);
    };
    if stated < required {
        return Err(invalid(format!(
            "the stated minimum version {stated} is lower than {required}, the oldest {PROGRAM} that reads this artifact"
        )));
    }
    let level = level();
    if stated > level {
        return Err(invalid(format!(
            "the stated minimum version {stated} is newer than this {PROGRAM} ({level}), which could not read the artifact"
        )));
    }
    Ok(stated)
}

/// Keep the marker of the artifact `manifest` derives from as a floor: a
/// derived machine needs at least what its source needed, such as an
/// emulation fix stated at import that the tables cannot see.
pub fn carry(manifest: &mut Manifest, source: &Manifest) -> Result<()> {
    floor(manifest, Requirement::of(source).release()?)
}

/// Raise the marker to at least `release`, keeping a higher one the manifest
/// already carries, such as one copied from the artifact it was made from.
pub fn floor(manifest: &mut Manifest, release: AppVersion) -> Result<()> {
    within_level(release)?;
    let own = Requirement::of(manifest).release()?;
    set(manifest, own.max(release));
    Ok(())
}

/// A computed requirement above this build's level means a table row names
/// a release after the package version that [`NEWEST_ROW`] does not cover:
/// a mistake in this build, never in the artifact.
fn within_level(required: AppVersion) -> Result<()> {
    let level = level();
    if required > level {
        return Err(invalid(format!(
            "a compatibility table row names {required}, newer than this build's level {level}; raise image_store::compat::NEWEST_ROW (or the package version) to it"
        )));
    }
    Ok(())
}

fn set(manifest: &mut Manifest, release: AppVersion) {
    if release > BASELINE {
        manifest
            .annotations
            .insert(MIN_APP_VERSION.into(), release.to_string());
    } else {
        manifest.annotations.remove(MIN_APP_VERSION);
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

/// The manifest artifact types the envelope reads.
pub fn artifact(kind: &str) -> Option<AppVersion> {
    match kind {
        media_type::MACHINE | media_type::MEDIA_SET | media_type::FIRMWARE => Some(BASELINE),
        _ => None,
    }
}

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
        .or_else(|| lookup(&[COMPRESSED], kind))
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

/// The newest release the manifest's artifact type and every payload media
/// type need; `provider` holds the rows of provider-declared types. A media
/// type without a row is refused.
pub fn envelope(manifest: &Manifest, provider: &[Row]) -> Result<AppVersion> {
    let mut release = artifact(manifest.artifact_type()).ok_or_else(|| {
        invalid(format!(
            "no compatibility row for artifact type {}",
            manifest.artifact_type()
        ))
    })?;
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

/// What the shared rows alone say an artifact needs: its envelope, and for an
/// installation set or a firmware package the keys and values of its config
/// that the store owns. Provider-declared payload types and the provider's
/// tables are left to the provider, which overrides
/// [`crate::ConfigValidator::required_release`] with its own rows.
pub fn shared_release(manifest: &Manifest, config: &[u8]) -> Result<AppVersion> {
    let mut release = artifact(manifest.artifact_type()).ok_or_else(|| {
        invalid(format!(
            "no compatibility row for artifact type {}",
            manifest.artifact_type()
        ))
    })?;
    for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
        if let Some(since) = payload(&descriptor.media_type) {
            release = release.max(since);
        }
    }
    release = release.max(redistribution(manifest)?);
    let table = || -> Result<toml::Table> {
        toml::from_str(std::str::from_utf8(config).map_err(|e| invalid(e.to_string()))?)
            .map_err(|e| invalid(e.to_string()))
    };
    match manifest.artifact_type() {
        media_type::MEDIA_SET => {
            release = release
                .max(keys(
                    &table()?,
                    &[MEDIA_SET_KEYS, EXTRA_KEYS],
                    &["media.format"],
                )?)
                .max(media_set_values(&media_set::Config::decode(config)?));
        }
        media_type::FIRMWARE => {
            release = release
                .max(keys(&table()?, &[FIRMWARE_KEYS, EXTRA_KEYS], &["fits"])?)
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
    use crate::{Descriptor, Digest};

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
        assert!(Requirement::Release(this).check("x:1").is_ok());
        let higher = AppVersion::new(this.major, this.minor, this.patch + 1);
        assert_eq!(
            Requirement::Release(higher)
                .check("x:1")
                .unwrap_err()
                .to_string(),
            format!("x:1 needs unemupc {higher} or newer (this is {this})")
        );
        assert_eq!(
            Requirement::Invalid("soon".into())
                .check("x:1")
                .unwrap_err()
                .to_string(),
            format!("x:1 needs a newer unemupc (this is {this})")
        );
        assert_eq!(
            needs(Some(higher)),
            format!("Needs unemupc {higher} or newer")
        );
        assert_eq!(needs(None), "Needs a newer unemupc");
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
        let future = br#"{"schemaVersion":3,"artifactType":"application/vnd.unemu.machine.v9","future":{"x":1},"annotations":{"unemu.platform":"ibm-pc-at","unemu.min-app-version":"7.1.0"}}"#;
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

        // A build whose tables rate an artifact 0.0.3 writes 0.0.3. This
        // build's level is lower, so writing it is the build's own mistake.
        let mut next = machine();
        set(&mut next, NEXT_RELEASE);
        assert_eq!(next.annotations[MIN_APP_VERSION], "0.0.3");
        let mistake = stamp(&mut machine(), NEXT_RELEASE, None)
            .unwrap_err()
            .to_string();
        assert!(mistake.contains("NEWEST_ROW"), "{mistake}");
        let mistake = floor(&mut machine(), NEXT_RELEASE).unwrap_err().to_string();
        assert!(mistake.contains("NEWEST_ROW"), "{mistake}");

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
                [MEDIA_SET_KEYS, EXTRA_KEYS, FIRMWARE_KEYS, REDISTRIBUTION]
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
        let store = crate::Store::open(&root).unwrap();
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
        let store = crate::Store::open(&root).unwrap();
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
        assert_eq!(shared_release(&manifest, &bytes).unwrap(), BASELINE);
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
