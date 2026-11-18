// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's lists and details panes drawn headless at the window widths
//! the approved mock-ups are checked at: nothing overlaps or is cut at an
//! edge, drives are named by their letters, raw configuration keys never
//! show, the text uses one body size and one smaller meta size, and the
//! actions are framed buttons in one wrapping row at the bottom.

use super::*;
use crate::library_test_support::Fixture;
use image_store::summary::SummaryCache;
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
    /// What each laid-out row shows.
    lines: Vec<String>,
    /// The colour of the first section, as painted.
    colour: egui::Color32,
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
                let section = text
                    .galley
                    .job
                    .sections
                    .first()
                    .map_or(egui::Color32::PLACEHOLDER, |s| s.format.color);
                texts.push(Drawn {
                    lines: text
                        .galley
                        .rows
                        .iter()
                        .map(|row| row.glyphs.iter().map(|g| g.chr).collect())
                        .collect(),
                    colour: text.override_text_color.unwrap_or(
                        if section == egui::Color32::PLACEHOLDER {
                            text.fallback_color
                        } else {
                            section
                        },
                    ),
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
    ui_kit::skin::configure_context(&ctx);
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
        // With the Source column the list scrolls sideways on the narrowest
        // window, as the mock-up's grid does; its leading columns show whole.
        let headers = [
            "Name",
            "Source",
            "Role",
            "Status",
            "Fits",
            "Version / date",
            "Redistribution",
        ];
        let shown = if width < 1000.0 { 3 } else { headers.len() };
        assert_headers_fit(
            "Firmware",
            width,
            &library(&mut view, width),
            &headers[..shown],
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
        let headers = ["Set", "Source", "Tag", "Platform", "Format", "Availability"];
        let shown = if width < 1000.0 { 3 } else { headers.len() };
        assert_headers_fit(
            "Installation media",
            width,
            &library(&mut view, width),
            &headers[..shown],
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
/// row with the mock-up's columns. On the narrowest window the Images list,
/// with the remotes mock-up's Source column, scrolls sideways in its own
/// area, as the mock-up's grid does: its leading columns show whole.
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
        let headers = [
            "Name",
            "Source",
            "Tags",
            "Digest",
            "Machines",
            "Redistribution",
            "Size",
        ];
        let shown = if width < 1000.0 { 5 } else { headers.len() };
        assert_headers_fit("Images", width, &texts, &headers[..shown]);
        assert!(
            texts
                .iter()
                .any(|t| t.text.contains("synthetic-0 · 2 images")),
            "Images at {width}: no group row"
        );
        assert_whole("Images", width, &texts, &["—", "0"]);
        if width >= 1000.0 {
            assert_whole("Images", width, &texts, &["user-supplied"]);
        }
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
    assert_eq!(row_cells(&row, 0)[5], "user-supplied");
    let summary = image.summary.as_mut().unwrap();
    for d in &mut summary.storage_blobs {
        license::mark(d, Some(&mit));
    }
    assert!(image_rights(summary).open);
    assert_eq!(redistribution(summary), "open");
}

/// The mock-up's chg-bg, the tint of a changed row, light and dark.
const CHANGED_TINT: [egui::Color32; 2] = [
    egui::Color32::from_rgb(0xff, 0xf8, 0xe0),
    egui::Color32::from_rgb(0x3a, 0x33, 0x24),
];
/// The mock-up's face, the rule under every row of a mini table.
const FACE: [egui::Color32; 2] = [
    egui::Color32::from_rgb(0xd6, 0xd3, 0xcb),
    egui::Color32::from_rgb(0x34, 0x36, 0x3a),
];
/// The mock-up's field, the ground of text fields and mini tables.
const FIELD: [egui::Color32; 2] = [
    egui::Color32::from_rgb(0xff, 0xff, 0xff),
    egui::Color32::from_rgb(0x26, 0x28, 0x2b),
];

/// The mini table's test rows: four drives, the third changed, with a long
/// head, under the Media per drive header.
const TABLE_HEADS: [&str; 4] = ["A:", "B:", "Extended memory", "D:"];
const TABLE_HEADER: [&str; 3] = ["Drive", "In the image", "In this machine"];

/// A [`pane::table`] of the test rows `width` wide in the modern skin, light
/// or dark, as drawn on the second frame, with its field-coloured ground.
fn mini_table(
    dark: bool,
    width: f32,
) -> (
    Vec<Drawn>,
    Vec<egui::epaint::RectShape>,
    egui::epaint::RectShape,
) {
    let rows: Vec<pane::Row> = TABLE_HEADS
        .iter()
        .enumerate()
        .map(|(i, head)| pane::Row {
            head: (*head).into(),
            cells: vec![
                vec![("1.44M · a disk in the drive".into(), pane::Tone::Body)],
                vec![("As in the image".into(), pane::Tone::Muted)],
            ],
            changed: i == 2,
        })
        .collect();
    let ctx = egui::Context::default();
    ui_kit::skin::configure_context(&ctx);
    ctx.set_visuals(ui_kit::skin::skin_visuals(false, dark));
    let mut draw = |ui: &mut egui::Ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let layout = egui::Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(egui::vec2(width, 0.0), layout, |ui| {
                pane::table(ui, "rules", &TABLE_HEADER, &rows);
            });
        });
    };
    let size = egui::vec2(800.0, 600.0);
    firmware::tests::frame(&ctx, size, 0.0, vec![], &mut draw);
    let output = firmware::tests::frame(&ctx, size, 1.0, vec![], &mut draw);
    let (texts, rects) = drawn(&output);
    let ground = rects
        .iter()
        .find(|r| r.fill == FIELD[usize::from(dark)])
        .unwrap_or_else(|| panic!("dark {dark}: no field-coloured table in {rects:?}"))
        .clone();
    (texts, rects, ground)
}

/// The mini table as the mock-up's `.mini`: the field colour inside a thin
/// outline, a muted header, a first column in the strong text colour (egui
/// has no bold face loaded) that does not wrap while it fits, a 1 px
/// face-coloured rule under the header and under every row instead of
/// stripes, and a changed row tinted across the whole table.
#[test]
fn the_mini_table_rules_every_row_and_tints_changed_ones_without_stripes() {
    let heads = TABLE_HEADS;
    let header = TABLE_HEADER;
    for dark in [false, true] {
        let theme = usize::from(dark);
        let visuals = ui_kit::skin::skin_visuals(false, dark);
        let (texts, rects, ground) = mini_table(dark, 420.0);
        // The table's ground: the field colour in an outline, holding every cell.
        assert!(
            ground.stroke.width >= 1.0 && ground.stroke.color.a() > 0,
            "dark {dark}: the table has no outline: {ground:?}"
        );
        for text in &texts {
            assert!(
                ground.rect.contains_rect(text.rect),
                "dark {dark}: {:?} outside the table",
                text.text
            );
        }
        // One rule under the header and under each row, across the table.
        let rules: Vec<&egui::epaint::RectShape> = rects
            .iter()
            .filter(|r| r.fill == FACE[theme] && r.rect.height() <= 1.5)
            .collect();
        assert_eq!(
            rules.len(),
            heads.len() + 1,
            "dark {dark}: rules {rules:?} in {rects:?}"
        );
        let lines: Vec<&Drawn> = std::iter::once(header[0])
            .chain(heads)
            .map(|t| at(&texts, t))
            .collect();
        for (rule, line) in rules.iter().zip(&lines) {
            assert!(
                rule.rect.width() >= ground.rect.width() - 4.0,
                "dark {dark}: a rule short of the table: {rule:?}"
            );
            assert!(
                rule.rect.top() >= line.rect.bottom(),
                "dark {dark}: {line:?}"
            );
        }
        for (rule, next) in rules.iter().zip(&lines[1..]) {
            assert!(
                rule.rect.bottom() <= next.rect.top(),
                "dark {dark}: {next:?}"
            );
        }
        // No stripes: the only fill between the rules is the changed row's.
        assert!(
            !rects
                .iter()
                .any(|r| r.fill == visuals.faint_bg_color && ground.rect.intersects(r.rect)),
            "dark {dark}: a striped row"
        );
        let tint: Vec<&egui::epaint::RectShape> = rects
            .iter()
            .filter(|r| r.fill == CHANGED_TINT[theme])
            .collect();
        assert_eq!(tint.len(), 1, "dark {dark}: {rects:?}");
        let changed = at(&texts, "Extended memory");
        assert!(tint[0].rect.contains_rect(changed.rect));
        assert!(tint[0].rect.width() >= ground.rect.width() - 4.0);
        // A muted header, a strong first column on one line.
        let weak = visuals.weak_text_color();
        for label in header {
            assert_eq!(at(&texts, label).colour, weak, "dark {dark}: {label}");
        }
        for head in heads {
            let head = at(&texts, head);
            assert_eq!(head.colour, visuals.strong_text_color(), "{head:?}");
            assert_eq!(head.lines.len(), 1, "dark {dark}: {head:?} wraps");
        }
    }
}

