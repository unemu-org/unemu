// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fixed-width CP437 from Windows NE RT_FONT resources.
//!
//! Microsoft Windows 3.1 Programmer's Reference, Volume 4, Chapter 4 (Font
//! File Format), and Windows Developer's Notes: Font-File Format (KB65123)
//! describe FNT v2/v3 column-major glyphs. Only embedded monochrome fixed-pitch
//! rasters with all 256 CP437 entries are accepted; vector, proportional, ABC
//! spacing and colour fonts are unsupported. See design/spec.md §9.4.

use std::{error::Error, fmt};

/// Unmodified Ultimate Oldschool PC Font Pack v2.2 MDA font (CC BY-SA 4.0).
pub const MDA_FONT: &[u8] = include_bytes!("../assets/fonts/Bm437_IBM_MDA.FON");
/// Unmodified Ultimate Oldschool PC Font Pack v2.2 VGA font (CC BY-SA 4.0).
pub const VGA_FONT: &[u8] = include_bytes!("../assets/fonts/Bm437_IBM_VGA_8x16.FON");

/// Invalid or unsupported input at the font-file boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontError {
    /// A header, resource or bitmap extends beyond its containing bytes.
    Truncated,
    /// The MZ/NE signatures or resource table are invalid or lack RT_FONT.
    InvalidContainer,
    /// The font is not a supported fixed-width CP437 monochrome raster.
    UnsupportedFont,
    /// Glyph widths, offsets or the declared file size are inconsistent.
    InvalidFont,
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "font error: {self:?}")
    }
}
impl Error for FontError {}

