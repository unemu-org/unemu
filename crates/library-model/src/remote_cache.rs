// SPDX-License-Identifier: AGPL-3.0-or-later
//! Disposable, verified browse records inside the local image store.
//!
//! The 64 MiB cap is soft across processes: each process indexes files once,
//! reconciles under capacity pressure, then evicts to 75% to amortize scans.
//! Concurrent writers can temporarily exceed it; the next process start or
//! over-cap write reconciles them. Digest content is shared; mappings live in
//! `hosts/<host-hash>/`. Old flat host/mapping files are ignored by lookups.

use image_registry::browse::Cache;
use image_store::{Digest, Store, MAX_SMALL_BLOB};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

// Enough for thousands of ordinary manifests and records, while keeping browsing
// from competing with images for an unbounded amount of disk space.
const CAPACITY: u64 = 64 * 1024 * 1024;
static FILES: Mutex<BTreeMap<PathBuf, FileIndex>> = Mutex::new(BTreeMap::new());
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct FileIndex {
    sizes: BTreeMap<PathBuf, (Option<std::time::SystemTime>, u64)>,
    age: BTreeSet<(Option<std::time::SystemTime>, PathBuf)>,
    total: u64,
    #[cfg(test)]
    scans: usize,
}

impl FileIndex {
    fn scan(root: &Path) -> Self {
        let mut index = Self::default();
        index.rescan(root);
        if index.total > CAPACITY {
            index.trim(CAPACITY * 3 / 4);
        }
        index
    }

    fn rescan(&mut self, root: &Path) {
        #[cfg(test)]
        {
            self.scans += 1;
        }
        self.sizes.clear();
        self.age.clear();
        self.total = 0;
        self.scan_directory(root);
    }

    fn scan_directory(&mut self, root: &Path) {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    self.scan_directory(&entry.path());
                } else {
                    self.record(&entry.path());
                }
            }
        }
    }

    fn forget(&mut self, path: &Path) {
        if let Some((age, size)) = self.sizes.remove(path) {
            self.age.remove(&(age, path.to_owned()));
            self.total = self.total.saturating_sub(size);
        }
    }

    fn record(&mut self, path: &Path) {
        self.forget(path);
        if let Ok(metadata) = fs::metadata(path) {
            if metadata.is_file() {
                let age = metadata.modified().ok();
                self.total += metadata.len();
                self.sizes.insert(path.to_owned(), (age, metadata.len()));
                self.age.insert((age, path.to_owned()));
            }
        }
    }

    fn read(&mut self, root: &Path, path: &Path) -> Option<Vec<u8>> {
        let bytes = bounded_read(path);
        if bytes.is_none() && self.sizes.contains_key(path) && !path.exists() {
            self.rescan(root);
        }
        bytes
    }

    fn write(&mut self, root: &Path, path: &Path, bytes: &[u8]) {
        if atomic_write(path, bytes).is_ok() {
            self.record(path);
            self.evict(root, CAPACITY);
        }
    }

    fn remove(&mut self, path: &Path) {
        if fs::remove_file(path).is_ok() || !path.exists() {
            self.forget(path);
        }
    }

    fn drop_host(&mut self, directory: &Path) -> Option<PathBuf> {
        let retired = retire_host(directory);
        let paths: Vec<_> = self
            .sizes
            .range(directory.to_owned()..)
            .take_while(|(path, _)| path.starts_with(directory))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            self.forget(&path);
        }
        retired
    }

    fn evict(&mut self, root: &Path, capacity: u64) {
        if self.total <= capacity {
            return;
        }
        // Reconcile other processes' additions and evictions only under pressure.
        self.rescan(root);
        self.trim(capacity * 3 / 4);
    }

    fn trim(&mut self, target: u64) {
        while self.total > target {
            let Some((_, path)) = self.age.pop_first() else {
                break;
            };
            self.remove(&path);
        }
    }
}

