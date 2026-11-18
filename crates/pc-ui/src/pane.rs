// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's details panes: the shared layout comes from
//! [`ui_kit::pane`], which the approved machines mock-up
//! (`backlog/docs/design/mockups/machines.html`) draws; the PC firmware
//! pieces of a pane (its Firmware section, the "newer version available"
//! hint and the firmware digest) are here, with the PC dialogs, for the
//! library's frame and collections to draw.

use egui::{RichText, Stroke, Ui};
// The library views name the shared pane helpers through this module.
pub use ui_kit::pane::*;

/// The short digest the mock-ups show: the first eight and the last four
/// hex digits.
pub fn short_digest(hex: &str) -> String {
    library_model::listing::short(hex)
}

/// What the "newer version available" hint asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HintAction {
    /// "Switch to …": open Settings with the replacement chosen.
    pub switch: bool,
    /// The newer tag of the same repository that already uses it.
    pub newer_tag: bool,
}

/// The "newer version available" hint for a superseded release: the
/// catalogue's stored reason, what a switch does (`after`), a link to a
/// newer tag of the image's repository that already uses the replacement,
/// and the Switch button. It is not a warning and cannot be dismissed: it
/// stays while the firmware is superseded.
pub fn newer_hint(
    ui: &mut Ui,
    newer: &crate::firmware_choices::Newer,
    after: &str,
    newer_tag: Option<&str>,
) -> HintAction {
    let mut action = HintAction::default();
    ui.add_space(6.0);
    egui::Frame::new()
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new("↑").strong().color(good_colour(ui.visuals())));
                ui.vertical(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(format!(
                            "Newer version available. {}.",
                            newer.text()
                        )))
                        .wrap(),
                    );
                    meta(ui, after);
                    if let Some(tag) = newer_tag {
                        ui.horizontal_wrapped(|ui| {
                            action.newer_tag = ui.link(RichText::new(tag).small()).clicked();
                            ui.label(
                                RichText::new(format!(
                                    "in your library already uses {}: new machines can start from it.",
                                    crate::firmware_choices::release(newer.by)
                                ))
                                .small()
                                .weak(),
                            );
                        });
                    }
                    action.switch = ui
                        .button(format!(
                            "Switch to {}…",
                            crate::firmware_choices::release(newer.by)
                        ))
                        .clicked();
                });
            });
        });
    action
}

/// The Firmware section of a machine or an image: each ROM by its role with
/// its catalogue or library name and its open or user-supplied mark. A
/// machine's rows say whether each comes from the image or is the machine's
/// own choice (`machine` lists those choices).
pub fn firmware_section(
    ui: &mut Ui,
    image: Option<&pc_library::hardware::ImageFacts>,
    machine: Option<&[crate::firmware_choices::Own]>,
) {
    section(ui, "Firmware");
    let Some(image) = image else {
        meta(ui, "Checking image…");
        return;
    };
    if image.firmware.is_empty() {
        meta(ui, "No firmware recorded.");
        return;
    }
    facts(ui, "firmware-rows", |ui| {
        for rom in &image.firmware {
            let own = machine.and_then(|own| own.iter().find(|o| o.slot.label() == rom.role));
            fact(ui, &rom.role, |ui| {
                ui.horizontal_wrapped(|ui| {
                    match own {
                        Some(own) => {
                            ui.add(
                                egui::Label::new(crate::firmware_choices::name(own.entry)).wrap(),
                            );
                            rights(ui, own.open);
                        }
                        None => {
                            ui.add(egui::Label::new(&rom.name).wrap());
                            if let Some(open) = rom.open {
                                rights(ui, open);
                            }
                        }
                    }
                    if machine.is_some() {
                        ui.label(
                            RichText::new(if own.is_some() {
                                "· this machine's choice"
                            } else {
                                "· from the image"
                            })
                            .weak(),
                        );
                    }
                });
            });
        }
    });
}
