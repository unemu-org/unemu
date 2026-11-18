// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pull… from Remotes as a model: what it pulls, under which local tag, what
//! it leaves out and why, its one progress bar, Cancel, and what Pull and
//! create machine… and Pull and insert… do once the pull answered. The
//! pulls themselves, against a registry, are in the Remotes view's tests.

use super::*;
use crate::{
    pc_desktop::PcDesktop,
    registry_transfer::PullFailure,
    remote_browse::{local_tag_of, TagRecord},
};
use image_registry::{
    credential_store::SessionStore,
    credentials::EnvCredentials,
    pull::{PullOutcome, PullReport},
};
use image_store::{machine::PendingInsert, media_type as mt, Descriptor, Digest};
use library_model::{
    registry_transfer::{Accounts, TransferEvent, TransferRequest},
    remote_browse::{LocalState, TagRow},
    PlatformFacts,
};
use std::collections::BTreeSet;
use std::sync::Arc;

const LAN: &str = "registry.lan.example";

fn accounts() -> Accounts {
    Accounts {
        path: None,
        store: Arc::new(SessionStore::default()),
        env: EnvCredentials::default(),
        default_override: None,
    }
}

fn blob(kind: &str, label: &str, size: u64) -> Descriptor {
    Descriptor::new(kind, Digest::of(label.as_bytes()), size)
}

/// A record of `title` whose manifest is 1 KiB and names a 2 KiB config and
/// `layers`.
fn record(artifact_type: &str, title: &str, layers: &[Descriptor]) -> TagRecord {
    let mut descriptors = vec![blob(mt::CONFIG, &format!("{title} config"), 2048)];
    descriptors.extend(layers.iter().cloned());
    let size = 1024 + descriptors.iter().map(|d| d.size).sum::<u64>();
    TagRecord {
        digest: Digest::of(title.as_bytes()),
        artifact_type: artifact_type.into(),
        title: Some(title.into()),
        version: None,
        formats: Vec::new(),
        min_app_version: None,
        platform: Some(pc_image::PLATFORM.into()),
        size,
        user_supplied: false,
        annotations: Default::default(),
        details: None,
        manifest_size: 1024,
        descriptors,
        media: Vec::new(),
    }
}

fn row(repository: &str, tag: &str, record: TagRecord, local: LocalState) -> TagRow {
    TagRow {
        tag: tag.into(),
        local_tag: local_tag_of(LAN, repository, tag),
        record: Ok(record),
        local: Some(local),
        stored: BTreeSet::new(),
        newest_revision: None,
    }
}

fn not_pulled() -> LocalState {
    LocalState::NotPulled { same_as: None }
}

/// The PC's facts, which every pick of these tests reads.
fn facts() -> &'static PlatformFacts {
    static FACTS: std::sync::LazyLock<PlatformFacts> = std::sync::LazyLock::new(PcDesktop::facts);
    &FACTS
}

fn pick<'a>(repository: &'a str, row: &'a TagRow) -> Pick<'a> {
    Pick {
        host: LAN,
        repository,
        tag: &row.tag,
        row: Some(row),
        blocked: None,
        facts: facts(),
    }
}

fn pull_jobs(requests: Vec<TransferRequest>) -> Vec<crate::registry_transfer::PullJob> {
    requests
        .into_iter()
        .map(|request| match request {
            TransferRequest::Pull(job) => *job,
            _ => panic!("a pull"),
        })
        .collect()
}

fn pulled(id: u64, tag: &str) -> TransferEvent {
    TransferEvent::Pulled {
        id,
        result: Ok(PullReport {
            local_tag: tag.into(),
            image: blob(mt::MANIFEST, tag, 1024),
            fetched: Vec::new(),
            present: Vec::new(),
            outcome: PullOutcome::Pulled,
            alias: None,
        }),
    }
}

