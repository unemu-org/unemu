// SPDX-License-Identifier: AGPL-3.0-or-later
//! Zstandard payload codec for stored image blobs; machine-images §2 and §3.
//!
//! Frames follow RFC 8878 (Zstandard Compression and the application/zstd Media
//! Type): §3.1.1 frame layout with a Content_Checksum, §3.1.1.1.4
//! Frame_Content_Size always written, and §3.1.1.1.2 Window_Descriptor, whose
//! decoded window is capped here so a hostile frame header cannot demand
//! unbounded decoder memory.
//!
//! OCI `digest` and `size` name the stored (compressed) bytes. The decoded
//! SHA-256 and length travel as descriptor annotations and stay the identity
//! that configs, journals and layer parents use, so recompressing with another
//! encoder profile changes blob and image digests but never a machine digest.
//! Which payloads may be compressed is declared by the platform provider as
//! `+zstd` payload types; this module never infers it from a media type name.
//!
//! # Encoder profile 1
//!
//! A pinned recipe's lock names stored digests (recipes §3.8 and §9), so the
//! exact bytes this encoder writes bind for good. The `+zstd` suffix of the
//! version-1 payload media types names encoder profile 1; no other manifest
//! field records it. Profile 1 is:
//!
//! - **Which payloads:** a payload is encoded exactly when its provider
//!   declares the `+zstd` variant of its media type, and it is at least
//!   [`MIN_COMPRESS_SIZE`] (4 KiB) and at most that variant's declared
//!   decoded-size limit; otherwise it is stored raw. The threshold and the
//!   limits decide stored digests too, so they are part of the profile:
//!   [`MEDIA_PAYLOAD_TYPES`], [`FIRMWARE_PAYLOAD_TYPES`] and each platform
//!   provider's own table (the PC provider's `pc_image::provider::PAYLOAD_TYPES`).
//! - **Library:** the `zstd` 0.13.5, `zstd-safe` 7.4.1 and `zstd-sys` 2.1.3
//!   crates, pinned exactly in this crate's manifest with default features
//!   off. `zstd-sys` compiles the libzstd 1.5.7 C sources it bundles, not a
//!   system libzstd or another backend. The `linked_zstd_is_the_pinned_release`
//!   test catches only another libzstd release; the vectors below catch any
//!   library whose frames differ.
//! - **Level:** [`LEVEL`], 3, with libzstd's own level-3 parameters for the
//!   pledged size and nothing else set: no explicit window, strategy or block
//!   size, no long-distance matching, no dictionary.
//! - **One thread:** no worker count is set, so libzstd encodes in the calling
//!   thread whether or not its multi-threaded encoder is built (Cargo feature
//!   unification could build it with no change to `Cargo.lock`).
//! - **One frame per payload**, written in one streaming pass with the exact
//!   decoded size pledged before the first byte and no flush before the end.
//! - **Frame header** (RFC 8878 §3.1.1.1.1): Content_Checksum_flag set, so the
//!   frame ends with the low 32 bits of the decoded bytes' XXH64;
//!   Frame_Content_Size always written; Dictionary_ID_flag 0; reserved bit 0.
//! - **Window** (RFC 8878 §3.1.1.1.2): level 3's 2 MiB (window log 21), which
//!   libzstd shrinks to the smallest power of two that holds a smaller
//!   payload. A payload of at most 2 MiB therefore gets a single-segment frame
//!   without a Window_Descriptor; a larger one records the 2 MiB window.
//!
//! The test vectors in `tests/zstd_profile.rs` pin profile 1's output: one
//! synthetic payload of each compressed kind with its frame's length and
//! SHA-256, payloads at the edges of the threshold and a limit, and this
//! crate's two tables by value. Profile 1 never changes. An encoder that
//! writes other bytes for the same input, through another library release,
//! level or option, or that encodes other payloads, is a new profile with a
//! marker of its own beside `+zstd`, and readers keep decoding both; failing
//! vectors mean the encoder in use is no longer profile 1, never that the pins
//! need updating. Decoding is not part of a profile: any conforming decoder
//! reads every profile's frames.
//!
//! Compressed blobs stay compressed in the store, so compression only ever
//! saves local disk. Verification streams the decoded bytes through SHA-256
//! and writes nothing. A decoded file under `cache/materialized/<decoded-sha256>`
//! exists only after a caller asked for a file path of random-access media,
//! such as a mounted hard disk; bounded payloads decode into memory. The sweep
//! removes decoded files that no tag or pinned image reaches. Encoding and
//! decoding stream through fixed buffers and never rely on sparse files.

