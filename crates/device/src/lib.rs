// SPDX-License-Identifier: AGPL-3.0-or-later
//! Device swap seam (§3.2, §7.1).
//!
//! [`Device`] is the fourth of the six swap seams from
//! `backlog/docs/architecture.md`: it is the contract every emulated piece of
//! hardware (16550 UART, 8259 PIC, 8253 PIT, 8042, …) implements. The `machine`
//! core and the `bus` never name a concrete device — they only see this trait.
//!
//! The contract carries **port** and **MMIO** access, [`Device::tick`] on the
//! virtual clock, an IRQ line query, [`Device::reset`], `save`/`restore`, and
//! trace emission — present on **every** device from the start (see spec §7.1),
//! so snapshotting and golden tracing never have to be retrofitted.

use thiserror::Error;

/// Virtual time base (see spec §6.1).
///
/// A monotonic counter shared by every device's [`Device::tick`], measured in
/// PIT input clocks. The machine advances it from the CPU's retired-instruction
/// delta by a fixed documented ratio (§6, virtual-clock model), never from wall
/// time — so device timing and every golden are reproducible.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VClock(pub u64);

impl VClock {
    /// The zero instant (machine reset).
    pub const ZERO: VClock = VClock(0);

    /// Advance the clock by `ticks` and return the new value.
    #[must_use]
    pub fn advance(self, ticks: u64) -> VClock {
        VClock(self.0.wrapping_add(ticks))
    }
}

/// Direction of a data transfer, shared by port and MMIO access paths.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AccessDir {
    /// Guest reads from the device.
    Read,
    /// Guest writes to the device.
    Write,
}

/// A single observed device event, drained by [`Device::take_trace`] (§3.2
/// tracing seam, §7.1).
///
/// Tracing is a **first-class seam**: golden trace tests read these events from
/// the `Device` contract, never from ad-hoc `println!`s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// A serviced port-I/O access.
    Port {
        /// Virtual time at which the access was serviced.
        at: VClock,
        /// 16-bit port number.
        port: u16,
        /// Access width in bytes (1/2/4).
        size: u8,
        /// Read or write.
        dir: AccessDir,
        /// Value written by the guest, or read back to it (low `size` bytes).
        value: u32,
    },
    /// A serviced MMIO access.
    Mmio {
        /// Virtual time at which the access was serviced.
        at: VClock,
        /// Guest physical address.
        addr: u64,
        /// Access width in bytes.
        size: u8,
        /// Read or write.
        dir: AccessDir,
        /// Value written by the guest, or read back to it.
        value: u64,
    },
}

/// Errors a device may raise from the [`Device`] contract (§7.1).
///
/// Stable, `thiserror`-based library error surface: bus faults and device
/// state-restore failures. Library code never `unwrap`s — it returns one of
/// these.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeviceError {
    /// An access hit a register the device does not decode.
    #[error("unmapped device register: {kind} access at {offset:#x}")]
    UnmappedRegister {
        /// Human-readable access kind ("port"/"mmio").
        kind: &'static str,
        /// Offset within the device's window.
        offset: u64,
    },
    /// [`Device::restore`] was handed a snapshot it cannot interpret.
    #[error("invalid snapshot for device {device}: {reason}")]
    InvalidSnapshot {
        /// The device's [`Device::name`].
        device: &'static str,
        /// Why the blob was rejected.
        reason: &'static str,
    },
}

/// A guest-memory handle for DMA transfers (§7.7).
///
/// A DMA controller moves bytes between an I/O device and guest RAM, but no
/// device may hold a reference into that RAM: writes must run through the path
/// that fires the JIT's `note_write` hook, or self-modifying-code tracking stops
/// invalidating translated blocks the transfer overwrites (see the memory model
/// in `backlog/docs/architecture.md`, §4.3). The machine implements this trait
/// over the CPU's `Vm` API and hands it to the controller per transfer; unit
/// tests implement it over a plain buffer. Addresses are full 24-bit physical
/// addresses (a channel's page latch combined with its 16-bit offset).
pub trait DmaMemory {
    /// Read one byte of guest memory at physical `addr`.
    fn dma_read(&self, addr: u32) -> u8;

    /// Write one byte of guest memory at physical `addr` (must run `note_write`).
    fn dma_write(&mut self, addr: u32, value: u8);
}

/// A device→memory DMA transfer a requesting device (the floppy controller) has
/// queued for the machine to run through the DMA controller (§7.7).
///
/// The requester produces the bytes (a floppy read pulls them from its backing
/// image) and names the DMA channel they belong to; it does **not** name the
/// destination address — that lives in the DMA controller's channel registers,
/// which the guest programmed. The machine pulls this with
/// [`Device::take_dma_request`], streams `data` through the controller's
/// [`Device::dma_service_write`] (which writes each byte via [`DmaMemory`] and
/// advances the channel), then reports the byte count back with
/// [`Device::dma_complete`] so the requester can finish its result phase and
/// raise its completion interrupt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaRequest {
    /// The DMA channel the transfer runs on (channel 2 for the PC floppy).
    pub channel: u8,
    /// The bytes to move into guest memory, in order.
    pub data: Vec<u8>,
}