#[derive(Default)]
pub(super) struct Session {
    pub(super) tags: BTreeMap<(String, String), Digest>,
    pub(super) identity: Option<Digest>,
    pub(super) revalidate: bool,
    pub(super) invalidate: bool,
}

pub(super) struct RemoteCache {
    root: PathBuf,
    host: String,
    session: Arc<Mutex<Session>>,
}

impl RemoteCache {
    pub(super) fn open(
        store: &Store,
        host: &str,
        identity: &[u8],
        session: Arc<Mutex<Session>>,
    ) -> Self {
        let cache = Self {
            root: store.root().join("cache/remote"),
            host: Digest::of(host.as_bytes()).hex().to_owned(),
            session,
        };
        let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
        let index = files
            .entry(cache.root.clone())
            .or_insert_with(|| FileIndex::scan(&cache.root));
        let mut session = cache.session.lock().unwrap_or_else(|e| e.into_inner());
        let identity = Digest::of(identity);
        let directory = cache.host_directory();
        let path = directory.join("identity");
        let previous = index.read(&cache.root, &path);
        let mut retired = None;
        let reset =
            session.invalidate || previous.as_deref() != Some(identity.to_string().as_bytes());
        if reset {
            retired = index.drop_host(&directory);
            session.tags.clear();
            session.invalidate = false;
        }
        if session
            .identity
            .as_ref()
            .is_some_and(|old| old != &identity)
        {
            session.tags.clear();
        }
        session.identity = Some(identity.clone());
        if reset {
            index.write(&cache.root, &path, identity.to_string().as_bytes());
        }
        drop(session);
        drop(files);
        if let Some(retired) = retired {
            let _ = fs::remove_dir_all(retired);
        }
        cache
    }

    fn host_directory(&self) -> PathBuf {
        self.root.join("hosts").join(&self.host)
    }

    fn tag_path(&self, repository: &str, tag: &str) -> PathBuf {
        let key = Digest::of(format!("{repository}:{tag}").as_bytes());
        self.host_directory().join(format!("tag-{}", key.hex()))
    }
}

pub(crate) fn with_preferences_host_invalidated<T>(
    preferences: &Path,
    host: &str,
    change: impl FnOnce() -> T,
) -> T {
    invalidate_preferences_host(preferences, host);
    let result = change();
    // A browse may recreate the old identity while verification or saving waits.
    invalidate_preferences_host(preferences, host);
    result
}

pub(crate) fn invalidate_preferences_host(preferences: &Path, host: &str) {
    let Ok(table) = image_registry::config::read_preferences(preferences) else {
        return;
    };
    let root = table
        .get("store_root")
        .and_then(toml::Value::as_str)
        .map(PathBuf::from)
        .or_else(|| image_store::location::default_store_root().ok());
    let Some(root) = root else {
        return;
    };
    let root = root.join("cache/remote");
    let host = Digest::of(host.as_bytes()).hex().to_owned();
    let directory = root.join("hosts").join(host);
    let retired = {
        let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
        match files.get_mut(&root) {
            Some(index) => index.drop_host(&directory),
            None => retire_host(&directory),
        }
    };
    if let Some(retired) = retired {
        let _ = fs::remove_dir_all(retired);
    }
}

