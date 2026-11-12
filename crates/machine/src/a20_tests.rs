// SPDX-License-Identifier: AGPL-3.0-or-later
//! Board A20 synchronization tests (IBM PC AT Technical Reference, Keyboard
//! Controller, Output Port; machine-config.md §A20 line and System Control Port A).

use super::*;
use cpu::{x86jit_backend::RealSegments, CpuError, CpuReg, X86JitCpu};
use devices::{A20Line, SysCtrlPortA, I8042};
use std::{cell::Cell, rc::Rc};

struct NoIc;
impl InterruptController for NoIc {
    fn set_irq_line(&mut self, _irq: u8, _level: bool) {}
    fn poll(&self) -> Option<u8> {
        None
    }
    fn acknowledge(&mut self) -> Option<u8> {
        None
    }
    fn end_of_interrupt(&mut self) {}
}

struct CountingCpu {
    inner: X86JitCpu,
    calls: Vec<bool>,
}

impl Cpu for CountingCpu {
    fn set_a20_enabled(&mut self, enabled: bool) {
        self.calls.push(enabled);
        self.inner.set_a20_enabled(enabled);
    }
    fn a20_enabled(&self) -> bool {
        self.inner.a20_enabled()
    }
    fn reset(&mut self) -> Result<(), CpuError> {
        self.inner.reset()
    }
    fn run(&mut self, budget: Option<u64>) -> Result<CpuStop, CpuError> {
        self.inner.run(budget)
    }
    fn run_steps(
        &mut self,
        max_steps: u64,
        options: cpu::StepRunOptions,
    ) -> Result<cpu::CpuStepRun, CpuError> {
        self.inner.run_steps(max_steps, options)
    }
    fn completed_steps(&self) -> u64 {
        self.inner.completed_steps()
    }
    fn reg(&self, reg: CpuReg) -> u64 {
        self.inner.reg(reg)
    }
    fn set_reg(&mut self, reg: CpuReg, value: u64) {
        self.inner.set_reg(reg, value);
    }
    fn complete_port_in(&mut self, value: u64) {
        self.inner.complete_port_in(value);
    }
    fn complete_mmio_read(&mut self, value: u64) {
        self.inner.complete_mmio_read(value);
    }
    fn complete_mmio_write(&mut self) {
        self.inner.complete_mmio_write();
    }
    fn interrupts_enabled(&self) -> bool {
        self.inner.interrupts_enabled()
    }
}

#[test]
fn a20_transition_counter_avoids_redundant_cpu_calls() {
    let line = A20Line::default();
    let reset = Rc::new(Cell::new(false));
    let keyboard = I8042::new().with_board_lines(reset.clone(), line.clone());
    let port = SysCtrlPortA::with_board_lines(reset, line.clone());
    let bus = Bus::new(
        vec![Box::new(port), Box::new(keyboard)],
        bus::DecodeMap {
            ports: vec![
                bus::PortRange {
                    ports: 0x92..=0x92,
                    device: 0,
                    width: bus::PortWidth::Byte,
                },
                bus::PortRange {
                    ports: 0x60..=0x64,
                    device: 1,
                    width: bus::PortWidth::Byte,
                },
            ],
            mmio: Vec::new(),
        },
    );
    let mut cpu = X86JitCpu::new_a20_bare(0x100000, 0, 0x1000, RealSegments::flat(0)).unwrap();
    cpu.map_ram(0, 0x2000).unwrap();
    cpu.write_mem(
        0x1000,
        &[
            0xB0, 0xD1, 0xE6, 0x64, 0xB0, 1, 0xE6, 0x60, 0xB0, 0xD1, 0xE6, 0x64, 0xB0, 1, 0xE6,
            0x60, 0xB0, 2, 0xE6, 0x92, 0xE6, 0x92, 0xB0, 0, 0xE6, 0x92, 0xE6, 0x92, 0xF4,
        ],
    )
    .unwrap();
    let mut machine = Machine::new(
        CountingCpu {
            inner: cpu,
            calls: Vec::new(),
        },
        bus,
        Box::new(NoIc),
        Vec::new(),
        None,
    )
    .with_step_limit(NonZeroU64::MIN)
    .with_a20_line({
        let line = line.clone();
        move || (line.change_count(), line.enabled())
    });
    assert_eq!(machine.cpu().calls, [true]);
    assert_eq!(machine.run().unwrap(), RunOutcome::Halted);
    assert_eq!(machine.cpu().calls, [true, false, true, false]);
    assert!(!machine.cpu().a20_enabled());
    machine.bus_mut().port_write(0x92, 1, 2).unwrap();
    let time = machine.virtual_time();
    assert_eq!(
        machine.run_bounded(Some(time)).unwrap(),
        RunOutcome::RetiredCapReached
    );
    assert_eq!(machine.cpu().calls, [true, false, true, false, true]);
    machine.reset_cpu().unwrap();
    assert_eq!(machine.cpu().calls, [true, false, true, false, true]);
    assert!(machine.cpu().a20_enabled());
}
