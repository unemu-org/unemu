// SPDX-License-Identifier: AGPL-3.0-or-later
//! Classic property-sheet rendering over the headless settings models: a
//! machine file's local child, and Settings on an image with its review of
//! the new image revision (`backlog/docs/design/mockups/save-as-image.html`).
//! See `backlog/docs/design/machine-images.md` §7. All hardware choices and
//! explanations come from assembly capabilities; this module only lays them out.

use crate::{
    firmware_choices::{self as fw, Slot},
    image_settings::{CmosLine, FirmwareChoice, ImageSettings},
    new_machine_view::why_disabled,
    save_image::{self, TagLevel, Target},
    settings_model::SettingsModel,
};
use assembly::capabilities::{self as caps, Domain, Draft, Severity, Value};
use assembly::CmosLayout;

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
    /// Settings… on a machine: keep the draft as its settings; `true`
    /// closes the sheet afterwards (OK), `false` keeps it open (Apply).
    Apply(bool),
    /// Import ROM folder… for a ROM that is not in the store, over the sheet.
    ImportRoms,
}

/// An edit the Firmware tab asked for.
pub(crate) enum FirmwareEdit {
    Set(Slot, &'static str),
    ImportRoms,
}

/// What the catalogue's status means, as the Firmware tab says it.
fn status_text(entry: &assembly::FirmwareEntry) -> &'static str {
    match entry.status {
        assembly::FirmwareStatus::Tested => "Tested: run by the project's golden tests",
        assembly::FirmwareStatus::Listed => "Listed: identified, not yet run by a golden test",
        assembly::FirmwareStatus::Superseded => {
            "Superseded: kept so that machines made with it still start"
        }
    }
}

fn choice_label(choice: &FirmwareChoice) -> String {
    format!(
        "{}{} — {}",
        fw::name(choice.entry),
        if choice.entry.superseded_by.is_some() {
            " (superseded)"
        } else {
            ""
        },
        match (&choice.reason, choice.open) {
            (Some(_), _) => "unavailable",
            (None, true) => "open",
            (None, false) => "user-supplied",
        }
    )
}

fn notice(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(
        egui::Label::new(egui::RichText::new(text.into()).color(ui.visuals().warn_fg_color)).wrap(),
    );
}

/// An entry that cannot be chosen now, with its reason, and Import ROM
/// folder… when its ROM is not in the store; returns whether that was clicked.
fn unavailable_line(ui: &mut egui::Ui, choice: &FirmwareChoice, read_only: bool) -> bool {
    let mut import = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} — {}",
                fw::name(choice.entry),
                choice.reason.as_deref().unwrap_or_default()
            ))
            .small(),
        );
        import = choice.rom.is_none()
            && ui
                .add_enabled(!read_only, egui::Link::new("Import ROM folder…"))
                .clicked();
    });
    import
}

