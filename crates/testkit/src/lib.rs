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

use std::collections::VecDeque;

use cpu::{Cpu, CpuError, CpuReg, CpuStop};
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
    regs: [u64; 9],
    /// Values the machine completed pending reads with, in order.
    pub completions: Vec<u64>,
    /// Retired instructions charged per `run()` (the virtual-clock stride).
    retired_per_run: u64,
    /// Retired-instruction counter, advanced on each `run()`.
    retired: u64,
    /// Vectors the machine injected via [`Cpu::inject_irq`], in order.
    pub injected: Vec<u8>,
    /// Whether an injected vector is still "pending" (models the CPU gate). A
    /// test clears it via [`MockCpu::clear_pending_irq`] to model delivery.
    pending_irq: bool,
}

impl MockCpu {
    /// Build a mock CPU that will return `program`'s stops in order, then
    /// [`CpuStop::Halt`] once exhausted. Each `run()` retires one instruction.
    pub fn new(program: Vec<CpuStop>) -> Self {
        Self {
            program: program.into(),
            regs: [0; 9],
            completions: Vec::new(),
            retired_per_run: 1,
            retired: 0,
            injected: Vec::new(),
            pending_irq: false,
        }
    }

    /// Set how many instructions each `run()` retires — the stride the machine
    /// converts into virtual-clock ticks.
    pub fn with_retired_per_run(mut self, stride: u64) -> Self {
        self.retired_per_run = stride;
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
        // an injected vector is consumed by the time the next stop is reached, so
        // the machine may acknowledge the next request.
        self.pending_irq = false;
        self.retired += self.retired_per_run;
        Ok(self.program.pop_front().unwrap_or(CpuStop::Halt))
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

    fn inject_irq(&mut self, vector: u8) {
        self.injected.push(vector);
        self.pending_irq = true;
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
