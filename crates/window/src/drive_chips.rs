// SPDX-License-Identifier: AGPL-3.0-or-later
//! Status-bar drive chips and the Media menu's per-drive labels.
//!
//! One chip per drive of the running machine names the medium in it now, its
//! format and write mode, and carries an activity light. The light follows the
//! drive's own busy state (floppy: the selected drive's motor or a pending
//! transfer; fixed disk: BSY or DRQ), sampled read-only at completed run
//! boundaries. A short hold in host UI time keeps an access shorter than a
//! frame visible; the hold never reaches the machine, so execution stays
//! deterministic. See `backlog/docs/design/machine-images.md` §7.

use crate::media_dialog::{filename, geometry_name, medium_format, FloppySlot};
use assembly::{
    DeviceId, DriveConfig, DriveKind, FloppyGeometry, HardDiskGeometry, ImageSource, MachineConfig,
    WriteMode,
};
use std::time::Duration;

/// Drive slots A:, B:, C: and D:: floppy units 0 and 1, then the IDE master
/// and slave.
pub(crate) const DRIVES: usize = 4;

/// Busy state of A:, B:, C: and D:, combined over every completed run
/// boundary since the shell last took it.
pub(crate) type DriveBusy = [bool; DRIVES];

/// Widest chip, so one long image name cannot take the whole drive area.
pub(crate) const CHIP_MAX: f32 = 300.0;

/// Where the running machine came from; names the source of its own media.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A machine file, by its library name.
    MachineFile(String),
    /// A machine image, by its reference (`name:tag`).
    Image(String),
}

impl Origin {
    fn source(&self) -> String {
        match self {
            Self::MachineFile(name) => format!("Machine file {name}"),
            Self::Image(reference) => format!("Machine image {reference}"),
        }
    }

    fn fixes_write_mode(&self) -> &'static str {
        match self {
            Self::MachineFile(_) => "fixed by the machine file",
            Self::Image(_) => "fixed by the machine image",
        }
    }
}

/// A drive bay of the running machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bay {
    Floppy {
        unit: u8,
        geometry: FloppyGeometry,
    },
    Fixed {
        unit: u8,
        geometry: HardDiskGeometry,
    },
}

/// The medium in a drive now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Medium {
    pub name: String,
    /// Library media set, machine file or image, or a file chosen with Browse….
    pub source: String,
    pub sha256: Option<[u8; 32]>,
    pub write: WriteMode,
    /// Why this medium's write mode cannot change in this session, if it cannot.
    pub fixed_by: Option<&'static str>,
    /// A floppy's own format, which may differ from its drive type's
    /// (a 720K disk in a 1.44M drive); `None` for a fixed disk.
    pub format: Option<FloppyGeometry>,
}

/// One drive as the status bar and the Media menu show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DriveView {
    pub bay: Bay,
    /// `None` is an empty floppy drive; a fixed disk always has its medium.
    pub medium: Option<Medium>,
    /// The session runs a machine image rather than a machine file.
    pub from_image: bool,
    /// The medium waits for the stopped machine's next Start.
    pub at_start: bool,
}

impl DriveView {
    /// Position among A:, B:, C: and D:, which also indexes [`DriveBusy`].
    pub fn slot(&self) -> usize {
        match self.bay {
            Bay::Floppy { unit, .. } => usize::from(unit),
            Bay::Fixed { unit, .. } => 2 + usize::from(unit),
        }
    }

    pub fn letter(&self) -> char {
        char::from(b'A' + self.slot() as u8)
    }

    /// The floppy unit, for the Media menu's Insert, Next and Eject commands.
    pub fn floppy_unit(&self) -> Option<u8> {
        match self.bay {
            Bay::Floppy { unit, .. } => Some(unit),
            Bay::Fixed { .. } => None,
        }
    }

    /// The chip's one line: image, format and write mode, or `empty`.
    pub fn chip_text(&self) -> String {
        let (head, name, tail) = self.chip_parts();
        format!("{head}{name}{tail}")
    }

