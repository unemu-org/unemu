// SPDX-License-Identifier: AGPL-3.0-or-later
//! Graphics isolation tests using SeaVGABIOS stdvgamodes.c register tables.
//! IBM VGA/XGA Technical Reference Manual, Graphics Modes and Fig. 2-65.
use super::Vga;
use testkit::MockBus;
use video::{render_graphics, IndexedPixels};

const SEQU_06: [u8; 4] = [0x01, 0x01, 0x00, 0x06];
const CRTC_06: [u8; 25] = [
    0x5f, 0x4f, 0x50, 0x82, 0x54, 0x80, 0xbf, 0x1f, 0x00, 0xc1, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x9c, 0x8e, 0x8f, 0x28, 0x00, 0x96, 0xb9, 0xc2, 0xff,
];
const ACTL_06: [u8; 20] = [
    0x00, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17, 0x17,
    0x01, 0x00, 0x01, 0x00,
];
const GRDC_06: [u8; 9] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0d, 0x0f, 0xff];

const SEQU_0D: [u8; 4] = [0x09, 0x0f, 0x00, 0x06];
const SEQU_0E: [u8; 4] = [0x01, 0x0f, 0x00, 0x06];
const SEQU_13: [u8; 4] = [0x01, 0x0f, 0x00, 0x0e];
const CRTC_0D: [u8; 25] = [
    0x2d, 0x27, 0x28, 0x90, 0x2b, 0x80, 0xbf, 0x1f, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x9c, 0x8e, 0x8f, 0x14, 0x00, 0x96, 0xb9, 0xe3, 0xff,
];
const CRTC_0E: [u8; 25] = [
    0x5f, 0x4f, 0x50, 0x82, 0x54, 0x80, 0xbf, 0x1f, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x9c, 0x8e, 0x8f, 0x28, 0x00, 0x96, 0xb9, 0xe3, 0xff,
];
const CRTC_0F: [u8; 25] = [
    0x5f, 0x4f, 0x50, 0x82, 0x54, 0x80, 0xbf, 0x1f, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x83, 0x85, 0x5d, 0x28, 0x0f, 0x63, 0xba, 0xe3, 0xff,
];
const CRTC_11: [u8; 25] = [
    0x5f, 0x4f, 0x50, 0x82, 0x54, 0x80, 0x0b, 0x3e, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xea, 0x8c, 0xdf, 0x28, 0x00, 0xe7, 0x04, 0xe3, 0xff,
];
const CRTC_13: [u8; 25] = [
    0x5f, 0x4f, 0x50, 0x82, 0x54, 0x80, 0xbf, 0x1f, 0x00, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x9c, 0x8e, 0x8f, 0x28, 0x40, 0x96, 0xb9, 0xa3, 0xff,
];
const ACTL_0D: [u8; 20] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
    0x01, 0x00, 0x0f, 0x00,
];
const ACTL_10: [u8; 20] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x14, 0x07, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f,
    0x01, 0x00, 0x0f, 0x00,
];
const ACTL_13: [u8; 20] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x41, 0x00, 0x0f, 0x00,
];
const GRDC_0D: [u8; 9] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x0f, 0xff];
const GRDC_13: [u8; 9] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x05, 0x0f, 0xff];

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
fn mode(mode: u8) -> MockBus {
    let mut b = MockBus::new(Box::new(Vga::new()));
    let (seq, crtc, attr, gc, misc) = match mode {
        6 => (&SEQU_06, &CRTC_06, &ACTL_06, &GRDC_06, 0x63),
        0x0d => (&SEQU_0D, &CRTC_0D, &ACTL_0D, &GRDC_0D, 0x63),
        0x0e => (&SEQU_0E, &CRTC_0E, &ACTL_0D, &GRDC_0D, 0x63),
        0x10 => (&SEQU_0E, &CRTC_0F, &ACTL_10, &GRDC_0D, 0xa3),
        0x12 => (&SEQU_0E, &CRTC_11, &ACTL_10, &GRDC_0D, 0xe3),
        0x13 => (&SEQU_13, &CRTC_13, &ACTL_13, &GRDC_13, 0x63),
        _ => unreachable!(),
    };
    out(&mut b, 0x3C2, misc);
    reg(&mut b, 0x3C4, 0, 3);
    for (port, start, values) in [
        (0x3C4, 1, seq.as_slice()),
        (0x3D4, 0, crtc.as_slice()),
        (0x3CE, 0, gc.as_slice()),
    ] {
        for (i, &value) in values.iter().enumerate() {
            reg(&mut b, port, start + i as u8, value);
        }
    }
    for (i, &value) in attr.iter().enumerate() {
        ac(&mut b, i as u8, value);
    }
    out(&mut b, 0x3C6, 255);
    out(&mut b, 0x3C8, 0);
    for index in 0..256_u16 {
        for component in [index as u8 & 63, (index >> 2) as u8 & 63, 63] {
            out(&mut b, 0x3C9, component);
        }
    }
    b
}
fn render(b: &MockBus) -> IndexedPixels {
    render_graphics(&b.device().graphics_scanout().expect("graphics capability"))
}
fn write(b: &mut MockBus, offset: u64, value: u8) {
    b.mmio_write(0xA0000 + offset, 1, u64::from(value)).unwrap();
}
fn expanded(v: u8) -> u8 {
    (v << 2) | (v >> 4)
}
fn expected_palette() -> [[u8; 3]; 256] {
    std::array::from_fn(|i| [expanded(i as u8 & 63), expanded((i >> 2) as u8 & 63), 255])
}

