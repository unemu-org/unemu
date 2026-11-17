// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic streaming codec, lazy materialization and atomic-ingest coverage;
//! machine-images §§2–3.

use image_store::{
    codec::{self, DECODED_DIGEST, DECODED_SIZE},
    media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest, Error, Manifest,
    PayloadType, Result, Store,
};
use std::{
    alloc::GlobalAlloc,
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
};
static LARGEST_ALLOCATION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
thread_local! {
    /// Only the measuring thread is counted; other tests run in parallel.
    static TRACKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn note(size: usize) {
    if TRACKED.try_with(|t| t.get()).unwrap_or(false) {
        LARGEST_ALLOCATION.fetch_max(size, Ordering::Relaxed);
    }
}
/// Records the largest single Rust allocation of a tracked thread. The zstd
/// library allocates its own bounded state in C, outside this counter; its
/// window is capped instead.
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        note(layout.size());
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        note(size);
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        note(layout.size());
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// A synthetic provider declaration: disk bases and firmware are compressible.
const CODECS: &[PayloadType] = &[
    PayloadType {
        media_type: "application/vnd.unemu.disk.base.v1+zstd",
        max_size: i64::MAX as u64,
    },
    PayloadType {
        media_type: "application/vnd.unemu.firmware.v1+zstd",
        max_size: i64::MAX as u64,
    },
];
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn store() -> (Self, Store) {
        let temp = Self(std::env::temp_dir().join(format!(
            "image-compression-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let store = Store::open(&temp.0).unwrap();
        (temp, store)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn payload(store: &Store, bytes: &[u8]) -> Descriptor {
    store
        .import_payload(
            bytes,
            mt::DISK_BASE,
            bytes.len() as u64,
            Some(&Digest::of(bytes)),
            CODECS,
        )
        .unwrap()
}
fn names(store: &Store, directory: &str) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(store.root().join(directory)) {
        Ok(entries) => entries
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}
/// Pseudo-random bytes that zstd cannot shrink much.
fn noise(length: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_u32;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

/// Synthetic platform whose config lists the decoded digests it references.
struct Provider;
impl ConfigValidator for Provider {
    fn platform(&self) -> &str {
        "c64"
    }
    fn payload_types(&self) -> &[PayloadType] {
        CODECS
    }
    fn validate(
        &self,
        table: &toml::Table,
        _: &[Descriptor],
        _: &dyn BlobSource,
    ) -> Result<Vec<Digest>> {
        table
            .get("blobs")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| Error::Invalid("missing blobs".into()))?
            .iter()
            .map(|v| v.as_str().unwrap_or_default().parse())
            .collect()
    }
}
/// Publish one image whose only payload is `layer`, under `tag`.
fn image(store: &Store, layer: &Descriptor, tag: &str) -> Descriptor {
    let put = |kind: &str, bytes: &[u8]| {
        let blob = store.import(bytes, None).unwrap();
        Descriptor::new(kind, blob.digest, blob.size)
    };
    let config = put(
        mt::CONFIG,
        format!(
            "schema = 1\nplatform = \"c64\"\nblobs = [\"{}\"]\n",
            layer.decoded_digest().unwrap()
        )
        .as_bytes(),
    );
    let metadata = put(
        mt::METADATA,
        br#"{"schema":1,"platform":"c64","titles":[]}"#,
    );
    let manifest =
        Manifest::new_with_types("c64", config, vec![layer.clone(), metadata], CODECS).unwrap();
    let descriptor = store.import_manifest_with_types(&manifest, CODECS).unwrap();
    store.update_tag(tag, &descriptor, None, &Provider).unwrap();
    descriptor
}

#[test]
fn zstd_round_trip_is_deterministic_and_records_both_identities() {
    let (_temp, store) = Temp::store();
    let bytes: Vec<u8> = (0..128 * 1024_u32).map(|i| (i % 251) as u8).collect();
    let first = payload(&store, &bytes);
    struct ShortRead<'a>(&'a [u8]);
    impl Read for ShortRead<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = bytes.len().min(17);
            self.0.read(&mut bytes[..count])
        }
    }
    let second = store
        .import_payload(
            ShortRead(&bytes),
            mt::DISK_BASE,
            bytes.len() as u64,
            None,
            CODECS,
        )
        .unwrap();
    let (_other_temp, other) = Temp::store();
    let third = payload(&other, &bytes);
    assert!(first.is_compressed());
    assert_eq!(first.media_type, "application/vnd.unemu.disk.base.v1+zstd");
    assert_eq!(first, second);
    assert_eq!(first, third);
    // One more frame of encoder profile 1 (the codec module documentation;
    // its vectors are in zstd_profile.rs). An encoder that changes it writes
    // a new profile, never a new pin; the decoded identity below never moves.
    assert_eq!(
        first.digest.to_string(),
        "sha256:fc20b8194fdf791bcc46de210ce6cf3db619ec12cea8dc6e935e59c9a3ef1e9c",
        "the encoder no longer writes Zstandard profile 1; an encoder that \
         writes other bytes is a new profile, never a re-pin"
    );
    assert_eq!(first.decoded_digest().unwrap(), Digest::of(&bytes));
    assert_eq!(first.decoded_size().unwrap(), bytes.len() as u64);
    assert_ne!(first.digest, first.decoded_digest().unwrap());
    assert!(first.size < bytes.len() as u64 / 4);
    let encoded = fs::read(store.blob_file_path(&first.digest, first.size).unwrap()).unwrap();
    assert_eq!(
        zstd::zstd_safe::get_frame_content_size(&encoded).unwrap(),
        Some(bytes.len() as u64)
    );
    assert_eq!(zstd::stream::decode_all(encoded.as_slice()).unwrap(), bytes);
    assert_eq!(
        store.read_verified(&first, bytes.len() as u64).unwrap(),
        bytes
    );
    assert!(store.read_verified(&first, bytes.len() as u64 - 1).is_err());
    store.verify_descriptor(&first).unwrap();
    assert!(names(&store, "cache/materialized").is_empty());
    // An explicit file request decodes one cache file.
    let mut file = store.open_verified(&first).unwrap();
    let mut decoded = Vec::new();
    file.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, bytes);
    assert_eq!(names(&store, "images/blobs/sha256").len(), 1);
}

#[test]
fn linked_zstd_is_the_pinned_release() {
    // A system libzstd (ZSTD_SYS_USE_PKG_CONFIG) of another release would
    // change stored frames; this makes that substitution fail loudly. A
    // system library of the same release passes here; the profile 1 vectors
    // in zstd_profile.rs catch any library whose frames differ.
    assert_eq!(zstd::zstd_safe::version_number(), 1_05_07);
}

#[test]
fn tiny_undeclared_oversized_and_explicit_raw_payloads_stay_raw() {
    let (_temp, store) = Temp::store();
    let small = [PayloadType {
        media_type: "application/vnd.unemu.firmware.v1+zstd",
        max_size: 8192,
    }];
    for (bytes, kind, codecs) in [
        (vec![3; 4095], mt::FIRMWARE, CODECS),
        (vec![4; 8192], mt::FIRMWARE, &[][..]),
        (vec![5; 8192], mt::METADATA, CODECS),
        (vec![6; 8192], mt::FLOPPY, CODECS),
        // Over its declared decoded-size limit, so it is not compressed.
        (vec![7; 8193], mt::FIRMWARE, &small[..]),
    ] {
        let d = store
            .import_payload(bytes.as_slice(), kind, bytes.len() as u64, None, codecs)
            .unwrap();
        assert!(!d.is_compressed());
        assert_eq!(d.media_type, kind);
        assert_eq!(d.digest, Digest::of(&bytes));
        assert!(d.annotations.is_empty());
        assert_eq!(store.read_verified(&d, 8193).unwrap(), bytes);
    }
}

#[test]
fn corrupt_frames_and_decoded_hash_size_mismatches_fail_closed() {
    let (_temp, store) = Temp::store();
    let original = payload(&store, &vec![0x51; 65536]);
    let encoded = fs::read(
        store
            .blob_file_path(&original.digest, original.size)
            .unwrap(),
    )
    .unwrap();
    let mut named = Vec::new();
    // The last byte belongs to the frame's Content_Checksum.
    let mut flipped = encoded.clone();
    *flipped.last_mut().unwrap() ^= 0xff;
    for bytes in [
        b"invalid zstd".as_slice(),
        &encoded[..encoded.len() - 1],
        &flipped,
    ] {
        let b = store.import(bytes, None).unwrap();
        let mut d = original.clone();
        d.digest = b.digest;
        d.size = b.size;
        named.push(d);
    }
    let mut wrong_size = original.clone();
    wrong_size
        .annotations
        .insert(DECODED_SIZE.into(), "1024".into());
    named.push(wrong_size);
    let mut non_canonical_size = original.clone();
    non_canonical_size
        .annotations
        .insert(DECODED_SIZE.into(), "065536".into());
    named.push(non_canonical_size);
    let mut missing = original.clone();
    missing.annotations.remove(DECODED_DIGEST);
    named.push(missing);
    for d in named {
        let error = store.verify_descriptor(&d).unwrap_err().to_string();
        assert!(error.contains(&d.digest.to_string()), "{error}");
        let error = store.open_verified(&d).unwrap_err().to_string();
        assert!(error.contains(&d.digest.to_string()), "{error}");
        let error = store.read_verified(&d, 65536).unwrap_err().to_string();
        assert!(error.contains(&d.digest.to_string()), "{error}");
    }
    // A decoded-hash mismatch stays the typed error, naming the decoded digest.
    let mut wrong_hash = original.clone();
    let wrong = Digest::of(b"wrong");
    wrong_hash
        .annotations
        .insert(DECODED_DIGEST.into(), wrong.to_string());
    for result in [
        store.verify_descriptor(&wrong_hash),
        store.open_verified(&wrong_hash).map(|_| ()),
        store.read_verified(&wrong_hash, 65536).map(|_| ()),
    ] {
        assert!(
            matches!(&result, Err(Error::DigestMismatch { expected, .. }) if *expected == wrong),
            "{result:?}"
        );
    }
    // Verification streams; a failed decode never reaches the cache.
    assert!(names(&store, "cache/materialized").is_empty());
}

#[test]
fn frames_asking_for_a_window_over_8_mib_are_refused() {
    let (_temp, store) = Temp::store();
    let content = noise(256 * 1024);
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
    encoder.window_log(24).unwrap();
    encoder.include_contentsize(false).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.write_all(&content).unwrap();
    let frame = encoder.finish().unwrap();
    // The frame is valid but declares a 16 MiB window.
    let mut decoded = Vec::new();
    let mut wide = zstd::stream::read::Decoder::new(frame.as_slice()).unwrap();
    wide.window_log_max(24).unwrap();
    wide.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, content);
    let mut narrow = zstd::stream::read::Decoder::new(frame.as_slice()).unwrap();
    narrow.window_log_max(23).unwrap();
    assert!(narrow.read_to_end(&mut Vec::new()).is_err());
    let blob = store.import(frame.as_slice(), None).unwrap();
    let mut d = Descriptor::new(
        codec::compressed_type(mt::DISK_BASE),
        blob.digest,
        blob.size,
    );
    d.annotations
        .insert(DECODED_DIGEST.into(), Digest::of(&content).to_string());
    d.annotations
        .insert(DECODED_SIZE.into(), content.len().to_string());
    let error = store.verify_descriptor(&d).unwrap_err().to_string();
    assert!(error.contains(&d.digest.to_string()), "{error}");
    assert!(store.read_verified(&d, 1 << 20).is_err());
    assert!(store.open_verified(&d).is_err());
    assert!(names(&store, "cache/materialized").is_empty());
}

