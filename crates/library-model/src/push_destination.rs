// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where a push from the library goes, and the verdict on it before anything
//! is sent; the approved remotes-push mock-up
//! (`backlog/docs/design/mockups/remotes-push.html`, Remotes revision 3)
//! and `backlog/docs/design/machine-images.md` §5.1.
//!
//! **The Remotes rule** preselects the destination of a library item. A
//! local tag that names a registry (`host[:port]/path:tag`) goes to that
//! registry, under its own path and tag. A short tag a pull wrote from the
//! default registry goes back there. Any other short tag, one made here,
//! goes where it was pushed last ([`image_store::PUSHED_TO`]); before its
//! first push nothing is preselected and the push waits for a choice, so a
//! private test image never stands one Enter away from the public
//! namespace. Pushing never renames the local tag, whatever the destination.
//!
//! **The chooser** lists the registries of Tools › Preferences › Registries
//! as the Remotes tree does, the default registry first, each with its
//! namespaces: those its entry names, or for a whole-registry entry the
//! first path components this session's listing found. Choosing a
//! namespace keeps the item's own path and tag under it: the whole local
//! path without repeating the namespace, so `private/test/dos5-bench`
//! stays that under `private/` and `open-at-test` becomes
//! `private/open-at-test`; a path under `unemu/` leaves that namespace
//! behind elsewhere.
//!
//! **The verdict** ([`Rules::evaluate`]) applies the guard's rules to the
//! item's dry run without any I/O, as push applies them again before it
//! sends anything: the registry entry and namespace, the login, every
//! blob's effective redistribution class, and then what the registry holds
//! under the chosen name as this session's Remotes listing shows it ("As
//! listed", [`Listing`]): nothing, this image (nothing to upload), or
//! another image, which a version tag, or a revision tag of a set or
//! package, never takes (the next free tag is suggested) and any other tag
//! takes only after the user confirmed the move with both digests. A
//! registry not listed in this session costs no request either: the push
//! reads the tag when it starts, before any blob goes up, and that read
//! stays the authority. A private namespace needs a login before a push, as
//! it does before a listing or a pull; every push needs one.

use crate::{
    registries_model::{Check, Mark},
    registry_transfer::{refusal_text, short, PlanData},
};
use image_registry::{
    config::{RegistryConfig, Scope},
    guard::{self, ClosureEntry, MarkSource, Redistribution},
    reference::{is_version_tag, OFFICIAL_NAMESPACE},
    Reference, RegistryEntry, DEFAULT_REGISTRY,
};
use image_store::{tag as set_tag, Digest, Store};
use std::collections::{BTreeMap, BTreeSet};

/// "Choose a registry …": the verdict of an item with no destination.
pub const CHOOSE: &str = "Choose a registry and a namespace: a short local tag names no registry.";

/// What this session's Remotes listing says of the registries it listed,
/// for the checks before a push. Reading it sends no request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub registries: BTreeMap<String, ListedRegistry>,
}

/// One listed registry: its repositories with their tags, each with the
/// image it names once read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedRegistry {
    /// Every page of the catalog was read.
    pub complete: bool,
    pub repositories: BTreeMap<String, BTreeMap<String, Option<Digest>>>,
    /// Each repository's artifact type, once the tag it stands for is read.
    pub types: BTreeMap<String, String>,
}

/// A local tag a push can take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImage {
    pub tag: String,
    pub digest: Digest,
    /// "machine image", "media set" or "firmware package".
    pub kind: &'static str,
    /// The registry a pull wrote the tag from ([`image_store::PULL_REGISTRY`]).
    pub pulled_from: Option<String>,
    /// The references pushes from here sent it to, the latest last.
    pub pushed: Vec<String>,
    /// A write in this library made it ([`image_store::MADE_HERE`]).
    pub made_here: bool,
}

impl LocalImage {
    /// The local tag `tag` as its index descriptor records it: its kind,
    /// where it came from and where it was pushed.
    pub fn of(tag: &str, descriptor: &image_store::Descriptor) -> Self {
        let record = image_store::Tag {
            name: tag.into(),
            manifest: descriptor.clone(),
        };
        Self {
            tag: tag.into(),
            digest: descriptor.digest.clone(),
            kind: match descriptor.artifact_type.as_deref() {
                Some(image_store::media_type::MEDIA_SET) => "media set",
                Some(image_store::media_type::FIRMWARE) => "firmware package",
                _ => "machine image",
            },
            pulled_from: record.pulled_from().map(str::to_owned),
            pushed: record
                .pushed_to()
                .into_iter()
                .map(|p| p.reference)
                .collect(),
            made_here: record.made_here(),
        }
    }