fn retire_host(directory: &Path) -> Option<PathBuf> {
    let retired = directory.parent()?.join(format!(
        ".retired-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::rename(directory, &retired).ok()?;
    Some(retired)
}

impl Cache for RemoteCache {
    fn get(&self, digest: &Digest) -> Option<Vec<u8>> {
        let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
        let index = files
            .entry(self.root.clone())
            .or_insert_with(|| FileIndex::scan(&self.root));
        let path = self.root.join(format!("content-{}", digest.hex()));
        match index.read(&self.root, &path) {
            Some(bytes) if Digest::of(&bytes) == *digest => Some(bytes),
            _ => {
                index.remove(&path);
                None
            }
        }
    }

    fn put(&self, digest: &Digest, bytes: &[u8]) {
        if bytes.len() as u64 > MAX_SMALL_BLOB || Digest::of(bytes) != *digest {
            return;
        }
        let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
        let index = files
            .entry(self.root.clone())
            .or_insert_with(|| FileIndex::scan(&self.root));
        let path = self.root.join(format!("content-{}", digest.hex()));
        index.write(&self.root, &path, bytes);
    }

    fn tag(&self, repository: &str, tag: &str) -> Option<Digest> {
        let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
        let index = files
            .entry(self.root.clone())
            .or_insert_with(|| FileIndex::scan(&self.root));
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let key = (repository.to_owned(), tag.to_owned());
        if let Some(digest) = session.tags.get(&key) {
            return Some(digest.clone());
        }
        if session.revalidate {
            return None;
        }
        let bytes = index.read(&self.root, &self.tag_path(repository, tag))?;
        let digest = std::str::from_utf8(&bytes).ok()?.parse::<Digest>().ok()?;
        session.tags.insert(key, digest.clone());
        Some(digest)
    }

    fn remember(&self, repository: &str, tag: &str, digest: &Digest, immutable: bool) {
        let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
        let index = files
            .entry(self.root.clone())
            .or_insert_with(|| FileIndex::scan(&self.root));
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tags
            .insert((repository.to_owned(), tag.to_owned()), digest.clone());
        if immutable {
            let path = self.tag_path(repository, tag);
            let value = digest.to_string();
            if bounded_read(&path).as_deref() != Some(value.as_bytes()) {
                index.write(&self.root, &path, value.as_bytes());
            }
        } else {
            index.remove(&self.tag_path(repository, tag));
        }
    }
}

fn bounded_read(path: &Path) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SMALL_BLOB + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_SMALL_BLOB).then_some(bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    let _ = fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_writes_and_reopens_keep_a_single_directory_index() {
        let temp = image_store::test_support::TempDir::new("remote-cache-scans");
        let store = Store::open(temp.path().join("store")).unwrap();
        let cache = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        let scans = FILES.lock().unwrap()[&cache.root].scans;
        for n in 0..256 {
            let bytes = format!("record-{n}");
            let digest = Digest::of(bytes.as_bytes());
            cache.put(&digest, bytes.as_bytes());
            cache.remember("repo", &format!("1.0.{n}"), &digest, true);
            let reopened =
                RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
            assert_eq!(reopened.get(&digest).as_deref(), Some(bytes.as_bytes()));
        }
        let files = FILES.lock().unwrap();
        let index = &files[&cache.root];
        assert_eq!(index.scans, scans);
        assert_eq!(scans, 1);
        assert_eq!(index.sizes.len(), 513);
        let disk_size = FileIndex::scan(&cache.root).total;
        assert_eq!(index.total, disk_size);
    }

    #[test]
    fn sustained_capacity_pressure_amortizes_reconciliation() {
        let temp = image_store::test_support::TempDir::new("remote-cache-sustained");
        let mut index = FileIndex::scan(temp.path());
        let capacity = 128;
        let writes = 1000;
        for n in 0..writes {
            let path = temp.path().join(format!("content-{n}"));
            atomic_write(&path, b"12345678").unwrap();
            index.record(&path);
            index.evict(temp.path(), capacity);
            assert!(index.total <= capacity);
        }
        assert!(index.scans > 1);
        assert!(
            index.scans <= 1 + writes * 8 / (capacity - capacity * 3 / 4) as usize,
            "{} scans for {writes} writes",
            index.scans
        );
    }

    #[test]
    fn capacity_pressure_reconciles_unindexed_writes_and_startup_enforces_the_cap() {
        let temp = image_store::test_support::TempDir::new("remote-cache-external-cap");
        let mut index = FileIndex::scan(temp.path());
        atomic_write(&temp.path().join("external"), &[0; 128]).unwrap();
        let own = temp.path().join("own");
        atomic_write(&own, &[0; 129]).unwrap();
        index.record(&own);
        index.evict(temp.path(), 128);
        assert!(index.total <= 96);
        assert_eq!(FileIndex::scan(temp.path()).total, index.total);
        assert_eq!(index.scans, 2);

        for n in 0..3 {
            let file = fs::File::create(temp.path().join(format!("large-{n}"))).unwrap();
            file.set_len(CAPACITY / 2).unwrap();
        }
        let index = FileIndex::scan(temp.path());
        assert!(index.total <= CAPACITY * 3 / 4);
        assert_eq!(index.scans, 1);
    }

    #[test]
    fn invalidation_removes_concurrently_recreated_mappings_after_success_or_failure() {
        let temp = image_store::test_support::TempDir::new("remote-cache-change-race");
        let store = Store::open(temp.path().join("store")).unwrap();
        let preferences = temp.path().join("preferences.toml");
        fs::write(&preferences, format!("store_root = {:?}\n", store.root())).unwrap();
        let digest = Digest::of(b"record");
        for succeeds in [true, false] {
            let before =
                RemoteCache::open(&store, "registry.example", b"same user", Arc::default());
            before.remember("repo", "1.0.0", &digest, true);
            let result =
                with_preferences_host_invalidated(&preferences, "registry.example", || {
                    let during =
                        RemoteCache::open(&store, "registry.example", b"same user", Arc::default());
                    assert_eq!(during.tag("repo", "1.0.0"), None);
                    during.remember("repo", "1.0.0", &digest, true);
                    succeeds.then_some(()).ok_or("save failed")
                });
            assert_eq!(result.is_ok(), succeeds);
            let after = RemoteCache::open(&store, "registry.example", b"same user", Arc::default());
            assert_eq!(after.tag("repo", "1.0.0"), None);
        }
    }

    #[test]
    fn host_invalidation_needs_no_scan_and_removes_unindexed_mappings() {
        let temp = image_store::test_support::TempDir::new("remote-cache-host-directory");
        let store = Store::open(temp.path().join("store")).unwrap();
        let preferences = temp.path().join("preferences.toml");
        fs::write(&preferences, format!("store_root = {:?}\n", store.root())).unwrap();
        let cache = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        let digest = Digest::of(b"record");
        cache.put(&digest, b"record");
        cache.remember("repo", "1.0.0", &digest, true);
        // Another process can add files our index has never seen.
        atomic_write(
            &cache.tag_path("repo", "1.1.0"),
            digest.to_string().as_bytes(),
        )
        .unwrap();
        let scans = FILES.lock().unwrap()[&cache.root].scans;
        invalidate_preferences_host(&preferences, "registry.example");
        assert!(!cache.host_directory().exists());
        assert_eq!(FILES.lock().unwrap()[&cache.root].scans, scans);
        assert_eq!(cache.get(&digest).as_deref(), Some(b"record".as_slice()));
        let reopened = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        assert_eq!(reopened.tag("repo", "1.0.0"), None);
        assert_eq!(reopened.tag("repo", "1.1.0"), None);
        assert_eq!(FILES.lock().unwrap()[&cache.root].scans, scans);

        FILES.lock().unwrap().remove(&cache.root);
        invalidate_preferences_host(&preferences, "registry.example");
        assert!(!FILES.lock().unwrap().contains_key(&cache.root));
    }

    #[test]
    fn settings_save_does_not_wait_for_the_cache_mutex_on_the_caller_thread() {
        use crate::registries_model::RegistriesPage;
        use image_registry::{credential_store::SessionStore, credentials::EnvCredentials};
        let temp = image_store::test_support::TempDir::new("remote-cache-settings-thread");
        let store = Store::open(temp.path().join("store")).unwrap();
        let preferences = temp.path().join("preferences.toml");
        fs::write(&preferences, format!("store_root = {:?}\n", store.root())).unwrap();
        let mut page = RegistriesPage::new(
            Some(preferences),
            Arc::new(SessionStore::default()),
            EnvCredentials::default(),
        );
        page.wait();
        page.open_add();
        page.editor.as_mut().unwrap().host = "registry.example".into();
        let guard = FILES.lock().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            page.submit_editor();
            send.send(()).unwrap();
            page.wait();
            assert!(page.editor.is_none(), "{:?}", page.error);
        });
        let returned = receive.recv_timeout(std::time::Duration::from_secs(5));
        drop(guard);
        caller.join().unwrap();
        assert!(
            returned.is_ok(),
            "settings save blocked on cache invalidation"
        );
    }

    #[test]
    fn external_eviction_and_capacity_pressure_reconcile_the_index() {
        let temp = image_store::test_support::TempDir::new("remote-cache-reconcile");
        let first = temp.path().join("first");
        fs::write(&first, b"1234").unwrap();
        let mut index = FileIndex::scan(temp.path());
        fs::remove_file(&first).unwrap();
        let second = temp.path().join("second");
        fs::write(&second, b"12345678").unwrap();
        assert_eq!(index.read(temp.path(), &first), None);
        assert_eq!(index.total, 8);
        assert_eq!(index.scans, 2);
        let third = temp.path().join("third");
        fs::write(&third, b"1234").unwrap();
        index.record(&third);
        index.evict(temp.path(), 4);
        assert!(index.total <= 4);
        assert_eq!(index.scans, 3);
        assert!(!second.exists());
    }

    #[test]
    fn preferences_settings_save_invalidates_only_the_changed_hosts_mappings() {
        use crate::registries_model::RegistriesPage;
        use image_registry::{credential_store::SessionStore, credentials::EnvCredentials};
        let temp = image_store::test_support::TempDir::new("remote-cache-preferences");
        let store = Store::open(temp.path().join("store")).unwrap();
        let preferences = temp.path().join("preferences.toml");
        fs::write(
            &preferences,
            format!(
                "store_root = {:?}\n[[registries]]\nhost = \"registry.example\"\nprivate = false\n",
                store.root()
            ),
        )
        .unwrap();
        let first = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        let other = RemoteCache::open(&store, "other.example", b"settings", Arc::default());
        let digest = Digest::of(b"record");
        first.put(&digest, b"record");
        first.remember("repo", "1.0.0", &digest, true);
        other.remember("repo", "1.0.0", &digest, true);
        let mut page = RegistriesPage::new(
            Some(preferences),
            Arc::new(SessionStore::default()),
            EnvCredentials::default(),
        );
        page.selected = page
            .rows
            .iter()
            .position(|row| row.entry.host == "registry.example")
            .unwrap();
        page.open_edit();
        page.editor.as_mut().unwrap().private = true;
        page.submit_editor();
        page.wait();
        assert!(page.editor.is_none(), "{:?}", page.error);
        let first = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        assert_eq!(first.tag("repo", "1.0.0"), None);
        assert_eq!(other.tag("repo", "1.0.0"), Some(digest.clone()));
        assert_eq!(first.get(&digest).as_deref(), Some(b"record".as_slice()));
    }

    #[test]
    fn restart_keeps_immutable_mappings_and_refresh_resolves_them_again() {
        let temp = image_store::test_support::TempDir::new("remote-cache-restart");
        let store = Store::open(temp.path().join("store")).unwrap();
        let digest = Digest::of(b"record");
        let cache = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        cache.put(&digest, b"record");
        cache.remember("repo", "1.0.0", &digest, true);
        cache.remember("repo", "latest", &digest, false);
        assert_eq!(cache.tag("repo", "latest"), Some(digest.clone()));
        drop(cache);
        let cache = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        assert_eq!(cache.tag("repo", "1.0.0"), Some(digest.clone()));
        assert_eq!(cache.tag("repo", "latest"), None);
        assert_eq!(cache.get(&digest).as_deref(), Some(b"record".as_slice()));
        let cache = RemoteCache::open(
            &store,
            "registry.example",
            b"settings",
            Arc::new(Mutex::new(Session {
                revalidate: true,
                ..Session::default()
            })),
        );
        assert_eq!(cache.tag("repo", "1.0.0"), None);
        assert_eq!(cache.get(&digest).as_deref(), Some(b"record".as_slice()));
        cache.remember("repo", "1.0.0", &digest, true);
        assert_eq!(cache.tag("repo", "1.0.0"), Some(digest));
    }

    #[test]
    fn capacity_evicts_oldest_entries_and_counts_mappings() {
        let temp = image_store::test_support::TempDir::new("remote-cache-capacity");
        for (name, age) in [("content-old", 1), ("tag-new", 2), ("content-newest", 3)] {
            let path = temp.path().join(name);
            fs::write(&path, b"1234").unwrap();
            fs::File::open(path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(age))
                .unwrap();
        }
        let mut index = FileIndex::scan(temp.path());
        index.evict(temp.path(), 8);
        assert!(!temp.path().join("content-old").exists());
        assert!(!temp.path().join("tag-new").exists());
        assert!(temp.path().join("content-newest").exists());
        index.evict(temp.path(), 0);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn content_is_bounded_verified_and_corruption_is_deleted() {
        let temp = image_store::test_support::TempDir::new("remote-cache-content");
        let store = Store::open(temp.path().join("store")).unwrap();
        let cache = RemoteCache::open(&store, "registry.example", b"settings", Arc::default());
        let digest = Digest::of(b"record");
        cache.put(&digest, b"record");
        assert_eq!(cache.get(&digest).as_deref(), Some(b"record".as_slice()));
        let path = cache.root.join(format!("content-{}", digest.hex()));
        fs::write(&path, b"corrupt").unwrap();
        assert_eq!(cache.get(&digest), None);
        assert!(!path.exists());
        fs::write(&path, vec![0; MAX_SMALL_BLOB as usize + 1]).unwrap();
        assert_eq!(cache.get(&digest), None);
        assert!(!path.exists());
        cache.put(&digest, b"wrong digest");
        assert!(!path.exists());
        let oversized = vec![0; MAX_SMALL_BLOB as usize + 1];
        let digest = Digest::of(&oversized);
        cache.put(&digest, &oversized);
        assert_eq!(cache.get(&digest), None);
    }
}

