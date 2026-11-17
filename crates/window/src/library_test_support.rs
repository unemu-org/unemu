// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless desktop sessions use synthetic firmware, disks, catalogue and CMOS.
use crate::library_view::ImageRow;
use image_store::Store;
use std::{fs, path::Path};
#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;
/// One medium of an installation set, as `upcli image media import` takes
/// it: the file, and its `--label N=TEXT` and `--format N=FORMAT` if given.
pub(crate) struct Medium<'a> {
    pub path: &'a Path,
    pub label: Option<&'a str>,
    pub format: Option<&'a str>,
}
impl<'a> Medium<'a> {
    pub fn new(path: &'a Path) -> Self {
        Self {
            path,
            label: None,
            format: None,
        }
    }
    pub fn label(self, label: &'a str) -> Self {
        Self {
            label: Some(label),
            ..self
        }
    }
    pub fn format(self, format: &'a str) -> Self {
        Self {
            format: Some(format),
            ..self
        }
    }
}

/// Publish an installation set titled `title` as `tag` into `store`, as
/// `upcli image media import` does with the same files, labels, formats and
/// extras (each with its DOS name, if given): compressed, and never moving an
/// existing tag. Returns the set's manifest.
pub(crate) fn import_media_set(
    store: &Store,
    tag: &str,
    title: &str,
    media: &[Medium<'_>],
    extras: &[(&Path, Option<&str>)],
) -> image_store::Descriptor {
    use pc_image::media_import;
    store.check_new_tag(tag).unwrap();
    let request = media_import::Request {
        tag: tag.into(),
        title: image_store::metadata::Title {
            title: title.into(),
            ..Default::default()
        },
        labels: Vec::new(),
        media: media
            .iter()
            .map(|medium| media_import::Source {
                path: medium.path.to_owned(),
                label: medium.label.map(Into::into),
                format: medium.format.map(Into::into),
                file_name: None,
            })
            .collect(),
        extras: extras
            .iter()
            .map(|(path, dos_name)| media_import::ExtraSource {
                path: path.to_path_buf(),
                dos_name: dos_name.map(Into::into),
                kind: None,
            })
            .collect(),
        compress: true,
    };
    let plan = media_import::plan_with(&request, None).unwrap();
    media_import::publish(store, &plan, &|_, _| {}, &|| false)
        .unwrap()
        .descriptor()
        .clone()
}

pub(crate) struct Fixture {
    pub temp: assembly::test_support::TempDir,
    pub store: Store,
    pub row: ImageRow,
}
impl Fixture {
    pub fn new() -> Self {
        let temp = assembly::test_support::TempDir::new("desktop-session");
        fs::write(temp.path().join("bios.bin"), [0xf4; 32768]).unwrap();
        fs::write(temp.path().join("disk.img"), [0; 16384]).unwrap();
        fs::write(temp.path().join("one.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(temp.path().join("two.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(temp.path().join("cmos.bin"), [0; 64]).unwrap();
        let path = temp.path().join("machine.toml");
        fs::write(&path,"schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nbase=0xF8000\nprovides=['floppy','fixed-disk']\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='cmos.upfj'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='one.img'\nwrite='journal'\njournal='one.upfj'\n[[card.drives.media]]\nimage='two.img'\nwrite='journal'\njournal='two.upfj'\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=32}\nimage='disk.img'\nwrite='journal'\njournal='disk.upfj'\n").unwrap();
        let store = Store::open(temp.path().join("store")).unwrap();
        let descriptor = pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::import_machine(&store, &path, "test:source", &Default::default())
        })
        .unwrap()
        .manifest;
        let row = ImageRow {
            reference: "test:source".into(),
            aliases: Vec::new(),
            descriptor,
            summary: None,
            failure: None,
        };
        Self { temp, store, row }
    }
    /// Import a one-disk 360K installation set, `disks-set:r1`, as the command
    /// line does; returns its manifest.
    pub fn import_set(&self) -> image_store::Descriptor {
        let disk = self.temp.path().join("set-disk.img");
        let args: Vec<std::ffi::OsString> = vec![
            "floppy".into(),
            "--type".into(),
            "360K".into(),
            "--format".into(),
            disk.clone().into_os_string(),
        ];
        pc_image::media_create::execute(
            pc_image::media_create::parse(&args).unwrap(),
            &mut std::io::sink(),
        )
        .unwrap();
        import_media_set(
            &self.store,
            "disks-set:r1",
            "Session disks",
            &[Medium::new(&disk)],
            &[],
        );
        self.store.tag_descriptor("disks-set:r1").unwrap()
    }
    pub fn change(&self, prepared: &pc_image::workspace::PreparedSession, value: u8) {
        let change = |source: &assembly::ImageSource, write: &assembly::WriteMode| {
            let assembly::ImageSource::File(path) = source else {
                panic!("expected file")
            };
            let assembly::WriteMode::Journal { path: journal, .. } = write else {
                panic!("expected journal")
            };
            // A compressed floppy has no file; its verified base holds the bytes.
            let bytes = match prepared.bases.get(path) {
                Some(base) if base.metadata().is_none() => {
                    let mut bytes = vec![0; media::SectorSource::byte_len(base) as usize];
                    media::SectorSource::read_range(base, 0, &mut bytes).unwrap();
                    bytes
                }
                _ => fs::read(path).unwrap(),
            };
            let mut media =
                media::JournalMedia::open(journal, &bytes, bytes.len() as u64, Default::default())
                    .unwrap();
            media.write_batch(&[(0, [value; 512])]);
            media.finish().unwrap();
        };
        for drive in &prepared.machine.drives {
            if let Some(list) = &drive.media {
                for entry in &list.entries {
                    change(&entry.image, &entry.write);
                }
            } else {
                change(&drive.image, &drive.write);
            }
        }
        for device in &prepared.machine.devices {
            if let assembly::DeviceKind::Rtc(rtc) = &device.kind {
                if let assembly::NvramSource::Journal { image, journal, .. } = &rtc.nvram {
                    let bytes = fs::read(image).unwrap();
                    let mut media =
                        media::JournalMedia::open(journal, &bytes, 64, Default::default()).unwrap();
                    let mut sector = [0; 512];
                    sector[..64].copy_from_slice(&bytes);
                    sector[32] = value;
                    media.write_batch(&[(0, sector)]);
                    media.finish().unwrap();
                }
            }
        }
    }
}

/// The source archive the open entry's fetch script builds and the catalogue
/// pins, so the copyleft package is open together with its source.
pub(crate) fn source_archive(candidate: &pc_image::firmware_import::Candidate) -> &'static str {
    match candidate.distribution {
        assembly::FirmwareDistribution::Open { fetch, .. } => match fetch {
            "vendor/unemu-at-bios/fetch.sh" => {
                "vendor/unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar"
            }
            "vendor/vgabios/fetch.sh" => "vendor/vgabios/vgabios-0.9d-unemu.1-src.tar",
            "vendor/xtide-universal-bios/fetch.sh" => {
                "vendor/xtide-universal-bios/xtide-universal-bios-r638-src.tar"
            }
            other => panic!("no source archive for {other}"),
        },
        assembly::FirmwareDistribution::UserSupplied => panic!("not open firmware"),
    }
}

