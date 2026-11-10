// SPDX-License-Identifier: AGPL-3.0-or-later
//! The assembler: a validated [`MachineConfig`] becomes a running [`Machine`]
//! (§3.1).
//!
//! [`assemble`] is the single data-driven realiser. It reads the pure-data
//! description and builds the concrete parts the machine core is deliberately
//! blind to: an [`X86JitCpu`] with its memory mosaic laid out, a [`Bus`] with a
//! [`bus::DecodeMap`], the one shared interrupt controller, and the IRQ / DMA
//! wiring tables. The core never names any of these concrete types;
//! naming them is exactly this layer's job, which is why it sits above the
//! `machine` crate.
//!
//! Adjacent MMIO windows share one CPU trap region so cross-device accesses
//! reach the bus instead of failing the CPU's single-region containment check.
//! The bus keeps each device's decode window and splits accesses at their
//! boundaries; gaps between windows stay outside the merged trap regions. See
//! `backlog/docs/design/machine-config.md`, §MemoryConfig.
//!
//! ## Devices are named, not numbered
//!
//! The old hand-written assembly addressed every device by a literal bus index
//! that had to stay in lock-step with the order devices were pushed onto a `Vec`.
//! Here each [`DeviceConfig`] carries a stable [`DeviceId`], and the assembler
//! returns a [`DeviceId`] → bus-index map ([`AssembledMachine::device_index`]) so
//! a later drain/inject path addresses a device by role, not by a fragile count.
//!
//! ## The shared interrupt controller
//!
//! A PC/AT's two 8259 PICs are one [`chipset::CascadedPic`]: it is the machine's
//! single [`device::InterruptController`] and it exposes its master and slave
//! chips as two separate bus [`device::Device`] faces. The config models the pair
//! as two [`DeviceConfig`] entries (a master with cascade set, a slave without);
//! the assembler builds one live `CascadedPic` and hands its master face, its
//! slave face, and its controller face out of that single chip, so a guest EOI
//! through either bus face reaches the state the machine's INTA set.
//!
//! ## CMOS initialization
//!
//! A stock BIOS reads its floppy types, equipment byte, and memory sizes from
//! CMOS and checksums them against the actual machine; a mismatch drops POST into
//! an interactive SETUP prompt a headless run cannot answer. So the seed is a
//! *product* of assembly ([`derive_cmos_config`]), computed from the memory map
//! and the attached drives. An explicit NVRAM image bypasses this derivation;
//! the fixed RTC start supplies the live calendar in either case.

use std::borrow::Cow;
use std::collections::HashMap;

use bus::{Bus, DecodeMap, MmioRange, PortRange};
use cpu::x86jit_backend::X86JitCpu;
use device::{Device, InterruptController};
use devices::{
    Absent80287, CmosConfig, CmosRtc, Dma8237, Drive, MdaText, Pit8253, PostPort, SysControl,
    Uart16550, Upd765, VgaText, I8042,
};
use machine::{DmaWiring, IrqLine, Machine};

use crate::device::{DeviceId, DeviceKind};
use crate::drive::{DriveKind, FloppyGeometry, WriteMode};
use crate::error::ConfigError;
use crate::image::{ImageKind, ImageSource};
use crate::machine::MachineConfig;
use crate::observation::{Observation, ObservationPolicy};

/// Top of conventional (base) memory: 640 KiB. RAM below this is what the BIOS
/// reports as base memory; RAM in `[0xA0000, 0x100000)` is the video/adapter/ROM
/// window, not counted.
const CONVENTIONAL_TOP: u64 = 0xA_0000;

/// Base of extended memory: 1 MiB. RAM at or above this is extended memory.
const EXTENDED_BASE: u64 = 0x10_0000;

/// A machine built from a [`MachineConfig`], with the name→index map that lets a
/// caller reach a device by its stable [`DeviceId`] (§3.1).
///
/// The `machine` is ready to run; `device_index` resolves each configured
/// [`DeviceId`] to the bus slot the assembler placed it in, so a drain or inject
/// path never hardcodes an integer index.
pub struct AssembledMachine {
    /// The running machine, fully wired.
    pub machine: Machine<X86JitCpu>,
    /// Stable device role → its index in the bus device set.
    pub device_index: HashMap<DeviceId, usize>,
    /// Validated observation routes and their bounded, run-scoped collectors.
    pub observation: Observation,
}

