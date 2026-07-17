// SPDX-License-Identifier: AGPL-3.0-or-later
//! Concrete emulated chips (§7).
//!
//! Each chip implements the [`device::Device`] swap seam (§7.1) so the
//! `machine` core and the `bus` never name a concrete device. The first device
//! is the [`uart16550::Uart16550`] serial controller (§7.2).

pub mod uart16550;

pub use uart16550::Uart16550;
