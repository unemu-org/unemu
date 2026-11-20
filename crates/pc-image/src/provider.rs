// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC bundle-profile validation and lowering through schema 1; machine-images §2.
//! IBM Personal Computer AT Technical Reference, System Board and Diskette Drive
//! Adapter: controller-local units keep their existing wiring. A fixed disk's
//! medium has the drive's exact geometry; a floppy medium has its payload's
//! exact geometry, one of the media the drive type takes
//! (`FloppyGeometry::accepted_media`).

use crate::*;
use image_store::{media_type as mt, BlobSource, ConfigValidator, PayloadType, Store};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

pub const CMOS: &str = "application/vnd.unemu.cmos.v1";
/// A blank fixed disk: geometry only, every sector zero (see [`crate::zero`]).
pub const DISK_ZERO: &str = crate::zero::MEDIA_TYPE;
/// Explicit PC codecs. Optical images and saved states are refused.
/// ATA/ATAPI-4, IDENTIFY DEVICE: raw fixed disks are further bounded by the
/// existing XTIDE-compatible CHS geometry validator. A `+zstd` limit bounds
/// the declared decoded size, the same limit as the raw type.
pub const PAYLOAD_TYPES: &[PayloadType] = &[
    PayloadType {
        media_type: "application/vnd.unemu.firmware.v1+zstd",
        max_size: 16 * 1024 * 1024,
    },
    PayloadType {
        media_type: "application/vnd.unemu.disk.base.v1+zstd",
        max_size: 16383 * 16 * 63 * 512,
    },
    PayloadType {
        media_type: "application/vnd.unemu.disk.sectors.v1+zstd",
        max_size: 16383 * 16 * 63 * 520,
    },
    PayloadType {
        media_type: "application/vnd.unemu.media.floppy.v1+zstd",
        max_size: 1440 * 1024,
    },
    PayloadType {
        media_type: mt::DISK_LAYER,
        max_size: image_store::MAX_SMALL_BLOB,
    },
    PayloadType {
        media_type: mt::DISK_SECTORS,
        max_size: 16383 * 16 * 63 * 520,
    },
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
    PayloadType {
        media_type: DISK_ZERO,
        max_size: crate::zero::MAX_DESCRIPTOR,
    },
    // A firmware source archive embedded with its open ROM.
    PayloadType {
        media_type: "application/x-tar+zstd",
        max_size: image_store::codec::MAX_SOURCE_ARCHIVE,
    },
];
/// The IBM PC/AT provider; no host endpoints or inheritance are accepted.
pub struct PcProvider;
impl ConfigValidator for PcProvider {
    fn platform(&self) -> &str {
        PLATFORM
    }
    fn validate_resolved(
        &self,
        image: &image_store::ResolvedImage,
        store: &Store,
    ) -> image_store::Result<()> {
        let config = config_from_image(image).map_err(store_error)?;
        crate::disk::check(store, &config, image).map_err(store_error)
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
    fn required_release(
        &self,
        manifest: &image_store::Manifest,
        config: &[u8],
    ) -> image_store::Result<image_store::compat::AppVersion> {
        crate::compat::release(manifest, config)
    }
    fn needs(&self) -> Option<&dyn image_store::needs::Vocabulary> {
        Some(&crate::needs::PcNeeds)
    }
}
fn store_error(e: Error) -> image_store::Error {
    image_store::Error::Invalid(e.to_string())
}
pub(crate) struct Prepared {
    pub(crate) config: Config,
    pub(crate) machine: assembly::MachineConfig,
    pub(crate) references: Vec<Digest>,
}
/// Check a config against the store, then lower it. The store half reads the
/// descriptors and blobs; the codec half reads only what the store half hands
/// it, so the same lowering can later run where there is no store.
pub(crate) fn prepare(
    config: Config,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<Prepared> {
    let stored = store_half(&config, descriptors, blobs)?;
    let lowered = codec::lower(
        config,
        &stored.rows,
        &stored.content,
        stored.platform_known,
        firmware_options(),
    )?;
    Ok(Prepared {
        config: lowered.config,
        machine: lowered.machine,
        references: stored.references,
    })
}

/// What the store half answers for one config.
struct Stored<'a> {
    /// Whether the config's platform names this provider: the PC id or its
    /// retired alias.
    platform_known: bool,
    /// One row per firmware blob, disk head and CMOS image the config binds.
    rows: BTreeMap<Digest, codec::ContentRow>,
    /// The closure, in the order the store expects it.
    references: Vec<Digest>,
    /// The firmware and CMOS bytes, read on demand.
    content: StoreContent<'a>,
}

/// The store half: every fact the descriptors and blobs decide. For each
/// digest the config binds it finds the descriptor of the right kind and
/// checks the PC size limit; it checks a ROM's source link and source
/// archive, walks a fixed disk's layer chain, applies the zero-base rule and
/// collects the closure. It refuses only on those facts: a config whose
/// shape is wrong is the codec half's to refuse, so the walk stops where the
/// codec half would refuse it and leaves the rest alone.
fn store_half<'a>(
    config: &Config,
    descriptors: &[Descriptor],
    blobs: &'a dyn BlobSource,
) -> Result<Stored<'a>> {
    let mut stored = Stored {
        platform_known: is_pc(&config.platform),
        rows: BTreeMap::new(),
        references: Vec::new(),
        content: StoreContent {
            blobs,
            small: BTreeMap::new(),
        },
    };
    // The walk stops wherever the codec half refuses the config's shape, and
    // the codec half then says why: the store half reads only what the codec
    // half would have bound before that point. The codec half binds the
    // bindings to the hardware's ROMs in order; a malformed firmware table
    // ends the list of ROMs where the codec half refuses it.
    let mut hardware = config.machine.clone();
    let mut roms = Vec::new();
    let _ = firmware_images(&mut hardware, |field, rom| {
        let image = rom.get("image").and_then(toml::Value::as_str);
        roms.push((field.to_owned(), image.map(str::to_owned)));
        Ok(())
    });
    for (binding, (field, image)) in config.firmware.iter().zip(&roms) {
        if binding.key != *field || image.as_deref() != Some(binding.source.as_str()) {
            return Ok(stored);
        }
        let (d, size) = bound(descriptors, &binding.blob, mt::FIRMWARE)?;
        stored.references.push(binding.blob.clone());
        // The push guard reads the ROM's source link from its descriptor.
        let link = d.annotations.get(image_store::license::SOURCE);
        if link
            != binding
                .source_archive
                .as_ref()
                .map(Digest::to_string)
                .as_ref()
        {
            return Err(invalid("firmware source link differs from binding"));
        }
        if let Some(archive) = &binding.source_archive {
            descriptors
                .iter()
                .find(|d| d.names(archive) && matches!(d.decoded_media_type(), mt::TAR | mt::ZIP))
                .ok_or_else(|| invalid("missing firmware source archive"))?;
            stored.references.push(archive.clone());
        }
        stored.rows.insert(
            binding.blob.clone(),
            codec::ContentRow {
                sha256: binding.blob.clone(),
                size,
            },
        );
        stored
            .content
            .small
            .entry(binding.blob.clone())
            .or_insert(d);
    }
    let mut ids = BTreeSet::new();
    if roms.len() != config.firmware.len()
        || config
            .media
            .iter()
            .any(|m| !ids.insert(m.id.as_str()) || m.id.is_empty())
    {
        return Ok(stored);
    }
    // Slots pair with the hardware's drives in order, as in the codec half; a
    // malformed card table ends the list of drives where the codec half
    // refuses it.
    let mut drives = Vec::new();
    let _ = drive_tables(&mut hardware, |controller, kind, drive| {
        drives.push((controller.to_owned(), kind.to_owned(), drive.clone()));
        Ok(())
    });
    let mut used = BTreeSet::new();
    for ((controller, kind, drive), slot) in drives.iter().zip(&config.slots) {
        if codec::check_media_fields(drive)
            .and_then(|()| codec::check_slot(controller, kind, drive, slot))
            .is_err()
        {
            return Ok(stored);
        }
        let ide = kind == "ide";
        for id in &slot.catalogue {
            let Some(medium) = config.media.iter().find(|m| &m.id == id) else {
                return Ok(stored);
            };
            let first_use = used.insert(id.clone());
            if !first_use && medium.policy != Policy::Readonly {
                return Ok(stored);
            }
            let payload_type = if ide {
                let head = descriptors
                    .iter()
                    .find(|d| d.names(&medium.head))
                    .ok_or_else(|| invalid("missing disk head"))?;
                match head.media_type.as_str() {
                    mt::DISK_LAYER => mt::DISK_LAYER,
                    DISK_ZERO => DISK_ZERO,
                    _ => mt::DISK_BASE,
                }
            } else {
                mt::FLOPPY
            };
            stored.references.push(medium.head.clone());
            let d = if first_use {
                bound(descriptors, &medium.head, payload_type)?.0
            } else {
                descriptor(descriptors, &medium.head, payload_type)?
            };
            let size = if ide && payload_type != mt::DISK_BASE {
                let refs =
                    crate::disk::references(&medium.head, &medium.geometry, descriptors, blobs)?;
                stored.references.extend(refs.into_iter().skip(1));
                // A zero-rooted disk has no bytes to load: only a journal over the
                // chain can mount it without materializing the whole disk.
                if medium.policy != Policy::Journal
                    && crate::zero::rooted(&medium.head, &medium.geometry, descriptors, blobs)?
                {
                    return Err(invalid(
                        "a blank (zero base) disk mounts only through a journal",
                    ));
                }
                crate::disk::geometry(&medium.geometry).capacity()? * 512
            } else {
                d.decoded_size()?
            };
            let sha256 = crate::disk::identity(&medium.head, descriptors, blobs)?;
            stored
                .rows
                .insert(medium.head.clone(), codec::ContentRow { sha256, size });
        }
    }
    if let Some(nvram) = &config.nvram {
        let (d, size) = bound(descriptors, &nvram.head, CMOS)?;
        stored.references.push(nvram.head.clone());
        if d.size != 64 {
            return Err(invalid("CMOS must contain exactly 64 bytes"));
        }
        stored.rows.insert(
            nvram.head.clone(),
            codec::ContentRow {
                sha256: nvram.head.clone(),
                size,
            },
        );
        stored.content.small.entry(nvram.head.clone()).or_insert(d);
    }
    Ok(stored)
}
/// The descriptor of `kind` for `digest` and its decoded size, within the PC
/// size limit of that kind.
fn bound(descriptors: &[Descriptor], digest: &Digest, kind: &str) -> Result<(Descriptor, u64)> {
    let d = descriptor(descriptors, digest, kind)?;
    let limit = PAYLOAD_TYPES
        .iter()
        .find(|t| t.media_type == kind)
        .map(|t| t.max_size)
        .ok_or_else(|| invalid("unsupported payload"))?;
    let size = d.decoded_size()?;
    if size > limit {
        return Err(invalid("payload exceeds PC size limit"));
    }
    Ok((d, size))
}
/// The firmware and CMOS blobs the store half bound, read whole and verified.
struct StoreContent<'a> {
    blobs: &'a dyn BlobSource,
    small: BTreeMap<Digest, Descriptor>,
}
impl codec::SmallContent for StoreContent<'_> {
    fn read(&self, digest: &Digest) -> io::Result<Vec<u8>> {
        let d = self
            .small
            .get(digest)
            .ok_or_else(|| io::Error::other("unbound logical asset"))?;
        let limit = PAYLOAD_TYPES
            .iter()
            .find(|t| t.media_type == d.decoded_media_type())
            .map(|t| t.max_size)
            .ok_or_else(|| io::Error::other("unsupported payload"))?;
        self.blobs.read_verified(d, limit).map_err(io::Error::other)
    }
}

