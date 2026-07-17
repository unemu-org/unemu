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
//!
//! ## Cascade
//!
//! A PC/AT has two of these chips: a master (`0x20`/`0x21`) carrying IRQ0–IRQ7
//! and a slave (`0xA0`/`0xA1`) carrying IRQ8–IRQ15, the slave's INT output wired
//! into the master's IRQ2 input. [`CascadedPic`] models that pair as one
//! 16-line [`InterruptController`]. It owns a [`SharedPic`] per chip and exposes
//! each as its own [`Device`] face ([`CascadedPic::master_device`] /
//! [`CascadedPic::slave_device`]) so the chipset can decode both port pairs; the
//! single-chip [`SharedPic`] stays usable on its own for chipsets that model
//! only the master.

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
    /// Wrap a fresh master 8259 (ports `0x20`/`0x21`) in a shared cell.
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Pic8259::new())),
        }
    }

    /// Wrap a fresh slave 8259 (ports `0xA0`/`0xA1`) in a shared cell.
    pub fn new_slave() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Pic8259::new_slave())),
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

/// The master input line the slave's INT output is wired to on every PC/AT.
const CASCADE_IRQ: u8 = 2;

/// A cascaded master + slave 8259 pair presented as one 16-line controller
/// (§5.1, §3.3).
///
/// Owns a master and a slave [`SharedPic`]. The 16 logical lines map as
/// IRQ0–IRQ7 → master lines 0–7 and IRQ8–IRQ15 → slave lines 0–7. Whenever the
/// slave has a serviceable request, its INT output asserts the master's IRQ2
/// input ([`CASCADE_IRQ`]); the master then resolves priority across all
/// sixteen lines, with IRQ8–IRQ15 slotting in at IRQ2's priority.
///
/// Hand [`CascadedPic::master_device`] and [`CascadedPic::slave_device`] to the
/// bus as the two [`Device`] faces (decoded at `0x20`/`0x21` and `0xA0`/`0xA1`)
/// and the `CascadedPic` itself to the machine as its [`InterruptController`].
///
/// ## EOI contract
///
/// A cascaded interrupt occupies two in-service bits at once: the actual line on
/// the slave, and IRQ2 on the master (the cascade). Real software ends it with
/// two OCW2 writes — EOI the slave, then EOI the master. [`end_of_interrupt`]
/// collapses that into a single call for the machine's uniform EOI seam: it
/// looks at the master's top in-service line, and **if that is the cascade line
/// (IRQ2) it EOIs the slave first, then the master**; otherwise it EOIs the
/// master alone. This assumes the cascaded interrupt sits at the top of the
/// master's in-service stack when it is ended — which holds because a cascaded
/// [`acknowledge`] nests the master's IRQ2 and the slave's line together. Two
/// slave interrupts nested on top of each other (special fully-nested mode) are
/// not modelled.
///
/// [`end_of_interrupt`]: InterruptController::end_of_interrupt
/// [`acknowledge`]: InterruptController::acknowledge
#[derive(Clone)]
pub struct CascadedPic {
    master: SharedPic,
    slave: SharedPic,
}

impl CascadedPic {
    /// Wrap a fresh master + slave pair, each unprogrammed.
    pub fn new() -> Self {
        Self {
            master: SharedPic::new(),
            slave: SharedPic::new_slave(),
        }
    }

    /// The master chip's [`Device`] face (decoded at `0x20`/`0x21`).
    ///
    /// A cheap clone sharing the same live chip as this controller, for the bus
    /// device set.
    pub fn master_device(&self) -> SharedPic {
        self.master.clone()
    }

    /// The slave chip's [`Device`] face (decoded at `0xA0`/`0xA1`).
    pub fn slave_device(&self) -> SharedPic {
        self.slave.clone()
    }

    /// Drive the master's IRQ2 (cascade) input from the slave's INT output.
    ///
    /// The slave asserts its INT whenever it has a serviceable request. The
    /// master latches IRQ2 on the rising edge, exactly as edge-triggered wiring
    /// would; when the slave's request moves into service (or is masked away)
    /// the line drops again.
    ///
    /// This runs on every line/acknowledge/EOI operation. Reprogramming the
    /// slave mask goes through its [`Device`] face, which this wrapper does not
    /// observe, so unmasking an already-pending slave line re-establishes the
    /// cascade only on the line's next edge — the normal order (unmask, then the
    /// device asserts) is unaffected.
    fn sync_cascade(&mut self) {
        let slave_int = self.slave.inner.borrow().irq_pending();
        self.master.set_irq_line(CASCADE_IRQ, slave_int);
    }
}

