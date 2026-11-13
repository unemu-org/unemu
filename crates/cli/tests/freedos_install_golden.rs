// SPDX-License-Identifier: AGPL-3.0-or-later
//! Install FreeDOS from open tools onto a blank ATA disk, then boot that disk.
//! ATA-2 §§6.1–6.2 PIO commands and Microsoft FAT Specification, FAT16 BPB,
//! run through the open AT firmware. See machines/README.md §Playing DOS games.
//! CPU and fixture revisions are pinned independently of the final screen.

use assembly::{
    AssembledMachine, ConsoleProbe, DeviceId, DriveKind, FloppyAction, FloppyGeometry,
    HardDiskGeometry, ImageSource, InjectionSchedule, KeyboardStimulus, ObservationPolicy,
    ObservationRequest, OutputKind, RunPolicy, StopCondition, WriteMode,
};
use cpu::Cpu;
use std::path::PathBuf;
use video::Frame;

#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/session.rs"]
pub mod session;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("freedos-install-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn console(assembled: &AssembledMachine) -> String {
    let Frame::Text(grid) = assembled.capture_text(DeviceId::Mda).unwrap().frame else {
        panic!("MDA text required")
    };
    grid.to_dump()
}

fn hard_disk(assembled: &mut AssembledMachine) -> Vec<u8> {
    let index = assembled.device_index[&DeviceId::Ide];
    let ata = assembled
        .machine
        .bus_mut()
        .device_mut(index)
        .unwrap()
        .as_any_mut()
        .unwrap()
        .downcast_mut::<devices::Ata>()
        .unwrap();
    ata.drive(0).unwrap().image()
}

fn keys(text: &str) -> Vec<u8> {
    let mut keys = Vec::new();
    for byte in text.bytes() {
        let (code, shift) = match byte {
            b'a'..=b'z' => (
                [
                    0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32,
                    0x31, 0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
                ][usize::from(byte - b'a')],
                false,
            ),
            b'1'..=b'9' => (byte - b'1' + 2, false),
            b'0' => (0x0b, false),
            b' ' => (0x39, false),
            b'/' => (0x35, false),
            b',' => (0x33, false),
            b':' => (0x27, true),
            b'\r' => (0x1c, false),
            _ => panic!("unsupported script key {byte}"),
        };
        if shift {
            keys.push(0x2a);
        }
        keys.extend([code, code | 0x80]);
        if shift {
            keys.push(0xaa);
        }
    }
    keys
}

fn run_for(assembled: &mut AssembledMachine, budget: u64) {
    let result = assembled.run_session(&RunPolicy::natural(
        Some(assembled.machine.virtual_time() + budget),
        Some(cli::BIOS_IDLE_LIMIT),
    ));
    eprintln!(
        "run={result:?}, retired={}, virtual={}, CS:IP={:04x}:{:04x}\n{}",
        assembled.machine.cpu().retired(),
        assembled.machine.virtual_time(),
        assembled.machine.cpu().code_segment(),
        assembled.machine.cpu().reg(cpu::CpuReg::Rip),
        console(assembled)
    );
    result.unwrap();
}

fn command(assembled: &mut AssembledMachine, command: &str) {
    let index = assembled.device_index[&DeviceId::Keyboard];
    // Pace characters so the BIOS's small typeahead ring never limits a long
    // command while FreeCOM is still updating its line editor.
    for character in format!("{command}\r").chars() {
        assembly::inject_scancodes(&mut assembled.machine, index, &keys(&character.to_string()));
        assembled
            .run_session(&RunPolicy::natural(
                Some(assembled.machine.virtual_time() + 100_000),
                Some(cli::BIOS_IDLE_LIMIT),
            ))
            .unwrap();
    }
    let deadline = assembled.machine.virtual_time() + 100_000_000;
    loop {
        run_for(assembled, 10_000_000);
        if console(assembled)
            .lines()
            .rfind(|row| !row.trim().is_empty())
            == Some("A:\\>")
        {
            break;
        }
        assert!(
            assembled.machine.virtual_time() < deadline,
            "command must return to A: prompt: {command}"
        );
    }
}

fn boot_prompt(assembled: &mut AssembledMachine, drive: &str) {
    let policy = RunPolicy {
        retired_cap: Some(assembled.machine.virtual_time() + 100_000_000),
        idle_limit: Some(cli::BIOS_IDLE_LIMIT),
        stop: StopCondition::PromptThen {
            console: ConsoleProbe {
                device: DeviceId::Mda,
            },
            needle: cli::DOS_DATE_PROMPT.into(),
            poll_step: cli::DOS_INTERACTIVE_POLL_STEP,
            then_budget: 2_000_000,
        },
        stimuli: vec![KeyboardStimulus {
            keyboard_index: assembled.device_index[&DeviceId::Keyboard],
            scancodes: keys("\r\r"),
            schedule: InjectionSchedule::AfterPrompt,
        }],
    };
    let result = assembled.run_session(&policy);
    eprintln!(
        "boot {drive}: {result:?}, retired={}, virtual={}\n{}",
        assembled.machine.cpu().retired(),
        assembled.machine.virtual_time(),
        console(assembled)
    );
    result.unwrap();
    assert!(
        console(assembled)
            .lines()
            .any(|row| row.trim() == format!("{drive}:\\>")),
        "boot must reach {drive}: prompt"
    );
}

fn reboot(assembled: &mut AssembledMachine) {
    assembled.before_cpu_reset().unwrap();
    assembled.machine.reset_cpu().unwrap();
    // POST leaves the previous console visible until video initialization. Wait
    // for it to clear so the old date prompt cannot trigger keyboard injection.
    let deadline = assembled.machine.virtual_time() + 30_000_000;
    while console(assembled).contains(cli::DOS_DATE_PROMPT) {
        run_for(assembled, 1_000_000);
        assert!(
            assembled.machine.virtual_time() < deadline,
            "POST must clear the old prompt"
        );
    }
}

