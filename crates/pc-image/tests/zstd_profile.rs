// SPDX-License-Identifier: AGPL-3.0-or-later
//! The PC provider's share of Zstandard encoder profile 1; machine-images §2.
//!
//! Which payloads of a PC machine image become `+zstd` follows from the
//! provider's payload-type table: a payload is encoded when the table declares
//! the `+zstd` variant of its media type and its length is at least the store's
//! threshold and at most that variant's limit. A pinned recipe's lock names the
//! stored digests, so a changed limit would silently change them. The table
//! and the boundary vectors below are therefore part of profile 1: a test that
//! fails here means a new profile, never a re-pin.

use image_store::{
    codec, media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest, Store,
};
use pc_image::{PcProvider, PAYLOAD_TYPES};
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

const NEW_PROFILE: &str = "the PC provider no longer encodes the payloads Zstandard profile 1 \
     encodes; another limit or compression flag is a new profile beside `+zstd`, never a re-pin";

/// The 1.44 MB floppy limit, the one boundary this file exercises.
const FLOPPY_LIMIT: usize = 1440 * 1024;

#[test]
fn the_pc_payload_type_table_is_frozen_by_value() {
    // Each row: media type, decoded-size limit, and whether the row is a
    // `+zstd` variant (the compression flag). Order in the table carries no
    // meaning, so the rows are compared as a set.
    let table: BTreeSet<(String, u64, bool)> = PAYLOAD_TYPES
        .iter()
        .map(|t| {
            (
                t.media_type.to_owned(),
                t.max_size,
                t.media_type.ends_with(codec::SUFFIX),
            )
        })
        .collect();
    assert_eq!(
        table.len(),
        PAYLOAD_TYPES.len(),
        "a media type is listed twice"
    );
    let frozen: BTreeSet<(String, u64, bool)> = [
        ("application/vnd.unemu.firmware.v1+zstd", 16 << 20, true),
        (
            "application/vnd.unemu.disk.base.v1+zstd",
            8_455_200_768,
            true,
        ),
        (
            "application/vnd.unemu.disk.sectors.v1+zstd",
            8_587_313_280,
            true,
        ),
        (
            "application/vnd.unemu.media.floppy.v1+zstd",
            1_474_560,
            true,
        ),
        ("application/x-tar+zstd", 2 << 30, true),
        ("application/vnd.unemu.firmware.v1", 16 << 20, false),
        ("application/vnd.unemu.disk.base.v1", 8_455_200_768, false),
        (
            "application/vnd.unemu.disk.sectors.v1",
            8_587_313_280,
            false,
        ),
        ("application/vnd.unemu.media.floppy.v1", 1_474_560, false),
        ("application/vnd.unemu.disk.layer.v1+toml", 1 << 20, false),
        ("application/vnd.unemu.cmos.v1", 64, false),
        ("application/vnd.unemu.disk.zero.v1+toml", 4096, false),
    ]
    .iter()
    .map(|(t, n, z)| (t.to_string(), *n, *z))
    .collect();
    assert_eq!(table, frozen, "{NEW_PROFILE}");
    // The threshold the store applies to every row is profile 1 too.
    assert_eq!(codec::MIN_COMPRESS_SIZE, 4096, "{NEW_PROFILE}");
}

/// A floppy-like payload one byte longer than the limit: every 512-byte
/// sector carries its own number and a fixed fill, as a formatter's test
/// pattern leaves it, so the frames stay small but not trivial. Each case
/// takes a prefix of it.
fn floppy() -> Vec<u8> {
    let mut image = vec![0xf6; FLOPPY_LIMIT + 1];
    for (lba, sector) in image.chunks_mut(512).enumerate() {
        let label = format!("pc floppy sector {lba:06}");
        let n = label.len().min(sector.len());
        sector[..n].copy_from_slice(&label.as_bytes()[..n]);
    }
    image
}

