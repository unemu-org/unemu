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
    /// A device cannot depend on itself for access synchronization.
    #[error("device {device:?} cannot declare itself as an access dependent")]
    SelfAccessCoupling {
        /// The device naming itself.
        device: DeviceId,
    },

    /// An access dependent must be present in the device set.
    #[error("device {device:?} declares absent access dependent {dependent:?}")]
    AccessDependentMissing {
        /// The accessed device.
        device: DeviceId,
        /// The absent dependent.
        dependent: DeviceId,
    },

    /// All eight UART registers must fit in the 16-bit I/O space.
    #[error("uart {device:?} base {base:#x} leaves no room for eight ports")]
    UartPortWindowOutOfSpace {
        /// The configured UART.
        device: DeviceId,
        /// The first register's port.
        base: u16,
    },

    /// The requested role is absent or does not expose the requested capability.
    #[error("unsupported observation output: {device:?} {output:?}")]
    UnsupportedObservationOutput {
        /// Requested device role.
        device: DeviceId,
        /// Missing or mismatched capability.
        output: crate::OutputKind,
    },

    /// Multiple collectors would compete for the same output channel.
    #[error("duplicate observation route: {device:?} {output:?}")]
    DuplicateObservationRoute {
        /// Requested device role.
        device: DeviceId,
        /// Duplicated channel.
        output: crate::OutputKind,
    },

    /// A file source could not be materialised before validation of its bytes.
    #[error("cannot read image file {path}: {message}")]
    ImageRead {
        /// The configured backing file.
        path: std::path::PathBuf,
        /// The underlying I/O error message.
        message: String,
    },

    /// Empty MMIO windows cannot form an inclusive bus decode range.
    #[error("mmio window for {device:?} at {base:#x} must have a nonzero size")]
    EmptyMmioWindow {
        /// The target device.
        device: DeviceId,
        /// The empty window's base.
        base: u64,
    },

    /// The cascaded AT controller exposes only IRQ0 through IRQ15.
    #[error("device {device:?} uses irq line {line}; the model supports irq lines 0 through 15")]
    InvalidIrqLine {
        /// The interrupt source.
        device: DeviceId,
        /// The unsupported PIC input.
        line: u8,
    },

    /// Each face of the shared PIC pair may be exposed only once.
    #[error("duplicate pic face (cascade={cascade}) on {a:?} and {b:?}")]
    DuplicatePicFace {
        /// One device exposing the face.
        a: DeviceId,
        /// The other device exposing the face.
        b: DeviceId,
        /// Whether the duplicated face is the master.
        cascade: bool,
    },

    /// The floppy device's sector size is fixed, rather than configurable.
    #[error("drive {drive_index} uses {bytes_per_sector}-byte sectors; the floppy model supports 512-byte sectors only")]
    UnsupportedFloppySectorSize {
        /// Index in the drive list.
        drive_index: usize,
        /// The requested sector size.
        bytes_per_sector: u32,
    },

    /// A loaded image would occupy a trapped device window.
    #[error("image {image_index} overlaps mmio for {device:?} at {addr:#x}")]
    ImageOverlapsMmio {
        /// Index in the image list.
        image_index: usize,
        /// The device owning the window.
        device: DeviceId,
        /// A shared address.
        addr: u64,
    },

    /// Loading order must not decide which image's bytes survive.
    #[error("images {a} and {b} overlap at {addr:#x}")]
    ImageOverlap {
        /// One index in the image list.
        a: usize,
        /// The other index in the image list.
        b: usize,
        /// A shared address.
        addr: u64,
    },

    /// A RAM load must fit entirely in the declared RAM regions.
    #[error("ram-load image {image_index} covers unmapped ram at {addr:#x}")]
    ImageOutsideRam {
        /// Index in the image list.
        image_index: usize,
        /// The first address without RAM backing.
        addr: u64,
    },

    /// The backend cannot provide exact instruction/REP-iteration step bounds.
    #[error("machine requires a Real16 CPU with step-bounded execution: {0}")]
    StepBoundedCpuRequired(String),

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
