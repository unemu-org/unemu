// SPDX-License-Identifier: AGPL-3.0-or-later
//! System-control port `0x61` (§7.1 device seam).
//!
//! On a PC/AT this port is the read/write control-and-status register historically
//! carried by port B of the 8255 PPI. A stock BIOS touches it for three duties:
//!
//! 1. **Speaker / timer-2 gating** — the low control bits gate PIT channel 2 and
//!    enable the speaker driver, so firmware can beep and time with the timer.
//! 2. **Error-check enables** — two more control bits arm the parity-check and
//!    I/O-channel-check machinery; the matching high status bits report whether an
//!    error latched (this model reports none).
//! 3. **A short-delay reference** — the BIOS `AT_DELAY` routine busy-waits on the
//!    *refresh* status bit (bit 4), which on real hardware toggles at the DRAM
//!    refresh rate, to measure sub-tick delays. See `SysControl::status_byte`
//!    for why that bit must cycle per read here.
//!
//! ## Coverage matrix
//! Modelled:
//! - **Gate** (bit 0) and **speaker-data enable** (bit 1): stored, read back.
//! - **Parity-check enable** (bit 2) and **I/O-channel-check enable** (bit 3):
//!   stored, read back.
//! - **Refresh detect** (bit 4): a deterministic per-read toggle so the BIOS
//!   delay loop terminates (see `SysControl::status_byte`).
//! - **Timer-2 output** (bit 5): a benign deterministic per-read toggle.
//! - **I/O-channel-check status** (bit 6) and **parity-check status** (bit 7):
//!   read back as *no error*.
//!
//! Not modelled:
//! - Real DRAM-refresh timing: the refresh bit is a per-read phase, not derived
//!   from a refresh clock.
//! - Actual speaker audio: the gate and data-enable bits are stored but drive no
//!   sound and are not wired to the PIT channel-2 gate.
//! - Real timer-2 output: bit 5 is a benign toggle, not the PIT channel-2 pin.
//! - Parity / I/O-channel-check NMI generation: the enable bits latch no error
//!   and raise no NMI; the status bits always read clean.
//!
//! Snapshots contain the control nibble, per-read status phase and absolute
//! trace time base. There are no timed deadlines in this compatibility model.
//! See IBM Personal Computer AT Technical Reference, System Timers.

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

/// Bit 4: refresh-detect status. Toggled per read; see `SysControl::status_byte`.
const REFRESH_DETECT: u8 = 0x10;
/// Bit 5: timer-2 output status. A benign per-read toggle.
const TIMER2_OUT: u8 = 0x20;
/// Bit 6: I/O-channel-check status. Always clear here (0 = no error).
const IOCHK_STATUS: u8 = 0x40;
/// Bit 7: parity-check status. Always clear here (0 = no error).
const PARITY_STATUS: u8 = 0x80;

/// Snapshot format version for [`Device::save`] / [`Device::restore`].
const SNAPSHOT_VERSION: u8 = 2;
/// Snapshot length: version, control byte, read counter and absolute trace clock.
const SNAPSHOT_LEN: usize = 1 + 1 + 8 + 8;

/// The PC/AT system-control port device (§7.1).
///
/// Decodes the one port [`SYS_CONTROL_PORT`]. Writes store the low control
/// nibble; reads return that nibble in the low bits plus status in the high bits,
/// with the refresh bit cycled deterministically per read.
#[derive(Default)]
pub struct SysControl {
    /// The guest-written control nibble (bits 0..=3); high bits are always clear.
    control: u8,
    /// Free-running counter bumped on every read of the port, driving the refresh
    /// and timer-2 status bits through their phases. See `SysControl::status_byte`
    /// for why the phase advances per read, not per [`Device::tick`].
    reads: u64,
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

