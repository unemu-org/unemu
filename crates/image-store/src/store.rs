// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI Image Layout 1.0.0 store, closure validation and local tag transactions.
//! See machine-images §§2–3.1: immutable blobs precede mutable index publication.

use crate::{
    artifact::valid_platform, filesystem as host, invalid, media_type, Descriptor, Digest, Error,
    Manifest, PayloadType, Result, MAX_OBJECTS, MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use serde_json::value::{to_raw_value, RawValue};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// Verified content identity and length returned by streaming ingestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub digest: Digest,
    pub size: u64,
}

/// Read-only access for providers traversing transitive config references.
/// OCI Content Descriptors require both digest and size checks before decoding.
pub trait BlobSource {
    /// Read and fully hash a bounded descriptor, refusing oversized data first.
    fn read_verified(&self, descriptor: &Descriptor, limit: u64) -> Result<Vec<u8>>;
}

/// Platform-owned validation of the common routing envelope's remaining fields.
/// See machine-images §§1–2: shared storage never interprets emulator hardware.
///
/// Implementations must reject unknown config fields/versions, host paths,
/// unsupported payloads and invalid bindings, and validate any transitive layer
/// descriptors (including depth <= 64, cycles and reconstruction identities).
/// Return the complete digest closure in deterministic traversal order, excluding
/// config, metadata and artwork. Repeated references are allowed and deduplicated
/// by first use. The store independently checks exact agreement with the manifest.
/// Implementations are trusted validators, not untrusted content.
pub trait ConfigValidator {
    /// Platform family this implementation understands.
    fn platform(&self) -> &str;

    /// Extra payload codecs supported by this provider, with stored-size limits.
    fn payload_types(&self) -> &[PayloadType] {
        &[]
    }

    /// Validate a schema-1 TOML table and enumerate every referenced payload.
    /// Descriptors provide the sizes/types for digest-only config references.
    /// Providers may use bounded verified reads for transitive graph traversal.
    fn validate(
        &self,
        config: &toml::Table,
        descriptors: &[Descriptor],
        blobs: &dyn BlobSource,
    ) -> Result<Vec<Digest>>;
}

/// One local repository:tag reference and its immutable manifest descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub manifest: Descriptor,
}

/// Lexicographic page; pass `next_after` back as the next call's cursor.
/// Pages are snapshots of individual index reads; callers may refresh after edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagPage {
    pub tags: Vec<Tag>,
    pub next_after: Option<String>,
}

/// Resolved image with fully verified required payloads. Payloads remain streamed through blob handles.
#[derive(Clone, Debug)]
pub struct ResolvedImage {
    pub image: Descriptor,
    pub manifest: Manifest,
    pub config: Vec<u8>,
    pub metadata: Vec<u8>,
    /// Metadata degraded to a display projection and must not be rewritten.
    pub metadata_read_only: bool,
    /// Artwork missing from storage; this never prevents running the machine.
    pub missing_artwork: Vec<Descriptor>,
}

