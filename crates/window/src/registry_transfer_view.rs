// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Push…, Image › Pull… and File › Get templates… as modal dialogs,
//! following the approved registry-accounts and get-templates mock-ups;
//! `backlog/docs/design/machine-images.md` §§5.1 and 5.5. Rendering only:
//! every decision lives in [`crate::registry_transfer`] and
//! [`crate::templates`].

use crate::{
    library_view::human_size,
    registries_model::Mark,
    registry_transfer::{PullDialog, PullState, PushDialog, PushState, TransferRequest},
    templates::{FailureKind, ListingFailure, Local, Stage, TemplatesDialog, TemplatesState},
};

/// The colour of a finished, good result.
const GOOD: egui::Color32 = egui::Color32::from_rgb(31, 107, 51);

pub(crate) enum Action {
    None,
    Close,
    Send(TransferRequest),
    /// Several jobs, which the worker runs in this order.
    SendAll(Vec<TransferRequest>),
    /// Open Preferences › Registries on this registry's Log in dialog.
    LogIn(String),
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
        Mark::Good => ("✓", GOOD),
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
                if let Some(line) = dialog.namespace_line() {
                    small(ui, line);
                }
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

pub(crate) fn show_templates(
    ctx: &egui::Context,
    dialog: &mut TemplatesDialog,
    classic: bool,
) -> Action {
    let mut action = Action::None;
    let mut log_in = false;
    let host = dialog.default_host.clone();
    egui::Modal::new(egui::Id::unique("registry-templates")).show(ctx, |ui| {
        ui.set_width(760.0);
        title(ui, "Get templates", classic);
        ui.add(
            egui::Label::new(format!(
                "Official machine templates from {host}/unemu/: open firmware with its source archives and blank disks, no operating system. Each one is pulled like Image › Pull… and written to the library under its short name."
            ))
            .wrap(),
        );
        small(
            ui,
            format!("{host} is your default registry; change it under Tools › Preferences › Registries."),
        );
        ui.add_space(6.0);
        match &dialog.state {
            TemplatesState::Listing => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Listing the templates on {host}…"));
                });
                small(
                    ui,
                    "Only unemu/ is read. An offline or unreachable registry ends in a message within a few seconds; closing the dialog stops the listing.",
                );
            }
            TemplatesState::Failed(failure) => log_in = listing_failure(ui, failure),
            TemplatesState::Ready if dialog.rows.is_empty() => {
                ui.group(|ui| {
                    ui.strong(format!("{host} offers no templates yet."));
                    small(
                        ui,
                        "Templates appear here once they are published under unemu/. Until then, File › New machine… builds a machine from firmware packages.",
                    );
                });
            }
            TemplatesState::Ready => {
                template_list(ui, dialog);
                small(
                    ui,
                    "Each template is offered at the version latest names, else its newest release; Image › Pull… gets other versions. Sizes count every blob of a template; blobs the library already has, such as the firmware both templates share, are not fetched again.",
                );
            }
            TemplatesState::Pulling(_) => template_pulls(ui, dialog),
        }
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if dialog.running() {
                let cancelling = dialog.cancelling();
                let label = if cancelling { "Cancelling…" } else { "Cancel" };
                if ui
                    .add_enabled(!cancelling, egui::Button::new(label))
                    .clicked()
                {
                    dialog.cancel();
                }
            } else {
                if ui.button("Close").clicked() {
                    action = Action::Close;
                }
                match dialog.state {
                    TemplatesState::Ready if dialog.offers_get() => {
                        if ui
                            .add_enabled(dialog.can_get(), egui::Button::new(dialog.get_label()))
                            .clicked()
                        {
                            action = Action::SendAll(dialog.get());
                        }
                    }
                    TemplatesState::Failed(_) => {
                        if ui.button("Try again").clicked() {
                            if let Some(request) = dialog.relist() {
                                action = Action::Send(request);
                            }
                        }
                    }
                    TemplatesState::Pulling(_) => {
                        if ui.button("Back to the list").clicked() {
                            if let Some(request) = dialog.relist() {
                                action = Action::Send(request);
                            }
                        }
                    }
                    TemplatesState::Ready | TemplatesState::Listing => {}
                }
            }
            if let Some(note) = dialog.footer_note() {
                small(ui, note);
            }
        });
    });
    if log_in {
        if let Some(host) = dialog.log_in() {
            action = Action::LogIn(host);
        }
    }
    if ctx.input_mut(crate::shell::escape_pressed) {
        if dialog.running() {
            dialog.cancel();
        } else {
            action = Action::Close;
        }
    }
    action
}