    /// Current value of the port: the stored control nibble plus the status bits.
    ///
    /// The BIOS `AT_DELAY` short-delay routine busy-waits on the **refresh** bit
    /// (bit 4): it reads this port in a tight loop and counts how many iterations
    /// pass before the bit flips, using that as a fine time reference. On real
    /// hardware the bit toggles at the DRAM refresh rate (IBM Personal Computer AT
    /// Technical Reference, "System Timers", p. 1-22). This compatibility model
    /// instead advances the status phase on reads, independently of virtual time,
    /// so both edges remain observable even at a single clock. Scheduling must
    /// preserve that phase until real PIT1/PIT2 wiring is modelled; see
    /// `backlog/docs/design/scheduler.md` § Device migration inventory.
    ///
    /// Phase, in the low bits of the read count taken *before* this read:
    /// - bit 4 (refresh detect) toggles every read — both a 0→1 and a 1→0 edge are
    ///   seen within two reads.
    /// - bit 5 (timer-2 output) toggles every second read — a benign oscillation
    ///   standing in for the PIT channel-2 output pin.
    ///
    /// The error-status bits (6 and 7) always read clean: this model latches no
    /// parity or I/O-channel-check error.
    ///
    fn status_byte(&mut self) -> u8 {
        let count = self.reads;
        self.reads = count.wrapping_add(1);
        let mut value = self.control & CONTROL_MASK;
        if count & 0b01 != 0 {
            value |= REFRESH_DETECT;
        }
        if count & 0b10 != 0 {
            value |= TIMER2_OUT;
        }
        // Bits 6 (IOCHK) and 7 (parity) stay clear: no error is ever latched.
        value &= !(IOCHK_STATUS | PARITY_STATUS);
        value
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
        self.reads = 0;
        self.trace.clear();
        self.now = VClock::ZERO;
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_LEN);
        blob.push(SNAPSHOT_VERSION);
        blob.push(self.control);
        blob.extend_from_slice(&self.reads.to_le_bytes());
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
        let mut counter = [0u8; 8];
        counter.copy_from_slice(&snapshot[2..10]);
        self.reads = u64::from_le_bytes(counter);
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
                // Consecutive reads at one clock retain the refresh/PIT2 phases.
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
    fn refresh_bit_sees_both_edges_within_bounded_reads() {
        // Model the BIOS `AT_DELAY` busy-wait: spin until the refresh bit is high,
        // then spin until it is low again. This isolated test deliberately omits
        // ticks to verify the compatibility model toggles the bit per read.
        let mut dev = SysControl::new();
        let mut reads = 0u32;
        // Wait for the refresh bit to go high.
        loop {
            reads += 1;
            assert!(reads < 100, "refresh bit never went high");
            if dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8 & REFRESH_DETECT != 0 {
                break;
            }
        }
        // Wait for it to go low again (the second edge).
        loop {
            reads += 1;
            assert!(reads < 100, "refresh bit never went low");
            if dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8 & REFRESH_DETECT == 0 {
                break;
            }
        }
    }

    #[test]
    fn refresh_bit_toggles_every_read() {
        let mut dev = SysControl::new();
        let r0 = dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8 & REFRESH_DETECT;
        let r1 = dev.port_read(SYS_CONTROL_PORT, 1).unwrap() as u8 & REFRESH_DETECT;
        assert_ne!(r0, r1, "refresh bit must change between successive reads");
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
        // Advance the read counter so its restored phase is observable too.
        dev.port_read(SYS_CONTROL_PORT, 1).unwrap();
        dev.port_read(SYS_CONTROL_PORT, 1).unwrap();
        dev.port_read(SYS_CONTROL_PORT, 1).unwrap();
        let blob = dev.save();

        let mut restored = SysControl::new();
        restored.restore(&blob).unwrap();
        assert_eq!(restored.control, GATE | SPEAKER_DATA);
        assert_eq!(restored.reads, dev.reads);
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
        assert_eq!(dev.reads, 0);
        // First post-reset read: refresh bit low (count 0), control nibble clear.
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
