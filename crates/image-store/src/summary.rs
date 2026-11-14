// SPDX-License-Identifier: AGPL-3.0-or-later
//! Rebuildable per-title facets and token search; machine-images §§2.1 and 7.
//! Startup reads only the OCI index, local library and cache. Immutable summaries
//! bind image/metadata digests; local content hashes invalidate affected images.

use crate::{
    filesystem as host, invalid,
    library::{Library, LibraryEntry},
    media_type,
    metadata::{Metadata, Title},
    store::read_bounded,
    BlobSource, Digest, Manifest, PayloadType, Result, Store, Tag, MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
};

/// Same-title facet predicates; values within a field and across fields are ANDed.
#[derive(Clone, Debug, Default)]
pub struct Facets {
    pub genres: Vec<String>,
    pub platforms: Vec<String>,
    pub years: Vec<i64>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub series: Vec<String>,
    pub labels: Vec<String>,
}

/// A compact local projection. Tokens include every textual title/personal field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitleSummary {
    pub id: String,
    pub title: String,
    pub genres: Vec<String>,
    pub platform: Option<String>,
    pub year: Option<i64>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub series: Option<String>,
    pub tokens: Vec<String>,
}

/// Immutable identities plus revisioned local data. Never used for Start hashing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSummary {
    pub image: Digest,
    pub metadata: Digest,
    pub metadata_schema: u32,
    pub local_content: Digest,
    pub warnings: Vec<String>,
    pub search_limited: bool,
    pub machine: Digest,
    pub platform: String,
    pub name: String,
    pub stored_size: u64,
    pub labels: Vec<String>,
    pub favorite: bool,
    pub last_run: Option<String>,
    pub overlay_revision: u64,
    pub personal_revision: u64,
    pub local_revision: u64,
    pub titles: Vec<TitleSummary>,
    pub tokens: Vec<String>,
}

impl ImageSummary {
    /// Catalogue fields from newer schemas may be displayed but never edited.
    pub fn metadata_is_read_only(&self) -> bool {
        self.metadata_schema != 1
    }
}

/// Matching titles are grouped beneath one local repository:tag reference.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub reference: String,
    pub image: Digest,
    pub title_ids: Vec<String>,
}

