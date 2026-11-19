// SPDX-License-Identifier: AGPL-3.0-or-later
//! Push to… of several library items, and Push here… of a registry or
//! namespace; the approved remotes-push mock-up
//! (`backlog/docs/design/mockups/remotes-push.html`, Remotes revision 3).
//!
//! **Several items** ([`PushBatch::open_to`]) go one after another under one
//! progress bar. A tag that names a registry goes there and a short one
//! pulled from the default registry goes back, as the Remotes rule says
//! ([`crate::push_destination`]); the other short and local ones go to the
//! registry and namespace picked once in the dialog, each under its own
//! path and tag, or where they were pushed last until a pick. Each item has
//! its own verdict; the push takes only those that are ready, and skips the
//! rest with the reason: refused by the guard, a tag that cannot move, a
//! tag that would move (push it alone to confirm), already there, not
//! logged in. A batch asks nothing on the way.
//!
//! **Push here…** ([`PushBatch::open_here`]) fixes the place, a namespace or
//! a registry, and lists the library with the verdict for this place per
//! item; Ctrl (⌘ on macOS) or Shift selects several. Items that cannot go
//! here are hidden behind a check box and say why. One selected item whose
//! tag would move goes after the move is confirmed, as from Push to….
//!
//! The run reuses the one push of [`crate::registry_transfer`]: each item is
//! a [`PushJob`](crate::registry_transfer::PushJob), the worker runs them in order, and each answers. A
//! failure on the way marks its row Failed ✗ and the next item starts.
//! Cancel stops the running push at its next blob or chunk and the waiting
//! ones before they contact a registry; items already pushed stay pushed.

use crate::{
    platform::PlatformFacts,
    push_destination::{
        here_destination, rule_destination, Evaluation, Listing, LocalImage, Place, Rule, Rules,
        Verdict,
    },
    registry_transfer::{
        next_id, record_push, start_job, Accounts, PlanData, Progress, PushFailure, TransferEvent,
        TransferRequest, PUSH_CANCELLED, STALE,
    },
};
use image_registry::{push::PushProgress, Reference};
use image_store::Digest;
use std::sync::atomic::Ordering;

/// Where the items go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Push to…: the library selection, each by the Remotes rule.
    To,
    /// Push here…: the place is fixed and the items are picked.
    Here(Place),
}

/// One library item of the dialog.
#[derive(Debug)]
pub struct Item {
    pub image: LocalImage,
    /// The dry run, or why it could not be read.
    pub plan: Result<PlanData, String>,
    pub target: Option<Reference>,
    pub rule: Rule,
    pub evaluation: Evaluation,
    /// Why the run leaves it out.
    pub skip: Option<String>,
    pub run: Option<Progress>,
    /// The worker's answer: the line it ends with, or the failure.
    pub result: Option<Result<String, PushFailure>>,
}

impl Item {
    /// The bytes it sends at most: every distinct blob.
    pub fn size(&self) -> u64 {
        self.plan.as_ref().map_or(0, PlanData::bytes)
    }

    /// Every blob is open.
    pub fn open(&self) -> bool {
        self.plan.as_ref().is_ok_and(PlanData::open)
    }

