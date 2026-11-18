// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Push… and Pull…'s model is `library_model::registry_transfer`,
//! re-exported here under the path the library frame names it by. The tests
//! that drive it through the library frame, its worker and the PC's images
//! are here; the few that run a transfer with the program's provider list
//! stay with the application, and drive the lab in `test_support`, behind
//! the `test-support` feature.

pub use library_model::registry_transfer::*;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    //! A library frame on a synthetic worker beside a mock registry that
    //! demands a login, as the push and pull tests here and the
    //! application's tests drive them.

    use super::*;
    use crate::{
        library_view::{ImageRow, LibraryView},
        library_worker::Request,
    };
    use image_registry::{
        credential_store::{CredentialStore, Secret, SessionStore},
        credentials::EnvCredentials,
        test_support::{MockRegistry, Policy},
    };
    use image_store::Store;
    use pc_library::{library_test_support::Fixture, test_wait::HANG_GUARD};
    use std::{
        path::Path,
        sync::{atomic::AtomicBool, Arc},
        time::{Duration, Instant},
    };

    pub const SECRET: &str = "synthetic-secret";
    pub const FIRST_BLOB_HEAD: &str = "HEAD /v2/private/test/blobs/";

    /// A registry that demands the Basic login `ci` with [`SECRET`].
    pub fn login_policy() -> Policy {
        Policy {
            user: Some(("ci".into(), SECRET.into())),
            ..Policy::default()
        }
    }

    /// Preferences that list `host` with the login `ci`, whose secret is in `store`.
    pub fn accounts_for(
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
    pub struct Lab {
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

        pub fn host(&self) -> String {
            self.registry.host()
        }

        pub fn open_push(&mut self, tag: &str) -> &mut PushDialog {
            open_push(&mut self.view, &self.accounts, &self.f.store, tag)
        }

        /// Push `tag` to `destination` through the dialog; the guard must allow it.
        pub fn push_through(&mut self, tag: &str, destination: &str) {
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

    pub fn open_push<'a>(
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

    pub fn push_settled(view: &LibraryView) -> bool {
        matches!(
            view.push.as_ref().unwrap().state,
            PushState::Finished { .. } | PushState::ConfirmMove { .. }
        )
    }

    /// Wait for `condition` while applying the worker's queued events, so the
    /// worker never waits on a full queue meanwhile.
    pub fn wait_pumping(view: &mut LibraryView, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + HANG_GUARD;
        while !condition() {
            assert!(Instant::now() < deadline, "the condition was not reached");
            view.pump_queued();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        library_test_support::Fixture, library_view::LibraryView, library_worker::Request,
        registries_model::Mark,
    };
    use image_registry::{
        credential_store::{CredentialStore, Secret, SessionStore, StoreKind},
        push::BlobOutcome,
        test_support::BlobFault,
    };
    use image_store::{BlobSource, Descriptor, Manifest, Store, MAX_SMALL_BLOB};
    use library_model::registry_transfer::test_support::{PULL_CANCELLED, PUSH_CANCELLED, STALE};
    use std::{
        collections::BTreeSet,
        sync::{atomic::Ordering, Arc, Mutex},
        time::{Duration, Instant},
    };

    // The lab and the helpers the application's tests share live in
    // `test_support`; the tests here name them as before.
    use super::test_support::push_settled;
    pub(crate) use super::test_support::{accounts_for, wait_pumping, Lab};

    fn pull_settled(view: &LibraryView) -> bool {
        matches!(
            view.pull.as_ref().unwrap().state,
            PullState::Finished { .. }
        )
    }

    fn manifest_of(store: &Store, image: &Descriptor) -> Manifest {
        Manifest::decode_for_summary(&store.read_verified(image, MAX_SMALL_BLOB).unwrap()).unwrap()
    }

    /// Tag `tag` as a copy of the fixture image whose manifest differs only
    /// in its title, so it has another digest and the same blobs.
    pub(crate) fn retitled(f: &Fixture, tag: &str) -> Descriptor {
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
                file_name: None,
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
    fn the_dry_run_says_a_revision_never_moves_and_an_alias_only_forward() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        installation_set(&lab.f, "sets:1.4-r1", 1);
        let dialog = lab.open_push("sets:1.4-r1");
        dialog.destination = format!("{host}/private/sets:1.4-r1");
        let note = tag_note(dialog);
        assert!(
            note.starts_with("1.4-r1 is a revision tag")
                && note.contains("never moves, not even with confirmation")
                && note.contains("1.4-r2"),
            "{note}"
        );
        dialog.destination = format!("{host}/private/sets:1.4");
        let note = tag_note(dialog);
        assert!(
            note.starts_with("1.4 is the alias of a release")
                && note.contains("moves only forward"),
            "{note}"
        );
        dialog.destination = format!("{host}/private/sets:latest");
        let evaluation = dialog.evaluate();
        assert!(
            evaluation
                .checks
                .iter()
                .any(|c| c.text.contains("have no latest tag")),
            "{evaluation:?}"
        );
        // A machine image's tag of the same shape moves when confirmed.
        let dialog = lab.open_push("test:source");
        dialog.destination = format!("{host}/private/test:1.4-r1");
        let note = tag_note(dialog);
        assert!(
            note.starts_with("1.4-r1 is not a version") && note.contains("you confirm the move"),
            "{note}"
        );
    }

    #[test]
    fn a_set_pushed_onto_a_taken_revision_tag_fails_even_when_confirmed() {
        let mut lab = Lab::new(true);
        let host = lab.host();
        let first = installation_set(&lab.f, "sets:1.4-r1", 1);
        let second = installation_set(&lab.f, "sets:changed-r1", 2);
        lab.push_through("sets:1.4-r1", &format!("{host}/private/sets:1.4-r1"));
        let dialog = lab.open_push("sets:changed-r1");
        dialog.destination = format!("{host}/private/sets:1.4-r1");
        let request = dialog.push().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::Finished { ok, message, .. } = &lab.view.push.as_ref().unwrap().state else {
            panic!("{:?}", lab.view.push.as_ref().unwrap().state)
        };
        assert!(!ok);
        assert!(
            message.contains("Revision tags of installation media sets and firmware packages never move, not even with confirmation")
                && message.contains("such as 1.4-r2")
                && message.contains(&first.digest.to_string())
                && message.contains(&second.digest.to_string()),
            "{message}"
        );
        assert_eq!(
            lab.registry.tag_digest("private/sets", "1.4-r1"),
            Some(first.digest.clone())
        );
        // The alias goes up only after its revision, which the registry
        // checks.
        lab.push_through("sets:1.4-r1", &format!("{host}/private/sets:1.4"));
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
        assert_eq!(
            dialog.resolve().unwrap().1,
            format!("{host}/private/test:1.0.0")
        );
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
        let local = format!("{host}/private/test:1.0.0");
        assert!(matches!(
            store.tag_descriptor(&local),
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
            store.tag_descriptor(&local).unwrap().digest,
            lab.f.row.descriptor.digest
        );
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
