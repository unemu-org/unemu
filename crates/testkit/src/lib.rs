// SPDX-License-Identifier: AGPL-3.0-or-later
//! Test harness for the emulator (§10).
//!
//! Everything a device needs to be tested **without a CPU**, and everything the
//! machine loop needs to be tested **without `x86jit`**:
//!
//! - [`MockBus`]: drive a single [`Device`] with a scripted sequence of
//!   port/MMIO reads and writes, asserting the values it returns — so every
//!   device is testable in isolation, without a CPU.
//! - [`MockCpu`]: a [`Cpu`] backend that replays a canned program of
//!   [`CpuStop`]s, so `machine` and `bus` wiring is tested with no `x86jit`
//!   dependency.
//! - [`golden`]: tiny helpers to assert captured output against a committed
//!   golden.

#![forbid(unsafe_code)]

use std::collections::VecDeque;

use cpu::{Cpu, CpuError, CpuReg, CpuStepRun, CpuStepStop, CpuStop, StepRunOptions};
use device::{Device, DeviceError, VClock};

/// A mock bus that drives one device in isolation (§10).
///
/// Holds a single boxed [`Device`] and a virtual clock; the test scripts
/// accesses against it. This is the default, fast test level for every device.
pub struct MockBus {
    device: Box<dyn Device>,
    clock: VClock,
}

impl MockBus {
    /// Wrap a device for scripted testing.
    pub fn new(device: Box<dyn Device>) -> Self {
        Self {
            device,
            clock: VClock::ZERO,
        }
    }

    /// Script a port read.
    pub fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.device.port_read(port, size)
    }

    /// Script a port write.
    pub fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.device.port_write(port, size, value)
    }

    /// Script an MMIO read.
    pub fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        self.device.mmio_read(addr, size)
    }

    /// Script an MMIO write.
    pub fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.device.mmio_write(addr, size, value)
    }

    /// Advance the virtual clock by `ticks` and tick the device.
    pub fn tick(&mut self, ticks: u64) {
        self.clock = self.clock.advance(ticks);
        self.device.tick(self.clock);
    }

    /// The current virtual time.
    pub fn clock(&self) -> VClock {
        self.clock
    }

    /// Borrow the device under test.
    pub fn device(&self) -> &dyn Device {
        self.device.as_ref()
    }

    /// Mutably borrow the device under test.
    pub fn device_mut(&mut self) -> &mut dyn Device {
        self.device.as_mut()
    }
}

/// One scripted step-bounded call for [`MockCpu`] (§10).
///
/// Steps and whole-instruction retirements are independent so a test can model
/// a REP slice that completes iterations without finishing the instruction.
#[derive(Copy, Clone, Debug)]
pub struct MockCpuStep {
    /// Expected positive budget; a mismatch fails the test.
    pub max_steps: u64,
    /// Result returned by this call.
    pub run: CpuStepRun,
    /// Whole instructions retired during this call.
    pub retired: u64,
    /// Interrupt readiness after this call, independent of the mock's IF flag.
    pub irq_ready: bool,
}

/// A [`Cpu`] backend that replays a canned [`CpuStop`] program (§10).
///
/// Lets `machine`/`bus` be exercised without `x86jit`: push a script of stops;
/// each [`Cpu::run`] pops the next one. Values supplied through
/// [`Cpu::complete_port_in`] / [`Cpu::complete_mmio_read`] are recorded so a
/// test can assert what the guest would have seen.
///
/// It also models the interrupt-clock seam enough to test the `machine` wiring:
/// each `run()` bumps the retired-instruction counter by a configurable stride
/// (default 1), and [`Cpu::inject_irq`] records the vector so a test can assert
/// the acknowledge-at-inject path fired exactly once per PIT period.
#[derive(Default)]
pub struct MockCpu {
    program: VecDeque<CpuStop>,
    step_program: VecDeque<MockCpuStep>,
    irq_ready: bool,
    /// Budgets supplied to [`Cpu::run_steps`], including zero-budget calls.
    pub step_budgets: Vec<u64>,
    /// Options supplied to [`Cpu::run_steps`], paired with [`Self::step_budgets`].
    pub step_options: Vec<StepRunOptions>,
    regs: [u64; 9],
    /// Values the machine completed pending reads with, in order.
    pub completions: Vec<u64>,
    /// Retired instructions charged per `run()` (the virtual-clock stride).
    retired_per_run: u64,
    /// Retired-instruction counter, advanced by legacy runs or scripted steps.
    retired: u64,
    completed_steps: u64,
    /// Vectors the machine injected via [`Cpu::inject_irq`], in order.
    pub injected: Vec<u8>,
    /// Whether an injected vector is queued until a run boundary with IF set.
    pending_irq: bool,
    interrupts_enabled: bool,
    /// Vectors consumed at a run boundary while IF is enabled.
    pub delivered: Vec<u8>,
}

