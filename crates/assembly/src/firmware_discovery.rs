// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-only SHA-256 discovery before assembly; no guest device or clock is used.
//! Files remain in place. See `backlog/docs/design/firmware-and-machine-classes.md` §3.5.

use crate::{FirmwareDistribution, FirmwareEntry, ImagePart, ImageSource, KnownPart};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

mod frontend;
mod persistent;
pub use frontend::{default_firmware_roots, locate_firmware_with_roots, FirmwareRoot, RootOrigin};
pub use persistent::{
    firmware_cache_path, with_discovery_session, with_firmware_pins, DiscoveryControl,
    DiscoveryPhase, DiscoveryProgress,
};

/// The identity representation found by discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserFirmwareLayout {
    /// One file matches the assembled image identity.
    Assembled,
    /// Zero-based index in the catalogue entry's alternative chip layouts.
    Parts(usize),
}

/// Paths ready to assign to a machine's load image. Do not log `source`:
/// `files` carries names with the private search root replaced by a placeholder.
pub struct FoundUserFirmware {
    /// A file or interleaved parts, including catalogue lanes and offsets.
    pub source: ImageSource,
    /// The matched catalogue representation.
    pub layout: UserFirmwareLayout,
    /// Root-relative names safe to display; never firmware bytes or hashes.
    pub files: Vec<String>,
    redactions: Vec<String>,
}

impl FoundUserFirmware {
    /// Remove the configured external root and its canonical spelling from a
    /// downstream diagnostic, including escaped paths in debug output.
    pub fn redact(&self, text: &str) -> String {
        self.redactions.iter().fold(text.to_owned(), |text, root| {
            text.replace(root, "<UNEMUPC98_USER_ROMS_DIR>")
        })
    }

    /// Fail at the calling test without exposing paths embedded in an error.
    #[track_caller]
    pub fn expect_redacted<T, E: std::fmt::Display>(
        &self,
        result: Result<T, E>,
        context: &str,
    ) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!(
                "{}: {}",
                self.redact(context),
                self.redact(&error.to_string())
            ),
        }
    }

    /// Require a diagnostic without formatting a possibly private success value.
    #[track_caller]
    pub fn expect_err_redacted<T, E>(&self, result: Result<T, E>, context: &str) -> E {
        match result {
            Ok(_) => panic!("{}: expected an error", self.redact(context)),
            Err(error) => error,
        }
    }
}

/// Discovery failures contain safe labels rather than private absolute paths.
#[derive(Debug, thiserror::Error)]
pub enum UserFirmwareError {
    /// A host scan was cancelled before resolution completed.
    #[error("firmware scan cancelled")]
    Cancelled,
    /// A test requested an entry outside the supplied board catalogue.
    #[error("unknown user-supplied firmware entry {0}")]
    UnknownEntry(String),
    /// Open fixtures belong to the ordinary fixture lane.
    #[error("firmware entry {0} is not user-supplied")]
    NotUserSupplied(String),
    /// A configured external search root must be usable independently of matches.
    #[error("<UNEMUPC98_USER_ROMS_DIR>: {0}")]
    ExternalRoot(&'static str),
    /// Inaccessible files must not be mistaken for absent firmware.
    #[error("cannot scan user-supplied firmware at {file}: {kind}")]
    Scan {
        /// Redacted root and relative name.
        file: String,
        /// Only the error kind, since OS messages can contain private paths.
        kind: io::ErrorKind,
    },
    /// A previous panic interrupted the shared hash cache.
    #[error("user-supplied firmware hash cache unavailable")]
    CacheUnavailable,
}

struct SearchRoot {
    path: PathBuf,
    label: &'static str,
    external: bool,
}

#[cfg(test)]
fn search_roots(repository: &Path, external: Option<OsString>) -> Vec<SearchRoot> {
    let mut roots = vec![SearchRoot {
        path: repository.join("vendor/user-roms"),
        label: "vendor/user-roms",
        external: false,
    }];
    if let Some(path) = external {
        roots.push(SearchRoot {
            path: path.into(),
            label: "<UNEMUPC98_USER_ROMS_DIR>",
            external: true,
        });
    }
    roots
}

fn validate_external_roots(roots: &[SearchRoot]) -> Result<Vec<String>, UserFirmwareError> {
    let mut redactions = Vec::new();
    for root in roots.iter().filter(|root| root.external) {
        if !root.path.is_absolute() {
            return Err(UserFirmwareError::ExternalRoot("must be an absolute path"));
        }
        let metadata = fs::metadata(&root.path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                UserFirmwareError::ExternalRoot("not found")
            } else {
                root.error(&root.path, error)
            }
        })?;
        if !metadata.is_dir() {
            return Err(UserFirmwareError::ExternalRoot("is not a directory"));
        }
        let canonical = root
            .path
            .canonicalize()
            .map_err(|error| root.error(&root.path, error))?;
        for path in [&root.path, &canonical] {
            redactions.push(path.display().to_string());
            let debug = format!("{path:?}");
            if let Some(escaped) = debug.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                redactions.push(escaped.into());
            }
        }
    }
    redactions.sort_by_key(|root| std::cmp::Reverse(root.len()));
    redactions.dedup();
    Ok(redactions)
}

impl SearchRoot {
    fn name(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.path).unwrap_or(Path::new(""));
        if relative.as_os_str().is_empty() {
            self.label.into()
        } else {
            format!("{}/{}", self.label, relative.display())
        }
    }

    fn error(&self, path: &Path, error: io::Error) -> UserFirmwareError {
        UserFirmwareError::Scan {
            file: self.name(path),
            kind: error.kind(),
        }
    }
}

type HashKey = (PathBuf, u64, std::time::SystemTime, (i64, i64), (u64, u64));

