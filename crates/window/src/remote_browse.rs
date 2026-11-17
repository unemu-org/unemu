// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless model of the library's Remotes: browsing the configured
//! registries without pulling, following the approved remotes mock-up,
//! revision 2 (`backlog/docs/design/mockups/remotes.html`), and
//! `backlog/docs/design/machine-images.md` §3 (local tags) and §5.1
//! (registry rules).
//!
//! The roots are the registries of Tools › Preferences › Registries, the
//! default registry first; the family default registry is listed even when
//! preferences do not name it. Nothing contacts a registry until its node is
//! opened. A listing reads the registry catalog (the Docker Registry HTTP API
//! V2 `_catalog` endpoint) 50 repositories a page, the next page on demand;
//! for each repository its tag list (`tags/list`), following the registry's
//! `Link` paging; and the record of the tag the repository row stands for:
//! its manifest and, for a media set or firmware package, its config record
//! ([`image_registry::browse`]). The other tags are read when a repository is
//! expanded or a tag selected. Selecting a tag also reads its small metadata
//! record, for the edition and version the details pane shows; no other layer
//! is ever fetched. A registry entry that names namespaces lists only the
//! repositories in them. A stored login goes with the first request, because
//! registries answer an anonymous catalog request with their anonymous view
//! instead of a challenge.
//!
//! Every request runs on the library worker, also while a machine runs, with
//! Get templates' short timeouts (10 s to connect, 15 s per read), and
//! answers on a later frame. Stop sets a flag the worker checks before every
//! request after a job's first, so a listing stops at its next read; answers
//! of a stopped job change nothing. The worker answers one request at a time,
//! so the window calls [`Remotes::stop_all`] before it starts a machine. What
//! is read stays in this session's memory and is never written to disk;
//! Refresh lists the registry again and replaces it.
//!
//! States, as the mock-up names them: listing; listed, or empty when nothing
//! is published; stopped; unreachable; unreachable after a listing, whose rows
//! stay, marked stale, while Pull waits; server error, with the status and its
//! reason, a registry without a catalog (`404`) included; and login required,
//! a `401` with the registry's challenge, where nobody is asked for a password
//! and nothing is kept. A failure on a next page keeps the pages before it.
//!
//! A registry entry that names namespaces shows those; a whole-registry entry
//! groups its repositories by their first path component.
//!
//! Each read tag is compared with the library within the one local name a
//! pull of it writes ([`Reference::local_tag`]): host-qualified, or the short
//! name for the family default registry. *In library*: that local tag names
//! this image. *Local tag names another image*: it names another one. *Newer
//! available*: the library holds older versions under that name and none as
//! new, by the artifact type's order: Semantic Versioning 2.0.0 §11
//! precedence for machine images, the set-tag order for installation media
//! sets ([`media_set::compare_set_tags`]) and the revision number for
//! firmware packages; a tag outside its type's order, an alias such as
//! `latest` or another tool's artifact makes no claim. *Not pulled*
//! otherwise, naming another local tag of the same image when there is one.
//! The comparison runs on the worker against the store, and again whenever
//! the library changes.

// The library window draws this model in a later change; until then only the
// worker and the tests use most of it.
#![cfg_attr(not(test), expect(dead_code))]

use crate::registry_transfer::{common_failure, next_id, Accounts};
use image_registry::{
    browse::{self, MIN_APP_VERSION},
    client::Challenge,
    config::Scope,
    reference::OFFICIAL_NAMESPACE,
    Client, Error, Reference, RegistryEntry, DEFAULT_REGISTRY,
};
use image_store::{media_set, media_type as mt, Digest, Store};
use std::{
    cmp::Ordering as Order,
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Repositories per catalog page; the list offers the next page when the
/// registry has more.
pub(crate) const CATALOG_PAGE: usize = 50;
/// Tags are asked for in pages of this size, following the registry's `Link`.
const TAG_PAGE: usize = 100;

const TITLE: &str = "org.opencontainers.image.title";
const VERSION: &str = "org.opencontainers.image.version";

/// What the list shows of one tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagRecord {
    /// The image the tag names now; a pull by this digest fetches it.
    pub digest: Digest,
    /// The manifest's artifact type: a machine image, an installation media
    /// set, a firmware package or another tool's artifact.
    pub artifact_type: String,
    /// The manifest's title, or a firmware package's name.
    pub title: Option<String>,
    /// The manifest's version, or a firmware package's.
    pub version: Option<String>,
    /// A media set's formats in set order, each once, such as `720K`.
    pub formats: Vec<String>,
    /// `unemu.min-app-version` as the manifest has it, not yet interpreted.
    pub min_app_version: Option<String>,
    /// The manifest and every distinct blob it names.
    pub size: u64,
    /// Some blob is marked user-supplied.
    pub user_supplied: bool,
    /// Every manifest annotation, for the details pane.
    pub annotations: BTreeMap<String, String>,
    /// What the metadata record adds once the tag is selected, or why it
    /// could not be read; `None` until then.
    pub details: Option<Result<Details, String>>,
}

/// The selected tag's edition and version from its metadata record; a
/// firmware package has none and keeps its config record's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Details {
    pub edition: Option<String>,
    pub version: Option<String>,
}

impl TagRecord {
    fn from_read(read: browse::TagRecord) -> Self {
        let annotation = |key: &str| {
            read.annotations
                .get(key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let config: Option<toml::Table> = read
            .config
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| text.parse().ok());
        let field = |key: &str| {
            config
                .as_ref()
                .and_then(|table| table.get(key))
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        };
        let mut formats = Vec::new();
        if read.artifact_type == mt::MEDIA_SET {
            let media = config
                .as_ref()
                .and_then(|table| table.get("media"))
                .and_then(toml::Value::as_array);
            for medium in media.into_iter().flatten() {
                let name = medium
                    .get("format")
                    .and_then(toml::Value::as_table)
                    .and_then(|format| pc_image::media_set::format_name(format).ok());
                if let Some(name) = name.filter(|name| !formats.iter().any(|f| f == name)) {
                    formats.push(name.to_owned());
                }
            }
        }
        Self {
            title: annotation(TITLE).or_else(|| field("name")),
            version: annotation(VERSION).or_else(|| field("version")),
            formats,
            min_app_version: read.annotations.get(MIN_APP_VERSION).cloned(),
            digest: read.digest,
            artifact_type: read.artifact_type,
            size: read.size,
            user_supplied: read.user_supplied,
            annotations: read.annotations,
            details: None,
        }
    }
}

/// How a listed tag stands against the library, within the local name a pull
/// of it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalState {
    /// The local tag a pull writes names this image.
    InLibrary,
    /// That local tag does not exist. `same_as` is another local tag of the
    /// same image, so a pull would fetch nothing.
    NotPulled { same_as: Option<String> },
    /// The library holds `have`, an older version under the same name, and
    /// no version as new as this one.
    NewerAvailable { have: String },
    /// The local tag a pull writes names `current`, another image.
    OtherImage { current: Digest },
}

/// One tag of a repository, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagRow {
    pub tag: String,
    /// The local tag a pull writes.
    pub local_tag: String,
    /// The record, or why it could not be read.
    pub record: Result<TagRecord, String>,
    /// `None` when the record could not be read.
    pub local: Option<LocalState>,
}

/// One repository of a registry's catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepositoryRow {
    pub name: String,
    /// Every tag, as the registry lists them.
    pub tags: Vec<String>,
    /// The tag the row stands for ([`representative`]).
    pub representative: Option<String>,
    /// The tags read so far.
    pub read: Vec<TagRow>,
    /// Why the tags could not be listed.
    pub error: Option<String>,
}

impl RepositoryRow {
    pub fn row(&self, tag: &str) -> Option<&TagRow> {
        self.read.iter().find(|row| row.tag == tag)
    }

    /// The row of the tag the repository stands for, once read.
    pub fn representative_row(&self) -> Option<&TagRow> {
        self.row(self.representative.as_deref()?)
    }

    fn upsert(&mut self, row: TagRow) {
        match self.read.iter_mut().find(|r| r.tag == row.tag) {
            Some(existing) => *existing = row,
            None => self.read.push(row),
        }
    }
}

/// The tag a repository row stands for, chosen before any manifest is read
/// and whatever order the registry lists the tags in: the newest release by
/// Semantic Versioning 2.0.0 §11 precedence, else the newest pre-release,
/// else the first by the set-tag order ([`media_set::newest_set_tag_first`]),
/// which also puts the highest firmware revision first, then `latest`, then
/// the other aliases by name.
pub(crate) fn representative(tags: &[String]) -> Option<String> {
    let newest = |release: bool| {
        tags.iter()
            .filter_map(|tag| {
                pc_image::template::precedence(tag)
                    .filter(|p| p.is_release() == release)
                    .map(|p| (p, tag))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, tag)| tag.clone())
    };
    newest(true).or_else(|| newest(false)).or_else(|| {
        tags.iter()
            .min_by(|a, b| media_set::newest_set_tag_first(a, b))
            .cloned()
    })
}

/// How two tags of one repository compare as versions of `artifact_type`, or
/// `None` when they make no claim: aliases, tags outside the type's order and
/// artifacts of other tools.
pub(crate) fn compare_tags(artifact_type: &str, a: &str, b: &str) -> Option<Order> {
    match artifact_type {
        mt::MACHINE => {
            Some(pc_image::template::precedence(a)?.cmp(&pc_image::template::precedence(b)?))
        }
        mt::MEDIA_SET => media_set::compare_set_tags(a, b),
        mt::FIRMWARE => pc_image::firmware_import::compare_revisions(a, b),
        _ => None,
    }
}

/// The local tag a pull of `repository:tag` from `host` writes.
pub(crate) fn local_tag_of(host: &str, repository: &str, tag: &str) -> String {
    Reference {
        host: host.into(),
        repository: repository.into(),
        tag: Some(tag.into()),
        digest: None,
    }
    .local_tag()
    .unwrap_or_default()
}

/// How the image `digest` of `artifact_type`, which a pull would write as
/// `local_tag`, stands against the library's tags.
pub(crate) fn local_state(
    local_tag: &str,
    digest: &Digest,
    artifact_type: &str,
    library: &[(String, Digest)],
) -> LocalState {
    if let Some((_, current)) = library.iter().find(|(tag, _)| tag == local_tag) {
        return if current == digest {
            LocalState::InLibrary
        } else {
            LocalState::OtherImage {
                current: current.clone(),
            }
        };
    }
    let (name, mine) = local_tag.rsplit_once(':').unwrap_or((local_tag, ""));
    let held: Vec<(Order, &str, &String)> = library
        .iter()
        .filter_map(|(tag, _)| {
            let (other, version) = tag.rsplit_once(':')?;
            let order = (other == name)
                .then(|| compare_tags(artifact_type, mine, version))
                .flatten()?;
            Some((order, version, tag))
        })
        .collect();
    if !held.is_empty() && held.iter().all(|(order, _, _)| order.is_gt()) {
        let newest = held
            .into_iter()
            .max_by(|a, b| compare_tags(artifact_type, a.1, b.1).unwrap_or(Order::Equal));
        if let Some((_, _, have)) = newest {
            return LocalState::NewerAvailable { have: have.clone() };
        }
    }
    LocalState::NotPulled {
        same_as: library
            .iter()
            .find(|(_, other)| other == digest)
            .map(|(tag, _)| tag.clone()),
    }
}

