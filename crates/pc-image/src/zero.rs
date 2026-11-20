// SPDX-License-Identifier: AGPL-3.0-or-later
//! Blank fixed disks stored as zero bases; machine-images §§2 and 4.
//!
//! A new blank disk is defined as all zero bytes. Its payload is a small
//! descriptor that records only geometry and capacity, so it costs a few hundred
//! bytes in the store, in a registry and on the host, on every filesystem. Its
//! decoded identity, the SHA-256 of that many zero bytes, is computed
//! deterministically without reading or allocating disk data. Machines read the
//! base, and any sealed layers over it, straight through the sector-overlay
//! interface: unwritten sectors read as zeros, writes go to the session journal
//! and a save seals only the written sectors. Nothing is ever materialized.
//! The ATA/ATAPI-4 CHS bounds (16383 cylinders, 16 heads, 63 sectors per track)
//! are those of the existing fixed-disk validator.

use crate::{invalid, Result};
use image_store::{media_type as mt, BlobSource, Descriptor, Digest, ResolvedImage, Store};
use media::{layer::Geometry, Header, SectorSource, VerifiedBase, SECTOR_LEN};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Read},
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

/// Provider-declared payload type of a zero base descriptor.
pub const MEDIA_TYPE: &str = "application/vnd.unemu.disk.zero.v1+toml";
/// A zero base descriptor is a few hundred bytes; anything larger is refused.
pub const MAX_DESCRIPTOR: u64 = 4096;
const KIND: &str = "zero-base";

/// Geometry and capacity of an all-zero fixed disk. Canonical TOML, like a
/// sealed layer descriptor: decoding re-encodes and requires identical bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBase {
    pub schema: u32,
    pub kind: String,
    pub capacity_sectors: u64,
    pub geometry: Geometry,
}