    /// The destination as text.
    pub fn reference(&self) -> String {
        self.target
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    /// It can go to the place of Push here…: ready, after a login, or alone
    /// after the move is confirmed.
    pub fn can_go_here(&self) -> bool {
        matches!(
            self.evaluation.verdict,
            Verdict::Ready | Verdict::Login | Verdict::Move { .. }
        )
    }

    /// How far its push is, from 0 to 1.
    fn fraction(&self) -> f32 {
        let Some(run) = &self.run else {
            return 0.0;
        };
        if run.published || matches!(self.result, Some(Ok(_))) {
            return 1.0;
        }
        let blobs = run.blobs();
        let bytes: u64 = blobs.iter().map(|row| row.size).sum();
        let sent: u64 = blobs.iter().map(|row| row.sent.min(row.size)).sum();
        sent as f32 / bytes.max(1) as f32 * 0.99
    }
}

/// Where one item's push stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Skipped,
    Waiting,
    Pushing,
    Stopping,
    Pushed,
    Failed,
    Cancelled,
    NotStarted,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Skipped => "Skipped",
            Self::Waiting => "Waiting",
            Self::Pushing => "Pushing",
            Self::Stopping => "Stopping…",
            Self::Pushed => "Pushed ✓",
            Self::Failed => "Failed ✗",
            Self::Cancelled => "Cancelled",
            Self::NotStarted => "Not started",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BatchState {
    /// The worker reads every local image's dry run.
    Planning,
    Ready,
    /// One item of Push here… whose tag names another image there.
    ConfirmMove {
        index: usize,
        current: Digest,
        new: Digest,
    },
    /// The pushes run, or answered.
    Run,
}

/// The note once every push answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub complete: bool,
    pub heading: String,
    pub text: String,
}

/// Push to… of several items, or Push here….
pub struct PushBatch {
    accounts: Accounts,
    /// What the dry run and the progress name blob types by.
    pub facts: PlatformFacts,
    pub rules: Rules,
    pub mode: Mode,
    /// Push to…'s selection in its order, or every local image for Push
    /// here….
    pub items: Vec<Item>,
    /// The tags Push to… takes, until the dry runs come.
    wanted: Vec<String>,
    /// Push to…: the registry and namespace the short tags go to.
    pub pick: Option<Place>,
    /// Push here…: the items selected, by tag.
    pub picked: Vec<String>,
    anchor: Option<String>,
    /// Push here…: the item whose checks show.
    pub focus: Option<String>,
    /// Push here…: the type shown, by kind; `None` for every type.
    pub kind: Option<&'static str>,
    /// Push here…: the items that cannot go here show too.
    pub show_all: bool,
    planning: Option<u64>,
    /// The library could not be read.
    pub error: Option<String>,
    pub state: BatchState,
    /// The items the run shows, in order.
    pub queue: Vec<usize>,
    /// The registry tags this dialog published, for Remotes and Source.
    pub landed: Vec<Reference>,
}

impl Drop for PushBatch {
    /// Closing the dialog, also for an exit, stops its pushes.
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The order of the types in Push here…'s list.
const KINDS: [&str; 3] = ["machine image", "media set", "firmware package"];

impl PushBatch {
    fn open(
        accounts: Accounts,
        facts: PlatformFacts,
        listing: Listing,
        mode: Mode,
        wanted: Vec<String>,
    ) -> (Self, TransferRequest) {
        let id = next_id();
        let rules = accounts.rules(listing);
        (
            Self {
                accounts,
                facts,
                rules,
                mode,
                items: Vec::new(),
                wanted,
                pick: None,
                picked: Vec::new(),
                anchor: None,
                focus: None,
                kind: None,
                show_all: false,
                planning: Some(id),
                error: None,
                state: BatchState::Planning,
                queue: Vec::new(),
                landed: Vec::new(),
            },
            TransferRequest::PlanAll { id },
        )
    }

    /// Push to… of the selected `tags`, checked against this session's
    /// `listing`; the request asks the worker for their dry runs.
    pub fn open_to(
        accounts: Accounts,
        facts: PlatformFacts,
        listing: Listing,
        tags: Vec<String>,
    ) -> (Self, TransferRequest) {
        Self::open(accounts, facts, listing, Mode::To, tags)
    }

    /// Push here… of `place`.
    pub fn open_here(
        accounts: Accounts,
        facts: PlatformFacts,
        listing: Listing,
        place: Place,
    ) -> (Self, TransferRequest) {
        Self::open(accounts, facts, listing, Mode::Here(place), Vec::new())
    }

    /// How many items Push to… takes, before and after their dry runs.
    pub fn total(&self) -> usize {
        self.items.len().max(self.wanted.len())
    }

