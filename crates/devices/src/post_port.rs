// SPDX-License-Identifier: AGPL-3.0-or-later
//! POST checkpoint port (§7.1 device seam).
//!
//! A tiny write-only diagnostic port. Power-on self-test firmware writes a
//! one-byte checkpoint code here at each stage it reaches; on real hardware the
//! byte lights a diagnostic card or a debug LED display. The emulator models it
//! purely as a capture point: every byte the guest writes is recorded on the
//! [`Device`] tracing seam, so a runner reconstructs the POST progress stream
//! through [`Device::take_trace`], the first-class observation seam, never an
//! ad-hoc side channel.
//!
//! The decoded port is configurable: [`PostPort::new`] decodes the test386
//! diagnostic's `0x190`, while a stock PC BIOS emits its checkpoints on the
//! conventional `0x80` and is wired with [`PostPort::with_port`]. The two never
//! run in the same machine, so a single field selects which port the instance
//! answers.
//!
//! This exists so a diagnostic ROM (test386) or a stock BIOS can report how far
//! it executes even when the run ends on an unsupported instruction rather than a
//! clean halt.
//!
//! ## Deviations from real hardware
//! - Read-only-meaningful in one direction: reads return `0xFF` (an open bus /
//!   unimplemented read), matching that no real POST card drives a read value.
//! - No display or latch state beyond the trace: the byte is observed, not held.

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "post-port";

/// The default port decoded by [`PostPort::new`]: the test386 diagnostic's POST
/// port. A stock PC BIOS instead uses `0x80`; see [`PostPort::with_port`].
pub const POST_PORT: u16 = 0x190;

/// Value returned for a read: an unimplemented/open-bus read.
const OPEN_BUS: u32 = 0xFF;

/// The POST checkpoint port device (§7.1).
///
/// Decodes a single, configurable port ([`PostPort::port`]). Writes surface on
/// the tracing seam as [`TraceEvent::Port`] writes to that port; a consumer
/// holding only a `&mut dyn Device` drains them to recover the checkpoint byte
/// stream.
pub struct PostPort {
    /// The one port this instance decodes ([`POST_PORT`] for test386, `0x80` for
    /// a stock BIOS).
    port: u16,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: Vec<TraceEvent>,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl Default for PostPort {
    fn default() -> Self {
        Self::new()
    }
}

impl PostPort {
    /// Create a POST port decoding [`POST_PORT`] (the test386 diagnostic port).
    #[must_use]
    pub fn new() -> Self {
        Self::with_port(POST_PORT)
    }

    /// Create a POST port decoding `port` — for a stock PC BIOS, whose POST
    /// checkpoints land on `0x80` rather than the test386 diagnostic's `0x190`.
    #[must_use]
    pub fn with_port(port: u16) -> Self {
        Self {
            port,
            trace: Vec::new(),
            now: VClock::ZERO,
        }
    }

    /// Reject any port outside this device's single decoded port.
    fn check(&self, port: u16) -> Result<(), DeviceError> {
        if port == self.port {
            Ok(())
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            })
        }
    }
}

impl Device for PostPort {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.check(port)?;
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value: OPEN_BUS,
        });
        Ok(OPEN_BUS)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.check(port)?;
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
        // The POST port is port-mapped only, never memory-mapped.
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
        self.now = now;
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }

    fn irq_pending(&self) -> bool {
        false
    }

    fn irq_vector(&self) -> Option<u8> {
        None
    }

    fn reset(&mut self) {
        self.trace.clear();
        self.now = VClock::ZERO;
    }

    fn save(&self) -> Vec<u8> {
        // Stateless beyond the transient trace; nothing to snapshot.
        Vec::new()
    }

    fn restore(&mut self, _snapshot: &[u8]) -> Result<(), DeviceError> {
        Ok(())
    }

    fn take_trace(&mut self) -> Vec<TraceEvent> {
        std::mem::take(&mut self.trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    #[test]
    fn dormant_matches_per_clock_seeded_accesses_and_long_jumps() {
        let mut reference = MockBus::new(Box::new(PostPort::new()));
        let mut scheduled = MockBus::new(Box::new(PostPort::new()));
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
                reference.port_write(POST_PORT, 1, seed as u32).unwrap();
                scheduled.port_write(POST_PORT, 1, seed as u32).unwrap();
            } else {
                assert_eq!(
                    reference.port_read(POST_PORT, 1).unwrap(),
                    scheduled.port_read(POST_PORT, 1).unwrap()
                );
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
    fn writes_surface_on_trace_in_order() {
        let mut bus = MockBus::new(Box::new(PostPort::new()));
        bus.port_write(POST_PORT, 1, 0x00).unwrap();
        bus.port_write(POST_PORT, 1, 0x01).unwrap();
        bus.port_write(POST_PORT, 1, 0x02).unwrap();

        let bytes: Vec<u8> = bus
            .device_mut()
            .take_trace()
            .into_iter()
            .filter_map(|ev| match ev {
                TraceEvent::Port {
                    port,
                    dir: AccessDir::Write,
                    value,
                    ..
                } if port == POST_PORT => Some(value as u8),
                _ => None,
            })
            .collect();
        assert_eq!(bytes, vec![0x00, 0x01, 0x02]);
    }

    #[test]
    fn trace_is_drained_on_take() {
        let mut port = PostPort::new();
        port.port_write(POST_PORT, 1, 0x42).unwrap();
        assert_eq!(port.take_trace().len(), 1);
        assert!(port.take_trace().is_empty());
    }

    #[test]
    fn port_outside_window_is_unmapped() {
        let mut port = PostPort::new();
        let err = port.port_write(0x191, 1, 0).unwrap_err();
        assert_eq!(
            err,
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 0x191,
            }
        );
    }

    #[test]
    fn read_returns_open_bus() {
        let mut port = PostPort::new();
        assert_eq!(port.port_read(POST_PORT, 1).unwrap(), 0xFF);
    }

    #[test]
    fn with_port_decodes_the_bios_post_port() {
        // A BIOS-configured instance answers 0x80 and rejects the test386 0x190.
        let mut port = PostPort::with_port(0x80);
        port.port_write(0x80, 1, 0x42).unwrap();
        let bytes: Vec<u8> = port
            .take_trace()
            .into_iter()
            .filter_map(|ev| match ev {
                TraceEvent::Port {
                    dir: AccessDir::Write,
                    value,
                    ..
                } => Some(value as u8),
                _ => None,
            })
            .collect();
        assert_eq!(bytes, vec![0x42]);
        assert_eq!(
            port.port_write(POST_PORT, 1, 0).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(POST_PORT),
            }
        );
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut port = PostPort::new();
        assert!(port.mmio_read(0x190, 1).is_err());
        assert!(port.mmio_write(0x190, 1, 0).is_err());
    }
}
