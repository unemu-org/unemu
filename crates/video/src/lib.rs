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
//! - [`render_text_mode`] — turns MDA or colour text video memory into a
//!   [`TextGrid`] (§9.2, VGA text mode 3, 80×25).
//! - [`CaptureFrameSink`] — a trivial sink that keeps the last frame, for tests.
//!
//! Ordinary text frames carry code-page bytes, decoded styles and optional
//! guest fonts for the presenter. Scrolled VGA text uses indexed pixels instead,
//! because partial cells and splits through glyphs require rasterisation here.

#![forbid(unsafe_code)]

/// A finished frame handed across the [`FrameSink`] seam (§9.1).
///
/// Owned text or indexed pixels, independent of host presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// A character-cell frame produced by a text mode (§9.2).
    Text(TextGrid),
    /// Logical graphics pixels and their captured RGB palette.
    Pixels(IndexedPixels),
}

/// An owned row-major indexed image. Indices address the captured DAC palette,
/// keeping capture compact and independent of subsequent guest palette writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedPixels {
    /// Logical width, without dot-clock repetition.
    pub width: u16,
    /// Logical height, without repeated scan lines.
    pub height: u16,
    /// Horizontal dot and vertical scan-line repetitions for presentation.
    pub repeat: (u8, u8),
    /// Exactly width × height DAC indices.
    pub indices: Vec<u8>,
    /// Captured RGB888 DAC entries, including the PEL mask.
    pub palette: Box<[[u8; 3]]>,
}

struct VgaScanLine {
    lower_screen: bool,
    counter_row: u16,
    row_scan: u16,
    pixel_pan: u8,
}

/// IBM VGA/XGA Technical Reference Manual, Preset Row Scan, Maximum Scan Line,
/// Line Compare and Attribute Mode Control. ASSUMED: the first lower line
/// resets both row scan and double-scan phase.
fn vga_scan_line(scroll: device::VgaScroll, y: u16, height: u8) -> VgaScanLine {
    let lower_screen = y >= scroll.split_line;
    let scan = if lower_screen {
        (y - scroll.split_line) / u16::from(scroll.double_scan)
    } else {
        y / u16::from(scroll.double_scan) + u16::from(scroll.preset_row)
    };
    VgaScanLine {
        lower_screen,
        counter_row: scan / u16::from(height),
        row_scan: scan % u16::from(height),
        pixel_pan: if lower_screen && scroll.split_pan_reset {
            0
        } else {
            scroll.pixel_pan
        },
    }
}

/// Render VGA graphics without reading ports or updating CPU latches.
/// IBM VGA/XGA Technical Reference Manual, Graphics Modes, Figs. 2-65, 2-75
/// and 2-84: planar bits select the attribute palette. Fig. 2-64 supplies
/// row-scan address substitution for CGA compatibility. Mode 13h's pairing
/// through the internal palette low nibble is DERIVED/ASSUMED, as documented
/// with the manual's qualifications in `backlog/docs/design/observation.md`.
#[must_use]
pub fn render_graphics(state: &device::GraphicsScanout<'_>) -> IndexedPixels {
    if let Some(text) = &state.text {
        return render_planar_text_pixels(text);
    }
    let (width, height) = state.dimensions;
    let mut indices = vec![0; usize::from(width) * usize::from(height)];
    if state.enabled {
        for y in 0..height {
            let line = vga_scan_line(
                state.scroll,
                y * u16::from(state.repeat.1),
                state.row_scan_height,
            );
            for x in 0..width {
                let x_source = x + u16::from(line.pixel_pan);
                let group = if state.packed {
                    x_source / 4
                } else {
                    x_source / 8
                };
                let address = (if line.lower_screen { 0 } else { state.start })
                    .wrapping_add(line.counter_row.wrapping_mul(state.row_stride))
                    .wrapping_add(group);
                let byte = match state.address_shift {
                    0 => address,
                    1 => (address << 1) | ((address >> state.word_wrap_bit) & 1),
                    _ => (address << 2) | ((address >> 12) & 3),
                };
                let byte = (byte.wrapping_add(u16::from(state.scroll.byte_pan))
                    & !state.row_scan_mask)
                    | (((line.row_scan & 3) << 13) & state.row_scan_mask);
                let index = if state.packed {
                    let value = state.planes[usize::from(x_source & 3)][usize::from(byte)];
                    (state.colours[usize::from(value & 15)] & 15)
                        | ((state.colours[usize::from(value >> 4)] & 15) << 4)
                } else {
                    let mut colour = 0;
                    for plane in 0..4 {
                        colour |=
                            ((state.planes[plane][usize::from(byte)] >> (7 - (x_source & 7))) & 1)
                                << plane;
                    }
                    state.colours[usize::from(colour)]
                };
                indices[usize::from(y) * usize::from(width) + usize::from(x)] = index;
            }
        }
    }
    IndexedPixels {
        width,
        height,
        indices,
        repeat: state.repeat,
        palette: Box::new(if state.enabled {
            state.palette
        } else {
            [[0; 3]; 256]
        }),
    }
}

