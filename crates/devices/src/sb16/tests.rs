// SPDX-License-Identifier: AGPL-3.0-or-later
//! Mock-bus DSP tests against Creative Sound Blaster Series Hardware Programming
//! Guide, §2 reset and IRQ, §3 DMA and §6 DSP commands.

use super::*;
use std::{cell::RefCell, rc::Rc};
use testkit::MockBus;

type Events = Rc<RefCell<Vec<Output>>>;
fn rig() -> (MockBus, Events) {
    let events = Events::default();
    let writer = events.clone();
    let mut dsp = Sb16::new(0x220, 1);
    for index in 0x30..=0x33 {
        dsp.port_write(0x224, 1, index).unwrap();
        dsp.port_write(0x225, 1, 0xF8).unwrap();
    }
    dsp.observe_output(move |event| writer.borrow_mut().push(event));
    (MockBus::new(Box::new(dsp)), events)
}
fn command(bus: &mut MockBus, bytes: &[u8]) {
    for &byte in bytes {
        bus.port_write(0x22C, 1, byte.into()).unwrap();
    }
}
fn at(bus: &mut MockBus, clock: u64) {
    bus.tick(clock - bus.clock().0);
}
fn request(bus: &mut MockBus, bytes: &[u8]) {
    assert_eq!(
        bus.device_mut().take_dma_request(),
        Some(DmaRequest {
            channel: 1,
            op: DmaOp::FromMemory { len: bytes.len() }
        })
    );
    bus.device_mut().dma_complete(DmaOutcome::FromMemory {
        bytes: bytes.to_vec(),
        terminal_count: false,
    });
}
fn frames(events: &Events) -> Vec<(u64, [i16; 2])> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Output::Frame { at, pcm, .. } => Some((at.0, *pcm)),
            _ => None,
        })
        .collect()
}

#[test]
fn reset_edges_clear_pending_replies_and_write_busy() {
    let (mut bus, _) = rig();
    assert_eq!(bus.port_read(0x22C, 1).unwrap(), 0);
    bus.port_write(0x226, 1, 0).unwrap();
    assert_eq!(bus.port_read(0x22E, 1).unwrap(), 0);
    command(&mut bus, &[0xE1]);
    bus.port_write(0x226, 1, 1).unwrap();
    assert_eq!(bus.port_read(0x22C, 1).unwrap(), 0x80);
    assert_eq!(bus.port_read(0x22E, 1).unwrap(), 0);
    command(&mut bus, &[0xE1]);
    at(&mut bus, 3);
    bus.port_write(0x226, 1, 0).unwrap();
    assert_eq!(bus.port_read(0x22C, 1).unwrap(), 0);
    assert_eq!(bus.port_read(0x22E, 1).unwrap(), 0x80);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 0xAA);
    assert_eq!(bus.port_read(0x22E, 1).unwrap(), 0);
    command(&mut bus, &[0xE1]);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 4);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 5);
}

#[test]
fn identification_version_unknown_and_reply_order() {
    let (mut bus, _) = rig();
    command(&mut bus, &[0xE1, 0xE0, 0xA5, 0xFF]);
    for byte in [4, 5, 0x5A] {
        assert_eq!(bus.port_read(0x22A, 1).unwrap(), byte);
    }
    assert_eq!(bus.port_read(0x22E, 1).unwrap(), 0);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 0xFF);
    assert_eq!(bus.port_read(0x22C, 1).unwrap(), 0);
}

#[test]
fn forced_irq_is_level_until_eight_bit_acknowledgement() {
    let (mut bus, _) = rig();
    command(&mut bus, &[0xF2]);
    assert!(bus.device().irq_output(0));
    at(&mut bus, 1000);
    assert!(bus.device().irq_pending());
    bus.port_read(0x22A, 1).unwrap();
    bus.port_read(0x22F, 1).unwrap();
    assert!(bus.device().irq_pending());
    bus.port_read(0x22E, 1).unwrap();
    assert!(!bus.device().irq_pending());
}

#[test]
fn exact_time_constant_and_explicit_rate_periods() {
    for (bytes, clock, numerator, denominator, width) in [
        (vec![0x40, 211, 0x14, 0, 0], 54, 4725, 88, 1),
        (
            vec![0x41, 0xAC, 0x44, 0xC0, 0x20, 1, 0],
            28,
            105_000_000,
            3_880_800,
            2,
        ),
        (vec![0x40, 211, 0xC0, 0x20, 1, 0], 108, 9450, 88, 2),
    ] {
        let (mut bus, events) = rig();
        command(&mut bus, &bytes);
        at(&mut bus, clock);
        request(&mut bus, &[0x80; 2][..width]);
        assert!(events.borrow().contains(&Output::Frame {
            at: VClock(clock),
            numerator,
            denominator,
            pcm: [0; 2]
        }));
    }
}

#[test]
fn single_cycle_fetches_at_exact_boundaries_then_silence() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0x14, 2, 0]);
    for (index, (clock, byte)) in [(150, 0), (299, 0x80), (448, 0xFF)].into_iter().enumerate() {
        at(&mut bus, clock - 1);
        assert!(bus.device_mut().take_dma_request().is_none());
        at(&mut bus, clock);
        request(&mut bus, &[byte]);
        assert_eq!(bus.device().irq_pending(), index == 2);
    }
    assert_eq!(
        frames(&events),
        [(150, [-32768; 2]), (299, [0; 2]), (448, [32512; 2])]
    );
    bus.port_read(0x22E, 1).unwrap();
    at(&mut bus, 597);
    assert!(events.borrow().contains(&Output::Pause(VClock(597))));
    at(&mut bus, 10000);
    assert!(bus.device_mut().take_dma_request().is_none());
    assert!(!bus.device().irq_pending());
}

