// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware choices of Settings… on a machine or an image, and the "newer
//! version available" hint for superseded firmware; the approved mock-up
//! `backlog/docs/design/mockups/firmware-settings.html` and
//! `backlog/docs/design/machine-images.md` §7.
//!
//! A machine's or an image's hardware tables name each ROM by its SHA-256
//! (`[firmware.bios]` and `[[firmware.option_roms]]`); the firmware catalogue
//! knows the entry those bytes are, its CMOS layout and, for a superseded
//! release, the release that replaces it and why. A choice binds the ROM of
//! a firmware package in the local store: the tables name it by content, and
//! the image or machine binds the package's blob by digest when it lowers.

use assembly::{CmosLayout, FirmwareDistribution, FirmwareEntry, FirmwareKind};
use pc_image::create::{Package, PackageRom};
use toml::{Table, Value};

/// A firmware choice of the settings sheet's Firmware tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    /// The system BIOS at the top of the first MiB.
    System,
    /// The video card's BIOS at C0000h.
    Video,
}

impl Slot {
    pub const ALL: [Self; 2] = [Self::System, Self::Video];
    pub fn kind(self) -> FirmwareKind {
        match self {
            Self::System => FirmwareKind::SystemBios,
            Self::Video => FirmwareKind::VideoBios,
        }
    }
    /// The firmware package role of a ROM for this slot.
    fn role(self) -> &'static str {
        match self {
            Self::System => "system-bios",
            Self::Video => "video-bios",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System BIOS",
            Self::Video => "Video BIOS",
        }
    }
}

/// The catalogue the library offers: the PC board's reviewed builds.
pub fn catalogue() -> &'static [FirmwareEntry] {
    pc_image::firmware::catalogue()
}

pub fn entry(id: &str) -> Option<&'static FirmwareEntry> {
    catalogue().iter().find(|e| e.id == id)
}

/// The catalogue entry a ROM table names by its SHA-256.
fn entry_of(rom: &Table, kind: FirmwareKind) -> Option<&'static FirmwareEntry> {
    let sha256 = rom.get("sha256")?.as_str()?;
    catalogue()
        .iter()
        .find(|e| e.kind == kind && e.sha256 == sha256)
}

fn firmware(machine: &Table) -> Option<&Table> {
    machine.get("firmware")?.as_table()
}

/// Whether an option ROM table is the video BIOS: its bytes are a catalogued
/// video BIOS, or it sits in the video-ROM window C0000h–C7FFFh (IBM
/// Personal Computer AT Technical Reference, "Adapters with System-Accessible
/// ROM Modules").
fn is_video(rom: &Table) -> bool {
    entry_of(rom, FirmwareKind::VideoBios).is_some()
        || rom
            .get("base")
            .and_then(Value::as_integer)
            .is_some_and(|base| (0xC0000..0xC8000).contains(&base))
}

/// The ROM table of a slot in hardware tables.
pub fn rom_table(machine: &Table, slot: Slot) -> Option<&Table> {
    let firmware = firmware(machine)?;
    match slot {
        Slot::System => firmware.get("bios")?.as_table(),
        Slot::Video => firmware
            .get("option_roms")?
            .as_array()?
            .iter()
            .filter_map(Value::as_table)
            .find(|rom| is_video(rom)),
    }
}

/// The option ROMs other than the video BIOS, by catalogue name: they follow
/// the card they serve (the XTIDE build follows its disk adapter).
pub fn option_roms(machine: &Table) -> Vec<String> {
    firmware(machine)
        .and_then(|f| f.get("option_roms"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_table)
        .filter(|rom| !is_video(rom))
        .map(|rom| {
            entry_of(rom, FirmwareKind::OptionRom)
                .map_or_else(|| "An unlisted option ROM".to_owned(), name)
        })
        .collect()
}

/// The catalogue entry hardware tables bind in a slot.
pub fn current(machine: &Table, slot: Slot) -> Option<&'static FirmwareEntry> {
    entry_of(rom_table(machine, slot)?, slot.kind())
}

/// Put `rom` into the slot: the system BIOS table, or the video BIOS in
/// place of the current one among the option ROMs (first when there is none,
/// as New machine… orders them).
pub fn replace(machine: &mut Table, slot: Slot, rom: Table) -> Result<(), String> {
    let firmware = machine
        .entry("firmware")
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or("[firmware] must be a table")?;
    match slot {
        Slot::System => {
            firmware.insert("bios".into(), rom.into());
        }
        Slot::Video => {
            let roms = firmware
                .entry("option_roms")
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or("option ROMs must be an array")?;
            match roms.iter().position(|r| r.as_table().is_some_and(is_video)) {
                Some(index) => roms[index] = rom.into(),
                None => roms.insert(0, rom.into()),
            }
        }
    }
    Ok(())
}

