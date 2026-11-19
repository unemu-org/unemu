// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine › Input…: which host keys work the machine's pads
//! (core-contract.md §13.3, "Pads from the keyboard", and the player mock-up's
//! Input dialog).
//!
//! One tab per pad port, by the port's label; a row per control the port
//! lists, with its keys by readable names. Click a row and press a key: the
//! key replaces the row's keys and leaves any other control that had it.
//! While a row waits, Esc leaves it as it was and Delete clears it, so
//! neither key can be bound. Defaults restores every player's keys. Nothing
//! changes until OK; Cancel and the window's Escape leave the keys as they
//! were.
//!
//! The key a waiting row takes comes from the application's raw key events
//! ([`crate::Player::key_event`]), not from egui: a pad may be worked by a
//! modifier such as the right Shift, which egui reports as no key.

use crate::PadPort;
use machine_view::{keys, pads::PadBindings};
use unemu_core_api::{Key, PadControl};
use winit::keyboard::KeyCode;

/// The words of the dialog's one hint line.
pub(crate) const HINT: &str =
    "Click a row, then press a key. Esc leaves it as it was; Delete clears it.";

/// A key by a readable name, derived from the name the preferences file
/// stores it by: `ARROW_UP` reads Arrow Up, `SHIFT_RIGHT` Shift Right,
/// `DIGIT_1` reads 1 and a letter stays itself.
pub fn key_label(key: Key) -> String {
    let Some(name) = keys::name(key) else {
        return format!("Key {:#04x}", key.0);
    };
    if let Some(digit) = name.strip_prefix("DIGIT_") {
        return digit.to_owned();
    }
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_string() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A control's name for people, as the mock-up's rows have them.
pub(crate) fn control_label(control: PadControl) -> &'static str {
    match control {
        PadControl::Up => "Up",
        PadControl::Down => "Down",
        PadControl::Left => "Left",
        PadControl::Right => "Right",
        PadControl::A => "A",
        PadControl::B => "B",
        PadControl::X => "X",
        PadControl::Y => "Y",
        PadControl::L => "L",
        PadControl::R => "R",
        PadControl::Start => "Start",
        PadControl::Select => "Select",
        _ => "Control",
    }
}

/// The open dialog: the keys as they will be after OK, the tab shown and
/// the row that waits for a key.
pub(crate) struct InputDialog {
    pub draft: PadBindings,
    pub tab: usize,
    pub waiting: Option<PadControl>,
    /// The Escape that left a waiting row came in as a raw key; the same
    /// press reaches egui and must not also cancel the dialog.
    escape_taken: bool,
}

impl InputDialog {
    pub fn new(draft: PadBindings) -> Self {
        Self {
            draft,
            tab: 0,
            waiting: None,
            escape_taken: false,
        }
    }

    /// A host key pressed while the dialog is open: it binds, clears or
    /// leaves the waiting row, and does nothing otherwise. A key the
    /// contract does not name leaves the row waiting.
    pub fn key(&mut self, key: KeyCode) {
        let Some(control) = self.waiting else {
            return;
        };
        match key {
            KeyCode::Escape => self.escape_taken = true,
            KeyCode::Delete => self.draft.unbind(self.tab, control),
            other => {
                let Some(usage) = keys::key_usage(other) else {
                    return;
                };
                self.draft.unbind(self.tab, control);
                self.draft.bind(self.tab, control, usage);
            }
        }
        self.waiting = None;
    }

    /// The keys of `control` of the shown player, by readable names.
    pub fn keys_of(&self, control: PadControl) -> Vec<String> {
        self.draft
            .entries()
            .filter(|&(player, bound, _)| player == self.tab && bound == control)
            .map(|(_, _, key)| key_label(key))
            .collect()
    }

    /// Draw the dialog. `Some(Some(keys))` on OK, `Some(None)` once it was
    /// cancelled, `None` while it stays open.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        pads: &[PadPort],
        classic: bool,
    ) -> Option<Option<PadBindings>> {
        let mut answer = None;
        if self.tab >= pads.len() {
            self.tab = 0;
        }
        egui::Modal::new(egui::Id::unique("player-input")).show(ctx, |ui| {
            ui.set_width(400.0);
            if classic {
                ui_kit::skin::gradient_title(ui, "Input");
            } else {
                ui.heading("Input");
            }
            ui.horizontal(|ui| {
                for (index, pad) in pads.iter().enumerate() {
                    if ui
                        .selectable_label(index == self.tab, pad.label.as_str())
                        .clicked()
                    {
                        self.tab = index;
                        self.waiting = None;
                    }
                }
            });
            let controls = pads.get(self.tab).map_or(&[][..], |pad| &pad.controls[..]);
            egui::Grid::new("player-input-binds")
                .num_columns(2)
                .striped(true)
                .min_col_width(160.0)
                .show(ui, |ui| {
                    ui.strong("Control");
                    ui.strong("Keys");
                    ui.end_row();
                    for &control in controls {
                        let waiting = self.waiting == Some(control);
                        let name = ui.selectable_label(waiting, control_label(control));
                        let keys = self.keys_of(control);
                        let text = if waiting {
                            egui::RichText::new("Press a key…").italics()
                        } else if keys.is_empty() {
                            egui::RichText::new("not bound").color(ui.visuals().weak_text_color())
                        } else {
                            egui::RichText::new(keys.join(", "))
                        };
                        let keys = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                        ui.end_row();
                        // Only a pointer's click waits: Enter or Space on a
                        // focused row is a key being bound, not a click.
                        let primary = egui::PointerButton::Primary;
                        if name.clicked_by(primary) || keys.clicked_by(primary) {
                            self.waiting = Some(control);
                        }
                    }
                });
            ui.add(egui::Label::new(egui::RichText::new(HINT).small()).wrap());
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Defaults").clicked() {
                    self.draft = PadBindings::defaults();
                    self.waiting = None;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        answer = Some(None);
                    }
                    if ui.button("OK").clicked() {
                        answer = Some(Some(self.draft.clone()));
                    }
                });
            });
            // A waiting row takes Escape for itself, whichever of the raw
            // key and egui's copy of it comes first.
            if ui.input_mut(ui_kit::skin::escape_pressed)
                && self.waiting.is_none()
                && !self.escape_taken
            {
                answer = Some(None);
            }
        });
        // A click outside leaves the dialog open, as the mock-up's dialogs
        // stay: closing then would lose the keys set so far.
        self.escape_taken = false;
        answer
    }
}
