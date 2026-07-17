// SPDX-License-Identifier: AGPL-3.0-or-later
//! Chipset swap seam (§3.2 seam 2, §3.3).
//!
//! A [`Chipset`] is the one place that **declares** the machine's port/MMIO
//! decode map, the device set behind it, and the interrupt controller (§3.3,
//! §5.1). The `bus` executes what the chipset declares; nothing else owns decode
//! policy, and the `machine` core reaches interrupts only through the
//! [`InterruptController`] trait object the chipset produces.
//!
//! Two concrete chipsets:
//! - [`SerialChipset`]: a single 16550 UART at COM1 and no interrupt controller
//!   — the Long64 serial walking skeleton (§7.2).
//! - [`AtChipset`]: a PC/AT-style set — UART at COM1, 8259 PIC at `0x20`/`0x21`,
//!   8253 PIT at `0x40`–`0x43` — with the PIC wired as the interrupt controller,
//!   enough to boot a real-mode guest that takes a timer interrupt (§5, §7).

mod shared_pic;

pub use shared_pic::SharedPic;

use bus::{DecodeMap, PortRange};
use device::Device;
use devices::{Pit8253, Uart16550};

// The interrupt-controller seam lives next to the `Device` trait; the chipset
// only needs to name it and provide a default no-op implementation.
pub use device::InterruptController;

/// COM1 base port on the IBM PC/AT (§7.2).
pub const COM1_BASE: u16 = 0x3F8;
/// COM1 top port (base + 7; the 16550 decodes eight registers).
pub const COM1_TOP: u16 = 0x3FF;

/// Master 8259 PIC command port (§7.3).
pub const PIC_COMMAND: u16 = 0x20;
/// Master 8259 PIC data port.
pub const PIC_DATA: u16 = 0x21;

/// 8253 PIT base port (counter 0 data) (§7.4).
pub const PIT_BASE: u16 = 0x40;
/// 8253 PIT top port (mode/command register).
pub const PIT_TOP: u16 = 0x43;

/// A no-op interrupt controller: no lines, never acknowledges (§5.1).
///
/// The controller for a chipset that models no PIC (the serial skeleton): every
/// method is inert so no interrupt is ever taken. [`AtChipset`] uses the real
/// [`SharedPic`] instead.
#[derive(Default)]
pub struct NoInterruptController;

impl InterruptController for NoInterruptController {
    fn set_irq_line(&mut self, _irq: u8, _level: bool) {}

    fn poll(&self) -> Option<u8> {
        None
    }

    fn acknowledge(&mut self) -> Option<u8> {
        None
    }

    fn end_of_interrupt(&mut self) {}
}

/// The chipset swap seam (§3.2 seam 2, §3.3).
///
/// A chipset produces the device set, the matching decode map, and the interrupt
/// controller wired to the device IRQ lines. The machine assembles a `bus` from
/// the device set + map and reaches interrupts through the controller — never
/// naming a concrete chip.
pub trait Chipset {
    /// Consume the chipset and produce the ordered device set, the decode map
    /// whose indices reference it, and the interrupt controller (§3.3, §5.1).
    ///
    /// When the chipset models a PIC, the controller shares state with the PIC's
    /// [`Device`] entry in the returned set, so a guest EOI (a port write through
    /// the bus) is seen by the machine's INTA path.
    fn build(
        self,
    ) -> (
        Vec<Box<dyn Device>>,
        DecodeMap,
        Box<dyn InterruptController>,
    );
}

/// Serial-only chipset: one 16550 UART at COM1, no interrupts (§7.2).
#[derive(Default)]
pub struct SerialChipset;

impl SerialChipset {
    /// Create the serial chipset.
    pub fn new() -> Self {
        Self
    }
}

impl Chipset for SerialChipset {
    fn build(
        self,
    ) -> (
        Vec<Box<dyn Device>>,
        DecodeMap,
        Box<dyn InterruptController>,
    ) {
        let uart = Uart16550::new(COM1_BASE);
        let devices: Vec<Box<dyn Device>> = vec![Box::new(uart)];
        let map = DecodeMap {
            ports: vec![PortRange {
                ports: COM1_BASE..=COM1_TOP,
                device: 0,
            }],
            mmio: Vec::new(),
        };
        (devices, map, Box::new(NoInterruptController))
    }
}

/// Device index of the UART in the [`AtChipset`] device set (COM1 draining).
pub const AT_UART_INDEX: usize = 0;

/// PC/AT-style chipset: UART + 8259 PIC + 8253 PIT, PIC as the controller (§7).
///
/// The device set is `[UART, PIC, PIT]`. The PIC is a [`SharedPic`]: the same
/// live chip backs both its bus [`Device`] entry (guest programming + EOI) and
/// the [`InterruptController`] the machine drives. The PIT's counter-0 OUT is the
/// IRQ0 source; the machine's wiring edge-detects it onto the PIC's IRQ0 line
/// (§5.1) — the chipset only declares the parts and the map.
pub struct AtChipset {
    pic: SharedPic,
}

impl AtChipset {
    /// Create the PC/AT chipset with a fresh, unprogrammed PIC and PIT.
    pub fn new() -> Self {
        Self {
            pic: SharedPic::new(),
        }
    }
}

impl Default for AtChipset {
    fn default() -> Self {
        Self::new()
    }
}

impl Chipset for AtChipset {
    fn build(
        self,
    ) -> (
        Vec<Box<dyn Device>>,
        DecodeMap,
        Box<dyn InterruptController>,
    ) {
        let uart = Uart16550::new(COM1_BASE);
        let pit = Pit8253::new();
        // The PIC handle is cloned: one clone drives the bus device face, the
        // other becomes the machine's interrupt controller. Both share one chip.
        let pic_device = self.pic.clone();
        let controller: Box<dyn InterruptController> = Box::new(self.pic);

        let devices: Vec<Box<dyn Device>> = vec![
            Box::new(uart),       // index 0 (AT_UART_INDEX)
            Box::new(pic_device), // index 1
            Box::new(pit),        // index 2
        ];
        let map = DecodeMap {
            ports: vec![
                PortRange {
                    ports: COM1_BASE..=COM1_TOP,
                    device: 0,
                },
                PortRange {
                    ports: PIC_COMMAND..=PIC_DATA,
                    device: 1,
                },
                PortRange {
                    ports: PIT_BASE..=PIT_TOP,
                    device: 2,
                },
            ],
            mmio: Vec::new(),
        };
        (devices, map, controller)
    }
}
