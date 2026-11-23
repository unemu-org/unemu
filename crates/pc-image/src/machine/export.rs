// SPDX-License-Identifier: AGPL-3.0-or-later
//! Store-owned next-run config and current media under stopped-machine locks.

use crate::*;
use image_store::{ResolvedImage, Store};
use journal_files::{SectorSource, SparseOverlay, VerifiedBase};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
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

/// A verified image under the same lock a running machine owns exclusively.
/// Shared, existing locks prevent starts and deletion without creating files.
pub struct Snapshot {
    config: Config,
    document: unemu_core_api::ConfigDocumentBuf,
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
    let mut document = crate::provider::export_document(&config, &descriptors, store)?;
    let mut pending = BTreeMap::new();
    for insert in &record.pending {
        let selected = super::select_insert(store, &machine, insert)?;
        let (_, medium, format) = selected
            .entries
            .into_iter()
            .next()
            .ok_or_else(|| invalid("pending insert has no selected medium"))?;
        let id = super::next_insert_id(&document.config);
        let digest = medium.blob.to_config()?;
        let size = selected
            .resolved
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&medium.blob))
            .ok_or_else(|| invalid("missing pending medium descriptor"))?
            .decoded_size()?;
        if !document.content.iter().any(|row| row.digest == digest) {
            document.content.push(pc_config::launch::ContentRow {
                digest,
                sha256: crate::provider::hash_array(&medium.blob)?,
                size,
            });
        }
        document.config.media.push(Medium {
            id: id.clone(),
            source: format!("assets/{id}.img"),
            head: digest,
            name: Some(medium.label.clone()),
            policy: if insert.journal {
                Policy::Journal
            } else {
                Policy::Readonly
            },
            geometry: pc_config::image::Geometry::of(assembly::DriveKind::Floppy {
                geometry: format,
            }),
        });
        if insert.journal {
            document.records.push(pc_config::launch::RecordRow {
                medium: id.clone(),
                name: format!("kept-{id}.upfj"),
                keep_generations: assembly::WriteMode::DEFAULT_KEEP_GENERATIONS,
            });
        }
        let slot = &mut document.config.slots[selected.slot_index];
        slot.catalogue_mode = true;
        slot.catalogue.insert(0, id.clone());
        slot.inserted = Some(id);
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
        document: unemu_core_api::ConfigDocumentBuf::new(
            pc_config::launch::MEDIA_TYPE,
            pc_config::launch::encode(&document)?,
        ),
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

    pub fn document(&self) -> unemu_core_api::ConfigDocument<'_> {
        self.document.as_document()
    }

    pub fn current(
        &self,
        store: &Store,
        label: &str,
    ) -> Result<Box<dyn unemu_core_api::ContentReader + '_>> {
        let (index, slot) = self
            .config
            .slots
            .iter()
            .enumerate()
            .find(|(_, slot)| self.name(slot).is_ok_and(|name| name == label))
            .ok_or_else(|| invalid(format!("no slot {label}")))?;
        let (base, journal) = self
            .source(store, index, slot)?
            .ok_or_else(|| invalid(format!("slot {label} is empty")))?;
        let overlay = self.overlay(journal.as_deref(), &base)?;
        Ok(Box::new(Current { base, overlay }))
    }
}

struct Current {
    base: VerifiedBase,
    overlay: SparseOverlay,
}
impl unemu_core_api::ContentReader for Current {
    fn size(&self) -> u64 {
        self.base.byte_len()
    }
    fn read_at(
        &self,
        offset: u64,
        bytes: &mut [u8],
    ) -> std::result::Result<(), unemu_core_api::Error> {
        use unemu_core_api::{Error, ErrorKind};
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.size())
        {
            return Err(Error::new(
                ErrorKind::Usage,
                "export read is outside the medium",
            ));
        }
        let checked = || self.base.check_unchanged().map_err(content_error);
        checked()?;
        let mut position = offset;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let start = (position % 512) as usize;
            if start == 0 && remaining.len() >= 512 {
                let count = remaining.len() / 512 * 512;
                self.overlay
                    .read_range(position / 512, &mut remaining[..count])
                    .map_err(content_error)?;
                remaining = &mut remaining[count..];
                position += count as u64;
            } else {
                let sector = self
                    .overlay
                    .read_sector(position / 512)
                    .map_err(content_error)?;
                let count = remaining.len().min(512 - start);
                remaining[..count].copy_from_slice(&sector[start..start + count]);
                remaining = &mut remaining[count..];
                position += count as u64;
            }
        }
        checked()
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

fn content_error(error: io::Error) -> unemu_core_api::Error {
    use unemu_core_api::{Error, ErrorKind};
    let kind = if error.to_string() == "base file changed after verification" {
        ErrorKind::Content
    } else {
        ErrorKind::Storage
    };
    Error::new(kind, error.to_string())
}
