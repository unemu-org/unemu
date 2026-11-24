// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC bundle-profile validation and lowering through schema 1; machine-images §2.
//! IBM Personal Computer AT Technical Reference, System Board and Diskette Drive
//! Adapter: controller-local units keep their existing wiring. A fixed disk's
//! medium has the drive's exact geometry; a floppy medium has its payload's
//! exact geometry, one of the media the drive type takes
//! (`FloppyGeometry::accepted_media`).

use crate::*;
use image_store::{media_type as mt, BlobSource, ConfigValidator, PayloadType, Store};
use pc_config::image::{self as codec, ContentRow};
use pc_config::launch;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use unemu_core_api::{Content, ContentSource, ErrorKind};

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
        max_size: launch::MAX_FIRMWARE_BYTES,
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
        max_size: launch::MAX_FIRMWARE_BYTES,
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
        max_size: launch::CMOS_BYTES,
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
    fn required_release(
        &self,
        manifest: &image_store::Manifest,
        config: &[u8],
    ) -> image_store::Result<image_store::compat::AppVersion> {
        Ok(self.required_releases(manifest, config)?.core)
    }

    fn newest_frontend_row(&self) -> image_store::compat::AppVersion {
        crate::compat::newest_frontend_row()
    }

    fn core_release(
        &self,
    ) -> Option<(
        image_store::compat::AppVersion,
        image_store::compat::AppVersion,
    )> {
        Some((
            crate::compat::from_core(pc_config::compat::package()),
            crate::compat::core_newest_row(),
        ))
    }

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
    fn required_releases(
        &self,
        manifest: &image_store::Manifest,
        config: &[u8],
    ) -> image_store::Result<image_store::compat::Releases> {
        crate::compat::releases(manifest, config)
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
/// it, as the launch document the PC core reads (`pc_config::launch::lower`,
/// the one lowering of the store and the core): the content rows, and the
/// firmware and CMOS bytes through a `ContentSource` over the blobs. Media
/// and a journalled CMOS are named by their logical asset names and journals
/// by `<id>.upfj` and `cmos.upfj`, which a start puts its files in place of.
pub(crate) fn prepare(
    config: Config,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<Prepared> {
    // A launch also carries live-insert labels, but stored images keep
    // their original name restrictions.
    if config.media.iter().any(|medium| {
        medium
            .name
            .as_ref()
            .is_some_and(|name| name.contains(['/', '\\', '\0']) || name.is_empty())
    }) {
        return Err(invalid("invalid medium name"));
    }
    let stored = store_half(&config, descriptors, blobs)?;
    let launch = validation_launch(&config, &stored);
    let lowered = launch::lower(&launch, &stored.content, firmware_options())?;
    Ok(Prepared {
        config,
        machine: lowered.machine,
        references: stored.references,
    })
}

/// The launch document a stored config validates as: the config under the
/// platform's current spelling (an unknown platform stays as it is, and the
/// codec refuses it), the store half's content rows, and each journal in
/// the record of today's file name with the default retention. A start
/// sets the retention of its session instead.
fn validation_launch(config: &Config, stored: &Stored<'_>) -> launch::Launch {
    let mut config = config.clone();
    if stored.platform_known {
        config.platform = launch::PLATFORM.into();
    }
    let content = stored
        .rows
        .iter()
        .map(|(digest, row)| launch::ContentRow {
            digest: *digest,
            sha256: row.sha256,
            size: row.size,
        })
        .collect();
    let journalled = config
        .media
        .iter()
        .filter(|m| m.policy == Policy::Journal)
        .map(|m| m.id.as_str())
        .chain(
            config
                .nvram
                .as_ref()
                .filter(|n| matches!(n.mode, NvramMode::Journal))
                .map(|_| launch::CMOS_MEDIUM),
        );
    let records = journalled
        .map(|id| launch::RecordRow {
            medium: id.into(),
            name: format!("{id}{}", launch::RECORD_SUFFIX),
            keep_generations: assembly::WriteMode::DEFAULT_KEEP_GENERATIONS,
        })
        .collect();
    launch::Launch {
        config,
        content,
        records,
    }
}

/// Config-only launch document for a read-only stored-media snapshot.
pub(crate) fn export_document(
    config: &Config,
    descriptors: &[Descriptor],
    blobs: &dyn BlobSource,
) -> Result<launch::Launch> {
    Ok(validation_launch(
        config,
        &store_half(config, descriptors, blobs)?,
    ))
}

/// What the store half answers for one config.
struct Stored<'a> {
    /// Whether the config's platform names this provider: the PC id or its
    /// retired alias.
    platform_known: bool,
    /// One row per firmware blob, disk head and CMOS image the config binds.
    rows: BTreeMap<codec::Digest, ContentRow>,
    /// The closure, in the order the store expects it.
    references: Vec<Digest>,
    /// The firmware and CMOS bytes, read on demand.
    content: StoreContent<'a>,
}

/// The store half: every fact the descriptors and blobs decide. For each
/// digest the config binds it finds the descriptor of the right kind and
/// checks the PC size limit; it checks a ROM's source link and source
/// archive, walks a fixed disk's layer chain, applies the zero-base rule and
/// collects the closure. Stored slots keep their strict shape checks here:
/// launch lowering also accepts selections made by desktop edits. For other
/// malformed bindings the walk stops where the codec half would refuse them.
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
        let blob = binding.blob.store()?;
        let (d, size) = bound(descriptors, &blob, mt::FIRMWARE)?;
        stored.references.push(blob);
        // The push guard reads the ROM's source link from its descriptor.
        let link = d.annotations.get(image_store::license::SOURCE);
        if link
            != binding
                .source_archive
                .as_ref()
                .map(codec::Digest::to_string)
                .as_ref()
        {
            return Err(invalid("firmware source link differs from binding"));
        }
        if let Some(archive) = &binding.source_archive {
            let archive = archive.store()?;
            descriptors
                .iter()
                .find(|d| d.names(&archive) && matches!(d.decoded_media_type(), mt::TAR | mt::ZIP))
                .ok_or_else(|| invalid("missing firmware source archive"))?;
            stored.references.push(archive);
        }
        stored.rows.insert(
            binding.blob,
            ContentRow {
                sha256: binding.blob.sha256(),
                size,
            },
        );
        stored
            .content
            .small
            .entry(binding.blob.sha256())
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
        if codec::check_media_fields(drive).is_err() {
            return Ok(stored);
        }
        // Shared read-only media can leave every content row present even
        // after this walk stops; launch lowering permits a later selection.
        codec::check_slot(controller, kind, drive, slot)?;
        let ide = kind == "ide";
        for id in &slot.catalogue {
            let Some(medium) = config.media.iter().find(|m| &m.id == id) else {
                return Ok(stored);
            };
            let first_use = used.insert(id.clone());
            if !first_use && medium.policy != Policy::Readonly {
                return Ok(stored);
            }
            let head_digest = crate::medium_head(medium)?.store()?;
            let payload_type = if ide {
                let head = descriptors
                    .iter()
                    .find(|d| d.names(&head_digest))
                    .ok_or_else(|| invalid("missing disk head"))?;
                match head.media_type.as_str() {
                    mt::DISK_LAYER => mt::DISK_LAYER,
                    DISK_ZERO => DISK_ZERO,
                    _ => mt::DISK_BASE,
                }
            } else {
                mt::FLOPPY
            };
            stored.references.push(head_digest.clone());
            let d = if first_use {
                bound(descriptors, &head_digest, payload_type)?.0
            } else {
                descriptor(descriptors, &head_digest, payload_type)?
            };
            let size = if ide && payload_type != mt::DISK_BASE {
                let refs =
                    crate::disk::references(&head_digest, &medium.geometry, descriptors, blobs)?;
                stored.references.extend(refs.into_iter().skip(1));
                // A zero-rooted disk has no bytes to load: only a journal over the
                // chain can mount it without materializing the whole disk.
                if medium.policy != Policy::Journal
                    && crate::zero::rooted(&head_digest, &medium.geometry, descriptors, blobs)?
                {
                    return Err(invalid(
                        "a blank (zero base) disk mounts only through a journal",
                    ));
                }
                crate::disk::geometry(&medium.geometry).capacity()? * 512
            } else {
                d.decoded_size()?
            };
            let sha256 = hash_array(&crate::disk::identity(&head_digest, descriptors, blobs)?)?;
            stored
                .rows
                .insert(crate::medium_head(medium)?, ContentRow { sha256, size });
        }
    }
    if let Some(nvram) = &config.nvram {
        let head = nvram.head.store()?;
        let (d, size) = bound(descriptors, &head, CMOS)?;
        stored.references.push(head);
        if d.size != 64 {
            return Err(invalid("CMOS must contain exactly 64 bytes"));
        }
        stored.rows.insert(
            nvram.head,
            ContentRow {
                sha256: nvram.head.sha256(),
                size,
            },
        );
        stored.content.small.entry(nvram.head.sha256()).or_insert(d);
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
/// The firmware and CMOS blobs the store half bound, by the SHA-256 of
/// their rows, read whole and verified: the content source the codec half
/// opens small content through.
struct StoreContent<'a> {
    blobs: &'a dyn BlobSource,
    small: BTreeMap<[u8; 32], Descriptor>,
}
impl ContentSource for StoreContent<'_> {
    fn open(
        &self,
        sha256: &[u8; 32],
        size: u64,
    ) -> std::result::Result<Content, unemu_core_api::Error> {
        let content = |message: String| unemu_core_api::Error::new(ErrorKind::Content, message);
        let d = self
            .small
            .get(sha256)
            .ok_or_else(|| content("unbound logical asset".into()))?;
        let limit = PAYLOAD_TYPES
            .iter()
            .find(|t| t.media_type == d.decoded_media_type())
            .map(|t| t.max_size)
            .ok_or_else(|| content("unsupported payload".into()))?;
        let bytes = self.blobs.read_verified(d, limit).map_err(|e| match e {
            image_store::Error::Io(_) => {
                unemu_core_api::Error::new(ErrorKind::Storage, e.to_string())
            }
            e => content(e.to_string()),
        })?;
        if bytes.len() as u64 != size {
            return Err(content("content size differs from its row".into()));
        }
        Ok(Content::Bytes(bytes))
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
    BTreeMap<std::path::PathBuf, journal_files::VerifiedBase>,
    BTreeMap<std::path::PathBuf, std::rc::Rc<crate::zero::ZeroChain>>,
);
pub(crate) struct PreparedImage {
    pub image: image_store::ResolvedImage,
    pub config: Config,
    pub machine: assembly::MachineConfig,
    pub bases: BTreeMap<codec::Digest, crate::disk::Disk>,
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
        std::cell::RefCell<Option<(Prepared, BTreeMap<codec::Digest, crate::disk::Disk>)>>,
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
    let validator = crate::compat::WithPcRows(&capture);
    let image = if publication {
        store.verify_publication(descriptor, &validator)?
    } else {
        store.resolve_descriptor(descriptor, &validator)?
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
/// The effective slot configuration shared by runs and read-only exports.
pub(crate) fn select_config(
    store: &Store,
    resolved: &PreparedImage,
    settings: Option<&toml::Table>,
) -> Result<(Config, Vec<Descriptor>, Option<[u8; 64]>)> {
    let image = &resolved.image;
    Ok(match settings {
        None => (resolved.config.clone(), image.manifest.layers.clone(), None),
        Some(machine) => {
            let packages = crate::revision::Packages::new(store);
            let overlay = crate::revision::overlay(
                &packages,
                &resolved.config,
                &image.manifest.layers,
                machine,
            )?;
            let descriptors = image
                .manifest
                .layers
                .iter()
                .cloned()
                .chain(overlay.firmware())
                .collect();
            (overlay.config, descriptors, overlay.cmos)
        }
    })
}

/// `settings`, a machine's overlay of hardware tables, replaces the image's
/// for this launch; disk bases stay the image's.
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
    crate::workspace::PreparedLaunch,
)> {
    use journal_files::SectorSource;
    let resolved = load_pinned(store, descriptor, false, true)?;
    let (config, descriptors, cmos) = select_config(store, &resolved, settings)?;
    let image = resolved.image;
    let stored = store_half(&config, &descriptors, store)?;
    let mut document = validation_launch(&config, &stored);
    let mut small = BTreeMap::new();
    for (sha256, descriptor) in &stored.content.small {
        let size = descriptor.decoded_size()?;
        if let Content::Bytes(bytes) = stored
            .content
            .open(sha256, size)
            .map_err(|e| invalid(e.to_string()))?
        {
            small.insert(*sha256, bytes);
        }
    }
    let bases = resolved.bases;
    for medium in &document.config.media {
        let disk = bases
            .get(&crate::medium_head(medium)?)
            .ok_or_else(|| invalid("missing prepared disk"))?;
        let row = document
            .content
            .iter_mut()
            .find(|r| Some(r.digest) == medium.head)
            .ok_or_else(|| invalid("missing content row"))?;
        row.sha256 = disk.base.identity().base_digest;
        row.size = disk.base.byte_len();
    }
    // A copied CMOS is recomputed for Settings at each start. Give those
    // bytes their own binding so lowering the document is the whole answer.
    if let Some(bytes) = cmos {
        let digest = codec::Digest::of(&bytes);
        let nvram = document
            .config
            .nvram
            .as_mut()
            .ok_or_else(|| invalid("missing CMOS binding"))?;
        nvram.head = digest;
        if !document.content.iter().any(|r| r.digest == digest) {
            document.content.push(launch::ContentRow {
                digest,
                sha256: digest.sha256(),
                size: bytes.len() as u64,
            });
        }
        small.insert(digest.sha256(), bytes.to_vec());
    }
    let imported = crate::ImportRecord::read(store, &image.image.digest)?;
    for row in &mut document.records {
        row.keep_generations = overrides
            .and_then(|map| map.get(&row.medium).copied())
            .or_else(|| {
                imported
                    .as_ref()
                    .and_then(|r| r.generations.iter().find(|g| g.medium == row.medium))
                    .and_then(|g| g.keep_generations)
            })
            .unwrap_or(assembly::WriteMode::DEFAULT_KEEP_GENERATIONS);
    }
    let mut sources = BTreeMap::new();
    let mut memory = BTreeMap::new();
    for medium in &document.config.media {
        let disk = bases
            .get(&crate::medium_head(medium)?)
            .ok_or_else(|| invalid("missing prepared disk"))?;
        sources.insert(medium.source.clone().into(), base_key(store, disk));
        if disk.path.is_none() {
            let mut bytes = vec![0; disk.base.byte_len() as usize];
            disk.base.read_range(0, &mut bytes)?;
            memory.insert(medium.source.clone().into(), bytes);
        }
    }
    if let Some(n) = &document.config.nvram {
        if matches!(n.mode, NvramMode::Journal) {
            sources.insert(
                n.source.clone().into(),
                store.resolved_file_path(&image, &n.head.store()?)?,
            );
        }
    }
    let records = document
        .records
        .iter()
        .map(|r| (r.name.clone(), session.join(&r.name)))
        .collect();
    let mut launch = crate::workspace::PreparedLaunch {
        document,
        small,
        sources,
        memory,
        records,
    };
    launch.prune();
    let machine = lower_session(&launch)?;
    let verified = bases
        .values()
        .map(|disk| (base_key(store, disk), disk.base.clone()))
        .collect();
    let zero = bases
        .values()
        .filter_map(|disk| Some((base_key(store, disk), disk.zero.clone()?)))
        .collect();
    Ok((image, machine, (verified, zero), launch))
}

/// Keep a host-path view for catalogue editors, pending CMOS resets and desktop
/// media tools. The core mounts logical keys through the launch plans instead.
/// All hardware, catalogue and retention choices come from the same document.
pub(crate) fn lower_session(
    input: &crate::workspace::PreparedLaunch,
) -> Result<assembly::MachineConfig> {
    for medium in &input.document.config.media {
        if !unemu_core_api::is_record_name(&format!("{}{}", medium.id, launch::RECORD_SUFFIX))
            || medium.id.starts_with(unemu_core_api::KEPT_RECORD_PREFIX)
        {
            return Err(invalid(format!(
                "the medium id {:?} cannot form a core record name",
                medium.id
            )));
        }
    }
    let lowered = launch::lower(&input.document, input, firmware_options())?;
    let mut machine = lowered.machine;
    let source = |image: &mut assembly::ImageSource, write: &assembly::WriteMode| -> Result<()> {
        if let assembly::ImageSource::File(key) = image {
            if !lowered.bases.contains_key(key) {
                return Err(invalid("unbound launch base key"));
            }
            let path = input
                .sources
                .get(key)
                .ok_or_else(|| invalid("missing launch base path"))?;
            *image = match (input.memory.get(key), write) {
                (Some(bytes), mode) if !matches!(mode, assembly::WriteMode::Journal { .. }) => {
                    assembly::ImageSource::Inline(bytes.clone())
                }
                _ => assembly::ImageSource::File(path.clone()),
            };
        }
        Ok(())
    };
    let record = |key: &mut std::path::PathBuf| -> Result<()> {
        let name = lowered
            .records
            .get(key)
            .ok_or_else(|| invalid("unbound launch record key"))?;
        *key = input
            .records
            .get(name)
            .ok_or_else(|| invalid("missing launch record path"))?
            .clone();
        Ok(())
    };
    let write = |write: &mut assembly::WriteMode| -> Result<()> {
        if let assembly::WriteMode::Journal { path, .. } = write {
            record(path)?;
        }
        Ok(())
    };
    for drive in &mut machine.drives {
        source(&mut drive.image, &drive.write)?;
        write(&mut drive.write)?;
        if let Some(list) = &mut drive.media {
            for medium in &mut list.entries {
                source(&mut medium.image, &medium.write)?;
                write(&mut medium.write)?;
            }
        }
    }
    for device in &mut machine.devices {
        if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
            if let assembly::NvramSource::Journal { image, journal, .. } = &mut rtc.nvram {
                let mut base = assembly::ImageSource::File(image.clone());
                source(&mut base, &assembly::WriteMode::ReadOnly)?;
                let assembly::ImageSource::File(path) = base else {
                    return Err(invalid("CMOS base must be a file"));
                };
                *image = path;
                record(journal)?;
            }
        }
    }
    Ok(machine)
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
use pc_harness::test_support::fixtures;

#[cfg(test)]
#[path = "../tests/support/core_pin.rs"]
mod core_pin;

#[cfg(test)]
mod tests {
    use super::*;
    use core_pin::assert_core_revision;
    use std::cell::RefCell;

