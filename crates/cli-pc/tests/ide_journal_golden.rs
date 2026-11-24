// SPDX-License-Identifier: AGPL-3.0-or-later
//! FreeDOS writes C: through XTIDE's ATA-2 PIO WRITE SECTORS and reboots.
//! See persistent-media.md §Integration in unemupc and Microsoft FAT Specification,
//! FAT16 BPB and Directory Structure. Firmware pins come from at-ide.toml;
//! the core revision is pinned independently for this golden below.

use assembly::{AssembledMachine, DeviceId, ImageSource, WriteMode};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use video::Frame;

use pc_harness::test_support::fixtures;

// Moving the core pin requires checking the recorded golden again.
const CORE_REV: &str = "4d638ccc4e2b13d3bb7385168a5de745931fed33";

#[path = "../../pc-image/tests/support/core_pin.rs"]
mod core_pin;
use core_pin::assert_core_revision;

#[test]
fn golden_core_revision_matches_lock() {
    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ide-dos-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn console(machine: &AssembledMachine) -> String {
    let Frame::Text(grid) = machine.capture_text(DeviceId::Mda).unwrap().frame else {
        panic!("MDA text required")
    };
    grid.rows_as_strings().join("\n")
}
fn keys(text: &str) -> Vec<u8> {
    let mut keys = Vec::new();
    for byte in text.bytes() {
        let (code, shift) = match byte {
            b'e' => (0x12, false),
            b'c' => (0x2E, false),
            b'h' => (0x23, false),
            b'o' => (0x18, false),
            b'x' => (0x2D, false),
            b'z' => (0x2C, false),
            b't' => (0x14, false),
            b'y' => (0x15, false),
            b'p' => (0x19, false),
            b' ' => (0x39, false),
            b'\r' => (0x1C, false),
            b'>' => (0x34, true),
            b':' => (0x27, true),
            _ => panic!("unsupported key"),
        };
        if shift {
            keys.push(0x2A);
        }
        keys.extend([code, code | 0x80]);
        if shift {
            keys.push(0xAA);
        }
    }
    keys
}
fn session(machine: &mut AssembledMachine, command: &str) {
    let keyboard_index = machine.device_index[&DeviceId::Keyboard];
    let policy = assembly::RunPolicy {
        retired_cap: Some(100_000_000),
        idle_limit: Some(pc_harness::BIOS_IDLE_LIMIT),
        stop: assembly::StopCondition::PromptThen {
            console: assembly::ConsoleProbe {
                device: DeviceId::Mda,
            },
            needle: pc_harness::DOS_DATE_PROMPT.into(),
            poll_step: pc_harness::DOS_INTERACTIVE_POLL_STEP,
            then_budget: 10_000_000,
        },
        stimuli: vec![assembly::KeyboardStimulus {
            keyboard_index,
            scancodes: keys(&format!("\r\r{command}\r")),
            schedule: assembly::InjectionSchedule::AfterPrompt,
        }],
    };
    machine.run_session(&policy).unwrap();
    eprintln!("{}", console(machine));
    assert!(
        console(machine).lines().any(|row| row.trim() == "A:\\>"),
        "command must return to the prompt"
    );
}

#[test]
#[ignore = "slow: two BIOS/FreeDOS boots and a persistent ATA write/read session"]
fn freedos_ide_journal_survives_a_fresh_assembly_without_changing_base() {
    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
    for path in [
        "unemu-at-bios/bios-unemu-at.bin",
        "freedos/freedos.img",
        "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
        "hdd/fat16.img",
    ] {
        if fixtures::fixture(path).is_none() {
            return;
        }
    }
    let temp = Temp::new();
    let mut config =
        machine_file::load(pc_harness::test_support::root().join("machines/at-ide.toml")).unwrap();
    let drive = config
        .drives
        .iter_mut()
        .find(|drive| drive.controller == DeviceId::Ide)
        .unwrap();
    let ImageSource::Inline(base) = &drive.image else {
        panic!("fixture bytes expected")
    };
    let base = base.clone();
    let base_path = temp.0.join("base.img");
    let journal = temp.0.join("disk.upfj");
    std::fs::write(&base_path, &base).unwrap();
    drive.image = ImageSource::File(base_path.clone());
    drive.write = WriteMode::journal(&journal);
    let policy = assembly::ObservationPolicy {
        routes: vec![assembly::ObservationRequest {
            device: DeviceId::Mda,
            output: assembly::OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..Default::default()
    };
    let mut first = machine_file::assemble_with_observation(&config, &policy).unwrap();
    assert_eq!(first.persistence.len(), 1);
    session(&mut first, "echo x>c:zz");
    assert!(console(&first).contains("A:\\>echo x>c:zz"));
    first.finish_persistence().unwrap();
    drop(first);
    let scan = journal_files::scan(&std::fs::read(&journal).unwrap(), &base, base.len() as u64);
    assert_eq!(scan.report.status, journal_files::Status::Valid);
    assert_eq!(scan.report.generations.len(), 1);
    assert!(scan.report.generations[0].commands > 0);
    let replay = scan.image.unwrap();
    assert_ne!(replay, base);
    assert_eq!(fat16_file(&replay, b"ZZ         "), b"x\r\n");
    let mut second = machine_file::assemble_with_observation(&config, &policy).unwrap();
    let index = second.device_index[&DeviceId::Ide];
    let ata = second
        .machine
        .bus_mut()
        .device_mut(index)
        .unwrap()
        .as_any_mut()
        .unwrap()
        .downcast_mut::<devices::Ata>()
        .unwrap();
    assert_eq!(ata.drive(0).unwrap().image(), replay);
    session(&mut second, "type c:zz");
    assert!(console(&second).contains("A:\\>type c:zz"));
    assert!(console(&second).lines().any(|row| row.trim() == "x"));
    second.finish_persistence().unwrap();
    drop(second);
    let exported = temp.0.join("export.img");
    let backup = temp.0.join("backup.upfj");
    for (operation, extra) in [
        ("generations", vec![]),
        ("verify", vec![]),
        (
            "commit",
            vec![
                "--to".into(),
                "1".into(),
                "--output".into(),
                exported.clone().into_os_string(),
            ],
        ),
        (
            "rollback",
            vec![
                "--to".into(),
                "1".into(),
                "--backup".into(),
                backup.into_os_string(),
            ],
        ),
    ] {
        let mut args = vec![
            "media".into(),
            operation.into(),
            "--base".into(),
            base_path.clone().into_os_string(),
            "--journal".into(),
            journal.clone().into_os_string(),
        ];
        args.extend(extra);
        let mut output = Vec::new();
        let mut errors = Vec::new();
        assert_eq!(
            cli_pc::command::run(&args, &mut output, &mut errors),
            0,
            "{operation}: {}",
            String::from_utf8_lossy(&errors)
        );
    }
    assert_eq!(std::fs::read(exported).unwrap(), replay);
    let scan = journal_files::scan(&std::fs::read(&journal).unwrap(), &base, base.len() as u64);
    assert_eq!(scan.report.generations.len(), 1);
    assert_eq!(scan.image.unwrap(), replay);
    assert_eq!(std::fs::read(base_path).unwrap(), base);
}

fn fat16_file(image: &[u8], name: &[u8; 11]) -> Vec<u8> {
    let partition = u32::from_le_bytes(image[454..458].try_into().unwrap()) as usize * 512;
    let volume = &image[partition..];
    let word = |at| usize::from(u16::from_le_bytes([volume[at], volume[at + 1]]));
    assert_eq!(word(11), 512);
    let fat = word(14) * 512;
    let root = fat + usize::from(volume[16]) * word(22) * 512;
    let root_bytes = word(17) * 32;
    let data = root + root_bytes.div_ceil(512) * 512;
    let entry = volume[root..root + root_bytes]
        .as_chunks::<32>()
        .0
        .iter()
        .take_while(|entry| entry[0] != 0)
        .find(|entry| entry[11] & 0x18 == 0 && &entry[..11] == name)
        .expect("persisted root entry");
    let cluster = usize::from(u16::from_le_bytes([entry[26], entry[27]]));
    let size = u32::from_le_bytes(entry[28..32].try_into().unwrap()) as usize;
    assert_eq!(size, 3);
    assert!(cluster >= 2);
    assert!(word(14) > 0);
    let next = u16::from_le_bytes(
        volume[fat + cluster * 2..fat + cluster * 2 + 2]
            .try_into()
            .unwrap(),
    );
    assert!(next >= 0xFFF8);
    let start = data + (cluster - 2) * usize::from(volume[13]) * 512;
    volume[start..start + size].to_vec()
}
