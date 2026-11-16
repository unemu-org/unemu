// SPDX-License-Identifier: AGPL-3.0-or-later
//! What every details pane of the library shares, as the approved machines
//! mock-up (`backlog/docs/design/mockups/machines.html`) draws it: a kind
//! tile beside the title block, label and value rows, sections side by side
//! that stack when the pane is narrow, mini tables, and the actions as framed
//! buttons in one wrapping row at the bottom with a short note under them.
//!
//! A pane uses two text sizes: the body style, and the small style for meta
//! lines at the mock-up's 11 px; digests and ids are monospace at that size.
//! A pane's title is the mock-up's 15 px heading.

use egui::{Align, Color32, Layout, RichText, Stroke, TextStyle, Ui, Vec2};

/// The mock-up's meta and code text beside its 12 px body text.
const META_SIZE: f32 = 11.0;
/// The mock-up's pane title.
const TITLE_SIZE: f32 = 15.0;
/// Two sections sit side by side from this pane width; narrower, they stack.
const SPLIT_WIDTH: f32 = 720.0;
/// Three sections sit side by side from this pane width.
const SPLIT3_WIDTH: f32 = 1080.0;
/// Room between sections side by side.
const GAP: f32 = 16.0;
/// The kind tile: the mock-up's 3:4 cover, 84 px wide there.
const TILE: Vec2 = Vec2::new(70.0, 93.0);
/// Height of the tile's kind band.
const BAND: f32 = 16.0;

/// Scope the pane's text styles and widget outlines. Buttons and text
/// fields get a visible outline, so every action reads as a framed button
/// and the labels field as a field in every skin and theme. A skin that
/// already outlines resting controls keeps its outline, so the pane draws
/// the same outline colour as the rest of the window.
pub(super) fn style(ui: &mut Ui) {
    let style = ui.style_mut();
    for (text, size) in [
        (TextStyle::Small, META_SIZE),
        (TextStyle::Monospace, META_SIZE),
        (TextStyle::Heading, TITLE_SIZE),
    ] {
        if let Some(font) = style.text_styles.get_mut(&text) {
            font.size = size;
        }
    }
    let visuals = &mut style.visuals;
    if !visuals.widgets.inactive.bg_stroke.is_empty() {
        return;
    }
    let outline = visuals
        .widgets
        .noninteractive
        .fg_stroke
        .color
        .gamma_multiply(0.45);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, outline);
}

/// Meta text: the small style, muted, wrapped.
pub(super) fn meta(ui: &mut Ui, text: impl Into<String>) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).small().weak()).wrap())
}

/// Body text wrapped to the available width.
pub(super) fn text(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).wrap())
}

/// Monospace text for digests and ids.
pub(super) fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).monospace()
}

/// A section title: the mock-up's bold body-size heading.
pub(super) fn section(ui: &mut Ui, title: impl Into<String>) {
    ui.add_space(6.0);
    ui.add(egui::Label::new(RichText::new(title).strong()).wrap());
}

/// The pane's title line: the kind icon, the name and a favourite star.
pub(super) fn title(ui: &mut Ui, icon: crate::shell_icons::Icon, name: &str, favourite: bool) {
    ui.horizontal_wrapped(|ui| {
        crate::shell_icons::show(ui, icon);
        ui.add(egui::Label::new(RichText::new(name).heading().strong()).wrap());
        if favourite {
            ui.label(
                RichText::new("★")
                    .heading()
                    .color(crate::shell_icons::STAR_GOLD),
            );
        }
    });
}

/// What the kind tile shows.
pub(super) struct Tile<'a> {
    pub title: &'a str,
    /// MACHINE, IMAGE, MEDIA or FIRMWARE.
    pub kind: &'a str,
    /// User-supplied content takes the mock-up's amber tile.
    pub user_supplied: bool,
}

/// The mock-up's cover: the title on a gradient, the kind in a band.
fn tile(ui: &mut Ui, tile: &Tile<'_>) {
    let (rect, _) = ui.allocate_exact_size(TILE, egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let (top, bottom) = if tile.user_supplied {
        (Color32::from_rgb(122, 90, 18), Color32::from_rgb(58, 42, 8))
    } else {
        (
            Color32::from_rgb(75, 121, 193),
            Color32::from_rgb(19, 51, 111),
        )
    };
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), bottom),
        (rect.left_bottom(), bottom),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    let painter = ui.painter().with_clip_rect(rect);
    painter.add(mesh);
    let font = egui::FontId::monospace(META_SIZE);
    let mut job = egui::text::LayoutJob::single_section(
        tile.title.to_owned(),
        egui::TextFormat::simple(font.clone(), Color32::WHITE),
    );
    // Break between words, as the cover's text does; egui still breaks a
    // word wider than the tile, so a long one is never cut.
    job.wrap = egui::text::TextWrapping {
        max_width: rect.width() - 8.0,
        max_rows: 4,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job.halign = Align::Center;
    let galley = ui.painter().layout_job(job);
    painter.galley(
        egui::pos2(rect.center().x, rect.top() + 6.0),
        galley,
        Color32::WHITE,
    );
    let band = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - BAND), rect.max);
    painter.rect_filled(band, 0.0, Color32::from_black_alpha(102));
    painter.text(
        band.center(),
        egui::Align2::CENTER_CENTER,
        tile.kind,
        egui::FontId::proportional(META_SIZE),
        Color32::WHITE,
    );
}

