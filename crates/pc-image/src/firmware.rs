// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC firmware package provider; machine-images §5.3.
//! IBM Personal Computer AT Technical Reference, Section 1, System Board ROM:
//! logical BIOS packages preserve even/odd socket lanes and assembled offsets.

pub use machine_file::firmware_package::{role, Fit, PcFirmwareRules, BOARDS, MACHINE_CLASSES};

/// Storage roles include controllers which are not yet executable by the machine.
pub const ROLES: &[&str] = &[
    "system-bios",
    "option-rom",
    "video-bios",
    "keyboard-controller",
    "other",
];

/// Device kinds a ROM can require, one per catalogue board part
/// (`assembly::FirmwarePart`): the keyboard controller, CMOS clock and
/// interrupt controllers a system BIOS drives, and the adapter an option ROM
/// or video BIOS drives.
pub const DEVICE_KINDS: &[&str] = &[
    "kbc-8042",
    "rtc-cmos",
    "rtc-cmos-128",
    "pic-cascade",
    "video-rom",
    "ata-primary",
    "vga-registers",
];

/// The package id of a catalogue board-part requirement.
pub fn device_kind(part: assembly::FirmwarePart) -> &'static str {
    use assembly::FirmwarePart::*;
    match part {
        Kbc8042 => "kbc-8042",
        RtcCmos => "rtc-cmos",
        RtcCmos128 => "rtc-cmos-128",
        PicCascade => "pic-cascade",
        VideoRom => "video-rom",
        AtaPrimary => "ata-primary",
        VgaRegisters => "vga-registers",
    }
}

/// Human-readable name of a PC ROM role, for the generated README.
pub fn role_label(role: &str) -> &str {
    match role {
        "system-bios" => "System BIOS",
        "option-rom" => "Option ROM",
        "video-bios" => "Video BIOS",
        "keyboard-controller" => "Keyboard controller",
        "other" => "Other",
        other => other,
    }
}

/// Licence of the generated README and of the package config when every
/// other layer is open: both hold only descriptive facts and identities.
pub const METADATA_LICENSE: &str = "CC0-1.0";

/// The catalogue entry the user-supplied (proprietary) bytes belong to: its
/// whole image or any chip of any of its layouts has this SHA-256 and length.
/// Rights follow the bytes, so no id, name or mark can open them.
pub fn proprietary<'a>(
    catalogue: &'a [assembly::FirmwareEntry],
    sha256: &str,
    size: u64,
) -> Option<&'a assembly::FirmwareEntry> {
    catalogue.iter().find(|e| {
        e.distribution == assembly::FirmwareDistribution::UserSupplied
            && ((e.sha256 == sha256 && e.size == size)
                || e.parts
                    .iter()
                    .flat_map(|layout| layout.iter())
                    .any(|p| p.sha256 == sha256 && p.size == size))
    })
}

/// Production catalogue; callers may inject synthetic identities for isolation tests.
pub fn catalogue() -> &'static [assembly::FirmwareEntry] {
    assembly::BoardModel::Ibm5170.firmware_catalogue()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_catalogue_board_part_has_a_device_kind_in_the_vocabulary() {
        for entry in catalogue() {
            for part in entry.requires {
                assert!(DEVICE_KINDS.contains(&device_kind(*part)), "{part}");
            }
        }
        for kind in DEVICE_KINDS {
            assert!(image_store::media_set::valid_id(kind), "{kind}");
        }
    }
    #[test]
    fn generated_content_is_open_under_a_valid_licence() {
        let d = crate::generated(image_store::Descriptor::new(
            image_store::media_type::CONFIG,
            image_store::Digest::of(b"config"),
            6,
        ));
        assert_eq!(
            image_store::license::open_license(&d).unwrap().as_str(),
            METADATA_LICENSE
        );
    }
}
