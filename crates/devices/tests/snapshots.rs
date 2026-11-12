// SPDX-License-Identifier: AGPL-3.0-or-later
//! Snapshot continuation and hostile-input checks on isolated mock buses.

use device::{Device, DeviceError, VClock};
use devices::*;
use testkit::MockBus;

fn exercise(bus: &mut MockBus, ports: &[u16], at: u64) {
    bus.device_mut().sync(VClock(at));
    bus.device().next_event(VClock(at));
    bus.device().irq_pending();
    for &port in ports {
        bus.port_read(port, 1).unwrap();
        bus.port_write(port, 1, 0).unwrap();
    }
    if ports.contains(&0x3D4) {
        bus.mmio_read(0xBFFFF, 4).unwrap();
        bus.mmio_write(0xBFFFF, 4, 0x1234).unwrap();
    }
    if ports.contains(&0x3B4) {
        bus.mmio_read(0xB0000, 4).unwrap();
        bus.mmio_write(0xB0000, 4, 0x1234).unwrap();
    }
    if ports.contains(&0x3F5) {
        let request = bus.device_mut().take_dma_request();
        if let Some(request) = request {
            let outcome = match request.op {
                DmaOp::ToMemory(bytes) => device::DmaOutcome::ToMemory {
                    transferred: bytes.len(),
                    terminal_count: true,
                },
                DmaOp::FromMemory { len } => device::DmaOutcome::FromMemory {
                    bytes: vec![0; len],
                    terminal_count: true,
                },
            };
            bus.device_mut().dma_complete(outcome);
        }
        bus.device_mut().dma_complete(device::DmaOutcome::ToMemory {
            transferred: 0,
            terminal_count: false,
        });
        bus.device_mut()
            .dma_complete(device::DmaOutcome::FromMemory {
                bytes: vec![],
                terminal_count: false,
            });
        for _ in 0..12 {
            bus.port_write(0x3F5, 1, 0).unwrap();
            bus.port_read(0x3F5, 1).unwrap();
        }
    }
}

fn continuation(make: fn() -> Box<dyn Device>, ports: &[u16]) {
    let mut original = MockBus::new(make());
    let original_output = device::ByteCapture::new(256);
    if let Some(output) = original.device_mut().byte_output() {
        output.bind_sink(Box::new(original_output.clone()));
    }
    exercise(&mut original, ports, 137);
    original.device_mut().sync(VClock(149));
    let blob = original.device().save();
    let mut restored = MockBus::new(make());
    let restored_output = device::ByteCapture::new(256);
    if let Some(output) = restored.device_mut().byte_output() {
        output.bind_sink(Box::new(restored_output.clone()));
    }
    original_output.start_epoch();
    restored.device_mut().restore(&blob).unwrap();
    assert_eq!(restored.device().save(), blob);
    assert!(restored_output.drain().unwrap().is_empty());
    original.device_mut().take_trace();
    // Access before another tick also checks the restored trace timestamp.
    for &port in ports {
        assert_eq!(original.port_read(port, 1), restored.port_read(port, 1));
    }
    assert_eq!(
        original.device_mut().take_trace(),
        restored.device_mut().take_trace()
    );
    for at in [150, 151, 167, 1493] {
        exercise(&mut original, ports, at);
        exercise(&mut restored, ports, at);
        assert_eq!(original.device().save(), restored.device().save());
        assert_eq!(original_output.drain(), restored_output.drain());
        assert_eq!(
            original.device().irq_pending(),
            restored.device().irq_pending()
        );
        assert_eq!(
            original.device_mut().take_trace(),
            restored.device_mut().take_trace()
        );
    }
}

fn sweep(make: fn() -> Box<dyn Device>, ports: &[u16]) {
    let mut source = MockBus::new(make());
    exercise(&mut source, ports, 137);
    let valid = source.device().save();
    sweep_blob(make, ports, &valid);
}

