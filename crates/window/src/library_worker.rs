// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host image-store jobs and recoverable desktop sessions; machine-images §§3–4, 7.
//! Guest execution stays on the presenter thread. Durable records precede
//! preparation so forced exit and interrupted publication never hide a session.
use crate::library_view::{self, ImageRow, Query};
use crate::save_image::Target;
use image_store::{
    summary::{ImageSummary, SummaryCache},
    workspace::Workspace,
    Descriptor, Digest, Store,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
};

type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum WorkspaceKind {
    Regular,
    Disposable,
    /// A machine's own workspace; its record in the store, not a desktop
    /// record, says what it is.
    Machine,
}
impl WorkspaceKind {
    pub fn disposable(self) -> bool {
        self == Self::Disposable
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Finish {
    Retain,
    Discard,
    Deferred,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Recovery {
    pub schema: u32,
    pub kind: WorkspaceKind,
    pub reference: String,
    pub image: Descriptor,
    pub title: String,
    pub recovery: bool,
    /// The machine a Gaming-mode copy was made from: keeping the copy takes
    /// that machine's settings and labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(skip)]
    pub name: String,
}
impl Recovery {
    pub fn row(&self) -> ImageRow {
        ImageRow {
            reference: self.reference.clone(),
            aliases: Vec::new(),
            descriptor: self.image.clone(),
            summary: None,
            failure: None,
        }
    }
    fn write(&self, owner: &Workspace<'_>) -> Result<()> {
        owner
            .write(
                "desktop.toml",
                toml::to_string(self).map_err(|e| e.to_string())?.as_bytes(),
            )
            .map_err(|e| e.to_string())
    }
}
pub(super) enum Request {
    Query(Query),
    Check(ImageRow, Option<String>),
    Launch(ImageRow, WorkspaceKind, Option<String>),
    Finish(Finish),
    Import(PathBuf, String, Vec<PathBuf>),
    Delete(ImageRow),
    Edit(Digest, bool, Vec<String>),
    Layers(ImageRow, String),
    /// Save as new image…: publish a workspace under the form's target.
    Publish(ImageRow, String, Target),
    /// Settings on an image: read the image's configuration and stored CMOS.
    OpenSettings(ImageRow),
    /// Settings on an image: check the draft at a revision of the sheet.
    PlanRevision(ImageRow, u64, Box<toml::Table>),
    /// Settings on an image: publish a checked configuration-only revision.
    PublishRevision(Box<pc_image::revision::Plan>, Target),
    Discard(String),
    /// Start a machine, or with `true` a disposable Gaming-mode copy of it.
    LaunchMachine(String, bool),
    /// The running machine assembled: its launch is confirmed in its record.
    Assembled,
    /// A change of the machines in the store.
    Machine(MachineJob),
    /// Scan a picked ROM folder, reusing the persistent ROM hash index.
    ScanFirmware(
        PathBuf,
        Option<PathBuf>,
        assembly::firmware_discovery::DiscoveryControl,
    ),
    /// The library's verdict on a set of the last scan with a statement, at a
    /// revision of the set's editor.
    PreviewFirmware(String, u64, Box<pc_image::firmware_import::Statement>),
    /// Publish the named catalogue sets of the last scan, each with its statement.
    ImportFirmware(Vec<(String, pc_image::firmware_import::Statement)>),
    /// Edit metadata…: read a stored package.
    OpenFirmwareEdit(String),
    /// Edit metadata…: the dry run of a revision of the package `tag` as
    /// `target`, at a revision of the editor.
    PreviewFirmwareEdit(
        String,
        String,
        u64,
        Box<pc_image::firmware_import::Statement>,
    ),
    /// Edit metadata…: publish the new revision.
    PublishFirmwareEdit(String, String, Box<pc_image::firmware_import::Statement>),
    /// The package pane's README text and push-guard lines for a tag.
    ReadFirmwarePackage(String, Digest),
    /// List picked installation images, a folder of them, or extras.
    PreviewMedia(library_view::install_media::Pick, Vec<PathBuf>),
    /// Publish a reviewed installation set as `image media import` does.
    ImportMedia(
        Box<pc_image::media_import::Request>,
        library_view::install_media::Control,
    ),
    /// Read the firmware packages and tags New machine… chooses from.
    FirmwarePackages,
    /// Build a new empty image under a reference, moving a tag only from the
    /// digest the user confirmed.
    CreateImage(Box<pc_image::create::NewImage>, String, Option<Digest>),
    /// Image › Push… and Image › Pull…: the dry run and the registry transfers.
    Transfer(crate::registry_transfer::TransferRequest),
    /// Create from files…: read the names and sizes of a host file or
    /// folder, never its contents.
    ReadHost(PathBuf),
    /// Create and insert: build the disks and store them where the place
    /// says, a stopped machine's as its pending insert.
    CreateDisks(
        Box<crate::created_media::CreateOrder>,
        crate::created_media::Place,
    ),
    /// Remotes: list and read a registry without pulling.
    Browse(crate::remote_browse::BrowseRequest),
    /// The running machine's floppy catalogues at Stop, sent before its
    /// `Finish`: a library machine, or a Gaming-mode copy of one, keeps them
    /// for its next Start.
    KeepCatalogue(Vec<crate::kept_catalogue::StoppedDrive>),
    Shutdown,
}
impl Request {
    /// The request sends a machine to run: Start, Gaming mode, a power cycle
    /// that starts again, and Start on an image that makes its machine first.
    pub fn launches(&self) -> bool {
        matches!(
            self,
            Request::Launch(..)
                | Request::LaunchMachine(..)
                | Request::Machine(MachineJob::Create { start: true, .. })
        )
    }
}
/// What the library asks of the machines in the store; machine-images §3.2.
pub(super) enum MachineJob {
    /// Make a machine from an image; `start` launches it once it is listed.
    Create {
        image: Box<ImageRow>,
        name: Option<String>,
        start: bool,
    },
    Rename(String, String),
    Favorite(String, bool),
    Labels(String, Vec<String>),
    Duplicate(String, String),
    Reset(String),
    Delete(String),
    /// Set or clear the disk one drive of a stopped machine gets at its next
    /// Start; the machine's other drives keep theirs.
    SetPending {
        id: String,
        drive: String,
        insert: Option<Box<image_store::machine::PendingInsert>>,
    },
    /// Choose which of the created disks waiting for one drive of a stopped
    /// machine goes in at its next Start, by its place among them.
    SetCreatedFirst {
        id: String,
        drive: String,
        first: usize,
    },
    /// Choose which disk of the catalogue one drive of a stopped machine
    /// kept at its last Stop goes in at its next Start, by its place there.
    SetKeptDisk {
        id: String,
        drive: String,
        index: usize,
    },
    /// Finish an interrupted Save as new image… as it was chosen.
    FinishCommit(String),
    /// Read a machine's settings beside its image's, for its details pane.
    ReadSettings(String),
    /// Open Settings… on a machine: the property sheet's source.
    OpenSettings(String),
    /// Keep tables as a machine's settings from its next start.
    ApplySettings(String, Box<toml::Table>),
    /// Save as new image… of a machine; `keep` leaves it on its old image.
    Commit {
        id: String,
        target: Target,
        keep: bool,
    },
    /// Keep as new machine… of a Gaming-mode copy.
    KeepCopy {
        workspace: String,
        name: String,
    },
}
/// What Settings… on a machine starts from: its id and name, its image's
/// row and source, the tables it runs with and every local tag.
pub(super) type OpenedMachineSettings = (
    String,
    String,
    ImageRow,
    pc_image::revision::Source,
    toml::Table,
    Vec<(String, Digest)>,
);
/// Firmware packages, every local tag with its manifest digest, and the
/// redistribution marks of every local manifest (the push guard's input).
pub(super) type StoreChoices = (
    Vec<pc_image::create::Package>,
    Vec<(String, Digest)>,
    image_registry::guard::LocalMarks,
);
/// A refused publication and the store's tags at that moment, so the form
/// asks again with the current digests.
pub(super) struct Refused {
    pub error: String,
    pub tags: Vec<(String, Digest)>,
}
/// What Settings on an image starts from.
pub(super) type OpenedSettings = (ImageRow, pc_image::revision::Source, Vec<(String, Digest)>);
pub(super) enum Event {
    Snapshot(Arc<library_view::Snapshot>),
    /// A readiness check, with what the details panes name from the image's
    /// manifest and configuration.
    Checked(
        Digest,
        Result<pc_image::inspection::ImageDetails>,
        Option<Box<CheckedImage>>,
    ),
    /// A session to run, with the pending inserts of the machine's list
    /// it put into their drives, by which the window names those disks.
    Launched(
        Box<pc_image::workspace::SessionTransfer>,
        Box<ImageRow>,
        String,
        WorkspaceKind,
        Vec<image_store::machine::PendingInsert>,
    ),
    Finished,
    /// A workspace's media, the image its session pins for Save as new image…,
    /// and what names the image's drives and firmware as the details panes
    /// name them: what a check reads of its configuration, and its summary,
    /// whose ROM layers carry the open marks.
    Layers(
        String,
        Result<pc_image::inspection::ImageDetails>,
        Box<crate::layers_dialog::SourceImage>,
        Option<Box<CheckedImage>>,
        Option<Box<ImageSummary>>,
    ),
    Done(String),
    Deleted(String),
    Published(Box<ImageRow>),
    Recoveries(Vec<Recovery>),
    Packages(Result<StoreChoices>),
    /// The result of New machine…'s Create, apart from every other job.
    Created(Result<Box<ImageRow>>),
    /// The source of Settings on an image, with every local tag.
    SettingsOpened(Result<Box<OpenedSettings>>),
    /// The check of a Settings draft at a sheet revision, with every local tag.
    Planned(
        u64,
        Result<Box<pc_image::revision::Plan>>,
        Vec<(String, Digest)>,
    ),
    /// The store refused Save as new image….
    PublishRefused(Refused),
    /// The result of publishing a Settings revision.
    Revised(std::result::Result<Box<ImageRow>, Refused>),
    Error(String),
    FirmwareError(String),
    FirmwareScanned(Box<library_view::firmware::ScanReport>),
    FirmwareImported(Vec<library_view::firmware::Outcome>),
    FirmwarePreviewed(
        String,
        u64,
        Result<Box<library_view::firmware_metadata::Verdict>>,
    ),
    FirmwareEditOpened(Result<Box<library_view::firmware_metadata::EditSource>>),
    FirmwareEditPreviewed(u64, Result<Box<library_view::firmware_metadata::Verdict>>),
    /// The edited tag, then the new revision's tag and the library's notes,
    /// or the refusal.
    FirmwareEdited(String, Result<(String, Vec<String>)>),
    FirmwarePackage(
        Digest,
        Result<Box<library_view::firmware_metadata::PackageDetails>>,
    ),
    /// A scan or import job failed; only the import dialog shows it.
    FirmwareFailed(String),
    Transfer(crate::registry_transfer::TransferEvent),
    /// What Remotes read of a registry, and local states.
    Browse(crate::remote_browse::BrowseEvent),
    /// Listed picks, or the refusal; only the media import dialog shows it.
    MediaSources(
        library_view::install_media::Pick,
        Result<library_view::install_media::Sources>,
    ),
    MediaImported(Result<library_view::install_media::Outcome>),
    /// Every machine in the store, or why they could not be listed.
    Machines(std::result::Result<Arc<Vec<library_view::machines::MachineRow>>, String>),
    /// Workspaces the store adopted as machines when the library opened.
    Migrated(pc_image::machine::Migration),
    /// A machine job finished: its message, and the machine to select with
    /// its image's row, started when `start`.
    MachineDone {
        message: String,
        select: Option<(String, Box<ImageRow>)>,
        start: bool,
    },
    /// A small machine change was kept (favourite, labels, a pending disk):
    /// its status line only, so no dialog or busy state of another job changes.
    MachineNote(String),
    /// A machine's settings beside its image's, for its details pane.
    MachineSettings(String, Result<Box<pc_image::machine::Settings>>),
    MachineSettingsOpened(Result<Box<OpenedMachineSettings>>),
    /// Settings… on a machine kept its tables, or refused them.
    MachineSettingsApplied(Result<Box<pc_image::machine::SettingsChange>>),
    /// The store refused Save as new image… of a machine.
    CommitRefused(Refused),
    /// A host file or folder read for Create from files…, or why not.
    HostRead(
        PathBuf,
        Box<std::result::Result<media::fat12::HostItem, String>>,
    ),
    /// The disks Create and insert made, or why it failed.
    DisksCreated(std::result::Result<Box<crate::created_media::Created>, String>),
    /// What a Start left out and why, sent before its `Launched`: created
    /// disks whose record or images could not be used.
    StartNotes(Vec<String>),
    /// The drives a Start gave back the catalogue they had at the last
    /// Stop, sent right before its `Launched`: the window names their set
    /// disks and puts the kept disk in before the first instruction.
    StartCatalogue(Vec<crate::kept_catalogue::RestoredDrive>),
}
pub(super) struct Worker {
    pub send: mpsc::Sender<Request>,
    pub receive: mpsc::Receiver<Event>,
    pub cancel: Arc<AtomicU64>,
    done: mpsc::Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn spawn(root: PathBuf, wake: egui::Context) -> Self {
        Self::spawn_backend(root, wake, PcBackend)
    }
    fn spawn_backend<B: Backend>(root: PathBuf, wake: egui::Context, backend: B) -> Self {
        let (send, requests) = mpsc::channel();
        let (events, receive) = mpsc::sync_channel(2);
        let (finished, done) = mpsc::channel();
        let cancel = Arc::new(AtomicU64::new(0));
        let token = cancel.clone();
        let join = std::thread::spawn(move || {
            run(root, requests, events, wake, token, backend);
            let _ = finished.send(());
        });
        Self {
            send,
            receive,
            cancel,
            done,
            join: Some(join),
        }
    }
    pub fn shutdown(&mut self) {
        self.cancel.fetch_add(1, Ordering::Relaxed);
        let _ = self.send.send(Request::Shutdown);
        // The event queue must be drained while waiting: a final notification
        // cannot hold the worker behind its bounded backpressure on exit.
        for _ in 0..25 {
            while self.receive.try_recv().is_ok() {}
            if self
                .done
                .recv_timeout(std::time::Duration::from_millis(10))
                .is_ok()
            {
                if let Some(join) = self.join.take() {
                    let _ = join.join();
                }
                return;
            }
        }
        // The durable desktop record remains the next-startup recovery source.
        self.join.take();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if self.join.is_some() {
            self.shutdown();
        }
    }
}

pub(super) fn regular_name(image: &Digest) -> String {
    format!("regular-{}", &image.hex()[..56])
}
// What a readiness check reads beside an image's details is the PC
// library's; the worker fills it.
pub(super) use pc_library::hardware::CheckedImage;

/// The OCI manifest annotations the details panes show.
const DESCRIPTION: &str = "org.opencontainers.image.description";

/// The PC as Remotes asks it while browsing: what a read tag's platform is
/// and how its media formats are named. Browsing validates and pushes
/// nothing, so it reads neither the provider list nor the firmware
/// catalogue.
fn browsing_platform() -> crate::pc_desktop::PcDesktop<'static> {
    crate::pc_desktop::PcDesktop::new(&[], &[])
}

/// Read an image's draft, cards and manifest annotations. Only the manifest
/// and configuration are read; no workspace is opened.
fn checked_image(store: &Store, row: &ImageRow) -> Option<Box<CheckedImage>> {
    let source = pc_image::revision::Source::open(store, &row.descriptor).ok()?;
    let cards = source
        .machine()
        .get("card")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|card| {
            let kind = card.get("kind")?.as_str()?;
            let id = card.get("id").and_then(toml::Value::as_str).unwrap_or(kind);
            Some((id.to_owned(), kind.to_owned()))
        })
        .collect();
    let annotations =
        image_store::BlobSource::read_verified(store, &row.descriptor, image_store::MAX_SMALL_BLOB)
            .ok()
            .and_then(|bytes| image_store::Manifest::decode_for_summary(&bytes).ok())
            .map(|manifest| manifest.annotations)
            .unwrap_or_default();
    Some(Box::new(CheckedImage {
        draft: crate::image_settings::describe(source.machine()).ok(),
        cards,
        description: annotations
            .get(DESCRIPTION)
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty()),
        base: annotations
            .get(pc_image::workspace::BASE_DIGEST)
            .and_then(|d| d.parse().ok()),
    }))
}

/// Tests: the readiness answer the worker sends for `row`, without its
/// sharing totals.
#[cfg(test)]
pub(super) fn checked(store: &Store, row: &ImageRow, workspace: Option<&str>) -> Event {
    let result = pc_image::desktop::check(store, &row.descriptor, workspace, || false)
        .map_err(|e| e.to_string());
    let machine = result.is_ok().then(|| checked_image(store, row));
    Event::Checked(row.descriptor.digest.clone(), result, machine.flatten())
}

