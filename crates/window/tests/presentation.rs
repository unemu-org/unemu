// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless tests of the CP437 presenter (design/spec.md §9.4).

use std::num::NonZeroU16;
use video::{render_text_mode, Frame, FrameSink, TextAttributeSet, TextModeState};
use window::{FontAtlas, Rasterizer, WindowSink, MDA_FONT, VGA_FONT};

fn grid(bytes: &[u8], mono: bool, blink: bool, cursor: bool) -> video::TextGrid {
    render_text_mode(&TextModeState {
        vram: bytes,
        cols: (bytes.len() / 2) as u16,
        rows: 1,
        start_cell: 0,
        cursor_cell: 0,
        cursor_visible: cursor,
        attributes: if mono {
            TextAttributeSet::Monochrome {
                blink_enabled: blink,
            }
        } else {
            TextAttributeSet::Colour {
                blink_enabled: blink,
            }
        },
    })
}

#[test]
fn bundled_fonts_have_exact_cp437_rows() {
    for (data, width, height, a, line, block) in [
        (
            MDA_FONT,
            9,
            14,
            vec![0, 0, 16, 56, 108, 198, 198, 254, 198, 198, 198, 0, 0, 0],
            vec![0, 0, 0, 0, 0, 0, 0, 511, 0, 0, 0, 0, 0, 0],
            511,
        ),
        (
            VGA_FONT,
            8,
            16,
            vec![
                0, 0, 16, 56, 108, 198, 198, 254, 198, 198, 198, 198, 0, 0, 0, 0,
            ],
            vec![0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0],
            255,
        ),
    ] {
        let font = FontAtlas::from_fon(data).unwrap();
        assert_eq!((font.width(), font.height()), (width, height));
        assert_eq!((font.first_char(), font.last_char()), (0, 255));
        assert_eq!(font.glyph_rows(b'A'), a);
        assert_eq!(font.glyph_rows(0xC4), line);
        assert_eq!(font.glyph_rows(0xDB), vec![block; height as usize]);
    }
}

#[test]
fn monochrome_effects_intensity_underline_and_non_display() {
    let raster = Rasterizer::bundled(NonZeroU16::new(1).unwrap()).unwrap();
    let image = raster
        .rasterize(&grid(
            &[0xDB, 7, 0xDB, 15, b' ', 1, 0xDB, 0x70, 0xDB, 8],
            true,
            true,
            false,
        ))
        .unwrap();
    assert_eq!((image.width, image.height), (45, 14));
    for y in 0..14 {
        for x in 0..45 {
            let expected = match x / 9 {
                0 => 0xAAAAAA,
                1 => 0xFFFFFF,
                2 if y == 12 => 0xAAAAAA,
                _ => 0,
            };
            assert_eq!(image.pixels[y * 45 + x], expected, "({x}, {y})");
        }
    }
    let reverse = raster
        .rasterize(&grid(&[b' ', 0x70, b' ', 0xF8], true, false, false))
        .unwrap();
    for row in reverse.pixels.chunks_exact(18) {
        assert_eq!(&row[..9], &[0xAAAAAA; 9]);
        assert_eq!(&row[9..], &[0xFFFFFF; 9]);
    }
    let background = raster
        .rasterize(&grid(&[b' ', 0x87], true, false, false))
        .unwrap();
    assert!(background.pixels.iter().all(|&pixel| pixel == 0x555555));
}