#[test]
fn pause_resume_preserves_fractional_phase_and_speaker_flag_does_not_mute() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0x14, 2, 0]);
    at(&mut bus, 150);
    request(&mut bus, &[0xFF]);
    at(&mut bus, 201);
    command(&mut bus, &[0xD0]);
    at(&mut bus, 901);
    assert!(bus.device_mut().take_dma_request().is_none());
    command(&mut bus, &[0xD4, 0xD1, 0xD8]);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 0xFF);
    command(&mut bus, &[0xD3, 0xD8]);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 0);
    at(&mut bus, 998);
    assert!(bus.device_mut().take_dma_request().is_none());
    at(&mut bus, 999);
    request(&mut bus, &[0]);
    at(&mut bus, 1148);
    request(&mut bus, &[0x81]);
    assert_eq!(
        frames(&events),
        [
            (150, [32512; 2]),
            (901, [32512; 2]),
            (999, [-32768; 2]),
            (1148, [256; 2])
        ]
    );
}

#[test]
fn direct_dac_holds_unsigned_levels_until_reset() {
    let (mut bus, events) = rig();
    at(&mut bus, 7);
    command(&mut bus, &[0x10, 0]);
    at(&mut bus, 29);
    command(&mut bus, &[0x10, 0xFF]);
    at(&mut bus, 100);
    bus.device_mut().reset();
    assert_eq!(frames(&events), [(7, [-32768; 2]), (29, [32512; 2])]);
    assert!(events.borrow().contains(&Output::Pause(VClock(100))));
    assert!(bus.device_mut().take_dma_request().is_none());
}

struct Memory(Vec<u8>);
impl device::DmaMemory for Memory {
    fn dma_read(&self, addr: u32) -> u8 {
        self.0[addr as usize]
    }
    fn dma_write(&mut self, addr: u32, byte: u8) {
        self.0[addr as usize] = byte;
    }
}

#[test]
fn legacy_and_fifo_auto_init_wrap_real_8237_and_exit_after_current_block() {
    for bytes in [
        vec![0x48, 1, 0, 0x1C],
        vec![0xC6, 0, 1, 0],
        vec![0xC4, 0, 1, 0],
    ] {
        let (mut bus, events) = rig();
        let mut dma = crate::Dma8237::new();
        for (port, value) in [
            (0xC, 0),
            (2, 0),
            (2, 0),
            (3, 1),
            (3, 0),
            (0xB, 0x59),
            (0xA, 1),
        ] {
            dma.port_write(port, 1, value).unwrap();
        }
        let mut memory = Memory(vec![0, 255]);
        command(&mut bus, &[0x41, 0x1F, 0x40]);
        command(&mut bus, &bytes);
        let mut terminal_counts = 0;
        let mut irqs = 0;
        for (i, clock) in [150, 299, 448, 597].into_iter().enumerate() {
            at(&mut bus, clock);
            let request = bus.device_mut().take_dma_request().unwrap();
            let DmaOp::FromMemory { len } = request.op else {
                panic!()
            };
            let mut data = vec![0; len];
            let result = dma.dma_service_read(request.channel, &mut memory, &mut data);
            terminal_counts += usize::from(result.terminal_count);
            assert_eq!(result.transferred, 1);
            bus.device_mut().dma_complete(DmaOutcome::FromMemory {
                bytes: data,
                terminal_count: result.terminal_count,
            });
            if bus.device().irq_pending() {
                irqs += 1;
                bus.port_read(0x22E, 1).unwrap();
            }
            if i == 2 {
                command(&mut bus, &[0xDA]);
            }
        }
        assert_eq!((irqs, terminal_counts), (2, 2));
        assert_eq!(
            frames(&events),
            [
                (150, [-32768; 2]),
                (299, [32512; 2]),
                (448, [-32768; 2]),
                (597, [32512; 2])
            ]
        );
        at(&mut bus, 746);
        assert!(bus.device_mut().take_dma_request().is_none());
        assert!(events.borrow().contains(&Output::Pause(VClock(746))));
    }
}

#[test]
fn signed_stereo_and_partial_dma_keep_channel_order() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0xC0, 0x30, 1, 0]);
    at(&mut bus, 150);
    assert_eq!(
        bus.device_mut().take_dma_request().unwrap().op,
        DmaOp::FromMemory { len: 2 }
    );
    bus.device_mut().dma_complete(DmaOutcome::FromMemory {
        bytes: vec![0x80],
        terminal_count: true,
    });
    request(&mut bus, &[0x7F]);
    assert_eq!(frames(&events), [(150, [-32768, 32512])]);
    assert!(bus.device().irq_pending());
}

