// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machines for the PC provider: named, local, mutable instances of one image;
//! machine-images §3.2.
//!
//! A machine is a record in the store's `local/machines.toml`
//! ([`image_store::machine`]) plus the workspace of the same id, which holds
//! its session pin and its disk, floppy and CMOS journals. The image stays
//! immutable: changes live in the journals, the machine's settings overlay
//! (hardware tables that replace the image's from the next Start) and its
//! pending inserts. Every operation that changes its journals, settings or
//! pending inserts holds its workspace lock ([`own`]), so a running machine
//! refuses them; the record is edited under the index lock afterwards,
//! keeping the store's lock order. Rename, favourite and labels touch only
//! the record and work while the machine runs.
//!
//! CMOS follows Settings on an image (`revision`): when the machine's
//! settings change a field that AT SETUP records in CMOS (IBM Personal
//! Computer AT Technical Reference, System Board, Real-Time Clock/CMOS RAM
//! Information), only that field and the checksum are rewritten, in the
//! machine's CMOS journal when the image journals CMOS, or at every Start when
//! it copies a stored CMOS image afresh.

use crate::ConfigDigest;
use crate::{
    decode, invalid, revision, workspace, Config, NvramMode, Policy, Result, CMOS, PAYLOAD_TYPES,
};
use image_store::{
    machine::{validate_name, Machine, Machines, Origin, PendingInsert},
    workspace::Workspace,
    BlobSource, Descriptor, Digest, Manifest, Store, Tag, MAX_SMALL_BLOB,
};
use media::SectorSource;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
};
use toml::Table;

/// What a migration did. Skipped workspaces stay as they are and are tried
/// again next time; a workspace another process is using (a running desktop
/// session) waits silently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migration {
    /// Ids of the machines adopted now.
    pub adopted: Vec<String>,
    /// Workspaces or machines left as they are, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Open an existing store for machine work. Older workspaces become machines
/// first ([`migrate`]).
pub fn open_store(root: impl AsRef<Path>) -> Result<(Store, Migration)> {
    let store = Store::open_existing(root)?;
    let migration = migrate(&store)?;
    Ok((store, migration))
}

/// What a workspace was made for before machines existed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The desktop's one persistent workspace per image.
    Regular,
    /// A Gaming-mode copy, kept until the user chooses Keep or Discard.
    Disposable,
    /// A workspace named on the command line.
    Named,
}

fn kind(name: &str, desktop: Option<&Table>) -> Kind {
    let recorded = desktop
        .and_then(|t| t.get("kind"))
        .and_then(toml::Value::as_str);
    if name.starts_with("disposable-")
        || name.starts_with("fresh-")
        || recorded == Some("disposable")
    {
        Kind::Disposable
    } else if name.starts_with("regular-")
        || name.starts_with("gaming-")
        || recorded == Some("regular")
    {
        // Older desktop builds named persistent sessions `gaming-`.
        Kind::Regular
    } else {
        Kind::Named
    }
}

/// Whether an error is another owner holding a workspace lock.
fn busy(error: &crate::Error) -> bool {
    matches!(error, crate::Error::Store(image_store::Error::Invalid(message))
        if message.starts_with("workspace is busy"))
}

/// Whether a workspace directory holds anything of a machine: a session, a
/// pending publication or discard, or any media file.
fn holds_data(path: &Path) -> Result<bool> {
    for name in ["session.toml", "transaction.toml", "discard.toml"] {
        if path.join(name).exists() {
            return Ok(true);
        }
    }
    match fs::read_dir(path.join("media")) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Make every unadopted workspace with a session a machine, without losing
/// any data. A desktop workspace (`regular-` from the image digest) becomes a
/// machine named after its image's title (then `title (2)`, …), with a new id
/// from that name; its directory moves there and it takes the image's
/// favourite star and last run. A workspace named on the command line keeps
/// its name as both id and name. Gaming-mode workspaces stay as they are. A
/// desktop workspace made again under an adopted name is adopted again, as a
/// new machine. One damaged workspace is reported and skipped; the others
/// still migrate. Idempotent.
pub fn migrate(store: &Store) -> Result<Migration> {
    let mut report = Migration::default();
    for (id, machine) in store.read_machines()?.iter() {
        if machine.migrated_from.is_none() {
            continue;
        }
        match finish_move(store, id) {
            Ok(_) => (),
            Err(error) if busy(&error) => (),
            Err(error) => report.skipped.push((id.clone(), error.to_string())),
        }
    }
    let machines = store.read_machines()?;
    let adopted: BTreeSet<String> = machines
        .iter()
        .flat_map(|(id, m)| std::iter::once(id.clone()).chain(m.migrated_from.clone()))
        .collect();
    let library = store.read_library()?;
    let tags = all_tags(store)?;
    for name in workspace_names(store, &mut report)? {
        if adopted.contains(&name) {
            continue;
        }
        match adopt(store, &library, &tags, &name) {
            Ok(Some(id)) => report.adopted.push(id),
            Ok(None) => (),
            Err(error) if busy(&error) => (),
            Err(error) => report.skipped.push((name, error.to_string())),
        }
    }
    Ok(report)
}

/// Workspace directories with a session, by name. A directory no workspace
/// can be named is reported, never opened.
fn workspace_names(store: &Store, report: &mut Migration) -> Result<Vec<String>> {
    let entries = match fs::read_dir(store.root().join("workspaces")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || !entry.path().join("session.toml").is_file() {
            continue;
        }
        match entry.file_name().to_str() {
            Some(name) if store.workspace_path(name).is_ok() => names.push(name.to_owned()),
            _ => report.skipped.push((
                entry.file_name().to_string_lossy().into_owned(),
                "not a valid workspace name".into(),
            )),
        }
    }
    names.sort();
    Ok(names)
}

/// Adopt one workspace; `None` when it stays as it is.
fn adopt(
    store: &Store,
    library: &image_store::library::Library,
    tags: &[Tag],
    name: &str,
) -> Result<Option<String>> {
    let desktop = read_desktop(store, name);
    let kind = kind(name, desktop.as_ref());
    if kind == Kind::Disposable {
        return Ok(None);
    }
    // Opening never creates anything: a workspace another process has just
    // moved is simply gone.
    let owner = store.open_workspace(name)?;
    let Some(image) = workspace::pinned_descriptor(&owner)? else {
        return Ok(None);
    };
    let image = identity(&image);
    let text = |key: &str| {
        desktop
            .as_ref()
            .and_then(|t| t.get(key))
            .and_then(toml::Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    };
    let source = tags
        .iter()
        .find(|t| t.manifest.digest == image.digest)
        .map(|t| t.name.clone())
        .or_else(|| text("reference").filter(|r| Store::validate_tag_name(r).is_ok()))
        .unwrap_or_default();
    if kind == Kind::Named {
        return Ok(store.edit_machines(|machines| {
            if taken(machines, name) {
                return Ok(None);
            }
            let machine = Machine::new(
                &machines.unique_name(name),
                image.clone(),
                &source,
                Origin::Migration,
            );
            machines.insert(name, machine)?;
            Ok(Some(name.to_owned()))
        })?);
    }
    let title = image_title(store, &image)
        .ok()
        .flatten()
        .or_else(|| text("title"))
        .unwrap_or_else(|| fallback_name(&source));
    let entry = library
        .images
        .get(&image.digest)
        .cloned()
        .unwrap_or_default();
    // The record names the old id until the move is made, so an interrupted
    // move completes on the next migration.
    let id = store.edit_machines(|machines| {
        if taken(machines, name) {
            return Ok(None);
        }
        let id = store.free_machine_id(machines, &title)?;
        let mut machine = Machine::new(
            &machines.unique_name(&title),
            image.clone(),
            &source,
            Origin::Migration,
        );
        machine.favorite = entry.favorite;
        machine.last_run = entry.last_run.clone();
        machine.migrated_from = Some(name.to_owned());
        machines.insert(&id, machine)?;
        Ok(Some(id))
    })?;
    let Some(id) = id else {
        return Ok(None);
    };
    move_into(store, owner, &id)?;
    Ok(Some(id))
}

fn taken(machines: &Machines, workspace: &str) -> bool {
    machines
        .iter()
        .any(|(id, m)| id == workspace || m.migrated_from.as_deref() == Some(workspace))
}

/// Move a desktop workspace a migration recorded to its machine's id. An
/// empty leftover directory at the id is removed first; one with data is
/// never touched. The desktop's own record goes, since its sessions are the
/// machine's now, and the machine record forgets the old id, so a desktop
/// workspace made there later is adopted as a machine of its own.
fn move_into<'a>(store: &'a Store, mut owner: Workspace<'a>, id: &str) -> Result<Workspace<'a>> {
    let target = store.workspace_path(id)?;
    if target.exists() {
        if holds_data(&target)? {
            return Err(invalid(format!(
                "workspace {id} exists already and holds data"
            )));
        }
        store.workspace(id)?.discard_and_remove()?;
    }
    owner.rename(id)?;
    owner.remove("desktop.toml")?;
    edit(store, id, |m| m.migrated_from = None)?;
    Ok(owner)
}

/// Finish or settle the move of a machine adopted from an older workspace,
/// returning the moved workspace when it moved now. With nothing left to move
/// (the old directory is gone or empty, or both directories hold data) the
/// record forgets the old id; a workspace still there with a session is then
/// adopted later as a new machine.
fn finish_move<'a>(store: &'a Store, id: &str) -> Result<Option<Workspace<'a>>> {
    let Some(from) = record(store, id)?.migrated_from else {
        return Ok(None);
    };
    let source = store.workspace_path(&from)?;
    if source.exists() && holds_data(&source)? && !holds_data(&store.workspace_path(id)?)? {
        let owner = store.open_workspace(&from)?;
        return move_into(store, owner, id).map(Some);
    }
    edit(store, id, |m| m.migrated_from = None)?;
    Ok(None)
}

