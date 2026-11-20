// SPDX-License-Identifier: AGPL-3.0-or-later
//! Generated FAT12 media and ATA-2 IDENTIFY DEVICE acceptance tests;
//! see machines/README.md § Preparing disks and Microsoft FAT Specification,
//! FAT on-disk format, BPB and FAT12 entries.

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

use cli_pc::command::{parse, run};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "unemupc-create-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn invoke(&self, options: &[&str], name: &str) -> (u8, String, String) {
        let mut args: Vec<OsString> = ["media", "create"]
            .into_iter()
            .chain(options.iter().copied())
            .map(Into::into)
            .collect();
        args.push(self.0.join(name).into());
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let code = run(&args, &mut output, &mut errors);
        (
            code,
            String::from_utf8(output).unwrap(),
            String::from_utf8(errors).unwrap(),
        )
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn word(bytes: &[u8], offset: usize) -> usize {
    usize::from(u16::from_le_bytes(
        bytes[offset..offset + 2].try_into().unwrap(),
    ))
}
fn fat12(fat: &[u8], cluster: usize) -> u16 {
    let offset = cluster * 3 / 2;
    let pair = u16::from_le_bytes([fat[offset], fat[offset + 1]]);
    if cluster.is_multiple_of(2) {
        pair & 0xFFF
    } else {
        pair >> 4
    }
}

#[test]
fn every_floppy_has_standard_readable_deterministic_fat12_or_zero_bytes() {
    let scratch = Scratch::new();
    for (kind, sectors, cluster_sectors, root_entries, media, fat_sectors, track) in [
        ("1.44M", 2880, 1, 224, 0xF0, 9, 18),
        ("1.2M", 2400, 1, 224, 0xF9, 7, 15),
        ("720K", 1440, 2, 112, 0xF9, 3, 9),
        ("360K", 720, 2, 112, 0xFD, 2, 9),
    ] {
        assert_eq!(
            scratch.invoke(&["floppy", "--type", kind], "blank.img").0,
            0
        );
        assert_eq!(
            fs::read(scratch.0.join("blank.img")).unwrap(),
            vec![0; sectors * 512]
        );
        fs::remove_file(scratch.0.join("blank.img")).unwrap();
        for name in ["first.img", "second.img"] {
            let result = scratch.invoke(
                &["floppy", "--type", kind, "--format", "--label", "test disk"],
                name,
            );
            assert_eq!(result.0, 0, "{result:?}");
        }
        let disk = fs::read(scratch.0.join("first.img")).unwrap();
        assert_eq!(disk, fs::read(scratch.0.join("second.img")).unwrap());
        assert_eq!(disk.len(), sectors * 512);
        assert_eq!(&disk[..3], &[0xEB, 0x3C, 0x90]);
        assert_eq!(word(&disk, 11), 512);
        assert_eq!(usize::from(disk[13]), cluster_sectors);
        assert_eq!(word(&disk, 14), 1);
        assert_eq!(disk[16], 2);
        assert_eq!(word(&disk, 17), root_entries);
        assert_eq!(word(&disk, 19), sectors);
        assert_eq!(disk[21], media);
        assert_eq!(word(&disk, 22), fat_sectors);
        assert_eq!(word(&disk, 24), track);
        assert_eq!(word(&disk, 26), 2);
        assert_eq!(&disk[28..36], &[0; 8]);
        assert_eq!(disk[38], 0x29);
        assert_eq!(&disk[39..43], &0x28959855u32.to_le_bytes());
        assert_eq!(&disk[43..54], b"TEST DISK  ");
        assert_eq!(&disk[54..62], b"FAT12   ");
        // The data-disk boot code that follows the BPB reports a disk
        // without a system; boot_sector_golden.rs runs it.
        assert!(String::from_utf8_lossy(&disk[62..510]).contains("This is not a system disk"));
        assert_eq!(&disk[510..512], &[0x55, 0xAA]);
        let fat = &disk[512..(1 + fat_sectors) * 512];
        assert_eq!(
            fat,
            &disk[(1 + fat_sectors) * 512..(1 + 2 * fat_sectors) * 512]
        );
        assert_eq!(fat12(fat, 0), 0xF00 | u16::from(media));
        assert_eq!(fat12(fat, 1), 0xFFF);
        let root_start = (1 + 2 * fat_sectors) * 512;
        let data_start = root_start + root_entries * 32;
        let clusters = (disk.len() - data_start) / (512 * cluster_sectors);
        assert!(clusters < 4085);
        for cluster in 2..clusters + 2 {
            assert_eq!(fat12(fat, cluster), 0);
        }
        assert_eq!(&disk[root_start..root_start + 11], b"TEST DISK  ");
        assert_eq!(disk[root_start + 11], 8);
        assert_eq!(word(&disk, root_start + 24), 0x21); // FAT epoch, 1980-01-01.
        assert!(disk[root_start + 32..].iter().all(|&byte| byte == 0));
        for name in ["first.img", "second.img"] {
            fs::remove_file(scratch.0.join(name)).unwrap();
        }
    }
}

