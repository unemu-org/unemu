// SPDX-License-Identifier: AGPL-3.0-or-later
//! Libraries and their index (recipes §3.4): folders the user names, where
//! the inputs of a recipe are found by size and SHA-256 whatever they are
//! called.
//!
//! A scan ([`Library::scan`]) lists every file below the folder, symbolic
//! links followed, with its size and change stamp, reads the central
//! directory of each zip archive and the rows of each catalogue, and hashes
//! nothing. A file is hashed only when a recipe wants a file of its size,
//! and then only until one candidate matches ([`Library::find`]); the hash
//! is kept in the index under the file's stamp, so a later plan does not
//! hash the file again to find it. A build hashes it again when it uses it.
//!
//! **The stamp** is what the index trusts a hash under: size, modification
//! time and, on Unix, the inode, the device and the change time (`ctime`),
//! which the system sets on every write and which no ordinary program can
//! set back. An entry whose stamp no longer matches the file is dropped and
//! the file is hashed again when it is wanted, never trusted. On Unix a
//! rewrite that keeps the size and restores the modification time is still
//! seen, through the change time; elsewhere the stamp is size and
//! modification time only, and such a rewrite is not seen until the file is
//! used.
//!
//! **Catalogues** in a library are hints, never proof: a CSV with a header
//! naming `path`, `size` and `sha256` (and optionally `crc32`, `sha1`,
//! `md5`), a `SHA256SUMS` file or a `*.sha256` file (`<hex>  <path>`). A row
//! moves the file it names to the front of the candidates for a value with
//! that hash, and the file is hashed all the same; a row that leads to
//! other bytes is reported as stale.
//!
//! **The index cache** is one JSON file per library in the store's
//! `cache/libraries/` folder (beside the store's other rebuildable caches),
//! named by the SHA-256 of the library's absolute path: `{"schema": 1,
//! "library": PATH, "files": [{"path", "stamp", "sha256", "zip":
//! {"members": [...]} or {"error": TEXT}}]}`, each member with its name and
//! raw name bytes, sizes, CRC-32, method, offset, any problem that keeps it
//! from the search, and its SHA-256 or why hashing it failed. Deleting it
//! loses nothing but the hashes.
//!
//! Nothing is skipped silently: a folder or entry the walk cannot read is
//! in [`Stats::unreadable`] with its relative path and the error, and the
//! scan goes on.

use crate::zip;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The folder of the store's cache that holds the library indexes.
pub const CACHE_FOLDER: &str = "cache/libraries";

const SCHEMA: u32 = 1;

/// The largest catalogue read; a larger file is not taken as one.
const MAX_CATALOGUE: u64 = 16 * 1024 * 1024;

/// What the index trusts a hash under: the file is unchanged as long as its
/// stamp is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub size: u64,
    pub modified_s: i64,
    pub modified_ns: u32,
    /// Unix only: the change time, the inode and the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_s: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_ns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
}

impl Stamp {
    pub fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                size: metadata.len(),
                modified_s: metadata.mtime(),
                modified_ns: metadata.mtime_nsec() as u32,
                changed_s: Some(metadata.ctime()),
                changed_ns: Some(metadata.ctime_nsec() as u32),
                inode: Some(metadata.ino()),
                device: Some(metadata.dev()),
            }
        }
        #[cfg(not(unix))]
        {
            let (modified_s, modified_ns) = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or((0, 0), |d| (d.as_secs() as i64, d.subsec_nanos()));
            Self {
                size: metadata.len(),
                modified_s,
                modified_ns,
                changed_s: None,
                changed_ns: None,
                inode: None,
                device: None,
            }
        }
    }
}

/// The zip directory of an archive, or why it could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum Listing {
    Members(Vec<IndexedMember>),
    Error(String),
}

/// A member of a zip archive and, once computed, its SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedMember {
    #[serde(flatten)]
    pub member: zip::Member,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Why hashing it failed, kept under the archive's stamp so that the
    /// next plan does not try again until the archive changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One file of a library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedFile {
    /// Relative to the library, `/` separators.
    pub path: String,
    pub stamp: Stamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip: Option<Listing>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheFile {
    schema: u32,
    library: String,
    files: Vec<IndexedFile>,
}

/// A row of a catalogue: what it claims of one file of the library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Claim {
    /// The catalogue it came from, relative to the library.
    pub catalogue: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub crc32: Option<String>,
    pub sha1: Option<String>,
    pub md5: Option<String>,
}

/// Where a value lies in a library: a file, or a member of a zip archive.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    pub path: String,
    pub member: Option<String>,
}

impl std::fmt::Display for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.member {
            Some(member) => write!(f, "{}!{member}", self.path),
            None => f.write_str(&self.path),
        }
    }
}

/// What a value is searched for by, besides its size and SHA-256.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// The canonical name first, then the other names.
    pub names: Vec<String>,
    /// The path a file set puts the file at; a file whose path ends in it is
    /// tried first.
    pub path: Option<String>,
    pub crc32: Option<String>,
    pub sha1: Option<String>,
    pub md5: Option<String>,
}

