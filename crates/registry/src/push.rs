// SPDX-License-Identifier: AGPL-3.0-or-later
//! Push a tagged local image; machine-images §5.1 and OCI Distribution
//! Specification 1.1, "Pushing blobs" and "Pushing manifests".
//!
//! Order: guard (run again on the plan's own target and closure, so a plan's
//! earlier verdict is never trusted), local closure verification, for
//! `unemu/` the declared release against the one the provider's tables
//! compute ([`guard::check_release`]),
//! anonymous-read probe for restricted content, remote tag preflight (which
//! accepts every manifest and index type, so a tag naming another tool's image
//! index is not taken for a new one), `HEAD`
//! per blob with upload of the missing ones, a second `HEAD` of every blob
//! (some registries accept manifests whose blobs are absent), a final tag
//! recheck, then the manifest `PUT`. Blobs up to one chunk use a single `PUT`; larger blobs use chunked
//! `PATCH` sessions whose `Location` is persisted after every accepted chunk, so
//! an interrupted upload resumes at the offset the registry reports. Bytes are
//! streamed from the store and never held whole in memory. The manifest bytes
//! are the store's canonical encoding, which carries no creation timestamp.
//!
//! A caller can cancel a push ([`push_cancellable`]): the check runs before
//! each blob, between the chunks of a large blob and before the manifest
//! `PUT`, so a cancelled push never publishes the manifest or moves the tag,
//! and its accepted chunks stay in their persisted session for the next push.

use crate::{
    client::{AnonymousAccess, Chunk, Started},
    config::Scope,
    guard::{self, ClosureEntry, LocalMarks, Refusal},
    http::Body,
    reference::is_version_tag,
    Client, Error, Reference, RegistryEntry, Result,
};
use image_store::{
    BlobSource, ConfigValidator, Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB,
};
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
};

/// Smallest chunk accepted: object-store backends refuse tiny multipart parts.
pub const MIN_CHUNK_SIZE: u64 = 8 * 1024 * 1024;
/// Default chunk size for large blobs.
pub const DEFAULT_CHUNK_SIZE: u64 = 64 * 1024 * 1024;

/// Transfer settings. `allow_move` is the explicit confirmation for moving a
/// non-version tag; version tags never move, nor do the revision tags of
/// installation media sets and firmware packages ([`image_store::tag`]),
/// whether the set or package is the image pushed or the one the tag names
/// now, and an alias of theirs moves only forward, to the newest revision of
/// its release that the registry already has, which the client reads from
/// the registry's tag list and the digest each revision names.
#[derive(Clone, Debug)]
pub struct PushOptions {
    pub chunk_size: u64,
    pub allow_move: bool,
    /// Consecutive failed attempts tolerated per blob before giving up.
    pub max_retries: u32,
    /// Another repository on the same registry to mount missing blobs from.
    /// Library callers only; the command line does not offer mounts yet.
    pub mount_from: Option<String>,
    /// The digest the user saw the tag name when confirming a move. With
    /// `allow_move`, a tag that names any other digest asks again, so the
    /// confirmation never moves the tag away from an image nobody was shown.
    pub move_from: Option<Digest>,
}

impl Default for PushOptions {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            allow_move: false,
            max_retries: 3,
            mount_from: None,
            move_from: None,
        }
    }
}

/// What a push would send and whether the guard allows it. Building a plan
/// performs no network access, so a dry run is entirely local. The fields are
/// read-only: a plan cannot be pointed at another target after its verdict.
///
/// ```compile_fail
/// fn retarget(plan: &mut image_registry::push::PushPlan, other: image_registry::Reference) {
///     plan.target = other;
/// }
/// ```
#[derive(Clone, Debug)]
pub struct PushPlan {
    pub(crate) target: Reference,
    pub(crate) entry: Option<RegistryEntry>,
    pub(crate) image: Descriptor,
    pub(crate) manifest: Manifest,
    pub(crate) manifest_bytes: Vec<u8>,
    pub(crate) closure: Vec<ClosureEntry>,
    pub(crate) verdict: std::result::Result<(), Refusal>,
}

