// SPDX-License-Identifier: AGPL-3.0-or-later
//! The IBM PC compatible rows of the compatibility table: for each feature
//! this provider reads, the first release that reads it; machine-images §5.6.
//!
//! The shared rows (envelope media types, installation-set and
//! firmware-package keys and their vocabularies) live in
//! [`image_store::compat`]; these are the provider's: its payload types, the
//! keys of the machine config (the bundle tables and the flattened schema-1
//! machine of `machine-files.md` § Schema), the installation-set formats, the
//! firmware-package fit tables, the vocabulary values (board, CPU, FPU, card
//! kind, drive type, controller, serial mouse, channel, write mode, chip lane,
//! firmware service and role, fit board and class, media format) and the
//! firmware catalogue entries. Vocabulary enums are matched exhaustively, so a
//! new value does not compile without its row. Every row here is release
//! 0.0.1, the baseline: releases 0.0.1 and 0.0.2 read the same images. The
//! platform id itself is a shared row ([`image_store::compat::PLATFORMS`]):
//! `ibm-pc`, which every writer writes, is first read by release 0.0.3, so
//! every PC artifact written now declares 0.0.3.
//! Accepted ranges (memory sizes, disk geometry, ROM placement) have not
//! grown since 0.0.1; a release that widens one adds a row that rates a value
//! outside the old range.

use crate::{NvramMode, Policy, CMOS, DISK_ZERO};
use image_store::{
    compat::{
        self, AppVersion, Requirement, Row, BASELINE, EXTRA_KEYS, FIRMWARE_KEYS, MEDIA_SET_KEYS,
    },
    firmware, media_set, media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest,
    Manifest, PayloadType, ResolvedImage, Store,
};
use machine_file::schema::v1 as schema;

/// Payload types only this provider declares; the compressed firmware,
/// floppy and source-archive types are shared rows.
pub const PAYLOADS: &[Row] = &[
    (CMOS, BASELINE),
    (DISK_ZERO, BASELINE),
    ("application/vnd.unemu.disk.base.v1+zstd", BASELINE),
    ("application/vnd.unemu.disk.sectors.v1+zstd", BASELINE),
];