/// Rasterise scrolled VGA text when clipping or a split cannot fit a cell grid.
/// IBM VGA/XGA Technical Reference Manual, Alphanumeric Modes, Figs. 2-49,
/// 2-50, 2-66, 2-84 and 2-88. Blink and cursor use a steady visible phase,
/// matching ordinary text presentation without introducing a host clock.
#[must_use]
pub fn render_planar_text_pixels(state: &device::PlanarTextState<'_>) -> IndexedPixels {
    let (cell_width, cell_height) = state.cell_size;
    let width = state.dimensions.0 * u16::from(cell_width);
    let height = state.scan_lines;
    let mut indices = vec![0; usize::from(width) * usize::from(height)];
    if state.enabled {
        for y in 0..height {
            let line = vga_scan_line(state.scroll, y, cell_height);
            for x in 0..width {
                let source = x + u16::from(line.pixel_pan);
                let col = source / u16::from(cell_width);
                let dot = source % u16::from(cell_width);
                let address = (if line.lower_screen { 0 } else { state.start })
                    .wrapping_add(line.counter_row.wrapping_mul(state.row_stride))
                    .wrapping_add(col)
                    .wrapping_add(u16::from(state.scroll.byte_pan));
                let byte = match state.address_shift {
                    0 => address,
                    1 => (address << 1) | ((address >> state.word_wrap_bit) & 1),
                    _ => (address << 2) | ((address >> 12) & 3),
                };
                let glyph = state.planes[0][usize::from(byte)];
                let attr = state.planes[1][usize::from(byte)];
                let map = usize::from(state.dual_font && attr & 8 != 0);
                let bits = state.planes[2][state.font_offsets[map]
                    + usize::from(glyph) * 32
                    + usize::from(line.row_scan)];
                let ink = if dot < 8 {
                    bits & (0x80 >> dot) != 0
                } else {
                    state.line_graphics && (0xc0..=0xdf).contains(&glyph) && bits & 1 != 0
                };
                let underline = attr & 0x77 == 1 && line.row_scan == u16::from(state.underline);
                let cursor = address == state.cursor
                    && state.cursor_shape.0
                    && line.row_scan >= u16::from(state.cursor_shape.1)
                    && line.row_scan <= u16::from(state.cursor_shape.2);
                let colour = if ink || underline || cursor {
                    attr & 15
                } else {
                    (attr >> 4) & if state.blink_enabled { 7 } else { 15 }
                };
                indices[usize::from(y) * usize::from(width) + usize::from(x)] = colour;
            }
        }
    }
    IndexedPixels {
        width,
        height,
        repeat: (state.horizontal_repeat, 1),
        indices,
        palette: (if state.enabled {
            state.palette
        } else {
            [[0; 3]; 16]
        })
        .into(),
    }
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
/// Preserves the two bytes a text cell occupies in video memory alongside a
/// decoded style, so presenters need no knowledge of the producing card. The
/// glyph is kept as a raw byte, not a `char`, because interpreting it needs a code page
/// and font. A grid can capture a guest font; otherwise the presenter supplies it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TextCell {
    /// Code-page glyph index (e.g. CP437 on the IBM PC).
    pub glyph: u8,
    /// Raw attribute byte, preserved independently of its card-specific meaning.
    pub attr: u8,
    /// Decoded drawing instructions; glyph rasterisation stays with the presenter.
    pub style: TextStyle,
}