#[test]
fn creation_refuses_existing_entries_and_force_replaces_without_rewriting_aliases() {
    let scratch = Scratch::new();
    let path = scratch.0.join("disk.img");
    fs::write(&path, b"original base").unwrap();
    fs::hard_link(&path, scratch.0.join("alias.img")).unwrap();
    assert_eq!(
        scratch.invoke(&["floppy", "--type", "360K"], "disk.img").0,
        1
    );
    assert_eq!(fs::read(&path).unwrap(), b"original base");
    assert_eq!(
        scratch
            .invoke(&["floppy", "--type", "360K", "--force"], "disk.img")
            .0,
        0
    );
    assert_eq!(
        fs::read(scratch.0.join("alias.img")).unwrap(),
        b"original base"
    );
    fs::create_dir(scratch.0.join("directory")).unwrap();
    assert_eq!(
        scratch
            .invoke(&["floppy", "--type", "360K", "--force"], "directory")
            .0,
        1
    );
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 3);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("missing", scratch.0.join("link.img")).unwrap();
        assert_eq!(
            scratch.invoke(&["floppy", "--type", "360K"], "link.img").0,
            1
        );
        assert_eq!(
            scratch
                .invoke(&["floppy", "--type", "360K", "--force"], "link.img")
                .0,
            0
        );
        assert!(!scratch.0.join("missing").exists());
        assert!(!fs::symlink_metadata(scratch.0.join("link.img"))
            .unwrap()
            .is_symlink());
    }
}

#[test]
fn hd_size_rounds_up_to_chs_and_explicit_geometry_reaches_identify() {
    use assembly::*;
    let scratch = Scratch::new();
    let (code, output, errors) = scratch.invoke(&["hd", "--size", "1MiB"], "size.img");
    assert_eq!(code, 0, "{errors}");
    assert_eq!(
        fs::metadata(scratch.0.join("size.img")).unwrap().len(),
        3 * 16 * 63 * 512
    );
    assert!(output.contains("cylinders = 3, heads = 16, sectors = 63"));
    assert!(output.contains("FDISK") && output.contains("FORMAT") && output.contains("XUB"));
    for chs in ["615/4/17", "1/16/63"] {
        let (code, _, errors) = scratch.invoke(&["hd", "--chs", chs, "--force"], "chs.img");
        assert_eq!(code, 0, "{errors}");
        let components: Vec<u32> = chs.split('/').map(|n| n.parse().unwrap()).collect();
        let geometry = HardDiskGeometry {
            cylinders: components[0],
            heads: components[1],
            sectors_per_track: components[2],
            bytes_per_sector: 512,
        };
        let bytes = fs::read(scratch.0.join("chs.img")).unwrap();
        assert_eq!(bytes.len() as u64, geometry.image_bytes());
        assert!(bytes.iter().all(|&byte| byte == 0));
        let mut config = at_boot(vec![0xFA, 0xF4]);
        let ide = IdeCfg {
            model: AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        config.devices.push(
            DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(ide), ide.port_windows())
                .with_irq(ide.irq()),
        );
        config.drives.push(DriveConfig {
            controller: DeviceId::Ide,
            unit: 0,
            kind: DriveKind::HardDisk { geometry },
            image: ImageSource::File(scratch.0.join("chs.img")),
            expected_sha256: None,
            write: WriteMode::Volatile,
            media: None,
        });
        let mut machine = assemble(&config).unwrap();
        let bus = machine.machine.bus_mut();
        bus.port_write(0x1F6, 1, 0xA0).unwrap();
        bus.port_write(0x1F7, 1, 0xEC).unwrap();
        let words: Vec<_> = (0..256).map(|_| bus.port_read(0x1F0, 2).unwrap()).collect();
        assert_eq!(
            (words[1], words[3], words[6]),
            (
                geometry.cylinders,
                geometry.heads,
                geometry.sectors_per_track
            )
        );
    }
}