#[test]
fn snapshot_round_trip_mid_command_mid_block_and_reset_reply() {
    let (mut bus, _) = rig();
    for bytes in [&[0x41, 0x1F][..], &[0x40, 0x14, 2, 0][..]] {
        command(&mut bus, bytes);
        let snapshot = bus.device().save();
        assert_eq!(&snapshot[..8], b"SB16-MV3");
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&snapshot).unwrap();
        assert_eq!(restored.save(), snapshot);
    }
    at(&mut bus, 150);
    request(&mut bus, &[0x80]);
    command(&mut bus, &[0xD0]);
    let snapshot = bus.device().save();
    let mut restored = Sb16::new(0x220, 1);
    restored.restore(&snapshot).unwrap();
    assert_eq!(restored.save(), snapshot);
    let mut delayed_reset = snapshot.clone();
    delayed_reset[8 + 4 * 8..8 + 5 * 8].copy_from_slice(&200u64.to_le_bytes());
    assert!(restored.restore(&delayed_reset).is_err());
    assert_eq!(restored.save(), snapshot);
    for len in 0..snapshot.len() {
        assert!(restored.restore(&snapshot[..len]).is_err());
        assert_eq!(restored.save(), snapshot);
    }
    bus.port_write(0x226, 1, 1).unwrap();
    let reset = bus.device().save();
    restored.restore(&reset).unwrap();
    assert_eq!(restored.port_read(0x22C, 1).unwrap(), 0x80);
    restored.port_write(0x226, 1, 0).unwrap();
    assert_eq!(restored.port_read(0x22A, 1).unwrap(), 0xAA);
}

#[test]
fn single_cycle_command_waits_for_current_auto_init_block() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0x48, 1, 0, 0x1C]);
    at(&mut bus, 150);
    request(&mut bus, &[0]);
    command(&mut bus, &[0x14, 0, 0]);
    let snapshot = bus.device().save();
    let mut restored = Sb16::new(0x220, 1);
    restored.restore(&snapshot).unwrap();
    assert_eq!(snapshot, restored.save());
    at(&mut bus, 299);
    request(&mut bus, &[0x40]);
    assert!(bus.device().irq_pending());
    bus.port_read(0x22E, 1).unwrap();
    at(&mut bus, 448);
    request(&mut bus, &[0xFF]);
    assert!(bus.device().irq_pending());
    at(&mut bus, 598);
    assert!(bus.device_mut().take_dma_request().is_none());
    assert_eq!(
        frames(&events),
        [(150, [-32768; 2]), (299, [-16384; 2]), (448, [32512; 2])]
    );
}

#[test]
fn empty_dma_retries_next_period_without_counting_or_spinning() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0x14, 0, 0]);
    at(&mut bus, 150);
    assert!(bus.device_mut().take_dma_request().is_some());
    bus.device_mut().dma_complete(DmaOutcome::FromMemory {
        bytes: vec![],
        terminal_count: false,
    });
    assert!(!bus.device().irq_pending());
    assert!(bus.device_mut().take_dma_request().is_none());
    at(&mut bus, 299);
    request(&mut bus, &[0xFF]);
    assert!(bus.device().irq_pending());
    assert_eq!(frames(&events), [(150, [0; 2]), (299, [32512; 2])]);
}

#[test]
fn unsupported_command_arguments_do_not_become_dac_or_dma_commands() {
    let commands = [
        (0x04, 1),
        (0x05, 2),
        (0x08, 1),
        (0x0E, 2),
        (0x0F, 1),
        (0x15, 2),
        (0x16, 2),
        (0x17, 2),
        (0x24, 2),
        (0x38, 1),
        (0x42, 2),
        (0x74, 2),
        (0x75, 2),
        (0x76, 2),
        (0x77, 2),
        (0x80, 2),
        (0xE2, 1),
        (0xE4, 1),
        (0xF9, 1),
        (0xFA, 2),
    ];
    for (opcode, count) in commands.into_iter().chain(
        (0xB0..=0xBF)
            .filter(|opcode| opcode & 9 != 0)
            .map(|opcode| (opcode, 3)),
    ) {
        let (mut bus, events) = rig();
        command(&mut bus, &[opcode]);
        command(&mut bus, &[0x10, 0x14, 0x1C][..count]);
        let snapshot = bus.device().save();
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&snapshot).unwrap();
        command(&mut bus, &[0xE1]);
        assert_eq!(bus.port_read(0x22A, 1).unwrap(), 4, "opcode {opcode:02X}");
        assert_eq!(bus.port_read(0x22A, 1).unwrap(), 5, "opcode {opcode:02X}");
        assert!(
            events.borrow().is_empty(),
            "opcode {opcode:02X}: {:?}",
            events.borrow()
        );
        at(&mut bus, 1000);
        assert!(
            bus.device_mut().take_dma_request().is_none(),
            "opcode {opcode:02X}"
        );
        assert!(!bus.device().irq_pending());
    }
    let (mut bus, _) = rig();
    command(&mut bus, &[0xB6, 0x10]);
    let snapshot = bus.device().save();
    let mut restored = Sb16::new(0x220, 1);
    restored.restore(&snapshot).unwrap();
    for byte in [0x14, 0x1C, 0xE1] {
        restored.port_write(0x22C, 1, byte).unwrap();
    }
    assert_eq!(restored.port_read(0x22A, 1).unwrap(), 4);
    assert_eq!(restored.port_read(0x22A, 1).unwrap(), 5);
}

