// SPDX-License-Identifier: AGPL-3.0-or-later
//! Push to… and Push here… as modal dialogs, drawn as the approved
//! remotes-push mock-up draws them
//! (`backlog/docs/design/mockups/remotes-push.html`, Remotes revision 3);
//! `backlog/docs/design/machine-images.md` §5.1. Rendering only: every
//! decision lives in [`library_model::push_destination`],
//! [`library_model::registry_transfer`] and [`library_model::push_batch`].
//!
//! Push to… of one item shows the item and its local tag, which stays; the
//! destination as text and, under it, the chooser of registries and
//! namespaces with the repository and tag; then today's dry run, the checks
//! and the verdict. Several items, and Push here… of a registry or
//! namespace, list the items with their destinations and verdicts; the run
//! shows one progress bar, the blobs of one item or a row per item.

use crate::registry_transfer_view::{Action, GOOD};
use image_registry::Reference;
use library_model::{
    listing::human_size,
    push_batch::{BatchState, Mode, PushBatch, Stage},
    push_destination::{ChooserNode, Evaluation, Listing, LocalImage, Place, Rule, Verdict},
    registries_model::Mark,
    registry_transfer::{short, BlobRow, DryRunRow, Progress, PushDialog, PushState},
};
use ui_kit::{pane, shell_icons::Icon};

fn title(ui: &mut egui::Ui, text: &str, classic: bool) {
    if classic {
        ui_kit::skin::gradient_title(ui, text);
    } else {
        ui.heading(text);
    }
}

fn small(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}

fn weak(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).weak()).wrap());
}

fn warn(ui: &mut egui::Ui, text: impl Into<String>) {
    let colour = ui.visuals().warn_fg_color;
    ui.add(egui::Label::new(egui::RichText::new(text.into()).color(colour)).wrap());
}

/// The mock-up's boxes: a refusal in the error colour, a good result in the
/// good colour, each a heading and its sentence.
fn boxed(ui: &mut egui::Ui, good: bool, heading: &str, text: &str) {
    let colour = if good {
        pane::good_colour(ui.visuals())
    } else {
        ui.visuals().error_fg_color
    };
    egui::Frame::group(ui.style())
        .corner_radius(0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(heading).strong().color(colour));
            ui.add(egui::Label::new(text).wrap());
        });
}

/// The type icon of a local image's kind.
fn kind_icon(kind: &str) -> Icon {
    match kind {
        "media set" => Icon::Media,
        "firmware package" => Icon::Firmware,
        _ => Icon::Image,
    }
}

/// One check line with its mark; a Log in… button beside the failed login
/// check. Returns whether Log in… was clicked.
fn check_line(ui: &mut egui::Ui, mark: Mark, text: &str, login: bool) -> bool {
    let (sign, colour) = match mark {
        Mark::Good => ("✓", GOOD),
        Mark::Bad => ("✗", ui.visuals().error_fg_color),
        Mark::Note => ("•", ui.visuals().text_color()),
    };
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(colour, sign);
        ui.add(egui::Label::new(text).wrap());
        login && ui.button("Log in…").clicked()
    })
    .inner
}

/// The checks of one verdict; `Some(host)` when Log in… was clicked.
fn checks(ui: &mut egui::Ui, evaluation: &Evaluation) -> Option<String> {
    let mut login = None;
    for (index, line) in evaluation.checks.iter().enumerate() {
        let beside = evaluation.login_check == Some(index);
        if check_line(ui, line.mark, &line.text, beside) {
            login = evaluation.resolved.as_ref().map(|r| r.host.clone());
        }
    }
    login
}

/// The verdict under the checks, in the mock-up's words.
fn verdict_box(ui: &mut egui::Ui, evaluation: &Evaluation, private: Option<&Place>) {
    let why = evaluation.refusal.clone().unwrap_or_default();
    let size = format!(
        "{} in {} blobs; blobs the registry already has are skipped.",
        human_size(evaluation.bytes),
        evaluation.blobs
    );
    let tag = evaluation
        .resolved
        .as_ref()
        .and_then(|r| r.tag.clone())
        .unwrap_or_default();
    match &evaluation.verdict {
        Verdict::Refused => {
            boxed(ui, false, "Push refused.", &why);
            let mut hint = "There is no override for this rule.".to_owned();
            if why.starts_with("This image") {
                if let Some(place) = private {
                    hint += &format!(" Choose a private namespace, such as {}.", place.prefix());
                }
            }
            hint += " Nothing was sent.";
            small(ui, hint);
        }
        Verdict::TagTaken => {
            boxed(ui, false, "Push refused.", &why);
            small(ui, "The registry’s image stays as it is. Nothing was sent.");
        }
        Verdict::Login => {
            let host = evaluation
                .resolved
                .as_ref()
                .map_or("", |r| r.host.as_str());
            ui.label(format!(
                "Log in to {host} to continue. Nothing is sent before the login."
            ));
        }
        Verdict::Same => boxed(
            ui,
            true,
            "Nothing to upload.",
            "As listed, the registry already has this image under this tag. Push checks it again and sends no blob.",
        ),
        Verdict::Move { .. } => boxed(
            ui,
            true,
            "Ready to push, after you confirm the move.",
            &format!("{size} Push asks before it moves {tag}."),
        ),
        Verdict::Ready => boxed(ui, true, "Ready to push.", &size),
        Verdict::Pending | Verdict::Unreadable | Verdict::Choose | Verdict::Bad => {
            ui.add(egui::Label::new(why).wrap());
        }
    }
}

