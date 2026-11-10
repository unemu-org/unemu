// SPDX-License-Identifier: AGPL-3.0-or-later
//! FreeDOS boot golden: boot a real FreeDOS floppy image on the full machine
//! (§3.1, §5, §9).
//!
//! Assembles the PC/AT machine ([`run_freedos`]) with the real Kiselev Xi8088
//! BIOS in masked ROM and a real, bootable **FreeDOS** 1.44 MB floppy image (8086
//! kernel + FreeCOM, built by `vendor/freedos/fetch.sh`) in the µPD765 floppy
//! drive, and boots from the reset vector. The golden captures the final shell
//! console and the complete POST checkpoint stream.
//!
//! ## Observed result (this CPU revision)
//!
//! The complete media-attached POST stream is:
//! `[01 02 03 04 05 06 07 08 09 10 12 20 21 22 24 25 30 31 40 43 00]`.
//! The BIOS's `src/errno.inc` names these checkpoints:
//!
//! ```text
//! 01 e_cpu_test       02 e_init_cfg        03 e_init_dmac
//! 04 e_low_ram_test   05 e_int_table       06 e_pit_init
//! 07 e_pic_init       08 e_kbd_init        09 e_int_ena
//! 10 e_vid_bios_scan  12 e_vid_no_bios     20 e_rtc_init
//! 21 e_cpu_detect    22 e_fpu_detect      24 e_serial_scan
//! 25 e_parallel_scan 30 e_ram_start       31 e_ram_complete
//! 40 e_ext_start     43 e_ext_complete    00 e_boot
//! ```
//!
//! The BIOS completes its full POST, prints its hardware-detection screen through
//! INT 10h, and boots from the floppy. **The console output lands in the MDA text
//! window at `0xB0000`** (the BIOS's no-video-BIOS path selects MDA), not the CGA
//! page — so [`BiosRun::mda`] carries the screen. From reset it runs all the way
//! into the FreeDOS shell:
//!
//! ```text
//! Xi 8088 BIOS, Version 1.0.2. Copyright (C) 2010 - 2026 Sergey Kiselev
//! ...
//! Floppy disk drives:         Drive 0: 1.44 MB, 3.5"; Drive 1: none
//! Total Conventional RAM:     640 KiB
//! Booting OS...
//! ................................................................................
//! ............123
//! FreeDOS kernel 2044 (build 2044 OEM:0xfd) [compiled Jan 25 2026]
//! Kernel compatibility 6.22 - WATCOMC - 808x compatible
//! (C) Copyright 1995-2023 Pasquale J. Villani and The FreeDOS Project.
//! ...
//! FreeCom version 0.86 - WATCOMC - XMS_Swap [Dec 30 2024 22:10:51]
//! Current date is Tue 01-01-1980
//! Enter new date (mm-dd-[cc]yy):
//! ```
//!
//! (This is the cumulative boot narrative. The captured [`BiosRun::mda`] snapshot
//! holds only the final 25-row window — by the time FreeCOM prompts, the boot has
//! scrolled the early BIOS banner and detection lines off the top of the buffer,
//! so the golden asserts on the still-visible tail: the kernel banner and shell.)
//!
//! Every device is detected correctly, INT 19h loads the FreeDOS boot sector, the
//! boot sector loads `KERNEL.SYS` through the BIOS INT 13h path (the dots), the
//! kernel initialises and prints its banner, and it hands off to `COMMAND.COM`
//! (FreeCOM), which starts and prompts for the date. With no `AUTOEXEC.BAT`,
//! FreeCOM asks for the date/time before giving the `A:\>` prompt; the run stops
//! there **waiting for keyboard input that never arrives in a headless run**.
//! `run_freedos` polls the console and stops as soon as that date prompt appears
//! within the configured instruction-equivalent safety cap. Host duration depends
//! on the build and host. The stop is `StopReason::BudgetExhausted` (a poll-chunk cap
//! reached at a terminal wait-for-input state, not a spin or an unlifted op).
//!
//! The monochrome (MDA) input status register at port `0x3BA` supplies the retrace
//! bits polled by the BIOS's INT 10h read/write-character routines. Their waits
//! must complete for the kernel's INT 10h AH=08h call to return.
//!
//! The golden pins the stable milestone reached: the FreeDOS kernel banner and
//! the FreeCOM shell prompt on the MDA console, with the run reaching the cap at
//! the keyboard wait. Marked `#[ignore]` because it runs BIOS POST and the full
//! FreeDOS boot chain.
//!
//! The BIOS never programs the colour card, so its cursor stays at `(0, 0)`
//! and visible. MDA card capture exposes the BIOS cursor at `(31, 24)`, visible;
//! the earlier RAM probe hid it. These are the intended independent-card states.
//!
//! ## Pinned inputs
//! - BIOS: Sergey Kiselev `8088_bios`, Xi8088 config (`vendor/kiselev-bios`).
//! - FreeDOS: 8086 kernel + FreeCOM, FAT12 1.44 MB image (`vendor/freedos`).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.

