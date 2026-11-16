// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's lists and details panes drawn headless at the window widths
//! the approved mock-ups are checked at: nothing overlaps or is cut at an
//! edge, drives are named by their letters, raw configuration keys never
//! show, the text uses one body size and one smaller meta size, and the
//! actions are framed buttons in one wrapping row at the bottom.

use super::*;
use crate::library_test_support::Fixture;
use pc_image::machine::{Change, State};

/// The window widths every layout is checked at; at 760 px the details
/// pane is narrow enough that its sections stack.
const WINDOW_WIDTHS: [f32; 4] = [760.0, 1000.0, 1400.0, 2000.0];
/// The library tree's fixed width beside the lists and the details pane.
const TREE_WIDTH: f32 = 230.0;
/// The smallest text the panes may draw: the mock-ups' 11 px meta text.
const SMALLEST_TEXT: f32 = 11.0;

/// Text as egui painted it: what, where, the visible area and its sizes.
#[derive(Clone, Debug)]
struct Drawn {
    text: String,
    rect: egui::Rect,
    /// Each laid-out row's glyph area. A label continuing a wrapped row
    /// starts after its leading space, so rows, not the whole galley, tell
    /// whether two texts overlap.
    rows: Vec<egui::Rect>,
    clip: egui::Rect,
    sizes: Vec<f32>,
    /// The galley keeps the whole text; a truncated one lays out an ellipsis
    /// the text does not have.
    truncated: bool,
}
impl Drawn {
    fn visible(&self) -> egui::Rect {
        self.rect.intersect(self.clip)
    }
}

/// Every text and filled or outlined rectangle of a frame.
fn drawn(output: &egui::FullOutput) -> (Vec<Drawn>, Vec<egui::epaint::RectShape>) {
    fn walk(
        shape: &egui::Shape,
        clip: egui::Rect,
        texts: &mut Vec<Drawn>,
        rects: &mut Vec<egui::epaint::RectShape>,
    ) {
        match shape {
            egui::Shape::Text(text) => {
                if text.galley.text().trim().is_empty() {
                    return;
                }
                let offset = text.pos.to_vec2();
                let rows: Vec<egui::Rect> = text
                    .galley
                    .rows
                    .iter()
                    .map(|row| row.rect_without_leading_space().translate(offset))
                    .filter(|rect| rect.width() > 0.0)
                    .collect();
                let shown: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|g| g.chr))
                    .collect();
                let ellipses = |t: &str| t.matches('…').count();
                texts.push(Drawn {
                    truncated: ellipses(&shown) > ellipses(text.galley.text()),
                    text: text.galley.text().to_owned(),
                    rect: rows
                        .iter()
                        .fold(egui::Rect::NOTHING, |all, row| all.union(*row)),
                    rows,
                    clip,
                    sizes: text
                        .galley
                        .job
                        .sections
                        .iter()
                        .map(|s| s.format.font_id.size)
                        .collect(),
                });
            }
            egui::Shape::Rect(rect) => rects.push(rect.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, clip, texts, rects)),
            _ => (),
        }
    }
    let mut texts = Vec::new();
    let mut rects = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut texts, &mut rects);
    }
    (texts, rects)
}

/// Two frames of `draw` in a window `width` wide and tall enough for a
/// whole pane; the second frame, laid out from the first, is returned.
fn frames(width: f32, mut draw: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
    let ctx = egui::Context::default();
    crate::shell::configure_context(&ctx);
    let size = egui::vec2(width, 2400.0);
    firmware::tests::frame(&ctx, size, 0.0, vec![], &mut draw);
    firmware::tests::frame(&ctx, size, 1.0, vec![], &mut draw)
}

/// The details pane alone, beside an empty tree as wide as the library's.
fn pane(view: &mut LibraryView, width: f32) -> (Vec<Drawn>, Vec<egui::epaint::RectShape>) {
    let output = frames(width, |ui| {
        egui::Panel::left("tree")
            .exact_size(TREE_WIDTH)
            .show(ui, |_| {});
        egui::CentralPanel::default().show(ui, |ui| {
            let mut commands = Vec::new();
            view.details_pane(ui, &[], &mut None, &mut commands);
        });
    });
    drawn(&output)
}

/// The whole library view.
fn library(view: &mut LibraryView, width: f32) -> Vec<Drawn> {
    let output = frames(width, |ui| {
        view.show(ui, &[], &mut None);
    });
    drawn(&output).0
}