use crate::{
    filesystem as host, invalid,
    store::{check_digest, check_size},
    Blob, ConfigValidator, Descriptor, Digest, Manifest, PayloadType, Result, Store,
    MAX_SMALL_BLOB,
};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Descriptor annotation with the decoded payload SHA-256 (`sha256:<hex>`).
pub const DECODED_DIGEST: &str = "unemu.decoded.digest";
/// Descriptor annotation with the decoded payload length in canonical decimal.
pub const DECODED_SIZE: &str = "unemu.decoded.size";
/// Media type suffix marking a Zstandard-encoded payload.
pub const SUFFIX: &str = "+zstd";
/// Shorter payloads stay raw: frame overhead outweighs any saving. Part of
/// encoder profile 1, like the payload-type limits below.
pub const MIN_COMPRESS_SIZE: u64 = 4096;
/// Encoder profile 1's level, encoded single-threaded, so the same input
/// always yields the same stored bytes; see the module documentation.
pub const LEVEL: i32 = 3;
/// Largest accepted decoder window, 8 MiB; level 3 never asks for more than 2 MiB.
pub const WINDOW_LOG_MAX: u32 = 23;
/// Largest standard floppy image: 2880 KiB, a 3.5-inch extra-density disk.
pub const MAX_FLOPPY: u64 = 2880 * 1024;
/// Largest compressed optical image: 900 MiB, which holds a 99-minute CD-ROM
/// of 2048-byte sectors. Larger images are stored raw.
pub const MAX_OPTICAL: u64 = 900 * 1024 * 1024;
/// Largest compressed extra file, 2 GiB. Larger extras are stored raw.
pub const MAX_EXTRA: u64 = 2 * 1024 * 1024 * 1024;
/// Largest compressed firmware ROM in a firmware package, 16 MiB. ROMs are
/// bounded, so they always decode into memory; larger ones are stored raw.
pub const MAX_FIRMWARE: u64 = 16 * 1024 * 1024;
/// Free space kept beyond a decoded payload's declared size before it is
/// materialized, so a mount never fills the store's filesystem.
pub const MATERIALIZE_MARGIN: u64 = 256 * 1024 * 1024;
/// Installation-set binary codecs, owned by the shared media-set envelope.
/// Each limit bounds the declared decoded size of a compressed payload.
pub const MEDIA_PAYLOAD_TYPES: &[PayloadType] = &[
    PayloadType {
        media_type: "application/vnd.unemu.media.floppy.v1+zstd",
        max_size: MAX_FLOPPY,
    },
    PayloadType {
        media_type: "application/vnd.unemu.media.optical.v1+zstd",
        max_size: MAX_OPTICAL,
    },
    PayloadType {
        media_type: "application/vnd.unemu.media.extra.v1+zstd",
        max_size: MAX_EXTRA,
    },
];
/// Largest compressed source archive, 2 GiB, the extra-file limit.
pub const MAX_SOURCE_ARCHIVE: u64 = MAX_EXTRA;
/// Firmware-package binary codecs: ROM images, the extras packages share
/// with media sets, and tar source archives. A zip archive is already
/// deflate-compressed, so it is always stored raw. Each limit bounds the
/// declared decoded size.
pub const FIRMWARE_PAYLOAD_TYPES: &[PayloadType] = &[
    PayloadType {
        media_type: "application/vnd.unemu.firmware.v1+zstd",
        max_size: MAX_FIRMWARE,
    },
    PayloadType {
        media_type: "application/vnd.unemu.media.extra.v1+zstd",
        max_size: MAX_EXTRA,
    },
    PayloadType {
        media_type: "application/x-tar+zstd",
        max_size: MAX_SOURCE_ARCHIVE,
    },
];
const BUFFER: usize = 64 * 1024;
const MATERIALIZED: &str = "cache/materialized";

/// The compressed variant of a payload media type.
pub fn compressed_type(kind: &str) -> String {
    format!("{kind}{SUFFIX}")
}

