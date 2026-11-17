// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's two skins and the small pieces every window draws with them;
//! see `backlog/docs/design/spec.md` §9.4 and
//! `backlog/docs/design/machine-images.md` §7.
//!
//! The classic and the modern skin follow the approved mock-ups in
//! `backlog/docs/design/mockups/`, each in a light and a dark theme:
//! [`skin_visuals`] gives their egui visuals, [`configure_context`] the fonts,
//! [`gradient_title`] and [`window_title`] the classic title strips, and
//! [`bevel`] and [`bevel_ring`] the classic bevels. [`escape_pressed`] lets one
//! Escape close one dialog, and [`audio_mark`] and [`EJECT_MARK`] are the
//! mock-ups' text marks for sound and eject.

use egui::{Color32, Sense, Stroke, Vec2};

/// One Escape closes one dialog: the first reader consumes the key, so a
/// dialog stacked beneath cannot close on the same press.
pub fn escape_pressed(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
}

/// Installs the desktop's fonts in `ctx` and keeps egui's keyboard zoom off.
pub fn configure_context(ctx: &egui::Context) {
    // The shortcut symbols need glyphs beyond the proportional Latin font.
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("Hack".into());
    // Last in both families, so it only supplies the marks no egui font has
    // (✓ ✗ ✕ Ⅱ ⌫); without it they would render as empty boxes.
    fonts.font_data.insert(
        "DejaVu-symbols".into(),
        std::sync::Arc::new(egui::FontData::from_static(crate::SYMBOL_FONT)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("DejaVu-symbols".into());
    }
    ctx.set_fonts(fonts);
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
}

/// The status bar's speaker, as the main-window mock-up marks its audio cell:
/// crossed out while no sound reaches the speakers of a running machine.
pub fn audio_mark(audible: bool) -> &'static str {
    if audible {
        "🔊"
    } else {
        "🔇"
    }
}

/// The eject mark of the Media menu and the drive menus.
pub const EJECT_MARK: &str = "⏏";

/// The visuals of one skin in one theme. Selected rows and items draw their
/// text in the selection stroke colour on the selection fill, so every skin
/// keeps that pair legible.
///
/// Both skins take the mock-ups' face colour for panels and dialogs, their
/// field colour for text fields, mini tables and scroll tracks, and their
/// stripe colour for striped rows: egui's dark field is near black and reads
/// as a hole in the pane, and its stripe is all but invisible. Warnings and
/// errors take the mock-ups' warn and bad colours: egui's orange and red
/// fade on the light face. In the mock-ups the modern skin shares every
/// colour with the classic one and changes only the corners and the
/// outlines; see `modern_colours`.
pub fn skin_visuals(classic: bool, dark: bool) -> egui::Visuals {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    (visuals.extreme_bg_color, visuals.faint_bg_color) = if dark {
        (Color32::from_rgb(38, 40, 43), Color32::from_rgb(44, 46, 50))
    } else {
        (Color32::WHITE, Color32::from_rgb(243, 241, 234))
    };
    (visuals.warn_fg_color, visuals.error_fg_color) = if dark {
        (
            Color32::from_rgb(255, 207, 115),
            Color32::from_rgb(255, 157, 146),
        )
    } else {
        (
            Color32::from_rgb(138, 83, 0),
            Color32::from_rgb(155, 28, 28),
        )
    };
    let face = if dark {
        Color32::from_rgb(52, 54, 58)
    } else {
        Color32::from_rgb(214, 211, 203)
    };
    visuals.panel_fill = face;
    visuals.window_fill = face;
    if classic {
        visuals.window_corner_radius = egui::CornerRadius::ZERO;
        visuals.menu_corner_radius = egui::CornerRadius::ZERO;
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
            &mut visuals.widgets.noninteractive,
        ] {
            widget.corner_radius = egui::CornerRadius::ZERO;
        }
        visuals.widgets.inactive.bg_fill = face;
        visuals.widgets.inactive.weak_bg_fill = face;
        visuals.selection.bg_fill = Color32::from_rgb(34, 70, 138);
        // egui draws selected text in the selection stroke colour; the
        // light theme's dark blue would vanish on the classic navy fill.
        visuals.selection.stroke.color = Color32::WHITE;
    } else {
        modern_colours(&mut visuals, face, dark);
    }
    visuals
}

