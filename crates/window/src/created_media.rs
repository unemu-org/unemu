// SPDX-License-Identifier: AGPL-3.0-or-later
//! Disks that Create from files… builds, owned by a machine; media-picker
//! mock-up revision 2 and machine-images §3.2.
//!
//! A created image is a read-only base named by its SHA-256,
//! `created/<sha-256>.img` in the media folder of the machine that owns it:
//! a library machine's workspace, a running session's media folder, or a
//! machine file's local media folder. It is published once through a synced
//! temporary and never opened for writing; a file of that name whose bytes
//! no longer hash to it is replaced by a new directory entry, never
//! rewritten. Guest writes go to the journal `<sha-256>.upfj` beside the
//! machine's other journals, or are refused. Every byte comes from the
//! FAT12 builder, so the same files give the same image and name.
//!
//! A stopped library machine keeps its pending insert of created disks in
//! `created/pending.toml` of its workspace, not in the store's machine list:
//! released versions read that list with every unknown field refused, so a
//! new kind of pending insert there would make them refuse every machine.
//! Reset and Delete drop the folder with the workspace's media; Duplicate
//! copies it. A pending insert of the machine list for the same drive, which
//! a version without created disks may have set later, goes in instead.

use media::fat12::{FloppyFormat, HostItem, ShortName, Survey};
use media::SectorSource as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;

/// The folder under a media folder that holds created images.
pub(crate) const FOLDER: &str = "created";
/// The pending-insert document in a workspace's [`FOLDER`].
const PENDING: &str = "pending.toml";

/// One created disk of a pending insert.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatedDisk {
    /// The image's SHA-256 in lower-case hex, which names its file.
    pub sha256: String,
    /// The disk's name in the drive's catalogue.
    pub name: String,
}

/// Created disks a stopped machine's drive takes at its next Start: the
/// first disk goes in and every disk joins the catalogue, the others in
/// order after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatedInsert {
    /// The drive as the guest names it, `A:` or `B:`.
    pub drive: String,
    /// What the disks are called together, as the drive chips show it.
    pub title: String,
    pub disks: Vec<CreatedDisk>,
    /// Whether guest writes go to the machine's journals; otherwise the
    /// disks are write-protected.
    #[serde(default)]
    pub journal: bool,
    /// The disk that goes in, by its place in `disks`, when it is not disk 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<usize>,
}

impl CreatedInsert {
    /// The disk that goes in at Start: the chosen one, else disk 1.
    pub fn first(&self) -> usize {
        self.first
            .filter(|first| *first < self.disks.len())
            .unwrap_or(0)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingFile {
    schema: u32,
    #[serde(default)]
    insert: Vec<CreatedInsert>,
}

/// What Create and insert asks for: the read host items, laid out as the
/// view showed them, and where the disks go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreateOrder {
    pub unit: u8,
    pub items: Vec<HostItem>,
    pub top: Option<ShortName>,
    pub format: FloppyFormat,
    pub label: Option<[u8; 11]>,
    pub split: bool,
    pub journal: bool,
    /// The set's tag and title when the disks are also kept in the library.
    pub keep: Option<(String, String)>,
    /// What the disks are called together.
    pub title: String,
}

/// Where the library worker puts the created images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// A media folder: the running session's, or a stopped machine file's.
    Folder(PathBuf),
    /// A stopped library machine, by id: the drive's pending insert.
    Machine(String),
}

/// A created image the worker opened and hashed in full: its open file,
/// identity and metadata, so the window puts it into a drive without reading
/// and hashing it again.
#[derive(Clone)]
pub(crate) struct Verified(std::sync::Arc<(fs::File, media::Header, fs::Metadata)>);

impl Verified {
    /// Hash the file at `path` in full; it must still be the image `digest`.
    fn open(path: &Path, digest: &[u8; 32]) -> io::Result<Self> {
        let base = media::VerifiedBase::from_file(fs::File::open(path)?)?;
        if base.identity().base_digest != *digest {
            return Err(io::Error::other(
                "the stored image does not match its SHA-256",
            ));
        }
        let metadata = base
            .metadata()
            .cloned()
            .ok_or_else(|| io::Error::other("the stored image is not a file"))?;
        Ok(Self(std::sync::Arc::new((
            base.try_clone_verified_file()?,
            base.identity().clone(),
            metadata,
        ))))
    }

