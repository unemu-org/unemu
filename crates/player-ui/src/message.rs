// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player's message boxes, as the player mock-up draws its refusals
//! (`backlog/docs/design/mockups/player.html`, "Message boxes"): the refused
//! command as the title, the refusal mark beside the text, and one OK.
//!
//! The player shows one over itself when its machine stops with an error or
//! cannot start again. The application shows the same box for a file that
//! does not open, with Open as its title.

use crate::{Dialog, Player};
use ui_kit::shell_icons::Icon;

/// One message with one OK.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageBox {
    /// The command that was refused: Open, Power cycle.
    pub title: String,
    pub text: String,
}

impl MessageBox {
    /// A refusal: the red mark beside `text`.
    pub fn refusal(title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            text: text.into(),
        }
    }

    /// Draw the box over everything in `ctx`; `true` once OK or Escape
    /// dismissed it. A click outside leaves it, as the mock-up's boxes stay.
    /// `classic` draws the classic title strip.
    pub fn show(&self, ctx: &egui::Context, classic: bool) -> bool {
        let mut done = false;
        egui::Modal::new(egui::Id::unique("player-message")).show(ctx, |ui| {
            ui.set_width(420.0);
            if classic {
                ui_kit::skin::gradient_title(ui, &self.title);
            } else {
                ui.heading(&self.title);
            }
            ui.horizontal_top(|ui| {
                ui_kit::shell_icons::show(ui, Icon::Refusal);
                ui.add(egui::Label::new(self.text.as_str()).wrap());
            });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("OK").clicked() {
                    done = true;
                }
            });
            if ui.input_mut(ui_kit::skin::escape_pressed) {
                done = true;
            }
        });
        done
    }
}

impl Player {
    /// Draw the player's dialog, if one is open. The Input dialog's OK gives
    /// the session its new pad keys and asks the application to store them.
    pub fn dialogs(&mut self, ctx: &egui::Context) -> Option<crate::Request> {
        match self.dialog.as_mut()? {
            Dialog::Message(message) => {
                if message.show(ctx, self.classic) {
                    self.dialog = None;
                }
                None
            }
            Dialog::Input(input) => match input.show(ctx, &self.pads, self.classic) {
                Some(Some(bindings)) => {
                    self.dialog = None;
                    if let Some(session) = &mut self.session {
                        session.set_pad_bindings(bindings.clone());
                    }
                    let changed = bindings != self.bindings;
                    self.bindings = bindings;
                    self.message = "Input saved".into();
                    changed.then_some(crate::Request::PadBindings)
                }
                Some(None) => {
                    self.dialog = None;
                    None
                }
                None => None,
            },
        }
    }
}