/// On a narrow pane the first column takes at most its share and wraps, so
/// every cell, rule and tint stays inside the table's outline, whole.
#[test]
fn a_narrow_mini_table_keeps_every_cell_inside_its_outline() {
    for width in [200.0, 260.0] {
        for dark in [false, true] {
            let what = format!("{width} px, dark {dark}");
            let (texts, rects, ground) = mini_table(dark, width);
            assert!(ground.rect.width() <= width + 0.5, "{what}: {ground:?}");
            let inside = ground.rect.shrink(ground.stroke.width - 0.5);
            for label in TABLE_HEADER.iter().chain(&TABLE_HEADS) {
                at(&texts, label);
            }
            for text in &texts {
                assert!(!text.truncated, "{what}: {:?} truncated", text.text);
                assert!(
                    inside.contains_rect(text.rect),
                    "{what}: {:?} at {:?} outside the outline {inside:?}",
                    text.text,
                    text.rect
                );
                assert!(whole(text), "{what}: {:?} cut", text.text);
            }
            // The rules are 1 px lines in the face colour; the panel behind
            // the table is face-coloured too in every skin.
            for rect in rects.iter().filter(|r| {
                (r.fill == FACE[usize::from(dark)] && r.rect.height() <= 1.5)
                    || r.fill == CHANGED_TINT[usize::from(dark)]
            }) {
                assert!(
                    ground.rect.contains_rect(rect.rect),
                    "{what}: {rect:?} outside the outline {ground:?}"
                );
            }
        }
    }
}

/// A pane's buttons and fields keep one outline colour with the rest of the
/// window: the modern skin's own shadow-coloured outline stays, and only
/// the classic skin, which outlines no resting control, gets the pane's
/// outline from its text colour.
#[test]
fn pane_controls_keep_the_skins_outline() {
    for classic in [false, true] {
        for dark in [false, true] {
            let visuals = ui_kit::skin::skin_visuals(classic, dark);
            let expected = if classic {
                egui::Stroke::new(
                    1.0,
                    visuals
                        .widgets
                        .noninteractive
                        .fg_stroke
                        .color
                        .gamma_multiply(0.45),
                )
            } else {
                visuals.widgets.inactive.bg_stroke
            };
            let ctx = egui::Context::default();
            ctx.set_visuals(visuals);
            let mut outline = egui::Stroke::NONE;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                pane::style(ui);
                outline = ui.visuals().widgets.inactive.bg_stroke;
            });
            output.textures_delta.clear();
            assert_eq!(outline, expected, "classic {classic}, dark {dark}");
        }
    }
}

/// A repository's group row keeps the mock-up's second face colour as its
/// ground, apart from the list's stripes.
#[test]
fn group_rows_keep_their_own_ground_apart_from_the_stripes() {
    let ground = [
        egui::Color32::from_rgb(0xe7, 0xe4, 0xdc),
        egui::Color32::from_rgb(0x3e, 0x40, 0x45),
    ];
    for dark in [false, true] {
        let visuals = ui_kit::skin::skin_visuals(false, dark);
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        ctx.set_visuals(visuals.clone());
        let output = firmware::tests::frame(&ctx, egui::vec2(600.0, 200.0), 0.0, vec![], |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                group_row(ui, true, "synthetic-0", "2 images", 300.0);
            });
        });
        let (_, rects) = drawn(&output);
        let row = rects
            .iter()
            .find(|r| r.rect.width() == 300.0)
            .unwrap_or_else(|| panic!("dark {dark}: no group row in {rects:?}"));
        assert_eq!(row.fill, ground[usize::from(dark)], "dark {dark}");
        assert_ne!(row.fill, visuals.faint_bg_color, "dark {dark}");
    }
}

/// The kind tile wraps its title between words, as the mock-up's cover
/// does; only a word wider than the tile breaks inside, and nothing is cut.
#[test]
fn the_kind_tile_wraps_its_title_between_words() {
    for title in [
        "Open AT 286",
        "Open AT 286 for games",
        "Supercalifragilistic",
    ] {
        let output = frames(400.0, |ui| {
            pane::head(
                ui,
                &pane::Tile {
                    title,
                    kind: "MACHINE",
                    user_supplied: false,
                },
                |_| {},
            );
        });
        let (texts, _) = drawn(&output);
        let tile = at(&texts, title);
        assert!(
            tile.lines.len() > 1,
            "{title:?} on one row: {:?}",
            tile.lines
        );
        assert!(!tile.truncated, "{tile:?}");
        if title == "Open AT 286" {
            let rows: Vec<&str> = tile.lines.iter().map(|l| l.trim()).collect();
            assert_eq!(rows, ["Open AT", "286"], "{title:?}");
        }
        if title.contains(' ') {
            let words: Vec<&str> = tile
                .lines
                .iter()
                .flat_map(|l| l.split_whitespace())
                .collect();
            assert_eq!(
                words,
                title.split(' ').collect::<Vec<_>>(),
                "{title:?} breaks inside a word: {:?}",
                tile.lines
            );
        } else {
            assert_eq!(tile.lines.concat(), title);
        }
    }
}

/// The note under the machine pane's actions.
const MACHINE_NOTE: &str =
    "Gaming mode plays a disposable copy of this machine as it is now; this \
                            machine does not change. Reset to image… throws its changes away and \
                            keeps the machine; Delete machine… removes both, never the image.";

/// The note under the installation media pane's actions.
const MEDIA_NOTE: &str = "Stored media and extras stay read-only. Installer writes use the \
                          selected workspace's journal.";

/// The window height the footer checks use, where the details pane opens at
/// its first height.
const WINDOW_HEIGHT: f32 = 1000.0;

/// The smallest window the shell allows.
const MIN_WINDOW: egui::Vec2 = egui::vec2(760.0, 500.0);
/// Room the shell's menu bar and toolbar take above the library, rounded
/// up, plus the 23 px title strip the classic skin draws over them outside
/// macOS; and the status bar's room below the library.
const CHROME_TOP: f32 = 64.0 + 23.0;
const CHROME_BOTTOM: f32 = 24.0;

/// One frame of the whole library in a `size` window; with `chrome`, under
/// the shell's menu bar and toolbar and over its status bar.
fn library_frame(
    ctx: &egui::Context,
    view: &mut LibraryView,
    size: egui::Vec2,
    chrome: bool,
    time: &mut f64,
    events: Vec<egui::Event>,
) -> (Vec<Drawn>, Vec<egui::epaint::RectShape>) {
    *time += 0.1;
    let output = firmware::tests::frame(ctx, size, *time, events, |ui| {
        if chrome {
            egui::Panel::top("chrome")
                .exact_size(CHROME_TOP)
                .show_separator_line(false)
                .show(ui, |_| {});
            egui::Panel::bottom("statusbar")
                .exact_size(CHROME_BOTTOM)
                .show_separator_line(false)
                .show(ui, |_| {});
        }
        view.show(ui, &[], &mut None);
    });
    drawn(&output)
}

/// Every laid-out row of `text` inside its visible area.
fn whole(text: &Drawn) -> bool {
    text.rows
        .iter()
        .all(|row| text.clip.expand(0.5).contains_rect(*row))
}

/// The panes the footer check visits, each with the note under its actions;
/// the machine is one made at migration, as the one the window was reported
/// with, so its pane carries that line too.
fn panes_with_notes() -> Vec<(
    &'static str,
    Box<dyn std::any::Any>,
    LibraryView,
    &'static str,
)> {
    let (f, mut machine) = machine_view();
    let mut rows = (*machine.machines).clone();
    rows[0].listing.machine.origin = image_store::machine::Origin::Migration;
    machine.set_machines(rows);
    let (g, image) = image_view();
    let (h, firmware_cache) = firmware::tests::library();
    let mut firmware = LibraryView::new(None);
    firmware.select_first_package(&firmware_cache, &mut None);
    let (m, media_cache) = install_media::tests::library();
    let mut media = LibraryView::new(None);
    media.set_filter(Filter::Media, &mut None);
    media.snapshot = Arc::new(snapshot(&media_cache, &media.query));
    let row = media.snapshot.media_rows[0].clone();
    media.select_media(&row, &mut None);
    vec![
        (
            "machine",
            Box::new(f),
            machine,
            MACHINE_NOTE,
        ),
        (
            "image",
            Box::new(g),
            image,
            "Settings… on an image publishes a new image revision; machines from this image keep running this one. To change one machine only, use Settings… on that machine.",
        ),
        (
            "firmware",
            Box::new((h, firmware_cache)),
            firmware,
            "Edit metadata… creates a new package revision; ROM blobs unchanged. A machine embeds its ROM blobs and records the source package digest. Source folders are not needed after import.",
        ),
        ("media", Box::new((m, media_cache)), media, MEDIA_NOTE),
    ]
}

