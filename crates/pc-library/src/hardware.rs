// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the details panes say about an image's hardware, drives and
//! firmware, in the words the capabilities sheet and the settings sheet use:
//! the board's display name, card names, drive letters with their drive
//! types and media, and firmware roles with catalogue or library names. The
//! readiness check names drives by controller and unit and firmware by its
//! configuration key; those keys never reach a pane.
//!
//! Drive letters are the ones the machine store gives its journals: diskette
//! units 0 and 1 are A: and B:, the IDE master and slave C: and D:, the
//! letters DOS gives the BIOS's diskette drives 00h–01h and fixed disks
//! 80h–81h (IBM Personal Computer AT Technical Reference, BIOS listing,
//! INT 13h).

use crate::firmware::is_rom;
use assembly::capabilities::{self as caps, Draft, Field, Group, Value};
use assembly::descriptors::CardKind;
use assembly::DeviceKind;
use image_store::{license, summary::ImageSummary, Descriptor, Digest};
use library_model::listing::human_size;
use pc_image::inspection::{ImageDetails, MediumDetails};
use std::collections::BTreeMap;

/// What a readiness check reads beside an image's details, for the details
/// panes and for floppies inserted before Start.
#[derive(Clone, Debug, Default)]
pub struct CheckedImage {
    /// The capabilities draft with empty floppy catalogues, as Settings reads
    /// it: the drives a floppy goes into and the names of the hardware.
    pub draft: Option<assembly::capabilities::Draft>,
    /// Every card of the configuration by its id, with its kind.
    pub cards: Vec<(String, String)>,
    /// The manifest's description.
    pub description: Option<String>,
    /// The image this one was saved from, as the manifest records it.
    pub base: Option<Digest>,
}

/// An image's hardware, drives and firmware as the panes name them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageFacts {
    /// One line, as the machines mock-up writes it: board, processor,
    /// memory and cards.
    pub hardware: String,
    /// The drives by letter, then the CMOS row.
    pub drives: Vec<DriveFact>,
    pub firmware: Vec<FirmwareFact>,
}

/// One drive of an image and what it holds there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveFact {
    /// `A:`, `C:` or `CMOS`.
    pub drive: String,
    /// The drive type and its media, such as "1.2M · empty catalogue".
    pub described: String,
}

/// One ROM of an image by its role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareFact {
    /// System BIOS, Video BIOS or Option ROM.
    pub role: String,
    /// The catalogue or library name, else the start of its digest.
    pub name: String,
    /// Open or user-supplied, when the ROM's layers could be read: open only
    /// when every chip of the ROM is open and the firmware catalogue does not
    /// list the ROM's bytes as proprietary. A catalogue entry listed as
    /// user-supplied is user-supplied also before its layers are read.
    pub open: Option<bool>,
}

/// The CMOS row's drive name, as the machine store names its journal.
pub const CMOS: &str = "CMOS";

impl ImageFacts {
    /// `checked` is what the check read from the image's configuration and
    /// `summary` its library summary; `rom_names` names the library's
    /// firmware packages by the raw identity of each ROM, for ROMs the board
    /// catalogue does not list.
    pub fn new(
        details: &ImageDetails,
        checked: Option<&CheckedImage>,
        summary: Option<&ImageSummary>,
        rom_names: &BTreeMap<Digest, String>,
    ) -> Self {
        let draft = checked.and_then(|c| c.draft.as_ref());
        let cards = checked.map_or(&[][..], |c| c.cards.as_slice());
        let fields = draft.map(caps::fields).unwrap_or_default();
        Self {
            hardware: match draft {
                Some(draft) => hardware(draft, &fields),
                None => hardware_without_draft(details),
            },
            drives: drives(details, draft, cards),
            firmware: firmware(details, draft, &fields, summary, rom_names),
        }
    }
}

/// Kibibytes as the mock-ups write memory: whole mebibytes in MiB.
fn kib(value: u64) -> String {
    if value >= 1024 && value.is_multiple_of(1024) {
        format!("{} MiB", value / 1024)
    } else {
        format!("{value} KiB")
    }
}