/// Keys of the PC machine config: the bundle tables and, under `machine`,
/// the flattened schema-1 machine. A bundle never holds the descriptive or
/// inheritance keys, the drive media keys or NVRAM image policy keys of a
/// machine file, so those have no row.
pub const MACHINE_KEYS: &[Row] = &[
    ("schema", BASELINE),
    ("platform", BASELINE),
    ("machine", BASELINE),
    ("firmware", BASELINE),
    ("firmware.key", BASELINE),
    ("firmware.source", BASELINE),
    ("firmware.blob", BASELINE),
    ("firmware.source_archive", BASELINE),
    ("media", BASELINE),
    ("media.id", BASELINE),
    ("media.source", BASELINE),
    ("media.head", BASELINE),
    ("media.name", BASELINE),
    ("media.policy", BASELINE),
    ("media.geometry", BASELINE),
    ("media.geometry.cylinders", BASELINE),
    ("media.geometry.heads", BASELINE),
    ("media.geometry.sectors", BASELINE),
    ("media.geometry.sector_bytes", BASELINE),
    ("slots", BASELINE),
    ("slots.controller", BASELINE),
    ("slots.unit", BASELINE),
    ("slots.catalogue", BASELINE),
    ("slots.catalogue_mode", BASELINE),
    ("slots.inserted", BASELINE),
    ("nvram", BASELINE),
    ("nvram.mode", BASELINE),
    ("nvram.source", BASELINE),
    ("nvram.head", BASELINE),
    ("machine.schema", BASELINE),
    ("machine.board", BASELINE),
    ("machine.board.model", BASELINE),
    ("machine.board.display_switch", BASELINE),
    ("machine.board.post_port", BASELINE),
    ("machine.board.rtc", BASELINE),
    ("machine.board.rtc.start", BASELINE),
    ("machine.board.rtc.nvram", BASELINE),
    ("machine.board.rtc.nvram.image", BASELINE),
    ("machine.board.mouse", BASELINE),
    ("machine.cpu", BASELINE),
    ("machine.cpu.model", BASELINE),
    ("machine.cpu.rate", BASELINE),
    ("machine.cpu.fpu", BASELINE),
    ("machine.memory", BASELINE),
    ("machine.memory.conventional_kib", BASELINE),
    ("machine.memory.extended_kib", BASELINE),
    ("machine.firmware", BASELINE),
    ("machine.firmware.bios", BASELINE),
    ("machine.firmware.bios.offset", BASELINE),
    ("machine.firmware.bios.parts", BASELINE),
    ("machine.firmware.bios.parts.image", BASELINE),
    ("machine.firmware.bios.parts.lane", BASELINE),
    ("machine.firmware.bios.parts.offset", BASELINE),
    ("machine.firmware.bios.parts.sha256", BASELINE),
    ("machine.firmware.bios.image", BASELINE),
    ("machine.firmware.bios.sha256", BASELINE),
    ("machine.firmware.bios.base", BASELINE),
    ("machine.firmware.bios.provides", BASELINE),
    ("machine.firmware.option_roms", BASELINE),
    ("machine.firmware.option_roms.offset", BASELINE),
    ("machine.firmware.option_roms.parts", BASELINE),
    ("machine.firmware.option_roms.parts.image", BASELINE),
    ("machine.firmware.option_roms.parts.lane", BASELINE),
    ("machine.firmware.option_roms.parts.offset", BASELINE),
    ("machine.firmware.option_roms.parts.sha256", BASELINE),
    ("machine.firmware.option_roms.image", BASELINE),
    ("machine.firmware.option_roms.sha256", BASELINE),
    ("machine.firmware.option_roms.base", BASELINE),
    ("machine.firmware.option_roms.provides", BASELINE),
    ("machine.card", BASELINE),
    ("machine.card.id", BASELINE),
    ("machine.card.kind", BASELINE),
    ("machine.card.controller_model", BASELINE),
    ("machine.card.mouse", BASELINE),
    ("machine.card.port", BASELINE),
    ("machine.card.frame", BASELINE),
    ("machine.card.size_kib", BASELINE),
    ("machine.card.base", BASELINE),
    ("machine.card.dma8", BASELINE),
    ("machine.card.dma16", BASELINE),
    ("machine.card.irq", BASELINE),
    ("machine.card.dma", BASELINE),
    ("machine.card.remove", BASELINE),
    ("machine.card.drives", BASELINE),
    ("machine.card.drives.unit", BASELINE),
    ("machine.card.drives.channel", BASELINE),
    ("machine.card.drives.type", BASELINE),
    ("machine.card.drives.geometry", BASELINE),
    ("machine.card.drives.geometry.cylinders", BASELINE),
    ("machine.card.drives.geometry.heads", BASELINE),
    ("machine.card.drives.geometry.sectors", BASELINE),
    ("machine.card.drives.remove", BASELINE),
];

/// Keys of a PC installation-set format table, under each medium.
pub const MEDIA_FORMAT_KEYS: &[Row] = &[
    ("media.format.kind", BASELINE),
    ("media.format.geometry", BASELINE),
    ("media.format.geometry.cylinders", BASELINE),
    ("media.format.geometry.heads", BASELINE),
    ("media.format.geometry.sectors", BASELINE),
    ("media.format.geometry.sector_bytes", BASELINE),
    ("media.format.encoding", BASELINE),
];

/// Keys of a PC firmware package's fit table.
pub const FIT_KEYS: &[Row] = &[
    ("fits.boards", BASELINE),
    ("fits.machine_classes", BASELINE),
];

/// Installation-set media formats ([`crate::media_set::format_name`]).
pub const MEDIA_FORMATS: &[Row] = &[
    (assembly::descriptors::FloppyType::HD_1_44.name, BASELINE),
    (assembly::descriptors::FloppyType::HD_1_2.name, BASELINE),
    (assembly::descriptors::FloppyType::DD_720K.name, BASELINE),
    (assembly::descriptors::FloppyType::DD_360K.name, BASELINE),
    ("iso-2048", BASELINE),
];

/// Firmware-package ROM roles ([`crate::firmware::ROLES`]).
pub const ROLES: &[Row] = &[
    ("system-bios", BASELINE),
    ("option-rom", BASELINE),
    ("video-bios", BASELINE),
    ("keyboard-controller", BASELINE),
    ("other", BASELINE),
];

