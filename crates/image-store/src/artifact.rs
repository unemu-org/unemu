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
/// Binary compression is provider-declared. Optical storage is reserved for media sets.
pub mod media_type {
    pub const MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
    pub const INDEX: &str = "application/vnd.oci.image.index.v1+json";
    pub const MACHINE: &str = "application/vnd.unemu.machine.v1";
    pub const MEDIA_SET: &str = "application/vnd.unemu.media-set.v1";
    pub const MEDIA_SET_CONFIG: &str = "application/vnd.unemu.media-set.config.v1+toml";
    pub const OPTICAL: &str = "application/vnd.unemu.media.optical.v1";
    pub const EXTRA: &str = "application/vnd.unemu.media.extra.v1";
    pub const PC_LAUNCH_CONFIG: &str = "application/vnd.unemu.pc.launch.v1+toml";
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
    /// A tar archive (POSIX.1-2001 pax or ustar), such as a firmware source archive.
    pub const TAR: &str = "application/x-tar";
    /// A zip archive (PKWARE APPNOTE), such as a firmware source archive.
    pub const ZIP: &str = "application/zip";
    /// UTF-8 Markdown text (RFC 7763), such as a firmware package README. The
    /// charset parameter is left out because OCI media types carry no parameters.
    pub const MARKDOWN: &str = "text/markdown";
    /// A media recipe (recipes.md §7.1): one artifact type per recipe kind,
    /// because a registry filters a listing by artifact type and by
    /// nothing else.
    pub const RECIPE_MEDIA: &str = "application/vnd.unemu.recipe.media.v1";
    /// A firmware recipe (recipes.md §7.1).
    pub const RECIPE_FIRMWARE: &str = "application/vnd.unemu.recipe.firmware.v1";
    /// A machine-template recipe and its permitted texts and lock.
    pub const RECIPE_MACHINE: &str = "application/vnd.unemu.recipe.machine.v1";
    /// A recipe artifact's config: the recipe file itself, whose digest is
    /// the recipe's.
    pub const RECIPE_CONFIG: &str = "application/vnd.unemu.recipe.v1+toml";
    /// A recipe artifact's layer holding the work file the recipe names
    /// (recipes.md §6.16).
    pub const RECIPE_WORK: &str = "application/vnd.unemu.recipe.work.v1+toml";
    /// A recipe artifact's layer holding one bundled input (`bundle = true`,
    /// recipes.md §6.8): our own text, at most 1 MiB.
    pub const RECIPE_INPUT: &str = "application/vnd.unemu.recipe.input.v1";
    /// A recipe artifact's layer holding one file of its lock (recipes.md
    /// §7.2): the output's manifest, config or another generated layer.
    pub const RECIPE_LOCK: &str = "application/vnd.unemu.recipe.lock.v1";
}

/// Whether `kind` is the artifact type of a recipe (recipes.md §7.1).
pub fn is_recipe(kind: &str) -> bool {
    matches!(
        kind,
        media_type::RECIPE_MEDIA | media_type::RECIPE_FIRMWARE | media_type::RECIPE_MACHINE
    )
}