struct BoundedStream {
    remaining: u64,
    fail: bool,
}
impl Read for BoundedStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        assert!(
            bytes.len() <= 64 * 1024,
            "input buffering grew with payload size"
        );
        if self.remaining == 0 {
            return if self.fail {
                Err(io::Error::other("injected interruption"))
            } else {
                Ok(0)
            };
        }
        let count = self.remaining.min(bytes.len() as u64) as usize;
        bytes[..count].fill(0x35);
        self.remaining -= count as u64;
        Ok(count)
    }
}

#[test]
fn large_stream_keeps_the_largest_single_rust_allocation_bounded() {
    let (_temp, store) = Temp::store();
    let size = 256 * 1024 * 1024;
    TRACKED.with(|t| t.set(true));
    let d = store
        .import_payload(
            BoundedStream {
                remaining: size,
                fail: false,
            },
            mt::DISK_BASE,
            size,
            None,
            CODECS,
        )
        .unwrap();
    assert!(d.is_compressed());
    assert!(d.size < 64 * 1024);
    assert_eq!(d.decoded_size().unwrap(), size);
    // Verification decodes the whole stream without writing a decoded file.
    store.verify_descriptor(&d).unwrap();
    TRACKED.with(|t| t.set(false));
    assert!(names(&store, "cache/materialized").is_empty());
    let largest = LARGEST_ALLOCATION.load(Ordering::Relaxed);
    eprintln!("largest single Rust allocation for the streaming codec: {largest}");
    assert!(
        largest <= 2 * 1024 * 1024,
        "allocated {largest} for a {size}-byte payload"
    );
}