/// The chooser's tree: the registries and namespaces of Preferences ›
/// Registries, as the Remotes tree draws them. Returns the place clicked.
fn chooser(ui: &mut egui::Ui, nodes: &[ChooserNode], current: Option<&Place>) -> Option<Place> {
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("push-chooser")
        .max_height(170.0)
        .show(ui, |ui| {
            for node in nodes {
                let namespace = node.place.namespace.is_some();
                let selected = current == Some(&node.place)
                    && (namespace || node.picks.as_ref() == Some(&node.place));
                ui.horizontal(|ui| {
                    if namespace {
                        ui.add_space(16.0);
                    }
                    let label = match &node.place.namespace {
                        Some(name) => format!("{name}/"),
                        None => node.place.host.clone(),
                    };
                    let mut atoms = egui::Atoms::new((
                        ui_kit::shell_icons::atom(if namespace {
                            Icon::Namespace
                        } else {
                            Icon::Registry
                        }),
                        label.clone(),
                    ));
                    if node.locked {
                        atoms.push_right(ui_kit::shell_icons::atom(Icon::Lock));
                    }
                    atoms.push_right(egui::RichText::new(&node.mark).small().weak());
                    let response = ui.selectable_label(selected, atoms);
                    let spoken = match &node.place.namespace {
                        Some(_) => format!("{label} on {} {}", node.place.host, node.mark),
                        None => format!("{label} {}", node.mark),
                    };
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::Button, true, selected, &spoken)
                    });
                    if response.clicked() {
                        picked = node.picks.clone();
                    }
                });
            }
        });
    picked
}

fn dry_run_table(ui: &mut egui::Ui, rows: &[DryRunRow]) {
    egui::ScrollArea::vertical()
        .id_salt("push-dry-run")
        .max_height(150.0)
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
                    for row in rows {
                        ui.label(&row.part);
                        ui.label(&row.kind);
                        ui.monospace(&row.digest);
                        ui.label(human_size(row.size));
                        pane::rights(ui, row.class == "open");
                        ui.label(&row.source);
                        ui.end_row();
                    }
                });
        });
}

/// Push to… of one item: the plan, the move question, or the run.
pub fn show_push(ctx: &egui::Context, dialog: &mut PushDialog, classic: bool) -> Action {
    let mut action = Action::None;
    let tag = dialog.image.clone().unwrap_or_default();
    let heading = match dialog.state {
        PushState::Pushing(_) | PushState::Finished { .. } => format!("Push — {tag}"),
        _ => format!("Push to… — {tag}"),
    };
    egui::Modal::new(egui::Id::unique("registry-push")).show(ctx, |ui| {
        let layout = ui_kit::skin::DialogLayout::new(
            ui,
            classic,
            (ctx.content_rect().width() - 48.0).clamp(360.0, 820.0),
        );
        title(ui, &heading, classic);
        match dialog.state {
            PushState::Planning | PushState::Ready => plan_one(ui, dialog, &mut action, &layout),
            PushState::ConfirmMove { .. } => confirm_move(ui, dialog, &mut action, &layout),
            PushState::Pushing(_) | PushState::Finished { .. } => {
                run_one(ui, dialog, &mut action, &layout)
            }
        }
    });
    if ctx.input_mut(ui_kit::skin::escape_pressed) {
        match dialog.state {
            PushState::Pushing(_) => dialog.cancel(),
            PushState::ConfirmMove { .. } => dialog.decline_move(),
            _ => action = Action::Close,
        }
    }
    action
}

