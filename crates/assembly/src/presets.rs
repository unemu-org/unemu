// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine-shape presets (§3.1).
//!
//! A preset is a plain function that emits a [`MachineConfig`] for one machine
//! shape — the ergonomic layer over the flat [`DeviceConfig`] list. Each preset
//! builds exactly the topology the corresponding hand-assembled runner used to
//! build by hand (same devices, ports, IRQ/DMA wiring, memory mosaic, reset
//! entry, and clock), so a machine assembled from the preset is observably
//! identical to the legacy one. The frontend picks a preset, assembles it, and
//! runs it; nothing here runs a machine.
//!
//! The old `AtChipset` / `SerialChipset` chipsets are exactly these presets in a
//! different guise: where the chipset trait returned a device set + decode map,
//! a preset returns the pure-data config the assembler realises into the same
//! parts. The one exception is the Long64 serial skeleton, which stays a tiny
//! hand-inlined device set in the frontend (no config mode-axis for a single
//! Long64 user).

use crate::clock::ClockConfig;
use crate::cpu::{CpuChip, CpuConfig, ResetSpec, SegmentSeed};
use crate::device::{
    CmosCfg, DeviceConfig, DeviceId, DeviceKind, DmaCfg, DmaSpec, FdcCfg, I8042Cfg, PicCfg, PitCfg,
    PortWindow, PostPortCfg, SysControlCfg, UartCfg, VgaTextCfg,
};
use crate::drive::{DriveConfig, DriveKind, FloppyGeometry};
use crate::image::{ImageKind, ImageSource, LoadImage};
use crate::machine::MachineConfig;
use crate::memory::{MemoryConfig, MmioWindow, RamRegion};
use crate::output::OutputConfig;

use chipset::{
    COM1_BASE, COM1_TOP, PIC_COMMAND, PIC_DATA, PIT_BASE, PIT_TOP, SLAVE_PIC_COMMAND,
    SLAVE_PIC_DATA,
};
use devices::post_port::POST_PORT;
use devices::vga_text::{
    CGA_MODE_CONTROL_PORT, CGA_STATUS_PORT, CRTC_DATA_PORT, CRTC_INDEX_PORT, MDA_CRTC_DATA_PORT,
    MDA_CRTC_INDEX_PORT, MDA_STATUS_PORT, TEXT_BUFFER_BASE, TEXT_BUFFER_LEN,
};

// ---------------------------------------------------------------------------
// Shared address-space and port constants (the layout the legacy runners baked
// in). They live here now: a preset owns its machine's memory map.
// ---------------------------------------------------------------------------

/// Flat guest RAM for the small real-mode demos: 64 KiB.
const DEMO_RAM_BYTES: u64 = 0x1_0000;
/// Classic boot load segment (CS = 0) the demo blobs run from.
const DEMO_CS: u16 = 0x0000;
/// Classic boot IP (linear load address, CS = 0): `0x7C00`.
const DEMO_IP: u16 = 0x7C00;
/// Load IP of the floppy-boot loader (CS = 0): `0x7E00`.
const FLOPPY_LOADER_IP: u16 = 0x7E00;
/// Segment the VGA text demo points DS/ES at (`0xB800 << 4 == 0xB8000`).
const VGA_TEXT_SEGMENT: u16 = 0xB800;

/// 8042 keyboard-controller data port (`0x60`).
const KBD_DATA: u16 = 0x60;
/// 8042 keyboard-controller command/status port (`0x64`).
const KBD_CMD: u16 = 0x64;
/// MC146818 CMOS/RTC index port (`0x70`).
const CMOS_INDEX_PORT: u16 = 0x70;
/// MC146818 CMOS/RTC data port (`0x71`).
const CMOS_DATA_PORT: u16 = 0x71;
/// System-control latch port (`0x61`: speaker gate, refresh, NMI status).
const SYS_CONTROL_PORT: u16 = 0x61;
/// The stock-BIOS POST checkpoint port (`0x80`).
const BIOS_POST_PORT: u16 = 0x80;
/// First port of the µPD765 floppy controller (`0x3F0`).
const FDC_BASE: u16 = 0x3F0;
/// Last port of the µPD765 floppy controller (`0x3F7`).
const FDC_TOP: u16 = 0x3F7;

