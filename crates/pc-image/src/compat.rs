// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC package-format compatibility and the conversion of core-owned release data;
//! machine-images §5.6. Hardware keys, vocabulary, accepted core platform ids and
//! firmware catalogue rows live in `pc_config::compat`, which has no store dependency.
//! This provider keeps payload types, media formats, fit tables, roles and package
//! lanes on the front-end side. Core and front-end floors are computed separately.

use crate::{CMOS, DISK_ZERO};
use image_store::{
    compat::{
        self, AppVersion, Compatibility, Levels, Releases, Requirement, Requirements, Row,
        BASELINE, EXTRA_KEYS, FIRMWARE_KEYS, MEDIA_SET_KEYS,
    },
    firmware, media_set, media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest,
    Manifest, PayloadType, ResolvedImage, Store,
};

/// Payload types only this provider declares; the compressed firmware,
/// floppy and source-archive types are shared rows.
pub const PAYLOADS: &[Row] = &[
    (CMOS, BASELINE),
    (DISK_ZERO, BASELINE),
    ("application/vnd.unemu.disk.base.v1+zstd", BASELINE),
    ("application/vnd.unemu.disk.sectors.v1+zstd", BASELINE),
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
    ("generic-486-isa", AppVersion::new(0, 0, 5)),
    ("i440fx-piix3", BASELINE),
    ("i440bx-piix4", BASELINE),
];

/// Firmware-package fit machine classes ([`crate::firmware::MACHINE_CLASSES`]).
pub const FIT_CLASSES: &[Row] = &[("at", BASELINE), ("xt", BASELINE)];

/// Firmware-package chip lanes.
pub const LANES: &[Row] = &[("even", BASELINE), ("odd", BASELINE)];

fn row(rows: &[Row], what: &str, value: &str) -> image_store::Result<AppVersion> {
    compat::lookup(&[rows], value).ok_or_else(|| {
        image_store::Error::Invalid(format!("no compatibility row for {what} {value}"))
    })
}

fn invalid(message: impl Into<String>) -> image_store::Error {
    image_store::Error::Invalid(message.into())
}

/// The standalone core level, raised only to its core-owned newest row.
pub fn level() -> AppVersion {
    from_core(pc_config::compat::package()).max(from_core(pc_config::compat::NEWEST_ROW))
}

/// PC front-end rows, kept separate from the core rows in `pc_config::compat`.
pub const NEXT_RELEASE: AppVersion = AppVersion::new(0, 0, 6);
pub const NEWEST_ROW: AppVersion = AppVersion::new(0, 0, 5);

/// Standalone PC tools supply their application release explicitly.
pub fn compatibility(frontend: AppVersion) -> Compatibility {
    Compatibility::new(frontend, newest_frontend_row()).with_core(
        crate::PLATFORM,
        from_core(pc_config::compat::package()),
        from_core(pc_config::compat::NEWEST_ROW),
    )
}

pub fn newest_frontend_row() -> AppVersion {
    NEWEST_ROW
        .max(image_store::compat::NEWEST_ROW)
        .max(recipe::compat::NEWEST_ROW)
}

pub fn core_newest_row() -> AppVersion {
    from_core(pc_config::compat::NEWEST_ROW)
}

pub fn levels(frontend: AppVersion) -> Levels {
    compatibility(frontend).levels(crate::PLATFORM)
}

/// A selected application's levels win; standalone PC codecs know their own core release.
pub fn source_levels(source: &dyn BlobSource) -> Levels {
    if source.compatibility_configured() {
        source.compatibility_levels(crate::PLATFORM)
    } else {
        levels(compat::EXTRACTION_BASELINE)
    }
}

/// The independent core and packaging floors of a PC artifact.
pub fn releases(manifest: &Manifest, config: &[u8]) -> image_store::Result<Releases> {
    let text = std::str::from_utf8(config).map_err(|e| invalid(e.to_string()))?;
    let table: toml::Table = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    let mut core =
        from_core(pc_config::compat::platform(manifest.platform()?).map_err(store_error_config)?);
    if let Some(id) = table.get("platform").and_then(toml::Value::as_str) {
        core = core.max(from_core(
            pc_config::compat::platform(id).map_err(store_error_config)?,
        ));
    }
    let mut frontend = compat::envelope(manifest, PAYLOADS)?;
    match manifest.artifact_type() {
        mt::MACHINE => {
            core = core.max(machine_release(manifest, &table)?);
        }
        mt::MEDIA_SET => {
            frontend = frontend.max(media_set_release(&table, config)?);
        }
        mt::FIRMWARE => {
            frontend = frontend.max(firmware_release(&table, config)?);
            for rom in firmware::Config::decode(config)?.roms {
                if let Some(release) = rom
                    .catalogue_id
                    .as_deref()
                    .and_then(|id| pc_config::compat::lookup(&[pc_config::compat::CATALOGUE], id))
                {
                    core = core.max(from_core(release));
                }
            }
        }
        other => return Err(invalid(format!("no PC rows for artifact type {other}"))),
    }
    Ok(Releases { core, frontend })
}

/// The core floor; packaging features never raise it.
pub fn release(manifest: &Manifest, config: &[u8]) -> image_store::Result<AppVersion> {
    Ok(releases(manifest, config)?.core)
}

