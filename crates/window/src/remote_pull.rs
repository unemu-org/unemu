// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless model of Pull… from the library's Remotes, following the
//! approved remotes mock-up, revision 2
//! (`backlog/docs/design/mockups/remotes.html`): Pull… of one selected tag
//! or of several, Pull and create machine… and Pull and insert…;
//! `backlog/docs/design/machine-images.md` §3.1 (local tags), §5.1 (registry
//! rules) and §5.6 (the release an image needs).
//!
//! A tag is pulled by the digest the list showed, through the path of
//! Image › Pull by reference… ([`PullRun`]): every blob is verified as it
//! arrives, blobs the store holds are not fetched again, and the local tag is
//! written last, after the whole closure is verified. Pull never moves a
//! local tag. A tag whose local name holds another image is pulled alone,
//! under another local tag the dialog asks for; a typed tag that names
//! another image is refused before anything is fetched. A tag this release
//! cannot run is not offered; should the registry's manifest say so after
//! all, the pull refuses it before any blob.
//!
//! Several tags are pulled one after another under one progress bar, which
//! counts blobs already in the store as done. Tags already in the library,
//! tags whose local name holds another image, tags this release cannot run,
//! tags whose record could not be read and tags of a registry that no longer
//! answers are left out, each with its reason. Cancel stops the running pull
//! at its next read and the waiting ones before they contact the registry;
//! each still answers. Every request runs on the library worker.
//!
//! Pull and create machine… then opens New machine from image… on the
//! pulled image. Pull and insert… then leaves the set's first disk pending in
//! a drive of a stopped machine that takes every disk of the set, for its
//! next Start (IBM Personal Computer AT Technical Reference, Diskette Drive
//! Adapter: which media a drive type takes, as `FloppyGeometry::accepted_media`
//! has it).

use crate::{
    library_view::human_size,
    registry_transfer::{
        kind_label, local_tag_problem, pulled_text, short, Accounts, PullFailure, PullRun,
        TransferEvent, TransferRequest,
    },
    remote_browse::{LocalState, TagRow},
};
use assembly::FloppyGeometry;
use image_registry::Reference;
use image_store::{machine::PendingInsert, media_type as mt, Descriptor, Digest, Store};
use std::collections::BTreeSet;

/// What the dialog does after the pull.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Pull,
    /// Then New machine from image… on the pulled machine image.
    CreateMachine,
    /// Then the set's first disk waits in a drive of a stopped machine.
    Insert,
}

impl Mode {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Pull => "Pull",
            Self::CreateMachine => "Pull and create machine",
            Self::Insert => "Pull and insert",
        }
    }
}

/// A floppy drive of a stopped machine of the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Drive {
    /// The machine's id and name.
    pub machine: String,
    pub name: String,
    pub unit: u8,
    pub geometry: FloppyGeometry,
    /// What waits in the drive for the next Start already: a pending disk
    /// or disks Create from files… made. Pull and insert… replaces it.
    pub holds: Option<String>,
}

impl Drive {
    /// "Games machine — A: 1.44M", with what the pull replaces there.
    pub fn label(&self) -> String {
        let mut label = format!(
            "{} — {} {}",
            self.name,
            crate::media_picker::drive_name(self.unit),
            crate::media_picker::format_name(self.geometry)
        );
        if let Some(holds) = &self.holds {
            label += &format!(" — replaces {holds}");
        }
        label
    }
}

/// The CD image format of an installation media set, which no machine here
/// takes yet.
const OPTICAL: &str = "iso-2048";

/// The geometry of a floppy format name such as `720K`.
fn geometry(format: &str) -> Option<FloppyGeometry> {
    assembly::descriptors::FLOPPY_TYPES
        .iter()
        .find(|kind| kind.name == format)
        .map(|kind| kind.geometry)
}

/// The drives that take every disk of a set whose media have `formats`:
/// a 1.44M drive also takes 720K disks, a 1.2M drive 360K ones.
pub(crate) fn drives_for<'a>(formats: &[String], drives: &'a [Drive]) -> Vec<&'a Drive> {
    if formats.is_empty() {
        return Vec::new();
    }
    drives
        .iter()
        .filter(|drive| {
            let accepted = drive.geometry.accepted_media();
            formats
                .iter()
                .all(|format| geometry(format).is_some_and(|g| accepted.contains(&g)))
        })
        .collect()
}

