// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only raw media exports of stopped library machines through SectorSource.

use crate::*;
use image_store::{ResolvedImage, Store};
use journal_files::{SectorSource, SparseOverlay, VerifiedBase};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

/// A current medium, named like the machine's other library actions.
#[derive(Debug)]
pub struct MediaRow {
    pub slot: String,
    pub kind: &'static str,
    pub bytes: u64,
    pub changed: bool,
    pub selection: &'static str,
    pub name: Option<String>,
}

/// The exact length and SHA-256 of the newly written raw bytes.
pub struct Exported {
    pub bytes: u64,
    pub sha256: String,
}

/// A verified image under the same lock a running machine owns exclusively.
/// Shared, existing locks prevent starts and deletion without creating files.
pub struct Snapshot {
    config: Config,
    image: ResolvedImage,
    names: BTreeMap<(String, u8), String>,
    kinds: BTreeMap<String, &'static str>,
    pending: BTreeMap<usize, Pending>,
    workspace: PathBuf,
    _locks: Vec<File>,
}

struct Pending {
    image: ResolvedImage,
    digest: Digest,
    name: String,
    journal: Option<PathBuf>,
}

fn shared(path: &Path) -> Result<File> {
    let file = File::open(path)?;
    file.try_lock_shared()
        .map_err(|e| invalid(format!("export is busy: {e}")))?;
    Ok(file)
}

pub fn open(store: &Store, id: &str) -> Result<Snapshot> {
    open_with_hook(store, id, || Ok(()))
}