fn plan_one(
    ui: &mut egui::Ui,
    dialog: &mut PushDialog,
    action: &mut Action,
    layout: &ui_kit::skin::DialogLayout,
) {
    layout.body(ui, |ui| {
    if let Some(notice) = &dialog.notice {
        warn(ui, notice.clone());
    }
    let evaluation = dialog.evaluate();
    let local = dialog.local().cloned();
    let tag = dialog.image.clone().unwrap_or_default();
    let plan = dialog.plan().cloned();
    egui::Grid::new("push-form")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(local.as_ref().map_or("Image", |l| l.kind_title()));
            ui.horizontal_wrapped(|ui| {
                if let Some(local) = &local {
                    ui_kit::shell_icons::show(ui, kind_icon(local.kind));
                }
                ui.monospace(&tag);
                if let Some(plan) = &plan {
                    ui.label("·");
                    ui.monospace(short(&plan.image.digest))
                        .on_hover_text(plan.image.digest.to_string());
                    ui.label(format!("· {}", human_size(plan.bytes())));
                    pane::rights(ui, plan.open());
                }
            });
            ui.end_row();
            ui.label("Local tag");
            ui.horizontal_wrapped(|ui| {
                ui.label("Stays");
                ui.monospace(&tag);
                ui.label("Pushing never renames it.");
            });
            ui.end_row();
            ui.label("Destination");
            ui.add(
                egui::TextEdit::singleline(&mut dialog.destination)
                    .desired_width(520.0)
                    .hint_text("registry/namespace/repository:tag"),
            );
            ui.end_row();
        });
    let host = match dialog.rule {
        Rule::Default => local
            .as_ref()
            .and_then(|l| l.pulled_from.clone())
            .unwrap_or_default(),
        _ => evaluation
            .resolved
            .as_ref()
            .map(|r| r.host.clone())
            .unwrap_or_default(),
    };
    small(
        ui,
        format!(
            "{} Type a reference here, or choose below; both show the same destination.",
            dialog.rule.sentence(&host)
        ),
    );
    let parts = dialog.parts();
    let nodes = dialog.rules.chooser();
    let mut picked = None;
    ui.columns(2, |columns| {
        columns[0].strong("Registry and namespace");
        picked = chooser(
            &mut columns[0],
            &nodes,
            parts.as_ref().map(|(place, _, _)| place),
        );
        let ui = &mut columns[1];
        let offered = dialog.offered();
        let label_width = ui.painter().layout_no_wrap(
            "Repository".into(),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        ).size().x;
        let resolved_width = (ui.available_width() - label_width - 10.0).max(1.0);
        egui::Grid::new("push-chooser-fields")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Repository");
                let mut sub = parts.as_ref().map(|(_, s, _)| s.clone()).unwrap_or_default();
                ui.horizontal(|ui| {
                    let field = ui.add_enabled(
                        parts.is_some(),
                        egui::TextEdit::singleline(&mut sub).desired_width(220.0),
                    );
                    if field.changed() {
                        dialog.set_repository(&sub);
                    }
                    if !offered.is_empty() {
                        ui.menu_button("Listed…", |ui| {
                            for name in &offered {
                                if ui.button(name).clicked() {
                                    dialog.set_repository(name);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                ui.end_row();
                ui.label("");
                let under = parts
                    .as_ref()
                    .and_then(|(place, _, _)| place.namespace.clone())
                    .map(|n| format!("Under {n}/. "))
                    .unwrap_or_default();
                small(
                    ui,
                    if offered.is_empty() {
                        format!("{under}A new name creates the repository.")
                    } else {
                        format!(
                            "{under}The field offers the {} repositories of this type listed there; a new name creates one.",
                            offered.len()
                        )
                    },
                );
                ui.end_row();
                ui.label("Tag");
                let mut version = parts.as_ref().map(|(_, _, t)| t.clone()).unwrap_or_default();
                if ui
                    .add_enabled(
                        parts.is_some(),
                        egui::TextEdit::singleline(&mut version).desired_width(220.0),
                    )
                    .changed()
                {
                    dialog.set_tag(&version);
                }
                ui.end_row();
                ui.label("Resolves to");
                match &evaluation.resolved {
                    Some(target) => {
                        if layout.bounded() {
                            ui.allocate_ui(egui::vec2(resolved_width, 0.0), |ui| {
                                ui.add(egui::Label::new(egui::RichText::new(target.to_string()).monospace()).wrap());
                            });
                        } else {
                            ui.monospace(target.to_string());
                        }
                    }
                    None => weak(ui, "— no registry chosen"),
                }
                ui.end_row();
            });
    });
    if let Some(place) = picked {
        dialog.pick(&place);
    }
    ui.separator();
    ui.strong("Dry run — every blob in the image, with its redistribution mark");
    if matches!(dialog.state, PushState::Planning) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading the image and the marks of every local image…");
        });
    } else if dialog.plan().is_some() {
        dry_run_table(ui, &dialog.dry_run());
    }
    ui.separator();
    ui.strong("Checks before upload");
    let evaluation = dialog.evaluate();
    if evaluation.checks.is_empty() {
        small(ui, "The checks appear once a destination is chosen.");
    } else if let Some(host) = checks(ui, &evaluation) {
        *action = Action::LogIn(host);
    }
    if !matches!(dialog.state, PushState::Planning) {
        verdict_box(ui, &evaluation, dialog.rules.private_place().as_ref());
    }
    });
    let evaluation = dialog.evaluate();
    layout.footer(ui, |ui| {
        ui.add_space(4.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                *action = Action::Close;
            }
            if ui
                .add_enabled(dialog.can_push(), pane::primary(ui, "Push"))
                .clicked()
            {
                if let Some(request) = dialog.push() {
                    *action = Action::Send(request);
                }
            }
            small(
                ui,
                match evaluation.verdict {
                    Verdict::Refused => "Push is disabled: the guard refused this destination.",
                    Verdict::TagTaken => "Push is disabled: this tag cannot move.",
                    _ => "Nothing has been sent.",
                },
            );
        });
    });
}

fn move_question(ui: &mut egui::Ui, target: &str, current: &str, new: &str, bounded: bool) {
    ui.strong("Move the tag to this image?");
    ui.add(
        egui::Label::new(format!(
            "{target} names another image on the registry. Pushing moves the tag; it is not a version tag, so it may move, and only away from the image shown here."
        ))
        .wrap(),
    );
    let digest_width = [current, new]
        .into_iter()
        .map(|text| {
            ui.painter()
                .layout_no_wrap(
                    text.into(),
                    egui::TextStyle::Monospace.resolve(ui.style()),
                    ui.visuals().text_color(),
                )
                .size()
                .x
        })
        .fold(0.0, f32::max);
    let bounded = bounded && digest_width + 100.0 > ui.available_width();
    let max_width = if bounded {
        ui.available_width() - 100.0
    } else {
        f32::INFINITY
    };
    egui::Grid::new("push-move")
        .max_col_width(max_width)
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("Now");
            if bounded {
                ui.add(egui::Label::new(egui::RichText::new(current).monospace()).wrap());
            } else {
                ui.monospace(current);
            }
            ui.end_row();
            ui.label("After the push");
            if bounded {
                ui.add(egui::Label::new(egui::RichText::new(new).monospace()).wrap());
            } else {
                ui.monospace(new);
            }
            ui.end_row();
        });
    small(
        ui,
        "The old image stays reachable by its digest and by its other tags. If the tag names yet another image when the push starts, you are asked again.",
    );
}

fn confirm_move(
    ui: &mut egui::Ui,
    dialog: &mut PushDialog,
    action: &mut Action,
    layout: &ui_kit::skin::DialogLayout,
) {
    let PushState::ConfirmMove {
        target,
        current,
        new,
    } = &dialog.state
    else {
        return;
    };
    layout.body(ui, |ui| {
        move_question(
            ui,
            &target.to_string(),
            &current.to_string(),
            &new.to_string(),
            layout.bounded(),
        );
    });
    let mut confirm = false;
    let mut back = false;
    layout.footer(ui, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            back = ui.button("Back").clicked();
            confirm = ui.button("Move tag").clicked();
        });
    });
    if confirm {
        if let Some(request) = dialog.confirm_move() {
            *action = Action::Send(request);
        }
    } else if back {
        dialog.decline_move();
    }
}

