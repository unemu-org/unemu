// SPDX-License-Identifier: AGPL-3.0-or-later
//! Process exits bypass destructors at every durable publication boundary.
//! This binary is separate from in-process journal-lock tests to avoid fork inheritance.
#[path = "support/workspace.rs"]
mod fixture;
use fixture::Fixture;
use pc_image::{workspace, PcProvider};
use std::{fs, process::Command};

#[test]
fn publication_exit_child() {
    let Ok(path) = std::env::var("PUBLICATION_TEST_STORE") else {
        return;
    };
    let store = image_store::Store::open_existing(path).unwrap();
    let owner = store.open_workspace("play").unwrap();
    let stop: usize = std::env::var("PUBLICATION_TEST_STEP")
        .unwrap()
        .parse()
        .unwrap();
    let expected = if std::env::var("PUBLICATION_TEST_CAS").unwrap() == "present" {
        Some(store.tag_descriptor("test:parent").unwrap().digest)
    } else {
        None
    };
    let mut step = 0;
    let result = workspace::publish_with_hook(&owner, "test:target", expected.as_ref(), |_| {
        if step == stop {
            std::process::exit(77);
        }
        step += 1;
        Ok(())
    });
    assert!(result.is_ok(), "{result:?}");
}
#[test]
fn death_at_every_publication_boundary_preserves_layered_parents_and_both_cas_paths() {
    let probe = Fixture::new();
    probe.changed("seed", 70);
    workspace::publish(
        &probe.store.open_workspace("seed").unwrap(),
        "test:parent",
        None,
    )
    .unwrap();
    probe.changed_from("play", "test:parent", 71);
    let owner = probe.store.open_workspace("play").unwrap();
    let mut steps = Vec::new();
    workspace::publish_with_hook(&owner, "test:probe", None, |step| {
        steps.push(step);
        Ok(())
    })
    .unwrap();
    drop(owner);
    drop(probe);
    for present in [false, true] {
        for (stop, step) in steps.iter().enumerate() {
            let fixture = Fixture::new();
            fixture.changed("seed", 70);
            let parent = workspace::publish(
                &fixture.store.open_workspace("seed").unwrap(),
                "test:parent",
                None,
            )
            .unwrap();
            fixture.changed_from("play", "test:parent", 71);
            if present {
                fixture
                    .store
                    .update_tag("test:target", &parent, None, &PcProvider)
                    .unwrap();
            }
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "publication_exit_child", "--nocapture"])
                .env("PUBLICATION_TEST_STORE", fixture.store.root())
                .env("PUBLICATION_TEST_STEP", stop.to_string())
                .env(
                    "PUBLICATION_TEST_CAS",
                    if present { "present" } else { "absent" },
                )
                .output()
                .unwrap();
            assert_eq!(
                status.status.code(),
                Some(77),
                "{}",
                String::from_utf8_lossy(&status.stderr)
            );
            let owner = fixture.store.open_workspace("play").unwrap();
            let target = fixture.store.tag_descriptor("test:target");
            match &target {
                Ok(image) if image.digest == parent.digest => {
                    fixture.assert_complete("test:target", 70)
                }
                Ok(_) => fixture.assert_complete("test:target", 71),
                Err(image_store::Error::TagNotFound(_)) => assert!(!present),
                Err(error) => panic!("{error}"),
            }
            // exit() skips the staging guard; an interrupted pack must remain visible.
            if matches!(step, workspace::PublicationStep::PackStaged(_)) {
                assert!(fs::read_dir(owner.path().join("publication"))
                    .unwrap()
                    .next()
                    .is_some());
            }
            if owner.path().join("publication").exists()
                && fs::read_dir(owner.path().join("publication"))
                    .unwrap()
                    .next()
                    .is_some()
            {
                assert!(
                    owner.path().join("media").exists() || owner.path().join("retired").exists()
                );
            }
            let expected = present.then_some(&parent.digest);
            if owner.read("transaction.toml").unwrap().is_some()
                || !target.as_ref().is_ok_and(|d| d.digest != parent.digest)
            {
                workspace::publish(&owner, "test:target", expected).unwrap();
            }
            fixture.assert_complete("test:target", 71);
            fixture.assert_complete("test:parent", 70);
            fixture
                .store
                .resolve_descriptor(&fixture.image, &PcProvider)
                .unwrap();
        }
    }
    death_then_tag_conflict_rolls_back_intent_and_preserves_journals();
}

fn death_then_tag_conflict_rolls_back_intent_and_preserves_journals() {
    let fixture = Fixture::new();
    fixture.changed("seed", 80);
    let parent = workspace::publish(
        &fixture.store.open_workspace("seed").unwrap(),
        "test:parent",
        None,
    )
    .unwrap();
    fixture.changed_from("probe", "test:parent", 81);
    let probe = fixture.store.open_workspace("probe").unwrap();
    let mut stop = 0;
    assert!(
        workspace::publish_with_hook(&probe, "test:probe", None, |step| {
            if step == workspace::PublicationStep::IntentStored {
                return Err(pc_image::Error::Invalid("injected interruption".into()));
            }
            stop += 1;
            Ok(())
        })
        .is_err()
    );
    drop(probe);
    fixture.changed_from("play", "test:parent", 81);
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "publication_exit_child", "--nocapture"])
        .env("PUBLICATION_TEST_STORE", fixture.store.root())
        .env("PUBLICATION_TEST_STEP", stop.to_string())
        .env("PUBLICATION_TEST_CAS", "absent")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(77));
    fixture
        .store
        .update_tag("test:target", &parent, None, &PcProvider)
        .unwrap();
    let owner = fixture.store.open_workspace("play").unwrap();
    let before = fs::read(owner.path().join("media/medium-0.upfj")).unwrap();
    assert!(matches!(
        workspace::publish(&owner, "test:target", None),
        Err(pc_image::Error::Store(image_store::Error::Conflict { .. }))
    ));
    assert!(owner.read("transaction.toml").unwrap().is_none());
    assert_eq!(
        fs::read(owner.path().join("media/medium-0.upfj")).unwrap(),
        before
    );
    workspace::start(&owner, "test:parent").unwrap();
    workspace::publish(&owner, "test:other", None).unwrap();
    fixture.assert_complete("test:other", 81);
    fixture.assert_complete("test:target", 80);
}
