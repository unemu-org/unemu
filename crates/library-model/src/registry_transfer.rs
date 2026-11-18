// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless model of Image › Push… and Image › Pull…;
//! `backlog/docs/design/machine-images.md` §5.1 and the approved
//! registry-accounts mock-up.
//!
//! Push… starts from a dry run that the library worker builds with the
//! command line's own plan ([`push::plan_with_catalogue`], with the blobs
//! every platform restricts, [`DesktopPlatform::restricted`]): every blob of the chosen
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
//! the command line's own client
//! ([`image_registry::accounts::registry_client`]); no secret ever
//! reaches a dialog. Cancel sets a flag the transfer checks before each blob,
//! between the chunks of a large upload and before it publishes, so a
//! cancelled push never publishes its manifest and a cancelled pull never
//! writes its local tag.

use crate::{
    listing::human_size,
    platform::{self, DesktopPlatform, PlatformFacts},
    registries_model::{Check, Mark},
};
use image_registry::{
    config::{self, RegistryConfig, Scope},
    credential_store::CredentialStore,
    credentials::EnvCredentials,
    guard::{self, ClosureEntry, MarkSource, Redistribution, Refusal},
    pull::{self, PullOutcome, PullReport},
    push::{self, BlobOutcome, PushOptions, PushProgress, PushReport, TagOutcome},
    reference::{is_version_tag, OFFICIAL_NAMESPACE},
    Error, Reference, RegistryEntry, DEFAULT_REGISTRY,
};
use image_store::{media_type as mt, tag as set_tag, ConfigValidator, Descriptor, Digest, Store};
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
pub struct Accounts {
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
    pub fn settings(&self) -> Result<(RegistryConfig, String), String> {
        let config = self.config()?;
        let host = config
            .default_host(None, self.default_override.as_deref())
            .map_err(|e| format!("Default registry: {e}"))?;
        Ok((config, host))
    }
}

/// Job ids are unique across dialogs, so a late event of a closed dialog
/// never changes a newer one.
pub fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Progress the worker writes while a transfer runs and the dialog reads each
/// frame. Writing never waits for the frame loop, so a slow frame never
/// stalls a transfer; only the final event goes through the worker's queue.
#[derive(Debug, Default)]
pub struct Live<T>(Arc<Mutex<T>>);

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
pub enum TransferRequest {
    /// List the local images and, for `local_tag`, build the dry run.
    Plan {
        id: u64,
        local_tag: Option<String>,
    },
    Push(Box<PushJob>),
    Pull(Box<PullJob>),
}

pub struct PushJob {
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

pub struct PullJob {
    pub id: u64,
    pub source: Reference,
    pub local_tag: String,
    pub accounts: Accounts,
    pub cancel: Arc<AtomicBool>,
    pub live: Live<PullLive>,
}

/// The final answers of the worker; progress goes through [`Live`].
pub enum TransferEvent {
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
}

/// A local tag the Push… dialog can choose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImage {
    pub tag: String,
    pub digest: Digest,
    pub kind: &'static str,
}

#[derive(Clone, Debug)]
pub struct Planned {
    pub images: Vec<LocalImage>,
    pub plan: Option<PlanData>,
}

