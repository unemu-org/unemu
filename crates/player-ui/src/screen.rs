// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player's screen: the machine's picture in the application's central
//! panel, in the shape the machine states (core-contract.md §13.3, "The
//! picture" and "The absolute pointer").
//!
//! [`machine_view::screen::layout`] does the geometry: overscan hidden by
//! default (View › Show overscan shows it), integer scaling as View chooses.
//! The panel is black around the picture, the picture is drawn with nearest
//! sampling, and a paused machine keeps its last picture under PAUSED, as
//! every machine's screen does. For a machine with an absolute pointer the
//! host pointer over the picture aims it, with a cross-hair cursor; for one
//! with a relative pointer a captured pointer is hidden.

use crate::Player;
use machine_view::screen::{layout, Rect};
use machine_view::Picture;

/// Whether the picture shows less than all of itself: its visible part is a
/// proper part of it. A visible part outside the picture states nothing, as
/// the geometry takes it.
pub(crate) fn cropped(picture: &Picture) -> bool {
    let visible = picture.visible;
    visible.width > 0
        && visible.height > 0
        && visible.is_within(picture.width, picture.height)
        && (visible.width < picture.width || visible.height < picture.height)
}

pub(crate) fn to_egui(rect: Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(rect.x, rect.y),
        egui::vec2(rect.width, rect.height),
    )
}

fn from_egui(rect: egui::Rect) -> Rect {
    Rect {
        x: rect.left(),
        y: rect.top(),
        width: rect.width(),
        height: rect.height(),
    }
}

/// The picture as an egui image: `0x00RRGGBB` pixels, rows top to bottom.
fn image(picture: &Picture) -> egui::ColorImage {
    let mut rgb = Vec::with_capacity(picture.pixels.len() * 3);
    for pixel in &picture.pixels {
        rgb.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]);
    }
    egui::ColorImage::from_rgb([picture.width as usize, picture.height as usize], &rgb)
}

impl Player {
    /// Draw the screen into the rest of `ui`, the application's central
    /// panel. The response senses clicks on the picture, so the application
    /// can give the machine the keyboard.
    pub fn screen(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let panel = ui.available_rect_before_wrap();
        ui.painter().rect_filled(panel, 0.0, egui::Color32::BLACK);
        let picture = self.session.as_ref().and_then(|session| session.picture());
        self.layout = picture.and_then(|picture| {
            layout(
                picture,
                from_egui(panel),
                self.show_overscan,
                self.integer_scale,
            )
        });
        if let Some(picture) = picture {
            let fresh = self.texture.as_ref().is_none_or(|(texture, serial)| {
                *serial != picture.serial
                    || texture.size() != [picture.width as usize, picture.height as usize]
            });
            if fresh {
                let image = image(picture);
                match &mut self.texture {
                    Some((texture, serial)) => {
                        texture.set(image, egui::TextureOptions::NEAREST);
                        *serial = picture.serial;
                    }
                    None => {
                        let texture = ui.ctx().load_texture(
                            "player-screen",
                            image,
                            egui::TextureOptions::NEAREST,
                        );
                        self.texture = Some((texture, picture.serial));
                    }
                }
            }
        }
        let Some(shown) = self.layout else {
            return ui.interact(
                panel,
                ui.scope_id().with("player-screen"),
                egui::Sense::click(),
            );
        };
        let rect = to_egui(shown.dest);
        if let (Some((texture, _)), Some(picture)) = (&self.texture, picture) {
            let (width, height) = (picture.width as f32, picture.height as f32);
            let source = shown.source;
            let uv = egui::Rect::from_min_max(
                egui::pos2(source.x as f32 / width, source.y as f32 / height),
                egui::pos2(
                    (source.x + source.width) as f32 / width,
                    (source.y + source.height) as f32 / height,
                ),
            );
            ui.painter()
                .image(texture.id(), rect, uv, egui::Color32::WHITE);
        }
        if self.paused {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "PAUSED",
                egui::FontId::proportional(20.0),
                egui::Color32::WHITE,
            );
        }
        let response = ui.interact(
            rect,
            ui.scope_id().with("player-screen"),
            egui::Sense::click(),
        );
        let blocked = self.dialog.is_some() || egui::Popup::is_any_open(ui.ctx());
        let hover = ui.input(|input| input.pointer.hover_pos());
        if let Some(session) = &mut self.session {
            if session.aims() {
                if !self.paused && !blocked {
                    let place = hover.and_then(|at| shown.pointer_position(at.x, at.y));
                    session.pointer_at(place);
                }
                if hover.is_some_and(|at| rect.contains(at)) && !blocked {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            } else if session.mouse.captured() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            }
        }
        response
    }
}
