// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless model of File › Get templates…, following the approved
//! get-templates mock-up; `backlog/docs/design/machine-images.md` §5.5
//! (official template images) and §5.1 (registry rules).
//!
//! The dialog lists the official templates of the default registry: the
//! machine images among the repositories the registry catalog lists under
//! `unemu/`. The catalog is the Docker Registry HTTP API V2 `_catalog`
//! endpoint, paged with `n` and `Link`; the OCI Distribution Specification
//! has none, and zot lists in it the repositories the caller may read, so a
//! stored login goes with the first request. Firmware packages
//! (`unemu/firmware/…`) and the live registry test's repositories
//! (`unemu/test-…`) are left out by name before any request; media sets and
//! anything else not a PC machine image by its manifest. Each repository
//! offers the version tag `latest` names when it is one of the 16 newest
//! releases, else its newest release (Semantic Versioning 2.0.0 §11
//! precedence; pre-releases only when it has no release). A repository that
//! fails to answer, or that this account may not read, is skipped; an
//! unreachable registry, a refused or missing login or a registry without a
//! catalog ends the listing with one message. The two login cases offer
//! Log in…, or Add registry… when preferences have no entry for the
//! registry, after which the dialog lists again. A row shows the name, tag,
//! title, description and download size from the manifest. The chosen
//! templates are pulled one after another through the Pull… path
//! ([`PullRun`]), by the digest the row showed, so every blob is verified and
//! each local tag is written last; one progress bar covers them all and
//! counts blobs already in the store as done. Cancel stops the running
//! pull at its next read and the queued ones before they contact the
//! registry. Listing and pulls run on the library worker; the listing checks
//! a cancel flag between repositories, which closing the dialog sets, and
//! uses the short timeouts of a login check, so an offline or unreachable
//! registry ends in a message instead of a long wait.

use crate::{
    library_view::human_size,
    registry_transfer::{
        common_failure, kind_label, next_id, pulled_text, Accounts, PullFailure, PullRun,
        TransferEvent, TransferRequest,
    },
};
use image_registry::{
    pull, reference::OFFICIAL_NAMESPACE, Client, Error, Reference, DEFAULT_REGISTRY,
};
use image_store::{media_type as mt, Descriptor, Digest, Manifest, Store};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Whether the local store already has the template's tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Local {
    Absent,
    /// The local tag names this image.
    Same,
    /// The local tag names another image, so a pull would be refused.
    Other,
}

/// One official template on the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateRow {
    pub reference: Reference,
    /// The repository name inside `unemu/`.
    pub name: String,
    pub tag: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// The manifest and every distinct blob it names, as stored.
    pub size: u64,
    pub digest: Digest,
    /// The short-form tag a pull writes.
    pub local_tag: String,
    pub local: Local,
    /// The distinct blobs the manifest names.
    pub blobs: Vec<Descriptor>,
    /// Those of them the library held when the list was read; a pull does
    /// not fetch them again.
    pub stored: BTreeSet<Digest>,
}

/// Why a listing ended without a list; each kind has its own heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// No answer: offline, a refused connection or a timeout.
    Unreachable,
    /// The registry refused the stored login (401).
    Refused,
    /// The registry asks for a login and none is stored.
    LoginRequired,
    /// The registry answers the catalog with 404, so it lists nothing.
    NoCatalog,
    /// Preferences, the library, the worker or any other answer.
    Other,
}

/// A listing that ended without a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListingFailure {
    pub kind: FailureKind,
    /// The registry that was listed.
    pub host: String,
    /// Preferences › Registries has an entry for the host, so its Log in
    /// dialog can open; without one the registry must be added first.
    pub listed: bool,
    pub message: String,
}

impl ListingFailure {
    fn other(host: &str, message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Other,
            host: host.into(),
            listed: false,
            message: message.into(),
        }
    }

    fn wants_login(&self) -> bool {
        matches!(self.kind, FailureKind::Refused | FailureKind::LoginRequired)
    }

    pub fn heading(&self) -> &'static str {
        match self.kind {
            FailureKind::Unreachable => "Could not reach the registry",
            FailureKind::Refused => "The registry refused the login",
            FailureKind::LoginRequired => "The registry asks for a login",
            FailureKind::NoCatalog => "This registry offers no templates",
            FailureKind::Other => "Could not list the templates",
        }
    }

    /// Log in… is offered when a login would let the listing through and
    /// the registry has an entry to log in on.
    pub fn offers_login(&self) -> bool {
        self.wants_login() && self.listed
    }

    /// Add registry… is offered instead when the registry has no entry.
    pub fn offers_add(&self) -> bool {
        self.wants_login() && !self.listed
    }

    /// The line under the message.
    pub fn hint(&self) -> String {
        match self.kind {
            FailureKind::NoCatalog if self.host == DEFAULT_REGISTRY => {
                "Templates are listed from the registry catalog.".into()
            }
            FailureKind::NoCatalog => format!(
                "Templates are listed from the registry catalog. {DEFAULT_REGISTRY} has one; choose it as the default registry to get the official templates."
            ),
            FailureKind::Refused | FailureKind::LoginRequired if self.listed => {
                "Nothing was pulled. After Log in…, the dialog lists again.".into()
            }
            FailureKind::Refused | FailureKind::LoginRequired => {
                "Nothing was pulled. After Add registry… and a login there, the dialog lists again."
                    .into()
            }
            FailureKind::Unreachable | FailureKind::Other => "Nothing was pulled.".into(),
        }
    }
}

/// Repositories of the namespace that are not templates by name: firmware
/// packages and the live registry test's repositories.
fn not_a_template(name: &str) -> bool {
    name.starts_with("firmware/") || name.starts_with("test-")
}

/// Version tags newest first by Semantic Versioning 2.0.0 precedence (§11),
/// releases only when the repository has any; pre-releases otherwise.
fn versions(tags: &[String]) -> Vec<String> {
    let mut versions: Vec<_> = tags
        .iter()
        .filter_map(|tag| pc_image::template::precedence(tag).map(|p| (p, tag.clone())))
        .collect();
    if versions.iter().any(|(p, _)| p.is_release()) {
        versions.retain(|(p, _)| p.is_release());
    }
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    versions.into_iter().map(|(_, tag)| tag).collect()
}

/// At most this many version tags are compared with `latest`.
const LATEST_LOOKUPS: usize = 16;

/// The tag a row offers: the version tag `latest` names, else the newest
/// version, else `latest` itself.
fn offered_tag(
    client: &Client,
    repository: &str,
    tags: &[String],
) -> Result<Option<String>, Error> {
    let candidates = versions(tags);
    let has_latest = tags.iter().any(|t| t == "latest");
    if has_latest {
        if let Some(latest) = client.manifest_digest(repository, "latest")? {
            for tag in candidates.iter().take(LATEST_LOOKUPS) {
                if client.manifest_digest(repository, tag)?.as_ref() == Some(&latest) {
                    return Ok(Some(tag.clone()));
                }
            }
        }
    }
    Ok(candidates
        .into_iter()
        .next()
        .or_else(|| has_latest.then(|| "latest".to_owned())))
}

