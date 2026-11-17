// SPDX-License-Identifier: AGPL-3.0-or-later
//! The IBM PC compatible provider of recipes (recipes §10): what the
//! platform-neutral checks of `recipe` ask about an `ibm-pc` recipe.
//!
//! The answers are the ones the importers already give: the media formats
//! and exact sizes of [`crate::media_set`] (IBM Personal Computer AT
//! Technical Reference, Diskette Drive Adapter, for the four diskette
//! geometries; ISO 9660 2048-byte sectors for a CD-ROM), the volume profile
//! of the FAT12 builder for those diskettes (Microsoft FAT Specification,
//! FAT Directory Structure, short name field), the DOS names of extras, the
//! ROM roles, lanes, boards and firmware catalogue of [`crate::firmware`],
//! and the needs vocabulary of [`crate::needs`].
//!
//! The software platforms and `[ids]` catalogues an `ibm-pc` title may name
//! are this provider's own lists. Both are append-only, as published
//! recipes name them (recipes §9), and both stay within what metadata
//! schema 1 and 2 writers accept (`image_store::metadata`).

use image_store::media_set::MediaFormats;
use media::fat12::{FloppyFormat, ShortName, MAX_FOLDER_PATH};
use recipe::provider::{CatalogueEntry, FirmwareProfile, MediaProfile, Provider, VolumeProfile};

/// The `ibm-pc` recipe provider.
pub struct PcRecipes;

/// The software platforms an `ibm-pc` title runs on, in the order they were
/// accepted; append-only.
pub const SOFTWARE_PLATFORMS: &[&str] = &["DOS", "Windows 3.x", "Windows 9x", "CP/M", "other"];

/// The media formats of a `[[medium]]`: the four diskettes, then a CD-ROM.
pub const FORMATS: &[&str] = &["1.44M", "1.2M", "720K", "360K", OPTICAL];

const OPTICAL: &str = "iso-2048";

/// The chip lanes of a ROM in parts: even and odd bytes of the 16-bit bus
/// (IBM Personal Computer AT Technical Reference, System Board ROM).
pub const LANES: &[&str] = &["even", "odd"];

/// The FAT12 volume of one diskette format.
pub struct FatVolume(FloppyFormat);

static VOLUMES: [FatVolume; 4] = [
    FatVolume(FloppyFormat::Hd144),
    FatVolume(FloppyFormat::Hd12),
    FatVolume(FloppyFormat::Dd720),
    FatVolume(FloppyFormat::Dd360),
];

impl Provider for PcRecipes {
    fn platform(&self) -> &str {
        crate::PLATFORM
    }
    fn media(&self) -> &dyn MediaProfile {
        self
    }
    fn firmware(&self) -> &dyn FirmwareProfile {
        self
    }
    fn needs(&self) -> Option<&dyn image_store::needs::Vocabulary> {
        Some(&crate::needs::PcNeeds)
    }
    fn software_platforms(&self) -> &[&str] {
        SOFTWARE_PLATFORMS
    }
    fn work_id_catalogues(&self) -> &[&str] {
        image_store::metadata::WORK_ID_KINDS
    }
}

impl MediaProfile for PcRecipes {
    fn formats(&self) -> &[&str] {
        FORMATS
    }

    fn size_fault(&self, format: &str, size: u64) -> Option<String> {
        let table = crate::media_set::format(format).ok()?;
        crate::media_set::PcMediaFormats
            .validate_format(&table, size)
            .err()?;
        Some(match FloppyFormat::from_name(format) {
            Some(floppy) => format!(
                "a {format} medium is exactly {} bytes",
                floppy.image_bytes()
            ),
            None => {
                format!("an {OPTICAL} medium is a whole number of 2048-byte sectors, at least one")
            }
        })
    }

    fn volume_profile(&self, format: &str) -> Option<&dyn VolumeProfile> {
        VOLUMES
            .iter()
            .find(|v| v.0.name() == format)
            .map(|v| v as &dyn VolumeProfile)
    }

    fn dos_name_fault(&self, name: &str) -> Option<String> {
        crate::media_set::PcMediaFormats
            .validate_name(name)
            .err()
            .map(|e| match e {
                image_store::Error::Invalid(message) => message,
                other => other.to_string(),
            })
    }
}

impl VolumeProfile for FatVolume {
    fn path_fault(&self, path: &str) -> Option<String> {
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts {
            if let Err(fault) = ShortName::strict(part) {
                return Some(format!("{part}: {fault}"));
            }
        }
        let folder = format!("\\{}", parts[..parts.len() - 1].join("\\"));
        (parts.len() > 1 && folder.len() > MAX_FOLDER_PATH).then(|| {
            format!(
                "the folder {folder} is {} characters long; DOS keeps at most {MAX_FOLDER_PATH} for a current directory",
                folder.len()
            )
        })
    }

    /// The label as stored: a label the builder would upper-case is
    /// refused, as a name is, never changed (recipes §6.9).
    fn label_fault(&self, label: &str) -> Option<String> {
        if let Err(fault) = media::fat12::volume_label(label) {
            return Some(fault.to_owned());
        }
        label.chars().find(char::is_ascii_lowercase).map(|lower| {
            format!("the lower-case letter {lower:?} is not allowed: a label is stored in upper case, so write it as it is on the disk")
        })
    }
}