    // Moving the core pin requires checking the recorded golden again.
    const CORE_REV: &str = "8b8f1d3818c0805368448a14c58c850d188bb74f";

    #[test]
    fn golden_core_revision_matches_lock() {
        assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
    }

    #[test]
    fn stored_image_refuses_document_owned_zero_disk_without_reading_content() {
        let config: toml::Table = r#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[[media]]
id = "disk"
source = "assets/disk.img"
zero = { size = 512 }
policy = "journal"
geometry = { cylinders = 1, heads = 1, sectors = 1, sector_bytes = 512 }
"#
        .parse()
        .unwrap();
        let blobs = Memory::new([]);
        let error = PcProvider.validate(&config, &[], &blobs).unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid image: invalid PC image: medium \"disk\": document-owned zero disks without a content head are not supported"
        );
        assert!(blobs.reads.borrow().is_empty());
    }

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
    impl ContentSource for Memory {
        fn open(
            &self,
            sha256: &[u8; 32],
            size: u64,
        ) -> std::result::Result<Content, unemu_core_api::Error> {
            let digest = codec::Digest::from_sha256(*sha256).store().unwrap();
            self.reads.borrow_mut().push(digest.clone());
            self.blobs
                .get(&digest)
                .filter(|bytes| bytes.len() as u64 == size)
                .map(|bytes| Content::Bytes(bytes.clone()))
                .ok_or_else(|| unemu_core_api::Error::new(ErrorKind::Content, "not in memory"))
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
    fn rows(blobs: &[Vec<u8>; 3]) -> BTreeMap<codec::Digest, ContentRow> {
        blobs
            .iter()
            .map(|bytes| {
                let digest = codec::Digest::of(bytes);
                let row = ContentRow {
                    sha256: digest.sha256(),
                    size: bytes.len() as u64,
                };
                (digest, row)
            })
            .collect()
    }

    /// The codec half runs over a config, rows and small content in memory,
    /// with no descriptor, blob source or store anywhere: it opens the ROM
    /// and the CMOS image by their rows and never the floppy, whose bytes it
    /// is not even given. Its machine is the one the store's path lowers,
    /// but for the journalled CMOS: the store validates through the launch
    /// document, which lowers it to an NVRAM journal over the CMOS image
    /// instead of a copy of its bytes.
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
        let mut expected = lowered.machine;
        for device in &mut expected.devices {
            if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                assert_eq!(
                    rtc.nvram,
                    assembly::NvramSource::Image(cmos.as_slice().try_into().unwrap())
                );
                rtc.nvram = assembly::NvramSource::Journal {
                    image: "assets/cmos.bin".into(),
                    expected_sha256: hash_array(&Digest::of(cmos)).unwrap(),
                    journal: "cmos.upfj".into(),
                    keep_generations: assembly::WriteMode::DEFAULT_KEEP_GENERATIONS,
                };
            }
        }
        assert_eq!(prepared.machine, expected);
        assert_eq!(
            prepared.references,
            blobs.iter().map(|b| Digest::of(b)).collect::<Vec<_>>()
        );
    }