    /// The chip line in three parts: the drive letter, the image name, which
    /// the chip may shorten, and the format and write mode, which it keeps.
    pub fn chip_parts(&self) -> (String, String, String) {
        let head = format!("{}: ", self.letter());
        match (&self.bay, &self.medium) {
            (Bay::Floppy { .. }, None) => (format!("{head}empty"), String::new(), String::new()),
            (Bay::Floppy { geometry, .. }, Some(medium)) => (
                head,
                medium.name.clone(),
                format!(
                    " · {} · {}{}",
                    geometry_name(medium.format.unwrap_or(*geometry)),
                    mode_short(&medium.write),
                    if self.at_start { " · at Start" } else { "" }
                ),
            ),
            (Bay::Fixed { .. }, Some(medium)) => (
                head,
                medium.name.clone(),
                format!(" · {}", mode_short(&medium.write)),
            ),
            (Bay::Fixed { .. }, None) => (format!("{head}no disk"), String::new(), String::new()),
        }
    }

    /// Why a fixed disk has no Eject: the disk belongs to the image or machine.
    pub fn fixed_eject_label(&self) -> &'static str {
        if self.from_image {
            "Eject (fixed disk, part of the image)"
        } else {
            "Eject (fixed disk, part of the machine)"
        }
    }

    /// The Media menu's heading for this drive.
    pub fn menu_label(&self) -> String {
        let letter = self.letter();
        let now = self.medium.as_ref().map_or_else(
            || "empty".to_owned(),
            |medium| {
                format!(
                    "{} · {}{}",
                    medium.name,
                    mode_short(&medium.write),
                    if self.at_start { " (at Start)" } else { "" }
                )
            },
        );
        match self.bay {
            Bay::Floppy { geometry, .. } => {
                format!("Drive {letter}: ({}) — {now}", geometry_name(geometry))
            }
            Bay::Fixed { unit, .. } => format!("{letter}: IDE {} — {now}", role(unit)),
        }
    }

    /// The write mode as the drive menu states it, with why it is fixed.
    pub fn write_label(&self) -> Option<String> {
        let medium = self.medium.as_ref()?;
        let mode = mode_title(&medium.write);
        Some(match medium.fixed_by {
            Some(reason) => format!("Write mode: {mode}, {reason}"),
            None => format!("Write mode: {mode}"),
        })
    }

    /// Hover rows: the drive, the full image name, its source, a short
    /// SHA-256, the format and the write mode, plus activity while lit.
    pub fn tooltip(&self, lit: bool) -> Vec<(&'static str, String)> {
        let letter = self.letter();
        let mut rows = Vec::new();
        match self.bay {
            Bay::Floppy { geometry, .. } => {
                let format = geometry_name(geometry);
                rows.push((
                    "Drive",
                    match form_factor(geometry) {
                        Some(form) => format!("{letter}: {form} {format}"),
                        None => format!("{letter}: {format}"),
                    },
                ));
                match &self.medium {
                    None => rows.push(("Medium", "Empty · click to insert a floppy".into())),
                    Some(medium) => {
                        medium_rows(&mut rows, medium);
                        let medium_format = medium.format.unwrap_or(geometry);
                        rows.insert(
                            rows.len() - 1,
                            (
                                "Format",
                                format!(
                                    "{} · {}/{}/{}",
                                    geometry_name(medium_format),
                                    medium_format.cylinders,
                                    medium_format.heads,
                                    medium_format.sectors_per_track
                                ),
                            ),
                        );
                    }
                }
            }
            Bay::Fixed { unit, geometry } => {
                rows.push((
                    "Drive",
                    format!(
                        "{letter}: IDE {} · {:.1} MiB, {}/{}/{}",
                        role(unit),
                        geometry.image_bytes() as f64 / (1024.0 * 1024.0),
                        geometry.cylinders,
                        geometry.heads,
                        geometry.sectors_per_track
                    ),
                ));
                if let Some(medium) = &self.medium {
                    medium_rows(&mut rows, medium);
                }
            }
        }
        if lit {
            rows.push(("Activity", "Drive busy · light on".into()));
        }
        rows
    }
}

fn medium_rows(rows: &mut Vec<(&'static str, String)>, medium: &Medium) {
    rows.push(("Image", medium.name.clone()));
    rows.push(("Source", medium.source.clone()));
    rows.push((
        "SHA-256",
        medium
            .sha256
            .as_ref()
            .map_or_else(|| "not pinned".into(), short_sha256),
    ));
    rows.push((
        "Write mode",
        match medium.fixed_by {
            Some(reason) => format!("{}; {reason}", mode_long(&medium.write)),
            None => mode_long(&medium.write).into(),
        },
    ));
}

fn role(unit: u8) -> &'static str {
    if unit == 0 {
        "master"
    } else {
        "slave"
    }
}

