// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic host ROM discovery through launch loading; machine-files.md §Frontends.
use assembly::test_support::TempDir;
use sha2::{Digest, Sha256};

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn launch_resolves_pins_in_place_and_preserves_machine_file_and_existing_path() {
    let temp = TempDir::new("launch-rom-discovery");
    let dumps = temp.path().join("dumps");
    std::fs::create_dir(&dumps).unwrap();
    let bytes = vec![0xf4; 32768];
    let found = dumps.join("arbitrary-name.bin");
    std::fs::write(&found, &bytes).unwrap();
    let path = temp.path().join("machine.toml");
    let text = format!(
        "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='missing.bin'\nsha256='{}'\n",
        digest(&bytes)
    );
    std::fs::write(&path, &text).unwrap();
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    assert!(machine_file::load_for_assembly(&path, options).is_err());
    let loaded =
        machine_file::load_for_assembly_with_rom_dirs(&path, options, std::slice::from_ref(&dumps))
            .unwrap();
    loaded.check().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert!(!temp.path().join("missing.bin").exists());
    assert_eq!(std::fs::read_dir(&dumps).unwrap().count(), 1);
    // A matching declared path needs no usable fallback directory.
    let valid = text.replace("missing.bin", &found.to_string_lossy());
    std::fs::write(&path, valid).unwrap();
    machine_file::load_for_assembly_with_rom_dirs(&path, options, &[temp.path().join("absent")])
        .unwrap();
}

#[test]
fn launch_discovers_split_chips_and_assembled_image_pins() {
    let temp = TempDir::new("launch-rom-parts");
    let dumps = temp.path().join("dumps");
    std::fs::create_dir(&dumps).unwrap();
    let even = vec![0xf4; 16384];
    let odd = vec![0x90; 16384];
    std::fs::write(dumps.join("one"), &even).unwrap();
    std::fs::write(dumps.join("two"), &odd).unwrap();
    let bytes = assembly::interleave_parts(&[
        (assembly::Lane::Even, 0, even.clone()),
        (assembly::Lane::Odd, 0, odd.clone()),
    ])
    .unwrap();
    let path = temp.path().join("machine.toml");
    let text = format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nsha256='{}'\n[[firmware.bios.parts]]\nimage='absent-even'\nlane='even'\nsha256='{}'\n[[firmware.bios.parts]]\nimage='absent-odd'\nlane='odd'\nsha256='{}'\n",digest(&bytes),digest(&even),digest(&odd));
    std::fs::write(&path, &text).unwrap();
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    machine_file::load_for_assembly_with_rom_dirs(&path, options, std::slice::from_ref(&dumps))
        .unwrap();
    std::fs::remove_file(dumps.join("one")).unwrap();
    std::fs::remove_file(dumps.join("two")).unwrap();
    std::fs::write(dumps.join("assembled"), &bytes).unwrap();
    machine_file::load_for_assembly_with_rom_dirs(&path, options, std::slice::from_ref(&dumps))
        .unwrap();
    std::fs::write(dumps.join("assembled"), vec![0; bytes.len()]).unwrap();
    assert!(machine_file::load_for_assembly_with_rom_dirs(&path, options, &[dumps]).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
}

#[test]
fn discovery_is_available_without_loading_a_machine_file() {
    use assembly::ImageSource;
    use machine_file::firmware_discovery::{
        locate_firmware_with_roots, with_discovery_session, DiscoveryControl, FirmwarePin,
        FirmwareRoot, RootOrigin,
    };
    let temp = TempDir::new("host-discovery-api");
    let bytes = b"synthetic firmware";
    let path = temp.path().join("renamed.rom");
    std::fs::write(&path, bytes).unwrap();
    let pin = FirmwarePin {
        sha256: format!("{:x}", Sha256::digest(bytes)),
        length: Some(bytes.len() as u64),
    };
    let roots = [FirmwareRoot::new(temp.path(), RootOrigin::CommandLine)];
    let control = DiscoveryControl::default();
    let mut warnings = Vec::new();
    let found = with_discovery_session(None, false, &control, || {
        locate_firmware_with_roots(&roots, None, Some(&pin), &[], &mut warnings)
    })
    .unwrap();
    assert_eq!(found, Some(ImageSource::File(path)));
    assert!(warnings.is_empty());
}
