// SPDX-License-Identifier: AGPL-3.0-or-later
//! The codec half of a stored config's validation: the envelope, its
//! bindings against the hardware tables, the slots, each medium's size
//! against its geometry, and the lowering. It reads nothing of a store:
//! what it knows of content comes from the caller's rows, and the firmware
//! and CMOS bytes through a `ContentSource`, each opened by the SHA-256 and
//! size of its row. See `backlog/docs/design/core-contract.md` §14.3.

use super::{
    drive_tables, firmware_images, invalid, unit, validate_bios, Config, Digest, Policy, Result,
    Slot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};
use unemu_core_api::{Content, ContentSource};

/// What the codec knows of one blob or disk head: the SHA-256 of the flat
/// bytes a machine opens, and their length. The codec reads a disk's row
/// for its size and its pin, and opens firmware and CMOS content by their
/// rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentRow {
    /// For a raw blob its own digest; for a layered head the chain's
    /// result; for a zero base the digest of that many zero bytes.
    pub sha256: [u8; 32],
    /// The decoded size of a raw blob. For a layered or zero head, the
    /// capacity of the medium's geometry, which the store's chain walk holds
    /// equal to the chain's root.
    pub size: u64,
}

/// A config that passed every check, and the machine it lowers to.
pub struct Lowered {
    pub config: Config,
    pub machine: assembly::MachineConfig,
}

/// The path a fixed disk lowers from while its size is checked: a disk
/// payload is never read in validation.
const VALIDATION_DISK: &str = "__pc_image_validation_disk__";
struct Reader<'a> {
    content: &'a dyn ContentSource,
    rows: &'a BTreeMap<Digest, ContentRow>,
    /// Logical asset name to its digest, and whether it is small content.
    bindings: BTreeMap<String, (Digest, bool)>,
}
impl Reader<'_> {
    /// The bytes of a firmware ROM or CMOS image, opened by its row. Content
    /// of another length than the row's is refused before it is read, so a
    /// source can neither make the codec allocate a size of its choosing nor
    /// hand it other bytes than the row names.
    fn small(&self, digest: &Digest) -> io::Result<Vec<u8>> {
        let row = self
            .rows
            .get(digest)
            .ok_or_else(|| io::Error::other("unbound logical asset"))?;
        match self
            .content
            .open(&row.sha256, row.size)
            .map_err(io::Error::other)?
        {
            Content::Bytes(bytes) if bytes.len() as u64 == row.size => Ok(bytes),
            Content::Bytes(_) => Err(io::Error::other("content size differs from its row")),
            Content::Reader(reader) => {
                if reader.size() != row.size {
                    return Err(io::Error::other("content size differs from its row"));
                }
                let len = usize::try_from(row.size)
                    .map_err(|_| io::Error::other("small content exceeds memory"))?;
                let mut bytes = vec![0; len];
                reader.read_at(0, &mut bytes).map_err(io::Error::other)?;
                Ok(bytes)
            }
            _ => Err(io::Error::other("unsupported content")),
        }
    }
}
impl crate::ImageReader for Reader<'_> {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        if path == Path::new(VALIDATION_DISK) {
            return Ok(vec![0; 512]);
        }
        let (digest, small) = self
            .bindings
            .get(&path.to_string_lossy().to_string())
            .ok_or_else(|| io::Error::other("unbound logical asset"))?;
        if !small {
            return Err(io::Error::other("disk payload must stay file-backed"));
        }
        let digest = *digest;
        self.small(&digest)
    }
}
fn logical(name: &str) -> Result<()> {
    if !name.starts_with("assets/")
        || name.len() > 200
        || name
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
    {
        return Err(invalid("invalid logical asset name"));
    }
    Ok(())
}
fn bind(
    reader: &mut Reader<'_>,
    names: &mut BTreeSet<String>,
    source: &str,
    digest: &Digest,
    small: bool,
) -> Result<()> {
    logical(source)?;
    if !names.insert(source.into()) {
        return Err(invalid("duplicate logical asset name"));
    }
    reader.bindings.insert(source.into(), (*digest, small));
    Ok(())
}
fn row<'a>(rows: &'a BTreeMap<Digest, ContentRow>, digest: &Digest) -> Result<&'a ContentRow> {
    rows.get(digest)
        .ok_or_else(|| invalid(format!("missing content row for {digest}")))
}

