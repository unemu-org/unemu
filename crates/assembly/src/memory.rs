// SPDX-License-Identifier: AGPL-3.0-or-later
//! Memory-map configuration (§4).

use crate::device::DeviceId;

/// A contiguous region of guest RAM (§4).
///
/// Plain read/write/execute memory. The conventional low memory, the
/// upper-memory probe area, and the zeroed option-ROM scan area are all RAM
/// regions — the option-ROM area is expressed as **writable RAM**, not ROM, so a
/// guest that probes it sees zeroes and moves on. Loaded blobs (a BIOS ROM) are
/// not here; they live once in [`crate::LoadImage`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RamRegion {
    /// Guest-physical base address.
    pub base: u64,
    /// Region size in bytes.
    pub size: u64,
}

impl RamRegion {
    /// A RAM region `[base, base + size)`.
    pub const fn new(base: u64, size: u64) -> Self {
        Self { base, size }
    }

    /// One past the last byte of the region (`base + size`).
    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }
}

/// A memory-mapped-I/O (trapped) window routed to a device (§4.2).
///
/// The config counterpart of [`bus::MmioRange`], naming the target by
/// [`DeviceId`] rather than a bus index. A trapped window may **not** overlap a
/// [`RamRegion`] — the CPU core forbids a `Trap` region over `Ram` — so RAM must
/// leave a hole where each window sits (the text buffer at `0xB8000` is the
/// canonical example).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MmioWindow {
    /// Guest-physical base address.
    pub base: u64,
    /// Window size in bytes.
    pub size: u64,
    /// The device this window traps to.
    pub device: DeviceId,
}

impl MmioWindow {
    /// An MMIO window `[base, base + size)` trapping to `device`.
    pub const fn new(base: u64, size: u64, device: DeviceId) -> Self {
        Self { base, size, device }
    }

    /// One past the last byte of the window (`base + size`).
    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }
}

/// The memory section of a [`crate::MachineConfig`] (§4).
///
/// Describes the whole flat address space: its total size, the RAM regions that
/// cover it, and the MMIO windows punched into it. Loaded images (ROM/blobs)
/// live separately in [`crate::MachineConfig::images`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryConfig {
    /// Total size of the flat guest address space in bytes. Every RAM region,
    /// MMIO window, and loaded image must fit inside `[0, address_space_bytes)`.
    pub address_space_bytes: u64,
    /// The RAM regions of the address space.
    pub ram: Vec<RamRegion>,
    /// The MMIO windows punched into the address space.
    pub mmio: Vec<MmioWindow>,
}
