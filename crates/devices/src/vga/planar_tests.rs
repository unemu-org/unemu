// SPDX-License-Identifier: AGPL-3.0-or-later
//! Planar text isolation tests. Register tables: SeaVGABIOS stdvgamodes.c,
//! mode 03h, sequ_03/crtc_03/actl_01/grdc_01 (80×25, 9×16).
use super::Vga;
use testkit::MockBus;
use video::{render_planar_text, Cursor, TextAppearance, TextCell, TextGrid, TextStyle};

fn out(b: &mut MockBus, port: u16, value: u8) {
    b.port_write(port, 1, u32::from(value)).unwrap();
}
fn reg(b: &mut MockBus, port: u16, index: u8, value: u8) {
    b.port_write(port, 2, u32::from(index) | (u32::from(value) << 8))
        .unwrap();
}
fn ac(b: &mut MockBus, index: u8, value: u8) {
    b.port_read(0x3DA, 1).unwrap();
    out(b, 0x3C0, index);
    out(b, 0x3C0, value);
    b.port_read(0x3DA, 1).unwrap();
    out(b, 0x3C0, 0x20);
}
fn mode3() -> MockBus {
    let mut b = MockBus::new(Box::new(Vga::new()));
    out(&mut b, 0x3C2, 0x67);
    reg(&mut b, 0x3C4, 0, 3);
    for (port, start, values) in [
        (0x3C4, 1, &[0x00, 0x03, 0x00, 0x02][..]),
        (
            0x3D4,
            0,
            &[
                0x5f, 0x4f, 0x50, 0x82, 0x55, 0x81, 0xbf, 0x1f, 0x00, 0x4f, 0x0d, 0x0e, 0x00, 0x00,
                0x00, 0x00, 0x9c, 0x8e, 0x8f, 0x28, 0x1f, 0x96, 0xb9, 0xa3, 0xff,
            ][..],
        ),
        (0x3CE, 0, &[0, 0, 0, 0, 0, 0x10, 0x0e, 0x0f, 0xff][..]),
    ] {
        for (i, &value) in values.iter().enumerate() {
            reg(&mut b, port, start + i as u8, value);
        }
    }
    for (i, value) in [
        0, 1, 2, 3, 4, 5, 0x14, 7, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x0c, 0, 0x0f, 8,
    ]
    .into_iter()
    .enumerate()
    {
        ac(&mut b, i as u8, value);
    }
    out(&mut b, 0x3C6, 0xff);
    b
}
fn text(b: &mut MockBus, cell: u64, glyph: u8, attr: u8) {
    b.mmio_write(
        0xB8000 + cell * 2,
        2,
        u64::from(glyph) | (u64::from(attr) << 8),
    )
    .unwrap();
}
fn font(b: &mut MockBus, offset: u64, glyph: u8, rows: &[u8]) {
    reg(b, 0x3C4, 2, 4);
    reg(b, 0x3C4, 4, 6);
    reg(b, 0x3CE, 5, 0);
    reg(b, 0x3CE, 6, 4);
    for (row, &bits) in rows.iter().enumerate() {
        b.mmio_write(
            0xA0000 + offset + u64::from(glyph) * 32 + row as u64,
            1,
            u64::from(bits),
        )
        .unwrap();
    }
    reg(b, 0x3C4, 2, 3);
    reg(b, 0x3C4, 4, 2);
    reg(b, 0x3CE, 5, 0x10);
    reg(b, 0x3CE, 6, 0x0e);
}
fn render(b: &MockBus) -> TextGrid {
    render_planar_text(&b.device().planar_text_scanout().unwrap())
}
fn colour(b: &mut MockBus, index: u8, rgb: [u8; 3]) {
    out(b, 0x3C8, index);
    for v in rgb {
        out(b, 0x3C9, v);
    }
}