/// The ROM table New machine… would write for a package ROM, with logical
/// asset names taken from the bytes' SHA-256, so a new binding never clashes
/// with a name the image already uses.
pub fn package_table(rom: &PackageRom) -> Result<Table, String> {
    let mut table = rom.table(0).map_err(|e| e.to_string())?;
    let name = |table: &Table| {
        table
            .get("sha256")
            .and_then(Value::as_str)
            .map(|sha| format!("assets/firmware-{}.bin", &sha[..sha.len().min(16)]))
            .ok_or_else(|| "a firmware ROM without SHA-256".to_owned())
    };
    match table.get_mut("parts").and_then(Value::as_array_mut) {
        Some(parts) => {
            for part in parts {
                let part = part.as_table_mut().ok_or("a ROM part must be a table")?;
                let image = name(part)?;
                part.insert("image".into(), image.into());
            }
        }
        None => {
            let image = name(&table)?;
            table.insert("image".into(), image.into());
        }
    }
    Ok(table)
}

/// "Needs unemupc X or newer" for a package this release does not read
/// (machine-images §5.6): it is offered with that reason and never bound.
pub fn package_needs_newer(package: &Package) -> Option<String> {
    image_store::compat::Requirement::of(&package.manifest).unmet_reason()
}

/// The package ROM of the local store that holds an entry's bytes: its
/// SHA-256 and size must be the catalogue's, whatever catalogue id the
/// package claims; a package this release reads, then a ROM naming the
/// entry's id, then an open package, wins.
pub fn package_rom(packages: &[Package], entry: &FirmwareEntry, slot: Slot) -> Option<PackageRom> {
    let mut found: Vec<(bool, PackageRom)> = packages
        .iter()
        .flat_map(|p| {
            p.config
                .roms
                .iter()
                .filter(|r| {
                    r.role == slot.role() && r.sha256 == entry.sha256 && r.size == entry.size
                })
                .map(move |r| {
                    (
                        r.catalogue_id.as_deref() == Some(entry.id),
                        PackageRom::new(p, r),
                    )
                })
        })
        .collect();
    found.sort_by_key(|(named, rom)| {
        (
            package_needs_newer(&rom.package).is_some(),
            !named,
            !rom_open(rom),
        )
    });
    found.into_iter().next().map(|(_, rom)| rom)
}

/// The SHA-256 of every ROM hardware tables name (a ROM in chips by its
/// assembled bytes), for telling which packages a machine's settings use.
pub fn rom_hashes(machine: &Table) -> Vec<String> {
    let Some(firmware) = firmware(machine) else {
        return Vec::new();
    };
    firmware
        .get("bios")
        .into_iter()
        .chain(
            firmware
                .get("option_roms")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|rom| rom.get("sha256")?.as_str().map(str::to_owned))
        .collect()
}

/// A firmware choice of a machine's own settings, with its mark.
#[derive(Clone, Copy, Debug)]
pub struct Own {
    pub slot: Slot,
    pub entry: &'static FirmwareEntry,
    pub open: bool,
}

/// The catalogue entries a machine's settings tables bind, each marked as
/// the package blob the settings record carries it (every chip open), or by
/// the catalogue's distribution where the record has none.
pub fn own_choices(settings: &Table) -> Vec<Own> {
    let blobs = pc_image::revision::bound_blobs(settings);
    Slot::ALL
        .into_iter()
        .filter_map(|slot| {
            let rom = rom_table(settings, slot)?;
            let entry = current(settings, slot)?;
            let hashes: Vec<&str> = match rom.get("parts").and_then(Value::as_array) {
                Some(parts) => parts
                    .iter()
                    .filter_map(|p| p.get("sha256")?.as_str())
                    .collect(),
                None => rom
                    .get("sha256")
                    .and_then(Value::as_str)
                    .into_iter()
                    .collect(),
            };
            let marks: Option<Vec<bool>> = hashes
                .iter()
                .map(|sha| {
                    blobs
                        .iter()
                        .find(|d| d.decoded_digest().is_ok_and(|digest| digest.hex() == *sha))
                        .map(|d| image_store::license::open_license(d).is_some())
                })
                .collect();
            let open = match marks {
                Some(marks) if !marks.is_empty() => marks.into_iter().all(|open| open),
                _ => catalogue_open(entry),
            };
            Some(Own { slot, entry, open })
        })
        .collect()
}