/// MDA's base display effect, independent of intensity and blink (§9.3).
///
/// IBM PC Technical Reference, Options and Adapters, IBM Monochrome Display
/// and Printer Adapter, Programming Considerations, attribute byte table.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MonochromeEffect {
    /// Black space, including attributes 00h, 08h, 80h and 88h.
    NonDisplay,
    /// Light character on a dark background.
    Normal,
    /// Underlined character: foreground bits 0–2 are 001, with any background.
    Underline,
    /// Dark character on a light background (base attribute 70h).
    Reverse,
}

/// Decoded text attributes that require no device registers to interpret (§9.2, §9.3).
///
/// IBM PC Technical Reference, Options and Adapters, IBM Monochrome Display
/// and Printer Adapter, Programming Considerations, attribute byte table and
/// mode-control register 3B8; IBM VGA Technical Reference, Alphanumeric Modes
/// and Attribute Mode Control. See `backlog/docs/design/spec.md`, §9.2 and §9.3.
/// Blink is a request, not a timed phase; this renderer never reads a host clock.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextStyle {
    /// Colour attributes using the standard IBM text palette indices.
    /// Guest palettes, when present, are supplied by [`TextGrid::appearance`].
    Colour {
        /// Foreground index 0–15; bit 3 is foreground intensity.
        foreground: u8,
        /// Background index 0–15; bit 3 is background intensity.
        background: u8,
        /// Alternate the glyph with its background during the blink phase.
        blink: bool,
    },
    /// Monochrome display effects; the presenter chooses the phosphor colour.
    Monochrome {
        /// Base effect; non-display stays black regardless of intensity.
        effect: MonochromeEffect,
        /// Character intensity (attribute bit 3).
        intense: bool,
        /// Alternate the glyph with its background during the blink phase.
        blink: bool,
        /// Background intensity (attribute bit 7 with blink disabled).
        /// Always false for non-display cells, which stay black.
        background_intense: bool,
    },
}

fn decode_text_style(attr: u8, attributes: TextAttributeSet) -> TextStyle {
    match attributes {
        TextAttributeSet::Colour { blink_enabled } => TextStyle::Colour {
            foreground: attr & 0x0F,
            background: (attr >> 4) & if blink_enabled { 0x07 } else { 0x0F },
            blink: blink_enabled && attr & 0x80 != 0,
        },
        TextAttributeSet::Monochrome { blink_enabled } => TextStyle::Monochrome {
            effect: match attr & 0x77 {
                0x00 => MonochromeEffect::NonDisplay,
                0x70 => MonochromeEffect::Reverse,
                _ if attr & 0x07 == 0x01 => MonochromeEffect::Underline,
                _ => MonochromeEffect::Normal,
            },
            intense: attr & 0x08 != 0,
            blink: blink_enabled && attr & 0x80 != 0,
            background_intense: !blink_enabled && attr & 0x80 != 0 && attr & 0x77 != 0,
        },
    }
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
    appearance: Option<TextAppearance>,
}

/// Guest-supplied text presentation data (IBM VGA/XGA Technical Reference
/// Manual, Alphanumeric Modes). Absent on legacy MDA and text-only VGA frames.
/// Font rows use the low nine bits, with the leftmost dot at `cell_width - 1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextAppearance {
    /// Dot width and scan-line height of each cell.
    pub cell_width: u8,
    /// Horizontal repetitions of each dot (1 or 2), from the sequencer clock.
    pub horizontal_repeat: u8,
    /// Number of font rows displayed per cell.
    pub cell_height: u8,
    /// Fonts for attribute bit 3 clear/set, 256 glyphs × 32 rows each.
    pub fonts: [Vec<u16>; 2],
    /// Attribute bit 3 additionally selects between fonts; colour is preserved.
    pub dual_font: bool,
    /// RGB colours indexed by decoded foreground/background styles.
    pub palette: [[u8; 3]; 16],
    /// Inclusive cursor scan-line range.
    pub cursor_start: u8,
    /// Inclusive cursor end; an end before start hides the VGA cursor.
    pub cursor_end: u8,
    /// Underline scan line; values outside the cell suppress underlining.
    pub underline: u8,
    /// False means a blank display, retaining the grid for text inspection.
    pub enabled: bool,
}

impl TextGrid {
    /// Optional guest font, palette and scan-line geometry.
    #[must_use]
    pub fn appearance(&self) -> Option<&TextAppearance> {
        self.appearance.as_ref()
    }

