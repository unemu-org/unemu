// SPDX-License-Identifier: AGPL-3.0-or-later
//! File › New machine…'s wizard: the PC library's model
//! (`pc_library::new_machine`), re-exported under the path the wizard's view
//! names it by, with the tests that draw the view.

pub(crate) use pc_library::new_machine::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_test_support::FirmwareStore;
    // What the tests took from the model's own imports.
    use assembly::capabilities::{self as caps, Value};

    fn wizard(store: &FirmwareStore) -> Wizard {
        let mut wizard = Wizard::new();
        wizard.store = Some(store.store.clone());
        wizard.set_store(store.choices());
        wizard
    }
    fn enabled(wizard: &Wizard, path: &str, value: Value) -> Option<bool> {
        wizard
            .choices(path)
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.enabled)
    }
    fn reason(wizard: &Wizard, path: &str, value: Value) -> String {
        wizard
            .choices(path)
            .iter()
            .find(|c| c.value == value)
            .and_then(|c| c.reason.clone())
            .unwrap_or_default()
    }

    #[test]
    fn every_step_renders_without_a_window() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let mut wizard = wizard(&store);
        let ctx = egui::Context::default();
        for step in 0..STEPS.len() {
            wizard.go(step);
            for classic in [true, false] {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    assert!(matches!(
                        crate::new_machine_view::show(ui.ctx(), &mut wizard, classic),
                        crate::new_machine_view::Action::None
                    ));
                });
                output.textures_delta.clear();
            }
        }
        let mut empty = Wizard::new();
        empty.set_store(Default::default());
        empty.go(MOTHERBOARD);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = crate::new_machine_view::show(ui.ctx(), &mut empty, true);
        });
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Import ROM folder"))));
        output.textures_delta.clear();
    }

    /// A greyed option in a list says why on hover, as its line under the
    /// field does without hovering.
    #[test]
    fn an_unavailable_option_says_why_on_hover_and_in_its_line() {
        use crate::library_test_support::{painted_at, Pointer};
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let mut wizard = wizard(&store);
        wizard.go(CPU_MEMORY);
        let path = "card[lotech-ems].frame";
        let frame = wizard.hardware().ems.unwrap().frame;
        assert_eq!(enabled(&wizard, path, Value::Integer(0xE000)), Some(false));
        let why = reason(&wizard, path, Value::Integer(0xE000));
        let draft = wizard.draft().unwrap();
        let label = |value: u16| caps::value_label(&draft, path, &Value::Integer(value.into()));
        let (current, blocked) = (label(frame), label(0xE000));
        let tooltip = crate::new_machine_view::capitalized(&why);
        let pointer = Pointer::new(egui::vec2(1040.0, 900.0));
        let mut draw = |ui: &mut egui::Ui| {
            let _ = crate::new_machine_view::show(ui.ctx(), &mut wizard, false);
        };
        // A new modal settles over its first frames.
        let mut shown = Vec::new();
        for _ in 0..3 {
            shown = pointer.frame(Vec::new(), &mut draw);
        }
        painted_at(&shown, &format!("{blocked}: {why}"));
        assert!(!shown.iter().any(|(t, _)| *t == tooltip));
        pointer.click(painted_at(&shown, &current).center(), &mut draw);
        pointer.frame(Vec::new(), &mut draw);
        let popup = pointer.frame(Vec::new(), &mut draw);
        let hovered = pointer.hover(
            painted_at(&popup, &format!("{blocked} — unavailable")).center(),
            &mut draw,
        );
        painted_at(&hovered, &tooltip);
    }

    #[test]
    fn the_wizard_keeps_one_height_inside_the_viewport() {
        let store = FirmwareStore::new(true);
        let mut wizard = match &store {
            Some(store) => wizard(store),
            None => {
                let mut empty = Wizard::new();
                empty.set_store(Default::default());
                empty
            }
        };
        let id = egui::Id::unique("new-machine-wizard");
        for size in [egui::vec2(1040.0, 700.0), egui::vec2(1040.0, 500.0)] {
            for step in 0..STEPS.len() {
                wizard.go(step);
                for classic in [true, false] {
                    crate::library_test_support::settled_modal(id, size, |ctx| {
                        let _ = crate::new_machine_view::show(ctx, &mut wizard, classic);
                    });
                }
            }
        }
    }

    /// The Name and tag step never offers an official template's name as an
    /// example repository: the user's own image is not the template.
    #[test]
    fn the_name_step_never_suggests_a_template_name() {
        let mut wizard = Wizard::new();
        wizard.set_store(Default::default());
        wizard.go(NAME);
        let ctx = egui::Context::default();
        let mut pass = || {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1040.0, 900.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let _ = crate::new_machine_view::show(ui.ctx(), &mut wizard, false);
            });
            output.textures_delta.clear();
            output
        };
        // A new modal settles over its first frames.
        pass();
        pass();
        let output = pass();
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => (),
            }
        }
        let mut shown = Vec::new();
        for clipped in &output.shapes {
            texts(&clipped.shape, &mut shown);
        }
        assert!(
            shown.iter().any(|text| text.contains("Repository name")),
            "{shown:?}"
        );
        for text in &shown {
            for word in text.split_whitespace() {
                let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
                assert!(
                    pc_image::template::TEMPLATES.iter().all(|t| t.name != word),
                    "{text:?} names the template {word:?}"
                );
            }
        }
    }
}
