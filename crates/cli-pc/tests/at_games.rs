// SPDX-License-Identifier: AGPL-3.0-or-later
//! Game-ready AT configuration contracts; machine-files.md §Validation in unemupc.
//! IBM Personal Computer AT Technical Reference, System Memory Map and TEST1
//! video-ROM scan; Creative Sound Blaster Series Hardware Programming Guide,
//! §2 Configuring DMA and Interrupt Settings.

use assembly::{AssemblyOptions, BoardModel, DeviceId, DeviceKind, ImageSource, WriteMode};
use machine_file::schema::v1;
use machine_file::FileMachineExt;

use pc_harness::test_support::at_games as support;
pub use pc_harness::test_support::session;

const OPTIONS: AssemblyOptions = AssemblyOptions {
    allow_unlisted_firmware: true,
};

use pc_harness::test_support::fixtures;

// Moving the core pin requires checking the recorded golden again.
const CORE_REV: &str = "779a9ecdc5dafe004165e325b8a6379f70ea5593";

#[path = "../../pc-image/tests/support/core_pin.rs"]
mod core_pin;
use core_pin::assert_core_revision;

#[test]
fn golden_core_revision_matches_lock() {
    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
}

// These values moved when the open profile replaced its MDA console with the
// VGA card and open video BIOS, and when the test began sampling POST. They
// moved again (from 15,241,264 and 41ff081e…) when the open AT BIOS began
// clearing extended memory during POST and its banner named the new release.
// The video BIOS based on upstream 0.9d moved the screen hash (from
// 39d92d28…). Its initialization takes a different time: sampled at the same
// point, POST has finished the parallel-port line and begun the floppy line,
// where with the 0.8a-based ROM it stood inside the parallel-port line. The
// guest's clock origin differs with it, so FreeCOM prints 12:00:21.30 instead
// of 12:00:20.98. The prompt still appears in the same poll window, so the
// count stays.
const OPEN_GOLDEN_RETIRED: u64 = 15_216_991;
const OPEN_GOLDEN_TEXT_SHA256: &str =
    "41ff081e1227af8f1a053df1b627b734de4032963cc0357c488cbbe67e01469e";
// These values moved when extended memory went from 4 MiB to 1 MiB,
// shortening the POST memory test.
const GOLDEN_STOP: Option<(u64, u64)> = Some((0x05E3, 0x1E59));
const GOLDEN_RETIRED: Option<u64> = Some(37_530_263);
const GOLDEN_TEXT_SHA256: Option<&str> =
    Some("89093eaf4577905f0bffb047c6c165b80878a9bd40ef9590a2aa39c0440d8f5f");
const GOLDEN_POST: Option<&[u8]> = Some(&[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
    0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
    0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
    0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37,
    0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
    0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57,
    0x58, 0x59, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67,
    0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77,
    0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F, 0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x8D, 0x8E, 0x8F, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
    0x98, 0x99, 0x9A, 0x9B, 0x9C, 0x9D, 0x9E, 0x9F, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7,
    0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF, 0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7,
    0xB8, 0xB9, 0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF, 0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7,
    0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE, 0xCF, 0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7,
    0xD8, 0xD9, 0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7,
    0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED, 0xEE, 0xEF, 0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
    0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD, 0x09, 0x0A, 0x0B, 0x0C, 0x0E, 0x0F, 0x32, 0x11, 0x12, 0x13,
    0x14, 0x15, 0x16, 0x18, 0x19, 0x81, 0x85, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x23,
    0x23, 0x23, 0x23, 0x23, 0x24, 0x25, 0x26, 0x27, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2F, 0x30, 0x81,
    0x85, 0x31, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32,
    0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32,
    0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32, 0x31, 0x32,
    0x31, 0x32, 0x31, 0x32, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33,
    0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x34, 0xF0, 0x81,
    0x85, 0xF1, 0xB0, 0xF2, 0x9D, 0xF3, 0xF4, 0x95, 0x95, 0xF5, 0xF6, 0x9D, 0xF7, 0xF8, 0xF9, 0xFA,
    0x35, 0x34, 0x35, 0x36, 0x37, 0x38, 0x3A, 0x3C, 0x3D, 0x3E, 0x3B, 0x40, 0x41, 0x43,
]);