/// The title block: the kind tile beside what `body` draws, as the mock-up's
/// head of a details pane.
pub(super) fn head(ui: &mut Ui, kind: &Tile<'_>, body: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        tile(ui, kind);
        ui.add_space(4.0);
        let width = ui.available_width();
        ui.allocate_ui_with_layout(Vec2::new(width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(width);
            body(ui);
        });
    });
}

fn column(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(Vec2::new(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

/// Two sections side by side, the first taking `share` of the width, as the
/// mock-up's detail grid; stacked when the pane is narrow.
pub(super) fn split(
    ui: &mut Ui,
    share: f32,
    first: impl FnOnce(&mut Ui),
    second: impl FnOnce(&mut Ui),
) {
    let width = ui.available_width();
    if width < SPLIT_WIDTH {
        first(ui);
        second(ui);
        return;
    }
    let spacing = ui.spacing().item_spacing.x;
    let left = ((width - GAP) * share).floor();
    let right = width - GAP - left;
    ui.horizontal_top(|ui| {
        column(ui, left, first);
        ui.add_space(GAP - spacing);
        column(ui, right, second);
    });
}

/// Three sections side by side on a wide pane; narrower, the first two side
/// by side and the third under them, or all three stacked.
pub(super) fn split3(
    ui: &mut Ui,
    first: impl FnOnce(&mut Ui),
    second: impl FnOnce(&mut Ui),
    third: impl FnOnce(&mut Ui),
) {
    let width = ui.available_width();
    if width < SPLIT3_WIDTH {
        split(ui, 0.5, first, second);
        third(ui);
        return;
    }
    let spacing = ui.spacing().item_spacing.x;
    let each = ((width - 2.0 * GAP) / 3.0).floor();
    ui.horizontal_top(|ui| {
        column(ui, each, first);
        ui.add_space(GAP - spacing);
        column(ui, each, second);
        ui.add_space(GAP - spacing);
        column(ui, width - 2.0 * (each + GAP), third);
    });
}

/// Label and value rows: the mock-up's description list, labels muted.
pub(super) fn facts(ui: &mut Ui, id: &str, rows: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, rows);
}

/// One row of [`facts`]: the label, then what `value` draws, wrapped.
pub(super) fn fact(ui: &mut Ui, label: &str, value: impl FnOnce(&mut Ui)) {
    ui.add(egui::Label::new(RichText::new(label).weak()).extend());
    ui.vertical(|ui| {
        ui.set_max_width(ui.available_width());
        value(ui);
    });
    ui.end_row();
}

/// One row of [`facts`] with a wrapped text value.
pub(super) fn fact_text(ui: &mut Ui, label: &str, value: impl Into<egui::WidgetText>) {
    let value = value.into();
    fact(ui, label, |ui| {
        text(ui, value);
    });
}

/// The mock-ups' colour for an open mark.
pub(super) fn good_colour(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(143, 214, 160)
    } else {
        Color32::from_rgb(31, 107, 51)
    }
}

/// A small chip: the mock-up's open and user-supplied marks.
pub(super) fn badge(ui: &mut Ui, label: &str, open: bool) {
    let colour = if open {
        good_colour(ui.visuals())
    } else {
        ui.visuals().warn_fg_color
    };
    egui::Frame::new()
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(label).small().color(colour));
        });
}

/// The redistribution chip of a ROM, an image or a set.
pub(super) fn rights(ui: &mut Ui, open: bool) {
    badge(ui, if open { "open" } else { "user-supplied" }, open);
}

/// How a mini-table line reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Body,
    Muted,
    /// What this machine changed: the mock-up's warning-coloured small line.
    Change,
    /// What waits for the next Start: the mock-up's italic small line.
    Pending,
}

/// One row of a [`table`]: its head cell, its other cells as lines, and
/// whether it shows a change, which tints it.
pub(super) struct Row {
    pub head: String,
    pub cells: Vec<Vec<(String, Tone)>>,
    pub changed: bool,
}

/// The mock-ups' face colour: the rule under every mini-table row.
fn face_colour(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(52, 54, 58)
    } else {
        Color32::from_rgb(214, 211, 203)
    }
}

/// The mock-ups' second face colour: a repository's group row in a list.
pub(super) fn group_colour(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(62, 64, 69)
    } else {
        Color32::from_rgb(231, 228, 220)
    }
}

/// The mock-ups' shadow colour: the thin inset outline of a mini table.
fn shadow_colour(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(31, 32, 35)
    } else {
        Color32::from_rgb(141, 138, 131)
    }
}

/// The mock-ups' tint of a changed mini-table row.
fn changed_colour(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(58, 51, 36)
    } else {
        Color32::from_rgb(255, 248, 224)
    }
}

