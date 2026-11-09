// SPDX-License-Identifier: AGPL-3.0-or-later
//! BIOS boot spike: load the real Kiselev Xi8088 ROM and see how far POST gets
//! (§3.1, §5, §9).
//!
//! Assembles the full PC/AT machine ([`run_bios`]) — cascaded 8259 pair, 8253
//! PIT, 8042, seeded MC146818 CMOS/RTC, VGA text card, port-`0x61` system
//! control, POST port at `0x80` — over a full 1 MiB address space with the 32 KiB
//! BIOS mapped as masked ROM at `0xF8000`, and boots it from the reset vector
//! `F000:FFF0`. This is an exploratory plumbing spike (like the test386 smoke
//! test but with real firmware), **not** CPU validation: a real-mode-only core is
//! expected to stop early, and the value is the honest stop point captured on
//! every exit path.
//!
//! ## Observed result (this CPU revision)
//!
//! The BIOS runs the **entire power-on self-test** and reaches the boot loader,
//! emitting these checkpoints on port `0x80` (Kiselev `errno.inc` names):
//!
//! ```text
//! 0x01 CPU test        0x02 chipset config   0x03 DMAC init
//! 0x04 low-RAM test    0x05 IVT init         0x06 PIT init
//! 0x07 PIC init        0x08 KBC/keyboard     0x09 enable interrupts
//! 0x10 video-BIOS scan 0x12 MDA/CGA fallback
//! 0x20 RTC init        0x21 detect CPU type  0x22 detect FPU
//! 0x24 serial scan     0x25 parallel scan
//! 0x30 RAM test start  0x31 RAM test done
//! 0x40 option-ROM scan 0x43 option-ROM done  0x00 boot loader (INT 19h)
//! ```
//!
//! **POST stream: `[01 02 … 22 24 25 30 31 40 43 00]`.** `0x11` (video-BIOS
//! *init*) is correctly skipped — the option-ROM area is zeroed RAM, no `0x55AA`
//! signature, so the BIOS falls back to `0x12` (MDA/CGA). Seeded CMOS lets RTC
//! init (`0x20`) pass; lifting x87 `FNINIT` let it clear FPU detect (`0x22`).
//!
//! After `e_boot` (`0x00`) the BIOS calls **INT 19h** (`int_19`, "load and execute
//! the boot sector") and enters its floppy-read retry loop. No floppy controller
//! is modelled — the undecoded µPD765 ports read open-bus `0xFF` — so the read
//! never succeeds and the BIOS spins there until the retired-instruction cap
//! (`StopReason::BudgetExhausted`). **Modelling the µPD765 floppy controller (on
//! the existing 8237 DMA) + a boot disk image is the next step**; then INT 19h
//! reads a real boot sector and this becomes a fast, natural-stop boot test.
//!
//! The text screen is blank: the BIOS's MDA fallback targets the `0xB0000` window,
//! which this spike maps as plain RAM rather than routing to the CGA card at
//! `0xB8000` — a caveat to revisit, not a fault.
//!
//! The assertions pin the observed byte stream and the stop *kind*
//! (`BudgetExhausted`), never an address.
//!
//! ## Pinned inputs
//!
//! - BIOS ROM: Sergey Kiselev `8088_bios`, Xi8088 config, built by
//!   `vendor/kiselev-bios/fetch.sh` (32 KiB, gitignored).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.
//!   The stop point is only meaningful against a known CPU revision; re-baseline
//!   the golden if `FNINIT` (or a nearer instruction) gains real-mode support.

use cli::{run_bios, StopReason};
use std::path::PathBuf;

/// Absolute path to the (gitignored) built Xi8088 BIOS image.
fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/kiselev-bios/bios-xi8088.bin")
}

/// The observed POST checkpoint stream. The BIOS now completes the whole
/// power-on self-test — CPU/chipset/DMA/RAM/IVT/PIT/PIC/KBC/interrupts, video
/// scan and MDA/CGA fallback (`0x10`/`0x12`), RTC (`0x20`), CPU-type and FPU
/// detect (`0x21`/`0x22`), serial and parallel scan (`0x24`/`0x25`), the RAM test
/// (`0x30`/`0x31`), the option-ROM scan (`0x40`/`0x43`) — and reaches the boot
/// loader (`e_boot` = `0x00`), where it calls INT 19h. It then spins in the
/// floppy-read retry loop because the µPD765 floppy controller is not modelled
/// yet (undecoded FDC ports read open-bus `0xFF`); adding it is the next step.
/// Golden the exact stream.
const GOLDEN_POST: &[u8] = &[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x12, 0x20, 0x21, 0x22, 0x24, 0x25,
    0x30, 0x31, 0x40, 0x43, 0x00,
];

// Slow: the BIOS runs its full RAM test and then spins in the INT 19h floppy-read
// loop until the retired-instruction cap, which takes minutes. Ignored by default;
// once a floppy controller is modelled INT 19h will read a boot sector and this
// becomes a fast, natural-stop boot test.
#[ignore = "slow: BIOS spins at INT 19h until a floppy controller is modelled"]
#[test]
fn bios_boot_stop_point_matches_golden() {
    let path = rom_path();
    let Ok(rom) = std::fs::read(&path) else {
        // Mirror the fixture-absent skip pattern: the ROM is fetched/built, not
        // committed, so a clean checkout without it simply skips (never fails).
        eprintln!(
            "skipping: BIOS ROM not found at {} — run vendor/kiselev-bios/fetch.sh",
            path.display()
        );
        return;
    };
    assert_eq!(rom.len(), 32768, "Xi8088 BIOS is a 32 KiB image");

    let run = run_bios(&rom);

    // End-to-end proof of the plumbing: reset → far jump → real-mode exec out of
    // masked ROM → POST capture. The stream must reach at least the first POST
    // code (`0x01`, CPU test).
    assert!(
        run.post.contains(&0x01),
        "expected the POST stream to reach at least 0x01, got {:02x?}",
        run.post
    );

    // The observed POST byte stream on this CPU revision. Pinned exactly.
    assert_eq!(
        run.post, GOLDEN_POST,
        "POST checkpoint stream drifted from the recorded golden"
    );

    // The BIOS completes POST and spins in the INT 19h floppy-read retry loop
    // (no floppy controller modelled), so it runs to the retired-instruction cap.
    assert!(
        matches!(run.stop, StopReason::BudgetExhausted),
        "expected the run to hit the retired cap spinning at the boot loader, \
         got {:?}",
        run.stop
    );
}
