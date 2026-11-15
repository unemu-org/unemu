// SPDX-License-Identifier: AGPL-3.0-or-later
//! System-control port `0x61` (§7.1 device seam).
//!
//! IBM Personal Computer AT Technical Reference, System Timers and System Board
//! I/O, Port B: bit 0 gates PIT channel 2; bit 4 is a flip-flop toggled by each
//! channel-1 OUT rising edge (refresh request); bit 5 is channel-2 OUT.
//! System Board, Speaker: the speaker drive is OUT2 AND bit 1. Timestamped
//! pin changes feed the audio observer; analogue filtering is not modelled.
//! See `backlog/docs/design/audio.md` §PC speaker.
//! See `backlog/docs/design/scheduler.md` §Device migration inventory.
//!
//! ## Coverage matrix
//! Modelled:
//! - Bits 0..3: stored gate, speaker-data and error-check controls.
//! - Bit 0 drives the shared PIT's channel-2 GATE in Modes 0, 2 and 3.
//! - Bits 4/5: refresh detect and OUT2 sampled at the virtual PIT access clock.
//! - Bits 6/7: no parity or I/O-channel-check error is reported.
//!
//! Not modelled: speaker analogue filtering, DRAM transfers, error latching and NMIs.
//! Without PIT wiring, refresh stays low and OUT2 stays at its idle high level.
//!
//! Snapshots contain the control nibble and absolute trace clock. The shared
//! PIT snapshot owns the refresh flip-flop, gate and counter phases; restoring
//! either device never replays an edge or overwrites the other's saved state.

use crate::pit8253::SharedPit8253;

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "sys-control";

/// The single port decoded: the PC/AT system-control port (8255 PPI port B).
pub const SYS_CONTROL_PORT: u16 = 0x61;

/// Bit 0: PIT channel-2 GATE (speaker timer gate).
const GATE: u8 = 0x01;
/// Bit 1: speaker data enable.
const SPEAKER_DATA: u8 = 0x02;
/// Bit 2: enable parity-check.
const PARITY_CHECK_ENABLE: u8 = 0x04;
/// Bit 3: enable I/O-channel-check.
const IOCHK_ENABLE: u8 = 0x08;

/// Mask of the guest-writable control bits kept in the low nibble.
const CONTROL_MASK: u8 = GATE | SPEAKER_DATA | PARITY_CHECK_ENABLE | IOCHK_ENABLE;

/// Bit 4: refresh-detect flip-flop.
const REFRESH_DETECT: u8 = 0x10;
/// Bit 5: timer-2 OUT pin.
const TIMER2_OUT: u8 = 0x20;
/// Bit 6: I/O-channel-check status. Always clear here (0 = no error).
const IOCHK_STATUS: u8 = 0x40;
/// Bit 7: parity-check status. Always clear here (0 = no error).
const PARITY_STATUS: u8 = 0x80;

/// Snapshot format version for [`Device::save`] / [`Device::restore`].
const SNAPSHOT_VERSION: u8 = 3;
/// Snapshot length: version, control byte and absolute trace clock.
const SNAPSHOT_LEN: usize = 1 + 1 + 8;

