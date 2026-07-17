// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wiring tests for the dual-faced 8259 PIC (§5.1).
//!
//! The PIC is shared between its [`Device`] face (guest programming + EOI over
//! the bus) and its [`InterruptController`] face (the machine's INTA). These
//! tests prove the two faces see one live chip: an EOI written through the device
//! face clears the ISR the controller face's INTA set.

use chipset::SharedPic;
use device::{Device, InterruptController};

/// Run the ICW1–ICW4 init a PC BIOS issues (vector base 0x08), then unmask IRQ0.
fn init_master(dev: &mut SharedPic) {
    dev.port_write(0x20, 1, 0x11).unwrap(); // ICW1: init + ICW4
    dev.port_write(0x21, 1, 0x08).unwrap(); // ICW2: vector base 0x08
    dev.port_write(0x21, 1, 0x04).unwrap(); // ICW3: cascade (ignored)
    dev.port_write(0x21, 1, 0x01).unwrap(); // ICW4: 8086 mode
    dev.port_write(0x21, 1, 0xFE).unwrap(); // OCW1: unmask only IRQ0
}

#[test]
fn eoi_via_port_write_clears_isr_seen_by_controller() {
    // Two handles onto one chip: the bus's device face and the machine's
    // controller face.
    let mut ic = SharedPic::new();
    let mut dev = ic.clone();
    init_master(&mut dev);

    // The controller face raises IRQ0 and runs INTA (IRR -> ISR).
    ic.set_irq_line(0, true);
    ic.set_irq_line(0, false);
    assert_eq!(ic.poll(), Some(0x08));
    assert_eq!(ic.acknowledge(), Some(0x08));
    // While IRQ0 is in service, nothing further is serviceable.
    assert_eq!(ic.poll(), None);

    // The guest's EOI arrives through the *device* face (a bus port write).
    dev.port_write(0x20, 1, 0x20).unwrap();

    // The controller face now sees the ISR cleared: a fresh edge is serviceable.
    ic.set_irq_line(0, true);
    ic.set_irq_line(0, false);
    assert_eq!(
        ic.poll(),
        Some(0x08),
        "EOI through the device face must clear the ISR the controller INTA set"
    );
}

#[test]
fn pulse_latches_one_request_per_edge() {
    // The machine drives IRQ0 as a high->low pulse per timer period. Each rising
    // edge must latch exactly one request (edge-triggered), independent of how
    // many times the line is re-pulsed after an unacknowledged one is pending.
    let mut ic = SharedPic::new();
    let mut dev = ic.clone();
    init_master(&mut dev);

    ic.set_irq_line(0, true);
    ic.set_irq_line(0, false);
    assert_eq!(ic.acknowledge(), Some(0x08));
    dev.port_write(0x20, 1, 0x20).unwrap(); // EOI

    // A second pulse latches a second request only because the input returned
    // low between the two rising edges.
    ic.set_irq_line(0, true);
    ic.set_irq_line(0, false);
    assert_eq!(ic.acknowledge(), Some(0x08));
}