/// The codec half: the config's envelope, its bindings against the hardware
/// tables, the slots, each medium's size against its geometry, and the
/// lowering. It reads nothing of the store: what it knows of content comes
/// from the store half's rows, and the firmware and CMOS bytes from a reader
/// of small content. Of the store's crate it names only `Digest`.
mod codec {
    use super::hash_array;
    use crate::{
        drive_tables, firmware_images, invalid, unit, validate_bios_with, Config, Policy, Result,
        Slot,
    };
    use image_store::Digest;
    use std::{
        collections::{BTreeMap, BTreeSet},
        io,
        path::Path,
    };

    /// What the codec half knows of one blob or disk head: the SHA-256 of the
    /// flat bytes a machine opens, and their length. The codec half reads a
    /// disk's rows here; the firmware and CMOS rows are not read yet, and are
    /// what a content source will open those blobs by.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct ContentRow {
        /// For a raw blob its own digest; for a layered head the chain's
        /// result; for a zero base the digest of that many zero bytes.
        pub(crate) sha256: Digest,
        /// The decoded size of a raw blob. For a layered or zero head, the
        /// capacity of the medium's geometry, which the store half's chain
        /// walk holds equal to the chain's root.
        pub(crate) size: u64,
    }
    /// Small content the lowering reads whole: a firmware ROM or a CMOS
    /// image, by the digest its binding names.
    pub(crate) trait SmallContent {
        fn read(&self, digest: &Digest) -> io::Result<Vec<u8>>;
    }
    pub(crate) struct Lowered {
        pub(crate) config: Config,
        pub(crate) machine: assembly::MachineConfig,
    }

    /// The path a fixed disk lowers from while its size is checked: a disk
    /// payload is never read in validation.
    const VALIDATION_DISK: &str = "__pc_image_validation_disk__";
    struct Reader<'a> {
        content: &'a dyn SmallContent,
        /// Logical asset name to its digest, and whether it is small content.
        bindings: BTreeMap<String, (Digest, bool)>,
    }
    impl machine_file::ImageReader for Reader<'_> {
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
            self.content.read(digest)
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
        reader
            .bindings
            .insert(source.into(), (digest.clone(), small));
        Ok(())
    }
    fn row<'a>(rows: &'a BTreeMap<Digest, ContentRow>, digest: &Digest) -> Result<&'a ContentRow> {
        rows.get(digest)
            .ok_or_else(|| invalid(format!("missing content row for {digest}")))
    }

    /// A stored drive table names no medium: the slots do.
    pub(crate) fn check_media_fields(drive: &toml::Table) -> Result<()> {
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
    /// A slot sits at its drive's position and has the shape its drive
    /// takes. The store half stops its walk where this refuses, so that the
    /// codec half's sentence is the one a person reads.
    pub(crate) fn check_slot(
        controller: &str,
        kind: &str,
        drive: &toml::Table,
        slot: &Slot,
    ) -> Result<()> {
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

    /// Lower `config`. `platform_known` is whether its platform names this
    /// provider, the alias check the store half makes; `options` carries the
    /// permission for unlisted firmware.
    pub(crate) fn lower(
        config: Config,
        rows: &BTreeMap<Digest, ContentRow>,
        content: &dyn SmallContent,
        platform_known: bool,
        options: assembly::AssemblyOptions,
    ) -> Result<Lowered> {
        if config.schema != 1 || !platform_known {
            return Err(invalid("unsupported PC config envelope"));
        }
        let mut hardware = config.machine.clone();
        let raw_text = toml::to_string(&hardware).map_err(|e| invalid(e.to_string()))?;
        machine_file::parse_toml("bundle.toml", &raw_text)
            .map_err(machine_file::LoadError::from)?;
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
            // above against the store half's size, and a disk is never read here.
            // The store verifies raw payloads; the resolved callback verifies a
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
        let document = machine_file::parse_toml("bundle.toml", &text)
            .map_err(machine_file::LoadError::from)?;
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
                if !m.geometry.fits(drive.kind) {
                    return Err(invalid("medium geometry differs from hardware"));
                }
                entries.push(assembly::FloppyMedium {
                    image: assembly::ImageSource::File(m.source.clone().into()),
                    expected_sha256: Some(hash_array(&row(rows, &m.head)?.sha256)?),
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
        validate_bios_with(&machine, options)?;
        Ok(Lowered { config, machine })
    }
}

/// Resolve a verified image to assembly input. Journal outputs use only the caller's
/// session directory; bases remain immutable blobs in the store. No session is created.
fn config_from_image(image: &image_store::ResolvedImage) -> Result<Config> {
    let table: toml::Table = std::str::from_utf8(&image.config)
        .map_err(|e| invalid(e.to_string()))?
        .parse()
        .map_err(|e: toml::de::Error| invalid(e.to_string()))?;
    decode(table)
}
/// Verified launch bases keyed by the path each drive names; zero-rooted heads
/// also keep their parts so a launch can move them to another thread.
pub(crate) type LaunchBases = (
    BTreeMap<std::path::PathBuf, media::VerifiedBase>,
    BTreeMap<std::path::PathBuf, std::rc::Rc<crate::zero::ZeroChain>>,
);
/// Assembly input only: a compressed journal floppy has no file, so launch
/// through `workspace::prepare_start`, which also returns the verified bases.
pub fn resolve(store: &Store, tag: &str, session: &Path) -> Result<assembly::MachineConfig> {
    Ok(resolve_pinned(store, &store.tag_descriptor(tag)?, session, None, None)?.1)
}
pub(crate) struct PreparedImage {
    pub image: image_store::ResolvedImage,
    pub config: Config,
    pub machine: assembly::MachineConfig,
    pub bases: BTreeMap<Digest, crate::disk::Disk>,
}
/// Resolve a pinned image. With `mount`, every medium is prepared for a
/// machine, which may materialize decoded files; without it the image is
/// only verified, nothing is written and `bases` stays empty.
pub(crate) fn load_pinned(
    store: &Store,
    descriptor: &Descriptor,
    publication: bool,
    mount: bool,
) -> Result<PreparedImage> {
    struct Capture(
        std::cell::RefCell<Option<(Prepared, BTreeMap<Digest, crate::disk::Disk>)>>,
        bool,
    );
    impl ConfigValidator for Capture {
        fn platform(&self) -> &str {
            PLATFORM
        }
        fn payload_types(&self) -> &[PayloadType] {
            PAYLOAD_TYPES
        }
        fn validate(
            &self,
            table: &toml::Table,
            descriptors: &[Descriptor],
            blobs: &dyn BlobSource,
        ) -> image_store::Result<Vec<Digest>> {
            let prepared = prepare(
                decode(table.clone()).map_err(store_error)?,
                descriptors,
                blobs,
            )
            .map_err(store_error)?;
            let refs = prepared.references.clone();
            self.0.replace(Some((prepared, BTreeMap::new())));
            Ok(refs)
        }
        fn validate_resolved(
            &self,
            image: &image_store::ResolvedImage,
            store: &Store,
        ) -> image_store::Result<()> {
            let mut capture = self.0.borrow_mut();
            let (prepared, bases) = capture
                .as_mut()
                .ok_or_else(|| image_store::Error::Invalid("provider was not called".into()))?;
            if self.1 {
                *bases = crate::disk::bases(store, &prepared.config, image).map_err(store_error)?;
            } else {
                crate::disk::check(store, &prepared.config, image).map_err(store_error)?;
            }
            Ok(())
        }
    }
    let capture = Capture(std::cell::RefCell::new(None), mount);
    let image = if publication {
        store.verify_publication(descriptor, &capture)?
    } else {
        store.resolve_descriptor(descriptor, &capture)?
    };
    if publication && image.metadata_read_only {
        return Err(invalid(
            "read-only metadata must be repaired before publication",
        ));
    }
    let (prepared, bases) = capture
        .0
        .into_inner()
        .ok_or_else(|| invalid("provider was not called"))?;
    Ok(PreparedImage {
        image,
        config: prepared.config,
        machine: prepared.machine,
        bases,
    })
}
/// `settings`, a machine's overlay of hardware tables, replaces the image's
/// for this launch; media, firmware and CMOS bases stay the image's.
pub(crate) fn resolve_pinned(
    store: &Store,
    descriptor: &Descriptor,
    session: &Path,
    overrides: Option<&BTreeMap<String, u32>>,
    settings: Option<&toml::Table>,
) -> Result<(
    image_store::ResolvedImage,
    assembly::MachineConfig,
    LaunchBases,
)> {
    let resolved = load_pinned(store, descriptor, false, true)?;
    let image = resolved.image;
    let mut prepared = match settings {
        None => Prepared {
            config: resolved.config,
            machine: resolved.machine,
            references: Vec::new(),
        },
        Some(machine) => {
            let packages = crate::revision::Packages::new(store);
            let overlay = crate::revision::overlay(
                &packages,
                &resolved.config,
                &image.manifest.layers,
                machine,
            )?;
            Prepared {
                config: overlay.config,
                machine: overlay.machine,
                references: Vec::new(),
            }
        }
    };
    let bases = resolved.bases;
    let record = crate::ImportRecord::read(store, &image.image.digest)?;
    let retention = |id: &str| {
        overrides
            .and_then(|map| map.get(id).copied())
            .or_else(|| {
                record
                    .as_ref()
                    .and_then(|r| r.generations.iter().find(|g| g.medium == id))
                    .and_then(|g| g.keep_generations)
            })
            .unwrap_or(assembly::WriteMode::DEFAULT_KEEP_GENERATIONS)
    };
    let base_path = |digest: &Digest| -> Result<std::path::PathBuf> {
        Ok(store.resolved_file_path(&image, digest)?)
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
                let disk = bases
                    .get(&m.head)
                    .ok_or_else(|| invalid("missing prepared disk"))?;
                entry.write = m.policy.write(session.join(format!("{}.upfj", m.id)));
                if let assembly::WriteMode::Journal {
                    keep_generations, ..
                } = &mut entry.write
                {
                    *keep_generations = retention(&m.id);
                }
                entry.image = source(store, disk, &entry.write)?;
                entry.expected_sha256 = Some(hash_array(&disk.identity)?);
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
            let disk = bases
                .get(&m.head)
                .ok_or_else(|| invalid("missing prepared disk"))?;
            drive.image = source(store, disk, &drive.write)?;
            drive.expected_sha256 = Some(hash_array(&disk.identity)?);
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
    let verified = bases
        .values()
        .map(|disk| (base_key(store, disk), disk.base.clone()))
        .collect();
    let zero = bases
        .values()
        .filter_map(|disk| Some((base_key(store, disk), disk.zero.clone()?)))
        .collect();
    Ok((image, prepared.machine, (verified, zero)))
}
/// The verified-base key assembly looks a medium up by: its file, or for a
/// compressed floppy held in memory a path under `cache/in-memory` that is
/// never created. Assembly reads such a journal base only through its
/// verified base, so nothing opens that path.
fn base_key(store: &Store, disk: &crate::disk::Disk) -> std::path::PathBuf {
    disk.path.clone().unwrap_or_else(|| {
        store
            .root()
            .join("cache/in-memory")
            .join(disk.identity.hex())
    })
}
/// A file-backed medium is opened by path. A compressed floppy stays in
/// memory: a journal needs a file-shaped source, so it names the in-memory
/// key; read-only and volatile drives receive the verified decoded bytes.
fn source(
    store: &Store,
    disk: &crate::disk::Disk,
    write: &assembly::WriteMode,
) -> Result<assembly::ImageSource> {
    use media::SectorSource;
    Ok(match (&disk.path, write) {
        (Some(path), _) => assembly::ImageSource::File(path.clone()),
        (None, assembly::WriteMode::Journal { .. }) => {
            assembly::ImageSource::File(base_key(store, disk))
        }
        (None, _) => {
            let mut bytes = vec![0; disk.base.byte_len() as usize];
            disk.base.read_range(0, &mut bytes)?;
            assembly::ImageSource::Inline(bytes)
        }
    })
}
/// Publish `target` as a compressed copy of the PC machine at `source`. The
/// local import record, which holds per-medium journal retention, is copied
/// to the new image digest before the tag appears.
pub fn compress_image(store: &Store, source: &str, target: &str) -> Result<Descriptor> {
    store.check_new_tag(target)?;
    let original = store.tag_descriptor(source)?;
    let result = store.compress_manifest(&original, &PcProvider)?;
    if let Some(record) = crate::ImportRecord::read(store, &original.digest)? {
        let record = crate::ImportRecord {
            manifest: result.clone(),
            ..record
        };
        store.write_local_record(
            &result.digest,
            toml::to_string(&record)
                .map_err(|e| invalid(e.to_string()))?
                .as_bytes(),
        )?;
    }
    store.update_tag(target, &result, None, &PcProvider)?;
    Ok(result)
}
pub(crate) fn hash_array(digest: &Digest) -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&digest.hex()[i * 2..i * 2 + 2], 16)
            .map_err(|e| invalid(e.to_string()))?;
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Blobs in memory by digest, with the digests read in order.
    struct Memory {
        blobs: BTreeMap<Digest, Vec<u8>>,
        reads: RefCell<Vec<Digest>>,
    }
    impl Memory {
        fn new<'a>(blobs: impl IntoIterator<Item = &'a [u8]>) -> Self {
            Self {
                blobs: blobs
                    .into_iter()
                    .map(|bytes| (Digest::of(bytes), bytes.to_vec()))
                    .collect(),
                reads: RefCell::default(),
            }
        }
    }
    impl codec::SmallContent for Memory {
        fn read(&self, digest: &Digest) -> io::Result<Vec<u8>> {
            self.reads.borrow_mut().push(digest.clone());
            self.blobs
                .get(digest)
                .cloned()
                .ok_or_else(|| io::Error::other("not in memory"))
        }
    }
    impl BlobSource for Memory {
        fn read_verified(&self, d: &Descriptor, limit: u64) -> image_store::Result<Vec<u8>> {
            let bytes = self
                .blobs
                .get(&d.digest)
                .ok_or_else(|| image_store::Error::Invalid("not in memory".into()))?;
            assert!(bytes.len() as u64 <= limit);
            Ok(bytes.clone())
        }
    }

    /// A stored config of an AT on the open Kiselev BIOS with a journalled
    /// 360K floppy and a journalled CMOS image, its descriptors and its
    /// three blobs; `None` when the BIOS fixture is missing.
    fn image() -> Option<(Config, Vec<Descriptor>, [Vec<u8>; 3])> {
        let bios = fixtures::fixture("kiselev-bios/bios-xi8088.bin")?;
        let floppy = vec![0xF6; 368_640];
        let cmos = vec![0; 64];
        let [b, f, c] = [&bios, &floppy, &cmos].map(|bytes| Digest::of(bytes));
        let text = format!(
            r#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[machine.board.rtc.nvram]
image = "assets/cmos.bin"
[machine.firmware.bios]
image = "assets/bios.bin"
sha256 = "{bios}"
[[machine.card]]
kind = "fdc-765"
[[machine.card.drives]]
unit = 0
type = "360K"
[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "{b}"
[[media]]
id = "a"
source = "assets/a.img"
head = "{f}"
policy = "journal"
geometry = {{ cylinders = 40, heads = 2, sectors = 9, sector_bytes = 512 }}
[[slots]]
controller = "fdc-765"
unit = 0
catalogue = ["a"]
catalogue_mode = false
inserted = "a"
[nvram]
mode = "journal"
source = "assets/cmos.bin"
head = "{c}"
"#,
            bios = b.hex(),
        );
        let config = decode(text.parse().unwrap()).unwrap();
        let descriptors = vec![
            Descriptor::new(mt::FIRMWARE, b, bios.len() as u64),
            Descriptor::new(mt::FLOPPY, f, floppy.len() as u64),
            Descriptor::new(CMOS, c, cmos.len() as u64),
        ];
        Some((config, descriptors, [bios, floppy, cmos]))
    }
    fn rows(blobs: &[Vec<u8>; 3]) -> BTreeMap<Digest, codec::ContentRow> {
        blobs
            .iter()
            .map(|bytes| {
                let digest = Digest::of(bytes);
                let row = codec::ContentRow {
                    sha256: digest.clone(),
                    size: bytes.len() as u64,
                };
                (digest, row)
            })
            .collect()
    }

    /// The codec half runs over a config, rows and small content in memory,
    /// with no descriptor, blob source or store anywhere: it reads the ROM
    /// and the CMOS image and never the floppy, whose bytes it is not even
    /// given. Its machine is the one the store's path lowers.
    #[test]
    fn the_codec_half_lowers_from_memory_and_reads_only_small_content() {
        let Some((config, descriptors, blobs)) = image() else {
            return;
        };
        let [bios, floppy, cmos] = &blobs;
        let small = Memory::new([&bios[..], &cmos[..]]);
        let lowered = codec::lower(
            config.clone(),
            &rows(&blobs),
            &small,
            true,
            assembly::AssemblyOptions::default(),
        )
        .unwrap();
        let mut reads = small.reads.take();
        reads.sort();
        let mut expected = vec![Digest::of(bios), Digest::of(cmos)];
        expected.sort();
        assert_eq!(reads, expected);
        let drive = &lowered.machine.drives[0];
        assert_eq!(
            drive.image,
            assembly::ImageSource::File("assets/a.img".into())
        );
        assert_eq!(
            drive.expected_sha256,
            Some(hash_array(&Digest::of(floppy)).unwrap())
        );
        assert!(matches!(
            &drive.write,
            assembly::WriteMode::Journal { path, .. } if path == Path::new("a.upfj")
        ));

        let store = Memory::new(blobs.iter().map(Vec::as_slice));
        let prepared = prepare(config, &descriptors, &store).unwrap();
        assert_eq!(prepared.machine, lowered.machine);
        assert_eq!(
            prepared.references,
            blobs.iter().map(|b| Digest::of(b)).collect::<Vec<_>>()
        );
    }

    /// What the codec half knows of a medium's size comes from its row, and
    /// whether the platform is this provider's from its argument.
    #[test]
    fn the_codec_half_refuses_from_its_rows_and_arguments() {
        let Some((config, _, blobs)) = image() else {
            return;
        };
        let small = Memory::new([&blobs[0][..], &blobs[2][..]]);
        let lower = |rows: &BTreeMap<Digest, codec::ContentRow>, platform_known| {
            codec::lower(
                config.clone(),
                rows,
                &small,
                platform_known,
                assembly::AssemblyOptions::default(),
            )
            .err()
            .map(|e| e.to_string())
        };
        let floppy = Digest::of(&blobs[1]);
        let mut short = rows(&blobs);
        short.get_mut(&floppy).unwrap().size -= 512;
        assert_eq!(
            lower(&short, true).as_deref(),
            Some("invalid PC image: medium geometry differs from payload length")
        );
        let mut missing = rows(&blobs);
        missing.remove(&floppy);
        assert_eq!(
            lower(&missing, true),
            Some(format!(
                "invalid PC image: missing content row for {floppy}"
            ))
        );
        assert_eq!(
            lower(&rows(&blobs), false).as_deref(),
            Some("invalid PC image: unsupported PC config envelope")
        );
    }

    /// An AT with a 360K floppy drive and then an IDE master, a journalled
    /// floppy `a` and fixed disk `c`, and `slots` as given (the heads of
    /// `a` and `c` stand for `HEAD_A` and `HEAD_C`). The BIOS is invented:
    /// these configs are refused before anything is lowered.
    fn two_drives(slots: &str) -> (Config, Vec<Descriptor>, Memory) {
        let bios = vec![0xF4; 0x8000];
        let floppy = vec![0xF6; 368_640];
        let disk = vec![0; 20 * 2 * 17 * 512];
        let [b, a, c] = [&bios, &floppy, &disk].map(|bytes| Digest::of(bytes));
        let text = format!(
            r#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[machine.firmware.bios]
image = "assets/bios.bin"
sha256 = "{bios}"
[[machine.card]]
kind = "fdc-765"
[[machine.card.drives]]
unit = 0
type = "360K"
[[machine.card]]
kind = "ide"
[[machine.card.drives]]
channel = "master"
geometry = {{ cylinders = 20, heads = 2, sectors = 17 }}
[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "{b}"
[[media]]
id = "a"
source = "assets/a.img"
head = "{a}"
policy = "journal"
geometry = {{ cylinders = 40, heads = 2, sectors = 9, sector_bytes = 512 }}
[[media]]
id = "c"
source = "assets/c.img"
head = "{c}"
policy = "journal"
geometry = {{ cylinders = 20, heads = 2, sectors = 17, sector_bytes = 512 }}
{slots}"#,
            bios = b.hex(),
            slots = slots.replace("HEAD_A", "a").replace("HEAD_C", "c"),
        );
        let config = decode(text.parse().unwrap()).unwrap();
        let descriptors = vec![
            Descriptor::new(mt::FIRMWARE, b, bios.len() as u64),
            Descriptor::new(mt::FLOPPY, a, floppy.len() as u64),
            Descriptor::new(mt::DISK_BASE, c, disk.len() as u64),
        ];
        let blobs = Memory::new([&bios[..], &floppy[..], &disk[..]]);
        (config, descriptors, blobs)
    }
    fn refusal(slots: &str) -> String {
        let (config, descriptors, blobs) = two_drives(slots);
        match prepare(config, &descriptors, &blobs) {
            Ok(_) => panic!("accepted"),
            Err(error) => error.to_string(),
        }
    }
    const FLOPPY_SLOT: &str = r#"[[slots]]
controller = "fdc-765"
unit = 0
catalogue = ["HEAD_A"]
catalogue_mode = false
inserted = "HEAD_A"
"#;

    const IDE_SLOT: &str = r#"[[slots]]
controller = "ide"
unit = 0
catalogue = ["HEAD_C"]
catalogue_mode = false
inserted = "HEAD_C"
"#;

    // A config with one defect in how its slots pair with the drives is
    // refused with the sentence that names that defect, never with the
    // store's sentence about a descriptor its medium was never meant to have.

    /// The fixed disk's slot before the floppy's: the drives are the other
    /// way round.
    #[test]
    fn slots_in_another_order_than_the_drives_are_refused_as_such() {
        assert_eq!(
            refusal(&format!("{IDE_SLOT}{FLOPPY_SLOT}")),
            "invalid PC image: slot binding differs from hardware position"
        );
    }

    /// The journalled floppy in the IDE slot as well.
    #[test]
    fn a_writable_medium_in_two_drives_of_two_kinds_is_refused_as_such() {
        let ide = IDE_SLOT.replace("HEAD_C", "HEAD_A");
        assert_eq!(
            refusal(&format!("{FLOPPY_SLOT}{ide}")),
            "invalid PC image: writable medium bound more than once"
        );
    }

    /// Two media for the IDE drive, the floppy first.
    #[test]
    fn an_ide_catalogue_of_two_media_is_refused_as_such() {
        let ide = IDE_SLOT
            .replace(r#"["HEAD_C"]"#, r#"["HEAD_A", "HEAD_C"]"#)
            .replace(r#"inserted = "HEAD_C""#, r#"inserted = "HEAD_A""#);
        assert_eq!(
            refusal(&format!("{FLOPPY_SLOT}{ide}")),
            "invalid PC image: IDE needs exactly one inserted medium"
        );
    }
}
