// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the details panes say about an image's hardware, drives and
//! firmware: the PC library's model (`pc_library::hardware`), re-exported
//! under its old path. Its tests stay here, since they read images through
//! the library worker and its fixture.

pub(crate) use pc_library::hardware::{
    controller_name, letter, medium_name, DriveFact, FirmwareFact, ImageFacts, CMOS,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_test_support::{FirmwareStore, Fixture};
    use crate::library_view::firmware::is_rom;
    use crate::library_view::ImageRow;
    use crate::library_worker::CheckedImage;
    use crate::library_worker::{checked, Event};
    use image_store::{license, summary::ImageSummary, Descriptor, Digest};
    use image_store::{license::Expression, media_type as mt};
    use pc_image::inspection::{ImageDetails, MediumDetails};
    use std::collections::BTreeMap;

    fn checked_image(store: &image_store::Store, row: &ImageRow) -> (ImageDetails, CheckedImage) {
        match checked(store, row, None) {
            Event::Checked(_, Ok(details), Some(checked)) => (details, *checked),
            _ => panic!("the test image checks"),
        }
    }
    /// The image's summary as the library caches it.
    fn summary(store: &image_store::Store, row: &ImageRow) -> ImageSummary {
        super::super::firmware::tests::warm(store)
            .summary(&row.descriptor.digest)
            .cloned()
            .expect("a summary")
    }
    fn rom(bytes: &[u8], open: bool) -> Descriptor {
        let mut d = Descriptor::new(mt::FIRMWARE, Digest::of(bytes), bytes.len() as u64);
        let mit = Expression::parse("MIT").unwrap();
        license::mark(&mut d, open.then_some(&mit));
        d
    }
    fn details(firmware: Vec<String>, media: Vec<MediumDetails>) -> ImageDetails {
        ImageDetails {
            media,
            slots: Vec::new(),
            firmware,
            hardware: Vec::new(),
            stored_bytes: 0,
            exclusive_bytes: 0,
            dedup_bytes: 0,
        }
    }

    /// Drives by letter with their type and media, CMOS last; the board,
    /// processor, memory and cards in the machines mock-up's words; the
    /// BIOS by its library package's name with its mark; no configuration
    /// key in any of it.
    #[test]
    fn facts_name_drives_hardware_and_firmware_as_the_mockup_does() {
        let f = Fixture::new();
        let (details, checked) = checked_image(&f.store, &f.row);
        let summary = summary(&f.store, &f.row);
        let bios = summary
            .storage_blobs
            .iter()
            .find(|d| is_rom(d))
            .and_then(|d| d.decoded_digest().ok())
            .expect("the image's BIOS layer");
        let names = BTreeMap::from([(bios, "Example BIOS".to_owned())]);
        let facts = ImageFacts::new(&details, Some(&checked), Some(&summary), &names);
        let drives: Vec<&str> = facts.drives.iter().map(|d| d.drive.as_str()).collect();
        assert_eq!(drives, ["A:", "C:", "CMOS"]);
        assert_eq!(
            facts.drives[0].described,
            "360K · catalogue: one.img, two.img"
        );
        assert!(
            facts.drives[1]
                .described
                .starts_with("16.0 KiB disk (1/1/32)"),
            "{:?}",
            facts.drives[1]
        );
        assert_eq!(facts.drives[2].described, "Stored, 64 B");
        assert!(
            facts
                .hardware
                .starts_with("IBM 5170-class board · Intel 80286, 6 MHz (8/13) · 640 KiB"),
            "{}",
            facts.hardware
        );
        assert!(facts.hardware.contains("ATA at 1F0h"), "{}", facts.hardware);
        assert!(!facts.hardware.contains("Floppy"), "{}", facts.hardware);
        let open = license::open_license(summary.storage_blobs.iter().find(|d| is_rom(d)).unwrap())
            .is_some();
        assert_eq!(
            facts.firmware,
            [FirmwareFact {
                role: "System BIOS".into(),
                name: "Example BIOS".into(),
                open: Some(open),
            }]
        );
        let text = format!("{facts:?}");
        for raw in ["ide:", "fdc-765:", "medium-", "firmware.", "ibm-5170"] {
            assert!(!text.contains(raw), "{raw} in {text}");
        }
        // Before the summary is read, the ROM is named by its digest's start.
        let early = ImageFacts::new(&details, Some(&checked), None, &names);
        let hex = details.firmware[0].split_once(" · ").unwrap().1;
        assert_eq!(early.firmware[0].name, format!("ROM {hex}"));
        assert_eq!(early.firmware[0].open, None);
    }

    /// The open template image names its system BIOS, video BIOS and option
    /// ROM from the board catalogue, each marked open.
    #[test]
    fn catalogue_roms_get_their_roles_names_and_open_marks() {
        let Some(store) = FirmwareStore::new(false) else {
            return;
        };
        let row = crate::image_settings::tests::open_image(&store);
        let (details, checked) = checked_image(&store.store, &row);
        let summary = summary(&store.store, &row);
        let facts = ImageFacts::new(&details, Some(&checked), Some(&summary), &BTreeMap::new());
        let roles: Vec<&str> = facts.firmware.iter().map(|f| f.role.as_str()).collect();
        assert_eq!(
            roles,
            ["System BIOS", "Video BIOS", "Option ROM"],
            "{facts:?}"
        );
        for rom in &facts.firmware {
            assert_eq!(rom.open, Some(true), "{rom:?}");
            assert!(!rom.name.starts_with("ROM "), "{rom:?}");
        }
        assert!(facts.firmware[0].name.contains("BIOS"), "{facts:?}");
    }

    /// A ROM in two chips is open only when both chips are.
    #[test]
    fn a_two_chip_rom_is_open_only_when_every_chip_is() {
        let f = Fixture::new();
        let mut summary = summary(&f.store, &f.row);
        let even = [0x31u8; 16];
        let odd = [0x72u8; 16];
        let lines = |a: &[u8], b: &[u8]| {
            vec![
                format!("firmware.bios.parts[0] · {}", &Digest::of(a).hex()[..12]),
                format!("firmware.bios.parts[1] · {}", &Digest::of(b).hex()[..12]),
            ]
        };
        let names = BTreeMap::new();
        for (open_even, open_odd, open) in [(true, true, true), (true, false, false)] {
            summary.storage_blobs = vec![rom(&even, open_even), rom(&odd, open_odd)];
            let facts = ImageFacts::new(
                &details(lines(&even, &odd), Vec::new()),
                None,
                Some(&summary),
                &names,
            );
            assert_eq!(facts.firmware.len(), 1, "{facts:?}");
            assert_eq!(facts.firmware[0].role, "System BIOS");
            assert_eq!(facts.firmware[0].open, Some(open));
        }
    }

    /// Without a draft, a drive's letter follows its card's kind, not its id.
    #[test]
    fn without_a_draft_letters_follow_the_card_kind() {
        let medium = |drive: &str| MediumDetails {
            id: "medium-0".into(),
            drive: drive.into(),
            name: "medium-0".into(),
            policy: pc_image::Policy::Journal,
            chain: Vec::new(),
            working_sectors: Some(0),
        };
        let image = details(Vec::new(), vec![medium("disk:0"), medium("floppies:1")]);
        let checked = CheckedImage {
            cards: vec![
                ("disk".into(), "ide".into()),
                ("floppies".into(), "fdc-765".into()),
            ],
            ..CheckedImage::default()
        };
        let facts = ImageFacts::new(&image, Some(&checked), None, &BTreeMap::new());
        let drives: Vec<&str> = facts.drives.iter().map(|d| d.drive.as_str()).collect();
        assert_eq!(drives, ["B:", "C:", "CMOS"]);
        assert_eq!(facts.drives[1].described, "disk 1");
        assert!(!facts.hardware.contains("Board:"), "{}", facts.hardware);
    }

    /// A ROM whose bytes the firmware catalogue lists as proprietary stays
    /// user-supplied whatever its layer's mark says, as the push guard and
    /// the settings sheet rule; an unlisted ROM marked open is open.
    #[test]
    fn a_rom_the_catalogue_lists_as_proprietary_is_never_open() {
        let f = Fixture::new();
        let mut summary = summary(&f.store, &f.row);
        let entry = pc_image::firmware::catalogue()
            .iter()
            .find(|e| e.distribution == assembly::FirmwareDistribution::UserSupplied)
            .expect("a proprietary catalogue ROM");
        let digest: Digest = format!("sha256:{}", entry.sha256).parse().unwrap();
        let mut marked = Descriptor::new(mt::FIRMWARE, digest, entry.size);
        license::mark(&mut marked, Some(&Expression::parse("MIT").unwrap()));
        for (layer, open) in [(marked, false), (rom(&[0x5a; 16], true), true)] {
            let hex = layer.decoded_digest().unwrap().hex()[..12].to_owned();
            summary.storage_blobs = vec![layer];
            let facts = ImageFacts::new(
                &details(vec![format!("firmware.bios · {hex}")], Vec::new()),
                None,
                Some(&summary),
                &BTreeMap::new(),
            );
            assert_eq!(facts.firmware.len(), 1, "{facts:?}");
            assert_eq!(facts.firmware[0].open, Some(open), "{facts:?}");
        }
    }
}
