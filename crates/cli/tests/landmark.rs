// SPDX-License-Identifier: AGPL-3.0-or-later
//! User-supplied Landmark AT real-mode-prefix golden.
//!
//! Landmark Research International, ROM POST User's Manual v1.2, pp. 16–24:
//! the first twelve tests precede the protected-mode test. IBM Personal
//! Computer Technical Reference, Monochrome Display and Printer Adapter,
//! Programming Considerations: text cells hold glyph/attribute byte pairs.
//! See `backlog/docs/design/compatibility-diagnostics.md` §2–3.

use cpu::{Cpu, CpuError, CpuReg};
use machine::{MachineError, RunOutcome};
use video::{Frame, TextGrid};

use assembly::test_support::{FoundUserFirmware, TempDir};
use assembly::{
    AssemblyOptions, BoardModel, DeviceId, ImageSource, ObservationPolicy, ObservationRequest,
    OutputKind, RunError, RunPolicy,
};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

const ENTRY_ID: &str = "landmark-rom-post-at-1.2.0";
const CPU_REV: &str = "a92b676e081ce862dcb48c82645b3d3a8aa2364f";
const STEP_BUDGET: u64 = 30_000_000;
const GOLDEN_RETIRED: u64 = 15_822_937;
const GOLDEN_TEXT_SHA256: &str = "373ae9a68d11ea20d8da2232d39a90cdbb1b6f573b9fb28ea04e3ab79d3b31d1";

const ROW_IDS: [&str; 12] = [
    "cpu",
    "rom_a",
    "rom_b",
    "pit_ch0",
    "pit_ch1",
    "pit_ch2",
    "dma1",
    "dma2",
    "dma_page",
    "kbc_parity",
    "mem_16k",
    "mem_refresh",
];

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Passed,
    Failed,
    NotPresent,
    Empty,
    Unrecognized,
}

fn verdict(status: &[u8]) -> Verdict {
    let start = status.iter().position(|byte| !matches!(byte, 0 | b' '));
    let end = status.iter().rposition(|byte| !matches!(byte, 0 | b' '));
    match start.zip(end).map(|(start, end)| &status[start..=end]) {
        None => Verdict::Empty,
        Some(b"PASSED") => Verdict::Passed,
        Some(b"FAILED") => Verdict::Failed,
        Some(b"NOT PRESENT") => Verdict::NotPresent,
        Some(_) => Verdict::Unrecognized,
    }
}

fn row_verdict(grid: &TextGrid, row: u16) -> Verdict {
    // Fixed status columns classify verdicts without reading proprietary row labels.
    let status: Vec<_> = (35..47)
        .map(|col| grid.cell(col, row).expect("status cell in MDA grid").glyph)
        .collect();
    verdict(&status)
}

fn run_result_summary(result: &Result<RunOutcome, RunError>) -> String {
    // Free-form backend diagnostics may contain byte dumps. Typed errors retain
    // their variant and address fields; no guest memory is read for this report.
    match result {
        Err(RunError::Machine(MachineError::Cpu(error))) => match error {
            CpuError::UnsupportedInstruction { addr } => {
                format!("Err(Machine(Cpu(UnsupportedInstruction {{ addr: {addr:#x} }})))")
            }
            CpuError::Unmapped { addr } => {
                format!("Err(Machine(Cpu(Unmapped {{ addr: {addr:#x} }})))")
            }
            CpuError::Memory(_) => "Err(Machine(Cpu(Memory(<details omitted>))))".into(),
            CpuError::UnexpectedExit { .. } => {
                "Err(Machine(Cpu(UnexpectedExit { <details omitted> })))".into()
            }
        },
        Err(RunError::Persistence(_)) => "Err(Persistence(<details omitted>))".into(),
        Err(RunError::ConsoleUnavailable { device, .. }) => {
            format!("Err(ConsoleUnavailable {{ device: {device:?} }})")
        }
        _ => format!("{result:?}"),
    }
}

fn assert_prefix_stop(
    assembled: &assembly::AssembledMachine,
    result: Result<RunOutcome, RunError>,
) {
    let cpu = assembled.machine.cpu();
    // UnsupportedInstruction.addr is the offending RIP offset in Real16.
    // Match the offset and code selector together to identify the instruction.
    assert!(
        matches!(
            result,
            Err(RunError::Machine(MachineError::Cpu(
                CpuError::UnsupportedInstruction { addr: 0xA3CE }
            )))
        ) && cpu.code_segment() == 0xF000,
        "expected unsupported instruction at F000:A3CE; result={}; \
         cs={:#x}; rip={:#x}; retired={}; completed_steps={}; virtual_time={}",
        run_result_summary(&result),
        cpu.code_segment(),
        cpu.reg(CpuReg::Rip),
        cpu.retired(),
        cpu.completed_steps(),
        assembled.machine.virtual_time(),
    );
}

