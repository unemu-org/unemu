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
//! the BIOS's live timer it is woken by IRQ0 and runs off through the sector
//! padding to the end of segment 0. There an instruction crosses offset FFFFh,
//! which raises the general-protection exception (interrupt 13; Intel 80286 and
//! 80287 Programmer's Reference Manual, real-address-mode exceptions). On the PC
//! that vector is shared with IRQ5: the BIOS handler reads the master PIC's
//! in-service register (OCW3 0Bh, then port 20h), finds no IRQ in service and
//! returns, the instruction faults again, and the guest loops in the handler
//! (CS:IP F000:FF2F at the end) until the step cap. Observed at x86jit
//! `54fad1766a458df7cedb6c0b4b5d50742628ef75` with the pinned Kiselev BIOS. The
//! value is the COM1 marker, not the stop kind.

use cli::{bootable_floppy_image, run_bios_boot, FLOPPY_BOOT_MARKER};
#[path = "support/fixtures.rs"]
mod fixtures;

#[ignore = "slow-ish: runs the full BIOS POST before INT 19h"]
#[test]
fn bios_int19_reads_boot_sector_over_the_floppy() {
    let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    assert_eq!(rom.len(), 32768, "Xi8088 BIOS is a 32 KiB image");

    let run = run_bios_boot(&rom, bootable_floppy_image());
    // Monochrome CRTC aliases update the card's cursor even though the colour
    // page is blank. A host snapshot must preserve that register state.
    assert_eq!(
        run.screen.cursor(),
        video::Cursor {
            col: 0,
            row: 17,
            visible: true
        }
    );
    let stamp = run
        .screen_stamp
        .expect("assembled display has a capture boundary");
    assert!(stamp.retired > 0);
    assert!(stamp.virtual_time >= stamp.retired);

    // INT 19h loaded sector 0 at 0x7C00 and jumped to it; the sector transmitted
    // its marker to COM1. After the halt, the run-off ends in the interrupt-13
    // loop described above and writes nothing more to COM1 before the step cap;
    // the scheduler A/B harness compares both engines over the first 10 million
    // steps of this run.
    eprintln!("stop: {:?}", run.stop);
    eprintln!("com1: {:02x?}", run.com1);
    assert_eq!(
        run.com1,
        [FLOPPY_BOOT_MARKER],
        "expected only the booted sector's marker on COM1 (INT 19h boot)"
    );
}
