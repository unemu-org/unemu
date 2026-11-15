// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Push… and Image › Pull… as modal dialogs, following the approved
//! registry-accounts mock-up; `backlog/docs/design/machine-images.md` §5.1.
//! Rendering only: every decision lives in [`crate::registry_transfer`].

use crate::{
    library_view::human_size,
    registries_model::Mark,
    registry_transfer::{PullDialog, PullState, PushDialog, PushState, TransferRequest},
};

pub(crate) enum Action {
    None,
    Close,
    Send(TransferRequest),
}

fn title(ui: &mut egui::Ui, text: &str, classic: bool) {
    if classic {
        crate::shell::gradient_title(ui, text);
    } else {
        ui.heading(text);
    }
}

fn small(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}

fn error(ui: &mut egui::Ui, text: &str) {
    let color = ui.visuals().error_fg_color;
    ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
}

fn mark(ui: &mut egui::Ui, mark: Mark, text: &str) {
    let (sign, color) = match mark {
        Mark::Good => ("✓", egui::Color32::from_rgb(31, 107, 51)),
        Mark::Bad => ("✗", ui.visuals().error_fg_color),
        Mark::Note => ("•", ui.visuals().text_color()),
    };
    ui.horizontal(|ui| {
        ui.colored_label(color, sign);
        ui.add(egui::Label::new(text).wrap());
    });
}

pub(crate) fn show_push(ctx: &egui::Context, dialog: &mut PushDialog, classic: bool) -> Action {
    let mut action = Action::None;
    let heading = match &dialog.image {
        Some(tag) => format!("Push {tag}"),
        None => "Push image".to_owned(),
    };
    egui::Modal::new(egui::Id::unique("registry-push")).show(ctx, |ui| {
        ui.set_width(760.0);
        title(ui, &heading, classic);
        match dialog.state {
            PushState::Planning | PushState::Ready => dry_run(ui, dialog, &mut action),
            PushState::ConfirmMove { .. } => confirm_move(ui, dialog, &mut action),
            PushState::Pushing(_) | PushState::Finished { .. } => {
                push_progress(ui, dialog, &mut action)
            }
        }
    });
    if ctx.input_mut(crate::shell::escape_pressed) {
        match dialog.state {
            PushState::Pushing(_) => dialog.cancel(),
            PushState::ConfirmMove { .. } => dialog.decline_move(),
            _ => action = Action::Close,
        }
    }
    action
}

