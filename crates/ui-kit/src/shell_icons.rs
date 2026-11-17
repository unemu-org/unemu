// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's icons, painted from the approved mock-ups' own artwork.
//!
//! The mock-ups in `backlog/docs/design/mockups/` draw their icons as inline
//! SVG in a 16-unit view box (48 for the empty-library picture). Each one is
//! painted here with egui shapes from the same coordinates, so no image
//! decoder or icon font enters the build and the icons follow the skin and
//! theme: the SVGs' `currentColor` parts take the widget's text colour, their
//! fixed fills (the green Start, the red Stop, the floppy, the blue top layer,
//! the application screen) stay fixed, and the empty-library picture takes the
//! panel and selection colours the mock-up names. Marks the mock-ups write as
//! Unicode text (★ ⚠ ✓ ▶ Ⅱ ■ ⏏ 🔊 …) stay text; the fonts that
//! `crate::skin::configure_context` installs cover every one of them. ⓘ is
//! painted ([`Icon::Info`]) because no bundled font has it.
//!
//! The `inventory`, built for tests only, lists every icon of every approved mock-up with
//! the place the app draws it, or the reason it does not; see machine-images.md
//! §7 and `backlog/docs/design/icons.md`.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Response, Shape, Stroke, Ui, Vec2};

#[cfg(any(test, feature = "test-support"))]
pub mod inventory;

/// An icon the mock-ups draw as SVG artwork (and ⓘ, which no font has).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// The application: a blue screen on a light monitor (title bar).
    App,
    /// Start, and Resume on the toolbar: a green triangle.
    Play,
    Pause,
    /// Stop: a red square.
    Stop,
    /// A: on the toolbar: a dark 3½-inch floppy.
    Floppy,
    Eject,
    Settings,
    /// Disk layers: three stacked sheets, the top one blue.
    Layers,
    /// A machine: a monitor outline on a stand.
    Machine,
    /// A machine image: three stacked outlines.
    Image,
    /// An installation media set: a floppy outline.
    Media,
    /// A firmware package: a chip with pins.
    Firmware,
    /// A machine file: a page with a folded corner.
    MachineFile,
    /// A note banner's ⓘ.
    Info,
    /// A question dialog's mark: a blue disc with "?".
    Question,
    /// A refusal dialog's mark: a red disc with "✕".
    Refusal,
    /// The empty library's picture: a monitor over a keyboard.
    EmptyLibrary,
    /// Refresh a registry listing: a circular arrow.
    Refresh,
    /// Pull from a registry: an arrow pointing down.
    Pull,
    /// A registry: two stacked server units.
    Registry,
    /// A registry namespace: a folder outline.
    Namespace,
    /// A private namespace before login: a padlock.
    Lock,
    /// The Remotes list's picture of a registry that cannot be reached or
    /// answered with an error: a crossed circle.
    Unreachable,
    /// The Remotes list's picture of a registry that asks for a login: a
    /// large padlock.
    LoginRequired,
    /// The Remotes list's picture of a registry with nothing published: an
    /// empty folder.
    EmptyRegistry,
    /// The Remotes list's picture of a listing in progress: a clock.
    Listing,
}

impl Icon {
    /// Every icon, in declaration order.
    #[cfg(test)]
    pub const ALL: [Icon; 26] = [
        Icon::App,
        Icon::Play,
        Icon::Pause,
        Icon::Stop,
        Icon::Floppy,
        Icon::Eject,
        Icon::Settings,
        Icon::Layers,
        Icon::Machine,
        Icon::Image,
        Icon::Media,
        Icon::Firmware,
        Icon::MachineFile,
        Icon::Info,
        Icon::Question,
        Icon::Refusal,
        Icon::EmptyLibrary,
        Icon::Refresh,
        Icon::Pull,
        Icon::Registry,
        Icon::Namespace,
        Icon::Lock,
        Icon::Unreachable,
        Icon::LoginRequired,
        Icon::EmptyRegistry,
        Icon::Listing,
    ];

