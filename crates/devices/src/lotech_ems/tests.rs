// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use testkit::MockBus;

fn board(port: u16, frame: u16, size: u32) -> MockBus {
    MockBus::new(Box::new(LotechEms::new(port, frame, size).unwrap()))
}

#[test]
fn jumper_decode_aliases_and_write_only_registers() {
    for base in [0x260, 0x264, 0x268, 0x26C] {
        let mut bus = board(base, 0xE000, 2048);
        for page in 0..128u32 {
            bus.port_write(base, 1, page).unwrap();
            bus.mmio_write(0xE0000, 2, u64::from(0x100 + page)).unwrap();
        }
        for alias in 0..64u16 {
            for window in 0..4u16 {
                let port = base + alias * 0x400 + window;
                let page = (alias + window * 16) % 128;
                bus.port_write(port, 1, u32::from(page)).unwrap();
                assert_eq!(bus.port_read(port, 1).unwrap(), 0xFF);
                assert_eq!(
                    bus.mmio_read(0xE0000 + u64::from(window) * 0x4000, 2)
                        .unwrap(),
                    u64::from(0x100 + page)
                );
            }
        }
        for wrong_base in [0x270, 0x240, 0x220, 0x2E0, 0x360, 0x060] {
            for window in 0..4 {
                assert!(bus.port_read(wrong_base + window, 1).is_err());
                assert!(bus.port_write(wrong_base + window, 1, 0).is_err());
            }
        }
        for port in 0x260..=0x26F {
            if port & 0x3FC != base {
                assert!(bus.port_write(port, 1, 0).is_err());
                assert!(bus.port_read(port, 1).is_err());
            }
        }
    }
}

#[test]
fn all_pages_remap_between_every_window_without_aliasing() {
    let mut bus = board(0x260, 0xE000, 2048);
    for page in 0..128u32 {
        bus.port_write(0x260, 1, page).unwrap();
        bus.mmio_write(0xE0000, 2, u64::from(page + 0x100)).unwrap();
        bus.mmio_write(0xE3FFF, 1, u64::from(page)).unwrap();
    }
    for window in 0..4u16 {
        let base = 0xE0000 + u64::from(window) * 0x4000;
        for page in 0..128u32 {
            bus.port_write(0x260 + window, 1, page).unwrap();
            assert_eq!(bus.mmio_read(base, 2).unwrap(), u64::from(page + 0x100));
            assert_eq!(bus.mmio_read(base + 0x3FFF, 1).unwrap(), u64::from(page));
        }
    }
}

#[test]
fn unpopulated_and_high_chip_selects_float_and_drop_writes() {
    for size in [512, 1024, 1536, 2048] {
        let mut bus = board(0x260, 0xE000, size);
        bus.port_write(0x260, 1, 0).unwrap();
        bus.mmio_write(0xE0000, 1, 0x5A).unwrap();
        for page in size / 16..256 {
            for window in 0..4u16 {
                let base = 0xE0000 + u64::from(window) * 0x4000;
                bus.port_write(0x260 + window, 1, page).unwrap();
                assert_eq!(bus.mmio_read(base, 4).unwrap(), 0xFFFFFFFF);
                bus.mmio_write(base, 4, 0x12345678).unwrap();
                assert_eq!(bus.mmio_read(base, 4).unwrap(), 0xFFFFFFFF);
            }
        }
        bus.port_write(0x260, 1, 0).unwrap();
        assert_eq!(bus.mmio_read(0xE0000, 1).unwrap(), 0x5A);
    }
}

#[test]
fn power_on_unmapped_frame_jumpers_and_reset_retention() {
    for frame in [0xC000, 0xD000, 0xE000] {
        let mut bus = board(0x260, frame, 2048);
        let base = u64::from(frame) << 4;
        assert_eq!(&bus.device().save()[5..9], &[0xFF; 4]);
        for window in 0..4u16 {
            let addr = base + u64::from(window) * 0x4000;
            assert_eq!(bus.mmio_read(addr, 1).unwrap(), 0xFF);
            bus.mmio_write(addr, 1, 0x77).unwrap();
            bus.port_write(0x260 + window, 1, u32::from(window))
                .unwrap();
            assert_eq!(bus.mmio_read(addr, 1).unwrap(), 0);
            bus.mmio_write(addr, 1, 0x80 + u64::from(window)).unwrap();
        }
        bus.device_mut().reset();
        for window in 0..4u16 {
            assert_eq!(
                bus.mmio_read(base + u64::from(window) * 0x4000, 1).unwrap(),
                0x80 + u64::from(window)
            );
        }
        for addr in [base - 1, base + FRAME, base + 0xF00000] {
            assert!(bus.mmio_read(addr, 1).is_err());
            assert!(bus.mmio_write(addr, 1, 0).is_err());
        }
        assert_eq!(bus.device().next_event(VClock(900)), NextEvent::Dormant);
        assert!(!bus.device().irq_pending());
    }
}

#[test]
fn wide_access_crosses_page_latches_in_byte_order() {
    let mut bus = board(0x260, 0xE000, 2048);
    bus.port_write(0x260, 1, 127).unwrap();
    bus.port_write(0x261, 1, 0).unwrap();
    bus.mmio_write(0xE3FFE, 4, 0x12345678).unwrap();
    assert_eq!(bus.mmio_read(0xE3FFE, 4).unwrap(), 0x12345678);
    bus.port_write(0x261, 1, 128).unwrap();
    assert_eq!(bus.mmio_read(0xE3FFE, 4).unwrap(), 0xFFFF5678);
    bus.port_write(0x261, 1, 0).unwrap();
    bus.mmio_write(0xE3FFF, 3, 0xABCDEF).unwrap();
    assert_eq!(bus.mmio_read(0xE3FFF, 3).unwrap(), 0xABCDEF);
    assert!(bus.mmio_write(0xEFFFF, 2, 0).is_err());
}

