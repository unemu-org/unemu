// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local images this release does not read (machine-images §5.6): the
//! library's machine images, installation sets and firmware packages, the
//! machines made from such an image and the media picker list them disabled
//! with "Needs unemupc X or newer", and offer nothing that reads their
//! config. Their details and Delete tag stay.

use super::firmware::{self, tests::query};
use super::install_media::{self, test_support::library};
use super::machines::test_support::row as machine_row;
use super::{snapshot, Filter, LibraryView, Query, Snapshot};
use egui::accesskit::Role;
use image_store::compat::level;
use pc_library::library_test_support::{tag_newer, warm};
use std::sync::Arc;

const NEEDS: &str = "Needs unemupc 99.0.0 or newer";
const SIZE: egui::Vec2 = egui::vec2(1400.0, 1000.0);
const DRIVE_A: [(u8, assembly::FloppyGeometry); 1] = [(0, assembly::FloppyGeometry::HD_1_44)];

/// What one frame paints, and each button with whether it takes a click.
struct Drawn {
    texts: Vec<String>,
    buttons: Vec<(String, bool)>,
}
impl Drawn {
    fn painted(&self, text: &str) -> bool {
        self.texts.iter().any(|t| t == text)
    }
    /// Whether the button `label` is enabled; it must be drawn.
    fn enabled(&self, label: &str) -> bool {
        self.buttons
            .iter()
            .find(|(shown, _)| shown == label)
            .unwrap_or_else(|| panic!("no button {label:?} in {:?}", self.buttons))
            .1
    }
}