#[test]
fn shipped_game_profiles_pin_firmware_and_select_the_intended_displays() {
    for name in [support::IBM, support::OPEN] {
        let (document, _) =
            machine_file::resolve(support::repository().join("machines").join(name)).unwrap();
        let memory = document.memory.as_ref().unwrap();
        assert_eq!(memory.conventional_kib, Some(640));
        assert_eq!(memory.extended_kib, Some(1024));
        let rtc = document.board.as_ref().unwrap().rtc.as_ref().unwrap();
        assert_eq!(rtc.start.as_deref(), Some("2000-01-01T00:00:00"));
        assert_eq!(rtc.nvram, Some(v1::Nvram::Derived));
        let firmware = document.firmware.unwrap();
        let bios = firmware.bios.unwrap();
        let catalogue = BoardModel::Ibm5170.firmware_catalogue();
        let id = if name == support::IBM {
            support::SYSTEM
        } else {
            "unemu-at-bios-1.0.2-unemu.3"
        };
        let entry = catalogue.iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(bios.sha256.unwrap().as_str(), entry.sha256);
        if name == support::IBM {
            let parts = bios.parts.unwrap();
            assert_eq!(parts.len(), 2);
            for (part, known) in parts.iter().zip(entry.parts[0]) {
                assert_eq!(part.sha256.as_ref().unwrap().as_str(), known.sha256);
                assert_eq!(part.offset, known.offset);
                assert_eq!(
                    part.lane,
                    if known.lane == assembly::Lane::Even {
                        v1::Lane::Even
                    } else {
                        v1::Lane::Odd
                    }
                );
            }
        }
        let roms = firmware.option_roms.unwrap();
        // Both video BIOSes fill the C0000h video-ROM window; the open
        // profile's XTIDE ROM follows at C8000h.
        let expected: &[(&str, u32)] = if name == support::IBM {
            &[(support::VIDEO, 0xC0000)]
        } else {
            &[
                (
                    assembly::FirmwareEntry::VGABIOS_0_9D_UNEMU_1_STD.id,
                    0xC0000,
                ),
                ("xub-r638-xtp-ata1f0", 0xC8000),
            ]
        };
        assert_eq!(roms.len(), expected.len());
        for (rom, (id, base)) in roms.iter().zip(expected) {
            let entry = catalogue.iter().find(|entry| entry.id == *id).unwrap();
            assert_eq!(rom.sha256.as_ref().unwrap().as_str(), entry.sha256);
            assert_eq!(rom.base, Some(*base));
            assert_eq!(rom.offset.unwrap_or(0), entry.rom_offset);
        }
        let cards = document.card.unwrap();
        let count = |kind| cards.iter().filter(|card| card.kind == Some(kind)).count();
        assert_eq!(count(v1::CardKind::Vga), 1, "{name}");
        assert_eq!(count(v1::CardKind::VgaText), 0, "{name}");
        assert_eq!(count(v1::CardKind::Mda), 0, "{name}");
    }
}

#[test]
#[ignore = "requires user-supplied IBM system/video ROMs and the FreeDOS 1.2M fixture"]
fn user_rom_at_games_freedos_prompt() {
    use assembly::{
        ConsoleProbe, InjectionSchedule, KeyboardStimulus, ObservationPolicy, ObservationRequest,
        OutputKind, RunPolicy, StopCondition,
    };
    use cpu::{Cpu, CpuReg};
    use machine::RunOutcome;

    let catalogue = BoardModel::Ibm5170.firmware_catalogue();
    let bios = machine_file::require_user_firmware!(support::SYSTEM, catalogue);
    let video = machine_file::require_user_firmware!(support::VIDEO, catalogue);
    let Some(floppy) = fixtures::fixture("freedos/freedos-1200.img") else {
        return;
    };
    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
    let Some(tools) = fixtures::fixture("freedos/freedos-tools-1200.img") else {
        return;
    };
    let profile = support::Profile::new(support::IBM, false);
    let result = (|| -> anyhow::Result<()> {
        let system = source_bytes(&bios.source)?;
        profile.write(
            &format!("vendor/user-roms/{}/6480090.u27", support::SYSTEM),
            &system.iter().step_by(2).copied().collect::<Vec<_>>(),
        );
        profile.write(
            &format!("vendor/user-roms/{}/6480091.u47", support::SYSTEM),
            &system
                .iter()
                .skip(1)
                .step_by(2)
                .copied()
                .collect::<Vec<_>>(),
        );
        profile.write(
            &format!("vendor/user-roms/{}/ibm-vga-1987-32k.bin", support::VIDEO),
            &source_bytes(&video.source)?,
        );
        profile.write("vendor/freedos/freedos-1200.img", &floppy);
        profile.write("vendor/freedos/freedos-tools-1200.img", &tools);
        let loaded = machine_file::load_for_assembly(&profile.file, AssemblyOptions::default())?;
        let policy = ObservationPolicy {
            routes: vec![
                ObservationRequest {
                    device: DeviceId::Vga,
                    output: OutputKind::TextScanout,
                    byte_limit: 0,
                },
                ObservationRequest {
                    device: DeviceId::Post,
                    output: OutputKind::Bytes(device::ByteStreamKind::PostCheckpoints),
                    byte_limit: 1_048_576,
                },
            ],
            ..ObservationPolicy::default()
        };
        let mut assembled = loaded.assemble(&policy)?;
        let cap = 500_000_000;
        let mut outcome = assembled.run_session(&RunPolicy {
            retired_cap: Some(cap),
            idle_limit: Some(pc_harness::BIOS_IDLE_LIMIT),
            stop: StopCondition::PromptThen {
                console: ConsoleProbe {
                    device: DeviceId::Vga,
                },
                needle: pc_harness::DOS_DATE_PROMPT.into(),
                poll_step: pc_harness::DOS_INTERACTIVE_POLL_STEP,
                then_budget: 0,
            },
            stimuli: vec![KeyboardStimulus {
                keyboard_index: assembled.device_index[&DeviceId::Keyboard],
                scancodes: [pc_harness::KEY_ENTER, pc_harness::KEY_ENTER].concat(),
                schedule: InjectionSchedule::AfterPrompt,
            }],
        })?;
        let (frame, dos_prompt) = loop {
            let capture = assembled.capture_text(DeviceId::Vga)?;
            let video::Frame::Text(grid) = &capture.frame else {
                anyhow::bail!("expected VGA text mode at the DOS prompt");
            };
            let dos_prompt = (0..grid.rows()).any(|row| {
                let glyphs: Vec<_> = (0..grid.cols())
                    .map(|col| grid.cell(col, row).unwrap().glyph)
                    .collect();
                glyphs.windows(3).any(|text| text == b"A:>")
                    || glyphs.windows(4).any(|text| text == b"A:\\>")
            });
            if dos_prompt
                || outcome != RunOutcome::RetiredCapReached
                || assembled.machine.virtual_time() >= cap
            {
                break (capture.frame, dos_prompt);
            }
            let next =
                (assembled.machine.virtual_time() + pc_harness::DOS_INTERACTIVE_POLL_STEP).min(cap);
            outcome = assembled.run_session(&RunPolicy::natural(
                Some(next),
                Some(pc_harness::BIOS_IDLE_LIMIT),
            ))?;
        };
        let cpu = assembled.machine.cpu();
        let location = (cpu.code_segment(), cpu.reg(CpuReg::Rip));
        let retired = cpu.retired();
        let hash = assembly::test_support::text_plane_sha256(&frame);
        let post = assembled
            .observation
            .drain_bytes(DeviceId::Post, device::ByteStreamKind::PostCheckpoints)?;
        eprintln!("at-games: stop={outcome:?} cs={:04x} ip={:04x} retired={retired} dos_prompt={dos_prompt} text_sha256={hash} post={post:02x?}", location.0, location.1);
        anyhow::ensure!(dos_prompt, "FreeDOS prompt not reached before the cap");
        assert_eq!(outcome, RunOutcome::RetiredCapReached);
        if let Some(expected) = GOLDEN_STOP {
            assert_eq!(location, expected);
        }
        if let Some(expected) = GOLDEN_RETIRED {
            assert_eq!(retired, expected);
        }
        if let Some(expected) = GOLDEN_TEXT_SHA256 {
            assert_eq!(hash, expected);
        }
        if let Some(expected) = GOLDEN_POST {
            assert_eq!(post, expected);
        }
        assembled.finish_persistence()?;
        Ok(())
    })();
    result.unwrap_or_else(|error| {
        panic!(
            "game profile boot: {}",
            video
                .redact(&bios.redact(&error.to_string()))
                .replace(&profile.tree.path().display().to_string(), "<temporary>")
        )
    });
}