/// One tag is pulled by the digest the list showed, written under the
/// local tag a pull writes. A tag whose local name holds another image is
/// offered another local tag; typed back, the clash is refused before
/// anything is asked of the worker, and so is a tag the store refuses.
#[test]
fn one_tag_is_pulled_by_its_digest_and_never_moves_a_local_tag() {
    let game = record(mt::MACHINE, "Dune II", &[blob(mt::FIRMWARE, "rom", 4096)]);
    let digest = game.digest.clone();
    let tag = row("private/games/dune2", "1.0.0", game.clone(), not_pulled());
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("private/games/dune2", &tag)],
        Vec::new(),
        &[],
    )
    .unwrap();
    assert_eq!(dialog.problem(), None);
    assert_eq!(dialog.go_label(), "Pull");
    assert_eq!(dialog.download(), 1024 + 2048 + 4096);
    let jobs = pull_jobs(dialog.start());
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].source.digest.as_ref(), Some(&digest), "by digest");
    assert_eq!(jobs[0].source.tag.as_deref(), Some("1.0.0"));
    assert_eq!(
        jobs[0].local_tag,
        format!("{LAN}/private/games/dune2:1.0.0")
    );
    assert!(dialog.start().is_empty(), "started once");

    // The local name holds another image: Pull offers another local tag.
    let other = Digest::of(b"pulled before the registry changed");
    let local = format!("{LAN}/private/games/dune2:1.0.0");
    let clash = row(
        "private/games/dune2",
        "1.0.0",
        game,
        LocalState::OtherImage {
            current: other.clone(),
        },
    );
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("private/games/dune2", &clash)],
        vec![(local.clone(), other)],
        &[],
    )
    .unwrap();
    assert_eq!(
        dialog.items[0].local_tag,
        format!("{LAN}/private/games/dune2-new:1.0.0")
    );
    assert_eq!(dialog.problem(), None);
    dialog.items[0].local_tag = local.clone();
    let problem = dialog.problem().unwrap();
    assert!(
        problem.starts_with(&format!("{local} already names another image (sha256:"))
            && problem
                .ends_with("Pull moves no local tag but a release alias: choose another one."),
        "{problem}"
    );
    assert!(dialog.start().is_empty(), "a clash is never pulled");
    dialog.items[0].local_tag = "Not A Tag".into();
    assert!(dialog
        .problem()
        .unwrap()
        .starts_with("Local tag “Not A Tag” is not valid"));
    assert!(dialog.start().is_empty());
    // A free local tag of the user's choice is fine.
    dialog.items[0].local_tag = "dune2-copy:1.0.0".into();
    assert_eq!(dialog.problem(), None);
}

/// A tag this release cannot run, one of a registry that no longer answers,
/// one already in the library and one whose record could not be read are
/// never pulled alone: the pane's Pull… is disabled with the reason, and an
/// opened dialog refuses to start.
#[test]
fn a_tag_that_cannot_be_pulled_says_why() {
    let mut future = record(mt::FIRMWARE, "Future BIOS", &[]);
    future.min_app_version = Some("99.0.0".into());
    let needs = row("unemu/firmware/future", "1", future, not_pulled());
    let reason = refusal(&pick("unemu/firmware/future", &needs)).unwrap();
    assert_eq!(
        reason,
        "Needs unemupc 99.0.0 or newer: Pull refuses it before fetching anything."
    );
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("unemu/firmware/future", &needs)],
        Vec::new(),
        &[],
    )
    .unwrap();
    assert_eq!(dialog.problem().as_deref(), Some(reason.as_str()));
    assert!(dialog.start().is_empty(), "nothing is asked of the worker");

    let ready = row(
        "private/games/a",
        "1.0.0",
        record(mt::MACHINE, "A", &[]),
        not_pulled(),
    );
    let stale = Pick {
        blocked: Some(format!(
            "{LAN} does not answer; Pull waits until Try again reaches it."
        )),
        ..pick("private/games/a", &ready)
    };
    assert_eq!(
        refusal(&stale).as_deref(),
        Some("registry.lan.example does not answer; Pull waits until Try again reaches it.")
    );
    let kept = row(
        "private/games/a",
        "1.0.0",
        record(mt::MACHINE, "A", &[]),
        LocalState::InLibrary,
    );
    assert_eq!(
        refusal(&pick("private/games/a", &kept)).as_deref(),
        Some("Already in the library under this name; Pull has nothing to do.")
    );
    let unread = TagRow {
        record: Err("no OCI image manifest under this tag".into()),
        local: None,
        ..ready.clone()
    };
    assert_eq!(
        refusal(&pick("private/games/a", &unread)).as_deref(),
        Some("The tag could not be read: no OCI image manifest under this tag")
    );
    assert_eq!(refusal(&pick("private/games/a", &ready)), None);
}

