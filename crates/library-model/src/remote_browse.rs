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
//! ([`image_registry::browse`]). That tag is the newest one this release runs
//! (machine-images §5.6): the version tag `latest` names when it is one of
//! the 16 newest versions and runs here, else the newest version that runs
//! here, versions newest first as [`candidates`] orders them. The listing
//! reads the versions newest first until it reaches that one, so it knows
//! which newer versions need which release; a repository none of whose
//! versions runs here stands for its newest one, which the list dims. The
//! other tags are read when a repository is expanded or a tag selected. Selecting a tag also reads its small metadata
//! record, for the edition and version the details pane shows; no other layer
//! is ever fetched. A registry entry that names namespaces lists only the
//! repositories in them. A stored login goes with the first request, because
//! registries answer an anonymous catalog request with their anonymous view
//! instead of a challenge.
//!
//! Every request runs on the library worker, also while a machine runs, with
//! the short timeouts of a login check (10 s to connect, 15 s per read), and
//! answers on a later frame. Stop sets a flag the worker checks before every
//! request after a job's first, so a listing stops at its next read; answers
//! of a stopped job change nothing. The worker answers one request at a time,
//! so the window calls [`Remotes::stop_all`] before it starts a machine.
//! Refresh lists the registry again and replaces the rows.
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
//! precedence for machine images, and for installation media sets and
//! firmware packages the revision numbers of one release
//! ([`image_store::tag::compare`]); a tag outside its type's order, an
//! alias such as `latest` or `1.4`, a revision of another release or
//! another tool's artifact makes no claim. Tags never order two releases.
//! Across releases of a set or package, a selected tag whose metadata record
//! was read is also *Newer available* when the library holds, under that
//! name, records of the same title, edition, regions, languages and
//! formats, and every one of them has an older `version` field by
//! [`image_store::tag::compare_versions`] ([`ReleaseFacts`]). The hint is
//! conservative: a version a word decides, a version that does not split
//! into segments, or a record under the name that cannot be read makes no
//! claim. It is a hint only: nothing is replaced or pulled. *Not pulled*
//! otherwise, naming another local tag of the same image when there is one.
//! The comparison runs on the worker against the store, and again whenever
//! the library changes. The same comparison notes which blobs of each read
//! tag the store already holds, so the details pane can say what a pull would
//! fetch.
//!
//! What a tag's platform is, whether this program runs it and the names of
//! a set's media formats come from the platforms the caller hands in
//! ([`DesktopPlatform`], [`PlatformFacts`]).
//!
//! Verified manifests, config blobs and selected metadata records are cached by
//! digest under the local image store’s `cache/remote/` directory. Every read
//! verifies the digest again; corrupt entries are discarded. Entries are limited
//! to the existing small-blob limit, with a soft 64 MiB total cap and oldest-first
//! eviction. This leaves room for thousands of ordinary records without an
//! unbounded disk footprint. Payload layers never enter this cache. Immutable
//! tag mappings persist per host, repository and tag; aliases resolve again
//! after restart, and Refresh resolves tags again. Registry or login changes
//! discard the host’s mappings. Content stays local, including private records,
//! and the cache never sends it anywhere.
//!
//! Pull runs in [`crate::remote_pull`]; the desktop's library window draws
//! both.

#[path = "remote_cache.rs"]
pub(crate) mod cache;

use cache::{RemoteCache, Session};

use crate::{
    platform::{DesktopPlatform, PlatformFacts},
    push_destination::ListedRegistry,
    registry_transfer::{common_failure, next_id, Accounts, Landed, Listing},
};
use image_registry::{
    browse::{self, MIN_APP_VERSION},
    client::Challenge,
    config::Scope,
    reference::{is_official_namespace, OFFICIAL_NAMESPACE},
    Client, Error, Reference, RegistryEntry, DEFAULT_REGISTRY,
};
use image_store::{
    compat, media_type as mt, metadata::Metadata, BlobSource, Descriptor, Digest, Manifest, Store,
    MAX_SMALL_BLOB,
};
use std::{
    cmp::Ordering as Order,
    collections::{BTreeMap, BTreeSet},
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
/// At most this many of the newest versions are compared with `latest`.
const LATEST_LOOKUPS: usize = 16;

const TITLE: &str = "org.opencontainers.image.title";
const VERSION: &str = "org.opencontainers.image.version";
const PLATFORM: &str = "unemu.platform";

/// What the list shows of one tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRecord {
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
    pub compatibility_levels: compat::Levels,
    /// `unemu.platform` as the manifest has it: the machine family the
    /// artifact is for.
    pub platform: Option<String>,
    /// The manifest and every distinct blob it names.
    pub size: u64,
    /// Some blob is marked user-supplied.
    pub user_supplied: bool,
    /// Every manifest annotation, for the details pane.
    pub annotations: BTreeMap<String, String>,
    /// What the metadata record adds once the tag is selected, or why it
    /// could not be read; `None` until then.
    pub details: Option<Result<Details, String>>,
    /// The manifest's own bytes.
    pub manifest_size: u64,
    /// Every descriptor the manifest names, the config first, then the
    /// layers in manifest order, with their sizes and marks: what Contents…
    /// lists. Reading them fetches none.
    pub descriptors: Vec<Descriptor>,
    /// A media set's media in set order: each one's id and format name.
    pub media: Vec<(String, String)>,
}

/// The selected tag's title, edition and version from its metadata record;
/// a firmware package has none and keeps its config record's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub title: Option<String>,
    pub edition: Option<String>,
    pub version: Option<String>,
    /// The record's regions and languages, as it lists them.
    pub regions: Vec<String>,
    pub languages: Vec<String>,
}

/// What a person calls an artifact of `artifact_type`.
fn kind_name(artifact_type: &str) -> &'static str {
    match artifact_type {
        mt::MACHINE => "machine image",
        mt::MEDIA_SET => "installation media set",
        mt::FIRMWARE => "firmware package",
        _ => "artifact",
    }
}

impl TagRecord {
    /// Why this release does not read the tag at all: another tool's
    /// artifact, or the artifact of a machine family no compiled-in platform
    /// runs (`facts`). The family is compared through its current
    /// id, so a retired id counts as its platform (machine-images §1,
    /// Platform neutrality).
    pub fn other_platform(&self, facts: &PlatformFacts) -> Option<String> {
        if ![mt::MACHINE, mt::MEDIA_SET, mt::FIRMWARE].contains(&self.artifact_type.as_str()) {
            return Some(format!(
                "Another tool's artifact ({}): this release pulls machine images, installation media sets and firmware packages",
                self.artifact_type
            ));
        }
        let kind = kind_name(&self.artifact_type);
        let family = facts.family();
        let Some(family_id) = self.platform.as_deref() else {
            return Some(format!(
                "A {kind} that names no machine family: this release runs {family}s"
            ));
        };
        if facts.runs(family_id) {
            return None;
        }
        let article = if family_id.starts_with(['a', 'e', 'i', 'o', 'u']) {
            "An"
        } else {
            "A"
        };
        Some(format!(
            "{article} {family_id} {kind}: this release runs {family}s"
        ))
    }

    /// Why this release does not run the tag: [`Self::other_platform`], else
    /// [`Self::needs_newer`]. A tag with a reason is muted, never stands for
    /// its repository while another version runs here, and is not pulled.
    pub fn not_runnable(&self, facts: &PlatformFacts) -> Option<String> {
        self.other_platform(facts).or_else(|| self.needs_newer())
    }

    /// "Needs unemupc X or newer" when this release does not run the tag, by
    /// the `unemu.min-app-version` of its manifest read as the store reads
    /// it: a value that is not a release version is unmet (machine-images
    /// §5.6).
    pub fn needs_newer_with(&self, levels: compat::Levels) -> Option<String> {
        compat::Requirements {
            core: compat::Requirement::of_text(self.min_app_version.as_deref()),
            frontend: compat::Requirement::of_text(
                self.annotations
                    .get(compat::MIN_FRONTEND_VERSION)
                    .map(String::as_str),
            ),
        }
        .unmet_reason(levels)
    }

    pub fn needs_newer(&self) -> Option<String> {
        self.needs_newer_with(self.compatibility_levels)
    }

