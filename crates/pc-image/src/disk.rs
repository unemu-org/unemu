// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fixed-disk closure traversal and shared raw journal bases; machine-images §4.
//! Journals keep their original raw-byte identity instead of naming descriptors.
//! Validation writes nothing. Mounting materializes only what a machine opens
//! as a file: a hard-disk base and a flattened layered head. Compressed
//! floppies decode into memory, and layered heads stream their members.

use crate::*;
use image_store::{media_type as mt, BlobSource, ContentReader, ResolvedImage, Store};
use journal_files::{layer::Descriptor as Layer, SectorSource, VerifiedBase, SECTOR_LEN};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufReader, BufWriter, Read, Write},
    path::PathBuf,
};

pub(crate) fn geometry(g: &Geometry) -> journal_files::layer::Geometry {
    journal_files::layer::Geometry {
        cylinders: g.cylinders,
        heads: g.heads,
        sectors: g.sectors,
        sector_bytes: g.sector_bytes,
    }
}
pub(crate) struct Graph {
    pub(crate) root: Descriptor,
    pub(crate) layers: Vec<(Digest, Layer, Descriptor)>,
    pub(crate) refs: Vec<Digest>,
}
pub(crate) fn graph(
    head: &Digest,
    g: &Geometry,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<Graph> {
    let mut next = head.clone();
    let mut seen = BTreeSet::new();
    let mut layers = Vec::new();
    let mut refs = Vec::new();
    let root = loop {
        if !seen.insert(next.clone()) {
            return Err(invalid("disk chain cycle"));
        }
        let d = descriptors
            .iter()
            .find(|d| d.names(&next))
            .ok_or_else(|| invalid("missing disk parent"))?;
        refs.push(next.clone());
        match d.decoded_media_type() {
            mt::DISK_BASE | crate::zero::MEDIA_TYPE => break d.clone(),
            mt::DISK_LAYER => {
                if layers.len() >= 64 {
                    return Err(invalid(
                        "disk chain exceeds 64 layers; publish with --flatten",
                    ));
                }
                let layer = Layer::decode(&blobs.read_verified(d, image_store::MAX_SMALL_BLOB)?)?;
                if layer.geometry != geometry(g) {
                    return Err(invalid("chain geometry mismatch"));
                }
                let pack = descriptor(descriptors, &layer.sectors.parse()?, mt::DISK_SECTORS)?;
                if Some(pack.decoded_size()?)
                    != layer
                        .sector_count
                        .checked_mul(journal_files::layer::RECORD_LEN)
                {
                    return Err(invalid("sector pack size mismatch"));
                }
                refs.push(pack.decoded_digest()?);
                next = layer.parent.parse()?;
                layers.push((d.digest.clone(), layer, pack));
            }
            _ => return Err(invalid("invalid fixed-disk parent type")),
        }
    };
    let root_length = if root.media_type == crate::zero::MEDIA_TYPE {
        let zero = crate::zero::ZeroBase::read(&root, blobs)?;
        if zero.geometry != geometry(g) {
            return Err(invalid("zero base geometry mismatch"));
        }
        zero.byte_len()
    } else {
        root.decoded_size()?
    };
    if geometry(g).capacity()?.checked_mul(512) != Some(root_length) {
        return Err(invalid("root geometry mismatch"));
    }
    let root_digest = root.decoded_digest()?.to_string();
    if layers.iter().any(|(_, layer, _)| layer.base != root_digest) {
        return Err(invalid("layer root mismatch"));
    }
    Ok(Graph { root, layers, refs })
}
pub(crate) fn display_chain(
    head: &Digest,
    g: &Geometry,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<Vec<(Digest, u64)>> {
    let graph = graph(head, g, descriptors, blobs)?;
    let mut members = vec![(graph.root.digest, graph.root.size)];
    for (id, _, pack) in graph.layers.into_iter().rev() {
        let descriptor = descriptors
            .iter()
            .find(|d| d.digest == id)
            .ok_or_else(|| invalid("missing layer"))?;
        members.push((id, descriptor.size + pack.size));
    }
    Ok(members)
}
pub(crate) fn references(
    head: &Digest,
    g: &Geometry,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<Vec<Digest>> {
    Ok(graph(head, g, descriptors, blobs)?.refs)
}
pub(crate) fn identity(
    head: &Digest,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<Digest> {
    let d = descriptors
        .iter()
        .find(|d| d.names(head))
        .ok_or_else(|| invalid("missing disk head"))?;
    if d.media_type == mt::DISK_LAYER {
        Ok(
            Layer::decode(&blobs.read_verified(d, image_store::MAX_SMALL_BLOB)?)?
                .result_sha256
                .parse()?,
        )
    } else if d.media_type == crate::zero::MEDIA_TYPE {
        Ok(crate::zero::ZeroBase::read(d, blobs)?.identity())
    } else {
        Ok(head.clone())
    }
}
/// A file-backed base from the verified file the image holds for `d`; for a
/// compressed hard disk this materializes its decoded file on first use.
fn file_base(image: &ResolvedImage, d: &Descriptor) -> Result<VerifiedBase> {
    let (file, metadata) = image.verified_file(&d.decoded_digest()?)?;
    Ok(VerifiedBase::from_verified_file(
        file,
        journal_files::Header {
            base_length: d.decoded_size()?,
            base_digest: super::provider::hash_array(&d.decoded_digest()?)?,
            anchor_generation: 0,
        },
        metadata,
    )?)
}
/// A compressed floppy is small and bounded, so it decodes into memory and
/// never becomes a cache file.
fn memory_base(image: &ResolvedImage, d: &Descriptor) -> Result<VerifiedBase> {
    let mut bytes = Vec::new();
    image
        .content_reader(&d.decoded_digest()?)?
        .read_to_end(&mut bytes)?;
    VerifiedBase::from_bytes(&bytes).map_err(|e| invalid(e.to_string()))
}

/// One sequential pass over a layered disk. The base and every sector pack
/// are read once, in LBA order, through verified content readers, and a
/// higher layer's record replaces the sector below it, so neither a
/// compressed base nor a compressed pack is decoded to a file or held whole
/// in memory. Reads must run contiguously from LBA 0, the way
/// `journal_files::layer::copy_and_digest` drives a source; each member's length,
/// SHA-256 and receipt are checked when the last sector has been read.
struct Layered {
    length: u64,
    stream: RefCell<Stream>,
}
struct Stream {
    /// `None` for a zero base, whose sectors all read as zeros.
    base: Option<BufReader<ContentReader>>,
    position: u64,
    /// Bottom layer first.
    packs: Vec<Pack>,
}
struct Pack {
    reader: BufReader<ContentReader>,
    remaining: u64,
    capacity: u64,
    previous: Option<u64>,
    next: Option<(u64, [u8; SECTOR_LEN])>,
}
impl Pack {
    /// The LBA of the next unconsumed record, reading it when needed.
    fn peek(&mut self) -> io::Result<Option<u64>> {
        if self.next.is_none() && self.remaining > 0 {
            let mut address = [0; 8];
            let mut sector = [0; SECTOR_LEN];
            self.reader.read_exact(&mut address)?;
            self.reader.read_exact(&mut sector)?;
            let lba = u64::from_le_bytes(address);
            if lba >= self.capacity || self.previous.is_some_and(|p| lba <= p) {
                return Err(io::Error::other("unordered or out-of-range sector pack"));
            }
            self.previous = Some(lba);
            self.remaining -= 1;
            self.next = Some((lba, sector));
        }
        Ok(self.next.as_ref().map(|(lba, _)| *lba))
    }
}
impl SectorSource for Layered {
    fn byte_len(&self) -> u64 {
        self.length
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        let mut sector = [0; SECTOR_LEN];
        self.read_range(lba, &mut sector)?;
        Ok(sector)
    }
    fn check_unchanged(&self) -> io::Result<()> {
        // Each content reader checks its receipt when its stream ends.
        Ok(())
    }
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        let mut stream = self
            .stream
            .try_borrow_mut()
            .map_err(|_| io::Error::other("layered disk read re-entered"))?;
        let start = first_lba
            .checked_mul(SECTOR_LEN as u64)
            .filter(|start| *start == stream.position)
            .ok_or_else(|| io::Error::other("layered disk read out of order"))?;
        let end = start
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= self.length)
            .ok_or_else(|| io::Error::other("layered disk read past its end"))?;
        match &mut stream.base {
            Some(base) => base.read_exact(bytes)?,
            None => bytes.fill(0),
        }
        stream.position = end;
        let last = first_lba + bytes.len().div_ceil(SECTOR_LEN) as u64;
        for pack in &mut stream.packs {
            while let Some(lba) = pack.peek()? {
                if lba >= last {
                    break;
                }
                let offset = lba
                    .checked_sub(first_lba)
                    .ok_or_else(|| io::Error::other("unordered sector pack"))?
                    as usize
                    * SECTOR_LEN;
                let count = SECTOR_LEN.min(bytes.len() - offset);
                if let Some((_, sector)) = pack.next.take() {
                    bytes[offset..offset + count].copy_from_slice(&sector[..count]);
                }
            }
        }
        if end == self.length {
            // The end of the disk is the end of every member; the final empty
            // read lets each reader check length, SHA-256 and receipt.
            let mut probe = [0; 1];
            if let Some(base) = &mut stream.base {
                if base.read(&mut probe)? != 0 {
                    return Err(io::Error::other("disk base longer than its geometry"));
                }
            }
            for pack in &mut stream.packs {
                if pack.peek()?.is_some() || pack.reader.read(&mut probe)? != 0 {
                    return Err(io::Error::other("unordered or out-of-range sector pack"));
                }
            }
        }
        Ok(())
    }
}
fn layered(store: &Store, head: &Digest, g: &Geometry, image: &ResolvedImage) -> Result<Layered> {
    let graph = graph(head, g, &image.manifest.layers, store)?;
    let (base, length) = if graph.root.media_type == crate::zero::MEDIA_TYPE {
        (
            None,
            crate::zero::ZeroBase::read(&graph.root, store)?.byte_len(),
        )
    } else {
        let base = image.content_reader(&graph.root.decoded_digest()?)?;
        let length = base.size();
        (Some(BufReader::with_capacity(64 * 1024, base)), length)
    };
    let mut packs = Vec::new();
    for (_, layer, pack) in graph.layers.into_iter().rev() {
        packs.push(Pack {
            reader: BufReader::with_capacity(
                64 * 1024,
                image.content_reader(&pack.decoded_digest()?)?,
            ),
            remaining: layer.sector_count,
            capacity: layer.capacity_sectors,
            previous: None,
            next: None,
        });
    }
    Ok(Layered {
        length,
        stream: RefCell::new(Stream {
            base,
            position: 0,
            packs,
        }),
    })
}

/// Random-access parents for journal replay, without store cache writes.
pub(crate) fn export_base(
    store: &Store,
    m: &Medium,
    image: &ResolvedImage,
) -> Result<VerifiedBase> {
    if let Some(disk) = crate::zero::disk(store, m, image)? {
        return Ok(disk.base);
    }
    let d = head(m, image)?;
    if d.decoded_media_type() != mt::DISK_LAYER {
        return export_content(image, &d.decoded_digest()?);
    }
    let expected = identity(
        &crate::medium_head(m)?.store()?,
        &image.manifest.layers,
        store,
    )?;
    let (store, medium, image) = (store.clone(), m.clone(), image.clone());
    verified_stream(
        move || {
            layered(
                &store,
                &crate::medium_head(&medium)?.store()?,
                &medium.geometry,
                &image,
            )
        },
        &expected,
    )
}

pub(crate) fn export_content(image: &ResolvedImage, digest: &Digest) -> Result<VerifiedBase> {
    let d = image
        .manifest
        .layers
        .iter()
        .find(|d| d.names(digest))
        .ok_or_else(|| invalid("missing disk content"))?;
    if !d.is_compressed() {
        return file_base(image, d);
    }
    let (image, content) = (image.clone(), digest.clone());
    verified_stream(move || stream_content(&image, &content), digest)
}

fn stream_content(image: &ResolvedImage, digest: &Digest) -> Result<Layered> {
    let reader = image.content_reader(digest)?;
    Ok(Layered {
        length: reader.size(),
        stream: RefCell::new(Stream {
            base: Some(BufReader::with_capacity(64 * 1024, reader)),
            position: 0,
            packs: Vec::new(),
        }),
    })
}

fn verified_stream(
    reopen: impl Fn() -> Result<Layered> + 'static,
    expected: &Digest,
) -> Result<VerifiedBase> {
    let source = reopen()?;
    if journal_files::layer::copy_and_digest(&source, &mut io::sink())? != expected.to_string() {
        return Err(invalid("reconstructed disk digest mismatch"));
    }
    let identity = journal_files::Header {
        base_length: source.byte_len(),
        base_digest: super::provider::hash_array(expected)?,
        anchor_generation: 0,
    };
    Ok(VerifiedBase::from_verified_source(
        std::rc::Rc::new(Replayable {
            length: source.byte_len(),
            source: RefCell::new(source),
            reopen: Box::new(reopen),
        }),
        identity,
    )?)
}

// Journal anchors revisit parent sectors. Reopen the verified stream on backward
// reads instead of writing a spool; forward reads retain bounded buffering.
struct Replayable {
    length: u64,
    source: RefCell<Layered>,
    reopen: Box<dyn Fn() -> Result<Layered>>,
}
impl SectorSource for Replayable {
    fn byte_len(&self) -> u64 {
        self.length
    }
    fn read_sector(&self, lba: u64) -> io::Result<[u8; SECTOR_LEN]> {
        let mut sector = [0; SECTOR_LEN];
        self.read_range(lba, &mut sector)?;
        Ok(sector)
    }
    fn check_unchanged(&self) -> io::Result<()> {
        self.source.borrow().check_unchanged()
    }
    fn read_range(&self, first_lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        let start = first_lba
            .checked_mul(SECTOR_LEN as u64)
            .filter(|start| {
                start
                    .checked_add(bytes.len() as u64)
                    .is_some_and(|end| end <= self.length)
            })
            .ok_or_else(|| io::Error::other("disk read past its end"))?;
        let mut source = self
            .source
            .try_borrow_mut()
            .map_err(|_| io::Error::other("disk read re-entered"))?;
        if source.stream.borrow().position > start {
            *source = (self.reopen)().map_err(io::Error::other)?;
        }
        let mut discard = [0; 64 * 1024];
        loop {
            let position = source.stream.borrow().position;
            if position == start {
                break;
            }
            let count = (start - position).min(discard.len() as u64) as usize;
            source.read_range(position / SECTOR_LEN as u64, &mut discard[..count])?;
        }
        source.read_range(first_lba, bytes)
    }
}
/// A medium ready to mount. `path` is the verified file assembly opens, or
/// `None` for a compressed floppy, whose bytes live only in `base`.
#[derive(Clone)]
pub(crate) struct Disk {
    pub path: Option<PathBuf>,
    pub identity: Digest,
    pub base: VerifiedBase,
    pub root: Digest,
    /// A zero-rooted head, read through its chain rather than a materialized file.
    pub zero: Option<std::rc::Rc<crate::zero::ZeroChain>>,
}
pub(crate) fn bases(
    store: &Store,
    config: &Config,
    image: &ResolvedImage,
) -> Result<BTreeMap<pc_config::image::Digest, Disk>> {
    let mut bases = BTreeMap::new();
    for medium in &config.media {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            bases.entry(crate::medium_head(medium)?)
        {
            entry.insert(base_path(store, medium, image)?);
        }
    }
    if let Some(n) = &config.nvram {
        bases.insert(n.head, nvram(store, n, image)?);
    }
    Ok(bases)
}
pub(crate) fn nvram(store: &Store, n: &Nvram, image: &ResolvedImage) -> Result<Disk> {
    let d = descriptor(&image.manifest.layers, &n.head.store()?, CMOS)?;
    Ok(Disk {
        path: Some(store.resolved_file_path(image, &d.decoded_digest()?)?),
        identity: d.decoded_digest()?,
        base: file_base(image, &d)?,
        root: d.decoded_digest()?,
        zero: None,
    })
}
/// Validate every medium without writing to the store: a layered head is
/// rebuilt in one streamed pass and compared with its recorded result, and a
/// compressed base or floppy was already proven by resolution's streamed
/// decode. Mounting, which may write decoded files, is [`bases`].
pub(crate) fn check(store: &Store, config: &Config, image: &ResolvedImage) -> Result<()> {
    let mut seen = BTreeSet::new();
    for m in &config.media {
        if !seen.insert(&m.head) {
            continue;
        }
        let d = head(m, image)?;
        let head_digest = crate::medium_head(m)?.store()?;
        if crate::zero::rooted(&head_digest, &m.geometry, &image.manifest.layers, store)? {
            // A zero base has no stored data; a chain over one streams like
            // any layered head, its unwritten sectors read as zeros.
            if d.media_type == mt::DISK_LAYER {
                let identity = identity(&head_digest, &image.manifest.layers, store)?;
                let layered = layered(store, &head_digest, &m.geometry, image)?;
                if journal_files::layer::copy_and_digest(&layered, &mut io::sink())?
                    != identity.to_string()
                {
                    return Err(invalid("reconstructed disk digest mismatch"));
                }
            }
        } else if d.media_type == mt::DISK_LAYER {
            let identity = identity(&head_digest, &image.manifest.layers, store)?;
            let (path, marker) = head_paths(store, &identity, &head_digest);
            if path.is_file() && known_head(&marker) {
                cached_head(&path, m, &identity)?;
            } else {
                let layered = layered(store, &head_digest, &m.geometry, image)?;
                if journal_files::layer::copy_and_digest(&layered, &mut io::sink())?
                    != identity.to_string()
                {
                    return Err(invalid("reconstructed disk digest mismatch"));
                }
            }
        } else if !d.is_compressed() {
            base_path(store, m, image)?;
        }
    }
    if let Some(n) = &config.nvram {
        nvram(store, n, image)?;
    }
    Ok(())
}
fn head<'a>(m: &Medium, image: &'a ResolvedImage) -> Result<&'a Descriptor> {
    let head = crate::medium_head(m)?.store()?;
    image
        .manifest
        .layers
        .iter()
        .find(|d| d.names(&head))
        .ok_or_else(|| invalid("missing disk head"))
}
fn head_paths(store: &Store, identity: &Digest, head: &Digest) -> (PathBuf, PathBuf) {
    let directory = store.root().join("cache/materialized");
    (
        directory.join(identity.hex()),
        directory.join(format!("{}.{}.verified", identity.hex(), head.hex())),
    )
}
const MARKER: &[u8] = b"verified disk head v1\n";
fn known_head(marker: &std::path::Path) -> bool {
    fs::File::open(marker)
        .and_then(|file| {
            let mut bytes = Vec::new();
            file.take(MARKER.len() as u64 + 1).read_to_end(&mut bytes)?;
            Ok(bytes == MARKER)
        })
        .unwrap_or(false)
}
fn cached_head(path: &std::path::Path, m: &Medium, identity: &Digest) -> Result<VerifiedBase> {
    let verified = VerifiedBase::open(path)?;
    if verified.byte_len() != geometry(&m.geometry).capacity()? * 512
        || verified.identity().base_digest != super::provider::hash_array(identity)?
    {
        return Err(invalid("materialized disk damaged"));
    }
    Ok(verified)
}
pub(crate) fn base_path(store: &Store, m: &Medium, image: &ResolvedImage) -> Result<Disk> {
    if let Some(disk) = crate::zero::disk(store, m, image)? {
        return Ok(disk);
    }
    let d = head(m, image)?;
    if d.media_type != mt::DISK_LAYER {
        let digest = d.decoded_digest()?;
        if d.is_compressed() && d.decoded_media_type() == mt::FLOPPY {
            return Ok(Disk {
                path: None,
                identity: digest.clone(),
                base: memory_base(image, d)?,
                root: digest,
                zero: None,
            });
        }
        return Ok(Disk {
            path: Some(store.resolved_file_path(image, &digest)?),
            identity: digest.clone(),
            base: file_base(image, d)?,
            root: digest,
            zero: None,
        });
    }
    let head_digest = crate::medium_head(m)?.store()?;
    let identity = identity(&head_digest, &image.manifest.layers, store)?;
    let directory = store.root().join("cache/materialized");
    fs::create_dir_all(&directory)?;
    fs::File::open(store.root())?.sync_all()?;
    fs::File::open(store.root().join("cache"))?.sync_all()?;
    let (path, marker) = head_paths(store, &identity, &head_digest);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{}.lock", identity.hex())))?;
    lock.lock()?;
    // A result digest may be shared by different heads. Only a head already
    // reconstructed under this codec may skip decoding and applying its packs.
    let known_head = known_head(&marker);
    let verified = if path.exists() && known_head {
        cached_head(&path, m, &identity)?
    } else if path.exists() {
        let layered = layered(store, &head_digest, &m.geometry, image)?;
        let file = fs::File::open(&path)?;
        let metadata = file.metadata()?;
        if metadata.len() != layered.byte_len() {
            return Err(invalid("materialized disk length mismatch"));
        }
        let mut compare = CompareCache(file);
        if journal_files::layer::copy_and_digest(&layered, &mut compare)? != identity.to_string() {
            return Err(invalid("reconstructed disk digest mismatch"));
        }
        VerifiedBase::from_verified_file(
            compare.0,
            journal_files::Header {
                base_length: layered.byte_len(),
                base_digest: super::provider::hash_array(&identity)?,
                anchor_generation: 0,
            },
            metadata,
        )?
    } else {
        let layered = layered(store, &head_digest, &m.geometry, image)?;
        store.require_space(layered.byte_len())?;
        let temporary = directory.join(format!("{}.pending", identity.hex()));
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        let result = (|| -> Result<VerifiedBase> {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut writer = BufWriter::with_capacity(64 * 1024, &file);
            if journal_files::layer::copy_and_digest(&layered, &mut writer)? != identity.to_string()
            {
                return Err(invalid("reconstructed disk digest mismatch"));
            }
            writer.flush()?;
            drop(writer);
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            fs::File::open(&directory)?.sync_all()?;
            let metadata = file.metadata()?;
            Ok(VerifiedBase::from_verified_file(
                fs::File::open(&path)?,
                journal_files::Header {
                    base_length: layered.byte_len(),
                    base_digest: super::provider::hash_array(&identity)?,
                    anchor_generation: 0,
                },
                metadata,
            )?)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?
    };
    if !known_head {
        let temporary = marker.with_extension("pending");
        let result = (|| -> Result<()> {
            if temporary.exists() {
                fs::remove_file(&temporary)?;
            }
            let mut file = fs::File::create_new(&temporary)?;
            file.write_all(MARKER)?;
            file.sync_all()?;
            fs::rename(&temporary, &marker)?;
            fs::File::open(&directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result?;
    }
    let layer = Layer::decode(&store.read_verified(d, image_store::MAX_SMALL_BLOB)?)?;
    Ok(Disk {
        path: Some(path),
        identity,
        base: verified,
        root: layer.base.parse()?,
        zero: None,
    })
}

struct CompareCache(fs::File);
impl Write for CompareCache {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut buffer = [0; 64 * 1024];
        for chunk in bytes.chunks(buffer.len()) {
            self.0.read_exact(&mut buffer[..chunk.len()])?;
            if chunk != &buffer[..chunk.len()] {
                return Err(io::Error::other("cache differs from reconstructed head"));
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
