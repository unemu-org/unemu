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
//! `crate::shell::configure_context` installs cover every one of them. ⓘ is
//! painted ([`Icon::Info`]) because no bundled font has it.
//!
//! The test-only `inventory` lists every icon of every approved mock-up with
//! the place the app draws it, or the reason it does not; see machine-images.md
//! §7 and `backlog/docs/design/icons.md`.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Response, Shape, Stroke, Ui, Vec2};

#[cfg(test)]
mod inventory;

/// An icon the mock-ups draw as SVG artwork (and ⓘ, which no font has).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
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
}

impl Icon {
    /// Every icon, in declaration order.
    #[cfg(test)]
    pub(crate) const ALL: [Icon; 17] = [
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
    ];

    /// The side of the square the icon is drawn in, in points.
    pub(crate) fn size(self) -> f32 {
        match self {
            Icon::Question | Icon::Refusal => 32.0,
            Icon::EmptyLibrary => 48.0,
            _ => 16.0,
        }
    }

    /// The side of the mock-up's SVG view box.
    fn view_box(self) -> f32 {
        if self == Icon::EmptyLibrary {
            48.0
        } else {
            16.0
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
pub(crate) const STAR_GOLD: Color32 = Color32::from_rgb(0xc0, 0x8a, 0x00);

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
        let points = points.iter().map(|&p| self.at(p)).collect();
        self.painter
            .add(Shape::closed_line(points, self.stroke(1.0, color)));
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
pub(crate) fn paint(
    painter: &Painter,
    rect: Rect,
    icon: Icon,
    ink: Color32,
    visuals: &egui::Visuals,
) {
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
    }
}

/// Allocates the icon's square in `ui` and paints it in the text colour.
pub(crate) fn show(ui: &mut Ui, icon: Icon) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(icon.size()), egui::Sense::hover());
    let ink = ui.visuals().text_color();
    paint(ui.painter(), rect, icon, ink, ui.visuals());
    response
}

/// The icon as a button or selectable atom, painted in the widget's text
/// colour, so it turns white on a selection as the label beside it does.
pub(crate) fn atom(icon: Icon) -> egui::Atom<'static> {
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
pub(crate) fn blank() -> egui::Atom<'static> {
    egui::Atom::paint(Vec2::splat(16.0), |_, _| ())
}

/// The mark column of a menu item, as the main-window mock-up lays its menus
/// out: a glyph such as ▶ or ⏏, or "" to keep the column empty so labels align.
pub(crate) fn mark(glyph: &'static str) -> egui::Atom<'static> {
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
/// Ctrl+Alt+Del.
pub(super) fn button(ui: &mut Ui, label: &str, icon: Option<Icon>) -> Response {
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
    let response = ui.add_sized([galley.size().x + extra, 26.0], egui::Button::new(""));
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

    /// The characters inside string and character literals of a Rust source,
    /// outside comments.
    fn literal_chars(source: &str) -> Vec<char> {
        let chars: Vec<char> = source.chars().collect();
        let mut found = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            } else if c == 'r'
                && matches!(next, Some('"' | '#'))
                && !chars
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.is_alphanumeric() || *p == '_')
            {
                let mut hashes = 0;
                i += 1;
                while chars.get(i) == Some(&'#') {
                    hashes += 1;
                    i += 1;
                }
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
                        i += hashes;
                        break;
                    }
                    found.push(chars[i]);
                    i += 1;
                }
                i += 1;
            } else if c == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    found.push(chars[i]);
                    i += 1;
                }
                i += 1;
            } else if c == '\'' {
                // A character literal, or a lifetime such as 'a or 'static.
                if next == Some('\\') {
                    let end = (i + 2..chars.len())
                        .find(|&k| chars[k] == '\'')
                        .unwrap_or(i);
                    i = end + 1;
                } else if chars.get(i + 2) == Some(&'\'') {
                    found.extend(next);
                    i += 3;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        found
    }

    #[test]
    fn literal_scan_skips_comments_and_reads_raw_and_char_literals() {
        let source = "// ★ not this\nlet a = \"✓ yes\"; let b = '✗'; fn f<'a>() {}\n\
                      /* ⓘ not this */ let c = r#\"⏏ \"raw\"\"#; let d = \"\\\"▶\";";
        let found: String = literal_chars(source)
            .into_iter()
            .filter(|c| !c.is_ascii())
            .collect();
        assert_eq!(found, "✓✗⏏▶");
    }

    /// Every source file of the desktop crate, by path.
    fn sources() -> Vec<(std::path::PathBuf, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    out.push((path, text));
                }
            }
        }
        let mut out = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut out,
        );
        assert!(out.len() > 20, "the desktop sources were not found");
        out
    }

    /// Runs `check` with the fonts the desktop installs.
    pub(super) fn with_fonts(check: impl FnOnce(&mut egui::epaint::FontsView<'_>)) {
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
        let mut check = Some(check);
        let output = ctx.run_pass(Default::default(), |ctx| {
            if let Some(check) = check.take() {
                ctx.fonts_mut(check);
            }
        });
        output.drop_without_applying_deltas();
    }

    /// Every character any string of the desktop draws has a glyph in both
    /// font families it can be drawn in, so none renders as an empty box.
    #[test]
    fn every_string_the_desktop_draws_has_a_glyph() {
        let sources = sources();
        with_fonts(|fonts| {
            let mut missing = Vec::new();
            for (path, text) in &sources {
                // Test modules and the test-only mock-up inventory draw nothing.
                if path.ends_with("shell_icons/inventory.rs") {
                    continue;
                }
                let drawn = text.split("\n#[cfg(test)]\nmod tests").next().unwrap();
                for ch in literal_chars(drawn) {
                    if ch.is_ascii() {
                        continue;
                    }
                    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                        if !fonts.has_glyph(&egui::FontId::new(14.0, family.clone()), ch) {
                            missing.push(format!(
                                "U+{:04X} {ch} ({family:?}) in {}",
                                ch as u32,
                                path.file_name().unwrap().to_string_lossy()
                            ));
                        }
                    }
                }
            }
            missing.sort();
            missing.dedup();
            assert!(missing.is_empty(), "no glyph for:\n{}", missing.join("\n"));
        });
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
                let visuals = crate::shell::skin_visuals(classic, dark);
                let ink = visuals.text_color();
                // Icons are graphics: WCAG 2.x asks 3:1 of them (non-text contrast).
                assert!(contrast(ink, visuals.panel_fill) >= 3.0);
                for icon in Icon::ALL {
                    let ctx = egui::Context::default();
                    crate::shell::configure_context(&ctx);
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
