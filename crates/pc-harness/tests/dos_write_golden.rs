// SPDX-License-Identifier: AGPL-3.0-or-later
//! DOS write golden: FreeDOS creates a file on the floppy through the keyboard
//! (§3.1, §5, §7.7, §7.8) — the end-to-end proof of the floppy WRITE path.
//!
//! The I286 model takes the BIOS single-step CPU probe traps per the Intel
//! 80286 Programmer's Reference Manual, §9.7.1 and Appendix B, and delivers
//! real-mode exceptions through the IVT. This changes retired counts; only the
//! CPU detection line, its segment-load warning and shifted rows may change.
//!
//! Port 61h bits follow the PIT since 2026-11-11; delay loops take real refresh time.
//!
//! Keyboard replies now take the host and reply transfer time and the keyboard's
//! basic assurance test delay (IBM PC AT Technical Reference, 101/102-Key
//! Keyboard, "Basic Assurance Test"), so the BIOS keyboard reset finishes later
//! in virtual time; screen content is unchanged apart from the clock rows.
//! Normalized logical hashes and cursors are unchanged.
//!
//! Builds on the interactive DIR golden ([`dos_interactive_golden`]): the same
//! Kiselev Xi8088 BIOS in masked ROM and the same bootable FreeDOS 1.44 MB floppy,
//! driven by [`pc_harness::run_freedos_interactive_config`], but the keyboard script
//! ([`dos_write_file_script`]) makes the shell **write to the disk**:
//!
//! ```text
//!   Enter (accept date), Enter (accept time),
//!   echo x>zz  Enter,     ; redirect one byte into a new file ZZ
//!   dir        Enter      ; list the directory
//! ```
//!
//! `echo x>zz` makes FreeCOM open `ZZ` for output and write the redirected line;
//! DOS commits it through INT 21h → INT 13h AH=03h → the µPD765 **WRITE DATA**
//! path onto the FAT12 media — a directory entry plus one 512-byte data sector,
//! moved memory→device over the 8237 on channel 2. `dir` may use DOS's buffer
//! cache, so its listing alone cannot prove that the write reached media. After
//! the run, the test reads the controller's final image and checks the FAT12 root
//! entry, size and cluster data. This closes the path FreeCOM → filesystem → FDC,
//! independently of DOS's cache.
//!
//! Writes are volatile (in-memory media only, not the host image file), so this is
//! self-contained: the final in-memory media is captured by the runner.
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
//! KERNEL   SYS        45,868  01-01-80 12:00a
//! COMMAND  COM        87,772  01-01-80 12:00a
//! ZZ                       3  01-01-00 12:00a
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
//! The BIOS never programs the colour card, so its cursor stays at `(0, 0)`
//! and visible. MDA card capture exposes the BIOS cursor at `(4, 24)`, visible;
//! the earlier RAM probe hid it. These are the intended independent-card states.
//!
//! ## Pinned inputs
//! - BIOS: Sergey Kiselev `8088_bios`, Xi8088 config (`vendor/kiselev-bios`).
//! - FreeDOS: 8086 kernel + FreeCOM, FAT12 1.44 MB image (`vendor/freedos`).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.

use pc_harness::{dos_write_file_script, StopReason};
#[path = "support/fat12.rs"]
mod fat12;
use fat12::file as fat12_file;

#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/session.rs"]
pub mod session;

#[ignore = "slow: runs the BIOS POST then the interactive FreeDOS boot"]
#[test]
fn freedos_writes_a_file_from_the_keyboard() {
    let (Some(_rom), Some(img)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
    ) else {
        return;
    };
    assert_eq!(img.len(), 1_474_560, "FreeDOS floppy is a 1.44 MB image");

    assert!(fat12_file(&img, b"ZZ         ").is_none(), "ZZ must be new");
    let run = pc_harness::run_freedos_interactive_config(
        machine_file::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml"),
        )
        .unwrap(),
        &dos_write_file_script(),
    );
    // The BIOS reads 32h=20h; FreeDOS reports Sat 01-01-2000 and stamps ZZ with year 00.
    session::assert_golden(
        &run,
        11_766_596,
        13_000_000,
        0x71f290c7a24fb363,
        0x6b0404d06d7532c3,
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

    // The typed command was echoed at the shell: the input path delivered it.
    assert!(
        console.contains("A:\\>echo x>zz"),
        "expected the A:\\> prompt with the typed echo redirection, got:\n{console}"
    );
    // The listing proves the shell saw the file; the media assertion below
    // independently checks persistence beyond DOS's buffer cache.
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

    let final_image = run.floppy_image.as_deref().expect("final floppy media");
    let contents = fat12_file(final_image, b"ZZ         ")
        .expect("new ZZ root directory entry must reach floppy media");
    assert_eq!(contents.len(), 3, "on-media ZZ directory size");
    assert_eq!(contents, b"x\r\n", "on-media ZZ file contents");

    // The command ran and FreeCOM returned to its keyboard wait, so the run ends
    // at the retired-instruction cap — a terminal wait-for-input, not a spin.
    assert!(
        matches!(run.stop, StopReason::BudgetExhausted),
        "expected the run to stop at the keyboard wait, got {:?}",
        run.stop
    );
}

#[test]
fn fat12_reads_noncontiguous_even_and_odd_clusters() {
    let mut image = vec![0u8; 7 * 512];
    image[11..13].copy_from_slice(&512u16.to_le_bytes());
    image[13] = 1;
    image[14] = 1;
    image[16] = 1;
    image[17] = 16;
    image[22] = 1;
    assert!(fat12_file(&image, b"ZZ         ").is_none());
    image[1024..1035].copy_from_slice(b"ZZ         ");
    image[1024 + 26] = 2;
    image[1024 + 28..1024 + 32].copy_from_slice(&515u32.to_le_bytes());
    // Cluster 2 points to 5; the odd entry 5 ends the chain.
    image[512 + 3] = 5;
    image[512 + 7] = 0xF0;
    image[512 + 8] = 0xFF;
    image[3 * 512..4 * 512].fill(b'a');
    image[6 * 512..6 * 512 + 3].copy_from_slice(b"xyz");
    let mut expected = vec![b'a'; 512];
    expected.extend_from_slice(b"xyz");
    assert_eq!(fat12_file(&image, b"ZZ         "), Some(expected));
}

#[test]
fn added_files_roundtrip_fragmented_clusters_and_both_fats() {
    let mut image = vec![0; 12 * 512];
    image[11..13].copy_from_slice(&512u16.to_le_bytes());
    image[13] = 1;
    image[14] = 1;
    image[16] = 2;
    image[17] = 16;
    image[22] = 1;
    // Reserve cluster three to force an even-to-even noncontiguous chain.
    image[512 + 4..512 + 6].copy_from_slice(&0xFFF0u16.to_le_bytes());
    image[1024 + 4..1024 + 6].copy_from_slice(&0xFFF0u16.to_le_bytes());
    fat12::add_file(&mut image, b"TEST    BIN", &[0xA5; 1300]);
    fat12::add_file(&mut image, b"EMPTY   TXT", &[]);
    assert_eq!(fat12_file(&image, b"TEST    BIN"), Some(vec![0xA5; 1300]));
    assert_eq!(fat12_file(&image, b"EMPTY   TXT"), Some(vec![]));
    assert_eq!(&image[512..1024], &image[1024..1536]);
}