/// Firmware-package fit boards ([`crate::firmware::BOARDS`], plus any board
/// a catalogue entry names).
pub const FIT_BOARDS: &[Row] = &[
    ("ibm-5170", BASELINE),
    ("ibm-5150", BASELINE),
    ("ibm-5160", BASELINE),
    ("compaq-deskpro", BASELINE),
    ("tandy-1000", BASELINE),
    ("generic-386-isa", BASELINE),
    ("i440fx-piix3", BASELINE),
    ("i440bx-piix4", BASELINE),
];

/// Firmware-package fit machine classes ([`crate::firmware::MACHINE_CLASSES`]).
pub const FIT_CLASSES: &[Row] = &[("at", BASELINE), ("xt", BASELINE)];

/// Firmware-package chip lanes.
pub const LANES: &[Row] = &[("even", BASELINE), ("odd", BASELINE)];

/// Firmware catalogue entries, by id ([`crate::firmware::catalogue`]). A
/// release that does not list an entry refuses its bytes.
pub const CATALOGUE: &[Row] = &[
    ("kiselev-xi8088-1.0.2", BASELINE),
    ("kiselev-xi8088-1.0.2-atreset", BASELINE),
    ("unemu-at-bios-1.0.2-unemu.3", BASELINE),
    ("unemu-at-bios-1.0.2-unemu.2", BASELINE),
    ("ibm-5170-bios-1984-01-10", BASELINE),
    ("ibm-5170-bios-1985-06-10", BASELINE),
    ("ibm-5170-bios-1985-11-15", BASELINE),
    ("landmark-rom-post-at-1.2.0", BASELINE),
    ("ibm-vga-bios-1987-32k", BASELINE),
    ("vgabios-0.9d-unemu.1-std", BASELINE),
    ("vgabios-0.8a-unemu.1-std", BASELINE),
    ("xub-r638-at", BASELINE),
    ("xub-r638-xtp-ata1f0", BASELINE),
];

fn row(rows: &[Row], what: &str, value: &str) -> image_store::Result<AppVersion> {
    compat::lookup(&[rows], value).ok_or_else(|| {
        image_store::Error::Invalid(format!("no compatibility row for {what} {value}"))
    })
}

fn invalid(message: impl Into<String>) -> image_store::Error {
    image_store::Error::Invalid(message.into())
}

/// The release this build reads; see [`image_store::compat::level`].
pub fn level() -> AppVersion {
    compat::level()
}

/// The oldest release that reads a PC artifact of any type, from its
/// manifest and config bytes: the maximum over every row it uses.
pub fn release(manifest: &Manifest, config: &[u8]) -> image_store::Result<AppVersion> {
    let text = std::str::from_utf8(config).map_err(|e| invalid(e.to_string()))?;
    let table: toml::Table = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    let envelope = compat::envelope(manifest, PAYLOADS)?;
    let content = match manifest.artifact_type() {
        mt::MACHINE => machine_release(manifest, &table)?,
        mt::MEDIA_SET => media_set_release(&table, config)?,
        mt::FIRMWARE => firmware_release(&table, config)?,
        other => return Err(invalid(format!("no PC rows for artifact type {other}"))),
    };
    Ok(envelope.max(content))
}

/// Stamp the marker: what [`release`] computes and what the artifact
/// `inherited` from the packages it is made from ([`packages_floor`]), raised
/// to `stated`; see [`image_store::compat::stamp`].
pub fn stamp(
    manifest: &mut Manifest,
    config: &[u8],
    inherited: AppVersion,
    stated: Option<AppVersion>,
) -> image_store::Result<()> {
    let required = release(manifest, config)?.max(inherited);
    compat::stamp(manifest, required, stated)
}

/// The newest release the firmware packages a machine binds declare: a
/// machine needs at least what each of its packages needs, an emulation fix
/// stated at a package's import included. A package this build does not
/// read is refused, naming it.
pub fn packages_floor<'a>(
    packages: impl IntoIterator<Item = &'a crate::create::Package>,
) -> image_store::Result<AppVersion> {
    let mut floor = BASELINE;
    for package in packages {
        let requirement = Requirement::of(&package.manifest);
        requirement.check(&package.reference)?;
        floor = floor.max(requirement.release()?);
    }
    Ok(floor)
}

