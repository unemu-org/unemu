// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline whole-machine publication and isolated sessions; machine-images §§3–5.
//! Independent journal finishes are recovery points, not a live machine snapshot.

use crate::*;
use image_store::{media_type as mt, workspace::Workspace, ConfigValidator, Manifest, Store};
use media::{
    layer::{seal, SectorReader},
    SectorSource, VerifiedBase,
};
use std::{fs, io};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    schema: u32,
    image: Descriptor,
    machine: Digest,
    #[serde(default)]
    retention: std::collections::BTreeMap<String, u32>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    schema: u32,
    old_image: Descriptor,
    proposed: Descriptor,
    tag: String,
    expected: Option<Digest>,
    /// The workspace stays on its old image with its journals.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    keep: bool,
}
/// A fault hook runs after each externally meaningful publication operation.
/// Returning an error leaves the workspace and any durable intent available.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicationStep {
    Begin,
    JournalRead(String),
    PackStaged(String),
    BlobStored(Digest),
    IntentStored,
    RetentionStored,
    TagUpdated,
    MediaRetired,
    SessionUpdated,
    JournalsRetired,
    StagingRetired,
    Finished,
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    toml::from_str(std::str::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?)
        .map_err(|e| invalid(e.to_string()))
}
fn write<T: Serialize>(workspace: &Workspace<'_>, name: &str, value: &T) -> Result<()> {
    workspace.write(
        name,
        toml::to_string(value)
            .map_err(|e| invalid(e.to_string()))?
            .as_bytes(),
    )?;
    Ok(())
}
fn session(workspace: &Workspace<'_>) -> Result<Session> {
    let session: Session = parse(
        &workspace
            .read("session.toml")?
            .ok_or_else(|| invalid("workspace does not exist"))?,
    )?;
    if session.schema != 1 {
        return Err(invalid("unsupported workspace schema"));
    }
    Ok(session)
}
pub fn pinned_image(workspace: &Workspace<'_>) -> Result<Digest> {
    Ok(session(workspace)?.image.digest)
}
/// A start's launch document and the host paths behind its logical keys.
/// Small content travels with it so an edit never reopens a store blob.
/// See `backlog/docs/design/core-contract.md` §14.3.
#[derive(Clone, Debug)]
pub struct PreparedLaunch {
    pub(crate) document: pc_config::launch::Launch,
    pub(crate) small: std::collections::BTreeMap<[u8; 32], Vec<u8>>,
    pub(crate) sources: std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    pub(crate) memory: std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    pub(crate) records: std::collections::BTreeMap<String, std::path::PathBuf>,
}
impl PreparedLaunch {
    /// The document this start derives its machine from.
    pub fn document(&self) -> &pc_config::launch::Launch {
        &self.document
    }

    /// The existing journal file each logical record stands for, including
    /// journals beside images outside the workspace.
    pub fn record_files(&self) -> &std::collections::BTreeMap<String, std::path::PathBuf> {
        &self.records
    }

    pub(crate) fn prune(&mut self) {
        let document = &mut self.document;
        document.config.media.retain(|m| {
            document
                .config
                .slots
                .iter()
                .any(|s| s.catalogue.contains(&m.id))
        });
        document.records.retain(|r| {
            document
                .config
                .media
                .iter()
                .any(|m| m.id == r.medium && m.policy == Policy::Journal)
                || (r.medium == pc_config::launch::CMOS_MEDIUM
                    && document
                        .config
                        .nvram
                        .as_ref()
                        .is_some_and(|n| matches!(n.mode, NvramMode::Journal)))
        });
        document.content.retain(|r| {
            document.config.firmware.iter().any(|f| f.blob == r.digest)
                || document.config.media.iter().any(|m| m.head == r.digest)
                || document
                    .config
                    .nvram
                    .as_ref()
                    .is_some_and(|n| n.head == r.digest)
        });
        self.sources.retain(|source, _| {
            document
                .config
                .media
                .iter()
                .any(|m| source == std::path::Path::new(&m.source))
                || document
                    .config
                    .firmware
                    .iter()
                    .any(|f| source == std::path::Path::new(&f.source))
                || document
                    .config
                    .nvram
                    .as_ref()
                    .is_some_and(|n| source == std::path::Path::new(&n.source))
        });
        self.memory
            .retain(|source, _| self.sources.contains_key(source));
        self.records
            .retain(|name, _| document.records.iter().any(|r| &r.name == name));
        self.small
            .retain(|hash, _| document.content.iter().any(|r| &r.sha256 == hash));
    }
}
impl unemu_core_api::ContentSource for PreparedLaunch {
    fn open(
        &self,
        sha256: &[u8; 32],
        size: u64,
    ) -> std::result::Result<unemu_core_api::Content, unemu_core_api::Error> {
        let bytes = self
            .small
            .get(sha256)
            .filter(|b| b.len() as u64 == size)
            .ok_or_else(|| {
                unemu_core_api::Error::new(
                    unemu_core_api::ErrorKind::Content,
                    "unbound small content",
                )
            })?;
        Ok(unemu_core_api::Content::Bytes(bytes.clone()))
    }
}

/// Editable launch input with verified bases and host bindings for PC tools.
pub struct PreparedSession {
    /// The document and its path bindings for a stored-image start. Synthetic
    /// callers that supply an assembled config directly have no document.
    pub launch: Option<PreparedLaunch>,
    /// Host-path view used by catalogue editors, pending CMOS resets and
    /// desktop media tools. The core mounts the launch document's logical keys.
    pub machine: assembly::MachineConfig,
    pub bases: std::collections::BTreeMap<std::path::PathBuf, VerifiedBase>,
    pub image: Digest,
    /// Zero-rooted bases among `bases`, kept as parts for a thread transfer.
    pub zero: std::collections::BTreeMap<std::path::PathBuf, std::rc::Rc<crate::zero::ZeroChain>>,
}
pub fn prepare_start(workspace: &Workspace<'_>, tag: &str, fresh: bool) -> Result<PreparedSession> {
    prepare_selected(workspace, tag, fresh, None, None)
}

