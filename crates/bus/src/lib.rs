// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bus swap seam (§3.2 seam 3, §3.3, §4.1).
//!
//! The [`Bus`] **executes** a decode map the `chipset` **declares** — the
//! "Chipset declares, Bus executes" rule (§3.3). It owns the set of
//! [`Device`]s, a table of port ranges → device index, and a table of MMIO
//! ranges → device index, and dispatches guest accesses to the right device. It
//! holds **no** decode *policy*: which ports/addresses map where is the
//! chipset's business.
//!
//! It also drives [`Bus::tick_all`] (advance every device on the virtual clock,
//! §6) and [`Bus::poll_irqs`] (aggregate pending IRQ lines, §5.1).

use std::ops::RangeInclusive;

use device::{AccessDir, Device, DeviceError, TraceEvent, VClock};
use thiserror::Error;

/// A contiguous port range routed to one device (§4.1).
pub struct PortRange {
    /// Inclusive 16-bit port range this device decodes.
    pub ports: RangeInclusive<u16>,
    /// Index into the [`Bus`]'s device vector.
    pub device: usize,
}

/// A contiguous MMIO address range routed to one device (§4.1).
pub struct MmioRange {
    /// Inclusive guest-physical address range this device decodes.
    pub addrs: RangeInclusive<u64>,
    /// Index into the [`Bus`]'s device vector.
    pub device: usize,
}

/// The decode map a chipset hands the bus (§3.3).
///
/// This is pure data: the chipset declares it, the [`Bus`] executes it. There is
/// no decode logic here — only the ranges and the device indices they resolve
/// to.
#[derive(Default)]
pub struct DecodeMap {
    /// Port-space routing table.
    pub ports: Vec<PortRange>,
    /// MMIO-space routing table.
    pub mmio: Vec<MmioRange>,
}

/// What the bus does with a port access no device decodes (§4.1).
///
/// On a real ISA machine the data bus is not actively driven for an undecoded
/// port: a read floats to all-ones (0xFF per byte — "open bus"), and a write
/// simply has no listener and is dropped. A stock BIOS relies on this — its POST
/// pokes many ports that a minimally-populated board never decodes (the DMA page
/// latches, the CGA registers on a machine with no CGA, a floppy-controller
/// probe, spare serial/parallel probes) and must not fault on any of them.
///
/// [`OpenBus`](UndecodedPortAccess::OpenBus) models that hardware behaviour and
/// is the default. [`Fault`](UndecodedPortAccess::Fault) instead raises
/// [`BusError::UnmappedPort`], for diagnostics or tests that want every port
/// access to resolve to a real device.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum UndecodedPortAccess {
    /// Read returns all-ones, write is dropped; both are traced. The hardware
    /// default an unmodified BIOS depends on.
    #[default]
    OpenBus,
    /// Any undecoded port access raises [`BusError::UnmappedPort`].
    Fault,
}

/// Errors raised while executing the decode (§4.1).
#[derive(Debug, Error)]
pub enum BusError {
    /// No device decodes this port.
    #[error("unmapped port {port:#06x}")]
    UnmappedPort {
        /// The undecoded 16-bit port.
        port: u16,
    },
    /// No device decodes this MMIO address.
    #[error("unmapped mmio address {addr:#x}")]
    UnmappedMmio {
        /// The undecoded guest-physical address.
        addr: u64,
    },
    /// The target device raised an error servicing the access.
    #[error("device error: {0}")]
    Device(#[from] DeviceError),
}

/// The bus: owns devices + the decode map and dispatches accesses (§3.3).
pub struct Bus {
    devices: Vec<Box<dyn Device>>,
    map: DecodeMap,
    /// Policy for a port access no device decodes (§4.1). Open-bus by default.
    undecoded_ports: UndecodedPortAccess,
    /// Virtual time of the most recent [`Bus::tick_all`], stamped onto open-bus
    /// traces so they carry the same deterministic clock as device traces.
    now: VClock,
    /// Open-bus accesses observed since the last [`Bus::take_open_bus_trace`].
    ///
    /// An undecoded port has no device to record it on the [`Device`] tracing
    /// seam, yet a BIOS-boot golden still wants to see which stray ports were
    /// poked. The bus therefore keeps its own trace of the accesses it absorbs.
    open_bus_trace: Vec<TraceEvent>,
}

impl Bus {
    /// Build a bus from a device set and the chipset-declared decode map.
    ///
    /// The `map`'s device indices must reference `devices`; the chipset is
    /// responsible for keeping them consistent (§3.3). Undecoded port accesses
    /// default to open-bus ([`UndecodedPortAccess::OpenBus`]); use
    /// [`Bus::with_undecoded_ports`] to select strict faulting instead.
    pub fn new(devices: Vec<Box<dyn Device>>, map: DecodeMap) -> Self {
        Self {
            devices,
            map,
            undecoded_ports: UndecodedPortAccess::default(),
            now: VClock::ZERO,
            open_bus_trace: Vec::new(),
        }
    }