#[test]
fn interrupted_or_resized_ingest_publishes_no_partial_blob() {
    let (_temp, store) = Temp::store();
    for (remaining, fail, size, kind) in [
        (128 * 1024, true, 128 * 1024, mt::DISK_BASE),
        // Shorter, then longer, than the pledged length.
        (128 * 1024, false, 256 * 1024, mt::DISK_BASE),
        (256 * 1024, false, 128 * 1024, mt::DISK_BASE),
        // The raw path checks the length before it publishes, too.
        (128 * 1024, false, 256 * 1024, mt::FLOPPY),
        (256 * 1024, false, 128 * 1024, mt::FLOPPY),
    ] {
        assert!(store
            .import_payload(BoundedStream { remaining, fail }, kind, size, None, CODECS)
            .is_err());
        assert!(names(&store, "images/blobs/sha256").is_empty());
        assert!(names(&store, "staging").is_empty());
    }
}

#[test]
fn materialized_bases_and_aliases_cannot_be_journal_outputs() {
    let (temp, store) = Temp::store();
    let d = payload(&store, &vec![0x61; 8192]);
    store.open_verified(&d).unwrap();
    let path = store
        .root()
        .join("cache/materialized")
        .join(d.decoded_digest().unwrap().hex());
    assert!(path.is_file());
    assert!(store
        .validate_media_output(&path)
        .unwrap_err()
        .to_string()
        .contains("materialized"));
    #[cfg(unix)]
    {
        let alias = temp.0.join("alias");
        std::os::unix::fs::symlink(path, &alias).unwrap();
        assert!(store.validate_media_output(&alias).is_err());
    }
}

