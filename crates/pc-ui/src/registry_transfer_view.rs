// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image › Push…, Image › Pull by reference… and Pull… from the library's
//! Remotes are drawn by `library_ui::registry_transfer_view`, re-exported
//! here under the path the library frame names it by. The dialogs' own
//! test plans a push with the program's provider list and stays with the
//! application; the drawing helpers the Remotes tests use are copied here.

pub(crate) use library_ui::registry_transfer_view::*;

#[cfg(test)]
pub(crate) mod tests {
    fn texts(output: &egui::FullOutput) -> Vec<String> {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut out);
        }
        out
    }

    /// Two frames, so the modal is laid out; the second frame's action and text.
    pub(crate) fn render<T>(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut draw: impl FnMut(&egui::Context) -> T,
    ) -> (T, Vec<String>) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw(ui.ctx());
        });
        output.textures_delta.clear();
        let mut result = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| result = Some(draw(ui.ctx())),
        );
        output.textures_delta.clear();
        (result.unwrap(), texts(&output))
    }

    pub(crate) fn escape() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    pub(crate) fn shows(texts: &[String], wanted: &str) -> bool {
        texts.iter().any(|text| text.contains(wanted))
    }

    /// Click the text `label` as a pointer would, after the frames that lay
    /// the modal out; the result of the frame that releases the button.
    pub(crate) fn click<T>(
        ctx: &egui::Context,
        label: &str,
        mut draw: impl FnMut(&egui::Context) -> T,
    ) -> T {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
                _ => None,
            }
        }
        let mut frame = |events: Vec<egui::Event>| {
            let mut result = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| result = Some(draw(ui.ctx())),
            );
            output.textures_delta.clear();
            (result.unwrap(), output)
        };
        frame(vec![]);
        let (_, output) = frame(vec![]);
        let at = output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, label))
            .unwrap_or_else(|| panic!("{label} is not shown"));
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(vec![egui::Event::PointerMoved(at), button(true)]);
        frame(vec![button(false)]).0
    }
}
