// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC installation formats; machine-images §5.2.
//! IBM Personal Computer AT Technical Reference, Diskette Drive Adapter:
//! exact floppy geometry is required until mixed-density mechanics are modelled.

use image_store::{media_set::MediaFormats, media_type, Error, Result};
use serde::{Deserialize, Serialize};

pub struct PcMediaFormats;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Format {
    Floppy { geometry: Geometry },
    Optical { encoding: String },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Geometry {
    cylinders: u16,
    heads: u8,
    sectors: u8,
    sector_bytes: u16,
}

pub fn format(name: &str) -> Result<toml::Table> {
    let format = if name == "iso-2048" {
        Format::Optical {
            encoding: name.into(),
        }
    } else {
        let kind = assembly::descriptors::FLOPPY_TYPES
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| Error::Invalid("unknown PC media format".into()))?;
        Format::Floppy {
            geometry: Geometry {
                cylinders: kind.geometry.cylinders as u16,
                heads: kind.geometry.heads as u8,
                sectors: kind.geometry.sectors_per_track as u8,
                sector_bytes: 512,
            },
        }
    };
    toml::Table::try_from(format).map_err(|e| Error::Invalid(e.to_string()))
}
pub fn detect(size: u64) -> Option<&'static str> {
    assembly::descriptors::FLOPPY_TYPES
        .iter()
        .find(|f| f.geometry.image_bytes() == size)
        .map(|f| f.name)
}
pub fn format_name(table: &toml::Table) -> Result<&'static str> {
    let f: Format = toml::Value::Table(table.clone())
        .try_into()
        .map_err(|e: toml::de::Error| Error::Invalid(e.to_string()))?;
    match f {
        Format::Optical { encoding } if encoding == "iso-2048" => Ok("iso-2048"),
        Format::Floppy { geometry: g } => assembly::descriptors::FLOPPY_TYPES
            .iter()
            .find(|f| {
                f.geometry
                    == assembly::FloppyGeometry {
                        cylinders: g.cylinders.into(),
                        heads: g.heads.into(),
                        sectors_per_track: g.sectors.into(),
                        bytes_per_sector: g.sector_bytes.into(),
                    }
            })
            .map(|f| f.name)
            .ok_or_else(|| Error::Invalid("unknown floppy geometry".into())),
        _ => Err(Error::Invalid("unknown PC media format".into())),
    }
}
impl MediaFormats for PcMediaFormats {
    fn platform(&self) -> &str {
        "ibm-pc-at"
    }
    fn validate_name(&self, name: &str) -> Result<()> {
        if valid_dos_name(name) {
            Ok(())
        } else {
            Err(Error::Invalid(
                "expected canonical uppercase DOS 8.3 name (no reserved device names)".into(),
            ))
        }
    }
    fn validate_format(&self, table: &toml::Table, size: u64) -> Result<&'static str> {
        let name = format_name(table)?;
        if name == "iso-2048" {
            if size == 0 || !size.is_multiple_of(2048) {
                return Err(Error::Invalid(
                    "ISO size must be a nonzero multiple of 2048".into(),
                ));
            }
            Ok(media_type::OPTICAL)
        } else if detect(size) == Some(name) {
            Ok(media_type::FLOPPY)
        } else {
            Err(Error::Invalid("media size differs from format".into()))
        }
    }
}

/// PC guest names use canonical uppercase DOS 8.3 spelling.
pub fn valid_dos_name(name: &str) -> bool {
    machine_file::pc_names::valid_dos_name(name)
}

/// Geometry comparisons use the same declarations as assembly and the drive.
pub fn geometry(table: &toml::Table) -> Result<Option<assembly::FloppyGeometry>> {
    let name = format_name(table)?;
    Ok(assembly::descriptors::FLOPPY_TYPES
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.geometry))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_set_formats_reject_unknown_fields_sizes_and_optical_encoding() {
        for kind in assembly::descriptors::FLOPPY_TYPES {
            let f = format(kind.name).unwrap();
            let size = kind.geometry.image_bytes();
            assert_eq!(
                PcMediaFormats.validate_format(&f, size).unwrap(),
                media_type::FLOPPY
            );
            assert!(PcMediaFormats.validate_format(&f, size + 1).is_err());
            let mut unknown = f.clone();
            unknown.insert("unknown".into(), true.into());
            assert!(PcMediaFormats.validate_format(&unknown, size).is_err());
        }
        assert!(format("unknown").is_err());
        let iso = format("iso-2048").unwrap();
        assert_eq!(
            PcMediaFormats.validate_format(&iso, 4096).unwrap(),
            media_type::OPTICAL
        );
        for size in [0, 2049] {
            assert!(PcMediaFormats.validate_format(&iso, size).is_err());
        }
        assert_eq!(detect(4096), None);
    }
}

/// Construct the existing sector adapter from a captured full-verification receipt.
pub fn verified_base(
    image: &image_store::ResolvedImage,
    digest: &image_store::Digest,
) -> crate::Result<media::VerifiedBase> {
    let (file, metadata) = image.verified_file(digest)?;
    Ok(media::VerifiedBase::from_verified_file(
        file,
        media::Header {
            base_length: metadata.len(),
            base_digest: crate::provider::hash_array(digest)?,
            anchor_generation: 0,
        },
        metadata,
    )?)
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn review_pc_names_require_uppercase_and_reject_devices() {
        for name in ["README.TXT", "SETUP.EXE", "A", "PATCH_1.ZIP"] {
            PcMediaFormats.validate_name(name).unwrap();
        }
        for name in [
            "readme.txt",
            "Readme.TXT",
            "CON",
            "CON.TXT",
            "NUL",
            "AUX",
            "PRN",
            "CLOCK$",
            "COM1.EXE",
            "LPT9",
            "LONGERTHAN8.TXT",
            "A..B",
            "A.",
            "A B.TXT",
        ] {
            assert!(PcMediaFormats.validate_name(name).is_err(), "{name}");
        }
    }
}
