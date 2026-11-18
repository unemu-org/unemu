// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless Settings on an image: the PC library's model
//! (`pc_library::image_settings`), re-exported under its old path, with its
//! tests and the helpers that build images for the desktop's tests. The tests
//! that draw the settings sheet stay here.

pub(crate) use pc_library::image_settings::*;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library_test_support::FirmwareStore;
    use crate::{
        firmware_choices::{self as fw, Slot},
        library_view::ImageRow,
    };
    use pc_image::revision::Source;
    // The helpers live with the model; the desktop's tests name them here.
    pub(crate) use pc_library::image_settings::test_support::{
        open_image, open_new_image, opened, reviewed, write_c,
    };

    /// Every text a frame of `draw` paints in a central panel.
    fn painted_ui(mut draw: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| draw(ui));
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        texts
    }
    /// Every text a frame of `draw` paints.
    fn painted(draw: impl FnMut(&egui::Context)) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut draw = draw;
        let mut texts = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| draw(ui.ctx()));
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        texts
    }

    #[test]
    fn a_revision_of_an_official_template_stays_local_under_a_local_suffix_tag() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let template = &pc_image::template::TEMPLATES[1];
        let mut image = open_new_image(&store, 1024);
        image.title = Some(template.title.into());
        let reference = format!("{}:1.1.0", template.name);
        let descriptor = pc_image::create::create(&store.store, &image, &reference, None).unwrap();
        pc_image::create::create(
            &store.store,
            &image,
            &format!("{}:latest", template.name),
            None,
        )
        .unwrap();
        let row = ImageRow {
            reference: reference.clone(),
            aliases: Vec::new(),
            descriptor,
            summary: None,
            failure: None,
        };
        let mut model = opened(&store, row, false);
        assert!(model.official_template());
        model.set_packages(store.choices().0);
        // A user-supplied ROM makes the open template's revision user-supplied.
        model.set_firmware(Slot::System, "kiselev-xi8088-1.0.2");
        reviewed(&mut model, &store.store);
        let review = model.review.as_mut().unwrap();
        assert_eq!(
            review.save.reference(),
            format!("{}:1.1.0-local.1", template.name)
        );
        assert!(review.plan.as_ref().unwrap().is_ok());
        // Every existing tag is refused, latest included.
        for taken in ["1.1.0", "latest"] {
            review.save.set_tag(taken.into());
            assert_eq!(
                review.save.check().level,
                crate::save_image::TagLevel::Bad,
                "{taken}"
            );
            assert_eq!(review.save.submit(), None, "{taken}");
            review.save.dismiss();
        }
        review.save.set_tag("1.1.0-local.1".into());
        let texts = painted(|ctx| {
            let _ = crate::settings_sheet::show_review(ctx, &mut model, false, true);
        });
        assert!(
            texts.iter().any(|t| t.contains(
                "which is read-only for you. The revision is written to your local store only"
            )),
            "{texts:?}"
        );
        assert!(texts
            .iter()
            .any(|t| t.starts_with("user-supplied: with the Kiselev")));
        assert!(texts
            .iter()
            .any(|t| t.starts_with("Suggested: the source version with a local suffix")));
        // An image of your own repository is no template.
        let mut image = open_new_image(&store, 1024);
        image.title = Some(template.title.into());
        let descriptor =
            pc_image::create::create(&store.store, &image, "my-at:1.0.0", None).unwrap();
        let own = opened(
            &store,
            ImageRow {
                reference: "my-at:1.0.0".into(),
                aliases: Vec::new(),
                descriptor,
                summary: None,
                failure: None,
            },
            false,
        );
        assert!(!own.official_template());
    }

    /// An image with an unlisted system BIOS and a stored CMOS: switching to
    /// the open AT BIOS resets the CMOS, as pc-image does, since nobody knows
    /// the unlisted BIOS's layout.
    #[test]
    fn an_unlisted_bios_switched_to_the_open_at_bios_shows_the_cmos_reset() {
        let Some(firmware) = FirmwareStore::new(true) else {
            return;
        };
        let fixture = crate::library_test_support::Fixture::new();
        let source = Source::open(&fixture.store, &fixture.row.descriptor).unwrap();
        assert!(source.stores_cmos());
        let tags = crate::save_image::local_tags(&fixture.store).unwrap();
        let mut model = ImageSettings::new(fixture.row.clone(), source, tags, false).unwrap();
        assert_eq!(model.firmware(Slot::System), None, "the BIOS is unlisted");
        model.set_packages(firmware.choices().0);
        model.set_firmware(Slot::System, "unemu-at-bios-1.0.2-unemu.3");
        let open_at = fw::entry("unemu-at-bios-1.0.2-unemu.3").unwrap();
        assert_eq!(
            model.cmos_line(),
            Some(CmosLine::Reset {
                from: None,
                to: Some(open_at)
            })
        );
        let texts = painted_ui(|ui| {
            let _ = crate::settings_sheet::firmware_tab(ui, &mut model, false);
        });
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("⚠ CMOS settings are reset at the next start.")),
            "{texts:?}"
        );
    }

    /// A VGA whose tables bind no video ROM shows None for its Video BIOS.
    #[test]
    fn a_vga_without_a_video_rom_shows_none() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let row = open_image(&store);
        let model = opened(&store, row, false);
        let mut tables = model.machine().clone();
        tables["firmware"]
            .as_table_mut()
            .unwrap()
            .get_mut("option_roms")
            .and_then(toml::Value::as_array_mut)
            .unwrap()
            .retain(|rom| rom.get("base").and_then(toml::Value::as_integer) != Some(0xC0000));
        let mut model = model.for_machine("vga".into(), "VGA".into(), tables);
        model.set_packages(store.choices().0);
        assert!(model.has_video_bios() && model.firmware(Slot::Video).is_none());
        let texts = painted_ui(|ui| {
            let _ = crate::settings_sheet::firmware_tab(ui, &mut model, true);
        });
        assert!(texts.iter().any(|t| t == "None"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "An unlisted ROM"));
    }
}
