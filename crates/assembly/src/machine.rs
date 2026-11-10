// SPDX-License-Identifier: AGPL-3.0-or-later
//! The root machine configuration (§3.1).

use crate::board::BoardModel;
use crate::clock::ClockConfig;
use crate::cpu::CpuConfig;
use crate::device::DeviceConfig;
use crate::drive::DriveConfig;
use crate::image::LoadImage;
use crate::memory::MemoryConfig;
use crate::output::OutputConfig;

/// A complete, declarative description of one emulated machine (§3.1).
///
/// Pure data: it says what the machine is, and [`crate::assemble`]
/// turns it into a running [`::machine::Machine`]. The whole tree derives
/// [`PartialEq`]/[`Eq`], so a config is identifiable — comparable against a
/// golden or another config byte-for-byte. Call [`MachineConfig::validate`]
/// before assembly to catch an internally inconsistent description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineConfig {
    /// Motherboard identity and its wiring constraints.
    pub board: BoardModel,
    /// The CPU: modelled chip, reset entry, seeded segments.
    pub cpu: CpuConfig,
    /// The virtual clock: the deterministic step-to-PIT ratio.
    pub clock: ClockConfig,
    /// The memory map: address-space size, RAM regions, MMIO windows.
    pub memory: MemoryConfig,
    /// The device set, one entry per modelled chip.
    pub devices: Vec<DeviceConfig>,
    /// The drives attached to disk controllers.
    pub drives: Vec<DriveConfig>,
    /// The blobs loaded into the address space (BIOS ROM, boot blobs).
    pub images: Vec<LoadImage>,
    /// Explicit system-BIOS image index, used when no ROM ends at 1 MiB.
    /// A sole undeclared ROM is also a BIOS candidate; ambiguous rigs leave
    /// this unset. Presets and machine files retain their declared BIOS here.
    /// Update this index when reordering the image list.
    pub system_bios_image: Option<usize>,
    /// How the machine is presented.
    pub output: OutputConfig,
}