/// The desktop's own record in a workspace, read only as a hint.
fn read_desktop(store: &Store, name: &str) -> Option<Table> {
    let path = store.workspace_path(name).ok()?.join("desktop.toml");
    let bytes = fs::read(path).ok()?;
    if bytes.len() as u64 > MAX_SMALL_BLOB {
        return None;
    }
    std::str::from_utf8(&bytes).ok()?.parse().ok()
}

fn all_tags(store: &Store) -> Result<Vec<Tag>> {
    let mut tags = Vec::new();
    let mut after = None;
    loop {
        let page = store.list_tags(after.as_deref(), 1000)?;
        tags.extend(page.tags);
        after = page.next_after;
        if after.is_none() {
            return Ok(tags);
        }
    }
}

/// An image's library title: its title annotation, else its first catalogue
/// title. `None` when it has neither.
pub fn image_title(store: &Store, image: &Descriptor) -> Result<Option<String>> {
    let manifest = Manifest::decode_for_summary(&store.read_verified(image, MAX_SMALL_BLOB)?)?;
    if let Some(title) = manifest
        .annotations
        .get(workspace::TITLE)
        .filter(|t| !t.trim().is_empty())
    {
        return Ok(Some(title.trim().into()));
    }
    let Some(metadata) = manifest
        .layers
        .iter()
        .find(|d| d.media_type == image_store::media_type::METADATA)
    else {
        return Ok(None);
    };
    let metadata = image_store::metadata::Metadata::decode(
        &store.read_verified(metadata, MAX_SMALL_BLOB)?,
        &manifest.layers,
    )?;
    Ok(metadata
        .titles
        .first()
        .map(|t| t.title.trim().to_owned())
        .filter(|t| !t.is_empty()))
}

/// The identity a machine pins: the manifest descriptor without the
/// annotations of the local tag it was resolved through, its name, where
/// the tag came from and where it was pushed.
fn identity(image: &Descriptor) -> Descriptor {
    let mut image = image.clone();
    image
        .annotations
        .remove("org.opencontainers.image.ref.name");
    image.annotations.remove(image_store::PULL_REGISTRY);
    image.annotations.remove(image_store::MADE_HERE);
    image.annotations.remove(image_store::BUILD_RECIPE);
    image.annotations.remove(image_store::PUSHED_TO);
    image
}

fn fallback_name(reference: &str) -> String {
    if reference.is_empty() {
        "Machine".into()
    } else {
        reference.into()
    }
}

fn clash(machines: &Machines, name: &str) -> image_store::Error {
    let existing = machines
        .find_name(name)
        .and_then(|id| machines.get(id))
        .map_or(name, |m| m.name.as_str());
    image_store::Error::Invalid(format!(
        "a machine named \u{201c}{existing}\u{201d} exists already; names are unique, ignoring case"
    ))
}

/// Make a machine from `image`, remembered as made from `source`
/// (`repository:tag`, or empty). Without `name` it is named after the image
/// title, then `title (2)`, … . Its id is a slug of the name. The workspace is
/// pinned to the image at once; nothing is verified or materialized until it
/// starts. `created` is host text for the library, like a last run. An image
/// that declares a newer release than this build reads is refused first,
/// with that release (machine-images §5.6): a machine made of it could not
/// start.
pub fn create(
    store: &Store,
    image: &Descriptor,
    source: &str,
    name: Option<&str>,
    created: Option<String>,
) -> Result<String> {
    let reference = if source.is_empty() {
        image.digest.to_string()
    } else {
        source.to_owned()
    };
    image_store::compat::Requirement::read(&store.read_verified(image, MAX_SMALL_BLOB)?)
        .check(&reference)?;
    let image = &identity(image);
    let name = name.map(validate_name).transpose()?;
    let title = match &name {
        Some(_) => String::new(),
        None => image_title(store, image)?.unwrap_or_else(|| fallback_name(source)),
    };
    let id = store.edit_machines(|machines| {
        let name = match &name {
            Some(name) if machines.find_name(name).is_some() => return Err(clash(machines, name)),
            Some(name) => name.clone(),
            None => machines.unique_name(&title),
        };
        let id = store.free_machine_id(machines, &name)?;
        let mut machine = Machine::new(&name, image.clone(), source, Origin::New);
        machine.created = created;
        machines.insert(&id, machine)?;
        Ok(id)
    })?;
    if let Err(error) = store
        .workspace(&id)
        .map_err(crate::Error::from)
        .and_then(|owner| workspace::pin(&owner, image))
    {
        let _ = store
            .workspace(&id)
            .and_then(|owner| owner.discard_and_remove());
        let _ = store.edit_machines(|machines| Ok(machines.remove(&id)));
        return Err(error);
    }
    Ok(id)
}

/// [`create`] from the image a local tag names now.
pub fn create_from_tag(
    store: &Store,
    reference: &str,
    name: Option<&str>,
    created: Option<String>,
) -> Result<String> {
    let image = store.tag_descriptor(reference)?;
    create(store, &image, reference, name, created)
}

/// What the library shows in a machine's State column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// A process holds the machine's workspace lock.
    Running,
    /// Never run and nothing changed.
    NeverStarted,
    /// Run before; its journals hold nothing the image does not.
    SameAsImage,
    /// Changed drives in drive order, then media inserted from installation
    /// sets, CMOS and settings.
    Changed {
        drives: Vec<String>,
        inserted: bool,
        cmos: bool,
        settings: bool,
    },
}

/// The `drive` of a [`Change`] in a medium inserted from an installation set:
/// its journal belongs to no drive of the image.
pub const INSERTED: &str = "inserted";

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => f.write_str("Running"),
            Self::NeverStarted => f.write_str("Never started"),
            Self::SameAsImage => f.write_str("Same as image"),
            Self::Changed {
                drives,
                inserted,
                cmos,
                settings,
            } => {
                let parts: Vec<&str> = drives
                    .iter()
                    .map(String::as_str)
                    .chain(inserted.then_some("inserted media"))
                    .chain(cmos.then_some("CMOS"))
                    .chain(settings.then_some("settings"))
                    .collect();
                write!(f, "Changed: {}", parts.join(", "))
            }
        }
    }
}

