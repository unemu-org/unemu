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
/// Create or resume a pinned session, refusing a different image at the supplied tag.
pub fn start(workspace: &Workspace<'_>, tag: &str) -> Result<assembly::MachineConfig> {
    prepare_start(workspace, tag, false).map(|prepared| prepared.machine)
}
/// Require a fresh disposable session; an existing session is never overwritten.
pub fn start_new(workspace: &Workspace<'_>, tag: &str) -> Result<assembly::MachineConfig> {
    prepare_start(workspace, tag, true).map(|prepared| prepared.machine)
}
pub fn pinned_image(workspace: &Workspace<'_>) -> Result<Digest> {
    Ok(session(workspace)?.image.digest)
}
/// Captured launch input retains verified bases through assembly.
pub struct PreparedSession {
    pub machine: assembly::MachineConfig,
    pub bases: std::collections::BTreeMap<std::path::PathBuf, VerifiedBase>,
    pub image: Digest,
}
pub fn prepare_start(workspace: &Workspace<'_>, tag: &str, fresh: bool) -> Result<PreparedSession> {
    prepare_selected(workspace, tag, fresh, None)
}

/// Start the exact descriptor selected by a desktop, even if its tag has moved.
pub fn prepare_pinned(
    workspace: &Workspace<'_>,
    image: &Descriptor,
    fresh: bool,
) -> Result<PreparedSession> {
    prepare_selected(workspace, "", fresh, Some(image))
}

