// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where the shared host preferences file lives; machine-images.md §7.
//!
//! One TOML file holds the desktop's ROM folders and presentation settings and
//! the registry list the command line edits. Both frontends take its path from
//! [`path`], so they always read and write the same file. It lives here,
//! beside the default store root, because it is the other host location both
//! frontends share.
//!
//! The front end is named unemu, and the file moved with
//! it to a directory under that name. The first time either frontend asks
//! for the path, an existing file under the former name is copied into the new
//! place before anything reads it, so registries, ROM folders and settings
//! carry over. The old file stays, so an older build still finds its own.

use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

/// The directory under the platform config directory that holds the file.
pub const DIRECTORY: &str = "unemu";

/// The original project directory; retained for upgrades that skip a release.
pub const FORMER_DIRECTORY: &str = "unemupc98";

const FILE_NAME: &str = "preferences.toml";

/// The shared preferences file in the platform config directory, after
/// copying it from the former directory when only that one has it.
pub fn path() -> io::Result<PathBuf> {
    let root = config_root(
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("APPDATA").map(PathBuf::from),
        cfg!(target_os = "macos"),
        cfg!(windows),
    )?;
    path_under(&root)
}

fn path_under(root: &Path) -> io::Result<PathBuf> {
    let path = root.join(DIRECTORY).join(FILE_NAME);
    migrate(&root.join("unemupc").join(FILE_NAME), &path)?;
    // Keep the earlier upgrade path for users who skipped the intermediate name.
    if !path.try_exists()? {
        migrate(&root.join(FORMER_DIRECTORY).join(FILE_NAME), &path)?;
    }
    Ok(path)
}

/// Application Support on macOS, `%APPDATA%` on Windows, otherwise an
/// absolute `XDG_CONFIG_HOME` or `~/.config`.
fn config_root(
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
    root.ok_or_else(|| io::Error::other("platform config directory is unavailable"))
}

/// Copy `old` to `new` when `new` does not exist and `old` is a file; true
/// when it copied. Every writer of the file holds the advisory lock on the
/// sibling `<file>.lock`, so the copy takes it too and looks again under it:
/// a frontend that started at the same moment and has already written the new
/// file keeps its write. The copy arrives by rename, so a reader that does not
/// lock never sees half a file.
fn migrate(old: &Path, new: &Path) -> io::Result<bool> {
    migrate_with_writer(old, new, |file, bytes| file.write_all(bytes))
}