/// Several tags pull one after another, in the order chosen, under one
/// progress bar; the ones Pull cannot take are left out with the reason. A
/// blob the tags share, or the store holds, counts once or not at all.
/// Cancel stops the running pull and the waiting one; each answers, and the
/// summary says what was added.
#[test]
fn several_tags_pull_in_order_under_one_progress_bar_and_skip_with_reasons() {
    let shared = blob(mt::FIRMWARE, "shared rom", 8192);
    let held = blob(mt::FIRMWARE, "held rom", 4096);
    let mut first = row(
        "private/games/a",
        "1.0.0",
        record(mt::MACHINE, "A", &[shared.clone(), held.clone()]),
        not_pulled(),
    );
    first.stored.insert(held.digest.clone());
    let second = row(
        "private/games/b",
        "1.0.0",
        record(mt::MACHINE, "B", std::slice::from_ref(&shared)),
        not_pulled(),
    );
    let kept = row(
        "private/games/c",
        "1.0.0",
        record(mt::MACHINE, "C", &[]),
        LocalState::InLibrary,
    );
    let clash = row(
        "private/games/d",
        "1.0.0",
        record(mt::MACHINE, "D", &[]),
        LocalState::OtherImage {
            current: Digest::of(b"another"),
        },
    );
    let mut future = record(mt::MACHINE, "E", &[]);
    future.min_app_version = Some("99.0.0".into());
    let future = row("private/games/e", "2.0.0", future, not_pulled());
    let picks = [
        pick("private/games/a", &first),
        pick("private/games/c", &kept),
        pick("private/games/d", &clash),
        pick("private/games/e", &future),
        pick("private/games/b", &second),
    ];
    let mut dialog = RemotePull::open(accounts(), Mode::Pull, &picks, Vec::new(), &[]).unwrap();
    assert!(dialog.several);
    assert!(
        RemotePull::open(accounts(), Mode::CreateMachine, &picks, Vec::new(), &[]).is_none(),
        "only Pull… takes several tags"
    );
    let skipped: Vec<Option<&str>> = dialog.items.iter().map(|i| i.skip.as_deref()).collect();
    assert_eq!(
        skipped,
        [
            None,
            Some("already in the library"),
            Some("its local tag names another image; pull it alone, under another tag"),
            Some("needs unemupc 99.0.0 or newer"),
            None
        ]
    );
    assert_eq!(dialog.detail(1), "Skipped: already in the library.");
    assert_eq!(dialog.go_label(), "Pull 2 tags");
    // Two manifests, two configs and the shared ROM once; the held ROM not.
    assert_eq!(dialog.download(), 2 * 1024 + 2 * 2048 + 8192);
    assert_eq!(dialog.problem(), None);
    assert_eq!(dialog.progress(), None);

    let jobs = pull_jobs(dialog.start());
    let order: Vec<&str> = jobs.iter().map(|j| j.source.repository.as_str()).collect();
    assert_eq!(order, ["private/games/a", "private/games/b"]);
    assert_eq!(dialog.stage(0), Stage::Pulling);
    assert_eq!(dialog.stage(1), Stage::Skipped);
    assert_eq!(dialog.stage(4), Stage::Waiting);
    assert_eq!(
        dialog.detail(4),
        "Starts when the pull before it has answered."
    );
    // The one bar counts the blob the store holds as done from the start.
    let (fraction, label) = dialog.progress().unwrap();
    let size = dialog.items[0].size;
    // The label counts the bytes to fetch: the manifest, the config and the
    // shared ROM; the held ROM is in the store.
    assert_eq!(
        label,
        "Tag 1 of 2 · private/games/a:1.0.0 · 0 B of 11.0 KiB to fetch"
    );
    assert_eq!(dialog.stage_label(0), "Pulling 0%");
    assert!(
        (fraction - 4096.0 / size as f32 / 2.0).abs() < 1e-6,
        "{fraction}"
    );

    dialog.on_event(&pulled(jobs[0].id, &jobs[0].local_tag));
    assert_eq!(dialog.stage(0), Stage::Pulled);
    assert_eq!(dialog.stage(4), Stage::Pulling);
    let (fraction, label) = dialog.progress().unwrap();
    assert!(
        label.starts_with("Tag 2 of 2 · private/games/b:1.0.0"),
        "{label}"
    );
    assert!((0.5..1.0).contains(&fraction), "{fraction}");

    dialog.cancel();
    assert!(jobs[1].cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(dialog.stage(4), Stage::Stopping);
    assert!(dialog.cancelling());
    dialog.on_event(&TransferEvent::Pulled {
        id: jobs[1].id,
        result: Err(PullFailure::Cancelled),
    });
    assert!(dialog.finished());
    assert_eq!(dialog.stage(4), Stage::Cancelled);
    assert_eq!(dialog.progress().unwrap().1, "Cancelled — 1 of 2 pulled");
    let summary = dialog.summary().unwrap();
    assert_eq!(summary.heading, "Pulled 1 of 2 tags.");
    assert!(
        summary
            .text
            .starts_with(&format!("In the library as {LAN}/private/games/a:1.0.0.")),
        "{}",
        summary.text
    );
    assert_eq!(dialog.pulled(), [format!("{LAN}/private/games/a:1.0.0")]);
}

/// Cancel before the first pull answers asks every pull to stop: the
/// running one at its next read, the waiting ones before they start.
#[test]
fn cancel_reaches_the_waiting_pulls_too() {
    let a = row(
        "private/a",
        "1",
        record(mt::FIRMWARE, "A", &[]),
        not_pulled(),
    );
    let b = row(
        "private/b",
        "1",
        record(mt::FIRMWARE, "B", &[]),
        not_pulled(),
    );
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("private/a", &a), pick("private/b", &b)],
        Vec::new(),
        &[],
    )
    .unwrap();
    let jobs = pull_jobs(dialog.start());
    dialog.cancel();
    assert!(jobs
        .iter()
        .all(|job| job.cancel.load(std::sync::atomic::Ordering::SeqCst)));
    dialog.on_event(&TransferEvent::Pulled {
        id: jobs[0].id,
        result: Err(PullFailure::Cancelled),
    });
    dialog.on_event(&TransferEvent::Pulled {
        id: jobs[1].id,
        result: Err(PullFailure::NotStarted),
    });
    assert_eq!(dialog.stage(1), Stage::NotStarted);
    assert_eq!(
        dialog.detail(1),
        "Cancelled before it started; nothing was fetched."
    );
    let summary = dialog.summary().unwrap();
    assert_eq!(summary.heading, "Cancelled.");
    assert!(summary
        .text
        .starts_with("The running pull stopped at its next read"));
    // A worker that is gone fails what still waits.
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("private/a", &a)],
        Vec::new(),
        &[],
    )
    .unwrap();
    dialog.start();
    dialog.fail("Image store worker stopped");
    assert_eq!(dialog.stage(0), Stage::Failed);
    assert_eq!(dialog.summary().unwrap().heading, "Nothing was pulled.");
}

