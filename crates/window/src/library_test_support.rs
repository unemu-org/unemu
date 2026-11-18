// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless desktop sessions use synthetic firmware, disks, catalogue and CMOS.
//!
//! The library's fixtures live with the library (`pc_library`'s test
//! support) and are re-exported here; the helpers that drive the toolkit stay.
pub(crate) use pc_library::library_test_support::*;

/// Draws a modal on a fresh context for thirty frames of one viewport and
/// returns where it settled. egui measures a new modal in an invisible first
/// frame and centres the next one on that guess, so the height must hold from
/// the first visible frame and the whole rectangle from the one after. A modal
/// lays its content out inside last frame's size: content that fills the
/// height on offer grows the dialog frame by frame.
pub(crate) fn settled_modal(
    id: egui::Id,
    size: egui::Vec2,
    mut draw: impl FnMut(&egui::Context),
) -> egui::Rect {
    let ctx = egui::Context::default();
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    let mut rects = Vec::new();
    for _ in 0..30 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            },
            |ui| draw(ui.ctx()),
        );
        output.textures_delta.clear();
        rects.push(ctx.memory(|m| m.area_rect(id)).expect("the modal is shown"));
    }
    let heights: Vec<f32> = rects.iter().map(|r| r.height()).collect();
    assert!(
        heights[1..].iter().all(|h| (h - heights[1]).abs() < 0.5),
        "{id:?} at {size:?} changes height: {heights:?}"
    );
    let settled = rects[2];
    assert!(
        rects[2..].iter().all(|r| *r == settled),
        "{id:?} at {size:?} moves: {rects:?}"
    );
    assert!(
        settled.top() >= viewport.top() && settled.bottom() <= viewport.bottom(),
        "{id:?} at {size:?} leaves the viewport: {settled:?}"
    );
    settled
}

/// One context driven frame by frame as a pointer would drive it, with
/// tooltips shown at once (no delay, pointer need not rest), so a test can
/// read hover text. Each frame returns every painted text with the rectangle
/// its glyphs cover, and lasts a sixtieth of a second.
pub(crate) struct Pointer {
    ctx: egui::Context,
    viewport: egui::Rect,
    time: std::cell::Cell<f64>,
}
impl Pointer {
    pub fn new(size: egui::Vec2) -> Self {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        Self {
            ctx,
            viewport: egui::Rect::from_min_size(egui::Pos2::ZERO, size),
            time: std::cell::Cell::new(0.0),
        }
    }
    pub fn frame(
        &self,
        events: Vec<egui::Event>,
        draw: &mut dyn FnMut(&mut egui::Ui),
    ) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                // A label in a wrapped row starts its galley at the row's
                // left edge; only the rows' glyphs say where the text is.
                egui::Shape::Text(text) => out.push((
                    text.galley.text().to_owned(),
                    text.galley
                        .rows
                        .iter()
                        .filter(|row| !row.glyphs.is_empty())
                        .map(|row| row.rect_without_leading_space())
                        .reduce(|a, b| a.union(b))
                        .unwrap_or_else(|| text.galley.rect)
                        .translate(text.pos.to_vec2()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => (),
            }
        }
        self.time.set(self.time.get() + 1.0 / 60.0);
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(self.viewport),
                time: Some(self.time.get()),
                events,
                ..Default::default()
            },
            |ui| draw(ui),
        );
        output.textures_delta.clear();
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }
    /// A primary click at `at`: press, then release (when egui reports it).
    pub fn click(&self, at: egui::Pos2, draw: &mut dyn FnMut(&mut egui::Ui)) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![egui::Event::PointerMoved(at), button(true)], draw);
        self.frame(vec![button(false)], draw);
    }
    /// What is painted while the pointer rests on `at`. The pointer arrives
    /// a while after any click, as a user's does: egui hides a tooltip right
    /// after a click.
    pub fn hover(
        &self,
        at: egui::Pos2,
        draw: &mut dyn FnMut(&mut egui::Ui),
    ) -> Vec<(String, egui::Rect)> {
        self.time.set(self.time.get() + 1.0);
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = self.frame(vec![egui::Event::PointerMoved(at)], draw);
        }
        texts
    }
}

/// Where `label` is painted, as exactly that text.
pub(crate) fn painted_at(texts: &[(String, egui::Rect)], label: &str) -> egui::Rect {
    texts
        .iter()
        .find(|(shown, _)| shown == label)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{label:?} not painted in {texts:?}"))
}
