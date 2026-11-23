// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helpers that drive egui frame by frame in tests, for the crates that draw
//! with this kit: a modal's settled place, a pointer that clicks and hovers,
//! and where a text is painted. They name no platform; see
//! `backlog/docs/design/core-contract.md` §8.1.

/// Draws a modal on a fresh context for thirty frames of one viewport and
/// returns where it settled. egui measures a new modal in an invisible first
/// frame and centres the next one on that guess, so the height must hold from
/// the first visible frame and the whole rectangle from the one after. A modal
/// lays its content out inside last frame's size: content that fills the
/// height on offer grows the dialog frame by frame.
pub fn settled_modal(
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
pub struct Pointer {
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
pub fn painted_at(texts: &[(String, egui::Rect)], label: &str) -> egui::Rect {
    texts
        .iter()
        .find(|(shown, _)| shown == label)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{label:?} not painted in {texts:?}"))
}

/// Checks settled modal bounds and the painted footer labels at the minimum window.
pub fn minimum_dialog(id: &str, width: f32, buttons: &[&str], draw: impl FnMut(&egui::Context)) {
    minimum_dialog_with_content(id, width, buttons, &[], draw);
}

/// Also checks that body text can be reached by scrolling while the footer stays visible.
pub fn minimum_dialog_with_content(
    id: &str,
    width: f32,
    buttons: &[&str],
    content: &[&str],
    mut draw: impl FnMut(&egui::Context),
) {
    let ctx = egui::Context::default();
    crate::skin::configure_context(&ctx);
    ctx.set_visuals(crate::skin::skin_visuals(true, false));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 500.0));
    let mut previous = None;
    let mut reached = vec![false; content.len()];
    for frame in 0..if content.is_empty() { 30 } else { 120 } {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events: if frame >= 15 && (frame == 15 || !content.is_empty()) {
                    vec![
                        egui::Event::PointerMoved(screen.center()),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Unknown,
                            delta: egui::vec2(
                                0.0,
                                if content.is_empty() { -1000.0 } else { -25.0 },
                            ),
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                } else {
                    Vec::new()
                },
                ..Default::default()
            },
            |ui| draw(ui.ctx()),
        );
        output.textures_delta.clear();
        if frame < 2 {
            continue;
        }
        let rect = ctx
            .memory(|m| m.area_rect(egui::Id::unique(id)))
            .expect("modal shown");
        assert!(screen.contains_rect(rect), "{id}: {rect:?}");
        assert!(
            (rect.width() - width).abs() < 0.5 && (rect.height() - 476.0).abs() < 0.5,
            "{id}: {rect:?}, expected {width} × 476"
        );
        if let Some(previous) = previous {
            assert_eq!(rect, previous, "modal stays settled");
        }
        previous = Some(rect);
        for (label, reached) in content.iter().zip(&mut reached) {
            fn visible(shape: &egui::Shape, clip: egui::Rect, label: &str) -> bool {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        clip.contains_rect(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    egui::Shape::Vec(shapes) => {
                        shapes.iter().any(|shape| visible(shape, clip, label))
                    }
                    _ => false,
                }
            }
            *reached |= frame >= 15
                && output
                    .shapes
                    .iter()
                    .any(|shape| visible(&shape.shape, shape.clip_rect.intersect(screen), label));
        }
        for button in buttons {
            let mut found = false;
            fn walk(
                shape: &egui::Shape,
                clip: egui::Rect,
                screen: egui::Rect,
                label: &str,
                found: &mut bool,
            ) {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        let rect = text.galley.rect.translate(text.pos.to_vec2());
                        assert!(
                            screen.contains_rect(rect) && clip.contains_rect(rect),
                            "{label}: {rect:?} clipped by {clip:?}"
                        );
                        *found = true;
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, clip, screen, label, found);
                        }
                    }
                    _ => (),
                }
            }
            for shape in &output.shapes {
                walk(&shape.shape, shape.clip_rect, screen, button, &mut found);
            }
            assert!(found, "footer label {button} is painted");
        }
    }
    for (label, reached) in content.iter().zip(reached) {
        assert!(reached, "body text {label:?} is reachable after scrolling");
    }
}