fn push_progress(ui: &mut egui::Ui, fraction: f32, status: String, separate_status: bool) {
    let bar = egui::ProgressBar::new(fraction);
    if separate_status {
        ui.label(status);
        ui.add(bar);
    } else {
        ui.add(bar.text(status));
    }
}

/// The blob list of one running or finished push, with the manifest last.
fn blob_rows(ui: &mut egui::Ui, progress: &Progress, finished: bool, separate_status: bool) {
    let blobs: Vec<BlobRow> = progress.blobs();
    let done = blobs.iter().filter(|row| row.outcome.is_some()).count();
    let bytes: u64 = blobs.iter().map(|row| row.size).sum();
    let sent: u64 = blobs.iter().map(|row| row.sent.min(row.size)).sum();
    let fraction = if progress.published {
        1.0
    } else {
        // The manifest is the last step, so the bar stops short of full.
        sent as f32 / (bytes.max(1) as f32) * 0.99
    };
    push_progress(
        ui,
        fraction,
        format!(
            "{done} of {} blobs · {} of {}",
            blobs.len(),
            human_size(sent),
            human_size(bytes)
        ),
        separate_status,
    );
    let current = blobs.iter().position(|row| row.outcome.is_none());
    egui::ScrollArea::new([separate_status, true])
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
                        ui.label(row.status(Some(index) == current, finished, progress.cancelling));
                        ui.end_row();
                    }
                    ui.label("Manifest and tag");
                    ui.label(progress.target.to_string());
                    ui.label(if progress.published {
                        "Written ✓"
                    } else if finished {
                        "Not written"
                    } else {
                        "Waiting — written after every blob"
                    });
                    ui.end_row();
                });
        });
}

/// "Remotes shows the tag now …", after a push: at once for a registry
/// this session listed, when it is first listed for one it did not.
fn after_line(ui: &mut egui::Ui, hosts: &[String], listing: &Listing, local: Option<&str>) {
    let mut text = String::new();
    let (shown, later): (Vec<&String>, Vec<&String>) = hosts
        .iter()
        .partition(|host| listing.registries.contains_key(*host));
    let join = |hosts: &[&String]| {
        hosts
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(" and ")
    };
    if !shown.is_empty() {
        text +=
            &format!(
            "Remotes shows {} now, and Source in the library names {}; neither needed a refresh. ",
            if shown.len() == 1 { "the tag" } else { "the tags" },
            join(&shown)
        );
    }
    if !later.is_empty() {
        text += &format!(
            "Source in the library names {}; Remotes shows the tag when that registry is first listed. ",
            join(&later)
        );
    }
    text += &match local {
        Some(tag) => format!("The local tag is still {tag}: pushing never renames."),
        None => "Local tags are as they were: pushing never renames.".into(),
    };
    small(ui, text);
}

fn run_one(
    ui: &mut egui::Ui,
    dialog: &mut PushDialog,
    action: &mut Action,
    layout: &ui_kit::skin::DialogLayout,
) {
    let (progress, finished) = match &dialog.state {
        PushState::Pushing(progress) => (progress, None),
        PushState::Finished {
            ok,
            message,
            progress,
        } => (progress, Some((*ok, message.clone()))),
        _ => return,
    };
    let cancelling = progress.cancelling;
    layout.body(ui, |ui| {
    ui.label(format!("To {}", progress.target));
    blob_rows(ui, progress, finished.is_some(), layout.bounded());
    let tag = dialog.image.clone().unwrap_or_default();
    match &finished {
        Some((true, message)) => {
            let heading = if message.starts_with("Moved ") {
                "Pushed, and the tag moved."
            } else if message.contains("nothing was uploaded") {
                "Nothing was uploaded."
            } else {
                "Pushed."
            };
            boxed(ui, true, heading, message);
            let hosts: Vec<String> = dialog.landed.iter().map(|r| r.host.clone()).collect();
            after_line(ui, &hosts, &dialog.rules.listing, Some(&tag));
        }
        Some((false, message)) => {
            let (heading, text) = match message.strip_prefix("Cancelled. ") {
                Some(rest) => ("Cancelled.", rest.to_owned()),
                None => match message.strip_prefix("Push refused. ") {
                    Some(rest) => ("Push refused.", rest.to_owned()),
                    None => ("Push failed.", message.clone()),
                },
            };
            boxed(ui, false, heading, &text);
        }
        None => small(
            ui,
            "Large blobs go up in chunks of about 64 MiB. Each accepted chunk is remembered, so an interrupted push resumes at the registry's offset instead of starting again. Cancel stops at the next blob or chunk; a request already sending, up to 64 MiB, finishes first. Blobs the registry already has are skipped. The tag is written last.",
        ),
    }
    });
    let shown = dialog.landed.first().cloned();
    layout.footer(ui, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if finished.is_some() {
                if ui.button("Close").clicked() {
                    *action = Action::Close;
                }
                if ui.button("Back to the dry run").clicked() {
                    dialog.back();
                }
                if let Some(target) = shown {
                    if ui.add(pane::primary(ui, "Show in Remotes")).clicked() {
                        *action = Action::ShowInRemotes(target);
                    }
                }
            } else {
                if ui
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
                if !cancelling {
                    small(ui, "Cancel or Escape stops at the next blob or chunk.");
                }
            }
        });
    });
}