    /// Set how the bus handles a port access no device decodes, returning the
    /// bus for chaining. Defaults to [`UndecodedPortAccess::OpenBus`].
    #[must_use]
    pub fn with_undecoded_ports(mut self, policy: UndecodedPortAccess) -> Self {
        self.undecoded_ports = policy;
        self
    }

    /// The current undecoded-port policy (§4.1).
    pub fn undecoded_ports(&self) -> UndecodedPortAccess {
        self.undecoded_ports
    }

    /// Drain the open-bus accesses absorbed since the last call.
    ///
    /// Mirrors [`Device::take_trace`] for accesses that never reached a device:
    /// each undecoded port read/write the bus serviced under
    /// [`UndecodedPortAccess::OpenBus`] appears here as a [`TraceEvent::Port`].
    pub fn take_open_bus_trace(&mut self) -> Vec<TraceEvent> {
        std::mem::take(&mut self.open_bus_trace)
    }

    /// Resolve a port to its device index, or `None` if undecoded.
    fn port_device(&self, port: u16) -> Option<usize> {
        self.map
            .ports
            .iter()
            .find(|r| r.ports.contains(&port))
            .map(|r| r.device)
    }

    /// Resolve an MMIO address to its device index, or `None` if undecoded.
    fn mmio_device(&self, addr: u64) -> Option<usize> {
        self.map
            .mmio
            .iter()
            .find(|r| r.addrs.contains(&addr))
            .map(|r| r.device)
    }

    /// Dispatch a guest `in` at `port`, `size` bytes wide.
    ///
    /// An undecoded port under [`UndecodedPortAccess::OpenBus`] returns all-ones
    /// (0xFF per byte) and is recorded on the open-bus trace, matching a real
    /// ISA bus that no device drives; under [`UndecodedPortAccess::Fault`] it
    /// raises [`BusError::UnmappedPort`].
    pub fn port_read(&mut self, port: u16, size: u8) -> Result<u32, BusError> {
        match self.port_device(port) {
            Some(idx) => Ok(self.devices[idx].port_read(port, size)?),
            None => match self.undecoded_ports {
                UndecodedPortAccess::OpenBus => {
                    let value = open_bus_value(size);
                    self.trace_open_bus(port, size, AccessDir::Read, value);
                    Ok(value)
                }
                UndecodedPortAccess::Fault => Err(BusError::UnmappedPort { port }),
            },
        }
    }

    /// Dispatch a guest `out` of `value` at `port`, `size` bytes wide.
    ///
    /// An undecoded port under [`UndecodedPortAccess::OpenBus`] drops the write
    /// (no listener drives the bus) and records it on the open-bus trace; under
    /// [`UndecodedPortAccess::Fault`] it raises [`BusError::UnmappedPort`].
    pub fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), BusError> {
        match self.port_device(port) {
            Some(idx) => Ok(self.devices[idx].port_write(port, size, value)?),
            None => match self.undecoded_ports {
                UndecodedPortAccess::OpenBus => {
                    self.trace_open_bus(port, size, AccessDir::Write, value);
                    Ok(())
                }
                UndecodedPortAccess::Fault => Err(BusError::UnmappedPort { port }),
            },
        }
    }

    /// Record an open-bus port access on the bus's own trace, stamped with the
    /// current virtual time (§6) — never wall time, so goldens stay reproducible.
    fn trace_open_bus(&mut self, port: u16, size: u8, dir: AccessDir, value: u32) {
        self.open_bus_trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir,
            value,
        });
    }

    /// Dispatch a guest MMIO read at `addr`, `size` bytes wide.
    ///
    /// The open-bus policy is deliberately **port-only**. An undecoded MMIO
    /// address is a memory-map concern, not a floating ISA data line: a BIOS
    /// machine covers its physical space with mapped RAM and ROM regions so
    /// there is no undecoded memory to float, and a stray MMIO trap that reaches
    /// the bus with no device is a real wiring error worth surfacing.
    pub fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, BusError> {
        let idx = self
            .mmio_device(addr)
            .ok_or(BusError::UnmappedMmio { addr })?;
        Ok(self.devices[idx].mmio_read(addr, size)?)
    }

    /// Dispatch a guest MMIO write of `value` at `addr`, `size` bytes wide.
    pub fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), BusError> {
        let idx = self
            .mmio_device(addr)
            .ok_or(BusError::UnmappedMmio { addr })?;
        Ok(self.devices[idx].mmio_write(addr, size, value)?)
    }

    /// Advance every device to virtual time `now` (§6).
    pub fn tick_all(&mut self, now: VClock) {
        self.now = now;
        for dev in &mut self.devices {
            dev.tick(now);
        }
    }

    /// Aggregate pending IRQ lines into the vector each device would drive on
    /// INTA (§5.1). No interrupt controller is wired up yet, so the machine loop
    /// does not act on this; the method exists so the seam is present.
    pub fn poll_irqs(&self) -> Vec<u8> {
        self.devices
            .iter()
            .filter(|d| d.irq_pending())
            .filter_map(|d| d.irq_vector())
            .collect()
    }

    /// The current IRQ-line level of the device at `index`, or `None` if the
    /// index is out of range (§5.1).
    ///
    /// This is the raw line level (`irq_pending`), not an edge: the integration
    /// layer edge-detects it before pulsing the interrupt controller, since a
    /// device (the PIT) drives a *level* while the PIC is edge-triggered. The
    /// machine names a device only by the wiring index the chipset declared, not
    /// by a concrete type.
    pub fn device_irq_line(&self, index: usize) -> Option<bool> {
        self.devices.get(index).map(|d| d.irq_pending())
    }

    /// Borrow a device by index (for tests and CLI serial draining).
    pub fn device(&self, index: usize) -> Option<&dyn Device> {
        self.devices.get(index).map(|b| b.as_ref())
    }

    /// Mutably borrow a device by index.
    pub fn device_mut(&mut self, index: usize) -> Option<&mut Box<dyn Device>> {
        self.devices.get_mut(index)
    }
}