/// One medium whose journal holds writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The drive, such as `A:` or `C:`, or [`INSERTED`] for a medium inserted
    /// from an installation set.
    pub drive: String,
    /// The medium id in the image configuration, or the journal name
    /// (`<set digest hex>-<medium id>`) of an inserted medium.
    pub medium: String,
    /// Sectors written, `None` when the journal needs a recovery scan to tell.
    pub sectors: Option<usize>,
}

/// A machine as the library lists it.
#[derive(Clone, Debug)]
pub struct Listing {
    pub id: String,
    /// The record, its image read from the workspace's session pin.
    pub machine: Machine,
    pub state: State,
    pub changes: Vec<Change>,
    /// Bytes of the machine's journals on the host.
    pub change_bytes: u64,
    /// The tags that name the machine's image now; none means detached.
    pub tags: Vec<String>,
    /// The image the tag the machine was made from names now, when it moved.
    pub moved_to: Option<Descriptor>,
    /// An unfinished Save as new image… blocks Start until it is retried.
    pub publication_pending: bool,
    /// Why the image or the workspace could not be read, if so. The rest of
    /// the listing then shows what could be read.
    pub problem: Option<String>,
}

impl Listing {
    /// No tag names the machine's image any more; it stays for the machine.
    pub fn detached(&self) -> bool {
        self.tags.is_empty()
    }
}

/// Every machine, by id.
pub fn list(store: &Store) -> Result<Vec<Listing>> {
    let machines = store.read_machines()?;
    let tags = all_tags(store)?;
    machines
        .iter()
        .map(|(id, machine)| listing(store, &tags, id, machine))
        .collect()
}

/// One machine, as [`list`] shows it.
pub fn get(store: &Store, id: &str) -> Result<Listing> {
    let machines = store.read_machines()?;
    let machine = machines
        .get(id)
        .ok_or_else(|| invalid(format!("no machine {id}")))?;
    listing(store, &all_tags(store)?, id, machine)
}

fn listing(store: &Store, tags: &[Tag], id: &str, record: &Machine) -> Result<Listing> {
    let mut machine = record.clone();
    let mut problem = None;
    let mut noted = |error: crate::Error| {
        problem.get_or_insert(error.to_string());
    };
    match workspace::pinned_descriptor_at(store, id) {
        Ok(Some(image)) => machine.image = identity(&image),
        Ok(None) => (),
        Err(error) => noted(error),
    }
    let running = running(store, id).unwrap_or_else(|e| {
        noted(e);
        false
    });
    let (changes, cmos) = changes(store, id, &machine.image).unwrap_or_else(|e| {
        noted(e);
        (Vec::new(), false)
    });
    let change_bytes = store
        .workspace_path(id)
        .map_err(crate::Error::from)
        .and_then(|path| journal_bytes(&path.join("media")))
        .unwrap_or_else(|e| {
            noted(e);
            0
        });
    let publication_pending = workspace::pending_publication_at(store, id)
        .map(|p| p.is_some())
        .unwrap_or_else(|e| {
            noted(e);
            false
        });
    let settings = machine.settings.is_some();
    let state = if running {
        State::Running
    } else if !changes.is_empty() || cmos || settings {
        let mut drives: Vec<String> = changes
            .iter()
            .filter(|c| c.drive != INSERTED)
            .map(|c| c.drive.clone())
            .collect();
        drives.dedup();
        State::Changed {
            drives,
            inserted: changes.iter().any(|c| c.drive == INSERTED),
            cmos,
            settings,
        }
    } else if machine.last_run.is_none() {
        State::NeverStarted
    } else {
        State::SameAsImage
    };
    Ok(Listing {
        id: id.into(),
        state,
        changes,
        change_bytes,
        tags: tags
            .iter()
            .filter(|t| t.manifest.digest == machine.image.digest)
            .map(|t| t.name.clone())
            .collect(),
        moved_to: tags
            .iter()
            .find(|t| t.name == machine.source && t.manifest.digest != machine.image.digest)
            .map(|t| t.manifest.clone()),
        publication_pending,
        problem,
        machine,
    })
}

/// Whether another owner holds the workspace lock. The probe never creates
/// the lock file and releases its own attempt at once. It asks for a shared
/// lock: an owner holds the lock exclusively, so a running machine still
/// refuses the probe, while two listings probing at the same moment (the
/// window's relist and a command-line listing, or two threads) share it and
/// neither reads the other's probe as a running machine.
fn running(store: &Store, id: &str) -> Result<bool> {
    let path = store.workspace_path(id)?.join("workspace.lock");
    let file = match fs::OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(fs::TryLockError::WouldBlock) => Ok(true),
        Err(fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

fn journal_bytes(directory: &Path) -> Result<u64> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut total = 0u64;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|e| e == "upfj") {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

fn image_config(store: &Store, image: &Descriptor) -> Result<(Manifest, Config)> {
    let bytes = store.read_verified(image, MAX_SMALL_BLOB)?;
    // The config of an image a newer release wrote is not read: the listing
    // shows the release it needs instead of a decoder message.
    image_store::compat::Requirement::read(&bytes).check(&image.digest.to_string())?;
    let manifest = Manifest::decode_with_types(&bytes, PAYLOAD_TYPES)?;
    let bytes = store.read_verified(&manifest.config, MAX_SMALL_BLOB)?;
    let config: Config = decode(
        std::str::from_utf8(&bytes)
            .map_err(|e| invalid(e.to_string()))?
            .parse()
            .map_err(|e: toml::de::Error| invalid(e.to_string()))?,
    )?;
    Ok((manifest, config))
}

/// Drive names by (controller, unit): floppy units A: and B:, IDE master C:
/// and slave D:.
fn drive_names(config: &Config) -> Result<BTreeMap<(String, u8), String>> {
    let mut names = BTreeMap::new();
    crate::drive_tables(&mut config.machine.clone(), |controller, kind, drive| {
        let unit = crate::unit(kind, drive)?;
        let first = if kind == "ide" { b'C' } else { b'A' };
        names.insert(
            (controller.to_owned(), unit),
            format!("{}:", char::from(first + unit)),
        );
        Ok(())
    })?;
    Ok(names)
}

/// The journaled media holding writes, in drive order, and whether the CMOS
/// journal differs from the image's CMOS.
fn changes(store: &Store, id: &str, image: &Descriptor) -> Result<(Vec<Change>, bool)> {
    let (manifest, config) = image_config(store, image)?;
    let names = drive_names(&config)?;
    let mut changes = Vec::new();
    for slot in &config.slots {
        let drive = names
            .get(&(slot.controller.clone(), slot.unit))
            .cloned()
            .unwrap_or_else(|| format!("{}:{}", slot.controller, slot.unit));
        for medium in &slot.catalogue {
            let Some(m) = config.media.iter().find(|m| &m.id == medium) else {
                continue;
            };
            if m.policy != Policy::Journal {
                continue;
            }
            let sectors = crate::desktop::working_count(store, Some(id), &m.id, m.policy)?;
            if sectors != Some(0) {
                changes.push(Change {
                    drive: drive.clone(),
                    medium: m.id.clone(),
                    sectors,
                });
            }
        }
    }
    changes.sort_by(|a, b| a.drive.cmp(&b.drive));
    // Journals of media inserted from installation sets follow the drives.
    let known: BTreeSet<&str> = config
        .media
        .iter()
        .map(|m| m.id.as_str())
        .chain(["cmos"])
        .collect();
    let mut inserted = Vec::new();
    match fs::read_dir(store.workspace_path(id)?.join("media")) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                let Some(stem) = path
                    .extension()
                    .filter(|e| *e == "upfj")
                    .and(path.file_stem())
                    .and_then(|s| s.to_str())
                else {
                    continue;
                };
                if known.contains(stem) || media::JournalMedia::is_creation_temp_name(&path) {
                    continue;
                }
                let sectors =
                    crate::desktop::working_count(store, Some(id), stem, Policy::Journal)?;
                if sectors != Some(0) {
                    inserted.push(Change {
                        drive: INSERTED.into(),
                        medium: stem.into(),
                        sectors,
                    });
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    inserted.sort_by(|a, b| a.medium.cmp(&b.medium));
    changes.extend(inserted);
    let cmos = match &config.nvram {
        Some(nvram) if matches!(nvram.mode, NvramMode::Journal) => {
            let d = crate::descriptor(
                &manifest.layers,
                &crate::StoreDigest::store(&nvram.head)?,
                CMOS,
            )?;
            let base = store.read_verified(&d, 64)?;
            let current = current_cmos(&store.workspace_path(id)?, &base)?;
            current.is_none_or(|bytes| bytes[..] != base[..])
        }
        _ => false,
    };
    Ok((changes, cmos))
}

/// The CMOS bytes the machine's journal holds over `base`: `Some(base)`
/// without a journal, `None` when the journal needs recovery to be read.
fn current_cmos(workspace: &Path, base: &[u8]) -> Result<Option<[u8; 64]>> {
    let mut bytes: [u8; 64] = base
        .try_into()
        .map_err(|_| invalid("CMOS must contain exactly 64 bytes"))?;
    let mut file = match fs::File::open(workspace.join("media/cmos.upfj")) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Some(bytes)),
        Err(e) => return Err(e.into()),
    };
    let verified = media::VerifiedBase::from_bytes(base).map_err(|e| invalid(e.to_string()))?;
    let scan = media::scan_overlay_reader(&mut file, &verified, 64)?;
    if scan.report.needs_recovery() || !matches!(scan.report.status, media::Status::Valid) {
        return Ok(None);
    }
    let Some(overlay) = scan.overlay else {
        return Ok(None);
    };
    media::SectorSource::read_range(&overlay, 0, &mut bytes)?;
    Ok(Some(bytes))
}

fn record(store: &Store, id: &str) -> Result<Machine> {
    store
        .read_machines()?
        .get(id)
        .cloned()
        .ok_or_else(|| invalid(format!("no machine {id}")))
}

/// Own a stopped machine's workspace, for a run ([`prepare`]) or a change.
/// A running machine refuses. A move a migration recorded is finished first;
/// while the old desktop workspace is in use the machine refuses too, and no
/// empty workspace is ever made at its id meanwhile.
pub fn own<'a>(store: &'a Store, id: &str) -> Result<Workspace<'a>> {
    let machine = record(store, id)?;
    if let Some(from) = &machine.migrated_from {
        match finish_move(store, id) {
            Ok(Some(owner)) => return Ok(owner),
            Ok(None) => (),
            Err(error) if busy(&error) => {
                return Err(invalid(format!(
                    "machine {id} is still being moved from workspace {from}, which is in use; stop the desktop session first"
                )))
            }
            Err(error) => return Err(error),
        }
    }
    // A listing's running probe holds the lock for an instant, so a busy
    // workspace is tried again briefly before it counts as running.
    for attempt in 0..=20 {
        match store.workspace(id) {
            Ok(owner) => return Ok(owner),
            Err(image_store::Error::Invalid(message))
                if message.starts_with("workspace is busy") && attempt < 20 =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(image_store::Error::Invalid(message))
                if message.starts_with("workspace is busy") =>
            {
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid(format!("machine {id} is running; stop it first")))
}

fn owner<'a>(store: &'a Store, id: &str) -> Result<(Workspace<'a>, Machine)> {
    let owner = own(store, id)?;
    Ok((owner, record(store, id)?))
}