#[test]
fn snapshot_is_atomic_config_bound_and_keeps_trace_policy() {
    let mut bus = board(0x260, 0xE000, 512);
    bus.device_mut()
        .set_trace_policy(device::TracePolicy::Ring { capacity: 2 });
    bus.port_write(0x260, 1, 31).unwrap();
    bus.mmio_write(0xE0000, 1, 0x56).unwrap();
    bus.tick(900);
    let saved = bus.device().save();
    bus.mmio_write(0xE0000, 1, 0x78).unwrap();
    bus.port_write(0x260, 1, 0).unwrap();
    bus.tick(23);
    assert_eq!(bus.device().save()[5], 0);
    assert_eq!(&bus.device().save()[9..17], &923u64.to_le_bytes());
    bus.device_mut().restore(&saved).unwrap();
    assert_eq!(bus.device().save()[5], 31);
    assert_eq!(&bus.device().save()[9..17], &900u64.to_le_bytes());
    bus.device_mut().sync(VClock(900));
    assert_eq!(bus.mmio_read(0xE0000, 1).unwrap(), 0x56);
    assert_eq!(bus.device().save(), saved);
    assert_eq!(bus.device_mut().take_trace().len(), 2);
    assert!(bus.device().trace_dropped() > 0);
    for index in [0, 1, 3] {
        let mut bad = saved.clone();
        bad[index] ^= 1;
        assert!(bus.device_mut().restore(&bad).is_err());
        assert_eq!(bus.device().save(), saved);
    }
    assert!(bus.device_mut().restore(&[]).is_err());
    assert!(board(0x260, 0xE000, 1024)
        .device_mut()
        .restore(&saved)
        .is_err());
}

#[test]
fn frame_handle_shares_latches_pages_and_change_counts() {
    let ems = LotechEms::new(0x260, 0xE000, 512).unwrap();
    let frame = ems.frame_memory();
    let mut bus = MockBus::new(Box::new(ems));
    assert_eq!(frame.latches(), [0xFF; 4]);
    assert_eq!((frame.latch_writes(), frame.restores()), (0, 0));
    for unbacked in [32, 0x7F, 0x80, 0xFF] {
        assert!(frame.page(unbacked).is_none());
        assert!(frame.page_mut(unbacked).is_none());
    }
    assert_eq!(frame.page(31).unwrap().len(), PAGE);

    bus.port_write(0x261, 1, 31).unwrap();
    assert_eq!(frame.latches(), [0xFF, 31, 0xFF, 0xFF]);
    assert_eq!(frame.latch_writes(), 1);
    bus.mmio_write(0xE7FFF, 1, 0xA5).unwrap();
    assert_eq!(frame.page(31).unwrap()[0x3FFF], 0xA5);
    frame.page_mut(31).unwrap()[0] = 0x5A;
    assert_eq!(bus.mmio_read(0xE4000, 1).unwrap(), 0x5A);
    assert!(bus.port_write(0x261, 2, 0).is_err());
    assert_eq!(frame.latch_writes(), 1);

    let saved = bus.device().save();
    bus.port_write(0x261, 1, 0).unwrap();
    assert_eq!(frame.latch_writes(), 2);
    bus.device_mut().restore(&saved).unwrap();
    assert_eq!(frame.restores(), 1);
    assert_eq!(frame.latches(), [0xFF, 31, 0xFF, 0xFF]);
    assert!(bus.device_mut().restore(&[]).is_err());
    assert_eq!(frame.restores(), 1);
    assert_eq!(frame.latch_writes(), 2);
}

#[test]
fn invalid_jumpers_and_populations_are_rejected() {
    for (port, frame, size) in [
        (0x261, 0xE000, 2048),
        (0x260, 0xE001, 2048),
        (0x260, 0xE000, 0),
        (0x260, 0xE000, 513),
    ] {
        assert!(LotechEms::new(port, frame, size).is_err());
    }
}

#[test]
fn non_byte_ports_are_rejected_without_changing_state() {
    let mut bus = board(0x260, 0xE000, 512);
    let saved = bus.device().save();
    for size in [0, 2, 3, 4, 8] {
        assert!(matches!(
            bus.port_read(0x260, size),
            Err(DeviceError::UnmappedRegister {
                kind: "port width",
                ..
            })
        ));
        assert!(matches!(
            bus.port_write(0x260, size, 0),
            Err(DeviceError::UnmappedRegister {
                kind: "port width",
                ..
            })
        ));
        assert_eq!(bus.device().save(), saved);
    }
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "device clock moved backwards")]
fn earlier_clock_panics_in_debug() {
    let mut bus = board(0x260, 0xE000, 512);
    bus.tick(900);
    bus.device_mut().tick(VClock(899));
}

#[test]
#[cfg(not(debug_assertions))]
fn earlier_clock_is_ignored_in_release() {
    let mut bus = board(0x260, 0xE000, 512);
    bus.tick(900);
    let saved = bus.device().save();
    bus.device_mut().tick(VClock(899));
    assert_eq!(bus.device().save(), saved);
}
