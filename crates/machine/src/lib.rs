// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine composition root (§3.1).
//!
//! [`Machine`] owns a [`Cpu`] and a [`Bus`] and runs the
//! fetch/dispatch/tick loop. It is the composition root: **nothing here names a
//! concrete device, chipset, or CPU backend** — it sees only the [`Cpu`] and
//! [`Bus`] seams (§3.2). A frontend (`cli`) builds the concrete parts and hands
//! them in.
//!
//! The loop (§3.1, memory/ownership model in `architecture.md`):
//! `cpu.run(quantum)` → match [`CpuStop`] → dispatch to the bus (port/MMIO) →
//! run the completion handshake for reads → [`Bus::tick_all`] on the virtual
//! clock → repeat until [`CpuStop::Halt`].

use bus::{Bus, BusError};
use cpu::{Cpu, CpuError, CpuStop};
use device::VClock;
use thiserror::Error;

/// Errors from running a [`Machine`] (§3.1).
#[derive(Debug, Error)]
pub enum MachineError {
    /// The CPU seam raised an error.
    #[error("cpu: {0}")]
    Cpu(#[from] CpuError),
    /// The bus seam raised an error (undecoded access or device fault).
    #[error("bus: {0}")]
    Bus(#[from] BusError),
    /// The guest raised a CPU exception the machine does not yet handle.
    #[error("unhandled guest exception, vector {vector:#x}")]
    Exception {
        /// x86 exception vector.
        vector: u8,
    },
}

/// A running machine: a CPU seam plus a bus seam (§3.1).
pub struct Machine<C: Cpu> {
    cpu: C,
    bus: Bus,
    clock: VClock,
    quantum: Option<u64>,
}

impl<C: Cpu> Machine<C> {
    /// Assemble a machine from a CPU backend and a bus.
    ///
    /// `quantum` is the cooperative run budget in `x86jit` block units; devices
    /// tick on each `run()` boundary, so it governs interrupt latency and
    /// goldens depend on it (§6.2). `None` runs each quantum to a non-budget
    /// stop.
    pub fn new(cpu: C, bus: Bus, quantum: Option<u64>) -> Self {
        Self {
            cpu,
            bus,
            clock: VClock::ZERO,
            quantum,
        }
    }

    /// The CPU backend (for post-run inspection).
    pub fn cpu(&self) -> &C {
        &self.cpu
    }

    /// The bus (to drain device output / traces after a run).
    pub fn bus(&self) -> &Bus {
        &self.bus
    }

    /// Mutable bus access.
    pub fn bus_mut(&mut self) -> &mut Bus {
        &mut self.bus
    }

    /// The current virtual time (§6).
    pub fn clock(&self) -> VClock {
        self.clock
    }

    /// Run until the guest halts (§3.1).
    ///
    /// Each iteration: run the CPU for one quantum, service the resulting stop
    /// against the bus, advance the virtual clock by one boundary, and tick
    /// every device. Port/MMIO reads run the CPU-side completion handshake so
    /// the retried instruction sees the device's value.
    ///
    // The clock advances one unit per run() boundary for now. Once the CPU core
    // exposes a retired-instruction count, advance by that instead.
    pub fn run(&mut self) -> Result<(), MachineError> {
        loop {
            let stop = self.cpu.run(self.quantum)?;
            match stop {
                CpuStop::PortOut { port, size, value } => {
                    self.bus.port_write(port, size, value as u32)?;
                }
                CpuStop::PortIn { port, size } => {
                    let value = self.bus.port_read(port, size)?;
                    self.cpu.complete_port_in(u64::from(value));
                }
                CpuStop::MmioWrite { addr, size, value } => {
                    self.bus.mmio_write(addr, size, value)?;
                }
                CpuStop::MmioRead { addr, size } => {
                    let value = self.bus.mmio_read(addr, size)?;
                    self.cpu.complete_mmio_read(value);
                }
                CpuStop::BudgetExhausted => {}
                CpuStop::Exception { vector } => {
                    return Err(MachineError::Exception { vector });
                }
                CpuStop::Halt => {
                    self.advance_clock();
                    return Ok(());
                }
            }
            self.advance_clock();
        }
    }

    fn advance_clock(&mut self) {
        self.clock = self.clock.advance(1);
        self.bus.tick_all(self.clock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::{DecodeMap, PortRange};
    use device::{Device, DeviceError, TraceEvent};
    use testkit::MockCpu;

    /// A minimal capture device: records port writes, so the loop can be tested
    /// without a concrete `devices` chip.
    #[derive(Default)]
    struct CaptureDevice {
        written: Vec<u8>,
        ticks: u64,
    }

    impl Device for CaptureDevice {
        fn name(&self) -> &'static str {
            "capture"
        }
        fn port_read(&mut self, _port: u16, _size: u8) -> Result<u32, DeviceError> {
            Ok(0xAB)
        }
        fn port_write(&mut self, _port: u16, _size: u8, value: u32) -> Result<(), DeviceError> {
            self.written.push(value as u8);
            Ok(())
        }
        fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
            Err(DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: addr,
            })
        }
        fn mmio_write(&mut self, addr: u64, _size: u8, _value: u64) -> Result<(), DeviceError> {
            Err(DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: addr,
            })
        }
        fn tick(&mut self, _now: VClock) {
            self.ticks += 1;
        }
        fn irq_pending(&self) -> bool {
            false
        }
        fn irq_vector(&self) -> Option<u8> {
            None
        }
        fn reset(&mut self) {
            self.written.clear();
            self.ticks = 0;
        }
        fn save(&self) -> Vec<u8> {
            self.written.clone()
        }
        fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
            self.written = snapshot.to_vec();
            Ok(())
        }
        fn take_trace(&mut self) -> Vec<TraceEvent> {
            Vec::new()
        }
    }

    fn bus_with_capture() -> Bus {
        let devices: Vec<Box<dyn Device>> = vec![Box::new(CaptureDevice::default())];
        let map = DecodeMap {
            ports: vec![PortRange {
                ports: 0x3F8..=0x3FF,
                device: 0,
            }],
            mmio: Vec::new(),
        };
        Bus::new(devices, map)
    }

    #[test]
    fn loop_dispatches_port_out_then_halts() {
        let program = vec![
            CpuStop::PortOut {
                port: 0x3F8,
                size: 1,
                value: b'h' as u64,
            },
            CpuStop::PortOut {
                port: 0x3F8,
                size: 1,
                value: b'i' as u64,
            },
            CpuStop::Halt,
        ];
        let mut machine = Machine::new(MockCpu::new(program), bus_with_capture(), None);
        machine.run().expect("run");

        // Three run() boundaries (two writes + halt) tick the clock three times.
        assert_eq!(machine.clock(), VClock(3));
    }

    #[test]
    fn loop_completes_port_in() {
        let program = vec![CpuStop::PortIn {
            port: 0x3F8,
            size: 1,
        }];
        let mut machine = Machine::new(MockCpu::new(program), bus_with_capture(), None);
        machine.run().expect("run");
        // The capture device returns 0xAB for reads; the machine completes the
        // pending in with it.
        assert_eq!(machine.cpu().completions, vec![0xAB]);
    }

    #[test]
    fn loop_surfaces_exception() {
        let program = vec![CpuStop::Exception { vector: 6 }];
        let mut machine = Machine::new(MockCpu::new(program), bus_with_capture(), None);
        let err = machine.run().unwrap_err();
        assert!(matches!(err, MachineError::Exception { vector: 6 }));
    }
}