/// What a scan and the searches after it did, for the report and the tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub files: usize,
    pub archives: usize,
    pub members: usize,
    pub catalogue_rows: usize,
    /// Entries of the cache whose stamp no longer matched their file.
    pub dropped: usize,
    /// Hashes of files and members taken from the cache, their stamps
    /// matching.
    pub kept: usize,
    /// Every file or member hashed since the scan, in order.
    pub hashed: Vec<Place>,
    /// Every file or member whose hashing failed since the scan, and why.
    pub failed: Vec<(Place, String)>,
    /// What the scan could not read or will not search: folders and entries
    /// it could not read, archives it could not list, members with ZIP64
    /// fields or an ambiguous name, members that failed before.
    pub unreadable: Vec<(Place, String)>,
    /// Catalogue rows whose file has other bytes than the row claims.
    pub stale: Vec<(String, Place)>,
}

/// A library folder and its index.
#[derive(Debug)]
pub struct Library {
    /// The folder as the user named it.
    pub named: PathBuf,
    /// The folder, absolute and with symbolic links resolved.
    pub root: PathBuf,
    files: BTreeMap<String, IndexedFile>,
    claims: BTreeMap<String, Vec<Claim>>,
    by_hash: HashMap<String, BTreeSet<Place>>,
    /// Every searchable file and member, with its size and lower-case
    /// names, built once after the scan.
    places: Vec<Indexed>,
    by_size: HashMap<u64, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
    pub stats: Stats,
}

/// A place of [`Library::places`].
#[derive(Debug)]
struct Indexed {
    place: Place,
    size: u64,
    /// `path` or `path!member`, lower case.
    full: String,
    /// The last component, lower case.
    base: String,
}

/// The cache file of the library at `root` (absolute) in a store's cache.
pub fn cache_file(store_root: &Path, root: &Path) -> PathBuf {
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    store_root
        .join(CACHE_FOLDER)
        .join(format!("{:x}.json", digest))
}

/// What a library's index cache holds, for a listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheSummary {
    pub files: usize,
    pub archives: usize,
    pub members: usize,
    /// Files and members whose hash the index keeps.
    pub hashed: usize,
}

/// Count what the index cache at `path` holds; `None` when there is none
/// this build reads.
pub fn cache_summary(path: &Path) -> Option<CacheSummary> {
    let cache: CacheFile = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if cache.schema != SCHEMA {
        return None;
    }
    let mut summary = CacheSummary {
        files: cache.files.len(),
        archives: 0,
        members: 0,
        hashed: 0,
    };
    for file in &cache.files {
        summary.hashed += usize::from(file.sha256.is_some());
        if let Some(Listing::Members(members)) = &file.zip {
            summary.archives += 1;
            summary.members += members.len();
            summary.hashed += members.iter().filter(|m| m.sha256.is_some()).count();
        }
    }
    Some(summary)
}

