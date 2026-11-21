// SPDX-License-Identifier: AGPL-3.0-or-later
//! Floppy catalogue drafts and blank-image forms for the desktop shell.
//!
//! The drafts, the forms and the pending media are PC media models in
//! `pc_media::media_dialog`, re-exported here. What stays is what needs the
//! desktop: the insert dialog's library form, drawn with egui, and putting the
//! pending floppies into the machine a Start assembled, which the desktop's
//! session holds. See `backlog/docs/design/machine-images.md` §7 and
//! persistent-media.md § Media changes.

use assembly::{ImageSource, WriteMode};
pub(crate) use pc_media::media_dialog::*;
use std::path::Path;
// The names the tests below shared with the model when both were one file.
#[cfg(test)]
use assembly::{FloppyGeometry, MachineConfig};
#[cfg(test)]
use std::io;

/// The library form of the Insert floppy dialog.
pub(crate) trait LibraryInsertView {
    /// Returns true when From library… asks the caller to open the library,
    /// which knows the session workspace.
    fn library_ui(&mut self, ui: &mut egui::Ui) -> bool;
}

impl LibraryInsertView for InsertDialog {
    fn library_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(library) = &mut self.library else {
            return ui.button("From library…").clicked();
        };
        if let Some(error) = library.poll() {
            self.error = Some(error);
        }
        let busy = library.is_busy() || library.ready.is_some();
        let mut back = false;
        ui.add_enabled_ui(!busy, |ui| {
        ui.label(format!(
            "Drive {}: — {} floppy",
            if self.unit == 0 { "A" } else { "B" },
            geometry_name(self.slot.geometry)
        ));
        ui.label("Choose an installation set. Unavailable formats stay visible with a reason.");
        ui.horizontal(|ui| {
            ui.label("Store");
            ui.text_edit_singleline(&mut library.store_path);
            if ui.button("Refresh").clicked() {
                self.error = None; library.refresh_async(self.slot.geometry, self.unit);
            }
        });
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                for (i, row) in library.rows.iter().enumerate() {
                    let response = ui.add_enabled(
                        row.reason.is_none(),
                        egui::RadioButton::new(
                            library.selected == Some(i),
                            format!("{} · {}", row.title, row.tag),
                        ),
                    );
                    if response.clicked() {
                        library.selected = Some(i);
                        library.first = 0;
                        self.error = None;
                    }
                    if let Some(reason) = &row.reason {
                        ui.label(reason);
                    }
                }
                if library.rows.is_empty() {
                    ui.label("No installation sets in this store.");
                }
            });
        if let Some(config) = library.selected.and_then(|i| library.rows.get(i)).and_then(|row| row.config.as_ref()) {
            egui::ComboBox::from_label("Insert disk")
                .selected_text(
                    config.media
                        .get(library.first)
                        .map_or("Choose a disk", |m| m.label.as_str()),
                )
                .show_ui(ui, |ui| {
                    for (i, m) in config.media.iter().enumerate() {
                        ui.selectable_value(&mut library.first, i, &m.label);
                    }
                });
        }
        ui.label("Workspace writes");
        ui.label("Already loaded disks retain their existing catalogue write policy.");
        if let Some(reason) = library.journal_unavailable {
            ui.add(egui::Label::new(reason).wrap());
        }
        ui.add_enabled_ui(library.journal_unavailable.is_none(), |ui| {
            ui.radio_value(&mut library.journal, true, "Per-workspace journal");
            ui.radio_value(&mut library.journal, false, "Write-protected");
        });
        ui.checkbox(
            &mut library.load_all,
            "Load all disks of the set into the catalogue in order",
        );
        ui.label("The library set stays read-only. Journals belong only to this session's workspace.");
        back = ui.button("Back to catalogue").clicked();
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&library.progress);
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
        if back {
            self.library = None;
        }
        false
    }
}

/// Start's half of the floppies that wait for it.
pub(crate) trait ApplyPending {
    /// Put each pending floppy into its drive of the machine Start assembled,
    /// before its first instruction, and name each drive whose floppy did not
    /// go in. A disk the machine file lists goes in only while the started
    /// catalogue still lists the same image at that place, so an edit of the
    /// file after the Insert never swaps in another disk. A library disk's
    /// journal belongs to the started session's media folder `media_dir`,
    /// which only Start opens; without one, that disk stays out.
    fn apply(self, presenter: &mut crate::Presenter, media_dir: Option<&Path>) -> Vec<String>;
}