/// Pull and insert… is offered only with a stopped machine's drive that
/// takes every disk of the set; once the set is in, its first disk waits
/// in the chosen drive for the machine's next Start, once.
#[test]
fn pull_and_insert_leaves_the_first_disk_in_a_drive_that_takes_the_set() {
    let geometry = |name: &str| {
        assembly::descriptors::FLOPPY_TYPES
            .iter()
            .find(|f| f.name == name)
            .unwrap()
            .geometry
    };
    let drive = |machine: &str, unit, format: &str| {
        pc_drive(
            machine.to_lowercase(),
            machine.into(),
            unit,
            geometry(format),
            None,
        )
    };
    let drives = [
        drive("Games", 0, "1.2M"),
        drive("Games", 1, "1.44M"),
        drive("Office", 0, "360K"),
    ];
    let formats = |f: &[&str]| f.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    // A 1.44M drive takes 720K disks, a 1.2M one 360K disks.
    let fitting: Vec<String> = drives_for(&formats(&["720K"]), &drives)
        .into_iter()
        .map(Drive::label)
        .collect();
    assert_eq!(fitting, ["Games — B: 1.44M"]);
    let fitting: Vec<String> = drives_for(&formats(&["360K"]), &drives)
        .into_iter()
        .map(Drive::label)
        .collect();
    assert_eq!(fitting, ["Games — A: 1.2M", "Office — A: 360K"]);
    assert!(drives_for(&formats(&["720K", "1.2M"]), &drives).is_empty());

    let mut set = record(mt::MEDIA_SET, "Monkey Island", &[]);
    set.media = vec![
        ("disk-1".into(), "720K".into()),
        ("disk-2".into(), "720K".into()),
    ];
    let tag = row(
        "private/media/monkey-island",
        "1.1-vga-720k",
        set.clone(),
        not_pulled(),
    );
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Insert,
        &[pick("private/media/monkey-island", &tag)],
        Vec::new(),
        &drives,
    )
    .unwrap();
    assert_eq!(dialog.go_label(), "Pull and insert");
    assert_eq!(dialog.drives.len(), 1);
    assert_eq!(dialog.problem(), None);
    assert!(dialog.take_insert().is_none(), "not before the pull");
    let jobs = pull_jobs(dialog.start());
    dialog.on_event(&pulled(jobs[0].id, &jobs[0].local_tag));
    let (machine, insert) = dialog.take_insert().unwrap();
    assert_eq!(machine, "games");
    assert_eq!(
        insert,
        PendingInsert {
            drive: "B:".into(),
            set: Descriptor::new(mt::MANIFEST, set.digest.clone(), 1024),
            reference: format!("{LAN}/private/media/monkey-island:1.1-vga-720k"),
            medium: "disk-1".into(),
            all: true,
            journal: true,
        }
    );
    assert!(dialog.take_insert().is_none(), "once");
    assert!(dialog
        .summary()
        .unwrap()
        .text
        .ends_with("Disk 1 waits in Games’s B: drive until its next Start."));

    // Without a drive that takes the set, Pull and insert… cannot start.
    let dialog = RemotePull::open(
        accounts(),
        Mode::Insert,
        &[pick("private/media/monkey-island", &tag)],
        Vec::new(),
        &drives[..1],
    )
    .unwrap();
    assert_eq!(
        dialog.problem().as_deref(),
        Some("Pull and insert… needs a stopped machine with a drive that takes 720K disks.")
    );
}