/// The packages of `packages` that a machine manifest lists in
/// `unemu.firmware.sources`, the packages its ROMs came from.
pub fn listed_packages<'a>(
    manifest: &Manifest,
    packages: &'a [crate::create::Package],
) -> Vec<&'a crate::create::Package> {
    let listed: Vec<Digest> = manifest
        .annotations
        .get(crate::revision::FIRMWARE_SOURCES)
        .map(|text| text.trim().trim_start_matches('[').trim_end_matches(']'))
        .unwrap_or_default()
        .split(',')
        .filter_map(|d| d.trim().trim_matches('"').parse().ok())
        .collect();
    packages
        .iter()
        .filter(|p| listed.contains(&p.descriptor.digest))
        .collect()
}

fn machine_release(manifest: &Manifest, table: &toml::Table) -> image_store::Result<AppVersion> {
    let mut release =
        compat::keys(table, &[MACHINE_KEYS], &[])?.max(compat::config_platform(table));
    let config: crate::Config = crate::decode(table.clone()).map_err(store_error)?;
    let machine: schema::MachineFile = toml::Value::Table(config.machine.clone())
        .try_into()
        .map_err(|e: toml::de::Error| invalid(e.to_string()))?;
    release = release.max(machine_values(&machine));
    for medium in &config.media {
        release = release.max(policy(medium.policy));
    }
    if let Some(nvram) = &config.nvram {
        release = release.max(nvram_mode(&nvram.mode));
    }
    // Catalogue entries by the bytes the image carries: a whole ROM image or
    // one chip of a known layout.
    for layer in &manifest.layers {
        if layer.decoded_media_type() != mt::FIRMWARE {
            continue;
        }
        let (digest, size) = (layer.decoded_digest()?, layer.decoded_size()?);
        for entry in crate::firmware::catalogue() {
            let chip = entry
                .parts
                .iter()
                .flat_map(|layout| layout.iter())
                .any(|p| p.sha256 == digest.hex() && p.size == size);
            if chip || (entry.sha256 == digest.hex() && entry.size == size) {
                release = release.max(row(CATALOGUE, "firmware catalogue entry", entry.id)?);
            }
        }
    }
    Ok(release)
}

fn media_set_release(table: &toml::Table, config: &[u8]) -> image_store::Result<AppVersion> {
    let mut release = compat::keys(table, &[MEDIA_SET_KEYS, EXTRA_KEYS, MEDIA_FORMAT_KEYS], &[])?
        .max(compat::config_platform(table));
    let config = media_set::Config::decode(config)?;
    release = release.max(compat::media_set_values(&config));
    for medium in &config.media {
        let name = crate::media_set::format_name(&medium.format)?;
        release = release.max(row(MEDIA_FORMATS, "media format", name)?);
    }
    Ok(release)
}

/// What an installation set's config alone needs, before its payloads are
/// stored: every payload type a set holds is read by the baseline, so this is
/// what [`release`] will compute for the published set.
pub fn media_set_config(config: &media_set::Config) -> image_store::Result<AppVersion> {
    let bytes = config.encode()?;
    let text = std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?;
    let table: toml::Table = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    media_set_release(&table, &bytes)
}

fn firmware_release(table: &toml::Table, config: &[u8]) -> image_store::Result<AppVersion> {
    let mut release = compat::keys(table, &[FIRMWARE_KEYS, EXTRA_KEYS, FIT_KEYS], &[])?
        .max(compat::config_platform(table));
    let config = firmware::Config::decode(config)?;
    release = release.max(compat::firmware_values(&config));
    let fit = crate::firmware::Fit::decode(&config.fits)?;
    for board in &fit.boards {
        release = release.max(row(FIT_BOARDS, "PC fit board", board)?);
    }
    for class in &fit.machine_classes {
        release = release.max(row(FIT_CLASSES, "PC machine class", class)?);
    }
    for rom in &config.roms {
        release = release.max(row(ROLES, "PC firmware role", &rom.role)?);
        // Only this build's catalogue has rows; an id from a catalogue a test
        // injects is one no release lists.
        let listed = rom
            .catalogue_id
            .as_deref()
            .filter(|id| crate::firmware::catalogue().iter().any(|e| e.id == *id));
        if let Some(id) = listed {
            release = release.max(row(CATALOGUE, "firmware catalogue entry", id)?);
        }
        for part in &rom.parts {
            release = release.max(row(LANES, "PC chip lane", &part.lane)?);
        }
    }
    Ok(release)
}

