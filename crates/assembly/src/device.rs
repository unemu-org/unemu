// SPDX-License-Identifier: AGPL-3.0-or-later
//! Device configuration (§7).
//!
//! One [`DeviceConfig`] describes one modelled chip: its stable role
//! ([`DeviceId`]), the chip-specific parameters ([`DeviceKind`]), the port
//! windows the bus decodes to it, and its IRQ / DMA wiring — all as data. The
//! assembler resolves each [`DeviceId`] to a [`bus`] device index and turns the
//! port windows into a [`bus::DecodeMap`]; nothing here holds a bus index.

/// A stable identifier for a modelled device role (§7).
///
/// The config names devices by *role*, not by chip type or bus index: a caller
/// asks for [`DeviceId::Uart`] and the assembler hands back which bus slot it
/// landed in. Each role appears at most once in a machine (duplication is a
/// [`crate::ConfigError`]), which is why this is a plain enum rather than a
/// string — it is exhaustive over the roles this machine models and is a natural
/// map key ([`Hash`]/[`Eq`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeviceId {
    /// 16550 serial UART (COM1).
    Uart,
    /// 8253 programmable interval timer.
    Pit,
    /// Master 8259 interrupt controller.
    MasterPic,
    /// Slave 8259 interrupt controller (cascaded into the master).
    SlavePic,
    /// 8042 keyboard controller.
    Keyboard,
    /// MC146818 CMOS RAM / real-time clock.
    Rtc,
    /// VGA/MDA text-mode card.
    Vga,
    /// System control port (0x61: speaker gate, refresh, NMI status).
    SysControl,
    /// POST checkpoint port.
    Post,
    /// uPD765 floppy-disk controller.
    Fdc,
    /// 8237 DMA controller.
    Dma,
}

/// A contiguous, inclusive port range decoded to a device (§4.1).
///
/// The config counterpart of [`bus::PortRange`], without the bus index the
/// assembler fills in. A device may decode several windows (a VGA card decodes
/// its CRTC, CGA, and MDA aliases); [`PortWindow::allow_overlap`] marks a window
/// that is *intended* to overlap an earlier device's — the bus resolves such an
/// overlap by first-match order, and validation treats an unmarked overlap as an
/// error.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PortWindow {
    /// First port in the range (inclusive).
    pub start: u16,
    /// Last port in the range (inclusive).
    pub end: u16,
    /// Whether this window may overlap an earlier device's decode. Deliberate
    /// overlaps (e.g. a page latch sharing a byte with a checkpoint port) are
    /// resolved by first-match order and must be flagged here.
    pub allow_overlap: bool,
}

impl PortWindow {
    /// A non-overlapping window `[start, end]`.
    pub const fn new(start: u16, end: u16) -> Self {
        Self {
            start,
            end,
            allow_overlap: false,
        }
    }

    /// A single-port window at `port`.
    pub const fn single(port: u16) -> Self {
        Self::new(port, port)
    }

    /// A window that may intentionally overlap an earlier device's decode.
    pub const fn overlapping(start: u16, end: u16) -> Self {
        Self {
            start,
            end,
            allow_overlap: true,
        }
    }

    /// Whether this window and `other` share any port.
    pub fn overlaps(&self, other: &PortWindow) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// One device's DMA wiring: the controller and channel it transfers through
/// (§7.7).
///
/// A bus-master device (the floppy controller) cannot reach guest memory itself;
/// it queues a transfer that a DMA controller runs. This names the controller by
/// role and the channel it uses; the assembler turns it into a
/// [`machine::DmaWiring`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DmaSpec {
    /// The DMA controller this device masters the bus through.
    pub controller: DeviceId,
    /// The DMA channel the transfer uses.
    pub channel: u8,
}

/// 16550 UART parameters (§7.2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UartCfg {
    /// Base port (the eight UART registers decode from here). COM1 is `0x3F8`.
    pub base: u16,
}

/// 8253 PIT parameters (§7.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PitCfg {
    /// Base port (counters 0-2 and the control word decode from here: `0x40`).
    pub base: u16,
}

/// 8259 PIC parameters (§7.3).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PicCfg {
    /// Command port (the data port is `command + 1`). Master `0x20`, slave
    /// `0xA0`.
    pub command: u16,
    /// Whether this PIC is cascaded (the master half of a master/slave pair).
    pub cascade: bool,
}

/// 8042 keyboard-controller parameters (§7.5).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct I8042Cfg {
    /// Data port (`0x60`).
    pub data: u16,
    /// Command/status port (`0x64`).
    pub command: u16,
}