/// Upper bound on one frame's stored length for `decoded` input bytes, the
/// `ZSTD_COMPRESSBOUND` formula of the Zstandard reference library. A stored
/// frame larger than this cannot have come from a single-frame encoder.
pub fn max_frame_size(decoded: u64) -> u64 {
    const BLOCK: u64 = 128 * 1024;
    let small = if decoded < BLOCK {
        (BLOCK - decoded) >> 11
    } else {
        0
    };
    decoded.saturating_add(decoded >> 8).saturating_add(small)
}

/// A payload is compressed only when its provider declares the `+zstd`
/// variant and its length fits that variant's decoded-size limit.
fn declared(kind: &str, codecs: &[PayloadType], size: u64) -> bool {
    let compressed = compressed_type(kind);
    size >= MIN_COMPRESS_SIZE
        && codecs
            .iter()
            .any(|t| t.media_type == compressed && size <= t.max_size)
}

/// Copy at most `limit` bytes through one fixed buffer, hashing what passes.
fn copy(reader: &mut dyn Read, writer: &mut dyn Write, limit: u64) -> Result<(Digest, u64)> {
    let mut buffer = vec![0; BUFFER];
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .filter(|n| *n <= limit)
            .ok_or_else(|| invalid("payload exceeds its declared size"))?;
        hash.update(&buffer[..count]);
        writer.write_all(&buffer[..count])?;
    }
    Ok((Digest::from_hash(hash.finalize()), size))
}

/// Hashes the bytes written through it, so compressed output is identified
/// while it is staged instead of being read back.
struct Hashing<W> {
    inner: W,
    hash: Sha256,
    size: u64,
}
impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.hash.update(&bytes[..count]);
        self.size += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Encode exactly `size` bytes into one encoder profile 1 frame written to
/// `output`, returning the decoded digest and the stored blob identity. A
/// source that is shorter, longer or differs from `expected` is refused. Any
/// change here that alters the frame's bytes is a new profile, not an edit.
fn encode(
    reader: &mut dyn Read,
    output: impl Write,
    size: u64,
    expected: Option<&Digest>,
) -> Result<(Digest, Blob)> {
    let output = Hashing {
        inner: output,
        hash: Sha256::new(),
        size: 0,
    };
    let mut encoder = zstd::stream::write::Encoder::new(output, LEVEL)?;
    encoder.include_checksum(true)?;
    encoder.include_contentsize(true)?;
    encoder.set_pledged_src_size(Some(size))?;
    let (digest, decoded) = copy(reader, &mut encoder, size)?;
    if decoded != size {
        return Err(invalid("source size changed during ingest"));
    }
    if let Some(expected) = expected {
        check_digest(expected, &digest)?;
    }
    let output = encoder.finish()?;
    Ok((
        digest,
        Blob {
            digest: Digest::from_hash(output.hash.finalize()),
            size: output.size,
        },
    ))
}

fn compressed_descriptor(kind: &str, digest: &Digest, blob: Blob, size: u64) -> Descriptor {
    let mut descriptor = Descriptor::new(compressed_type(kind), blob.digest, blob.size);
    descriptor
        .annotations
        .insert(DECODED_DIGEST.into(), digest.to_string());
    descriptor
        .annotations
        .insert(DECODED_SIZE.into(), size.to_string());
    descriptor
}

/// The descriptor [`Store::import_payload`] would store for these bytes,
/// computed without writing anything. The encoding is deterministic, so a
/// later import of the same bytes publishes exactly this blob.
pub fn describe(
    reader: impl Read,
    kind: &str,
    size: u64,
    codecs: &[PayloadType],
) -> Result<Descriptor> {
    let mut reader = reader.take(size.saturating_add(1));
    if !declared(kind, codecs, size) {
        let (digest, length) = copy(&mut reader, &mut io::sink(), size)?;
        if length != size {
            return Err(invalid("source size changed during ingest"));
        }
        return Ok(Descriptor::new(kind, digest, size));
    }
    let (digest, blob) = encode(&mut reader, io::sink(), size, None)?;
    Ok(compressed_descriptor(kind, &digest, blob, size))
}

