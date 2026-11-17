// SPDX-License-Identifier: AGPL-3.0-or-later
//! Floppy-boot plumbing golden: read a boot sector over the µPD765 + 8237 DMA and
//! jump to it (§7.7).
//!
//! Exercises the whole floppy path end to end without a BIOS: a hand-encoded
//! real-mode loader ([`pc_harness::FLOPPY_LOADER`]) programs 8237 channel 2 for a
//! device→memory transfer to `0x7C00`, enables the µPD765, and issues READ DATA
//! for cylinder 0 / head 0 / sector 1. As the READ command completes, the machine
//! pulls the sector bytes the controller read from its backing image and streams
//! them through the 8237 into guest RAM (the SMC-safe `note_write` path), then the
//! loader far-jumps to `0x7C00`. The minimal boot sector there transmits `'B'` to
//! COM1 and halts, so a working path yields exactly that byte and a natural halt.
//!
//! This checks without BIOS POST: the FDC
//! command state machine, the FDC → 8237 → guest-memory transfer, and the boot
//! jump. The real BIOS INT 19h path over the same devices lives in
//! `bios_floppy_boot_golden` (slow, ignored).

use pc_harness::{run_floppy_boot, FLOPPY_BOOT_MARKER};

#[test]
fn floppy_boot_reads_sector_and_runs_it() {
    let out = run_floppy_boot().expect("floppy-boot machine runs to the boot sector's hlt");
    assert_eq!(
        out,
        vec![FLOPPY_BOOT_MARKER],
        "the boot sector loaded at 0x7C00 must run and transmit its marker to COM1"
    );
}