#[test]
fn chained_single_cycle_blocks_preserve_the_sample_grid_without_pause() {
    for block in [vec![0x14, 1, 0], vec![0xC0, 0, 1, 0]] {
        let (mut bus, events) = rig();
        command(&mut bus, &[0x41, 0x1F, 0x40]);
        command(&mut bus, &block);
        at(&mut bus, 150);
        request(&mut bus, &[0]);
        at(&mut bus, 299);
        request(&mut bus, &[0x40]);
        assert!(bus.device().irq_pending());
        bus.port_read(0x22E, 1).unwrap();
        at(&mut bus, 310);
        command(&mut bus, &block);
        assert!(!events
            .borrow()
            .iter()
            .any(|e| matches!(e, Output::Pause(at) if at.0 > 0)));
        let snapshot = bus.device().save();
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&snapshot).unwrap();
        at(&mut bus, 447);
        assert!(bus.device_mut().take_dma_request().is_none());
        at(&mut bus, 448);
        request(&mut bus, &[0x81]);
        at(&mut bus, 597);
        request(&mut bus, &[0xFF]);
        assert_eq!(
            frames(&events),
            [
                (150, [-32768; 2]),
                (299, [-16384; 2]),
                (448, [256; 2]),
                (597, [32512; 2])
            ]
        );
        assert!(!events
            .borrow()
            .iter()
            .any(|e| matches!(e, Output::Pause(at) if at.0 > 0)));
        at(&mut bus, 746);
        assert!(events.borrow().contains(&Output::Pause(VClock(746))));
    }
}

#[test]
fn live_rate_changes_hold_the_frame_until_the_existing_boundary() {
    for rate in [vec![0x41, 0x3E, 0x80], vec![0x40, 193]] {
        let (mut bus, events) = rig();
        command(&mut bus, &[0x41, 0x1F, 0x40, 0x14, 2, 0]);
        at(&mut bus, 150);
        request(&mut bus, &[0xFF]);
        at(&mut bus, 200);
        command(&mut bus, &rate);
        assert!(!events
            .borrow()
            .iter()
            .any(|e| matches!(e, Output::Pause(at) if at.0 > 0)));
        let snapshot = bus.device().save();
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&snapshot).unwrap();
        at(&mut bus, 298);
        assert!(bus.device_mut().take_dma_request().is_none());
        at(&mut bus, 299);
        request(&mut bus, &[0x81]);
        let third = if rate[0] == 0x41 { 374 } else { 375 };
        at(&mut bus, third);
        request(&mut bus, &[0]);
        assert_eq!(
            frames(&events),
            [(150, [32512; 2]), (299, [256; 2]), (third, [-32768; 2])]
        );
    }
}

#[test]
fn single_cycle_lengths_do_not_replace_the_legacy_auto_init_block_size() {
    for block in [
        vec![0x14, 15, 0],
        vec![0xC0, 0, 15, 0],
        vec![0xC4, 0, 15, 0],
    ] {
        let (mut bus, _) = rig();
        command(&mut bus, &[0x41, 0x1F, 0x40, 0x48, 0xFF, 7]);
        command(&mut bus, &block);
        for index in 1..=16u64 {
            at(&mut bus, (index * 105_000_000).div_ceil(704_000));
            request(&mut bus, &[0x80]);
        }
        assert!(bus.device().irq_pending());
        bus.port_read(0x22E, 1).unwrap();
        command(&mut bus, &[0x1C]);
        let snapshot = bus.device().save();
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&snapshot).unwrap();
        assert_eq!(restored.save(), snapshot);
        for index in 0..2048 {
            let NextEvent::At(next) = bus.device().next_event(bus.clock()) else {
                panic!()
            };
            at(&mut bus, next.0);
            request(&mut bus, &[0x80]);
            assert_eq!(
                bus.device().irq_pending(),
                index == 2047,
                "auto-init byte {index}"
            );
        }
    }
}

#[test]
fn explicit_rate_clamps_to_the_documented_limits() {
    for (input, clamped) in [
        (0u16, 5000u64),
        (4999, 5000),
        (45001, 45000),
        (65535, 45000),
    ] {
        let (mut bus, events) = rig();
        command(
            &mut bus,
            &[0x40, 255, 0x41, (input >> 8) as u8, input as u8, 0x14, 0, 0],
        );
        let clock = 105_000_000u64.div_ceil(88 * clamped);
        at(&mut bus, clock);
        request(&mut bus, &[0x80]);
        assert!(events.borrow().contains(&Output::Frame {
            at: VClock(clock),
            numerator: 105_000_000,
            denominator: 88 * clamped,
            pcm: [0; 2]
        }));
    }
}

#[test]
fn reset_accepts_a_same_clock_pulse_and_replies_before_a_poll_loop() {
    let (mut bus, _) = rig();
    command(&mut bus, &[0xF2, 0x10, 0xFF]);
    bus.port_write(0x226, 1, 1).unwrap();
    assert!(!bus.device().irq_pending());
    assert_eq!(bus.port_read(0x22C, 1).unwrap(), 0x80);
    command(&mut bus, &[0xE1]);
    bus.port_write(0x226, 1, 0).unwrap();
    assert_eq!(bus.port_read(0x22C, 1).unwrap(), 0);
    assert_eq!(bus.port_read(0x22E, 1).unwrap(), 0x80);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 0xAA);
    command(&mut bus, &[0xE1]);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 4);
    assert_eq!(bus.port_read(0x22A, 1).unwrap(), 5);
}