/// Reads a shared file at its own offset, so concurrent users of one verified
/// stored blob never move each other's read position.
pub(crate) struct At {
    file: Arc<File>,
    offset: u64,
}
impl At {
    pub(crate) fn new(file: Arc<File>) -> Self {
        Self { file, offset: 0 }
    }
}
impl Read for At {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        let count = std::os::unix::fs::FileExt::read_at(&*self.file, bytes, self.offset)?;
        #[cfg(not(unix))]
        let count = {
            use std::io::{Seek, SeekFrom};
            let mut file = self.file.try_clone()?;
            file.seek(SeekFrom::Start(self.offset))?;
            file.read(bytes)?
        };
        self.offset += count as u64;
        Ok(count)
    }
}

/// A decoder over a stored frame with the window cap applied.
pub(crate) fn decoder<R: Read>(
    stored: R,
) -> Result<zstd::stream::read::Decoder<'static, io::BufReader<R>>> {
    let mut decoder = zstd::stream::read::Decoder::new(stored)?;
    decoder.window_log_max(WINDOW_LOG_MAX)?;
    Ok(decoder)
}

/// Stream a stored frame through SHA-256 and check its decoded identity.
pub(crate) fn verify_decoded(stored: &Arc<File>, digest: &Digest, size: u64) -> Result<()> {
    let (actual, length) = copy(
        &mut decoder(At::new(stored.clone()))?,
        &mut io::sink(),
        size,
    )?;
    check_size(size, length)?;
    check_digest(digest, &actual)
}

fn open_read_only(path: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

/// Remove whatever occupies a cache path: a file, a symlink or a directory.
fn remove_entry(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    }
}

fn is_digest_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Outcome of a materialized-cache sweep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Decoded files and leftovers removed.
    pub removed_files: usize,
    pub removed_bytes: u64,
    /// Bytes still held by reachable decoded files and flattened chain heads.
    pub kept_bytes: u64,
}

impl Store {
    /// Ingest one payload of exactly `size` bytes. It is compressed when
    /// `codecs` declares the `+zstd` variant of `kind`, the size is at least
    /// [`MIN_COMPRESS_SIZE`] and within that variant's limit; otherwise it is
    /// stored raw. The size is pledged into the frame, so the same content
    /// always gives the same stored bytes. Length and `expected` are checked
    /// before publication; an interrupted or failed ingest leaves no blob.
    pub fn import_payload(
        &self,
        reader: impl Read,
        kind: &str,
        size: u64,
        expected: Option<&Digest>,
        codecs: &[PayloadType],
    ) -> Result<Descriptor> {
        let _lease = self.lease()?;
        // One byte past the pledge is enough to detect a longer source.
        let mut reader = reader.take(size.saturating_add(1));
        if !declared(kind, codecs, size) {
            let blob = self.import_sized(&mut reader, expected, Some(size))?;
            return Ok(Descriptor::new(kind, blob.digest, blob.size));
        }
        let mut staged = host::Temporary::create(&self.root().join("staging"))?;
        let (digest, blob) = encode(&mut reader, &mut staged.file, size, expected)?;
        staged.file.sync_all()?;
        // The staged frame is published by rename, never copied a second time.
        self.publish_staged(&mut staged, &blob)?;
        Ok(compressed_descriptor(kind, &digest, blob, size))
    }

    /// Re-encode one raw payload when `types` declares its `+zstd` variant.
    /// Compressed, tiny and undeclared payloads are returned unchanged.
    pub fn compress_payload(
        &self,
        descriptor: &Descriptor,
        types: &[PayloadType],
    ) -> Result<Descriptor> {
        if descriptor.is_compressed() || !declared(&descriptor.media_type, types, descriptor.size) {
            return Ok(descriptor.clone());
        }
        // The expected digest and size are checked while encoding, so one pass
        // over the raw blob both verifies and compresses it.
        let mut result = self.import_payload(
            self.open_blob(&descriptor.digest, descriptor.size)?,
            &descriptor.media_type,
            descriptor.size,
            Some(&descriptor.digest),
            types,
        )?;
        result.annotations.extend(descriptor.annotations.clone());
        // Inline bytes and alternate locations describe the old encoding.
        result.artifact_type = descriptor.artifact_type.clone();
        result.platform = descriptor.platform.clone();
        Ok(result)
    }