#[test]
fn planar_mode_three_exact_frame_and_passive_observation() {
    let mut b = mode3();
    font(&mut b, 0, b'A', &[0x81, 0x42, 0x24, 0x18]);
    text(&mut b, 0, b'A', 0x87);
    reg(&mut b, 0x3D4, 0x0F, 1);
    let before = b.device().save();
    let grid = render(&b);
    assert_eq!(b.device().save(), before);
    assert_eq!((grid.cols(), grid.rows()), (80, 25));
    let appearance = grid.appearance().unwrap();
    let mut expected_font = vec![0; 8192];
    expected_font[usize::from(b'A') * 32..usize::from(b'A') * 32 + 4]
        .copy_from_slice(&[0x102, 0x84, 0x48, 0x30]);
    assert_eq!(
        appearance,
        &TextAppearance {
            cell_width: 9,
            horizontal_repeat: 1,
            cell_height: 16,
            fonts: [expected_font.clone(), expected_font],
            dual_font: false,
            palette: [[0; 3]; 16],
            cursor_start: 13,
            cursor_end: 14,
            underline: 31,
            enabled: true,
        }
    );
    let mut cells = vec![
        TextCell {
            glyph: 0,
            attr: 0,
            style: TextStyle::Colour {
                foreground: 0,
                background: 0,
                blink: false
            }
        };
        2000
    ];
    cells[0] = TextCell {
        glyph: b'A',
        attr: 0x87,
        style: TextStyle::Colour {
            foreground: 7,
            background: 0,
            blink: true,
        },
    };
    let actual_cells: Vec<_> = (0..grid.rows())
        .flat_map(|row| {
            let grid = &grid;
            (0..grid.cols()).map(move |col| *grid.cell(col, row).unwrap())
        })
        .collect();
    assert_eq!(actual_cells, cells);
    assert_eq!(
        grid.cursor(),
        Cursor {
            col: 1,
            row: 0,
            visible: true
        }
    );
    assert_eq!(render(&b), grid);
    font(&mut b, 0, b'A', &[0]);
    assert_eq!(grid.glyph_row(grid.cell(0, 0).unwrap(), 0), Some(0x102));
    assert_eq!(render(&b).glyph_row(grid.cell(0, 0).unwrap(), 0), Some(0));
}

#[test]
fn planar_start_stride_geometry_and_cursor_are_crtc_driven() {
    let mut b = mode3();
    text(&mut b, 80, b'S', 7);
    text(&mut b, 160, b'T', 7);
    text(&mut b, 240, b'U', 7);
    reg(&mut b, 0x3D4, 0x0D, 80);
    b.tick(40_000);
    reg(&mut b, 0x3D4, 0x0F, 160);
    let grid = render(&b);
    assert_eq!(grid.cell(0, 0).unwrap().glyph, b'S');
    assert_eq!(grid.cell(0, 1).unwrap().glyph, b'T');
    assert_eq!(
        grid.cursor(),
        Cursor {
            col: 0,
            row: 1,
            visible: true
        }
    );
    reg(&mut b, 0x3D4, 0x13, 80);
    assert_eq!(render(&b).cell(0, 1).unwrap().glyph, b'U');
    reg(&mut b, 0x3D4, 0x0F, 161);
    assert!(!render(&b).cursor().visible);
    reg(&mut b, 0x3D4, 0x0F, 80);
    reg(&mut b, 0x3D4, 0x0A, 0x2d);
    assert!(!render(&b).cursor().visible);
    reg(&mut b, 0x3D4, 0x0A, 15);
    assert!(!render(&b).cursor().visible);
    reg(&mut b, 0x3D4, 0x11, 0x0e);
    reg(&mut b, 0x3D4, 1, 39);
    reg(&mut b, 0x3D4, 9, 7);
    assert_eq!((render(&b).cols(), render(&b).rows()), (40, 50));
    reg(&mut b, 0x3D4, 9, 0x87);
    assert_eq!(render(&b).rows(), 25);
    reg(&mut b, 0x3D4, 7, 0x5d);
    assert_eq!(render(&b).rows(), 41);
}

