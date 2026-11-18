// SPDX-License-Identifier: AGPL-3.0-or-later
//! Classic modal rendering of the New machine… wizard; machine-images §7 and
//! `backlog/docs/design/mockups/new-machine.html`. Every option and reason is
//! taken from the headless model; this module only lays them out.

use crate::new_machine::{self as model, Offer, Slot, TagLevel, Wizard, STEPS};
use assembly::capabilities::{self as caps, Value};
use pc_image::create::{self, Chs, Serial, Video};

pub enum Action {
    None,
    Cancel,
    Create,
    ImportRoms,
}

pub fn show(ctx: &egui::Context, wizard: &mut Wizard, classic: bool) -> Action {
    let mut action = Action::None;
    egui::Modal::new(egui::Id::unique("new-machine-wizard")).show(ctx, |ui| {
        ui.set_width(860.0);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 30.0).max(100.0);
            ui.allocate_ui(egui::vec2(width, 23.0), |ui| {
                let title = format!(
                    "New machine — step {} of {}: {}",
                    wizard.step + 1,
                    STEPS.len(),
                    STEPS[wizard.step]
                );
                if classic {
                    ui_kit::skin::gradient_title(ui, &title);
                } else {
                    ui.heading(title);
                }
            });
            if ui
                .add_sized([23.0, 23.0], egui::Button::new("×"))
                .on_hover_text("Close the wizard")
                .clicked()
            {
                action = Action::Cancel;
            }
        });
        let problems = wizard.problems();
        // A modal lays its content out inside last frame's size, and the
        // vertical separator below fills the height on offer. Offered the
        // whole dialog, it would make the wizard taller on every frame, so
        // the body gets a fixed height that fits the window and the page
        // scrolls inside it.
        let body = (ctx.content_rect().height() - 190.0).clamp(240.0, 460.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), body),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                ui.vertical(|ui| {
                    ui.set_width(190.0);
                    ui.strong("New machine");
                    for (index, name) in STEPS.iter().enumerate() {
                        let label = format!(
                            "{}. {name}{}",
                            index + 1,
                            step_mark(index, wizard.step, wizard.visited, &problems)
                        );
                        let response = ui.add_enabled(
                            index <= wizard.visited,
                            egui::Button::selectable(index == wizard.step, label),
                        );
                        if response.clicked() {
                            wizard.go(index);
                        }
                    }
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "Creates an empty image: configuration, firmware, blank disks and empty media catalogues. No operating system yet.",
                            )
                            .small(),
                        )
                        .wrap(),
                    );
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("new-machine-page")
                        .max_height(body)
                        .show(ui, |ui| {
                            ui.set_width(620.0);
                            match wizard.step {
                                0 => page_class(ui, wizard),
                                model::MOTHERBOARD => {
                                    if page_board(ui, wizard) {
                                        action = Action::ImportRoms;
                                    }
                                }
                                model::CPU_MEMORY => page_cpu_memory(ui, wizard),
                                model::DEVICES => {
                                    if page_devices(ui, wizard) {
                                        action = Action::ImportRoms;
                                    }
                                }
                                model::DRIVES => page_drives(ui, wizard),
                                model::NAME => page_name(ui, wizard),
                                model::SUMMARY => page_summary(ui, wizard),
                                _ => (),
                            }
                        });
                });
            },
        );
        ui.separator();
        let problems = wizard.problems();
        let here = problems.iter().find(|(step, _)| *step == wizard.step);
        let last = wizard.step == STEPS.len() - 1;
        let message = wizard
            .error
            .clone()
            .or_else(|| here.map(|(_, text)| text.clone()))
            .or_else(|| {
                (last && !problems.is_empty()).then(|| {
                    let (step, text) = &problems[0];
                    format!("Fix step {} ({}) first: {text}", step + 1, STEPS[*step])
                })
            })
            .or_else(|| wizard.busy.then(|| "Working in the image store…".into()));
        if let Some(message) = message {
            ui.add(egui::Label::new(message).wrap());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                action = Action::Cancel;
            }
            if ui
                .add_enabled(
                    last && problems.is_empty() && !wizard.busy,
                    egui::Button::new("Create"),
                )
                .clicked()
            {
                if wizard.tag_check().moving.is_some() && wizard.confirmed_move.is_none() {
                    wizard.move_prompt = true;
                } else {
                    action = Action::Create;
                }
            }
            if ui
                .add_enabled(!last && here.is_none(), egui::Button::new("Next >"))
                .clicked()
            {
                wizard.go(wizard.step + 1);
            }
            if ui
                .add_enabled(wizard.step > 0, egui::Button::new("< Back"))
                .clicked()
            {
                wizard.go(wizard.step - 1);
            }
        });
    });
    if wizard.move_prompt {
        match move_dialog(ctx, wizard, classic) {
            Some(true) => {
                wizard.confirmed_move = wizard.tag_check().moving;
                wizard.move_prompt = false;
                action = Action::Create;
            }
            Some(false) => wizard.move_prompt = false,
            None => (),
        }
    } else if ctx.input_mut(ui_kit::skin::escape_pressed) {
        action = Action::Cancel;
    }
    action
}

