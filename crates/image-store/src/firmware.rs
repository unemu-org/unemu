// SPDX-License-Identifier: AGPL-3.0-or-later
//! Neutral firmware package bindings and provider validation; machine-images §5.3.
//! OCI Image Manifest 1.1 supplies closure identity; providers own ROM semantics.
//! Descriptive fields, source archives and the README are plain TOML, tar or
//! zip and UTF-8 Markdown, readable without this program.

use crate::{
    invalid,
    media_set::{self, Extra},
    media_type, BlobSource, ConfigValidator, Descriptor, Digest, Result, MAX_OBJECTS,
    MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub blob: Digest,
    pub sha256: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_number: Option<String>,
    pub lane: String,
    #[serde(default)]
    pub offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rom {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalogue_id: Option<String>,
    pub sha256: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_number: Option<String>,
    /// Provider device kinds the ROM needs, such as the card a video BIOS drives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
}

/// How the package's bytes were obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProvenanceKind {
    /// Read from chips; `from` names the board and the chip markings.
    Dumped,
    /// Downloaded; `from` names the source and file.
    Downloaded,
    /// Built; `from` names the repository, revision and patches.
    Built,
}
impl ProvenanceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dumped => "dumped",
            Self::Downloaded => "downloaded",
            Self::Built => "built",
        }
    }
}

/// Where the bytes came from and when. The date is supplied, never the host clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProvenanceKind>,
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Container format of a source archive layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveFormat {
    Tar,
    Zip,
}
impl ArchiveFormat {
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Tar => media_type::TAR,
            Self::Zip => media_type::ZIP,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tar => "tar",
            Self::Zip => "zip",
        }
    }
}

/// The corresponding source of one or more logical ROMs, as its own layer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub blob: Digest,
    pub filename: String,
    pub size: u64,
    pub format: ArchiveFormat,
    /// Logical ROM ids whose source this archive is.
    pub roms: Vec<String>,
}

