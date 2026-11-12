// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8259A Edge and Level Triggered Modes across real-mode guest I/O (§5.1).
//! See `backlog/docs/design/scheduler.md`, §PIC state must follow CPU acceptance.

use bus::{Bus, DecodeMap, PortRange, PortWidth};
use chipset::{PicObservation, SharedPic};
use cpu::x86jit_backend::{RealSegments, X86JitCpu};
use device::{Device, InterruptController};
use devices::I8042;
use machine::{IrqLine, Machine, RunOutcome};

fn keyboard_request(read_before_unmask: bool) {
    let mut cpu = X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).unwrap();
    cpu.map_ram(0, 0x10000).unwrap();
    let mut program = vec![
        0xFA, 0xBC, 0x00, 0x70, // CLI; MOV SP,7000h
        0xB0, 0x13, 0xE6, 0x20, // ICW1, single edge-triggered PIC
        0xB0, 0x08, 0xE6, 0x21, // ICW2
        0xB0, 0x01, 0xE6, 0x21, // ICW4
        0xB0, 0xFF, 0xE6, 0x21, // mask all inputs
        0xB0, 0x60, 0xE6, 0x64, // write controller command byte
        0xB0, 0x01, 0xE6, 0x60, // enable keyboard IRQ
        0xE4, 0x64, // read OBF while IRQ1 is masked
    ];
    if read_before_unmask {
        program.extend_from_slice(&[0xE4, 0x60]);
    }
    program.extend_from_slice(&[0xB0, 0xFD, 0xE6, 0x21, 0xFB]);
    program.extend_from_slice(&[0x90; 20]);
    // A later polled byte must not become a second keyboard interrupt.
    program.extend_from_slice(&[
        0xFA, 0xB0, 0xFF, 0xE6, 0x21, 0xB0, 0xF4, 0xE6, 0x60, 0xE4, 0x64, 0xA8, 0x01, 0x74, 0xFA,
        0xE4, 0x60, 0xB0, 0xFD, 0xE6, 0x21, 0xFB, 0x90, 0x90, 0xFA, 0xF4,
    ]);
    cpu.write_mem(0x7C00, &program).unwrap();
    cpu.write_mem(0x24, &[0, 0x80, 0, 0]).unwrap();
    cpu.write_mem(
        0x8000,
        &[
            0xFF, 0x06, 0x00, 0x05, // INC word [0500h]
            0xE4, 0x60, // read keyboard byte
            0xB0, 0x20, 0xE6, 0x20, // EOI
            0xCF, // IRET
        ],
    )
    .unwrap();
    let mut keyboard = I8042::new();
    keyboard.push_scancode(0x1E);
    let pic = SharedPic::new();
    let bus = Bus::new(
        vec![Box::new(pic.clone()), Box::new(keyboard)],
        DecodeMap {
            ports: vec![
                PortRange {
                    ports: 0x20..=0x21,
                    device: 0,
                    width: PortWidth::default(),
                },
                PortRange {
                    ports: 0x60..=0x60,
                    device: 1,
                    width: PortWidth::default(),
                },
                PortRange {
                    ports: 0x64..=0x64,
                    device: 1,
                    width: PortWidth::default(),
                },
            ],
            mmio: Vec::new(),
        },
    );
    let log = pic.observe(bus.observation_clock());
    let mut machine = Machine::new(
        cpu,
        bus,
        Box::new(pic.clone()),
        vec![IrqLine {
            device: 1,
            output: 0,
            irq: 1,
        }],
        None,
    );
    assert_eq!(
        machine.run_bounded(Some(15)).unwrap(),
        RunOutcome::RetiredCapReached
    );
    assert!(machine.bus().device(1).unwrap().irq_pending());
    assert_eq!(pic.poll(), None);
    assert_eq!(machine.run_bounded(Some(5000)).unwrap(), RunOutcome::Halted);
    let expected = u8::from(!read_before_unmask);
    let mut count = [0; 2];
    machine.cpu().read_mem(0x500, &mut count).unwrap();
    assert_eq!(count, [expected, 0]);
    let vectors: Vec<_> = log
        .take()
        .into_iter()
        .filter_map(|event| match event {
            PicObservation::Acknowledge { vector, .. } => Some(vector),
            _ => None,
        })
        .collect();
    assert_eq!(vectors, if read_before_unmask { vec![] } else { vec![9] });
    let mut registers = pic.clone();
    for select in [0x0A, 0x0B] {
        registers.port_write(0x20, 1, select).unwrap();
        assert_eq!(registers.port_read(0x20, 1).unwrap(), 0);
    }
}