/// Caller-rooted, platform-neutral store. Handles retain no locks between calls.
/// All operations take a shared store lease before any short index lock.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreVersion {
    schema: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Layout {
    image_layout_version: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Index {
    schema_version: u32,
    media_type: String,
    manifests: Vec<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations: Option<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subject: Option<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_type: Option<Box<RawValue>>,
}

impl Store {
    /// Create or open layout version 1. Refuse newer formats and roots without
    /// local filesystem semantics. Existing index and content are never reset.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        host::require_local(root)?;
        let store = Self {
            root: root.canonicalize()?,
        };
        let version_path = store.root.join("store.toml");
        if version_path.exists() {
            store.check_version()?;
        }
        fs::create_dir_all(store.root.join("locks"))?;
        let _lease = store.lease()?;
        let _index_lock = host::probe_lock(&store.root.join("locks/index.lock"))?;
        if version_path.exists() {
            store.check_version()?;
        }
        for relative in ["images/blobs/sha256", "staging"] {
            fs::create_dir_all(store.root.join(relative))?;
        }
        host::probe_publish(&store.root.join("staging"))?;
        if !version_path.exists() {
            store.atomic_write(&version_path, b"schema = 1\n")?;
        }
        let layout_path = store.root.join("images/oci-layout");
        if layout_path.exists() {
            let layout: Layout = decode_json(&read_bounded(&layout_path, MAX_SMALL_BLOB)?)?;
            if layout.image_layout_version != "1.0.0" {
                return Err(invalid("unsupported OCI image layout version"));
            }
        } else {
            store.atomic_write(&layout_path, b"{\"imageLayoutVersion\":\"1.0.0\"}")?;
        }
        let index_path = store.root.join("images/index.json");
        if index_path.exists() {
            store.read_index()?;
        } else {
            store.write_index(&Index {
                schema_version: 2,
                media_type: media_type::INDEX.into(),
                manifests: Vec::new(),
                annotations: None,
                subject: None,
                artifact_type: None,
            })?;
        }
        for relative in [
            "images/blobs/sha256",
            "images/blobs",
            "images",
            "locks",
            "staging",
            "",
        ] {
            host::sync_dir(&store.root.join(relative))?;
        }
        Ok(store)
    }

    /// Root selected by the caller; no platform data-directory policy is applied.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stream and hash with a fixed 64 KiB buffer. A reader error is also the
    /// interruption-injection seam: dropping the staged file publishes no blob.
    pub fn import(&self, mut reader: impl Read, expected: Option<&Digest>) -> Result<Blob> {
        let _lease = self.lease()?;
        let mut temporary = host::Temporary::create(&self.root.join("staging"))?;
        let blob = copy_hash(&mut reader, Some(&mut temporary.file))?;
        if let Some(expected) = expected {
            check_digest(expected, &blob.digest)?;
        }
        temporary.file.sync_all()?;
        let destination = self.blob_path(&blob.digest);
        match host::publish_blob(&temporary.path, &destination) {
            Ok(()) => temporary.disarm(),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.verify_inner(&blob.digest, blob.size, "payload")?;
            }
            Err(error) => return Err(error),
        }
        host::sync_dir(&self.root.join("images/blobs/sha256"))?;
        host::sync_dir(&self.root.join("staging"))?;
        Ok(blob)
    }

    /// Copy a source file without linking it or retaining its path in content.
    pub fn import_file(&self, path: impl AsRef<Path>, expected: Option<&Digest>) -> Result<Blob> {
        self.import(File::open(path)?, expected)
    }

    /// Open immutable bytes after checking exact descriptor length. Use `verify`
    /// or `read_verified` when digest verification is required as well.
    pub fn open_blob(&self, digest: &Digest, expected_size: u64) -> Result<File> {
        let _lease = self.lease()?;
        self.open_inner(digest, expected_size, "payload")
    }

    /// Re-hash every stored byte, including arbitrarily large disk bases.
    pub fn verify(&self, digest: &Digest, expected_size: u64) -> Result<()> {
        let _lease = self.lease()?;
        self.verify_inner(digest, expected_size, "payload")
    }

    /// Store canonical manifest bytes, without creating a tag.
    pub fn import_manifest(&self, manifest: &Manifest) -> Result<Descriptor> {
        self.import_manifest_with_types(manifest, &[])
    }

    /// Store a manifest using an explicit provider codec declaration.
    pub fn import_manifest_with_types(
        &self,
        manifest: &Manifest,
        types: &[PayloadType],
    ) -> Result<Descriptor> {
        let bytes = manifest.encode_with_types(types)?;
        let blob = self.import(bytes.as_slice(), None)?;
        Ok(Descriptor::new(
            media_type::MANIFEST,
            blob.digest,
            blob.size,
        ))
    }

    /// Publish only a complete, verified closure. Hashing runs before the short
    /// index lock; the shared lease prevents a future GC from removing its blobs.
    /// A post-rename fsync failure may leave the new index visible: resolve before
    /// retrying after any I/O error, rather than assuming the old tag survived.
    pub fn update_tag(
        &self,
        name: &str,
        manifest: &Descriptor,
        expected: Option<&Digest>,
        validator: &dyn ConfigValidator,
    ) -> Result<()> {
        validate_tag(name)?;
        let _lease = self.lease()?;
        self.resolve_image_inner(manifest, validator, true)?;
        let _index_lock = host::exclusive_lock(&self.root.join("locks/index.lock"))?;
        self.update_tag_locked(name, manifest, expected)
    }

    /// Caller holds the store lease and index lock and has verified the closure.
    pub(crate) fn update_tag_locked(
        &self,
        name: &str,
        manifest: &Descriptor,
        expected: Option<&Digest>,
    ) -> Result<()> {
        validate_tag(name)?;
        let mut index = self.read_index()?;
        let position = index.manifests.iter().position(|raw| {
            local_descriptor(raw)
                .ok()
                .flatten()
                .is_some_and(|d| tag_name(&d) == Some(name))
        });
        let current = position
            .map(|i| {
                local_descriptor(&index.manifests[i])
                    .and_then(|d| d.ok_or_else(|| invalid("invalid local tag")))
            })
            .transpose()?
            .map(|d| d.digest);
        if current.as_ref() != expected {
            return Err(Error::Conflict { current });
        }
        let mut descriptor = manifest.clone();
        descriptor
            .annotations
            .insert("org.opencontainers.image.ref.name".into(), name.into());
        let replacement = to_raw_value(&descriptor).map_err(|e| invalid(e.to_string()))?;
        if let Some(position) = position {
            // Keep opaque OCI fields as raw JSON even when editing our own tag.
            let mut existing: BTreeMap<String, Box<RawValue>> =
                decode_json(index.manifests[position].get().as_bytes())?;
            if existing.contains_key("data") && current.as_ref() != Some(&manifest.digest) {
                return Err(invalid(
                    "cannot replace a tag with embedded OCI descriptor data",
                ));
            }
            let mut replacement: BTreeMap<String, Box<RawValue>> =
                decode_json(replacement.get().as_bytes())?;
            let mut annotations: BTreeMap<String, Box<RawValue>> = existing
                .get("annotations")
                .map(|raw| decode_json(raw.get().as_bytes()))
                .transpose()?
                .unwrap_or_default();
            if let Some(raw) = replacement.remove("annotations") {
                annotations.extend(decode_json::<BTreeMap<String, Box<RawValue>>>(
                    raw.get().as_bytes(),
                )?);
            }
            existing.insert(
                "annotations".into(),
                to_raw_value(&annotations).map_err(|e| invalid(e.to_string()))?,
            );
            existing.extend(replacement);
            index.manifests[position] =
                to_raw_value(&existing).map_err(|e| invalid(e.to_string()))?;
        } else {
            index.manifests.push(replacement);
        }
        self.write_index(&index)
    }

    /// Resolve once and pin the image digest. Required payloads are fully hashed;
    /// absent artwork is reported separately. No source-folder fallback is used.
    pub fn resolve(&self, name: &str, validator: &dyn ConfigValidator) -> Result<ResolvedImage> {
        validate_tag(name)?;
        let _lease = self.lease()?;
        let descriptor = self
            .read_index()?
            .manifests
            .iter()
            .filter_map(|raw| local_descriptor(raw).transpose())
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .find(|d| tag_name(d) == Some(name))
            .ok_or_else(|| Error::TagNotFound(name.into()))?;
        self.resolve_image_inner(&descriptor, validator, false)
    }

    /// List a bounded lexicographic page without opening or hashing image blobs.
    pub fn list_tags(&self, after: Option<&str>, limit: usize) -> Result<TagPage> {
        if limit == 0 || limit > 1000 {
            return Err(invalid("page size must be 1..=1000"));
        }
        let mut tags = self.all_tags()?;
        tags.retain(|tag| after.is_none_or(|after| tag.name.as_str() > after));
        let more = tags.len() > limit;
        tags.truncate(limit);
        let next_after = if more {
            tags.last().map(|tag| tag.name.clone())
        } else {
            None
        };
        Ok(TagPage { tags, next_after })
    }

    pub(crate) fn all_tags(&self) -> Result<Vec<Tag>> {
        let _lease = self.lease()?;
        self.tags_inner()
    }

    pub(crate) fn tags_inner(&self) -> Result<Vec<Tag>> {
        let mut tags: Vec<_> = self
            .read_index()?
            .manifests
            .iter()
            .filter_map(|raw| local_descriptor(raw).transpose())
            .map(|result| {
                result.map(|manifest| Tag {
                    name: tag_name(&manifest).unwrap_or_default().into(),
                    manifest,
                })
            })
            .collect::<Result<_>>()?;
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tags)
    }

    pub(crate) fn resolve_image_inner(
        &self,
        image: &Descriptor,
        validator: &dyn ConfigValidator,
        require_artwork: bool,
    ) -> Result<ResolvedImage> {
        image.validate()?;
        if image.media_type != media_type::MANIFEST {
            return Err(invalid("tag target is not a manifest"));
        }
        let manifest = Manifest::decode_with_types(
            &self.read_inner(image, MAX_SMALL_BLOB)?,
            validator.payload_types(),
        )?;
        let config = self.read_inner(&manifest.config, MAX_SMALL_BLOB)?;
        let table: toml::Table = std::str::from_utf8(&config)
            .map_err(|e| invalid(e.to_string()))?
            .parse()
            .map_err(|e: toml::de::Error| invalid(e.to_string()))?;
        let platform = table
            .get("platform")
            .and_then(toml::Value::as_str)
            .filter(|p| valid_platform(p))
            .ok_or_else(|| invalid("missing or invalid config platform"))?;
        if table.get("schema").and_then(toml::Value::as_integer) != Some(1)
            || platform != manifest.platform()?
            || platform != validator.platform()
        {
            return Err(invalid("config version or platform mismatch"));
        }
        let references = validator.validate(&table, &manifest.layers, self)?;
        if references.len() > MAX_OBJECTS {
            return Err(invalid("provider graph exceeds 10,000 objects"));
        }
        let metadata_descriptor = manifest
            .layers
            .iter()
            .find(|d| d.media_type == media_type::METADATA)
            .ok_or_else(|| invalid("missing metadata"))?;
        let metadata = self.read_inner(metadata_descriptor, MAX_SMALL_BLOB)?;
        let envelope = if require_artwork {
            let decoded = crate::metadata::Metadata::decode(&metadata, &manifest.layers)?;
            if decoded.platform != platform {
                return Err(invalid("metadata version or platform mismatch"));
            }
            decoded
        } else {
            match crate::metadata::Metadata::decode_for_display(&metadata, &manifest.layers) {
                Ok(decoded) if decoded.platform == platform => decoded,
                _ => crate::metadata::Metadata::unavailable(platform),
            }
        };
        let mut ordered = Vec::new();
        let mut seen = BTreeSet::new();
        for digest in references {
            let descriptor = manifest
                .layers
                .iter()
                .find(|d| d.digest == digest)
                .ok_or_else(|| invalid("referenced config blob absent from manifest"))?;
            if matches!(
                descriptor.media_type.as_str(),
                media_type::METADATA | media_type::PNG | media_type::JPEG
            ) {
                return Err(invalid("config references catalogue content"));
            }
            if seen.insert(digest.clone()) {
                ordered.push(digest);
            }
        }
        if seen.insert(metadata_descriptor.digest.clone()) {
            ordered.push(metadata_descriptor.digest.clone());
        }
        let artwork = if !envelope.is_read_only() {
            envelope.artwork().cloned().collect::<Vec<_>>()
        } else {
            let position = manifest
                .layers
                .iter()
                .position(|d| d.digest == metadata_descriptor.digest)
                .ok_or_else(|| invalid("missing metadata"))?;
            manifest.layers[position + 1..]
                .iter()
                .map(|d| d.digest.clone())
                .collect()
        };
        for digest in artwork {
            let descriptor = manifest
                .layers
                .iter()
                .find(|d| d.digest == digest)
                .ok_or_else(|| invalid("artwork absent from manifest"))?;
            if !matches!(
                descriptor.media_type.as_str(),
                media_type::PNG | media_type::JPEG
            ) {
                return Err(invalid("artwork has wrong media type"));
            }
            if seen.insert(digest.clone()) {
                ordered.push(digest);
            }
        }
        if ordered
            != manifest
                .layers
                .iter()
                .map(|d| d.digest.clone())
                .collect::<Vec<_>>()
        {
            return Err(invalid(
                "layers differ from the exact ordered config/metadata closure",
            ));
        }
        let mut missing_artwork = Vec::new();
        for descriptor in &manifest.layers {
            match self.verify_inner(&descriptor.digest, descriptor.size, &descriptor.media_type) {
                Err(Error::MissingBlob { .. }) if !require_artwork && is_artwork(descriptor) => {
                    missing_artwork.push(descriptor.clone())
                }
                result => result?,
            }
        }
        Ok(ResolvedImage {
            image: image.clone(),
            manifest,
            config,
            metadata,
            metadata_read_only: envelope.is_read_only(),
            missing_artwork,
        })
    }

    fn check_version(&self) -> Result<()> {
        let bytes = read_bounded(&self.root.join("store.toml"), MAX_SMALL_BLOB)?;
        let version: StoreVersion =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?)
                .map_err(|e| invalid(e.to_string()))?;
        if version.schema != 1 {
            return Err(invalid("unsupported store layout version"));
        }
        Ok(())
    }

    pub(crate) fn lease(&self) -> Result<File> {
        host::shared_lease(&self.root.join("locks/store.lock"))
    }

    fn blob_path(&self, digest: &Digest) -> PathBuf {
        self.root.join("images/blobs/sha256").join(digest.hex())
    }

    fn open_inner(&self, digest: &Digest, expected_size: u64, role: &str) -> Result<File> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(self.blob_path(digest)).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::MissingBlob {
                    digest: digest.clone(),
                    media_type: role.into(),
                }
            } else {
                Error::Io(error)
            }
        })?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("blob is not a regular file"));
        }
        check_size(expected_size, metadata.len())?;
        Ok(file)
    }

    fn verify_inner(&self, digest: &Digest, size: u64, role: &str) -> Result<()> {
        let mut file = self.open_inner(digest, size, role)?;
        let actual = copy_hash(&mut file, None)?;
        check_size(size, actual.size)?;
        check_digest(digest, &actual.digest)
    }

    fn read_inner(&self, descriptor: &Descriptor, limit: u64) -> Result<Vec<u8>> {
        descriptor.validate_limit(
            crate::artifact::neutral_size_limit(&descriptor.media_type).unwrap_or(limit),
        )?;
        if descriptor.size > limit {
            return Err(invalid("blob exceeds read limit"));
        }
        let file = self.open_inner(&descriptor.digest, descriptor.size, &descriptor.media_type)?;
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
        check_size(descriptor.size, bytes.len() as u64)?;
        check_digest(&descriptor.digest, &Digest::of(&bytes))?;
        Ok(bytes)
    }

    fn read_index(&self) -> Result<Index> {
        // Index size scales with the library, not with individual image documents.
        let index: Index = decode_json(&read_bounded(
            &self.root.join("images/index.json"),
            64 * MAX_SMALL_BLOB,
        )?)?;
        if index.schema_version != 2 || index.media_type != media_type::INDEX {
            return Err(invalid("unsupported OCI index"));
        }
        let mut names = BTreeSet::new();
        for raw in &index.manifests {
            if let Some(descriptor) = local_descriptor(raw)? {
                let name = tag_name(&descriptor)
                    .ok_or_else(|| invalid("tag annotation missing"))?
                    .to_owned();
                if !names.insert(name) {
                    return Err(invalid("duplicate local tag"));
                }
            }
        }
        Ok(index)
    }

    fn write_index(&self, index: &Index) -> Result<()> {
        let bytes = serde_json::to_vec(index).map_err(|e| invalid(e.to_string()))?;
        if bytes.len() as u64 > 64 * MAX_SMALL_BLOB {
            return Err(invalid("index exceeds 64 MiB"));
        }
        self.atomic_write(&self.root.join("images/index.json"), &bytes)
    }

    pub(crate) fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let mut temporary = host::Temporary::create(&self.root.join("staging"))?;
        temporary.file.write_all(bytes)?;
        temporary.file.sync_all()?;
        fs::rename(&temporary.path, path)?;
        temporary.disarm();
        host::sync_dir(
            path.parent()
                .ok_or_else(|| invalid("missing publication directory"))?,
        )?;
        host::sync_dir(&self.root.join("staging"))
    }
}

