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
        assert_eq!(render(&b).indices[0], expected);
        reg(&mut b, 0x3D4, 0x13, 0);
        assert_eq!(render(&b).indices[usize::from(render(&b).width)], expected);
    }
    let mut b = mode(0x12);
    reg(&mut b, 0x3CE, 5, 2);
    write(&mut b, 0x4001, 3);
    reg(&mut b, 0x3D4, 0x0c, 0x20);
    reg(&mut b, 0x3D4, 0x17, 0x83);
    assert_eq!(render(&b).indices[0], 3);
    reg(&mut b, 0x3D4, 0x17, 0xa3);
    assert_eq!(render(&b).indices[0], 0);
    reg(&mut b, 0x3D4, 0x0c, 0x10);
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
    assert_eq!(render(&b).indices, frame.indices);
    reg(&mut b, 0x3D4, 0x17, 0x80);
    reg(&mut b, 0x3D4, 0x0c, 0x10);
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