#[test]
#[ignore = "slow: open FreeDOS partition, format, system transfer and hard disk boot"]
fn freedos_installs_and_boots_from_a_blank_hard_disk() {
    assert!(
        include_str!("../../../Cargo.lock").contains("#fe240a56ac2040e3deaebb2537e642137ee02c71")
    );
    for path in [
        "kiselev-bios/bios-xi8088-atreset.bin",
        "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
        "hdd/fat16.img",
        "freedos/freedos.img",
        "freedos/freedos-tools-1200.img",
    ] {
        if fixtures::fixture(path).is_none() {
            return;
        }
    }
    let tools = fixtures::fixture("freedos/freedos-tools-1200.img").unwrap();
    let tools_digest: String = media::Header::new(&tools, 0)
        .unwrap()
        .base_digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        tools_digest,
        "8d91283a1c0d6fc5b5fdb5e40db5b66dd9fabbf014d37262b7892f3b150b6f63"
    );
    let scratch = Scratch::new();
    let base_path = scratch.0.join("blank.img");
    let journal = scratch.0.join("installed.upfj");
    let args = [
        "media".into(),
        "create".into(),
        "hd".into(),
        "--size".into(),
        "32MiB".into(),
        base_path.clone().into_os_string(),
    ];
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(
        cli::command::run(&args, &mut output, &mut errors),
        0,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    let base = std::fs::read(&base_path).unwrap();
    assert!(base.iter().all(|&byte| byte == 0));
    let base_digest = media::Header::new(&base, 0).unwrap().base_digest;
    let mut config = machine_file::load(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-ide.toml"),
    )
    .unwrap();
    let floppy = config
        .drives
        .iter_mut()
        .find(|drive| drive.controller == DeviceId::Fdc)
        .unwrap();
    floppy.image = ImageSource::Inline(tools);
    floppy.kind = DriveKind::Floppy {
        geometry: FloppyGeometry::HD_1_2,
    };
    floppy.expected_sha256 = None;
    floppy.write = WriteMode::ReadOnly;
    let disk = config
        .drives
        .iter_mut()
        .find(|drive| drive.controller == DeviceId::Ide)
        .unwrap();
    disk.image = ImageSource::File(base_path.clone());
    disk.kind = DriveKind::HardDisk {
        geometry: HardDiskGeometry {
            cylinders: 66,
            heads: 16,
            sectors_per_track: 63,
            bytes_per_sector: 512,
        },
    };
    disk.expected_sha256 = Some(base_digest);
    disk.write = WriteMode::journal(&journal);
    let observation = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Mda,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..Default::default()
    };
    let mut assembled = assembly::assemble_with_observation(&config, &observation).unwrap();
    boot_prompt(&mut assembled, "A");
    // FDISK's DOC/FDISK/HELP.TXT defines the ,100 percentage suffix and /PRIO
    // forces FAT16, matching the 8086 FAT12/16 kernel rather than FAT32.
    command(&mut assembled, "fdisk /prio:100,100 1");
    command(&mut assembled, "fdisk /activate:1 1");
    command(&mut assembled, "fdisk /mbr 1");
    let partitioned = hard_disk(&mut assembled);
    assert_eq!(&partitioned[510..512], &[0x55, 0xaa]);
    assert_eq!(partitioned[446], 0x80, "primary partition must be active");
    assert!(
        matches!(partitioned[450], 0x04 | 0x06),
        "primary partition must be FAT16"
    );
    assert_eq!(
        u32::from_le_bytes(partitioned[454..458].try_into().unwrap()),
        63
    );
    drop(partitioned);
    reboot(&mut assembled);
    boot_prompt(&mut assembled, "A");
    // DOC/FORMAT/HELP.TXT defines /Q /U for a fresh disk, /S invokes SYS,
    // and /Z:seriously suppresses the hard-disk confirmation prompt.
    command(&mut assembled, "format c: /s /q /u /v:freedos /z:seriously");
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
        .unwrap();
    reboot(&mut assembled);
    boot_prompt(&mut assembled, "C");
    let screen = console(&assembled);
    eprintln!(
        "FINAL hash={:016x}, retired={}, virtual={}",
        session::screen_hash(&screen),
        assembled.machine.cpu().retired(),
        assembled.machine.virtual_time()
    );
    // NEC uPD765A datasheet, READ ID command and Status Register 1 (MA): an ID
    // search ends only on an ID field or after two index pulses. Successful
    // empty-drive calibration reaches that search, which cannot end without a
    // disk. Intel 82077AA datasheet, §8.1 Command and Result Phase Handshaking,
    // corroborates the required software timeout. The BIOS exhausts one IRQ
    // wait and three command-ready waits before booting C:. Prompt polling
    // rounds the added firmware work up to two million virtual steps.
    assert_eq!(session::screen_hash(&screen), 0x8f1e57cd86975e25);
    assert_eq!(assembled.machine.cpu().retired(), 104_210_281);
    assert_eq!(assembled.machine.virtual_time(), 108_800_000);
    let installed = hard_disk(&mut assembled);
    assembled.finish_persistence().unwrap();
    drop(assembled);
    assert_eq!(std::fs::read(&base_path).unwrap(), base);
    let scan = media::scan(&std::fs::read(journal).unwrap(), &base, base.len() as u64);
    assert_eq!(scan.report.status, media::Status::Valid);
    assert_ne!(installed, base);
    assert_eq!(
        scan.image.unwrap(),
        installed,
        "journal must replay the complete installation"
    );
}