/// Push to… of several items, or Push here….
pub fn show_push_batch(ctx: &egui::Context, batch: &mut PushBatch, classic: bool) -> Action {
    let mut action = Action::None;
    let here = batch.here().map(Place::prefix);
    let heading = match (&batch.state, &here) {
        (BatchState::ConfirmMove { index, .. }, _) => {
            format!("Push to… — {}", batch.items[*index].image.tag)
        }
        (BatchState::Run, _) => {
            let mut heading = match batch.queue.as_slice() {
                [one] => format!("Push — {}", batch.items[*one].image.tag),
                queue => format!("Push {} items", queue.len()),
            };
            if let Some(here) = &here {
                heading += &format!(" — to {here}");
            }
            heading
        }
        (_, Some(here)) => format!("Push here — {here}"),
        (_, None) => format!("Push {} items to…", batch.total()),
    };
    egui::Modal::new(egui::Id::unique("registry-push-batch")).show(ctx, |ui| {
        let layout = ui_kit::skin::DialogLayout::new(
            ui,
            classic,
            (ctx.content_rect().width() - 48.0).clamp(360.0, 860.0),
        );
        title(ui, &heading, classic);
        match &batch.state {
            BatchState::Planning => {
                layout.body(ui, |ui| {
                    batch_error(ui, batch);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading every image and the marks of every local image…");
                    });
                });
                layout.footer(ui, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            action = Action::Close;
                        }
                    });
                });
            }
            BatchState::Ready => {
                layout.body(ui, |ui| {
                    batch_error(ui, batch);
                    match batch.mode {
                        Mode::To => plan_many(ui, batch, &mut action),
                        Mode::Here(_) => plan_here(ui, batch, &mut action),
                    }
                });
                layout.footer(ui, |ui| {
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            action = Action::Close;
                        }
                        if ui
                            .add_enabled(batch.can_push(), pane::primary(ui, &batch.go_label()))
                            .clicked()
                        {
                            let requests = batch.start();
                            if !requests.is_empty() {
                                action = Action::SendAll(requests);
                            }
                        }
                        small(ui, batch.message());
                    });
                });
            }
            BatchState::ConfirmMove {
                index,
                current,
                new,
            } => {
                let target = batch.items[*index].reference();
                layout.body(ui, |ui| {
                    batch_error(ui, batch);
                    move_question(
                        ui,
                        &target,
                        &current.to_string(),
                        &new.to_string(),
                        layout.bounded(),
                    );
                });
                let mut confirm = false;
                let mut back = false;
                layout.footer(ui, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        back = ui.button("Back").clicked();
                        confirm = ui.button("Move tag").clicked();
                    });
                });
                if confirm {
                    action = Action::SendAll(batch.confirm_move());
                } else if back {
                    batch.decline_move();
                }
            }
            BatchState::Run => run_many(ui, batch, &mut action, &layout),
        }
    });
    if ctx.input_mut(ui_kit::skin::escape_pressed) {
        match batch.state {
            BatchState::Run if batch.running() => batch.cancel(),
            BatchState::ConfirmMove { .. } => batch.decline_move(),
            _ => action = Action::Close,
        }
    }
    action
}

fn plan_many(ui: &mut egui::Ui, batch: &mut PushBatch, action: &mut Action) {
    ui.add(
        egui::Label::new(
            "One after another under one progress bar, each with its own verdict. A tag that names a registry goes to that registry. The short and local ones go where you pick here. Local tags stay as they are: pushing never renames.",
        )
        .wrap(),
    );
    let (picks, own) = batch.counts();
    let nodes = batch.rules.chooser();
    let mut picked = None;
    ui.columns(2, |columns| {
        columns[0].strong(format!(
            "For the {picks} {}",
            if picks == 1 { "short tag" } else { "short tags" }
        ));
        picked = chooser(&mut columns[0], &nodes, batch.pick.as_ref());
        let ui = &mut columns[1];
        ui.strong("They go to");
        match &batch.pick {
            Some(place) => {
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(place.prefix());
                    ui.label("+ each one’s own path and tag");
                });
            }
            None => warn(ui, "Pick a registry and a namespace on the left"),
        }
        ui.strong(format!("The other {own}"));
        ui.add(
            egui::Label::new(
                "Name a registry, or came from the default one, and go there. The choice on the left does not move them.",
            )
            .wrap(),
        );
        small(
            ui,
            "To push one item under another repository or tag, or to confirm a tag’s move, push it alone.",
        );
    });
    if let Some(place) = picked {
        batch.choose(place);
    }
    egui::ScrollArea::vertical()
        .id_salt("push-items")
        .max_height(260.0)
        .show(ui, |ui| {
            for item in &batch.items {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(&item.image.tag);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        verdict_cell(ui, &item.evaluation.verdict, false);
                    });
                });
                let open = if item.open() { "open" } else { "user-supplied" };
                small(
                    ui,
                    match &item.target {
                        Some(target) => format!(
                            "→ {target} · {} · {} · {open}",
                            item.rule.word(),
                            human_size(item.size())
                        ),
                        None => format!(
                            "→ no registry picked yet · {} · {open}",
                            human_size(item.size())
                        ),
                    },
                );
                if let Some(reason) = item.evaluation.skip_reason() {
                    ui.horizontal_wrapped(|ui| {
                        warn(ui, format!("Skipped: {reason}."));
                        if item.evaluation.verdict == Verdict::Login
                            && ui.button("Log in…").clicked()
                        {
                            if let Some(target) = &item.target {
                                *action = Action::LogIn(target.host.clone());
                            }
                        }
                    });
                }
                if !item.evaluation.checks.is_empty() {
                    egui::CollapsingHeader::new("Checks")
                        .id_salt(("push-item-checks", &item.image.tag))
                        .show(ui, |ui| {
                            if let Some(host) = checks(ui, &item.evaluation) {
                                *action = Action::LogIn(host);
                            }
                        });
                }
            }
        });
    let ready: Vec<_> = batch
        .items
        .iter()
        .filter(|i| i.evaluation.verdict == Verdict::Ready)
        .collect();
    small(
        ui,
        format!(
            "Upload: at most {} in {} {}. Blobs the registry already has, or two items share, go up once or not at all.",
            human_size(ready.iter().map(|i| i.size()).sum()),
            ready.len(),
            if ready.len() == 1 { "item" } else { "items" }
        ),
    );
}

