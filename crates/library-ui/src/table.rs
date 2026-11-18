// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's list tables, as the approved machines and remotes mock-ups
//! (`backlog/docs/design/mockups/machines.html`, `remotes.html`) draw them:
//! the column headers that sort, the columns that fit the room a list has,
//! the striped and selectable rows with a name cell that leads with the
//! row's icon and badges, and a repository's group row. Every collection of
//! the library and Remotes draws its list with them; machine-images §3.

use ui_kit::{pane, shell_icons::Icon};

/// What a list row's name cell shows around the name, as the machines
/// mock-up draws it: the row kind's icon before the name, a warning and a
/// favourite star after it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lead {
    pub icon: Option<Icon>,
    /// A row inside an expanded repository group, set in under its header.
    pub child: bool,
    /// The row cannot be used as it is: unreadable, or its firmware is missing.
    pub warning: bool,
    pub favourite: bool,
}

/// The warning badge after a name, as the main-window mock-up marks a
/// machine that cannot start.
const WARNING_BADGE: &str = " ⚠";
const FAVOURITE_BADGE: &str = " ★";
/// The set-in of a group's child rows.
const CHILD_INDENT: f32 = 16.0;
/// The icon's 16 points, its 2-point margin and the mock-up's 5-point gap
/// (rounded) before the name.
const NAME_ICON_ROOM: f32 = 21.0;

/// A list header cell's height.
const HEADER_HEIGHT: f32 = 22.0;
/// The room the library mock-up's list header keeps beside each label. Its
/// 3 px above and below are what centring the label in [`HEADER_HEIGHT`]
/// leaves.
const HEADER_INSET: f32 = 6.0;

/// A list's column headers as the library mock-up's list header draws them:
/// cells edge to edge on the face colour, each inside the skin's raised
/// outline and sunk while pressed, labels left-aligned and truncated with an
/// ellipsis. Each cell starts where its column's cells start and reaches the
/// next column, so the gap the rows keep between two columns belongs to the
/// cell before it; the last one runs on over the scroll bar's room to the
/// list's edge. A click sorts by a column, and on the sorted column reverses
/// it. The `plain` columns are headers only: their list cannot sort by them,
/// so they look the same but take no click.
pub fn sort_header<const N: usize>(
    ui: &mut egui::Ui,
    labels: &[&str; N],
    widths: &[f32; N],
    plain: &[usize],
    classic: bool,
    sort: &mut usize,
    descending: &mut bool,
) -> bool {
    // The rows' gap between columns, taken before the header drops it.
    let gap = ui.spacing().item_spacing.x;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (column, label) in labels.iter().enumerate() {
            let sortable = !plain.contains(&column);
            let width = if column + 1 < N {
                widths[column] + gap
            } else {
                ui.available_width().max(widths[column] + SCROLL_ROOM)
            };
            let sense = if sortable {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let (rect, response) = ui.allocate_exact_size(egui::vec2(width, HEADER_HEIGHT), sense);
            let text = if sortable && *sort == column {
                format!("{label} {}", if *descending { "▼" } else { "▲" })
            } else {
                (*label).to_owned()
            };
            let role = if sortable {
                egui::Role::Button
            } else {
                egui::Role::Label
            };
            response.widget_info(|| egui::WidgetInfo::labeled(role, ui.is_enabled(), &text));
            // A plain header keeps a resting header's text colour, so it
            // reads like the others.
            let ink = if sortable {
                ui.style().interact(&response).text_color()
            } else {
                ui.visuals().widgets.inactive.text_color()
            };
            let truncated = header_face(ui, rect, &text, ink);
            if classic && response.is_pointer_button_down_on() {
                pane::inset(ui, rect, true);
            } else if classic || column == 0 {
                pane::outset(ui, rect, classic);
            } else {
                // The mock-up draws the modern line outside each cell, so
                // neighbours share one line: this cell's starts on the last
                // pixel of the cell before it.
                pane::outset(ui, rect.with_min_x(rect.left() - 1.0), classic);
            }
            let response = if truncated {
                response.on_hover_text(&text)
            } else {
                response
            };
            if response.clicked() {
                if *sort == column {
                    *descending = !*descending;
                } else {
                    *sort = column;
                    *descending = false;
                }
                changed = true;
            }
        }
    });
    changed
}

/// A header cell's ground in the face colour and its label in the mock-up's
/// padding, left-aligned and truncated with an ellipsis to the cell. Returns
/// whether the label was truncated.
fn header_face(ui: &egui::Ui, rect: egui::Rect, text: &str, ink: egui::Color32) -> bool {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(egui::TextStyle::Button.resolve(ui.style()), ink),
    );
    job.wrap =
        egui::text::TextWrapping::truncate_at_width((rect.width() - 2.0 * HEADER_INSET).max(0.0));
    let galley = ui.painter().layout_job(job);
    let truncated = galley.elided;
    painter.galley(
        egui::pos2(
            rect.left() + HEADER_INSET,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        ink,
    );
    truncated
}