/// Full 1 MiB guest space for the test386 and BIOS machines.
const MIB: u64 = 0x10_0000;
/// Segment the test386 diagnostic image is loaded at (`0xF0000`).
const TEST386_LOAD_SEG: u16 = 0xF000;
/// Reset IP the test386 image starts at (`F000:FFF0`).
const RESET_IP: u16 = 0xFFF0;
/// Reset CS the BIOS/test386 machines start at (`0xF000`).
const RESET_CS: u16 = 0xF000;
/// Base of the zeroed option-ROM scan area (`0xC0000`).
const BIOS_OPTION_ROM_BASE: u64 = 0x0C_0000;
/// Base of the masked 32 KiB BIOS ROM (`0xF8000`).
const BIOS_ROM_BASE: u64 = 0x0F_8000;

// ---------------------------------------------------------------------------
// Per-chip device builders. Each returns the DeviceConfig for one modelled chip
// with its decoded ports; the caller adds the IRQ / DMA wiring that a given
// machine needs.
// ---------------------------------------------------------------------------

/// The 16550 UART at COM1.
fn uart() -> DeviceConfig {
    DeviceConfig::new(
        DeviceId::Uart,
        DeviceKind::Uart(UartCfg { base: COM1_BASE }),
        vec![PortWindow::new(COM1_BASE, COM1_TOP)],
    )
}

/// The master 8259 PIC (cascade half of the pair).
fn master_pic() -> DeviceConfig {
    DeviceConfig::new(
        DeviceId::MasterPic,
        DeviceKind::Pic(PicCfg {
            command: PIC_COMMAND,
            cascade: true,
        }),
        vec![PortWindow::new(PIC_COMMAND, PIC_DATA)],
    )
}

/// The slave 8259 PIC (carries IRQ8–IRQ15, cascaded into master IRQ2).
fn slave_pic() -> DeviceConfig {
    DeviceConfig::new(
        DeviceId::SlavePic,
        DeviceKind::Pic(PicCfg {
            command: SLAVE_PIC_COMMAND,
            cascade: false,
        }),
        vec![PortWindow::new(SLAVE_PIC_COMMAND, SLAVE_PIC_DATA)],
    )
}

/// The 8253 PIT; `irq` wires its counter-0 OUT to a PIC input when set.
fn pit(irq: Option<u8>) -> DeviceConfig {
    let dc = DeviceConfig::new(
        DeviceId::Pit,
        DeviceKind::Pit(PitCfg { base: PIT_BASE }),
        vec![PortWindow::new(PIT_BASE, PIT_TOP)],
    );
    match irq {
        Some(line) => dc.with_irq(line),
        None => dc,
    }
}

/// The 8042 keyboard controller (two non-adjacent ports, `0x60` and `0x64`);
/// `irq` wires its keyboard-byte-ready line to a PIC input when set.
fn keyboard(irq: Option<u8>) -> DeviceConfig {
    let dc = DeviceConfig::new(
        DeviceId::Keyboard,
        DeviceKind::Keyboard(I8042Cfg {
            data: KBD_DATA,
            command: KBD_CMD,
        }),
        vec![PortWindow::single(KBD_DATA), PortWindow::single(KBD_CMD)],
    );
    match irq {
        Some(line) => dc.with_irq(line),
        None => dc,
    }
}

