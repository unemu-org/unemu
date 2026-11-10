// SPDX-License-Identifier: AGPL-3.0-or-later
//! Empty IBM AT 80287 socket and its motherboard controls.
//!
//! IBM Personal Computer AT Technical Reference (September 1985), System
//! Board, I/O Address Map (p. 1-37), NMI and Coprocessor Controls (p. 1-38),
//! System Interrupts, and Section 2, Coprocessor: F0h clears latched BUSY,
//! F1h resets the coprocessor, and F8h–FFh select the coprocessor. F2h–F7h
//! are not assigned to this circuit. With no 80287 fitted there is no data
//! driver or error source: all assigned reads return the emulator's open-bus
//! byte FFh, all writes are discarded, and IRQ13 stays low. FFh is an open-bus
//! convention, not a coprocessor status register value specified by IBM.
//! F0h/F1h are write-only controls (IBM specifies writing zero); with an empty
//! socket other written values also have no effect. This is an empty socket
//! on the I/O bus only: the pinned CPU core executes ungated x87 instructions
//! in Real16, and FNINIT/FNSTCW stores 037Fh despite this bus configuration.
//! An absent-FPU mode is requested from the CPU core project; instruction
//! gating belongs there. BUSY timing and populated-socket bus behaviour are
//! not implemented here.
//! See `backlog/docs/design/machine-config.md` §Board model.

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// Motherboard control decode with no 80287 installed; never raises IRQ13.
#[derive(Default)]
pub struct Absent80287 {
    now: VClock,
    trace: device::TraceBuffer,
}

impl Absent80287 {
    /// Create an empty socket with inactive BUSY/error outputs.
    pub fn new() -> Self {
        Self::default()
    }

    fn check(port: u16) -> Result<(), DeviceError> {
        if matches!(port, 0xF0 | 0xF1 | 0xF8..=0xFF) {
            Ok(())
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            })
        }
    }

    fn trace_port(&mut self, port: u16, size: u8, dir: AccessDir, value: u32) {
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir,
            value,
        });
    }
}

impl Device for Absent80287 {
    fn name(&self) -> &'static str {
        "absent-80287"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        Self::check(port)?;
        self.trace_port(port, size, AccessDir::Read, 0xFF);
        Ok(0xFF)
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        Self::check(port)?;
        self.trace_port(port, size, AccessDir::Write, value);
        Ok(())
    }
    fn mmio_read(&mut self, addr: u64, _: u8) -> Result<u64, DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn mmio_write(&mut self, addr: u64, _: u8, _: u64) -> Result<(), DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now >= self.now {
            self.now = now;
        }
    }
    fn next_event(&self, _: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn reset(&mut self) {
        self.now = VClock::ZERO;
        self.trace.clear();
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = vec![1];
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }
    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() != 9 || snapshot[0] != 1 {
            return Err(DeviceError::InvalidSnapshot {
                device: "absent-80287",
                reason: "invalid snapshot",
            });
        }
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| snapshot[i + 1])));
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
    fn empty_socket_reads_open_bus_discards_writes_and_never_interrupts() {
        let mut bus = MockBus::new(Box::new(Absent80287::new()));
        for port in [0xF0, 0xF1, 0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD, 0xFE, 0xFF] {
            for value in [0, 0x55, 0xFF] {
                bus.port_write(port, 1, value).unwrap();
                bus.tick(1);
                assert_eq!(bus.port_read(port, 1).unwrap(), 0xFF);
                assert!(!bus.device().irq_pending());
            }
        }
    }

    #[test]
    fn unassigned_ports_and_memory_are_not_decoded() {
        let mut bus = MockBus::new(Box::new(Absent80287::new()));
        for port in [0xEF, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0x100] {
            assert!(matches!(
                bus.port_read(port, 1),
                Err(DeviceError::UnmappedRegister { .. })
            ));
            assert!(matches!(
                bus.port_write(port, 1, 0),
                Err(DeviceError::UnmappedRegister { .. })
            ));
        }
        assert!(bus.mmio_read(0xF0, 1).is_err());
        assert!(bus.mmio_write(0xF0, 1, 0).is_err());
    }
}
