// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Push…, Image › Pull by reference… and Pull… from the library's
//! Remotes as modal dialogs, following the approved registry-accounts and
//! remotes mock-ups; `backlog/docs/design/machine-images.md` §§5.1 and 5.5.
//! Rendering only: every decision lives in [`crate::registry_transfer`] and
//! [`crate::remote_pull`].

use crate::{
    library_view::human_size,
    registries_model::Mark,
    registry_transfer::{PullDialog, PullState, PushDialog, PushState, TransferRequest},
    remote_pull::{Drive, Mode, RemotePull, Stage},
};

/// The colour of a finished, good result.
const GOOD: egui::Color32 = egui::Color32::from_rgb(31, 107, 51);

pub(crate) enum Action {
    None,
    Close,
    Send(TransferRequest),
    /// Several jobs, which the worker runs in this order.
    SendAll(Vec<TransferRequest>),
    /// Show the pulled tag in the library.
    ShowPulled,
    /// New machine from image… on the pulled image.
    NewMachine,
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
                        .hint_text("from the reference"),
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

/// Pull… from the library's Remotes, as the remotes mock-up's dialog: one
/// tag with its local tag to confirm or change, or several under one
/// progress bar, then the pulls and what they added.
pub(crate) fn show_remote_pull(
    ctx: &egui::Context,
    dialog: &mut RemotePull,
    classic: bool,
) -> Action {
    let mut action = Action::None;
    let heading = if dialog.several {
        let count = dialog.items.iter().filter(|i| i.skip.is_none()).count();
        format!("Pull {count} {}", if count == 1 { "tag" } else { "tags" })
    } else {
        format!("{} — {}", dialog.mode.verb(), dialog.items[0].name())
    };
    egui::Modal::new(egui::Id::unique("remote-pull")).show(ctx, |ui| {
        ui.set_width(640.0);
        title(ui, &heading, classic);
        if !dialog.started() && !dialog.several {
            pull_form(ui, dialog);
        } else if !dialog.started() {
            ui.add(
                egui::Label::new(
                    "One after another, each by the digest the list showed, under one progress bar. Each local tag is written after every blob of its image is verified.",
                )
                .wrap(),
            );
            pull_items(ui, dialog);
            small(
                ui,
                format!(
                    "Download: {} to fetch. A blob the tags share, or the store already has, is fetched once or not at all.",
                    human_size(dialog.download())
                ),
            );
        } else {
            if !dialog.several {
                let item = &dialog.items[0];
                ui.label(format!(
                    "From {}/{} by {} to {}",
                    item.source.host,
                    item.source.repository,
                    item.source
                        .digest
                        .as_ref()
                        .map_or_else(String::new, crate::registry_transfer::short),
                    item.local_tag
                ));
            }
            if let Some((fraction, label)) = dialog.progress() {
                ui.label(label);
                ui.add(egui::ProgressBar::new(fraction));
            }
            pull_items(ui, dialog);
            if dialog.running() {
                small(
                    ui,
                    "Each blob is verified as it arrives; an interrupted blob continues where it stopped. Each local tag is written only after every blob of its image is verified.",
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
                if !cancelling {
                    small(ui, "Cancel or Escape stops at the next read.");
                }
            } else if dialog.started() {
                if ui.button("Close").clicked() {
                    action = Action::Close;
                }
                if !dialog.several && !dialog.pulled().is_empty() {
                    if dialog.created_image().is_some() {
                        if ui.button("New machine from image…").clicked() {
                            action = Action::NewMachine;
                        }
                    } else if ui.button("Show in library").clicked() {
                        action = Action::ShowPulled;
                    }
                }
            } else {
                if ui.button("Cancel").clicked() {
                    action = Action::Close;
                }
                let problem = dialog.problem();
                if ui
                    .add_enabled(problem.is_none(), egui::Button::new(dialog.go_label()))
                    .clicked()
                {
                    // Done: the local tag names this image already.
                    action = if dialog.unchanged() {
                        Action::Close
                    } else {
                        Action::SendAll(dialog.start())
                    };
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

/// The question of one tag's pull: where from, by which digest, the local
/// tag it writes, what follows and what it downloads.
fn pull_form(ui: &mut egui::Ui, dialog: &mut RemotePull) {
    let problem = dialog.problem();
    let formats = dialog.formats();
    // A proposed local tag in place of one that names another image is put
    // in front, once.
    let focus = std::mem::take(&mut dialog.focus_tag);
    let item = &mut dialog.items[0];
    let default_registry = item.source.host == image_registry::DEFAULT_REGISTRY;
    egui::Grid::new("remote-pull-form")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("From");
            ui.monospace(item.remote());
            ui.end_row();
            ui.label("Digest");
            ui.horizontal(|ui| {
                ui.monospace(
                    item.source
                        .digest
                        .as_ref()
                        .map_or_else(String::new, crate::registry_transfer::short),
                );
                ui.weak("· pulled by this digest, as listed");
            });
            ui.end_row();
            ui.label("Local tag");
            let field = ui.add(
                egui::TextEdit::singleline(&mut item.local_tag)
                    .desired_width(440.0)
                    .hint_text(item.default_tag.as_str()),
            );
            if focus {
                field.request_focus();
            }
            ui.end_row();
        });
    match &problem {
        Some(problem) => error(ui, problem),
        None => small(
            ui,
            if default_registry {
                "From the default registry the short name is kept. Pull moves no local tag, except that a set or package pulled by its release writes the revision and moves its local release alias forward."
            } else {
                "From this registry the name carries its host, so the same path on another registry, or made here, never clashes. Pull moves no local tag, except that a set or package pulled by its release writes the revision and moves its local release alias forward."
            },
        ),
    }
    match dialog.mode {
        Mode::Insert if !dialog.drives.is_empty() => {
            ui.horizontal(|ui| {
                ui.label("Then insert into");
                let chosen = dialog
                    .drives
                    .get(dialog.drive)
                    .map(Drive::label)
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("remote-pull-drive")
                    .width(360.0)
                    .selected_text(chosen)
                    .show_ui(ui, |ui| {
                        for (index, drive) in dialog.drives.iter().enumerate() {
                            ui.selectable_value(&mut dialog.drive, index, drive.label());
                        }
                    });
            });
            small(
                ui,
                format!(
                    "Disk 1 waits in the drive until the machine's next Start. Only stopped machines with a drive that takes {} disks are listed.",
                    formats.join(" and ")
                ),
            );
        }
        Mode::CreateMachine => {
            let name = dialog.items[0]
                .title
                .clone()
                .unwrap_or_else(|| dialog.items[0].local_tag.clone());
            ui.label(format!(
                "Then New machine from image… asks for the machine's name; the default is “{name}”."
            ));
        }
        _ => {}
    }
    let item = &dialog.items[0];
    ui.label(if item.same_as.is_some() {
        "Download: nothing to fetch; the image is already in the store.".to_owned()
    } else {
        let present = item
            .blobs
            .iter()
            .filter(|b| item.stored.contains(&b.digest))
            .count();
        format!(
            "Download: {} of {} · {present} of {} blobs already in the store",
            human_size(item.download()),
            human_size(item.size),
            item.blobs.len()
        )
    });
}

/// The tags of a pull of several, or the one tag once it runs: each with
/// where it goes, its stage and its line.
fn pull_items(ui: &mut egui::Ui, dialog: &RemotePull) {
    egui::ScrollArea::vertical()
        .id_salt("remote-pull-items")
        .max_height(260.0)
        .show(ui, |ui| {
            for (index, item) in dialog.items.iter().enumerate() {
                let stage = dialog.stage(index);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(item.name());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(dialog.stage_label(index));
                    });
                });
                small(
                    ui,
                    format!(
                        "{} → {} · {}",
                        item.source.host,
                        item.local_tag,
                        human_size(item.size)
                    ),
                );
                let color = match stage {
                    Stage::Pulled => GOOD,
                    Stage::Failed => ui.visuals().error_fg_color,
                    Stage::Skipped => ui.visuals().warn_fg_color,
                    _ => ui.visuals().text_color(),
                };
                let detail = dialog.detail(index);
                if !detail.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(detail).small().color(color)).wrap(),
                    );
                }
            }
        });
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
            let (mut dialog, request) = PushDialog::open(accounts.clone(), Some(tag.into()));
            let events = Mutex::new(Vec::new());
            pc_image::test_support::with_synthetic_firmware(|| {
                platforms::with_image_providers(|providers| {
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
