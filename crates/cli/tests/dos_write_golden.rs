// SPDX-License-Identifier: AGPL-3.0-or-later
//! DOS write golden: FreeDOS creates a file on the floppy through the keyboard
//! (§3.1, §5, §7.7, §7.8) — the end-to-end proof of the floppy WRITE path.
//!
//! Builds on the interactive DIR golden ([`dos_interactive_golden`]): the same
//! Kiselev Xi8088 BIOS in masked ROM and the same bootable FreeDOS 1.44 MB floppy,
//! driven by [`run_freedos_interactive`], but the keyboard script
//! ([`dos_write_file_script`]) makes the shell **write to the disk**:
//!
//! ```text
//!   Enter (accept date), Enter (accept time),
//!   echo x>zz  Enter,     ; redirect one byte into a new file ZZ
//!   dir        Enter      ; read the directory back from disk
//! ```
//!
//! `echo x>zz` makes FreeCOM open `ZZ` for output and write the redirected line;
//! DOS commits it through INT 21h → INT 13h AH=03h → the µPD765 **WRITE DATA**
//! path onto the FAT12 media — a directory entry plus one 512-byte data sector,
//! moved memory→device over the 8237 on channel 2. `dir` then lists the directory
//! read back **from disk**, so `ZZ` appearing in the listing means the write
//! reached media: this is the DOS-level counterpart to the controller-level write
//! golden ([`fdc_write_golden`]), closing the path FreeCOM → filesystem → FDC.
//!
//! Writes are volatile (in-memory media only, not the host image file), so this is
//! self-contained: the file exists for the life of the run and is observed by the
//! same run's `dir`.
//!
//! ## Observed session (this CPU revision)
//!
//! The captured MDA console at `0xB0000` holds the final 25-row window (the boot
//! banners have scrolled off the top), showing the tail:
//!
//! ```text
//! A:\>echo x>zz
//! A:\>dir
//!  Volume in drive A is FREEDOS
//!  Volume Serial Number is 2895-9853
//!  Directory of A:\
//! KERNEL   SYS        45,868  01-01-80  1:00a
//! COMMAND  COM        87,772  01-01-80  1:00a
//! ZZ                       3  01-01-80 12:00a
//!          3 file(s)        133,643 bytes
//!          0 dir(s)       1,323,008 bytes free
//! A:\>
//! ```
//!
//! `ZZ` is 3 bytes (`x\r\n`), and the file count is now 3 (KERNEL.SYS +
//! COMMAND.COM + ZZ) where the read-only DIR golden sees 2 — the difference is the
//! file DOS just wrote. After the listing FreeCOM busy-polls INT 16h at the final
//! `A:\>`, so the run ends at the retired-instruction cap
//! (`StopReason::BudgetExhausted`) — a terminal wait-for-input state.
//!
//! Deterministic: nothing in the path reads a wall clock, and the scancodes are
//! injected at retired-instruction boundaries (§6). Marked `#[ignore]` because it
//! runs the full BIOS POST and the FreeDOS boot chain.
//!
//! ## Pinned inputs
//! - BIOS: Sergey Kiselev `8088_bios`, Xi8088 config (`vendor/kiselev-bios`).
//! - FreeDOS: 8086 kernel + FreeCOM, FAT12 1.44 MB image (`vendor/freedos`).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.

use cli::{dos_write_file_script, run_freedos_interactive, StopReason};
use std::path::PathBuf;

fn vendor(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
        .join(p)
}

#[ignore = "slow: runs the BIOS POST then the interactive FreeDOS boot"]
#[test]
fn freedos_writes_a_file_from_the_keyboard() {
    let (Ok(rom), Ok(img)) = (
        std::fs::read(vendor("kiselev-bios/bios-xi8088.bin")),
        std::fs::read(vendor("freedos/freedos.img")),
    ) else {
        eprintln!(
            "skipping: BIOS ROM or FreeDOS image not found — run the vendor fetch.sh scripts"
        );
        return;
    };
    assert_eq!(img.len(), 1_474_560, "FreeDOS floppy is a 1.44 MB image");

    let run = run_freedos_interactive(&rom, img, &dos_write_file_script());
    let console = run.mda.rows_as_strings().join("\n");
    // Print the whole captured console so `--nocapture` shows the session; the run
    // is deterministic, so this is a stable record of what the keyboard drove.
    eprintln!(
        "--- MDA console ---\n{console}\n--- end console ---\nstop: {:?}",
        run.stop
    );

    // The typed command was echoed at the shell: the input path delivered it.
    assert!(
        console.contains("A:\\>echo x>zz"),
        "expected the A:\\> prompt with the typed echo redirection, got:\n{console}"
    );
    // The created file appears in the directory read back from disk: DOS issued
    // WRITE DATA and the µPD765 committed the directory entry + data sector to
    // media. `ZZ` with a 3-byte size (x\r\n) is the file the shell just wrote.
    assert!(
        console.contains("ZZ"),
        "expected the created file ZZ in the directory listing, got:\n{console}"
    );
    // The root now holds three files (KERNEL.SYS, COMMAND.COM, ZZ) where the
    // read-only session sees two — the write is what changed the count.
    assert!(
        console.contains("3 file(s)"),
        "expected three files after the write, got:\n{console}"
    );

    // The command ran and FreeCOM returned to its keyboard wait, so the run ends
    // at the retired-instruction cap — a terminal wait-for-input, not a spin.
    assert!(
        matches!(run.stop, StopReason::BudgetExhausted),
        "expected the run to stop at the keyboard wait, got {:?}",
        run.stop
    );
}