    /// Guest font row for a cell, with line-graphics dot nine already resolved.
    #[must_use]
    pub fn glyph_row(&self, cell: &TextCell, row: u8) -> Option<u16> {
        let appearance = self.appearance.as_ref()?;
        if row >= appearance.cell_height {
            return None;
        }
        let map = usize::from(appearance.dual_font && cell.attr & 8 != 0);
        appearance.fonts[map]
            .get(usize::from(cell.glyph) * 32 + usize::from(row))
            .copied()
    }

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

/// Hardware-neutral scanout consumed by the text renderer (§9.2).
pub use device::{PlanarTextState, TextAttributeSet, TextModeState};

/// Render MDA or colour text video memory into a [`TextGrid`] (§9.2).
///
/// IBM VGA Technical Reference, CRT Controller Registers, Start Address and
/// Cursor Location: scanout and cursor comparison use video-memory addresses.
/// The cursor compares the full 16-bit location with the address counter.
/// ASSUMED: scanout wraps at the supplied CPU memory aperture (complete cells,
/// at most 65,536), rather than modelling VGA plane/address-mode wiring.
/// Empty memory renders blank; zero dimensions hide the cursor.
#[must_use]
pub fn render_text_mode(state: &TextModeState) -> TextGrid {
    let cols = state.cols;
    let rows = state.rows;
    let count = usize::from(cols) * usize::from(rows);
    let memory_cells = (state.vram.len() / 2).min(65536);
    let mut cells = Vec::with_capacity(count);
    for i in 0..count {
        let cell = if memory_cells == 0 {
            TextCell {
                glyph: 0,
                attr: 0,
                style: decode_text_style(0, state.attributes),
            }
        } else {
            let byte = (((usize::from(state.start_cell) + i) & 0xFFFF) % memory_cells) * 2;
            TextCell {
                glyph: state.vram[byte],
                attr: state.vram[byte + 1],
                style: decode_text_style(state.vram[byte + 1], state.attributes),
            }
        };
        cells.push(cell);
    }

    let mut cursor = Cursor {
        col: 0,
        row: 0,
        visible: false,
    };
    if cols != 0 && rows != 0 && memory_cells != 0 {
        let rel = usize::from(state.cursor_cell.wrapping_sub(state.start_cell));
        if rel < count {
            cursor = Cursor {
                col: (rel % usize::from(cols)) as u16,
                row: (rel / usize::from(cols)) as u16,
                visible: state.cursor_visible,
            };
        }
    }

    TextGrid {
        cols,
        rows,
        cells,
        cursor,
        appearance: None,
    }
}

/// Render planar alphanumeric scanout without rasterising it.
///
/// IBM VGA/XGA Technical Reference Manual, Alphanumeric Modes, Figs. 2-7–2-8,
/// 2-33–2-35, 2-60, 2-65 and 2-84. See `backlog/docs/design/observation.md`.
/// Fonts are captured once per observation, so later guest writes cannot change
/// a retained frame. Blink is a request; no host-time phase is introduced.
#[must_use]
pub fn render_planar_text(state: &device::PlanarTextState<'_>) -> TextGrid {
    let (cols, rows) = state.dimensions;
    let (cell_width, cell_height) = state.cell_size;
    let fonts = state.font_offsets.map(|base| {
        (0..256 * 32)
            .map(|offset| {
                let bits = u16::from(state.planes[2][base + offset]);
                if cell_width == 8 {
                    bits
                } else {
                    let ninth = state.line_graphics
                        && (0xC0..=0xDF).contains(&(offset / 32))
                        && bits & 1 != 0;
                    (bits << 1) | u16::from(ninth)
                }
            })
            .collect()
    });
    let mut cells = Vec::with_capacity(usize::from(cols) * usize::from(rows));
    let mut cursor = Cursor {
        col: 0,
        row: 0,
        visible: false,
    };
    for row in 0..rows {
        for col in 0..cols {
            let address = state
                .start
                .wrapping_add(row.wrapping_mul(state.row_stride))
                .wrapping_add(col);
            let byte = match state.address_shift {
                0 => address,
                1 => (address << 1) | ((address >> state.word_wrap_bit) & 1),
                _ => (address << 2) | ((address >> 12) & 3),
            };
            let glyph = state.planes[0][usize::from(byte)];
            let attr = state.planes[1][usize::from(byte)];
            cells.push(TextCell {
                glyph,
                attr,
                style: decode_text_style(
                    attr,
                    TextAttributeSet::Colour {
                        blink_enabled: state.blink_enabled,
                    },
                ),
            });
            if address == state.cursor && !cursor.visible {
                cursor = Cursor {
                    col,
                    row,
                    visible: state.enabled
                        && state.cursor_shape.0
                        && state.cursor_shape.1 <= state.cursor_shape.2
                        && state.cursor_shape.1 < cell_height,
                };
            }
        }
    }
    TextGrid {
        cols,
        rows,
        cells,
        cursor,
        appearance: Some(TextAppearance {
            cell_width,
            horizontal_repeat: state.horizontal_repeat,
            cell_height,
            fonts,
            dual_font: state.dual_font,
            palette: state.palette,
            cursor_start: state.cursor_shape.1,
            cursor_end: state.cursor_shape.2,
            underline: state.underline,
            enabled: state.enabled,
        }),
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
            Frame::Pixels(_) => None,
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

    #[test]
    fn mda_attribute_classes_decode_without_changing_raw_cells_or_dumps() {
        use MonochromeEffect::{NonDisplay, Normal, Reverse, Underline};
        let cases = [
            (0x00, NonDisplay, false, false),
            (0x08, NonDisplay, true, false),
            (0x80, NonDisplay, false, true),
            (0x88, NonDisplay, true, true),
            (0x01, Underline, false, false),
            (0x09, Underline, true, false),
            (0x81, Underline, false, true),
            (0x89, Underline, true, true),
            (0x07, Normal, false, false),
            (0x0F, Normal, true, false),
            (0x87, Normal, false, true),
            (0x8F, Normal, true, true),
            (0x70, Reverse, false, false),
            (0x78, Reverse, true, false),
            (0xF0, Reverse, false, true),
            (0xF8, Reverse, true, true),
            (0x02, Normal, false, false),
            (0x71, Underline, false, false),
            (0x11, Underline, false, false),
        ];
        let vram: Vec<u8> = cases.iter().flat_map(|&(attr, ..)| [b'X', attr]).collect();
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: cases.len() as u16,
            rows: 1,
            start_cell: 0,
            cursor_cell: 0,
            cursor_visible: false,
            attributes: TextAttributeSet::Monochrome {
                blink_enabled: true,
            },
        });
        for (col, &(attr, effect, intense, blink)) in cases.iter().enumerate() {
            let cell = grid.cell(col as u16, 0).unwrap();
            assert_eq!((cell.glyph, cell.attr), (b'X', attr));
            assert_eq!(
                cell.style,
                TextStyle::Monochrome {
                    effect,
                    intense,
                    blink,
                    background_intense: false,
                },
                "attr={attr:#04x}"
            );
        }
        assert_eq!(grid.rows_as_strings(), vec!["X".repeat(cases.len())]);
        assert_eq!(grid.to_dump(), "X".repeat(cases.len()));
    }