fn draw(mut content: impl FnMut(&mut egui::Ui)) -> Drawn {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => (),
        }
    }
    let ctx = egui::Context::default();
    ui_kit::skin::configure_context(&ctx);
    ctx.enable_accesskit();
    let mut output = None;
    // The second frame lays the panes out from the first one's sizes.
    for _ in 0..2 {
        let mut frame = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE)),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| content(ui));
            },
        );
        frame.textures_delta.clear();
        output = Some(frame);
    }
    let output = output.unwrap();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut texts);
    }
    let buttons = output
        .platform_output
        .accesskit_update
        .as_ref()
        .map(|update| {
            update
                .nodes
                .iter()
                .filter(|(_, node)| node.role() == Role::Button)
                .map(|(_, node)| {
                    (
                        node.label().or(node.value()).unwrap_or_default().to_owned(),
                        !node.is_disabled(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Drawn { texts, buttons }
}

fn snapshot_of(store: &image_store::Store, filter: Filter) -> Snapshot {
    snapshot(
        &warm(store),
        &Query {
            filter,
            ..Default::default()
        },
    )
}

/// A machine image whose manifest needs a newer release, with or without a
/// config key this release refuses, is a summarised row with the reason, not
/// an unreadable one. Its pane says why, and Start, New machine from image…
/// and everything else that reads its config are disabled; Delete tag…
/// stays. An image at this release is unaffected.
#[test]
fn a_machine_image_that_needs_a_newer_release_is_disabled_with_the_reason() {
    let (f, _) = library();
    tag_newer(
        &f.store,
        "test:source",
        "newer:1.0.0",
        Some("99.0.0"),
        false,
    );
    tag_newer(
        &f.store,
        "test:source",
        "future:1.0.0",
        Some("99.0.0"),
        true,
    );
    tag_newer(
        &f.store,
        "test:source",
        "here:1.0.0",
        Some(&level().to_string()),
        false,
    );
    let result = Arc::new(snapshot_of(&f.store, Filter::All));
    let image = |tag: &str| {
        result
            .rows
            .iter()
            .find(|r| r.image.has_tag(tag))
            .unwrap_or_else(|| panic!("{tag}"))
            .image
            .clone()
    };
    for (tag, needs) in [
        ("newer:1.0.0", Some(NEEDS)),
        ("future:1.0.0", Some(NEEDS)),
        ("here:1.0.0", None),
    ] {
        let row = image(tag);
        assert_eq!(row.failure, None, "{tag}: not an unreadable row");
        assert_eq!(
            row.summary.as_ref().unwrap().needs_newer().as_deref(),
            needs,
            "{tag}"
        );
        let mut view = LibraryView::new(None);
        view.query.filter = Filter::All;
        view.snapshot = result.clone();
        view.selected = Some(row.clone());
        view.recheck();
        let drawn = draw(|ui| {
            view.show(ui, &[], &mut None);
        });
        match needs {
            Some(needs) => {
                assert!(
                    matches!(&view.readiness, Some(Err(r)) if r == needs),
                    "{tag}"
                );
                assert!(drawn.painted(needs), "{tag}: {:?}", drawn.texts);
                for label in [
                    "Start",
                    "New machine from image…",
                    "Gaming mode",
                    "Settings…",
                    "Layers…",
                    "Push…",
                ] {
                    assert!(!drawn.enabled(label), "{tag}: {label}");
                }
                // Refused also from the menus and keys that reach them.
                view.begin_new_from_image();
                assert_eq!(view.error.as_deref(), Some(needs));
                view.error = None;
                view.launch(crate::library_worker::WorkspaceKind::Regular);
                assert_eq!(view.error.as_deref(), Some(needs));
            }
            None => {
                assert!(view.readiness.is_none(), "{tag}: checked as usual");
                assert!(!drawn.painted(NEEDS), "{tag}");
                for label in ["New machine from image…", "Settings…", "Layers…", "Push…"] {
                    assert!(drawn.enabled(label), "{tag}: {label}");
                }
            }
        }
        // Details from the manifest and Delete tag… stay.
        assert!(drawn.enabled("Delete tag…"), "{tag}");
        assert!(drawn.painted("Machines from this image (0)"), "{tag}");
    }
}

/// A machine made from such an image is disabled with the same reason: the
/// ⚠ the list marks a machine that cannot start with, the reason in its
/// pane, and Start, Settings…, Insert media… and the rest that read the
/// image's config disabled. Rename and Delete machine… stay.
#[test]
fn a_machine_of_an_image_that_needs_a_newer_release_is_disabled_with_the_reason() {
    let mut machine = machine_row("newer", "Newer", "a", &["newer:1.0.0"]);
    machine.needs_newer = Some(NEEDS.into());
    machine.listing.problem = Some(NEEDS.into());
    let mut view = LibraryView::new(None);
    view.machines = Arc::new(vec![machine.clone()]);
    view.machine = Some("newer".into());
    view.selected = Some(machine.image_row());
    view.recheck();
    assert!(matches!(&view.readiness, Some(Err(r)) if r == NEEDS));
    let drawn = draw(|ui| {
        view.show(ui, &[], &mut None);
    });
    assert!(drawn.painted(&format!("⚠ {NEEDS}")), "{:?}", drawn.texts);
    for label in [
        "Start",
        "Gaming mode",
        "Settings…",
        "Insert media…",
        "Save as new image…",
        "Disk layers…",
    ] {
        assert!(!drawn.enabled(label), "{label}");
    }
    for label in ["Rename…", "Delete machine…", "Duplicate…"] {
        assert!(drawn.enabled(label), "{label}");
    }
    view.launch(crate::library_worker::WorkspaceKind::Machine);
    assert_eq!(view.error.as_deref(), Some(NEEDS));
    // Refused from the menus, keys and toolbar too, before the worker.
    let refused = |view: &mut LibraryView, act: &dyn Fn(&mut LibraryView)| {
        view.error = None;
        act(view);
        assert_eq!(view.error.as_deref(), Some(NEEDS));
    };
    refused(&mut view, &|v| v.open_settings());
    refused(&mut view, &|v| v.open_image_settings());
    refused(&mut view, &|v| v.open_layers());
    assert!(view.layers.is_none());
    refused(&mut view, &|v| v.begin_save_image());
    refused(&mut view, &|v| {
        v.set_pending(
            "newer",
            "A:",
            Some(image_store::machine::PendingInsert {
                drive: "A:".into(),
                set: super::machines::test_support::image("set"),
                reference: "set:r1".into(),
                medium: "disk-1".into(),
                all: false,
                journal: true,
            }),
        )
    });
    assert!(view.machines[0].listing.machine.pending.is_empty());

    // A machine of an image this release reads is unaffected.
    let plain = machine_row("plain", "Plain", "b", &["plain:1.0.0"]);
    view.machines = Arc::new(vec![plain.clone()]);
    view.machine = Some("plain".into());
    view.selected = Some(plain.image_row());
    view.error = None;
    view.recheck();
    assert!(view.readiness.is_none());
    let drawn = draw(|ui| {
        view.show(ui, &[], &mut None);
    });
    assert!(drawn.enabled("Settings…"));
    assert!(!drawn.painted(&format!("⚠ {NEEDS}")));
}

/// An installation set that needs a newer release says so in its
/// Availability, as a CD-ROM set does, and goes into no drive: Insert and
/// Insert into… are disabled, and the media picker lists it with the reason
/// and inserts nothing. Without its disks (a config key this release
/// refuses) it is still listed, not unreadable.
#[test]
fn an_installation_set_that_needs_a_newer_release_is_disabled_with_the_reason() {
    let (f, _) = library();
    let source = "example-adventure:r1";
    tag_newer(
        &f.store,
        source,
        "newer-adventure:r1",
        Some("99.0.0"),
        false,
    );
    tag_newer(
        &f.store,
        source,
        "future-adventure:r1",
        Some("99.0.0"),
        true,
    );
    let here = level().to_string();
    tag_newer(&f.store, source, "here-adventure:r1", Some(&here), false);
    let result = snapshot_of(&f.store, Filter::Media);
    let set = |tag: &str| {
        result
            .media_rows
            .iter()
            .find(|r| r.image.has_tag(tag))
            .unwrap_or_else(|| panic!("{tag}"))
            .clone()
    };
    let mut picker_sets = Vec::new();
    for (tag, needs, disks) in [
        ("newer-adventure:r1", Some(NEEDS), true),
        ("future-adventure:r1", Some(NEEDS), false),
        ("here-adventure:r1", None, true),
    ] {
        let row = set(tag);
        assert_eq!(row.image.failure, None, "{tag}: not an unreadable row");
        assert_eq!(row.set().is_some(), disks, "{tag}");
        let cells = install_media::cells(&row, &DRIVE_A);
        assert!(cells[0].starts_with("Example Adventure"), "{tag}");
        let drawn = draw(|ui| {
            install_media::details(ui, &row, &DRIVE_A, &[]);
        });
        let entry = install_media::picker_set(&row).expect("listed in the picker");
        match needs {
            Some(needs) => {
                assert_eq!(cells[5], needs, "{tag}");
                assert!(drawn.painted(needs), "{tag}: {:?}", drawn.texts);
                assert!(!drawn.enabled("Insert"), "{tag}");
                assert!(!drawn.enabled("Insert into… ▾"), "{tag}");
                assert_eq!(entry.problem.as_deref(), Some(needs), "{tag}");
                assert!(!entry.fits(assembly::FloppyGeometry::HD_1_44), "{tag}");
                picker_sets.push(entry);
            }
            None => {
                assert_eq!(cells[5], "Ready to insert into A:", "{tag}");
                assert!(drawn.enabled("Insert"), "{tag}");
                assert_eq!(entry.problem, None, "{tag}");
            }
        }
    }
    // The media picker: listed with the reason, and nothing is inserted.
    use media_ui::media_picker::{show, test_support::machine, Choice, Picker, When};
    for (index, entry) in picker_sets.iter().enumerate() {
        let mut picker = Picker::new(
            vec![machine(None, When::Now)],
            0,
            0,
            picker_sets.clone(),
            None,
        );
        picker.show_misfits = true;
        picker.choose(Choice::Set(index));
        assert_eq!(
            picker.problem().as_deref(),
            Some(NEEDS),
            "{}",
            entry.reference
        );
        assert!(picker.insert().is_none());
        let drawn = draw(|ui| {
            show(ui.ctx(), &mut picker, false);
        });
        assert!(drawn.painted(NEEDS), "{:?}", drawn.texts);
        assert!(drawn.painted(&format!("{NEEDS}.")), "{:?}", drawn.texts);
    }
}

/// A firmware package that needs a newer release says so in its Status;
/// Use in machine… and Edit metadata… are disabled with the reason, while
/// its details and Delete tag… stay. Without its config's details it is
/// still listed, not unreadable.
#[test]
fn a_firmware_package_that_needs_a_newer_release_is_disabled_with_the_reason() {
    let (f, cache) = firmware::tests::library();
    let first = snapshot(&cache, &query(Filter::Firmware, ""));
    let source = first.firmware_rows[0].image.reference.clone();
    let name = first.firmware_rows[0].image.name().to_owned();
    tag_newer(&f.store, &source, "newer-bios:r1", Some("99.0.0"), false);
    tag_newer(&f.store, &source, "future-bios:r1", Some("99.0.0"), true);
    let here = level().to_string();
    tag_newer(&f.store, &source, "here-bios:r1", Some(&here), false);
    let result = Arc::new(snapshot_of(&f.store, Filter::Firmware));
    for (tag, needs, details) in [
        ("newer-bios:r1", Some(NEEDS), true),
        ("future-bios:r1", Some(NEEDS), false),
        ("here-bios:r1", None, true),
    ] {
        let row = result
            .firmware_rows
            .iter()
            .find(|r| r.image.has_tag(tag))
            .unwrap_or_else(|| panic!("{tag}"))
            .clone();
        assert_eq!(row.image.failure, None, "{tag}: not an unreadable row");
        assert_eq!(row.firmware().is_some(), details, "{tag}");
        let cells = firmware::cells(&row);
        // Named by its manifest even without its config's details.
        assert_eq!(cells[0], name, "{tag}");
        let drawn = draw(|ui| {
            firmware::details(ui, &row, None, true, &[]);
        });
        let mut view = LibraryView::new(None);
        view.query.filter = Filter::Firmware;
        view.snapshot = result.clone();
        view.firmware_selected = Some(row.clone());
        match needs {
            Some(needs) => {
                assert_eq!(cells[3], needs, "{tag}");
                assert!(drawn.painted(needs), "{tag}: {:?}", drawn.texts);
                assert!(!drawn.enabled("Edit metadata…"), "{tag}");
                assert!(!view.can_edit_firmware(), "{tag}");
            }
            None => {
                assert_ne!(cells[3], NEEDS, "{tag}");
                assert!(drawn.enabled("Edit metadata…"), "{tag}");
            }
        }
        assert!(!drawn.enabled("Use in machine…"), "{tag}");
        assert!(view.can_delete_package(), "{tag}: Delete tag stays");
    }
}