fn hash_key(path: PathBuf, metadata: &fs::Metadata) -> io::Result<HashKey> {
    #[cfg(unix)]
    let (change, identity) = {
        use std::os::unix::fs::MetadataExt;
        (
            (metadata.ctime(), metadata.ctime_nsec()),
            (metadata.dev(), metadata.ino()),
        )
    };
    #[cfg(not(unix))]
    let (change, identity) = ((0, 0), (0, 0));
    Ok((path, metadata.len(), metadata.modified()?, change, identity))
}

#[derive(Default)]
struct HashCache {
    hashes: BTreeMap<HashKey, String>,
    seen: BTreeSet<PathBuf>,
    listed_directories: BTreeSet<PathBuf>,
    listing: Option<(Vec<FirmwareRoot>, Vec<ListedFile>)>,
    candidates: Vec<Candidate>,
    checked_candidates: BTreeSet<PathBuf>,
    matched_candidates: BTreeSet<PathBuf>,
    reported_mismatches: BTreeSet<PathBuf>,
    pins: Vec<FirmwarePin>,
    unpersistable: BTreeSet<PathBuf>,
    dirty: bool,
    #[cfg(test)]
    hash_time: Option<std::time::SystemTime>,
    #[cfg(test)]
    cancel_after_hash: bool,
    bypass: BTreeSet<PathBuf>,
    control: Option<DiscoveryControl>,
    #[cfg(test)]
    calls: usize,
    #[cfg(test)]
    after_read: Option<fn(&Path)>,
}

impl HashCache {
    fn hash(&mut self, path: &Path) -> io::Result<String> {
        if self
            .control
            .as_ref()
            .is_some_and(DiscoveryControl::is_cancelled)
        {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
        }
        #[cfg(test)]
        let hashed_at = self.hash_time.unwrap_or_else(std::time::SystemTime::now);
        #[cfg(not(test))]
        let hashed_at = std::time::SystemTime::now();
        let canonical = path.canonicalize()?;
        let before = fs::metadata(&canonical)?;
        let key = hash_key(canonical.clone(), &before)?;
        self.seen.insert(canonical.clone());
        // Windows metadata has no portable change-time identity. Rehash there
        // rather than trust a hit after an in-place edit with a restored mtime.
        #[cfg(not(windows))]
        if let Some(hash) = self
            .hashes
            .get(&key)
            .filter(|_| !self.bypass.contains(&key.0))
        {
            if hash_key(canonical.clone(), &fs::metadata(&canonical)?)? == key {
                return Ok(hash.clone());
            }
        }
        let mut file = fs::File::open(&canonical)?;
        if let Some(control) = &self.control {
            control.update(|progress| progress.files_read += 1);
        }
        let mut digest = Sha256::new();
        let mut buffer = [0; 8192];
        let mut length = 0;
        loop {
            if self
                .control
                .as_ref()
                .is_some_and(DiscoveryControl::is_cancelled)
            {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
            }
            let remaining = before.len().saturating_add(1).saturating_sub(length);
            let limit = buffer.len().min(remaining as usize);
            let count = file.read(&mut buffer[..limit])?;
            if count == 0 {
                break;
            }
            length += count as u64;
            digest.update(&buffer[..count]);
        }
        #[cfg(test)]
        if let Some(after_read) = self.after_read {
            after_read(path);
        }
        if length != before.len()
            || hash_key(canonical.clone(), &file.metadata()?)? != key
            || hash_key(canonical, &fs::metadata(path)?)? != key
        {
            return Err(io::Error::other("ROM changed while hashing"));
        }
        let hash = format!("{:x}", digest.finalize());
        // Coarse filesystem timestamps can hide another write in this window.
        // Such hashes are useful within this scan but must not enter the disk index.
        let racy = persistent::racy_timestamp(&key, hashed_at);
        let unchanged =
            self.hashes.get(&key) == Some(&hash) && self.unpersistable.contains(&key.0) == racy;
        let previously_persisted = self
            .hashes
            .keys()
            .any(|old| old.0 == key.0 && !self.unpersistable.contains(&old.0));
        self.hashes.retain(|old, _| old.0 != key.0);
        self.bypass.remove(&key.0);
        if racy {
            self.unpersistable.insert(key.0.clone());
        } else {
            self.unpersistable.remove(&key.0);
        }
        self.hashes.insert(key, hash.clone());
        self.dirty |= if racy {
            previously_persisted
        } else {
            !unchanged
        };
        #[cfg(test)]
        if self.cancel_after_hash {
            if let Some(control) = &self.control {
                control.cancel();
            }
        }
        #[cfg(test)]
        {
            self.calls += 1;
        }
        Ok(hash)
    }
}

type ListedFile = (PathBuf, PathBuf, u64, String);

#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    canonical: PathBuf,
    name: String,
    size: u64,
    hash: String,
}

fn scan(
    root: &SearchRoot,
    path: &Path,
    sizes: &BTreeSet<u64>,
    cache: &mut HashCache,
    candidates: &mut Vec<Candidate>,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<(), UserFirmwareError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && path == root.path && !root.external =>
        {
            return Ok(())
        }
        Err(error) => return Err(root.error(path, error)),
    };
    if metadata.is_dir() {
        // Directory aliases and back-links must not repeat traversal or form loops.
        let canonical = path
            .canonicalize()
            .map_err(|error| root.error(path, error))?;
        if !visited.insert(canonical) {
            return Ok(());
        }
        let mut children = fs::read_dir(path)
            .map_err(|error| root.error(path, error))?
            .map(|child| child.map(|child| child.path()))
            .collect::<io::Result<Vec<_>>>()
            .map_err(|error| root.error(path, error))?;
        children.sort();
        for child in children {
            scan(root, &child, sizes, cache, candidates, visited)?;
        }
    } else if metadata.is_file() && (sizes.is_empty() || sizes.contains(&metadata.len())) {
        let canonical = path
            .canonicalize()
            .map_err(|error| root.error(path, error))?;
        if candidates.iter().any(|file| file.canonical == canonical) {
            return Ok(());
        }
        let hash = cache.hash(path).map_err(|error| root.error(path, error))?;
        candidates.push(Candidate {
            path: path.into(),
            canonical,
            name: root.name(path),
            size: metadata.len(),
            hash,
        });
    }
    Ok(())
}

