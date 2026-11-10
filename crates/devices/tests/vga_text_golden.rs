// SPDX-License-Identifier: AGPL-3.0-or-later
//! Golden test for the VGA text device + presentation seam (§9.2, §10).
//!
//! Writes text into the VGA text buffer through the device's MMIO path, sets the
//! cursor through the CRTC ports, renders the buffer with the `video` crate's
//! text renderer, presents the frame into a [`CaptureFrameSink`], and asserts the
//! captured grid against a committed golden. Fully deterministic — no wall-clock,
//! no CPU.
//!
//! This test also demonstrates the decoupling: the card is a [`Device`] that owns
//! only memory and registers; the renderer and the sink live in `video`; nothing
//! here wires the card to the seam except the glue a later machine integration
//! will own.

use device::Device;
use devices::vga_text::{
    VgaText, COLS, CRTC_CURSOR_HIGH_INDEX, CRTC_CURSOR_LOW_INDEX, CRTC_DATA_PORT, CRTC_INDEX_PORT,
    ROWS, TEXT_BUFFER_BASE,
};
use video::{CaptureFrameSink, Cursor, Frame, FrameSink, TextModeState};

/// The committed screen golden: 80×25 with "HELLO" on the top row.
const GOLDEN: &str = include_str!("golden/vga_text_hello.txt");

/// Write a text cell (glyph + attribute) at cell index `n` via a 16-bit MMIO
/// write, the way a real-mode `mov word [es:di], ax` store to `0xB8000` lands.
fn write_cell(vga: &mut VgaText, n: u64, glyph: u8, attr: u8) {
    let value = u64::from(glyph) | (u64::from(attr) << 8);
    vga.mmio_write(TEXT_BUFFER_BASE + n * 2, 2, value).unwrap();
}

/// Program a CRTC register through the index/data port pair.
fn write_crtc(vga: &mut VgaText, index: u8, value: u8) {
    vga.port_write(CRTC_INDEX_PORT, 1, u32::from(index))
        .unwrap();
    vga.port_write(CRTC_DATA_PORT, 1, u32::from(value)).unwrap();
}

#[test]
fn hello_renders_to_captured_grid() {
    let mut vga = VgaText::new();

    // Write "HELLO" with attribute 0x1F (bright white on blue) into the top row.
    let attr = 0x1F;
    for (i, &glyph) in b"HELLO".iter().enumerate() {
        write_cell(&mut vga, i as u64, glyph, attr);
    }

    // Park the cursor just past the 'O' at cell 5 (row 0, col 5).
    let cursor_cell: u16 = 5;
    write_crtc(&mut vga, CRTC_CURSOR_HIGH_INDEX, (cursor_cell >> 8) as u8);
    write_crtc(&mut vga, CRTC_CURSOR_LOW_INDEX, cursor_cell as u8);

    // Render the card's memory into a frame and present it into the capture sink.
    let grid = video::render_text_mode(&TextModeState {
        vram: vga.vram(),
        cols: COLS,
        rows: ROWS,
        start_cell: vga.start_cell(),
        cursor_cell: vga.cursor_cell(),
        cursor_visible: vga.cursor_visible(),
        attributes: video::TextAttributeSet::Colour {
            blink_enabled: true,
        },
    });
    let mut sink = CaptureFrameSink::new();
    sink.present(&Frame::Text(grid));

    let captured = sink.last_text().expect("a text frame was presented");

    // The top row reads "HELLO", every other row is blank.
    let expected_rows: Vec<&str> = GOLDEN.lines().collect();
    assert_eq!(expected_rows.len(), usize::from(ROWS));
    assert_eq!(captured.rows_as_strings(), expected_rows);

    // The attribute byte rode along with the glyph.
    assert_eq!(captured.cell(0, 0).unwrap().attr, attr);

    // The cursor is at (col 5, row 0) and visible.
    assert_eq!(
        captured.cursor(),
        Cursor {
            col: 5,
            row: 0,
            visible: true,
        }
    );
}
