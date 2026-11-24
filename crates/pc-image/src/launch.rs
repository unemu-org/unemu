// SPDX-License-Identifier: AGPL-3.0-or-later
//! Stored PC images as launch objects, without opening journals on the worker.
//! The launch document, verified flat content and session records cross the
//! thread boundary separately (backlog/docs/design/core-contract.md §14.2.4).
//! Bases stay immutable; the journal overlay owns every written sector.

use crate::workspace::{PreparedSession, Transfer};
use image_store::{machine::PendingInsert, workspace::Workspace, Descriptor, Digest};
use journal_files::{SectorSource, VerifiedBase};
use pc_config::launch;
use std::{collections::BTreeMap, path::PathBuf, rc::Rc};
use unemu_core_api::{
    ConfigDocumentBuf, Content, ContentReader, ContentSource, Core, Error, ErrorKind, Record,
    Storage,
};

type Identity = ([u8; 32], u64);

/// A stored start and the image it pins. Pending inserts are confirmed by the
/// caller only after creation succeeds, as with `machine::prepare`.
pub struct Prepared {
    pub prepared: core_launch::Prepared,
    pub image: Digest,
    pub inserted: Vec<PendingInsert>,
}

/// Prepare an image's workspace, retaining its existing pin unless fresh.
/// The caller keeps workspace ownership through the run and journal finish.
pub fn prepare_workspace(
    owner: &Workspace<'_>,
    tag: &str,
    fresh: bool,
    core: &'static dyn Core,
    title: String,
) -> crate::Result<Prepared> {
    from_session(
        crate::workspace::prepare_start(owner, tag, fresh)?,
        core,
        title,
    )
}

/// Prepare the exact image selected by a library, even if its tag moved.
pub fn prepare_pinned(
    owner: &Workspace<'_>,
    image: &Descriptor,
    fresh: bool,
    core: &'static dyn Core,
    title: String,
) -> crate::Result<Prepared> {
    from_session(
        crate::workspace::prepare_pinned(owner, image, fresh)?,
        core,
        title,
    )
}

/// Prepare a machine with its settings and pending inserts, without confirming it.
pub fn prepare_machine(
    owner: &Workspace<'_>,
    core: &'static dyn Core,
    title: String,
) -> crate::Result<Prepared> {
    machine_parts(crate::machine::prepare(owner)?, core, title)
}

/// Prepare a disposable copy with its own journals and the source's settings.
pub fn prepare_copy(
    source: &Workspace<'_>,
    copy: &Workspace<'_>,
    core: &'static dyn Core,
    title: String,
) -> crate::Result<Prepared> {
    machine_parts(crate::machine::prepare_copy(source, copy)?, core, title)
}

/// Prepare an existing disposable copy again after a power cycle.
pub fn resume_copy(
    copy: &Workspace<'_>,
    core: &'static dyn Core,
    title: String,
) -> crate::Result<Prepared> {
    machine_parts(crate::machine::resume_copy(copy)?, core, title)
}

fn machine_parts(
    machine: crate::machine::Prepared,
    core: &'static dyn Core,
    title: String,
) -> crate::Result<Prepared> {
    let mut prepared = from_session(machine.session, core, title)?;
    prepared.inserted = machine.inserted;
    Ok(prepared)
}

