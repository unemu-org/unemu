// SPDX-License-Identifier: AGPL-3.0-or-later
//! Emulator-only diagnostic console, not period hardware.
//!
//! Protocol reference: QEMU System Emulator, debugcon option and isa-debugcon
//! device. Its one-byte port emits written bytes and defaults to E9h readback.
//! SeaBIOS, qemu_debug_preinit / qemu_debug_putc, probes for E9h and uses OUT
//! bytes at 402h before a serial console is available. Unlike QEMU's default
//! E9h address, this device defaults to SeaBIOS's 402h; it is configurable.
//! See `backlog/docs/design/firmware-and-machine-classes.md` §3.6 and
//! `backlog/docs/design/observation.md` §Debug console stream.
//!
//! Direct word/dword device accesses are rejected: QEMU's isa-debugcon declares
//! byte-only valid accesses. Our integration policy assumes the existing ISA
//! bus splits wider guest operations into consecutive byte cycles, of which
//! only the configured port belongs to this device. No QEMU wider-dispatch
//! equivalence is assumed. There is no IRQ, MMIO, host I/O or output history.
//! Observation owns the sink; reset keeps it and restore never replays bytes.
//! Snapshots retain only the virtual trace clock; port wiring stays with the
//! receiving instance and access traces remain outside the snapshot.

use device::{
    AccessDir, ByteOutput, ByteSink, ByteStreamKind, Device, DeviceError, DiscardByteSink,
    NextEvent, TraceBuffer, TraceEvent, TracePolicy, VClock,
};

/// SeaBIOS's default debug-output port.
pub const DEBUG_CONSOLE_PORT: u16 = 0x402;
const READBACK: u32 = 0xE9;
const NAME: &str = "debug-console";
const SNAPSHOT_VERSION: u8 = 1;

/// A single-byte emulator diagnostic port using the QEMU debugcon protocol.
///
/// The discarding default sink allocates no history. See
/// `backlog/docs/design/observation.md` §Debug console stream for capture bounds.
pub struct DebugConsole {
    port: u16,
    sink: Box<dyn ByteSink>,
    trace: TraceBuffer,
    now: VClock,
}

impl Default for DebugConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugConsole {
    /// Create the diagnostic port at SeaBIOS's default 402h address.
    #[must_use]
    pub fn new() -> Self {
        Self::with_port(DEBUG_CONSOLE_PORT)
    }

    /// Relocate the one-byte port for a bare test rig.
    #[must_use]
    pub fn with_port(port: u16) -> Self {
        Self {
            port,
            sink: Box::new(DiscardByteSink),
            trace: TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    fn check(&self, port: u16, size: u8) -> Result<(), DeviceError> {
        if port != self.port {
            return Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            });
        }
        if size != 1 {
            return Err(DeviceError::UnmappedRegister {
                kind: "port width",
                offset: u64::from(port),
            });
        }
        Ok(())
    }
}

impl ByteOutput for DebugConsole {
    fn kind(&self) -> ByteStreamKind {
        ByteStreamKind::DebugConsole
    }

    fn bind_sink(&mut self, sink: Box<dyn ByteSink>) {
        self.sink = sink;
    }
}

