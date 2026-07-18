// SPDX-License-Identifier: AGPL-3.0-or-later
//! BIOS floppy-boot spike: the real Kiselev Xi8088 ROM boots a disk over the
//! µPD765 + 8237 (§3.1, §5, §7.7).
//!
//! Assembles the full PC/AT machine as `bios_boot_golden` does but with a µPD765
//! floppy controller (ports `0x3F0..=0x3F7`, IRQ6) driven by the 8237 DMA
//! (`0x00..=0x0F` + page latches `0x81..=0x8F`), backed by a minimal bootable
//! 1.44 MB image whose sector 0 writes `'B'` to COM1 and halts. The BIOS runs its
//! whole POST, then INT 19h issues INT 13h floppy reads through the controller.
//!
//! Slow-ish: the full POST + RAM test runs before INT 19h, so this is ignored by
//! default. The fast, deterministic proof of the FDC → 8237 → memory → boot-jump
//! path lives in `floppy_boot_golden` (no BIOS).
//!
//! ## Observed result (this CPU/ROM revision)
//!
//! The BIOS completes POST, calls INT 19h, and its INT 13h floppy driver reads
//! sector 0 of the image into `0x7C00` over the µPD765 + 8237 and jumps to it —
//! the boot sector runs and transmits its marker `'B'` to COM1. This is the
//! milestone: the whole disk path (FDC command sequence, DMA into guest memory,
//! IRQ6, INT 13h, INT 19h, boot jump) works against real firmware.
//!
//! The minimal boot sector emits its marker then `hlt`s without looping, so under
//! the BIOS's live timer it is woken by IRQ0 and runs off into the zeroed sector
//! padding, eventually hitting an instruction the real-mode core does not lift —
//! a `StopReason::Unsupported`, reached in well under a second. A looping boot
//! sector would instead spin to the retired cap (minutes); the run-off is the
//! faster, honest stop for this spike. The value is the COM1 marker, not the
//! stop kind.

use cli::{bootable_floppy_image, run_bios_boot, FLOPPY_BOOT_MARKER};
use std::path::PathBuf;

fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/kiselev-bios/bios-xi8088.bin")
}

#[ignore = "slow-ish: runs the full BIOS POST before INT 19h"]
#[test]
fn bios_int19_reads_boot_sector_over_the_floppy() {
    let path = rom_path();
    let Ok(rom) = std::fs::read(&path) else {
        eprintln!(
            "skipping: BIOS ROM not found at {} — run vendor/kiselev-bios/fetch.sh",
            path.display()
        );
        return;
    };
    assert_eq!(rom.len(), 32768, "Xi8088 BIOS is a 32 KiB image");

    let run = run_bios_boot(&rom, bootable_floppy_image());

    // INT 19h loaded sector 0 at 0x7C00 and jumped to it; the sector transmitted
    // its marker to COM1. That byte appearing at all proves the whole disk path.
    eprintln!("stop: {:?}", run.stop);
    eprintln!("com1: {:02x?}", run.com1);
    assert!(
        run.com1.contains(&FLOPPY_BOOT_MARKER),
        "expected the booted sector's marker on COM1 (INT 19h boot), got {:02x?}",
        run.com1
    );
}
