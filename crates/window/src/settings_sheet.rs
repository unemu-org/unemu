// SPDX-License-Identifier: AGPL-3.0-or-later
//! Classic property-sheet rendering over the headless settings models: a
//! machine file's local child, and Settings on an image with its review of
//! the new image revision (`backlog/docs/design/mockups/save-as-image.html`).
//! See `backlog/docs/design/machine-images.md` §7. All hardware choices and
//! explanations come from assembly capabilities; this module only lays them out.

use crate::{
    image_settings::ImageSettings,
    save_image::{self, TagLevel, Target},
    settings_model::SettingsModel,
};
use assembly::capabilities::{self as caps, Domain, Draft, Severity, Value};

/// What a field control needs from its settings model.
pub(crate) trait Sheet {
    fn draft(&self) -> &Draft;
    fn choices(&mut self, path: &str) -> Vec<caps::Choice>;
    fn read_only(&self) -> bool;
}
impl Sheet for SettingsModel {
    fn draft(&self) -> &Draft {
        &self.draft
    }
    fn choices(&mut self, path: &str) -> Vec<caps::Choice> {
        SettingsModel::choices(self, path).to_vec()
    }
    fn read_only(&self) -> bool {
        self.read_only
    }
}
impl Sheet for ImageSettings {
    fn draft(&self) -> &Draft {
        &self.draft
    }
    fn choices(&mut self, path: &str) -> Vec<caps::Choice> {
        ImageSettings::choices(self, path).to_vec()
    }
    fn read_only(&self) -> bool {
        self.read_only
    }
}

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

pub(crate) enum ImageAction {
    None,
    Cancel,
    Review,
}

fn title_bar(ui: &mut egui::Ui, title: &str, classic: bool) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 30.0).max(100.0);
        ui.allocate_ui(egui::vec2(width, 23.0), |ui| {
            if classic {
                crate::shell::gradient_title(ui, title);
            } else {
                ui.heading(title);
            }
        });
        close = ui
            .add_sized([23.0, 23.0], egui::Button::new("×"))
            .on_hover_text("Cancel")
            .clicked();
    });
    close
}

fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}

fn floppy_label(kind: Option<&str>) -> String {
    match kind {
        None => "Not installed".into(),
        Some(kind @ ("1.2M" | "360K")) => format!("5.25\" {kind}"),
        Some(kind) => format!("3.5\" {kind}"),
    }
}

