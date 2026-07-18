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
//! The BIOS runs a long way through POST, emitting these checkpoints on port
//! `0x80` (mapped to the Kiselev `errno.inc` names):
//!
//! ```text
//! 0x01 CPU test        0x02 chipset config   0x03 DMAC init
//! 0x04 low-RAM test    0x05 IVT init         0x06 PIT init
//! 0x07 PIC init        0x08 KBC/keyboard     0x09 enable interrupts
//! 0x10 video-BIOS scan 0x12 no video BIOS → MDA/CGA fallback
//! 0x20 RTC init        0x21 detect CPU type  0x22 detect FPU  <-- stops here
//! ```
//!
//! **POST stream: `[01 02 03 04 05 06 07 08 09 10 12 20 21 22]`.** Note `0x11`
//! (video-BIOS *init*) is correctly skipped — the option-ROM scan area is mapped
//! as zeroed RAM, so no `0x55AA` signature is found and the BIOS falls back to
//! `0x12` (MDA/CGA). The seeded CMOS lets RTC init (`0x20`) pass.
//!
//! It stops in **`detect_fpu`** (`cpu.inc`), on the coprocessor-init instruction:
//!
//! ```text
//! F000:A85B  DB E3   fninit    <- stops here: x87 FNINIT is not lifted
//! F000:A85D  B9 03 00  mov cx, 3
//! F000:A860  E2 FE   loop $     ; coprocessor settle delay
//! ```
//!
//! i.e. **`FNINIT` (opcode `DB E3`) at linear `0xFA85B`** surfaces as
//! `Exit::UnknownInstruction`. So the x87 FPU instruction set is the next
//! real-mode gap a seam would need to close to push this BIOS past FPU detection
//! into the RAM test and video output.
//!
//! The text screen is blank at the stop: the BIOS reached FPU detection before
//! painting the CGA text page (and its "no video BIOS → MDA/CGA" fallback may
//! target the MDA window at `0xB0000`, which this spike maps as plain RAM rather
//! than routing to the CGA card at `0xB8000`).
//!
//! The assertions pin the observed byte stream and the stop *kind*
//! (`Unsupported`), never an address — the faulting address lives only in the
//! diagnostic `detail` string and this comment.
//!
//! ## Pinned inputs
//!
//! - BIOS ROM: Sergey Kiselev `8088_bios`, Xi8088 config, built by
//!   `vendor/kiselev-bios/fetch.sh` (32 KiB, gitignored).
//! - CPU: `x86jit-core` (branch `pc-emulator`), the rev pinned in `Cargo.lock`.
//!   The stop point is only meaningful against a known CPU revision; re-baseline
//!   the golden if `FNINIT` (or a nearer instruction) gains real-mode support.

use cli::{run_bios, StopReason};
use std::path::PathBuf;

/// Absolute path to the (gitignored) built Xi8088 BIOS image.
fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/kiselev-bios/bios-xi8088.bin")
}

/// The observed POST checkpoint stream, before execution stops on the unlifted
/// `FNINIT` in FPU detection. Golden the exact stream.
const GOLDEN_POST: &[u8] = &[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x12, 0x20, 0x21, 0x22,
];

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

    // Assert the stop KIND only — the address is CPU-revision-dependent and lives
    // in the detail string for the report, never in an assertion.
    assert!(
        matches!(run.stop, StopReason::Unsupported { .. }),
        "expected an unsupported-instruction stop (FNINIT in FPU detection), \
         got {:?}",
        run.stop
    );
}
