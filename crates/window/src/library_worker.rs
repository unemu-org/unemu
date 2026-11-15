// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host image-store jobs and recoverable desktop sessions; machine-images §§3–4, 7.
//! Guest execution stays on the presenter thread. Durable records precede
//! preparation so forced exit and interrupted publication never hide a session.
use crate::library_view::{self, ImageRow, Query};
use image_store::{summary::SummaryCache, workspace::Workspace, Descriptor, Digest, Store};
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
    #[serde(skip)]
    pub name: String,
}
impl Recovery {
    pub fn row(&self) -> ImageRow {
        ImageRow {
            reference: self.reference.clone(),
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
    Publish(ImageRow, String, String, bool),
    Discard(String),
    /// Scan a picked ROM folder, reusing the persistent ROM hash index.
    ScanFirmware(
        PathBuf,
        Option<PathBuf>,
        assembly::firmware_discovery::DiscoveryControl,
    ),
    /// Publish the named catalogue sets of the last scan.
    ImportFirmware(Vec<String>),
    Shutdown,
}
pub(super) enum Event {
    Snapshot(Arc<library_view::Snapshot>),
    Checked(Digest, Result<pc_image::inspection::ImageDetails>),
    Launched(
        Box<pc_image::workspace::SessionTransfer>,
        Box<ImageRow>,
        String,
        WorkspaceKind,
    ),
    Finished,
    Layers(String, Result<pc_image::inspection::ImageDetails>),
    Done(String),
    Deleted(String),
    Published(Box<ImageRow>),
    Recoveries(Vec<Recovery>),
    Error(String),
    FirmwareError(String),
    FirmwareScanned(Box<library_view::firmware::ScanReport>),
    FirmwareImported(Vec<library_view::firmware::Outcome>),
    /// A scan or import job failed; only the import dialog shows it.
    FirmwareFailed(String),
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
    fn publish(&self, owner: &Workspace<'_>, tag: &str, flatten: bool) -> Result<Descriptor>;
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
    fn publish(&self, owner: &Workspace<'_>, tag: &str, flatten: bool) -> Result<Descriptor> {
        pc_image::workspace::publish_with_options(owner, tag, None, flatten, |_| Ok(()))
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
    fn publish(&self, w: &Workspace<'_>, tag: &str, flatten: bool) -> Result<Descriptor> {
        pc_image::test_support::with_synthetic_firmware(|| PcBackend.publish(w, tag, flatten))
    }
    fn firmware_catalogue(&self) -> &'static [assembly::FirmwareEntry] {
        library_view::firmware::tests::synthetic_catalogue()
    }
}
#[cfg(test)]
impl Worker {
    pub(crate) fn synthetic(root: PathBuf) -> Self {
        Self::spawn_backend(root, egui::Context::default(), SyntheticBackend)
    }
    /// A worker whose thread dies inside a firmware job after `survive`
    /// successful catalogue lookups: 0 dies in the scan, 1 in the import.
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
    fn publish(&self, _: &Workspace<'_>, _: &str, _: bool) -> Result<Descriptor> {
        Err("not used by firmware tests".into())
    }
    fn firmware_catalogue(&self) -> &'static [assembly::FirmwareEntry] {
        if self.calls.fetch_add(1, Ordering::Relaxed) >= self.survive {
            panic!("injected worker crash in a firmware job");
        }
        library_view::firmware::tests::synthetic_catalogue()
    }
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
                            send(Event::Checked(row.descriptor.digest.clone(), result));
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
                        send(Event::Layers(
                            name.clone(),
                            backend
                                .check(&store, &row, Some(&name), &|| false)
                                .and_then(|details| {
                                    storage_details(&store, &cache, &row, details, &|| false)
                                }),
                        ));
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
                            name: name.clone(),
                        };
                        let prepared = (|| -> Result<_> {
                            record.write(&owner)?;
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
                        send(Event::Launched(
                            Box::new(transfer),
                            Box::new(row),
                            name,
                            kind,
                        ));
                        let outcome = loop {
                            match requests.recv() {
                                Ok(Request::Finish(outcome)) => break outcome,
                                Ok(Request::Shutdown) | Err(_) => {
                                    record.recovery = true;
                                    record.write(&owner)?;
                                    drop(owner);
                                    return Ok(false);
                                }
                                Ok(Request::Layers(row, name)) => send(Event::Layers(
                                    name.clone(),
                                    backend
                                        .check(&store, &row, Some(&name), &|| false)
                                        .and_then(|details| {
                                            storage_details(&store, &cache, &row, details, &|| {
                                                false
                                            })
                                        }),
                                )),
                                _ => (),
                            }
                        };
                        if outcome == Finish::Discard {
                            owner.discard_and_remove().map_err(|e| e.to_string())?;
                        } else {
                            record.recovery = matches!(outcome, Finish::Deferred | Finish::Failed);
                            record.write(&owner)?;
                            drop(owner);
                        }
                        send(Event::Finished);
                        send(Event::Recoveries(recoveries(&store)?));
                    }
                    Request::Publish(row, name, tag, flatten) => {
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
                        let descriptor = backend.publish(&owner, &tag, flatten)?;
                        if record.as_ref().is_some_and(|r| r.kind.disposable()) {
                            owner.discard_and_remove().map_err(|e| e.to_string())?;
                        } else if let Some(mut record) = record {
                            record.image = descriptor.clone();
                            record.reference = tag.clone();
                            record.recovery = false;
                            record.write(&owner)?;
                        }
                        send(Event::Published(Box::new(ImageRow {
                            reference: tag,
                            descriptor,
                            summary: None,
                            failure: None,
                        })));
                        send(Event::Recoveries(recoveries(&store)?));
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
                    Request::Finish(_) => (),
                }
                Ok(true)
            };
            match operation() {
                Ok(false) => break,
                Err(e) => send(Event::Error(e)),
                Ok(true) => (),
            }
            cache.refresh(&store).map_err(|e| e.to_string())?;
            send(Event::Snapshot(Arc::new(library_view::snapshot(
                &cache, &query,
            ))));
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
                .recv_timeout(Duration::from_secs(20))
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
        let Event::Launched(transfer, _, name, actual) =
            until(w, |e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        assert_eq!(actual, kind);
        (transfer.restore().unwrap(), name)
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
            let crate::layers_dialog::Action::Publish(workspace, tag, flatten) =
                crate::layers_dialog::tests::reviewed_publication(
                    &name,
                    kind,
                    details,
                    "test:kept",
                    kind == WorkspaceKind::Regular,
                )
            else {
                panic!()
            };
            w.send
                .send(Request::Publish(f.row.clone(), workspace, tag, flatten))
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
    fn failed_keep_leaves_workspace_for_next_startup() {
        let f = Fixture::new();
        let mut w = worker(&f);
        let (_, name) = launch(&w, &f.row, WorkspaceKind::Disposable);
        finish(&w, Finish::Deferred);
        w.send
            .send(Request::Publish(
                f.row.clone(),
                name.clone(),
                "invalid tag".into(),
                false,
            ))
            .unwrap();
        loop {
            if matches!(
                w.receive.recv_timeout(Duration::from_secs(20)).unwrap(),
                Event::Error(_)
            ) {
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
            })
        }
        fn publish(&self, _: &Workspace<'_>, _: &str, _: bool) -> Result<Descriptor> {
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
                    w.receive.recv_timeout(Duration::from_secs(20)).unwrap(),
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
        fn publish(&self, w: &Workspace<'_>, tag: &str, flatten: bool) -> Result<Descriptor> {
            SyntheticBackend.publish(w, tag, flatten)
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
                .publish(&owner, &format!("test:depth-{depth}"), false)
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
        let Event::Checked(_, result) = until(&w, |e| matches!(e, Event::Checked(..))) else {
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
        let Event::Layers(_, result) = until(&w, |e| matches!(e, Event::Layers(..))) else {
            unreachable!()
        };
        let details = result.unwrap();
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
                match w.receive.recv_timeout(Duration::from_secs(20)).unwrap() {
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
            match w.receive.recv_timeout(Duration::from_secs(20)) {
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
}