    /// The side of the square the icon is drawn in, in points.
    pub fn size(self) -> f32 {
        match self {
            Icon::Question | Icon::Refusal => 32.0,
            Icon::Unreachable | Icon::LoginRequired | Icon::EmptyRegistry | Icon::Listing => 40.0,
            Icon::EmptyLibrary => 48.0,
            _ => 16.0,
        }
    }

    /// The side of the mock-up's SVG view box.
    fn view_box(self) -> f32 {
        match self {
            Icon::EmptyLibrary => 48.0,
            Icon::Unreachable | Icon::LoginRequired | Icon::EmptyRegistry | Icon::Listing => 40.0,
            _ => 16.0,
        }
    }
}

const START_GREEN: Color32 = Color32::from_rgb(0x2c, 0x8a, 0x3a);
const STOP_RED: Color32 = Color32::from_rgb(0xb2, 0x3a, 0x2b);
const LAYER_BLUE: Color32 = Color32::from_rgb(0x4b, 0x79, 0xc1);
const MONITOR_FACE: Color32 = Color32::from_rgb(0xc9, 0xd6, 0xe8);
const MONITOR_EDGE: Color32 = Color32::from_rgb(0x0b, 0x1a, 0x33);
const MONITOR_SCREEN: Color32 = Color32::from_rgb(0x0b, 0x2a, 0x5a);
const FLOPPY_BODY: Color32 = Color32::from_rgb(0x3a, 0x3f, 0x4a);
const FLOPPY_SHUTTER: Color32 = Color32::from_rgb(0xc9, 0xcd, 0xd4);
const FLOPPY_LABEL: Color32 = Color32::from_rgb(0xe9, 0xe6, 0xdc);
/// The favourite star's gold, as the machines mock-up colours it.
pub const STAR_GOLD: Color32 = Color32::from_rgb(0xc0, 0x8a, 0x00);

/// Coordinates in the mock-up's view box, scaled into the icon's rect.
struct Pen<'a> {
    painter: &'a Painter,
    origin: Pos2,
    unit: f32,
}

impl Pen<'_> {
    fn at(&self, (x, y): (f32, f32)) -> Pos2 {
        self.origin + Vec2::new(x, y) * self.unit
    }
    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.at((x, y)), Vec2::new(w, h) * self.unit)
    }
    fn stroke(&self, width: f32, color: Color32) -> Stroke {
        Stroke::new(width * self.unit, color)
    }
    fn fill(&self, (x, y, w, h): (f32, f32, f32, f32), color: Color32) {
        self.painter.rect_filled(self.rect(x, y, w, h), 0.0, color);
    }
    /// An SVG rect's outline: a one-unit stroke centred on the edge.
    fn frame(&self, (x, y, w, h): (f32, f32, f32, f32), color: Color32) {
        self.painter.rect_stroke(
            self.rect(x, y, w, h),
            0.0,
            self.stroke(1.0, color),
            egui::StrokeKind::Middle,
        );
    }
    fn line(&self, points: &[(f32, f32)], width: f32, color: Color32) {
        let points = points.iter().map(|&p| self.at(p)).collect();
        self.painter
            .add(Shape::line(points, self.stroke(width, color)));
    }
    fn closed(&self, points: &[(f32, f32)], color: Color32) {
        self.closed_width(points, 1.0, color);
    }
    fn closed_width(&self, points: &[(f32, f32)], width: f32, color: Color32) {
        let points = points.iter().map(|&p| self.at(p)).collect();
        self.painter
            .add(Shape::closed_line(points, self.stroke(width, color)));
    }
    /// An SVG rect's outline with a `width`-unit stroke centred on the edge.
    fn frame_width(&self, (x, y, w, h): (f32, f32, f32, f32), width: f32, color: Color32) {
        self.painter.rect_stroke(
            self.rect(x, y, w, h),
            0.0,
            self.stroke(width, color),
            egui::StrokeKind::Middle,
        );
    }
    fn circle(&self, center: (f32, f32), radius: f32, width: f32, color: Color32) {
        self.painter.circle_stroke(
            self.at(center),
            radius * self.unit,
            self.stroke(width, color),
        );
    }
    /// The points of an SVG arc around `center`, from `from` to `to` degrees
    /// measured as SVG measures them (y down, so increasing angles turn
    /// clockwise on screen), one point every 15 degrees at most.
    fn arc_points(center: (f32, f32), radius: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
        let steps = ((to - from).abs() / 15.0).ceil().max(1.0) as usize;
        (0..=steps)
            .map(|i| {
                let angle = (from + (to - from) * i as f32 / steps as f32).to_radians();
                (
                    center.0 + radius * angle.cos(),
                    center.1 + radius * angle.sin(),
                )
            })
            .collect()
    }
    fn polygon(&self, points: &[(f32, f32)], color: Color32) {
        let points = points.iter().map(|&p| self.at(p)).collect();
        self.painter
            .add(Shape::convex_polygon(points, color, Stroke::NONE));
    }
    /// SVG subpaths of single segments, such as "M8 1v3M8 12v3".
    fn segments(&self, segments: &[[(f32, f32); 2]], width: f32, color: Color32) {
        for [a, b] in segments {
            self.painter
                .line_segment([self.at(*a), self.at(*b)], self.stroke(width, color));
        }
    }
    fn glyph(&self, glyph: &str, size: f32, color: Color32) {
        self.painter.text(
            self.at((8.0, 8.0)),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(size * self.unit),
            color,
        );
    }
}