fn store_error(error: crate::Error) -> image_store::Error {
    match error {
        crate::Error::Store(error) => error,
        other => invalid(other.to_string()),
    }
}

/// The newest release the vocabulary values of a flattened machine need.
fn machine_values(machine: &schema::MachineFile) -> AppVersion {
    let mut values = Vec::new();
    if let Some(board) = &machine.board {
        values.extend(board.model.map(board_model));
        values.extend(board.display_switch.map(display_switch));
        values.extend(board.post_port.map(number_or_none));
        values.extend(
            board
                .rtc
                .as_ref()
                .and_then(|rtc| rtc.nvram.as_ref())
                .map(rtc_nvram),
        );
    }
    if let Some(cpu) = &machine.cpu {
        values.extend(cpu.model.map(cpu_model));
        values.extend(cpu.fpu.map(fpu_model));
    }
    if let Some(firmware) = &machine.firmware {
        let roms = firmware
            .bios
            .iter()
            .chain(firmware.option_roms.iter().flatten());
        for rom in roms {
            values.extend(rom.provides.iter().flatten().map(|s| firmware_service(*s)));
            values.extend(rom.parts.iter().flatten().map(|p| lane(p.lane)));
        }
    }
    for card in machine.card.iter().flatten() {
        values.extend(card.kind.map(card_kind));
        values.extend(card.controller_model.map(controller_model));
        values.extend(card.mouse.map(serial_mouse));
        values.extend(card.irq.map(number_or_none));
        values.extend(card.dma.map(number_or_none));
        for drive in card.drives.iter().flatten() {
            values.extend(drive.channel.map(channel));
            values.extend(drive.drive_type.map(drive_type));
            values.extend(drive.write.map(write_mode));
            values.extend(
                drive
                    .media
                    .iter()
                    .flatten()
                    .filter_map(|m| m.write.map(write_mode)),
            );
            values.extend(drive.geometry.as_ref().map(geometry));
        }
    }
    values.into_iter().fold(BASELINE, AppVersion::max)
}

/// The release that first reads a board model.
pub fn board_model(value: schema::BoardModel) -> AppVersion {
    match value {
        schema::BoardModel::Ibm5170 => BASELINE,
    }
}

/// The release that first reads a CPU model.
pub fn cpu_model(value: schema::CpuModel) -> AppVersion {
    match value {
        schema::CpuModel::I80286 => BASELINE,
    }
}

/// The release that first reads a coprocessor choice.
pub fn fpu_model(value: schema::FpuModel) -> AppVersion {
    match value {
        schema::FpuModel::None | schema::FpuModel::I80287 => BASELINE,
    }
}

/// The release that first reads a card kind.
pub fn card_kind(value: schema::CardKind) -> AppVersion {
    use schema::CardKind::*;
    match value {
        Mda | VgaText | Vga | Uart16550 | Fdc765 | Ide | Sb16 | LotechEms => BASELINE,
    }
}

/// The release that first reads an IDE controller model.
pub fn controller_model(value: schema::ControllerModel) -> AppVersion {
    match value {
        schema::ControllerModel::Ata | schema::ControllerModel::AtAdapter => BASELINE,
    }
}

/// The release that first reads a serial mouse model.
pub fn serial_mouse(value: schema::SerialMouseModel) -> AppVersion {
    match value {
        schema::SerialMouseModel::Microsoft | schema::SerialMouseModel::None => BASELINE,
    }
}

/// The release that first reads a floppy drive type.
pub fn drive_type(value: schema::DriveType) -> AppVersion {
    use schema::DriveType::*;
    match value {
        Hd144 | Hd12 | Dd720 | Dd360 => BASELINE,
    }
}