/// Freeze an edited session after created disks and kept catalogues have been
/// applied. Synthetic sessions without a launch document are refused.
pub fn from_session(
    session: PreparedSession,
    core: &'static dyn Core,
    title: String,
) -> crate::Result<Prepared> {
    let image = session.image.clone();
    let identities: BTreeMap<_, _> = session
        .bases
        .iter()
        .map(|(path, base)| (path.clone(), base.identity().clone()))
        .collect();
    let transfer = session.into_transfer()?;
    let input = transfer
        .launch
        .ok_or_else(|| crate::invalid("this session has no launch document"))?;
    let config = launch::encode(&input.document)?;
    let mut entries = BTreeMap::new();
    for row in &input.document.content {
        let entry = if let Some(medium) = input
            .document
            .config
            .media
            .iter()
            .find(|m| m.head == Some(row.digest))
        {
            let path = input
                .sources
                .get(&PathBuf::from(&medium.source))
                .ok_or_else(|| crate::invalid("missing launch base path"))?;
            if let Some(chain) = transfer.zero.get(path) {
                Entry::Zero(
                    chain.try_clone()?,
                    identities
                        .get(path)
                        .ok_or_else(|| crate::invalid("missing verified zero base"))?
                        .clone(),
                )
            } else {
                match transfer
                    .bases
                    .get(path)
                    .ok_or_else(|| crate::invalid("missing verified launch base"))?
                {
                    Transfer::File(file, identity, metadata) => {
                        Entry::File(file.try_clone()?, identity.clone(), metadata.clone())
                    }
                    Transfer::Memory(bytes) => Entry::Bytes(bytes.clone()),
                }
            }
        } else {
            let bytes = input
                .small
                .get(&row.digest.sha256())
                .ok_or_else(|| crate::invalid("missing verified small content"))?;
            Entry::Bytes(bytes.clone())
        };
        // Check each row, even when two heads describe the same flat bytes.
        entry
            .check((row.sha256, row.size))
            .map_err(|e| crate::invalid(e.message))?;
        entries.entry((row.sha256, row.size)).or_insert(entry);
    }
    let mut records = BTreeMap::new();
    for row in &input.document.records {
        let native = input
            .document
            .config
            .media
            .iter()
            .find(|m| m.id == row.medium)
            .and_then(|m| m.zero.as_ref());
        let base = if let Some(zero) = native {
            journal_files::Header {
                base_digest: crate::provider::hash_array(&crate::zero::zero_digest(zero.size))?,
                base_length: zero.size,
                anchor_generation: 0,
            }
        } else {
            let digest = if row.medium == launch::CMOS_MEDIUM {
                input.document.config.nvram.as_ref().map(|n| n.head)
            } else {
                input
                    .document
                    .config
                    .media
                    .iter()
                    .find(|m| m.id == row.medium)
                    .and_then(|m| m.head)
            }
            .ok_or_else(|| crate::invalid("missing journal base binding"))?;
            let content = input
                .document
                .content
                .iter()
                .find(|c| c.digest == digest)
                .ok_or_else(|| crate::invalid("missing journal base row"))?;
            journal_files::Header {
                base_digest: content.sha256,
                base_length: content.size,
                anchor_generation: 0,
            }
        };
        let path = input
            .records
            .get(&row.name)
            .ok_or_else(|| crate::invalid("missing journal file binding"))?;
        let source = if row.medium == launch::CMOS_MEDIUM {
            input.document.config.nvram.as_ref().map(|n| &n.source)
        } else {
            input
                .document
                .config
                .media
                .iter()
                .find(|m| m.id == row.medium)
                .map(|m| &m.source)
        };
        let metadata = source
            .and_then(|source| input.sources.get(&PathBuf::from(source)))
            .and_then(|p| transfer.bases.get(p))
            .and_then(|base| match base {
                Transfer::File(_, _, metadata) => Some(metadata.clone()),
                Transfer::Memory(_) => None,
            });
        records.insert(
            row.name.clone(),
            Journal {
                path: path.clone(),
                base,
                metadata,
            },
        );
    }
    Ok(Prepared {
        prepared: core_launch::Prepared::from_parts(core_launch::Parts {
            core,
            platform: launch::PLATFORM,
            title,
            notes: Vec::new(),
            config: ConfigDocumentBuf::new(launch::MEDIA_TYPE, config),
            content: Box::new(ContentPlan { entries }),
            storage: Box::new(StoragePlan { records }),
            options: Vec::new(),
        }),
        image,
        inserted: Vec::new(),
    })
}

/// Reusable verified receipts. Files travel as handles, compressed floppies as
/// bytes, and zero-rooted disks as sparse parts, never as a materialized disk.
pub struct ContentPlan {
    entries: BTreeMap<Identity, Entry>,
}

enum Entry {
    Bytes(Vec<u8>),
    File(std::fs::File, journal_files::Header, Box<std::fs::Metadata>),
    Zero(crate::zero::ZeroChain, journal_files::Header),
}

enum Opened {
    Bytes(Vec<u8>),
    Base(Box<VerifiedBase>),
}

fn content_error(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Content, error.to_string())
}

fn base_error(error: std::io::Error) -> Error {
    // The media receipt reports a changed file through io::Error; other I/O
    // failures belong to the host, rather than to the content identity.
    let kind = if error.to_string() == "base file changed after verification" {
        ErrorKind::Content
    } else {
        ErrorKind::Storage
    };
    Error::new(kind, error.to_string())
}

impl Entry {
    fn check(&self, identity: Identity) -> Result<(), Error> {
        let (digest, size) = match self {
            Self::Bytes(bytes) => (
                pc_config::image::Digest::of(bytes).sha256(),
                bytes.len() as u64,
            ),
            Self::File(_, header, _) => (header.base_digest, header.base_length),
            Self::Zero(_, header) => (header.base_digest, header.base_length),
        };
        if (digest, size) != identity {
            return Err(content_error(
                "verified content does not match its launch row",
            ));
        }
        Ok(())
    }

    fn open(&self, identity: Identity) -> Result<Opened, Error> {
        self.check(identity)?;
        match self {
            Self::Bytes(bytes) => Ok(Opened::Bytes(bytes.clone())),
            Self::File(file, header, metadata) => VerifiedBase::from_verified_file(
                file.try_clone().map_err(base_error)?,
                header.clone(),
                *metadata.clone(),
            )
            .map(|base| Opened::Base(Box::new(base)))
            .map_err(base_error),
            Self::Zero(chain, _) => chain
                .mount()
                .map(|base| Opened::Base(Box::new(base)))
                .map_err(content_error),
        }
    }
}