    #[test]
    fn mda_bit_seven_selects_background_intensity_when_blink_is_disabled() {
        let grid = render_text_mode(&TextModeState {
            vram: &[
                b'A', 0x07, b'B', 0x87, b'C', 0x89, b'D', 0xF8, b'E', 0x80, b'F', 0x88,
            ],
            cols: 6,
            rows: 1,
            start_cell: 0,
            cursor_cell: 0,
            cursor_visible: false,
            attributes: TextAttributeSet::Monochrome {
                blink_enabled: false,
            },
        });
        for (col, effect, intense, background_intense) in [
            (0, MonochromeEffect::Normal, false, false),
            (1, MonochromeEffect::Normal, false, true),
            (2, MonochromeEffect::Underline, true, true),
            (3, MonochromeEffect::Reverse, true, true),
            (4, MonochromeEffect::NonDisplay, false, false),
            (5, MonochromeEffect::NonDisplay, true, false),
        ] {
            assert_eq!(
                grid.cell(col, 0).unwrap().style,
                TextStyle::Monochrome {
                    effect,
                    intense,
                    blink: false,
                    background_intense,
                }
            );
        }
    }

    #[test]
    fn colour_attributes_decode_palette_indices_intensity_and_blink() {
        let cases = [
            (0x00, 0, 0, false),
            (0x01, 1, 0, false),
            (0x07, 7, 0, false),
            (0x1F, 15, 1, false),
            (0x70, 0, 7, false),
            (0x87, 7, 0, true),
            (0x8F, 15, 0, true),
            (0xFE, 14, 7, true),
        ];
        let vram: Vec<u8> = cases.iter().flat_map(|&(attr, ..)| [b'X', attr]).collect();
        for blink_enabled in [true, false] {
            let grid = render_text_mode(&TextModeState {
                vram: &vram,
                cols: cases.len() as u16,
                rows: 1,
                start_cell: 0,
                cursor_cell: 0,
                cursor_visible: false,
                attributes: TextAttributeSet::Colour { blink_enabled },
            });
            for (col, &(attr, foreground, background, blink)) in cases.iter().enumerate() {
                let cell = grid.cell(col as u16, 0).unwrap();
                assert_eq!((cell.glyph, cell.attr), (b'X', attr));
                assert_eq!(
                    cell.style,
                    TextStyle::Colour {
                        foreground,
                        background: background | if !blink_enabled && blink { 8 } else { 0 },
                        blink: blink_enabled && blink,
                    },
                    "attr={attr:#04x}, blink_enabled={blink_enabled}"
                );
            }
            assert_eq!(grid.to_dump(), "X".repeat(cases.len()));
        }
    }