impl ZeroBase {
    pub fn new(cylinders: u32, heads: u32, sectors: u32) -> Result<Self> {
        let geometry = Geometry {
            cylinders,
            heads,
            sectors,
            sector_bytes: 512,
        };
        let zero = Self {
            schema: 1,
            kind: KIND.into(),
            capacity_sectors: geometry.capacity().map_err(|e| invalid(e.to_string()))?,
            geometry,
        };
        zero.validate()?;
        Ok(zero)
    }
    fn validate(&self) -> Result<()> {
        let g = &self.geometry;
        if self.schema != 1
            || self.kind != KIND
            || !(1..=16383).contains(&g.cylinders)
            || !(1..=16).contains(&g.heads)
            || !(1..=63).contains(&g.sectors)
            || g.capacity().ok() != Some(self.capacity_sectors)
        {
            return Err(invalid("invalid zero base geometry or envelope"));
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_DESCRIPTOR {
            return Err(invalid("zero base descriptor too large"));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?;
        let zero: Self = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        zero.validate()?;
        if zero.encode()? != bytes {
            return Err(invalid("noncanonical zero base descriptor"));
        }
        Ok(zero)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(toml::to_string(self)
            .map_err(|e| invalid(e.to_string()))?
            .into_bytes())
    }
    /// Decoded length of the blank disk in bytes.
    pub fn byte_len(&self) -> u64 {
        self.capacity_sectors * SECTOR_LEN as u64
    }
    /// SHA-256 of the decoded content; never stored, always derived.
    pub fn identity(&self) -> Digest {
        zero_digest(self.byte_len())
    }
    /// Read a verified descriptor from the store or a provider blob source.
    pub fn read(descriptor: &Descriptor, blobs: &dyn BlobSource) -> Result<Self> {
        if descriptor.media_type != MEDIA_TYPE {
            return Err(invalid("not a zero base descriptor"));
        }
        Self::decode(&blobs.read_verified(descriptor, MAX_DESCRIPTOR)?)
    }
}

/// SHA-256 of zero bytes for the blank-disk presets offered by New machine…,
/// keyed by byte length, so the common sizes cost nothing at first Start.
const PRESETS: &[(u64, &str)] = &[
    (
        615 * 4 * 17 * 512,
        "5f2f30a8114b67137632d7052e84b9930b30d76caeab46de07986f73289b45ae",
    ),
    (
        940 * 8 * 17 * 512,
        "d8e242953c2a4fb2ec57b0c72cecb8970f03ddd4cff03a67bc984d930dd4cb9a",
    ),
    (
        256 * 16 * 63 * 512,
        "1ce558ba4b7eb198f42fc3a63bb183c0a6549763ebb718fcb76224f4956e8693",
    ),
    (
        512 * 16 * 63 * 512,
        "c2726f08d0177d1f658c4481190c914030028d6c9ff73bd6f28297051d60e53c",
    ),
    (
        1024 * 16 * 63 * 512,
        "c3962b0c4fc21cbc16689f72a2a6b35a0c1a235cdb605729ee3d30abbd3c5033",
    ),
];

/// SHA-256 of `length` zero bytes. The presets are constants; any other
/// length is streamed through one fixed buffer, so no allocation grows with
/// the disk, and remembered per length for the life of the process: the value
/// is a pure function of the length.
pub fn zero_digest(length: u64) -> Digest {
    if let Some((_, hex)) = PRESETS.iter().find(|(bytes, _)| *bytes == length) {
        return format!("sha256:{hex}")
            .parse()
            .expect("preset zero digests are valid SHA-256 strings");
    }
    static KNOWN: OnceLock<Mutex<BTreeMap<u64, Digest>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(digest) = known.lock().ok().and_then(|map| map.get(&length).cloned()) {
        return digest;
    }
    let digest = streamed_digest(length);
    if let Ok(mut map) = known.lock() {
        map.insert(length, digest.clone());
    }
    digest
}
fn streamed_digest(length: u64) -> Digest {
    media::layer::logical_digest(&ZeroSource(length))
        .expect("an all-zero source cannot fail to read")
        .parse()
        .expect("the streamed digest is a valid SHA-256 string")
}

/// The sectors of a zero base: every read returns zeros and nothing is stored.
pub struct ZeroSource(pub u64);
impl SectorSource for ZeroSource {
    fn byte_len(&self) -> u64 {
        self.0
    }
    fn read_sector(&self, _: u64) -> io::Result<[u8; SECTOR_LEN]> {
        Ok([0; SECTOR_LEN])
    }
    fn check_unchanged(&self) -> io::Result<()> {
        Ok(())
    }
    fn read_range(&self, _: u64, bytes: &mut [u8]) -> io::Result<()> {
        bytes.fill(0);
        Ok(())
    }
}

/// Written sectors over a zero base, top layer winning, decoded once.
type Written = Arc<BTreeMap<u64, Box<[u8; SECTOR_LEN]>>>;

/// A verified fixed-disk head rooted in a zero base, kept as its parts: the
/// zero base and the sectors its layers wrote, decoded into memory once, like
/// any other sector pack. A launch moves it to the execution thread and
/// rebuilds the reader there; no disk is materialized.
pub struct ZeroChain {
    zero: ZeroBase,
    written: Written,
    identity: Header,
}

/// A zero base with its written sectors: random access for a machine.
struct Sectors {
    length: u64,
    written: Written,
}
impl SectorSource for Sectors {
    fn byte_len(&self) -> u64 {
        self.length
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        Ok(self
            .written
            .get(&lba)
            .map_or([0; SECTOR_LEN], |sector| **sector))
    }
    fn check_unchanged(&self) -> io::Result<()> {
        Ok(())
    }
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        bytes.fill(0);
        let end = first_lba
            .checked_add(bytes.len().div_ceil(SECTOR_LEN) as u64)
            .ok_or_else(|| io::Error::other("zero base range overflow"))?;
        for (&lba, sector) in self.written.range(first_lba..end) {
            let start = (lba - first_lba) as usize * SECTOR_LEN;
            let count = (bytes.len() - start).min(SECTOR_LEN);
            bytes[start..start + count].copy_from_slice(&sector[..count]);
        }
        Ok(())
    }
}

impl ZeroChain {
    /// Rebuild the execution-local reader without reading or hashing anything.
    pub fn mount(&self) -> Result<VerifiedBase> {
        Ok(VerifiedBase::from_verified_source(
            Rc::new(Sectors {
                length: self.zero.byte_len(),
                written: self.written.clone(),
            }),
            self.identity.clone(),
        )?)
    }
    /// Share the decoded sectors with another thread's reader.
    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            zero: self.zero.clone(),
            written: self.written.clone(),
            identity: self.identity.clone(),
        })
    }
    /// The zero base itself, for sealing a flattened chain against it.
    pub fn root_source(&self) -> ZeroSource {
        ZeroSource(self.zero.byte_len())
    }
    /// Distinct sectors the chain's layers replace.
    pub fn stored_sectors(&self) -> u64 {
        self.written.len() as u64
    }
}