#[test]
fn recompression_discards_inline_bytes_and_locations_for_the_old_encoding() {
    let (_temp, store) = Temp::store();
    let raw = vec![0; 4096];
    let blob = store.import(raw.as_slice(), None).unwrap();
    let mut descriptor = Descriptor::new(mt::DISK_BASE, blob.digest, blob.size);
    descriptor.data = Some(format!("{}AA==", "AAAA".repeat(1365)));
    descriptor.urls = Some(vec!["https://example.invalid/raw".into()]);
    descriptor
        .annotations
        .insert("description".into(), "Synthetic base".into());
    assert_eq!(
        store.compress_payload(&descriptor, &[]).unwrap(),
        descriptor
    );
    let result = store.compress_payload(&descriptor, CODECS).unwrap();
    assert!(result.is_compressed());
    assert!(result.data.is_none());
    assert!(result.urls.is_none());
    assert_eq!(result.annotations["description"], "Synthetic base");
    assert_eq!(result.decoded_digest().unwrap(), descriptor.digest);
    assert_eq!(store.read_verified(&result, 4096).unwrap(), raw);
    assert_eq!(store.compress_payload(&result, CODECS).unwrap(), result);
}

#[test]
fn resolution_writes_nothing_and_a_file_request_decodes_exactly_once() {
    let (_temp, store) = Temp::store();
    let bytes = noise(64 * 1024);
    let d = payload(&store, &bytes);
    image(&store, &d, "demo:base");
    let resolved = store.resolve("demo:base", &Provider).unwrap();
    assert!(names(&store, "cache/materialized").is_empty());
    // A streamed read writes nothing either and checks the decoded identity.
    let mut streamed = Vec::new();
    resolved
        .content_reader(&d.decoded_digest().unwrap())
        .unwrap()
        .read_to_end(&mut streamed)
        .unwrap();
    assert_eq!(streamed, bytes);
    assert!(names(&store, "cache/materialized").is_empty());
    let hex = d.decoded_digest().unwrap().hex().to_owned();
    for _ in 0..2 {
        let path = store
            .resolved_file_path(&resolved, &d.decoded_digest().unwrap())
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let (mut file, metadata) = resolved
            .verified_file(&d.decoded_digest().unwrap())
            .unwrap();
        assert_eq!(metadata.len(), bytes.len() as u64);
        let mut read = Vec::new();
        std::io::Seek::rewind(&mut file).unwrap();
        file.read_to_end(&mut read).unwrap();
        assert_eq!(read, bytes);
    }
    assert_eq!(
        names(&store, "cache/materialized"),
        [hex.clone(), format!("{hex}.lock")]
    );
    assert_eq!(store.materialized_bytes().unwrap(), bytes.len() as u64);
}