/// Whether a catalogue entry replaces a superseded one.
pub fn replaces_any(entry: &FirmwareEntry) -> bool {
    catalogue()
        .iter()
        .any(|old| old.superseded_by.is_some_and(|s| s.by == entry.id))
}

/// Whether every blob of a package ROM is open.
pub fn rom_open(rom: &PackageRom) -> bool {
    rom.rom().is_ok_and(|r| rom.package.open(r))
}

/// The entry's name as the catalogue labels it, without the superseded mark.
pub fn name(entry: &FirmwareEntry) -> String {
    thread_local! {
        // The labels come from the capabilities API, which reads them from
        // the board's catalogue; one draft serves every lookup.
        static DRAFT: assembly::capabilities::Draft =
            assembly::capabilities::Draft::for_board(assembly::BoardModel::Ibm5170);
    }
    let label = DRAFT.with(|draft| {
        assembly::capabilities::value_label(
            draft,
            "firmware.bios",
            &assembly::capabilities::Value::Name(entry.id.into()),
        )
    });
    label
        .strip_suffix(" (superseded)")
        .map_or(label.clone(), str::to_owned)
}

/// The entry's release as the hint names it: `1.0.2-unemu.3`, `0.9d-unemu.1`,
/// `1985-06-10`.
pub fn release(entry: &FirmwareEntry) -> String {
    let id = entry.id;
    for prefix in [
        "unemu-at-bios-",
        "ibm-5170-bios-",
        "landmark-rom-post-at-",
        "xub-",
    ] {
        if let Some(rest) = id.strip_prefix(prefix) {
            return rest.to_owned();
        }
    }
    if let Some(rest) = id.strip_prefix("vgabios-") {
        return rest.trim_end_matches("-std").to_owned();
    }
    if let Some(rest) = id.strip_prefix("kiselev-xi8088-") {
        return match rest.strip_suffix("-atreset") {
            Some(version) => format!("{version} (AT reset)"),
            None => rest.to_owned(),
        };
    }
    if id == "ibm-vga-bios-1987-32k" {
        return "1987".into();
    }
    id.to_owned()
}

/// The family a system BIOS choice is listed under; the entries of one family
/// share a CMOS layout.
pub fn family(layout: Option<CmosLayout>) -> &'static str {
    match layout {
        Some(CmosLayout::OpenAtBios) => "Open AT BIOS (unemu-at-bios)",
        Some(CmosLayout::KiselevXi8088) => "Kiselev 8088_bios (Xi 8088 builds)",
        Some(CmosLayout::IbmAt) => "IBM 5170 BIOS",
        Some(CmosLayout::LandmarkRomPost) => "Diagnostic ROMs",
        None => "Other firmware",
    }
}

/// Whether the catalogue lists an entry as open firmware.
pub fn catalogue_open(entry: &FirmwareEntry) -> bool {
    matches!(entry.distribution, FirmwareDistribution::Open { .. })
}

/// The release a superseded entry names as its replacement, and why.
pub fn replacement(entry: &FirmwareEntry) -> Option<(&'static FirmwareEntry, &'static str)> {
    let supersession = entry.superseded_by?;
    Some((self::entry(supersession.by)?, supersession.reason))
}

/// One superseded release a machine or an image uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Newer {
    pub slot: Slot,
    pub old: &'static FirmwareEntry,
    pub by: &'static FirmwareEntry,
    pub reason: &'static str,
}

impl Newer {
    /// "System BIOS 1.0.2-unemu.2 is superseded by 1.0.2-unemu.3 (why)".
    pub fn text(&self) -> String {
        format!(
            "{} {} is superseded by {} ({})",
            self.slot.label(),
            release(self.old),
            release(self.by),
            self.reason
        )
    }
    /// Whether the replacement keeps the CMOS layout, so a switch keeps CMOS.
    pub fn same_layout(&self) -> bool {
        self.old.cmos_layout == self.by.cmos_layout
    }
}