impl ApplyPending for PendingMedia {
    fn apply(self, presenter: &mut crate::Presenter, media_dir: Option<&Path>) -> Vec<String> {
        let mut failures = Vec::new();
        for (unit, insert) in (0u8..).zip(self.inserts) {
            let Some((mut entries, chosen)) = insert else {
                continue;
            };
            let name = entries
                .get(chosen)
                .map(|entry| entry.medium.name.clone())
                .unwrap_or_default();
            let mut insert = || -> Result<(), String> {
                for entry in &mut entries {
                    let listed = entry
                        .index
                        .filter(|_| entry.declared)
                        .map(|index| listed_medium(&presenter.media_config, unit, index));
                    // Start pins the hash it verified; the Insert knew only
                    // the file's own pin, if it has one.
                    let same =
                        |(image, sha256, write): (&ImageSource, Option<[u8; 32]>, &WriteMode)| {
                            image == &entry.medium.image
                                && write == &entry.medium.write
                                && entry
                                    .medium
                                    .expected_sha256
                                    .is_none_or(|pin| sha256 == Some(pin))
                        };
                    if listed.is_some_and(|listed| !listed.is_some_and(same)) {
                        return Err("the machine file now lists another disk there".into());
                    }
                    if entry.workspace_journal.is_some() {
                        entry.move_workspace_journal(
                            media_dir.ok_or("no session media folder for its journal")?,
                        );
                    }
                }
                let (assembled, config, slots) = presenter.media_parts();
                let indices = append_entries(assembled, config, slots, unit, &entries)?;
                let index = *indices.get(chosen).ok_or("Select an image")?;
                presenter.media_events.push((
                    unemu_core_api::SlotId(unit),
                    unemu_core_api::MediaAction::Insert(index),
                ));
                Ok(())
            };
            if let Err(error) = insert() {
                failures.push(format!("{}: {name}: {error}", char::from(b'A' + unit)));
            }
        }
        failures
    }
}