    #[test]
    fn cursor_compares_full_sixteen_bit_address() {
        let vram = vec![0; 0x8000];
        for cursor_cell in [0x4000, 0x8000, 0xC000, 0xFFFF] {
            let grid = render_text_mode(&TextModeState {
                vram: &vram,
                cols: 80,
                rows: 25,
                start_cell: 0,
                cursor_cell,
                cursor_visible: true,
                attributes: TextAttributeSet::Colour {
                    blink_enabled: true,
                },
            });
            assert!(!grid.cursor().visible, "cursor={cursor_cell:#x}");
        }
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: 3,
            rows: 1,
            start_cell: 0xFFFF,
            cursor_cell: 0,
            cursor_visible: true,
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
        });
        assert_eq!(
            grid.cursor(),
            Cursor {
                col: 1,
                row: 0,
                visible: true
            }
        );
    }

    #[test]
    fn fidelity_scanout_wrap_does_not_wrap_cursor_comparison() {
        let grid = render_text_mode(&TextModeState {
            vram: &[b'A', 7, b'B', 7, b'C', 7, b'D', 7],
            cols: 3,
            rows: 1,
            start_cell: 3,
            cursor_cell: 0,
            cursor_visible: true,
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
        });
        assert_eq!(grid.to_dump(), "DAB");
        assert_eq!(
            grid.cursor(),
            Cursor {
                col: 0,
                row: 0,
                visible: false
            }
        );
    }

    #[test]
    fn fidelity_zero_width_frame_is_empty_and_has_no_cursor() {
        let grid = render_text_mode(&TextModeState {
            vram: &[],
            cols: 0,
            rows: 25,
            start_cell: 0,
            cursor_cell: 0,
            cursor_visible: true,
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
        });
        assert!(grid.cells.is_empty());
        assert!(!grid.cursor().visible);
    }

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
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
        });
        assert_eq!(
            grid.cell(0, 0),
            Some(&TextCell {
                glyph: b'A',
                attr: 0x07,
                style: TextStyle::Colour {
                    foreground: 7,
                    background: 0,
                    blink: false
                },
            })
        );
        assert_eq!(
            grid.cell(1, 0),
            Some(&TextCell {
                glyph: b'B',
                attr: 0x70,
                style: TextStyle::Colour {
                    foreground: 0,
                    background: 7,
                    blink: false
                },
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
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
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
    fn cursor_before_start_stays_outside_grid() {
        let vram = vec![0u8; 8];
        let grid = render_text_mode(&TextModeState {
            vram: &vram,
            cols: 2,
            rows: 2,
            start_cell: 2,
            cursor_cell: 0,
            cursor_visible: true,
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
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
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
        });
        assert_eq!(
            grid.cell(0, 0),
            Some(&TextCell {
                glyph: 0,
                attr: 0,
                style: TextStyle::Colour {
                    foreground: 0,
                    background: 0,
                    blink: false
                }
            })
        );
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
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
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
            attributes: TextAttributeSet::Colour {
                blink_enabled: true,
            },
        });
        sink.present(&Frame::Text(grid.clone()));
        assert_eq!(sink.last_text(), Some(&grid));
    }
}