impl BlobSource for Store {
    fn read_verified(&self, descriptor: &Descriptor, limit: u64) -> Result<Vec<u8>> {
        let _lease = self.lease()?;
        self.read_inner(descriptor, limit)
    }
}

fn decode_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(invalid("document exceeds size limit"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("document exceeds size limit"));
    }
    Ok(bytes)
}

fn copy_hash(reader: &mut dyn Read, mut writer: Option<&mut File>) -> Result<Blob> {
    let mut buffer = [0; 64 * 1024];
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| invalid("blob length overflow"))?;
        hasher.update(&buffer[..count]);
        if let Some(writer) = writer.as_mut() {
            writer.write_all(&buffer[..count])?;
        }
    }
    Ok(Blob {
        digest: Digest::from_hash(hasher.finalize()),
        size,
    })
}

fn check_size(expected: u64, actual: u64) -> Result<()> {
    if expected != actual {
        return Err(Error::SizeMismatch { expected, actual });
    }
    Ok(())
}

fn check_digest(expected: &Digest, actual: &Digest) -> Result<()> {
    if expected != actual {
        return Err(Error::DigestMismatch {
            expected: expected.clone(),
            actual: actual.clone(),
        });
    }
    Ok(())
}

fn tag_name(descriptor: &Descriptor) -> Option<&str> {
    descriptor
        .annotations
        .get("org.opencontainers.image.ref.name")
        .map(String::as_str)
}