/// The MC146818 CMOS/RTC. `seed` writes the AT configuration NVRAM (a stock BIOS
/// checksums it); `timing` overrides the virtual-clock tick ratio; `irq` wires
/// the update-ended output to a PIC input when set.
fn rtc(seed: bool, timing: Option<(u64, u64)>, irq: Option<u8>) -> DeviceConfig {
    let dc = DeviceConfig::new(
        DeviceId::Rtc,
        DeviceKind::Rtc(CmosCfg {
            index: CMOS_INDEX_PORT,
            data: CMOS_DATA_PORT,
            seed,
            timing,
        }),
        vec![PortWindow::new(CMOS_INDEX_PORT, CMOS_DATA_PORT)],
    );
    match irq {
        Some(line) => dc.with_irq(line),
        None => dc,
    }
}

/// The VGA/MDA text card. `bios_aliases` adds the monochrome CRTC and input-status
/// ports the Kiselev BIOS drives in its MDA console path, on top of the CGA-page
/// CRTC and mode/colour/status registers every machine decodes.
fn vga(bios_aliases: bool) -> DeviceConfig {
    let mut ports = vec![
        PortWindow::new(CRTC_INDEX_PORT, CRTC_DATA_PORT),
        PortWindow::new(CGA_MODE_CONTROL_PORT, CGA_STATUS_PORT),
    ];
    if bios_aliases {
        ports.push(PortWindow::new(MDA_CRTC_INDEX_PORT, MDA_CRTC_DATA_PORT));
        ports.push(PortWindow::single(MDA_STATUS_PORT));
    }
    DeviceConfig::new(
        DeviceId::Vga,
        DeviceKind::Vga(VgaTextCfg {
            crtc_index: CRTC_INDEX_PORT,
        }),
        ports,
    )
}

/// The port-`0x61` system-control latch.
fn sys_control() -> DeviceConfig {
    DeviceConfig::new(
        DeviceId::SysControl,
        DeviceKind::SysControl(SysControlCfg {
            port: SYS_CONTROL_PORT,
        }),
        vec![PortWindow::single(SYS_CONTROL_PORT)],
    )
}

/// A POST checkpoint port decoding `port`.
fn post(port: u16) -> DeviceConfig {
    DeviceConfig::new(
        DeviceId::Post,
        DeviceKind::Post(PostPortCfg { port }),
        vec![PortWindow::single(port)],
    )
}

/// The 8237 DMA controller decoding `ports` (its channel registers plus the page
/// latches — the exact latch range differs between machines that overlap the
/// POST port and those that do not).
fn dma(ports: Vec<PortWindow>) -> DeviceConfig {
    DeviceConfig::new(DeviceId::Dma, DeviceKind::Dma(DmaCfg { base: 0x00 }), ports)
}

/// A µPD765 floppy controller offering two drive slots, decoding `ports`, with
/// optional IRQ and DMA wiring.
fn fdc(ports: Vec<PortWindow>, irq: Option<u8>, dma_channel: Option<u8>) -> DeviceConfig {
    let mut dc = DeviceConfig::new(
        DeviceId::Fdc,
        DeviceKind::Fdc(FdcCfg {
            base: FDC_BASE,
            drive_slots: 2,
        }),
        ports,
    );
    if let Some(line) = irq {
        dc = dc.with_irq(line);
    }
    if let Some(channel) = dma_channel {
        dc = dc.with_dma(DmaSpec {
            controller: DeviceId::Dma,
            channel,
        });
    }
    dc
}

/// A single 1.44 MB floppy in drive slot 0 backed by `image`.
fn floppy_drive(image: ImageSource) -> DriveConfig {
    DriveConfig {
        kind: DriveKind::Floppy {
            geometry: FloppyGeometry::HD_1_44,
        },
        image,
        controller_slot: 0,
    }
}

// ---------------------------------------------------------------------------
// Presets: one function per machine shape.
// ---------------------------------------------------------------------------

