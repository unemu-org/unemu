// SPDX-License-Identifier: AGPL-3.0-or-later
//! 8259 PIC composition + the null interrupt controller (§5.1, §3.3).
//!
//! What a machine *is* — its device set, decode map, and wiring — is now plain
//! data assembled by the `assembly` crate (§3.3); this crate no longer declares
//! any machine. What remains here is the interrupt-controller building block that
//! composition needs and the machine core reaches only through the
//! [`InterruptController`] trait object:
//!
//! - [`SharedPic`] / [`CascadedPic`] — one 8259, and the cascaded master/slave
//!   pair, each presenting a [`Device`](device::Device) face (guest programming +
//!   EOI) and an [`InterruptController`] face (INTA) over the *same* register
//!   state.
//! - [`NoInterruptController`] — a null controller for a machine that models no
//!   PIC (the Long64 serial skeleton), so no interrupt is ever taken.
//!
//! The port-address constants for the parts an AT machine decodes live here too,
//! shared by the assembler's presets.

mod shared_pic;

pub use shared_pic::{CascadedPic, SharedPic};

// The interrupt-controller seam lives next to the `Device` trait; this crate only
// needs to name it and provide a default no-op implementation.
pub use device::InterruptController;

/// COM1 base port on the IBM PC/AT (§7.2).
pub const COM1_BASE: u16 = 0x3F8;
/// COM1 top port (base + 7; the 16550 decodes eight registers).
pub const COM1_TOP: u16 = 0x3FF;

/// Master 8259 PIC command port (§7.3).
pub const PIC_COMMAND: u16 = 0x20;
/// Master 8259 PIC data port.
pub const PIC_DATA: u16 = 0x21;

/// Slave 8259 PIC command port; carries IRQ8–IRQ15, cascaded onto master IRQ2
/// (§7.3).
pub const SLAVE_PIC_COMMAND: u16 = 0xA0;
/// Slave 8259 PIC data port.
pub const SLAVE_PIC_DATA: u16 = 0xA1;

/// 8253 PIT base port (counter 0 data) (§7.4).
pub const PIT_BASE: u16 = 0x40;
/// 8253 PIT top port (mode/command register).
pub const PIT_TOP: u16 = 0x43;

/// A no-op interrupt controller: no lines, never acknowledges (§5.1).
///
/// The controller for a machine that models no PIC (the serial skeleton): every
/// method is inert so no interrupt is ever taken. A machine with a PIC uses the
/// real [`SharedPic`] / [`CascadedPic`] instead.
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
