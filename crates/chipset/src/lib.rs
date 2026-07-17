// SPDX-License-Identifier: AGPL-3.0-or-later
//! Chipset swap seam (§3.2 seam 2, §3.3).
//!
//! A [`Chipset`] is the one place that **declares** the machine's port/MMIO
//! decode map and the device set behind it (§3.3). The `bus` executes what the
//! chipset declares; nothing else owns decode policy. [`Chipset`] also owns the
//! interrupt controller, currently an [`InterruptController`] seam stub to be
//! replaced by the 8259 PIC later (§5).
//!
//! [`SerialChipset`] is the concrete chipset used so far: a single 16550 UART at
//! COM1 (ports `0x3F8..=0x3FF`), the minimum to drive the serial golden test
//! (§7.2).

use bus::{DecodeMap, PortRange};
use device::Device;
use devices::Uart16550;

/// COM1 base port on the IBM PC/AT (§7.2).
pub const COM1_BASE: u16 = 0x3F8;
/// COM1 top port (base + 7; the 16550 decodes eight registers).
pub const COM1_TOP: u16 = 0x3FF;

/// Interrupt-controller seam (§5.1), currently a stub.
///
/// In a real machine this is the 8259 PIC: it aggregates device IRQ lines into a
/// single INTR and answers INTA with a vector (§5.4). No PIC is modelled yet and
/// no interrupts are taken, so the trait is declared to fix the seam in place,
/// with the no-op [`NoInterruptController`] as its only implementation. A real
/// PIC replaces it later.
pub trait InterruptController {
    /// The vector to dispatch on INTA, or `None` when no line is asserted.
    fn ack(&mut self) -> Option<u8>;
}

/// A no-op interrupt controller: never asserts, never acknowledges (§5.1).
#[derive(Default)]
pub struct NoInterruptController;

impl InterruptController for NoInterruptController {
    fn ack(&mut self) -> Option<u8> {
        None
    }
}

/// The chipset swap seam (§3.2 seam 2, §3.3).
///
/// A chipset produces the device set and the matching decode map, and owns the
/// interrupt controller. The machine assembles a `bus` from
/// [`Chipset::build`]'s output.
pub trait Chipset {
    /// Consume the chipset and produce the ordered device set together with the
    /// decode map whose indices reference it (§3.3).
    fn build(self) -> (Vec<Box<dyn Device>>, DecodeMap);

    /// The interrupt controller for this chipset (§5.1); a no-op by default.
    fn interrupt_controller(&self) -> Box<dyn InterruptController> {
        Box::new(NoInterruptController)
    }
}

/// Serial-only chipset: one 16550 UART at COM1 (§7.2).
#[derive(Default)]
pub struct SerialChipset;

impl SerialChipset {
    /// Create the serial chipset.
    pub fn new() -> Self {
        Self
    }
}

impl Chipset for SerialChipset {
    fn build(self) -> (Vec<Box<dyn Device>>, DecodeMap) {
        let uart = Uart16550::new(COM1_BASE);
        let devices: Vec<Box<dyn Device>> = vec![Box::new(uart)];
        let map = DecodeMap {
            ports: vec![PortRange {
                ports: COM1_BASE..=COM1_TOP,
                device: 0,
            }],
            mmio: Vec::new(),
        };
        (devices, map)
    }
}
