// SPDX-License-Identifier: AGPL-3.0-or-later
//! Open firmware packages and new empty images for zero-base tests.
//! Only the fetched open fixtures are used: the Kiselev AT BIOS, the open VGA
//! BIOS and XTIDE Universal BIOS.
use image_store::Store;
use pc_image::create::{Chs, Hardware, NewImage, Package, PackageRom, Serial, Video};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Open {
    pub path: PathBuf,
    pub store: Store,
    pub packages: Vec<Package>,
}
impl Open {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zero-base-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("roms")).unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (from, to) in [
            ("vendor/kiselev-bios/bios-xi8088-atreset.bin", "bios.bin"),
            ("vendor/vgabios/vgabios-std.bin", "vga.bin"),
            (
                "vendor/xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
                "xtide.bin",
            ),
        ] {
            fs::copy(root.join(from), path.join("roms").join(to)).unwrap();
        }
        let store = Store::open(path.join("store")).unwrap();
        let catalogue = pc_image::firmware::catalogue();
        let scan = pc_image::firmware_import::scan(&[path.join("roms")], catalogue).unwrap();
        assert_eq!(scan.candidates.len(), 3, "{:?}", scan.issues);
        for candidate in &scan.candidates {
            pc_image::firmware_import::publish(&store, candidate, &[], catalogue).unwrap();
        }
        let packages = pc_image::create::packages(&store).unwrap();
        Self {
            path,
            store,
            packages,
        }
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
                self.rom("kiselev-xi8088-1.0.2-atreset"),
                self.rom("vgabios-0.8a-unemu.1-std"),
                self.rom("xub-r638-xtp-ata1f0"),
            ],
            title: Some("Open AT workbench".into()),
        }
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
