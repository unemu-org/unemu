// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC firmware package provider; machine-images §5.3.
//! IBM Personal Computer AT Technical Reference, Section 1, System Board ROM:
//! logical BIOS packages preserve even/odd socket lanes and assembled offsets.

pub use crate::firmware_package::{
    firmware_sources, lane, prepare_for_assembly_with_compatibility,
    prepare_for_assembly_with_firmware_store, prepare_for_assembly_with_store_root, reconstructed,
    role, Fit, PcFirmwareRules, StorePackages, BOARDS, MACHINE_CLASSES,
};

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

/// The blobs of `manifest` that may never be redistributed, whatever marks
/// they carry: each one whose bytes, stored or decoded, `catalogue` lists as
/// a proprietary image or chip ([`proprietary`]). The platform-neutral push
/// guard has no catalogue, so the frontends hand it these digests; a pulled
/// package that marks such a ROM open still cannot leave a private registry.
pub fn restricted(
    manifest: &image_store::Manifest,
    catalogue: &[assembly::FirmwareEntry],
) -> image_store::Result<Vec<image_store::Digest>> {
    let mut digests = Vec::new();
    for d in std::iter::once(&manifest.config).chain(&manifest.layers) {
        let mut identities = vec![(d.digest.clone(), d.size)];
        if d.is_compressed() {
            identities.push((d.decoded_digest()?, d.decoded_size()?));
        }
        for (digest, size) in identities {
            if proprietary(catalogue, digest.hex(), size).is_some() {
                digests.push(digest);
            }
        }
    }
    Ok(digests)
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
    fn restricted_names_every_blob_whose_stored_or_decoded_bytes_are_proprietary() {
        use image_store::{codec, media_type as mt, Descriptor, Digest, Manifest};
        let (rom, other) = (
            b"synthetic proprietary rom".as_slice(),
            b"open rom".as_slice(),
        );
        let entry = |bytes: &[u8], distribution| assembly::FirmwareEntry {
            id: "synthetic-option-rom",
            kind: assembly::FirmwareKind::OptionRom,
            sha256: Box::leak(Digest::of(bytes).hex().to_owned().into_boxed_str()),
            size: bytes.len() as u64,
            distribution,
            ..assembly::KISELEV_XI8088
        };
        let plain = Descriptor::new(mt::FIRMWARE, Digest::of(rom), rom.len() as u64);
        let mut compressed = Descriptor::new(
            format!("{}{}", mt::FIRMWARE, codec::SUFFIX),
            Digest::of(b"frame"),
            5,
        );
        compressed
            .annotations
            .insert(codec::DECODED_DIGEST.into(), Digest::of(rom).to_string());
        compressed
            .annotations
            .insert(codec::DECODED_SIZE.into(), rom.len().to_string());
        let open = Descriptor::new(mt::FIRMWARE, Digest::of(other), other.len() as u64);
        let config = Descriptor::new(mt::FIRMWARE_CONFIG, Digest::of(b"config"), 6);
        let manifest =
            Manifest::new_firmware("ibm-pc", config, vec![plain, compressed, open]).unwrap();
        let user_supplied = [entry(rom, assembly::FirmwareDistribution::UserSupplied)];
        assert_eq!(
            restricted(&manifest, &user_supplied).unwrap(),
            vec![Digest::of(rom), Digest::of(rom)],
            "the plain layer by its digest, the compressed one by its decoded digest"
        );
        let open_entry = [entry(rom, assembly::KISELEV_XI8088.distribution)];
        assert!(restricted(&manifest, &open_entry).unwrap().is_empty());
        assert!(restricted(&manifest, &[]).unwrap().is_empty());
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