impl Library {
    /// List the folder `named`, taking from the cache at `cache` (when
    /// given) the hashes and zip directories of files whose stamp still
    /// matches. Hashes nothing.
    pub fn scan(named: &Path, cache: Option<&Path>) -> io::Result<Self> {
        let root = named
            .canonicalize()
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", named.display())))?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: not a folder", named.display()),
            ));
        }
        let cached: BTreeMap<String, IndexedFile> = cache
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|c| c.schema == SCHEMA && Path::new(&c.library) == root)
            .map(|c| c.files.into_iter().map(|f| (f.path.clone(), f)).collect())
            .unwrap_or_default();
        let mut found = Vec::new();
        let mut problems = Vec::new();
        walk(&root, "", &mut BTreeSet::new(), &mut found, &mut problems);
        let mut library = Self {
            named: named.to_path_buf(),
            root,
            files: BTreeMap::new(),
            claims: BTreeMap::new(),
            by_hash: HashMap::new(),
            places: Vec::new(),
            by_size: HashMap::new(),
            by_name: HashMap::new(),
            stats: Stats::default(),
        };
        for (path, error) in problems {
            library
                .stats
                .unreadable
                .push((Place { path, member: None }, error));
        }
        let mut catalogues = Vec::new();
        for (path, metadata) in found {
            let stamp = Stamp::of(&metadata);
            let mut entry = match cached.get(&path) {
                Some(old) if old.stamp == stamp => {
                    library.stats.kept += usize::from(old.sha256.is_some());
                    if let Some(Listing::Members(members)) = &old.zip {
                        library.stats.kept += members.iter().filter(|m| m.sha256.is_some()).count();
                    }
                    old.clone()
                }
                old => {
                    if old.is_some() {
                        library.stats.dropped += 1;
                    }
                    IndexedFile {
                        path: path.clone(),
                        stamp,
                        sha256: None,
                        zip: None,
                    }
                }
            };
            let lower = path.to_ascii_lowercase();
            if lower.ends_with(".zip") && entry.zip.is_none() {
                entry.zip = Some(match zip::list(&library.root.join(&path)) {
                    Ok(members) => Listing::Members(
                        members
                            .into_iter()
                            .map(|member| IndexedMember {
                                member,
                                sha256: None,
                                error: None,
                            })
                            .collect(),
                    ),
                    Err(error) => Listing::Error(error.to_string()),
                });
            }
            let name = lower.rsplit('/').next().unwrap_or_default();
            if (name == "sha256sums" || name.ends_with(".sha256") || name.ends_with(".csv"))
                && entry.stamp.size <= MAX_CATALOGUE
            {
                catalogues.push(path.clone());
            }
            library.stats.files += 1;
            match &entry.zip {
                Some(Listing::Members(members)) => {
                    library.stats.archives += 1;
                    library.stats.members += members.len();
                    for m in members {
                        if let Some(why) = m.member.problem.as_ref().or(m.error.as_ref()) {
                            library.stats.unreadable.push((
                                Place {
                                    path: path.clone(),
                                    member: Some(m.member.name.clone()),
                                },
                                why.clone(),
                            ));
                        }
                    }
                }
                Some(Listing::Error(error)) => library.stats.unreadable.push((
                    Place {
                        path: path.clone(),
                        member: None,
                    },
                    error.clone(),
                )),
                None => {}
            }
            library.files.insert(path, entry);
        }
        for path in catalogues {
            library.read_catalogue(&path);
        }
        library.reindex();
        library.index_places();
        Ok(library)
    }

    /// Build the place table and its indexes by size and by name. Members
    /// with a problem or a failed hash are not searched.
    fn index_places(&mut self) {
        let mut places = Vec::new();
        for file in self.files.values() {
            places.push((
                Place {
                    path: file.path.clone(),
                    member: None,
                },
                file.stamp.size,
            ));
            if let Some(Listing::Members(members)) = &file.zip {
                for m in members {
                    if m.member.problem.is_none() && m.error.is_none() {
                        places.push((
                            Place {
                                path: file.path.clone(),
                                member: Some(m.member.name.clone()),
                            },
                            m.member.size,
                        ));
                    }
                }
            }
        }
        for (place, size) in places {
            let full = place.to_string().to_ascii_lowercase();
            let base = full
                .rsplit(['/', '!'])
                .next()
                .unwrap_or_default()
                .to_owned();
            let at = self.places.len();
            self.by_size.entry(size).or_default().push(at);
            self.by_name.entry(base.clone()).or_default().push(at);
            self.places.push(Indexed {
                place,
                size,
                full,
                base,
            });
        }
    }

    fn reindex(&mut self) {
        self.by_hash.clear();
        for file in self.files.values() {
            if let Some(sha) = &file.sha256 {
                self.by_hash.entry(sha.clone()).or_default().insert(Place {
                    path: file.path.clone(),
                    member: None,
                });
            }
            if let Some(Listing::Members(members)) = &file.zip {
                for m in members {
                    if let Some(sha) = &m.sha256 {
                        self.by_hash.entry(sha.clone()).or_default().insert(Place {
                            path: file.path.clone(),
                            member: Some(m.member.name.clone()),
                        });
                    }
                }
            }
        }
    }

    /// Read the catalogue at `path` (relative); rows that name no file of
    /// the library are ignored.
    fn read_catalogue(&mut self, path: &str) {
        let Ok(text) = fs::read_to_string(self.root.join(path)) else {
            return;
        };
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let join = |relative: &str| {
            let relative = relative.trim_start_matches("./").replace('\\', "/");
            if folder.is_empty() {
                relative
            } else {
                format!("{folder}/{relative}")
            }
        };
        let mut rows = Vec::new();
        if path.to_ascii_lowercase().ends_with(".csv") {
            let mut lines = text.lines();
            let Some(header) = lines.next() else {
                return;
            };
            let columns: Vec<String> = csv_fields(header)
                .iter()
                .map(|c| c.trim().to_ascii_lowercase())
                .collect();
            let column = |name: &str| columns.iter().position(|c| c == name);
            let (Some(p), Some(s), Some(h)) = (column("path"), column("size"), column("sha256"))
            else {
                return;
            };
            let (crc, sha1, md5) = (column("crc32"), column("sha1"), column("md5"));
            for line in lines {
                let fields = csv_fields(line);
                let get = |i: Option<usize>| {
                    i.and_then(|i| fields.get(i))
                        .map(|v| v.trim().to_ascii_lowercase())
                        .filter(|v| !v.is_empty())
                };
                let Some(file) = fields.get(p) else {
                    continue;
                };
                rows.push((
                    join(file.trim()),
                    Claim {
                        catalogue: path.to_owned(),
                        size: get(Some(s)).and_then(|v| v.parse().ok()),
                        sha256: get(Some(h)),
                        crc32: get(crc),
                        sha1: get(sha1),
                        md5: get(md5),
                    },
                ));
            }
        } else {
            for line in text.lines() {
                let Some((hex, file)) = line.split_once(' ') else {
                    continue;
                };
                let file = file.trim_start_matches(' ').trim_start_matches('*');
                if hex.len() != 64 || file.is_empty() {
                    continue;
                }
                rows.push((
                    join(file),
                    Claim {
                        catalogue: path.to_owned(),
                        sha256: Some(hex.to_ascii_lowercase()),
                        ..Claim::default()
                    },
                ));
            }
        }
        for (file, claim) in rows {
            if self.files.contains_key(&file) {
                self.stats.catalogue_rows += 1;
                self.claims.entry(file).or_default().push(claim);
            }
        }
    }

    /// The SHA-256 already known for a place, if any.
    pub fn known(&self, place: &Place) -> Option<&str> {
        let file = self.files.get(&place.path)?;
        match &place.member {
            None => file.sha256.as_deref(),
            Some(name) => match &file.zip {
                Some(Listing::Members(members)) => members
                    .iter()
                    .find(|m| &m.member.name == name)
                    .and_then(|m| m.sha256.as_deref()),
                _ => None,
            },
        }
    }

    /// Hash a place now and keep the hash under its stamp. A file that
    /// changed while it was read is not kept. A member that fails is
    /// recorded as failed under the archive's stamp and not tried again.
    pub fn hash(&mut self, place: &Place) -> io::Result<String> {
        let result = self.hash_inner(place);
        match &result {
            Ok(_) => self.stats.hashed.push(place.clone()),
            Err(error) => {
                self.stats.failed.push((place.clone(), error.to_string()));
                if let (Some(name), Some(file)) = (&place.member, self.files.get_mut(&place.path)) {
                    if let Some(Listing::Members(members)) = &mut file.zip {
                        if let Some(m) = members.iter_mut().find(|m| &m.member.name == name) {
                            m.error = Some(error.to_string());
                        }
                    }
                }
            }
        }
        result
    }

    fn hash_inner(&mut self, place: &Place) -> io::Result<String> {
        let path = self.root.join(&place.path);
        let Some(file) = self.files.get(&place.path) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, place.to_string()));
        };
        let before = file.stamp.clone();
        let sha = match &place.member {
            None => {
                let mut input = fs::File::open(&path)?;
                let mut sha = Sha256::new();
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    let n = input.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    sha.update(&buffer[..n]);
                }
                format!("{:x}", sha.finalize())
            }
            Some(name) => {
                let member = match &file.zip {
                    Some(Listing::Members(members)) => members
                        .iter()
                        .find(|m| &m.member.name == name)
                        .map(|m| m.member.clone()),
                    _ => None,
                }
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, place.to_string()))?;
                zip::hash(&path, &member)?
            }
        };
        if Stamp::of(&fs::metadata(&path)?) != before {
            return Err(io::Error::other(format!(
                "{}: changed while it was read",
                path.display()
            )));
        }
        let Some(file) = self.files.get_mut(&place.path) else {
            return Ok(sha);
        };
        match &place.member {
            None => file.sha256 = Some(sha.clone()),
            Some(name) => {
                if let Some(Listing::Members(members)) = &mut file.zip {
                    if let Some(m) = members.iter_mut().find(|m| &m.member.name == name) {
                        m.sha256 = Some(sha.clone());
                    }
                }
            }
        }
        self.by_hash
            .entry(sha.clone())
            .or_default()
            .insert(place.clone());
        Ok(sha)
    }

    /// Find bytes of this size and SHA-256: first where the index already
    /// knows the hash, then by hashing the candidates of that size in the
    /// order of recipes §3.4 (the path a file set gives; the canonical
    /// name; another listed name; a weaker hash or a claim of a catalogue
    /// or a zip directory; every other file of that size) until one
    /// matches. Hashes nothing of another size. The path a file set gives
    /// comes before the bare name, so a copy of a whole folder is hashed
    /// file by file, each once.
    pub fn find(&mut self, size: u64, sha256: &str, hints: &Hints) -> Option<Place> {
        if let Some(place) = self.known_place(size, sha256) {
            return Some(place);
        }
        for place in self.candidates(size, sha256, hints) {
            match self.hash(&place) {
                Ok(sha) if sha == sha256 => return Some(place),
                Ok(sha) => {
                    for claim in self.claims.get(&place.path).into_iter().flatten() {
                        if place.member.is_none()
                            && claim.sha256.as_deref().is_some_and(|c| c != sha)
                        {
                            self.stats
                                .stale
                                .push((claim.catalogue.clone(), place.clone()));
                        }
                    }
                }
                // Counted in `stats.failed` by `hash`.
                Err(_) => {}
            }
        }
        None
    }

    fn known_place(&self, size: u64, sha256: &str) -> Option<Place> {
        self.by_hash
            .get(sha256)?
            .iter()
            .find(|place| self.size_of(place) == Some(size) && self.known(place) == Some(sha256))
            .cloned()
    }

    fn size_of(&self, place: &Place) -> Option<u64> {
        let file = self.files.get(&place.path)?;
        match &place.member {
            None => Some(file.stamp.size),
            Some(name) => match &file.zip {
                Some(Listing::Members(members)) => members
                    .iter()
                    .find(|m| &m.member.name == name)
                    .map(|m| m.member.size),
                _ => None,
            },
        }
    }

    /// The places of `size` not yet hashed, in the search order.
    fn candidates(&self, size: u64, sha256: &str, hints: &Hints) -> Vec<Place> {
        let names: Vec<String> = hints.names.iter().map(|n| n.to_ascii_lowercase()).collect();
        let suffix = hints.path.as_ref().map(|p| p.to_ascii_lowercase());
        let (slash, bang) = match &suffix {
            Some(s) => (format!("/{s}"), format!("!{s}")),
            None => (String::new(), String::new()),
        };
        let mut ranked: Vec<(u8, &Place)> = self
            .by_size
            .get(&size)
            .into_iter()
            .flatten()
            .map(|&at| &self.places[at])
            .filter(|p| self.known(&p.place).is_none())
            .map(|p| {
                let at_path = suffix.as_ref().is_some_and(|s| {
                    p.full == *s || p.full.ends_with(&slash) || p.full.ends_with(&bang)
                });
                let rank = if at_path {
                    0
                } else if names.first() == Some(&p.base) {
                    1
                } else if names.iter().skip(1).any(|n| *n == p.base) {
                    2
                } else if self.hinted(&p.place, sha256, hints) {
                    3
                } else {
                    4
                };
                (rank, &p.place)
            })
            .collect();
        ranked.sort();
        ranked.into_iter().map(|(_, place)| place.clone()).collect()
    }

    /// A catalogue row or a zip directory says something of `place` that
    /// matches the value sought.
    fn hinted(&self, place: &Place, sha256: &str, hints: &Hints) -> bool {
        let same = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
        if let Some(name) = &place.member {
            if let Some(Listing::Members(members)) =
                self.files.get(&place.path).and_then(|f| f.zip.as_ref())
            {
                let crc = members
                    .iter()
                    .find(|m| &m.member.name == name)
                    .map(|m| m.member.crc32.clone());
                if same(&crc, &hints.crc32) {
                    return true;
                }
            }
            return false;
        }
        self.claims
            .get(&place.path)
            .into_iter()
            .flatten()
            .any(|claim| {
                claim.sha256.as_deref() == Some(sha256)
                    || same(&claim.crc32, &hints.crc32)
                    || same(&claim.sha1, &hints.sha1)
                    || same(&claim.md5, &hints.md5)
            })
    }

    /// Files and members that are near misses for a value not found: those
    /// with one of its names and another size or hash, then those of its
    /// size whose hash is known to differ, each group in path order. Read
    /// from the indexes by name and by size, not from every place.
    pub fn near_misses(&self, size: u64, sha256: &str, hints: &Hints) -> Vec<NearMiss> {
        let mut named = BTreeSet::new();
        for name in &hints.names {
            if let Some(found) = self.by_name.get(&name.to_ascii_lowercase()) {
                named.extend(found.iter().copied());
            }
        }
        let sized: BTreeSet<usize> = self
            .by_size
            .get(&size)
            .into_iter()
            .flatten()
            .copied()
            .filter(|at| !named.contains(at))
            .collect();
        let miss = |at: usize, is_named: bool| {
            let p = &self.places[at];
            let known = self.known(&p.place);
            (known != Some(sha256) && (is_named || known.is_some())).then(|| NearMiss {
                place: p.place.clone(),
                size: p.size,
                sha256: known.map(str::to_owned),
                named: is_named,
            })
        };
        let mut out: Vec<NearMiss> = named.iter().filter_map(|&at| miss(at, true)).collect();
        out.sort_by(|a, b| a.place.cmp(&b.place));
        let mut by_size: Vec<NearMiss> = sized.iter().filter_map(|&at| miss(at, false)).collect();
        by_size.sort_by(|a, b| a.place.cmp(&b.place));
        out.extend(by_size);
        out
    }

    /// The bytes at a place, for a build: a file read whole, or a member
    /// decoded and checked against its archive's directory. The caller
    /// checks them against the size and SHA-256 it wants; the index is not
    /// trusted for that (recipes §3.4: a value is hashed again when used).
    pub fn read(&self, place: &Place) -> io::Result<Vec<u8>> {
        let path = self.root.join(&place.path);
        match &place.member {
            None => fs::read(&path),
            Some(name) => {
                let member = match self.files.get(&place.path).and_then(|f| f.zip.as_ref()) {
                    Some(Listing::Members(members)) => members
                        .iter()
                        .find(|m| &m.member.name == name)
                        .map(|m| m.member.clone()),
                    _ => None,
                }
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, place.to_string()))?;
                let mut out = Vec::new();
                zip::extract(&path, &member, &mut |chunk| {
                    out.extend_from_slice(chunk);
                    Ok(())
                })?;
                Ok(out)
            }
        }
    }

    /// Write the index to `path`, by rename from a temporary sibling.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let file = CacheFile {
            schema: SCHEMA,
            library: self.root.to_string_lossy().into_owned(),
            files: self.files.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec(&file).map_err(io::Error::other)?;
        write_cache(path, &bytes)
    }
}