fn prepare_selected(
    workspace: &Workspace<'_>,
    tag: &str,
    fresh: bool,
    selected: Option<&Descriptor>,
) -> Result<PreparedSession> {
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
    let mut state = if existing {
        session(workspace)?
    } else {
        let image = match selected {
            Some(image) => image.clone(),
            None => store.tag_descriptor(tag)?,
        };
        let retention = crate::ImportRecord::read(store, &image.digest)?
            .map(|r| {
                r.generations
                    .into_iter()
                    .filter_map(|g| g.keep_generations.map(|n| (g.medium, n)))
                    .collect()
            })
            .unwrap_or_default();
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
    let directory = workspace.path().join("media");
    let (image, machine, bases) =
        crate::provider::resolve_pinned(store, &state.image, &directory, Some(&state.retention))?;
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
        machine,
        bases,
        image: state.image.digest,
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
    if expected.is_none() {
        store.check_new_tag(tag)?;
    }
    let resolved = crate::provider::load_pinned(store, &old.image, true, true)?;
    let image = resolved.image;
    let mut config = resolved.config;
    let bases = resolved.bases;
    if old.machine != image.manifest.config.digest {
        return Err(invalid("workspace machine identity mismatch"));
    }
    let scratch = workspace.path().join("publication");
    fs::create_dir_all(&scratch)?;
    let _staging = Staging(scratch.clone());
    let mut descriptors = image.manifest.layers.clone();
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
        let old_head = descriptors
            .iter()
            .find(|d| d.names(&m.head))
            .ok_or_else(|| invalid("missing medium head"))?;
        let flatten_disk = flatten
            && matches!(
                old_head.decoded_media_type(),
                mt::DISK_BASE | mt::DISK_LAYER
            );
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
        let d = if old_head.decoded_media_type() == mt::FLOPPY || flatten_disk {
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
            store.require_space(
                (overlay
                    .as_ref()
                    .map_or(0, |overlay| overlay.written_sectors()) as u64)
                    .saturating_mul(media::layer::RECORD_LEN)
                    .saturating_mul(2),
            )?;
            let pack_path = scratch.join(format!("{}.pack", m.id));
            let mut file = fs::File::create(&pack_path)?;
            let layer = seal(
                &disk.base,
                final_disk,
                crate::disk::geometry(&m.geometry),
                &m.head.to_string(),
                &disk.root.to_string(),
                &mut file,
            )?;
            file.sync_all()?;
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
        m.head = d.decoded_digest()?;
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
            n.head = blob.digest.clone();
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
    layers.extend(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| matches!(d.media_type.as_str(), mt::METADATA | mt::PNG | mt::JPEG))
            .cloned(),
    );
    let configuration = stored(
        store,
        mt::CONFIG,
        toml::to_string(&config)
            .map_err(|e| invalid(e.to_string()))?
            .as_bytes(),
        &mut hook,
    )?;
    let manifest = Manifest::new_with_types(
        "ibm-pc-at",
        configuration,
        layers,
        PcProvider.payload_types(),
    )?;
    let proposed = store.import_manifest_with_types(&manifest, PcProvider.payload_types())?;
    hook(PublicationStep::BlobStored(proposed.digest.clone()))?;
    let verified = store.verify_publication_reusing(&proposed, &PcProvider, &image)?;
    let transaction = Transaction {
        schema: 1,
        old_image: old.image,
        proposed,
        tag: tag.into(),
        expected: expected.cloned(),
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
    if retired.exists() {
        fs::remove_dir_all(retired)?;
    }
    fs::create_dir_all(&media)?;
    fs::File::open(workspace.path())?.sync_all()?;
    hook(PublicationStep::JournalsRetired)?;
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

/// Thread-transferable launch inputs keep verified file descriptors instead of
/// the execution thread's reference-counted sector readers.
pub struct SessionTransfer {
    machine: assembly::MachineConfig,
    image: Digest,
    bases: std::collections::BTreeMap<std::path::PathBuf, Transfer>,
}
/// A file-backed base travels as its verified descriptor; an in-memory base,
/// a compressed floppy, as its decoded bytes.
enum Transfer {
    File(fs::File, media::Header, fs::Metadata),
    Memory(Vec<u8>),
}
impl PreparedSession {
    /// Duplicate already verified handles; paths are never reopened during transfer.
    pub fn into_transfer(self) -> Result<SessionTransfer> {
        let mut bases = std::collections::BTreeMap::new();
        for (path, base) in self.bases {
            base.check_unchanged()?;
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
                    Transfer::File(file, identity, metadata)
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
            machine: self.machine,
            image: self.image,
            bases,
        })
    }
}
impl SessionTransfer {
    /// Rebuild execution-local sector readers without reading or hashing
    /// file-backed payloads.
    pub fn restore(self) -> Result<PreparedSession> {
        let bases = self
            .bases
            .into_iter()
            .map(|(path, transfer)| {
                let base = match transfer {
                    Transfer::File(file, identity, metadata) => {
                        VerifiedBase::from_verified_file(file, identity, metadata)?
                    }
                    Transfer::Memory(bytes) => {
                        VerifiedBase::from_bytes(&bytes).map_err(|e| invalid(e.to_string()))?
                    }
                };
                Ok((path, base))
            })
            .collect::<Result<_>>()?;
        Ok(PreparedSession {
            machine: self.machine,
            image: self.image,
            bases,
        })
    }
}

/// Sweep decoded payload files that neither a tag nor a workspace's pinned
/// image reaches; see `Store::sweep_materialized`. A damaged session refuses
/// the sweep instead of dropping what that workspace may need.
pub fn sweep_materialized(store: &Store) -> Result<image_store::codec::Sweep> {
    let mut pinned = Vec::new();
    for name in store.list_workspaces()? {
        if let Some(digest) = pinned_image_at(store, &name)? {
            pinned.push(digest);
        }
    }
    Ok(store.sweep_materialized(&pinned)?)
}

/// Read a workspace's image identity without acquiring its execution lock.
/// Desktop discovery uses this only as a hint; Start rechecks under ownership.
pub fn pinned_image_at(store: &Store, name: &str) -> Result<Option<Digest>> {
    use std::io::Read;
    let path = store.workspace_path(name)?.join("session.toml");
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
    let state: Session = parse(&bytes)?;
    if state.schema != 1 {
        return Err(invalid("unsupported workspace schema"));
    }
    Ok(Some(state.image.digest))
}