/// Standard PC drives: 1.2M and 360K are 5.25-inch, 1.44M and 720K 3.5-inch.
fn form_factor(geometry: FloppyGeometry) -> Option<&'static str> {
    if geometry == FloppyGeometry::HD_1_2 || geometry == FloppyGeometry::DD_360K {
        Some("5.25\"")
    } else if geometry == FloppyGeometry::HD_1_44 || geometry == FloppyGeometry::DD_720K {
        Some("3.5\"")
    } else {
        None
    }
}

fn mode_short(mode: &WriteMode) -> &'static str {
    match mode {
        WriteMode::ReadOnly => "read-only",
        WriteMode::Journal { .. } => "journal",
        WriteMode::Volatile => "volatile",
    }
}

fn mode_title(mode: &WriteMode) -> &'static str {
    match mode {
        WriteMode::ReadOnly => "Read-only",
        WriteMode::Journal { .. } => "Journal",
        WriteMode::Volatile => "Volatile",
    }
}

fn mode_long(mode: &WriteMode) -> &'static str {
    match mode {
        WriteMode::ReadOnly => "Read-only — the guest's writes are refused",
        WriteMode::Journal { .. } => {
            "Journal — changes go to a separate journal; the image stays unchanged"
        }
        WriteMode::Volatile => "Volatile — changes are lost when the machine stops",
    }
}

/// First four and last two bytes, as `9a3ec07b…41c2`.
pub(crate) fn short_sha256(digest: &[u8; 32]) -> String {
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    format!("{}…{}", hex(&digest[..4]), hex(&digest[30..]))
}

/// The dialog row of the medium a drive catalogue entry belongs to. A row keeps
/// the entries of its earlier write modes as alternates.
pub(crate) fn entry_position(slot: &FloppySlot, index: usize) -> Option<usize> {
    slot.entries.iter().position(|entry| {
        entry.index == Some(index) || entry.alternates.iter().any(|(_, other)| *other == index)
    })
}

/// Every A:–D: drive of `config` in letter order. `inserted` holds each
/// floppy unit's catalogue index and host label, `None` for an open door.
pub(crate) fn drive_views(
    config: &MachineConfig,
    slots: &[Option<FloppySlot>; 2],
    inserted: &[Option<(usize, String)>; 2],
    origin: &Origin,
) -> Vec<DriveView> {
    let mut views: Vec<DriveView> = config
        .drives
        .iter()
        .filter(|drive| drive.unit < 2)
        .filter_map(|drive| match (drive.kind, drive.controller) {
            (DriveKind::Floppy { geometry }, DeviceId::Fdc) => {
                let unit = usize::from(drive.unit);
                Some(DriveView {
                    bay: Bay::Floppy {
                        unit: drive.unit,
                        geometry,
                    },
                    medium: inserted[unit].as_ref().map(|(index, name)| {
                        floppy_medium(drive, slots[unit].as_ref(), *index, name, origin)
                    }),
                    from_image: matches!(origin, Origin::Image(_)),
                    at_start: false,
                })
            }
            (DriveKind::HardDisk { geometry }, DeviceId::Ide) => Some(DriveView {
                bay: Bay::Fixed {
                    unit: drive.unit,
                    geometry,
                },
                medium: Some(fixed_medium(drive, origin)),
                from_image: matches!(origin, Origin::Image(_)),
                at_start: false,
            }),
            _ => None,
        })
        .collect();
    views.sort_by_key(DriveView::slot);
    views
}