fn synthetic_stop_machine(program: &[u8]) -> assembly::AssembledMachine {
    let temporary = TempDir::new("landmark-synthetic");
    let mut rom = vec![0xF4; 65536];
    rom[0xFFF0..0xFFF5].copy_from_slice(&[0xEA, 0xCE, 0xA3, 0, 0xF0]);
    rom[0xA3CE..0xA3CE + program.len()].copy_from_slice(program);
    let image = temporary.path().join("synthetic.bin");
    std::fs::write(&image, rom).unwrap();
    let file = write_machine_file(temporary.path(), &ImageSource::File(image)).unwrap();
    let loaded = machine_file::load_for_assembly(
        file,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    loaded.assemble(&mda_policy()).unwrap()
}

fn run_policy(budget: u64) -> RunPolicy {
    RunPolicy::natural(Some(budget), Some(cli::BIOS_IDLE_LIMIT))
}

fn mda_policy() -> ObservationPolicy {
    ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Mda,
            output: OutputKind::TextScanout,
            byte_limit: 1_048_576,
        }],
        ..ObservationPolicy::default()
    }
}

fn quoted_path(path: &Path) -> anyhow::Result<String> {
    let absolute = path.canonicalize()?;
    let text = absolute
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF-8 firmware path"))?;
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            character if character.is_control() => {
                write!(quoted, "\\u{:04x}", u32::from(character))?
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn write_machine_file(directory: &Path, source: &ImageSource) -> anyhow::Result<PathBuf> {
    let mut text = String::from(
        "schema=1\nextends='preset:ibm-5170'\n\
         [board.rtc]\nstart='2000-01-01T00:00:00'\nnvram='derived'\n\
         [firmware.bios]\nprovides=[]\n",
    );
    match source {
        ImageSource::File(path) => writeln!(text, "image={}", quoted_path(path)?)?,
        ImageSource::Interleaved(parts) => {
            text.push_str("parts=[\n");
            for part in parts {
                let lane = match part.lane {
                    assembly::Lane::Even => "even",
                    assembly::Lane::Odd => "odd",
                };
                writeln!(
                    text,
                    "{{image={},lane='{lane}',offset={}}},",
                    quoted_path(&part.path)?,
                    part.offset
                )?;
            }
            text.push_str("]\n");
        }
        _ => anyhow::bail!("discovery must return file-backed firmware"),
    }
    let path = directory.join("diagnostic.toml");
    std::fs::write(&path, text)?;
    Ok(path)
}

fn expect_redacted<T, E: std::fmt::Display>(
    found: &FoundUserFirmware,
    result: Result<T, E>,
    temporary: &Path,
    context: &str,
) -> T {
    found.expect_redacted(
        result.map_err(|error| redact_machine_error(&error.to_string(), temporary)),
        context,
    )
}

fn redact_machine_error(error: &str, temporary: &Path) -> String {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut message = error.to_owned();
    for (root, label) in [(repository, "<repository>"), (temporary, "<test-machine>")] {
        for path in [Some(root.to_path_buf()), root.canonicalize().ok()]
            .into_iter()
            .flatten()
        {
            message = message.replace(&path.display().to_string(), label);
            let debug = format!("{path:?}");
            message = message.replace(&debug[1..debug.len() - 1], label);
        }
    }
    message
}

#[test]
fn landmark_machine_errors_redact_temporary_and_repository_paths() {
    let temporary = TempDir::new("landmark-redaction");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    for (path, label) in [
        (temporary.path(), "<test-machine>"),
        (repository, "<repository>"),
    ] {
        let error = format!(
            "could not load {:?} at {}",
            path.join("diagnostic.toml"),
            path.display()
        );
        let redacted = redact_machine_error(&error, temporary.path());
        assert!(redacted.contains(label));
        assert!(!redacted.contains(&path.display().to_string()));
    }
}

#[test]
fn landmark_frontend_path_reads_board_adapter_gap() {
    let mut assembled =
        synthetic_stop_machine(&[0xB8, 0, 0xC0, 0x8E, 0xD8, 0xA0, 1, 0, 0x0F, 0x01, 0x0F]);
    let result = assembled.run_session(&run_policy(16));
    assert!(
        matches!(
            result,
            Err(RunError::Machine(MachineError::Cpu(
                CpuError::UnsupportedInstruction { addr: 0xA3D6 }
            )))
        ),
        "adapter read must reach the synthetic SIDT: {}",
        run_result_summary(&result)
    );
    assert_eq!(assembled.machine.cpu().reg(CpuReg::Rax) & 0xFF, 0);
}

#[test]
fn landmark_machine_file_preserves_absolute_parts_and_preset_settings() {
    let temporary = TempDir::new("landmark-parts");
    let even = temporary.path().join("even-\"-\\-\n.bin");
    let odd = temporary.path().join("odd.bin");
    std::fs::write(&even, vec![0x12; 32768]).unwrap();
    std::fs::write(&odd, vec![0x34; 32768]).unwrap();
    let source = ImageSource::Interleaved(vec![
        assembly::ImagePart {
            path: even.clone(),
            lane: assembly::Lane::Even,
            offset: 0,
        },
        assembly::ImagePart {
            path: odd.clone(),
            lane: assembly::Lane::Odd,
            offset: 0,
        },
    ]);
    let file = write_machine_file(temporary.path(), &source).unwrap();
    let (document, _) = machine_file::resolve(&file).unwrap();
    let parts = document.firmware.unwrap().bios.unwrap().parts.unwrap();
    assert_eq!(parts.len(), 2);
    for (part, (expected, lane)) in parts.iter().zip([
        (even, machine_file::schema::v1::Lane::Even),
        (odd, machine_file::schema::v1::Lane::Odd),
    ]) {
        assert_eq!(part.lane, lane);
        assert_eq!(part.offset, 0);
        assert!(Path::new(&part.image).is_absolute());
        assert!(Path::new(&part.image) == expected.canonicalize().unwrap());
    }
    let loaded = machine_file::load_for_assembly(
        file,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let ImageSource::Inline(bytes) = &loaded.config.images[0].source else {
        panic!("expected lowered synthetic parts")
    };
    assert!(*bytes == [0x12, 0x34].repeat(32768), "part order changed");
    assert!(loaded.config.drives.is_empty());
    assert!(loaded.config.images[0].provides.is_empty());
    assert_eq!(loaded.config.images[0].base, 0xF0000);
    assert_eq!(
        loaded.config.memory.ram,
        vec![assembly::RamRegion::new(0, 0xB0000)]
    );
    assert_eq!(loaded.config.clock.retired_per_pit_clock, 1);
    assert_eq!(loaded.config.clock.retired_per_pit_clock_denominator, 1);
    for id in [DeviceId::Mda, DeviceId::Vga, DeviceId::Fdc] {
        assert!(loaded.config.devices.iter().any(|device| device.id == id));
    }
    let rtc = loaded
        .config
        .devices
        .iter()
        .find_map(|device| match &device.kind {
            assembly::DeviceKind::Rtc(rtc) => Some(rtc),
            _ => None,
        })
        .unwrap();
    assert_eq!(rtc.start, assembly::RtcStart::default());
    assert_eq!(rtc.nvram, assembly::NvramSource::Derived);
}

#[test]
fn landmark_stop_address_is_real_mode_offset() {
    let mut assembled = synthetic_stop_machine(&[0x0F, 0x01, 0x0F]);
    let result = assembled.run_session(&run_policy(8));
    assert_prefix_stop(&assembled, result);
    assert_eq!(assembled.machine.cpu().retired(), 1);
}

#[test]
fn landmark_unexpected_stop_reports_variant_location_and_counts() {
    for (program, expected_result, count) in [
        (
            &[0x90, 0x0F, 0x01, 0x0F][..],
            "Err(Machine(Cpu(UnsupportedInstruction { addr: 0xa3cf })))",
            2,
        ),
        (&[0xEB, 0xFE][..], "Ok(RetiredCapReached)", 8),
    ] {
        let mut assembled = synthetic_stop_machine(program);
        let result = assembled.run_session(&run_policy(8));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_prefix_stop(&assembled, result);
        }))
        .expect_err("unexpected stop must fail");
        let message = panic
            .downcast_ref::<String>()
            .expect("formatted stop failure");
        for field in [
            format!("result={expected_result}"),
            "cs=0xf000".into(),
            format!("retired={count}"),
            format!("completed_steps={count}"),
            format!("virtual_time={count}"),
        ] {
            assert!(message.contains(&field), "missing stop field: {field}");
        }
        assert!(message.contains("rip=0x"), "missing RIP field");
    }
}

