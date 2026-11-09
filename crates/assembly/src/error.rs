// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration validation errors (§3.1).

use thiserror::Error;

use crate::device::DeviceId;

/// A way a [`crate::MachineConfig`] is internally inconsistent (§3.1).
///
/// Each variant names one class of failure [`crate::MachineConfig::validate`]
/// catches before any hardware is assembled. Variants derive [`PartialEq`] so a
/// test can assert the exact failure a malformed config produces.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// An unbounded or zero block budget prevents cooperative clock progress.
    #[error("clock quantum must be a positive finite block budget (got {0:?})")]
    InvalidQuantum(Option<u64>),

    /// A rational CPU rate must have a positive numerator and denominator.
    #[error(
        "cpu rate must have a positive numerator and denominator (got {numerator}/{denominator})"
    )]
    InvalidCpuRate {
        /// Retired steps in the modelling ratio.
        numerator: u64,
        /// PIT input clocks in the modelling ratio.
        denominator: u64,
    },

    /// The same [`DeviceId`] appears on more than one device.
    #[error("duplicate device id {0:?}")]
    DuplicateDeviceId(DeviceId),

    /// Two devices decode an overlapping port range and neither marks the
    /// overlap as intentional.
    #[error("port overlap at {port:#06x} between {a:?} and {b:?}")]
    PortOverlap {
        /// One of the contending devices.
        a: DeviceId,
        /// The other contending device.
        b: DeviceId,
        /// A port both decode.
        port: u16,
    },

    /// Two MMIO windows cover an overlapping address range.
    #[error("mmio overlap at {addr:#x} between {a:?} and {b:?}")]
    MmioOverlap {
        /// One of the contending windows' devices.
        a: DeviceId,
        /// The other contending window's device.
        b: DeviceId,
        /// An address both cover.
        addr: u64,
    },

    /// A trapped MMIO window overlaps a RAM region — the CPU core forbids a
    /// `Trap` region over `Ram`.
    #[error("mmio window for {device:?} at {addr:#x} overlaps a ram region")]
    MmioOverlapsRam {
        /// The MMIO window's device.
        device: DeviceId,
        /// An address the window and a RAM region share.
        addr: u64,
    },

    /// An MMIO window traps to a device that is not in the device set.
    #[error("mmio window traps to absent device {0:?}")]
    MmioDeviceMissing(DeviceId),

    /// A RAM region, MMIO window, or loaded image falls outside
    /// `[0, address_space_bytes)`.
    #[error("region [{base:#x}, {end:#x}) is outside the {space:#x}-byte address space")]
    RegionOutOfSpace {
        /// Region base address.
        base: u64,
        /// One past the region's last byte.
        end: u64,
        /// The configured address-space size.
        space: u64,
    },

    /// Two devices drive the same PIC input line.
    #[error("irq line {line} driven by both {a:?} and {b:?}")]
    IrqConflict {
        /// One of the contending devices.
        a: DeviceId,
        /// The other contending device.
        b: DeviceId,
        /// The shared PIC input line.
        line: u8,
    },

    /// A device's DMA wiring names a controller that is not in the device set.
    #[error("device {requester:?} wires dma through absent controller {controller:?}")]
    DmaControllerMissing {
        /// The device that requested the wiring.
        requester: DeviceId,
        /// The absent controller it named.
        controller: DeviceId,
    },

    /// A drive occupies a controller slot no controller offers.
    #[error(
        "drive {drive_index} binds to controller slot {slot} but only {available} slot(s) exist"
    )]
    MissingController {
        /// Index of the drive in the config's drive list.
        drive_index: usize,
        /// The slot the drive asked for.
        slot: u8,
        /// The total number of controller slots available.
        available: u8,
    },

    /// A drive's backing image does not match its geometry's size.
    #[error("drive {drive_index} image is {actual} bytes but its geometry needs {expected}")]
    ImageGeometryMismatch {
        /// Index of the drive in the config's drive list.
        drive_index: usize,
        /// The size the geometry requires.
        expected: u64,
        /// The size the backing image actually has.
        actual: u64,
    },
}
