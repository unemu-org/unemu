// SPDX-License-Identifier: AGPL-3.0-or-later
//! A single 8259 PIC shared between its two faces (§5.1, §3.3).
//!
//! The 8259 sits on two seams at once: the guest programs it and issues EOI
//! through its **[`Device`] face** (ports `0x20`/`0x21`, decoded by the `bus`),
//! while the machine drives INTA and reads the aggregated INTR through its
//! **[`InterruptController`] face**. Both faces must see the *same* register
//! state — an EOI the guest writes through the bus has to clear the ISR the
//! machine's INTA set.
//!
//! One live PIC therefore backs both faces, shared behind `Rc<RefCell<…>>`.
//! [`SharedPic`] is a cheap handle over that cell: cloned once into the bus's
//! device set (as a [`Device`]) and once into the machine (as an
//! [`InterruptController`]). The machine core still never names `Pic8259` — it
//! sees only the [`InterruptController`] trait object.

use std::cell::RefCell;
use std::rc::Rc;

use device::{Device, DeviceError, InterruptController, TraceEvent, VClock};
use devices::Pic8259;

/// A shared handle to one live 8259 PIC (§5.1).
///
/// Clone it to hand the same PIC to the bus (its [`Device`] face) and to the
/// machine (its [`InterruptController`] face); both clones borrow the one
/// [`Pic8259`] through the shared cell, so register state stays consistent.
#[derive(Clone)]
pub struct SharedPic {
    inner: Rc<RefCell<Pic8259>>,
}

impl SharedPic {
    /// Wrap a fresh master 8259 in a shared cell.
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Pic8259::new())),
        }
    }
}

impl Default for SharedPic {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for SharedPic {
    fn name(&self) -> &'static str {
        // A `&'static str` can't be borrowed out of the cell, so return the
        // stable name directly (it matches `Pic8259::name`).
        "8259a-pic"
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.inner.borrow_mut().port_read(port, size)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.inner.borrow_mut().port_write(port, size, value)
    }

    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        self.inner.borrow_mut().mmio_read(addr, size)
    }

    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.inner.borrow_mut().mmio_write(addr, size, value)
    }

    fn tick(&mut self, now: VClock) {
        self.inner.borrow_mut().tick(now);
    }

    fn irq_pending(&self) -> bool {
        self.inner.borrow().irq_pending()
    }

    fn irq_vector(&self) -> Option<u8> {
        self.inner.borrow().irq_vector()
    }

    fn reset(&mut self) {
        self.inner.borrow_mut().reset();
    }

    fn save(&self) -> Vec<u8> {
        self.inner.borrow().save()
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        self.inner.borrow_mut().restore(snapshot)
    }

    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.inner.borrow_mut().take_trace()
    }
}

impl InterruptController for SharedPic {
    fn set_irq_line(&mut self, irq: u8, level: bool) {
        self.inner.borrow_mut().set_irq_line(irq, level);
    }

    fn poll(&self) -> Option<u8> {
        self.inner.borrow().poll()
    }

    fn acknowledge(&mut self) -> Option<u8> {
        self.inner.borrow_mut().acknowledge()
    }

    fn end_of_interrupt(&mut self) {
        self.inner.borrow_mut().end_of_interrupt();
    }
}