#[test]
fn irq_level_keyboard_read_before_unmask_withdraws_request() {
    keyboard_request(true);
}

#[test]
fn irq_level_keyboard_unread_at_unmask_delivers_once() {
    keyboard_request(false);
}

fn counter_cpu(program: &[u8], vector: u8, handler: &[u8]) -> X86JitCpu {
    let mut cpu = X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).unwrap();
    cpu.map_ram(0, 0x10000).unwrap();
    cpu.write_mem(0x7C00, program).unwrap();
    cpu.write_mem(u64::from(vector) * 4, &[0, 0x80, 0, 0])
        .unwrap();
    let mut code = vec![0xFF, 0x06, 0x00, 0x05];
    code.extend_from_slice(handler);
    code.push(0xCF);
    cpu.write_mem(0x8000, &code).unwrap();
    cpu
}

/// Intel 8254 datasheet, Mode 3; Intel 8259A datasheet, Edge and Level
/// Triggered Modes: an OUT0 rise cannot survive the following falling input.
#[test]
fn irq_level_timer_fall_before_sti_or_unmask_withdraws_request() {
    for masked in [false, true] {
        let mut program = vec![
            0xFA,
            0xBC,
            0x00,
            0x70,
            0xB0,
            0x13,
            0xE6,
            0x20,
            0xB0,
            0x08,
            0xE6,
            0x21,
            0xB0,
            0x01,
            0xE6,
            0x21,
            0xB0,
            if masked { 0xFF } else { 0xFE },
            0xE6,
            0x21,
            0xB0,
            0x36,
            0xE6,
            0x43,
            0xB0,
            0x00,
            0xE6,
            0x40,
            0xB0,
            0x01,
            0xE6,
            0x40,
        ];
        if masked {
            program.push(0xFB);
        }
        // Wait for low, then a genuine rise, and record the pending request.
        for branch in [0x75, 0x74] {
            program
                .extend_from_slice(&[0xB0, 0xE2, 0xE6, 0x43, 0xE4, 0x40, 0xA8, 0x80, branch, 0xF6]);
        }
        program.extend_from_slice(&[0xB0, 0x0A, 0xE6, 0x20, 0xE4, 0x20, 0xA2, 0x02, 0x05]);
        program.extend_from_slice(&[0xB0, 0xE2, 0xE6, 0x43, 0xE4, 0x40, 0xA8, 0x80, 0x75, 0xF6]);
        program.extend_from_slice(&[0xB0, 0xFE, 0xE6, 0x21, 0xFB, 0x90, 0x90, 0xFA, 0xF4]);
        let cpu = counter_cpu(&program, 8, &[0xB0, 0x20, 0xE6, 0x20]);
        let pic = SharedPic::new();
        let bus = Bus::new(
            vec![Box::new(pic.clone()), Box::new(devices::Pit8253::new())],
            DecodeMap {
                ports: vec![
                    PortRange {
                        ports: 0x20..=0x21,
                        device: 0,
                        width: PortWidth::default(),
                    },
                    PortRange {
                        ports: 0x40..=0x43,
                        device: 1,
                        width: PortWidth::default(),
                    },
                ],
                mmio: Vec::new(),
            },
        );
        let log = pic.observe(bus.observation_clock());
        let mut machine = Machine::new(
            cpu,
            bus,
            Box::new(pic),
            vec![IrqLine {
                device: 1,
                output: 0,
                irq: 0,
            }],
            None,
        );
        assert_eq!(machine.run_bounded(Some(3000)).unwrap(), RunOutcome::Halted);
        let mut bytes = [0; 3];
        machine.cpu().read_mem(0x500, &mut bytes).unwrap();
        assert_eq!(bytes[2] & 1, 1, "a real rising edge must first be pending");
        assert_eq!(&bytes[..2], &[0, 0], "falling OUT0 must cancel the request");
        assert!(!machine.bus().device(1).unwrap().irq_pending());
        assert!(!log
            .take()
            .iter()
            .any(|e| matches!(e, PicObservation::Acknowledge { .. })));
    }
}