#[test]
#[ignore = "requires user-supplied IBM system/video ROMs and the FreeDOS 1.2M fixture"]
fn user_rom_at_games_ctrl_alt_del_reboot() {
    run_ctrl_alt_del_reboot(None);
}

#[test]
#[ignore = "requires user-supplied IBM system/video ROMs and the FreeDOS 1.2M fixture"]
fn user_rom_at_games_ctrl_alt_del_reboot_frame_chunks() {
    use device::clock::{ClockDomain, Ratio};
    run_ctrl_alt_del_reboot(Some(ClockDomain::from_hz(Ratio::new(60, 1)).ticks(1)));
}

fn run_ctrl_alt_del_reboot(frame_chunk: Option<u64>) {
    use assembly::{
        ConsoleProbe, InjectionSchedule, KeyboardStimulus, ObservationPolicy, ObservationRequest,
        OutputKind, RunPolicy, StopCondition,
    };
    use cpu::{Cpu, CpuReg};
    use machine::RunOutcome;

    // These values moved when extended memory went from 4 MiB to 1 MiB,
    // shortening the POST memory test.
    const REBOOT_STOP: Option<(u64, u64)> = Some((0x05E3, 0x15DD));
    const REBOOT_RETIRED: Option<u64> = Some(51_435_525);
    const REBOOT_TEXT_SHA256: Option<&str> =
        Some("469b33578f197882383cfff84f86c9621593f74df1a97e5d96aae8617d551229");

    let catalogue = BoardModel::Ibm5170.firmware_catalogue();
    let bios = machine_file::require_user_firmware!(support::SYSTEM, catalogue);
    let video = machine_file::require_user_firmware!(support::VIDEO, catalogue);
    let Some(floppy) = fixtures::fixture("freedos/freedos-1200.img") else {
        return;
    };
    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
    let Some(tools) = fixtures::fixture("freedos/freedos-tools-1200.img") else {
        return;
    };
    let profile = support::Profile::new(support::IBM, false);
    let result = (|| -> anyhow::Result<()> {
        let system = source_bytes(&bios.source)?;
        profile.write(
            &format!("vendor/user-roms/{}/6480090.u27", support::SYSTEM),
            &system.iter().step_by(2).copied().collect::<Vec<_>>(),
        );
        profile.write(
            &format!("vendor/user-roms/{}/6480091.u47", support::SYSTEM),
            &system
                .iter()
                .skip(1)
                .step_by(2)
                .copied()
                .collect::<Vec<_>>(),
        );
        profile.write(
            &format!("vendor/user-roms/{}/ibm-vga-1987-32k.bin", support::VIDEO),
            &source_bytes(&video.source)?,
        );
        profile.write("vendor/freedos/freedos-1200.img", &floppy);
        profile.write("vendor/freedos/freedos-tools-1200.img", &tools);
        let loaded = machine_file::load_for_assembly(&profile.file, AssemblyOptions::default())?;
        let policy = ObservationPolicy {
            routes: vec![
                ObservationRequest {
                    device: DeviceId::Vga,
                    output: OutputKind::TextScanout,
                    byte_limit: 0,
                },
                ObservationRequest {
                    device: DeviceId::Post,
                    output: OutputKind::Bytes(device::ByteStreamKind::PostCheckpoints),
                    byte_limit: 1_048_576,
                },
            ],
            ..ObservationPolicy::default()
        };
        let mut assembled = loaded.assemble(&policy)?;
        let boot = |assembled: &mut assembly::AssembledMachine| -> anyhow::Result<_> {
            let cap = assembled.machine.virtual_time() + 500_000_000;
            let policy = RunPolicy {
                retired_cap: Some(cap),
                idle_limit: Some(pc_harness::BIOS_IDLE_LIMIT),
                stop: StopCondition::PromptThen {
                    console: ConsoleProbe {
                        device: DeviceId::Vga,
                    },
                    needle: pc_harness::DOS_DATE_PROMPT.into(),
                    poll_step: pc_harness::DOS_INTERACTIVE_POLL_STEP,
                    then_budget: 0,
                },
                stimuli: vec![KeyboardStimulus {
                    keyboard_index: assembled.device_index[&DeviceId::Keyboard],
                    scancodes: [pc_harness::KEY_ENTER, pc_harness::KEY_ENTER].concat(),
                    schedule: InjectionSchedule::AfterPrompt,
                }],
            };
            let mut outcome = session::run_progressing(assembled, &policy)?;
            let (frame, dos_prompt) = loop {
                let capture = assembled.capture_text(DeviceId::Vga)?;
                let dos_prompt = match &capture.frame {
                    video::Frame::Text(grid) => (0..grid.rows()).any(|row| {
                        let glyphs: Vec<_> = (0..grid.cols())
                            .map(|col| grid.cell(col, row).unwrap().glyph)
                            .collect();
                        glyphs.windows(3).any(|text| text == b"A:>")
                            || glyphs.windows(4).any(|text| text == b"A:\\>")
                    }),
                    _ => false,
                };
                if dos_prompt || assembled.machine.virtual_time() >= cap {
                    break (capture.frame, dos_prompt);
                }
                let next = (assembled.machine.virtual_time()
                    + pc_harness::DOS_INTERACTIVE_POLL_STEP)
                    .min(cap);
                outcome = session::run_progressing(
                    assembled,
                    &RunPolicy::natural(Some(next), Some(pc_harness::BIOS_IDLE_LIMIT)),
                )?;
            };
            anyhow::ensure!(dos_prompt, "FreeDOS prompt not reached before the cap");
            assert_eq!(outcome, RunOutcome::RetiredCapReached);
            Ok((frame, outcome))
        };
        boot(&mut assembled)?;
        assert!(assembled.type_scancodes(&[0x1D, 0x38, 0x53, 0xD3, 0xB8, 0x9D]));
        let clear_cap = assembled.machine.virtual_time() + 100_000_000;
        loop {
            let capture = assembled.capture_text(DeviceId::Vga)?;
            let video::Frame::Text(grid) = &capture.frame else {
                break;
            };
            if !grid
                .rows_as_strings()
                .join("\n")
                .contains(pc_harness::DOS_DATE_PROMPT)
            {
                break;
            }
            anyhow::ensure!(
                assembled.machine.virtual_time() < clear_cap,
                "warm POST reached the cap before clearing the old date prompt"
            );
            let next = (assembled.machine.virtual_time() + pc_harness::DOS_INTERACTIVE_POLL_STEP)
                .min(clear_cap);
            // Keep observation caps fixed while also testing frame-sized run boundaries.
            while assembled.machine.virtual_time() < next {
                let chunk = frame_chunk.map_or(next, |size| {
                    (assembled.machine.virtual_time() + size).min(next)
                });
                session::run_progressing(
                    &mut assembled,
                    &RunPolicy::natural(Some(chunk), Some(pc_harness::BIOS_IDLE_LIMIT)),
                )?;
            }
        }
        let (frame, outcome) = boot(&mut assembled)?;
        let cpu = assembled.machine.cpu();
        let location = (cpu.code_segment(), cpu.reg(CpuReg::Rip));
        let retired = cpu.retired();
        let hash = assembly::test_support::text_plane_sha256(&frame);
        eprintln!("at-games reboot: stop={outcome:?} cs={:04x} ip={:04x} retired={retired} text_sha256={hash}", location.0, location.1);
        if let Some(expected) = REBOOT_STOP {
            assert_eq!(location, expected);
        }
        if let Some(expected) = REBOOT_RETIRED {
            assert_eq!(retired, expected);
        }
        if let Some(expected) = REBOOT_TEXT_SHA256 {
            assert_eq!(hash, expected);
        }
        assembled.finish_persistence()?;
        Ok(())
    })();
    result.unwrap_or_else(|error| {
        panic!(
            "game profile reboot: {}",
            video
                .redact(&bios.redact(&error.to_string()))
                .replace(&profile.tree.path().display().to_string(), "<temporary>")
        )
    });
}

