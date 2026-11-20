// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local, metadata-validated host hash indexes and cancellable scan progress.
//! No guest clock or ROM bytes are persisted; firmware-and-machine-classes.md §3.5.

use super::*;
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RESULTS: usize = 256;

/// Host discovery phase; listing has no known denominator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiscoveryPhase {
    #[default]
    Idle,
    Listing,
    Hashing,
    Loading,
    Complete,
    Cancelled,
}

/// A bounded snapshot safe to poll from the UI without waiting for host I/O.
#[derive(Clone, Debug, Default)]
pub struct DiscoveryProgress {
    pub phase: DiscoveryPhase,
    pub listed: usize,
    /// Directory listing attempts, including empty directories.
    pub folder_listings: usize,
    pub size_skipped: usize,
    pub candidates: usize,
    /// Candidates checked, including metadata-validated cache hits.
    pub checked: usize,
    /// Discovery SHA-256 reads only; excludes cache hits and loader verification.
    pub files_read: usize,
    pub matches: usize,
    pub result_count: usize,
    /// All found labels plus at most 256 skipped labels and a size-skip summary.
    pub results: Vec<String>,
}

#[derive(Default)]
struct DiscoveryState {
    progress: DiscoveryProgress,
    found: Vec<String>,
}

/// Shared cancellation and progress for one host worker, outside guest execution.
#[derive(Clone, Default)]
pub struct DiscoveryControl {
    cancelled: Arc<AtomicBool>,
    progress: Arc<Mutex<DiscoveryState>>,
}

