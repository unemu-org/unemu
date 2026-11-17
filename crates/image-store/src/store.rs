// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI Image Layout 1.0.0 store, closure validation and local tag transactions.
//! See machine-images §§2–3.1: immutable blobs precede mutable index publication.

use crate::{
    artifact::valid_platform, compat::AppVersion, filesystem as host, invalid, media_type,
    Descriptor, Digest, Error, Manifest, PayloadType, Result, MAX_OBJECTS, MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use serde_json::value::{to_raw_value, RawValue};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
    /// Open a verified file of the decoded payload for random access. A raw
    /// payload is its stored blob; a compressed one is decoded into the
    /// rebuildable materialized cache first.
    fn open_verified(&self, _descriptor: &Descriptor) -> Result<File> {
        Err(invalid("file-backed payload access unavailable"))
    }
}

/// Platform-owned validation of the common routing envelope's remaining fields.
/// See machine-images §§1–2: shared storage never interprets emulator hardware.
///
/// Implementations must reject unknown config fields/versions, host paths,
/// unsupported payloads and invalid bindings, and validate any transitive layer
/// descriptors (including depth <= 64, cycles and reconstruction identities).
/// Return the complete digest closure in deterministic traversal order, excluding
/// config, metadata and artwork. Repeated references are allowed and deduplicated
/// by first use. References name decoded content; the store maps them to stored
/// descriptors and independently checks exact agreement with the manifest.
/// Implementations are trusted validators, not untrusted content.
pub trait ConfigValidator {
    /// Artifact codec accepted by this validator.
    fn artifact_type(&self) -> &str {
        media_type::MACHINE
    }

    /// Platform family this implementation understands.
    fn platform(&self) -> &str;
    /// Validate reconstructed payloads once their verified file receipts exist.
    fn validate_resolved(&self, _image: &ResolvedImage, _store: &Store) -> Result<()> {
        Ok(())
    }

    /// Extra payload codecs supported by this provider, with size limits that
    /// bound both stored and decoded lengths. A `+zstd` entry declares that the
    /// matching raw payload type is compressible.
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

    /// The oldest release that reads this artifact by the compatibility
    /// tables (machine-images §5.6), from its manifest and config bytes. The
    /// default knows the shared rows only; a provider that owns config keys,
    /// vocabulary or payload types overrides it with its own rows.
    fn required_release(&self, manifest: &Manifest, config: &[u8]) -> Result<AppVersion> {
        crate::compat::shared_release(manifest, config)
    }
}

/// One local repository:tag reference and its immutable manifest descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub manifest: Descriptor,
}

/// Index annotation on a local tag's manifest descriptor: the registry host a
/// pull wrote the tag from, port included, such as `registry.unemu.dev` or
/// `nas.example:5000`. It tells a short tag pulled from the family default
/// registry apart from one made in this library. The record, not the name,
/// says where a tag came from: a person may type a host-qualified name for an
/// image made here, as with Docker. It lives in `index.json` only, never in a
/// manifest, so it is never pushed. Only a pull writes it, and a later write
/// that moves the tag to another image drops it.
pub const PULL_REGISTRY: &str = "unemu.pull.registry";

impl Tag {
    /// The registry a pull fetched this tag's image from ([`PULL_REGISTRY`]).
    /// `None` for a tag made in this library, and also for one pulled before
    /// pulls recorded their registry: the store cannot tell those apart and
    /// never guesses.
    pub fn pulled_from(&self) -> Option<&str> {
        self.manifest
            .annotations
            .get(PULL_REGISTRY)
            .map(String::as_str)
            .filter(|host| check_host(host).is_ok())
    }
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
    /// One receipt per stored descriptor digest.
    verified: BTreeMap<Digest, VerifiedBlob>,
    /// Decoded content digest to the stored digest of the first layer proving it.
    content: BTreeMap<Digest, Digest>,
    publication_ready: bool,
    store_root: PathBuf,
    receipt_image: Descriptor,
    receipt_manifest: Vec<u8>,
}
/// A fully hashed stored blob, held open with the metadata captured before
/// its hash. A compressed payload also carries its streamed decoded identity
/// and, once a caller asked for a file, the verified decoded cache file.
#[derive(Clone, Debug)]
struct VerifiedBlob {
    file: Arc<File>,
    metadata: fs::Metadata,
    /// SHA-256 of the stored bytes in `file`.
    digest: Digest,
    decoded: Option<Decoded>,
}
/// A verified decoded cache file with the metadata captured before its hash.
type CacheFile = (Arc<File>, fs::Metadata);
/// Decoded identity proven by streaming the stored frame through SHA-256.
#[derive(Clone, Debug)]
struct Decoded {
    digest: Digest,
    size: u64,
    /// Filled on the first file request and shared by clones of the receipt.
    file: Arc<Mutex<Option<CacheFile>>>,
}
impl VerifiedBlob {
    /// Refuse a receipt that does not prove exactly this descriptor's stored
    /// and decoded identities, for example one carried over from a receipt
    /// whose manifest was edited after verification.
    fn proves(&self, descriptor: &Descriptor) -> Result<()> {
        let decoded = match (&self.decoded, descriptor.is_compressed()) {
            (None, false) => true,
            (Some(decoded), true) => {
                descriptor
                    .decoded_digest()
                    .is_ok_and(|d| d == decoded.digest)
                    && descriptor.decoded_size().is_ok_and(|n| n == decoded.size)
            }
            _ => false,
        };
        if !decoded || self.digest != descriptor.digest || self.metadata.len() != descriptor.size {
            return Err(invalid(format!(
                "blob {}: verification receipt does not match its descriptor",
                descriptor.digest
            )));
        }
        Ok(())
    }