/// A stored drive table names no medium: the slots do.
pub fn check_media_fields(drive: &toml::Table) -> Result<()> {
    if [
        "image",
        "sha256",
        "media",
        "empty",
        "write",
        "journal",
        "keep_generations",
    ]
    .iter()
    .any(|k| drive.contains_key(*k))
    {
        return Err(invalid("bundle hardware contains media fields"));
    }
    Ok(())
}
/// A slot sits at its drive's position and has the shape its drive takes.
/// The store's walk stops where this refuses, so that the codec's sentence
/// is the one a person reads.
pub fn check_slot(controller: &str, kind: &str, drive: &toml::Table, slot: &Slot) -> Result<()> {
    if slot.controller != controller || slot.unit != unit(kind, drive)? {
        return Err(invalid("slot binding differs from hardware position"));
    }
    if slot
        .inserted
        .as_ref()
        .is_some_and(|id| slot.catalogue.first() != Some(id))
    {
        return Err(invalid("unsupported initial selection"));
    }
    if kind == "ide"
        && (slot.catalogue_mode || slot.catalogue.len() != 1 || slot.inserted.is_none())
    {
        return Err(invalid("IDE needs exactly one inserted medium"));
    }
    if !slot.catalogue_mode && (slot.catalogue.len() != 1 || slot.inserted.is_none()) {
        return Err(invalid("single image slot requires one inserted medium"));
    }
    Ok(())
}

