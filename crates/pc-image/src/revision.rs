// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings on an image: a configuration-only revision of a stored machine;
//! machine-images §§2, 4 and 7.
//!
//! A revision replaces the hardware tables of the source configuration and
//! binds every disk, catalogue medium, metadata and artwork blob of the source
//! again, by digest, and every firmware ROM the tables still name. A ROM the
//! tables name instead binds the blob of a firmware package of the local
//! store, by digest (with the source archive that travels with an open
//! copyleft ROM). Only the configuration is new, plus the stored CMOS image
//! when a setting that SETUP records there changes or a system BIOS of
//! another CMOS layout replaces it, plus those firmware blobs; the manifest's
//! `unemu.firmware.sources` names the packages the closure's ROMs come from.
//! The source image and its tags never change.
//!
//! CMOS follows what the AT SETUP program writes. IBM Personal Computer AT
//! Technical Reference, System Board, Real-Time Clock/CMOS RAM Information:
//! Diskette Drive Type Byte (Hex 10), Fixed Disk Type Byte (Hex 12), Equipment
//! Byte (Hex 14), Low and High Base Memory Bytes (Hex 15–16), Low and High
//! Expansion Memory Bytes (Hex 17–18), Drive C and D Extended Bytes (Hex 19–1A),
//! Checksum (Hex 2E–2F) and the expansion memory size POST records (Hex 30–31).
//! Only the fields whose value derived from the machine changes are rewritten,
//! then the checksum over 10h–2Dh; the clock, the status registers and every
//! other option keep their stored values.

use crate::*;
use image_store::{media_type as mt, BlobSource, ConfigValidator, Manifest, Store, MAX_SMALL_BLOB};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use toml::{Table, Value};

/// A stored machine image opened for Settings: its configuration and CMOS.
pub struct Source {
    descriptor: Descriptor,
    pub(crate) manifest: Manifest,
    pub(crate) config: Config,
    pub(crate) cmos: Option<[u8; 64]>,
}

impl Source {
    /// Read the manifest, configuration and stored CMOS of an image. Disk and
    /// firmware payloads are not read; publication verifies them.
    pub fn open(store: &Store, descriptor: &Descriptor) -> Result<Self> {
        let manifest = Manifest::decode_with_types(
            &store.read_verified(descriptor, MAX_SMALL_BLOB)?,
            PAYLOAD_TYPES,
        )?;
        if manifest.artifact_type() != mt::MACHINE || !crate::is_pc(manifest.platform()?) {
            return Err(invalid("not a PC machine image"));
        }
        let text = String::from_utf8(store.read_verified(&manifest.config, MAX_SMALL_BLOB)?)
            .map_err(|e| invalid(e.to_string()))?;
        let config: Config = decode(
            text.parse::<Table>()
                .map_err(|e: toml::de::Error| invalid(e.to_string()))?,
        )?;
        let cmos = match &config.nvram {
            Some(nvram) => {
                let d = crate::descriptor(&manifest.layers, &nvram.head.store()?, CMOS)?;
                Some(
                    store
                        .read_verified(&d, 64)?
                        .try_into()
                        .map_err(|_| invalid("CMOS must contain exactly 64 bytes"))?,
                )
            }
            None => None,
        };
        Ok(Self {
            descriptor: descriptor.clone(),
            manifest,
            config,
            cmos,
        })
    }
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    /// The flattened hardware tables of the source configuration.
    pub fn machine(&self) -> &Table {
        &self.config.machine
    }
    pub fn annotations(&self) -> &BTreeMap<String, String> {
        &self.manifest.annotations
    }
    /// Whether the image stores a CMOS image; without one Start derives CMOS
    /// from the configuration.
    pub fn stores_cmos(&self) -> bool {
        self.cmos.is_some()
    }
    /// Hardware tables as a machine document the capabilities API describes.
    /// Floppy drives get an empty media list, as in a new image, so drive-type
    /// choices never depend on the media in a catalogue.
    pub fn describable(machine: &Table) -> Table {
        let mut document = machine.clone();
        for card in document
            .get_mut("card")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if card.get("kind").and_then(Value::as_str) != Some("fdc-765") {
                continue;
            }
            for drive in card
                .get_mut("drives")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(drive) = drive.as_table_mut() {
                    drive.insert("media".into(), Value::Array(Vec::new()));
                    drive.insert("empty".into(), true.into());
                }
            }
        }
        document
    }
}

/// One CMOS byte a revision changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CmosChange {
    pub offset: usize,
    pub old: u8,
    pub new: u8,
    /// What the byte records, as SETUP describes it.
    pub field: &'static str,
}

/// One line of the flattened configuration diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    /// The table the following changes belong to, such as `[memory]`.
    Section(String),
    Removed(String),
    Added(String),
}

