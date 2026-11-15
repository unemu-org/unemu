// SPDX-License-Identifier: AGPL-3.0-or-later
//! Stable local workspace ownership and durable documents; machine-images §3.1.
//! The store does not interpret platform journals or session bindings.

use crate::{filesystem as host, invalid, store::read_bounded, Result, Store, MAX_SMALL_BLOB};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

/// Ownership spans guest execution, journal finish and offline publication.
/// Standard discard retains its lock. Complete removal serializes acquisition
/// through the store namespace lock so it cannot race a replacement owner.
pub struct Workspace<'a> {
    store: &'a Store,
    path: PathBuf,
    _lock: File,
    _lease: File,
}
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}
impl Store {
    /// Locate a workspace without taking its writer lock, for read-only desktop
    /// inspection while the emulator owns it. Symlinks cannot escape the store.
    pub fn workspace_path(&self, name: &str) -> Result<PathBuf> {
        if !valid_name(name) {
            return Err(invalid("invalid workspace name"));
        }
        let parent = self.root().join("workspaces");
        let path = parent.join(name);
        for part in [&parent, &path] {
            match fs::symlink_metadata(part) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(invalid("workspace cannot be a symlink"))
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(path)
    }

    /// Obtain one owner, rejecting concurrent owners even within this process.
    pub fn workspace(&self, name: &str) -> Result<Workspace<'_>> {
        if !valid_name(name) {
            return Err(invalid("invalid workspace name"));
        }
        let lease = self.lease()?;
        let _namespace = host::exclusive_lock(&self.root().join("locks/workspaces.lock"))?;
        let parent = self.root().join("workspaces");
        fs::create_dir_all(&parent)?;
        host::sync_dir(self.root())?;
        if fs::symlink_metadata(&parent)?.file_type().is_symlink() {
            return Err(invalid("workspace area cannot be a symlink"));
        }
        let path = parent.join(name);
        fs::create_dir_all(&path)?;
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(invalid("workspace cannot be a symlink"));
        }
        host::sync_dir(&parent)?;
        let lock = host::lock_file(&path.join("workspace.lock"))?;
        lock.try_lock()
            .map_err(|e| invalid(format!("workspace is busy: {e}")))?;
        Ok(Workspace {
            store: self,
            path,
            _lock: lock,
            _lease: lease,
        })
    }
    /// Open a stored session without creating directories or lock files.
    pub fn open_workspace(&self, name: &str) -> Result<Workspace<'_>> {
        if !valid_name(name) {
            return Err(invalid("invalid workspace name"));
        }
        let lease = self.lease()?;
        let _namespace = host::exclusive_lock(&self.root().join("locks/workspaces.lock"))?;
        let path = self.root().join("workspaces").join(name);
        if !fs::symlink_metadata(&path)?.is_dir() {
            return Err(invalid("workspace does not exist"));
        }
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join("workspace.lock"))?;
        lock.try_lock()
            .map_err(|e| invalid(format!("workspace is busy: {e}")))?;
        let owner = Workspace {
            store: self,
            path,
            _lock: lock,
            _lease: lease,
        };
        if owner.read("session.toml")?.is_none()
            && owner.read("discard.toml")?.is_none()
            && !owner.contents()?.iter().any(|p| p.components().count() > 1)
        {
            return Err(invalid("workspace does not exist"));
        }
        Ok(owner)
    }
    /// Desktop recovery includes marked preparations that have no session yet.
    pub fn list_desktop_workspaces(&self) -> Result<Vec<String>> {
        let _lease = self.lease()?;
        let parent = self.root().join("workspaces");
        let entries = match fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path().join("desktop.toml").is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }
    pub fn list_workspaces(&self) -> Result<Vec<String>> {
        let _lease = self.lease()?;
        let parent = self.root().join("workspaces");
        if !parent.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path().join("session.toml").is_file() {
                names.push(
                    entry
                        .file_name()
                        .to_str()
                        .ok_or_else(|| invalid("invalid workspace directory"))?
                        .into(),
                );
            }
        }
        names.sort();
        Ok(names)
    }
}
impl Workspace<'_> {
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// The workspace id: its directory name under `workspaces/`.
    pub fn name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }
    /// Move this stopped workspace to a new id, with its lock, journals and
    /// documents. The namespace lock serializes the move with acquisition, so
    /// no other owner can attach to either name meanwhile; the target must not
    /// exist. The owner keeps its lock and continues under the new path.
    pub fn rename(&mut self, name: &str) -> Result<()> {
        if !valid_name(name) {
            return Err(invalid("invalid workspace name"));
        }
        let _namespace = host::exclusive_lock(&self.store.root().join("locks/workspaces.lock"))?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("missing workspace parent"))?
            .to_path_buf();
        let target = parent.join(name);
        match fs::symlink_metadata(&target) {
            Ok(_) => return Err(invalid(format!("workspace {name} exists already"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        fs::rename(&self.path, &target)?;
        host::sync_dir(&parent)?;
        self.path = target;
        Ok(())
    }
    pub fn store(&self) -> &Store {
        self.store
    }
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        document_name(name)?;
        match read_bounded(&self.path.join(name), MAX_SMALL_BLOB) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(crate::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        document_name(name)?;
        if bytes.len() as u64 > MAX_SMALL_BLOB {
            return Err(invalid("workspace document too large"));
        }
        self.store.atomic_write(&self.path.join(name), bytes)
    }
    pub fn remove(&self, name: &str) -> Result<()> {
        document_name(name)?;
        match fs::remove_file(self.path.join(name)) {
            Ok(()) => host::sync_dir(&self.path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    /// Discard affects only the selected workspace. The stable lock survives.
    pub fn discard(self) -> Result<()> {
        self.discard_contents()
    }

    /// Discard like `discard`, keeping ownership so the caller can set the
    /// workspace up again before anyone else can start it.
    pub fn clear(&self) -> Result<()> {
        self.discard_contents()
    }

    /// Remove a discarded workspace under the same namespace lock used by
    /// acquisition. A stale owner can never attach to an unlinked lock inode.
    pub fn discard_and_remove(self) -> Result<()> {
        let _namespace = host::exclusive_lock(&self.store.root().join("locks/workspaces.lock"))?;
        self.discard_contents()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("missing workspace parent"))?;
        fs::remove_dir_all(&self.path)?;
        host::sync_dir(parent)
    }

    fn discard_contents(&self) -> Result<()> {
        for name in ["session.toml", "transaction.toml", "discard.toml"] {
            if let Some(bytes) = self.read(name)? {
                let table: toml::Table = std::str::from_utf8(&bytes)
                    .map_err(|e| invalid(e.to_string()))?
                    .parse()
                    .map_err(|e: toml::de::Error| invalid(e.to_string()))?;
                if table.get("schema").and_then(toml::Value::as_integer) != Some(1) {
                    return Err(invalid("unsupported workspace schema"));
                }
            }
        }
        let _locks = self.lock_contents()?;
        self.write("discard.toml", b"schema = 1\n")?;
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name();
                let tombstone = if name.to_string_lossy().starts_with("discarded-") {
                    entry.path()
                } else {
                    let target = self
                        .path
                        .join(format!("discarded-{}", name.to_string_lossy()));
                    fs::rename(entry.path(), &target)?;
                    host::sync_dir(&self.path)?;
                    target
                };
                fs::remove_dir_all(tombstone)?;
                host::sync_dir(&self.path)?;
            }
        }
        self.remove("transaction.toml")?;
        self.remove("session.toml")?;
        self.remove("discard.toml")?;
        self.remove("desktop.toml")?;
        host::sync_dir(&self.path)
    }
    /// All local data affected by discard, independent of provider file names.
    pub fn contents(&self) -> Result<Vec<PathBuf>> {
        fn visit(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    visit(root, &entry.path(), output)?;
                } else if entry.file_type()?.is_file()
                    && entry.path().extension().is_none_or(|e| e != "lock")
                {
                    output.push(
                        entry
                            .path()
                            .strip_prefix(root)
                            .map_err(|e| invalid(e.to_string()))?
                            .to_path_buf(),
                    );
                }
            }
            Ok(())
        }
        let mut output = Vec::new();
        visit(&self.path, &self.path, &mut output)?;
        output.sort();
        Ok(output)
    }
    /// Lock generic sidecars recursively; providers may choose their own directories.
    pub fn lock_contents(&self) -> Result<Vec<File>> {
        fn visit(directory: &Path, owned_lock: &Path, locks: &mut Vec<File>) -> Result<()> {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    visit(&entry.path(), owned_lock, locks)?;
                } else if entry.file_type()?.is_file()
                    && entry.path().extension().is_some_and(|e| e == "lock")
                    && entry.path() != owned_lock
                {
                    let lock = host::lock_file(&entry.path())?;
                    lock.try_lock()
                        .map_err(|e| invalid(format!("media writer still active: {e}")))?;
                    locks.push(lock);
                }
            }
            Ok(())
        }
        let mut locks = Vec::new();
        visit(&self.path, &self.path.join("workspace.lock"), &mut locks)?;
        Ok(locks)
    }
}
fn document_name(name: &str) -> Result<()> {
    if matches!(
        name,
        "session.toml" | "transaction.toml" | "discard.toml" | "desktop.toml"
    ) {
        Ok(())
    } else {
        Err(invalid("unsupported workspace document"))
    }
}