    // The codec half's refusals from its rows and its platform argument are
    // tested where the codec lives, in `pc_config::image`.

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
    fn test_blob(store: &Store, kind: &str, bytes: &[u8]) -> Descriptor {
        let blob = store.import(bytes, None).unwrap();
        Descriptor::new(kind, blob.digest, blob.size)
    }

    fn start_fixture(
        mutate: impl FnOnce(&mut Config),
    ) -> Option<(image_store::test_support::TempDir, Store)> {
        let (mut config, mut descriptors, blobs) = image()?;
        mutate(&mut config);
        let temp = image_store::test_support::TempDir::new("launch-start");
        let store = Store::open(temp.path().join("store"))
            .unwrap()
            .with_compatibility(crate::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
        for bytes in &blobs {
            store.import(bytes.as_slice(), None).unwrap();
        }
        descriptors.push(test_blob(&store, mt::METADATA, crate::EMPTY_METADATA));
        let bytes = config.encode().unwrap();
        let blob = store.import(bytes.as_bytes(), None).unwrap();
        let manifest = image_store::Manifest::new_with_types(
            PLATFORM,
            Descriptor::new(mt::CONFIG, blob.digest, blob.size),
            descriptors,
            PAYLOAD_TYPES,
        )
        .unwrap();
        let image = store
            .import_manifest_with_types(&manifest, PAYLOAD_TYPES)
            .unwrap();
        store
            .update_tag("test:launch", &image, None, &PcProvider)
            .unwrap();
        Some((temp, store))
    }

    #[test]
    fn launch_edits_rederive_catalogues_and_retention_and_survive_transfer() {
        let Some((_temp, store)) = start_fixture(|_| {}) else {
            return;
        };
        let owner = store.workspace("launch").unwrap();
        let mut session = crate::workspace::prepare_start(&owner, "test:launch", false).unwrap();
        let before_bases: Vec<_> = session.bases.keys().cloned().collect();
        let original = session.machine.clone();
        session
            .edit_launch(|document| {
                document.config.media[0].name = Some("Launch disk".into());
                document.config.slots[0].catalogue_mode = true;
                document.config.slots[0].inserted = None;
                for row in &mut document.records {
                    row.keep_generations = 7;
                }
            })
            .unwrap();
        assert_ne!(session.machine, original);
        let list = session.machine.drives[0].media.as_ref().unwrap();
        assert!(list.empty);
        assert_eq!(list.entries[0].name, "Launch disk");
        assert!(
            matches!(&list.entries[0].write, assembly::WriteMode::Journal { path, keep_generations: 7 } if path == &owner.path().join("media/a.upfj"))
        );
        assert!(session.machine.devices.iter().any(|d| matches!(&d.kind, assembly::DeviceKind::Rtc(rtc) if matches!(&rtc.nvram, assembly::NvramSource::Journal { keep_generations: 7, .. }))));
        assert_eq!(
            session.bases.keys().cloned().collect::<Vec<_>>(),
            before_bases
        );
        let expected = session.machine.clone();
        let document = launch::encode(session.launch.as_ref().unwrap().document()).unwrap();
        let transfer = session.into_transfer().unwrap();
        let mut restored =
            std::thread::spawn(move || transfer.restore().unwrap().into_transfer().unwrap())
                .join()
                .unwrap()
                .restore()
                .unwrap();
        assert_eq!(restored.machine, expected);
        assert_eq!(
            launch::encode(restored.launch.as_ref().unwrap().document()).unwrap(),
            document
        );
        restored
            .edit_launch(|document| {
                document.config.slots[0].inserted = Some("a".into());
            })
            .unwrap();
        assert!(!restored.machine.drives[0].media.as_ref().unwrap().empty);
    }

    #[test]
    fn a_refused_launch_edit_keeps_both_the_document_and_the_machine() {
        let Some((_temp, store)) = start_fixture(|_| {}) else {
            return;
        };
        let owner = store.workspace("launch").unwrap();
        let mut session = crate::workspace::prepare_start(&owner, "test:launch", false).unwrap();
        let machine = session.machine.clone();
        let document = launch::encode(session.launch.as_ref().unwrap().document()).unwrap();
        let error = session
            .edit_launch(|document| {
                document.records[0].keep_generations = 0;
            })
            .unwrap_err();
        assert!(
            error.to_string().contains("at least one generation"),
            "{error}"
        );
        assert_eq!(session.machine, machine);
        assert_eq!(
            launch::encode(session.launch.as_ref().unwrap().document()).unwrap(),
            document
        );
    }

    #[test]
    fn a_start_refuses_medium_ids_that_cannot_name_core_records_even_when_readonly() {
        for id in ["-a", "kept-a"] {
            let Some((_temp, store)) = start_fixture(|config| {
                config.media[0].id = id.into();
                config.media[0].policy = Policy::Readonly;
                config.slots[0].catalogue = vec![id.into()];
                config.slots[0].inserted = Some(id.into());
            }) else {
                return;
            };
            let owner = store.workspace("launch").unwrap();
            let error = crate::workspace::prepare_start(&owner, "test:launch", false)
                .err()
                .unwrap();
            assert_eq!(
                error.to_string(),
                format!("invalid PC image: the medium id {id:?} cannot form a core record name")
            );
            assert!(owner.read("session.toml").unwrap().is_none());
        }
    }

    #[test]
    fn pending_inserts_edit_the_launch_and_keep_the_previous_catalogue_and_journal_paths() {
        use image_store::media_set;
        let Some((_temp, store)) = start_fixture(|config| {
            config.media[0].source = "assets/insert-0.img".into();
        }) else {
            return;
        };
        let blob = store.import(vec![0x42; 368_640].as_slice(), None).unwrap();
        let set_config = media_set::Config {
            schema: 1,
            platform: PLATFORM.into(),
            extras: Vec::new(),
            media: ["one", "two"]
                .into_iter()
                .map(|id| media_set::Medium {
                    id: id.into(),
                    label: format!("Disk {id} / side \\"),
                    format: crate::media_set::format("360K").unwrap(),
                    blob: blob.digest.clone(),
                })
                .collect(),
        };
        let bytes = set_config.encode().unwrap();
        let config = store.import(bytes.as_slice(), None).unwrap();
        let manifest = image_store::Manifest::new_media_set(
            PLATFORM,
            Descriptor::new(mt::MEDIA_SET_CONFIG, config.digest, config.size),
            vec![
                Descriptor::new(mt::FLOPPY, blob.digest.clone(), blob.size),
                test_blob(&store, mt::METADATA, crate::EMPTY_METADATA),
            ],
        )
        .unwrap();
        let set = store.import_manifest(&manifest).unwrap();
        for (all, journal) in [(false, false), (false, true), (true, true)] {
            let id = crate::machine::create_from_tag(&store, "test:launch", None, None).unwrap();
            let insert = image_store::machine::PendingInsert {
                drive: "A:".into(),
                set: set.clone(),
                reference: "test:set".into(),
                medium: "two".into(),
                all,
                journal,
            };
            crate::machine::set_pending_insert(&store, &id, "A:", Some(insert)).unwrap();
            let owner = crate::machine::own(&store, &id).unwrap();
            let original = crate::workspace::prepare_start(&owner, "test:launch", false).unwrap();
            let mut expected = original.machine.clone();
            let resolved = store
                .resolve_descriptor(
                    &set,
                    &media_set::Validator(&crate::media_set::PcMediaFormats),
                )
                .unwrap();
            let path = store.resolved_file_path(&resolved, &blob.digest).unwrap();
            let mut entries = Vec::new();
            for label in if all { vec!["two", "one"] } else { vec!["two"] } {
                entries.push(assembly::FloppyMedium {
                    image: assembly::ImageSource::File(path.clone()),
                    expected_sha256: Some(hash_array(&blob.digest).unwrap()),
                    write: if journal {
                        assembly::WriteMode::journal(
                            owner
                                .path()
                                .join(format!("media/{}-{label}.upfj", set.digest.hex())),
                        )
                    } else {
                        assembly::WriteMode::ReadOnly
                    },
                    name: format!("Disk {label} / side \\"),
                });
            }
            let drive = &mut expected.drives[0];
            entries.push(assembly::FloppyMedium {
                image: drive.image.clone(),
                expected_sha256: drive.expected_sha256,
                write: drive.write.clone(),
                name: "A: image".into(),
            });
            drive.media = Some(assembly::FloppyMediaList {
                entries,
                empty: false,
                inserted: 0,
            });
            drive.image = assembly::ImageSource::Blank {
                size: drive.kind.image_bytes(),
            };
            drive.expected_sha256 = None;
            drive.write = assembly::WriteMode::Volatile;
            let mut prepared = crate::machine::prepare(&owner).unwrap();
            assert_eq!(prepared.session.machine, expected);
            let document = prepared.session.launch.as_ref().unwrap().document();
            let slot = &document.config.slots[0];
            assert_eq!(slot.inserted.as_ref(), slot.catalogue.first());
            assert_eq!(slot.catalogue.len(), if all { 3 } else { 2 });
            let added = document
                .config
                .media
                .iter()
                .find(|m| Some(&m.id) == slot.inserted.as_ref())
                .unwrap();
            assert_eq!(added.name.as_deref(), Some("Disk two / side \\"));
            assert!(document
                .records
                .iter()
                .filter(|r| r.medium.starts_with("insert-"))
                .all(|r| unemu_core_api::is_kept_record_name(&r.name)));
            prepared
                .session
                .edit_launch(|document| {
                    document.config.slots[0].inserted = None;
                })
                .unwrap();
            assert!(
                prepared.session.machine.drives[0]
                    .media
                    .as_ref()
                    .unwrap()
                    .empty
            );
        }
    }

    #[test]
    fn stored_image_start_keeps_its_retired_count() {
        use cpu::Cpu;
        assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
        let Some((_temp, store)) = start_fixture(|_| {}) else {
            return;
        };
        let owner = store.workspace("count").unwrap();
        let launch = crate::launch::prepare_workspace(
            &owner,
            "test:launch",
            false,
            &unemu_pc_core::PcCore,
            "Retired count".into(),
        )
        .unwrap();
        let mut pc = core_launch::start_with(&launch.prepared, |request| {
            unemu_pc_core::PcCore.create_machine(request)
        })
        .unwrap();
        let assembled = pc.assembled();
        let outcome = assembled
            .run_session(&assembly::RunPolicy::natural(
                Some(100_000),
                Some(1_000_000),
            ))
            .unwrap();
        assert_eq!(outcome, ::machine::RunOutcome::RetiredCapReached);
        assert_eq!(assembled.machine.cpu().retired(), 67_234);
        assert_eq!(assembled.machine.clock().0, 162_500);
        assembled.finish_persistence().unwrap();
    }
    #[test]
    fn stored_shared_catalogues_refuse_a_non_first_selection() {
        let Some((mut config, mut descriptors, blobs)) = image() else {
            return;
        };
        config.nvram = None;
        config.machine["board"]["rtc"]
            .as_table_mut()
            .unwrap()
            .remove("nvram");
        config.media[0].policy = Policy::Readonly;
        config.media[0].name = Some("Disk a".into());
        let floppy = vec![0x31; 368_640];
        let mut second = config.media[0].clone();
        second.id = "b".into();
        second.source = "assets/b.img".into();
        second.name = Some("Disk b".into());
        second.head = Some(codec::Digest::of(&floppy));
        config.media.push(second);
        descriptors.push(Descriptor::new(
            mt::FLOPPY,
            Digest::of(&floppy),
            floppy.len() as u64,
        ));
        let drives = config.machine["card"][0]["drives"].as_array_mut().unwrap();
        let mut drive = drives[0].clone();
        drive["unit"] = 1.into();
        drives.push(drive);
        config.slots[0].catalogue = vec!["a".into(), "b".into()];
        config.slots[0].catalogue_mode = true;
        let mut slot = config.slots[0].clone();
        slot.unit = 1;
        config.slots.push(slot);
        let content = Memory::new(blobs.iter().map(Vec::as_slice).chain([floppy.as_slice()]));
        let table = config.encode().unwrap().parse().unwrap();
        PcProvider.validate(&table, &descriptors, &content).unwrap();

        config.slots[1].inserted = Some("b".into());
        let table = config.encode().unwrap().parse().unwrap();
        let error = PcProvider
            .validate(&table, &descriptors, &content)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid image: invalid PC image: unsupported initial selection"
        );
    }