impl MockCpu {
    /// Build a mock CPU that will return `program`'s stops in order, then
    /// [`CpuStop::Halt`] once exhausted. Each `run()` retires one instruction.
    pub fn new(program: Vec<CpuStop>) -> Self {
        Self {
            program: program.into(),
            step_program: VecDeque::new(),
            irq_ready: false,
            step_budgets: Vec::new(),
            step_options: Vec::new(),
            regs: [0; 9],
            completions: Vec::new(),
            retired_per_run: 1,
            retired: 0,
            completed_steps: 0,
            injected: Vec::new(),
            pending_irq: false,
            interrupts_enabled: false,
            delivered: Vec::new(),
        }
    }

    /// Set how many instructions each `run()` retires — the stride the machine
    /// converts into virtual-clock ticks.
    pub fn with_retired_per_run(mut self, stride: u64) -> Self {
        self.retired_per_run = stride;
        self
    }

    /// Set guest IF for interrupt and HLT tests (disabled at reset).
    pub fn with_interrupts_enabled(mut self, enabled: bool) -> Self {
        self.interrupts_enabled = enabled;
        self
    }

    /// Script positive-budget step runs independently of the legacy stop list.
    /// Zero-budget calls have no guest effects and do not consume the script.
    /// An exhausted script returns an error so unexpected execution fails tests.
    /// Readiness is scripted explicitly; completion methods only record values.
    pub fn with_step_program(mut self, program: Vec<MockCpuStep>) -> Self {
        self.step_program = program.into();
        self
    }

    /// Set initial interrupt readiness, including shadows/completions, for tests
    /// that need a boundary different from the mock's separate IF flag.
    pub fn with_irq_ready(mut self, ready: bool) -> Self {
        self.irq_ready = ready;
        self
    }

    fn reg_slot(reg: CpuReg) -> usize {
        match reg {
            CpuReg::Rax => 0,
            CpuReg::Rcx => 1,
            CpuReg::Rdx => 2,
            CpuReg::Rbx => 3,
            CpuReg::Rsp => 4,
            CpuReg::Rbp => 5,
            CpuReg::Rsi => 6,
            CpuReg::Rdi => 7,
            CpuReg::Rip => 8,
        }
    }
}

impl Cpu for MockCpu {
    fn run(&mut self, _budget: Option<u64>) -> Result<CpuStop, CpuError> {
        // Model the CPU delivering any queued interrupt at the run() boundary:
        // IF must permit delivery before the machine can acknowledge another
        // request; CLI holds the injected vector in flight.
        if self.pending_irq && self.interrupts_enabled {
            self.delivered
                .push(*self.injected.last().expect("pending vector"));
            self.pending_irq = false;
        }
        self.retired += self.retired_per_run;
        Ok(self.program.pop_front().unwrap_or(CpuStop::Halt))
    }

    fn run_steps(
        &mut self,
        max_steps: u64,
        options: StepRunOptions,
    ) -> Result<CpuStepRun, CpuError> {
        self.step_budgets.push(max_steps);
        self.step_options.push(options);
        if max_steps == 0 {
            return Ok(CpuStepRun {
                steps: 0,
                stop: CpuStepStop::Exit(CpuStop::BudgetExhausted),
            });
        }
        if self.pending_irq && self.irq_ready {
            self.delivered
                .push(*self.injected.last().expect("pending vector"));
            self.pending_irq = false;
            self.irq_ready = false;
        }
        if options.stop_when_irq_ready && self.irq_ready {
            return Ok(CpuStepRun {
                steps: 0,
                stop: CpuStepStop::IrqReady,
            });
        }
        let step = self
            .step_program
            .pop_front()
            .ok_or_else(|| CpuError::UnexpectedExit {
                what: "mock step script exhausted".to_string(),
            })?;
        assert_eq!(step.max_steps, max_steps, "unexpected mock step budget");
        assert!(
            step.run.steps <= max_steps,
            "mock result exceeds step budget"
        );
        if step.run.stop == CpuStepStop::IrqReady {
            assert!(
                options.stop_when_irq_ready,
                "unsolicited mock readiness stop"
            );
            assert!(step.irq_ready, "mock readiness stop must be IRQ-ready");
        }
        self.completed_steps += step.run.steps;
        self.retired += step.retired;
        self.irq_ready = step.irq_ready;
        Ok(step.run)
    }

    fn completed_steps(&self) -> u64 {
        self.completed_steps
    }

    fn irq_ready(&self) -> bool {
        self.irq_ready
    }

    fn reg(&self, reg: CpuReg) -> u64 {
        self.regs[Self::reg_slot(reg)]
    }

    fn set_reg(&mut self, reg: CpuReg, value: u64) {
        self.regs[Self::reg_slot(reg)] = value;
    }

    fn complete_port_in(&mut self, value: u64) {
        self.completions.push(value);
    }

    fn complete_mmio_read(&mut self, value: u64) {
        self.completions.push(value);
    }

    fn complete_mmio_write(&mut self) {
        // Scripted stops advance on their own; nothing to acknowledge here.
    }