/// The `Settings — repository:tag (image)` property sheet over an image's hardware.
pub(crate) fn show_image(
    ctx: &egui::Context,
    model: &mut ImageSettings,
    classic: bool,
) -> ImageAction {
    let mut action = ImageAction::None;
    let mut close = false;
    let reference = model.row.reference.clone();
    let digest = save_image::short(&model.row.descriptor.digest);
    egui::Modal::new(egui::Id::unique("image-settings")).show(ctx, |ui| {
        ui.set_width(680.0);
        if title_bar(ui, &format!("Settings — {reference} (image)"), classic) {
            close = true;
        }
        ui.strong("Changes create a new image revision; disks are shared.");
        hint(ui, format!("The image {digest} itself never changes. Its disks, firmware and media are reused as they are; only the configuration is new."));
        if model.source_tag_moves() {
            hint(ui, format!("{reference} is not a version tag: publishing under it moves it to the revision after a confirmation, and the old image stays reachable by its digest."));
        }
        if model.read_only {
            ui.label("This image is running. Stop it before changing its settings.");
        }
        let changes = model.changes();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for tab in caps::Group::ALL {
                let changed = changes.iter().any(|(field, _)| field.group == tab);
                let label = if changed {
                    format!("{} ●", tab.label())
                } else {
                    tab.label().to_owned()
                };
                let response = ui.selectable_label(model.tab == tab, label);
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
        let mut floppy = None;
        let response = egui::Frame::group(ui.style()).corner_radius(0).show(ui, |ui| {
            ui.set_min_height(180.0);
            egui::ScrollArea::vertical().id_salt("image-settings-fields").max_height(260.0).show(ui, |ui| {
                let mut heading = String::new();
                let mut fields: Vec<_> = caps::fields(&model.draft)
                    .into_iter()
                    .filter(|field| field.group == model.tab && !field.path.contains(".drives[unit="))
                    .collect();
                fields.sort_by_key(|field| (field.heading.clone(), !field.path.ends_with(".enabled")));
                for field in fields {
                    if field.heading != heading {
                        ui.strong(&field.heading);
                        heading = field.heading.clone();
                    }
                    if field.group == caps::Group::Firmware {
                        ui.horizontal(|ui| {
                            ui.add_sized([170.0, 20.0], egui::Label::new(&field.label));
                            ui.label(caps::value_label(&model.draft, &field.path, &field.value));
                        });
                        continue;
                    }
                    if let Some(value) = field_control(ui, model, &field) {
                        edit = Some((field.path.clone(), value));
                    }
                    if let Some((_, Some(old))) = changes.iter().find(|(f, _)| f.path == field.path) {
                        hint(ui, format!("Was: {}", caps::value_label(&model.draft, &field.path, old)));
                    }
                    for diagnostic in validation.diagnostics.iter().filter(|d| caps::diagnostic_affects_field(d, &field.path)) {
                        ui.add(egui::Label::new(&diagnostic.reason).wrap());
                    }
                }
                if model.tab == caps::Group::Storage {
                    ui.strong("Diskette drives");
                    for unit in 0..2u8 {
                        let current = model.floppy(unit);
                        ui.add_enabled_ui(!model.read_only, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_sized([170.0, 20.0], egui::Label::new(format!("Drive {}:", char::from(b'A' + unit))));
                                egui::ComboBox::from_id_salt(("image-floppy", unit))
                                    .width(260.0)
                                    .selected_text(floppy_label(current.as_deref()))
                                    .show_ui(ui, |ui| {
                                        for (kind, reason) in model.floppy_choices(unit) {
                                            let label = format!("{}{}", floppy_label(kind.as_deref()), if reason.is_some() { " — unavailable" } else { "" });
                                            if ui.add_enabled(reason.is_none(), egui::Button::selectable(kind == current, label)).clicked() {
                                                floppy = Some((unit, kind.clone()));
                                            }
                                            if let Some(reason) = reason {
                                                ui.add(egui::Label::new(reason).wrap());
                                            }
                                        }
                                    });
                            });
                        });
                    }
                    hint(ui, "A drive added here starts with an empty media catalogue. A drive whose catalogue holds media keeps its type.");
                    for summary in caps::storage_summary(&model.draft) {
                        ui.label(summary);
                    }
                    hint(ui, "Disk contents are shared and cannot change here; use Disk layers… on a running workspace.");
                }
                if model.tab == caps::Group::Firmware {
                    hint(ui, "Firmware stays as the image stores it; the revision binds the same ROM blobs.");
                }
            });
        });
        if classic {
            crate::shell::bevel(ui, response.response.rect, true);
        }
        if let Some((path, value)) = edit {
            model.set(&path, value);
        }
        if let Some((unit, kind)) = floppy {
            model.set_floppy(unit, kind.as_deref());
        }
        if let Some(diagnostic) = model
            .validation()
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
        {
            ui.add(egui::Label::new(format!("Cannot review: {}", diagnostic.reason)).wrap());
        }
        if let Some(error) = &model.error {
            ui.add(egui::Label::new(error).wrap());
        }
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui
                .add_enabled(model.can_review(), egui::Button::new("Review changes…"))
                .clicked()
            {
                action = ImageAction::Review;
            }
            let count = model.changes().len();
            ui.label(match count {
                0 if model.dirty() => "Drive changes — a new revision on review".to_owned(),
                0 => "No changes".to_owned(),
                1 => "1 change — a new revision on review".to_owned(),
                n => format!("{n} changes — a new revision on review"),
            });
        });
    });
    if model.discard_prompt {
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("image-settings-discard")).show(ctx, |ui| {
            ui.set_width(420.0);
            if classic {
                crate::shell::gradient_title(ui, "Discard changes?");
            } else {
                ui.heading("Discard changes?");
            }
            ui.label(format!(
                "The changes to {reference} are not published. Discard them?"
            ));
            ui.horizontal(|ui| {
                if ui.button("Discard changes").clicked() {
                    answer = Some(true);
                }
                if ui.button("Keep editing").clicked() {
                    answer = Some(false);
                }
            });
        });
        if ctx.input_mut(crate::shell::escape_pressed) {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                model.discard_prompt = false;
                action = ImageAction::Cancel;
            }
            Some(false) => model.discard_prompt = false,
            None => (),
        }
        return action;
    }
    if (close || ctx.input_mut(crate::shell::escape_pressed)) && model.request_close() {
        action = ImageAction::Cancel;
    }
    action
}