    /// The place of Push here….
    pub fn here(&self) -> Option<&Place> {
        match &self.mode {
            Mode::Here(place) => Some(place),
            Mode::To => None,
        }
    }

    /// Read the registries list again, as after a login, and judge every
    /// item anew.
    pub fn reload(&mut self, listing: Listing) {
        self.rules = self.accounts.rules(listing);
        self.judge();
    }

    /// Push here… of a registry whose entry lists several namespaces: the
    /// dialog asks which one, as the chooser does.
    pub fn asks_namespace(&self) -> bool {
        match &self.mode {
            Mode::Here(place) => {
                place.namespace.is_none()
                    && self
                        .rules
                        .config
                        .entry(&place.host)
                        .is_some_and(|e| e.namespaces.len() > 1)
            }
            Mode::To => false,
        }
    }

    /// Where Push here… goes: its place, or the namespace picked in it.
    pub fn here_place(&self) -> Option<Place> {
        if self.asks_namespace() {
            return self.pick.clone();
        }
        self.here().cloned()
    }

    /// Every item's destination and verdict, from the mode, the pick and
    /// the rules.
    fn judge(&mut self) {
        let here = self.here_place();
        for item in &mut self.items {
            let (target, rule) = match (&self.mode, &here) {
                (Mode::To, _) => rule_destination(&item.image, self.pick.as_ref()),
                (Mode::Here(_), Some(place)) => {
                    (Some(here_destination(&item.image, place)), Rule::Here)
                }
                (Mode::Here(_), None) => (None, Rule::Here),
            };
            let text = target.as_ref().map(ToString::to_string).unwrap_or_default();
            let (plan, error) = match &item.plan {
                Ok(plan) => (Some(plan), None),
                Err(error) => (None, Some(error.as_str())),
            };
            item.evaluation = self.rules.evaluate(plan, error, &text);
            item.target = target;
            item.rule = rule;
        }
        let items = &self.items;
        self.picked.retain(|tag| {
            items
                .iter()
                .any(|item| item.image.tag == *tag && item.can_go_here())
        });
    }

    /// Push to…: choose where the short and local tags go; Push here… of a
    /// registry of several namespaces: choose the namespace.
    pub fn choose(&mut self, place: Place) {
        if self.state == BatchState::Ready {
            self.pick = Some(place);
            self.judge();
        }
    }

    /// Push to…: the items that go elsewhere whatever is picked, and those
    /// the pick moves.
    pub fn counts(&self) -> (usize, usize) {
        let picks = self
            .items
            .iter()
            .filter(|item| matches!(item.rule, Rule::Pick | Rule::Last))
            .count();
        (picks, self.items.len() - picks)
    }

    /// Push here…'s rows: the items that can go here first, then by type and
    /// tag; those that cannot go here only with [`Self::show_all`].
    pub fn shown(&self) -> Vec<usize> {
        let mut shown: Vec<usize> = (0..self.items.len())
            .filter(|&i| {
                let item = &self.items[i];
                self.kind.is_none_or(|kind| item.image.kind == kind)
                    && (self.show_all || item.can_go_here() || self.here_place().is_none())
            })
            .collect();
        shown.sort_by_key(|&i| {
            let item = &self.items[i];
            (
                !item.can_go_here(),
                KINDS.iter().position(|k| *k == item.image.kind),
                item.image.tag.clone(),
            )
        });
        shown
    }

    /// Push here…: items of the type shown that cannot go here.
    pub fn cannot(&self) -> usize {
        self.items
            .iter()
            .filter(|item| self.kind.is_none_or(|kind| item.image.kind == kind))
            .filter(|item| !item.can_go_here())
            .count()
    }