/// The note under a pane's actions can be read in a `size` window, with or
/// without the shell's `chrome`: it is whole in view, or the pane, taller
/// than its area, shows its scroll bar at rest and scrolling to its end
/// brings the whole note into view. Returns whether the pane had to scroll.
fn assert_note_reachable(
    what: &str,
    view: &mut LibraryView,
    note: &str,
    size: egui::Vec2,
    chrome: bool,
) -> bool {
    let width = size.x;
    let (top, bottom) = if chrome {
        (CHROME_TOP, size.y - CHROME_BOTTOM)
    } else {
        (0.0, size.y)
    };
    let ctx = egui::Context::default();
    ui_kit::skin::configure_context(&ctx);
    let mut time = 0.0;
    let mut shown = library_frame(&ctx, view, size, chrome, &mut time, vec![]);
    for _ in 0..3 {
        shown = library_frame(&ctx, view, size, chrome, &mut time, vec![]);
    }
    if shown.0.iter().any(|t| t.text == note && whole(t)) {
        return false;
    }
    // The pane's visible area: where its texts are clipped, under the list.
    let area = shown
        .0
        .iter()
        .map(|t| t.clip)
        .filter(|c| {
            c.left() >= TREE_WIDTH - 1.0
                && c.top() > top + LIST_MIN_HEIGHT / 2.0
                && c.bottom() > bottom - 10.0
        })
        .max_by(|a, b| a.height().total_cmp(&b.height()))
        .unwrap_or_else(|| panic!("{what} at {size:?}: no details pane"));
    // An opaque bar at the pane's right edge, as tall as it at most, that
    // stays put frame after frame with the pointer away: shown at rest, not
    // faded in on hover.
    let bars = |rects: &[egui::epaint::RectShape]| -> Vec<egui::epaint::RectShape> {
        rects
            .iter()
            .filter(|r| {
                r.fill.a() == u8::MAX
                    && r.rect.left() >= area.right() - 12.0
                    && r.rect.right() <= width
                    && r.rect.top() >= area.top() - 1.0
                    && r.rect.bottom() <= area.bottom() + 1.0
                    && r.rect.width() <= 12.0
                    && r.rect.height() >= 12.0
            })
            .cloned()
            .collect()
    };
    let before = bars(&shown.1);
    let after = bars(&library_frame(&ctx, view, size, chrome, &mut time, vec![]).1);
    assert!(
        before.iter().any(|bar| after.contains(bar)),
        "{what} at {size:?}: the note is cut off in {area:?} and no scroll bar shows \
         at rest ({before:?} then {after:?})"
    );
    let pointer = area.center();
    library_frame(
        &ctx,
        view,
        size,
        chrome,
        &mut time,
        vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                delta: egui::vec2(0.0, -12000.0),
                modifiers: egui::Modifiers::default(),
            },
        ],
    );
    for _ in 0..30 {
        shown = library_frame(
            &ctx,
            view,
            size,
            chrome,
            &mut time,
            vec![egui::Event::PointerMoved(pointer)],
        );
    }
    let text = shown.0.iter().find(|t| t.text == note);
    assert!(
        text.is_some_and(whole),
        "{what} at {size:?}: scrolled to its end, the note is {text:?}"
    );
    true
}

/// The note under every pane's actions can be read at the details pane's
/// first height: it is whole in view, or the pane, taller than its area,
/// shows its scroll bar at rest and scrolling to its end brings the whole
/// note into view.
#[test]
fn every_pane_footer_note_is_in_view_or_scrolls_into_view_at_the_first_height() {
    let mut scrolled = 0;
    for (what, _fixture, mut view, note) in panes_with_notes() {
        for width in WINDOW_WIDTHS {
            let size = egui::vec2(width, WINDOW_HEIGHT);
            scrolled += usize::from(assert_note_reachable(what, &mut view, note, size, false));
        }
    }
    assert!(scrolled > 0, "no pane was taller than its first height");
}

/// On a laptop-wide window and wider, the details pane's first height
/// shows the whole pane of a machine or an image without hints or
/// warnings, the note under its actions included: the note is never left
/// cut off at the pane's bottom edge. A machine with a newer-firmware hint
/// is longer; its note scrolls whole into view.
#[test]
fn the_first_height_shows_a_whole_machine_or_image_pane_on_a_laptop_wide_window() {
    for (what, _fixture, mut view, note) in panes_with_notes() {
        if !matches!(what, "machine" | "image") {
            continue;
        }
        for width in [1280.0, 1400.0, 2000.0] {
            let size = egui::vec2(width, 800.0);
            let ctx = egui::Context::default();
            ui_kit::skin::configure_context(&ctx);
            let mut time = 0.0;
            let mut shown = library_frame(&ctx, &mut view, size, true, &mut time, vec![]);
            for _ in 0..3 {
                shown = library_frame(&ctx, &mut view, size, true, &mut time, vec![]);
            }
            let text = shown.0.iter().find(|t| t.text == note);
            assert!(
                text.is_some_and(whole),
                "{what} at {size:?}: the note at the first height is {text:?}"
            );
        }
    }
    let (_f, mut view) = machine_with_newer_hint();
    for width in [1280.0, 1400.0, 2000.0] {
        let size = egui::vec2(width, 800.0);
        assert_note_reachable("machine with the hint", &mut view, MACHINE_NOTE, size, true);
    }
}

/// In the smallest window the shell allows, under its menu bar and toolbar
/// and over its status bar, the note under every pane's actions is whole in
/// view or scrolls whole into view: the pane never cuts it off.
#[test]
fn every_pane_footer_note_reads_whole_in_the_smallest_window() {
    let mut scrolled = 0;
    for (what, _fixture, mut view, note) in panes_with_notes() {
        scrolled += usize::from(assert_note_reachable(
            what, &mut view, note, MIN_WINDOW, true,
        ));
    }
    assert!(
        scrolled > 0,
        "no pane was taller than the smallest window's"
    );
}

/// The test machine with its own choice of the superseded open AT BIOS, so
/// its pane shows the "newer version available" hint.
fn machine_with_newer_hint() -> (Fixture, LibraryView) {
    let (f, mut view) = machine_view();
    let old = crate::firmware_choices::entry("unemu-at-bios-1.0.2-unemu.2").unwrap();
    let mut rows = view.machines.as_ref().clone();
    rows[0].listing.machine.settings = Some(
        toml::from_str(&format!(
            "[firmware.bios]\nimage='assets/firmware-0.bin'\nsha256='{}'\n",
            old.sha256
        ))
        .unwrap(),
    );
    view.set_machines(rows);
    (f, view)
}

/// A firmware layer of the superseded open AT BIOS, as an image's summary
/// lists it, in place of the image's own system BIOS.
fn with_superseded_bios(summary: &mut image_store::summary::ImageSummary) {
    let old = crate::firmware_choices::entry("unemu-at-bios-1.0.2-unemu.2").unwrap();
    summary
        .storage_blobs
        .retain(|d| d.decoded_media_type() != image_store::media_type::FIRMWARE);
    summary.storage_blobs.push(image_store::Descriptor::new(
        image_store::media_type::FIRMWARE,
        format!("sha256:{}", old.sha256).parse().unwrap(),
        old.size,
    ));
}

/// The "newer version available" hint in the machine pane, from the
/// machine's own firmware choice, at every width: nothing overlaps or is
/// cut, and its Switch button is drawn whole.
#[test]
fn the_newer_version_hint_lays_out_in_the_machine_pane_at_every_width() {
    let (_f, mut view) = machine_with_newer_hint();
    for width in WINDOW_WIDTHS {
        let (texts, _) = pane(&mut view, width);
        assert_laid_out("machine pane with the hint", width, &texts);
        assert_type_sizes("machine pane with the hint", &texts);
        assert!(texts.iter().any(|t| t
            .text
            .starts_with("Newer version available. System BIOS 1.0.2-unemu.2")));
        assert!(whole(at(&texts, "Switch to 1.0.2-unemu.3…")));
        assert!(texts
            .iter()
            .any(|t| t.text.starts_with("Switching changes only this machine")));
        at(&texts, "· this machine's choice");
    }
}

