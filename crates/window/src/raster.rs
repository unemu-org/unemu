// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure text-cell rasterisation (design/spec.md §9.4).
//!
//! IBM PC Technical Reference, Monochrome Display and Printer Adapter,
//! Programming Considerations, defines MDA attribute classes; IBM VGA Technical
//! Reference, Alphanumeric Modes, defines foreground/background palette indices.
//! This presenter consumes decoded styles, never raw card registers.

use crate::{FontAtlas, FontError, MDA_FONT, VGA_FONT};
use std::{error::Error, fmt, num::NonZeroU16};
use video::{MonochromeEffect, TextGrid, TextStyle};

const PALETTE: [u32; 16] = [
    0x000000, 0x0000AA, 0x00AA00, 0x00AAAA, 0xAA0000, 0xAA00AA, 0xAA5500, 0xAAAAAA, 0x555555,
    0x5555FF, 0x55FF55, 0x55FFFF, 0xFF5555, 0xFF55FF, 0xFFFF55, 0xFFFFFF,
];

/// A complete row-major softbuffer-compatible image (`0x00RRGGBB`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PixelFrame {
    /// Width in pixels, including integer scale.
    pub width: u32,
    /// Height in pixels, including integer scale.
    pub height: u32,
    /// Exactly `width * height` pixels.
    pub pixels: Vec<u32>,
}

/// A frame that cannot fit the presenter's bounded pixel buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterError {
    /// Frame exceeds 16,777,216 pixels (64 MiB) or dimensions overflow.
    TooLarge,
}
impl fmt::Display for RasterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("text frame exceeds the 64 MiB pixel buffer limit")
    }
}
impl Error for RasterError {}

/// CP437 rasteriser with bundled monochrome and colour font atlases.
pub struct Rasterizer {
    mda: FontAtlas,
    vga: FontAtlas,
    scale: NonZeroU16,
}

impl Rasterizer {
    /// Load the original MDA 9×14 and VGA 8×16 fonts once.
    pub fn bundled(scale: NonZeroU16) -> Result<Self, FontError> {
        Ok(Self {
            mda: FontAtlas::from_fon(MDA_FONT)?,
            vga: FontAtlas::from_fon(VGA_FONT)?,
            scale,
        })
    }

    /// Rasterise a grid without a window or host time. One card supplies one
    /// attribute set; the first cell selects its font (empty grids use MDA).
    /// Blink stays visible. Integer scaling replicates pixels without filtering.
    pub fn rasterize(&self, grid: &TextGrid) -> Result<PixelFrame, RasterError> {
        let mono = grid
            .cell(0, 0)
            .is_none_or(|cell| matches!(cell.style, TextStyle::Monochrome { .. }));
        let font = if mono { &self.mda } else { &self.vga };
        let scale = u32::from(self.scale.get());
        let appearance = grid.appearance();
        let dot_width = appearance.map_or(font.width(), |a| u16::from(a.cell_width));
        let dot_height = appearance.map_or(font.height(), |a| u16::from(a.cell_height));
        let horizontal_scale = scale * appearance.map_or(1, |a| u32::from(a.horizontal_repeat));
        let cell_width = u32::from(dot_width) * horizontal_scale;
        let cell_height = u32::from(dot_height) * scale;
        let width = u32::from(grid.cols())
            .checked_mul(cell_width)
            .ok_or(RasterError::TooLarge)?;
        let height = u32::from(grid.rows())
            .checked_mul(cell_height)
            .ok_or(RasterError::TooLarge)?;
        let len = u64::from(width) * u64::from(height);
        if len > 16_777_216 {
            return Err(RasterError::TooLarge);
        }
        let mut image = PixelFrame {
            width,
            height,
            pixels: vec![0; len as usize],
        };
        let cursor = grid.cursor();
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let Some(cell) = grid.cell(col, row) else {
                    continue;
                };
                let (mut fg, mut bg, mut underline) = colours(cell.style);
                if let Some(a) = appearance {
                    if let TextStyle::Colour {
                        foreground,
                        background,
                        ..
                    } = cell.style
                    {
                        let rgb = |index: u8| {
                            let [r, g, b] = a.palette[usize::from(index)];
                            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
                        };
                        fg = rgb(foreground);
                        bg = rgb(background);
                        underline = cell.attr & 0x77 == 1;
                    }
                    if !a.enabled {
                        fg = 0;
                        bg = 0;
                    }
                }
                let cursor_here = cursor.visible && cursor.col == col && cursor.row == row;
                for y in 0..dot_height {
                    let bits = grid
                        .glyph_row(cell, y as u8)
                        .unwrap_or_else(|| font.glyph_rows(cell.glyph)[usize::from(y)]);
                    let cursor_start = appearance
                        .map_or(if mono { 11 } else { 13 }, |a| u16::from(a.cursor_start));
                    let cursor_end =
                        appearance.map_or(cursor_start + 1, |a| u16::from(a.cursor_end));
                    let underline_row = appearance.map_or(12, |a| u16::from(a.underline));
                    let line = (underline && y == underline_row)
                        || (cursor_here && (cursor_start..=cursor_end).contains(&y));
                    for x in 0..dot_width {
                        let lit = line || bits & (1 << (dot_width - 1 - x)) != 0;
                        let pixel = if lit { fg } else { bg };
                        let px = u32::from(col) * cell_width + u32::from(x) * horizontal_scale;
                        let py = u32::from(row) * cell_height + y as u32 * scale;
                        for dy in 0..scale {
                            let start = ((py + dy) * width + px) as usize;
                            image.pixels[start..start + horizontal_scale as usize].fill(pixel);
                        }
                    }
                }
            }
        }
        Ok(image)
    }
}