/// The verdict column, in the colours of the mock-up's states.
fn verdict_cell(ui: &mut egui::Ui, verdict: &Verdict, here: bool) {
    let colour = match verdict {
        Verdict::Ready => pane::good_colour(ui.visuals()),
        Verdict::Same | Verdict::Pending => ui.visuals().weak_text_color(),
        Verdict::Move { .. } | Verdict::Login | Verdict::Choose => ui.visuals().warn_fg_color,
        Verdict::Refused | Verdict::TagTaken | Verdict::Bad | Verdict::Unreadable => {
            ui.visuals().error_fg_color
        }
    };
    ui.label(egui::RichText::new(verdict.cell(here)).color(colour));
}

fn plan_here(ui: &mut egui::Ui, batch: &mut PushBatch, action: &mut Action) {
    let Some(fixed) = batch.here().cloned() else {
        return;
    };
    // A registry of several namespaces: the dialog asks which one, as the
    // chooser does.
    if batch.asks_namespace() {
        ui.strong(format!("Choose a namespace of {}", fixed.host));
        let nodes: Vec<ChooserNode> = batch
            .rules
            .chooser()
            .into_iter()
            .filter(|n| n.place.host == fixed.host && n.place.namespace.is_some())
            .collect();
        if let Some(place) = chooser(ui, &nodes, batch.pick.as_ref()) {
            batch.choose(place);
        }
    }
    let place = batch.here_place().unwrap_or(fixed);
    let entry = batch.rules.config.entry_or_public(&place.host);
    let private = match &place.namespace {
        Some(name) => batch
            .rules
            .namespaces(&entry)
            .iter()
            .any(|(n, private)| n == name && *private),
        None => entry.private,
    };
    egui::Grid::new("push-here-form")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("To");
            ui.horizontal_wrapped(|ui| {
                ui_kit::shell_icons::show(ui, Icon::Registry);
                ui.monospace(&place.host);
                match &place.namespace {
                    Some(name) => {
                        ui.label("›");
                        ui_kit::shell_icons::show(ui, Icon::Namespace);
                        ui.monospace(format!("{name}/"));
                    }
                    None => {
                        ui.label("· the whole registry");
                    }
                }
                pane::badge(
                    ui,
                    if private {
                        "private"
                    } else {
                        "public, open content only"
                    },
                    true,
                );
                match batch.rules.login(&place.host).map(|(user, _)| user) {
                    Some(user) => {
                        ui.label(egui::RichText::new(format!("· Login {user}")).small());
                    }
                    None => {
                        warn(ui, "· not signed in");
                        if ui.button("Log in…").clicked() {
                            *action = Action::LogIn(place.host.clone());
                        }
                    }
                }
            });
            ui.end_row();
            ui.label("Show");
            ui.horizontal_wrapped(|ui| {
                let label = |kind: Option<&str>| match kind {
                    None => "All types",
                    Some("media set") => "Installation media",
                    Some("firmware package") => "Firmware",
                    Some(_) => "Machine images",
                };
                egui::ComboBox::from_id_salt("push-here-type")
                    .selected_text(label(batch.kind))
                    .show_ui(ui, |ui| {
                        for kind in [
                            None,
                            Some("machine image"),
                            Some("media set"),
                            Some("firmware package"),
                        ] {
                            ui.selectable_value(&mut batch.kind, kind, label(kind));
                        }
                    });
                let cannot = batch.cannot();
                ui.checkbox(
                    &mut batch.show_all,
                    format!(
                        "and the {cannot} {} that cannot go here, with the reason",
                        if cannot == 1 { "item" } else { "items" }
                    ),
                );
            });
            ui.end_row();
        });
    let shown = batch.shown();
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("push-here-items")
        .max_height(220.0)
        .show(ui, |ui| {
            egui::Grid::new("push-here-grid")
                .num_columns(6)
                .striped(true)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    for heading in ["Local tag", "Source", "Type", "Licence", "Size", "Here"] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    if shown.is_empty() {
                        weak(ui, "Nothing in the library can go here.");
                        ui.end_row();
                    }
                    for &index in &shown {
                        let item = &batch.items[index];
                        let can = item.can_go_here();
                        let selected = batch.picked.contains(&item.image.tag);
                        let tag = item.image.tag.clone();
                        let row = ui.add(
                            egui::Button::selectable(
                                selected,
                                (
                                    ui_kit::shell_icons::atom(kind_icon(item.image.kind)),
                                    egui::RichText::new(&tag).monospace(),
                                ),
                            )
                            .sense(egui::Sense::click()),
                        );
                        let hover = if can {
                            format!("→ {}", item.reference())
                        } else {
                            item.evaluation.skip_reason().unwrap_or_default()
                        };
                        if row.on_hover_text(hover).clicked() {
                            let modifiers = ui.input(|i| i.modifiers);
                            clicked = Some((tag, modifiers.command, modifiers.shift));
                        }
                        let tone = |ui: &mut egui::Ui, text: String| {
                            if can {
                                ui.label(text);
                            } else {
                                weak(ui, text);
                            }
                        };
                        tone(ui, item.image.source_cell());
                        tone(ui, item.image.kind_title().into());
                        pane::rights(ui, item.open());
                        tone(ui, human_size(item.size()));
                        verdict_cell(ui, &item.evaluation.verdict, true);
                        ui.end_row();
                    }
                });
        });
    if let Some((tag, command, shift)) = clicked {
        batch.click(&tag, command, shift);
    }
    small(
        ui,
        format!(
            "Each selected item goes to {} under its own path and tag, and keeps its local tag. Ctrl (⌘ on macOS) or Shift selects several. For another repository or tag, use Push to… on the item.",
            place.prefix()
        ),
    );
    let focus = batch
        .focus
        .as_ref()
        .and_then(|tag| batch.items.iter().find(|i| i.image.tag == *tag))
        .or_else(|| {
            batch
                .picked
                .first()
                .and_then(|tag| batch.items.iter().find(|i| i.image.tag == *tag))
        })
        .or_else(|| shown.first().map(|&i| &batch.items[i]));
    if let Some(item) = focus {
        ui.strong(format!(
            "Checks — {} → {}",
            item.image.tag,
            item.reference()
        ));
        if let Some(host) = checks(ui, &item.evaluation) {
            *action = Action::LogIn(host);
        }
        let lead = match item.evaluation.verdict {
            Verdict::Same => Some("Already here: "),
            Verdict::Move { .. } => Some("Alone only: "),
            Verdict::Refused | Verdict::TagTaken => Some("Cannot go here: "),
            _ => None,
        };
        if let (Some(lead), Some(reason)) = (lead, item.evaluation.skip_reason()) {
            warn(ui, format!("{lead}{reason}."));
        }
    }
}