/// The PC/AT system-control port device (§7.1).
///
/// Decodes the one port [`SYS_CONTROL_PORT`]. Writes store the low control
/// nibble; reads return that nibble in the low bits plus status in the high bits,
/// sampled from the PIT at the virtual access clock.
#[derive(Default)]
pub struct SysControl {
    /// The guest-written control nibble (bits 0..=3); high bits are always clear.
    control: u8,
    /// Optional AT timer wiring; absent timers leave refresh low and OUT2 high.
    pit: Option<SharedPit8253>,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl SysControl {
    /// Create a system-control port decoding [`SYS_CONTROL_PORT`] at power-on
    /// (all control bits clear).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reject any port outside this device's single decoded port.
    fn check(&self, port: u16) -> Result<(), DeviceError> {
        if port == SYS_CONTROL_PORT {
            Ok(())
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            })
        }
    }

    /// Connect the AT timer lines. Bit 0 starts low at power-on.
    #[must_use]
    pub fn with_pit(mut self, pit: SharedPit8253) -> Self {
        pit.bind_gate2();
        pit.set_gate2(self.now, self.control & GATE != 0);
        self.pit = Some(pit);
        self
    }

    /// IBM Personal Computer AT Technical Reference, System Timers and
    /// System Board I/O, Port B: bit 4 toggles per refresh request; bit 5
    /// samples counter 2 OUT. Sampling itself never advances either phase.
    fn status_byte(&mut self) -> u8 {
        let (refresh, out2) = self
            .pit
            .as_ref()
            .map_or((false, true), |pit| pit.status(self.now));
        let value = (self.control & CONTROL_MASK)
            | if refresh { REFRESH_DETECT } else { 0 }
            | if out2 { TIMER2_OUT } else { 0 };
        value & !(IOCHK_STATUS | PARITY_STATUS)
    }
}