impl PushPlan {
    pub fn target(&self) -> &Reference {
        &self.target
    }

    pub fn image(&self) -> &Descriptor {
        &self.image
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The config and layers with their effective redistribution classes.
    pub fn closure(&self) -> &[ClosureEntry] {
        &self.closure
    }

    /// The guard's configuration-only verdict; [`push`] decides again.
    pub fn verdict(&self) -> &std::result::Result<(), Refusal> {
        &self.verdict
    }

    /// Where the target falls on its registry entry: the whole registry, a
    /// listed namespace, or none; `None` when the host is not configured.
    pub fn scope(&self) -> Option<Scope> {
        self.entry
            .as_ref()
            .map(|entry| entry.scope(&self.target.repository))
    }
}

/// Per-blob outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobOutcome {
    Present,
    Mounted,
    Uploaded,
}

impl BlobOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Mounted => "mounted",
            Self::Uploaded => "uploaded",
        }
    }
}

/// What happened to the target tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagOutcome {
    Created,
    /// The tag already named this image; nothing was uploaded.
    Unchanged,
    Moved {
        from: Digest,
    },
    /// Pushed by digest without a tag.
    Untagged,
}

/// What a running push reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushProgress<'a> {
    /// The blob is on the registry.
    Blob(&'a Descriptor, BlobOutcome),
    /// The registry accepted a chunk of a large blob, or reported where a
    /// resumed upload stands: `sent` bytes are held.
    Chunk { blob: &'a Descriptor, sent: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushReport {
    pub image: Descriptor,
    pub blobs: Vec<(Descriptor, BlobOutcome)>,
    pub tag: TagOutcome,
}

/// Read the local tag's manifest, classify its closure against every local
/// manifest and apply the configuration-only guard rules.
pub fn plan(
    store: &Store,
    local_tag: &str,
    target: Reference,
    entry: Option<&RegistryEntry>,
) -> Result<PushPlan> {
    plan_with_marks(store, local_tag, target, entry, LocalMarks::scan(store)?)
}

/// [`plan`] with marks the caller has gathered: [`LocalMarks::scan`] plus any
/// restrictions only the frontend knows, such as firmware the platform
/// catalogue lists as proprietary.
pub fn plan_with_marks(
    store: &Store,
    local_tag: &str,
    target: Reference,
    entry: Option<&RegistryEntry>,
    marks: LocalMarks,
) -> Result<PushPlan> {
    if target.tag.is_some() && target.digest.is_some() {
        return Err(Error::Reference(format!(
            "{target} names both a tag and a digest; push to the tag or to the digest"
        )));
    }
    if entry.is_some_and(|e| e.host != target.host) {
        return Err(Error::Config(
            "registry entry and target registry differ".into(),
        ));
    }
    let image = store.tag_descriptor(local_tag)?;
    let manifest_bytes = store.read_verified(&image, MAX_SMALL_BLOB)?;
    // An image a newer release tagged in a shared store is refused with the
    // release it needs, even when this build cannot decode its envelope.
    image_store::compat::Requirement::read(&manifest_bytes).check(local_tag)?;
    let manifest = Manifest::decode_for_summary(&manifest_bytes)?;
    if target.digest.as_ref().is_some_and(|d| d != &image.digest) {
        return Err(Error::Reference(format!(
            "{target} does not name the local image {}",
            image.digest
        )));
    }
    let closure = guard::closure(&manifest, &marks);
    let verdict = guard::check(entry, &target, &closure);
    Ok(PushPlan {
        target,
        entry: entry.cloned(),
        image,
        manifest,
        manifest_bytes,
        closure,
        verdict,
    })
}

/// The push plan of `local_tag` that the command line and the desktop shell
/// both use: every local mark, plus a catalogue restriction for each digest
/// `restricted` names in the tag's manifest (the blobs a platform's firmware
/// catalogue lists as proprietary), built from one reading of the manifest.
/// When the tag moves to another image meanwhile, the plan is refused as a
/// tag conflict, so its marks always belong to the image it plans.
pub fn plan_with_catalogue(
    store: &Store,
    local_tag: &str,
    target: Reference,
    entry: Option<&RegistryEntry>,
    restricted: impl FnOnce(&Manifest) -> image_store::Result<Vec<Digest>>,
) -> Result<PushPlan> {
    let image = store.tag_descriptor(local_tag)?;
    let manifest =
        Manifest::decode_for_summary(&BlobSource::read_verified(store, &image, MAX_SMALL_BLOB)?)?;
    let mut marks = LocalMarks::scan(store)?;
    for digest in restricted(&manifest)? {
        marks.restrict_catalogue(digest);
    }
    let plan = plan_with_marks(store, local_tag, target, entry, marks)?;
    if plan.image().digest != image.digest {
        return Err(image_store::Error::Conflict {
            current: Some(plan.image().digest.clone()),
        }
        .into());
    }
    Ok(plan)
}

/// Push a planned image. The matching platform provider checks the closure
/// locally before anything is sent.
pub fn push(
    store: &Store,
    plan: &PushPlan,
    providers: &[&dyn ConfigValidator],
    client: &Client,
    options: &PushOptions,
    progress: &mut dyn FnMut(&Descriptor, BlobOutcome),
) -> Result<PushReport> {
    let mut report = |event: PushProgress<'_>| {
        if let PushProgress::Blob(descriptor, outcome) = event {
            progress(descriptor, outcome);
        }
    };
    push_cancellable(
        store,
        plan,
        providers,
        client,
        options,
        &mut report,
        &|| false,
    )
}