fn sweep_blob(make: fn() -> Box<dyn Device>, ports: &[u16], valid: &[u8]) {
    let mut bus = MockBus::new(make());
    for length in 0..valid.len() {
        assert!(
            matches!(
                bus.device_mut().restore(&valid[..length]),
                Err(DeviceError::InvalidSnapshot { .. })
            ),
            "accepted truncation {length}"
        );
    }
    let mut extra = valid.to_vec();
    extra.push(0);
    assert!(matches!(
        bus.device_mut().restore(&extra),
        Err(DeviceError::InvalidSnapshot { .. })
    ));
    for index in 0..valid.len() {
        for value in [0, 1, 0x7F, 0xFF] {
            let mut blob = valid.to_vec();
            blob[index] = value;
            bus.device_mut().restore(valid).unwrap();
            match bus.device_mut().restore(&blob) {
                Ok(()) => {
                    assert!(index != 0 || value == valid[0], "accepted wrong version");
                    // Mutated snapshots may carry any absolute clock value.
                    exercise(&mut bus, ports, u64::MAX);
                }
                Err(DeviceError::InvalidSnapshot { .. }) => assert_eq!(
                    bus.device().save(),
                    valid,
                    "failed restore mutated state at {index}"
                ),
                Err(error) => panic!("unexpected restore error: {error}"),
            }
        }
    }
}

macro_rules! checks {
    ($roundtrip:ident, $sweep:ident, $make:expr, $ports:expr) => {
        #[test]
        fn $roundtrip() {
            continuation($make, $ports);
        }
        #[test]
        fn $sweep() {
            sweep($make, $ports);
        }
    };
}

checks!(
    pit_continuation,
    pit_sweep,
    || Box::new(Pit8253::new()),
    &[0x40, 0x41, 0x42, 0x43]
);
checks!(
    rtc_continuation,
    rtc_sweep,
    || Box::new(CmosRtc::new().with_timing(100, 3)),
    &[0x70, 0x71]
);
checks!(
    pic_continuation,
    pic_sweep,
    || Box::new(Pic8259::new()),
    &[0x20, 0x21]
);
checks!(
    keyboard_continuation,
    keyboard_sweep,
    || Box::new(I8042::new()),
    &[0x60, 0x64]
);
checks!(
    dma_continuation,
    dma_sweep,
    || Box::new(Dma8237::new()),
    &[0, 1, 8, 9, 10, 11, 12, 13, 14, 15, 0x81]
);
checks!(
    fdc_continuation,
    fdc_sweep,
    || Box::new(Upd765::new()),
    &[0x3F2, 0x3F4, 0x3F5, 0x3F7]
);
checks!(
    uart_continuation,
    uart_sweep,
    || Box::new(Uart16550::new(0x3F8)),
    &[0x3F8, 0x3F9, 0x3FA, 0x3FB, 0x3FC, 0x3FF]
);
checks!(
    mda_continuation,
    mda_sweep,
    || Box::new(MdaText::new()),
    &[0x3B4, 0x3B5, 0x3B8, 0x3BA]
);
checks!(
    vga_continuation,
    vga_sweep,
    || Box::new(VgaText::new()),
    &[0x3D4, 0x3D5, 0x3DA]
);
checks!(
    post_continuation,
    post_sweep,
    || Box::new(PostPort::new()),
    &[0x190]
);
checks!(
    system_continuation,
    system_sweep,
    || Box::new(SysControl::new()),
    &[0x61]
);

#[test]
fn uart_output_mebibyte_does_not_grow_device_state() {
    use std::{cell::Cell, rc::Rc};

    struct CheckingSink(Rc<Cell<usize>>);
    impl device::ByteSink for CheckingSink {
        fn push(&mut self, byte: u8) {
            let count = self.0.get();
            assert_eq!(byte, count as u8);
            self.0.set(count + 1);
        }
    }

    const BYTES: usize = 1024 * 1024;
    for mode in 0..3 {
        let mut bus = MockBus::new(Box::new(Uart16550::new(0x3F8)));
        let count = Rc::new(Cell::new(0));
        let capture = device::ByteCapture::new(3);
        match mode {
            0 => {}
            1 => bus
                .device_mut()
                .byte_output()
                .unwrap()
                .bind_sink(Box::new(CheckingSink(count.clone()))),
            _ => bus
                .device_mut()
                .byte_output()
                .unwrap()
                .bind_sink(Box::new(capture.clone())),
        }
        let before = bus.device().save();
        for index in 0..BYTES {
            bus.port_write(0x3F8, 1, u32::from(index as u8)).unwrap();
        }
        let after = bus.device().save();
        assert_eq!(after.len(), before.len(), "sink mode {mode}");
        assert_eq!(after, before, "transmit history is not device state");
        assert!(bus.device_mut().take_trace().is_empty());
        assert_eq!(bus.device().trace_dropped(), 0);
        if mode == 1 {
            assert_eq!(count.get(), BYTES);
        } else if mode == 2 {
            assert_eq!(capture.buffered_len(), 3);
            assert_eq!(capture.drain(), Err(device::ByteOverflow { limit: 3 }));
        }
    }
}