#[test]
fn malformed_or_unaddressable_requests_fail_before_creating_output() {
    let scratch = Scratch::new();
    for options in [
        vec!["floppy"],
        vec!["floppy", "--type", "2.88M"],
        vec!["floppy", "--type", "360K", "--label", "LABEL"],
        vec![
            "floppy",
            "--type",
            "360K",
            "--format",
            "--label",
            "TOO LONG LABEL",
        ],
        vec![
            "floppy", "--type", "360K", "--format", "--label", "bad/name",
        ],
        vec!["floppy", "--type", "360K", "--format", "--label", "é"],
        vec!["floppy", "--type", "360K", "--force", "--force"],
        vec!["floppy", "--type", "360K", "--size", "1MiB"],
        vec!["hd"],
        vec!["hd", "--size", "0"],
        vec!["hd", "--size", "1GiB"],
        vec!["hd", "--size", "505MiB"],
        vec!["hd", "--size", "18446744073709551615GiB"],
        vec!["hd", "--size", "1.5MiB"],
        vec!["hd", "--size", "1MB"],
        vec!["hd", "--chs", "0/2/17"],
        vec!["hd", "--chs", "1025/16/63"],
        vec!["hd", "--chs", "1/17/63"],
        vec!["hd", "--chs", "1/16/64"],
        vec!["hd", "--chs", "1/2/3/4"],
        vec!["hd", "--chs", "1/2"],
        vec!["hd", "--chs", "1/2/3", "--size", "1MiB"],
        vec!["hd", "--chs", "1/2/3", "--format"],
    ] {
        let result = scratch.invoke(&options, "invalid.img");
        assert_eq!(result.0, 2, "{options:?}: {result:?}");
        assert!(!scratch.0.join("invalid.img").exists());
    }
    for size in ["504MiB", "504", "1MiB"] {
        let args: Vec<OsString> = ["media", "create", "hd", "--size", size, "out.img"]
            .into_iter()
            .map(Into::into)
            .collect();
        assert!(parse(&args).is_ok());
    }
}

#[test]
fn size_boundaries_defaults_and_options_after_output_are_explicit() {
    use cli_pc::create::ImageKind;
    use cli_pc::PcCommand;
    for (size, cylinders) in [("1", 3), ("32MiB", 66), ("504MiB", 1024)] {
        let args = ["media", "create", "hd", "--size", size, "out.img"].map(OsString::from);
        let Some(Ok(PcCommand::Create(command))) = cli_pc::parse(&args) else {
            panic!("expected create");
        };
        let ImageKind::HardDisk(geometry) = command.kind else {
            panic!("expected hard disk");
        };
        assert_eq!(
            (
                geometry.cylinders,
                geometry.heads,
                geometry.sectors_per_track
            ),
            (cylinders, 16, 63)
        );
    }
    let scratch = Scratch::new();
    let mut args: Vec<OsString> = ["media", "create", "floppy", "--type", "360K"]
        .map(Into::into)
        .into();
    args.push(scratch.0.join("default.img").into());
    args.extend(["--format", "--force"].map(Into::into));
    assert_eq!(run(&args, &mut Vec::new(), &mut Vec::new()), 0);
    let bytes = fs::read(scratch.0.join("default.img")).unwrap();
    assert_eq!(&bytes[43..54], b"NO NAME    ");
}