/// No two visible texts overlap, none is cut at its area's side and none is
/// truncated.
fn assert_laid_out(what: &str, width: f32, texts: &[Drawn]) {
    let visible: Vec<&Drawn> = texts.iter().filter(|t| t.visible().is_positive()).collect();
    assert!(!visible.is_empty(), "{what} at {width}: nothing drawn");
    for text in &visible {
        assert!(
            !text.truncated,
            "{what} at {width}: {:?} is truncated",
            text.text
        );
        assert!(
            text.rect.right() <= text.clip.right() + 1.0
                && text.rect.left() >= text.clip.left() - 1.0,
            "{what} at {width}: {:?} is cut at the side ({:?} in {:?})",
            text.text,
            text.rect,
            text.clip
        );
    }
    let overlap = |a: &Drawn, b: &Drawn| {
        a.rows.iter().any(|ra| {
            b.rows.iter().any(|rb| {
                let both = ra.intersect(a.clip).intersect(rb.intersect(b.clip));
                both.width() > 1.0 && both.height() > 1.0
            })
        })
    };
    for (i, a) in visible.iter().enumerate() {
        for b in &visible[i + 1..] {
            assert!(
                !overlap(a, b),
                "{what} at {width}: {:?} {:?} overlaps {:?} {:?}",
                a.text,
                a.rect,
                b.text,
                b.rect
            );
        }
    }
}

/// One body size and one smaller meta size, plus the title's.
fn assert_type_sizes(what: &str, texts: &[Drawn]) {
    let mut sizes: Vec<f32> = texts.iter().flat_map(|t| t.sizes.clone()).collect();
    sizes.sort_by(f32::total_cmp);
    sizes.dedup();
    let tiny: Vec<&String> = texts
        .iter()
        .filter(|t| t.sizes.iter().any(|s| *s < SMALLEST_TEXT))
        .map(|t| &t.text)
        .collect();
    assert!(
        tiny.is_empty(),
        "{what}: text below {SMALLEST_TEXT} pt: {tiny:?}"
    );
    assert!(sizes.len() <= 3, "{what}: sizes {sizes:?}");
}

/// Raw configuration keys and internal ids never reach the panes.
fn assert_no_raw_ids(what: &str, texts: &[Drawn]) {
    for text in texts {
        for raw in [
            "firmware.bios",
            "option_roms[",
            "ide:0",
            "fdc-765:",
            "medium-0",
            "medium-1",
            "Board: ibm-5170",
            "zero base 0",
            "→ working",
        ] {
            assert!(
                !text.text.contains(raw),
                "{what}: {raw:?} drawn in {:?}",
                text.text
            );
        }
    }
}

fn at<'a>(texts: &'a [Drawn], wanted: &str) -> &'a Drawn {
    texts.iter().find(|t| t.text == wanted).unwrap_or_else(|| {
        panic!(
            "{wanted:?} not drawn in {:?}",
            texts.iter().map(|t| &t.text).collect::<Vec<_>>()
        )
    })
}

/// The test machine image with its firmware packages in the library and its
/// row as the Images collection lists it.
fn library_with_image() -> (Fixture, SummaryCache, ImageRow) {
    let (f, cache) = firmware::tests::library();
    let rows = snapshot(&cache, &Query::default());
    let image = rows
        .rows
        .iter()
        .find(|r| r.image.reference == "test:source")
        .expect("the test image is listed")
        .image
        .clone();
    (f, cache, image)
}

/// A machine of the test image with labels, a favourite star, C: and CMOS
/// changed and a pending insert, selected in the Machines collection.
fn machine_view() -> (Fixture, LibraryView) {
    let (f, cache, image) = library_with_image();
    let mut view = LibraryView::new(None);
    view.snapshot = Arc::new(snapshot(&cache, &view.query));
    let mut machine = machines::tests::row("dos-games", "DOS games", "unused", &["test:source"]);
    machine.listing.machine.image = image.descriptor.clone();
    machine.summary = image.summary.clone();
    machine.title = Some("Test machine".into());
    machine.listing.machine.favorite = true;
    machine.listing.machine.labels = vec!["games".into(), "testing".into()];
    machine.listing.state = State::Changed {
        drives: vec!["C:".into()],
        inserted: false,
        cmos: true,
        settings: false,
    };
    machine.listing.changes = vec![Change {
        drive: "C:".into(),
        medium: "medium-2".into(),
        sectors: Some(61030),
    }];
    machine.listing.change_bytes = 31_247_360;
    machine
        .listing
        .machine
        .pending
        .push(image_store::machine::PendingInsert {
            drive: "A:".into(),
            set: machines::tests::image("set"),
            reference: "keen4:1.4".into(),
            medium: "disk-1".into(),
            all: false,
            journal: true,
        });
    view.set_machines(vec![machine]);
    view.query.filter = Filter::Machines;
    view.machine = Some("dos-games".into());
    view.machine_labels = "games, testing".into();
    view.machine_settings = Some(("dos-games".into(), Some(Ok(Vec::new()))));
    view.selected = Some(image.clone());
    view.on_event(crate::library_worker::checked(&f.store, &image, None));
    assert!(
        matches!(view.readiness, Some(Ok(_))),
        "{:?}",
        view.readiness
    );
    (f, view)
}