/// The catalogue entries an image binds, by slot, read from its library
/// summary: each firmware layer whose decoded digest is a catalogued ROM.
pub fn in_summary(
    summary: &image_store::summary::ImageSummary,
) -> Vec<(Slot, &'static FirmwareEntry)> {
    let mut found: Vec<(Slot, &'static FirmwareEntry)> = summary
        .storage_blobs
        .iter()
        .filter(|d| d.decoded_media_type() == image_store::media_type::FIRMWARE)
        .filter_map(|d| d.decoded_digest().ok())
        .filter_map(|digest| {
            let entry = catalogue().iter().find(|e| e.sha256 == digest.hex())?;
            let slot = Slot::ALL.into_iter().find(|s| s.kind() == entry.kind)?;
            Some((slot, entry))
        })
        .collect();
    found.sort_by_key(|(slot, _)| *slot);
    found.dedup_by_key(|(slot, _)| *slot);
    found
}

/// The entries a machine runs: its own settings' firmware where it has
/// settings, else its image's.
pub fn machine_firmware(
    settings: Option<&Table>,
    summary: Option<&image_store::summary::ImageSummary>,
) -> Vec<(Slot, &'static FirmwareEntry)> {
    match settings {
        Some(tables) => Slot::ALL
            .into_iter()
            .filter_map(|slot| Some((slot, current(tables, slot)?)))
            .collect(),
        None => summary.map(in_summary).unwrap_or_default(),
    }
}

/// The superseded releases among catalogue entries a machine or an image
/// binds, as (slot, entry) pairs.
pub fn newer_of(entries: &[(Slot, &'static FirmwareEntry)]) -> Vec<Newer> {
    entries
        .iter()
        .filter_map(|(slot, old)| {
            let (by, reason) = replacement(old)?;
            Some(Newer {
                slot: *slot,
                old,
                by,
                reason,
            })
        })
        .collect()
}

/// The quiet row badge's tooltip: every hint, one sentence each.
pub fn badge_tooltip(newer: &[Newer]) -> Option<String> {
    (!newer.is_empty()).then(|| {
        newer
            .iter()
            .map(|n| format!("{}.", n.text()))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// The quiet badge after a machine's state or an image's name.
pub const BADGE: &str = "↑ newer BIOS";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_and_families_name_the_catalogue_entries_as_the_mock_up_does() {
        let release_of = |id: &str| release(entry(id).unwrap());
        assert_eq!(release_of("unemu-at-bios-1.0.2-unemu.3"), "1.0.2-unemu.3");
        assert_eq!(release_of("vgabios-0.8a-unemu.1-std"), "0.8a-unemu.1");
        assert_eq!(
            release_of("kiselev-xi8088-1.0.2-atreset"),
            "1.0.2 (AT reset)"
        );
        assert_eq!(release_of("ibm-5170-bios-1985-06-10"), "1985-06-10");
        assert_eq!(release_of("ibm-vga-bios-1987-32k"), "1987");
        assert_eq!(
            family(entry("kiselev-xi8088-1.0.2").unwrap().cmos_layout),
            "Kiselev 8088_bios (Xi 8088 builds)"
        );
        assert_eq!(
            name(entry("unemu-at-bios-1.0.2-unemu.2").unwrap()),
            "unemu AT BIOS 1.0.2-unemu.2"
        );
    }

    #[test]
    fn a_superseded_bios_in_the_tables_gives_the_stored_reason_and_its_replacement() {
        let old = entry("unemu-at-bios-1.0.2-unemu.2").unwrap();
        let vga = entry("vgabios-0.8a-unemu.1-std").unwrap();
        let mut machine: Table = toml::from_str(&format!(
            "[firmware.bios]\nimage='assets/firmware-0.bin'\nsha256='{}'\n[[firmware.option_roms]]\nimage='assets/firmware-1.bin'\nsha256='{}'\nbase=0xC0000\n",
            old.sha256, vga.sha256
        ))
        .unwrap();
        let newer = |machine: &Table| newer_of(&machine_firmware(Some(machine), None));
        let hints = newer(&machine);
        assert_eq!(hints.len(), 2);
        assert_eq!(
            hints[0].text(),
            "System BIOS 1.0.2-unemu.2 is superseded by 1.0.2-unemu.3 (clears memory above 1 MiB on POST: XMS after Ctrl+Alt+Del)"
        );
        assert!(hints[0].same_layout());
        assert_eq!(
            hints[1].text(),
            "Video BIOS 0.8a-unemu.1 is superseded by 0.9d-unemu.1 (teletype BEL sounds the speaker)"
        );
        assert_eq!(
            badge_tooltip(&hints).unwrap(),
            format!("{}. {}.", hints[0].text(), hints[1].text())
        );
        // A current release has no hint.
        let current = entry("unemu-at-bios-1.0.2-unemu.3").unwrap();
        let mut bios = rom_table(&machine, Slot::System).unwrap().clone();
        bios.insert("sha256".into(), current.sha256.into());
        replace(&mut machine, Slot::System, bios).unwrap();
        assert_eq!(current_of(&machine), Some(current.id));
        assert_eq!(newer(&machine).len(), 1);
        assert_eq!(badge_tooltip(&[]), None);
    }

    fn current_of(machine: &Table) -> Option<&'static str> {
        current(machine, Slot::System).map(|e| e.id)
    }
}