impl DiscoveryControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.update(|progress| {
            if progress.phase != DiscoveryPhase::Loading {
                progress.phase = DiscoveryPhase::Cancelled;
            }
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> DiscoveryProgress {
        self.progress
            .lock()
            .map(|state| {
                let mut progress = state.progress.clone();
                if progress.size_skipped > 0 {
                    progress.results.truncate(MAX_RESULTS - 1);
                    progress.results.push(format!(
                        "{} files skipped (size is not a candidate)",
                        progress.size_skipped
                    ));
                    progress.result_count += 1;
                }
                progress.results.splice(0..0, state.found.iter().cloned());
                progress
            })
            .unwrap_or_default()
    }

    pub(super) fn update(&self, update: impl FnOnce(&mut DiscoveryProgress)) {
        if let Ok(mut progress) = self.progress.lock() {
            update(&mut progress.progress);
        }
    }

    /// The subsequent machine loader performs reads that cannot be interrupted.
    pub fn loading(&self) {
        self.update(|progress| progress.phase = DiscoveryPhase::Loading);
    }

    pub(super) fn found(&self, text: String) {
        if let Ok(mut state) = self.progress.lock() {
            state.progress.result_count += 1;
            state.found.push(text);
        }
    }

    pub(super) fn result(&self, text: String) {
        self.update(|progress| {
            progress.result_count += 1;
            if progress.results.len() < MAX_RESULTS {
                progress.results.push(text);
            }
        });
    }
}

thread_local! {
    // The machine-file resolver calls discovery separately for each ROM. A scoped
    // worker session shares its index without adding host settings to machine data.
    static SESSION: RefCell<Option<HashCache>> = const { RefCell::new(None) };
}

pub(super) fn with_cache<T>(
    operation: impl FnOnce(&mut HashCache) -> Result<T, UserFirmwareError>,
) -> Result<T, UserFirmwareError> {
    SESSION.with(|session| {
        if let Some(cache) = session.borrow_mut().as_mut() {
            return operation(cache);
        }
        let mut cache = shared_scan()?;
        operation(&mut cache)
    })
}

/// Share the union of ROM pins and one listing across a machine-file check.
/// Frontends with an existing worker session keep its cache and cancellation flag.
pub fn with_firmware_pins<T>(pins: &[FirmwarePin], operation: impl FnOnce() -> T) -> T {
    let active = SESSION.with(|session| session.borrow().is_some());
    let operation = || {
        SESSION.with(|session| {
            if let Some(cache) = session.borrow_mut().as_mut() {
                cache.pins = pins.to_vec();
            }
        });
        operation()
    };
    if active {
        operation()
    } else {
        with_discovery_session(None, false, &DiscoveryControl::default(), operation)
    }
}

/// Whether a hash taken at `hashed_at` cannot be trusted on matching metadata
/// later. A rewrite that lands in the same filesystem clock step keeps size,
/// inode and both timestamps, and a step can be as long as two seconds (FAT
/// modification times). Only when the modification and change times are both
/// older than the hash by more than that must any later write alter them; git
/// applies the same rule to "racily clean" index entries. Racy hashes are never
/// written to the index and are read again by every later scan.
pub(super) fn racy_timestamp(key: &HashKey, hashed_at: SystemTime) -> bool {
    let window = Duration::from_secs(2);
    let recent = |time: SystemTime| {
        hashed_at
            .duration_since(time)
            .map_or(true, |age| age <= window)
    };
    #[cfg(unix)]
    let change = if key.3 .0 >= 0 && (0..1_000_000_000).contains(&key.3 .1) {
        UNIX_EPOCH.checked_add(Duration::new(key.3 .0 as u64, key.3 .1 as u32))
    } else {
        None
    };
    #[cfg(not(unix))]
    let change: Option<SystemTime> = None;
    recent(key.2) || change.is_some_and(recent)
}

/// A desktop session retains folder listings and verified hashes between host jobs.
#[derive(Clone, Default)]
pub struct DiscoverySession(Arc<Mutex<Option<(HashCache, bool)>>>);

impl DiscoverySession {
    /// Serialize host checks while keeping their listing outside guest state.
    pub fn run<T>(
        &self,
        path: Option<&Path>,
        rescan: bool,
        control: &DiscoveryControl,
        operation: impl FnOnce() -> T,
    ) -> T {
        let mut retained = self.0.lock().unwrap_or_else(|error| error.into_inner());
        run_session(path, rescan, control, Some(&mut retained), operation)
    }
}

/// Run existing resolver calls with one explicit local cache and cancellation flag.
/// `None` disables disk persistence (including in tests). Invalid indexes are
/// ignored. A cancelled run retains the old index plus completed, validated hashes.
/// Cache errors are nonfatal and appear as path-free progress diagnostics.
pub fn with_discovery_session<T>(
    path: Option<&Path>,
    rescan: bool,
    control: &DiscoveryControl,
    operation: impl FnOnce() -> T,
) -> T {
    run_session(path, rescan, control, None, operation)
}

fn run_session<T>(
    path: Option<&Path>,
    rescan: bool,
    control: &DiscoveryControl,
    mut retained: Option<&mut Option<(HashCache, bool)>>,
    operation: impl FnOnce() -> T,
) -> T {
    let previous = retained.as_mut().and_then(|cache| cache.take());
    let path = path.filter(|_| !cfg!(windows));
    if let Some(path) = path {
        sweep_temps(path, SystemTime::now());
    }
    let (mut cache, writable) = if let Some(previous) = previous {
        previous
    } else {
        match path.map(load_result) {
            Some(Ok(Some(cache))) => (cache, true),
            Some(Ok(None)) => (
                HashCache {
                    dirty: true,
                    ..Default::default()
                },
                true,
            ),
            Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => (
                HashCache {
                    dirty: true,
                    ..Default::default()
                },
                true,
            ),
            Some(Err(_)) => {
                control.result("Local firmware index: skipped (cannot read cache)".into());
                (HashCache::default(), false)
            }
            None => (HashCache::default(), false),
        }
    };
    cache.begin_scan();
    cache.retained_roots = cache
        .listings
        .iter()
        .map(|(root, _, _, _)| root.clone())
        .collect();
    cache.stale_roots.clear();
    cache.listed_directories.clear();
    control.update(|progress| progress.phase = DiscoveryPhase::Listing);
    if rescan {
        cache.bypass = cache.hashes.keys().map(|key| key.0.clone()).collect();
        cache.listings.clear();
        cache.retained_roots.clear();
        cache.seen.clear();
        cache.listed_directories.clear();
    }
    cache.candidates.clear();
    cache.checked_candidates.clear();
    cache.matched_candidates.clear();
    cache.reported_mismatches.clear();
    cache.pins.clear();
    cache.control = Some(control.clone());
    struct Restore(Option<HashCache>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SESSION.with(|session| *session.borrow_mut() = self.0.take());
        }
    }
    let restore = Restore(SESSION.with(|session| session.replace(Some(cache))));
    let result = operation();
    let cache = SESSION.with(|session| session.take());
    if let Some(mut cache) = cache {
        if !control.is_cancelled() {
            let before = cache.hashes.len();
            // Only a successful listing establishes which direct children vanished.
            cache.hashes.retain(|key, _| {
                cache.seen.contains(&key.0)
                    || key
                        .0
                        .parent()
                        .is_none_or(|parent| !cache.listed_directories.contains(parent))
            });
            cache.dirty |= before != cache.hashes.len();
        }
        while cache.hashes.len() > MAX_ENTRIES {
            let old = cache
                .hashes
                .keys()
                .find(|key| !cache.seen.contains(&key.0))
                .or_else(|| cache.hashes.keys().next())
                .cloned();
            if let Some(old) = old {
                cache.hashes.remove(&old);
                cache.dirty |= !cache.unpersistable.contains(&old.0);
            }
        }
        if writable && cache.dirty {
            if let Some(path) = path {
                match save(path, &cache) {
                    Ok(()) => cache.dirty = false,
                    Err(_) => {
                        control.result("Local firmware index: skipped (cannot save cache)".into())
                    }
                }
            }
        }
        let indexed = cache
            .hashes
            .keys()
            .map(|key| key.0.clone())
            .collect::<BTreeSet<_>>();
        cache.seen.retain(|path| indexed.contains(path));
        cache.listed_directories.clear();
        cache.control = None;
        cache.bypass.clear();
        if let Some(retained) = retained {
            *retained = Some((cache, writable));
        }
    }
    control.update(|progress| {
        progress.phase = if control.is_cancelled() {
            DiscoveryPhase::Cancelled
        } else {
            DiscoveryPhase::Complete
        };
    });
    drop(restore);
    result
}