    fn inject_irq(&mut self, vector: u8) {
        self.injected.push(vector);
        self.pending_irq = true;
    }

    fn interrupts_enabled(&self) -> bool {
        self.interrupts_enabled
    }

    fn has_pending_irq(&self) -> bool {
        self.pending_irq
    }

    fn retired(&self) -> u64 {
        self.retired
    }
}

/// Golden-comparison helpers (§10).
pub mod golden {
    /// Assert captured bytes equal the expected golden, with a readable panic on
    /// mismatch (lossy UTF-8 for the message).
    pub fn assert_bytes_eq(actual: &[u8], expected: &[u8]) {
        assert_eq!(
            actual,
            expected,
            "golden mismatch:\n  actual   = {:?}\n  expected = {:?}",
            String::from_utf8_lossy(actual),
            String::from_utf8_lossy(expected),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpu::{CpuStepRun, CpuStepStop};

    #[test]
    fn ready_stop_defers_to_zero_budget_and_queued_delivery() {
        let mut cpu = MockCpu::new(vec![])
            .with_irq_ready(true)
            .with_step_program(vec![MockCpuStep {
                max_steps: 8,
                run: CpuStepRun {
                    steps: 1,
                    stop: CpuStepStop::Exit(CpuStop::Halt),
                },
                retired: 1,
                irq_ready: false,
            }]);
        let options = StepRunOptions {
            stop_when_irq_ready: true,
        };
        assert_eq!(
            cpu.run_steps(0, options).unwrap(),
            CpuStepRun {
                steps: 0,
                stop: CpuStepStop::Exit(CpuStop::BudgetExhausted),
            }
        );
        assert_eq!(
            cpu.run_steps(8, options).unwrap(),
            CpuStepRun {
                steps: 0,
                stop: CpuStepStop::IrqReady,
            }
        );
        assert_eq!(cpu.completed_steps(), 0);
        cpu.inject_irq(8);
        assert_eq!(cpu.run_steps(8, options).unwrap().steps, 1);
        assert_eq!(cpu.delivered, [8]);
        assert_eq!(cpu.completed_steps(), 1);
        assert_eq!(cpu.step_options, [options; 3]);
    }

    #[test]
    fn stepped_script_tracks_budget_retirement_and_readiness() {
        let run = CpuStepRun {
            steps: 3,
            stop: CpuStepStop::IrqReady,
        };
        let mut cpu = MockCpu::new(vec![]).with_step_program(vec![MockCpuStep {
            max_steps: 7,
            run,
            retired: 0,
            irq_ready: true,
        }]);
        assert!(!cpu.irq_ready());
        let zero = cpu.run_steps(0, StepRunOptions::default()).unwrap();
        assert_eq!(zero.steps, 0);
        assert_eq!(zero.stop, CpuStepStop::Exit(CpuStop::BudgetExhausted));
        assert!(!cpu.irq_ready());
        assert_eq!(cpu.completed_steps(), 0);
        assert_eq!(
            cpu.run_steps(
                7,
                StepRunOptions {
                    stop_when_irq_ready: true
                }
            )
            .unwrap(),
            run
        );
        assert_eq!(cpu.completed_steps(), 3);
        assert_eq!(cpu.retired(), 0); // A partial REP can consume steps without retiring.
        assert!(cpu.irq_ready());
        assert_eq!(cpu.step_budgets, vec![0, 7]);
        assert!(matches!(
            cpu.run_steps(1, StepRunOptions::default()),
            Err(CpuError::UnexpectedExit { .. })
        ));
        assert_eq!(cpu.completed_steps(), 3);
    }

    #[test]
    fn stepped_script_is_independent_of_legacy_stops_and_uses_ready_gate() {
        let run = CpuStepRun {
            steps: 1,
            stop: CpuStepStop::Exit(CpuStop::Halt),
        };
        let mut cpu = MockCpu::new(vec![CpuStop::PortOut {
            port: 0x80,
            size: 1,
            value: 0,
        }])
        .with_interrupts_enabled(true)
        .with_irq_ready(false)
        .with_step_program(vec![
            MockCpuStep {
                max_steps: 1,
                run,
                retired: 1,
                irq_ready: true,
            },
            MockCpuStep {
                max_steps: 1,
                run,
                retired: 1,
                irq_ready: false,
            },
        ]);
        cpu.inject_irq(0x20);
        cpu.run_steps(1, StepRunOptions::default()).unwrap();
        assert!(cpu.has_pending_irq());
        assert!(cpu.delivered.is_empty());
        cpu.run_steps(0, StepRunOptions::default()).unwrap();
        assert!(cpu.has_pending_irq());
        cpu.run_steps(1, StepRunOptions::default()).unwrap();
        assert!(!cpu.has_pending_irq());
        assert_eq!(cpu.delivered, vec![0x20]);
        assert_eq!(cpu.retired(), 2);
        assert!(matches!(cpu.run(Some(1)).unwrap(), CpuStop::PortOut { .. }));
        assert_eq!(cpu.retired(), 3);
    }
}