/// Whether a fixed-disk head is a zero base or a layer chain rooted in one.
pub(crate) fn rooted(
    head: &Digest,
    geometry: &crate::Geometry,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<bool> {
    let kind = descriptors
        .iter()
        .find(|d| d.names(head))
        .map(|d| d.decoded_media_type());
    if !matches!(kind, Some(MEDIA_TYPE | mt::DISK_LAYER)) {
        return Ok(false);
    }
    Ok(crate::disk::graph(head, geometry, descriptors, blobs)?
        .root
        .media_type
        == MEDIA_TYPE)
}

/// Mount a zero-rooted head of a resolved image, or `None` for any other head.
/// Sector packs, raw or compressed, are read once through verified content
/// readers into memory; the reconstructed head's full SHA-256 is then checked,
/// as every Start does for raw bases. A bare zero base has no stored data.
pub(crate) fn disk(
    store: &Store,
    medium: &crate::Medium,
    image: &ResolvedImage,
) -> Result<Option<crate::disk::Disk>> {
    let descriptors = &image.manifest.layers;
    let head_digest = crate::StoreDigest::store(&medium.head)?;
    let head = descriptors
        .iter()
        .find(|d| d.names(&head_digest))
        .ok_or_else(|| invalid("missing disk head"))?;
    if !matches!(head.media_type.as_str(), MEDIA_TYPE | mt::DISK_LAYER) {
        return Ok(None);
    }
    let graph = crate::disk::graph(&head_digest, &medium.geometry, descriptors, store)?;
    if graph.root.media_type != MEDIA_TYPE {
        return Ok(None);
    }
    let zero = ZeroBase::read(&graph.root, store)?;
    let layered = !graph.layers.is_empty();
    let mut written = BTreeMap::new();
    let mut identity = zero.identity();
    for (_, layer, pack) in graph.layers.into_iter().rev() {
        let mut reader = image.content_reader(&pack.decoded_digest()?)?;
        let mut previous = None;
        for _ in 0..layer.sector_count {
            let mut address = [0; 8];
            let mut sector = Box::new([0; SECTOR_LEN]);
            reader.read_exact(&mut address)?;
            reader.read_exact(&mut sector[..])?;
            let lba = u64::from_le_bytes(address);
            if lba >= layer.capacity_sectors || previous.is_some_and(|p| lba <= p) {
                return Err(invalid("unordered or out-of-range sector pack"));
            }
            previous = Some(lba);
            written.insert(lba, sector);
        }
        // The final empty read lets the reader check the pack's length and SHA-256.
        if reader.read(&mut [0; 1])? != 0 {
            return Err(invalid("sector pack longer than its record count"));
        }
        identity = layer.result_sha256.parse()?;
    }
    let chain = ZeroChain {
        identity: Header {
            base_length: zero.byte_len(),
            base_digest: crate::provider::hash_array(&identity)?,
            anchor_generation: 0,
        },
        zero,
        written: Arc::new(written),
    };
    let base = chain.mount()?;
    if layered && media::layer::logical_digest(&base)? != identity.to_string() {
        return Err(invalid("reconstructed disk digest mismatch"));
    }
    Ok(Some(crate::disk::Disk {
        // Never created: the name only keys the verified reader handed to assembly.
        path: Some(
            store
                .root()
                .join("cache/unmaterialized")
                .join(medium.head.hex()),
        ),
        identity,
        base,
        root: graph.root.digest,
        zero: Some(Rc::new(chain)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_the_sha256_of_that_many_zero_bytes() {
        for length in [0_u64, 1, 512, 65536, 65537, 3 * 512 * 17] {
            assert_eq!(
                zero_digest(length),
                Digest::of(&vec![0; length as usize]),
                "{length}"
            );
        }
    }

    #[test]
    fn preset_constants_match_the_streamed_hash() {
        for (length, _) in PRESETS {
            assert_eq!(zero_digest(*length), streamed_digest(*length), "{length}");
        }
        for chs in crate::create::BLANK_DISKS {
            assert!(PRESETS.iter().any(|(length, _)| *length == chs.bytes()));
        }
    }

    #[test]
    fn descriptor_is_canonical_bounded_and_rejects_unknown_fields() {
        let zero = ZeroBase::new(1024, 16, 63).unwrap();
        assert_eq!(zero.byte_len(), 1024 * 16 * 63 * 512);
        let bytes = zero.encode().unwrap();
        assert!(bytes.len() < 256, "{}", bytes.len());
        assert_eq!(ZeroBase::decode(&bytes).unwrap(), zero);
        let mut extra = bytes.clone();
        extra.push(b'\n');
        assert!(ZeroBase::decode(&extra).is_err());
        let text = String::from_utf8(bytes).unwrap();
        assert!(ZeroBase::decode(format!("other = 1\n{text}").as_bytes()).is_err());
        assert!(ZeroBase::decode(text.replace("schema = 1", "schema = 2").as_bytes()).is_err());
        assert!(ZeroBase::decode(
            text.replace("capacity_sectors = 1032192", "capacity_sectors = 1")
                .as_bytes()
        )
        .is_err());
        for (c, h, s) in [(0, 16, 63), (16384, 16, 63), (1024, 17, 63), (1024, 16, 64)] {
            assert!(ZeroBase::new(c, h, s).is_err(), "{c}/{h}/{s}");
        }
    }

    #[test]
    fn every_sector_reads_as_zero_without_storage() {
        let source = ZeroSource(4 * 512);
        assert_eq!(source.read_sector(3).unwrap(), [0; 512]);
        let mut bytes = [0xA5; 1024];
        source.read_range(1, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 1024]);
    }
}