    /// What the Newer available hint across releases compares, for a set or
    /// package whose metadata record was read: the record's title, edition
    /// and version, each falling back as the details pane's does to the
    /// manifest's or config record's, its regions and languages, and the
    /// set's formats. `None` for
    /// another type, a record not read yet or that could not be read, or
    /// one without a title or a version.
    pub fn release(&self) -> Option<ReleaseFacts> {
        if !image_store::tag::governs(&self.artifact_type) {
            return None;
        }
        let details = self.details.as_ref()?.as_ref().ok()?;
        let sorted = |list: &[String]| {
            let mut list = list.to_vec();
            list.sort();
            list.dedup();
            list
        };
        Some(ReleaseFacts {
            title: details.title.clone().or_else(|| self.title.clone())?,
            edition: details.edition.clone(),
            version: details.version.clone().or_else(|| self.version.clone())?,
            regions: sorted(&details.regions),
            languages: sorted(&details.languages),
            formats: sorted(&self.formats),
        })
    }

    /// Each blob the manifest names once, the config first.
    pub fn blobs(&self) -> Vec<&Descriptor> {
        let mut seen = BTreeSet::new();
        self.descriptors
            .iter()
            .filter(|d| seen.insert(&d.digest))
            .collect()
    }

    /// The record of a read tag; a media set's format names come from the
    /// first platform.
    fn from_read(
        read: browse::TagRecord,
        platforms: &[&dyn DesktopPlatform],
        levels: compat::Levels,
    ) -> Self {
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
        let mut set = Vec::new();
        if read.artifact_type == mt::MEDIA_SET {
            let media = config
                .as_ref()
                .and_then(|table| table.get("media"))
                .and_then(toml::Value::as_array);
            for medium in media.into_iter().flatten() {
                let name = medium
                    .get("format")
                    .and_then(toml::Value::as_table)
                    .and_then(|format| {
                        platforms
                            .first()
                            .and_then(|platform| platform.medium_format(format))
                    });
                let id = medium.get("id").and_then(toml::Value::as_str);
                if let (Some(id), Some(name)) = (id, name) {
                    set.push((id.to_owned(), name.to_owned()));
                }
                if let Some(name) = name.filter(|name| !formats.iter().any(|f| f == name)) {
                    formats.push(name.to_owned());
                }
            }
        }
        let descriptors: Vec<Descriptor> = std::iter::once(read.config_descriptor)
            .chain(read.layers)
            .collect();
        let mut seen = BTreeSet::new();
        let blobs: u64 = descriptors
            .iter()
            .filter(|d| seen.insert(d.digest.clone()))
            .map(|d| d.size)
            .sum();
        Self {
            manifest_size: read.size.saturating_sub(blobs),
            descriptors,
            media: set,
            title: annotation(TITLE).or_else(|| field("name")),
            version: annotation(VERSION).or_else(|| field("version")),
            formats,
            min_app_version: read.annotations.get(MIN_APP_VERSION).cloned(),
            compatibility_levels: levels,
            platform: annotation(PLATFORM),
            digest: read.digest,
            artifact_type: read.artifact_type,
            size: read.size,
            user_supplied: read.user_supplied,
            annotations: read.annotations,
            details: None,
        }
    }
}

/// The title record of a set or package release, as the Newer available
/// hint across releases compares it ([`TagRecord::release`]). Two are the
/// same software only when title, edition, regions, languages and formats
/// are equal, a field one states and the other does not included; then
/// their versions compare by [`image_store::tag::compare_versions`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseFacts {
    pub title: String,
    pub edition: Option<String>,
    pub version: String,
    /// The record's regions and languages, sorted.
    pub regions: Vec<String>,
    pub languages: Vec<String>,
    /// The set's format names, sorted; none for a package.
    pub formats: Vec<String>,
}

impl ReleaseFacts {
    /// The same title, edition, regions, languages and formats.
    fn same_software(&self, other: &Self) -> bool {
        (
            &self.title,
            &self.edition,
            &self.regions,
            &self.languages,
            &self.formats,
        ) == (
            &other.title,
            &other.edition,
            &other.regions,
            &other.languages,
            &other.formats,
        )
    }
}

/// How a listed tag stands against the library, within the local name a pull
/// of it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalState {
    /// The local tag a pull writes names this image.
    InLibrary,
    /// That local tag does not exist. `same_as` is another local tag of the
    /// same image, so a pull would fetch nothing.
    NotPulled { same_as: Option<String> },
    /// The library holds `have`, an older version under the same name, and
    /// no version as new as this one.
    NewerAvailable { have: String },
    /// The library holds `have` under the same name, a release whose title
    /// record says `version`, older than this tag's record says, and no
    /// record of the same title, edition and formats as new.
    NewerRelease { have: String, version: String },
    /// The local alias of a set's or package's release names `have`, a
    /// higher revision than the newest the registry lists for it.
    LocalNewer { have: String },
    /// The local tag a pull writes names `current`, another image.
    OtherImage { current: Digest },
    /// The local tag a pull writes does not exist, and `tag`, a local tag
    /// of the same image, records a push of this registry tag from here.
    /// Pushing never renames, so the image stays under `tag`.
    PushedFromHere { tag: String },
}

/// One tag of a repository, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRow {
    pub tag: String,
    /// The local tag a pull writes.
    pub local_tag: String,
    /// The record, or why it could not be read.
    pub record: Result<TagRecord, String>,
    /// `None` when the record could not be read.
    pub local: Option<LocalState>,
    /// The blobs of the record the store held when it was last compared;
    /// a pull does not fetch them again.
    pub stored: BTreeSet<Digest>,
    /// For the release alias of a set or package, the highest revision of
    /// that release the registry lists (`newest_listed_revision`).
    pub newest_revision: Option<u64>,
}

/// What a pull of a tag would fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Download {
    /// The manifest and every blob the store does not hold.
    pub bytes: u64,
    /// Blobs the store holds already, of `blobs`.
    pub present: usize,
    pub blobs: usize,
}

impl TagRow {
    /// What a pull would fetch; `None` when it fetches nothing, as the image
    /// is in the library under this name or another.
    pub fn download(&self) -> Option<Download> {
        let record = self.record.as_ref().ok()?;
        if matches!(
            self.local,
            Some(
                LocalState::InLibrary
                    | LocalState::NotPulled { same_as: Some(_) }
                    | LocalState::PushedFromHere { .. }
            )
        ) {
            return None;
        }
        let blobs = record.blobs();
        let missing: u64 = blobs
            .iter()
            .filter(|d| !self.stored.contains(&d.digest))
            .map(|d| d.size)
            .sum();
        Some(Download {
            bytes: record.manifest_size.saturating_add(missing),
            present: blobs
                .iter()
                .filter(|d| self.stored.contains(&d.digest))
                .count(),
            blobs: blobs.len(),
        })
    }
}

/// One repository of a registry's catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryRow {
    pub name: String,
    /// Every tag, as the registry lists them.
    pub tags: Vec<String>,
    /// The tag the row stands for: the newest version this release runs,
    /// as the listing chose it from the [`candidates`].
    pub representative: Option<String>,
    /// The tags read so far.
    pub read: Vec<TagRow>,
    /// Why the tags could not be listed.
    pub error: Option<String>,
    /// Versions newer than the representative that this release cannot
    /// run, newest first, each with the reason, such as "Needs unemupc X or
    /// newer".
    pub newer_needs: Vec<(String, String)>,
}

impl RepositoryRow {
    /// The list label within a namespace; registry-wide lists keep the full path.
    pub fn label(&self, namespace: Option<&str>) -> &str {
        namespace
            .and_then(|name| self.name.strip_prefix(name))
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(&self.name)
    }

