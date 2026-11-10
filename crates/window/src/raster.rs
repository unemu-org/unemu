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
        let cell_width = u32::from(font.width()) * scale;
        let cell_height = u32::from(font.height()) * scale;
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
                let (fg, bg, underline) = colours(cell.style);
                let cursor_here = cursor.visible && cursor.col == col && cursor.row == row;
                for (y, &bits) in font.glyph_rows(cell.glyph).iter().enumerate() {
                    let cursor_start = if mono { 11 } else { 13 };
                    let line = (underline && y == 12)
                        || (cursor_here && (cursor_start..=cursor_start + 1).contains(&y));
                    for x in 0..font.width() {
                        let lit = line || bits & (1 << (font.width() - 1 - x)) != 0;
                        let pixel = if lit { fg } else { bg };
                        let px = u32::from(col) * cell_width + u32::from(x) * scale;
                        let py = u32::from(row) * cell_height + y as u32 * scale;
                        for dy in 0..scale {
                            let start = ((py + dy) * width + px) as usize;
                            image.pixels[start..start + scale as usize].fill(pixel);
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