/// The test image selected in the Images collection, with one machine.
fn image_view() -> (Fixture, LibraryView) {
    let (f, mut view) = machine_view();
    view.query.filter = Filter::All;
    view.machine = None;
    let image = view.selected.clone().unwrap();
    view.labels = "template".into();
    view.on_event(crate::library_worker::checked(&f.store, &image, None));
    // The manifest's description and base, as a saved image records them;
    // a base no tag names shows the start of its digest.
    let (_, checked) = view.checked.as_mut().expect("the check's reading");
    checked.description = Some("A machine for the layout tests.".into());
    checked.base = Some(Digest::of(b"the image this one was saved from"));
    (f, view)
}

/// The machine pane at every width: nothing overlaps, every drive row names
/// its letter in the drive table with its header, no raw key is drawn and
/// the type keeps to its sizes.
#[test]
fn the_machine_pane_lays_out_at_every_width_with_named_drives() {
    let (_f, mut view) = machine_view();
    for width in WINDOW_WIDTHS {
        let (texts, _) = pane(&mut view, width);
        assert_laid_out("machine pane", width, &texts);
        assert_no_raw_ids("machine pane", &texts);
        assert_type_sizes("machine pane", &texts);
        // The drive table: its header on one row, then A:, C: and CMOS.
        let header = [
            at(&texts, "Drive"),
            at(&texts, "In the image"),
            at(&texts, "In this machine"),
        ];
        assert!(
            header
                .iter()
                .all(|h| (h.rect.top() - header[0].rect.top()).abs() < 1.0),
            "{header:?}"
        );
        let rows = [at(&texts, "A:"), at(&texts, "C:"), at(&texts, "CMOS")];
        assert!(header[0].rect.bottom() <= rows[0].rect.top());
        assert!(rows
            .windows(2)
            .all(|w| w[0].rect.bottom() <= w[1].rect.top()));
        for row in rows {
            assert!(
                (row.rect.left() - header[0].rect.left()).abs() < 1.0,
                "{row:?}"
            );
        }
        assert!(texts
            .iter()
            .any(|t| t.text == "61\u{a0}030 sectors written · 29.8 MiB"));
        // A narrow pane stacks its sections: the settings come under the
        // drive table.
        if width < 1000.0 {
            assert!(
                at(&texts, "Settings that differ from the image").rect.top()
                    > at(&texts, "CMOS").rect.bottom()
            );
        }
        assert!(texts.iter().any(|t| t.text.starts_with("Pending: ")));
        for section in ["Media per drive", "Firmware", "Hardware, from the image"] {
            at(&texts, section);
        }
    }
}

/// The machine's actions are framed buttons in one wrapping row below the
/// facts and sections, all on one line on a wide window.
#[test]
fn machine_actions_are_framed_buttons_in_one_row_at_the_bottom() {
    let (_f, mut view) = machine_view();
    let actions = [
        "Start",
        "Gaming mode",
        "Settings…",
        "Insert media…",
        "Save as new image…",
        "Disk layers…",
        "Duplicate…",
        "Rename…",
        "Reset to image…",
        "Delete machine…",
    ];
    for width in WINDOW_WIDTHS {
        let (texts, rects) = pane(&mut view, width);
        let buttons: Vec<&Drawn> = actions.iter().map(|a| at(&texts, a)).collect();
        let sections = ["Media per drive", "CMOS", "Hardware, from the image"];
        let lowest = sections
            .iter()
            .map(|s| at(&texts, s).rect.bottom())
            .fold(f32::MIN, f32::max);
        for button in &buttons {
            assert!(
                button.rect.top() >= lowest,
                "at {width}: {:?} above the sections",
                button.text
            );
            assert!(
                rects.iter().any(|r| r.rect.contains_rect(button.rect)
                    && (r.stroke.width > 0.0 && r.stroke.color.a() > 0)),
                "at {width}: {:?} has no frame",
                button.text
            );
        }
        // Start is the default action: its frame is heavier than the others'.
        let frame = |button: &Drawn| {
            rects
                .iter()
                .filter(|r| r.rect.contains_rect(button.rect) && r.stroke.width > 0.0)
                .map(|r| r.stroke.width)
                .fold(0.0, f32::max)
        };
        assert!(
            frame(buttons[0]) > frame(buttons[1]),
            "at {width}: Start is not the default action"
        );
        if width >= 1400.0 {
            assert!(
                buttons
                    .iter()
                    .all(|b| (b.rect.top() - buttons[0].rect.top()).abs() < 1.0),
                "at {width}: the actions wrap"
            );
        }
    }
}