/// The generated plain-text README layer (UTF-8 Markdown).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Readme {
    pub blob: Digest,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub platform: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Free notes and known quirks; several lines are allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Free labels in the media id grammar, such as `open-source` or `xt-ide`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default)]
    pub fits: toml::Table,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    pub roms: Vec<Rom>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub extras: Vec<Extra>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme: Option<Readme>,
}
impl Config {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("firmware config exceeds 1 MiB"));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?;
        let config: Self = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = toml::to_string(self)
            .map_err(|e| invalid(e.to_string()))?
            .into_bytes();
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("firmware config exceeds 1 MiB"));
        }
        Ok(bytes)
    }
    fn validate(&self) -> Result<()> {
        if self.schema != 1
            || !crate::artifact::valid_platform(&self.platform)
            || !media_set::valid_text(&self.name)
            || self.roms.is_empty()
        {
            return Err(invalid("invalid firmware envelope"));
        }
        for text in [&self.vendor, &self.version].into_iter().flatten() {
            if !media_set::valid_text(text) {
                return Err(invalid("invalid firmware text"));
            }
        }
        for text in [&self.notes, &self.description].into_iter().flatten() {
            if !valid_long_text(text) {
                return Err(invalid("invalid firmware notes or description"));
            }
        }
        if self.date.as_ref().is_some_and(|s| !valid_date(s)) {
            return Err(invalid("invalid firmware calendar date"));
        }
        ids_list(&self.labels, "label")?;
        if let Some(p) = &self.provenance {
            if !valid_long_text(&p.from)
                || [&p.revision, &p.url]
                    .into_iter()
                    .flatten()
                    .any(|s| !valid_line(s))
                || p.date.as_ref().is_some_and(|s| !valid_date(s))
            {
                return Err(invalid("invalid firmware provenance"));
            }
        }
        let mut ids = BTreeSet::new();
        let mut count = self.extras.len() + self.sources.len() + usize::from(self.readme.is_some());
        for rom in &self.roms {
            if !media_set::valid_id(&rom.id)
                || !ids.insert(&rom.id)
                || !media_set::valid_id(&rom.role)
                || rom
                    .catalogue_id
                    .as_ref()
                    .is_some_and(|s| !media_set::valid_id(s))
                || rom.blob.is_some() != rom.parts.is_empty()
            {
                return Err(invalid("invalid firmware ROM id, role or binding"));
            }
            identity(rom.blob.as_ref(), &rom.sha256, rom.size)?;
            labels(&rom.position, &rom.part_number)?;
            ids_list(&rom.requires, "required device kind")?;
            if !rom.parts.is_empty() && (rom.position.is_some() || rom.part_number.is_some()) {
                return Err(invalid("logical split ROM cannot name a socket"));
            }
            count = count.saturating_add(rom.parts.len().max(1));
            for part in &rom.parts {
                identity(Some(&part.blob), &part.sha256, part.size)?;
                labels(&part.position, &part.part_number)?;
                if !media_set::valid_id(&part.lane) || part.offset > i64::MAX as u64 {
                    return Err(invalid("invalid firmware part placement"));
                }
            }
        }
        if count > MAX_OBJECTS - 2 {
            return Err(invalid("firmware graph exceeds 10,000 objects"));
        }
        let mut covered = BTreeSet::new();
        for source in &self.sources {
            if !media_set::valid_text(&source.filename)
                || source.filename.contains(['/', '\\', '\0'])
                || matches!(source.filename.as_str(), "." | "..")
                || source.size == 0
                || source.size > i64::MAX as u64
                || source.roms.is_empty()
                || source
                    .roms
                    .iter()
                    .any(|id| !ids.contains(id) || !covered.insert(id))
            {
                return Err(invalid(
                    "invalid source archive: name, size, or ROM ids (each ROM has one source)",
                ));
            }
        }
        if self
            .readme
            .as_ref()
            .is_some_and(|r| r.size == 0 || r.size > MAX_SMALL_BLOB)
        {
            return Err(invalid("firmware README must be 1 byte to 1 MiB"));
        }
        media_set::validate_extras(&self.extras)
    }
    /// The source archive that is the corresponding source of ROM `id`.
    pub fn source_of(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.roms.iter().any(|r| r == id))
    }
    /// Exact payload traversal, retaining declared chip order.
    pub fn payloads(&self) -> Vec<(Digest, u64, &'static str)> {
        let mut result = Vec::new();
        for rom in &self.roms {
            if let Some(blob) = &rom.blob {
                result.push((blob.clone(), rom.size, media_type::FIRMWARE));
            }
            for p in &rom.parts {
                result.push((p.blob.clone(), p.size, media_type::FIRMWARE));
            }
        }
        result.extend(
            self.sources
                .iter()
                .map(|s| (s.blob.clone(), s.size, s.format.media_type())),
        );
        result.extend(
            self.extras
                .iter()
                .map(|e| (e.blob.clone(), e.size, media_type::EXTRA)),
        );
        result.extend(
            self.readme
                .iter()
                .map(|r| (r.blob.clone(), r.size, media_type::MARKDOWN)),
        );
        result
    }
}
fn identity(blob: Option<&Digest>, hash: &str, size: u64) -> Result<()> {
    let digest: Digest = format!("sha256:{hash}").parse()?;
    if size == 0 || size > i64::MAX as u64 || blob.is_some_and(|d| d != &digest) {
        return Err(invalid("firmware digest or size mismatch"));
    }
    Ok(())
}
/// Longer descriptive text: several LF-separated lines, no other controls.
pub fn valid_long_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= 4096
        && !text
            .chars()
            .any(|c| (c <= '\u{1f}' && c != '\n') || c == '\u{7f}')
}
/// One line of up to 1024 characters, for revisions and addresses.
fn valid_line(text: &str) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= 1024
        && !text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}
fn ids_list(items: &[String], what: &str) -> Result<()> {
    let mut unique = BTreeSet::new();
    if items.len() > 64
        || items
            .iter()
            .any(|s| !media_set::valid_id(s) || !unique.insert(s))
    {
        return Err(invalid(format!("invalid or duplicate firmware {what}")));
    }
    Ok(())
}
fn labels(position: &Option<String>, number: &Option<String>) -> Result<()> {
    if [position, number]
        .into_iter()
        .flatten()
        .any(|s| !media_set::valid_text(s))
    {
        return Err(invalid("invalid chip label"));
    }
    Ok(())
}
/// An ISO 8601 calendar date, `YYYY-MM-DD`, that exists.
pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, b)| i != 4 && i != 7 && !b.is_ascii_digit())
    {
        return false;
    }
    let year = s[..4].parse::<u32>().unwrap_or(0);
    let month = s[5..7].parse::<usize>().unwrap_or(0);
    let day = s[8..].parse::<u32>().unwrap_or(0);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        0,
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    year > 0 && (1..=12).contains(&month) && day > 0 && day <= days[month]
}

/// Provider-owned vocabulary, layout and catalogue checks; machine-images §5.3.
pub trait FirmwareRules {
    fn newest_frontend_row(&self) -> crate::compat::AppVersion {
        crate::compat::NEWEST_ROW
    }

    fn core_release(&self) -> Option<(crate::compat::AppVersion, crate::compat::AppVersion)> {
        None
    }
    fn required_releases(
        &self,
        manifest: &crate::Manifest,
        config: &[u8],
    ) -> Result<crate::compat::Releases> {
        Ok(crate::compat::Releases {
            core: crate::compat::BASELINE,
            frontend: crate::compat::shared_frontend_release(manifest, config)?,
        })
    }