/// The modern skin's colours from the mock-ups' tokens, on egui's rounded
/// corners. Its flat 1 px outlines take the mock-ups' shadow colour (`lo`):
/// around dialogs and menus, along separators, and around resting buttons,
/// fields and check boxes, as the mock-ups outline every control in the
/// modern skin. egui's own grey all but vanishes on the face colour, and a
/// resting check box would be hard to find on it without its outline.
///
/// Buttons rest on the face colour, as the mock-ups' `.btn` does, and turn
/// to the second face colour (`face-2`) when hovered or while their menu is
/// open, as the mock-ups' chips do; pressed ones stay on the face. The boxes
/// of check boxes and radio buttons and the rails of sliders take the second
/// face colour at rest, so they stand apart from the face, and hovered ones
/// go halfway from it to the highlight (`hi`). Text takes the ink colour and
/// weak text the muted one. Strong text, egui's stand-in for the mock-ups'
/// bold, as no bold face is loaded, and the text of hovered and pressed
/// widgets go one step past the ink, to white or black. Selected rows take
/// the selection colour with white text, links the link colour.
fn modern_colours(visuals: &mut egui::Visuals, face: Color32, dark: bool) {
    let rgb = Color32::from_rgb;
    let (face_2, highlight, outline, ink, strong, muted, selection, link) = if dark {
        (
            rgb(62, 64, 69),
            rgb(92, 95, 102),
            rgb(31, 32, 35),
            rgb(230, 228, 223),
            Color32::WHITE,
            rgb(169, 166, 160),
            rgb(60, 100, 176),
            rgb(169, 198, 255),
        )
    } else {
        (
            rgb(231, 228, 220),
            Color32::WHITE,
            rgb(141, 138, 131),
            rgb(29, 28, 26),
            Color32::BLACK,
            rgb(91, 88, 82),
            rgb(34, 70, 138),
            rgb(27, 74, 150),
        )
    };
    let hovered_box = face_2.lerp_to_gamma(highlight, 0.5);
    visuals.window_stroke.color = outline;
    visuals.weak_text_color = Some(muted);
    visuals.hyperlink_color = link;
    visuals.selection.bg_fill = selection;
    visuals.selection.stroke.color = Color32::WHITE;
    let widgets = &mut visuals.widgets;
    // Per state: the ground of buttons (`weak_bg_fill`), the box of check
    // boxes and radio buttons and the rail of sliders (`bg_fill`), the text.
    for (widget, button, boxes, text) in [
        (&mut widgets.noninteractive, face, face, ink),
        (&mut widgets.inactive, face, face_2, ink),
        (&mut widgets.hovered, face_2, hovered_box, strong),
        (&mut widgets.active, face, face, strong),
        (&mut widgets.open, face_2, face_2, ink),
    ] {
        widget.weak_bg_fill = button;
        widget.bg_fill = boxes;
        widget.fg_stroke.color = text;
    }
    widgets.noninteractive.bg_stroke.color = outline;
    widgets.inactive.bg_stroke = Stroke::new(1.0, outline);
}

/// A classic 1 px bevel just inside `rect`: raised, or sunken when `pressed`,
/// in the light or dark theme's highlight and shadow.
pub fn bevel(ui: &egui::Ui, rect: egui::Rect, pressed: bool) {
    let light = if ui.visuals().dark_mode {
        Color32::from_gray(92)
    } else {
        Color32::WHITE
    };
    let shadow = if ui.visuals().dark_mode {
        Color32::from_gray(24)
    } else {
        Color32::from_rgb(141, 138, 131)
    };
    let (top, bottom) = if pressed {
        (shadow, light)
    } else {
        (light, shadow)
    };
    bevel_ring(ui.painter(), rect, top, bottom);
}

/// One 1 px ring of a classic bevel just inside `rect`: `top` along its top
/// and left edges, `bottom` along its right and bottom ones.
pub fn bevel_ring(painter: &egui::Painter, rect: egui::Rect, top: Color32, bottom: Color32) {
    let rect = rect.shrink(0.5);
    painter.line_segment([rect.left_bottom(), rect.left_top()], Stroke::new(1.0, top));
    painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, top));
    painter.line_segment(
        [rect.right_top(), rect.right_bottom()],
        Stroke::new(1.0, bottom),
    );
    painter.line_segment(
        [rect.right_bottom(), rect.left_bottom()],
        Stroke::new(1.0, bottom),
    );
}

/// A dialog's classic title strip: the title on the navy gradient.
pub fn gradient_title(ui: &mut egui::Ui, title: &str) {
    title_strip(ui, title, false);
}

/// The main window's classic title strip: the application icon, as the
/// main-window mock-up's title bar shows it, then the title.
pub fn window_title(ui: &mut egui::Ui, title: &str) {
    title_strip(ui, title, true);
}

fn title_strip(ui: &mut egui::Ui, title: &str, icon: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 23.0), Sense::hover());
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), Color32::from_rgb(19, 51, 111)),
        (rect.right_top(), Color32::from_rgb(75, 121, 193)),
        (rect.right_bottom(), Color32::from_rgb(75, 121, 193)),
        (rect.left_bottom(), Color32::from_rgb(19, 51, 111)),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(mesh);
    let mut text_left = 7.0;
    if icon {
        let icon_rect = egui::Rect::from_center_size(
            rect.left_center() + Vec2::new(14.0, 0.0),
            Vec2::splat(16.0),
        );
        crate::shell_icons::paint(
            ui.painter(),
            icon_rect,
            crate::shell_icons::Icon::App,
            Color32::WHITE,
            ui.visuals(),
        );
        text_left = 28.0;
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(text_left, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, Color32::WHITE),
    );
}
