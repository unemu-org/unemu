// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC bundle-profile validation and lowering through schema 1; machine-images §2.
//! IBM Personal Computer AT Technical Reference, System Board and Diskette Drive
//! Adapter: controller-local units and exact geometry retain their existing wiring.

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
        "ibm-pc-at"
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
        if !matches!(d.decoded_media_type(), mt::FIRMWARE | CMOS) {
            return Err(io::Error::other("disk payload must stay file-backed"));
        }
        let limit = PAYLOAD_TYPES
            .iter()
            .find(|t| t.media_type == d.decoded_media_type())
            .map(|t| t.max_size)
            .ok_or_else(|| io::Error::other("unsupported payload"))?;
        self.blobs.read_verified(d, limit).map_err(io::Error::other)
    }
}
pub(crate) struct Prepared {
    pub(crate) config: Config,
    pub(crate) machine: assembly::MachineConfig,
    pub(crate) references: Vec<Digest>,
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
    if d.decoded_size()? > limit {
        return Err(invalid("payload exceeds PC size limit"));
    }
    reader.bindings.insert(source.into(), d.clone());
    refs.push(digest.clone());
    Ok(d)
}
pub(crate) fn prepare(
    config: Config,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<Prepared> {
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
        let d = bind(
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
            references.push(archive.clone());
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
            let disk_size = if kind == "ide" && payload_type != mt::DISK_BASE {
                let refs =
                    crate::disk::references(&medium.head, &medium.geometry, descriptors, blobs)?;
                references.extend(refs.into_iter().skip(1));
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
        // Lower only a bounded placeholder; descriptor geometry is validated above.
        // The store verifies raw payloads; the resolved callback verifies a layered head once.
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
                expected_sha256: Some(hash_array(&crate::disk::identity(
                    &m.head,
                    descriptors,
                    blobs,
                )?)?),
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
    Ok(resolve_pinned(store, &store.tag_descriptor(tag)?, session, None)?.1)
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
            "ibm-pc-at"
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
pub(crate) fn resolve_pinned(
    store: &Store,
    descriptor: &Descriptor,
    session: &Path,
    overrides: Option<&BTreeMap<String, u32>>,
) -> Result<(
    image_store::ResolvedImage,
    assembly::MachineConfig,
    LaunchBases,
)> {
    let resolved = load_pinned(store, descriptor, false, true)?;
    let image = resolved.image;
    let mut prepared = Prepared {
        config: resolved.config,
        machine: resolved.machine,
        references: Vec::new(),
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