fn move_dialog(ctx: &egui::Context, wizard: &Wizard, classic: bool) -> Option<bool> {
    let check = wizard.tag_check();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("new-machine-move-dialog")).show(ctx, |ui| {
        ui.set_width(480.0);
        if classic {
            ui_kit::skin::gradient_title(ui, "Move tag?");
        } else {
            ui.heading("Move tag?");
        }
        ui.label(format!(
            "{} already names another image. Move it to the new one?",
            wizard.reference()
        ));
        if let Some(digest) = &check.moving {
            ui.monospace(format!("Now: {}", model::short(digest)));
        }
        ui.label("After the move: the image created now.");
        ui.add(
            egui::Label::new(
                "The old image stays in the store, reachable by its digest. Workspaces that pinned it keep running it. Version tags never move this way.",
            )
            .wrap(),
        );
        ui.horizontal(|ui| {
            if ui.button("Move tag and create").clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    if ctx.input_mut(ui_kit::skin::escape_pressed) {
        answer = Some(false);
    }
    answer
}

/// A step's mark in the wizard's step list, as the new-machine mock-up shows
/// it: "!" on a visited step that needs attention, ✓ on one that is done,
/// nothing on the current step or one not reached yet.
fn step_mark(
    index: usize,
    current: usize,
    visited: usize,
    problems: &[(usize, String)],
) -> &'static str {
    if index == current || index > visited {
        ""
    } else if problems.iter().any(|(step, _)| *step == index) {
        "  !"
    } else {
        "  ✓"
    }
}

fn small(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}
fn why(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.into())
                .small()
                .color(ui.visuals().warn_fg_color),
        )
        .wrap(),
    );
}

/// A disabled control says why on hover: egui gives a disabled widget no
/// hover text of its own, so "unavailable" alone would leave the user guessing.
pub(crate) fn why_disabled(response: egui::Response, reason: Option<&str>) -> egui::Response {
    match reason {
        Some(reason) => response.on_disabled_hover_text(capitalized(reason)),
        None => response,
    }
}

/// A field whose options and reasons are the capabilities API's choices.
fn api_combo(
    ui: &mut egui::Ui,
    wizard: &Wizard,
    label: &str,
    path: &str,
    current: Value,
) -> Option<Value> {
    let choices = wizard.choices(path);
    let draft = wizard.draft().ok();
    let text = |value: &Value| {
        draft.as_deref().map_or_else(
            || format!("{value:?}"),
            |d| caps::value_label(d, path, value),
        )
    };
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 20.0], egui::Label::new(label));
        egui::ComboBox::from_id_salt(path)
            .width(300.0)
            .selected_text(text(&current))
            .show_ui(ui, |ui| {
                for choice in choices.iter() {
                    let label = format!(
                        "{}{}",
                        text(&choice.value),
                        if choice.enabled {
                            ""
                        } else {
                            " — unavailable"
                        }
                    );
                    let entry = ui.add_enabled(
                        choice.enabled,
                        egui::Button::selectable(choice.value == current, label),
                    );
                    if why_disabled(entry, choice.reason.as_deref()).clicked() {
                        edit = Some(choice.value.clone());
                    }
                }
            });
    });
    for choice in choices.iter().filter(|c| !c.enabled) {
        if let Some(reason) = &choice.reason {
            why(ui, format!("{}: {reason}", text(&choice.value)));
        }
    }
    for d in wizard
        .errors()
        .iter()
        .filter(|d| caps::diagnostic_affects_field(d, path))
    {
        why(ui, d.reason.clone());
    }
    edit.filter(|value| *value != current)
}

/// A structural option list: each entry is checked as a candidate document.
fn candidate_combo<T: Clone + PartialEq>(
    ui: &mut egui::Ui,
    wizard: &Wizard,
    label: &str,
    id: &str,
    options: &[(T, String)],
    current: &T,
    apply: impl Fn(&mut model::Selection, &T),
) -> Option<T> {
    let reasons: Vec<Option<String>> = options
        .iter()
        .enumerate()
        .map(|(index, (value, _))| {
            if value == current {
                None
            } else {
                wizard.candidate(&format!("{id}:{index}"), |s| apply(s, value))
            }
        })
        .collect();
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 20.0], egui::Label::new(label));
        let selected = options
            .iter()
            .find(|(v, _)| v == current)
            .map_or_else(String::new, |(_, l)| l.clone());
        egui::ComboBox::from_id_salt(id)
            .width(300.0)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for ((value, text), reason) in options.iter().zip(&reasons) {
                    let text = format!(
                        "{text}{}",
                        if reason.is_some() {
                            " — unavailable"
                        } else {
                            ""
                        }
                    );
                    let entry = ui.add_enabled(
                        reason.is_none(),
                        egui::Button::selectable(value == current, text),
                    );
                    if why_disabled(entry, reason.as_deref()).clicked() {
                        edit = Some(value.clone());
                    }
                }
            });
    });
    for ((_, text), reason) in options.iter().zip(&reasons) {
        if let Some(reason) = reason {
            why(ui, format!("{text}: {reason}"));
        }
    }
    edit.filter(|value| value != current)
}

