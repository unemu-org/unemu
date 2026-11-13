// SPDX-License-Identifier: AGPL-3.0-or-later
//! Microsoft serial mouse through PC16550D registers on an isolated mock bus.
//! Microsoft Mouse Programmer's Reference, Mouse Hardware, serial data format;
//! National Semiconductor PC16550D datasheet §8, Registers.

use device::{NextEvent, VClock};
use devices::{serial_mouse::STARTUP_CLOCKS, SerialMouse, Uart16550};
use testkit::MockBus;

const BASE: u16 = 0x3f8;
const FRAME: u64 = 8949;

fn mouse_bus() -> MockBus {
    let mut bus = MockBus::new(Box::new(
        Uart16550::new(BASE).with_serial_device(Box::new(SerialMouse::new())),
    ));
    write(&mut bus, 3, 0x80);
    write(&mut bus, 0, 96);
    write(&mut bus, 1, 0);
    write(&mut bus, 3, 2);
    write(&mut bus, 1, 5);
    bus
}

fn write(bus: &mut MockBus, offset: u16, value: u32) {
    bus.port_write(BASE + offset, 1, value).unwrap();
}
fn read(bus: &mut MockBus, offset: u16) -> u8 {
    bus.port_read(BASE + offset, 1).unwrap() as u8
}
fn inject(bus: &mut MockBus, x: i32, y: i32, buttons: u8) {
    bus.device_mut()
        .as_any_mut()
        .unwrap()
        .downcast_mut::<Uart16550>()
        .unwrap()
        .serial_device_mut()
        .unwrap()
        .as_any_mut()
        .downcast_mut::<SerialMouse>()
        .unwrap()
        .inject_motion(x, y, buttons);
}
fn identify(bus: &mut MockBus) {
    write(bus, 4, 0x0b);
    bus.tick(STARTUP_CLOCKS + FRAME);
    assert_eq!(read(bus, 0), b'M');
}
fn byte(bus: &mut MockBus) -> u8 {
    bus.tick(FRAME - 1);
    assert_eq!(read(bus, 5) & 1, 0);
    bus.tick(1);
    assert_ne!(read(bus, 5) & 1, 0);
    read(bus, 0)
}
fn packet(bus: &mut MockBus) -> [u8; 3] {
    std::array::from_fn(|_| byte(bus))
}

#[test]
fn serial_mouse_identification_waits_for_both_lines_and_full_frame_then_raises_irq4_source() {
    let mut bus = mouse_bus();
    write(&mut bus, 4, 9);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
    assert!(!bus.device().irq_pending());
    let powered = bus.clock().0;
    write(&mut bus, 4, 0x0b);
    assert_eq!(
        bus.device().next_event(bus.clock()),
        NextEvent::At(VClock(powered + STARTUP_CLOCKS))
    );
    bus.tick(STARTUP_CLOCKS + FRAME - 1);
    assert_eq!(read(&mut bus, 5), 0x60);
    bus.tick(1);
    assert!(bus.device().irq_pending());
    assert_eq!(read(&mut bus, 2), 4);
    assert_eq!(read(&mut bus, 0), b'M');
    assert!(!bus.device().irq_pending());
    write(&mut bus, 4, 0x0b);
    bus.tick(100_000);
    assert_eq!(
        read(&mut bus, 5),
        0x60,
        "unchanged active lines do not identify again"
    );
}

#[test]
fn serial_mouse_rts_low_drops_motion_and_aborts_inflight_output() {
    let mut bus = mouse_bus();
    identify(&mut bus);
    inject(&mut bus, 10, 20, 1);
    bus.tick(100);
    write(&mut bus, 4, 9);
    inject(&mut bus, 99, 99, 0);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
    assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
    identify(&mut bus);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
}

#[test]
fn serial_mouse_dtr_low_also_cancels_startup_and_motion() {
    let mut bus = mouse_bus();
    write(&mut bus, 4, 0x0b);
    bus.tick(1000);
    write(&mut bus, 4, 0x0a);
    inject(&mut bus, 50, 50, 0);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
    identify(&mut bus);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
}