    /// Push here…: a click on an item's row. Ctrl (⌘ on macOS) adds or
    /// removes it, Shift selects the rows between the last click and this
    /// one; an item that cannot go here only shows its checks.
    pub fn click(&mut self, tag: &str, command: bool, shift: bool) {
        self.focus = Some(tag.into());
        let Some(item) = self.items.iter().find(|i| i.image.tag == tag) else {
            return;
        };
        if !item.can_go_here() || self.state != BatchState::Ready {
            return;
        }
        let rows: Vec<String> = self
            .shown()
            .into_iter()
            .filter(|&i| self.items[i].can_go_here())
            .map(|i| self.items[i].image.tag.clone())
            .collect();
        if command {
            match self.picked.iter().position(|t| t == tag) {
                Some(at) => {
                    self.picked.remove(at);
                }
                None => self.picked.push(tag.into()),
            }
            self.anchor = Some(tag.into());
        } else if let (true, Some(anchor)) = (shift, &self.anchor) {
            let at = |t: &str| rows.iter().position(|r| r == t);
            self.picked = match (at(anchor), at(tag)) {
                (Some(a), Some(b)) => rows[a.min(b)..=a.max(b)].to_vec(),
                _ => vec![tag.into()],
            };
        } else {
            self.picked = vec![tag.into()];
            self.anchor = Some(tag.into());
        }
    }

    /// The items Push takes now: Push to…'s ready ones; Push here…'s
    /// selected ready ones, or one selected item whose tag moves.
    fn takes(&self) -> Vec<usize> {
        match &self.mode {
            Mode::To => (0..self.items.len())
                .filter(|&i| self.items[i].evaluation.verdict == Verdict::Ready)
                .collect(),
            Mode::Here(_) => {
                let picked: Vec<usize> = self.picked_items();
                if let [one] = picked.as_slice() {
                    if self.items[*one].evaluation.verdict.pushes()
                        && self.items[*one].evaluation.verdict != Verdict::Same
                    {
                        return vec![*one];
                    }
                }
                picked
                    .into_iter()
                    .filter(|&i| self.items[i].evaluation.verdict == Verdict::Ready)
                    .collect()
            }
        }
    }

    fn picked_items(&self) -> Vec<usize> {
        self.picked
            .iter()
            .filter_map(|tag| self.items.iter().position(|i| i.image.tag == *tag))
            .collect()
    }

    /// The label of Push: how many items it takes.
    pub fn go_label(&self) -> String {
        match self.takes().len() {
            1 => "Push 1 item".into(),
            n => format!("Push {n} items"),
        }
    }

    pub fn can_push(&self) -> bool {
        self.state == BatchState::Ready && !self.takes().is_empty()
    }

    /// The line beside Push before it starts.
    pub fn message(&self) -> String {
        let takes = self.takes().len();
        match &self.mode {
            Mode::To => {
                let skipped = self.items.len() - takes;
                if skipped > 0 {
                    format!("{skipped} will be skipped. Nothing has been sent.")
                } else {
                    "Nothing has been sent.".into()
                }
            }
            Mode::Here(_) => {
                let picked = self.picked.len();
                if picked == 0 {
                    "Select the items to push. Nothing has been sent.".into()
                } else if takes < picked {
                    format!(
                        "{} of the selected will be skipped; the checks say why. Nothing has been sent.",
                        picked - takes
                    )
                } else {
                    "Nothing has been sent.".into()
                }
            }
        }
    }

    /// Start the pushes, in order: the worker runs them one after another.
    /// The other items of the run are skipped with their reason. One item
    /// of Push here… whose tag moves asks first.
    pub fn start(&mut self) -> Vec<TransferRequest> {
        if !self.can_push() {
            return Vec::new();
        }
        let takes = self.takes();
        if let [one] = takes.as_slice() {
            if let Verdict::Move { current } = &self.items[*one].evaluation.verdict {
                let new = match &self.items[*one].plan {
                    Ok(plan) => plan.image.digest.clone(),
                    Err(_) => return Vec::new(),
                };
                self.state = BatchState::ConfirmMove {
                    index: *one,
                    current: current.clone(),
                    new,
                };
                return Vec::new();
            }
        }
        self.run(takes, None)
    }