use cli::{run_freedos, StopReason};
#[path = "support/fixtures.rs"]
mod fixtures;

const GOLDEN_POST: &[u8] = &[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x12, 0x20, 0x21, 0x22, 0x24, 0x25,
    0x30, 0x31, 0x40, 0x43, 0x00,
];

#[ignore = "slow: runs BIOS POST and the full FreeDOS boot chain"]
#[test]
fn freedos_reaches_shell_prompt() {
    let (Some(rom), Some(img)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
    ) else {
        return;
    };
    assert_eq!(img.len(), 1_474_560, "FreeDOS floppy is a 1.44 MB image");

    let run = run_freedos(&rom, img);
    assert_eq!(
        run.post, GOLDEN_POST,
        "media-attached POST checkpoint stream"
    );
    // The BIOS never programs the colour card, so it keeps its power-on cursor.
    assert_eq!(
        run.screen.cursor(),
        video::Cursor {
            col: 0,
            row: 0,
            visible: true
        }
    );
    assert_eq!(
        run.mda.cursor(),
        video::Cursor {
            col: 31,
            row: 24,
            visible: true
        }
    );
    let stamp = run
        .screen_stamp
        .expect("assembled display has a capture boundary");
    assert!(stamp.retired > 0);
    assert!(stamp.virtual_time >= stamp.retired);

    // The MDA console at 0xB0000 holds only the final 25-row window: by the time
    // FreeCOM prompts, the whole boot has scrolled the buffer (the BIOS scrolls by
    // copying lines up in video RAM, and capture starts at the CRTC start address), so the
    // early "Xi 8088 BIOS" banner and the detection lines have scrolled off the
    // top. Assert on the tail that is still on screen — the kernel banner and the
    // shell prompt — which proves the whole chain ran.
    let console = run.mda.rows_as_strings().join("\n");

    // The FreeDOS boot sector loads KERNEL.SYS through the BIOS INT 13h path and
    // the kernel prints its banner: real firmware + FDC + INT 10h all cooperate.
    assert!(
        console.contains("FreeDOS kernel 2044"),
        "expected the FreeDOS kernel banner, got:\n{console}"
    );
    // The kernel hands off to COMMAND.COM (FreeCOM), which starts and prompts for
    // the date — the interactive shell is running. This is the furthest milestone
    // and depends on the MDA status-register (0x3BA) retrace model.
    assert!(
        console.contains("FreeCom version 0.86"),
        "expected the FreeCOM shell to start, got:\n{console}"
    );
    assert!(
        console.contains("Enter new date"),
        "expected the FreeCOM date prompt, got:\n{console}"
    );

    // FreeCOM then idles in the DOS INT 16h keyboard wait for input that never
    // arrives in a headless run; the timer IRQ0 keeps ticking, so the run ends at
    // the retired cap rather than idling out. Assert the kind only.
    assert!(
        matches!(run.stop, StopReason::BudgetExhausted),
        "expected the run to reach the retired cap waiting for keyboard input at \
         the shell prompt, got {:?}",
        run.stop
    );
}
