// SPDX-License-Identifier: AGPL-3.0-or-later
//! Neutral installation-set codec and provider format seam; machine-images §5.2.
//! OCI Image Manifest 1.1 supplies closure identity; array position defines order.

use crate::{
    invalid, media_type, BlobSource, ConfigValidator, Descriptor, Digest, Result, MAX_OBJECTS,
    MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Provider-owned format validation, independent of emulator hardware in storage.
/// See machine-images § Platform neutrality and §5.2.
pub trait MediaFormats {
    fn platform(&self) -> &str;
    /// Validate a provider-owned guest filename, including canonical spelling.
    fn validate_name(&self, name: &str) -> Result<()>;
    /// Return the declared payload type after validating the exact stored length.
    fn validate_format(&self, format: &toml::Table, size: u64) -> Result<&'static str>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Medium {
    pub id: String,
    pub label: String,
    pub format: toml::Table,
    pub blob: Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtraKind {
    Document,
    Archive,
    Executable,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extra {
    pub blob: Digest,
    pub filename: String,
    pub size: u64,
    pub kind: ExtraKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dos_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub platform: String,
    pub media: Vec<Medium>,
    #[serde(default)]
    pub extras: Vec<Extra>,
}

impl Config {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("media-set config exceeds 1 MiB"));
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
            return Err(invalid("media-set config exceeds 1 MiB"));
        }
        Ok(bytes)
    }
    fn validate(&self) -> Result<()> {
        if self.schema != 1
            || !crate::artifact::valid_platform(&self.platform)
            || self.media.is_empty()
            || self.media.len() + self.extras.len() > MAX_OBJECTS - 3
        {
            return Err(invalid("invalid media-set envelope"));
        }
        let mut ids = BTreeSet::new();
        for m in &self.media {
            if !valid_id(&m.id) || !ids.insert(&m.id) || !valid_text(&m.label) {
                return Err(invalid("invalid or duplicate media id, or empty label"));
            }
        }
        validate_extras(&self.extras)?;
        Ok(())
    }
}

/// Shared attachment checks for installation media and firmware packages.
pub fn validate_extras(extras: &[Extra]) -> Result<()> {
    let mut dos_names = BTreeSet::new();
    for e in extras {
        if !valid_text(&e.filename)
            || e.filename.contains(['/', '\\', '\0'])
            || matches!(e.filename.as_str(), "." | "..")
            || e.size > i64::MAX as u64
        {
            return Err(invalid("invalid extra filename or size"));
        }
        if let Some(name) = &e.dos_name {
            if !valid_text(name) || !dos_names.insert(name.clone()) {
                return Err(invalid("invalid or duplicate DOS 8.3 name"));
            }
        }
    }
    Ok(())
}

/// Bounded display text cannot inject terminal or menu controls.
pub fn valid_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= 128
        && !text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}
/// Stable media identifiers share the workspace/media grammar.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && !id.as_bytes()[0].is_ascii_uppercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// Whether a tag of an installation media set is an alias that may move.
///
/// Set tags follow `<upstream version>[-<edition>][-<format>][-rN]` (`1.4`,
/// `1.4-720k`, `1.4-r2`, `1.4-registered-720k-r2`), so each starts with a digit
/// of the upstream version and names one set forever, even when it is not a
/// semantic version. A tag whose first character is not an ASCII digit
/// (`latest`, `stable`) is an alias and moves with the usual confirmation;
/// machine-images §5.2.
pub fn is_alias_tag(tag: &str) -> bool {
    !tag.starts_with(|c: char| c.is_ascii_digit())
}

/// The revision tag to offer when a set's tag already names another set: a
/// trailing `-rN` counts up, and a tag without one gets `-r2`.
pub fn next_revision(tag: &str) -> String {
    let next = tag.rsplit_once("-r").and_then(|(base, number)| {
        let number = number
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| number.parse::<u64>().ok())
            .flatten()?;
        Some(format!("{base}-r{}", number.checked_add(1)?))
    });
    next.unwrap_or_else(|| format!("{tag}-r2"))
}