/// A checked revision, ready to publish under a tag. Nothing is written yet.
#[derive(Clone, Debug)]
pub struct Plan {
    source: Descriptor,
    config: Vec<u8>,
    cmos: Option<Vec<u8>>,
    manifest: Manifest,
    /// The flattened hardware tables, before and after.
    pub diff: Vec<DiffLine>,
    /// CMOS bytes the revision rewrites, or, without a stored CMOS image, the
    /// bytes Start will derive differently.
    pub cmos_changes: Vec<CmosChange>,
    /// Whether the source stores a CMOS image (otherwise Start derives it).
    pub cmos_stored: bool,
    /// The revision's system BIOS keeps CMOS in another layout, so the
    /// stored CMOS image is replaced by the bytes derived from the revision's
    /// configuration instead of having its SETUP fields recomputed.
    pub cmos_reset: bool,
    /// Why a stored CMOS image is kept byte for byte, when SETUP values cannot
    /// be derived for this machine.
    pub cmos_note: Option<String>,
    /// Blobs bound again by digest, unchanged.
    pub shared: Vec<Descriptor>,
    /// Blobs the revision adds besides its configuration.
    pub added: Vec<Descriptor>,
    /// Machine digests (configuration digests) of the source and the revision.
    pub machine: (Digest, Digest),
}

impl Plan {
    /// The manifest Publish would write.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn source(&self) -> &Descriptor {
        &self.source
    }
}

/// CMOS fields SETUP derives from the machine: (description, byte and mask).
const SETUP_FIELDS: &[(&str, &[(usize, u8)])] = &[
    ("diskette drive A: type", &[(0x10, 0xF0)]),
    ("diskette drive B: type", &[(0x10, 0x0F)]),
    ("fixed disk C: type", &[(0x12, 0xF0), (0x19, 0xFF)]),
    ("fixed disk D: type", &[(0x12, 0x0F), (0x1A, 0xFF)]),
    ("equipment byte: diskette drives", &[(0x14, 0xC1)]),
    ("equipment byte: math coprocessor", &[(0x14, 0x02)]),
    ("equipment byte: primary display", &[(0x14, 0x30)]),
    ("base memory", &[(0x15, 0xFF), (0x16, 0xFF)]),
    ("expansion memory", &[(0x17, 0xFF), (0x18, 0xFF)]),
    (
        "expansion memory found by POST",
        &[(0x30, 0xFF), (0x31, 0xFF)],
    ),
];
const CHECKSUM: &str = "checksum of 10h–2Dh";

fn field_of(offset: usize) -> &'static str {
    SETUP_FIELDS
        .iter()
        .find(|(_, bytes)| bytes.iter().any(|&(at, _)| at == offset))
        .map_or(CHECKSUM, |(name, _)| name)
}

/// The values SETUP would store for a machine. POST records the expansion
/// memory it finds at 30h–31h; it is the size at 17h–18h.
fn setup_values(machine: &assembly::MachineConfig) -> Result<[u8; 64]> {
    let mut bytes = assembly::derive_cmos_config(machine).map_err(|e| invalid(e.to_string()))?;
    bytes[0x30] = bytes[0x17];
    bytes[0x31] = bytes[0x18];
    Ok(bytes)
}

/// Rewrite only the SETUP fields whose derived value differs between the old
/// and the new machine, then the checksum. Returns the new image and the
/// changed bytes.
pub fn recompute_cmos(
    stored: &[u8; 64],
    old: &assembly::MachineConfig,
    new: &assembly::MachineConfig,
) -> Result<([u8; 64], Vec<CmosChange>)> {
    let (before, after) = (setup_values(old)?, setup_values(new)?);
    let mut bytes = *stored;
    for (_, field) in SETUP_FIELDS {
        if field
            .iter()
            .any(|&(at, mask)| before[at] & mask != after[at] & mask)
        {
            for &(at, mask) in *field {
                bytes[at] = bytes[at] & !mask | after[at] & mask;
            }
        }
    }
    if bytes[0x10..=0x2D] != stored[0x10..=0x2D] {
        let sum = bytes[0x10..=0x2D]
            .iter()
            .fold(0u16, |sum, &b| sum.wrapping_add(u16::from(b)));
        bytes[0x2E..=0x2F].copy_from_slice(&sum.to_be_bytes());
    }
    let changes = (0..64)
        .filter(|&i| bytes[i] != stored[i])
        .map(|offset| CmosChange {
            offset,
            old: stored[offset],
            new: bytes[offset],
            field: field_of(offset),
        })
        .collect();
    Ok((bytes, changes))
}