/// MC146818 CMOS/RTC parameters (§7.6).
///
/// The NVRAM configuration bytes a stock BIOS checksums (floppy types, base and
/// extended memory size, the equipment byte) are **derived** from the rest of
/// the [`crate::MachineConfig`] — memory size from [`crate::MemoryConfig`],
/// floppy types from the [`crate::DriveConfig`] list — rather than duplicated
/// here, so the two can never disagree. This carries only the choice of whether
/// to seed that area at all; a blank NVRAM (the power-on state of a
/// battery-dead chip) drops a stock BIOS into its SETUP prompt. See
/// [`devices::CmosConfig`], the seed the assembler builds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CmosCfg {
    /// Index/NMI-control port (`0x70`).
    pub index: u16,
    /// Data port (`0x71`).
    pub data: u16,
    /// Whether to seed the AT configuration NVRAM (and its checksum) so a stock
    /// BIOS's POST passes unattended.
    pub seed: bool,
    /// Override the RTC's virtual-clock timing as `(second_ticks, uip_ticks)`, or
    /// `None` for the real ~1.193 MHz-derived default.
    ///
    /// The real per-second ratio is over a million virtual clocks — far more than
    /// a bounded headless run advances — so a machine that must observe an
    /// update-ended interrupt (IRQ8) within a short run sets a small ratio here.
    /// The timing is still measured in virtual clocks, never wall time, so the run
    /// stays deterministic.
    pub timing: Option<(u64, u64)>,
}

/// VGA/MDA text-card parameters (§9.2).
///
/// The card's port bases only; the text buffer it exposes is an MMIO window and
/// lives in [`crate::MemoryConfig`] (single home), not here.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VgaTextCfg {
    /// CRTC index port (the data port is `crtc_index + 1`: `0x3D4`/`0x3D5`).
    pub crtc_index: u16,
}

/// System-control-port parameters (§7).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SysControlCfg {
    /// The control port (`0x61`).
    pub port: u16,
}

/// POST-checkpoint-port parameters (§7).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PostPortCfg {
    /// The checkpoint port the guest writes progress bytes to.
    pub port: u16,
}

/// uPD765 floppy-controller parameters (§7.7).
///
/// Drives are not listed here — they live once in the top-level
/// [`crate::MachineConfig::drives`] and bind to this controller by
/// `controller_slot`. This declares only how many drive slots the controller
/// offers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FdcCfg {
    /// Base port (`0x3F0`).
    pub base: u16,
    /// Number of drive slots this controller offers (a standard FDC drives two,
    /// units 0 and 1).
    pub drive_slots: u8,
}

/// 8237 DMA-controller parameters (§7.7).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DmaCfg {
    /// Base port of the channel registers (`0x00`).
    pub base: u16,
}

/// The chip-specific parameters of a [`DeviceConfig`] (§7).
///
/// One variant per modelled chip. The variant fixes *which* chip a device is;
/// the surrounding [`DeviceConfig`] carries the wiring (ports, IRQ, DMA) common
/// to all of them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    /// 16550 UART.
    Uart(UartCfg),
    /// 8253 PIT.
    Pit(PitCfg),
    /// 8259 PIC.
    Pic(PicCfg),
    /// 8042 keyboard controller.
    Keyboard(I8042Cfg),
    /// MC146818 CMOS/RTC.
    Rtc(CmosCfg),
    /// VGA/MDA text card.
    Vga(VgaTextCfg),
    /// System control port.
    SysControl(SysControlCfg),
    /// POST checkpoint port.
    Post(PostPortCfg),
    /// uPD765 floppy controller.
    Fdc(FdcCfg),
    /// 8237 DMA controller.
    Dma(DmaCfg),
}

/// One device in a [`crate::MachineConfig`] (§7).
///
/// The `id` is the role callers and the assembler address the device by; `kind`
/// carries its chip parameters; `ports` are the windows the bus decodes to it;
/// `irq` is the PIC input line it drives (if any); `dma` is its DMA wiring (if
/// it masters the bus). Marking `allow_port_overlap` permits this device's
/// windows to overlap an earlier device's — the deliberate, first-match-resolved
/// kind of overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceConfig {
    /// The device's stable role.
    pub id: DeviceId,
    /// The chip and its parameters.
    pub kind: DeviceKind,
    /// The port windows the bus decodes to this device.
    pub ports: Vec<PortWindow>,
    /// The PIC input line this device drives, if it raises interrupts.
    pub irq: Option<u8>,
    /// This device's DMA wiring, if it masters the bus.
    pub dma: Option<DmaSpec>,
    /// Whether this device's port windows may overlap an earlier device's.
    pub allow_port_overlap: bool,
}

impl DeviceConfig {
    /// A device with the given role, kind, and decoded ports, no IRQ or DMA
    /// wiring and no overlap allowance.
    pub fn new(id: DeviceId, kind: DeviceKind, ports: Vec<PortWindow>) -> Self {
        Self {
            id,
            kind,
            ports,
            irq: None,
            dma: None,
            allow_port_overlap: false,
        }
    }

    /// Set the PIC input line this device drives.
    #[must_use]
    pub fn with_irq(mut self, irq: u8) -> Self {
        self.irq = Some(irq);
        self
    }

    /// Set this device's DMA wiring.
    #[must_use]
    pub fn with_dma(mut self, dma: DmaSpec) -> Self {
        self.dma = Some(dma);
        self
    }

    /// Allow this device's port windows to overlap an earlier device's.
    #[must_use]
    pub fn allowing_port_overlap(mut self) -> Self {
        self.allow_port_overlap = true;
        self
    }
}