    /// Store a copy of the `source` manifest whose declared binary payloads
    /// are compressed, without creating a tag. The config blob, and therefore
    /// the machine digest, is unchanged; the source stays intact.
    pub fn compress_manifest(
        &self,
        source: &Descriptor,
        validator: &dyn ConfigValidator,
    ) -> Result<Descriptor> {
        let image = self.resolve_descriptor(source, validator)?;
        let mut manifest = image.manifest;
        let layers = manifest.layers.clone();
        for descriptor in &mut manifest.layers {
            *descriptor = if descriptor.media_type == crate::media_type::METADATA {
                crate::platform::current_metadata(self, descriptor, &layers)?
            } else {
                self.compress_payload(descriptor, validator.payload_types())?
            };
        }
        // The copy is written now, so its manifest and metadata name the
        // platform by its current id; the config, and with it the machine
        // digest, stays the source's.
        manifest.canonicalize_platform()?;
        // The compressed payload types have rows of their own; the source's
        // marker stays as a floor.
        let required = validator.required_release(&manifest, &image.config)?;
        crate::compat::floor(&mut manifest, required)?;
        self.import_manifest_with_types(&manifest, validator.payload_types())
    }

    /// Publish `target` as a compressed copy of `source`; see
    /// [`Store::compress_manifest`].
    pub fn compress_image(
        &self,
        source: &str,
        target: &str,
        validator: &dyn ConfigValidator,
    ) -> Result<Descriptor> {
        self.check_new_tag(target)?;
        let result = self.compress_manifest(&self.tag_descriptor(source)?, validator)?;
        self.update_tag(target, &result, None, validator)?;
        Ok(result)
    }

    pub(crate) fn decoded_path(&self, digest: &Digest) -> PathBuf {
        self.root().join(MATERIALIZED).join(digest.hex())
    }

    /// Decode a bounded payload into memory and verify its decoded identity.
    pub(crate) fn decode_bounded(
        &self,
        descriptor: &Descriptor,
        stored: &Arc<File>,
    ) -> Result<Vec<u8>> {
        let size = descriptor.decoded_size()?;
        let mut bytes = Vec::new();
        let (digest, length) = copy(&mut decoder(At::new(stored.clone()))?, &mut bytes, size)?;
        check_size(size, length)?;
        check_digest(&descriptor.decoded_digest()?, &digest)?;
        Ok(bytes)
    }

