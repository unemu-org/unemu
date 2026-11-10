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
    /// A journal mode violates the persistent-media config contract.
    #[error("drive {drive_index} journal configuration: {message}")]
    Journal { drive_index: usize, message: String },

    /// The configured kind does not declare a word register at this port.
    #[error("device {device:?} has no 16-bit register at port {port:#x}")]
    UnsupportedWordPort {
        /// The device whose register contract rejects the word cycle.
        device: DeviceId,
        /// The unsupported cycle address.
        port: u16,
    },

    /// A word-capable ISA decode must begin at an even cycle address.
    #[error("16-bit port window for {device:?} starts at odd port {start:#x}")]
    UnalignedWordPortWindow {
        /// The device owning the window.
        device: DeviceId,
        /// The unaligned starting port.
        start: u16,
    },

    /// A present device requires a service no configured ROM provides.
    #[error("device {device:?} needs firmware service {service}; no ROM provides it")]
    NoProvider {
        /// Device that needs the service.
        device: DeviceId,
        /// Missing service.
        service: crate::FirmwareService,
    },
    /// A RAM-loaded blob cannot declare firmware services.
    #[error("RAM load image {image} cannot provide firmware services")]
    ServicesOnRamLoad {
        /// Index in the machine image list.
        image: usize,
    },
    /// A calendar seed is outside the modelled calendar range.
    #[error("device {device:?} has invalid RTC start {start}: invalid {field}; expected {range}", range = rtc_start_range(.start, .field))]
    InvalidRtcStart {
        /// RTC device with an invalid start.
        device: DeviceId,
        /// Rejected calendar value.
        start: crate::RtcStart,
        /// First invalid calendar or time field.
        field: &'static str,
    },
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

    /// A flagged overlap cannot give every shared port to a narrower window.
    #[error("port overlap at {port:#06x} between {a:?} [{a_start:#06x}..={a_end:#06x}] and {b:?} [{b_start:#06x}..={b_end:#06x}]: overlapping windows must share a start port and differ in width; split the wider window or move one", a_start = .a_window.start, a_end = .a_window.end, b_start = .b_window.start, b_end = .b_window.end)]
    PortOverlapDecodeOrder {
        /// One of the contending devices.
        a: DeviceId,
        /// The other contending device.
        b: DeviceId,
        /// The first device's overlapping window.
        a_window: crate::PortWindow,
        /// The second device's overlapping window.
        b_window: crate::PortWindow,
        /// The first port both windows decode.
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

    /// A required motherboard role is absent.
    #[error("board {board:?} requires device {device:?}")]
    BoardDeviceMissing {
        /// Selected motherboard.
        board: crate::BoardModel,
        /// Required role.
        device: DeviceId,
    },

    /// Motherboard roles must precede cards in canonical bus order.
    #[error("board {board:?} requires device {device:?} at index {index}")]
    BoardDeviceOrder {
        /// Selected motherboard.
        board: crate::BoardModel,
        /// Role at the wrong position.
        device: DeviceId,
        /// Required bus index.
        index: usize,
    },

    /// A motherboard field differs from its standard configuration.
    #[error("board {board:?} device {device:?} field {field}: expected {expected}, got {actual}")]
    BoardDeviceWiring {
        /// Selected motherboard.
        board: crate::BoardModel,
        /// Role whose configuration differs.
        device: DeviceId,
        /// The first differing field.
        field: &'static str,
        /// Standard value of that field.
        expected: String,
        /// Configured value of that field.
        actual: String,
    },

    /// A card uses an interrupt input reserved by the motherboard.
    #[error("board {board:?} does not offer irq line {line} to card {device:?}")]
    BoardCardIrq {
        /// Selected motherboard.
        board: crate::BoardModel,
        /// Card requesting the input.
        device: DeviceId,
        /// Unavailable interrupt line.
        line: u8,
    },

    /// The motherboard has no modelled DMA channel for this card.
    #[error("board {board:?} does not offer dma channel {channel} to card {device:?}")]
    BoardCardDma {
        /// Selected motherboard.
        board: crate::BoardModel,
        /// Card requesting the channel.
        device: DeviceId,
        /// Unavailable DMA channel.
        channel: u8,
    },

    /// A card names a device other than the motherboard's 8237 for DMA.
    #[error("board {board:?} requires dma controller Dma for card {device:?}, got {controller:?}")]
    BoardCardDmaController {
        /// Selected motherboard.
        board: crate::BoardModel,
        /// Card requesting DMA.
        device: DeviceId,
        /// Incorrect controller role.
        controller: DeviceId,
    },

    /// A drive names an absent controller role.
    #[error("drive {drive_index} binds to absent controller {controller:?}")]
    DriveControllerMissing {
        /// Index in the drive list.
        drive_index: usize,
        /// The missing role.
        controller: DeviceId,
    },

    /// The named device cannot serve this medium kind.
    #[error("drive {drive_index} kind is incompatible with controller {controller:?}")]
    DriveControllerKind {
        /// Index in the drive list.
        drive_index: usize,
        /// The incompatible device.
        controller: DeviceId,
    },

    /// The named controller does not offer this unit.
    #[error("drive {drive_index} binds to controller {controller:?} unit {unit} but only {available} unit(s) exist")]
    DriveUnitUnavailable {
        /// Index in the drive list.
        drive_index: usize,
        /// The named controller.
        controller: DeviceId,
        /// Requested unit.
        unit: u8,
        /// Units supported by both config and hardware.
        available: u8,
    },

    /// Two media cannot occupy the same controller unit.
    #[error("drives {a} and {b} bind to controller {controller:?} unit {unit}")]
    DuplicateDriveUnit {
        /// Earlier drive index.
        a: usize,
        /// Later drive index.
        b: usize,
        /// Shared controller.
        controller: DeviceId,
        /// Shared unit.
        unit: u8,
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

fn rtc_start_range(start: &crate::RtcStart, field: &str) -> String {
    match field {
        "year" => "1980-2099".into(),
        "month" => "1-12".into(),
        "day" => format!(
            "1-{} for the selected month and year",
            start.days_in_month()
        ),
        "hour" => "0-23".into(),
        "minute" | "second" => "0-59".into(),
        _ => "a valid calendar date and time".into(),
    }
}