impl core_launch::ContentPlan for ContentPlan {
    fn open(&self) -> Result<Box<dyn ContentSource>, Error> {
        let entries = self
            .entries
            .iter()
            .map(|(identity, entry)| Ok((*identity, entry.open(*identity)?)))
            .collect::<Result<_, Error>>()?;
        Ok(Box::new(Source { entries }))
    }
}

struct Source {
    entries: BTreeMap<Identity, Opened>,
}
impl ContentSource for Source {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        match self.entries.get(&(*sha256, size)) {
            Some(Opened::Bytes(bytes)) => Ok(Content::Bytes(bytes.clone())),
            Some(Opened::Base(base)) => {
                base.check_unchanged().map_err(base_error)?;
                Ok(Content::Reader(Box::new(Reader(base.as_ref().clone()))))
            }
            None => Err(content_error(
                "content digest and size are not listed by this launch",
            )),
        }
    }
}

struct Reader(VerifiedBase);
impl ContentReader for Reader {
    fn size(&self) -> u64 {
        self.0.byte_len()
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.size())
        {
            return Err(Error::new(
                ErrorKind::Usage,
                "content read is outside its launch row",
            ));
        }
        self.0.check_unchanged().map_err(base_error)?;
        let mut position = offset;
        let mut remaining = bytes;
        if !position.is_multiple_of(journal_files::SECTOR_LEN as u64) {
            let sector = self
                .0
                .read_sector(position / journal_files::SECTOR_LEN as u64)
                .map_err(base_error)?;
            let start = (position % journal_files::SECTOR_LEN as u64) as usize;
            let count = remaining.len().min(journal_files::SECTOR_LEN - start);
            remaining[..count].copy_from_slice(&sector[start..start + count]);
            remaining = &mut remaining[count..];
            position += count as u64;
        }
        if !remaining.is_empty() {
            self.0
                .read_range(position / journal_files::SECTOR_LEN as u64, remaining)
                .map_err(base_error)?;
        }
        self.0.check_unchanged().map_err(base_error)
    }
}

/// The launch's record table over existing journal paths, including kept
/// catalogues outside the workspace. Opening the plan takes no journal lock;
/// each record takes its own `FILE.lock` until the machine releases it.
pub struct StoragePlan {
    records: BTreeMap<String, Journal>,
}
#[derive(Clone)]
struct Journal {
    path: PathBuf,
    base: journal_files::Header,
    metadata: Option<Box<std::fs::Metadata>>,
}
impl core_launch::StoragePlan for StoragePlan {
    fn open(&self) -> Result<Rc<dyn Storage>, Error> {
        Ok(Rc::new(Files {
            records: self.records.clone(),
        }))
    }
}
struct Files {
    records: BTreeMap<String, Journal>,
}
impl Storage for Files {
    fn open(&self, name: &str) -> Result<Box<dyn Record>, Error> {
        let journal = self
            .records
            .get(name)
            .ok_or_else(|| Error::new(ErrorKind::Usage, "record is not listed by this launch"))?;
        Ok(Box::new(journal_files::JournalFile::open(
            &journal.path,
            &journal.base,
            journal.metadata.as_deref(),
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_launch::ContentPlan as _;

    #[test]
    fn plans_and_prepared_are_send() {
        fn send<T: Send>() {}
        send::<ContentPlan>();
        send::<StoragePlan>();
        send::<Prepared>();
    }

    #[test]
    fn content_plan_checks_every_byte_and_file_receipt_against_its_row() {
        let temp = image_store::test_support::TempDir::new("launch-receipts");
        for (index, bytes) in [vec![0x37; 64], vec![0x42; 32768], vec![0xf6; 368640]]
            .into_iter()
            .enumerate()
        {
            let digest = pc_config::image::Digest::of(&bytes).sha256();
            let size = bytes.len() as u64;
            let path = temp.path().join(format!("base-{index}"));
            std::fs::write(&path, &bytes).unwrap();
            let base = VerifiedBase::open(&path).unwrap();
            let entries = [
                Entry::Bytes(bytes),
                Entry::File(
                    base.try_clone_verified_file().unwrap(),
                    base.identity().clone(),
                    Box::new(base.metadata().unwrap().clone()),
                ),
            ];
            for entry in entries {
                entry.open((digest, size)).unwrap();
                for wrong in [([0x99; 32], size), (digest, size + 1)] {
                    assert_eq!(entry.open(wrong).err().unwrap().kind, ErrorKind::Content);
                }
                let plan = ContentPlan {
                    entries: [((digest, size + 1), entry)].into(),
                };
                // A bad receipt fails opening the plan, before a core can ask
                // for just the other, valid rows.
                assert_eq!(plan.open().err().unwrap().kind, ErrorKind::Content);
            }
        }
    }
}