fn dry_run(ui: &mut egui::Ui, dialog: &mut PushDialog, action: &mut Action) {
    if let Some(notice) = &dialog.notice {
        let color = ui.visuals().warn_fg_color;
        ui.colored_label(color, notice);
    }
    let mut chosen = None;
    let hint = dialog.private_suggestion();
    egui::Grid::new("push-form")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Image");
            egui::ComboBox::from_id_salt("push-image")
                .width(460.0)
                .selected_text(dialog.image.clone().unwrap_or_else(|| "—".into()))
                .show_ui(ui, |ui| {
                    for image in &dialog.images {
                        let selected = dialog.image.as_deref() == Some(image.tag.as_str());
                        if ui
                            .add(egui::Button::selectable(
                                selected,
                                format!("{}  ·  {}", image.tag, image.kind),
                            ))
                            .clicked()
                        {
                            chosen = Some(image.tag.clone());
                        }
                    }
                });
            ui.end_row();
            ui.label("Destination");
            ui.add(
                egui::TextEdit::singleline(&mut dialog.destination)
                    .desired_width(460.0)
                    .hint_text(hint),
            );
            ui.end_row();
        });
    if let Some(tag) = chosen {
        if let Some(request) = dialog.select_image(&tag) {
            *action = Action::Send(request);
        }
    }
    ui.horizontal_wrapped(|ui| {
        for suggestion in dialog.suggestions() {
            if ui.button(&suggestion).clicked() {
                dialog.destination = suggestion;
            }
        }
    });
    let evaluation = dialog.evaluate();
    if let Some(target) = &evaluation.resolved {
        ui.label(format!("Resolves to {target}"));
    }
    ui.separator();
    ui.strong("Dry run — every blob in the image, with its redistribution mark");
    if let Some(plan) = dialog.plan() {
        ui.monospace(format!("Image {}", plan.image.digest));
    }
    if matches!(dialog.state, PushState::Planning) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading the image and the marks of every local image…");
        });
    } else if dialog.plan().is_some() {
        egui::ScrollArea::vertical()
            .id_salt("push-dry-run")
            .max_height(170.0)
            .show(ui, |ui| {
                egui::Grid::new("push-closure")
                    .num_columns(6)
                    .striped(true)
                    .min_col_width(60.0)
                    .show(ui, |ui| {
                        for heading in [
                            "Part",
                            "Type",
                            "Digest",
                            "Size",
                            "Redistribution",
                            "Recorded in",
                        ] {
                            ui.strong(heading);
                        }
                        ui.end_row();
                        for row in dialog.dry_run() {
                            ui.label(&row.part);
                            ui.label(&row.kind);
                            ui.monospace(&row.digest);
                            ui.label(human_size(row.size));
                            if row.class == "open" {
                                ui.label(row.class);
                            } else {
                                ui.colored_label(ui.visuals().warn_fg_color, row.class);
                            }
                            ui.label(&row.source);
                            ui.end_row();
                        }
                    });
            });
    }
    ui.separator();
    ui.strong("Checks before upload");
    for line in &evaluation.checks {
        mark(ui, line.mark, &line.text);
    }
    match &evaluation.refusal {
        Some(refusal) if evaluation.refused_by_guard => {
            egui::Frame::group(ui.style())
                .corner_radius(0)
                .show(ui, |ui| {
                    error(ui, &format!("Push refused. {refusal}"));
                });
            small(
                ui,
                format!(
                    "There is no override for this rule. Choose a private destination such as {}.",
                    dialog.private_suggestion()
                ),
            );
        }
        // While the dry run is read, the spinner above says so.
        Some(_) if matches!(dialog.state, PushState::Planning) => {}
        Some(problem) => error(ui, problem),
        None => {
            ui.label(format!(
                "Ready to push. {} in {} blobs; blobs the registry already has are skipped.",
                human_size(evaluation.bytes),
                evaluation.blobs
            ));
        }
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button("Cancel").clicked() {
            *action = Action::Close;
        }
        if ui
            .add_enabled(dialog.can_push(), egui::Button::new("Push"))
            .clicked()
        {
            if let Some(request) = dialog.push() {
                *action = Action::Send(request);
            }
        }
        if evaluation.refused_by_guard {
            small(ui, "Push is disabled: the guard refused this destination.");
        }
    });
}

fn confirm_move(ui: &mut egui::Ui, dialog: &mut PushDialog, action: &mut Action) {
    let PushState::ConfirmMove {
        target,
        current,
        new,
    } = &dialog.state
    else {
        return;
    };
    ui.strong("Move the tag to this image?");
    ui.add(
        egui::Label::new(format!(
            "{target} names another image on the registry. Pushing moves the tag; it is not a version tag, so it may move, and only away from the image shown here."
        ))
        .wrap(),
    );
    egui::Grid::new("push-move")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("Now");
            ui.monospace(current.to_string());
            ui.end_row();
            ui.label("After the push");
            ui.monospace(new.to_string());
            ui.end_row();
        });
    small(
        ui,
        "The old image stays reachable by its digest and by its other tags. If the tag names yet another image when the push starts, you are asked again.",
    );
    let mut confirm = false;
    let mut back = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        back = ui.button("Back").clicked();
        confirm = ui.button("Move tag").clicked();
    });
    if confirm {
        if let Some(request) = dialog.confirm_move() {
            *action = Action::Send(request);
        }
    } else if back {
        dialog.decline_move();
    }
}