#[test]
fn mode_twelve_exact_planar_frame_from_cpu_write_modes_zero_and_two() {
    let mut b = mode(0x12);
    for (mask, value) in [(1, 0x55), (2, 0x33), (4, 0x0f), (8, 0)] {
        reg(&mut b, 0x3C4, 2, mask);
        write(&mut b, 0, value);
    }
    reg(&mut b, 0x3C4, 2, 15);
    reg(&mut b, 0x3CE, 5, 2);
    write(&mut b, 80, 15);
    let before = b.device().save();
    let frame = render(&b);
    let mut indices = vec![0; 640 * 480];
    indices[..8].copy_from_slice(&[0, 1, 2, 3, 4, 5, 0x14, 7]);
    indices[640..648].fill(0x3f);
    assert_eq!(
        frame,
        IndexedPixels {
            width: 640,
            height: 480,
            repeat: (1, 1),
            indices,
            palette: Box::new(expected_palette())
        }
    );
    assert_eq!(b.device().save(), before);
    assert_eq!(render(&b), frame);
    write(&mut b, 0, 0);
    out(&mut b, 0x3C8, 1);
    out(&mut b, 0x3C9, 63);
    assert_ne!(render(&b), frame);
    assert_eq!(frame.indices[1], 1);
    assert_eq!(frame.palette[1], [4, 0, 255]);
}

#[test]
fn mode_thirteen_exact_chain_four_frame_and_doubleword_high_address_bits() {
    let mut b = mode(0x13);
    let points = [
        (0, 0x12),
        (1, 0x34),
        (2, 0x56),
        (3, 0x78),
        (4, 0x9a),
        (319, 0xbc),
        (320, 0xde),
        (16384, 0xe1),
        (32768, 0xf2),
        (63999, 0x7f),
    ];
    let mut indices = vec![0; 320 * 200];
    for (offset, value) in points {
        write(&mut b, offset, value);
        indices[offset as usize] = value;
    }
    assert_eq!(
        render(&b),
        IndexedPixels {
            width: 320,
            height: 200,
            repeat: (2, 2),
            indices,
            palette: Box::new(expected_palette())
        }
    );
    reg(&mut b, 0x3CE, 5, 0x60);
    assert_eq!(render(&b).indices[4], 0x9a);
    // MSL=1 repeats each memory row even though DSC itself is clear.
    reg(&mut b, 0x3D4, 9, 0x40);
    assert_eq!(render(&b).height, 400);
    reg(&mut b, 0x3D4, 9, 0xc1);
    assert_eq!(render(&b).height, 100);
}