fn matching_parts<'a>(
    layout: &'a [KnownPart],
    candidates: &'a [Candidate],
) -> Vec<(&'a KnownPart, &'a Candidate)> {
    let mut used = BTreeSet::new();
    layout
        .iter()
        .filter_map(|part| {
            let file = candidates.iter().find(|file| {
                file.size == part.size
                    && file.hash == part.sha256
                    && !used.contains(&file.canonical)
            })?;
            used.insert(file.canonical.clone());
            Some((part, file))
        })
        .collect()
}

fn matched(entry: &FirmwareEntry, candidates: &[Candidate]) -> Option<FoundUserFirmware> {
    if let Some(image) = candidates
        .iter()
        .find(|file| file.size == entry.size && file.hash == entry.sha256)
    {
        return Some(FoundUserFirmware {
            source: ImageSource::File(image.path.clone()),
            layout: UserFirmwareLayout::Assembled,
            files: vec![image.name.clone()],
            redactions: Vec::new(),
        });
    }
    for (index, layout) in entry.parts.iter().enumerate() {
        if layout.is_empty() {
            continue;
        }
        let files = matching_parts(layout, candidates);
        if files.len() == layout.len() {
            return Some(FoundUserFirmware {
                source: ImageSource::Interleaved(
                    files
                        .iter()
                        .map(|(part, file)| ImagePart {
                            path: file.path.clone(),
                            lane: part.lane,
                            offset: part.offset,
                        })
                        .collect(),
                ),
                layout: UserFirmwareLayout::Parts(index),
                files: files.iter().map(|(_, file)| file.name.clone()).collect(),
                redactions: Vec::new(),
            });
        }
    }
    None
}

#[derive(Default)]
struct Discovery {
    found: Option<FoundUserFirmware>,
    incomplete: Vec<String>,
}

fn find_in_roots(
    entry_id: &str,
    catalogue: &[FirmwareEntry],
    roots: &[SearchRoot],
    cache: &mut HashCache,
) -> Result<Discovery, UserFirmwareError> {
    let entry = catalogue
        .iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| UserFirmwareError::UnknownEntry(entry_id.into()))?;
    if entry.distribution != FirmwareDistribution::UserSupplied {
        return Err(UserFirmwareError::NotUserSupplied(entry_id.into()));
    }
    let redactions = validate_external_roots(roots)?;
    let mut sizes = BTreeSet::from([entry.size]);
    sizes.extend(
        entry
            .parts
            .iter()
            .flat_map(|layout| layout.iter().map(|p| p.size)),
    );
    let mut candidates = Vec::new();
    let mut visited = BTreeSet::new();
    for root in roots {
        scan(
            root,
            &root.path,
            &sizes,
            cache,
            &mut candidates,
            &mut visited,
        )?;
        if let Some(mut found) = matched(entry, &candidates) {
            found.redactions = redactions;
            return Ok(Discovery {
                found: Some(found),
                incomplete: Vec::new(),
            });
        }
    }
    let incomplete = entry
        .parts
        .iter()
        .enumerate()
        .filter_map(|(index, layout)| {
            let count = matching_parts(layout, &candidates).len();
            if count > 0 && count < layout.len() {
                Some(format!(
                    "layout {index}: {count} of {} parts found",
                    layout.len()
                ))
            } else {
                None
            }
        })
        .collect();
    Ok(Discovery {
        found: None,
        incomplete,
    })
}

/// Discover a catalogue identity recursively in explicit host directories.
/// Names do not determine identity. Only the assembled or chip lengths in the
/// catalogue are hashed, and correct layouts can span multiple roots.
/// Hashes are cached by canonical path and file metadata; edits are rehashed.
/// Unusable roots and entries are skipped with warnings; symlinks cannot escape
/// the selected root. The legacy required-ROM test lane uses its strict policy.
pub fn find_catalogue_in_roots(
    entry_id: &str,
    catalogue: &[FirmwareEntry],
    paths: &[PathBuf],
) -> Result<Option<FoundUserFirmware>, UserFirmwareError> {
    let entry = catalogue
        .iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| UserFirmwareError::UnknownEntry(entry_id.into()))?;
    if entry.distribution != FirmwareDistribution::UserSupplied {
        return Err(UserFirmwareError::NotUserSupplied(entry_id.into()));
    }
    let roots = paths
        .iter()
        .map(|path| FirmwareRoot::new(path, RootOrigin::CommandLine))
        .collect::<Vec<_>>();
    let assembled = FirmwarePin {
        sha256: entry.sha256.into(),
        length: Some(entry.size),
    };
    let layouts = if entry.parts.is_empty() {
        vec![&[][..]]
    } else {
        entry.parts.to_vec()
    };
    let mut warnings = Vec::new();
    let result = (|| {
        for (index, layout) in layouts.iter().enumerate() {
            let parts = layout
                .iter()
                .map(|part| PinnedPart {
                    part: ImagePart {
                        path: PathBuf::new(),
                        lane: part.lane,
                        offset: part.offset,
                    },
                    pin: Some(FirmwarePin {
                        sha256: part.sha256.into(),
                        length: Some(part.size),
                    }),
                })
                .collect::<Vec<_>>();
            if let Some(source) =
                locate_firmware_with_roots(&roots, None, Some(&assembled), &parts, &mut warnings)?
            {
                let (layout, files) = match &source {
                    ImageSource::File(path) => (UserFirmwareLayout::Assembled, vec![path]),
                    ImageSource::Interleaved(parts) => (
                        UserFirmwareLayout::Parts(index),
                        parts.iter().map(|part| &part.path).collect(),
                    ),
                    _ => continue,
                };
                let files = files
                    .into_iter()
                    .map(|path| {
                        roots
                            .iter()
                            .find(|root| path.starts_with(&root.path))
                            .map_or_else(
                                || "<ROM file>".into(),
                                |root| {
                                    SearchRoot {
                                        path: root.path.clone(),
                                        label: root.origin.label(),
                                        external: true,
                                    }
                                    .name(path)
                                },
                            )
                    })
                    .collect();
                let redactions = paths
                    .iter()
                    .flat_map(|path| [Some(path.clone()), path.canonicalize().ok()])
                    .flatten()
                    .map(|path| path.display().to_string())
                    .collect();
                return Ok(Some(FoundUserFirmware {
                    source,
                    layout,
                    files,
                    redactions,
                }));
            }
        }
        Ok(None)
    })();
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    result
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn find_test_firmware(
    entry_id: &str,
    catalogue: &[FirmwareEntry],
    paths: &[PathBuf],
) -> Result<Option<FoundUserFirmware>, UserFirmwareError> {
    let roots = explicit_roots(paths);
    let mut cache = shared_cache()
        .lock()
        .map_err(|_| UserFirmwareError::CacheUnavailable)?;
    find_in_roots(entry_id, catalogue, &roots, &mut cache).map(|discovery| discovery.found)
}

