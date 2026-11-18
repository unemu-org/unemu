// SPDX-License-Identifier: AGPL-3.0-or-later
//! The one "Save as new image…" form, drawn: the Name fieldset and the Move
//! tag? and Version tag already exists questions; machine-images §§3 and 7,
//! `backlog/docs/design/mockups/save-as-image.html`. The form's model and its
//! tag rules are `library_model::save_image`, re-exported here under the old
//! path.

pub(crate) use library_model::save_image::*;

/// A dialog's message beside its mark, as the save-as-image mock-up lays out
/// its question and refusal dialogs.
fn alert(ui: &mut egui::Ui, icon: ui_kit::shell_icons::Icon, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui_kit::shell_icons::show(ui, icon);
        ui.vertical(body);
    });
}

fn small(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}

/// The Name fieldset: repository, tag with version chips, title, flatten.
pub(crate) fn form(ui: &mut egui::Ui, model: &mut SaveAsImage) {
    let mut repo = model.repo.clone();
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new("Repository name"));
        ui.text_edit_singleline(&mut repo);
    });
    model.set_repo(repo);
    small(
        ui,
        format!(
            "Keep {} for a new version, or name what the image now holds (a new name starts at 1.0.0).",
            model.source_repo()
        ),
    );
    let mut tag = model.tag.clone();
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new("Tag"));
        ui.text_edit_singleline(&mut tag);
    });
    ui.horizontal_wrapped(|ui| {
        for chip in model.chips() {
            if ui.button(&chip).clicked() {
                tag = chip;
            }
        }
    });
    model.set_tag(tag);
    let check = model.check();
    ui.add(
        egui::Label::new(if check.level == TagLevel::Ok {
            // The save-as-image mock-up checks a free tag off with ✓.
            egui::RichText::new(format!("✓ {}", check.text))
        } else {
            egui::RichText::new(&check.text).color(ui.visuals().warn_fg_color)
        })
        .wrap(),
    );
    if let Some(title) = &mut model.title {
        ui.horizontal(|ui| {
            ui.add_sized([130.0, 20.0], egui::Label::new("Title"));
            ui.text_edit_singleline(title);
        });
    }
    if let Some(flatten) = &mut model.flatten {
        ui.checkbox(flatten, "Flatten disk chains into new bases");
    }
}

/// The Move tag? and Version tag already exists questions. Returns a target
/// once a move is confirmed.
pub(crate) fn prompts(
    ctx: &egui::Context,
    model: &mut SaveAsImage,
    classic: bool,
) -> Option<Target> {
    let prompt = model.prompt.clone()?;
    let reference = model.reference();
    let mut answer = None;
    let mut close = false;
    let title = match &prompt {
        Prompt::Move(_) => format!("Move tag {reference}?"),
        Prompt::Refuse(_) => "Version tag already exists".to_owned(),
    };
    egui::Modal::new(egui::Id::unique("save-as-image-prompt")).show(ctx, |ui| {
        ui.set_width(500.0);
        if classic {
            ui_kit::skin::gradient_title(ui, &title);
        } else {
            ui.heading(&title);
        }
        match &prompt {
            Prompt::Move(current) => {
                // The save-as-image mock-up's question mark beside the text.
                alert(ui, ui_kit::shell_icons::Icon::Question, |ui| {
                    ui.label(format!(
                        "{reference} already names another image. Move it to the image you are saving?"
                    ));
                    ui.monospace(format!("Now:   {}", short(current)));
                    ui.label("After: the image saved now.");
                    let others: Vec<_> = model
                        .tags
                        .iter()
                        .filter(|(name, d)| d == current && name != &reference)
                        .map(|(name, _)| name.as_str())
                        .collect();
                    small(
                        ui,
                        format!(
                            "The old image stays in the store. It remains reachable by its digest{}. Workspaces that pinned it keep running it.",
                            if others.is_empty() {
                                String::new()
                            } else {
                                format!(" and by {}", others.join(", "))
                            }
                        ),
                    );
                    small(ui, "Only tags that are not versions can move. The move is a compare-and-swap: if another program changed the tag meanwhile, nothing moves and this question returns with the current digest.");
                });
                ui.horizontal(|ui| {
                    if ui.button("Move tag").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            }
            Prompt::Refuse(current) => {
                // The save-as-image mock-up's stop mark beside the refusal.
                alert(ui, ui_kit::shell_icons::Icon::Refusal, |ui| {
                    ui.label(format!("{reference} already names {}.", short(current)));
                    ui.label(format!(
                        "Tags shaped like a version (x.y.z) never move, so this image cannot use {}. There is no override.",
                        model.tag.trim()
                    ));
                    small(ui, "Use the next version, or a tag that is not a version — such as latest, which can move after a confirmation.");
                });
                ui.horizontal(|ui| {
                    if ui.button(format!("Use {}", model.suggested())).clicked() {
                        answer = Some(false);
                    }
                    if ui.button("Back").clicked() {
                        close = true;
                    }
                });
            }
        }
    });
    if close || ctx.input_mut(ui_kit::skin::escape_pressed) {
        model.dismiss();
        return None;
    }
    match answer {
        Some(true) => model.confirm_move(),
        Some(false) => {
            model.use_next();
            None
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_store::Digest;

    fn digest(label: &str) -> Digest {
        Digest::of(label.as_bytes())
    }
    fn tags() -> Vec<(String, Digest)> {
        vec![
            ("open-at:1.0.0".into(), digest("a")),
            ("open-at:latest".into(), digest("a")),
            ("open-at:2.0.0".into(), digest("b")),
            ("ibm-at:1.0.0".into(), digest("c")),
        ]
    }

    #[test]
    fn the_form_and_both_questions_render_without_a_window() {
        let ctx = egui::Context::default();
        let mut model = SaveAsImage::new("open-at:1.0.0", digest("a"), tags())
            .with_title(Some("Open AT"))
            .with_flatten();
        for prompt in [
            None,
            Some(Prompt::Move(digest("a"))),
            Some(Prompt::Refuse(digest("a"))),
        ] {
            model.prompt = prompt;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                form(ui, &mut model);
                assert!(prompts(ui.ctx(), &mut model, true).is_none());
            });
            output.textures_delta.clear();
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Repository name"))));
        }
    }
}