/// Paints `icon` into `rect`. `ink` stands for the SVG's `currentColor`: the
/// text colour of the widget the icon belongs to.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, ink: Color32, visuals: &egui::Visuals) {
    let side = rect.width().min(rect.height());
    let pen = Pen {
        painter,
        origin: rect.center() - Vec2::splat(side / 2.0),
        unit: side / icon.view_box(),
    };
    // The coordinates below are the mock-ups' SVG coordinates, element by element.
    match icon {
        Icon::App => {
            pen.fill((1.0, 2.0, 14.0, 9.0), MONITOR_FACE);
            pen.frame((1.0, 2.0, 14.0, 9.0), MONITOR_EDGE);
            pen.fill((3.0, 4.0, 10.0, 5.0), MONITOR_SCREEN);
            pen.fill((5.0, 12.0, 6.0, 2.0), MONITOR_FACE);
            pen.frame((5.0, 12.0, 6.0, 2.0), MONITOR_EDGE);
        }
        Icon::Play => pen.polygon(&[(4.0, 2.0), (14.0, 8.0), (4.0, 14.0)], START_GREEN),
        Icon::Pause => {
            pen.fill((3.0, 2.0, 3.5, 12.0), ink);
            pen.fill((9.5, 2.0, 3.5, 12.0), ink);
        }
        Icon::Stop => pen.fill((3.0, 3.0, 10.0, 10.0), STOP_RED),
        Icon::Floppy => {
            pen.fill((2.0, 1.5, 12.0, 13.0), FLOPPY_BODY);
            pen.fill((4.5, 1.5, 7.0, 4.5), FLOPPY_SHUTTER);
            pen.fill((4.0, 9.0, 8.0, 5.5), FLOPPY_LABEL);
        }
        Icon::Eject => {
            // The mock-up writes ⏏ as text; this is the same symbol, drawn.
            pen.polygon(&[(2.0, 10.0), (8.0, 2.0), (14.0, 10.0)], ink);
            pen.fill((2.0, 12.0, 12.0, 3.0), ink);
        }
        Icon::Settings => {
            painter.circle_stroke(pen.at((8.0, 8.0)), 3.0 * pen.unit, pen.stroke(2.0, ink));
            pen.segments(
                &[
                    [(8.0, 1.0), (8.0, 4.0)],
                    [(8.0, 12.0), (8.0, 15.0)],
                    [(1.0, 8.0), (4.0, 8.0)],
                    [(12.0, 8.0), (15.0, 8.0)],
                    [(3.0, 3.0), (5.0, 5.0)],
                    [(11.0, 11.0), (13.0, 13.0)],
                    [(13.0, 3.0), (11.0, 5.0)],
                    [(5.0, 11.0), (3.0, 13.0)],
                ],
                1.5,
                ink,
            );
        }
        Icon::Layers => {
            pen.polygon(
                &[(8.0, 2.0), (14.0, 5.0), (8.0, 8.0), (2.0, 5.0)],
                LAYER_BLUE,
            );
            pen.line(&[(2.0, 8.0), (8.0, 11.0), (14.0, 8.0)], 1.0, ink);
            pen.line(&[(2.0, 11.0), (8.0, 14.0), (14.0, 11.0)], 1.0, ink);
        }
        Icon::Machine => {
            pen.frame((1.5, 2.5, 13.0, 8.5), ink);
            pen.segments(
                &[[(5.0, 14.0), (11.0, 14.0)], [(8.0, 11.0), (8.0, 14.0)]],
                1.0,
                ink,
            );
        }
        Icon::Image => {
            pen.closed(&[(8.0, 1.5), (14.0, 4.5), (8.0, 7.5), (2.0, 4.5)], ink);
            pen.line(&[(2.0, 8.0), (8.0, 11.0), (14.0, 8.0)], 1.0, ink);
            pen.line(&[(2.0, 11.0), (8.0, 14.0), (14.0, 11.0)], 1.0, ink);
        }
        Icon::Media => {
            pen.frame((2.5, 1.5, 11.0, 13.0), ink);
            pen.frame((5.0, 1.5, 6.0, 4.0), ink);
        }
        Icon::Firmware => {
            pen.frame((3.5, 3.5, 9.0, 9.0), ink);
            pen.segments(
                &[
                    [(6.0, 1.0), (6.0, 3.5)],
                    [(10.0, 1.0), (10.0, 3.5)],
                    [(6.0, 12.5), (6.0, 15.0)],
                    [(10.0, 12.5), (10.0, 15.0)],
                    [(1.0, 6.0), (3.5, 6.0)],
                    [(1.0, 10.0), (3.5, 10.0)],
                    [(12.5, 6.0), (15.0, 6.0)],
                    [(12.5, 10.0), (15.0, 10.0)],
                ],
                1.0,
                ink,
            );
        }
        Icon::MachineFile => {
            pen.closed(
                &[
                    (3.0, 1.5),
                    (10.0, 1.5),
                    (13.0, 4.5),
                    (13.0, 14.5),
                    (3.0, 14.5),
                ],
                ink,
            );
            pen.segments(
                &[
                    [(5.0, 7.0), (11.0, 7.0)],
                    [(5.0, 9.5), (11.0, 9.5)],
                    [(5.0, 12.0), (9.0, 12.0)],
                ],
                1.0,
                ink,
            );
        }
        Icon::Info => {
            painter.circle_stroke(pen.at((8.0, 8.0)), 6.5 * pen.unit, pen.stroke(1.0, ink));
            painter.circle_filled(pen.at((8.0, 4.75)), pen.unit, ink);
            pen.segments(&[[(8.0, 7.0), (8.0, 12.0)]], 1.6, ink);
        }
        Icon::Question | Icon::Refusal => {
            let (disc, glyph) = if icon == Icon::Question {
                // The mock-up's title blue, a shade darker in the dark theme.
                let blue = if visuals.dark_mode {
                    Color32::from_rgb(0x3a, 0x5f, 0x99)
                } else {
                    LAYER_BLUE
                };
                (blue, "?")
            } else {
                (STOP_RED, "✕")
            };
            painter.circle_filled(pen.at((8.0, 8.0)), 8.0 * pen.unit, disc);
            pen.glyph(glyph, 9.0, Color32::WHITE);
        }
        Icon::EmptyLibrary => {
            let face = visuals.panel_fill;
            let selection = visuals.selection.bg_fill;
            let selection_ink = visuals.selection.stroke.color;
            pen.fill((6.0, 5.0, 36.0, 27.0), face);
            pen.frame((6.0, 5.0, 36.0, 27.0), ink);
            pen.fill((10.0, 9.0, 28.0, 19.0), selection);
            pen.segments(
                &[
                    [(13.0, 13.0), (22.0, 13.0)],
                    [(13.0, 17.0), (28.0, 17.0)],
                    [(13.0, 21.0), (19.0, 21.0)],
                ],
                1.5,
                selection_ink,
            );
            pen.segments(
                &[[(20.0, 32.0), (20.0, 36.0)], [(28.0, 32.0), (28.0, 36.0)]],
                1.0,
                ink,
            );
            pen.fill((4.0, 36.0, 40.0, 8.0), face);
            pen.frame((4.0, 36.0, 40.0, 8.0), ink);
            pen.segments(&[[(27.0, 40.0), (38.0, 40.0)]], 1.5, ink);
        }
        Icon::Refresh => {
            // "M13 8a5 5 0 1 1-1.5-3.6": the large clockwise arc from 0° to
            // the end point at about 314°, then the arrow head.
            pen.line(&Pen::arc_points((8.0, 8.0), 5.0, 0.0, 314.2), 1.5, ink);
            pen.line(&[(12.5, 1.5), (12.5, 5.0), (9.0, 5.0)], 1.5, ink);
        }
        Icon::Pull => {
            // "M8 3v10M4 9l4 4 4-4": the shaft, then the arrow head.
            pen.segments(&[[(8.0, 3.0), (8.0, 13.0)]], 1.6, ink);
            pen.line(&[(4.0, 9.0), (8.0, 13.0), (12.0, 9.0)], 1.6, ink);
        }
        Icon::Registry => {
            pen.frame((2.5, 2.5, 11.0, 4.0), ink);
            pen.frame((2.5, 9.5, 11.0, 4.0), ink);
            pen.segments(
                &[[(5.0, 4.5), (6.0, 4.5)], [(5.0, 11.5), (6.0, 11.5)]],
                1.0,
                ink,
            );
        }
        Icon::Namespace => pen.closed(
            &[
                (1.5, 3.5),
                (6.5, 3.5),
                (8.0, 5.0),
                (14.5, 5.0),
                (14.5, 13.5),
                (1.5, 13.5),
            ],
            ink,
        ),
        Icon::Lock => {
            pen.frame((3.5, 7.5, 9.0, 7.0), ink);
            let mut shackle = vec![(5.5, 7.5)];
            shackle.extend(Pen::arc_points((8.0, 5.0), 2.5, 180.0, 360.0));
            shackle.push((10.5, 7.5));
            pen.line(&shackle, 1.0, ink);
            pen.segments(&[[(8.0, 10.0), (8.0, 12.0)]], 1.0, ink);
        }
        Icon::Unreachable => {
            pen.circle((20.0, 20.0), 16.0, 2.0, ink);
            pen.segments(
                &[[(13.0, 13.0), (27.0, 27.0)], [(27.0, 13.0), (13.0, 27.0)]],
                2.5,
                ink,
            );
        }
        Icon::LoginRequired => {
            pen.frame_width((9.0, 18.0, 22.0, 17.0), 2.0, ink);
            let mut shackle = vec![(14.0, 18.0)];
            shackle.extend(Pen::arc_points((20.0, 13.0), 6.0, 180.0, 360.0));
            shackle.push((26.0, 18.0));
            pen.line(&shackle, 2.0, ink);
            pen.segments(&[[(20.0, 24.0), (20.0, 29.0)]], 2.5, ink);
        }
        Icon::EmptyRegistry => {
            pen.closed_width(
                &[
                    (4.0, 12.0),
                    (16.0, 12.0),
                    (19.0, 15.0),
                    (36.0, 15.0),
                    (36.0, 34.0),
                    (4.0, 34.0),
                ],
                2.0,
                ink,
            );
            pen.segments(&[[(14.0, 25.0), (26.0, 25.0)]], 2.0, ink);
        }
        Icon::Listing => {
            pen.circle((20.0, 20.0), 16.0, 2.0, ink);
            pen.line(&[(20.0, 10.0), (20.0, 20.0), (27.0, 24.0)], 2.5, ink);
        }
    }
}