impl Default for CascadedPic {
    fn default() -> Self {
        Self::new()
    }
}

impl InterruptController for CascadedPic {
    fn set_irq_line(&mut self, irq: u8, level: bool) {
        if irq < 8 {
            self.master.set_irq_line(irq, level);
        } else if irq < 16 {
            self.slave.set_irq_line(irq - 8, level);
            // A change on any slave line may raise or drop the cascade.
            self.sync_cascade();
        }
    }

    fn poll(&self) -> Option<u8> {
        // The master resolves priority first. A winning cascade line means the
        // vector comes from the slave's own highest-priority pending line.
        if self.master.inner.borrow().highest_pending() == Some(CASCADE_IRQ) {
            self.slave.poll()
        } else {
            self.master.poll()
        }
    }

    fn acknowledge(&mut self) -> Option<u8> {
        let master_line = self.master.inner.borrow().highest_pending();
        if master_line == Some(CASCADE_IRQ) {
            // Cascaded INTA: the slave supplies the vector and moves its line
            // into service; the master's IRQ2 moves into service in step.
            let vector = self.slave.acknowledge();
            self.master.acknowledge();
            // The slave's INT typically drops now that its line is in service.
            self.sync_cascade();
            vector
        } else {
            self.master.acknowledge()
        }
    }