fn offer_row(ui: &mut egui::Ui, offer: &Offer, checked: bool) -> bool {
    let mut clicked = false;
    ui.add_enabled_ui(offer.reasons.is_empty(), |ui| {
        ui.horizontal(|ui| {
            clicked = ui
                .radio(checked, egui::RichText::new(&offer.name).strong())
                .clicked();
            ui.label(if offer.open { "open" } else { "user-supplied" });
        });
        ui.indent(&offer.rom.0, |ui| {
            small(ui, format!("{} · {}", offer.vendor, offer.detail));
            small(ui, offer.layout.clone());
            for reason in &offer.reasons {
                why(ui, reason.clone());
            }
        });
    });
    clicked && offer.reasons.is_empty()
}

/// A firmware picker with its incompatible list; returns the chosen ROM.
fn picker(
    ui: &mut egui::Ui,
    wizard: &mut Wizard,
    slot: Slot,
    group: bool,
) -> Option<model::RomRef> {
    let (fits, misfits) = wizard.offers(slot);
    let current = match slot {
        Slot::Bios => wizard.selection().bios.clone(),
        Slot::Video => wizard.selection().video_bios.clone(),
        Slot::OptionRom => wizard.selection().option_rom.clone(),
    };
    let mut chosen = None;
    let mut vendor = String::new();
    for offer in &fits {
        if group && offer.vendor != vendor {
            vendor = offer.vendor.clone();
            let count = fits.iter().filter(|o| o.vendor == vendor).count();
            ui.strong(format!("{vendor} ({count})"));
        }
        if offer_row(ui, offer, current.as_ref() == Some(&offer.rom)) {
            chosen = Some(offer.rom.clone());
        }
    }
    if !misfits.is_empty() {
        let open = &mut wizard.show_incompatible[slot.index()];
        if ui
            .button(format!(
                "{} incompatible ({})",
                if *open { "Hide" } else { "Show" },
                misfits.len()
            ))
            .clicked()
        {
            *open = !*open;
        }
        if wizard.show_incompatible[slot.index()] {
            for offer in &misfits {
                offer_row(ui, offer, false);
            }
        }
    }
    chosen
}

fn fetch_buttons(ui: &mut egui::Ui, enabled: bool) -> bool {
    let mut import = false;
    ui.horizontal(|ui| {
        import = ui
            .add_enabled(enabled, egui::Button::new("Import ROM folder…"))
            .on_disabled_hover_text("Stop the running machine and wait for the image store first.")
            .clicked();
        ui.add_enabled(false, egui::Button::new("Get from registry…"))
            .on_disabled_hover_text(
                "Browsing registries for firmware packages is not available yet.",
            );
    });
    import
}

