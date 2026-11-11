// SPDX-License-Identifier: AGPL-3.0-or-later
//! CPU-only AT warm restart (Intel 80286 PRM §5.3; IBM AT Technical Reference,
//! System Board, Keyboard Controller Output Port; machine-config.md §Board model).

use bus::{Bus, DecodeMap, PortRange, PortWidth};
use chipset::NoInterruptController;
use cpu::{x86jit_backend::RealSegments, Cpu, CpuReg, X86JitCpu};
use devices::{CmosRtc, I8042};
use machine::{Machine, RunOutcome};

#[test]
fn keyboard_reset_restarts_rom_and_preserves_ram_cmos_devices_and_time() {
    let mut cpu = X86JitCpu::new_real16_bare(0x10_0000, 0, 0x7000, RealSegments::flat(0)).unwrap();
    cpu.map_ram(0, 0xF0000).unwrap();
    for (index, reg) in [
        CpuReg::Rbx,
        CpuReg::Rcx,
        CpuReg::Rdx,
        CpuReg::Rbp,
        CpuReg::Rsi,
        CpuReg::Rdi,
    ]
    .into_iter()
    .enumerate()
    {
        cpu.set_reg(reg, 0x1230 + index as u64);
    }
    let program = [
        0xC6, 0x06, 0, 5, 0x77, // RAM marker.
        0xB0, 0x0F, 0xE6, 0x70, 0xB0, 0x0A, 0xE6, 0x71, // CMOS shutdown byte.
        0xB0, 0xD1, 0xE6, 0x64, 0xB0, 1, 0xE6, 0x60, // Keep reset high, A20 low.
        0x68, 0xD7, 6, 0x9D, // Dirty FLAGS, including IF and DF.
        0xB8, 0, 1, 0x8E, 0xD8, 0x8E, 0xC0, 0x8E, 0xD0, // Dirty DS/ES/SS.
        0xB0, 0xFE, 0xE6, 0x64, 0xF4, // The old instruction stream must not resume here.
    ];
    cpu.write_mem(0x7000, &program).unwrap();
    let mut rom = vec![0; 0x10000];
    rom[0xFFF0..0xFFF5].copy_from_slice(&[0xEA, 0, 1, 0, 0xF0]);
    let stub = [
        0xC6, 0x06, 1, 5, 0xA5, // Reset vector reached with DS=0.
        0x8C, 0x1E, 2, 5, // Save DS.
        0x8C, 0x06, 4, 5, // Save ES.
        0x8C, 0x16, 6, 5, // Save SS.
        0x9C, 0x58, 0xA3, 8, 5, // Save FLAGS through the reset SS.
        0xF4,
    ];
    rom[0x100..0x100 + stub.len()].copy_from_slice(&stub);
    cpu.map_rom(0xF0000, &rom).unwrap();
    let keyboard = I8042::new();
    let reset = keyboard.reset_signal();
    let bus = Bus::new(
        vec![Box::new(keyboard), Box::new(CmosRtc::new())],
        DecodeMap {
            ports: vec![
                PortRange {
                    ports: 0x60..=0x64,
                    device: 0,
                    width: PortWidth::Byte,
                },
                PortRange {
                    ports: 0x70..=0x71,
                    device: 1,
                    width: PortWidth::Byte,
                },
            ],
            mmio: Vec::new(),
        },
    );
    let mut machine = Machine::new(cpu, bus, Box::new(NoInterruptController), Vec::new(), None)
        .with_reset_signal(reset);
    assert_eq!(
        machine.run_bounded(Some(100)).unwrap(),
        RunOutcome::ResetRequested
    );
    let time = machine.virtual_time();
    let clock = machine.clock();
    let retired = machine.cpu().retired();
    let regs = [
        CpuReg::Rax,
        CpuReg::Rbx,
        CpuReg::Rcx,
        CpuReg::Rdx,
        CpuReg::Rsp,
        CpuReg::Rbp,
        CpuReg::Rsi,
        CpuReg::Rdi,
    ];
    let before = regs.map(|reg| machine.cpu().reg(reg));
    machine.reset_cpu().unwrap();
    assert_eq!(machine.cpu().code_segment(), 0xF000);
    assert_eq!(machine.cpu().reg(CpuReg::Rip), 0xFFF0);
    assert!(!machine.cpu().interrupts_enabled());
    assert_eq!(regs.map(|reg| machine.cpu().reg(reg)), before);
    assert_eq!(machine.cpu().retired(), retired);
    assert_eq!(machine.virtual_time(), time);
    assert_eq!(machine.clock(), clock);
    assert_eq!(machine.bus_mut().port_read(0x71, 1).unwrap(), 0x0A);
    machine.bus_mut().port_write(0x64, 1, 0xD0).unwrap();
    assert_eq!(machine.bus_mut().port_read(0x60, 1).unwrap(), 1);
    assert_eq!(machine.run_bounded(Some(100)).unwrap(), RunOutcome::Halted);
    let mut memory = [0xFF; 10];
    machine.cpu().read_mem(0x500, &mut memory).unwrap();
    assert_eq!(memory, [0x77, 0xA5, 0, 0, 0, 0, 0, 0, 2, 0]);
    assert!(machine.cpu().retired() > retired);
    assert!(machine.virtual_time() > time);
    assert!(machine.clock() > clock);
    assert_eq!(machine.bus_mut().port_read(0x71, 1).unwrap(), 0x0A);
}