    fn platform(&self) -> &str;
    fn validate(
        &self,
        config: &Config,
        blobs: &dyn BlobSource,
        descriptors: &[Descriptor],
    ) -> Result<()>;
    fn validate_name(&self, name: &str) -> Result<()>;
}
pub struct Validator<'a>(pub &'a dyn FirmwareRules);
impl ConfigValidator for Validator<'_> {
    fn required_release(
        &self,
        manifest: &crate::Manifest,
        config: &[u8],
    ) -> Result<crate::compat::AppVersion> {
        Ok(self.required_releases(manifest, config)?.core)
    }

    fn newest_frontend_row(&self) -> crate::compat::AppVersion {
        self.0.newest_frontend_row()
    }

    fn core_release(&self) -> Option<(crate::compat::AppVersion, crate::compat::AppVersion)> {
        self.0.core_release()
    }
    fn required_releases(
        &self,
        manifest: &crate::Manifest,
        config: &[u8],
    ) -> Result<crate::compat::Releases> {
        self.0.required_releases(manifest, config)
    }

    fn artifact_type(&self) -> &str {
        media_type::FIRMWARE
    }
    fn platform(&self) -> &str {
        self.0.platform()
    }
    fn payload_types(&self) -> &[crate::PayloadType] {
        crate::codec::FIRMWARE_PAYLOAD_TYPES
    }
    fn validate(
        &self,
        table: &toml::Table,
        descriptors: &[Descriptor],
        blobs: &dyn BlobSource,
    ) -> Result<Vec<Digest>> {
        let config: Config = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| invalid(e.to_string()))?;
        config.validate()?;
        if !crate::platform::same(&config.platform, self.platform()) {
            return Err(invalid("firmware platform mismatch"));
        }
        let mut references = Vec::new();
        // Bindings name decoded content, so a ROM matches raw or `+zstd`.
        for (digest, size, kind) in config.payloads() {
            let descriptor = descriptors
                .iter()
                .find(|d| d.names(&digest))
                .ok_or_else(|| invalid("missing firmware payload"))?;
            if descriptor.decoded_size()? != size || descriptor.decoded_media_type() != kind {
                return Err(invalid("firmware payload size or type mismatch"));
            }
            references.push(digest);
        }
        // The push guard reads the ROM-to-source link from descriptors, so it
        // must agree with the config.
        for rom in &config.roms {
            let link = config.source_of(&rom.id).map(|s| s.blob.to_string());
            for digest in rom.blob.iter().chain(rom.parts.iter().map(|p| &p.blob)) {
                let descriptor = descriptors
                    .iter()
                    .find(|d| d.names(digest))
                    .ok_or_else(|| invalid("missing firmware payload"))?;
                if descriptor.annotations.get(crate::license::SOURCE) != link.as_ref() {
                    return Err(invalid("ROM source link differs from the config"));
                }
            }
        }
        if let Some(readme) = &config.readme {
            let descriptor = descriptors
                .iter()
                .find(|d| d.names(&readme.blob))
                .ok_or_else(|| invalid("missing firmware README"))?;
            let bytes = blobs.read_verified(descriptor, MAX_SMALL_BLOB)?;
            if std::str::from_utf8(&bytes).is_err() {
                return Err(invalid("firmware README is not UTF-8"));
            }
        }
        for e in &config.extras {
            if let Some(name) = &e.dos_name {
                self.0.validate_name(name)?;
            }
        }
        self.0.validate(&config, blobs, descriptors)?;
        Ok(references)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Config {
        let blob = Digest::of(b"synthetic");
        Config {
            schema: 1,
            platform: "synthetic".into(),
            name: "Synthetic firmware".into(),
            vendor: None,
            version: None,
            date: Some("2000-02-29".into()),
            notes: None,
            description: None,
            labels: Vec::new(),
            fits: toml::Table::new(),
            provenance: None,
            roms: vec![Rom {
                id: "boot".into(),
                role: "reset".into(),
                catalogue_id: None,
                sha256: blob.hex().into(),
                size: 9,
                blob: Some(blob),
                position: None,
                part_number: None,
                requires: Vec::new(),
                parts: Vec::new(),
            }],
            sources: Vec::new(),
            extras: Vec::new(),
            readme: None,
        }
    }
    fn described() -> Config {
        let mut config = sample();
        config.notes = Some("First line.\nSecond line.".into());
        config.description = Some("A synthetic boot ROM.".into());
        config.labels = vec!["open-source".into(), "synthetic".into()];
        config.provenance = Some(Provenance {
            kind: Some(ProvenanceKind::Built),
            from: "example/repository at a fixed revision".into(),
            revision: Some("0123456789abcdef".into()),
            url: None,
            date: Some("2026-11-11".into()),
        });
        config.roms[0].requires = vec!["synthetic-bus".into()];
        config.sources.push(Source {
            blob: Digest::of(b"source tar"),
            filename: "synthetic-src.tar".into(),
            size: 10,
            format: ArchiveFormat::Tar,
            roms: vec!["boot".into()],
        });
        config.readme = Some(Readme {
            blob: Digest::of(b"# Synthetic"),
            size: 11,
        });
        config
    }
    #[test]
    fn firmware_descriptive_fields_sources_and_readme_round_trip_in_traversal_order() {
        let config = described();
        let bytes = config.encode().unwrap();
        assert_eq!(Config::decode(&bytes).unwrap(), config);
        assert_eq!(config.encode().unwrap(), bytes);
        let kinds = config
            .payloads()
            .into_iter()
            .map(|(_, _, kind)| kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [media_type::FIRMWARE, media_type::TAR, media_type::MARKDOWN]
        );
        assert_eq!(
            config.source_of("boot").unwrap().filename,
            "synthetic-src.tar"
        );
        // A package written before these fields existed still decodes.
        assert_eq!(
            Config::decode(&sample().encode().unwrap()).unwrap(),
            sample()
        );
        let mut bad = described();
        bad.labels.push("open-source".into());
        assert!(bad.encode().is_err(), "duplicate label");
        bad = described();
        bad.labels = vec!["Not An Id".into()];
        assert!(bad.encode().is_err());
        bad = described();
        bad.roms[0].requires = vec!["a".into(), "a".into()];
        assert!(bad.encode().is_err());
        bad = described();
        bad.sources[0].roms = vec!["missing".into()];
        assert!(bad.encode().is_err(), "source of an unknown ROM");
        bad = described();
        let second = bad.sources[0].clone();
        bad.sources.push(second);
        assert!(bad.encode().is_err(), "two sources for one ROM");
        bad = described();
        bad.sources[0].filename = "../src.tar".into();
        assert!(bad.encode().is_err());
        bad = described();
        bad.description = Some("bell\u{7}".into());
        assert!(bad.encode().is_err());
        bad = described();
        bad.provenance.as_mut().unwrap().date = Some("2026-02-30".into());
        assert!(bad.encode().is_err());
        bad = described();
        bad.readme.as_mut().unwrap().size = MAX_SMALL_BLOB + 1;
        assert!(bad.encode().is_err());
    }
    #[test]
    fn firmware_codec_round_trip_is_deterministic_and_platform_neutral() {
        let config = sample();
        let bytes = config.encode().unwrap();
        assert_eq!(Config::decode(&bytes).unwrap(), config);
        assert_eq!(config.encode().unwrap(), bytes);
        let descriptor = Descriptor::new(
            media_type::FIRMWARE_CONFIG,
            Digest::of(&bytes),
            bytes.len() as u64,
        );
        let payload = &config.roms[0];
        let manifest = crate::Manifest::new_firmware(
            "synthetic",
            descriptor,
            vec![Descriptor::new(
                media_type::FIRMWARE,
                payload.blob.clone().unwrap(),
                payload.size,
            )],
        )
        .unwrap();
        let bytes = manifest.encode().unwrap();
        assert_eq!(crate::Manifest::decode(&bytes).unwrap(), manifest);
        assert_eq!(manifest.encode().unwrap(), bytes);
    }
    #[test]
    fn firmware_codec_rejects_unknown_fields_versions_identity_and_calendar_errors() {
        let config = sample();
        let bytes = config.encode().unwrap();
        for date in [
            "2001-02-29",
            "2000-13-01",
            "0000-01-01",
            "2000-01-00",
            "2000-1-01",
            "éé-01-01",
        ] {
            let mut bad = config.clone();
            bad.date = Some(date.into());
            assert!(bad.encode().is_err(), "{date}");
        }
        for text in [
            format!(
                "unknown=true\n{}",
                String::from_utf8(bytes.clone()).unwrap()
            ),
            String::from_utf8(bytes.clone())
                .unwrap()
                .replace("schema = 1", "schema = 2"),
            String::from_utf8(bytes)
                .unwrap()
                .replace("role =", "unknown_role ="),
        ] {
            assert!(Config::decode(text.as_bytes()).is_err());
        }
        let mut bad = config.clone();
        bad.roms[0].sha256 = "00".repeat(32);
        assert!(bad.encode().is_err());
        bad = config.clone();
        bad.roms.push(bad.roms[0].clone());
        assert!(bad.encode().is_err());
        bad = config.clone();
        bad.roms[0].blob = None;
        assert!(bad.encode().is_err());
        bad = config.clone();
        bad.extras.push(Extra {
            blob: Digest::of(b"doc"),
            filename: "../manual".into(),
            size: 3,
            kind: media_set::ExtraKind::Document,
            dos_name: None,
        });
        assert!(bad.encode().is_err());
        assert!(Config::decode(&vec![b' '; MAX_SMALL_BLOB as usize + 1]).is_err());
    }
}