    /// Where Push to… sends it by the Remotes rule, in words for its pane.
    pub fn rule_line(&self) -> String {
        match rule_destination(self, None) {
            (Some(target), Rule::Own) => {
                format!("its own registry, {}: the local tag names it", target.host)
            }
            (Some(target), Rule::Default) => format!(
                "the default registry, {}/{OFFICIAL_NAMESPACE}/, where it was pulled from",
                target.host
            ),
            (Some(target), Rule::Last) => format!(
                "{}/{}, where it was pushed last",
                target.host, target.repository
            ),
            _ => "a registry you pick: a short local tag names none".into(),
        }
    }

    /// The artifact type its kind stands for.
    pub fn artifact_type(&self) -> &'static str {
        match self.kind {
            "media set" => image_store::media_type::MEDIA_SET,
            "firmware package" => image_store::media_type::FIRMWARE,
            _ => image_store::media_type::MACHINE,
        }
    }

    /// The kind as a heading names it: "Machine image", "Installation
    /// media", "Firmware package".
    pub fn kind_title(&self) -> &'static str {
        match self.kind {
            "media set" => "Installation media",
            "firmware package" => "Firmware package",
            _ => "Machine image",
        }
    }

    /// The Source cell, as the library lists draw it: where it came from,
    /// then where it was pushed (`local → registry.lan.example`).
    pub fn source_cell(&self) -> String {
        let origin = match (&self.pulled_from, self.made_here) {
            (Some(host), _) => host.clone(),
            (None, true) => "local".into(),
            (None, false) => "—".into(),
        };
        let mut hosts: Vec<&str> = Vec::new();
        for reference in &self.pushed {
            let host = reference
                .split_once('/')
                .map_or(reference.as_str(), |(h, _)| h);
            if Some(host) != self.pulled_from.as_deref() && !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        match hosts.as_slice() {
            [] => origin,
            [one] => format!("{origin} → {one}"),
            several => format!("{origin} → {} registries", several.len()),
        }
    }

    /// The tag's path: its name without a registry host.
    pub fn path(&self) -> &str {
        let name = self
            .tag
            .rsplit_once(':')
            .map_or(self.tag.as_str(), |(n, _)| n);
        match Store::tag_host(&self.tag) {
            Some(host) => name.strip_prefix(host).map_or(name, |p| &p[1..]),
            None => name,
        }
    }

    /// The tag after the colon.
    pub fn version(&self) -> &str {
        self.tag.rsplit_once(':').map_or("latest", |(_, t)| t)
    }
}

/// Why a destination was preselected, as the dialog explains it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// The local tag names the registry.
    Own,
    /// A short name pulled from the default registry goes back there.
    Default,
    /// Where it was pushed last.
    Last,
    /// The user picks: nothing preselects a short tag made here.
    Pick,
    /// Push here…: the place is fixed.
    Here,
}

impl Rule {
    /// The line under the destination of one item.
    pub fn sentence(self, host: &str) -> String {
        match self {
            Self::Own => format!(
                "The local tag names {host}, so its own registry is preselected."
            ),
            Self::Default => format!(
                "A short name pulled from the default registry: {host}/{OFFICIAL_NAMESPACE}/ is preselected."
            ),
            Self::Last => "Preselected: where this tag was pushed last.".into(),
            Self::Pick | Self::Here => {
                "A short local tag names no registry, so nothing is preselected.".into()
            }
        }
    }

    /// The words after an item's destination in a list of several.
    pub fn word(self) -> &'static str {
        match self {
            Self::Own => "its own registry",
            Self::Default => "the default registry, where it came from",
            Self::Last => "where it was pushed last",
            Self::Pick => "picked here",
            Self::Here => "here",
        }
    }
}

/// A registry and one of its namespaces, or the registry itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Place {
    pub host: String,
    pub namespace: Option<String>,
}