/// Pull and create machine… hands New machine from image… the pulled image
/// and the manifest's title as the name, once the pull answered.
#[test]
fn pull_and_create_machine_hands_over_the_pulled_image() {
    let game = record(mt::MACHINE, "Dune II", &[]);
    let tag = row("private/games/dune2", "1.0.0", game.clone(), not_pulled());
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::CreateMachine,
        &[pick("private/games/dune2", &tag)],
        Vec::new(),
        &[],
    )
    .unwrap();
    assert_eq!(dialog.go_label(), "Pull and create machine");
    assert_eq!(dialog.created_image(), None);
    let jobs = pull_jobs(dialog.start());
    dialog.on_event(&pulled(jobs[0].id, &jobs[0].local_tag));
    assert_eq!(
        dialog.created_image(),
        Some((
            format!("{LAN}/private/games/dune2:1.0.0"),
            Descriptor::new(mt::MANIFEST, game.digest, 1024),
            "Dune II".into()
        ))
    );
    let summary = dialog.summary().unwrap();
    assert_eq!(summary.heading, "Pulled.");
    assert_eq!(
        summary.text,
        format!("{LAN}/private/games/dune2:1.0.0 is in the library as {LAN}/private/games/dune2:1.0.0. The tag was written after every blob was verified.")
    );
}

