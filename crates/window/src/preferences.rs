// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-only ROM folder and presentation preferences; machine-images.md §7.

use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub store_root: Option<PathBuf>,
    pub rom_dirs: Vec<PathBuf>,
    pub reduced_motion: bool,
    /// Off by default: taking macOS system shortcuts for the guest needs the
    /// Accessibility privilege, which only the user may decide to grant.
    pub send_system_shortcuts: bool,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, toml::Value>,
}

impl Preferences {
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }
    /// Write this shell's keys into the file as it is now. Other writers (the
    /// command-line registry login, a hand edit) own the remaining keys, so
    /// those come from a fresh read under the shared lock, not from the copy
    /// loaded at startup.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.save_before_rename(path, || Ok(()))
    }

    fn save_before_rename(
        &self,
        path: &Path,
        before_rename: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let _lock = lock(path)?;
        let mut table: toml::Table = match std::fs::read_to_string(path) {
            Ok(text) => text.parse().map_err(|error| {
                io::Error::other(format!("not overwriting invalid preferences: {error}"))
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => toml::Table::new(),
            Err(error) => return Err(error),
        };
        let mine = toml::Table::try_from(self).map_err(io::Error::other)?;
        for key in OWNED_KEYS {
            match mine.get(key) {
                Some(value) => table.insert(key.into(), value.clone()),
                None => table.remove(key),
            };
        }
        let text = toml::to_string(&table).map_err(io::Error::other)?;
        let (temp, mut file) = loop {
            let temp = parent.join(format!(
                ".preferences-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
            {
                Ok(file) => break (temp, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = (|| {
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            before_rename()?;
            std::fs::rename(&temp, path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
    pub fn add_folder(&mut self, path: &Path) -> io::Result<()> {
        let path = path.canonicalize()?;
        if !path.is_dir() {
            return Err(io::Error::other("ROM folder must be a directory"));
        }
        if !self.rom_dirs.contains(&path) {
            self.rom_dirs.push(path);
        }
        Ok(())
    }
    pub fn roots(
        &self,
        explicit: &[PathBuf],
        defaults: &[assembly::firmware_discovery::FirmwareRoot],
    ) -> Vec<assembly::firmware_discovery::FirmwareRoot> {
        use assembly::firmware_discovery::{FirmwareRoot, RootOrigin};
        let mut roots = explicit
            .iter()
            .map(|path| FirmwareRoot::new(path, RootOrigin::CommandLine))
            .collect::<Vec<_>>();
        roots.extend(
            self.rom_dirs
                .iter()
                .map(|path| FirmwareRoot::new(path, RootOrigin::Preferences)),
        );
        roots.extend_from_slice(defaults);
        roots
    }
}

/// The keys this shell writes; every other key belongs to another writer.
const OWNED_KEYS: [&str; 4] = [
    "store_root",
    "rom_dirs",
    "reduced_motion",
    "send_system_shortcuts",
];

/// Advisory lock on the sibling `<file>.lock`, shared with the command-line
/// registry login. The file itself is replaced by rename, so it cannot carry
/// the lock. Released when the returned handle drops.
fn lock(path: &Path) -> io::Result<std::fs::File> {
    let mut name = path
        .file_name()
        .ok_or_else(|| io::Error::other("preferences path has no file name"))?
        .to_owned();
    name.push(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_file_name(name))?;
    file.lock()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reduced_motion_defaults_off_and_round_trips() {
        let mut prefs: Preferences = toml::from_str("rom_dirs = []").unwrap();
        assert!(!prefs.reduced_motion);
        prefs.reduced_motion = true;
        let restored: Preferences = toml::from_str(&toml::to_string(&prefs).unwrap()).unwrap();
        assert!(restored.reduced_motion);
    }

    #[test]
    fn system_shortcuts_stay_with_macos_until_the_user_turns_them_on() {
        let temp = assembly::test_support::TempDir::new("shell-system-shortcuts");
        let path = temp.path().join("preferences.toml");
        assert!(!Preferences::load(&path).unwrap().send_system_shortcuts);
        std::fs::write(&path, "rom_dirs = []\n").unwrap();
        let mut prefs = Preferences::load(&path).unwrap();
        assert!(!prefs.send_system_shortcuts);
        prefs.send_system_shortcuts = true;
        prefs.save(&path).unwrap();
        assert!(Preferences::load(&path).unwrap().send_system_shortcuts);
        prefs.send_system_shortcuts = false;
        prefs.save(&path).unwrap();
        assert!(!Preferences::load(&path).unwrap().send_system_shortcuts);
    }

    #[test]
    fn preferences_round_trip_with_validated_rom_folders() {
        let temp = assembly::test_support::TempDir::new("shell-preferences");
        let path = temp.path().join("config/preferences.toml");
        assert_eq!(Preferences::load(&path).unwrap(), Preferences::default());
        let mut prefs = Preferences::default();
        prefs.add_folder(temp.path()).unwrap();
        prefs.add_folder(temp.path()).unwrap();
        assert_eq!(prefs.rom_dirs.len(), 1);
        prefs.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), prefs);
        assert!(prefs.add_folder(&path).is_err());
    }
    #[test]
    fn atomic_save_preserves_unknown_fields_and_old_contents_on_failure() {
        let temp = assembly::test_support::TempDir::new("atomic-shell-preferences");
        let path = temp.path().join("preferences.toml");
        let original = "rom_dirs = []\nfuture_option = true\n[future_table]\nvalue = 'preserved'\n";
        std::fs::write(&path, original).unwrap();
        let mut prefs = Preferences::load(&path).unwrap();
        prefs.add_folder(temp.path()).unwrap();
        assert!(prefs
            .save_before_rename(&path, || Err(io::Error::other("injected save failure")))
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(temporary_files(temp.path()), 0);
        #[cfg(unix)]
        let old_handle = std::fs::File::open(&path).unwrap();
        prefs.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), prefs);
        assert_eq!(prefs.extra["future_option"].as_bool(), Some(true));
        assert_eq!(
            prefs.extra["future_table"]["value"].as_str(),
            Some("preserved")
        );
        assert_eq!(temporary_files(temp.path()), 0);
        #[cfg(unix)]
        {
            use std::io::Read;
            let mut contents = String::new();
            (&old_handle).read_to_string(&mut contents).unwrap();
            assert_eq!(contents, original);
        }
    }

    fn temporary_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count()
    }

    #[test]
    fn a_save_keeps_registry_settings_written_after_the_shell_loaded_its_copy() {
        let temp = assembly::test_support::TempDir::new("shared-shell-preferences");
        let path = temp.path().join("preferences.toml");
        let registry = |private: bool, user: &str| {
            format!(
                "[[registries]]\nhost = \"nas.example\"\nprivate = {private}\nuser = \"{user}\"\n"
            )
        };
        std::fs::write(&path, format!("rom_dirs = []\n{}", registry(true, "old"))).unwrap();
        let mut prefs = Preferences::load(&path).unwrap();
        // Meanwhile a registry login records a user, and the user un-marks
        // the registry as private by hand.
        std::fs::write(
            &path,
            format!(
                "rom_dirs = []\ndefault_registry = \"nas.example\"\n{}",
                registry(false, "new")
            ),
        )
        .unwrap();
        prefs.add_folder(temp.path()).unwrap();
        prefs.reduced_motion = true;
        prefs.save(&path).unwrap();
        let saved: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        let entry = &saved["registries"][0];
        assert_eq!(entry["private"].as_bool(), Some(false));
        assert_eq!(entry["user"].as_str(), Some("new"));
        assert_eq!(saved["default_registry"].as_str(), Some("nas.example"));
        assert_eq!(saved["reduced_motion"].as_bool(), Some(true));
        let reloaded = Preferences::load(&path).unwrap();
        assert_eq!(reloaded.rom_dirs, prefs.rom_dirs);

        prefs.store_root = None;
        std::fs::write(
            &path,
            format!("store_root = \"/elsewhere\"\n{}", registry(false, "new")),
        )
        .unwrap();
        prefs.save(&path).unwrap();
        let saved: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert!(
            saved.get("store_root").is_none(),
            "the shell owns store_root"
        );
        assert_eq!(saved["registries"][0]["user"].as_str(), Some("new"));
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn review_store_override_round_trips_without_changing_old_preferences() {
        let old: Preferences = toml::from_str("rom_dirs=[]").unwrap();
        assert_eq!(old.store_root, None);
        let prefs = Preferences {
            store_root: Some(PathBuf::from("/data/custom-store")),
            ..old
        };
        let loaded: Preferences = toml::from_str(&toml::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(loaded.store_root, prefs.store_root);
    }
}
