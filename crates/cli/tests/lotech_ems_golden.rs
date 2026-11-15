// SPDX-License-Identifier: AGPL-3.0-or-later
//! Lo-tech 2MB EMS Board schematic, revision r02, HCT573 page registers and SRAM.
//! Open AT firmware and FreeDOS MEM exercise a fetched local-only driver.
//! LIM Expanded Memory Specification 4.0, INT 67h AH=41h/43h/44h/45h;
//! Intel 80286 Programmer's Reference Manual, Appendix A, Jcc and JMP.
//! See machines/README.md §Playing DOS games. CPU revision:
//! 060d446a883a9b2fc94b3ef91258cb4e6e98d916.

use assembly::{
    ConsoleProbe, DeviceConfig, DeviceId, DeviceKind, ImageSource, LotechEmsCfg, MmioWindow,
    ObservationPolicy, ObservationRequest, OutputKind, RunPolicy, StopCondition,
};
use cpu::Cpu;
use device::{Device, DeviceError, NextEvent, VClock};
use video::Frame;

#[path = "support/fat12.rs"]
mod fat12;
#[path = "support/fixtures.rs"]
pub mod fixtures;

const GOLDEN: (u64, u64) = (0x0eda968da7052433, 18_681_813);

// Deliberately alias every physical page to page zero as a negative control.
struct AliasedPages(devices::LotechEms);
impl Device for AliasedPages {
    fn name(&self) -> &'static str {
        "aliased-ems-test"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.0.port_read(port, size)
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.0
            .port_write(port, size, if value < 128 { 0 } else { value })
    }
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        self.0.mmio_read(addr, size)
    }
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.0.mmio_write(addr, size, value)
    }
    fn tick(&mut self, now: VClock) {
        self.0.tick(now);
    }
    fn next_event(&self, _: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn save(&self) -> Vec<u8> {
        self.0.save()
    }
    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        self.0.restore(blob)
    }
}

fn page_check_program() -> Vec<u8> {
    let mut code = Vec::new();
    let mut labels = std::collections::BTreeMap::new();
    let mut branches = Vec::new();
    // The 286 has short conditional jumps; invert the condition over a near JMP.
    let branch_ne =
        |code: &mut Vec<u8>, branches: &mut Vec<(usize, &'static str)>, label: &'static str| {
            code.extend([0x74, 3, 0xE9, 0, 0]);
            branches.push((code.len() - 2, label));
        };
    // INT 67h: get frame segment, allocate all 128 logical pages.
    code.extend([0xB4, 0x41, 0xCD, 0x67, 0x84, 0xE4]);
    branch_ne(&mut code, &mut branches, "failed");
    code.extend([0x8E, 0xC3, 0xBB, 128, 0, 0xB4, 0x43, 0xCD, 0x67, 0x84, 0xE4]);
    branch_ne(&mut code, &mut branches, "failed");
    code.extend([0x89, 0xD6, 0x31, 0xED]);
    labels.insert("fill", code.len());
    // Spread logical pages across all four physical windows and latches.
    for (label, verifying) in [("fill", false), ("verify", true)] {
        if verifying {
            code.extend([0x31, 0xED]);
            labels.insert(label, code.len());
        }
        code.extend([
            0x89, 0xEB, 0x89, 0xF2, 0x89, 0xE8, 0x24, 3, 0xB4, 0x44, 0xCD, 0x67, 0x84, 0xE4,
        ]);
        branch_ne(&mut code, &mut branches, "failed-free");
        code.extend([0x89, 0xEF, 0x83, 0xE7, 3, 0xB1, 14, 0xD3, 0xE7]);
        if verifying {
            code.extend([0x26, 0x39, 0x2D]);
            branch_ne(&mut code, &mut branches, "failed-free");
        } else {
            code.extend([0x89, 0xE8, 0x26, 0x89, 5]);
        }
        code.extend([0x45, 0x81, 0xFD, 128, 0]);
        branch_ne(&mut code, &mut branches, label);
    }
    // Release the handle before MEM observes free pages and handles.
    code.extend([0x89, 0xF2, 0xB4, 0x45, 0xCD, 0x67, 0x84, 0xE4]);
    branch_ne(&mut code, &mut branches, "failed");
    code.extend([0xB8, 0, 0x4C, 0xCD, 0x21]);
    labels.insert("failed-free", code.len());
    code.extend([0x89, 0xF2, 0xB4, 0x45, 0xCD, 0x67]);
    labels.insert("failed", code.len());
    code.extend([0xB8, 1, 0x4C, 0xCD, 0x21]);
    for (at, label) in branches {
        let displacement = (labels[label] as i16).wrapping_sub((at + 2) as i16);
        code[at..at + 2].copy_from_slice(&displacement.to_le_bytes());
    }
    code
}

fn screen_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

fn capture(config: &assembly::MachineConfig, broken: bool) -> (String, u64) {
    let observation = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Mda,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..Default::default()
    };
    let mut rig = assembly::assemble_with_observation(config, &observation).unwrap();
    if broken {
        let index = rig.device_index[&DeviceId::LotechEms];
        *rig.machine.bus_mut().device_mut(index).unwrap() = Box::new(AliasedPages(
            devices::LotechEms::new(0x260, 0xD000, 2048).unwrap(),
        ));
    }
    rig.run_session(&RunPolicy {
        retired_cap: Some(80_000_000),
        idle_limit: Some(cli::BIOS_IDLE_LIMIT),
        stop: StopCondition::PromptThen {
            console: ConsoleProbe {
                device: DeviceId::Mda,
            },
            needle: "A:\\>".into(),
            poll_step: 100_000,
            then_budget: 0,
        },
        stimuli: vec![],
    })
    .unwrap();
    let Frame::Text(grid) = rig.capture_text(DeviceId::Mda).unwrap().frame else {
        panic!("expected text");
    };
    (grid.to_dump(), rig.machine.cpu().retired())
}