/// A way assembling a validated [`MachineConfig`] can fail (§3.1).
///
/// Configuration inconsistencies and unreadable backing files surface as
/// [`AssemblyError::Config`] before hardware is built. The remaining failures
/// come from the CPU core rejecting a memory operation or the bus rejecting
/// resolved access-coupling indices.
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    /// The configuration is internally inconsistent (validation ran first).
    #[error("invalid configuration: {0}")]
    Config(#[from] ConfigError),

    /// Resolved access-coupling indices were rejected by the bus.
    #[error("bus access coupling: {0}")]
    AccessCoupling(#[from] bus::AccessCouplingError),

    /// The CPU core rejected a memory-map operation (a region that does not fit,
    /// a `Trap` window overlapping RAM the config did not catch).
    #[error("cpu memory map: {0}")]
    Cpu(#[from] cpu::CpuError),
}

/// Turn a [`MachineConfig`] into a running [`Machine`] and its device-index map
/// (§3.1).
///
/// The steps are: validate the config; build the CPU and lay out its memory
/// mosaic (RAM regions, MMIO windows, then loaded images); instantiate each
/// device in config order, recording its [`DeviceId`] → index; build the decode
/// map, the interrupt controller, and the IRQ / DMA wiring from the device data;
/// derive and seed the CMOS from the memory map and drives; and assemble the
/// machine.
///
/// The **idle limit** — how long a halted guest is kept alive waiting for an
/// interrupt before the machine declares the source quiescent — is *not* set here.
/// It is a "how we run" concern that belongs to [`RunPolicy`](crate::RunPolicy),
/// so the assembled machine is built without one and the run driver
/// ([`run`](crate::run)) stamps it on before the loop runs.
pub fn assemble(config: &MachineConfig) -> Result<AssembledMachine, AssemblyError> {
    assemble_with_observation(config, &ObservationPolicy::default())
}

/// Assemble hardware and resolve explicitly requested observation routes.
///
/// Missing or mismatched capabilities are [`ConfigError`]s before execution.
/// Capture choices are separate from guest hardware configuration; existing
/// [`assemble`] callers retain their current capture paths.
pub fn assemble_with_observation(
    config: &MachineConfig,
    observation_policy: &ObservationPolicy,
) -> Result<AssembledMachine, AssemblyError> {
    assemble_with_cpu(config, observation_policy, build_cpu)
}

fn assemble_with_cpu(
    config: &MachineConfig,
    observation_policy: &ObservationPolicy,
    build: impl FnOnce(&MachineConfig) -> Result<X86JitCpu, AssemblyError>,
) -> Result<AssembledMachine, AssemblyError> {
    // A validated config does not fail assembly for configuration reasons; run
    // the checks first so any inconsistency surfaces as a precise ConfigError.
    config.validate()?;

    // Validate the bytes that will actually be mounted, without a second file
    // read that could observe a different length or contents.
    let resolved = resolve_file_sources(config)?;
    let config = resolved.as_ref();
    config.validate()?;

    let mut cpu = build(config)?;
    validate_step_cpu(&mut cpu)?;
    let cmos = derive_cmos_config(config);

    // Instantiate devices in config order, recording each role's bus index. The
    // one shared PIC is created lazily on the first PIC entry and reused for the
    // second face and the controller seam.
    let mut devices: Vec<Box<dyn Device>> = Vec::with_capacity(config.devices.len());
    let mut device_index: HashMap<DeviceId, usize> = HashMap::with_capacity(config.devices.len());
    let mut pic: Option<chipset::CascadedPic> = None;
    let mut reset_signal = None;

    for dc in &config.devices {
        let boxed: Box<dyn Device> = match &dc.kind {
            DeviceKind::Uart(c) => Box::new(Uart16550::new(c.base)),
            DeviceKind::Pit(_) => Box::new(Pit8253::new()),
            DeviceKind::Pic(c) => {
                // One live master/slave pair backs both faces and the controller.
                let shared = pic.get_or_insert_with(chipset::CascadedPic::new);
                if c.cascade {
                    Box::new(shared.master_device())
                } else {
                    Box::new(shared.slave_device())
                }
            }
            DeviceKind::Keyboard(_) => {
                let keyboard = I8042::new();
                reset_signal = Some(keyboard.reset_signal());
                Box::new(keyboard)
            }
            DeviceKind::Rtc(c) => {
                let mut rtc = CmosRtc::new();
                if let Some((second_ticks, uip_ticks)) = c.timing {
                    rtc = rtc.with_timing(second_ticks, uip_ticks);
                }
                rtc = match c.nvram {
                    crate::NvramSource::Derived => rtc.with_config(cmos),
                    crate::NvramSource::Blank => rtc,
                    crate::NvramSource::Image(bytes) => rtc.with_nvram(bytes),
                };
                let start = c.start;
                rtc.set_time(
                    start.second,
                    start.minute,
                    start.hour,
                    start.weekday(),
                    start.day,
                    start.month,
                    (start.year % 100) as u8,
                );
                Box::new(rtc)
            }
            DeviceKind::Vga(_) => Box::new(VgaText::new()),
            DeviceKind::Mda(_) => Box::new(MdaText::new()),
            DeviceKind::SysControl(_) => Box::new(SysControl::new()),
            DeviceKind::Post(c) => Box::new(PostPort::with_port(c.port)),
            DeviceKind::Fdc(_) => Box::new(Upd765::with_drives(build_fdc_drives(config, dc.id)?)),
            DeviceKind::Dma(_) => Box::new(Dma8237::new()),
            DeviceKind::DmaWord(_) => Box::new(Dma8237::new_at_word()),
            DeviceKind::AbsentCoprocessor => Box::new(Absent80287::new()),
        };
        let index = devices.len();
        devices.push(boxed);
        device_index.insert(dc.id, index);
    }

    let mut observation = Observation::bind(observation_policy, &device_index, &mut devices)?;
    observation.configure_output(config.output);

    // The machine needs exactly one controller. If the config has no PIC (an
    // interrupt-free machine), a null controller keeps the loop total.
    let controller: Box<dyn InterruptController> = match pic {
        Some(p) => Box::new(p),
        None => Box::new(chipset::NoInterruptController),
    };

    let map = build_decode_map(config, &device_index);
    let irq_lines = build_irq_lines(config, &device_index);
    let dma_wirings = build_dma_wirings(config, &device_index);

    // The machine is built with no idle limit; the run policy stamps it on before
    // the loop runs (idle_limit is a "how we run" concern, not machine identity).
    let (rate_numerator, rate_denominator) = config.clock.cpu_rate()?;
    let mut bus = Bus::new(devices, map);
    for device in &config.devices {
        for dependent in &device.access_dependents {
            bus.add_access_coupling(device_index[&device.id], device_index[dependent])?;
        }
    }
    bus.set_trace_policy(observation_policy.trace);
    let mut machine = Machine::new(cpu, bus, controller, irq_lines, None)
        .with_cpu_rate(rate_numerator, rate_denominator)
        .with_dma_wirings(dma_wirings);
    if let Some(signal) = reset_signal {
        machine = machine.with_reset_signal(signal);
    }

    Ok(AssembledMachine {
        machine,
        device_index,
        observation,
    })
}

/// A zero-step probe rejects unsupported backends without executing guest code.
fn validate_step_cpu(cpu: &mut impl cpu::Cpu) -> Result<(), ConfigError> {
    cpu.run_steps(0, cpu::StepRunOptions::default())
        .map(|_| ())
        .map_err(|error| ConfigError::StepBoundedCpuRequired(error.to_string()))
}

/// Build the CPU with the whole memory mosaic laid out and the reset entry seeded
/// (§4).
///
/// The bare real-mode constructor maps nothing; the assembler lays out every RAM
/// region, MMIO window, and loaded image from the config, so the memory map is a
/// pure function of [`MemoryConfig`] rather than a constructor's baked-in layout.
fn build_cpu(config: &MachineConfig) -> Result<X86JitCpu, AssemblyError> {
    let mut cpu = X86JitCpu::new_real16_bare(
        config.memory.address_space_bytes,
        config.cpu.reset.cs(),
        config.cpu.reset.ip(),
        config.cpu.segments.into(),
    )?;

    // RAM and MMIO first so a RAM-loaded image writes into an already-mapped
    // region; a ROM image maps and populates its own region.
    for region in &config.memory.ram {
        cpu.map_ram(region.base, region.size)?;
    }
    let mut mmio: Vec<_> = config.memory.mmio.iter().collect();
    mmio.sort_unstable_by_key(|window| window.base);
    // The CPU requires each access to fit one region, while device ownership
    // remains a bus concern. Only touching windows can share a trap region.
    for run in mmio.chunk_by(|left, right| left.end() == right.base) {
        let base = run[0].base;
        let end = run[run.len() - 1].end();
        cpu.map_mmio(base, end - base)?;
    }
    for image in &config.images {
        let bytes = load_image_bytes(&image.source)?;
        match image.kind {
            ImageKind::Rom => cpu.map_rom(image.base, &bytes)?,
            ImageKind::RamLoad => cpu.write_mem(image.base, &bytes)?,
        }
    }
    Ok(cpu)
}

/// Materialise an image's bytes: inline bytes verbatim, a blank run of zeroes, or
/// a file read from disk at load time.
fn load_image_bytes(source: &ImageSource) -> Result<Vec<u8>, ConfigError> {
    match source {
        ImageSource::Inline(bytes) => Ok(bytes.clone()),
        ImageSource::Blank { size } => Ok(vec![0u8; *size as usize]),
        ImageSource::File(path) => std::fs::read(path).map_err(|source| ConfigError::ImageRead {
            path: path.clone(),
            message: source.to_string(),
        }),
    }
}

/// Resolve file lengths before validating placement and disk geometry. Inline
/// presets borrow their config so the common path needs no additional copy.
fn resolve_file_sources(config: &MachineConfig) -> Result<Cow<'_, MachineConfig>, ConfigError> {
    if !config
        .images
        .iter()
        .map(|image| &image.source)
        .chain(config.drives.iter().map(|drive| &drive.image))
        .any(|source| matches!(source, ImageSource::File(_)))
    {
        return Ok(Cow::Borrowed(config));
    }
    let mut resolved = config.clone();
    for source in resolved
        .images
        .iter_mut()
        .map(|image| &mut image.source)
        .chain(resolved.drives.iter_mut().map(|drive| &mut drive.image))
    {
        if matches!(source, ImageSource::File(_)) {
            *source = ImageSource::Inline(load_image_bytes(source)?);
        }
    }
    Ok(Cow::Owned(resolved))
}

/// Build the port + MMIO decode map from the device windows, sorted so the map is
/// a pure function of the config, not of device build order (§4.1).
///
/// Non-overlapping ranges are unaffected by the sort; a deliberately overlapping
/// pair (flagged in the config and already accepted by validation) is ordered
/// narrower-first, so first-match hands the shared byte to the more specific
/// device — the `0x80` POST latch winning over a wider DMA page-register range,
/// for example.
fn build_decode_map(config: &MachineConfig, device_index: &HashMap<DeviceId, usize>) -> DecodeMap {
    let mut ports: Vec<PortRange> = Vec::new();
    for dc in &config.devices {
        let index = device_index[&dc.id];
        for window in &dc.ports {
            ports.push(PortRange {
                width: window.width,
                ports: window.start..=window.end,
                device: index,
            });
        }
    }
    ports.sort_by_key(|r| (*r.ports.start(), *r.ports.end()));

    let mut mmio: Vec<MmioRange> = config
        .memory
        .mmio
        .iter()
        .map(|window| MmioRange {
            addrs: window.base..=window.end() - 1,
            device: device_index[&window.device],
        })
        .collect();
    mmio.sort_by_key(|r| (*r.addrs.start(), *r.addrs.end()));

    DecodeMap { ports, mmio }
}

/// Build the device-IRQ → PIC-input wiring table from the config, resolving each
/// source [`DeviceId`] to its bus index (§5.1).
fn build_irq_lines(
    config: &MachineConfig,
    device_index: &HashMap<DeviceId, usize>,
) -> Vec<IrqLine> {
    config
        .devices
        .iter()
        .filter_map(|dc| {
            dc.irq.map(|irq| IrqLine {
                device: device_index[&dc.id],
                irq,
            })
        })
        .collect()
}

/// Build the bus-master → DMA-controller coupling table, resolving both the
/// requester and its named controller to their bus indices (§7.7).
fn build_dma_wirings(
    config: &MachineConfig,
    device_index: &HashMap<DeviceId, usize>,
) -> Vec<DmaWiring> {
    config
        .devices
        .iter()
        .filter_map(|dc| {
            dc.dma.map(|dma| DmaWiring {
                requester: device_index[&dc.id],
                controller: device_index[&dma.controller],
            })
        })
        .collect()
}

/// Route the config's floppy drives into the controller's four drive positions by
/// their `controller` and `unit` (§7.7).
///
/// Only floppy drives are mounted here; a slot past the fourth position is
/// ignored (validation bounds units to both declared count and hardware). The
/// config geometry maps directly to the controller's own geometry type.
///
/// This is the pattern a future fixed-disk controller reuses. A
/// [`DriveKind::HardDisk`] drive is rejected by validation until a compatible
/// controller exists. When one is added it plugs into the same seam: a
/// `build_ide_drives(config)` that filters `config.drives` for `HardDisk` and
/// routes them into the new device by their `controller` and `unit`, called from the
/// device-construction match arm just as [`DeviceKind::Fdc`] calls this. No new
/// assembler architecture is needed — the drive list, the slot routing, and the
/// CMOS derivation are already in place; only the target device is missing.
fn build_fdc_drives(
    config: &MachineConfig,
    controller: DeviceId,
) -> Result<[Option<Drive>; 4], ConfigError> {
    let mut slots: [Option<Drive>; 4] = [None, None, None, None];
    for drive in &config.drives {
        if drive.controller != controller {
            continue;
        }
        let DriveKind::Floppy { geometry } = &drive.kind else {
            continue;
        };
        let slot = drive.unit as usize;
        if slot >= slots.len() {
            continue;
        }
        let image = load_image_bytes(&drive.image)?;
        let geometry = to_device_geometry(geometry);
        slots[slot] = Some(if drive.write == WriteMode::ReadOnly {
            Drive::new_read_only(geometry, image)
        } else {
            Drive::new(geometry, image)
        });
    }
    Ok(slots)
}

/// Convert a config [`FloppyGeometry`] into the floppy controller's own geometry
/// type (both are CHS with a fixed 512-byte sector).
fn to_device_geometry(geometry: &FloppyGeometry) -> devices::FloppyGeometry {
    devices::FloppyGeometry {
        cylinders: geometry.cylinders,
        heads: geometry.heads,
        sectors_per_track: geometry.sectors_per_track,
    }
}

/// Derive the CMOS seed the BIOS expects from the memory map and the attached
/// drives (§7.6).
///
/// The BIOS checksums floppy types, the equipment byte, and the base/extended
/// memory sizes against the real machine, so these are computed here rather than
/// supplied by the caller: base memory is the RAM below 640 KiB, extended memory
/// is the RAM at or above 1 MiB, and the floppy nibbles and equipment byte come
/// from the floppy drives on the primary controller (DeviceId::Fdc). Secondary
/// controllers have no entries in the AT CMOS diskette inventory. For a plain AT — 640 KiB base, one 1.44 MB
/// floppy, no hard disk or coprocessor — this reproduces [`CmosConfig::default`]
/// byte-for-byte.
pub fn derive_cmos_config(config: &MachineConfig) -> CmosConfig {
    let base_memory_kb = clamp_kb(ram_bytes_in(config, 0, CONVENTIONAL_TOP));
    let extended_memory_kb = clamp_kb(ram_bytes_in(config, EXTENDED_BASE, u64::MAX));

    // Floppy nibbles: high = drive 0 (slot 0), low = drive 1 (slot 1).
    let mut floppy_types = 0u8;
    let mut floppy_count = 0u8;
    let mut hard_disk_types = 0u8;
    for drive in &config.drives {
        match &drive.kind {
            DriveKind::Floppy { geometry } => {
                if drive.controller != DeviceId::Fdc {
                    continue;
                }
                floppy_count += 1;
                let code = floppy_cmos_type(geometry);
                match drive.unit {
                    0 => floppy_types |= code << 4,
                    1 => floppy_types |= code,
                    _ => {}
                }
            }
            DriveKind::HardDisk { .. } => {
                // A hard disk present but with no controller device to serve it
                // yet: the CMOS still derives its nibble here, so the day a future
                // IDE/ATA device is added, an attached HardDisk drive already
                // reports itself to the BIOS with no change to this function. Mark
                // the nibble "type stored in the extended table" (0xF), the
                // encoding a BIOS uses for drives beyond the built-in type list.
                match drive.unit {
                    0 => hard_disk_types |= 0xF0,
                    1 => hard_disk_types |= 0x0F,
                    _ => {}
                }
            }
        }
    }

    // Equipment byte (index 0x14): bit 0 = a floppy drive is installed; bits 6-7
    // = floppy count minus one (00 = one drive, 01 = two). No coprocessor
    // (bit 1) and EGA/VGA display (bits 4-5 = 00), matching the modelled machine.
    let mut equipment = 0u8;
    if floppy_count > 0 {
        equipment |= 0x01;
        equipment |= (floppy_count - 1).min(0x03) << 6;
    }

    CmosConfig {
        floppy_types,
        hard_disk_types,
        equipment,
        base_memory_kb,
        extended_memory_kb,
    }
}

/// Total bytes of RAM lying within `[low, high)`, clamping each region to the
/// window so a region that straddles the boundary contributes only its
/// in-window part.
fn ram_bytes_in(config: &MachineConfig, low: u64, high: u64) -> u64 {
    config
        .memory
        .ram
        .iter()
        .map(|r| {
            let start = r.base.clamp(low, high);
            let end = r.end().clamp(low, high);
            end.saturating_sub(start)
        })
        .sum()
}

/// Convert a byte count to whole kilobytes, saturating at the CMOS field's 16-bit
/// range.
fn clamp_kb(bytes: u64) -> u16 {
    (bytes / 1024).min(u64::from(u16::MAX)) as u16
}

/// The CMOS floppy-type code for a geometry: 1.44 MB → 4, 1.2 MB → 2, 720 KB → 3,
/// 360 KB → 1 (the standard AT nibble values); an unrecognised geometry reports 0
/// (no drive).
fn floppy_cmos_type(geometry: &FloppyGeometry) -> u8 {
    if *geometry == FloppyGeometry::HD_1_44 {
        4
    } else if *geometry == FloppyGeometry::HD_1_2 {
        2
    } else if *geometry == FloppyGeometry::DD_720K {
        3
    } else if *geometry == FloppyGeometry::DD_360K {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtc_config(config: &mut MachineConfig) -> &mut CmosCfg {
        let device = config
            .devices
            .iter_mut()
            .find(|d| d.id == DeviceId::Rtc)
            .unwrap();
        let DeviceKind::Rtc(ref mut cfg) = device.kind else {
            panic!("RTC kind")
        };
        cfg
    }

    fn rtc_registers(config: &MachineConfig) -> [u8; 64] {
        let mut assembled = assemble(config).unwrap();
        let bus = assembled.machine.bus_mut();
        std::array::from_fn(|index| {
            bus.port_write(0x70, 1, index as u32).unwrap();
            bus.port_read(0x71, 1).unwrap() as u8
        })
    }

    #[test]
    fn bios_preset_assembles_both_dma_banks_and_empty_coprocessor() {
        let config = crate::bios(vec![0; 0x8000], Some(vec![0; 1_474_560]));
        config.validate().unwrap();
        let mut assembled = assemble(&config).unwrap();
        let ids = &assembled.device_index;
        assert_eq!(ids[&DeviceId::Dma], 3);
        assert_eq!(ids[&DeviceId::DmaWord], 4);
        assert_eq!(ids[&DeviceId::Coprocessor], 7);
        let bus = assembled.machine.bus_mut();
        bus.port_write(0x0C, 1, 0).unwrap();
        bus.port_write(0x00, 1, 0x56).unwrap();
        bus.port_write(0x00, 1, 0x78).unwrap();
        bus.port_write(0xC0, 1, 0x12).unwrap();
        bus.port_write(0xC0, 1, 0x34).unwrap();
        bus.port_write(0xD8, 1, 0).unwrap();
        assert_eq!(bus.port_read(0xC0, 1).unwrap(), 0x12);
        assert_eq!(bus.port_read(0xC0, 1).unwrap(), 0x34);
        bus.port_write(0x0C, 1, 0).unwrap();
        assert_eq!(bus.port_read(0x00, 1).unwrap(), 0x56);
        assert_eq!(bus.port_read(0x00, 1).unwrap(), 0x78);
        for (port, value) in [(0x81, 0x12), (0x8B, 0x34), (0x89, 0x56), (0x8A, 0x78)] {
            bus.port_write(port, 1, value).unwrap();
            assert_eq!(bus.port_read(port, 1).unwrap(), value);
        }
        for port in [0xF0, 0xF1, 0xF8, 0xFF] {
            bus.port_write(port, 1, 0).unwrap();
            assert_eq!(bus.port_read(port, 1).unwrap(), 0xFF);
        }
        let irq = build_irq_lines(&config, ids);
        assert!(irq
            .iter()
            .any(|line| line.irq == 13 && line.device == ids[&DeviceId::Coprocessor]));
    }

    #[test]
    fn cpu_executes_x87_despite_the_boards_empty_coprocessor_socket() {
        // Keep DS flat so the guest's control-word store and host readback
        // address the same scratch word, independently of the BIOS segment seed.
        let program = [
            0xDB, 0xE3, // FNINIT
            0xC7, 0x06, 0xFE, 0x03, 0, 0, // MOV word [03FEh], 0
            0xD9, 0x3E, 0xFE, 0x03, // FNSTCW [03FEh]
            0xFA, 0xF4, // CLI; HLT
        ];
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        let mut config = crate::bios(rom, Some(vec![0; 1_474_560]));
        config.cpu.reset = crate::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        config.cpu.segments = crate::SegmentSeed::flat(0);
        let mut assembled = assemble(&config).unwrap();
        assert_eq!(
            assembled.machine.run_bounded(Some(100)).unwrap(),
            machine::RunOutcome::Halted
        );
        let mut control_word = [0xFF; 2];
        assembled
            .machine
            .cpu()
            .read_mem(0x3FE, &mut control_word)
            .unwrap();
        assert_eq!(u16::from_le_bytes(control_word), 0x037F);
    }

    #[test]
    fn guest_keyboard_reset_stops_before_the_following_instruction() {
        for (program, reset_time) in [
            (vec![0xB0, 0xFE, 0xE6, 0x64, 0xB0, 0x55, 0xFA, 0xF4], 2),
            (
                vec![
                    0xB0, 0xD1, 0xE6, 0x64, 0xB0, 2, 0xE6, 0x60, 0xB0, 0x55, 0xFA, 0xF4,
                ],
                4,
            ),
        ] {
            let mut rom = vec![0; 0x8000];
            rom[..program.len()].copy_from_slice(&program);
            let mut config = crate::bios(rom, Some(vec![0; 1_474_560]));
            config.cpu.reset = crate::ResetSpec::Entry { cs: 0xF800, ip: 0 };
            let mut assembled = assemble(&config).unwrap();
            assert_eq!(
                assembled.machine.run_bounded(Some(100)).unwrap(),
                machine::RunOutcome::ResetRequested
            );
            assert_eq!(assembled.machine.virtual_time(), reset_time);
            assert_ne!(assembled.machine.cpu().reg(cpu::CpuReg::Rax) & 0xFF, 0x55);
            assert_eq!(
                assembled.machine.run_bounded(Some(100)).unwrap(),
                machine::RunOutcome::Halted
            );
            assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rax) & 0xFF, 0x55);
        }
    }

    #[test]
    fn run_policies_continue_after_reset_requests() {
        // Cover a reset before prompt detection and another after the policy
        // injects input, without letting either one terminate the session.
        let program = [0xB0, 0xFE, 0xE6, 0x64, 0xE6, 0x64, 0xB0, 0x55, 0xFA, 0xF4];
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        let mut config = crate::bios(rom, Some(vec![0; 1_474_560]));
        config.cpu.reset = crate::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        for prompt in [false, true] {
            let mut assembled = assemble_with_observation(
                &config,
                &ObservationPolicy {
                    routes: vec![crate::ObservationRequest {
                        device: DeviceId::Vga,
                        output: crate::OutputKind::TextScanout,
                        byte_limit: 0,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
            let mut policy = crate::RunPolicy::natural(Some(100), None);
            if prompt {
                policy.stop = crate::StopCondition::PromptThen {
                    console: crate::ConsoleProbe {
                        device: DeviceId::Vga,
                    },
                    needle: String::new(),
                    poll_step: 2,
                    then_budget: 10,
                };
                policy.stimuli.push(crate::KeyboardStimulus {
                    keyboard_index: assembled.device_index[&DeviceId::Keyboard],
                    scancodes: vec![0x1E],
                    schedule: crate::InjectionSchedule::AfterPrompt,
                });
            }
            assert_eq!(
                crate::run(&mut assembled.machine, &assembled.observation, &policy).unwrap(),
                machine::RunOutcome::Halted
            );
            assert_eq!(assembled.machine.virtual_time(), 6);
            assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rax) & 0xFF, 0x55);
        }
    }

    #[test]
    fn bios_dma_word_out_uses_both_aliased_register_ports() {
        let mut rom = vec![0; 0x8000];
        rom[..7].copy_from_slice(&[0xB8, 0x12, 0x34, 0xE7, 0xC0, 0xFA, 0xF4]);
        let mut config = crate::bios(rom, Some(vec![0; 1_474_560]));
        config.cpu.reset = crate::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut assembled = assemble(&config).unwrap();
        assert_eq!(
            assembled.machine.run_bounded(Some(100)).unwrap(),
            machine::RunOutcome::Halted
        );
        let bus = assembled.machine.bus_mut();
        assert_eq!(bus.port_read(0xC0, 2).unwrap(), 0x3412);
        bus.port_write(0xC1, 1, 0x56).unwrap();
        bus.port_write(0xC0, 1, 0x78).unwrap();
        assert_eq!(bus.port_read(0xC0, 2).unwrap(), 0x7856);
    }

    #[test]
    fn initialization_default_rtc_matches_legacy_state_and_every_register() {
        for (config, expected_nvram) in [
            (
                crate::bios(vec![0; 32768], None),
                crate::NvramSource::Derived,
            ),
            (
                crate::bios(vec![0; 32768], Some(vec![0; 1_474_560])),
                crate::NvramSource::Derived,
            ),
            (
                crate::at_rtc(vec![0xF4], 100, 10),
                crate::NvramSource::Blank,
            ),
        ] {
            let rtc_cfg = config
                .devices
                .iter()
                .find_map(|device| match device.kind {
                    DeviceKind::Rtc(cfg) => Some(cfg),
                    _ => None,
                })
                .unwrap();
            assert_eq!(rtc_cfg.start, crate::RtcStart::default());
            let mut legacy = CmosRtc::new();
            if let Some((second, uip)) = rtc_cfg.timing {
                legacy = legacy.with_timing(second, uip);
            }
            assert_eq!(rtc_cfg.nvram, expected_nvram);
            if expected_nvram == crate::NvramSource::Derived {
                legacy = legacy.with_config(derive_cmos_config(&config));
            }
            let mut assembled = assemble(&config).unwrap();
            let index = assembled.device_index[&DeviceId::Rtc];
            let actual = assembled
                .machine
                .bus_mut()
                .device_mut(index)
                .unwrap()
                .save();
            assert_eq!(
                actual,
                legacy.save(),
                "includes every backing byte and calendar field"
            );
            let registers = rtc_registers(&config);
            for (index, value) in registers.into_iter().enumerate() {
                legacy.port_write(0x70, 1, index as u32).unwrap();
                assert_eq!(
                    value,
                    legacy.port_read(0x71, 1).unwrap() as u8,
                    "register {index:#x}"
                );
            }
        }
    }

    #[test]
    fn initialization_non_default_rtc_start_reads_back_through_ports() {
        let mut config = crate::at_rtc(vec![0xF4], 100, 10);
        rtc_config(&mut config).start = crate::RtcStart {
            year: 2024,
            month: 2,
            day: 29,
            hour: 23,
            minute: 45,
            second: 56,
        };
        let bytes = rtc_registers(&config);
        for (index, expected) in [
            (0, 0x56),
            (2, 0x45),
            (4, 0x23),
            (6, 5),
            (7, 0x29),
            (8, 0x02),
            (9, 0x24),
        ] {
            assert_eq!(bytes[index], expected, "register {index:#x}");
        }
    }

    #[test]
    fn initialization_period_dates_read_back_and_roll_over_the_century() {
        for (start, next_date) in [
            (
                crate::RtcStart {
                    year: 1980,
                    month: 1,
                    day: 1,
                    hour: 23,
                    minute: 59,
                    second: 59,
                },
                [4, 0x02, 0x01, 0x80],
            ),
            (
                crate::RtcStart {
                    year: 1999,
                    month: 12,
                    day: 31,
                    hour: 23,
                    minute: 59,
                    second: 59,
                },
                [7, 0x01, 0x01, 0x00],
            ),
            (
                crate::RtcStart {
                    year: 1984,
                    month: 2,
                    day: 28,
                    hour: 23,
                    minute: 59,
                    second: 59,
                },
                [4, 0x29, 0x02, 0x84],
            ),
            (
                crate::RtcStart {
                    year: 2024,
                    month: 2,
                    day: 29,
                    hour: 23,
                    minute: 59,
                    second: 59,
                },
                [6, 0x01, 0x03, 0x24],
            ),
        ] {
            let mut config = crate::at_rtc(vec![0xF4], 100, 10);
            rtc_config(&mut config).start = start;
            let mut assembled = assemble(&config).unwrap();
            let bus = assembled.machine.bus_mut();
            bus.port_write(0x70, 1, 9).unwrap();
            let year = (start.year % 100) as u8;
            assert_eq!(
                bus.port_read(0x71, 1).unwrap() as u8,
                ((year / 10) << 4) | (year % 10)
            );
            bus.tick_due(device::VClock(100));
            for (index, expected) in [6, 7, 8, 9].into_iter().zip(next_date) {
                bus.port_write(0x70, 1, index).unwrap();
                assert_eq!(
                    bus.port_read(0x71, 1).unwrap() as u8,
                    expected,
                    "{start}, register {index}"
                );
            }
        }
    }

    #[test]
    fn initialization_nvram_image_bypasses_derivation_and_uses_fixed_start() {
        let mut config = crate::bios(vec![0; 32768], None);
        let mut image = std::array::from_fn(|index| (index as u8).wrapping_mul(37));
        // Motorola MC146818 datasheet, Register B: binary, 12-hour encoding.
        image[0x0B] = 0x04;
        let cfg = rtc_config(&mut config);
        cfg.nvram = crate::NvramSource::Image(image);
        cfg.start = crate::RtcStart {
            year: 2031,
            month: 12,
            day: 25,
            hour: 13,
            minute: 47,
            second: 59,
        };
        let bytes = rtc_registers(&config);
        for index in (0..64).filter(|index| ![0, 2, 4, 6, 7, 8, 9, 10, 12, 13].contains(index)) {
            assert_eq!(bytes[index], image[index], "register {index:#x}");
        }
        assert_eq!(
            [bytes[0], bytes[2], bytes[4], bytes[6], bytes[7], bytes[8], bytes[9]],
            [59, 47, 0x81, 5, 25, 12, 31]
        );
        // Motorola MC146818 datasheet, Registers A and D: live UIP and VRT.
        assert_eq!(bytes[10], image[10] & 0x7F);
        assert_eq!(bytes[12], 0);
        assert_eq!(bytes[13], 0x80);
    }

    #[test]
    fn observation_rejects_missing_or_mismatched_capabilities() {
        use crate::{ObservationPolicy, ObservationRequest, OutputKind};
        let config = crate::at_boot(vec![0xF4]);
        for (device, output) in [
            (DeviceId::Uart, OutputKind::TextScanout),
            (
                DeviceId::Vga,
                OutputKind::Bytes(device::ByteStreamKind::SerialTx),
            ),
            (
                DeviceId::Uart,
                OutputKind::Bytes(device::ByteStreamKind::PostCheckpoints),
            ),
            (
                DeviceId::Post,
                OutputKind::Bytes(device::ByteStreamKind::PostCheckpoints),
            ),
        ] {
            let policy = ObservationPolicy {
                trace: device::TracePolicy::Off,
                routes: vec![ObservationRequest {
                    device,
                    output,
                    byte_limit: 8,
                }],
            };
            assert!(matches!(
                crate::assemble_with_observation(&config, &policy),
                Err(AssemblyError::Config(ConfigError::UnsupportedObservationOutput {
                    device: actual_device, output: actual_output,
                })) if actual_device == device && actual_output == output
            ));
        }
    }
    use crate::board::BoardModel;
    use crate::clock::ClockConfig;
    use crate::cpu::{CpuConfig, ResetSpec, SegmentSeed};
    use crate::device::{
        CmosCfg, DeviceConfig, DmaCfg, DmaSpec, FdcCfg, I8042Cfg, PicCfg, PitCfg, PortWindow,
        PostPortCfg, SysControlCfg, UartCfg, VgaTextCfg,
    };
    use crate::drive::DriveConfig;
    use crate::image::LoadImage;
    use crate::memory::{MemoryConfig, MmioWindow, RamRegion};
    use crate::output::OutputConfig;
    use cpu::Cpu;
    use device::{DmaOp, DmaOutcome};

    const SPACE: u64 = 0x10_0000;
    const TEXT_BASE: u64 = 0xB_8000;
    const TEXT_LEN: u64 = 0x8000;
    const ROM_BASE: u64 = 0xF_8000;
    const ROM_LEN: u64 = 0x8000;

    /// A reference AT-class machine: a small stand-in for the real presets
    /// covering the memory map and device wiring. Everything the assembler must build — the memory mosaic, the
    /// full device set with the cascaded PIC, the trapped text buffer, the floppy
    /// controller with its DMA, and a seeded CMOS — is present.
    fn reference_config() -> MachineConfig {
        let devices = vec![
            DeviceConfig::new(
                DeviceId::Uart,
                DeviceKind::Uart(UartCfg { base: 0x3F8 }),
                vec![PortWindow::new(0x3F8, 0x3FF)],
            ),
            DeviceConfig::new(
                DeviceId::Pit,
                DeviceKind::Pit(PitCfg {}),
                vec![PortWindow::new(0x40, 0x43)],
            )
            .with_irq(0),
            DeviceConfig::new(
                DeviceId::MasterPic,
                DeviceKind::Pic(PicCfg { cascade: true }),
                vec![PortWindow::new(0x20, 0x21)],
            ),
            DeviceConfig::new(
                DeviceId::SlavePic,
                DeviceKind::Pic(PicCfg { cascade: false }),
                vec![PortWindow::new(0xA0, 0xA1)],
            ),
            DeviceConfig::new(
                DeviceId::Keyboard,
                DeviceKind::Keyboard(I8042Cfg {}),
                vec![PortWindow::single(0x60), PortWindow::single(0x64)],
            )
            .with_irq(1),
            DeviceConfig::new(
                DeviceId::Rtc,
                DeviceKind::Rtc(CmosCfg {
                    nvram: crate::NvramSource::Derived,
                    start: crate::RtcStart::default(),
                    timing: None,
                }),
                vec![PortWindow::new(0x70, 0x71)],
            )
            .with_irq(8),
            DeviceConfig::new(
                DeviceId::Vga,
                DeviceKind::Vga(VgaTextCfg {}),
                vec![PortWindow::new(0x3D4, 0x3D5)],
            ),
            DeviceConfig::new(
                DeviceId::SysControl,
                DeviceKind::SysControl(SysControlCfg {}),
                vec![PortWindow::single(0x61)],
            ),
            DeviceConfig::new(
                DeviceId::Post,
                DeviceKind::Post(PostPortCfg { port: 0x80 }),
                vec![PortWindow::single(0x80)],
            ),
            DeviceConfig::new(
                DeviceId::Fdc,
                DeviceKind::Fdc(FdcCfg { drive_slots: 2 }),
                vec![PortWindow::new(0x3F0, 0x3F7)],
            )
            .with_irq(6)
            .with_dma(DmaSpec {
                controller: DeviceId::Dma,
                channel: 2,
            }),
            DeviceConfig::new(
                DeviceId::Dma,
                DeviceKind::Dma(DmaCfg {}),
                // 0x81..=0x8F is the page-latch block; it sits above the POST
                // latch at 0x80, so the two never share a byte.
                vec![PortWindow::new(0x00, 0x0F), PortWindow::new(0x81, 0x8F)],
            ),
        ];

        let memory = MemoryConfig {
            address_space_bytes: SPACE,
            ram: vec![
                RamRegion::new(0, TEXT_BASE),
                RamRegion::new(TEXT_BASE + TEXT_LEN, ROM_BASE - (TEXT_BASE + TEXT_LEN)),
            ],
            mmio: vec![MmioWindow::new(TEXT_BASE, TEXT_LEN, DeviceId::Vga)],
        };

        let images = vec![LoadImage {
            base: ROM_BASE,
            source: ImageSource::Inline(vec![0u8; ROM_LEN as usize]),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        }];

        let drives = vec![DriveConfig {
            kind: DriveKind::Floppy {
                geometry: FloppyGeometry::HD_1_44,
            },
            image: ImageSource::Blank {
                size: FloppyGeometry::HD_1_44.image_bytes(),
            },
            controller: DeviceId::Fdc,
            unit: 0,
            write: WriteMode::Volatile,
        }];

        MachineConfig {
            board: BoardModel::Bare,
            cpu: CpuConfig {
                chip: crate::cpu::CpuChip::I80286,
                reset: ResetSpec::reset_vector(),
                segments: SegmentSeed::flat(0xF000),
            },
            clock: ClockConfig::default(),
            memory,
            devices,
            drives,
            images,
            output: OutputConfig::default(),
        }
    }

    #[test]
    fn reference_config_assembles() {
        assert!(assemble(&reference_config()).is_ok());
    }

    #[test]
    fn access_couplings_resolve_roles_after_device_reordering() {
        for reverse in [false, true] {
            let mut config = reference_config();
            config
                .devices
                .iter_mut()
                .find(|d| d.id == DeviceId::Uart)
                .unwrap()
                .access_dependents = vec![DeviceId::Pit];
            if reverse {
                config.devices.reverse();
            }
            let mut assembled = assemble(&config).unwrap();
            let pit_index = assembled.device_index[&DeviceId::Pit];
            let rtc_index = assembled.device_index[&DeviceId::Rtc];
            let bus = assembled.machine.bus_mut();
            let pit_before = bus.device(pit_index).unwrap().save();
            let rtc_before = bus.device(rtc_index).unwrap().save();
            bus.tick_due(device::VClock(100));
            assert_eq!(bus.device(pit_index).unwrap().save(), pit_before);
            bus.port_write(0x3F8, 1, b'A' as u32).unwrap();
            let mut expected = Pit8253::new();
            expected.sync(device::VClock(100));
            assert_ne!(expected.save(), pit_before);
            assert_eq!(bus.device(pit_index).unwrap().save(), expected.save());
            assert_eq!(bus.device(rtc_index).unwrap().save(), rtc_before);
        }
    }

    #[test]
    fn non_step_bounded_cpu_is_a_clear_configuration_error() {
        let mut cpu = X86JitCpu::new(0x10000, 0x1000, &[0xF4]).unwrap();
        let error = validate_step_cpu(&mut cpu).unwrap_err();
        assert!(matches!(error, ConfigError::StepBoundedCpuRequired(_)));
        assert!(error
            .to_string()
            .contains("Real16 CPU with step-bounded execution"));
        assert_eq!(cpu.retired(), 0);
    }

    #[test]
    fn assembly_rejects_cpu_without_step_bounds_before_execution() {
        let error = assemble_with_cpu(&reference_config(), &ObservationPolicy::default(), |_| {
            Ok(X86JitCpu::new(0x10000, 0x1000, &[0xF4])?)
        });
        assert!(matches!(
            error,
            Err(AssemblyError::Config(ConfigError::StepBoundedCpuRequired(
                _
            )))
        ));
    }

    #[test]
    fn every_preset_applies_trace_policy_to_every_device() {
        let configs = [
            crate::at_boot(vec![0xF4]),
            crate::at_keyboard(vec![0xF4]),
            crate::at_rtc(vec![0xF4], 64, 8),
            crate::at_fdc_write(vec![0xF4], vec![0; 512], 0x600),
            crate::at_vga_text(vec![0xF4]),
            crate::test386(vec![0; 65536]),
            crate::floppy_boot(vec![0xF4], vec![0; 1_474_560]),
            crate::bios(vec![0; 32768], None),
            crate::bios(vec![0; 32768], Some(vec![0; 1_474_560])),
        ];
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Ring { capacity: 0 },
            ..ObservationPolicy::default()
        };
        for config in configs {
            let mut assembled = assemble_with_observation(&config, &policy).unwrap();
            for dc in &config.devices {
                let port = if dc.id == DeviceId::Fdc {
                    0x3F4
                } else {
                    dc.ports[0].start
                };
                let index = assembled.device_index[&dc.id];
                if dc.ports.is_empty() {
                    // The no-media BIOS keeps an undecoded FDC only for CMOS derivation.
                    assembled
                        .machine
                        .bus_mut()
                        .device_mut(index)
                        .unwrap()
                        .port_read(port, 1)
                        .unwrap();
                } else {
                    assembled.machine.bus_mut().port_read(port, 1).unwrap();
                }
                let device = assembled.machine.bus_mut().device_mut(index).unwrap();
                assert!(
                    device.trace_dropped() > 0,
                    "{:?} ignored zero-capacity recording",
                    dc.id
                );
                assert!(device.take_trace().is_empty());
            }
        }
    }

    #[test]
    fn configured_cpu_rate_drives_assembled_clock() {
        for (numerator, denominator, clocks) in [(1, 1, 4), (2, 1, 2), (1, 3, 12), (3, 2, 2)] {
            let mut config = reference_config();
            config.clock.retired_per_pit_clock = numerator;
            config.clock.retired_per_pit_clock_denominator = denominator;
            // Two MOVs and CLI; HLT retire four instructions without device access.
            config.images[0].source = ImageSource::Inline({
                let mut rom = vec![0; ROM_LEN as usize];
                rom[..6].copy_from_slice(&[0xB0, 0, 0xB0, 0, 0xFA, 0xF4]);
                rom
            });
            config.cpu.reset = ResetSpec::Entry { cs: 0xF800, ip: 0 };
            config.cpu.segments = SegmentSeed::flat(0xF800);
            let mut assembled = assemble(&config).expect("assembles");
            assembled.machine.run().expect("run");
            assert_eq!(assembled.machine.cpu().retired(), 4);
            assert_eq!(assembled.machine.clock(), device::VClock(clocks));
        }
    }

    #[test]
    fn invalid_cpu_rates_are_rejected_before_assembly() {
        for (numerator, denominator) in [(0, 1), (1, 0), (0, 0)] {
            let mut config = reference_config();
            config.clock.retired_per_pit_clock = numerator;
            config.clock.retired_per_pit_clock_denominator = denominator;
            assert!(matches!(
                assemble(&config),
                Err(AssemblyError::Config(ConfigError::InvalidCpuRate { .. }))
            ));
        }
    }

    #[test]
    fn device_index_map_follows_config_order() {
        let assembled = assemble(&reference_config()).expect("assembles");
        let map = &assembled.device_index;
        // The devices land in config order, each role at its own bus slot; the
        // master and slave PIC faces are two distinct slots of the one live chip.
        assert_eq!(map[&DeviceId::Uart], 0);
        assert_eq!(map[&DeviceId::Pit], 1);
        assert_eq!(map[&DeviceId::MasterPic], 2);
        assert_eq!(map[&DeviceId::SlavePic], 3);
        assert_eq!(map[&DeviceId::Keyboard], 4);
        assert_eq!(map[&DeviceId::Rtc], 5);
        assert_eq!(map[&DeviceId::Vga], 6);
        assert_eq!(map[&DeviceId::SysControl], 7);
        assert_eq!(map[&DeviceId::Post], 8);
        assert_eq!(map[&DeviceId::Fdc], 9);
        assert_eq!(map[&DeviceId::Dma], 10);
        assert_eq!(map.len(), 11);
    }

    #[test]
    fn isa_decode_map_preserves_each_windows_width() {
        let mut config = reference_config();
        let index = config
            .devices
            .iter()
            .enumerate()
            .map(|(index, device)| (device.id, index))
            .collect();
        let byte_map = build_decode_map(&config, &index);
        assert!(byte_map
            .ports
            .iter()
            .all(|range| range.width == bus::PortWidth::Byte));
        let post = config
            .devices
            .iter_mut()
            .find(|device| device.id == DeviceId::Post)
            .unwrap();
        post.ports = vec![
            PortWindow::single(0x190).with_width(bus::PortWidth::Word),
            PortWindow::single(0x191),
        ];
        let map = build_decode_map(&config, &index);
        for (port, width) in [(0x190, bus::PortWidth::Word), (0x191, bus::PortWidth::Byte)] {
            let range = map
                .ports
                .iter()
                .find(|range| range.ports.contains(&port))
                .unwrap();
            assert_eq!(range.width, width);
            assert_eq!(range.device, index[&DeviceId::Post]);
        }
    }

    #[test]
    fn decode_map_routes_key_ports_and_mmio() {
        let config = reference_config();
        let index = {
            // Rebuild the index the same way assemble does, to assert the map.
            let assembled = assemble(&config).expect("assembles");
            assembled.device_index
        };
        let map = build_decode_map(&config, &index);

        let port_owner = |port: u16| -> Option<usize> {
            map.ports
                .iter()
                .find(|r| r.ports.contains(&port))
                .map(|r| r.device)
        };
        assert_eq!(port_owner(0x3F8), Some(index[&DeviceId::Uart])); // COM1
        assert_eq!(port_owner(0x20), Some(index[&DeviceId::MasterPic]));
        assert_eq!(port_owner(0xA0), Some(index[&DeviceId::SlavePic]));
        assert_eq!(port_owner(0x80), Some(index[&DeviceId::Post])); // POST latch
        assert_eq!(port_owner(0x81), Some(index[&DeviceId::Dma])); // page latch
        assert_eq!(port_owner(0x60), Some(index[&DeviceId::Keyboard]));

        // Ports are sorted ascending, a pure function of the config.
        let starts: Vec<u16> = map.ports.iter().map(|r| *r.ports.start()).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted);

        // The text buffer at 0xB8000 traps to the VGA card.
        let vga = index[&DeviceId::Vga];
        assert!(map
            .mmio
            .iter()
            .any(|r| r.device == vga && r.addrs.contains(&TEXT_BASE)));
    }

    #[test]
    fn wiring_tables_resolve_ids_to_indices() {
        let config = reference_config();
        let assembled = assemble(&config).expect("assembles");
        let index = &assembled.device_index;

        let irqs = build_irq_lines(&config, index);
        // PIT→0, keyboard→1, RTC→8, FDC→6, each resolved to its bus slot.
        assert!(irqs
            .iter()
            .any(|l| l.device == index[&DeviceId::Pit] && l.irq == 0));
        assert!(irqs
            .iter()
            .any(|l| l.device == index[&DeviceId::Rtc] && l.irq == 8));
        assert!(irqs
            .iter()
            .any(|l| l.device == index[&DeviceId::Fdc] && l.irq == 6));

        let dma = build_dma_wirings(&config, index);
        assert_eq!(dma.len(), 1);
        assert_eq!(dma[0].requester, index[&DeviceId::Fdc]);
        assert_eq!(dma[0].controller, index[&DeviceId::Dma]);
    }

    #[test]
    fn derived_cmos_equals_default_for_the_freedos_machine() {
        // The reference machine is a plain AT: 640 KiB base, one 1.44 MB floppy,
        // no hard disk or coprocessor. Its derived CMOS must match the device's
        // own default byte-for-byte, or a stock BIOS's equipment probe would see
        // a self-inconsistent machine.
        let derived = derive_cmos_config(&reference_config());
        assert_eq!(derived, CmosConfig::default());
    }

    #[test]
    fn assembled_cpu_maps_are_live() {
        // The ROM is loaded at its base and reads back through the CPU seam; low
        // RAM is writable. This proves the memory mosaic was laid out, without a
        // full boot.
        let mut assembled = assemble(&reference_config()).expect("assembles");
        let cpu = assembled.machine.cpu();
        let mut rom = [0xFFu8; 4];
        cpu.read_mem(ROM_BASE, &mut rom).expect("rom readable");
        assert_eq!(rom, [0u8; 4]);

        let bus = assembled.machine.bus_mut();
        // The bus holds the full device set the config named.
        assert!(bus.device_mut(0).is_some());
        assert!(bus.device_mut(10).is_some());
    }

    #[test]
    fn guest_word_access_straddles_mda_and_colour_scanout() {
        for reverse_windows in [false, true] {
            let mut config = crate::bios(vec![0; 0x8000], None);
            if reverse_windows {
                config.memory.mmio.reverse();
            }
            config.cpu.reset = ResetSpec::Entry { cs: 0, ip: 0x7C00 };
            config.cpu.segments = SegmentSeed::flat(0);
            config.cpu.segments.ds = 0xB000;
            config.images.push(LoadImage {
                base: 0x7C00,
                source: ImageSource::Inline(vec![
                    0xFA, 0xC7, 0x06, 0xFF, 0x7F, 0x34, 0x12, 0xA1, 0xFF, 0x7F, 0xF4,
                ]),
                kind: ImageKind::RamLoad,
                provides: Vec::new(),
            });
            let mut assembled = assemble(&config).unwrap();
            assert_eq!(
                assembled.machine.run_bounded(Some(100)).unwrap(),
                machine::RunOutcome::Halted
            );
            assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rax), 0x1234);
            assembled.machine.bus_mut().settle();
            let scanout = |card| {
                assembled
                    .machine
                    .bus()
                    .device(assembled.device_index[&card])
                    .unwrap()
                    .text_scanout()
                    .unwrap()
            };
            let mda = scanout(DeviceId::Mda);
            let colour = scanout(DeviceId::Vga);
            assert_eq!(mda.vram.last(), Some(&0x34));
            assert_eq!(mda.vram[0], 0);
            assert_eq!(&colour.vram[..2], &[0x12, 0]);
        }
    }

    #[test]
    fn cpu_trap_regions_merge_chains_but_preserve_ram_gaps() {
        let mut config = crate::bios(vec![0; 0x8000], None);
        config.memory.mmio = vec![
            MmioWindow::new(0xB0005, 3, DeviceId::Mda),
            MmioWindow::new(0xB8000, 0x8000, DeviceId::Vga),
            MmioWindow::new(0xB0000, 2, DeviceId::Mda),
            MmioWindow::new(0xB0002, 3, DeviceId::Mda),
        ];
        config.memory.ram = vec![
            RamRegion::new(0, 0xB0000),
            RamRegion::new(0xB0008, 0xB8000 - 0xB0008),
            RamRegion::new(0xC0000, 0xF8000 - 0xC0000),
        ];
        config.validate().unwrap();
        let mut cpu = build_cpu(&config).unwrap();
        let mut bytes = [0xFF; 8];
        cpu.read_mem(0xB0000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
        cpu.write_mem(0xB0008, &[0x5A]).unwrap();
        cpu.read_mem(0xB0008, &mut bytes[..1]).unwrap();
        assert_eq!(bytes[0], 0x5A);
        assert!(cpu.read_mem(0xB0007, &mut bytes[..2]).is_err());
        cpu.read_mem(0xB8000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
    }

    #[test]
    fn invalid_config_is_rejected_before_assembly() {
        let mut config = reference_config();
        // A stray UART window over the PIT's 0x40 base is an unallowed overlap.
        config.devices[0].ports.push(PortWindow::single(0x40));
        let result = assemble(&config);
        assert!(matches!(
            result,
            Err(AssemblyError::Config(ConfigError::PortOverlap {
                a: DeviceId::Uart,
                b: DeviceId::Pit,
                port: 0x40,
            }))
        ));
    }

    #[test]
    fn a_machine_with_no_pic_gets_a_null_controller() {
        // A minimal machine with a single UART and no interrupt hardware still
        // assembles: the loop is handed a controller that acknowledges nothing.
        let config = MachineConfig {
            board: BoardModel::Bare,
            cpu: CpuConfig {
                chip: crate::cpu::CpuChip::I80286,
                reset: ResetSpec::Entry { cs: 0, ip: 0 },
                segments: SegmentSeed::flat(0),
            },
            clock: ClockConfig::default(),
            memory: MemoryConfig {
                address_space_bytes: 0x1_0000,
                ram: vec![RamRegion::new(0, 0x1_0000)],
                mmio: Vec::new(),
            },
            devices: vec![DeviceConfig::new(
                DeviceId::Uart,
                DeviceKind::Uart(UartCfg { base: 0x3F8 }),
                vec![PortWindow::new(0x3F8, 0x3FF)],
            )],
            drives: Vec::new(),
            images: Vec::new(),
            output: OutputConfig::default(),
        };
        let assembled = assemble(&config).expect("assembles");
        assert_eq!(assembled.device_index[&DeviceId::Uart], 0);
    }

    #[test]
    fn floppy_type_codes_match_the_standard_nibbles() {
        assert_eq!(floppy_cmos_type(&FloppyGeometry::HD_1_44), 4);
        assert_eq!(floppy_cmos_type(&FloppyGeometry::HD_1_2), 2);
        assert_eq!(floppy_cmos_type(&FloppyGeometry::DD_720K), 3);
        assert_eq!(floppy_cmos_type(&FloppyGeometry::DD_360K), 1);
    }

    /// The reference machine with two floppies attached: a 1.2 MB drive A: in slot
    /// 0 and a 720 KB drive B: in slot 1, each with a correctly sized blank image.
    fn two_floppy_config() -> MachineConfig {
        let mut config = reference_config();
        config.drives = vec![
            DriveConfig {
                kind: DriveKind::Floppy {
                    geometry: FloppyGeometry::HD_1_2,
                },
                image: ImageSource::Blank {
                    size: FloppyGeometry::HD_1_2.image_bytes(),
                },
                controller: DeviceId::Fdc,
                unit: 0,
                write: WriteMode::Volatile,
            },
            DriveConfig {
                kind: DriveKind::Floppy {
                    geometry: FloppyGeometry::DD_720K,
                },
                image: ImageSource::Blank {
                    size: FloppyGeometry::DD_720K.image_bytes(),
                },
                controller: DeviceId::Fdc,
                unit: 1,
                write: WriteMode::Volatile,
            },
        ];
        config
    }

    #[test]
    fn two_floppies_route_to_slots_and_derive_cmos_with_no_manual_edit() {
        let config = two_floppy_config();

        // (a) Both drives land in the controller's slots, each carrying the exact
        // geometry the config named — slot 0 is the 1.2 MB drive, slot 1 the 720 KB.
        let slots = build_fdc_drives(&config, DeviceId::Fdc).expect("drives mounted");
        assert_eq!(
            slots[0].as_ref().expect("slot 0 mounted").geometry,
            to_device_geometry(&FloppyGeometry::HD_1_2)
        );
        assert_eq!(
            slots[1].as_ref().expect("slot 1 mounted").geometry,
            to_device_geometry(&FloppyGeometry::DD_720K)
        );
        assert!(slots[2].is_none() && slots[3].is_none());

        // (b) The CMOS floppy nibbles and equipment byte fall out of the attached
        // drives alone — no caller ever edits CMOS. High nibble = slot 0 (1.2 MB →
        // code 2), low nibble = slot 1 (720 KB → code 3); equipment bit 0 set (a
        // floppy is present) and bits 6-7 = 01 (two drives installed).
        let cmos = derive_cmos_config(&config);
        assert_eq!(cmos.floppy_types, 0x23);
        assert_eq!(cmos.equipment & 0x01, 0x01);
        assert_eq!(cmos.equipment >> 6, 0b01);

        // And the whole machine assembles from that config unchanged.
        assert!(assemble(&config).is_ok());
    }

    #[test]
    fn write_modes_route_to_the_mounted_drive() {
        // Two drives on one controller: slot 0 writable (the default), slot 1
        // write-protected. The mode must reach the device drive it belongs to.
        let mut config = reference_config();
        config.drives = vec![
            DriveConfig {
                kind: DriveKind::Floppy {
                    geometry: FloppyGeometry::HD_1_44,
                },
                image: ImageSource::Blank {
                    size: FloppyGeometry::HD_1_44.image_bytes(),
                },
                controller: DeviceId::Fdc,
                unit: 0,
                write: WriteMode::Volatile,
            },
            DriveConfig {
                kind: DriveKind::Floppy {
                    geometry: FloppyGeometry::HD_1_44,
                },
                image: ImageSource::Blank {
                    size: FloppyGeometry::HD_1_44.image_bytes(),
                },
                controller: DeviceId::Fdc,
                unit: 1,
                write: WriteMode::ReadOnly,
            },
        ];

        let slots = build_fdc_drives(&config, DeviceId::Fdc).expect("drives mounted");
        assert!(
            !slots[0].as_ref().expect("slot 0 mounted").write_protected(),
            "the default drive is writable"
        );
        assert!(
            slots[1].as_ref().expect("slot 1 mounted").write_protected(),
            "the flagged drive is mounted write-protected"
        );
    }

    #[test]
    fn cmos_floppy_inventory_uses_only_the_primary_controller() {
        let mut config = two_floppy_config();
        let secondary = config
            .devices
            .iter_mut()
            .find(|d| d.id == DeviceId::Uart)
            .unwrap();
        secondary.kind = DeviceKind::Fdc(FdcCfg { drive_slots: 2 });
        secondary.ports.clear();
        config.drives[1].controller = DeviceId::Uart;
        config.drives[1].unit = 0;
        assert_eq!(config.validate(), Ok(()));
        let cmos = derive_cmos_config(&config);
        assert_eq!(cmos.floppy_types, 0x20);
        assert_eq!(cmos.equipment, 0x01);
        config.drives.remove(0);
        let cmos = derive_cmos_config(&config);
        assert_eq!(cmos.floppy_types, 0);
        assert_eq!(cmos.equipment, 0);
    }

    #[test]
    fn named_controllers_keep_their_media_separate() {
        let mut config = two_floppy_config();
        let uart = config
            .devices
            .iter_mut()
            .find(|d| d.id == DeviceId::Uart)
            .unwrap();
        uart.kind = DeviceKind::Fdc(FdcCfg { drive_slots: 2 });
        uart.ports.clear();
        config.drives[1].controller = DeviceId::Uart;
        config.drives[1].unit = 0;
        for (drive, byte) in config.drives.iter_mut().zip([0x12, 0x72]) {
            drive.image = ImageSource::Inline(vec![byte; drive.kind.image_bytes() as usize]);
        }
        assert_eq!(config.validate(), Ok(()));
        let mut assembled = assemble(&config).unwrap();
        for (controller, geometry, byte) in [
            (DeviceId::Fdc, FloppyGeometry::HD_1_2, 0x12),
            (DeviceId::Uart, FloppyGeometry::DD_720K, 0x72),
        ] {
            let slots = build_fdc_drives(&config, controller).unwrap();
            assert_eq!(
                slots[0].as_ref().unwrap().geometry,
                to_device_geometry(&geometry)
            );
            assert!(slots[1..].iter().all(Option::is_none));
            let index = assembled.device_index[&controller];
            let fdc = assembled.machine.bus_mut().device_mut(index).unwrap();
            // Direct accesses let the bare rig exercise two controller roles
            // without asking the fixed-port FDC model to relocate its registers.
            fdc.port_write(0x3F2, 1, 0x1C).unwrap();
            for value in [0x06, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
                fdc.port_write(0x3F5, 1, value).unwrap();
            }
            let DmaOp::ToMemory(data) = fdc.take_dma_request().unwrap().op else {
                panic!("read must queue data");
            };
            assert_eq!(data, vec![byte; 512]);
        }
    }

    #[test]
    fn readonly_guest_write_reports_not_writable_without_mutating_media() {
        assert_eq!(WriteMode::default(), WriteMode::Volatile);
        let mut config = reference_config();
        config.drives[0].write = WriteMode::ReadOnly;
        let original = vec![0; FloppyGeometry::HD_1_44.image_bytes() as usize];
        let mut assembled = assemble(&config).unwrap();
        let index = assembled.device_index[&DeviceId::Fdc];
        let fdc = assembled.machine.bus_mut().device_mut(index).unwrap();
        fdc.port_write(0x3F2, 1, 0x1C).unwrap();
        for byte in [0x05, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
            fdc.port_write(0x3F5, 1, byte).unwrap();
        }
        assert!(fdc.take_dma_request().is_none());
        let result: Vec<_> = (0..7).map(|_| fdc.port_read(0x3F5, 1).unwrap()).collect();
        assert_eq!(result, [0x40, 0x02, 0, 0, 0, 1, 2]);
        for byte in [0x06, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
            fdc.port_write(0x3F5, 1, byte).unwrap();
        }
        let DmaOp::ToMemory(data) = fdc.take_dma_request().unwrap().op else {
            panic!("read must queue data");
        };
        assert_eq!(data, original[..512]);
    }

    /// The reference machine carrying a single floppy of `geometry` backed by
    /// `image` in slot 0.
    fn single_floppy_config(geometry: FloppyGeometry, image: ImageSource) -> MachineConfig {
        let mut config = reference_config();
        config.drives = vec![DriveConfig {
            kind: DriveKind::Floppy { geometry },
            image,
            controller: DeviceId::Fdc,
            unit: 0,
            write: WriteMode::Volatile,
        }];
        config
    }

    #[test]
    fn a_guest_reads_at_the_configured_geometry_and_faults_past_it() {
        const SECTOR: usize = 512;
        // µPD765 data/DOR ports; the guest drives these through the assembled bus.
        const DOR: u16 = 0x3F2;
        const DATA: u16 = 0x3F5;

        // A 360 KB drive spans 40 cylinders (0..=39). Seed a distinctive first
        // sector so the queued DMA bytes are recognisable.
        let mut image = vec![0u8; FloppyGeometry::DD_360K.image_bytes() as usize];
        for (i, b) in image.iter_mut().take(SECTOR).enumerate() {
            *b = (i % 251) as u8;
        }
        let config =
            single_floppy_config(FloppyGeometry::DD_360K, ImageSource::Inline(image.clone()));
        let mut assembled = assemble(&config).expect("assembles");
        let idx = assembled.device_index[&DeviceId::Fdc];
        let bus = assembled.machine.bus_mut();
        let fdc = bus.device_mut(idx).expect("fdc is on the bus");

        // Bring the controller out of reset (not-reset, DMA, motor 0, drive 0) and
        // drain the four post-reset sense polls, exactly as a BIOS would.
        fdc.port_write(DOR, 1, 0x1C).unwrap();
        for _ in 0..4 {
            fdc.port_write(DATA, 1, 0x08).unwrap(); // SENSE INTERRUPT STATUS
            let _ = fdc.port_read(DATA, 1).unwrap();
            let _ = fdc.port_read(DATA, 1).unwrap();
        }

        // READ DATA at C=0 H=0 R=1 — a valid sector on a 360 KB disk. The
        // controller reads the configured drive's sector 0 and queues it for DMA.
        let read = [0x06u8, 0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF];
        for &b in &read {
            fdc.port_write(DATA, 1, u32::from(b)).unwrap();
        }
        let request = fdc
            .take_dma_request()
            .expect("an in-geometry read queues a DMA transfer");
        let DmaOp::ToMemory(data) = request.op else {
            panic!("a floppy read queues a device -> memory transfer");
        };
        assert_eq!(
            &data[..],
            &image[..SECTOR],
            "the queued bytes are the configured drive's sector 0"
        );
        fdc.dma_complete(DmaOutcome::ToMemory {
            transferred: SECTOR,
            terminal_count: true,
        });
        let st0 = fdc.port_read(DATA, 1).unwrap() as u8;
        assert_eq!(st0 & 0xC0, 0, "normal termination for an in-geometry read");
        for _ in 0..6 {
            let _ = fdc.port_read(DATA, 1).unwrap(); // drain the rest of the result
        }

        // READ DATA at C=40 — one past a 360 KB disk's last cylinder, though it
        // would be valid on a 1.44 MB disk (80 cylinders). The controller honours
        // the CONFIGURED geometry: there is no data field, so it faults with no
        // transfer instead of reading a 1.44 MB-shaped sector.
        let past = [0x06u8, 0x00, 40, 0x00, 0x01, 0x02, 0x01, 0x1B, 0xFF];
        for &b in &past {
            fdc.port_write(DATA, 1, u32::from(b)).unwrap();
        }
        assert!(
            fdc.take_dma_request().is_none(),
            "an out-of-geometry read queues no transfer"
        );
        let st0 = fdc.port_read(DATA, 1).unwrap() as u8;
        let st1 = fdc.port_read(DATA, 1).unwrap() as u8;
        assert_eq!(st0 & 0x40, 0x40, "abnormal termination (ST0 IC = 01)");
        assert_eq!(st1 & 0x04, 0x04, "ST1 ND: no data field past the geometry");
    }

    #[test]
    fn unreadable_floppy_file_is_an_error() {
        let mut config = reference_config();
        config.drives[0].image = ImageSource::File(std::path::PathBuf::new());
        assert!(
            matches!(assemble(&config), Err(AssemblyError::Config(ConfigError::ImageRead { path, message })) if path.as_os_str().is_empty() && !message.is_empty())
        );
    }

    #[test]
    fn file_backed_floppy_geometry_is_checked() {
        let mut config = reference_config();
        config.drives[0].image =
            ImageSource::File(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml").into());
        assert!(
            matches!(assemble(&config), Err(AssemblyError::Config(ConfigError::ImageGeometryMismatch { drive_index: 0, expected: 1_474_560, actual })) if actual == include_bytes!("../Cargo.toml").len() as u64)
        );
    }

    #[test]
    fn file_backed_load_image_bounds_are_checked() {
        let mut config = reference_config();
        config.images[0] = LoadImage {
            base: SPACE - 1,
            source: ImageSource::File(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml").into()),
            kind: ImageKind::RamLoad,
            provides: Vec::new(),
        };
        assert!(
            matches!(assemble(&config), Err(AssemblyError::Config(ConfigError::RegionOutOfSpace { base, end, space })) if base == SPACE - 1 && end == base + include_bytes!("../Cargo.toml").len() as u64 && space == SPACE),
            "file bounds must produce a config error"
        );
    }

    fn file_source() -> ImageSource {
        ImageSource::File(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml").into())
    }

    #[test]
    fn file_backed_load_over_mmio_is_rejected() {
        let mut config = reference_config();
        config.images[0] = LoadImage {
            base: TEXT_BASE,
            source: file_source(),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        };
        assert!(matches!(
            assemble(&config),
            Err(AssemblyError::Config(ConfigError::ImageOverlapsMmio {
                image_index: 0,
                device: DeviceId::Vga,
                addr: TEXT_BASE
            }))
        ));
    }

    #[test]
    fn file_backed_load_over_another_image_is_rejected() {
        let mut config = reference_config();
        config.images.push(LoadImage {
            base: ROM_BASE + 1,
            source: file_source(),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        });
        assert!(
            matches!(assemble(&config), Err(AssemblyError::Config(ConfigError::ImageOverlap { a: 0, b: 1, addr })) if addr == ROM_BASE + 1)
        );
    }

    #[test]
    fn file_backed_ram_load_outside_ram_is_rejected() {
        let mut config = reference_config();
        config.images[0] = LoadImage {
            base: ROM_BASE,
            source: file_source(),
            kind: ImageKind::RamLoad,
            provides: Vec::new(),
        };
        assert!(matches!(
            assemble(&config),
            Err(AssemblyError::Config(ConfigError::ImageOutsideRam {
                image_index: 0,
                addr: ROM_BASE
            }))
        ));
    }

    #[test]
    fn file_backed_ram_load_preserves_bytes_and_source_config() {
        let mut config = reference_config();
        config.images[0] = LoadImage {
            base: 0,
            source: file_source(),
            kind: ImageKind::RamLoad,
            provides: Vec::new(),
        };
        let assembled = assemble(&config).expect("readable RAM load assembles");
        let expected = include_bytes!("../Cargo.toml");
        let mut actual = vec![0; expected.len()];
        assembled.machine.cpu().read_mem(0, &mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(config.images[0].source, file_source());
    }
}