/// One registry job: its id, the registry, where logins come from, and the
/// flag Stop sets.
#[derive(Clone)]
pub(crate) struct Job {
    pub id: u64,
    pub host: String,
    pub accounts: Accounts,
    pub cancel: Arc<AtomicBool>,
}

/// Work for the library worker.
pub(crate) enum BrowseRequest {
    /// A catalog page from `next`, the first page without it; then for each
    /// of its repositories the tag list and the record of the tag the row
    /// stands for.
    List { job: Box<Job>, next: Option<String> },
    /// The records of these tags of one repository; `details` adds each
    /// one's metadata record, for a selected tag.
    Read {
        job: Box<Job>,
        repository: String,
        tags: Vec<String>,
        details: bool,
    },
    /// Each read tag's local state again: the local tag a pull writes, the
    /// image the tag names and its artifact type. No registry is contacted.
    Local {
        id: u64,
        tags: Vec<(String, Digest, String)>,
    },
}

/// Why a job ended without finishing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// No answer: offline, a refused connection or a timeout.
    Unreachable(String),
    /// `401`: the registry asks for a login, or `refused` the stored one.
    LoginRequired {
        challenge: Option<Challenge>,
        refused: bool,
    },
    /// The registry answered with an error status, or with something that is
    /// not a registry's answer (no status).
    ServerError { status: Option<u16>, reason: String },
    /// Preferences, the credential store or the library could not be read.
    Local(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum End {
    Done,
    /// The job saw Stop before its next request.
    Stopped,
    Failed(Failure),
}

/// The worker's answers, in order; `id` names the job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BrowseEvent {
    /// The catalog page: the repositories that will be read, those of the
    /// entry's namespaces when it names any, and the next page, if any.
    Page {
        id: u64,
        repositories: Vec<String>,
        next: Option<String>,
    },
    /// One repository of the page, as far as it could be read.
    Repository {
        id: u64,
        row: RepositoryRow,
    },
    /// One tag read on demand.
    Tag {
        id: u64,
        repository: String,
        row: Box<TagRow>,
    },
    Ended {
        id: u64,
        end: End,
    },
    /// Local states by local tag and image, or why the library could not be
    /// read.
    Local {
        id: u64,
        states: Result<Vec<(String, Digest, LocalState)>, String>,
    },
}

/// Answer one request on the library worker; `send` gets every event.
pub(crate) fn run(store: &Store, request: BrowseRequest, send: &dyn Fn(BrowseEvent)) {
    match request {
        BrowseRequest::List { job, next } => {
            let end = list(store, &job, next, send);
            send(BrowseEvent::Ended { id: job.id, end });
        }
        BrowseRequest::Read {
            job,
            repository,
            tags,
            details,
        } => {
            let end = read(store, &job, &repository, &tags, details, send);
            send(BrowseEvent::Ended { id: job.id, end });
        }
        BrowseRequest::Local { id, tags } => send(BrowseEvent::Local {
            id,
            states: library(store).map(|library| {
                tags.into_iter()
                    .map(|(tag, digest, artifact_type)| {
                        let state = local_state(&tag, &digest, &artifact_type, &library);
                        (tag, digest, state)
                    })
                    .collect()
            }),
        }),
    }
}

fn library(store: &Store) -> Result<Vec<(String, Digest)>, String> {
    crate::save_image::local_tags(store)
        .map_err(|error| format!("The library could not be read: {error}"))
}

/// Errors that end a job; any other failure stays with its repository or tag.
fn ends_job(error: &Error) -> bool {
    matches!(
        error,
        Error::Transport(_) | Error::Credentials(_) | Error::Store(_) | Error::Cancelled
    )
}

/// RFC 9110 §15 reason phrases of the statuses a registry answers with.
fn reason_phrase(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => return None,
    })
}

/// The reason phrase, with the registry's own error code when it sent one.
fn reason(status: u16, code: Option<&str>) -> String {
    match (reason_phrase(status), code) {
        (Some(phrase), Some(code)) => format!("{phrase} ({code})"),
        (Some(phrase), None) => phrase.into(),
        (None, Some(code)) => code.into(),
        (None, None) => "error".into(),
    }
}

fn failure(error: Error, client: Option<&Client>) -> End {
    let challenge = || client.and_then(Client::last_challenge);
    End::Failed(match error {
        Error::Cancelled => return End::Stopped,
        Error::Transport(message) => Failure::Unreachable(message),
        Error::AuthRequired { .. } | Error::UnsupportedAuth(_) => Failure::LoginRequired {
            challenge: challenge(),
            refused: false,
        },
        Error::Unauthorized { .. } => Failure::LoginRequired {
            challenge: challenge(),
            refused: true,
        },
        Error::Status { status, code, .. } => Failure::ServerError {
            status: Some(status),
            reason: reason(status, code.as_deref()),
        },
        Error::Forbidden { .. } => Failure::ServerError {
            status: Some(403),
            reason: reason(403, None),
        },
        Error::Store(error) => Failure::Local(format!("The library could not be read: {error}")),
        Error::Credentials(message) => {
            Failure::Local(format!("The credential store did not answer: {message}"))
        }
        Error::Config(message) => Failure::Local(message),
        other => Failure::ServerError {
            status: None,
            reason: other.to_string(),
        },
    })
}

/// The client of Get templates and connection tests, with a stored login
/// sent from the first request, and the registry's entry.
fn connect(job: &Job) -> Result<(Client, RegistryEntry), End> {
    let (config, _) = job
        .accounts
        .settings()
        .map_err(|message| End::Failed(Failure::Local(message)))?;
    let client =
        cli::registry_probe_client(&config, &job.host, &job.accounts.store, &job.accounts.env)
            .map_err(|error| failure(error, None))?;
    client
        .use_credentials_now()
        .map_err(|error| failure(error, Some(&client)))?;
    Ok((client, config.entry_or_public(&job.host)))
}

/// Whether a catalog name is listed under `entry`: an entry that names
/// namespaces lists only the repositories in them.
fn listed_under(entry: &RegistryEntry, repository: &str) -> bool {
    entry.namespaces.is_empty() || matches!(entry.scope(repository), Scope::Namespace { .. })
}

/// One tag's row, with its metadata record when `details`; only an error
/// that ends the job is returned as one.
fn tag_row(
    client: &Client,
    repository: &str,
    tag: &str,
    library: &[(String, Digest)],
    details: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<TagRow, Error> {
    let local_tag = local_tag_of(client.host(), repository, tag);
    let text = |error: &Error| common_failure(error).unwrap_or_else(|| error.to_string());
    let read = match browse::read_tag(client, repository, tag, cancelled) {
        Ok(read) => read,
        Err(error) if ends_job(&error) => return Err(error),
        Err(error) => {
            return Ok(TagRow {
                tag: tag.into(),
                local_tag,
                record: Err(text(&error)),
                local: None,
            })
        }
    };
    let title = if details {
        match browse::read_title(client, repository, &read, cancelled) {
            Ok(title) => Some(Ok(Details {
                edition: title.as_ref().and_then(|t| t.edition.clone()),
                version: title.and_then(|t| t.version),
            })),
            Err(error) if ends_job(&error) => return Err(error),
            Err(error) => Some(Err(text(&error))),
        }
    } else {
        None
    };
    let mut record = TagRecord::from_read(read);
    record.details = title;
    let local = local_state(&local_tag, &record.digest, &record.artifact_type, library);
    Ok(TagRow {
        tag: tag.into(),
        local_tag,
        record: Ok(record),
        local: Some(local),
    })
}

fn list(store: &Store, job: &Job, next: Option<String>, send: &dyn Fn(BrowseEvent)) -> End {
    let stopped = || job.cancel.load(Ordering::SeqCst);
    let library = match library(store) {
        Ok(library) => library,
        Err(message) => return End::Failed(Failure::Local(message)),
    };
    if stopped() {
        return End::Stopped;
    }
    let (client, entry) = match connect(job) {
        Ok(connected) => connected,
        Err(end) => return end,
    };
    let page = match client.catalog_page(CATALOG_PAGE, next.as_deref()) {
        Ok(page) => page,
        Err(error) => return failure(error, Some(&client)),
    };
    let names: Vec<String> = page
        .names
        .into_iter()
        .filter(|name| listed_under(&entry, name))
        .collect();
    send(BrowseEvent::Page {
        id: job.id,
        repositories: names.clone(),
        next: page.next,
    });
    for name in names {
        if stopped() {
            return End::Stopped;
        }
        let mut row = RepositoryRow {
            name,
            tags: Vec::new(),
            representative: None,
            read: Vec::new(),
            error: None,
        };
        match browse::list_tags(&client, &row.name, TAG_PAGE, &stopped) {
            Ok(tags) => {
                row.representative = representative(&tags);
                row.tags = tags;
            }
            Err(error) if ends_job(&error) => return failure(error, Some(&client)),
            Err(error) => {
                row.error = Some(common_failure(&error).unwrap_or_else(|| error.to_string()))
            }
        }
        if let Some(tag) = row.representative.clone() {
            if stopped() {
                return End::Stopped;
            }
            match tag_row(&client, &row.name, &tag, &library, false, &stopped) {
                Ok(tag_row) => row.read.push(tag_row),
                Err(error) => return failure(error, Some(&client)),
            }
        }
        send(BrowseEvent::Repository { id: job.id, row });
    }
    End::Done
}

fn read(
    store: &Store,
    job: &Job,
    repository: &str,
    tags: &[String],
    details: bool,
    send: &dyn Fn(BrowseEvent),
) -> End {
    let stopped = || job.cancel.load(Ordering::SeqCst);
    let library = match library(store) {
        Ok(library) => library,
        Err(message) => return End::Failed(Failure::Local(message)),
    };
    let (client, _) = match connect(job) {
        Ok(connected) => connected,
        Err(end) => return end,
    };
    for tag in tags {
        if stopped() {
            return End::Stopped;
        }
        match tag_row(&client, repository, tag, &library, details, &stopped) {
            Ok(row) => send(BrowseEvent::Tag {
                id: job.id,
                repository: repository.into(),
                row: Box::new(row),
            }),
            Err(error) => return failure(error, Some(&client)),
        }
    }
    End::Done
}

/// Where a registry's listing stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Not contacted in this session.
    Idle,
    /// Rows appear as their manifests arrive.
    Listing,
    Listed,
    /// Stop ended the listing; the rows read so far stay.
    Stopped,
    /// No answer. `stale`: the rows of an earlier listing stay, marked.
    Unreachable {
        reason: String,
        stale: bool,
    },
    /// Nothing is listed or kept until a login, apart from the pages read
    /// before a next page asked for one.
    LoginRequired {
        challenge: Option<Challenge>,
        refused: bool,
    },
    ServerError {
        status: Option<u16>,
        reason: String,
    },
    /// Preferences, the credential store or the library failed.
    Failed(String),
}

