// SPDX-License-Identifier: AGPL-3.0-or-later
//! Neutral firmware package bindings and provider validation; machine-images §5.3.
//! OCI Image Manifest 1.1 supplies closure identity; providers own ROM semantics.

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub fits: toml::Table,
    pub roms: Vec<Rom>,
    #[serde(default)]
    pub extras: Vec<Extra>,
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
        for text in [&self.vendor, &self.version, &self.notes]
            .into_iter()
            .flatten()
        {
            if !media_set::valid_text(text) {
                return Err(invalid("invalid firmware text"));
            }
        }
        if self.date.as_ref().is_some_and(|s| !valid_date(s)) {
            return Err(invalid("invalid firmware calendar date"));
        }
        let mut ids = BTreeSet::new();
        let mut count = self.extras.len();
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
        media_set::validate_extras(&self.extras)
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
            self.extras
                .iter()
                .map(|e| (e.blob.clone(), e.size, media_type::EXTRA)),
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
fn valid_date(s: &str) -> bool {
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
        if config.platform != self.platform() {
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
            fits: toml::Table::new(),
            roms: vec![Rom {
                id: "boot".into(),
                role: "reset".into(),
                catalogue_id: None,
                sha256: blob.hex().into(),
                size: 9,
                blob: Some(blob),
                position: None,
                part_number: None,
                parts: Vec::new(),
            }],
            extras: Vec::new(),
        }
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