/// Default host roots for frontends and the legacy test wrapper. Discovery itself
/// always takes an explicit list and never changes the process environment.
pub fn default_search_roots() -> Vec<PathBuf> {
    let mut roots = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/user-roms")];
    if let Some(path) = std::env::var_os("UNEMUPC98_USER_ROMS_DIR") {
        roots.push(path.into());
    }
    roots
}

fn shared_cache() -> &'static Mutex<HashCache> {
    static CACHE: OnceLock<Mutex<HashCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashCache::default()))
}

fn explicit_roots(paths: &[PathBuf]) -> Vec<SearchRoot> {
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/user-roms");
    paths
        .iter()
        .map(|path| SearchRoot {
            path: path.clone(),
            external: path != &vendor,
            label: if path == &vendor {
                "vendor/user-roms"
            } else {
                "<UNEMUPC98_USER_ROMS_DIR>"
            },
        })
        .collect()
}

/// A full-file identity requested by a machine file. When a catalogue length is
/// known it is checked as well; SHA-256-only pins still hash the complete file.
#[derive(Clone, Debug)]
pub struct FirmwarePin {
    pub sha256: String,
    pub length: Option<u64>,
}

/// One chip declaration, retaining its original path and assembled byte lane.
#[derive(Clone, Debug)]
pub struct PinnedPart {
    pub part: ImagePart,
    pub pin: Option<FirmwarePin>,
}

/// Match a complete file, reading it in place and checking its full length.
pub fn matches_file(path: &Path, pin: &FirmwarePin) -> io::Result<bool> {
    if !pin.accepts_size(fs::metadata(path)?.len()) {
        return Ok(false);
    }
    let mut cache = shared_cache()
        .lock()
        .map_err(|_| io::Error::other("firmware hash cache unavailable"))?;
    Ok(cache.hash(path)? == pin.sha256)
}

