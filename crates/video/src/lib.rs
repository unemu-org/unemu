// SPDX-License-Identifier: AGPL-3.0-or-later
//! Presentation swap seam (§3.2 seam 5, §9.1, §9.2).
//!
//! [`FrameSink`] is the fifth of the six swap seams from
//! `backlog/docs/architecture.md`: it is the boundary between the machine (which
//! produces a finished [`Frame`]) and whatever presents it — a terminal, a
//! windowed framebuffer, or a test capture. Nothing upstream of this trait names
//! a concrete presenter.
//!
//! A video **card** (MDA/CGA/VGA) is an ordinary [`device::Device`] and is *not*
//! part of this seam: the card only owns registers and video memory. Turning a
//! card's memory into a [`Frame`] is the job of a renderer (here,
//! [`render_text_mode`]); handing that frame to a consumer is the job of a
//! `FrameSink`. Keeping the two apart is what lets the same VGA card feed a
//! terminal, a GUI, or a golden test without the card knowing the difference.
//!
//! ## What is modelled here
//! - The [`FrameSink`] trait and the [`Frame`] it carries.
//! - [`TextGrid`] — the character-cell frame for text modes: a grid of
//!   (glyph, attribute) cells plus a [`Cursor`].
//! - [`render_text_mode`] — turns a slice of VGA text video memory into a
//!   [`TextGrid`] (§9.2, VGA text mode 3, 80×25).
//! - [`CaptureFrameSink`] — a trivial sink that keeps the last frame, for tests.
//!
//! ## Not modelled yet
//! - No pixel/graphics frame: [`Frame`] has only a text variant so far. The enum
//!   exists precisely so a future framebuffer variant is an additive change, not
//!   a reshape of the seam.
//! - No glyph rasterisation: a text frame carries code-page bytes and attribute
//!   bytes, not pixels. Mapping a glyph byte + font to pixels is a presenter's
//!   concern, deliberately left out of the seam.

/// A finished frame handed across the [`FrameSink`] seam (§9.1).
///
/// Presentation-mode agnostic on purpose: today the only variant is
/// [`Frame::Text`], but a future pixel framebuffer becomes a new variant without
/// disturbing the trait or its existing consumers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// A character-cell frame produced by a text mode (§9.2).
    Text(TextGrid),
}

/// The presentation swap seam (§3.2 seam 5, §9.1).
///
/// A consumer of finished frames. The machine calls [`FrameSink::present`] with a
/// freshly rendered [`Frame`]; what happens next (draw to a terminal, blit to a
/// window, store for assertions) is entirely the sink's business.
pub trait FrameSink {
    /// Present a finished frame. Called once per rendered frame; the sink must
    /// not retain the borrow beyond the call.
    fn present(&mut self, frame: &Frame);
}

/// One character cell in a [`TextGrid`].
///
/// Mirrors the two bytes a VGA text cell occupies in video memory: a code-page
/// glyph index and an attribute byte (foreground/background/blink). The glyph is
/// kept as a raw byte, not a `char`, because interpreting it needs a code page
/// and font that live in the presenter, not the seam.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TextCell {
    /// Code-page glyph index (e.g. CP437 on the IBM PC).
    pub glyph: u8,
    /// Attribute byte: typically `bg<<4 | fg`, with the top bit blink/intensity.
    pub attr: u8,
}

/// The text-mode cursor position within a [`TextGrid`] (§9.2).
///
/// Coordinates are grid-relative (`0..cols`, `0..rows`), already resolved from
/// the card's absolute cursor-location register and display start address.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Column, `0..cols`.
    pub col: u16,
    /// Row, `0..rows`.
    pub row: u16,
    /// Whether the cursor is displayed at all (the card can hide it).
    pub visible: bool,
}

/// A character-cell frame: a `cols × rows` grid of [`TextCell`]s plus a
/// [`Cursor`] (§9.2).
///
/// Row-major: cell `(col, row)` is at index `row * cols + col`. This is the
/// natural, presenter-independent representation of a text screen — small enough
/// to clone into a capture sink and to diff against a committed golden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextGrid {
    cols: u16,
    rows: u16,
    cells: Vec<TextCell>,
    cursor: Cursor,
}