fn is_artwork(descriptor: &Descriptor) -> bool {
    matches!(
        descriptor.media_type.as_str(),
        media_type::PNG | media_type::JPEG
    )
}

fn raw_tag_name(value: &serde_json::Value) -> Option<&str> {
    value
        .get("annotations")?
        .get("org.opencontainers.image.ref.name")?
        .as_str()
}

fn local_descriptor(raw: &RawValue) -> Result<Option<Descriptor>> {
    let value: serde_json::Value = decode_json(raw.get().as_bytes())?;
    let Some(name) = raw_tag_name(&value).filter(|name| validate_tag(name).is_ok()) else {
        return Ok(None);
    };
    if value.get("mediaType").and_then(serde_json::Value::as_str) != Some(media_type::MANIFEST) {
        return Ok(None);
    }
    let descriptor: Descriptor = serde_json::from_str(raw.get())
        .map_err(|e| invalid(format!("invalid local index entry {name}: {e}")))?;
    descriptor.validate()?;
    Ok(Some(descriptor))
}

/// OCI Distribution Specification, Pulling Manifests: repository components are
/// `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*` and tags are
/// `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`.
fn validate_tag(value: &str) -> Result<()> {
    let (repository, tag) = value
        .rsplit_once(':')
        .ok_or_else(|| invalid("tag must be repository:tag"))?;
    fn component(mut bytes: &[u8]) -> bool {
        let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        loop {
            let count = bytes.iter().take_while(|b| alnum(b)).count();
            if count == 0 {
                return false;
            }
            bytes = &bytes[count..];
            if bytes.is_empty() {
                return true;
            }
            let separator = match bytes[0] {
                b'.' => 1,
                b'_' => {
                    if bytes.get(1) == Some(&b'_') {
                        2
                    } else {
                        1
                    }
                }
                b'-' => bytes.iter().take_while(|&&b| b == b'-').count(),
                _ => return false,
            };
            bytes = &bytes[separator..];
        }
    }
    if repository.is_empty()
        || repository.len() > 255
        || !repository.split('/').all(|part| component(part.as_bytes()))
        || tag.is_empty()
        || tag.len() > 128
        || !tag.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphanumeric() || b == b'_' || (i > 0 && matches!(b, b'.' | b'-'))
        })
    {
        return Err(invalid("invalid local repository:tag"));
    }
    let first = repository.split('/').next().unwrap_or_default();
    if repository.contains('/') && (first.contains('.') || first == "localhost") {
        return Err(invalid("local tags cannot include a registry host"));
    }
    Ok(())
}