/// A terminal summary failure. Repairing a blob can be followed by `retry`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryFailure {
    #[serde(skip, default = "empty_digest")]
    pub image: Digest,
    pub local_content: Digest,
    pub reason: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheFile {
    schema: u32,
    images: BTreeMap<Digest, ImageSummary>,
    failures: BTreeMap<Digest, SummaryFailure>,
    warnings: Vec<String>,
}
impl Default for CacheFile {
    fn default() -> Self {
        Self {
            schema: 2,
            images: BTreeMap::new(),
            failures: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }
}

fn empty_digest() -> Digest {
    Digest::of(b"")
}

fn read_cache(store: &Store) -> Result<CacheFile> {
    match read_bounded(
        &store.root().join("cache/summaries/index.json"),
        64 * MAX_SMALL_BLOB,
    ) {
        Ok(bytes) => {
            if let Ok(mut cache) = serde_json::from_slice::<CacheFile>(&bytes) {
                for (id, failure) in &mut cache.failures {
                    failure.image = id.clone();
                }
                if cache.schema == 2
                    && cache.images.iter().all(|(id, row)| id == &row.image)
                    && cache.failures.iter().all(|(id, row)| id == &row.image)
                {
                    return Ok(cache);
                }
            }
            Ok(CacheFile::default())
        }
        Err(crate::Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(CacheFile::default()),
        Err(crate::Error::Invalid(_)) => Ok(CacheFile::default()),
        Err(error) => Err(error),
    }
}

/// Index snapshot with independent ready, damaged and pending rows. Rebuilds are
/// driven by the caller, so storage work never runs in the emulator's execution path.
pub struct SummaryCache {
    tags: Vec<Tag>,
    library: Library,
    local_hashes: BTreeMap<Digest, Digest>,
    empty_hash: Digest,
    library_error: Option<String>,
    images: BTreeMap<Digest, ImageSummary>,
    failures: BTreeMap<Digest, SummaryFailure>,
    warnings: Vec<String>,
    dirty: BTreeSet<Digest>,
}

impl SummaryCache {
    /// Startup reads only the index and local files. A damaged local library is
    /// preserved; listing continues without local data and cache writes are disabled.
    pub fn open(store: &Store) -> Result<Self> {
        let _lease = store.lease()?;
        let cache = read_cache(store)?;
        let mut result = Self {
            tags: Vec::new(),
            library: Library::default(),
            local_hashes: BTreeMap::new(),
            empty_hash: LibraryEntry::default().content_digest()?,
            library_error: None,
            images: cache.images,
            failures: cache.failures,
            warnings: cache.warnings,
            dirty: BTreeSet::new(),
        };
        result.refresh(store)?;
        Ok(result)
    }

    pub fn refresh(&mut self, store: &Store) -> Result<()> {
        self.tags = store.all_tags()?;
        self.set_library(store.read_library())
    }

    fn set_library(&mut self, library: Result<Library>) -> Result<()> {
        match library {
            Ok(library) => {
                self.library = library;
                self.library_error = None;
            }
            Err(error) => {
                self.library = Library::default();
                self.library_error = Some(error.to_string());
            }
        }
        self.local_hashes = self
            .library
            .images
            .iter()
            .map(|(id, entry)| Ok((id.clone(), entry.content_digest()?)))
            .collect::<Result<_>>()?;
        Ok(())
    }

    fn local_hash(&self, image: &Digest) -> &Digest {
        self.local_hashes.get(image).unwrap_or(&self.empty_hash)
    }
    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }
    pub fn library_error(&self) -> Option<&str> {
        self.library_error.as_deref()
    }
    pub fn is_read_only(&self) -> bool {
        self.library_error.is_some()
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn summary(&self, image: &Digest) -> Option<&ImageSummary> {
        self.images
            .get(image)
            .filter(|row| &row.local_content == self.local_hash(image))
    }
    pub fn failure(&self, image: &Digest) -> Option<&SummaryFailure> {
        self.failures
            .get(image)
            .filter(|row| &row.local_content == self.local_hash(image))
    }
    pub fn retry(&mut self, image: &Digest) {
        self.failures.remove(image);
        self.images.remove(image);
        self.dirty.insert(image.clone());
    }
    pub fn pending(&self) -> Vec<&Tag> {
        self.tags
            .iter()
            .filter(|tag| {
                self.summary(&tag.manifest.digest).is_none()
                    && self.failure(&tag.manifest.digest).is_none()
            })
            .collect()
    }

    /// Attempt at most 64 distinct images, prioritizing visible tags. Per-image
    /// failures become rows with reasons, so a damaged tag cannot starve later tags.
    /// Provider payload types are intentionally opaque for display, not execution.
    pub fn rebuild(
        &mut self,
        store: &Store,
        limit: usize,
        priority: &[&str],
        _types: &[PayloadType],
    ) -> Result<usize> {
        let _lease = store.lease()?;
        self.refresh(store)?;
        let mut pending: Vec<_> = self.pending().into_iter().cloned().collect();
        pending.sort_by_key(|tag| {
            priority
                .iter()
                .position(|name| *name == tag.name)
                .unwrap_or(usize::MAX)
        });
        let mut seen = BTreeSet::new();
        let pending: Vec<_> = pending
            .into_iter()
            .filter(|tag| seen.insert(tag.manifest.digest.clone()))
            .take(limit.min(64))
            .collect();
        for tag in &pending {
            let id = tag.manifest.digest.clone();
            match self.build(store, tag) {
                Ok(row) => {
                    self.failures.remove(&id);
                    self.images.insert(id.clone(), row);
                }
                Err(error) => {
                    self.images.remove(&id);
                    self.failures.insert(
                        id.clone(),
                        SummaryFailure {
                            image: id.clone(),
                            local_content: self.local_hash(&id).clone(),
                            reason: error.to_string().chars().take(512).collect(),
                        },
                    );
                }
            }
            self.dirty.insert(id);
        }
        if !pending.is_empty() {
            self.save(store)?;
        }
        Ok(pending.len())
    }

    fn build(&self, store: &Store, tag: &Tag) -> Result<ImageSummary> {
        let manifest =
            Manifest::decode_for_summary(&store.read_verified(&tag.manifest, MAX_SMALL_BLOB)?)?;
        let descriptor = manifest
            .layers
            .iter()
            .find(|d| d.media_type == media_type::METADATA)
            .ok_or_else(|| invalid("missing metadata"))?;
        let metadata = Metadata::decode(
            &store.read_verified(descriptor, MAX_SMALL_BLOB)?,
            &manifest.layers,
        )?;
        if metadata.platform != manifest.platform()? {
            return Err(invalid("summary platform mismatch"));
        }
        let entry = self
            .library
            .images
            .get(&tag.manifest.digest)
            .cloned()
            .unwrap_or_default();
        let metadata = if metadata.is_read_only() {
            metadata
        } else {
            entry.catalogue.apply(&metadata)?
        };
        let metadata_schema = metadata.schema();
        let warnings = metadata.warnings().to_vec();
        let mut labels = metadata.labels;
        labels.extend(entry.labels.clone());
        labels.sort();
        labels.dedup();
        let name = manifest
            .annotations
            .get("org.opencontainers.image.title")
            .cloned()
            .unwrap_or_default();
        let titles = metadata
            .titles
            .iter()
            .map(|title| title_summary(title, &entry))
            .collect::<Result<Vec<_>>>()?;
        let stored_size = manifest
            .layers
            .iter()
            .chain(std::iter::once(&manifest.config))
            .try_fold(tag.manifest.size, |sum, d| {
                sum.checked_add(d.size)
                    .ok_or_else(|| invalid("summary size overflow"))
            })?;
        let tokens = tokenize(&format!(
            "{} {} {}",
            name,
            metadata.platform,
            labels.join(" ")
        ));
        Ok(ImageSummary {
            image: tag.manifest.digest.clone(),
            metadata: descriptor.digest.clone(),
            metadata_schema,
            local_content: self.local_hash(&tag.manifest.digest).clone(),
            warnings,
            search_limited: false,
            machine: manifest.config.digest,
            platform: metadata.platform,
            name,
            stored_size,
            labels,
            favorite: entry.favorite,
            last_run: entry.last_run,
            overlay_revision: entry.overlay_revision,
            personal_revision: entry.personal_revision,
            local_revision: entry.local_revision,
            titles,
            tokens,
        })
    }

    fn save(&mut self, store: &Store) -> Result<()> {
        if self.is_read_only() {
            return Ok(());
        }
        let _lease = store.lease()?;
        let _lock = host::exclusive_lock(&store.root().join("locks/index.lock"))?;
        self.set_library(store.read_library_inner())?;
        if self.is_read_only() {
            return Ok(());
        }
        self.tags = store.tags_inner()?;
        let mut cache = read_cache(store)?;
        for id in &self.dirty {
            cache.images.remove(id);
            cache.failures.remove(id);
        }
        for (id, row) in &self.images {
            if self.dirty.contains(id)
                || (!cache.images.contains_key(id) && !cache.failures.contains_key(id))
            {
                cache.failures.remove(id);
                cache.images.insert(id.clone(), row.clone());
            }
        }
        for (id, row) in &self.failures {
            if self.dirty.contains(id)
                || (!cache.images.contains_key(id) && !cache.failures.contains_key(id))
            {
                cache.images.remove(id);
                cache.failures.insert(id.clone(), row.clone());
            }
        }
        let reachable: BTreeSet<_> = self.tags.iter().map(|t| &t.manifest.digest).collect();
        cache
            .images
            .retain(|id, row| reachable.contains(id) && &row.local_content == self.local_hash(id));
        cache
            .failures
            .retain(|id, row| reachable.contains(id) && &row.local_content == self.local_hash(id));
        let bytes = bounded_cache(&mut cache, 64 * MAX_SMALL_BLOB as usize)?;
        fs::create_dir_all(store.root().join("cache/summaries"))?;
        host::sync_dir(&store.root().join("cache"))?;
        host::sync_dir(store.root())?;
        store.atomic_write(&store.root().join("cache/summaries/index.json"), &bytes)?;
        self.images = cache.images;
        self.failures = cache.failures;
        self.warnings = cache.warnings;
        self.dirty.clear();
        Ok(())
    }

    /// Incremental prefix matching uses summaries only; stale rows remain pending.
    pub fn query(&self, text: &str, facets: &Facets) -> Vec<SearchResult> {
        let terms = tokenize(text);
        self.tags
            .iter()
            .filter_map(|tag| {
                let image = self.summary(&tag.manifest.digest)?;
                if !all_in(&facets.labels, &image.labels) {
                    return None;
                }
                let tag_tokens = tokenize(&tag.name);
                let matches_text = |tokens: &[String]| {
                    terms.iter().all(|term| {
                        tokens
                            .iter()
                            .chain(&image.tokens)
                            .chain(&tag_tokens)
                            .any(|token| token.starts_with(term))
                    })
                };
                let title_ids: Vec<_> = image
                    .titles
                    .iter()
                    .filter(|t| {
                        all_in(&facets.genres, &t.genres)
                            && all_in(&facets.developers, &t.developers)
                            && all_in(&facets.publishers, &t.publishers)
                            && facets
                                .platforms
                                .iter()
                                .all(|p| t.platform.as_ref() == Some(p))
                            && facets.years.iter().all(|y| t.year.as_ref() == Some(y))
                            && facets.series.iter().all(|s| t.series.as_ref() == Some(s))
                            && matches_text(&t.tokens)
                    })
                    .map(|t| t.id.clone())
                    .collect();
                let no_title_facets = facets.genres.is_empty()
                    && facets.developers.is_empty()
                    && facets.publishers.is_empty()
                    && facets.platforms.is_empty()
                    && facets.years.is_empty()
                    && facets.series.is_empty();
                if title_ids.is_empty()
                    && !(image.titles.is_empty() && no_title_facets && matches_text(&[]))
                {
                    return None;
                }
                Some(SearchResult {
                    reference: tag.name.clone(),
                    image: image.image.clone(),
                    title_ids,
                })
            })
            .collect()
    }
}

fn all_in(wanted: &[String], actual: &[String]) -> bool {
    wanted.iter().all(|value| actual.contains(value))
}

fn title_summary(title: &Title, entry: &LibraryEntry) -> Result<TitleSummary> {
    let mut text = String::new();
    let mut value = serde_json::to_value(title).map_err(|e| invalid(e.to_string()))?;
    if let Some(fields) = value.as_object_mut() {
        fields.remove("cover");
        fields.remove("screenshots");
    }
    collect_text(&value, &mut text);
    if let Some(notes) = entry
        .personal
        .get(&title.id)
        .and_then(|p| p.notes.as_deref())
    {
        text.push_str(notes);
        text.push(' ');
    }
    Ok(TitleSummary {
        id: title.id.clone(),
        title: title.title.clone(),
        genres: title.genres.clone(),
        platform: title.platform.clone(),
        year: title.release_year,
        developers: title.developers.clone(),
        publishers: title.publishers.clone(),
        series: title.series.clone(),
        tokens: tokenize(&text),
    })
}

fn collect_text(value: &serde_json::Value, text: &mut String) {
    match value {
        serde_json::Value::String(s) => {
            text.push_str(s);
            text.push(' ');
        }
        serde_json::Value::Number(n) => {
            text.push_str(&n.to_string());
            text.push(' ');
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_text(value, text);
            }
        }
        serde_json::Value::Object(fields) => {
            for value in fields.values() {
                collect_text(value, text);
            }
        }
        _ => (),
    }
}

fn encoded<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| invalid(e.to_string()))
}

