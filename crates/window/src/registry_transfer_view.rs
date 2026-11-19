// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Push…, Image › Pull by reference… and Pull… from the library's
//! Remotes are drawn by `library_ui::registry_transfer_view`, re-exported
//! here under its old path. Its test, which plans a push of a PC image with
//! the program's provider list, stays with the window, with the drawing
//! helpers it uses.

pub(crate) use library_ui::registry_transfer_view::*;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        library_test_support::Fixture,
        pc_desktop::PcDesktop,
        registry_transfer::{
            self, tests::accounts_for, PullDialog, PullFailure, PullState, PushDialog, PushFailure,
            PushState, TransferEvent, TransferRequest,
        },
    };
    use image_registry::credential_store::SessionStore;
    use image_store::Digest;
    use std::sync::{Arc, Mutex};

    fn texts(output: &egui::FullOutput) -> Vec<String> {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut out);
        }
        out
    }

    /// Two frames, so the modal is laid out; the second frame's action and text.
    pub(crate) fn render<T>(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut draw: impl FnMut(&egui::Context) -> T,
    ) -> (T, Vec<String>) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw(ui.ctx());
        });
        output.textures_delta.clear();
        let mut result = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| result = Some(draw(ui.ctx())),
        );
        output.textures_delta.clear();
        (result.unwrap(), texts(&output))
    }

    pub(crate) fn escape() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    pub(crate) fn shows(texts: &[String], wanted: &str) -> bool {
        texts.iter().any(|text| text.contains(wanted))
    }

    #[test]
    fn push_and_pull_render_every_stage_without_a_window() {
        let f = Fixture::new();
        let host = "127.0.0.1:5999";
        let accounts = accounts_for(f.temp.path(), host, true, Arc::new(SessionStore::default()));
        // The dry run comes from the function the worker runs.
        let planned = |tag: &str| {
            let (mut dialog, request) =
                PushDialog::open(accounts.clone(), PcDesktop::facts(), Some(tag.into()));
            let events = Mutex::new(Vec::new());
            pc_image::test_support::with_synthetic_firmware(|| {
                platforms::with_image_providers(|providers| {
                    let pc = PcDesktop::new(providers, &[]);
                    registry_transfer::run(
                        &f.store,
                        request,
                        &[&pc],
                        &|event| events.lock().unwrap().push(event),
                        &|| {},
                    )
                })
            });
            for event in events.into_inner().unwrap() {
                dialog.on_event(&event);
            }
            dialog
        };
        let ctx = egui::Context::default();
        let push = |dialog: &mut PushDialog, events| {
            render(&ctx, events, |ctx| show_push(ctx, dialog, true))
        };

        // A dry run that could not be read shows its error, not the spinner text.
        let mut missing = planned("missing:1.0.0");
        assert!(matches!(missing.state, PushState::Ready));
        let (_, shown) = push(&mut missing, vec![]);
        assert!(shows(&shown, "tag not found"), "{shown:?}");
        assert!(!shows(&shown, "Reading the image"), "{shown:?}");

        let mut dialog = planned("test:source");
        assert!(matches!(dialog.state, PushState::Ready));

        dialog.destination = format!("{host}/unemu/test:1.0.0");
        let (_, shown) = push(&mut dialog, vec![]);
        for wanted in [
            "Push to… — test:source",
            "Stays",
            "Pushing never renames it.",
            "Dry run — every blob in the image, with its redistribution mark",
            "user-supplied",
            "Resolves to",
            &format!("{host}/unemu/test:1.0.0"),
            "Push refused.",
            "This image contains user-supplied firmware",
            &format!("There is no override for this rule. Choose a private namespace, such as {host}/. Nothing was sent."),
            "Push is disabled: the guard refused this destination.",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        assert!(!shows(&shown, "synthetic-secret"));
        dialog.destination = format!("{host}/private/test:1.0.0");
        let (_, shown) = push(&mut dialog, vec![]);
        assert!(shows(&shown, "Ready to push."), "{shown:?}");

        let Some(TransferRequest::Push(job)) = dialog.push() else {
            panic!()
        };
        let (_, shown) = push(&mut dialog, vec![]);
        for wanted in [
            "Manifest and tag",
            "Waiting — written after every blob",
            "Uploading · single chunk",
            "Queued",
            "Cancel",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }

        let (old, new) = (Digest::of(b"old"), Digest::of(b"new"));
        dialog.on_event(&TransferEvent::Pushed {
            id: job.id,
            result: Err(PushFailure::Move {
                tag: "1.0.0".into(),
                current: old.clone(),
                new: new.clone(),
            }),
        });
        let (_, shown) = push(&mut dialog, vec![]);
        for wanted in [
            "Move the tag to this image?",
            &old.to_string(),
            &new.to_string(),
            "Move tag",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        let (action, _) = push(&mut dialog, escape());
        assert!(matches!(action, Action::None));
        assert!(matches!(dialog.state, PushState::Ready));

        let Some(TransferRequest::Push(job)) = dialog.push() else {
            panic!()
        };
        // The worker reports a large blob at its second chunk.
        let chunk = image_registry::push::DEFAULT_CHUNK_SIZE;
        job.live.update(|rows| {
            rows[0].size = 3 * chunk + 1;
            rows[0].sent = chunk;
        });
        let (_, shown) = push(&mut dialog, vec![]);
        assert!(
            shows(&shown, "chunk 2 of 4 · 64.0 MiB of 192.0 MiB"),
            "{shown:?}"
        );
        // Escape asks a running push to stop; the dialog stays for its answer.
        let (action, _) = push(&mut dialog, escape());
        assert!(matches!(action, Action::None));
        assert!(matches!(&dialog.state, PushState::Pushing(p) if p.cancelling));
        let (_, shown) = push(&mut dialog, vec![]);
        assert!(shows(&shown, "Cancelling…"), "{shown:?}");
        assert!(shows(&shown, "Stopping — chunk 2 of 4"), "{shown:?}");
        dialog.on_event(&TransferEvent::Pushed {
            id: job.id,
            result: Err(PushFailure::Cancelled),
        });
        let (_, shown) = push(&mut dialog, vec![]);
        for wanted in [
            "Cancelled.",
            "The manifest was not published",
            "Not written",
            "Not sent",
            "Close",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        let (action, _) = push(&mut dialog, escape());
        assert!(matches!(action, Action::Close));

        let mut pull = PullDialog::open(accounts, PcDesktop::facts());
        let draw =
            |pull: &mut PullDialog, events| render(&ctx, events, |ctx| show_pull(ctx, pull, false));
        let (_, shown) = draw(&mut pull, vec![]);
        assert!(shows(
            &shown,
            "A short name such as open-at:1.0.0 means registry.unemu.dev/unemu/open-at:1.0.0"
        ));
        pull.reference = "open-at:1.0.0".into();
        let (_, shown) = draw(&mut pull, vec![]);
        assert!(
            shows(
                &shown,
                "Resolves to registry.unemu.dev/unemu/open-at:1.0.0; stored as open-at:1.0.0"
            ),
            "{shown:?}"
        );
        pull.reference = "lab:1.0.0+b1".into();
        let (_, shown) = draw(&mut pull, vec![]);
        assert!(shows(&shown, "does not follow the registry tag grammar"));
        assert!(!pull.can_pull());
        pull.reference = format!("{host}/private/test:1.0.0");
        let Some(TransferRequest::Pull(job)) = pull.pull() else {
            panic!()
        };
        let mut receiving = f.row.descriptor.clone();
        receiving.size = 4 << 20;
        job.live.update(|state| {
            state.fetched.push(f.row.descriptor.clone());
            state.receiving.push((receiving, 1 << 20));
        });
        let (_, shown) = draw(&mut pull, vec![]);
        assert!(
            shows(&shown, "Fetched 1 blobs")
                && shows(&shown, &format!("for {host}/private/test:1.0.0")),
            "{shown:?}"
        );
        assert!(shows(&shown, "1.0 MiB of 4.0 MiB"), "{shown:?}");
        let (action, _) = draw(&mut pull, escape());
        assert!(matches!(action, Action::None));
        assert!(matches!(&pull.state, PullState::Pulling(p) if p.cancelling));
        pull.on_event(&TransferEvent::Pulled {
            id: job.id,
            result: Err(PullFailure::Cancelled),
        });
        let (_, shown) = draw(&mut pull, vec![]);
        assert!(shows(&shown, "Cancelled. The local tag was not set."));
        assert!(shows(&shown, "Pull another"));
    }
}
