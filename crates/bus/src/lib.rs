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

use device::{Device, DeviceError, VClock};
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
}

impl Bus {
    /// Build a bus from a device set and the chipset-declared decode map.
    ///
    /// The `map`'s device indices must reference `devices`; the chipset is
    /// responsible for keeping them consistent (§3.3).
    pub fn new(devices: Vec<Box<dyn Device>>, map: DecodeMap) -> Self {
        Self { devices, map }
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
    pub fn port_read(&mut self, port: u16, size: u8) -> Result<u32, BusError> {
        let idx = self
            .port_device(port)
            .ok_or(BusError::UnmappedPort { port })?;
        Ok(self.devices[idx].port_read(port, size)?)
    }

    /// Dispatch a guest `out` of `value` at `port`, `size` bytes wide.
    pub fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), BusError> {
        let idx = self
            .port_device(port)
            .ok_or(BusError::UnmappedPort { port })?;
        Ok(self.devices[idx].port_write(port, size, value)?)
    }

    /// Dispatch a guest MMIO read at `addr`, `size` bytes wide.
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
