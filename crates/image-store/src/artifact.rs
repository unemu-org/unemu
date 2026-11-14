// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI Image Manifest 1.1 and Content Descriptors codec; machine-images §2.
//! Struct field order and sorted annotation keys make the writer deterministic.

use crate::{invalid, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeMap, fmt, str::FromStr};

/// Maximum descriptor graph size before any payload is fetched.
pub const MAX_OBJECTS: usize = 10_000;
/// Maximum encoded manifest, config, metadata or sealed layer descriptor size.
pub const MAX_SMALL_BLOB: u64 = 1024 * 1024;

/// Explicitly supported media types, from machine-images §2.
/// Compression and saved-state codecs remain unsupported. Optical storage is reserved for media sets.
pub mod media_type {
    pub const MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
    pub const INDEX: &str = "application/vnd.oci.image.index.v1+json";
    pub const MACHINE: &str = "application/vnd.unemu.machine.v1";
    pub const MEDIA_SET: &str = "application/vnd.unemu.media-set.v1";
    pub const MEDIA_SET_CONFIG: &str = "application/vnd.unemu.media-set.config.v1+toml";
    pub const OPTICAL: &str = "application/vnd.unemu.media.optical.v1";
    pub const EXTRA: &str = "application/vnd.unemu.media.extra.v1";
    pub const CONFIG: &str = "application/vnd.unemu.machine.config.v1+toml";
    pub const METADATA: &str = "application/vnd.unemu.metadata.v1+json";
    pub const FIRMWARE_CONFIG: &str = "application/vnd.unemu.firmware.config.v1+toml";
    pub const FIRMWARE: &str = "application/vnd.unemu.firmware.v1";
    pub const DISK_BASE: &str = "application/vnd.unemu.disk.base.v1";
    pub const DISK_LAYER: &str = "application/vnd.unemu.disk.layer.v1+toml";
    pub const DISK_SECTORS: &str = "application/vnd.unemu.disk.sectors.v1";
    pub const FLOPPY: &str = "application/vnd.unemu.media.floppy.v1";
    pub const PNG: &str = "image/png";
    pub const JPEG: &str = "image/jpeg";
}

/// A provider-declared payload codec and its maximum stored size.
#[derive(Clone, Copy, Debug)]
pub struct PayloadType {
    pub media_type: &'static str,
    pub max_size: u64,
}

/// A validated full lowercase SHA-256 content address. Only its hex enters paths.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(String);

impl Digest {
    /// Hash exact stored bytes. Streaming ingestion is provided by [`crate::Store`].
    pub fn of(bytes: &[u8]) -> Self {
        Self::from_hash(Sha256::digest(bytes))
    }

    pub(crate) fn from_hash(bytes: impl AsRef<[u8]>) -> Self {
        let hex = bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        Self(hex)
    }

    /// Lowercase hexadecimal without the algorithm prefix.
    pub fn hex(&self) -> &str {
        &self.0
    }
}

impl FromStr for Digest {
    type Err = crate::Error;

    fn from_str(value: &str) -> Result<Self> {
        let hex = value
            .strip_prefix("sha256:")
            .filter(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or_else(|| invalid("digest must be sha256: followed by 64 lowercase hex digits"))?;
        Ok(Self(hex.to_owned()))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.0)
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// OCI Content Descriptor. Unknown fields fail closed; annotations are extensible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub media_type: String,
    pub digest: Digest,
    pub size: u64,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "unique_annotations"
    )]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urls: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

impl Descriptor {
    /// Build a descriptor; the enclosing document validates role and size limits.
    pub fn new(media_type: impl Into<String>, digest: Digest, size: u64) -> Self {
        Self {
            media_type: media_type.into(),
            digest,
            size,
            annotations: BTreeMap::new(),
            artifact_type: None,
            platform: None,
            urls: None,
            data: None,
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.validate_with_types(&[])
    }

    pub(crate) fn validate_with_types(&self, types: &[PayloadType]) -> Result<()> {
        let max = neutral_size_limit(&self.media_type)
            .or_else(|| {
                types
                    .iter()
                    .find(|t| t.media_type == self.media_type)
                    .map(|t| t.max_size)
            })
            .ok_or_else(|| invalid(format!("unsupported media type: {}", self.media_type)))?;
        self.validate_limit(max)
    }

    pub(crate) fn validate_limit(&self, max: u64) -> Result<()> {
        validate_annotations(&self.annotations)?;
        if self.size > max.min(i64::MAX as u64) {
            return Err(invalid("descriptor exceeds its media type size limit"));
        }
        Ok(())
    }
}

/// OCI machine or installation-media artifact. Decode validates the envelope; publication also validates
/// the provider's exact ordered closure against the stored config and metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    schema_version: u32,
    media_type: String,
    artifact_type: String,
    pub config: Descriptor,
    pub layers: Vec<Descriptor>,
    #[serde(deserialize_with = "unique_annotations")]
    pub annotations: BTreeMap<String, String>,
}

