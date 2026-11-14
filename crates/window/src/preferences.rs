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
    pub rom_dirs: Vec<PathBuf>,
    pub reduced_motion: bool,
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
        let text = toml::to_string(self).map_err(io::Error::other)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
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
    pub fn roots(&self, explicit: &[PathBuf]) -> Vec<assembly::firmware_discovery::FirmwareRoot> {
        use assembly::firmware_discovery::{default_firmware_roots, FirmwareRoot, RootOrigin};
        let mut roots = explicit
            .iter()
            .map(|path| FirmwareRoot::new(path, RootOrigin::CommandLine))
            .collect::<Vec<_>>();
        roots.extend(
            self.rom_dirs
                .iter()
                .map(|path| FirmwareRoot::new(path, RootOrigin::Preferences)),
        );
        roots.extend(default_firmware_roots());
        roots
    }
}

pub(super) fn config_path() -> io::Result<PathBuf> {
    platform_path(
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("APPDATA").map(PathBuf::from),
        cfg!(target_os = "macos"),
        cfg!(windows),
    )
}

fn platform_path(
    home: Option<PathBuf>,
    xdg: Option<PathBuf>,
    appdata: Option<PathBuf>,
    macos: bool,
    windows: bool,
) -> io::Result<PathBuf> {
    let root = if macos {
        home.map(|home| home.join("Library/Application Support"))
    } else if windows {
        appdata
    } else {
        xdg.filter(|path| path.is_absolute())
            .or_else(|| home.map(|home| home.join(".config")))
    };
    root.map(|root| root.join("unemupc98/preferences.toml"))
        .ok_or_else(|| io::Error::other("platform config directory is unavailable"))
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
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
        #[cfg(unix)]
        let old_handle = std::fs::File::open(&path).unwrap();
        prefs.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), prefs);
        assert_eq!(prefs.extra["future_option"].as_bool(), Some(true));
        assert_eq!(
            prefs.extra["future_table"]["value"].as_str(),
            Some("preserved")
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::io::Read;
            let mut contents = String::new();
            (&old_handle).read_to_string(&mut contents).unwrap();
            assert_eq!(contents, original);
        }
    }

    #[cfg(unix)]
    #[test]
    fn config_directory_matches_platform_conventions() {
        let home = PathBuf::from("/synthetic-home");
        assert_eq!(
            platform_path(Some(home.clone()), None, None, true, false).unwrap(),
            home.join("Library/Application Support/unemupc98/preferences.toml")
        );
        assert_eq!(
            platform_path(
                Some(home),
                Some(PathBuf::from("/synthetic-config")),
                None,
                false,
                false
            )
            .unwrap(),
            PathBuf::from("/synthetic-config/unemupc98/preferences.toml")
        );
    }
}