#[test]
fn playback_command_restarts_a_paused_single_cycle_tail() {
    for block in [
        vec![0x14, 1, 0],
        vec![0xC0, 0, 1, 0],
        vec![0xC6, 0, 1, 0],
        vec![0x1C],
    ] {
        let (mut bus, events) = rig();
        command(&mut bus, &[0x41, 0x1F, 0x40, 0x48, 1, 0, 0x14, 0, 0]);
        at(&mut bus, 150);
        request(&mut bus, &[0xFF]);
        assert!(bus.device().irq_pending());
        bus.port_read(0x22E, 1).unwrap();
        at(&mut bus, 160);
        command(&mut bus, &[0xD0]);
        at(&mut bus, 500);
        command(&mut bus, &block);
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(650))
        );
        at(&mut bus, 649);
        assert!(bus.device_mut().take_dma_request().is_none());
        at(&mut bus, 650);
        request(&mut bus, &[0x81]);
        assert!(!bus.device().irq_pending());
        at(&mut bus, 799);
        request(&mut bus, &[0x80]);
        assert!(bus.device().irq_pending());
        assert_eq!(
            frames(&events),
            [(150, [32512; 2]), (650, [256; 2]), (799, [0; 2])]
        );
    }
}

#[test]
fn single_cycle_command_replaces_paused_auto_init_without_continue() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0x48, 3, 0, 0x1C]);
    at(&mut bus, 150);
    request(&mut bus, &[0xFF]);
    at(&mut bus, 160);
    command(&mut bus, &[0xD0]);
    at(&mut bus, 500);
    command(&mut bus, &[0x14, 1, 0]);
    assert_eq!(
        bus.device().next_event(bus.clock()),
        NextEvent::At(VClock(650))
    );
    at(&mut bus, 650);
    request(&mut bus, &[0x81]);
    assert!(!bus.device().irq_pending());
    at(&mut bus, 799);
    request(&mut bus, &[0x80]);
    assert!(bus.device().irq_pending());
    assert_eq!(
        frames(&events),
        [(150, [32512; 2]), (650, [256; 2]), (799, [0; 2])]
    );
    at(&mut bus, 948);
    assert!(events.borrow().contains(&Output::Pause(VClock(948))));
    assert!(bus.device_mut().take_dma_request().is_none());
}

#[test]
fn snapshot_restores_after_a_live_rate_change_finishes_the_last_frame() {
    for rate in [vec![0x40, 193], vec![0x41, 0x3E, 0x80]] {
        let (mut bus, _) = rig();
        command(&mut bus, &[0x41, 0x1F, 0x40, 0x14, 0, 0]);
        at(&mut bus, 150);
        request(&mut bus, &[0xFF]);
        at(&mut bus, 200);
        command(&mut bus, &rate);
        at(&mut bus, 299);
        assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
        at(&mut bus, 400);
        let snapshot = bus.device().save();
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&snapshot).unwrap();
        assert_eq!(restored.save(), snapshot);
    }
}

#[test]
fn snapshot_restores_after_direct_dac_replaces_a_pending_live_rate_change() {
    for rate in [vec![0x40, 193], vec![0x41, 0x3E, 0x80]] {
        let (mut bus, _) = rig();
        command(&mut bus, &[0x41, 0x1F, 0x40, 0x14, 0, 0]);
        at(&mut bus, 150);
        request(&mut bus, &[0xFF]);
        at(&mut bus, 200);
        command(&mut bus, &rate);
        at(&mut bus, 210);
        command(&mut bus, &[0x10, 0x40]);
        let snapshot = bus.device().save();
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&snapshot).unwrap();
        assert_eq!(restored.save(), snapshot);
    }
}

fn mixer_write(bus: &mut MockBus, index: u8, value: u8) {
    bus.port_write(0x224, 1, index.into()).unwrap();
    bus.port_write(0x225, 1, value.into()).unwrap();
}
fn mixer_read(bus: &mut MockBus, index: u8) -> u32 {
    bus.port_write(0x224, 1, index.into()).unwrap();
    bus.port_read(0x225, 1).unwrap()
}

#[test]
fn ct1745_reset_masks_aliases_and_fixed_resources() {
    let (mut bus, _) = rig();
    mixer_write(&mut bus, 0, 0);
    for index in 0x30..=0x47 {
        let expected = match index {
            0x30..=0x35 => 0xC0,
            0x3C => 0x1F,
            0x3D => 0x15,
            0x3E => 0x0B,
            0x44..=0x47 => 0x80,
            _ => 0,
        };
        assert_eq!(mixer_read(&mut bus, index), expected, "{index:02x}");
        mixer_write(&mut bus, index, 255);
    }
    mixer_write(&mut bus, 0, 123);
    assert_eq!(mixer_read(&mut bus, 0x30), 0xC0);
    assert_eq!(mixer_read(&mut bus, 0x44), 0x80);
    for (old, new) in [
        (4, 0x32),
        (0x22, 0x30),
        (0x26, 0x34),
        (0x28, 0x36),
        (0x2E, 0x38),
    ] {
        mixer_write(&mut bus, old, 0xA3);
        assert_eq!(mixer_read(&mut bus, new), 0xA8);
        assert_eq!(mixer_read(&mut bus, new + 1), 0x38);
        assert_eq!(mixer_read(&mut bus, old), 0xA3);
        mixer_write(&mut bus, new, 0xF8);
        assert_eq!(mixer_read(&mut bus, old), 0xF3);
    }
    mixer_write(&mut bus, 0x0A, 5);
    assert_eq!(mixer_read(&mut bus, 0x3A), 0xA0);
    assert_eq!(mixer_read(&mut bus, 0x0A), 5);
    for (index, expected) in [(0x80, 2), (0x81, 0x22), (0x82, 0)] {
        mixer_write(&mut bus, index, 255);
        assert_eq!(mixer_read(&mut bus, index), expected);
    }
    mixer_write(&mut bus, 0x7F, 42);
    assert_eq!(mixer_read(&mut bus, 0x7F), 255);
}