/// Platform cache path; callers choose whether to enable persistence explicitly.
///
/// The directory carries the project's current name. An index left under its
/// former name is not copied: the index only saves rehashing, so the first
/// discovery after the rename hashes the ROM folders again and writes a fresh one.
pub fn firmware_cache_path() -> Option<PathBuf> {
    platform_path(
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        cfg!(target_os = "macos"),
        cfg!(windows),
    )
}

fn platform_path(
    home: Option<PathBuf>,
    xdg: Option<PathBuf>,
    _local: Option<PathBuf>,
    macos: bool,
    windows: bool,
) -> Option<PathBuf> {
    let root = if macos {
        home.map(|home| home.join("Library/Caches"))
    } else if windows {
        return None;
    } else {
        xdg.filter(|path| path.is_absolute())
            .or_else(|| home.map(|home| home.join(".cache")))
    };
    root.map(|root| root.join("unemupc/firmware-index.v2"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
fn load(path: &Path) -> Option<HashCache> {
    load_result(path).ok().flatten()
}

fn load_result(path: &Path) -> io::Result<Option<HashCache>> {
    let mut text = String::new();
    match fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)
    {
        Ok(_) => Ok(decode(&text)),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(None),
        Err(error) => Err(error),
    }
}

fn decode(text: &str) -> Option<HashCache> {
    if text.len() as u64 > MAX_BYTES {
        return None;
    }
    let mut lines = text.lines();
    if lines.next()? != "unemupc-firmware-index 2" {
        return None;
    }
    let mut cache = HashCache::default();
    for line in lines {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 10 || cache.hashes.len() >= MAX_ENTRIES {
            return None;
        }
        let path = PathBuf::from(String::from_utf8(unhex(fields[0])?).ok()?);
        if !path.is_absolute() || fields[9].len() != 64 || unhex(fields[9])?.len() != 32 {
            return None;
        }
        let nanos: u32 = fields[3].parse().ok()?;
        if nanos >= 1_000_000_000 {
            return None;
        }
        let duration = Duration::new(fields[2].parse().ok()?, nanos);
        let modified = match fields[4] {
            "+" => UNIX_EPOCH.checked_add(duration)?,
            "-" => UNIX_EPOCH.checked_sub(duration)?,
            _ => return None,
        };
        let key = (
            path,
            fields[1].parse().ok()?,
            modified,
            (fields[5].parse().ok()?, fields[6].parse().ok()?),
            (fields[7].parse().ok()?, fields[8].parse().ok()?),
        );
        let hash = fields[9].to_ascii_lowercase();
        cache.dirty |= hash != fields[9];
        cache.hashes.insert(key, hash);
    }
    Some(cache)
}

fn sweep_temps(path: &Path, now: SystemTime) {
    let Some(parent) = path.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(".firmware-index-") || !name.ends_with(".tmp") {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > Duration::from_secs(300))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn save(path: &Path, cache: &HashCache) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut hashes = match load_result(path) {
        Ok(Some(latest)) => latest.hashes,
        Ok(None) => BTreeMap::new(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error),
    };
    let own_paths = cache
        .hashes
        .keys()
        .map(|key| &key.0)
        .collect::<BTreeSet<_>>();
    hashes.retain(|key, _| {
        !own_paths.contains(&key.0)
            && !cache.unpersistable.contains(&key.0)
            && !(key
                .0
                .parent()
                .is_some_and(|parent| cache.listed_directories.contains(parent))
                && !cache.seen.contains(&key.0)
                && fs::metadata(&key.0).is_err_and(|error| error.kind() == io::ErrorKind::NotFound))
    });
    hashes.extend(
        cache
            .hashes
            .iter()
            .map(|(key, hash)| (key.clone(), hash.clone())),
    );
    while hashes.len() > MAX_ENTRIES {
        let old = hashes
            .keys()
            .find(|key| !cache.seen.contains(&key.0))
            .or_else(|| hashes.keys().next())
            .cloned();
        if let Some(old) = old {
            hashes.remove(&old);
        }
    }
    let mut text = String::from("unemupc-firmware-index 2\n");
    for (key, hash) in &hashes {
        if cache.unpersistable.contains(&key.0) {
            continue;
        }
        let Some(name) = key.0.to_str().filter(|name| name.len() <= 16384) else {
            continue;
        };
        let (sign, duration) = match key.2.duration_since(UNIX_EPOCH) {
            Ok(duration) => ('+', duration),
            Err(error) => ('-', error.duration()),
        };
        let line = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            hex(name.as_bytes()),
            key.1,
            duration.as_secs(),
            duration.subsec_nanos(),
            sign,
            key.3 .0,
            key.3 .1,
            key.4 .0,
            key.4 .1,
            hash
        );
        if (text.len() + line.len()) as u64 > MAX_BYTES {
            break;
        }
        text.push_str(&line);
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let (temp, mut file) = loop {
        let temp = parent.join(format!(
            ".firmware-index-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use assembly::test_support::TempDir;

    fn test_session<T>(
        path: Option<&Path>,
        rescan: bool,
        control: &DiscoveryControl,
        operation: impl FnOnce() -> T,
    ) -> T {
        with_discovery_session(path, rescan, control, || {
            with_cache(|cache| {
                cache.hash_time = Some(SystemTime::now() + Duration::from_secs(10));
                Ok(())
            })
            .unwrap();
            operation()
        })
    }

    fn discover(root: &Path, path: &Path, rescan: bool, control: &DiscoveryControl) {
        let pin = FirmwarePin {
            sha256: format!("{:x}", Sha256::digest([1, 2, 3, 4])),
            length: Some(4),
        };
        test_session(Some(path), rescan, control, || {
            locate_firmware_with_roots(
                &[FirmwareRoot::new(root, RootOrigin::Preferences)],
                None,
                Some(&pin),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        });
    }

    #[test]
    fn retained_runs_rehash_racy_entries_once_per_discovery_run() {
        let temp = TempDir::new("racy-retained-index");
        let file = temp.path().join("rom");
        fs::write(&file, [1; 512]).unwrap();
        let session = DiscoverySession::default();
        for _ in 0..2 {
            let control = DiscoveryControl::default();
            session.run(None, false, &control, || {
                with_cache(|cache| {
                    cache.hash_time = Some(UNIX_EPOCH);
                    for _ in 0..2 {
                        cache.hash(&file).unwrap();
                    }
                    assert!(cache.unpersistable.contains(&file.canonicalize().unwrap()));
                    Ok(())
                })
                .unwrap();
            });
            assert_eq!(control.snapshot().files_read, 1);
        }
    }

    #[test]
    fn retained_save_merges_entries_written_by_another_instance() {
        let temp = TempDir::new("merged-retained-index");
        let index = temp.path().join("index");
        let session = DiscoverySession::default();
        let hash = |file: &Path| {
            with_cache(|cache| {
                cache.hash_time = Some(SystemTime::now() + Duration::from_secs(10));
                cache.hash(file).unwrap();
                Ok(())
            })
            .unwrap()
        };
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let third = temp.path().join("third");
        for file in [&first, &second, &third] {
            fs::write(file, [1; 512]).unwrap();
        }
        session.run(Some(&index), false, &DiscoveryControl::default(), || {
            hash(&first)
        });
        test_session(Some(&index), false, &DiscoveryControl::default(), || {
            hash(&second)
        });
        session.run(Some(&index), false, &DiscoveryControl::default(), || {
            hash(&third)
        });
        let stored = load(&index).unwrap();
        assert_eq!(stored.hashes.len(), 3);
        for file in [&first, &second, &third] {
            assert!(stored
                .hashes
                .keys()
                .any(|key| key.0 == file.canonicalize().unwrap()));
        }
    }

    #[test]
    fn desktop_session_retains_listing_hashes_and_does_not_rewrite_a_clean_index() {
        let temp = TempDir::new("desktop-firmware-session");
        let root = temp.path().join("roms");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("rom"), [1; 512]).unwrap();
        let path = temp.path().join("index");
        let roots = [FirmwareRoot::new(&root, RootOrigin::Preferences)];
        let pin = FirmwarePin {
            sha256: format!("{:x}", Sha256::digest([1; 512])),
            length: Some(512),
        };
        let session = DiscoverySession::default();
        let discover = |control: &DiscoveryControl| {
            session.run(Some(&path), false, control, || {
                with_cache(|cache| {
                    cache.hash_time = Some(SystemTime::now() + Duration::from_secs(10));
                    Ok(())
                })
                .unwrap();
                assert!(
                    locate_firmware_with_roots(&roots, None, Some(&pin), &[], &mut Vec::new())
                        .unwrap()
                        .is_some()
                );
            })
        };
        let first = DiscoveryControl::default();
        discover(&first);
        assert_eq!(first.snapshot().folder_listings, 1);
        assert_eq!(first.snapshot().files_read, 1);
        let before = hash_key(path.clone(), &fs::metadata(&path).unwrap()).unwrap();
        let second = DiscoveryControl::default();
        discover(&second);
        assert_eq!(second.snapshot().folder_listings, 0);
        assert_eq!(second.snapshot().files_read, 0);
        assert_eq!(
            hash_key(path.clone(), &fs::metadata(&path).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn desktop_session_preserves_an_unreadable_index_across_checks() {
        let temp = TempDir::new("desktop-unreadable-index");
        let path = temp.path().join("index");
        fs::create_dir(&path).unwrap();
        let session = DiscoverySession::default();
        for _ in 0..2 {
            let control = DiscoveryControl::default();
            session.run(Some(&path), false, &control, || {
                with_cache(|cache| {
                    cache.dirty = true;
                    Ok(())
                })
                .unwrap();
            });
            assert!(path.is_dir());
            assert!(!control
                .snapshot()
                .results
                .iter()
                .any(|line| line.contains("cannot save")));
        }
    }

    #[test]
    fn persistent_discovery_reuses_hashes_revalidates_metadata_and_prunes_deleted_files() {
        let temp = TempDir::new("persistent-firmware");
        let root = temp.path().join("roms");
        fs::create_dir(&root).unwrap();
        let file = root.join("synthetic.bin");
        fs::write(&file, [1, 2, 3, 4]).unwrap();
        let path = temp.path().join("cache/firmware-index.v2");
        let first = DiscoveryControl::default();
        discover(&root, &path, false, &first);
        assert_eq!(first.snapshot().files_read, 1);
        assert_eq!(load(&path).unwrap().hashes.len(), 1);
        let second = DiscoveryControl::default();
        discover(&root, &path, false, &second);
        assert_eq!(second.snapshot().files_read, 0);
        assert_eq!(second.snapshot().matches, 1);
        let handle = fs::OpenOptions::new().write(true).open(&file).unwrap();
        handle
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(12345)))
            .unwrap();
        let touched = DiscoveryControl::default();
        discover(&root, &path, false, &touched);
        assert_eq!(touched.snapshot().files_read, 1);
        fs::remove_file(&file).unwrap();
        let removed = DiscoveryControl::default();
        discover(&root, &path, false, &removed);
        assert_eq!(removed.snapshot().matches, 0);
        assert!(load(&path).unwrap().hashes.is_empty());
    }

    #[test]
    fn corrupt_unknown_and_oversized_indexes_are_rebuilt_and_rescan_bypasses_hits() {
        let temp = TempDir::new("invalid-firmware-cache");
        let root = temp.path().join("roms");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("synthetic.bin"), [1, 2, 3, 4]).unwrap();
        let path = temp.path().join("index");
        for text in [
            "corrupt",
            "unemupc-firmware-index 99\n",
            "unemupc-firmware-index 2\ninvalid\n",
        ] {
            fs::write(&path, text).unwrap();
            let control = DiscoveryControl::default();
            discover(&root, &path, false, &control);
            assert_eq!(control.snapshot().files_read, 1);
            assert_eq!(load(&path).unwrap().hashes.len(), 1);
        }
        let control = DiscoveryControl::default();
        discover(&root, &path, true, &control);
        assert_eq!(control.snapshot().files_read, 1);
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(load(&path).is_none());
        discover(&root, &path, false, &DiscoveryControl::default());
        assert_eq!(load(&path).unwrap().hashes.len(), 1);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn cancellation_keeps_completed_cache_and_interrupts_between_files() {
        let temp = TempDir::new("cancel-firmware-cache");
        let root = temp.path().join("roms");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("first.bin"), [1, 2, 3, 4]).unwrap();
        fs::write(root.join("second.bin"), [4, 3, 2, 1]).unwrap();
        let path = temp.path().join("index");
        let control = DiscoveryControl::default();
        test_session(Some(&path), false, &control, || {
            with_cache(|cache| {
                cache.cancel_after_hash = true;
                Ok(())
            })
            .unwrap();
            let pin = FirmwarePin {
                sha256: format!("{:x}", Sha256::digest([1, 2, 3, 4])),
                length: Some(4),
            };
            let result = locate_firmware_with_roots(
                &[FirmwareRoot::new(&root, RootOrigin::Preferences)],
                None,
                Some(&pin),
                &[],
                &mut Vec::new(),
            );
            assert!(matches!(result, Err(UserFirmwareError::Cancelled)));
        });
        assert_eq!(control.snapshot().phase, DiscoveryPhase::Cancelled);
        assert_eq!(control.snapshot().files_read, 1);
        assert_eq!(
            load(&path).unwrap().hashes.len(),
            1,
            "the completed, non-racy hash is retained"
        );
        let next = DiscoveryControl::default();
        discover(&root, &path, false, &next);
        assert_eq!(
            next.snapshot().files_read,
            1,
            "only the interrupted session's unread file is hashed"
        );
    }

    #[test]
    fn index_caps_entries_and_results_and_ignores_invalid_timestamps() {
        let temp = TempDir::new("bounded-firmware-cache");
        let path = temp.path().join("index");
        let control = DiscoveryControl::default();
        test_session(Some(&path), false, &control, || {
            persistent::with_cache(|cache| {
                for index in 0..MAX_ENTRIES + 10 {
                    let file = temp.path().join(format!("synthetic-{index}"));
                    cache.seen.insert(file.clone());
                    cache
                        .hashes
                        .insert((file, 4, UNIX_EPOCH, (0, 0), (0, 0)), "00".repeat(32));
                    control.result("skipped (synthetic)".into());
                }
                Ok(())
            })
            .unwrap();
        });
        assert_eq!(load(&path).unwrap().hashes.len(), MAX_ENTRIES);
        assert_eq!(control.snapshot().results.len(), MAX_RESULTS);
        assert_eq!(control.snapshot().result_count, MAX_ENTRIES + 10);
        fs::write(&path, format!("unemupc-firmware-index 2\n{}\t4\t18446744073709551615\t4294967295\t+\t0\t0\t0\t0\t{}\n", hex(b"/synthetic"), "00".repeat(32))).unwrap();
        assert!(load(&path).is_none());
    }

    #[test]
    fn unrelated_checks_and_other_pin_sizes_preserve_cached_hashes() {
        let temp = TempDir::new("retained-firmware-index");
        let root = temp.path().join("roms");
        fs::create_dir(&root).unwrap();
        let first = root.join("first.bin");
        let other = root.join("other.bin");
        fs::write(&first, [1, 2, 3, 4]).unwrap();
        fs::write(&other, [5; 8]).unwrap();
        let path = temp.path().join("cache/index");
        test_session(Some(&path), false, &DiscoveryControl::default(), || {
            with_cache(|cache| {
                cache.hash(&first).unwrap();
                cache.hash(&other).unwrap();
                Ok(())
            })
            .unwrap();
        });
        assert_eq!(load(&path).unwrap().hashes.len(), 2);
        test_session(Some(&path), false, &DiscoveryControl::default(), || {});
        assert_eq!(load(&path).unwrap().hashes.len(), 2);
        let control = DiscoveryControl::default();
        discover(&root, &path, false, &control);
        assert_eq!(control.snapshot().files_read, 0);
        assert_eq!(load(&path).unwrap().hashes.len(), 2);
        let control = DiscoveryControl::default();
        let pin = FirmwarePin {
            sha256: format!("{:x}", Sha256::digest([5; 8])),
            length: Some(8),
        };
        test_session(Some(&path), false, &control, || {
            assert!(locate_firmware_with_roots(
                &[FirmwareRoot::new(&root, RootOrigin::Preferences)],
                None,
                Some(&pin),
                &[],
                &mut Vec::new(),
            )
            .unwrap()
            .is_some());
        });
        assert_eq!(control.snapshot().files_read, 0);
        fs::remove_file(other).unwrap();
        discover(&root, &path, false, &DiscoveryControl::default());
        assert_eq!(load(&path).unwrap().hashes.len(), 1);
    }

    #[test]
    fn racy_hashes_are_not_persisted_and_uppercase_hashes_are_normalized() {
        let temp = TempDir::new("racy-firmware-index");
        let file = temp.path().join("rom.bin");
        fs::write(&file, [1, 2, 3, 4]).unwrap();
        let path = temp.path().join("index");
        with_discovery_session(Some(&path), false, &DiscoveryControl::default(), || {
            with_cache(|cache| {
                cache.hash(&file).unwrap();
                Ok(())
            })
            .unwrap();
        });
        assert!(load(&path).unwrap().hashes.is_empty());
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        fs::write(&file, [4, 3, 2, 1]).unwrap();
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let control = DiscoveryControl::default();
        with_discovery_session(Some(&path), false, &control, || {
            with_cache(|cache| {
                assert_eq!(
                    cache.hash(&file).unwrap(),
                    format!("{:x}", Sha256::digest([4, 3, 2, 1]))
                );
                Ok(())
            })
            .unwrap();
        });
        assert_eq!(control.snapshot().files_read, 1);
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            UNIX_EPOCH,
            "a nonpersistable hash does not dirty an empty index"
        );
        test_session(Some(&path), false, &DiscoveryControl::default(), || {
            with_cache(|cache| {
                cache.hash(&file).unwrap();
                Ok(())
            })
            .unwrap();
        });
        let text = fs::read_to_string(&path).unwrap();
        let mut lines = text.lines();
        let header = lines.next().unwrap();
        let mut fields: Vec<String> = lines
            .next()
            .unwrap()
            .split('\t')
            .map(str::to_owned)
            .collect();
        fields[9] = fields[9].to_uppercase();
        fs::write(&path, format!("{header}\n{}\n", fields.join("\t"))).unwrap();
        assert!(load(&path)
            .unwrap()
            .hashes
            .values()
            .all(|hash| *hash == hash.to_ascii_lowercase()));
    }

    #[test]
    fn unchanged_indexes_are_not_rewritten_and_stale_temps_are_swept() {
        let temp = TempDir::new("clean-firmware-index");
        let root = temp.path().join("roms");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("rom"), [1, 2, 3, 4]).unwrap();
        let path = temp.path().join("index");
        discover(&root, &path, false, &DiscoveryControl::default());
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        let old = temp.path().join(".firmware-index-old.tmp");
        let fresh = temp.path().join(".firmware-index-fresh.tmp");
        fs::write(&old, "stale").unwrap();
        fs::write(&fresh, "active").unwrap();
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        discover(&root, &path, false, &DiscoveryControl::default());
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), UNIX_EPOCH);
        assert!(!old.exists());
        assert!(fresh.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_index_is_never_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new("unreadable-firmware-index");
        let root = temp.path().join("roms");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("rom"), [1, 2, 3, 4]).unwrap();
        let path = temp.path().join("index");
        fs::write(&path, "preserve this index").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
        let denied = access_denied(&path);
        discover(&root, &path, false, &DiscoveryControl::default());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        if !denied {
            eprintln!("skipped unreadable-index check: this host reads files without permission (for example, as root)");
            // A readable but invalid index is rebuilt like any corrupt one.
            assert!(load(&path).is_some());
            return;
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "preserve this index");
    }

    #[test]
    fn eviction_preserves_seen_entries_before_older_paths() {
        let temp = TempDir::new("seen-firmware-eviction");
        let path = temp.path().join("index");
        let hot = temp.path().join("a-hot");
        test_session(Some(&path), false, &DiscoveryControl::default(), || {
            with_cache(|cache| {
                for index in 0..MAX_ENTRIES {
                    cache.hashes.insert(
                        (
                            temp.path().join(format!("z-old-{index}")),
                            4,
                            UNIX_EPOCH,
                            (0, 0),
                            (0, 0),
                        ),
                        "00".repeat(32),
                    );
                }
                cache.hashes.insert(
                    (hot.clone(), 4, UNIX_EPOCH, (0, 0), (0, 0)),
                    "00".repeat(32),
                );
                cache.seen.insert(hot.clone());
                cache.dirty = true;
                Ok(())
            })
            .unwrap();
        });
        let cache = load(&path).unwrap();
        assert_eq!(cache.hashes.len(), MAX_ENTRIES);
        assert!(cache.hashes.keys().any(|key| key.0 == hot));
    }

    #[test]
    fn platform_cache_paths_follow_host_conventions() {
        assert!(platform_path(
            None,
            None,
            Some("synthetic-local-cache".into()),
            false,
            true
        )
        .is_none());
        let home = PathBuf::from("/synthetic-home");
        assert_eq!(
            platform_path(Some(home.clone()), None, None, true, false),
            Some(home.join("Library/Caches/unemupc/firmware-index.v2"))
        );
        assert_eq!(
            platform_path(
                Some(home.clone()),
                Some("/synthetic-cache".into()),
                None,
                false,
                false
            ),
            Some(PathBuf::from("/synthetic-cache/unemupc/firmware-index.v2"))
        );
        assert_eq!(
            platform_path(
                Some(home.clone()),
                Some("relative".into()),
                None,
                false,
                false
            ),
            Some(home.join(".cache/unemupc/firmware-index.v2"))
        );
    }
}