fn page_class(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.heading("Choose a machine class");
    small(ui, "The list of machine classes is closed. Classes that are not built yet stay visible, greyed, with the phase that brings them.");
    for (index, class) in create::MACHINE_CLASSES.iter().enumerate() {
        let available = !class.boards.is_empty();
        ui.add_enabled_ui(available, |ui| {
            if ui
                .radio(
                    wizard.selection().class == class.id,
                    egui::RichText::new(format!("{}. {} · {}", index + 1, class.name, class.cpu))
                        .strong(),
                )
                .clicked()
            {
                wizard.edit(|s| s.class = class.id);
            }
            ui.indent(class.id, |ui| {
                small(ui, class.examples);
                if available {
                    small(
                        ui,
                        format!(
                            "{}. The motherboard and its BIOS are chosen in the next step.",
                            capitalized(class.phase)
                        ),
                    );
                } else {
                    why(ui, format!("Not available yet · {}.", class.phase));
                }
            });
        });
    }
    small(
        ui,
        "Anything outside this list needs a new project decision.",
    );
}
pub(crate) fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Returns whether Import ROM folder… was pressed.
fn page_board(ui: &mut egui::Ui, wizard: &mut Wizard) -> bool {
    let mut import = false;
    ui.heading("Motherboard");
    small(ui, "Boards of the AT 286 class. The system BIOS belongs to the board: only firmware packages whose fit metadata names the chosen board are offered.");
    let boards = create::MACHINE_CLASSES
        .iter()
        .find(|c| c.id == wizard.selection().class)
        .map_or(&[][..], |c| c.boards);
    for board in boards {
        let selected = wizard.hardware().board == *board;
        if ui
            .radio(
                selected,
                egui::RichText::new(format!("{} · {}", wizard.board_label(), board.id())).strong(),
            )
            .clicked()
            && !selected
        {
            wizard.choose_board(*board);
        }
        let count = wizard.fitting(Slot::Bios);
        ui.indent(board.id(), |ui| {
            if count == 0 {
                why(ui, "No BIOS package in your store fits this board yet.");
            } else {
                small(
                    ui,
                    format!(
                        "{count} BIOS package{} fit{}",
                        if count == 1 { "" } else { "s" },
                        if count == 1 { "s" } else { "" }
                    ),
                );
            }
        });
    }
    ui.separator();
    ui.strong(format!("System BIOS for the {}", wizard.board_label()));
    if wizard.packages.is_none() {
        ui.label("Reading firmware packages from the store…");
        return false;
    }
    if wizard.fitting(Slot::Bios) == 0 {
        ui.strong(format!("No system BIOS fits the {}.", wizard.board_label()));
        small(ui, format!("None of the firmware packages in your store names {} in its fit metadata. Import your own ROM dumps once, or pull a package from a registry.", wizard.hardware().board.id()));
        import |= fetch_buttons(ui, wizard.can_import_roms);
    } else {
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.filter)
                    .hint_text("name, vendor, version, date…"),
            );
        });
    }
    if let Some(rom) = picker(ui, wizard, Slot::Bios, true) {
        wizard.choose_bios(rom);
    }
    if wizard.fitting(Slot::Bios) > 0 && wizard.offers(Slot::Bios).0.is_empty() {
        small(
            ui,
            format!("No fitting package matches \"{}\".", wizard.filter.trim()),
        );
    }
    let bios_open = wizard.bios_rom().map(|b| wizard.open(&b));
    if bios_open == Some(false) {
        why(ui, "User-supplied firmware: this image stays in your private store and can be pushed only to a private registry, never to unemu/.");
    }
    if wizard.fitting(Slot::Bios) > 0 {
        ui.horizontal(|ui| {
            ui.label("Another BIOS for this board?");
        });
        import |= fetch_buttons(ui, wizard.can_import_roms);
    }
    small(ui, "Changing the board picks a fitting BIOS again. Changing the BIOS resets what it decides: the video BIOS default, floppy types and the disk option ROM.");
    import
}