/// The live configuration lists every catalogue entry the drive has, in
/// controller order; the dialog's row says where a session image came from.
fn floppy_medium(
    drive: &DriveConfig,
    slot: Option<&FloppySlot>,
    index: usize,
    name: &str,
    origin: &Origin,
) -> Medium {
    let listed = drive
        .media
        .as_ref()
        .and_then(|list| list.entries.get(index));
    let (image, sha256, write) = listed.map_or(
        (&drive.image, drive.expected_sha256, &drive.write),
        |medium| (&medium.image, medium.expected_sha256, &medium.write),
    );
    let entry = slot.and_then(|slot| entry_position(slot, index).map(|row| &slot.entries[row]));
    let declared = entry.is_none_or(|entry| entry.declared);
    let source = match entry {
        Some(entry) if entry.source.is_some() => entry
            .origin
            .clone()
            .unwrap_or_else(|| "Media set from the library".into()),
        Some(_) if !declared => match image {
            ImageSource::File(path) => format!("File {}, chosen with Browse…", path.display()),
            _ => "Chosen with Browse…".into(),
        },
        _ => {
            let declared_count = slot.map_or(1, |slot| {
                slot.entries.iter().filter(|entry| entry.declared).count()
            });
            if declared_count > 1 {
                format!(
                    "{} · disk {} of {declared_count}",
                    origin.source(),
                    index + 1
                )
            } else {
                origin.source()
            }
        }
    };
    let name = if name.is_empty() {
        match image {
            ImageSource::File(path) => filename(path),
            _ => format!("disk {}", index + 1),
        }
    } else {
        name.to_owned()
    };
    let format = match drive.kind {
        DriveKind::Floppy { geometry } => Some(
            entry
                .and_then(|entry| entry.geometry)
                .unwrap_or_else(|| medium_format(geometry, image)),
        ),
        DriveKind::HardDisk { .. } => None,
    };
    Medium {
        name,
        source,
        sha256,
        write: write.clone(),
        fixed_by: declared.then(|| origin.fixes_write_mode()),
        format,
    }
}

fn fixed_medium(drive: &DriveConfig, origin: &Origin) -> Medium {
    let (name, source) = match (origin, &drive.image) {
        (Origin::Image(reference), _) => (reference.clone(), origin.source()),
        (Origin::MachineFile(_), ImageSource::File(path)) => (
            filename(path),
            format!("{} · {}", origin.source(), path.display()),
        ),
        (Origin::MachineFile(_), ImageSource::Blank { .. }) => {
            ("blank disk".into(), origin.source())
        }
        (Origin::MachineFile(_), _) => ("inline image".into(), origin.source()),
    };
    Medium {
        name,
        source,
        sha256: drive.expected_sha256,
        write: drive.write.clone(),
        fixed_by: Some(origin.fixes_write_mode()),
        format: None,
    }
}

/// Each light stays on for a short host-time hold after its last busy sample.
/// Host UI time only: the machine never reads it.
#[derive(Debug, Default)]
pub(crate) struct DriveLights {
    until: [Option<Duration>; DRIVES],
}

impl DriveLights {
    /// Long enough to see a single sector access, short enough to look live.
    pub(crate) const HOLD: Duration = Duration::from_millis(200);

    /// Record one frame's busy samples, taken at host time `now`.
    pub fn observe(&mut self, busy: DriveBusy, now: Duration) {
        for (until, busy) in self.until.iter_mut().zip(busy) {
            if busy {
                *until = Some(now.saturating_add(Self::HOLD));
            }
        }
    }

    pub fn lit(&self, slot: usize, now: Duration) -> bool {
        self.until
            .get(slot)
            .copied()
            .flatten()
            .is_some_and(|until| now < until)
    }

    /// Host time until the next lit light goes out, so the shell repaints then.
    pub fn next_off(&self, now: Duration) -> Option<Duration> {
        self.until
            .iter()
            .flatten()
            .filter(|until| **until > now)
            .min()
            .map(|until| *until - now)
    }
}

