// SPDX-License-Identifier: AGPL-3.0-or-later
//! Platform-neutral catalogue codec; machine-images §2.1. Software platforms are
//! extensible strings, independent of the machine provider's routing platform.

use crate::{
    artifact::valid_platform, invalid, media_type, Descriptor, Digest, Result, MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// Canonical software-environment choices. Stored strings remain extensible so
/// a newer provider's vocabulary survives round trips and remains searchable.
pub const SOFTWARE_PLATFORMS: &[&str] = &[
    "DOS",
    "Windows 3.x",
    "Windows 9x",
    "CP/M",
    "MSX",
    "ZX Spectrum",
    "C64",
    "Atari 8-bit",
    "Atari ST",
    "other",
];

/// Positive inclusive player-count range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Players {
    pub min: u32,
    pub max: u32,
}

/// One stable catalogue record. Unknown software platforms are retained verbatim.
/// Launch data is provider-owned; shared code validates only its profile selector.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub developers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publishers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_year: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<Players>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_media: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub screenshots: Vec<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<serde_json::Map<String, Value>>,
}

impl Title {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || self.title.trim().is_empty() {
            return Err(invalid("title id and title must be nonempty"));
        }
        if self
            .players
            .as_ref()
            .is_some_and(|p| p.min == 0 || p.max < p.min)
        {
            return Err(invalid("players requires positive min <= max"));
        }
        if self.launch.as_ref().is_some_and(|launch| {
            launch
                .get("profile")
                .and_then(Value::as_str)
                .is_none_or(|s| s.trim().is_empty())
        }) {
            return Err(invalid("launch requires a profile string"));
        }
        Ok(())
    }

    pub(crate) fn artwork(&self) -> impl Iterator<Item = &Digest> {
        self.cover.iter().chain(&self.screenshots)
    }
}

/// A schema-1 catalogue or a read-only display projection.
/// Private schema/read-only state prevents rewriting damaged or future data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    schema: u32,
    #[serde(skip)]
    read_only: bool,
    #[serde(skip)]
    warnings: Vec<String>,
    pub platform: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub titles: Vec<Title>,
}

impl Metadata {
    pub fn new(platform: impl Into<String>) -> Self {
        Self {
            schema: 1,
            read_only: false,
            warnings: Vec::new(),
            platform: platform.into(),
            labels: Vec::new(),
            titles: Vec::new(),
        }
    }

    pub fn schema(&self) -> u32 {
        self.schema
    }
    pub fn is_read_only(&self) -> bool {
        self.schema != 1 || self.read_only
    }

    pub(crate) fn unavailable(platform: &str) -> Self {
        let mut metadata = Self::new(platform);
        metadata.read_only = true;
        metadata.warn("metadata unavailable for display".into());
        metadata
    }