    /// The complete reference for details and row hover text.
    pub fn reference(&self, host: &str, tag: Option<&str>) -> String {
        match tag {
            Some(tag) => format!("{host}/{}:{tag}", self.name),
            None => format!("{host}/{}", self.name),
        }
    }

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

/// The tags a repository row may stand for, newest first, whatever order the
/// registry lists them in: the releases by Semantic Versioning 2.0.0 §11
/// precedence; without a release the pre-releases; without either every tag
/// `latest`, as a machine repository's moving tag, then the order of set and
/// package tags ([`image_store::tag::newest_first`]): by release, each
/// release's alias before its revisions, highest revision first, then the
/// tags outside the scheme by name.
pub fn candidates(tags: &[String]) -> Vec<String> {
    let versions = |release: bool| {
        let mut versions: Vec<_> = tags
            .iter()
            .filter_map(|tag| {
                image_store::tag::precedence(tag)
                    .filter(|p| p.is_release() == release)
                    .map(|p| (p, tag.clone()))
            })
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        versions.into_iter().map(|(_, tag)| tag).collect::<Vec<_>>()
    };
    let releases = versions(true);
    if !releases.is_empty() {
        return releases;
    }
    let pre_releases = versions(false);
    if !pre_releases.is_empty() {
        return pre_releases;
    }
    let mut all = tags.to_vec();
    all.sort_by(|a, b| {
        let latest = |tag: &str| tag == image_store::tag::LATEST;
        latest(b)
            .cmp(&latest(a))
            .then_with(|| image_store::tag::newest_first(a, b))
    });
    all
}

/// The tag a repository row stands for before any manifest is read: the
/// first of its [`candidates`].
#[cfg(any(test, feature = "test-support"))]
pub fn representative(tags: &[String]) -> Option<String> {
    candidates(tags).into_iter().next()
}

/// How two tags of one repository compare as versions of `artifact_type`, or
/// `None` when they make no claim: aliases, tags outside the type's order and
/// artifacts of other tools.
pub(crate) fn compare_tags(artifact_type: &str, a: &str, b: &str) -> Option<Order> {
    match artifact_type {
        mt::MACHINE => {
            Some(image_store::tag::precedence(a)?.cmp(&image_store::tag::precedence(b)?))
        }
        mt::MEDIA_SET | mt::FIRMWARE => image_store::tag::compare(a, b),
        _ => None,
    }
}

/// The local tag a pull of `repository:tag` from `host` writes.
pub fn local_tag_of(host: &str, repository: &str, tag: &str) -> String {
    Reference {
        host: host.into(),
        repository: repository.into(),
        tag: Some(tag.into()),
        digest: None,
    }
    .local_tag()
    .unwrap_or_default()
}

/// The highest revision of the release of the alias `tag` among the tags a
/// repository lists; `None` for any other tag, or a release without one.
pub(crate) fn newest_listed_revision(listed: &[String], tag: &str) -> Option<u64> {
    let Some(image_store::tag::Kind::Alias { release }) = image_store::tag::parse(tag) else {
        return None;
    };
    listed
        .iter()
        .filter_map(|other| match image_store::tag::parse(other)? {
            image_store::tag::Kind::Revision {
                release: Some(of),
                number,
            } if of == release => Some(number),
            _ => None,
        })
        .max()
}

/// How the image `digest` of `artifact_type`, which a pull would write as
/// `local_tag`, stands against the library's tags. `newest_revision` is,
/// for a set's or package's release alias, the highest revision of that
/// release the registry lists.
pub(crate) fn local_state(
    local_tag: &str,
    digest: &Digest,
    artifact_type: &str,
    library: &[(String, Digest)],
    newest_revision: Option<u64>,
) -> LocalState {
    let (name, mine) = local_tag.rsplit_once(':').unwrap_or((local_tag, ""));
    if let Some((_, current)) = library.iter().find(|(tag, _)| tag == local_tag) {
        if current == digest {
            return LocalState::InLibrary;
        }
        // The local alias of a set's or package's release names another image
        // than the registry's: it is shown by the revision it names, against
        // the newest revision the registry lists. An alias no local revision
        // names, as an earlier release wrote it, makes no claim.
        if let (true, Some(image_store::tag::Kind::Alias { release })) = (
            image_store::tag::governs(artifact_type),
            image_store::tag::parse(mine),
        ) {
            let revision = |tag: &str| match image_store::tag::parse(tag)? {
                image_store::tag::Kind::Revision {
                    release: Some(of),
                    number,
                } if of == release => Some(number),
                _ => None,
            };
            let have = library
                .iter()
                .filter(|(_, image)| image == current)
                .filter_map(|(tag, _)| {
                    let (other, version) = tag.rsplit_once(':')?;
                    (other == name).then(|| revision(version).map(|n| (n, tag)))?
                })
                .max();
            match (have, newest_revision) {
                (Some((have, tag)), Some(newest)) if newest > have => {
                    return LocalState::NewerAvailable { have: tag.clone() }
                }
                (Some((have, tag)), Some(newest)) if newest < have => {
                    return LocalState::LocalNewer { have: tag.clone() }
                }
                _ => {}
            }
        }
        return LocalState::OtherImage {
            current: current.clone(),
        };
    }
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

/// Newer available across releases: when every record under the local name
/// that is the same software as `remote` ([`ReleaseFacts`]) holds an older
/// version, the one with the newest version, named by its tag. `held` lists
/// the library's records under the name, `None` for one that cannot be read
/// or names no title or version. The hint is conservative: `None`, no claim,
/// when none is the same software, when one is as new or newer, when the
/// versions of one cannot be compared, or when one cannot be read at all.
pub(crate) fn newer_release(
    remote: &ReleaseFacts,
    held: &[(String, Option<ReleaseFacts>)],
) -> Option<LocalState> {
    let mut older = Vec::new();
    for (tag, local) in held {
        let local = local.as_ref()?;
        if !local.same_software(remote) {
            continue;
        }
        match image_store::tag::compare_versions(&remote.version, &local.version)? {
            Order::Greater => older.push((tag, local)),
            _ => return None,
        }
    }
    older
        .into_iter()
        .max_by(|a, b| {
            image_store::tag::compare_versions(&a.1.version, &b.1.version)
                .unwrap_or(Order::Equal)
                .then_with(|| a.0.cmp(b.0))
        })
        .map(|(tag, local)| LocalState::NewerRelease {
            have: tag.clone(),
            version: local.version.clone(),
        })
}

/// `state`, or [`LocalState::NewerRelease`] when the tag a pull writes is
/// not in the library and the records of the library's tags under its name
/// are older releases of `remote` ([`newer_release`]).
fn across_releases(
    store: &Store,
    local_tag: &str,
    artifact_type: &str,
    remote: Option<&ReleaseFacts>,
    library: &[(String, Digest)],
    platforms: &[&dyn DesktopPlatform],
    state: LocalState,
) -> LocalState {
    let (LocalState::NotPulled { .. }, Some(remote)) = (&state, remote) else {
        return state;
    };
    let name = local_tag
        .rsplit_once(':')
        .map_or(local_tag, |(name, _)| name);
    let held: Vec<(String, Option<ReleaseFacts>)> = library
        .iter()
        .filter(|(tag, _)| tag.rsplit_once(':').is_some_and(|(other, _)| other == name))
        .filter_map(
            |(tag, _)| match local_release(store, tag, artifact_type, platforms) {
                Ok(Some(facts)) => Some((tag.clone(), Some(facts))),
                Ok(None) => None,
                Err(()) => Some((tag.clone(), None)),
            },
        )
        .collect();
    newer_release(remote, &held).unwrap_or(state)
}

/// The [`ReleaseFacts`] of the local tag `tag`, read from the store as a
/// selected remote tag is read: its manifest, its config record and its
/// metadata record's first title. `Ok(None)` for an image of another type;
/// `Err` for one whose records cannot be read, or that names no title or
/// version, which keeps the hint from making a claim.
fn local_release(
    store: &Store,
    tag: &str,
    artifact_type: &str,
    platforms: &[&dyn DesktopPlatform],
) -> Result<Option<ReleaseFacts>, ()> {
    let descriptor = store.tag_descriptor(tag).map_err(drop)?;
    let bytes = store
        .read_verified(&descriptor, MAX_SMALL_BLOB)
        .map_err(drop)?;
    let manifest = Manifest::decode_for_summary(&bytes).map_err(drop)?;
    if manifest.artifact_type() != artifact_type {
        return Ok(None);
    }
    let config = match manifest.config.media_type.as_str() {
        mt::MEDIA_SET_CONFIG | mt::FIRMWARE_CONFIG => Some(
            store
                .read_verified(&manifest.config, MAX_SMALL_BLOB)
                .map_err(drop)?,
        ),
        _ => None,
    };
    let mut metadata = manifest
        .layers
        .iter()
        .filter(|layer| layer.media_type == mt::METADATA);
    let title = match (metadata.next(), metadata.next()) {
        (Some(layer), None) => {
            let bytes = store.read_verified(layer, MAX_SMALL_BLOB).map_err(drop)?;
            Metadata::decode_for_display(&bytes, &manifest.layers)
                .map_err(drop)?
                .titles
                .into_iter()
                .next()
        }
        _ => None,
    };
    let mut record = TagRecord::from_read(
        browse::TagRecord {
            digest: descriptor.digest,
            artifact_type: artifact_type.into(),
            annotations: manifest.annotations,
            config_descriptor: manifest.config,
            layers: manifest.layers,
            size: descriptor.size,
            user_supplied: false,
            config,
        },
        platforms,
        store
            .compatibility_with(&crate::platform::validators(platforms))
            .manifest_levels(&bytes),
    );
    record.details = Some(Ok(details_of(title)));
    record.release().map(Some).ok_or(())
}

/// What the details pane shows of a metadata record's first title.
fn details_of(title: Option<image_store::metadata::Title>) -> Details {
    let Some(title) = title else {
        return Details::default();
    };
    Details {
        title: Some(title.title),
        edition: title.edition,
        version: title.version,
        regions: title.regions,
        languages: title.languages,
    }
}

/// One registry job: its id, the registry, where logins come from, and the
/// flag Stop sets.
#[derive(Clone)]
pub struct Job {
    pub id: u64,
    pub host: String,
    pub accounts: Accounts,
    pub cancel: Arc<AtomicBool>,
    cache: Arc<std::sync::Mutex<Session>>,
}

/// Work for the library worker.
pub enum BrowseRequest {
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
        /// Every tag the repository lists, to tell the newest revision of a
        /// release alias's release.
        listed: Vec<String>,
        details: bool,
    },
    /// Each read tag's local state again. No registry is contacted.
    Local { id: u64, tags: Vec<LocalQuery> },
}

/// One read tag to compare with the library again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalQuery {
    /// The local tag a pull writes.
    pub local_tag: String,
    /// The image the tag names, and its artifact type.
    pub digest: Digest,
    pub artifact_type: String,
    /// Its blobs, to see which ones the store holds.
    pub blobs: Vec<Descriptor>,
    /// As [`TagRow::newest_revision`].
    pub newest_revision: Option<u64>,
    /// As [`TagRecord::release`].
    pub release: Option<ReleaseFacts>,
    /// The registry tag, `host/repository:tag`, as a push records it.
    pub reference: String,
}

