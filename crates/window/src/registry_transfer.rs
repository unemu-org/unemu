// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless model of Image › Push… and Image › Pull…;
//! `backlog/docs/design/machine-images.md` §5.1 and the approved
//! registry-accounts mock-up.
//!
//! Push… starts from a dry run that the library worker builds with the
//! command line's own plan ([`cli::push_plan`]): every blob of the chosen
//! local image (machine image, media set or firmware package) with its
//! effective redistribution class and where that class was recorded, this
//! image, another local tag or the firmware catalogue. The destination
//! resolves Docker-style against the default registry while the user types,
//! and the guard's verdict follows at once from the same rules push applies
//! again before it sends anything; a refusal has no override. A tag that is
//! not a version moves only after the user confirmed the old and the new
//! digest, and only away from the digest that was shown. Pull… resolves its
//! reference the same way; the registry crate continues an interrupted blob by
//! range and writes the local tag only after the complete closure is verified.
//!
//! Every registry request runs on the library worker thread, never on the
//! frame loop: the dialogs only build jobs, read the progress the worker
//! shares with them and take its final event. The registries list comes from
//! the shared preferences file and secrets from the account store, through
//! the command line's own client ([`cli::registry_client`]); no secret ever
//! reaches a dialog. Cancel sets a flag the transfer checks before each blob,
//! between the chunks of a large upload and before it publishes, so a
//! cancelled push never publishes its manifest and a cancelled pull never
//! writes its local tag.

use crate::registries_model::{Check, Mark};
use image_registry::{
    config::{self, RegistryConfig, Scope},
    credential_store::CredentialStore,
    credentials::EnvCredentials,
    guard::{self, ClosureEntry, MarkSource, Redistribution, Refusal},
    pull::{self, PullOutcome, PullReport},
    push::{self, BlobOutcome, PushOptions, PushProgress, PushReport, TagOutcome},
    reference::{is_version_tag, OFFICIAL_NAMESPACE},
    Error, Reference, DEFAULT_REGISTRY,
};
use image_store::{media_set, media_type as mt, ConfigValidator, Descriptor, Digest, Store};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

const NO_OVERRIDE: &str = "There is no override for this rule.";
const PUSH_CANCELLED: &str = "Cancelled. The manifest was not published, so the registry keeps its old tag. Accepted chunks are kept, and the next push resumes them.";
const PULL_CANCELLED: &str = "Cancelled. The local tag was not set. Blobs already fetched stay in the store, and the next pull fetches only the rest.";
const PULL_NOT_STARTED: &str = "Cancelled before it started; nothing was fetched.";
const STALE: &str = "The local tag now names another image; review the dry run, then push again.";

/// Where push and pull find logins: the shared preferences file and the
/// credential store the shell detected, as the command line uses them.
#[derive(Clone)]
pub(crate) struct Accounts {
    pub path: Option<PathBuf>,
    pub store: Arc<dyn CredentialStore>,
    pub env: EnvCredentials,
    /// `UNEMU_REGISTRY`, which replaces the preferences' default registry as
    /// it does on the command line.
    pub default_override: Option<String>,
}

impl Accounts {
    /// The registries list as preferences hold it now.
    fn config(&self) -> Result<RegistryConfig, String> {
        match &self.path {
            Some(path) => config::read_preferences(path)
                .and_then(|table| RegistryConfig::from_table(&table))
                .map_err(|e| format!("Preferences: {e}")),
            None => Ok(RegistryConfig::default()),
        }
    }

    /// The registries list and the host short names resolve to. An invalid
    /// default registry is an error, as on the command line.
    pub(crate) fn settings(&self) -> Result<(RegistryConfig, String), String> {
        let config = self.config()?;
        let host = config
            .default_host(None, self.default_override.as_deref())
            .map_err(|e| format!("Default registry: {e}"))?;
        Ok((config, host))
    }
}

/// Job ids are unique across dialogs, so a late event of a closed dialog
/// never changes a newer one.
pub(crate) fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Progress the worker writes while a transfer runs and the dialog reads each
/// frame. Writing never waits for the frame loop, so a slow frame never
/// stalls a transfer; only the final event goes through the worker's queue.
#[derive(Debug, Default)]
pub(crate) struct Live<T>(Arc<Mutex<T>>);