    /// Return a verified decoded file for a compressed payload whose stored
    /// bytes were already verified. The caller holds a store lease. An existing
    /// cache file is reused when its own SHA-256 is the decoded digest; anything
    /// else at the path is removed before decoding, so a rebuild never needs
    /// room for two copies. The returned metadata was captured before the
    /// content hash, so later receipt checks detect a change made after it.
    pub(crate) fn materialize(
        &self,
        digest: &Digest,
        size: u64,
        stored: &Arc<File>,
    ) -> Result<(File, fs::Metadata)> {
        let directory = self.root().join(MATERIALIZED);
        fs::create_dir_all(&directory)?;
        let path = self.decoded_path(digest);
        // The same lock guards a flattened chain head with the same content.
        let _lock = host::exclusive_lock(&directory.join(format!("{}.lock", digest.hex())))?;
        if let Some(reused) = reuse_cache(&path, digest, size)? {
            return Ok(reused);
        }
        // The cache is rebuildable: a damaged entry is replaced, never trusted.
        remove_entry(&path)?;
        host::require_space(self.root(), size.saturating_add(MATERIALIZE_MARGIN))?;
        let pending = directory.join(format!("{}.decoding", digest.hex()));
        let result = (|| -> Result<()> {
            // A leftover from an interrupted decode is never published.
            remove_entry(&pending)?;
            let mut file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&pending)?;
            let (actual, length) = copy(&mut decoder(At::new(stored.clone()))?, &mut file, size)?;
            check_size(size, length)?;
            check_digest(digest, &actual)?;
            file.sync_all()?;
            host::publish_blob(&pending, &path)?;
            host::sync_dir(&directory)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&pending);
        }
        result?;
        let mut file = open_read_only(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("decoded cache is not a regular file"));
        }
        #[cfg(test)]
        if tests::MUTATE_AFTER_PUBLISH.with(|armed| armed.replace(false)) {
            fs::write(&path, vec![0x69; size as usize])?;
        }
        let (actual, length) = copy(&mut file, &mut io::sink(), size)?;
        check_size(size, length)?;
        check_digest(digest, &actual)?;
        Ok((file, metadata))
    }

    /// Bytes held by `cache/materialized`: decoded payloads of mounted media
    /// and flattened chain heads. Both are rebuildable.
    pub fn materialized_bytes(&self) -> Result<u64> {
        let _lease = self.lease()?;
        let entries = match fs::read_dir(self.root().join(MATERIALIZED)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            other => other?,
        };
        let mut total = 0_u64;
        for entry in entries {
            let metadata = entry?.metadata()?;
            if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
        Ok(total)
    }

    /// Remove decoded payload files that no local tag and no `pinned` image
    /// manifest reaches, plus lock and partial-decode leftovers. It holds the
    /// exclusive store lease, so no operation or running workspace uses the
    /// cache meanwhile; a busy store is refused rather than waited for. A pin
    /// missed by a race only costs a rebuild at the next mount, because the
    /// cache is rebuildable. Flattened chain heads, marked by a verified-head
    /// marker, are left for the provider and a future GC.
    pub fn sweep_materialized(&self, pinned: &[Digest]) -> Result<Sweep> {
        let lease = host::lock_file(&self.root().join("locks/store.lock"))?;
        lease
            .try_lock()
            .map_err(|error| invalid(format!("store busy: {error}")))?;
        let mut roots: BTreeSet<Digest> = self
            .tags_inner()?
            .into_iter()
            .map(|tag| tag.manifest.digest)
            .collect();
        roots.extend(pinned.iter().cloned());
        let mut reachable = BTreeSet::new();
        for root in &roots {
            // An unreadable manifest cannot be resolved, so nothing it names
            // could be mounted from this store.
            let Ok(manifest) = self.manifest_by_digest(root) else {
                continue;
            };
            for layer in manifest.layers.iter().filter(|d| d.is_compressed()) {
                if let Ok(digest) = layer.decoded_digest() {
                    reachable.insert(digest.hex().to_owned());
                }
            }
        }
        let directory = self.root().join(MATERIALIZED);
        let entries = match fs::read_dir(&directory) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Sweep::default()),
            other => other?,
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            names.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.metadata()?.len(),
            ));
        }
        let heads: BTreeSet<&str> = names
            .iter()
            .filter(|(name, _)| name.ends_with(".verified"))
            .filter_map(|(name, _)| name.split('.').next())
            .collect();
        let mut sweep = Sweep::default();
        for (name, length) in &names {
            let remove = if is_digest_name(name) {
                !reachable.contains(name) && !heads.contains(name.as_str())
            } else {
                // Nobody holds a cache lock under the exclusive lease, so
                // locks and interrupted decodes are safe to drop.
                name.ends_with(".lock") || name.ends_with(".decoding") || name.ends_with(".pending")
            };
            if remove {
                remove_entry(&directory.join(name))?;
                sweep.removed_files += 1;
                sweep.removed_bytes = sweep.removed_bytes.saturating_add(*length);
            } else if is_digest_name(name) {
                sweep.kept_bytes = sweep.kept_bytes.saturating_add(*length);
            }
        }
        host::sync_dir(&directory)?;
        Ok(sweep)
    }

    fn manifest_by_digest(&self, digest: &Digest) -> Result<Manifest> {
        let bytes = crate::store::read_bounded(
            &self.root().join("images/blobs/sha256").join(digest.hex()),
            MAX_SMALL_BLOB,
        )?;
        check_digest(digest, &Digest::of(&bytes))?;
        Manifest::decode_for_summary(&bytes)
    }
}