pub(super) fn regular_workspace(store: &Store, image: &Digest) -> Result<String> {
    let names = store.list_workspaces().map_err(|e| e.to_string())?;
    // Older persistent desktop sessions used this prefix; retain their journals.
    for name in names
        .iter()
        .filter(|n| n.starts_with("regular-") || n.starts_with("gaming-"))
    {
        if matches!(pc_image::workspace::pinned_image_at(store,name),Ok(Some(id)) if &id==image) {
            return Ok(name.clone());
        }
    }
    let preferred = regular_name(image);
    if !names.contains(&preferred) {
        return Ok(preferred);
    }
    for number in 1u64.. {
        let name = format!("regular-{number}-{}", &image.hex()[..40]);
        if !names.contains(&name) {
            return Ok(name);
        }
    }
    Err("No workspace name is available".into())
}
fn disposable_name(store: &Store, image: &Digest) -> Result<String> {
    for number in 0u64.. {
        let name = format!(
            "disposable-{}-{}-{number}",
            &image.hex()[..16],
            std::process::id()
        );
        if !store
            .workspace_path(&name)
            .map_err(|e| e.to_string())?
            .exists()
        {
            return Ok(name);
        }
    }
    Err("No disposable workspace name is available".into())
}
/// Whether a workspace belongs to a machine record.
fn is_machine(store: &Store, name: &str) -> bool {
    store
        .read_machines()
        .is_ok_and(|machines| machines.get(name).is_some())
}
fn read_record(store: &Store, name: &str) -> Result<Recovery> {
    use std::io::Read;
    let file = std::fs::File::open(
        store
            .workspace_path(name)
            .map_err(|e| e.to_string())?
            .join("desktop.toml"),
    )
    .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(image_store::MAX_SMALL_BLOB + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > image_store::MAX_SMALL_BLOB {
        return Err("Desktop record exceeds size limit".into());
    }
    let mut record: Recovery =
        toml::from_str(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if record.schema != 1 {
        return Err("Unsupported desktop workspace schema".into());
    }
    record.name = name.into();
    Ok(record)
}
fn recoveries(store: &Store) -> Result<Vec<Recovery>> {
    let mut rows = Vec::new();
    for name in store.list_desktop_workspaces().map_err(|e| e.to_string())? {
        let record = read_record(store, &name)?;
        if record.kind.disposable() || record.recovery {
            rows.push(record);
        }
    }
    // Preserve disposable sessions made before durable desktop records existed.
    let known: BTreeSet<_> = rows.iter().map(|r| r.name.clone()).collect();
    for name in store.list_workspaces().map_err(|e| e.to_string())? {
        if name.starts_with("fresh-") && !known.contains(&name) {
            if let Some(id) =
                pc_image::workspace::pinned_image_at(store, &name).map_err(|e| e.to_string())?
            {
                let path = store
                    .workspace_path(&name)
                    .map_err(|e| e.to_string())?
                    .join("session.toml");
                let table: toml::Table = std::fs::read_to_string(path)
                    .map_err(|e| e.to_string())?
                    .parse::<toml::Table>()
                    .map_err(|e| e.to_string())?;
                let image: Descriptor = table
                    .get("image")
                    .cloned()
                    .ok_or("Missing workspace image")?
                    .try_into()
                    .map_err(|e: toml::de::Error| e.to_string())?;
                rows.push(Recovery {
                    schema: 1,
                    kind: WorkspaceKind::Disposable,
                    reference: format!("Recovered {}", &id.hex()[..12]),
                    title: "Recovered gaming session".into(),
                    image,
                    recovery: true,
                    machine: None,
                    name,
                });
            }
        }
    }
    Ok(rows)
}

/// Replace host operations in lifecycle tests so invented firmware can exercise
/// sessions without relaxing production validation; machine-images §7.
trait Backend: Send + 'static {
    fn prepare(
        &self,
        owner: &Workspace<'_>,
        image: &Descriptor,
        new: bool,
    ) -> Result<pc_image::workspace::PreparedSession>;
    fn publish(&self, owner: &Workspace<'_>, target: &Target) -> Result<Descriptor>;
    fn revise(
        &self,
        store: &Store,
        plan: &pc_image::revision::Plan,
        target: &Target,
    ) -> Result<Descriptor> {
        pc_image::revision::publish(store, plan, &target.reference, target.expected.as_ref())
            .map_err(|e| e.to_string())
    }
    fn check(
        &self,
        store: &Store,
        row: &ImageRow,
        name: Option<&str>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<pc_image::inspection::ImageDetails> {
        pc_image::desktop::check(store, &row.descriptor, name, cancelled).map_err(|e| e.to_string())
    }
    fn firmware_catalogue(&self) -> &'static [assembly::FirmwareEntry] {
        pc_image::firmware::catalogue()
    }
    /// The platform providers registry transfers validate with, the same list
    /// the command line uses.
    fn with_providers<T>(&self, job: impl FnOnce(&[&dyn image_store::ConfigValidator]) -> T) -> T {
        platforms::with_image_providers(job)
    }
    fn import_media(
        &self,
        store: &Store,
        request: &pc_image::media_import::Request,
        control: &library_view::install_media::Control,
    ) -> Result<library_view::install_media::Outcome> {
        library_view::install_media::import(store, request, control)
    }
    /// A machine's launch from its own workspace.
    fn prepare_machine(&self, owner: &Workspace<'_>) -> Result<pc_image::machine::Prepared> {
        pc_image::machine::prepare(owner).map_err(|e| e.to_string())
    }
    /// A Gaming-mode copy of a stopped machine in a fresh workspace.
    fn prepare_copy(
        &self,
        source: &Workspace<'_>,
        copy: &Workspace<'_>,
    ) -> Result<pc_image::machine::Prepared> {
        pc_image::machine::prepare_copy(source, copy).map_err(|e| e.to_string())
    }
    /// A power cycle of a Gaming-mode copy of a machine.
    fn resume_copy(&self, copy: &Workspace<'_>) -> Result<pc_image::machine::Prepared> {
        pc_image::machine::resume_copy(copy).map_err(|e| e.to_string())
    }
    /// An interrupted Save as new image… of a machine, finished as chosen.
    fn finish_commit(&self, store: &Store, id: &str) -> Result<Descriptor> {
        pc_image::machine::finish_commit(store, id).map_err(|e| e.to_string())
    }
    /// Save as new image… of a machine.
    fn commit(&self, store: &Store, id: &str, target: &Target, keep: bool) -> Result<Descriptor> {
        pc_image::machine::commit(
            store,
            id,
            &target.reference,
            target.expected.as_ref(),
            target.title.as_deref(),
            keep,
        )
        .map_err(|e| e.to_string())
    }
    /// Keep as new image… of a Gaming-mode copy of a machine.
    fn publish_copy(&self, owner: &Workspace<'_>, target: &Target) -> Result<Descriptor> {
        pc_image::machine::publish_copy(
            owner,
            &target.reference,
            target.expected.as_ref(),
            target.flatten,
            target.title.as_deref(),
        )
        .map_err(|e| e.to_string())
    }
}
struct PcBackend;
impl Backend for PcBackend {
    fn prepare(
        &self,
        owner: &Workspace<'_>,
        image: &Descriptor,
        new: bool,
    ) -> Result<pc_image::workspace::PreparedSession> {
        pc_image::workspace::prepare_pinned(owner, image, new).map_err(|e| e.to_string())
    }
    fn publish(&self, owner: &Workspace<'_>, target: &Target) -> Result<Descriptor> {
        pc_image::workspace::publish_as(
            owner,
            &target.reference,
            target.expected.as_ref(),
            target.flatten,
            target.title.as_deref(),
            |_| Ok(()),
        )
        .map_err(|e| e.to_string())
    }
}
#[cfg(test)]
struct SyntheticBackend;
#[cfg(test)]
impl Backend for SyntheticBackend {
    fn prepare(
        &self,
        w: &Workspace<'_>,
        image: &Descriptor,
        new: bool,
    ) -> Result<pc_image::workspace::PreparedSession> {
        pc_image::test_support::with_synthetic_firmware(|| PcBackend.prepare(w, image, new))
    }
    fn publish(&self, w: &Workspace<'_>, target: &Target) -> Result<Descriptor> {
        pc_image::test_support::with_synthetic_firmware(|| PcBackend.publish(w, target))
    }
    fn revise(
        &self,
        store: &Store,
        plan: &pc_image::revision::Plan,
        target: &Target,
    ) -> Result<Descriptor> {
        pc_image::test_support::with_synthetic_firmware(|| PcBackend.revise(store, plan, target))
    }
    fn firmware_catalogue(&self) -> &'static [assembly::FirmwareEntry] {
        library_view::firmware::tests::synthetic_catalogue()
    }
    fn with_providers<T>(&self, job: impl FnOnce(&[&dyn image_store::ConfigValidator]) -> T) -> T {
        pc_image::test_support::with_synthetic_firmware(|| platforms::with_image_providers(job))
    }
    fn prepare_machine(&self, owner: &Workspace<'_>) -> Result<pc_image::machine::Prepared> {
        pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::machine::prepare(owner).map_err(|e| e.to_string())
        })
    }
    fn prepare_copy(
        &self,
        source: &Workspace<'_>,
        copy: &Workspace<'_>,
    ) -> Result<pc_image::machine::Prepared> {
        pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::machine::prepare_copy(source, copy).map_err(|e| e.to_string())
        })
    }
    fn resume_copy(&self, copy: &Workspace<'_>) -> Result<pc_image::machine::Prepared> {
        pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::machine::resume_copy(copy).map_err(|e| e.to_string())
        })
    }
    fn finish_commit(&self, store: &Store, id: &str) -> Result<Descriptor> {
        pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::machine::finish_commit(store, id).map_err(|e| e.to_string())
        })
    }
    fn commit(&self, store: &Store, id: &str, target: &Target, keep: bool) -> Result<Descriptor> {
        pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::machine::commit(
                store,
                id,
                &target.reference,
                target.expected.as_ref(),
                target.title.as_deref(),
                keep,
            )
            .map_err(|e| e.to_string())
        })
    }
    fn publish_copy(&self, owner: &Workspace<'_>, target: &Target) -> Result<Descriptor> {
        pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::machine::publish_copy(
                owner,
                &target.reference,
                target.expected.as_ref(),
                target.flatten,
                target.title.as_deref(),
            )
            .map_err(|e| e.to_string())
        })
    }
}
#[cfg(test)]
impl Worker {
    pub(crate) fn synthetic(root: PathBuf) -> Self {
        Self::spawn_backend(root, egui::Context::default(), SyntheticBackend)
    }
    /// A worker without a thread: the test reads every request the library
    /// sends and hands it events of its own.
    pub(crate) fn recording() -> (Self, mpsc::Receiver<Request>, mpsc::SyncSender<Event>) {
        let (send, requests) = mpsc::channel();
        let (events, receive) = mpsc::sync_channel(64);
        let (_, done) = mpsc::channel();
        let worker = Self {
            send,
            receive,
            cancel: Arc::new(AtomicU64::new(0)),
            done,
            join: None,
        };
        (worker, requests, events)
    }
    /// A worker whose thread dies inside a firmware job after `survive`
    /// successful catalogue lookups. Every scan, set or edit preview, import,
    /// package reading, Edit metadata… opening and publication looks the
    /// catalogue up once, so `survive` picks the job that dies.
    pub(crate) fn firmware_crash(root: PathBuf, survive: usize) -> Self {
        Self::spawn_backend(
            root,
            egui::Context::default(),
            FirmwareCrash {
                survive,
                calls: Default::default(),
            },
        )
    }
    /// A worker whose thread dies inside an installation-media import.
    pub(crate) fn media_crash(root: PathBuf) -> Self {
        Self::spawn_backend(root, egui::Context::default(), MediaCrash)
    }
}
#[cfg(test)]
struct MediaCrash;
#[cfg(test)]
impl Backend for MediaCrash {
    fn prepare(
        &self,
        _: &Workspace<'_>,
        _: &Descriptor,
        _: bool,
    ) -> Result<pc_image::workspace::PreparedSession> {
        Err("not used by media tests".into())
    }
    fn publish(&self, _: &Workspace<'_>, _: &Target) -> Result<Descriptor> {
        Err("not used by media tests".into())
    }
    fn import_media(
        &self,
        _: &Store,
        _: &pc_image::media_import::Request,
        _: &library_view::install_media::Control,
    ) -> Result<library_view::install_media::Outcome> {
        panic!("injected worker crash in a media import");
    }
}
#[cfg(test)]
struct FirmwareCrash {
    survive: usize,
    calls: std::sync::atomic::AtomicUsize,
}
#[cfg(test)]
impl Backend for FirmwareCrash {
    fn prepare(
        &self,
        _: &Workspace<'_>,
        _: &Descriptor,
        _: bool,
    ) -> Result<pc_image::workspace::PreparedSession> {
        Err("not used by firmware tests".into())
    }
    fn publish(&self, _: &Workspace<'_>, _: &Target) -> Result<Descriptor> {
        Err("not used by firmware tests".into())
    }
    fn firmware_catalogue(&self) -> &'static [assembly::FirmwareEntry] {
        if self.calls.fetch_add(1, Ordering::Relaxed) >= self.survive {
            panic!("injected worker crash in a firmware job");
        }
        library_view::firmware::tests::synthetic_catalogue()
    }
}

pub(super) fn store_choices(store: &Store) -> Result<StoreChoices> {
    let packages = pc_image::create::packages(store).map_err(|e| e.to_string())?;
    let marks = image_registry::guard::LocalMarks::scan(store).map_err(|e| e.to_string())?;
    let mut tags = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store
            .list_tags(after.as_deref(), 1000)
            .map_err(|e| e.to_string())?;
        tags.extend(page.tags.into_iter().map(|t| (t.name, t.manifest.digest)));
        match page.next_after {
            Some(next) => after = Some(next),
            None => return Ok((packages, tags, marks)),
        }
    }
}

/// New machine…'s Create. The store refuses to move a version-shaped tag.
fn create_image(
    store: &Store,
    image: &pc_image::create::NewImage,
    reference: String,
    expected: Option<&Digest>,
) -> Result<Box<ImageRow>> {
    let descriptor =
        pc_image::create::create(store, image, &reference, expected).map_err(|e| e.to_string())?;
    Ok(Box::new(ImageRow {
        reference,
        aliases: Vec::new(),
        descriptor,
        summary: None,
        failure: None,
    }))
}

/// Settings on an image: the store refuses to move a version-shaped tag.
fn revise_image<B: Backend>(
    backend: &B,
    store: &Store,
    plan: &pc_image::revision::Plan,
    target: Target,
) -> std::result::Result<Box<ImageRow>, Refused> {
    match backend.revise(store, plan, &target) {
        Ok(descriptor) => Ok(Box::new(ImageRow {
            reference: target.reference,
            aliases: Vec::new(),
            descriptor,
            summary: None,
            failure: None,
        })),
        Err(error) => Err(Refused {
            error,
            tags: tags_of(store),
        }),
    }
}

fn tags_of(store: &Store) -> Vec<(String, Digest)> {
    crate::save_image::local_tags(store).unwrap_or_default()
}

fn open_settings(store: &Store, row: ImageRow) -> Result<Box<OpenedSettings>> {
    let source =
        pc_image::revision::Source::open(store, &row.descriptor).map_err(|e| e.to_string())?;
    Ok(Box::new((row, source, tags_of(store))))
}

fn plan_revision(
    store: &Store,
    row: &ImageRow,
    machine: toml::Table,
) -> Result<Box<pc_image::revision::Plan>> {
    let source =
        pc_image::revision::Source::open(store, &row.descriptor).map_err(|e| e.to_string())?;
    pc_image::revision::plan(store, &source, machine)
        .map(Box::new)
        .map_err(|e| e.to_string())
}

/// The image a workspace's session pins (or the row's, before a session
/// exists), named by a tag that names it, with its title, every local tag and
/// any unfinished save. Read without the workspace lock, as display hints.
fn save_source(store: &Store, row: &ImageRow, name: &str) -> crate::layers_dialog::SourceImage {
    use image_store::BlobSource;
    let tags = tags_of(store);
    let image = pc_image::workspace::pinned_descriptor_at(store, name)
        .ok()
        .flatten()
        .unwrap_or_else(|| row.descriptor.clone());
    let reference = if image.digest == row.descriptor.digest {
        row.reference.clone()
    } else {
        let repo = row.reference.rsplit_once(':').map(|(repo, _)| repo);
        let naming: Vec<_> = tags
            .iter()
            .filter(|(_, digest)| *digest == image.digest)
            .map(|(tag, _)| tag.clone())
            .collect();
        naming
            .iter()
            .find(|tag| tag.rsplit_once(':').map(|(r, _)| r) == repo)
            .or(naming.first())
            .cloned()
            .unwrap_or_else(|| format!("{}@{}", repo.unwrap_or("recovered"), image.digest))
    };
    let title = store
        .read_verified(&image, image_store::MAX_SMALL_BLOB)
        .ok()
        .and_then(|bytes| image_store::Manifest::decode_for_summary(&bytes).ok())
        .and_then(|manifest| {
            manifest
                .annotations
                .get(pc_image::workspace::TITLE)
                .cloned()
        });
    crate::layers_dialog::SourceImage {
        reference,
        digest: Some(image.digest),
        title,
        tags,
        pending: pc_image::workspace::pending_publication_at(store, name)
            .ok()
            .flatten(),
    }
}

fn layers_event<B: Backend>(
    backend: &B,
    store: &Store,
    cache: &SummaryCache,
    row: &ImageRow,
    name: String,
) -> Event {
    let details = backend
        .check(store, row, Some(&name), &|| false)
        .and_then(|details| storage_details(store, cache, row, details, &|| false));
    let source = Box::new(save_source(store, row, &name));
    let checked = details.is_ok().then(|| checked_image(store, row)).flatten();
    let summary = cache
        .summary(&row.descriptor.digest)
        .or(row.summary.as_ref())
        .cloned()
        .map(Box::new);
    Event::Layers(name, details, source, checked, summary)
}