    fn check(&self) -> Result<()> {
        unchanged(&self.file, &self.metadata)
    }
}
/// Refuse a file whose identity, length or timestamps moved since `before`.
fn unchanged(file: &File, before: &fs::Metadata) -> Result<()> {
    let now = file.metadata()?;
    #[cfg(unix)]
    let same = {
        use std::os::unix::fs::MetadataExt;
        now.dev() == before.dev()
            && now.ino() == before.ino()
            && now.len() == before.len()
            && now.mtime() == before.mtime()
            && now.mtime_nsec() == before.mtime_nsec()
            && now.ctime() == before.ctime()
            && now.ctime_nsec() == before.ctime_nsec()
    };
    #[cfg(not(unix))]
    let same = now.len() == before.len() && now.modified()? == before.modified()?;
    if !same {
        return Err(invalid("payload changed after verification"));
    }
    Ok(())
}
impl ResolvedImage {
    /// A verified file whose content has this SHA-256, as config references
    /// name it: the raw blob, or for a compressed payload its decoded cache
    /// file, materialized on the first request only. Use this for media that
    /// needs random access; [`ResolvedImage::content_reader`] streams without
    /// writing anything. Refuses mutations since hashing.
    pub fn verified_file(&self, digest: &Digest) -> Result<(File, fs::Metadata)> {
        let (descriptor, receipt) = self.content_receipt(digest)?;
        receipt.check()?;
        match &receipt.decoded {
            None => Ok((receipt.file.try_clone()?, receipt.metadata.clone())),
            Some(decoded) => {
                let (file, metadata) = self.decoded_file(descriptor, receipt, decoded)?;
                Ok((file.try_clone()?, metadata))
            }
        }
    }

    /// Stream the decoded content with this SHA-256 without writing a file.
    pub fn content_reader(&self, digest: &Digest) -> Result<ContentReader> {
        let (descriptor, receipt) = self.content_receipt(digest)?;
        receipt.check()?;
        let stored = crate::codec::At::new(receipt.file.clone());
        let (inner, size): (Box<dyn Read>, u64) = match &receipt.decoded {
            None => (Box::new(stored), descriptor.size),
            Some(decoded) => (Box::new(crate::codec::decoder(stored)?), decoded.size),
        };
        Ok(ContentReader {
            inner,
            receipt: receipt.clone(),
            blob: descriptor.digest.clone(),
            digest: digest.clone(),
            size,
            hash: Sha256::new(),
            read: 0,
            done: false,
        })
    }

    fn content_receipt(&self, digest: &Digest) -> Result<(&Descriptor, &VerifiedBlob)> {
        let stored = self
            .content
            .get(digest)
            .ok_or_else(|| invalid("payload was not verified"))?;
        let descriptor = self
            .manifest
            .layers
            .iter()
            .find(|d| &d.digest == stored)
            .ok_or_else(|| invalid("payload was not verified"))?;
        let receipt = self
            .verified
            .get(stored)
            .ok_or_else(|| invalid("payload was not verified"))?;
        receipt.proves(descriptor)?;
        Ok((descriptor, receipt))
    }

    /// The decoded cache file of a compressed receipt, decoded on first use.
    fn decoded_file(
        &self,
        descriptor: &Descriptor,
        receipt: &VerifiedBlob,
        decoded: &Decoded,
    ) -> Result<CacheFile> {
        let mut slot = decoded
            .file
            .lock()
            .map_err(|_| invalid("decoded cache receipt poisoned"))?;
        if let Some((file, metadata)) = slot.as_ref() {
            unchanged(file, metadata).map_err(|e| named(descriptor, e))?;
            return Ok((file.clone(), metadata.clone()));
        }
        let store = Store {
            root: self.store_root.clone(),
        };
        let _lease = store.lease()?;
        let (file, metadata) = store
            .materialize(&decoded.digest, decoded.size, &receipt.file)
            .map_err(|e| named(descriptor, e))?;
        // The stored frame must not have changed while it was decoded.
        receipt.check()?;
        let file = Arc::new(file);
        *slot = Some((file.clone(), metadata.clone()));
        Ok((file, metadata))
    }
}

