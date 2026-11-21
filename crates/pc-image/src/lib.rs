// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM PC/AT OCI bundle import and validation; machine-images §§1–2, 4, 5 and 8.
//! The stored config's model, its decode and encode, the frozen defaults and
//! the codec half of validation live in `pc_config::image`, below the
//! contract; this crate keeps the store's side (descriptors, kinds, size
//! limits, layer chains, the closure) and calls the codec to validate
//! (core-contract.md §14.3). Hardware parsing and catalogue checks
//! remain in pc-config, machine-file and assembly.
//! The platform-neutral store never depends on this provider.

pub mod compat;
pub mod create;
pub mod desktop;
mod disk;
pub mod firmware;
pub mod firmware_import;
mod firmware_package;
mod firmware_readme;
mod import;
pub mod inspection;
pub mod launch;
pub mod machine;
pub mod media_create;
pub mod media_import;
pub mod media_set;
pub mod needs;
mod provider;
pub mod recipe;
pub mod recipe_build;
pub mod revision;
pub mod template;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod workspace;
pub mod zero;

pub use firmware_package::PLATFORM;
pub use import::{import_machine, ChosenGeneration, ImportOptions, ImportRecord, RecoveryChoice};
pub use provider::{compress_image, PcProvider, CMOS, DISK_ZERO, PAYLOAD_TYPES};

const MEDIA_SETS: image_store::media_set::Validator<'static> =
    image_store::media_set::Validator(&media_set::PcMediaFormats);

/// Run `job` with every image provider of this platform: machine images,
/// media sets and firmware packages, each rating an artifact with the PC rows
/// of the compatibility table, which the push guard of `unemu/` checks. The
/// command line's PC platform and the programs' list of every platform
/// compiled in both take the PC's providers from here.
pub fn with_providers<T>(job: impl FnOnce(&[&dyn image_store::ConfigValidator]) -> T) -> T {
    let firmware_rules = firmware::PcFirmwareRules(firmware::catalogue());
    let firmware = image_store::firmware::Validator(&firmware_rules);
    job(&[
        &PcProvider,
        &compat::WithPcRows(&MEDIA_SETS),
        &compat::WithPcRows(&firmware),
    ])
}

/// What a person reads for this platform, whichever of its ids an artifact
/// names.
pub const PLATFORM_NAME: &str = "IBM PC compatible";

/// The namespace of open content, which the official registry takes: the
/// templates, and the sets and packages whose every blob is open.
pub const OPEN_NAMESPACE: &str = "unemu";
/// The namespace of user-supplied content, which only private registries
/// take (recipes.md §7.3).
pub const PRIVATE_NAMESPACE: &str = "private";

/// The repository of an installation media set of `work` in `namespace`:
/// `unemu/media/<platform>/<work>` for open sets, `private/media/…` for
/// user-supplied ones (recipes.md §8.3).
pub fn media_repository(namespace: &str, work: &str) -> String {
    [namespace, "media", PLATFORM, work].join("/")
}

/// The repository of the firmware package of the catalogue entry
/// `catalogue_id` in `namespace`: `unemu/firmware/<platform>/<catalogue id>`
/// for open packages, `private/firmware/…` for user-supplied ones
/// (recipes.md §8.3).
pub fn firmware_repository(namespace: &str, catalogue_id: &str) -> String {
    [namespace, "firmware", PLATFORM, catalogue_id].join("/")
}

/// Whether `id` names this provider's platform: [`PLATFORM`] or its retired
/// alias.
pub fn is_pc(id: &str) -> bool {
    image_store::platform::same(id, PLATFORM)
}

/// A platform id as a person reads it: [`PLATFORM_NAME`] for this platform,
/// any other id as it is.
pub fn platform_name(id: &str) -> &str {
    if is_pc(id) {
        PLATFORM_NAME
    } else {
        id
    }
}

use image_store::{Descriptor, Digest};
pub use pc_config::image::Policy;
use pc_config::image::{
    defaults, encode, Config, Firmware, Geometry, Medium, Nvram, NvramMode, Slot,
};
use serde::{Deserialize, Serialize};

