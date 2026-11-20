// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host storage locations shared by the shell and library; machine-images §3.
//! Location selection is host preparation and never changes guest virtual time.

use crate::{invalid, Result, Store};
use std::path::{Path, PathBuf};

pub mod frontend;
pub mod preferences;

pub fn default_store_root() -> Result<PathBuf> {
    Ok(data_directory()?.join("unemu"))
}
/// The platform data directory the store and other host data live under:
/// `%LOCALAPPDATA%` on Windows, Application Support on macOS, otherwise an
/// absolute `XDG_DATA_HOME` or `~/.local/share`.
pub fn data_directory() -> Result<PathBuf> {
    platform_data_directory(
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        cfg!(target_os = "macos"),
        cfg!(windows),
    )
}
fn platform_data_directory(
    home: Option<PathBuf>,
    xdg: Option<PathBuf>,
    local: Option<PathBuf>,
    macos: bool,
    windows: bool,
) -> Result<PathBuf> {
    let root = if windows {
        local.ok_or_else(|| invalid("LOCALAPPDATA is unavailable"))?
    } else if macos {
        home.ok_or_else(|| invalid("HOME is unavailable"))?
            .join("Library/Application Support")
    } else {
        xdg.filter(|p| p.is_absolute())
            .or_else(|| home.map(|p| p.join(".local/share")))
            .ok_or_else(|| invalid("data directory is unavailable"))?
    };
    Ok(root)
}
/// Machine-file sessions do not have an image workspace.
pub fn machine_media_dir(machine: &Path) -> std::io::Result<PathBuf> {
    let machine = machine
        .canonicalize()
        .or_else(|_| std::path::absolute(machine))?;
    let mut path = machine.into_os_string();
    path.push(".local");
    Ok(PathBuf::from(path).join("media"))
}
/// Image sessions retain media under their owned workspace, including store-local workspaces.
pub fn workspace_media_dir(workspace: &crate::workspace::Workspace<'_>) -> PathBuf {
    workspace.path().join("media")
}
pub fn prospective_path(path: &Path) -> std::io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => return path.canonicalize(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("path has no parent"))?;
    Ok(prospective_path(parent)?.join(
        path.file_name()
            .ok_or_else(|| std::io::Error::other("path has no name"))?,
    ))
}
impl Store {
    /// Journals may live in workspaces, but must never alias immutable blob storage.
    pub fn validate_media_output(&self, path: &Path) -> Result<PathBuf> {
        let path = prospective_path(path)?;
        if path.starts_with(self.root().join("images/blobs").canonicalize()?) {
            return Err(invalid("journal cannot write into blobs"));
        }
        if path.starts_with(prospective_path(&self.root().join("cache/materialized"))?) {
            return Err(invalid("journal cannot write into materialized blobs"));
        }
        #[cfg(unix)]
        if let Ok(metadata) = std::fs::metadata(&path) {
            use std::os::unix::fs::MetadataExt;
            if metadata.is_file() && metadata.nlink() > 1 {
                return Err(invalid("journal cannot share an inode with immutable data"));
            }
        }
        Ok(path)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_platform_defaults_use_data_directories() {
        assert_eq!(
            platform_data_directory(Some("/data/user".into()), None, None, true, false).unwrap(),
            Path::new("/data/user/Library/Application Support")
        );
        assert_eq!(
            platform_data_directory(None, Some("/data/xdg".into()), None, false, false).unwrap(),
            Path::new("/data/xdg")
        );
        for xdg in [None, Some(PathBuf::new()), Some("relative".into())] {
            assert_eq!(
                platform_data_directory(Some("/data/user".into()), xdg, None, false, false)
                    .unwrap(),
                Path::new("/data/user/.local/share")
            );
        }
        assert_eq!(
            platform_data_directory(None, None, Some("C:/Data".into()), false, true).unwrap(),
            Path::new("C:/Data")
        );
        assert_eq!(
            default_store_root().unwrap(),
            data_directory().unwrap().join("unemu")
        );
    }
}