/// The release that first reads an IDE channel.
pub fn channel(value: schema::Channel) -> AppVersion {
    match value {
        schema::Channel::Master | schema::Channel::Slave => BASELINE,
    }
}

/// The release that first reads a media write mode.
pub fn write_mode(value: schema::WriteMode) -> AppVersion {
    use schema::WriteMode::*;
    match value {
        Volatile | ReadOnly | Journal => BASELINE,
    }
}

/// The release that first reads a split ROM's byte lane.
pub fn lane(value: schema::Lane) -> AppVersion {
    match value {
        schema::Lane::Even | schema::Lane::Odd => BASELINE,
    }
}

/// The release that first reads a declared firmware service.
pub fn firmware_service(value: schema::FirmwareService) -> AppVersion {
    match value {
        schema::FirmwareService::Floppy | schema::FirmwareService::FixedDisk => BASELINE,
    }
}

/// The release that first reads a primary-display switch setting.
pub fn display_switch(value: schema::DisplaySwitch) -> AppVersion {
    use schema::DisplaySwitch::*;
    match value {
        Auto | Mono | Colour => BASELINE,
    }
}

/// The release that first reads a line number or its explicit `"none"`.
pub fn number_or_none<const MAX: u16>(value: schema::NumberOrNone<MAX>) -> AppVersion {
    match value {
        schema::NumberOrNone::Number(_) | schema::NumberOrNone::None => BASELINE,
    }
}

/// The release that first reads a form of the RTC NVRAM setting.
pub fn rtc_nvram(value: &schema::Nvram) -> AppVersion {
    match value {
        schema::Nvram::Derived | schema::Nvram::Blank => BASELINE,
        schema::Nvram::Image(image) => image.write.map_or(BASELINE, write_mode),
    }
}

/// The release that first reads a fixed-disk geometry form.
pub fn geometry(value: &schema::Geometry) -> AppVersion {
    match value {
        schema::Geometry::Chs(_) => BASELINE,
    }
}

/// The release that first reads a bundle medium's policy.
pub fn policy(value: Policy) -> AppVersion {
    match value {
        Policy::Readonly | Policy::Volatile | Policy::Journal => BASELINE,
    }
}

fn nvram_mode(value: &NvramMode) -> AppVersion {
    match value {
        NvramMode::Image | NvramMode::Journal => BASELINE,
    }
}

/// A provider with the PC rows of the table: [`ConfigValidator::required_release`]
/// computes [`release`], everything else is the wrapped provider's. The
/// shared installation-set and firmware-package validators know only the
/// shared rows; the push guard of the official namespace wraps them in this.
pub struct WithPcRows<'a>(pub &'a dyn ConfigValidator);