impl Store {
    /// Open an initialized store without creating a directory or layout.
    pub fn open_existing(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        host::require_local(&root)?;
        let store = Self { root };
        store.check_version()?;
        let layout: Layout = decode_json(&read_bounded(
            &store.root.join("images/oci-layout"),
            MAX_SMALL_BLOB,
        )?)?;
        if layout.image_layout_version != "1.0.0" {
            return Err(invalid("unsupported OCI image layout version"));
        }
        store.read_index()?;
        Ok(store)
    }

    /// Return a checked immutable blob path for file-backed platform media.
    /// This checks length and file kind; resolution performs the streamed hash.
    pub fn blob_file_path(&self, digest: &Digest, size: u64) -> Result<PathBuf> {
        let _lease = self.lease()?;
        drop(self.open_inner(digest, size, "payload")?);
        Ok(self.blob_path(digest))
    }

    /// Reject invalid or occupied tag names before an expensive ingest.
    /// Publication must still use compare-and-swap to catch concurrent changes.
    pub fn check_new_tag(&self, name: &str) -> Result<()> {
        validate_tag(name)?;
        let _lease = self.lease()?;
        for raw in self.read_index()?.manifests {
            if let Some(d) = local_descriptor(&raw)? {
                if tag_name(&d) == Some(name) {
                    return Err(Error::Conflict {
                        current: Some(d.digest),
                    });
                }
            }
        }
        Ok(())
    }
}
