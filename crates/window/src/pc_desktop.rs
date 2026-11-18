// SPDX-License-Identifier: AGPL-3.0-or-later
//! The PC's part of the desktop library: what the platform-neutral registry
//! and Remotes models (`library_model`) ask of the IBM PC/AT family, behind
//! the provider seam `library_model::DesktopPlatform`
//! (`backlog/docs/design/core-contract.md` §8.1.4).
//!
//! The PC core describes the platform, `pc-image` validates its artifacts,
//! restricts the blobs its firmware catalogue lists as proprietary and names
//! its media formats and payload types.

use image_store::{ConfigValidator, Digest, Manifest, Store};
use library_model::{DesktopPlatform, PlatformFacts};
use unemu_core_api::{Core, PlatformDescription};

/// The PC's payload media types and the names a transfer lists them by.
const PAYLOAD_LABELS: &[(&str, &str)] = &[
    (pc_image::CMOS, "CMOS"),
    (pc_image::DISK_ZERO, "disk base (zero)"),
];

/// The PC's part of the desktop library.
pub(crate) struct PcDesktop<'a> {
    validators: &'a [&'a dyn ConfigValidator],
    catalogue: &'a [assembly::FirmwareEntry],
}

impl<'a> PcDesktop<'a> {
    /// `validators` is the PC's provider list; `catalogue` is the firmware
    /// catalogue whose proprietary images restrict a push.
    pub(crate) fn new(
        validators: &'a [&'a dyn ConfigValidator],
        catalogue: &'a [assembly::FirmwareEntry],
    ) -> Self {
        Self {
            validators,
            catalogue,
        }
    }

    /// The PC's facts, for the frame and its dialogs.
    pub(crate) fn facts() -> PlatformFacts {
        PlatformFacts::of(&[&PcDesktop::new(&[], &[])])
    }
}

impl DesktopPlatform for PcDesktop<'_> {
    /// The one platform the PC core describes.
    fn description(&self) -> &'static PlatformDescription {
        &unemu_pc_core::PcCore.description().platforms[0]
    }

    fn validators(&self) -> Vec<&dyn ConfigValidator> {
        self.validators.to_vec()
    }

    fn restricted(&self, _: &Store, manifest: &Manifest) -> image_store::Result<Vec<Digest>> {
        pc_image::firmware::restricted(manifest, self.catalogue)
    }

    fn medium_format(&self, format: &toml::Table) -> Option<&'static str> {
        pc_image::media_set::format_name(format).ok()
    }

    fn payload_labels(&self) -> &'static [(&'static str, &'static str)] {
        PAYLOAD_LABELS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The neutral models read the platform from the PC core's description:
    /// it must name the id and the name the PC's images carry.
    #[test]
    fn the_pc_cores_platform_is_the_one_pc_images_name() {
        let platform = PcDesktop::new(&[], &[]).description();
        assert_eq!(platform.id, pc_image::PLATFORM);
        assert_eq!(platform.name, pc_image::PLATFORM_NAME);
        let facts = PcDesktop::facts();
        assert!(facts.runs(pc_image::PLATFORM));
        assert_eq!(facts.family(), pc_image::PLATFORM_NAME);
    }

    /// A transfer names the PC's own payload blobs as it did before the
    /// names came through the platform, compressed ones included.
    #[test]
    fn pc_payload_blobs_keep_their_names_in_a_transfer() {
        let facts = PcDesktop::facts();
        let blob = |media_type: &str| {
            image_store::Descriptor::new(media_type, Digest::of(media_type.as_bytes()), 1)
        };
        let label =
            |media_type: &str| crate::registry_transfer::kind_label(&facts, &blob(media_type));
        assert_eq!(label(pc_image::CMOS), "CMOS");
        assert_eq!(label(pc_image::DISK_ZERO), "disk base (zero)");
        assert_eq!(label(&format!("{}+zstd", pc_image::CMOS)), "CMOS, zstd");
        assert_eq!(
            label("application/vnd.example.other"),
            "application/vnd.example.other"
        );
    }
}