fn storage_details(
    store: &Store,
    cache: &SummaryCache,
    row: &ImageRow,
    mut details: pc_image::inspection::ImageDetails,
    cancelled: &dyn Fn() -> bool,
) -> Result<pc_image::inspection::ImageDetails> {
    use image_store::BlobSource;
    let mut sharing = cache
        .blob_sharing_cancellable(cancelled)
        .ok_or("inspection cancelled")?;
    let cached = cache
        .summary(&row.descriptor.digest)
        .filter(|s| !s.storage_blobs.is_empty());
    let descriptors = if let Some(summary) = cached {
        summary.storage_blobs.clone()
    } else {
        if cancelled() {
            return Err("inspection cancelled".into());
        }
        let manifest = image_store::Manifest::decode_for_summary(
            &store
                .read_verified(&row.descriptor, image_store::MAX_SMALL_BLOB)
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        manifest
            .layers
            .into_iter()
            .chain([manifest.config, row.descriptor.clone()])
            .collect()
    };
    let blobs: BTreeMap<_, _> = descriptors
        .into_iter()
        .map(|d| (d.digest, d.size))
        .collect();
    if cached.is_none() {
        for id in blobs.keys() {
            *sharing.entry(id.clone()).or_default() += 1;
        }
    }
    for medium in &mut details.media {
        for member in &mut medium.chain {
            member.shared_by = sharing.get(&member.digest).copied().unwrap_or(1);
        }
    }
    details.stored_bytes = blobs.values().sum();
    details.dedup_bytes = blobs
        .iter()
        .filter(|(id, _)| sharing.get(*id).copied().unwrap_or(1) > 1)
        .map(|(_, size)| size)
        .sum();
    details.exclusive_bytes = details.stored_bytes - details.dedup_bytes;
    Ok(details)
}

/// The floppy drives of a machine configuration, by unit.
fn floppies(config: &assembly::MachineConfig) -> Vec<(u8, assembly::FloppyGeometry)> {
    config
        .drives
        .iter()
        .filter_map(|drive| match drive.kind {
            assembly::DriveKind::Floppy { geometry } => Some((drive.unit, geometry)),
            _ => None,
        })
        .collect()
}

/// A machine as the library lists it: its listing, its image's summary and
/// title, and the floppy drives it runs with (its settings', else its
/// image's), read once per image.
fn machine_row(
    store: &Store,
    cache: &SummaryCache,
    listing: pc_image::machine::Listing,
    drives_of: &mut BTreeMap<Digest, Vec<(u8, assembly::FloppyGeometry)>>,
) -> library_view::machines::MachineRow {
    let image = listing.machine.image.clone();
    let described = |table: &toml::Table| {
        crate::image_settings::describe(table)
            .map(|draft| floppies(&draft.config))
            .unwrap_or_default()
    };
    let drives = match &listing.machine.settings {
        Some(table) => described(table),
        None => drives_of
            .entry(image.digest.clone())
            .or_insert_with(|| {
                pc_image::revision::Source::open(store, &image)
                    .map(|source| described(source.machine()))
                    .unwrap_or_default()
            })
            .clone(),
    };
    let workspace = store.workspace_path(&listing.id).map_err(|e| e.to_string());
    let (created, created_problem) = match workspace
        .clone()
        .and_then(|workspace| crate::created_media::read_pending(&workspace))
    {
        Ok(created) => (created, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    // A record that cannot be read lists nothing; its Start says why.
    let kept = workspace
        .and_then(|workspace| crate::kept_catalogue::read(&workspace))
        .unwrap_or_default();
    library_view::machines::MachineRow {
        summary: cache.summary(&image.digest).cloned(),
        title: pc_image::machine::image_title(store, &image).ok().flatten(),
        drives,
        created,
        created_problem,
        kept,
        listing,
    }
}

fn machine_listing(store: &Store, cache: &SummaryCache) -> Event {
    let mut drives = BTreeMap::new();
    Event::Machines(
        pc_image::machine::list(store)
            .map(|listings| {
                Arc::new(
                    listings
                        .into_iter()
                        .map(|listing| machine_row(store, cache, listing, &mut drives))
                        .collect(),
                )
            })
            .map_err(|e| e.to_string()),
    )
}

/// One machine's row, for the machine a job selects or a launch runs.
fn machine_named(
    store: &Store,
    cache: &SummaryCache,
    id: &str,
) -> Result<library_view::machines::MachineRow> {
    let listing = pc_image::machine::get(store, id).map_err(|e| e.to_string())?;
    Ok(machine_row(store, cache, listing, &mut BTreeMap::new()))
}

fn selected(
    store: &Store,
    cache: &SummaryCache,
    id: &str,
) -> Result<Option<(String, Box<ImageRow>)>> {
    let row = machine_named(store, cache, id)?;
    Ok(Some((id.to_owned(), Box::new(row.image_row()))))
}

/// Run one machine job; a refusal becomes an error event, except for Save
/// as new image…, whose form asks again with the store's current tags.
fn machine_job<B: Backend>(
    backend: &B,
    store: &Store,
    cache: &SummaryCache,
    job: MachineJob,
) -> Event {
    use pc_image::machine;
    let now = || Some(library_view::host_last_run());
    let done = |message: String| Event::MachineDone {
        message,
        select: None,
        start: false,
    };
    let result = (|| -> Result<Event> {
        let error = |e: pc_image::Error| e.to_string();
        Ok(match job {
            MachineJob::Create { image, name, start } => {
                let id = machine::create(
                    store,
                    &image.descriptor,
                    &image.reference,
                    name.as_deref(),
                    now(),
                )
                .map_err(error)?;
                let name = machine_named(store, cache, &id)?.name().to_owned();
                Event::MachineDone {
                    message: format!(
                        "Created the machine \u{201c}{name}\u{201d} from {}",
                        image.reference
                    ),
                    select: selected(store, cache, &id)?,
                    start,
                }
            }
            MachineJob::Rename(id, name) => {
                let old = machine_named(store, cache, &id)?.name().to_owned();
                machine::rename(store, &id, &name).map_err(error)?;
                done(format!(
                    "Renamed \u{201c}{old}\u{201d} to \u{201c}{}\u{201d}",
                    name.trim()
                ))
            }
            MachineJob::Favorite(id, favorite) => {
                machine::set_favorite(store, &id, favorite).map_err(error)?;
                Event::MachineNote(
                    if favorite {
                        "Added to Favourites"
                    } else {
                        "Removed from Favourites"
                    }
                    .into(),
                )
            }
            MachineJob::Labels(id, labels) => {
                machine::set_labels(store, &id, labels).map_err(error)?;
                Event::MachineNote("Labels saved".into())
            }
            MachineJob::Duplicate(id, name) => {
                let old = machine_named(store, cache, &id)?.name().to_owned();
                let copy = machine::duplicate(store, &id, Some(&name), now()).map_err(error)?;
                Event::MachineDone {
                    message: format!(
                        "Duplicated \u{201c}{old}\u{201d} as \u{201c}{}\u{201d}",
                        name.trim()
                    ),
                    select: selected(store, cache, &copy)?,
                    start: false,
                }
            }
            MachineJob::Reset(id) => {
                machine::reset(store, &id).map_err(error)?;
                let row = machine_named(store, cache, &id)?;
                done(format!(
                    "\u{201c}{}\u{201d} is the same as {} again",
                    row.name(),
                    row.reference()
                ))
            }
            MachineJob::Delete(id) => {
                let name = machine_named(store, cache, &id)?.name().to_owned();
                machine::delete(store, &id).map_err(error)?;
                done(format!(
                    "Deleted the machine \u{201c}{name}\u{201d}. Its image is unchanged."
                ))
            }
            MachineJob::SetPending { id, drive, insert } => {
                let name = machine_named(store, cache, &id)?.name().to_owned();
                let message = match &insert {
                    Some(_) => format!(
                        "{drive} of \u{201c}{name}\u{201d} takes the disk at its next Start"
                    ),
                    None => {
                        format!("Took the pending disk out of {drive} of \u{201c}{name}\u{201d}")
                    }
                };
                // Created disks waiting for the drive leave it first, so the
                // machine list stays as it was when their record cannot be
                // changed. A record that cannot be read is replaced.
                let workspace = store.workspace_path(&id).map_err(|e| e.to_string())?;
                if crate::created_media::read_pending(&workspace)
                    .map_or(true, |created| created.iter().any(|c| c.drive == drive))
                {
                    let owner = machine::own(store, &id).map_err(error)?;
                    crate::created_media::set_pending(owner.path(), &drive, None)?;
                }
                machine::set_pending_insert(store, &id, &drive, insert.map(|insert| *insert))
                    .map_err(error)?;
                Event::MachineNote(message)
            }
            MachineJob::SetCreatedFirst { id, drive, first } => {
                let name = machine_named(store, cache, &id)?.name().to_owned();
                let owner = machine::own(store, &id).map_err(error)?;
                crate::created_media::set_first(owner.path(), &drive, first)?;
                Event::MachineNote(format!(
                    "{drive} of \u{201c}{name}\u{201d} takes the disk at its next Start"
                ))
            }
            MachineJob::SetKeptDisk { id, drive, index } => {
                let name = machine_named(store, cache, &id)?.name().to_owned();
                let owner = machine::own(store, &id).map_err(error)?;
                crate::kept_catalogue::set_in_drive(owner.path(), &drive, index)?;
                Event::MachineNote(format!(
                    "{drive} of \u{201c}{name}\u{201d} takes the disk at its next Start"
                ))
            }
            MachineJob::FinishCommit(id) => {
                let tag = machine::pending_commit(store, &id)
                    .map_err(error)?
                    .map(|pending| pending.tag)
                    .unwrap_or_default();
                match backend.finish_commit(store, &id) {
                    Ok(_) => Event::MachineDone {
                        message: format!("Finished saving {tag}"),
                        select: selected(store, cache, &id)?,
                        start: false,
                    },
                    Err(error) => Event::CommitRefused(Refused {
                        error,
                        tags: tags_of(store),
                    }),
                }
            }
            MachineJob::ReadSettings(id) => Event::MachineSettings(
                id.clone(),
                machine::settings(store, &id).map(Box::new).map_err(error),
            ),
            MachineJob::OpenSettings(id) => {
                let row = machine_named(store, cache, &id)?;
                let image = row.image_row();
                let source =
                    pc_image::revision::Source::open(store, &image.descriptor).map_err(error)?;
                let settings = machine::settings(store, &id).map_err(error)?;
                Event::MachineSettingsOpened(Ok(Box::new((
                    id,
                    row.name().to_owned(),
                    image,
                    source,
                    settings.machine,
                    tags_of(store),
                ))))
            }
            MachineJob::ApplySettings(id, table) => Event::MachineSettingsApplied(
                machine::change_settings(store, &id, *table, true)
                    .map(Box::new)
                    .map_err(error),
            ),
            MachineJob::Commit { id, target, keep } => {
                let row = machine_named(store, cache, &id)?;
                match backend.commit(store, &id, &target, keep) {
                    Ok(_) => Event::MachineDone {
                        message: if keep {
                            format!(
                                "Saved {}. \u{201c}{}\u{201d} keeps its changes on {}.",
                                target.reference,
                                row.name(),
                                row.reference()
                            )
                        } else {
                            format!(
                                "Saved {}. \u{201c}{}\u{201d} now runs it with fresh, empty journals.",
                                target.reference,
                                row.name()
                            )
                        },
                        select: selected(store, cache, &id)?,
                        start: false,
                    },
                    Err(error) => Event::CommitRefused(Refused {
                        error,
                        tags: tags_of(store),
                    }),
                }
            }
            MachineJob::KeepCopy { workspace, name } => {
                let id = machine::keep_copy(store, &workspace, &name, now()).map_err(error)?;
                Event::MachineDone {
                    message: format!(
                        "Kept the gaming copy as the machine \u{201c}{}\u{201d}",
                        name.trim()
                    ),
                    select: selected(store, cache, &id)?,
                    start: false,
                }
            }
        })
    })();
    result.unwrap_or_else(Event::Error)
}

/// A Gaming-mode copy of a machine plays with the machine's created disks
/// pending, as with its other pending disks; the copy keeps them for every
/// power cycle and the machine keeps them for its own Start. The copy's
/// drives take back the catalogue the copy kept, at first the machine's.
/// The drives that took one back are for the window, with the launch.
fn copied_disks(
    copy: &Workspace<'_>,
    session: &mut pc_image::workspace::PreparedSession,
    send: &dyn Fn(Event),
) -> Result<Vec<crate::kept_catalogue::RestoredDrive>> {
    let listed = pc_image::machine::copy_of(copy)
        .map_err(|e| e.to_string())?
        .map(|record| record.pending)
        .unwrap_or_default();
    let drives: Vec<String> = listed.iter().map(|p| p.drive.clone()).collect();
    let applied = crate::created_media::apply(session, copy.path(), &drives);
    let restored =
        crate::kept_catalogue::restore(session, copy.store(), copy.path(), &listed, &applied.done);
    note_start(send, [applied.notes, restored.notes].concat());
    Ok(restored.drives)
}

/// The catalogues the window handed over at Stop, kept in `workspace` for
/// its next Start. What could not be kept goes to the log and comes back,
/// for the status bar once the session has finished.
fn keep_catalogue(
    store: &Store,
    workspace: &Workspace<'_>,
    drives: Vec<crate::kept_catalogue::StoppedDrive>,
    send: &dyn Fn(Event),
) -> Vec<String> {
    match crate::kept_catalogue::keep(store, workspace.path(), drives) {
        Ok(notes) => {
            for note in &notes {
                eprintln!("unemupc: {note}");
            }
            notes
        }
        Err(error) => {
            send(Event::Error(format!(
                "The drives' catalogues were not kept for the next Start: {error}"
            )));
            Vec::new()
        }
    }
}

/// What a Stop could not keep, after its `Finished`, which would otherwise
/// put "Ready" over it.
fn note_stop(send: &dyn Fn(Event), notes: Vec<String>) {
    if !notes.is_empty() {
        send(Event::MachineNote(notes.join(" ")));
    }
}

/// What a Start left out goes to the log and, after the Start, to the
/// status bar; the Start itself goes on.
fn note_start(send: &dyn Fn(Event), notes: Vec<String>) {
    if !notes.is_empty() {
        for note in &notes {
            eprintln!("unemupc: {note}");
        }
        send(Event::StartNotes(notes));
    }
}

/// The names and sizes of a host file or folder, for Create from files….
fn read_host(path: PathBuf) -> Event {
    let read = media::fat12::HostItem::read(&path).map_err(|e| e.to_string());
    Event::HostRead(path, Box::new(read))
}

/// Serve the library while a session runs, until the session finishes.
/// `None` when the program shuts down first. `assembled` runs when the
/// shell reports that the machine assembled, and `stopped` with the drives'
/// catalogues the shell hands over at Stop.
#[allow(clippy::too_many_arguments)]
fn serve_session<B: Backend>(
    backend: &B,
    store: &Store,
    cache: &mut SummaryCache,
    query: &Query,
    requests: &mpsc::Receiver<Request>,
    send: &dyn Fn(Event),
    mut assembled: impl FnMut(),
    mut stopped: impl FnMut(Vec<crate::kept_catalogue::StoppedDrive>),
) -> Result<Option<Finish>> {
    loop {
        match requests.recv() {
            Ok(Request::Finish(outcome)) => return Ok(Some(outcome)),
            Ok(Request::Shutdown) | Err(_) => return Ok(None),
            Ok(Request::Assembled) => assembled(),
            Ok(Request::KeepCatalogue(drives)) => stopped(drives),
            // Machine jobs are served while a machine runs: renames, settings
            // read read-only and new machines work; a job that needs the
            // running machine's lock is refused by the store with a reason.
            Ok(Request::Machine(job)) => {
                send(machine_job(backend, store, cache, job));
                send(machine_listing(store, cache));
            }
            Ok(Request::Layers(row, name)) => send(layers_event(backend, store, cache, &row, name)),
            // Settings on any image, read-only for the running one.
            Ok(Request::OpenSettings(row)) => {
                send(Event::SettingsOpened(open_settings(store, row)))
            }
            Ok(Request::PlanRevision(row, revision, machine)) => send(Event::Planned(
                revision,
                plan_revision(store, &row, *machine),
                tags_of(store),
            )),
            // New machine… builds images beside the running one.
            Ok(Request::FirmwarePackages) => send(Event::Packages(store_choices(store))),
            Ok(Request::CreateImage(image, reference, expected)) => {
                send(Event::Created(create_image(
                    store,
                    &image,
                    reference,
                    expected.as_ref(),
                )));
                cache.refresh(store).map_err(|e| e.to_string())?;
                send(Event::Snapshot(Arc::new(library_view::snapshot(
                    cache, query,
                ))));
            }
            // Browsing reads registries and local tags only; no session lock.
            Ok(Request::Browse(job)) => {
                crate::remote_browse::run(store, job, &[&browsing_platform()], &|event| {
                    send(Event::Browse(event))
                })
            }
            // Settings on another, stopped image.
            Ok(Request::PublishRevision(plan, target)) => {
                send(Event::Revised(revise_image(backend, store, &plan, target)));
                cache.refresh(store).map_err(|e| e.to_string())?;
                send(Event::Snapshot(Arc::new(library_view::snapshot(
                    cache, query,
                ))));
            }
            // Create from files… into the running machine, or a stopped one.
            Ok(Request::ReadHost(path)) => send(read_host(path)),
            Ok(Request::CreateDisks(order, place)) => {
                send(Event::DisksCreated(
                    crate::created_media::create(store, *order, place).map(Box::new),
                ));
                cache.refresh(store).map_err(|e| e.to_string())?;
                send(Event::Snapshot(Arc::new(library_view::snapshot(
                    cache, query,
                ))));
                send(machine_listing(store, cache));
            }
            _ => (),
        }
    }
}

fn run<B: Backend>(
    root: PathBuf,
    requests: mpsc::Receiver<Request>,
    events: mpsc::SyncSender<Event>,
    wake: egui::Context,
    cancel: Arc<AtomicU64>,
    backend: B,
) {
    let send = |event| {
        let _ = events.send(event);
        wake.request_repaint();
    };
    let run = || -> Result<()> {
        let store = Store::open(root).map_err(|e| e.to_string())?;
        // Desktop sessions made before machines existed become machines
        // once, when the library first opens the store.
        match pc_image::machine::migrate(&store) {
            Ok(migration) if migration.adopted.is_empty() && migration.skipped.is_empty() => (),
            Ok(migration) => send(Event::Migrated(migration)),
            Err(error) => send(Event::Error(error.to_string())),
        }
        let mut cache = SummaryCache::open(&store).map_err(|e| e.to_string())?;
        let mut query = Query::default();
        let mut pending_check: Option<(ImageRow, Option<String>)> = None;
        let mut priority = Vec::<String>::new();
        // Matched sets of the last ROM-folder scan, awaiting the user's picks.
        let mut firmware_candidates = Vec::new();
        send(Event::Snapshot(Arc::new(library_view::snapshot(
            &cache, &query,
        ))));
        send(Event::Recoveries(recoveries(&store)?));
        send(machine_listing(&store, &cache));
        loop {
            let request = match requests.try_recv() {
                Ok(request) => request,
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some((row, name)) = &pending_check {
                        let epoch = cancel.load(Ordering::Relaxed);
                        let cancelled = || cancel.load(Ordering::Relaxed) != epoch;
                        let result = backend
                            .check(&store, row, name.as_deref(), &cancelled)
                            .and_then(|details| {
                                storage_details(&store, &cache, row, details, &cancelled)
                            });
                        if !cancelled() {
                            let machine = result.is_ok().then(|| checked_image(&store, row));
                            send(Event::Checked(
                                row.descriptor.digest.clone(),
                                result,
                                machine.flatten(),
                            ));
                            pending_check = None;
                        }
                        continue;
                    }
                    if !cache.pending().is_empty() {
                        cache
                            .rebuild(
                                &store,
                                8,
                                &priority.iter().map(String::as_str).collect::<Vec<_>>(),
                                pc_image::PAYLOAD_TYPES,
                            )
                            .map_err(|e| e.to_string())?;
                        send(Event::Snapshot(Arc::new(library_view::snapshot(
                            &cache, &query,
                        ))));
                        continue;
                    }
                    match requests.recv() {
                        Ok(request) => request,
                        Err(_) => break,
                    }
                }
            };
            // Previews and readings change nothing, so no new snapshot follows
            // them; a search or a check changes no machine either.
            let mut quiet = false;
            let lists_machines = !matches!(request, Request::Query(_) | Request::Check(..));
            let operation = || -> Result<bool> {
                match request {
                    Request::Shutdown => return Ok(false),
                    Request::Query(q) => {
                        query = q;
                    }
                    Request::Check(row, name) => {
                        priority = vec![row.reference.clone()];
                        pending_check = Some((row, name));
                    }
                    Request::Layers(row, name) => {
                        let name = if name.starts_with("regular-") {
                            regular_workspace(&store, &row.descriptor.digest)?
                        } else {
                            name
                        };
                        send(layers_event(&backend, &store, &cache, &row, name));
                    }
                    Request::Launch(row, kind, resume) => {
                        pending_check = None;
                        let is_new = resume.is_none();
                        let name = match resume {
                            Some(name) => name,
                            None if kind.disposable() => {
                                disposable_name(&store, &row.descriptor.digest)?
                            }
                            None => regular_workspace(&store, &row.descriptor.digest)?,
                        };
                        let owner = store.workspace(&name).map_err(|e| e.to_string())?;
                        let existed = owner
                            .read("session.toml")
                            .map_err(|e| e.to_string())?
                            .is_some();
                        let mut record = Recovery {
                            schema: 1,
                            kind,
                            reference: row.reference.clone(),
                            image: row.descriptor.clone(),
                            title: row.name().into(),
                            recovery: true,
                            machine: None,
                            name: name.clone(),
                        };
                        // A power cycle of a Gaming-mode copy of a machine
                        // keeps its record and runs it again with what the
                        // copy was made with, never as a plain image copy.
                        if !is_new {
                            if let Ok(existing) = read_record(&store, &name) {
                                record.machine = existing.machine;
                                record.title = existing.title;
                                record.reference = existing.reference;
                            }
                        }
                        let mut restored = Vec::new();
                        let prepared = (|| -> Result<_> {
                            record.write(&owner)?;
                            let prepared = if record.machine.is_some() {
                                let mut session = backend.resume_copy(&owner)?.session;
                                restored = copied_disks(&owner, &mut session, &send)?;
                                session
                            } else {
                                let prepared = backend.prepare(
                                    &owner,
                                    &row.descriptor,
                                    is_new && kind.disposable(),
                                )?;
                                store
                                    .edit_library(&row.descriptor.digest, |entry| {
                                        entry.last_run = Some(library_view::host_last_run());
                                        Ok(())
                                    })
                                    .map_err(|e| e.to_string())?;
                                prepared
                            };
                            prepared.into_transfer().map_err(|e| e.to_string())
                        })();
                        let transfer = match prepared {
                            Ok(p) => p,
                            Err(error) => {
                                if !existed {
                                    owner
                                        .discard_and_remove()
                                        .map_err(|e| format!("{error}; cleanup: {e}"))?;
                                }
                                return Err(error);
                            }
                        };
                        if !restored.is_empty() {
                            send(Event::StartCatalogue(restored));
                        }
                        send(Event::Launched(
                            Box::new(transfer),
                            Box::new(row),
                            name,
                            kind,
                            Vec::new(),
                        ));
                        let mut stopped = None;
                        let Some(outcome) = serve_session(
                            &backend,
                            &store,
                            &mut cache,
                            &query,
                            &requests,
                            &send,
                            || (),
                            |drives| stopped = Some(drives),
                        )?
                        else {
                            record.recovery = true;
                            record.write(&owner)?;
                            drop(owner);
                            return Ok(false);
                        };
                        // Only a copy of a machine keeps its catalogue, and
                        // not when it is discarded; an image's session has
                        // no machine to keep it for.
                        let mut stop_notes = Vec::new();
                        if let Some(drives) = stopped
                            .take()
                            .filter(|_| record.machine.is_some() && outcome != Finish::Discard)
                        {
                            stop_notes = keep_catalogue(&store, &owner, drives, &send);
                        }
                        if outcome == Finish::Discard {
                            owner.discard_and_remove().map_err(|e| e.to_string())?;
                        } else {
                            record.recovery = matches!(outcome, Finish::Deferred | Finish::Failed);
                            record.write(&owner)?;
                            drop(owner);
                        }
                        send(Event::Finished);
                        note_stop(&send, stop_notes);
                        send(Event::Recoveries(recoveries(&store)?));
                    }
                    Request::LaunchMachine(id, gaming) => {
                        pending_check = None;
                        let machine = machine_named(&store, &cache, &id)?;
                        let row = machine.image_row();
                        if gaming {
                            let source =
                                pc_image::machine::own(&store, &id).map_err(|e| e.to_string())?;
                            let name = disposable_name(&store, &row.descriptor.digest)?;
                            let owner = store.workspace(&name).map_err(|e| e.to_string())?;
                            let mut record = Recovery {
                                schema: 1,
                                kind: WorkspaceKind::Disposable,
                                reference: row.reference.clone(),
                                image: row.descriptor.clone(),
                                title: format!("{} (gaming copy)", machine.name()),
                                recovery: true,
                                machine: Some(id.clone()),
                                name: name.clone(),
                            };
                            let prepared = (|| -> Result<_> {
                                record.write(&owner)?;
                                let mut prepared = backend.prepare_copy(&source, &owner)?;
                                let restored = copied_disks(&owner, &mut prepared.session, &send)?;
                                let transfer = prepared
                                    .session
                                    .into_transfer()
                                    .map_err(|e| e.to_string())?;
                                Ok((transfer, restored))
                            })();
                            drop(source);
                            let (transfer, restored) = match prepared {
                                Ok(prepared) => prepared,
                                Err(error) => {
                                    owner
                                        .discard_and_remove()
                                        .map_err(|e| format!("{error}; cleanup: {e}"))?;
                                    return Err(error);
                                }
                            };
                            if !restored.is_empty() {
                                send(Event::StartCatalogue(restored));
                            }
                            send(Event::Launched(
                                Box::new(transfer),
                                Box::new(row),
                                name,
                                WorkspaceKind::Disposable,
                                Vec::new(),
                            ));
                            let mut stopped = None;
                            let Some(outcome) = serve_session(
                                &backend,
                                &store,
                                &mut cache,
                                &query,
                                &requests,
                                &send,
                                || (),
                                |drives| stopped = Some(drives),
                            )?
                            else {
                                record.write(&owner)?;
                                return Ok(false);
                            };
                            // A discarded copy keeps nothing.
                            let mut stop_notes = Vec::new();
                            if let Some(drives) =
                                stopped.take().filter(|_| outcome != Finish::Discard)
                            {
                                stop_notes = keep_catalogue(&store, &owner, drives, &send);
                            }
                            if outcome == Finish::Discard {
                                owner.discard_and_remove().map_err(|e| e.to_string())?;
                            } else {
                                record.recovery =
                                    matches!(outcome, Finish::Deferred | Finish::Failed);
                                record.write(&owner)?;
                            }
                            send(Event::Finished);
                            note_stop(&send, stop_notes);
                            send(Event::Recoveries(recoveries(&store)?));
                        } else {
                            let owner =
                                pc_image::machine::own(&store, &id).map_err(|e| e.to_string())?;
                            let mut prepared = backend.prepare_machine(&owner)?;
                            let inserted = prepared.inserted;
                            let listed: Vec<String> =
                                inserted.iter().map(|insert| insert.drive.clone()).collect();
                            let applied = crate::created_media::apply(
                                &mut prepared.session,
                                owner.path(),
                                &listed,
                            );
                            let created = applied.done;
                            let restored = crate::kept_catalogue::restore(
                                &mut prepared.session,
                                &store,
                                owner.path(),
                                &inserted,
                                &created,
                            );
                            note_start(&send, [applied.notes, restored.notes].concat());
                            let transfer = prepared
                                .session
                                .into_transfer()
                                .map_err(|e| e.to_string())?;
                            if !restored.drives.is_empty() {
                                send(Event::StartCatalogue(restored.drives));
                            }
                            send(Event::Launched(
                                Box::new(transfer),
                                Box::new(row),
                                id,
                                WorkspaceKind::Machine,
                                inserted.clone(),
                            ));
                            // The record takes the last run and drops the
                            // inserts only once the machine has assembled.
                            let confirm = || {
                                if let Err(error) = pc_image::machine::confirm(
                                    &owner,
                                    &inserted,
                                    Some(library_view::host_last_run()),
                                ) {
                                    send(Event::Error(error.to_string()));
                                }
                                if let Err(error) =
                                    crate::created_media::confirm(owner.path(), &created)
                                {
                                    send(Event::Error(error));
                                }
                            };
                            let mut stop_notes = Vec::new();
                            let outcome = serve_session(
                                &backend,
                                &store,
                                &mut cache,
                                &query,
                                &requests,
                                &send,
                                confirm,
                                |drives| stop_notes = keep_catalogue(&store, &owner, drives, &send),
                            )?;
                            drop(owner);
                            if outcome.is_none() {
                                return Ok(false);
                            }
                            send(Event::Finished);
                            note_stop(&send, stop_notes);
                        }
                    }
                    Request::Assembled => (),
                    Request::Machine(job) => {
                        // A kept gaming copy is no unfinished session any more.
                        let kept = matches!(job, MachineJob::KeepCopy { .. });
                        send(machine_job(&backend, &store, &cache, job));
                        if kept {
                            send(Event::Recoveries(recoveries(&store)?));
                        }
                    }
                    // A machine's Disk layers… publishes as Save as new image…
                    // does: onto the new image with fresh journals, or it
                    // finishes an interrupted save as that save was chosen.
                    Request::Publish(_, name, target) if is_machine(&store, &name) => {
                        let pending = pc_image::machine::pending_commit(&store, &name)
                            .map_err(|e| e.to_string())?;
                        let published = match pending {
                            Some(_) => backend.finish_commit(&store, &name),
                            None => backend.commit(&store, &name, &target, false),
                        };
                        match published {
                            Ok(descriptor) => send(Event::Published(Box::new(ImageRow {
                                reference: target.reference,
                                aliases: Vec::new(),
                                descriptor,
                                summary: None,
                                failure: None,
                            }))),
                            Err(error) => send(Event::PublishRefused(Refused {
                                error,
                                tags: tags_of(&store),
                            })),
                        }
                    }
                    Request::Publish(row, name, target) => {
                        let owner = store.workspace(&name).map_err(|e| e.to_string())?;
                        let record = read_record(&store, &name).ok().or_else(|| {
                            (name.starts_with("fresh-") || name.starts_with("disposable-")).then(
                                || Recovery {
                                    schema: 1,
                                    kind: WorkspaceKind::Disposable,
                                    reference: row.reference.clone(),
                                    image: row.descriptor.clone(),
                                    title: row.name().into(),
                                    recovery: true,
                                    machine: None,
                                    name: name.clone(),
                                },
                            )
                        });
                        if owner
                            .read("session.toml")
                            .map_err(|e| e.to_string())?
                            .is_none()
                        {
                            backend.prepare(&owner, &row.descriptor, false)?;
                        }
                        let published = if record.as_ref().is_some_and(|r| r.machine.is_some()) {
                            backend.publish_copy(&owner, &target)
                        } else {
                            backend.publish(&owner, &target)
                        };
                        let descriptor = match published {
                            Ok(descriptor) => descriptor,
                            Err(error) => {
                                send(Event::PublishRefused(Refused {
                                    error,
                                    tags: tags_of(&store),
                                }));
                                return Ok(true);
                            }
                        };
                        if record.as_ref().is_some_and(|r| r.kind.disposable()) {
                            owner.discard_and_remove().map_err(|e| e.to_string())?;
                        } else if let Some(mut record) = record {
                            record.image = descriptor.clone();
                            record.reference = target.reference.clone();
                            record.recovery = false;
                            record.write(&owner)?;
                        }
                        send(Event::Published(Box::new(ImageRow {
                            reference: target.reference,
                            aliases: Vec::new(),
                            descriptor,
                            summary: None,
                            failure: None,
                        })));
                        send(Event::Recoveries(recoveries(&store)?));
                    }
                    // Disk layers › Discard changes on a machine is Reset to image….
                    Request::Discard(name) if is_machine(&store, &name) => {
                        pc_image::machine::reset(&store, &name).map_err(|e| e.to_string())?;
                        send(Event::Done("Workspace changes discarded".into()));
                    }
                    Request::Discard(name) => {
                        store
                            .workspace(&name)
                            .map_err(|e| e.to_string())?
                            .discard_and_remove()
                            .map_err(|e| e.to_string())?;
                        send(Event::Done("Workspace changes discarded".into()));
                        send(Event::Recoveries(recoveries(&store)?));
                    }
                    Request::Delete(row) => {
                        library_view::delete_tag(&store, &row)?;
                        send(Event::Deleted(row.reference));
                    }
                    Request::Edit(id, favorite, labels) => store
                        .edit_library(&id, |entry| {
                            entry.favorite = favorite;
                            entry.labels = labels;
                            Ok(())
                        })
                        .map_err(|e| e.to_string())?,
                    Request::Import(path, tag, roots) => match pc_image::import_machine(
                        &store,
                        &path,
                        &tag,
                        &pc_image::ImportOptions {
                            rom_dirs: roots,
                            ..Default::default()
                        },
                    ) {
                        Ok(_) => send(Event::Done(format!("Imported {tag}"))),
                        Err(e) => {
                            if e.needs_firmware() {
                                send(Event::FirmwareError(e.to_string()));
                            } else {
                                return Err(e.to_string());
                            }
                        }
                    },
                    Request::FirmwarePackages => send(Event::Packages(store_choices(&store))),
                    Request::CreateImage(image, reference, expected) => send(Event::Created(
                        create_image(&store, &image, reference, expected.as_ref()),
                    )),
                    Request::PublishRevision(plan, target) => send(Event::Revised(revise_image(
                        &backend, &store, &plan, target,
                    ))),
                    Request::OpenSettings(row) => {
                        send(Event::SettingsOpened(open_settings(&store, row)))
                    }
                    Request::PlanRevision(row, revision, machine) => send(Event::Planned(
                        revision,
                        plan_revision(&store, &row, *machine),
                        tags_of(&store),
                    )),
                    // Every registry request of the dialogs runs here, on the
                    // worker thread, never in the frame loop.
                    Request::Transfer(job) => backend.with_providers(|providers| {
                        let pc = crate::pc_desktop::PcDesktop::new(
                            providers,
                            backend.firmware_catalogue(),
                        );
                        crate::registry_transfer::run(
                            &store,
                            job,
                            &[&pc],
                            &|event| send(Event::Transfer(event)),
                            &|| wake.request_repaint(),
                        )
                    }),
                    // Browsing reads registries only, so no snapshot follows.
                    Request::Browse(job) => {
                        crate::remote_browse::run(&store, job, &[&browsing_platform()], &|event| {
                            send(Event::Browse(event))
                        });
                        quiet = true;
                    }
                    Request::ScanFirmware(folder, index, control) => {
                        match library_view::firmware::scan_folder(
                            &store,
                            &folder,
                            index.as_deref(),
                            &control,
                            backend.firmware_catalogue(),
                        ) {
                            Ok((report, candidates)) => {
                                firmware_candidates = candidates;
                                send(Event::FirmwareScanned(Box::new(report)));
                            }
                            Err(error) => {
                                firmware_candidates.clear();
                                send(Event::FirmwareFailed(error));
                            }
                        }
                    }
                    Request::ImportFirmware(selected) => {
                        match library_view::firmware::import_selected(
                            &store,
                            &firmware_candidates,
                            &selected,
                            backend.firmware_catalogue(),
                        ) {
                            Ok(outcomes) => send(Event::FirmwareImported(outcomes)),
                            Err(error) => send(Event::FirmwareFailed(error)),
                        }
                        firmware_candidates.clear();
                    }
                    Request::PreviewFirmware(id, revision, statement) => {
                        send(Event::FirmwarePreviewed(
                            id.clone(),
                            revision,
                            library_view::firmware::preview_set(
                                &firmware_candidates,
                                &id,
                                &statement,
                                backend.firmware_catalogue(),
                            )
                            .map(Box::new),
                        ));
                        quiet = true;
                    }
                    Request::OpenFirmwareEdit(tag) => {
                        send(Event::FirmwareEditOpened(
                            library_view::firmware_metadata::open_edit(
                                &store,
                                &tag,
                                backend.firmware_catalogue(),
                            )
                            .map(Box::new),
                        ));
                        quiet = true;
                    }
                    Request::PreviewFirmwareEdit(tag, target, revision, statement) => {
                        send(Event::FirmwareEditPreviewed(
                            revision,
                            library_view::firmware_metadata::preview_edit(
                                &store,
                                &tag,
                                &target,
                                &statement,
                                backend.firmware_catalogue(),
                            )
                            .map(Box::new),
                        ));
                        quiet = true;
                    }
                    Request::PublishFirmwareEdit(tag, target, statement) => {
                        send(Event::FirmwareEdited(
                            tag.clone(),
                            library_view::firmware_metadata::publish_edit(
                                &store,
                                &tag,
                                &target,
                                &statement,
                                backend.firmware_catalogue(),
                            ),
                        ))
                    }
                    Request::ReadFirmwarePackage(tag, digest) => {
                        let details = library_view::firmware_metadata::read_package(
                            &store,
                            &tag,
                            &digest,
                            backend.firmware_catalogue(),
                        );
                        send(Event::FirmwarePackage(digest, details.map(Box::new)));
                        quiet = true;
                    }
                    Request::PreviewMedia(pick, paths) => send(Event::MediaSources(
                        pick,
                        library_view::install_media::inspect(pick, &paths),
                    )),
                    Request::ImportMedia(request, control) => send(Event::MediaImported(
                        backend.import_media(&store, &request, &control),
                    )),
                    Request::ReadHost(path) => {
                        send(read_host(path));
                        quiet = true;
                    }
                    Request::CreateDisks(order, place) => send(Event::DisksCreated(
                        crate::created_media::create(&store, *order, place).map(Box::new),
                    )),
                    // Catalogues come only while a machine runs.
                    Request::Finish(_) | Request::KeepCatalogue(_) => (),
                }
                Ok(true)
            };
            match operation() {
                Ok(false) => break,
                Err(e) => send(Event::Error(e)),
                Ok(true) => (),
            }
            if quiet {
                continue;
            }
            cache.refresh(&store).map_err(|e| e.to_string())?;
            send(Event::Snapshot(Arc::new(library_view::snapshot(
                &cache, &query,
            ))));
            if lists_machines {
                send(machine_listing(&store, &cache));
            }
        }
        Ok(())
    };
    if let Err(error) = run() {
        send(Event::Error(error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_test_support::Fixture;
    use std::{fs, time::Duration};
    fn until(worker: &Worker, mut predicate: impl FnMut(&Event) -> bool) -> Event {
        loop {
            let event = worker
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .expect("worker notification");
            if let Event::Error(e) = &event {
                panic!("worker: {e}");
            }
            if predicate(&event) {
                return event;
            }
        }
    }
    fn worker(f: &Fixture) -> Worker {
        restarted(f).0
    }
    fn restarted(f: &Fixture) -> (Worker, Vec<Recovery>) {
        let w = Worker::synthetic(f.store.root().to_owned());
        let Event::Recoveries(rows) = until(&w, |e| matches!(e, Event::Recoveries(_))) else {
            unreachable!()
        };
        (w, rows)
    }
    fn launch(
        w: &Worker,
        row: &ImageRow,
        kind: WorkspaceKind,
    ) -> (pc_image::workspace::PreparedSession, String) {
        w.send
            .send(Request::Launch(row.clone(), kind, None))
            .unwrap();
        let Event::Launched(transfer, _, name, actual, _) =
            until(w, |e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        assert_eq!(actual, kind);
        (transfer.restore().unwrap(), name)
    }
    fn target(reference: &str, flatten: bool) -> Target {
        Target {
            reference: reference.into(),
            expected: None,
            title: None,
            flatten,
        }
    }
    fn finish(w: &Worker, outcome: Finish) -> Vec<Recovery> {
        w.send.send(Request::Finish(outcome)).unwrap();
        let Event::Recoveries(rows) = until(w, |e| matches!(e, Event::Recoveries(_))) else {
            unreachable!()
        };
        rows
    }
    #[test]
    fn regular_finish_reuses_journals_while_gaming_isolated_finish_discards_every_medium() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let (prepared, regular) = launch(&w, &f.row, WorkspaceKind::Regular);
        f.change(&prepared, 31);
        drop(prepared);
        assert!(finish(&w, Finish::Retain).is_empty());
        let (_, again) = launch(&w, &f.row, WorkspaceKind::Regular);
        assert_eq!(regular, again);
        let retained =
            pc_image::desktop::check(&f.store, &f.row.descriptor, Some(&again), || false).unwrap();
        assert_eq!(retained.media.len(), 4);
        assert!(retained.media.iter().all(|m| m.working_sectors == Some(1)));
        finish(&w, Finish::Retain);
        let regular_before: Vec<_> =
            fs::read_dir(f.store.workspace_path(&regular).unwrap().join("media"))
                .unwrap()
                .map(|e| {
                    let p = e.unwrap().path();
                    (p.clone(), fs::read(p).unwrap())
                })
                .collect();
        let (prepared, gaming) = launch(&w, &f.row, WorkspaceKind::Disposable);
        assert_ne!(gaming, regular);
        let details =
            pc_image::desktop::check(&f.store, &f.row.descriptor, Some(&gaming), || false).unwrap();
        assert_eq!(details.media.len(), 4);
        assert!(details.media.iter().all(|m| m.working_sectors == Some(0)));
        f.change(&prepared, 63);
        drop(prepared);
        finish(&w, Finish::Discard);
        assert!(!f.store.workspace_path(&gaming).unwrap().exists());
        for (path, bytes) in regular_before {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        w.shutdown();
    }
    #[test]
    fn failed_or_exit_deferred_gaming_session_is_recovered_then_explicitly_discarded() {
        for outcome in [Finish::Failed, Finish::Deferred] {
            let f = Fixture::new();
            let mut w = worker(&f);
            let (prepared, name) = launch(&w, &f.row, WorkspaceKind::Disposable);
            f.change(&prepared, 51);
            drop(prepared);
            let rows = finish(&w, outcome);
            assert_eq!(rows[0].name, name);
            assert!(rows[0].recovery);
            w.shutdown();
            let (mut w, listed) = restarted(&f);
            assert_eq!(listed[0].name, name);
            w.send.send(Request::Discard(name.clone())).unwrap();
            until(&w, |e| matches!(e, Event::Done(_)));
            assert!(!f.store.workspace_path(&name).unwrap().exists());
            w.shutdown();
        }
    }
    #[test]
    fn shutdown_joins_inflight_finish_and_defers_an_active_disposable_session() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let (_, name) = launch(&w, &f.row, WorkspaceKind::Disposable);
        w.send.send(Request::Finish(Finish::Deferred)).unwrap();
        w.shutdown();
        let (mut w, listed) = restarted(&f);
        assert_eq!(listed[0].name, name);
        let (_, active) = launch(&w, &f.row, WorkspaceKind::Disposable);
        w.shutdown();
        assert!(recoveries(&f.store)
            .unwrap()
            .iter()
            .any(|r| r.name == active && r.recovery));
    }
    #[test]
    fn keep_publishes_then_removes_disposable_but_regular_publication_keeps_workspace() {
        for kind in [WorkspaceKind::Disposable, WorkspaceKind::Regular] {
            let f = Fixture::new();
            let mut w = worker(&f);
            let (prepared, name) = launch(&w, &f.row, kind);
            f.change(&prepared, 42);
            drop(prepared);
            finish(&w, Finish::Retain);
            let details =
                pc_image::desktop::check(&f.store, &f.row.descriptor, Some(&name), || false)
                    .unwrap();
            let crate::layers_dialog::Action::Publish(workspace, target) =
                crate::layers_dialog::tests::reviewed_publication(
                    &name,
                    kind,
                    details,
                    crate::layers_dialog::tests::source(&f.row.reference, &f.row.descriptor.digest),
                    "test:kept",
                    kind == WorkspaceKind::Regular,
                    None,
                )
            else {
                panic!()
            };
            assert_eq!(target.flatten, kind == WorkspaceKind::Regular);
            w.send
                .send(Request::Publish(f.row.clone(), workspace, target))
                .unwrap();
            until(&w, |e| matches!(e, Event::Published(_)));
            assert_eq!(
                f.store.workspace_path(&name).unwrap().exists(),
                kind == WorkspaceKind::Regular
            );
            let details = pc_image::desktop::check(
                &f.store,
                &f.store.tag_descriptor("test:kept").unwrap(),
                None,
                || false,
            )
            .unwrap();
            assert_eq!(details.media.len(), 4);
            assert!(details
                .media
                .iter()
                .all(|m| m.chain.len() == 1 || kind.disposable()));
            let resolved = pc_image::test_support::with_synthetic_firmware(|| {
                pc_image::resolve(&f.store, "test:kept", &f.temp.path().join("inspect"))
            })
            .unwrap();
            let image = pc_image::test_support::with_synthetic_firmware(|| {
                f.store.resolve("test:kept", &pc_image::PcProvider)
            })
            .unwrap();
            let assert_medium = |source: &assembly::ImageSource, sha256: Option<[u8; 32]>| {
                let assembly::ImageSource::File(path) = source else {
                    panic!("expected file")
                };
                // A compressed floppy has no file; read its verified payload.
                let bytes = if path.is_file() {
                    fs::read(path).unwrap()
                } else {
                    let hex: String = sha256.unwrap().iter().map(|b| format!("{b:02x}")).collect();
                    let mut bytes = Vec::new();
                    let digest = format!("sha256:{hex}").parse().unwrap();
                    std::io::Read::read_to_end(
                        &mut image.content_reader(&digest).unwrap(),
                        &mut bytes,
                    )
                    .unwrap();
                    bytes
                };
                assert_eq!(&bytes[..512], &[42; 512]);
            };
            for drive in &resolved.drives {
                if let Some(list) = &drive.media {
                    for entry in &list.entries {
                        assert_medium(&entry.image, entry.expected_sha256);
                    }
                } else {
                    assert_medium(&drive.image, drive.expected_sha256);
                }
            }
            let mut cmos = false;
            for device in &resolved.devices {
                if let assembly::DeviceKind::Rtc(rtc) = &device.kind {
                    if let assembly::NvramSource::Journal { image, .. } = &rtc.nvram {
                        assert_eq!(fs::read(image).unwrap()[32], 42);
                        cmos = true;
                    }
                }
            }
            assert!(cmos);
            w.shutdown();
        }
    }
    #[test]
    fn the_save_form_starts_from_the_image_the_workspace_session_pins() {
        let Some(firmware) = crate::library_test_support::FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&firmware);
        let owner = firmware.store.workspace("pinned").unwrap();
        let prepared = pc_image::workspace::prepare_start(&owner, "open-at:1.0.0", false).unwrap();
        crate::image_settings::tests::write_c(&prepared, 9);
        drop(prepared);
        let saved = pc_image::workspace::publish_as(
            &owner,
            "open-at-dos:1.0.0",
            None,
            false,
            Some("Open AT with FreeDOS"),
            |_| Ok(()),
        )
        .unwrap();
        drop(owner);
        let source = save_source(&firmware.store, &row, "pinned");
        assert_eq!(source.reference, "open-at-dos:1.0.0");
        assert_eq!(source.digest, Some(saved.digest.clone()));
        assert_eq!(source.title.as_deref(), Some("Open AT with FreeDOS"));
        assert_eq!(source.pending, None);
        // An untagged pin keeps the row's repository and names the digest.
        firmware
            .store
            .remove_tag("open-at-dos:1.0.0", &saved.digest)
            .unwrap();
        let source = save_source(&firmware.store, &row, "pinned");
        assert_eq!(source.reference, format!("open-at@{}", saved.digest));
    }
    #[test]
    fn failed_keep_leaves_workspace_for_next_startup() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let (_, name) = launch(&w, &f.row, WorkspaceKind::Disposable);
        finish(&w, Finish::Deferred);
        w.send
            .send(Request::Publish(
                f.row.clone(),
                name.clone(),
                target("invalid tag", false),
            ))
            .unwrap();
        loop {
            if let Event::PublishRefused(refused) = w
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .unwrap()
            {
                assert!(refused.tags.iter().any(|(tag, _)| tag == "test:source"));
                break;
            }
        }
        assert!(f.store.workspace_path(&name).unwrap().exists());
        w.shutdown();
        let (mut restarted, listed) = restarted(&f);
        assert_eq!(listed[0].name, name);
        restarted.shutdown();
    }
    struct BrokenPreparation {
        phase: u8,
    }
    impl Backend for BrokenPreparation {
        fn prepare(
            &self,
            w: &Workspace<'_>,
            image: &Descriptor,
            _: bool,
        ) -> Result<pc_image::workspace::PreparedSession> {
            fs::create_dir_all(w.path().join("media")).unwrap();
            fs::write(w.path().join("media/partial"), b"partial").unwrap();
            if self.phase == 3 {
                panic!("injected interrupted preparation");
            }
            if self.phase == 0 {
                return Err("injected preparation failure".into());
            }
            if self.phase == 2 {
                let library = w.store().root().join("local/library.toml");
                let _ = fs::remove_file(&library);
                fs::create_dir(library).unwrap();
            }
            Ok(pc_image::workspace::PreparedSession {
                machine: assembly::bios(vec![0xf4; 32768], None),
                image: image.digest.clone(),
                bases: if self.phase == 1 {
                    // A base changed after verification cannot be transferred.
                    let path = w.path().join("base");
                    fs::write(&path, [0; 512]).unwrap();
                    let base = media::VerifiedBase::open(&path).unwrap();
                    fs::write(&path, [1; 512]).unwrap();
                    std::collections::BTreeMap::from([(path, base)])
                } else {
                    Default::default()
                },
                zero: Default::default(),
            })
        }
        fn publish(&self, _: &Workspace<'_>, _: &Target) -> Result<Descriptor> {
            unreachable!()
        }
    }
    #[test]
    fn preparation_edit_and_transfer_errors_remove_half_created_workspaces_and_locks() {
        for phase in 0..3 {
            let f = Fixture::new();
            let mut w = Worker::spawn_backend(
                f.store.root().to_owned(),
                egui::Context::default(),
                BrokenPreparation { phase },
            );
            until(&w, |e| matches!(e, Event::Recoveries(_)));
            w.send
                .send(Request::Launch(
                    f.row.clone(),
                    WorkspaceKind::Disposable,
                    None,
                ))
                .unwrap();
            loop {
                if matches!(
                    w.receive
                        .recv_timeout(crate::test_wait::HANG_GUARD)
                        .unwrap(),
                    Event::Error(_)
                ) {
                    break;
                }
            }
            assert_eq!(
                fs::read_dir(f.store.root().join("workspaces"))
                    .unwrap()
                    .count(),
                0
            );
            w.shutdown();
        }
    }
    struct GatedCheck {
        first: std::sync::atomic::AtomicBool,
        started: mpsc::Sender<()>,
        release: std::sync::Mutex<mpsc::Receiver<()>>,
        counts: Arc<std::sync::atomic::AtomicUsize>,
        root: Digest,
    }
    impl Backend for GatedCheck {
        fn prepare(
            &self,
            w: &Workspace<'_>,
            image: &Descriptor,
            new: bool,
        ) -> Result<pc_image::workspace::PreparedSession> {
            let stored = image_store::verification_passes(&self.root);
            let raw = media::data_passes(16384);
            let result = SyntheticBackend.prepare(w, image, new);
            assert_eq!(image_store::verification_passes(&self.root) - stored, 1);
            assert_eq!(media::data_passes(16384) - raw, 1);
            result
        }
        fn publish(&self, w: &Workspace<'_>, target: &Target) -> Result<Descriptor> {
            SyntheticBackend.publish(w, target)
        }
        fn check(
            &self,
            store: &Store,
            row: &ImageRow,
            name: Option<&str>,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<pc_image::inspection::ImageDetails> {
            self.counts.fetch_add(1, Ordering::Relaxed);
            if self.first.swap(false, Ordering::Relaxed) {
                self.started.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
            } else {
                assert!(store.read_library().unwrap().images[&row.descriptor.digest].favorite);
            }
            let stored = image_store::verification_passes(&self.root);
            let raw = media::data_passes(16384);
            let result = PcBackend.check(store, row, name, cancelled);
            assert_eq!(image_store::verification_passes(&self.root), stored);
            assert_eq!(media::data_passes(16384), raw);
            result
        }
    }
    #[test]
    fn ten_rapid_multilayer_selections_cancel_and_coalesce_without_disk_passes_or_materialization()
    {
        use image_store::BlobSource;
        let mut f = Fixture::new();
        let manifest = image_store::Manifest::decode_for_summary(
            &f.store
                .read_verified(&f.row.descriptor, image_store::MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        let root = manifest
            .layers
            .iter()
            .find(|d| d.decoded_media_type() == image_store::media_type::DISK_BASE)
            .unwrap()
            .digest
            .clone();
        for depth in 0..3 {
            let owner = f.store.workspace(&format!("build-{depth}")).unwrap();
            let prepared = SyntheticBackend
                .prepare(&owner, &f.row.descriptor, false)
                .unwrap();
            f.change(&prepared, depth + 1);
            drop(prepared);
            f.row.descriptor = SyntheticBackend
                .publish(&owner, &target(&format!("test:depth-{depth}"), false))
                .unwrap();
        }
        let mut manifest = image_store::Manifest::decode_for_summary(
            &f.store
                .read_verified(&f.row.descriptor, image_store::MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        let rows: Vec<_> = (0..10)
            .map(|index| {
                manifest.annotations.insert(
                    "org.opencontainers.image.title".into(),
                    format!("Layered selection {index}"),
                );
                let descriptor = f
                    .store
                    .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
                    .unwrap();
                let reference = format!("test:selection-{index}");
                pc_image::test_support::with_synthetic_firmware(|| {
                    f.store
                        .update_tag(&reference, &descriptor, None, &pc_image::PcProvider)
                })
                .unwrap();
                ImageRow {
                    reference,
                    aliases: Vec::new(),
                    descriptor,
                    summary: None,
                    failure: None,
                }
            })
            .collect();
        f.row = rows[9].clone();
        let mut summaries = SummaryCache::open(&f.store).unwrap();
        while !summaries.pending().is_empty() {
            summaries
                .rebuild(&f.store, 8, &[], pc_image::PAYLOAD_TYPES)
                .unwrap();
        }
        let cache = f.store.root().join("cache/materialized");
        if cache.exists() {
            fs::remove_dir_all(&cache).unwrap();
        }
        let (started, entered) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let counts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut w = Worker::spawn_backend(
            f.store.root().to_owned(),
            egui::Context::default(),
            GatedCheck {
                first: std::sync::atomic::AtomicBool::new(true),
                started,
                release: std::sync::Mutex::new(gate),
                counts: counts.clone(),
                root,
            },
        );
        until(&w, |e| matches!(e, Event::Recoveries(_)));
        w.send.send(Request::Check(rows[0].clone(), None)).unwrap();
        // Drain snapshots while the worker enters its cancellable inspection.
        loop {
            if entered.try_recv().is_ok() {
                break;
            }
            let _ = w.receive.recv_timeout(Duration::from_millis(10));
        }
        for row in rows.iter().skip(1) {
            w.cancel.fetch_add(1, Ordering::Relaxed);
            w.send.send(Request::Check(row.clone(), None)).unwrap();
        }
        w.cancel.fetch_add(1, Ordering::Relaxed);
        w.send
            .send(Request::Edit(f.row.descriptor.digest.clone(), true, vec![]))
            .unwrap();
        w.send.send(Request::Query(Query::default())).unwrap();
        release.send(()).unwrap();
        let Event::Checked(_, result, _) = until(&w, |e| matches!(e, Event::Checked(..))) else {
            unreachable!()
        };
        assert!(result.unwrap().media.iter().any(|m| m.chain.len() == 4));
        assert_eq!(counts.load(Ordering::Relaxed), 2);
        assert!(!cache.exists());
        w.send
            .send(Request::Layers(
                f.row.clone(),
                regular_name(&f.row.descriptor.digest),
            ))
            .unwrap();
        let Event::Layers(_, result, _, checked, summary) =
            until(&w, |e| matches!(e, Event::Layers(..)))
        else {
            unreachable!()
        };
        let details = result.unwrap();
        // Disk layers names the drives from the draft and the ROMs as the
        // details pane does.
        let checked = checked.expect("the image's configuration");
        assert!(checked.draft.is_some());
        let facts = crate::library_view::hardware::ImageFacts::new(
            &details,
            Some(&checked),
            summary.as_deref(),
            &BTreeMap::new(),
        );
        assert!(!facts.firmware.is_empty(), "{facts:?}");
        assert!(details.dedup_bytes > 0);
        assert_eq!(
            details.exclusive_bytes + details.dedup_bytes,
            details.stored_bytes
        );
        assert!(details
            .media
            .iter()
            .flat_map(|m| &m.chain)
            .any(|m| m.shared_by > 1));
        assert!(!cache.exists());
        pc_image::test_support::with_synthetic_firmware(|| {
            f.store.update_tag(
                &f.row.reference,
                &rows[0].descriptor,
                Some(&f.row.descriptor.digest),
                &pc_image::PcProvider,
            )
        })
        .unwrap();
        let (prepared, _) = launch(&w, &f.row, WorkspaceKind::Regular);
        assert_eq!(prepared.image, f.row.descriptor.digest);
        finish(&w, Finish::Retain);
        w.shutdown();
    }
    #[test]
    fn import_handoff_distinguishes_missing_firmware_from_a_disk_with_firmware_in_its_name() {
        for firmware in [false, true] {
            let f = Fixture::new();
            let mut w = worker(&f);
            let path = f.temp.path().join("machine.toml");
            if firmware {
                fs::remove_file(f.temp.path().join("bios.bin")).unwrap();
            } else {
                let text = fs::read_to_string(&path)
                    .unwrap()
                    .replace("one.img", "firmware-bios.img");
                fs::write(&path, text).unwrap();
            }
            w.send
                .send(Request::Import(path, "test:imported".into(), vec![]))
                .unwrap();
            loop {
                match w
                    .receive
                    .recv_timeout(crate::test_wait::HANG_GUARD)
                    .unwrap()
                {
                    Event::FirmwareError(_) => {
                        assert!(firmware);
                        break;
                    }
                    Event::Error(_) => {
                        assert!(!firmware);
                        break;
                    }
                    _ => (),
                }
            }
            w.shutdown();
        }
    }

    #[test]
    fn interrupted_preparation_without_session_is_listed_on_next_startup() {
        let f = Fixture::new();
        let mut w = Worker::spawn_backend(
            f.store.root().to_owned(),
            egui::Context::default(),
            BrokenPreparation { phase: 3 },
        );
        until(&w, |e| matches!(e, Event::Recoveries(_)));
        w.send
            .send(Request::Launch(
                f.row.clone(),
                WorkspaceKind::Disposable,
                None,
            ))
            .unwrap();
        loop {
            match w.receive.recv_timeout(crate::test_wait::HANG_GUARD) {
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(_) => (),
                Err(e) => panic!("{e}"),
            }
        }
        w.shutdown();
        let rows = recoveries(&f.store).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!f
            .store
            .workspace_path(&rows[0].name)
            .unwrap()
            .join("session.toml")
            .exists());
        let (mut w, listed) = restarted(&f);
        assert_eq!(listed[0].name, rows[0].name);
        w.send.send(Request::Discard(rows[0].name.clone())).unwrap();
        until(&w, |e| matches!(e, Event::Done(_)));
        w.shutdown();
        assert!(!f.store.workspace_path(&rows[0].name).unwrap().exists());
    }

    fn machine_done(w: &Worker) -> (String, Option<(String, Box<ImageRow>)>, bool) {
        let Event::MachineDone {
            message,
            select,
            start,
        } = until(w, |e| matches!(e, Event::MachineDone { .. }))
        else {
            unreachable!()
        };
        (message, select, start)
    }
    fn run_machine(w: &Worker, f: &Fixture, id: &str, value: u8, assembled: bool) {
        w.send
            .send(Request::LaunchMachine(id.into(), false))
            .unwrap();
        let Event::Launched(transfer, row, name, kind, _) =
            until(w, |e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        assert_eq!((name.as_str(), kind), (id, WorkspaceKind::Machine));
        assert_eq!(row.descriptor.digest, f.row.descriptor.digest);
        let prepared = transfer.restore().unwrap();
        f.change(&prepared, value);
        drop(prepared);
        if assembled {
            w.send.send(Request::Assembled).unwrap();
        }
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(w, |e| matches!(e, Event::Finished));
    }

    /// Start runs a machine in its own workspace; its record takes the run
    /// only once the shell reports that it assembled. Disk layers' Discard
    /// changes on a machine resets it to its image and keeps it.
    #[test]
    fn a_machine_runs_in_its_own_workspace_and_its_run_counts_once_it_assembled() {
        use pc_image::machine::State;
        let f = Fixture::new();
        let mut w = worker(&f);
        w.send
            .send(Request::Machine(MachineJob::Create {
                image: Box::new(f.row.clone()),
                name: Some("DOS".into()),
                start: true,
            }))
            .unwrap();
        let (message, select, start) = machine_done(&w);
        assert!(message.contains("\u{201c}DOS\u{201d}"), "{message}");
        let (id, row) = select.unwrap();
        assert!(start);
        assert_eq!(id, "dos");
        assert_eq!(row.descriptor.digest, f.row.descriptor.digest);
        run_machine(&w, &f, &id, 9, false);
        let listing = pc_image::machine::get(&f.store, &id).unwrap();
        assert_eq!(listing.machine.last_run, None, "not assembled, not run");
        run_machine(&w, &f, &id, 9, true);
        let listing = pc_image::machine::get(&f.store, &id).unwrap();
        assert!(listing.machine.last_run.is_some());
        assert!(matches!(listing.state, State::Changed { .. }));
        assert!(!f
            .store
            .workspace_path(&id)
            .unwrap()
            .join("desktop.toml")
            .exists());
        w.send.send(Request::Discard(id.clone())).unwrap();
        until(&w, |e| matches!(e, Event::Done(_)));
        let listing = pc_image::machine::get(&f.store, &id).unwrap();
        assert_eq!(listing.state, State::SameAsImage);
        assert!(listing.machine.last_run.is_some());
        w.shutdown();
    }

    /// Gaming mode on a machine plays a disposable copy of it; the machine
    /// does not change, and Keep as new machine… renames the copy into a
    /// machine of its own.
    #[test]
    fn gaming_mode_on_a_machine_plays_a_copy_that_is_kept_as_a_new_machine() {
        use pc_image::machine::State;
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        w.send
            .send(Request::LaunchMachine(id.clone(), true))
            .unwrap();
        let Event::Launched(transfer, _, name, kind, _) =
            until(&w, |e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        assert_eq!(kind, WorkspaceKind::Disposable);
        assert!(name.starts_with("disposable-"), "{name}");
        let prepared = transfer.restore().unwrap();
        f.change(&prepared, 3);
        drop(prepared);
        let rows = finish(&w, Finish::Deferred);
        let record = rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(record.machine.as_deref(), Some(id.as_str()));
        assert_eq!(record.title, "DOS (gaming copy)");
        assert_eq!(
            pc_image::machine::get(&f.store, &id).unwrap().state,
            State::NeverStarted
        );
        w.send
            .send(Request::Machine(MachineJob::KeepCopy {
                workspace: name.clone(),
                name: "DOS saves".into(),
            }))
            .unwrap();
        let (_, select, _) = machine_done(&w);
        let (kept, _) = select.unwrap();
        assert!(!f.store.workspace_path(&name).unwrap().exists());
        let listing = pc_image::machine::get(&f.store, &kept).unwrap();
        assert_eq!(listing.machine.name, "DOS saves");
        assert!(matches!(listing.state, State::Changed { .. }));
        w.shutdown();
        assert!(recoveries(&f.store).unwrap().is_empty());
    }

    /// Save as new image… of a machine moves it onto the new image with
    /// fresh journals by default; a version tag that exists is refused, so
    /// the form asks again with the store's tags.
    #[test]
    fn save_as_new_image_of_a_machine_moves_it_and_a_taken_version_asks_again() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("Saver"),
            None,
        )
        .unwrap();
        run_machine(&w, &f, &id, 7, true);
        w.send
            .send(Request::Machine(MachineJob::Commit {
                id: id.clone(),
                target: target("saver:1.0.0", false),
                keep: false,
            }))
            .unwrap();
        let (message, select, _) = machine_done(&w);
        assert!(message.contains("fresh, empty journals"), "{message}");
        let (_, row) = select.unwrap();
        assert_ne!(row.descriptor.digest, f.row.descriptor.digest);
        let listing = pc_image::machine::get(&f.store, &id).unwrap();
        assert_eq!(listing.machine.source, "saver:1.0.0");
        assert!(listing.changes.is_empty());
        w.send
            .send(Request::Machine(MachineJob::Commit {
                id: id.clone(),
                target: target("saver:1.0.0", false),
                keep: true,
            }))
            .unwrap();
        let Event::CommitRefused(refused) = until(&w, |e| matches!(e, Event::CommitRefused(_)))
        else {
            unreachable!()
        };
        assert!(refused.tags.iter().any(|(tag, _)| tag == "saver:1.0.0"));
        w.shutdown();
    }

    /// A desktop session made before machines existed becomes a machine the
    /// first time the library opens the store, and only then.
    #[test]
    fn desktop_sessions_made_before_machines_become_machines_once() {
        let f = Fixture::new();
        let name = regular_name(&f.row.descriptor.digest);
        {
            let owner = f.store.workspace(&name).unwrap();
            let prepared = pc_image::test_support::with_synthetic_firmware(|| {
                pc_image::workspace::prepare_pinned(&owner, &f.row.descriptor, false)
            })
            .unwrap();
            f.change(&prepared, 4);
        }
        let mut w = Worker::synthetic(f.store.root().to_owned());
        let Event::Migrated(migration) = until(&w, |e| matches!(e, Event::Migrated(_))) else {
            unreachable!()
        };
        assert_eq!(migration.adopted.len(), 1);
        let Event::Machines(Ok(rows)) = until(&w, |e| matches!(e, Event::Machines(_))) else {
            unreachable!()
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id(), migration.adopted[0]);
        assert!(!rows[0].listing.changes.is_empty());
        assert!(!rows[0].drives.is_empty(), "its floppy drives are listed");
        w.shutdown();
        let mut again = Worker::synthetic(f.store.root().to_owned());
        assert!(matches!(
            until(&again, |e| matches!(
                e,
                Event::Migrated(_) | Event::Machines(_)
            )),
            Event::Machines(_)
        ));
        again.shutdown();
    }

    fn pending_a(set: &Descriptor) -> image_store::machine::PendingInsert {
        image_store::machine::PendingInsert {
            drive: "A:".into(),
            set: set.clone(),
            reference: "disks-set:r1".into(),
            medium: "disk-1".into(),
            all: false,
            journal: true,
        }
    }
    /// Whether drive A: of a prepared session starts with a disk of the
    /// store's installation sets.
    fn starts_with_set_disk(f: &Fixture, prepared: &pc_image::workspace::PreparedSession) -> bool {
        prepared
            .machine
            .drives
            .iter()
            .find(|d| d.unit == 0 && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
            .and_then(|d| d.media.as_ref())
            .and_then(|list| list.entries.first())
            .is_some_and(|m| matches!(&m.image, assembly::ImageSource::File(p) if p.starts_with(f.store.root())))
    }

    /// A stopped machine's pending disk is in its drive at Start and leaves
    /// the record only once the machine assembled; a run that never
    /// assembled keeps it.
    #[test]
    fn a_pending_disk_goes_in_at_start_and_leaves_the_record_once_assembled() {
        let f = Fixture::new();
        let set = f.import_set();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        w.send
            .send(Request::Machine(MachineJob::SetPending {
                id: id.clone(),
                drive: "A:".into(),
                insert: Some(Box::new(pending_a(&set))),
            }))
            .unwrap();
        until(&w, |e| matches!(e, Event::MachineNote(_)));
        for assembled in [false, true] {
            w.send
                .send(Request::LaunchMachine(id.clone(), false))
                .unwrap();
            let Event::Launched(transfer, _, _, _, inserted) =
                until(&w, |e| matches!(e, Event::Launched(..)))
            else {
                unreachable!()
            };
            // The window names the disks the inserts put in by them.
            assert_eq!(inserted, [pending_a(&set)]);
            let prepared = transfer.restore().unwrap();
            assert!(starts_with_set_disk(&f, &prepared));
            drop(prepared);
            if assembled {
                w.send.send(Request::Assembled).unwrap();
            }
            w.send.send(Request::Finish(Finish::Retain)).unwrap();
            until(&w, |e| matches!(e, Event::Finished));
            let pending = pc_image::machine::get(&f.store, &id)
                .unwrap()
                .machine
                .pending;
            assert_eq!(pending.is_empty(), assembled, "assembled: {assembled}");
        }
        w.shutdown();
    }

    /// Of created disks waiting for a stopped machine, the one chosen for
    /// its Start goes in first and the others follow in order.
    #[test]
    fn a_created_disk_chosen_for_start_goes_in_first() {
        use crate::created_media::{CreateOrder, Place};
        use media::fat12::{volume_label, FloppyFormat, HostItem, ShortName};
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let game = f.temp.path().join("host/game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("one.dat"), vec![1; 250_000]).unwrap();
        fs::write(game.join("two.dat"), vec![2; 250_000]).unwrap();
        w.send
            .send(Request::CreateDisks(
                Box::new(CreateOrder {
                    unit: 0,
                    items: vec![HostItem::read(&game).unwrap()],
                    top: ShortName::parse("GAME"),
                    format: FloppyFormat::Dd360,
                    label: volume_label("GAME").ok(),
                    split: true,
                    journal: true,
                    keep: None,
                    title: "GAME".into(),
                }),
                Place::Machine(id.clone()),
            ))
            .unwrap();
        let Event::DisksCreated(created) = until(&w, |e| matches!(e, Event::DisksCreated(_)))
        else {
            unreachable!()
        };
        assert_eq!(created.unwrap().disks.len(), 2);
        w.send
            .send(Request::Machine(MachineJob::SetCreatedFirst {
                id: id.clone(),
                drive: "A:".into(),
                first: 1,
            }))
            .unwrap();
        until(&w, |e| matches!(e, Event::MachineNote(_)));
        let workspace = f.store.workspace_path(&id).unwrap();
        assert_eq!(
            crate::created_media::read_pending(&workspace).unwrap()[0].first,
            Some(1)
        );
        w.send
            .send(Request::LaunchMachine(id.clone(), false))
            .unwrap();
        let Event::Launched(transfer, ..) = until(&w, |e| matches!(e, Event::Launched(..))) else {
            unreachable!()
        };
        let prepared = transfer.restore().unwrap();
        let names: Vec<String> = prepared
            .machine
            .drives
            .iter()
            .find(|d| d.unit == 0 && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
            .and_then(|d| d.media.as_ref())
            .map(|list| list.entries.iter().map(|m| m.name.clone()).collect())
            .unwrap_or_default();
        assert_eq!(names[..2], ["GAME 2 of 2", "GAME 1 of 2"]);
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        w.shutdown();
    }

    /// Create from files… into a stopped machine: the disk is the machine's
    /// own, waiting in its workspace, and replaces a set disk pending for
    /// the drive; the machine list stays as released versions read it. At
    /// Start the disk is in the drive, and it leaves the pending record once
    /// the machine assembled. Take out again clears it; Keep in the library
    /// publishes the disk as a set, once.
    #[test]
    fn a_created_disk_waits_in_a_stopped_machine_and_goes_in_at_start() {
        use crate::created_media::{CreateOrder, Place};
        use media::fat12::{volume_label, FloppyFormat, HostItem, ShortName};
        let f = Fixture::new();
        let set = f.import_set();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        pc_image::machine::set_pending_insert(&f.store, &id, "A:", Some(pending_a(&set))).unwrap();
        let game = f.temp.path().join("host/game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("PLAY.EXE"), vec![0x90; 3000]).unwrap();
        fs::write(game.join("readme.txt"), b"read me").unwrap();
        let order = |keep: Option<(String, String)>| CreateOrder {
            unit: 0,
            items: vec![HostItem::read(&game).unwrap()],
            top: ShortName::parse("GAME"),
            format: FloppyFormat::Dd360,
            label: volume_label("GAME").ok(),
            split: false,
            journal: true,
            keep,
            title: "GAME".into(),
        };
        let create = |w: &Worker, keep| {
            w.send
                .send(Request::CreateDisks(
                    Box::new(order(keep)),
                    Place::Machine(id.clone()),
                ))
                .unwrap();
            let Event::DisksCreated(created) = until(w, |e| matches!(e, Event::DisksCreated(_)))
            else {
                unreachable!()
            };
            created.unwrap()
        };
        let created = create(&w, None);
        let (image, digest, name) = created.disks[0].clone();
        assert_eq!(name, "GAME");
        let workspace = f.store.workspace_path(&id).unwrap();
        assert!(image.starts_with(workspace.join("media/created")));
        assert_eq!(
            fs::read(&image).unwrap(),
            media::fat12::Survey::new(
                &order(None).items,
                order(None).top,
                FloppyFormat::Dd360,
                order(None).label
            )
            .unwrap()
            .plan()
            .unwrap()
            .image(0)
            .unwrap(),
            "the bytes the builder makes, as the command line writes them"
        );
        // The machine list keeps only what released versions know.
        let text = fs::read_to_string(f.store.root().join("local/machines.toml")).unwrap();
        let machines: image_store::machine::Machines = toml::from_str(&text).unwrap();
        assert!(
            machines.get(&id).unwrap().pending.is_empty(),
            "the set disk was replaced"
        );
        assert_eq!(f.store.read_machines().unwrap(), machines);
        let Event::Machines(Ok(rows)) = until(&w, |e| matches!(e, Event::Machines(_))) else {
            unreachable!()
        };
        let row = rows.iter().find(|row| row.id() == id).unwrap();
        assert_eq!(row.created.len(), 1);
        assert_eq!(row.created[0].drive, "A:");
        for assembled in [false, true] {
            w.send
                .send(Request::LaunchMachine(id.clone(), false))
                .unwrap();
            let Event::Launched(transfer, ..) = until(&w, |e| matches!(e, Event::Launched(..)))
            else {
                unreachable!()
            };
            let prepared = transfer.restore().unwrap();
            let first = prepared
                .machine
                .drives
                .iter()
                .find(|d| d.unit == 0 && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
                .and_then(|d| d.media.as_ref())
                .and_then(|list| list.entries.first())
                .cloned()
                .unwrap();
            assert_eq!(first.image, assembly::ImageSource::File(image.clone()));
            assert_eq!(first.expected_sha256, Some(digest));
            assert!(matches!(
                &first.write,
                assembly::WriteMode::Journal { path, .. } if path.parent() == Some(&workspace.join("media"))
            ));
            drop(prepared);
            if assembled {
                w.send.send(Request::Assembled).unwrap();
            }
            w.send.send(Request::Finish(Finish::Retain)).unwrap();
            until(&w, |e| matches!(e, Event::Finished));
            assert_eq!(
                crate::created_media::read_pending(&workspace)
                    .unwrap()
                    .is_empty(),
                assembled,
                "assembled: {assembled}"
            );
        }
        // Take out again clears created disks waiting for the drive.
        create(&w, None);
        w.send
            .send(Request::Machine(MachineJob::SetPending {
                id: id.clone(),
                drive: "A:".into(),
                insert: None,
            }))
            .unwrap();
        until(&w, |e| matches!(e, Event::MachineNote(_)));
        assert!(crate::created_media::read_pending(&workspace)
            .unwrap()
            .is_empty());
        // Kept in the library: one private set, the same tag the next time.
        let keep = || Some(("game:1.0-r1".to_owned(), "GAME".to_owned()));
        assert_eq!(create(&w, keep()).kept.as_deref(), Some("game:1.0-r1"));
        assert_eq!(create(&w, keep()).kept.as_deref(), Some("game:1.0-r1"));
        assert!(f.store.tag_descriptor("game:1.0-r1").is_ok());
        w.shutdown();
    }

    /// A one-disk 360K GAME made from host files for drive A: of machine
    /// `id`, through the worker, as Create and insert asks for it.
    fn created_game(f: &Fixture, w: &Worker, id: &str) -> Box<crate::created_media::Created> {
        use media::fat12::{FloppyFormat, HostItem, ShortName};
        let game = f.temp.path().join("host/game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("PLAY.EXE"), vec![0x90; 3000]).unwrap();
        let order = crate::created_media::CreateOrder {
            unit: 0,
            items: vec![HostItem::read(&game).unwrap()],
            top: ShortName::parse("GAME"),
            format: FloppyFormat::Dd360,
            label: None,
            split: false,
            journal: true,
            keep: None,
            title: "GAME".into(),
        };
        w.send
            .send(Request::CreateDisks(
                Box::new(order),
                crate::created_media::Place::Machine(id.into()),
            ))
            .unwrap();
        let Event::DisksCreated(created) = until(w, |e| matches!(e, Event::DisksCreated(_))) else {
            unreachable!()
        };
        created.unwrap()
    }

    /// The first medium drive A: of a prepared session starts with.
    fn first_in_a(prepared: &pc_image::workspace::PreparedSession) -> assembly::FloppyMedium {
        prepared
            .machine
            .drives
            .iter()
            .find(|d| d.unit == 0 && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
            .and_then(|d| d.media.as_ref())
            .and_then(|list| list.entries.first())
            .cloned()
            .unwrap()
    }

    /// The next worker event that is not a listing, an error included.
    fn next_answer(w: &Worker) -> Event {
        loop {
            let event = w
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .unwrap();
            if !matches!(event, Event::Snapshot(_) | Event::Machines(_)) {
                return event;
            }
        }
    }

    /// A machine's created disks follow it as its pending disks do: a
    /// Gaming-mode copy plays with them and the machine keeps them,
    /// Duplicate copies them, Save as new image… with fresh journals keeps
    /// them, and Reset to image… drops them.
    #[test]
    fn created_disks_follow_copies_saves_and_resets_of_the_machine() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        run_machine(&w, &f, &id, 7, true);
        let created = created_game(&f, &w, &id);
        let digest = created.disks[0].1;
        let workspace = f.store.workspace_path(&id).unwrap();
        // Gaming mode: the copy's drive starts with the disk, from the copy.
        w.send
            .send(Request::LaunchMachine(id.clone(), true))
            .unwrap();
        let Event::Launched(transfer, _, name, _, _) =
            until(&w, |e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        let prepared = transfer.restore().unwrap();
        let first = first_in_a(&prepared);
        assert_eq!(first.expected_sha256, Some(digest));
        assert!(
            matches!(&first.image, assembly::ImageSource::File(p) if p.starts_with(f.store.workspace_path(&name).unwrap())),
            "{first:?}"
        );
        drop(prepared);
        finish(&w, Finish::Discard);
        assert_eq!(
            crate::created_media::read_pending(&workspace)
                .unwrap()
                .len(),
            1
        );
        // Duplicate copies the disk and its pending insert.
        w.send
            .send(Request::Machine(MachineJob::Duplicate(
                id.clone(),
                "DOS copy".into(),
            )))
            .unwrap();
        let (_, select, _) = machine_done(&w);
        let (copy, _) = select.unwrap();
        let copied = f.store.workspace_path(&copy).unwrap();
        assert_eq!(
            crate::created_media::read_pending(&copied).unwrap(),
            crate::created_media::read_pending(&workspace).unwrap()
        );
        assert!(copied
            .join("media/created")
            .join(format!(
                "{}.img",
                created.disks[0].0.file_stem().unwrap().to_string_lossy()
            ))
            .is_file());
        // Save as new image… with fresh journals keeps the disk waiting.
        w.send
            .send(Request::Machine(MachineJob::Commit {
                id: id.clone(),
                target: target("saver:1.0.0", false),
                keep: false,
            }))
            .unwrap();
        let (message, _, _) = machine_done(&w);
        assert!(message.contains("fresh, empty journals"), "{message}");
        assert_eq!(
            crate::created_media::read_pending(&workspace)
                .unwrap()
                .len(),
            1
        );
        assert!(created.disks[0].0.is_file());
        w.send
            .send(Request::LaunchMachine(id.clone(), false))
            .unwrap();
        let Event::Launched(transfer, ..) = until(&w, |e| matches!(e, Event::Launched(..))) else {
            unreachable!()
        };
        assert_eq!(
            first_in_a(&transfer.restore().unwrap()).expected_sha256,
            Some(digest)
        );
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        // Reset to image… drops it.
        w.send
            .send(Request::Machine(MachineJob::Reset(copy.clone())))
            .unwrap();
        machine_done(&w);
        assert!(crate::created_media::read_pending(&copied)
            .unwrap()
            .is_empty());
        assert!(!copied.join("media/created").exists());
        w.shutdown();
    }

    /// A record of created disks that cannot be read never stops a Start:
    /// the disks stay out and the Start says why, as it does for a disk
    /// whose image is gone. Take out again and another Insert replace such
    /// a record; when the record cannot be changed at all, the machine
    /// list keeps its pending disk as it was.
    #[test]
    fn an_unreadable_created_record_never_stops_a_start() {
        let f = Fixture::new();
        let set = f.import_set();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let workspace = f.store.workspace_path(&id).unwrap();
        let record = workspace.join("media/created/pending.toml");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        fs::write(&record, "schema = 9\n[[insert]]\nwhatever = 1\n").unwrap();
        w.send
            .send(Request::Machine(MachineJob::Favorite(id.clone(), true)))
            .unwrap();
        until(&w, |e| matches!(e, Event::MachineNote(_)));
        let Event::Machines(Ok(rows)) = until(&w, |e| matches!(e, Event::Machines(_))) else {
            unreachable!()
        };
        let row = rows.iter().find(|row| row.id() == id).unwrap();
        assert!(row.created_problem.is_some());
        assert!(
            row.state_text().contains("created disks unreadable"),
            "{}",
            row.state_text()
        );
        let start = |w: &Worker| -> Vec<String> {
            w.send
                .send(Request::LaunchMachine(id.clone(), false))
                .unwrap();
            let Event::StartNotes(notes) = until(w, |e| matches!(e, Event::StartNotes(_))) else {
                unreachable!()
            };
            until(w, |e| matches!(e, Event::Launched(..)));
            w.send.send(Request::Finish(Finish::Retain)).unwrap();
            until(w, |e| matches!(e, Event::Finished));
            notes
        };
        let notes = start(&w);
        assert!(notes[0].contains("cannot be read"), "{notes:?}");
        assert!(
            record.exists(),
            "the record stays for a version that reads it"
        );
        // Take out again replaces the record.
        w.send
            .send(Request::Machine(MachineJob::SetPending {
                id: id.clone(),
                drive: "A:".into(),
                insert: None,
            }))
            .unwrap();
        until(&w, |e| matches!(e, Event::MachineNote(_)));
        assert!(!record.exists());
        // A disk whose image is gone stays out with a note; the Start goes on.
        let created = created_game(&f, &w, &id);
        fs::remove_file(&created.disks[0].0).unwrap();
        let notes = start(&w);
        assert!(notes[0].starts_with("GAME did not go into A:"), "{notes:?}");
        // Another Insert replaces an unreadable record too.
        fs::write(&record, "not a record").unwrap();
        w.send
            .send(Request::Machine(MachineJob::SetPending {
                id: id.clone(),
                drive: "A:".into(),
                insert: Some(Box::new(pending_a(&set))),
            }))
            .unwrap();
        until(&w, |e| matches!(e, Event::MachineNote(_)));
        assert!(!record.exists());
        assert_eq!(
            pc_image::machine::get(&f.store, &id)
                .unwrap()
                .machine
                .pending,
            [pending_a(&set)]
        );
        // A record that cannot be changed leaves the machine list as it was.
        fs::create_dir_all(record.join("in-the-way")).unwrap();
        w.send
            .send(Request::Machine(MachineJob::SetPending {
                id: id.clone(),
                drive: "A:".into(),
                insert: None,
            }))
            .unwrap();
        assert!(matches!(next_answer(&w), Event::Error(_)));
        assert_eq!(
            pc_image::machine::get(&f.store, &id)
                .unwrap()
                .machine
                .pending,
            [pending_a(&set)]
        );
        w.shutdown();
    }

    /// Gaming mode on a machine plays it with its settings and pending disk,
    /// which the machine keeps; a power cycle starts the same copy again as
    /// a copy of the machine; Keep as new image… publishes it with the
    /// machine's settings.
    #[test]
    fn a_machine_copy_power_cycles_as_the_machine_and_is_kept_as_an_image() {
        let f = Fixture::new();
        let set = f.import_set();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let mut tables = pc_image::machine::settings(&f.store, &id).unwrap().machine;
        tables
            .entry("memory")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .unwrap()
            .insert("extended_kib".into(), 1024.into());
        pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::machine::change_settings(&f.store, &id, tables, true)
        })
        .unwrap();
        pc_image::machine::set_pending_insert(&f.store, &id, "A:", Some(pending_a(&set))).unwrap();
        let memory = 0x100000 + 1024 * 1024;
        w.send
            .send(Request::LaunchMachine(id.clone(), true))
            .unwrap();
        let Event::Launched(transfer, row, name, kind, _) =
            until(&w, |e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        assert_eq!(kind, WorkspaceKind::Disposable);
        let prepared = transfer.restore().unwrap();
        assert!(starts_with_set_disk(&f, &prepared));
        assert_eq!(prepared.machine.memory.address_space_bytes, memory);
        f.change(&prepared, 3);
        drop(prepared);
        // Power cycle: stop keeping the copy, start it again by its name.
        finish(&w, Finish::Retain);
        w.send
            .send(Request::Launch(
                *row.clone(),
                WorkspaceKind::Disposable,
                Some(name.clone()),
            ))
            .unwrap();
        let Event::Launched(transfer, ..) = until(&w, |e| matches!(e, Event::Launched(..))) else {
            unreachable!()
        };
        let prepared = transfer.restore().unwrap();
        assert_eq!(prepared.machine.memory.address_space_bytes, memory);
        assert!(starts_with_set_disk(&f, &prepared));
        drop(prepared);
        let rows = finish(&w, Finish::Deferred);
        let record = rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(record.machine.as_deref(), Some(id.as_str()));
        assert_eq!(record.title, "DOS (gaming copy)");
        assert_eq!(
            pc_image::machine::get(&f.store, &id)
                .unwrap()
                .machine
                .pending,
            [pending_a(&set)]
        );
        // Keep as new image…: published with the machine's settings.
        w.send
            .send(Request::Publish(
                *row,
                name.clone(),
                target("copy:1.0.0", false),
            ))
            .unwrap();
        until(&w, |e| matches!(e, Event::Published(_)));
        assert!(!f.store.workspace_path(&name).unwrap().exists());
        let resolved = pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::resolve(&f.store, "copy:1.0.0", &f.temp.path().join("copy"))
        })
        .unwrap();
        assert_eq!(resolved.memory.address_space_bytes, memory);
        w.shutdown();
    }

    /// While a machine runs the worker still answers machine jobs: its
    /// settings open (read-only in the window), a rename and a new machine
    /// are made, and a change the running machine refuses says why.
    #[test]
    fn machine_jobs_are_answered_while_a_machine_runs() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        w.send
            .send(Request::LaunchMachine(id.clone(), false))
            .unwrap();
        until(&w, |e| matches!(e, Event::Launched(..)));
        w.send
            .send(Request::Machine(MachineJob::OpenSettings(id.clone())))
            .unwrap();
        assert!(matches!(
            until(&w, |e| matches!(e, Event::MachineSettingsOpened(_))),
            Event::MachineSettingsOpened(Ok(_))
        ));
        w.send
            .send(Request::Machine(MachineJob::Rename(
                id.clone(),
                "DOS 2".into(),
            )))
            .unwrap();
        machine_done(&w);
        w.send
            .send(Request::Machine(MachineJob::Create {
                image: Box::new(f.row.clone()),
                name: Some("Second".into()),
                start: false,
            }))
            .unwrap();
        let (_, select, _) = machine_done(&w);
        assert!(select.is_some());
        w.send
            .send(Request::Machine(MachineJob::Reset(id.clone())))
            .unwrap();
        let refused = loop {
            match w
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .unwrap()
            {
                Event::Error(error) => break error,
                _ => continue,
            }
        };
        assert!(refused.contains("running"), "{refused}");
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        assert_eq!(
            pc_image::machine::get(&f.store, &id).unwrap().machine.name,
            "DOS 2"
        );
        assert_eq!(pc_image::machine::list(&f.store).unwrap().len(), 2);
        w.shutdown();
    }

    use crate::kept_catalogue::{KeptDisk, RestoredDrive, StoppedDisk, StoppedDrive};

    /// A Start of machine `id`, or of its Gaming-mode copy: the prepared
    /// session, its workspace name, the drives that took back a kept
    /// catalogue and what the Start left out.
    fn start_kept(
        w: &Worker,
        id: &str,
        gaming: bool,
    ) -> (
        pc_image::workspace::PreparedSession,
        String,
        Vec<RestoredDrive>,
        Vec<String>,
    ) {
        w.send
            .send(Request::LaunchMachine(id.into(), gaming))
            .unwrap();
        let mut drives = Vec::new();
        let mut notes = Vec::new();
        loop {
            match until(w, |e| {
                matches!(
                    e,
                    Event::Launched(..) | Event::StartCatalogue(_) | Event::StartNotes(_)
                )
            }) {
                Event::StartCatalogue(restored) => drives = restored,
                Event::StartNotes(left_out) => notes = left_out,
                Event::Launched(transfer, _, name, ..) => {
                    return (transfer.restore().unwrap(), name, drives, notes)
                }
                _ => unreachable!(),
            }
        }
    }

    /// Stop the running session as the window does: its catalogues, then
    /// its finish.
    fn stop_kept(w: &Worker, drives: Vec<StoppedDrive>, outcome: Finish) {
        w.send.send(Request::KeepCatalogue(drives)).unwrap();
        w.send.send(Request::Finish(outcome)).unwrap();
        until(w, |e| matches!(e, Event::Finished));
    }

    /// Drive A:'s catalogue in a prepared session.
    fn drive_a(prepared: &pc_image::workspace::PreparedSession) -> &assembly::FloppyMediaList {
        prepared
            .machine
            .drives
            .iter()
            .find(|d| d.unit == 0 && matches!(d.kind, assembly::DriveKind::Floppy { .. }))
            .and_then(|d| d.media.as_ref())
            .expect("drive A:'s catalogue")
    }

    fn names(list: &assembly::FloppyMediaList) -> Vec<String> {
        list.entries.iter().map(|m| m.name.clone()).collect()
    }

    /// The disks a session started with, as the window hands them over.
    fn started_with(list: &assembly::FloppyMediaList) -> Vec<StoppedDisk> {
        list.entries
            .iter()
            .map(|m| StoppedDisk {
                name: m.name.clone(),
                source: None,
                reference: None,
                image: m.image.clone(),
                sha256: m.expected_sha256,
                write: m.write.clone(),
                format: None,
                started_with: true,
            })
            .collect()
    }

    /// A 360K image file of this computer, inserted with Browse….
    fn browsed(f: &Fixture, byte: u8) -> (std::path::PathBuf, StoppedDisk) {
        use sha2::Digest as _;
        let path = f.temp.path().join("host/game.img");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = vec![byte; 360 * 1024];
        fs::write(&path, &bytes).unwrap();
        let disk = StoppedDisk {
            name: "game.img".into(),
            source: None,
            reference: None,
            image: assembly::ImageSource::File(path.clone()),
            sha256: Some(sha2::Sha256::digest(&bytes).into()),
            write: assembly::WriteMode::ReadOnly,
            format: Some(assembly::FloppyGeometry::DD_360K),
            started_with: false,
        };
        (path, disk)
    }

    /// The library source of the set's one disk, as the window names it.
    fn set_source(f: &Fixture, set: &Descriptor) -> image_store::media_set::CatalogueSource {
        let resolved = f
            .store
            .resolve_descriptor(
                set,
                &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
            )
            .unwrap();
        let config = image_store::media_set::Config::decode(&resolved.config).unwrap();
        image_store::media_set::CatalogueSource {
            set: set.digest.clone(),
            medium_id: config.media[0].id.clone(),
            blob: config.media[0].blob.clone(),
        }
    }

    /// A library machine's catalogue comes back at its next Start as it
    /// was at Stop: a set's disk a pending insert put in, the image's own
    /// disks and an image file Browse… inserted keep their order, names and
    /// write settings, and the disk that was in the drive goes in again.
    /// The image file is the machine's own copy, so a later change of the
    /// file changes nothing; the record lives in the machine's workspace,
    /// never in the machine list, which no Stop or Start rewrites. Choosing
    /// another kept disk for the next Start changes the disk in the drive.
    #[test]
    fn a_machine_takes_back_at_start_the_catalogue_it_had_at_stop() {
        use sha2::Digest as _;
        let f = Fixture::new();
        let set = f.import_set();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        pc_image::machine::set_pending_insert(&f.store, &id, "A:", Some(pending_a(&set))).unwrap();
        let (prepared, _, restored, _) = start_kept(&w, &id, false);
        assert!(restored.is_empty(), "nothing was kept yet");
        w.send.send(Request::Assembled).unwrap();
        let first = drive_a(&prepared).clone();
        drop(prepared);
        let (host, game) = browsed(&f, 0x5a);
        let mut disks = started_with(&first);
        disks[0].source = Some(set_source(&f, &set));
        disks[0].reference = Some("disks-set:r1".into());
        disks.push(game.clone());
        let at = disks.len() - 1;
        let list_file = f.store.root().join("local/machines.toml");
        // The run's last-run stamp lands at Assembled, which the worker has
        // done once it answers the next job; the Stop changes no machine
        // list after it.
        w.send
            .send(Request::Machine(MachineJob::ReadSettings(id.clone())))
            .unwrap();
        until(&w, |e| matches!(e, Event::MachineSettings(..)));
        let machines = fs::read(&list_file).unwrap();
        stop_kept(
            &w,
            vec![StoppedDrive {
                unit: 0,
                disks,
                in_drive: Some(at),
            }],
            Finish::Retain,
        );
        assert_eq!(fs::read(&list_file).unwrap(), machines);
        let workspace = f.store.workspace_path(&id).unwrap();
        let kept = crate::kept_catalogue::read(&workspace).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].in_drive, Some(at));
        assert!(matches!(
            &kept[0].disk[0],
            KeptDisk::Set { journal: true, .. }
        ));
        assert!(kept[0].disk[1..at]
            .iter()
            .all(|disk| matches!(disk, KeptDisk::Own { .. })));
        let sha = crate::created_media::hex(&game.sha256.unwrap());
        assert_eq!(
            kept[0].disk[at],
            KeptDisk::Image {
                name: "game.img".into(),
                sha256: sha.clone(),
                journal: false,
                journal_file: None,
                format: Some("360K".into()),
            }
        );
        let copy = workspace.join(format!("media/created/{sha}.img"));
        assert_eq!(fs::read(&copy).unwrap(), fs::read(&host).unwrap());
        fs::write(&host, vec![0x11; 360 * 1024]).unwrap();
        // The next Start: no pending insert, the kept catalogue alone.
        let (prepared, _, restored, notes) = start_kept(&w, &id, false);
        assert!(notes.is_empty(), "{notes:?}");
        let list = drive_a(&prepared);
        let mut expected = names(&first);
        expected.push("game.img".into());
        assert_eq!(names(list), expected);
        assert_eq!(list.entries[..at], first.entries[..]);
        let last = &list.entries[at];
        assert_eq!(last.image, assembly::ImageSource::File(copy.clone()));
        assert_eq!(last.write, assembly::WriteMode::ReadOnly);
        assert_eq!(
            last.expected_sha256,
            Some(sha2::Sha256::digest(vec![0x5a; 360 * 1024]).into())
        );
        assert!(prepared.bases.contains_key(&copy));
        // The browsed disk is in the drive from power-on; the set's disk
        // keeps its library source and the tag it came from.
        assert!(!list.empty);
        assert_eq!(list.inserted, at);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].sets.len(), 1);
        assert_eq!(restored[0].sets[0].index, 0);
        assert_eq!(restored[0].sets[0].source, set_source(&f, &set));
        assert_eq!(restored[0].sets[0].reference, "disks-set:r1");
        drop(prepared);
        stop_kept(&w, Vec::new(), Finish::Retain);
        assert_eq!(fs::read(&list_file).unwrap(), machines, "never assembled");
        // An empty catalogue list from the window keeps nothing.
        assert!(crate::kept_catalogue::read(&workspace).unwrap().is_empty());
        w.shutdown();
    }

    /// Choosing a kept disk for a stopped machine's next Start makes it the
    /// disk in the drive from power-on then.
    #[test]
    fn a_kept_disk_chosen_for_start_goes_in_then() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        let first = drive_a(&prepared).clone();
        drop(prepared);
        let (_, game) = browsed(&f, 0x33);
        let mut disks = started_with(&first);
        disks.push(game);
        stop_kept(
            &w,
            vec![StoppedDrive {
                unit: 0,
                disks,
                in_drive: None,
            }],
            Finish::Retain,
        );
        // The drive was empty at Stop: it is empty at Start.
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        assert!(drive_a(&prepared).empty);
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        for index in [first.entries.len(), 0] {
            w.send
                .send(Request::Machine(MachineJob::SetKeptDisk {
                    id: id.clone(),
                    drive: "A:".into(),
                    index,
                }))
                .unwrap();
            until(&w, |e| matches!(e, Event::MachineNote(_)));
            let (prepared, _, _, _) = start_kept(&w, &id, false);
            assert!(!drive_a(&prepared).empty, "{index}");
            assert_eq!(drive_a(&prepared).inserted, index);
            drop(prepared);
            w.send.send(Request::Finish(Finish::Retain)).unwrap();
            until(&w, |e| matches!(e, Event::Finished));
        }
        w.shutdown();
    }

    /// Disks Create from files… made while the machine ran stay in its
    /// catalogue across a restart: all of a split, in order, with their
    /// journals in the machine's workspace, the one in the drive in again.
    #[test]
    fn a_created_split_stays_in_the_catalogue_across_a_restart() {
        use crate::created_media::{CreateOrder, Place};
        use media::fat12::{volume_label, FloppyFormat, HostItem, ShortName};
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        let first = drive_a(&prepared).clone();
        drop(prepared);
        let game = f.temp.path().join("host/game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("one.dat"), vec![1; 250_000]).unwrap();
        fs::write(game.join("two.dat"), vec![2; 250_000]).unwrap();
        let workspace = f.store.workspace_path(&id).unwrap();
        let media = workspace.join("media");
        w.send
            .send(Request::CreateDisks(
                Box::new(CreateOrder {
                    unit: 0,
                    items: vec![HostItem::read(&game).unwrap()],
                    top: ShortName::parse("GAME"),
                    format: FloppyFormat::Dd360,
                    label: volume_label("GAME").ok(),
                    split: true,
                    journal: true,
                    keep: None,
                    title: "GAME".into(),
                }),
                Place::Folder(media.clone()),
            ))
            .unwrap();
        let Event::DisksCreated(created) = until(&w, |e| matches!(e, Event::DisksCreated(_)))
        else {
            unreachable!()
        };
        let created = created.unwrap();
        assert_eq!(created.disks.len(), 2);
        let mut disks = started_with(&first);
        for entry in crate::created_media::entries(&created, 3) {
            disks.push(StoppedDisk {
                name: entry.medium.name,
                source: None,
                reference: None,
                image: entry.medium.image,
                sha256: entry.medium.expected_sha256,
                write: entry.medium.write,
                format: entry.geometry,
                started_with: false,
            });
        }
        let second = disks.len() - 1;
        stop_kept(
            &w,
            vec![StoppedDrive {
                unit: 0,
                disks,
                in_drive: Some(second),
            }],
            Finish::Retain,
        );
        let (prepared, _, restored, notes) = start_kept(&w, &id, false);
        assert!(notes.is_empty(), "{notes:?}");
        let list = drive_a(&prepared);
        let mut expected = names(&first);
        expected.extend(["GAME 1 of 2".to_owned(), "GAME 2 of 2".to_owned()]);
        assert_eq!(names(list), expected);
        for (medium, (path, digest, _)) in list.entries[second - 1..].iter().zip(&created.disks) {
            assert_eq!(medium.image, assembly::ImageSource::File(path.clone()));
            assert_eq!(medium.expected_sha256, Some(*digest));
            assert!(
                matches!(&medium.write, assembly::WriteMode::Journal { path, .. }
                    if *path == media.join(format!("{}.upfj", crate::created_media::hex(digest)))),
                "{medium:?}"
            );
        }
        assert_eq!((list.empty, list.inserted), (false, second));
        assert!(restored[0].sets.is_empty());
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        w.shutdown();
    }

    /// A pending insert for a drive wins it: its disk leads the catalogue
    /// and is in the drive at Start, as the insert chose it, and the kept
    /// catalogue follows in its order, without the insert's disk a second
    /// time; the other drive keeps its kept catalogue whole.
    #[test]
    fn a_pending_insert_wins_its_drive_and_the_kept_catalogue_follows() {
        let f = Fixture::new();
        let set = f.import_set();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        pc_image::machine::set_pending_insert(&f.store, &id, "A:", Some(pending_a(&set))).unwrap();
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        w.send.send(Request::Assembled).unwrap();
        let first = drive_a(&prepared).clone();
        drop(prepared);
        let (_, game) = browsed(&f, 0x44);
        let mut disks = started_with(&first);
        disks[0].source = Some(set_source(&f, &set));
        disks.push(game);
        let at = disks.len() - 1;
        stop_kept(
            &w,
            vec![StoppedDrive {
                unit: 0,
                disks,
                in_drive: Some(at),
            }],
            Finish::Retain,
        );
        // The same disk again, now write-protected.
        let insert = image_store::machine::PendingInsert {
            journal: false,
            ..pending_a(&set)
        };
        pc_image::machine::set_pending_insert(&f.store, &id, "A:", Some(insert)).unwrap();
        let (prepared, _, restored, notes) = start_kept(&w, &id, false);
        assert!(notes.is_empty(), "{notes:?}");
        let list = drive_a(&prepared);
        assert_eq!(names(list)[..at], names(&first)[..]);
        assert_eq!(names(list)[at], "game.img");
        assert_eq!(list.entries.len(), at + 1, "the set's disk once");
        assert_eq!(list.entries[0].image, first.entries[0].image);
        assert_eq!(list.entries[0].write, assembly::WriteMode::ReadOnly);
        assert!(!list.empty, "the insert's disk is in the drive at power-on");
        assert_eq!(list.inserted, 0);
        assert!(restored[0].sets.is_empty(), "the insert names its own disk");
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        w.shutdown();
    }

    /// Stop the running session as the window does, and the notes the Stop
    /// sends after its finish, up to the answer of the next job.
    fn stop_noted(w: &Worker, id: &str, drives: Vec<StoppedDrive>) -> Vec<String> {
        stop_kept(w, drives, Finish::Retain);
        w.send
            .send(Request::Machine(MachineJob::ReadSettings(id.into())))
            .unwrap();
        let mut notes = Vec::new();
        while let Event::MachineNote(note) = until(w, |e| {
            matches!(e, Event::MachineNote(_) | Event::MachineSettings(..))
        }) {
            notes.push(note);
        }
        notes
    }

    /// A kept catalogue whose record cannot be read, or that a newer
    /// version wrote, never stops a Start: the drive is as the image sets
    /// it and the Start says why. The next Stop leaves a newer version's
    /// record as it is and says so, and replaces a damaged one. A kept disk
    /// whose image is gone stays out with a note, and the others come back.
    #[test]
    fn an_unreadable_kept_catalogue_never_stops_a_start() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        let first = drive_a(&prepared).clone();
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        let workspace = f.store.workspace_path(&id).unwrap();
        let record = workspace.join("media/created/catalogue.toml");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let (_, game) = browsed(&f, 0x66);
        let sha = crate::created_media::hex(&game.sha256.unwrap());
        let mut disks = started_with(&first);
        disks.push(game);
        for (text, newer) in [
            ("schema = 2\n[[drive]]\ndrive = \"A:\"\n", true),
            (
                "schema = 1\n[[drive]]\ndrive = \"A:\"\n[[drive.disk]]\nkind = \"own\"\nname = \"x\"\nlater = 1\n",
                true,
            ),
            ("not a record", false),
        ] {
            fs::write(&record, text).unwrap();
            let (prepared, _, restored, notes) = start_kept(&w, &id, false);
            assert!(restored.is_empty(), "{text}");
            assert_eq!(notes.len(), 1, "{text}: {notes:?}");
            assert!(notes[0].contains("cannot be read"), "{notes:?}");
            assert_eq!(*drive_a(&prepared), first, "{text}");
            drop(prepared);
            let notes = stop_noted(
                &w,
                &id,
                vec![StoppedDrive {
                    unit: 0,
                    disks: disks.clone(),
                    in_drive: Some(0),
                }],
            );
            if newer {
                assert_eq!(fs::read_to_string(&record).unwrap(), text, "kept for its version");
                assert_eq!(notes.len(), 1, "{notes:?}");
                assert!(notes[0].contains("a newer version keeps them"), "{notes:?}");
            } else {
                assert!(notes.is_empty(), "{notes:?}");
                let kept = crate::kept_catalogue::read(&workspace).unwrap();
                assert_eq!(kept[0].disk.len(), disks.len(), "a damaged record is replaced");
            }
        }
        fs::remove_file(workspace.join(format!("media/created/{sha}.img"))).unwrap();
        let (prepared, _, _, notes) = start_kept(&w, &id, false);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].starts_with("game.img left A:'s catalogue: its image is gone"),
            "{notes:?}"
        );
        assert_eq!(*drive_a(&prepared), first);
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        w.shutdown();
    }

    /// Write `value` to sector 0 of `base` through the journal at `path`, as
    /// the guest's writes go.
    fn journal_write(path: &std::path::Path, base: &[u8], value: u8) {
        let mut journal =
            media::JournalMedia::open(path, base, base.len() as u64, Default::default()).unwrap();
        journal.write_batch(&[(0, [value; 512])]);
        journal.finish().unwrap();
    }

    /// A journal Insert put beside an image file of this computer becomes
    /// the machine's own at Stop, copied into its media folder, so Gaming
    /// mode and Duplicate write their own copies of it: the machine's
    /// journal and the one beside the file stay as they were, also when the
    /// Gaming-mode copy is discarded.
    #[test]
    fn copies_of_a_machine_never_share_a_journal_beside_an_image_file() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        let first = drive_a(&prepared).clone();
        drop(prepared);
        let (host, mut game) = browsed(&f, 0x44);
        let beside = f.temp.path().join("host/game.img.upfj");
        journal_write(&beside, &fs::read(&host).unwrap(), 0x45);
        game.write = assembly::WriteMode::journal(&beside);
        let sha = crate::created_media::hex(&game.sha256.unwrap());
        let mut disks = started_with(&first);
        disks.push(game);
        let at = disks.len() - 1;
        stop_kept(
            &w,
            vec![StoppedDrive {
                unit: 0,
                disks,
                in_drive: Some(at),
            }],
            Finish::Retain,
        );
        let workspace = f.store.workspace_path(&id).unwrap();
        let kept = crate::kept_catalogue::read(&workspace).unwrap();
        assert!(matches!(
            &kept[0].disk[at],
            KeptDisk::Image {
                journal: true,
                journal_file: None,
                ..
            }
        ));
        let own = workspace.join(format!("media/{sha}.upfj"));
        let beside_bytes = fs::read(&beside).unwrap();
        assert_eq!(
            fs::read(&own).unwrap(),
            beside_bytes,
            "the machine's own copy"
        );
        let journals = |w: &Worker, copy: &str, gaming: bool| {
            let (prepared, name, _, _) = start_kept(w, copy, gaming);
            let media = f.store.workspace_path(&name).unwrap().join("media");
            let last = drive_a(&prepared).entries.last().unwrap().clone();
            assert!(
                matches!(&last.write, assembly::WriteMode::Journal { path, .. }
                    if *path == media.join(format!("{sha}.upfj"))),
                "{last:?}"
            );
            // The copy's guest writes every journaled disk.
            f.change(&prepared, 0x99);
        };
        journals(&w, &id, true);
        stop_kept(&w, Vec::new(), Finish::Discard);
        assert_eq!(fs::read(&own).unwrap(), beside_bytes);
        assert_eq!(fs::read(&beside).unwrap(), beside_bytes);
        w.send
            .send(Request::Machine(MachineJob::Duplicate(
                id.clone(),
                "DOS copy".into(),
            )))
            .unwrap();
        let (_, select, _) = machine_done(&w);
        let (copy, _) = select.unwrap();
        journals(&w, &copy, false);
        stop_kept(&w, Vec::new(), Finish::Retain);
        assert_eq!(fs::read(&own).unwrap(), beside_bytes);
        assert_eq!(fs::read(&beside).unwrap(), beside_bytes);
        w.shutdown();
    }

    /// Import a one-disk 360K set under the revision tag `tag`, as the
    /// command line does; its release alias follows it.
    fn import_tagged(f: &Fixture, tag: &str) -> Descriptor {
        use pc_image::media_create;
        let disk = f.temp.path().join("tagged-disk.img");
        let args: Vec<std::ffi::OsString> = vec![
            "floppy".into(),
            "--type".into(),
            "360K".into(),
            "--format".into(),
            disk.clone().into_os_string(),
        ];
        media_create::execute(media_create::parse(&args).unwrap(), &mut std::io::sink()).unwrap();
        crate::library_test_support::import_media_set(
            &f.store,
            tag,
            "Tagged",
            &[crate::library_test_support::Medium::new(&disk)],
            &[],
        );
        f.store.tag_descriptor(tag).unwrap()
    }

    /// A set's disk keeps the tag it came from for its tooltip, unless that
    /// is a release alias, which moves: then, and when the window knows no
    /// tag, the set's revision tag.
    #[test]
    fn a_set_disk_keeps_its_tag_and_never_a_release_alias() {
        let f = Fixture::new();
        let set = import_tagged(&f, "games:1.0-r1");
        assert_eq!(
            f.store.tag_descriptor("games:1.0").unwrap().digest,
            set.digest,
            "the release alias names the set"
        );
        let workspace = f.temp.path().join("workspace");
        let source = set_source(&f, &set);
        for (reference, kept) in [
            (Some("games:1.0-r1"), "games:1.0-r1"),
            (Some("games:1.0"), "games:1.0-r1"),
            (None, "games:1.0-r1"),
        ] {
            let disk = StoppedDisk {
                name: "Disk 1".into(),
                source: Some(source.clone()),
                reference: reference.map(str::to_owned),
                image: assembly::ImageSource::File("/store/disk.img".into()),
                sha256: None,
                write: assembly::WriteMode::ReadOnly,
                format: None,
                started_with: false,
            };
            crate::kept_catalogue::keep(
                &f.store,
                &workspace,
                vec![StoppedDrive {
                    unit: 0,
                    disks: vec![disk],
                    in_drive: Some(0),
                }],
            )
            .unwrap();
            let record = crate::kept_catalogue::read(&workspace).unwrap();
            assert!(
                matches!(&record[0].disk[0], KeptDisk::Set { reference, .. } if reference == kept),
                "{reference:?}: {record:?}"
            );
        }
    }

    /// An image file that changed since it went in is not kept: the Stop
    /// says so once the session has finished, and keeps the other disks,
    /// here the image's own, which leave no record.
    #[test]
    fn a_changed_image_file_is_not_kept_and_the_stop_says_so() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        let first = drive_a(&prepared).clone();
        drop(prepared);
        let (host, game) = browsed(&f, 0x22);
        fs::write(&host, vec![0x23; 360 * 1024]).unwrap();
        let mut disks = started_with(&first);
        disks.push(game);
        w.send
            .send(Request::KeepCatalogue(vec![StoppedDrive {
                unit: 0,
                disks,
                in_drive: Some(0),
            }]))
            .unwrap();
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        // The notes after the finish, up to the answer of the next job.
        w.send
            .send(Request::Machine(MachineJob::ReadSettings(id.clone())))
            .unwrap();
        let mut notes = Vec::new();
        while let Event::MachineNote(note) = until(&w, |e| {
            matches!(e, Event::MachineNote(_) | Event::MachineSettings(..))
        }) {
            notes.push(note);
        }
        assert_eq!(
            notes,
            ["game.img left A:'s catalogue at Stop: its image file changed since it went in."]
        );
        // What is left is the image's own catalogue, which needs no record.
        let workspace = f.store.workspace_path(&id).unwrap();
        assert!(crate::kept_catalogue::read(&workspace).unwrap().is_empty());
        assert!(!workspace.join("media/created/catalogue.toml").exists());
        w.shutdown();
    }

    /// The kept catalogue follows the machine's media: Duplicate copies it
    /// with the machine's own copy of an image file, Gaming mode plays with
    /// it from the copy, Save as new image… keeps it and Reset to image…
    /// drops it.
    #[test]
    fn duplicate_and_gaming_mode_carry_the_kept_catalogue_and_reset_drops_it() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        let (prepared, _, _, _) = start_kept(&w, &id, false);
        let first = drive_a(&prepared).clone();
        drop(prepared);
        let (_, game) = browsed(&f, 0x77);
        let sha = crate::created_media::hex(&game.sha256.unwrap());
        let mut disks = started_with(&first);
        disks.push(game);
        let at = disks.len() - 1;
        stop_kept(
            &w,
            vec![StoppedDrive {
                unit: 0,
                disks,
                in_drive: Some(at),
            }],
            Finish::Retain,
        );
        let workspace = f.store.workspace_path(&id).unwrap();
        let kept = crate::kept_catalogue::read(&workspace).unwrap();
        let ends_with_game = |name: &str, prepared: &pc_image::workspace::PreparedSession| {
            let last = drive_a(prepared).entries.last().unwrap().clone();
            let copy = f
                .store
                .workspace_path(name)
                .unwrap()
                .join(format!("media/created/{sha}.img"));
            last.name == "game.img" && last.image == assembly::ImageSource::File(copy)
        };
        // Gaming mode plays the machine's catalogue, from the copy.
        let (prepared, name, _, _) = start_kept(&w, &id, true);
        assert!(ends_with_game(&name, &prepared));
        assert_eq!(drive_a(&prepared).inserted, at);
        drop(prepared);
        stop_kept(&w, Vec::new(), Finish::Discard);
        assert_eq!(crate::kept_catalogue::read(&workspace).unwrap(), kept);
        // Duplicate copies it, with the machine's own copy of the file.
        w.send
            .send(Request::Machine(MachineJob::Duplicate(
                id.clone(),
                "DOS copy".into(),
            )))
            .unwrap();
        let (_, select, _) = machine_done(&w);
        let (copy, _) = select.unwrap();
        let copied = f.store.workspace_path(&copy).unwrap();
        assert_eq!(crate::kept_catalogue::read(&copied).unwrap(), kept);
        let (prepared, name, _, _) = start_kept(&w, &copy, false);
        assert_eq!(name, copy);
        assert!(ends_with_game(&copy, &prepared));
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        // Save as new image… with fresh journals keeps it, as it keeps
        // created disks: the new image's own disks come back by name.
        w.send
            .send(Request::Machine(MachineJob::Commit {
                id: id.clone(),
                target: target("saver:1.0.0", false),
                keep: false,
            }))
            .unwrap();
        let (message, _, _) = machine_done(&w);
        assert!(message.contains("fresh, empty journals"), "{message}");
        assert_eq!(crate::kept_catalogue::read(&workspace).unwrap(), kept);
        let (prepared, _, _, notes) = start_kept(&w, &id, false);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(names(drive_a(&prepared))[..at], names(&first)[..]);
        assert!(ends_with_game(&id, &prepared));
        assert_eq!(drive_a(&prepared).inserted, at);
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        // Reset to image… drops it.
        w.send
            .send(Request::Machine(MachineJob::Reset(id.clone())))
            .unwrap();
        machine_done(&w);
        assert!(crate::kept_catalogue::read(&workspace).unwrap().is_empty());
        let (prepared, _, restored, _) = start_kept(&w, &id, false);
        assert!(restored.is_empty());
        assert_eq!(names(drive_a(&prepared)), names(&first));
        drop(prepared);
        w.send.send(Request::Finish(Finish::Retain)).unwrap();
        until(&w, |e| matches!(e, Event::Finished));
        w.shutdown();
    }
}