/// The dry run of one local image.
#[derive(Clone, Debug)]
pub struct PlanData {
    pub local_tag: String,
    pub image: Descriptor,
    pub closure: Vec<ClosureEntry>,
    /// The image is an installation media set or a firmware package, whose
    /// revision tags never move on the registry and whose release aliases
    /// move only forward ([`image_store::tag`]).
    pub revisions: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushFailure {
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
pub enum PullFailure {
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
/// the dialogs happens here. `platforms` validate what moves and name the
/// blobs whose redistribution they restrict; `repaint` asks for a frame
/// after progress changed.
pub fn run(
    store: &Store,
    request: TransferRequest,
    platforms: &[&dyn DesktopPlatform],
    send: &(dyn Fn(TransferEvent) + Sync),
    repaint: &(dyn Fn() + Sync),
) {
    let providers = &platform::validators(platforms);
    match request {
        TransferRequest::Plan { id, local_tag } => send(TransferEvent::Planned {
            id,
            result: plan(store, local_tag.as_deref(), platforms).map(Box::new),
        }),
        TransferRequest::Push(job) => {
            let live = job.live.clone();
            let result = run_push(store, &job, providers, platforms, &mut |event| {
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
pub fn dry_run_target() -> Reference {
    Reference {
        host: DEFAULT_REGISTRY.into(),
        repository: format!("{OFFICIAL_NAMESPACE}/dry-run"),
        tag: Some("dry-run".into()),
        digest: None,
    }
}

/// The push plan of `local_tag`: every local mark, plus a restriction for
/// each blob a platform restricts.
fn push_plan(
    store: &Store,
    local_tag: &str,
    target: Reference,
    entry: Option<&RegistryEntry>,
    platforms: &[&dyn DesktopPlatform],
) -> image_registry::Result<push::PushPlan> {
    push::plan_with_catalogue(store, local_tag, target, entry, |manifest| {
        platform::restricted(platforms, store, manifest)
    })
}

fn plan(
    store: &Store,
    local_tag: Option<&str>,
    platforms: &[&dyn DesktopPlatform],
) -> Result<Planned, String> {
    let images = local_images(store)?;
    let plan = match local_tag {
        Some(tag) => {
            let plan = push_plan(store, tag, dry_run_target(), None, platforms)
                .map_err(|e| e.to_string())?;
            Some(PlanData {
                local_tag: tag.into(),
                image: plan.image().clone(),
                closure: plan.closure().to_vec(),
                revisions: set_tag::governs(plan.manifest().artifact_type()),
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
    platforms: &[&dyn DesktopPlatform],
    progress: &mut dyn FnMut(PushProgress<'_>),
) -> Result<PushReport, PushFailure> {
    // Preferences are read again: a registry un-marked private since the
    // dialog opened must not receive what it may no longer hold.
    let config = job.accounts.config().map_err(PushFailure::Failed)?;
    let entry = config.entry(&job.target.host);
    let plan = push_plan(
        store,
        &job.local_tag,
        job.target.clone(),
        entry.as_ref(),
        platforms,
    )
    .map_err(|e| push_failure(e, &[]))?;
    if plan.image().digest != job.image {
        return Err(PushFailure::Stale);
    }
    if let Err(refusal) = plan.verdict() {
        return Err(PushFailure::Refused(refusal_text(refusal, plan.closure())));
    }
    let client = image_registry::accounts::registry_client(
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
    let client = image_registry::accounts::registry_client(
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
        Error::ImmutableRevisionTag { tag, current, new } => PushFailure::Failed(format!(
            "Revision tag {tag} already names {current} on the registry, and this image is {new}. Revision tags of installation media sets and firmware packages never move, not even with confirmation: push a changed one under a new revision tag{}.",
            set_tag::next_revision(&tag)
                .map(|next| format!(" such as {next}"))
                .unwrap_or_default()
        )),
        Error::NoLatest => PushFailure::Failed(
            "Installation media sets and firmware packages have no latest tag: push a revision tag such as r1 or 1.0-r1, then its release alias.".into(),
        ),
        Error::AliasNotForward { tag, reason } => PushFailure::Failed(format!(
            "Release alias {tag} moves only to the newest revision of its release that the registry already has: {reason}."
        )),
        Error::OutsideTagScheme { tag } => PushFailure::Failed(format!(
            "{tag} is neither a revision tag ([RELEASE-]rN) nor a release alias: installation media sets and firmware packages take only those."
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

/// A local tag the store refused, explained: its length, its registry host,
/// or the grammar, which allows a host first.
pub(crate) fn local_tag_problem(local: &str) -> String {
    let name = local.rsplit_once(':').map_or(local, |(name, _)| name);
    if name.len() > 255 {
        return format!(
            "Local tag “{local}” is too long: the name, registry host included, has at most 255 characters."
        );
    }
    if let Some(Err(image_store::Error::Invalid(why))) =
        Store::tag_host(local).map(Store::validate_registry_host)
    {
        return format!("Local tag “{local}” is not valid: {why}.");
    }
    format!(
        "Local tag “{local}” is not valid: use name:tag, or host[:port]/name:tag, with the name in lowercase letters and digits and a tag that follows the registry tag grammar."
    )
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

/// The blob type as a person names it: the neutral media types first, then
/// a platform's own payload types by the name the platform states.
pub fn kind_label(facts: &PlatformFacts, descriptor: &Descriptor) -> String {
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
        other => facts.payload_label(other).unwrap_or(other),
    };
    if descriptor.is_compressed() {
        format!("{name}, zstd")
    } else {
        name.into()
    }
}

pub fn short(digest: &Digest) -> String {
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
pub struct DryRunRow {
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
pub struct Evaluation {
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
pub struct BlobRow {
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
        let human = human_size;
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
pub struct Progress {
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
pub enum PushState {
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

pub struct PushDialog {
    accounts: Accounts,
    /// What the dry run and the progress name blob types by.
    pub facts: PlatformFacts,
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
    pub fn open(
        accounts: Accounts,
        facts: PlatformFacts,
        local_tag: Option<String>,
    ) -> (Self, TransferRequest) {
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
            facts,
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
                kind: kind_label(&self.facts, &entry.descriptor),
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
                    if plan.revisions && set_tag::is_revision(&tag) {
                        format!(
                            "{tag} is a revision tag: once it names an image on the registry it never moves, not even with confirmation; push a changed one under the next revision{}. The manifest and the tag are written after every blob",
                            set_tag::next_revision(&tag)
                                .map(|next| format!(", {next}"))
                                .unwrap_or_default()
                        )
                    } else if plan.revisions && tag == set_tag::LATEST {
                        "Installation media sets and firmware packages have no latest tag: push a revision tag such as r1 or 1.0-r1, then its release alias".to_owned()
                    } else if plan.revisions && set_tag::parse(&tag).is_some() {
                        format!(
                            "{tag} is the alias of a release: it names the newest revision of {tag} the registry already has, and moves only forward to a newer one, after you confirm the move with both digests. The manifest and the tag are written after every blob"
                        )
                    } else if plan.revisions {
                        format!(
                            "{tag} is neither a revision tag ([RELEASE-]rN) nor a release alias: installation media sets and firmware packages take only those, so the push is refused"
                        )
                    } else if is_version_tag(&tag) {
                        format!(
                            "{tag} is a version tag: once it names an image on the registry it never moves. The manifest and the tag are written after every blob"
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
                        kind_label(&self.facts, &entry.descriptor),
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
pub struct PullLive {
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
pub struct PullRun {
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
    pub fn start(
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

    pub fn id(&self) -> u64 {
        self.id
    }

    /// The progress as the worker reports it now.
    pub fn progress(&self) -> PullLive {
        self.live.read(|state| state.clone())
    }

    /// Ask the pull to stop at its next check; it reports back.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.cancelling = true;
    }
}

#[derive(Debug)]
pub enum PullState {
    Ready,
    Pulling(PullRun),
    Finished {
        ok: bool,
        message: String,
        /// The local tag the pull wrote.
        pulled: Option<String>,
    },
}

pub struct PullDialog {
    accounts: Accounts,
    /// What the progress names blob types by.
    pub facts: PlatformFacts,
    config: RegistryConfig,
    pub default_host: String,
    pub reference: String,
    /// Empty: the reference's own local tag, short from the family default
    /// registry and host-qualified from any other.
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
    pub fn open(accounts: Accounts, facts: PlatformFacts) -> Self {
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
            facts,
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
        Store::validate_tag_name(&local).map_err(|_| local_tag_problem(&local))?;
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
    // A set or package pulled by its release writes the revision, and the
    // local alias of the release moves forward to it.
    let alias = report
        .alias
        .as_ref()
        .map(|alias| format!(" The local alias {alias} moved to it."))
        .unwrap_or_default();
    match report.outcome {
        PullOutcome::Unchanged => format!(
            "{} already names this image; nothing was fetched.{alias}",
            report.local_tag
        ),
        PullOutcome::Pulled => format!(
            "Pulled {source} as {} ({}): {} blobs fetched ({}), {} already in the store. The tag was written after every blob was verified.{alias}",
            report.local_tag,
            short(&report.image.digest),
            report.fetched.len(),
            human_size(report.fetched.iter().map(|d| d.size).sum()),
            report.present.len()
        ),
    }
}

/// What a closed dialog's transfer ended with, for the status bar.
pub fn closed_outcome(event: &TransferEvent) -> Option<&'static str> {
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

/// The texts the tests of the desktop's transfer dialogs compare with.
#[cfg(feature = "test-support")]
pub mod test_support {
    pub const NO_OVERRIDE: &str = super::NO_OVERRIDE;
    pub const PUSH_CANCELLED: &str = super::PUSH_CANCELLED;
    pub const PULL_CANCELLED: &str = super::PULL_CANCELLED;
    pub const STALE: &str = super::STALE;
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_registry::credential_store::SessionStore;

    #[test]
    fn a_refused_local_tag_says_what_is_wrong_with_it() {
        let long = format!("nas.example:5000/{}:1", "a".repeat(240));
        assert!(
            local_tag_problem(&long)
                .contains("too long: the name, registry host included, has at most 255 characters"),
            "{}",
            local_tag_problem(&long)
        );
        let port = local_tag_problem("nas.example:05000/x:1");
        assert!(
            port.contains("nas.example:05000 is not a valid registry host")
                && port.contains("without leading zeros"),
            "{port}"
        );
        let grammar = local_tag_problem("Lab:1");
        assert!(grammar.contains("host[:port]/name:tag"), "{grammar}");
    }

    #[test]
    fn the_default_registry_follows_the_environment_and_a_bad_one_stops_both_dialogs() {
        let temp = image_store::test_support::TempDir::new("registry-default");
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
        let (push, _) = PushDialog::open(
            accounts(listed, None),
            PlatformFacts::default(),
            Some("lab:1".into()),
        );
        assert_eq!(push.default_host, "nas.example:5000");
        let (push, _) = PushDialog::open(
            accounts(listed, Some("env.example")),
            PlatformFacts::default(),
            Some("lab:1".into()),
        );
        assert_eq!(push.default_host, "env.example");
        let mut pull = PullDialog::open(
            accounts(listed, Some("env.example")),
            PlatformFacts::default(),
        );
        pull.reference = "open-at:1.0.0".into();
        assert_eq!(
            pull.resolve().unwrap().0.to_string(),
            "env.example/unemu/open-at:1.0.0"
        );

        for (text, default_override, wanted) in [
            ("default_registry = \"not a host\"\n", None, "Preferences"),
            (listed, Some("not a host"), "Default registry"),
        ] {
            let (push, _) = PushDialog::open(
                accounts(text, default_override),
                PlatformFacts::default(),
                Some("lab:1".into()),
            );
            let refusal = push.evaluate().refusal.unwrap();
            assert!(refusal.starts_with(wanted), "{refusal}");
            assert!(!push.can_push());
            let mut pull =
                PullDialog::open(accounts(text, default_override), PlatformFacts::default());
            pull.reference = "open-at:1.0.0".into();
            assert!(pull.resolve().unwrap_err().starts_with(wanted));
            assert!(!pull.can_pull() && pull.pull().is_none());
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
}