fn require_settled(owner: &Workspace<'_>) -> Result<()> {
    if owner.read("transaction.toml")?.is_some() {
        return Err(invalid(
            "publication pending; retry Save as new image or abandon it first",
        ));
    }
    if owner.read("discard.toml")?.is_some() {
        return Err(invalid("discard pending; reset the machine to finish it"));
    }
    Ok(())
}

/// The image the machine runs: its session pin, else its record.
fn pinned(owner: &Workspace<'_>, machine: &Machine) -> Result<Descriptor> {
    Ok(workspace::pinned_descriptor(owner)?
        .map(|image| identity(&image))
        .unwrap_or_else(|| machine.image.clone()))
}

/// Change only the display name; the id stays.
pub fn rename(store: &Store, id: &str, name: &str) -> Result<()> {
    Ok(store.edit_machines(|machines| machines.rename(id, name))?)
}

fn edit(store: &Store, id: &str, change: impl FnOnce(&mut Machine)) -> Result<()> {
    Ok(store.edit_machines(|machines| {
        change(
            machines
                .get_mut(id)
                .ok_or_else(|| image_store::Error::Invalid(format!("no machine {id}")))?,
        );
        Ok(())
    })?)
}

pub fn set_favorite(store: &Store, id: &str, favorite: bool) -> Result<()> {
    edit(store, id, |m| m.favorite = favorite)
}

/// Local labels, trimmed, sorted and without duplicates or empty ones.
pub fn set_labels(store: &Store, id: &str, labels: Vec<String>) -> Result<()> {
    let mut labels: Vec<String> = labels
        .into_iter()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    labels.sort();
    labels.dedup();
    edit(store, id, |m| m.labels = labels)
}

/// Replace the media a stopped machine gets at its next Start. Each names a
/// floppy drive (`A:` or `B:`) and a medium of an installation media set.
pub fn set_pending_inserts(store: &Store, id: &str, pending: Vec<PendingInsert>) -> Result<()> {
    let (_owner, _) = owner(store, id)?;
    for insert in &pending {
        floppy_unit(&insert.drive)?;
    }
    edit(store, id, |m| m.pending = pending)
}

/// Set or clear the disk drive `drive` (`A:` or `B:`) of a stopped machine
/// gets at its next Start, leaving its other drives' pending disks as they
/// are. The change is made on the record as it is now, under the index lock,
/// so two drives changed one after another both keep their disks.
pub fn set_pending_insert(
    store: &Store,
    id: &str,
    drive: &str,
    insert: Option<PendingInsert>,
) -> Result<()> {
    let (_owner, _) = owner(store, id)?;
    floppy_unit(drive)?;
    if insert.as_ref().is_some_and(|insert| insert.drive != drive) {
        return Err(invalid("the pending disk names another drive"));
    }
    edit(store, id, |m| {
        m.pending.retain(|p| p.drive != drive);
        m.pending.extend(insert);
    })
}

fn floppy_unit(drive: &str) -> Result<u8> {
    match drive {
        "A:" => Ok(0),
        "B:" => Ok(1),
        _ => Err(invalid(format!(
            "{drive}: media can be inserted only into floppy drive A: or B:"
        ))),
    }
}

/// Copy a stopped machine with its changes: journals, CMOS, settings, pending
/// inserts and labels, on the same image. The copy is independent: neither
/// sees the other's later writes. Without `name` it is the source's name,
/// then `name (2)`, … .
pub fn duplicate(
    store: &Store,
    id: &str,
    name: Option<&str>,
    created: Option<String>,
) -> Result<String> {
    let (source, machine) = owner(store, id)?;
    require_settled(&source)?;
    let image = pinned(&source, &machine)?;
    let name = name.map(validate_name).transpose()?;
    let new_id = store.edit_machines(|machines| {
        let name = match &name {
            Some(name) if machines.find_name(name).is_some() => return Err(clash(machines, name)),
            Some(name) => name.clone(),
            None => machines.unique_name(&machine.name),
        };
        let id = store.free_machine_id(machines, &name)?;
        let mut copy = Machine::new(&name, image.clone(), &machine.source, Origin::Duplicate);
        copy.created = created;
        copy.labels = machine.labels.clone();
        copy.pending = machine.pending.clone();
        copy.settings = machine.settings.clone();
        machines.insert(&id, copy)?;
        Ok(id)
    })?;
    let copied = (|| -> Result<()> {
        let target = store.workspace(&new_id)?;
        copy_workspace(source.path(), target.path())?;
        workspace::pin(&target, &image)
    })();
    if let Err(error) = copied {
        let _ = store
            .workspace(&new_id)
            .and_then(|owner| owner.discard_and_remove());
        let _ = store.edit_machines(|machines| Ok(machines.remove(&new_id)));
        return Err(error);
    }
    Ok(new_id)
}