/// Errors that end the whole listing: the registry cannot be reached, or
/// refuses this client. Any other failure skips only its repository, also a
/// repository this account may not read.
fn fatal(error: &Error) -> bool {
    matches!(
        error,
        Error::Transport(_)
            | Error::AuthRequired { .. }
            | Error::Unauthorized { .. }
            | Error::UnsupportedAuth(_)
            | Error::Credentials(_)
    )
}

/// What the dialog says when the registry cannot be listed; `listed` tells
/// whether preferences have an entry for `host`.
fn listing_failure(host: &str, listed: bool, error: Error) -> ListingFailure {
    let kind = match &error {
        Error::Transport(_) => FailureKind::Unreachable,
        Error::Unauthorized { .. } => FailureKind::Refused,
        Error::AuthRequired { .. } => FailureKind::LoginRequired,
        Error::Status { status: 404, .. } => FailureKind::NoCatalog,
        _ => FailureKind::Other,
    };
    let message = match &error {
        Error::AuthRequired { .. } if !listed => Some(format!(
            "{host} asks for a login, and it is not in your registry list. Add it under Tools › Preferences › Registries and log in, then try again."
        )),
        _ => common_failure(&error),
    };
    let message = message.unwrap_or_else(|| match error {
        Error::Transport(message) => format!(
            "Could not reach {host}: {message}. Check the network connection, or choose another default registry under Tools › Preferences › Registries."
        ),
        Error::Status { status: 404, .. } => {
            format!("{host} does not list its repositories, so it offers no templates here.")
        }
        other => format!("Could not list the templates on {host}: {other}."),
    });
    ListingFailure {
        kind,
        host: host.into(),
        listed,
        message,
    }
}

/// One repository's row, `None` when it offers no PC machine image.
fn row(
    client: &Client,
    store: &Store,
    host: &str,
    repository: &str,
    name: &str,
) -> Result<Option<TemplateRow>, Error> {
    let Some(tag) = offered_tag(
        client,
        repository,
        &pull::list_tags(client, repository, 100)?,
    )?
    else {
        return Ok(None);
    };
    let Some(fetched) = client.manifest(repository, &tag)? else {
        return Ok(None);
    };
    // Anything the client cannot read as a PC machine image is not offered.
    let Ok(manifest) = Manifest::decode_for_summary(&fetched.bytes) else {
        return Ok(None);
    };
    if manifest.artifact_type() != mt::MACHINE || manifest.platform().ok() != Some("ibm-pc-at") {
        return Ok(None);
    }
    // The pull fetches the very manifest the row describes.
    let reference = Reference {
        host: host.into(),
        repository: repository.into(),
        tag: Some(tag.clone()),
        digest: Some(fetched.digest.clone()),
    };
    let Some(local_tag) = reference.local_tag() else {
        return Ok(None);
    };
    let mut seen = BTreeSet::new();
    let blobs: Vec<Descriptor> = std::iter::once(&manifest.config)
        .chain(&manifest.layers)
        .filter(|d| seen.insert(d.digest.clone()))
        .cloned()
        .collect();
    let size = blobs
        .iter()
        .fold(fetched.bytes.len() as u64, |n, d| n.saturating_add(d.size));
    // The same check a pull makes before it fetches anything.
    let mut stored = BTreeSet::new();
    for blob in &blobs {
        match store.open_blob(&blob.digest, blob.size) {
            Ok(_) => {
                stored.insert(blob.digest.clone());
            }
            Err(image_store::Error::MissingBlob { .. }) => {}
            Err(error) => return Err(Error::Store(error)),
        }
    }
    let local = match store.tag_descriptor(&local_tag) {
        Ok(current) if current.digest == fetched.digest => Local::Same,
        Ok(_) => Local::Other,
        Err(image_store::Error::TagNotFound(_)) => Local::Absent,
        Err(error) => return Err(Error::Store(error)),
    };
    let annotation = |key: &str| manifest.annotations.get(key).cloned();
    Ok(Some(TemplateRow {
        name: name.into(),
        tag,
        title: annotation("org.opencontainers.image.title"),
        description: annotation("org.opencontainers.image.description"),
        size,
        digest: fetched.digest,
        local_tag,
        local,
        blobs,
        stored,
        reference,
    }))
}