/// The Firmware tab: a System BIOS choice, a Video BIOS choice for a card
/// with a BIOS of its own, and the option ROMs, which follow their cards.
/// Every choice is a catalogue entry that fits the board; an entry that
/// cannot be chosen says why.
pub(crate) fn firmware_tab(
    ui: &mut egui::Ui,
    model: &mut ImageSettings,
    machine: bool,
) -> Option<FirmwareEdit> {
    let mut edit = None;
    for slot in Slot::ALL {
        ui.strong(slot.label());
        if slot == Slot::Video && !model.has_video_bios() {
            ui.horizontal(|ui| {
                ui.add_sized([170.0, 20.0], egui::Label::new(slot.label()));
                ui.label("None");
            });
            hint(
                ui,
                "The card has no BIOS of its own; the system BIOS drives it.",
            );
            continue;
        }
        let choices = model.firmware_choices(slot);
        let current = model.firmware(slot);
        ui.add_enabled_ui(!model.read_only && model.packages_read(), |ui| {
            ui.horizontal(|ui| {
                ui.add_sized([170.0, 20.0], egui::Label::new(slot.label()));
                let selected = current.map_or_else(
                    || {
                        if fw::rom_table(model.machine(), slot).is_some() {
                            "An unlisted ROM".to_owned()
                        } else {
                            "None".to_owned()
                        }
                    },
                    |entry| {
                        choices
                            .iter()
                            .find(|c| c.entry.id == entry.id)
                            .map_or_else(|| fw::name(entry), choice_label)
                    },
                );
                egui::ComboBox::from_id_salt(("firmware-choice", slot))
                    .width(320.0)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        let mut family = None;
                        for choice in &choices {
                            if slot == Slot::System && family != Some(choice.entry.cmos_layout) {
                                family = Some(choice.entry.cmos_layout);
                                ui.label(
                                    egui::RichText::new(fw::family(choice.entry.cmos_layout))
                                        .strong(),
                                );
                            }
                            let chosen = current.is_some_and(|c| c.id == choice.entry.id);
                            let entry = ui.add_enabled(
                                choice.reason.is_none(),
                                egui::Button::selectable(chosen, choice_label(choice)),
                            );
                            if why_disabled(entry, choice.reason.as_deref()).clicked() && !chosen {
                                edit = Some(FirmwareEdit::Set(slot, choice.entry.id));
                            }
                        }
                    });
            });
        });
        if let Some(entry) = current {
            let rights = choices
                .iter()
                .find(|c| c.entry.id == entry.id)
                .map_or_else(|| fw::catalogue_open(entry), |c| c.open);
            let licence = match entry.distribution {
                assembly::FirmwareDistribution::Open { license, .. } if rights => {
                    format!("open, {license}, with its source archive")
                }
                _ => "user-supplied".into(),
            };
            hint(
                ui,
                format!(
                    "{} · {licence} · {} KiB · SHA-256 {}…{}",
                    status_text(entry),
                    entry.size / 1024,
                    &entry.sha256[..8],
                    &entry.sha256[entry.sha256.len() - 4..]
                ),
            );
            if let Some(rom) = choices
                .iter()
                .find(|c| c.entry.id == entry.id)
                .and_then(|c| c.rom.as_ref())
            {
                hint(ui, format!("In your store: {}.", rom.package.reference));
            }
            if let Some((by, reason)) = fw::replacement(entry) {
                let replacement = choices.iter().find(|c| c.entry.id == by.id);
                let blocked = replacement.and_then(|c| c.reason.as_deref());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Superseded by {} ({reason}).",
                            fw::release(by)
                        ))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                    );
                    let usable = replacement.is_some_and(|c| c.reason.is_none());
                    let link = ui.add_enabled(
                        usable && !model.read_only,
                        egui::Link::new(format!("Use {}", fw::release(by))),
                    );
                    if why_disabled(link, blocked).clicked() {
                        edit = Some(FirmwareEdit::Set(slot, by.id));
                    }
                });
                // Say why Use is greyed right under it: the list further down
                // may be scrolled out of view.
                if let Some(choice) =
                    replacement.filter(|_| blocked.is_some() && model.packages_read())
                {
                    if unavailable_line(ui, choice, model.read_only) {
                        edit = Some(FirmwareEdit::ImportRoms);
                    }
                }
            }
        }
        if slot == Slot::System {
            match model.cmos_line() {
                None => (),
                Some(CmosLine::Same(layout)) => hint(ui, format!(
                    "✓ Same CMOS layout ({}): {} CMOS stays as it is.",
                    layout.map_or("unlisted", CmosLayout::label),
                    if machine { "this machine's" } else { "the image's" }
                )),
                Some(CmosLine::Derived(layout)) => hint(ui, format!(
                    "Another CMOS layout ({}). This image derives its CMOS from the configuration at every start, so nothing is reset.",
                    layout.map_or("unlisted", CmosLayout::label)
                )),
                Some(CmosLine::Reset { from, to }) => notice(ui, format!(
                    "⚠ CMOS settings are reset at the next start. {} {}",
                    match (from, to) {
                        (Some(from), Some(to)) => format!(
                            "The {} uses the {} CMOS layout, the {} the {} one.",
                            fw::name(to),
                            to.cmos_layout.map_or("unlisted", CmosLayout::label),
                            fw::name(from),
                            from.cmos_layout.map_or("unlisted", CmosLayout::label),
                        ),
                        (from, to) => format!(
                            "The CMOS layout of {} is not known, so the {} cannot read what {} wrote.",
                            if from.is_none() { "the current BIOS, which the catalogue does not list," } else { "the new BIOS, which the catalogue does not list," },
                            to.map_or_else(|| "new BIOS".to_owned(), fw::name),
                            from.map_or_else(|| "the current BIOS".to_owned(), |e| format!("the {}", fw::name(e))),
                        ),
                    },
                    if machine {
                        "This machine's CMOS is replaced by the bytes derived from its configuration, as on a new machine; set the SETUP choices again in the guest. The date and time come from Start every session."
                    } else {
                        "The revision stores the bytes derived from its configuration instead of the image's CMOS."
                    }
                )),
            }
        }
        let image = model.image_firmware(slot);
        if image.map(|e| e.id) != current.map(|e| e.id) {
            let was = image.map_or_else(|| "None".to_owned(), fw::name);
            if machine {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Image: {was} · ● differs from the image"))
                            .small(),
                    );
                    if let Some(image) = image {
                        if ui
                            .add_enabled(!model.read_only, egui::Link::new("Use the image's value"))
                            .clicked()
                        {
                            edit = Some(FirmwareEdit::Set(slot, image.id));
                        }
                    }
                });
            } else {
                hint(ui, format!("Was: {was}"));
            }
        }
        // A superseded entry's replacement already says why under its line.
        let explained = current.and_then(fw::replacement).map(|(by, _)| by.id);
        let unavailable: Vec<&FirmwareChoice> = choices
            .iter()
            .filter(|c| c.reason.is_some() && Some(c.entry.id) != explained)
            .collect();
        if !unavailable.is_empty() && model.packages_read() {
            hint(ui, "Fit this board but cannot be chosen now:");
            for choice in unavailable {
                if unavailable_line(ui, choice, model.read_only) {
                    edit = Some(FirmwareEdit::ImportRoms);
                }
            }
        }
    }
    ui.strong("Option ROMs");
    let options = fw::option_roms(model.machine());
    ui.horizontal(|ui| {
        ui.add_sized([170.0, 20.0], egui::Label::new("Option ROM"));
        ui.label(if options.is_empty() {
            "None".to_owned()
        } else {
            options.join(", ")
        });
    });
    if !options.is_empty() {
        hint(
            ui,
            "Follows the disk adapter that serves C: (Storage tab); not chosen here.",
        );
    }
    let users = model.new_user_supplied();
    if !users.is_empty() {
        let names = users
            .iter()
            .map(|e| fw::name(e))
            .collect::<Vec<_>>()
            .join(" and the ");
        notice(
            ui,
            if machine {
                format!("user-supplied: with the {names}, a later Save as new image… from this machine makes a user-supplied image, which can be pushed only to a namespace marked private.")
            } else {
                format!("user-supplied: with the {names}, this revision is user-supplied: it can be pushed only to a namespace marked private, never to unemu/.")
            },
        );
    }
    hint(ui, format!(
        "The choices are the firmware catalogue entries that fit the {} board, grouped by family; the entries in one group share a CMOS layout. A superseded entry is never preselected.",
        model.board_label()
    ));
    edit
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
    let machine = model.machine_name().map(str::to_owned);
    egui::Modal::new(egui::Id::unique("image-settings")).show(ctx, |ui| {
        ui.set_width(680.0);
        let heading = match &machine {
            Some(name) => format!("Settings — {name} (machine)"),
            None => format!("Settings — {reference} (image)"),
        };
        if title_bar(ui, &heading, classic) {
            close = true;
        }
        // The save-as-image mock-up's note banner: ⓘ beside its two lines;
        // the machines mock-up's banner for Settings… on a machine.
        ui.horizontal_top(|ui| {
            crate::shell_icons::show(ui, crate::shell_icons::Icon::Info);
            ui.vertical(|ui| match &machine {
                Some(_) => {
                    ui.strong("These settings belong to this machine.");
                    hint(ui, format!("Changes apply at its next start; the image {reference} never changes. Save as new image… writes them into a new image. To change the image itself, use Image › Settings…, which publishes a new image revision."));
                }
                None => {
                    ui.strong("Changes create a new image revision; disks are shared.");
                    hint(ui, format!("The image {digest} itself never changes. Its disks and media are reused as they are; a firmware change binds other ROM blobs, and the rest of the new revision is its configuration."));
                }
            });
        });
        if machine.is_none() && model.source_tag_moves() {
            hint(ui, format!("{reference} is not a version tag: publishing under it moves it to the revision after a confirmation, and the old image stays reachable by its digest."));
        }
        if model.read_only {
            ui.label(if machine.is_some() {
                "The machine is running, so its settings open read-only. Stop it to change them."
            } else {
                "This image is running. Stop it before changing its settings."
            });
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
        let mut firmware_edit = None;
        let response = egui::Frame::group(ui.style()).corner_radius(0).show(ui, |ui| {
            ui.set_min_height(180.0);
            egui::ScrollArea::vertical().id_salt("image-settings-fields").max_height(260.0).show(ui, |ui| {
                let mut heading = String::new();
                let mut fields: Vec<_> = caps::fields(&model.draft)
                    .into_iter()
                    .filter(|field| {
                        field.group == model.tab
                            && field.group != caps::Group::Firmware
                            && !field.path.contains(".drives[unit=")
                    })
                    .collect();
                fields.sort_by_key(|field| (field.heading.clone(), !field.path.ends_with(".enabled")));
                for field in fields {
                    if field.heading != heading {
                        ui.strong(&field.heading);
                        heading = field.heading.clone();
                    }
                    if let Some(value) = field_control(ui, model, &field) {
                        edit = Some((field.path.clone(), value));
                    }
                    if let Some((_, Some(old))) = changes.iter().find(|(f, _)| f.path == field.path) {
                        let old = caps::value_label(&model.draft, &field.path, old);
                        hint(ui, if machine.is_some() {
                            format!("Image: {old} · ● differs from the image")
                        } else {
                            format!("Was: {old}")
                        });
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
                    firmware_edit = firmware_tab(ui, model, machine.is_some());
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
        match firmware_edit {
            Some(FirmwareEdit::Set(slot, id)) => model.set_firmware(slot, id),
            Some(FirmwareEdit::ImportRoms) => action = ImageAction::ImportRoms,
            None => (),
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
        if machine.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(model.can_apply(), egui::Button::new("Apply")).clicked() {
                    action = ImageAction::Apply(false);
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.add_enabled(!model.read_only, egui::Button::new("OK")).clicked() {
                    action = ImageAction::Apply(true);
                }
                let count = model.changes().len();
                ui.label(match count {
                    0 => "Same as the image".to_owned(),
                    1 => "1 setting differs from the image".to_owned(),
                    n => format!("{n} settings differ from the image"),
                });
            });
            return;
        }
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
            ui.label(match &machine {
                Some(name) => format!("The changes to {name} are not saved. Discard them?"),
                None => format!("The changes to {reference} are not published. Discard them?"),
            });
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
    let official = model.official_template();
    // A template pulled from another registry keeps its host in the tag; the
    // texts name the template and the registry it came from.
    let repository = image_registry::reference::short_name(model.row.repository()).to_owned();
    let registry = image_store::Store::tag_host(&reference)
        .unwrap_or(image_registry::DEFAULT_REGISTRY)
        .to_owned();
    let users: Vec<String> = model
        .new_user_supplied()
        .iter()
        .map(|e| fw::name(e))
        .collect();
    let Some(review) = &mut model.review else {
        return action;
    };
    let mut publish = false;
    egui::Modal::new(egui::Id::unique("image-revision")).show(ctx, |ui| {
        ui.set_width(680.0);
        if title_bar(ui, "Publish new image revision", classic) {
            action = ReviewAction::Cancel;
        }
        ui.label(format!(
            "From {reference} {} · stopped{}",
            save_image::short(&digest),
            if official { format!(" · official template from unemu/{repository}") } else { String::new() }
        ));
        ui.strong("New revision");
        save_image::form(ui, &mut review.save);
        hint(ui, if review.save.local_suffix() {
            format!("Suggested: the source version with a local suffix, so that no later official release of {repository} can clash with it. Or name your own repository, which starts at 1.0.0.")
        } else {
            "Suggested: the next minor version. A revision never overwrites a tag; version tags never move.".to_owned()
        });
        if official {
            notice(ui, format!("{repository} comes from unemu/ on {registry}, which is read-only for you. The revision is written to your local store only; Push… can send it to a namespace of your own."));
        }
        if !users.is_empty() {
            notice(ui, format!(
                "user-supplied: with the {}, this revision is user-supplied: it can be pushed only to a namespace marked private, never to unemu/.",
                users.join(" and the ")
            ));
        }
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
                    if plan.cmos_reset {
                        notice(ui, "Another CMOS layout: the stored CMOS image is replaced by the bytes derived from the revision's configuration, as a new machine starts.");
                    }
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
    use crate::image_settings::tests::{open_image, open_new_image, opened};
    use crate::library_test_support::{painted_at, FirmwareStore, Pointer};

    /// A disabled entry of the open list says why on hover, and its line
    /// under the slot says the same without hovering; an entry that can be
    /// chosen says nothing.
    #[test]
    fn an_unavailable_firmware_entry_says_why_on_hover_and_in_its_line() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let mut model = opened(&store, open_image(&store), false);
        model.set_packages(store.choices().0);
        let choices = model.firmware_choices(Slot::System);
        let choice = |id: &str| choices.iter().find(|c| c.entry.id == id).unwrap();
        let current = choice("unemu-at-bios-1.0.2-unemu.3");
        let blocked = choice("ibm-5170-bios-1985-06-10");
        let reason = blocked.reason.as_deref().unwrap();
        assert!(reason.contains("never downloaded"), "{reason}");
        let available = choice("kiselev-xi8088-1.0.2");
        assert_eq!(available.reason, None, "a package of the store");
        let tooltip = crate::new_machine_view::capitalized(reason);
        let pointer = Pointer::new(egui::vec2(1400.0, 1400.0));
        let mut draw = |ui: &mut egui::Ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = firmware_tab(ui, &mut model, false);
            });
        };
        pointer.frame(Vec::new(), &mut draw);
        let shown = pointer.frame(Vec::new(), &mut draw);
        painted_at(&shown, &format!("{} — {reason}", fw::name(blocked.entry)));
        let available_line = format!("{} — ", fw::name(available.entry));
        assert!(
            !shown.iter().any(|(t, _)| t.starts_with(&available_line)),
            "{shown:?}"
        );
        assert!(!shown.iter().any(|(t, _)| *t == tooltip));

        pointer.click(
            painted_at(&shown, &choice_label(current)).center(),
            &mut draw,
        );
        pointer.frame(Vec::new(), &mut draw);
        let popup = pointer.frame(Vec::new(), &mut draw);
        let calm: Vec<&String> = popup.iter().map(|(t, _)| t).collect();
        let hovered = pointer.hover(
            painted_at(&popup, &choice_label(available)).center(),
            &mut draw,
        );
        let extra: Vec<&String> = hovered
            .iter()
            .map(|(t, _)| t)
            .filter(|t| !calm.contains(t))
            .collect();
        assert!(extra.is_empty(), "an available entry shows {extra:?}");
        let hovered = pointer.hover(
            painted_at(&popup, &choice_label(blocked)).center(),
            &mut draw,
        );
        painted_at(&hovered, &tooltip);
    }

    /// The superseded open VGA BIOS, the release replacing it and why.
    fn superseded_video() -> (
        &'static assembly::FirmwareEntry,
        &'static assembly::FirmwareEntry,
        &'static str,
    ) {
        let old = fw::entry("vgabios-0.8a-unemu.1-std").unwrap();
        let (by, why) = fw::replacement(old).unwrap();
        (old, by, why)
    }

    /// Settings… on a machine whose video BIOS is the superseded release.
    /// With `image_holds`, its image binds the replacement, as the fixture
    /// image does; otherwise the image binds no video BIOS, so neither the
    /// image nor the machine's applied tables hold the replacement. Packages
    /// are not read yet.
    fn machine_on_superseded_video(store: &FirmwareStore, image_holds: bool) -> ImageSettings {
        let (old, by, _) = superseded_video();
        let row = if image_holds {
            open_image(store)
        } else {
            let mut image = open_new_image(store, 1024);
            image
                .firmware
                .retain(|rom| rom.rom().is_ok_and(|r| r.sha256 != by.sha256));
            let reference = "no-video-bios:1.0.0";
            let descriptor =
                pc_image::create::create(&store.store, &image, reference, None).unwrap();
            crate::library_view::ImageRow {
                reference: reference.into(),
                aliases: Vec::new(),
                descriptor,
                summary: None,
                failure: None,
            }
        };
        let model = opened(store, row, false);
        let rom = fw::package_rom(&store.choices().0, by, Slot::Video).unwrap();
        let mut table = fw::package_table(&rom).unwrap();
        table.insert("sha256".into(), old.sha256.into());
        let mut tables = model.machine().clone();
        fw::replace(&mut tables, Slot::Video, table).unwrap();
        let model = model.for_machine("superseded".into(), "Superseded".into(), tables);
        assert_eq!(model.firmware(Slot::Video).map(|e| e.id), Some(old.id));
        assert_eq!(
            model.image_firmware(Slot::Video).map(|e| e.id),
            image_holds.then_some(by.id)
        );
        model
    }

    /// The store's packages without the one holding `entry`.
    fn packages_without(
        store: &FirmwareStore,
        entry: &assembly::FirmwareEntry,
    ) -> Vec<pc_image::create::Package> {
        store
            .choices()
            .0
            .into_iter()
            .filter(|p| !p.config.roms.iter().any(|r| r.sha256 == entry.sha256))
            .collect()
    }

    /// A machine on a superseded video BIOS whose replacement is not in the
    /// store, and which neither its image nor its applied settings hold:
    /// right under "Superseded by …", the replacement says why Use is
    /// greyed, on hover and in a line with Import ROM folder…, which asks for
    /// the sheet's own import.
    #[test]
    fn a_superseded_entry_says_why_its_replacement_cannot_be_chosen() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let (_, by, why) = superseded_video();
        let mut model = machine_on_superseded_video(&store, false);
        model.set_packages(packages_without(&store, by));
        let reason = model
            .firmware_choices(Slot::Video)
            .into_iter()
            .find(|c| c.entry.id == by.id)
            .and_then(|c| c.reason)
            .unwrap();
        assert!(reason.contains("not in your store"), "{reason}");
        let pointer = Pointer::new(egui::vec2(1400.0, 1400.0));
        let mut edit = None;
        let mut draw = |ui: &mut egui::Ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if let Some(e) = firmware_tab(ui, &mut model, true) {
                    edit = Some(e);
                }
            });
        };
        pointer.frame(Vec::new(), &mut draw);
        let shown = pointer.frame(Vec::new(), &mut draw);
        let superseded = painted_at(
            &shown,
            &format!("Superseded by {} ({why}).", fw::release(by)),
        );
        let line_text = format!("{} — {reason}", fw::name(by));
        let line = painted_at(&shown, &line_text);
        assert_eq!(
            shown.iter().filter(|(t, _)| *t == line_text).count(),
            1,
            "said once"
        );
        let next = shown
            .iter()
            .filter(|(t, r)| {
                r.top() > superseded.top()
                    && (t.starts_with("Fit this board") || t.starts_with("Image: "))
            })
            .map(|(_, r)| r.top())
            .fold(f32::INFINITY, f32::min);
        assert!(
            superseded.top() < line.top() && line.top() < next,
            "the reason sits right under the superseded line: {shown:?}"
        );
        let import = shown
            .iter()
            .find(|(t, r)| {
                t == "Import ROM folder…"
                    && r.center().y > line.top()
                    && r.center().y < line.bottom()
            })
            .map(|(_, r)| r.center())
            .expect("Import ROM folder… on the reason's line");
        let tooltip = crate::new_machine_view::capitalized(&reason);
        assert!(!shown.iter().any(|(t, _)| *t == tooltip));
        let hovered = pointer.hover(
            painted_at(&shown, &format!("Use {}", fw::release(by))).center(),
            &mut draw,
        );
        painted_at(&hovered, &tooltip);
        pointer.click(import, &mut draw);
        assert!(matches!(edit, Some(FirmwareEdit::ImportRoms)));
    }

    /// A replacement the machine's image already binds can be chosen with no
    /// package in the store, as Use the image's value always could: it has
    /// no reason and no line, and Use binds it.
    #[test]
    fn a_replacement_the_image_holds_can_be_chosen_without_a_package() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let (_, by, _) = superseded_video();
        let mut model = machine_on_superseded_video(&store, true);
        model.set_packages(Vec::new());
        let choice = model
            .firmware_choices(Slot::Video)
            .into_iter()
            .find(|c| c.entry.id == by.id)
            .unwrap();
        assert!(choice.rom.is_none(), "no package holds it");
        assert_eq!(choice.reason, None);
        let pointer = Pointer::new(egui::vec2(1400.0, 1400.0));
        let mut edit = None;
        let mut draw = |ui: &mut egui::Ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if let Some(e) = firmware_tab(ui, &mut model, true) {
                    edit = Some(e);
                }
            });
        };
        pointer.frame(Vec::new(), &mut draw);
        let shown = pointer.frame(Vec::new(), &mut draw);
        let line = format!("{} — ", fw::name(by));
        assert!(
            !shown.iter().any(|(t, _)| t.starts_with(&line)),
            "{shown:?}"
        );
        let calm: Vec<&String> = shown.iter().map(|(t, _)| t).collect();
        let at = painted_at(&shown, &format!("Use {}", fw::release(by))).center();
        let hovered = pointer.hover(at, &mut draw);
        let extra: Vec<&String> = hovered
            .iter()
            .map(|(t, _)| t)
            .filter(|t| !calm.contains(t))
            .collect();
        assert!(extra.is_empty(), "Use shows {extra:?}");
        pointer.click(at, &mut draw);
        assert!(matches!(edit, Some(FirmwareEdit::Set(Slot::Video, id)) if id == by.id));
        model.set_firmware(Slot::Video, by.id);
        assert_eq!(model.error, None);
        assert_eq!(model.firmware(Slot::Video).map(|e| e.id), Some(by.id));
    }

    /// Before the store's packages are read, no replacement line shows, and
    /// the greyed Use says why on hover: the packages are being read.
    #[test]
    fn before_the_packages_are_read_use_says_they_are_being_read() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let (_, by, _) = superseded_video();
        let mut model = machine_on_superseded_video(&store, false);
        assert!(!model.packages_read());
        let reason = model
            .firmware_choices(Slot::Video)
            .into_iter()
            .find(|c| c.entry.id == by.id)
            .and_then(|c| c.reason)
            .unwrap();
        assert!(reason.contains("being read"), "{reason}");
        let pointer = Pointer::new(egui::vec2(1400.0, 1400.0));
        let mut draw = |ui: &mut egui::Ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = firmware_tab(ui, &mut model, true);
            });
        };
        pointer.frame(Vec::new(), &mut draw);
        let shown = pointer.frame(Vec::new(), &mut draw);
        let line = format!("{} — ", fw::name(by));
        assert!(
            !shown.iter().any(|(t, _)| t.starts_with(&line)),
            "{shown:?}"
        );
        let tooltip = crate::new_machine_view::capitalized(&reason);
        assert_eq!(
            tooltip,
            "The firmware packages of your store are being read."
        );
        let hovered = pointer.hover(
            painted_at(&shown, &format!("Use {}", fw::release(by))).center(),
            &mut draw,
        );
        painted_at(&hovered, &tooltip);
    }

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