#[test]
fn uart_output_legacy_snapshots_are_rejected_without_mutation() {
    let mut bus = MockBus::new(Box::new(Uart16550::new(0x3F8)));
    bus.port_write(0x3FF, 1, 0xA5).unwrap();
    bus.tick(123);
    let before = bus.device().save();
    for version in [1, 2] {
        let mut legacy = vec![version, 1, 2, 3, 4, 5, 6, 7];
        legacy.extend_from_slice(&3u32.to_le_bytes());
        legacy.extend_from_slice(&42u64.to_le_bytes());
        legacy.extend_from_slice(b"old");
        assert!(matches!(
            bus.device_mut().restore(&legacy),
            Err(DeviceError::InvalidSnapshot {
                reason: "unsupported snapshot version",
                ..
            })
        ));
        assert_eq!(bus.device().save(), before);
    }
    assert_eq!(&before[..8], &[3, 0, 0, 0, 0, 0xA5, 0, 0]);
    assert_eq!(&before[8..], &123u64.to_le_bytes());
    for length in 0..before.len() {
        assert!(matches!(
            bus.device_mut().restore(&before[..length]),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(bus.device().save(), before);
    }
    let mut trailing = before.clone();
    trailing.push(0);
    assert!(matches!(
        bus.device_mut().restore(&trailing),
        Err(DeviceError::InvalidSnapshot { .. })
    ));
    assert_eq!(bus.device().save(), before);
}

#[test]
fn rtc_rejects_invalid_timing() {
    let mut rtc = CmosRtc::new();
    let valid = rtc.save();
    // Version-independent offsets preceding the appended time base.
    for (offset, value) in [(82, 0u64), (90, u64::MAX), (74, u64::MAX)] {
        let mut blob = valid.clone();
        blob[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        assert!(matches!(
            rtc.restore(&blob),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(rtc.save(), valid);
    }
    for scale in [0, 2, 4, 255] {
        let mut blob = valid.clone();
        blob[108] = scale;
        assert!(matches!(
            rtc.restore(&blob),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(rtc.save(), valid);
    }
}

#[test]
fn rtc_rejects_second_shorter_than_one_pit_clock() {
    let mut bus = MockBus::new(Box::new(CmosRtc::new()));
    let valid = bus.device().save();
    let mut blob = valid.clone();
    blob[82..90].copy_from_slice(&1u64.to_le_bytes());
    blob[90..98].copy_from_slice(&0u64.to_le_bytes());
    blob[108] = 3;
    assert_eq!(
        bus.device_mut().restore(&blob),
        Err(DeviceError::InvalidSnapshot {
            device: "mc146818-rtc",
            reason: "invalid clock state",
        })
    );
    assert_eq!(bus.device().save(), valid);
    blob[108] = 1;
    bus.device_mut().restore(&blob).unwrap();
    assert_eq!(bus.device().save(), blob);
}

#[test]
fn rtc_reports_snapshot_version_before_length() {
    let mut bus = MockBus::new(Box::new(CmosRtc::new()));
    let valid = bus.device().save();
    let mut version_three = valid.clone();
    version_three.remove(108);
    version_three[0] = 3;
    assert_eq!(version_three.len(), 116);
    for blob in [&version_three[..], &[3][..]] {
        assert_eq!(
            bus.device_mut().restore(blob),
            Err(DeviceError::InvalidSnapshot {
                device: "mc146818-rtc",
                reason: "unsupported snapshot version",
            })
        );
        assert_eq!(bus.device().save(), valid);
    }
    assert_eq!(
        bus.device_mut().restore(&[]),
        Err(DeviceError::InvalidSnapshot {
            device: "mc146818-rtc",
            reason: "unexpected snapshot length",
        })
    );
    assert_eq!(bus.device().save(), valid);
}

#[test]
fn pic_rejects_overflowing_vector_base() {
    let mut pic = Pic8259::new();
    let mut blob = pic.save();
    blob[4] = 0xFF;
    assert!(matches!(
        pic.restore(&blob),
        Err(DeviceError::InvalidSnapshot { .. })
    ));
}

#[test]
fn fdc_rejects_inconsistent_command_phase() {
    let mut fdc = Upd765::new();
    let mut blob = fdc.save();
    blob[2] = 1;
    blob[10] = 1;
    assert!(matches!(
        fdc.restore(&blob),
        Err(DeviceError::InvalidSnapshot { .. })
    ));
}

#[test]
fn running_pit_continues_all_modes_and_partial_latches() {
    for mode in [0, 2, 3] {
        let mut source = MockBus::new(Box::new(Pit8253::new()));
        source.device_mut().sync(VClock(100));
        source.port_write(0x43, 1, 0x30 | mode << 1).unwrap();
        source.port_write(0x40, 1, 13).unwrap();
        source.port_write(0x40, 1, 0).unwrap();
        source.device_mut().sync(VClock(105));
        source.port_write(0x43, 1, 0).unwrap();
        source.port_read(0x40, 1).unwrap();
        let blob = source.device().save();
        let mut restored = MockBus::new(Box::new(Pit8253::new()));
        restored.device_mut().restore(&blob).unwrap();
        source.device_mut().take_trace();
        for clock in 105..180 {
            for bus in [&mut source, &mut restored] {
                bus.device_mut().sync(VClock(clock));
            }
            assert_eq!(
                source.device().next_event(VClock(clock)),
                restored.device().next_event(VClock(clock))
            );
            assert_eq!(source.port_read(0x40, 1), restored.port_read(0x40, 1));
            assert_eq!(
                source.device().irq_pending(),
                restored.device().irq_pending()
            );
            assert_eq!(source.device().save(), restored.device().save());
            assert_eq!(
                source.device_mut().take_trace(),
                restored.device_mut().take_trace()
            );
        }
        sweep_blob(|| Box::new(Pit8253::new()), &[0x40, 0x43], &blob);
    }
}

#[test]
fn running_rtc_continues_through_uip_alarm_and_update() {
    let mut source = MockBus::new(Box::new(CmosRtc::new().with_timing(100, 10)));
    for (index, value) in [(0x0B, 0x36), (1, 0xC0), (3, 0xC0), (5, 0xC0)] {
        source.port_write(0x70, 1, index).unwrap();
        source.port_write(0x71, 1, value).unwrap();
    }
    source.device_mut().sync(VClock(289));
    let blob = source.device().save();
    let mut restored = MockBus::new(Box::new(CmosRtc::new()));
    restored.device_mut().restore(&blob).unwrap();
    sweep_blob(|| Box::new(CmosRtc::new()), &[0x70, 0x71], &blob);
    source.device_mut().take_trace();
    for clock in 289..410 {
        for bus in [&mut source, &mut restored] {
            bus.device_mut().sync(VClock(clock));
        }
        assert_eq!(
            source.device().next_event(VClock(clock)),
            restored.device().next_event(VClock(clock))
        );
        for index in [0, 0x0A, 0x0C] {
            for bus in [&mut source, &mut restored] {
                bus.port_write(0x70, 1, index).unwrap();
            }
            assert_eq!(source.port_read(0x71, 1), restored.port_read(0x71, 1));
        }
        assert_eq!(source.device().save(), restored.device().save());
        assert_eq!(
            source.device_mut().take_trace(),
            restored.device_mut().take_trace()
        );
    }
}

#[test]
fn default_rtc_snapshot_preserves_thirds_and_timing_in_an_integer_device() {
    for saved_at in [1_193_182, 2_386_364, 3_578_352] {
        let mut source = MockBus::new(Box::new(CmosRtc::new()));
        source.port_write(0x70, 1, 0x0A).unwrap();
        source.port_write(0x71, 1, 0x26).unwrap();
        source.device_mut().sync(VClock(saved_at));
        let blob = source.device().save();
        let mut restored = MockBus::new(Box::new(CmosRtc::new().with_timing(64, 8)));
        restored.device_mut().restore(&blob).unwrap();
        source.device_mut().take_trace();
        for clock in [
            saved_at,
            saved_at + 1,
            3_579_544,
            3_579_545,
            4_771_534,
            4_772_727,
        ] {
            for bus in [&mut source, &mut restored] {
                bus.device_mut().sync(VClock(clock));
            }
            assert_eq!(
                source.device().next_event(VClock(clock)),
                restored.device().next_event(VClock(clock))
            );
            for index in [0, 0x0A, 0x0C] {
                for bus in [&mut source, &mut restored] {
                    bus.port_write(0x70, 1, index).unwrap();
                }
                assert_eq!(source.port_read(0x71, 1), restored.port_read(0x71, 1));
            }
            assert_eq!(source.device().save(), restored.device().save());
            assert_eq!(
                source.device_mut().take_trace(),
                restored.device_mut().take_trace()
            );
        }
    }
}

#[test]
fn keyboard_continues_pending_frame_into_fresh_device() {
    let mut keyboard = I8042::new();
    keyboard.sync(VClock(100));
    keyboard.push_scancode(0x1E);
    keyboard.push_scancode(0x9E);
    let mut source = MockBus::new(Box::new(keyboard));
    source.port_read(0x60, 1).unwrap();
    source.device_mut().sync(VClock(300));
    let blob = source.device().save();
    let mut restored = MockBus::new(Box::new(I8042::new()));
    restored.device_mut().restore(&blob).unwrap();
    source.device_mut().take_trace();
    for clock in [300, 301, 1412, 1413, 1414] {
        for bus in [&mut source, &mut restored] {
            bus.device_mut().sync(VClock(clock));
        }
        assert_eq!(
            source.device().next_event(VClock(clock)),
            restored.device().next_event(VClock(clock))
        );
        assert_eq!(source.port_read(0x64, 1), restored.port_read(0x64, 1));
        assert_eq!(
            source.device().irq_pending(),
            restored.device().irq_pending()
        );
        assert_eq!(source.port_read(0x60, 1), restored.port_read(0x60, 1));
        assert_eq!(source.device().save(), restored.device().save());
        assert_eq!(
            source.device_mut().take_trace(),
            restored.device_mut().take_trace()
        );
    }
}

#[test]
fn pic_preserves_held_input_edge_detection() {
    use device::InterruptController;
    let mut source = Pic8259::new();
    source.set_irq_line(7, true);
    assert_eq!(source.acknowledge(), Some(7));
    source.end_of_interrupt();
    let mut restored = Pic8259::new();
    restored.restore(&source.save()).unwrap();
    for pic in [&mut source, &mut restored] {
        pic.set_irq_line(7, true);
    }
    assert_eq!(source.poll(), restored.poll());
    assert_eq!(restored.poll(), None);
    for pic in [&mut source, &mut restored] {
        pic.set_irq_line(7, false);
        pic.set_irq_line(7, true);
        assert_eq!(pic.acknowledge(), Some(7));
    }
}

fn mounted_fdc() -> Box<dyn Device> {
    Box::new(Upd765::with_image(vec![0xA5; 512]))
}

#[test]
fn fdc_reset_poll_supersession_continues_at_every_byte_boundary() {
    for (polls, completion, status, cylinder) in [
        (1, vec![3, 0xDF, 2, 7, 0], 0x20, 0),
        (2, vec![0x0F, 1, 5], 0x21, 5),
        (0, vec![0x0F, 0, 9, 0x0F, 1, 5], 0x21, 5),
        (4, vec![0x0F, 0, 9, 0x0F, 1, 5], 0x21, 5),
    ] {
        // None writes a command byte; Some reads the expected result byte.
        let mut actions = Vec::new();
        for unit in 0..polls {
            actions.extend([(8, None), (0, Some(0xC0 | unit)), (0, Some(0))]);
        }
        actions.extend(completion.into_iter().map(|byte| (byte, None)));
        actions.extend([
            (8, None),
            (0, Some(status)),
            (0, Some(cylinder)),
            (8, None),
            (0, Some(0x80)),
        ]);
        for boundary in 0..=actions.len() {
            let mut source = MockBus::new(mounted_fdc());
            source.port_write(0x3F2, 1, 0x0C).unwrap();
            for &(byte, expected) in &actions[..boundary] {
                if let Some(expected) = expected {
                    assert_eq!(source.port_read(0x3F5, 1).unwrap(), expected);
                } else {
                    source.port_write(0x3F5, 1, byte).unwrap();
                }
            }
            let blob = source.device().save();
            assert_eq!(blob[0], 4);
            let mut restored = MockBus::new(mounted_fdc());
            restored.device_mut().restore(&blob).unwrap();
            assert_eq!(restored.device().save(), blob);
            source.device_mut().take_trace();
            for &(byte, expected) in &actions[boundary..] {
                for bus in [&mut source, &mut restored] {
                    if let Some(expected) = expected {
                        assert_eq!(bus.port_read(0x3F5, 1).unwrap(), expected);
                    } else {
                        bus.port_write(0x3F5, 1, byte).unwrap();
                    }
                }
                assert_eq!(source.device().save(), restored.device().save());
                assert_eq!(
                    source.device_mut().take_trace(),
                    restored.device_mut().take_trace()
                );
            }
        }
    }
}

#[test]
fn fdc_continues_each_command_prefix_result_and_dma_phase() {
    use device::{DmaOp, DmaOutcome};
    for opcode in [3, 4, 7, 8, 0x0F, 0x4A, 0x46, 0x45] {
        let bytes: &[u32] = match opcode {
            3 => &[3, 0, 0],
            4 => &[4, 0],
            7 => &[7, 0],
            8 => &[8],
            0x0F => &[0x0F, 0, 42],
            0x4A => &[0x4A, 0],
            0x46 => &[0x46, 0, 0, 0, 1, 2, 1, 0, 0],
            _ => &[0x45, 0, 0, 0, 1, 2, 1, 0, 0],
        };
        for prefix in 0..=bytes.len() {
            for request_taken in [false, true] {
                let mut source = MockBus::new(mounted_fdc());
                source.device_mut().sync(VClock(251));
                source.port_write(0x3F2, 1, 0x0C).unwrap();
                for &byte in &bytes[..prefix] {
                    source.port_write(0x3F5, 1, byte).unwrap();
                }
                let taken = if request_taken {
                    source.device_mut().take_dma_request()
                } else {
                    None
                };
                let blob = source.device().save();
                sweep_blob(mounted_fdc, &[0x3F4, 0x3F5], &blob);
                let mut restored = MockBus::new(mounted_fdc());
                restored.device_mut().restore(&blob).unwrap();
                source.device_mut().take_trace();
                for &byte in &bytes[prefix..] {
                    for bus in [&mut source, &mut restored] {
                        bus.port_write(0x3F5, 1, byte).unwrap();
                    }
                }
                let request = source.device_mut().take_dma_request();
                assert_eq!(request, restored.device_mut().take_dma_request());
                if let Some(request) = request.or(taken) {
                    let outcome = match request.op {
                        DmaOp::ToMemory(bytes) => DmaOutcome::ToMemory {
                            transferred: bytes.len(),
                            terminal_count: true,
                        },
                        DmaOp::FromMemory { len } => DmaOutcome::FromMemory {
                            bytes: vec![0x5A; len],
                            terminal_count: true,
                        },
                    };
                    source.device_mut().dma_complete(outcome.clone());
                    restored.device_mut().dma_complete(outcome);
                }
                for _ in 0..7 {
                    assert_eq!(source.port_read(0x3F5, 1), restored.port_read(0x3F5, 1));
                }
                assert_eq!(source.device().save(), restored.device().save());
                assert_eq!(
                    source.device_mut().take_trace(),
                    restored.device_mut().take_trace()
                );
            }
        }
    }
}

#[test]
fn rtc_rejects_invalid_periodic_phase_without_mutation() {
    let mut bus = MockBus::new(Box::new(CmosRtc::new()));
    let valid = bus.device().save();
    for (offset, bytes) in [
        (98, u64::MAX.to_le_bytes().to_vec()),
        (106, 32768u16.to_le_bytes().to_vec()),
    ] {
        let mut blob = valid.clone();
        blob[offset..offset + bytes.len()].copy_from_slice(&bytes);
        assert!(matches!(
            bus.device_mut().restore(&blob),
            Err(DeviceError::InvalidSnapshot { .. })
        ));
        assert_eq!(bus.device().save(), valid);
    }
}

#[test]
fn pit_rejects_unpaired_write_lsb_without_mutation() {
    let mut bus = MockBus::new(Box::new(Pit8253::new()));
    let valid = bus.device().save();
    let mut blob = valid.clone();
    blob[12] = 0x55;
    assert!(matches!(
        bus.device_mut().restore(&blob),
        Err(DeviceError::InvalidSnapshot { .. })
    ));
    assert_eq!(bus.device().save(), valid);
}

#[test]
fn vga_rejects_out_of_range_index_without_mutation() {
    let mut bus = MockBus::new(Box::new(VgaText::new()));
    let valid = bus.device().save();
    let mut blob = valid.clone();
    blob[1] = 0x20;
    assert!(matches!(
        bus.device_mut().restore(&blob),
        Err(DeviceError::InvalidSnapshot { .. })
    ));
    assert_eq!(bus.device().save(), valid);
}

#[test]
#[cfg(not(debug_assertions))]
fn restored_devices_ignore_backwards_clock_without_replaying_time() {
    let devices: Vec<Box<dyn Device>> = vec![
        Box::new(Pit8253::new()),
        Box::new(CmosRtc::new().with_timing(100, 3)),
        Box::new(Pic8259::new()),
        Box::new(I8042::new()),
        Box::new(Dma8237::new()),
        Box::new(Upd765::new()),
        Box::new(Uart16550::new(0x3F8)),
        Box::new(VgaText::new()),
        Box::new(MdaText::new()),
        Box::new(PostPort::new()),
        Box::new(SysControl::new()),
    ];
    for device in devices {
        let mut bus = MockBus::new(device);
        bus.device_mut().sync(VClock(149));
        let snapshot = bus.device().save();
        bus.device_mut().restore(&snapshot).unwrap();
        bus.device_mut().sync(VClock(100));
        assert_eq!(
            bus.device().save(),
            snapshot,
            "{} rewound on sync",
            bus.device().name()
        );
        bus.device_mut().tick(VClock(101));
        bus.device_mut().sync(VClock(149));
        assert_eq!(
            bus.device().save(),
            snapshot,
            "{} replayed time",
            bus.device().name()
        );
    }
}

#[test]
fn crafted_timer_snapshots_survive_large_time_jumps() {
    let mut pit = MockBus::new(Box::new(Pit8253::new()));
    pit.port_write(0x43, 1, 0x36).unwrap();
    pit.port_write(0x40, 1, 3).unwrap();
    pit.port_write(0x40, 1, 0).unwrap();
    let mut rtc = MockBus::new(Box::new(CmosRtc::new().with_timing(1, 0)));
    for bus in [&mut pit, &mut rtc] {
        let mut blob = bus.device().save();
        // Absolute time is the final field in both timer formats. A restored
        // early epoch must tolerate a jump spanning almost the whole clock.
        let clock = blob.len() - 8;
        blob[clock..].copy_from_slice(&1u64.to_le_bytes());
        bus.device_mut().restore(&blob).unwrap();
        bus.device_mut().sync(VClock(u64::MAX - 1));
        let final_state = bus.device().save();
        bus.device_mut().restore(&final_state).unwrap();
        bus.device_mut().sync(VClock(u64::MAX - 1));
        assert_eq!(bus.device().save(), final_state);
        assert_ne!(final_state, blob);
    }
}

checks!(
    ata_continuation,
    ata_sweep,
    mounted_ata,
    &[0x1F1, 0x1F2, 0x1F3, 0x1F4, 0x1F5, 0x1F6, 0x1F7, 0x3F6]
);

fn mounted_ata() -> Box<dyn Device> {
    let geometry = AtaGeometry::new(8, 2, 4).unwrap();
    let drive = AtaDrive::new(geometry, vec![0xA5; 64 * 512]).unwrap();
    Box::new(Ata::with_drives([Some(drive), None]).unwrap())
}

#[test]
fn ata_translated_mid_transfer_snapshots_continue_and_reject_malformed_state() {
    for (opcode, prefix, fault) in [
        (0xC4, 1, false),
        (0xC4, 256, false),
        (0xC4, 1025, false),
        (0xC5, 1, false),
        (0xC5, 256, false),
        (0xC5, 1025, false),
        (0xC4, 1, true),
        (0xC4, 257, true),
        (0xC5, 257, true),
    ] {
        let prepare = || {
            let mut bus = MockBus::new(mounted_ata());
            for (port, value) in [(0x1F6, 0xA3), (0x1F2, 2), (0x1F7, 0x91)] {
                bus.port_write(port, 1, value).unwrap();
            }
            assert_eq!(bus.port_read(0x1F7, 1).unwrap(), 0x50);
            bus.port_write(0x1F7, 1, 0xEC).unwrap();
            let words: Vec<_> = (0..256).map(|_| bus.port_read(0x1F0, 2).unwrap()).collect();
            assert_eq!(&words[54..59], &[8, 4, 2, 64, 0]);
            for (port, value) in [
                (0x1F2, 4),
                (0x1F7, 0xC6),
                (0x1F6, if fault { 0xE0 } else { 0xA1 }),
                (0x1F2, if fault { 4 } else { 6 }),
                (0x1F3, if fault { 63 } else { 1 }),
                (0x1F4, 0),
                (0x1F5, 0),
                (0x1F7, opcode),
            ] {
                bus.port_write(port, 1, value).unwrap();
            }
            for word in 0..prefix {
                if opcode == 0xC5 {
                    bus.port_write(0x1F0, 2, word).unwrap();
                } else {
                    bus.port_read(0x1F0, 2).unwrap();
                }
            }
            bus
        };
        let mut source = prepare();
        source.device_mut().sync(VClock(137));
        let saved = source.device().save();
        sweep_blob(mounted_ata, &[0x1F7, 0x3F6], &saved);
        let mut restored = prepare();
        restored.device_mut().restore(&saved).unwrap();
        assert_eq!(restored.device().save(), saved);
        let total = if fault { 4 } else { 6 } * 256;
        for word in prefix..total {
            if opcode == 0xC5 {
                source.port_write(0x1F0, 2, word).unwrap();
                restored.port_write(0x1F0, 2, word).unwrap();
            } else {
                assert_eq!(source.port_read(0x1F0, 2), restored.port_read(0x1F0, 2));
            }
            assert_eq!(source.device().save(), restored.device().save());
            assert_eq!(
                source.device().irq_pending(),
                restored.device().irq_pending()
            );
        }
        assert_eq!(
            source.port_read(0x1F7, 1).unwrap(),
            if fault { 0x51 } else { 0x50 }
        );
    }
}

checks!(
    coprocessor_continuation,
    coprocessor_sweep,
    || Box::new(Absent80287::new()),
    &[0xF0, 0xF1, 0xF8, 0xFF]
);
checks!(
    word_dma_continuation,
    word_dma_sweep,
    || Box::new(Dma8237::new_at_word()),
    &[0xC4, 0xC6, 0xD0, 0xD6, 0xD8, 0xDA, 0xDC, 0xDE, 0x8B]
);

#[test]
fn fdc_data_rate_round_trip_preserves_nondefault_and_reserved_selections() {
    for rate in 1..4 {
        let mut original = MockBus::new(mounted_fdc());
        original.port_write(0x3F2, 1, 0x0C).unwrap();
        original.port_write(0x3F7, 1, rate).unwrap();
        let blob = original.device().save();
        assert_eq!(blob[0], 4);
        assert_eq!(blob.last(), Some(&(rate as u8)));
        let mut restored = MockBus::new(mounted_fdc());
        restored.device_mut().restore(&blob).unwrap();
        assert_eq!(restored.device().save(), blob);
        for bus in [&mut original, &mut restored] {
            for byte in [0x4A, 0] {
                bus.port_write(0x3F5, 1, byte).unwrap();
            }
            let result: Vec<_> = (0..7).map(|_| bus.port_read(0x3F5, 1).unwrap()).collect();
            assert_eq!(result, [0x40, 1, 0, 0, 0, 1, 2]);
        }
    }
}

#[test]
fn fdc_data_rate_previous_version_restores_with_500_kbps_default() {
    // Literal v3 idle state: DOR=0Ch; no pending reset polls, IRQ or DMA;
    // PCN=0, no command/results, and trace time zero. V3 has no rate byte.
    let previous = [
        3, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut bus = MockBus::new(mounted_fdc());
    bus.port_write(0x3F7, 1, 2).unwrap();
    bus.device_mut().restore(&previous).unwrap();
    assert_eq!(bus.device().save()[0], 4);
    assert_eq!(bus.device().save().last(), Some(&0));
    for byte in [0x4A, 0] {
        bus.port_write(0x3F5, 1, byte).unwrap();
    }
    let result: Vec<_> = (0..7).map(|_| bus.port_read(0x3F5, 1).unwrap()).collect();
    assert_eq!(result, [0, 0, 0, 0, 0, 1, 2]);
}
