// SPDX-License-Identifier: AGPL-3.0-or-later
//! Platform-neutral catalogue codec; machine-images §2.1. Software platforms are
//! extensible strings, independent of the machine provider's routing platform.
//!
//! Schema 2 adds what a title needs from its platform ([`crate::needs`]), the
//! box's own requirements text, and the work a release belongs to: its id,
//! its synopsis and its ids in outside catalogues (recipes §6.4, §6.7, §6.16).
//! The writer uses the oldest schema that holds every field a title sets
//! ([`crate::compat::TITLE_FIELDS`]), so metadata without the new fields stays
//! schema 1 with the same bytes, which every release reads. Release 0.0.2
//! shows schema 2 read-only and still runs the artifact.

use crate::{
    artifact::valid_platform,
    compat, invalid,
    media_set::{valid_id, valid_text},
    media_type,
    needs::{self, Needs},
    Descriptor, Digest, Result, MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The newest metadata schema this build reads and writes in full; a newer
/// one is shown read-only.
pub const SCHEMA: u32 = 2;

/// The id earlier releases gave the one title record of every installation
/// set, whatever its repository. Sets that carry it keep it; a set published
/// again takes [`title_id`].
pub const LEGACY_TITLE_ID: &str = "title";

/// The stable id of an installation set's title record (recipes §6.4): the
/// last component of the repository `reference` names, which is the work id
/// of a set that names its work. `None` when the reference names no
/// repository.
pub fn title_id(reference: &str) -> Option<&str> {
    let repository = reference
        .rsplit_once(':')
        .map_or(reference, |(repository, _)| repository);
    let last = repository.rsplit('/').next().unwrap_or(repository);
    (!last.is_empty()).then_some(last)
}

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

/// Catalogues outside this program whose ids a title's work may carry
/// (recipes §6.8): ids are strings this program never interprets, and it
/// never copies a catalogue's data. Writers state only these; readers keep
/// any catalogue name of a valid shape, so one a later release adds still
/// reads.
pub const WORK_ID_KINDS: &[&str] = &["wikidata", "mobygames", "igdb"];

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
    /// Schema 2: what the title needs from its platform family, as kinds of
    /// feature and their alternatives ([`crate::needs`]). A kind not stated
    /// is unknown, not absent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub needs: Needs,
    /// Schema 2: the box's own words about the machine it wants ("IBM PC, XT,
    /// AT; 640K; CGA, EGA or VGA"), shown beside the needs, never evaluated.
    /// Blank text is not written.
    #[serde(default, skip_serializing_if = "blank")]
    pub requirements: Option<String>,
    /// Schema 2: the id of the work this release belongs to, the last
    /// component of its repository (recipes §6.16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    /// Schema 2: what the work is, shared by its releases; the description
    /// says what this release is. Blank text is not written.
    #[serde(default, skip_serializing_if = "blank")]
    pub synopsis: Option<String>,
    /// Schema 2: the work's ids in outside catalogues, keyed by the
    /// catalogue's name, one of [`WORK_ID_KINDS`] when written.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub work_ids: BTreeMap<String, String>,
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
        needs::shape(&self.needs)?;
        for (field, text) in [
            ("requirements", &self.requirements),
            ("synopsis", &self.synopsis),
        ] {
            if text.as_deref().is_some_and(has_control) {
                return Err(invalid(format!("{field}: no control characters")));
            }
        }
        if let Some(work) = self.work_id.as_deref().filter(|id| !valid_id(id)) {
            return Err(invalid(format!(
                "work {work:?}: a work id is lowercase ASCII letters, digits, '.', '_' and '-', starting with a letter or digit, at most 64 characters"
            )));
        }
        for (catalogue, id) in &self.work_ids {
            // The shape of a catalogue name is the shape of a need kind.
            if !needs::valid_kind(catalogue) {
                return Err(invalid(format!(
                    "the work's id in catalogue {catalogue:?}: a catalogue name is lowercase ASCII letters, digits and '_', starting with a letter, at most 32 characters"
                )));
            }
            if !valid_text(id) {
                return Err(invalid(format!(
                    "the work's {catalogue} id: at most 128 characters, no control characters, not blank"
                )));
            }
        }
        Ok(())
    }

    /// What only a writer refuses: a catalogue this release does not know.
    fn check_written(&self) -> Result<()> {
        for catalogue in self.work_ids.keys() {
            if !WORK_ID_KINDS.contains(&catalogue.as_str()) {
                return Err(invalid(format!(
                    "the work's id in catalogue {catalogue:?}: the catalogue is not one of {}",
                    WORK_ID_KINDS.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// The oldest metadata schema that holds every field this record sets.
    fn schema(&self) -> Result<u32> {
        let value = serde_json::to_value(self).map_err(|e| invalid(e.to_string()))?;
        let fields = value.as_object().into_iter().flat_map(|o| o.keys());
        compat::metadata_schema_for(fields.map(String::as_str))
    }

    pub(crate) fn artwork(&self) -> impl Iterator<Item = &Digest> {
        self.cover.iter().chain(&self.screenshots)
    }
}

/// Text the writer leaves out: absent or blank.
fn blank(text: &Option<String>) -> bool {
    text.as_deref().is_none_or(|t| t.trim().is_empty())
}

/// C0 control characters and DEL, as [`valid_text`] refuses them.
fn has_control(text: &str) -> bool {
    text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

/// A catalogue of schema 1 or 2, or a read-only display projection.
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

    /// The schema the metadata was read with, 1 for new metadata. The
    /// writer picks its own: the oldest that holds the fields in use.
    pub fn schema(&self) -> u32 {
        self.schema
    }
    pub fn is_read_only(&self) -> bool {
        self.schema > SCHEMA || self.read_only
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

    /// Decode strict schema 1 or 2, or retain individually understood fields of
    /// future titles. A title field the stated schema does not hold is
    /// refused, as an unknown field is. Artwork validation for future schemas
    /// remains owned by the manifest closure.
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
        if envelope.schema <= SCHEMA {
            let metadata: Self =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            metadata.validate(layers)?;
            // The typed decode cannot tell a stated empty collection from an
            // absent one, so the keys are read as written.
            let value: Value = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            for title in value
                .get("titles")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for field in title.as_object().into_iter().flat_map(|o| o.keys()) {
                    if compat::metadata_schema_for([field.as_str()])? > envelope.schema {
                        return Err(invalid(format!(
                            "metadata schema {} has no title field {field}",
                            envelope.schema
                        )));
                    }
                }
            }
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
                    needs: field(value, "needs").unwrap_or_default(),
                    requirements: field(value, "requirements").unwrap_or_default(),
                    work_id: field(value, "work_id").unwrap_or_default(),
                    synopsis: field(value, "synopsis").unwrap_or_default(),
                    work_ids: field(value, "work_ids").unwrap_or_default(),
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
                if needs::shape(&title.needs).is_err() {
                    title.needs.clear();
                }
                if title.work_id.as_deref().is_some_and(|id| !valid_id(id)) {
                    title.work_id = None;
                }
                for text in [&mut title.requirements, &mut title.synopsis] {
                    if text.as_deref().is_some_and(has_control) {
                        *text = None;
                    }
                }
                title
                    .work_ids
                    .retain(|catalogue, id| needs::valid_kind(catalogue) && valid_text(id));
                metadata.titles.push(title);
            }
        }
        Ok(metadata)
    }

    /// Canonical writer: fixed struct order and sorted object keys, no
    /// timestamps, and the oldest schema that holds every field in use.
    pub fn encode(&self, layers: &[Descriptor]) -> Result<Vec<u8>> {
        if self.is_read_only() {
            return Err(invalid("newer metadata is read-only"));
        }
        self.validate(layers)?;
        let mut written = self.clone();
        written.schema = 1;
        for title in &self.titles {
            title.check_written()?;
            written.schema = written.schema.max(title.schema()?);
        }
        let bytes = serde_json::to_vec(&written).map_err(|e| invalid(e.to_string()))?;
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