/// Room a list keeps beside its columns for the vertical scroll bar.
const SCROLL_ROOM: f32 = 12.0;
/// The sort mark a header adds after its label.
const SORT_MARK: &str = " ▼";

/// How a list cell draws its text, as the mock-ups' tables do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Text,
    /// Digests, tags and ids in the code style.
    Mono,
    /// Counts and sizes, right-aligned.
    Number,
    /// The open or user-supplied chip.
    Badge,
}

/// A list's columns: headers, designed widths, how cells draw, and a value
/// each column always shows whole (a size, a count, a mark).
pub struct Columns<const N: usize> {
    pub labels: [&'static str; N],
    pub widths: [f32; N],
    pub cells: [Cell; N],
    pub samples: [&'static str; N],
}

fn cell_font(ui: &egui::Ui, cell: Cell) -> egui::FontId {
    match cell {
        Cell::Mono => egui::TextStyle::Monospace.resolve(ui.style()),
        _ => egui::TextStyle::Body.resolve(ui.style()),
    }
}

/// Room a badge adds around its text.
pub const BADGE_PADDING: f32 = 10.0;

/// The narrowest each column may get: its header with the sort mark in the
/// header cell's padding, and its sample value whole.
fn column_floors<const N: usize>(ui: &egui::Ui, columns: &Columns<N>) -> [f32; N] {
    let width = |text: &str, font: egui::FontId| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE)
            .size()
            .x
    };
    let button = egui::TextStyle::Button.resolve(ui.style());
    let padding = 2.0 * HEADER_INSET;
    std::array::from_fn(|i| {
        let header = width(&format!("{}{SORT_MARK}", columns.labels[i]), button.clone()) + padding;
        let sample = width(columns.samples[i], cell_font(ui, columns.cells[i]))
            + if columns.cells[i] == Cell::Badge {
                BADGE_PADDING
            } else {
                4.0
            };
        header.max(sample).ceil()
    })
}

/// A list's column widths for the room the list has. Each column gets at
/// least its floor, so headers, counts, sizes and marks never truncate; the
/// room left is shared out by the designed widths, so the table fills its
/// area as the mock-ups' tables do. With less room than the floors take,
/// the list keeps them and scrolls sideways in its own area.
pub fn fit_columns<const N: usize>(ui: &egui::Ui, columns: &Columns<N>) -> [f32; N] {
    let floors = column_floors(ui, columns);
    let gaps = ui.spacing().item_spacing.x * (N.saturating_sub(1)) as f32;
    let room = ui.available_width() - gaps - SCROLL_ROOM;
    let needed: f32 = floors.iter().sum();
    if room <= needed {
        return floors;
    }
    let designed: f32 = columns.widths.iter().sum();
    std::array::from_fn(|i| (floors[i] + (room - needed) * columns.widths[i] / designed).floor())
}

/// The width a row of `widths` takes, with the gaps between its cells.
pub fn table_width<const N: usize>(ui: &egui::Ui, widths: &[f32; N]) -> f32 {
    widths.iter().sum::<f32>() + ui.spacing().item_spacing.x * (N.saturating_sub(1)) as f32
}

/// A repository's group row, as wide as the table: the machines mock-up's
/// bold title with the repository and its image count after it. Returns
/// whether it was clicked, which opens or closes the group.
pub fn group_row(ui: &mut egui::Ui, open: bool, title: &str, detail: &str, width: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), title));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        // The mock-up's group row has its own ground, apart from the stripes.
        painter.rect_filled(rect, 0.0, pane::group_colour(ui.visuals()));
        let font = egui::TextStyle::Body.resolve(ui.style());
        let ink = ui.visuals().strong_text_color();
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &format!("{} {title}", if open { "−" } else { "+" }),
            0.0,
            egui::TextFormat::simple(font.clone(), ink),
        );
        job.append(
            &format!("  {detail}"),
            0.0,
            egui::TextFormat::simple(font, ui.visuals().weak_text_color()),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 8.0).max(0.0));
        let galley = ui.painter().layout_job(job);
        painter.galley(
            egui::pos2(rect.left() + 4.0, rect.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
    }
    response.clicked()
}
pub fn draw_cells<const N: usize>(
    ui: &mut egui::Ui,
    cells: &[String; N],
    widths: &[f32; N],
    kinds: &[Cell; N],
    selected: bool,
    striped: bool,
    lead: Lead,
) -> (bool, bool) {
    draw_cells_with_hints(ui, cells, widths, kinds, selected, striped, lead, &[])
}
/// [`draw_cells`] where a cell's tooltip may say more than its text, as the
/// quiet "↑ newer BIOS" badge's hint does.
#[allow(clippy::too_many_arguments)]
pub fn draw_cells_with_hints<const N: usize>(
    ui: &mut egui::Ui,
    cells: &[String; N],
    widths: &[f32; N],
    kinds: &[Cell; N],
    selected: bool,
    striped: bool,
    lead: Lead,
    hints: &[Option<String>],
) -> (bool, bool) {
    let mut clicked = false;
    let mut double = false;
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else if striped {
        ui.visuals().faint_bg_color
    } else {
        egui::Color32::TRANSPARENT
    };
    // On the selection fill, text takes the selection stroke colour, as
    // egui's own selectable items do; every skin keeps that pair legible.
    let selected_text = selected.then(|| ui.visuals().selection.stroke.color);
    egui::Frame::new().fill(fill).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (column, (text, width)) in cells.iter().zip(widths.iter().copied()).enumerate() {
                let response = if column == 0 {
                    name_cell(ui, text, width, selected_text, lead)
                } else {
                    cell(ui, text, width, kinds[column], selected_text)
                };
                let hint = hints.get(column).and_then(Option::as_deref).unwrap_or(text);
                let response = response.on_hover_text(hint);
                clicked |= response.clicked();
                double |= response.double_clicked();
            }
        });
    });
    (clicked, double)
}