/// The PC/AT device set shared by the real-mode demos: UART, cascaded 8259 pair,
/// PIT, 8042, VGA text card, and CMOS/RTC. Each argument wires the one interrupt
/// source a given demo takes (the rest stay unwired, exactly as the demo's
/// hand-assembly left them); `rtc_timing` fast-ticks the RTC for the demo that
/// must see an update-ended interrupt within a bounded run.
fn at_device_set(
    pit_irq: Option<u8>,
    kbd_irq: Option<u8>,
    rtc_irq: Option<u8>,
    rtc_timing: Option<(u64, u64)>,
) -> Vec<DeviceConfig> {
    vec![
        uart(),
        master_pic(),
        pit(pit_irq),
        keyboard(kbd_irq),
        vga(false),
        slave_pic(),
        rtc(false, rtc_timing, rtc_irq),
    ]
}

/// A real-mode demo machine: `blob` loaded at `0x7C00` in flat 64 KiB RAM, run
/// from CS:IP `0000:7C00`, on the AT device set `devices`. Used by the boot,
/// keyboard, and RTC demos, which differ only in wiring and segment seed.
fn at_demo(blob: Vec<u8>, devices: Vec<DeviceConfig>) -> MachineConfig {
    MachineConfig {
        cpu: CpuConfig {
            chip: CpuChip::I80286,
            reset: ResetSpec::Entry {
                cs: DEMO_CS,
                ip: DEMO_IP,
            },
            segments: SegmentSeed::flat(DEMO_CS),
        },
        clock: ClockConfig::default(),
        memory: MemoryConfig {
            address_space_bytes: DEMO_RAM_BYTES,
            ram: vec![RamRegion::new(0, DEMO_RAM_BYTES)],
            mmio: Vec::new(),
        },
        devices,
        drives: Vec::new(),
        images: vec![LoadImage {
            base: (u64::from(DEMO_CS) << 4) + u64::from(DEMO_IP),
            source: ImageSource::Inline(blob),
            kind: ImageKind::RamLoad,
        }],
        output: OutputConfig::default(),
    }
}

/// The real-mode boot demo: the PIT counter-0 OUT wired to IRQ0, no other source.
pub fn at_boot(blob: Vec<u8>) -> MachineConfig {
    at_demo(blob, at_device_set(Some(0), None, None, None))
}

/// The real-mode keyboard demo: the 8042 keyboard-byte-ready line wired to IRQ1.
///
/// The scancode script is queued into the controller by the frontend after
/// assembly (through the device-index seam), matching the demo's power-on key
/// seeding.
pub fn at_keyboard(blob: Vec<u8>) -> MachineConfig {
    at_demo(blob, at_device_set(None, Some(1), None, None))
}

/// The real-mode RTC demo: the RTC update-ended line wired to IRQ8 (on the slave
/// PIC, cascaded into the master), with the RTC fast-ticked so the update-ended
/// interrupt fires within a bounded run.
pub fn at_rtc(blob: Vec<u8>, second_ticks: u64, uip_ticks: u64) -> MachineConfig {
    at_demo(
        blob,
        at_device_set(None, None, Some(8), Some((second_ticks, uip_ticks))),
    )
}

/// The VGA text demo: `blob` runs with DS/ES pointed at the colour text page, the
/// `0xB8000` buffer trapped as MMIO to the card, and no interrupt source.
pub fn at_vga_text(blob: Vec<u8>) -> MachineConfig {
    let space = TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64;
    MachineConfig {
        cpu: CpuConfig {
            chip: CpuChip::I80286,
            reset: ResetSpec::Entry {
                cs: DEMO_CS,
                ip: DEMO_IP,
            },
            segments: SegmentSeed {
                ds: VGA_TEXT_SEGMENT,
                es: VGA_TEXT_SEGMENT,
                ss: DEMO_CS,
                sp: SegmentSeed::flat(DEMO_CS).sp,
            },
        },
        clock: ClockConfig::default(),
        memory: MemoryConfig {
            address_space_bytes: space,
            ram: vec![RamRegion::new(0, TEXT_BUFFER_BASE)],
            mmio: vec![MmioWindow::new(
                TEXT_BUFFER_BASE,
                TEXT_BUFFER_LEN as u64,
                DeviceId::Vga,
            )],
        },
        devices: at_device_set(None, None, None, None),
        drives: Vec::new(),
        images: vec![LoadImage {
            base: (u64::from(DEMO_CS) << 4) + u64::from(DEMO_IP),
            source: ImageSource::Inline(blob),
            kind: ImageKind::RamLoad,
        }],
        output: OutputConfig::default(),
    }
}