fn bytes(data: &[u8], offset: usize, len: usize) -> Result<&[u8], FontError> {
    data.get(offset..offset.checked_add(len).ok_or(FontError::Truncated)?)
        .ok_or(FontError::Truncated)
}
fn word(data: &[u8], offset: usize) -> Result<u16, FontError> {
    let b = bytes(data, offset, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
fn dword(data: &[u8], offset: usize) -> Result<usize, FontError> {
    let b = bytes(data, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// An atlas of 256 CP437 glyphs, each row packed MSB-left into `width` bits.
#[derive(Debug)]
pub struct FontAtlas {
    width: u16,
    height: u16,
    rows: Vec<u16>,
}

impl FontAtlas {
    /// Parse the first RT_FONT resource from an MZ + NE Windows .FON file.
    /// Resource and bitmap extents are checked before indexing or allocating.
    pub fn from_fon(data: &[u8]) -> Result<Self, FontError> {
        if bytes(data, 0, 2)? != b"MZ" {
            return Err(FontError::InvalidContainer);
        }
        let ne = dword(data, 0x3C)?;
        let header = bytes(data, ne, 0x28)?;
        if &header[..2] != b"NE" {
            return Err(FontError::InvalidContainer);
        }
        let start = usize::from(word(header, 0x24)?);
        let end = usize::from(word(header, 0x26)?);
        let table = bytes(data, ne, end)?
            .get(start..end)
            .ok_or(FontError::InvalidContainer)?;
        let shift = word(table, 0)?;
        if shift > 16 {
            return Err(FontError::InvalidContainer);
        }
        let mut cursor = 2;
        let mut font = None;
        loop {
            let kind = word(table, cursor)?;
            if kind == 0 {
                break;
            }
            let count = usize::from(word(table, cursor + 2)?);
            let records = bytes(table, cursor + 8, count * 12)?;
            let (entries, _) = records.as_chunks::<12>();
            for record in entries {
                let offset = usize::from(word(record, 0)?) << shift;
                let len = usize::from(word(record, 2)?) << shift;
                let resource = bytes(data, offset, len)?;
                if kind == 0x8008 && font.is_none() {
                    font = Some(Self::from_fnt(resource)?);
                }
            }
            cursor += 8 + records.len();
        }
        font.ok_or(FontError::InvalidContainer)
    }

    fn from_fnt(resource: &[u8]) -> Result<Self, FontError> {
        let (table, stride) = match word(resource, 0)? {
            0x0200 => (118, 4),
            0x0300 => (148, 6),
            _ => return Err(FontError::UnsupportedFont),
        };
        let data = bytes(resource, 0, dword(resource, 2)?)?;
        let width = word(data, 86)?;
        let height = word(data, 88)?;
        // Small bounds keep malformed headers from requesting unbounded atlases.
        if word(data, 66)? != 0
            || !(1..=16).contains(&width)
            || !(1..=32).contains(&height)
            || bytes(data, 95, 2)? != [0, 255]
            || bytes(data, 90, 1)?[0] & 1 != 0
        {
            return Err(FontError::UnsupportedFont);
        }
        if table == 148 && dword(data, 118)? & !0x11 != 0 {
            return Err(FontError::UnsupportedFont);
        }
        let entries = bytes(data, table, 257 * stride)?;
        let bits = dword(data, 113)?;
        if bits < table + entries.len() || bits > data.len() {
            return Err(FontError::InvalidFont);
        }
        let height_usize = usize::from(height);
        let glyph_size = usize::from(width).div_ceil(8) * height_usize;
        let mut rows = vec![0; 256 * height_usize];
        for (glyph, entry) in entries.chunks_exact(stride).take(256).enumerate() {
            if word(entry, 0)? != width {
                return Err(FontError::InvalidFont);
            }
            let offset = if stride == 4 {
                usize::from(word(entry, 2)?)
            } else {
                dword(entry, 2)?
            };
            if offset < bits {
                return Err(FontError::InvalidFont);
            }
            let bitmap = bytes(data, offset, glyph_size)?;
            for y in 0..height_usize {
                for x in 0..usize::from(width) {
                    let bit = (bitmap[(x / 8) * height_usize + y] >> (7 - x % 8)) & 1;
                    rows[glyph * height_usize + y] |=
                        u16::from(bit) << (usize::from(width) - 1 - x);
                }
            }
        }
        Ok(Self {
            width,
            height,
            rows,
        })
    }

    /// Fixed cell width in pixels.
    pub fn width(&self) -> u16 {
        self.width
    }
    /// Fixed cell height in pixels.
    pub fn height(&self) -> u16 {
        self.height
    }
    /// First CP437 code point represented in the atlas.
    pub fn first_char(&self) -> u8 {
        0
    }
    /// Last CP437 code point represented in the atlas.
    pub fn last_char(&self) -> u8 {
        255
    }
    /// Top-to-bottom scanlines; bit `width - 1` is the leftmost pixel.
    pub fn glyph_rows(&self, glyph: u8) -> &[u16] {
        let start = usize::from(glyph) * usize::from(self.height);
        &self.rows[start..start + usize::from(self.height)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_truncation_and_single_byte_mutation_is_panic_free() {
        for original in [MDA_FONT, VGA_FONT] {
            for len in 0..original.len() {
                // VERSION ends at EOF; checking all resources rejects every truncation.
                assert!(FontAtlas::from_fon(&original[..len]).is_err(), "len={len}");
            }
            for offset in 0..original.len() {
                let mut data = original.to_vec();
                data[offset] ^= 0xFF;
                let _ = FontAtlas::from_fon(&data);
            }
        }
    }

    #[test]
    fn bad_headers_tables_and_bitmap_offsets_return_errors() {
        assert_eq!(FontAtlas::from_fon(b"").unwrap_err(), FontError::Truncated);
        for (offset, replacement) in [
            (0, vec![0, 0]),
            (0x3C, u32::MAX.to_le_bytes().to_vec()),
            (128, vec![0, 0]),
            (192, vec![0xFF, 0xFF]),
            (202, vec![0xFF, 0xFF]),
            (480, vec![0, 1]),
            (482, vec![0, 0, 0, 0]),
            (480 + 95, vec![1]),
            (480 + 118, vec![0, 0]),
            (480 + 120, vec![0xFF, 0xFF]),
        ] {
            let mut data = MDA_FONT.to_vec();
            data[offset..offset + replacement.len()].copy_from_slice(&replacement);
            assert!(FontAtlas::from_fon(&data).is_err(), "offset={offset}");
        }
    }

    #[test]
    fn ne_resource_offsets_honour_a_nondefault_alignment_shift() {
        let fnt = bytes(MDA_FONT, 480, dword(MDA_FONT, 482).unwrap()).unwrap();
        let resource_len = fnt.len().next_multiple_of(2);
        let mut container = vec![0; 256 + resource_len];
        container[..2].copy_from_slice(b"MZ");
        container[0x3C..0x40].copy_from_slice(&64u32.to_le_bytes());
        container[64..66].copy_from_slice(b"NE");
        container[100..102].copy_from_slice(&64u16.to_le_bytes());
        container[102..104].copy_from_slice(&88u16.to_le_bytes());
        container[128..130].copy_from_slice(&1u16.to_le_bytes());
        container[130..132].copy_from_slice(&0x8008u16.to_le_bytes());
        container[132..134].copy_from_slice(&1u16.to_le_bytes());
        container[138..140].copy_from_slice(&128u16.to_le_bytes());
        container[140..142].copy_from_slice(&((resource_len / 2) as u16).to_le_bytes());
        container[256..256 + fnt.len()].copy_from_slice(fnt);
        let atlas = FontAtlas::from_fon(&container).unwrap();
        assert_eq!((atlas.width(), atlas.height()), (9, 14));
        assert_eq!(
            atlas.glyph_rows(0xC4),
            &[0, 0, 0, 0, 0, 0, 0, 511, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn v3_uses_wide_offsets_and_column_major_rows() {
        let bits = 0x1_0000;
        let size = bits + 256 * 4;
        let mut fnt = vec![0; size];
        fnt[0..2].copy_from_slice(&0x0300u16.to_le_bytes());
        fnt[2..6].copy_from_slice(&(size as u32).to_le_bytes());
        fnt[86..88].copy_from_slice(&9u16.to_le_bytes());
        fnt[88..90].copy_from_slice(&2u16.to_le_bytes());
        fnt[96] = 255;
        fnt[113..117].copy_from_slice(&(bits as u32).to_le_bytes());
        fnt[118] = 1;
        for glyph in 0..256 {
            let entry = 148 + glyph * 6;
            fnt[entry..entry + 2].copy_from_slice(&9u16.to_le_bytes());
            fnt[entry + 2..entry + 6].copy_from_slice(&((bits + glyph * 4) as u32).to_le_bytes());
            fnt[bits + glyph * 4..bits + glyph * 4 + 4].copy_from_slice(&[0x80, 0x01, 0x80, 0]);
        }
        let atlas = FontAtlas::from_fnt(&fnt).unwrap();
        assert_eq!(atlas.glyph_rows(b'A'), &[0x101, 2]);
        fnt[150..154].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(FontAtlas::from_fnt(&fnt).unwrap_err(), FontError::Truncated);
    }
}