fn bounded_cache(cache: &mut CacheFile, limit: usize) -> Result<Vec<u8>> {
    let bytes = encoded(cache)?;
    if bytes.len() <= limit {
        return Ok(bytes);
    }
    cache.warnings =
        vec!["cache capacity: search text reduced; inspect row warnings/status".into()];
    let mut size = encoded(cache)?.len();
    let mut largest = cache
        .images
        .iter()
        .map(|(id, row)| Ok((encoded(row)?.len(), id.clone())))
        .collect::<Result<Vec<_>>>()?;
    largest.sort_by(|a, b| b.cmp(a));
    for (_, id) in &largest {
        if size <= limit {
            break;
        }
        if let Some(row) = cache.images.get_mut(id) {
            let before = encoded(row)?.len();
            row.tokens.clear();
            for title in &mut row.titles {
                title.tokens.clear();
            }
            row.search_limited = true;
            size = size
                .saturating_sub(before)
                .saturating_add(encoded(row)?.len());
        }
    }
    if size > limit {
        // Facets can themselves exceed the cap; retain a compact terminal status
        // instead of repeatedly trying to serialize an unbounded row.
        for (_, id) in &largest {
            if size <= limit {
                break;
            }
            if let Some(row) = cache.images.remove(id) {
                let before = encoded(&row)?.len();
                let failure = SummaryFailure {
                    image: id.clone(),
                    local_content: row.local_content,
                    reason: "summary omitted: cache capacity".into(),
                };
                size = size
                    .saturating_sub(before)
                    .saturating_add(encoded(&failure)?.len());
                cache.failures.insert(id.clone(), failure);
            }
        }
    }
    let bytes = encoded(cache)?;
    if bytes.len() <= limit {
        return Ok(bytes);
    }
    for failure in cache.failures.values_mut() {
        failure.reason = "capacity".into();
    }
    // Compact status rows are smaller than their required OCI index descriptors;
    // the bounded index therefore bounds this fallback under the production cap.
    let bytes = encoded(cache)?;
    if bytes.len() > limit {
        return Err(invalid("cache limit too small for compact status rows"));
    }
    Ok(bytes)
}

