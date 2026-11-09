// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wiring tests for the dual-faced 8259 PIC (§5.1).
//!
//! The PIC is shared between its [`Device`] face (guest programming + EOI over
//! the bus) and its [`InterruptController`] face (the machine's INTA). These
//! tests prove the two faces see one live chip: an EOI written through the device
//! face clears the ISR the controller face's INTA set.

use chipset::{CascadedPic, SharedPic};
use device::{Device, InterruptController};
use testkit::MockBus;

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

/// Program the PC/AT cascade through the guest's port interfaces.
fn init_cascade() -> (CascadedPic, MockBus, MockBus) {
    let pic = CascadedPic::new();
    let mut master = MockBus::new(Box::new(pic.master_device()));
    let mut slave = MockBus::new(Box::new(pic.slave_device()));
    for (bus, command, base, wiring) in [
        (&mut master, 0x20, 0x08, 0x04),
        (&mut slave, 0xA0, 0x70, 0x02),
    ] {
        bus.port_write(command, 1, 0x11).unwrap();
        bus.port_write(command + 1, 1, base).unwrap();
        bus.port_write(command + 1, 1, wiring).unwrap();
        bus.port_write(command + 1, 1, 0x01).unwrap();
    }
    (pic, master, slave)
}

/// Read IRR or ISR with OCW3 through the guest's command port.
fn read_register(bus: &mut MockBus, command: u16, select: u32) -> u32 {
    bus.port_write(command, 1, select).unwrap();
    bus.port_read(command, 1).unwrap()
}

#[test]
fn masking_slave_withdraws_cascade_and_serves_pending_master_irq() {
    let (mut pic, mut master, mut slave) = init_cascade();
    pic.set_irq_line(8, true);
    pic.set_irq_line(6, true);
    pic.set_irq_line(8, false);
    pic.set_irq_line(6, false);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0x44);

    slave.port_write(0xA1, 1, 0x01).unwrap();
    assert_eq!(pic.poll(), Some(0x0E));
    assert_eq!(pic.acknowledge(), Some(0x0E));
    assert_eq!(read_register(&mut slave, 0xA0, 0x0B), 0);
    assert_eq!(read_register(&mut slave, 0xA0, 0x0A), 1);
    assert_eq!(read_register(&mut master, 0x20, 0x0B), 0x40);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0);
    assert_eq!(pic.acknowledge(), None);

    master.port_write(0x20, 1, 0x20).unwrap();
    slave.port_write(0xA1, 1, 0x00).unwrap();
    assert_eq!(pic.acknowledge(), Some(0x70));
}

#[test]
fn slave_port_eoi_delivers_pending_request_without_new_line_edge() {
    let (mut pic, mut master, mut slave) = init_cascade();
    pic.set_irq_line(8, true);
    assert_eq!(pic.acknowledge(), Some(0x70));
    pic.set_irq_line(14, true);
    assert_eq!(pic.acknowledge(), None);

    slave.port_write(0xA0, 1, 0x20).unwrap();
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0x04);
    assert_eq!(pic.acknowledge(), None);
    master.port_write(0x20, 1, 0x20).unwrap();
    assert_eq!(pic.acknowledge(), Some(0x76));
    assert_eq!(read_register(&mut slave, 0xA0, 0x0B), 0x40);

    slave.port_write(0xA0, 1, 0x20).unwrap();
    master.port_write(0x20, 1, 0x20).unwrap();
    assert_eq!(pic.acknowledge(), None);
}

#[test]
fn unmasking_pending_slave_request_raises_cascade_without_line_edge() {
    let (mut pic, mut master, mut slave) = init_cascade();
    slave.port_write(0xA1, 1, 0x40).unwrap();
    pic.set_irq_line(14, true);
    assert_eq!(pic.acknowledge(), None);

    slave.port_write(0xA1, 1, 0x00).unwrap();
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0x04);
    assert_eq!(pic.acknowledge(), Some(0x76));
}

#[test]
fn reinitializing_slave_withdraws_latched_cascade_request() {
    let (mut pic, mut master, mut slave) = init_cascade();
    pic.set_irq_line(8, true);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0x04);
    slave.port_write(0xA0, 1, 0x11).unwrap();
    slave.port_write(0xA1, 1, 0x78).unwrap();
    slave.port_write(0xA1, 1, 0x02).unwrap();
    slave.port_write(0xA1, 1, 0x01).unwrap();

    assert_eq!(pic.poll(), None);
    assert_eq!(pic.acknowledge(), None);
    assert_eq!(read_register(&mut slave, 0xA0, 0x0B), 0);
    assert_eq!(read_register(&mut master, 0x20, 0x0B), 0);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0);
}