/// Stamp the marker: what [`release`] computes and what the artifact
/// `inherited` from the packages it is made from ([`packages_floor`]), raised
/// to `stated`; see [`image_store::compat::stamp`].
pub fn stamp(
    manifest: &mut Manifest,
    config: &[u8],
    inherited: AppVersion,
    stated: Option<AppVersion>,
    levels: Levels,
) -> image_store::Result<()> {
    let mut required = releases(manifest, config)?;
    required.core = required.core.max(inherited);
    compat::stamp(manifest, required, stated, levels)
}

/// The newest release the firmware packages a machine binds declare: a
/// machine needs at least what each of its packages needs, an emulation fix
/// stated at a package's import included. A package this build does not
/// read is refused, naming it.
pub fn packages_floor<'a>(
    levels: Levels,
    packages: impl IntoIterator<Item = &'a crate::create::Package>,
) -> image_store::Result<AppVersion> {
    let mut floor = BASELINE;
    for package in packages {
        let requirement = Requirement::of(&package.manifest);
        Requirements::of(&package.manifest).check(&package.reference, levels)?;
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
    let mut release = from_core(pc_config::compat::machine(table).map_err(store_error_config)?);
    for layer in &manifest.layers {
        if layer.decoded_media_type() == mt::FIRMWARE {
            release = release.max(from_core(
                pc_config::compat::firmware(layer.decoded_digest()?.hex(), layer.decoded_size()?)
                    .map_err(store_error_config)?,
            ));
        }
    }
    Ok(release)
}

pub fn from_core(release: pc_config::compat::Release) -> AppVersion {
    AppVersion::new(release.0, release.1, release.2)
}
fn store_error_config(error: pc_config::image::Error) -> image_store::Error {
    invalid(error.to_string())
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
    let _ = media_set_release(&table, &bytes)?;
    Ok(from_core(
        pc_config::compat::platform(&config.platform).map_err(store_error_config)?,
    ))
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
        for part in &rom.parts {
            release = release.max(row(LANES, "PC chip lane", &part.lane)?);
        }
    }
    Ok(release)
}

/// A provider with the PC rows of the table: [`ConfigValidator::required_release`]
/// computes [`release`], everything else is the wrapped provider's. The
/// shared installation-set and firmware-package validators know only the
/// shared rows; the push guard of the official namespace wraps them in this.
pub struct WithPcRows<'a>(pub &'a dyn ConfigValidator);

impl ConfigValidator for WithPcRows<'_> {
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

    fn core_release(&self) -> Option<(AppVersion, AppVersion)> {
        Some((from_core(pc_config::compat::package()), core_newest_row()))
    }

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
    fn required_releases(
        &self,
        manifest: &Manifest,
        config: &[u8],
    ) -> image_store::Result<Releases> {
        releases(manifest, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::{BOARDS, MACHINE_CLASSES};

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
                pc_config::compat::lookup(&[pc_config::compat::CATALOGUE], entry.id).is_some(),
                "no row for firmware catalogue entry {}",
                entry.id
            );
            for board in entry.boards {
                assert!(compat::lookup(&[FIT_BOARDS], board).is_some(), "{board}");
            }
        }
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

    #[test]
    fn every_package_board_in_the_core_vocabulary_has_a_fit_row() {
        use serde::{de, Deserialize};
        #[derive(Debug)]
        struct Names(&'static [&'static str]);
        impl std::fmt::Display for Names {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("board names")
            }
        }
        impl std::error::Error for Names {}
        impl de::Error for Names {
            fn custom<T: std::fmt::Display>(_: T) -> Self {
                Self(&[])
            }
            fn unknown_variant(_: &str, expected: &'static [&'static str]) -> Self {
                Self(expected)
            }
        }
        let error = pc_config::schema::v1::BoardModel::deserialize(de::value::StrDeserializer::<
            Names,
        >::new(""))
        .unwrap_err();
        assert!(!error.0.is_empty());
        for board in error.0 {
            // A bare board is a Rust test rig and cannot consume file-based firmware packages.
            if *board != "bare" {
                assert!(compat::lookup(&[FIT_BOARDS], board).is_some(), "{board}");
                assert!(BOARDS.contains(board), "{board}");
            }
        }
    }

    /// The level a build reads must cover every row it writes.
    #[test]
    fn newest_row_names_the_newest_release_in_the_tables() {
        let newest = [
            PAYLOADS,
            MEDIA_FORMAT_KEYS,
            FIT_KEYS,
            MEDIA_FORMATS,
            ROLES,
            FIT_BOARDS,
            FIT_CLASSES,
            LANES,
            MEDIA_SET_KEYS,
            EXTRA_KEYS,
            FIRMWARE_KEYS,
            compat::REDISTRIBUTION,
            compat::PLATFORMS,
            compat::METADATA_SCHEMAS,
            compat::TITLE_FIELDS,
        ]
        .iter()
        .flat_map(|rows| rows.iter().map(|(_, since)| *since))
        .fold(BASELINE, AppVersion::max);
        assert_eq!(
            newest, NEWEST_ROW,
            "the tables name {newest}; update the front-end newest row"
        );
        assert!(levels(compat::EXTRACTION_BASELINE).frontend >= newest);
        let frontend: AppVersion = env!("CARGO_PKG_VERSION").parse().unwrap();
        assert!(NEXT_RELEASE > frontend);
        assert_eq!(NEXT_RELEASE, compat::NEXT_RELEASE);
        assert_eq!(NEXT_RELEASE, recipe::compat::NEXT_RELEASE);
    }
}