impl Manifest {
    /// Construct a deterministic envelope. Layers must already be deduplicated in
    /// config traversal order, followed by metadata and first-use artwork order.
    pub fn new(platform: &str, config: Descriptor, layers: Vec<Descriptor>) -> Result<Self> {
        Self::new_with_types(platform, config, layers, &[])
    }

    /// Construct an artifact with explicitly declared provider payload codecs.
    pub fn new_with_types(
        platform: &str,
        config: Descriptor,
        layers: Vec<Descriptor>,
        types: &[PayloadType],
    ) -> Result<Self> {
        let manifest = Self {
            schema_version: 2,
            media_type: media_type::MANIFEST.into(),
            artifact_type: media_type::MACHINE.into(),
            config,
            layers,
            annotations: BTreeMap::from([("unemu.platform".into(), platform.into())]),
        };
        manifest.validate(types)?;
        Ok(manifest)
    }

    /// Construct a media-set envelope using the same OCI closure rules.
    pub fn new_media_set(
        platform: &str,
        config: Descriptor,
        layers: Vec<Descriptor>,
    ) -> Result<Self> {
        let manifest = Self {
            schema_version: 2,
            media_type: media_type::MANIFEST.into(),
            artifact_type: media_type::MEDIA_SET.into(),
            config,
            layers,
            annotations: BTreeMap::from([("unemu.platform".into(), platform.into())]),
        };
        manifest.validate(&[])?;
        Ok(manifest)
    }

    /// Construct a firmware package; its closure contains ROMs and extras only.
    pub fn new_firmware(
        platform: &str,
        config: Descriptor,
        layers: Vec<Descriptor>,
    ) -> Result<Self> {
        let manifest = Self {
            schema_version: 2,
            media_type: media_type::MANIFEST.into(),
            artifact_type: media_type::FIRMWARE.into(),
            config,
            layers,
            annotations: BTreeMap::from([("unemu.platform".into(), platform.into())]),
        };
        manifest.validate(&[])?;
        Ok(manifest)
    }

    /// Artifact identity determines the required config codec.
    pub fn artifact_type(&self) -> &str {
        &self.artifact_type
    }

    /// Decode bounded JSON, rejecting unknown envelope/descriptor fields and types.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Self::decode_with_types(bytes, &[])
    }

    /// Decode with the selected provider's explicit payload types and size limits.
    pub fn decode_with_types(bytes: &[u8], types: &[PayloadType]) -> Result<Self> {
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("manifest exceeds 1 MiB"));
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        manifest.validate(types)?;
        Ok(manifest)
    }

    /// Bounded display-only decode. Unknown provider payload types remain opaque;
    /// this does not validate a closure or authorize execution/publication.
    pub fn decode_for_summary(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("manifest exceeds 1 MiB"));
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        manifest.validate_policy(&[], true)?;
        Ok(manifest)
    }

    /// Encode stable field order, sorted annotations and no timestamps or whitespace.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.encode_with_types(&[])
    }

    /// Encode with explicitly supported provider payload codecs.
    pub fn encode_with_types(&self, types: &[PayloadType]) -> Result<Vec<u8>> {
        self.validate(types)?;
        let bytes = serde_json::to_vec(self).map_err(|e| invalid(e.to_string()))?;
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("manifest exceeds 1 MiB"));
        }
        Ok(bytes)
    }

    /// Required platform routing hint; config validation remains authoritative.
    pub fn platform(&self) -> Result<&str> {
        self.annotations
            .get("unemu.platform")
            .map(String::as_str)
            .filter(|p| valid_platform(p))
            .ok_or_else(|| invalid("missing or invalid unemu.platform"))
    }

    fn validate(&self, types: &[PayloadType]) -> Result<()> {
        self.validate_policy(types, false)
    }

    fn validate_policy(&self, types: &[PayloadType], display: bool) -> Result<()> {
        if self.schema_version != 2
            || self.media_type != media_type::MANIFEST
            || !matches!(
                self.artifact_type.as_str(),
                media_type::MACHINE | media_type::MEDIA_SET | media_type::FIRMWARE
            )
        {
            return Err(invalid("unsupported manifest envelope"));
        }
        self.platform()?;
        validate_annotations(&self.annotations)?;
        self.config.validate()?;
        if self.config.media_type
            != if self.artifact_type == media_type::MEDIA_SET {
                media_type::MEDIA_SET_CONFIG
            } else if self.artifact_type == media_type::FIRMWARE {
                media_type::FIRMWARE_CONFIG
            } else {
                media_type::CONFIG
            }
        {
            return Err(invalid("config descriptor has the wrong media type"));
        }
        if self.layers.len() + 2 > MAX_OBJECTS {
            return Err(invalid("image exceeds 10,000 objects"));
        }
        let mut seen = std::collections::BTreeSet::from([&self.config.digest]);
        let mut metadata_count = 0;
        for layer in &self.layers {
            if !display
                && self.artifact_type == media_type::MACHINE
                && matches!(
                    layer.media_type.as_str(),
                    media_type::OPTICAL | media_type::EXTRA
                )
                && !types.iter().any(|kind| kind.media_type == layer.media_type)
            {
                return Err(invalid("media-set payload is not supported in a machine"));
            }
            if display {
                layer.validate_limit(
                    neutral_size_limit(&layer.media_type).unwrap_or(i64::MAX as u64),
                )?;
            } else {
                layer.validate_with_types(types)?;
            }
            if matches!(
                layer.media_type.as_str(),
                media_type::MANIFEST
                    | media_type::CONFIG
                    | media_type::MEDIA_SET_CONFIG
                    | media_type::FIRMWARE_CONFIG
            ) {
                return Err(invalid("manifest/config cannot be an artifact layer"));
            }
            if !seen.insert(&layer.digest) {
                return Err(invalid("duplicate descriptor digest"));
            }
            metadata_count += usize::from(layer.media_type == media_type::METADATA);
        }
        if self.artifact_type == media_type::FIRMWARE
            && self.layers.iter().any(|d| {
                !matches!(
                    d.media_type.as_str(),
                    media_type::FIRMWARE | media_type::EXTRA
                )
            })
        {
            return Err(invalid("firmware layers must be ROMs or extras"));
        }
        if metadata_count != usize::from(self.artifact_type != media_type::FIRMWARE) {
            return Err(invalid("exactly one metadata descriptor is required"));
        }
        Ok(())
    }
}