/// Serves the revision's new CMOS image before the store holds it.
struct Pending<'a> {
    blobs: &'a dyn BlobSource,
    extra: &'a BTreeMap<Digest, Vec<u8>>,
}
impl BlobSource for Pending<'_> {
    fn read_verified(&self, descriptor: &Descriptor, limit: u64) -> image_store::Result<Vec<u8>> {
        match self.extra.get(&descriptor.digest) {
            Some(bytes) if bytes.len() as u64 == descriptor.size && descriptor.size <= limit => {
                Ok(bytes.clone())
            }
            Some(_) => Err(image_store::Error::Invalid(
                "pending blob size mismatch".into(),
            )),
            None => self.blobs.read_verified(descriptor, limit),
        }
    }
}

fn config_table(config: &Config) -> Result<Table> {
    toml::Value::try_from(config)
        .map_err(|e| invalid(e.to_string()))?
        .as_table()
        .cloned()
        .ok_or_else(|| invalid("config is not a table"))
}

fn drive_letter(kind: &str, unit: u8) -> String {
    if kind == "ide" {
        format!("Fixed disk {}:", char::from(b'C' + unit))
    } else {
        format!("Drive {}:", char::from(b'A' + unit))
    }
}

/// Keep the slot of every drive position that stays, give a new diskette
/// drive an empty catalogue and drop the catalogue of a removed drive. A
/// diskette drive whose type changes cannot keep media of the old size.
fn rebind(source: &Config, config: &mut Config) -> Result<()> {
    // The kind and diskette type of every drive position of the source.
    let mut old_drives = BTreeMap::new();
    drive_tables(&mut source.machine.clone(), |controller, kind, drive| {
        old_drives.insert(
            (controller.to_owned(), unit(kind, drive)?),
            (kind.to_owned(), drive.get("type").cloned()),
        );
        Ok(())
    })?;
    let mut slots = Vec::new();
    drive_tables(&mut config.machine, |controller, kind, drive| {
        let position = (controller.to_owned(), unit(kind, drive)?);
        match source
            .slots
            .iter()
            .find(|s| s.controller == position.0 && s.unit == position.1)
        {
            Some(slot) => {
                if !slot.catalogue.is_empty()
                    && kind != "ide"
                    && old_drives.get(&position).map(|(_, kind)| kind)
                        != Some(&drive.get("type").cloned())
                {
                    return Err(invalid(format!(
                        "{} holds media of its current type in its catalogue; another drive type cannot take them",
                        drive_letter(kind, position.1)
                    )));
                }
                slots.push(slot.clone());
            }
            None if kind != "ide" => slots.push(Slot {
                controller: position.0,
                unit: position.1,
                catalogue: Vec::new(),
                catalogue_mode: true,
                inserted: None,
            }),
            None => {
                return Err(invalid(format!(
                    "{} cannot be added here: disks keep the image's contents",
                    drive_letter(kind, position.1)
                )))
            }
        }
        Ok(())
    })?;
    // Media leave an image only through its own catalogue tools, never as a
    // side effect of removing their drive.
    if let Some(slot) = source.slots.iter().find(|old| {
        !old.catalogue.is_empty()
            && !slots
                .iter()
                .any(|s| s.controller == old.controller && s.unit == old.unit)
    }) {
        let kind = old_drives
            .get(&(slot.controller.clone(), slot.unit))
            .map_or("", |(kind, _)| kind.as_str());
        return Err(invalid(format!(
            "{} holds media in its catalogue; a drive with media cannot be removed here",
            drive_letter(kind, slot.unit)
        )));
    }
    let used: BTreeSet<_> = slots.iter().flat_map(|s| s.catalogue.iter()).collect();
    config.media = source
        .media
        .iter()
        .filter(|m| used.contains(&m.id))
        .cloned()
        .collect();
    config.slots = slots;
    Ok(())
}

/// The key of a machine's settings overlay under which the firmware it binds
/// from packages of the store is recorded (each ROM's package digest and
/// layer descriptor): the ROM stays bindable by digest when its package loses
/// its tag. It is never a hardware table: lowering takes it out first, and
/// [`crate::machine::settings`] never shows it.
pub const BOUND_FIRMWARE: &str = "bound_firmware";

/// One ROM blob that hardware tables bind from a firmware package rather
/// than from their image: the package's manifest digest, the blob's layer
/// descriptor as the image would carry it (with its marks) and the source
/// archive that travels with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bound {
    pub package: Digest,
    pub blob: Descriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<Descriptor>,
}

impl Bound {
    fn names(&self, sha256: &str) -> bool {
        self.blob
            .decoded_digest()
            .is_ok_and(|digest| digest.hex() == sha256)
    }
}

/// Take the recorded package bindings out of hardware tables.
pub(crate) fn take_bound(machine: &mut Table) -> Result<Vec<Bound>> {
    match machine.remove(BOUND_FIRMWARE) {
        None => Ok(Vec::new()),
        Some(value) => value
            .try_into()
            .map_err(|e: toml::de::Error| invalid(format!("{BOUND_FIRMWARE}: {e}"))),
    }
}