/// Copy the session pin and every journal, never locks, the desktop's own
/// record, publication staging or journal temporaries.
fn copy_workspace(from: &Path, to: &Path) -> Result<()> {
    fn visit(from: &Path, to: &Path, top: bool) -> Result<()> {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                if top && (name != "media") {
                    continue;
                }
                fs::create_dir_all(to.join(&*name))?;
                visit(&path, &to.join(&*name), false)?;
            } else if entry.file_type()?.is_file() {
                let copied = if top {
                    name == "session.toml"
                } else {
                    !name.ends_with(".lock") && !media::JournalMedia::is_creation_temp_name(&path)
                };
                if copied {
                    let target = to.join(&*name);
                    fs::copy(&path, &target)?;
                    fs::File::open(&target)?.sync_all()?;
                }
            }
        }
        fs::File::open(to)?.sync_all()?;
        Ok(())
    }
    visit(from, to, true)
}

/// Drop a stopped machine's changes: journals, CMOS, settings and pending
/// inserts. Its name, id, favourite, labels and last run stay, and it stays
/// pinned to its image.
pub fn reset(store: &Store, id: &str) -> Result<()> {
    let (owner, machine) = owner(store, id)?;
    let image = pinned(&owner, &machine)?;
    owner.clear()?;
    reset_done(owner.path())?;
    edit(store, id, |m| {
        m.settings = None;
        m.pending.clear();
        m.image = image.clone();
    })?;
    workspace::pin(&owner, &image)
}

/// Delete a stopped machine with its workspace. The image and its tags stay.
pub fn delete(store: &Store, id: &str) -> Result<()> {
    let (owner, _) = owner(store, id)?;
    owner.discard_and_remove()?;
    store.edit_machines(|machines| Ok(machines.remove(id)))?;
    Ok(())
}

/// A machine's hardware tables beside its image's.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The image's hardware tables.
    pub image: Table,
    /// The tables the machine runs with: its overlay, else the image's.
    pub machine: Table,
    /// Settings that differ from the image, as a table diff.
    pub diff: Vec<revision::DiffLine>,
}

fn pretty(table: &Table) -> Result<String> {
    toml::to_string_pretty(table).map_err(|e| invalid(e.to_string()))
}

/// Read a machine's settings without taking its lock. The firmware bindings
/// its overlay records ([`revision::BOUND_FIRMWARE`]) are not hardware
/// tables and are left out.
pub fn settings(store: &Store, id: &str) -> Result<Settings> {
    let machine = record(store, id)?;
    let image = pinned_image(store, id)?;
    let source = revision::Source::open(store, &image)?;
    let image = source.machine().clone();
    let mut current = machine.settings.unwrap_or_else(|| image.clone());
    revision::take_bound(&mut current)?;
    Ok(Settings {
        diff: revision::line_diff(&pretty(&image)?, &pretty(&current)?),
        image,
        machine: current,
    })
}

/// A checked change of a machine's settings.
#[derive(Clone, Debug)]
pub struct SettingsChange {
    /// Settings that differ from the image afterwards; empty when the machine
    /// runs with the image's own settings again.
    pub diff: Vec<revision::DiffLine>,
    /// CMOS bytes the change rewrites in the machine's CMOS journal, or that
    /// every Start rewrites in a stored CMOS image copied afresh, or (without
    /// a stored CMOS) that Start derives differently.
    pub cmos_changes: Vec<revision::CmosChange>,
    /// Why stored CMOS bytes are kept as they are, when SETUP values cannot
    /// be derived for this machine.
    pub cmos_note: Option<String>,
    /// The new system BIOS keeps CMOS in another layout (or either BIOS is
    /// unlisted), so the machine's stored CMOS is reset to the bytes derived
    /// from its configuration: the SETUP choices stored there are lost, while
    /// the date and time come from Start every session. `false` when the CMOS
    /// stays or the image stores none.
    pub cmos_reset: bool,
    /// The CMOS layout of the system BIOS the machine runs afterwards; `None`
    /// for an unlisted BIOS.
    pub cmos_layout: Option<assembly::CmosLayout>,
}

/// A step of [`change_settings_with_hook`] after which a fault-injection
/// test may stop it, as a crash would.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsStep {
    /// The record holds the new settings; the CMOS journal is not written yet.
    Recorded,
}

/// The file beside the machine's CMOS journal that says the journal must be
/// reset to the bytes derived from its configuration: written before a change
/// of CMOS layout reaches the record and removed once the journal holds those
/// bytes. Start finishes a reset an interruption left behind. It lives with
/// the journals, so Duplicate… copies it and Reset to image… drops it.
const CMOS_RESET: &str = "media/cmos-reset.pending";

/// Whether the machine's CMOS waits for a reset its last settings change could
/// not finish.
pub fn cmos_reset_pending(store: &Store, id: &str) -> Result<bool> {
    Ok(store.workspace_path(id)?.join(CMOS_RESET).is_file())
}

fn reset_pending(workspace: &Path) -> bool {
    workspace.join(CMOS_RESET).is_file()
}

/// Announce a CMOS reset durably, before the record names the new BIOS.
fn announce_reset(workspace: &Path) -> Result<()> {
    let path = workspace.join(CMOS_RESET);
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    fs::write(&path, b"schema = 1\n")?;
    fs::File::open(&path)?.sync_all()?;
    if let Some(directory) = path.parent() {
        fs::File::open(directory)?.sync_all()?;
    }
    Ok(())
}