/// The image pane at every width: its sections, the drives by letter, the
/// firmware by name, the machines made from it and no raw key.
#[test]
fn the_image_pane_lays_out_at_every_width_with_its_sections() {
    let (_f, mut view) = image_view();
    for width in WINDOW_WIDTHS {
        let (texts, _) = pane(&mut view, width);
        assert_laid_out("image pane", width, &texts);
        assert_no_raw_ids("image pane", &texts);
        assert_type_sizes("image pane", &texts);
        for section in [
            "Hardware",
            "Drives",
            "Base image",
            "Firmware",
            "Machines from this image (1)",
        ] {
            at(&texts, section);
        }
        for drive in ["A:", "C:", "CMOS"] {
            at(&texts, drive);
        }
        at(&texts, "DOS games");
        assert!(texts.iter().any(|t| t.text == "System BIOS"));
        at(&texts, "A machine for the layout tests.");
        let base = Digest::of(b"the image this one was saved from");
        at(
            &texts,
            &format!("sha256:{}", pane::short_digest(base.hex())),
        );
    }
}

/// The firmware and installation media panes keep the same rules.
#[test]
fn the_firmware_and_media_panes_lay_out_at_every_width() {
    let (_f, cache) = firmware::tests::library();
    let mut view = LibraryView::new(None);
    view.select_first_package(&cache, &mut None);
    for width in WINDOW_WIDTHS {
        let (texts, _) = pane(&mut view, width);
        assert_laid_out("firmware pane", width, &texts);
        assert_type_sizes("firmware pane", &texts);
        assert_headers_fit(
            "Firmware",
            width,
            &library(&mut view, width),
            &[
                "Name",
                "Role summary",
                "Fits",
                "Version / date",
                "Redistribution",
            ],
        );
    }
    let (_m, cache) = install_media::tests::library();
    let mut view = LibraryView::new(None);
    view.set_filter(Filter::Media, &mut None);
    view.snapshot = Arc::new(snapshot(&cache, &view.query));
    let row = view.snapshot.media_rows[0].clone();
    view.select_media(&row, &mut None);
    for width in WINDOW_WIDTHS {
        let (texts, _) = pane(&mut view, width);
        assert_laid_out("media pane", width, &texts);
        assert_type_sizes("media pane", &texts);
        assert_headers_fit(
            "Installation media",
            width,
            &library(&mut view, width),
            &["Set", "Tag", "Platform", "Format", "Availability"],
        );
    }
}

/// The list's column headers, on the row of its first header, are all drawn
/// whole inside the list's visible area.
fn assert_headers_fit(what: &str, width: f32, texts: &[Drawn], headers: &[&str]) {
    let name = texts
        .iter()
        .filter(|t| t.text.starts_with(headers[0]) && t.rect.left() >= TREE_WIDTH)
        .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
        .unwrap_or_else(|| panic!("{what} at {width}: no {:?} column", headers[0]));
    for header in headers {
        let drawn = texts
            .iter()
            .find(|t| t.text.starts_with(header) && (t.rect.top() - name.rect.top()).abs() < 2.0)
            .unwrap_or_else(|| panic!("{what} at {width}: no {header:?} column"));
        assert!(
            drawn.rect.right() <= drawn.clip.right() + 1.0 && drawn.visible().is_positive(),
            "{what} at {width}: {header:?} drawn beyond the list ({:?} in {:?})",
            drawn.rect,
            drawn.clip
        );
        assert!(
            !drawn.truncated,
            "{what} at {width}: {:?} is truncated",
            drawn.text
        );
    }
}