fn page_cpu_memory(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.heading("CPU and memory");
    small(
        ui,
        format!(
            "Board: {} · Processor: Intel 80286 (fixed by the machine class).",
            wizard.board_label()
        ),
    );
    let hardware = wizard.hardware().clone();
    let speeds = create::speeds(hardware.board);
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 20.0], egui::Label::new("CPU speed"));
        let label = |s: &create::Speed| {
            format!(
                "{} MHz — {}/{} steps per PIT clock ({})",
                s.mhz,
                s.steps,
                s.pit_clocks,
                if s.calibrated {
                    "calibrated default"
                } else {
                    "scaled"
                }
            )
        };
        let current = speeds
            .iter()
            .find(|s| (s.steps, s.pit_clocks) == hardware.rate)
            .map_or_else(|| format!("{}/{}", hardware.rate.0, hardware.rate.1), label);
        egui::ComboBox::from_id_salt("speed")
            .width(300.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                for speed in &speeds {
                    if ui
                        .selectable_label(
                            (speed.steps, speed.pit_clocks) == hardware.rate,
                            label(speed),
                        )
                        .clicked()
                    {
                        wizard.edit(|s| s.hardware.rate = (speed.steps, speed.pit_clocks));
                    }
                }
            });
    });
    small(
        ui,
        "Speed is an instruction rate per PIT clock, not a cycle-exact clock.",
    );
    for path in ["cpu.rate_steps", "cpu.rate_pit_clocks"] {
        for d in wizard
            .errors()
            .iter()
            .filter(|d| caps::diagnostic_affects_field(d, path))
        {
            why(ui, d.reason.clone());
        }
    }
    let fpu = Value::Name(if hardware.fpu { "80287" } else { "none" }.into());
    if let Some(Value::Name(name)) = api_combo(ui, wizard, "Coprocessor", "cpu.fpu", fpu) {
        wizard.edit(|s| s.hardware.fpu = name == "80287");
    }
    if let Some(Value::Integer(kib)) = api_combo(
        ui,
        wizard,
        "Conventional",
        "memory.conventional_kib",
        Value::Integer(hardware.conventional_kib),
    ) {
        wizard.edit(|s| s.hardware.conventional_kib = kib);
    }
    if let Some(domain) = wizard.domain("memory.extended_kib") {
        let mut kib = hardware.extended_kib;
        ui.horizontal(|ui| {
            ui.add_sized([150.0, 20.0], egui::Label::new("Extended"));
            ui.add(
                egui::DragValue::new(&mut kib)
                    .range(domain.min..=domain.max)
                    .speed(domain.step),
            );
            ui.label(format!(
                "KiB · {}–{} in steps of {}",
                domain.min, domain.max, domain.step
            ));
        });
        let kib = domain.min + (kib - domain.min) / domain.step * domain.step;
        if kib != hardware.extended_kib {
            wizard.edit(|s| s.hardware.extended_kib = kib);
        }
        small(ui, "1024 KiB keeps the POST memory test short.");
        for d in wizard
            .errors()
            .iter()
            .filter(|d| caps::diagnostic_affects_field(d, "memory.extended_kib"))
        {
            why(ui, d.reason.clone());
        }
    }
    let ems_default = hardware.ems.unwrap_or(wizard.ems_default);
    let options = [
        (true, "Lo-tech 2 MB EMS board".to_owned()),
        (false, "None".to_owned()),
    ];
    if let Some(on) = candidate_combo(
        ui,
        wizard,
        "Expanded memory",
        "ems",
        &options,
        &hardware.ems.is_some(),
        |s, on| s.hardware.ems = on.then_some(ems_default),
    ) {
        wizard.edit(|s| s.hardware.ems = on.then_some(ems_default));
        wizard.sanitize();
    }
    if let Some(ems) = wizard.hardware().ems {
        small(
            ui,
            "Expanded memory through a 64 KiB page frame; no IRQ or DMA.",
        );
        if let Some(Value::Integer(port)) = api_combo(
            ui,
            wizard,
            "EMS I/O port",
            "card[lotech-ems].port",
            Value::Integer(ems.port.into()),
        ) {
            wizard.edit(|s| {
                s.hardware.ems = Some(create::Ems {
                    port: port as u16,
                    ..ems
                })
            });
        }
        if let Some(Value::Integer(size)) = api_combo(
            ui,
            wizard,
            "EMS memory",
            "card[lotech-ems].size_kib",
            Value::Integer(ems.size_kib),
        ) {
            wizard.edit(|s| {
                s.hardware.ems = Some(create::Ems {
                    size_kib: size,
                    ..ems
                })
            });
        }
        if let Some(Value::Integer(frame)) = api_combo(
            ui,
            wizard,
            "Page frame",
            "card[lotech-ems].frame",
            Value::Integer(ems.frame.into()),
        ) {
            wizard.edit(|s| {
                s.hardware.ems = Some(create::Ems {
                    frame: frame as u16,
                    ..ems
                })
            });
        }
        small(
            ui,
            format!(
                "DOS needs the board's driver: DEVICE=LTEMM.EXE /p:{:X}",
                wizard.hardware().ems.map_or(ems.frame, |e| e.frame)
            ),
        );
    }
}

