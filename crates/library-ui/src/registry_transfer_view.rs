// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Pull by reference… and Pull… from the library's Remotes as modal
//! dialogs, following the approved registry-accounts and remotes mock-ups;
//! `backlog/docs/design/machine-images.md` §§5.1 and 5.5. Push to… and Push
//! here… are drawn by [`crate::push_view`] and re-exported here, beside the
//! actions every transfer dialog answers with. Rendering only: every
//! decision lives in [`library_model::registry_transfer`] and
//! [`library_model::remote_pull`].

pub use crate::push_view::{show_push, show_push_batch};
use image_registry::Reference;
use library_model::{
    listing::human_size,
    registry_transfer::{PullDialog, PullState, TransferRequest},
    remote_pull::{Drive, Mode, RemotePull, Stage},
};

/// The colour of a finished, good result.
pub(crate) const GOOD: egui::Color32 = egui::Color32::from_rgb(31, 107, 51);

pub enum Action {
    None,
    Close,
    Send(TransferRequest),
    /// Several jobs, which the worker runs in this order.
    SendAll(Vec<TransferRequest>),
    /// Show the pulled tag in the library.
    ShowPulled,
    /// New machine from image… on the pulled image.
    NewMachine,
    /// Preferences › Registries opens on this registry's Log in….
    LogIn(String),
    /// Show the tag a push published in Remotes.
    ShowInRemotes(Reference),
}

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

fn error(ui: &mut egui::Ui, text: &str) {
    let color = ui.visuals().error_fg_color;
    ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
}

pub fn show_pull(ctx: &egui::Context, dialog: &mut PullDialog, classic: bool) -> Action {
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
                        library_model::registry_transfer::kind_label(&dialog.facts, descriptor),
                        human_size(*have),
                        human_size(descriptor.size)
                    ));
                }
                for descriptor in live.fetched.iter().rev().take(6) {
                    ui.label(format!(
                        "{} · {} ✓",
                        library_model::registry_transfer::kind_label(&dialog.facts, descriptor),
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
    if ctx.input_mut(ui_kit::skin::escape_pressed) {
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
pub fn show_remote_pull(ctx: &egui::Context, dialog: &mut RemotePull, classic: bool) -> Action {
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
                        .map_or_else(String::new, library_model::registry_transfer::short),
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
    if ctx.input_mut(ui_kit::skin::escape_pressed) {
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
                        .map_or_else(String::new, library_model::registry_transfer::short),
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