fn colours(style: TextStyle) -> (u32, u32, bool) {
    match style {
        TextStyle::Colour {
            foreground,
            background,
            ..
        } => (
            PALETTE[usize::from(foreground & 15)],
            PALETTE[usize::from(background & 15)],
            false,
        ),
        TextStyle::Monochrome {
            effect,
            intense,
            background_intense,
            ..
        } => {
            let ink = if intense { 0xFFFFFF } else { 0xAAAAAA };
            let paper = if background_intense { 0x555555 } else { 0 };
            match effect {
                MonochromeEffect::NonDisplay => (0, 0, false),
                MonochromeEffect::Normal => (ink, paper, false),
                MonochromeEffect::Underline => (ink, paper, true),
                MonochromeEffect::Reverse => (
                    0,
                    if background_intense {
                        0xFFFFFF
                    } else {
                        0xAAAAAA
                    },
                    false,
                ),
            }
        }
    }
}

#[cfg(test)]
mod planar_tests {
    use super::*;
    use video::{render_planar_text, PlanarTextState};

    #[test]
    fn guest_font_palette_ninth_dot_cursor_underline_and_blank_are_presented() {
        let glyphs = vec![0xC0, b'A'];
        let attributes = vec![2, 11];
        let mut font = vec![0; 65536];
        font[0xC0 * 32] = 1;
        font[0x2000 + usize::from(b'A') * 32] = 0x80;
        let mut palette = [[0; 3]; 16];
        palette[1] = [0, 255, 0];
        palette[2] = [255, 0, 0];
        palette[3] = [0, 0, 255];
        palette[11] = [0, 255, 0];
        let mut state = PlanarTextState {
            planes: [&glyphs, &attributes, &font],
            dimensions: (2, 1),
            start: 0,
            cursor: 1,
            row_stride: 2,
            address_shift: 0,
            word_wrap_bit: 15,
            font_offsets: [0, 0x2000],
            dual_font: true,
            cell_size: (9, 2),
            horizontal_repeat: 1,
            line_graphics: true,
            blink_enabled: true,
            cursor_shape: (true, 1, 1),
            underline: 1,
            palette,
            enabled: true,
        };
        let raster = Rasterizer::bundled(NonZeroU16::new(1).unwrap()).unwrap();
        let pixels = raster.rasterize(&render_planar_text(&state)).unwrap();
        let mut expected = vec![0; 36];
        expected[7..9].fill(0xFF0000);
        expected[9] = 0x00FF00;
        expected[27..36].fill(0x00FF00);
        assert_eq!(
            pixels,
            PixelFrame {
                width: 18,
                height: 2,
                pixels: expected
            }
        );
        state.enabled = false;
        assert_eq!(
            raster
                .rasterize(&render_planar_text(&state))
                .unwrap()
                .pixels,
            vec![0; 36]
        );
        let underlined_attributes = vec![1, 11];
        state.planes[1] = &underlined_attributes;
        state.enabled = true;
        state.cursor_shape.0 = false;
        let pixels = raster.rasterize(&render_planar_text(&state)).unwrap();
        assert_eq!(&pixels.pixels[18..27], &[0x00FF00; 9]);
        assert_eq!(&pixels.pixels[27..], &[0; 9]);
    }

    #[test]
    fn divided_dot_clock_doubles_each_dot_for_forty_columns() {
        let glyphs = vec![b'A'; 40];
        let attributes = vec![2; 40];
        let mut font = vec![0; 65536];
        font[usize::from(b'A') * 32] = 0x81;
        let mut palette = [[0; 3]; 16];
        palette[2] = [255, 0, 0];
        let state = PlanarTextState {
            planes: [&glyphs, &attributes, &font],
            dimensions: (40, 1),
            start: 0,
            cursor: 0,
            row_stride: 40,
            address_shift: 0,
            word_wrap_bit: 15,
            font_offsets: [0, 0],
            dual_font: false,
            cell_size: (9, 2),
            horizontal_repeat: 2,
            line_graphics: false,
            blink_enabled: true,
            cursor_shape: (false, 0, 0),
            underline: 31,
            palette,
            enabled: true,
        };
        let raster = Rasterizer::bundled(NonZeroU16::new(1).unwrap()).unwrap();
        let pixels = raster.rasterize(&render_planar_text(&state)).unwrap();
        let mut expected_row = [0; 18];
        expected_row[0..2].fill(0xFF0000);
        expected_row[14..16].fill(0xFF0000);
        let mut expected = expected_row.repeat(40);
        expected.extend(vec![0; 720]);
        assert_eq!(
            pixels,
            PixelFrame {
                width: 720,
                height: 2,
                pixels: expected
            }
        );
    }
}