    #[test]
    fn stored_catalogue_labels_keep_their_original_restrictions() {
        let Some((config, descriptors, blobs)) = image() else {
            return;
        };
        for label in ["Disk / side", "Disk \\ side", "", "Disk\0side"] {
            let mut config = config.clone();
            config.media[0].name = Some(label.into());
            config.slots[0].catalogue_mode = true;
            let content = Memory::new(blobs.iter().map(Vec::as_slice));
            let stored = prepare(config.clone(), &descriptors, &content);
            assert_eq!(
                stored.err().map(|e| e.to_string()).as_deref(),
                Some("invalid PC image: invalid medium name"),
                "stored label {label:?}"
            );
            let lowered = codec::lower(
                config,
                &rows(&blobs),
                &content,
                true,
                assembly::AssemblyOptions::default(),
            );
            assert_eq!(
                lowered.err().map(|e| e.to_string()).as_deref(),
                Some("invalid PC image: invalid medium name"),
                "codec label {label:?}"
            );
        }
    }

    #[test]
    fn two_drive_inserts_reuse_source_keys_without_stale_compressed_bytes() {
        use image_store::media_set;
        let Some((_temp, store)) = start_fixture(|config| {
            config.media[0].id = "insert-1".into();
            config.media[0].source = "assets/insert-1.img".into();
            config.media[0].name = Some("Disk a".into());
            config.media[0].policy = Policy::Readonly;
            config.slots[0].catalogue = vec!["insert-1".into()];
            config.slots[0].inserted = Some("insert-1".into());
            config.slots[0].catalogue_mode = true;
            let drives = config.machine["card"][0]["drives"].as_array_mut().unwrap();
            let mut second = drives[0].clone();
            second["unit"] = 1.into();
            drives.push(second);
            let mut second = config.slots[0].clone();
            second.unit = 1;
            second.catalogue.clear();
            second.inserted = None;
            config.slots.push(second);
        }) else {
            return;
        };
        compress_image(&store, "test:launch", "test:compressed").unwrap();
        let disks = [
            test_blob(&store, mt::FLOPPY, &vec![0xf6; 368_640]),
            test_blob(&store, mt::FLOPPY, &vec![0x42; 368_640]),
        ];
        let config = media_set::Config {
            schema: 1,
            platform: PLATFORM.into(),
            extras: Vec::new(),
            media: ["a", "b"]
                .into_iter()
                .zip(&disks)
                .map(|(id, disk)| media_set::Medium {
                    id: id.into(),
                    label: format!("Disk {id}"),
                    format: crate::media_set::format("360K").unwrap(),
                    blob: disk.digest.clone(),
                })
                .collect(),
        };
        let config = test_blob(&store, mt::MEDIA_SET_CONFIG, &config.encode().unwrap());
        let manifest = image_store::Manifest::new_media_set(
            PLATFORM,
            config,
            vec![
                disks[0].clone(),
                disks[1].clone(),
                test_blob(&store, mt::METADATA, crate::EMPTY_METADATA),
            ],
        )
        .unwrap();
        let set = store.import_manifest(&manifest).unwrap();
        let id = crate::machine::create_from_tag(&store, "test:compressed", None, None).unwrap();
        let inserts = [("A:", "a"), ("B:", "b")]
            .into_iter()
            .map(|(drive, medium)| image_store::machine::PendingInsert {
                drive: drive.into(),
                set: set.clone(),
                reference: "test:set".into(),
                medium: medium.into(),
                all: false,
                journal: false,
            })
            .collect();
        crate::machine::set_pending_inserts(&store, &id, inserts).unwrap();
        let owner = crate::machine::own(&store, &id).unwrap();
        let prepared = crate::machine::prepare(&owner).unwrap();
        let input = prepared.session.launch.as_ref().unwrap();
        assert_eq!(input.document.config.slots[0].catalogue, ["insert-0"]);
        assert_eq!(input.document.config.slots[1].catalogue, ["insert-1"]);
        let resolved = store
            .resolve_descriptor(
                &set,
                &media_set::Validator(&crate::media_set::PcMediaFormats),
            )
            .unwrap();
        let expected = store
            .resolved_file_path(&resolved, &disks[1].digest)
            .unwrap();
        let b = &prepared.session.machine.drives[1]
            .media
            .as_ref()
            .unwrap()
            .entries[0];
        assert!(matches!(&b.image, assembly::ImageSource::File(path) if path == &expected));
        assert_eq!(
            b.expected_sha256,
            Some(hash_array(&disks[1].digest).unwrap())
        );
        assert_eq!(std::fs::read(expected).unwrap(), vec![0x42; 368_640]);
        assert!(!input
            .memory
            .contains_key(std::path::Path::new("assets/insert-1.img")));
    }