/// Lower `config`. `rows` holds one row per firmware blob, disk head and
/// CMOS image the config binds; `content` serves the firmware and CMOS rows.
/// `platform_known` is whether the config's platform names the PC, the
/// alias check the caller makes; `options` carries the permission for
/// unlisted firmware.
pub fn lower(
    config: Config,
    rows: &BTreeMap<Digest, ContentRow>,
    content: &dyn ContentSource,
    platform_known: bool,
    options: assembly::AssemblyOptions,
) -> Result<Lowered> {
    if config.schema != 1 || !platform_known {
        return Err(invalid("unsupported PC config envelope"));
    }
    let mut hardware = config.machine.clone();
    let raw_text = toml::to_string(&hardware).map_err(|e| invalid(e.to_string()))?;
    crate::parse_toml("bundle.toml", &raw_text)?;
    if ["extends", "name", "description"]
        .iter()
        .any(|k| hardware.contains_key(*k))
    {
        return Err(invalid(
            "bundle hardware must be resolved and contain no descriptive fields",
        ));
    }
    let mut reader = Reader {
        content,
        rows,
        bindings: BTreeMap::new(),
    };
    let mut names = BTreeSet::new();
    let mut firmware_count = 0;
    firmware_images(&mut hardware, |field, rom| {
        let binding = config
            .firmware
            .get(firmware_count)
            .ok_or_else(|| invalid("missing firmware binding"))?;
        if binding.key != field
            || rom.get("image").and_then(toml::Value::as_str) != Some(&binding.source)
        {
            return Err(invalid("firmware binding differs from hardware"));
        }
        bind(
            &mut reader,
            &mut names,
            &binding.source,
            &binding.blob,
            true,
        )?;
        if rom.get("sha256").and_then(toml::Value::as_str) != Some(binding.blob.hex().as_str()) {
            return Err(invalid("firmware hash differs from binding"));
        }
        firmware_count += 1;
        Ok(())
    })?;
    if firmware_count != config.firmware.len() {
        return Err(invalid("unreferenced firmware binding"));
    }
    let mut used = BTreeSet::new();
    let mut slot_index = 0;
    let mut ids = BTreeSet::new();
    if config
        .media
        .iter()
        .any(|m| !ids.insert(m.id.clone()) || m.id.is_empty())
    {
        return Err(invalid("duplicate or empty medium id"));
    }
    drive_tables(&mut hardware, |controller, kind, drive| {
        check_media_fields(drive)?;
        let slot = config
            .slots
            .get(slot_index)
            .ok_or_else(|| invalid("missing slot binding"))?;
        check_slot(controller, kind, drive, slot)?;
        for id in &slot.catalogue {
            let medium = config
                .media
                .iter()
                .find(|m| &m.id == id)
                .ok_or_else(|| invalid("unknown catalogue medium"))?;
            if used.insert(id.clone()) {
                bind(&mut reader, &mut names, &medium.source, &medium.head, false)?;
            } else if !matches!(medium.policy, Policy::Readonly) {
                return Err(invalid("writable medium bound more than once"));
            }
            let disk_size = row(rows, &medium.head)?.size;
            let g = &medium.geometry;
            if g.sector_bytes != 512
                || u64::from(g.cylinders)
                    .checked_mul(u64::from(g.heads))
                    .and_then(|n| n.checked_mul(u64::from(g.sectors)))
                    .and_then(|n| n.checked_mul(512))
                    != Some(disk_size)
            {
                return Err(invalid("medium geometry differs from payload length"));
            }
            if medium
                .name
                .as_ref()
                .is_some_and(|n| n.contains(['/', '\\', '\0']) || n.is_empty())
            {
                return Err(invalid("invalid medium name"));
            }
            if slot.catalogue_mode != medium.name.is_some() {
                return Err(invalid("medium name differs from catalogue shape"));
            }
            if kind == "ide" {
                let expected = toml::Table::from_iter([
                    ("cylinders".into(), i64::from(g.cylinders).into()),
                    ("heads".into(), i64::from(g.heads).into()),
                    ("sectors".into(), i64::from(g.sectors).into()),
                ]);
                if drive.get("geometry").and_then(toml::Value::as_table) != Some(&expected) {
                    return Err(invalid("medium geometry differs from hardware"));
                }
            }
        }
        // Lower only a bounded placeholder; the medium's geometry is checked
        // above against the row's size, and a disk is never read here. The
        // store verifies raw payloads; its resolved callback verifies a
        // layered head once.
        if kind == "ide" {
            drive.insert("image".into(), VALIDATION_DISK.into());
            drive.insert("write".into(), "volatile".into());
            drive.insert(
                "geometry".into(),
                toml::Table::from_iter([
                    ("cylinders".into(), 1.into()),
                    ("heads".into(), 1.into()),
                    ("sectors".into(), 1.into()),
                ])
                .into(),
            );
        } else {
            drive.insert("media".into(), Vec::<toml::Value>::new().into());
            drive.insert("empty".into(), true.into());
        }
        slot_index += 1;
        Ok(())
    })?;
    if used.len() != config.media.len() || slot_index != config.slots.len() {
        return Err(invalid("unreferenced medium or slot binding"));
    }
    let rtc_nvram = hardware
        .get_mut("board")
        .and_then(toml::Value::as_table_mut)
        .and_then(|b| b.get_mut("rtc"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|rtc| rtc.get_mut("nvram"));
    if let Some(nvram) = &config.nvram {
        let target = rtc_nvram.ok_or_else(|| invalid("missing hardware NVRAM"))?;
        let expected = toml::Table::from_iter([("image".into(), nvram.source.clone().into())]);
        if target.as_table() != Some(&expected) {
            return Err(invalid("NVRAM hardware differs from binding"));
        }
        bind(&mut reader, &mut names, &nvram.source, &nvram.head, true)?;
        let mut table = expected;
        table.insert("sha256".into(), nvram.head.hex().into());
        table.insert("write".into(), "volatile".into());
        *target = table.into();
    } else if rtc_nvram.is_some_and(|n| n.as_str().is_none()) {
        return Err(invalid("unbound hardware NVRAM"));
    }
    let text = toml::to_string(&hardware).map_err(|e| invalid(e.to_string()))?;
    let document = crate::parse_toml("bundle.toml", &text)?;
    let mut machine = crate::lower(&document, &crate::Origins::new("bundle.toml"), &mut reader)?;
    for (drive, slot) in machine.drives.iter_mut().zip(&config.slots) {
        if drive.unit != slot.unit {
            return Err(invalid("slot order differs from effective drive order"));
        }
        let mut entries = Vec::new();
        for id in &slot.catalogue {
            let m = config
                .media
                .iter()
                .find(|m| &m.id == id)
                .ok_or_else(|| invalid("missing medium"))?;
            if matches!(drive.kind, assembly::DriveKind::HardDisk { .. }) {
                drive.kind = assembly::DriveKind::HardDisk {
                    geometry: assembly::HardDiskGeometry {
                        cylinders: m.geometry.cylinders,
                        heads: m.geometry.heads,
                        sectors_per_track: m.geometry.sectors,
                        bytes_per_sector: m.geometry.sector_bytes,
                    },
                };
            }
            if !m.geometry.fits(drive.kind) {
                return Err(invalid("medium geometry differs from hardware"));
            }
            entries.push(assembly::FloppyMedium {
                image: assembly::ImageSource::File(m.source.clone().into()),
                expected_sha256: Some(row(rows, &m.head)?.sha256),
                write: m.policy.write(format!("{}.upfj", m.id).into()),
                name: m.name.clone().unwrap_or_default(),
            });
        }
        if slot.catalogue_mode {
            drive.media = Some(assembly::FloppyMediaList {
                entries,
                empty: slot.inserted.is_none(),
                inserted: 0,
            });
            drive.expected_sha256 = None;
            drive.write = assembly::WriteMode::Volatile;
            drive.image = assembly::ImageSource::Blank {
                size: drive.kind.image_bytes(),
            };
        } else {
            let entry = entries
                .pop()
                .ok_or_else(|| invalid("empty single image slot"))?;
            drive.media = None;
            drive.image = entry.image;
            drive.expected_sha256 = entry.expected_sha256;
            drive.write = entry.write;
        }
    }
    machine.validate().map_err(|e| invalid(e.to_string()))?;
    assembly::check_option_rom_catalogue(&machine, assembly::AssemblyOptions::default())
        .map_err(|e| invalid(e.to_string()))?;
    validate_bios(&machine, options)?;
    Ok(Lowered { config, machine })
}