fn source_bytes(source: &ImageSource) -> anyhow::Result<Vec<u8>> {
    Ok(match source {
        ImageSource::File(path) => std::fs::read(path)?,
        ImageSource::Interleaved(parts) => {
            let parts = parts
                .iter()
                .map(|part| Ok((part.lane, part.offset, std::fs::read(&part.path)?)))
                .collect::<std::io::Result<Vec<_>>>()?;
            assembly::interleave_parts(&parts)?
        }
        _ => anyhow::bail!("user firmware discovery must return file-backed sources"),
    })
}

#[test]
fn synthetic_game_profiles_lower_and_pass_machine_check_without_opening_journals() {
    for name in [support::IBM, support::OPEN] {
        let profile = support::Profile::new(name, true);
        let error = machine_file::load_with_options(&profile.file, OPTIONS).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("system BIOS unlisted has no known video-ROM scan"),
            "{error}"
        );
        profile.relocate_synthetic_video();
        let loaded = machine_file::load_for_assembly(&profile.file, OPTIONS).unwrap();
        let config = &loaded.config;
        assert_eq!(config.board, BoardModel::Ibm5170);
        assert_eq!(config.cpu.fpu, assembly::FpuChip::None);
        assert_eq!(config.memory.address_space_bytes, 2 * 1024 * 1024);
        assert!(config
            .memory
            .ram
            .contains(&assembly::RamRegion::new(0x100000, 1024 * 1024)));
        assert!(config
            .memory
            .ram
            .iter()
            .any(|ram| ram.base == 0 && ram.size >= 640 * 1024));
        let sb = config
            .devices
            .iter()
            .find(|device| device.id == DeviceId::Sb16)
            .unwrap();
        assert!(matches!(sb.kind, DeviceKind::Sb16(cfg) if cfg.base == 0x220 && cfg.dma16 == 5));
        assert_eq!(sb.irq, Some(5));
        assert_eq!(sb.dma.unwrap().channel, 1);
        for port in [
            0x220, 0x223, 0x224, 0x225, 0x226, 0x228, 0x229, 0x22A, 0x22C, 0x22E, 0x22F, 0x388,
            0x389, 0x38A, 0x38B,
        ] {
            assert!(
                sb.ports
                    .iter()
                    .any(|range| range.start <= port && port <= range.end),
                "missing SB16/OPL3 port {port:x}"
            );
        }
        let uart = config
            .devices
            .iter()
            .find(|device| device.id == DeviceId::Uart)
            .unwrap();
        assert!(matches!(uart.kind, DeviceKind::Uart(cfg) if cfg.base == 0x3F8 && cfg.mouse));
        assert_eq!(uart.irq, Some(4));
        assert!(config
            .devices
            .iter()
            .any(|device| matches!(device.kind, DeviceKind::Keyboard(cfg) if !cfg.mouse)));
        let ide = config
            .devices
            .iter()
            .find(|device| device.id == DeviceId::Ide)
            .unwrap();
        assert!(
            matches!(ide.kind, DeviceKind::Ide(cfg) if cfg.base == 0x1F0 && cfg.control == 0x3F6 && cfg.model == if name == support::IBM { assembly::AtaControllerModel::AtAdapter } else { assembly::AtaControllerModel::Ata })
        );
        assert_eq!(ide.irq, Some(14));
        // The open profile adds an empty 3.5-inch 1.44M B:; the June 1985 IBM
        // BIOS profile keeps A: only.
        assert_eq!(
            config.drives.len(),
            if name == support::OPEN { 3 } else { 2 }
        );
        let b = config
            .drives
            .iter()
            .find(|drive| drive.controller == DeviceId::Fdc && drive.unit == 1);
        if name == support::OPEN {
            let b = b.unwrap();
            assert_eq!(
                b.kind,
                assembly::DriveKind::Floppy {
                    geometry: assembly::FloppyGeometry::HD_1_44
                }
            );
            let media = b.media.as_ref().unwrap();
            assert!(media.empty && media.entries.is_empty());
            assert_eq!(assembly::derive_cmos_config(config).unwrap()[0x10], 0x24);
        } else {
            assert!(b.is_none());
        }
        let disk = config
            .drives
            .iter()
            .find(|drive| drive.controller == DeviceId::Ide)
            .unwrap();
        assert_eq!(disk.unit, 0);
        assert_eq!(disk.kind.image_bytes(), 615 * 4 * 17 * 512);
        assert_eq!(disk.write, WriteMode::journal(profile.journal()));
        assert!(matches!(disk.image, ImageSource::File(_)));
        assert!(
            disk.expected_sha256.is_some(),
            "the entire journal base must be hashed"
        );
        let floppy = config
            .drives
            .iter()
            .find(|drive| drive.controller == DeviceId::Fdc && drive.unit == 0)
            .unwrap();
        assert_eq!(floppy.unit, 0);
        assert_eq!(floppy.kind.image_bytes(), 1_228_800);
        let media = floppy.media.as_ref().unwrap();
        assert!(!media.empty);
        assert_eq!(media.entries.len(), 3);
        assert_eq!(media.entries[0].name, "freedos-1200.img");
        assert_eq!(media.entries[1].name, "freedos-tools-1200.img");
        assert_eq!(media.entries[2].name, "transfer-1200.img");
        assert!(media
            .entries
            .iter()
            .all(|entry| entry.write == WriteMode::ReadOnly && entry.expected_sha256.is_some()));
        // The positive synthetic smoke moved the video ROM out of the video
        // window. Derive the display selection from the shipped C0000h layout.
        let mut shipped = config.clone();
        shipped.images[1].base = 0xC0000;
        assert_eq!(
            shipped.primary_display().unwrap(),
            assembly::PrimaryDisplay::OwnBios
        );
        assert!(config
            .devices
            .iter()
            .any(|device| matches!(device.kind, DeviceKind::VgaRegisters(_))));
        assert!(!config
            .devices
            .iter()
            .any(|device| matches!(device.kind, DeviceKind::Mda(_) | DeviceKind::Vga(_))));
        if name == support::IBM {
            assert!(matches!(
                config.images[1].source,
                ImageSource::RomContainer { offset: 8192, .. }
            ));
            assert_eq!(config.images[1].source.known_len(), Some(24576));
        } else {
            // The open video BIOS is a raw ROM: every byte is mapped.
            assert!(matches!(config.images[1].source, ImageSource::Inline(_)));
            assert_eq!(config.images[1].source.known_len(), Some(32768));
            assert_eq!(config.images[2].base, 0xC8000);
        }
        let args = [
            "machine".into(),
            "check".into(),
            profile.file.clone().into_os_string(),
            "--allow-unlisted-firmware".into(),
        ];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            unemupc::run(&args, &mut out, &mut err),
            0,
            "{}",
            String::from_utf8_lossy(&err)
        );
        assert!(!profile.journal().exists());
        assert!(!profile.journal().with_extension("upfj.lock").exists());
    }
}

