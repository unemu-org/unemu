// SPDX-License-Identifier: AGPL-3.0-or-later
//! Concrete emulated chips (§7).
//!
//! Each chip implements the [`device::Device`] swap seam (§7.1) so the
//! `machine` core and the `bus` never name a concrete device. The serial path
//! uses the [`uart16550::Uart16550`] controller (§7.2); the interrupt and timer
//! path uses the [`pic8259::Pic8259`] controller (§7.3) and the
//! [`pit8253::Pit8253`] timer (§7.4).

pub mod pic8259;
pub mod pit8253;
pub mod uart16550;

pub use pic8259::Pic8259;
pub use pit8253::Pit8253;
pub use uart16550::Uart16550;