impl Place {
    /// `host/namespace/`, or `host/` for the registry itself.
    pub fn prefix(&self) -> String {
        match &self.namespace {
            Some(namespace) => format!("{}/{namespace}/", self.host),
            None => format!("{}/", self.host),
        }
    }
}

/// A local path under a namespace: its own leading namespace is not
/// repeated, and `unemu/` is not carried elsewhere.
pub fn under_namespace(path: &str, namespace: Option<&str>) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    if let Some(namespace) = namespace {
        if parts.first() == Some(&namespace) {
            return path.into();
        }
    }
    if parts.len() > 1 && parts[0] == OFFICIAL_NAMESPACE {
        parts.remove(0);
    }
    match namespace {
        Some(namespace) => format!("{namespace}/{}", parts.join("/")),
        None => parts.join("/"),
    }
}

/// The destination the Remotes rule gives `image`: a tag that names a
/// registry goes there; a short tag pulled from the default registry goes
/// back; any other goes to `pick` when there is one, else where it was
/// pushed last, else nowhere yet.
pub fn rule_destination(image: &LocalImage, pick: Option<&Place>) -> (Option<Reference>, Rule) {
    if let Some(host) = Store::tag_host(&image.tag) {
        return (Reference::parse(&image.tag, host).ok(), Rule::Own);
    }
    if let Some(host) = image.pulled_from.as_deref() {
        return (Reference::parse(&image.tag, host).ok(), Rule::Default);
    }
    if let Some(place) = pick {
        let repository = under_namespace(image.path(), place.namespace.as_deref());
        return (
            Some(Reference {
                host: place.host.clone(),
                repository,
                tag: Some(image.version().into()),
                digest: None,
            }),
            Rule::Pick,
        );
    }
    match image.pushed.last() {
        Some(last) => (Reference::parse(last, DEFAULT_REGISTRY).ok(), Rule::Last),
        None => (None, Rule::Pick),
    }
}

/// Push here…: the item keeps its own path and tag under the place.
pub fn here_destination(image: &LocalImage, place: &Place) -> Reference {
    Reference {
        host: place.host.clone(),
        repository: under_namespace(image.path(), place.namespace.as_deref()),
        tag: Some(image.version().into()),
        digest: None,
    }
}

/// The next patch version of a version tag: `1.0.0` gives `1.0.1`.
fn next_patch(tag: &str) -> Option<String> {
    let release = tag.split(['-', '+']).next()?;
    let parts: Vec<u64> = release
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let [major, minor, patch] = parts.as_slice() else {
        return None;
    };
    Some(format!("{major}.{minor}.{}", patch + 1))
}

/// What the dialog may do with a destination.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Verdict {
    /// The dry run is being read.
    #[default]
    Pending,
    /// The dry run could not be read; the refusal says why.
    Unreadable,
    /// No registry chosen.
    Choose,
    /// Not a destination: a bad name or tag, or preferences that fail.
    Bad,
    /// The guard refuses it; there is no override.
    Refused,
    /// A login is needed first.
    Login,
    /// The registry's version or revision tag names another image.
    TagTaken,
    /// As listed, the registry has this image under this tag already.
    Same,
    /// As listed, the tag names `current`; it moves after confirmation.
    Move {
        current: Digest,
    },
    Ready,
}

impl Verdict {
    /// Push may start: Ready, Same, or Move after its confirmation.
    pub fn pushes(&self) -> bool {
        matches!(self, Self::Ready | Self::Same | Self::Move { .. })
    }

    /// The verdict column of a list of several items; `here` words it for
    /// Push here….
    pub fn cell(&self, here: bool) -> &'static str {
        match self {
            Self::Ready if here => "Can go here",
            Self::Ready => "Ready",
            Self::Same if here => "Already here",
            Self::Same => "Already there",
            Self::Move { .. } => "Moves a tag",
            Self::Login if here => "After the login",
            Self::Login => "Log in first",
            Self::Choose => "Pick a registry",
            Self::Refused => "Refused",
            Self::TagTaken => "Tag taken",
            Self::Pending => "Reading…",
            Self::Unreadable => "Not readable",
            Self::Bad => "Check the name",
        }
    }
}