/// The formats of a set's media, each once, in set order.
fn formats(media: &[(String, String)]) -> Vec<String> {
    let mut formats: Vec<String> = Vec::new();
    for (_, format) in media {
        if !formats.contains(format) {
            formats.push(format.clone());
        }
    }
    formats
}

/// A selected tag as the list read it.
pub(crate) struct Pick<'a> {
    pub host: &'a str,
    pub repository: &'a str,
    pub tag: &'a str,
    /// `None` while its record is still read.
    pub row: Option<&'a TagRow>,
    /// Why the registry's rows cannot be pulled now ([`crate::remote_browse::RegistryView::pull_blocked`]).
    pub blocked: Option<String>,
}

/// The phrase `text` as it reads after a colon: its first letter lowercase.
fn phrase(text: String) -> String {
    let mut phrase = text;
    if let Some(first) = phrase.get_mut(..1) {
        first.make_ascii_lowercase();
    }
    phrase
}

/// Why a tag cannot be pulled at all, as the details pane says it under a
/// disabled Pull…; `None` when it can be, a clash of its local name
/// included, which the dialog resolves.
pub(crate) fn refusal(pick: &Pick<'_>) -> Option<String> {
    if let Some(blocked) = &pick.blocked {
        return Some(blocked.clone());
    }
    let Some(row) = pick.row else {
        return Some("The tag is not read yet; its row says when it is.".into());
    };
    let record = match &row.record {
        Ok(record) => record,
        Err(reason) => return Some(format!("The tag could not be read: {reason}")),
    };
    if let Some(other) = record.other_platform() {
        return Some(format!("{other}."));
    }
    if let Some(needs) = record.needs_newer() {
        return Some(format!(
            "{needs}: Pull refuses it before fetching anything."
        ));
    }
    (row.local == Some(LocalState::InLibrary))
        .then(|| "Already in the library under this name; Pull has nothing to do.".into())
}

/// Why a pull of several leaves the tag out, as a phrase after "Skipped:".
fn skip_reason(pick: &Pick<'_>) -> Option<String> {
    if pick.blocked.is_some() {
        return Some("the registry does not answer".into());
    }
    let Some(row) = pick.row else {
        return Some("it is not read yet".into());
    };
    let record = match &row.record {
        Ok(record) => record,
        Err(_) => return Some("its record could not be read".into()),
    };
    if let Some(reason) = record.not_runnable() {
        return Some(phrase(reason));
    }
    match row.local {
        Some(LocalState::InLibrary) => Some("already in the library".into()),
        Some(LocalState::OtherImage { .. }) => {
            Some("its local tag names another image; pull it alone, under another tag".into())
        }
        _ => None,
    }
}

/// Another local tag for an image whose own local name holds another image:
/// the name with `-new`, the same tag.
fn another_tag(local_tag: &str) -> String {
    match local_tag.rsplit_once(':') {
        Some((name, tag)) => format!("{name}-new:{tag}"),
        None => format!("{local_tag}-new"),
    }
}

/// One tag to pull.
#[derive(Debug)]
pub(crate) struct Item {
    /// The registry, repository, tag and the digest the list showed.
    pub source: Reference,
    pub title: Option<String>,
    pub artifact_type: String,
    /// The manifest and every distinct blob it names.
    pub size: u64,
    pub manifest_size: u64,
    /// Each blob once.
    pub blobs: Vec<Descriptor>,
    /// Those the store held when the list compared them.
    pub stored: BTreeSet<Digest>,
    /// The local tag the pull writes; one tag alone may be given another.
    pub local_tag: String,
    /// The local tag a pull writes by default.
    pub default_tag: String,
    /// Another local tag of the same image, so nothing is fetched.
    pub same_as: Option<String>,
    /// A media set's media: each one's id and format.
    pub media: Vec<(String, String)>,
    /// Why a pull of several leaves it out.
    pub skip: Option<String>,
    pub run: Option<PullRun>,
    /// The worker's answer: the line it ends with, or the failure.
    pub result: Option<Result<String, PullFailure>>,
}