fn open_fixture_profile() -> Option<support::Profile> {
    let mut images = Vec::new();
    for relative in [
        "unemu-at-bios/bios-unemu-at.bin",
        "vgabios/vgabios-std.bin",
        "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
        "freedos/freedos-1200.img",
        "freedos/freedos-tools-1200.img",
    ] {
        images.push((relative, fixtures::fixture(relative)?));
    }
    let profile = support::Profile::new(support::OPEN, false);
    for (relative, bytes) in images {
        profile.write(&format!("vendor/{relative}"), &bytes);
    }
    let directory = profile.tree.path().join("machines/local");
    for (name, options) in [
        ("games.img", vec!["hd", "--chs", "615/4/17"]),
        (
            "transfer-1200.img",
            vec!["floppy", "--type", "1.2M", "--format"],
        ),
    ] {
        let path = directory.join(name);
        std::fs::remove_file(&path).unwrap();
        let mut args: Vec<std::ffi::OsString> = ["media", "create"]
            .into_iter()
            .chain(options)
            .map(Into::into)
            .collect();
        args.push(path.into_os_string());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            cli_pc::command::run(&args, &mut out, &mut err),
            0,
            "{}",
            String::from_utf8_lossy(&err)
        );
    }
    Some(profile)
}

#[test]
fn open_game_profile_checks_real_pinned_fixtures_without_creating_journals() {
    let Some(profile) = open_fixture_profile() else {
        return;
    };
    let args = [
        "machine".into(),
        "check".into(),
        profile.file.clone().into_os_string(),
    ];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        unemupc::run(&args, &mut out, &mut err),
        0,
        "{}",
        String::from_utf8_lossy(&err)
    );
    assert!(!profile.journal().exists());
    assert!(!profile.journal().with_extension("upfj.lock").exists());

    let bios = profile
        .tree
        .path()
        .join("vendor/unemu-at-bios/bios-unemu-at.bin");
    let mut bytes = std::fs::read(&bios).unwrap();
    bytes[0] ^= 1;
    std::fs::write(bios, bytes).unwrap();
    out.clear();
    err.clear();
    assert_eq!(unemupc::run(&args, &mut out, &mut err), 1);
    assert!(String::from_utf8_lossy(&err).contains("sha256 mismatch"));
    assert!(!profile.journal().exists());
    assert!(!profile.journal().with_extension("upfj.lock").exists());
}