/// The test386 diagnostic machine: the whole 1 MiB is RAM, the 64 KiB `image` is
/// loaded at `0xF0000`, and execution starts at the reset vector `F000:FFF0`.
///
/// Just two devices — the POST checkpoint port at `0x190` and a COM1 UART — and no
/// interrupt controller: the diagnostic runs `cli`, so any wired IRQ could never
/// be delivered and would keep the run from terminating.
pub fn test386(image: Vec<u8>) -> MachineConfig {
    MachineConfig {
        cpu: CpuConfig {
            chip: CpuChip::I80286,
            reset: ResetSpec::Entry {
                cs: RESET_CS,
                ip: RESET_IP,
            },
            segments: SegmentSeed::flat(TEST386_LOAD_SEG),
        },
        clock: ClockConfig {
            retired_per_pit_clock: machine::RETIRED_PER_PIT_CLOCK,
            quantum: Some(65_536),
        },
        memory: MemoryConfig {
            address_space_bytes: MIB,
            ram: vec![RamRegion::new(0, MIB)],
            mmio: Vec::new(),
        },
        devices: vec![post(POST_PORT), uart()],
        drives: Vec::new(),
        images: vec![LoadImage {
            base: u64::from(TEST386_LOAD_SEG) << 4,
            source: ImageSource::Inline(image),
            kind: ImageKind::RamLoad,
        }],
        output: OutputConfig::default(),
    }
}

/// The fast floppy-boot machine: a COM1 UART, an 8237 DMA controller, and a µPD765
/// floppy controller backed by `floppy`, with `loader` loaded at `0x7E00` in flat
/// 64 KiB RAM. The loader drives the controller to read the boot sector over DMA
/// and jumps to it; there is no BIOS and no interrupt controller (the DMA transfer
/// is synchronous, so the loader never waits on IRQ6).
pub fn floppy_boot(loader: Vec<u8>, floppy: Vec<u8>) -> MachineConfig {
    let devices = vec![
        uart(),
        // The DMA page latches here run from 0x80 (no POST port competes for it).
        dma(vec![
            PortWindow::new(0x00, 0x0F),
            PortWindow::new(0x80, 0x8F),
        ]),
        fdc(vec![PortWindow::new(FDC_BASE, FDC_TOP)], None, Some(2)),
    ];
    MachineConfig {
        cpu: CpuConfig {
            chip: CpuChip::I80286,
            reset: ResetSpec::Entry {
                cs: DEMO_CS,
                ip: FLOPPY_LOADER_IP,
            },
            segments: SegmentSeed::flat(DEMO_CS),
        },
        clock: ClockConfig::default(),
        memory: MemoryConfig {
            address_space_bytes: DEMO_RAM_BYTES,
            ram: vec![RamRegion::new(0, DEMO_RAM_BYTES)],
            mmio: Vec::new(),
        },
        devices,
        drives: vec![floppy_drive(ImageSource::Inline(floppy))],
        images: vec![LoadImage {
            base: (u64::from(DEMO_CS) << 4) + u64::from(FLOPPY_LOADER_IP),
            source: ImageSource::Inline(loader),
            kind: ImageKind::RamLoad,
        }],
        output: OutputConfig::default(),
    }
}