/// Reuse a cache file only when it is a single-link regular file of the
/// declared length whose SHA-256 is the decoded digest.
fn reuse_cache(path: &Path, digest: &Digest, size: u64) -> Result<Option<(File, fs::Metadata)>> {
    // A symlink, directory or any other non-file entry is rebuilt.
    let Ok(mut file) = open_read_only(path) else {
        return Ok(None);
    };
    let metadata = file.metadata()?;
    #[cfg(unix)]
    let linked = std::os::unix::fs::MetadataExt::nlink(&metadata) != 1;
    #[cfg(not(unix))]
    let linked = false;
    // A second hard link could be written through another name.
    if !metadata.is_file() || metadata.len() != size || linked {
        return Ok(None);
    }
    match copy(&mut file, &mut io::sink(), size) {
        Ok((actual, length)) if length == size && actual == *digest => Ok(Some((file, metadata))),
        _ => Ok(None),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! Cache mutation, rebuild and space refusal; machine-images §3.
    use super::*;
    use crate::{media_type, BlobSource};

    thread_local! {
        pub(crate) static MUTATE_AFTER_PUBLISH: std::cell::Cell<bool> =
            const { std::cell::Cell::new(false) };
    }

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn compressed(name: &str) -> (Temp, Store, Descriptor) {
        let temp = Temp(std::env::temp_dir().join(format!("codec-{name}-{}", std::process::id())));
        let store = Store::open(&temp.0).unwrap();
        let types = [PayloadType {
            media_type: "application/vnd.unemu.disk.base.v1+zstd",
            max_size: 1 << 20,
        }];
        let descriptor = store
            .import_payload(
                [0x59; 8192].as_slice(),
                media_type::DISK_BASE,
                8192,
                None,
                &types,
            )
            .unwrap();
        assert!(descriptor.is_compressed());
        (temp, store, descriptor)
    }

    fn cache_names(store: &Store) -> Vec<String> {
        match fs::read_dir(store.root().join(MATERIALIZED)) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[test]
    fn cache_changed_after_publication_is_rejected() {
        let (_temp, store, descriptor) = compressed("mutated");
        MUTATE_AFTER_PUBLISH.with(|armed| armed.set(true));
        let result = store.open_verified(&descriptor);
        assert!(!MUTATE_AFTER_PUBLISH.with(|armed| armed.get()));
        let decoded = descriptor.decoded_digest().unwrap();
        assert!(
            matches!(&result, Err(crate::Error::DigestMismatch { expected, .. }) if *expected == decoded),
            "{result:?}"
        );
    }

    #[test]
    fn damaged_linked_and_directory_caches_are_rebuilt_and_a_valid_one_is_not_rewritten() {
        let (temp, store, descriptor) = compressed("rebuild");
        let path = store.decoded_path(&descriptor.decoded_digest().unwrap());
        let read = |store: &Store| {
            let mut file = store.open_verified(&descriptor).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, [0x59; 8192]);
        };
        read(&store);
        fs::write(&path, [0x11; 8192]).unwrap();
        read(&store);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let before = fs::metadata(&path).unwrap();
            store.open_verified(&descriptor).unwrap();
            let after = fs::metadata(&path).unwrap();
            assert_eq!(before.ino(), after.ino());
            assert_eq!(before.mtime_nsec(), after.mtime_nsec());
            // A correct file with a second name is not trusted.
            let alias = temp.0.join("alias");
            fs::hard_link(&path, &alias).unwrap();
            read(&store);
            assert_ne!(fs::metadata(&path).unwrap().ino(), before.ino());
            assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside"), b"x").unwrap();
        read(&store);
        assert!(path.is_file());
        assert!(cache_names(&store)
            .iter()
            .all(|name| !name.ends_with(".decoding")));
    }

    #[test]
    fn bounded_reads_decode_in_memory_without_a_cache_file() {
        let (_temp, store, descriptor) = compressed("memory");
        assert_eq!(
            store.read_verified(&descriptor, 8192).unwrap(),
            [0x59; 8192]
        );
        assert!(cache_names(&store).is_empty());
    }

    #[test]
    fn materializing_more_than_the_free_space_is_refused_as_storage_full() {
        let (_temp, store, descriptor) = compressed("space");
        let stored = Arc::new(
            store
                .open_blob(&descriptor.digest, descriptor.size)
                .unwrap(),
        );
        // No test host has this much space, so the check fires before decoding.
        let error = store
            .materialize(&descriptor.decoded_digest().unwrap(), 1 << 60, &stored)
            .unwrap_err();
        assert!(
            matches!(&error, crate::Error::Io(e) if e.kind() == io::ErrorKind::StorageFull),
            "{error:?}"
        );
        assert!(cache_names(&store)
            .iter()
            .all(|name| name.ends_with(".lock")));
    }
}