#[test]
fn word_dma_formats_and_auto_init_use_real_at_word_controller() {
    for opcode in [0xB0, 0xB2, 0xB4, 0xB6] {
        for mode in [0, 0x10, 0x20, 0x30] {
            let (mut bus, events) = rig();
            for index in 0x30..=0x33 {
                mixer_write(&mut bus, index, 0xF8);
            }
            let mut dma = crate::Dma8237::new_at_word();
            for (port, value) in [
                (0xD8, 0),
                (0xC4, 0),
                (0xC4, 0x08),
                (0xC6, 3),
                (0xC6, 0),
                (0x8B, 0),
                (0xD6, 0x59),
                (0xD4, 1),
            ] {
                dma.port_write(port, 1, value).unwrap();
            }
            let mut memory = Memory(vec![0; 0x1008]);
            memory.0[0x1000..].copy_from_slice(&[0, 0x80, 0xFF, 0x7F, 0, 0, 0x34, 0x12]);
            command(&mut bus, &[0x41, 0x1F, 0x40, opcode, mode, 3, 0]);
            let stereo = mode & 0x20 != 0;
            let signed = mode & 0x10 != 0;
            let samples = if signed {
                [-32768, 32767, 0, 0x1234]
            } else {
                [0, -1, -32768, -28108]
            };
            let blocks = if opcode & 4 != 0 { 2 } else { 1 };
            let per_block = if stereo { 2 } else { 4 };
            for frame in 0..blocks * per_block {
                let clock = ((frame as u64 + 1) * 105_000_000).div_ceil(704_000);
                at(&mut bus, clock - 1);
                assert!(bus.device_mut().take_dma_request().is_none());
                at(&mut bus, clock);
                let request = bus
                    .device_mut()
                    .take_dma_request()
                    .expect("word DMA request");
                assert_eq!(request.channel, 5);
                let DmaOp::FromMemory { len } = request.op else {
                    panic!()
                };
                assert_eq!(len, if stereo { 4 } else { 2 });
                let mut bytes = vec![0; len];
                let result = dma.dma_service_read(1, &mut memory, &mut bytes);
                assert_eq!(result.transferred, len);
                bus.device_mut().dma_complete(DmaOutcome::FromMemory {
                    bytes,
                    terminal_count: result.terminal_count,
                });
                let i = (frame % per_block) * if stereo { 2 } else { 1 };
                assert_eq!(
                    frames(&events).last(),
                    Some(&(clock, [samples[i], samples[if stereo { i + 1 } else { i }]]))
                );
                assert_eq!(
                    bus.device().irq_pending(),
                    frame % per_block == per_block - 1
                );
                if bus.device().irq_pending() {
                    assert_eq!(mixer_read(&mut bus, 0x82), 2);
                    bus.port_read(0x22E, 1).unwrap();
                    assert!(bus.device().irq_pending());
                    bus.port_read(0x22F, 1).unwrap();
                    assert_eq!(mixer_read(&mut bus, 0x82), 0);
                }
            }
        }
    }
}

fn word_request(bus: &mut MockBus, bytes: &[u8]) {
    assert_eq!(
        bus.device_mut().take_dma_request(),
        Some(DmaRequest {
            channel: 5,
            op: DmaOp::FromMemory { len: bytes.len() }
        })
    );
    bus.device_mut().dma_complete(DmaOutcome::FromMemory {
        bytes: bytes.to_vec(),
        terminal_count: false,
    });
}