#[test]
fn printed_mount_snippet_loads_and_assembles_with_quoted_paths() {
    let scratch = Scratch::new();
    let (code, output, error) = scratch.invoke(&["hd", "--size", "1MiB"], "quoted\"disk.img");
    assert_eq!(code, 0, "{error}");
    let start = output.find("[[card]]").unwrap();
    let end = output.find("\nXUB supplies").unwrap();
    let mut rom = vec![0; 32768];
    rom[0x7FF0..0x7FF2].copy_from_slice(&[0xFA, 0xF4]);
    fs::write(scratch.0.join("bios.bin"), rom).unwrap();
    let text = format!(
        "schema = 1\n[board]\nmodel = \"ibm-5170\"\n[firmware.bios]\nimage = \"bios.bin\"\nprovides = [\"fixed-disk\"]\n{}",
        &output[start..end]
    );
    let path = scratch.0.join("machine.toml");
    fs::write(&path, text).unwrap();
    let loaded = machine_file::load_for_assembly(
        &path,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let config = loaded.config;
    let drive = config
        .drives
        .iter()
        .find(|drive| drive.controller == assembly::DeviceId::Ide)
        .unwrap();
    assert_eq!(
        drive.write,
        assembly::WriteMode::journal(
            scratch
                .0
                .canonicalize()
                .unwrap()
                .join("quoted\"disk.img.upfj")
        )
    );
    assert!(!output.contains("not available yet"));
    let mut assembled = assembly::assemble_with_options(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let bus = assembled.machine.bus_mut();
    bus.port_write(0x1F6, 1, 0xA0).unwrap();
    bus.port_write(0x1F7, 1, 0xEC).unwrap();
    assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 0x40);
    assert_eq!(bus.port_read(0x1F0, 2).unwrap(), 3);
}

#[test]
fn unlabeled_formatted_floppies_have_no_root_label_entry() {
    let scratch = Scratch::new();
    for kind in ["1.44M", "1.2M", "720K", "360K"] {
        let (code, _, errors) = scratch.invoke(
            &["floppy", "--type", kind, "--format", "--force"],
            "unlabeled.img",
        );
        assert_eq!(code, 0, "{errors}");
        let disk = fs::read(scratch.0.join("unlabeled.img")).unwrap();
        assert_eq!(&disk[43..54], b"NO NAME    ");
        let root_start = (word(&disk, 14) + usize::from(disk[16]) * word(&disk, 22)) * 512;
        let root_end = root_start + word(&disk, 17) * 32;
        assert!(
            disk[root_start..root_end].iter().all(|&byte| byte == 0),
            "{kind} root must be empty without --label"
        );
        let (code, _, errors) = scratch.invoke(
            &[
                "floppy", "--type", kind, "--format", "--label", "NO NAME", "--force",
            ],
            "explicit.img",
        );
        assert_eq!(code, 0, "{errors}");
        let named = fs::read(scratch.0.join("explicit.img")).unwrap();
        assert_eq!(&named[root_start..root_start + 11], b"NO NAME    ");
        assert_eq!(named[root_start + 11], 8);
    }
}

#[test]
fn leading_space_volume_label_is_rejected_before_writing() {
    let scratch = Scratch::new();
    let result = scratch.invoke(
        &["floppy", "--type", "360K", "--format", "--label", " LABEL"],
        "invalid.img",
    );
    assert_eq!(result.0, 2, "{result:?}");
    assert!(!scratch.0.join("invalid.img").exists());
}

#[test]
#[cfg(unix)]
fn preparing_disks_guide_override_loads_the_real_ide_profile() {
    for fixture in [
        "unemu-at-bios/bios-unemu-at.bin",
        "freedos/freedos.img",
        "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
    ] {
        if fixtures::fixture(fixture).is_none() {
            return;
        }
    }
    let scratch = Scratch::new();
    let profile = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../machines/at-ide.toml")
        .canonicalize()
        .unwrap();
    std::os::unix::fs::symlink(&profile, scratch.0.join("at-ide.toml")).unwrap();
    let guide = include_str!("../../../machines/README.md")
        .split_once("## Preparing disks")
        .unwrap()
        .1;
    let example = guide
        .split_once("```toml\n")
        .unwrap()
        .1
        .split_once("```")
        .unwrap()
        .0;
    fs::write(scratch.0.join("local.toml"), example).unwrap();
    assert_eq!(scratch.invoke(&["hd", "--size", "32MiB"], "disk.img").0, 0);
    let loaded = machine_file::load_for_assembly(
        scratch.0.join("local.toml"),
        assembly::AssemblyOptions::default(),
    )
    .unwrap();
    let disk = loaded
        .config
        .drives
        .iter()
        .find(|drive| drive.controller == assembly::DeviceId::Ide)
        .unwrap();
    let root = scratch.0.canonicalize().unwrap();
    assert_eq!(
        disk.image,
        assembly::ImageSource::File(root.join("disk.img"))
    );
    assert_eq!(
        disk.write,
        assembly::WriteMode::journal(root.join("disk.upfj"))
    );
    let args = [
        OsString::from("machine"),
        OsString::from("check"),
        scratch.0.join("local.toml").into(),
    ];
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(
        unemupc::run(&args, &mut output, &mut errors),
        0,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    assert!(
        !root.join("disk.upfj").exists(),
        "machine check must not initialize a journal"
    );
}
