// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Push… and Pull…'s model is `library_model::registry_transfer`,
//! re-exported here under its old path. Its tests live with the library
//! frame in `pc_ui`; the ones here run a transfer with the program's
//! provider list, and drive the frame's lab.

pub(crate) use library_model::registry_transfer::*;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        library_test_support::Fixture, library_worker::Request, pc_desktop::PcDesktop,
        registries_model::Mark,
    };
    use image_registry::{
        credential_store::SessionStore,
        guard::{MarkSource, Redistribution},
        test_support::{MockRegistry, Policy},
        Reference,
    };
    use image_store::{Digest, Store};
    use library_model::registry_transfer::test_support::{NO_OVERRIDE, PUSH_CANCELLED};
    use std::sync::{Arc, Mutex};

    // The lab and the helpers these tests share with the library frame's
    // tests live with the frame.
    pub(crate) use pc_ui::registry_transfer::test_support::{accounts_for, Lab};
    use pc_ui::registry_transfer::test_support::{login_policy, open_push, push_settled};

    /// Run a request on this thread as the worker would, with the synthetic
    /// firmware rules and `catalogue`, and return its events.
    fn run_here(
        store: &Store,
        request: TransferRequest,
        catalogue: &[assembly::FirmwareEntry],
    ) -> Vec<TransferEvent> {
        let events = Mutex::new(Vec::new());
        pc_image::test_support::with_synthetic_firmware(|| {
            platforms::with_image_providers(|providers| {
                let pc = PcDesktop::new(providers, catalogue);
                run(
                    store,
                    request,
                    &[&pc],
                    &|event| events.lock().unwrap().push(event),
                    &|| {},
                )
            })
        });
        events.into_inner().unwrap()
    }

    #[test]
    fn the_guard_refuses_without_override_and_a_forged_job_reaches_no_registry() {
        let mut lab = Lab::new(false);
        let host = lab.host();
        let image = lab.f.row.descriptor.clone();
        let dialog = lab.open_push("test:source");
        for (destination, refusal, line) in [
            (
                format!("{host}/private/test:1"),
                "push it only to a private registry",
                "the target is not marked private",
            ),
            (
                format!("{host}/unemu/test:1"),
                "never to unemu/",
                "unemu/ accepts only open blobs",
            ),
            (
                "other.example/private/test:1".to_owned(),
                "not in your registry list",
                "is not in your registry list",
            ),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(evaluation.refused_by_guard, "{destination}");
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(refusal),
                "{destination}: {evaluation:?}"
            );
            assert!(
                evaluation
                    .checks
                    .iter()
                    .any(|c| c.mark == Mark::Bad && c.text.contains(line)),
                "{destination}: {:?}",
                evaluation.checks
            );
            assert!(!dialog.can_push());
            assert!(dialog.push().is_none());
        }
        dialog.destination = format!("{host}/private/test:1");
        assert!(dialog
            .evaluate()
            .refusal
            .unwrap()
            .contains("contains user-supplied firmware"));
        for (destination, problem) in [
            (format!("{host}/private/test:1.0.0+b1"), "tag grammar"),
            (
                format!("{host}/private/test@{}", image.digest),
                "needs a tag",
            ),
            (String::new(), "Choose a registry and a namespace"),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(!evaluation.refused_by_guard && !evaluation.allowed());
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(problem),
                "{destination}: {evaluation:?}"
            );
            assert!(dialog.push().is_none());
        }

        // A job built past the dialog is refused by the worker before any request.
        let forged = TransferRequest::Push(Box::new(PushJob {
            id: next_id(),
            local_tag: "test:source".into(),
            image: image.digest.clone(),
            target: Reference::parse(&format!("{host}/private/test:1"), "unused.example").unwrap(),
            move_from: None,
            accounts: lab.accounts.clone(),
            cancel: Arc::default(),
            live: Live::default(),
        }));
        let events = run_here(&lab.f.store, forged, &[]);
        assert!(
            matches!(events.as_slice(), [TransferEvent::Pushed { result: Err(PushFailure::Refused(text)), .. }]
                if text.contains("private registry")),
            "{:?}",
            events.len()
        );
        assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());

        // A registry marked private that answers anonymous reads is refused at
        // push time, before any blob goes up.
        let leaky = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..login_policy()
        });
        let accounts = accounts_for(
            lab.f.temp.path(),
            &leaky.host(),
            true,
            Arc::new(SessionStore::default()),
        );
        let dialog = open_push(&mut lab.view, &accounts, &lab.f.store, "test:source");
        dialog.destination = format!("{}/private/test:1", leaky.host());
        let request = dialog
            .push()
            .expect("the dry run allows a private registry");
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let PushState::Finished { ok, message, .. } = &lab.view.push.as_ref().unwrap().state else {
            panic!()
        };
        assert!(!ok);
        assert!(
            message.starts_with("Push refused.") && message.contains("read anonymously"),
            "{message}"
        );
        assert!(message.ends_with(NO_OVERRIDE));
        assert!(!leaky
            .log()
            .iter()
            .any(|l| l.starts_with("POST ") || l.starts_with("PUT ")));
    }

    /// List the lab registry with a public `unemu/` and a private `private/`,
    /// as the maintainer's one host serves them, keeping its login.
    fn namespaced(lab: &Lab) {
        let host = lab.host();
        std::fs::write(
            lab.accounts.path.as_ref().unwrap(),
            format!(
                "[[registries]]\nhost = \"{host}\"\nuser = \"ci\"\nplain_http = true\nnamespaces = [{{ name = \"unemu\", private = false }}, {{ name = \"private\", private = true }}]\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn push_and_pull_name_the_namespace_and_the_guard_decides_per_namespace() {
        let mut lab = Lab::new(false);
        namespaced(&lab);
        let host = lab.host();
        let image = lab.f.row.descriptor.clone();
        let dialog = lab.open_push("test:source");
        for (destination, refusal, mark, line) in [
            (
                format!("{host}/unemu/test:1"),
                "never to unemu/".to_owned(),
                Mark::Good,
                format!("{host} is configured; unemu/ is public"),
            ),
            (
                format!("{host}/owner/test:1"),
                format!("{host}/owner/ is not a configured namespace on this registry."),
                Mark::Bad,
                format!(
                    "{host} is configured; owner/ is not a configured namespace on this registry"
                ),
            ),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(evaluation.refused_by_guard, "{destination}");
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(&refusal),
                "{destination}: {evaluation:?}"
            );
            assert!(
                evaluation
                    .checks
                    .iter()
                    .any(|c| c.mark == mark && c.text == line),
                "{destination}: {:?}",
                evaluation.checks
            );
            assert!(dialog.push().is_none());
        }
        dialog.destination = format!("{host}/private/test:1.0.0");
        let evaluation = dialog.evaluate();
        assert!(evaluation.allowed(), "{evaluation:?}");
        assert_eq!(
            evaluation.checks[0].text,
            format!("{host} is configured; private/ is marked private")
        );
        assert!(evaluation
            .checks
            .iter()
            .any(|c| c.mark == Mark::Good && c.text.contains("may go here")));
        let request = dialog.push().unwrap();
        lab.view.send(Request::Transfer(request));
        lab.view.pump_until(push_settled);
        let state = &lab.view.push.as_ref().unwrap().state;
        assert!(
            matches!(state, PushState::Finished { ok: true, .. }),
            "{state:?}"
        );
        assert_eq!(
            lab.registry.tag_digest("private/test", "1.0.0"),
            Some(image.digest.clone())
        );

        // A job built past the dialog for an unlisted namespace reaches no registry.
        lab.registry.clear_log();
        let forged = TransferRequest::Push(Box::new(PushJob {
            id: next_id(),
            local_tag: "test:source".into(),
            image: image.digest.clone(),
            target: Reference::parse(&format!("{host}/owner/test:1"), "unused.example").unwrap(),
            move_from: None,
            accounts: lab.accounts.clone(),
            cancel: Arc::default(),
            live: Live::default(),
        }));
        let events = run_here(&lab.f.store, forged, &[]);
        assert!(
            matches!(events.as_slice(), [TransferEvent::Pushed { result: Err(PushFailure::Refused(text)), .. }]
                if text.contains("is not a configured namespace on this registry")),
            "{:?}",
            events.len()
        );
        assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());

        // Pull… names the namespace the same way, and nothing for an unlisted host.
        let mut pull = PullDialog::open(lab.accounts.clone(), PcDesktop::facts());
        pull.reference = format!("{host}/private/test:1.0.0");
        assert_eq!(
            pull.namespace_line().as_deref(),
            Some(format!("{host} is configured; private/ is marked private").as_str())
        );
        pull.reference = "other.example/x/y:1".into();
        assert_eq!(pull.namespace_line(), None);
    }

    /// A firmware package that marks its ROM open under MIT, as one made
    /// where the catalogue does not know the ROM and pulled from elsewhere
    /// would. This catalogue lists the same bytes as one chip of a
    /// proprietary BIOS, so the guard keeps the package private whatever its
    /// marks say.
    #[test]
    fn a_package_marking_catalogued_proprietary_firmware_open_cannot_leave() {
        let mut lab = Lab::new(false);
        let host = lab.host();
        let rom = lab.f.temp.path().join("rom.bin");
        std::fs::write(&rom, [0x31; 16384]).unwrap();
        let candidate =
            pc_image::firmware_import::unlisted(&rom, "foreign-rom", "option-rom", &[]).unwrap();
        let statement = pc_image::firmware_import::Statement {
            open: true,
            license: Some("MIT".into()),
            ..Default::default()
        };
        pc_image::firmware_import::publish_with(&lab.f.store, &candidate, &[], &statement, &[])
            .unwrap();
        let tag = "private/firmware/ibm-pc/foreign-rom:r1";
        let catalogue = crate::library_view::firmware::tests::synthetic_catalogue();
        assert!(pc_image::firmware::proprietary(
            catalogue,
            Digest::of(&[0x31; 16384]).hex(),
            16384
        )
        .is_some());

        let dialog = lab.open_push(tag);
        let rows = dialog.dry_run();
        let firmware = rows
            .iter()
            .find(|r| r.kind.starts_with("firmware") && !r.kind.contains("config"))
            .unwrap();
        assert_eq!(
            (firmware.class, firmware.source.as_str()),
            ("user-supplied", "firmware catalogue"),
            "{rows:#?}"
        );
        for (destination, refusal) in [
            (
                format!("{host}/owner/rom:2"),
                "push it only to a private registry",
            ),
            (format!("{host}/unemu/rom:r1"), "never to unemu/"),
        ] {
            dialog.destination = destination.clone();
            let evaluation = dialog.evaluate();
            assert!(evaluation.refused_by_guard, "{destination}");
            assert!(
                evaluation.refusal.as_deref().unwrap().contains(refusal),
                "{destination}: {evaluation:?}"
            );
            assert!(evaluation.checks.iter().any(|c| c
                .text
                .contains("firmware the catalogue lists as proprietary")));
        }

        // The worker refuses a job built past the dialog the same way.
        let image = lab.f.store.tag_descriptor(tag).unwrap();
        let job = |target: &str| {
            TransferRequest::Push(Box::new(PushJob {
                id: next_id(),
                local_tag: tag.into(),
                image: image.digest.clone(),
                target: Reference::parse(&format!("{host}/{target}"), "unused.example").unwrap(),
                move_from: None,
                accounts: lab.accounts.clone(),
                cancel: Arc::default(),
                live: Live::default(),
            }))
        };
        for target in ["owner/rom:r2", "unemu/rom:r1"] {
            let events = run_here(&lab.f.store, job(target), catalogue);
            assert!(
                matches!(events.as_slice(), [TransferEvent::Pushed { result: Err(PushFailure::Refused(text)), .. }]
                    if text.contains("user-supplied firmware")),
                "{target}"
            );
        }
        assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());

        // Without the catalogue's knowledge the package reads as open.
        let events = run_here(
            &lab.f.store,
            TransferRequest::Plan {
                id: next_id(),
                local_tag: Some(tag.into()),
            },
            &[],
        );
        let [TransferEvent::Planned {
            result: Ok(planned),
            ..
        }] = events.as_slice()
        else {
            panic!()
        };
        assert!(planned
            .plan
            .as_ref()
            .unwrap()
            .closure
            .iter()
            .all(|c| c.effective == Redistribution::Open && c.source == MarkSource::ThisImage));
    }

    #[test]
    fn a_move_or_stale_answer_after_cancel_shows_the_cancel() {
        let f = Fixture::new();
        for failure in [
            PushFailure::Move {
                tag: "latest".into(),
                current: Digest::of(b"old"),
                new: f.row.descriptor.digest.clone(),
            },
            PushFailure::Stale,
        ] {
            let accounts = accounts_for(
                f.temp.path(),
                "127.0.0.1:5999",
                true,
                Arc::new(SessionStore::default()),
            );
            let (mut dialog, request) =
                PushDialog::open(accounts, PcDesktop::facts(), Some("test:source".into()));
            for event in run_here(&f.store, request, &[]) {
                dialog.on_event(&event);
            }
            dialog.destination = "127.0.0.1:5999/private/test:latest".into();
            let Some(TransferRequest::Push(job)) = dialog.push() else {
                panic!()
            };
            dialog.cancel();
            let follow_up = dialog.on_event(&TransferEvent::Pushed {
                id: job.id,
                result: Err(failure),
            });
            assert!(follow_up.is_none());
            assert!(
                matches!(&dialog.state, PushState::Finished { ok: false, message, .. } if message == PUSH_CANCELLED),
                "{:?}",
                dialog.state
            );
        }
    }
}