#[test]
fn racing_decoders_on_one_digest_publish_one_valid_cache_file() {
    let (_temp, store) = Temp::store();
    let bytes = noise(4 * 1024 * 1024);
    let d = payload(&store, &bytes);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (store, d, barrier) = (store.clone(), d.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                let mut file = store.open_verified(&d).unwrap();
                let mut read = Vec::new();
                file.read_to_end(&mut read).unwrap();
                #[cfg(unix)]
                let inode = std::os::unix::fs::MetadataExt::ino(&file.metadata().unwrap());
                #[cfg(not(unix))]
                let inode = 0;
                (read, inode)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(results.iter().all(|(read, _)| *read == bytes));
    assert_eq!(results[0].1, results[1].1);
    let hex = d.decoded_digest().unwrap().hex().to_owned();
    assert_eq!(
        names(&store, "cache/materialized"),
        [hex.clone(), format!("{hex}.lock")]
    );
}

#[test]
fn sweep_removes_unreachable_decoded_files_and_keeps_reachable_ones() {
    let (_temp, store) = Temp::store();
    let kept = payload(&store, &noise(16 * 1024));
    let dropped = payload(&store, &vec![0x44; 16 * 1024]);
    let pinned = payload(&store, &vec![0x45; 16 * 1024]);
    let kept_image = image(&store, &kept, "demo:kept");
    let dropped_image = image(&store, &dropped, "demo:dropped");
    let pinned_image = image(&store, &pinned, "demo:pinned");
    for (tag, d) in [
        ("demo:kept", &kept),
        ("demo:dropped", &dropped),
        ("demo:pinned", &pinned),
    ] {
        let resolved = store.resolve(tag, &Provider).unwrap();
        resolved
            .verified_file(&d.decoded_digest().unwrap())
            .unwrap();
    }
    store
        .remove_tag("demo:dropped", &dropped_image.digest)
        .unwrap();
    store
        .remove_tag("demo:pinned", &pinned_image.digest)
        .unwrap();
    // A flattened chain head belongs to its provider and is left alone.
    let directory = store.root().join("cache/materialized");
    let head = Digest::of(b"flattened head");
    fs::write(directory.join(head.hex()), [0; 512]).unwrap();
    fs::write(
        directory.join(format!("{}.{}.verified", head.hex(), kept.digest.hex())),
        b"verified disk head v1\n",
    )
    .unwrap();
    fs::write(directory.join(format!("{}.decoding", head.hex())), b"x").unwrap();
    // A running owner holds a shared lease, so the sweep refuses to run.
    let owner = store.workspace("busy").unwrap();
    assert!(store
        .sweep_materialized(&[])
        .unwrap_err()
        .to_string()
        .contains("busy"));
    drop(owner);
    let sweep = store
        .sweep_materialized(std::slice::from_ref(&pinned_image.digest))
        .unwrap();
    let hex = |d: &Descriptor| d.decoded_digest().unwrap().hex().to_owned();
    let mut expected = vec![
        hex(&kept),
        hex(&pinned),
        head.hex().to_owned(),
        format!("{}.{}.verified", head.hex(), kept.digest.hex()),
    ];
    expected.sort();
    assert_eq!(names(&store, "cache/materialized"), expected);
    assert_eq!(sweep.removed_bytes, 16 * 1024 + 1);
    assert_eq!(sweep.kept_bytes, 2 * 16 * 1024 + 512);
    // A swept decoded file is rebuilt by the next mount.
    image(&store, &dropped, "demo:again");
    let resolved = store.resolve("demo:again", &Provider).unwrap();
    resolved
        .verified_file(&dropped.decoded_digest().unwrap())
        .unwrap();
    assert!(names(&store, "cache/materialized").contains(&hex(&dropped)));
    let _ = kept_image;
}

#[test]
fn media_set_compressed_sizes_over_their_format_limits_are_refused() {
    let (_temp, store) = Temp::store();
    let put = |kind: &str, bytes: &[u8]| {
        let blob = store.import(bytes, None).unwrap();
        Descriptor::new(kind, blob.digest, blob.size)
    };
    let config = put(mt::MEDIA_SET_CONFIG, b"schema = 1\n");
    let metadata = put(
        mt::METADATA,
        br#"{"schema":1,"platform":"c64","titles":[]}"#,
    );
    let frame = put("application/vnd.unemu.media.optical.v1+zstd", &[0; 64]);
    for (kind, limit) in [
        (mt::FLOPPY, codec::MAX_FLOPPY),
        (mt::OPTICAL, codec::MAX_OPTICAL),
        (mt::EXTRA, codec::MAX_EXTRA),
    ] {
        for (size, allowed) in [(limit, true), (limit + 1, false)] {
            let mut d = frame.clone();
            d.media_type = codec::compressed_type(kind);
            d.annotations
                .insert(DECODED_DIGEST.into(), Digest::of(b"any").to_string());
            d.annotations.insert(DECODED_SIZE.into(), size.to_string());
            let result =
                Manifest::new_media_set("c64", config.clone(), vec![d.clone(), metadata.clone()]);
            assert_eq!(result.is_ok(), allowed, "{kind} {size}: {result:?}");
        }
    }
    // A stored frame cannot be larger than any encoder could emit.
    let mut d = frame.clone();
    d.annotations
        .insert(DECODED_DIGEST.into(), Digest::of(b"any").to_string());
    d.annotations.insert(DECODED_SIZE.into(), "0".into());
    d.size = codec::max_frame_size(0) + 1;
    assert!(Manifest::new_media_set("c64", config, vec![d, metadata]).is_err());
}