#[cfg(test)]
mod media_set_tests {
    use super::*;
    use image_store::{
        self as store,
        media_set::{Config, Medium},
    };
    use pc_image::media_set as pc;
    fn config(format: &str) -> Config {
        Config {
            schema: 1,
            platform: "ibm-pc".into(),
            media: vec![Medium {
                id: "disk-1".into(),
                label: "Disk one".into(),
                format: pc::format(format).unwrap(),
                blob: store::Digest::of(b"disk"),
            }],
            extras: Vec::new(),
        }
    }
    #[test]
    fn media_set_incompatible_and_optical_rows_explain_disabled_state() {
        assert_eq!(
            set_compatibility(&config("1.2M"), FloppyGeometry::HD_1_44, 0).as_deref(),
            Some("1.2M disks; drive A: is 1.44M, which takes 1.44M or 720K")
        );
        assert_eq!(
            set_compatibility(&config("iso-2048"), FloppyGeometry::HD_1_44, 0).as_deref(),
            Some("CD-ROM emulation not available yet")
        );
        assert_eq!(
            set_compatibility(&config("1.44M"), FloppyGeometry::HD_1_44, 0),
            None
        );
        // A 1.44M drive takes 720K disks too; a 1.2M disk in the set refuses it.
        let mut mixed = config("1.44M");
        mixed.media.push(config("720K").media.remove(0));
        assert_eq!(set_compatibility(&mixed, FloppyGeometry::HD_1_44, 1), None);
        assert_eq!(
            set_compatibility(&mixed, FloppyGeometry::DD_720K, 1).as_deref(),
            Some("1.44M disks; drive B: is 720K, which takes 720K")
        );
        mixed.media.push(config("1.2M").media.remove(0));
        assert!(set_compatibility(&mixed, FloppyGeometry::HD_1_44, 1)
            .unwrap()
            .contains("drive B:"));
        assert_eq!(
            set_compatibility(&config("360K"), FloppyGeometry::HD_1_2, 0),
            None
        );
    }
    #[test]
    fn media_set_load_all_appends_in_order_and_journals_preserve_store() {
        let temp = assembly::test_support::TempDir::new("library-insert");
        let root = temp.path().canonicalize().unwrap();
        let store_path = root.join("store");
        let source = root.join("first.img");
        let second = root.join("second.img");
        let args = vec![
            "floppy".into(),
            "--type".into(),
            "1.44M".into(),
            "--format".into(),
            source.clone().into_os_string(),
        ];
        pc_image::media_create::execute(
            pc_image::media_create::parse(&args).unwrap(),
            &mut io::sink(),
        )
        .unwrap();
        let bytes = std::fs::read(&source).unwrap();
        let mut other = bytes.clone();
        other[100] = 5;
        std::fs::write(&second, &other).unwrap();
        crate::library_test_support::import_media_set(
            &image_store::Store::open(&store_path).unwrap(),
            "test-set:r1",
            "Synthetic installation",
            &[
                crate::library_test_support::Medium::new(&source),
                crate::library_test_support::Medium::new(&second),
            ],
            &[],
        );
        let mut machine = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; bytes.len()]);
        let mut assembled = machine_file::assemble(&machine).unwrap();
        let mut slots = FloppySlot::from_config(&machine);
        let mut library = LibraryInsert::new(
            image_store::location::machine_media_dir(&root.join("machine.toml")).unwrap(),
        );
        library.store_path = store_path.to_string_lossy().into_owned();
        library.refresh(FloppyGeometry::HD_1_44, 0).unwrap();
        assert_eq!(library.rows.len(), 1);
        assert_eq!(library.rows[0].reason, None);
        library.selected = Some(0);
        library.first = 1;
        let slot = slots[0].as_ref().unwrap();
        let (entries, chosen) = library.entries(slot, 0).unwrap();
        assert_eq!(chosen, 1);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].medium.name, "first.img");
        assert_eq!(entries[1].medium.name, "second.img");
        assert_eq!(
            entries[1].origin.as_deref(),
            Some("Media set test-set:r1 from the library · disk 2 of 2")
        );
        let indices =
            append_entries(&mut assembled, &mut machine, &mut slots, 0, &entries).unwrap();
        assert_eq!(indices, [1, 2]);
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 3);
        assembled
            .change_floppy(
                assembly::DeviceId::Fdc,
                0,
                assembly::FloppyAction::Insert(indices[chosen]),
            )
            .unwrap();
        let views = crate::drive_chips::drive_views(
            &machine,
            &slots,
            &[
                Some((
                    indices[chosen],
                    assembled.floppy_name(assembly::DeviceId::Fdc, 0),
                )),
                None,
            ],
            &crate::drive_chips::Origin::MachineFile("machine".into()),
        );
        assert_eq!(views[0].chip_text(), "A: second.img · 1.44M · journal");
        assert!(views[0].tooltip(false).contains(&(
            "Source",
            "Media set test-set:r1 from the library · disk 2 of 2".into()
        )));
        assert_eq!(
            assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "second.img"
        );
        assembled
            .change_floppy(assembly::DeviceId::Fdc, 0, assembly::FloppyAction::Eject)
            .unwrap();
        let ImageSource::File(base_path) = &entries[0].medium.image else {
            panic!("file base");
        };
        let WriteMode::Journal { path, .. } = &entries[0].medium.write else {
            panic!("journal");
        };
        assert!(!path.starts_with(&store_path));
        assembled
            .change_floppy(
                assembly::DeviceId::Fdc,
                0,
                assembly::FloppyAction::Insert(indices[0]),
            )
            .unwrap();
        let device_index = assembled.device_index[&assembly::DeviceId::Fdc];
        let fdc = assembled
            .machine
            .bus_mut()
            .device_mut(device_index)
            .unwrap();
        fdc.port_write(0x3f2, 1, 0x1c).unwrap();
        for byte in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        assert!(fdc.take_dma_request().is_some());
        fdc.dma_complete(device::DmaOutcome::FromMemory {
            bytes: vec![0xA5; 512],
            terminal_count: true,
        });
        for _ in 0..7 {
            fdc.port_read(0x3f5, 1).unwrap();
        }
        assembled
            .change_floppy(assembly::DeviceId::Fdc, 0, assembly::FloppyAction::Eject)
            .unwrap();
        let store = store::Store::open_existing(&store_path).unwrap();
        let source = entries[0].source.as_ref().unwrap();
        let image = entries[0].receipt.as_ref().unwrap();
        let stored = image
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&source.blob))
            .unwrap();
        assert!(stored.is_compressed());
        store.verify(&stored.digest, stored.size).unwrap();
        assert_eq!(std::fs::read(base_path).unwrap(), bytes);
        let scan = journal_files::scan(&std::fs::read(path).unwrap(), &bytes, bytes.len() as u64);
        assert_eq!(&scan.image.unwrap()[..512], &[0xA5; 512]);
        let mut receiving = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; bytes.len()]);
        receiving.drives[0].unit = 1;
        let mut receiving_slots = FloppySlot::from_config(&receiving);
        let mut receiving_machine = machine_file::assemble(&receiving).unwrap();
        let (moved, _) = library
            .entries(receiving_slots[1].as_ref().unwrap(), 1)
            .unwrap();
        assert_eq!(moved[0].medium.write, entries[0].medium.write);
        let moved_indices = append_entries(
            &mut receiving_machine,
            &mut receiving,
            &mut receiving_slots,
            1,
            &moved,
        )
        .unwrap();
        receiving_machine
            .change_floppy(
                assembly::DeviceId::Fdc,
                1,
                assembly::FloppyAction::Insert(moved_indices[0]),
            )
            .unwrap();
        let fdc_index = receiving_machine.device_index[&assembly::DeviceId::Fdc];
        let fdc = receiving_machine
            .machine
            .bus_mut()
            .device_mut(fdc_index)
            .unwrap();
        fdc.port_write(0x3f2, 1, 0x2d).unwrap();
        for byte in [6, 1, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        let device::DmaOp::ToMemory(read_back) = fdc.take_dma_request().unwrap().op else {
            panic!("drive B read");
        };
        assert_eq!(read_back, vec![0xA5; 512]);
        fdc.dma_complete(device::DmaOutcome::ToMemory {
            transferred: 512,
            terminal_count: true,
        });
        for _ in 0..7 {
            fdc.port_read(0x3f5, 1).unwrap();
        }
        receiving_machine
            .change_floppy(assembly::DeviceId::Fdc, 1, assembly::FloppyAction::Eject)
            .unwrap();
        store.verify(&stored.digest, stored.size).unwrap();

        let (reused, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let reused_indices =
            append_entries(&mut assembled, &mut machine, &mut slots, 0, &reused).unwrap();
        assert_eq!(reused_indices, indices);
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 3);
        assert!(source.roots().contains(&library.rows[0].image.digest));
        #[cfg(unix)]
        {
            std::fs::remove_file(path).unwrap();
            std::os::unix::fs::symlink(base_path, path).unwrap();
            assert!(library
                .entries(slots[0].as_ref().unwrap(), 0)
                .unwrap_err()
                .contains("blobs"));
            std::fs::remove_file(path).unwrap();
        }
        library.load_all = false;
        library.journal = false;
        let (single, chosen) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(chosen, 0);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].medium.name, "second.img");
        assert_eq!(single[0].index, Some(2));
        assert_eq!(single[0].medium.write, entries[1].medium.write);
        library.workspace = store_path.join("images/blobs/forbidden");
        assert!(library
            .entries(slots[0].as_ref().unwrap(), 0)
            .unwrap_err()
            .contains("blobs"));
    }
    #[test]
    fn media_set_batch_rejection_leaves_catalogue_unchanged() {
        let temp = assembly::test_support::TempDir::new("library-batch");
        let path = temp.path().join("disk.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; 1_474_560]);
        let mut assembled = machine_file::assemble(&config).unwrap();
        let mut slots = FloppySlot::from_config(&config);
        let first = browse(&path, FloppyGeometry::HD_1_44).unwrap();
        let mut bad = first.clone();
        bad.reason = Some("incompatible disk".into());
        assert!(append_entries(&mut assembled, &mut config, &mut slots, 0, &[first, bad]).is_err());
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        assert!(config.drives[0].media.is_none());
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    fn fixture() -> (
        assembly::test_support::TempDir,
        LibraryInsert,
        MachineConfig,
    ) {
        let temp = assembly::test_support::TempDir::new("review-media");
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("disk.img");
        std::fs::write(&source, vec![0x11; 1_474_560]).unwrap();
        crate::library_test_support::import_media_set(
            &image_store::Store::open(root.join("store")).unwrap(),
            "review-set:r1",
            "Review set",
            &[crate::library_test_support::Medium::new(&source)],
            &[],
        );
        let mut library = LibraryInsert::new(
            image_store::location::machine_media_dir(&root.join("machine.toml")).unwrap(),
        );
        library.store_path = root.join("store").to_string_lossy().into_owned();
        library.refresh(FloppyGeometry::HD_1_44, 0).unwrap();
        library.selected = Some(0);
        (
            temp,
            library,
            assembly::floppy_boot(vec![0xfa, 0xeb, 0xfe], vec![0; 1_474_560]),
        )
    }
    #[test]
    fn reused_library_disk_takes_the_source_of_the_set_it_came_from_again() {
        let (_temp, mut library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = machine_file::assemble(&config).unwrap();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let first = "Media set review-set:r1 from the library · disk 1 of 1";
        assert_eq!(entries[0].origin.as_deref(), Some(first));
        let indices = append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap();
        // The same set under another tag reuses the loaded disk.
        library.rows[0].tag = "review-other:r1".into();
        let (again, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let other = "Media set review-other:r1 from the library · disk 1 of 1";
        assert_eq!(again[0].index, Some(indices[0]));
        assert_eq!(again[0].origin.as_deref(), Some(other));
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &again).unwrap(),
            indices
        );
        let slot = slots[0].as_ref().unwrap();
        assert_eq!(slot.entries.len(), 2);
        assert_eq!(slot.entries[1].origin.as_deref(), Some(other));
    }

    #[test]
    fn review_single_hash_reuse_per_medium_journal_and_gc_roots() {
        let (_temp, mut library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = machine_file::assemble(&config).unwrap();
        let medium = &library.rows[0].config.as_ref().unwrap().media[0];
        let decoded_digest = medium.blob.clone();
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let image = store
            .resolve_descriptor(
                &library.rows[0].image,
                &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
            )
            .unwrap();
        let digest = image
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&medium.blob))
            .unwrap()
            .digest
            .clone();
        let stored = image_store::verification_passes(&digest);
        let hashes = journal_files::hash_passes();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(image_store::verification_passes(&digest) - stored, 1);
        let indices = append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap();
        assert_eq!(journal_files::hash_passes(), hashes);
        let (again, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &again).unwrap(),
            indices
        );
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 2);
        let (other_drive, _) = library
            .entries(
                &FloppySlot {
                    entries: vec![],
                    ..slots[0].as_ref().unwrap().clone()
                },
                1,
            )
            .unwrap();
        assert_eq!(entries[0].medium.write, other_drive[0].medium.write);
        let mut job = library.snapshot();
        assert!(job.request_entries(slots[0].as_ref().unwrap(), 0).is_none());
        let receiver = job.pending.take().unwrap();
        let mut progress = false;
        loop {
            match receiver.recv_timeout(crate::test_wait::HANG_GUARD).unwrap() {
                LibraryMessage::Progress(_) => progress = true,
                LibraryMessage::Ready(result) => {
                    assert_eq!(result.unwrap().0[0].index, Some(indices[0]));
                    break;
                }
                LibraryMessage::Rows(_) => panic!("unexpected refresh result"),
            }
        }
        assert!(progress);

        let roots = entries[0].source.as_ref().unwrap().roots();
        assert!(roots.contains(&decoded_digest));
        assert!(roots.contains(&library.rows[0].image.digest));
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let owner = store.workspace("review").unwrap();
        library.workspace = image_store::location::workspace_media_dir(&owner);
        let (workspace_entries, _) = library
            .entries(
                &FloppySlot {
                    entries: vec![],
                    ..slots[0].as_ref().unwrap().clone()
                },
                0,
            )
            .unwrap();
        let WriteMode::Journal { path, .. } = &workspace_entries[0].medium.write else {
            panic!("journal")
        };
        assert!(path.starts_with(owner.path().join("media")));
    }
    /// A set that names the retired PC platform id goes into a drive like one
    /// that names `ibm-pc`, in this dialog and in the library's Insert; a set
    /// of an unknown platform does not.
    #[test]
    fn a_set_that_names_the_retired_platform_id_goes_in_and_an_unknown_one_does_not() {
        use crate::library_view::install_media::{availability, Availability};
        use image_store::platform::{with_platform_id, ALIASES};
        let (_temp, mut library, config) = fixture();
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let set = store.tag_descriptor("review-set:r1").unwrap();
        let validator = image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats);
        let old = with_platform_id(&store, &set, &[], ALIASES[0].0).unwrap();
        store
            .update_tag("review-old:r1", &old, None, &validator)
            .unwrap();
        let unknown = with_platform_id(&store, &set, &[], "ibm-pc-xt").unwrap();
        assert!(store
            .update_tag("review-unknown:r1", &unknown, None, &validator)
            .is_err());
        store
            .tag_as_newer_release("review-unknown:r1", &unknown)
            .unwrap();
        library.refresh(FloppyGeometry::HD_1_44, 0).unwrap();
        let index = |tag: &str| library.rows.iter().position(|r| r.tag == tag).unwrap();
        assert_eq!(library.rows[index("review-old:r1")].reason, None);
        let reason = library.rows[index("review-unknown:r1")].reason.clone();
        assert_eq!(
            reason.as_deref(),
            Some("Platform ibm-pc-xt is unavailable for this drive")
        );
        library.selected = Some(index("review-old:r1"));
        let slots = FloppySlot::from_config(&config);
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(entries.len(), 1);

        let mut summaries = image_store::summary::SummaryCache::open(&store).unwrap();
        while !summaries.pending().is_empty() {
            summaries.rebuild(&store, 64, &[], &[]).unwrap();
        }
        let drives = [(0, FloppyGeometry::HD_1_44)];
        let summary = summaries.summary(&old.digest).unwrap();
        assert_eq!(availability(summary, &drives), Availability::Ready(0));
        let summary = summaries.summary(&unknown.digest).unwrap();
        assert_eq!(
            availability(summary, &drives),
            Availability::Unavailable("Platform ibm-pc-xt is unavailable for this drive".into())
        );
    }
    #[test]
    fn library_disks_switch_between_write_protection_and_the_workspace_journal() {
        let (_temp, mut library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = machine_file::assemble(&config).unwrap();
        library.journal = false;
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(entries[0].medium.write, WriteMode::ReadOnly);
        let journal = entries[0].workspace_journal.clone().unwrap();
        assert!(!journal.starts_with(&library.store_path));
        assert!(!journal.parent().unwrap().exists());
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap(),
            [1]
        );

        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        draft.select(Some(1));
        assert!(!draft.journal);
        draft.journal = true;
        draft.journal_path = "ignored-for-library-media.upfj".into();
        let entry = draft.selection().unwrap();
        assert_eq!(entry.index, None);
        assert_eq!(
            entry.medium.write,
            WriteMode::Journal {
                path: journal.clone(),
                keep_generations: draft.slot.journal_retention,
            }
        );
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &[entry]).unwrap(),
            [2]
        );
        assert!(journal.parent().unwrap().is_dir());
        for action in [
            assembly::FloppyAction::Insert(2),
            assembly::FloppyAction::Eject,
        ] {
            assembled
                .change_floppy(assembly::DeviceId::Fdc, 0, action)
                .unwrap();
        }
        let row = &slots[0].as_ref().unwrap().entries[1];
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 2);
        assert_eq!(row.alternates, [(WriteMode::ReadOnly, 1)]);

        // Loading the set again keeps the disk's current policy; the
        // catalogue switches it back without appending.
        let (again, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(again[0].index, Some(2));
        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        draft.select(Some(1));
        assert!(draft.journal);
        draft.journal = false;
        let entry = draft.selection().unwrap();
        assert_eq!(entry.index, Some(1));
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &[entry]).unwrap(),
            [1]
        );
        assert_eq!(
            assembled.floppy_names[&(assembly::DeviceId::Fdc, 0)].len(),
            3
        );
    }
    #[test]
    fn review_candidate_validation_and_mid_batch_failure_are_atomic() {
        let (temp, library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = machine_file::assemble(&config).unwrap();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let mut bad = entries[0].clone();
        let ImageSource::File(base) = &bad.medium.image else {
            panic!("base")
        };
        bad.medium.write = WriteMode::Journal {
            path: base.clone(),
            keep_generations: 10,
        };
        assert!(append_entries(&mut assembled, &mut config, &mut slots, 0, &[bad]).is_err());
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        let mut second = entries[0].clone();
        second.medium.name = "Second".into();
        second.medium.write = WriteMode::Journal {
            path: temp.path().join("second.upfj"),
            keep_generations: 10,
        };
        let original_handles = assembled.persistence.len();
        second.medium.expected_sha256 = Some([0; 32]);
        let error = append_entries(
            &mut assembled,
            &mut config,
            &mut slots,
            0,
            &[entries[0].clone(), second],
        )
        .unwrap_err();
        assert!(error.contains("SHA-256"));
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        assert!(config.drives[0].media.is_none());
        assert_eq!(assembled.persistence.len(), original_handles);
        assert_eq!(
            assembled.floppy_names[&(assembly::DeviceId::Fdc, 0)].len(),
            1
        );
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap(),
            [1]
        );
    }
    #[test]
    fn review_tampered_blob_and_receipt_refuse_insertion() {
        let (_temp, library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = machine_file::assemble(&config).unwrap();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let ImageSource::File(base) = &entries[0].medium.image else {
            panic!("base")
        };
        // The decoded file is a rebuildable cache: a damaged copy is rebuilt from
        // the verified stored blob, while receipts taken before stay refused.
        std::fs::write(base, vec![0x22; 1_474_560]).unwrap();
        library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(std::fs::read(base).unwrap(), vec![0x11; 1_474_560]);
        assert!(append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).is_err());
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        // Damaged stored bytes cannot be repaired and refuse the set.
        let receipt = entries[0].receipt.as_ref().unwrap();
        let blob = &entries[0].source.as_ref().unwrap().blob;
        let stored = receipt
            .manifest
            .layers
            .iter()
            .find(|d| d.names(blob))
            .unwrap();
        assert!(stored.is_compressed());
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let path = store.blob_file_path(&stored.digest, stored.size).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
        assert!(library
            .entries(slots[0].as_ref().unwrap(), 0)
            .unwrap_err()
            .contains("digest mismatch"));
    }
    #[test]
    fn review_damaged_set_is_a_disabled_row_and_refresh_is_nonblocking() {
        let (_temp, mut library, _) = fixture();
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let image = store
            .resolve_descriptor(
                &library.rows[0].image,
                &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
            )
            .unwrap();
        let mut manifest = image.manifest.clone();
        let bytes = b"invalid config";
        let blob = store.import(bytes.as_slice(), None).unwrap();
        manifest.config = image_store::Descriptor::new(
            image_store::media_type::MEDIA_SET_CONFIG,
            blob.digest,
            blob.size,
        );
        let damaged = store.import_manifest(&manifest).unwrap();
        let mut bad_summary = image.manifest.clone();
        let blob = store
            .import(b"invalid metadata JSON".as_slice(), None)
            .unwrap();
        let metadata = bad_summary
            .layers
            .iter_mut()
            .find(|d| d.media_type == image_store::media_type::METADATA)
            .unwrap();
        *metadata =
            image_store::Descriptor::new(image_store::media_type::METADATA, blob.digest, blob.size);
        let summary_image = store.import_manifest(&bad_summary).unwrap();

        let index_path = store.root().join("images/index.json");
        // A damaged imported index can reference an invalid set without going through publication.
        let mut index = std::fs::read_to_string(&index_path).unwrap();
        let new= format!("{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"review-damaged:r1\"}}}},",image_store::media_type::MANIFEST,damaged.digest,damaged.size);
        let new = format!("{new}{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"review-summary-damaged:r1\"}}}},", image_store::media_type::MANIFEST, summary_image.digest, summary_image.size);
        index = index.replacen("\"manifests\":[", &format!("\"manifests\":[{new}"), 1);
        std::fs::write(index_path, index).unwrap();
        library.refresh(FloppyGeometry::HD_1_44, 0).unwrap();
        assert_eq!(library.rows.len(), 3);
        assert!(library
            .rows
            .iter()
            .any(|row| row.tag == "review-damaged:r1" && row.reason.is_some()));
        assert!(library
            .rows
            .iter()
            .any(|row| row.tag == "review-set:r1" && row.reason.is_none()));
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.root().join("locks/store.lock"))
            .unwrap();
        lock.lock().unwrap();
        library.refresh_async(FloppyGeometry::HD_1_44, 0);
        assert!(library.is_busy());
        let message = library
            .pending
            .take()
            .unwrap()
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        let LibraryMessage::Rows(Err(reason)) = message else {
            panic!("busy result")
        };
        assert!(reason.contains("store busy"));
    }
}