impl TextGrid {
    /// Column count.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Row count.
    #[must_use]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// The cell at `(col, row)`, or `None` if out of bounds.
    #[must_use]
    pub fn cell(&self, col: u16, row: u16) -> Option<&TextCell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        self.cells
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
    }

    /// The cursor.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Render each row to a `String`, one entry per row, with trailing blanks
    /// trimmed.
    ///
    /// Glyph bytes are interpreted as Latin-1 (`char::from(byte)`); the null
    /// glyph `0x00` — an untouched cell — is rendered as a space so blank screen
    /// area reads as whitespace. This is a text-dump for human-readable golden
    /// comparison, not a faithful CP437 rendering.
    #[must_use]
    pub fn rows_as_strings(&self) -> Vec<String> {
        (0..self.rows)
            .map(|row| {
                let mut line = String::with_capacity(usize::from(self.cols));
                for col in 0..self.cols {
                    let glyph = self.cell(col, row).map_or(0, |c| c.glyph);
                    line.push(if glyph == 0 { ' ' } else { char::from(glyph) });
                }
                line.trim_end().to_string()
            })
            .collect()
    }

    /// The whole grid as a newline-joined text dump (one line per row, trailing
    /// blanks trimmed). Convenience over [`TextGrid::rows_as_strings`] for goldens.
    #[must_use]
    pub fn to_dump(&self) -> String {
        self.rows_as_strings().join("\n")
    }
}

/// The inputs [`render_text_mode`] needs from a text-mode video card (§9.2).
///
/// Deliberately plain data (a memory slice + a handful of register-derived
/// scalars) so the renderer depends on no concrete device type — a VGA card, a
/// CGA card, or a test fixture all populate the same struct.
pub struct TextModeState<'a> {
    /// Text video memory: two bytes per cell (glyph, then attribute),
    /// little-endian within a 16-bit cell. Read starting at `start_cell`.
    pub vram: &'a [u8],
    /// Visible columns (80 for mode 3).
    pub cols: u16,
    /// Visible rows (25 for mode 3).
    pub rows: u16,
    /// Display start address, in cells (CRTC start-address registers). The
    /// top-left cell is at byte offset `start_cell * 2`.
    pub start_cell: u16,
    /// Absolute cursor location, in cells (CRTC cursor-location registers). Its
    /// grid position is measured relative to `start_cell`.
    pub cursor_cell: u16,
    /// Whether the card is displaying the cursor at all.
    pub cursor_visible: bool,
}

/// Render VGA text video memory into a [`TextGrid`] (§9.2).
///
/// Reads `cols * rows` cells starting at `start_cell`, two bytes each. Cells that
/// fall past the end of `vram` render as blank (glyph 0, attr 0) rather than
/// panicking. The cursor's grid position is `cursor_cell - start_cell`; a cursor
/// before the start address or past the last row is reported not visible.
#[must_use]
pub fn render_text_mode(state: &TextModeState) -> TextGrid {
    let cols = state.cols;
    let rows = state.rows;
    let count = usize::from(cols) * usize::from(rows);
    let mut cells = Vec::with_capacity(count);
    for i in 0..count {
        let cell_index = usize::from(state.start_cell) + i;
        let byte = cell_index * 2;
        let glyph = state.vram.get(byte).copied().unwrap_or(0);
        let attr = state.vram.get(byte + 1).copied().unwrap_or(0);
        cells.push(TextCell { glyph, attr });
    }

    let cursor = if state.cursor_cell >= state.start_cell {
        let rel = state.cursor_cell - state.start_cell;
        let col = rel % cols;
        let row = rel / cols;
        Cursor {
            col,
            row,
            visible: state.cursor_visible && row < rows,
        }
    } else {
        Cursor {
            col: 0,
            row: 0,
            visible: false,
        }
    };

    TextGrid {
        cols,
        rows,
        cells,
        cursor,
    }
}

