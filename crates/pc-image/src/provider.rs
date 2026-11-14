// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC bundle-profile validation and lowering through schema 1; machine-images §2.
//! IBM Personal Computer AT Technical Reference, System Board and Diskette Drive
//! Adapter: controller-local units and exact geometry retain their existing wiring.

use crate::*;
use image_store::{media_type as mt, BlobSource, ConfigValidator, PayloadType, Store};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

pub const CMOS: &str = "application/vnd.unemu.cmos.v1";
/// Explicit PC codecs. Optical images, saved states and sparse layers are refused.
/// ATA/ATAPI-4, IDENTIFY DEVICE: raw fixed disks are further bounded by the
/// existing XTIDE-compatible CHS geometry validator.
pub const PAYLOAD_TYPES: &[PayloadType] = &[
    PayloadType {
        media_type: mt::FIRMWARE,
        max_size: 16 * 1024 * 1024,
    },
    PayloadType {
        media_type: mt::DISK_BASE,
        max_size: 16383 * 16 * 63 * 512,
    },
    PayloadType {
        media_type: mt::FLOPPY,
        max_size: 1440 * 1024,
    },
    PayloadType {
        media_type: CMOS,
        max_size: 64,
    },
];
/// The IBM PC/AT provider; no host endpoints or inheritance are accepted.
pub struct PcProvider;
impl ConfigValidator for PcProvider {
    fn platform(&self) -> &str {
        "ibm-pc-at"
    }
    fn payload_types(&self) -> &[PayloadType] {
        PAYLOAD_TYPES
    }
    fn validate(
        &self,
        config: &toml::Table,
        descriptors: &[Descriptor],
        blobs: &dyn BlobSource,
    ) -> image_store::Result<Vec<Digest>> {
        prepare(
            decode(config.clone()).map_err(store_error)?,
            descriptors,
            blobs,
        )
        .map(|p| p.references)
        .map_err(store_error)
    }
}
fn store_error(e: Error) -> image_store::Error {
    image_store::Error::Invalid(e.to_string())
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
struct Reader<'a> {
    blobs: &'a dyn BlobSource,
    bindings: BTreeMap<String, Descriptor>,
}
impl machine_file::ImageReader for Reader<'_> {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        if path == Path::new("__pc_image_validation_disk__") {
            return Ok(vec![0; 512]);
        }
        let d = self
            .bindings
            .get(&path.to_string_lossy().to_string())
            .ok_or_else(|| io::Error::other("unbound logical asset"))?;
        if !matches!(d.media_type.as_str(), mt::FIRMWARE | CMOS) {
            return Err(io::Error::other("disk payload must stay file-backed"));
        }
        let limit = PAYLOAD_TYPES
            .iter()
            .find(|t| t.media_type == d.media_type)
            .map(|t| t.max_size)
            .ok_or_else(|| io::Error::other("unsupported payload"))?;
        self.blobs.read_verified(d, limit).map_err(io::Error::other)
    }
}
struct Prepared {
    config: Config,
    machine: assembly::MachineConfig,
    references: Vec<Digest>,
}
fn bind(
    reader: &mut Reader<'_>,
    names: &mut BTreeSet<String>,
    source: &str,
    digest: &Digest,
    kind: &str,
    descriptors: &[Descriptor],
    refs: &mut Vec<Digest>,
) -> Result<Descriptor> {
    logical(source)?;
    if !names.insert(source.into()) {
        return Err(invalid("duplicate logical asset name"));
    }
    let d = descriptor(descriptors, digest, kind)?;
    let limit = PAYLOAD_TYPES
        .iter()
        .find(|t| t.media_type == kind)
        .map(|t| t.max_size)
        .ok_or_else(|| invalid("unsupported payload"))?;
    if d.size > limit {
        return Err(invalid("payload exceeds PC size limit"));
    }
    reader.bindings.insert(source.into(), d.clone());
    refs.push(digest.clone());
    Ok(d)
}
fn prepare(config: Config, descriptors: &[Descriptor], blobs: &dyn BlobSource) -> Result<Prepared> {
    if config.schema != 1 || config.platform != "ibm-pc-at" {
        return Err(invalid("unsupported PC config envelope"));
    }
    let mut hardware = config.machine.clone();
    let raw_text = toml::to_string(&hardware).map_err(|e| invalid(e.to_string()))?;
    machine_file::parse_toml("bundle.toml", &raw_text).map_err(machine_file::LoadError::from)?;
    if ["extends", "name", "description"]
        .iter()
        .any(|k| hardware.contains_key(*k))
    {
        return Err(invalid(
            "bundle hardware must be resolved and contain no descriptive fields",
        ));
    }
    let mut reader = Reader {
        blobs,
        bindings: BTreeMap::new(),
    };
    let mut names = BTreeSet::new();
    let mut references = Vec::new();
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
            mt::FIRMWARE,
            descriptors,
            &mut references,
        )?;
        if rom.get("sha256").and_then(toml::Value::as_str) != Some(binding.blob.hex()) {
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
        let slot = config
            .slots
            .get(slot_index)
            .ok_or_else(|| invalid("missing slot binding"))?;
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
        for id in &slot.catalogue {
            let medium = config
                .media
                .iter()
                .find(|m| &m.id == id)
                .ok_or_else(|| invalid("unknown catalogue medium"))?;
            let payload_type = if kind == "ide" {
                mt::DISK_BASE
            } else {
                mt::FLOPPY
            };
            let d = if used.insert(id.clone()) {
                bind(
                    &mut reader,
                    &mut names,
                    &medium.source,
                    &medium.head,
                    payload_type,
                    descriptors,
                    &mut references,
                )?
            } else {
                if !matches!(medium.policy, Policy::Readonly) {
                    return Err(invalid("writable medium bound more than once"));
                }
                references.push(medium.head.clone());
                descriptor(descriptors, &medium.head, payload_type)?
            };
            let g = &medium.geometry;
            if g.sector_bytes != 512
                || u64::from(g.cylinders)
                    .checked_mul(u64::from(g.heads))
                    .and_then(|n| n.checked_mul(u64::from(g.sectors)))
                    .and_then(|n| n.checked_mul(512))
                    != Some(d.size)
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
        // Lower only a bounded placeholder; descriptor geometry is validated above.
        // Actual disk bytes are streamed and hashed by the store, never by this reader.
        if kind == "ide" {
            drive.insert("image".into(), "__pc_image_validation_disk__".into());
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
        let d = bind(
            &mut reader,
            &mut names,
            &nvram.source,
            &nvram.head,
            CMOS,
            descriptors,
            &mut references,
        )?;
        if d.size != 64 {
            return Err(invalid("CMOS must contain exactly 64 bytes"));
        }
        let mut table = expected;
        table.insert("sha256".into(), nvram.head.hex().into());
        table.insert("write".into(), "volatile".into());
        *target = table.into();
    } else if rtc_nvram.is_some_and(|n| n.as_str().is_none()) {
        return Err(invalid("unbound hardware NVRAM"));
    }
    let text = toml::to_string(&hardware).map_err(|e| invalid(e.to_string()))?;
    let document =
        machine_file::parse_toml("bundle.toml", &text).map_err(machine_file::LoadError::from)?;
    let mut machine = machine_file::lower(
        &document,
        &machine_file::Origins::new("bundle.toml"),
        &mut reader,
    )?;
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
            if m.geometry != Geometry::of(drive.kind) {
                return Err(invalid("medium geometry differs from hardware"));
            }
            entries.push(assembly::FloppyMedium {
                image: assembly::ImageSource::File(m.source.clone().into()),
                expected_sha256: Some(hash_array(&m.head)?),
                write: m.policy.write(format!("{}.upfj", m.id).into()),
                name: m.name.clone().unwrap_or_default(),
            });
        }
        if slot.catalogue_mode {
            drive.media = Some(assembly::FloppyMediaList {
                entries,
                empty: slot.inserted.is_none(),
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
    validate_bios(&machine)?;
    Ok(Prepared {
        config,
        machine,
        references,
    })
}

/// Resolve a verified image to assembly input. Journal outputs use only the caller's
/// session directory; bases remain immutable blobs in the store. No session is created.
pub fn resolve(store: &Store, tag: &str, session: &Path) -> Result<assembly::MachineConfig> {
    struct Capture(RefCell<Option<Prepared>>);
    impl ConfigValidator for Capture {
        fn platform(&self) -> &str {
            "ibm-pc-at"
        }
        fn payload_types(&self) -> &[PayloadType] {
            PAYLOAD_TYPES
        }
        fn validate(
            &self,
            config: &toml::Table,
            descriptors: &[Descriptor],
            blobs: &dyn BlobSource,
        ) -> image_store::Result<Vec<Digest>> {
            let prepared = prepare(
                decode(config.clone()).map_err(store_error)?,
                descriptors,
                blobs,
            )
            .map_err(store_error)?;
            let references = prepared.references.clone();
            self.0.replace(Some(prepared));
            Ok(references)
        }
    }
    let capture = Capture(RefCell::new(None));
    let image = store.resolve(tag, &capture)?;
    let record = crate::ImportRecord::read(store, &image.image.digest)?;
    let retention = |id: &str| {
        record
            .as_ref()
            .and_then(|r| r.generations.iter().find(|g| g.medium == id))
            .and_then(|g| g.keep_generations)
            .unwrap_or(assembly::WriteMode::DEFAULT_KEEP_GENERATIONS)
    };
    let mut prepared = capture
        .0
        .into_inner()
        .ok_or_else(|| invalid("provider was not called"))?;
    let base_path = |digest: &Digest| -> Result<std::path::PathBuf> {
        let d = image
            .manifest
            .layers
            .iter()
            .find(|d| &d.digest == digest)
            .ok_or_else(|| invalid("missing medium descriptor"))?;
        Ok(store.blob_file_path(digest, d.size)?)
    };
    for (drive, slot) in prepared
        .machine
        .drives
        .iter_mut()
        .zip(&prepared.config.slots)
    {
        if let Some(list) = &mut drive.media {
            for (entry, id) in list.entries.iter_mut().zip(&slot.catalogue) {
                let m = prepared
                    .config
                    .media
                    .iter()
                    .find(|m| &m.id == id)
                    .ok_or_else(|| invalid("missing medium"))?;
                entry.image = assembly::ImageSource::File(base_path(&m.head)?);
                entry.write = m.policy.write(session.join(format!("{}.upfj", m.id)));
                if let assembly::WriteMode::Journal {
                    keep_generations, ..
                } = &mut entry.write
                {
                    *keep_generations = retention(&m.id);
                }
            }
        } else {
            let m = prepared
                .config
                .media
                .iter()
                .find(|m| slot.catalogue.first() == Some(&m.id))
                .ok_or_else(|| invalid("missing medium"))?;
            drive.write = m.policy.write(session.join(format!("{}.upfj", m.id)));
            if let assembly::WriteMode::Journal {
                keep_generations, ..
            } = &mut drive.write
            {
                *keep_generations = retention(&m.id);
            }
            drive.image = assembly::ImageSource::File(base_path(&m.head)?);
            drive.expected_sha256 = Some(hash_array(&m.head)?);
        }
    }
    if let Some(n) = &prepared.config.nvram {
        if matches!(n.mode, NvramMode::Journal) {
            for device in &mut prepared.machine.devices {
                if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                    rtc.nvram = assembly::NvramSource::Journal {
                        image: base_path(&n.head)?,
                        expected_sha256: hash_array(&n.head)?,
                        journal: session.join("cmos.upfj"),
                        keep_generations: retention("cmos"),
                    };
                }
            }
        }
    }
    Ok(prepared.machine)
}
fn hash_array(digest: &Digest) -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&digest.hex()[i * 2..i * 2 + 2], 16)
            .map_err(|e| invalid(e.to_string()))?;
    }
    Ok(bytes)
}
