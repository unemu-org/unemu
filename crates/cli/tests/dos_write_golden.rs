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
//! Builds on the interactive DIR golden ([`dos_interactive_golden`]): the same
//! Kiselev Xi8088 BIOS in masked ROM and the same bootable FreeDOS 1.44 MB floppy,
//! driven by [`cli::run_freedos_interactive_config`], but the keyboard script
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
//! The BIOS never programs the colour card, so its cursor stays at `(0, 0)`
//! and visible. MDA card capture exposes the BIOS cursor at `(4, 24)`, visible;
//! the earlier RAM probe hid it. These are the intended independent-card states.
//!
//! ## Pinned inputs
//! - BIOS: Sergey Kiselev `8088_bios`, Xi8088 config (`vendor/kiselev-bios`).
//! - FreeDOS: 8086 kernel + FreeCOM, FAT12 1.44 MB image (`vendor/freedos`).
//! - CPU: `x86jit-core`, the rev pinned in `Cargo.lock`.

use cli::{dos_write_file_script, StopReason};
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
    let run = cli::run_freedos_interactive_config(
        machine_file::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml"),
        )
        .unwrap(),
        &dos_write_file_script(),
    );
    session::assert_golden(
        &run,
        11_766_599,
        13_000_000,
        0x153da3e002975834,
        0xa6601fff99236a61,
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

/// Microsoft FAT Specification, BPB, FAT12 entries and FAT Directory Structure.
/// Follow the on-media cluster chain so DOS's cached directory cannot stand in
/// for the root entry and file contents actually written to the drive.
fn fat12_file(image: &[u8], name: &[u8; 11]) -> Option<Vec<u8>> {
    let word = |at| usize::from(u16::from_le_bytes([image[at], image[at + 1]]));
    let sector_bytes = word(11);
    let cluster_bytes = usize::from(image[13]) * sector_bytes;
    let fat_start = word(14) * sector_bytes;
    let fat_bytes = word(22) * sector_bytes;
    let root_start = fat_start + usize::from(image[16]) * fat_bytes;
    let root_bytes = word(17) * 32;
    let data_start = root_start + root_bytes.div_ceil(sector_bytes) * sector_bytes;
    let entry = image[root_start..root_start + root_bytes]
        .as_chunks::<32>()
        .0
        .iter()
        .take_while(|entry| entry[0] != 0)
        .find(|entry| entry[0] != 0xE5 && entry[11] & 0x18 == 0 && &entry[..11] == name)?;
    let size = u32::from_le_bytes(entry[28..32].try_into().unwrap()) as usize;
    assert!(size <= image.len(), "file size exceeds media size");
    let mut cluster = usize::from(u16::from_le_bytes([entry[26], entry[27]]));
    let fat = &image[fat_start..fat_start + fat_bytes];
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        assert!((2..0xFF0).contains(&cluster), "invalid FAT12 data cluster");
        let start = data_start + (cluster - 2) * cluster_bytes;
        let count = cluster_bytes.min(size - bytes.len());
        assert!(count > 0, "empty FAT12 cluster");
        bytes.extend_from_slice(&image[start..start + count]);
        let at = cluster + cluster / 2;
        let packed = u16::from_le_bytes([fat[at], fat[at + 1]]);
        cluster = usize::from(if cluster & 1 == 0 {
            packed & 0xFFF
        } else {
            packed >> 4
        });
    }
    assert!(
        size == 0 || cluster >= 0xFF8,
        "file chain must end after its data"
    );
    Some(bytes)
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