/// How one read tag stands against the library now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAnswer {
    pub local_tag: String,
    pub digest: Digest,
    pub state: LocalState,
    /// The blobs of the tag the store holds.
    pub stored: BTreeSet<Digest>,
}

/// Why a job ended without finishing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
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
pub enum End {
    Done,
    /// The job saw Stop before its next request.
    Stopped,
    Failed(Failure),
}

/// The worker's answers, in order; `id` names the job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowseEvent {
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
        states: Result<Vec<LocalAnswer>, String>,
    },
}

/// Answer one request on the library worker; `send` gets every event.
/// `platforms` say what a read tag is and whether this program runs it.
pub fn run(
    store: &Store,
    request: BrowseRequest,
    platforms: &[&dyn DesktopPlatform],
    send: &dyn Fn(BrowseEvent),
) {
    let platforms = Platforms {
        list: platforms,
        facts: PlatformFacts::of(platforms),
        pushes: std::cell::OnceCell::new(),
    };
    match request {
        BrowseRequest::List { job, next } => {
            let end = list(store, &job, next, &platforms, send);
            send(BrowseEvent::Ended { id: job.id, end });
        }
        BrowseRequest::Read {
            job,
            repository,
            tags,
            listed,
            details,
        } => {
            let end = read(
                store,
                &job,
                &repository,
                &tags,
                &listed,
                details,
                &platforms,
                send,
            );
            send(BrowseEvent::Ended { id: job.id, end });
        }
        BrowseRequest::Local { id, tags } => send(BrowseEvent::Local {
            id,
            states: library(store).and_then(|library| {
                let pushes = library_pushes(store)?;
                tags.into_iter()
                    .map(|query| {
                        Ok(LocalAnswer {
                            state: pushed_from_here(
                                across_releases(
                                    store,
                                    &query.local_tag,
                                    &query.artifact_type,
                                    query.release.as_ref(),
                                    &library,
                                    platforms.list,
                                    local_state(
                                        &query.local_tag,
                                        &query.digest,
                                        &query.artifact_type,
                                        &library,
                                        query.newest_revision,
                                    ),
                                ),
                                &query.reference,
                                &query.digest,
                                &pushes,
                            ),
                            stored: stored(store, &query.blobs).map_err(|error| {
                                format!("The library could not be read: {error}")
                            })?,
                            local_tag: query.local_tag,
                            digest: query.digest,
                        })
                    })
                    .collect()
            }),
        }),
    }
}

/// The platforms a job reads tags with, and what it reads of them.
struct Platforms<'p> {
    list: &'p [&'p dyn DesktopPlatform],
    facts: PlatformFacts,
    /// The library's push records, read once per job when a tag needs them.
    pushes: std::cell::OnceCell<Result<Pushes, String>>,
}

fn library(store: &Store) -> Result<Vec<(String, Digest)>, String> {
    crate::save_image::local_tags(store)
        .map_err(|error| format!("The library could not be read: {error}"))
}

/// Every local tag that records a push from here, with its image and the
/// references it was pushed to ([`image_store::PUSHED_TO`]).
pub(crate) type Pushes = Vec<(String, Digest, Vec<String>)>;

fn library_pushes(store: &Store) -> Result<Pushes, String> {
    let mut pushes = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store
            .list_tags(after.as_deref(), 1000)
            .map_err(|error| format!("The library could not be read: {error}"))?;
        for tag in page.tags {
            let references: Vec<String> =
                tag.pushed_to().into_iter().map(|p| p.reference).collect();
            if !references.is_empty() {
                pushes.push((tag.name, tag.manifest.digest, references));
            }
        }
        match page.next_after {
            Some(next) => after = Some(next),
            None => return Ok(pushes),
        }
    }
}

/// `state`, or [`LocalState::PushedFromHere`] when the local tag a pull
/// writes is not in the library and a local tag of the same image records
/// a push of `reference`: the image is here, under the tag it was pushed
/// from.
pub(crate) fn pushed_from_here(
    state: LocalState,
    reference: &str,
    digest: &Digest,
    pushes: &[(String, Digest, Vec<String>)],
) -> LocalState {
    if !matches!(state, LocalState::NotPulled { .. }) {
        return state;
    }
    pushes
        .iter()
        .find(|(_, image, references)| image == digest && references.iter().any(|r| r == reference))
        .map_or(state, |(tag, _, _)| LocalState::PushedFromHere {
            tag: tag.clone(),
        })
}

/// The record a listing would read of the local image `descriptor`, from
/// the store: what a push put on the registry, so Remotes shows the tag
/// without a request. `None` when the image cannot be read.
pub fn local_record(
    store: &Store,
    descriptor: &Descriptor,
    platforms: &[&dyn DesktopPlatform],
) -> Option<TagRecord> {
    let bytes = store.read_verified(descriptor, MAX_SMALL_BLOB).ok()?;
    let manifest = Manifest::decode_for_summary(&bytes).ok()?;
    let config = match manifest.config.media_type.as_str() {
        mt::MEDIA_SET_CONFIG | mt::FIRMWARE_CONFIG => {
            Some(store.read_verified(&manifest.config, MAX_SMALL_BLOB).ok()?)
        }
        _ => None,
    };
    let user_supplied = std::iter::once(&manifest.config)
        .chain(&manifest.layers)
        .any(|d| {
            d.annotations
                .get(image_store::license::REDISTRIBUTION)
                .map(String::as_str)
                == Some(image_store::license::USER_SUPPLIED)
        });
    let size = descriptor.size
        + std::iter::once(&manifest.config)
            .chain(&manifest.layers)
            .map(|d| (&d.digest, d.size))
            .collect::<BTreeMap<_, _>>()
            .values()
            .sum::<u64>();
    // The metadata record a selected tag reads comes from the store too.
    let mut metadata = manifest
        .layers
        .iter()
        .filter(|layer| layer.media_type == mt::METADATA);
    let title = match (metadata.next(), metadata.next()) {
        (Some(layer), None) => store
            .read_verified(layer, MAX_SMALL_BLOB)
            .ok()
            .and_then(|bytes| Metadata::decode_for_display(&bytes, &manifest.layers).ok())
            .and_then(|record| record.titles.into_iter().next()),
        _ => None,
    };
    let mut record = TagRecord::from_read(
        browse::TagRecord {
            digest: descriptor.digest.clone(),
            artifact_type: manifest.artifact_type().to_owned(),
            annotations: manifest.annotations.clone(),
            config_descriptor: manifest.config.clone(),
            layers: manifest.layers.clone(),
            size,
            user_supplied,
            config,
        },
        platforms,
        store
            .compatibility_with(&crate::platform::validators(platforms))
            .manifest_levels(&bytes),
    );
    record.details = Some(Ok(details_of(title)));
    Some(record)
}