#[test]
fn planar_geometry_from_all_five_firmware_tables() {
    for (m, w, h, stride) in [
        (0x0d, 320, 200, 40),
        (0x0e, 640, 200, 80),
        (0x10, 640, 350, 80),
        (0x12, 640, 480, 80),
    ] {
        let mut b = mode(m);
        reg(&mut b, 0x3CE, 5, 2);
        write(&mut b, stride, 1);
        let frame = render(&b);
        assert_eq!((frame.width, frame.height), (w, h));
        assert_eq!(&frame.indices[usize::from(w)..usize::from(w) + 8], &[1; 8]);
        // Dot-clock division changes physical dot duration, not logical width.
        reg(&mut b, 0x3C4, 1, 0x19);
        let mut expected = frame;
        expected.repeat.0 = 2;
        assert_eq!(render(&b), expected);
    }
    let mut b = mode(0x12);
    reg(&mut b, 0x3D4, 0x11, 0x0c);
    for (overflow, height) in [(0x7e, 992), (0x3c, 224)] {
        reg(&mut b, 0x3D4, 7, overflow);
        assert_eq!(render(&b).height, height);
    }
}

#[test]
fn graphics_start_offset_word_wrap_and_doubleword_addressing() {
    for (m, offset, expected) in [(0x12, 80, 0x3f), (0x13, 320, 15)] {
        let mut b = mode(m);
        if m == 0x12 {
            reg(&mut b, 0x3CE, 5, 2);
        }
        write(&mut b, offset, 15);
        reg(&mut b, 0x3D4, 0x0d, 80);
        latch(&mut b);
        assert_eq!(render(&b).indices[0], expected);
        reg(&mut b, 0x3D4, 0x13, 0);
        assert_eq!(render(&b).indices[usize::from(render(&b).width)], expected);
    }
    let mut b = mode(0x12);
    reg(&mut b, 0x3CE, 5, 2);
    write(&mut b, 0x4001, 3);
    reg(&mut b, 0x3D4, 0x0c, 0x20);
    latch(&mut b);
    reg(&mut b, 0x3D4, 0x17, 0x83);
    assert_eq!(render(&b).indices[0], 3);
    reg(&mut b, 0x3D4, 0x17, 0xa3);
    assert_eq!(render(&b).indices[0], 0);
    reg(&mut b, 0x3D4, 0x0c, 0x10);
    latch(&mut b);
    reg(&mut b, 0x3D4, 0x14, 0x40);
    assert_eq!(render(&b).indices[0], 3);
}

#[test]
fn graphics_attribute_palette_plane_enable_color_select_p54s_and_pel_mask() {
    let mut b = mode(0x12);
    reg(&mut b, 0x3CE, 5, 2);
    write(&mut b, 0, 15);
    ac(&mut b, 0x12, 7);
    ac(&mut b, 7, 0x2a);
    assert_eq!(render(&b).indices[0], 0x2a);
    ac(&mut b, 0x14, 0x0e);
    assert_eq!(render(&b).indices[0], 0xea);
    ac(&mut b, 0x10, 0x81);
    assert_eq!(render(&b).indices[0], 0xea);
    ac(&mut b, 0x14, 0x0d);
    let frame = render(&b);
    assert_eq!(frame.indices[0], 0xda);
    out(&mut b, 0x3C6, 15);
    assert_eq!(render(&b).palette[0xda], expected_palette()[10]);
    let mut b = mode(0x13);
    write(&mut b, 0, 0xff);
    ac(&mut b, 0x12, 7);
    assert_eq!(render(&b).indices[0], 0x77);
    ac(&mut b, 7, 3);
    assert_eq!(render(&b).indices[0], 0x33);
    out(&mut b, 0x3C6, 15);
    assert_eq!(render(&b).palette[0x33], expected_palette()[3]);
}