impl Item {
    /// `repository:tag`, as the dialog names the tag.
    pub fn name(&self) -> String {
        format!(
            "{}:{}",
            self.source.repository,
            self.source.tag.as_deref().unwrap_or_default()
        )
    }

    /// `host/repository:tag`.
    pub fn remote(&self) -> String {
        format!("{}/{}", self.source.host, self.name())
    }

    /// The bytes a pull fetches: the manifest and the blobs the store did
    /// not hold; nothing when the image is in the library already.
    pub fn download(&self) -> u64 {
        if self.same_as.is_some() {
            return 0;
        }
        self.blobs
            .iter()
            .filter(|b| !self.stored.contains(&b.digest))
            .map(|b| b.size)
            .sum::<u64>()
            .saturating_add(self.manifest_size)
    }

    /// The bytes this pull fetches, with the blobs in the store before it
    /// (`before`) left out: the manifest and the rest.
    fn to_fetch(&self, before: &BTreeSet<Digest>) -> u64 {
        self.blobs
            .iter()
            .filter(|b| !before.contains(&b.digest))
            .map(|b| b.size)
            .sum::<u64>()
            .saturating_add(self.manifest_size)
    }

    /// The bytes of [`Self::to_fetch`] that arrived so far: blobs it
    /// verified and the part of those still arriving.
    fn fetched(&self, before: &BTreeSet<Digest>) -> u64 {
        let Some(run) = &self.run else {
            return 0;
        };
        let live = run.progress();
        let verified: u64 = live
            .fetched
            .iter()
            .filter(|d| !before.contains(&d.digest))
            .map(|d| d.size)
            .sum();
        let staged: u64 = live.receiving.iter().map(|(_, have)| *have).sum();
        verified.saturating_add(staged).min(self.to_fetch(before))
    }

    /// Bytes in the store so far, at most `size`: blobs that were there
    /// before it (`before`), those it verified and the part of those still
    /// arriving.
    fn received(&self, before: &BTreeSet<Digest>) -> u64 {
        let Some(run) = &self.run else {
            return 0;
        };
        let live = run.progress();
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
}

/// Where one tag's pull stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Left out of a pull of several, with its reason.
    Skipped,
    /// Not started yet: the dialog asks, or it waits behind the pull before.
    Waiting,
    Pulling,
    /// Cancel was asked; the pull stops at its next read.
    Stopping,
    Pulled,
    Failed,
    Cancelled,
    /// Cancelled while it waited, before contacting the registry.
    NotStarted,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Skipped => "Skipped",
            Self::Waiting => "Waiting",
            Self::Pulling => "Pulling",
            Self::Stopping => "Stopping…",
            Self::Pulled => "Pulled ✓",
            Self::Failed => "Failed ✗",
            Self::Cancelled => "Cancelled",
            Self::NotStarted => "Not started",
        }
    }
}

/// The note once every pull answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    /// Every pull that started added its tag.
    pub complete: bool,
    pub heading: String,
    pub text: String,
}