impl ConfigValidator for WithPcRows<'_> {
    fn artifact_type(&self) -> &str {
        self.0.artifact_type()
    }
    fn platform(&self) -> &str {
        self.0.platform()
    }
    fn validate_resolved(&self, image: &ResolvedImage, store: &Store) -> image_store::Result<()> {
        self.0.validate_resolved(image, store)
    }
    fn payload_types(&self) -> &[PayloadType] {
        self.0.payload_types()
    }
    fn validate(
        &self,
        config: &toml::Table,
        descriptors: &[Descriptor],
        blobs: &dyn BlobSource,
    ) -> image_store::Result<Vec<Digest>> {
        self.0.validate(config, descriptors, blobs)
    }
    fn required_release(
        &self,
        manifest: &Manifest,
        config: &[u8],
    ) -> image_store::Result<AppVersion> {
        release(manifest, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::{BOARDS, MACHINE_CLASSES};
    use image_store::compat::{struct_fields, NEWEST_ROW};

    fn paths(prefix: &str, fields: &[&str]) -> Vec<String> {
        fields
            .iter()
            .map(|field| {
                if prefix.is_empty() {
                    (*field).to_owned()
                } else {
                    format!("{prefix}.{field}")
                }
            })
            .collect()
    }

    /// Timing requests: schema 1 refuses them in the machine, board, CPU and
    /// card tables.
    const TIMING: &[&str] = &[
        "mhz",
        "frequency",
        "clock_hz",
        "cycles",
        "cycle_exact",
        "wait_states",
        "bus_mhz",
        "memory_wait_states",
        "timing",
    ];

    /// Every field of every config struct has a row or is a key a bundle's
    /// machine never holds, and every row names a field, so a new optional
    /// config key cannot ship without a row. Schema 1 refuses the reserved
    /// names (timing, unmodelled hardware, run and presentation tables); the
    /// bundle refuses descriptive and inheritance keys, a drive's media keys
    /// and the NVRAM image's policy keys.
    #[test]
    fn every_machine_config_key_has_a_row() {
        let mut expected = Vec::new();
        let mut check = |prefix: &str, fields: &[&str], refused: &[&[&str]]| {
            for (field, path) in fields.iter().zip(paths(prefix, fields)) {
                if refused.iter().any(|list| list.contains(field)) {
                    assert!(
                        compat::lookup(&[MACHINE_KEYS], &path).is_none(),
                        "{path} has a row but no bundle holds it"
                    );
                    continue;
                }
                assert!(
                    compat::lookup(&[MACHINE_KEYS], &path).is_some(),
                    "no row for config key {path}"
                );
                expected.push(path);
            }
        };
        check("", struct_fields::<crate::Config>(), &[]);
        check("firmware", struct_fields::<crate::Firmware>(), &[]);
        check("media", struct_fields::<crate::Medium>(), &[]);
        check("media.geometry", struct_fields::<crate::Geometry>(), &[]);
        check("slots", struct_fields::<crate::Slot>(), &[]);
        check("nvram", struct_fields::<crate::Nvram>(), &[]);
        check(
            "machine",
            struct_fields::<schema::MachineFile>(),
            &[
                TIMING,
                &[
                    "run",
                    "presentation",
                    "streams",
                    "name",
                    "description",
                    "extends",
                ],
            ],
        );
        check(
            "machine.board",
            struct_fields::<schema::Board>(),
            &[
                TIMING,
                &[
                    "a20",
                    "kbc",
                    "keyboard",
                    "keyboard_layout",
                    "dip_switches",
                    "chipset",
                    "speaker",
                ],
            ],
        );
        check("machine.board.rtc", struct_fields::<schema::Rtc>(), &[]);
        check(
            "machine.board.rtc.nvram",
            struct_fields::<schema::NvramImage>(),
            &[&["sha256", "write", "journal", "keep_generations"]],
        );
        check(
            "machine.cpu",
            struct_fields::<schema::Cpu>(),
            &[TIMING, &["turbo"]],
        );
        check(
            "machine.memory",
            struct_fields::<schema::Memory>(),
            &[&["shadow", "umb", "upper"]],
        );
        check(
            "machine.firmware",
            struct_fields::<schema::Firmware>(),
            &[&["basic"]],
        );
        for rom in ["bios", "option_roms"] {
            let prefix = format!("machine.firmware.{rom}");
            check(&prefix, struct_fields::<schema::Rom>(), &[]);
            check(
                &format!("{prefix}.parts"),
                struct_fields::<schema::RomPart>(),
                &[],
            );
        }
        check(
            "machine.card",
            struct_fields::<schema::Card>(),
            &[TIMING, &["monitor", "rom", "mmio"]],
        );
        check(
            "machine.card.drives",
            struct_fields::<schema::Drive>(),
            &[&[
                "image",
                "sha256",
                "write",
                "journal",
                "keep_generations",
                "media",
                "empty",
            ]],
        );
        check(
            "machine.card.drives.geometry",
            struct_fields::<schema::Chs>(),
            &[],
        );
        for (row, _) in MACHINE_KEYS {
            assert!(expected.iter().any(|p| p == row), "row {row} names no key");
        }
        let fits = paths("fits", struct_fields::<crate::firmware::Fit>());
        for path in &fits {
            assert!(compat::lookup(&[FIT_KEYS], path).is_some(), "{path}");
        }
        assert_eq!(fits.len(), FIT_KEYS.len());
    }

    #[test]
    fn every_payload_type_catalogue_entry_and_string_vocabulary_value_has_a_row() {
        for kind in crate::PAYLOAD_TYPES {
            assert!(
                compat::payload(kind.media_type)
                    .or_else(|| compat::lookup(&[PAYLOADS], kind.media_type))
                    .is_some(),
                "no row for payload type {}",
                kind.media_type
            );
        }
        for entry in crate::firmware::catalogue() {
            assert!(
                compat::lookup(&[CATALOGUE], entry.id).is_some(),
                "no row for firmware catalogue entry {}",
                entry.id
            );
            for board in entry.boards {
                assert!(compat::lookup(&[FIT_BOARDS], board).is_some(), "{board}");
            }
        }
        assert_eq!(CATALOGUE.len(), crate::firmware::catalogue().len());
        for (rows, values) in [
            (ROLES, crate::firmware::ROLES),
            (FIT_BOARDS, BOARDS),
            (FIT_CLASSES, MACHINE_CLASSES),
        ] {
            for value in values {
                assert!(compat::lookup(&[rows], value).is_some(), "{value}");
            }
            assert_eq!(rows.len(), values.len());
        }
        for format in crate::media_import::format_names() {
            assert!(
                compat::lookup(&[MEDIA_FORMATS], format).is_some(),
                "{format}"
            );
        }
        for kind in assembly::descriptors::FLOPPY_TYPES {
            assert!(compat::lookup(&[MEDIA_FORMATS], kind.name).is_some());
        }
    }

    /// What every value of the vocabulary enums rates, listed by hand: a new
    /// variant does not compile without its rating, and belongs here too.
    fn vocabulary() -> Vec<AppVersion> {
        use schema::*;
        let mut all = vec![
            board_model(BoardModel::Ibm5170),
            cpu_model(CpuModel::I80286),
        ];
        all.extend([FpuModel::None, FpuModel::I80287].map(fpu_model));
        all.extend(
            [
                CardKind::Mda,
                CardKind::VgaText,
                CardKind::Vga,
                CardKind::Uart16550,
                CardKind::Fdc765,
                CardKind::Ide,
                CardKind::Sb16,
                CardKind::LotechEms,
            ]
            .map(card_kind),
        );
        all.extend([ControllerModel::Ata, ControllerModel::AtAdapter].map(controller_model));
        all.extend([SerialMouseModel::Microsoft, SerialMouseModel::None].map(serial_mouse));
        all.extend(
            [
                DriveType::Hd144,
                DriveType::Hd12,
                DriveType::Dd720,
                DriveType::Dd360,
            ]
            .map(drive_type),
        );
        all.extend([Channel::Master, Channel::Slave].map(channel));
        all.extend([WriteMode::Volatile, WriteMode::ReadOnly, WriteMode::Journal].map(write_mode));
        all.extend([Lane::Even, Lane::Odd].map(lane));
        all.extend([FirmwareService::Floppy, FirmwareService::FixedDisk].map(firmware_service));
        all.extend(
            [
                DisplaySwitch::Auto,
                DisplaySwitch::Mono,
                DisplaySwitch::Colour,
            ]
            .map(display_switch),
        );
        all.extend([NumberOrNone::<15>::Number(0), NumberOrNone::<15>::None].map(number_or_none));
        all.extend([Nvram::Derived, Nvram::Blank].iter().map(rtc_nvram));
        all.extend([Policy::Readonly, Policy::Volatile, Policy::Journal].map(policy));
        all.extend(
            [NvramMode::Image, NvramMode::Journal]
                .iter()
                .map(nvram_mode),
        );
        all
    }

    /// The level a build reads must cover every row it writes.
    #[test]
    fn newest_row_names_the_newest_release_in_the_tables() {
        let newest = [
            PAYLOADS,
            MACHINE_KEYS,
            MEDIA_FORMAT_KEYS,
            FIT_KEYS,
            MEDIA_FORMATS,
            ROLES,
            FIT_BOARDS,
            FIT_CLASSES,
            LANES,
            CATALOGUE,
            MEDIA_SET_KEYS,
            EXTRA_KEYS,
            FIRMWARE_KEYS,
            compat::REDISTRIBUTION,
            compat::PLATFORMS,
        ]
        .iter()
        .flat_map(|rows| rows.iter().map(|(_, since)| *since))
        .chain(vocabulary())
        .fold(BASELINE, AppVersion::max);
        assert_eq!(
            newest, NEWEST_ROW,
            "the tables name {newest}; set image_store::compat::NEWEST_ROW to it"
        );
        assert!(level() >= newest);
    }
}