/// One list cell, painted left-aligned, numbers right-aligned, digests in
/// the code style and marks as the mock-ups' chip, truncated to the column.
fn cell(
    ui: &mut egui::Ui,
    text: &str,
    width: f32,
    kind: Cell,
    selected_text: Option<egui::Color32>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), text));
    if text.is_empty() || !ui.is_rect_visible(rect) {
        return response;
    }
    let ink = selected_text.unwrap_or_else(|| match kind {
        Cell::Badge if text == "open" => pane::good_colour(ui.visuals()),
        Cell::Badge => ui.visuals().warn_fg_color,
        _ => ui.style().interact(&response).text_color(),
    });
    let room = rect.shrink2(egui::vec2(2.0, 0.0));
    let inner = if kind == Cell::Badge {
        room.width() - BADGE_PADDING
    } else {
        room.width()
    };
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(cell_font(ui, kind), ink),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(inner.max(0.0));
    let galley = ui.painter().layout_job(job);
    let size = galley.size();
    let left = match kind {
        Cell::Number => room.right() - size.x,
        Cell::Badge => room.left() + BADGE_PADDING / 2.0,
        _ => room.left(),
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let at = egui::pos2(left, rect.center().y - size.y / 2.0);
    if kind == Cell::Badge {
        painter.rect_stroke(
            egui::Rect::from_min_size(at, size).expand2(egui::vec2(BADGE_PADDING / 2.0 - 1.0, 0.0)),
            0.0,
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    painter.galley(at, galley, ink);
    response
}

/// The name cell as the machines mock-up draws it: left-aligned, the row's
/// icon (after a group's indent), the name, then its badges, truncated
/// together to the column. It is painted rather than a centred label, because
/// a centred egui label drops the space the icon and the indent need.
fn name_cell(
    ui: &mut egui::Ui,
    text: &str,
    width: f32,
    selected_text: Option<egui::Color32>,
    lead: Lead,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), text));
    // The colour an interactive label's text takes; the icon shares it.
    let ink = selected_text.unwrap_or_else(|| ui.style().interact(&response).text_color());
    let indent = if lead.child { CHILD_INDENT } else { 0.0 };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let mut left = rect.left() + indent;
    if let Some(icon) = lead.icon {
        let at = egui::Rect::from_min_size(
            egui::pos2(left + 2.0, rect.center().y - 8.0),
            egui::Vec2::splat(16.0),
        );
        ui_kit::shell_icons::paint(&painter, at, icon, ink, ui.visuals());
        left += NAME_ICON_ROOM;
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat::simple(font.clone(), ink));
    for (on, badge, color) in [
        (lead.warning, WARNING_BADGE, ui.visuals().warn_fg_color),
        (
            lead.favourite,
            FAVOURITE_BADGE,
            ui_kit::shell_icons::STAR_GOLD,
        ),
    ] {
        if on {
            job.append(
                badge,
                0.0,
                egui::TextFormat::simple(font.clone(), selected_text.unwrap_or(color)),
            );
        }
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - left).max(0.0));
    let galley = ui.painter().layout_job(job);
    painter.galley(
        egui::pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    response
}

/// The layout figures the desktop's tests of these tables compare with.
#[cfg(feature = "test-support")]
pub mod test_support {
    /// The set-in of a group's child rows.
    pub const CHILD_INDENT: f32 = super::CHILD_INDENT;
    /// The room the icon takes before a row's name.
    pub const NAME_ICON_ROOM: f32 = super::NAME_ICON_ROOM;
    /// Room a list keeps beside its columns for the vertical scroll bar.
    pub const SCROLL_ROOM: f32 = super::SCROLL_ROOM;
}
