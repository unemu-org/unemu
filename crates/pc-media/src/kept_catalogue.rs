// SPDX-License-Identifier: AGPL-3.0-or-later
//! A library machine's floppy catalogues as they were at Stop, back at its
//! next Start; machine-images §3.2 (Machines: Start, Reset, Gaming mode) and
//! §7 (the desktop's media picker and Catalogue submenu).
//!
//! A running machine's drive catalogue grows as disks go in: a set of the
//! library (Insert…), an image file of this computer (Browse for an image
//! file…), the disks Create from files… makes. Those join the catalogue for
//! the run only, so at Stop the window hands each floppy drive's catalogue to
//! the library worker, which keeps it in the machine's workspace,
//! `created/catalogue.toml` of its media folder, beside the created disks'
//! pending insert. It never goes into the store's machine list: released
//! versions read that list with every unknown field refused. A catalogue
//! that is the image's own (its own disks only, the first in the drive)
//! leaves no record.
//!
//! Each disk is kept by what identifies it, never by a host path: one of the
//! image's own disks by its name and SHA-256, a set's disk by the set's
//! manifest, the medium's id and the tag it came from, any other image by
//! its SHA-256 among the machine's created images. An image file of this
//! computer is copied there at Stop, a read-only base named by its SHA-256,
//! so a later change or removal of the file never changes the machine's
//! disk; a journal Insert put beside the file is copied into the machine's
//! media folder too, so Duplicate and Gaming mode copy it with the machine
//! and never share it. Each disk keeps its write setting (a journal, or
//! write protection), its format and its place, and the record says which
//! disk each drive held, or that it was empty. The same disk inserted again
//! is kept once.
//!
//! At Start the catalogue comes back in its order before the machine's first
//! instruction, the way pending inserts and created disks go in: into the
//! prepared session's drive, with the disk that was in it in the drive from
//! power-on, as the guest sees a catalogue's first disk. A pending insert
//! for the drive wins the drive: its disks lead the catalogue and its chosen
//! disk goes in, as without a kept catalogue, and the kept disks follow in
//! their order; a kept disk the insert also brings appears once, as the
//! insert has it. The image's own disks the record does not name follow
//! last. A kept disk that cannot come back (its set or image is gone, or the
//! drive no longer takes it) stays out with a note; a record that cannot be
//! read, or that a newer version wrote, leaves the whole catalogue out with
//! a note, and a Stop then keeps nothing rather than replace a newer record.
//! Nothing here stops a Start.
//!
//! The record follows the machine's media: Duplicate and Gaming mode copy it,
//! and a Gaming-mode copy keeps its own at its Stop, for its power cycles and
//! Keep as new machine…, unless it is discarded; Reset to image… drops it.
//! Save as new image… keeps it, as it keeps created disks, but with fresh
//! journals: the kept disks' writes start afresh, and the image's own disks
//! are matched by name where their bytes changed.

use crate::created_media::{hex, FOLDER};
use crate::media_dialog::FloppySlot;
use crate::media_picker::{drive_name, CatalogueDisk, SetEntry};
use assembly::{FloppyMedium, ImageSource, WriteMode};
use image_store::{media_set::CatalogueSource, Descriptor, Digest, Store};
use journal_files::SectorSource as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use unemu_core_api::{MediaAction, SlotId};

type Result<T> = std::result::Result<T, String>;

/// The record in a workspace's created-image folder.
const RECORD: &str = "catalogue.toml";

/// One disk of a kept catalogue, by what identifies it. `format` is the
/// disk's own format (`360K`), which the waiting lists show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum KeptDisk {
    /// One of the image's own disks, with the image's write setting.
    Own {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// A disk of an installation media set of the library.
    Set {
        /// The disk's own label in the set.
        name: String,
        set: Descriptor,
        /// The tag the disk came from, for the drive's tooltip: never a
        /// release alias, which moves.
        reference: String,
        medium: String,
        #[serde(default)]
        journal: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// An image the machine keeps among its created images: a disk Create
    /// from files… made, or a copy of an image file of this computer.
    Image {
        name: String,
        sha256: String,
        #[serde(default)]
        journal: bool,
        /// The journal's file name in the machine's media folder, when it is
        /// not `<sha-256>.upfj`: never a path, so a copy of the machine has
        /// its own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        journal_file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
}

impl KeptDisk {
    pub fn name(&self) -> &str {
        match self {
            Self::Own { name, .. } | Self::Set { name, .. } | Self::Image { name, .. } => name,
        }
    }

    fn format(&self) -> Option<assembly::FloppyGeometry> {
        let (Self::Own { format, .. } | Self::Set { format, .. } | Self::Image { format, .. }) =
            self;
        let name = format.as_deref()?;
        assembly::descriptors::FLOPPY_TYPES
            .iter()
            .find(|kind| kind.name == name)
            .map(|kind| kind.geometry)
    }

    /// What makes two kept disks the same disk in the drive: a set's disk
    /// and an image with the same write setting. The image's own disks are
    /// each their own.
    fn same_disk(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Set {
                    set,
                    medium,
                    journal,
                    ..
                },
                Self::Set {
                    set: other_set,
                    medium: other_medium,
                    journal: other_journal,
                    ..
                },
            ) => {
                set.digest == other_set.digest && medium == other_medium && journal == other_journal
            }
            (
                Self::Image {
                    sha256,
                    journal,
                    journal_file,
                    ..
                },
                Self::Image {
                    sha256: other_sha256,
                    journal: other_journal,
                    journal_file: other_file,
                    ..
                },
            ) => sha256 == other_sha256 && journal == other_journal && journal_file == other_file,
            _ => false,
        }
    }
}