fn push_progress(ui: &mut egui::Ui, dialog: &mut PushDialog, action: &mut Action) {
    let (progress, finished) = match &dialog.state {
        PushState::Pushing(progress) => (progress, None),
        PushState::Finished {
            ok,
            message,
            progress,
        } => (progress, Some((*ok, message.clone()))),
        _ => return,
    };
    ui.label(format!("To {}", progress.target));
    let blobs = progress.blobs();
    let done = blobs.iter().filter(|row| row.outcome.is_some()).count();
    let bytes: u64 = blobs.iter().map(|row| row.size).sum();
    let sent: u64 = blobs.iter().map(|row| row.sent.min(row.size)).sum();
    let fraction = if progress.published {
        1.0
    } else {
        // The manifest is the last step, so the bar stops short of full.
        sent as f32 / (bytes.max(1) as f32) * 0.99
    };
    ui.add(egui::ProgressBar::new(fraction).text(format!(
        "{done} of {} blobs · {} of {}",
        blobs.len(),
        human_size(sent),
        human_size(bytes)
    )));
    let current = blobs.iter().position(|row| row.outcome.is_none());
    egui::ScrollArea::vertical()
        .id_salt("push-progress")
        .max_height(220.0)
        .show(ui, |ui| {
            egui::Grid::new("push-blobs")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for (index, row) in blobs.iter().enumerate() {
                        ui.label(&row.label);
                        ui.label(human_size(row.size));
                        ui.label(row.status(
                            Some(index) == current,
                            finished.is_some(),
                            progress.cancelling,
                        ));
                        ui.end_row();
                    }
                    ui.label("Manifest and tag");
                    ui.label("");
                    ui.label(if progress.published {
                        "Written ✓"
                    } else if finished.is_some() {
                        "Not written"
                    } else {
                        "Waiting — written after every blob"
                    });
                    ui.end_row();
                });
        });
    small(
        ui,
        "Large blobs go up in chunks of about 64 MiB. Each accepted chunk is remembered, so an interrupted push resumes at the registry's offset instead of starting again. Cancel stops at the next blob or chunk; a request already sending, up to 64 MiB, finishes first. Blobs the registry already has are skipped. The tag is written last.",
    );
    match &finished {
        Some((true, message)) => {
            ui.add(egui::Label::new(message).wrap());
        }
        Some((false, message)) => error(ui, message),
        None => {}
    }
    let cancelling = progress.cancelling;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if finished.is_some() {
            if ui.button("Close").clicked() {
                *action = Action::Close;
            }
            if ui.button("Back to the dry run").clicked() {
                dialog.back();
            }
        } else if ui
            .add_enabled(
                !cancelling,
                egui::Button::new(if cancelling {
                    "Cancelling…"
                } else {
                    "Cancel"
                }),
            )
            .clicked()
        {
            dialog.cancel();
        }
    });
}