/// A card in the machines mock-up's words; the floppy controller every AT
/// has goes unnamed.
fn card(device: &assembly::DeviceConfig) -> Option<String> {
    let irq = device.irq;
    let dma = device.dma.map(|d| d.channel);
    Some(match &device.kind {
        DeviceKind::Sb16(cfg) => {
            let mut text = format!("SB16 at {:X}h", cfg.base);
            let mut wiring = Vec::new();
            if let Some(irq) = irq {
                wiring.push(format!("IRQ {irq}"));
            }
            if let Some(dma) = dma {
                wiring.push(format!("DMA {dma} and {}", cfg.dma16));
            }
            if !wiring.is_empty() {
                text.push_str(&format!(" ({})", wiring.join(", ")));
            }
            text
        }
        DeviceKind::Uart(cfg) => {
            let port = match cfg.base {
                0x3F8 => "COM1".to_owned(),
                0x2F8 => "COM2".to_owned(),
                0x3E8 => "COM3".to_owned(),
                0x2E8 => "COM4".to_owned(),
                base => format!("the serial port at {base:X}h"),
            };
            if cfg.mouse {
                format!("Microsoft mouse on {port}")
            } else {
                format!("{port} without a mouse")
            }
        }
        DeviceKind::LotechEms(cfg) => format!(
            "Lo-tech EMS {} at {:X}h, frame {:X}h",
            kib(u64::from(cfg.size_kib)),
            cfg.port,
            cfg.frame
        ),
        DeviceKind::Ide(cfg) => format!("ATA at {:X}h", cfg.base),
        kind => match CardKind::of(kind)? {
            CardKind::Vga => "VGA".into(),
            CardKind::VgaText => "VGA text".into(),
            CardKind::Mda => "MDA".into(),
            _ => return None,
        },
    })
}

fn hardware(draft: &Draft, fields: &[Field]) -> String {
    let field = |path: &str| fields.iter().find(|f| f.path == path);
    let label = |f: &Field| caps::value_label(draft, &f.path, &f.value);
    let integer = |path: &str| match field(path).map(|f| &f.value) {
        Some(Value::Integer(value)) => Some(u64::from(*value)),
        _ => None,
    };
    let mut parts = Vec::new();
    if let Some(board) = field("board.model").map(label) {
        // "IBM 5170 (AT)" in the settings sheet is the mock-up's "IBM 5170-class board".
        let name = board.split(" (").next().unwrap_or(&board).trim();
        parts.push(format!("{name}-class board"));
    }
    if let Some(cpu) = field("cpu.model").map(label) {
        let config = &draft.config;
        let rate = (
            config.clock.retired_per_pit_clock,
            config.clock.retired_per_pit_clock_denominator,
        );
        let speed = pc_image::create::speeds(config.board)
            .into_iter()
            .find(|s| (u64::from(s.steps), u64::from(s.pit_clocks)) == rate);
        parts.push(match speed {
            Some(speed) => format!("{cpu}, {} MHz ({}/{})", speed.mhz, rate.0, rate.1),
            None => format!("{cpu} ({}/{})", rate.0, rate.1),
        });
    }
    if let Some(fpu) = field("cpu.fpu").filter(|f| f.value != Value::Name("none".into())) {
        parts.push(label(fpu));
    }
    match (
        integer("memory.conventional_kib"),
        integer("memory.extended_kib"),
    ) {
        (Some(base), Some(extended)) if extended > 0 => {
            parts.push(format!("{} + {}", kib(base), kib(extended)))
        }
        (Some(base), _) => parts.push(kib(base)),
        _ => (),
    }
    parts.extend(draft.config.devices.iter().filter_map(card));
    parts.join(" · ")
}

