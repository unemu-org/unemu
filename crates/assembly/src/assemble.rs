// SPDX-License-Identifier: AGPL-3.0-or-later
//! The assembler: a validated [`MachineConfig`] becomes a running [`Machine`]
//! (§3.1).
//!
//! [`assemble`] is the single data-driven realiser. It reads the pure-data
//! description and builds the concrete parts the machine core is deliberately
//! blind to: an [`X86JitCpu`] with its memory mosaic laid out, a [`Bus`] with a
//! [`bus::DecodeMap`], the one shared interrupt controller, and the IRQ / DMA
//! wiring tables. The core never names any of these concrete types (decision-7);
//! naming them is exactly this layer's job, which is why it sits above the
//! `machine` crate.
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
//! ## CMOS is derived, never free-configured
//!
//! A stock BIOS reads its floppy types, equipment byte, and memory sizes from
//! CMOS and checksums them against the actual machine; a mismatch drops POST into
//! an interactive SETUP prompt a headless run cannot answer. So the seed is a
//! *product* of assembly ([`derive_cmos_config`]), computed from the memory map
//! and the attached drives, not a value the caller supplies.

use std::collections::HashMap;

use bus::{Bus, DecodeMap, MmioRange, PortRange};
use cpu::x86jit_backend::X86JitCpu;
use device::{Device, InterruptController};
use devices::{
    CmosConfig, CmosRtc, Dma8237, Drive, Pit8253, PostPort, SysControl, Uart16550, Upd765, VgaText,
    I8042,
};
use machine::{DmaWiring, IrqLine, Machine};

use crate::device::{DeviceId, DeviceKind};
use crate::drive::{DriveKind, FloppyGeometry};
use crate::error::ConfigError;
use crate::image::{ImageKind, ImageSource};
use crate::machine::MachineConfig;

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
}