/// `a`, `a and b`, `a, b and c`.
fn joined(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Pull… of the selected tags, from the question to the last answer.
pub(crate) struct RemotePull {
    accounts: Accounts,
    pub mode: Mode,
    pub items: Vec<Item>,
    /// More than one tag was selected.
    pub several: bool,
    /// Every local tag of the library and the image it names, when the
    /// dialog opened: the typed local tag is checked against them.
    library: Vec<(String, Digest)>,
    /// The drives Pull and insert… may use, and the chosen one.
    pub drives: Vec<Drive>,
    pub drive: usize,
    /// Pull and insert… left the disk pending.
    inserted: Option<String>,
    /// The proposed local tag replaces one that names another image: the
    /// dialog puts the field in front once.
    pub focus_tag: bool,
}

impl Drop for RemotePull {
    /// Closing the dialog, also for an exit, stops its pulls.
    fn drop(&mut self) {
        self.cancel();
    }
}

impl RemotePull {
    /// The dialog for `picks`: one tag for any `mode`, several for Pull…
    /// only. `library` is every local tag; `drives` every floppy drive of
    /// the library's stopped machines.
    pub fn open(
        accounts: Accounts,
        mode: Mode,
        picks: &[Pick<'_>],
        library: Vec<(String, Digest)>,
        drives: &[Drive],
    ) -> Option<Self> {
        let several = picks.len() > 1;
        if picks.is_empty() || (several && mode != Mode::Pull) {
            return None;
        }
        let mut focus_tag = false;
        let items: Vec<Item> = picks
            .iter()
            .map(|pick| {
                let record = pick.row.and_then(|row| row.record.as_ref().ok());
                let skip = if several {
                    skip_reason(pick)
                } else {
                    refusal(pick)
                };
                let default_tag = pick.row.map_or_else(
                    || crate::remote_browse::local_tag_of(pick.host, pick.repository, pick.tag),
                    |row| row.local_tag.clone(),
                );
                let local = pick.row.and_then(|row| row.local.as_ref());
                // A tag whose own local name holds another image goes alone,
                // under another local tag, which the dialog puts in front.
                let local_tag = match local {
                    Some(LocalState::OtherImage { .. }) if !several => {
                        focus_tag = true;
                        another_tag(&default_tag)
                    }
                    _ => default_tag.clone(),
                };
                Item {
                    source: Reference {
                        host: pick.host.into(),
                        repository: pick.repository.into(),
                        tag: Some(pick.tag.to_owned()),
                        digest: record.map(|r| r.digest.clone()),
                    },
                    title: record.and_then(|r| r.title.clone()),
                    artifact_type: record.map(|r| r.artifact_type.clone()).unwrap_or_default(),
                    size: record.map_or(0, |r| r.size),
                    manifest_size: record.map_or(0, |r| r.manifest_size),
                    blobs: record
                        .map(|r| r.blobs().into_iter().cloned().collect())
                        .unwrap_or_default(),
                    stored: pick.row.map(|row| row.stored.clone()).unwrap_or_default(),
                    local_tag,
                    default_tag,
                    same_as: match local {
                        Some(LocalState::NotPulled { same_as }) => same_as.clone(),
                        _ => None,
                    },
                    media: record.map(|r| r.media.clone()).unwrap_or_default(),
                    skip,
                    run: None,
                    result: None,
                }
            })
            .collect();
        let drives = if mode == Mode::Insert {
            drives_for(&formats(&items[0].media), drives)
                .into_iter()
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        Some(Self {
            accounts,
            mode,
            items,
            several,
            library,
            drives,
            drive: 0,
            inserted: None,
            focus_tag,
        })
    }

    /// The tags a pull of several takes: those not left out.
    fn chosen(&self) -> impl Iterator<Item = (usize, &Item)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.skip.is_none())
    }

    pub fn started(&self) -> bool {
        self.items.iter().any(|item| item.run.is_some())
    }

    /// Whether a pull still waits for its answer.
    pub fn running(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.run.is_some() && item.result.is_none())
    }

    /// Cancel was asked and a pull has not answered yet.
    pub fn cancelling(&self) -> bool {
        self.items.iter().any(|item| {
            item.result.is_none() && item.run.as_ref().is_some_and(|run| run.cancelling)
        })
    }

    /// Every started pull answered.
    pub fn finished(&self) -> bool {
        self.started() && !self.running()
    }

    /// Why Pull cannot start: a tag left out, a local tag that is not valid
    /// or names another image, or no drive to insert into. `None` when it
    /// can.
    pub fn problem(&self) -> Option<String> {
        if self.several {
            return (self.chosen().next().is_none())
                .then(|| "Every selected tag is left out; see why beside each.".into());
        }
        let item = &self.items[0];
        if let Some(skip) = &item.skip {
            return Some(skip.clone());
        }
        let tag = item.local_tag.trim();
        if Store::validate_tag_name(tag).is_err() {
            return Some(local_tag_problem(tag));
        }
        let digest = item.source.digest.as_ref();
        if let Some((_, current)) = self
            .library
            .iter()
            .find(|(local, _)| local == tag)
            .filter(|(_, current)| Some(current) != digest)
        {
            return Some(format!(
                "{tag} already names another image ({}). Pull never moves a local tag: choose another one.",
                short(current)
            ));
        }
        if self.mode == Mode::Insert && self.formats().iter().any(|f| f == OPTICAL) {
            return Some(
                "Pull and insert… takes floppy sets only: no machine here takes CD images yet."
                    .into(),
            );
        }
        if self.mode == Mode::Insert && self.drives.is_empty() {
            return Some(format!(
                "Pull and insert… needs a stopped machine with a drive that takes {} disks.",
                joined(&self.formats())
            ));
        }
        None
    }

    /// A media set's formats, each once, in set order.
    pub fn formats(&self) -> Vec<String> {
        formats(&self.items[0].media)
    }

    /// The one tag's local tag names this image already: nothing is
    /// pulled, and Pull says Done.
    pub fn unchanged(&self) -> bool {
        let item = &self.items[0];
        let tag = item.local_tag.trim();
        !self.several
            && item.source.digest.is_some()
            && self.library.iter().any(|(local, current)| {
                local == tag && Some(current) == item.source.digest.as_ref()
            })
    }

    /// The label of Pull: its verb, Done when there is nothing to pull, or
    /// how many tags it takes.
    pub fn go_label(&self) -> String {
        if self.unchanged() {
            return "Done".into();
        }
        if !self.several {
            return self.mode.verb().into();
        }
        match self.chosen().count() {
            1 => "Pull 1 tag".into(),
            n => format!("Pull {n} tags"),
        }
    }

    /// What the pulls would fetch: each blob once across the tags, and none
    /// the store holds.
    pub fn download(&self) -> u64 {
        if !self.several {
            return self.items[0].download();
        }
        let mut seen = BTreeSet::new();
        let mut bytes = 0u64;
        for (_, item) in self.chosen().filter(|(_, item)| item.same_as.is_none()) {
            bytes = bytes.saturating_add(item.manifest_size);
            for blob in &item.blobs {
                if !item.stored.contains(&blob.digest) && seen.insert(blob.digest.clone()) {
                    bytes = bytes.saturating_add(blob.size);
                }
            }
        }
        bytes
    }

    /// Start the pulls, in order; the worker runs them one after another.
    pub fn start(&mut self) -> Vec<TransferRequest> {
        if self.started() || self.problem().is_some() {
            return Vec::new();
        }
        let mut requests = Vec::new();
        for item in self.items.iter_mut().filter(|item| item.skip.is_none()) {
            item.local_tag = item.local_tag.trim().to_owned();
            let (run, request) =
                PullRun::start(&self.accounts, item.source.clone(), item.local_tag.clone());
            item.run = Some(run);
            requests.push(request);
        }
        requests
    }

    /// Ask the running pull to stop at its next read and the waiting ones
    /// not to start; each still answers.
    pub fn cancel(&mut self) {
        for item in &mut self.items {
            if item.result.is_none() {
                if let Some(run) = &mut item.run {
                    run.cancel();
                }
            }
        }
    }

    /// The library worker stopped, so no answer will come.
    pub fn fail(&mut self, message: &str) {
        for item in &mut self.items {
            if item.run.is_some() && item.result.is_none() {
                item.result = Some(Err(PullFailure::Failed(message.into())));
            }
        }
    }

    /// Whether `id` is one of this dialog's pulls.
    pub fn owns(&self, id: u64) -> bool {
        self.items
            .iter()
            .any(|item| item.run.as_ref().is_some_and(|run| run.id() == id))
    }

    pub fn on_event(&mut self, event: &TransferEvent) {
        let TransferEvent::Pulled { id, result } = event else {
            return;
        };
        let Some(item) = self.items.iter_mut().find(|item| {
            item.result.is_none() && item.run.as_ref().is_some_and(|run| run.id() == *id)
        }) else {
            return;
        };
        item.result = Some(match result {
            Ok(report) => Ok(pulled_text(
                &Reference {
                    digest: None,
                    ..item.source.clone()
                },
                report,
            )),
            Err(failure) => Err(failure.clone()),
        });
    }

    /// The stage of the tag at `index`: the first started one without an
    /// answer runs, the ones after it wait.
    pub fn stage(&self, index: usize) -> Stage {
        let item = &self.items[index];
        if item.skip.is_some() {
            return Stage::Skipped;
        }
        let current = self
            .items
            .iter()
            .position(|i| i.run.is_some() && i.result.is_none());
        match (&item.run, &item.result) {
            (_, Some(Ok(_))) => Stage::Pulled,
            (_, Some(Err(PullFailure::Failed(_)))) => Stage::Failed,
            (_, Some(Err(PullFailure::Cancelled))) => Stage::Cancelled,
            (_, Some(Err(PullFailure::NotStarted))) => Stage::NotStarted,
            (None, None) => Stage::Waiting,
            (Some(_), None) if current != Some(index) => Stage::Waiting,
            (Some(run), None) if run.cancelling => Stage::Stopping,
            (Some(_), None) => Stage::Pulling,
        }
    }

    /// The stage of the tag at `index` as its row says it, the running one
    /// with how much of what it fetches arrived: "Pulling 42%".
    pub fn stage_label(&self, index: usize) -> String {
        let stage = self.stage(index);
        if stage != Stage::Pulling {
            return stage.label().into();
        }
        let item = &self.items[index];
        let before = self.stored_before(index);
        let percent = item.fetched(&before) * 100 / item.to_fetch(&before).max(1);
        format!("{} {percent}%", stage.label())
    }

    /// The line under the tag at `index`: what arrived, how it ended, or why
    /// it is left out; empty before Pull.
    pub fn detail(&self, index: usize) -> String {
        let item = &self.items[index];
        if let Some(skip) = &item.skip {
            return format!("Skipped: {skip}.");
        }
        match (&item.result, self.stage(index)) {
            (Some(Ok(message)), _) => message.clone(),
            (Some(Err(failure)), _) => failure.message().to_owned(),
            (None, Stage::Pulling | Stage::Stopping) => {
                let live = item.run.as_ref().map(PullRun::progress).unwrap_or_default();
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
                line
            }
            _ if self.started() => "Starts when the pull before it has answered.".into(),
            // Before Pull, the line of where it goes says all.
            _ => String::new(),
        }
    }

    /// The blobs in the store before the pull at `index` runs: those the
    /// list found, and those the pulls before it verified.
    fn stored_before(&self, index: usize) -> BTreeSet<Digest> {
        let mut stored = self.items[index].stored.clone();
        for earlier in &self.items[..index] {
            if let Some(run) = &earlier.run {
                stored.extend(run.progress().fetched.into_iter().map(|d| d.digest));
            }
        }
        stored
    }

    /// The one progress bar over every tag: its fraction and label.
    pub fn progress(&self) -> Option<(f32, String)> {
        if !self.started() {
            return None;
        }
        let total = self.chosen().count().max(1);
        let answered = self.chosen().filter(|(_, i)| i.result.is_some()).count();
        let current = self
            .chosen()
            .enumerate()
            .find(|(_, (_, item))| item.result.is_none());
        let pulled = self.pulled().len();
        let Some((position, (index, item))) = current else {
            let cancelled = self.items.iter().any(|item| {
                matches!(
                    item.result,
                    Some(Err(PullFailure::Cancelled | PullFailure::NotStarted))
                )
            });
            return Some((
                1.0,
                format!(
                    "{} — {pulled} of {total} pulled",
                    if cancelled { "Cancelled" } else { "Done" }
                ),
            ));
        };
        let before = self.stored_before(index);
        let part = item.received(&before) as f32 / item.size.max(1) as f32;
        let fetched = human_size(item.fetched(&before));
        let to_fetch = human_size(item.to_fetch(&before));
        let label = if self.cancelling() {
            "Cancelling — stopping at the next read…".into()
        } else if self.several {
            format!(
                "Tag {} of {total} · {} · {fetched} of {to_fetch} to fetch",
                position + 1,
                item.name(),
            )
        } else {
            format!("Fetching {fetched} of {to_fetch}")
        };
        Some(((answered as f32 + part) / total as f32, label))
    }

    /// The local tags the finished pulls wrote.
    pub fn pulled(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| matches!(item.result, Some(Ok(_))))
            .map(|item| item.local_tag.clone())
            .collect()
    }

    /// What the pulls added, once every one answered.
    pub fn summary(&self) -> Option<Summary> {
        if !self.finished() {
            return None;
        }
        let pulled = self.pulled();
        let started = self.items.iter().filter(|i| i.run.is_some()).count();
        let cancelled = self.items.iter().any(|item| {
            matches!(
                item.result,
                Some(Err(PullFailure::Cancelled | PullFailure::NotStarted))
            )
        });
        let inserted = self
            .inserted
            .as_ref()
            .map(|place| format!(" {place}"))
            .unwrap_or_default();
        Some(if pulled.len() == started && !self.several {
            let item = &self.items[0];
            Summary {
                complete: true,
                heading: "Pulled.".into(),
                text: format!(
                    "{} is in the library as {}. The tag was written after every blob was verified.{inserted}",
                    item.remote(),
                    item.local_tag
                ),
            }
        } else if pulled.len() == started {
            Summary {
                complete: true,
                heading: format!(
                    "Pulled {started} {}.",
                    if started == 1 { "tag" } else { "tags" }
                ),
                text: format!(
                    "Each is in the library under the local tag shown: {}.",
                    joined(&pulled)
                ),
            }
        } else if cancelled && pulled.is_empty() {
            Summary {
                complete: false,
                heading: "Cancelled.".into(),
                text: if self.several {
                    "The running pull stopped at its next read and the waiting ones did not start. Their local tags were not written; blobs already fetched stay in the store, and the next pull fetches only the rest.".into()
                } else {
                    "The local tag was not written. Blobs already fetched stay in the store, and the next pull fetches only the rest.".into()
                },
            }
        } else if pulled.is_empty() {
            Summary {
                complete: false,
                heading: "Nothing was pulled.".into(),
                text: "No local tag was written; the line under each tag says why.".into(),
            }
        } else {
            Summary {
                complete: false,
                heading: format!("Pulled {} of {started} tags.", pulled.len()),
                text: format!(
                    "In the library as {}. The others' local tags were not written; blobs already fetched stay in the store, and the next pull fetches only the rest.",
                    joined(&pulled)
                ),
            }
        })
    }

    /// Once Pull and insert… pulled its set: the machine whose drive gets
    /// the set's first disk at its next Start, and that disk. Given once.
    pub fn take_insert(&mut self) -> Option<(String, PendingInsert)> {
        if self.mode != Mode::Insert || self.inserted.is_some() {
            return None;
        }
        let item = &self.items[0];
        if !matches!(item.result, Some(Ok(_))) {
            return None;
        }
        let drive = self.drives.get(self.drive)?;
        let (first, _) = item.media.first()?;
        let insert = PendingInsert {
            drive: crate::media_picker::drive_name(drive.unit),
            set: Descriptor::new(
                mt::MANIFEST,
                item.source.digest.clone()?,
                item.manifest_size,
            ),
            reference: item.local_tag.clone(),
            medium: first.clone(),
            all: item.media.len() > 1,
            journal: true,
        };
        self.inserted = Some(format!(
            "Disk 1 waits in {}’s {} drive until its next Start.",
            drive.name, insert.drive
        ));
        Some((drive.machine.clone(), insert))
    }

    /// Once Pull and create machine… pulled its image: the image New
    /// machine from image… starts from, and the name it proposes.
    pub fn created_image(&self) -> Option<(String, Descriptor, String)> {
        if self.mode != Mode::CreateMachine {
            return None;
        }
        let item = &self.items[0];
        if !matches!(item.result, Some(Ok(_))) {
            return None;
        }
        Some((
            item.local_tag.clone(),
            Descriptor::new(
                mt::MANIFEST,
                item.source.digest.clone()?,
                item.manifest_size,
            ),
            item.title.clone().unwrap_or_else(|| item.local_tag.clone()),
        ))
    }
}

#[cfg(test)]
mod tests;