/// Start the exact descriptor selected by a desktop, even if its tag has moved.
pub fn prepare_pinned(
    workspace: &Workspace<'_>,
    image: &Descriptor,
    fresh: bool,
) -> Result<PreparedSession> {
    prepare_selected(workspace, "", fresh, Some(image), None)
}

/// Start a machine's workspace on its pinned image, with the machine's
/// settings overlay (hardware tables replacing the image's) when it has one.
pub(crate) fn prepare_machine(
    workspace: &Workspace<'_>,
    image: &Descriptor,
    settings: Option<&toml::Table>,
) -> Result<PreparedSession> {
    prepare_selected(workspace, "", false, Some(image), settings)
}

/// Pin a new workspace to `image` without preparing a launch: the session
/// names the image and its machine identity, and no media exist yet. An
/// existing session is left as it is.
pub(crate) fn pin(workspace: &Workspace<'_>, image: &Descriptor) -> Result<()> {
    if workspace.read("session.toml")?.is_some() {
        return Ok(());
    }
    let store = workspace.store();
    let manifest = Manifest::decode_with_types(
        &image_store::BlobSource::read_verified(store, image, image_store::MAX_SMALL_BLOB)?,
        PAYLOAD_TYPES,
    )?;
    if manifest.artifact_type() != mt::MACHINE || !crate::is_pc(manifest.platform()?) {
        return Err(invalid("not a PC machine image"));
    }
    write(
        workspace,
        "session.toml",
        &Session {
            schema: 1,
            image: image.clone(),
            machine: manifest.config.digest,
            retention: retention_of(store, &image.digest)?,
        },
    )
}

/// The descriptor this workspace's session pins, read under ownership.
pub(crate) fn pinned_descriptor(workspace: &Workspace<'_>) -> Result<Option<Descriptor>> {
    if workspace.read("session.toml")?.is_none() {
        return Ok(None);
    }
    Ok(Some(session(workspace)?.image))
}

fn retention_of(store: &Store, image: &Digest) -> Result<std::collections::BTreeMap<String, u32>> {
    Ok(crate::ImportRecord::read(store, image)?
        .map(|r| {
            r.generations
                .into_iter()
                .filter_map(|g| g.keep_generations.map(|n| (g.medium, n)))
                .collect()
        })
        .unwrap_or_default())
}

/// Select a run's image under ownership, preserving preparation's refusal order.
/// CLI core selection must not decode a manifest before these workspace checks.
pub fn select_run_image(
    workspace: &Workspace<'_>,
    tag: &str,
    fresh: bool,
    selected: Option<&Descriptor>,
) -> Result<Descriptor> {
    Ok(select_session(workspace, tag, fresh, selected)?.0.image)
}