/// Without a draft the check's own lines stay, less the board's internal id.
fn hardware_without_draft(details: &ImageDetails) -> String {
    details
        .hardware
        .iter()
        .flat_map(|line| line.split(" · "))
        .filter(|part| !part.starts_with("Board: "))
        .map(|part| {
            part.trim_start_matches("RAM: ")
                .trim_start_matches("Devices: ")
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The configuration name of the card a drive hangs on: the card's own id,
/// else its model name, as the image's slots record it.
pub fn controller_name(draft: &Draft, id: assembly::DeviceId) -> String {
    draft
        .card_ids
        .iter()
        .find(|(role, _)| *role == id)
        .map(|(_, name)| name.clone())
        .or_else(|| {
            draft
                .config
                .devices
                .iter()
                .find(|d| d.id == id)
                .and_then(|d| CardKind::of(&d.kind))
                .map(|kind| kind.name().to_owned())
        })
        .unwrap_or_default()
}

/// The drive letter of `unit` counted from `first`, if there is one.
pub fn letter(first: u8, unit: u8) -> Option<String> {
    first
        .checked_add(unit)
        .filter(u8::is_ascii_uppercase)
        .map(|letter| format!("{}:", char::from(letter)))
}

/// A medium's name, or its place in the catalogue when it has none.
pub fn medium_name(index: usize, medium: &MediumDetails) -> String {
    if medium.name.is_empty() || medium.name == medium.id {
        format!("disk {}", index + 1)
    } else {
        medium.name.clone()
    }
}

fn floppy(kind: &str, media: &[&MediumDetails]) -> String {
    if media.is_empty() {
        return format!("{kind} · empty catalogue");
    }
    let names: Vec<String> = media
        .iter()
        .enumerate()
        .map(|(i, m)| medium_name(i, m))
        .collect();
    format!("{kind} · catalogue: {}", names.join(", "))
}

fn fixed_disk(geometry: assembly::HardDiskGeometry, medium: Option<&MediumDetails>) -> String {
    let size = human_size(geometry.image_bytes());
    let Some(medium) = medium else {
        return format!("{size} disk, no medium");
    };
    if medium.chain.len() == 1 && medium.chain[0].blank_bytes.is_some() {
        return format!("{size} blank disk (zero base)");
    }
    let mut text = format!(
        "{size} disk ({}/{}/{})",
        geometry.cylinders, geometry.heads, geometry.sectors_per_track
    );
    if !(medium.name.is_empty() || medium.name == medium.id) {
        text.push_str(&format!(" · {}", medium.name));
    }
    if medium.chain.len() > 1 {
        text.push_str(&format!(" · {} layers", medium.chain.len()));
    }
    text
}

fn drives(
    details: &ImageDetails,
    draft: Option<&Draft>,
    cards: &[(String, String)],
) -> Vec<DriveFact> {
    let mut rows = Vec::new();
    match draft {
        Some(draft) => {
            for drive in &draft.config.drives {
                let key = format!(
                    "{}:{}",
                    controller_name(draft, drive.controller),
                    drive.unit
                );
                let media: Vec<&MediumDetails> =
                    details.media.iter().filter(|m| m.drive == key).collect();
                let (first, described) = match drive.kind {
                    assembly::DriveKind::Floppy { geometry } => (
                        b'A',
                        floppy(
                            assembly::descriptors::floppy_type_name(geometry).unwrap_or("Diskette"),
                            &media,
                        ),
                    ),
                    assembly::DriveKind::HardDisk { geometry } => {
                        (b'C', fixed_disk(geometry, media.first().copied()))
                    }
                };
                if let Some(drive) = letter(first, drive.unit) {
                    rows.push(DriveFact { drive, described });
                }
            }
        }
        // Without a draft the check's slots still tell the drives apart: the
        // store names an IDE card's units from C: and every other card's
        // from A:, by the card's kind.
        None => {
            let mut keys: Vec<&str> = details
                .media
                .iter()
                .map(|m| m.drive.as_str())
                .filter(|key| *key != CMOS && key.contains(':'))
                .collect();
            keys.dedup();
            for key in keys {
                let Some((controller, unit)) = key.rsplit_once(':') else {
                    continue;
                };
                let Ok(unit) = unit.parse::<u8>() else {
                    continue;
                };
                let ide = cards
                    .iter()
                    .any(|(id, kind)| id == controller && kind == "ide");
                let media: Vec<&MediumDetails> =
                    details.media.iter().filter(|m| m.drive == key).collect();
                let names: Vec<String> = media
                    .iter()
                    .enumerate()
                    .map(|(i, m)| medium_name(i, m))
                    .collect();
                if let Some(drive) = letter(if ide { b'C' } else { b'A' }, unit) {
                    rows.push(DriveFact {
                        drive,
                        described: names.join(", "),
                    });
                }
            }
        }
    }
    rows.sort_by(|a, b| a.drive.cmp(&b.drive));
    rows.dedup_by(|a, b| a.drive == b.drive);
    rows.push(DriveFact {
        drive: CMOS.into(),
        described: match details.media.iter().find(|m| m.drive == CMOS) {
            Some(cmos) if cmos.policy == pc_image::Policy::Journal => format!(
                "Stored, {}",
                human_size(cmos.chain.first().map_or(0, |c| c.size))
            ),
            Some(cmos) => format!(
                "Stored, {}; each start begins from it",
                human_size(cmos.chain.first().map_or(0, |c| c.size))
            ),
            None => "Derived at Start from the configuration".into(),
        },
    });
    rows
}

/// The digest starts the check reports for a configuration key: one for a
/// single ROM, one per chip for a ROM in parts.
fn rom_hexes<'a>(details: &'a ImageDetails, key: &str) -> Vec<&'a str> {
    let prefix = format!("{key}.");
    details
        .firmware
        .iter()
        .filter_map(|line| line.split_once(" · "))
        .filter(|(k, _)| *k == key || k.starts_with(&prefix))
        .map(|(_, hex)| hex)
        .collect()
}

/// The image's ROM layers of a configuration key, every chip of it.
fn rom_layers<'a>(
    hexes: &[&str],
    summary: Option<&'a ImageSummary>,
) -> Option<Vec<&'a Descriptor>> {
    let summary = summary?;
    let layers: Vec<&Descriptor> = hexes
        .iter()
        .filter_map(|hex| {
            summary.storage_blobs.iter().find(|d| {
                is_rom(d)
                    && d.decoded_digest()
                        .is_ok_and(|digest| digest.hex().starts_with(hex))
            })
        })
        .collect();
    (!layers.is_empty() && layers.len() == hexes.len()).then_some(layers)
}

