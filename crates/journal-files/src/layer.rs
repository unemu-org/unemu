// SPDX-License-Identifier: AGPL-3.0-or-later
//! Immutable fixed-disk descriptors and sorted sector packs; machine-images §4.
//! This host format uses the existing sector-source seam, independently of controllers.

use crate::{SectorSource, VerifiedBase, SECTOR_LEN};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
};

pub const MEDIA_TYPE: &str = "application/vnd.unemu.disk.layer.v1+toml";
pub const SECTORS_TYPE: &str = "application/vnd.unemu.disk.sectors.v1";
pub const RECORD_LEN: u64 = 520;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub cylinders: u32,
    pub heads: u32,
    pub sectors: u32,
    pub sector_bytes: u32,
}
impl Geometry {
    pub fn capacity(&self) -> io::Result<u64> {
        u64::from(self.cylinders)
            .checked_mul(self.heads.into())
            .and_then(|n| n.checked_mul(self.sectors.into()))
            .filter(|&n| n > 0 && self.sector_bytes == 512)
            .ok_or_else(|| io::Error::other("invalid layer geometry"))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub schema: u32,
    pub kind: String,
    pub parent: String,
    pub base: String,
    pub geometry: Geometry,
    pub sectors: String,
    pub sector_count: u64,
    pub result_sha256: String,
    pub capacity_sectors: u64,
}
impl Descriptor {
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > 1024 * 1024 {
            return Err(io::Error::other("layer descriptor too large"));
        }
        let descriptor: Self =
            toml::from_str(std::str::from_utf8(bytes).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
        descriptor.validate()?;
        if descriptor.encode()? != bytes {
            return Err(io::Error::other("noncanonical layer descriptor"));
        }
        Ok(descriptor)
    }
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        Ok(toml::to_string(self)
            .map_err(io::Error::other)?
            .into_bytes())
    }
    fn validate(&self) -> io::Result<()> {
        for digest in [&self.parent, &self.base, &self.sectors, &self.result_sha256] {
            validate_digest(digest)?;
        }
        if self.schema != 1
            || self.kind != "disk-layer"
            || self.geometry.capacity()? != self.capacity_sectors
            || self.capacity_sectors.checked_mul(512).is_none()
            || self.sector_count > self.capacity_sectors
        {
            return Err(io::Error::other("invalid layer envelope or capacity"));
        }
        Ok(())
    }
}
fn validate_digest(value: &str) -> io::Result<()> {
    if value.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        Ok(())
    } else {
        Err(io::Error::other("invalid layer SHA-256"))
    }
}
fn hex(hash: impl AsRef<[u8]>) -> String {
    format!(
        "sha256:{}",
        hash.as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
/// Stream the complete logical disk, including untouched sectors.
pub fn logical_digest(source: &dyn SectorSource) -> io::Result<String> {
    copy_and_digest(source, &mut io::sink())
}
/// Hash and export the logical head in one bounded streaming pass.
pub fn copy_and_digest(source: &dyn SectorSource, output: &mut impl Write) -> io::Result<String> {
    #[cfg(feature = "test-support")]
    crate::overlay::note_data_pass(source.byte_len());
    let mut hasher = Sha256::new();
    let mut position = 0;
    let mut bytes = [0; 64 * 1024];
    while position < source.byte_len() {
        let count = (source.byte_len() - position).min(bytes.len() as u64) as usize;
        source.read_range(position / 512, &mut bytes[..count])?;
        output.write_all(&bytes[..count])?;
        hasher.update(&bytes[..count]);
        position += count as u64;
    }
    source.check_unchanged()?;
    Ok(hex(hasher.finalize()))
}

/// A flattened LBA index keeps read cost independent of layer depth. Pack files
/// stay open and metadata-checked, so sectors are never retained as whole disks.
pub struct Chain {
    root: std::rc::Rc<dyn SectorSource>,
    head: String,
    base: String,
    geometry: Geometry,
    seen: BTreeSet<String>,
    packs: Vec<(File, std::fs::Metadata)>,
    index: BTreeMap<u64, (usize, u64)>,
}
impl Chain {
    pub fn new(root: VerifiedBase, geometry: Geometry) -> io::Result<Self> {
        let base = hex(root.identity().base_digest);
        Self::from_source(std::rc::Rc::new(root), base, geometry)
    }
    /// Compose an immutable source without changing the controller-facing seam.
    pub fn from_source(
        root: std::rc::Rc<dyn SectorSource>,
        base: String,
        geometry: Geometry,
    ) -> io::Result<Self> {
        validate_digest(&base)?;
        if geometry.capacity()?.checked_mul(512) != Some(root.byte_len()) {
            return Err(io::Error::other("root geometry mismatch"));
        }
        Ok(Self {
            root,
            head: base.clone(),
            base: base.clone(),
            geometry,
            seen: BTreeSet::from([base]),
            packs: Vec::new(),
            index: BTreeMap::new(),
        })
    }
    pub fn head(&self) -> &str {
        &self.head
    }
    pub fn verify_head(&self, expected: &str) -> io::Result<()> {
        if logical_digest(self)? == expected {
            Ok(())
        } else {
            Err(io::Error::other("reconstructed disk digest mismatch"))
        }
    }
    pub fn base(&self) -> &str {
        &self.base
    }
    /// Index an exact child after verifying its pack. Call `verify_head` once
    /// after all children are indexed, so hashing cost does not grow with depth.
    pub fn apply(
        &mut self,
        digest: &str,
        descriptor: &Descriptor,
        mut pack: File,
    ) -> io::Result<()> {
        let metadata = pack.metadata()?;
        if hex(crate::overlay::hash_reader(&mut pack)?) != descriptor.sectors {
            return Err(io::Error::other("sector pack digest mismatch"));
        }
        self.apply_verified(digest, descriptor, pack, metadata)
    }
    /// Reuse a pack's completed object verification on the same descriptor.
    pub fn apply_verified(
        &mut self,
        digest: &str,
        descriptor: &Descriptor,
        mut pack: File,
        metadata: std::fs::Metadata,
    ) -> io::Result<()> {
        descriptor.validate()?;
        validate_digest(digest)?;
        if self.seen.contains(digest) {
            return Err(io::Error::other("layer cycle"));
        }
        if self.packs.len() >= 64 {
            return Err(io::Error::other(
                "disk chain exceeds 64 layers; flatten before adding a layer",
            ));
        }
        if descriptor.parent != self.head
            || descriptor.base != self.base
            || descriptor.geometry != self.geometry
        {
            return Err(io::Error::other("layer parent or geometry mismatch"));
        }
        check_pack(&(pack.try_clone()?, metadata.clone()))?;
        if !metadata.is_file()
            || Some(metadata.len()) != descriptor.sector_count.checked_mul(RECORD_LEN)
        {
            return Err(io::Error::other("sector pack digest or length mismatch"));
        }
        pack.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::with_capacity(64 * 1024, pack);
        let mut previous = None;
        let mut next_index: BTreeMap<_, _> = self
            .index
            .iter()
            .map(|(&lba, &location)| (lba, location))
            .collect();
        for n in 0..descriptor.sector_count {
            let mut record = [0; RECORD_LEN as usize];
            reader.read_exact(&mut record)?;
            let mut address = [0; 8];
            address.copy_from_slice(&record[..8]);
            let lba = u64::from_le_bytes(address);
            if lba >= descriptor.capacity_sectors || previous.is_some_and(|p| lba <= p) {
                return Err(io::Error::other("unordered or out-of-range sector pack"));
            }
            previous = Some(lba);
            next_index.insert(lba, (self.packs.len(), n * RECORD_LEN + 8));
        }
        let pack = reader.into_inner();
        check_pack(&(pack.try_clone()?, metadata.clone()))?;
        self.packs.push((pack, metadata));
        std::mem::swap(&mut self.index, &mut next_index);
        self.head = digest.into();
        self.seen.insert(digest.into());
        Ok(())
    }
}
impl SectorSource for Chain {
    fn byte_len(&self) -> u64 {
        self.root.byte_len()
    }
    fn check_unchanged(&self) -> io::Result<()> {
        self.root.check_unchanged()?;
        for pack in &self.packs {
            check_pack(pack)?;
        }
        Ok(())
    }
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        self.check_unchanged()?;
        self.root.read_range(first_lba, bytes)?;
        let end = first_lba
            .checked_add(bytes.len().div_ceil(SECTOR_LEN) as u64)
            .ok_or_else(|| io::Error::other("chain range overflow"))?;
        let mut entries = self.index.range(first_lba..end).peekable();
        let mut buffer = [0; 64 * 1024];
        while let Some((&lba, &(pack, offset))) = entries.next() {
            let mut records = 1;
            while records < buffer.len() / RECORD_LEN as usize
                && entries.peek().is_some_and(|(next_lba, location)| {
                    **next_lba == lba + records as u64
                        && location.0 == pack
                        && location.1 == offset + records as u64 * RECORD_LEN
                })
            {
                entries.next();
                records += 1;
            }
            let length = (records - 1) * RECORD_LEN as usize + SECTOR_LEN;
            read_at(&self.packs[pack].0, offset, &mut buffer[..length])?;
            for index in 0..records {
                let start = (lba - first_lba) as usize * SECTOR_LEN + index * SECTOR_LEN;
                let count = SECTOR_LEN.min(bytes.len() - start);
                let source = index * RECORD_LEN as usize;
                bytes[start..start + count].copy_from_slice(&buffer[source..source + count]);
            }
        }
        self.check_unchanged()
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        self.root.check_unchanged()?;
        if let Some(&(pack, offset)) = self.index.get(&lba) {
            check_pack(&self.packs[pack])?;
            let mut bytes = [0; SECTOR_LEN];
            read_at(&self.packs[pack].0, offset, &mut bytes)?;
            check_pack(&self.packs[pack])?;
            self.root.check_unchanged()?;
            Ok(bytes)
        } else {
            self.root.read_sector(lba)
        }
    }
}
#[cfg(test)]
thread_local! { static READ_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn read_at(file: &File, offset: u64, bytes: &mut [u8]) -> io::Result<()> {
    #[cfg(test)]
    READ_CALLS.with(|count| count.set(count.get() + 1));
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(bytes, offset)
    }
    #[cfg(not(unix))]
    {
        let mut file = file.try_clone()?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(bytes)
    }
}
fn check_pack((file, before): &(File, std::fs::Metadata)) -> io::Result<()> {
    let now = file.metadata()?;
    #[cfg(unix)]
    let same = {
        use std::os::unix::fs::MetadataExt;
        before.len() == now.len()
            && before.mtime() == now.mtime()
            && before.mtime_nsec() == now.mtime_nsec()
            && before.ctime() == now.ctime()
            && before.ctime_nsec() == now.ctime_nsec()
    };
    #[cfg(not(unix))]
    let same = before.len() == now.len() && before.modified()? == now.modified()?;
    if same {
        Ok(())
    } else {
        Err(io::Error::other("sector pack changed after verification"))
    }
}
/// Emit final differing sectors in LBA order, discarding journal history and
/// writes that restored their parent bytes. Memory usage stays bounded.
pub fn seal(
    parent: &dyn SectorSource,
    final_disk: &dyn SectorSource,
    geometry: Geometry,
    parent_digest: &str,
    base: &str,
    output: &mut impl Write,
) -> io::Result<Descriptor> {
    if geometry.capacity()?.checked_mul(512) != Some(parent.byte_len())
        || parent.byte_len() != final_disk.byte_len()
    {
        return Err(io::Error::other("seal capacity mismatch"));
    }
    let mut output = BufWriter::with_capacity(64 * 1024, output);
    let mut pack = Sha256::new();
    let mut result = Sha256::new();
    let mut count = 0;
    let mut parent_bytes = [0; 64 * 1024];
    let mut final_bytes = [0; 64 * 1024];
    let mut first = 0;
    while first < geometry.capacity()? {
        let sectors =
            (geometry.capacity()? - first).min((final_bytes.len() / SECTOR_LEN) as u64) as usize;
        let length = sectors * SECTOR_LEN;
        parent.read_range(first, &mut parent_bytes[..length])?;
        final_disk.read_range(first, &mut final_bytes[..length])?;
        result.update(&final_bytes[..length]);
        for index in 0..sectors {
            let range = index * SECTOR_LEN..(index + 1) * SECTOR_LEN;
            if parent_bytes[range.clone()] != final_bytes[range.clone()] {
                let lba = (first + index as u64).to_le_bytes();
                output.write_all(&lba)?;
                output.write_all(&final_bytes[range.clone()])?;
                pack.update(lba);
                pack.update(&final_bytes[range]);
                count += 1;
            }
        }
        first += sectors as u64;
    }
    output.flush()?;
    parent.check_unchanged()?;
    final_disk.check_unchanged()?;
    let descriptor = Descriptor {
        schema: 1,
        kind: "disk-layer".into(),
        parent: parent_digest.into(),
        base: base.into(),
        geometry,
        sectors: hex(pack.finalize()),
        sector_count: count,
        result_sha256: hex(result.finalize()),
        capacity_sectors: parent.byte_len() / 512,
    };
    descriptor.validate()?;
    Ok(descriptor)
}
/// Borrowed streaming view used for store ingestion and raw exports.
pub struct SectorReader<'a> {
    pub source: &'a dyn SectorSource,
    pub position: u64,
}
impl Read for SectorReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = (self.source.byte_len().saturating_sub(self.position)).min(output.len() as u64)
            as usize;
        if self.position.is_multiple_of(512) && count > 0 {
            self.source
                .read_range(self.position / 512, &mut output[..count])?;
            self.position += count as u64;
            return Ok(count);
        }
        let mut done = 0;
        while done < count {
            let offset = (self.position % 512) as usize;
            let sector = self.source.read_sector(self.position / 512)?;
            let n = (512 - offset).min(count - done);
            output[done..done + n].copy_from_slice(&sector[offset..offset + n]);
            done += n;
            self.position += n as u64;
        }
        self.source.check_unchanged()?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sector-layers-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn pack(&self, bytes: &[u8]) -> File {
            let path = self
                .0
                .join(format!("pack-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::write(&path, bytes).unwrap();
            File::open(path).unwrap()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn geometry() -> Geometry {
        Geometry {
            cylinders: 1,
            heads: 1,
            sectors: 257,
            sector_bytes: 512,
        }
    }
    #[test]
    fn deterministic_random_writes_match_raw_after_every_layer() {
        let temp = Temp::new();
        let mut raw = vec![0; 257 * 512];
        let base = VerifiedBase::from_bytes(&raw).unwrap();
        let mut chain = Chain::new(base, geometry()).unwrap();
        let mut random = 123456789_u64;
        for _ in 0..4 {
            for _ in 0..100 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let lba = random as usize % 257;
                raw[lba * 512..(lba + 1) * 512].fill((random >> 32) as u8);
            }
            let final_disk = VerifiedBase::from_bytes(&raw).unwrap();
            let mut pack = Vec::new();
            let layer = seal(
                &chain,
                &final_disk,
                geometry(),
                chain.head(),
                chain.base(),
                &mut pack,
            )
            .unwrap();
            let encoded = layer.encode().unwrap();
            assert_eq!(
                Descriptor::decode(&encoded).unwrap().encode().unwrap(),
                encoded
            );
            let mut second = Vec::new();
            let repeated = seal(
                &chain,
                &final_disk,
                geometry(),
                chain.head(),
                chain.base(),
                &mut second,
            )
            .unwrap();
            assert_eq!(pack, second);
            assert_eq!(encoded, repeated.encode().unwrap());
            let id = hex(Sha256::digest(&encoded));
            chain.apply(&id, &layer, temp.pack(&pack)).unwrap();
            chain.verify_head(&layer.result_sha256).unwrap();
            for lba in 0..257 {
                assert_eq!(
                    &chain.read_sector(lba).unwrap(),
                    &raw[lba as usize * 512..(lba as usize + 1) * 512]
                );
            }
        }
    }
    #[test]
    fn rejects_wrong_parent_cycle_geometry_and_logical_digest() {
        let temp = Temp::new();
        let base = VerifiedBase::from_bytes(&vec![0; 257 * 512]).unwrap();
        let mut chain = Chain::new(base.clone(), geometry()).unwrap();
        let mut pack = Vec::new();
        let layer = seal(
            &base,
            &VerifiedBase::from_bytes(&vec![1; 257 * 512]).unwrap(),
            geometry(),
            chain.head(),
            chain.base(),
            &mut pack,
        )
        .unwrap();
        let id = hex(Sha256::digest(layer.encode().unwrap()));
        let mut bad = layer.clone();
        bad.parent = hex([7; 32]);
        assert!(chain
            .apply(&id, &bad, temp.pack(&pack))
            .unwrap_err()
            .to_string()
            .contains("parent"));
        assert!(chain
            .apply(chain.head().to_owned().as_str(), &layer, temp.pack(&pack))
            .unwrap_err()
            .to_string()
            .contains("cycle"));
        let mut bad = layer.clone();
        bad.geometry.heads = 2;
        assert!(chain.apply(&id, &bad, temp.pack(&pack)).is_err());
        chain.apply(&id, &layer, temp.pack(&pack)).unwrap();
        assert!(chain
            .verify_head(&hex([0; 32]))
            .unwrap_err()
            .to_string()
            .contains("reconstructed"));
        chain.verify_head(&layer.result_sha256).unwrap();
    }
    #[test]
    fn pack_bounds_order_hash_and_unknown_schema_are_checked() {
        let temp = Temp::new();
        let base = VerifiedBase::from_bytes(&vec![0; 257 * 512]).unwrap();
        let mut chain = Chain::new(base.clone(), geometry()).unwrap();
        let mut bytes = Vec::new();
        let layer = seal(
            &base,
            &VerifiedBase::from_bytes(&vec![1; 257 * 512]).unwrap(),
            geometry(),
            chain.head(),
            chain.base(),
            &mut bytes,
        )
        .unwrap();
        let id = hex([2; 32]);
        for lba in [0_u64, 257] {
            let mut corrupt = bytes.clone();
            corrupt[520..528].copy_from_slice(&lba.to_le_bytes());
            let mut bad = layer.clone();
            bad.sectors = hex(Sha256::digest(&corrupt));
            assert!(chain.apply(&id, &bad, temp.pack(&corrupt)).is_err());
        }
        let mut corrupt = bytes.clone();
        corrupt[10] ^= 1;
        assert!(chain.apply(&id, &layer, temp.pack(&corrupt)).is_err());
        assert!(chain
            .apply(&id, &layer, temp.pack(&bytes[..bytes.len() - 1]))
            .is_err());
        let text = String::from_utf8(layer.encode().unwrap()).unwrap();
        assert!(Descriptor::decode(text.replace("schema = 1", "schema = 2").as_bytes()).is_err());
        assert!(Descriptor::decode(format!("unexpected=1\n{text}").as_bytes()).is_err());
    }
    #[test]
    fn restored_parent_sectors_make_an_empty_pack() {
        let base = VerifiedBase::from_bytes(&vec![0; 257 * 512]).unwrap();
        let mut pack = Vec::new();
        let digest = hex(base.identity().base_digest);
        let layer = seal(&base, &base, geometry(), &digest, &digest, &mut pack).unwrap();
        assert!(pack.is_empty());
        assert_eq!(layer.sector_count, 0);
        assert_eq!(layer.result_sha256, digest);
    }
    #[test]
    fn logical_chain_reads_are_batched_independent_of_depth() {
        use std::{cell::Cell, rc::Rc};
        struct Counted {
            base: VerifiedBase,
            calls: Rc<Cell<usize>>,
        }
        impl SectorSource for Counted {
            fn byte_len(&self) -> u64 {
                self.base.byte_len()
            }
            fn check_unchanged(&self) -> io::Result<()> {
                self.base.check_unchanged()
            }
            fn read_sector(&self, _: u64) -> io::Result<[u8; 512]> {
                panic!("whole-disk passes must use ranges")
            }
            fn read_range(&self, lba: u64, bytes: &mut [u8]) -> io::Result<()> {
                self.calls.set(self.calls.get() + 1);
                self.base.read_range(lba, bytes)
            }
        }
        let temp = Temp::new();
        let calls = Rc::new(Cell::new(0));
        let bytes = vec![0; 1024 * 1024];
        let root = VerifiedBase::from_bytes(&bytes).unwrap();
        let digest = hex(root.identity().base_digest);
        let geometry = Geometry {
            cylinders: 1,
            heads: 1,
            sectors: 2048,
            sector_bytes: 512,
        };
        let mut chain = Chain::from_source(
            Rc::new(Counted {
                base: root,
                calls: calls.clone(),
            }),
            digest,
            geometry.clone(),
        )
        .unwrap();
        for value in 1..=3 {
            let final_disk = VerifiedBase::from_bytes(&vec![value; bytes.len()]).unwrap();
            let mut pack = Vec::new();
            let layer = seal(
                &chain,
                &final_disk,
                geometry.clone(),
                chain.head(),
                chain.base(),
                &mut pack,
            )
            .unwrap();
            let id = hex(Sha256::digest(layer.encode().unwrap()));
            chain.apply(&id, &layer, temp.pack(&pack)).unwrap();
        }
        calls.set(0);
        READ_CALLS.with(|count| count.set(0));
        logical_digest(&chain).unwrap();
        assert_eq!(calls.get(), 16);
        assert!(READ_CALLS.with(|count| count.get()) <= 32);
    }
    #[test]
    fn noncanonical_layer_toml_is_rejected() {
        let base = VerifiedBase::from_bytes(&vec![0; 257 * 512]).unwrap();
        let digest = hex(base.identity().base_digest);
        let layer = seal(&base, &base, geometry(), &digest, &digest, &mut Vec::new()).unwrap();
        let bytes = layer.encode().unwrap();
        let mut extra = bytes.clone();
        extra.push(b'\n');
        assert!(Descriptor::decode(&extra).is_err());
        Descriptor::decode(&bytes).unwrap();
    }
}
