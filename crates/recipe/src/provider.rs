// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the platform provider of a recipe's `platform` supplies to the
//! checks (recipes §10): media formats and the sizes they hold, a volume
//! profile per format a `fat` step can build, the rule for DOS names of
//! extras, firmware roles, lanes, boards and the catalogue, the needs
//! vocabulary, and the software platforms and `[ids]` catalogues a title may
//! name.
//!
//! The engine never names a platform: everything here is data the provider
//! answers. Each provider's vocabularies are append-only, as published
//! recipes name them (recipes §9).

use image_store::needs::Vocabulary;

/// The provider of one platform family.
pub trait Provider {
    /// The platform id it routes on (`ibm-pc`), current, never an alias.
    fn platform(&self) -> &str;
    fn media(&self) -> &dyn MediaProfile;
    fn firmware(&self) -> &dyn FirmwareProfile;
    /// The needs vocabulary; `None` when titles of the platform state no
    /// needs.
    fn needs(&self) -> Option<&dyn Vocabulary>;
    /// The values of a title's `software_platform` (recipes §6.4).
    fn software_platforms(&self) -> &[&str];
    /// The catalogues whose ids `[ids]` may hold (recipes §6.8).
    fn work_id_catalogues(&self) -> &[&str];
}

/// Media formats and what they hold.
pub trait MediaProfile {
    /// Every media format name of a `[[medium]]` (`1.44M`, `iso-2048`).
    fn formats(&self) -> &[&str];
    /// Why a medium of `format` cannot be `size` bytes, or `None`. `format`
    /// is one of [`Self::formats`].
    fn size_fault(&self, format: &str, size: u64) -> Option<String>;
    /// The volume profile of a format a `fat` step builds, or `None` when
    /// the format holds no FAT volume.
    fn volume_profile(&self, format: &str) -> Option<&dyn VolumeProfile>;
    /// Why `name` is not a DOS name an extra may carry, or `None`.
    fn dos_name_fault(&self, name: &str) -> Option<String>;
}

/// How a file system step lays out a volume of one format: here, which
/// names and labels it takes. The step refuses a bad name and never renames
/// (recipes §6.9).
pub trait VolumeProfile {
    /// Why the volume cannot hold a file at `path` (`/` separators), or
    /// `None`. A folder above it counts as well as the file's own name.
    fn path_fault(&self, path: &str) -> Option<String>;
    /// Why `label` cannot be the volume label, or `None`.
    fn label_fault(&self, label: &str) -> Option<String>;
}

/// One entry of the provider's firmware catalogue, as the checks need it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueEntry {
    /// The size of the whole ROM, or of the chips assembled.
    pub size: u64,
    /// Its lower-case SHA-256.
    pub sha256: String,
    /// The SPDX expression of open firmware; `None` when the catalogue
    /// lists the ROM as user-supplied.
    pub license: Option<String>,
}

/// Firmware rules of a provider.
pub trait FirmwareProfile {
    /// The roles of a `[[rom]]` (`system-bios`).
    fn roles(&self) -> &[&str];
    /// The lanes of a ROM's chip (`even`, `odd`).
    fn lanes(&self) -> &[&str];
    /// The boards a package may fit.
    fn boards(&self) -> &[&str];
    /// The machine classes a package may fit.
    fn machine_classes(&self) -> &[&str];
    /// The catalogue entry `id`, or `None` when the catalogue has none.
    fn entry(&self, id: &str) -> Option<CatalogueEntry>;
    /// The catalogue entry, and its id, that bytes of this SHA-256 and size
    /// belong to: the whole ROM has this SHA-256 (rule 10 then compares the
    /// size), or one chip of one of its layouts has this SHA-256 and size.
    /// A ROM is known by its bytes, whatever id a recipe gives it or not.
    fn identify(&self, sha256: &str, size: u64) -> Option<(String, CatalogueEntry)>;
}

/// The providers a check may ask, found by the platform a recipe names.
#[derive(Clone, Copy)]
pub struct Providers<'a>(pub &'a [&'a dyn Provider]);

impl<'a> Providers<'a> {
    /// The provider of `platform`; a retired id finds the provider of its
    /// replacement.
    pub fn get(&self, platform: &str) -> Option<&'a dyn Provider> {
        self.0
            .iter()
            .copied()
            .find(|p| image_store::platform::same(p.platform(), platform))
    }

    /// The platform ids known, for a message.
    pub fn platforms(&self) -> Vec<&'a str> {
        self.0.iter().map(|p| p.platform()).collect()
    }
}
