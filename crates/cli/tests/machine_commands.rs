// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine-file command contracts without guest windows or lock-holding siblings.
//! See machine-files § Frontends and persistent-media § Session lifecycle.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

// A privilege-changing return keeps the failure path independent of task switching.
const REFUSED_PRIVILEGE_RETURN: &[u8] = &[
    0xFA, 0x31, 0xC0, 0x8E, 0xD0, 0x6A, 0x0B, 0x6A, 0, 0xB0, 1, 0x0F, 1, 0xF0, 0xCB,
];

struct Files(PathBuf);
impl Files {
    fn new(program: &[u8]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "machine-commands-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let files = Self(root);
        let mut rom = vec![0; 32768];
        rom[0x7FF0..0x7FF0 + program.len()].copy_from_slice(program);
        files.write("bios.bin", rom);
        files.write("machine.toml", "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[card]]\nkind='uart-16550'\nid='console'\nirq='none'\n[[card]]\nkind='mda'\nid='screen'\n");
        files
    }
    fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
    fn machine(&self) -> PathBuf {
        self.0.join("machine.toml")
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn command(args: &[&str], path: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unemupc98"))
        .args(args)
        .arg(path)
        .args(extra)
        .output()
        .unwrap()
}
fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).unwrap()
}

#[test]
fn machine_check_has_valid_invalid_and_usage_exit_codes() {
    // Executing this ROM would fail; check must assemble without executing it.
    let files = Files::new(REFUSED_PRIVILEGE_RETURN);
    let accepted = command(
        &["machine", "check"],
        &files.machine(),
        &["--allow-unlisted-firmware"],
    );
    assert_eq!(accepted.status.code(), Some(0), "{accepted:?}");
    assert!(accepted.stdout.is_empty());
    let invalid = files.write("invalid.toml", "schema=1\n[board]\nmodel='unknown'\n");
    let rejected = command(&["machine", "check"], &invalid, &[]);
    assert_eq!(rejected.status.code(), Some(1), "{rejected:?}");
    assert!(stderr(&rejected).contains(&format!("{}:", invalid.display())));
    assert!(stderr(&rejected).contains("line 3"), "{rejected:?}");
    for args in [
        vec!["machine"],
        vec!["machine", "check"],
        vec!["machine", "check", "a.toml", "extra"],
        vec!["run", "--machine"],
        vec!["run", "--machine", "a.toml", "--max-steps", "-1"],
        vec![
            "run",
            "--machine",
            "a.toml",
            "--idle-limit",
            "18446744073709551616",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_unemupc98"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
    }
}

#[test]
fn inherited_unlisted_firmware_is_positioned_and_escape_reaches_both_commands() {
    let files = Files::new(&[0xFA, 0xBA, 0xF8, 0x03, 0xB0, b'Z', 0xEE, 0xF4]);
    let child = files.write("child.toml", "schema=1\nextends='machine.toml'\n");
    for args in [&["machine", "check"][..], &["run", "--machine"][..]] {
        let rejected = command(args, &child, &[]);
        assert_eq!(rejected.status.code(), Some(1), "{rejected:?}");
        assert!(
            stderr(&rejected).contains(&format!("{}:5:", files.machine().display())),
            "{rejected:?}"
        );
        assert!(stderr(&rejected).contains("firmware.bios.image: bios image (sha256"));
        let accepted = command(args, &child, &["--allow-unlisted-firmware"]);
        assert_eq!(accepted.status.code(), Some(0), "{accepted:?}");
        assert_eq!(
            stderr(&accepted)
                .matches("warning: running unlisted firmware")
                .count(),
            1
        );
    }
    let run = command(
        &["run", "--machine"],
        &child,
        &["--allow-unlisted-firmware", "--print", "console"],
    );
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    assert_eq!(run.stdout, b"Z");
    assert!(stderr(&run).ends_with("stopped: halt\n"));
    let bad_card = command(
        &["run", "--machine"],
        &child,
        &["--allow-unlisted-firmware", "--print", "absent"],
    );
    assert_eq!(bad_card.status.code(), Some(1));
    assert!(stderr(&bad_card).contains("output card \"absent\" is not configured"));
}

#[test]
fn run_prints_text_and_reports_budget_and_execution_failure() {
    let files = Files::new(&[
        0xFA, 0xB8, 0x00, 0xB0, 0x8E, 0xC0, 0x26, 0xC7, 0x06, 0, 0, b'H', 7, 0xF4,
    ]);
    let run = command(
        &["run", "--machine"],
        &files.machine(),
        &[
            "--allow-unlisted-firmware",
            "--max-steps",
            "100",
            "--idle-limit",
            "10",
            "--print",
            "screen",
        ],
    );
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    assert!(run.stdout.starts_with(b"H\n"), "{run:?}");
    assert!(stderr(&run).ends_with("stopped: halt\n"));
    let capped = command(
        &["run", "--machine"],
        &files.machine(),
        &["--allow-unlisted-firmware", "--max-steps", "0"],
    );
    assert_eq!(capped.status.code(), Some(0), "{capped:?}");
    assert!(stderr(&capped).ends_with("stopped: budget exhausted\n"));
    let mut rom = vec![0; 32768];
    rom[0x7FF0..0x7FF0 + REFUSED_PRIVILEGE_RETURN.len()].copy_from_slice(REFUSED_PRIVILEGE_RETURN);
    files.write("bios.bin", rom);
    let failed = command(
        &["run", "--machine"],
        &files.machine(),
        &["--allow-unlisted-firmware"],
    );
    assert_eq!(failed.status.code(), Some(1), "{failed:?}");
    assert!(stderr(&failed).contains(
        "stopped: unsupported instruction: unsupported cpu system operation LegacyPrivilegeTransition"
    ));
}

#[path = "support/fixtures.rs"]
mod fixtures;

#[test]
fn machine_check_accepts_the_shipped_machine() {
    let (Some(_rom), Some(_)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
    ) else {
        return;
    };
    let machine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml");
    let valid = command(&["machine", "check"], &machine, &[]);
    assert_eq!(valid.status.code(), Some(0), "{valid:?}");
}

#[test]
fn run_finishes_journal_writes_before_offline_verification() {
    journal_run(false);
}

#[test]
fn failed_run_leaves_journal_commands_unsealed() {
    journal_run(true);
}

fn journal_run(fail: bool) {
    let files = Files::new(&[0xF4]);
    let mut program = vec![0xFA, 0xFC, 0x31, 0xC0, 0x8E, 0xD8, 0x8E, 0xC0];
    let out = |program: &mut Vec<u8>, port: u16, value: u8| {
        program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
    };
    out(&mut program, 0x3F2, 0x1C);
    out(&mut program, 0x3F7, 2);
    program.extend([0xBF, 0, 6, 0xB9, 0, 2, 0xB0, 0xC7, 0xF3, 0xAA]);
    for (port, value) in [
        (0x0A, 6),
        (0x0C, 0),
        (0x0B, 0x4A),
        (4, 0),
        (4, 6),
        (0x81, 0),
        (5, 0xFF),
        (5, 1),
        (0x0A, 2),
    ] {
        out(&mut program, port, value);
    }
    for value in [5, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
        out(&mut program, 0x3F5, value);
    }
    if fail {
        program.extend_from_slice(REFUSED_PRIVILEGE_RETURN);
    } else {
        program.push(0xF4);
    }
    let mut rom = vec![0; 32768];
    rom[..program.len()].copy_from_slice(&program);
    rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
    files.write("bios.bin", rom);
    let base = files.write("base.img", vec![0; 368_640]);
    let journal = files.0.join("disk.upfj");
    files.write("machine.toml", "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='base.img'\nwrite='journal'\njournal='disk.upfj'\n");
    let run = command(
        &["run", "--machine"],
        &files.machine(),
        &[
            "--allow-unlisted-firmware",
            "--max-steps",
            "1000",
            "--idle-limit",
            "10",
        ],
    );
    assert_eq!(run.status.code(), Some(i32::from(fail)), "{run:?}");
    if fail {
        assert!(stderr(&run).contains(
            "stopped: unsupported instruction: unsupported cpu system operation LegacyPrivilegeTransition"
        ));
    }
    let verify = Command::new(env!("CARGO_BIN_EXE_unemupc98"))
        .args(["media", "verify", "--base"])
        .arg(&base)
        .arg("--journal")
        .arg(&journal)
        .output()
        .unwrap();
    assert_eq!(
        verify.status.code(),
        Some(if fail { 3 } else { 0 }),
        "{verify:?}"
    );
    if fail {
        assert!(std::str::from_utf8(&verify.stdout)
            .unwrap()
            .contains("unsealed generation 1: commands 1, sectors 1"));
        assert_eq!(std::fs::read(base).unwrap(), vec![0; 368_640]);
        return;
    }
    assert!(std::str::from_utf8(&verify.stdout)
        .unwrap()
        .contains("session end"));
    let exported = files.0.join("export.img");
    let export = Command::new(env!("CARGO_BIN_EXE_unemupc98"))
        .args(["media", "commit", "--base"])
        .arg(&base)
        .arg("--journal")
        .arg(&journal)
        .args(["--to", "1", "--output"])
        .arg(&exported)
        .output()
        .unwrap();
    assert_eq!(export.status.code(), Some(0), "{export:?}");
    assert_eq!(&std::fs::read(exported).unwrap()[..512], &[0xC7; 512]);
    assert_eq!(std::fs::read(base).unwrap(), vec![0; 368_640]);
}

#[test]
fn default_output_prefers_display_then_falls_back_to_uart() {
    let files = Files::new(&[0xFA, 0xBA, 0xF8, 0x03, 0xB0, b'Z', 0xEE, 0xF4]);
    let invoke = || {
        command(
            &["run", "--machine"],
            &files.machine(),
            &["--allow-unlisted-firmware"],
        )
    };
    let both = invoke();
    assert_eq!(both.status.code(), Some(0), "{both:?}");
    assert_eq!(both.stdout, vec![b'\n'; 25]);
    let source = std::fs::read_to_string(files.machine()).unwrap();
    files.write(
        "machine.toml",
        source.split("[[card]]\nkind='mda'").next().unwrap(),
    );
    let serial = invoke();
    assert_eq!(serial.status.code(), Some(0), "{serial:?}");
    assert_eq!(serial.stdout, b"Z");
}

#[test]
fn idle_limit_bounds_interrupt_enabled_halt() {
    let files = Files::new(&[0xFB, 0xF4]);
    for (limit, stop) in [("0", "halt"), ("10", "halt"), ("10000", "budget exhausted")] {
        let run = command(
            &["run", "--machine"],
            &files.machine(),
            &[
                "--allow-unlisted-firmware",
                "--idle-limit",
                limit,
                "--max-steps",
                "100",
            ],
        );
        assert_eq!(run.status.code(), Some(0), "{run:?}");
        assert!(
            stderr(&run).ends_with(&format!("stopped: {stop}\n")),
            "{run:?}"
        );
    }
}

#[test]
fn print_rejects_a_configured_card_without_output_as_a_usage_error() {
    let files = Files::new(&[0xFA, 0xF4]);
    let source = std::fs::read_to_string(files.machine()).unwrap();
    files.write(
        "machine.toml",
        format!("{source}\n[[card]]\nkind='fdc-765'\n"),
    );
    let run = command(
        &["run", "--machine"],
        &files.machine(),
        &["--allow-unlisted-firmware", "--print", "fdc-765"],
    );
    assert_eq!(run.status.code(), Some(2), "{run:?}");
    assert!(
        stderr(&run).contains("card \"fdc-765\" has no printable output"),
        "{run:?}"
    );
}

#[test]
fn run_machine_reboots_at_reset_vector_with_ram_retained() {
    let files = Files::new(&[0xEA, 0, 0, 0, 0xF8]);
    let mut rom = std::fs::read(files.0.join("bios.bin")).unwrap();
    let program = [
        0x31, 0xC0, 0x8E, 0xD8, 0x80, 0x3E, 0, 5, 1, 0x74, 10, 0xC6, 0x06, 0, 5, 1, 0xB0, 0xFE,
        0xE6, 0x64, 0xF4, 0xBA, 0xF8, 3, 0xB0, b'Z', 0xEE, 0xFA, 0xF4,
    ];
    rom[..program.len()].copy_from_slice(&program);
    files.write("bios.bin", rom);
    let output = command(
        &["run", "--machine"],
        &files.machine(),
        &[
            "--allow-unlisted-firmware",
            "--max-steps",
            "100",
            "--print",
            "console",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"Z");
    assert!(stderr(&output).ends_with("stopped: halt\n"));
}
