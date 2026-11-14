// SPDX-License-Identifier: AGPL-3.0-or-later
//! Classic property-sheet rendering over the headless settings model.
//! See `backlog/docs/design/machine-images.md` §7. All hardware choices and
//! explanations come from assembly capabilities; this module only lays them out.

use crate::settings_model::SettingsModel;
use assembly::capabilities::{self as caps, Domain, Severity, Value};

pub(crate) enum Action {
    None,
    Cancel,
    Save { close: bool },
}

pub(crate) fn show(ctx: &egui::Context, model: &mut SettingsModel, classic: bool) -> Action {
    let mut action = Action::None;
    egui::Modal::new(egui::Id::unique("machine-settings")).show(ctx, |ui| {
        ui.set_width(660.0);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 30.0).max(100.0);
            ui.allocate_ui(egui::vec2(width, 23.0), |ui| {
                let title = format!("Settings — {}", model.name);
                if classic {
                    crate::shell::gradient_title(ui, &title);
                } else {
                    ui.heading(title);
                }
            });
            if ui
                .add_sized([23.0, 23.0], egui::Button::new("×"))
                .on_hover_text("Cancel")
                .clicked()
            {
                action = Action::Cancel;
            }
        });
        if model.read_only {
            ui.label("The machine is running. Stop it before changing settings.");
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for tab in caps::Group::ALL {
                let response = ui.selectable_label(model.tab == tab, tab.label());
                if classic {
                    crate::shell::bevel(ui, response.rect, model.tab == tab);
                }
                if response.clicked() {
                    model.tab = tab;
                }
            }
        });
        let validation = model.validation();
        let mut edit = None;
        let response = egui::Frame::group(ui.style()).corner_radius(0).show(ui, |ui| {
            ui.set_min_height(180.0);
            egui::ScrollArea::vertical().id_salt("settings-fields").max_height(220.0).show(ui, |ui| {
                let mut heading = String::new();
                let mut fields: Vec<_> = caps::fields(&model.draft)
                    .into_iter()
                    .filter(|field| field.group == model.tab)
                    .collect();
                fields.sort_by_key(|field| {
                    (field.heading.clone(), !field.path.ends_with(".enabled"))
                });
                for field in fields {
                    if field.heading != heading {
                        ui.strong(&field.heading);
                        heading = field.heading.clone();
                    }
                    if let Some(value) = field_control(ui, model, &field) {
                        edit = Some((field.path.clone(), value));
                    }
                    for diagnostic in validation.diagnostics.iter().filter(|d| {
                        caps::diagnostic_affects_field(d, &field.path)
                    }) {
                        ui.add(egui::Label::new(&diagnostic.reason).wrap());
                    }
                }
                if model.tab == caps::Group::Storage {
                    for summary in caps::storage_summary(&model.draft) {
                        ui.label(summary);
                    }
                    ui.label("Media bindings and write policies are preserved unless changed.");
                }
                if model.tab == caps::Group::Firmware {
                    ui.label("Firmware is located by SHA-256 in ROM folders. Image bytes are checked before Start.");
                }
            });
        });
        if classic {
            crate::shell::bevel(ui, response.response.rect, true);
        }
        if let Some((path, value)) = edit {
            model.set(&path, value);
        }
        let validation = model.validation();
        if let Some(diagnostic) = validation
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
        {
            ui.add(egui::Label::new(format!("Cannot apply: {}", diagnostic.reason)).wrap());
        } else if validation
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Pending)
        {
            ui.label("Image verification is pending; saving settings does not authorize Start.");
        }
        ui.separator();
        ui.strong("Resulting machine file — review before OK / Apply");
        ui.label(model.target.file_name().unwrap_or_default().to_string_lossy());
        if let Some(content) = &model.current_content {
            ui.label("Current content");
            preview_pane(ui, "settings-current", content, 65.0);
            ui.label("Changes");
            preview_pane(ui, "settings-diff", &model.diff_preview(), 65.0);
        }
        preview_pane(ui, "settings-preview", &model.preview, 100.0);
        if model.current_content.is_some() {
            let response = ui.add_enabled(
                !model.read_only,
                egui::Checkbox::new(
                    &mut model.overwrite,
                    "Replace the existing local file shown above",
                ),
            );
            if response.changed() {
                model.error = None;
            }
        }
        if let Some(error) = &model.error {
            ui.add(egui::Label::new(error).wrap());
        }
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can_save = model.can_apply() && (model.current_content.is_none() || model.overwrite);
            if ui
                .add_enabled(can_save, egui::Button::new("Apply"))
                .clicked()
            {
                action = Action::Save { close: false };
            }
            if ui.button("Cancel").clicked() {
                action = Action::Cancel;
            }
            let can_accept = !validation.has_errors()
                && model.error.is_none()
                && (model.read_only || !model.dirty() || can_save);
            if ui
                .add_enabled(can_accept, egui::Button::new("OK"))
                .clicked()
            {
                action = if can_save {
                    Action::Save { close: true }
                } else {
                    Action::Cancel
                };
            }
        });
    });
    if ctx.input_mut(crate::shell::escape_pressed) {
        action = Action::Cancel;
    }
    action
}