/// The dialog draws every stage without a window: the question of one tag
/// with its local tag, a clash, the question of several with the reasons of
/// those left out, the one progress bar, Cancel and Escape, and the end with
/// its follow-up button.
#[test]
fn the_dialog_renders_every_stage() {
    use crate::registry_transfer_view::{
        show_remote_pull,
        tests::{click, escape, render, shows},
        Action,
    };
    let ctx = egui::Context::default();
    let draw = |dialog: &mut RemotePull, events| {
        render(&ctx, events, |ctx| show_remote_pull(ctx, dialog, true))
    };
    let game = record(
        mt::MACHINE,
        "Dune II",
        &[blob(mt::FIRMWARE, "rom", 3 << 20)],
    );
    let tag = row("private/games/dune2", "1.0.0", game.clone(), not_pulled());
    let local = format!("{LAN}/private/games/dune2:1.0.0");
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::CreateMachine,
        &[pick("private/games/dune2", &tag)],
        vec![("taken:1.0.0".into(), Digest::of(b"another"))],
        &[],
    )
    .unwrap();
    let (_, shown) = draw(&mut dialog, vec![]);
    for wanted in [
        "Pull and create machine — private/games/dune2:1.0.0",
        &format!("{LAN}/private/games/dune2:1.0.0"),
        "· pulled by this digest, as listed",
        "From this registry the name carries its host, so the same path on another registry, or made here, never clashes. Pull moves no local tag, except that a set or package pulled by its release writes the revision and moves its local release alias forward.",
        "Then New machine from image… asks for the machine's name; the default is “Dune II”.",
        "Download: 3.0 MiB of 3.0 MiB · 0 of 2 blobs already in the store",
        "Pull and create machine",
        "Cancel",
    ] {
        assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
    }
    dialog.items[0].local_tag = "taken:1.0.0".into();
    let (_, shown) = draw(&mut dialog, vec![]);
    assert!(
        shows(&shown, "taken:1.0.0 already names another image")
            && shows(
                &shown,
                "Pull moves no local tag but a release alias: choose another one."
            ),
        "{shown:?}"
    );
    dialog.items[0].local_tag = local.clone();
    let action = click(&ctx, "Pull and create machine", |ctx| {
        show_remote_pull(ctx, &mut dialog, true)
    });
    let Action::SendAll(requests) = action else {
        panic!("the button starts the pull")
    };
    let jobs = pull_jobs(requests);
    let mut receiving = blob(mt::FIRMWARE, "rom", 3 << 20);
    receiving.size = 3 << 20;
    jobs[0]
        .live
        .update(|state| state.receiving.push((receiving, 1 << 20)));
    let (_, shown) = draw(&mut dialog, vec![]);
    for wanted in [
        "From registry.lan.example/private/games/dune2 by sha256:",
        "Fetching 1.0 MiB of 3.0 MiB",
        "Pulling",
        "0 blobs verified (0 B) · 1.0 MiB arriving: firmware",
        "Cancel or Escape stops at the next read.",
    ] {
        assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
    }
    let (action, _) = draw(&mut dialog, escape());
    assert!(
        matches!(action, Action::None),
        "the dialog waits for the answer"
    );
    let (_, shown) = draw(&mut dialog, vec![]);
    for wanted in [
        "Cancelling — stopping at the next read…",
        "Stopping…",
        "Cancelling…",
    ] {
        assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
    }
    dialog.on_event(&pulled(jobs[0].id, &local));
    let (_, shown) = draw(&mut dialog, vec![]);
    for wanted in ["Pulled.", "Pulled ✓", "New machine from image…", "Close"] {
        assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
    }
    let action = click(&ctx, "New machine from image…", |ctx| {
        show_remote_pull(ctx, &mut dialog, true)
    });
    assert!(matches!(action, Action::NewMachine));

    // Several tags: the ones left out say why, and the button counts the rest.
    let other = row(
        "private/games/b",
        "1.0.0",
        record(mt::MACHINE, "B", &[]),
        not_pulled(),
    );
    let kept = row(
        "private/games/c",
        "1.0.0",
        record(mt::MACHINE, "C", &[]),
        LocalState::InLibrary,
    );
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[
            pick("private/games/dune2", &tag),
            pick("private/games/b", &other),
            pick("private/games/c", &kept),
        ],
        Vec::new(),
        &[],
    )
    .unwrap();
    let (_, shown) = draw(&mut dialog, vec![]);
    for wanted in [
        "Pull 2 tags",
        "One after another, each by the digest the list showed, under one progress bar.",
        "Skipped: already in the library.",
        "Skipped",
        "Download: ",
    ] {
        assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
    }
    // The title and the button say the same; the button's click is the
    // single tag's above.
    let jobs = pull_jobs(dialog.start());
    assert_eq!(jobs.len(), 2);
    let (_, shown) = draw(&mut dialog, vec![]);
    assert!(
        shows(
            &shown,
            "Tag 1 of 2 · private/games/dune2:1.0.0 · 0 B of 3.0 MiB"
        ) && shows(&shown, "Waiting"),
        "{shown:?}"
    );
    for job in &jobs {
        dialog.on_event(&pulled(job.id, &job.local_tag));
    }
    let (_, shown) = draw(&mut dialog, vec![]);
    for wanted in ["Done — 2 of 2 pulled", "Pulled 2 tags.", "Close"] {
        assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
    }
    let (action, _) = draw(&mut dialog, escape());
    assert!(matches!(action, Action::Close));
}