    /// Nonfatal local-overlay diagnostics; never serialized into a blob.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub(crate) fn warn(&mut self, reason: String) {
        self.warnings.push(reason);
    }

    /// Decode strict v1 or retain individually understood fields of future titles.
    /// Artwork validation for future schemas remains owned by the manifest closure.
    pub fn decode(bytes: &[u8], layers: &[Descriptor]) -> Result<Self> {
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("metadata exceeds 1 MiB"));
        }
        #[derive(Deserialize)]
        struct Envelope {
            schema: u32,
            platform: String,
        }
        let envelope: Envelope =
            serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        if envelope.schema == 0 || !valid_platform(&envelope.platform) {
            return Err(invalid("invalid metadata schema or platform"));
        }
        if envelope.schema == 1 {
            let metadata: Self =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            metadata.validate(layers)?;
            return Ok(metadata);
        }
        Self::project(bytes)
    }

    /// Display understood fields without granting permission to rewrite damaged v1.
    pub fn decode_for_display(bytes: &[u8], layers: &[Descriptor]) -> Result<Self> {
        Self::decode(bytes, layers).or_else(|_| Self::project(bytes))
    }

    fn project(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("metadata exceeds 1 MiB"));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        let schema = value
            .get("schema")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| invalid("invalid metadata schema"))?;
        let platform = value
            .get("platform")
            .and_then(Value::as_str)
            .filter(|p| valid_platform(p))
            .ok_or_else(|| invalid("invalid metadata platform"))?;
        fn field<T: serde::de::DeserializeOwned>(value: &Value, name: &str) -> Option<T> {
            value
                .get(name)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        }
        let mut metadata = Self::new(platform);
        metadata.schema = schema;
        metadata.read_only = true;
        metadata.labels = value
            .get("labels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        if let Some(titles) = value.get("titles").and_then(Value::as_array) {
            let mut ids = BTreeSet::new();
            for value in titles {
                let (Some(id), Some(title)) = (
                    value.get("id").and_then(Value::as_str),
                    value.get("title").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if id.trim().is_empty() || title.trim().is_empty() || !ids.insert(id) {
                    continue;
                }
                // Look up known fields once; unknown keys never copy a large title.
                let mut title = Title {
                    id: id.into(),
                    title: title.into(),
                    sort_title: field(value, "sort_title").unwrap_or_default(),
                    series: field(value, "series").unwrap_or_default(),
                    genres: field(value, "genres").unwrap_or_default(),
                    developers: field(value, "developers").unwrap_or_default(),
                    publishers: field(value, "publishers").unwrap_or_default(),
                    platform: field(value, "platform").unwrap_or_default(),
                    release_year: field(value, "release_year").unwrap_or_default(),
                    players: field(value, "players").unwrap_or_default(),
                    languages: field(value, "languages").unwrap_or_default(),
                    regions: field(value, "regions").unwrap_or_default(),
                    description: field(value, "description").unwrap_or_default(),
                    version: field(value, "version").unwrap_or_default(),
                    edition: field(value, "edition").unwrap_or_default(),
                    source_media: field(value, "source_media").unwrap_or_default(),
                    tags: field(value, "tags").unwrap_or_default(),
                    cover: field(value, "cover").unwrap_or_default(),
                    screenshots: field(value, "screenshots").unwrap_or_default(),
                    launch: field(value, "launch").unwrap_or_default(),
                };
                if title
                    .players
                    .as_ref()
                    .is_some_and(|p| p.min == 0 || p.max < p.min)
                {
                    title.players = None;
                }
                if title.launch.as_ref().is_some_and(|l| {
                    l.get("profile")
                        .and_then(Value::as_str)
                        .is_none_or(|p| p.trim().is_empty())
                }) {
                    title.launch = None;
                }
                metadata.titles.push(title);
            }
        }
        Ok(metadata)
    }

    /// Canonical writer: fixed struct order and sorted object keys, no timestamps.
    pub fn encode(&self, layers: &[Descriptor]) -> Result<Vec<u8>> {
        if self.is_read_only() {
            return Err(invalid("newer metadata is read-only"));
        }
        self.validate(layers)?;
        let bytes = serde_json::to_vec(self).map_err(|e| invalid(e.to_string()))?;
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("metadata exceeds 1 MiB"));
        }
        Ok(bytes)
    }

    pub fn artwork(&self) -> impl Iterator<Item = &Digest> {
        self.titles.iter().flat_map(Title::artwork)
    }

    pub(crate) fn validate(&self, layers: &[Descriptor]) -> Result<()> {
        if !valid_platform(&self.platform) {
            return Err(invalid("invalid metadata platform"));
        }
        let mut ids = BTreeSet::new();
        for title in &self.titles {
            title.validate()?;
            if !ids.insert(&title.id) {
                return Err(invalid("duplicate title id"));
            }
            for digest in title.artwork() {
                if !layers.iter().any(|d| {
                    &d.digest == digest
                        && matches!(d.media_type.as_str(), media_type::PNG | media_type::JPEG)
                }) {
                    return Err(invalid("artwork absent from manifest or wrong media type"));
                }
            }
        }
        Ok(())
    }
}