/// Record package bindings in a machine's settings overlay.
pub(crate) fn record_bound(machine: &mut Table, bound: &[Bound]) -> Result<()> {
    machine.remove(BOUND_FIRMWARE);
    if !bound.is_empty() {
        machine.insert(
            BOUND_FIRMWARE.into(),
            Value::try_from(bound).map_err(|e| invalid(e.to_string()))?,
        );
    }
    Ok(())
}

/// The ROM blob descriptors a machine's settings overlay records, with their
/// marks, as its image would carry them.
pub fn bound_blobs(settings: &Table) -> Vec<Descriptor> {
    let mut tables = settings.clone();
    take_bound(&mut tables)
        .unwrap_or_default()
        .into_iter()
        .map(|b| b.blob)
        .collect()
}

/// The firmware packages of the local store, read once per check or Start.
pub(crate) struct Packages<'a> {
    pub store: &'a Store,
    list: std::cell::OnceCell<Vec<crate::create::Package>>,
}

impl<'a> Packages<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            list: std::cell::OnceCell::new(),
        }
    }
    pub fn list(&self) -> Result<&[crate::create::Package]> {
        if let Some(list) = self.list.get() {
            return Ok(list);
        }
        let list = crate::create::packages(self.store)?;
        Ok(self.list.get_or_init(|| list))
    }
}

/// A revision lowered with the real ROM bytes: its configuration, the
/// lowered machines of the revision and of the source, and the firmware it
/// binds from packages instead of from the source.
pub(crate) struct Lowered {
    pub config: Config,
    pub machine: assembly::MachineConfig,
    pub source: assembly::MachineConfig,
    pub bound: Vec<Bound>,
}

impl Lowered {
    /// Whether the system BIOS of the revision keeps its CMOS in another
    /// layout than the source's, so the source's CMOS bytes cannot be kept.
    pub fn cmos_layout_changes(&self) -> bool {
        cmos_layout_changes(&self.source, &self.machine)
    }
    /// The layers of the package firmware: each ROM blob and the source
    /// archive that travels with it.
    pub fn firmware(&self) -> Vec<Descriptor> {
        firmware_layers(&self.bound)
    }
}

fn firmware_layers(bound: &[Bound]) -> Vec<Descriptor> {
    let mut layers: Vec<Descriptor> = Vec::new();
    for d in bound
        .iter()
        .flat_map(|b| std::iter::once(&b.blob).chain(&b.archive))
    {
        if !layers.iter().any(|known| known.digest == d.digest) {
            layers.push(d.clone());
        }
    }
    layers
}

/// The system BIOS bytes of a lowered machine.
fn system_bios(machine: &assembly::MachineConfig) -> Option<&[u8]> {
    let bios = machine.images.iter().find(|image| {
        image.kind == assembly::ImageKind::Rom
            && image
                .source
                .known_len()
                .and_then(|n| image.base.checked_add(n))
                == Some(0x100000)
    })?;
    match &bios.source {
        assembly::ImageSource::Inline(bytes) => Some(bytes),
        _ => None,
    }
}

/// The CMOS layout of a lowered machine's system BIOS, as the catalogue
/// records it; `None` for firmware the catalogue does not list.
pub(crate) fn cmos_layout(machine: &assembly::MachineConfig) -> Option<assembly::CmosLayout> {
    let bytes = system_bios(machine)?;
    machine
        .board
        .firmware_catalogue()
        .iter()
        .find(|entry| entry.kind == assembly::FirmwareKind::SystemBios && entry.identifies(bytes))?
        .cmos_layout
}