#[test]
fn landmark_stop_summary_omits_free_form_cpu_payloads() {
    for error in [
        CpuError::Memory("synthetic private payload".into()),
        CpuError::UnexpectedExit {
            what: "synthetic private payload".into(),
        },
    ] {
        let summary = run_result_summary(&Err(RunError::Machine(MachineError::Cpu(error))));
        assert!(!summary.contains("synthetic private payload"));
        assert!(summary.contains("<details omitted>"));
    }
    assert_eq!(
        run_result_summary(&Err(RunError::Machine(MachineError::Cpu(
            CpuError::Unmapped { addr: 0x12345 }
        )))),
        "Err(Machine(Cpu(Unmapped { addr: 0x12345 })))",
    );
}

#[test]
fn landmark_status_classification_uses_only_status_tokens() {
    assert_eq!(verdict(b" PASSED "), Verdict::Passed);
    assert_eq!(verdict(b" FAILED "), Verdict::Failed);
    assert_eq!(verdict(b" NOT PRESENT "), Verdict::NotPresent);
    assert_eq!(verdict(b" \0      "), Verdict::Empty);
    assert_eq!(verdict(b"PASS"), Verdict::Unrecognized);
    assert_eq!(verdict(b"XPASSED "), Verdict::Unrecognized);
}