/// The value an open-bus read of `size` bytes floats to: all-ones in every
/// byte, which a stock BIOS reads as "nothing here" (0xFF/0xFFFF/0xFFFFFFFF).
fn open_bus_value(size: u8) -> u32 {
    match size {
        1 => 0x0000_00FF,
        2 => 0x0000_FFFF,
        _ => 0xFFFF_FFFF,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device decoding a single port, so the decoded path can be exercised
    /// alongside the undecoded open-bus path.
    struct OnePortDevice;

    impl Device for OnePortDevice {
        fn name(&self) -> &'static str {
            "one-port"
        }
        fn port_read(&mut self, _port: u16, _size: u8) -> Result<u32, DeviceError> {
            Ok(0x42)
        }
        fn port_write(&mut self, _port: u16, _size: u8, _value: u32) -> Result<(), DeviceError> {
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
        fn tick(&mut self, _now: VClock) {}
        fn irq_pending(&self) -> bool {
            false
        }
        fn irq_vector(&self) -> Option<u8> {
            None
        }
        fn reset(&mut self) {}
        fn save(&self) -> Vec<u8> {
            Vec::new()
        }
        fn restore(&mut self, _snapshot: &[u8]) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    fn bus_decoding(port: u16) -> Bus {
        let devices: Vec<Box<dyn Device>> = vec![Box::new(OnePortDevice)];
        let map = DecodeMap {
            ports: vec![PortRange {
                ports: port..=port,
                device: 0,
            }],
            mmio: Vec::new(),
        };
        Bus::new(devices, map)
    }

    #[test]
    fn open_bus_is_the_default_policy() {
        assert_eq!(
            bus_decoding(0x60).undecoded_ports(),
            UndecodedPortAccess::OpenBus
        );
    }

    #[test]
    fn undecoded_read_floats_to_all_ones() {
        let mut bus = bus_decoding(0x60);
        assert_eq!(bus.port_read(0x80, 1).unwrap(), 0xFF);
        assert_eq!(bus.port_read(0x80, 2).unwrap(), 0xFFFF);
        assert_eq!(bus.port_read(0x80, 4).unwrap(), 0xFFFF_FFFF);
    }

    #[test]
    fn undecoded_write_is_a_no_op() {
        let mut bus = bus_decoding(0x60);
        // No device to fault; the write is absorbed.
        bus.port_write(0x3D8, 1, 0xAB).unwrap();
    }

    #[test]
    fn open_bus_accesses_are_traced_with_the_virtual_clock() {
        let mut bus = bus_decoding(0x60);
        bus.tick_all(VClock(7));
        bus.port_write(0x80, 1, 0xAB).unwrap();
        let _ = bus.port_read(0x3DA, 1).unwrap();

        let trace = bus.take_open_bus_trace();
        assert_eq!(
            trace,
            vec![
                TraceEvent::Port {
                    at: VClock(7),
                    port: 0x80,
                    size: 1,
                    dir: AccessDir::Write,
                    value: 0xAB,
                },
                TraceEvent::Port {
                    at: VClock(7),
                    port: 0x3DA,
                    size: 1,
                    dir: AccessDir::Read,
                    value: 0xFF,
                },
            ]
        );
        // The trace is drained on take.
        assert!(bus.take_open_bus_trace().is_empty());
    }

    #[test]
    fn decoded_ports_bypass_open_bus_and_are_not_traced() {
        let mut bus = bus_decoding(0x60);
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x42);
        bus.port_write(0x60, 1, 0x99).unwrap();
        assert!(bus.take_open_bus_trace().is_empty());
    }

    #[test]
    fn strict_mode_still_faults_on_undecoded_ports() {
        let mut bus = bus_decoding(0x60).with_undecoded_ports(UndecodedPortAccess::Fault);
        assert!(matches!(
            bus.port_read(0x80, 1),
            Err(BusError::UnmappedPort { port: 0x80 })
        ));
        assert!(matches!(
            bus.port_write(0x80, 1, 0),
            Err(BusError::UnmappedPort { port: 0x80 })
        ));
        // A fault leaves nothing on the open-bus trace.
        assert!(bus.take_open_bus_trace().is_empty());
    }
}
