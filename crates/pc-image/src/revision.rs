// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings on an image: a configuration-only revision of a stored machine;
//! machine-images §§2, 4 and 7.
//!
//! A revision replaces the hardware tables of the source configuration and
//! binds every disk, firmware, catalogue medium, metadata and artwork blob of
//! the source again, by digest. Only the configuration is new, plus the stored
//! CMOS image when a setting that SETUP records there changes. The machine
//! digest (the configuration digest) therefore changes and nothing else; the
//! source image and its tags never change.
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
        if manifest.artifact_type() != mt::MACHINE || manifest.platform()? != "ibm-pc-at" {
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
                let d = crate::descriptor(&manifest.layers, &nvram.head, CMOS)?;
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

/// Check a revision with `machine` as its hardware tables. Firmware ROM
/// bytes are read to lower the machine; disks are not.
/// The configuration that `machine` as hardware tables makes of `source`:
/// drive slots rebound, lowering defaults frozen into the tables. Returns it
/// with the lowered machines of the revision and of the source. Firmware ROM
/// bytes are read to lower the machines; disks are not.
pub(crate) fn lower_revision(
    blobs: &dyn BlobSource,
    source: &Config,
    layers: &[Descriptor],
    machine: Table,
) -> Result<(Config, assembly::MachineConfig, assembly::MachineConfig)> {
    let mut config = Config {
        schema: 1,
        platform: "ibm-pc-at".into(),
        machine,
        firmware: source.firmware.clone(),
        media: Vec::new(),
        slots: Vec::new(),
        nvram: source.nvram.clone(),
    };
    rebind(source, &mut config)?;
    let lowered = crate::provider::prepare(decode(config_table(&config)?)?, layers, blobs)?;
    // What only the ROM bytes decide, such as a BIOS that types its fixed disks
    // from CMOS, refuses the machine here, unless a stored CMOS image replaces
    // the derived one, as assembly does.
    if source.nvram.is_none() {
        assembly::derive_cmos_config(&lowered.machine).map_err(|e| invalid(e.to_string()))?;
    }
    crate::defaults::materialize(&mut config.machine, &lowered.machine)?;
    let old = crate::provider::prepare(decode(config_table(source)?)?, layers, blobs)?;
    Ok((config, lowered.machine, old.machine))
}

/// A machine's settings overlay on an image configuration: the configuration
/// it makes and its lowered machine. A stored CMOS image without a journal is
/// copied afresh at every Start, so its SETUP fields are recomputed here for
/// the overlay (`cmos`, when they change) and the lowered machine starts from
/// those bytes. A journaled CMOS received the recomputed bytes in its journal
/// when the settings changed, so it needs nothing here.
pub(crate) struct Overlay {
    pub config: Config,
    pub machine: assembly::MachineConfig,
    pub cmos: Option<[u8; 64]>,
}

pub(crate) fn overlay(
    blobs: &dyn BlobSource,
    source: &Config,
    layers: &[Descriptor],
    machine: &Table,
) -> Result<Overlay> {
    let (config, mut lowered, old) = lower_revision(blobs, source, layers, machine.clone())?;
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
            // A machine whose SETUP values cannot be derived keeps the stored
            // bytes, as Settings on an image does.
            if let Ok((bytes, changes)) = recompute_cmos(&stored, &old, &lowered) {
                if !changes.is_empty() {
                    for device in &mut lowered.devices {
                        if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                            rtc.nvram = assembly::NvramSource::Image(bytes);
                        }
                    }
                    cmos = Some(bytes);
                }
            }
        }
    }
    Ok(Overlay {
        config,
        machine: lowered,
        cmos,
    })
}

pub fn plan(store: &Store, source: &Source, machine: Table) -> Result<Plan> {
    let mut descriptors = source.manifest.layers.clone();
    let (mut config, lowered, old) = lower_revision(store, &source.config, &descriptors, machine)?;
    let mut pending = BTreeMap::new();
    let mut cmos_note = None;
    let cmos_changes = match &source.cmos {
        Some(stored) => {
            let (bytes, changes) = match recompute_cmos(stored, &old, &lowered) {
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
                    nvram.head = d.digest.clone();
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
            (0..64)
                .filter(|&i| before[i] != after[i])
                .map(|offset| CmosChange {
                    offset,
                    old: before[offset],
                    new: after[offset],
                    field: field_of(offset),
                })
                .collect()
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
    layers.extend(
        source
            .manifest
            .layers
            .iter()
            .filter(|d| matches!(d.media_type.as_str(), mt::METADATA | mt::PNG | mt::JPEG))
            .map(|d| {
                if d.media_type == mt::METADATA {
                    generated(d.clone())
                } else {
                    d.clone()
                }
            }),
    );
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
    let mut manifest = Manifest::new_with_types("ibm-pc-at", configuration, layers, PAYLOAD_TYPES)?;
    crate::workspace::derive_annotations(
        &mut manifest,
        &source.manifest,
        &source.descriptor.digest,
        None,
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