    /// Push the one item whose move was confirmed, away from the digest
    /// that was shown.
    pub fn confirm_move(&mut self) -> Vec<TransferRequest> {
        let BatchState::ConfirmMove { index, current, .. } = &self.state else {
            return Vec::new();
        };
        let (index, current) = (*index, current.clone());
        self.run(vec![index], Some(current))
    }

    /// Back from the move confirmation.
    pub fn decline_move(&mut self) {
        if matches!(self.state, BatchState::ConfirmMove { .. }) {
            self.state = BatchState::Ready;
        }
    }

    fn run(&mut self, takes: Vec<usize>, move_from: Option<Digest>) -> Vec<TransferRequest> {
        self.queue = match &self.mode {
            Mode::To => (0..self.items.len()).collect(),
            Mode::Here(_) if takes.len() == 1 && self.picked.len() <= 1 => takes.clone(),
            Mode::Here(_) => self.picked_items(),
        };
        let mut requests = Vec::new();
        for &index in &self.queue {
            let item = &mut self.items[index];
            item.result = None;
            item.run = None;
            if !takes.contains(&index) {
                item.skip = Some(
                    item.evaluation
                        .skip_reason()
                        .unwrap_or_else(|| "it is not ready".into()),
                );
                continue;
            }
            item.skip = None;
            let (Ok(plan), Some(target)) = (&item.plan, item.target.clone()) else {
                continue;
            };
            let (progress, job) = start_job(
                &self.facts,
                &self.accounts,
                plan,
                target,
                plan.image.digest.clone(),
                move_from.clone(),
            );
            item.run = Some(progress);
            requests.push(TransferRequest::Push(Box::new(job)));
        }
        self.state = BatchState::Run;
        requests
    }

    pub fn started(&self) -> bool {
        self.state == BatchState::Run
    }

    /// Whether a push still waits for its answer.
    pub fn running(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.run.is_some() && item.result.is_none())
    }

    pub fn cancelling(&self) -> bool {
        self.items.iter().any(|item| {
            item.result.is_none() && item.run.as_ref().is_some_and(|run| run.cancelling)
        })
    }

    pub fn finished(&self) -> bool {
        self.started() && !self.running()
    }

    /// Stop the running push at its next blob or chunk and the waiting ones
    /// before they start; each still answers.
    pub fn cancel(&mut self) {
        for item in &mut self.items {
            if item.result.is_none() {
                if let Some(run) = &mut item.run {
                    run.cancel.store(true, Ordering::SeqCst);
                    run.cancelling = true;
                }
            }
        }
    }

    /// The library worker stopped, so no answer will come.
    pub fn fail(&mut self, message: &str) {
        if self.planning.take().is_some() {
            self.error = Some(message.into());
            self.state = BatchState::Ready;
        }
        for item in &mut self.items {
            if item.run.is_some() && item.result.is_none() {
                item.result = Some(Err(PushFailure::Failed(message.into())));
            }
        }
    }

    /// Whether `id` is one of this dialog's jobs.
    pub fn owns(&self, id: u64) -> bool {
        self.planning == Some(id)
            || self
                .items
                .iter()
                .any(|item| item.run.as_ref().is_some_and(|run| run.id == id))
    }