pub(crate) fn neutral_size_limit(kind: &str) -> Option<u64> {
    use media_type::*;
    match kind {
        MANIFEST | CONFIG | MEDIA_SET_CONFIG | FIRMWARE_CONFIG | METADATA | DISK_LAYER => {
            Some(MAX_SMALL_BLOB)
        }
        PNG | JPEG => Some(4 * MAX_SMALL_BLOB),
        FIRMWARE | DISK_BASE | DISK_SECTORS | FLOPPY | OPTICAL | EXTRA => Some(i64::MAX as u64),
        _ => None,
    }
}

fn validate_annotations(annotations: &BTreeMap<String, String>) -> Result<()> {
    if annotations
        .get("unemu.redistribution")
        .is_some_and(|v| !matches!(v.as_str(), "open" | "user-supplied"))
    {
        return Err(invalid("invalid redistribution annotation"));
    }
    if let Some(value) = annotations.get("org.opencontainers.image.base.digest") {
        value.parse::<Digest>()?;
    }
    Ok(())
}

fn unique_annotations<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    struct Annotations;
    impl<'de> serde::de::Visitor<'de> for Annotations {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("unique string annotations")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut annotations = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if annotations.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate annotation key"));
                }
            }
            Ok(annotations)
        }
    }
    deserializer.deserialize_map(Annotations)
}

pub(crate) fn valid_platform(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || (i > 0 && matches!(b, b'-' | b'_' | b'.'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_identity_matches_the_standard_abc_vector() {
        assert_eq!(
            Digest::of(b"abc").to_string(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn manifest_requires_one_metadata_blob_and_unique_content_addresses() {
        let config = Descriptor::new(media_type::CONFIG, Digest::of(b"config"), 6);
        assert!(Manifest::new("c64", config.clone(), Vec::new()).is_err());
        let metadata = Descriptor::new(media_type::METADATA, Digest::of(b"metadata"), 8);
        let second = Descriptor::new(media_type::METADATA, Digest::of(b"other metadata"), 14);
        assert!(Manifest::new("c64", config.clone(), vec![metadata.clone(), second]).is_err());
        let alias = Descriptor::new(media_type::FIRMWARE, config.digest.clone(), config.size);
        assert!(Manifest::new("c64", config, vec![alias, metadata]).is_err());
    }

    #[test]
    fn image_graph_and_artwork_are_bounded_before_fetching() {
        let config = Descriptor::new(media_type::CONFIG, Digest::of(b"config"), 6);
        let metadata = Descriptor::new(media_type::METADATA, Digest::of(b"metadata"), 8);
        let artwork = Descriptor::new(media_type::PNG, Digest::of(b"art"), 4 * MAX_SMALL_BLOB + 1);
        assert!(Manifest::new("c64", config.clone(), vec![metadata.clone(), artwork]).is_err());
        let mut layers: Vec<_> = (0..MAX_OBJECTS)
            .map(|i| Descriptor::new(media_type::FIRMWARE, Digest::of(&i.to_le_bytes()), 1))
            .collect();
        layers.push(metadata);
        assert!(Manifest::new("c64", config, layers).is_err());
    }
}