#[test]
fn graphics_screen_off_and_unsupported_shift_modes_blank_without_changing_vram() {
    for m in [0x12, 0x13] {
        let mut b = mode(m);
        write(&mut b, 0, 0xff);
        let frame = render(&b);
        assert_ne!(frame.indices[0], 0);
        reg(&mut b, 0x3C4, 1, 0x21);
        assert!(render(&b).indices.iter().all(|&p| p == 0));
        assert_eq!(render(&b).palette.as_ref(), &[[0; 3]; 256]);
        reg(&mut b, 0x3C4, 1, 1);
        assert_eq!(render(&b), frame);
        b.port_read(0x3DA, 1).unwrap();
        out(&mut b, 0x3C0, 0);
        assert_eq!(render(&b).palette.as_ref(), &[[0; 3]; 256]);
        ac(&mut b, 0x10, 1);
        reg(&mut b, 0x3CE, 5, 0x20);
        assert_eq!(render(&b).palette.as_ref(), &[[0; 3]; 256]);
        reg(&mut b, 0x3CE, 5, 0x40);
        assert_eq!(render(&b).palette.as_ref(), &[[0; 3]; 256]);
    }
}

#[test]
fn mode_six_cga_row_scan_banks_preserve_two_hundred_logical_lines() {
    let mut b = mode(6);
    for (offset, byte) in [(0, 0x80), (0x2000, 0x40), (80, 0x20), (0x2050, 0x10)] {
        b.mmio_write(0xB8000 + offset, 1, byte).unwrap();
    }
    let frame = render(&b);
    assert_eq!((frame.width, frame.height), (640, 200));
    let mut expected = vec![0; 640 * 200];
    for (y, x) in [(0, 0), (1, 1), (2, 2), (3, 3)] {
        expected[y * 640 + x] = 0x17;
    }
    assert_eq!(frame.indices, expected);
    // Both row-scan bits may replace counter bits, independently of byte wiring.
    reg(&mut b, 0x3D4, 9, 0xc3);
    reg(&mut b, 0x3D4, 0x17, 0xc0);
    b.mmio_write(0xBC000, 1, 0x08).unwrap();
    b.mmio_write(0xBE000, 1, 0x04).unwrap();
    let frame = render(&b);
    assert_eq!(
        &frame.indices[2 * 640..2 * 640 + 8],
        &[0, 0, 0, 0, 0x17, 0, 0, 0]
    );
    assert_eq!(
        &frame.indices[3 * 640..3 * 640 + 8],
        &[0, 0, 0, 0, 0, 0x17, 0, 0]
    );
    reg(&mut b, 0x3D4, 0x0c, 0x60);
    latch(&mut b);
    assert_eq!(render(&b).indices, frame.indices);
    reg(&mut b, 0x3D4, 0x17, 0x80);
    reg(&mut b, 0x3D4, 0x0c, 0x10);
    latch(&mut b);
    assert_eq!(&render(&b).indices[..8], &[0x17, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn mode_thirteen_ignores_color_select_p54s_and_high_internal_palette_bits() {
    let mut b = mode(0x13);
    write(&mut b, 0, 0x11);
    let original = render(&b);
    ac(&mut b, 1, 0x31);
    assert_eq!(render(&b), original);
    ac(&mut b, 0x14, 0x0f);
    ac(&mut b, 0x10, 0xc1);
    assert_eq!(render(&b), original);
    ac(&mut b, 1, 0x3f);
    assert_eq!(render(&b).indices[0], 0xff);
    ac(&mut b, 0x14, 0);
    ac(&mut b, 0x10, 0x41);
    assert_eq!(render(&b).indices[0], 0xff);
}

#[test]
fn firmware_graphics_modes_retain_dot_and_scan_line_repeats() {
    for (mode_number, dimensions, repeat) in [
        (6, (640, 200), (1, 2)),
        (0x0d, (320, 200), (2, 2)),
        (0x0e, (640, 200), (1, 2)),
        (0x10, (640, 350), (1, 1)),
        (0x12, (640, 480), (1, 1)),
        (0x13, (320, 200), (2, 2)),
    ] {
        let b = mode(mode_number);
        let frame = render(&b);
        assert_eq!((frame.width, frame.height), dimensions);
        assert_eq!(frame.repeat, repeat);
    }
}

fn latch(b: &mut MockBus) {
    b.tick(40_000);
}

fn pattern(b: &mut MockBus) {
    for (plane, bytes) in [
        (1, [0x55, 0x55]),
        (2, [0x33, 0x33]),
        (4, [0x0f, 0x0f]),
        (8, [0, 0xff]),
    ] {
        reg(b, 0x3C4, 2, plane);
        for (i, value) in bytes.into_iter().enumerate() {
            write(b, i as u64, value);
        }
    }
}

#[test]
fn scrolling_planar_panning_all_eight_positions_crosses_byte_boundary() {
    for m in [0x0d, 0x12] {
        for pan in 0..8 {
            let mut b = mode(m);
            pattern(&mut b);
            ac(&mut b, 0x13, pan);
            latch(&mut b);
            let colours = if m == 0x12 { &ACTL_10 } else { &ACTL_0D };
            let expected: Vec<_> = (pan..pan + 8).map(|x| colours[usize::from(x)]).collect();
            assert_eq!(&render(&b).indices[..8], expected, "mode {m:x}, pan {pan}");
        }
    }
}

#[test]
fn scrolling_packed_half_steps_and_assumed_odd_values_in_both_memory_layouts() {
    for chained in [true, false] {
        for pan in 0..8 {
            let mut b = mode(0x13);
            if chained {
                for x in 0..8 {
                    write(&mut b, x, x as u8 + 1);
                }
            } else {
                reg(&mut b, 0x3C4, 4, 6);
                reg(&mut b, 0x3D4, 0x14, 0);
                reg(&mut b, 0x3D4, 0x17, 0xe3);
                for x in 0..8 {
                    reg(&mut b, 0x3C4, 2, 1 << (x & 3));
                    write(&mut b, x / 4, x as u8 + 1);
                }
            }
            ac(&mut b, 0x13, pan);
            latch(&mut b);
            let first = pan / 2 + 1;
            assert_eq!(
                &render(&b).indices[..4],
                &[first, first + 1, first + 2, first + 3]
            );
        }
    }
}

#[test]
fn scrolling_byte_panning_selects_next_plane_byte_and_packed_ignores_it() {
    for pan in 1..=3 {
        let mut b = mode(0x12);
        reg(&mut b, 0x3CE, 5, 2);
        write(&mut b, u64::from(pan), pan);
        reg(&mut b, 0x3D4, 8, pan << 5);
        latch(&mut b);
        assert_eq!(&render(&b).indices[..8], &[pan; 8]);
    }
    let mut b = mode(0x13);
    write(&mut b, 0, 0x45);
    reg(&mut b, 0x3D4, 8, 0x60);
    assert_eq!(render(&b).indices[0], 0x45);
}

#[test]
fn scrolling_preset_row_scan_advances_address_after_remaining_scans() {
    let mut b = mode(0x12);
    reg(&mut b, 0x3D4, 9, 0x43);
    reg(&mut b, 0x3D4, 8, 2);
    reg(&mut b, 0x3CE, 5, 2);
    write(&mut b, 0, 1);
    write(&mut b, 80, 2);
    latch(&mut b);
    let frame = render(&b);
    let rows: Vec<_> = frame.indices.chunks(640).take(6).map(|r| r[0]).collect();
    assert_eq!(frame.repeat.1, 1);
    assert_eq!(rows, [1, 1, 2, 2, 2, 2]);
}

#[test]
fn scrolling_line_compare_restarts_zero_and_optionally_resets_pixel_panning() {
    for compat in [false, true] {
        let mut b = mode(0x12);
        pattern(&mut b);
        reg(&mut b, 0x3C4, 2, 15);
        reg(&mut b, 0x3CE, 5, 2);
        write(&mut b, 80, 3);
        reg(&mut b, 0x3D4, 0x0d, 80);
        reg(&mut b, 0x3D4, 0x11, 0x0c);
        reg(&mut b, 0x3D4, 7, 0x2e);
        reg(&mut b, 0x3D4, 9, 0);
        reg(&mut b, 0x3D4, 0x18, 1);
        ac(&mut b, 0x13, 3);
        ac(&mut b, 0x10, if compat { 0x21 } else { 1 });
        latch(&mut b);
        let f = render(&b);
        assert_eq!(&f.indices[..5], &[3; 5]);
        assert_eq!(
            &f.indices[2 * 640..2 * 640 + 5],
            if compat {
                &[0, 1, 2, 3, 4]
            } else {
                &[3, 4, 5, 0x14, 7]
            }
        );
    }
}

#[test]
fn scrolling_line_compare_uses_both_overflow_bits_and_physical_double_scans() {
    for target in [2_u16, 258, 514, 770] {
        let mut b = mode(0x12);
        reg(&mut b, 0x3D4, 0x11, 0x0c);
        reg(&mut b, 0x3D4, 7, 0x4a | (((target >> 8) as u8 & 1) << 4));
        reg(&mut b, 0x3D4, 9, 0x80 | (((target >> 9) as u8 & 1) << 6));
        reg(&mut b, 0x3D4, 0x12, 0xff);
        reg(&mut b, 0x3D4, 0x18, target as u8);
        reg(&mut b, 0x3D4, 0x0d, 80);
        reg(&mut b, 0x3D4, 0x13, 0);
        reg(&mut b, 0x3CE, 5, 2);
        write(&mut b, 0, 1);
        write(&mut b, 80, 2);
        latch(&mut b);
        let f = render(&b);
        assert_eq!((f.height, f.repeat.1), (1024, 1));
        for y in [0, target, target + 1, target + 2] {
            assert_eq!(
                f.indices[usize::from(y) * 640],
                if y <= target { 2 } else { 1 },
                "target {target}, y {y}"
            );
        }
    }
}

#[test]
fn scrolling_start_and_pel_pan_latch_at_retrace_even_when_tick_skips_the_pulse() {
    let mut b = mode(0x12);
    reg(&mut b, 0x3D4, 0x11, 9);
    for (r, v) in [
        (0, 5),
        (1, 0),
        (6, 8),
        (7, 0x10),
        (9, 0x40),
        (0x10, 7),
        (0x12, 3),
    ] {
        reg(&mut b, 0x3D4, r, v);
    }
    pattern(&mut b);
    reg(&mut b, 0x3C4, 2, 15);
    reg(&mut b, 0x3CE, 5, 2);
    write(&mut b, 80, 3);
    let tick_for_dot = |dot: u64| device::clock::VGA_25.edge_at(dot).0;
    b.tick(tick_for_dot(80));
    reg(&mut b, 0x3D4, 0x0d, 80);
    ac(&mut b, 0x13, 3);
    let pending = b.device().save();
    assert_eq!(
        b.device().next_event(b.clock()),
        device::NextEvent::At(device::VClock(tick_for_dot(560)))
    );
    assert_eq!(&render(&b).indices[..5], &[0, 1, 2, 3, 4]);
    b.tick(tick_for_dot(560) - 1 - b.clock().0);
    assert_eq!(&render(&b).indices[..5], &[0, 1, 2, 3, 4]);
    b.tick(tick_for_dot(560) - b.clock().0);
    assert_eq!(&render(&b).indices[..5], &[3; 5]);
    assert_eq!(
        b.device().next_event(b.clock()),
        device::NextEvent::At(device::VClock(tick_for_dot(720)))
    );
    reg(&mut b, 0x3D4, 0x0d, 0);
    assert_eq!(&render(&b).indices[..5], &[3; 5]);
    b.tick(tick_for_dot(1600) - b.clock().0);
    assert_eq!(&render(&b).indices[..5], &[3, 4, 5, 0x14, 7]);
    b.device_mut().restore(&pending).unwrap();
    assert_eq!(&render(&b).indices[..5], &[0, 1, 2, 3, 4]);
    b.device_mut().tick(device::VClock(tick_for_dot(560)));
    assert_eq!(&render(&b).indices[..5], &[3; 5]);
}

#[test]
fn scrolling_snapshot_four_preserves_pending_and_three_migrates_live_registers() {
    let mut b = mode(0x12);
    pattern(&mut b);
    reg(&mut b, 0x3D4, 0x0d, 1);
    ac(&mut b, 0x13, 2);
    let saved = b.device().save();
    assert_eq!(saved[0], 4);
    let mut old = saved.clone();
    old[0] = 3;
    let control = super::HEADER_LEN - 16;
    old[control + 3..control + 8].fill(0);
    b.device_mut().restore(&old).unwrap();
    assert_eq!(&render(&b).indices[..4], &[0x3a, 0x3b, 0x3c, 0x3d]);
    b.device_mut().restore(&saved).unwrap();
    assert_eq!(&render(&b).indices[..4], &[0, 1, 2, 3]);
    let mut bad = saved.clone();
    bad[control + 5] = 16;
    assert!(b.device_mut().restore(&bad).is_err());
    assert_eq!(b.device().save(), saved);
}

fn scrolling_raster() -> MockBus {
    let mut b = mode(0x12);
    reg(&mut b, 0x3D4, 0x11, 9);
    for (index, value) in [
        (0, 5),
        (1, 0),
        (6, 8),
        (7, 0x10),
        (9, 0x40),
        (0x10, 7),
        (0x12, 3),
    ] {
        reg(&mut b, 0x3D4, index, value);
    }
    pattern(&mut b);
    b
}

fn tick_at_dot(b: &mut MockBus, dot: u64) {
    let target = device::clock::VGA_25.edge_at(dot).0;
    b.tick(target - b.clock().0);
}

fn tick_before_dot(b: &mut MockBus, dot: u64) {
    let target = device::clock::VGA_25.edge_at(dot).0 - 1;
    b.tick(target - b.clock().0);
}

#[test]
fn scrolling_active_start_and_retrace_pan_pair_for_next_frame() {
    let mut b = scrolling_raster();
    ac(&mut b, 0x13, 7);
    tick_at_dot(&mut b, 720);
    assert_eq!(&render(&b).indices[..5], &[7, 0x38, 0x39, 0x3a, 0x3b]);
    tick_at_dot(&mut b, 880);
    reg(&mut b, 0x3D4, 0x0d, 1);
    assert_eq!(&render(&b).indices[..5], &[7, 0x38, 0x39, 0x3a, 0x3b]);
    tick_at_dot(&mut b, 1360);
    assert_ne!(b.port_read(0x3DA, 1).unwrap() & 8, 0);
    ac(&mut b, 0x13, 0);
    let end = device::clock::VGA_25.edge_at(1520_u64).0;
    assert_eq!(
        b.device().next_event(b.clock()),
        device::NextEvent::At(device::VClock(end))
    );
    tick_before_dot(&mut b, 1520);
    assert_eq!(render(&b).indices[0], 0x3f);
    tick_at_dot(&mut b, 1520);
    assert_eq!(b.port_read(0x3DA, 1).unwrap() & 8, 0);
    assert_eq!(&render(&b).indices[..5], &[0x38, 0x39, 0x3a, 0x3b, 0x3c]);
    assert_eq!(b.device().next_event(b.clock()), device::NextEvent::Dormant);
}

#[test]
fn scrolling_display_pan_waits_for_retrace_end_and_restores_pending_phase() {
    let mut b = scrolling_raster();
    tick_at_dot(&mut b, 80);
    ac(&mut b, 0x13, 3);
    let saved = b.device().save();
    tick_at_dot(&mut b, 560);
    assert_eq!(&render(&b).indices[..5], &[0, 1, 2, 3, 4]);
    tick_before_dot(&mut b, 720);
    assert_eq!(&render(&b).indices[..5], &[0, 1, 2, 3, 4]);
    tick_at_dot(&mut b, 720);
    assert_eq!(&render(&b).indices[..5], &[3, 4, 5, 0x14, 7]);
    b.device_mut().restore(&saved).unwrap();
    b.device_mut()
        .tick(device::VClock(device::clock::VGA_25.edge_at(560_u64).0));
    assert_eq!(&render(&b).indices[..5], &[0, 1, 2, 3, 4]);
    // A coarse tick crosses both edges without status polling.
    b.device_mut()
        .tick(device::VClock(device::clock::VGA_25.edge_at(1600_u64).0));
    assert_eq!(&render(&b).indices[..5], &[3, 4, 5, 0x14, 7]);
}

#[test]
fn scrolling_crtc_row_pan_latches_with_start_and_snapshot_preserves_it() {
    let mut b = scrolling_raster();
    reg(&mut b, 0x3D4, 9, 0x43);
    reg(&mut b, 0x3C4, 2, 15);
    reg(&mut b, 0x3CE, 5, 2);
    for (offset, colour) in [(0, 1), (1, 2), (80, 3), (81, 4), (161, 5)] {
        write(&mut b, offset, colour);
    }
    tick_at_dot(&mut b, 80);
    reg(&mut b, 0x3D4, 8, 0x21);
    reg(&mut b, 0x3D4, 0x0d, 80);
    assert_eq!(&render(&b).indices[..5], &[1; 5]);
    tick_before_dot(&mut b, 560);
    assert_eq!(&render(&b).indices[..5], &[1; 5]);
    tick_at_dot(&mut b, 560);
    let f = render(&b);
    assert_eq!(f.repeat.1, 1);
    assert_eq!(
        f.indices.chunks(8).map(|line| line[0]).collect::<Vec<_>>(),
        [4, 4, 4, 5]
    );
    reg(&mut b, 0x3D4, 8, 0);
    let saved = b.device().save();
    b.device_mut().restore(&saved).unwrap();
    assert_eq!(render(&b), f);
    let control = super::HEADER_LEN - 16;
    assert_eq!(saved[control + 6], 0x21);
    let next_start = device::clock::VGA_25.edge_at(1360_u64).0;
    assert_eq!(
        b.device().next_event(b.clock()),
        device::NextEvent::At(device::VClock(next_start))
    );
    let mut invalid = saved.clone();
    invalid[control + 6] = 0x80;
    assert!(b.device_mut().restore(&invalid).is_err());
    assert_eq!(b.device().save(), saved);
    let mut old = saved;
    old[0] = 3;
    old[control + 3..control + 8].fill(0);
    b.device_mut().restore(&old).unwrap();
    assert_eq!(&render(&b).indices[..5], &[3; 5]);
    old[12 + 4 + 5 + 8] = 0x21;
    b.device_mut().restore(&old).unwrap();
    assert_eq!(render(&b), f);
}

#[test]
fn scrolling_retrace_end_wraps_across_total_and_skipped_pulses() {
    let mut b = scrolling_raster();
    reg(&mut b, 0x3D4, 0x10, 9);
    reg(&mut b, 0x3D4, 0x11, 11);
    tick_at_dot(&mut b, 160);
    ac(&mut b, 0x13, 5);
    let end = device::clock::VGA_25.edge_at(880_u64).0;
    assert_eq!(
        b.device().next_event(b.clock()),
        device::NextEvent::At(device::VClock(end))
    );
    tick_at_dot(&mut b, 800);
    assert_ne!(b.port_read(0x3DA, 1).unwrap() & 8, 0);
    assert_eq!(&render(&b).indices[..3], &[0, 1, 2]);
    tick_at_dot(&mut b, 880);
    assert_eq!(b.port_read(0x3DA, 1).unwrap() & 8, 0);
    assert_eq!(&render(&b).indices[..3], &[5, 0x14, 7]);
    ac(&mut b, 0x13, 2);
    tick_at_dot(&mut b, 16_800);
    assert_eq!(&render(&b).indices[..3], &[2, 3, 4]);
}
