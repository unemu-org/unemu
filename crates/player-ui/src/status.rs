// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player's status line, in the cells and order of every machine's
//! status bar (`backlog/docs/design/mockups/player.html`, "Status line"):
//! the message, a light with Running, Paused, Halted or Stopped where Power
//! is, a chip per removable slot where the drive chips are, the platform and
//! the content's name where the processor and EMS cells are, then sound and
//! speed. Only Running lights it.

use crate::{Condition, Player};
use egui::{Color32, Sense, Vec2};
use ui_kit::skin::{audio_mark, bevel};

/// The sound and speed cells' widths, as every machine's status bar has
/// them, wider when their text needs it.
const AUDIO: f32 = 118.0;
const SPEED: f32 = 124.0;
/// The widest the platform, the content's name or a slot's chip grows.
const NAME_MAX: f32 = 300.0;
const CHIP_MAX: f32 = 220.0;
/// The width the message keeps before the platform, the content's name or
/// a slot's chip get theirs, so the pointer-release hint stays readable; the
/// machine status line keeps the same.
pub(crate) const MESSAGE_MIN: f32 = 240.0;
/// The narrowest the platform, the content's name or a slot's chip becomes
/// before the message gives up its width.
const SHRUNK_MIN: f32 = 60.0;
/// A cell's text inset, across and down, and the gap between a light and
/// its text, as the mock-up's cells have them.
const PAD: f32 = 6.0;
const PAD_Y: f32 = 3.0;
const LED: Vec2 = Vec2::new(10.0, 6.0);
const LED_GAP: f32 = 5.0;

/// The green light of a running machine, or its dark off state.
fn power_colour(on: bool) -> Color32 {
    if on {
        Color32::from_rgb(54, 210, 74)
    } else {
        Color32::from_rgb(74, 74, 68)
    }
}

/// The amber light of a busy slot, or its dark off state.
fn slot_colour(ui: &egui::Ui, on: bool) -> Color32 {
    if on {
        Color32::from_rgb(255, 178, 31)
    } else if ui.visuals().dark_mode {
        Color32::from_rgb(42, 42, 39)
    } else {
        Color32::from_rgb(74, 74, 68)
    }
}

/// What one cell shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cell {
    Message(String),
    /// The light and its word.
    Light(bool, &'static str),
    /// A slot's chip: lit, its text, whether the slot is empty.
    Slot(bool, String, bool),
    Text(String),
}

impl Player {
    /// The status line's cells, left to right.
    pub(crate) fn status_cells(&self) -> Vec<Cell> {
        let condition = self.condition();
        let mut cells = vec![
            Cell::Message(self.status_message()),
            Cell::Light(condition == Condition::Running, condition.word()),
        ];
        for view in &self.slots {
            let lit = view.lit_until.is_some_and(|until| self.now < until);
            let inserted = view.inserted();
            cells.push(Cell::Slot(
                lit,
                format!("{}: {}", view.label, inserted.unwrap_or("empty")),
                inserted.is_none(),
            ));
        }
        cells.push(Cell::Text(self.prepared.platform_name().to_owned()));
        cells.push(Cell::Text(self.prepared.title().to_owned()));
        let (audio, speed) = match &self.session {
            Some(session) => {
                let text = session.audio.status_text();
                (
                    format!(
                        "{} {}",
                        audio_mark(session.audio.audible()),
                        text.strip_prefix("Audio ").unwrap_or(text)
                    ),
                    session.speed_status(),
                )
            }
            None => (format!("{} stopped", audio_mark(true)), "Speed 100%".into()),
        };
        cells.push(Cell::Text(audio));
        cells.push(Cell::Text(speed));
        cells
    }

    /// The message cell: a captured pointer says how to release it.
    fn status_message(&self) -> String {
        if self.pointer_captured() {
            if self.macos_shortcuts {
                "Pointer captured — ⌘G releases".into()
            } else {
                "Pointer captured — Ctrl+Alt+G releases".into()
            }
        } else {
            self.message.clone()
        }
    }

    /// Draw the status line into one row of `ui`.
    pub fn status(&self, ui: &mut egui::Ui) {
        draw_cells(ui, self.status_cells());
    }
}