#[cfg(test)]
mod invalidation_tests {
    use super::*;

    #[test]
    fn settings_and_login_invalidation_drop_only_the_hosts_mappings() {
        let temp = image_store::test_support::TempDir::new("remote-cache-hosts");
        let store = Store::open(temp.path().join("store")).unwrap();
        let digest = Digest::of(b"record");
        let first = RemoteCache::open(&store, "first.example", b"settings", Arc::default());
        let other = RemoteCache::open(&store, "other.example", b"settings", Arc::default());
        first.put(&digest, b"record");
        first.remember("repo", "1.0.0", &digest, true);
        other.remember("repo", "1.0.0", &digest, true);
        let first = RemoteCache::open(&store, "first.example", b"changed settings", Arc::default());
        assert_eq!(first.tag("repo", "1.0.0"), None);
        assert_eq!(other.tag("repo", "1.0.0"), Some(digest.clone()));
        assert_eq!(first.get(&digest).as_deref(), Some(b"record".as_slice()));
        first.remember("repo", "1.0.0", &digest, true);
        let first = RemoteCache::open(
            &store,
            "first.example",
            b"changed settings",
            Arc::new(Mutex::new(Session {
                invalidate: true,
                ..Session::default()
            })),
        );
        assert_eq!(first.tag("repo", "1.0.0"), None);
        assert_eq!(first.get(&digest).as_deref(), Some(b"record".as_slice()));
        first.remember("repo", "1.0.0", &digest, true);
        let first = RemoteCache::open(&store, "first.example", b"changed settings", Arc::default());
        assert_eq!(first.tag("repo", "1.0.0"), Some(digest.clone()));
        let other = RemoteCache::open(&store, "other.example", b"settings", Arc::default());
        assert_eq!(other.tag("repo", "1.0.0"), Some(digest));
    }
}