    #[test]
    fn launch_pruning_removes_unused_backing_bindings() {
        let Some((_temp, store)) = start_fixture(|_| {}) else {
            return;
        };
        compress_image(&store, "test:launch", "test:compressed").unwrap();
        let owner = store.workspace("pruning").unwrap();
        let session = crate::workspace::prepare_start(&owner, "test:compressed", false).unwrap();
        let mut input = session.launch.unwrap();
        assert!(input
            .sources
            .contains_key(std::path::Path::new("assets/a.img")));
        assert!(input
            .memory
            .contains_key(std::path::Path::new("assets/a.img")));
        input.document.config.slots[0].catalogue.clear();
        input.document.config.slots[0].inserted = None;
        input.document.config.nvram = None;
        input.prune();
        assert!(input.sources.is_empty());
        assert!(input.memory.is_empty());
        assert!(input.records.is_empty());
        assert_eq!(input.small.len(), 1);
        let bios = input.document.config.firmware[0].blob.sha256();
        assert!(input.small.contains_key(&bios));
    }

    #[test]
    fn a_compressed_floppys_source_follows_the_edited_launch_policy() {
        let Some((_temp, store)) = start_fixture(|_| {}) else {
            return;
        };
        compress_image(&store, "test:launch", "test:compressed").unwrap();
        let owner = store.workspace("policy").unwrap();
        let mut session =
            crate::workspace::prepare_start(&owner, "test:compressed", false).unwrap();
        let original = session.machine.clone();
        session
            .edit_launch(|document| {
                document.config.media[0].policy = Policy::Readonly;
                document.records.retain(|r| r.medium != "a");
            })
            .unwrap();
        assert!(
            matches!(&session.machine.drives[0].image, assembly::ImageSource::Inline(bytes) if bytes.len() == 368_640 && bytes.iter().all(|b| *b == 0xf6))
        );
        assert_eq!(
            session.machine.drives[0].write,
            assembly::WriteMode::ReadOnly
        );
        session
            .edit_launch(|document| {
                document.config.media[0].policy = Policy::Journal;
                document.records.push(launch::RecordRow {
                    medium: "a".into(),
                    name: "a.upfj".into(),
                    keep_generations: assembly::WriteMode::DEFAULT_KEEP_GENERATIONS,
                });
            })
            .unwrap();
        assert_eq!(session.machine, original);
    }
}