/// Import and bundle validation failures, preserving source diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] image_store::Error),
    #[error(transparent)]
    Load(#[from] machine_file::LoadError),
    #[error(transparent)]
    Lower(#[from] machine_file::LowerError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Journal(#[from] journal_files::offline::ToolError),
    #[error("missing inputs:\n{paths}")]
    MissingInputs { paths: String, firmware: bool },
    #[error("invalid PC image: {0}")]
    Invalid(String),
}

impl Error {
    /// A firmware locator can act on these failures without parsing diagnostics.
    pub fn needs_firmware(&self) -> bool {
        match self {
            Self::MissingInputs { firmware, .. } => *firmware,
            Self::Load(machine_file::LoadError::Lower(e)) | Self::Lower(e) => {
                e.field.starts_with("firmware.")
            }
            _ => false,
        }
    }
}
impl From<pc_config::image::Error> for Error {
    /// The codec's refusals keep the variants and words they had here.
    fn from(error: pc_config::image::Error) -> Self {
        match error {
            pc_config::image::Error::Parse(e) => Self::Load(e.into()),
            pc_config::image::Error::Lower(e) => Self::Lower(e),
            pc_config::image::Error::Invalid(message) => Self::Invalid(message),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;
/// The metadata blob of a new machine: schema, platform, no labels or titles.
pub(crate) const EMPTY_METADATA: &[u8] =
    br#"{"schema":1,"platform":"ibm-pc","labels":[],"titles":[]}"#;
/// Mark content generated by this program from the user's own settings, such
/// as a machine configuration or catalogue metadata, as open: it never holds
/// vendor bytes.
/// Content this program generates (configuration, metadata, zero bases)
/// holds no third-party work: open under the generated-metadata licence, as
/// an open mark needs a licence.
pub(crate) fn generated(mut descriptor: Descriptor) -> Descriptor {
    use image_store::license::{LICENSE, OPEN, REDISTRIBUTION};
    descriptor
        .annotations
        .insert(REDISTRIBUTION.into(), OPEN.into());
    descriptor
        .annotations
        .insert(LICENSE.into(), firmware::METADATA_LICENSE.into());
    descriptor
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
fn decode(table: toml::Table) -> Result<Config> {
    Ok(Config::decode(table)?)
}

fn descriptor(descriptors: &[Descriptor], digest: &Digest, kind: &str) -> Result<Descriptor> {
    descriptors
        .iter()
        .find(|d| d.names(digest) && d.decoded_media_type() == kind)
        .cloned()
        .ok_or_else(|| invalid(format!("missing {kind} descriptor for {digest}")))
}

fn firmware_options() -> assembly::AssemblyOptions {
    #[cfg(feature = "test-support")]
    let allow_unlisted_firmware = test_support::permitted();
    #[cfg(not(feature = "test-support"))]
    let allow_unlisted_firmware = false;
    assembly::AssemblyOptions {
        allow_unlisted_firmware,
    }
}
fn validate_bios(config: &assembly::MachineConfig) -> Result<()> {
    Ok(pc_config::image::validate_bios(config, firmware_options())?)
}
/// The stored config's walk over the hardware's firmware ROMs, failing with
/// this crate's errors.
fn firmware_images(
    table: &mut toml::Table,
    visit: impl FnMut(&str, &mut toml::Table) -> Result<()>,
) -> Result<()> {
    pc_config::image::firmware_images(table, visit)
}
/// The stored config's walk over the hardware's drive tables, failing with
/// this crate's errors.
fn drive_tables(
    machine: &mut toml::Table,
    visit: impl FnMut(&str, &str, &mut toml::Table) -> Result<()>,
) -> Result<()> {
    pc_config::image::drive_tables(machine, visit)
}
fn unit(kind: &str, drive: &toml::Table) -> Result<u8> {
    Ok(pc_config::image::unit(kind, drive)?)
}

/// The store's digest for a digest of the stored config: the same text form.
pub(crate) trait StoreDigest {
    fn store(&self) -> Result<Digest>;
}
impl StoreDigest for pc_config::image::Digest {
    fn store(&self) -> Result<Digest> {
        Ok(self.to_string().parse()?)
    }
}
/// The stored config's digest for a digest of the store: the same text form.
pub(crate) trait ConfigDigest {
    fn to_config(&self) -> Result<pc_config::image::Digest>;
}
impl ConfigDigest for Digest {
    fn to_config(&self) -> Result<pc_config::image::Digest> {
        self.to_string()
            .parse()
            .map_err(|e: pc_config::image::DigestError| invalid(e.to_string()))
    }
}

#[cfg(test)]
mod desktop_error_tests {
    use super::*;
    #[test]
    fn firmware_handoff_uses_error_kind_instead_of_diagnostic_words() {
        assert!(!Error::Invalid("firmware BIOS ROM is missing".into()).needs_firmware());
        assert!(Error::MissingInputs {
            paths: "input absent".into(),
            firmware: true
        }
        .needs_firmware());
        assert!(!Error::MissingInputs {
            paths: "bios firmware name used by a disk".into(),
            firmware: false
        }
        .needs_firmware());
    }
}
