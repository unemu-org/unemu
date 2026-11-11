// SPDX-License-Identifier: AGPL-3.0-or-later
//! Interactive FreeDOS golden: drive the shell past its startup prompts and run a
//! command through the keyboard (§3.1, §5, §7.5) — the input-path north star.
//!
//! Builds on the FreeDOS boot spike ([`freedos_boot_golden`]): the same Kiselev
//! Xi8088 BIOS in masked ROM and the same bootable FreeDOS 1.44 MB floppy, but
//! [`run_freedos_interactive_config`] boots to FreeCOM's date prompt and then injects a
//! **keyboard script** into the 8042, closing the full input path end to end:
//!
//! ```text
//!   set-1 scancode in the 8042  ->  IRQ1  ->  BIOS INT 09h  ->  BDA key ring
//!                               ->  DOS INT 16h read  ->  FreeCOM line input
//! ```
//!
//! The script is [`dos_dir_script`]: `Enter` (accept the date), `Enter` (accept
//! the time), then `D I R Enter` (type and run `DIR`). Each key is a set-1 make
//! code then its break code; the machine models no scancode translation, so the
//! exact bytes are what the BIOS INT 09h handler reads at port `0x60`.
//!
//! Two conditions make this work:
//!
//! - **Deferred injection (driver).** Keys queued at power-on are consumed or
//!   corrupted by the BIOS keyboard-reset handshake and the POST buffer flush, so
//!   the prompt never advances. The script is injected only once DOS owns the
//!   keyboard.
//! - **Instruction-boundary interrupts.** IRQ1 is accepted between completed
//!   instructions when IF and interrupt shadows allow it; PIT IRQ0 cannot starve
//!   the keyboard merely because the host grants a large cooperation budget.
//!
//! The whole script is queued into the 8042 at once: the controller delivers
//! queued bytes serially — shifting the next in on a tick after each read — so
//! every scancode drops and re-raises the IRQ1 line on its own and INT 09h gets a
//! clean edge per byte. (Earlier the 8042 refilled its output buffer the instant a
//! byte was read, collapsing the queued bytes into one sustained level; that is now
//! fixed in the controller model, so the driver no longer paces keys one at a time.)
//!
//! ## Observed session (this CPU revision)
//!
//! From reset the machine runs the BIOS POST, boots the FreeDOS kernel, and
//! reaches FreeCOM, which — with no `AUTOEXEC.BAT` — prompts for the date and the
//! time. The Enters accept both, giving `A:\>`; `DIR` then types and runs. The
//! captured MDA console at `0xB0000` holds the final 25-row window (the boot has
//! scrolled the early banners off the top), showing the tail:
//!
//! ```text
//! Enter new date (mm-dd-[cc]yy):
//! Current time is 12:00:07.41 am
//! Enter new time:
//! A:\>dir
//!  Volume in drive A is FREEDOS
//!  Volume Serial Number is 2895-9853
//!  Directory of A:\
//! KERNEL   SYS        45,868  01-01-80 12:00a
//! COMMAND  COM        87,772  01-01-80 12:00a
//!          2 file(s)        133,640 bytes
//!          0 dir(s)       1,323,520 bytes free
//! A:\>
//! ```
//!
//! After the listing FreeCOM busy-polls INT 16h at the final `A:\>` for the next
//! (never-arriving) keystroke while the timer IRQ0 keeps it alive, so the run ends
//! at the retired-instruction cap (`StopReason::BudgetExhausted`) — a terminal
//! wait-for-input state after the command ran, not a spin.
//!
//! Deterministic: nothing in the path reads a wall clock, and the scancodes are
//! injected at retired-instruction boundaries rather than timed (§6). Marked
//! `#[ignore]` because it runs the full BIOS POST and the FreeDOS boot chain.
//!
//! The BIOS never programs the colour card, so its cursor stays at `(0, 0)`
//! and visible. MDA card capture exposes the BIOS cursor at `(4, 24)`, visible;
//! the earlier RAM probe hid it. These are the intended independent-card states.
//!
//! ## Pinned inputs
//! - BIOS: Sergey Kiselev `8088_bios`, Xi8088 config (`vendor/kiselev-bios`).
//! - FreeDOS: 8086 kernel + FreeCOM, FAT12 1.44 MB image (`vendor/freedos`).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.

use cli::{dos_dir_script, run_freedos_interactive_config, StopReason};
#[path = "support/fixtures.rs"]
mod fixtures;

#[ignore = "slow: runs the BIOS POST then the interactive FreeDOS boot"]
#[test]
fn freedos_runs_dir_from_the_keyboard() {
    let (Some(_rom), Some(img)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
    ) else {
        return;
    };
    assert_eq!(img.len(), 1_474_560, "FreeDOS floppy is a 1.44 MB image");

    let run = run_freedos_interactive_config(
        machine_file::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml"),
        )
        .unwrap(),
        &dos_dir_script(),
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
            col: 4,
            row: 24,
            visible: true
        }
    );
    let stamp = run
        .screen_stamp
        .expect("assembled display has a capture boundary");
    assert!(stamp.retired > 0);
    assert!(stamp.virtual_time >= stamp.retired);

    let console = run.mda.rows_as_strings().join("\n");
    // Print the whole captured console so `--nocapture` shows the session; the run
    // is deterministic, so this is a stable record of what the keyboard drove.
    eprintln!(
        "--- MDA console ---\n{console}\n--- end console ---\nstop: {:?}",
        run.stop
    );

    // The keyboard Enters accepted the date/time prompts and FreeCOM gave its shell
    // prompt with the typed command echoed: the input path (scancode -> 8042 ->
    // IRQ1 -> INT 09h -> BDA ring -> INT 16h -> DOS) delivered every keystroke.
    assert!(
        console.contains("A:\\>dir"),
        "expected the A:\\> prompt with the typed DIR echoed, got:\n{console}"
    );
    // DIR ran and listed the FreeDOS floppy root: volume label plus the system
    // files. This proves the command executed, not just that the keys arrived.
    for needle in [
        "Volume in drive A is FREEDOS",
        "Directory of A:\\",
        "KERNEL",
        "COMMAND",
    ] {
        assert!(
            console.contains(needle),
            "expected the DIR listing to contain {needle:?}, got:\n{console}"
        );
    }

    // After the listing FreeCOM busy-polls at the final A:\> for the next keystroke
    // while IRQ0 keeps ticking, so the run ends at the retired cap, not idled out.
    assert!(
        matches!(run.stop, StopReason::BudgetExhausted),
        "expected the run to reach the retired cap idling at the shell after DIR, got {:?}",
        run.stop
    );
}