fn parts_match(
    parts: &[PinnedPart],
    assembled: Option<&FirmwarePin>,
    cache: &mut HashCache,
) -> bool {
    let mut loaded = Vec::new();
    for part in parts {
        if cache
            .control
            .as_ref()
            .is_some_and(DiscoveryControl::is_cancelled)
        {
            return false;
        }
        let Ok(metadata) = fs::metadata(&part.part.path) else {
            return false;
        };
        if !metadata.is_file()
            || part
                .pin
                .as_ref()
                .map_or(metadata.len() > 1024 * 1024, |pin| {
                    !pin.accepts_size(metadata.len())
                })
        {
            return false;
        }
        if part.pin.as_ref().is_some_and(|pin| {
            !cache
                .hash(&part.part.path)
                .is_ok_and(|hash| hash == pin.sha256)
        }) {
            return false;
        }
        let Ok(file) = fs::File::open(&part.part.path) else {
            return false;
        };
        let mut bytes = Vec::new();
        if file
            .take(metadata.len() + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 != metadata.len()
        {
            return false;
        }
        loaded.push((part.part.lane, part.part.offset, bytes));
    }
    let Ok(bytes) = crate::interleave_parts(&loaded) else {
        return false;
    };
    assembled.is_none_or(|pin| {
        pin.length.is_none_or(|length| length == bytes.len() as u64)
            && format!("{:x}", Sha256::digest(&bytes)) == pin.sha256
    })
}

/// Resolve a pinned single image or chip layout without copying any files.
/// Correct declared paths win. Missing or mismatching parts are searched by
/// full-file pin; an assembled pin can also select a complete image instead.
/// Interleaving uses the same checked assembly helper as machine-file lowering.
pub fn locate_firmware(
    roots: &[PathBuf],
    image: Option<&Path>,
    assembled: Option<&FirmwarePin>,
    parts: &[PinnedPart],
) -> Result<Option<ImageSource>, UserFirmwareError> {
    let roots = roots
        .iter()
        .map(|path| FirmwareRoot::new(path, RootOrigin::CommandLine))
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    let found = locate_firmware_with_roots(&roots, image, assembled, parts, &mut warnings);
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    found
}

fn discover_user_firmware(
    entry_id: &str,
    catalogue: &[FirmwareEntry],
) -> Result<Discovery, UserFirmwareError> {
    let roots = explicit_roots(&default_search_roots());
    let mut cache = shared_cache()
        .lock()
        .map_err(|_| UserFirmwareError::CacheUnavailable)?;
    find_in_roots(entry_id, catalogue, &roots, &mut cache)
}

#[track_caller]
fn finish_requirement(
    entry_id: &str,
    discovery: Result<Discovery, UserFirmwareError>,
    require: Option<OsString>,
    output: &mut impl Write,
) -> Option<FoundUserFirmware> {
    let discovery = match discovery {
        Ok(discovery) => discovery,
        Err(error) => panic!("invalid user-supplied firmware configuration: {error}"),
    };
    if discovery.found.is_some() {
        return discovery.found;
    }
    let mut message = format!("SKIPPED: user-supplied firmware {entry_id} not found (set UNEMUPC98_USER_ROMS_DIR or see vendor/user-roms/README.md)");
    if !discovery.incomplete.is_empty() {
        message.push_str(&format!("; {}", discovery.incomplete.join("; ")));
    }
    if require.as_deref() == Some(std::ffi::OsStr::new("1")) {
        panic!("{message}");
    }
    // Direct writes remain visible in the ignored lane under libtest capture.
    if let Err(error) = writeln!(output, "{message}") {
        panic!(
            "cannot report missing user-supplied firmware: {}",
            error.kind()
        );
    }
    None
}

/// Shared skip/require policy. Use [`crate::require_user_firmware!`] to also
/// return early from the test. Discovery errors always fail with redacted names.
#[track_caller]
pub fn required_user_firmware(
    entry_id: &str,
    catalogue: &[FirmwareEntry],
) -> Option<FoundUserFirmware> {
    finish_requirement(
        entry_id,
        discover_user_firmware(entry_id, catalogue),
        std::env::var_os("UNEMUPC98_REQUIRE_USER_ROMS"),
        &mut io::stdout().lock(),
    )
}

/// Require a catalogue identity or print the shared skip line and return from
/// the test. Such tests must be ignored and have names starting with `user_rom_`.
#[macro_export]
macro_rules! require_user_firmware {
    ($entry_id:expr, $catalogue:expr) => {
        match $crate::test_support::required_user_firmware($entry_id, $catalogue) {
            Some(found) => found,
            None => return,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_support::TempDir, KnownPart, Lane};

    fn hash(bytes: &[u8]) -> &'static str {
        Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str())
    }

    fn catalogue() -> [FirmwareEntry; 1] {
        let part = |bytes: &[u8], lane, offset| KnownPart {
            label: "synthetic socket",
            part_number: "synthetic chip",
            lane,
            offset,
            size: bytes.len() as u64,
            sha256: hash(bytes),
        };
        let pair = Box::leak(Box::new([
            part(&[1, 3], Lane::Even, 0),
            part(&[2, 4], Lane::Odd, 0),
        ]));
        let halves = Box::leak(Box::new([
            part(&[1], Lane::Even, 0),
            part(&[2], Lane::Odd, 0),
            part(&[3], Lane::Even, 2),
            part(&[4], Lane::Odd, 2),
        ]));
        [FirmwareEntry {
            id: "synthetic",
            size: 4,
            sha256: hash(&[1, 2, 3, 4]),
            parts: Box::leak(Box::new([pair.as_slice(), halves.as_slice()])),
            ..crate::firmware::IBM_5170_1985_06_10
        }]
    }

    fn write(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn discover(roots: &[SearchRoot], cache: &mut HashCache) -> Option<FoundUserFirmware> {
        find_in_roots("synthetic", &catalogue(), roots, cache)
            .unwrap()
            .found
    }

    fn pair(root: &Path) -> Vec<PathBuf> {
        vec![
            write(root, "arbitrary/nested/renamed-a.dump", &[1, 3]),
            write(root, "elsewhere/renamed-b.bin", &[2, 4]),
        ]
    }

    #[test]
    fn user_firmware_parts_found_by_hash_nested_in_either_root() {
        for external in [false, true] {
            let repository = TempDir::new("firmware-repository");
            let dumps = TempDir::new("firmware-external");
            let roots = search_roots(
                repository.path(),
                external.then(|| dumps.path().as_os_str().to_owned()),
            );
            let selected = if external { 1 } else { 0 };
            let paths = pair(&roots[selected].path);
            let found = discover(&roots, &mut HashCache::default()).unwrap();
            assert_eq!(found.layout, UserFirmwareLayout::Parts(0));
            assert_eq!(
                found.source,
                ImageSource::Interleaved(vec![
                    ImagePart {
                        path: paths[0].clone(),
                        lane: Lane::Even,
                        offset: 0
                    },
                    ImagePart {
                        path: paths[1].clone(),
                        lane: Lane::Odd,
                        offset: 0
                    },
                ])
            );
            assert_eq!(
                found.files,
                vec![
                    format!("{}/arbitrary/nested/renamed-a.dump", roots[selected].label),
                    format!("{}/elsewhere/renamed-b.bin", roots[selected].label),
                ]
            );
            assert!(!found
                .files
                .join(" ")
                .contains(&dumps.path().display().to_string()));
        }
    }

    #[test]
    fn user_firmware_missing_part_is_absent() {
        let repository = TempDir::new("firmware-missing");
        let roots = search_roots(repository.path(), None);
        write(&roots[0].path, "renamed", &[1, 3]);
        assert!(discover(&roots, &mut HashCache::default()).is_none());
    }

    #[test]
    fn user_firmware_wrong_part_is_absent() {
        let repository = TempDir::new("firmware-wrong");
        let roots = search_roots(repository.path(), None);
        write(&roots[0].path, "renamed", &[1, 3]);
        write(&roots[0].path, "wrong", &[2, 5]);
        assert!(discover(&roots, &mut HashCache::default()).is_none());
    }

    #[test]
    fn user_firmware_assembled_image_found_in_either_root() {
        for external in [false, true] {
            let repository = TempDir::new("firmware-assembled");
            let dumps = TempDir::new("firmware-external");
            let roots = search_roots(repository.path(), external.then(|| dumps.path().into()));
            let selected = usize::from(external);
            let path = write(&roots[selected].path, "nested/unlabelled", &[1, 2, 3, 4]);
            let found = discover(&roots, &mut HashCache::default()).unwrap();
            assert_eq!(found.layout, UserFirmwareLayout::Assembled);
            assert_eq!(found.source, ImageSource::File(path));
        }
    }

    #[test]
    fn user_firmware_alternative_layout_preserves_offsets() {
        let repository = TempDir::new("firmware-halves");
        let roots = search_roots(repository.path(), None);
        for byte in 1..=4 {
            write(&roots[0].path, &format!("unknown/{byte}"), &[byte]);
        }
        let found = discover(&roots, &mut HashCache::default()).unwrap();
        assert_eq!(found.layout, UserFirmwareLayout::Parts(1));
        let ImageSource::Interleaved(parts) = found.source else {
            panic!("expected parts")
        };
        assert_eq!(
            parts.iter().map(|p| (p.lane, p.offset)).collect::<Vec<_>>(),
            vec![
                (Lane::Even, 0),
                (Lane::Odd, 0),
                (Lane::Even, 2),
                (Lane::Odd, 2)
            ]
        );
        let bytes = parts
            .iter()
            .map(|p| (p.lane, p.offset, fs::read(&p.path).unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(crate::interleave_parts(&bytes).unwrap(), [1, 2, 3, 4]);
    }

    #[test]
    fn user_firmware_vendor_root_wins_over_external_image() {
        let repository = TempDir::new("firmware-order");
        let dumps = TempDir::new("firmware-external");
        let roots = search_roots(repository.path(), Some(dumps.path().into()));
        pair(&roots[0].path);
        write(&roots[1].path, "image", &[1, 2, 3, 4]);
        let mut cache = HashCache::default();
        assert_eq!(
            discover(&roots, &mut cache).unwrap().layout,
            UserFirmwareLayout::Parts(0)
        );
        assert_eq!(cache.calls, 2);
    }

    #[test]
    fn user_firmware_parts_can_span_roots() {
        let repository = TempDir::new("firmware-spanning");
        let dumps = TempDir::new("firmware-external");
        let roots = search_roots(repository.path(), Some(dumps.path().into()));
        write(&roots[0].path, "chip", &[1, 3]);
        write(&roots[1].path, "chip", &[2, 4]);
        assert_eq!(
            discover(&roots, &mut HashCache::default()).unwrap().layout,
            UserFirmwareLayout::Parts(0)
        );
    }

    #[test]
    fn user_firmware_size_filter_and_process_cache_count_hashes() {
        let repository = TempDir::new("firmware-filter");
        let roots = search_roots(repository.path(), None);
        let paths = pair(&roots[0].path);
        write(&roots[0].path, "large-unrelated", &vec![7; 1_048_576]);
        let mut cache = HashCache::default();
        assert!(discover(&roots, &mut cache).is_some());
        assert_eq!(cache.calls, 2, "unrelated lengths must not be hashed");
        assert!(discover(&roots, &mut cache).is_some());
        assert_eq!(
            cache.calls,
            if cfg!(windows) { 4 } else { 2 },
            "repeat discovery follows the platform cache policy"
        );
        cache
            .hash(&paths[0].parent().unwrap().join("./renamed-a.dump"))
            .unwrap();
        assert_eq!(
            cache.calls,
            if cfg!(windows) { 5 } else { 2 },
            "canonical aliases follow the platform cache policy"
        );
    }

    #[test]
    fn user_firmware_unset_environment_searches_only_vendor() {
        let repository = TempDir::new("firmware-unset");
        let dumps = TempDir::new("firmware-external");
        pair(dumps.path());
        let roots = search_roots(repository.path(), None);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, repository.path().join("vendor/user-roms"));
        let mut cache = HashCache::default();
        assert!(discover(&roots, &mut cache).is_none());
        assert_eq!(cache.calls, 0);
    }

    #[test]
    fn user_firmware_skip_line_is_exact_and_require_only_accepts_one() {
        for require in [None, Some("0".into()), Some("true".into())] {
            let mut output = Vec::new();
            assert!(finish_requirement(
                "synthetic",
                Ok(Discovery::default()),
                require,
                &mut output
            )
            .is_none());
            assert_eq!(String::from_utf8(output).unwrap(), "SKIPPED: user-supplied firmware synthetic not found (set UNEMUPC98_USER_ROMS_DIR or see vendor/user-roms/README.md)\n");
        }
    }

    #[test]
    fn user_firmware_require_one_panics_with_exact_skip_text() {
        let mut output = Vec::new();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finish_requirement(
                "synthetic",
                Ok(Discovery::default()),
                Some("1".into()),
                &mut output,
            );
        }))
        .expect_err("require=1 must fail when absent");
        assert_eq!(panic.downcast_ref::<String>().unwrap(), "SKIPPED: user-supplied firmware synthetic not found (set UNEMUPC98_USER_ROMS_DIR or see vendor/user-roms/README.md)");
        assert!(output.is_empty());
    }

    #[test]
    fn user_firmware_present_is_returned_even_when_required() {
        let found = FoundUserFirmware {
            source: ImageSource::File("synthetic".into()),
            layout: UserFirmwareLayout::Assembled,
            files: vec!["synthetic".into()],
            redactions: Vec::new(),
        };
        let mut output = Vec::new();
        assert!(finish_requirement(
            "synthetic",
            Ok(Discovery {
                found: Some(found),
                incomplete: Vec::new()
            }),
            Some("1".into()),
            &mut output
        )
        .is_some());
        assert!(output.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn user_firmware_file_symlink_is_found_and_hashed_once() {
        let repository = TempDir::new("firmware-symlink");
        let dumps = TempDir::new("firmware-external");
        let roots = search_roots(repository.path(), None);
        let path = write(dumps.path(), "image", &[1, 2, 3, 4]);
        fs::create_dir_all(&roots[0].path).unwrap();
        let link = roots[0].path.join("linked-file");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        std::os::unix::fs::symlink(&path, roots[0].path.join("second-link")).unwrap();
        let mut cache = HashCache::default();
        assert_eq!(
            discover(&roots, &mut cache).unwrap().source,
            ImageSource::File(link)
        );
        assert_eq!(cache.calls, 1);
        assert!(discover(&roots, &mut cache).is_some());
        assert_eq!(cache.calls, 1);
    }

    #[cfg(unix)]
    #[test]
    fn user_firmware_directory_symlink_is_searched() {
        let repository = TempDir::new("firmware-directory-link");
        let dumps = TempDir::new("firmware-external");
        pair(dumps.path());
        let roots = search_roots(repository.path(), None);
        fs::create_dir_all(&roots[0].path).unwrap();
        std::os::unix::fs::symlink(dumps.path(), roots[0].path.join("linked-directory")).unwrap();
        assert_eq!(
            discover(&roots, &mut HashCache::default()).unwrap().layout,
            UserFirmwareLayout::Parts(0)
        );
    }

    #[cfg(unix)]
    #[test]
    fn user_firmware_directory_loop_is_visited_once() {
        let repository = TempDir::new("firmware-loop");
        let roots = search_roots(repository.path(), None);
        write(&roots[0].path, "chip", &[1, 3]);
        std::os::unix::fs::symlink(&roots[0].path, roots[0].path.join("loop")).unwrap();
        let mut cache = HashCache::default();
        let discovery = find_in_roots("synthetic", &catalogue(), &roots, &mut cache).unwrap();
        assert!(discovery.found.is_none());
        assert_eq!(discovery.incomplete, ["layout 0: 1 of 2 parts found"]);
        assert_eq!(cache.calls, 1);
        let mut visited = BTreeSet::new();
        let mut candidates = Vec::new();
        scan(
            &roots[0],
            &roots[0].path,
            &BTreeSet::from([2]),
            &mut cache,
            &mut candidates,
            &mut visited,
        )
        .unwrap();
        assert_eq!(visited.len(), 1);
        assert_eq!(candidates.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn user_firmware_symlinked_external_root_works_without_trailing_slash() {
        let repository = TempDir::new("firmware-root-link");
        let dumps = TempDir::new("firmware-external");
        pair(dumps.path());
        let link = repository.path().join("root-link");
        std::os::unix::fs::symlink(dumps.path(), &link).unwrap();
        let roots = search_roots(repository.path(), Some(link.into()));
        let found = discover(&roots, &mut HashCache::default()).unwrap();
        assert_eq!(found.layout, UserFirmwareLayout::Parts(0));
        assert!(found
            .files
            .iter()
            .all(|name| name.starts_with("<UNEMUPC98_USER_ROMS_DIR>/")));
    }

    fn assert_root_configuration_failure(roots: &[SearchRoot], reason: &str) {
        for require in [None, Some("1".into())] {
            let mut output = Vec::new();
            let result = find_in_roots("synthetic", &catalogue(), roots, &mut HashCache::default());
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                finish_requirement("synthetic", result, require, &mut output);
            }))
            .expect_err("invalid root must fail instead of skipping");
            assert_eq!(panic.downcast_ref::<String>().unwrap(), &format!("invalid user-supplied firmware configuration: <UNEMUPC98_USER_ROMS_DIR>: {reason}"));
            assert!(output.is_empty());
        }
    }

    #[test]
    fn user_firmware_missing_external_root_is_configuration_failure() {
        let repository = TempDir::new("firmware-root-missing");
        let roots = search_roots(
            repository.path(),
            Some(repository.path().join("absent").into()),
        );
        assert_root_configuration_failure(&roots, "not found");
        pair(&roots[0].path);
        assert_root_configuration_failure(&roots, "not found");
    }

    #[test]
    fn user_firmware_file_external_root_is_configuration_failure() {
        let repository = TempDir::new("firmware-root-file");
        let file = write(repository.path(), "not-a-directory", &[1, 2, 3, 4]);
        let roots = search_roots(repository.path(), Some(file.into()));
        assert_root_configuration_failure(&roots, "is not a directory");
    }

    #[test]
    fn user_firmware_relative_or_empty_external_root_is_configuration_failure() {
        let repository = TempDir::new("firmware-root-relative");
        for path in [OsString::from("../relative-dumps"), OsString::new()] {
            let roots = search_roots(repository.path(), Some(path));
            assert_root_configuration_failure(&roots, "must be an absolute path");
        }
    }

    #[test]
    fn user_firmware_partial_layouts_are_reported_in_skip_and_require_text() {
        let repository = TempDir::new("firmware-partial");
        let roots = search_roots(repository.path(), None);
        write(&roots[0].path, "secret-part-name", &[1, 3]);
        write(&roots[0].path, "other-secret-name", &[2]);
        let expected = "SKIPPED: user-supplied firmware synthetic not found (set UNEMUPC98_USER_ROMS_DIR or see vendor/user-roms/README.md); layout 0: 1 of 2 parts found; layout 1: 1 of 4 parts found";
        let mut output = Vec::new();
        let discovery = find_in_roots("synthetic", &catalogue(), &roots, &mut HashCache::default());
        assert!(finish_requirement("synthetic", discovery, None, &mut output).is_none());
        assert_eq!(String::from_utf8(output).unwrap(), format!("{expected}\n"));
        let discovery = find_in_roots("synthetic", &catalogue(), &roots, &mut HashCache::default());
        let panic = std::panic::catch_unwind(|| {
            finish_requirement("synthetic", discovery, Some("1".into()), &mut Vec::new());
        })
        .expect_err("required partial layout must fail");
        assert_eq!(panic.downcast_ref::<String>().unwrap(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn user_firmware_found_redacts_downstream_errors_and_canonical_root() {
        let repository = TempDir::new("firmware-redaction");
        let dumps = TempDir::new("firmware-external");
        let target = dumps.path().join("quoted-\"-dumps");
        pair(&target);
        let link = repository.path().join("external-alias");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let roots = search_roots(repository.path(), Some(link.clone().into()));
        let found = discover(&roots, &mut HashCache::default()).unwrap();
        let canonical = target.canonicalize().unwrap();
        for root in [&link, &canonical] {
            for message in [
                format!("could not load {}/chip", root.display()),
                format!("could not load {:?}", root.join("chip")),
            ] {
                let redacted = found.redact(&message);
                assert!(redacted.contains("<UNEMUPC98_USER_ROMS_DIR>/chip"));
                assert!(!redacted.contains(&root.display().to_string()));
                let panic = std::panic::catch_unwind(|| {
                    found.expect_redacted(Err::<(), _>(message.clone()), "loading firmware");
                })
                .expect_err("expected redacted failure");
                assert_eq!(
                    panic.downcast_ref::<String>().unwrap(),
                    &format!("loading firmware: {redacted}")
                );
            }
        }
        assert_eq!(found.expect_redacted(Ok::<_, String>(7), "success"), 7);
        assert_eq!(
            found.expect_err_redacted(Err::<(), _>("expected"), "failure"),
            "expected"
        );
        let panic = std::panic::catch_unwind(|| {
            found.expect_err_redacted(
                Ok::<_, String>(&found.source),
                &format!("loading {}", link.display()),
            );
        })
        .expect_err("unexpected success must not print private values");
        assert_eq!(
            panic.downcast_ref::<String>().unwrap(),
            "loading <UNEMUPC98_USER_ROMS_DIR>: expected an error"
        );
    }

    #[test]
    fn user_firmware_errors_redact_private_root() {
        let repository = TempDir::new("firmware-errors");
        let dumps = TempDir::new("firmware-external");
        let roots = search_roots(repository.path(), Some(dumps.path().into()));
        let error = roots[1].error(
            &dumps.path().join("nested/chip"),
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                dumps.path().display().to_string(),
            ),
        );
        assert_eq!(error.to_string(), "cannot scan user-supplied firmware at <UNEMUPC98_USER_ROMS_DIR>/nested/chip: permission denied");
        let mut cache = HashCache::default();
        assert!(matches!(
            find_in_roots("typo", &catalogue(), &roots, &mut cache),
            Err(UserFirmwareError::UnknownEntry(_))
        ));
        assert!(matches!(
            find_in_roots(
                crate::KISELEV_XI8088.id,
                &[crate::KISELEV_XI8088],
                &roots,
                &mut cache
            ),
            Err(UserFirmwareError::NotUserSupplied(_))
        ));
    }
    #[test]
    fn explicit_roots_resolve_single_and_split_pins_without_copying_files() {
        let temp = TempDir::new("firmware-explicit-pins");
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("wrong"), [9, 9]).unwrap();
        fs::write(a.join("even"), [1, 3]).unwrap();
        fs::write(b.join("odd"), [2, 4]).unwrap();
        let whole = FirmwarePin {
            sha256: hash(&[1, 2, 3, 4]).into(),
            length: Some(4),
        };
        let part = |name, lane, bytes: &[u8]| PinnedPart {
            part: ImagePart {
                path: temp.path().join(name),
                lane,
                offset: 0,
            },
            pin: Some(FirmwarePin {
                sha256: hash(bytes).into(),
                length: Some(bytes.len() as u64),
            }),
        };
        let parts = [
            part("absent-even", Lane::Even, &[1, 3]),
            part("absent-odd", Lane::Odd, &[2, 4]),
        ];
        let source = locate_firmware(&[a.clone(), b.clone()], None, Some(&whole), &parts)
            .unwrap()
            .unwrap();
        let ImageSource::Interleaved(found) = source else {
            panic!("split source expected");
        };
        assert_eq!(found[0].path, a.join("even"));
        assert_eq!(found[1].path, b.join("odd"));
        assert!(!parts[0].part.path.exists());
        assert!(!parts[1].part.path.exists());
        fs::write(b.join("assembled"), [1, 2, 3, 4]).unwrap();
        assert_eq!(
            locate_firmware(
                std::slice::from_ref(&b),
                Some(&temp.path().join("missing")),
                Some(&whole),
                &[]
            )
            .unwrap(),
            Some(ImageSource::File(b.join("assembled")))
        );
        assert_eq!(
            locate_firmware(
                &[temp.path().join("absent-root")],
                Some(&b.join("assembled")),
                Some(&whole),
                &[]
            )
            .unwrap(),
            Some(ImageSource::File(b.join("assembled")))
        );
        let wrong_length = FirmwarePin {
            sha256: whole.sha256.clone(),
            length: Some(5),
        };
        assert!(locate_firmware(&[b], None, Some(&wrong_length), &[])
            .unwrap()
            .is_none());
    }

    #[test]
    fn cache_rechecks_identity_after_same_size_replacement_with_restored_mtime() {
        let temp = TempDir::new("firmware-cache-replacement");
        let path = temp.path().join("rom");
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut cache = HashCache::default();
        let first = cache.hash(&path).unwrap();
        let replacement = temp.path().join("replacement");
        fs::write(&replacement, [4, 3, 2, 1]).unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        fs::rename(replacement, &path).unwrap();
        assert_ne!(cache.hash(&path).unwrap(), first);
    }

    #[test]
    fn cached_hash_is_invalidated_when_a_file_changes() {
        let temp = TempDir::new("firmware-cache-change");
        let path = temp.path().join("rom");
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let pin = FirmwarePin {
            sha256: hash(&[1, 2, 3, 4]).into(),
            length: Some(4),
        };
        assert!(matches_file(&path, &pin).unwrap());
        fs::write(&path, [4, 3, 2, 1]).unwrap();
        assert!(!matches_file(&path, &pin).unwrap());
    }

    #[test]
    fn catalogue_discovery_accepts_explicit_roots() {
        let temp = TempDir::new("firmware-public-roots");
        let path = write(temp.path(), "renamed", &[1, 2, 3, 4]);
        let found = find_catalogue_in_roots(
            "synthetic",
            &catalogue(),
            &[temp.path().join("unmounted"), temp.path().into()],
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.source, ImageSource::File(path));
    }
}