/// What the list area says instead of rows, or above them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub heading: String,
    pub text: String,
    pub action: Option<Action>,
}

/// The button a message offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Stop,
    Refresh,
    TryAgain,
    /// Opens Tools › Preferences › Registries on the registry's entry.
    LogIn,
}

/// One namespace node under a registry.
#[derive(Debug)]
pub(crate) struct Namespace<'a> {
    /// The first path component; `None` for repositories whose name is a
    /// single component.
    pub name: Option<String>,
    /// The entry marks it private, so it lists only with a login.
    pub private: bool,
    pub repositories: Vec<&'a RepositoryRow>,
}

/// A running job of a registry.
struct Running {
    id: u64,
    cancel: Arc<AtomicBool>,
}

impl Running {
    fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// A running read on demand: its repository and tags, and whether it reads
/// their metadata records.
struct Reading {
    job: Running,
    repository: String,
    tags: Vec<String>,
    details: bool,
}

/// One registry node and what this session read of it.
pub(crate) struct RegistryView {
    pub host: String,
    pub entry: RegistryEntry,
    /// Preferences list it; the family default registry is shown without.
    pub configured: bool,
    pub state: State,
    /// In catalog order.
    pub repositories: Vec<RepositoryRow>,
    /// The catalog's next page, when the registry has more.
    pub next: Option<String>,
    /// Repositories expected when the page being read is complete.
    pub expected: usize,
    /// The rows of the earlier listing while a refresh runs: an unreachable
    /// registry shows them again, marked stale.
    previous: Option<(Vec<RepositoryRow>, Option<String>)>,
    listing: Option<Running>,
    /// The running listing reads a next page, after rows that stay.
    paging: bool,
    reads: Vec<Reading>,
}

impl RegistryView {
    fn new(entry: RegistryEntry, configured: bool) -> Self {
        Self {
            host: entry.host.clone(),
            entry,
            configured,
            state: State::Idle,
            repositories: Vec::new(),
            next: None,
            expected: 0,
            previous: None,
            listing: None,
            paging: false,
            reads: Vec::new(),
        }
    }

    /// Set every running job's Stop flag and forget the jobs.
    fn cancel_jobs(&mut self) {
        if let Some(listing) = self.listing.take() {
            listing.stop();
        }
        for read in self.reads.drain(..) {
            read.job.stop();
        }
    }

    /// Stop: a running listing ends with the rows read so far.
    fn stop(&mut self) {
        let listing = self.listing.is_some();
        self.cancel_jobs();
        if listing {
            self.state = State::Stopped;
            self.previous = None;
        }
    }

    pub fn repository(&self, name: &str) -> Option<&RepositoryRow> {
        self.repositories.iter().find(|r| r.name == name)
    }

    fn repository_mut(&mut self, name: &str) -> Option<&mut RepositoryRow> {
        self.repositories.iter_mut().find(|r| r.name == name)
    }

    /// The rows are from an earlier listing that the registry no longer
    /// answers.
    pub fn stale(&self) -> bool {
        matches!(self.state, State::Unreachable { stale: true, .. })
    }

    /// Why Pull is unavailable for this registry's rows.
    pub fn pull_blocked(&self) -> Option<String> {
        self.stale().then(|| {
            format!(
                "{} does not answer; Pull waits until Try again reaches it.",
                self.host
            )
        })
    }

    /// The namespace nodes: those the entry names, or for a whole-registry
    /// entry the first path components of the listed repositories, sorted.
    pub fn namespaces(&self) -> Vec<Namespace<'_>> {
        let first = |repository: &str| {
            repository
                .split_once('/')
                .map(|(first, _)| first.to_owned())
        };
        if !self.entry.namespaces.is_empty() {
            return self
                .entry
                .namespaces
                .iter()
                .map(|namespace| Namespace {
                    name: Some(namespace.name.clone()),
                    private: namespace.private && namespace.name != OFFICIAL_NAMESPACE,
                    repositories: self
                        .repositories
                        .iter()
                        .filter(|r| first(&r.name).as_deref() == Some(namespace.name.as_str()))
                        .collect(),
                })
                .collect();
        }
        let mut groups: BTreeMap<Option<String>, Vec<&RepositoryRow>> = BTreeMap::new();
        for repository in &self.repositories {
            groups
                .entry(first(&repository.name))
                .or_default()
                .push(repository);
        }
        groups
            .into_iter()
            .map(|(name, repositories)| Namespace {
                name,
                private: self.entry.private,
                repositories,
            })
            .collect()
    }

    /// The strip above the rows: listing, stopped, stale rows, or a failure
    /// after which the rows listed before stay.
    pub fn banner(&self) -> Option<Message> {
        let read = self.repositories.len();
        let expected = self.expected.max(read);
        match &self.state {
            State::Listing if read > 0 => Some(Message {
                heading: format!("Listing {}…", self.host),
                text: format!(
                    "{read} of {expected} repositories read; rows appear as their manifests arrive."
                ),
                action: Some(Action::Stop),
            }),
            State::Stopped => Some(Message {
                heading: "Listing stopped".into(),
                text: format!(
                    "after {read} of {expected} repositories. Refresh lists the registry again."
                ),
                action: Some(Action::Refresh),
            }),
            State::Unreachable { stale: true, .. } => Some(Message {
                heading: format!("{} does not answer.", self.host),
                text: "These rows are from an earlier listing in this session. Pull is unavailable until the registry answers.".into(),
                action: Some(Action::TryAgain),
            }),
            _ if read > 0 => self.failure_message(true),
            _ => None,
        }
    }

    /// What the list area says instead of rows.
    pub fn message(&self) -> Option<Message> {
        let host = &self.host;
        if !self.repositories.is_empty() {
            return None;
        }
        match &self.state {
            State::Listing => Some(Message {
                heading: format!("Listing {host}…"),
                text: "Reading the catalog. Rows appear as manifests arrive.".into(),
                action: Some(Action::Stop),
            }),
            State::Listed => Some(Message {
                heading: format!("{host} has nothing published yet"),
                text: "Machine images, installation media and firmware packages appear here once they are published.".into(),
                action: Some(Action::Refresh),
            }),
            _ => self.failure_message(false),
        }
    }

    /// A failure's heading, text and action; `kept`: the rows listed before
    /// it stay.
    fn failure_message(&self, kept: bool) -> Option<Message> {
        let host = &self.host;
        let listed = if kept {
            "The repositories listed before stay."
        } else {
            "Nothing was listed."
        };
        let pull = "Image › Pull by reference… still works for a name you know.";
        let login_tail = if kept {
            listed
        } else {
            "Nothing is listed or kept without a login."
        };
        let (heading, text, action) = match &self.state {
            State::Unreachable { reason, .. } => (
                format!("Could not reach {host}"),
                format!("{reason}. Check the network connection. {listed}"),
                Some(Action::TryAgain),
            ),
            State::LoginRequired { refused: true, .. } => (
                format!("{host} refused the login"),
                format!("The registry answered the stored login with 401 Unauthorized. Log in again to list its repositories. {login_tail}"),
                Some(Action::LogIn),
            ),
            State::LoginRequired {
                challenge: Some(Challenge::Basic { realm }),
                ..
            } => (
                format!("{host} asks for a login"),
                format!(
                    "The registry answered with 401 Unauthorized, asking for a Basic login{}. Log in to list its repositories. {login_tail}",
                    realm
                        .as_deref()
                        .map(|realm| format!(" to “{realm}”"))
                        .unwrap_or_default()
                ),
                Some(Action::LogIn),
            ),
            State::LoginRequired {
                challenge: Some(Challenge::Bearer { .. }),
                ..
            } => (
                format!("{host} asks for a login"),
                format!("The registry answered with 401 Unauthorized and a token-service challenge; only Basic authentication is implemented. {login_tail}"),
                None,
            ),
            State::LoginRequired { challenge, .. } => (
                format!("{host} asks for a login"),
                format!(
                    "The registry answered with 401 Unauthorized and {}, which unemupc cannot answer. {login_tail}",
                    match challenge {
                        // A 401 without a challenge parses as the scheme "none".
                        Some(Challenge::Other(scheme)) if scheme != "none" => {
                            format!("a “{scheme}” challenge")
                        }
                        _ => "no challenge".into(),
                    }
                ),
                None,
            ),
            State::ServerError {
                status: Some(404),
                reason,
            } => (
                format!("{host} does not list its repositories"),
                format!("The catalog request ended with 404 {reason}. {listed} {pull}"),
                Some(Action::TryAgain),
            ),
            State::ServerError {
                status: Some(status),
                reason,
            } => (
                format!("{host} answered with an error"),
                format!("A request ended with {status} {reason}. The registry may be restarting. {listed} {pull}"),
                Some(Action::TryAgain),
            ),
            State::ServerError {
                status: None,
                reason,
            } => (
                format!("{host} answered with an error"),
                format!("Its answer was not a registry's: {reason}. {listed} {pull}"),
                Some(Action::TryAgain),
            ),
            State::Failed(message) => (
                format!("Could not list {host}"),
                if kept {
                    format!("{message} {listed}")
                } else {
                    message.clone()
                },
                Some(Action::TryAgain),
            ),
            _ => return None,
        };
        Some(Message {
            heading,
            text,
            action,
        })
    }

    /// The listing job ended.
    fn listing_ended(&mut self, end: End) {
        self.listing = None;
        match end {
            End::Done => {
                self.state = State::Listed;
                self.previous = None;
            }
            End::Stopped => {
                self.state = State::Stopped;
                self.previous = None;
            }
            End::Failed(Failure::Unreachable(reason)) => {
                if let Some((repositories, next)) = self.previous.take() {
                    self.repositories = repositories;
                    self.next = next;
                }
                self.state = State::Unreachable {
                    reason,
                    stale: !self.repositories.is_empty(),
                };
            }
            End::Failed(failure) => {
                // A next page that fails keeps the pages before it.
                if !self.paging {
                    self.repositories.clear();
                    self.next = None;
                }
                self.previous = None;
                self.state = match failure {
                    Failure::LoginRequired { challenge, refused } => {
                        State::LoginRequired { challenge, refused }
                    }
                    Failure::ServerError { status, reason } => {
                        State::ServerError { status, reason }
                    }
                    Failure::Local(message) | Failure::Unreachable(message) => {
                        State::Failed(message)
                    }
                };
            }
        }
    }

    /// A read on demand ended. An unreachable registry keeps its rows,
    /// marked stale; any other failure stays with the repository.
    fn read_ended(&mut self, repository: &str, end: End) {
        match end {
            End::Done | End::Stopped => {}
            End::Failed(Failure::Unreachable(reason)) => {
                if self.listing.is_none() {
                    self.state = State::Unreachable {
                        stale: !self.repositories.is_empty(),
                        reason,
                    };
                }
            }
            End::Failed(failure) => {
                let message = match failure {
                    Failure::LoginRequired { .. } => {
                        format!("{} asks for a login to read these tags.", self.host)
                    }
                    Failure::ServerError { status, reason } => match status {
                        Some(status) => format!("The registry answered {status} {reason}."),
                        None => reason,
                    },
                    Failure::Local(message) | Failure::Unreachable(message) => message,
                };
                if let Some(row) = self.repository_mut(repository) {
                    row.error = Some(message);
                }
            }
        }
    }
}

