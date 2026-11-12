// SPDX-License-Identifier: AGPL-3.0-or-later
//! Volatile BIOS/FreeDOS media leave vendor fixtures and their namespaces intact.
//! NEC µPD765 READ/WRITE DATA and Intel 8237A single-transfer mode are exercised
//! through firmware and DOS; see persistent-media.md §Implementation acceptance tests.
//!
//! Port 61h bits follow the PIT since 2026-11-11; delay loops take real refresh time.
//!

use cpu::Cpu;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use assembly::{
    assemble_with_observation, ConsoleProbe, DeviceId, ImageSource, InjectionSchedule,
    KeyboardStimulus, ObservationPolicy, ObservationRequest, OutputKind, RunPolicy, StopCondition,
    WriteMode,
};

#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/session.rs"]
pub mod session;

const PINNED_SCRIPTS: &[(&str, &str, &str)] = &[
    (
        "kiselev-bios/bios-xi8088.bin",
        include_str!("../../../vendor/kiselev-bios/fetch.sh"),
        "OUTPUT_SHA256=\"",
    ),
    (
        "freedos/freedos.img",
        include_str!("../../../vendor/freedos/fetch.sh"),
        "check_sha256 \"$OUT\" \"",
    ),
    (
        "test386/test386.bin",
        include_str!("../../../vendor/test386/fetch.sh"),
        "check_sha256 \"$OUT\" \"",
    ),
    (
        "hdd/fat16.img",
        include_str!("../../../vendor/hdd/build.sh"),
        "check_sha256 \"$IMAGE\" \"",
    ),
    (
        "xtide-universal-bios/ide_at-r638-8k.bin",
        include_str!("../../../vendor/xtide-universal-bios/fetch.sh"),
        "OUTPUT_SHA256=\"",
    ),
];

fn parse_pin<'a>(script: &'a str, prefix: &str) -> Result<&'a str, &'static str> {
    let mut matches = script
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix));
    let value = matches
        .next()
        .filter(|_| matches.next().is_none())
        .ok_or("script format changed: expected exactly one output SHA-256 pin line")?;
    value
        .strip_suffix('"')
        .filter(|pin| pin.len() == 64 && pin.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("script format changed: expected a quoted 64-digit output SHA-256 pin")
}

fn fixture_pins() -> Vec<(&'static str, &'static str)> {
    PINNED_SCRIPTS
        .iter()
        .map(|&(relative, script, prefix)| {
            let pin = parse_pin(script, prefix).unwrap_or_else(|error| {
                panic!("fixture {relative}: {error}; update the build-script pin parser")
            });
            (relative, pin)
        })
        .collect()
}

#[test]
fn fixture_output_pins_follow_build_script_format() {
    fixture_pins();
}

#[test]
fn fixture_pin_parser_rejects_script_format_drift() {
    let prefix = "OUTPUT_SHA256=\"";
    let hash = "a".repeat(64);
    let valid = format!("{prefix}{hash}\"");
    assert_eq!(parse_pin(&valid, prefix), Ok(hash.as_str()));
    for invalid in [
        String::new(),
        format!("UPSTREAM_SHA256=\"{hash}\""),
        format!("OUTPUT_SHA256={hash}"),
        format!("{prefix}{}\"", "a".repeat(63)),
        format!("{prefix}{}\"", "z".repeat(64)),
        format!("{valid}\n{valid}"),
    ] {
        assert!(parse_pin(&invalid, prefix)
            .unwrap_err()
            .contains("script format changed"));
    }
}

fn entries(root: &Path, listing: &mut BTreeSet<PathBuf>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        listing.insert(entry.path());
        if entry.file_type().unwrap().is_dir() {
            entries(&entry.path(), listing);
        }
    }
}

fn listing(vendor: &Path, checked: &[(&str, &str)]) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    entries(vendor, &mut paths);
    // Worktrees may symlink fixtures. Check their real directories too, without
    // traversing directory symlinks or assuming the checkout owns the targets.
    for &(relative, _) in checked {
        let target = vendor.join(relative).canonicalize().unwrap();
        entries(target.parent().unwrap(), &mut paths);
    }
    paths
}

fn assert_pinned(vendor: &Path, checked: &[(&str, &str)]) {
    for &(relative, expected) in checked {
        let bytes = std::fs::read(vendor.join(relative)).unwrap();
        let hash: String = media::Header::new(&bytes, 0)
            .unwrap()
            .base_digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(hash, expected, "fixture SHA-256: {relative}");
    }
}