/// Whether the CMOS bytes one system BIOS wrote cannot be kept for another:
/// another catalogued layout, or other bytes where either BIOS is unlisted,
/// whose layout nobody knows.
pub(crate) fn cmos_layout_changes(
    old: &assembly::MachineConfig,
    new: &assembly::MachineConfig,
) -> bool {
    if system_bios(old) == system_bios(new) {
        return false;
    }
    match (cmos_layout(old), cmos_layout(new)) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

/// Bind every ROM table of `machine` to a blob, in table order: the source's
/// binding of the same bytes; else a binding `recorded` with the machine's
/// settings, which holds even when its package lost its tag; else the ROM of
/// a firmware package in the local store with those bytes ("in the store").
/// Returns the bindings and the package ROMs they bind, each blob marked as
/// an image marks it, with the source archive that travels with an open
/// copyleft ROM.
fn bind_firmware(
    packages: &Packages<'_>,
    source: &Config,
    recorded: &[Bound],
    machine: &mut Table,
) -> Result<(Vec<Firmware>, Vec<Bound>)> {
    let mut bindings = Vec::new();
    let mut bound: Vec<Bound> = Vec::new();
    firmware_images(machine, |field, rom| {
        let text = |key: &str| {
            rom.get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| invalid(format!("{field} needs {key}")))
        };
        let (name, sha256) = (text("image")?, text("sha256")?);
        if let Some(binding) = source.firmware.iter().find(|b| b.blob.hex() == sha256) {
            bindings.push(Firmware {
                key: field.into(),
                source: name,
                blob: binding.blob,
                source_archive: binding.source_archive,
            });
            return Ok(());
        }
        let found = match recorded.iter().find(|b| b.names(&sha256)) {
            Some(known) => known.clone(),
            None => {
                let found = packages.list()?.iter().find_map(|package| {
                    package.config.roms.iter().find_map(|rom| {
                        crate::create::rom_blobs(rom)
                            .into_iter()
                            .find(|digest| digest.hex() == sha256)
                            .map(|digest| (package, rom, digest))
                    })
                });
                let Some((package, rom, digest)) = found else {
                    return Err(invalid(format!(
                        "no firmware package in the local store holds the ROM with SHA-256 {sha256}; import its ROM folder or pull its package first"
                    )));
                };
                Bound {
                    package: package.descriptor.digest.clone(),
                    blob: crate::create::rom_blob(package, rom, &digest)?,
                    archive: package.source_archive(rom).cloned(),
                }
            }
        };
        bindings.push(Firmware {
            key: field.into(),
            source: name,
            blob: found.blob.decoded_digest()?.to_config()?,
            source_archive: found
                .archive
                .as_ref()
                .map(|d| d.decoded_digest()?.to_config())
                .transpose()?,
        });
        if !bound.iter().any(|b| b.blob.digest == found.blob.digest) {
            bound.push(found);
        }
        Ok(())
    })?;
    Ok((bindings, bound))
}

/// The configuration that `machine` as hardware tables makes of `source`:
/// firmware and drive slots bound again, lowering defaults frozen into the
/// tables, with the lowered machines of the revision and of the source.
/// Firmware ROM bytes are read to lower the machines; disks are not. A ROM
/// the source does not bind comes from the bindings the tables record
/// ([`BOUND_FIRMWARE`]) or from a firmware package of the store.
pub(crate) fn lower_revision(
    packages: &Packages<'_>,
    source: &Config,
    layers: &[Descriptor],
    mut machine: Table,
) -> Result<Lowered> {
    let store = packages.store;
    let recorded = take_bound(&mut machine)?;
    let (firmware, bound) = bind_firmware(packages, source, &recorded, &mut machine)?;
    let mut config = Config {
        schema: 1,
        platform: crate::PLATFORM.into(),
        machine,
        firmware,
        media: Vec::new(),
        slots: Vec::new(),
        nvram: source.nvram.clone(),
    };
    rebind(source, &mut config)?;
    let descriptors: Vec<Descriptor> = layers
        .iter()
        .cloned()
        .chain(firmware_layers(&bound))
        .collect();
    let lowered = crate::provider::prepare(decode(config_table(&config)?)?, &descriptors, store)?;
    let old = crate::provider::prepare(decode(config_table(source)?)?, layers, store)?;
    let reset = cmos_layout_changes(&old.machine, &lowered.machine);
    // What only the ROM bytes decide, such as a BIOS that types its fixed disks
    // from CMOS, refuses the machine here, unless a stored CMOS image of the
    // same layout replaces the derived one, as assembly does.
    if source.nvram.is_none() || reset {
        assembly::derive_cmos_config(&lowered.machine).map_err(|e| invalid(e.to_string()))?;
    }
    crate::defaults::materialize(&mut config.machine, &lowered.machine)?;
    Ok(Lowered {
        config,
        machine: lowered.machine,
        source: old.machine,
        bound,
    })
}

/// A machine's settings overlay on an image configuration: the configuration
/// it makes, the recomputed CMOS and the package firmware it binds. A stored
/// CMOS image without a journal is copied afresh at every Start, so its SETUP
/// fields are recomputed here for the overlay (`cmos`, when they change) and
/// the launch binds those bytes; a system BIOS of another CMOS
/// layout starts from the bytes derived from the configuration instead. A
/// journaled CMOS received the recomputed or derived bytes in its journal
/// when the settings changed, so it needs nothing here.
pub(crate) struct Overlay {
    pub config: Config,
    pub cmos: Option<[u8; 64]>,
    pub bound: Vec<Bound>,
}

impl Overlay {
    pub fn firmware(&self) -> Vec<Descriptor> {
        firmware_layers(&self.bound)
    }
}