#[test]
fn colour_palette_blink_cursor_and_integer_scaling() {
    let raster = Rasterizer::bundled(NonZeroU16::new(2).unwrap()).unwrap();
    let image = raster
        .rasterize(&grid(&[b' ', 0x1E, 0xDB, 0x9C], false, true, true))
        .unwrap();
    assert_eq!((image.width, image.height), (32, 32));
    for y in 0..32 {
        for x in 0..32 {
            let expected = if x >= 16 {
                0xFF5555
            } else if (26..30).contains(&y) {
                0xFFFF55
            } else {
                0x0000AA
            };
            assert_eq!(image.pixels[y * 32 + x], expected);
        }
    }
    let steady = Rasterizer::bundled(NonZeroU16::new(1).unwrap()).unwrap();
    assert_eq!(
        steady
            .rasterize(&grid(&[b'A', 0x87], false, true, false))
            .unwrap(),
        steady
            .rasterize(&grid(&[b'A', 7], false, true, false))
            .unwrap()
    );
    let bright_bg = steady
        .rasterize(&grid(&[b' ', 0xF0], false, false, false))
        .unwrap();
    assert!(bright_bg.pixels.iter().all(|&pixel| pixel == 0xFFFFFF));
    let mono_cursor = steady
        .rasterize(&grid(&[b' ', 7], true, true, true))
        .unwrap();
    for y in 0..14 {
        let expected = if (11..=12).contains(&y) { 0xAAAAAA } else { 0 };
        assert!(mono_cursor.pixels[y * 9..(y + 1) * 9]
            .iter()
            .all(|&p| p == expected));
    }
}

#[test]
fn headless_sink_delivers_shared_pixels_and_replaces_the_last_frame() {
    let mut sink = WindowSink::new(NonZeroU16::new(1).unwrap()).unwrap();
    let observer = sink.clone();
    assert!(observer.frame().is_none());
    let frame = Frame::Text(grid(&[0xDB, 15], true, true, false));
    let boxed: &mut dyn FrameSink = &mut sink;
    boxed.present(&frame);
    let borrow = observer.frame();
    let pixels = borrow.as_ref().unwrap();
    assert_eq!((pixels.width, pixels.height), (9, 14));
    assert_eq!(pixels.pixels, vec![0xFFFFFF; 9 * 14]);
    assert_eq!(observer.error(), None);
    drop(borrow);
    sink.present(&Frame::Text(grid(&[b' ', 0], true, true, false)));
    assert_eq!(observer.frame().as_ref().unwrap().pixels, vec![0; 9 * 14]);
}