/// Returns whether Import ROM folder… was pressed.
fn page_devices(ui: &mut egui::Ui, wizard: &mut Wizard) -> bool {
    let mut import = false;
    ui.heading("Video, sound and ports");
    small(ui, "Expansion cards on the ISA bus. Ports, IRQs and DMA channels already taken by another card are greyed with the reason.");
    let hardware = wizard.hardware().clone();
    let draft = wizard.draft().ok();
    let card_label = |video: Video| {
        draft.as_deref().map_or_else(
            || video.card().to_owned(),
            |d| caps::value_label(d, "card[vga].kind", &Value::Name(video.card().into())),
        )
    };
    let options: Vec<_> = Video::ALL.iter().map(|v| (*v, card_label(*v))).collect();
    if let Some(video) = candidate_combo(
        ui,
        wizard,
        "Video card",
        "video",
        &options,
        &hardware.video,
        |s, v| s.hardware.video = *v,
    ) {
        wizard.edit(|s| s.hardware.video = video);
        wizard.sanitize();
    }
    if wizard.hardware().video == Video::Vga {
        ui.strong("Video BIOS for the VGA card");
        small(
            ui,
            "Video BIOS packages whose catalogue entry fits the register-level VGA card.",
        );
        if wizard.fitting(Slot::Video) == 0 {
            ui.strong("No video BIOS fits the VGA card.");
            import |= fetch_buttons(ui, wizard.can_import_roms);
        }
        if let Some(rom) = picker(ui, wizard, Slot::Video, false) {
            wizard.choose_video_bios(rom);
        }
        if let Some(video) = wizard.video_rom() {
            if !wizard.open(&video) {
                why(ui, "User-supplied video BIOS: this image stays in your private store and can be pushed only to a private registry, never to unemu/.");
            }
        }
    } else {
        small(ui, "This card needs no video BIOS.");
    }
    ui.separator();
    let sb16 = hardware.sound.unwrap_or(wizard.sb16_default);
    let options = [
        (true, "Sound Blaster 16 (includes OPL3 FM)".to_owned()),
        (false, "None".to_owned()),
    ];
    if let Some(on) = candidate_combo(
        ui,
        wizard,
        "Sound card",
        "sound",
        &options,
        &hardware.sound.is_some(),
        |s, on| s.hardware.sound = on.then_some(sb16),
    ) {
        wizard.edit(|s| s.hardware.sound = on.then_some(sb16));
    }
    if let Some(sb) = wizard.hardware().sound {
        for (label, field, value) in [
            ("Base I/O port", "base", u32::from(sb.base)),
            ("IRQ", "irq", u32::from(sb.irq)),
            ("DMA, 8-bit", "dma8", u32::from(sb.dma8)),
            ("DMA, 16-bit", "dma16", u32::from(sb.dma16)),
        ] {
            let path = format!("card[sb16].{field}");
            if let Some(Value::Integer(n)) =
                api_combo(ui, wizard, label, &path, Value::Integer(value))
            {
                wizard.edit(|s| {
                    if let Some(sb) = &mut s.hardware.sound {
                        match field {
                            "base" => sb.base = n as u16,
                            "irq" => sb.irq = n as u8,
                            "dma8" => sb.dma8 = n as u8,
                            _ => sb.dma16 = n as u8,
                        }
                    }
                });
            }
        }
        small(
            ui,
            format!(
                "For DOS: SET BLASTER=A{:X} I{} D{} H{} T6 · OPL3 at 388h–38Bh",
                sb.base, sb.irq, sb.dma8, sb.dma16
            ),
        );
    }
    ui.separator();
    let options = [
        (
            Serial::Mouse,
            "16550 UART · Microsoft serial mouse".to_owned(),
        ),
        (Serial::Empty, "16550 UART · nothing attached".to_owned()),
        (Serial::None, "Not installed".to_owned()),
    ];
    let com1 = format!("COM1 ({:X}h)", assembly::descriptors::UART_PORT.default);
    if let Some(serial) = candidate_combo(
        ui,
        wizard,
        &com1,
        "serial",
        &options,
        &hardware.serial,
        |s, v| s.hardware.serial = *v,
    ) {
        wizard.edit(|s| s.hardware.serial = serial);
    }
    if wizard.hardware().serial == Serial::Mouse {
        small(ui, "Mouse driver in DOS, for example CTMOUSE.");
    }
    import
}

fn page_drives(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.heading("Drives");
    small(ui, "Floppy drives, the disk controller with its option ROM, and new blank fixed disks. Partition and format the disks later inside the guest (FDISK, FORMAT).");
    let hardware = wizard.hardware().clone();
    let a = Value::Name(hardware.floppies[0].clone().unwrap_or_default());
    if let Some(Value::Name(kind)) = api_combo(
        ui,
        wizard,
        "Drive A:",
        "card[fdc-765].drives[unit=0].type",
        a,
    ) {
        wizard.edit(|s| s.hardware.floppies[0] = Some(kind));
    }
    let draft = wizard.draft().ok();
    let floppy_label = |name: &str| {
        draft.as_deref().map_or_else(
            || name.to_owned(),
            |d| {
                caps::value_label(
                    d,
                    "card[fdc-765].drives[unit=0].type",
                    &Value::Name(name.into()),
                )
            },
        )
    };
    let mut options = vec![(None, "Not installed".to_owned())];
    options.extend(
        assembly::descriptors::FLOPPY_TYPES
            .iter()
            .map(|t| (Some(t.name.to_owned()), floppy_label(t.name))),
    );
    if let Some(kind) = candidate_combo(
        ui,
        wizard,
        "Drive B:",
        "floppy-b",
        &options,
        &hardware.floppies[1],
        |s, v| s.hardware.floppies[1] = v.clone(),
    ) {
        wizard.edit(|s| s.hardware.floppies[1] = kind);
    }
    small(
        ui,
        "Media catalogues start empty; insert installation media after Start.",
    );
    ui.separator();
    ui.strong("Disk controller — AT fixed-disk adapter");
    let serves = wizard
        .bios_rom()
        .and_then(|b| b.entry())
        .is_some_and(|e| e.provides.contains(&assembly::FirmwareService::FixedDisk));
    let bios_name = wizard
        .bios_rom()
        .map_or_else(|| "system BIOS".into(), |b| b.package.config.name.clone());
    small(
        ui,
        if serves {
            format!("Option ROMs whose catalogue entry fits this adapter. The {bios_name} already serves fixed disks, so an option ROM is optional.")
        } else {
            format!("Option ROMs whose catalogue entry fits this adapter. The {bios_name} serves floppies only: fixed disks need an option ROM.")
        },
    );
    if ui
        .radio(
            wizard.selection().option_rom.is_none(),
            egui::RichText::new("None").strong(),
        )
        .clicked()
    {
        wizard.choose_option_rom(None);
    }
    if let Some(rom) = picker(ui, wizard, Slot::OptionRom, false) {
        wizard.choose_option_rom(Some(rom));
    }
    // The adapter is fitted only with a disk or an option ROM for it.
    if !wizard.choices("card[ide].port").is_empty() {
        if let Some(Value::Integer(port)) = api_combo(
            ui,
            wizard,
            "Channel",
            "card[ide].port",
            Value::Integer(hardware.ide_port.into()),
        ) {
            wizard.edit(|s| s.hardware.ide_port = port as u16);
        }
    }
    for (unit, name) in [(0, "C:"), (1, "D:")] {
        disk(ui, wizard, unit, name);
    }
    ui.add(egui::Label::new(egui::RichText::new("A new blank disk costs nothing until written. It is stored as a zero base, a few bytes that record its size and geometry, in the store, in a registry and on this computer. Only sectors the guest writes are stored, later, as disk layers.").strong()).wrap());
}