/// A trivial [`FrameSink`] that keeps the most recently presented frame (§10).
///
/// The default test sink: present a frame, then inspect it. Holds exactly one
/// frame (the last), so a test asserts against a known, deterministic snapshot.
#[derive(Default)]
pub struct CaptureFrameSink {
    last: Option<Frame>,
}

impl CaptureFrameSink {
    /// Create an empty capture sink (no frame presented yet).
    #[must_use]
    pub fn new() -> Self {
        Self { last: None }
    }

    /// The last presented frame, or `None` if nothing has been presented.
    #[must_use]
    pub fn last(&self) -> Option<&Frame> {
        self.last.as_ref()
    }

    /// The last presented frame as a [`TextGrid`], or `None` if there is no frame
    /// or it was not a text frame.
    #[must_use]
    pub fn last_text(&self) -> Option<&TextGrid> {
        match self.last.as_ref()? {
            Frame::Text(grid) => Some(grid),
        }
    }
}

impl FrameSink for CaptureFrameSink {
    fn present(&mut self, frame: &Frame) {
        self.last = Some(frame.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 4-cell-wide vram fragment "AB" with attributes.
    fn vram_ab() -> Vec<u8> {
        // Cell 0: 'A' attr 0x07, cell 1: 'B' attr 0x70.
        vec![b'A', 0x07, b'B', 0x70]
    }

    #[test]
    fn render_places_glyphs_and_attributes() {
        let vram = vram_ab();
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: 2,
            rows: 1,
            start_cell: 0,
            cursor_cell: 1,
            cursor_visible: true,
        });
        assert_eq!(
            grid.cell(0, 0),
            Some(&TextCell {
                glyph: b'A',
                attr: 0x07
            })
        );
        assert_eq!(
            grid.cell(1, 0),
            Some(&TextCell {
                glyph: b'B',
                attr: 0x70
            })
        );
        assert_eq!(grid.cell(2, 0), None);
    }

    #[test]
    fn cursor_is_grid_relative_to_start_address() {
        let vram = vec![0u8; 8];
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: 2,
            rows: 2,
            start_cell: 1,
            cursor_cell: 3, // one past start -> rel 2 -> (col 0, row 1)
            cursor_visible: true,
        });
        assert_eq!(
            grid.cursor(),
            Cursor {
                col: 0,
                row: 1,
                visible: true
            }
        );
    }

    #[test]
    fn cursor_before_start_is_hidden() {
        let vram = vec![0u8; 8];
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: 2,
            rows: 2,
            start_cell: 2,
            cursor_cell: 0,
            cursor_visible: true,
        });
        assert!(!grid.cursor().visible);
    }

    #[test]
    fn out_of_range_cells_render_blank() {
        let vram: Vec<u8> = Vec::new();
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: 2,
            rows: 1,
            start_cell: 0,
            cursor_cell: 0,
            cursor_visible: false,
        });
        assert_eq!(grid.cell(0, 0), Some(&TextCell { glyph: 0, attr: 0 }));
    }

    #[test]
    fn dump_renders_null_as_space_and_trims() {
        let mut vram = vec![0u8; 2 * 4];
        // "Hi" on a 4-wide, 1-row grid.
        vram[0] = b'H';
        vram[2] = b'i';
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: 4,
            rows: 1,
            start_cell: 0,
            cursor_cell: 0,
            cursor_visible: false,
        });
        assert_eq!(grid.rows_as_strings(), vec!["Hi".to_string()]);
    }

    #[test]
    fn capture_sink_keeps_last_frame() {
        let mut sink = CaptureFrameSink::new();
        assert!(sink.last().is_none());
        let grid = render_text_mode(&TextModeState {
            vram: &vram_ab(),
            cols: 2,
            rows: 1,
            start_cell: 0,
            cursor_cell: 0,
            cursor_visible: false,
        });
        sink.present(&Frame::Text(grid.clone()));
        assert_eq!(sink.last_text(), Some(&grid));
    }
}