impl Device for SysControl {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.check(port)?;
        let value = self.status_byte();
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value: u32::from(value),
        });
        Ok(u32::from(value))
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.check(port)?;
        // Only the low control nibble is guest-writable; the status high bits are
        // read-only and any bits the guest sets there are dropped.
        self.control = (value as u8) & CONTROL_MASK;
        if let Some(pit) = &self.pit {
            pit.set_gate2(self.now, self.control & GATE != 0);
            pit.set_speaker_data(self.now, self.control & SPEAKER_DATA != 0);
        }
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Write,
            value,
        });
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
        // The system-control port is port-mapped only, never memory-mapped.
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }

    fn mmio_write(&mut self, addr: u64, _size: u8, _value: u64) -> Result<(), DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }

    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now < self.now {
            return;
        }
        self.now = now;
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }

    fn irq_pending(&self) -> bool {
        // The system-control port drives no IRQ line in this model (parity /
        // I/O-channel-check NMIs are not generated).
        false
    }

    fn reset(&mut self) {
        self.control = 0;
        if let Some(pit) = &self.pit {
            pit.clear_speaker_data();
            pit.clear_gate2();
        }
        self.trace.clear();
        self.now = VClock::ZERO;
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_LEN);
        blob.push(SNAPSHOT_VERSION);
        blob.push(self.control);
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() != SNAPSHOT_LEN {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot length mismatch",
            });
        }
        if snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        if snapshot[1] & !CONTROL_MASK != 0 {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid control bits",
            });
        }
        self.control = snapshot[1];
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| {
            snapshot[snapshot.len() - 8 + i]
        })));
        Ok(())
    }

    fn set_trace_policy(&mut self, policy: device::TracePolicy) {
        self.trace.set_policy(policy);
    }

    fn trace_dropped(&self) -> u64 {
        self.trace.dropped()
    }

    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.trace.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    type SpeakerEdges = std::rc::Rc<std::cell::RefCell<Vec<(VClock, bool)>>>;

    fn speaker_bus(mode: u8, divisor: u8) -> (MockBus, MockBus, SpeakerEdges) {
        let pit = SharedPit8253::default();
        let sys = SysControl::new().with_pit(pit.clone());
        let edges = SpeakerEdges::default();
        let output = edges.clone();
        pit.observe_speaker(move |at, high| output.borrow_mut().push((at, high)));
        let mut timer = MockBus::new(Box::new(pit));
        timer
            .port_write(0x43, 1, 0xB0 | u32::from(mode << 1))
            .unwrap();
        timer.port_write(0x42, 1, u32::from(divisor)).unwrap();
        timer.port_write(0x42, 1, 0).unwrap();
        (MockBus::new(Box::new(sys)), timer, edges)
    }

    /// Intel 8254 datasheet, Modes 0, 2 and 3: load takes one clock;
    /// low pulses and square-wave half-periods must survive a sparse sync.
    #[test]
    fn speaker_sparse_sync_records_exact_out2_edges() {
        for (mode, expected) in [
            (0, vec![(0, false), (5, true)]),
            (
                2,
                vec![
                    (0, false),
                    (0, true),
                    (4, false),
                    (5, true),
                    (8, false),
                    (9, true),
                ],
            ),
            (
                3,
                vec![
                    (0, false),
                    (0, true),
                    (3, false),
                    (5, true),
                    (7, false),
                    (9, true),
                ],
            ),
        ] {
            let (mut sys, mut timer, edges) = speaker_bus(mode, 4);
            sys.port_write(0x61, 1, 3).unwrap();
            timer.tick(10);
            assert_eq!(
                *edges.borrow(),
                expected
                    .into_iter()
                    .map(|(at, level)| (VClock(at), level))
                    .collect::<Vec<_>>(),
                "mode {mode}"
            );
        }
    }

    /// IBM Personal Computer AT Technical Reference, System Board, Speaker;
    /// Intel 8254 datasheet, Read/Write Logic: a host reads port 61h's lines
    /// and counter 2's complete count without moving the timer clock.
    #[test]
    fn speaker_controls_report_port_b_and_counter2_without_syncing() {
        use crate::pit8253::SpeakerControls;
        let pit = SharedPit8253::default();
        let mut sys = MockBus::new(Box::new(SysControl::new().with_pit(pit.clone())));
        let mut timer = MockBus::new(Box::new(pit.clone()));
        timer.port_write(0x43, 1, 0xB6).unwrap();
        timer.port_write(0x42, 1, 0xA9).unwrap();
        assert_eq!(pit.speaker_controls().divisor, 0, "half a two-byte count");
        timer.port_write(0x42, 1, 0x04).unwrap();
        sys.tick(50);
        sys.port_write(0x61, 1, 3).unwrap();
        let on = SpeakerControls {
            at: VClock(50),
            gate: true,
            data: true,
            mode: 3,
            divisor: 0x04A9,
        };
        assert_eq!(pit.speaker_controls(), on);
        sys.tick(20);
        assert_eq!(pit.speaker_controls(), on);
    }

    /// IBM Personal Computer AT Technical Reference, System Board, Speaker:
    /// bit 0 controls the timer, while bit 1 independently masks its OUT pin.
    #[test]
    fn speaker_gate_and_data_enable_are_independent() {
        let (mut sys, mut timer, edges) = speaker_bus(3, 4);
        sys.port_write(0x61, 1, 2).unwrap();
        sys.tick(100);
        sys.port_write(0x61, 1, 3).unwrap();
        sys.tick(3);
        sys.port_read(0x61, 1).unwrap();
        sys.port_write(0x61, 1, 2).unwrap();
        sys.tick(1);
        sys.port_write(0x61, 1, 0).unwrap();
        timer.tick(200);
        assert_eq!(
            *edges.borrow(),
            vec![
                (VClock(0), false),
                (VClock(0), true),
                (VClock(103), false),
                (VClock(103), true),
                (VClock(104), false)
            ]
        );
    }

    #[test]
    fn speaker_direct_bit1_writes_record_pwm_edges_without_gate() {
        let (mut sys, _timer, edges) = speaker_bus(3, 4);
        for (at, value) in [
            (7, 2),
            (13, 0),
            (29, 2),
            (31, 0),
            (48, 2),
            (52, 0),
            (73, 2),
            (99, 0),
        ] {
            sys.device_mut().sync(VClock(at));
            sys.port_write(0x61, 1, value).unwrap();
        }
        assert_eq!(
            *edges.borrow(),
            vec![
                (VClock(0), false),
                (VClock(7), true),
                (VClock(13), false),
                (VClock(29), true),
                (VClock(31), false),
                (VClock(48), true),
                (VClock(52), false),
                (VClock(73), true),
                (VClock(99), false)
            ]
        );
    }

    /// Intel 8254 datasheet, Mode 3: a new count waits for the current
    /// half-period to finish, then defines the following half-periods.
    #[test]
    fn speaker_mode3_count_written_mid_half_preserves_current_edge() {
        let (mut sys, mut timer, edges) = speaker_bus(3, 8);
        sys.port_write(0x61, 1, 3).unwrap();
        timer.device_mut().sync(VClock(3));
        timer.port_write(0x42, 1, 4).unwrap();
        timer.port_write(0x42, 1, 0).unwrap();
        timer.device_mut().sync(VClock(12));
        assert_eq!(
            *edges.borrow(),
            vec![
                (VClock(0), false),
                (VClock(0), true),
                (VClock(5), false),
                (VClock(7), true),
                (VClock(9), false),
                (VClock(11), true)
            ]
        );
    }

    /// Intel 8254 datasheet, Mode 3: a control word raises OUT immediately;
    /// a following count loads on the next clock and establishes a new phase.
    #[test]
    fn speaker_control_word_during_low_half_restarts_tone_phase() {
        let (mut sys, mut timer, edges) = speaker_bus(3, 8);
        sys.port_write(0x61, 1, 3).unwrap();
        timer.device_mut().sync(VClock(6));
        timer.port_write(0x43, 1, 0xB6).unwrap();
        timer.port_write(0x42, 1, 6).unwrap();
        timer.port_write(0x42, 1, 0).unwrap();
        timer.device_mut().sync(VClock(16));
        assert_eq!(
            *edges.borrow(),
            vec![
                (VClock(0), false),
                (VClock(0), true),
                (VClock(5), false),
                (VClock(6), true),
                (VClock(10), false),
                (VClock(13), true),
                (VClock(16), false)
            ]
        );
    }

    #[test]
    fn dormant_matches_per_clock_seeded_accesses_and_long_jumps() {
        let mut reference = MockBus::new(Box::new(crate::trace_policy_tests::traced(
            SysControl::new(),
        )));
        let mut scheduled = MockBus::new(Box::new(crate::trace_policy_tests::traced(
            SysControl::new(),
        )));
        let mut seed = 0x8042_1468_1816_5501u64;
        for step in 0..128 {
            assert_eq!(
                scheduled.device().next_event(reference.clock()),
                device::NextEvent::Dormant
            );
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let clocks = if step % 31 == 0 {
                100_003
            } else {
                (seed >> 32) % 17
            };
            for _ in 0..clocks {
                reference.tick(1);
                assert!(!reference.device().irq_pending());
            }
            scheduled.device_mut().sync(reference.clock());
            assert_eq!(reference.device().save(), scheduled.device().save());
            assert_eq!(
                reference.device().irq_pending(),
                scheduled.device().irq_pending()
            );
            if seed & 1 == 0 {
                reference.port_write(0x61, 1, seed as u32).unwrap();
                scheduled.port_write(0x61, 1, seed as u32).unwrap();
            } else {
                // Consecutive reads at one clock retain the timer status.
                for _ in 0..4 {
                    assert_eq!(
                        reference.port_read(0x61, 1).unwrap(),
                        scheduled.port_read(0x61, 1).unwrap()
                    );
                }
            }
            assert_eq!(reference.device().save(), scheduled.device().save());
            assert_eq!(
                reference.device().irq_pending(),
                scheduled.device().irq_pending()
            );
            assert_eq!(
                reference.device_mut().take_trace(),
                scheduled.device_mut().take_trace()
            );
        }
    }

    fn timer_bus(channel: u8, mode: u8, count: u8) -> (MockBus, MockBus) {
        let pit = SharedPit8253::default();
        let sys = SysControl::new().with_pit(pit.clone());
        let mut timer = MockBus::new(Box::new(pit));
        timer
            .port_write(0x43, 1, u32::from((channel << 6) | 0x30 | (mode << 1)))
            .unwrap();
        timer
            .port_write(0x40 + u16::from(channel), 1, u32::from(count))
            .unwrap();
        timer.port_write(0x40 + u16::from(channel), 1, 0).unwrap();
        (MockBus::new(Box::new(sys)), timer)
    }

    fn reset_pair_then_check_timer2_gate(pit_first: bool) {
        let (mut sys, mut timer) = timer_bus(2, 0, 4);
        sys.port_write(0x61, 1, 1).unwrap();
        sys.tick(3);
        sys.port_read(0x61, 1).unwrap();
        if pit_first {
            timer.device_mut().reset();
            sys.device_mut().reset();
        } else {
            sys.device_mut().reset();
            timer.device_mut().reset();
        }
        assert_eq!(sys.port_read(0x61, 1).unwrap() & 1, 0);
        timer.port_write(0x43, 1, 0xB0).unwrap();
        timer.port_write(0x42, 1, 4).unwrap();
        timer.port_write(0x42, 1, 0).unwrap();
        sys.tick(100);
        // Intel 8254 datasheet, Mode 0: low GATE permits loading but inhibits
        // counting. OUT must remain low until the software gate is raised.
        assert_eq!(sys.port_read(0x61, 1).unwrap() & 0x21, 0);
        timer.port_write(0x43, 1, 0x80).unwrap();
        assert_eq!(timer.port_read(0x42, 1).unwrap(), 4);
        assert_eq!(timer.port_read(0x42, 1).unwrap(), 0);
        sys.port_write(0x61, 1, 1).unwrap();
        sys.tick(4);
        assert_eq!(sys.port_read(0x61, 1).unwrap() & 0x21, 0x21);
    }

    #[test]
    fn reset_pit_then_sys_control_keeps_timer2_gate_low() {
        reset_pair_then_check_timer2_gate(true);
    }

    #[test]
    fn reset_sys_control_then_pit_keeps_timer2_gate_low() {
        reset_pair_then_check_timer2_gate(false);
    }

    #[test]
    fn refresh_tracks_channel1_rising_edges_not_reads() {
        // Intel 8254 datasheet, Mode 2: count loads at clock 1, OUT goes
        // low at 18 and rises at 19; following rising edges are 18 apart.
        let (mut sys, _timer) = timer_bus(1, 2, 18);
        for (at, high) in [
            (0, false),
            (1, false),
            (17, false),
            (18, false),
            (19, true),
            (36, true),
            (37, false),
            (55, true),
            (109, false),
            (1_000_009, false),
        ] {
            sys.device_mut().sync(VClock(at));
            for _ in 0..4 {
                assert_eq!(
                    sys.port_read(0x61, 1).unwrap() & 0x10 != 0,
                    high,
                    "clock {at}"
                );
            }
        }
    }

    #[test]
    fn timer2_output_follows_modes_and_port61_gate() {
        for (mode, levels) in [
            (0, [false, false, false, false, true, true]),
            (2, [true, true, true, false, true, true]),
            (3, [true, true, false, false, true, true]),
        ] {
            let (mut sys, _timer) = timer_bus(2, mode, 4);
            sys.port_write(0x61, 1, 1).unwrap();
            sys.tick(1); // Initial load.
            for high in levels {
                for _ in 0..3 {
                    assert_eq!(
                        sys.port_read(0x61, 1).unwrap() & 0x20 != 0,
                        high,
                        "mode {mode}"
                    );
                }
                sys.tick(1);
            }
            sys.port_write(0x61, 1, 0).unwrap();
            let frozen = sys.port_read(0x61, 1).unwrap() & 0x20;
            sys.tick(100);
            assert_eq!(sys.port_read(0x61, 1).unwrap() & 0x20, frozen);
            if mode != 0 {
                assert_eq!(frozen, 0x20);
            }
            sys.port_write(0x61, 1, 1).unwrap();
            sys.tick(1); // Gate rising reloads Modes 2/3.
            if mode != 0 {
                assert_ne!(sys.port_read(0x61, 1).unwrap() & 0x20, 0);
            }
        }
    }

    #[test]
    fn timer2_mode0_gate_pauses_and_resumes_without_reloading() {
        let (mut sys, _timer) = timer_bus(2, 0, 4);
        sys.tick(10); // Load while GATE is low, but do not count.
        assert_eq!(sys.port_read(0x61, 1).unwrap() & 0x20, 0);
        sys.port_write(0x61, 1, 1).unwrap();
        sys.tick(2);
        sys.port_write(0x61, 1, 0).unwrap();
        sys.tick(100);
        assert_eq!(sys.port_read(0x61, 1).unwrap() & 0x20, 0);
        sys.port_write(0x61, 1, 1).unwrap();
        sys.tick(1);
        assert_eq!(sys.port_read(0x61, 1).unwrap() & 0x20, 0);
        sys.tick(1);
        assert_eq!(sys.port_read(0x61, 1).unwrap() & 0x20, 0x20);
    }

    #[test]
    fn refresh_snapshot_preserves_unread_edges_and_restore_order() {
        let (mut sys, timer) = timer_bus(1, 2, 18);
        sys.tick(19);
        assert_ne!(sys.port_read(0x61, 1).unwrap() & 0x10, 0);
        sys.tick(54); // Three more rising edges have not been sampled yet.
        let sys_state = sys.device().save();
        let timer_state = timer.device().save();
        for timer_first in [false, true] {
            let (mut restored, mut pit) = timer_bus(1, 2, 18);
            if timer_first {
                pit.device_mut().restore(&timer_state).unwrap();
            }
            restored.device_mut().restore(&sys_state).unwrap();
            if !timer_first {
                pit.device_mut().restore(&timer_state).unwrap();
            }
            assert_eq!(restored.port_read(0x61, 1).unwrap() & 0x10, 0);
            restored.device_mut().sync(VClock(91));
            assert_eq!(restored.port_read(0x61, 1).unwrap() & 0x10, 0x10);
        }
    }

    #[test]
    fn refresh_sparse_reads_match_per_clock_edges_after_reprogramming() {
        for mode in [0, 2, 3] {
            let (mut sparse, mut sparse_pit) = timer_bus(1, mode, 5);
            let (mut dense, mut dense_pit) = timer_bus(1, mode, 5);
            for (at, count) in [(1, 9), (13, 4), (101, 18), (100_003, 7)] {
                sparse.device_mut().sync(VClock(at));
                for clock in dense.clock().0 + 1..=at {
                    dense.tick(1);
                    // Observe every intermediate edge, including skipped periods.
                    dense.port_read(0x61, 1).unwrap();
                    assert_eq!(dense.clock().0, clock);
                }
                assert_eq!(
                    sparse.port_read(0x61, 1).unwrap(),
                    dense.port_read(0x61, 1).unwrap()
                );
                for timer in [&mut sparse_pit, &mut dense_pit] {
                    timer.device_mut().sync(VClock(at));
                    timer.port_write(0x41, 1, count).unwrap();
                    timer.port_write(0x41, 1, 0).unwrap();
                }
            }
        }
    }

    #[test]
    fn control_nibble_writes_and_reads_back() {
        let mut bus = MockBus::new(Box::new(SysControl::new()));
        // Set gate + speaker + parity-check + iochk enables (low nibble 0b1111).
        let control = GATE | SPEAKER_DATA | PARITY_CHECK_ENABLE | IOCHK_ENABLE;
        bus.port_write(SYS_CONTROL_PORT, 1, u32::from(control))
            .unwrap();
        // The low nibble must read back exactly what was written.
        let read = bus.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8;
        assert_eq!(read & CONTROL_MASK, control);
    }

    #[test]
    fn individual_control_bits_round_trip() {
        for bit in [GATE, SPEAKER_DATA, PARITY_CHECK_ENABLE, IOCHK_ENABLE] {
            let mut dev = SysControl::new();
            dev.port_write(SYS_CONTROL_PORT, 1, u32::from(bit)).unwrap();
            assert_eq!(
                dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8 & CONTROL_MASK,
                bit
            );
        }
    }

    #[test]
    fn high_bits_written_by_guest_are_dropped() {
        let mut dev = SysControl::new();
        // Guest sets every bit; only the low control nibble may survive.
        dev.port_write(SYS_CONTROL_PORT, 1, 0xFF).unwrap();
        // Read back with the volatile status bits masked off.
        let control = dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8 & CONTROL_MASK;
        assert_eq!(control, CONTROL_MASK);
    }

    #[test]
    fn status_error_bits_read_no_error() {
        let mut dev = SysControl::new();
        // Across several reads (covering every phase), the error-status bits stay 0.
        for _ in 0..8 {
            let v = dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8;
            assert_eq!(v & IOCHK_STATUS, 0, "I/O-channel-check must read no error");
            assert_eq!(v & PARITY_STATUS, 0, "parity-check must read no error");
        }
    }

    #[test]
    fn save_restore_round_trip() {
        let mut dev = SysControl::new();
        dev.port_write(SYS_CONTROL_PORT, 1, u32::from(GATE | SPEAKER_DATA))
            .unwrap();
        // Reads at one clock must leave the hardware state unchanged.
        dev.port_read(SYS_CONTROL_PORT, 1).unwrap();
        dev.port_read(SYS_CONTROL_PORT, 1).unwrap();
        dev.port_read(SYS_CONTROL_PORT, 1).unwrap();
        let blob = dev.save();

        let mut restored = SysControl::new();
        restored.restore(&blob).unwrap();
        assert_eq!(restored.control, GATE | SPEAKER_DATA);
        assert_eq!(restored.save(), dev.save());
    }

    #[test]
    fn restore_rejects_bad_snapshot() {
        let mut dev = SysControl::new();
        assert!(matches!(
            dev.restore(&[]).unwrap_err(),
            DeviceError::InvalidSnapshot { .. }
        ));
        // Right length, wrong version byte.
        let mut bad = dev.save();
        bad[0] = SNAPSHOT_VERSION.wrapping_add(1);
        assert!(matches!(
            dev.restore(&bad).unwrap_err(),
            DeviceError::InvalidSnapshot { .. }
        ));
    }

    #[test]
    fn reset_returns_to_power_on() {
        let mut dev = SysControl::new();
        dev.port_write(SYS_CONTROL_PORT, 1, u32::from(CONTROL_MASK))
            .unwrap();
        dev.port_read(SYS_CONTROL_PORT, 1).unwrap();
        dev.reset();
        assert_eq!(dev.control, 0);
        // First post-reset read: refresh bit low, control nibble clear.
        assert_eq!(
            dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8 & REFRESH_DETECT,
            0
        );
    }

    #[test]
    fn port_outside_window_is_unmapped() {
        let mut dev = SysControl::new();
        let err = dev.port_write(0x60, 1, 0).unwrap_err();
        assert_eq!(
            err,
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 0x60,
            }
        );
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut dev = SysControl::new();
        assert!(dev.mmio_read(0x61, 1).is_err());
        assert!(dev.mmio_write(0x61, 1, 0).is_err());
    }

    #[test]
    fn no_irq_is_asserted() {
        let dev = SysControl::new();
        assert!(!dev.irq_pending());
    }

    #[test]
    fn writes_and_reads_surface_on_trace() {
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(
            SysControl::new(),
        )));
        bus.port_write(SYS_CONTROL_PORT, 1, 0x03).unwrap();
        bus.port_read(SYS_CONTROL_PORT, 1).unwrap();
        let events = bus.device_mut().take_trace();
        assert_eq!(events.len(), 2);
        assert!(bus.device_mut().take_trace().is_empty());
    }
}