pub(crate) fn overlay(
    packages: &Packages<'_>,
    source: &Config,
    layers: &[Descriptor],
    machine: &Table,
) -> Result<Overlay> {
    let lowered = lower_revision(packages, source, layers, machine.clone())?;
    let reset = lowered.cmos_layout_changes();
    let Lowered {
        config,
        machine: mut lowered,
        source: old,
        bound,
    } = lowered;
    let mut cmos = None;
    if source
        .nvram
        .as_ref()
        .is_some_and(|n| matches!(n.mode, NvramMode::Image))
    {
        let stored = lowered
            .devices
            .iter()
            .find_map(|device| match &device.kind {
                assembly::DeviceKind::Rtc(rtc) => match rtc.nvram {
                    assembly::NvramSource::Image(bytes) => Some(bytes),
                    _ => None,
                },
                _ => None,
            });
        if let Some(stored) = stored {
            let replaced = if reset {
                Some(reset_cmos(&stored, &lowered)?.0)
            } else {
                // A machine whose SETUP values cannot be derived keeps the
                // stored bytes, as Settings on an image does.
                recompute_cmos(&stored, &old, &lowered)
                    .ok()
                    .filter(|(_, changes)| !changes.is_empty())
                    .map(|(bytes, _)| bytes)
            };
            if let Some(bytes) = replaced {
                for device in &mut lowered.devices {
                    if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                        rtc.nvram = assembly::NvramSource::Image(bytes);
                    }
                }
                cmos = Some(bytes);
            }
        }
    }
    Ok(Overlay {
        config,
        cmos,
        bound,
    })
}

/// The manifest annotation naming the firmware packages an image's ROMs
/// come from, as a JSON array of manifest digests.
pub(crate) const FIRMWARE_SOURCES: &str = "unemu.firmware.sources";

/// `unemu.firmware.sources` for a new closure (`layers`) made from a source
/// image: the source's packages that still provide one of the closure's ROM
/// blobs, then the packages of ROMs it newly binds (`bound`), in first-use
/// order. A package of the source that no tag names any more cannot be read
/// and stays as the source recorded it. With the same ROMs as the source and
/// nothing bound, the source's own value stands. `None` when neither the
/// source nor the closure names a package.
pub(crate) fn firmware_sources(
    packages: &Packages<'_>,
    source: &Manifest,
    layers: &[Descriptor],
    bound: &[Bound],
) -> Result<Option<String>> {
    let roms = |layers: &[Descriptor]| -> BTreeSet<Digest> {
        layers
            .iter()
            .filter(|d| d.decoded_media_type() == mt::FIRMWARE)
            .filter_map(|d| d.decoded_digest().ok())
            .collect()
    };
    let used = roms(layers);
    let listed = source.annotations.get(FIRMWARE_SOURCES);
    if bound.is_empty() && used == roms(&source.layers) {
        return Ok(listed.cloned());
    }
    let mut out: Vec<Digest> = Vec::new();
    for text in listed
        .map(|l| l.trim().trim_start_matches('[').trim_end_matches(']'))
        .unwrap_or_default()
        .split(',')
        .map(|d| d.trim().trim_matches('"'))
        .filter(|d| !d.is_empty())
    {
        let digest: Digest = text.parse()?;
        let keep = match packages
            .list()?
            .iter()
            .find(|p| p.descriptor.digest == digest)
        {
            Some(package) => package.config.roms.iter().any(|rom| {
                crate::create::rom_blobs(rom)
                    .iter()
                    .any(|blob| used.contains(blob))
            }),
            None => true,
        };
        if keep && !out.contains(&digest) {
            out.push(digest);
        }
    }
    for b in bound {
        if b.blob.decoded_digest().is_ok_and(|d| used.contains(&d)) && !out.contains(&b.package) {
            out.push(b.package.clone());
        }
    }
    if out.is_empty() && listed.is_none() {
        return Ok(None);
    }
    Ok(Some(format!(
        "[{}]",
        out.iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(",")
    )))
}

/// The CMOS bytes that differ between two images, described as SETUP fields.
fn differences(old: &[u8; 64], new: &[u8; 64]) -> Vec<CmosChange> {
    (0..64)
        .filter(|&i| old[i] != new[i])
        .map(|offset| CmosChange {
            offset,
            old: old[offset],
            new: new[offset],
            field: field_of(offset),
        })
        .collect()
}

/// Reset a CMOS image to the bytes derived from `machine`, as a new machine
/// starts, when its system BIOS keeps CMOS in another layout. The clock and
/// status registers (00h–0Dh; Motorola MC146818 datasheet, Address Map) and
/// the shutdown status byte (0Fh; IBM Personal Computer AT Technical
/// Reference, CMOS RAM Address Map) keep their stored values: they are the
/// RTC's and POST's own state, not configuration, and a CMOS journal never
/// rewrites them. Returns the new image and the bytes that change.
pub(crate) fn reset_cmos(
    stored: &[u8; 64],
    machine: &assembly::MachineConfig,
) -> Result<([u8; 64], Vec<CmosChange>)> {
    let mut bytes = assembly::derive_cmos_config(machine).map_err(|e| invalid(e.to_string()))?;
    bytes[..0x0E].copy_from_slice(&stored[..0x0E]);
    bytes[0x0F] = stored[0x0F];
    Ok((bytes, differences(stored, &bytes)))
}

