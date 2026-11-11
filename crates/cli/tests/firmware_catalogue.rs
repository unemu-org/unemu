// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware policy at the CLI boundary; see firmware-and-machine-classes.md §6.

use std::{path::PathBuf, process::Command};

struct RomFile(PathBuf);

impl RomFile {
    fn new() -> Self {
        Self::program("halt", &[0xFA, 0xBA, 0xF8, 0x03, 0xB0, b'Z', 0xEE, 0xF4])
    }

    fn program(label: &str, program: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "unemupc98-firmware-{}-{label}.bin",
            std::process::id()
        ));
        let mut rom = vec![0; 32768];
        rom[0x7FF0..0x7FF0 + program.len()].copy_from_slice(program);
        std::fs::write(&path, rom).unwrap();
        Self(path)
    }
}

impl Drop for RomFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn cli_escape_flag_reaches_assembly_and_prints_one_hash_warning() {
    let rom = RomFile::new();
    let run = |allow: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_unemupc98"));
        command.arg("--bios").arg(&rom.0);
        if allow {
            command.arg("--allow-unlisted-firmware");
        }
        command.output().unwrap()
    };
    let rejected = run(false);
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert_eq!(String::from_utf8(rejected.stderr).unwrap().lines().next().unwrap(), "Error: invalid configuration: bios image (sha256 4788bdc606fe61e142d4670b3c5eb1ad2827c84753fec7cdba242319476f4bc0) is not a known image for board \"ibm-5170\"; known: kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh), kiselev-xi8088-1.0.2-atreset (open, vendor/kiselev-bios/fetch.sh), ibm-5170-bios-1984-01-10 (user-supplied: U27 6181028 even, U47 6181029 odd or U17 6181024 even@0, U37 6181026 odd@0, U27 6181025 even@32768, U47 6181027 odd@32768; see vendor/user-roms/README.md), ibm-5170-bios-1985-06-10 (user-supplied: U27 6480090 even, U47 6480091 odd; see vendor/user-roms/README.md), ibm-5170-bios-1985-11-15 (user-supplied: U27 61X9266 even, U47 61X9265 odd; see vendor/user-roms/README.md)");
    let accepted = run(true);
    assert!(accepted.status.success(), "{:?}", accepted);
    assert_eq!(accepted.stdout, b"Z");
    assert_eq!(std::str::from_utf8(&accepted.stderr).unwrap(), "warning: running unlisted firmware (sha256 4788bdc606fe61e142d4670b3c5eb1ad2827c84753fec7cdba242319476f4bc0)\nstopped: halt\n");

    let leading = Command::new(env!("CARGO_BIN_EXE_unemupc98"))
        .arg("--allow-unlisted-firmware")
        .arg("--bios")
        .arg(&rom.0)
        .output()
        .unwrap();
    assert!(leading.status.success());
    assert_eq!(leading.stdout, accepted.stdout);
    assert_eq!(leading.stderr, accepted.stderr);

    std::fs::write(&rom.0, vec![0; 32767]).unwrap();
    let misplaced = run(true);
    assert!(!misplaced.status.success());
    assert_eq!(String::from_utf8(misplaced.stderr).unwrap().lines().next().unwrap(), "Error: invalid configuration: system BIOS of 32767 bytes at 0xf8000 must have a positive size that is a multiple of 8 KiB and end at 0x100000");
}

#[path = "support/fixtures.rs"]
mod fixtures;

#[test]
fn legacy_post_runner_checks_the_catalogue_without_changing_the_bare_rig() {
    let Some(mut rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let real = cli::run_bios_with_cap(&rom, 0);
    assert!(!matches!(real.stop, cli::StopReason::ConfigFailed { .. }));
    rom[0] ^= 1;
    let changed = cli::run_bios_with_cap(&rom, 0);
    match changed.stop {
        cli::StopReason::ConfigFailed { detail } => {
            assert!(detail.contains("is not a known image for board \"ibm-5170\""));
            assert!(detail.contains("kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh)"));
        }
        stop => panic!("changed BIOS reached execution: {stop:?}"),
    }
}

#[test]
fn bios_runtime_errors_print_the_stop_and_exit_nonzero() {
    for (label, program, expected) in [
        (
            "unsupported",
            &[0x0F, 0x05][..],
            "stopped: unsupported instruction: ",
        ),
        (
            "unmapped",
            &[0xEA, 0x10, 0x00, 0xFF, 0xFF][..],
            "stopped: execution error: ",
        ),
    ] {
        let rom = RomFile::program(label, program);
        let output = Command::new(env!("CARGO_BIN_EXE_unemupc98"))
            .arg("--bios")
            .arg(&rom.0)
            .arg("--allow-unlisted-firmware")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        let lines: Vec<_> = stderr.lines().collect();
        assert_eq!(lines.len(), 2, "{stderr}");
        assert!(
            lines[0].starts_with("warning: running unlisted firmware (sha256 "),
            "{stderr}"
        );
        assert!(lines[1].starts_with(expected), "{stderr}");
    }
}

#[test]
fn bios_budget_exhaustion_prints_the_stop_and_exits_zero() {
    let rom = RomFile::program("bounded", &[0xFA, 0xEB, 0xFE]);
    let output = Command::new(env!("CARGO_BIN_EXE_unemupc98"))
        .arg("--bios")
        .arg(&rom.0)
        .arg("--allow-unlisted-firmware")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = stderr.lines().collect();
    assert_eq!(lines.len(), 2, "{stderr}");
    assert!(lines[0].starts_with("warning: running unlisted firmware (sha256 "));
    assert_eq!(lines[1], "stopped: budget exhausted");
}