/// A mini-table cell's padding: the mock-up's 6 px beside and 4 px above
/// and below its text.
const CELL_PAD: Vec2 = Vec2::new(6.0, 4.0);
/// The most of a mini table's width its first column takes, so the other
/// columns keep room on a narrow pane.
const FIRST_SHARE: f32 = 0.35;

/// The mock-up's mini table: the field colour inside a thin inset outline,
/// a muted header row, a first column in the strong text colour, a 1 px rule
/// in the face colour under the header and every row, and changed rows
/// tinted across the table. The mock-up's first column is bold; egui has no
/// bold face loaded, so the strong colour marks it instead. The first column
/// is as wide as its widest cell up to a share of the table and wraps only
/// beyond that; the other columns share the rest of the width, their cells
/// wrapped, so every cell stays inside the outline.
pub(super) fn table(ui: &mut Ui, id: &str, header: &[&str], rows: &[Row]) {
    let visuals = ui.visuals().clone();
    let head = |text: &str| RichText::new(text).strong();
    let muted = |text: &str| RichText::new(text).weak();
    let measure = |text: RichText| {
        egui::WidgetText::from(text)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                TextStyle::Body,
            )
            .size()
            .x
    };
    let first = rows
        .iter()
        .map(|row| measure(head(&row.head)))
        .chain(header.first().map(|label| measure(muted(label))))
        .fold(0.0, f32::max)
        .ceil();
    egui::Frame::new()
        .fill(visuals.text_edit_bg_color())
        .stroke(Stroke::new(1.0, shadow_colour(&visuals)))
        .show(ui, |ui| {
            ui.push_id(id, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let width = ui.available_width();
                ui.set_width(width);
                let others = header.len().max(2) - 1;
                let gaps = 2.0 * CELL_PAD.x * (others + 1) as f32;
                let room = (width - gaps).max(0.0);
                let first = first.min((room * FIRST_SHARE).floor());
                let cell = ((room - first) / others as f32).floor();
                let widths: Vec<f32> = std::iter::once(first)
                    .chain(std::iter::repeat_n(cell, others))
                    .collect();
                let rule = face_colour(&visuals);
                table_line(ui, &widths, None, rule, |ui, column| {
                    if let Some(label) = header.get(column) {
                        ui.add(egui::Label::new(muted(label)).wrap());
                    }
                });
                for row in rows {
                    debug_assert_eq!(row.cells.len() + 1, header.len(), "{}", row.head);
                    let tint = row.changed.then(|| changed_colour(&visuals));
                    table_line(ui, &widths, tint, rule, |ui, column| {
                        if column == 0 {
                            ui.add(egui::Label::new(head(&row.head)).wrap());
                            return;
                        }
                        for (line, tone) in row.cells.get(column - 1).into_iter().flatten() {
                            let rich = RichText::new(line);
                            let rich = match tone {
                                Tone::Body => rich,
                                Tone::Muted => rich.weak(),
                                Tone::Change => rich.small().color(ui.visuals().warn_fg_color),
                                Tone::Pending => rich.small().italics(),
                            };
                            ui.add(egui::Label::new(rich).wrap());
                        }
                    });
                }
            });
        });
}

/// One line of a [`table`], its header or a row: the cells side by side at
/// `widths`, the tint behind them, and the rule under them.
fn table_line(
    ui: &mut Ui,
    widths: &[f32],
    tint: Option<Color32>,
    rule: Color32,
    mut cell: impl FnMut(&mut Ui, usize),
) {
    let width = ui.available_width();
    egui::Frame::new()
        .fill(tint.unwrap_or(Color32::TRANSPARENT))
        .inner_margin(egui::Margin::symmetric(CELL_PAD.x as i8, CELL_PAD.y as i8))
        .show(ui, |ui| {
            ui.set_width((width - 2.0 * CELL_PAD.x).max(0.0));
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0 * CELL_PAD.x, 2.0);
                for (index, cell_width) in widths.iter().enumerate() {
                    column(ui, *cell_width, |ui| cell(ui, index));
                }
            });
        });
    let (line, _) = ui.allocate_exact_size(Vec2::new(width.max(0.0), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(line, 0.0, rule);
}

/// The default action of a pane, as the mock-ups' default button: bold,
/// with a darker outline than the other actions.
pub(super) fn primary(ui: &Ui, label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label).strong()).stroke(Stroke::new(
        1.5,
        ui.visuals().widgets.noninteractive.fg_stroke.color,
    ))
}

/// The pane's actions: framed buttons in one row that wraps.
pub(super) fn actions(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        add(ui);
    });
}

/// The short digest the mock-ups show: the first eight and the last four
/// hex digits.
pub(super) fn short_digest(hex: &str) -> String {
    super::firmware::short(hex)
}

/// What the "newer version available" hint asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HintAction {
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
pub(super) fn newer_hint(
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
pub(super) fn firmware_section(
    ui: &mut Ui,
    image: Option<&super::hardware::ImageFacts>,
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