#[test]
fn glyph_pixels_and_offset_cursor_scale_across_multiple_rows() {
    let raster = Rasterizer::bundled(NonZeroU16::new(3).unwrap()).unwrap();
    let font = FontAtlas::from_fon(VGA_FONT).unwrap();
    let grid = render_text_mode(&TextModeState {
        vram: &[b'A', 0x1F, 0xC4, 0x1F],
        cols: 1,
        rows: 2,
        start_cell: 0,
        cursor_cell: 1,
        cursor_visible: true,
        attributes: TextAttributeSet::Colour {
            blink_enabled: true,
        },
    });
    let image = raster.rasterize(&grid).unwrap();
    assert_eq!((image.width, image.height), (24, 96));
    for (cell, glyph) in [b'A', 0xC4].into_iter().enumerate() {
        for (y, bits) in font.glyph_rows(glyph).iter().enumerate() {
            for x in 0..8 {
                let expected =
                    if (cell == 1 && (13..=14).contains(&y)) || bits & (1 << (7 - x)) != 0 {
                        0xFFFFFF
                    } else {
                        0x0000AA
                    };
                for dy in 0..3 {
                    for dx in 0..3 {
                        assert_eq!(
                            image.pixels[((cell * 16 + y) * 3 + dy) * 24 + x * 3 + dx],
                            expected
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn empty_and_oversized_frames_are_bounded_and_sink_keeps_last_success() {
    let empty = grid(&[], true, true, false);
    let raster = Rasterizer::bundled(NonZeroU16::new(1).unwrap()).unwrap();
    assert!(raster.rasterize(&empty).unwrap().pixels.is_empty());
    let mut sink = WindowSink::new(NonZeroU16::new(u16::MAX).unwrap()).unwrap();
    sink.present(&Frame::Text(empty));
    let last = sink.frame().clone();
    sink.present(&Frame::Text(grid(&[b'A', 7], true, true, false)));
    assert_eq!(sink.error(), Some(window::RasterError::TooLarge));
    assert_eq!(*sink.frame(), last);
}

#[test]
fn observation_subscriber_smoke_rasterizes_guest_text_without_a_window() {
    use assembly::{DeviceId, ObservationPolicy, ObservationRequest, OutputKind};
    let config = assembly::at_vga_text(vec![0xC7, 0x06, 0, 0, 0xDB, 0x1F, 0xF4]);
    let policy = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Vga,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..ObservationPolicy::default()
    };
    let mut assembled = assembly::assemble_with_observation(&config, &policy).unwrap();
    let sink = WindowSink::new(NonZeroU16::new(1).unwrap()).unwrap();
    assembled
        .observation
        .subscribe(DeviceId::Vga, Box::new(sink.clone()))
        .unwrap();
    assert_eq!(
        assembled.machine.run_bounded(Some(100)).unwrap(),
        machine::RunOutcome::Halted
    );
    assert!(sink.frame().is_none());
    assembled.capture_text(DeviceId::Vga).unwrap();
    let borrow = sink.frame();
    let frame = borrow.as_ref().unwrap();
    assert_eq!((frame.width, frame.height), (640, 400));
    for y in 0..400 {
        for x in 0..640 {
            assert_eq!(
                frame.pixels[y * 640 + x],
                if y < 16 && x < 8 { 0xFFFFFF } else { 0 }
            );
        }
    }
}

#[test]
fn mda_corner_glyph_preserves_the_ninth_column() {
    let font = FontAtlas::from_fon(MDA_FONT).unwrap();
    assert_eq!(
        font.glyph_rows(0xC0),
        &[48, 48, 48, 48, 48, 48, 48, 63, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn reverse_intensity_keeps_black_ink_and_normal_grey_paper() {
    let raster = Rasterizer::bundled(NonZeroU16::new(1).unwrap()).unwrap();
    let image = raster
        .rasterize(&grid(&[b' ', 0x78, 0xDB, 0x78], true, true, false))
        .unwrap();
    for row in image.pixels.chunks_exact(18) {
        assert_eq!(&row[..9], &[0xAAAAAA; 9]);
        assert_eq!(&row[9..], &[0; 9]);
    }
}

#[test]
fn colour_foregrounds_cover_the_full_palette_including_brown() {
    let expected = [
        0x000000, 0x0000AA, 0x00AA00, 0x00AAAA, 0xAA0000, 0xAA00AA, 0xAA5500, 0xAAAAAA, 0x555555,
        0x5555FF, 0x55FF55, 0x55FFFF, 0xFF5555, 0xFF55FF, 0xFFFF55, 0xFFFFFF,
    ];
    let cells: Vec<u8> = (0..16).flat_map(|attr| [0xDB, attr]).collect();
    let raster = Rasterizer::bundled(NonZeroU16::new(1).unwrap()).unwrap();
    let image = raster.rasterize(&grid(&cells, false, true, false)).unwrap();
    assert_eq!(expected[6], 0xAA5500, "CGA brown");
    for row in image.pixels.chunks_exact(128) {
        for (glyph, &colour) in row.chunks_exact(8).zip(&expected) {
            assert_eq!(glyph, &[colour; 8]);
        }
    }
}

#[test]
fn font_notice_contains_all_required_attribution() {
    for part in [
        "Ultimate Oldschool PC Font Pack v2.2",
        "(c) 2016-2020 VileR",
        "CC BY-SA 4.0",
        "https://creativecommons.org/licenses/by-sa/4.0/",
        "http://int10h.org/oldschool-pc-fonts/",
        "font files unmodified",
    ] {
        assert!(window::FONT_NOTICE.contains(part), "missing {part}");
    }
}
