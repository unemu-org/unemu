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
                "Role",
                "Status",
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
    crate::shell::configure_context(&ctx);
    ctx.set_visuals(crate::shell::skin_visuals(false, dark));
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
        let visuals = crate::shell::skin_visuals(false, dark);
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
            let visuals = crate::shell::skin_visuals(classic, dark);
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
        let visuals = crate::shell::skin_visuals(false, dark);
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
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

/// The note under the installation media pane's actions.
const MEDIA_NOTE: &str = "Stored media and extras stay read-only. Installer writes use the \
                          selected workspace's journal.";

/// The window height the footer checks use, where the details pane opens at
/// its first height.
const WINDOW_HEIGHT: f32 = 1000.0;

/// One frame of the whole library in a `width` × [`WINDOW_HEIGHT`] window.
fn library_frame(
    ctx: &egui::Context,
    view: &mut LibraryView,
    width: f32,
    time: &mut f64,
    events: Vec<egui::Event>,
) -> (Vec<Drawn>, Vec<egui::epaint::RectShape>) {
    *time += 0.1;
    let size = egui::vec2(width, WINDOW_HEIGHT);
    let output = firmware::tests::frame(ctx, size, *time, events, |ui| {
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
            "Gaming mode plays a disposable copy of this machine as it is now; this machine does not change. Reset to image… throws its changes away and keeps the machine; Delete machine… removes both, never the image.",
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

/// The note under every pane's actions can be read at the details pane's
/// first height: it is whole in view, or the pane, taller than its area,
/// shows its scroll bar at rest and scrolling to its end brings the whole
/// note into view.
#[test]
fn every_pane_footer_note_is_in_view_or_scrolls_into_view_at_the_first_height() {
    let mut scrolled = 0;
    for (what, _fixture, mut view, note) in panes_with_notes() {
        for width in WINDOW_WIDTHS {
            let ctx = egui::Context::default();
            crate::shell::configure_context(&ctx);
            let mut time = 0.0;
            let mut shown = library_frame(&ctx, &mut view, width, &mut time, vec![]);
            for _ in 0..3 {
                shown = library_frame(&ctx, &mut view, width, &mut time, vec![]);
            }
            if shown.0.iter().any(|t| t.text == note && whole(t)) {
                continue;
            }
            // The pane's visible area: where its texts are clipped, under the list.
            let area = shown
                .0
                .iter()
                .map(|t| t.clip)
                .filter(|c| {
                    c.left() >= TREE_WIDTH - 1.0
                        && c.top() > WINDOW_HEIGHT / 2.0
                        && c.bottom() > WINDOW_HEIGHT - 10.0
                })
                .max_by(|a, b| a.height().total_cmp(&b.height()))
                .unwrap_or_else(|| panic!("{what} at {width}: no details pane"));
            // An opaque bar at the pane's right edge, as tall as it at most,
            // that stays put frame after frame with the pointer away: shown at
            // rest, not faded in on hover.
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
            let after = bars(&library_frame(&ctx, &mut view, width, &mut time, vec![]).1);
            assert!(
                before.iter().any(|bar| after.contains(bar)),
                "{what} at {width}: the note is cut off in {area:?} and no scroll bar shows \
                 at rest ({before:?} then {after:?})"
            );
            let pointer = area.center();
            library_frame(
                &ctx,
                &mut view,
                width,
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
                    &mut view,
                    width,
                    &mut time,
                    vec![egui::Event::PointerMoved(pointer)],
                );
            }
            let text = shown.0.iter().find(|t| t.text == note);
            assert!(
                text.is_some_and(whole),
                "{what} at {width}: scrolled to its end, the note is {text:?}"
            );
            scrolled += 1;
        }
    }
    assert!(scrolled > 0, "no pane was taller than its first height");
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
    let (_f, mut view) = machine_view();
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