/// Sequential reader of a verified payload's decoded bytes: the raw blob, or
/// a compressed blob decoded on the fly. It reads at its own offset and, at
/// the end of the stream, refuses content whose length or SHA-256 differs
/// from the descriptor, or whose stored file changed since verification.
pub struct ContentReader {
    inner: Box<dyn Read>,
    receipt: VerifiedBlob,
    blob: Digest,
    digest: Digest,
    size: u64,
    hash: Sha256,
    read: u64,
    done: bool,
}
impl ContentReader {
    /// Decoded length the stream must deliver.
    pub fn size(&self) -> u64 {
        self.size
    }
}
impl Read for ContentReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let blob = &self.blob;
        let count = self
            .inner
            .read(bytes)
            .map_err(|e| io::Error::new(e.kind(), format!("blob {blob}: {e}")))?;
        self.read = self.read.saturating_add(count as u64);
        if self.read > self.size {
            return Err(io::Error::other(format!(
                "blob {blob}: payload exceeds its declared size"
            )));
        }
        self.hash.update(&bytes[..count]);
        if count == 0 && !self.done {
            self.done = true;
            let actual = Digest::from_hash(std::mem::take(&mut self.hash).finalize());
            if self.read != self.size || actual != self.digest {
                return Err(io::Error::other(format!(
                    "blob {blob}: content differs from its descriptor"
                )));
            }
            self.receipt.check().map_err(io::Error::other)?;
        }
        Ok(count)
    }
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
    pub fn import(&self, reader: impl Read, expected: Option<&Digest>) -> Result<Blob> {
        self.import_sized(reader, expected, None)
    }

    /// Import with an optional exact length, checked before publication so a
    /// source that changed size leaves no blob behind.
    pub(crate) fn import_sized(
        &self,
        mut reader: impl Read,
        expected: Option<&Digest>,
        size: Option<u64>,
    ) -> Result<Blob> {
        let _lease = self.lease()?;
        let mut temporary = host::Temporary::create(&self.root.join("staging"))?;
        let blob = copy_hash(&mut reader, Some(&mut temporary.file))?;
        if size.is_some_and(|n| n != blob.size) {
            return Err(invalid("source size changed during ingest"));
        }
        if let Some(expected) = expected {
            check_digest(expected, &blob.digest)?;
        }
        temporary.file.sync_all()?;
        self.publish_staged(&mut temporary, &blob)?;
        Ok(blob)
    }

    /// Rename a synced staged file to its content address without replacing
    /// an existing blob; an existing one is verified instead.
    pub(crate) fn publish_staged(
        &self,
        temporary: &mut host::Temporary,
        blob: &Blob,
    ) -> Result<()> {
        let destination = self.blob_path(&blob.digest);
        match host::publish_blob(&temporary.path, &destination) {
            Ok(()) => temporary.disarm(),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.verify_inner(&blob.digest, blob.size, "payload")?;
            }
            Err(error) => return Err(error),
        }
        host::sync_dir(&self.root.join("images/blobs/sha256"))?;
        host::sync_dir(&self.root.join("staging"))
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

    /// Re-hash a payload's stored bytes and, when it is compressed, stream its
    /// decoded bytes through SHA-256 against the descriptor. Writes nothing.
    pub fn verify_descriptor(&self, descriptor: &Descriptor) -> Result<()> {
        let _lease = self.lease()?;
        self.verify_payload(descriptor).map(|_| ())
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
        self.update_tag_from(name, manifest, expected, validator, None)
    }

    /// [`Store::update_tag`] for a pull: the tag also records `registry`, the
    /// host it was pulled from ([`PULL_REGISTRY`]). Only a pull writes that
    /// record; every other write leaves it off the tag it writes.
    pub fn update_pulled_tag(
        &self,
        name: &str,
        manifest: &Descriptor,
        expected: Option<&Digest>,
        validator: &dyn ConfigValidator,
        registry: &str,
    ) -> Result<()> {
        Self::validate_registry_host(registry)?;
        self.update_tag_from(name, manifest, expected, validator, Some(registry))
    }

    fn update_tag_from(
        &self,
        name: &str,
        manifest: &Descriptor,
        expected: Option<&Digest>,
        validator: &dyn ConfigValidator,
        pulled: Option<&str>,
    ) -> Result<()> {
        validate_tag(name)?;
        let _lease = self.lease()?;
        self.resolve_image_inner(manifest, validator, true)?;
        let _index_lock = host::exclusive_lock(&self.root.join("locks/index.lock"))?;
        self.update_tag_locked(name, manifest, expected, pulled)
    }

    /// Caller holds the store lease and index lock and has verified the closure.
    /// `pulled` is the registry a pull fetched the image from; any other write
    /// passes `None`. A descriptor read from another tag carries that tag's
    /// pull record, which never travels to the tag written here.
    pub(crate) fn update_tag_locked(
        &self,
        name: &str,
        manifest: &Descriptor,
        expected: Option<&Digest>,
        pulled: Option<&str>,
    ) -> Result<()> {
        validate_tag(name)?;
        let mut index = self.read_index()?;
        let position = index.manifests.iter().position(|raw| {
            local_descriptor(raw)
                .ok()
                .flatten()
                .is_some_and(|d| tag_name(&d) == Some(name))
        });
        let current_image = position
            .map(|i| {
                local_descriptor(&index.manifests[i])
                    .and_then(|d| d.ok_or_else(|| invalid("invalid local tag")))
            })
            .transpose()?;
        let current = current_image.as_ref().map(|d| d.digest.clone());
        if current.as_ref() != expected {
            return Err(Error::Conflict { current });
        }
        // Even a caller that confirmed the current digest cannot move a version
        // tag, nor bind a deleted one to another image, nor move a set's tag.
        let retired = self.read_retired_inner()?;
        refuse_version_move(
            name,
            current.as_ref().or(retired.tags.get(name)),
            Some(&manifest.digest),
        )?;
        if let Some(current) = current_image.filter(|d| d.digest != manifest.digest) {
            self.refuse_media_set_move(name, &current, Some(manifest))?;
        }
        let mut descriptor = manifest.clone();
        descriptor
            .annotations
            .insert("org.opencontainers.image.ref.name".into(), name.into());
        descriptor.annotations.remove(PULL_REGISTRY);
        if let Some(registry) = pulled {
            descriptor
                .annotations
                .insert(PULL_REGISTRY.into(), registry.into());
        }
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
            // Where the old image came from says nothing about the new one.
            if current.as_ref() != Some(&manifest.digest) {
                annotations.remove(PULL_REGISTRY);
            }
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
        self.resolve_image_reusing(image, validator, require_artwork, None, None)
    }
    fn resolve_image_reusing(
        &self,
        image: &Descriptor,
        validator: &dyn ConfigValidator,
        require_artwork: bool,
        reuse: Option<&ResolvedImage>,
        progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<ResolvedImage> {
        image.validate()?;
        if image.media_type != media_type::MANIFEST {
            return Err(invalid("tag target is not a manifest"));
        }
        let bytes = self.read_inner(image, MAX_SMALL_BLOB)?;
        // The marker comes first: an image a newer release wrote is refused
        // with the release it needs, never with a decoder message.
        crate::compat::Requirement::read(&bytes).check(&match tag_name(image) {
            Some(name) => name.to_owned(),
            None => image.digest.to_string(),
        })?;
        let manifest = Manifest::decode_with_types(&bytes, validator.payload_types())?;
        if manifest.artifact_type() != validator.artifact_type() {
            return Err(invalid("artifact type does not match validator"));
        }
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
            .find(|d| d.media_type == media_type::METADATA);
        let metadata = match metadata_descriptor {
            Some(d) => self.read_inner(d, MAX_SMALL_BLOB)?,
            None if manifest.artifact_type() == media_type::FIRMWARE => Vec::new(),
            None => return Err(invalid("missing metadata")),
        };
        let envelope = if manifest.artifact_type() == media_type::FIRMWARE {
            crate::metadata::Metadata::new(platform)
        } else if require_artwork {
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
                .find(|d| d.names(&digest))
                .ok_or_else(|| invalid("referenced config blob absent from manifest"))?;
            if matches!(
                descriptor.media_type.as_str(),
                media_type::METADATA | media_type::PNG | media_type::JPEG
            ) {
                return Err(invalid("config references catalogue content"));
            }
            if seen.insert(descriptor.digest.clone()) {
                ordered.push(descriptor.digest.clone());
            }
        }
        if let Some(d) = metadata_descriptor {
            if seen.insert(d.digest.clone()) {
                ordered.push(d.digest.clone());
            }
        }
        let artwork = if !envelope.is_read_only() {
            envelope.artwork().cloned().collect::<Vec<_>>()
        } else {
            let position = manifest
                .layers
                .iter()
                .position(|d| Some(d) == metadata_descriptor)
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
        let mut verified = BTreeMap::new();
        let mut content = BTreeMap::new();
        for (position, descriptor) in manifest.layers.iter().enumerate() {
            if let Some(progress) = progress {
                progress(position, manifest.layers.len());
            }
            if let Some(source) = reuse {
                if source.manifest.layers.iter().any(|old| old == descriptor) {
                    if let Some(receipt) = source.verified.get(&descriptor.digest) {
                        receipt.proves(descriptor)?;
                        receipt.check()?;
                        content
                            .entry(descriptor.decoded_digest()?)
                            .or_insert_with(|| descriptor.digest.clone());
                        verified.insert(descriptor.digest.clone(), receipt.clone());
                        continue;
                    }
                }
            }
            match self.verify_payload(descriptor) {
                Err(Error::MissingBlob { .. }) if !require_artwork && is_artwork(descriptor) => {
                    missing_artwork.push(descriptor.clone())
                }
                Ok(file) => {
                    content
                        .entry(descriptor.decoded_digest()?)
                        .or_insert_with(|| descriptor.digest.clone());
                    verified.insert(descriptor.digest.clone(), file);
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(progress) = progress {
            progress(manifest.layers.len(), manifest.layers.len());
        }
        let receipt_manifest = serde_json::to_vec(&manifest).map_err(|e| invalid(e.to_string()))?;
        let image = ResolvedImage {
            image: image.clone(),
            manifest,
            config,
            metadata,
            metadata_read_only: envelope.is_read_only(),
            missing_artwork,
            verified,
            content,
            publication_ready: require_artwork,
            store_root: self.root.clone(),
            receipt_image: image.clone(),
            receipt_manifest,
        };
        validator.validate_resolved(&image, self)?;
        Ok(image)
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

    fn verify_file(&self, digest: &Digest, size: u64, role: &str) -> Result<VerifiedBlob> {
        let mut file = self.open_inner(digest, size, role)?;
        let metadata = file.metadata()?;
        #[cfg(feature = "test-support")]
        VERIFICATIONS.with(|counts| *counts.borrow_mut().entry(digest.clone()).or_default() += 1);
        let actual = copy_hash(&mut file, None)?;
        check_size(size, actual.size)?;
        check_digest(digest, &actual.digest)?;
        let verified = VerifiedBlob {
            file: Arc::new(file),
            metadata,
            digest: digest.clone(),
            decoded: None,
        };
        verified.check()?;
        Ok(verified)
    }
    /// Verify stored bytes and, for a compressed payload, stream the decoded
    /// bytes through SHA-256. Nothing is written; the receipt binds to the
    /// stored blob. Failures that are not typed mismatches name the blob.
    fn verify_payload(&self, descriptor: &Descriptor) -> Result<VerifiedBlob> {
        let mut stored = self
            .verify_file(&descriptor.digest, descriptor.size, &descriptor.media_type)
            .map_err(|e| named(descriptor, e))?;
        if !descriptor.is_compressed() {
            return Ok(stored);
        }
        let decoded = (|| -> Result<Decoded> {
            let digest = descriptor.decoded_digest()?;
            let size = descriptor.decoded_size()?;
            crate::codec::verify_decoded(&stored.file, &digest, size)?;
            stored.check()?;
            Ok(Decoded {
                digest,
                size,
                file: Arc::default(),
            })
        })();
        stored.decoded = Some(decoded.map_err(|e| named(descriptor, e))?);
        Ok(stored)
    }

    fn verify_inner(&self, digest: &Digest, size: u64, role: &str) -> Result<()> {
        self.verify_file(digest, size, role).map(|_| ())
    }

    fn read_inner(&self, descriptor: &Descriptor, limit: u64) -> Result<Vec<u8>> {
        descriptor.validate_limit(
            crate::artifact::neutral_size_limit(&descriptor.media_type).unwrap_or(i64::MAX as u64),
        )?;
        if descriptor.decoded_size()? > limit {
            return Err(invalid("blob exceeds read limit"));
        }
        if descriptor.is_compressed() {
            // Bounded payloads decode into memory; no cache file is needed.
            let stored = self
                .verify_file(&descriptor.digest, descriptor.size, &descriptor.media_type)
                .map_err(|e| named(descriptor, e))?;
            return self
                .decode_bounded(descriptor, &stored.file)
                .map_err(|e| named(descriptor, e));
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
    fn open_verified(&self, descriptor: &Descriptor) -> Result<File> {
        let _lease = self.lease()?;
        let verified = self.verify_payload(descriptor)?;
        let mut file = match &verified.decoded {
            None => verified.file.try_clone()?,
            Some(decoded) => {
                self.materialize(&decoded.digest, decoded.size, &verified.file)
                    .map_err(|e| named(descriptor, e))?
                    .0
            }
        };
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

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

/// Name the stored blob in a compressed payload's failure. Typed errors stay
/// typed so callers can act on them: a missing blob, a digest or size
/// mismatch, and a full store.
fn named(descriptor: &Descriptor, error: Error) -> Error {
    match error {
        Error::MissingBlob { .. } | Error::DigestMismatch { .. } | Error::SizeMismatch { .. } => {
            error
        }
        Error::Io(ref e) if e.kind() == io::ErrorKind::StorageFull => error,
        _ if descriptor.is_compressed() => invalid(format!("blob {}: {error}", descriptor.digest)),
        _ => error,
    }
}

pub(crate) fn check_size(expected: u64, actual: u64) -> Result<()> {
    if expected != actual {
        return Err(Error::SizeMismatch { expected, actual });
    }
    Ok(())
}

pub(crate) fn check_digest(expected: &Digest, actual: &Digest) -> Result<()> {
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

/// A tag shaped like a semantic version names one image forever: once it
/// exists, only the same digest (`replacement`, when known) may be written to it
/// again. `replacement = None` refuses any update of an existing version tag.
fn version_shaped(name: &str) -> bool {
    name.rsplit_once(':')
        .is_some_and(|(_, tag)| Store::is_version_tag(tag))
}

fn refuse_version_move(
    name: &str,
    current: Option<&Digest>,
    replacement: Option<&Digest>,
) -> Result<()> {
    let version = version_shaped(name);
    match current {
        Some(current) if version && replacement != Some(current) => Err(invalid(format!(
            "{name} already names {current}; version tags never move"
        ))),
        _ => Ok(()),
    }
}

impl Store {
    /// A tag of an installation media set names one set forever unless it is
    /// an alias ([`crate::media_set::is_alias_tag`]), even when it is not a
    /// semantic version: sets are pulled by tag, and a moved tag would hand
    /// different media out under a name already in use. The tag cannot move
    /// away from a set, and no set can take a tag that names another image.
    /// Caller holds the store lease.
    fn refuse_media_set_move(
        &self,
        name: &str,
        current: &Descriptor,
        replacement: Option<&Descriptor>,
    ) -> Result<()> {
        let (repository, tag) = name.rsplit_once(':').unwrap_or(("", name));
        if crate::media_set::is_alias_tag(tag) {
            return Ok(());
        }
        let is_set = |image: &Descriptor| -> Result<bool> {
            let bytes = self.read_inner(image, MAX_SMALL_BLOB)?;
            Ok(Manifest::decode_for_summary(&bytes)?.artifact_type() == media_type::MEDIA_SET)
        };
        if is_set(current)? || replacement.map(is_set).transpose()? == Some(true) {
            return Err(invalid(format!(
                "{name} already names {}; tags of installation media sets never move; tag the changed set with a new revision such as {repository}:{}",
                current.digest,
                crate::media_set::next_revision(tag)
            )));
        }
        Ok(())
    }
}

/// Split a local tag or repository into its registry host, if it names one,
/// and the rest. As in Docker references, a first component followed by `/`
/// names a host when it contains a `.` or a `:` (a port), or is `localhost`.
fn split_host(name: &str) -> (Option<&str>, &str) {
    match name.split_once('/') {
        Some((first, rest)) if first.contains(['.', ':']) || first == "localhost" => {
            (Some(first), rest)
        }
        _ => (None, name),
    }
}

/// A registry host as local tags write it: lowercase DNS labels of at most 63
/// characters or an IPv4 address, or a bracketed IPv6 address (RFC 3986
/// §3.2.2), with an optional decimal port from 1 to 65535 without leading
/// zeros. Reference parsing lowercases hosts; with the port rules this gives
/// one registry one spelling in the library.
fn check_host(host: &str) -> std::result::Result<(), &'static str> {
    let (name, port) = match host.strip_prefix('[') {
        Some(inner) => {
            let (address, rest) = inner
                .split_once(']')
                .ok_or("the IPv6 address has no closing ]")?;
            if !ipv6_shaped(address) {
                return Err("an IPv6 address is eight groups of one to four lowercase hexadecimal digits, or fewer around one ::");
            }
            let port = match rest {
                "" => None,
                rest => Some(rest.strip_prefix(':').ok_or("a port follows a colon")?),
            };
            (None, port)
        }
        None => match host.rsplit_once(':') {
            Some((name, port)) => (Some(name), Some(port)),
            None => (Some(host), None),
        },
    };
    if let Some(name) = name {
        let label = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if name.is_empty() || name.len() > 253 || !name.split('.').all(label) {
            return Err("a host name is dot-separated labels of up to 63 lowercase letters, digits and inner hyphens");
        }
    }
    if let Some(port) = port {
        if !port.bytes().all(|b| b.is_ascii_digit())
            || port.starts_with('0')
            || !matches!(port.parse::<u16>(), Ok(1..))
        {
            return Err("a port is a number from 1 to 65535 without leading zeros");
        }
    }
    Ok(())
}

/// RFC 4291 §2.2 text form, lowercase and without an embedded IPv4 address:
/// eight groups of one to four hexadecimal digits, or fewer with one `::`
/// standing for the missing ones.
fn ipv6_shaped(address: &str) -> bool {
    if !address
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f' | b':'))
    {
        return false;
    }
    let groups = |part: &str| -> Option<usize> {
        if part.is_empty() {
            return Some(0);
        }
        part.split(':').try_fold(0, |n, group| {
            (1..=4).contains(&group.len()).then_some(n + 1)
        })
    };
    match address.split_once("::") {
        Some((head, tail)) => {
            !tail.contains("::")
                && matches!((groups(head), groups(tail)), (Some(a), Some(b)) if a + b <= 7)
        }
        None => groups(address) == Some(8),
    }
}

/// OCI Distribution Specification, Pulling Manifests: repository components are
/// `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*` and tags are
/// `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`. A registry host may come first
/// ([`split_host`]): pulls from registries other than the family default write
/// one, and a person may type one; the app never derives one for an image
/// made here, and the name alone never says the tag was pulled
/// ([`PULL_REGISTRY`]). The name, host included, is at most 255 characters,
/// the limit clients put on a full reference.
fn validate_tag(value: &str) -> Result<()> {
    let (name, tag) = value
        .rsplit_once(':')
        .ok_or_else(|| invalid("tag must be repository:tag"))?;
    let (host, repository) = split_host(name);
    if let Some(host) = host {
        check_host(host).map_err(|why| {
            invalid(format!(
                "{value}: {host} is not a valid registry host: {why}, such as nas.example:5000"
            ))
        })?;
    }
    if name.len() > 255 {
        return Err(invalid(format!(
            "{value}: the name, registry host included, is longer than 255 characters"
        )));
    }
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
        || !repository.split('/').all(|part| component(part.as_bytes()))
        || tag.is_empty()
        || tag.len() > 128
        || !tag.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphanumeric() || b == b'_' || (i > 0 && matches!(b, b'.' | b'-'))
        })
    {
        return Err(invalid("invalid local repository:tag"));
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

    /// Path of the verified file a resolved image holds for this content
    /// digest: the raw blob, or the decoded cache file of a compressed payload,
    /// which this materializes on the first request. Refuses a path whose file
    /// is no longer the verified one.
    pub fn resolved_file_path(&self, image: &ResolvedImage, digest: &Digest) -> Result<PathBuf> {
        let (descriptor, receipt) = image.content_receipt(digest)?;
        receipt.check()?;
        let (path, verified) = match &receipt.decoded {
            Some(decoded) => (
                self.decoded_path(&decoded.digest),
                image.decoded_file(descriptor, receipt, decoded)?.1,
            ),
            None => (self.blob_path(&descriptor.digest), receipt.metadata.clone()),
        };
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let current = options.open(&path)?.metadata()?;
        if !current.is_file() || current.len() != verified.len() {
            return Err(invalid("payload path changed after verification"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if current.dev() != verified.dev() || current.ino() != verified.ino() {
                return Err(invalid("payload path changed after verification"));
            }
        }
        Ok(path)
    }

    /// Reject a tag update before an expensive ingest: an occupied version tag
    /// never moves, nor does an occupied tag of an installation media set that
    /// is not an alias, and any other occupied tag moves only from the digest
    /// the user confirmed (`expected`). An absent tag needs `expected = None`.
    /// Publication still uses compare-and-swap to catch concurrent changes.
    /// A deleted version tag still refuses any digest other than `replacement`
    /// (`None` when the new image is not known yet).
    pub fn check_tag_target(
        &self,
        name: &str,
        expected: Option<&Digest>,
        replacement: Option<&Digest>,
    ) -> Result<()> {
        validate_tag(name)?;
        let current_image = match self.tag_descriptor(name) {
            Ok(current) => Some(current),
            Err(Error::TagNotFound(_)) => None,
            Err(error) => return Err(error),
        };
        let current = current_image.as_ref().map(|d| d.digest.clone());
        let _lease = self.lease()?;
        let retired = self.read_retired_inner()?;
        refuse_version_move(
            name,
            current.as_ref().or(retired.tags.get(name)),
            replacement.filter(|_| current.is_none()),
        )?;
        if let Some(current) = &current_image {
            self.refuse_media_set_move(name, current, None)?;
        }
        if current.as_ref() != expected {
            return Err(Error::Conflict { current });
        }
        Ok(())
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

impl Store {
    /// Resolve a pinned descriptor independently of mutable tag names.
    pub fn resolve_descriptor(
        &self,
        descriptor: &Descriptor,
        validator: &dyn ConfigValidator,
    ) -> Result<ResolvedImage> {
        let _lease = self.lease()?;
        self.resolve_image_inner(descriptor, validator, false)
    }
    /// Remove only index metadata, with compare-and-swap protection.
    pub fn remove_tag(&self, name: &str, expected: &Digest) -> Result<()> {
        validate_tag(name)?;
        let _lease = self.lease()?;
        let _lock = host::exclusive_lock(&self.root.join("locks/index.lock"))?;
        let mut index = self.read_index()?;
        let position = index
            .manifests
            .iter()
            .position(|raw| {
                local_descriptor(raw)
                    .ok()
                    .flatten()
                    .is_some_and(|d| tag_name(&d) == Some(name))
            })
            .ok_or_else(|| Error::TagNotFound(name.into()))?;
        let current = local_descriptor(&index.manifests[position])?
            .ok_or_else(|| invalid("invalid local tag"))?;
        if &current.digest != expected {
            return Err(Error::Conflict {
                current: Some(current.digest),
            });
        }
        if version_shaped(name) {
            // Recorded before the index changes: a crash in between leaves a
            // name bound to the digest it already names, never a free version.
            let mut retired = self.read_retired_inner()?;
            retired.tags.insert(name.into(), current.digest.clone());
            self.write_retired_locked(&retired)?;
        }
        index.manifests.remove(position);
        self.write_index(&index)
    }
}

/// Deleted version tags and the digest each named, `local/retired-version-tags.toml`.
/// A version tag names one image forever (machine-images §3), so its name
/// stays bound after its index entry goes.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredTags {
    schema: u32,
    #[serde(default)]
    tags: BTreeMap<String, Digest>,
}

const RETIRED_TAGS: &str = "local/retired-version-tags.toml";

impl Store {
    /// Caller holds the store lease.
    fn read_retired_inner(&self) -> Result<RetiredTags> {
        let bytes = match read_bounded(&self.root.join(RETIRED_TAGS), 16 * MAX_SMALL_BLOB) {
            Ok(bytes) => bytes,
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(RetiredTags {
                    schema: 1,
                    tags: BTreeMap::new(),
                })
            }
            Err(error) => return Err(error),
        };
        let retired: RetiredTags =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?)
                .map_err(|e| invalid(e.to_string()))?;
        if retired.schema != 1 {
            return Err(invalid("unsupported retired version tag record"));
        }
        Ok(retired)
    }
    /// Caller holds the store lease and the index lock.
    fn write_retired_locked(&self, retired: &RetiredTags) -> Result<()> {
        let bytes = toml::to_string(retired).map_err(|e| invalid(e.to_string()))?;
        fs::create_dir_all(self.root.join("local"))?;
        host::sync_dir(&self.root)?;
        self.atomic_write(&self.root.join(RETIRED_TAGS), bytes.as_bytes())
    }
}

impl Store {
    /// Check known staging requirements before a streamed disk operation. Other
    /// writers can still exhaust the disk later; normal write errors remain fatal.
    pub fn require_space(&self, bytes: u64) -> Result<()> {
        let _lease = self.lease()?;
        host::require_space(self.root(), bytes)
    }
}

#[cfg(feature = "test-support")]
thread_local! { static VERIFICATIONS: std::cell::RefCell<BTreeMap<Digest, usize>> = const { std::cell::RefCell::new(BTreeMap::new()) }; }
/// Full SHA-256 passes over one stored object on this thread.
#[cfg(feature = "test-support")]
pub fn verification_passes(digest: &Digest) -> usize {
    VERIFICATIONS.with(|counts| counts.borrow().get(digest).copied().unwrap_or(0))
}

impl Store {
    /// Verify the complete publication closure, including metadata and artwork.
    pub fn verify_publication(
        &self,
        descriptor: &Descriptor,
        validator: &dyn ConfigValidator,
    ) -> Result<ResolvedImage> {
        let _lease = self.lease()?;
        self.resolve_image_inner(descriptor, validator, true)
    }
    /// Resolve only the tag pointer; payload verification belongs to the pinned resolution.
    pub fn tag_descriptor(&self, name: &str) -> Result<Descriptor> {
        validate_tag(name)?;
        let _lease = self.lease()?;
        self.tags_inner()?
            .into_iter()
            .find(|t| t.name == name)
            .map(|t| t.manifest)
            .ok_or_else(|| Error::TagNotFound(name.into()))
    }
    /// Atomically persist a bounded local provenance document outside image blobs.
    pub fn write_local_record(&self, digest: &Digest, bytes: &[u8]) -> Result<()> {
        let _lease = self.lease()?;
        let _lock = host::exclusive_lock(&self.root.join("locks/index.lock"))?;
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("local record too large"));
        }
        let directory = self.root.join("local/imports");
        fs::create_dir_all(&directory)?;
        host::sync_dir(self.root())?;
        host::sync_dir(&self.root.join("local"))?;
        self.atomic_write(&directory.join(format!("{}.toml", digest.hex())), bytes)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Store {
    /// Tag an image without reading its closure, as a newer release tags an
    /// image this build cannot read in a store both share. Only for tests:
    /// every other write validates the closure first.
    pub fn tag_as_newer_release(&self, name: &str, manifest: &Descriptor) -> Result<()> {
        validate_tag(name)?;
        let _lease = self.lease()?;
        let _index_lock = host::exclusive_lock(&self.root.join("locks/index.lock"))?;
        self.update_tag_locked(name, manifest, None, None)
    }
}

impl Store {
    /// Publish a strict verification receipt after rechecking open-file identity.
    pub fn update_verified_tag(
        &self,
        name: &str,
        image: &ResolvedImage,
        expected: Option<&Digest>,
    ) -> Result<()> {
        if image.image != image.receipt_image
            || serde_json::to_vec(&image.manifest).map_err(|e| invalid(e.to_string()))?
                != image.receipt_manifest
        {
            return Err(invalid("verification receipt was modified"));
        }
        if image.store_root != self.root {
            return Err(invalid("verification receipt belongs to another store"));
        }
        if !image.publication_ready {
            return Err(invalid(
                "publication requires strict metadata and artwork verification",
            ));
        }
        let _lease = self.lease()?;
        self.verify_inner(&image.image.digest, image.image.size, media_type::MANIFEST)?;
        self.verify_inner(
            &image.manifest.config.digest,
            image.manifest.config.size,
            media_type::CONFIG,
        )?;
        for descriptor in &image.manifest.layers {
            let receipt = image
                .verified
                .get(&descriptor.digest)
                .ok_or_else(|| invalid("missing receipt"))?;
            receipt.proves(descriptor)?;
            receipt.check()?;
            let verified = receipt.file.try_clone()?;
            let current =
                self.open_inner(&descriptor.digest, descriptor.size, &descriptor.media_type)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let a = verified.metadata()?;
                let b = current.metadata()?;
                if a.dev() != b.dev() || a.ino() != b.ino() {
                    return Err(invalid("blob path changed after verification"));
                }
            }
        }
        let _lock = host::exclusive_lock(&self.root.join("locks/index.lock"))?;
        self.update_tag_locked(name, &image.image, expected, None)
    }
}

impl Store {
    /// Reuse unchanged immutable inputs from this operation's source verification.
    pub fn verify_publication_reusing(
        &self,
        descriptor: &Descriptor,
        validator: &dyn ConfigValidator,
        source: &ResolvedImage,
    ) -> Result<ResolvedImage> {
        if source.store_root != self.root || !source.publication_ready {
            return Err(invalid("source publication receipt is unavailable"));
        }
        let _lease = self.lease()?;
        self.resolve_image_reusing(descriptor, validator, true, Some(source), None)
    }
}

impl Store {
    /// A UI worker reports busy rather than waiting for an exclusive store owner.
    pub fn try_read_lease(&self) -> Result<File> {
        let file = host::lock_file(&self.root.join("locks/store.lock"))?;
        file.try_lock_shared()
            .map_err(|error| invalid(format!("store busy: {error}")))?;
        Ok(file)
    }
    /// Resolve each payload once while reporting host preparation progress.
    pub fn resolve_with_progress(
        &self,
        image: &Descriptor,
        validator: &dyn ConfigValidator,
        progress: &dyn Fn(usize, usize),
    ) -> Result<ResolvedImage> {
        let _lease = self.try_read_lease()?;
        self.resolve_image_reusing(image, validator, false, None, Some(progress))
    }
    /// Validate an import reference before reading sources or creating blobs.
    pub fn validate_tag_name(name: &str) -> Result<()> {
        validate_tag(name)
    }
    /// Check a registry host as local tags and pull records write it:
    /// lowercase, with an optional port from 1 to 65535 without leading
    /// zeros, so that one registry has one spelling.
    pub fn validate_registry_host(host: &str) -> Result<()> {
        check_host(host)
            .map_err(|why| invalid(format!("{host} is not a valid registry host: {why}")))
    }
    /// The registry host a local tag or repository names first, port
    /// included, such as `nas.example:5000` in
    /// `nas.example:5000/private/x:1`; `None` for a name without one.
    pub fn tag_host(name: &str) -> Option<&str> {
        split_host(name).0
    }
    /// A local tag or repository without its registry host:
    /// `private/x:1` for `nas.example:5000/private/x:1`.
    pub fn without_host(name: &str) -> &str {
        split_host(name).1
    }
    /// Semantic Versioning 2.0.0 §§2, 9–10: `MAJOR.MINOR.PATCH` without leading
    /// zeros, optionally followed by `-prerelease` and `+build` identifiers. Tags
    /// of this shape are immutable once they name an image.
    pub fn is_version_tag(tag: &str) -> bool {
        let (core, build) = match tag.split_once('+') {
            Some((core, build)) => (core, Some(build)),
            None => (tag, None),
        };
        let (version, pre) = match core.split_once('-') {
            Some((version, pre)) => (version, Some(pre)),
            None => (core, None),
        };
        let numeric = |part: &str| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part == "0" || !part.starts_with('0'))
        };
        let identifiers = |text: &str| {
            text.split('.').all(|id| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        };
        let parts: Vec<_> = version.split('.').collect();
        parts.len() == 3
            && parts.iter().all(|p| numeric(p))
            && pre.is_none_or(|pre| {
                identifiers(pre)
                    && pre
                        .split('.')
                        .all(|id| !id.bytes().all(|b| b.is_ascii_digit()) || numeric(id))
            })
            && build.is_none_or(identifiers)
    }
}