/// [`push`] with progress per blob and per accepted chunk, ending with
/// [`Error::Cancelled`] once `cancelled` answers true: it is asked before each
/// blob, between the chunks of a large blob and before the manifest is
/// published.
pub fn push_cancellable(
    store: &Store,
    plan: &PushPlan,
    providers: &[&dyn ConfigValidator],
    client: &Client,
    options: &PushOptions,
    progress: &mut dyn FnMut(PushProgress<'_>),
    cancelled: &dyn Fn() -> bool,
) -> Result<PushReport> {
    if plan.target.host != client.host() {
        return Err(Error::Config("client and target registry differ".into()));
    }
    guard::check(plan.entry.as_ref(), &plan.target, &plan.closure)?;
    if options.chunk_size < MIN_CHUNK_SIZE {
        return Err(Error::Config(format!(
            "chunk size must be at least {MIN_CHUNK_SIZE} bytes"
        )));
    }
    let validator = crate::select_provider(providers, &plan.manifest)?;
    store.verify_publication(&plan.image, validator)?;
    if plan.target.namespace() == crate::reference::OFFICIAL_NAMESPACE {
        let config = store.read_verified(&plan.manifest.config, MAX_SMALL_BLOB)?;
        let required = validator.required_release(&plan.manifest, &config)?;
        guard::check_release(&plan.target, &plan.manifest, required)?;
    }
    let repository = plan.target.repository.as_str();
    if guard::needs_private_probe(&plan.closure) {
        let reference = format!("{}/{}", plan.target.host, repository);
        match client.anonymous_access(repository)? {
            AnonymousAccess::Denied => {}
            AnonymousAccess::Readable => {
                return Err(Refusal::AnonymousReadable { reference }.into())
            }
            AnonymousAccess::Inconclusive => {
                return Err(Refusal::AnonymousAccessUnverified { reference }.into())
            }
        }
    }
    let reference = plan.target.manifest_reference();
    let preflight = client.manifest_digest(repository, &reference)?;
    let new = || plan.image.digest.clone();
    // The tag rules of sets and packages apply when either the image pushed
    // or the one the tag names now is one; they replace the version rule of
    // machine images, so a release shaped like a semantic version is an
    // alias like any other.
    let scheme = match (&plan.target.digest, &plan.target.tag, preflight.as_ref()) {
        (None, Some(_), current) if current != Some(&plan.image.digest) => {
            image_store::tag::governs(plan.manifest.artifact_type())
                || match current {
                    Some(current) => names_set_or_package(client, repository, current)?,
                    None => false,
                }
        }
        _ => false,
    };
    if scheme {
        if let Some(tag) = &plan.target.tag {
            let package = plan.manifest.artifact_type() == image_store::media_type::FIRMWARE;
            scheme_rules(
                client,
                repository,
                tag,
                &plan.image.digest,
                preflight.as_ref(),
                package,
                cancelled,
            )?;
        }
    }
    let tag = match (&plan.target.digest, &plan.target.tag, preflight.as_ref()) {
        (_, _, Some(current)) if current == &plan.image.digest => {
            return Ok(PushReport {
                image: plan.image.clone(),
                blobs: Vec::new(),
                tag: TagOutcome::Unchanged,
            })
        }
        (Some(_), _, _) | (None, None, _) => TagOutcome::Untagged,
        (None, Some(_), None) => TagOutcome::Created,
        (None, Some(tag), Some(current)) if is_version_tag(tag) && !scheme => {
            return Err(Error::ImmutableTag {
                tag: tag.clone(),
                current: current.clone(),
                new: new(),
            })
        }
        (None, Some(tag), Some(current))
            if !options.allow_move
                || options
                    .move_from
                    .as_ref()
                    .is_some_and(|from| from != current) =>
        {
            return Err(Error::TagMoveNeedsConfirmation {
                tag: tag.clone(),
                current: current.clone(),
                new: new(),
            })
        }
        (None, Some(_), Some(current)) => TagOutcome::Moved {
            from: current.clone(),
        },
    };
    let mut watch = Watch {
        progress,
        cancelled,
    };
    finish(store, plan, client, options, &mut watch, tag, preflight)
}

/// Whether the registry's manifest `digest` is an installation media set or
/// a firmware package. A manifest this client cannot fetch or read as an OCI
/// image manifest, such as an image index, is neither.
fn names_set_or_package(client: &Client, repository: &str, digest: &Digest) -> Result<bool> {
    Ok(client
        .manifest(repository, &digest.to_string())?
        .and_then(|fetched| Manifest::decode_for_summary(&fetched.bytes).ok())
        .is_some_and(|manifest| image_store::tag::governs(manifest.artifact_type())))
}

/// The tag rules of sets and packages on a registry ([`image_store::tag`]),
/// for a push of `new` to `tag`, which names `current` there now: no
/// `latest`; a revision tag never moves; an alias only as [`check_alias`]
/// allows; no tag outside the scheme. A move the rules allow still needs
/// the usual confirmation. `package` is a firmware package push.
fn scheme_rules(
    client: &Client,
    repository: &str,
    tag: &str,
    new: &Digest,
    current: Option<&Digest>,
    package: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    if tag == image_store::tag::LATEST {
        return Err(Error::NoLatest);
    }
    match image_store::tag::parse(tag) {
        Some(image_store::tag::Kind::Alias { release }) => check_alias(
            client, repository, tag, release, new, current, package, cancelled,
        ),
        Some(image_store::tag::Kind::Revision { .. }) => match current {
            Some(current) => Err(Error::ImmutableRevisionTag {
                tag: tag.into(),
                current: current.clone(),
                new: new.clone(),
            }),
            None => Ok(()),
        },
        None => Err(Error::OutsideTagScheme { tag: tag.into() }),
    }
}

/// An alias of a set or package names the newest revision of its release
/// that the registry has, and moves only forward. The client checks it on
/// the registry itself, never from its own store: it lists the
/// repository's tags (`tags/list`, every page), keeps the revision tags of
/// `release`, and asks for the digest each names (a manifest `HEAD`),
/// highest revision first. `new` must be the image of the highest revision;
/// when the alias exists, the image it names now (`current`) must be the one
/// of a lower revision. An alias at an image no revision of its release
/// names there, as one published before revisions, or a release without
/// revisions there, is refused.
#[allow(clippy::too_many_arguments)]
fn check_alias(
    client: &Client,
    repository: &str,
    tag: &str,
    release: &str,
    new: &Digest,
    current: Option<&Digest>,
    package: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let refuse = |reason: String| Error::AliasNotForward {
        tag: tag.into(),
        reason,
    };
    let revisions = crate::browse::release_revisions(client, repository, release, cancelled)?;
    let Some(newest) = revisions.first() else {
        return Err(refuse(format!(
            "the registry has no revision of release {release}; push one such as {} first",
            image_store::tag::first_revision(release, package)
        )));
    };
    if client.manifest_digest(repository, newest)?.as_ref() != Some(new) {
        return Err(refuse(format!(
            "the newest revision there is {newest}, which names another image; push this image as a new revision first"
        )));
    }
    let Some(current) = current else {
        return Ok(());
    };
    for lower in &revisions[1..] {
        if client.manifest_digest(repository, lower)?.as_ref() == Some(current) {
            return Ok(());
        }
    }
    Err(refuse(format!(
        "it names {current}, which no revision of release {release} names there, as an alias published before revisions; delete {tag} on the registry first"
    )))
}

/// What a running push reports, and asks whether to stop.
pub(crate) struct Watch<'a> {
    pub(crate) progress: &'a mut dyn FnMut(PushProgress<'_>),
    pub(crate) cancelled: &'a dyn Fn() -> bool,
}