/// One message for a listing that ended without a list, with Log in… for
/// the two login failures, or Add registry… when the registry has no entry
/// yet; true when either was clicked.
fn listing_failure(ui: &mut egui::Ui, failure: &ListingFailure) -> bool {
    // A registry without a catalog is an answer, not an error.
    let color = if failure.kind == FailureKind::NoCatalog {
        ui.visuals().warn_fg_color
    } else {
        ui.visuals().error_fg_color
    };
    let mut log_in = false;
    ui.group(|ui| {
        ui.label(egui::RichText::new(failure.heading()).strong().color(color));
        ui.add(egui::Label::new(egui::RichText::new(&failure.message).color(color)).wrap());
        if failure.offers_login() {
            log_in = ui.button("Log in…").clicked();
        } else if failure.offers_add() {
            log_in = ui.button("Add registry…").clicked();
        }
    });
    small(ui, failure.hint());
    log_in
}

fn template_list(ui: &mut egui::Ui, dialog: &mut TemplatesDialog) {
    let mut toggled = None;
    ui.label(
        egui::RichText::new(format!(
            "{}/unemu/ ({})",
            dialog.default_host,
            dialog.rows.len()
        ))
        .strong(),
    );
    egui::ScrollArea::vertical()
        .id_salt("template-list")
        .max_height(330.0)
        .show(ui, |ui| {
            for (index, row) in dialog.rows.iter().enumerate() {
                ui.separator();
                let choosable = dialog.choosable(index);
                // A row already in the library, or blocked by its local tag,
                // is dimmed and cannot be chosen.
                let shown = ui.add_enabled_ui(choosable, |ui| {
                    ui.horizontal(|ui| {
                        let mut chosen = dialog.chosen.contains(&index);
                        let checkbox = egui::Checkbox::without_text(&mut chosen);
                        if ui.add(checkbox).changed() {
                            toggled = Some(index);
                        }
                        ui.strong(&row.name);
                        ui.monospace(&row.tag);
                        ui.label(format!("· {}", row.title.as_deref().unwrap_or("untitled")));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(human_size(row.size));
                        });
                    });
                    if let Some(description) = &row.description {
                        small(ui, description.as_str());
                    }
                    match row.local {
                        Local::Absent => {}
                        Local::Same => {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("In the library")
                                        .small()
                                        .strong()
                                        .color(GOOD),
                                );
                                small(ui, format!("as {}", row.local_tag));
                            });
                        }
                        Local::Other => {
                            let color = ui.visuals().warn_fg_color;
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "The local tag {} names another image; pull this one under another tag with Image › Pull….",
                                        row.local_tag
                                    ))
                                    .small()
                                    .color(color),
                                )
                                .wrap(),
                            );
                        }
                    }
                });
                // The whole row toggles its checkbox, as a label would.
                if choosable
                    && ui
                        .interact(
                            shown.response.rect,
                            ui.make_persistent_id(("template-row", index)),
                            egui::Sense::click(),
                        )
                        .clicked()
                {
                    toggled = Some(index);
                }
            }
        });
    if let Some(index) = toggled {
        dialog.toggle(index);
    }
}