    /// The verified base again, checked unchanged but not hashed.
    pub fn base(&self) -> io::Result<media::VerifiedBase> {
        let (file, identity, metadata) = &*self.0;
        media::VerifiedBase::from_verified_file(
            file.try_clone()?,
            identity.clone(),
            metadata.clone(),
        )
    }
}

impl std::fmt::Debug for Verified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Verified").field(&self.0 .1).finish()
    }
}

impl PartialEq for Verified {
    fn eq(&self, other: &Self) -> bool {
        self.0 .1 == other.0 .1
    }
}

impl Eq for Verified {}

/// What the worker made of an order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Created {
    pub order: CreateOrder,
    pub place: Place,
    /// Each disk's image file, SHA-256 and name, in catalogue order.
    pub disks: Vec<(PathBuf, [u8; 32], String)>,
    /// The disks hashed in full by the worker, for a running machine.
    pub verified: Vec<Verified>,
    /// The tag the set was kept under in the library.
    pub kept: Option<String>,
    /// Why the set was not kept in the library, when that failed; the disks
    /// are the machine's own all the same.
    pub keep_failed: Option<String>,
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The drive geometry of a diskette format.
pub(crate) fn geometry(format: FloppyFormat) -> Option<assembly::FloppyGeometry> {
    assembly::descriptors::FLOPPY_TYPES
        .iter()
        .find(|kind| kind.name == format.name())
        .map(|kind| kind.geometry)
}

/// The diskette format of a drive geometry.
pub(crate) fn format_of(geometry: assembly::FloppyGeometry) -> Option<FloppyFormat> {
    assembly::descriptors::FLOPPY_TYPES
        .iter()
        .find(|kind| kind.geometry == geometry)
        .and_then(|kind| FloppyFormat::from_name(kind.name))
}

/// Disk `index` of `count`, as its catalogue entry names it: the short name
/// a disk of a set has, "AZURAMON 3 of 10".
pub(crate) fn disk_name(title: &str, index: usize, count: usize) -> String {
    crate::catalogue_menu::short_name(
        Some(crate::catalogue_menu::SetDisk {
            title,
            number: index + 1,
            count,
            label: "",
        }),
        title,
    )
}