    pub fn on_event(&mut self, event: &TransferEvent) {
        match event {
            TransferEvent::PlannedAll { id, result } if self.planning == Some(*id) => {
                self.planning = None;
                self.state = BatchState::Ready;
                match result {
                    Ok(images) => {
                        let take = |(image, plan): &(LocalImage, Result<PlanData, String>)| Item {
                            image: image.clone(),
                            plan: plan.clone(),
                            target: None,
                            rule: Rule::Pick,
                            evaluation: Evaluation::default(),
                            skip: None,
                            run: None,
                            result: None,
                        };
                        self.items = match &self.mode {
                            Mode::To => self
                                .wanted
                                .iter()
                                .filter_map(|tag| images.iter().find(|(i, _)| i.tag == *tag))
                                .map(take)
                                .collect(),
                            Mode::Here(_) => images.iter().map(take).collect(),
                        };
                        if self.items.is_empty() {
                            self.error = Some("The image store holds no image to push yet.".into());
                        }
                        self.judge();
                    }
                    Err(error) => self.error = Some(error.clone()),
                }
            }
            TransferEvent::Landed { id, landed } => {
                if self.owns(*id) {
                    self.landed.push(landed.target.clone());
                }
            }
            TransferEvent::Pushed { id, result } => {
                let Some(item) = self.items.iter_mut().find(|item| {
                    item.result.is_none() && item.run.as_ref().is_some_and(|run| run.id == *id)
                }) else {
                    return;
                };
                if let (Ok(report), Some(run)) = (result, &mut item.run) {
                    run.live.update(|rows| {
                        for (descriptor, outcome) in &report.blobs {
                            record_push(rows, PushProgress::Blob(descriptor, *outcome));
                        }
                    });
                    run.published = true;
                }
                item.result = Some(match result {
                    Ok(report) => Ok(crate::registry_transfer::pushed_text(
                        &item.target.clone().unwrap_or_else(|| report_target(item)),
                        report,
                    )),
                    Err(failure) => Err(failure.clone()),
                });
            }
            _ => {}
        }
    }

    /// The stage of the item at `index`: the first started one without an
    /// answer runs, the ones after it wait.
    pub fn stage(&self, index: usize) -> Stage {
        let item = &self.items[index];
        if item.skip.is_some() {
            return Stage::Skipped;
        }
        let current = self
            .queue
            .iter()
            .copied()
            .find(|&i| self.items[i].run.is_some() && self.items[i].result.is_none());
        match (&item.run, &item.result) {
            (_, Some(Ok(_))) => Stage::Pushed,
            (_, Some(Err(PushFailure::Cancelled))) => Stage::Cancelled,
            (_, Some(Err(PushFailure::NotStarted))) => Stage::NotStarted,
            (_, Some(Err(_))) => Stage::Failed,
            (None, None) => Stage::Waiting,
            (Some(_), None) if current != Some(index) => Stage::Waiting,
            (Some(run), None) if run.cancelling => Stage::Stopping,
            (Some(_), None) => Stage::Pushing,
        }
    }

    /// The stage as the row says it, the running one with its share sent:
    /// "Pushing 42%".
    pub fn stage_label(&self, index: usize) -> String {
        let stage = self.stage(index);
        if stage != Stage::Pushing {
            return stage.label().into();
        }
        let percent = (self.items[index].fraction() * 100.0).round() as u32;
        format!("{} {percent}%", stage.label())
    }

    /// Why the item at `index` was skipped or failed, as its row says it.
    pub fn detail(&self, index: usize) -> Option<String> {
        let item = &self.items[index];
        if let Some(skip) = &item.skip {
            return Some(format!("Skipped: {skip}."));
        }
        let reason = match &item.result {
            Some(Err(PushFailure::Failed(text))) => text.trim_end_matches('.').to_owned(),
            Some(Err(PushFailure::Refused(text))) => {
                format!("push refused: {}", text.trim_end_matches('.'))
            }
            Some(Err(PushFailure::Move { .. })) => {
                "its tag names another image on the registry; push it alone to confirm the move"
                    .into()
            }
            Some(Err(PushFailure::Stale)) => STALE.trim_end_matches('.').to_owned(),
            _ => return None,
        };
        Some(format!("Failed: {reason}."))
    }

    fn todo(&self) -> impl Iterator<Item = &Item> {
        self.queue
            .iter()
            .map(|&i| &self.items[i])
            .filter(|item| item.skip.is_none())
    }

    fn pushed(&self) -> usize {
        self.todo()
            .filter(|item| matches!(item.result, Some(Ok(_))))
            .count()
    }