/// Draw `cells` into one row of `ui`, each as wide as its text and its
/// light, the message taking what is left; return each cell's rectangle.
pub(crate) fn draw_cells(ui: &mut egui::Ui, cells: Vec<Cell>) -> Vec<egui::Rect> {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let measure = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    let last = cells.len() - 1;
    // Each cell's natural width and the narrowest it may become.
    let (natural, narrowest): (Vec<f32>, Vec<f32>) = cells
        .iter()
        .enumerate()
        .map(|(index, cell)| match cell {
            Cell::Message(_) => (MESSAGE_MIN, 0.0),
            // As wide as the widest word, so the line does not shift when
            // the machine pauses or stops, and never narrower: it is short.
            Cell::Light(..) => {
                let width = [
                    Condition::Running,
                    Condition::Paused,
                    Condition::Halted,
                    Condition::Stopped,
                ]
                .into_iter()
                .map(|condition| measure(condition.word()))
                .fold(0.0, f32::max)
                    + 2.0 * PAD
                    + LED.x
                    + LED_GAP;
                (width, width)
            }
            Cell::Slot(_, text, _) => {
                let width = (measure(text) + 2.0 * PAD + LED.x + LED_GAP).min(CHIP_MAX);
                (width, width.min(SHRUNK_MIN))
            }
            Cell::Text(text) if index == last => (SPEED.max(measure(text) + 2.0 * PAD), 0.0),
            Cell::Text(text) if index == last - 1 => (AUDIO.max(measure(text) + 2.0 * PAD), 0.0),
            Cell::Text(text) => {
                let width = (measure(text) + 2.0 * PAD).min(NAME_MAX);
                (width, width.min(SHRUNK_MIN))
            }
        })
        .collect();
    // What gives when the line is too narrow, in turn: the platform and the
    // content's name, which ellipsize, then the slots' chips, then the
    // message, and only then sound and speed.
    let group = |pick: fn(&Cell) -> bool| -> Vec<usize> {
        (0..cells.len())
            .filter(|&index| pick(&cells[index]))
            .collect()
    };
    let names: Vec<usize> = (0..cells.len())
        .filter(|&index| matches!(cells[index], Cell::Text(_)) && index + 2 < cells.len())
        .collect();
    let order = [
        names,
        group(|cell| matches!(cell, Cell::Slot(..))),
        group(|cell| matches!(cell, Cell::Message(_))),
        vec![last - 1, last],
    ];
    let available = ui.available_width();
    let mut widths = fit(available, &natural, &narrowest, &order);
    let used: f32 = widths.iter().sum();
    widths[0] += (available - used).max(0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(available, 20.0), Sense::hover());
    let mut x = rect.left();
    let mut areas = Vec::with_capacity(cells.len());
    for (cell, width) in cells.into_iter().zip(widths) {
        let area =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width, rect.height()));
        x += width;
        areas.push(area);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area.shrink2(Vec2::new(PAD, PAD_Y)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.set_clip_rect(area.intersect(ui.clip_rect()));
                ui.spacing_mut().item_spacing.x = LED_GAP;
                match &cell {
                    Cell::Light(on, word) => {
                        let (led, _) = ui.allocate_exact_size(LED, Sense::hover());
                        ui.painter().rect_filled(led, 0.0, power_colour(*on));
                        ui.label(*word);
                    }
                    Cell::Slot(lit, text, empty) => {
                        let (led, _) = ui.allocate_exact_size(LED, Sense::hover());
                        ui.painter().rect_filled(led, 0.0, slot_colour(ui, *lit));
                        let mut label = egui::RichText::new(text.as_str());
                        if *empty {
                            label = label.color(ui.visuals().weak_text_color());
                        }
                        ui.add(egui::Label::new(label).truncate())
                            .on_hover_text(text);
                    }
                    Cell::Message(text) | Cell::Text(text) => {
                        ui.add(egui::Label::new(text.as_str()).truncate())
                            .on_hover_text(text);
                    }
                }
            },
        );
        bevel(ui, area, true);
    }
    areas
}

/// Widths from `natural` that fit `available`: each group of `order` in
/// turn gives up width, its cells in proportion to what they have above
/// their `narrowest`, until the line fits or every group is at its
/// narrowest. Cells in no group keep their natural width.
fn fit(available: f32, natural: &[f32], narrowest: &[f32], order: &[Vec<usize>]) -> Vec<f32> {
    let mut widths = natural.to_vec();
    let mut excess = widths.iter().sum::<f32>() - available;
    for group in order {
        if excess <= 0.0 {
            break;
        }
        let slack: f32 = group.iter().map(|&i| widths[i] - narrowest[i]).sum();
        if slack <= 0.0 {
            continue;
        }
        let share = (excess / slack).min(1.0);
        for &i in group {
            widths[i] -= (widths[i] - narrowest[i]) * share;
        }
        excess -= slack * share;
    }
    widths
}