impl Device for DebugConsole {
    fn name(&self) -> &'static str {
        NAME
    }

    fn byte_output(&mut self) -> Option<&mut dyn ByteOutput> {
        Some(self)
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.check(port, size)?;
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value: READBACK,
        });
        Ok(READBACK)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.check(port, size)?;
        self.sink.push(value as u8);
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
        if now >= self.now {
            self.now = now;
        }
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }

    fn irq_pending(&self) -> bool {
        false
    }

    fn reset(&mut self) {
        self.trace.clear();
        self.now = VClock::ZERO;
    }

    fn save(&self) -> Vec<u8> {
        let mut snapshot = vec![SNAPSHOT_VERSION];
        snapshot.extend_from_slice(&self.now.0.to_le_bytes());
        snapshot
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() != 9 || snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot format",
            });
        }
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| snapshot[1 + i])));
        Ok(())
    }

    fn set_trace_policy(&mut self, policy: TracePolicy) {
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
    use device::{ByteCapture, DeviceError, NextEvent, VClock};
    use testkit::MockBus;

    fn rig(port: u16) -> (MockBus, ByteCapture) {
        let console = if port == DEBUG_CONSOLE_PORT {
            DebugConsole::default()
        } else {
            DebugConsole::with_port(port)
        };
        let mut bus = MockBus::new(Box::new(console));
        let capture = ByteCapture::new(32);
        let output = bus.device_mut().byte_output().unwrap();
        assert_eq!(output.kind(), device::ByteStreamKind::DebugConsole);
        output.bind_sink(Box::new(capture.clone()));
        (bus, capture)
    }

    #[test]
    fn readback_enables_seabios_without_emitting_bytes() {
        let (mut bus, capture) = rig(DEBUG_CONSOLE_PORT);
        assert_eq!(bus.port_read(0x402, 1).unwrap(), 0xE9);
        assert_eq!(bus.port_read(0x402, 1).unwrap(), 0xE9);
        assert!(capture.drain().unwrap().is_empty());
    }

    #[test]
    fn writes_emit_ordered_bytes_independently_of_access_traces() {
        let (mut bus, capture) = rig(DEBUG_CONSOLE_PORT);
        for byte in [b'H', b'i', b'\n', 0, 0xFF, 0xFF] {
            bus.port_write(0x402, 1, u32::from(byte)).unwrap();
        }
        assert!(bus.device_mut().take_trace().is_empty());
        assert_eq!(capture.drain().unwrap(), [b'H', b'i', b'\n', 0, 0xFF, 0xFF]);
        assert!(capture.drain().unwrap().is_empty());
    }

    #[test]
    fn other_ports_are_unmapped_and_emit_nothing() {
        let (mut bus, capture) = rig(DEBUG_CONSOLE_PORT);
        for port in [0x401, 0x403, 0xE9] {
            assert_eq!(
                bus.port_read(port, 1),
                Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                })
            );
            assert_eq!(
                bus.port_write(port, 1, 7),
                Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                })
            );
        }
        assert!(capture.drain().unwrap().is_empty());
    }

    #[test]
    fn configured_port_replaces_the_default_decode() {
        let (mut bus, capture) = rig(0xE9);
        assert_eq!(bus.port_read(0xE9, 1).unwrap(), 0xE9);
        bus.port_write(0xE9, 1, u32::from(b'X')).unwrap();
        assert!(bus.port_read(0x402, 1).is_err());
        assert!(bus.port_write(0x402, 1, 7).is_err());
        assert_eq!(capture.drain().unwrap(), b"X");
    }

    #[test]
    fn direct_wide_cycles_are_rejected_without_emission() {
        let (mut bus, capture) = rig(DEBUG_CONSOLE_PORT);
        for size in [2, 4] {
            assert!(bus.port_read(0x402, size).is_err());
            assert!(bus.port_write(0x402, size, 0x44434241).is_err());
        }
        assert!(capture.drain().unwrap().is_empty());
    }

    #[test]
    fn reset_and_restore_preserve_sink_without_replaying_history() {
        let (mut bus, capture) = rig(DEBUG_CONSOLE_PORT);
        bus.tick(17);
        assert_eq!(bus.device().next_event(VClock(17)), NextEvent::Dormant);
        assert!(!bus.device().irq_pending());
        assert!(bus.mmio_read(0x402, 1).is_err());
        assert!(bus.mmio_write(0x402, 1, 0).is_err());
        let snapshot = bus.device().save();
        bus.port_write(0x402, 1, u32::from(b'A')).unwrap();
        bus.device_mut().reset();
        bus.device_mut().restore(&snapshot).unwrap();
        assert_eq!(bus.device().save(), snapshot);
        bus.port_write(0x402, 1, u32::from(b'B')).unwrap();
        assert_eq!(capture.drain().unwrap(), b"AB");
        assert!(bus.device_mut().restore(&[]).is_err());
    }
}
