// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless desktop sessions use synthetic firmware, disks, catalogue and CMOS.
use crate::library_view::ImageRow;
use image_store::Store;
use std::fs;
pub(crate) struct Fixture {
    pub temp: assembly::test_support::TempDir,
    pub store: Store,
    pub row: ImageRow,
}
impl Fixture {
    pub fn new() -> Self {
        let temp = assembly::test_support::TempDir::new("desktop-session");
        fs::write(temp.path().join("bios.bin"), [0xf4; 32768]).unwrap();
        fs::write(temp.path().join("disk.img"), [0; 16384]).unwrap();
        fs::write(temp.path().join("one.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(temp.path().join("two.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(temp.path().join("cmos.bin"), [0; 64]).unwrap();
        let path = temp.path().join("machine.toml");
        fs::write(&path,"schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nbase=0xF8000\nprovides=['floppy','fixed-disk']\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='cmos.upfj'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='one.img'\nwrite='journal'\njournal='one.upfj'\n[[card.drives.media]]\nimage='two.img'\nwrite='journal'\njournal='two.upfj'\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=32}\nimage='disk.img'\nwrite='journal'\njournal='disk.upfj'\n").unwrap();
        let store = Store::open(temp.path().join("store")).unwrap();
        let descriptor = pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::import_machine(&store, &path, "test:source", &Default::default())
        })
        .unwrap()
        .manifest;
        let row = ImageRow {
            reference: "test:source".into(),
            descriptor,
            summary: None,
            failure: None,
        };
        Self { temp, store, row }
    }
    pub fn change(&self, prepared: &pc_image::workspace::PreparedSession, value: u8) {
        let change = |source: &assembly::ImageSource, write: &assembly::WriteMode| {
            let assembly::ImageSource::File(path) = source else {
                panic!("expected file")
            };
            let assembly::WriteMode::Journal { path: journal, .. } = write else {
                panic!("expected journal")
            };
            // A compressed floppy has no file; its verified base holds the bytes.
            let bytes = match prepared.bases.get(path) {
                Some(base) if base.metadata().is_none() => {
                    let mut bytes = vec![0; media::SectorSource::byte_len(base) as usize];
                    media::SectorSource::read_range(base, 0, &mut bytes).unwrap();
                    bytes
                }
                _ => fs::read(path).unwrap(),
            };
            let mut media =
                media::JournalMedia::open(journal, &bytes, bytes.len() as u64, Default::default())
                    .unwrap();
            media.write_batch(&[(0, [value; 512])]);
            media.finish().unwrap();
        };
        for drive in &prepared.machine.drives {
            if let Some(list) = &drive.media {
                for entry in &list.entries {
                    change(&entry.image, &entry.write);
                }
            } else {
                change(&drive.image, &drive.write);
            }
        }
        for device in &prepared.machine.devices {
            if let assembly::DeviceKind::Rtc(rtc) = &device.kind {
                if let assembly::NvramSource::Journal { image, journal, .. } = &rtc.nvram {
                    let bytes = fs::read(image).unwrap();
                    let mut media =
                        media::JournalMedia::open(journal, &bytes, 64, Default::default()).unwrap();
                    let mut sector = [0; 512];
                    sector[..64].copy_from_slice(&bytes);
                    sector[32] = value;
                    media.write_batch(&[(0, sector)]);
                    media.finish().unwrap();
                }
            }
        }
    }
}

/// A store with the open firmware packages (the Kiselev AT BIOS, the open VGA
/// BIOS and XTIDE) and, with `misfits`, a user-supplied package of the other
/// Kiselev build plus synthetic packages that must not fit: an XT-class BIOS
/// and one naming no board.
pub(crate) struct FirmwareStore {
    pub temp: assembly::test_support::TempDir,
    pub store: Store,
}
impl FirmwareStore {
    pub fn new(misfits: bool) -> Self {
        let temp = assembly::test_support::TempDir::new("new-machine-firmware");
        let roms = temp.path().join("roms");
        fs::create_dir(&roms).unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (from, to) in [
            ("vendor/kiselev-bios/bios-xi8088-atreset.bin", "bios.bin"),
            ("vendor/vgabios/vgabios-std.bin", "vga.bin"),
            (
                "vendor/xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
                "xtide.bin",
            ),
        ] {
            fs::copy(root.join(from), roms.join(to)).unwrap();
        }
        let store = Store::open(temp.path().join("store")).unwrap();
        let catalogue = pc_image::firmware::catalogue();
        let scan = pc_image::firmware_import::scan(std::slice::from_ref(&roms), catalogue).unwrap();
        assert_eq!(scan.candidates.len(), 3, "{:?}", scan.issues);
        for candidate in &scan.candidates {
            pc_image::firmware_import::publish(&store, candidate, &[], catalogue).unwrap();
        }
        if misfits {
            fs::copy(
                root.join("vendor/kiselev-bios/bios-xi8088.bin"),
                temp.path().join("xi8088.bin"),
            )
            .unwrap();
            let bios = fs::read(temp.path().join("xi8088.bin")).unwrap();
            let digest = image_store::Digest::of(&bios);
            let synthetic = vec![0xf4; 32768];
            let other = image_store::Digest::of(&synthetic);
            fs::write(roms.join("synthetic.bin"), &synthetic).unwrap();
            for (tag, name, catalogue_id, fits, source, digest, distribution) in [
                (
                    "private-kiselev",
                    "Kiselev AT BIOS (private copy)",
                    "catalogue_id='kiselev-xi8088-1.0.2'\n",
                    "boards=['ibm-5170']\nmachine_classes=['at']\n",
                    "../xi8088.bin",
                    &digest,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
                (
                    "xt-bios",
                    "Synthetic XT BIOS",
                    "",
                    "boards=['ibm-5160']\nmachine_classes=['xt']\n",
                    "synthetic.bin",
                    &other,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
                (
                    "unlisted-bios",
                    "Unlisted 286 BIOS",
                    "",
                    "machine_classes=['at']\n",
                    "synthetic.bin",
                    &other,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
            ] {
                let config = image_store::firmware::Config::decode(
                    format!(
                        "schema=1\nplatform='ibm-pc-at'\nname='{name}'\nvendor='Synthetic vendor'\n[fits]\n{fits}[[roms]]\nid='system-bios'\nrole='system-bios'\n{catalogue_id}size=32768\nsha256='{}'\nblob='{}'\n",
                        digest.hex(),
                        digest
                    )
                    .as_bytes(),
                )
                .unwrap();
                let candidate = pc_image::firmware_import::Candidate {
                    catalogue_id: tag.into(),
                    config,
                    sources: vec![roms.join(source)],
                    distribution,
                };
                pc_image::firmware_import::publish(&store, &candidate, &[], catalogue).unwrap();
            }
        }
        Self { temp, store }
    }
    /// Packages, tags and restricted blobs, as the worker hands them to the wizard.
    pub fn choices(&self) -> crate::library_worker::StoreChoices {
        crate::library_worker::store_choices(&self.store).unwrap()
    }
}