#[test]
#[ignore = "slow: open AT BIOS/VGA/XTIDE POST and FreeDOS 1.2M boot with a blank type-2 HDD"]
fn open_game_profile_boots_freedos() {
    use assembly::{
        ConsoleProbe, InjectionSchedule, KeyboardStimulus, ObservationPolicy, ObservationRequest,
        OutputKind, RunPolicy, StopCondition,
    };
    use cpu::Cpu;
    use machine::RunOutcome;

    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
    let Some(profile) = open_fixture_profile() else {
        return;
    };
    let loaded =
        machine_file::load_for_assembly(&profile.file, AssemblyOptions::default()).unwrap();
    assert_eq!(
        loaded.config.primary_display().unwrap(),
        assembly::PrimaryDisplay::OwnBios
    );
    let policy = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Vga,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..ObservationPolicy::default()
    };
    let mut assembled = loaded.assemble(&policy).unwrap();
    let cap = 100_000_000;
    // FreeDOS scrolls the POST screen away, so sample it while the open AT
    // BIOS's display line is visible: POST found and initialized the video BIOS.
    let post_screen = loop {
        let next = assembled.machine.virtual_time() + 100_000;
        assert!(next < cap, "POST never reported the video BIOS");
        let outcome = assembled
            .run_session(&RunPolicy::natural(
                Some(next),
                Some(pc_harness::BIOS_IDLE_LIMIT),
            ))
            .unwrap();
        assert_eq!(outcome, RunOutcome::RetiredCapReached);
        let capture = assembled.capture_text(DeviceId::Vga).unwrap();
        if let video::Frame::Text(grid) = &capture.frame {
            let text = grid.rows_as_strings().join("\n");
            if text.contains("EGA/VGA (Video BIOS Present)") {
                break text;
            }
        }
    };
    eprintln!("at-games-open POST:\n{post_screen}");
    assert!(
        post_screen.contains("Bochs VGABios 0.9d-unemu"),
        "{post_screen}"
    );
    // The open AT BIOS signs on and identifies the 80286 without the 8088
    // interrupt warning.
    for line in ["unemu AT BIOS, Version 1.0.2-unemu.3", "Intel 80286"] {
        assert!(post_screen.contains(line), "{line}: {post_screen}");
    }
    let mut outcome = assembled
        .run_session(&RunPolicy {
            retired_cap: Some(cap),
            idle_limit: Some(pc_harness::BIOS_IDLE_LIMIT),
            stop: StopCondition::PromptThen {
                console: ConsoleProbe {
                    device: DeviceId::Vga,
                },
                needle: pc_harness::DOS_DATE_PROMPT.into(),
                poll_step: pc_harness::DOS_INTERACTIVE_POLL_STEP,
                then_budget: 0,
            },
            stimuli: vec![KeyboardStimulus {
                keyboard_index: assembled.device_index[&DeviceId::Keyboard],
                scancodes: [pc_harness::KEY_ENTER, pc_harness::KEY_ENTER].concat(),
                schedule: InjectionSchedule::AfterPrompt,
            }],
        })
        .unwrap();
    let (frame, dos_prompt) = loop {
        let capture = assembled.capture_text(DeviceId::Vga).unwrap();
        let video::Frame::Text(grid) = &capture.frame else {
            panic!("expected VGA text during FreeDOS boot");
        };
        let text = grid.rows_as_strings().join("\n");
        let dos_prompt = text.contains("A:\\>") || text.contains("A:>");
        if dos_prompt
            || outcome != RunOutcome::RetiredCapReached
            || assembled.machine.virtual_time() >= cap
        {
            eprintln!("at-games-open console:\n{text}");
            break (capture.frame, dos_prompt);
        }
        let next =
            (assembled.machine.virtual_time() + pc_harness::DOS_INTERACTIVE_POLL_STEP).min(cap);
        outcome = assembled
            .run_session(&RunPolicy::natural(
                Some(next),
                Some(pc_harness::BIOS_IDLE_LIMIT),
            ))
            .unwrap();
    };
    let retired = assembled.machine.cpu().retired();
    let text_hash = assembly::test_support::text_plane_sha256(&frame);
    eprintln!("at-games-open: stop={outcome:?} retired={retired} dos_prompt={dos_prompt} text_sha256={text_hash}");
    assert_eq!(outcome, RunOutcome::RetiredCapReached);
    assert!(dos_prompt, "FreeDOS did not reach the A: prompt");
    assert_eq!(retired, OPEN_GOLDEN_RETIRED);
    assert_eq!(text_hash, OPEN_GOLDEN_TEXT_SHA256);
    assembled.finish_persistence().unwrap();
}