/// Adapter reuses store closure, full-blob verification and CAS publication.
pub struct Validator<'a>(pub &'a dyn MediaFormats);
impl ConfigValidator for Validator<'_> {
    fn artifact_type(&self) -> &str {
        media_type::MEDIA_SET
    }
    fn platform(&self) -> &str {
        self.0.platform()
    }
    fn payload_types(&self) -> &[crate::PayloadType] {
        crate::codec::MEDIA_PAYLOAD_TYPES
    }
    fn validate(
        &self,
        table: &toml::Table,
        descriptors: &[Descriptor],
        _: &dyn BlobSource,
    ) -> Result<Vec<Digest>> {
        let config: Config = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| invalid(e.to_string()))?;
        config.validate()?;
        if config.platform != self.0.platform() {
            return Err(invalid("media-set platform mismatch"));
        }
        let mut references = Vec::new();
        for m in &config.media {
            let d = descriptors
                .iter()
                .find(|d| d.names(&m.blob))
                .ok_or_else(|| invalid("missing media descriptor"))?;
            if d.decoded_media_type() != self.0.validate_format(&m.format, d.decoded_size()?)? {
                return Err(invalid("media format and payload type mismatch"));
            }
            references.push(m.blob.clone());
        }
        for e in &config.extras {
            if let Some(name) = &e.dos_name {
                self.0.validate_name(name)?;
            }
            let d = descriptors
                .iter()
                .find(|d| d.names(&e.blob))
                .ok_or_else(|| invalid("missing extra descriptor"))?;
            if d.decoded_size()? != e.size || d.decoded_media_type() != media_type::EXTRA {
                return Err(invalid("extra size or type mismatch"));
            }
            references.push(e.blob.clone());
        }
        Ok(references)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Config {
        Config {
            schema: 1,
            platform: "synthetic".into(),
            media: vec![Medium {
                id: "disk-1".into(),
                label: "Same label".into(),
                format: toml::Table::new(),
                blob: Digest::of(b"disk"),
            }],
            extras: vec![Extra {
                blob: Digest::of(b"manual"),
                filename: "manual.txt".into(),
                size: 6,
                kind: ExtraKind::Document,
                dos_name: Some("MANUAL.TXT".into()),
            }],
        }
    }
    #[test]
    fn media_set_round_trip_and_deterministic_digest() {
        let config = sample();
        let bytes = config.encode().unwrap();
        assert_eq!(Config::decode(&bytes).unwrap(), config);
        assert_eq!(Digest::of(&bytes), Digest::of(&config.encode().unwrap()));
        let mut repeated = config.clone();
        let mut second = repeated.media[0].clone();
        second.id = "disk-2".into();
        repeated.media.push(second);
        assert!(repeated.encode().is_ok());
        repeated.media.swap(0, 1);
        assert_ne!(repeated.encode().unwrap(), bytes);
    }
    #[test]
    fn media_set_rejects_bad_names_duplicate_ids_and_unknown_fields() {
        let config = sample();
        for name in ["bad\nname", "\u{7f}", ""] {
            let mut bad = config.clone();
            bad.extras[0].dos_name = Some(name.into());
            assert!(bad.encode().is_err());
        }
        let mut bad = config.clone();
        bad.media.push(bad.media[0].clone());
        assert!(bad.encode().is_err());
        bad = config.clone();
        bad.schema = 2;
        assert!(bad.encode().is_err());
        bad = config.clone();
        bad.media[0].label.clear();
        assert!(bad.encode().is_err());
        bad = config.clone();
        bad.media.clear();
        assert!(bad.encode().is_err());
        let text = String::from_utf8(config.encode().unwrap()).unwrap();
        assert!(Config::decode(format!("unknown = true\n{text}").as_bytes()).is_err());
        assert!(Config::decode(text.replace("sha256:", "invalid:").as_bytes()).is_err());
        assert!(Config::decode(&vec![b' '; MAX_SMALL_BLOB as usize + 1]).is_err());
    }
}

/// Catalogue provenance is independent of a drive and supplies future GC roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueSource {
    pub set: Digest,
    pub medium_id: String,
    /// Decoded content identity, as the set's config names the medium.
    pub blob: Digest,
}
impl CatalogueSource {
    /// The set manifest digest and the medium's decoded digest. A compressed
    /// medium is stored under a different blob digest, so a GC must mark
    /// through the set manifest's descriptors, never treat `blob` as a stored
    /// blob name.
    pub fn roots(&self) -> BTreeSet<Digest> {
        BTreeSet::from([self.set.clone(), self.blob.clone()])
    }
    pub fn same_medium(&self, other: &Self) -> bool {
        self.medium_id == other.medium_id && self.blob == other.blob
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn review_media_ids_text_and_neutral_guest_names() {
        for id in ["a", "disk.1_tools-2", &"a".repeat(64)] {
            assert!(valid_id(id), "{id}");
        }
        for id in ["", "-disk", ".disk", "_disk", "DISK", &"a".repeat(65)] {
            assert!(!valid_id(id), "{id}");
        }
        assert!(valid_text(&"a".repeat(128)));
        for text in ["a\nb", "a\tb", "a\0b", "a\u{7f}b", &"a".repeat(129)] {
            assert!(!valid_text(text));
        }
        let config = Config {
            schema: 1,
            platform: "other".into(),
            media: vec![Medium {
                id: "disk.1_tools".into(),
                label: "Disk".into(),
                format: toml::Table::new(),
                blob: Digest::of(b"disk"),
            }],
            extras: vec![Extra {
                blob: Digest::of(b"extra"),
                filename: "file".into(),
                size: 5,
                kind: ExtraKind::Other,
                dos_name: Some("CON".into()),
            }],
        };
        assert!(
            config.encode().is_ok(),
            "guest device names are provider-owned"
        );
    }

    #[test]
    fn set_tags_starting_with_a_digit_are_fixed_and_others_are_aliases() {
        for tag in ["1.4", "1.4-720k", "1.4-r2", "1.4-registered-720k-r2", "6"] {
            assert!(!is_alias_tag(tag), "{tag}");
        }
        for tag in ["latest", "stable", "v1.4", "_1"] {
            assert!(is_alias_tag(tag), "{tag}");
        }
        for (tag, next) in [
            ("1.4", "1.4-r2"),
            ("1.4-720k", "1.4-720k-r2"),
            ("1.4-r2", "1.4-r3"),
            ("1.4-registered-720k-r9", "1.4-registered-720k-r10"),
            ("1.4-registered", "1.4-registered-r2"),
            ("1.4-r", "1.4-r-r2"),
        ] {
            assert_eq!(next_revision(tag), next, "{tag}");
        }
    }
}