#[test]
fn planar_ninth_dot_blink_intensity_and_font_selection() {
    let mut b = mode3();
    for glyph in [0xBF, 0xC0, 0xDF, 0xE0] {
        font(&mut b, 0, glyph, &[1]);
        text(&mut b, u64::from(glyph - 0xBF), glyph, 0x8f);
    }
    let grid = render(&b);
    for (col, bits) in [(0, 2), (1, 3), (32, 3), (33, 2)] {
        assert_eq!(grid.glyph_row(grid.cell(col, 0).unwrap(), 0), Some(bits));
    }
    ac(&mut b, 0x10, 0);
    let grid = render(&b);
    assert_eq!(grid.glyph_row(grid.cell(1, 0).unwrap(), 0), Some(2));
    assert_eq!(
        grid.cell(1, 0).unwrap().style,
        TextStyle::Colour {
            foreground: 15,
            background: 8,
            blink: false
        }
    );
    reg(&mut b, 0x3C4, 1, 1);
    assert_eq!(render(&b).glyph_row(grid.cell(1, 0).unwrap(), 0), Some(1));
    font(&mut b, 0x2000, 0xC0, &[0x80]);
    reg(&mut b, 0x3C4, 3, 0x20);
    let grid = render(&b);
    assert_eq!(grid.glyph_row(grid.cell(1, 0).unwrap(), 0), Some(0x80));
    assert_eq!(
        grid.cell(1, 0).unwrap().style,
        TextStyle::Colour {
            foreground: 15,
            background: 8,
            blink: false
        }
    );
    text(&mut b, 1, 0xC0, 7);
    assert_eq!(
        render(&b).glyph_row(render(&b).cell(1, 0).unwrap(), 0),
        Some(1)
    );
    // The high select bits address alternating 8 KiB blocks, not linear banks.
    for (select, offset) in [
        (4, 0x4000),
        (8, 0x8000),
        (12, 0xC000),
        (0x24, 0x6000),
        (0x28, 0xA000),
        (0x2C, 0xE000),
    ] {
        font(&mut b, offset, 0xC0, &[select]);
        reg(&mut b, 0x3C4, 3, select);
        text(&mut b, 1, 0xC0, 15);
        assert_eq!(
            render(&b).glyph_row(render(&b).cell(1, 0).unwrap(), 0),
            Some(u16::from(select))
        );
    }
}

#[test]
fn planar_palette_plane_enable_color_select_p54s_and_pel_mask() {
    let mut b = mode3();
    text(&mut b, 0, b'A', 0x9f);
    ac(&mut b, 15, 0x2a);
    colour(&mut b, 0x2a, [1, 16, 63]);
    assert_eq!(render(&b).appearance().unwrap().palette[15], [4, 65, 255]);
    ac(&mut b, 0x12, 7);
    ac(&mut b, 7, 0x13);
    colour(&mut b, 0x13, [2, 3, 4]);
    assert_eq!(render(&b).appearance().unwrap().palette[15], [8, 12, 16]);
    ac(&mut b, 0x14, 0x0e);
    colour(&mut b, 0xd3, [8, 9, 10]);
    assert_eq!(render(&b).appearance().unwrap().palette[15], [32, 36, 40]);
    ac(&mut b, 0x10, 0x8c);
    colour(&mut b, 0xe3, [11, 12, 13]);
    assert_eq!(render(&b).appearance().unwrap().palette[15], [44, 48, 52]);
    out(&mut b, 0x3C6, 0x0f);
    colour(&mut b, 3, [14, 15, 16]);
    assert_eq!(render(&b).appearance().unwrap().palette[15], [56, 60, 65]);
}

fn plane_bytes(b: &mut MockBus, offset: u64, glyph: u8, attr: u8) {
    reg(b, 0x3C4, 4, 6);
    reg(b, 0x3CE, 5, 0);
    reg(b, 0x3CE, 6, 4);
    for (mask, value) in [(1, glyph), (2, attr)] {
        reg(b, 0x3C4, 2, mask);
        b.mmio_write(0xA0000 + offset, 1, u64::from(value)).unwrap();
    }
}

#[test]
fn planar_doubleword_routes_counter_bits_twelve_and_thirteen_to_low_bits() {
    let mut b = mode3();
    for (start, offset) in [(0x1000_u16, 0x4001), (0x2000, 0x8002), (0xFFFF, 0xFFFF)] {
        plane_bytes(&mut b, offset, b'D', 0x1e);
        reg(&mut b, 0x3D4, 0x14, 0x40);
        reg(&mut b, 0x3D4, 0x0C, (start >> 8) as u8);
        reg(&mut b, 0x3D4, 0x0D, start as u8);
        b.tick(40_000);
        let grid = render(&b);
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!((cell.glyph, cell.attr), (b'D', 0x1e), "start={start:#06x}");
    }
}

