// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless Settings on an image: the PC library's model
//! (`pc_library::image_settings`), re-exported under its old path. Its tests
//! stay here, since they open images through the library's fixture and draw
//! the settings sheet; other tests of the desktop build their images with
//! the helpers below.

pub(crate) use pc_library::image_settings::*;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library_test_support::FirmwareStore;
    use crate::{
        firmware_choices::{self as fw, Slot},
        library_view::ImageRow,
    };
    use assembly::capabilities::{self as caps, Value};
    use assembly::CmosLayout;
    use pc_image::create::Package;
    use pc_image::create::{Chs, Hardware, NewImage, PackageRom, Serial, Video};
    use pc_image::revision::Source;

    /// A New machine… image of the open fixtures: Kiselev AT BIOS, open VGA
    /// BIOS, XTIDE, a blank C:, drive A: 1.2M and a title.
    pub(crate) fn open_new_image(store: &FirmwareStore, extended_kib: u32) -> NewImage {
        let packages = pc_image::create::packages(&store.store).unwrap();
        let rom = |id: &str| {
            packages
                .iter()
                .find_map(|p| {
                    p.config
                        .roms
                        .iter()
                        .find(|r| r.catalogue_id.as_deref() == Some(id))
                        .map(|r| PackageRom::new(p, r))
                })
                .unwrap()
        };
        NewImage {
            hardware: Hardware {
                board: assembly::BoardModel::Ibm5170,
                rate: (8, 13),
                fpu: false,
                conventional_kib: 640,
                extended_kib,
                video: Video::Vga,
                sound: None,
                serial: Serial::Empty,
                ems: None,
                floppies: [Some("1.2M".into()), None],
                ide_port: 0x1F0,
                disks: [Some(Chs::new(615, 4, 17)), None],
            },
            firmware: vec![
                rom("unemu-at-bios-1.0.2-unemu.3"),
                rom("vgabios-0.9d-unemu.1-std"),
                rom("xub-r638-xtp-ata1f0"),
            ],
            title: Some("Open AT workbench".into()),
            description: None,
        }
    }
    /// That image with 1 MiB extended memory, created as open-at:1.0.0.
    pub(crate) fn open_image(store: &FirmwareStore) -> ImageRow {
        let image = open_new_image(store, 1024);
        let descriptor =
            pc_image::create::create(&store.store, &image, "open-at:1.0.0", None).unwrap();
        ImageRow {
            reference: "open-at:1.0.0".into(),
            aliases: Vec::new(),
            descriptor,
            summary: None,
            failure: None,
        }
    }

    /// Write `value` into sector 0 of C: through the session's journal.
    pub(crate) fn write_c(prepared: &pc_image::workspace::PreparedSession, value: u8) {
        let drive = prepared
            .machine
            .drives
            .iter()
            .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
            .unwrap();
        let assembly::ImageSource::File(path) = &drive.image else {
            panic!("expected a named base")
        };
        let assembly::WriteMode::Journal { path: journal, .. } = &drive.write else {
            panic!("expected a journal")
        };
        let base = prepared.bases[path].clone();
        let mut media = media::JournalMedia::open_verified_with_metadata(
            journal,
            &base,
            media::SectorSource::byte_len(&base),
            Default::default(),
            None,
        )
        .unwrap();
        media.write_batch(&[(0, [value; 512])]);
        media.finish().unwrap();
    }

    /// Settings as the worker opens them: source and tags read from the store.
    pub(crate) fn opened(store: &FirmwareStore, row: ImageRow, read_only: bool) -> ImageSettings {
        let source = Source::open(&store.store, &row.descriptor).unwrap();
        let tags = crate::save_image::local_tags(&store.store).unwrap();
        ImageSettings::new(row, source, tags, read_only).unwrap()
    }
    /// The review as the worker answers it.
    pub(crate) fn reviewed(model: &mut ImageSettings, store: &image_store::Store) {
        let request = model.begin_review();
        let plan = pc_image::revision::plan(store, &model.source, request.machine)
            .map_err(|e| e.to_string());
        let tags = crate::save_image::local_tags(store).unwrap();
        model.planned(request.revision, plan, tags);
    }

    fn bios_sha(machine: &assembly::MachineConfig) -> String {
        machine
            .images
            .iter()
            .find_map(|image| match &image.source {
                assembly::ImageSource::Inline(bytes)
                    if image.base + bytes.len() as u64 == 0x100000 =>
                {
                    Some(image_store::Digest::of(bytes).hex().to_owned())
                }
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn the_firmware_tab_offers_fitting_entries_by_family_and_binds_a_package_rom() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        // Until the packages are read, only the current entry can be chosen.
        assert!(model
            .firmware_choices(Slot::System)
            .iter()
            .all(|c| c.reason.is_some() == (c.entry.id != "unemu-at-bios-1.0.2-unemu.3")));
        model.set_packages(store.choices().0);
        let choices = model.firmware_choices(Slot::System);
        let ids: Vec<&str> = choices.iter().map(|c| c.entry.id).collect();
        assert_eq!(
            ids,
            [
                "unemu-at-bios-1.0.2-unemu.3",
                "unemu-at-bios-1.0.2-unemu.2",
                "kiselev-xi8088-1.0.2",
                "kiselev-xi8088-1.0.2-atreset",
                "ibm-5170-bios-1984-01-10",
                "ibm-5170-bios-1985-06-10",
                "ibm-5170-bios-1985-11-15",
                "landmark-rom-post-at-1.2.0",
            ],
            "grouped by family, a superseded release after the one replacing it"
        );
        let reason = |id: &str| {
            choices
                .iter()
                .find(|c| c.entry.id == id)
                .unwrap()
                .reason
                .clone()
        };
        assert_eq!(reason("unemu-at-bios-1.0.2-unemu.3"), None);
        assert!(reason("unemu-at-bios-1.0.2-unemu.2")
            .unwrap()
            .contains("not in your store"));
        assert!(reason("ibm-5170-bios-1985-06-10")
            .unwrap()
            .contains("never downloaded"));
        assert_eq!(
            reason("kiselev-xi8088-1.0.2"),
            None,
            "a package of the store"
        );
        let videos: Vec<&str> = model
            .firmware_choices(Slot::Video)
            .iter()
            .map(|c| c.entry.id)
            .collect();
        assert_eq!(
            videos,
            [
                "ibm-vga-bios-1987-32k",
                "vgabios-0.9d-unemu.1-std",
                "vgabios-0.8a-unemu.1-std"
            ]
        );
        assert!(model.has_video_bios());

        // Another family: this image derives its CMOS, so nothing is reset;
        // the user-supplied package makes later images user-supplied.
        model.set_firmware(Slot::System, "kiselev-xi8088-1.0.2");
        assert_eq!(
            model.firmware(Slot::System).unwrap().id,
            "kiselev-xi8088-1.0.2"
        );
        assert_eq!(
            model.image_firmware(Slot::System).unwrap().id,
            "unemu-at-bios-1.0.2-unemu.3"
        );
        assert!(model.dirty() && model.can_review());
        assert_eq!(
            model.cmos_line(),
            Some(CmosLine::Derived(Some(CmosLayout::KiselevXi8088)))
        );
        let users: Vec<&str> = model.new_user_supplied().iter().map(|e| e.id).collect();
        assert_eq!(users, ["kiselev-xi8088-1.0.2"]);
        assert!(model
            .changes()
            .iter()
            .any(|(field, _)| field.group == caps::Group::Firmware));
        // The revision binds the package's ROM blob by digest.
        reviewed(&mut model, &store.store);
        let plan = model.review.as_ref().unwrap().plan.as_ref().unwrap();
        let plan = plan.as_ref().unwrap();
        assert!(plan
            .added
            .iter()
            .any(|d| d.decoded_media_type() == image_store::media_type::FIRMWARE));
        // The image's own entry brings its table back exactly.
        model.set_firmware(Slot::System, "unemu-at-bios-1.0.2-unemu.3");
        assert!(!model.dirty());
        assert_eq!(model.cmos_line(), None);
        // An entry that is not in the store cannot be chosen.
        model.set_firmware(Slot::System, "ibm-5170-bios-1985-06-10");
        assert!(model
            .error
            .as_deref()
            .unwrap()
            .contains("not in your store"));
        assert!(!model.dirty());
    }

    #[test]
    fn a_machine_firmware_choice_applies_at_its_next_start() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let row = open_image(&store);
        let id = pc_image::machine::create(
            &store.store,
            &row.descriptor,
            "open-at:1.0.0",
            Some("Firmware"),
            None,
        )
        .unwrap();
        let tables = pc_image::machine::settings(&store.store, &id)
            .unwrap()
            .machine;
        let mut model =
            opened(&store, row, false).for_machine(id.clone(), "Firmware".into(), tables);
        model.set_packages(store.choices().0);
        model.set_firmware(Slot::System, "kiselev-xi8088-1.0.2");
        let (machine, table) = model.apply_request(true).unwrap();
        let change =
            pc_image::machine::change_settings(&store.store, &machine, table, true).unwrap();
        assert!(!change.cmos_reset, "the image derives its CMOS");
        model.applied();
        assert!(!model.unsaved());
        let owner = pc_image::machine::own(&store.store, &id).unwrap();
        let prepared = pc_image::machine::prepare(&owner).unwrap();
        assert_eq!(
            bios_sha(&prepared.session.machine),
            fw::entry("kiselev-xi8088-1.0.2").unwrap().sha256
        );
    }

    #[test]
    fn switch_to_opens_the_firmware_tab_with_the_choice_made_once_packages_are_read() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        model.switch_to(Slot::System, "kiselev-xi8088-1.0.2");
        assert_eq!(model.tab, caps::Group::Firmware);
        assert_eq!(
            model.firmware(Slot::System).unwrap().id,
            "unemu-at-bios-1.0.2-unemu.3",
            "nothing changes before the packages are read"
        );
        model.set_packages(store.choices().0);
        assert_eq!(
            model.firmware(Slot::System).unwrap().id,
            "kiselev-xi8088-1.0.2"
        );
        // With the packages read, the choice is made at once.
        model.switch_to(Slot::System, "unemu-at-bios-1.0.2-unemu.3");
        assert!(!model.dirty());
    }

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

    /// A package's claim of a catalogue id never stands in for the
    /// catalogue's bytes: SHA-256 and size decide.
    #[test]
    fn a_package_rom_fits_an_entry_only_with_its_bytes() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let packages = store.choices().0;
        let old = fw::entry("unemu-at-bios-1.0.2-unemu.2").unwrap();
        let current = fw::entry("unemu-at-bios-1.0.2-unemu.3").unwrap();
        // A package of other bytes claiming the superseded release's id.
        let mut liar = packages
            .iter()
            .find(|p| {
                p.config
                    .roms
                    .iter()
                    .any(|r| r.catalogue_id.is_none() && r.role == "system-bios")
            })
            .unwrap()
            .clone();
        for rom in &mut liar.config.roms {
            rom.catalogue_id = Some(old.id.into());
        }
        assert!(fw::package_rom(std::slice::from_ref(&liar), old, Slot::System).is_none());
        let found = fw::package_rom(&packages, current, Slot::System).unwrap();
        assert_eq!(found.rom().unwrap().sha256, current.sha256);
    }

    /// "Switch to …" for a ROM that is not in the store says so, and is made
    /// once Import ROM folder… brought it in.
    #[test]
    fn a_switch_waits_for_its_rom_and_is_made_after_an_import() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        let all = store.choices().0;
        let kiselev = fw::entry("kiselev-xi8088-1.0.2").unwrap();
        let without: Vec<Package> = all
            .iter()
            .filter(|p| !p.config.roms.iter().any(|r| r.sha256 == kiselev.sha256))
            .cloned()
            .collect();
        model.set_packages(without);
        model.switch_to(Slot::System, kiselev.id);
        assert!(model
            .error
            .as_deref()
            .unwrap()
            .contains("Import ROM folder"));
        assert_eq!(
            model.firmware(Slot::System).unwrap().id,
            "unemu-at-bios-1.0.2-unemu.3"
        );
        // The import's package refresh makes the switch and clears the error.
        model.set_packages(all);
        assert_eq!(model.error, None);
        assert_eq!(model.firmware(Slot::System).unwrap().id, kiselev.id);
    }

    /// Official templates are known by their bare name; a revision of a
    /// local revision takes the next local number, and a source tagged only
    /// `latest` takes the repository's newest version.
    #[test]
    fn local_revision_tags_follow_the_template_release_without_doubling_the_suffix() {
        let Some(store) = FirmwareStore::new(true) else {
            return;
        };
        let template = pc_image::template::TEMPLATES[1].name;
        let tag_image = |tag: &str, kib: u32| {
            let mut image = open_new_image(&store, kib);
            image.title = Some("Not the template's title".into());
            let reference = format!("{template}:{tag}");
            let descriptor =
                pc_image::create::create(&store.store, &image, &reference, None).unwrap();
            ImageRow {
                reference,
                aliases: Vec::new(),
                descriptor,
                summary: None,
                failure: None,
            }
        };
        tag_image("1.1.0", 1024);
        let local = tag_image("1.1.0-local.1", 2048);
        let latest = tag_image("latest", 3072);
        let mut model = opened(&store, local, false);
        assert!(model.official_template(), "the bare name decides");
        model.set("memory.extended_kib", Value::Integer(4096));
        reviewed(&mut model, &store.store);
        assert_eq!(
            model.review.as_ref().unwrap().save.reference(),
            format!("{template}:1.1.0-local.2")
        );
        let mut model = opened(&store, latest, false);
        model.set("memory.extended_kib", Value::Integer(4096));
        reviewed(&mut model, &store.store);
        assert_eq!(
            model.review.as_ref().unwrap().save.reference(),
            format!("{template}:1.1.0-local.2")
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

    #[test]
    fn closing_with_edits_asks_first_and_a_stale_check_is_ignored() {
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        assert!(model.request_close(), "nothing to lose");
        model.set("memory.extended_kib", Value::Integer(2048));
        assert!(!model.request_close());
        assert!(model.discard_prompt);
        // A check of an older draft never lands in a newer review.
        let stale = model.begin_review();
        model.set("memory.extended_kib", Value::Integer(4096));
        let request = model.begin_review();
        assert_ne!(stale.revision, request.revision);
        model.planned(stale.revision, Err("old".into()), Vec::new());
        assert!(model.review.as_ref().unwrap().plan.is_none());
        model.planned(request.revision, Err("current".into()), Vec::new());
        assert!(model.review.as_ref().unwrap().plan.is_some());
        // Diskette choices are computed once per draft.
        let first = model.floppy_choices(1);
        assert_eq!(model.floppies.borrow().len(), 1);
        assert_eq!(model.floppy_choices(1), first);
        model.set_floppy(1, Some("720K"));
        assert!(model.floppies.borrow().is_empty());
    }

    #[test]
    fn edits_go_through_the_capabilities_api_into_the_hardware_tables() {
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        assert!(!model.dirty() && !model.can_review());
        model.set("memory.extended_kib", Value::Integer(4096));
        assert!(model.dirty() && model.can_review());
        assert_eq!(
            model.machine["memory"]["extended_kib"].as_integer(),
            Some(4096)
        );
        let changes = model.changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0.path, "memory.extended_kib");
        assert_eq!(changes[0].1, Some(Value::Integer(1024)));
        // Installing the Sound Blaster adds the card; its defaults come from lowering.
        model.set("card[sb16].enabled", Value::Name("installed".into()));
        assert!(caps::fields(&model.draft)
            .iter()
            .any(|f| f.path == "card[sb16].base" && f.value == Value::Integer(0x220)));
        model.set("card[sb16].enabled", Value::Name("none".into()));
        // Reverting the memory leaves nothing to review.
        model.set("memory.extended_kib", Value::Integer(1024));
        assert!(!model.dirty());
        // Firmware is chosen on its own tab, never as a raw field.
        model.set("firmware.bios", Value::Disconnected);
        assert!(model.error.as_deref().unwrap().contains("Firmware tab"));
    }

    #[test]
    fn drive_b_can_be_installed_and_offers_only_valid_types() {
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row, false);
        assert_eq!(model.floppy(0).as_deref(), Some("1.2M"));
        assert_eq!(model.floppy(1), None);
        let choices = model.floppy_choices(1);
        assert_eq!(choices[0], (None, None));
        assert!(choices
            .iter()
            .any(|(kind, reason)| kind.as_deref() == Some("720K") && reason.is_none()));
        model.set_floppy(1, Some("720K"));
        assert_eq!(model.floppy(1).as_deref(), Some("720K"));
        assert!(model.can_review());
        model.set_floppy(1, None);
        assert!(!model.dirty());
    }

    #[test]
    fn running_images_are_read_only_and_invalid_drafts_cannot_review() {
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row.clone(), true);
        model.set("memory.extended_kib", Value::Integer(4096));
        assert!(!model.dirty() && !model.can_review());
        let mut model = opened(&store, row, false);
        // A resource conflict stays in the draft with the validator's reason.
        model.set("card[lotech-ems].enabled", Value::Name("installed".into()));
        model.set("card[sb16].enabled", Value::Name("installed".into()));
        let port = caps::fields(&model.draft)
            .into_iter()
            .find(|f| f.path.ends_with("lotech-ems].port"))
            .unwrap()
            .value;
        model.set("card[sb16].base", port);
        assert!(model.dirty());
        assert!(model.validation().has_errors());
        assert!(!model.can_review());
    }

    #[test]
    fn review_shows_the_configuration_diff_shared_blobs_and_suggests_the_next_version() {
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let row = open_image(&store);
        let mut model = opened(&store, row.clone(), false);
        model.set("memory.extended_kib", Value::Integer(2048));
        reviewed(&mut model, &store.store);
        let review = model.review.as_ref().unwrap();
        let plan = review.plan.as_ref().unwrap().as_ref().unwrap();
        assert!(plan.diff.contains(&pc_image::revision::DiffLine::Added(
            "extended_kib = 2048".into()
        )));
        assert!(
            plan.added.is_empty(),
            "disks, firmware and metadata are shared"
        );
        assert_ne!(plan.machine.0, plan.machine.1);
        // open-at is the official template's name: a revision of it takes the
        // source version with a local suffix.
        assert_eq!(review.save.reference(), "open-at:1.0.0-local.1");
        // Another edit drops the review: it described the old draft.
        model.set("memory.extended_kib", Value::Integer(4096));
        assert!(model.review.is_none());
    }

    /// A template pulled from a registry other than the family default
    /// keeps its host in the local tag, and is still the official template:
    /// its revision takes the family short name with a local suffix.
    #[test]
    fn a_template_pulled_from_another_registry_is_revised_under_its_short_name() {
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let reference = "nas.example:5000/unemu/open-at:1.0.0";
        let image = open_new_image(&store, 1024);
        let descriptor = pc_image::create::create(&store.store, &image, reference, None).unwrap();
        let row = ImageRow {
            reference: reference.into(),
            aliases: Vec::new(),
            descriptor,
            summary: None,
            failure: None,
        };
        let mut model = opened(&store, row, false);
        assert!(model.official_template());
        model.set("memory.extended_kib", Value::Integer(2048));
        reviewed(&mut model, &store.store);
        let review = model.review.as_ref().unwrap();
        assert_eq!(review.save.reference(), "open-at:1.0.0-local.1");
        assert!(review.save.local_suffix());
    }
}