/// The Remotes group of the library: one node per registry.
pub(crate) struct Remotes {
    accounts: Accounts,
    pub registries: Vec<RegistryView>,
    /// Why preferences could not be read.
    pub error: Option<String>,
    /// The local-state request whose answer is awaited.
    local: Option<u64>,
}

impl Drop for Remotes {
    /// Closing the library stops every job.
    fn drop(&mut self) {
        for view in &mut self.registries {
            view.cancel_jobs();
        }
    }
}

/// The root registries: the default registry, the family default registry,
/// then the other configured ones in preferences order; each with its entry
/// and whether preferences list it.
fn roots(accounts: &Accounts) -> Result<Vec<(RegistryEntry, bool)>, String> {
    let (config, default_host) = accounts.settings()?;
    let mut hosts = vec![default_host];
    for host in
        std::iter::once(DEFAULT_REGISTRY).chain(config.registries.iter().map(|e| e.host.as_str()))
    {
        if !hosts.iter().any(|h| h == host) {
            hosts.push(host.into());
        }
    }
    Ok(hosts
        .into_iter()
        .map(|host| {
            let configured = config.registries.iter().any(|e| e.host == host);
            (config.entry_or_public(&host), configured)
        })
        .collect())
}

impl Remotes {
    pub fn open(accounts: Accounts) -> Self {
        let mut remotes = Self {
            accounts,
            registries: Vec::new(),
            error: None,
            local: None,
        };
        remotes.reload();
        remotes
    }

    /// Read the registries from preferences again; what a registry still
    /// listed has shown stays.
    pub fn reload(&mut self) {
        let roots = match roots(&self.accounts) {
            Ok(roots) => roots,
            Err(message) => {
                self.error = Some(message);
                return;
            }
        };
        self.error = None;
        let mut old = std::mem::take(&mut self.registries);
        for (entry, configured) in roots {
            let view = match old.iter().position(|v| v.host == entry.host) {
                Some(index) => {
                    let mut view = old.remove(index);
                    view.entry = entry;
                    view.configured = configured;
                    view
                }
                None => RegistryView::new(entry, configured),
            };
            self.registries.push(view);
        }
        for mut gone in old {
            gone.cancel_jobs();
        }
    }

    pub fn registry(&self, host: &str) -> Option<&RegistryView> {
        self.registries.iter().find(|v| v.host == host)
    }