pub(crate) fn show_pull(ctx: &egui::Context, dialog: &mut PullDialog, classic: bool) -> Action {
    let mut action = Action::None;
    egui::Modal::new(egui::Id::unique("registry-pull")).show(ctx, |ui| {
        ui.set_width(620.0);
        title(ui, "Pull image", classic);
        if let Some(problem) = &dialog.error {
            error(ui, problem);
        }
        let running = dialog.running();
        egui::Grid::new("pull-form")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Reference");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut dialog.reference)
                        .desired_width(420.0)
                        .hint_text("open-at:1.0.0"),
                );
                ui.end_row();
                ui.label("Local tag");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut dialog.local_tag)
                        .desired_width(420.0)
                        .hint_text("the short form of the reference"),
                );
                ui.end_row();
            });
        small(
            ui,
            format!(
                "A short name such as open-at:1.0.0 means {}/unemu/open-at:1.0.0, the way Docker names resolve to Docker Hub.",
                dialog.default_host
            ),
        );
        match dialog.resolve() {
            Ok((source, local)) => {
                ui.label(format!("Resolves to {source}; stored as {local}"));
            }
            Err(problem) if !dialog.reference.trim().is_empty() => error(ui, &problem),
            Err(_) => {}
        }
        match &dialog.state {
            PullState::Ready => {}
            PullState::Pulling(run) => {
                let live = run.progress();
                let bytes: u64 = live.fetched.iter().map(|d| d.size).sum();
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if run.cancelling {
                        "Stopping at the next read…".to_owned()
                    } else {
                        format!(
                            "Fetched {} blobs ({}) from {} for {}",
                            live.fetched.len(),
                            human_size(bytes),
                            run.source,
                            run.local_tag
                        )
                    });
                });
                for (descriptor, have) in &live.receiving {
                    ui.label(format!(
                        "{} · {} of {}",
                        crate::registry_transfer::kind_label(descriptor),
                        human_size(*have),
                        human_size(descriptor.size)
                    ));
                }
                for descriptor in live.fetched.iter().rev().take(6) {
                    ui.label(format!(
                        "{} · {} ✓",
                        crate::registry_transfer::kind_label(descriptor),
                        human_size(descriptor.size)
                    ));
                }
                small(
                    ui,
                    "An interrupted blob continues where it stopped. The local tag is written after every blob is verified.",
                );
            }
            PullState::Finished { ok: true, message, .. } => {
                ui.add(egui::Label::new(message).wrap());
            }
            PullState::Finished { message, .. } => error(ui, message),
        }
        let stage = match &dialog.state {
            PullState::Ready => None,
            PullState::Pulling(run) => Some(Some(run.cancelling)),
            PullState::Finished { .. } => Some(None),
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match stage {
            None => {
                if ui.button("Cancel").clicked() {
                    action = Action::Close;
                }
                if ui
                    .add_enabled(dialog.can_pull(), egui::Button::new("Pull"))
                    .clicked()
                {
                    if let Some(request) = dialog.pull() {
                        action = Action::Send(request);
                    }
                }
            }
            Some(Some(cancelling)) => {
                if ui
                    .add_enabled(
                        !cancelling,
                        egui::Button::new(if cancelling { "Cancelling…" } else { "Cancel" }),
                    )
                    .clicked()
                {
                    dialog.cancel();
                }
            }
            Some(None) => {
                if ui.button("Close").clicked() {
                    action = Action::Close;
                }
                if ui.button("Pull another").clicked() {
                    dialog.back();
                }
            }
        });
    });
    if ctx.input_mut(crate::shell::escape_pressed) {
        if dialog.running() {
            dialog.cancel();
        } else {
            action = Action::Close;
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        library_test_support::Fixture,
        registry_transfer::{self, tests::accounts_for, PullFailure, PushFailure, TransferEvent},
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
    fn render<T>(
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

    fn escape() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    fn shows(texts: &[String], wanted: &str) -> bool {
        texts.iter().any(|text| text.contains(wanted))
    }

    #[test]
    fn push_and_pull_render_every_stage_without_a_window() {
        let f = Fixture::new();
        let host = "127.0.0.1:5999";
        let accounts = accounts_for(f.temp.path(), host, true, Arc::new(SessionStore::default()));
        // The dry run comes from the function the worker runs.
        let planned = |tag: &str| {
            let (mut dialog, request) = PushDialog::open(accounts.clone(), Some(tag.into()));
            let events = Mutex::new(Vec::new());
            pc_image::test_support::with_synthetic_firmware(|| {
                cli::with_image_providers(|providers| {
                    registry_transfer::run(
                        &f.store,
                        request,
                        providers,
                        &[],
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
            "Push test:source",
            "Dry run — every blob in the image, with its redistribution mark",
            "user-supplied",
            &format!("Resolves to {host}/unemu/test:1.0.0"),
            "Push refused. This image contains user-supplied firmware",
            "There is no override for this rule. Choose a private destination such as private/test:source.",
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
            "Cancelled. The manifest was not published",
            "Not written",
            "Not sent",
            "Close",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        let (action, _) = push(&mut dialog, escape());
        assert!(matches!(action, Action::Close));

        let mut pull = PullDialog::open(accounts);
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
            shows(&shown, "Fetched 1 blobs") && shows(&shown, "for private/test:1.0.0"),
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