pub fn plan(store: &Store, source: &Source, machine: Table) -> Result<Plan> {
    let packages = Packages::new(store);
    let mut descriptors = source.manifest.layers.clone();
    let revised = lower_revision(&packages, &source.config, &descriptors, machine)?;
    let cmos_reset = revised.cmos_layout_changes();
    let firmware = revised.firmware();
    let Lowered {
        mut config,
        machine: lowered,
        source: old,
        bound,
    } = revised;
    for d in firmware {
        if !descriptors.iter().any(|known| known.digest == d.digest) {
            descriptors.push(d);
        }
    }
    let mut pending = BTreeMap::new();
    let mut cmos_note = None;
    let cmos_changes = match &source.cmos {
        Some(stored) => {
            let recomputed = if cmos_reset {
                // Another layout cannot read the stored bytes: the revision
                // stores the bytes derived from its configuration instead.
                Ok(reset_cmos(stored, &lowered)?)
            } else {
                recompute_cmos(stored, &old, &lowered)
            };
            let (bytes, changes) = match recomputed {
                Ok(recomputed) => recomputed,
                Err(error) => {
                    cmos_note = Some(format!(
                        "The stored CMOS image is kept byte for byte: SETUP values cannot be derived for this machine ({error})."
                    ));
                    (*stored, Vec::new())
                }
            };
            if !changes.is_empty() {
                let mut d = Descriptor::new(CMOS, Digest::of(&bytes), 64);
                // The new image derives from the user's own stored CMOS image.
                d.annotations
                    .insert("unemu.redistribution".into(), "user-supplied".into());
                if let Some(nvram) = &mut config.nvram {
                    nvram.head = d.digest.to_config()?;
                }
                if !descriptors.iter().any(|known| known.digest == d.digest) {
                    descriptors.push(d.clone());
                }
                pending.insert(d.digest.clone(), bytes.to_vec());
            }
            changes
        }
        None => {
            let before = assembly::derive_cmos_config(&old).map_err(|e| invalid(e.to_string()))?;
            let after =
                assembly::derive_cmos_config(&lowered).map_err(|e| invalid(e.to_string()))?;
            differences(&before, &after)
        }
    };
    let overlay = Pending {
        blobs: store,
        extra: &pending,
    };
    let references = PcProvider.validate(&config_table(&config)?, &descriptors, &overlay)?;
    let mut seen = BTreeSet::new();
    let mut layers = Vec::new();
    for digest in references {
        if seen.insert(digest.clone()) {
            layers.push(
                descriptors
                    .iter()
                    .find(|d| d.names(&digest))
                    .ok_or_else(|| invalid("missing closure object"))?
                    .clone(),
            );
        }
    }
    for d in source
        .manifest
        .layers
        .iter()
        .filter(|d| matches!(d.media_type.as_str(), mt::METADATA | mt::PNG | mt::JPEG))
    {
        layers.push(if d.media_type == mt::METADATA {
            generated(image_store::platform::current_metadata(
                store,
                d,
                &source.manifest.layers,
            )?)
        } else {
            d.clone()
        });
    }
    let bytes = toml::to_string(&config)
        .map_err(|e| invalid(e.to_string()))?
        .into_bytes();
    if bytes.len() as u64 > MAX_SMALL_BLOB {
        return Err(invalid("configuration exceeds 1 MiB"));
    }
    if Digest::of(&bytes) == source.manifest.config.digest {
        return Err(invalid("the settings are unchanged"));
    }
    let configuration = generated(Descriptor::new(
        mt::CONFIG,
        Digest::of(&bytes),
        bytes.len() as u64,
    ));
    let sources = firmware_sources(&packages, &source.manifest, &layers, &bound)?;
    let mut manifest =
        Manifest::new_with_types(crate::PLATFORM, configuration, layers, PAYLOAD_TYPES)?;
    if let Some(sources) = sources {
        manifest
            .annotations
            .insert(FIRMWARE_SOURCES.into(), sources);
    }
    let inherited = crate::compat::packages_floor(
        crate::compat::source_levels(store),
        crate::compat::listed_packages(&manifest, packages.list()?),
    )?;
    crate::compat::stamp(
        &mut manifest,
        &bytes,
        inherited,
        None,
        crate::compat::source_levels(store),
    )?;
    crate::workspace::derive_annotations(
        &mut manifest,
        &source.manifest,
        &source.descriptor.digest,
        None,
    )?;
    image_store::compat::carry(
        &mut manifest,
        &source.manifest,
        crate::compat::source_levels(store),
    )?;
    let old_digests: BTreeSet<_> = source.manifest.layers.iter().map(|d| &d.digest).collect();
    let (shared, added) = manifest
        .layers
        .iter()
        .cloned()
        .partition(|d| old_digests.contains(&d.digest));
    let pretty = |table: &Table| toml::to_string_pretty(table).map_err(|e| invalid(e.to_string()));
    Ok(Plan {
        source: source.descriptor.clone(),
        diff: line_diff(&pretty(&source.config.machine)?, &pretty(&config.machine)?),
        cmos_changes,
        cmos_stored: source.cmos.is_some(),
        cmos_reset: cmos_reset && source.cmos.is_some(),
        cmos_note,
        shared,
        added,
        machine: (
            source.manifest.config.digest.clone(),
            manifest.config.digest.clone(),
        ),
        config: bytes,
        cmos: pending.into_values().next(),
        manifest,
    })
}