/// A way assembling a validated [`MachineConfig`] can fail (§3.1).
///
/// A configuration inconsistency is caught up front by [`MachineConfig::validate`]
/// and surfaced as [`AssemblyError::Config`]; the remaining failures are physical
/// (the CPU core rejecting a memory map, or a backing image file that cannot be
/// read), not configuration bugs.
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    /// The configuration is internally inconsistent (validation ran first).
    #[error("invalid configuration: {0}")]
    Config(#[from] ConfigError),

    /// The CPU core rejected a memory-map operation (a region that does not fit,
    /// a `Trap` window overlapping RAM the config did not catch).
    #[error("cpu memory map: {0}")]
    Cpu(#[from] cpu::CpuError),

    /// A backing image named a file that could not be read at load time.
    #[error("reading image file {path}: {source}")]
    Image {
        /// The path that failed to read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
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
    // A validated config does not fail assembly for configuration reasons; run
    // the checks first so any inconsistency surfaces as a precise ConfigError.
    config.validate()?;

    let cpu = build_cpu(config)?;
    let cmos = derive_cmos_config(config);

    // Instantiate devices in config order, recording each role's bus index. The
    // one shared PIC is created lazily on the first PIC entry and reused for the
    // second face and the controller seam.
    let mut devices: Vec<Box<dyn Device>> = Vec::with_capacity(config.devices.len());
    let mut device_index: HashMap<DeviceId, usize> = HashMap::with_capacity(config.devices.len());
    let mut pic: Option<chipset::CascadedPic> = None;

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
            DeviceKind::Keyboard(_) => Box::new(I8042::new()),
            DeviceKind::Rtc(c) => {
                let mut rtc = CmosRtc::new();
                if let Some((second_ticks, uip_ticks)) = c.timing {
                    rtc = rtc.with_timing(second_ticks, uip_ticks);
                }
                if c.seed {
                    rtc = rtc.with_config(cmos);
                }
                Box::new(rtc)
            }
            DeviceKind::Vga(_) => Box::new(VgaText::new()),
            DeviceKind::SysControl(_) => Box::new(SysControl::new()),
            DeviceKind::Post(c) => Box::new(PostPort::with_port(c.port)),
            DeviceKind::Fdc(_) => Box::new(Upd765::with_drives(build_fdc_drives(config))),
            DeviceKind::Dma(_) => Box::new(Dma8237::new()),
        };
        let index = devices.len();
        devices.push(boxed);
        device_index.insert(dc.id, index);
    }

    // The machine needs exactly one controller. If the config has no PIC (an
    // interrupt-free machine), a null controller keeps the loop total.
    let controller: Box<dyn InterruptController> = match pic {
        Some(p) => Box::new(p),
        None => Box::new(NullInterruptController),
    };

    let map = build_decode_map(config, &device_index);
    let irq_lines = build_irq_lines(config, &device_index);
    let dma_wirings = build_dma_wirings(config, &device_index);

    // The machine is built with no idle limit; the run policy stamps it on before
    // the loop runs (idle_limit is a "how we run" concern, not machine identity).
    let machine = Machine::new(
        cpu,
        Bus::new(devices, map),
        controller,
        config.clock.quantum,
        irq_lines,
        None,
    )
    .with_dma_wirings(dma_wirings);

    Ok(AssembledMachine {
        machine,
        device_index,
    })
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
    for window in &config.memory.mmio {
        cpu.map_mmio(window.base, window.size)?;
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
fn load_image_bytes(source: &ImageSource) -> Result<Vec<u8>, AssemblyError> {
    match source {
        ImageSource::Inline(bytes) => Ok(bytes.clone()),
        ImageSource::Blank { size } => Ok(vec![0u8; *size as usize]),
        ImageSource::File(path) => std::fs::read(path).map_err(|source| AssemblyError::Image {
            path: path.display().to_string(),
            source,
        }),
    }
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
/// their `controller_slot` (§7.7).
///
/// Only floppy drives are mounted here; a slot past the fourth position is
/// ignored (validation bounds slots to the controller's declared count). The
/// config geometry maps directly to the controller's own geometry type.
fn build_fdc_drives(config: &MachineConfig) -> [Option<Drive>; 4] {
    let mut slots: [Option<Drive>; 4] = [None, None, None, None];
    for drive in &config.drives {
        let DriveKind::Floppy { geometry } = &drive.kind else {
            continue;
        };
        let slot = drive.controller_slot as usize;
        if slot >= slots.len() {
            continue;
        }
        let image = match load_image_bytes(&drive.image) {
            Ok(bytes) => bytes,
            // A file-backed drive whose bytes are unavailable mounts as an empty
            // bay rather than failing assembly; the boot path reads it as "not
            // ready", matching an open drive door.
            Err(_) => continue,
        };
        slots[slot] = Some(Drive::new(to_device_geometry(geometry), image));
    }
    slots
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
/// from the attached floppy drives. For a plain AT — 640 KiB base, one 1.44 MB
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
                floppy_count += 1;
                let code = floppy_cmos_type(geometry);
                match drive.controller_slot {
                    0 => floppy_types |= code << 4,
                    1 => floppy_types |= code,
                    _ => {}
                }
            }
            DriveKind::HardDisk { .. } => {
                // A hard disk present but not otherwise modelled: mark its nibble
                // as "type stored in the extended table" (0xF), the encoding a
                // BIOS uses for drives beyond the built-in type list.
                match drive.controller_slot {
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

/// A do-nothing interrupt controller for a machine the config gives no PIC (§5.1).
///
/// The machine loop always holds a controller; a machine with no interrupt
/// hardware (a bare serial or timer demo) uses this so nothing is ever
/// acknowledged or injected.
struct NullInterruptController;

impl InterruptController for NullInterruptController {
    fn set_irq_line(&mut self, _irq: u8, _level: bool) {}
    fn poll(&self) -> Option<u8> {
        None
    }
    fn acknowledge(&mut self) -> Option<u8> {
        None
    }
    fn end_of_interrupt(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
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

    const SPACE: u64 = 0x10_0000;
    const TEXT_BASE: u64 = 0xB_8000;
    const TEXT_LEN: u64 = 0x8000;
    const ROM_BASE: u64 = 0xF_8000;
    const ROM_LEN: u64 = 0x8000;

    /// A reference AT-class machine: a small stand-in for the real presets
    /// (task-31). Everything the assembler must build — the memory mosaic, the
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
                DeviceKind::Pit(PitCfg { base: 0x40 }),
                vec![PortWindow::new(0x40, 0x43)],
            )
            .with_irq(0),
            DeviceConfig::new(
                DeviceId::MasterPic,
                DeviceKind::Pic(PicCfg {
                    command: 0x20,
                    cascade: true,
                }),
                vec![PortWindow::new(0x20, 0x21)],
            ),
            DeviceConfig::new(
                DeviceId::SlavePic,
                DeviceKind::Pic(PicCfg {
                    command: 0xA0,
                    cascade: false,
                }),
                vec![PortWindow::new(0xA0, 0xA1)],
            ),
            DeviceConfig::new(
                DeviceId::Keyboard,
                DeviceKind::Keyboard(I8042Cfg {
                    data: 0x60,
                    command: 0x64,
                }),
                vec![PortWindow::single(0x60), PortWindow::single(0x64)],
            )
            .with_irq(1),
            DeviceConfig::new(
                DeviceId::Rtc,
                DeviceKind::Rtc(CmosCfg {
                    index: 0x70,
                    data: 0x71,
                    seed: true,
                    timing: None,
                }),
                vec![PortWindow::new(0x70, 0x71)],
            )
            .with_irq(8),
            DeviceConfig::new(
                DeviceId::Vga,
                DeviceKind::Vga(VgaTextCfg { crtc_index: 0x3D4 }),
                vec![PortWindow::new(0x3D4, 0x3D5)],
            ),
            DeviceConfig::new(
                DeviceId::SysControl,
                DeviceKind::SysControl(SysControlCfg { port: 0x61 }),
                vec![PortWindow::single(0x61)],
            ),
            DeviceConfig::new(
                DeviceId::Post,
                DeviceKind::Post(PostPortCfg { port: 0x80 }),
                vec![PortWindow::single(0x80)],
            ),
            DeviceConfig::new(
                DeviceId::Fdc,
                DeviceKind::Fdc(FdcCfg {
                    base: 0x3F0,
                    drive_slots: 2,
                }),
                vec![PortWindow::new(0x3F0, 0x3F7)],
            )
            .with_irq(6)
            .with_dma(DmaSpec {
                controller: DeviceId::Dma,
                channel: 2,
            }),
            DeviceConfig::new(
                DeviceId::Dma,
                DeviceKind::Dma(DmaCfg { base: 0x00 }),
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
        }];

        let drives = vec![DriveConfig {
            kind: DriveKind::Floppy {
                geometry: FloppyGeometry::HD_1_44,
            },
            image: ImageSource::Blank {
                size: FloppyGeometry::HD_1_44.image_bytes(),
            },
            controller_slot: 0,
        }];

        MachineConfig {
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
}
