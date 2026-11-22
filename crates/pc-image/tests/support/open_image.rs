// SPDX-License-Identifier: AGPL-3.0-or-later
//! Open firmware packages and new empty images for zero-base tests.
//! Only the fetched open fixtures are used: the open AT BIOS, the open VGA
//! BIOS and XTIDE Universal BIOS.
use image_store::Store;
use pc_image::create::{Chs, Hardware, NewImage, Package, PackageRom, Serial, Video};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

pub use pc_harness::test_support::fixtures;
static NEXT: AtomicU64 = AtomicU64::new(0);
/// The source archive the open entry's fetch script builds and the catalogue
/// pins, so the copyleft package is open together with its source.
pub fn source_archive(candidate: &pc_image::firmware_import::Candidate) -> &'static str {
    match candidate.distribution {
        assembly::FirmwareDistribution::Open { fetch, .. } => match fetch {
            "vendor/unemu-at-bios/fetch.sh" => {
                "vendor/unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar"
            }
            "vendor/vgabios/fetch.sh" => "vendor/vgabios/vgabios-0.9d-unemu.1-src.tar",
            "vendor/xtide-universal-bios/fetch.sh" => {
                "vendor/xtide-universal-bios/xtide-universal-bios-r638-src.tar"
            }
            other => panic!("no source archive for {other}"),
        },
        assembly::FirmwareDistribution::UserSupplied => panic!("not open firmware"),
    }
}
pub struct Open {
    pub path: PathBuf,
    pub store: Store,
    pub packages: Vec<Package>,
}
impl Open {
    /// `None` when a fetched ROM or source archive is missing, which skips
    /// the test.
    pub fn new() -> Option<Self> {
        let mut roms = Vec::new();
        for (from, to) in [
            ("unemu-at-bios/bios-unemu-at.bin", "bios.bin"),
            ("vgabios/vgabios-std.bin", "vga.bin"),
            (
                "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
                "xtide.bin",
            ),
        ] {
            roms.push((fixtures::fixture(from)?, to));
        }
        for archive in [
            "unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar",
            "vgabios/vgabios-0.9d-unemu.1-src.tar",
            "xtide-universal-bios/xtide-universal-bios-r638-src.tar",
        ] {
            fixtures::fixture(archive)?;
        }
        let path = std::env::temp_dir().join(format!(
            "zero-base-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("roms")).unwrap();
        let root = pc_harness::test_support::root();
        for (bytes, to) in roms {
            fs::write(path.join("roms").join(to), bytes).unwrap();
        }
        let store = Store::open(path.join("store")).unwrap().with_compatibility(
            pc_image::compat::compatibility(image_store::compat::EXTRACTION_BASELINE),
        );
        let catalogue = pc_image::firmware::catalogue();
        let scan = pc_image::firmware_import::scan(&[path.join("roms")], catalogue).unwrap();
        assert_eq!(scan.candidates.len(), 3, "{:?}", scan.issues);
        for candidate in &scan.candidates {
            let statement = pc_image::firmware_import::Statement {
                source: Some(root.join(source_archive(candidate))),
                ..Default::default()
            };
            pc_image::firmware_import::publish_with(&store, candidate, &[], &statement, catalogue)
                .unwrap();
        }
        let packages = pc_image::create::packages(&store).unwrap();
        Some(Self {
            path,
            store,
            packages,
        })
    }
    pub fn rom(&self, catalogue_id: &str) -> PackageRom {
        for package in &self.packages {
            for rom in &package.config.roms {
                if rom.catalogue_id.as_deref() == Some(catalogue_id) {
                    return PackageRom::new(package, rom);
                }
            }
        }
        panic!("no package for {catalogue_id}")
    }
    pub fn image(&self, disks: [Option<Chs>; 2]) -> NewImage {
        NewImage {
            hardware: Hardware {
                board: assembly::BoardModel::Ibm5170,
                rate: (8, 13),
                fpu: false,
                conventional_kib: 640,
                extended_kib: 1024,
                video: Video::Vga,
                sound: None,
                serial: Serial::Empty,
                ems: None,
                floppies: [Some("1.2M".into()), None],
                ide_port: 0x1F0,
                disks,
            },
            firmware: vec![
                self.rom("unemu-at-bios-1.0.2-unemu.3"),
                self.rom("vgabios-0.9d-unemu.1-std"),
                self.rom("xub-r638-xtp-ata1f0"),
            ],
            title: Some("Open AT workbench".into()),
            description: None,
        }
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