pub(crate) enum ReviewAction {
    None,
    Back,
    Cancel,
    Publish(Box<pc_image::revision::Plan>, Target),
}

fn blob_kind(descriptor: &image_store::Descriptor) -> &'static str {
    use image_store::media_type as mt;
    match descriptor.decoded_media_type() {
        mt::DISK_BASE => "disk base",
        mt::DISK_LAYER => "disk layer",
        mt::DISK_SECTORS => "disk sectors",
        mt::FLOPPY => "diskette",
        mt::FIRMWARE => "firmware ROM",
        mt::METADATA => "metadata",
        mt::PNG | mt::JPEG => "artwork",
        pc_image::CMOS => "CMOS image",
        pc_image::DISK_ZERO => "zero base",
        _ => "blob",
    }
}

/// Publish new image revision: the configuration diff, the CMOS bytes SETUP
/// would rewrite, the bindings and the name, confirmed before anything is written.
pub(crate) fn show_review(
    ctx: &egui::Context,
    model: &mut ImageSettings,
    busy: bool,
    classic: bool,
) -> ReviewAction {
    let mut action = ReviewAction::None;
    let reference = model.row.reference.clone();
    let digest = model.row.descriptor.digest.clone();
    let stores_cmos = model.stores_cmos();
    let Some(review) = &mut model.review else {
        return action;
    };
    let mut publish = false;
    egui::Modal::new(egui::Id::unique("image-revision")).show(ctx, |ui| {
        ui.set_width(680.0);
        if title_bar(ui, "Publish new image revision", classic) {
            action = ReviewAction::Cancel;
        }
        ui.label(format!("From {reference} {} · stopped", save_image::short(&digest)));
        ui.strong("New revision");
        save_image::form(ui, &mut review.save);
        // Scrolled content takes the full height at once; offered the rest of
        // last frame's dialog, it would grow the modal frame by frame.
        egui::ScrollArea::vertical().id_salt("image-revision-review").max_height(330.0).min_scrolled_height(330.0).show(ui, |ui| {
            match &review.plan {
                None => {
                    ui.label("Checking the revision with the image's firmware…");
                }
                Some(Err(error)) => {
                    ui.add(egui::Label::new(format!("Cannot publish: {error}")).wrap());
                }
                Some(Ok(plan)) => {
                    ui.strong("Hardware changes — flattened machine configuration");
                    let mut diff = String::new();
                    for line in &plan.diff {
                        match line {
                            pc_image::revision::DiffLine::Section(name) => diff.push_str(&format!("  {name}\n")),
                            pc_image::revision::DiffLine::Removed(text) => diff.push_str(&format!("- {text}\n")),
                            pc_image::revision::DiffLine::Added(text) => diff.push_str(&format!("+ {text}\n")),
                        }
                    }
                    preview_pane(ui, "image-revision-diff", &diff, 120.0);
                    ui.strong("CMOS bytes recomputed, as SETUP would");
                    if let Some(note) = &plan.cmos_note {
                        ui.add(egui::Label::new(note).wrap());
                    } else if plan.cmos_changes.is_empty() {
                        ui.monospace("  (no memory or drive change: the CMOS image stays as it is)");
                    } else {
                        if !stores_cmos {
                            hint(ui, "This image stores no CMOS image; Start derives these bytes from the configuration.");
                        }
                        let text: String = plan
                            .cmos_changes
                            .iter()
                            .map(|c| format!("{:02X}h  {:02X} → {:02X}   {}\n", c.offset, c.old, c.new, c.field))
                            .collect();
                        preview_pane(ui, "image-revision-cmos", &text, 80.0);
                    }
                    hint(ui, "Only the bytes that describe memory, drives, the display and the coprocessor change, plus the checksum over 10h–2Dh. The clock and every other option keep their stored values.");
                    ui.strong("Image and media bindings");
                    for d in &plan.shared {
                        ui.monospace(format!("{:<13} {}  unchanged, shared", blob_kind(d), save_image::short(&d.digest)));
                    }
                    for d in &plan.added {
                        ui.monospace(format!("{:<13} {}  new", blob_kind(d), save_image::short(&d.digest)));
                    }
                    ui.label(format!(
                        "New in the store: one configuration blob{}, plus the manifest. 0 disk bytes copied.",
                        if plan.added.is_empty() { String::new() } else { format!(" and {} other blob(s)", plan.added.len()) }
                    ));
                    ui.monospace(format!(
                        "Machine digest  {} → {}",
                        save_image::short(&plan.machine.0),
                        save_image::short(&plan.machine.1)
                    ));
                    ui.label("Validator ✓ The draft passes validation; no pending checks.");
                    hint(ui, format!("The new settings apply when the new revision starts. Workspaces of {reference} keep running that image."));
                }
            }
        });
        if let Some(error) = &review.error {
            ui.add(egui::Label::new(error).wrap());
        }
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                action = ReviewAction::Cancel;
            }
            if ui.button("< Back").clicked() {
                action = ReviewAction::Back;
            }
            let check = review.save.check();
            let ready = !busy
                && matches!(review.plan, Some(Ok(_)))
                && (check.level != TagLevel::Bad || check.taken.is_some());
            if ui
                .add_enabled(ready, egui::Button::new("Publish revision"))
                .clicked()
            {
                publish = true;
            }
        });
    });
    let confirmed = save_image::prompts(ctx, &mut review.save, classic);
    let target = if publish {
        review.save.submit()
    } else {
        confirmed
    };
    if let (Some(target), Some(Ok(plan))) = (target, &review.plan) {
        review.error = None;
        action = ReviewAction::Publish(Box::new(plan.clone()), target);
    } else if review.save.prompt.is_none()
        && matches!(action, ReviewAction::None)
        && ctx.input_mut(crate::shell::escape_pressed)
    {
        action = ReviewAction::Back;
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

fn field_control(ui: &mut egui::Ui, model: &mut impl Sheet, field: &caps::Field) -> Option<Value> {
    let descriptor = caps::describe(model.draft(), &field.path);
    let mut edit = None;
    let mut reasons = Vec::new();
    ui.add_enabled_ui(!model.read_only(), |ui| {
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
                        let choices = model.choices(&field.path);
                        egui::ComboBox::from_id_salt(&field.path)
                            .width(260.0)
                            .selected_text(caps::value_label(
                                model.draft(),
                                &field.path,
                                &field.value,
                            ))
                            .show_ui(ui, |ui| {
                                for choice in &choices {
                                    let label = caps::value_label(
                                        model.draft(),
                                        &field.path,
                                        &choice.value,
                                    );
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
                                    caps::value_label(model.draft(), &field.path, &choice.value)
                                ));
                            }
                        }
                    }
                }
                if let Some(unit) = descriptor.unit {
                    ui.label(unit);
                }
            } else {
                ui.label(caps::value_label(model.draft(), &field.path, &field.value));
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