/// A store with the open firmware packages (the open AT BIOS, the open VGA
/// BIOS and XTIDE) and, with `misfits`, a user-supplied package of the
/// Kiselev Xi 8088 build plus synthetic packages that must not fit: an
/// XT-class BIOS and one naming no board. `None` when a fetched fixture it reads is missing,
/// which skips the test.
pub(crate) struct FirmwareStore {
    pub temp: assembly::test_support::TempDir,
    pub store: Store,
}
impl FirmwareStore {
    pub fn new(misfits: bool) -> Option<Self> {
        Self::build(misfits, true)
    }
    /// The open packages imported without their source archives, which
    /// keeps the copyleft ones user-supplied.
    pub fn without_sources() -> Option<Self> {
        Self::build(false, false)
    }
    fn build(misfits: bool, sources: bool) -> Option<Self> {
        let mut images = Vec::new();
        for (from, to) in [
            ("unemu-at-bios/bios-unemu-at.bin", "bios.bin"),
            ("vgabios/vgabios-std.bin", "vga.bin"),
            (
                "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
                "xtide.bin",
            ),
        ] {
            images.push((fixtures::fixture(from)?, to));
        }
        if sources {
            for archive in [
                "unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar",
                "vgabios/vgabios-0.9d-unemu.1-src.tar",
                "xtide-universal-bios/xtide-universal-bios-r638-src.tar",
            ] {
                fixtures::fixture(archive)?;
            }
        }
        let misfit = if misfits {
            Some(fixtures::fixture("kiselev-bios/bios-xi8088.bin")?)
        } else {
            None
        };
        let temp = assembly::test_support::TempDir::new("new-machine-firmware");
        let roms = temp.path().join("roms");
        fs::create_dir(&roms).unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (bytes, to) in images {
            fs::write(roms.join(to), bytes).unwrap();
        }
        let store = Store::open(temp.path().join("store")).unwrap();
        let catalogue = pc_image::firmware::catalogue();
        let scan = pc_image::firmware_import::scan(std::slice::from_ref(&roms), catalogue).unwrap();
        assert_eq!(scan.candidates.len(), 3, "{:?}", scan.issues);
        for candidate in &scan.candidates {
            // The copyleft packages are open only with the archives their
            // fetch scripts build, which the catalogue pins.
            let statement = pc_image::firmware_import::Statement {
                source: sources.then(|| root.join(source_archive(candidate))),
                ..Default::default()
            };
            pc_image::firmware_import::publish_with(&store, candidate, &[], &statement, catalogue)
                .unwrap();
        }
        if let Some(bios) = misfit {
            fs::write(temp.path().join("xi8088.bin"), &bios).unwrap();
            let digest = image_store::Digest::of(&bios);
            let synthetic = vec![0xf4; 32768];
            let other = image_store::Digest::of(&synthetic);
            fs::write(roms.join("synthetic.bin"), &synthetic).unwrap();
            for (tag, name, catalogue_id, fits, source, digest, distribution) in [
                (
                    "private-kiselev",
                    "Kiselev AT BIOS (private copy)",
                    "catalogue_id='kiselev-xi8088-1.0.2'\n",
                    "boards=['ibm-5170']\nmachine_classes=['at']\n",
                    "../xi8088.bin",
                    &digest,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
                (
                    "xt-bios",
                    "Synthetic XT BIOS",
                    "",
                    "boards=['ibm-5160']\nmachine_classes=['xt']\n",
                    "synthetic.bin",
                    &other,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
                (
                    "unlisted-bios",
                    "Unlisted 286 BIOS",
                    "",
                    "machine_classes=['at']\n",
                    "synthetic.bin",
                    &other,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
            ] {
                let config = image_store::firmware::Config::decode(
                    format!(
                        "schema=1\nplatform='ibm-pc'\nname='{name}'\nvendor='Synthetic vendor'\n[fits]\n{fits}[[roms]]\nid='system-bios'\nrole='system-bios'\n{catalogue_id}size=32768\nsha256='{}'\nblob='{}'\n",
                        digest.hex(),
                        digest
                    )
                    .as_bytes(),
                )
                .unwrap();
                let candidate = pc_image::firmware_import::Candidate {
                    catalogue_id: tag.into(),
                    config,
                    sources: vec![roms.join(source)],
                    distribution,
                };
                pc_image::firmware_import::publish(&store, &candidate, &[], catalogue).unwrap();
            }
        }
        Some(Self { temp, store })
    }
    /// Packages, tags and restricted blobs, as the worker hands them to the wizard.
    pub fn choices(&self) -> crate::library_worker::StoreChoices {
        crate::library_worker::store_choices(&self.store).unwrap()
    }
}

/// Draws a modal on a fresh context for thirty frames of one viewport and
/// returns where it settled. egui measures a new modal in an invisible first
/// frame and centres the next one on that guess, so the height must hold from
/// the first visible frame and the whole rectangle from the one after. A modal
/// lays its content out inside last frame's size: content that fills the
/// height on offer grows the dialog frame by frame.
pub(crate) fn settled_modal(
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
pub(crate) struct Pointer {
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
pub(crate) fn painted_at(texts: &[(String, egui::Rect)], label: &str) -> egui::Rect {
    texts
        .iter()
        .find(|(shown, _)| shown == label)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{label:?} not painted in {texts:?}"))
}