fn reset_test_cpu(program: &[u8], vector: &[u8]) -> X86JitCpu {
    let mut cpu = X86JitCpu::new_real16_bare(0x10_0000, 0, 0x7000, RealSegments::flat(0)).unwrap();
    cpu.map_ram(0, 0xF0000).unwrap();
    cpu.write_mem(0x7000, program).unwrap();
    cpu.map_rom(0xFFFF0, vector).unwrap();
    cpu
}

fn keyboard_bus(keyboard: I8042) -> Bus {
    Bus::new(
        vec![Box::new(keyboard)],
        DecodeMap {
            ports: vec![PortRange {
                ports: 0x60..=0x64,
                device: 0,
                width: PortWidth::Byte,
            }],
            mmio: Vec::new(),
        },
    )
}

#[test]
fn next_run_performs_the_reported_reset_before_execution_or_cap() {
    for bounded in [false, true] {
        let cpu = reset_test_cpu(
            &[0xB0, 0xFE, 0xE6, 0x64, 0xF4],
            &[0xC6, 0x06, 0, 5, 0xA5, 0xF4],
        );
        let keyboard = I8042::new();
        let signal = keyboard.reset_signal();
        let mut machine = Machine::new(
            cpu,
            keyboard_bus(keyboard),
            Box::new(NoInterruptController),
            Vec::new(),
            None,
        )
        .with_reset_signal(signal.clone());
        assert_eq!(
            machine.run_bounded(Some(2)).unwrap(),
            RunOutcome::ResetRequested
        );
        assert!(!signal.get());
        assert_eq!(machine.cpu().code_segment(), 0);
        if bounded {
            assert_eq!(
                machine.run_bounded(Some(2)).unwrap(),
                RunOutcome::RetiredCapReached
            );
            assert_eq!(machine.cpu().code_segment(), 0xF000);
            assert_eq!(machine.cpu().reg(CpuReg::Rip), 0xFFF0);
            assert_eq!(machine.virtual_time(), 2);
        }
        assert_eq!(machine.run().unwrap(), RunOutcome::Halted);
        let mut marker = [0];
        machine.cpu().read_mem(0x500, &mut marker).unwrap();
        assert_eq!(marker, [0xA5]);
        let rip = machine.cpu().reg(CpuReg::Rip);
        assert_eq!(
            machine.run_bounded(Some(4)).unwrap(),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(
            machine.cpu().reg(CpuReg::Rip),
            rip,
            "a reset is performed only once"
        );
        assert_eq!(machine.cpu().retired(), 4);
        assert_eq!(machine.virtual_time(), 4);
    }
}

#[test]
fn machine_reset_abandons_an_answered_ins_before_its_deferred_store() {
    use cpu::{CpuStepStop, CpuStop, StepRunOptions};

    let mut cpu = reset_test_cpu(&[0xF3, 0x6C, 0xF4], &[0xF4]);
    cpu.set_reg(CpuReg::Rdx, 0x60);
    cpu.set_reg(CpuReg::Rcx, 3);
    cpu.set_reg(CpuReg::Rdi, 0x500);
    assert_eq!(
        cpu.run_steps(1, StepRunOptions::default()).unwrap().stop,
        CpuStepStop::Exit(CpuStop::PortIn {
            port: 0x60,
            size: 1
        })
    );
    cpu.complete_port_in(0xA7);
    let retired = cpu.retired();
    let steps = cpu.completed_steps();
    let keyboard = I8042::new();
    let signal = keyboard.reset_signal();
    let mut bus = keyboard_bus(keyboard);
    bus.port_write(0x64, 1, 0xFE).unwrap();
    let mut machine = Machine::new(cpu, bus, Box::new(NoInterruptController), Vec::new(), None)
        .with_reset_signal(signal);
    assert_eq!(
        machine.run_bounded(Some(10)).unwrap(),
        RunOutcome::ResetRequested
    );
    let outcome = machine.run_bounded(Some(10));
    let mut marker = [0xFF; 3];
    machine.cpu().read_mem(0x500, &mut marker).unwrap();
    assert_eq!(
        marker, [0; 3],
        "a pre-reset INS answer must never write guest RAM"
    );
    assert_eq!(outcome.unwrap(), RunOutcome::Halted);
    assert_eq!(machine.cpu().reg(CpuReg::Rcx), 3);
    assert_eq!(machine.cpu().reg(CpuReg::Rdi), 0x500);
    assert_eq!(machine.cpu().retired(), retired + 1);
    assert_eq!(machine.cpu().completed_steps(), steps + 1);
}

#[test]
fn pending_pic_irq_survives_cpu_reset_and_is_delivered_after_sti() {
    use chipset::SharedPic;
    use device::{Device, InterruptController};

    for explicit in [false, true] {
        let mut cpu = reset_test_cpu(&[0xB0, 0xFE, 0xE6, 0x64, 0xF4], &[0xFB, 0x90, 0xFA, 0xF4]);
        cpu.write_mem(0x21 * 4, &[0, 6, 0, 0]).unwrap();
        cpu.write_mem(
            0x600,
            &[
                0xC6, 0x06, 1, 5,
                0xA5, // Record the interrupt delivered from the retained PIC request.
                0xB0, 0x20, 0xE6, 0x20, 0xCF,
            ],
        )
        .unwrap();
        let mut pic = SharedPic::new();
        // Intel 8259A datasheet, Initialization Command Words: single,
        // edge-triggered, 8086 mode, vector base 20h, IRQ1 unmasked.
        for (port, value) in [(0x20, 0x13), (0x21, 0x20), (0x21, 1), (0x21, 0xFD)] {
            pic.port_write(port, 1, value).unwrap();
        }
        pic.set_irq_line(1, true);
        pic.set_irq_line(1, false);
        let keyboard = I8042::new();
        let signal = keyboard.reset_signal();
        let bus = Bus::new(
            vec![Box::new(keyboard), Box::new(pic.clone())],
            DecodeMap {
                ports: vec![
                    PortRange {
                        ports: 0x60..=0x64,
                        device: 0,
                        width: PortWidth::Byte,
                    },
                    PortRange {
                        ports: 0x20..=0x21,
                        device: 1,
                        width: PortWidth::Byte,
                    },
                ],
                mmio: Vec::new(),
            },
        );
        let mut machine = Machine::new(cpu, bus, Box::new(pic.clone()), Vec::new(), None)
            .with_reset_signal(signal);
        assert_eq!(
            machine.run_bounded(Some(100)).unwrap(),
            RunOutcome::ResetRequested
        );
        assert_eq!(pic.poll(), Some(0x21));
        assert!(!machine.cpu().has_pending_irq());
        let time = machine.virtual_time();
        if explicit {
            machine.reset_cpu().unwrap();
        }
        assert_eq!(
            machine.run_bounded(Some(time)).unwrap(),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.cpu().code_segment(), 0xF000);
        assert!(!machine.cpu().interrupts_enabled());
        assert_eq!(pic.poll(), Some(0x21));
        assert_eq!(machine.run_bounded(Some(100)).unwrap(), RunOutcome::Halted);
        let mut marker = [0];
        machine.cpu().read_mem(0x501, &mut marker).unwrap();
        assert_eq!(marker, [0xA5]);
        assert_eq!(pic.poll(), None);
        assert!(!machine.cpu().has_pending_irq());
    }
}