    fn job(&self, host: &str) -> (Box<Job>, Running) {
        let id = next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            id,
            host: host.into(),
            accounts: self.accounts.clone(),
            cancel: cancel.clone(),
        };
        (Box::new(job), Running { id, cancel })
    }

    fn start_listing(&mut self, host: &str, next: Option<String>) -> Option<BrowseRequest> {
        let (job, running) = self.job(host);
        let view = self.registries.iter_mut().find(|v| v.host == host)?;
        view.listing = Some(running);
        view.paging = next.is_some();
        view.state = State::Listing;
        view.expected = view.repositories.len();
        Some(BrowseRequest::List { job, next })
    }

    /// Opening a registry's node lists it the first time in this session.
    pub fn open_node(&mut self, host: &str) -> Option<BrowseRequest> {
        if self.registry(host)?.state != State::Idle {
            return None;
        }
        self.start_listing(host, None)
    }

    /// Refresh and Try again: list the registry again. The rows shown stay
    /// aside until the new listing answers, for an unreachable registry.
    pub fn refresh(&mut self, host: &str) -> Option<BrowseRequest> {
        let view = self.registries.iter_mut().find(|v| v.host == host)?;
        let listing = view.state == State::Listing;
        view.cancel_jobs();
        let shown = (std::mem::take(&mut view.repositories), view.next.take());
        if !(listing && view.previous.is_some()) {
            view.previous = (!shown.0.is_empty()).then_some(shown);
        }
        self.start_listing(host, None)
    }

    /// Next 50 ›: the catalog's next page, added to the rows.
    pub fn next_page(&mut self, host: &str) -> Option<BrowseRequest> {
        let view = self.registry(host)?;
        if view.state != State::Listed {
            return None;
        }
        let next = view.next.clone()?;
        self.start_listing(host, Some(next))
    }

    /// Stop: the listing ends at its next read; the rows read so far stay.
    pub fn stop(&mut self, host: &str) {
        if let Some(view) = self.registries.iter_mut().find(|v| v.host == host) {
            view.stop();
        }
    }

    /// Stop every registry's listing and reads, as the window does before a
    /// machine starts, so the worker is free for it.
    pub fn stop_all(&mut self) {
        for view in &mut self.registries {
            view.stop();
        }
    }

    /// Read these tags of a repository, but none a running read of it already
    /// covers.
    fn read(
        &mut self,
        host: &str,
        repository: &str,
        tags: Vec<String>,
        details: bool,
    ) -> Option<BrowseRequest> {
        let view = self.registry(host)?;
        view.repository(repository)?;
        let covered = |tag: &String| {
            view.reads.iter().any(|r| {
                r.repository == repository && (r.details || !details) && r.tags.contains(tag)
            })
        };
        let tags: Vec<String> = tags.into_iter().filter(|tag| !covered(tag)).collect();
        if tags.is_empty() || view.stale() {
            return None;
        }
        let (job, running) = self.job(host);
        let view = self.registries.iter_mut().find(|v| v.host == host)?;
        view.reads.push(Reading {
            job: running,
            repository: repository.into(),
            tags: tags.clone(),
            details,
        });
        Some(BrowseRequest::Read {
            job,
            repository: repository.into(),
            tags,
            details,
        })
    }

    /// Expanding a repository reads the tags not read yet.
    pub fn expand(&mut self, host: &str, repository: &str) -> Option<BrowseRequest> {
        let row = self.registry(host)?.repository(repository)?;
        let tags = row
            .tags
            .iter()
            .filter(|tag| row.row(tag).is_none())
            .cloned()
            .collect();
        self.read(host, repository, tags, false)
    }

    /// Selecting a tag reads it with its metadata record, unless that is
    /// read already; a tag whose read failed is read again.
    pub fn select(&mut self, host: &str, repository: &str, tag: &str) -> Option<BrowseRequest> {
        let row = self.registry(host)?.repository(repository)?;
        let done = row
            .row(tag)
            .is_some_and(|row| row.record.as_ref().is_ok_and(|r| r.details.is_some()));
        if done {
            return None;
        }
        self.read(host, repository, vec![tag.into()], true)
    }

    /// The library changed: compare every read tag with it again.
    pub fn library_changed(&mut self) -> Option<BrowseRequest> {
        let tags: Vec<(String, Digest, String)> = self
            .registries
            .iter()
            .flat_map(|view| &view.repositories)
            .flat_map(|repository| &repository.read)
            .filter_map(|row| {
                let record = row.record.as_ref().ok()?;
                Some((
                    row.local_tag.clone(),
                    record.digest.clone(),
                    record.artifact_type.clone(),
                ))
            })
            .collect();
        if tags.is_empty() {
            return None;
        }
        let id = next_id();
        self.local = Some(id);
        Some(BrowseRequest::Local { id, tags })
    }

    /// Take one of the worker's answers; answers of stopped or replaced jobs
    /// change nothing.
    pub fn on_event(&mut self, event: BrowseEvent) {
        let listing_of =
            |view: &RegistryView, id: u64| view.listing.as_ref().is_some_and(|r| r.id == id);
        match event {
            BrowseEvent::Page {
                id,
                repositories,
                next,
            } => {
                if let Some(view) = self.registries.iter_mut().find(|v| listing_of(v, id)) {
                    view.expected = view.repositories.len() + repositories.len();
                    view.next = next;
                }
            }
            BrowseEvent::Repository { id, row } => {
                if let Some(view) = self.registries.iter_mut().find(|v| listing_of(v, id)) {
                    match view.repository_mut(&row.name) {
                        Some(existing) => *existing = row,
                        None => view.repositories.push(row),
                    }
                }
            }
            BrowseEvent::Tag {
                id,
                repository,
                row,
            } => {
                let view = self
                    .registries
                    .iter_mut()
                    .find(|v| v.reads.iter().any(|r| r.job.id == id));
                if let Some(existing) = view.and_then(|v| v.repository_mut(&repository)) {
                    existing.upsert(*row);
                }
            }
            BrowseEvent::Ended { id, end } => {
                for view in &mut self.registries {
                    if listing_of(view, id) {
                        view.listing_ended(end);
                        return;
                    }
                    if let Some(index) = view.reads.iter().position(|r| r.job.id == id) {
                        let reading = view.reads.remove(index);
                        view.read_ended(&reading.repository, end);
                        return;
                    }
                }
            }
            BrowseEvent::Local { id, states } => {
                if self.local != Some(id) {
                    return;
                }
                self.local = None;
                let states = match states {
                    Ok(states) => states,
                    Err(reason) => {
                        // The local states shown may be out of date: say so on
                        // every registry that shows rows.
                        for view in &mut self.registries {
                            if view.listing.is_none() && !view.repositories.is_empty() {
                                view.state = State::Failed(reason.clone());
                            }
                        }
                        return;
                    }
                };
                for view in &mut self.registries {
                    for row in view
                        .repositories
                        .iter_mut()
                        .flat_map(|repository| repository.read.iter_mut())
                    {
                        let found = states.iter().find(|(tag, digest, _)| {
                            *tag == row.local_tag
                                && row.record.as_ref().is_ok_and(|r| r.digest == *digest)
                        });
                        if let Some((_, _, state)) = found {
                            row.local = Some(state.clone());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_registry::{
        credential_store::{Secret, SessionStore},
        credentials::EnvCredentials,
        test_support::{MockRegistry, Policy},
    };
    use image_store::{media_set, BlobSource, ConfigValidator, Descriptor, Manifest};
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::{mpsc, Mutex},
    };

    /// A neutral machine provider for local tags: the config lists its
    /// payload digests.
    struct Synthetic;
    impl ConfigValidator for Synthetic {
        fn platform(&self) -> &str {
            "synthetic"
        }
        fn validate(
            &self,
            table: &toml::Table,
            _: &[Descriptor],
            _: &dyn BlobSource,
        ) -> image_store::Result<Vec<Digest>> {
            table
                .get("blobs")
                .and_then(toml::Value::as_array)
                .ok_or_else(|| image_store::Error::Invalid("missing blobs".into()))?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().parse())
                .collect()
        }
    }

    /// A synthetic artifact: its manifest and every blob it names.
    struct Image {
        manifest: Manifest,
        bytes: Vec<u8>,
        digest: Digest,
        config: Vec<u8>,
        layers: Vec<Vec<u8>>,
    }

    impl Image {
        fn new(manifest: Manifest, config: Vec<u8>, layers: Vec<Vec<u8>>) -> Self {
            let bytes = manifest.encode().unwrap();
            Self {
                digest: Digest::of(&bytes),
                manifest,
                bytes,
                config,
                layers,
            }
        }

        fn layer_digests(&self) -> Vec<Digest> {
            self.layers.iter().map(|l| Digest::of(l)).collect()
        }

        /// Publish it to `registry` as `repository:tag`.
        fn seed(&self, registry: &MockRegistry, repository: &str, tag: &str) {
            registry.seed_blob(repository, &self.config);
            for layer in &self.layers {
                registry.seed_blob(repository, layer);
            }
            registry.seed_manifest(repository, tag, &self.bytes);
        }

        /// Tag it in the library as `tag`.
        fn tag(&self, store: &Store, tag: &str) {
            for blob in std::iter::once(&self.config).chain(&self.layers) {
                store.import(blob.as_slice(), None).unwrap();
            }
            let descriptor = store.import_manifest(&self.manifest).unwrap();
            assert_eq!(descriptor.digest, self.digest);
            store
                .update_tag(tag, &descriptor, None, &Synthetic)
                .unwrap();
        }
    }

    fn blob(kind: &str, bytes: &[u8]) -> Descriptor {
        Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64)
    }

    /// A metadata record with one title, its edition and version named after
    /// `label`.
    fn metadata(label: &str) -> Vec<u8> {
        format!(
            r#"{{"schema":1,"platform":"synthetic","titles":[{{"id":"t","title":"{label}","edition":"{label} edition","version":"{label} version"}}]}}"#
        )
        .into_bytes()
    }

    /// A machine image whose one payload holds `label`.
    fn machine(label: &str, annotations: &[(&str, &str)]) -> Image {
        let payload = label.as_bytes().to_vec();
        let config = format!(
            "schema = 1\nplatform = \"synthetic\"\nblobs = [\"{}\"]\n",
            Digest::of(&payload)
        )
        .into_bytes();
        let mut manifest = Manifest::new(
            "synthetic",
            blob(mt::CONFIG, &config),
            vec![
                blob(mt::FIRMWARE, &payload),
                blob(mt::METADATA, &metadata(label)),
            ],
        )
        .unwrap();
        for (key, value) in annotations {
            manifest.annotations.insert((*key).into(), (*value).into());
        }
        Image::new(manifest, config, vec![payload, metadata(label)])
    }

    /// An installation media set of one disk per format.
    fn media_set(label: &str, formats: &[&str]) -> Image {
        let disks: Vec<Vec<u8>> = (0..formats.len())
            .map(|i| format!("{label} disk {i}").into_bytes())
            .collect();
        let config = media_set::Config {
            schema: 1,
            platform: "synthetic".into(),
            media: formats
                .iter()
                .zip(&disks)
                .enumerate()
                .map(|(i, (format, disk))| media_set::Medium {
                    id: format!("disk-{i}"),
                    label: format!("Disk {i}"),
                    format: pc_image::media_set::format(format).unwrap(),
                    blob: Digest::of(disk),
                })
                .collect(),
            extras: Vec::new(),
        }
        .encode()
        .unwrap();
        let mut layers: Vec<Descriptor> = disks.iter().map(|d| blob(mt::FLOPPY, d)).collect();
        layers.push(blob(mt::METADATA, &metadata(label)));
        let mut manifest =
            Manifest::new_media_set("synthetic", blob(mt::MEDIA_SET_CONFIG, &config), layers)
                .unwrap();
        manifest
            .annotations
            .insert(TITLE.into(), format!("{label} set"));
        let mut all = disks;
        all.push(metadata(label));
        Image::new(manifest, config, all)
    }

    /// A store, preferences and the accounts that read them, each in its own
    /// directory.
    struct Setup {
        _temp: assembly::test_support::TempDir,
        store: Store,
        preferences: PathBuf,
        accounts: Accounts,
    }

    impl Setup {
        /// Preferences that list `entries` (TOML `[[registries]]` bodies).
        fn new(entries: &[String]) -> Self {
            let temp = assembly::test_support::TempDir::new("remote-browse");
            let store = Store::open(temp.path().join("store")).unwrap();
            let config = temp.path().join("config");
            std::fs::create_dir(&config).unwrap();
            let preferences = config.join("preferences.toml");
            let text: String = entries
                .iter()
                .map(|e| format!("[[registries]]\n{e}\n"))
                .collect();
            std::fs::write(&preferences, text).unwrap();
            let accounts = Accounts {
                path: Some(preferences.clone()),
                store: Arc::new(SessionStore::default()),
                env: EnvCredentials::default(),
                default_override: None,
            };
            Self {
                _temp: temp,
                store,
                preferences,
                accounts,
            }
        }

        /// Preferences that list the mock registry as a whole-registry entry.
        fn with(registry: &MockRegistry) -> Self {
            Self::new(&[plain(&registry.host())])
        }

        fn remotes(&self) -> Remotes {
            Remotes::open(self.accounts.clone())
        }

        /// Run one request as the worker would and hand its answers to `remotes`.
        fn answer(
            &self,
            remotes: &mut Remotes,
            request: Option<BrowseRequest>,
        ) -> Vec<BrowseEvent> {
            let events = Mutex::new(Vec::new());
            run(&self.store, request.expect("a request"), &|event| {
                events.lock().unwrap().push(event)
            });
            let events = events.into_inner().unwrap();
            for event in events.clone() {
                remotes.on_event(event);
            }
            events
        }
    }

    /// Run `request` on a thread until `registry` holds the request that
    /// starts with `prefix`, handing the answers so far to `remotes`; then
    /// press Stop for `host`, release the request and hand over the rest.
    /// Returns the answers after Stop.
    fn stop_while_held(
        setup: &Setup,
        remotes: &mut Remotes,
        registry: &MockRegistry,
        host: &str,
        prefix: &str,
        request: Option<BrowseRequest>,
    ) -> Vec<BrowseEvent> {
        registry.hold_next(prefix);
        let request = request.expect("a request");
        let (send, receive) = mpsc::channel();
        let root = setup.store.root().to_owned();
        let worker = std::thread::spawn(move || {
            let store = Store::open(root).unwrap();
            run(&store, request, &|event| send.send(event).unwrap());
        });
        for () in crate::test_wait::rounds() {
            if registry.held() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(registry.held(), "{prefix} was never asked");
        for event in receive.try_iter() {
            remotes.on_event(event);
        }
        remotes.stop(host);
        registry.release();
        worker.join().unwrap();
        let late: Vec<_> = receive.try_iter().collect();
        for event in late.clone() {
            remotes.on_event(event);
        }
        late
    }

    fn plain(host: &str) -> String {
        format!("host = \"{host}\"\nplain_http = true")
    }

    fn open_registry() -> Policy {
        Policy {
            anonymous_read: vec!["*".into()],
            ..Policy::default()
        }
    }

    fn ended(events: &[BrowseEvent]) -> &End {
        match events.last() {
            Some(BrowseEvent::Ended { end, .. }) => end,
            other => panic!("the job did not end: {other:?}"),
        }
    }

    fn names(view: &RegistryView) -> Vec<&str> {
        view.repositories.iter().map(|r| r.name.as_str()).collect()
    }

    /// `GET` requests for blobs, by digest.
    fn blob_requests(registry: &MockRegistry) -> Vec<String> {
        registry
            .log()
            .into_iter()
            .filter(|line| line.starts_with("GET ") && line.contains("/blobs/"))
            .filter_map(|line| line.rsplit('/').next().map(str::to_owned))
            .collect()
    }

    #[test]
    fn roots_put_the_default_registry_first_and_contact_none() {
        let registry = MockRegistry::start(open_registry());
        let setup = Setup::new(&[
            plain(&registry.host()),
            "host = \"nas.example:5000\"\nprivate = true".into(),
        ]);
        let remotes = setup.remotes();
        let hosts: Vec<_> = remotes.registries.iter().map(|v| v.host.as_str()).collect();
        assert_eq!(
            hosts,
            [DEFAULT_REGISTRY, &registry.host(), "nas.example:5000"]
        );
        assert!(!remotes.registries[0].configured);
        assert!(remotes.registries[1].configured);
        assert!(remotes.registries.iter().all(|v| v.state == State::Idle));
        assert!(
            registry.log().is_empty(),
            "nothing is contacted before a node opens"
        );

        // A configured default registry comes first; the family default follows.
        std::fs::write(
            &setup.preferences,
            format!(
                "default_registry = \"{}\"\n[[registries]]\n{}\n",
                registry.host(),
                plain(&registry.host())
            ),
        )
        .unwrap();
        let remotes = setup.remotes();
        let hosts: Vec<_> = remotes.registries.iter().map(|v| v.host.as_str()).collect();
        assert_eq!(hosts, [registry.host().as_str(), DEFAULT_REGISTRY]);
    }

    #[test]
    fn the_catalog_is_read_fifty_repositories_a_page() {
        let registry = MockRegistry::start(open_registry());
        let repositories: Vec<String> = (0..60).map(|i| format!("games/r{i:02}")).collect();
        registry.seed_repositories(&repositories.iter().map(String::as_str).collect::<Vec<_>>());
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        assert!(remotes.open_node(&host).is_none(), "one listing at a time");
        let events = setup.answer(&mut remotes, request);
        let BrowseEvent::Page {
            repositories: page,
            next,
            ..
        } = &events[0]
        else {
            panic!("{:?}", events[0]);
        };
        assert_eq!(page.len(), CATALOG_PAGE);
        assert!(
            next.as_deref().is_some_and(|n| n.contains("n=50")),
            "{next:?}"
        );
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(
            names(view),
            repositories[..50]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert!(view.next.is_some(), "Next 50 › is offered");

        let request = remotes.next_page(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(
            names(view),
            repositories.iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert_eq!(view.next, None);
        assert!(
            remotes.next_page(&host).is_none(),
            "the last page has no next"
        );
        let catalog = registry
            .log()
            .iter()
            .filter(|line| *line == "GET /v2/_catalog")
            .count();
        assert_eq!(catalog, 2);
    }

    #[test]
    fn tags_follow_the_registry_paging_and_only_the_newest_is_read() {
        let registry = MockRegistry::start(open_registry());
        let image = machine("many", &[]);
        let tags: Vec<String> = (0..=TAG_PAGE).map(|i| format!("1.0.{i}")).collect();
        for tag in &tags {
            image.seed(&registry, "games/many", tag);
        }
        image.seed(&registry, "games/many", "latest");
        image.seed(&registry, "games/many", "2.0.0-rc.1");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("games/many")
            .unwrap()
            .clone();
        assert_eq!(row.tags.len(), TAG_PAGE + 3, "every page was read");
        assert_eq!(
            row.representative.as_deref(),
            Some("1.0.100"),
            "the newest release"
        );
        assert_eq!(row.read.len(), 1);
        let log = registry.log();
        assert_eq!(
            log.iter()
                .filter(|line| *line == "GET /v2/games/many/tags/list")
                .count(),
            2,
            "{log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|line| line.starts_with("GET /v2/games/many/manifests/"))
                .count(),
            1,
            "only the representative tag's manifest is read while listing"
        );
        assert_eq!(
            representative(&["latest".into(), "beta".into()]).as_deref(),
            Some("latest")
        );
        assert_eq!(
            representative(&["2.0.0-rc.1".into(), "2.0.0-rc.2".into()]).as_deref(),
            Some("2.0.0-rc.2"),
            "pre-releases when there is no release"
        );
        let pick =
            |tags: &[&str]| representative(&tags.iter().map(|t| t.to_string()).collect::<Vec<_>>());
        assert_eq!(pick(&["1.4", "1.4-720k"]).as_deref(), Some("1.4"));
        assert_eq!(pick(&["1.4-720k", "1.4"]).as_deref(), Some("1.4"));
        assert_eq!(
            pick(&["1.4-720k", "1.4-r2", "1.4"]).as_deref(),
            Some("1.4-r2")
        );
        assert_eq!(pick(&["1", "10", "2"]).as_deref(), Some("10"), "firmware");
        assert_eq!(pick(&["stable", "beta"]).as_deref(), Some("beta"));
    }

    #[test]
    fn records_read_the_manifest_and_config_record_but_never_a_layer() {
        let registry = MockRegistry::start(open_registry());
        let set = media_set("Monkey Island", &["720K", "720K", "1.44M"]);
        set.seed(&registry, "private/media/monkey-island", "1.1-vga");
        let game = machine(
            "dune",
            &[
                (TITLE, "Dune II"),
                (VERSION, "1.07"),
                (MIN_APP_VERSION, "99.0.0"),
                ("example.unknown", "kept"),
            ],
        );
        game.seed(&registry, "private/games/dune2", "1.0.0");
        let older = machine("dune old", &[(TITLE, "Dune II")]);
        older.seed(&registry, "private/games/dune2", "0.9.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);

        let view = remotes.registry(&host).unwrap();
        let row = view.repository("private/media/monkey-island").unwrap();
        let record = row.representative_row().unwrap().record.clone().unwrap();
        assert_eq!(record.artifact_type, mt::MEDIA_SET);
        assert_eq!(record.formats, ["720K", "1.44M"]);
        assert_eq!(record.title.as_deref(), Some("Monkey Island set"));
        assert_eq!(record.digest, set.digest);
        let size =
            set.bytes.len() + set.config.len() + set.layers.iter().map(Vec::len).sum::<usize>();
        assert_eq!(record.size, size as u64);
        assert_eq!(record.min_app_version, None);

        let row = view.repository("private/games/dune2").unwrap();
        assert_eq!(row.representative.as_deref(), Some("1.0.0"));
        let record = row.representative_row().unwrap().record.clone().unwrap();
        assert_eq!(record.artifact_type, mt::MACHINE);
        assert_eq!(record.title.as_deref(), Some("Dune II"));
        assert_eq!(record.version.as_deref(), Some("1.07"));
        assert_eq!(
            record.min_app_version.as_deref(),
            Some("99.0.0"),
            "kept raw"
        );
        assert_eq!(record.annotations["example.unknown"], "kept");
        assert!(record.formats.is_empty());

        // Expanding reads the other tag; a read tag is never read again.
        let request = remotes.expand(&host, "private/games/dune2");
        let events = setup.answer(&mut remotes, request);
        assert_eq!(ended(&events), &End::Done);
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("private/games/dune2")
            .unwrap();
        assert_eq!(row.read.len(), 2);
        assert_eq!(
            row.row("0.9.0").unwrap().record.as_ref().unwrap().digest,
            older.digest
        );
        assert!(row
            .row("0.9.0")
            .unwrap()
            .record
            .as_ref()
            .unwrap()
            .details
            .is_none());
        assert!(remotes.expand(&host, "private/games/dune2").is_none());

        // Only the media set's config record was fetched while listing and
        // expanding: no machine config, and no layer of any image.
        assert_eq!(
            blob_requests(&registry),
            [Digest::of(&set.config).to_string()]
        );

        // Selecting a tag adds its metadata record, and only that layer.
        registry.clear_log();
        let request = remotes.select(&host, "private/games/dune2", "0.9.0");
        setup.answer(&mut remotes, request);
        let record = |remotes: &Remotes, repository: &str, tag: &str| {
            remotes
                .registry(&host)
                .unwrap()
                .repository(repository)
                .unwrap()
                .row(tag)
                .unwrap()
                .record
                .clone()
                .unwrap()
        };
        assert_eq!(
            record(&remotes, "private/games/dune2", "0.9.0").details,
            Some(Ok(Details {
                edition: Some("dune old edition".into()),
                version: Some("dune old version".into()),
            }))
        );
        assert_eq!(
            blob_requests(&registry),
            [Digest::of(&older.layers[1]).to_string()]
        );
        assert!(
            remotes
                .select(&host, "private/games/dune2", "0.9.0")
                .is_none(),
            "read once"
        );
        registry.clear_log();
        let request = remotes.select(&host, "private/media/monkey-island", "1.1-vga");
        setup.answer(&mut remotes, request);
        let details = record(&remotes, "private/media/monkey-island", "1.1-vga").details;
        assert_eq!(
            details.unwrap().unwrap().edition.as_deref(),
            Some("Monkey Island edition")
        );
        let metadata = set.layers.last().unwrap();
        assert_eq!(
            blob_requests(&registry),
            [
                Digest::of(&set.config).to_string(),
                Digest::of(metadata).to_string()
            ],
            "the set's config record again, then its metadata; never a disk"
        );
        for layer in set.layer_digests().into_iter().chain(game.layer_digests()) {
            if layer != Digest::of(metadata) {
                assert!(!blob_requests(&registry).contains(&layer.to_string()));
            }
        }
    }

    #[test]
    fn stop_ends_the_listing_at_its_next_read_and_keeps_its_rows() {
        let registry = MockRegistry::start(open_registry());
        for (name, label) in [("games/a", "a"), ("games/b", "b"), ("games/c", "c")] {
            machine(label, &[]).seed(&registry, name, "1.0.0");
        }
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        let late = stop_while_held(
            &setup,
            &mut remotes,
            &registry,
            &host,
            "GET /v2/games/b/tags/list",
            request,
        );
        assert_eq!(ended(&late), &End::Stopped, "the worker saw Stop");
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Stopped);
        assert_eq!(names(view), ["games/a"]);
        let banner = view.banner().unwrap();
        assert_eq!(banner.heading, "Listing stopped");
        assert!(
            banner.text.contains("after 1 of 3 repositories"),
            "{}",
            banner.text
        );
        assert!(
            !registry.log().iter().any(|line| line.contains("games/c")),
            "nothing after the held read was asked"
        );
        assert!(
            !registry
                .log()
                .iter()
                .any(|line| line.starts_with("GET /v2/games/b/manifests/")),
            "the stop took effect at the next read"
        );
        // Refresh lists the registry again.
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["games/a", "games/b", "games/c"]);
    }

    /// A loopback port nothing listens on.
    fn closed_port() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        drop(listener);
        host
    }

    #[test]
    fn an_unreachable_registry_lists_nothing() {
        let host = closed_port();
        let setup = Setup::new(&[plain(&host)]);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        let events = setup.answer(&mut remotes, request);
        assert!(matches!(
            ended(&events),
            End::Failed(Failure::Unreachable(_))
        ));
        let view = remotes.registry(&host).unwrap();
        assert!(
            matches!(&view.state, State::Unreachable { stale: false, .. }),
            "{:?}",
            view.state
        );
        assert!(view.repositories.is_empty());
        assert_eq!(view.banner(), None);
        let message = view.message().unwrap();
        assert_eq!(message.heading, format!("Could not reach {host}"));
        assert!(
            message.text.ends_with("Nothing was listed."),
            "{}",
            message.text
        );
        assert_eq!(message.action, Some(Action::TryAgain));
    }

    #[test]
    fn an_unreachable_registry_keeps_its_earlier_rows_marked_stale() {
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        machine("b", &[]).seed(&registry, "games/b", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        assert_eq!(remotes.registry(&host).unwrap().pull_blocked(), None);
        drop(registry);

        let request = remotes.refresh(&host);
        assert!(
            remotes.registry(&host).unwrap().repositories.is_empty(),
            "a refresh replaces the rows"
        );
        let events = setup.answer(&mut remotes, request);
        assert!(matches!(
            ended(&events),
            End::Failed(Failure::Unreachable(_))
        ));
        let view = remotes.registry(&host).unwrap();
        assert!(view.stale());
        assert_eq!(names(view), ["games/a", "games/b"], "the earlier rows stay");
        assert_eq!(view.message(), None, "the rows are shown, under a banner");
        let banner = view.banner().unwrap();
        assert_eq!(banner.heading, format!("{host} does not answer."));
        assert_eq!(banner.action, Some(Action::TryAgain));
        assert!(view.pull_blocked().is_some(), "Pull waits");
        assert!(remotes.expand(&host, "games/a").is_none());

        // Try again while it still does not answer keeps them stale.
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert!(view.stale());
        assert_eq!(names(view), ["games/a", "games/b"]);
    }

    #[test]
    fn server_errors_name_their_status_and_reason() {
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        machine("b", &[]).seed(&registry, "games/b", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        registry.fail_requests("GET /v2/_catalog", 500);
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(
            view.state,
            State::ServerError {
                status: Some(500),
                reason: "Internal Server Error (UNKNOWN)".into()
            }
        );
        let message = view.message().unwrap();
        assert_eq!(message.heading, format!("{host} answered with an error"));
        assert!(
            message.text.contains("500 Internal Server Error"),
            "{}",
            message.text
        );
        assert!(view.repositories.is_empty());

        // One repository's failure stays with its row; the others list.
        registry.fail_requests("GET /v2/games/a/tags/list", 503);
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        let failed = view.repository("games/a").unwrap();
        assert!(
            failed.error.as_deref().is_some_and(|e| e.contains("503")),
            "{failed:?}"
        );
        assert!(view
            .repository("games/b")
            .unwrap()
            .representative_row()
            .is_some());

        // A registry without a catalog says so.
        registry.stop_failing();
        let none = MockRegistry::start(Policy {
            no_catalog: true,
            ..open_registry()
        });
        let setup = Setup::with(&none);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&none.host());
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&none.host()).unwrap();
        assert!(matches!(
            view.state,
            State::ServerError {
                status: Some(404),
                ..
            }
        ));
        assert_eq!(
            view.message().unwrap().heading,
            format!("{} does not list its repositories", none.host())
        );
    }

    #[test]
    fn a_login_request_reports_the_challenge_and_keeps_nothing() {
        let registry = MockRegistry::start(Policy {
            user: Some(("ci".into(), "secret".into())),
            catalog_requires_login: true,
            ..Policy::default()
        });
        machine("a", &[]).seed(&registry, "private/games/a", "1.0.0");
        let host = registry.host();
        let setup = Setup::new(&[format!("{}\nuser = \"ci\"", plain(&host))]);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(
            view.state,
            State::LoginRequired {
                challenge: Some(Challenge::Basic {
                    realm: Some("mock".into())
                }),
                refused: false
            }
        );
        let message = view.message().unwrap();
        assert_eq!(message.heading, format!("{host} asks for a login"));
        assert!(
            message.text.contains("401 Unauthorized"),
            "{}",
            message.text
        );
        assert_eq!(message.action, Some(Action::LogIn));
        assert_eq!(
            registry.authorized_log(),
            Vec::<String>::new(),
            "no login was invented"
        );

        // A login lists the namespace; the login goes with the first request.
        setup
            .accounts
            .store
            .store(&host, "ci", &Secret::new("secret"))
            .unwrap();
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["private/games/a"]);
        assert!(registry
            .authorized_log()
            .contains(&"GET /v2/_catalog".to_owned()));

        // A refused login keeps nothing, not even the rows listed before.
        setup
            .accounts
            .store
            .store(&host, "ci", &Secret::new("wrong"))
            .unwrap();
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert!(
            matches!(view.state, State::LoginRequired { refused: true, .. }),
            "{:?}",
            view.state
        );
        assert!(view.repositories.is_empty());
        assert_eq!(
            view.message().unwrap().heading,
            format!("{host} refused the login")
        );
        let request = remotes.refresh(&host);
        drop(registry);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert!(
            matches!(view.state, State::Unreachable { stale: false, .. }),
            "nothing was kept to show stale"
        );
    }

    #[test]
    fn log_in_is_offered_only_for_a_basic_challenge() {
        let registry = MockRegistry::start(Policy {
            bearer_challenge: true,
            catalog_requires_login: true,
            ..Policy::default()
        });
        let host = registry.host();
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert!(
            matches!(
                view.state,
                State::LoginRequired {
                    challenge: Some(Challenge::Bearer { .. }),
                    ..
                }
            ),
            "{:?}",
            view.state
        );
        assert_eq!(view.message().unwrap().action, None);

        // A 401 without a challenge unemupc can answer offers no login.
        let other = MockRegistry::start(open_registry());
        let setup = Setup::with(&other);
        let mut remotes = setup.remotes();
        other.fail_requests("GET /v2/_catalog", 401);
        let request = remotes.open_node(&other.host());
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&other.host()).unwrap();
        assert!(
            matches!(view.state, State::LoginRequired { .. }),
            "{:?}",
            view.state
        );
        let message = view.message().unwrap();
        assert_eq!(message.action, None, "{}", message.text);
        assert!(message.text.contains("no challenge"), "{}", message.text);
    }

    fn row(name: &str) -> RepositoryRow {
        RepositoryRow {
            name: name.into(),
            tags: Vec::new(),
            representative: None,
            read: Vec::new(),
            error: None,
        }
    }

    fn view(entry: &str, repositories: &[&str]) -> RegistryView {
        let config =
            image_registry::RegistryConfig::from_toml(&format!("[[registries]]\n{entry}\n"))
                .unwrap();
        let mut view = RegistryView::new(config.registries[0].clone(), true);
        view.repositories = repositories.iter().map(|name| row(name)).collect();
        view
    }

    fn grouped(view: &RegistryView) -> Vec<(Option<String>, bool, Vec<&str>)> {
        view.namespaces()
            .into_iter()
            .map(|n| {
                (
                    n.name,
                    n.private,
                    n.repositories.iter().map(|r| r.name.as_str()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn whole_registry_entries_group_repositories_by_their_first_path_component() {
        let repositories = [
            "unemu/open-at",
            "private/media/monkey-island",
            "games/dune2",
            "private/games/dune2",
            "solo",
        ];
        let whole = view("host = \"nas.example:5000\"\nprivate = true", &repositories);
        assert_eq!(
            grouped(&whole),
            [
                (None, true, vec!["solo"]),
                (Some("games".into()), true, vec!["games/dune2"]),
                (
                    Some("private".into()),
                    true,
                    vec!["private/media/monkey-island", "private/games/dune2"]
                ),
                (Some("unemu".into()), true, vec!["unemu/open-at"]),
            ]
        );
        // An entry that names namespaces shows those, in its order.
        let named = view(
            "host = \"registry.lan.example\"\nnamespaces = [{ name = \"private\", private = true }, { name = \"unemu\" }]",
            &repositories,
        );
        assert_eq!(
            grouped(&named),
            [
                (
                    Some("private".into()),
                    true,
                    vec!["private/media/monkey-island", "private/games/dune2"]
                ),
                (Some("unemu".into()), false, vec!["unemu/open-at"]),
            ]
        );
    }

    fn digest(label: &str) -> Digest {
        Digest::of(label.as_bytes())
    }

    #[test]
    fn local_names_carry_the_host_except_for_the_family_default_registry() {
        assert_eq!(
            local_tag_of(DEFAULT_REGISTRY, "unemu/open-at", "1.2.0"),
            "open-at:1.2.0"
        );
        assert_eq!(
            local_tag_of(DEFAULT_REGISTRY, "unemu/media/freedos", "1.4"),
            "unemu/media/freedos:1.4"
        );
        assert_eq!(
            local_tag_of(
                "nas.example:5000",
                "private/media/monkey-island",
                "1.1-vga-720k"
            ),
            "nas.example:5000/private/media/monkey-island:1.1-vga-720k"
        );
        assert_eq!(
            local_tag_of("registry.lan.example", "private/games/dune2", "1.0.0"),
            "registry.lan.example/private/games/dune2:1.0.0"
        );
    }

    #[test]
    fn local_states_compare_within_one_host_qualified_name() {
        let lan = "registry.lan.example/private/games/monkey-island";
        let nas = "nas.example:5000/private/games/monkey-island";
        let library = vec![
            (format!("{lan}:1.0.0"), digest("lan 1.0.0")),
            (format!("{lan}:1.1.0"), digest("lan 1.1.0")),
            (format!("{lan}:latest"), digest("lan 1.1.0")),
            ("open-at:1.0.0".to_owned(), digest("open-at 1.0.0")),
            ("vgabios-std:1".to_owned(), digest("vga")),
            ("unemu/media/freedos:1.3".to_owned(), digest("freedos 1.3")),
        ];
        let state =
            |tag: &str, image: &str| local_state(tag, &digest(image), mt::MACHINE, &library);
        let set =
            |tag: &str, image: &str| local_state(tag, &digest(image), mt::MEDIA_SET, &library);
        let package =
            |tag: &str, image: &str| local_state(tag, &digest(image), mt::FIRMWARE, &library);
        assert_eq!(
            state(&format!("{lan}:1.1.0"), "lan 1.1.0"),
            LocalState::InLibrary
        );
        assert_eq!(
            state(&format!("{lan}:1.0.0"), "moved"),
            LocalState::OtherImage {
                current: digest("lan 1.0.0")
            }
        );
        assert_eq!(
            state(&format!("{lan}:1.2.0"), "lan 1.2.0"),
            LocalState::NewerAvailable {
                have: format!("{lan}:1.1.0")
            },
            "the newest older version is named; latest makes no claim"
        );
        assert_eq!(
            state(&format!("{lan}:1.0.5"), "lan 1.0.5"),
            LocalState::NotPulled { same_as: None },
            "1.1.0 is newer than this tag"
        );
        // The same path on another registry is another name: no clash, no claim.
        assert_eq!(
            state(&format!("{nas}:1.0.0"), "nas 1.0.0"),
            LocalState::NotPulled { same_as: None }
        );
        assert_eq!(
            state(&format!("{nas}:1.2.0"), "nas 1.2.0"),
            LocalState::NotPulled { same_as: None }
        );
        // The family default registry's short names.
        assert_eq!(
            state("open-at:1.2.0", "open-at 1.2.0"),
            LocalState::NewerAvailable {
                have: "open-at:1.0.0".into()
            }
        );
        // The same image under another local tag.
        assert_eq!(
            package("unemu/firmware/vgabios-std:1", "vga"),
            LocalState::NotPulled {
                same_as: Some("vgabios-std:1".into())
            }
        );
        // Installation media sets compare by the set-tag order, within one
        // edition and format.
        assert_eq!(
            set("unemu/media/freedos:1.4", "freedos 1.4"),
            LocalState::NewerAvailable {
                have: "unemu/media/freedos:1.3".into()
            }
        );
        assert_eq!(
            set("unemu/media/freedos:1.3-r2", "freedos 1.3 r2"),
            LocalState::NewerAvailable {
                have: "unemu/media/freedos:1.3".into()
            }
        );
        assert_eq!(
            set("unemu/media/freedos:1.4-720k", "freedos 1.4 720k"),
            LocalState::NotPulled { same_as: None },
            "another format is a sibling"
        );
        // Firmware packages compare by revision number.
        assert_eq!(
            package("vgabios-std:2", "vga 2"),
            LocalState::NewerAvailable {
                have: "vgabios-std:1".into()
            }
        );
        // A SemVer tag of another type, or of another tool's artifact, makes
        // no claim.
        assert_eq!(
            local_state(
                &format!("{lan}:1.2.0"),
                &digest("x"),
                "application/example",
                &library
            ),
            LocalState::NotPulled { same_as: None }
        );
        assert_eq!(
            package(&format!("{lan}:1.2.0"), "lan 1.2.0"),
            LocalState::NotPulled { same_as: None }
        );
    }

    #[test]
    fn listed_tags_are_compared_with_the_store_and_again_when_the_library_changes() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        let setup = Setup::with(&registry);
        let local = |path: &str| format!("{host}/{path}");
        // In library.
        let pulled = machine("pulled", &[]);
        pulled.seed(&registry, "private/games/pulled", "1.0.0");
        pulled.tag(&setup.store, &local("private/games/pulled:1.0.0"));
        // The tag changed on the registry after the pull.
        let moved = machine("moved", &[]);
        moved.seed(&registry, "private/games/moved", "1.0.0");
        machine("moved before", &[]).tag(&setup.store, &local("private/games/moved:1.0.0"));
        // A newer version than the library's.
        let newer = machine("newer 2", &[]);
        newer.seed(&registry, "private/games/newer", "2.0.0");
        machine("newer 1", &[]).tag(&setup.store, &local("private/games/newer:1.0.0"));
        // The same image under another name.
        let copy = machine("copy", &[]);
        copy.seed(&registry, "private/games/copy", "1.0.0");
        copy.tag(&setup.store, "private/games/copy-made-here:1.0.0");

        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let states = |remotes: &Remotes| -> Vec<(String, LocalState)> {
            remotes
                .registry(&host)
                .unwrap()
                .repositories
                .iter()
                .map(|r| {
                    let row = r.representative_row().unwrap();
                    (r.name.clone(), row.local.clone().unwrap())
                })
                .collect()
        };
        assert_eq!(
            states(&remotes),
            [
                (
                    "private/games/copy".into(),
                    LocalState::NotPulled {
                        same_as: Some("private/games/copy-made-here:1.0.0".into())
                    }
                ),
                (
                    "private/games/moved".into(),
                    LocalState::OtherImage {
                        current: machine("moved before", &[]).digest
                    }
                ),
                (
                    "private/games/newer".into(),
                    LocalState::NewerAvailable {
                        have: local("private/games/newer:1.0.0")
                    }
                ),
                ("private/games/pulled".into(), LocalState::InLibrary),
            ]
        );
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("private/games/newer")
            .unwrap()
            .representative_row()
            .unwrap()
            .clone();
        assert_eq!(row.local_tag, local("private/games/newer:2.0.0"));

        // A pull of the newer version: the states follow the store, and no
        // registry is asked.
        newer.tag(&setup.store, &local("private/games/newer:2.0.0"));
        registry.clear_log();
        let request = remotes.library_changed();
        setup.answer(&mut remotes, request);
        assert!(registry.log().is_empty());
        let now = states(&remotes);
        assert_eq!(
            now[2],
            ("private/games/newer".into(), LocalState::InLibrary)
        );
        assert_eq!(
            now[3],
            ("private/games/pulled".into(), LocalState::InLibrary)
        );
    }

    /// Every file under `root` with its bytes.
    fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_owned()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.insert(path.clone(), Vec::new());
                    pending.push(path);
                } else {
                    files.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        files
    }

    #[test]
    fn browsing_writes_nothing_to_disk() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        let set = media_set("Keen", &["360K"]);
        set.seed(&registry, "games/keen", "1.4");
        let game = machine("pop", &[(TITLE, "Prince of Persia")]);
        game.seed(&registry, "games/pop", "1.0.0");
        game.seed(&registry, "games/pop", "1.1.0");
        let setup = Setup::with(&registry);
        game.tag(&setup.store, &format!("{host}/games/pop:1.0.0"));
        let store_before = files(setup.store.root());
        let config = setup.preferences.parent().unwrap().to_owned();
        let config_before = files(&config);

        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let request = remotes.expand(&host, "games/pop");
        setup.answer(&mut remotes, request);
        let request = remotes.library_changed();
        setup.answer(&mut remotes, request);
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        assert_eq!(remotes.registry(&host).unwrap().state, State::Listed);

        assert_eq!(
            files(setup.store.root()),
            store_before,
            "the store is unchanged"
        );
        assert_eq!(
            files(&config),
            config_before,
            "the preferences are unchanged"
        );
    }

    #[test]
    fn the_library_worker_answers_browsing_on_a_later_frame() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let worker = crate::library_worker::Worker::synthetic(setup.store.root().to_owned());
        let request = remotes.open_node(&host).unwrap();
        worker
            .send
            .send(crate::library_worker::Request::Browse(request))
            .unwrap();
        loop {
            let event = worker
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .expect("worker notification");
            match event {
                crate::library_worker::Event::Browse(event) => {
                    let done = matches!(event, BrowseEvent::Ended { .. });
                    remotes.on_event(event);
                    if done {
                        break;
                    }
                }
                crate::library_worker::Event::Error(error) => panic!("worker: {error}"),
                _ => {}
            }
        }
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["games/a"]);
    }

    #[test]
    fn a_failed_next_page_keeps_the_pages_before_it() {
        let registry = MockRegistry::start(open_registry());
        let repositories: Vec<String> = (0..60).map(|i| format!("games/r{i:02}")).collect();
        registry.seed_repositories(&repositories.iter().map(String::as_str).collect::<Vec<_>>());
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        for status in [500, 401] {
            registry.fail_requests("GET /v2/_catalog", status);
            let request = remotes.next_page(&host);
            setup.answer(&mut remotes, request);
            let view = remotes.registry(&host).unwrap();
            assert!(
                !matches!(view.state, State::Listed | State::Listing),
                "{:?}",
                view.state
            );
            assert_eq!(
                view.repositories.len(),
                CATALOG_PAGE,
                "{status}: page 1 stays"
            );
            assert_eq!(view.message(), None, "the rows are shown");
            let banner = view.banner().unwrap();
            assert!(
                banner.text.contains("The repositories listed before stay.")
                    && !banner.text.to_lowercase().contains("nothing"),
                "{status}: {}",
                banner.text
            );
            // Back to a listed state for the next round.
            registry.stop_failing();
            let request = remotes.refresh(&host);
            setup.answer(&mut remotes, request);
            assert_eq!(remotes.registry(&host).unwrap().state, State::Listed);
        }
    }

    #[test]
    fn an_entry_that_names_namespaces_reads_only_their_repositories() {
        let registry = MockRegistry::start(open_registry());
        for (name, label) in [
            ("games/a", "a"),
            ("private/b", "b"),
            ("private/c", "c"),
            ("unemu/d", "d"),
        ] {
            machine(label, &[]).seed(&registry, name, "1.0.0");
        }
        let host = registry.host();
        let setup = Setup::new(&[format!(
            "{}\nnamespaces = [{{ name = \"private\", private = true }}]",
            plain(&host)
        )]);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        let events = setup.answer(&mut remotes, request);
        let BrowseEvent::Page { repositories, .. } = &events[0] else {
            panic!("{:?}", events[0]);
        };
        assert_eq!(repositories, &["private/b", "private/c"]);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(names(view), ["private/b", "private/c"]);
        assert_eq!(view.expected, 2);
        let log = registry.log();
        assert!(
            !log.iter()
                .any(|l| l.contains("games/a") || l.contains("unemu/d")),
            "{log:?}"
        );
    }

    #[test]
    fn a_library_that_cannot_be_read_is_reported() {
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let Some(BrowseRequest::Local { id, .. }) = remotes.library_changed() else {
            panic!("a local-state request");
        };
        remotes.on_event(BrowseEvent::Local {
            id,
            states: Err("The library could not be read: locked".into()),
        });
        let view = remotes.registry(&host).unwrap();
        assert_eq!(
            view.state,
            State::Failed("The library could not be read: locked".into())
        );
        assert_eq!(names(view), ["games/a"], "the rows stay");
        assert!(view.banner().unwrap().text.contains("locked"));
    }

    #[test]
    fn a_tag_whose_read_failed_is_read_again_when_selected() {
        let registry = MockRegistry::start(open_registry());
        let image = machine("a", &[]);
        image.seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        registry.fail_requests("GET /v2/games/a/manifests/", 500);
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let failed = |remotes: &Remotes| {
            remotes
                .registry(&host)
                .unwrap()
                .repository("games/a")
                .unwrap()
                .row("1.0.0")
                .unwrap()
                .record
                .is_err()
        };
        assert!(failed(&remotes));
        registry.stop_failing();
        let request = remotes.select(&host, "games/a", "1.0.0");
        assert!(request.is_some(), "a failed read is read again");
        setup.answer(&mut remotes, request);
        assert!(!failed(&remotes));
    }

    #[test]
    fn stop_holds_between_tag_pages_and_between_a_manifest_and_its_records() {
        let registry = MockRegistry::start(open_registry());
        let image = machine("many", &[]);
        for i in 0..=TAG_PAGE {
            image.seed(&registry, "games/many", &format!("1.0.{i}"));
        }
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        let late = stop_while_held(
            &setup,
            &mut remotes,
            &registry,
            &host,
            "GET /v2/games/many/tags/list",
            request,
        );
        assert_eq!(ended(&late), &End::Stopped);
        let log = registry.log();
        assert_eq!(
            log.iter()
                .filter(|l| *l == "GET /v2/games/many/tags/list")
                .count(),
            1,
            "no second tag page after Stop: {log:?}"
        );

        // Between a set's manifest and its config record.
        let set = media_set("Keen", &["360K"]);
        let other = MockRegistry::start(open_registry());
        set.seed(&other, "games/keen", "1.4");
        let setup = Setup::with(&other);
        let mut remotes = setup.remotes();
        let host = other.host();
        let request = remotes.open_node(&host);
        stop_while_held(
            &setup,
            &mut remotes,
            &other,
            &host,
            "GET /v2/games/keen/manifests/",
            request,
        );
        assert_eq!(
            blob_requests(&other),
            Vec::<String>::new(),
            "no config after Stop"
        );

        // Between the config record and the metadata record of a selected tag.
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        other.clear_log();
        let request = remotes.select(&host, "games/keen", "1.4");
        let config = format!("GET /v2/games/keen/blobs/{}", Digest::of(&set.config));
        let late = stop_while_held(&setup, &mut remotes, &other, &host, &config, request);
        assert_eq!(ended(&late), &End::Stopped);
        assert_eq!(
            blob_requests(&other),
            [Digest::of(&set.config).to_string()],
            "no metadata after Stop"
        );
    }

    #[test]
    fn stop_all_stops_every_registry() {
        let one = MockRegistry::start(open_registry());
        let two = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&one, "games/a", "1.0.0");
        machine("b", &[]).seed(&two, "games/b", "1.0.0");
        let setup = Setup::new(&[plain(&one.host()), plain(&two.host())]);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&one.host());
        setup.answer(&mut remotes, request);
        let read = remotes.select(&one.host(), "games/a", "1.0.0");
        let list = remotes.open_node(&two.host());
        let flag = |request: &Option<BrowseRequest>| match request {
            Some(BrowseRequest::List { job, .. } | BrowseRequest::Read { job, .. }) => {
                job.cancel.clone()
            }
            _ => panic!("a registry request"),
        };
        let (read_flag, list_flag) = (flag(&read), flag(&list));
        remotes.stop_all();
        assert!(read_flag.load(Ordering::SeqCst) && list_flag.load(Ordering::SeqCst));
        assert_eq!(remotes.registry(&two.host()).unwrap().state, State::Stopped);
        assert_eq!(remotes.registry(&one.host()).unwrap().state, State::Listed);
        // Their answers change nothing now.
        setup.answer(&mut remotes, read);
        setup.answer(&mut remotes, list);
        assert_eq!(remotes.registry(&two.host()).unwrap().repositories.len(), 0);
        let row = remotes
            .registry(&one.host())
            .unwrap()
            .repository("games/a")
            .unwrap();
        assert!(row
            .row("1.0.0")
            .unwrap()
            .record
            .as_ref()
            .unwrap()
            .details
            .is_none());
    }

    #[test]
    fn browsing_is_answered_while_a_machine_runs() {
        use crate::library_worker::{Event, Finish, Request, Worker, WorkspaceKind};
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let fixture = crate::library_test_support::Fixture::new();
        let worker = Worker::synthetic(fixture.store.root().to_owned());
        let wait = |want: &dyn Fn(&Event) -> bool| loop {
            let event = worker
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .expect("worker notification");
            if let Event::Error(error) = &event {
                panic!("worker: {error}");
            }
            if want(&event) {
                return event;
            }
        };
        worker
            .send
            .send(Request::Launch(
                fixture.row.clone(),
                WorkspaceKind::Regular,
                None,
            ))
            .unwrap();
        let Event::Launched(transfer, ..) = wait(&|e| matches!(e, Event::Launched(..))) else {
            unreachable!()
        };
        let session = transfer.restore().unwrap();
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&registry.host()).unwrap();
        worker.send.send(Request::Browse(request)).unwrap();
        loop {
            let Event::Browse(event) = wait(&|e| matches!(e, Event::Browse(_))) else {
                unreachable!()
            };
            let done = matches!(event, BrowseEvent::Ended { .. });
            remotes.on_event(event);
            if done {
                break;
            }
        }
        let view = remotes.registry(&registry.host()).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["games/a"]);
        drop(session);
        worker.send.send(Request::Finish(Finish::Retain)).unwrap();
        wait(&|e| matches!(e, Event::Recoveries(_)));
    }
}