/// Test boundary immediately before acquiring the workspace lock.
#[doc(hidden)]
pub fn open_with_hook(
    store: &Store,
    id: &str,
    before_lock: impl FnOnce() -> Result<()>,
) -> Result<Snapshot> {
    let lease = shared(&store.root().join("locks/store.lock"))?;
    let namespace = shared(&store.root().join("locks/workspaces.lock"))?;
    let workspace = store.workspace_path(id)?;
    before_lock()?;
    let lock = match File::open(workspace.join("workspace.lock")) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(invalid(format!(
                "no machine {id}; use unemu image machine ls to list machine IDs"
            )));
        }
        Err(error) => return Err(error.into()),
    };
    match lock.try_lock_shared() {
        Ok(()) => (),
        Err(fs::TryLockError::WouldBlock) => {
            return Err(invalid(format!("machine {id} is running; stop it first")))
        }
        Err(fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    let record = store.read_machines()?.get(id).cloned().ok_or_else(|| {
        invalid(format!(
            "no machine {id}; use unemu image machine ls to list machine IDs"
        ))
    })?;
    if let Some(from) = &record.migrated_from {
        if super::holds_data(&store.workspace_path(from)?)? {
            return Err(invalid("machine migration pending; open the machine first"));
        }
    }
    drop(namespace);
    for name in ["transaction.toml", "discard.toml"] {
        if workspace.join(name).try_exists()? {
            return Err(invalid(format!(
                "machine has pending {name}; finish it before exporting"
            )));
        }
    }
    let pinned = super::pinned_image(store, id)?;
    let prepared = crate::provider::load_pinned(store, &pinned, false, false)?;
    let (config, descriptors, _) =
        crate::provider::select_config(store, &prepared, record.settings.as_ref())?;
    let machine = crate::provider::prepare(config.clone(), &descriptors, store)?.machine;
    let mut pending = BTreeMap::new();
    for insert in &record.pending {
        let selected = super::select_insert(store, &machine, insert)?;
        let (_, medium, _) = selected
            .entries
            .into_iter()
            .next()
            .ok_or_else(|| invalid("pending insert has no selected medium"))?;
        pending.insert(
            selected.slot_index,
            Pending {
                image: selected.resolved,
                digest: medium.blob,
                name: medium.label,
                journal: insert.journal.then(|| {
                    workspace.join("media").join(format!(
                        "{}-{}.upfj",
                        insert.set.digest.hex(),
                        medium.id
                    ))
                }),
            },
        );
    }
    let names = super::drive_names(&config)?;
    let mut kinds = BTreeMap::new();
    crate::drive_tables(
        &mut config.machine.clone(),
        |controller, kind, _| -> Result<()> {
            kinds.insert(
                controller.to_owned(),
                if kind == "ide" { "fixed" } else { "floppy" },
            );
            Ok(())
        },
    )?;
    Ok(Snapshot {
        config,
        image: prepared.image,
        names,
        kinds,
        pending,
        workspace,
        _locks: vec![lease, lock],
    })
}

impl Snapshot {
    fn name(&self, slot: &Slot) -> Result<&str> {
        self.names
            .get(&(slot.controller.clone(), slot.unit))
            .map(String::as_str)
            .ok_or_else(|| invalid("missing library slot name"))
    }

    fn medium(&self, slot: &Slot) -> Option<&Medium> {
        self.config
            .media
            .iter()
            .find(|m| Some(&m.id) == slot.inserted.as_ref())
    }

    fn source(
        &self,
        store: &Store,
        index: usize,
        slot: &Slot,
    ) -> Result<Option<(VerifiedBase, Option<PathBuf>)>> {
        if let Some(pending) = self.pending.get(&index) {
            return Ok(Some((
                crate::disk::export_content(&pending.image, &pending.digest)?,
                pending.journal.clone(),
            )));
        }
        self.medium(slot)
            .map(|medium| {
                Ok((
                    crate::disk::export_base(store, medium, &self.image)?,
                    (medium.policy == Policy::Journal).then(|| {
                        self.workspace
                            .join("media")
                            .join(format!("{}.upfj", medium.id))
                    }),
                ))
            })
            .transpose()
    }

    fn overlay(&self, journal: Option<&Path>, base: &VerifiedBase) -> Result<SparseOverlay> {
        let Some(path) = journal else {
            return Ok(SparseOverlay::new(std::rc::Rc::new(base.clone())));
        };
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(SparseOverlay::new(std::rc::Rc::new(base.clone())));
            }
            Err(e) => return Err(e.into()),
        };
        let scan = journal_files::scan_overlay_reader(&mut file, base, base.byte_len())?;
        if scan.report.needs_recovery()
            || !matches!(scan.report.status, journal_files::Status::Valid)
        {
            return Err(invalid(format!(
                "journal {} needs recovery; export does not recover journals",
                path.display()
            )));
        }
        scan.overlay
            .ok_or_else(|| invalid("journal needs recovery; no readable overlay"))
    }

    pub fn media(&self, store: &Store) -> Result<Vec<MediaRow>> {
        let mut rows = Vec::new();
        for (index, slot) in self.config.slots.iter().enumerate() {
            let (bytes, changed) = if let Some((base, journal)) = self.source(store, index, slot)? {
                let overlay = self.overlay(journal.as_deref(), &base)?;
                (base.byte_len(), has_changes(&base, &overlay)?)
            } else {
                (0, false)
            };
            rows.push(MediaRow {
                slot: self.name(slot)?.into(),
                kind: self
                    .kinds
                    .get(&slot.controller)
                    .copied()
                    .ok_or_else(|| invalid("missing library slot kind"))?,
                bytes,
                changed,
                selection: if self.pending.contains_key(&index) {
                    "pending-insert"
                } else if self.medium(slot).is_some() {
                    "configured"
                } else {
                    "empty"
                },
                name: self
                    .pending
                    .get(&index)
                    .map(|p| p.name.clone())
                    .or_else(|| {
                        self.medium(slot).map(|m| {
                            m.name.clone().unwrap_or_else(|| {
                                format!("{} image", self.name(slot).unwrap_or("slot"))
                            })
                        })
                    }),
            });
        }
        rows.sort_by(|a, b| a.slot.cmp(&b.slot));
        Ok(rows)
    }

    pub fn export(&self, store: &Store, slot: &str, to: &Path) -> Result<Exported> {
        self.export_with_hook(store, slot, to, |_| Ok(()))
    }

    /// Fault-injection boundary after each streamed write, before publication.
    /// An error removes only this export's temporary file.
    #[doc(hidden)]
    pub fn export_with_hook(
        &self,
        store: &Store,
        slot: &str,
        to: &Path,
        hook: impl FnMut(u64) -> io::Result<()>,
    ) -> Result<Exported> {
        let (index, selected) = self
            .config
            .slots
            .iter()
            .enumerate()
            .find(|(_, s)| self.name(s).is_ok_and(|name| name == slot))
            .ok_or_else(|| invalid(format!("no slot {slot}; omit --slot to list media")))?;
        let parent = to
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let path = parent.canonicalize()?.join(
            to.file_name()
                .ok_or_else(|| invalid("output needs a file name"))?,
        );
        if path.starts_with(store.root()) {
            return Err(invalid(
                "export destination must be outside the read-only store",
            ));
        }
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(invalid(format!("output {} already exists", path.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let (base, journal) = self
            .source(store, index, selected)?
            .ok_or_else(|| invalid(format!("slot {slot} is empty")))?;
        let overlay = self.overlay(journal.as_deref(), &base)?;
        let mut created = Created::new(&path)?;
        let result = (|| -> Result<Exported> {
            let mut output = Progress {
                output: BufWriter::new(&created.file),
                written: 0,
                hook,
            };
            let sha256 = journal_files::layer::copy_and_digest(&overlay, &mut output)?;
            output.flush()?;
            created.file.sync_all()?;
            Ok(Exported {
                bytes: output.written,
                sha256,
            })
        })();
        match result {
            Ok(exported) => {
                fs::hard_link(
                    created
                        .path
                        .as_ref()
                        .ok_or_else(|| invalid("missing export temp file"))?,
                    &path,
                )?;
                created.remove()?;
                Ok(exported)
            }
            Err(error) => match created.remove() {
                Ok(()) => Err(error),
                Err(cleanup) => Err(invalid(format!("{error}; {cleanup}"))),
            },
        }
    }
}

fn has_changes(base: &VerifiedBase, overlay: &SparseOverlay) -> Result<bool> {
    let mut changed = false;
    let mut replacements = overlay.replacements().peekable();
    let mut buffer = [0; 64 * 1024];
    for first in (0..base.byte_len() / 512).step_by(buffer.len() / 512) {
        let count = (base.byte_len() - first * 512).min(buffer.len() as u64) as usize;
        base.read_range(first, &mut buffer[..count])?;
        while let Some(&(&lba, sector)) = replacements.peek() {
            if lba >= first + (count / 512) as u64 {
                break;
            }
            let offset = (lba - first) as usize * 512;
            changed |= buffer[offset..offset + 512] != sector[..];
            replacements.next();
        }
    }
    Ok(changed)
}

struct Progress<W, F> {
    output: W,
    written: u64,
    hook: F,
}
impl<W: Write, F: FnMut(u64) -> io::Result<()>> Write for Progress<W, F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.output.write(bytes)?;
        self.written += count as u64;
        (self.hook)(self.written)?;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

struct Created {
    file: File,
    path: Option<PathBuf>,
}
impl Created {
    fn new(destination: &Path) -> io::Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let mut name = std::ffi::OsString::from(".");
            name.push(
                destination
                    .file_name()
                    .ok_or_else(|| io::Error::other("output needs a file name"))?,
            );
            name.push(format!(
                ".unemu-export-{}-{}.part",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let path = destination.with_file_name(name);
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        path: Some(path),
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn remove(&mut self) -> io::Result<()> {
        if let Some(path) = &self.path {
            fs::remove_file(path)?;
            self.path = None;
        }
        Ok(())
    }
}

impl Drop for Created {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}