/// The full PC/AT BIOS machine: the 1 MiB memory mosaic with the 32 KiB `rom`
/// masked at `0xF8000`, the cascaded 8259 pair, PIT, 8042, VGA text card,
/// CMOS/RTC (seeded), the port-`0x61` latch, and a POST port at `0x80`, booting
/// from the reset vector. `quantum` sets the cooperative run budget (the coarse
/// boot quantum, or the fine interactive one).
///
/// `floppy` chooses the disk story:
///
/// - `Some(image)` attaches a real µPD765 floppy controller (ports `0x3F0..=0x3F7`,
///   IRQ6) driven by an 8237 (`0x00..=0x0F`, page latches `0x81..=0x8F`) backed by
///   `image` — the path INT 13h / INT 19h use to load a boot sector.
/// - `None` models the pre-floppy-controller spike: the BIOS's floppy-probe ports
///   stay undecoded (open-bus `0xFF`), so INT 19h spins in its retry loop. The
///   machine still declares a 1.44 MB drive in CMOS (a real AT always does, and the
///   original spike seeded the same CMOS), so a floppy drive is present for the
///   CMOS derivation but the controller decodes no ports and takes no interrupt.
pub fn bios(rom: Vec<u8>, floppy: Option<Vec<u8>>, quantum: Option<u64>) -> MachineConfig {
    let mut devices = vec![
        uart(),
        master_pic(),
        pit(Some(0)),
        keyboard(Some(1)),
        vga(true),
        slave_pic(),
        rtc(true, None, Some(8)),
        sys_control(),
        post(BIOS_POST_PORT),
    ];

    let drives = match &floppy {
        Some(_) => {
            // A real, decoded floppy controller and its 8237: the boot path.
            devices.push(fdc(
                vec![PortWindow::new(FDC_BASE, FDC_TOP)],
                Some(6),
                Some(2),
            ));
            devices.push(dma(vec![
                PortWindow::new(0x00, 0x0F),
                PortWindow::new(0x81, 0x8F),
            ]));
            vec![floppy_drive(ImageSource::Inline(floppy.unwrap()))]
        }
        None => {
            // No controller decoded (INT 19h reads open-bus and spins), but the AT
            // still declares its drive A: in CMOS. The FDC entry decodes nothing and
            // is unwired; it exists only so the drive binds and the CMOS derivation
            // sees a 1.44 MB floppy, exactly as the original no-floppy spike did.
            devices.push(fdc(Vec::new(), None, None));
            vec![floppy_drive(ImageSource::Blank {
                size: FloppyGeometry::HD_1_44.image_bytes(),
            })]
        }
    };

    MachineConfig {
        cpu: CpuConfig {
            chip: CpuChip::I80286,
            reset: ResetSpec::ResetVector {
                cs: RESET_CS,
                ip: RESET_IP,
            },
            segments: SegmentSeed::flat(RESET_CS),
        },
        clock: ClockConfig {
            retired_per_pit_clock: machine::RETIRED_PER_PIT_CLOCK,
            quantum,
        },
        memory: MemoryConfig {
            address_space_bytes: MIB,
            ram: vec![
                // 640 KiB conventional RAM + the graphics-adapter probe window up to
                // the trapped text page.
                RamRegion::new(0, TEXT_BUFFER_BASE),
                // Zeroed option-ROM scan area between the text page and the ROM.
                RamRegion::new(BIOS_OPTION_ROM_BASE, BIOS_ROM_BASE - BIOS_OPTION_ROM_BASE),
            ],
            mmio: vec![MmioWindow::new(
                TEXT_BUFFER_BASE,
                TEXT_BUFFER_LEN as u64,
                DeviceId::Vga,
            )],
        },
        devices,
        drives,
        images: vec![LoadImage {
            base: BIOS_ROM_BASE,
            source: ImageSource::Inline(rom),
            kind: ImageKind::Rom,
        }],
        output: OutputConfig::default(),
    }
}