impl Watch<'_> {
    fn check(&self) -> Result<()> {
        if (self.cancelled)() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

/// Upload the plan's missing blobs, check that the registry holds every
/// one, recheck the tag against `preflight` and publish the manifest last.
/// Recipe pushes ([`crate::recipe`]) end here too, after their own rules.
pub(crate) fn finish(
    store: &Store,
    plan: &PushPlan,
    client: &Client,
    options: &PushOptions,
    watch: &mut Watch<'_>,
    tag: TagOutcome,
    preflight: Option<Digest>,
) -> Result<PushReport> {
    let repository = plan.target.repository.as_str();
    let sessions = UploadSessions::for_store(store);
    let mut seen = BTreeSet::new();
    let mut blobs = Vec::new();
    for entry in &plan.closure {
        let descriptor = &entry.descriptor;
        if !seen.insert(descriptor.digest.clone()) {
            continue;
        }
        watch.check()?;
        let outcome = if client.blob_exists(repository, descriptor)? {
            BlobOutcome::Present
        } else {
            upload(
                store, client, repository, descriptor, options, &sessions, watch,
            )?
        };
        if outcome != BlobOutcome::Uploaded {
            // An upload interrupted earlier is no longer needed.
            sessions.remove(&SessionKey {
                host: client.host(),
                repository,
                digest: &descriptor.digest,
            });
        }
        (watch.progress)(PushProgress::Blob(descriptor, outcome));
        blobs.push((descriptor.clone(), outcome));
    }
    for (descriptor, _) in &blobs {
        if !client.blob_exists(repository, descriptor)? {
            return Err(Error::Protocol(format!(
                "registry does not hold {} after upload",
                descriptor.digest
            )));
        }
    }
    let reference = plan.target.manifest_reference();
    if plan.target.digest.is_none() {
        let current = client.manifest_digest(repository, &reference)?;
        if current != preflight {
            return Err(Error::RemoteTagChanged {
                tag: reference,
                current,
            });
        }
    }
    watch.check()?;
    client.put_manifest(repository, &reference, &plan.manifest_bytes)?;
    Ok(PushReport {
        image: plan.image.clone(),
        blobs,
        tag,
    })
}

fn upload(
    store: &Store,
    client: &Client,
    repository: &str,
    descriptor: &Descriptor,
    options: &PushOptions,
    sessions: &UploadSessions,
    watch: &mut Watch<'_>,
) -> Result<BlobOutcome> {
    let mut mounted_session = None;
    if let Some(from) = options.mount_from.as_deref().filter(|f| *f != repository) {
        match client.start_upload(repository, Some((&descriptor.digest, from)))? {
            Started::Mounted => return Ok(BlobOutcome::Mounted),
            Started::Session(location) => mounted_session = Some(location),
        }
    }
    let file = store.open_blob(&descriptor.digest, descriptor.size)?;
    if descriptor.size <= options.chunk_size {
        monolithic(
            client,
            repository,
            descriptor,
            &file,
            options,
            mounted_session,
        )?;
    } else {
        let key = SessionKey {
            host: client.host(),
            repository,
            digest: &descriptor.digest,
        };
        chunked(
            client,
            repository,
            descriptor,
            &file,
            options,
            sessions,
            &key,
            mounted_session,
            watch,
        )?;
    }
    Ok(BlobOutcome::Uploaded)
}

fn new_session(client: &Client, repository: &str) -> Result<String> {
    match client.start_upload(repository, None)? {
        Started::Session(location) => Ok(location),
        Started::Mounted => Err(Error::Protocol("unexpected mount".into())),
    }
}

fn retryable(error: &Error) -> bool {
    matches!(error, Error::Transport(_))
        || matches!(error, Error::Status { status, .. } if *status >= 500)
}

fn monolithic(
    client: &Client,
    repository: &str,
    descriptor: &Descriptor,
    file: &std::fs::File,
    options: &PushOptions,
    mut session: Option<String>,
) -> Result<()> {
    let mut failures = 0;
    loop {
        let location = match session.take() {
            Some(location) => location,
            None => new_session(client, repository)?,
        };
        let body = Body::File {
            file,
            offset: 0,
            len: descriptor.size,
        };
        match client.finish_upload(&location, &descriptor.digest, &body) {
            Ok(()) => return Ok(()),
            Err(error) if retryable(&error) && failures < options.max_retries => failures += 1,
            Err(error) => return Err(error),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn chunked(
    client: &Client,
    repository: &str,
    descriptor: &Descriptor,
    file: &std::fs::File,
    options: &PushOptions,
    sessions: &UploadSessions,
    key: &SessionKey<'_>,
    fresh: Option<String>,
    watch: &mut Watch<'_>,
) -> Result<()> {
    let size = descriptor.size;
    let resumed = match fresh {
        Some(location) => Some((location, 0)),
        None => sessions
            .load(key)
            .and_then(|location| match client.upload_offset(&location) {
                Ok(Some(offset)) if offset <= size => Some((location, offset)),
                _ => None,
            }),
    };
    let (mut location, mut offset) = match resumed {
        Some(state) => state,
        None => (new_session(client, repository)?, 0),
    };
    sessions.save(key, &location)?;
    if offset > 0 {
        (watch.progress)(PushProgress::Chunk {
            blob: descriptor,
            sent: offset,
        });
    }
    let mut failures = 0;
    while offset < size {
        // The session stays persisted, so the next push resumes at the offset.
        watch.check()?;
        let len = options.chunk_size.min(size - offset);
        let outcome = client.upload_chunk(&location, file, offset, len)?;
        let (error, restart) = match outcome {
            Chunk::Accepted {
                location: next,
                offset: server,
            } if server > offset && server <= size => {
                location = next;
                offset = server;
                sessions.save(key, &location)?;
                failures = 0;
                (watch.progress)(PushProgress::Chunk {
                    blob: descriptor,
                    sent: offset,
                });
                continue;
            }
            Chunk::Accepted { .. } => (
                Error::Protocol("registry made no upload progress".into()),
                false,
            ),
            Chunk::SessionLost => (
                Error::Protocol("registry lost the upload session".into()),
                true,
            ),
            Chunk::RangeRejected => (
                Error::Protocol("registry rejected the chunk range it reported".into()),
                true,
            ),
            Chunk::Interrupted(error) => (error, false),
        };
        failures += 1;
        if failures > options.max_retries {
            return Err(error);
        }
        let server = if restart {
            None
        } else {
            client.upload_offset(&location)?
        };
        match server {
            Some(server) if server <= size => offset = server,
            _ => {
                location = new_session(client, repository)?;
                offset = 0;
                sessions.save(key, &location)?;
            }
        }
    }
    client.finish_upload(&location, &descriptor.digest, &Body::Empty)?;
    sessions.remove(key);
    Ok(())
}

/// Identifies one blob upload to one repository.
pub struct SessionKey<'a> {
    pub host: &'a str,
    pub repository: &'a str,
    pub digest: &'a Digest,
}

/// Upload-session URLs persisted between attempts and processes. The files
/// hold only the registry's session URL, never credentials, and live in the
/// store's local area, outside image blobs.
pub struct UploadSessions {
    dir: PathBuf,
}

impl UploadSessions {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn for_store(store: &Store) -> Self {
        Self::new(store.root().join("local/registry-uploads"))
    }

    fn path(&self, key: &SessionKey<'_>) -> PathBuf {
        let id = Digest::of(format!("{}\n{}\n{}", key.host, key.repository, key.digest).as_bytes());
        self.dir.join(format!("{}.session", id.hex()))
    }

    /// The persisted session URL, if any. Unreadable records are ignored.
    pub fn load(&self, key: &SessionKey<'_>) -> Option<String> {
        let bytes = std::fs::read(self.path(key)).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let line = text.lines().next()?.trim();
        (!line.is_empty() && line.len() <= 4096).then(|| line.to_owned())
    }

    fn save(&self, key: &SessionKey<'_>, location: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(image_store::Error::from)?;
        let path = self.path(key);
        let temporary = path.with_extension("tmp");
        write_private(&temporary, location).map_err(image_store::Error::from)?;
        std::fs::rename(&temporary, &path).map_err(image_store::Error::from)?;
        Ok(())
    }

    fn remove(&self, key: &SessionKey<'_>) {
        let _ = std::fs::remove_file(self.path(key));
    }

    /// Persisted sessions, for tests and diagnostics.
    pub fn count(&self) -> usize {
        std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "session"))
                    .count()
            })
            .unwrap_or(0)
    }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credentials::Anonymous,
        test_support::MockRegistry,
        tests::{image, open_policy, target, Temp, PROVIDERS},
    };

    /// Only this module can alter a plan. Altering it stands in for a stale or
    /// forged verdict: push decides again from the plan's target and closure.
    #[test]
    fn push_decides_again_instead_of_trusting_the_plan_verdict() {
        let dir = Temp::new();
        let store = dir.store();
        image(&store, "rom:1.0.0", &[(b"vendor rom", None)], None);
        let registry = MockRegistry::start(open_policy());
        let attempt = |plan: &PushPlan, entry: &RegistryEntry| {
            let client = Client::new(entry, Box::new(Anonymous)).unwrap();
            push(
                &store,
                plan,
                &PROVIDERS,
                &client,
                &PushOptions::default(),
                &mut |_, _| {},
            )
        };

        let public = registry.entry(false);
        let mut forged = plan(
            &store,
            "rom:1.0.0",
            target(&registry, "owner/rom:1.0.0"),
            Some(&public),
        )
        .unwrap();
        assert!(matches!(
            forged.verdict(),
            Err(Refusal::RegistryNotPrivate { .. })
        ));
        forged.verdict = Ok(());
        let refused = attempt(&forged, &public);
        assert!(
            matches!(
                refused,
                Err(Error::Refused(Refusal::RegistryNotPrivate { .. }))
            ),
            "{refused:?}"
        );

        let private = registry.entry(true);
        let mut retargeted = plan(
            &store,
            "rom:1.0.0",
            target(&registry, "private/rom:1.0.0"),
            Some(&private),
        )
        .unwrap();
        assert_eq!(retargeted.verdict(), &Ok(()));
        retargeted.target = target(&registry, "unemu/rom:1.0.0");
        let refused = attempt(&retargeted, &private);
        assert!(
            matches!(
                refused,
                Err(Error::Refused(Refusal::OfficialNamespaceNeedsOpen { .. }))
            ),
            "{refused:?}"
        );
        assert!(registry.log().is_empty(), "{:?}", registry.log());
    }
}
