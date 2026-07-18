// SPDX-License-Identifier: AGPL-3.0-or-later
//! FreeDOS boot spike: boot a real FreeDOS floppy image on the full machine
//! (§3.1, §5, §9) — the north-star target.
//!
//! Assembles the PC/AT machine ([`run_freedos`]) with the real Kiselev Xi8088
//! BIOS in masked ROM and a real, bootable **FreeDOS** 1.44 MB floppy image (8086
//! kernel + FreeCOM, built by `vendor/freedos/fetch.sh`) in the µPD765 floppy
//! drive, and boots from the reset vector. Exploratory, like the BIOS and floppy
//! spikes before it: the value is the honest boot progress captured on every exit
//! path.
//!
//! ## Observed result (this CPU revision)
//!
//! The BIOS completes its full POST, prints its hardware-detection screen through
//! INT 10h, and boots from the floppy. **The console output lands in the MDA text
//! window at `0xB0000`** (the BIOS's no-video-BIOS path selects MDA), not the CGA
//! page — so [`BiosRun::mda`] carries the screen:
//!
//! ```text
//! Xi 8088 BIOS, Version 1.0.2. Copyright (C) 2010 - 2026 Sergey Kiselev
//! ...
//! Main Processor:             Harris-designed 80C88 @ 4.77 MHz
//! Mathematics Co-processor:   Absent
//! Floppy disk drives:         Drive 0: 1.44 MB, 3.5"; Drive 1: none
//! Total Conventional RAM:     640 KiB
//! Booting OS...
//! ```
//!
//! Every device is detected correctly (80C88, no FPU, the seeded RTC, MDA, the
//! 1.44 MB floppy, 640 KiB RAM), and INT 19h loads and jumps to the FreeDOS boot
//! sector — the whole BIOS + machine stack works under real firmware. The FreeDOS
//! boot sector then runs but does not yet reach a kernel banner or the `A:\>`
//! prompt: execution runs to the retired-instruction cap
//! (`StopReason::BudgetExhausted`), i.e. it spins early in the FreeDOS boot chain
//! rather than hitting an unlifted instruction. Diagnosing that spin is the next
//! step.
//!
//! The golden pins the stable milestone reached: the BIOS renders its full
//! detection screen and hands off to the OS (`Booting OS...` on the MDA console),
//! and the run reaches the cap. Marked `#[ignore]` because it runs the full
//! multi-minute BIOS POST.
//!
//! ## Pinned inputs
//! - BIOS: Sergey Kiselev `8088_bios`, Xi8088 config (`vendor/kiselev-bios`).
//! - FreeDOS: 8086 kernel + FreeCOM, FAT12 1.44 MB image (`vendor/freedos`).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.

use cli::{run_freedos, StopReason};
use std::path::PathBuf;

fn vendor(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
        .join(p)
}

#[ignore = "slow: runs the full multi-minute BIOS POST then boots FreeDOS"]
#[test]
fn freedos_reaches_boot_handoff() {
    let (Ok(rom), Ok(img)) = (
        std::fs::read(vendor("kiselev-bios/bios-xi8088.bin")),
        std::fs::read(vendor("freedos/freedos.img")),
    ) else {
        eprintln!("skipping: BIOS ROM or FreeDOS image not found — run the vendor fetch.sh scripts");
        return;
    };
    assert_eq!(img.len(), 1_474_560, "FreeDOS floppy is a 1.44 MB image");

    let run = run_freedos(&rom, img);

    // The BIOS console (its INT 10h MDA window at 0xB0000) shows the full
    // hardware-detection screen and the hand-off to the OS. Proving this text
    // proves: reset → POST complete → INT 10h video → INT 19h boots the floppy.
    let console = run.mda.rows_as_strings().join("\n");
    assert!(
        console.contains("Xi 8088 BIOS"),
        "expected the BIOS banner on the MDA console, got:\n{console}"
    );
    assert!(
        console.contains("Booting OS"),
        "expected the BIOS to reach the OS hand-off, got:\n{console}"
    );

    // FreeDOS loads but spins early in its boot chain (no unlifted instruction),
    // so the run reaches the retired cap. Assert the kind only.
    assert!(
        matches!(run.stop, StopReason::BudgetExhausted),
        "expected the run to reach the retired cap spinning in early FreeDOS boot, \
         got {:?}",
        run.stop
    );
}