impl FirmwareProfile for PcRecipes {
    fn roles(&self) -> &[&str] {
        crate::firmware::ROLES
    }
    fn lanes(&self) -> &[&str] {
        LANES
    }
    fn boards(&self) -> &[&str] {
        crate::firmware::BOARDS
    }
    fn machine_classes(&self) -> &[&str] {
        crate::firmware::MACHINE_CLASSES
    }
    fn entry(&self, id: &str) -> Option<CatalogueEntry> {
        let entry = crate::firmware::catalogue().iter().find(|e| e.id == id)?;
        Some(catalogue_entry(entry))
    }
    fn identify(&self, sha256: &str, size: u64) -> Option<(String, CatalogueEntry)> {
        let entry = crate::firmware::catalogue().iter().find(|e| {
            e.sha256 == sha256
                || e.parts
                    .iter()
                    .flat_map(|layout| layout.iter())
                    .any(|chip| chip.sha256 == sha256 && chip.size == size)
        })?;
        Some((entry.id.to_owned(), catalogue_entry(entry)))
    }
}

fn catalogue_entry(entry: &assembly::FirmwareEntry) -> CatalogueEntry {
    CatalogueEntry {
        size: entry.size,
        sha256: entry.sha256.to_owned(),
        license: match entry.distribution {
            assembly::FirmwareDistribution::Open { license, .. } => Some(license.to_owned()),
            assembly::FirmwareDistribution::UserSupplied => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_stay_within_what_the_importers_take() {
        for name in SOFTWARE_PLATFORMS {
            assert!(
                image_store::metadata::SOFTWARE_PLATFORMS.contains(name),
                "{name}"
            );
        }
        let floppies: Vec<&str> = assembly::descriptors::FLOPPY_TYPES
            .iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(&FORMATS[..4], floppies.as_slice());
        for format in FORMATS {
            assert!(crate::media_set::format(format).is_ok(), "{format}");
        }
        for (volume, floppy) in VOLUMES.iter().zip(FloppyFormat::ALL) {
            assert_eq!(volume.0, floppy);
        }
        for lane in LANES {
            assert!(machine_file::firmware_package::lane(lane).is_ok());
        }
    }

    #[test]
    fn sizes_are_exact_for_diskettes_and_whole_sectors_for_a_cd() {
        assert_eq!(PcRecipes.size_fault("720K", 737_280), None);
        assert_eq!(
            PcRecipes.size_fault("720K", 737_281).as_deref(),
            Some("a 720K medium is exactly 737280 bytes")
        );
        assert_eq!(PcRecipes.size_fault("iso-2048", 316_557_312), None);
        assert!(PcRecipes.size_fault("iso-2048", 0).is_some());
        assert!(PcRecipes.size_fault("iso-2048", 2049).is_some());
    }

    #[test]
    fn a_volume_takes_8_3_names_as_stored_and_folders_dos_can_enter() {
        let volume = PcRecipes.volume_profile("1.44M").unwrap();
        assert!(PcRecipes.volume_profile("iso-2048").is_none());
        for good in ["README.TXT", "LEVEL1/ATTACK.BIN", "A/B/C"] {
            assert_eq!(volume.path_fault(good), None, "{good}");
        }
        assert_eq!(
            volume.path_fault("LEVEL1/readme.txt").as_deref(),
            Some("readme.txt: the lower-case letter 'r' is not allowed: a directory entry stores upper case, so write the name as it is on the disk")
        );
        assert!(volume.path_fault("LONGFILENAME.TXT").is_some());
        assert!(volume.path_fault("CON").is_some());
        // Folders of eight characters: seven of them are 63 characters with
        // their backslashes, eight are 72.
        let deep = |n: usize| format!("{}X.TXT", "ABCDEFGH/".repeat(n));
        assert_eq!(volume.path_fault(&deep(7)), None);
        assert!(volume
            .path_fault(&deep(8))
            .is_some_and(|f| f.contains("72 characters long")));
        assert_eq!(volume.label_fault("KENSLAB"), None);
        assert!(volume.label_fault("TWELVE CHARS").is_some());
        assert!(volume.label_fault("Kenslab").is_some());
    }

    #[test]
    fn the_catalogue_gives_size_hash_and_licence() {
        let open = PcRecipes.entry("unemu-at-bios-1.0.2-unemu.3").unwrap();
        assert_eq!(open.size, 32768);
        assert_eq!(open.license.as_deref(), Some("GPL-3.0-or-later"));
        let private = PcRecipes.entry("ibm-5170-bios-1985-06-10").unwrap();
        assert_eq!(private.size, 65536);
        assert_eq!(private.license, None);
        assert!(PcRecipes.entry("nothing").is_none());
        // By its bytes: the whole ROM, or one chip with its size.
        let (id, _) = PcRecipes.identify(&open.sha256, 1).unwrap();
        assert_eq!(id, "unemu-at-bios-1.0.2-unemu.3");
        let u27 = "c8654ed7a922644db2b2cce54c540fcfa7cbbae8c514d7eb3dbfbb8187471bfd";
        let (id, _) = PcRecipes.identify(u27, 32768).unwrap();
        assert_eq!(id, "ibm-5170-bios-1985-06-10");
        assert!(PcRecipes.identify(u27, 32767).is_none());
    }
}