/// The layer types of a recipe artifact, each at most 1 MiB, as the bundle
/// and a lock file are bounded (recipes.md §6.11 rule 9, §7.2).
pub(crate) const RECIPE_PARTS: &[PayloadType] = &[
    PayloadType {
        media_type: media_type::RECIPE_WORK,
        max_size: MAX_SMALL_BLOB,
    },
    PayloadType {
        media_type: media_type::RECIPE_INPUT,
        max_size: MAX_SMALL_BLOB,
    },
    PayloadType {
        media_type: media_type::RECIPE_LOCK,
        max_size: MAX_SMALL_BLOB,
    },
];

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

    /// Whether the stored bytes are a Zstandard frame (a `+zstd` media type).
    pub fn is_compressed(&self) -> bool {
        self.media_type.ends_with(crate::codec::SUFFIX)
    }

    /// Payload type after decoding; support still requires provider declaration.
    pub fn decoded_media_type(&self) -> &str {
        self.media_type
            .strip_suffix(crate::codec::SUFFIX)
            .unwrap_or(&self.media_type)
    }

    /// Raw identity used by machine configs, journals and layer parents.
    pub fn decoded_digest(&self) -> Result<Digest> {
        if self.is_compressed() {
            self.annotations
                .get(crate::codec::DECODED_DIGEST)
                .ok_or_else(|| invalid(format!("blob {} missing decoded digest", self.digest)))?
                .parse()
                .map_err(|e| invalid(format!("blob {}: {e}", self.digest)))
        } else {
            Ok(self.digest.clone())
        }
    }

    /// Raw length, checked before allocating or decoding. Only canonical
    /// decimal is accepted, so one length has one manifest spelling.
    pub fn decoded_size(&self) -> Result<u64> {
        if self.is_compressed() {
            self.annotations
                .get(crate::codec::DECODED_SIZE)
                .filter(|s| {
                    s.bytes().all(|b| b.is_ascii_digit())
                        && (s.as_str() == "0" || !s.starts_with('0'))
                })
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| *n <= i64::MAX as u64)
                .ok_or_else(|| {
                    invalid(format!(
                        "blob {} missing or invalid decoded size",
                        self.digest
                    ))
                })
        } else {
            Ok(self.size)
        }
    }

    /// Match a config reference without tying machine identity to compression.
    pub fn names(&self, digest: &Digest) -> bool {
        self.decoded_digest().is_ok_and(|d| &d == digest)
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

    /// `max` bounds the stored length of a raw payload and the declared
    /// decoded length of a compressed one, whose stored frame may not exceed
    /// the worst-case frame size for that decoded length.
    pub(crate) fn validate_limit(&self, max: u64) -> Result<()> {
        validate_annotations(&self.annotations)?;
        let max = max.min(i64::MAX as u64);
        if self.is_compressed() {
            self.decoded_digest()?;
            let decoded = self.decoded_size()?;
            if decoded > max {
                return Err(invalid(format!(
                    "blob {}: decoded size exceeds its media type size limit",
                    self.digest
                )));
            }
            if self.size > crate::codec::max_frame_size(decoded) {
                return Err(invalid("compressed frame exceeds its worst-case size"));
            }
            return Ok(());
        }
        if self.annotations.contains_key(crate::codec::DECODED_DIGEST)
            || self.annotations.contains_key(crate::codec::DECODED_SIZE)
        {
            return Err(invalid(
                "decoded identity annotations require a compressed payload",
            ));
        }
        if self.size > max {
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

    /// Construct a recipe artifact (recipes.md §7.1) of `artifact_type`,
    /// [`media_type::RECIPE_MEDIA`] or [`media_type::RECIPE_FIRMWARE`]: the
    /// recipe file as its config and its work file, bundled inputs and lock
    /// files as layers. The recipe crate adds the annotations and checks
    /// what the parts hold.
    pub fn new_recipe(
        artifact_type: &str,
        platform: &str,
        config: Descriptor,
        layers: Vec<Descriptor>,
    ) -> Result<Self> {
        if !is_recipe(artifact_type) {
            return Err(invalid(format!(
                "{artifact_type} is not a recipe artifact type"
            )));
        }
        let manifest = Self {
            schema_version: 2,
            media_type: media_type::MANIFEST.into(),
            artifact_type: artifact_type.into(),
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

    /// Name the platform by its current id ([`crate::platform::canonical`]):
    /// a writer that derives a manifest from one naming a retired id never
    /// writes that id again.
    pub fn canonicalize_platform(&mut self) -> Result<()> {
        let current = crate::platform::canonical(self.platform()?).to_owned();
        self.annotations.insert("unemu.platform".into(), current);
        Ok(())
    }

    fn validate(&self, types: &[PayloadType]) -> Result<()> {
        self.validate_policy(types, false)
    }

    fn validate_policy(&self, types: &[PayloadType], display: bool) -> Result<()> {
        if self.schema_version != 2
            || self.media_type != media_type::MANIFEST
            || !matches!(
                self.artifact_type.as_str(),
                media_type::MACHINE
                    | media_type::MEDIA_SET
                    | media_type::FIRMWARE
                    | media_type::RECIPE_MEDIA
                    | media_type::RECIPE_FIRMWARE
                    | media_type::RECIPE_MACHINE
            )
        {
            return Err(invalid("unsupported manifest envelope"));
        }
        self.platform()?;
        validate_annotations(&self.annotations)?;
        let recipe = is_recipe(&self.artifact_type);
        if recipe {
            self.config.validate_limit(MAX_SMALL_BLOB)?;
        } else {
            self.config.validate()?;
        }
        if !(self.artifact_type == media_type::MACHINE
            && self.config.media_type == media_type::PC_LAUNCH_CONFIG)
            && self.config.media_type
                != if recipe {
                    media_type::RECIPE_CONFIG
                } else if self.artifact_type == media_type::MEDIA_SET {
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
            if recipe
                && !RECIPE_PARTS
                    .iter()
                    .any(|t| t.media_type == layer.media_type)
            {
                return Err(invalid(format!(
                    "a recipe artifact holds its work file, bundled inputs and lock files only, not {}",
                    layer.media_type
                )));
            }
            let archival_extra = self.artifact_type == media_type::MACHINE
                && self.config.media_type == media_type::PC_LAUNCH_CONFIG
                && layer.decoded_media_type() == media_type::EXTRA;
            if !display
                && !archival_extra
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
                // Envelope-owned artifacts declare their own codecs.
                let types = match self.artifact_type.as_str() {
                    _ if recipe => RECIPE_PARTS,
                    _ if archival_extra => crate::codec::FIRMWARE_PAYLOAD_TYPES,
                    media_type::MEDIA_SET if types.is_empty() => crate::codec::MEDIA_PAYLOAD_TYPES,
                    media_type::FIRMWARE if types.is_empty() => {
                        crate::codec::FIRMWARE_PAYLOAD_TYPES
                    }
                    _ => types,
                };
                layer.validate_with_types(types)?;
            }
            if matches!(
                layer.media_type.as_str(),
                media_type::MANIFEST
                    | media_type::CONFIG
                    | media_type::PC_LAUNCH_CONFIG
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
                    d.decoded_media_type(),
                    media_type::FIRMWARE
                        | media_type::EXTRA
                        | media_type::TAR
                        | media_type::ZIP
                        | media_type::MARKDOWN
                )
            })
        {
            return Err(invalid(
                "firmware layers must be ROMs, extras, source archives or a README",
            ));
        }
        let metadata_wanted = matches!(
            self.artifact_type.as_str(),
            media_type::MACHINE | media_type::MEDIA_SET
        );
        if metadata_count != usize::from(metadata_wanted) {
            return Err(invalid("exactly one metadata descriptor is required"));
        }
        Ok(())
    }
}

/// Media types the neutral envelope reads by itself: each one's size limit
/// and the first release that reads it (machine-images §5.6), so a new type
/// cannot be read without a row.
pub(crate) const NEUTRAL_TYPES: &[(&str, u64, crate::compat::AppVersion)] = {
    use crate::compat::BASELINE;
    use media_type::*;
    const ANY: u64 = i64::MAX as u64;
    &[
        (MANIFEST, MAX_SMALL_BLOB, BASELINE),
        (CONFIG, MAX_SMALL_BLOB, BASELINE),
        (
            PC_LAUNCH_CONFIG,
            MAX_SMALL_BLOB,
            crate::compat::NEXT_RELEASE,
        ),
        (MEDIA_SET_CONFIG, MAX_SMALL_BLOB, BASELINE),
        (FIRMWARE_CONFIG, MAX_SMALL_BLOB, BASELINE),
        (METADATA, MAX_SMALL_BLOB, BASELINE),
        (DISK_LAYER, MAX_SMALL_BLOB, BASELINE),
        (MARKDOWN, MAX_SMALL_BLOB, BASELINE),
        (PNG, 4 * MAX_SMALL_BLOB, BASELINE),
        (JPEG, 4 * MAX_SMALL_BLOB, BASELINE),
        (FIRMWARE, ANY, BASELINE),
        (DISK_BASE, ANY, BASELINE),
        (DISK_SECTORS, ANY, BASELINE),
        (FLOPPY, ANY, BASELINE),
        (OPTICAL, ANY, BASELINE),
        (EXTRA, ANY, BASELINE),
        (TAR, ANY, BASELINE),
        (ZIP, ANY, BASELINE),
    ]
};

pub(crate) fn neutral_size_limit(kind: &str) -> Option<u64> {
    NEUTRAL_TYPES
        .iter()
        .find(|(t, _, _)| *t == kind)
        .map(|(_, max, _)| *max)
}

fn validate_annotations(annotations: &BTreeMap<String, String>) -> Result<()> {
    // The accepted values are the compatibility table's rows, so a new value
    // cannot be read without one.
    if annotations
        .get("unemu.redistribution")
        .is_some_and(|v| crate::compat::lookup(&[crate::compat::REDISTRIBUTION], v).is_none())
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

    /// A recipe artifact holds its recipe as config and only its work
    /// file, bundled inputs and lock files as layers, each at most 1 MiB,
    /// and no metadata; every one of its types has a compatibility row.
    #[test]
    fn a_recipe_artifact_holds_its_parts_only_and_fails_closed() {
        let config = Descriptor::new(media_type::RECIPE_CONFIG, Digest::of(b"recipe"), 6);
        let part =
            |kind: &str, bytes: &[u8]| Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
        let parts = vec![
            part(media_type::RECIPE_WORK, b"work"),
            part(media_type::RECIPE_INPUT, b"notes"),
            part(media_type::RECIPE_LOCK, b"manifest"),
        ];
        for kind in [
            media_type::RECIPE_MEDIA,
            media_type::RECIPE_FIRMWARE,
            media_type::RECIPE_MACHINE,
        ] {
            let manifest =
                Manifest::new_recipe(kind, "ibm-pc", config.clone(), parts.clone()).unwrap();
            let bytes = manifest.encode().unwrap();
            assert_eq!(Manifest::decode(&bytes).unwrap(), manifest);
            assert_eq!(Manifest::decode_for_summary(&bytes).unwrap(), manifest);
            assert!(is_recipe(manifest.artifact_type()));
            assert_eq!(
                crate::compat::envelope(&manifest, &[]).unwrap(),
                if kind == media_type::RECIPE_MACHINE {
                    crate::compat::NEXT_RELEASE
                } else {
                    crate::compat::AppVersion::new(0, 0, 3)
                }
            );
        }
        let refused = |config: Descriptor, layers: Vec<Descriptor>| {
            Manifest::new_recipe(media_type::RECIPE_MEDIA, "ibm-pc", config, layers).is_err()
        };
        // Another layer type, a metadata blob, a payload or a large part.
        assert!(refused(
            config.clone(),
            vec![part(media_type::METADATA, b"m")]
        ));
        assert!(refused(
            config.clone(),
            vec![part(media_type::FLOPPY, b"disk")]
        ));
        let mut large = part(media_type::RECIPE_INPUT, b"large");
        large.size = MAX_SMALL_BLOB + 1;
        assert!(refused(config.clone(), vec![large]));
        // Another config type, or a recipe config in another envelope.
        assert!(refused(
            Descriptor::new(media_type::MEDIA_SET_CONFIG, Digest::of(b"c"), 1),
            Vec::new()
        ));
        assert!(Manifest::new_media_set("ibm-pc", config.clone(), Vec::new()).is_err());
        assert!(Manifest::new_recipe(media_type::MEDIA_SET, "ibm-pc", config, Vec::new()).is_err());
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