#[test]
fn planar_word_address_wrap_selects_counter_bit_thirteen_or_fifteen() {
    let mut b = mode3();
    plane_bytes(&mut b, 0x4000, b'F', 7);
    plane_bytes(&mut b, 0x4001, b'T', 7);
    reg(&mut b, 0x3D4, 0x0C, 0x20);
    b.tick(40_000);
    reg(&mut b, 0x3D4, 0x17, 0xa3);
    assert_eq!(render(&b).cell(0, 0).unwrap().glyph, b'F');
    reg(&mut b, 0x3D4, 0x17, 0x83);
    assert_eq!(render(&b).cell(0, 0).unwrap().glyph, b'T');
}

#[test]
fn planar_screen_off_blanks_scanout_without_losing_text() {
    let mut b = mode3();
    text(&mut b, 0, b'S', 7);
    assert!(render(&b).appearance().unwrap().enabled);
    reg(&mut b, 0x3C4, 1, 0x20);
    let grid = render(&b);
    assert!(!grid.appearance().unwrap().enabled);
    assert!(!grid.cursor().visible);
    assert_eq!(grid.cell(0, 0).unwrap().glyph, b'S');
    reg(&mut b, 0x3C4, 1, 0);
    assert!(render(&b).appearance().unwrap().enabled);
}

#[test]
fn planar_graphics_mode_disables_text_presentation() {
    let mut b = mode3();
    assert!(render(&b).appearance().unwrap().enabled);
    ac(&mut b, 0x10, 0x0d);
    assert!(!render(&b).appearance().unwrap().enabled);
    ac(&mut b, 0x10, 0x0c);
    assert!(render(&b).appearance().unwrap().enabled);
}

#[test]
fn planar_two_fonts_require_extended_memory() {
    let mut b = mode3();
    font(&mut b, 0, b'A', &[1]);
    font(&mut b, 0x2000, b'A', &[0x80]);
    reg(&mut b, 0x3C4, 3, 0x20);
    text(&mut b, 0, b'A', 0x0f);
    reg(&mut b, 0x3C4, 4, 0);
    let grid = render(&b);
    assert!(!grid.appearance().unwrap().dual_font);
    assert_eq!(grid.glyph_row(grid.cell(0, 0).unwrap(), 0), Some(2));
    reg(&mut b, 0x3C4, 4, 2);
    let grid = render(&b);
    assert!(grid.appearance().unwrap().dual_font);
    assert_eq!(grid.glyph_row(grid.cell(0, 0).unwrap(), 0), Some(0x100));
}

#[test]
fn planar_nonzero_map_b_uses_both_select_fields() {
    let mut b = mode3();
    text(&mut b, 0, b'B', 7);
    text(&mut b, 1, b'B', 15);
    font(&mut b, 0, b'B', &[0x80]);
    for (select, offset) in [
        (1, 0x4000),
        (2, 0x8000),
        (3, 0xC000),
        (0x10, 0x2000),
        (0x11, 0x6000),
        (0x12, 0xA000),
        (0x13, 0xE000),
    ] {
        font(&mut b, offset, b'B', &[select]);
        reg(&mut b, 0x3C4, 3, select);
        let grid = render(&b);
        assert_eq!(
            grid.glyph_row(grid.cell(0, 0).unwrap(), 0),
            Some(u16::from(select) << 1)
        );
        assert_eq!(grid.glyph_row(grid.cell(1, 0).unwrap(), 0), Some(0x100));
    }
}