/// The guard's view of one destination.
#[derive(Clone, Debug, Default)]
pub struct Evaluation {
    pub resolved: Option<Reference>,
    pub checks: Vec<Check>,
    /// The check a Log in… button goes beside.
    pub login_check: Option<usize>,
    /// Why the push cannot go there; `None` when it may.
    pub refusal: Option<String>,
    /// The refusal is the guard's, which has no override; otherwise the
    /// destination is incomplete or malformed, or the dry run is missing.
    pub refused_by_guard: bool,
    pub verdict: Verdict,
    pub blobs: usize,
    pub bytes: u64,
}

impl Evaluation {
    pub fn allowed(&self) -> bool {
        self.refusal.is_none() && self.resolved.is_some()
    }

    /// Why a push of several leaves the item out, as a phrase after
    /// "Skipped:"; `None` for one that goes.
    pub fn skip_reason(&self) -> Option<String> {
        let reference = self
            .resolved
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        Some(match &self.verdict {
            Verdict::Ready => return None,
            Verdict::Same => {
                format!("as listed, {reference} already names this image; nothing to upload")
            }
            Verdict::Move { .. } => {
                "its tag names another image on the registry; push it alone to confirm the move"
                    .into()
            }
            Verdict::Login => format!(
                "not signed in to {}",
                self.resolved.as_ref().map_or("", |r| r.host.as_str())
            ),
            Verdict::Choose => "no registry is picked for the short and local tags".into(),
            _ => self
                .refusal
                .clone()
                .unwrap_or_default()
                .trim_end_matches('.')
                .to_owned(),
        })
    }
}

fn check(mark: Mark, text: String) -> Check {
    Check { mark, text }
}

/// Each blob once, in manifest order: a push sends every digest once.
pub(crate) fn distinct(closure: &[ClosureEntry]) -> Vec<&ClosureEntry> {
    let mut seen = BTreeSet::new();
    closure
        .iter()
        .filter(|entry| seen.insert(entry.descriptor.digest.clone()))
        .collect()
}

/// A node of the destination chooser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChooserNode {
    pub place: Place,
    /// "official, public", "private, whole registry", "open content only".
    pub mark: String,
    /// A private namespace before a login: the lock.
    pub locked: bool,
    /// Choosing the registry's own node picks its only namespace, or the
    /// whole registry; with several namespaces it picks none.
    pub picks: Option<Place>,
}

/// What the verdicts read: the registries list, the host short names
/// resolve to, and this session's listing.
#[derive(Clone, Debug)]
pub struct Rules {
    pub config: RegistryConfig,
    pub default_host: String,
    /// Preferences or the default registry could not be read.
    pub preferences: Option<String>,
    pub listing: Listing,
    /// The environment's login pair, which pushes and pulls use for its
    /// host before the stored one.
    pub env: image_registry::credentials::EnvCredentials,
}

/// "HOST is configured; NAMESPACE/ is marked private", as Push… and Pull…
/// name the registry entry and the namespace a reference falls in.
pub(crate) fn configured_line(host: &str, default_host: &str, scope: &Scope) -> String {
    format!(
        "{host} is configured{}; {scope}",
        if host == default_host {
            " (default registry)"
        } else {
            ""
        }
    )
}