/// One floppy drive's kept catalogue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptDrive {
    /// The drive as the guest names it, `A:` or `B:`.
    pub drive: String,
    /// The disk in the drive at Stop, by its place in `disk`; none when the
    /// drive was empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_drive: Option<usize>,
    #[serde(default)]
    pub disk: Vec<KeptDisk>,
}

impl KeptDrive {
    /// The drive is as the image sets it: its own disks only, the first in
    /// the drive, so Start needs nothing of the record for it.
    pub fn is_default(&self) -> bool {
        self.disk
            .iter()
            .all(|disk| matches!(disk, KeptDisk::Own { .. }))
            && (self.disk.is_empty() || self.in_drive == Some(0))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordFile {
    schema: u32,
    #[serde(default)]
    drive: Vec<KeptDrive>,
}

fn record_path(workspace: &Path) -> PathBuf {
    workspace.join("media").join(FOLDER).join(RECORD)
}

/// The catalogues a stopped machine had at its last Stop, read from its
/// workspace folder; none when it never stopped with one.
pub fn read(workspace: &Path) -> Result<Vec<KeptDrive>> {
    let text = match fs::read_to_string(record_path(workspace)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let file: RecordFile = toml::from_str(&text).map_err(|e| e.to_string())?;
    if file.schema != 1 {
        return Err("unsupported kept-catalogue record schema".into());
    }
    Ok(file.drive)
}

/// Whether the record is a document this version cannot read but a newer
/// one may have written: valid TOML of another schema or with fields this
/// version does not know. Damaged text is no such record.
fn newer_record(workspace: &Path) -> bool {
    fs::read_to_string(record_path(workspace))
        .is_ok_and(|text| text.parse::<toml::Table>().is_ok() && read(workspace).is_err())
}

/// Replace the record through a synced temporary. A record whose every
/// drive is as the image sets it is removed instead. The caller owns the
/// workspace.
fn write(workspace: &Path, drives: Vec<KeptDrive>) -> Result<()> {
    let path = record_path(workspace);
    if drives.iter().all(KeptDrive::is_default) {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    let text = toml::to_string(&RecordFile {
        schema: 1,
        drive: drives,
    })
    .map_err(|e| e.to_string())?;
    let folder = path.parent().ok_or("no created-image folder")?;
    publish(folder, RECORD, text.as_bytes()).map_err(|e| e.to_string())
}

/// Write `bytes` as `name` in `folder` through a synced temporary, so a
/// crash leaves the old file or the new one, never part of it.
fn publish(folder: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(folder)?;
    let temporary = folder.join(format!("{name}.tmp"));
    fs::write(&temporary, bytes)?;
    fs::File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, folder.join(name))?;
    fs::File::open(folder)?.sync_all()
}

/// Choose the kept disk that goes into `drive` of a stopped machine at its
/// next Start, by its place in the kept catalogue.
pub fn set_in_drive(workspace: &Path, drive: &str, index: usize) -> Result<()> {
    let mut drives = read(workspace)?;
    let kept = drives
        .iter_mut()
        .find(|kept| kept.drive == drive)
        .ok_or_else(|| format!("{drive} kept no catalogue"))?;
    if index >= kept.disk.len() {
        return Err(format!("{drive}'s catalogue has no disk {}", index + 1));
    }
    kept.in_drive = Some(index);
    write(workspace, drives)
}

/// A disk of a running drive's catalogue, as the window hands it over at
/// Stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoppedDisk {
    pub name: String,
    /// The library set and medium of a set's disk.
    pub source: Option<CatalogueSource>,
    /// The tag a set's disk came from.
    pub reference: Option<String>,
    pub image: ImageSource,
    pub sha256: Option<[u8; 32]>,
    pub write: WriteMode,
    /// The disk's own format, when known.
    pub format: Option<assembly::FloppyGeometry>,
    /// The run started with it: one of the image's own disks, or one Start
    /// put in, rather than one inserted while the machine ran.
    pub started_with: bool,
}

/// A running floppy drive's catalogue in order, and the place of the disk
/// in it, none while the drive is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoppedDrive {
    pub unit: u8,
    pub disks: Vec<StoppedDisk>,
    pub in_drive: Option<usize>,
}

/// The catalogue index drive `unit` holds once the media changes queued for
/// the next run boundary (`queued`) are in: a Stop before that boundary
/// keeps the disk the user put in, not the one before it. `now` is the
/// index it holds and `count` its catalogue's length, as Next counts it.
pub fn after_queued(
    unit: u8,
    now: Option<usize>,
    count: usize,
    queued: &[(SlotId, MediaAction)],
) -> Option<usize> {
    queued
        .iter()
        .filter(|(at, _)| *at == SlotId(unit))
        .fold(now, |now, (_, action)| match action {
            MediaAction::Insert(index) => Some(*index),
            MediaAction::Eject => None,
            MediaAction::Next if count == 0 => now,
            MediaAction::Next => Some(now.map_or(0, |index| (index + 1) % count)),
            _ => now,
        })
}

/// The running drives' catalogues as This machine lists them, with the
/// catalogue index each drive holds (`inserted`), for the record at Stop.
pub fn stopped(slots: &[Option<FloppySlot>; 2], inserted: [Option<usize>; 2]) -> Vec<StoppedDrive> {
    (0u8..)
        .zip(slots)
        .filter_map(|(unit, slot)| {
            let slot = slot.as_ref()?;
            let listed: Vec<_> = slot
                .entries
                .iter()
                .filter(|entry| entry.reason.is_none() && entry.index.is_some())
                .collect();
            // The drive may hold an earlier entry of a disk whose write
            // setting changed; it is the same disk.
            let in_drive = inserted[usize::from(unit)].and_then(|now| {
                listed.iter().position(|entry| {
                    entry.index == Some(now) || entry.alternates.iter().any(|(_, at)| *at == now)
                })
            });
            Some(StoppedDrive {
                unit,
                disks: listed
                    .into_iter()
                    .map(|entry| StoppedDisk {
                        name: entry.medium.name.clone(),
                        source: entry.source.clone(),
                        reference: entry.reference.clone(),
                        image: entry.medium.image.clone(),
                        sha256: entry.medium.expected_sha256,
                        write: entry.medium.write.clone(),
                        format: entry.geometry,
                        started_with: entry.declared,
                    })
                    .collect(),
                in_drive,
            })
        })
        .collect()
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

/// The set with manifest `digest` and a tag for it that never moves: a
/// revision tag of the store, else another tag that is no release alias,
/// else the tag the record this Stop replaces had, else the digest.
fn set_of(
    store: &Store,
    previous: &[KeptDrive],
    tags: &mut Option<Vec<image_store::Tag>>,
    digest: &Digest,
) -> Option<(Descriptor, String)> {
    use image_store::tag::{parse, Kind};
    let tags = tags.get_or_insert_with(|| {
        let mut tags = Vec::new();
        let mut after = None;
        while let Ok(page) = store.list_tags(after.as_deref(), 1000) {
            tags.extend(page.tags);
            after = page.next_after;
            if after.is_none() {
                break;
            }
        }
        tags
    });
    let named: Vec<_> = tags
        .iter()
        .filter(|tag| tag.manifest.digest == *digest)
        .collect();
    let version = |name: &str| {
        name.rsplit_once(':')
            .map_or(name, |(_, version)| version)
            .to_owned()
    };
    let recorded = || {
        previous
            .iter()
            .flat_map(|drive| &drive.disk)
            .find_map(|disk| match disk {
                KeptDisk::Set { set, reference, .. } if set.digest == *digest => {
                    Some((set.clone(), reference.clone()))
                }
                _ => None,
            })
    };
    let fixed = named
        .iter()
        .find(|tag| matches!(parse(&version(&tag.name)), Some(Kind::Revision { .. })))
        .or_else(|| {
            named
                .iter()
                .find(|tag| parse(&version(&tag.name)).is_none())
        });
    match (fixed, named.first()) {
        (Some(tag), _) => Some((tag.manifest.clone(), tag.name.clone())),
        (None, Some(alias)) => recorded()
            .map(|(_, reference)| reference)
            .or_else(|| Some(digest.to_string()))
            .map(|reference| (alias.manifest.clone(), reference)),
        (None, None) => recorded(),
    }
}

/// The journal file name an image disk keeps in the machine's media folder,
/// `None` for its own `<sha-256>.upfj`. A journal Insert put beside an
/// image file of this computer is copied in, under the first free name of
/// the disk (`<sha-256>-2.upfj`, …) when the machine already has another
/// journal for it, so the machine's copies never share the file.
fn own_journal(media: &Path, sha256: &str, journal: &Path) -> Result<Option<String>> {
    let named = |n: usize| {
        if n == 1 {
            format!("{sha256}.upfj")
        } else {
            format!("{sha256}-{n}.upfj")
        }
    };
    let file = |name: String| (name != named(1)).then_some(name);
    if let (Some(parent), Some(name)) = (journal.parent(), journal.file_name()) {
        if same_path(parent, media) {
            return Ok(file(name.to_string_lossy().into_owned()));
        }
    }
    let bytes = match fs::read(journal) {
        Ok(bytes) => bytes,
        // Nothing was written: the machine's own journal starts empty.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("its journal cannot be read ({error})")),
    };
    for n in 1.. {
        let name = named(n);
        match fs::read(media.join(&name)) {
            Ok(existing) if existing == bytes => return Ok(file(name)),
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                publish(media, &name, &bytes).map_err(|e| e.to_string())?;
                return Ok(file(name));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!("the journal names never run out")
}

/// One disk as the record keeps it; an image file of this computer and a
/// journal beside it are copied into the machine's media folder first.
fn keep_disk(
    store: &Store,
    workspace: &Path,
    previous: &[KeptDrive],
    tags: &mut Option<Vec<image_store::Tag>>,
    disk: StoppedDisk,
) -> Result<KeptDisk> {
    let journal = match &disk.write {
        WriteMode::Journal { path, .. } => Some(path.clone()),
        _ => None,
    };
    let format = disk.format.map(crate::media_dialog::geometry_name);
    if let Some(source) = disk.source {
        let (set, reference) = set_of(store, previous, tags, &source.set)
            .ok_or("its set is no longer in the library")?;
        return Ok(KeptDisk::Set {
            name: disk.name,
            set,
            // The tag the disk came from, unless it is a release alias.
            reference: disk
                .reference
                .filter(|reference| {
                    let (_, version) = reference.rsplit_once(':').unwrap_or(("", reference));
                    !matches!(
                        image_store::tag::parse(version),
                        Some(image_store::tag::Kind::Alias { .. })
                    )
                })
                .unwrap_or(reference),
            medium: source.medium_id,
            journal: journal.is_some(),
            format,
        });
    }
    let media = workspace.join("media");
    let created = |sha256: &str| media.join(FOLDER).join(format!("{sha256}.img"));
    let path = match &disk.image {
        ImageSource::File(path) => Some(path),
        _ => None,
    };
    let sha256 = disk.sha256.map(|digest| hex(&digest));
    let stored = sha256
        .as_deref()
        .zip(path)
        .is_some_and(|(sha256, path)| same_path(path, &created(sha256)));
    if !stored {
        if disk.started_with {
            return Ok(KeptDisk::Own {
                name: disk.name,
                sha256,
                format,
            });
        }
        let (Some(path), Some(digest)) = (path, disk.sha256) else {
            return Err("it has no image file to keep".into());
        };
        let bytes = fs::read(path).map_err(|e| format!("its image file cannot be read ({e})"))?;
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != digest {
            return Err("its image file changed since it went in".into());
        }
        crate::created_media::store_image(&media, &bytes).map_err(|e| e.to_string())?;
    }
    let sha256 = sha256.ok_or("it has no SHA-256 to keep it by")?;
    let journal_file = match &journal {
        Some(journal) => own_journal(&media, &sha256, journal)?,
        None => None,
    };
    Ok(KeptDisk::Image {
        name: disk.name,
        journal: journal.is_some(),
        journal_file,
        sha256,
        format,
    })
}

/// Keep the stopped drives' catalogues in the machine's workspace, as its
/// next Start takes them back; the record replaces the last one, unless a
/// newer version wrote that one. Each disk that cannot be kept is named in
/// a note, and the others are kept; the same disk is kept once.
pub fn keep(store: &Store, workspace: &Path, drives: Vec<StoppedDrive>) -> Result<Vec<String>> {
    let previous = match read(workspace) {
        Ok(previous) => previous,
        Err(error) if newer_record(workspace) => {
            return Ok(vec![format!(
                "The drives' catalogues were not kept at Stop: a newer version keeps them in this machine ({error})."
            )]);
        }
        // A damaged record would bring nothing back at Start: it is replaced.
        Err(_) => Vec::new(),
    };
    let mut tags = None;
    let mut notes = Vec::new();
    let mut kept = Vec::new();
    for drive in drives {
        let letter = drive_name(drive.unit);
        let mut disks: Vec<KeptDisk> = Vec::new();
        let mut in_drive = None;
        for (at, disk) in drive.disks.into_iter().enumerate() {
            let name = disk.name.clone();
            match keep_disk(store, workspace, &previous, &mut tags, disk) {
                Ok(disk) => {
                    // The same disk inserted again is the one already kept.
                    let place = disks
                        .iter()
                        .position(|other| other.same_disk(&disk))
                        .unwrap_or_else(|| {
                            disks.push(disk);
                            disks.len() - 1
                        });
                    if drive.in_drive == Some(at) {
                        in_drive = Some(place);
                    }
                }
                Err(why) => notes.push(format!("{name} left {letter}'s catalogue at Stop: {why}.")),
            }
        }
        kept.push(KeptDrive {
            drive: letter,
            in_drive,
            disk: disks,
        });
    }
    write(workspace, kept)?;
    Ok(notes)
}

/// A set's disk the kept catalogue brought back, for the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredSet {
    /// Its place in the drive's catalogue.
    pub index: usize,
    pub source: CatalogueSource,
    /// The tag it came from.
    pub reference: String,
    /// The drive tooltip's words for it.
    pub origin: String,
}

/// What [`restore`] did to one drive, for the window once the machine has
/// assembled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredDrive {
    pub unit: u8,
    /// The set disks the kept catalogue brought back.
    pub sets: Vec<RestoredSet>,
}

/// What [`restore`] did: the drives it changed, and what it left out.
#[derive(Debug, Default)]
pub struct Restored {
    pub drives: Vec<RestoredDrive>,
    pub notes: Vec<String>,
}

/// A set resolved once per Start, however many of its disks come back.
struct ResolvedSet {
    resolved: image_store::ResolvedImage,
    config: image_store::media_set::Config,
}

#[derive(Default)]
struct Sets(BTreeMap<Digest, Result<ResolvedSet>>);

impl Sets {
    fn get(&mut self, store: &Store, set: &Descriptor) -> Result<&ResolvedSet> {
        self.0
            .entry(set.digest.clone())
            .or_insert_with(|| {
                let resolved = store
                    .resolve_descriptor(
                        set,
                        &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
                    )
                    .map_err(|_| "its set is no longer in the library".to_owned())?;
                let config = image_store::media_set::Config::decode(&resolved.config)
                    .map_err(|e| e.to_string())?;
                if !pc_image::is_pc(&config.platform) {
                    return Err("its set is for another platform".into());
                }
                Ok(ResolvedSet { resolved, config })
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// A medium of a kept catalogue, ready for the drive, with the verified base
/// assembly takes it from and, for a set's disk, what the window shows of it.
struct Ready {
    medium: FloppyMedium,
    base: Option<(PathBuf, journal_files::VerifiedBase)>,
    set: Option<(CatalogueSource, String, String)>,
}

fn doesnt_take(geometry: assembly::FloppyGeometry) -> String {
    format!(
        "{} does not take it",
        assembly::descriptors::floppy_drive_media(geometry)
    )
}

fn set_disk(
    store: &Store,
    sets: &mut Sets,
    workspace: &Path,
    geometry: assembly::FloppyGeometry,
    (set, reference, medium, journal): (&Descriptor, &str, &str, bool),
) -> Result<Ready> {
    let found = sets.get(store, set)?;
    let count = found.config.media.len();
    let at = found
        .config
        .media
        .iter()
        .position(|m| m.id == medium)
        .ok_or("its set no longer has it")?;
    let medium = &found.config.media[at];
    let format = pc_image::media_set::geometry(&medium.format)
        .map_err(|e| e.to_string())?
        .ok_or("an optical disc needs a CD-ROM drive")?;
    if !geometry.accepted_media().contains(&format) {
        return Err(doesnt_take(geometry));
    }
    let path = store
        .resolved_file_path(&found.resolved, &medium.blob)
        .map_err(|e| e.to_string())?;
    let base = pc_image::media_set::verified_base(&found.resolved, &medium.blob)
        .map_err(|e| e.to_string())?;
    // A journal named as a live insert from the library names it, so the
    // disk keeps one journal in this machine.
    let write = if journal {
        WriteMode::journal(workspace.join("media").join(format!(
            "{}-{}.upfj",
            set.digest.hex(),
            medium.id
        )))
    } else {
        WriteMode::ReadOnly
    };
    Ok(Ready {
        medium: FloppyMedium {
            image: ImageSource::File(path.clone()),
            expected_sha256: Some(base.identity().base_digest),
            write,
            name: medium.label.clone(),
        },
        base: Some((path, base)),
        set: Some((
            CatalogueSource {
                set: set.digest.clone(),
                medium_id: medium.id.clone(),
                blob: medium.blob.clone(),
            },
            reference.to_owned(),
            crate::media_dialog::set_origin(reference, at, count),
        )),
    })
}

fn image_disk(
    workspace: &Path,
    geometry: assembly::FloppyGeometry,
    (name, sha256, journal, journal_file): (&str, &str, bool, Option<&String>),
) -> Result<Ready> {
    let media = workspace.join("media");
    let path = media.join(FOLDER).join(format!("{sha256}.img"));
    let file = fs::File::open(&path).map_err(|e| format!("its image is gone ({e})"))?;
    let base = journal_files::VerifiedBase::from_file(file).map_err(|e| e.to_string())?;
    let digest = base.identity().base_digest;
    if hex(&digest) != sha256 {
        return Err("its image no longer matches its SHA-256".into());
    }
    if !geometry
        .medium_for_len(base.byte_len())
        .is_some_and(|medium| geometry.accepted_media().contains(&medium))
    {
        return Err(doesnt_take(geometry));
    }
    let write = if journal {
        let file = journal_file
            .cloned()
            .unwrap_or_else(|| format!("{sha256}.upfj"));
        // A record names a file of the media folder, never a path.
        if Path::new(&file).file_name() != Some(std::ffi::OsStr::new(&file)) {
            return Err("its record names no journal of this machine".into());
        }
        WriteMode::journal(media.join(file))
    } else {
        WriteMode::ReadOnly
    };
    Ok(Ready {
        medium: FloppyMedium {
            image: ImageSource::File(path.clone()),
            expected_sha256: Some(digest),
            write,
            name: name.to_owned(),
        },
        base: Some((path, base)),
        set: None,
    })
}

/// The image's own disk a kept one names, among those not taken yet: the
/// same name and SHA-256, else the same name (the disk's bytes changed, as
/// Save as new image… changes them), else the same SHA-256.
fn own_disk(
    own: &[FloppyMedium],
    used: &[bool],
    name: &str,
    sha256: Option<&str>,
) -> Option<usize> {
    let pinned = |medium: &FloppyMedium| {
        sha256.is_some() && medium.expected_sha256.map(|digest| hex(&digest)).as_deref() == sha256
    };
    let free = || (0..own.len()).filter(|at| !used[*at]);
    free()
        .find(|at| pinned(&own[*at]) && own[*at].name == name)
        .or_else(|| free().find(|at| own[*at].name == name))
        .or_else(|| free().find(|at| pinned(&own[*at])))
}

/// Put the catalogue `kept` back into its drive of the prepared session.
/// `lead` is the number of disks a pending insert put at the head of the
/// drive's catalogue, the chosen one first.
fn restore_drive(
    session: &mut pc_image::workspace::PreparedSession,
    store: &Store,
    workspace: &Path,
    sets: &mut Sets,
    kept: &KeptDrive,
    lead: usize,
) -> Result<(RestoredDrive, Vec<String>)> {
    let unit = match kept.drive.as_str() {
        "A:" => 0,
        "B:" => 1,
        _ => return Err("it is no floppy drive".into()),
    };
    let drive = session
        .machine
        .drives
        .iter()
        .find(|d| d.unit == unit && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
        .ok_or("the machine has no such floppy drive now")?;
    let assembly::DriveKind::Floppy { geometry } = drive.kind else {
        return Err("floppy drive expected".into());
    };
    let started = match &drive.media {
        Some(list) => list.entries.clone(),
        None => vec![FloppyMedium {
            image: drive.image.clone(),
            expected_sha256: drive.expected_sha256,
            write: drive.write.clone(),
            name: match &drive.image {
                ImageSource::File(path) => crate::media_dialog::filename(path),
                _ => "image".into(),
            },
        }],
    };
    let (pending, own) = started.split_at(lead.min(started.len()));
    let mut used = vec![false; own.len()];
    let mut entries = pending.to_vec();
    let mut restored = RestoredDrive {
        unit,
        sets: Vec::new(),
    };
    let mut notes = Vec::new();
    let mut in_drive = None;
    let mut bases = std::collections::BTreeMap::new();
    for (at, disk) in kept.disk.iter().enumerate() {
        let ready = match disk {
            KeptDisk::Own { name, sha256, .. } => {
                match own_disk(own, &used, name, sha256.as_deref()) {
                    Some(found) => {
                        used[found] = true;
                        Ok(Ready {
                            medium: own[found].clone(),
                            base: None,
                            set: None,
                        })
                    }
                    None => Err("the image no longer has it".to_owned()),
                }
            }
            KeptDisk::Set {
                set,
                reference,
                medium,
                journal,
                ..
            } => set_disk(
                store,
                sets,
                workspace,
                geometry,
                (set, reference, medium, *journal),
            ),
            KeptDisk::Image {
                name,
                sha256,
                journal,
                journal_file,
                ..
            } => image_disk(
                workspace,
                geometry,
                (name, sha256, *journal, journal_file.as_ref()),
            ),
        };
        let ready = match ready {
            Ok(ready) => ready,
            Err(why) => {
                notes.push(format!(
                    "{} left {}'s catalogue: {why}.",
                    disk.name(),
                    kept.drive
                ));
                continue;
            }
        };
        // The pending insert brings this disk itself, as it chose it.
        if pending.iter().any(|medium| {
            medium.image == ready.medium.image
                && medium.expected_sha256 == ready.medium.expected_sha256
        }) {
            continue;
        }
        if kept.in_drive == Some(at) {
            in_drive = Some(entries.len());
        }
        if let Some((path, base)) = ready.base {
            bases.insert(path, base);
        }
        if let Some((source, reference, origin)) = ready.set {
            restored.sets.push(RestoredSet {
                index: entries.len(),
                source,
                reference,
                origin,
            });
        }
        entries.push(ready.medium);
    }
    entries.extend(
        own.iter()
            .zip(&used)
            .filter(|(_, used)| !**used)
            .map(|(medium, _)| medium.clone()),
    );
    // The disk in the drive at power-on: the pending insert's chosen disk,
    // else the one that was in it at Stop; none when the drive was empty.
    let start = if pending.is_empty() {
        in_drive
    } else {
        Some(0)
    };
    session
        .replace_floppy_catalogue(unit, entries, start, bases)
        .map_err(|e| e.to_string())?;
    Ok((restored, notes))
}

/// Put each drive's kept catalogue back into the prepared session of the
/// machine in `workspace`, before its first instruction, after its pending
/// inserts went in: `listed`, the machine list's, and `created`, the created
/// disks' (those for a drive with a listed insert stayed out). Nothing here
/// stops the Start: a record that cannot be read leaves every catalogue out
/// with a note, and a disk that cannot come back stays out with one.
pub fn restore(
    session: &mut pc_image::workspace::PreparedSession,
    store: &Store,
    workspace: &Path,
    listed: &[image_store::machine::PendingInsert],
    created: &[crate::created_media::CreatedInsert],
) -> Restored {
    let mut restored = Restored::default();
    let drives = match read(workspace) {
        Ok(drives) => drives,
        Err(error) => {
            restored.notes.push(format!(
                "The catalogue kept at the last Stop was left out: its record cannot be read ({error})."
            ));
            return restored;
        }
    };
    let mut sets = Sets::default();
    for kept in &drives {
        let lead = match listed.iter().find(|insert| insert.drive == kept.drive) {
            Some(insert) if insert.all => sets
                .get(store, &insert.set)
                .map_or(1, |set| set.config.media.len()),
            Some(_) => 1,
            None => created
                .iter()
                .find(|insert| insert.drive == kept.drive)
                .map_or(0, |insert| insert.disks.len()),
        };
        match restore_drive(session, store, workspace, &mut sets, kept, lead) {
            Ok((drive, notes)) => {
                restored.notes.extend(notes);
                restored.drives.push(drive);
            }
            Err(why) => restored.notes.push(format!(
                "{}'s catalogue from the last Stop was left out: {why}.",
                kept.drive
            )),
        }
    }
    restored
}

/// The disks a stopped machine's drive takes back at its next Start, as the
/// Catalogue submenu and the picker's This machine list them while it waits:
/// in order, each with its short name and its own format, numbered from
/// `first`. `machine` names the machine in the write words; `find` looks a
/// library set up by its digest; `skip` leaves out the kept disks a pending
/// insert brings itself.
pub fn waiting_disks(
    kept: &KeptDrive,
    geometry: assembly::FloppyGeometry,
    machine: &str,
    find: &dyn Fn(&Digest) -> Option<SetEntry>,
    first: usize,
    skip: &dyn Fn(&KeptDisk) -> bool,
) -> Vec<CatalogueDisk> {
    use crate::catalogue_menu::{set_disk, short_name};
    let words = |journal: bool| {
        if journal {
            format!("changes kept in {machine}")
        } else {
            "write-protected".to_owned()
        }
    };
    kept.disk
        .iter()
        .enumerate()
        .filter(|(_, disk)| !skip(disk))
        .map(|(at, disk)| {
            let format = disk.format().unwrap_or(geometry);
            let index = first + at;
            match disk {
                KeptDisk::Set {
                    name,
                    set,
                    medium,
                    journal,
                    ..
                } => {
                    let entry = find(&set.digest);
                    let at = entry
                        .as_ref()
                        .and_then(|entry| entry.disks.iter().position(|d| d.id == *medium));
                    CatalogueDisk {
                        name: short_name(
                            entry.as_ref().and_then(|entry| set_disk(entry, medium)),
                            name,
                        ),
                        format,
                        write: words(*journal),
                        inserted: false,
                        index,
                        set: at.map(|at| (set.digest.clone(), at)),
                    }
                }
                KeptDisk::Image { name, journal, .. } => CatalogueDisk {
                    name: name.clone(),
                    format,
                    write: words(*journal),
                    inserted: false,
                    index,
                    set: None,
                },
                KeptDisk::Own { name, .. } => CatalogueDisk {
                    name: name.clone(),
                    format,
                    write: "as the image sets it".into(),
                    inserted: false,
                    index,
                    set: None,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_dialog::CatalogueEntry;

    fn entry(name: &str, index: Option<usize>, declared: bool) -> CatalogueEntry {
        CatalogueEntry {
            source: None,
            receipt: None,
            readonly_bytes: None,
            medium: FloppyMedium {
                image: ImageSource::File(format!("/media/{name}").into()),
                expected_sha256: Some([index.unwrap_or(9) as u8; 32]),
                write: WriteMode::ReadOnly,
                name: name.into(),
            },
            geometry: Some(assembly::FloppyGeometry::HD_1_44),
            reason: None,
            index,
            declared,
            alternates: Vec::new(),
            workspace_journal: None,
            origin: None,
            verified: None,
            reference: None,
        }
    }

    /// The window hands over what This machine lists, in order: no disk
    /// the drive refused, none not in the catalogue yet. The disk in the
    /// drive is found by its catalogue index, also when the drive holds an
    /// earlier entry of a disk whose write setting changed since.
    #[test]
    fn the_stopped_catalogue_is_what_this_machine_lists() {
        let mut refused = entry("bad.img", Some(2), false);
        refused.reason = Some("damaged".into());
        let mut switched = entry("switched.img", Some(4), false);
        switched.alternates = vec![(WriteMode::journal("/media/switched.upfj"), 3)];
        let slot = FloppySlot {
            geometry: assembly::FloppyGeometry::HD_1_44,
            entries: vec![
                entry("own.img", Some(0), true),
                entry("game.img", Some(1), false),
                refused,
                entry("waiting.img", None, false),
                switched,
            ],
            journal_retention: 1,
        };
        let slots = [Some(slot.clone()), None];
        let drives = stopped(&slots, [Some(3), None]);
        assert_eq!(drives.len(), 1);
        let names: Vec<&str> = drives[0].disks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["own.img", "game.img", "switched.img"]);
        assert_eq!(
            drives[0]
                .disks
                .iter()
                .map(|d| d.started_with)
                .collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(drives[0].in_drive, Some(2));
        assert_eq!(stopped(&slots, [Some(1), None])[0].in_drive, Some(1));
        assert_eq!(stopped(&slots, [None, None])[0].in_drive, None);
        let both = stopped(&[Some(slot.clone()), Some(slot)], [None, Some(0)]);
        assert_eq!(
            both.iter()
                .map(|d| (d.unit, d.in_drive))
                .collect::<Vec<_>>(),
            [(0, None), (1, Some(0))]
        );
    }

    /// The record keeps every kind of disk as written; a field it does not
    /// know, or a newer schema, makes it unreadable rather than half read,
    /// and an empty catalogue list removes it.
    #[test]
    fn the_record_round_trips_and_refuses_what_it_does_not_know() {
        let temp = machine_file::test_support::TempDir::new("kept-record");
        let workspace = temp.path();
        assert!(read(workspace).unwrap().is_empty());
        let drives = vec![
            KeptDrive {
                drive: "A:".into(),
                in_drive: Some(2),
                disk: vec![
                    KeptDisk::Own {
                        name: "one.img".into(),
                        sha256: Some("00".repeat(32)),
                        format: Some("360K".into()),
                    },
                    KeptDisk::Set {
                        name: "Disk 1".into(),
                        set: Descriptor::new(
                            image_store::media_type::MANIFEST,
                            Digest::of(b"set"),
                            12,
                        ),
                        reference: "games:1.0-r1".into(),
                        medium: "disk-1".into(),
                        journal: true,
                        format: Some("720K".into()),
                    },
                    KeptDisk::Image {
                        name: "game.img".into(),
                        sha256: "11".repeat(32),
                        journal: true,
                        journal_file: Some(format!("{}-2.upfj", "11".repeat(32))),
                        format: None,
                    },
                ],
            },
            KeptDrive {
                drive: "B:".into(),
                in_drive: None,
                disk: Vec::new(),
            },
        ];
        write(workspace, drives.clone()).unwrap();
        assert_eq!(read(workspace).unwrap(), drives);
        set_in_drive(workspace, "A:", 0).unwrap();
        assert_eq!(read(workspace).unwrap()[0].in_drive, Some(0));
        assert!(set_in_drive(workspace, "A:", 3).is_err());
        assert!(set_in_drive(workspace, "B:", 0).is_err());
        for text in [
            "schema = 2\n",
            "schema = 1\nlater = true\n",
            "schema = 1\n[[drive]]\ndrive = \"A:\"\n[[drive.disk]]\nkind = \"own\"\nname = \"x\"\nlater = 1\n",
            "schema = 1\n[[drive]]\ndrive = \"A:\"\n[[drive.disk]]\nkind = \"tape\"\nname = \"x\"\n",
        ] {
            fs::write(record_path(workspace), text).unwrap();
            assert!(read(workspace).is_err(), "{text}");
        }
        write(workspace, Vec::new()).unwrap();
        assert!(!record_path(workspace).exists());
    }

    /// The image's own disks are found by name and SHA-256, then by name
    /// alone, as after Save as new image… changed one's bytes, and only then
    /// by SHA-256, so two disks with equal bytes never swap places.
    #[test]
    fn own_disks_are_found_by_name_where_their_bytes_changed() {
        let medium = |name: &str, byte: u8| FloppyMedium {
            image: ImageSource::File(format!("/store/{name}").into()),
            expected_sha256: Some([byte; 32]),
            write: WriteMode::ReadOnly,
            name: name.into(),
        };
        let own = [medium("one.img", 1), medium("two.img", 0)];
        let zero = hex(&[0; 32]);
        let mut used = [false; 2];
        let one = own_disk(&own, &used, "one.img", Some(&zero));
        assert_eq!(one, Some(0));
        used[0] = true;
        assert_eq!(own_disk(&own, &used, "two.img", Some(&zero)), Some(1));
        assert_eq!(
            own_disk(&own, &[false; 2], "renamed.img", Some(&zero)),
            Some(1)
        );
        assert_eq!(own_disk(&own, &[true, true], "two.img", Some(&zero)), None);
    }

    fn own(name: &str) -> KeptDisk {
        KeptDisk::Own {
            name: name.into(),
            sha256: None,
            format: None,
        }
    }

    /// A catalogue as the image sets it, its own disks only with the first
    /// in the drive, leaves no record; any other catalogue does.
    #[test]
    fn a_catalogue_as_the_image_sets_it_leaves_no_record() {
        let temp = machine_file::test_support::TempDir::new("kept-default");
        let workspace = temp.path();
        let drive = |in_drive, disk| KeptDrive {
            drive: "A:".into(),
            in_drive,
            disk,
        };
        let default = drive(Some(0), vec![own("one.img"), own("two.img")]);
        assert!(default.is_default());
        assert!(drive(None, Vec::new()).is_default());
        write(workspace, vec![default.clone()]).unwrap();
        assert!(!record_path(workspace).exists());
        for other in [
            drive(Some(1), vec![own("one.img"), own("two.img")]),
            drive(None, vec![own("one.img")]),
        ] {
            assert!(!other.is_default());
            write(workspace, vec![default.clone(), other.clone()]).unwrap();
            assert_eq!(read(workspace).unwrap(), [default.clone(), other]);
        }
        // Choosing the first disk again makes it the image's catalogue.
        write(
            workspace,
            vec![drive(Some(1), vec![own("one.img"), own("two.img")])],
        )
        .unwrap();
        set_in_drive(workspace, "A:", 0).unwrap();
        assert!(!record_path(workspace).exists());
    }

    /// The same disk inserted again in a run, as a restored image file
    /// browsed once more, is kept once, and the drive holds that one.
    #[test]
    fn the_same_disk_inserted_again_is_kept_once() {
        let temp = machine_file::test_support::TempDir::new("kept-once");
        let store = Store::open(temp.path().join("store")).unwrap();
        let workspace = temp.path().join("workspace");
        let bytes = vec![0x5a; 368_640];
        let host = temp.path().join("game.img");
        fs::write(&host, &bytes).unwrap();
        let (copy, sha) =
            crate::created_media::store_image(&workspace.join("media"), &bytes).unwrap();
        let disk = |image: &Path, started_with| StoppedDisk {
            name: "game.img".into(),
            source: None,
            reference: None,
            image: ImageSource::File(image.to_path_buf()),
            sha256: Some(sha),
            write: WriteMode::ReadOnly,
            format: Some(assembly::FloppyGeometry::DD_360K),
            started_with,
        };
        let own_disk = StoppedDisk {
            name: "one.img".into(),
            sha256: None,
            ..disk(Path::new("/store/one.img"), true)
        };
        let notes = keep(
            &store,
            &workspace,
            vec![StoppedDrive {
                unit: 0,
                disks: vec![own_disk, disk(&copy, true), disk(&host, false)],
                in_drive: Some(2),
            }],
        )
        .unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let kept = read(&workspace).unwrap();
        assert_eq!(kept[0].disk.len(), 2);
        assert_eq!(kept[0].in_drive, Some(1));
        assert!(
            matches!(&kept[0].disk[1], KeptDisk::Image { format: Some(format), .. } if format == "360K")
        );
    }

    /// A journal beside an image file of this computer becomes the
    /// machine's own, copied into its media folder under the disk's name,
    /// or the next free one when the machine has another journal for it;
    /// the same bytes again are the same journal.
    #[test]
    fn a_journal_beside_a_file_is_copied_into_the_machine() {
        let temp = machine_file::test_support::TempDir::new("kept-journal");
        let media = temp.path().join("media");
        fs::create_dir_all(&media).unwrap();
        let host = temp.path().join("game.img.upfj");
        let sha = "ab".repeat(32);
        assert_eq!(
            own_journal(&media, &sha, &host).unwrap(),
            None,
            "nothing written"
        );
        assert!(!media.join(format!("{sha}.upfj")).exists());
        fs::write(&host, b"first writes").unwrap();
        assert_eq!(own_journal(&media, &sha, &host).unwrap(), None);
        assert_eq!(
            fs::read(media.join(format!("{sha}.upfj"))).unwrap(),
            b"first writes"
        );
        assert_eq!(own_journal(&media, &sha, &host).unwrap(), None);
        fs::write(&host, b"other writes").unwrap();
        let second = format!("{sha}-2.upfj");
        assert_eq!(
            own_journal(&media, &sha, &host).unwrap(),
            Some(second.clone())
        );
        assert_eq!(fs::read(media.join(&second)).unwrap(), b"other writes");
        // A journal of the machine's own keeps its name.
        assert_eq!(
            own_journal(&media, &sha, &media.join(&second)).unwrap(),
            Some(second)
        );
        assert_eq!(
            own_journal(&media, &sha, &media.join(format!("{sha}.upfj"))).unwrap(),
            None
        );
    }

    /// A Stop before the next run boundary keeps the disk the user put in:
    /// the queued changes of that drive count, in order.
    #[test]
    fn queued_media_changes_count_at_stop() {
        use MediaAction::{Eject, Insert, Next};
        let (a, b) = (SlotId(0), SlotId(1));
        assert_eq!(after_queued(0, Some(1), 3, &[]), Some(1));
        assert_eq!(after_queued(0, Some(1), 3, &[(a, Insert(2))]), Some(2));
        assert_eq!(after_queued(0, Some(1), 3, &[(b, Insert(2))]), Some(1));
        assert_eq!(after_queued(0, Some(2), 3, &[(a, Next)]), Some(0));
        assert_eq!(after_queued(0, None, 3, &[(a, Next)]), Some(0));
        assert_eq!(after_queued(0, None, 0, &[(a, Next)]), None);
        assert_eq!(
            after_queued(0, Some(1), 3, &[(a, Insert(2)), (a, Eject)]),
            None
        );
    }
}