/// Every drawn text of `wanted` is whole: counts, sizes and marks never
/// truncate in a list.
fn assert_whole(what: &str, width: f32, texts: &[Drawn], wanted: &[&str]) {
    for want in wanted {
        let found: Vec<&Drawn> = texts.iter().filter(|t| t.text == *want).collect();
        assert!(!found.is_empty(), "{what} at {width}: no {want:?}");
        for text in found {
            assert!(!text.truncated, "{what} at {width}: {want:?} is truncated");
            assert!(
                text.rect.right() <= text.clip.right() + 1.0,
                "{what} at {width}: {want:?} drawn beyond the list"
            );
        }
    }
}

/// The Machines and Images lists draw every column inside their area at
/// every width, and the Images list groups a repository's images under one
/// row with the mock-up's columns.
#[test]
fn lists_fit_their_area_and_images_group_by_repository() {
    let (_f, mut view) = machine_view();
    for width in WINDOW_WIDTHS {
        let texts = library(&mut view, width);
        assert_headers_fit(
            "Machines",
            width,
            &texts,
            &["Name", "Source image", "State", "Last run", "Changes"],
        );
        assert_whole("Machines", width, &texts, &["29.8 MiB"]);
    }
    let (_temp, store, images) = tests::catalogue(3);
    store
        .update_tag("synthetic-0:two", &images[2], None, &tests::Provider)
        .unwrap();
    store
        .remove_tag("synthetic-2:one", &images[2].digest)
        .unwrap();
    let cache = firmware::tests::warm(&store);
    let mut view = LibraryView::new(None);
    view.query.filter = Filter::All;
    view.query.expanded.insert("synthetic-0".into());
    view.snapshot = Arc::new(snapshot(&cache, &view.query));
    for width in WINDOW_WIDTHS {
        let texts = library(&mut view, width);
        assert_headers_fit(
            "Images",
            width,
            &texts,
            &[
                "Name",
                "Tags",
                "Digest",
                "Machines",
                "Redistribution",
                "Size",
            ],
        );
        assert!(
            texts
                .iter()
                .any(|t| t.text.contains("synthetic-0 · 2 images")),
            "Images at {width}: no group row"
        );
        assert_whole("Images", width, &texts, &["user-supplied", "0"]);
    }
}

/// The machine pane's settings that differ from the image are a table with
/// its header and one row per setting, laid out at every width.
#[test]
fn a_machine_with_its_own_settings_shows_them_as_a_table() {
    let (_f, mut view) = machine_view();
    view.machine_settings = Some((
        "dos-games".into(),
        Some(Ok(vec![(
            "Extended memory".into(),
            "3072".into(),
            "0".into(),
        )])),
    ));
    for width in WINDOW_WIDTHS {
        let (texts, _) = pane(&mut view, width);
        assert_laid_out("machine settings", width, &texts);
        let header = [
            at(&texts, "Setting"),
            at(&texts, "This machine"),
            at(&texts, "Image"),
        ];
        let row = at(&texts, "Extended memory");
        assert!(header
            .iter()
            .all(|h| (h.rect.top() - header[0].rect.top()).abs() < 1.0));
        assert!(row.rect.top() >= header[0].rect.bottom());
        at(&texts, "3072");
    }
}

/// An image with an open BIOS and a user-supplied disk is user-supplied as a
/// whole, as the push guard judges it, though its BIOS alone is open; with
/// every layer open, the image is open.
#[test]
fn an_open_rom_with_a_user_supplied_disk_makes_the_image_user_supplied() {
    use image_store::{license, media_type as mt};
    let (_f, _cache, mut image) = library_with_image();
    let mit = license::Expression::parse("MIT").unwrap();
    let summary = image.summary.as_mut().unwrap();
    assert!(
        summary
            .storage_blobs
            .iter()
            .any(|d| d.media_type != mt::MANIFEST && !firmware::is_rom(d)),
        "the image has disks and a configuration"
    );
    for d in &mut summary.storage_blobs {
        let open = firmware::is_rom(d) || d.media_type == mt::CONFIG;
        license::mark(d, open.then_some(&mit));
    }
    assert!(
        firmware::Redistribution::of(summary).open,
        "the BIOS alone is open"
    );
    assert!(!image_rights(summary).open);
    assert_eq!(redistribution(summary), "user-supplied");
    let row = ViewRow {
        image: image.clone(),
        group: None,
        child: false,
    };
    assert_eq!(row_cells(&row, 0)[4], "user-supplied");
    let summary = image.summary.as_mut().unwrap();
    for d in &mut summary.storage_blobs {
        license::mark(d, Some(&mit));
    }
    assert!(image_rights(summary).open);
    assert_eq!(redistribution(summary), "open");
}