#[test]
fn word_pause_continue_and_exit_are_width_specific_and_preserve_phase() {
    for auto in [false, true] {
        let (mut bus, events) = rig();
        command(
            &mut bus,
            &[0x41, 0x1F, 0x40, if auto { 0xB6 } else { 0xB0 }, 0x10, 1, 0],
        );
        at(&mut bus, 150);
        word_request(&mut bus, &[0x34, 0x12]);
        at(&mut bus, 201);
        command(&mut bus, &[0xD0, 0xDA]);
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(299))
        );
        command(&mut bus, &[0xD5]);
        assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
        at(&mut bus, 901);
        command(&mut bus, &[0xD4]);
        assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
        command(&mut bus, &[0xD6]);
        assert_eq!(
            bus.device().next_event(bus.clock()),
            NextEvent::At(VClock(999))
        );
        let state = bus.device().save();
        let mut restored = Sb16::new(0x220, 1);
        restored.restore(&state).unwrap();
        assert_eq!(restored.save(), state);
        at(&mut bus, 999);
        word_request(&mut bus, &[0, 0x80]);
        assert!(bus.device().irq_pending());
        bus.port_read(0x22F, 1).unwrap();
        if auto {
            at(&mut bus, 1148);
            word_request(&mut bus, &[0, 0]);
            command(&mut bus, &[0xD9]);
            at(&mut bus, 1297);
            word_request(&mut bus, &[0xFF, 0x7F]);
            assert!(bus.device().irq_pending());
        }
        let end = if auto { 1446 } else { 1148 };
        at(&mut bus, end);
        assert!(bus.device_mut().take_dma_request().is_none());
        assert!(events.borrow().contains(&Output::Pause(VClock(end))));
        assert_eq!(
            frames(&events)[..3],
            [(150, [0x1234; 2]), (901, [0x1234; 2]), (999, [-32768; 2])]
        );
    }
    let (mut bus, _) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0xC4, 0, 1, 0]);
    at(&mut bus, 150);
    request(&mut bus, &[0x80]);
    command(&mut bus, &[0xD5, 0xD6, 0xD9]);
    at(&mut bus, 299);
    request(&mut bus, &[0x80]);
    bus.port_read(0x22E, 1).unwrap();
    at(&mut bus, 448);
    request(&mut bus, &[0x80]);
}

#[test]
fn word_handler_chaining_and_paused_replacement_keep_one_stream() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0xB0, 0x30, 1, 0]);
    at(&mut bus, 150);
    word_request(&mut bus, &[1, 0, 2, 0]);
    command(&mut bus, &[0xF2]);
    assert_eq!(mixer_read(&mut bus, 0x82), 3);
    bus.port_read(0x22F, 1).unwrap();
    assert_eq!(mixer_read(&mut bus, 0x82), 1);
    bus.port_read(0x22E, 1).unwrap();
    at(&mut bus, 160);
    command(&mut bus, &[0xB2, 0x10, 0, 0]);
    at(&mut bus, 299);
    word_request(&mut bus, &[3, 0]);
    assert_eq!(frames(&events), [(150, [1, 2]), (299, [3, 3])]);
    command(&mut bus, &[0xD5]);
    at(&mut bus, 500);
    command(&mut bus, &[0xB4, 0x10, 0, 0]);
    at(&mut bus, 650);
    word_request(&mut bus, &[4, 0]);
    command(&mut bus, &[0x14, 0, 0]);
    at(&mut bus, 800);
    request(&mut bus, &[0x81]);
    assert_eq!(frames(&events).last(), Some(&(800, [256; 2])));
    command(&mut bus, &[0xB0, 0x10, 0, 0]);
    at(&mut bus, 950);
    word_request(&mut bus, &[5, 0]);
    assert_eq!(frames(&events).last(), Some(&(950, [5; 2])));
}

#[test]
fn word_partial_stereo_odd_block_and_snapshot_restore_keep_word_order() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0xB0, 0x30, 2, 0]);
    at(&mut bus, 150);
    assert_eq!(
        bus.device_mut().take_dma_request().unwrap().op,
        DmaOp::FromMemory { len: 4 }
    );
    bus.device_mut().dma_complete(DmaOutcome::FromMemory {
        bytes: vec![0x34, 0x12],
        terminal_count: true,
    });
    let state = bus.device().save();
    let mut restored = Sb16::new(0x220, 1);
    restored.restore(&state).unwrap();
    assert_eq!(restored.save(), state);
    word_request(&mut bus, &[0x78, 0x56]);
    at(&mut bus, 299);
    word_request(&mut bus, &[0xBC, 0x7A]);
    assert_eq!(
        frames(&events),
        [(150, [0x1234, 0x5678]), (299, [0x7ABC, 0])]
    );
    let state = bus.device().save();
    restored.restore(&state).unwrap();
    assert_eq!(restored.save(), state);
    assert!(restored.irq_pending());
}

#[test]
fn mixer_attenuation_has_exact_integers_live_changes_and_stored_only_controls() {
    let (mut bus, events) = rig();
    mixer_write(&mut bus, 0, 0);
    command(&mut bus, &[0x10, 0]);
    assert_eq!(frames(&events).last(), Some(&(0, [-1304; 2])));
    at(&mut bus, 10);
    for index in 0x30..=0x33 {
        mixer_write(&mut bus, index, 0xF8);
    }
    assert_eq!(frames(&events).last(), Some(&(10, [-32768; 2])));
    mixer_write(&mut bus, 0x30, 0xF0);
    mixer_write(&mut bus, 0x33, 0xE8);
    assert_eq!(frames(&events).last(), Some(&(10, [-26028, -20675])));
    for index in 0x34..=0x47 {
        mixer_write(&mut bus, index, 255);
        assert_eq!(
            mixer_read(&mut bus, index),
            match index {
                0x34..=0x3A => 0xF8,
                0x3B | 0x3F..=0x42 => 0xC0,
                0x3C => 0x1F,
                0x3D..=0x3E => 0x7F,
                0x43 => 1,
                _ => 0xF0,
            }
        );
        assert_eq!(frames(&events).last(), Some(&(10, [-26028, -20675])));
    }
    mixer_write(&mut bus, 0x30, 0);
    mixer_write(&mut bus, 0x33, 0);
    assert_eq!(frames(&events).last(), Some(&(10, [-26; 2])));
    let state = bus.device().save();
    let mut restored = Sb16::new(0x220, 1);
    restored.restore(&state).unwrap();
    assert_eq!(restored.save(), state);
}