    fn end_of_interrupt(&mut self) {
        // See the EOI contract on the type: a single call ends the interrupt at
        // the top of the master's in-service stack, and if that is the cascade
        // line it also ends the matching slave interrupt.
        let cascaded = self.master.inner.borrow().highest_in_service() == Some(CASCADE_IRQ);
        if cascaded {
            self.slave.end_of_interrupt();
        }
        self.master.end_of_interrupt();
        // Ending a slave interrupt may re-expose a lower pending slave line.
        self.sync_cascade();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Master ports and the PC BIOS vector base (IRQ0 → 0x08).
    const MASTER_CMD: u16 = 0x20;
    const MASTER_DATA: u16 = 0x21;
    const MASTER_BASE: u8 = 0x08;
    // Slave ports and vector base (IRQ8 → 0x70).
    const SLAVE_CMD: u16 = 0xA0;
    const SLAVE_DATA: u16 = 0xA1;
    const SLAVE_BASE: u8 = 0x70;

    // ICW1 = init (bit4) + ICW4-needed (bit0); ICW4 = 8086 mode (bit0).
    const ICW1: u32 = 0x11;
    const ICW4_8086: u32 = 0x01;

    /// Program a chip's ICW1–ICW4 through its `Device` face at `base` port.
    fn init_chip(dev: &mut SharedPic, cmd: u16, data: u16, vector_base: u8, icw3: u32) {
        dev.port_write(cmd, 1, ICW1).unwrap();
        dev.port_write(data, 1, u32::from(vector_base)).unwrap();
        dev.port_write(data, 1, icw3).unwrap();
        dev.port_write(data, 1, ICW4_8086).unwrap();
    }

    /// A fully initialized cascaded pair (master base 0x08, slave base 0x70).
    fn init_pair() -> CascadedPic {
        let pic = CascadedPic::new();
        let mut master = pic.master_device();
        let mut slave = pic.slave_device();
        // ICW3: master has a slave on IRQ2 (bit 2); slave identity is 2.
        init_chip(&mut master, MASTER_CMD, MASTER_DATA, MASTER_BASE, 0x04);
        init_chip(&mut slave, SLAVE_CMD, SLAVE_DATA, SLAVE_BASE, 0x02);
        pic
    }

    #[test]
    fn cascaded_acknowledge_returns_slave_vector_and_nests_irq2() {
        let mut pic = init_pair();
        // IRQ8 (the RTC line) goes through the slave.
        pic.set_irq_line(8, true);
        // The controller offers the slave's vector, not a master vector.
        assert_eq!(pic.poll(), Some(SLAVE_BASE));
        assert_eq!(pic.acknowledge(), Some(SLAVE_BASE));
        // The slave's line and the master's IRQ2 are both in service.
        assert_eq!(pic.slave.inner.borrow().highest_in_service(), Some(0));
        assert_eq!(
            pic.master.inner.borrow().highest_in_service(),
            Some(CASCADE_IRQ)
        );
        // With IRQ2 in service, nothing lower on either chip is offered.
        assert_eq!(pic.poll(), None);
    }

    #[test]
    fn cascaded_irq15_uses_high_slave_vector() {
        let mut pic = init_pair();
        pic.set_irq_line(15, true);
        // IRQ15 is slave line 7 → vector 0x77.
        assert_eq!(pic.acknowledge(), Some(SLAVE_BASE + 7));
    }

    #[test]
    fn single_end_of_interrupt_clears_both_chips_for_cascade() {
        let mut pic = init_pair();
        pic.set_irq_line(8, true);
        pic.acknowledge().unwrap();
        // One EOI ends the cascaded interrupt on both chips.
        pic.end_of_interrupt();
        assert_eq!(pic.master.inner.borrow().highest_in_service(), None);
        assert_eq!(pic.slave.inner.borrow().highest_in_service(), None);
    }

    #[test]
    fn cascade_outranks_lower_master_lines_but_not_higher() {
        let mut pic = init_pair();
        // A slave interrupt enters at IRQ2's priority: it outranks IRQ7 …
        pic.set_irq_line(7, true);
        pic.set_irq_line(8, true);
        assert_eq!(pic.poll(), Some(SLAVE_BASE));
        assert_eq!(pic.acknowledge(), Some(SLAVE_BASE));
        // … while the lower-priority master IRQ7 stays blocked underneath it.
        assert_eq!(pic.poll(), None);
        pic.end_of_interrupt();
        assert_eq!(pic.poll(), Some(MASTER_BASE + 7));
    }

    #[test]
    fn higher_master_line_outranks_the_cascade() {
        let mut pic = init_pair();
        // IRQ1 is higher priority than the IRQ2 cascade.
        pic.set_irq_line(1, true);
        pic.set_irq_line(8, true);
        assert_eq!(pic.poll(), Some(MASTER_BASE + 1));
        assert_eq!(pic.acknowledge(), Some(MASTER_BASE + 1));
        // Once IRQ1 is done, the cascaded slave interrupt surfaces.
        pic.end_of_interrupt();
        assert_eq!(pic.poll(), Some(SLAVE_BASE));
    }

    #[test]
    fn within_slave_priority_picks_lower_line_first() {
        let mut pic = init_pair();
        // IRQ10 (slave line 2) then IRQ8 (slave line 0); line 0 is higher.
        pic.set_irq_line(10, true);
        pic.set_irq_line(8, true);
        assert_eq!(pic.acknowledge(), Some(SLAVE_BASE));
        // IRQ8 in service blocks the lower slave line until EOI.
        assert_eq!(pic.poll(), None);
        pic.end_of_interrupt();
        assert_eq!(pic.poll(), Some(SLAVE_BASE + 2));
    }

    #[test]
    fn masked_slave_line_does_not_surface() {
        let mut pic = init_pair();
        // Mask IRQ8 (slave line 0) through the slave's data port.
        pic.slave_device().port_write(SLAVE_DATA, 1, 0x01).unwrap();
        pic.set_irq_line(8, true);
        // The slave latched the request but does not assert its INT, so the
        // master's IRQ2 never rises and nothing is offered.
        assert_eq!(pic.poll(), None);
        assert_eq!(pic.acknowledge(), None);
        assert_eq!(pic.master.inner.borrow().highest_pending(), None);
    }

    #[test]
    fn master_lines_are_unaffected_by_the_slave() {
        let mut pic = init_pair();
        // A plain master interrupt behaves exactly as the single-chip PIC does.
        pic.set_irq_line(3, true);
        assert_eq!(pic.poll(), Some(MASTER_BASE + 3));
        assert_eq!(pic.acknowledge(), Some(MASTER_BASE + 3));
        assert_eq!(pic.master.inner.borrow().highest_in_service(), Some(3));
        pic.end_of_interrupt();
        assert_eq!(pic.master.inner.borrow().highest_in_service(), None);
    }
}
