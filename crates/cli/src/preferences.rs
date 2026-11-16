// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where the shared host preferences file lives; machine-images.md §7.
//!
//! One TOML file holds the desktop's ROM folders and presentation settings and
//! the registry list the command line edits. Both frontends take its path from
//! [`path`], so they always read and write the same file.
//!
//! The project was renamed from unemupc98 to unemupc, and the file moved with
//! it to a directory under the new name. The first time either frontend asks
//! for the path, an existing file under the former name is copied into the new
//! place before anything reads it, so registries, ROM folders and settings
//! carry over. The old file stays, so an older build still finds its own.

use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

/// The directory under the platform config directory that holds the file.
pub const DIRECTORY: &str = "unemupc";

/// The directory the project used before its rename; a preferences file left
/// there is copied once into [`DIRECTORY`].
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
    migrate(&root.join(FORMER_DIRECTORY).join(FILE_NAME), &path)?;
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
    fs::create_dir_all(parent).map_err(context)?;
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
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
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
    use assembly::test_support::TempDir;

    fn files(root: &Path) -> (PathBuf, PathBuf) {
        (
            root.join(FORMER_DIRECTORY).join(FILE_NAME),
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