/// A ROM the board catalogue does not name: its library package's name, else
/// the start of its digest as the check reports it.
fn library_name(
    hexes: &[&str],
    layers: Option<&[&Descriptor]>,
    rom_names: &BTreeMap<Digest, String>,
) -> String {
    layers
        .into_iter()
        .flatten()
        .filter_map(|d| d.decoded_digest().ok())
        .find_map(|digest| rom_names.get(&digest).cloned())
        .unwrap_or_else(|| match hexes.first() {
            Some(hex) => format!("ROM {hex}"),
            None => "ROM".into(),
        })
}

fn role_of_key(key: &str) -> &'static str {
    if key.starts_with("firmware.bios") {
        "System BIOS"
    } else {
        "Option ROM"
    }
}

/// Whether a ROM layer may be called open: it is marked open, and the
/// firmware catalogue does not list its bytes, stored or decoded, whole or as
/// a chip, as proprietary. Rights follow the bytes, as the push guard and the
/// settings sheet rule, so no mark opens a ROM the catalogue lists as
/// user-supplied.
fn layer_open(layer: &Descriptor) -> bool {
    let catalogue = pc_image::firmware::catalogue();
    let listed = |digest: &Digest, size: u64| {
        pc_image::firmware::proprietary(catalogue, digest.hex(), size).is_some()
    };
    let decoded_listed = || match (layer.decoded_digest(), layer.decoded_size()) {
        (Ok(digest), Ok(size)) => listed(&digest, size),
        // Bytes whose identity cannot be read are not known to be open.
        _ => true,
    };
    license::open_license(layer).is_some()
        && !listed(&layer.digest, layer.size)
        && !(layer.is_compressed() && decoded_listed())
}

fn firmware(
    details: &ImageDetails,
    draft: Option<&Draft>,
    fields: &[Field],
    summary: Option<&ImageSummary>,
    rom_names: &BTreeMap<Digest, String>,
) -> Vec<FirmwareFact> {
    // Open only when every chip is, and never when the catalogue entry the
    // configuration names is user-supplied; without the layers, such an
    // entry is still known to be user-supplied.
    let open = |layers: Option<&[&Descriptor]>, entry: Option<&assembly::FirmwareEntry>| {
        let listed_open = entry.is_none_or(crate::firmware_choices::catalogue_open);
        match layers {
            Some(layers) => Some(listed_open && layers.iter().all(|d| layer_open(d))),
            None => (!listed_open).then_some(false),
        }
    };
    let mut rows: Vec<FirmwareFact> = match draft {
        Some(draft) => fields
            .iter()
            .filter(|f| f.group == Group::Firmware && f.value != Value::Disconnected)
            .map(|f| {
                let hexes = rom_hexes(details, f.path.trim_end_matches(".image"));
                let layers = rom_layers(&hexes, summary);
                let entry = match &f.value {
                    Value::Name(id) => crate::firmware_choices::entry(id),
                    _ => None,
                };
                FirmwareFact {
                    role: f.label.clone(),
                    name: match &f.value {
                        Value::Name(_) => caps::value_label(draft, &f.path, &f.value),
                        _ => library_name(&hexes, layers.as_deref(), rom_names),
                    },
                    open: open(layers.as_deref(), entry),
                }
            })
            .collect(),
        None => {
            let mut keys: Vec<&str> = details
                .firmware
                .iter()
                .filter_map(|line| line.split_once(" · "))
                .map(|(key, _)| key.split(".parts[").next().unwrap_or(key))
                .collect();
            keys.dedup();
            keys.into_iter()
                .map(|key| {
                    let hexes = rom_hexes(details, key);
                    let layers = rom_layers(&hexes, summary);
                    FirmwareFact {
                        role: role_of_key(key).into(),
                        name: library_name(&hexes, layers.as_deref(), rom_names),
                        open: open(layers.as_deref(), None),
                    }
                })
                .collect()
        }
    };
    rows.dedup();
    rows
}