/// Which of `blobs` the store holds, by the check a pull makes before it
/// fetches anything.
pub fn stored<'a>(
    store: &Store,
    blobs: impl IntoIterator<Item = &'a Descriptor>,
) -> image_store::Result<BTreeSet<Digest>> {
    let mut held = BTreeSet::new();
    for blob in blobs {
        match store.open_blob(&blob.digest, blob.size) {
            Ok(_) => {
                held.insert(blob.digest.clone());
            }
            Err(image_store::Error::MissingBlob { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(held)
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

fn remote_cache(store: &Store, job: &Job, entry: &RegistryEntry) -> RemoteCache {
    let identity = format!("{entry:?} {:?}", job.accounts.env.login_for(&job.host));
    RemoteCache::open(store, &job.host, identity.as_bytes(), job.cache.clone())
}

/// The client of connection tests, with its short timeouts and a stored
/// login sent from the first request, and the registry's entry.
fn connect(job: &Job) -> Result<(Client, RegistryEntry), End> {
    let (config, _) = job
        .accounts
        .settings()
        .map_err(|message| End::Failed(Failure::Local(message)))?;
    let client = image_registry::accounts::registry_probe_client(
        &config,
        &job.host,
        &job.accounts.store,
        &job.accounts.env,
    )
    .map_err(|error| failure(error, None))?;
    client
        .use_credentials_now()
        .map_err(|error| failure(error, Some(&client)))?;
    Ok((client, config.entry_or_public(&job.host)))
}

/// Whether a catalog name is listed under `entry`: an entry that names
/// namespaces lists only the repositories in them. The repositories the
/// live registry test leaves under `unemu/test-` are never listed, nor read.
fn listed_under(entry: &RegistryEntry, repository: &str) -> bool {
    let live_test = repository
        .strip_prefix(OFFICIAL_NAMESPACE)
        .is_some_and(|rest| rest.starts_with("/test-"));
    !live_test
        && (entry.namespaces.is_empty()
            || matches!(entry.scope(repository), Scope::Namespace { .. }))
}

/// One tag's row, with its metadata record when `details`; only an error
/// that ends the job is returned as one.
#[allow(clippy::too_many_arguments)]
fn tag_row(
    client: &Client,
    cache: &RemoteCache,
    store: &Store,
    repository: &str,
    tag: &str,
    listed: &[String],
    library: &[(String, Digest)],
    details: bool,
    platforms: &Platforms<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Result<TagRow, Error> {
    let local_tag = local_tag_of(client.host(), repository, tag);
    let text = |error: &Error| common_failure(error).unwrap_or_else(|| error.to_string());
    let providers: Vec<_> = platforms
        .list
        .iter()
        .flat_map(|platform| platform.validators())
        .collect();
    let runtime = store.compatibility_with(&providers);
    let read = match browse::read_tag_cached(client, repository, tag, cancelled, &runtime, cache) {
        Ok(read) => read,
        Err(error) if ends_job(&error) => return Err(error),
        Err(error) => {
            return Ok(TagRow {
                tag: tag.into(),
                local_tag,
                record: Err(text(&error)),
                local: None,
                stored: BTreeSet::new(),
                newest_revision: None,
            })
        }
    };
    let levels = runtime.levels(
        read.annotations
            .get("unemu.platform")
            .map_or("", String::as_str),
    );
    let requirements = compat::Requirements {
        core: compat::Requirement::of_text(
            read.annotations
                .get(compat::MIN_APP_VERSION)
                .map(String::as_str),
        ),
        frontend: compat::Requirement::of_text(
            read.annotations
                .get(compat::MIN_FRONTEND_VERSION)
                .map(String::as_str),
        ),
    };
    let title = if details && levels.core.is_some() && requirements.unmet_reason(levels).is_none() {
        match browse::read_title_cached(client, repository, &read, cancelled, cache) {
            Ok(title) => Some(Ok(details_of(title))),
            Err(error) if ends_job(&error) => return Err(error),
            Err(error) => Some(Err(text(&error))),
        }
    } else {
        None
    };
    let mut record = TagRecord::from_read(read, platforms.list, levels);
    record.details = title;
    let newest_revision = image_store::tag::governs(&record.artifact_type)
        .then(|| newest_listed_revision(listed, tag))
        .flatten();
    let mut local = across_releases(
        store,
        &local_tag,
        &record.artifact_type,
        record.release().as_ref(),
        library,
        platforms.list,
        local_state(
            &local_tag,
            &record.digest,
            &record.artifact_type,
            library,
            newest_revision,
        ),
    );
    // Only a tag not pulled may have been pushed from here; the library's
    // push records are read once for the job, when a tag first needs them.
    if matches!(local, LocalState::NotPulled { .. }) {
        let pushes = platforms
            .pushes
            .get_or_init(|| library_pushes(store))
            .as_ref()
            .map_err(|e| Error::Store(image_store::Error::Invalid(e.clone())))?;
        local = pushed_from_here(
            local,
            &format!("{}/{repository}:{tag}", client.host()),
            &record.digest,
            pushes,
        );
    }
    let stored = stored(store, &record.descriptors).map_err(Error::Store)?;
    Ok(TagRow {
        tag: tag.into(),
        local_tag,
        record: Ok(record),
        local: Some(local),
        stored,
        newest_revision,
    })
}

/// The version tag `latest` names, when the repository has `latest` and it
/// names one of the [`LATEST_LOOKUPS`] newest `candidates`; compared by the
/// manifest digests the registry answers. A cold immutable version also reads
/// its manifest once so the fresh mapping can persist for the next session.
fn latest_version(
    client: &Client,
    repository: &str,
    tags: &[String],
    candidates: &[String],
    cache: &dyn browse::Cache,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<String>, Error> {
    if !tags.iter().any(|tag| tag == "latest") {
        return Ok(None);
    }
    let Some(latest) = browse::cached_digest(client, repository, "latest", cache, cancelled)?
    else {
        return Ok(None);
    };
    for tag in candidates
        .iter()
        .filter(|tag| *tag != "latest")
        .take(LATEST_LOOKUPS)
    {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        if browse::cached_digest(client, repository, tag, cache, cancelled)?.as_ref()
            == Some(&latest)
        {
            return Ok(Some(tag.clone()));
        }
    }
    Ok(None)
}

/// Read a repository's versions newest first until the one its row stands
/// for: `latest`'s version when it runs here, else the newest version that
/// runs here; a version whose record cannot be read ends the search there.
/// At most the [`LATEST_LOOKUPS`] newest versions are read, so a repository
/// whose 16 newest versions all need a newer release stands for its newest
/// one, dimmed, even if an older one would run. Sets the representative, the
/// rows read and the newer versions' reasons.
#[allow(clippy::too_many_arguments)]
fn choose_representative(
    client: &Client,
    cache: &RemoteCache,
    store: &Store,
    row: &mut RepositoryRow,
    library: &[(String, Digest)],
    platforms: &Platforms<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    let candidates = candidates(&row.tags);
    let latest = match latest_version(client, &row.name, &row.tags, &candidates, cache, cancelled) {
        Ok(latest) => latest,
        Err(error) if ends_job(&error) => return Err(error),
        // A `latest` that cannot be compared makes no claim.
        Err(_) => None,
    };
    let mut runnable: Option<String> = None;
    let mut chosen = None;
    for tag in candidates.iter().take(LATEST_LOOKUPS) {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let read = tag_row(
            client, cache, store, &row.name, tag, &row.tags, library, false, platforms, cancelled,
        )?;
        let runs = read
            .record
            .as_ref()
            .ok()
            .map(|r| r.not_runnable(&platforms.facts).is_none());
        row.read.push(read);
        match runs {
            None => {
                chosen = Some(runnable.clone().unwrap_or_else(|| tag.clone()));
                break;
            }
            Some(true) if latest.as_ref() == Some(tag) => {
                chosen = Some(tag.clone());
                break;
            }
            Some(true) => {
                runnable.get_or_insert_with(|| tag.clone());
            }
            Some(false) => {}
        }
        let latest_ahead = latest
            .as_ref()
            .is_some_and(|latest| !row.read.iter().any(|r| &r.tag == latest));
        if runnable.is_some() && !latest_ahead {
            chosen = runnable.clone();
            break;
        }
    }
    let representative = chosen.or(runnable).or_else(|| candidates.first().cloned());
    row.newer_needs = candidates
        .iter()
        .take_while(|tag| Some(*tag) != representative.as_ref())
        .filter_map(|tag| {
            let needs = row
                .row(tag)?
                .record
                .as_ref()
                .ok()?
                .not_runnable(&platforms.facts)?;
            Some((tag.clone(), needs))
        })
        .collect();
    row.representative = representative;
    Ok(())
}

fn list(
    store: &Store,
    job: &Job,
    next: Option<String>,
    platforms: &Platforms<'_>,
    send: &dyn Fn(BrowseEvent),
) -> End {
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
    let cache = remote_cache(store, job, &entry);
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
            newer_needs: Vec::new(),
        };
        match browse::list_tags(&client, &row.name, TAG_PAGE, &stopped) {
            Ok(tags) => {
                row.tags = tags;
                if let Err(error) = choose_representative(
                    &client, &cache, store, &mut row, &library, platforms, &stopped,
                ) {
                    return failure(error, Some(&client));
                }
            }
            Err(error) if ends_job(&error) => return failure(error, Some(&client)),
            Err(error) => {
                row.error = Some(common_failure(&error).unwrap_or_else(|| error.to_string()))
            }
        }
        send(BrowseEvent::Repository { id: job.id, row });
    }
    End::Done
}

#[allow(clippy::too_many_arguments)]
fn read(
    store: &Store,
    job: &Job,
    repository: &str,
    tags: &[String],
    listed: &[String],
    details: bool,
    platforms: &Platforms<'_>,
    send: &dyn Fn(BrowseEvent),
) -> End {
    let stopped = || job.cancel.load(Ordering::SeqCst);
    let library = match library(store) {
        Ok(library) => library,
        Err(message) => return End::Failed(Failure::Local(message)),
    };
    let (client, entry) = match connect(job) {
        Ok(connected) => connected,
        Err(end) => return end,
    };
    let cache = remote_cache(store, job, &entry);
    for tag in tags {
        if stopped() {
            return End::Stopped;
        }
        match tag_row(
            &client, &cache, store, repository, tag, listed, &library, details, platforms, &stopped,
        ) {
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
pub enum State {
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
pub struct Message {
    pub heading: String,
    pub text: String,
    pub action: Option<Action>,
}

/// The button a message offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Stop,
    Refresh,
    TryAgain,
    /// Opens Tools › Preferences › Registries on the registry's entry.
    LogIn,
}

/// One namespace node under a registry.
#[derive(Debug)]
pub struct Namespace<'a> {
    /// The configured scope path, or a catalog first component; `None` for
    /// top-level repositories in a whole-registry entry.
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
pub struct RegistryView {
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
    cache: Arc<std::sync::Mutex<Session>>,
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
            cache: Arc::default(),
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
                    private: self.entry.scope(&namespace.name).private(),
                    repositories: self
                        .repositories
                        .iter()
                        .filter(|r| matches!(self.entry.scope(&r.name), Scope::Namespace { name, .. } if name == namespace.name))
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
                private: self.entry.private && !name.as_deref().is_some_and(is_official_namespace),
                name,
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
            State::Stopped if read > 0 => Some(Message {
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
            State::Stopped => Some(Message {
                heading: "Listing stopped".into(),
                text: "Stop came before the first repository was read. Refresh lists the registry again.".into(),
                action: Some(Action::Refresh),
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
                    "The registry answered with 401 Unauthorized and {}, which unemu cannot answer. {login_tail}",
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
pub struct Remotes {
    accounts: Accounts,
    pub registries: Vec<RegistryView>,
    /// Why preferences could not be read.
    pub error: Option<String>,
    /// The local-state request whose answer is awaited.
    local: Option<u64>,
    /// Counts every change, so a view can keep what it derives until the
    /// next one.
    revision: u64,
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
            revision: 0,
        };
        remotes.reload();
        remotes
    }

    /// Read the registries from `accounts`' preferences again, as
    /// [`Remotes::reload`] does, and use its logins from now on.
    pub fn reattach(&mut self, accounts: Accounts) {
        self.accounts = accounts;
        for view in &mut self.registries {
            view.cache = Arc::new(std::sync::Mutex::new(Session {
                invalidate: true,
                ..Session::default()
            }));
        }
        self.reload();
    }

    /// The count of changes so far: anything derived from the registries
    /// holds while it stays the same.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Where Pull finds logins: the same preferences and credential store.
    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }

    /// Tests change the registries' fields directly; this marks the change.
    #[cfg(any(test, feature = "test-support"))]
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    /// End every running listing, read and local-state request with
    /// `message`, as when the library worker is gone and no answer comes.
    pub fn fail_running(&mut self, message: &str) {
        let mut ended = Vec::new();
        for view in &self.registries {
            ended.extend(view.listing.iter().map(|job| job.id));
            ended.extend(view.reads.iter().map(|read| read.job.id));
        }
        for id in ended {
            self.on_event(BrowseEvent::Ended {
                id,
                end: End::Failed(Failure::Local(message.into())),
            });
        }
        if let Some(id) = self.local {
            self.on_event(BrowseEvent::Local {
                id,
                states: Err(message.into()),
            });
        }
    }

    /// Read the registries from preferences again; what a registry still
    /// listed has shown stays.
    pub fn reload(&mut self) {
        self.revision += 1;
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
            cache: self
                .registry(host)
                .map(|v| v.cache.clone())
                .unwrap_or_default(),
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
        self.revision += 1;
        if self.registry(host)?.state != State::Idle {
            return None;
        }
        self.start_listing(host, None)
    }

    /// Refresh and Try again: list the registry again. The rows shown stay
    /// aside until the new listing answers, for an unreachable registry.
    pub fn refresh(&mut self, host: &str) -> Option<BrowseRequest> {
        self.revision += 1;
        let view = self.registries.iter_mut().find(|v| v.host == host)?;
        let listing = view.state == State::Listing;
        view.cancel_jobs();
        let invalidate = view
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invalidate;
        view.cache = Arc::new(std::sync::Mutex::new(Session {
            revalidate: true,
            invalidate,
            ..Session::default()
        }));
        let shown = (std::mem::take(&mut view.repositories), view.next.take());
        if !(listing && view.previous.is_some()) {
            view.previous = (!shown.0.is_empty()).then_some(shown);
        }
        self.start_listing(host, None)
    }

    /// Next 50 ›: the catalog's next page, added to the rows.
    pub fn next_page(&mut self, host: &str) -> Option<BrowseRequest> {
        self.revision += 1;
        let view = self.registry(host)?;
        if view.state != State::Listed {
            return None;
        }
        let next = view.next.clone()?;
        self.start_listing(host, Some(next))
    }

    /// Stop: the listing ends at its next read; the rows read so far stay.
    pub fn stop(&mut self, host: &str) {
        self.revision += 1;
        if let Some(view) = self.registries.iter_mut().find(|v| v.host == host) {
            view.stop();
        }
    }

    /// Stop every registry's listing and reads, as the window does before a
    /// machine starts, so the worker is free for it.
    pub fn stop_all(&mut self) {
        self.revision += 1;
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
        let listed = view.repository(repository)?.tags.clone();
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
            listed,
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
        self.revision += 1;
        let tags: Vec<LocalQuery> = self
            .registries
            .iter()
            .flat_map(|view| {
                view.repositories.iter().flat_map(move |repository| {
                    repository
                        .read
                        .iter()
                        .map(move |row| (view, repository, row))
                })
            })
            .filter_map(|(view, repository, row)| {
                let record = row.record.as_ref().ok()?;
                Some(LocalQuery {
                    local_tag: row.local_tag.clone(),
                    digest: record.digest.clone(),
                    artifact_type: record.artifact_type.clone(),
                    blobs: record.blobs().into_iter().cloned().collect(),
                    newest_revision: row.newest_revision,
                    release: record.release(),
                    reference: format!("{}/{}:{}", view.host, repository.name, row.tag),
                })
            })
            .collect();
        if tags.is_empty() {
            return None;
        }
        let id = next_id();
        self.local = Some(id);
        Some(BrowseRequest::Local { id, tags })
    }

    /// What this session has listed, for the checks before a push: every
    /// registry whose listing ended, or whose rows stay from an earlier
    /// one, with its repositories, their tags and the images read.
    pub fn listing(&self) -> Listing {
        let mut listing = Listing::default();
        for view in &self.registries {
            if !(view.state == State::Listed || view.stale()) {
                continue;
            }
            let repositories = view
                .repositories
                .iter()
                .map(|repository| {
                    let tags = repository
                        .tags
                        .iter()
                        .map(|tag| {
                            let digest = repository
                                .row(tag)
                                .and_then(|row| row.record.as_ref().ok())
                                .map(|record| record.digest.clone());
                            (tag.clone(), digest)
                        })
                        .collect();
                    (repository.name.clone(), tags)
                })
                .collect();
            let types = view
                .repositories
                .iter()
                .filter_map(|repository| {
                    let record = repository.representative_row()?.record.as_ref().ok()?;
                    Some((repository.name.clone(), record.artifact_type.clone()))
                })
                .collect();
            listing.registries.insert(
                view.host.clone(),
                ListedRegistry {
                    complete: view.next.is_none(),
                    repositories,
                    types,
                },
            );
        }
        listing
    }

    /// A push published `landed`: the registry's list gains the repository
    /// or the tag from the manifest just pushed, without a request, and the
    /// tag reads as in the library or pushed from here. A registry not
    /// listed in this session shows the tag when it is first listed.
    pub fn landed(&mut self, landed: &Landed) {
        let target = &landed.target;
        let Some(tag) = target.tag.clone() else {
            return;
        };
        let Some(view) = self.registries.iter_mut().find(|v| v.host == target.host) else {
            return;
        };
        if !(view.state == State::Listed || view.stale()) {
            return;
        }
        self.revision += 1;
        let local_tag = local_tag_of(&target.host, &target.repository, &tag);
        let local = landed.local.clone();
        let row = TagRow {
            stored: landed
                .record
                .blobs()
                .into_iter()
                .map(|d| d.digest.clone())
                .collect(),
            tag: tag.clone(),
            local_tag,
            record: Ok(landed.record.clone()),
            local: Some(local),
            newest_revision: None,
        };
        match view.repository_mut(&target.repository) {
            Some(repository) => {
                if !repository.tags.contains(&tag) {
                    repository.tags.push(tag.clone());
                }
                if repository.representative.is_none() {
                    repository.representative = Some(tag.clone());
                }
                repository.upsert(row);
            }
            None => {
                // The catalog lists repositories by name.
                let at = view
                    .repositories
                    .iter()
                    .position(|r| r.name > target.repository)
                    .unwrap_or(view.repositories.len());
                view.repositories.insert(
                    at,
                    RepositoryRow {
                        name: target.repository.clone(),
                        tags: vec![tag.clone()],
                        representative: Some(tag),
                        read: vec![row],
                        error: None,
                        newer_needs: Vec::new(),
                    },
                );
            }
        }
    }

    /// Take one of the worker's answers; answers of stopped or replaced jobs
    /// change nothing.
    pub fn on_event(&mut self, event: BrowseEvent) {
        self.revision += 1;
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
                        let found = states.iter().find(|answer| {
                            answer.local_tag == row.local_tag
                                && row.record.as_ref().is_ok_and(|r| r.digest == answer.digest)
                        });
                        if let Some(answer) = found {
                            row.local = Some(answer.state.clone());
                            row.stored.clone_from(&answer.stored);
                        }
                    }
                }
            }
        }
    }
}

/// The limits and annotation keys the tests of the desktop's Remotes
/// compare with.
#[cfg(feature = "test-support")]
pub mod test_support {
    pub const TAG_PAGE: usize = super::TAG_PAGE;
    pub const LATEST_LOOKUPS: usize = super::LATEST_LOOKUPS;
    pub const TITLE: &str = super::TITLE;
    pub const VERSION: &str = super::VERSION;
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_registry::{
        credential_store::SessionStore,
        credentials::EnvCredentials,
        test_support::{MockRegistry, Policy},
    };
    use std::{path::PathBuf, sync::Mutex};

    /// A store, preferences and the accounts that read them, each in its own
    /// directory.
    struct Setup {
        _temp: image_store::test_support::TempDir,
        store: Store,
        preferences: PathBuf,
        accounts: Accounts,
    }

    impl Setup {
        /// Preferences that list `entries` (TOML `[[registries]]` bodies).
        fn new(entries: &[String]) -> Self {
            let temp = image_store::test_support::TempDir::new("remote-browse");
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
            run(&self.store, request.expect("a request"), &[], &|event| {
                events.lock().unwrap().push(event)
            });
            let events = events.into_inner().unwrap();
            for event in events.clone() {
                remotes.on_event(event);
            }
            events
        }
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

    /// A loopback port nothing listens on.
    fn closed_port() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        drop(listener);
        host
    }

    fn row(name: &str) -> RepositoryRow {
        RepositoryRow {
            name: name.into(),
            tags: Vec::new(),
            representative: None,
            read: Vec::new(),
            error: None,
            newer_needs: Vec::new(),
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

    fn digest(label: &str) -> Digest {
        Digest::of(label.as_bytes())
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

        // A 401 without a challenge unemu can answer offers no login.
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

    #[test]
    fn nested_namespaces_labels_keep_full_references() {
        let row = row("unemu/experimental/media/freedos");
        assert_eq!(row.label(Some("unemu/experimental")), "media/freedos");
        assert_eq!(row.label(Some("unemu")), "experimental/media/freedos");
        assert_eq!(row.label(Some("unemu/exp")), row.name);
        assert_eq!(row.label(None), row.name);
        assert_eq!(
            row.reference("registry.example", Some("1.4-r1")),
            "registry.example/unemu/experimental/media/freedos:1.4-r1"
        );
    }

    #[test]
    fn nested_namespaces_catalog_belongs_to_longest_scope_once() {
        let repositories = [
            "unemu/media/freedos",
            "unemu/experimental/media/freedos-append",
            "unemu/experimental",
            "other/image",
        ];
        for nested in [true, false] {
            let scopes = if nested {
                r#"{ name = "unemu" }, { name = "unemu/experimental" }"#
            } else {
                r#"{ name = "unemu" }"#
            };
            let view = view(
                &format!("host = \"registry.example\"\nnamespaces = [{scopes}]"),
                &repositories,
            );
            assert!(!listed_under(&view.entry, repositories[3]));
            assert!(repositories[..3]
                .iter()
                .all(|name| listed_under(&view.entry, name)));
            let groups = grouped(&view);
            assert_eq!(
                groups
                    .iter()
                    .map(|(_, _, repos)| repos.len())
                    .sum::<usize>(),
                3
            );
            assert_eq!(
                groups[0].2,
                if nested {
                    vec![repositories[0]]
                } else {
                    repositories[..3].to_vec()
                }
            );
            if nested {
                assert_eq!(
                    groups[1],
                    (
                        Some("unemu/experimental".into()),
                        false,
                        repositories[1..3].to_vec()
                    )
                );
            }
        }
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
                (Some("unemu".into()), false, vec!["unemu/open-at"]),
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

    #[test]
    fn local_names_carry_the_host_except_for_the_family_default_registry() {
        assert_eq!(
            local_tag_of(DEFAULT_REGISTRY, "unemu/open-at", "1.2.0"),
            "open-at:1.2.0"
        );
        assert_eq!(
            local_tag_of(DEFAULT_REGISTRY, "unemu/media/ibm-pc/freedos", "1.4-r1"),
            "unemu/media/ibm-pc/freedos:1.4-r1"
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
        let freedos = "unemu/media/ibm-pc/freedos";
        let contra = "nas.example:5000/private/media/nes/contra";
        let library = vec![
            (format!("{lan}:1.0.0"), digest("lan 1.0.0")),
            (format!("{lan}:1.1.0"), digest("lan 1.1.0")),
            (format!("{lan}:latest"), digest("lan 1.1.0")),
            ("open-at:1.0.0".to_owned(), digest("open-at 1.0.0")),
            ("vgabios-std:r1".to_owned(), digest("vga")),
            ("unemu/media/freedos:1.3".to_owned(), digest("freedos 1.3")),
            (format!("{freedos}:1.4-r1"), digest("freedos 1.4 r1")),
            (format!("{freedos}:1.4"), digest("freedos 1.4 r1")),
            (format!("{contra}:usa-rev1-r1"), digest("contra usa rev1")),
            (format!("{contra}:usa-rev1"), digest("contra usa rev1")),
            (format!("{contra}:japan-r1"), digest("contra japan r1")),
            (format!("{contra}:japan-r2"), digest("contra japan r2")),
            (format!("{contra}:japan"), digest("contra japan r2")),
        ];
        let state =
            |tag: &str, image: &str| local_state(tag, &digest(image), mt::MACHINE, &library, None);
        let set = |tag: &str, image: &str| {
            local_state(tag, &digest(image), mt::MEDIA_SET, &library, None)
        };
        // A release alias as the registry lists it with its newest revision.
        let alias = |tag: &str, image: &str, newest: u64| {
            local_state(tag, &digest(image), mt::MEDIA_SET, &library, Some(newest))
        };
        let package =
            |tag: &str, image: &str| local_state(tag, &digest(image), mt::FIRMWARE, &library, None);
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
            package("unemu/firmware/ibm-pc/vgabios-std:r1", "vga"),
            LocalState::NotPulled {
                same_as: Some("vgabios-std:r1".into())
            }
        );
        // Sets and packages compare the revisions of one release.
        assert_eq!(
            set(&format!("{freedos}:1.4-r2"), "freedos 1.4 r2"),
            LocalState::NewerAvailable {
                have: format!("{freedos}:1.4-r1")
            }
        );
        assert_eq!(
            set(&format!("{contra}:usa-rev1-r2"), "contra usa rev1 r2"),
            LocalState::NewerAvailable {
                have: format!("{contra}:usa-rev1-r1")
            }
        );
        assert_eq!(
            package("vgabios-std:r2", "vga 2"),
            LocalState::NewerAvailable {
                have: "vgabios-std:r1".into()
            }
        );
        // The alias of a release stands for its newest revision. Behind the
        // registry's newest revision, it names the revision the library has;
        // ahead of it, the library's is newer; level, or without the
        // registry's list, it names another image.
        assert_eq!(
            alias(&format!("{freedos}:1.4"), "freedos 1.4 r2", 2),
            LocalState::NewerAvailable {
                have: format!("{freedos}:1.4-r1")
            }
        );
        assert_eq!(
            alias(&format!("{contra}:japan"), "contra japan r1", 1),
            LocalState::LocalNewer {
                have: format!("{contra}:japan-r2")
            },
            "the registry's alias still names r1"
        );
        for newest in [Some(1), None] {
            assert_eq!(
                local_state(
                    &format!("{freedos}:1.4"),
                    &digest("freedos 1.4 rebuilt"),
                    mt::MEDIA_SET,
                    &library,
                    newest
                ),
                LocalState::OtherImage {
                    current: digest("freedos 1.4 r1")
                }
            );
        }
        assert_eq!(
            alias(&format!("{freedos}:1.4"), "freedos 1.4 r1", 1),
            LocalState::InLibrary
        );
        // An alias an earlier release wrote names no revision: no claim.
        assert_eq!(
            alias("unemu/media/freedos:1.3", "freedos 1.3 r2", 2),
            LocalState::OtherImage {
                current: digest("freedos 1.3")
            }
        );
        // Another release or format, a region without the cartridge
        // revision, or another chip layout is other software: no claim.
        for (tag, image) in [
            (format!("{freedos}:1.5-r1"), "freedos 1.5"),
            (format!("{freedos}:1.4-720k-r2"), "freedos 1.4 720k"),
            (format!("{contra}:usa-r2"), "contra usa"),
        ] {
            assert_eq!(
                set(&tag, image),
                LocalState::NotPulled { same_as: None },
                "{tag}"
            );
        }
        assert_eq!(
            package("vgabios-std:2x32k-r1", "vga split"),
            LocalState::NotPulled { same_as: None }
        );
        // A revision tag at another image is a clash, and a tag an earlier
        // release wrote still reads.
        assert_eq!(
            set(&format!("{freedos}:1.4-r1"), "other"),
            LocalState::OtherImage {
                current: digest("freedos 1.4 r1")
            }
        );
        assert_eq!(
            set("unemu/media/freedos:1.3", "freedos 1.3"),
            LocalState::InLibrary
        );
        // A SemVer tag of another type, or of another tool's artifact, makes
        // no claim.
        assert_eq!(
            local_state(
                &format!("{lan}:1.2.0"),
                &digest("x"),
                "application/example",
                &library,
                None
            ),
            LocalState::NotPulled { same_as: None }
        );
        assert_eq!(
            package(&format!("{lan}:1.2.0"), "lan 1.2.0"),
            LocalState::NotPulled { same_as: None }
        );
    }

    #[test]
    fn a_newer_release_needs_the_same_software_and_an_older_version_everywhere() {
        let facts =
            |title: &str, edition: Option<&str>, version: &str, formats: &[&str]| ReleaseFacts {
                title: title.into(),
                edition: edition.map(str::to_owned),
                version: version.into(),
                regions: Vec::new(),
                languages: Vec::new(),
                formats: formats.iter().map(|f| (*f).to_owned()).collect(),
            };
        let floppy = |version: &str| facts("FreeDOS", Some("Floppy"), version, &["1.44M"]);
        let held = |list: &[(&str, ReleaseFacts)]| -> Vec<(String, Option<ReleaseFacts>)> {
            list.iter()
                .map(|(tag, facts)| (format!("freedos:{tag}"), Some(facts.clone())))
                .collect()
        };
        let remote = floppy("1.4");
        assert_eq!(
            newer_release(&remote, &held(&[("1.3-r1", floppy("1.3"))])),
            Some(LocalState::NewerRelease {
                have: "freedos:1.3-r1".into(),
                version: "1.3".into()
            })
        );
        // The newest older release is named; another software is left out.
        assert_eq!(
            newer_release(
                &floppy("95"),
                &held(&[
                    ("1.4-r1", floppy("1.4")),
                    ("3.11-r1", floppy("3.11")),
                    ("live-r1", facts("FreeDOS", Some("Live"), "99", &["1.44M"])),
                ])
            ),
            Some(LocalState::NewerRelease {
                have: "freedos:3.11-r1".into(),
                version: "3.11".into()
            })
        );
        let mut usa = floppy("1.1");
        usa.regions = vec!["usa".into()];
        let mut japan = floppy("1.0");
        japan.regions = vec!["japan".into()];
        let mut english = floppy("1.3");
        english.languages = vec!["en".into()];
        for (why, remote, local) in [
            (
                "another edition",
                remote.clone(),
                facts("FreeDOS", Some("Live"), "1.3", &["1.44M"]),
            ),
            (
                "no edition",
                remote.clone(),
                facts("FreeDOS", None, "1.3", &["1.44M"]),
            ),
            (
                "another format",
                remote.clone(),
                facts("FreeDOS", Some("Floppy"), "1.3", &["720K"]),
            ),
            (
                "another title",
                remote.clone(),
                facts("OpenDOS", Some("Floppy"), "1.3", &["1.44M"]),
            ),
            ("another region", usa.clone(), japan.clone()),
            ("a region on one side only", usa.clone(), floppy("1.0")),
            (
                "a language on one side only",
                remote.clone(),
                english.clone(),
            ),
            ("the same version", remote.clone(), floppy("1.4")),
            (
                "the same version with a trailing zero",
                remote.clone(),
                floppy("1.4.0"),
            ),
            ("a newer version", remote.clone(), floppy("1.5")),
            ("a version of words", remote.clone(), floppy("1.3 beta")),
            ("a word decides", floppy("1.4_beta"), floppy("1.4_alpha")),
            (
                "a mixed segment decides",
                floppy("4.00.950b"),
                floppy("4.00.950"),
            ),
            (
                "a remote version that does not split",
                floppy("1.4 final"),
                floppy("1.3"),
            ),
        ] {
            assert_eq!(
                newer_release(&remote, &held(&[("x-r1", local)])),
                None,
                "{why}"
            );
        }
        // The same region and language compare.
        let mut japan_new = japan.clone();
        japan_new.version = "1.1".into();
        assert!(newer_release(&japan_new, &held(&[("japan-r1", japan)])).is_some());
        // One record as new as the registry's makes no claim.
        assert_eq!(
            newer_release(
                &remote,
                &held(&[("1.3-r1", floppy("1.3")), ("1.4-r9", floppy("1.4"))])
            ),
            None
        );
        // A record of the same software whose version cannot be compared
        // vetoes the hint, and so does a record that cannot be read.
        assert_eq!(
            newer_release(
                &remote,
                &held(&[("1.3-r1", floppy("1.3")), ("1.5-r1", floppy("1.5 beta"))])
            ),
            None
        );
        let mut unread = held(&[("1.3-r1", floppy("1.3"))]);
        unread.push(("freedos:broken-r1".into(), None));
        assert_eq!(newer_release(&remote, &unread), None);
        assert_eq!(newer_release(&remote, &[]), None);
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
}

#[cfg(test)]
mod compatibility_domain_tests {
    use super::*;
    #[test]
    fn remotes_disabled_reasons_keep_the_component_release_domains_separate() {
        use compat::{AppVersion as V, Levels};
        let v = |patch| V::new(0, 0, patch);
        let mut row = TagRecord {
            digest: Digest::of(b"synthetic"),
            artifact_type: mt::MACHINE.into(),
            title: None,
            version: None,
            formats: Vec::new(),
            min_app_version: Some("0.0.5".into()),
            compatibility_levels: Levels::new(Some((v(3), v(1))), v(9), v(1)),
            platform: Some("synthetic".into()),
            size: 0,
            user_supplied: false,
            annotations: BTreeMap::new(),
            details: None,
            manifest_size: 0,
            descriptors: Vec::new(),
            media: Vec::new(),
        };
        assert_eq!(
            row.needs_newer().as_deref(),
            Some("Needs unemupc 0.0.5 or newer")
        );
        row.min_app_version = None;
        row.annotations
            .insert(compat::MIN_FRONTEND_VERSION.into(), "0.0.5".into());
        assert_eq!(row.needs_newer(), None);
        row.compatibility_levels = Levels::new(Some((v(9), v(1))), v(3), v(1));
        assert_eq!(
            row.needs_newer().as_deref(),
            Some("Needs front end 0.0.5 or newer")
        );
        row.annotations
            .insert(compat::MIN_FRONTEND_VERSION.into(), "soon".into());
        assert_eq!(
            row.needs_newer().as_deref(),
            Some("Needs a newer front end")
        );
    }
}