#[test]
fn planar_two_fonts_keep_colour_bit_three_until_color_plane_enable() {
    let mut b = mode3();
    font(&mut b, 0, b'A', &[1]);
    font(&mut b, 0x2000, b'A', &[0x80]);
    reg(&mut b, 0x3C4, 3, 0x20);
    text(&mut b, 0, b'A', 0x0f);
    colour(&mut b, 0x3f, [63, 0, 0]);
    colour(&mut b, 7, [0, 63, 0]);
    let grid = render(&b);
    assert_eq!(
        grid.cell(0, 0).unwrap().style,
        TextStyle::Colour {
            foreground: 15,
            background: 0,
            blink: false,
        }
    );
    assert_eq!(grid.appearance().unwrap().palette[15], [255, 0, 0]);
    ac(&mut b, 0x12, 7);
    let grid = render(&b);
    assert_eq!(
        grid.cell(0, 0).unwrap().style,
        TextStyle::Colour {
            foreground: 15,
            background: 0,
            blink: false,
        }
    );
    assert_eq!(grid.appearance().unwrap().palette[15], [0, 255, 0]);
    assert_eq!(grid.glyph_row(grid.cell(0, 0).unwrap(), 0), Some(0x100));
}

#[test]
fn planar_forty_columns_preserve_the_divided_dot_clock() {
    // SeaVGABIOS stdvgamodes.c modes 00h/01h use sequ_01=08h and CR01=27h.
    let mut b = mode3();
    reg(&mut b, 0x3D4, 0x11, 0x0e);
    reg(&mut b, 0x3D4, 1, 0x27);
    reg(&mut b, 0x3C4, 1, 8);
    let grid = render(&b);
    let appearance = grid.appearance().unwrap();
    assert_eq!(grid.cols(), 40);
    assert_eq!(
        (appearance.cell_width, appearance.horizontal_repeat),
        (9, 2)
    );
    reg(&mut b, 0x3C4, 1, 0);
    assert_eq!(render(&b).appearance().unwrap().horizontal_repeat, 1);
}

#[test]
fn scrolling_text_nine_dot_panning_raster_has_exact_pixels_and_extra_cell() {
    for pan in 0..=8 {
        let mut b = mode3();
        colour(&mut b, 1, [63, 0, 0]);
        colour(&mut b, 2, [0, 63, 0]);
        reg(&mut b, 0x3D4, 0x0a, 0x20);
        font(&mut b, 0, b'A', &[0x81]);
        font(&mut b, 0, b'B', &[0x42]);
        text(&mut b, 0, b'A', 1);
        text(&mut b, 1, b'B', 2);
        text(&mut b, 80, b'B', 2);
        ac(&mut b, 0x13, pan);
        b.tick(40_000);
        let shift = if pan == 8 { 0 } else { pan + 1 };
        if shift == 0 {
            assert!(b.device().graphics_scanout().is_none());
            continue;
        }
        let f = video::render_graphics(&b.device().graphics_scanout().expect("panned text raster"));
        let pattern = [1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 0, 0, 2, 0, 0];
        assert_eq!(
            &f.indices[..9],
            &pattern[usize::from(shift)..usize::from(shift) + 9]
        );
        if shift > 1 {
            assert_eq!(f.indices[721 - usize::from(shift)], 2);
        }
        assert_eq!((f.width, f.height), (720, 400));
        assert_eq!(f.palette[1], [255, 0, 0]);
        assert_eq!(f.palette[2], [0, 255, 0]);
    }
}

#[test]
fn scrolling_text_preset_row_byte_pan_and_split_restart_font_and_address() {
    let mut b = mode3();
    reg(&mut b, 0x3D4, 0x0a, 0x20);
    font(&mut b, 0, b'A', &[0x80, 0x40, 0x20]);
    text(&mut b, 1, b'A', 1);
    text(&mut b, 81, b'A', 2);
    reg(&mut b, 0x3D4, 8, 0x21);
    reg(&mut b, 0x3D4, 0x0d, 80);
    reg(&mut b, 0x3D4, 0x11, 0x0e);
    reg(&mut b, 0x3D4, 7, 0x0f);
    reg(&mut b, 0x3D4, 9, 0x0f);
    reg(&mut b, 0x3D4, 0x18, 1);
    b.tick(40_000);
    let f = video::render_graphics(&b.device().graphics_scanout().expect("scrolled text raster"));
    assert_eq!(&f.indices[..4], &[0, 2, 0, 0]);
    assert_eq!(&f.indices[720..724], &[0, 0, 2, 0]);
    assert_eq!(&f.indices[1440..1444], &[1, 0, 0, 0]);
}
