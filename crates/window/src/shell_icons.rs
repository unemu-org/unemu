// SPDX-License-Identifier: AGPL-3.0-or-later
//! Font-independent desktop toolbar marks; see machine-images.md §7.

use egui::{Align2, Color32, Rect, Response, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Play,
    Pause,
    Stop,
    Reset,
    Floppy,
    Eject,
    Settings,
    Layers,
}

pub(super) fn button(ui: &mut Ui, label: &str, icon: Icon) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), font.clone(), Color32::WHITE);
    let response = ui.add_sized([galley.size().x + 34.0, 26.0], egui::Button::new(""));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let color = ui.style().interact(&response).fg_stroke.color;
    let rect = Rect::from_min_size(
        response.rect.left_center() + Vec2::new(5.0, -8.0),
        Vec2::splat(16.0),
    );
    paint(ui, rect, icon, color);
    ui.painter().text(
        response.rect.left_center() + Vec2::new(26.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
    response
}

fn paint(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.5, color);
    let point = |x: f32, y: f32| rect.min + Vec2::new(x, y);
    let triangle = || {
        Shape::convex_polygon(
            vec![point(3.0, 2.0), point(14.0, 8.0), point(3.0, 14.0)],
            color,
            Stroke::NONE,
        )
    };
    match icon {
        Icon::Play => {
            painter.add(triangle());
        }
        Icon::Pause => {
            for x in [3.0, 10.0] {
                painter.rect_filled(
                    Rect::from_min_size(point(x, 2.0), Vec2::new(3.0, 12.0)),
                    0.0,
                    color,
                );
            }
        }
        Icon::Stop => {
            painter.rect_filled(
                Rect::from_min_max(point(3.0, 3.0), point(13.0, 13.0)),
                0.0,
                color,
            );
        }
        Icon::Reset => {
            painter.circle_stroke(point(8.0, 8.0), 5.5, stroke);
            painter.line_segment([point(8.0, 0.0), point(8.0, 7.0)], stroke);
        }
        Icon::Floppy => {
            for (left, top, right, bottom) in [
                (1.0, 1.0, 15.0, 15.0),
                (4.0, 1.0, 12.0, 6.0),
                (4.0, 9.0, 12.0, 15.0),
            ] {
                painter.rect_stroke(
                    Rect::from_min_max(point(left, top), point(right, bottom)),
                    0.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Eject => {
            painter.add(Shape::convex_polygon(
                vec![point(2.0, 10.0), point(8.0, 2.0), point(14.0, 10.0)],
                color,
                Stroke::NONE,
            ));
            painter.rect_filled(
                Rect::from_min_max(point(2.0, 12.0), point(14.0, 15.0)),
                0.0,
                color,
            );
        }
        Icon::Settings => {
            painter.circle_stroke(point(8.0, 8.0), 4.0, stroke);
            painter.circle_stroke(point(8.0, 8.0), 1.5, stroke);
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::TAU / 8.0;
                let direction = Vec2::angled(angle);
                painter.line_segment(
                    [
                        point(8.0, 8.0) + direction * 4.0,
                        point(8.0, 8.0) + direction * 7.0,
                    ],
                    stroke,
                );
            }
        }
        Icon::Layers => {
            for y in [4.0, 8.0, 12.0] {
                painter.add(Shape::line(
                    vec![
                        point(1.0, y),
                        point(8.0, y - 3.0),
                        point(15.0, y),
                        point(8.0, y + 3.0),
                        point(1.0, y),
                    ],
                    stroke,
                ));
            }
        }
    }
}

pub(super) fn custom_title_bar(classic: bool, macos: bool) -> bool {
    classic && !macos
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_macos_title_replaces_only_the_custom_title_strip() {
        assert!(!custom_title_bar(true, true));
        assert!(custom_title_bar(true, false));
        assert!(!custom_title_bar(false, false));
    }

    #[test]
    fn shell_strings_have_configured_font_coverage() {
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
        let output = ctx.run_pass(Default::default(), |ctx| {
            ctx.fonts_mut(|fonts| {
                for source in [include_str!("shell.rs"), include_str!("shell_model.rs")] {
                    let source = source.split("#[cfg(test)]").next().unwrap();
                    let mut literal = false;
                    let mut escaped = false;
                    for ch in source.chars() {
                        if escaped {
                            escaped = false;
                            continue;
                        }
                        if ch == '\\' && literal {
                            escaped = true;
                            continue;
                        }
                        if ch == '"' {
                            literal = !literal;
                            continue;
                        }
                        if literal && !ch.is_ascii() {
                            assert!(
                                fonts.has_glyph(&egui::FontId::proportional(14.0), ch),
                                "missing UI glyph U+{:04X}",
                                ch as u32
                            );
                        }
                    }
                }
            });
        });
        output.drop_without_applying_deltas();
    }
}