/// Allocates the icon's square in `ui` and paints it in the text colour.
pub fn show(ui: &mut Ui, icon: Icon) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(icon.size()), egui::Sense::hover());
    let ink = ui.visuals().text_color();
    paint(ui.painter(), rect, icon, ink, ui.visuals());
    response
}

/// The icon as a button or selectable atom, painted in the widget's text
/// colour, so it turns white on a selection as the label beside it does.
pub fn atom(icon: Icon) -> egui::Atom<'static> {
    egui::Atom::paint(Vec2::splat(icon.size()), move |ui, args| {
        paint(
            ui.painter(),
            args.rect,
            icon,
            args.fallback_text_color,
            ui.visuals(),
        );
    })
}

/// An empty icon-sized atom, so a label without an icon lines up with its
/// neighbours that have one.
pub fn blank() -> egui::Atom<'static> {
    egui::Atom::paint(Vec2::splat(16.0), |_, _| ())
}

/// The mark column of a menu item, as the main-window mock-up lays its menus
/// out: a glyph such as ▶ or ⏏, or "" to keep the column empty so labels align.
pub fn mark(glyph: &'static str) -> egui::Atom<'static> {
    egui::Atom::paint(Vec2::splat(14.0), move |ui, args| {
        if !glyph.is_empty() {
            ui.painter().text(
                args.rect.center(),
                Align2::CENTER_CENTER,
                glyph,
                egui::TextStyle::Button.resolve(ui.style()),
                args.fallback_text_color,
            );
        }
    })
}

/// A toolbar button as the main-window mock-up draws it: the icon, then the
/// label. Without an icon the label is code-styled text, as the mock-up sets
/// Ctrl+Alt+Del. Without `framed` its frame shows only while it is hovered,
/// pressed or focused.
pub fn button(ui: &mut Ui, label: &str, icon: Option<Icon>, framed: bool) -> Response {
    let font = match icon {
        Some(_) => egui::TextStyle::Button.resolve(ui.style()),
        None => FontId::monospace(11.0),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), font.clone(), Color32::WHITE);
    let (lead, extra) = if icon.is_some() {
        (26.0, 34.0)
    } else {
        (8.0, 16.0)
    };
    let response = ui.add_sized(
        [galley.size().x + extra, 26.0],
        egui::Button::new("").frame_when_inactive(framed),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let color = ui.style().interact(&response).fg_stroke.color;
    if let Some(icon) = icon {
        let rect = Rect::from_min_size(
            response.rect.left_center() + Vec2::new(5.0, -8.0),
            Vec2::splat(16.0),
        );
        paint(ui.painter(), rect, icon, color, ui.visuals());
    }
    ui.painter().text(
        response.rect.left_center() + Vec2::new(lead, 0.0),
        Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
    response
}

/// Whether the window draws its own classic title strip: the classic skin
/// does, except on macOS, where the native title bar takes its place.
pub fn custom_title_bar(classic: bool, macos: bool) -> bool {
    classic && !macos
}

/// This crate's source folder, for the desktop's test that every string it
/// and this crate draw has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Runs `check` with the fonts the desktop installs.
#[cfg(any(test, feature = "test-support"))]
pub fn with_fonts(check: impl FnOnce(&mut egui::epaint::FontsView<'_>)) {
    let ctx = egui::Context::default();
    crate::skin::configure_context(&ctx);
    let mut check = Some(check);
    let output = ctx.run_pass(Default::default(), |ctx| {
        if let Some(check) = check.take() {
            ctx.fonts_mut(check);
        }
    });
    output.drop_without_applying_deltas();
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

    /// WCAG 2.x contrast ratio between two opaque sRGB colours.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let luminance = |c: Color32| {
            let channel = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn colours(shape: &Shape, out: &mut Vec<Color32>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| colours(s, out)),
            Shape::LineSegment { stroke, .. } => out.push(stroke.color),
            Shape::Path(path) => {
                out.push(path.fill);
                if let egui::epaint::ColorMode::Solid(color) = path.stroke.color {
                    out.push(color);
                }
            }
            Shape::Rect(rect) => {
                out.push(rect.fill);
                out.push(rect.stroke.color);
            }
            Shape::Circle(circle) => {
                out.push(circle.fill);
                out.push(circle.stroke.color);
            }
            Shape::Text(text) => out.push(text.fallback_color),
            _ => (),
        }
    }

    /// Every icon paints inside its square in both skins and both themes, and
    /// its text-coloured parts stand out from the panel.
    #[test]
    fn every_icon_draws_legibly_in_every_skin_and_theme() {
        for classic in [false, true] {
            for dark in [false, true] {
                let visuals = crate::skin::skin_visuals(classic, dark);
                let ink = visuals.text_color();
                // Icons are graphics: WCAG 2.x asks 3:1 of them (non-text contrast).
                assert!(contrast(ink, visuals.panel_fill) >= 3.0);
                for icon in Icon::ALL {
                    let ctx = egui::Context::default();
                    crate::skin::configure_context(&ctx);
                    ctx.set_visuals(visuals.clone());
                    let mut drawn = Rect::NOTHING;
                    let mut output = ctx.run_ui(Default::default(), |ui| {
                        drawn = show(ui, icon).rect;
                    });
                    output.textures_delta.clear();
                    let mut painted = Vec::new();
                    for clipped in &output.shapes {
                        colours(&clipped.shape, &mut painted);
                        let bounds = clipped.shape.visual_bounding_rect();
                        assert!(
                            drawn.expand(2.0).contains_rect(bounds),
                            "{icon:?} draws outside its square: {bounds:?} in {drawn:?}"
                        );
                    }
                    painted.retain(|c| *c != Color32::TRANSPARENT);
                    assert!(!painted.is_empty(), "{icon:?}: nothing painted");
                    assert_eq!(drawn.size(), Vec2::splat(icon.size()), "{icon:?}");
                }
            }
        }
    }

    /// The mock-up's `currentColor` parts follow the widget's text colour; the
    /// fixed fills do not.
    #[test]
    fn current_colour_parts_take_the_text_colour() {
        let ink = Color32::from_rgb(1, 2, 3);
        let painted = |icon: Icon| {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(Default::default(), |ui| {
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(icon.size()));
                paint(ui.painter(), rect, icon, ink, ui.visuals());
            });
            output.textures_delta.clear();
            let mut colours_seen = Vec::new();
            for clipped in &output.shapes {
                colours(&clipped.shape, &mut colours_seen);
            }
            colours_seen.retain(|c| *c != Color32::TRANSPARENT);
            colours_seen
        };
        for icon in [
            Icon::Pause,
            Icon::Eject,
            Icon::Settings,
            Icon::Machine,
            Icon::Image,
            Icon::Media,
            Icon::Firmware,
            Icon::MachineFile,
            Icon::Info,
            Icon::Refresh,
            Icon::Pull,
            Icon::Registry,
            Icon::Namespace,
            Icon::Lock,
            Icon::Unreachable,
            Icon::LoginRequired,
            Icon::EmptyRegistry,
            Icon::Listing,
        ] {
            assert!(painted(icon).iter().all(|c| *c == ink), "{icon:?}");
        }
        assert_eq!(painted(Icon::Play), vec![START_GREEN]);
        assert_eq!(painted(Icon::Stop), vec![STOP_RED]);
        let layers = painted(Icon::Layers);
        assert!(layers.contains(&LAYER_BLUE) && layers.contains(&ink));
        assert!(!painted(Icon::App).contains(&ink));
        assert!(!painted(Icon::Floppy).contains(&ink));
    }
}