struct Temp(PathBuf);
impl Temp {
    fn store() -> (Self, Store) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temp = Self(std::env::temp_dir().join(format!(
            "pc-zstd-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let store = Store::open(&temp.0).unwrap();
        (temp, store)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The descriptor and stored bytes of a floppy payload through the PC
/// provider's table, by both production paths, which must agree.
fn store_floppy(store: &Store, input: &[u8]) -> (Descriptor, Vec<u8>) {
    let types = PcProvider.payload_types();
    let size = input.len() as u64;
    let described = codec::describe(input, mt::FLOPPY, size, types).unwrap();
    let imported = store
        .import_payload(input, mt::FLOPPY, size, Some(&Digest::of(input)), types)
        .unwrap();
    assert_eq!(described, imported);
    let path = store
        .blob_file_path(&imported.digest, imported.size)
        .unwrap();
    (imported, fs::read(path).unwrap())
}

/// Each case: name, payload length, whether it is stored `+zstd`, and the
/// stored blob's length and SHA-256.
const BOUNDARY: [(&str, usize, bool, u64, &str); 5] = [
    (
        "one byte under the threshold",
        4095,
        false,
        4095,
        "sha256:b415aa71e6d17e3b0027df3d758d5a89e0fdc8323412ae589cf66fa4a912042b",
    ),
    (
        "exactly at the threshold",
        4096,
        true,
        72,
        "sha256:d48ba627db6ee9ef34d5a9ba8e2f65cd96e5ccc9cd306dfa72ed55847c6e6e18",
    ),
    (
        "one byte under the floppy limit",
        FLOPPY_LIMIT - 1,
        true,
        4832,
        "sha256:cba247d44d0a71401164fae583b81d3b04b609320271da763564d9fb5f80dfdd",
    ),
    (
        "exactly at the floppy limit",
        FLOPPY_LIMIT,
        true,
        4832,
        "sha256:3cad92997b09eced6ab9a866fe738479cdc712b2b9ed353cc804f93e0cf2ab04",
    ),
    (
        "one byte over the floppy limit",
        FLOPPY_LIMIT + 1,
        false,
        FLOPPY_LIMIT as u64 + 1,
        "sha256:90167a8627b87068e87ebc43ef6ad2ba32795e26a00c983af2ca8bfd034c4c72",
    ),
];

#[test]
fn the_pc_floppy_limit_and_the_threshold_decide_which_payloads_become_zstd() {
    let (_temp, store) = Temp::store();
    let image = floppy();
    let mut report = String::new();
    let mut failed = false;
    for (case, length, zstd, stored, digest) in BOUNDARY {
        let input = &image[..length];
        let (descriptor, blob) = store_floppy(&store, input);
        let compressed = descriptor.media_type == codec::compressed_type(mt::FLOPPY);
        if compressed {
            // The stored frame decodes, through the store's verified read,
            // to exactly the payload.
            assert_eq!(
                store.read_verified(&descriptor, length as u64).unwrap(),
                input,
                "{case}"
            );
            assert_eq!(
                descriptor.decoded_digest().unwrap(),
                Digest::of(input),
                "{case}"
            );
        } else {
            assert_eq!(descriptor.media_type, mt::FLOPPY, "{case}");
            assert!(descriptor.annotations.is_empty(), "{case}");
            assert_eq!(blob, input, "{case}");
        }
        assert_eq!(descriptor.digest, Digest::of(&blob), "{case}");
        let matches = compressed == zstd
            && blob.len() as u64 == stored
            && descriptor.digest.to_string() == digest;
        failed |= !matches;
        writeln!(
            report,
            "{} {case}: {length} bytes, {}, stored {}, {}",
            if matches { "ok  " } else { "DIFF" },
            if compressed { "+zstd" } else { "raw" },
            blob.len(),
            descriptor.digest
        )
        .unwrap();
    }
    assert!(!failed, "{NEW_PROFILE}:\n{report}");
}