fn select_session(
    workspace: &Workspace<'_>,
    tag: &str,
    fresh: bool,
    selected: Option<&Descriptor>,
) -> Result<(Session, bool)> {
    if workspace.read("discard.toml")?.is_some() {
        return Err(invalid("discard pending; finish discarding this workspace"));
    }
    if workspace.read("transaction.toml")?.is_some() {
        return Err(invalid(
            "publication pending; retry or abandon publication before running",
        ));
    }
    let store = workspace.store();
    let existing = workspace.read("session.toml")?.is_some();
    if fresh && existing {
        return Err(invalid("--new requires a fresh workspace name"));
    }
    if !existing
        && workspace.path().join("media").exists()
        && fs::read_dir(workspace.path().join("media"))?
            .next()
            .is_some()
    {
        return Err(invalid(
            "orphaned media without a session; refusing to overwrite it",
        ));
    }
    let state = if existing {
        session(workspace)?
    } else {
        let image = match selected {
            Some(image) => image.clone(),
            None => store.tag_descriptor(tag)?,
        };
        let retention = retention_of(store, &image.digest)?;
        Session {
            schema: 1,
            image,
            machine: Digest::of(b""),
            retention,
        }
    };
    if existing {
        let requested = match selected {
            Some(image) => Ok(image.clone()),
            None => store.tag_descriptor(tag),
        };
        match requested {
            Ok(requested) if requested.digest != state.image.digest => {
                return Err(invalid(format!(
                    "workspace pins {}; requested tag points to {}",
                    state.image.digest, requested.digest
                )))
            }
            Ok(_) | Err(image_store::Error::TagNotFound(_)) => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok((state, existing))
}

fn prepare_selected(
    workspace: &Workspace<'_>,
    tag: &str,
    fresh: bool,
    selected: Option<&Descriptor>,
    settings: Option<&toml::Table>,
) -> Result<PreparedSession> {
    let (mut state, existing) = select_session(workspace, tag, fresh, selected)?;
    let store = workspace.store();
    let directory = workspace.path().join("media");
    let (image, machine, (bases, zero), launch) = crate::provider::resolve_pinned(
        store,
        &state.image,
        &directory,
        Some(&state.retention),
        settings,
    )?;
    if existing && state.machine != image.manifest.config.digest {
        return Err(invalid("workspace machine identity mismatch"));
    }
    if !existing {
        state.machine = image.manifest.config.digest;
        write(workspace, "session.toml", &state)?;
    }
    fs::create_dir_all(&directory)?;
    fs::File::open(workspace.path())?.sync_all()?;
    Ok(PreparedSession {
        launch: Some(launch),
        machine,
        bases,
        image: state.image.digest,
        zero,
    })
}
/// Cancel an unfinished publication without deleting its media or published tag.
pub fn abandon_publication(workspace: &Workspace<'_>) -> Result<()> {
    if workspace.read("discard.toml")?.is_some() {
        return Err(invalid("discard pending"));
    }
    let _locks = workspace.lock_contents()?;
    if let Some(bytes) = workspace.read("transaction.toml")? {
        let transaction: Transaction = parse(&bytes)?;
        if transaction.schema != 1 {
            return Err(invalid("unsupported transaction schema"));
        }
        let current = session(workspace)?;
        let retired = workspace.path().join("retired");
        let media = workspace.path().join("media");
        if current.image.digest == transaction.old_image.digest && retired.exists() {
            if media.exists() {
                if fs::read_dir(&media)?.next().is_some() {
                    return Err(invalid("cannot restore retired media over current media"));
                }
                fs::remove_dir(&media)?;
            }
            fs::rename(retired, media)?;
            fs::File::open(workspace.path())?.sync_all()?;
        }
        workspace.remove("transaction.toml")?;
    }
    let staging = workspace.path().join("publication");
    if staging.exists() {
        fs::remove_dir_all(staging)?;
        fs::File::open(workspace.path())?.sync_all()?;
    }
    Ok(())
}
/// The media folder's subfolder of disks the window created from host
/// files: their read-only images and the machine's record of which drive
/// takes them at its next Start.
const CREATED: &str = "created";
struct Staging(std::path::PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn stored(
    store: &Store,
    kind: &str,
    bytes: &[u8],
    hook: &mut impl FnMut(PublicationStep) -> Result<()>,
) -> Result<Descriptor> {
    let blob = store.import(bytes, None)?;
    hook(PublicationStep::BlobStored(blob.digest.clone()))?;
    Ok(classify(Descriptor::new(kind, blob.digest, blob.size)))
}
fn classify(mut descriptor: Descriptor) -> Descriptor {
    descriptor
        .annotations
        .insert("unemu.redistribution".into(), "user-supplied".into());
    descriptor
}
/// Publish stopped media, with a failure hook for storage-boundary acceptance tests.
/// A durable intent blocks execution until retry completes the workspace rebase.
pub fn publish_with_hook(
    workspace: &Workspace<'_>,
    tag: &str,
    expected: Option<&Digest>,
    hook: impl FnMut(PublicationStep) -> Result<()>,
) -> Result<Descriptor> {
    publish_with_options(workspace, tag, expected, false, hook)
}
/// Flatten all layered fixed disks when requested, using bounded streaming exports.
pub fn publish_with_options(
    workspace: &Workspace<'_>,
    tag: &str,
    expected: Option<&Digest>,
    flatten: bool,
    hook: impl FnMut(PublicationStep) -> Result<()>,
) -> Result<Descriptor> {
    publish_as(workspace, tag, expected, flatten, None, hook)
}
/// Save as new image…: publish under `tag`, keeping the source image's
/// descriptive manifest annotations (its title, the firmware packages it came
/// from) and recording the source as the new image's base. `title` replaces
/// the title; an empty one removes it, `None` keeps the source's. A version
/// tag never moves; any other existing tag moves only from `expected`.
pub fn publish_as(
    workspace: &Workspace<'_>,
    tag: &str,
    expected: Option<&Digest>,
    flatten: bool,
    title: Option<&str>,
    hook: impl FnMut(PublicationStep) -> Result<()>,
) -> Result<Descriptor> {
    publish_machine(workspace, tag, expected, flatten, title, None, false, hook)
}
/// Publish a machine's workspace: `settings`, the machine's overlay of
/// hardware tables, replaces the image's in the new configuration. With
/// `keep` the workspace stays on its old image with its journals; otherwise
/// it moves onto the new image with fresh journals, as `publish_as` does.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_machine(
    workspace: &Workspace<'_>,
    tag: &str,
    expected: Option<&Digest>,
    flatten: bool,
    title: Option<&str>,
    settings: Option<&toml::Table>,
    keep: bool,
    mut hook: impl FnMut(PublicationStep) -> Result<()>,
) -> Result<Descriptor> {
    if workspace.read("discard.toml")?.is_some() {
        return Err(invalid("discard pending"));
    }
    hook(PublicationStep::Begin)?;
    if let Some(bytes) = workspace.read("transaction.toml")? {
        let transaction: Transaction = parse(&bytes)?;
        if transaction.schema != 1
            || transaction.tag != tag
            || transaction.expected.as_ref() != expected
            || transaction.keep != keep
        {
            return Err(invalid(
                "pending publication has different target or schema",
            ));
        }
        return complete(
            workspace,
            transaction,
            &mut hook,
            retry_locks(workspace)?,
            true,
            None,
        );
    }
    let old = session(workspace)?;
    let store = workspace.store();
    store.check_tag_target(tag, expected, None)?;
    let resolved = crate::provider::load_pinned(store, &old.image, true, true)?;
    let image = resolved.image;
    let mut config = resolved.config;
    // The new image names the platform by its current id, also when the image
    // it derives from names the retired one.
    config.platform = crate::PLATFORM.into();
    let mut overlay_cmos = None;
    let mut descriptors = image.manifest.layers.clone();
    let packages = crate::revision::Packages::new(store);
    let mut bound = Vec::new();
    if let Some(machine) = settings {
        let overlay =
            crate::revision::overlay(&packages, &config, &image.manifest.layers, machine)?;
        // Firmware the machine's settings chose from a package of the store
        // joins the new image's closure.
        for d in overlay.firmware() {
            if !descriptors.iter().any(|known| known.digest == d.digest) {
                descriptors.push(d);
            }
        }
        config = overlay.config;
        overlay_cmos = overlay.cmos;
        bound = overlay.bound;
    }
    let bases = resolved.bases;
    if old.machine != image.manifest.config.digest {
        return Err(invalid("workspace machine identity mismatch"));
    }
    let scratch = workspace.path().join("publication");
    fs::create_dir_all(&scratch)?;
    let _staging = Staging(scratch.clone());
    let mut locks = Vec::new();
    // Retain all journal locks until the tag and workspace pointers are durable.
    // This also refuses a writer that bypassed the workspace API.
    for id in config
        .media
        .iter()
        .map(|m| m.id.as_str())
        .chain(config.nvram.iter().map(|_| "cmos"))
    {
        let path = workspace
            .path()
            .join("media")
            .join(format!("{id}.upfj.lock"));
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        lock.try_lock()
            .map_err(|e| invalid(format!("media writer still active: {e}")))?;
        locks.push(lock);
    }
    for m in &mut config.media {
        let disk = bases
            .get(&m.head)
            .ok_or_else(|| invalid("missing verified medium"))?;
        let head_digest = m.head.store()?;
        let old_head = descriptors
            .iter()
            .find(|d| d.names(&head_digest))
            .ok_or_else(|| invalid("missing medium head"))?;
        let flatten_disk = flatten
            && matches!(
                old_head.decoded_media_type(),
                mt::DISK_BASE | mt::DISK_LAYER | DISK_ZERO
            );
        // A zero-rooted chain flattens to one layer over its zero base, holding
        // only the sectors that differ from zero, never a full raw copy.
        let zero_root = disk.zero.as_ref().map(|chain| chain.root_source());
        let overlay = if m.policy == Policy::Journal {
            journal(workspace, &m.id, &disk.base)?
        } else {
            None
        };
        if overlay.is_some() {
            hook(PublicationStep::JournalRead(m.id.clone()))?;
        }
        let changed = overlay
            .as_ref()
            .map(|overlay| overlay.has_changes())
            .transpose()?
            .unwrap_or(false);
        if !changed && !flatten_disk {
            continue;
        }
        let final_disk: &dyn SectorSource = overlay
            .as_ref()
            .map_or(&disk.base as &dyn SectorSource, |overlay| overlay);
        let d = if old_head.decoded_media_type() == mt::FLOPPY
            || (flatten_disk && zero_root.is_none())
        {
            store.require_space(final_disk.byte_len())?;
            let kind = if old_head.decoded_media_type() == mt::FLOPPY {
                mt::FLOPPY
            } else {
                mt::DISK_BASE
            };
            let d = store.import_payload(
                SectorReader {
                    source: final_disk,
                    position: 0,
                },
                kind,
                final_disk.byte_len(),
                None,
                PAYLOAD_TYPES,
            )?;
            hook(PublicationStep::BlobStored(d.digest.clone()))?;
            classify(d)
        } else {
            let sealed_sectors = (overlay
                .as_ref()
                .map_or(0, |overlay| overlay.written_sectors())
                as u64)
                .saturating_add(disk.zero.as_ref().map_or(0, |chain| chain.stored_sectors()));
            store.require_space(
                sealed_sectors
                    .saturating_mul(media::layer::RECORD_LEN)
                    .saturating_mul(2),
            )?;
            let (parent, parent_digest): (&dyn SectorSource, String) = match &zero_root {
                Some(root) if flatten_disk => (root, disk.root.to_string()),
                _ => (&disk.base, m.head.to_string()),
            };
            let pack_path = scratch.join(format!("{}.pack", m.id));
            let mut file = fs::File::create(&pack_path)?;
            let layer = seal(
                parent,
                final_disk,
                crate::disk::geometry(&m.geometry),
                &parent_digest,
                &disk.root.to_string(),
                &mut file,
            )?;
            file.sync_all()?;
            if flatten_disk && layer.sector_count == 0 {
                // Every sector is zero again: the flattened head is the zero base.
                m.head = disk.root.to_config()?;
                continue;
            }
            hook(PublicationStep::PackStaged(m.id.clone()))?;
            let file = fs::File::open(&pack_path)?;
            let d = store.import_payload(
                &file,
                mt::DISK_SECTORS,
                file.metadata()?.len(),
                Some(&layer.sectors.parse()?),
                PAYLOAD_TYPES,
            )?;
            hook(PublicationStep::BlobStored(d.digest.clone()))?;
            descriptors.push(classify(d));
            stored(store, mt::DISK_LAYER, &layer.encode()?, &mut hook)?
        };
        m.head = d.decoded_digest()?.to_config()?;
        descriptors.push(d);
    }
    if let Some(n) = &mut config.nvram {
        let base = &bases
            .get(&n.head)
            .ok_or_else(|| invalid("missing verified CMOS"))?
            .base;
        if let Some(final_disk) = journal(workspace, "cmos", base)? {
            hook(PublicationStep::JournalRead("cmos".into()))?;
            let blob = store.import(
                SectorReader {
                    source: &final_disk,
                    position: 0,
                },
                None,
            )?;
            hook(PublicationStep::BlobStored(blob.digest.clone()))?;
            n.head = blob.digest.to_config()?;
            descriptors.push(classify(Descriptor::new(CMOS, blob.digest, blob.size)));
        } else if let Some(bytes) = overlay_cmos {
            // An unjournaled CMOS image gets the SETUP fields of the machine's
            // settings, as Settings on an image stores them.
            let blob = store.import(bytes.as_slice(), None)?;
            hook(PublicationStep::BlobStored(blob.digest.clone()))?;
            n.head = blob.digest.to_config()?;
            descriptors.push(classify(Descriptor::new(CMOS, blob.digest, blob.size)));
        }
    }
    let table = toml::Value::try_from(&config)
        .map_err(|e| invalid(e.to_string()))?
        .as_table()
        .cloned()
        .ok_or_else(|| invalid("config is not a table"))?;
    let refs = PcProvider.validate(&table, &descriptors, store)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut layers = Vec::new();
    for digest in refs {
        if seen.insert(digest.clone()) {
            layers.push(
                descriptors
                    .iter()
                    .find(|d| d.names(&digest))
                    .ok_or_else(|| invalid("missing closure object"))?
                    .clone(),
            );
        }
    }
    // Metadata is generated catalogue data and stays open; artwork keeps its mark.
    for d in image
        .manifest
        .layers
        .iter()
        .filter(|d| matches!(d.media_type.as_str(), mt::METADATA | mt::PNG | mt::JPEG))
    {
        layers.push(if d.media_type == mt::METADATA {
            crate::generated(image_store::platform::current_metadata(
                store,
                d,
                &image.manifest.layers,
            )?)
        } else {
            d.clone()
        });
    }
    // The configuration is this program's own file: open, never re-marked.
    let config_bytes = toml::to_string(&config).map_err(|e| invalid(e.to_string()))?;
    let configuration = crate::generated(stored(
        store,
        mt::CONFIG,
        config_bytes.as_bytes(),
        &mut hook,
    )?);
    let sources = crate::revision::firmware_sources(&packages, &image.manifest, &layers, &bound)?;
    let mut manifest = Manifest::new_with_types(
        crate::PLATFORM,
        configuration,
        layers,
        PcProvider.payload_types(),
    )?;
    if let Some(sources) = sources {
        manifest
            .annotations
            .insert(crate::revision::FIRMWARE_SOURCES.into(), sources);
    }
    let inherited =
        crate::compat::packages_floor(crate::compat::listed_packages(&manifest, packages.list()?))?;
    crate::compat::stamp(&mut manifest, config_bytes.as_bytes(), inherited, None)?;
    derive_annotations(&mut manifest, &image.manifest, &old.image.digest, title)?;
    image_store::compat::carry(&mut manifest, &image.manifest)?;
    let proposed = store.import_manifest_with_types(&manifest, PcProvider.payload_types())?;
    hook(PublicationStep::BlobStored(proposed.digest.clone()))?;
    let verified = store.verify_publication_reusing(&proposed, &PcProvider, &image)?;
    let transaction = Transaction {
        schema: 1,
        old_image: old.image,
        proposed,
        tag: tag.into(),
        expected: expected.cloned(),
        keep,
    };
    write(workspace, "transaction.toml", &transaction)?;
    hook(PublicationStep::IntentStored)?;
    complete(
        workspace,
        transaction,
        &mut hook,
        locks,
        false,
        Some(verified),
    )
}
/// The OCI annotation naming the image a new one was saved from.
pub const BASE_DIGEST: &str = "org.opencontainers.image.base.digest";
/// The OCI annotation holding an image's library title.
pub const TITLE: &str = "org.opencontainers.image.title";
/// OCI annotations that describe one particular build, not its copies.
const PER_MANIFEST: &[&str] = &[
    "org.opencontainers.image.created",
    "org.opencontainers.image.version",
    "org.opencontainers.image.revision",
    "org.opencontainers.image.base.name",
    "org.opencontainers.image.ref.name",
];
/// A saved image keeps what describes its source (title, firmware package
/// provenance and any other manifest annotation); the platform comes from the
/// new envelope. A save that changes nothing is the source image itself, so a
/// new tag becomes an alias of the same digest. Otherwise the annotations of
/// one particular build are dropped and the source is recorded as the base.
pub(crate) fn derive_annotations(
    manifest: &mut Manifest,
    source: &Manifest,
    source_digest: &Digest,
    title: Option<&str>,
) -> Result<()> {
    for (key, value) in &source.annotations {
        manifest
            .annotations
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    match title.map(str::trim) {
        Some("") => {
            manifest.annotations.remove(TITLE);
        }
        Some(title) => {
            manifest.annotations.insert(TITLE.into(), title.into());
        }
        None => {}
    }
    if Digest::of(&manifest.encode_with_types(PAYLOAD_TYPES)?) == *source_digest {
        return Ok(());
    }
    for key in PER_MANIFEST {
        manifest.annotations.remove(*key);
    }
    manifest
        .annotations
        .insert(BASE_DIGEST.into(), source_digest.to_string());
    Ok(())
}
/// The tag and expected digest of an unfinished publication of a workspace,
/// if one is pending. Publishing again with exactly these finishes it; Start
/// stays blocked until then. Read without the workspace lock, as a hint for
/// the desktop; the publication itself rechecks under ownership.
pub fn pending_publication_at(
    store: &Store,
    name: &str,
) -> Result<Option<(String, Option<Digest>)>> {
    let Some(bytes) = read_small(&store.workspace_path(name)?.join("transaction.toml"))? else {
        return Ok(None);
    };
    let transaction: Transaction = parse(&bytes)?;
    if transaction.schema != 1 {
        return Err(invalid("unsupported transaction schema"));
    }
    Ok(Some((transaction.tag, transaction.expected)))
}
/// A bounded workspace document, or `None` when it does not exist.
fn read_small(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(image_store::MAX_SMALL_BLOB + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > image_store::MAX_SMALL_BLOB {
        return Err(invalid("workspace document exceeds size limit"));
    }
    Ok(Some(bytes))
}
fn journal(
    workspace: &Workspace<'_>,
    id: &str,
    base: &VerifiedBase,
) -> Result<Option<media::SparseOverlay>> {
    let path = workspace.path().join("media").join(format!("{id}.upfj"));
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let scan = media::scan_overlay_reader(&mut file, base, base.byte_len())?;
    if scan.report.needs_recovery() || !matches!(scan.report.status, media::Status::Valid) {
        return Err(invalid(format!(
            "{id}: journal requires recovery before publication"
        )));
    }
    Ok(Some(scan.overlay.ok_or_else(|| {
        invalid("journal reconstruction refused")
    })?))
}
fn retry_locks(workspace: &Workspace<'_>) -> Result<Vec<fs::File>> {
    let mut locks = Vec::new();
    for name in ["media", "retired"] {
        let directory = workspace.path().join(name);
        if !directory.exists() {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "lock") {
                let lock = fs::OpenOptions::new().read(true).write(true).open(path)?;
                lock.try_lock()
                    .map_err(|e| invalid(format!("media writer still active: {e}")))?;
                locks.push(lock);
            }
        }
    }
    Ok(locks)
}
fn complete(
    workspace: &Workspace<'_>,
    transaction: Transaction,
    hook: &mut impl FnMut(PublicationStep) -> Result<()>,
    _locks: Vec<fs::File>,
    retry: bool,
    verified: Option<image_store::ResolvedImage>,
) -> Result<Descriptor> {
    let store = workspace.store();
    let current = session(workspace)?;
    if current.image.digest != transaction.old_image.digest
        && current.image.digest != transaction.proposed.digest
    {
        return Err(invalid("workspace differs from publication intent"));
    }
    let image = match verified {
        Some(image) => image,
        None => store.verify_publication(&transaction.proposed, &PcProvider)?,
    };
    save_retention(
        store,
        &transaction.proposed,
        &current.retention,
        &parse::<Config>(&image.config)?,
    )?;
    hook(PublicationStep::RetentionStored)?;

    // Only recovery may accept a target already at the proposed digest. A new
    // transaction must win its compare-and-swap even for identical bytes.
    let published = retry
        && match store.tag_descriptor(&transaction.tag) {
            Ok(image) => image.digest == transaction.proposed.digest,
            Err(image_store::Error::TagNotFound(_)) => false,
            Err(e) => return Err(e.into()),
        };
    if !published {
        if let Err(error) =
            store.update_verified_tag(&transaction.tag, &image, transaction.expected.as_ref())
        {
            if matches!(error, image_store::Error::Conflict { .. }) {
                workspace.remove("transaction.toml")?;
            }
            return Err(error.into());
        }
    }
    hook(PublicationStep::TagUpdated)?;
    if !transaction.keep {
        let media = workspace.path().join("media");
        let retired = workspace.path().join("retired");
        // Identical manifests still need fresh journals. Pending intents block runs,
        // so retry can also safely retire an already empty media directory.
        if media.exists() && !retired.exists() {
            fs::rename(&media, &retired)?;
            fs::File::open(workspace.path())?.sync_all()?;
        }
        hook(PublicationStep::MediaRetired)?;
        write(
            workspace,
            "session.toml",
            &Session {
                schema: 1,
                image: transaction.proposed.clone(),
                machine: image.manifest.config.digest,
                retention: current.retention,
            },
        )?;
        hook(PublicationStep::SessionUpdated)?;
        fs::create_dir_all(&media)?;
        // Disks the window made from host files are no journals: they are
        // read-only bases the machine owns, waiting for a drive, and stay
        // with the machine on its new image. Their journals go with the rest.
        let created = retired.join(CREATED);
        if created.is_dir() && !media.join(CREATED).exists() {
            fs::rename(&created, media.join(CREATED))?;
            fs::File::open(&media)?.sync_all()?;
        }
        if retired.exists() {
            fs::remove_dir_all(retired)?;
        }
        fs::File::open(workspace.path())?.sync_all()?;
        hook(PublicationStep::JournalsRetired)?;
    }
    let staging = workspace.path().join("publication");
    if staging.exists() {
        fs::remove_dir_all(staging)?;
        fs::File::open(workspace.path())?.sync_all()?;
    }
    hook(PublicationStep::StagingRetired)?;
    workspace.remove("transaction.toml")?;
    hook(PublicationStep::Finished)?;
    Ok(transaction.proposed)
}
pub fn publish(
    workspace: &Workspace<'_>,
    tag: &str,
    expected: Option<&Digest>,
) -> Result<Descriptor> {
    publish_with_hook(workspace, tag, expected, |_| Ok(()))
}

fn save_retention(
    store: &Store,
    image: &Descriptor,
    retention: &std::collections::BTreeMap<String, u32>,
    config: &Config,
) -> Result<()> {
    let mut generations = config
        .media
        .iter()
        .map(|m| crate::ChosenGeneration {
            medium: m.id.clone(),
            policy: m.policy,
            generation: None,
            recovered: false,
            keep_generations: retention.get(&m.id).copied(),
        })
        .collect::<Vec<_>>();
    if let Some(n) = &config.nvram {
        generations.push(crate::ChosenGeneration {
            medium: "cmos".into(),
            policy: if matches!(n.mode, NvramMode::Journal) {
                Policy::Journal
            } else {
                Policy::Volatile
            },
            generation: None,
            recovered: false,
            keep_generations: retention.get("cmos").copied(),
        });
    }
    let record = crate::ImportRecord {
        schema: 1,
        manifest: image.clone(),
        generations,
        warnings: Vec::new(),
    };
    store.write_local_record(
        &image.digest,
        toml::to_string(&record)
            .map_err(|e| invalid(e.to_string()))?
            .as_bytes(),
    )?;
    Ok(())
}

/// Verified content for the launch plans travels as descriptors instead of
/// the execution thread's reference-counted sector readers. The host-path
/// machine view also supports compatibility tests and desktop test inspection.
pub struct SessionTransfer {
    pub(crate) launch: Option<PreparedLaunch>,
    machine: assembly::MachineConfig,
    image: Digest,
    pub(crate) bases: std::collections::BTreeMap<std::path::PathBuf, Transfer>,
    pub(crate) zero: std::collections::BTreeMap<std::path::PathBuf, crate::zero::ZeroChain>,
}
/// A file-backed base travels as its verified descriptor; an in-memory base,
/// a compressed floppy, as its decoded bytes. A zero-rooted disk travels as
/// its parts (`zero`), never as its whole decoded length. The metadata is
/// boxed because its size depends on the host, and on Linux it alone makes
/// the file variant several times larger than the byte-buffer one.
pub(crate) enum Transfer {
    File(fs::File, media::Header, Box<fs::Metadata>),
    Memory(Vec<u8>),
}
impl PreparedSession {
    /// Apply an edit and derive the machine before publishing either value.
    /// A refused document leaves the previous start intact.
    pub fn edit_launch(&mut self, edit: impl FnOnce(&mut pc_config::launch::Launch)) -> Result<()> {
        self.update_launch(|input| {
            edit(&mut input.document);
            Ok(())
        })
    }

    pub(crate) fn update_launch(
        &mut self,
        edit: impl FnOnce(&mut PreparedLaunch) -> Result<()>,
    ) -> Result<()> {
        let mut launch = self
            .launch
            .clone()
            .ok_or_else(|| invalid("this session has no launch document"))?;
        edit(&mut launch)?;
        let machine = crate::provider::lower_session(&launch)?;
        self.launch = Some(launch);
        self.machine = machine;
        Ok(())
    }

    /// Replace one floppy catalogue in the launch and derive its machine.
    /// Existing bindings keep their content and records; added disks use the
    /// supplied verified bases and short record names independent of host paths.
    /// Neither the document nor the bases change if lowering refuses the edit.
    pub fn replace_floppy_catalogue(
        &mut self,
        unit: u8,
        entries: Vec<assembly::FloppyMedium>,
        inserted: Option<usize>,
        bases: std::collections::BTreeMap<std::path::PathBuf, VerifiedBase>,
    ) -> Result<()> {
        let at = self
            .machine
            .drives
            .iter()
            .position(|d| d.unit == unit && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
            .ok_or_else(|| invalid("no such floppy drive"))?;
        let drive = &self.machine.drives[at];
        let old_entries = drive
            .media
            .as_ref()
            .map(|list| list.entries.clone())
            .unwrap_or_else(|| {
                vec![assembly::FloppyMedium {
                    image: drive.image.clone(),
                    expected_sha256: drive.expected_sha256,
                    write: drive.write.clone(),
                    name: String::new(),
                }]
            });
        let all_bases = &self.bases;
        let mut input = self
            .launch
            .clone()
            .ok_or_else(|| invalid("this session has no launch document"))?;
        let old_slot = input.document.config.slots[at].clone();
        let mut catalogue: Vec<String> = Vec::new();
        for entry in entries {
            let existing = old_entries
                .iter()
                .position(|old| {
                    old.image == entry.image
                        && old.expected_sha256 == entry.expected_sha256
                        && old.write == entry.write
                })
                .and_then(|i| old_slot.catalogue.get(i));
            let id = if let Some(id) = existing.filter(|id| {
                !catalogue.contains(*id)
                    && !input
                        .document
                        .config
                        .slots
                        .iter()
                        .enumerate()
                        .any(|(i, slot)| i != at && slot.catalogue.contains(*id))
            }) {
                let medium = input
                    .document
                    .config
                    .media
                    .iter_mut()
                    .find(|m| &m.id == id)
                    .ok_or_else(|| invalid("missing launch medium"))?;
                medium.name = Some(entry.name);
                id.clone()
            } else {
                let mut index = 0u64;
                let id = loop {
                    let id = format!("catalogue-{index}");
                    let source = format!("assets/{id}.img");
                    if !input
                        .document
                        .config
                        .media
                        .iter()
                        .any(|m| m.id == id || m.source == source)
                        && !input
                            .document
                            .config
                            .firmware
                            .iter()
                            .any(|f| f.source == source)
                        && !input
                            .document
                            .config
                            .nvram
                            .as_ref()
                            .is_some_and(|n| n.source == source)
                    {
                        break id;
                    }
                    index += 1;
                };
                let source = format!("assets/{id}.img");
                let mut medium = if let Some(existing) = existing {
                    let old = input
                        .document
                        .config
                        .media
                        .iter()
                        .find(|m| &m.id == existing)
                        .ok_or_else(|| invalid("missing launch medium"))?
                        .clone();
                    let path = input
                        .sources
                        .get(std::path::Path::new(&old.source))
                        .ok_or_else(|| invalid("missing launch base path"))?
                        .clone();
                    input.sources.insert(source.clone().into(), path);
                    if let Some(bytes) =
                        input.memory.get(std::path::Path::new(&old.source)).cloned()
                    {
                        input.memory.insert(source.clone().into(), bytes);
                    }
                    old
                } else {
                    let assembly::ImageSource::File(path) = &entry.image else {
                        return Err(invalid(
                            "an added catalogue disk needs a verified file base",
                        ));
                    };
                    let base = bases
                        .get(path)
                        .or_else(|| all_bases.get(path))
                        .ok_or_else(|| invalid("missing verified catalogue base"))?;
                    let hash = base.identity().base_digest;
                    if entry.expected_sha256 != Some(hash) {
                        return Err(invalid("catalogue base differs from its SHA-256"));
                    }
                    let digest = pc_config::image::Digest::from_sha256(hash);
                    if !input.document.content.iter().any(|r| r.digest == digest) {
                        input.document.content.push(pc_config::launch::ContentRow {
                            digest,
                            sha256: hash,
                            size: base.byte_len(),
                        });
                    }
                    let assembly::DriveKind::Floppy { geometry } = drive.kind else {
                        return Err(invalid("floppy drive expected"));
                    };
                    let geometry = geometry
                        .medium_for_len(base.byte_len())
                        .ok_or_else(|| invalid("invalid floppy base length"))?;
                    input.sources.insert(source.clone().into(), path.clone());
                    pc_config::image::Medium {
                        id: id.clone(),
                        source: source.clone(),
                        head: digest,
                        name: None,
                        policy: Policy::from_write(&entry.write),
                        geometry: pc_config::image::Geometry::of(assembly::DriveKind::Floppy {
                            geometry,
                        }),
                    }
                };
                medium.id = id.clone();
                medium.source = source;
                medium.name = Some(entry.name);
                if let assembly::WriteMode::Journal {
                    path,
                    keep_generations,
                } = entry.write
                {
                    let name = format!("kept-{id}.upfj");
                    input.records.insert(name.clone(), path);
                    input.document.records.push(pc_config::launch::RecordRow {
                        medium: id.clone(),
                        name,
                        keep_generations,
                    });
                }
                input.document.config.media.push(medium);
                id
            };
            catalogue.push(id);
        }
        let selected = inserted
            .map(|i| {
                catalogue
                    .get(i)
                    .cloned()
                    .ok_or_else(|| invalid("catalogue selection is out of range"))
            })
            .transpose()?;
        let slot = &mut input.document.config.slots[at];
        slot.catalogue = catalogue;
        slot.inserted = selected;
        slot.catalogue_mode = true;
        input.prune();
        let machine = crate::provider::lower_session(&input)?;
        self.launch = Some(input);
        self.machine = machine;
        self.bases.extend(bases);
        Ok(())
    }

    /// Duplicate already verified handles; paths are never reopened during transfer.
    pub fn into_transfer(self) -> Result<SessionTransfer> {
        let mut bases = std::collections::BTreeMap::new();
        let mut zero = std::collections::BTreeMap::new();
        for (path, base) in self.bases {
            base.check_unchanged()?;
            if let Some(chain) = self.zero.get(&path) {
                zero.insert(path, chain.try_clone()?);
                continue;
            }
            let transfer = match base.metadata() {
                Some(metadata) => {
                    let metadata = metadata.clone();
                    let file = base.try_clone_verified_file()?;
                    let identity = base.identity().clone();
                    VerifiedBase::from_verified_file(
                        file.try_clone()?,
                        identity.clone(),
                        metadata.clone(),
                    )?;
                    Transfer::File(file, identity, Box::new(metadata))
                }
                None => {
                    let mut bytes = vec![0; base.byte_len() as usize];
                    base.read_range(0, &mut bytes)?;
                    Transfer::Memory(bytes)
                }
            };
            bases.insert(path, transfer);
        }
        Ok(SessionTransfer {
            launch: self.launch,
            machine: self.machine,
            image: self.image,
            bases,
            zero,
        })
    }
}
impl SessionTransfer {
    /// Rebuild execution-local sector readers without reading or hashing
    /// file-backed payloads. A zero-rooted disk is rebuilt from its parts.
    pub fn restore(self) -> Result<PreparedSession> {
        let mut bases: std::collections::BTreeMap<_, _> = self
            .bases
            .into_iter()
            .map(|(path, transfer)| {
                let base = match transfer {
                    Transfer::File(file, identity, metadata) => {
                        VerifiedBase::from_verified_file(file, identity, *metadata)?
                    }
                    Transfer::Memory(bytes) => {
                        VerifiedBase::from_bytes(&bytes).map_err(|e| invalid(e.to_string()))?
                    }
                };
                Ok((path, base))
            })
            .collect::<Result<_>>()?;
        let mut zero = std::collections::BTreeMap::new();
        for (path, chain) in self.zero {
            bases.insert(path.clone(), chain.mount()?);
            zero.insert(path, std::rc::Rc::new(chain));
        }
        Ok(PreparedSession {
            launch: self.launch,
            machine: self.machine,
            image: self.image,
            bases,
            zero,
        })
    }
}

/// Sweep decoded payload files that neither a tag, a workspace's pinned
/// image nor a machine's image reaches; see `Store::sweep_materialized`. A
/// damaged session refuses the sweep instead of dropping what that workspace
/// may need.
pub fn sweep_materialized(store: &Store) -> Result<image_store::codec::Sweep> {
    let mut pinned = Vec::new();
    for name in store.list_workspaces()? {
        if let Some(digest) = pinned_image_at(store, &name)? {
            pinned.push(digest);
        }
    }
    pinned.extend(
        store
            .read_machines()?
            .images()
            .map(|image| image.digest.clone()),
    );
    Ok(store.sweep_materialized(&pinned)?)
}

/// Read a workspace's image identity without acquiring its execution lock.
/// Desktop discovery uses this only as a hint; Start rechecks under ownership.
pub fn pinned_image_at(store: &Store, name: &str) -> Result<Option<Digest>> {
    Ok(pinned_descriptor_at(store, name)?.map(|image| image.digest))
}
/// The manifest descriptor a workspace's session pins, read the same way.
pub fn pinned_descriptor_at(store: &Store, name: &str) -> Result<Option<Descriptor>> {
    let Some(bytes) = read_small(&store.workspace_path(name)?.join("session.toml"))? else {
        return Ok(None);
    };
    let state: Session = parse(&bytes)?;
    if state.schema != 1 {
        return Err(invalid("unsupported workspace schema"));
    }
    Ok(Some(state.image))
}