#[test]
fn serial_mouse_encodes_buttons_signed_motion_and_splits_excess_without_loss() {
    let mut bus = mouse_bus();
    identify(&mut bus);
    inject(&mut bus, 300, -300, 3);
    assert_eq!(packet(&mut bus), [0x79, 63, 0]); // +127, -128
    assert_eq!(packet(&mut bus), [0x79, 63, 0]);
    assert_eq!(packet(&mut bus), [0x7c, 46, 20]); // +46, -44
    inject(&mut bus, 0, 0, 2);
    assert_eq!(packet(&mut bus), [0x50, 0, 0]);
    inject(&mut bus, 0, 0, 4);
    assert_eq!(packet(&mut bus), [0x40, 0, 0]);
    inject(&mut bus, 0, 0, 0);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
}

#[test]
fn serial_mouse_busy_packet_finishes_before_new_motion_packet() {
    let mut bus = mouse_bus();
    identify(&mut bus);
    inject(&mut bus, -1, 1, 1);
    assert_eq!(byte(&mut bus), 0x63);
    inject(&mut bus, 2, -2, 2);
    assert_eq!(byte(&mut bus), 63);
    assert_eq!(byte(&mut bus), 1);
    assert_eq!(packet(&mut bus), [0x5c, 2, 62]);
}

#[test]
fn serial_mouse_wrong_divisor_or_frame_produces_zero_with_framing_error_at_physical_rate() {
    for (divisor, lcr) in [(12, 2), (96, 3), (96, 0x0a), (0, 2)] {
        let mut bus = mouse_bus();
        write(&mut bus, 3, 0x80);
        write(&mut bus, 0, divisor);
        write(&mut bus, 3, lcr);
        write(&mut bus, 4, 0x0b);
        bus.tick(STARTUP_CLOCKS + FRAME - 1);
        assert_eq!(read(&mut bus, 5), 0x60);
        bus.tick(1);
        assert_eq!(read(&mut bus, 2), 6);
        assert!(bus.device().irq_pending());
        assert_eq!(read(&mut bus, 5), 0x69);
        assert_eq!(read(&mut bus, 5), 0x61);
        assert_eq!(read(&mut bus, 0), 0);
        assert!(!bus.device().irq_pending());
    }
}

#[test]
fn serial_mouse_irq_requires_ier_and_out2_and_fifo_timeout_follows_guest_divisor() {
    let mut bus = mouse_bus();
    write(&mut bus, 1, 0);
    identify(&mut bus);
    inject(&mut bus, 1, 1, 0);
    bus.tick(FRAME);
    assert!(!bus.device().irq_pending());
    write(&mut bus, 4, 3);
    write(&mut bus, 1, 1);
    assert_eq!(read(&mut bus, 2), 4);
    assert!(!bus.device().irq_pending());
    write(&mut bus, 4, 0x0b);
    assert!(bus.device().irq_pending());
    read(&mut bus, 0);
    write(&mut bus, 2, 0x41); // FIFO trigger four; only two packet bytes remain.
    bus.tick(2 * FRAME);
    assert_eq!(read(&mut bus, 2), 0xc1);
    bus.tick(35_796 - 1); // ceil(4 * 9 * 3579545 / 3600)
    assert!(!bus.device().irq_pending());
    bus.tick(1);
    assert_eq!(read(&mut bus, 2), 0xcc);
    assert!(bus.device().irq_pending());
    assert_eq!(read(&mut bus, 0), 1);
    assert_eq!(read(&mut bus, 2), 0xc1);
    write(&mut bus, 2, 0x43);
    assert_eq!(read(&mut bus, 5), 0x60);
}

#[test]
fn serial_mouse_overrun_retains_fifo_and_reports_line_status_priority() {
    for fifo in [false, true] {
        let mut bus = mouse_bus();
        identify(&mut bus);
        if fifo {
            write(&mut bus, 2, 1);
        }
        inject(&mut bus, 3000, 0, 0);
        bus.tick(20 * FRAME);
        assert_eq!(read(&mut bus, 2) & 0xf, 6);
        assert_eq!(read(&mut bus, 5) & 3, 3);
        assert_eq!(read(&mut bus, 2) & 0xf, 4);
        let mut count = 0;
        while read(&mut bus, 5) & 1 != 0 {
            read(&mut bus, 0);
            count += 1;
        }
        assert_eq!(count, if fifo { 16 } else { 1 });
    }
}

