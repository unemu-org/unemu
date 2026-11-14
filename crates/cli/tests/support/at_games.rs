// SPDX-License-Identifier: AGPL-3.0-or-later
//! Disposable game profiles and synthetic media; machine-files.md §Validation.

use assembly::test_support::TempDir;
use std::path::{Path, PathBuf};

pub const IBM: &str = "at-games.toml";
pub const OPEN: &str = "at-games-open.toml";
pub const SYSTEM: &str = "ibm-5170-bios-1985-06-10";
pub const VIDEO: &str = "ibm-vga-bios-1987-32k";

pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub struct Profile {
    pub tree: TempDir,
    pub file: PathBuf,
}

impl Profile {
    pub fn new(name: &str, synthetic: bool) -> Self {
        let tree = TempDir::new("at-games");
        let text = std::fs::read_to_string(repository().join("machines").join(name)).unwrap();
        let text = if synthetic {
            text.lines()
                .filter(|line| !line.trim_start().starts_with("sha256 ="))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            text
        };
        let file = tree.path().join("machines").join(name);
        let profile = Self { tree, file };
        profile.write(&format!("machines/{name}"), text.as_bytes());
        profile.write(
            &format!("vendor/user-roms/{SYSTEM}/6480090.u27"),
            &vec![0xF4; 32768],
        );
        profile.write(
            &format!("vendor/user-roms/{SYSTEM}/6480091.u47"),
            &vec![0xF4; 32768],
        );
        profile.write(
            &format!("vendor/user-roms/{VIDEO}/ibm-vga-1987-32k.bin"),
            &option_rom(32768, 8192),
        );
        profile.write(
            "vendor/kiselev-bios/bios-xi8088-atreset.bin",
            &vec![0xF4; 32768],
        );
        profile.write(
            "vendor/xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
            &option_rom(8192, 0),
        );
        profile.write("vendor/vgabios/vgabios-std.bin", &option_rom(32768, 0));
        profile.write("vendor/freedos/freedos-1200.img", &vec![0; 1_228_800]);
        profile.write("vendor/freedos/freedos-tools-1200.img", &vec![0; 1_228_800]);
        profile.write("machines/local/transfer-1200.img", &vec![0; 1_228_800]);
        profile.write("machines/local/games.img", &vec![0; 615 * 4 * 17 * 512]);
        profile
    }

    pub fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.tree.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    pub fn journal(&self) -> PathBuf {
        self.tree
            .path()
            .canonicalize()
            .unwrap()
            .join("machines/local/games.upfj")
    }

    pub fn relocate_synthetic_video(&self) {
        // Only identified system firmware establishes the C0000h video scan.
        // Exercise the remaining hardware at the unlisted extension window,
        // above the open profile's XTIDE ROM at C8000h and EMS frame at D0000h.
        let base = if self.file.ends_with(OPEN) {
            "base = 0xE0000"
        } else {
            "base = 0xC8000"
        };
        let text = std::fs::read_to_string(&self.file).unwrap();
        std::fs::write(&self.file, text.replace("base = 0xC0000", base)).unwrap();
    }
}

fn option_rom(size: usize, offset: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    bytes[offset..offset + 4].copy_from_slice(&[0x55, 0xAA, ((size - offset) / 512) as u8, 0xCB]);
    let sum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    bytes[size - 1] = 0u8.wrapping_sub(sum);
    bytes
}