impl<T> Clone for Live<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Live<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(value)))
    }

    pub fn update(&self, change: impl FnOnce(&mut T)) {
        change(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()));
    }

    pub fn read<R>(&self, look: impl FnOnce(&T) -> R) -> R {
        look(&self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Work for the library worker.
pub(crate) enum TransferRequest {
    /// List the local images and, for `local_tag`, build the dry run.
    Plan {
        id: u64,
        local_tag: Option<String>,
    },
    Push(Box<PushJob>),
    Pull(Box<PullJob>),
    /// File › Get templates…: list the official templates of the default
    /// registry until `cancel` is set.
    Templates {
        id: u64,
        accounts: Box<Accounts>,
        cancel: Arc<AtomicBool>,
    },
}

pub(crate) struct PushJob {
    pub id: u64,
    pub local_tag: String,
    /// The image the user reviewed; a tag that names another one by now is
    /// not pushed.
    pub image: Digest,
    /// The reference the dialog showed; the worker never resolves it again.
    pub target: Reference,
    /// The digest the user confirmed the registry's tag names, when moving it.
    pub move_from: Option<Digest>,
    pub accounts: Accounts,
    pub cancel: Arc<AtomicBool>,
    pub live: Live<Vec<BlobRow>>,
}

pub(crate) struct PullJob {
    pub id: u64,
    pub source: Reference,
    pub local_tag: String,
    pub accounts: Accounts,
    pub cancel: Arc<AtomicBool>,
    pub live: Live<PullLive>,
}

/// The final answers of the worker; progress goes through [`Live`].
pub(crate) enum TransferEvent {
    Planned {
        id: u64,
        result: Result<Box<Planned>, String>,
    },
    Pushed {
        id: u64,
        result: Result<PushReport, PushFailure>,
    },
    Pulled {
        id: u64,
        result: Result<PullReport, PullFailure>,
    },
    Templates {
        id: u64,
        result: Result<Vec<crate::templates::TemplateRow>, crate::templates::ListingFailure>,
    },
}

/// A local tag the Push… dialog can choose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalImage {
    pub tag: String,
    pub digest: Digest,
    pub kind: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct Planned {
    pub images: Vec<LocalImage>,
    pub plan: Option<PlanData>,
}

/// The dry run of one local image.
#[derive(Clone, Debug)]
pub(crate) struct PlanData {
    pub local_tag: String,
    pub image: Descriptor,
    pub closure: Vec<ClosureEntry>,
    /// The image is an installation media set, whose tags other than aliases
    /// never move on the registry.
    pub media_set: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PushFailure {
    /// The guard refused the destination.
    Refused(String),
    /// The registry's tag names another image; moving it needs confirmation.
    Move {
        tag: String,
        current: Digest,
        new: Digest,
    },
    /// The local tag names another image than the one reviewed.
    Stale,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PullFailure {
    Cancelled,
    /// Cancelled while it waited behind another job, before any request.
    NotStarted,
    Failed(String),
}

impl PullFailure {
    pub fn message(&self) -> &str {
        match self {
            Self::Cancelled => PULL_CANCELLED,
            Self::NotStarted => PULL_NOT_STARTED,
            Self::Failed(message) => message,
        }
    }
}

/// Run one request on the library worker thread; every registry request of
/// the dialogs happens here. `catalogue` is the firmware catalogue whose
/// proprietary images restrict a push; `repaint` asks for a frame after
/// progress changed.
pub(crate) fn run(
    store: &Store,
    request: TransferRequest,
    providers: &[&dyn ConfigValidator],
    catalogue: &[assembly::FirmwareEntry],
    send: &(dyn Fn(TransferEvent) + Sync),
    repaint: &(dyn Fn() + Sync),
) {
    match request {
        TransferRequest::Plan { id, local_tag } => send(TransferEvent::Planned {
            id,
            result: plan(store, local_tag.as_deref(), catalogue).map(Box::new),
        }),
        TransferRequest::Push(job) => {
            let live = job.live.clone();
            let result = run_push(store, &job, providers, catalogue, &mut |event| {
                live.update(|rows| record_push(rows, event));
                repaint();
            });
            send(TransferEvent::Pushed { id: job.id, result });
        }
        TransferRequest::Pull(job) => {
            let live = job.live.clone();
            let result = run_pull(store, &job, providers, &|event| {
                live.update(|state| state.record(event));
                repaint();
            });
            send(TransferEvent::Pulled { id: job.id, result });
        }
        TransferRequest::Templates {
            id,
            accounts,
            cancel,
        } => send(TransferEvent::Templates {
            id,
            result: crate::templates::list(store, &accounts, &cancel),
        }),
    }
}

fn record_push(rows: &mut [BlobRow], event: PushProgress<'_>) {
    let (digest, sent, outcome) = match event {
        PushProgress::Blob(blob, outcome) => (&blob.digest, blob.size, Some(outcome)),
        PushProgress::Chunk { blob, sent } => (&blob.digest, sent, None),
    };
    for row in rows.iter_mut().filter(|row| &row.digest == digest) {
        row.sent = sent;
        if outcome.is_some() {
            row.outcome = outcome;
        }
    }
}

/// The dry-run target: only the verdict depends on the target, and the
/// dialog decides that itself as the destination changes; push decides again.
pub(crate) fn dry_run_target() -> Reference {
    Reference {
        host: DEFAULT_REGISTRY.into(),
        repository: format!("{OFFICIAL_NAMESPACE}/dry-run"),
        tag: Some("dry-run".into()),
        digest: None,
    }
}

fn plan(
    store: &Store,
    local_tag: Option<&str>,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Planned, String> {
    let images = local_images(store)?;
    let plan = match local_tag {
        Some(tag) => {
            let plan = cli::push_plan(store, tag, dry_run_target(), None, catalogue)
                .map_err(|e| e.to_string())?;
            Some(PlanData {
                local_tag: tag.into(),
                image: plan.image().clone(),
                closure: plan.closure().to_vec(),
                media_set: plan.manifest().artifact_type() == mt::MEDIA_SET,
            })
        }
        None => None,
    };
    Ok(Planned { images, plan })
}

fn local_images(store: &Store) -> Result<Vec<LocalImage>, String> {
    let mut images = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store
            .list_tags(after.as_deref(), 1000)
            .map_err(|e| e.to_string())?;
        images.extend(page.tags.into_iter().map(|tag| LocalImage {
            kind: match tag.manifest.artifact_type.as_deref() {
                Some(mt::MEDIA_SET) => "media set",
                Some(mt::FIRMWARE) => "firmware package",
                _ => "machine image",
            },
            tag: tag.name,
            digest: tag.manifest.digest,
        }));
        match page.next_after {
            Some(next) => after = Some(next),
            None => return Ok(images),
        }
    }
}

fn run_push(
    store: &Store,
    job: &PushJob,
    providers: &[&dyn ConfigValidator],
    catalogue: &[assembly::FirmwareEntry],
    progress: &mut dyn FnMut(PushProgress<'_>),
) -> Result<PushReport, PushFailure> {
    // Preferences are read again: a registry un-marked private since the
    // dialog opened must not receive what it may no longer hold.
    let config = job.accounts.config().map_err(PushFailure::Failed)?;
    let entry = config.entry(&job.target.host);
    let plan = cli::push_plan(
        store,
        &job.local_tag,
        job.target.clone(),
        entry.as_ref(),
        catalogue,
    )
    .map_err(|e| push_failure(e, &[]))?;
    if plan.image().digest != job.image {
        return Err(PushFailure::Stale);
    }
    if let Err(refusal) = plan.verdict() {
        return Err(PushFailure::Refused(refusal_text(refusal, plan.closure())));
    }
    let client = cli::registry_client(
        &config,
        &job.target.host,
        &job.accounts.store,
        &job.accounts.env,
    )
    .map_err(|e| push_failure(e, &[]))?;
    let options = PushOptions {
        allow_move: job.move_from.is_some(),
        move_from: job.move_from.clone(),
        ..PushOptions::default()
    };
    let cancel = &job.cancel;
    push::push_cancellable(
        store,
        &plan,
        providers,
        &client,
        &options,
        progress,
        &|| cancel.load(Ordering::SeqCst),
    )
    .map_err(|e| push_failure(e, plan.closure()))
}

fn run_pull(
    store: &Store,
    job: &PullJob,
    providers: &[&dyn ConfigValidator],
    progress: &(dyn Fn(pull::PullProgress<'_>) + Sync),
) -> Result<PullReport, PullFailure> {
    // A pull queued behind others may be cancelled before its turn; it then
    // contacts no registry at all.
    if job.cancel.load(Ordering::SeqCst) {
        return Err(PullFailure::NotStarted);
    }
    let config = job.accounts.config().map_err(PullFailure::Failed)?;
    let client = cli::registry_client(
        &config,
        &job.source.host,
        &job.accounts.store,
        &job.accounts.env,
    )
    .map_err(pull_failure)?;
    let cancel = &job.cancel;
    pull::pull_cancellable(
        store,
        &client,
        &job.source,
        &job.local_tag,
        providers,
        progress,
        &|| cancel.load(Ordering::SeqCst),
    )
    .map_err(pull_failure)
}

/// Failures the push and pull dialogs share. Registry errors never carry a
/// secret, so their text may be shown.
pub(crate) fn common_failure(error: &Error) -> Option<String> {
    Some(match error {
        Error::AuthRequired { host } => {
            format!("{host} asks for a login. Log in under Tools › Preferences › Registries, then try again.")
        }
        Error::Unauthorized { host } => format!(
            "{host} refused the stored login (401). Log in again under Tools › Preferences › Registries."
        ),
        Error::Forbidden { host, path } => {
            format!("{host} denied this account access to {path}.")
        }
        Error::UnsupportedAuth(_) => {
            "The registry asks for a token service; only Basic authentication is implemented."
                .into()
        }
        Error::Credentials(message) => format!("The credential store did not answer: {message}"),
        _ => return None,
    })
}

fn push_failure(error: Error, closure: &[ClosureEntry]) -> PushFailure {
    if let Some(text) = common_failure(&error) {
        return PushFailure::Failed(text);
    }
    match error {
        Error::Cancelled => PushFailure::Cancelled,
        // The local tag moved while the plan was built.
        Error::Store(image_store::Error::Conflict { .. }) => PushFailure::Stale,
        Error::Refused(refusal) => PushFailure::Refused(refusal_text(&refusal, closure)),
        Error::TagMoveNeedsConfirmation { tag, current, new } => {
            PushFailure::Move { tag, current, new }
        }
        Error::ImmutableTag { tag, current, new } => PushFailure::Failed(format!(
            "Version tag {tag} already names {current} on the registry, and this image is {new}. Version tags never move: push this image under a new version."
        )),
        Error::ImmutableMediaSetTag { tag, current, new } => PushFailure::Failed(format!(
            "Tag {tag} already names {current} on the registry, and this image is {new}. Tags of installation media sets never move, not even with confirmation: push a changed set under a new revision tag such as {}.",
            media_set::next_revision(&tag)
        )),
        Error::RemoteTagChanged { tag, .. } => PushFailure::Failed(format!(
            "Tag {tag} changed on the registry during the push, so nothing was published. Push again to see the image it names now."
        )),
        Error::Transport(message) => PushFailure::Failed(format!(
            "Network: {message}. Accepted chunks are kept, and the next push resumes them."
        )),
        other => PushFailure::Failed(other.to_string()),
    }
}

fn pull_failure(error: Error) -> PullFailure {
    if let Some(text) = common_failure(&error) {
        return PullFailure::Failed(text);
    }
    PullFailure::Failed(match error {
        Error::Cancelled => return PullFailure::Cancelled,
        Error::LocalTagConflict { tag, current, .. } => format!(
            "Local tag {tag} already names another image ({current}). Pull under another local tag."
        ),
        Error::NotFound(reference) => format!("{reference} is not on the registry."),
        Error::DigestMismatch { expected, actual } => format!(
            "The registry sent bytes that do not match {expected} (they hash to {actual}); nothing was tagged."
        ),
        Error::Transport(message) => format!(
            "Network: {message}. Blobs already fetched stay in the store, and the next pull fetches only the rest."
        ),
        other => other.to_string(),
    })
}

/// "HOST is configured; NAMESPACE/ is marked private", as Push… and Pull…
/// name the registry entry and the namespace a reference falls in.
fn configured_line(host: &str, default_host: &str, scope: &Scope) -> String {
    format!(
        "{host} is configured{}; {scope}",
        if host == default_host {
            " (default registry)"
        } else {
            ""
        }
    )
}

/// The guard's refusal as the dialog says it.
fn refusal_text(refusal: &Refusal, closure: &[ClosureEntry]) -> String {
    match refusal {
        Refusal::UnknownRegistry { host } => format!(
            "{host} is not in your registry list. Add it in Tools › Preferences › Registries first."
        ),
        Refusal::UnlistedNamespace { .. } => format!("{refusal}."),
        Refusal::OfficialNamespaceNeedsOpen { .. }
        | Refusal::RegistryNotPrivate { .. }
        | Refusal::NamespaceNotPrivate { .. } => {
            let firmware = closure.iter().any(|c| {
                c.effective == Redistribution::UserSupplied
                    && c.descriptor.decoded_media_type() == mt::FIRMWARE
            });
            format!(
                "This image contains user-supplied {}: push it only to a private registry, never to unemu/.",
                if firmware { "firmware" } else { "content" }
            )
        }
        other => format!("{other}."),
    }
}

/// A reference the parser refused, explained; a bad tag gets the grammar.
fn parse_problem(text: &str, default_host: &str, error: &Error) -> String {
    let named = text.split('@').next().unwrap_or(text);
    if let Some((path, tag)) = named.rsplit_once(':') {
        if !tag.contains('/') && Reference::parse(path, default_host).is_ok() {
            return format!(
                "Tag “{tag}” does not follow the registry tag grammar: a letter, digit or _ first, then up to 127 letters, digits, . _ or -. A + build suffix is not allowed; versions are x.y.z or x.y.z-pre."
            );
        }
    }
    format!("{error}. Repository names use lowercase letters and digits, separated by . _ or -.")
}

/// The blob type as a person names it.
pub(crate) fn kind_label(descriptor: &Descriptor) -> String {
    let name = match descriptor.decoded_media_type() {
        mt::CONFIG => "machine config",
        mt::METADATA => "metadata",
        mt::FIRMWARE => "firmware",
        mt::FIRMWARE_CONFIG => "firmware package config",
        mt::MEDIA_SET_CONFIG => "media set config",
        mt::DISK_BASE => "disk base",
        mt::DISK_LAYER => "disk layer",
        mt::DISK_SECTORS => "sector pack",
        mt::FLOPPY => "floppy",
        mt::OPTICAL => "optical disc",
        mt::EXTRA => "extra file",
        mt::PNG | mt::JPEG => "artwork",
        pc_image::CMOS => "CMOS",
        pc_image::DISK_ZERO => "disk base (zero)",
        other => other,
    };
    if descriptor.is_compressed() {
        format!("{name}, zstd")
    } else {
        name.into()
    }
}

fn short(digest: &Digest) -> String {
    format!("sha256:{}…", &digest.hex()[..12])
}

/// Each blob once, in manifest order: a push sends every digest once.
fn distinct(closure: &[ClosureEntry]) -> Vec<&ClosureEntry> {
    let mut seen = BTreeSet::new();
    closure
        .iter()
        .filter(|entry| seen.insert(entry.descriptor.digest.clone()))
        .collect()
}

/// One line of the dry-run table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DryRunRow {
    pub part: String,
    pub kind: String,
    pub digest: String,
    pub size: u64,
    pub class: &'static str,
    /// "this image", "local tag NAME" or "firmware catalogue": where the
    /// effective class was recorded.
    pub source: String,
}

/// The guard's view of one destination.
#[derive(Clone, Debug, Default)]
pub(crate) struct Evaluation {
    pub resolved: Option<Reference>,
    pub checks: Vec<Check>,
    /// Why the push cannot go there; `None` when it may.
    pub refusal: Option<String>,
    /// The refusal is the guard's, which has no override; otherwise the
    /// destination is incomplete or malformed, or the dry run is missing.
    pub refused_by_guard: bool,
    pub blobs: usize,
    pub bytes: u64,
}

impl Evaluation {
    pub fn allowed(&self) -> bool {
        self.refusal.is_none() && self.resolved.is_some()
    }
}

fn check(mark: Mark, text: String) -> Check {
    Check { mark, text }
}

/// One blob of a running or finished push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlobRow {
    pub digest: Digest,
    pub label: String,
    pub size: u64,
    /// Bytes the registry holds, as its accepted chunks report them.
    pub sent: u64,
    pub outcome: Option<BlobOutcome>,
}

impl BlobRow {
    /// The status column: `current` is the blob the push works on now.
    pub fn status(&self, current: bool, finished: bool, cancelling: bool) -> String {
        let human = crate::library_view::human_size;
        let chunk = push::DEFAULT_CHUNK_SIZE;
        match self.outcome {
            Some(BlobOutcome::Present) => "Already there — skipped".into(),
            Some(BlobOutcome::Mounted) => "Mounted from another repository".into(),
            Some(BlobOutcome::Uploaded) => "Uploaded ✓".into(),
            None if finished => "Not sent".into(),
            None if !current => "Queued".into(),
            None if self.size > chunk => format!(
                "{}chunk {} of {} · {} of {}",
                if cancelling { "Stopping — " } else { "" },
                (self.sent / chunk + 1).min(self.size.div_ceil(chunk)),
                self.size.div_ceil(chunk),
                human(self.sent),
                human(self.size)
            ),
            None if cancelling => "Stopping after this upload request".into(),
            None => "Uploading · single chunk".into(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Progress {
    id: u64,
    cancel: Arc<AtomicBool>,
    live: Live<Vec<BlobRow>>,
    pub target: Reference,
    pub cancelling: bool,
    /// The manifest and the tag are on the registry.
    pub published: bool,
}

impl Progress {
    /// The blobs as the worker reports them now.
    pub fn blobs(&self) -> Vec<BlobRow> {
        self.live.read(|rows| rows.clone())
    }
}

#[derive(Debug)]
pub(crate) enum PushState {
    /// The worker reads the image and every local manifest for the dry run.
    Planning,
    /// The dry run is shown.
    Ready,
    /// The registry's tag names another image; confirming moves it away from
    /// `current` only.
    ConfirmMove {
        target: Reference,
        current: Digest,
        new: Digest,
    },
    Pushing(Progress),
    Finished {
        ok: bool,
        message: String,
        progress: Progress,
    },
}

pub(crate) struct PushDialog {
    accounts: Accounts,
    config: RegistryConfig,
    pub default_host: String,
    pub images: Vec<LocalImage>,
    /// The local tag being pushed.
    pub image: Option<String>,
    pub destination: String,
    plan: Option<PlanData>,
    pub state: PushState,
    /// Preferences or the default registry could not be read.
    pub preferences: Option<String>,
    /// The image could not be read.
    pub error: Option<String>,
    /// Why the dry run was built again.
    pub notice: Option<String>,
    planning: Option<u64>,
}

impl Drop for PushDialog {
    /// Closing the dialog, also for an exit, stops a running push.
    fn drop(&mut self) {
        self.cancel();
    }
}

impl PushDialog {
    /// Open for `local_tag`, or for the first local image when none is
    /// selected; the returned request asks the worker for the dry run.
    pub fn open(accounts: Accounts, local_tag: Option<String>) -> (Self, TransferRequest) {
        let (config, default_host, preferences) = match accounts.settings() {
            Ok((config, host)) => (config, host, None),
            Err(error) => (
                RegistryConfig::default(),
                DEFAULT_REGISTRY.into(),
                Some(error),
            ),
        };
        let id = next_id();
        let dialog = Self {
            accounts,
            default_host,
            config,
            images: Vec::new(),
            destination: local_tag.clone().unwrap_or_default(),
            image: local_tag.clone(),
            plan: None,
            state: PushState::Planning,
            preferences,
            error: None,
            notice: None,
            planning: Some(id),
        };
        (dialog, TransferRequest::Plan { id, local_tag })
    }

    pub fn plan(&self) -> Option<&PlanData> {
        self.plan.as_ref()
    }

    pub fn running(&self) -> bool {
        matches!(self.state, PushState::Pushing(_))
    }

    /// Choose another local image; its dry run comes from the worker.
    pub fn select_image(&mut self, tag: &str) -> Option<TransferRequest> {
        if self.running() || self.image.as_deref() == Some(tag) {
            return None;
        }
        self.image = Some(tag.into());
        self.destination = tag.into();
        self.notice = None;
        self.replan()
    }

    /// Ask the worker for the dry run of the chosen image again.
    fn replan(&mut self) -> Option<TransferRequest> {
        let tag = self.image.clone()?;
        self.plan = None;
        self.state = PushState::Planning;
        let id = next_id();
        self.planning = Some(id);
        Some(TransferRequest::Plan {
            id,
            local_tag: Some(tag),
        })
    }

    /// The name and tag the destination suggestions use.
    fn name_and_tag(&self) -> (String, String) {
        let local = self.image.as_deref().unwrap_or("name:1.0.0");
        let (repository, tag) = local.rsplit_once(':').unwrap_or((local, "latest"));
        let name = repository.rsplit('/').next().unwrap_or(repository);
        (name.into(), tag.into())
    }

    /// A private destination to suggest after a refusal.
    pub fn private_suggestion(&self) -> String {
        let (name, tag) = self.name_and_tag();
        format!("private/{name}:{tag}")
    }

    /// Destinations offered as one-click choices: the local name, its private
    /// counterpart on the default registry, and each other configured registry.
    pub fn suggestions(&self) -> Vec<String> {
        let Some(local) = self.image.clone() else {
            return Vec::new();
        };
        let (name, tag) = self.name_and_tag();
        let mut out = vec![local, self.private_suggestion()];
        for entry in &self.config.registries {
            if entry.host != self.default_host {
                out.push(format!("{}/private/{name}:{tag}", entry.host));
            }
        }
        let mut seen = BTreeSet::new();
        out.retain(|s| seen.insert(s.clone()));
        out
    }

    /// The dry-run table of the chosen image.
    pub fn dry_run(&self) -> Vec<DryRunRow> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let mut layer = 0;
        plan.closure
            .iter()
            .map(|entry| DryRunRow {
                part: if entry.role == "config" {
                    "configuration".into()
                } else {
                    layer += 1;
                    format!("layer {layer}")
                },
                kind: kind_label(&entry.descriptor),
                digest: short(&entry.descriptor.digest),
                size: entry.descriptor.size,
                class: entry.effective.as_str(),
                source: entry.source.to_string(),
            })
            .collect()
    }

    /// Resolve the destination and apply the guard's rules to the dry run,
    /// without any I/O: the verdict follows the typing.
    pub fn evaluate(&self) -> Evaluation {
        let mut evaluation = Evaluation::default();
        if let Some(problem) = &self.preferences {
            evaluation.refusal = Some(problem.clone());
            return evaluation;
        }
        let Some(plan) = &self.plan else {
            evaluation.refusal =
                Some(self.error.clone().unwrap_or_else(|| {
                    "Reading the image and the marks of every local image…".into()
                }));
            return evaluation;
        };
        let blobs = distinct(&plan.closure);
        evaluation.blobs = blobs.len();
        evaluation.bytes = blobs.iter().map(|c| c.descriptor.size).sum();
        let text = self.destination.trim();
        if text.is_empty() {
            evaluation.refusal = Some(format!(
                "Enter a destination such as {}.",
                self.private_suggestion()
            ));
            return evaluation;
        }
        let target = match Reference::parse(text, &self.default_host) {
            Ok(target) => target,
            Err(error) => {
                evaluation.refusal = Some(parse_problem(text, &self.default_host, &error));
                return evaluation;
            }
        };
        if target.digest.is_some() {
            evaluation.refusal = Some("A push needs a tag, not a digest.".into());
            return evaluation;
        }
        let entry = self.config.entry(&target.host);
        let scope = entry.as_ref().map(|e| e.scope(&target.repository));
        let unlisted = matches!(scope, Some(Scope::Unlisted { .. }));
        match (&entry, &scope) {
            (Some(entry), Some(scope)) => {
                evaluation.checks.push(check(
                    if unlisted { Mark::Bad } else { Mark::Good },
                    configured_line(&entry.host, &self.default_host, scope),
                ));
                evaluation.checks.push(match &entry.user {
                    Some(user) => check(
                        Mark::Note,
                        format!("Login {user}; its secret comes from the credential store"),
                    ),
                    None => check(
                        Mark::Note,
                        format!(
                            "No login recorded for {}; if it asks for one, log in under Tools › Preferences › Registries",
                            entry.host
                        ),
                    ),
                });
            }
            _ => evaluation.checks.push(check(
                Mark::Bad,
                format!("{} is not in your registry list", target.host),
            )),
        }
        let restricted = blobs
            .iter()
            .filter(|c| c.effective == Redistribution::UserSupplied)
            .count();
        let official = target.namespace() == OFFICIAL_NAMESPACE;
        let count = blobs.len();
        // An entry that lists namespaces takes no push outside them, whatever
        // the image holds, so the class lines would only distract.
        if !unlisted {
            if restricted == 0 {
                evaluation.checks.push(check(
                    Mark::Good,
                    format!(
                        "Every blob is open{}",
                        if official { ": allowed in unemu/" } else { "" }
                    ),
                ));
            } else if official {
                evaluation.checks.push(check(
                    Mark::Bad,
                    format!(
                        "{restricted} of {count} blobs are user-supplied; unemu/ accepts only open blobs"
                    ),
                ));
            } else if scope.as_ref().is_some_and(Scope::private) {
                evaluation.checks.push(check(
                    Mark::Good,
                    format!(
                        "{restricted} of {count} blobs are user-supplied and may go here: the target is private and not under unemu/"
                    ),
                ));
                evaluation.checks.push(check(
                    Mark::Note,
                    format!(
                        "Before the upload, anonymous reads of {}/{} are tried; the push stops unless the registry refuses them",
                        target.host, target.repository
                    ),
                ));
            } else if entry.is_some() {
                evaluation.checks.push(check(
                    Mark::Bad,
                    format!(
                        "{restricted} of {count} blobs are user-supplied; the target is not marked private"
                    ),
                ));
            }
        }
        let elsewhere =
            |wanted: fn(&MarkSource) -> bool| blobs.iter().filter(|c| wanted(&c.source)).count();
        let local = elsewhere(|source| matches!(source, MarkSource::LocalTag(_)));
        if local > 0 {
            evaluation.checks.push(check(
                Mark::Note,
                format!(
                    "{local} blobs this image marks open are user-supplied in another local image, so they count as user-supplied"
                ),
            ));
        }
        let catalogued = elsewhere(|source| matches!(source, MarkSource::Catalogue));
        if catalogued > 0 {
            evaluation.checks.push(check(
                Mark::Note,
                format!(
                    "{catalogued} blobs this image marks open are firmware the catalogue lists as proprietary, so they count as user-supplied"
                ),
            ));
        }
        match guard::check(entry.as_ref(), &target, &plan.closure) {
            Err(refusal) => {
                evaluation.refusal = Some(refusal_text(&refusal, &plan.closure));
                evaluation.refused_by_guard = true;
            }
            Ok(()) => {
                let tag = target.tag.clone().unwrap_or_else(|| "latest".into());
                evaluation.checks.push(check(
                    Mark::Note,
                    if is_version_tag(&tag) {
                        format!(
                            "{tag} is a version tag: once it names an image on the registry it never moves. The manifest and the tag are written after every blob"
                        )
                    } else if plan.media_set && !media_set::is_alias_tag(&tag) {
                        format!(
                            "{tag} is a tag of an installation media set: once it names a set on the registry it never moves, not even with confirmation; push a changed set under a new revision tag such as {}. The manifest and the tag are written after every blob",
                            media_set::next_revision(&tag)
                        )
                    } else {
                        format!(
                            "{tag} is not a version: if the registry has it for another image, you confirm the move with both digests first. The manifest and the tag are written after every blob"
                        )
                    },
                ));
            }
        }
        evaluation.resolved = Some(target);
        evaluation
    }

    pub fn can_push(&self) -> bool {
        matches!(self.state, PushState::Ready) && self.evaluate().allowed()
    }

    /// Start the push the dry run allows; `None` when the guard refuses.
    pub fn push(&mut self) -> Option<TransferRequest> {
        if !self.can_push() {
            return None;
        }
        let target = self.evaluate().resolved?;
        let image = self.plan.as_ref()?.image.digest.clone();
        self.start(target, image, None)
    }

    /// Push again, moving the tag away from the digest the user was shown.
    pub fn confirm_move(&mut self) -> Option<TransferRequest> {
        let PushState::ConfirmMove {
            target,
            current,
            new,
        } = &self.state
        else {
            return None;
        };
        let (target, current, new) = (target.clone(), current.clone(), new.clone());
        self.start(target, new, Some(current))
    }

    /// Back from the move confirmation to the dry run.
    pub fn decline_move(&mut self) {
        if matches!(self.state, PushState::ConfirmMove { .. }) {
            self.state = PushState::Ready;
        }
    }

    /// Back from a finished push to the dry run, for another destination.
    pub fn back(&mut self) {
        if matches!(self.state, PushState::Finished { .. }) {
            self.state = PushState::Ready;
        }
    }

    fn start(
        &mut self,
        target: Reference,
        image: Digest,
        move_from: Option<Digest>,
    ) -> Option<TransferRequest> {
        let plan = self.plan.as_ref()?;
        self.notice = None;
        let id = next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let live = Live::new(
            distinct(&plan.closure)
                .into_iter()
                .map(|entry| BlobRow {
                    digest: entry.descriptor.digest.clone(),
                    label: format!(
                        "{} · {}",
                        kind_label(&entry.descriptor),
                        short(&entry.descriptor.digest)
                    ),
                    size: entry.descriptor.size,
                    sent: 0,
                    outcome: None,
                })
                .collect(),
        );
        let job = PushJob {
            id,
            local_tag: plan.local_tag.clone(),
            image,
            target: target.clone(),
            move_from,
            accounts: self.accounts.clone(),
            cancel: cancel.clone(),
            live: live.clone(),
        };
        self.state = PushState::Pushing(Progress {
            id,
            cancel,
            live,
            target,
            cancelling: false,
            published: false,
        });
        Some(TransferRequest::Push(Box::new(job)))
    }

    /// Ask a running push to stop at its next check; it reports back.
    pub fn cancel(&mut self) {
        if let PushState::Pushing(progress) = &mut self.state {
            progress.cancel.store(true, Ordering::SeqCst);
            progress.cancelling = true;
        }
    }

    /// The library worker stopped, so no answer will come.
    pub fn fail(&mut self, message: &str) {
        self.planning = None;
        match std::mem::replace(&mut self.state, PushState::Ready) {
            PushState::Pushing(progress) => {
                self.state = PushState::Finished {
                    ok: false,
                    message: message.into(),
                    progress,
                }
            }
            PushState::Planning => self.error = Some(message.into()),
            other => self.state = other,
        }
    }

    /// Apply a worker event; a follow-up request may be needed.
    pub fn on_event(&mut self, event: &TransferEvent) -> Option<TransferRequest> {
        match event {
            TransferEvent::Planned { id, result } if self.planning == Some(*id) => {
                self.planning = None;
                self.state = PushState::Ready;
                match result {
                    Ok(planned) => {
                        self.images = planned.images.clone();
                        match &planned.plan {
                            Some(plan) => {
                                self.error = None;
                                self.plan = Some(plan.clone());
                            }
                            None => match self.images.first().map(|i| i.tag.clone()) {
                                Some(first) => return self.select_image(&first),
                                None => {
                                    self.error =
                                        Some("The image store holds no image to push yet.".into())
                                }
                            },
                        }
                    }
                    Err(error) => self.error = Some(error.clone()),
                }
            }
            TransferEvent::Pushed { id, result } => {
                if !matches!(&self.state, PushState::Pushing(p) if p.id == *id) {
                    return None;
                }
                let PushState::Pushing(mut progress) =
                    std::mem::replace(&mut self.state, PushState::Ready)
                else {
                    return None;
                };
                // Nothing was sent before these answers, so after Cancel they
                // are only the cancel the user asked for.
                let cancelled = matches!(
                    result,
                    Err(PushFailure::Cancelled | PushFailure::Move { .. } | PushFailure::Stale)
                ) && (progress.cancelling
                    || matches!(result, Err(PushFailure::Cancelled)));
                if cancelled {
                    self.state = PushState::Finished {
                        ok: false,
                        message: PUSH_CANCELLED.into(),
                        progress,
                    };
                    return None;
                }
                self.state = match result {
                    Ok(report) => {
                        progress.live.update(|rows| {
                            for (descriptor, outcome) in &report.blobs {
                                record_push(rows, PushProgress::Blob(descriptor, *outcome));
                            }
                        });
                        progress.published = true;
                        PushState::Finished {
                            ok: true,
                            message: pushed_text(&progress.target, report),
                            progress,
                        }
                    }
                    Err(PushFailure::Move { current, new, .. }) => PushState::ConfirmMove {
                        target: progress.target,
                        current: current.clone(),
                        new: new.clone(),
                    },
                    Err(PushFailure::Stale) => {
                        self.notice = Some(STALE.into());
                        return self.replan();
                    }
                    Err(PushFailure::Cancelled) => PushState::Finished {
                        ok: false,
                        message: PUSH_CANCELLED.into(),
                        progress,
                    },
                    Err(PushFailure::Refused(text)) => PushState::Finished {
                        ok: false,
                        message: format!("Push refused. {text} {NO_OVERRIDE}"),
                        progress,
                    },
                    Err(PushFailure::Failed(text)) => PushState::Finished {
                        ok: false,
                        message: text.clone(),
                        progress,
                    },
                };
            }
            _ => {}
        }
        None
    }
}

fn pushed_text(target: &Reference, report: &PushReport) -> String {
    let digest = &report.image.digest;
    match &report.tag {
        TagOutcome::Created => format!(
            "Pushed {target} ({}). The manifest and the tag were written last.",
            short(digest)
        ),
        TagOutcome::Moved { from } => format!(
            "Moved {target} from {from} to {digest}. The old image stays reachable by its digest."
        ),
        TagOutcome::Unchanged => {
            format!("{target} already names this image; nothing was uploaded.")
        }
        TagOutcome::Untagged => format!("Pushed {digest} without a tag."),
    }
}

/// What a running pull has received so far.
#[derive(Clone, Debug, Default)]
pub(crate) struct PullLive {
    /// Blobs verified in the store.
    pub fetched: Vec<Descriptor>,
    /// Blobs still downloading, with the bytes staged of each.
    pub receiving: Vec<(Descriptor, u64)>,
}

impl PullLive {
    fn record(&mut self, event: pull::PullProgress<'_>) {
        match event {
            pull::PullProgress::Receiving { blob, have } => {
                match self
                    .receiving
                    .iter_mut()
                    .find(|(d, _)| d.digest == blob.digest)
                {
                    Some((_, staged)) => *staged = have,
                    None => self.receiving.push((blob.clone(), have)),
                }
            }
            pull::PullProgress::Verified(blob) => {
                self.receiving.retain(|(d, _)| d.digest != blob.digest);
                self.fetched.push(blob.clone());
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct PullRun {
    id: u64,
    cancel: Arc<AtomicBool>,
    live: Live<PullLive>,
    pub source: Reference,
    pub local_tag: String,
    pub cancelling: bool,
}

impl PullRun {
    /// A pull of `source` written as `local_tag`: the run a dialog shows and
    /// the job for the library worker.
    pub(crate) fn start(
        accounts: &Accounts,
        source: Reference,
        local_tag: String,
    ) -> (Self, TransferRequest) {
        let id = next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let live = Live::new(PullLive::default());
        let job = PullJob {
            id,
            source: source.clone(),
            local_tag: local_tag.clone(),
            accounts: accounts.clone(),
            cancel: cancel.clone(),
            live: live.clone(),
        };
        let run = Self {
            id,
            cancel,
            live,
            source,
            local_tag,
            cancelling: false,
        };
        (run, TransferRequest::Pull(Box::new(job)))
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// The progress as the worker reports it now.
    pub fn progress(&self) -> PullLive {
        self.live.read(|state| state.clone())
    }

    /// Ask the pull to stop at its next check; it reports back.
    pub(crate) fn cancel(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.cancelling = true;
    }
}

#[derive(Debug)]
pub(crate) enum PullState {
    Ready,
    Pulling(PullRun),
    Finished {
        ok: bool,
        message: String,
        /// The local tag the pull wrote.
        pulled: Option<String>,
    },
}

pub(crate) struct PullDialog {
    accounts: Accounts,
    config: RegistryConfig,
    pub default_host: String,
    pub reference: String,
    /// Empty: the short form of the reference.
    pub local_tag: String,
    /// Preferences or the default registry could not be read.
    pub error: Option<String>,
    pub state: PullState,
}

impl Drop for PullDialog {
    /// Closing the dialog, also for an exit, stops a running pull.
    fn drop(&mut self) {
        self.cancel();
    }
}

impl PullDialog {
    pub fn open(accounts: Accounts) -> Self {
        let (config, default_host, error) = match accounts.settings() {
            Ok((config, host)) => (config, host, None),
            Err(error) => (
                RegistryConfig::default(),
                DEFAULT_REGISTRY.into(),
                Some(error),
            ),
        };
        Self {
            accounts,
            config,
            default_host,
            reference: String::new(),
            local_tag: String::new(),
            error,
            state: PullState::Ready,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.state, PullState::Pulling(_))
    }

    /// The registry reference and the local tag the pull would write.
    pub fn resolve(&self) -> Result<(Reference, String), String> {
        if let Some(problem) = &self.error {
            return Err(problem.clone());
        }
        let text = self.reference.trim();
        if text.is_empty() {
            return Err("Enter a reference such as open-at:1.0.0 or private/name:tag.".into());
        }
        let source = Reference::parse(text, &self.default_host)
            .map_err(|e| parse_problem(text, &self.default_host, &e))?;
        let local = match self.local_tag.trim() {
            "" => source
                .local_tag()
                .ok_or("Pulling by digest needs a local tag.")?,
            typed => typed.to_owned(),
        };
        Store::validate_tag_name(&local).map_err(|_| {
            format!(
                "Local tag “{local}” is not valid: use name:tag in lowercase letters and digits, with a tag that follows the registry tag grammar."
            )
        })?;
        Ok((source, local))
    }

    /// The source's registry entry and namespace, as Push… names them;
    /// `None` until the reference resolves, or for a host with no entry.
    pub fn namespace_line(&self) -> Option<String> {
        let (source, _) = self.resolve().ok()?;
        let entry = self.config.entry(&source.host)?;
        Some(configured_line(
            &entry.host,
            &self.default_host,
            &entry.scope(&source.repository),
        ))
    }

    pub fn can_pull(&self) -> bool {
        !self.running() && self.resolve().is_ok()
    }

    pub fn pull(&mut self) -> Option<TransferRequest> {
        if self.running() {
            return None;
        }
        let (source, local_tag) = self.resolve().ok()?;
        let (run, request) = PullRun::start(&self.accounts, source, local_tag);
        self.state = PullState::Pulling(run);
        Some(request)
    }

    /// Ask a running pull to stop at its next check; it reports back.
    pub fn cancel(&mut self) {
        if let PullState::Pulling(run) = &mut self.state {
            run.cancel();
        }
    }

    /// The library worker stopped, so no answer will come.
    pub fn fail(&mut self, message: &str) {
        if self.running() {
            self.state = PullState::Finished {
                ok: false,
                message: message.into(),
                pulled: None,
            };
        }
    }

    /// Back from a finished pull to the form.
    pub fn back(&mut self) {
        if matches!(self.state, PullState::Finished { .. }) {
            self.state = PullState::Ready;
        }
    }

    pub fn on_event(&mut self, event: &TransferEvent) {
        let PullState::Pulling(run) = &self.state else {
            return;
        };
        let TransferEvent::Pulled { id, result } = event else {
            return;
        };
        if *id != run.id {
            return;
        }
        let finished = match result {
            Ok(report) => PullState::Finished {
                ok: true,
                message: pulled_text(&run.source, report),
                pulled: Some(report.local_tag.clone()),
            },
            Err(failure) => PullState::Finished {
                ok: false,
                message: failure.message().into(),
                pulled: None,
            },
        };
        self.state = finished;
    }
}

pub(crate) fn pulled_text(source: &Reference, report: &PullReport) -> String {
    match report.outcome {
        PullOutcome::Unchanged => format!(
            "{} already names this image; nothing was fetched.",
            report.local_tag
        ),
        PullOutcome::Pulled => format!(
            "Pulled {source} as {} ({}): {} blobs fetched ({}), {} already in the store. The tag was written after every blob was verified.",
            report.local_tag,
            short(&report.image.digest),
            report.fetched.len(),
            crate::library_view::human_size(report.fetched.iter().map(|d| d.size).sum()),
            report.present.len()
        ),
    }
}

/// What a closed dialog's transfer ended with, for the status bar.
pub(crate) fn closed_outcome(event: &TransferEvent) -> Option<&'static str> {
    match event {
        TransferEvent::Pushed {
            result: Err(PushFailure::Cancelled),
            ..
        } => Some(
            "Push cancelled: the manifest was not published, so the registry keeps its old tag.",
        ),
        TransferEvent::Pulled {
            result: Err(PullFailure::Cancelled | PullFailure::NotStarted),
            ..
        } => Some("Pull cancelled: the local tag was not set."),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        library_test_support::Fixture,
        library_view::{ImageRow, LibraryView},
        library_worker::Request,
    };
    use image_registry::{
        credential_store::{Secret, SessionStore, StoreKind},
        test_support::{BlobFault, MockRegistry, Policy},
    };
    use image_store::{BlobSource, Manifest, MAX_SMALL_BLOB};
    use std::{
        path::Path,
        time::{Duration, Instant},
    };

    const SECRET: &str = "synthetic-secret";
    const FIRST_BLOB_HEAD: &str = "HEAD /v2/private/test/blobs/";

    /// A registry that demands the Basic login `ci` with [`SECRET`].
    fn login_policy() -> Policy {
        Policy {
            user: Some(("ci".into(), SECRET.into())),
            ..Policy::default()
        }
    }

    /// Preferences that list `host` with the login `ci`, whose secret is in `store`.
    pub(crate) fn accounts_for(
        dir: &Path,
        host: &str,
        private: bool,
        store: Arc<dyn CredentialStore>,
    ) -> Accounts {
        let path = dir.join(format!("preferences-{}.toml", host.replace(':', "-")));
        std::fs::write(
            &path,
            format!(
                "[[registries]]\nhost = \"{host}\"\nprivate = {private}\nuser = \"ci\"\nplain_http = true\n"
            ),
        )
        .unwrap();
        store.store(host, "ci", &Secret::new(SECRET)).unwrap();
        Accounts {
            path: Some(path),
            store,
            env: EnvCredentials::default(),
            default_override: None,
        }
    }

    /// A store with the synthetic fixture image, a library worker on it and a
    /// mock registry listed in preferences with its login.
    pub(crate) struct Lab {
        pub f: Fixture,
        pub registry: MockRegistry,
        pub accounts: Accounts,
        pub view: LibraryView,
    }

    impl Lab {
        pub fn new(private: bool) -> Self {
            let f = Fixture::new();
            let registry = MockRegistry::start(login_policy());
            let accounts = accounts_for(
                f.temp.path(),
                &registry.host(),
                private,
                Arc::new(SessionStore::default()),
            );
            let view = LibraryView::synthetic(f.store.root().to_owned());
            Self {
                f,
                registry,
                accounts,
                view,
            }
        }

        fn host(&self) -> String {
            self.registry.host()
        }

        fn open_push(&mut self, tag: &str) -> &mut PushDialog {
            open_push(&mut self.view, &self.accounts, &self.f.store, tag)
        }

        /// Push `tag` to `destination` through the dialog; the guard must allow it.
        fn push_through(&mut self, tag: &str, destination: &str) {
            let dialog = self.open_push(tag);
            dialog.destination = destination.into();
            let request = dialog.push().expect("the guard allows this push");
            self.view.send(Request::Transfer(request));
            self.view.pump_until(push_settled);
            let state = &self.view.push.as_ref().unwrap().state;
            assert!(
                matches!(state, PushState::Finished { ok: true, .. }),
                "{state:?}"
            );
        }

        /// Start a push of the fixture image to `private/test:1.0.0` that the
        /// registry holds at its first blob, and return the push's cancel flag.
        pub fn held_push(&mut self) -> Arc<AtomicBool> {
            let host = self.host();
            let dialog = self.open_push("test:source");
            dialog.destination = format!("{host}/private/test:1.0.0");
            let Some(TransferRequest::Push(job)) = dialog.push() else {
                panic!("the guard allows this push")
            };
            let cancel = job.cancel.clone();
            self.registry.hold_next(FIRST_BLOB_HEAD);
            self.view
                .send(Request::Transfer(TransferRequest::Push(job)));
            let registry = &self.registry;
            wait_pumping(&mut self.view, || registry.held());
            cancel
        }

        pub fn manifest_was_put(&self) -> bool {
            self.registry
                .log()
                .iter()
                .any(|l| l.starts_with("PUT /v2/private/test/manifests/"))
        }
    }

    fn open_push<'a>(
        view: &'a mut LibraryView,
        accounts: &Accounts,
        store: &Store,
        tag: &str,
    ) -> &'a mut PushDialog {
        view.push = None;
        view.selected = Some(ImageRow {
            reference: tag.into(),
            aliases: Vec::new(),
            descriptor: store.tag_descriptor(tag).unwrap(),
            summary: None,
            failure: None,
        });
        view.begin_push(accounts.clone());
        view.pump_until(|v| matches!(v.push.as_ref().unwrap().state, PushState::Ready));
        view.push.as_mut().unwrap()
    }

    fn push_settled(view: &LibraryView) -> bool {
        matches!(
            view.push.as_ref().unwrap().state,
            PushState::Finished { .. } | PushState::ConfirmMove { .. }
        )
    }

    fn pull_settled(view: &LibraryView) -> bool {
        matches!(
            view.pull.as_ref().unwrap().state,
            PullState::Finished { .. }
        )
    }

    /// Wait for `condition` while applying the worker's queued events, so the
    /// worker never waits on a full queue meanwhile.
    pub(crate) fn wait_pumping(view: &mut LibraryView, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + crate::test_wait::HANG_GUARD;
        while !condition() {
            assert!(Instant::now() < deadline, "the condition was not reached");
            view.pump_queued();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn manifest_of(store: &Store, image: &Descriptor) -> Manifest {
        Manifest::decode_for_summary(&store.read_verified(image, MAX_SMALL_BLOB).unwrap()).unwrap()
    }

    /// Tag `tag` as a copy of the fixture image whose manifest differs only
    /// in its title, so it has another digest and the same blobs.
    fn retitled(f: &Fixture, tag: &str) -> Descriptor {
        let mut manifest = manifest_of(&f.store, &f.row.descriptor);
        manifest
            .annotations
            .insert("org.opencontainers.image.title".into(), tag.into());
        let image = f
            .store
            .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
            .unwrap();
        pc_image::test_support::with_synthetic_firmware(|| {
            f.store.update_tag(tag, &image, None, &pc_image::PcProvider)
        })
        .unwrap();
        image
    }

    fn distinct_blobs(store: &Store, image: &Descriptor) -> Vec<Descriptor> {
        let manifest = manifest_of(store, image);
        let mut seen = BTreeSet::new();
        std::iter::once(manifest.config)
            .chain(manifest.layers)
            .filter(|d| seen.insert(d.digest.clone()))
            .collect()
    }

    /// Run a request on this thread as the worker would, with the synthetic
    /// firmware rules and `catalogue`, and return its events.
    fn run_here(
        store: &Store,
        request: TransferRequest,
        catalogue: &[assembly::FirmwareEntry],
    ) -> Vec<TransferEvent> {
        let events = Mutex::new(Vec::new());
        pc_image::test_support::with_synthetic_firmware(|| {
            cli::with_image_providers(|providers| {
                run(
                    store,
                    request,
                    providers,
                    catalogue,
                    &|event| events.lock().unwrap().push(event),
                    &|| {},
                )
            })
        });
        events.into_inner().unwrap()
    }

    #[test]
    fn the_dry_run_lists_every_blob_and_an_allowed_push_publishes_the_manifest_last() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        let image = lab.f.row.descriptor.clone();
        let blobs = distinct_blobs(&lab.f.store, &image).len();
        let dialog = lab.open_push("test:source");
        assert_eq!(dialog.plan().unwrap().image.digest, image.digest);
        assert!(dialog.images.iter().any(|i| i.tag == "test:source"));
        let rows = dialog.dry_run();
        assert!(rows
            .iter()
            .any(|r| r.kind == "machine config" && r.class == "open" && r.source == "this image"));
        assert!(rows
            .iter()
            .any(|r| r.kind.starts_with("firmware") && r.class == "user-supplied"));
        dialog.destination = format!("{host}/private/test:1.0.0");
        let evaluation = dialog.evaluate();
        assert!(evaluation.allowed(), "{evaluation:?}");
        assert_eq!(evaluation.blobs, blobs);
        assert!(evaluation
            .checks
            .iter()
            .any(|c| c.mark == Mark::Good && c.text.contains("may go here")));
        assert!(evaluation
            .checks
            .iter()
            .any(|c| c.text.contains("anonymous reads")));
        let request = dialog.push().unwrap();
        // Hold the manifest: every blob is up, and the dialog shows it before
        // the push ends.
        lab.registry.hold_next("PUT /v2/private/test/manifests/");
        lab.view.send(Request::Transfer(request));
        let registry = &lab.registry;
        wait_pumping(&mut lab.view, || registry.held());
        let PushState::Pushing(progress) = &lab.view.push.as_ref().unwrap().state else {
            panic!("{:?}", lab.view.push.as_ref().unwrap().state)
        };
        let shown = progress.blobs();
        assert_eq!(shown.len(), blobs);
        assert!(shown
            .iter()
            .all(|b| b.outcome == Some(BlobOutcome::Uploaded) && b.sent == b.size));
        assert!(!progress.published);
        assert_eq!(lab.registry.tag_digest("private/test", "1.0.0"), None);
        lab.registry.release();
        lab.view.pump_until(push_settled);
        let PushState::Finished {
            ok,
            message,
            progress,
        } = &lab.view.push.as_ref().unwrap().state
        else {
            panic!()
        };
        assert!(ok, "{message}");
        assert!(message.contains("written last"), "{message}");
        assert!(progress.published);
        assert_eq!(
            lab.registry.tag_digest("private/test", "1.0.0"),
            Some(image.digest)
        );
        assert_eq!(
            lab.registry.log().last().map(String::as_str),
            Some("PUT /v2/private/test/manifests/1.0.0")
        );
    }

    #[test]
    fn the_guard_refuses_without_override_and_a_forged_job_reaches_no_registry() {
        let mut lab = Lab::new(false);
        let host = lab.host();
        let image = lab.f.row.descriptor.clone();
        let dialog = lab.open_push("test:source");
        for (destination, refusal, line) in [
            (
                format!("{host}/private/test:1"),
                "push it only to a private registry",
                "the target is not marked private",
            ),
            (
                format!("{host}/unemu/test:1"),
                "never to unemu/",
                "unemu/ accepts only open blobs",
            ),
            (
                "other.example/private/test:1".to_owned(),
                "not in your registry list",
                "is not in your registry list",
            ),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(evaluation.refused_by_guard, "{destination}");
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(refusal),
                "{destination}: {evaluation:?}"
            );
            assert!(
                evaluation
                    .checks
                    .iter()
                    .any(|c| c.mark == Mark::Bad && c.text.contains(line)),
                "{destination}: {:?}",
                evaluation.checks
            );
            assert!(!dialog.can_push());
            assert!(dialog.push().is_none());
        }
        dialog.destination = format!("{host}/private/test:1");
        assert!(dialog
            .evaluate()
            .refusal
            .unwrap()
            .contains("contains user-supplied firmware"));
        for (destination, problem) in [
            (format!("{host}/private/test:1.0.0+b1"), "tag grammar"),
            (
                format!("{host}/private/test@{}", image.digest),
                "needs a tag",
            ),
            (String::new(), "Enter a destination"),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(!evaluation.refused_by_guard && !evaluation.allowed());
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(problem),
                "{destination}: {evaluation:?}"
            );
            assert!(dialog.push().is_none());
        }

        // A job built past the dialog is refused by the worker before any request.
        let forged = TransferRequest::Push(Box::new(PushJob {
            id: next_id(),
            local_tag: "test:source".into(),
            image: image.digest.clone(),
            target: Reference::parse(&format!("{host}/private/test:1"), "unused.example").unwrap(),
            move_from: None,
            accounts: lab.accounts.clone(),
            cancel: Arc::default(),
            live: Live::default(),
        }));
        let events = run_here(&lab.f.store, forged, &[]);
        assert!(
            matches!(events.as_slice(), [TransferEvent::Pushed { result: Err(PushFailure::Refused(text)), .. }]
                if text.contains("private registry")),
            "{:?}",
            events.len()
        );
        assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());

        // A registry marked private that answers anonymous reads is refused at
        // push time, before any blob goes up.
        let leaky = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..login_policy()
        });
        let accounts = accounts_for(
            lab.f.temp.path(),
            &leaky.host(),
            true,
            Arc::new(SessionStore::default()),
        );
        let dialog = open_push(&mut lab.view, &accounts, &lab.f.store, "test:source");
        dialog.destination = format!("{}/private/test:1", leaky.host());
        let request = dialog
            .push()
            .expect("the dry run allows a private registry");
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::Finished { ok, message, .. } = &lab.view.push.as_ref().unwrap().state else {
            panic!()
        };
        assert!(!ok);
        assert!(
            message.starts_with("Push refused.") && message.contains("read anonymously"),
            "{message}"
        );
        assert!(message.ends_with(NO_OVERRIDE));
        assert!(!leaky
            .log()
            .iter()
            .any(|l| l.starts_with("POST ") || l.starts_with("PUT ")));
    }

    /// List the lab registry with a public `unemu/` and a private `private/`,
    /// as the maintainer's one host serves them, keeping its login.
    fn namespaced(lab: &Lab) {
        let host = lab.host();
        std::fs::write(
            lab.accounts.path.as_ref().unwrap(),
            format!(
                "[[registries]]\nhost = \"{host}\"\nuser = \"ci\"\nplain_http = true\nnamespaces = [{{ name = \"unemu\", private = false }}, {{ name = \"private\", private = true }}]\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn push_and_pull_name_the_namespace_and_the_guard_decides_per_namespace() {
        let mut lab = Lab::new(false);
        namespaced(&lab);
        let host = lab.host();
        let image = lab.f.row.descriptor.clone();
        let dialog = lab.open_push("test:source");
        for (destination, refusal, mark, line) in [
            (
                format!("{host}/unemu/test:1"),
                "never to unemu/".to_owned(),
                Mark::Good,
                format!("{host} is configured; unemu/ is public"),
            ),
            (
                format!("{host}/owner/test:1"),
                format!("{host}/owner/ is not a configured namespace on this registry."),
                Mark::Bad,
                format!(
                    "{host} is configured; owner/ is not a configured namespace on this registry"
                ),
            ),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(evaluation.refused_by_guard, "{destination}");
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(&refusal),
                "{destination}: {evaluation:?}"
            );
            assert!(
                evaluation
                    .checks
                    .iter()
                    .any(|c| c.mark == mark && c.text == line),
                "{destination}: {:?}",
                evaluation.checks
            );
            assert!(dialog.push().is_none());
        }
        dialog.destination = format!("{host}/private/test:1.0.0");
        let evaluation = dialog.evaluate();
        assert!(evaluation.allowed(), "{evaluation:?}");
        assert_eq!(
            evaluation.checks[0].text,
            format!("{host} is configured; private/ is marked private")
        );
        assert!(evaluation
            .checks
            .iter()
            .any(|c| c.mark == Mark::Good && c.text.contains("may go here")));
        let request = dialog.push().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let state = &lab.view.push.as_ref().unwrap().state;
        assert!(
            matches!(state, PushState::Finished { ok: true, .. }),
            "{state:?}"
        );
        assert_eq!(
            lab.registry.tag_digest("private/test", "1.0.0"),
            Some(image.digest.clone())
        );

        // A job built past the dialog for an unlisted namespace reaches no registry.
        lab.registry.clear_log();
        let forged = TransferRequest::Push(Box::new(PushJob {
            id: next_id(),
            local_tag: "test:source".into(),
            image: image.digest.clone(),
            target: Reference::parse(&format!("{host}/owner/test:1"), "unused.example").unwrap(),
            move_from: None,
            accounts: lab.accounts.clone(),
            cancel: Arc::default(),
            live: Live::default(),
        }));
        let events = run_here(&lab.f.store, forged, &[]);
        assert!(
            matches!(events.as_slice(), [TransferEvent::Pushed { result: Err(PushFailure::Refused(text)), .. }]
                if text.contains("is not a configured namespace on this registry")),
            "{:?}",
            events.len()
        );
        assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());

        // Pull… names the namespace the same way, and nothing for an unlisted host.
        let mut pull = PullDialog::open(lab.accounts.clone());
        pull.reference = format!("{host}/private/test:1.0.0");
        assert_eq!(
            pull.namespace_line().as_deref(),
            Some(format!("{host} is configured; private/ is marked private").as_str())
        );
        pull.reference = "other.example/x/y:1".into();
        assert_eq!(pull.namespace_line(), None);
    }

    #[test]
    fn open_marks_count_only_while_no_other_local_tag_restricts_the_same_blobs() {
        let mut lab = Lab::new(false);
        let host = lab.host();
        let mut manifest = manifest_of(&lab.f.store, &lab.f.row.descriptor);
        let mit = image_store::license::Expression::parse("MIT").unwrap();
        for descriptor in std::iter::once(&mut manifest.config).chain(&mut manifest.layers) {
            image_store::license::mark(descriptor, Some(&mit));
        }
        let open = lab
            .f
            .store
            .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
            .unwrap();
        pc_image::test_support::with_synthetic_firmware(|| {
            lab.f
                .store
                .update_tag("open:1.0.0", &open, None, &pc_image::PcProvider)
        })
        .unwrap();
        let dialog = lab.open_push("open:1.0.0");
        let rows = dialog.dry_run();
        assert!(
            rows.iter().any(|r| r.kind.starts_with("firmware")
                && r.class == "user-supplied"
                && r.source == "local tag test:source"),
            "{rows:#?}"
        );
        dialog.destination = format!("{host}/unemu/open:1.0.0");
        let evaluation = dialog.evaluate();
        assert!(evaluation.refused_by_guard);
        assert!(evaluation
            .checks
            .iter()
            .any(|c| c.text.contains("user-supplied in another local image")));

        lab.f
            .store
            .remove_tag("test:source", &lab.f.row.descriptor.digest)
            .unwrap();
        let dialog = lab.open_push("open:1.0.0");
        assert!(dialog
            .dry_run()
            .iter()
            .all(|r| r.class == "open" && r.source == "this image"));
        dialog.destination = format!("{host}/unemu/open:1.0.0");
        let evaluation = dialog.evaluate();
        assert!(evaluation.allowed(), "{evaluation:?}");
        assert!(evaluation
            .checks
            .iter()
            .any(|c| c.text == "Every blob is open: allowed in unemu/"));
        lab.push_through("open:1.0.0", &format!("{host}/unemu/open:1.0.0"));
        assert_eq!(
            lab.registry.tag_digest("unemu/open", "1.0.0"),
            Some(open.digest)
        );
    }

    /// A firmware package that marks its ROM open under MIT, as one made
    /// where the catalogue does not know the ROM and pulled from elsewhere
    /// would. This catalogue lists the same bytes as one chip of a
    /// proprietary BIOS, so the guard keeps the package private whatever its
    /// marks say.
    #[test]
    fn a_package_marking_catalogued_proprietary_firmware_open_cannot_leave() {
        let mut lab = Lab::new(false);
        let host = lab.host();
        let rom = lab.f.temp.path().join("rom.bin");
        std::fs::write(&rom, [0x31; 16384]).unwrap();
        let candidate =
            pc_image::firmware_import::unlisted(&rom, "foreign-rom", "option-rom", &[]).unwrap();
        let statement = pc_image::firmware_import::Statement {
            open: true,
            license: Some("MIT".into()),
            ..Default::default()
        };
        pc_image::firmware_import::publish_with(&lab.f.store, &candidate, &[], &statement, &[])
            .unwrap();
        let tag = "firmware/foreign-rom:1";
        let catalogue = crate::library_view::firmware::tests::synthetic_catalogue();
        assert!(pc_image::firmware::proprietary(
            catalogue,
            Digest::of(&[0x31; 16384]).hex(),
            16384
        )
        .is_some());

        let dialog = lab.open_push(tag);
        let rows = dialog.dry_run();
        let firmware = rows
            .iter()
            .find(|r| r.kind.starts_with("firmware") && !r.kind.contains("config"))
            .unwrap();
        assert_eq!(
            (firmware.class, firmware.source.as_str()),
            ("user-supplied", "firmware catalogue"),
            "{rows:#?}"
        );
        for (destination, refusal) in [
            (
                format!("{host}/owner/rom:2"),
                "push it only to a private registry",
            ),
            (format!("{host}/unemu/rom:1"), "never to unemu/"),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(evaluation.refused_by_guard, "{destination}");
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(refusal),
                "{destination}: {evaluation:?}"
            );
            assert!(evaluation.checks.iter().any(|c| c
                .text
                .contains("firmware the catalogue lists as proprietary")));
        }

        // The worker refuses a job built past the dialog the same way.
        let image = lab.f.store.tag_descriptor(tag).unwrap();
        let job = |target: &str| {
            TransferRequest::Push(Box::new(PushJob {
                id: next_id(),
                local_tag: tag.into(),
                image: image.digest.clone(),
                target: Reference::parse(&format!("{host}/{target}"), "unused.example").unwrap(),
                move_from: None,
                accounts: lab.accounts.clone(),
                cancel: Arc::default(),
                live: Live::default(),
            }))
        };
        for target in ["owner/rom:2", "unemu/rom:1"] {
            let events = run_here(&lab.f.store, job(target), catalogue);
            assert!(
                matches!(events.as_slice(), [TransferEvent::Pushed { result: Err(PushFailure::Refused(text)), .. }]
                    if text.contains("user-supplied firmware")),
                "{target}"
            );
        }
        assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());

        // Without the catalogue's knowledge the package reads as open.
        let events = run_here(
            &lab.f.store,
            TransferRequest::Plan {
                id: next_id(),
                local_tag: Some(tag.into()),
            },
            &[],
        );
        let [TransferEvent::Planned {
            result: Ok(planned),
            ..
        }] = events.as_slice()
        else {
            panic!()
        };
        assert!(planned
            .plan
            .as_ref()
            .unwrap()
            .closure
            .iter()
            .all(|c| c.effective == Redistribution::Open && c.source == MarkSource::ThisImage));
    }

    #[test]
    fn moving_a_tag_needs_a_confirmation_with_both_digests_bound_to_the_old_one() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        let first = lab.f.row.descriptor.digest.clone();
        lab.push_through("test:source", &format!("{host}/private/test:latest"));
        let second = retitled(&lab.f, "test:other").digest;
        let dialog = lab.open_push("test:other");
        dialog.destination = format!("{host}/private/test:latest");
        let request = dialog.push().unwrap();
        lab.registry.clear_log();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::ConfirmMove { current, new, .. } = &lab.view.push.as_ref().unwrap().state
        else {
            panic!("{:?}", lab.view.push.as_ref().unwrap().state)
        };
        assert_eq!((current, new), (&first, &second));
        assert_eq!(
            lab.registry.tag_digest("private/test", "latest"),
            Some(first)
        );
        assert!(
            !lab.registry.log().iter().any(|l| l.starts_with("POST ")),
            "nothing goes up before the confirmation"
        );

        // Another client moves the tag meanwhile: the confirmation is asked
        // again with the digest the tag names now.
        let elsewhere =
            lab.registry
                .seed_manifest("private/test", "latest", br#"{"schemaVersion":2}"#);
        let dialog = lab.view.push.as_mut().unwrap();
        let request = dialog.confirm_move().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::ConfirmMove { current, .. } = &lab.view.push.as_ref().unwrap().state else {
            panic!("{:?}", lab.view.push.as_ref().unwrap().state)
        };
        assert_eq!(current, &elsewhere);
        assert_eq!(
            lab.registry.tag_digest("private/test", "latest"),
            Some(elsewhere.clone())
        );
        let request = lab.view.push.as_mut().unwrap().confirm_move().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::Finished { ok, message, .. } = &lab.view.push.as_ref().unwrap().state else {
            panic!()
        };
        assert!(ok, "{message}");
        assert!(
            message.contains(&elsewhere.to_string()) && message.contains(&second.to_string()),
            "{message}"
        );
        assert_eq!(
            lab.registry.tag_digest("private/test", "latest"),
            Some(second.clone())
        );

        // A version tag never moves and offers no confirmation.
        lab.push_through("test:source", &format!("{host}/private/test:1.0.0"));
        let dialog = lab.open_push("test:other");
        dialog.destination = format!("{host}/private/test:1.0.0");
        let request = dialog.push().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::Finished { ok, message, .. } = &lab.view.push.as_ref().unwrap().state else {
            panic!("{:?}", lab.view.push.as_ref().unwrap().state)
        };
        assert!(!ok);
        assert!(message.contains("Version tags never move"), "{message}");
        assert!(message.contains(&lab.f.row.descriptor.digest.to_string()));
        assert!(message.contains(&second.to_string()));
    }

    /// Tag `tag` as an installation media set of one 360K floppy filled with `byte`.
    fn installation_set(f: &Fixture, tag: &str, byte: u8) -> Descriptor {
        use pc_image::media_import::{plan, publish, Request, Source};
        let disk = f.temp.path().join(format!("set-{byte}.img"));
        std::fs::write(&disk, vec![byte; 368_640]).unwrap();
        let request = Request {
            tag: tag.into(),
            title: image_store::metadata::Title {
                title: "Synthetic set".into(),
                ..Default::default()
            },
            labels: Vec::new(),
            media: vec![Source {
                path: disk,
                label: None,
                format: None,
            }],
            extras: Vec::new(),
            compress: false,
        };
        publish(&f.store, &plan(&request).unwrap(), &|_, _| {}, &|| false)
            .unwrap()
            .descriptor()
            .clone()
    }

    /// The dry run's note on whether the destination tag can move.
    fn tag_note(dialog: &PushDialog) -> String {
        let evaluation = dialog.evaluate();
        evaluation
            .checks
            .iter()
            .find(|c| c.mark == Mark::Note && c.text.contains("written after every blob"))
            .unwrap_or_else(|| panic!("{evaluation:?}"))
            .text
            .clone()
    }

    #[test]
    fn the_dry_run_says_a_set_tag_never_moves_while_aliases_and_machine_tags_move_when_confirmed() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        installation_set(&lab.f, "sets:1.4", 1);
        let dialog = lab.open_push("sets:1.4");
        dialog.destination = format!("{host}/private/sets:1.4");
        let note = tag_note(dialog);
        assert!(
            note.starts_with("1.4 is a tag of an installation media set")
                && note.contains("never moves, not even with confirmation")
                && note.contains("such as 1.4-r2"),
            "{note}"
        );
        dialog.destination = format!("{host}/private/sets:latest");
        let note = tag_note(dialog);
        assert!(
            note.starts_with("latest is not a version") && note.contains("you confirm the move"),
            "{note}"
        );
        // A machine image's tag of the same shape is not a set's tag.
        let dialog = lab.open_push("test:source");
        dialog.destination = format!("{host}/private/test:1.4");
        let note = tag_note(dialog);
        assert!(
            note.starts_with("1.4 is not a version") && note.contains("you confirm the move"),
            "{note}"
        );
    }

    #[test]
    fn a_set_pushed_onto_a_taken_set_tag_fails_without_a_move_confirmation() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        let first = installation_set(&lab.f, "sets:1.4", 1);
        let second = installation_set(&lab.f, "sets:changed", 2);
        lab.push_through("sets:1.4", &format!("{host}/private/sets:1.4"));
        let dialog = lab.open_push("sets:changed");
        dialog.destination = format!("{host}/private/sets:1.4");
        let request = dialog.push().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::Finished { ok, message, .. } = &lab.view.push.as_ref().unwrap().state else {
            panic!("{:?}", lab.view.push.as_ref().unwrap().state)
        };
        assert!(!ok);
        assert!(
            message
                .contains("Tags of installation media sets never move, not even with confirmation")
                && message.contains("such as 1.4-r2")
                && message.contains(&first.digest.to_string())
                && message.contains(&second.digest.to_string()),
            "{message}"
        );
        assert_eq!(
            lab.registry.tag_digest("private/sets", "1.4"),
            Some(first.digest)
        );
    }

    #[test]
    fn a_tag_moved_since_the_dry_run_is_not_pushed_and_the_dry_run_is_built_again() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        let reviewed = lab.f.row.descriptor.clone();
        let dialog = lab.open_push("test:source");
        dialog.destination = format!("{host}/private/test:1.0.0");
        let other = retitled(&lab.f, "test:other");
        pc_image::test_support::with_synthetic_firmware(|| {
            lab.f.store.update_tag(
                "test:source",
                &other,
                Some(&reviewed.digest),
                &pc_image::PcProvider,
            )
        })
        .unwrap();
        let dialog = lab.view.push.as_mut().unwrap();
        let Some(TransferRequest::Push(job)) = dialog.push() else {
            panic!()
        };
        assert_eq!(job.image, reviewed.digest);
        lab.view.send(Request::Transfer(TransferRequest::Push(job)));
        lab.view
            .pump_until(|v| matches!(v.push.as_ref().unwrap().state, PushState::Planning));
        assert_eq!(
            lab.view.push.as_ref().unwrap().notice.as_deref(),
            Some(STALE)
        );
        lab.view
            .pump_until(|v| matches!(v.push.as_ref().unwrap().state, PushState::Ready));
        let dialog = lab.view.push.as_ref().unwrap();
        assert_eq!(dialog.plan().unwrap().image.digest, other.digest);
        assert_eq!(dialog.destination, format!("{host}/private/test:1.0.0"));
        assert_eq!(dialog.notice.as_deref(), Some(STALE));
        assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());
    }

    #[test]
    fn an_interrupted_pull_resumes_by_range_and_writes_the_local_tag_last() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        let image = lab.f.row.descriptor.clone();
        lab.push_through("test:source", &format!("{host}/private/test:1.0.0"));
        let blobs = distinct_blobs(&lab.f.store, &image);
        let largest = blobs.iter().max_by_key(|d| d.size).unwrap().clone();
        let keep = (largest.size / 2) as usize;
        assert!(keep > 0);
        // The largest blob is cut off half-way after the test lets it go.
        lab.registry.inject_blob_fault(BlobFault {
            digest: largest.digest.clone(),
            keep,
        });
        lab.registry
            .hold_next(&format!("GET /v2/private/test/blobs/{}", largest.digest));

        let target = assembly::test_support::TempDir::new("registry-pull");
        let store = Store::open(target.path().join("store")).unwrap();
        let mut view = LibraryView::synthetic(store.root().to_owned());
        view.begin_pull(lab.accounts.clone());
        let dialog = view.pull.as_mut().unwrap();
        dialog.reference = format!("{host}/private/test:1.0.0");
        assert_eq!(dialog.resolve().unwrap().1, "private/test:1.0.0");
        dialog.local_tag = "copy:1.0.0".into();
        let request = dialog.pull().unwrap();
        view.send(Request::Transfer(request));
        let registry = &lab.registry;
        wait_pumping(&mut view, || registry.held());
        // Every other blob is verified while the largest one waits, and the
        // dialog shows them before the pull ends.
        let fetched = |view: &LibraryView| match &view.pull.as_ref().unwrap().state {
            PullState::Pulling(run) => run.progress().fetched.len(),
            _ => usize::MAX,
        };
        let others = blobs.len() - 1;
        let deadline = Instant::now() + crate::test_wait::HANG_GUARD;
        while fetched(&view) != others {
            assert!(Instant::now() < deadline, "{}", fetched(&view));
            view.pump_queued();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(store.tag_descriptor("copy:1.0.0").is_err());
        lab.registry.release();
        view.pump_until(pull_settled);
        let PullState::Finished {
            ok,
            message,
            pulled,
        } = &view.pull.as_ref().unwrap().state
        else {
            panic!()
        };
        assert!(ok, "{message}");
        assert_eq!(pulled.as_deref(), Some("copy:1.0.0"));
        assert!(
            message.contains(&format!("{} blobs fetched", blobs.len())),
            "{message}"
        );
        assert!(
            lab.registry
                .range_requests()
                .contains(&format!("bytes={keep}-")),
            "{:?}",
            lab.registry.range_requests()
        );
        assert_eq!(
            store.tag_descriptor("copy:1.0.0").unwrap().digest,
            image.digest
        );
        assert_eq!(view.status.as_deref(), Some("Pulled copy:1.0.0"));
    }

    #[test]
    fn a_push_cancelled_between_blobs_never_publishes_the_manifest() {
        let mut lab = Lab::new(true);
        let blobs = distinct_blobs(&lab.f.store, &lab.f.row.descriptor).len();
        // One blob goes up after the cancel and at least one must stay behind.
        assert!(blobs >= 2, "{blobs}");
        // The registry holds the first blob's request until the cancel is set,
        // so that blob finishes and the next check stops the push.
        lab.held_push();
        lab.view.push.as_mut().unwrap().cancel();
        lab.registry.release();
        lab.view.pump_until(push_settled);
        let PushState::Finished {
            ok,
            message,
            progress,
        } = &lab.view.push.as_ref().unwrap().state
        else {
            panic!()
        };
        assert!(!ok);
        assert_eq!(message, PUSH_CANCELLED);
        assert!(!progress.published);
        let sent = progress
            .blobs()
            .iter()
            .filter(|b| b.outcome.is_some())
            .count();
        assert_eq!(sent, 1, "{blobs} blobs");
        assert_eq!(lab.registry.tag_digest("private/test", "1.0.0"), None);
        assert!(!lab.manifest_was_put());

        // Pushing again skips the blob already there.
        let dialog = lab.view.push.as_mut().unwrap();
        dialog.back();
        let request = dialog.push().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::Finished { ok, progress, .. } = &lab.view.push.as_ref().unwrap().state
        else {
            panic!()
        };
        assert!(ok);
        let blobs = progress.blobs();
        assert_eq!(
            blobs
                .iter()
                .filter(|b| b.outcome == Some(BlobOutcome::Present))
                .count(),
            1
        );
    }

    #[test]
    fn closing_the_dialog_or_shutting_down_during_a_push_cancels_it() {
        let mut lab = Lab::new(true);
        lab.held_push();
        lab.view.close_dialogs();
        assert!(lab.view.push.is_none());
        lab.registry.release();
        lab.view.pump_until(|v| {
            v.status
                .as_deref()
                .is_some_and(|s| s.starts_with("Push cancelled"))
        });
        assert!(!lab.manifest_was_put());
        assert_eq!(lab.registry.tag_digest("private/test", "1.0.0"), None);

        // Shutting the library down drops the dialog, which cancels its push.
        let cancel = lab.held_push();
        let registry = &lab.registry;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = Instant::now() + crate::test_wait::HANG_GUARD;
                while !cancel.load(Ordering::SeqCst) {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                registry.release();
            });
            lab.view.shutdown();
        });
        assert!(cancel.load(Ordering::SeqCst));
        assert!(lab.view.push.is_none());
    }

    #[test]
    fn a_pull_cancelled_while_blobs_download_never_sets_the_local_tag() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        lab.push_through("test:source", &format!("{host}/private/test:1.0.0"));
        let target = assembly::test_support::TempDir::new("registry-pull-cancel");
        let store = Store::open(target.path().join("store")).unwrap();
        let mut view = LibraryView::synthetic(store.root().to_owned());
        view.begin_pull(lab.accounts.clone());
        let dialog = view.pull.as_mut().unwrap();
        dialog.reference = format!("{host}/private/test:1.0.0");
        // The registry holds the first blob download until the cancel is set.
        lab.registry.hold_next("GET /v2/private/test/blobs/");
        let request = dialog.pull().unwrap();
        view.send(Request::Transfer(request));
        let registry = &lab.registry;
        wait_pumping(&mut view, || registry.held());
        view.pull.as_mut().unwrap().cancel();
        lab.registry.release();
        view.pump_until(pull_settled);
        let PullState::Finished { ok, message, .. } = &view.pull.as_ref().unwrap().state else {
            panic!()
        };
        assert!(!ok);
        assert_eq!(message, PULL_CANCELLED);
        assert!(matches!(
            store.tag_descriptor("private/test:1.0.0"),
            Err(image_store::Error::TagNotFound(_))
        ));

        let dialog = view.pull.as_mut().unwrap();
        dialog.back();
        let request = dialog.pull().unwrap();
        view.send(Request::Transfer(request));
        view.pump_until(pull_settled);
        let PullState::Finished { ok, message, .. } = &view.pull.as_ref().unwrap().state else {
            panic!()
        };
        assert!(ok, "{message}");
        assert_eq!(
            store.tag_descriptor("private/test:1.0.0").unwrap().digest,
            lab.f.row.descriptor.digest
        );
    }

    #[test]
    fn the_default_registry_follows_the_environment_and_a_bad_one_stops_both_dialogs() {
        let temp = assembly::test_support::TempDir::new("registry-default");
        let path = temp.path().join("preferences.toml");
        let accounts = |text: &str, default_override: Option<&str>| {
            std::fs::write(&path, text).unwrap();
            Accounts {
                path: Some(path.clone()),
                store: Arc::new(SessionStore::default()),
                env: EnvCredentials::default(),
                default_override: default_override.map(Into::into),
            }
        };
        let listed = "default_registry = \"nas.example:5000\"\n[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n";
        let (push, _) = PushDialog::open(accounts(listed, None), Some("lab:1".into()));
        assert_eq!(push.default_host, "nas.example:5000");
        let (push, _) =
            PushDialog::open(accounts(listed, Some("env.example")), Some("lab:1".into()));
        assert_eq!(push.default_host, "env.example");
        let mut pull = PullDialog::open(accounts(listed, Some("env.example")));
        pull.reference = "open-at:1.0.0".into();
        assert_eq!(
            pull.resolve().unwrap().0.to_string(),
            "env.example/unemu/open-at:1.0.0"
        );

        for (text, default_override, wanted) in [
            ("default_registry = \"not a host\"\n", None, "Preferences"),
            (listed, Some("not a host"), "Default registry"),
        ] {
            let (push, _) =
                PushDialog::open(accounts(text, default_override), Some("lab:1".into()));
            let refusal = push.evaluate().refusal.unwrap();
            assert!(refusal.starts_with(wanted), "{refusal}");
            assert!(!push.can_push());
            let mut pull = PullDialog::open(accounts(text, default_override));
            pull.reference = "open-at:1.0.0".into();
            assert!(pull.resolve().unwrap_err().starts_with(wanted));
            assert!(!pull.can_pull() && pull.pull().is_none());
        }
    }

    #[test]
    fn a_move_or_stale_answer_after_cancel_shows_the_cancel() {
        let f = Fixture::new();
        for failure in [
            PushFailure::Move {
                tag: "latest".into(),
                current: Digest::of(b"old"),
                new: f.row.descriptor.digest.clone(),
            },
            PushFailure::Stale,
        ] {
            let accounts = accounts_for(
                f.temp.path(),
                "127.0.0.1:5999",
                true,
                Arc::new(SessionStore::default()),
            );
            let (mut dialog, request) = PushDialog::open(accounts, Some("test:source".into()));
            for event in run_here(&f.store, request, &[]) {
                dialog.on_event(&event);
            }
            dialog.destination = "127.0.0.1:5999/private/test:latest".into();
            let Some(TransferRequest::Push(job)) = dialog.push() else {
                panic!()
            };
            dialog.cancel();
            let follow_up = dialog.on_event(&TransferEvent::Pushed {
                id: job.id,
                result: Err(failure),
            });
            assert!(follow_up.is_none());
            assert!(
                matches!(&dialog.state, PushState::Finished { ok: false, message, .. } if message == PUSH_CANCELLED),
                "{:?}",
                dialog.state
            );
        }
    }

    #[test]
    fn blob_status_names_the_chunk_and_what_a_cancel_waits_for() {
        let chunk = push::DEFAULT_CHUNK_SIZE;
        let large = BlobRow {
            digest: Digest::of(b"large"),
            label: "disk base".into(),
            size: 3 * chunk + 1,
            sent: chunk,
            outcome: None,
        };
        assert_eq!(
            large.status(true, false, false),
            "chunk 2 of 4 · 64.0 MiB of 192.0 MiB"
        );
        assert!(large
            .status(true, false, true)
            .starts_with("Stopping — chunk 2 of 4"));
        assert_eq!(large.status(false, false, false), "Queued");
        assert_eq!(large.status(false, true, false), "Not sent");
        let small = BlobRow {
            size: 4096,
            sent: 0,
            ..large.clone()
        };
        assert_eq!(small.status(true, false, false), "Uploading · single chunk");
        assert_eq!(
            small.status(true, false, true),
            "Stopping after this upload request"
        );
        let done = BlobRow {
            outcome: Some(BlobOutcome::Uploaded),
            ..small
        };
        assert_eq!(done.status(true, true, true), "Uploaded ✓");
    }

    /// A session store that notes which threads read a secret.
    #[derive(Default)]
    struct Recording {
        inner: SessionStore,
        readers: Mutex<Vec<std::thread::ThreadId>>,
    }
    impl CredentialStore for Recording {
        fn kind(&self) -> StoreKind {
            StoreKind::Session
        }
        fn read(&self, host: &str, user: &str) -> image_registry::Result<Option<Secret>> {
            self.readers
                .lock()
                .unwrap()
                .push(std::thread::current().id());
            self.inner.read(host, user)
        }
        fn store(&self, host: &str, user: &str, secret: &Secret) -> image_registry::Result<()> {
            self.inner.store(host, user, secret)
        }
        fn delete(&self, host: &str, user: &str) -> image_registry::Result<()> {
            self.inner.delete(host, user)
        }
    }

    #[test]
    fn registry_requests_and_secret_reads_never_run_on_the_frame_loop() {
        // A registry that accepts the connection and never answers: the frame
        // loop goes on while the worker waits for it.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        silent.set_nonblocking(true).unwrap();
        let host = silent.local_addr().unwrap().to_string();
        let f = Fixture::new();
        let accounts = accounts_for(
            f.temp.path(),
            &host,
            true,
            Arc::new(SessionStore::default()),
        );
        let mut view = LibraryView::synthetic(f.store.root().to_owned());
        view.begin_pull(accounts);
        let dialog = view.pull.as_mut().unwrap();
        dialog.reference = format!("{host}/private/test:1.0.0");
        let request = dialog.pull().unwrap();
        view.send(Request::Transfer(request));
        let mut connection = None;
        wait_pumping(&mut view, || {
            connection = silent.accept().ok().map(|(stream, _)| stream);
            connection.is_some()
        });
        let ctx = egui::Context::default();
        for _ in 0..3 {
            view.poll(&ctx);
        }
        assert!(view.pull.as_ref().unwrap().running());
        view.pull.as_mut().unwrap().cancel();
        drop(connection);
        drop(silent);
        view.pump_until(pull_settled);
        assert!(matches!(
            view.pull.as_ref().unwrap().state,
            PullState::Finished { ok: false, .. }
        ));

        // The secret is read when the registry challenges, on the worker.
        let mut lab = Lab::new(true);
        let recording = Arc::new(Recording::default());
        lab.accounts = accounts_for(lab.f.temp.path(), &lab.host(), true, recording.clone());
        let host = lab.host();
        lab.push_through("test:source", &format!("{host}/private/test:1.0.0"));
        let readers = recording.readers.lock().unwrap().clone();
        assert!(!readers.is_empty());
        assert!(readers
            .iter()
            .all(|reader| *reader != std::thread::current().id()));
    }
}
