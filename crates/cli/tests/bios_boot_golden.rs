// SPDX-License-Identifier: AGPL-3.0-or-later
//! BIOS POST golden: boot the Kiselev Xi8088 ROM without decoded floppy ports
//! (§3.1, §5, §9).
//!
//! The bare rig's 8042 selects the 80286 model. Its single-step trap after POP DS
//! (Intel 80286 Programmer's Reference Manual, §9.7.1) changes the open BIOS's
//! CPU identification and adds a warning row; other screen content is unchanged.
//!
//! Port 61h bits follow the PIT since 2026-11-11; delay loops take real refresh time.
//!
//! Keyboard replies now take the host and reply transfer time and the keyboard's
//! basic assurance test delay (IBM PC AT Technical Reference, 101/102-Key
//! Keyboard, "Basic Assurance Test"), so the BIOS keyboard reset finishes later
//! in virtual time; screen content is unchanged apart from the clock rows.
//! Normalized logical hashes and cursors are unchanged.
//!
//! Assembles the full PC/AT machine ([`run_bios_with_cap`]) — cascaded 8259 pair, 8253
//! PIT, 8042, seeded MC146818 CMOS/RTC, VGA text card, port-`0x61` system
//! control, POST port at `0x80` — over a full 1 MiB address space with the 32 KiB
//! BIOS mapped as masked ROM at `0xF8000`, and boots it from the reset vector
//! `F000:FFF0`. The golden captures complete POST and bounds the subsequent
//! no-controller boot retry loop.
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
//! init (`0x20`) pass; FPU detect (`0x22`) executes x87 `FNINIT`.
//!
//! After `e_boot` (`0x00`) the BIOS calls **INT 19h** (`int_19`, "load and execute
//! the boot sector") and enters its floppy-read retry loop. This configuration
//! attaches no floppy, so the read never succeeds and the BIOS spins there until
//! the retired-instruction cap (`StopReason::BudgetExhausted`).
//!
//! The colour text screen is blank: the BIOS's MDA fallback targets the separate
//! MDA card at `0xB0000`. The runner captures that card's console and cursor.
//!
//! The assertions pin the observed byte stream and the stop *kind*
//! (`BudgetExhausted`), never an address.
//!
//! The BIOS never programs the colour card, so its cursor stays at `(0, 0)`
//! and visible. MDA card capture exposes the BIOS cursor at `(0, 19)`, visible;
//! the earlier RAM probe hid it. These are the intended independent-card states.
//!
//! ## Pinned inputs
//!
//! - BIOS ROM: Sergey Kiselev `8088_bios`, Xi8088 config, built by
//!   `vendor/kiselev-bios/fetch.sh` (32 KiB, gitignored).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.
//!   Re-pinning requires re-running this golden against the same ROM and policy.

use cli::{run_bios_with_cap, StopReason};
#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/session.rs"]
pub mod session;

/// The observed POST checkpoint stream. The BIOS completes the whole
/// power-on self-test — CPU/chipset/DMA/RAM/IVT/PIT/PIC/KBC/interrupts, video
/// scan and MDA/CGA fallback (`0x10`/`0x12`), RTC (`0x20`), CPU-type and FPU
/// detect (`0x21`/`0x22`), serial and parallel scan (`0x24`/`0x25`), the RAM test
/// (`0x30`/`0x31`), the option-ROM scan (`0x40`/`0x43`) — and reaches the boot
/// loader (`e_boot` = `0x00`), where it calls INT 19h. It then spins in the
/// floppy-read retry loop because this configuration attaches no floppy.
/// Golden the exact stream.
const GOLDEN_POST: &[u8] = &[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x12, 0x20, 0x21, 0x22, 0x24, 0x25,
    0x30, 0x31, 0x40, 0x43, 0x00,
];

// The BIOS ROM is fetched and built separately, so this test stays opt-in.
// With no floppy attached, INT 19h spins until the dedicated POST cap.
#[ignore = "requires fetched BIOS ROM; no floppy attached, so INT 19h runs to the golden cap"]
#[test]
fn bios_boot_stop_point_matches_golden() {
    let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    assert_eq!(rom.len(), 32768, "Xi8088 BIOS is a 32 KiB image");

    let run = run_bios_with_cap(&rom, cli::BIOS_GOLDEN_MAX_RETIRED);
    // The bare rig's 8042 selects the 80286 model; its trap after POP DS changes
    // the open BIOS's CPU identification and adds its segment-load warning.
    // CMOS 32h=20h supplies the century in the BIOS RTC date 2000-01-01.
    session::assert_golden(
        &run,
        19_015_022,
        20_000_000,
        0xb59246d76d78f3fc,
        0x945fcce568e5d520,
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
            col: 0,
            row: 19,
            visible: true
        }
    );
    let stamp = run
        .screen_stamp
        .expect("assembled display has a capture boundary");
    assert!(stamp.retired > 0);
    assert!(stamp.virtual_time >= stamp.retired);

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
    // (no floppy attached), so it runs to the retired-instruction cap.
    assert!(
        matches!(run.stop, StopReason::BudgetExhausted),
        "expected the run to hit the retired cap spinning at the boot loader, \
         got {:?}",
        run.stop
    );
}