/// Chip widths inside the fixed drive area. Each chip wants its natural width
/// up to [`CHIP_MAX`] and needs at least `minimum`, the width that still shows
/// its format and write mode. When the wants do not all fit, every chip keeps
/// its minimum and the rest is shared, narrow demands first. When even the
/// minimums do not fit, all chips shrink in proportion. The widths never sum
/// past `area`, so no chip starts beyond its edge; the area itself never grows,
/// so the cells beside it keep their places whatever the drives hold.
pub(crate) fn chip_widths(area: f32, natural: &[f32], minimum: &[f32]) -> Vec<f32> {
    let area = area.max(0.0);
    let wanted: Vec<f32> = natural.iter().map(|width| width.min(CHIP_MAX)).collect();
    if wanted.iter().sum::<f32>() <= area {
        return wanted;
    }
    let floor: Vec<f32> = wanted
        .iter()
        .zip(minimum.iter().chain(std::iter::repeat(&0.0)))
        .map(|(wanted, minimum)| minimum.clamp(0.0, *wanted))
        .collect();
    let floors: f32 = floor.iter().sum();
    if floors >= area {
        let scale = if floors > 0.0 { area / floors } else { 0.0 };
        return floor.iter().map(|width| width * scale).collect();
    }
    let extra: Vec<f32> = wanted.iter().zip(&floor).map(|(w, f)| w - f).collect();
    let mut order: Vec<usize> = (0..extra.len()).collect();
    order.sort_by(|left, right| extra[*left].total_cmp(&extra[*right]));
    let mut left = area - floors;
    let mut widths = floor;
    for (done, index) in order.iter().enumerate() {
        let share = (left / (order.len() - done) as f32).min(extra[*index]);
        widths[*index] += share;
        left -= share;
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_dialog::CatalogueEntry;
    use assembly::FloppyMedium;

    fn floppy_drive(unit: u8, geometry: FloppyGeometry) -> DriveConfig {
        DriveConfig {
            media: None,
            kind: DriveKind::Floppy { geometry },
            image: ImageSource::Blank {
                size: geometry.image_bytes(),
            },
            expected_sha256: None,
            controller: DeviceId::Fdc,
            unit,
            write: WriteMode::ReadOnly,
        }
    }

    fn hard_drive(image: ImageSource, write: WriteMode) -> DriveConfig {
        DriveConfig {
            media: None,
            kind: DriveKind::HardDisk {
                geometry: HardDiskGeometry {
                    cylinders: 615,
                    heads: 4,
                    sectors_per_track: 17,
                    bytes_per_sector: 512,
                },
            },
            image,
            expected_sha256: Some([0xAB; 32]),
            controller: DeviceId::Ide,
            unit: 0,
            write,
        }
    }

    fn medium(name: &str, write: WriteMode, sha: Option<[u8; 32]>) -> FloppyMedium {
        FloppyMedium {
            image: ImageSource::File(format!("/media/{name}.img").into()),
            expected_sha256: sha,
            write,
            name: name.into(),
        }
    }

    /// A machine-file A: drive with two declared disks, a B: drive and an IDE C:.
    fn machine() -> MachineConfig {
        let mut config = assembly::bios(vec![0xF4; 32768], None);
        config.drives.clear();
        let mut a = floppy_drive(0, FloppyGeometry::HD_1_2);
        a.media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: vec![
                medium("FreeDOS 1.2 boot", WriteMode::ReadOnly, Some([0x9A; 32])),
                medium("tools", WriteMode::journal("/journals/tools.upfj"), None),
            ],
        });
        config.drives.push(hard_drive(
            ImageSource::File("/disks/dos.img".into()),
            WriteMode::journal("/journals/dos.upfj"),
        ));
        config.drives.push(floppy_drive(1, FloppyGeometry::DD_360K));
        config.drives.push(a);
        config
    }

    fn slots(config: &MachineConfig) -> [Option<FloppySlot>; 2] {
        FloppySlot::from_config(config)
    }

    fn origin() -> Origin {
        Origin::MachineFile("AT games".into())
    }

    #[test]
    fn chips_follow_letter_order_and_name_each_drive_state() {
        let config = machine();
        let views = drive_views(
            &config,
            &slots(&config),
            &[Some((0, "FreeDOS 1.2 boot".into())), None],
            &origin(),
        );
        let texts: Vec<String> = views.iter().map(DriveView::chip_text).collect();
        assert_eq!(
            texts,
            [
                "A: FreeDOS 1.2 boot · 1.2M · read-only",
                "B: empty",
                "C: dos.img · journal",
            ]
        );
        assert_eq!(
            views.iter().map(DriveView::letter).collect::<String>(),
            "ABC"
        );
        assert_eq!(
            views
                .iter()
                .map(|view| view.floppy_unit())
                .collect::<Vec<_>>(),
            [Some(0), Some(1), None]
        );
    }

    #[test]
    fn chips_name_the_medium_format_in_a_mixed_density_drive() {
        let temp = assembly::test_support::TempDir::new("chips-mixed-density");
        let game = temp.path().join("game.img");
        std::fs::write(&game, vec![0; 737_280]).unwrap();
        let mut config = machine();
        config
            .drives
            .retain(|drive| drive.unit != 1 || drive.controller != DeviceId::Fdc);
        let mut b = floppy_drive(1, FloppyGeometry::HD_1_44);
        b.media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: vec![FloppyMedium {
                image: ImageSource::File(game),
                expected_sha256: None,
                write: WriteMode::ReadOnly,
                name: "game.img".into(),
            }],
        });
        config.drives.push(b);
        let views = drive_views(
            &config,
            &slots(&config),
            &[None, Some((0, "game.img".into()))],
            &origin(),
        );
        let b = views.iter().find(|view| view.letter() == 'B').unwrap();
        assert_eq!(b.chip_text(), "B: game.img · 720K · read-only");
        assert_eq!(b.menu_label(), "Drive B: (1.44M) — game.img · read-only");
        let tooltip = b.tooltip(false);
        assert!(
            tooltip.contains(&("Drive", "B: 3.5\" 1.44M".into())),
            "{tooltip:?}"
        );
        assert!(
            tooltip.contains(&("Format", "720K · 80/2/9".into())),
            "{tooltip:?}"
        );
    }

    #[test]
    fn chip_parts_keep_format_and_mode_apart_from_the_name() {
        let config = machine();
        let views = drive_views(
            &config,
            &slots(&config),
            &[Some((0, "FreeDOS 1.2 boot".into())), None],
            &origin(),
        );
        assert_eq!(
            views[0].chip_parts(),
            (
                "A: ".to_owned(),
                "FreeDOS 1.2 boot".to_owned(),
                " · 1.2M · read-only".to_owned()
            )
        );
        assert_eq!(
            views[1].chip_parts(),
            ("B: empty".to_owned(), String::new(), String::new())
        );
        assert_eq!(
            views[2].chip_parts(),
            (
                "C: ".to_owned(),
                "dos.img".to_owned(),
                " · journal".to_owned()
            )
        );
        assert_eq!(
            views[2].fixed_eject_label(),
            "Eject (fixed disk, part of the machine)"
        );
        let image = Origin::Image("open-at-dos:1.0.0".into());
        let views = drive_views(&config, &slots(&config), &[None, None], &image);
        assert_eq!(
            views[2].fixed_eject_label(),
            "Eject (fixed disk, part of the image)"
        );
    }

    #[test]
    fn media_menu_labels_show_the_current_image_and_write_mode() {
        let config = machine();
        let views = drive_views(
            &config,
            &slots(&config),
            &[Some((1, "tools".into())), None],
            &origin(),
        );
        let labels: Vec<String> = views.iter().map(DriveView::menu_label).collect();
        assert_eq!(
            labels,
            [
                "Drive A: (1.2M) — tools · journal",
                "Drive B: (360K) — empty",
                "C: IDE master — dos.img · journal",
            ]
        );
        assert_eq!(
            views[0].write_label().unwrap(),
            "Write mode: Journal, fixed by the machine file"
        );
        assert_eq!(views[1].write_label(), None);
    }

    #[test]
    fn floppy_tooltip_has_name_source_short_hash_format_and_write_mode() {
        let config = machine();
        let views = drive_views(
            &config,
            &slots(&config),
            &[Some((0, "FreeDOS 1.2 boot".into())), None],
            &origin(),
        );
        assert_eq!(
            views[0].tooltip(false),
            [
                ("Drive", "A: 5.25\" 1.2M".to_owned()),
                ("Image", "FreeDOS 1.2 boot".into()),
                ("Source", "Machine file AT games · disk 1 of 2".into()),
                ("SHA-256", "9a9a9a9a…9a9a".into()),
                ("Format", "1.2M · 80/2/15".into()),
                (
                    "Write mode",
                    "Read-only — the guest's writes are refused; fixed by the machine file".into()
                ),
            ]
        );
        assert_eq!(
            views[1].tooltip(false),
            [
                ("Drive", "B: 5.25\" 360K".to_owned()),
                ("Medium", "Empty · click to insert a floppy".into()),
            ]
        );
        let lit = views[1].tooltip(true);
        assert_eq!(
            lit.last().unwrap(),
            &("Activity", "Drive busy · light on".to_owned())
        );
    }

    #[test]
    fn unpinned_journal_and_ide_tooltips_say_so() {
        let config = machine();
        let views = drive_views(
            &config,
            &slots(&config),
            &[Some((1, "tools".into())), None],
            &origin(),
        );
        let rows = views[0].tooltip(false);
        assert!(rows.contains(&("SHA-256", "not pinned".into())));
        assert!(rows.contains(&("Source", "Machine file AT games · disk 2 of 2".into())));
        assert!(rows.iter().any(|(key, value)| *key == "Write mode"
            && value.starts_with("Journal — changes go to a separate journal")));
        assert_eq!(
            views[2].tooltip(false),
            [
                ("Drive", "C: IDE master · 20.4 MiB, 615/4/17".to_owned()),
                ("Image", "dos.img".into()),
                ("Source", "Machine file AT games · /disks/dos.img".into()),
                ("SHA-256", "abababab…abab".into()),
                (
                    "Write mode",
                    "Journal — changes go to a separate journal; the image stays unchanged; fixed by the machine file"
                        .into()
                ),
            ]
        );
    }

    #[test]
    fn image_sessions_name_the_image_reference_for_the_fixed_disk() {
        let mut config = machine();
        config
            .drives
            .retain(|drive| drive.controller == DeviceId::Ide);
        let image = Origin::Image("open-at-dos:1.0.0".into());
        let views = drive_views(&config, &[None, None], &[None, None], &image);
        assert_eq!(views[0].chip_text(), "C: open-at-dos:1.0.0 · journal");
        let rows = views[0].tooltip(false);
        assert!(rows.contains(&("Source", "Machine image open-at-dos:1.0.0".into())));
        assert!(rows
            .iter()
            .any(|(key, value)| *key == "Write mode"
                && value.ends_with("fixed by the machine image")));
        config.drives[0].image = ImageSource::Blank { size: 0 };
        let views = drive_views(&config, &[None, None], &[None, None], &origin());
        assert_eq!(views[0].chip_text(), "C: blank disk · journal");
    }

    fn library_entry(index: usize, write: WriteMode) -> CatalogueEntry {
        CatalogueEntry {
            source: Some(image_store::media_set::CatalogueSource {
                set: image_store::Digest::of(b"set"),
                medium_id: "boot".into(),
                blob: image_store::Digest::of(b"boot"),
            }),
            receipt: None,
            readonly_bytes: None,
            medium: medium("FreeDOS boot", write, Some([0x11; 32])),
            geometry: Some(FloppyGeometry::HD_1_2),
            reason: None,
            index: Some(index),
            declared: false,
            alternates: Vec::new(),
            workspace_journal: None,
            origin: Some("Media set freedos:1.2.0 from the library · disk 1 of 2".into()),
        }
    }

    #[test]
    fn media_set_and_browsed_floppies_name_their_source_and_may_switch_mode() {
        let mut config = machine();
        // Slots come from the machine as it started; session disks follow.
        let mut slots = slots(&config);
        let list = config.drives[2].media.as_mut().unwrap();
        list.entries.push(medium(
            "FreeDOS boot",
            WriteMode::ReadOnly,
            Some([0x11; 32]),
        ));
        list.entries.push(medium(
            "FreeDOS boot",
            WriteMode::journal("/workspace/boot.upfj"),
            Some([0x11; 32]),
        ));
        list.entries
            .push(medium("utils-360", WriteMode::ReadOnly, Some([0x22; 32])));
        let slot = slots[0].as_mut().unwrap();
        let mut set = library_entry(3, WriteMode::journal("/workspace/boot.upfj"));
        set.alternates.push((WriteMode::ReadOnly, 2));
        slot.entries.push(set);
        let mut browsed = library_entry(4, WriteMode::ReadOnly);
        browsed.source = None;
        browsed.origin = None;
        browsed.medium = medium("utils-360", WriteMode::ReadOnly, Some([0x22; 32]));
        slot.entries.push(browsed);

        for (index, mode) in [(2, "read-only"), (3, "journal")] {
            let views = drive_views(
                &config,
                &slots,
                &[Some((index, "FreeDOS boot".into())), None],
                &origin(),
            );
            assert_eq!(
                views[0].chip_text(),
                format!("A: FreeDOS boot · 1.2M · {mode}")
            );
            let rows = views[0].tooltip(false);
            assert!(rows.contains(&(
                "Source",
                "Media set freedos:1.2.0 from the library · disk 1 of 2".into()
            )));
            assert!(rows.contains(&("SHA-256", "11111111…1111".into())));
            assert_eq!(views[0].medium.as_ref().unwrap().fixed_by, None);
            assert_eq!(entry_position(slots[0].as_ref().unwrap(), index), Some(2));
        }
        let views = drive_views(
            &config,
            &slots,
            &[Some((4, "utils-360".into())), None],
            &origin(),
        );
        assert!(views[0].tooltip(false).contains(&(
            "Source",
            "File /media/utils-360.img, chosen with Browse…".into()
        )));
        assert_eq!(views[0].write_label().unwrap(), "Write mode: Read-only");
    }

    #[test]
    fn unnamed_media_fall_back_to_the_file_name() {
        let config = machine();
        let views = drive_views(
            &config,
            &slots(&config),
            &[Some((0, String::new())), None],
            &origin(),
        );
        assert_eq!(
            views[0].chip_text(),
            "A: FreeDOS 1.2 boot.img · 1.2M · read-only"
        );
    }

    #[test]
    fn lights_hold_after_the_last_busy_sample_and_then_go_out() {
        let ms = Duration::from_millis;
        let mut lights = DriveLights::default();
        assert!(!lights.lit(0, ms(0)));
        assert_eq!(lights.next_off(ms(0)), None);
        lights.observe([true, false, false, true], ms(0));
        assert!(lights.lit(0, ms(0)) && lights.lit(3, ms(0)));
        assert!(!lights.lit(1, ms(0)) && !lights.lit(2, ms(0)));
        assert!(lights.lit(0, ms(199)));
        assert_eq!(lights.next_off(ms(50)), Some(ms(150)));
        lights.observe([false, false, false, true], ms(150));
        assert!(lights.lit(0, ms(199)), "an idle sample keeps the hold");
        assert!(!lights.lit(0, ms(200)), "the hold ends 200 ms after busy");
        assert!(lights.lit(3, ms(349)));
        assert!(!lights.lit(3, ms(350)));
        assert_eq!(lights.next_off(ms(350)), None);
        assert!(!lights.lit(DRIVES, ms(0)));
    }

    #[test]
    fn chips_keep_their_width_until_the_area_is_full_then_share_it() {
        let none = [0.0; 3];
        assert_eq!(chip_widths(500.0, &[200.0, 60.0], &none), [200.0, 60.0]);
        assert_eq!(chip_widths(900.0, &[400.0], &none), [CHIP_MAX]);
        assert_eq!(
            chip_widths(200.0, &[200.0, 60.0, 150.0], &none),
            [70.0, 60.0, 70.0]
        );
        let widths = chip_widths(90.0, &[200.0, 200.0, 200.0], &none);
        assert_eq!(widths, [30.0, 30.0, 30.0]);
        assert_eq!(chip_widths(0.0, &[50.0], &none), [0.0]);
        assert!(chip_widths(100.0, &[], &[]).is_empty());
    }

    #[test]
    fn chips_keep_their_format_and_mode_width_before_names_share_the_rest() {
        // A: wants 246 and needs 156, C: wants 142 and needs 104: the 36 px
        // past both minimums are shared, so A: no longer squeezes C: below
        // the width that shows its write mode.
        assert_eq!(
            chip_widths(296.0, &[246.0, 142.0], &[156.0, 104.0]),
            [174.0, 122.0]
        );
        assert_eq!(
            chip_widths(270.0, &[246.0, 142.0], &[156.0, 104.0]),
            [161.0, 109.0]
        );
        // A minimum larger than the chip's want counts only as the want.
        assert_eq!(
            chip_widths(100.0, &[60.0, 90.0], &[80.0, 0.0]),
            [60.0, 40.0]
        );
    }

    #[test]
    fn chip_widths_never_run_past_the_area_edge() {
        for area in [0.0, 1.0, 50.0, 150.0, 260.0, 299.0, 900.0] {
            for (natural, minimum) in [
                (vec![246.0, 78.0, 142.0], vec![156.0, 78.0, 104.0]),
                (vec![400.0; 4], vec![300.0; 4]),
                (vec![10.0], vec![10.0]),
            ] {
                let widths = chip_widths(area, &natural, &minimum);
                assert_eq!(widths.len(), natural.len());
                assert!(widths.iter().all(|width| *width >= 0.0));
                assert!(
                    widths.iter().sum::<f32>() <= area + 0.001,
                    "{area} {widths:?}"
                );
            }
        }
        // Even the minimums overflow: every chip shrinks in proportion.
        assert_eq!(
            chip_widths(150.0, &[400.0, 400.0], &[200.0, 100.0]),
            [100.0, 50.0]
        );
    }

    #[test]
    fn short_hash_keeps_the_first_four_and_last_two_bytes() {
        let mut digest = [0; 32];
        digest[..4].copy_from_slice(&[0x9a, 0x3e, 0xc0, 0x7b]);
        digest[30..].copy_from_slice(&[0x41, 0xc2]);
        assert_eq!(short_sha256(&digest), "9a3ec07b…41c2");
    }
}