/// The mock-up's details of the dialog: a typed local tag that already
/// names this image makes Pull say Done and pull nothing; a drive that holds
/// a disk for the next Start says what the pull replaces; a CD set is never
/// offered a floppy drive; a tag not read yet is left out with the reason.
#[test]
fn done_replaced_disks_cd_sets_and_unread_tags_say_so() {
    let game = record(mt::MACHINE, "Dune II", &[]);
    let tag = row("private/games/dune2", "1.0.0", game.clone(), not_pulled());
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("private/games/dune2", &tag)],
        vec![("dune2-copy:1.0.0".into(), game.digest.clone())],
        &[],
    )
    .unwrap();
    assert!(!dialog.unchanged());
    dialog.items[0].local_tag = "dune2-copy:1.0.0".into();
    assert!(dialog.unchanged());
    assert_eq!(dialog.go_label(), "Done");

    let drive = pc_drive(
        "games".into(),
        "Games".into(),
        1,
        assembly::descriptors::FLOPPY_TYPES[0].geometry,
        Some("FreeDOS — Disk 1".into()),
    );
    assert_eq!(
        drive.label(),
        "Games — B: 1.44M — replaces FreeDOS — Disk 1"
    );

    let mut cd = record(mt::MEDIA_SET, "Windows 95", &[]);
    cd.media = vec![("cd".into(), "iso-2048".into())];
    let cd = row("private/media/win95", "1", cd, not_pulled());
    let dialog = RemotePull::open(
        accounts(),
        Mode::Insert,
        &[pick("private/media/win95", &cd)],
        Vec::new(),
        std::slice::from_ref(&drive),
    )
    .unwrap();
    assert_eq!(
        dialog.problem().as_deref(),
        Some("Pull and insert… takes floppy sets only: no machine here takes CD images yet.")
    );

    let unread = Pick {
        host: LAN,
        repository: "private/games/dune2",
        tag: "0.9.0",
        row: None,
        blocked: None,
        facts: facts(),
    };
    assert_eq!(
        refusal(&unread).as_deref(),
        Some("The tag is not read yet; its row says when it is.")
    );
    let dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("private/games/dune2", &tag), unread],
        Vec::new(),
        &[],
    )
    .unwrap();
    assert_eq!(dialog.items[1].skip.as_deref(), Some("it is not read yet"));
    assert_eq!(
        dialog.items[1].local_tag,
        format!("{LAN}/private/games/dune2:0.9.0")
    );
    assert_eq!(dialog.go_label(), "Pull 1 tag");
}

/// A proposed local tag in place of one that names another image has the
/// keyboard when the dialog opens; Done closes the dialog without a pull.
#[test]
fn the_proposed_tag_has_the_keyboard_and_done_closes() {
    use crate::registry_transfer_view::{
        show_remote_pull,
        tests::{click, render},
        Action,
    };
    let ctx = egui::Context::default();
    let game = record(mt::MACHINE, "Dune II", &[]);
    let other = Digest::of(b"pulled before the registry changed");
    let clash = row(
        "private/games/dune2",
        "1.0.0",
        game.clone(),
        LocalState::OtherImage {
            current: other.clone(),
        },
    );
    let mut dialog = RemotePull::open(
        accounts(),
        Mode::Pull,
        &[pick("private/games/dune2", &clash)],
        vec![
            (format!("{LAN}/private/games/dune2:1.0.0"), other),
            ("dune2-copy:1.0.0".into(), game.digest.clone()),
        ],
        &[],
    )
    .unwrap();
    assert!(dialog.focus_tag);
    render(&ctx, Vec::new(), |ctx| {
        show_remote_pull(ctx, &mut dialog, true)
    });
    assert!(!dialog.focus_tag, "once");
    assert!(
        ctx.memory(|m| m.focused()).is_some(),
        "the local tag field has the keyboard"
    );
    dialog.items[0].local_tag = "dune2-copy:1.0.0".into();
    let action = click(&ctx, "Done", |ctx| show_remote_pull(ctx, &mut dialog, true));
    assert!(matches!(action, Action::Close));
    assert!(!dialog.started(), "nothing was pulled");
}