/// Set-1 make/break codes for a FreeDOS command line: lowercase letters,
/// digits, space, period, colon (shifted) and Enter.
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
            b'.' => (0x34, false),
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

/// Add one root-directory file to a freshly formatted FAT12 floppy, its one
/// cluster at `cluster`, so reading it moves the head away from cylinder 0.
/// Microsoft FAT Specification, BPB, FAT12 entries and directory entries.
fn add_fat12_file(image: &mut [u8], name: &[u8; 11], cluster: u16, data: &[u8]) {
    let word = |offset: usize| usize::from(u16::from_le_bytes([image[offset], image[offset + 1]]));
    let (sector, cluster_sectors, reserved) = (word(11), usize::from(image[13]), word(14));
    let (fats, root_entries, fat_sectors) = (usize::from(image[16]), word(17), word(22));
    let root = (reserved + fats * fat_sectors) * sector;
    let data_start = root + root_entries * 32;
    for fat in 0..fats {
        let entry = (reserved + fat * fat_sectors) * sector + usize::from(cluster) * 3 / 2;
        if cluster.is_multiple_of(2) {
            image[entry] = 0xFF;
            image[entry + 1] |= 0x0F;
        } else {
            image[entry] |= 0xF0;
            image[entry + 1] = 0xFF;
        }
    }
    let slot = (0..root_entries)
        .map(|index| root + index * 32)
        .find(|&offset| image[offset] == 0)
        .unwrap();
    image[slot..slot + 11].copy_from_slice(name);
    image[slot + 11] = 0x20;
    image[slot + 24..slot + 26].copy_from_slice(&0x21u16.to_le_bytes());
    image[slot + 26..slot + 28].copy_from_slice(&cluster.to_le_bytes());
    image[slot + 28..slot + 32].copy_from_slice(&(data.len() as u32).to_le_bytes());
    let offset = data_start + usize::from(cluster - 2) * cluster_sectors * sector;
    image[offset..offset + data.len()].copy_from_slice(data);
}

// MEASURED with the CPU revision selected by the core pin above when the open
// profile gained its empty B:,
// and again (from 22,988,197 and b9e3a1cb…) when the open AT BIOS began
// clearing extended memory during POST and its banner named the new release.
// The video BIOS based on upstream 0.9d moved the screen hash again (from
// 0bebf03e…): its POST takes a different time, so the guest's clock origin
// differs and FreeCOM prints another time of day, as in the boot golden
// above; the count stays.
const MIXED_DENSITY_RETIRED: u64 = 23_463_925;
const MIXED_DENSITY_TEXT_SHA256: &str =
    "b4e1ac642b435d2d215d87ddd14c02ac22fae92a6b03da0f77b6b49ff237d83e";