fn batch_error(ui: &mut egui::Ui, batch: &PushBatch) {
    if let Some(error) = &batch.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
}

fn run_many(
    ui: &mut egui::Ui,
    batch: &mut PushBatch,
    action: &mut Action,
    layout: &ui_kit::skin::DialogLayout,
) {
    let finished = batch.finished();
    layout.body(ui, |ui| {
    batch_error(ui, batch);
    if let [one] = batch.queue.as_slice() {
        let item = &batch.items[*one];
        match &item.run {
            Some(run) => {
                ui.label(format!("To {}", run.target));
                blob_rows(ui, run, finished, layout.bounded());
            }
            None => {
                if let Some(detail) = batch.detail(*one) {
                    warn(ui, detail);
                }
            }
        }
    } else if let Some((fraction, label)) = batch.progress() {
        ui.label(label);
        ui.add(egui::ProgressBar::new(fraction));
        egui::ScrollArea::vertical()
            .id_salt("push-run-items")
            .max_height(260.0)
            .show(ui, |ui| {
                for &index in &batch.queue {
                    let item = &batch.items[index];
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.strong(&item.image.tag);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let stage = batch.stage(index);
                            let colour = match stage {
                                Stage::Pushed => pane::good_colour(ui.visuals()),
                                Stage::Failed => ui.visuals().error_fg_color,
                                Stage::Skipped => ui.visuals().warn_fg_color,
                                _ => ui.visuals().text_color(),
                            };
                            ui.label(egui::RichText::new(batch.stage_label(index)).color(colour));
                        });
                    });
                    small(
                        ui,
                        format!(
                            "→ {} · {}",
                            if item.target.is_some() {
                                item.reference()
                            } else {
                                "no registry picked".into()
                            },
                            human_size(item.size())
                        ),
                    );
                    if let Some(detail) = batch.detail(index) {
                        warn(ui, detail);
                    }
                }
            });
    }
    if finished {
        if let Some(summary) = batch.summary() {
            boxed(
                ui,
                summary.complete || summary.heading.starts_with("Pushed"),
                &summary.heading,
                &summary.text,
            );
        }
        if let Some(note) = batch.failed_note() {
            ui.label(note);
        }
        let single = batch.queue.len() == 1;
        let local = single.then(|| batch.items[batch.queue[0]].image.tag.clone());
        after_line(ui, &batch.hosts(), &batch.rules.listing, local.as_deref());
    } else {
        small(
            ui,
            "Each item’s manifest and tag are written after every blob of that item. Cancel stops at the next blob or chunk.",
        );
    }
    });
    let shown = batch.first_landed().cloned();
    layout.footer(ui, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if finished {
                if ui.button("Close").clicked() {
                    *action = Action::Close;
                }
                if ui.button("Back to the dry run").clicked() {
                    batch.state = BatchState::Ready;
                }
                if let Some(target) = shown {
                    if ui.add(pane::primary(ui, "Show in Remotes")).clicked() {
                        *action = Action::ShowInRemotes(target);
                    }
                }
            } else {
                let cancelling = batch.cancelling();
                if ui
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
                    batch.cancel();
                }
                if !cancelling {
                    small(ui, "Cancel or Escape stops at the next blob or chunk.");
                }
            }
        });
    });
}

/// The Source and Push to… facts of a library item's details pane, inside
/// its facts grid: where it came from and was pushed to, and where Push to…
/// sends it by the Remotes rule.
pub fn item_facts(ui: &mut egui::Ui, image: &LocalImage, descriptor: &image_store::Descriptor) {
    pane::fact(ui, "Source", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(library_model::listing::source_cell(descriptor));
            let origin = if image.pulled_from.is_some() {
                "pulled from there"
            } else {
                "made in this library"
            };
            let pushed = if image.pushed.is_empty() {
                ""
            } else {
                ", then pushed"
            };
            ui.label(egui::RichText::new(format!("· {origin}{pushed}")).weak());
        });
    });
    pane::fact_text(ui, "Push to…", format!("Goes to {}.", image.rule_line()));
}