/// Motorola MC146818 datasheet, Register C; Intel 8259A datasheet, Edge and
/// Level Triggered Modes: clearing the source before unmasking withdraws IRR.
#[test]
fn irq_level_rtc_register_c_read_before_slave_unmask_withdraws_request() {
    let program = [
        0xFA, 0xBC, 0x00, 0x70, 0xB0, 0x11, 0xE6, 0x20, 0xB0, 0x08, 0xE6, 0x21, 0xB0, 0x04, 0xE6,
        0x21, 0xB0, 0x01, 0xE6, 0x21, 0xB0, 0x11, 0xE6, 0xA0, 0xB0, 0x70, 0xE6, 0xA1, 0xB0, 0x02,
        0xE6, 0xA1, 0xB0, 0x01, 0xE6, 0xA1, 0xB0, 0xFB, 0xE6, 0x21, 0xB0, 0xFF, 0xE6, 0xA1, 0xB0,
        0x0B, 0xE6, 0x70, 0xB0, 0x12, 0xE6, 0x71, 0xFB, 0xB9, 0xB0, 0x04, 0xE2, 0xFE, 0xB0, 0x0A,
        0xE6, 0xA0, 0xE4, 0xA0, 0xA2, 0x02, 0x05, 0xB0, 0x0C, 0xE6, 0x70, 0xE4, 0x71, 0xA2, 0x03,
        0x05, 0xB0, 0xFE, 0xE6, 0xA1, 0x90, 0x90, 0xFA, 0xF4,
    ];
    let cpu = counter_cpu(
        &program,
        0x70,
        &[
            0xB0, 0x0C, 0xE6, 0x70, 0xE4, 0x71, 0xB0, 0x20, 0xE6, 0xA0, 0xE6, 0x20,
        ],
    );
    let pic = chipset::CascadedPic::new();
    let master = pic.master_device();
    let bus = Bus::new(
        vec![
            Box::new(master.clone()),
            Box::new(pic.slave_device()),
            Box::new(devices::CmosRtc::new().with_timing(1024, 1)),
        ],
        DecodeMap {
            ports: vec![
                PortRange {
                    ports: 0x20..=0x21,
                    device: 0,
                    width: PortWidth::default(),
                },
                PortRange {
                    ports: 0xA0..=0xA1,
                    device: 1,
                    width: PortWidth::default(),
                },
                PortRange {
                    ports: 0x70..=0x71,
                    device: 2,
                    width: PortWidth::default(),
                },
            ],
            mmio: Vec::new(),
        },
    );
    let log = master.observe(bus.observation_clock());
    let mut machine = Machine::new(
        cpu,
        bus,
        Box::new(pic),
        vec![IrqLine {
            device: 2,
            output: 0,
            irq: 8,
        }],
        None,
    );
    assert_eq!(machine.run_bounded(Some(2000)).unwrap(), RunOutcome::Halted);
    let mut bytes = [0; 4];
    machine.cpu().read_mem(0x500, &mut bytes).unwrap();
    assert_eq!(
        bytes[2] & 1,
        1,
        "masked slave request must first be pending"
    );
    assert_ne!(
        bytes[3] & 0x80,
        0,
        "register C must acknowledge a real RTC IRQ"
    );
    assert_eq!(&bytes[..2], &[0, 0]);
    assert!(!machine.bus().device(2).unwrap().irq_pending());
    assert!(!log
        .take()
        .iter()
        .any(|e| matches!(e, PicObservation::Acknowledge { .. })));
}