/// A reference the parser refused, explained; a bad tag gets the grammar.
pub(crate) fn parse_problem(
    text: &str,
    default_host: &str,
    error: &image_registry::Error,
) -> String {
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

impl Rules {
    /// The login a push to `host` uses, and where its secret comes from: the
    /// environment pair for its host first, as the client takes it, then
    /// the entry's user with the credential store; `None` without either.
    pub fn login(&self, host: &str) -> Option<(String, &'static str)> {
        if let Some(user) = self.env.login_for(host) {
            return Some((user.to_owned(), "the environment"));
        }
        self.config
            .entry(host)?
            .user
            .map(|user| (user, "the credential store"))
    }

    /// The registries' entries in the chooser's order: the default
    /// registry, the family default registry, then preferences' order.
    pub fn entries(&self) -> Vec<RegistryEntry> {
        let mut hosts = vec![self.default_host.clone()];
        for host in std::iter::once(DEFAULT_REGISTRY)
            .chain(self.config.registries.iter().map(|e| e.host.as_str()))
        {
            if !hosts.iter().any(|h| h == host) {
                hosts.push(host.into());
            }
        }
        hosts
            .iter()
            .map(|host| self.config.entry_or_public(host))
            .collect()
    }

    /// An entry's namespaces, each with whether it is private: those it
    /// names, or for a whole registry the first path components listed in
    /// this session, `unemu/` always on the family default registry.
    pub fn namespaces(&self, entry: &RegistryEntry) -> Vec<(String, bool)> {
        if !entry.namespaces.is_empty() {
            return entry
                .namespaces
                .iter()
                .map(|n| (n.name.clone(), n.private && n.name != OFFICIAL_NAMESPACE))
                .collect();
        }
        let mut names: BTreeSet<String> = self
            .listing
            .registries
            .get(&entry.host)
            .into_iter()
            .flat_map(|listed| listed.repositories.keys())
            .filter_map(|r| r.split_once('/').map(|(first, _)| first.to_owned()))
            .collect();
        if entry.host == DEFAULT_REGISTRY {
            names.insert(OFFICIAL_NAMESPACE.into());
        }
        names
            .into_iter()
            .map(|name| {
                let private = entry.private && name != OFFICIAL_NAMESPACE;
                (name, private)
            })
            .collect()
    }

    /// The chooser's nodes: each registry, then its namespaces.
    pub fn chooser(&self) -> Vec<ChooserNode> {
        let mut nodes = Vec::new();
        for entry in self.entries() {
            let whole = entry.namespaces.is_empty();
            let namespaces = self.namespaces(&entry);
            let signed_in = self.login(&entry.host).is_some();
            let kind = if entry.host == DEFAULT_REGISTRY {
                "official, public"
            } else if entry.private || entry.namespaces.iter().any(|n| n.private) {
                "private"
            } else {
                "public"
            };
            let registry = Place {
                host: entry.host.clone(),
                namespace: None,
            };
            nodes.push(ChooserNode {
                mark: if whole {
                    format!("{kind}, whole registry")
                } else {
                    kind.into()
                },
                locked: false,
                picks: match namespaces.as_slice() {
                    _ if whole => Some(registry.clone()),
                    [(only, _)] => Some(Place {
                        host: entry.host.clone(),
                        namespace: Some(only.clone()),
                    }),
                    _ => None,
                },
                place: registry,
            });
            for (name, private) in namespaces {
                let place = Place {
                    host: entry.host.clone(),
                    namespace: Some(name),
                };
                nodes.push(ChooserNode {
                    mark: if private {
                        "private".into()
                    } else {
                        "open content only".into()
                    },
                    locked: private && !signed_in,
                    picks: Some(place.clone()),
                    place,
                });
            }
        }
        nodes
    }

    /// The place a destination falls in: its registry, and the namespace
    /// its first path component names, when the registry has it.
    pub fn place_of(&self, target: &Reference) -> (Place, String) {
        let entry = self.config.entry_or_public(&target.host);
        let namespaces = self.namespaces(&entry);
        match target.repository.split_once('/') {
            Some((first, rest)) if namespaces.iter().any(|(n, _)| n == first) => (
                Place {
                    host: target.host.clone(),
                    namespace: Some(first.into()),
                },
                rest.into(),
            ),
            _ => (
                Place {
                    host: target.host.clone(),
                    namespace: None,
                },
                target.repository.clone(),
            ),
        }
    }

    /// The first private namespace in the registries list, as the place a
    /// refused image may go instead.
    pub fn private_place(&self) -> Option<Place> {
        self.entries().into_iter().find_map(|entry| {
            if entry.namespaces.is_empty() {
                return entry.private.then(|| Place {
                    host: entry.host.clone(),
                    namespace: None,
                });
            }
            entry
                .namespaces
                .iter()
                .find(|n| n.private && n.name != OFFICIAL_NAMESPACE)
                .map(|n| Place {
                    host: entry.host.clone(),
                    namespace: Some(n.name.clone()),
                })
        })
    }

    /// Resolve `text` and apply the guard's rules and this session's
    /// listing to the dry run `plan`, without any I/O. `error` is why the
    /// dry run could not be read.
    pub fn evaluate(&self, plan: Option<&PlanData>, error: Option<&str>, text: &str) -> Evaluation {
        let mut evaluation = Evaluation::default();
        if let Some(problem) = &self.preferences {
            evaluation.refusal = Some(problem.clone());
            evaluation.verdict = Verdict::Bad;
            return evaluation;
        }
        let Some(plan) = plan else {
            // A dry run that failed says why, never that it is still read.
            match error {
                Some(error) => {
                    evaluation.refusal = Some(error.to_owned());
                    evaluation.verdict = Verdict::Unreadable;
                }
                None => {
                    evaluation.refusal =
                        Some("Reading the image and the marks of every local image…".into());
                }
            }
            return evaluation;
        };
        let blobs = distinct(&plan.closure);
        evaluation.blobs = blobs.len();
        evaluation.bytes = blobs.iter().map(|c| c.descriptor.size).sum();
        let text = text.trim();
        if text.is_empty() {
            evaluation.refusal = Some(CHOOSE.into());
            evaluation.verdict = Verdict::Choose;
            return evaluation;
        }
        let target = match Reference::parse(text, &self.default_host) {
            Ok(target) => target,
            Err(error) => {
                evaluation.refusal = Some(parse_problem(text, &self.default_host, &error));
                evaluation.verdict = Verdict::Bad;
                return evaluation;
            }
        };
        if target.digest.is_some() {
            evaluation.refusal = Some("A push needs a tag, not a digest.".into());
            evaluation.verdict = Verdict::Bad;
            return evaluation;
        }
        let entry = self.config.entry(&target.host);
        let scope = entry.as_ref().map(|e| e.scope(&target.repository));
        let unlisted = matches!(scope, Some(Scope::Unlisted { .. }));
        let private = scope.as_ref().is_some_and(Scope::private);
        let mut login = false;
        match (&entry, &scope) {
            (Some(entry), Some(scope)) => {
                evaluation.checks.push(check(
                    if unlisted { Mark::Bad } else { Mark::Good },
                    configured_line(&entry.host, &self.default_host, scope),
                ));
                match self.login(&entry.host) {
                    Some((user, from)) => evaluation.checks.push(check(
                        Mark::Note,
                        format!("Login {user}; its secret comes from {from}"),
                    )),
                    None if !unlisted => {
                        login = true;
                        evaluation.login_check = Some(evaluation.checks.len());
                        evaluation.checks.push(check(
                            Mark::Bad,
                            format!(
                                "Not signed in to {}: {}",
                                entry.host,
                                if private {
                                    "a private namespace needs a login before a push, as it does before a listing or a pull"
                                } else {
                                    "every push needs a login"
                                }
                            ),
                        ));
                    }
                    None => {}
                }
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
            } else if private {
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
        if let Err(refusal) = guard::check(entry.as_ref(), &target, &plan.closure) {
            evaluation.refusal = Some(refusal_text(&refusal, &plan.closure));
            evaluation.refused_by_guard = true;
            evaluation.verdict = Verdict::Refused;
            evaluation.resolved = Some(target);
            return evaluation;
        }
        let tag = target.tag.clone().unwrap_or_else(|| "latest".into());
        evaluation.verdict = Verdict::Ready;
        if let Some(taken) = self.as_listed(plan, &target, &tag, &mut evaluation) {
            evaluation.refusal = Some(taken);
            evaluation.verdict = Verdict::TagTaken;
            evaluation.resolved = Some(target);
            return evaluation;
        }
        evaluation
            .checks
            .push(check(Mark::Note, tag_note(plan, &tag)));
        if login {
            evaluation.verdict = Verdict::Login;
            evaluation.refusal = Some(format!("Not signed in to {}.", target.host));
        }
        evaluation.resolved = Some(target);
        evaluation
    }

    /// The "As listed" line: what this session's listing says the registry
    /// holds under the target. A version or revision tag that names another
    /// image there returns why it cannot be pushed; a tag that moves or
    /// already names this image sets the verdict.
    fn as_listed(
        &self,
        plan: &PlanData,
        target: &Reference,
        tag: &str,
        evaluation: &mut Evaluation,
    ) -> Option<String> {
        let host = &target.host;
        let repository = &target.repository;
        let unread = |why: &str| {
            check(
                Mark::Note,
                format!("{host} {why}: whether {tag} exists there is read when the push starts, before any blob is sent"),
            )
        };
        let Some(listed) = self.listing.registries.get(host) else {
            evaluation
                .checks
                .push(unread("has not been listed in this session"));
            return None;
        };
        let Some(tags) = listed.repositories.get(repository) else {
            evaluation.checks.push(if listed.complete {
                check(
                    Mark::Note,
                    format!("As listed, {repository} is not on the registry yet: the push creates the repository"),
                )
            } else {
                unread("has been listed only in part in this session")
            });
            return None;
        };
        match tags.get(tag) {
            None => evaluation.checks.push(check(
                Mark::Good,
                format!("As listed, {repository} has no tag {tag} yet"),
            )),
            Some(None) => evaluation.checks.push(check(
                Mark::Note,
                format!("As listed, {repository} has the tag {tag}; the image it names is read when the push starts, before any blob is sent"),
            )),
            Some(Some(current)) if *current == plan.image.digest => {
                evaluation.verdict = Verdict::Same;
                evaluation.checks.push(check(
                    Mark::Good,
                    format!("As listed, {target} already names this image: nothing to upload"),
                ));
            }
            Some(Some(current)) => {
                let other = format!(
                    "As listed, {tag} names another image on the registry ({})",
                    short(current)
                );
                if plan.revisions && set_tag::is_revision(tag) {
                    evaluation.checks.push(check(Mark::Bad, other));
                    return Some(format!(
                        "{tag} names another image on the registry. Revision tags of installation media sets and firmware packages never move, not even with confirmation: push it under the next revision{}.",
                        set_tag::next_revision(tag)
                            .map(|next| format!(", such as {next}"))
                            .unwrap_or_default()
                    ));
                }
                if !plan.revisions && is_version_tag(tag) {
                    evaluation.checks.push(check(Mark::Bad, other));
                    return Some(format!(
                        "{tag} names another image on the registry. Version tags never move: choose another tag{}.",
                        next_patch(tag)
                            .map(|next| format!(", such as {next}"))
                            .unwrap_or_default()
                    ));
                }
                evaluation.verdict = Verdict::Move {
                    current: current.clone(),
                };
                evaluation.checks.push(check(
                    Mark::Note,
                    format!("{other}: pushing moves the tag, after you confirm the move with both digests"),
                ));
            }
        }
        None
    }
}

/// The note on whether the destination tag can move.
fn tag_note(plan: &PlanData, tag: &str) -> String {
    if plan.revisions && set_tag::is_revision(tag) {
        format!(
            "{tag} is a revision tag: once it names an image on the registry it never moves, not even with confirmation; push a changed one under the next revision{}. The manifest and the tag are written after every blob",
            set_tag::next_revision(tag)
                .map(|next| format!(", {next}"))
                .unwrap_or_default()
        )
    } else if plan.revisions && tag == set_tag::LATEST {
        "Installation media sets and firmware packages have no latest tag: push a revision tag such as r1 or 1.0-r1, then its release alias".to_owned()
    } else if plan.revisions && set_tag::parse(tag).is_some() {
        format!(
            "{tag} is the alias of a release: it names the newest revision of {tag} the registry already has, and moves only forward to a newer one, after you confirm the move with both digests. The manifest and the tag are written after every blob"
        )
    } else if plan.revisions {
        format!(
            "{tag} is neither a revision tag ([RELEASE-]rN) nor a release alias: installation media sets and firmware packages take only those, so the push is refused"
        )
    } else if is_version_tag(tag) {
        format!(
            "{tag} is a version tag: once it names an image on the registry it never moves. The manifest and the tag are written after every blob"
        )
    } else {
        format!(
            "{tag} is not a version: if the registry has it for another image, you confirm the move with both digests first. The manifest and the tag are written after every blob"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(tag: &str, pulled: Option<&str>, pushed: &[&str]) -> LocalImage {
        LocalImage {
            tag: tag.into(),
            digest: Digest::of(tag.as_bytes()),
            kind: "machine image",
            pulled_from: pulled.map(Into::into),
            pushed: pushed.iter().map(|p| (*p).to_owned()).collect(),
            made_here: pulled.is_none(),
        }
    }

    /// The Remotes rule: a tag that names a registry goes there, a short
    /// one pulled from the default registry goes back, a short one made
    /// here where it was pushed last, else nowhere until a choice; a pick
    /// of several items wins over the last push only for the short ones.
    #[test]
    fn the_remotes_rule_preselects_by_the_tag_its_origin_and_its_last_push() {
        let rule = |image: &LocalImage, pick: Option<&Place>| {
            let (target, rule) = rule_destination(image, pick);
            (target.map(|t| t.to_string()), rule)
        };
        let lan = Place {
            host: "registry.lan.example".into(),
            namespace: Some("private".into()),
        };
        let rig = image("nas.example:5000/private/test/at-test-rig:0.3.0", None, &[]);
        assert_eq!(
            rule(&rig, Some(&lan)),
            (
                Some("nas.example:5000/private/test/at-test-rig:0.3.0".into()),
                Rule::Own
            )
        );
        let pulled = image("open-at:1.2.0", Some(DEFAULT_REGISTRY), &[]);
        assert_eq!(
            rule(&pulled, None),
            (
                Some(format!("{DEFAULT_REGISTRY}/unemu/open-at:1.2.0")),
                Rule::Default
            )
        );
        let bench = image("private/test/dos5-bench:0.2.0", None, &[]);
        assert_eq!(rule(&bench, None), (None, Rule::Pick));
        assert_eq!(
            rule(&bench, Some(&lan)),
            (
                Some("registry.lan.example/private/test/dos5-bench:0.2.0".into()),
                Rule::Pick
            )
        );
        let pushed = image(
            "private/test/dos5-bench:0.2.0",
            None,
            &[
                "nas.example:5000/private/test/dos5-bench:0.2.0",
                "registry.lan.example/private/test/dos5-bench:0.2.0",
            ],
        );
        assert_eq!(
            rule(&pushed, None),
            (
                Some("registry.lan.example/private/test/dos5-bench:0.2.0".into()),
                Rule::Last
            )
        );
        // The whole local path, without repeating the namespace.
        let open = image("open-at-test:0.1.0", None, &[]);
        assert_eq!(
            here_destination(&open, &lan).to_string(),
            "registry.lan.example/private/open-at-test:0.1.0"
        );
        assert_eq!(
            under_namespace("unemu/media/freedos", Some("private")),
            "private/media/freedos"
        );
        assert_eq!(
            under_namespace("private/test/x", Some("private")),
            "private/test/x"
        );
        assert_eq!(next_patch("1.0.0").as_deref(), Some("1.0.1"));
    }

    fn rules(preferences: &str, env: image_registry::credentials::EnvCredentials) -> Rules {
        let table: toml::Table = preferences.parse().unwrap();
        Rules {
            config: RegistryConfig::from_table(&table).unwrap(),
            default_host: DEFAULT_REGISTRY.into(),
            preferences: None,
            listing: Listing::default(),
            env,
        }
    }

    fn plan() -> PlanData {
        PlanData {
            local_tag: "x:1".into(),
            image: image_store::Descriptor::new(
                image_store::media_type::MANIFEST,
                Digest::of(b"x"),
                1,
            ),
            closure: Vec::new(),
            revisions: false,
        }
    }

    /// A dry run that could not be read says why instead of reading on.
    #[test]
    fn a_failed_dry_run_says_why() {
        let rules = rules("", Default::default());
        let failed = rules.evaluate(None, Some("Needs unemupc 9.0.0 or newer"), "a.example/x:1");
        assert_eq!(failed.verdict, Verdict::Unreadable);
        assert_eq!(
            failed.refusal.as_deref(),
            Some("Needs unemupc 9.0.0 or newer")
        );
        assert_eq!(
            failed.skip_reason().as_deref(),
            Some("Needs unemupc 9.0.0 or newer")
        );
        assert_eq!(
            rules.evaluate(None, None, "a.example/x:1").verdict,
            Verdict::Pending
        );
    }

    /// The environment's login pair for a host counts as a login, as pushes
    /// use it; a registry with neither it nor a stored login waits for one.
    #[test]
    fn the_environment_login_counts_and_none_at_all_waits() {
        let preferences = "[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n";
        let target = "nas.example:5000/private/x:1";
        let none = rules(preferences, Default::default());
        assert_eq!(
            none.evaluate(Some(&plan()), None, target).verdict,
            Verdict::Login
        );
        let env = image_registry::credentials::EnvCredentials::new("nas.example:5000", "ci", "s");
        let signed = rules(preferences, env);
        let evaluation = signed.evaluate(Some(&plan()), None, target);
        assert_eq!(evaluation.verdict, Verdict::Ready);
        assert!(evaluation
            .checks
            .iter()
            .any(|c| c.text == "Login ci; its secret comes from the environment"));
        assert_eq!(next_patch("0.2.0-rc.1").as_deref(), Some("0.2.1"));
    }
}