/// List the official templates of the default registry; runs on the library
/// worker, checking `cancel` between repositories. `store` tells which
/// templates are already in the library; a store that cannot be read ends
/// the listing.
pub(crate) fn list(
    store: &Store,
    accounts: &Accounts,
    cancel: &AtomicBool,
) -> Result<Vec<TemplateRow>, ListingFailure> {
    let (config, host) = accounts
        .settings()
        .map_err(|message| ListingFailure::other(DEFAULT_REGISTRY, message))?;
    let listed = config.entry(&host).is_some();
    let failure = |error| listing_failure(&host, listed, error);
    let client = cli::registry_probe_client(&config, &host, &accounts.store, &accounts.env)
        .map_err(failure)?;
    // Registries answer an anonymous catalog request with their anonymous
    // view instead of a challenge, so a stored login must go first.
    client.use_credentials_now().map_err(failure)?;
    let prefix = format!("{OFFICIAL_NAMESPACE}/");
    let mut rows = Vec::new();
    for repository in pull::list_repositories(&client, 100).map_err(failure)? {
        if cancel.load(Ordering::SeqCst) {
            return Err(ListingFailure::other(&host, "Cancelled."));
        }
        let Some(name) = repository
            .strip_prefix(&prefix)
            .filter(|name| !not_a_template(name))
        else {
            continue;
        };
        match row(&client, store, &host, &repository, name) {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => {}
            Err(Error::Store(error)) => {
                return Err(ListingFailure::other(
                    &host,
                    format!("The library could not be read: {error}"),
                ))
            }
            Err(error) if fatal(&error) => return Err(failure(error)),
            Err(_) => {}
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// One chosen template's pull.
#[derive(Debug)]
pub(crate) struct TemplatePull {
    pub name: String,
    pub tag: String,
    /// The size its row showed: the manifest and its distinct blobs.
    pub size: u64,
    blobs: Vec<Descriptor>,
    /// Blobs the library held when the list was read.
    stored: BTreeSet<Digest>,
    pub run: PullRun,
    /// The worker's answer.
    pub result: Option<Result<String, PullFailure>>,
}

impl TemplatePull {
    /// Bytes in the store so far, at most the size the row showed: blobs
    /// that were there before it (`before`), the ones it verified and the
    /// part of those still arriving.
    fn received(&self, before: &BTreeSet<Digest>) -> u64 {
        let live = self.run.progress();
        let verified: BTreeSet<_> = live.fetched.iter().map(|d| &d.digest).collect();
        let done: u64 = self
            .blobs
            .iter()
            .filter(|b| before.contains(&b.digest) || verified.contains(&b.digest))
            .map(|b| b.size)
            .sum();
        let staged: u64 = live.receiving.iter().map(|(_, have)| *have).sum();
        done.saturating_add(staged).min(self.size)
    }

    /// Where it comes from and the local tag it writes.
    pub fn route(&self) -> String {
        let source = &self.run.source;
        format!(
            "{}/{}:{} → {} · {}",
            source.host,
            source.repository,
            self.tag,
            self.run.local_tag,
            human_size(self.size)
        )
    }

    /// The line under the template: what arrived so far, or how it ended.
    fn detail(&self, stage: Stage, before: &BTreeSet<Digest>) -> String {
        match &self.result {
            Some(Ok(message)) => message.clone(),
            Some(Err(PullFailure::Failed(message))) => format!("{}: {message}", self.name),
            Some(Err(cancelled)) => cancelled.message().into(),
            None if stage == Stage::Waiting => {
                "Starts when the pull before it has answered.".into()
            }
            None => {
                let live = self.run.progress();
                let verified: u64 = live.fetched.iter().map(|d| d.size).sum();
                let mut line = format!(
                    "{} blobs verified ({})",
                    live.fetched.len(),
                    human_size(verified)
                );
                if let Some((first, _)) = live.receiving.first() {
                    let staged = live.receiving.iter().map(|(_, have)| *have).sum();
                    line += &format!(" · {} arriving: {}", human_size(staged), kind_label(first));
                }
                let already = self
                    .blobs
                    .iter()
                    .filter(|b| before.contains(&b.digest))
                    .count();
                if already > 0 {
                    line += &format!(" · {already} already in the store");
                }
                line
            }
        }
    }
}

/// Where one chosen template's pull stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Queued behind the pull before it.
    Waiting,
    Pulling,
    /// Cancel was asked; the pull stops at its next read.
    Stopping,
    Pulled,
    Failed,
    Cancelled,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::Pulling => "Pulling",
            Self::Stopping => "Stopping…",
            Self::Pulled => "Pulled ✓",
            Self::Failed => "Failed ✗",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// The note once every pull answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    /// Every chosen template was added.
    pub complete: bool,
    pub heading: String,
    pub text: String,
}

#[derive(Debug)]
pub(crate) enum TemplatesState {
    /// The worker lists the registry.
    Listing,
    /// The list is shown; the chosen rows can be pulled.
    Ready,
    /// The registry could not be listed.
    Failed(ListingFailure),
    /// The chosen templates, pulled one after another, until each answered.
    Pulling(Vec<TemplatePull>),
}

pub(crate) struct TemplatesDialog {
    accounts: Accounts,
    pub default_host: String,
    pub rows: Vec<TemplateRow>,
    /// Chosen rows, by index.
    pub chosen: BTreeSet<usize>,
    pub state: TemplatesState,
    /// The running listing's id and its cancel flag.
    listing: Option<(u64, Arc<AtomicBool>)>,
    /// Log in… opened Preferences; closing it lists again.
    relist_after_login: bool,
}

impl Drop for TemplatesDialog {
    /// Closing the dialog, also for an exit, stops its listing and its pulls.
    fn drop(&mut self) {
        self.stop_listing();
        self.cancel();
    }
}

/// `a`, `a and b`, `a, b and c`.
fn joined(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl TemplatesDialog {
    /// Open and, unless preferences cannot be read, ask the worker for the list.
    pub fn open(accounts: Accounts) -> (Self, Option<TransferRequest>) {
        let mut dialog = Self {
            default_host: DEFAULT_REGISTRY.into(),
            accounts,
            rows: Vec::new(),
            chosen: BTreeSet::new(),
            state: TemplatesState::Listing,
            listing: None,
            relist_after_login: false,
        };
        let request = dialog.relist();
        (dialog, request)
    }

    /// List the registry again, for Try again and after the pulls.
    pub fn relist(&mut self) -> Option<TransferRequest> {
        if self.running() {
            return None;
        }
        self.stop_listing();
        match self.accounts.settings() {
            Ok((_, host)) => self.default_host = host,
            Err(error) => {
                self.state =
                    TemplatesState::Failed(ListingFailure::other(&self.default_host, error));
                return None;
            }
        }
        let id = next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        self.listing = Some((id, cancel.clone()));
        self.state = TemplatesState::Listing;
        Some(TransferRequest::Templates {
            id,
            accounts: Box::new(self.accounts.clone()),
            cancel,
        })
    }

    /// Log in… or Add registry… of a login failure: the registry Preferences
    /// opens on. The dialog lists again once Preferences closes.
    pub fn log_in(&mut self) -> Option<String> {
        let TemplatesState::Failed(failure) = &self.state else {
            return None;
        };
        if !failure.wants_login() {
            return None;
        }
        self.relist_after_login = true;
        Some(failure.host.clone())
    }

    /// Preferences closed: list again when Log in… opened it.
    pub fn after_preferences(&mut self) -> Option<TransferRequest> {
        if !std::mem::take(&mut self.relist_after_login) {
            return None;
        }
        self.relist()
    }

    /// Ask a running listing to stop before its next repository.
    fn stop_listing(&mut self) {
        if let Some((_, cancel)) = self.listing.take() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Whether a pull still waits for its answer.
    pub fn running(&self) -> bool {
        matches!(&self.state, TemplatesState::Pulling(pulls) if pulls.iter().any(|p| p.result.is_none()))
    }

    /// Cancel was asked and a pull has not answered yet.
    pub fn cancelling(&self) -> bool {
        matches!(&self.state, TemplatesState::Pulling(pulls) if pulls.iter().any(|p| p.result.is_none() && p.run.cancelling))
    }

    /// A row may be chosen unless its local tag already exists.
    pub fn choosable(&self, index: usize) -> bool {
        self.rows
            .get(index)
            .is_some_and(|r| r.local == Local::Absent)
    }

    pub fn toggle(&mut self, index: usize) {
        if matches!(self.state, TemplatesState::Ready)
            && self.choosable(index)
            && !self.chosen.remove(&index)
        {
            self.chosen.insert(index);
        }
    }

    /// Get is shown while a list with templates is.
    pub fn offers_get(&self) -> bool {
        matches!(self.state, TemplatesState::Ready) && !self.rows.is_empty()
    }

    pub fn can_get(&self) -> bool {
        self.offers_get() && !self.chosen.is_empty()
    }

    /// The Get button counts the choice.
    pub fn get_label(&self) -> String {
        match self.chosen.len() {
            0 => "Get".into(),
            1 => "Get 1 template".into(),
            n => format!("Get {n} templates"),
        }
    }

    /// The note beside the buttons.
    pub fn footer_note(&self) -> Option<&'static str> {
        match &self.state {
            TemplatesState::Listing => Some("Close stops the listing."),
            TemplatesState::Ready if self.rows.is_empty() => None,
            TemplatesState::Ready if !(0..self.rows.len()).any(|i| self.choosable(i)) => {
                Some("Every template here is already in the library.")
            }
            TemplatesState::Ready if self.chosen.is_empty() => Some("Choose the templates to get."),
            TemplatesState::Pulling(_) if self.running() && !self.cancelling() => {
                Some("Cancel or Escape stops at the next read.")
            }
            _ => None,
        }
    }

    /// Pull every chosen template; the worker runs the jobs in this order.
    pub fn get(&mut self) -> Vec<TransferRequest> {
        if !self.can_get() {
            return Vec::new();
        }
        let mut requests = Vec::new();
        let mut pulls = Vec::new();
        for index in std::mem::take(&mut self.chosen) {
            let row = &self.rows[index];
            let (run, request) =
                PullRun::start(&self.accounts, row.reference.clone(), row.local_tag.clone());
            pulls.push(TemplatePull {
                name: row.name.clone(),
                tag: row.tag.clone(),
                size: row.size,
                blobs: row.blobs.clone(),
                stored: row.stored.clone(),
                run,
                result: None,
            });
            requests.push(request);
        }
        self.state = TemplatesState::Pulling(pulls);
        requests
    }

    /// Ask the running pull to stop at its next read and the queued ones not
    /// to start; each still answers.
    pub fn cancel(&mut self) {
        if let TemplatesState::Pulling(pulls) = &mut self.state {
            for pull in pulls.iter_mut().filter(|p| p.result.is_none()) {
                pull.run.cancel();
            }
        }
    }

    /// The stage of the pull at `index`: the first one without an answer runs,
    /// the ones after it wait.
    pub fn stage(&self, index: usize) -> Option<Stage> {
        let TemplatesState::Pulling(pulls) = &self.state else {
            return None;
        };
        let current = pulls.iter().position(|p| p.result.is_none());
        let pull = pulls.get(index)?;
        Some(match &pull.result {
            Some(Ok(_)) => Stage::Pulled,
            Some(Err(PullFailure::Failed(_))) => Stage::Failed,
            Some(Err(_)) => Stage::Cancelled,
            None if current != Some(index) => Stage::Waiting,
            None if pull.run.cancelling => Stage::Stopping,
            None => Stage::Pulling,
        })
    }

    /// Whether `id` is one of this dialog's pulls.
    pub fn owns(&self, id: u64) -> bool {
        matches!(&self.state, TemplatesState::Pulling(pulls) if pulls.iter().any(|p| p.run.id() == id))
    }

    /// The blobs in the store before the pull at `index` runs: those the
    /// list found, and those the pulls before it verified.
    fn stored_before(pulls: &[TemplatePull], index: usize) -> BTreeSet<Digest> {
        let mut stored = pulls[index].stored.clone();
        for earlier in &pulls[..index] {
            stored.extend(earlier.run.progress().fetched.into_iter().map(|d| d.digest));
        }
        stored
    }

    /// The line under the pull at `index`.
    pub fn detail(&self, index: usize) -> Option<String> {
        let TemplatesState::Pulling(pulls) = &self.state else {
            return None;
        };
        let stage = self.stage(index)?;
        Some(pulls[index].detail(stage, &Self::stored_before(pulls, index)))
    }

    /// The one progress bar over every chosen template: its fraction and label.
    pub fn progress(&self) -> Option<(f32, String)> {
        let TemplatesState::Pulling(pulls) = &self.state else {
            return None;
        };
        let total = pulls.len().max(1);
        let answered = pulls.iter().filter(|p| p.result.is_some()).count();
        let current = pulls.iter().enumerate().find(|(_, p)| p.result.is_none());
        let (part, label) = match current {
            None => (
                0.0,
                format!(
                    "Done — {} of {total} {} pulled",
                    self.pulled().len(),
                    if total == 1 { "template" } else { "templates" }
                ),
            ),
            Some((index, pull)) => {
                let received = pull.received(&Self::stored_before(pulls, index));
                let part = received as f32 / pull.size.max(1) as f32;
                if self.cancelling() {
                    (part, "Cancelling — stopping at the next read…".into())
                } else {
                    (
                        part,
                        format!(
                            "Template {} of {total} · {} · {} of {}",
                            index + 1,
                            pull.name,
                            human_size(received),
                            human_size(pull.size)
                        ),
                    )
                }
            }
        };
        Some(((answered as f32 + part) / total as f32, label))
    }

    /// What the pulls added, once every one answered.
    pub fn summary(&self) -> Option<Summary> {
        let TemplatesState::Pulling(pulls) = &self.state else {
            return None;
        };
        if self.running() {
            return None;
        }
        let pulled = self.pulled();
        let total = pulls.len();
        Some(if pulled.len() == total {
            Summary {
                complete: true,
                heading: if total == 1 {
                    "Got the template.".into()
                } else {
                    format!("Got all {total} templates.")
                },
                text: format!(
                    "In the library as {}. Start one as it is, or open Settings… first to change its hardware.",
                    joined(&pulled)
                ),
            }
        } else if !pulled.is_empty() {
            Summary {
                complete: false,
                heading: format!("Got {} of {total} templates.", pulled.len()),
                text: "Back to the list to get the others again; blobs already fetched are not fetched again.".into(),
            }
        } else {
            Summary {
                complete: false,
                heading: "No template was added.".into(),
                text: "No local tag was written. Blobs already fetched stay in the store, and the next pull fetches only the rest.".into(),
            }
        })
    }

    /// The library worker stopped, so no answer will come.
    pub fn fail(&mut self, message: &str) {
        match &mut self.state {
            TemplatesState::Listing => {
                self.stop_listing();
                self.state =
                    TemplatesState::Failed(ListingFailure::other(&self.default_host, message));
            }
            TemplatesState::Pulling(pulls) => {
                for pull in pulls.iter_mut().filter(|p| p.result.is_none()) {
                    pull.result = Some(Err(PullFailure::Failed(message.into())));
                }
            }
            _ => {}
        }
    }

    /// The local tags this dialog's finished pulls wrote.
    pub fn pulled(&self) -> Vec<String> {
        match &self.state {
            TemplatesState::Pulling(pulls) => pulls
                .iter()
                .filter(|p| matches!(p.result, Some(Ok(_))))
                .map(|p| p.run.local_tag.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn on_event(&mut self, event: &TransferEvent) {
        match event {
            TransferEvent::Templates { id, result }
                if self
                    .listing
                    .as_ref()
                    .is_some_and(|(listing, _)| listing == id) =>
            {
                self.listing = None;
                self.chosen.clear();
                match result {
                    Ok(rows) => {
                        self.rows = rows.clone();
                        self.state = TemplatesState::Ready;
                    }
                    Err(failure) => self.state = TemplatesState::Failed(failure.clone()),
                }
            }
            TransferEvent::Pulled { id, result } => {
                let TemplatesState::Pulling(pulls) = &mut self.state else {
                    return;
                };
                if let Some(pull) = pulls
                    .iter_mut()
                    .find(|p| p.run.id() == *id && p.result.is_none())
                {
                    pull.result = Some(match result {
                        // The row's name and tag; the short digest follows.
                        Ok(report) => Ok(pulled_text(
                            &Reference {
                                digest: None,
                                ..pull.run.source.clone()
                            },
                            report,
                        )),
                        Err(failure) => Err(failure.clone()),
                    });
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        library_test_support::FirmwareStore, library_view::LibraryView, library_worker::Request,
        registry_transfer::tests::wait_pumping,
    };
    use image_registry::{
        credential_store::{Secret, SessionStore},
        credentials::{Anonymous, CredentialSource, Credentials, EnvCredentials},
        push,
        test_support::{MockRegistry, Policy},
        Client, RegistryEntry,
    };
    use std::{path::Path, sync::Arc};

    const PULL_CANCELLED: &str = "Cancelled. The local tag was not set. Blobs already fetched stay in the store, and the next pull fetches only the rest.";

    /// Preferences that list `host` as a plain-HTTP loopback registry and make
    /// it the default registry, as `UNEMU_REGISTRY` would.
    fn accounts(dir: &Path, host: &str) -> Accounts {
        let path = dir.join(format!("templates-{}.toml", host.replace(':', "-")));
        std::fs::write(
            &path,
            format!("[[registries]]\nhost = \"{host}\"\nplain_http = true\n"),
        )
        .unwrap();
        Accounts {
            path: Some(path),
            store: Arc::new(SessionStore::default()),
            env: EnvCredentials::default(),
            default_override: Some(host.into()),
        }
    }

    /// Log in to `host` as `ci` with `secret`, as Preferences › Registries
    /// does: the user in preferences, the secret in the credential store.
    fn log_in(accounts: &Accounts, host: &str, secret: &str) {
        std::fs::write(
            accounts.path.as_ref().unwrap(),
            format!("[[registries]]\nhost = \"{host}\"\nplain_http = true\nuser = \"ci\"\n"),
        )
        .unwrap();
        accounts
            .store
            .store(host, "ci", &Secret::new(secret))
            .unwrap();
    }

    /// Publish the built templates to `registry` under `unemu/` through the
    /// command line's push plan and the guard: `open-at` with `latest` on
    /// 1.0.1 and a newer 1.1.0 that `latest` does not name, `open-at-games`
    /// with a 2.0.0 pre-release, a firmware package, a live-test repository
    /// and an image outside the namespace.
    fn publish(
        store: &Store,
        registry: &MockRegistry,
        entry: &RegistryEntry,
        credentials: Box<dyn CredentialSource>,
    ) {
        let client = Client::new(entry, credentials).unwrap();
        for (local, remote) in [
            ("open-at:1.1.1", "unemu/open-at:1.0.1"),
            ("open-at:latest", "unemu/open-at:latest"),
            ("open-at-games:1.1.1", "unemu/open-at:1.1.0"),
            ("open-at-games:1.1.1", "unemu/open-at-games:1.0.1"),
            ("open-at:1.1.1", "unemu/open-at-games:2.0.0-rc.1"),
            (
                "unemu/firmware/xub-r638-xtp-ata1f0:1.1.1",
                "unemu/firmware/xub-r638-xtp-ata1f0:1.0.1",
            ),
            ("open-at:1.1.1", "unemu/test-live:1.0.1"),
            ("open-at:1.1.1", "elsewhere/open-at:1.0.1"),
        ] {
            let target =
                Reference::parse(&format!("{}/{remote}", registry.host()), "unused.example")
                    .unwrap();
            let plan = cli::push_plan(
                store,
                local,
                target,
                Some(entry),
                pc_image::firmware::catalogue(),
            )
            .unwrap();
            assert_eq!(plan.verdict(), &Ok(()), "{remote}");
            cli::with_image_providers(|providers| {
                push::push(
                    store,
                    &plan,
                    providers,
                    &client,
                    &push::PushOptions::default(),
                    &mut |_, _| {},
                )
            })
            .unwrap();
        }
    }

    /// The templates built from the fixtures and published to a registry
    /// with `policy`, through the login `ci`/`secret` when it has one, or
    /// `None` (the test skips) when the vendor fixtures are missing.
    fn published(policy: Policy) -> Option<(FirmwareStore, MockRegistry)> {
        let built = FirmwareStore::new(false)?;
        let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor");
        cli::build_templates(&built.store, &vendor).unwrap();
        let credentials: Box<dyn CredentialSource> = match &policy.user {
            Some((user, secret)) => Box::new(Credentials::new(user.clone(), secret.clone())),
            None => Box::new(Anonymous),
        };
        let registry = MockRegistry::start(policy);
        let entry = registry.entry(false);
        publish(&built.store, &registry, &entry, credentials);
        registry.clear_log();
        Some((built, registry))
    }

    fn open_registry() -> Policy {
        Policy {
            anonymous_read: vec!["*".into()],
            anonymous_write: vec!["*".into()],
            ..Policy::default()
        }
    }

    /// A private registry that wants the login `ci`/`secret` for every read.
    /// `anonymous_catalog` lists its repositories to anyone, so a request for
    /// one of them, not the catalog, asks for the login.
    fn private_registry(anonymous_catalog: bool) -> Policy {
        Policy {
            user: Some(("ci".into(), "secret".into())),
            anonymous_catalog,
            ..Policy::default()
        }
    }

    fn settled(view: &LibraryView) -> bool {
        let dialog = view.templates.as_ref().unwrap();
        !matches!(dialog.state, TemplatesState::Listing) && !dialog.running()
    }

    /// A library on an empty store with Get templates… open and listed.
    fn listed(dir: &Path, accounts: Accounts) -> LibraryView {
        let store = Store::open(dir.join("store")).unwrap();
        let mut view = LibraryView::synthetic(store.root().to_owned());
        view.begin_templates(accounts);
        view.pump_until(settled);
        view
    }

    fn failure(view: &LibraryView) -> &ListingFailure {
        match &view.templates.as_ref().unwrap().state {
            TemplatesState::Failed(failure) => failure,
            other => panic!("{other:?}"),
        }
    }

    fn send_all(view: &mut LibraryView, requests: Vec<TransferRequest>) {
        for request in requests {
            view.send(Request::Transfer(request));
        }
    }

    #[test]
    fn get_templates_lists_the_official_machines_and_pulls_the_chosen_ones() {
        let Some((built, registry)) = published(open_registry()) else {
            return;
        };
        // A repository whose manifest does not match its digest is skipped.
        registry.forge_manifest("unemu/broken", "1.0.1", &Digest::of(b"claimed"), b"{}");
        let host = registry.host();
        registry.clear_log();

        let target = assembly::test_support::TempDir::new("templates-target");
        let mut view = listed(target.path(), accounts(target.path(), &host));
        let store = Store::open(target.path().join("store")).unwrap();
        let dialog = view.templates.as_mut().unwrap();
        assert_eq!(dialog.default_host, host);
        assert!(
            matches!(dialog.state, TemplatesState::Ready),
            "{:?}",
            dialog.state
        );
        // Machines under unemu/ only: the version latest names, else the
        // newest release. Firmware and live-test repositories are not even read.
        let log = registry.log();
        assert!(
            !log.iter()
                .any(|l| l.contains("/unemu/firmware/") || l.contains("/unemu/test-")),
            "{log:?}"
        );
        assert!(
            log.iter()
                .any(|l| l.starts_with("GET /v2/unemu/broken/manifests/")),
            "{log:?}"
        );
        let shown: Vec<_> = dialog
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.tag.as_str(), r.local_tag.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("open-at", "1.0.1", "open-at:1.0.1"),
                ("open-at-games", "1.0.1", "open-at-games:1.0.1")
            ]
        );
        let open_at = built.store.tag_descriptor("open-at:1.1.1").unwrap();
        let row = &dialog.rows[0];
        assert_eq!(row.digest, open_at.digest);
        assert_eq!(
            row.title.as_deref(),
            Some(pc_image::template::TEMPLATES[0].title)
        );
        assert_eq!(
            row.description.as_deref(),
            Some(pc_image::template::TEMPLATES[0].description)
        );
        // The pull fetches the manifest the row showed, by digest.
        assert_eq!(
            row.reference.to_string(),
            format!("{host}/unemu/open-at:1.0.1@{}", open_at.digest)
        );
        assert_eq!(row.local, Local::Absent);
        let manifest = image_store::Manifest::decode_for_summary(
            &image_store::BlobSource::read_verified(
                &built.store,
                &open_at,
                image_store::MAX_SMALL_BLOB,
            )
            .unwrap(),
        )
        .unwrap();
        let mut seen = BTreeSet::new();
        let blobs: u64 = std::iter::once(&manifest.config)
            .chain(&manifest.layers)
            .filter(|d| seen.insert(d.digest.clone()))
            .map(|d| d.size)
            .sum();
        assert_eq!(row.size, blobs + open_at.size);

        // Get pulls the chosen template only.
        assert!(dialog.offers_get() && !dialog.can_get());
        assert_eq!(dialog.footer_note(), Some("Choose the templates to get."));
        dialog.toggle(0);
        assert_eq!(dialog.get_label(), "Get 1 template");
        assert_eq!(dialog.footer_note(), None);
        let requests = dialog.get();
        assert_eq!(requests.len(), 1);
        assert!(
            matches!(&requests[0], TransferRequest::Pull(job) if job.source.digest.as_ref() == Some(&open_at.digest))
        );
        send_all(&mut view, requests);
        view.pump_until(settled);
        let dialog = view.templates.as_ref().unwrap();
        let TemplatesState::Pulling(pulls) = &dialog.state else {
            panic!("{:?}", dialog.state)
        };
        // The row's reference and the short digest, not the full digest.
        let pulled = format!("Pulled {host}/unemu/open-at:1.0.1 as open-at:1.0.1 (");
        assert!(
            matches!(&pulls[0].result, Some(Ok(message)) if message.starts_with(&pulled)),
            "{pulls:?}"
        );
        assert_eq!(dialog.stage(0), Some(Stage::Pulled));
        assert_eq!(
            dialog.progress(),
            Some((1.0, "Done — 1 of 1 template pulled".into()))
        );
        assert_eq!(
            dialog.summary(),
            Some(Summary {
                complete: true,
                heading: "Got the template.".into(),
                text: "In the library as open-at:1.0.1. Start one as it is, or open Settings… first to change its hardware.".into(),
            })
        );
        assert_eq!(dialog.pulled(), ["open-at:1.0.1"]);
        assert_eq!(
            store.tag_descriptor("open-at:1.0.1").unwrap().digest,
            open_at.digest
        );
        assert!(store.tag_descriptor("open-at-games:1.0.1").is_err());
        assert_eq!(view.status.as_deref(), Some("Got open-at:1.0.1"));

        // Listed again, the pulled template is in the library and cannot be
        // chosen twice; a local tag that names another image blocks its row.
        store
            .update_tag("open-at-games:1.0.1", &open_at, None, &pc_image::PcProvider)
            .unwrap();
        let request = view.templates.as_mut().unwrap().relist().unwrap();
        view.send(Request::Transfer(request));
        view.pump_until(settled);
        let dialog = view.templates.as_mut().unwrap();
        assert_eq!(dialog.rows[0].local, Local::Same);
        assert_eq!(dialog.rows[1].local, Local::Other);
        dialog.toggle(0);
        dialog.toggle(1);
        assert!(dialog.chosen.is_empty());
        assert_eq!(
            dialog.footer_note(),
            Some("Every template here is already in the library.")
        );
    }

    #[test]
    fn the_chosen_templates_pull_in_turn_and_cancel_stops_the_queued_one_before_it_starts() {
        let Some((_built, registry)) = published(open_registry()) else {
            return;
        };
        let host = registry.host();
        let target = assembly::test_support::TempDir::new("templates-queue");
        let mut view = listed(target.path(), accounts(target.path(), &host));
        let store = Store::open(target.path().join("store")).unwrap();
        let dialog = view.templates.as_mut().unwrap();
        dialog.toggle(0);
        dialog.toggle(1);
        assert_eq!(dialog.get_label(), "Get 2 templates");
        // The registry holds the first template's first blob download.
        registry.hold_next("GET /v2/unemu/open-at/blobs/");
        let requests = dialog.get();
        send_all(&mut view, requests);
        wait_pumping(&mut view, || registry.held());
        let dialog = view.templates.as_mut().unwrap();
        assert_eq!(dialog.stage(0), Some(Stage::Pulling));
        assert_eq!(dialog.stage(1), Some(Stage::Waiting));
        let (fraction, label) = dialog.progress().unwrap();
        assert!(fraction < 0.5, "{fraction}");
        assert!(label.starts_with("Template 1 of 2 · open-at · "), "{label}");
        let TemplatesState::Pulling(pulls) = &dialog.state else {
            panic!()
        };
        assert_eq!(
            dialog.detail(1).as_deref(),
            Some("Starts when the pull before it has answered.")
        );
        assert_eq!(
            pulls[0].route(),
            format!(
                "{host}/unemu/open-at:1.0.1 → open-at:1.0.1 · {}",
                human_size(pulls[0].size)
            )
        );
        assert_eq!(
            dialog.footer_note(),
            Some("Cancel or Escape stops at the next read.")
        );

        // Cancel stops the running pull at its next read and the queued one
        // before it starts.
        dialog.cancel();
        assert!(dialog.cancelling());
        assert_eq!(dialog.stage(0), Some(Stage::Stopping));
        assert_eq!(dialog.stage(1), Some(Stage::Waiting));
        assert_eq!(
            dialog.progress().unwrap().1,
            "Cancelling — stopping at the next read…"
        );
        assert_eq!(dialog.footer_note(), None);
        registry.clear_log();
        registry.release();
        view.pump_until(settled);
        let dialog = view.templates.as_ref().unwrap();
        assert_eq!(dialog.stage(0), Some(Stage::Cancelled));
        assert_eq!(dialog.stage(1), Some(Stage::Cancelled));
        assert_eq!(dialog.detail(0).as_deref(), Some(PULL_CANCELLED));
        assert_eq!(
            dialog.detail(1).as_deref(),
            Some("Cancelled before it started; nothing was fetched.")
        );
        assert!(
            !registry
                .log()
                .iter()
                .any(|l| l.contains("/unemu/open-at-games/")),
            "{:?}",
            registry.log()
        );
        assert!(store.tag_descriptor("open-at:1.0.1").is_err());
        assert!(store.tag_descriptor("open-at-games:1.0.1").is_err());
        assert_eq!(
            dialog.progress(),
            Some((1.0, "Done — 0 of 2 templates pulled".into()))
        );
        let summary = dialog.summary().unwrap();
        assert!(!summary.complete);
        assert_eq!(summary.heading, "No template was added.");
        assert_eq!(summary.text, "No local tag was written. Blobs already fetched stay in the store, and the next pull fetches only the rest.");
        // The dialog says how its pulls ended; the status bar stays as it was.
        assert_eq!(view.status, None);

        // Back to the list: both again, one after another under one bar. The
        // second fetches only what the first did not; both share firmware,
        // which its bar and its line count as already there.
        let request = view.templates.as_mut().unwrap().relist().unwrap();
        view.send(Request::Transfer(request));
        view.pump_until(settled);
        let dialog = view.templates.as_mut().unwrap();
        let shared: Vec<_> = dialog.rows[1]
            .blobs
            .iter()
            .filter(|b| dialog.rows[0].blobs.iter().any(|a| a.digest == b.digest))
            .collect();
        assert!(!shared.is_empty());
        let shared_bytes: u64 = shared.iter().map(|b| b.size).sum();
        let (shared_count, games_size) = (shared.len(), dialog.rows[1].size);
        dialog.toggle(0);
        dialog.toggle(1);
        registry.hold_next("GET /v2/unemu/open-at-games/blobs/");
        let requests = dialog.get();
        send_all(&mut view, requests);
        wait_pumping(&mut view, || registry.held());
        // The first answered before the second asked for anything.
        view.pump_queued();
        let dialog = view.templates.as_ref().unwrap();
        assert_eq!(dialog.stage(0), Some(Stage::Pulled));
        assert_eq!(dialog.stage(1), Some(Stage::Pulling));
        let (fraction, label) = dialog.progress().unwrap();
        let least = (1.0 + shared_bytes as f32 / games_size as f32) / 2.0;
        assert!(fraction >= least - 1e-6, "{fraction} < {least}: {label}");
        assert!(
            dialog
                .detail(1)
                .unwrap()
                .ends_with(&format!(" · {shared_count} already in the store")),
            "{:?}",
            dialog.detail(1)
        );
        registry.release();
        view.pump_until(settled);
        let dialog = view.templates.as_ref().unwrap();
        let TemplatesState::Pulling(pulls) = &dialog.state else {
            panic!()
        };
        assert_eq!(dialog.stage(0), Some(Stage::Pulled));
        assert_eq!(dialog.stage(1), Some(Stage::Pulled));
        assert!(
            matches!(&pulls[1].result, Some(Ok(m)) if !m.contains(", 0 already in the store")),
            "{pulls:?}"
        );
        assert_eq!(
            dialog.progress(),
            Some((1.0, "Done — 2 of 2 templates pulled".into()))
        );
        assert_eq!(
            dialog.summary(),
            Some(Summary {
                complete: true,
                heading: "Got all 2 templates.".into(),
                text: "In the library as open-at:1.0.1 and open-at-games:1.0.1. Start one as it is, or open Settings… first to change its hardware.".into(),
            })
        );
        assert_eq!(
            view.status.as_deref(),
            Some("Got open-at:1.0.1, open-at-games:1.0.1")
        );
    }

    #[test]
    fn a_partial_get_says_how_many_templates_were_added() {
        let Some((_built, registry)) = published(open_registry()) else {
            return;
        };
        let host = registry.host();
        let target = assembly::test_support::TempDir::new("templates-partial");
        let mut view = listed(target.path(), accounts(target.path(), &host));
        let dialog = view.templates.as_mut().unwrap();
        dialog.toggle(0);
        dialog.toggle(1);
        let requests = dialog.get();
        // The second pull's job fails as a dropped connection would.
        let TransferRequest::Pull(second) = &requests[1] else {
            panic!()
        };
        let second = second.id;
        view.send(Request::Transfer(requests.into_iter().next().unwrap()));
        let dialog = view.templates.as_mut().unwrap();
        dialog.on_event(&TransferEvent::Pulled {
            id: second,
            result: Err(PullFailure::Failed(
                "Network: connection reset by peer.".into(),
            )),
        });
        view.pump_until(settled);
        let dialog = view.templates.as_ref().unwrap();
        assert_eq!(dialog.stage(0), Some(Stage::Pulled));
        assert_eq!(dialog.stage(1), Some(Stage::Failed));
        assert_eq!(
            dialog.detail(1).as_deref(),
            Some("open-at-games: Network: connection reset by peer.")
        );
        assert_eq!(
            dialog.summary(),
            Some(Summary {
                complete: false,
                heading: "Got 1 of 2 templates.".into(),
                text: "Back to the list to get the others again; blobs already fetched are not fetched again.".into(),
            })
        );
    }

    #[test]
    fn an_unreachable_registry_ends_the_listing_with_a_message() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = closed.local_addr().unwrap().to_string();
        drop(closed);
        let target = assembly::test_support::TempDir::new("templates-unreachable");
        let view = listed(target.path(), accounts(target.path(), &host));
        let failure = failure(&view);
        assert_eq!(failure.kind, FailureKind::Unreachable);
        assert_eq!(failure.heading(), "Could not reach the registry");
        assert!(
            failure
                .message
                .starts_with(&format!("Could not reach {host}:")),
            "{failure:?}"
        );
        assert!(
            failure.message.ends_with("Check the network connection, or choose another default registry under Tools › Preferences › Registries."),
            "{failure:?}"
        );
        assert_eq!(failure.hint(), "Nothing was pulled.");
        assert!(!failure.offers_login());

        // Preferences that cannot be read fail at once, without the worker.
        let broken = accounts(target.path(), "not a host");
        let (dialog, request) = TemplatesDialog::open(broken);
        assert!(request.is_none());
        assert!(
            matches!(&dialog.state, TemplatesState::Failed(f) if f.kind == FailureKind::Other && f.message.starts_with("Preferences")),
            "{:?}",
            dialog.state
        );
    }

    #[test]
    fn a_missing_or_refused_login_offers_log_in_and_lists_again_once_preferences_close() {
        let Some((_built, registry)) = published(private_registry(true)) else {
            return;
        };
        let host = registry.host();
        let target = assembly::test_support::TempDir::new("templates-login");
        let accounts = accounts(target.path(), &host);
        let mut view = listed(target.path(), accounts.clone());
        let missing = failure(&view);
        assert_eq!(missing.kind, FailureKind::LoginRequired);
        assert_eq!(missing.heading(), "The registry asks for a login");
        assert_eq!(
            missing.message,
            format!("{host} asks for a login. Log in under Tools › Preferences › Registries, then try again.")
        );
        assert_eq!(
            missing.hint(),
            "Nothing was pulled. After Log in…, the dialog lists again."
        );
        assert!(missing.offers_login());

        // Log in… asks for Preferences on this registry; the dialog lists
        // again only once Preferences closes.
        let ctx = egui::Context::default();
        crate::registry_transfer_view::tests::click(&ctx, "Log in…", |ctx| {
            view.modals(ctx, None, Vec::new(), true, false)
        });
        assert_eq!(view.login_wanted.as_deref(), Some(host.as_str()));
        log_in(&accounts, &host, "secret");
        view.preferences_closed();
        view.pump_until(settled);
        let dialog = view.templates.as_ref().unwrap();
        assert!(
            matches!(dialog.state, TemplatesState::Ready),
            "{:?}",
            dialog.state
        );
        assert_eq!(dialog.rows.len(), 2);
        // Closing Preferences again, without a Log in…, lists nothing.
        view.preferences_closed();
        assert!(matches!(
            view.templates.as_ref().unwrap().state,
            TemplatesState::Ready
        ));

        // A login the registry refuses.
        log_in(&accounts, &host, "wrong");
        let request = view.templates.as_mut().unwrap().relist().unwrap();
        view.send(Request::Transfer(request));
        view.pump_until(settled);
        let refused = failure(&view);
        assert_eq!(refused.kind, FailureKind::Refused);
        assert_eq!(refused.heading(), "The registry refused the login");
        assert_eq!(
            refused.message,
            format!("{host} refused the stored login (401). Log in again under Tools › Preferences › Registries.")
        );
        assert!(refused.offers_login());
        assert_eq!(
            view.templates.as_mut().unwrap().log_in().as_deref(),
            Some(host.as_str())
        );
    }

    #[test]
    fn a_stored_login_goes_with_the_catalog_request() {
        // The catalog shows anonymous callers only what they may read, here
        // nothing, so the listing must not ask anonymously first.
        let Some((_built, registry)) = published(private_registry(false)) else {
            return;
        };
        let host = registry.host();
        let target = assembly::test_support::TempDir::new("templates-private");
        let accounts = accounts(target.path(), &host);
        log_in(&accounts, &host, "secret");
        let view = listed(target.path(), accounts);
        let dialog = view.templates.as_ref().unwrap();
        assert!(
            matches!(dialog.state, TemplatesState::Ready),
            "{:?}",
            dialog.state
        );
        let names: Vec<_> = dialog.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["open-at", "open-at-games"]);
        assert!(
            registry
                .authorized_log()
                .iter()
                .any(|l| l.starts_with("GET /v2/_catalog")),
            "{:?}",
            registry.authorized_log()
        );
    }

    #[test]
    fn a_registry_without_a_catalog_offers_no_templates_and_no_login() {
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            no_catalog: true,
            ..Policy::default()
        });
        let host = registry.host();
        let target = assembly::test_support::TempDir::new("templates-no-catalog");
        let mut view = listed(target.path(), accounts(target.path(), &host));
        let failure = failure(&view).clone();
        assert_eq!(failure.kind, FailureKind::NoCatalog);
        assert_eq!(failure.heading(), "This registry offers no templates");
        assert_eq!(
            failure.message,
            format!("{host} does not list its repositories, so it offers no templates here.")
        );
        assert_eq!(failure.hint(), "Templates are listed from the registry catalog. registry.unemu.dev has one; choose it as the default registry to get the official templates.");
        assert!(!failure.offers_login());
        assert_eq!(view.templates.as_mut().unwrap().log_in(), None);
    }

    #[test]
    fn a_registry_without_templates_lists_none() {
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..Policy::default()
        });
        registry.seed_repositories(&["private/lab"]);
        let target = assembly::test_support::TempDir::new("templates-empty");
        let view = listed(target.path(), accounts(target.path(), &registry.host()));
        let dialog = view.templates.as_ref().unwrap();
        assert!(matches!(dialog.state, TemplatesState::Ready));
        assert!(dialog.rows.is_empty());
        // Get is hidden: there is nothing to choose.
        assert!(!dialog.offers_get());
        assert_eq!(dialog.footer_note(), None);
    }

    #[test]
    fn releases_come_newest_first_and_pre_releases_only_without_one() {
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(
            versions(&tags(&["latest", "1.0.1", "1.10.0", "1.9.3", "2.0.0-rc.1"])),
            tags(&["1.10.0", "1.9.3", "1.0.1"])
        );
        assert_eq!(
            versions(&tags(&["2.0.0-rc.1", "2.0.0-rc.2", "latest"])),
            tags(&["2.0.0-rc.2", "2.0.0-rc.1"])
        );
        assert!(versions(&tags(&["latest", "dev"])).is_empty());
        assert!(not_a_template("firmware/xub-r638-xtp-ata1f0"));
        assert!(not_a_template("test-push-1"));
        assert!(!not_a_template("open-at"));
    }

    #[test]
    fn closing_the_dialog_cancels_its_listing_between_repositories() {
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..Policy::default()
        });
        registry.seed_repositories(&["unemu/one", "unemu/two"]);
        let target = assembly::test_support::TempDir::new("templates-cancel");
        let store = Store::open(target.path().join("store")).unwrap();
        let (dialog, request) = TemplatesDialog::open(accounts(target.path(), &registry.host()));
        let Some(TransferRequest::Templates {
            accounts, cancel, ..
        }) = request
        else {
            panic!("a listing")
        };
        registry.clear_log();
        // The first repository's tag list waits until the dialog is closed.
        registry.hold_next("GET /v2/unemu/one/tags/list");
        std::thread::scope(|scope| {
            let listing = scope.spawn(|| list(&store, &accounts, &cancel));
            let deadline = std::time::Instant::now() + crate::test_wait::HANG_GUARD;
            while !registry.held() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the listing did not start"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(!cancel.load(Ordering::SeqCst));
            drop(dialog);
            assert!(cancel.load(Ordering::SeqCst));
            registry.release();
            assert_eq!(
                listing.join().unwrap().map_err(|f| f.message),
                Err("Cancelled.".into())
            );
        });
        // The catalog and the first repository were read; the second was not.
        let log = registry.log();
        assert!(
            log.iter().any(|l| l.starts_with("GET /v2/_catalog")),
            "{log:?}"
        );
        assert!(
            log.iter().any(|l| l.starts_with("GET /v2/unemu/one/")),
            "{log:?}"
        );
        assert!(!log.iter().any(|l| l.contains("/unemu/two/")), "{log:?}");
    }

    #[test]
    fn a_repository_this_account_may_not_read_is_skipped_and_the_listing_goes_on() {
        let forbidden = Error::Forbidden {
            host: "nas.example:5000".into(),
            path: "/v2/unemu/lab/tags/list".into(),
        };
        assert!(!fatal(&forbidden));
        assert!(fatal(&Error::Unauthorized {
            host: "nas.example:5000".into()
        }));
        assert!(fatal(&Error::Transport("connection reset".into())));
    }

    #[test]
    fn a_login_for_a_registry_without_an_entry_offers_add_registry_instead() {
        let host = "nas.example:5000";
        let required = || Error::AuthRequired { host: host.into() };
        let unlisted = listing_failure(host, false, required());
        assert_eq!(unlisted.kind, FailureKind::LoginRequired);
        assert!(!unlisted.offers_login() && unlisted.offers_add());
        assert_eq!(
            unlisted.message,
            format!("{host} asks for a login, and it is not in your registry list. Add it under Tools › Preferences › Registries and log in, then try again.")
        );
        assert_eq!(
            unlisted.hint(),
            "Nothing was pulled. After Add registry… and a login there, the dialog lists again."
        );
        let listed = listing_failure(host, true, required());
        assert!(listed.offers_login() && !listed.offers_add());
        assert_eq!(
            listed.message,
            format!("{host} asks for a login. Log in under Tools › Preferences › Registries, then try again.")
        );
        // Either button asks the shell for Preferences on that registry.
        let target = assembly::test_support::TempDir::new("templates-unlisted");
        let (mut dialog, request) =
            TemplatesDialog::open(accounts(target.path(), "127.0.0.1:5999"));
        let Some(TransferRequest::Templates { id, .. }) = request else {
            panic!("a listing")
        };
        dialog.on_event(&TransferEvent::Templates {
            id,
            result: Err(unlisted),
        });
        assert_eq!(dialog.log_in().as_deref(), Some(host));
    }
}