fn disk(ui: &mut egui::Ui, wizard: &mut Wizard, unit: usize, name: &str) {
    let current = wizard.hardware().disks[unit];
    let size = current.unwrap_or(create::BLANK_DISKS[0]);
    let options = [
        (false, "None".to_owned()),
        (
            true,
            format!(
                "New blank disk (IDE {})",
                if unit == 0 { "master" } else { "slave" }
            ),
        ),
    ];
    if let Some(on) = candidate_combo(
        ui,
        wizard,
        name,
        &format!("disk-{unit}"),
        &options,
        &current.is_some(),
        |s, on| s.hardware.disks[unit] = on.then_some(size),
    ) {
        wizard.edit(|s| s.hardware.disks[unit] = on.then_some(size));
    }
    let Some(chs) = wizard.hardware().disks[unit] else {
        return;
    };
    let mut presets: Vec<(Option<Chs>, String)> = create::BLANK_DISKS
        .iter()
        .map(|c| {
            (
                Some(*c),
                format!(
                    "{} · {}/{}/{}",
                    library_model::listing::human_size(c.bytes()),
                    c.cylinders,
                    c.heads,
                    c.sectors
                ),
            )
        })
        .collect();
    presets.push((None, "Custom geometry…".into()));
    let selected = if wizard.custom[unit] || !create::BLANK_DISKS.contains(&chs) {
        None
    } else {
        Some(chs)
    };
    // Custom geometry is checked with a geometry no preset lists, so a BIOS
    // that only serves its CMOS disk types gives this option its reason.
    let probe = if create::BLANK_DISKS.contains(&chs) {
        Chs {
            cylinders: chs.cylinders.saturating_sub(1).max(1),
            ..chs
        }
    } else {
        chs
    };
    if let Some(choice) = candidate_combo(
        ui,
        wizard,
        &format!("{name} size"),
        &format!("disk-size-{unit}"),
        &presets,
        &selected,
        |s, v| s.hardware.disks[unit] = Some(v.unwrap_or(probe)),
    ) {
        match choice {
            Some(c) => {
                wizard.custom[unit] = false;
                wizard.edit(|s| s.hardware.disks[unit] = Some(c));
            }
            None => wizard.custom[unit] = true,
        }
    }
    if selected.is_none() {
        let mut c = chs;
        ui.horizontal(|ui| {
            ui.add_sized([150.0, 20.0], egui::Label::new("Geometry"));
            ui.add(egui::DragValue::new(&mut c.cylinders).range(1..=65535));
            ui.label("cylinders");
            ui.add(egui::DragValue::new(&mut c.heads).range(1..=255));
            ui.label("heads");
            ui.add(egui::DragValue::new(&mut c.sectors).range(1..=255));
            ui.label("sectors");
        });
        if c != chs {
            wizard.edit(|s| s.hardware.disks[unit] = Some(c));
        }
        small(
            ui,
            format!(
                "{} · CHS addressing",
                library_model::listing::human_size(c.bytes())
            ),
        );
    }
}