/// The hint in the image pane, with the link to a newer tag of the same
/// repository that already uses the replacement, at every width; the list
/// row carries the quiet badge after the name.
#[test]
fn the_newer_version_hint_lays_out_in_the_image_pane_with_its_newer_tag() {
    let Some(store) = crate::library_test_support::FirmwareStore::new(false) else {
        return;
    };
    // Three images of one repository on the current open AT BIOS; the newest
    // version carries `latest` too.
    let image = |kib: u32, tag: &str| {
        pc_image::create::create(
            &store.store,
            &crate::image_settings::tests::open_new_image(&store, kib),
            tag,
            None,
        )
        .unwrap()
    };
    let old = image(1024, "open-at:1.0.0");
    image(2048, "open-at:1.1.0");
    let newest = image(3072, "open-at:1.2.0");
    store
        .store
        .update_tag("open-at:latest", &newest, None, &pc_image::PcProvider)
        .unwrap();
    let cache = firmware::tests::warm(&store.store);
    // The group is closed and a search hides every image: the link still
    // comes from the whole store.
    let query = Query {
        text: "nothing matches this".into(),
        ..Query::default()
    };
    let snapshot = snapshot(&cache, &query);
    assert!(snapshot.rows.is_empty());
    let mut selected = snapshot
        .replacements
        .iter()
        .find(|r| r.descriptor.digest == old.digest)
        .unwrap()
        .clone();
    // The selected image as one on the superseded release would read.
    with_superseded_bios(selected.summary.as_mut().unwrap());
    let mut view = LibraryView::new(None);
    view.query.filter = Filter::All;
    view.snapshot = Arc::new(snapshot);
    view.selected = Some(selected.clone());
    let current = crate::firmware_choices::entry("unemu-at-bios-1.0.2-unemu.3").unwrap();
    let tag = view.newer_tag(&selected, current).unwrap();
    assert_eq!(
        tag.reference, "open-at:1.2.0",
        "the newest version, not latest"
    );
    assert_eq!(tag.descriptor.digest, newest.digest);
    let row = ViewRow {
        image: selected.clone(),
        group: None,
        child: false,
    };
    assert!(row_cells(&row, 0)[0].ends_with("↑ newer BIOS"));
    for width in WINDOW_WIDTHS {
        let (texts, _) = pane(&mut view, width);
        assert_laid_out("image pane with the hint", width, &texts);
        assert_type_sizes("image pane with the hint", &texts);
        assert!(texts.iter().any(|t| t
            .text
            .starts_with("Newer version available. System BIOS 1.0.2-unemu.2")));
        assert!(whole(at(&texts, "open-at:1.2.0")));
        assert!(whole(at(&texts, "Switch to 1.0.2-unemu.3…")));
    }
}

/// The settings sheet's Firmware tab on a machine and on an image, at every
/// window width: the choices, the CMOS line, the unavailable entries with
/// their reasons and the notes never overlap or get cut.
#[test]
fn the_settings_sheet_firmware_tab_lays_out_at_every_width() {
    let Some(store) = crate::library_test_support::FirmwareStore::new(true) else {
        return;
    };
    let row = crate::image_settings::tests::open_image(&store);
    for machine in [false, true] {
        let mut model = crate::image_settings::tests::opened(&store, row.clone(), false);
        if machine {
            let tables = model.machine().clone();
            model = model.for_machine("firmware".into(), "Firmware".into(), tables);
        }
        model.set_packages(store.choices().0);
        model.tab = assembly::capabilities::Group::Firmware;
        model.set_firmware(
            crate::firmware_choices::Slot::System,
            "kiselev-xi8088-1.0.2",
        );
        let what = if machine {
            "machine sheet"
        } else {
            "image sheet"
        };
        // The sheet at every window width: what shows of its scrolled tab
        // never overlaps or gets cut, and the sheet stays in the window.
        for width in WINDOW_WIDTHS {
            let output = frames(width, |ui| {
                let _ = crate::settings_sheet::show_image(ui.ctx(), &mut model, true);
            });
            let (texts, _) = drawn(&output);
            assert_laid_out(what, width, &texts);
            assert!(
                texts.iter().all(|t| t.rect.right() <= width + 1.0),
                "{what} at {width}"
            );
            assert!(texts.iter().any(|t| t.text == "System BIOS"), "{what}");
        }
        // The whole tab at the sheet's width, unscrolled.
        let output = frames(660.0, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = crate::settings_sheet::firmware_tab(ui, &mut model, machine);
            });
        });
        let (texts, _) = drawn(&output);
        assert_laid_out(what, 660.0, &texts);
        for heading in ["System BIOS", "Video BIOS", "Option ROMs"] {
            assert!(texts.iter().any(|t| t.text == heading), "{what}: {heading}");
        }
        assert!(texts
            .iter()
            .any(|t| t.text.starts_with("Another CMOS layout (Kiselev Xi 8088)")));
        assert!(texts
            .iter()
            .any(|t| t.text == "Fit this board but cannot be chosen now:"));
        assert!(texts.iter().any(|t| t.text == "Import ROM folder…"));
        assert!(texts
            .iter()
            .any(|t| t.text.starts_with("user-supplied: with the Kiselev")));
        assert!(texts.iter().any(|t| t.text.starts_with(if machine {
            "Image: unemu AT BIOS 1.0.2-unemu.3 · ● differs from the image"
        } else {
            "Was: unemu AT BIOS 1.0.2-unemu.3"
        })));
    }
}

/// The mock-ups' second face colour (`face-2`), light and dark: the details
/// pane's ground and the inner lower edge of the classic sunken bevel.
const FACE_2: [egui::Color32; 2] = [
    egui::Color32::from_rgb(0xe7, 0xe4, 0xdc),
    egui::Color32::from_rgb(0x3e, 0x40, 0x45),
];
/// The mock-ups' shadow colour (`lo`), light and dark.
const LO: [egui::Color32; 2] = [
    egui::Color32::from_rgb(0x8d, 0x8a, 0x83),
    egui::Color32::from_rgb(0x1f, 0x20, 0x23),
];
/// The mock-ups' dark shadow colour (`dk`), light and dark.
const DK: [egui::Color32; 2] = [
    egui::Color32::from_rgb(0x3f, 0x3d, 0x39),
    egui::Color32::from_rgb(0x0f, 0x10, 0x11),
];
/// The mock-ups' highlight colour (`hi`), light and dark.
const HI: [egui::Color32; 2] = [
    egui::Color32::from_rgb(0xff, 0xff, 0xff),
    egui::Color32::from_rgb(0x5c, 0x5f, 0x66),
];
/// The mock-ups' gap between the library's panes and around them.
const PANE_GAP: f32 = 3.0;
/// The window width the pane grounds are checked at.
const GROUNDS_WIDTH: f32 = 1000.0;

/// One rectangle or line segment a frame painted: its place in paint order,
/// the clip it was painted under, and the shape.
struct Item<T> {
    order: usize,
    clip: egui::Rect,
    shape: T,
}

impl<T> Item<T> {
    /// The part of `bounds`, the shape's own extent, its clip lets show.
    fn visible(&self, bounds: egui::Rect) -> egui::Rect {
        bounds.intersect(self.clip)
    }
}

/// A line segment's end points and stroke.
type Line = ([egui::Pos2; 2], egui::Stroke);

/// A widget as egui reported it to AccessKit: its role, its label and the
/// area it was given.
struct Widget {
    role: egui::accesskit::Role,
    label: String,
    rect: egui::Rect,
}

/// The rectangles and line segments a frame paints, in paint order, with
/// its texts and, where AccessKit is on, its widgets.
struct Painted {
    rects: Vec<Item<egui::epaint::RectShape>>,
    lines: Vec<Item<Line>>,
    texts: Vec<Drawn>,
    widgets: Vec<Widget>,
}

