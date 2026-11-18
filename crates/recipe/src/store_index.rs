// SPDX-License-Identifier: AGPL-3.0-or-later
//! The store's index of decoded digests (recipes §3.4, source 1): which
//! content the store already holds, by the SHA-256 and size of its decoded
//! bytes. A compressed layer is stored under the digest of its frame and
//! names its decoded content in the `unemu.decoded.digest` and
//! `unemu.decoded.size` annotations of its descriptor; a raw layer's digest
//! is its content's. So the index is read from the layer descriptors of the
//! tagged manifests, without opening a payload.
//!
//! The index is a rebuildable cache, `cache/decoded/index.json` in the
//! store: per manifest digest, its layers' decoded digests and sizes
//! (`{"schema": 1, "manifests": {"sha256:…": [{"digest", "size"}]}}`). A
//! manifest is read once, as manifests never change; one no longer tagged
//! leaves the cache when it is next written.

use image_store::{BlobSource, Manifest, Store, MAX_SMALL_BLOB};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

/// The store's cache file of decoded digests.
pub const CACHE_FILE: &str = "cache/decoded/index.json";

const SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Layer {
    digest: String,
    size: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheFile {
    schema: u32,
    manifests: BTreeMap<String, Vec<Layer>>,
}

/// Content the store holds, by decoded SHA-256 (lower-case hex).
#[derive(Clone, Debug, Default)]
pub struct StoreIndex {
    /// The store's root.
    pub root: PathBuf,
    content: BTreeMap<String, (u64, Vec<String>)>,
    /// Manifests read from blobs in this run rather than from the cache.
    pub read: usize,
    /// What the index could not read or keep: a tag whose manifest cannot
    /// be read or decoded is left out, and a cache that cannot be written
    /// is read again next time. Neither stops a plan.
    pub warnings: Vec<String>,
}

impl StoreIndex {
    /// Read the index of `store`, reading the manifests the cache lacks and
    /// writing the cache back when it changed. Only a store whose tags
    /// cannot be listed is an error; see [`Self::warnings`].
    pub fn load(store: &Store) -> io::Result<Self> {
        let error = |e: image_store::Error| io::Error::other(e.to_string());
        let path = store.root().join(CACHE_FILE);
        let mut cache: CacheFile = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|c| c.schema == SCHEMA)
            .unwrap_or_default();
        let mut tags = Vec::new();
        let mut after = None;
        loop {
            let page = store.list_tags(after.as_deref(), 1000).map_err(error)?;
            tags.extend(page.tags);
            match page.next_after {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        let mut index = Self {
            root: store.root().to_path_buf(),
            ..Self::default()
        };
        let mut manifests = BTreeMap::new();
        for tag in &tags {
            let key = tag.manifest.digest.to_string();
            let layers = match cache
                .manifests
                .remove(&key)
                .or_else(|| manifests.get(&key).cloned())
            {
                Some(layers) => layers,
                None => {
                    let manifest = store
                        .read_verified(&tag.manifest, MAX_SMALL_BLOB)
                        .and_then(|bytes| Manifest::decode_for_summary(&bytes));
                    let manifest = match manifest {
                        Ok(manifest) => manifest,
                        Err(e) => {
                            index.warnings.push(format!(
                                "the image of {} is left out: its manifest cannot be read: {e}",
                                tag.name
                            ));
                            continue;
                        }
                    };
                    index.read += 1;
                    manifest
                        .layers
                        .iter()
                        .filter_map(|layer| {
                            Some(Layer {
                                digest: layer.decoded_digest().ok()?.to_string(),
                                size: layer.decoded_size().ok()?,
                            })
                        })
                        .collect()
                }
            };
            for layer in &layers {
                let hex = layer
                    .digest
                    .strip_prefix("sha256:")
                    .unwrap_or(&layer.digest)
                    .to_owned();
                let entry = index
                    .content
                    .entry(hex)
                    .or_insert_with(|| (layer.size, Vec::new()));
                if !entry.1.contains(&tag.name) {
                    entry.1.push(tag.name.clone());
                }
            }
            manifests.insert(key, layers);
        }
        let changed = index.read > 0 || !cache.manifests.is_empty();
        if changed {
            let bytes = serde_json::to_vec(&CacheFile {
                schema: SCHEMA,
                manifests,
            })
            .map_err(io::Error::other)?;
            if let Err(e) = crate::library::write_cache(&path, &bytes) {
                index.warnings.push(format!(
                    "the index of the store's content was not kept in {}: {e}",
                    path.display()
                ));
            }
        }
        Ok(index)
    }

    /// The tags whose images hold content of this SHA-256 and size.
    pub fn find(&self, size: u64, sha256: &str) -> Option<&[String]> {
        self.content
            .get(sha256)
            .filter(|(s, _)| *s == size)
            .map(|(_, tags)| tags.as_slice())
    }

    /// An index of the given content, for tests.
    #[cfg(test)]
    pub(crate) fn of(content: &[(&str, u64, &str)]) -> Self {
        Self {
            root: PathBuf::new(),
            content: content
                .iter()
                .map(|(sha, size, tag)| ((*sha).to_owned(), (*size, vec![(*tag).to_owned()])))
                .collect(),
            read: 0,
            warnings: Vec::new(),
        }
    }
}