/// FreeDOS on the open profile reads a 720K disk inserted into the empty
/// 1.44M B: (250 kbps) and a 360K disk swapped into the 1.2M A: (300 kbps,
/// double-stepped). Each file sits beyond cylinder 0, so TYPE seeks: the 360K
/// file's track 11 is physical cylinder 22 of the 1.2M drive. Both disks go in
/// through the Media › Insert path (append, then insert at a run boundary).
#[test]
#[ignore = "slow: open AT BIOS POST, FreeDOS 1.2M boot, then a 720K disk in B: and a 360K disk in A:"]
fn open_game_profile_reads_720k_in_b_and_360k_in_a() {
    use assembly::{
        ConsoleProbe, FloppyAction, FloppyMedium, InjectionSchedule, KeyboardStimulus,
        ObservationPolicy, ObservationRequest, OutputKind, RunPolicy, StopCondition,
    };
    use cpu::Cpu;
    use machine::RunOutcome;

    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
    let Some(profile) = open_fixture_profile() else {
        return;
    };
    let directory = profile.tree.path().join("machines/local");
    let disk = |file: &str, format: &str, label: &str, name: &[u8; 11], cluster, text: &[u8]| {
        let path = directory.join(file);
        let args: Vec<std::ffi::OsString> = [
            "media", "create", "floppy", "--type", format, "--format", "--label", label,
        ]
        .into_iter()
        .map(Into::into)
        .chain([path.clone().into_os_string()])
        .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            cli_pc::command::run(&args, &mut out, &mut err),
            0,
            "{}",
            String::from_utf8_lossy(&err)
        );
        let mut image = std::fs::read(&path).unwrap();
        add_fat12_file(&mut image, name, cluster, text);
        std::fs::write(&path, image).unwrap();
        FloppyMedium {
            image: ImageSource::File(path),
            expected_sha256: None,
            write: WriteMode::ReadOnly,
            name: file.into(),
        }
    };
    // 720K: data start 14, two sectors per cluster; cluster 300 is cylinder 33.
    let dd720 = disk(
        "games-720.img",
        "720K",
        "GAMES720",
        b"GAME720 TXT",
        300,
        b"seven-twenty in b\r\n",
    );
    // 360K: data start 12, two sectors per cluster; cluster 100 is track 11.
    let dd360 = disk(
        "old-360.img",
        "360K",
        "OLD360",
        b"OLD360  TXT",
        100,
        b"three-sixty in a\r\n",
    );

    let loaded =
        machine_file::load_for_assembly(&profile.file, AssemblyOptions::default()).unwrap();
    let mut config = loaded.config.clone();
    assert_eq!(assembly::derive_cmos_config(&config).unwrap()[0x10], 0x24);
    let policy = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Vga,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..ObservationPolicy::default()
    };
    let mut assembled = loaded.assemble(&policy).unwrap();
    let keyboard = assembled.device_index[&DeviceId::Keyboard];
    let cap = 400_000_000;
    let screen = |assembled: &mut assembly::AssembledMachine| {
        let capture = assembled.capture_text(DeviceId::Vga).unwrap();
        let video::Frame::Text(grid) = &capture.frame else {
            panic!("expected VGA text");
        };
        let text = grid.rows_as_strings().join("\n");
        (text, capture.frame)
    };
    let run_until = |assembled: &mut assembly::AssembledMachine, done: &dyn Fn(&str) -> bool| loop {
        let (text, frame) = screen(assembled);
        if done(&text) {
            return (text, frame);
        }
        let next = assembled.machine.virtual_time() + 200_000;
        assert!(next < cap, "budget exhausted:\n{text}");
        let outcome = session::run_progressing(
            assembled,
            &RunPolicy::natural(Some(next), Some(pc_harness::BIOS_IDLE_LIMIT)),
        )
        .unwrap();
        assert_eq!(outcome, RunOutcome::RetiredCapReached, "\n{text}");
    };
    let type_line = |assembled: &mut assembly::AssembledMachine, line: &str| {
        // Pace characters so the BIOS typeahead ring never fills.
        for character in format!("{line}\r").chars() {
            assembly::inject_scancodes(
                &mut assembled.machine,
                keyboard,
                &keys(&character.to_string()),
            );
            let next = assembled.machine.virtual_time() + 100_000;
            session::run_progressing(
                assembled,
                &RunPolicy::natural(Some(next), Some(pc_harness::BIOS_IDLE_LIMIT)),
            )
            .unwrap();
        }
    };
    let outcome = assembled
        .run_session(&RunPolicy {
            retired_cap: Some(cap),
            idle_limit: Some(pc_harness::BIOS_IDLE_LIMIT),
            stop: StopCondition::PromptThen {
                console: ConsoleProbe {
                    device: DeviceId::Vga,
                },
                needle: pc_harness::DOS_DATE_PROMPT.into(),
                poll_step: pc_harness::DOS_INTERACTIVE_POLL_STEP,
                then_budget: 0,
            },
            stimuli: vec![KeyboardStimulus {
                keyboard_index: keyboard,
                scancodes: [pc_harness::KEY_ENTER, pc_harness::KEY_ENTER].concat(),
                schedule: InjectionSchedule::AfterPrompt,
            }],
        })
        .unwrap();
    assert_eq!(outcome, RunOutcome::RetiredCapReached);
    let prompt = |text: &str| text.trim_end().ends_with("A:\\>");
    run_until(&mut assembled, &prompt);

    let index = assembled
        .append_floppy(DeviceId::Fdc, 1, &mut config, &dd720)
        .unwrap();
    assembled
        .change_floppy(DeviceId::Fdc, 1, FloppyAction::Insert(index))
        .unwrap();
    type_line(&mut assembled, "dir b:");
    run_until(&mut assembled, &|text| {
        text.contains("GAME720") && prompt(text)
    });
    type_line(&mut assembled, "type b:game720.txt");
    let (b_text, _) = run_until(&mut assembled, &|text| {
        text.contains("seven-twenty in b") && prompt(text)
    });
    eprintln!("at-games-open B: 720K:\n{b_text}");
    for line in [" Volume in drive B is GAMES720", "729,088 bytes free"] {
        assert!(b_text.contains(line), "{line}: {b_text}");
    }

    let index = assembled
        .append_floppy(DeviceId::Fdc, 0, &mut config, &dd360)
        .unwrap();
    assembled
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(index))
        .unwrap();
    type_line(&mut assembled, "dir a:");
    run_until(&mut assembled, &|text| {
        text.contains("OLD360") && prompt(text)
    });
    type_line(&mut assembled, "type a:old360.txt");
    let (a_text, frame) = run_until(&mut assembled, &|text| {
        text.contains("three-sixty in a") && prompt(text)
    });
    eprintln!("at-games-open A: 360K:\n{a_text}");
    for line in [" Volume in drive A is OLD360", "361,472 bytes free"] {
        assert!(a_text.contains(line), "{line}: {a_text}");
    }
    let fdc = assembled.device_index[&DeviceId::Fdc];
    let fdc = assembled
        .machine
        .bus_mut()
        .device_mut(fdc)
        .unwrap()
        .as_any_mut()
        .unwrap()
        .downcast_mut::<devices::Upd765>()
        .unwrap();
    // The 360K file's track 11 was read with the head at physical cylinder 22.
    assert_eq!(fdc.present_cylinder(0), 22);
    assert_eq!(fdc.drive_type(1), Some(devices::DriveType::HD1440));
    let retired = assembled.machine.cpu().retired();
    let text_hash = assembly::test_support::text_plane_sha256(&frame);
    eprintln!("at-games-open mixed density: retired={retired} text_sha256={text_hash}");
    assert_eq!(retired, MIXED_DENSITY_RETIRED);
    assert_eq!(text_hash, MIXED_DENSITY_TEXT_SHA256);
    assembled.finish_persistence().unwrap();
}
