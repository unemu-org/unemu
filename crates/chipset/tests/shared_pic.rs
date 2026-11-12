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
    assert_eq!(ic.poll(), Some(0x08));
    assert_eq!(ic.acknowledge(), Some(0x08));
    ic.set_irq_line(0, false);
    // While IRQ0 is in service, nothing further is serviceable.
    assert_eq!(ic.poll(), None);

    // The guest's EOI arrives through the *device* face (a bus port write).
    dev.port_write(0x20, 1, 0x20).unwrap();

    // The controller face now sees the ISR cleared: a fresh edge is serviceable.
    ic.set_irq_line(0, true);
    assert_eq!(
        ic.poll(),
        Some(0x08),
        "EOI through the device face must clear the ISR the controller INTA set"
    );
}

#[test]
fn held_line_latches_one_request_per_edge() {
    // Each request holds its input high until INTA. A fresh request needs
    // another low-to-high transition after acknowledgement.
    let mut ic = SharedPic::new();
    let mut dev = ic.clone();
    init_master(&mut dev);

    ic.set_irq_line(0, true);
    assert_eq!(ic.acknowledge(), Some(0x08));
    ic.set_irq_line(0, false);
    dev.port_write(0x20, 1, 0x20).unwrap(); // EOI

    // A second edge latches a second request only because the input returned
    // low between the two rising edges.
    ic.set_irq_line(0, true);
    assert_eq!(ic.acknowledge(), Some(0x08));
    ic.set_irq_line(0, false);
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

/// Intel 8259A datasheet, Edge and Level Triggered Modes: masking does not
/// replace the requirement to hold IR high until INTA.
#[test]
fn irq_level_masked_request_is_withdrawn_before_unmask() {
    let (mut pic, mut master, _) = init_cascade();
    master.port_write(0x21, 1, 2).unwrap();
    pic.set_irq_line(1, true);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 2);
    assert_eq!(pic.poll(), None);
    pic.set_irq_line(1, false);
    master.port_write(0x21, 1, 0).unwrap();
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0);
    assert_eq!(pic.poll(), None);
}

#[test]
fn irq_level_unmasking_held_request_services_exactly_one_edge() {
    let (mut pic, mut master, _) = init_cascade();
    master.port_write(0x21, 1, 2).unwrap();
    pic.set_irq_line(1, true);
    master.port_write(0x21, 1, 0).unwrap();
    assert_eq!(pic.acknowledge(), Some(9));
    master.port_write(0x20, 1, 0x20).unwrap();
    pic.set_irq_line(1, true);
    assert_eq!(pic.poll(), None);
    pic.set_irq_line(1, false);
    pic.set_irq_line(1, true);
    pic.set_irq_line(1, false);
    assert_eq!(pic.poll(), None);
}

#[test]
fn irq_level_fall_between_poll_and_inta_returns_default_ir7() {
    for slave in [false, true] {
        let mut pic = if slave {
            SharedPic::new_slave()
        } else {
            SharedPic::new()
        };
        let command = if slave { 0xA0 } else { 0x20 };
        let base = if slave { 0x70 } else { 8 };
        let mut bus = MockBus::new(Box::new(pic.clone()));
        for (port, value) in [(command, 0x13), (command + 1, base), (command + 1, 1)] {
            bus.port_write(port, 1, value).unwrap();
        }
        pic.set_irq_line(1, true);
        assert_eq!(pic.poll(), Some(base as u8 + 1));
        pic.set_irq_line(1, false);
        assert_eq!(pic.acknowledge(), Some(base as u8 + 7));
        assert_eq!(read_register(&mut bus, command, 0x0B), 0);
        pic.set_irq_line(7, true);
        assert_eq!(pic.acknowledge(), Some(base as u8 + 7));
        pic.set_irq_line(7, false);
        assert_eq!(pic.acknowledge(), Some(base as u8 + 7));
        assert_eq!(read_register(&mut bus, command, 0x0B), 0x80);
    }
}

#[test]
fn irq_level_slave_fall_withdraws_master_cascade_before_inta() {
    let (mut pic, mut master, mut slave) = init_cascade();
    pic.set_irq_line(14, true);
    assert_eq!(pic.poll(), Some(0x76));
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 4);
    pic.set_irq_line(14, false);
    assert_eq!(read_register(&mut slave, 0xA0, 0x0A), 0);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0);
    assert_eq!(pic.poll(), None);
    assert_eq!(pic.acknowledge(), Some(0x0F));
    assert_eq!(read_register(&mut slave, 0xA0, 0x0B), 0);
    assert_eq!(read_register(&mut master, 0x20, 0x0B), 0);
}

