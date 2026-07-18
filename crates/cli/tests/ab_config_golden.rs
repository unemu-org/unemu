// SPDX-License-Identifier: AGPL-3.0-or-later
//! A/B result-equality tests for the config migration (§3.1).
//!
//! Every PC-family runner now builds a machine from a declarative
//! `assembly::MachineConfig` preset instead of hand-assembling it. These tests are
//! the proof that the migration is behaviour-preserving: each runs the config path
//! and its `*_legacy` hand-assembled twin on the same inputs and asserts their full
//! results are identical — not just the golden files, which do not cover every
//! observable (a `BiosRun` carries POST + colour screen + MDA console + COM1 +
//! stop; a golden may pin only some of them).
//!
//! The self-contained runners (floppy-boot and the four real-mode demos) run in the
//! default suite. The fixture-backed ones (test386, BIOS, FreeDOS) need a vendored
//! image and run the multi-minute boot, so they are `#[ignore]`d and skip cleanly
//! when the image is absent — run them with `--release -- --ignored`.
//!
//! These tests are removed together with the `*_legacy` paths when the legacy
//! chipset layer is retired.

use cli::testing::{assert_bios_run_eq, assert_test386_run_eq};
use cli::{
    bootable_floppy_image, run_bios, run_bios_boot, run_bios_boot_legacy, run_bios_legacy,
    run_boot, run_boot_legacy, run_floppy_boot, run_floppy_boot_legacy, run_freedos,
    run_freedos_legacy, run_keyboard, run_keyboard_legacy, run_rtc, run_rtc_legacy, run_test386,
    run_test386_legacy, run_vga_text, run_vga_text_legacy, BOOT_PROGRAM, KBD_PROGRAM, KBD_SCANCODE,
    RTC_PROGRAM, VGA_TEXT_PROGRAM,
};
use std::path::PathBuf;

fn vendor(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
        .join(p)
}

// --- Self-contained runners (no vendored fixture): run in the default suite. ---

#[test]
fn floppy_boot_config_matches_legacy() {
    let legacy = run_floppy_boot_legacy().expect("legacy floppy-boot runs");
    let got = run_floppy_boot().expect("config floppy-boot runs");
    assert_eq!(legacy, got, "floppy-boot COM1 stream diverged");
}

#[test]
fn boot_config_matches_legacy() {
    let legacy = run_boot_legacy(BOOT_PROGRAM).expect("legacy boot runs");
    let got = run_boot(BOOT_PROGRAM).expect("config boot runs");
    assert_eq!(legacy, got, "boot demo COM1 stream diverged");
}

#[test]
fn keyboard_config_matches_legacy() {
    let legacy = run_keyboard_legacy(KBD_PROGRAM, &[KBD_SCANCODE]).expect("legacy keyboard runs");
    let got = run_keyboard(KBD_PROGRAM, &[KBD_SCANCODE]).expect("config keyboard runs");
    assert_eq!(legacy, got, "keyboard demo COM1 stream diverged");
}

#[test]
fn rtc_config_matches_legacy() {
    let legacy = run_rtc_legacy(RTC_PROGRAM).expect("legacy rtc runs");
    let got = run_rtc(RTC_PROGRAM).expect("config rtc runs");
    assert_eq!(legacy, got, "rtc demo COM1 stream diverged");
}

#[test]
fn vga_text_config_matches_legacy() {
    let legacy = run_vga_text_legacy(VGA_TEXT_PROGRAM).expect("legacy vga-text runs");
    let got = run_vga_text(VGA_TEXT_PROGRAM).expect("config vga-text runs");
    assert_eq!(legacy, got, "vga-text grid diverged");
}

// --- Fixture-backed runners: ignored by default, skip when the image is absent. ---

#[ignore = "needs the vendored test386 image; run with --ignored"]
#[test]
fn test386_config_matches_legacy() {
    let Ok(blob) = std::fs::read(vendor("test386/test386.bin")) else {
        eprintln!("skipping: test386 image not found — run vendor/test386/fetch.sh");
        return;
    };
    let legacy = run_test386_legacy(&blob);
    let got = run_test386(&blob);
    assert_test386_run_eq(&legacy, &got);
}

#[ignore = "slow: runs the full BIOS POST twice; needs the vendored BIOS ROM"]
#[test]
fn bios_config_matches_legacy() {
    let Ok(rom) = std::fs::read(vendor("kiselev-bios/bios-xi8088.bin")) else {
        eprintln!("skipping: BIOS ROM not found — run vendor/kiselev-bios/fetch.sh");
        return;
    };
    let legacy = run_bios_legacy(&rom);
    let got = run_bios(&rom);
    assert_bios_run_eq(&legacy, &got);
}

#[ignore = "slow: runs the full BIOS POST twice; needs the vendored BIOS ROM"]
#[test]
fn bios_boot_config_matches_legacy() {
    let Ok(rom) = std::fs::read(vendor("kiselev-bios/bios-xi8088.bin")) else {
        eprintln!("skipping: BIOS ROM not found — run vendor/kiselev-bios/fetch.sh");
        return;
    };
    let legacy = run_bios_boot_legacy(&rom, bootable_floppy_image());
    let got = run_bios_boot(&rom, bootable_floppy_image());
    assert_bios_run_eq(&legacy, &got);
}

#[ignore = "slow: runs the full FreeDOS boot chain twice; needs the vendored fixtures"]
#[test]
fn freedos_config_matches_legacy() {
    let (Ok(rom), Ok(img)) = (
        std::fs::read(vendor("kiselev-bios/bios-xi8088.bin")),
        std::fs::read(vendor("freedos/freedos.img")),
    ) else {
        eprintln!(
            "skipping: BIOS ROM or FreeDOS image not found — run the vendor fetch.sh scripts"
        );
        return;
    };
    let legacy = run_freedos_legacy(&rom, img.clone());
    let got = run_freedos(&rom, img);
    assert_bios_run_eq(&legacy, &got);
}