#[test]
#[ignore = "slow: boots open AT firmware and FreeDOS MEM /E, including an aliased-page mutation"]
fn freedos_mem_reports_lotech_expanded_memory() {
    let Some(driver) = fixtures::optional_fixture("ltemm/LTEMM.EXE") else {
        return;
    };
    assert!(
        include_str!("../../../Cargo.lock").contains("#060d446a883a9b2fc94b3ef91258cb4e6e98d916")
    );
    for path in [
        "kiselev-bios/bios-xi8088-atreset.bin",
        "freedos/freedos.img",
        "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
        "hdd/fat16.img",
    ] {
        if fixtures::fixture(path).is_none() {
            return;
        }
    }
    let Some(mut image) = fixtures::fixture("freedos/freedos-tools-1200.img") else {
        return;
    };
    let driver_pin = fixtures::parse_pin(
        include_str!("../../../vendor/ltemm/fetch.sh"),
        "OUTPUT_SHA256=\"",
    )
    .unwrap();
    let tools_pin = fixtures::parse_pin(
        include_str!("../../../vendor/freedos/fetch.sh"),
        "check_sha256 \"$OUT_TOOLS\" \"",
    )
    .unwrap();
    assert_eq!(assembly::test_support::fixture_sha256(&driver), driver_pin);
    assert_eq!(assembly::test_support::fixture_sha256(&image), tools_pin);
    fat12::add_file(&mut image, b"LTEMM   EXE", &driver);
    fat12::add_file(
        &mut image,
        b"CONFIG  SYS",
        b"FILES=20\r\nBUFFERS=10\r\nDEVICE=LTEMM.EXE /p:D000 /n\r\nSHELL=COMMAND.COM /P /E:256\r\n",
    );
    fat12::add_file(&mut image, b"EMSCHECKCOM", &page_check_program());
    fat12::add_file(&mut image, b"AUTOEXECBAT", b"@ECHO OFF\r\nCLS\r\nEMSCHECK.COM\r\nIF ERRORLEVEL 1 GOTO FAILED\r\nMEM /E\r\nECHO EMS-CHECK-DONE\r\nGOTO END\r\n:FAILED\r\nECHO EMS-CHECK-FAILED\r\n:END\r\n");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut config = machine_file::load(root.join("machines/at-ide.toml")).unwrap();
    let drive = config
        .drives
        .iter_mut()
        .find(|d| d.controller == DeviceId::Fdc && d.unit == 0)
        .unwrap();
    drive.image = ImageSource::Inline(image);
    drive.media = None;
    drive.expected_sha256 = None;
    drive.kind = assembly::DriveKind::Floppy {
        geometry: assembly::FloppyGeometry::HD_1_2,
    };
    drive.write = assembly::WriteMode::Volatile;
    let cfg = LotechEmsCfg {
        frame: 0xD000,
        ..Default::default()
    };
    config.devices.push(DeviceConfig::new(
        DeviceId::LotechEms,
        DeviceKind::LotechEms(cfg),
        cfg.port_windows(),
    ));
    config
        .memory
        .mmio
        .push(MmioWindow::new(0xD0000, 0x10000, DeviceId::LotechEms));
    let (text, retired) = capture(&config, false);
    // Only FreeDOS MEM's output is inspected; the driver banner is never stored.
    eprintln!(
        "EMS golden retired={retired} hash={:016x}",
        screen_hash(&text)
    );
    for row in text.lines().filter(|row| {
        row.starts_with("Total Expanded (EMS)")
            || row.starts_with("Free Expanded (EMS)")
            || row.trim_start().starts_with("Free EMS memory")
    }) {
        eprintln!("MEM: {row}");
    }
    assert!(
        text.contains("EMS-CHECK-DONE"),
        "DOS page checks did not finish"
    );
    for prefix in ["Total Expanded (EMS)", "Free Expanded (EMS)"] {
        let row = text
            .lines()
            .find(|row| row.starts_with(prefix))
            .expect("FreeDOS MEM EMS row");
        assert!(
            row.contains("2,048K (2,097,152 bytes)"),
            "unexpected EMS size: {row}"
        );
    }
    assert!(text
        .lines()
        .any(|row| row.trim() == "Free EMS memory     2,097,152 bytes"));
    let (mutant, mutant_retired) = capture(&config, true);
    eprintln!(
        "EMS mutant retired={mutant_retired} hash={:016x}",
        screen_hash(&mutant)
    );
    assert!(
        mutant.contains("EMS-CHECK-FAILED"),
        "mutation must fail the DOS page check"
    );
    assert_ne!(
        (screen_hash(&mutant), mutant_retired),
        GOLDEN,
        "aliased page mutation must fail the golden"
    );
    assert_eq!((screen_hash(&text), retired), GOLDEN);
}