fn migrate_with_writer(
    old: &Path,
    new: &Path,
    write: impl FnOnce(&mut fs::File, &[u8]) -> io::Result<()>,
) -> io::Result<bool> {
    let context = |error: io::Error| {
        io::Error::new(
            error.kind(),
            format!(
                "copying preferences from {} to {}: {error}",
                old.display(),
                new.display()
            ),
        )
    };
    if new.try_exists().map_err(context)? || !old.is_file() {
        return Ok(false);
    }
    let (Some(parent), Some(name)) = (new.parent(), new.file_name()) else {
        return Err(context(io::Error::other("no file name")));
    };
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        // Preferences may have relied on a private directory for protection.
        directory.mode(0o700);
    }
    directory.create(parent).map_err(context)?;
    let mut lock_name = name.to_owned();
    lock_name.push(".lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(lock_name))
        .map_err(context)?;
    lock.lock().map_err(context)?;
    if new.try_exists().map_err(context)? {
        return Ok(false);
    }
    let bytes = fs::read(old).map_err(context)?;
    let temporary = parent.join(format!(".preferences-migrate-{}.tmp", std::process::id()));
    let result = (|| {
        // A killed writer may leave this name; unlink it rather than opening
        // it, since a leftover link must never redirect a write into old data.
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(fs::metadata(old)?.permissions().mode() & 0o600);
        }
        let mut file = options.open(&temporary)?;
        write(&mut file, &bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, new)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|()| true).map_err(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temporary directory, removed on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn files(root: &Path) -> (PathBuf, PathBuf) {
        (
            root.join("unemupc").join(FILE_NAME),
            root.join(DIRECTORY).join(FILE_NAME),
        )
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn the_new_path_takes_a_copy_when_only_the_former_file_exists() {
        let temp = TempDir::new("preferences-migrate");
        let (old, new) = files(temp.path());
        let text = "rom_dirs = [\"/roms\"]\n[[registries]]\nhost = \"nas.example\"\n";
        write(&old, text);
        assert_eq!(path_under(temp.path()).unwrap(), new);
        assert_eq!(fs::read_to_string(&new).unwrap(), text);
        assert_eq!(
            fs::read_to_string(&old).unwrap(),
            text,
            "the old file stays"
        );
        let leftovers = fs::read_dir(new.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn an_existing_new_file_is_left_alone() {
        let temp = TempDir::new("preferences-keep-new");
        let (old, new) = files(temp.path());
        write(&old, "reduced_motion = false\n");
        write(&new, "reduced_motion = true\n");
        assert!(!migrate(&old, &new).unwrap());
        assert_eq!(path_under(temp.path()).unwrap(), new);
        assert_eq!(fs::read_to_string(&new).unwrap(), "reduced_motion = true\n");
        assert_eq!(
            fs::read_to_string(&old).unwrap(),
            "reduced_motion = false\n"
        );
    }

    #[test]
    fn nothing_is_created_when_neither_file_exists() {
        let temp = TempDir::new("preferences-none");
        let (old, new) = files(temp.path());
        assert_eq!(path_under(temp.path()).unwrap(), new);
        assert!(!new.parent().unwrap().exists());
        assert!(!old.parent().unwrap().exists());
    }

    #[test]
    fn copying_happens_once() {
        let temp = TempDir::new("preferences-once");
        let (old, new) = files(temp.path());
        write(&old, "reduced_motion = true\n");
        assert!(migrate(&old, &new).unwrap());
        assert!(!migrate(&old, &new).unwrap());
        // A save after the copy is never overwritten by a later start.
        fs::write(&new, "reduced_motion = false\n").unwrap();
        assert_eq!(path_under(temp.path()).unwrap(), new);
        assert!(!migrate(&old, &new).unwrap());
        assert_eq!(
            fs::read_to_string(&new).unwrap(),
            "reduced_motion = false\n"
        );
        assert_eq!(fs::read_to_string(&old).unwrap(), "reduced_motion = true\n");
    }

    #[test]
    fn an_error_after_a_partial_preferences_write_keeps_the_old_file_and_retries() {
        let temp = TempDir::new("preferences-interrupted");
        let (old, new) = files(temp.path());
        let original = "reduced_motion = true\nrom_dirs = ['roms']\n";
        write(&old, original);
        let modified = fs::metadata(&old).unwrap().modified().unwrap();
        let error = migrate_with_writer(&old, &new, |file, bytes| {
            file.write_all(&bytes[..5])?;
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "simulated interruption",
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(!new.exists());
        assert_eq!(fs::read_to_string(&old).unwrap(), original);
        assert_eq!(fs::metadata(&old).unwrap().modified().unwrap(), modified);
        // Simulate the temporary bytes a killed process could leave behind.
        let temporary = new
            .parent()
            .unwrap()
            .join(format!(".preferences-migrate-{}.tmp", std::process::id()));
        fs::write(&temporary, "partial").unwrap();
        assert_eq!(path_under(temp.path()).unwrap(), new);
        assert_eq!(fs::read_to_string(&new).unwrap(), original);
        assert_eq!(fs::read_to_string(&old).unwrap(), original);
        assert!(!temporary.exists());
    }

    #[cfg(unix)]
    #[test]
    fn migration_keeps_preferences_private_at_creation() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o644, 0o400] {
            let temp = TempDir::new("preferences-private");
            let (old, new) = files(temp.path());
            write(&old, "rom_dirs = ['roms']\n");
            fs::set_permissions(old.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&old, fs::Permissions::from_mode(mode)).unwrap();
            assert!(migrate_with_writer(&old, &new, |file, bytes| {
                assert_eq!(
                    fs::metadata(new.parent().unwrap())?.permissions().mode() & 0o777,
                    0o700
                );
                assert_eq!(file.metadata()?.permissions().mode() & 0o777, mode & 0o600);
                file.write_all(bytes)
            })
            .unwrap());
            assert_eq!(
                fs::metadata(&new).unwrap().permissions().mode() & 0o777,
                mode & 0o600
            );
            assert_eq!(
                fs::metadata(&old).unwrap().permissions().mode() & 0o777,
                mode
            );
            assert_eq!(fs::read_to_string(&new).unwrap(), "rom_dirs = ['roms']\n");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_leftover_preferences_link_cannot_overwrite_the_old_file() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = TempDir::new("preferences-leftover-link");
        let (old, new) = files(temp.path());
        write(&old, "reduced_motion = true\n");
        fs::set_permissions(&old, fs::Permissions::from_mode(0o600)).unwrap();
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        let temporary = new
            .parent()
            .unwrap()
            .join(format!(".preferences-migrate-{}.tmp", std::process::id()));
        symlink(&old, &temporary).unwrap();
        assert!(migrate_with_writer(&old, &new, |file, bytes| {
            assert_eq!(file.metadata()?.permissions().mode() & 0o777, 0o600);
            file.write_all(&bytes[..5])?;
            Err(io::Error::other("simulated interruption"))
        })
        .is_err());
        assert_eq!(fs::read_to_string(&old).unwrap(), "reduced_motion = true\n");
        assert!(!new.exists());
        assert_eq!(path_under(temp.path()).unwrap(), new);
        assert_eq!(fs::read_to_string(&new).unwrap(), "reduced_motion = true\n");
    }

    #[cfg(unix)]
    #[test]
    fn config_directory_matches_platform_conventions() {
        let home = PathBuf::from("/synthetic-home");
        assert_eq!(
            config_root(Some(home.clone()), None, None, true, false).unwrap(),
            home.join("Library/Application Support")
        );
        assert_eq!(
            config_root(
                Some(home.clone()),
                Some(PathBuf::from("/synthetic-config")),
                None,
                false,
                false
            )
            .unwrap(),
            PathBuf::from("/synthetic-config")
        );
        assert_eq!(
            config_root(
                Some(home.clone()),
                Some("relative".into()),
                None,
                false,
                false
            )
            .unwrap(),
            home.join(".config")
        );
        assert_eq!(
            config_root(
                None,
                None,
                Some(PathBuf::from("/synthetic-appdata")),
                false,
                true
            )
            .unwrap(),
            PathBuf::from("/synthetic-appdata")
        );
        assert!(config_root(None, None, None, true, false).is_err());
    }
}