#[test]
fn masking_slave_withdraws_cascade_and_serves_pending_master_irq() {
    let (mut pic, mut master, mut slave) = init_cascade();
    pic.set_irq_line(8, true);
    pic.set_irq_line(6, true);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0x44);

    slave.port_write(0xA1, 1, 0x01).unwrap();
    assert_eq!(pic.poll(), Some(0x0E));
    assert_eq!(pic.acknowledge(), Some(0x0E));
    assert_eq!(read_register(&mut slave, 0xA0, 0x0B), 0);
    assert_eq!(read_register(&mut slave, 0xA0, 0x0A), 1);
    assert_eq!(read_register(&mut master, 0x20, 0x0B), 0x40);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0);
    assert_eq!(pic.poll(), None);

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
    assert_eq!(pic.poll(), None);

    slave.port_write(0xA0, 1, 0x20).unwrap();
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0x04);
    assert_eq!(pic.poll(), None);
    master.port_write(0x20, 1, 0x20).unwrap();
    assert_eq!(pic.acknowledge(), Some(0x76));
    assert_eq!(read_register(&mut slave, 0xA0, 0x0B), 0x40);

    slave.port_write(0xA0, 1, 0x20).unwrap();
    master.port_write(0x20, 1, 0x20).unwrap();
    assert_eq!(pic.poll(), None);
}

#[test]
fn unmasking_pending_slave_request_raises_cascade_without_line_edge() {
    let (mut pic, mut master, mut slave) = init_cascade();
    slave.port_write(0xA1, 1, 0x40).unwrap();
    pic.set_irq_line(14, true);
    assert_eq!(pic.poll(), None);

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
    assert_eq!(pic.poll(), None);
    assert_eq!(read_register(&mut slave, 0xA0, 0x0B), 0);
    assert_eq!(read_register(&mut master, 0x20, 0x0B), 0);
    assert_eq!(read_register(&mut master, 0x20, 0x0A), 0);
}

#[test]
fn observation_log_records_inta_and_eoi_without_changing_service_state() {
    use chipset::PicObservation;
    use device::VClock;

    let mut controller = SharedPic::new();
    let log = controller.observe(std::rc::Rc::new(|| VClock(17)));
    let mut bus = MockBus::new(Box::new(controller.clone()));
    for (port, value) in [(0x20, 0x11), (0x21, 8), (0x21, 4), (0x21, 1)] {
        bus.port_write(port, 1, value).unwrap();
    }
    bus.tick(17);
    assert_eq!(controller.poll(), None);
    controller.set_irq_line(0, true);
    assert_eq!(controller.acknowledge(), Some(8));
    assert_eq!(controller.poll(), None);
    assert_eq!(read_register(&mut bus, 0x20, 0x0B), 1);
    bus.port_write(0x20, 1, 0x60).unwrap();
    assert_eq!(read_register(&mut bus, 0x20, 0x0B), 0);
    assert_eq!(
        log.take(),
        vec![
            PicObservation::Acknowledge {
                at: VClock(17),
                vector: 8
            },
            PicObservation::Eoi {
                at: VClock(17),
                command: 0x20,
                value: Some(0x60)
            },
        ]
    );
    assert!(log.take().is_empty());
    controller.end_of_interrupt();
    assert_eq!(
        log.take(),
        vec![PicObservation::Eoi {
            at: VClock(17),
            command: 0x20,
            value: None
        }]
    );
}

#[test]
fn cascade_observation_records_only_the_cpu_vector_and_both_eois() {
    use chipset::PicObservation;
    use device::VClock;

    let (mut controller, mut master, mut slave) = init_cascade();
    let log = controller
        .master_device()
        .observe(std::rc::Rc::new(|| VClock(25)));
    master.tick(25);
    slave.tick(25);
    controller.set_irq_line(8, true);
    assert_eq!(controller.acknowledge(), Some(0x70));
    slave.port_write(0xA0, 1, 0x20).unwrap();
    master.port_write(0x20, 1, 0x20).unwrap();
    assert_eq!(
        log.take(),
        vec![
            PicObservation::Acknowledge {
                at: VClock(25),
                vector: 0x70
            },
            PicObservation::Eoi {
                at: VClock(25),
                command: 0xA0,
                value: Some(0x20)
            },
            PicObservation::Eoi {
                at: VClock(25),
                command: 0x20,
                value: Some(0x20)
            },
        ]
    );
    controller.set_irq_line(8, false);
    controller.set_irq_line(8, true);
    assert_eq!(controller.acknowledge(), Some(0x70));
    controller.end_of_interrupt();
    assert_eq!(log.take().len(), 3);
}

#[test]
fn observation_is_opt_in_and_does_not_mistake_initialization_or_ocw3_for_eoi() {
    let mut controller = SharedPic::new();
    init_master(&mut controller);
    controller.set_irq_line(0, true);
    assert_eq!(controller.acknowledge(), Some(8));
    controller.end_of_interrupt();
    let log = controller.observe(std::rc::Rc::new(|| device::VClock::ZERO));
    assert!(log.take().is_empty());
    let mut bus = MockBus::new(Box::new(controller));
    bus.port_write(0x20, 1, 0x30).unwrap();
    bus.port_write(0x20, 1, 0x28).unwrap();
    assert!(log.take().is_empty());
}
