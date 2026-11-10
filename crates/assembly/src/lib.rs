// SPDX-License-Identifier: AGPL-3.0-or-later
//! Declarative machine configuration + validation (§3.1).
//!
//! A [`MachineConfig`] is a **pure-data description** of one emulated computer:
//! its CPU, virtual clock, memory map, device set, drives, loadable images, and
//! output. It names *what* the machine is made of, never *how* to wire it up.
//! Turning this description into a running machine (a [`bus::Bus`] with a
//! [`bus::DecodeMap`], the [`::machine::IrqLine`] / [`::machine::DmaWiring`] tables,
//! and a [`::machine::Machine`]) is the job of [`assemble`], the single data-driven
//! realiser in this crate. Keeping the description separate from the assembly step
//! is what makes a machine reproducible and snapshot-comparable: the whole config
//! derives [`PartialEq`]/[`Eq`], so two configs — or a config and a golden — can
//! be compared byte-for-byte.
//!
//! ## What is modelled
//!
//! - [`CpuConfig`] — the modelled chip, the reset entry point, and the seeded
//!   real-mode segment registers ([`::cpu::x86jit_backend::RealSegments`]).
//! - [`ClockConfig`] — the deterministic virtual-clock ratio.
//! - [`MemoryConfig`] — the RAM regions and MMIO windows of the address space.
//! - [`DeviceConfig`] — one entry per modelled chip ([`devices`]), carrying its
//!   role ([`DeviceId`]), decoded port windows, IRQ line, and DMA wiring.
//! - [`LoadImage`] — the single home for any blob loaded into the address space
//!   (a masked BIOS ROM, a RAM-loaded boot blob).
//! - [`DriveConfig`] — the removable/fixed media attached to a disk controller.
//! - [`OutputConfig`] — initial presentation subscriber selection.
//!
//! ## Validation
//!
//! [`MachineConfig::validate`] catches the ways a description can be internally
//! inconsistent before any hardware is built — overlapping decodes, an MMIO
//! window colliding with RAM (which the CPU core forbids), a region outside the
//! address space, two devices contending for one IRQ line, a drive bound to a
//! controller slot that no controller offers, and an image whose size does not
//! match its geometry. Every failure is a distinct [`ConfigError`], and the
//! validators are unit-tested here without assembling a machine.

mod assemble;
mod board;
mod clock;
mod cpu;
mod device;
mod drive;
mod error;
mod image;
mod machine;
mod memory;
mod observation;
mod output;
mod presets;
mod run;
mod validate;

pub use assemble::{
    assemble, assemble_with_observation, derive_cmos_config, AssembledMachine, AssemblyError,
};
pub use board::{BoardModel, MachineClass};
pub use bus::PortWidth;
pub use clock::ClockConfig;
pub use cpu::{CpuChip, CpuConfig, ResetSpec, SegmentSeed};
pub use device::{
    CmosCfg, DeviceConfig, DeviceId, DeviceKind, DmaCfg, DmaSpec, FdcCfg, I8042Cfg, MdaTextCfg,
    NvramSource, PicCfg, PitCfg, PortWindow, PostPortCfg, RtcStart, SysControlCfg, UartCfg,
    VgaTextCfg,
};
pub use drive::{DriveConfig, DriveKind, FloppyGeometry, HardDiskGeometry, WriteMode};
pub use error::ConfigError;
pub use image::{FirmwareService, ImageKind, ImageSource, LoadImage};
pub use machine::MachineConfig;
pub use memory::{MemoryConfig, MmioWindow, RamRegion};
pub use observation::{
    Observation, ObservationError, ObservationPolicy, ObservationRequest, ObservationRoute,
    OutputKind,
};
pub use output::{FrameSinkKind, OutputConfig};
pub use presets::{
    at_boot, at_fdc_write, at_keyboard, at_rtc, at_vga_text, bios, floppy_boot, test386,
};
pub use run::{
    inject_scancodes, keyboard_leds_if_ready, run, try_inject_scancode, ConsoleProbe,
    InjectionSchedule, KeyboardStimulus, RunError, RunPolicy, StopCondition,
};
