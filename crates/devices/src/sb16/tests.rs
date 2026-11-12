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
        assert_eq!(&snapshot[..8], b"SB16-8V2");
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
    for (opcode, count) in commands
        .into_iter()
        .chain((0xB0..=0xBF).map(|opcode| (opcode, 3)))
    {
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