fn cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
}

/// Std has no normalization API. Use lowercase plus a small Latin diacritic
/// table and discard combining accents; CJK gets character and bigram tokens.
fn tokenize(text: &str) -> Vec<String> {
    let mut folded = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        let replacement = match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'é' | 'è' | 'ê' | 'ë' | 'ě' | 'ę' | 'ē' => "e",
            'í' | 'ì' | 'î' | 'ï' | 'ī' => "i",
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' => "o",
            'ú' | 'ù' | 'û' | 'ü' | 'ū' => "u",
            'ç' | 'ć' | 'č' => "c",
            'ñ' | 'ń' => "n",
            'ł' => "l",
            'š' | 'ś' => "s",
            'ž' | 'ź' | 'ż' => "z",
            'ý' | 'ÿ' => "y",
            'ř' => "r",
            'ď' | 'đ' => "d",
            'ť' => "t",
            'ğ' => "g",
            'æ' => "ae",
            'œ' => "oe",
            'ß' => "ss",
            '\u{0300}'..='\u{036f}' => "",
            _ => {
                folded.push(c);
                continue;
            }
        };
        folded.push_str(replacement);
    }
    let mut tokens = BTreeSet::new();
    for word in folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
    {
        tokens.insert(word.to_owned());
        let mut previous = None;
        for c in word.chars() {
            if cjk(c) {
                tokens.insert(c.to_string());
                if let Some(p) = previous {
                    tokens.insert(format!("{p}{c}"));
                }
                previous = Some(c);
            } else {
                previous = None;
            }
        }
    }
    tokens.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_capacity_keeps_facets_and_reports_search_reduction() {
        let mut cache = CacheFile::default();
        let id = Digest::of(b"large");
        cache.images.insert(
            id.clone(),
            ImageSummary {
                image: id.clone(),
                metadata: id.clone(),
                machine: id.clone(),
                local_content: id.clone(),
                metadata_schema: 1,
                warnings: vec![],
                search_limited: false,
                platform: "c64".into(),
                name: "Large".into(),
                stored_size: 1,
                labels: vec!["facet".into()],
                favorite: false,
                last_run: None,
                overlay_revision: 0,
                personal_revision: 0,
                local_revision: 0,
                titles: vec![],
                tokens: vec!["x".repeat(100_000)],
            },
        );
        let bytes = bounded_cache(&mut cache, 4096).unwrap();
        assert!(bytes.len() <= 4096);
        assert!(!cache.warnings.is_empty());
        assert!(cache.images[&id].search_limited);
        assert_eq!(cache.images[&id].labels, ["facet"]);
        cache.images.get_mut(&id).unwrap().name = "y".repeat(100_000);
        assert!(bounded_cache(&mut cache, 4096).unwrap().len() <= 4096);
        assert!(cache.failures[&id].reason.contains("capacity"));
        for i in 0_u32..20 {
            let image = Digest::of(&i.to_le_bytes());
            cache.failures.insert(
                image.clone(),
                SummaryFailure {
                    image,
                    local_content: id.clone(),
                    reason: "z".repeat(10_000),
                },
            );
        }
        assert!(bounded_cache(&mut cache, 8192).unwrap().len() <= 8192);
        assert!(cache.failures.values().all(|row| row.reason == "capacity"));
    }
}