#[test]
fn serial_mouse_snapshot_continues_startup_partial_packet_and_accumulated_motion() {
    for stage in [0, 1, 2, 3] {
        let mut original = mouse_bus();
        write(&mut original, 2, 1);
        write(&mut original, 4, 0x0b);
        if stage > 0 {
            original.tick(STARTUP_CLOCKS + FRAME);
            read(&mut original, 0);
        }
        inject(&mut original, 300, -300, 1);
        if stage > 1 {
            original.tick(FRAME + 100);
        }
        if stage > 2 {
            write(&mut original, 4, 9);
        }
        let saved = original.device().save();
        assert!(saved
            .windows(b"microsoft-serial-mouse".len())
            .any(|s| s == b"microsoft-serial-mouse"));
        let mut restored = mouse_bus();
        restored.device_mut().restore(&saved).unwrap();
        for step in 1..12 {
            let now = VClock(original.clock().0 + step * FRAME);
            original.device_mut().sync(now);
            restored.device_mut().sync(now);
            assert_eq!(original.device().save(), restored.device().save());
            assert_eq!(read(&mut original, 2), read(&mut restored, 2));
            assert_eq!(read(&mut original, 5), read(&mut restored, 5));
            assert_eq!(read(&mut original, 0), read(&mut restored, 0));
        }
    }
}

#[test]
fn serial_mouse_snapshot_rejects_wrong_model_truncations_and_trailing_bytes_atomically() {
    let mut bus = mouse_bus();
    identify(&mut bus);
    inject(&mut bus, 500, -500, 3);
    bus.tick(FRAME + 100);
    let saved = bus.device().save();
    for len in 0..saved.len() {
        assert!(
            bus.device_mut().restore(&saved[..len]).is_err(),
            "truncation {len}"
        );
        assert_eq!(bus.device().save(), saved);
    }
    let mut invalid = saved.clone();
    invalid.push(0);
    assert!(bus.device_mut().restore(&invalid).is_err());
    assert_eq!(bus.device().save(), saved);
    let mut plain = Uart16550::new(BASE);
    assert!(device::Device::restore(&mut plain, &saved).is_err());
}

#[test]
fn serial_mouse_large_jump_matches_one_clock_ticks() {
    let mut fine = mouse_bus();
    let mut coarse = mouse_bus();
    for bus in [&mut fine, &mut coarse] {
        write(bus, 2, 1);
        write(bus, 4, 0x0b);
        inject(bus, 500, -600, 3);
    }
    for _ in 0..200_000 {
        fine.tick(1);
    }
    coarse.tick(200_000);
    assert_eq!(fine.device().save(), coarse.device().save());
}

#[test]
fn serial_mouse_reset_keeps_attachment_and_requires_a_new_power_transition() {
    let mut bus = mouse_bus();
    identify(&mut bus);
    inject(&mut bus, 1000, -1000, 3);
    bus.tick(100);
    bus.device_mut().reset();
    assert_eq!(bus.device().next_event(VClock::ZERO), NextEvent::Dormant);
    for (offset, value) in [(3, 0x80), (0, 96), (1, 0), (3, 2), (4, 0x0b)] {
        write(&mut bus, offset, value);
    }
    bus.device_mut().sync(VClock(STARTUP_CLOCKS + FRAME));
    assert_eq!(read(&mut bus, 0), b'M');
    bus.device_mut().sync(VClock(100_000));
    assert_eq!(read(&mut bus, 5), 0x60);
}

#[test]
fn serial_mouse_accumulation_saturates_at_i32_limits_before_packet_splitting() {
    let mut bus = mouse_bus();
    identify(&mut bus);
    inject(&mut bus, i32::MAX, i32::MIN, 0);
    inject(&mut bus, 1, -1, 0);
    assert_eq!(packet(&mut bus), [0x49, 63, 0]);
    assert_eq!(packet(&mut bus), [0x49, 63, 0]);
}

#[test]
fn serial_mouse_motion_during_startup_and_held_unpowered_buttons_follow_identification() {
    let mut bus = mouse_bus();
    inject(&mut bus, 99, 99, 1);
    write(&mut bus, 4, 0x0b);
    inject(&mut bus, 1, 2, 1);
    bus.tick(STARTUP_CLOCKS + FRAME);
    assert_eq!(read(&mut bus, 0), b'M');
    assert_eq!(packet(&mut bus), [0x60, 1, 2]);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
}

#[test]
fn serial_mouse_loopback_probe_disconnects_endpoint_and_restarts_identification() {
    let mut bus = mouse_bus();
    identify(&mut bus);
    inject(&mut bus, 300, 10, 0);
    bus.tick(100);
    write(&mut bus, 4, 0x1f);
    assert_eq!(read(&mut bus, 4), 0x1f);
    inject(&mut bus, 50, 50, 0);
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
    assert!(!bus.device().irq_pending());
    assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
    write(&mut bus, 4, 0x0b);
    bus.tick(STARTUP_CLOCKS + FRAME - 1);
    assert_eq!(read(&mut bus, 5), 0x60);
    bus.tick(1);
    assert_eq!(read(&mut bus, 0), b'M');
    bus.tick(100_000);
    assert_eq!(read(&mut bus, 5), 0x60);
}