/// Store the revision and point `tag` at it. A version tag never moves; any
/// other existing tag moves only from `expected`, the digest the user saw.
/// The source image and its tags stay as they are. The tag update verifies
/// the whole closure, disks included, like every publication.
pub fn publish(
    store: &Store,
    plan: &Plan,
    tag: &str,
    expected: Option<&Digest>,
) -> Result<Descriptor> {
    let digest = Digest::of(&plan.manifest.encode_with_types(PAYLOAD_TYPES)?);
    store.check_tag_target(tag, expected, Some(&digest))?;
    store.import(plan.config.as_slice(), Some(&plan.manifest.config.digest))?;
    if let Some(bytes) = &plan.cmos {
        store.import(bytes.as_slice(), None)?;
    }
    let descriptor = store.import_manifest_with_types(&plan.manifest, PAYLOAD_TYPES)?;
    store.update_tag(tag, &descriptor, expected, &PcProvider)?;
    // Journal retention per medium is local provenance; the revision keeps it.
    // It is written once the tag names the revision, so a lost compare-and-swap
    // leaves no record for an untagged manifest.
    if let Some(record) = ImportRecord::read(store, &plan.source.digest)? {
        let record = ImportRecord {
            manifest: descriptor.clone(),
            ..record
        };
        store.write_local_record(
            &descriptor.digest,
            toml::to_string(&record)
                .map_err(|e| invalid(e.to_string()))?
                .as_bytes(),
        )?;
    }
    Ok(descriptor)
}

/// The table each line of a pretty-printed machine document belongs to; a
/// card is named by its kind.
fn sections(lines: &[&str]) -> Vec<String> {
    let mut labels = Vec::with_capacity(lines.len());
    let mut current = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with('[') {
            current = line.trim().to_owned();
            if current == "[[card]]" {
                if let Some(kind) = lines[i + 1..]
                    .iter()
                    .take_while(|l| !l.starts_with('['))
                    .find_map(|l| l.strip_prefix("kind = "))
                {
                    current = format!("[[card]]  # {}", kind.trim_matches('"'));
                }
            }
        }
        labels.push(current.clone());
    }
    labels
}

/// A longest-common-subsequence line diff with the table of each change.
pub(crate) fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<_> = old.lines().collect();
    let b: Vec<_> = new.lines().collect();
    let (sa, sb) = (sections(&a), sections(&b));
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut diff = Vec::new();
    let mut section = None::<String>;
    let mut emit = |diff: &mut Vec<DiffLine>, label: &str, line: DiffLine| {
        if section.as_deref() != Some(label) {
            diff.push(DiffLine::Section(label.to_owned()));
            section = Some(label.to_owned());
        }
        diff.push(line);
    };
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            emit(&mut diff, &sa[i], DiffLine::Removed(a[i].to_owned()));
            i += 1;
        } else {
            emit(&mut diff, &sb[j], DiffLine::Added(b[j].to_owned()));
            j += 1;
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_diff_names_the_table_of_each_change() {
        let old = "[memory]\nconventional_kib = 640\nextended_kib = 1024\n\n[[card]]\nkind = \"sb16\"\nbase = 544\n";
        let new = "[memory]\nconventional_kib = 640\nextended_kib = 4096\n\n[[card]]\nkind = \"sb16\"\nbase = 576\n";
        assert_eq!(
            line_diff(old, new),
            [
                DiffLine::Section("[memory]".into()),
                DiffLine::Removed("extended_kib = 1024".into()),
                DiffLine::Added("extended_kib = 4096".into()),
                DiffLine::Section("[[card]]  # sb16".into()),
                DiffLine::Removed("base = 544".into()),
                DiffLine::Added("base = 576".into()),
            ]
        );
        assert!(line_diff(old, old).is_empty());
    }
}