/// The pane's On registries section: each registry reference the item was
/// pushed to, when, and what that tag names as this session lists it.
/// Returns a reference clicked, to show in Remotes.
pub fn on_registries(
    ui: &mut egui::Ui,
    image: &LocalImage,
    descriptor: &image_store::Descriptor,
    listing: &Listing,
) -> Option<Reference> {
    pane::section(ui, "On registries");
    let pushes = library_model::listing::pushes(descriptor);
    if pushes.is_empty() {
        pane::meta(ui, "Not pushed from this library yet.");
        return None;
    }
    let mut clicked = None;
    egui::Grid::new("on-registries")
        .num_columns(3)
        .striped(true)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            for heading in ["Pushed to", "When", "The registry’s tag, as listed"] {
                ui.label(egui::RichText::new(heading).weak());
            }
            ui.end_row();
            for push in pushes.iter().rev() {
                let target =
                    Reference::parse(&push.reference, image_registry::DEFAULT_REGISTRY).ok();
                if ui
                    .link(egui::RichText::new(&push.reference).monospace())
                    .on_hover_text("Show in Remotes")
                    .clicked()
                {
                    clicked = target.clone();
                }
                ui.label(library_model::listing::utc(push.at));
                let now = target.as_ref().and_then(|t| {
                    let listed = listing.registries.get(&t.host)?;
                    Some(
                        listed
                            .repositories
                            .get(&t.repository)
                            .and_then(|tags| tags.get(t.tag.as_deref().unwrap_or("latest")))
                            .cloned(),
                    )
                });
                match now {
                    None => weak(ui, "Not listed in this session"),
                    Some(Some(Some(digest))) if digest == image.digest => {
                        ui.label(
                            egui::RichText::new("Names this image")
                                .color(pane::good_colour(ui.visuals())),
                        );
                    }
                    Some(Some(Some(_))) => warn(ui, "⚠ Names another image now"),
                    Some(Some(None)) => weak(ui, "Listed; not read yet"),
                    // A partial listing, or a tag deleted on the registry.
                    Some(None) => weak(ui, "Not in this session’s listing"),
                }
                ui.end_row();
            }
        });
    clicked
}

/// One row of the pane of several selected library items.
pub struct Selected {
    pub image: LocalImage,
    pub source: String,
    pub size: Option<u64>,
    pub open: Option<bool>,
}

/// What the pane of several selected items asked for.
pub enum SeveralAction {
    PushTo,
    Clear,
}

/// The pane of several library items selected with Ctrl or Shift: each with
/// its source, size, licence and where Push to… sends it.
pub fn several_items(ui: &mut egui::Ui, items: &[Selected]) -> Option<SeveralAction> {
    let mut action = None;
    pane::title(
        ui,
        kind_icon(items.first().map_or("machine image", |i| i.image.kind)),
        &format!("{} items selected", items.len()),
        false,
    );
    pane::meta(
        ui,
        "Ctrl (⌘ on macOS) adds or removes a row; Shift selects a range.",
    );
    egui::Grid::new("selected-items")
        .num_columns(5)
        .striped(true)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            for heading in ["Local tag", "Source", "Size", "Licence", "Push to… goes to"] {
                ui.label(egui::RichText::new(heading).weak());
            }
            ui.end_row();
            for item in items {
                ui.label(pane::mono(&item.image.tag));
                ui.label(&item.source);
                ui.label(item.size.map_or_else(|| "—".into(), human_size));
                match item.open {
                    Some(open) => pane::rights(ui, open),
                    None => {
                        ui.label("—");
                    }
                }
                ui.add(egui::Label::new(item.image.rule_line()).wrap());
                ui.end_row();
            }
        });
    pane::meta(
        ui,
        "Push to… takes them one after another under one progress bar, each with its own verdict. A tag that names a registry goes there; the short ones go to the registry and namespace picked once in the dialog. An item the guard refuses is skipped with the reason. Local tags stay as they are.",
    );
    pane::actions(ui, |ui| {
        if ui
            .add(pane::primary(
                ui,
                &format!("Push {} items to…", items.len()),
            ))
            .clicked()
        {
            action = Some(SeveralAction::PushTo);
        }
        if ui.button("Clear selection").clicked() {
            action = Some(SeveralAction::Clear);
        }
    });
    action
}

#[cfg(test)]
mod readability_tests {
    use super::*;

    #[test]
    fn classic_light_progress_status_is_above_the_unlabelled_bar() {
        let ctx = egui::Context::default();
        ctx.set_visuals(ui_kit::skin::skin_visuals(true, false));
        for fraction in [0.0, 0.2, 1.0] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                push_progress(ui, fraction, "1 of 5 blobs · 2 MiB of 10 MiB".into(), true);
            });
            output.textures_delta.clear();
            let mut status = None;
            let mut bar = None;
            for shape in &output.shapes {
                match &shape.shape {
                    egui::Shape::Text(text) => {
                        assert!(status.is_none(), "only the status label paints text");
                        assert_eq!(text.galley.text(), "1 of 5 blobs · 2 MiB of 10 MiB");
                        status = Some(text.galley.rect.translate(text.pos.to_vec2()));
                    }
                    egui::Shape::Rect(rect) if rect.fill == egui::Color32::WHITE => {
                        bar = Some(rect.rect)
                    }
                    _ => (),
                }
            }
            assert!(status.unwrap().bottom() < bar.unwrap().top());
        }
    }
}