#[test]
fn serial_mouse_loopback_masks_out2_for_already_received_data() {
    let mut bus = mouse_bus();
    write(&mut bus, 4, 0x0b);
    bus.tick(STARTUP_CLOCKS + FRAME);
    assert!(bus.device().irq_pending());
    write(&mut bus, 4, 0x1f);
    assert_eq!(read(&mut bus, 2), 4);
    assert!(!bus.device().irq_pending());
    assert_eq!(read(&mut bus, 0), b'M');
    write(&mut bus, 4, 0x0b);
    bus.tick(STARTUP_CLOCKS + FRAME);
    assert_eq!(read(&mut bus, 0), b'M');
}

#[test]
fn serial_mouse_receiver_accepts_7n2_at_the_7n1_source_rate() {
    let mut bus = mouse_bus();
    write(&mut bus, 3, 6);
    write(&mut bus, 4, 0x0b);
    bus.tick(STARTUP_CLOCKS + FRAME);
    assert_eq!(read(&mut bus, 5), 0x61);
    assert_eq!(read(&mut bus, 0), b'M');
    inject(&mut bus, -1, 1, 3);
    assert_eq!(packet(&mut bus), [0x73, 63, 1]);
}

#[test]
fn serial_mouse_raise_then_program_samples_line_settings_at_the_start_bit() {
    let mut bus = MockBus::new(Box::new(
        Uart16550::new(BASE).with_serial_device(Box::new(SerialMouse::new())),
    ));
    write(&mut bus, 4, 0x0b);
    bus.tick(STARTUP_CLOCKS - 100);
    for (offset, value) in [(3, 0x80), (0, 96), (1, 0), (3, 2)] {
        write(&mut bus, offset, value);
    }
    bus.tick(99);
    assert_eq!(read(&mut bus, 5), 0x60);
    bus.tick(1);
    // After the start bit, changing the receiver cannot reinterpret this frame.
    write(&mut bus, 3, 3);
    bus.tick(FRAME);
    assert_eq!(read(&mut bus, 5), 0x61);
    assert_eq!(read(&mut bus, 0), b'M');
}

#[test]
fn serial_mouse_restore_rejects_mcr_line_mismatches_without_mutation() {
    for active in [false, true] {
        let mut bus = mouse_bus();
        if active {
            identify(&mut bus);
        }
        let saved = bus.device().save();
        for bit in [1, 2, 0x10] {
            if !active && bit == 0x10 {
                continue;
            }
            let mut corrupt = saved.clone();
            corrupt[4] ^= bit;
            assert!(
                bus.device_mut().restore(&corrupt).is_err(),
                "active={active}, bit={bit}"
            );
            assert_eq!(bus.device().save(), saved);
        }
    }
}

#[test]
fn serial_mouse_snapshot_preserves_start_bit_sampling_and_loopback_power_state() {
    for clocks in [STARTUP_CLOCKS - 1, STARTUP_CLOCKS, STARTUP_CLOCKS + 1] {
        let mut original = mouse_bus();
        write(&mut original, 4, 0x0b);
        original.tick(clocks);
        let saved = original.device().save();
        let mut restored = mouse_bus();
        restored.device_mut().restore(&saved).unwrap();
        if clocks >= STARTUP_CLOCKS {
            for bus in [&mut original, &mut restored] {
                write(bus, 3, 3);
            }
        }
        for bus in [&mut original, &mut restored] {
            bus.device_mut().sync(VClock(STARTUP_CLOCKS + FRAME));
            assert_eq!(read(bus, 5), 0x61);
            assert_eq!(read(bus, 0), b'M');
        }
        assert_eq!(original.device().save(), restored.device().save());
        write(&mut original, 4, 0x1f);
        let saved = original.device().save();
        restored.device_mut().restore(&saved).unwrap();
        assert_eq!(restored.device().save(), saved);
        for bus in [&mut original, &mut restored] {
            write(bus, 3, 2);
            write(bus, 4, 0x0b);
            bus.device_mut().sync(VClock(2 * (STARTUP_CLOCKS + FRAME)));
            assert_eq!(read(bus, 0), b'M');
        }
    }
}