/// A file or member that looks like a wanted value but is not it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearMiss {
    pub place: Place,
    pub size: u64,
    /// Its SHA-256, when it was hashed.
    pub sha256: Option<String>,
    /// It has one of the value's names.
    pub named: bool,
}

/// Replace a cache file atomically: a reader sees the old file or the new
/// one, never half of one. Two writers race harmlessly; the last one wins.
pub(crate) fn write_cache(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}-{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::write(&temporary, bytes).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Every regular file below `dir`, symbolic links followed and each folder
/// visited once, in path order. A folder or entry that cannot be read is
/// put in `problems` with its relative path (`.` for the library itself)
/// and the error, and the walk goes on.
fn walk(
    dir: &Path,
    prefix: &str,
    seen: &mut BTreeSet<PathBuf>,
    out: &mut Vec<(String, fs::Metadata)>,
    problems: &mut Vec<(String, String)>,
) {
    let here = if prefix.is_empty() { "." } else { prefix };
    let canonical = match dir.canonicalize() {
        Ok(path) => path,
        Err(error) => return problems.push((here.to_owned(), error.to_string())),
    };
    if !seen.insert(canonical) {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return problems.push((here.to_owned(), error.to_string())),
    };
    let mut listed = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => listed.push(entry),
            Err(error) => problems.push((here.to_owned(), error.to_string())),
        }
    }
    listed.sort_by_key(|e| e.file_name());
    for entry in listed {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                problems.push((relative, error.to_string()));
                continue;
            }
        };
        if metadata.is_dir() {
            walk(&path, &relative, seen, out, problems);
        } else if metadata.is_file() {
            out.push((relative, metadata));
        }
    }
}