/// The device swap seam (§3.2 seam 4, §7.1).
///
/// Every emulated chip implements this. Method contracts:
///
/// - [`Device::port_read`] / [`Device::port_write`]: ISA port-space access at a
///   16-bit `port`, `size` bytes wide (1/2/4). Backs `IN`/`OUT`.
/// - [`Device::mmio_read`] / [`Device::mmio_write`]: memory-mapped access at a
///   guest physical `addr`, `size` bytes. Backs `RegionKind::Trap` loads/stores.
/// - [`Device::tick`]: advance internal timers/state to virtual time `now`
///   (§6). Called on `run()` boundaries; no wall-clock.
/// - [`Device::irq_pending`] / [`Device::irq_vector`]: the device's IRQ line and
///   the vector it would drive on INTA (§5). The machine edge-detects a device's
///   line onto the interrupt controller and injects the acknowledged vector into
///   the CPU (see the `machine` crate).
/// - [`Device::reset`]: return to power-on register state.
/// - [`Device::save`] / [`Device::restore`]: snapshot/replay (§7).
/// - [`Device::take_trace`]: drain observed events for golden tests.
pub trait Device {
    /// Stable, human-readable chip name (e.g. `"16550-uart"`). Used in traces
    /// and errors.
    fn name(&self) -> &'static str;

    /// Read `size` bytes from port `port`. Returns the value in the low bytes.
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError>;

    /// Write the low `size` bytes of `value` to port `port`.
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError>;

    /// Read `size` bytes from MMIO address `addr`.
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError>;

    /// Write the low `size` bytes of `value` to MMIO address `addr`.
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError>;

    /// Advance internal state to virtual time `now` (§6).
    fn tick(&mut self, now: VClock);

    /// Whether the device is currently asserting its IRQ line (§5.1).
    fn irq_pending(&self) -> bool;

    /// The interrupt vector this device drives on INTA when its line is pending
    /// (§5.4). `None` if no line is asserted.
    fn irq_vector(&self) -> Option<u8>;

    /// Return to power-on register state.
    fn reset(&mut self);

    /// Serialize device state for snapshot/replay (§7, day-one).
    fn save(&self) -> Vec<u8>;

    /// Restore device state from a [`Device::save`] blob.
    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError>;

    /// A queued device→memory DMA transfer, if this device (a bus master such as
    /// the floppy controller) has one pending (§7.7).
    ///
    /// The machine drains this each boundary and, for a device wired to a DMA
    /// controller, runs the returned bytes through the controller's
    /// [`Device::dma_service_write`] before delivering the byte count back with
    /// [`Device::dma_complete`]. Default `None`: a device that never masters the
    /// bus has nothing to hand over.
    fn take_dma_request(&mut self) -> Option<DmaRequest> {
        None
    }

    /// Finish a DMA transfer the machine ran on this device's behalf: `transferred`
    /// bytes reached memory (§7.7). The requester advances its transfer state,
    /// fills its result phase, and raises its completion interrupt. Default no-op.
    fn dma_complete(&mut self, transferred: usize) {
        let _ = transferred;
    }

    /// Act as the DMA controller for a device→memory transfer: write `data` to
    /// guest memory through `mem` on `channel`, advancing the channel's address
    /// and count, and return the number of bytes moved (§7.7).
    ///
    /// Only a DMA controller (the 8237) overrides this; every other device
    /// inherits the no-op default that moves nothing.
    fn dma_service_write(&mut self, channel: u8, mem: &mut dyn DmaMemory, data: &[u8]) -> usize {
        let _ = (channel, mem, data);
        0
    }

    /// Drain and return the trace events observed since the last call (§3.2
    /// tracing seam). Empty `Vec` by default for devices that do not trace.
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        Vec::new()
    }

    /// Downcast escape hatch for host-side driving of a concrete device, when the
    /// machine core only holds `&mut dyn Device`. It exists for out-of-band input
    /// the bus cannot express — e.g. the frontend injecting keyboard scancodes
    /// into the 8042 mid-run, mirroring bytes arriving from a real keyboard.
    ///
    /// Defaults to `None`: a device opts in by returning `Some(self)`. It is not a
    /// guest-visible seam and must never be used from the emulated execution path;
    /// only the embedding frontend reaches for it.
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        None
    }
}

/// The interrupt-controller seam between the device IRQ lines and the CPU (§5).
///
/// A real machine routes device IRQ lines through an 8259 PIC, which aggregates
/// them into a single INTR to the CPU and answers the CPU's INTA cycle with an
/// interrupt vector (§5.1, §5.4). This trait is that seam: devices (the PIT, the
/// keyboard controller, …) drive input lines with [`InterruptController::set_irq_line`],
/// and the CPU integration polls, acknowledges and end-of-interrupts through the
/// remaining methods.
///
/// The trait is deliberately minimal and lives here, next to [`Device`], so both
/// the concrete controller device and the `chipset` seam can name it without a
/// heavier dependency. The `machine`/`cpu` wiring — asking for a vector on INTR
/// and injecting it — is later integration work; this trait only fixes the shape
/// that wiring will call.
pub trait InterruptController {
    /// Drive input IRQ line `irq` to `level` (`true` = asserted).
    ///
    /// This is how devices raise and lower their request lines. Whether a rising
    /// level actually latches a request depends on the controller's trigger mode
    /// and mask.
    fn set_irq_line(&mut self, irq: u8, level: bool);

    /// The vector the controller would dispatch on INTA right now, if any: the
    /// highest-priority pending, unmasked, not-already-in-service request
    /// resolved to its vector (§5.4). Does not change controller state.
    fn poll(&self) -> Option<u8>;

    /// Perform an INTA (interrupt-acknowledge) cycle: latch the highest-priority
    /// serviceable request into the in-service state and return its vector, or
    /// `None` if nothing is serviceable (§5.4).
    fn acknowledge(&mut self) -> Option<u8>;

    /// End-of-interrupt: clear the highest-priority in-service request, allowing
    /// lower-priority requests to be serviced again (§5.4).
    fn end_of_interrupt(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vclock_advances_monotonically() {
        let t0 = VClock::ZERO;
        let t1 = t0.advance(5);
        let t2 = t1.advance(3);
        assert!(t0 < t1 && t1 < t2);
        assert_eq!(t2, VClock(8));
    }
}
