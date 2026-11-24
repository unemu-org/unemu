// SPDX-License-Identifier: AGPL-3.0-or-later
//! A bound-form machine using only the reviewed open firmware fixtures.
use image_store::{Digest, Store};
use std::fs;

pub fn bound_config() -> Vec<u8> {
    br#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
disks = [{ id = "system", size = 1048576 }, { id = "data", size = 1048576 }]
[machine.board]
model = "ibm-5170"
display_switch = "colour"
[machine.board.rtc]
start = "2000-01-01T00:00:00"
nvram = "derived"
[machine.cpu]
model = "80286"
rate = "8/13"
fpu = "none"
[machine.memory]
conventional_kib = 640
extended_kib = 1024
[machine.firmware.bios]
use = "bios"
rom = "system-bios"
role = "system-bios"
base = 0xF8000
[[machine.firmware.option_roms]]
use = "vga"
rom = "video-bios"
role = "video-bios"
base = 0xC0000
[[machine.firmware.option_roms]]
use = "ide"
rom = "option-rom"
role = "option-rom"
base = 0xC8000
[[machine.card]]
kind = "vga"
[[machine.card]]
kind = "ide"
controller_model = "ata"
port = 0x1F0
irq = 14
[[machine.card.drives]]
channel = "master"
disk = "system"
geometry = { cylinders = 32, heads = 4, sectors = 16 }
write = "journal"
[[machine.card.drives]]
channel = "slave"
disk = "data"
geometry = { cylinders = 16, heads = 4, sectors = 32 }
write = "journal"
[[machine.card]]
kind = "fdc-765"
[[machine.card.drives]]
unit = 0
type = "1.2M"
empty = true
[[machine.card.drives]]
unit = 1
type = "1.44M"
empty = true
"#
    .to_vec()
}

pub fn template_recipe(store: &Store, root: &std::path::Path) -> std::path::PathBuf {
    let packages = pc_image::template::build_firmware(
        store,
        &[pc_harness::test_support::root().join("vendor")],
        &|_, _, _| Ok(()),
        &mut |_| {},
    )
    .unwrap();
    let config = bound_config();
    fs::write(root.join("bound.toml"), &config).unwrap();
    let mut text = format!(
        r#"recipe = "1.1"
[machine]
name = "private/machines/ibm-pc/bound"
platform = "ibm-pc"
version = "1.3.0"
[template]
config = "config"
disks = [{{ id = "system", size = 1048576 }}, {{ id = "data", size = 1048576 }}]
title = "Bound test machine"
labels = ["test"]
[license]
expression = "CC0-1.0"
[input.config]
name = "bound.toml"
size = {}
sha256 = "{}"
bundle = true
"#,
        config.len(),
        Digest::of(&config).hex()
    );
    for (id, package) in ["bios", "vga", "ide"].into_iter().zip(packages) {
        text.push_str(&format!(
            "\n[use.{id}]\nimage = \"{}:r1\"\ndigest = \"{}\"\n",
            package.repository, package.descriptor.digest
        ));
    }
    let path = root.join("bound.machine.toml");
    fs::write(&path, text).unwrap();
    path
}