fn reset_done(workspace: &Path) -> Result<()> {
    match fs::remove_file(workspace.join(CMOS_RESET)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Check `machine` as the stopped machine's hardware tables and, with
/// `apply`, keep them from its next Start. Tables that lower to the image's
/// machine drop the overlay. A change of a field that SETUP records rewrites
/// that field and the checksum in the machine's CMOS, as Settings on an image
/// does.
pub fn change_settings(
    store: &Store,
    id: &str,
    machine: Table,
    apply: bool,
) -> Result<SettingsChange> {
    settings_change(store, id, Some(machine), apply, |_| Ok(()))
}

/// [`change_settings`] with a hook after each step, which may stop it there
/// as a crash would; fault-injection tests use it.
#[doc(hidden)]
pub fn change_settings_with_hook(
    store: &Store,
    id: &str,
    machine: Table,
    apply: bool,
    hook: impl FnMut(SettingsStep) -> Result<()>,
) -> Result<SettingsChange> {
    settings_change(store, id, Some(machine), apply, hook)
}

/// Return a stopped machine to its image's settings: the overlay goes and the
/// CMOS fields it changed are recomputed back.
pub fn clear_settings(store: &Store, id: &str) -> Result<SettingsChange> {
    settings_change(store, id, None, true, |_| Ok(()))
}

fn settings_change(
    store: &Store,
    id: &str,
    machine: Option<Table>,
    apply: bool,
    mut hook: impl FnMut(SettingsStep) -> Result<()>,
) -> Result<SettingsChange> {
    let (owner, record) = owner(store, id)?;
    require_settled(&owner)?;
    let image = pinned(&owner, &record)?;
    let source = revision::Source::open(store, &image)?;
    let layers = &source.manifest.layers;
    let packages = revision::Packages::new(store);
    let current = record
        .settings
        .clone()
        .unwrap_or_else(|| source.config.machine.clone());
    let clear = machine.is_none();
    let mut wanted = machine.unwrap_or_else(|| source.config.machine.clone());
    // Tables from a sheet carry no bindings: the ones the machine recorded
    // still bind its chosen ROMs, also after their package lost its tag.
    if !wanted.contains_key(revision::BOUND_FIRMWARE) {
        if let Some(recorded) = current.get(revision::BOUND_FIRMWARE) {
            wanted.insert(revision::BOUND_FIRMWARE.into(), recorded.clone());
        }
    }
    // The machine's current tables may no longer bind (a ROM no record or
    // package holds); clearing them and going back to the image's firmware
    // must still work, so the image's machine stands in, with an unknown
    // CMOS layout.
    let before = revision::lower_revision(&packages, &source.config, layers, current)
        .ok()
        .map(|lowered| lowered.machine);
    let revised = revision::lower_revision(&packages, &source.config, layers, wanted)?;
    let (after, image_machine) = (&revised.machine, &revised.source);
    let settings =
        match (!clear && revised.config.machine != source.config.machine && after != image_machine)
            .then(|| revised.config.machine.clone())
        {
            Some(mut table) => {
                revision::record_bound(&mut table, &revised.bound)?;
                Some(table)
            }
            None => None,
        };
    // The CMOS was written by the system BIOS the machine runs now; one of
    // another layout cannot read it, so the machine starts from the bytes
    // derived from its configuration, as a new machine does. A reset an
    // interrupted change left pending is finished now, too.
    let pending = reset_pending(owner.path());
    let layout = revision::cmos_layout(after);
    let reset = pending
        || before
            .as_ref()
            .is_none_or(|before| revision::cmos_layout_changes(before, after));
    let before = before.as_ref().unwrap_or(image_machine);
    let cmos_reset = reset && source.cmos.is_some();
    let kept = |error: crate::Error| {
        Some(format!(
            "The stored CMOS bytes are kept as they are: SETUP values cannot be derived for this machine ({error})."
        ))
    };
    let mut cmos_note = None;
    let mut journal_bytes = None;
    let cmos_changes = match (&source.config.nvram, &source.cmos) {
        (Some(nvram), Some(base)) if matches!(nvram.mode, NvramMode::Journal) => {
            let current = current_cmos(owner.path(), base)?.ok_or_else(|| {
                invalid("the CMOS journal needs recovery; start the machine once")
            })?;
            let recomputed = if reset {
                revision::reset_cmos(&current, after)
            } else {
                revision::recompute_cmos(&current, before, after)
            };
            match recomputed {
                Ok((bytes, changes)) => {
                    if !changes.is_empty() {
                        journal_bytes = Some((*base, bytes));
                    }
                    changes
                }
                Err(error) => {
                    cmos_note = kept(error);
                    Vec::new()
                }
            }
        }
        // A stored CMOS image is copied afresh at every Start: Start resets
        // it whenever the machine's BIOS keeps another layout than the
        // image's, which these are the bytes of.
        (Some(_), Some(base)) => match if revision::cmos_layout_changes(image_machine, after) {
            revision::reset_cmos(base, after)
        } else {
            revision::recompute_cmos(base, image_machine, after)
        } {
            Ok((_, changes)) => changes,
            Err(error) => {
                cmos_note = kept(error);
                Vec::new()
            }
        },
        _ => {
            let derive = |m: &assembly::MachineConfig| {
                assembly::derive_cmos_config(m).map_err(|e| invalid(e.to_string()))
            };
            let (old, new) = (derive(before)?, derive(after)?);
            (0..64)
                .filter(|&i| old[i] != new[i])
                .map(|offset| revision::CmosChange {
                    offset,
                    old: old[offset],
                    new: new[offset],
                    field: "derived at Start",
                })
                .collect()
        }
    };
    let diff = match &settings {
        Some(table) => revision::line_diff(&pretty(&source.config.machine)?, &pretty(table)?),
        None => Vec::new(),
    };
    if apply {
        // A reset of the journal is announced before the record changes: if
        // the process dies between the two, Start finishes the reset instead
        // of booting the new BIOS over bytes of another layout.
        let journaled =
            matches!(&source.config.nvram, Some(n) if matches!(n.mode, NvramMode::Journal));
        if reset && journaled {
            announce_reset(owner.path())?;
        }
        // The record first: if the journal write then fails, the record goes
        // back, so CMOS is never recomputed for settings the machine lacks.
        let previous = record.settings.clone();
        edit(store, id, |m| {
            m.settings = settings;
            m.image = image.clone();
        })?;
        hook(SettingsStep::Recorded)?;
        if let Some((base, bytes)) = journal_bytes {
            if let Err(error) = write_cmos(owner.path(), &base, &bytes) {
                let _ = edit(store, id, |m| m.settings = previous);
                if !pending {
                    let _ = reset_done(owner.path());
                }
                return Err(error);
            }
        }
        reset_done(owner.path())?;
    }
    Ok(SettingsChange {
        diff,
        cmos_changes,
        cmos_note,
        cmos_reset,
        cmos_layout: layout,
    })
}

/// Finish a CMOS reset an interrupted settings change left pending: the
/// machine's CMOS journal takes the bytes derived from the configuration the
/// session runs, before the session opens it.
fn finish_cmos_reset(owner: &Workspace<'_>, session: &workspace::PreparedSession) -> Result<()> {
    if !reset_pending(owner.path()) {
        return Ok(());
    }
    let base = session.machine.devices.iter().find_map(|d| match &d.kind {
        assembly::DeviceKind::Rtc(rtc) => match &rtc.nvram {
            assembly::NvramSource::Journal { image, .. } => Some(image.clone()),
            _ => None,
        },
        _ => None,
    });
    if let Some(base) = base {
        let base = fs::read(base)?;
        let current = current_cmos(owner.path(), &base)?
            .ok_or_else(|| invalid("the CMOS journal needs recovery before its pending reset"))?;
        let (bytes, changes) = revision::reset_cmos(&current, &session.machine)?;
        if !changes.is_empty() {
            write_cmos(owner.path(), &base, &bytes)?;
        }
    }
    reset_done(owner.path())
}

/// Write the CMOS image `bytes` into the machine's CMOS journal over `base`.
fn write_cmos(workspace: &Path, base: &[u8], bytes: &[u8; 64]) -> Result<()> {
    let directory = workspace.join("media");
    fs::create_dir_all(&directory)?;
    let mut journal = media::JournalMedia::open(
        &directory.join("cmos.upfj"),
        base,
        64,
        media::SinkStatus::default(),
    )
    .map_err(|e| invalid(format!("CMOS journal: {e}")))?;
    let mut sector = journal.read_sector(0);
    sector[..64].copy_from_slice(bytes);
    journal.write_batch(&[(0, sector)]);
    journal
        .finish()
        .map_err(|e| invalid(format!("CMOS journal: {}", e.message)))
}

/// Save as new image…: publish the stopped machine's changes and settings as
/// a new image under `tag` (a version tag never moves; another existing tag
/// moves only from `expected`). The title defaults to the machine name. By
/// default the machine then moves onto the new image with fresh, empty
/// journals and no settings overlay; with `keep_on_old_image` it stays on its
/// image with its changes, and the new image stands alone.
pub fn commit(
    store: &Store,
    id: &str,
    tag: &str,
    expected: Option<&Digest>,
    title: Option<&str>,
    keep_on_old_image: bool,
) -> Result<Descriptor> {
    commit_with_hook(store, id, tag, expected, title, keep_on_old_image, |_| {
        Ok(())
    })
}

/// [`commit`] with a hook after each publication step, which may fail it
/// there; fault-injection tests use it to interrupt a save.
#[doc(hidden)]
pub fn commit_with_hook(
    store: &Store,
    id: &str,
    tag: &str,
    expected: Option<&Digest>,
    title: Option<&str>,
    keep_on_old_image: bool,
    hook: impl FnMut(workspace::PublicationStep) -> Result<()>,
) -> Result<Descriptor> {
    let (owner, machine) = owner(store, id)?;
    let image = pinned(&owner, &machine)?;
    workspace::pin(&owner, &image)?;
    // A CMOS reset an interrupted settings change left pending is finished
    // first, so the new image never takes bytes of the old layout.
    if reset_pending(owner.path()) {
        let session = workspace::prepare_machine(&owner, &image, machine.settings.as_ref())?;
        finish_cmos_reset(&owner, &session)?;
    }
    let published = workspace::publish_machine(
        &owner,
        tag,
        expected,
        false,
        Some(title.unwrap_or(&machine.name)),
        machine.settings.as_ref(),
        keep_on_old_image,
        hook,
    )?;
    if !keep_on_old_image {
        edit(store, id, |m| {
            m.image = published.clone();
            m.source = tag.into();
            m.settings = None;
        })?;
    }
    Ok(published)
}

/// The target of an unfinished Save as new image…, as its durable intent
/// records it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct PendingCommit {
    pub tag: String,
    pub expected: Option<Digest>,
    /// The machine stays on its old image with its changes.
    #[serde(default)]
    pub keep: bool,
}

/// The unfinished Save as new image… of a machine, if one is pending.
pub fn pending_commit(store: &Store, id: &str) -> Result<Option<PendingCommit>> {
    let path = store.workspace_path(id)?.join("transaction.toml");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if bytes.len() as u64 > MAX_SMALL_BLOB {
        return Err(invalid("publication intent exceeds the size limit"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?;
    Ok(Some(
        toml::from_str(text).map_err(|e| invalid(e.to_string()))?,
    ))
}

/// Finish an interrupted Save as new image… with the target and the choice
/// (move onto the new image, or keep the changes on the old one) its intent
/// recorded, so every change it was saving is kept.
pub fn finish_commit(store: &Store, id: &str) -> Result<Descriptor> {
    let pending = pending_commit(store, id)?
        .ok_or_else(|| invalid(format!("machine {id} has no unfinished save")))?;
    commit(
        store,
        id,
        &pending.tag,
        pending.expected.as_ref(),
        None,
        pending.keep,
    )
}

/// What a Gaming-mode copy of a machine runs with, recorded in the copy when
/// it is made: keeping it, publishing it or starting it again uses exactly
/// this, even after the machine changes or goes.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyOf {
    pub schema: u32,
    /// The machine the copy was made from.
    pub machine: String,
    pub name: String,
    /// The tag the machine was made from.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Table>,
    #[serde(default)]
    pub pending: Vec<PendingInsert>,
}

const COPY_OF: &str = "copy-of.toml";

/// The record of the machine a Gaming-mode copy was made from, if it is one.
pub fn copy_of(copy: &Workspace<'_>) -> Result<Option<CopyOf>> {
    let Some(bytes) = copy.read(COPY_OF)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?;
    let record: CopyOf = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    if record.schema != 1 {
        return Err(invalid("unsupported copy record schema"));
    }
    Ok(Some(record))
}

/// Prepare a disposable copy of a stopped machine, as Gaming mode plays it:
/// `copy`, a fresh workspace the caller owns and later keeps or discards,
/// takes the machine's session pin and journals and a record of its name,
/// tag, labels, settings overlay and pending inserts ([`CopyOf`]); the
/// session runs with that overlay and those inserts. The machine itself does
/// not change, so nothing of the copy is ever confirmed.
pub fn prepare_copy(source: &Workspace<'_>, copy: &Workspace<'_>) -> Result<Prepared> {
    let store = source.store();
    let machine = record(store, source.name())?;
    require_settled(source)?;
    let image = pinned(source, &machine)?;
    copy_workspace(source.path(), copy.path())?;
    workspace::pin(copy, &image)?;
    let record = CopyOf {
        schema: 1,
        machine: source.name().to_owned(),
        name: machine.name.clone(),
        source: machine.source.clone(),
        labels: machine.labels.clone(),
        settings: machine.settings.clone(),
        pending: machine.pending.clone(),
    };
    copy.write(
        COPY_OF,
        toml::to_string(&record)
            .map_err(|e| invalid(e.to_string()))?
            .as_bytes(),
    )?;
    resume_copy(copy)
}

/// Prepare a Gaming-mode copy again, as a power cycle restarts it: with the
/// settings overlay and pending inserts recorded when it was made, over its
/// own journals.
pub fn resume_copy(copy: &Workspace<'_>) -> Result<Prepared> {
    let record = copy_of(copy)?.ok_or_else(|| invalid("not a copy of a machine"))?;
    let image = workspace::pinned_descriptor(copy)?
        .map(|image| identity(&image))
        .ok_or_else(|| invalid("the copy was never started"))?;
    let mut session = workspace::prepare_machine(copy, &image, record.settings.as_ref())?;
    finish_cmos_reset(copy, &session)?;
    for insert in &record.pending {
        insert_medium(copy.store(), copy, &mut session, insert)?;
    }
    Ok(Prepared {
        session,
        inserted: Vec::new(),
    })
}

/// Keep a disposable Gaming-mode workspace as a machine named `name`. The
/// workspace is renamed to the new machine's id; nothing is copied. A copy of
/// a machine ([`copy_of`]) keeps the tag, settings overlay and labels it ran
/// with; another copy takes a tag that names its image.
pub fn keep_copy(
    store: &Store,
    workspace_name: &str,
    name: &str,
    created: Option<String>,
) -> Result<String> {
    let name = validate_name(name)?;
    let mut owner = store.workspace(workspace_name)?;
    require_settled(&owner)?;
    let image = workspace::pinned_descriptor(&owner)?
        .map(|image| identity(&image))
        .ok_or_else(|| invalid("the copy was never started"))?;
    let template = copy_of(&owner)?;
    let source = match &template {
        Some(record) => record.source.clone(),
        None => all_tags(store)?
            .into_iter()
            .find(|tag| tag.manifest.digest == image.digest)
            .map(|tag| tag.name)
            .unwrap_or_default(),
    };
    let id = store.edit_machines(|machines| {
        if machines.find_name(&name).is_some() {
            return Err(clash(machines, &name));
        }
        let id = store.free_machine_id(machines, &name)?;
        let origin = if template.is_some() {
            Origin::Duplicate
        } else {
            Origin::New
        };
        let mut machine = Machine::new(&name, image.clone(), &source, origin);
        machine.created = created;
        if let Some(template) = &template {
            machine.settings = template.settings.clone();
            machine.labels = template.labels.clone();
        }
        machines.insert(&id, machine)?;
        Ok(id)
    })?;
    if let Err(error) = owner.rename(&id) {
        let _ = store.edit_machines(|machines| Ok(machines.remove(&id)));
        return Err(error.into());
    }
    // The machine exists now; leftovers of the copy are only tidied, so a
    // failure here never leaves it looking like an unfinished session.
    let _ = owner.remove("desktop.toml");
    let _ = owner.remove(COPY_OF);
    Ok(id)
}

/// Publish a disposable Gaming-mode workspace as a new image, as Keep as new
/// image… does. A copy of a machine ran with that machine's settings overlay
/// as recorded in the copy, so the new image's configuration takes it too.
pub fn publish_copy(
    copy: &Workspace<'_>,
    tag: &str,
    expected: Option<&Digest>,
    flatten: bool,
    title: Option<&str>,
) -> Result<Descriptor> {
    let settings = copy_of(copy)?.and_then(|record| record.settings);
    workspace::publish_machine(
        copy,
        tag,
        expected,
        flatten,
        title,
        settings.as_ref(),
        false,
        |_| Ok(()),
    )
}

/// A prepared launch of a machine and the pending inserts it puts in.
pub struct Prepared {
    pub session: workspace::PreparedSession,
    pub inserted: Vec<PendingInsert>,
}

/// The image a machine runs now, read without its lock.
pub fn pinned_image(store: &Store, id: &str) -> Result<Descriptor> {
    let machine = record(store, id)?;
    Ok(workspace::pinned_descriptor_at(store, id)?
        .map(|image| identity(&image))
        .unwrap_or(machine.image))
}

/// Prepare a machine to run in `owner`, from [`own`], which the caller keeps
/// for the whole run and journal finish. The session opens the machine's
/// journals on its pinned image with its settings overlay and puts each
/// pending insert into its drive. Nothing in the record changes until the
/// caller [`confirm`]s the launch, once the machine has assembled.
pub fn prepare(owner: &Workspace<'_>) -> Result<Prepared> {
    let store = owner.store();
    let id = owner.name();
    let machine = record(store, id)?;
    if let Some(from) = &machine.migrated_from {
        if holds_data(&store.workspace_path(from)?)? {
            return Err(invalid(format!(
                "machine {id} is still being moved from workspace {from}; open it with machine::own"
            )));
        }
    }
    let image = pinned(owner, &machine)?;
    let mut session = workspace::prepare_machine(owner, &image, machine.settings.as_ref())?;
    finish_cmos_reset(owner, &session)?;
    for insert in &machine.pending {
        insert_medium(store, owner, &mut session, insert)?;
    }
    Ok(Prepared {
        session,
        inserted: machine.pending,
    })
}

/// Confirm a launch once its machine has assembled: the record takes
/// `last_run` (host text, like the library's) and the image the session pins,
/// and drops the pending inserts the launch put in.
pub fn confirm(
    owner: &Workspace<'_>,
    inserted: &[PendingInsert],
    last_run: Option<String>,
) -> Result<()> {
    let store = owner.store();
    let id = owner.name();
    let image = pinned(owner, &record(store, id)?)?;
    edit(store, id, |m| {
        m.image = image;
        if last_run.is_some() {
            m.last_run = last_run;
        }
        m.pending.retain(|p| !inserted.contains(p));
    })
}

/// Put a pending insert into its floppy drive for this launch: the chosen
/// medium first, so the drive starts with it, then the set's other media when
/// all join the catalogue. Journals use the names of a live insert from the
/// library, so the same disk keeps one journal in this machine.
fn insert_medium(
    store: &Store,
    owner: &Workspace<'_>,
    session: &mut workspace::PreparedSession,
    insert: &PendingInsert,
) -> Result<()> {
    let unit = floppy_unit(&insert.drive)?;
    let slot_index = session
        .machine
        .drives
        .iter()
        .position(|d| d.unit == unit && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
        .ok_or_else(|| invalid(format!("{} is not a floppy drive here", insert.drive)))?;
    let assembly::DriveKind::Floppy { geometry } = session.machine.drives[slot_index].kind else {
        return Err(invalid("floppy drive expected"));
    };
    let resolved = store.resolve_descriptor(
        &insert.set,
        &image_store::media_set::Validator(&crate::media_set::PcMediaFormats),
    )?;
    let config = image_store::media_set::Config::decode(&resolved.config)?;
    if !crate::is_pc(&config.platform) {
        return Err(invalid("installation media for another platform"));
    }
    let chosen = config
        .media
        .iter()
        .position(|m| m.id == insert.medium)
        .ok_or_else(|| invalid(format!("no medium {} in the set", insert.medium)))?;
    let directory = owner.path().join("media");
    let mut entries = Vec::new();
    for (index, medium) in config.media.iter().enumerate() {
        if !insert.all && index != chosen {
            continue;
        }
        let format = crate::media_set::geometry(&medium.format)?
            .ok_or_else(|| invalid("an optical medium needs a CD-ROM drive"))?;
        if !geometry.accepted_media().contains(&format) {
            return Err(invalid(format!(
                "{}: {} does not take {}",
                insert.drive,
                assembly::descriptors::floppy_drive_media(geometry),
                medium.label
            )));
        }
        let path = store.resolved_file_path(&resolved, &medium.blob)?;
        let base = crate::media_set::verified_base(&resolved, &medium.blob)?;
        entries.push((index != chosen, medium, format, path, base));
    }
    entries.sort_by_key(|(later, ..)| *later);
    session.update_launch(|input| {
        let mut added = Vec::new();
        for (_, medium, format, path, base) in &entries {
            let id = next_insert_id(&input.document.config);
            let source = format!("assets/{id}.img");
            let digest = medium.blob.to_config()?;
            let hash = base.identity().base_digest;
            if !input.document.content.iter().any(|r| r.digest == digest) {
                input.document.content.push(pc_config::launch::ContentRow {
                    digest,
                    sha256: hash,
                    size: base.byte_len(),
                });
            }
            input.memory.remove(Path::new(&source));
            input.sources.insert(source.clone().into(), path.clone());
            if insert.journal {
                let name = format!("kept-{id}.upfj");
                input.records.insert(
                    name.clone(),
                    directory.join(format!("{}-{}.upfj", insert.set.digest.hex(), medium.id)),
                );
                input.document.records.push(pc_config::launch::RecordRow {
                    medium: id.clone(),
                    name,
                    keep_generations: assembly::WriteMode::DEFAULT_KEEP_GENERATIONS,
                });
            }
            input.document.config.media.push(pc_config::image::Medium {
                id: id.clone(),
                source,
                head: digest,
                name: Some(medium.label.clone()),
                policy: if insert.journal {
                    Policy::Journal
                } else {
                    Policy::Readonly
                },
                geometry: pc_config::image::Geometry::of(assembly::DriveKind::Floppy {
                    geometry: *format,
                }),
            });
            added.push(id);
        }
        let old_slot = input.document.config.slots[slot_index].clone();
        let mut catalogue = added.clone();
        for id in &old_slot.catalogue {
            let old = input
                .document
                .config
                .media
                .iter()
                .find(|m| &m.id == id)
                .ok_or_else(|| invalid("missing medium"))?
                .clone();
            let duplicate = old_slot.catalogue_mode
                && added.iter().any(|id| {
                    let new = input.document.config.media.iter().find(|m| &m.id == id);
                    new.is_some_and(|m| {
                        m.name == old.name
                            && input
                                .document
                                .content
                                .iter()
                                .find(|r| r.digest == m.head)
                                .map(|r| r.sha256)
                                == input
                                    .document
                                    .content
                                    .iter()
                                    .find(|r| r.digest == old.head)
                                    .map(|r| r.sha256)
                    })
                });
            if duplicate {
                continue;
            }
            let mut id = id.clone();
            if !old_slot.catalogue_mode {
                // A shared read-only image needs its own binding when this
                // slot becomes a catalogue and gives the image a name.
                let shared = input
                    .document
                    .config
                    .slots
                    .iter()
                    .enumerate()
                    .any(|(i, s)| i != slot_index && s.catalogue.contains(&id));
                if shared {
                    let mut copy = old.clone();
                    id = next_insert_id(&input.document.config);
                    copy.id = id.clone();
                    copy.source = format!("assets/{id}.img");
                    let source = input
                        .sources
                        .get(Path::new(&old.source))
                        .ok_or_else(|| invalid("missing launch base path"))?
                        .clone();
                    input.memory.remove(Path::new(&copy.source));
                    input.sources.insert(copy.source.clone().into(), source);
                    if let Some(bytes) = input.memory.get(Path::new(&old.source)).cloned() {
                        input.memory.insert(copy.source.clone().into(), bytes);
                    }
                    input.document.config.media.push(copy);
                }
                let medium = input
                    .document
                    .config
                    .media
                    .iter_mut()
                    .find(|m| m.id == id)
                    .ok_or_else(|| invalid("missing medium"))?;
                medium.name = Some(format!("{} image", insert.drive));
            }
            catalogue.push(id);
        }
        let slot = &mut input.document.config.slots[slot_index];
        slot.inserted = catalogue.first().cloned();
        slot.catalogue = catalogue;
        slot.catalogue_mode = true;
        input.prune();
        Ok(())
    })?;
    for (_, _, _, path, base) in entries {
        session.bases.insert(path, base);
    }
    Ok(())
}

fn next_insert_id(config: &Config) -> String {
    let mut index = 0u64;
    loop {
        let id = format!("insert-{index}");
        let source = format!("assets/{id}.img");
        if !config
            .media
            .iter()
            .any(|m| m.id == id || m.source == source)
            && !config.firmware.iter().any(|f| f.source == source)
            && !config.nvram.as_ref().is_some_and(|n| n.source == source)
        {
            return id;
        }
        index += 1;
    }
}