/// The fields of one CSV line: commas separate, double quotes group, a
/// doubled quote inside quotes is one.
fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                fields.last_mut().unwrap_or(&mut String::new()).push('"');
            }
            (_, '"') => quoted = !quoted,
            (false, ',') => fields.push(String::new()),
            (_, c) => {
                if let Some(field) = fields.last_mut() {
                    field.push(c);
                }
            }
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) struct Temp(pub PathBuf);
    impl Temp {
        pub(crate) fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("recipe-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn place(path: &str) -> Place {
        Place {
            path: path.into(),
            member: None,
        }
    }

    #[test]
    fn a_scan_hashes_nothing_and_a_search_hashes_only_files_of_the_wanted_size() {
        let temp = Temp::new("library-sizes");
        let lib = temp.0.join("lib");
        fs::create_dir_all(lib.join("sub")).unwrap();
        fs::write(lib.join("a.bin"), [1u8; 100]).unwrap();
        fs::write(lib.join("b.bin"), [2u8; 100]).unwrap();
        fs::write(lib.join("sub/c.bin"), [3u8; 100]).unwrap();
        fs::write(lib.join("d.bin"), [4u8; 101]).unwrap();
        fs::write(lib.join("e.bin"), [5u8; 7]).unwrap();
        let mut library = Library::scan(&lib, None).unwrap();
        assert_eq!(library.stats.files, 5);
        assert!(library.stats.hashed.is_empty(), "a scan hashes nothing");
        let wanted = sha(&[3u8; 100]);
        let found = library.find(100, &wanted, &Hints::default());
        assert_eq!(found, Some(place("sub/c.bin")));
        // Only files of 100 bytes were hashed, in path order until the match.
        assert_eq!(
            library.stats.hashed,
            [place("a.bin"), place("b.bin"), place("sub/c.bin")]
        );
        // A name moves its file to the front.
        let mut library = Library::scan(&lib, None).unwrap();
        let hints = Hints {
            names: vec!["C.BIN".into()],
            ..Hints::default()
        };
        assert_eq!(library.find(100, &wanted, &hints), Some(place("sub/c.bin")));
        assert_eq!(library.stats.hashed, [place("sub/c.bin")]);
        // A value of a size no file has hashes nothing.
        assert_eq!(library.find(55, &wanted, &hints), None);
        assert_eq!(library.stats.hashed.len(), 1);
    }

    #[test]
    fn a_kept_hash_is_reused_and_a_stale_entry_is_dropped_never_trusted() {
        let temp = Temp::new("library-stale");
        let lib = temp.0.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let cache = temp.0.join("cache.json");
        let file = lib.join("game.bin");
        fs::write(&file, [1u8; 64]).unwrap();
        fs::write(lib.join("other.bin"), [9u8; 64]).unwrap();
        let first = sha(&[1u8; 64]);
        let mut library = Library::scan(&lib, Some(&cache)).unwrap();
        assert_eq!(
            library.find(64, &first, &Hints::default()),
            Some(place("game.bin"))
        );
        library.save(&cache).unwrap();

        // Unchanged: the hash is kept and nothing is hashed again.
        let mut library = Library::scan(&lib, Some(&cache)).unwrap();
        assert_eq!((library.stats.kept, library.stats.dropped), (1, 0));
        assert_eq!(
            library.find(64, &first, &Hints::default()),
            Some(place("game.bin"))
        );
        assert!(library.stats.hashed.is_empty());

        // Another size: the entry is dropped and the file hashed again.
        fs::write(&file, [2u8; 65]).unwrap();
        let mut library = Library::scan(&lib, Some(&cache)).unwrap();
        assert_eq!((library.stats.kept, library.stats.dropped), (0, 1));
        assert_eq!(library.find(64, &first, &Hints::default()), None);
        assert_eq!(library.stats.hashed, [place("other.bin")]);
        let second = sha(&[2u8; 65]);
        assert_eq!(
            library.find(65, &second, &Hints::default()),
            Some(place("game.bin"))
        );
        library.save(&cache).unwrap();
        same_size_rewrite(&lib, &cache, &file, &second);
    }

    /// Other bytes of the same size, with the modification time put back:
    /// on Unix the change time differs, so the entry is dropped and the file
    /// is hashed again; the old hash is never found. Elsewhere the index
    /// cannot see such a rewrite (the module documentation says so).
    #[cfg(unix)]
    fn same_size_rewrite(lib: &Path, cache: &Path, file: &Path, old: &str) {
        let modified = fs::metadata(file).unwrap().modified().unwrap();
        let before = Stamp::of(&fs::metadata(file).unwrap());
        for _ in 0..100 {
            fs::write(file, [3u8; 65]).unwrap();
            fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(modified)
                .unwrap();
            if Stamp::of(&fs::metadata(file).unwrap()) != before {
                break;
            }
            // A coarse clock gave the rewrite the same change time: write
            // again at its next tick.
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let now = Stamp::of(&fs::metadata(file).unwrap());
        assert_eq!(
            (now.size, now.modified_s, now.modified_ns),
            (before.size, before.modified_s, before.modified_ns),
            "size and modification time are as they were"
        );
        let mut library = Library::scan(lib, Some(cache)).unwrap();
        assert_eq!(library.stats.dropped, 1);
        assert_eq!(library.find(65, old, &Hints::default()), None);
        assert_eq!(
            library.find(65, &sha(&[3u8; 65]), &Hints::default()),
            Some(place("game.bin"))
        );
    }

    #[cfg(not(unix))]
    fn same_size_rewrite(_: &Path, _: &Path, _: &Path, _: &str) {}

    #[test]
    fn zip_members_are_listed_without_hashing_and_found_by_their_hash() {
        let temp = Temp::new("library-zip");
        let lib = temp.0.join("lib");
        fs::create_dir_all(&lib).unwrap();
        use crate::zip::build::{archive, stored};
        let bytes = archive(&[
            stored("GAME/KEN.EXE", b"executable bytes"),
            stored("GAME/OTHER.DAT", b"other bytes here"),
        ]);
        fs::write(lib.join("LABFULL.ZIP"), bytes).unwrap();
        let mut library = Library::scan(&lib, None).unwrap();
        assert_eq!((library.stats.archives, library.stats.members), (1, 2));
        assert!(library.stats.hashed.is_empty());
        let wanted = sha(b"executable bytes");
        let found = library.find(
            16,
            &wanted,
            &Hints {
                names: vec!["KEN.EXE".into()],
                ..Hints::default()
            },
        );
        let ken = Place {
            path: "LABFULL.ZIP".into(),
            member: Some("GAME/KEN.EXE".into()),
        };
        assert_eq!(found, Some(ken.clone()));
        assert_eq!(library.stats.hashed, std::slice::from_ref(&ken));
        assert_eq!(ken.to_string(), "LABFULL.ZIP!GAME/KEN.EXE");
        // The member's hash survives the cache with the directory.
        let cache = temp.0.join("cache.json");
        library.save(&cache).unwrap();
        let mut library = Library::scan(&lib, Some(&cache)).unwrap();
        assert_eq!(library.stats.kept, 1);
        assert_eq!(library.find(16, &wanted, &Hints::default()), Some(ken));
        assert!(library.stats.hashed.is_empty());
        assert_eq!(
            cache_summary(&cache),
            Some(CacheSummary {
                files: 1,
                archives: 1,
                members: 2,
                hashed: 1
            })
        );
    }

    #[test]
    fn catalogue_rows_are_hints_and_a_stale_row_is_reported() {
        let temp = Temp::new("library-catalogue");
        let lib = temp.0.join("lib");
        fs::create_dir_all(lib.join("roms")).unwrap();
        fs::write(lib.join("roms/a.rom"), [1u8; 32]).unwrap();
        fs::write(lib.join("roms/b.rom"), [2u8; 32]).unwrap();
        fs::write(lib.join("roms/c.rom"), [3u8; 32]).unwrap();
        let (b, c) = (sha(&[2u8; 32]), sha(&[3u8; 32]));
        // The CSV says c.rom holds b's bytes, which it does not; the
        // checksum file is right about c.rom.
        fs::write(
            lib.join("roms/KATALOG.csv"),
            format!("path,size,sha256,crc32\n\"c.rom\",32,{b},\n"),
        )
        .unwrap();
        fs::write(lib.join("SHA256SUMS"), format!("{c}  roms/c.rom\n")).unwrap();
        let mut library = Library::scan(&lib, None).unwrap();
        assert_eq!(library.stats.catalogue_rows, 2);
        // b is sought: the row that claims it moves c.rom first; c.rom is
        // hashed, is not b, and the row is reported stale. Then a.rom, b.rom.
        assert_eq!(
            library.find(32, &b, &Hints::default()),
            Some(place("roms/b.rom"))
        );
        assert_eq!(
            library.stats.hashed,
            [
                place("roms/c.rom"),
                place("roms/a.rom"),
                place("roms/b.rom")
            ]
        );
        assert_eq!(
            library.stats.stale,
            [("roms/KATALOG.csv".to_owned(), place("roms/c.rom"))]
        );
        assert_eq!(csv_fields(r#"a,"b,c","d""e""#), ["a", "b,c", "d\"e"]);
    }

    #[test]
    fn a_member_that_fails_is_recorded_and_not_tried_again_until_the_archive_changes() {
        let temp = Temp::new("library-failed");
        let lib = temp.0.join("lib");
        fs::create_dir_all(&lib).unwrap();
        use crate::zip::build::{archive, stored, Entry};
        // The directory states 3 bytes; the member holds 5.
        let zip = archive(&[Entry {
            stated: Some(3),
            ..stored("LIAR.BIN", b"12345")
        }]);
        fs::write(lib.join("A.ZIP"), &zip).unwrap();
        let cache = temp.0.join("cache.json");
        let wanted = sha(b"123");
        let mut library = Library::scan(&lib, Some(&cache)).unwrap();
        assert_eq!(library.find(3, &wanted, &Hints::default()), None);
        let liar = Place {
            path: "A.ZIP".into(),
            member: Some("LIAR.BIN".into()),
        };
        assert!(library.stats.hashed.is_empty());
        assert_eq!(library.stats.failed.len(), 1);
        assert!(library.stats.failed[0]
            .1
            .contains("expands past the 3 bytes"));
        library.save(&cache).unwrap();

        // The next scan reports it and the search does not pay again.
        let mut library = Library::scan(&lib, Some(&cache)).unwrap();
        assert_eq!(library.stats.unreadable.len(), 1);
        assert_eq!(library.stats.unreadable[0].0, liar);
        assert_eq!(library.find(3, &wanted, &Hints::default()), None);
        assert!(library.stats.failed.is_empty() && library.stats.hashed.is_empty());

        // A changed archive is listed afresh and tried again.
        fs::write(lib.join("A.ZIP"), archive(&[stored("LIAR.BIN", b"123")])).unwrap();
        let mut library = Library::scan(&lib, Some(&cache)).unwrap();
        assert_eq!(library.find(3, &wanted, &Hints::default()), Some(liar));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_read_is_reported_and_the_scan_goes_on() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new("library-locked");
        let lib = temp.0.join("lib");
        fs::create_dir_all(lib.join("locked")).unwrap();
        fs::write(lib.join("locked/a.bin"), b"a").unwrap();
        fs::write(lib.join("open.bin"), b"b").unwrap();
        std::os::unix::fs::symlink(lib.join("gone"), lib.join("dangling")).unwrap();
        fs::set_permissions(lib.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        // Whoever may read the folder anyway (a superuser) cannot test this.
        let readable = fs::read_dir(lib.join("locked")).is_ok();
        let library = Library::scan(&lib, None);
        fs::set_permissions(lib.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            return;
        }
        let library = library.unwrap();
        assert_eq!(library.stats.files, 1, "open.bin");
        let paths: Vec<String> = library
            .stats
            .unreadable
            .iter()
            .map(|(place, _)| place.to_string())
            .collect();
        assert_eq!(paths, ["dangling", "locked"]);
    }

    #[test]
    fn near_misses_are_the_same_name_with_other_bytes_and_the_same_size_hashed() {
        let temp = Temp::new("library-near");
        let lib = temp.0.join("lib");
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("DISK1.IMG"), [1u8; 10]).unwrap();
        fs::write(lib.join("other.img"), [2u8; 20]).unwrap();
        let mut library = Library::scan(&lib, None).unwrap();
        let hints = Hints {
            names: vec!["disk1.img".into()],
            ..Hints::default()
        };
        let wanted = sha(&[7u8; 20]);
        assert_eq!(library.find(20, &wanted, &hints), None);
        let misses = library.near_misses(20, &wanted, &hints);
        assert_eq!(
            misses,
            [
                NearMiss {
                    place: place("DISK1.IMG"),
                    size: 10,
                    sha256: None,
                    named: true
                },
                NearMiss {
                    place: place("other.img"),
                    size: 20,
                    sha256: Some(sha(&[2u8; 20])),
                    named: false
                }
            ]
        );
    }
}