#[test]
#[ignore = "slow: BIOS POST, FreeDOS prompt and DOS writes; requires fetched boot fixtures"]
fn volatile_boots_preserve_pinned_fixture_hashes_and_vendor_entries() {
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor");
    // The CI fixture gate fetches the boot/CPU inputs. Auxiliary ATA/option-ROM
    // images remain optional, but a file present initially must survive the run.
    let checked: Vec<_> = fixture_pins()
        .into_iter()
        .filter(|&(relative, _)| {
            !relative.starts_with("hdd/") && !relative.starts_with("xtide-")
                || vendor.join(relative).symlink_metadata().is_ok()
        })
        .collect();
    let mut images = Vec::new();
    for &(relative, _) in &checked {
        let Some(bytes) = fixtures::fixture(relative) else {
            return;
        };
        images.push(bytes);
    }
    assert_pinned(&vendor, &checked);
    let before = listing(&vendor, &checked);
    for boot in ["bios", "freedos", "dos-write"] {
        let mut config = assembly::bios(
            images[0].clone(),
            (boot != "bios").then(|| images[1].clone()),
        );
        for drive in &mut config.drives {
            drive.image = ImageSource::File(vendor.join("freedos/freedos.img"));
            drive.write = WriteMode::Volatile;
        }
        let observation = ObservationPolicy {
            routes: vec![ObservationRequest {
                device: DeviceId::Mda,
                output: OutputKind::TextScanout,
                byte_limit: 0,
            }],
            ..Default::default()
        };
        let mut assembled = assemble_with_observation(&config, &observation).unwrap();
        assert!(assembled.persistence.is_empty());
        let policy = if boot == "bios" {
            RunPolicy::natural(
                Some(cli::BIOS_GOLDEN_MAX_RETIRED),
                Some(cli::BIOS_IDLE_LIMIT),
            )
        } else {
            RunPolicy {
                retired_cap: Some(cli::BIOS_MAX_RETIRED),
                idle_limit: Some(cli::BIOS_IDLE_LIMIT),
                stop: StopCondition::PromptThen {
                    console: ConsoleProbe {
                        device: DeviceId::Mda,
                    },
                    needle: cli::DOS_DATE_PROMPT.into(),
                    poll_step: cli::DOS_INTERACTIVE_POLL_STEP,
                    then_budget: if boot == "dos-write" {
                        cli::DOS_INTERACTIVE_COMMAND_BUDGET
                    } else {
                        0
                    },
                },
                stimuli: if boot == "dos-write" {
                    vec![KeyboardStimulus {
                        keyboard_index: assembled.device_index[&DeviceId::Keyboard],
                        scancodes: cli::dos_write_file_script(),
                        schedule: InjectionSchedule::AfterPrompt,
                    }]
                } else {
                    Vec::new()
                },
            }
        };
        let outcome = assembled.run_session(&policy);
        eprintln!(
            "{boot}: stop={outcome:?}, cs={:04x}, ip={:04x}, retired={}",
            assembled.machine.cpu().code_segment(),
            assembled.machine.cpu().reg(cpu::CpuReg::Rip),
            assembled.machine.cpu().retired()
        );
        let video::Frame::Text(grid) = assembled.capture_text(DeviceId::Mda).unwrap().frame else {
            panic!("expected text frame")
        };
        eprintln!(
            "{boot}: {outcome:?}, virtual={}\n{}",
            assembled.machine.virtual_time(),
            grid.to_dump()
        );
        assert_eq!(outcome.unwrap(), machine::RunOutcome::RetiredCapReached);
        let console = grid.rows_as_strings().join("\n");
        let logical_hash = match boot {
            "bios" => 0xe36ef0102a7cbc14,
            // BIOS century 20h gives FreeDOS Sat 01-01-2000 and stamps new files with year 00.
            "freedos" => 0xc46f0c147c881837,
            "dos-write" => 0x6b0404d06d7532c3,
            _ => unreachable!(),
        };
        assert_eq!(session::logical_hash(&console), logical_hash, "{boot}");

        assert!(
            console.contains(if boot == "bios" {
                "Booting OS"
            } else if boot == "freedos" {
                "Enter new date"
            } else {
                "ZZ"
            }),
            "{boot}: {console}"
        );
        assembled.finish_persistence().unwrap();
        drop(assembled);
        assert_pinned(&vendor, &checked);
        assert_eq!(
            listing(&vendor, &checked),
            before,
            "vendor entries changed during {boot}"
        );
    }
}