#[test]
fn landmark_status_cells_ignore_surrounding_text() {
    let mut vram = vec![0; 80 * 25 * 2];
    for cell in vram.as_chunks_mut::<2>().0 {
        cell[0] = b' ';
    }
    for (row, col, token) in [
        (1, 0, &b"|"[..]),
        (1, 2, b"FAILED"),
        (1, 34, b"X"),
        (1, 35, b"PASSED"),
        (1, 47, b"|"),
        (1, 49, b"FAILED"),
        (2, 35, b"FAILED"),
        (2, 47, b"|"),
        (3, 35, b"NOT PRESENT"),
        (3, 47, b"|"),
        (13, 0, b"|"),
        (13, 2, b"PASSED"),
        (13, 47, b"|"),
        (13, 49, b"PASSED"),
    ] {
        for (offset, glyph) in token.iter().enumerate() {
            vram[(row * 80 + col + offset) * 2] = *glyph;
        }
    }
    let grid = video::render_text_mode(&device::TextModeState {
        vram: &vram,
        cols: 80,
        rows: 25,
        start_cell: 0,
        cursor_cell: 0,
        cursor_visible: false,
        attributes: device::TextAttributeSet::Monochrome {
            blink_enabled: true,
        },
    });
    assert_eq!(row_verdict(&grid, 1), Verdict::Passed);
    assert_eq!(row_verdict(&grid, 2), Verdict::Failed);
    assert_eq!(row_verdict(&grid, 3), Verdict::NotPresent);
    assert_eq!(row_verdict(&grid, 13), Verdict::Empty);
}

#[test]
#[ignore = "requires separately supplied proprietary firmware; real-mode prefix only"]
fn user_rom_landmark_at_real_mode_prefix() {
    let catalogue = BoardModel::Ibm5170.firmware_catalogue();
    let found = assembly::require_user_firmware!(ENTRY_ID, catalogue);
    assert!(
        include_str!("../../../Cargo.lock").contains(CPU_REV),
        "CPU pin drifted"
    );
    let temporary = TempDir::new("landmark-machine");
    let file = expect_redacted(
        &found,
        write_machine_file(temporary.path(), &found.source),
        temporary.path(),
        "writing diagnostic machine file",
    );
    let loaded = expect_redacted(
        &found,
        machine_file::load_for_assembly(&file, AssemblyOptions::default()),
        temporary.path(),
        "loading diagnostic machine file",
    );
    assert!(loaded
        .cards
        .iter()
        .any(|(id, device)| id == "mda" && *device == DeviceId::Mda));
    let mut assembled = expect_redacted(
        &found,
        loaded.assemble(&mda_policy()),
        temporary.path(),
        "assembling diagnostic",
    );
    let result = assembled.run_session(&run_policy(STEP_BUDGET));
    assert_prefix_stop(&assembled, result);
    let cpu = assembled.machine.cpu();
    let mut opcode = [0; 3];
    found.expect_redacted(cpu.read_mem(0xFA3CE, &mut opcode), "reading stop opcode");
    // Boolean comparison keeps firmware bytes out of assertion diagnostics.
    assert!(
        opcode == [0x0F, 0x01, 0x0F],
        "expected SIDT opcode prefix 0F 01"
    );
    let retired = cpu.retired();
    let capture = found.expect_redacted(assembled.capture_text(DeviceId::Mda), "capturing MDA");
    let Frame::Text(grid) = capture.frame;
    assert_eq!((grid.cols(), grid.rows()), (80, 25));
    let text_sha256 = assembly::test_support::text_plane_sha256(&video::Frame::Text(grid.clone()));
    eprintln!("{ENTRY_ID}: retired={retired}, text_sha256={text_sha256}");
    for (index, id) in ROW_IDS.iter().enumerate() {
        assert_eq!(
            row_verdict(&grid, index as u16 + 1),
            Verdict::Passed,
            "{id}"
        );
    }
    assert_eq!(row_verdict(&grid, 13), Verdict::Empty, "protected_mode");
    assert_eq!(retired, GOLDEN_RETIRED, "retired count");
    assert_eq!(text_sha256, GOLDEN_TEXT_SHA256, "MDA text plane hash");
}
