// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real-mode VGA text-mode golden test (§9.2, §10).
//!
//! The first end-to-end path where the machine paints the **text screen**. A
//! hand-encoded **real-mode** guest runs on the PC/AT machine shape through the real
//! `X86JitCpu` in `CpuMode::Real16`. The embedder seeds DS/ES to `0xB800` so a
//! near word store reaches the colour text buffer at `0xB8000`, which is mapped
//! as trapped MMIO; the guest writes two glyph+attribute cells ("Hi", bright
//! white on black) into cells 0 and 1, then `hlt`s.
//!
//! After the run the observation facade borrows the VGA card's scanout state,
//! renders it into a [`video::TextGrid`], and presents the same frame to a
//! capture sink. The golden asserts the first two cells hold the written glyphs
//! and attributes, the rest of the screen is blank, and the grid is the full
//! 80×25 mode-3 size. Fully deterministic: nothing in the path reads a wall clock.
//!
//! ## Pinned CPU revision
//!
//! Golden traces are only valid against a known CPU revision (AGENTS.md); this
//! path runs the same pinned `x86jit-core` rev as the other real-mode goldens —
//! the rev in `Cargo.lock`. Re-baseline when that rev moves.

use cli::{run_vga_text, VGA_TEXT_PROGRAM};
use video::{Cursor, TextCell};

#[test]
fn vga_text_writes_render_into_the_grid() {
    let grid = run_vga_text(VGA_TEXT_PROGRAM).expect("machine run");

    // Full mode-3 dimensions.
    assert_eq!(grid.cols(), 80);
    assert_eq!(grid.rows(), 25);
    for row in 0..25 {
        for col in 0..80 {
            let expected = match (col, row) {
                (0, 0) => TextCell {
                    glyph: b'H',
                    attr: 0x1F,
                    style: video::TextStyle::Colour {
                        foreground: 15,
                        background: 1,
                        blink: false,
                    },
                },
                (1, 0) => TextCell {
                    glyph: b'i',
                    attr: 0x1F,
                    style: video::TextStyle::Colour {
                        foreground: 15,
                        background: 1,
                        blink: false,
                    },
                },
                _ => TextCell {
                    glyph: 0,
                    attr: 0,
                    style: video::TextStyle::Colour {
                        foreground: 0,
                        background: 0,
                        blink: false,
                    },
                },
            };
            assert_eq!(grid.cell(col, row), Some(&expected), "cell ({col}, {row})");
        }
    }
    assert_eq!(
        grid.cursor(),
        Cursor {
            col: 0,
            row: 0,
            visible: true
        }
    );

    // The human-readable dump of the first row is exactly "Hi".
    assert_eq!(grid.rows_as_strings()[0], "Hi");
}

#[test]
fn vga_snapshot_decodes_five_bit_indices_for_start_and_cursor() {
    // IBM VGA Technical Reference, CRT Controller Address Register: bits 4–0
    // select the register. Upper bits in 2Dh/2Fh/2Ah are ignored: move start
    // to cell 1, cursor to cell 3, and disable the cursor via register 0Ah.
    let program = [
        0xB8, b'A', 0x4E, 0xA3, 2, 0, 0xBA, 0xD4, 0x03, 0xB8, 0x2D, 1, 0xEF, 0xB8, 0x2F, 3, 0xEF,
        0xB8, 0x2A, 0x20, 0xEF, 0xF4,
    ];
    let grid = run_vga_text(&program).unwrap();
    assert_eq!(
        grid.cell(1, 0),
        Some(&TextCell {
            glyph: 0,
            attr: 0,
            style: video::TextStyle::Colour {
                foreground: 0,
                background: 0,
                blink: false
            }
        })
    );
    assert_eq!(
        grid.cell(0, 0),
        Some(&TextCell {
            glyph: b'A',
            attr: 0x4E,
            style: video::TextStyle::Colour {
                foreground: 14,
                background: 4,
                blink: false
            },
        })
    );
    assert_eq!(
        grid.cursor(),
        Cursor {
            col: 2,
            row: 0,
            visible: false
        }
    );
}

/// Guard the hand-encoded program bytes so an accidental edit is caught
/// independently of the run, and document every instruction's exact bytes.
///
/// Loaded at linear `0x7C00` (segment `0x0000`, IP `0x7C00`). DS is seeded to
/// `0xB800`, so a near store `mov [disp16], ax` writes to `0xB8000 + disp16`.
///
/// ```text
///   7C00: B8 48 1F     mov ax, 0x1F48        ; 'H' (0x48), attribute 0x1F
///   7C03: 89 06 00 00  mov [0x0000], ax      ; -> DS:0x0000 = 0xB8000 (cell 0)
///   7C07: B8 69 1F     mov ax, 0x1F69        ; 'i' (0x69), attribute 0x1F
///   7C0A: 89 06 02 00  mov [0x0002], ax      ; -> DS:0x0002 = 0xB8002 (cell 1)
///   7C0E: F4           hlt
/// ```
#[test]
fn vga_text_program_bytes_are_the_documented_encoding() {
    #[rustfmt::skip]
    let expected: &[u8] = &[
        0xB8, b'H', 0x1F,       // mov ax,0x1F48
        0x89, 0x06, 0x00, 0x00, // mov [0x0000],ax
        0xB8, b'i', 0x1F,       // mov ax,0x1F69
        0x89, 0x06, 0x02, 0x00, // mov [0x0002],ax
        0xF4,                   // hlt
    ];
    assert_eq!(VGA_TEXT_PROGRAM, expected);
}