/// The chosen templates under one progress bar, a row each, and what they
/// added once every pull answered.
fn template_pulls(ui: &mut egui::Ui, dialog: &TemplatesDialog) {
    let TemplatesState::Pulling(pulls) = &dialog.state else {
        return;
    };
    ui.label(format!(
        "From {}/unemu/, one after another, each by the digest the list showed.",
        dialog.default_host
    ));
    if let Some((fraction, label)) = dialog.progress() {
        ui.label(label);
        ui.add(egui::ProgressBar::new(fraction));
    }
    egui::ScrollArea::vertical()
        .id_salt("template-pulls")
        .max_height(260.0)
        .show(ui, |ui| {
            for (index, pull) in pulls.iter().enumerate() {
                let Some(stage) = dialog.stage(index) else {
                    continue;
                };
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(&pull.name);
                    ui.monospace(&pull.tag);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(stage.label());
                    });
                });
                small(ui, pull.route());
                let detail = dialog.detail(index).unwrap_or_default();
                let color = match stage {
                    Stage::Pulled => GOOD,
                    Stage::Failed => ui.visuals().error_fg_color,
                    _ => ui.visuals().text_color(),
                };
                ui.add(egui::Label::new(egui::RichText::new(detail).small().color(color)).wrap());
            }
        });
    if dialog.running() {
        small(
            ui,
            "An interrupted blob continues where it stopped. Each local tag is written only after every blob of its template is verified.",
        );
    } else if let Some(summary) = dialog.summary() {
        let color = if summary.complete {
            GOOD
        } else {
            ui.visuals().warn_fg_color
        };
        ui.group(|ui| {
            ui.label(egui::RichText::new(summary.heading).strong().color(color));
            ui.add(egui::Label::new(summary.text).wrap());
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        library_test_support::Fixture,
        registry_transfer::{self, tests::accounts_for, PullFailure, PushFailure, TransferEvent},
    };
    use image_registry::credential_store::SessionStore;
    use image_store::Digest;
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };

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

    #[test]
    fn get_templates_renders_every_stage_without_a_window() {
        use crate::templates::{FailureKind, ListingFailure, TemplateRow};
        let f = Fixture::new();
        let host = "127.0.0.1:5999";
        let accounts = accounts_for(
            f.temp.path(),
            host,
            false,
            Arc::new(SessionStore::default()),
        );
        let accounts = registry_transfer::Accounts {
            default_override: Some(host.into()),
            ..accounts
        };
        let ctx = egui::Context::default();
        let draw = |dialog: &mut TemplatesDialog, events| {
            render(&ctx, events, |ctx| show_templates(ctx, dialog, true))
        };
        // The Get button, not the title that starts with the same word.
        let get_shown = |shown: &[String]| {
            shown
                .iter()
                .any(|t| t.starts_with("Get") && t != "Get templates")
        };
        let (mut dialog, request) = TemplatesDialog::open(accounts);
        let Some(TransferRequest::Templates { id, .. }) = request else {
            panic!("the dialog asks for the list")
        };
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            "Get templates",
            &format!("Official machine templates from {host}/unemu/: open firmware with its source archives and blank disks, no operating system. Each one is pulled like Image › Pull… and written to the library under its short name."),
            &format!("{host} is your default registry; change it under Tools › Preferences › Registries."),
            &format!("Listing the templates on {host}…"),
            "Only unemu/ is read. An offline or unreachable registry ends in a message within a few seconds; closing the dialog stops the listing.",
            "Close stops the listing.",
            "Close",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        assert!(!get_shown(&shown), "{shown:?}");
        let row = |name: &str, local| TemplateRow {
            reference: image_registry::Reference::parse(
                &format!("{host}/unemu/{name}:1.0.0"),
                host,
            )
            .unwrap(),
            name: name.into(),
            tag: "1.0.0".into(),
            title: Some(format!("Title of {name}")),
            description: Some(format!("Description of {name}")),
            size: 3 << 20,
            digest: Digest::of(name.as_bytes()),
            local_tag: format!("{name}:1.0.0"),
            local,
            blobs: Vec::new(),
            stored: BTreeSet::new(),
        };
        dialog.on_event(&TransferEvent::Templates {
            id,
            result: Ok(vec![
                row("open-at", Local::Absent),
                row("open-at-games", Local::Absent),
                row("kept", Local::Same),
                row("taken", Local::Other),
            ]),
        });
        // The list grew since the last frames; one more pass lets the modal settle.
        draw(&mut dialog, vec![]);
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            &format!("{host}/unemu/ (4)"),
            "open-at",
            "1.0.0",
            "· Title of open-at",
            "Description of open-at",
            "3.0 MiB",
            "In the library",
            "as kept:1.0.0",
            "The local tag taken:1.0.0 names another image; pull this one under another tag with Image › Pull….",
            "Each template is offered at the version latest names, else its newest release; Image › Pull… gets other versions. Sizes count every blob of a template; blobs the library already has, such as the firmware both templates share, are not fetched again.",
            "Choose the templates to get.",
            "Close",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        assert!(shown.iter().any(|t| t == "Get"), "{shown:?}");
        // A click anywhere on a row toggles it, except on the dimmed rows
        // whose local tag already exists.
        for unavailable in ["Description of kept", "Description of taken"] {
            click(&ctx, unavailable, |ctx| {
                show_templates(ctx, &mut dialog, true)
            });
        }
        dialog.toggle(2);
        dialog.toggle(3);
        assert!(
            dialog.chosen.is_empty(),
            "a template whose local tag exists is not chosen"
        );
        click(&ctx, "Description of open-at", |ctx| {
            show_templates(ctx, &mut dialog, true)
        });
        assert_eq!(dialog.chosen, BTreeSet::from([0]));
        let (_, shown) = draw(&mut dialog, vec![]);
        assert!(shows(&shown, "Get 1 template"), "{shown:?}");
        dialog.toggle(1);
        let (_, shown) = draw(&mut dialog, vec![]);
        assert!(shows(&shown, "Get 2 templates"), "{shown:?}");
        let requests = dialog.get();
        let [TransferRequest::Pull(first), TransferRequest::Pull(second)] = requests.as_slice()
        else {
            panic!("two pulls")
        };
        assert_eq!(first.local_tag, "open-at:1.0.0");
        assert_eq!(second.local_tag, "open-at-games:1.0.0");
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            &format!("From {host}/unemu/, one after another, each by the digest the list showed."),
            "Template 1 of 2 · open-at · 0 B of 3.0 MiB",
            &format!("{host}/unemu/open-at:1.0.0 → open-at:1.0.0 · 3.0 MiB"),
            "0 blobs verified (0 B)",
            "Pulling",
            "Waiting",
            "Starts when the pull before it has answered.",
            "An interrupted blob continues where it stopped. Each local tag is written only after every blob of its template is verified.",
            "Cancel or Escape stops at the next read.",
            "Cancel",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        // The worker reports a blob arriving for the first template.
        let mut receiving = f.row.descriptor.clone();
        receiving.size = 2 << 20;
        first
            .live
            .update(|state| state.receiving.push((receiving, 1 << 20)));
        let (_, shown) = draw(&mut dialog, vec![]);
        assert!(
            shows(&shown, "Template 1 of 2 · open-at · 1.0 MiB of 3.0 MiB"),
            "{shown:?}"
        );
        assert!(
            shows(&shown, "0 blobs verified (0 B) · 1.0 MiB arriving: "),
            "{shown:?}"
        );
        // Escape asks the pulls to stop; the dialog waits for their answers.
        let (action, _) = draw(&mut dialog, escape());
        assert!(matches!(action, Action::None));
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            "Cancelling — stopping at the next read…",
            "Stopping…",
            "Waiting",
            "Cancelling…",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        assert!(!shows(&shown, "Cancel or Escape"), "{shown:?}");
        dialog.on_event(&TransferEvent::Pulled {
            id: first.id,
            result: Err(PullFailure::Cancelled),
        });
        assert!(dialog.running());
        dialog.on_event(&TransferEvent::Pulled {
            id: second.id,
            result: Err(PullFailure::NotStarted),
        });
        assert!(!dialog.running());
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            "Cancelled. The local tag was not set.",
            "Cancelled before it started; nothing was fetched.",
            "Cancelled",
            "Done — 0 of 2 templates pulled",
            "No template was added.",
            "No local tag was written. Blobs already fetched stay in the store, and the next pull fetches only the rest.",
            "Back to the list",
            "Close",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }

        // Each failure shows its heading, message and hint, with Try again.
        let fail = |dialog: &mut TemplatesDialog, kind, listed, message: &str| {
            let Some(TransferRequest::Templates { id, .. }) = dialog.relist() else {
                panic!("the list is read again")
            };
            dialog.on_event(&TransferEvent::Templates {
                id,
                result: Err(ListingFailure {
                    kind,
                    host: host.into(),
                    listed,
                    message: message.into(),
                }),
            });
        };
        fail(
            &mut dialog,
            FailureKind::LoginRequired,
            true,
            &format!("{host} asks for a login. Log in under Tools › Preferences › Registries, then try again."),
        );
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            "The registry asks for a login",
            &format!("{host} asks for a login."),
            "Log in…",
            "Nothing was pulled. After Log in…, the dialog lists again.",
            "Try again",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        // Log in… asks for Preferences › Registries on this registry.
        let action = click(&ctx, "Log in…", |ctx| {
            show_templates(ctx, &mut dialog, true)
        });
        assert!(matches!(&action, Action::LogIn(h) if h == host));
        // A registry without an entry offers Add registry… instead.
        fail(
            &mut dialog,
            FailureKind::LoginRequired,
            false,
            &format!("{host} asks for a login, and it is not in your registry list."),
        );
        let (_, shown) = draw(&mut dialog, vec![]);
        assert!(shows(&shown, "Add registry…"), "{shown:?}");
        assert!(!shows(&shown, "Log in…"), "{shown:?}");
        let action = click(&ctx, "Add registry…", |ctx| {
            show_templates(ctx, &mut dialog, true)
        });
        assert!(matches!(&action, Action::LogIn(h) if h == host));
        fail(
            &mut dialog,
            FailureKind::NoCatalog,
            false,
            &format!("{host} does not list its repositories, so it offers no templates here."),
        );
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            "This registry offers no templates",
            "does not list its repositories",
            "Templates are listed from the registry catalog. registry.unemu.dev has one; choose it as the default registry to get the official templates.",
            "Try again",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        assert!(!shows(&shown, "Log in…"), "{shown:?}");

        // A registry without templates says so, and Get is hidden.
        let Some(TransferRequest::Templates { id, .. }) = dialog.relist() else {
            panic!("the list is read again")
        };
        dialog.on_event(&TransferEvent::Templates {
            id,
            result: Ok(Vec::new()),
        });
        let (_, shown) = draw(&mut dialog, vec![]);
        for wanted in [
            &format!("{host} offers no templates yet."),
            "Templates appear here once they are published under unemu/. Until then, File › New machine… builds a machine from firmware packages.",
            "Close",
        ] {
            assert!(shows(&shown, wanted), "{wanted}: {shown:?}");
        }
        assert!(!get_shown(&shown), "{shown:?}");
        let (action, _) = draw(&mut dialog, escape());
        assert!(matches!(action, Action::Close));
    }

    /// Click the text `label` as a pointer would, after the frames that lay
    /// the modal out; the result of the frame that releases the button.
    pub(crate) fn click<T>(
        ctx: &egui::Context,
        label: &str,
        mut draw: impl FnMut(&egui::Context) -> T,
    ) -> T {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
                _ => None,
            }
        }
        let mut frame = |events: Vec<egui::Event>| {
            let mut result = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| result = Some(draw(ui.ctx())),
            );
            output.textures_delta.clear();
            (result.unwrap(), output)
        };
        frame(vec![]);
        let (_, output) = frame(vec![]);
        let at = output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, label))
            .unwrap_or_else(|| panic!("{label} is not shown"));
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(vec![egui::Event::PointerMoved(at), button(true)]);
        frame(vec![button(false)]).0
    }
}