fn page_name(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.heading("Name and tag");
    small(
        ui,
        "The image is stored as repository:tag. The machine configuration itself holds no name.",
    );
    let mut repo = wizard.repo.clone();
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 20.0], egui::Label::new("Repository name"));
        ui.text_edit_singleline(&mut repo);
    });
    if repo != wizard.repo {
        wizard.set_repo(repo);
    }
    small(ui, "Lowercase letters and digits, separated by . _ or -; for example my-at-286, then my-at-286-dos once DOS is installed.");
    let mut tag = wizard.tag.clone();
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 20.0], egui::Label::new("Tag"));
        ui.text_edit_singleline(&mut tag);
    });
    ui.horizontal(|ui| {
        let next = wizard.next_version(wizard.repo.trim());
        let mut chips = vec!["1.0.0".to_owned()];
        if next != "1.0.0" {
            chips.push(next);
        }
        chips.push("latest".into());
        chips.push(wizard.hardware_tag());
        for chip in chips {
            if ui.button(&chip).clicked() {
                tag = chip;
            }
        }
    });
    let check = wizard.tag_check();
    match check.level {
        // The new-machine mock-up checks a free tag off with ✓.
        TagLevel::Ok => ui.label(format!("✓ {}", check.text)),
        TagLevel::Warn | TagLevel::Bad => ui.add(
            egui::Label::new(egui::RichText::new(&check.text).color(ui.visuals().warn_fg_color))
                .wrap(),
        ),
    };
    if let Some(fix) = &check.fix {
        if ui.button(format!("Use {fix}")).clicked() {
            tag = fix.clone();
        }
    }
    if tag != wizard.tag {
        wizard.set_tag(tag);
    }
    let mut title = wizard.title().to_owned();
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 20.0], egui::Label::new("Title (optional)"));
        ui.add(egui::TextEdit::singleline(&mut title).hint_text("Open AT workbench"));
    });
    if title != wizard.title() {
        wizard.set_title(title);
    }
    small(ui, "Shown in the library; stored as the image's title.");
    small(ui, "The image is the immutable recipe: configuration, firmware, blank disks, empty media catalogues.");
    ui.separator();
    // The machines mock-up: the step also names the machine made from it.
    ui.strong("Machine");
    ui.checkbox(&mut wizard.make_machine, "Also create a machine from it");
    ui.add_enabled_ui(wizard.make_machine, |ui| {
        ui.horizontal(|ui| {
            ui.add_sized([150.0, 20.0], egui::Label::new("Machine name"));
            let hint = if wizard.title().trim().is_empty() {
                "the image's title".to_owned()
            } else {
                wizard.title().trim().to_owned()
            };
            ui.add(egui::TextEdit::singleline(&mut wizard.machine_name).hint_text(hint));
        });
    });
    if wizard.make_machine {
        let name = wizard
            .machine_choice()
            .flatten()
            .unwrap_or_else(|| "machine".into());
        small(
            ui,
            format!(
                "Defaults to the image title. Workspace id for the command line: {}.",
                image_store::machine::slug(&name)
            ),
        );
        if let Some(problem) = wizard.machine_problem() {
            ui.colored_label(ui.visuals().warn_fg_color, problem);
        }
    } else {
        small(ui, "No machine: the image waits in Images until you start it or choose New machine from image….");
    }
    ui.separator();
    ui.strong("Which tags can move");
    small(ui, "Version tags, shaped like x.y.z or x.y.z-pre such as 1.0.0 or 1.1.0-beta.1, never move: once 1.0.0 names an image, it always names that image.");
    small(ui, "Other tags, latest or a descriptive tag such as 8mhz-1mb-2mbems, may later move to a newer image, but only after a confirmation that shows the old and the new digest. The old image stays reachable by its digest and its other tags.");
    small(ui, "Every tag follows the registry tag grammar: a letter, digit or _, then up to 127 letters, digits, ., _ or -. A + build suffix is not allowed.");
    small(ui, wizard.full_reference());
}

fn page_summary(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.heading("Summary");
    small(
        ui,
        &match wizard.machine_choice() {
            Some(name) => format!(
                "Create builds the image {} in your local store and the machine \u{201c}{}\u{201d} from it, selected in Machines and ready to Start. Use Back to change anything.",
                wizard.reference(),
                name.unwrap_or_else(|| "named after the image".into())
            ),
            None => format!(
                "Create builds the image {} in your local store. Use Back to change anything.",
                wizard.reference()
            ),
        },
    );
    egui::Grid::new("new-machine-summary")
        .num_columns(2)
        .show(ui, |ui| {
            for (key, value) in wizard.summary() {
                ui.monospace(key);
                ui.add(egui::Label::new(egui::RichText::new(value).monospace()).wrap());
                ui.end_row();
            }
        });
    ui.separator();
    ui.strong("New bytes in the store");
    small(ui, "A few KiB: configuration and metadata. Firmware blobs are shared with their packages; blank disks are zero bases.");
    ui.strong("Redistribution");
    match wizard.private_notice() {
        Some(notice) => why(ui, notice),
        None => small(ui, "Every blob is open: the image can be pushed to a private registry or to the public unemu/ namespace."),
    }
    small(ui, "The generated configuration is open, because it is our own file; the image's class still comes from its blobs.");
    ui.strong("Next steps");
    small(ui, format!("Start it, insert FreeDOS from Installation media and install it to C:, then save it under a new name such as {}-dos:1.0.0. That image records this one as its base.", wizard.repo.trim()));
}