    fn failed(&self) -> usize {
        self.todo()
            .filter(|item| {
                matches!(
                    item.result,
                    Some(Err(ref f)) if !matches!(f, PushFailure::Cancelled | PushFailure::NotStarted)
                )
            })
            .count()
    }

    fn was_cancelled(&self) -> bool {
        self.todo().any(|item| {
            matches!(
                item.result,
                Some(Err(PushFailure::Cancelled | PushFailure::NotStarted))
            )
        })
    }

    /// The one progress bar over every item: its fraction and label.
    pub fn progress(&self) -> Option<(f32, String)> {
        if !self.started() {
            return None;
        }
        let total = self.todo().count();
        let skipped = self.queue.len() - total;
        let current = self
            .todo()
            .enumerate()
            .find(|(_, item)| item.result.is_none() && item.run.is_some());
        let ended = self.todo().filter(|item| item.result.is_some()).count();
        match current {
            Some((position, item)) => {
                let label = if self.cancelling() {
                    "Cancelling — stopping at the next blob or chunk…".into()
                } else {
                    format!("Item {} of {total} · {}", position + 1, item.image.tag)
                };
                Some((
                    (ended as f32 + item.fraction()) / total.max(1) as f32,
                    label,
                ))
            }
            None => {
                let mut label = format!(
                    "{} — {} of {total} pushed",
                    if self.was_cancelled() {
                        "Cancelled"
                    } else {
                        "Done"
                    },
                    self.pushed()
                );
                if skipped > 0 {
                    label += &format!(" · {skipped} skipped");
                }
                let failed = self.failed();
                if failed > 0 {
                    label += &format!(" · {failed} failed");
                }
                Some((1.0, label))
            }
        }
    }

    /// What the run did, once every push answered.
    pub fn summary(&self) -> Option<Summary> {
        if !self.finished() {
            return None;
        }
        let pushed = self.pushed();
        if self.was_cancelled() {
            return Some(Summary {
                complete: false,
                heading: "Cancelled.".into(),
                text: format!(
                    "The running push stopped at its next blob or chunk and the waiting ones did not start. Their manifests were not published, so the registry keeps its tags.{}",
                    if pushed > 0 {
                        format!(" The {pushed} already pushed stay pushed.")
                    } else {
                        String::new()
                    }
                ),
            });
        }
        let skipped = self.queue.len() - self.todo().count();
        let failed = self.failed();
        let items = self.queue.len();
        Some(Summary {
            complete: skipped == 0 && failed == 0,
            heading: format!(
                "Pushed {pushed} of {items} {}.",
                if items == 1 { "item" } else { "items" }
            ),
            text: format!(
                "{}{}; each row says why.",
                if skipped > 0 {
                    format!("{skipped} skipped")
                } else {
                    "None skipped".into()
                },
                if failed > 0 {
                    format!(", {failed} failed")
                } else {
                    String::new()
                }
            ),
        })
    }

    /// The note after a run with a failure.
    pub fn failed_note(&self) -> Option<&'static str> {
        (self.finished() && !self.was_cancelled() && self.failed() > 0)
            .then_some("Push to… on a failed item again resumes its accepted chunks.")
    }

    /// The registries the run published to, each once.
    pub fn hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for target in &self.landed {
            if !hosts.contains(&target.host) {
                hosts.push(target.host.clone());
            }
        }
        hosts
    }

    /// The first tag published, for Show in Remotes.
    pub fn first_landed(&self) -> Option<&Reference> {
        self.landed.first()
    }

    /// The text of a cancelled single push, as Push to… says it.
    pub fn cancelled_text() -> &'static str {
        PUSH_CANCELLED
    }
}

/// An item's target when it has none: never, as a push needs one.
fn report_target(item: &Item) -> Reference {
    item.run
        .as_ref()
        .map(|run| run.target.clone())
        .unwrap_or_else(|| Reference {
            host: String::new(),
            repository: String::new(),
            tag: None,
            digest: None,
        })
}