impl Painted {
    fn of(output: &egui::FullOutput) -> Self {
        let widgets = output
            .platform_output
            .accesskit_update
            .as_ref()
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| {
                        let area = node.bounds()?;
                        Some(Widget {
                            role: node.role(),
                            // egui gives a label its text as the node's value.
                            label: node.label().or(node.value()).unwrap_or_default().to_owned(),
                            rect: egui::Rect::from_min_max(
                                egui::pos2(area.x0 as f32, area.y0 as f32),
                                egui::pos2(area.x1 as f32, area.y1 as f32),
                            ),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        fn walk(shape: &egui::Shape, clip: egui::Rect, painted: &mut Painted, order: &mut usize) {
            *order += 1;
            match shape {
                egui::Shape::Rect(rect) => painted.rects.push(Item {
                    order: *order,
                    clip,
                    shape: rect.clone(),
                }),
                egui::Shape::LineSegment { points, stroke } => painted.lines.push(Item {
                    order: *order,
                    clip,
                    shape: (*points, *stroke),
                }),
                egui::Shape::Vec(shapes) => {
                    shapes.iter().for_each(|s| walk(s, clip, painted, order));
                }
                _ => (),
            }
        }
        let mut painted = Painted {
            rects: Vec::new(),
            lines: Vec::new(),
            texts: drawn(output).0,
            widgets,
        };
        let mut order = 0;
        for clipped in &output.shapes {
            walk(&clipped.shape, clipped.clip_rect, &mut painted, &mut order);
        }
        painted
    }

    /// The first pane-sized rectangle filled with `fill` where `place` wants it.
    fn ground(
        &self,
        fill: egui::Color32,
        place: impl Fn(egui::Rect) -> bool,
    ) -> Option<&Item<egui::epaint::RectShape>> {
        self.rects.iter().find(|r| {
            let rect = r.shape.rect;
            r.shape.fill == fill && rect.width() > 100.0 && rect.height() > 100.0 && place(rect)
        })
    }

    /// The 1 px ring just inside `rect`, its top, left, right and bottom
    /// edges, each as last painted.
    fn ring(&self, rect: egui::Rect) -> [Option<&Item<Line>>; 4] {
        let rect = rect.shrink(0.5);
        let edge = |a: egui::Pos2, b: egui::Pos2| {
            self.lines.iter().rev().find(|line| {
                let (points, stroke) = line.shape;
                let at = |p: egui::Pos2, q: egui::Pos2| p.distance(q) < 0.01;
                stroke.width == 1.0
                    && ((at(points[0], a) && at(points[1], b))
                        || (at(points[0], b) && at(points[1], a)))
            })
        };
        [
            edge(rect.left_top(), rect.right_top()),
            edge(rect.left_top(), rect.left_bottom()),
            edge(rect.right_top(), rect.right_bottom()),
            edge(rect.left_bottom(), rect.right_bottom()),
        ]
    }

    /// The colours of [`Self::ring`].
    fn ring_colours(&self, rect: egui::Rect) -> [Option<egui::Color32>; 4] {
        self.ring(rect)
            .map(|edge| edge.map(|line| line.shape.1.color))
    }

    /// The tree's, the list's and the details pane's grounds, in that order.
    fn panes(&self, dark: bool, what: &str) -> [&Item<egui::epaint::RectShape>; 3] {
        let theme = usize::from(dark);
        let find = |name: &str, fill, place: &dyn Fn(egui::Rect) -> bool| {
            self.ground(fill, place).unwrap_or_else(|| {
                let rects: Vec<_> = self.rects.iter().map(|r| &r.shape).collect();
                panic!("{what}: no {name} on {fill:?} in {rects:?}")
            })
        };
        [
            find("tree", FIELD[theme], &|r| r.right() <= TREE_WIDTH + 0.5),
            find("list", FIELD[theme], &|r| {
                r.left() >= TREE_WIDTH && r.top() < WINDOW_HEIGHT / 2.0
            }),
            find("details pane", FACE_2[theme], &|r| {
                r.left() >= TREE_WIDTH && r.bottom() > WINDOW_HEIGHT / 2.0
            }),
        ]
    }
}

/// A context in the classic or the modern skin, light or dark, that reports
/// its widgets to AccessKit, so a frame tells where each widget lies.
fn skinned_context(classic: bool, dark: bool) -> egui::Context {
    let ctx = egui::Context::default();
    ui_kit::skin::configure_context(&ctx);
    ctx.set_visuals(ui_kit::skin::skin_visuals(classic, dark));
    ctx.enable_accesskit();
    ctx
}

/// The library `view` in the classic or the modern skin, light or dark, in a
/// window `size` large, as its second frame paints it. With `status_bar`, a
/// 20 px status bar under it takes the window's bottom, as the shell's does.
/// Returns what was painted and the area the library had.
fn paint_library(
    view: &mut LibraryView,
    classic: bool,
    dark: bool,
    size: egui::Vec2,
    status_bar: bool,
) -> (Painted, egui::Rect) {
    view.classic = classic;
    let ctx = skinned_context(classic, dark);
    let mut area = egui::Rect::NOTHING;
    let mut draw = |ui: &mut egui::Ui| {
        if status_bar {
            egui::Panel::bottom("statusbar")
                .exact_size(20.0)
                .show_separator_line(false)
                .show(ui, |_| {});
        }
        area = ui.available_rect_before_wrap();
        view.show(ui, &[], &mut None);
    };
    firmware::tests::frame(&ctx, size, 0.0, vec![], &mut draw);
    let output = firmware::tests::frame(&ctx, size, 1.0, vec![], &mut draw);
    (Painted::of(&output), area)
}

/// The selected machine's library in the classic or the modern skin, light
/// or dark, in a window the footer checks' height tall.
fn skinned_library(classic: bool, dark: bool) -> Painted {
    let (_f, mut view) = machine_view();
    let size = egui::vec2(GROUNDS_WIDTH, WINDOW_HEIGHT);
    paint_library(&mut view, classic, dark, size, false).0
}

/// The library's panes as the mock-ups draw them, in both skins and both
/// themes: the tree and the list on the field colour (`.pane`), the details
/// pane on the second face colour (`.details`), each inside the inset
/// outline (`--in`) painted over its ground, with the face colour in the
/// 3 px gap around them and the 6 px split between the list and the details
/// pane. The classic skin draws the outline as the sunken two-pixel bevel,
/// shadow and dark shadow at the top and left, highlight and second face at
/// the bottom and right, on square corners; the modern skin as a flat 1 px
/// line in the shadow colour inside the pane's edge, around the mock-ups'
/// 6 px rounded corners. The list's search field stays in view on the
/// field: the modern skin outlines it, the classic skin sinks it into the
/// same bevel.
#[test]
fn the_panes_sit_on_the_field_and_the_second_face_inside_the_inset_outline() {
    for classic in [false, true] {
        for dark in [false, true] {
            let what = format!("classic {classic}, dark {dark}");
            let theme = usize::from(dark);
            let painted = skinned_library(classic, dark);
            let panes = painted.panes(dark, &what);
            let [tree, list, details] = panes.map(|pane| pane.shape.rect);
            assert_eq!(
                tree,
                egui::Rect::from_min_max(
                    egui::pos2(PANE_GAP, PANE_GAP),
                    egui::pos2(TREE_WIDTH, WINDOW_HEIGHT - PANE_GAP)
                ),
                "{what}"
            );
            for (pane, rect) in [("list", list), ("details pane", details)] {
                assert_eq!(rect.left(), TREE_WIDTH + PANE_GAP, "{what}: {pane}");
                assert_eq!(rect.right(), GROUNDS_WIDTH - PANE_GAP, "{what}: {pane}");
            }
            assert_eq!(list.top(), PANE_GAP, "{what}");
            assert_eq!(details.bottom(), WINDOW_HEIGHT - PANE_GAP, "{what}");
            assert_eq!(details.top() - list.bottom(), 2.0 * PANE_GAP, "{what}");
            let window = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(GROUNDS_WIDTH, WINDOW_HEIGHT),
            );
            assert!(
                painted
                    .rects
                    .iter()
                    .any(|r| r.shape.fill == FACE[theme] && r.shape.rect.contains_rect(window)),
                "{what}: no face colour under the gaps"
            );
            // The bevel's edges in the mock-up's colours, each painted after `after`.
            let sunken = |rect: egui::Rect, name: &str, after: usize| {
                assert_eq!(
                    painted.ring_colours(rect),
                    [
                        Some(LO[theme]),
                        Some(LO[theme]),
                        Some(HI[theme]),
                        Some(HI[theme])
                    ],
                    "{what}: {name}'s outer bevel"
                );
                assert_eq!(
                    painted.ring_colours(rect.shrink(1.0)),
                    [
                        Some(DK[theme]),
                        Some(DK[theme]),
                        Some(FACE_2[theme]),
                        Some(FACE_2[theme])
                    ],
                    "{what}: {name}'s inner bevel"
                );
                for edge in painted
                    .ring(rect)
                    .into_iter()
                    .chain(painted.ring(rect.shrink(1.0)))
                {
                    assert!(
                        edge.is_some_and(|line| line.order > after),
                        "{what}: {name}'s bevel is painted before its ground"
                    );
                }
            };
            let corners = egui::CornerRadius::same(if classic { 0 } else { 6 });
            for (name, pane) in ["tree", "list", "details pane"].into_iter().zip(panes) {
                let rect = pane.shape.rect;
                assert_eq!(
                    pane.shape.corner_radius, corners,
                    "{what}: the {name}'s corners"
                );
                if classic {
                    sunken(rect, name, pane.order);
                } else {
                    let outline = painted.rects.iter().find(|r| {
                        r.shape.rect == rect
                            && r.shape.fill == egui::Color32::TRANSPARENT
                            && r.shape.stroke == egui::Stroke::new(1.0, LO[theme])
                    });
                    let outline = outline.unwrap_or_else(|| {
                        panic!("{what}: the {name} has no 1 px shadow-coloured outline")
                    });
                    assert_eq!(outline.shape.corner_radius, corners, "{what}: {name}");
                    assert_eq!(
                        outline.shape.stroke_kind,
                        egui::StrokeKind::Inside,
                        "{what}: the {name}'s outline is not inside its edge"
                    );
                    assert!(
                        outline.order > pane.order,
                        "{what}: the {name}'s outline is painted before its ground"
                    );
                }
            }
            let search = painted
                .rects
                .iter()
                .find(|r| {
                    r.shape.fill == FIELD[theme]
                        && r.shape.rect.height() < 30.0
                        && list.contains_rect(r.shape.rect)
                        && r.shape.rect.top() < list.top() + 30.0
                })
                .unwrap_or_else(|| panic!("{what}: no search field"));
            if classic {
                sunken(search.shape.rect, "search field", search.order);
            } else {
                assert_eq!(
                    search.shape.stroke,
                    egui::Stroke::new(1.0, LO[theme]),
                    "{what}: search field"
                );
            }
        }
    }
}

/// In a short window under the status bar, with more in the tree, the list
/// or the details pane than fits, no pane's ground or outline shows outside
/// the library's area or in the gaps between and around the panes, in
/// either skin. A pane takes the room its panel is given, not the room its
/// content asks for: the tree scrolls, as the mock-ups' panes do.
#[test]
fn panes_stay_inside_the_library_in_a_short_window() {
    let (_m, mut machine) = machine_view();
    let (_f, cache) = firmware::tests::library();
    let mut firmware = LibraryView::new(None);
    firmware.select_first_package(&cache, &mut None);
    let mut failures = Vec::new();
    for (collection, view) in [("Machines", &mut machine), ("Firmware", &mut firmware)] {
        for classic in [false, true] {
            for height in [400.0, 300.0] {
                let what = format!("{collection}, classic {classic}, {height} px");
                let size = egui::vec2(GROUNDS_WIDTH, height);
                let (painted, area) = paint_library(view, classic, false, size, true);
                assert!(area.bottom() < height, "{what}: no status bar");
                let inside = area.shrink(PANE_GAP);
                let between = egui::Rect::from_x_y_ranges(
                    area.left() + TREE_WIDTH..=area.left() + TREE_WIDTH + PANE_GAP,
                    area.y_range(),
                );
                let pane_colours = [FIELD[0], FACE_2[0], LO[0], HI[0], DK[0]];
                let mut shown: Vec<(String, egui::Rect)> = Vec::new();
                for rect in &painted.rects {
                    let colour = if rect.shape.fill == egui::Color32::TRANSPARENT {
                        rect.shape.stroke.color
                    } else {
                        rect.shape.fill
                    };
                    if pane_colours.contains(&colour) {
                        let bounds = rect.shape.visual_bounding_rect();
                        shown.push((format!("{:?}", rect.shape), rect.visible(bounds)));
                    }
                }
                for line in &painted.lines {
                    let (points, stroke) = line.shape;
                    if pane_colours.contains(&stroke.color) {
                        let bounds = egui::Rect::from_two_pos(points[0], points[1])
                            .expand(stroke.width / 2.0);
                        shown.push((format!("{points:?} {stroke:?}"), line.visible(bounds)));
                    }
                }
                assert!(!shown.is_empty(), "{what}: no pane painted");
                for (shape, visible) in shown {
                    if !visible.is_positive() {
                        continue;
                    }
                    if !inside.expand(0.01).contains_rect(visible) {
                        failures.push(format!(
                            "{what}: {shape} shows at {visible:?}, outside the panes' {inside:?}"
                        ));
                    }
                    let gap = visible.intersect(between);
                    if gap.is_positive() {
                        failures.push(format!(
                            "{what}: {shape} shows at {gap:?}, in the gap beside the tree"
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A tree taller than its pane scrolls, as the mock-ups' `.pane` does: in a
/// short window the Firmware collection's open facet groups push the tree's
/// last line out of view, and the mouse wheel over the tree brings it back
/// whole inside the pane.
#[test]
fn a_tree_taller_than_its_pane_scrolls() {
    let (_f, cache) = firmware::tests::library();
    let mut view = LibraryView::new(None);
    view.select_first_package(&cache, &mut None);
    let ctx = egui::Context::default();
    ui_kit::skin::configure_context(&ctx);
    ctx.set_visuals(ui_kit::skin::skin_visuals(true, false));
    let size = egui::vec2(GROUNDS_WIDTH, 300.0);
    // The Remotes heading closes the tree of a library without registries.
    let last = "Remotes";
    let mut time = 0.0;
    let mut pass = |events: Vec<egui::Event>| {
        time += 0.1;
        let output = firmware::tests::frame(&ctx, size, time, events, |ui| {
            egui::Panel::bottom("statusbar")
                .exact_size(20.0)
                .show(ui, |_| {});
            view.show(ui, &[], &mut None);
        });
        drawn(&output).0.into_iter().find(|t| t.text == last)
    };
    pass(Vec::new());
    let before = pass(Vec::new());
    assert!(
        !before.as_ref().is_some_and(whole),
        "the tree fits a 300 px window: {before:?}"
    );
    let pointer = egui::pos2(TREE_WIDTH / 2.0, 100.0);
    pass(vec![
        egui::Event::PointerMoved(pointer),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            delta: egui::vec2(0.0, -12000.0),
            modifiers: egui::Modifiers::default(),
        },
    ]);
    let mut after = None;
    for _ in 0..30 {
        after = pass(vec![egui::Event::PointerMoved(pointer)]);
    }
    let after = after.unwrap_or_else(|| panic!("{last:?} is not drawn"));
    assert!(
        whole(&after),
        "scrolled to its end, the tree shows {after:?}"
    );
    assert!(after.rect.right() <= TREE_WIDTH, "{after:?}");
}

/// Text, stripes and the selection still read on the panes' grounds, by
/// the contrast checks the skins already meet on the face. In the modern
/// skin, whose text takes the mock-ups' ink and muted colours, plain,
/// strong and muted text reach 4.5:1, the WCAG 2.x AA ratio for body text,
/// on the field, the stripe and the second face; the one exception is the
/// mock-ups' own muted text on their second face in the dark theme, which
/// reads 4.27:1 there. Links, warnings, errors and the open mark in the
/// details pane reach 4.5:1 on the second face too; the classic skin shares
/// the warning, error and open-mark colours. The classic skin keeps egui's
/// text and link colours, which the existing checks hold to 3:1, the ratio
/// its icons in the text colour need; its plain and strong text keep that on
/// every ground. Its link colour is egui's own and is left out. A stripe lies
/// between the field and the face, so a striped row reads as a band of the
/// list rather than a hole in it, and the selection stands out from the
/// field at least as well as it did from the face.
#[test]
fn text_stripes_and_the_selection_read_on_the_pane_grounds() {
    use super::tests::contrast;
    const AA: f64 = 4.5;
    const NON_TEXT: f64 = 3.0;
    const MUTED_ON_FACE_2: f64 = 4.2;
    let mut failures = Vec::new();
    for classic in [false, true] {
        for dark in [false, true] {
            let what = format!("classic {classic}, dark {dark}");
            let visuals = ui_kit::skin::skin_visuals(classic, dark);
            let painted = skinned_library(classic, dark);
            let [tree, list, details] = painted.panes(dark, &what).map(|pane| pane.shape.fill);
            let (field, face_2) = (list, details);
            assert_eq!(tree, field, "{what}");
            let stripe = visuals.faint_bg_color;
            let face = visuals.panel_fill;
            let text = if classic { NON_TEXT } else { AA };
            let mut pairs = Vec::new();
            for (ground_name, ground) in [("field", field), ("stripe", stripe), ("face-2", face_2)]
            {
                pairs.push(("text", visuals.text_color(), ground_name, ground, text));
                pairs.push((
                    "strong text",
                    visuals.strong_text_color(),
                    ground_name,
                    ground,
                    text,
                ));
                if !classic {
                    let muted = if ground_name == "face-2" && dark {
                        MUTED_ON_FACE_2
                    } else {
                        AA
                    };
                    let weak = visuals.weak_text_color();
                    pairs.push(("muted text", weak, ground_name, ground, muted));
                }
            }
            if !classic {
                pairs.push(("link", visuals.hyperlink_color, "face-2", face_2, AA));
            }
            for (ink_name, ink) in [
                ("warning", visuals.warn_fg_color),
                ("error", visuals.error_fg_color),
                ("open mark", pane::good_colour(&visuals)),
            ] {
                pairs.push((ink_name, ink, "face-2", face_2, AA));
            }
            for (ink_name, ink, ground_name, ground, needed) in pairs {
                let ratio = contrast(ink, ground);
                if !ink.is_opaque() || ratio < needed {
                    failures.push(format!(
                        "{what}: {ink_name} {ink:?} on the {ground_name} {ground:?} is \
                         {ratio:.2}:1, under {needed}:1"
                    ));
                }
            }
            let across = contrast(field, face);
            if !(contrast(field, stripe) > 1.0
                && contrast(field, stripe) < across
                && contrast(stripe, face) < across)
            {
                failures.push(format!(
                    "{what}: the stripe {stripe:?} is not between the field {field:?} and the \
                     face {face:?}"
                ));
            }
            let selection = visuals.selection.bg_fill;
            if contrast(selection, field) < contrast(selection, face) {
                failures.push(format!(
                    "{what}: the selection {selection:?} stands out less from the field \
                     {field:?} than from the face {face:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The Machines list's column headers; every one of them sorts.
const MACHINE_HEADERS: [&str; 5] = ["Name", "Source image", "State", "Last run", "Changes"];
/// The room the library mock-up's list header keeps beside each label
/// (`.ghead button`, 6 px).
const HEADER_INSET: f32 = 6.0;

/// The Images collection of a store with three images, one tag each, so
/// every row is an image row under the list's header.
fn images_list() -> (assembly::test_support::TempDir, LibraryView) {
    let (temp, store, _) = tests::catalogue(3);
    let cache = firmware::tests::warm(&store);
    let mut view = LibraryView::new(None);
    view.query.filter = Filter::All;
    view.snapshot = Arc::new(snapshot(&cache, &view.query));
    (temp, view)
}

/// A list's header cells, by `labels`, and the cells of its first row that
/// fills every column, each left to right, from the list pane of a
/// [`paint_library`] frame. A sorted column's header adds its sort mark.
fn header_and_row<'a>(
    painted: &'a Painted,
    labels: &[&str],
    what: &str,
) -> (Vec<&'a Widget>, Vec<&'a Widget>) {
    let in_list = |w: &&Widget| w.rect.left() >= TREE_WIDTH && w.rect.top() < WINDOW_HEIGHT / 2.0;
    let header: Vec<&Widget> = labels
        .iter()
        .map(|label| {
            painted
                .widgets
                .iter()
                .filter(in_list)
                .filter(|w| w.label == *label || w.label.starts_with(&format!("{label} ")))
                .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
                .unwrap_or_else(|| panic!("{what}: no {label:?} header"))
        })
        .collect();
    let cell = |w: &&Widget| {
        w.role == egui::accesskit::Role::Label && w.rect.top() >= header[0].rect.bottom()
    };
    let mut tops: Vec<f32> = painted
        .widgets
        .iter()
        .filter(in_list)
        .filter(cell)
        .map(|w| w.rect.top())
        .collect();
    tops.sort_by(f32::total_cmp);
    tops.dedup();
    let row = tops
        .into_iter()
        .find_map(|top| {
            let mut cells: Vec<&Widget> = painted
                .widgets
                .iter()
                .filter(in_list)
                .filter(cell)
                .filter(|w| w.rect.top() == top)
                .collect();
            cells.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
            (cells.len() == labels.len()).then_some(cells)
        })
        .unwrap_or_else(|| {
            let listed: Vec<_> = painted
                .widgets
                .iter()
                .filter(in_list)
                .map(|w| (w.role, &w.label, w.rect))
                .collect();
            panic!("{what}: no row fills every column in {listed:?}")
        });
    (header, row)
}

/// The library mock-up's list header (`.ghead`), in both skins and both
/// themes, over the Machines list, whose columns all sort, and the Images
/// list, whose Machines column is a plain header: the cells sit edge to edge
/// in one row, each starts where its column's cells start and reaches the
/// next column, so the gap between two columns belongs to the cell before
/// it, and the last runs on over the scroll bar's room without leaving the
/// pane. Every label, the plain one included, sits left-aligned in the
/// mock-up's 6 px padding.
#[test]
fn list_headers_sit_edge_to_edge_over_their_columns() {
    let (_f, machines) = machine_view();
    let (_temp, images) = images_list();
    let lists: [(&str, LibraryView, &[&str]); 2] = [
        ("Machines", machines, &MACHINE_HEADERS),
        ("Images", images, &IMAGE_COLUMNS.labels),
    ];
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    let mut failures = Vec::new();
    for (collection, mut view, labels) in lists {
        for classic in [false, true] {
            for dark in [false, true] {
                let what = format!("{collection}, classic {classic}, dark {dark}");
                let size = egui::vec2(GROUNDS_WIDTH, WINDOW_HEIGHT);
                let (painted, _) = paint_library(&mut view, classic, dark, size, false);
                let (header, row) = header_and_row(&painted, labels, &what);
                let first = header[0].rect;
                for (i, cell) in header.iter().enumerate() {
                    let (rect, column, name) = (cell.rect, row[i].rect, &cell.label);
                    if !near(rect.top(), first.top()) || !near(rect.bottom(), first.bottom()) {
                        failures.push(format!(
                            "{what}: {name:?} {rect:?} is off the row {first:?}"
                        ));
                    }
                    if !near(rect.left(), column.left()) {
                        failures.push(format!(
                            "{what}: {name:?} starts at {}, its column at {}",
                            rect.left(),
                            column.left()
                        ));
                    }
                    match (header.get(i + 1), row.get(i + 1)) {
                        (Some(next), Some(next_column)) => {
                            if !near(next.rect.left(), rect.right()) {
                                failures.push(format!(
                                    "{what}: {:?} starts at {}, {name:?} ends at {}",
                                    next.label,
                                    next.rect.left(),
                                    rect.right()
                                ));
                            }
                            if !near(rect.right(), next_column.rect.left()) {
                                failures.push(format!(
                                    "{what}: {name:?} ends at {}, the next column starts at {}",
                                    rect.right(),
                                    next_column.rect.left()
                                ));
                            }
                        }
                        _ => {
                            if rect.right() < column.right() + SCROLL_ROOM - 0.01 {
                                failures.push(format!(
                                    "{what}: {name:?} ends at {}, short of the scroll bar's \
                                     room after its column's end {}",
                                    rect.right(),
                                    column.right()
                                ));
                            }
                            if rect.right() > GROUNDS_WIDTH - PANE_GAP {
                                failures.push(format!(
                                    "{what}: {name:?} ends at {}, past the pane",
                                    rect.right()
                                ));
                            }
                        }
                    }
                    let Some(text) = painted
                        .texts
                        .iter()
                        .find(|t| t.text == *name && rect.intersects(t.rect))
                    else {
                        failures.push(format!("{what}: no label drawn in {name:?}"));
                        continue;
                    };
                    let room = rect.shrink2(egui::vec2(HEADER_INSET, 0.0));
                    if (text.rect.left() - room.left()).abs() > 0.5
                        || !room.expand(0.5).contains_rect(text.rect)
                    {
                        failures.push(format!(
                            "{what}: {name:?} is drawn at {:?}, not left-aligned in {room:?}",
                            text.rect
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Each header cell sits on the face colour inside the skin's raised outline
/// (`--out`), in both themes, the Images list's plain header included. The
/// classic skin draws the raised two-pixel bevel: highlight and second face
/// along the top and left, dark shadow and shadow along the bottom and right.
/// The modern skin draws the flat 1 px line in the shadow colour, on square
/// corners. The mock-up draws that line outside each cell, so neighbours
/// share one line: each later cell's line starts on the last pixel of the
/// cell before it. Every outline is painted over its cell's ground.
#[test]
fn list_header_cells_wear_the_skins_raised_outline() {
    let (_temp, mut view) = images_list();
    let mut failures = Vec::new();
    for classic in [false, true] {
        for dark in [false, true] {
            let theme = usize::from(dark);
            let what = format!("classic {classic}, dark {dark}");
            let size = egui::vec2(GROUNDS_WIDTH, WINDOW_HEIGHT);
            let (painted, _) = paint_library(&mut view, classic, dark, size, false);
            let (header, _) = header_and_row(&painted, &IMAGE_COLUMNS.labels, &what);
            for (i, cell) in header.iter().enumerate() {
                let (rect, name) = (cell.rect, &cell.label);
                let Some(ground) = painted
                    .rects
                    .iter()
                    .find(|r| r.shape.fill == FACE[theme] && r.shape.rect == rect)
                else {
                    failures.push(format!("{what}: {name:?} is not on the face colour"));
                    continue;
                };
                if classic {
                    let raised = [
                        painted.ring_colours(rect),
                        painted.ring_colours(rect.shrink(1.0)),
                    ];
                    let expected = [
                        [HI, HI, DK, DK].map(|c| Some(c[theme])),
                        [FACE_2, FACE_2, LO, LO].map(|c| Some(c[theme])),
                    ];
                    if raised != expected {
                        failures.push(format!(
                            "{what}: {name:?} has the bevel {raised:?}, not {expected:?}"
                        ));
                    }
                    let over = painted
                        .ring(rect)
                        .into_iter()
                        .chain(painted.ring(rect.shrink(1.0)))
                        .all(|edge| edge.is_some_and(|line| line.order > ground.order));
                    if !over {
                        failures.push(format!("{what}: {name:?}'s bevel is under its ground"));
                    }
                } else {
                    let outline = if i == 0 {
                        rect
                    } else {
                        rect.with_min_x(rect.left() - 1.0)
                    };
                    let found = painted.rects.iter().any(|r| {
                        r.shape.rect == outline
                            && r.shape.stroke == egui::Stroke::new(1.0, LO[theme])
                            && r.shape.stroke_kind == egui::StrokeKind::Inside
                            && r.shape.corner_radius == egui::CornerRadius::ZERO
                            && r.order > ground.order
                    });
                    if !found {
                        failures.push(format!(
                            "{what}: {name:?} has no square 1 px shadow-coloured outline on \
                             {outline:?} over its ground"
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A header cell pressed in the classic skin sinks into the mock-up's inset
/// bevel (`--in`), in both themes; its click sorts the Machines list by its
/// column, and a second click reverses the order.
#[test]
fn a_pressed_header_cell_sinks_and_its_click_sorts_the_list() {
    for dark in [false, true] {
        let theme = usize::from(dark);
        let what = format!("dark {dark}");
        let (_f, mut view) = machine_view();
        view.classic = true;
        let ctx = skinned_context(true, dark);
        let size = egui::vec2(GROUNDS_WIDTH, WINDOW_HEIGHT);
        let mut time = 0.0;
        let mut pass = |view: &mut LibraryView, events: Vec<egui::Event>| {
            time += 0.1;
            let output = firmware::tests::frame(&ctx, size, time, events, |ui| {
                view.show(ui, &[], &mut None);
            });
            Painted::of(&output)
        };
        pass(&mut view, Vec::new());
        let painted = pass(&mut view, Vec::new());
        let at = header_and_row(&painted, &MACHINE_HEADERS, &what).0[1]
            .rect
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let painted = pass(&mut view, vec![egui::Event::PointerMoved(at), button(true)]);
        let rect = header_and_row(&painted, &MACHINE_HEADERS, &what).0[1].rect;
        assert_eq!(
            [
                painted.ring_colours(rect),
                painted.ring_colours(rect.shrink(1.0))
            ],
            [
                [LO, LO, HI, HI].map(|c| Some(c[theme])),
                [DK, DK, FACE_2, FACE_2].map(|c| Some(c[theme])),
            ],
            "{what}: the pressed cell does not sink"
        );
        pass(&mut view, vec![button(false)]);
        assert_eq!(
            (view.query.machine_sort, view.query.machine_descending),
            (1, false),
            "{what}: the click does not sort by the column"
        );
        pass(&mut view, vec![button(true)]);
        pass(&mut view, vec![button(false)]);
        assert_eq!(
            (view.query.machine_sort, view.query.machine_descending),
            (1, true),
            "{what}: a second click does not reverse the order"
        );
    }
}

/// The details pane alone as [`pane`] draws it, with the room its panel
/// gives it and the bounds of every mesh it paints, the kind tile's among
/// them.
fn pane_in_room(view: &mut LibraryView, width: f32) -> (Vec<Drawn>, Vec<egui::Rect>, egui::Rect) {
    fn meshes(shape: &egui::Shape, found: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Mesh(mesh) => found.push(mesh.calc_bounds()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| meshes(s, found)),
            _ => (),
        }
    }
    let mut room = egui::Rect::NOTHING;
    let output = frames(width, |ui| {
        egui::Panel::left("tree")
            .exact_size(TREE_WIDTH)
            .show(ui, |_| {});
        egui::CentralPanel::default().show(ui, |ui| {
            room = ui.max_rect();
            let mut commands = Vec::new();
            view.details_pane(ui, &[], &mut None, &mut commands);
        });
    });
    let mut found = Vec::new();
    for clipped in &output.shapes {
        meshes(&clipped.shape, &mut found);
    }
    (drawn(&output).0, found, room)
}

/// Every collection's details pane with sections side by side starts its
/// columns level at the top of the pane on a wide window: the first column
/// with the pane's head (the kind tile, the title and the facts under it),
/// the others with their first section beside the head, not under it. A
/// pane too narrow for three columns keeps two beside each other and stacks
/// the third under them. On a narrow window the columns stack: the head,
/// the first column's sections, then the others under them.
#[test]
fn details_columns_start_level_at_the_top_and_stack_when_narrow() {
    // Each pane: the first text of every column after the first, and the
    // last text of the first column.
    let columns: [(&str, &[&str], &str); 4] = [
        ("machine", &["Settings that differ from the image"], "CMOS"),
        ("image", &["Firmware"], "Apply labels"),
        (
            "firmware",
            &["Chip layout", "Documents (extras)"],
            "Provenance",
        ),
        (
            "media",
            &["Media in catalogue order", "Extras"],
            "Set SHA-256",
        ),
    ];
    for ((what, _fixture, mut view, _), (name, others, last)) in
        panes_with_notes().into_iter().zip(columns)
    {
        assert_eq!(what, name);
        for width in WINDOW_WIDTHS {
            let (texts, meshes, room) = pane_in_room(&mut view, width);
            let firsts: Vec<&Drawn> = others.iter().map(|text| at(&texts, text)).collect();
            if room.width() < pane::SPLIT_WIDTH {
                for first in &firsts {
                    assert!(
                        (first.rect.left() - room.left()).abs() <= 1.0
                            && first.rect.top() > at(&texts, last).rect.bottom(),
                        "{what} at {width}: {:?} at {:?} does not stack under {last:?} in {room:?}",
                        first.text,
                        first.rect
                    );
                }
                continue;
            }
            let beside = if room.width() >= pane::SPLIT3_WIDTH {
                others.len()
            } else {
                1
            };
            // Too narrow for three columns, the third stacks under the first two.
            for under in &firsts[beside..] {
                assert!(
                    (under.rect.left() - room.left()).abs() <= 1.0
                        && under.rect.top() > at(&texts, last).rect.bottom(),
                    "{what} at {width}: {:?} at {:?} does not stack under {last:?} in {room:?}",
                    under.text,
                    under.rect
                );
            }
            let firsts = &firsts[..beside];
            for first in firsts {
                assert!(
                    first.rect.left() > room.left() + 1.0
                        && (first.rect.top() - room.top()).abs() <= 1.0,
                    "{what} at {width}: {:?} at {:?}, not beside the first column at the \
                     pane's top {}",
                    first.text,
                    first.rect,
                    room.top()
                );
            }
            // The first column: everything drawn left of the next one.
            let split = firsts
                .iter()
                .map(|t| t.rect.left())
                .fold(f32::INFINITY, f32::min);
            let top = texts
                .iter()
                .filter(|t| t.visible().is_positive())
                .map(|t| t.rect)
                .chain(meshes.iter().copied())
                .filter(|r| r.right() <= split)
                .map(|r| r.top())
                .fold(f32::INFINITY, f32::min);
            assert!(
                (top - room.top()).abs() <= 1.0,
                "{what} at {width}: the first column starts at {top}, the pane at {}",
                room.top()
            );
        }
    }
}

/// The labels field of a machine's and of an image's pane keeps its focus
/// while the window is resized across the width where the pane's columns
/// stack, so typing goes on into the same field.
#[test]
fn the_labels_field_keeps_its_focus_when_the_columns_stack() {
    for (what, _fixture, mut view, _) in panes_with_notes() {
        let labels = match what {
            "machine" => "games, testing",
            "image" => "template",
            _ => continue,
        };
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        let mut time = 0.0;
        let mut draw = |view: &mut LibraryView, width: f32, events: Vec<egui::Event>| {
            time += 0.1;
            let size = egui::vec2(width, 2400.0);
            let output = firmware::tests::frame(&ctx, size, time, events, |ui| {
                egui::Panel::left("tree")
                    .exact_size(TREE_WIDTH)
                    .show(ui, |_| {});
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut commands = Vec::new();
                    view.details_pane(ui, &[], &mut None, &mut commands);
                });
            });
            drawn(&output).0
        };
        draw(&mut view, 1400.0, vec![]);
        let texts = draw(&mut view, 1400.0, vec![]);
        let field = at(&texts, labels).rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: field,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        draw(&mut view, 1400.0, vec![egui::Event::PointerMoved(field)]);
        draw(&mut view, 1400.0, vec![button(true)]);
        draw(&mut view, 1400.0, vec![button(false)]);
        let focused = ctx.memory(|m| m.focused());
        assert!(
            focused.is_some(),
            "{what}: a click does not focus the field"
        );
        for _ in 0..3 {
            draw(&mut view, 760.0, vec![]);
        }
        assert_eq!(
            ctx.memory(|m| m.focused()),
            focused,
            "{what}: the field loses its focus when the columns stack"
        );
    }
}
