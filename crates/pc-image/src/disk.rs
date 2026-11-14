// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fixed-disk closure traversal and shared raw journal bases; machine-images §4.
//! Journals keep their original raw-byte identity instead of naming descriptors.

use crate::*;
use image_store::{media_type as mt, BlobSource, ResolvedImage, Store};
use media::{
    layer::{Chain, Descriptor as Layer},
    SectorSource, VerifiedBase,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufWriter, Read, Write},
    path::PathBuf,
};

pub(crate) fn geometry(g: &Geometry) -> media::layer::Geometry {
    media::layer::Geometry {
        cylinders: g.cylinders,
        heads: g.heads,
        sectors: g.sectors,
        sector_bytes: g.sector_bytes,
    }
}
struct Graph {
    root: Descriptor,
    layers: Vec<(Digest, Layer, Descriptor)>,
    refs: Vec<Digest>,
}
fn graph(
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
            .find(|d| d.digest == next)
            .ok_or_else(|| invalid("missing disk parent"))?;
        refs.push(next.clone());
        match d.media_type.as_str() {
            mt::DISK_BASE => break d.clone(),
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
                if Some(pack.size) != layer.sector_count.checked_mul(media::layer::RECORD_LEN) {
                    return Err(invalid("sector pack size mismatch"));
                }
                refs.push(pack.digest.clone());
                next = layer.parent.parse()?;
                layers.push((d.digest.clone(), layer, pack));
            }
            _ => return Err(invalid("invalid fixed-disk parent type")),
        }
    };
    if geometry(g).capacity()?.checked_mul(512) != Some(root.size) {
        return Err(invalid("root geometry mismatch"));
    }
    if layers
        .iter()
        .any(|(_, layer, _)| layer.base != root.digest.to_string())
    {
        return Err(invalid("layer root mismatch"));
    }
    Ok(Graph { root, layers, refs })
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
        .find(|d| &d.digest == head)
        .ok_or_else(|| invalid("missing disk head"))?;
    if d.media_type == mt::DISK_LAYER {
        Ok(
            Layer::decode(&blobs.read_verified(d, image_store::MAX_SMALL_BLOB)?)?
                .result_sha256
                .parse()?,
        )
    } else {
        Ok(head.clone())
    }
}
fn base(image: &ResolvedImage, d: &Descriptor) -> Result<VerifiedBase> {
    let (file, metadata) = image.verified_file(&d.digest)?;
    Ok(VerifiedBase::from_verified_file(
        file,
        media::Header {
            base_length: d.size,
            base_digest: super::provider::hash_array(&d.digest)?,
            anchor_generation: 0,
        },
        metadata,
    )?)
}
pub(crate) fn chain(
    store: &Store,
    head: &Digest,
    g: &Geometry,
    image: &ResolvedImage,
) -> Result<Chain> {
    let graph = graph(head, g, &image.manifest.layers, store)?;
    let mut chain = Chain::new(base(image, &graph.root)?, geometry(g))?;
    for (id, layer, pack) in graph.layers.into_iter().rev() {
        let (file, metadata) = image.verified_file(&pack.digest)?;
        chain.apply_verified(&id.to_string(), &layer, file, metadata)?;
    }
    Ok(chain)
}
#[derive(Clone)]
pub(crate) struct Disk {
    pub path: PathBuf,
    pub identity: Digest,
    pub base: VerifiedBase,
    pub root: Digest,
}
pub(crate) fn bases(
    store: &Store,
    config: &Config,
    image: &ResolvedImage,
) -> Result<BTreeMap<Digest, Disk>> {
    let mut bases = BTreeMap::new();
    for medium in &config.media {
        if !bases.contains_key(&medium.head) {
            bases.insert(medium.head.clone(), base_path(store, medium, image)?);
        }
    }
    if let Some(n) = &config.nvram {
        let d = descriptor(&image.manifest.layers, &n.head, CMOS)?;
        bases.insert(
            n.head.clone(),
            Disk {
                path: store.blob_file_path(&d.digest, d.size)?,
                identity: d.digest.clone(),
                base: base(image, &d)?,
                root: d.digest.clone(),
            },
        );
    }
    Ok(bases)
}
pub(crate) fn base_path(store: &Store, m: &Medium, image: &ResolvedImage) -> Result<Disk> {
    let d = image
        .manifest
        .layers
        .iter()
        .find(|d| d.digest == m.head)
        .ok_or_else(|| invalid("missing disk head"))?;
    if d.media_type != mt::DISK_LAYER {
        return Ok(Disk {
            path: store.blob_file_path(&d.digest, d.size)?,
            identity: d.digest.clone(),
            base: base(image, d)?,
            root: d.digest.clone(),
        });
    }
    let identity = identity(&m.head, &image.manifest.layers, store)?;
    let directory = store.root().join("cache/materialized");
    fs::create_dir_all(&directory)?;
    fs::File::open(store.root())?.sync_all()?;
    fs::File::open(store.root().join("cache"))?.sync_all()?;
    let path = directory.join(identity.hex());
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{}.lock", identity.hex())))?;
    lock.lock()?;
    // A result digest may be shared by different heads. Only a head already
    // reconstructed under this codec may skip decoding and applying its packs.
    let marker = directory.join(format!("{}.{}.verified", identity.hex(), m.head.hex()));
    let marker_bytes = b"verified disk head v1\n";
    let known_head = fs::File::open(&marker)
        .and_then(|file| {
            let mut bytes = Vec::new();
            file.take(marker_bytes.len() as u64 + 1)
                .read_to_end(&mut bytes)?;
            Ok(bytes == marker_bytes)
        })
        .unwrap_or(false);
    let verified = if path.exists() && known_head {
        let verified = VerifiedBase::open(&path)?;
        if verified.byte_len() != geometry(&m.geometry).capacity()? * 512
            || verified.identity().base_digest != super::provider::hash_array(&identity)?
        {
            return Err(invalid("materialized disk damaged"));
        }
        verified
    } else if path.exists() {
        let chain = chain(store, &m.head, &m.geometry, image)?;
        let file = fs::File::open(&path)?;
        let metadata = file.metadata()?;
        if metadata.len() != chain.byte_len() {
            return Err(invalid("materialized disk length mismatch"));
        }
        let mut compare = CompareCache(file);
        if media::layer::copy_and_digest(&chain, &mut compare)? != identity.to_string() {
            return Err(invalid("reconstructed disk digest mismatch"));
        }
        VerifiedBase::from_verified_file(
            compare.0,
            media::Header {
                base_length: chain.byte_len(),
                base_digest: super::provider::hash_array(&identity)?,
                anchor_generation: 0,
            },
            metadata,
        )?
    } else {
        let chain = chain(store, &m.head, &m.geometry, image)?;
        store.require_space(chain.byte_len())?;
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
            if media::layer::copy_and_digest(&chain, &mut writer)? != identity.to_string() {
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
                media::Header {
                    base_length: chain.byte_len(),
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
            file.write_all(marker_bytes)?;
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
        path,
        identity,
        base: verified,
        root: layer.base.parse()?,
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