/// Store `bytes` as a created image in `media`: its file, named by its
/// SHA-256, and that digest. An existing file is kept only when its bytes
/// still hash to its name.
pub(crate) fn store_image(media: &Path, bytes: &[u8]) -> io::Result<(PathBuf, [u8; 32])> {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let folder = media.join(FOLDER);
    fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{}.img", hex(&digest)));
    let intact = match fs::read(&path) {
        Ok(existing) => <[u8; 32]>::from(Sha256::digest(&existing)) == digest,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if !intact {
        let exists = fs::symlink_metadata(&path).is_ok();
        publish_image(&path, bytes, &digest, exists)?;
    }
    Ok((path, digest))
}

/// Publish `bytes` at `path` through a synced temporary; with `replace`, as
/// a new directory entry over a damaged file. A file another create of the
/// same image published meanwhile counts as done.
fn publish_image(path: &Path, bytes: &[u8], digest: &[u8; 32], replace: bool) -> io::Result<()> {
    match media::create::image(path, bytes.len() as u64, bytes, replace) {
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                && fs::read(path).is_ok_and(|existing| {
                    <[u8; 32]>::from(Sha256::digest(&existing)) == *digest
                }) =>
        {
            Ok(())
        }
        result => result,
    }
}

fn pending_path(workspace: &Path) -> PathBuf {
    workspace.join("media").join(FOLDER).join(PENDING)
}

/// The created disks a stopped machine's drives take at its next Start, read
/// from its workspace folder.
pub(crate) fn read_pending(workspace: &Path) -> Result<Vec<CreatedInsert>> {
    let text = match fs::read_to_string(pending_path(workspace)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let file: PendingFile = toml::from_str(&text).map_err(|e| e.to_string())?;
    if file.schema != 1 {
        return Err("unsupported created-disk record schema".into());
    }
    Ok(file.insert)
}

/// Replace the record; none left removes it. The caller owns the workspace.
fn write_pending(workspace: &Path, inserts: Vec<CreatedInsert>) -> Result<()> {
    let path = pending_path(workspace);
    if inserts.is_empty() {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    let text = toml::to_string(&PendingFile {
        schema: 1,
        insert: inserts,
    })
    .map_err(|e| e.to_string())?;
    let folder = path.parent().ok_or("no created-disk folder")?;
    fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let temporary = folder.join(format!("{PENDING}.tmp"));
    (|| -> io::Result<()> {
        fs::write(&temporary, text.as_bytes())?;
        fs::File::open(&temporary)?.sync_all()?;
        fs::rename(&temporary, &path)?;
        fs::File::open(folder)?.sync_all()
    })()
    .map_err(|e| e.to_string())
}

/// Set or clear the created disks `drive` of a stopped machine takes at its
/// next Start; the machine's other drives keep theirs. A record that cannot
/// be read, or that a newer version wrote, is replaced: nothing of it would
/// go in at Start anyway.
pub(crate) fn set_pending(
    workspace: &Path,
    drive: &str,
    insert: Option<CreatedInsert>,
) -> Result<()> {
    let mut inserts = read_pending(workspace).unwrap_or_default();
    inserts.retain(|other| other.drive != drive);
    inserts.extend(insert);
    write_pending(workspace, inserts)
}

/// Choose the created disk waiting for `drive` that goes in at the next
/// Start, by its place; the others follow in order.
pub(crate) fn set_first(workspace: &Path, drive: &str, first: usize) -> Result<()> {
    let mut inserts = read_pending(workspace)?;
    let insert = inserts
        .iter_mut()
        .find(|insert| insert.drive == drive)
        .ok_or_else(|| format!("no created disks wait for {drive}"))?;
    if first >= insert.disks.len() {
        return Err(format!("{} has no disk {}", insert.title, first + 1));
    }
    insert.first = (first > 0).then_some(first);
    write_pending(workspace, inserts)
}

/// Drop the pending inserts a launch put in, once its machine assembled.
pub(crate) fn confirm(workspace: &Path, inserted: &[CreatedInsert]) -> Result<()> {
    let mut inserts = read_pending(workspace)?;
    let before = inserts.len();
    inserts.retain(|insert| !inserted.contains(insert));
    if inserts.len() == before {
        return Ok(());
    }
    write_pending(workspace, inserts)
}

/// What [`apply`] did: the inserts that are done with (put in, or replaced
/// by a pending insert of the machine list) and what went wrong, in words.
#[derive(Debug, Default)]
pub(crate) struct Applied {
    pub done: Vec<CreatedInsert>,
    pub notes: Vec<String>,
}

/// Put each pending insert of created disks into its drive of a prepared
/// session of the machine in `workspace`, before its first instruction: the
/// chosen disk first, then the others in order, ahead of the drive's own
/// catalogue. A drive with
/// a pending insert of the machine list (`listed`) keeps that one. Nothing
/// here stops the Start: a record that cannot be read, or an insert whose
/// image is gone or changed, stays out with a note.
pub(crate) fn apply(
    session: &mut pc_image::workspace::PreparedSession,
    workspace: &Path,
    listed: &[String],
) -> Applied {
    let mut applied = Applied::default();
    let inserts = match read_pending(workspace) {
        Ok(inserts) => inserts,
        Err(error) => {
            applied.notes.push(format!(
                "The created disks waiting for this machine were left out: their record cannot be read ({error})."
            ));
            return applied;
        }
    };
    for insert in inserts {
        if !listed.contains(&insert.drive) {
            if let Err(error) = apply_one(session, workspace, &insert) {
                applied.notes.push(format!(
                    "{} did not go into {}: {error}.",
                    insert.title, insert.drive
                ));
                continue;
            }
        }
        applied.done.push(insert);
    }
    applied
}

/// One pending insert into its drive; the session changes only once every
/// disk of it checked out.
fn apply_one(
    session: &mut pc_image::workspace::PreparedSession,
    workspace: &Path,
    insert: &CreatedInsert,
) -> Result<()> {
    let unit = match insert.drive.as_str() {
        "A:" => 0,
        "B:" => 1,
        _ => return Err(format!("{} is not a floppy drive", insert.drive)),
    };
    let drive = session
        .machine
        .drives
        .iter_mut()
        .find(|d| d.unit == unit && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
        .ok_or_else(|| format!("{} is not a floppy drive here", insert.drive))?;
    let assembly::DriveKind::Floppy { geometry } = drive.kind else {
        return Err("floppy drive expected".into());
    };
    let mut media = Vec::new();
    let mut bases = Vec::new();
    for disk in &insert.disks {
        let path = workspace
            .join("media")
            .join(FOLDER)
            .join(format!("{}.img", disk.sha256));
        let file = fs::File::open(&path).map_err(|e| format!("{}: {e}", disk.name))?;
        let base = media::VerifiedBase::from_file(file).map_err(|e| e.to_string())?;
        let hash = base.identity().base_digest;
        if hex(&hash) != disk.sha256 {
            return Err(format!(
                "{}: the image no longer matches its SHA-256",
                disk.name
            ));
        }
        if !geometry
            .medium_for_len(base.byte_len())
            .is_some_and(|medium| geometry.accepted_media().contains(&medium))
        {
            return Err(format!(
                "{} does not take {}",
                assembly::descriptors::floppy_drive_media(geometry),
                disk.name
            ));
        }
        let write = if insert.journal {
            assembly::WriteMode::journal(
                workspace
                    .join("media")
                    .join(format!("{}.upfj", disk.sha256)),
            )
        } else {
            assembly::WriteMode::ReadOnly
        };
        bases.push((path.clone(), base));
        media.push(assembly::FloppyMedium {
            image: assembly::ImageSource::File(path),
            expected_sha256: Some(hash),
            write,
            name: disk.name.clone(),
        });
    }
    session.bases.extend(bases);
    // The chosen disk leads, so the drive starts with it.
    if !media.is_empty() {
        let first = media.remove(insert.first());
        media.insert(0, first);
    }
    match &mut drive.media {
        Some(list) => {
            list.entries
                .retain(|e| !media.iter().any(|m| m.expected_sha256 == e.expected_sha256));
            media.append(&mut list.entries);
            list.entries = media;
            list.empty = false;
        }
        None => {
            // A drive with one fixed image becomes a catalogue for this run.
            media.push(assembly::FloppyMedium {
                image: drive.image.clone(),
                expected_sha256: drive.expected_sha256,
                write: drive.write.clone(),
                name: format!("{} image", insert.drive),
            });
            drive.media = Some(assembly::FloppyMediaList {
                entries: media,
                empty: false,
            });
            drive.image = assembly::ImageSource::Blank {
                size: drive.kind.image_bytes(),
            };
            drive.expected_sha256 = None;
            drive.write = assembly::WriteMode::Volatile;
        }
    }
    Ok(())
}

/// The catalogue entries of disks made into a media folder, in order, as the
/// machine's own disks: read-only bases pinned by SHA-256, with guest writes
/// in the folder's journals (`retention` closed generations kept) or
/// refused.
pub(crate) fn entries(
    created: &Created,
    retention: u32,
) -> Vec<crate::media_dialog::CatalogueEntry> {
    let Place::Folder(media) = &created.place else {
        return Vec::new();
    };
    let count = created.disks.len();
    created
        .disks
        .iter()
        .enumerate()
        .map(
            |(index, (path, digest, name))| crate::media_dialog::CatalogueEntry {
                source: None,
                receipt: None,
                readonly_bytes: None,
                medium: assembly::FloppyMedium {
                    image: assembly::ImageSource::File(path.clone()),
                    expected_sha256: Some(*digest),
                    write: if created.order.journal {
                        assembly::WriteMode::Journal {
                            path: media.join(format!("{}.upfj", hex(digest))),
                            keep_generations: retention,
                        }
                    } else {
                        assembly::WriteMode::ReadOnly
                    },
                    name: name.clone(),
                },
                geometry: geometry(created.order.format),
                reason: None,
                index: None,
                declared: false,
                alternates: Vec::new(),
                workspace_journal: None,
                origin: Some(format!(
                    "Created from files · disk {} of {count}",
                    index + 1
                )),
                verified: created.verified.get(index).cloned(),
            },
        )
        .collect()
}

/// Build the order's disks and store them where `place` says: for a stopped
/// library machine, as the drive's pending insert in its workspace. With
/// `keep`, the disks also become an installation media set of the library.
pub(crate) fn create(
    store: &image_store::Store,
    order: CreateOrder,
    place: Place,
) -> Result<Created> {
    let survey = Survey::new(&order.items, order.top, order.format, order.label)
        .map_err(|e| e.to_string())?;
    let plan = if order.split {
        survey.plan_split()
    } else {
        survey.plan()
    }
    .map_err(|e| e.to_string())?;
    let images = plan.images().map_err(|e| e.to_string())?;
    let count = images.len();
    let drive = crate::media_picker::drive_name(order.unit);
    let store_all = |media: &Path| -> Result<Vec<(PathBuf, [u8; 32], String)>> {
        images
            .iter()
            .enumerate()
            .map(|(index, image)| {
                let (path, digest) = store_image(media, image).map_err(|e| e.to_string())?;
                Ok((path, digest, disk_name(&order.title, index, count)))
            })
            .collect()
    };
    let disks = match &place {
        Place::Folder(media) => store_all(media)?,
        Place::Machine(id) => {
            let owner = pc_image::machine::own(store, id).map_err(|e| e.to_string())?;
            if owner
                .read("session.toml")
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err("the machine has no workspace session yet; start it once first".into());
            }
            let disks = store_all(&owner.path().join("media"))?;
            let before = read_pending(owner.path()).ok();
            set_pending(
                owner.path(),
                &drive,
                Some(CreatedInsert {
                    drive: drive.clone(),
                    title: order.title.clone(),
                    disks: disks
                        .iter()
                        .map(|(_, digest, name)| CreatedDisk {
                            sha256: hex(digest),
                            name: name.clone(),
                        })
                        .collect(),
                    journal: order.journal,
                    first: None,
                }),
            )?;
            drop(owner);
            // Another Insert into the same drive replaces its pending disk.
            let listed = pc_image::machine::get(store, id)
                .map_err(|e| e.to_string())?
                .machine
                .pending
                .iter()
                .any(|p| p.drive == drive);
            if listed {
                if let Err(error) = pc_image::machine::set_pending_insert(store, id, &drive, None) {
                    // Leave the drive as it was: its disk from the library
                    // stays and the created one goes again.
                    if let (Ok(owner), Some(before)) = (pc_image::machine::own(store, id), before) {
                        let _ = write_pending(owner.path(), before);
                    }
                    return Err(error.to_string());
                }
            }
            disks
        }
    };
    // A running machine takes the disks as they were hashed here, off the
    // window's thread; a stopped one hashes them again at its Start.
    let verified = match &place {
        Place::Folder(_) => disks
            .iter()
            .map(|(path, digest, _)| Verified::open(path, digest))
            .collect::<io::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?,
        Place::Machine(_) => Vec::new(),
    };
    let (kept, keep_failed) = match &order.keep {
        Some((tag, title)) => match keep(
            store,
            tag,
            title,
            order.format,
            &disks,
            &export_names(order.top.as_ref(), order.label.as_ref(), disks.len()),
        ) {
            Ok(tag) => (Some(tag), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, None),
    };
    Ok(Created {
        order,
        place,
        disks,
        verified,
        kept,
        keep_failed,
    })
}

/// The file names the disks of a kept set export to, rather than the names
/// of their image files: the top folder the builder used, as
/// `<top>-NN.img` with two digits (`AZURAMON-03.img`) or `<top>.img` for
/// one disk; without a top folder, or when it gives no valid export file
/// name, the volume label the same way; failing both, `disk-NN.img`.
pub(crate) fn export_names(
    top: Option<&ShortName>,
    label: Option<&[u8; 11]>,
    count: usize,
) -> Vec<String> {
    let named = |stem: &str| -> Vec<String> {
        (1..=count)
            .map(|n| {
                if count == 1 {
                    format!("{stem}.img")
                } else {
                    format!("{stem}-{n:02}.img")
                }
            })
            .collect()
    };
    let label = label.map(|label| String::from_utf8_lossy(label).trim_end().to_owned());
    top.map(ShortName::to_string)
        .into_iter()
        .chain(label)
        .filter(|stem| !stem.is_empty())
        .map(|stem| named(&stem))
        .find(|names| {
            names
                .iter()
                .all(|name| image_store::export_name::input_problem(name).is_none())
        })
        .unwrap_or_else(|| (1..=count).map(|n| format!("disk-{n:02}.img")).collect())
}

/// Publish the disks as a private installation media set under `tag`, or
/// under the first free `.1`, `.2` … of its version when another set has it,
/// each disk exported under its name in `names`.
fn keep(
    store: &image_store::Store,
    tag: &str,
    title: &str,
    format: FloppyFormat,
    disks: &[(PathBuf, [u8; 32], String)],
    names: &[String],
) -> Result<String> {
    use pc_image::media_import;
    let count = disks.len();
    let (repository, version) = tag.rsplit_once(':').unwrap_or((tag, "1.0"));
    let mut last = String::new();
    for n in 0..100 {
        let tag = if n == 0 {
            format!("{repository}:{version}")
        } else {
            format!("{repository}:{version}.{n}")
        };
        let request = media_import::Request {
            tag: tag.clone(),
            title: image_store::metadata::Title {
                id: "title".into(),
                title: title.into(),
                ..Default::default()
            },
            labels: vec!["from files".into()],
            media: disks
                .iter()
                .enumerate()
                .zip(names)
                .map(|((index, (path, _, _)), name)| media_import::Source {
                    path: path.clone(),
                    label: Some(if count > 1 {
                        format!("Disk {} of {count}", index + 1)
                    } else {
                        "Disk 1".into()
                    }),
                    format: Some(format.name().into()),
                    file_name: Some(name.clone()),
                })
                .collect(),
            extras: Vec::new(),
            compress: true,
        };
        let taken = store.tag_descriptor(&tag).is_ok();
        let published = media_import::plan(&request)
            .and_then(|plan| media_import::publish(store, &plan, &|_, _| (), &|| false));
        match published {
            Ok(_) => return Ok(tag),
            Err(error) if taken => last = error.to_string(),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image is stored once under its SHA-256; a damaged file of that
    /// name is replaced by a new directory entry, never written in place.
    #[test]
    fn created_images_are_stored_once_by_their_sha256() {
        let temp = assembly::test_support::TempDir::new("created-store");
        let media = temp.path().join("media");
        let bytes = vec![0x5a; 1_474_560];
        let (path, digest) = store_image(&media, &bytes).unwrap();
        assert_eq!(
            path,
            media.join(FOLDER).join(format!("{}.img", hex(&digest)))
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let (again, _) = store_image(&media, &bytes).unwrap();
        assert_eq!(again, path);
        assert_eq!(fs::read_dir(media.join(FOLDER)).unwrap().count(), 1);
        // A damaged copy is replaced by a new inode; a reader of the old one
        // keeps its bytes.
        fs::write(&path, b"damaged").unwrap();
        let old = fs::File::open(&path).unwrap();
        store_image(&media, &bytes).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let mut kept = Vec::new();
        std::io::Read::read_to_end(&mut &old, &mut kept).unwrap();
        assert_eq!(kept, b"damaged");
    }

    /// Two first creates of one image race to publish it: the one that comes
    /// second finds the same bytes there and is done; other bytes under the
    /// name are still an error.
    #[test]
    fn a_create_that_finds_its_image_published_meanwhile_is_done() {
        let temp = assembly::test_support::TempDir::new("created-race");
        let bytes = vec![0x33; 737_280];
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let path = temp.path().join(format!("{}.img", hex(&digest)));
        publish_image(&path, &bytes, &digest, false).unwrap();
        publish_image(&path, &bytes, &digest, false).unwrap();
        let other: [u8; 32] = Sha256::digest(b"other").into();
        let error = publish_image(&path, &bytes, &other, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    /// A record that cannot be read, or that a newer version wrote, is
    /// replaced by the next change rather than refusing it.
    #[test]
    fn an_unreadable_record_is_replaced_by_the_next_change() {
        let temp = assembly::test_support::TempDir::new("created-unreadable");
        let workspace = temp.path();
        std::fs::create_dir_all(workspace.join("media").join(FOLDER)).unwrap();
        for text in ["not toml at all", "schema = 2\n"] {
            fs::write(pending_path(workspace), text).unwrap();
            assert!(read_pending(workspace).is_err(), "{text}");
            set_pending(workspace, "A:", None).unwrap();
            assert!(!pending_path(workspace).exists(), "{text}");
            fs::write(pending_path(workspace), text).unwrap();
            let insert = CreatedInsert {
                drive: "B:".into(),
                title: "GAME".into(),
                disks: Vec::new(),
                journal: false,
                first: None,
            };
            set_pending(workspace, "B:", Some(insert.clone())).unwrap();
            assert_eq!(read_pending(workspace).unwrap(), [insert]);
            fs::remove_file(pending_path(workspace)).unwrap();
        }
    }

    /// The pending record keeps one insert per drive and goes once empty.
    #[test]
    fn the_pending_record_keeps_one_insert_per_drive() {
        let temp = assembly::test_support::TempDir::new("created-pending");
        let workspace = temp.path();
        let insert = |drive: &str, title: &str| CreatedInsert {
            drive: drive.into(),
            title: title.into(),
            disks: vec![CreatedDisk {
                sha256: "00".repeat(32),
                name: title.into(),
            }],
            journal: true,
            first: None,
        };
        assert!(read_pending(workspace).unwrap().is_empty());
        set_pending(workspace, "A:", Some(insert("A:", "ONE"))).unwrap();
        set_pending(workspace, "B:", Some(insert("B:", "TWO"))).unwrap();
        set_pending(workspace, "A:", Some(insert("A:", "THREE"))).unwrap();
        assert_eq!(
            read_pending(workspace).unwrap(),
            [insert("B:", "TWO"), insert("A:", "THREE")]
        );
        confirm(workspace, &[insert("B:", "TWO")]).unwrap();
        set_pending(workspace, "A:", None).unwrap();
        assert!(read_pending(workspace).unwrap().is_empty());
        assert!(!pending_path(workspace).exists());
    }

    /// A kept disk exports under its top folder's name, not under the
    /// SHA-256 name of its image file.
    #[test]
    fn kept_disks_export_under_the_top_folder_numbered() {
        let top = ShortName::parse("azuramon");
        let label = media::fat12::volume_label("GAMES").ok();
        assert_eq!(
            export_names(top.as_ref(), label.as_ref(), 3),
            ["AZURAMON-01.img", "AZURAMON-02.img", "AZURAMON-03.img"]
        );
        assert_eq!(
            export_names(top.as_ref(), label.as_ref(), 1),
            ["AZURAMON.img"]
        );
        assert_eq!(
            export_names(None, label.as_ref(), 2),
            ["GAMES-01.img", "GAMES-02.img"]
        );
        assert_eq!(export_names(None, None, 2), ["disk-01.img", "disk-02.img"]);
        assert_eq!(export_names(None, None, 1), ["disk-01.img"]);
        // A name Windows reserves for a device is no export file name.
        let device = media::fat12::volume_label("CONIN$").ok();
        assert_eq!(export_names(None, device.as_ref(), 1), ["disk-01.img"]);
        assert_eq!(
            export_names(None, device.as_ref(), 2),
            ["CONIN$-01.img", "CONIN$-02.img"]
        );

        let temp = assembly::test_support::TempDir::new("created-keep-names");
        let game = temp.path().join("galaxia");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("a.dat"), vec![1; 900_000]).unwrap();
        fs::write(game.join("b.dat"), vec![2; 900_000]).unwrap();
        let store = image_store::Store::open(temp.path().join("store")).unwrap();
        let order = CreateOrder {
            unit: 0,
            items: vec![HostItem::read(&game).unwrap()],
            top: ShortName::parse("GALAXIA"),
            format: FloppyFormat::Hd144,
            label: media::fat12::volume_label("GALAXIA").ok(),
            split: true,
            journal: true,
            keep: Some(("galaxia:1.0-r1".into(), "GALAXIA".into())),
            title: "GALAXIA".into(),
        };
        let created = create(&store, order, Place::Folder(temp.path().join("media"))).unwrap();
        assert_eq!(
            created.kept.as_deref(),
            Some("galaxia:1.0-r1"),
            "{:?}",
            created.keep_failed
        );
        let set = store
            .resolve(
                "galaxia:1.0-r1",
                &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
            )
            .unwrap();
        let titles: Vec<_> = set
            .manifest
            .layers
            .iter()
            .map(|d| image_store::export_name::of(d).unwrap_or("<none>"))
            .collect();
        assert_eq!(
            titles,
            ["GALAXIA-01.img.zst", "GALAXIA-02.img.zst", "metadata.json"]
        );
    }
}