#[test]
fn legacy_snapshot_restores_mixer_defaults_and_no_word_interrupt() {
    // Frozen save from the byte-only DSP: 8000 Hz, two unsigned mono bytes,
    // first byte FFh completed at clock 150, with one byte still pending.
    let legacy: &[u8] = &[
        0x53, 0x42, 0x31, 0x36, 0x2D, 0x38, 0x56, 0x32, 0x20, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x96, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7F,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x02, 0x01, 0x00, 0x00,
    ];
    let output = Events::default();
    let writer = output.clone();
    let mut restored = Sb16::new(0x220, 1);
    restored.observe_output(move |event| writer.borrow_mut().push(event));
    restored.restore(legacy).unwrap();
    assert_eq!(frames(&output), [(150, [1294; 2])]);
    restored.port_write(0x224, 1, 0x30).unwrap();
    assert_eq!(restored.port_read(0x225, 1).unwrap(), 0xC0);
    restored.port_write(0x224, 1, 0x82).unwrap();
    assert_eq!(restored.port_read(0x225, 1).unwrap(), 0);
    assert_eq!(&restored.save()[..8], b"SB16-MV3");
}

#[test]
fn fixed_resource_readback_and_word_requests_follow_every_supported_selection() {
    for irq in [2, 5, 7, 10] {
        for dma8 in [0, 1, 3] {
            for dma16 in [5, 6, 7] {
                let mut bus =
                    MockBus::new(Box::new(Sb16::new(0x220, dma8).with_resources(dma16, irq)));
                let irq_bit = match irq {
                    2 => 1,
                    5 => 2,
                    7 => 4,
                    _ => 8,
                };
                mixer_write(&mut bus, 0x80, 0);
                assert_eq!(mixer_read(&mut bus, 0x80), irq_bit);
                mixer_write(&mut bus, 0x81, 0);
                assert_eq!(mixer_read(&mut bus, 0x81), (1 << dma8) | (1 << dma16));
                command(&mut bus, &[0xB6, 0x30, 255, 255]);
                let state = bus.device().save();
                let mut restored = Sb16::new(0x220, dma8).with_resources(dma16, irq);
                restored.restore(&state).unwrap();
                assert_eq!(restored.save(), state);
                let mut wrong =
                    Sb16::new(0x220, dma8).with_resources(if dma16 == 5 { 6 } else { 5 }, irq);
                assert!(wrong.restore(&state).is_err());
                at(&mut bus, 55);
                assert_eq!(
                    bus.device_mut().take_dma_request().unwrap(),
                    DmaRequest {
                        channel: dma16,
                        op: DmaOp::FromMemory { len: 4 }
                    }
                );
            }
        }
    }
}

#[test]
fn mixer_reset_domains_and_paused_volume_changes_preserve_raw_level() {
    let (mut bus, events) = rig();
    command(&mut bus, &[0x41, 0x1F, 0x40, 0xB0, 0x10, 1, 0]);
    at(&mut bus, 150);
    word_request(&mut bus, &[0, 0x80]);
    at(&mut bus, 200);
    command(&mut bus, &[0xD5]);
    let old_len = events.borrow().len();
    mixer_write(&mut bus, 0x30, 0xF0);
    assert_eq!(events.borrow().len(), old_len);
    at(&mut bus, 300);
    command(&mut bus, &[0xD6]);
    assert_eq!(frames(&events).last(), Some(&(300, [-26028, -32768])));
    bus.port_write(0x226, 1, 1).unwrap();
    bus.port_write(0x226, 1, 0).unwrap();
    assert_eq!(mixer_read(&mut bus, 0x30), 0xF0);
    bus.device_mut().reset();
    assert_eq!(mixer_read(&mut bus, 0x30), 0xC0);
}

#[test]
fn compatibility_full_scale_is_unity_in_native_registers_and_pcm() {
    let (mut bus, events) = rig();
    for (alias, native) in [
        (0x22, 0x30),
        (4, 0x32),
        (0x26, 0x34),
        (0x28, 0x36),
        (0x2E, 0x38),
    ] {
        mixer_write(&mut bus, alias, 0xFF);
        assert_eq!(mixer_read(&mut bus, native), 0xF8);
        assert_eq!(mixer_read(&mut bus, native + 1), 0xF8);
        assert_eq!(mixer_read(&mut bus, alias), 0xFF);
    }
    command(&mut bus, &[0x10, 0]);
    assert_eq!(frames(&events).last(), Some(&(0, [-32768; 2])));
}

#[test]
fn native_zero_volume_is_minus_sixty_two_db_per_stage() {
    let (mut bus, events) = rig();
    mixer_write(&mut bus, 0x30, 0);
    mixer_write(&mut bus, 0x33, 0);
    command(&mut bus, &[0x10, 0]);
    assert_eq!(frames(&events).last(), Some(&(0, [-26; 2])));
    mixer_write(&mut bus, 0x32, 0);
    mixer_write(&mut bus, 0x31, 0);
    assert_eq!(frames(&events).last(), Some(&(0, [0; 2])));
}