fn preview_pane(ui: &mut egui::Ui, id: &str, text: &str, height: f32) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(height)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut { text })
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .interactive(false),
            );
        });
}

fn field_control(
    ui: &mut egui::Ui,
    model: &mut SettingsModel,
    field: &caps::Field,
) -> Option<Value> {
    let descriptor = caps::describe(&model.draft, &field.path);
    let mut edit = None;
    let mut reasons = Vec::new();
    ui.add_enabled_ui(!model.read_only, |ui| {
        ui.horizontal(|ui| {
            ui.add_sized([170.0, 20.0], egui::Label::new(&field.label));
            if let Some(descriptor) = descriptor {
                match &descriptor.domain {
                    Domain::Integer(domain) if (domain.max - domain.min) / domain.step > 256 => {
                        if let Value::Integer(value) = &field.value {
                            let mut value = *value;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut value)
                                        .range(domain.min..=domain.max)
                                        .speed(domain.step),
                                )
                                .changed()
                            {
                                edit = Some(Value::Integer(value));
                            }
                        }
                    }
                    _ => {
                        let choices = model.choices(&field.path).to_vec();
                        egui::ComboBox::from_id_salt(&field.path)
                            .width(260.0)
                            .selected_text(caps::value_label(
                                &model.draft,
                                &field.path,
                                &field.value,
                            ))
                            .show_ui(ui, |ui| {
                                for choice in &choices {
                                    let label =
                                        caps::value_label(&model.draft, &field.path, &choice.value);
                                    let response = ui.add_enabled(
                                        choice.enabled,
                                        egui::Button::selectable(
                                            choice.value == field.value,
                                            &label,
                                        ),
                                    );
                                    if response.clicked() {
                                        edit = Some(choice.value.clone());
                                    }
                                    if let Some(reason) = &choice.reason {
                                        ui.add(egui::Label::new(reason).wrap());
                                        reasons.push(format!("{label}: {reason}"));
                                    }
                                }
                            });
                        for choice in choices.iter().filter(|choice| !choice.enabled) {
                            if let Some(reason) = &choice.reason {
                                reasons.push(format!(
                                    "{}: {reason}",
                                    caps::value_label(&model.draft, &field.path, &choice.value)
                                ));
                            }
                        }
                    }
                }
                if let Some(unit) = descriptor.unit {
                    ui.label(unit);
                }
            } else {
                ui.label(caps::value_label(&model.draft, &field.path, &field.value));
            }
        });
    });
    reasons.sort();
    reasons.dedup();
    for reason in reasons {
        ui.add(egui::Label::new(egui::RichText::new(reason).small()).wrap());
    }
    edit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_and_invalid_reason_render_without_a_window() {
        let temp = assembly::test_support::TempDir::new("settings-render");
        std::fs::create_dir(temp.path().join("machines")).unwrap();
        let path = temp.path().join("machines/example.toml");
        std::fs::write(&path, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='missing.bin'\n[[card]]\nkind='sb16'\n[[card]]\nkind='lotech-ems'\nframe=0xD000\n").unwrap();
        let mut model = SettingsModel::open(&path, false).unwrap();
        let ctx = egui::Context::default();
        for tab in caps::Group::ALL {
            model.tab = tab;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(matches!(show(ui.ctx(), &mut model, true), Action::None));
            });
            output.textures_delta.clear();
        }
        model.set("card[sb16].base", Value::Integer(0x260));
        let reason = model
            .validation()
            .diagnostics
            .into_iter()
            .find(|d| d.severity == Severity::Error)
            .unwrap()
            .reason;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = show(ui.ctx(), &mut model, true);
        });
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains(&reason))));
        output.textures_delta.clear();
        assert!(!model.can_apply());
    }
    #[test]
    fn fix_round_escape_and_title_close_cancel_without_saving() {
        let temp = assembly::test_support::TempDir::new("sheet-close");
        std::fs::create_dir(temp.path().join("machines")).unwrap();
        let path = temp.path().join("machines/example.toml");
        std::fs::write(
            &path,
            "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='missing.bin'\n",
        )
        .unwrap();
        let mut model = SettingsModel::open(&path, false).unwrap();
        model.set("memory.extended_kib", Value::Integer(1024));
        let ctx = egui::Context::default();
        let mut frame = |events| {
            let mut action = Action::None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = show(ui.ctx(), &mut model, true);
                },
            );
            output.textures_delta.clear();
            (action, output)
        };
        let _ = frame(vec![]);
        let (_, output) = frame(vec![]);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "IBM 5170 (AT)")));
        let (action, _) = frame(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }]);
        assert!(matches!(action, Action::Cancel));
        let (_, output) = frame(vec![]);
        let close = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    (text.galley.text() == "×").then(|| text.pos + text.galley.size() * 0.5)
                } else {
                    None
                }
            })
            .unwrap();
        let _ = frame(vec![
            egui::Event::PointerMoved(close),
            egui::Event::PointerButton {
                pos: close,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        let (action, _) = frame(vec![egui::Event::PointerButton {
            pos: close,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert!(matches!(action, Action::Cancel));
        assert!(!model.target.exists());
    }
}
