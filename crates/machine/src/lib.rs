// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine composition root (§3.1).
//!
//! [`Machine`] owns a [`Cpu`], a [`Bus`], and an [`InterruptController`] and runs
//! the fetch/dispatch/tick/interrupt loop. It is the composition root: **nothing
//! here names a concrete device, chipset, or CPU backend** — it sees only the
//! [`Cpu`], [`Bus`], and [`InterruptController`] seams (§3.2). A frontend (`cli`)
//! builds the concrete parts and hands them in.
//!
//! ## The loop (§3.1)
//!
//! `cpu.run(quantum)` → match [`CpuStop`] → dispatch to the bus (port/MMIO) → run
//! the completion handshake for reads → drive the deterministic clock and the
//! interrupt path → repeat.
//!
//! ## Virtual clock (§6, virtual-clock model in `architecture.md`)
//!
//! The clock advances from the **retired-instruction delta** across each
//! `run()` — never wall time — so every golden is reproducible. Retired
//! instructions convert to PIT input clocks by a fixed, documented ratio,
//! [`RETIRED_PER_PIT_CLOCK`]. This is a *modelling approximation* (one PIT clock
//! per retired instruction by default), not a cycle-accurate 1.193 MHz timebase:
//! it only has to be deterministic and let the timer fire in a bounded number of
//! instructions.
//!
//! ## Interrupt wiring (§5)
//!
//! A device drives a *level* on an IRQ line (the PIT's counter-0 OUT), while the
//! 8259 PIC is *edge-triggered*. The devices deliberately do not edge-detect; the
//! machine does, here:
//!
//! - **Edge-detect.** After ticking the devices, the machine samples each wired
//!   IRQ line ([`Bus::device_irq_line`]) and, on a rising edge, pulses
//!   [`InterruptController::set_irq_line`] — one PIC request per timer period,
//!   matching real IRQ0 wiring.
//! - **Acknowledge-at-inject.** When the CPU has no injected IRQ in flight
//!   ([`Cpu::has_pending_irq`]) and the PIC has a serviceable vector, the machine
//!   runs INTA ([`InterruptController::acknowledge`], IRR→ISR, returns
//!   `base+irq`) and injects the vector ([`Cpu::inject_irq`]). The CPU delivers it
//!   at the next boundary once the guest's `IF` is set — that gating lives in the
//!   CPU. The guest's `EOI` (an `out` to `0x20`) reaches the PIC through the
//!   `Bus`, clearing the ISR the machine's INTA set (the PIC is shared).
//! - **HLT wakeup.** On [`CpuStop::Halt`] the machine keeps advancing the clock
//!   and ticking devices so the timer can fire and inject → the CPU resumes past
//!   the `hlt`. If the interrupt source falls quiescent (the guest masked it, so
//!   no serviceable vector appears for [`Machine::idle_limit`] consecutive halted
//!   boundaries), the machine terminates rather than spinning forever.

use bus::{Bus, BusError};
use cpu::{Cpu, CpuError, CpuStop};
use device::{DmaMemory, DmaOp, DmaOutcome, InterruptController, VClock};
use thiserror::Error;

/// Retired guest instructions per PIT input clock (§6).
///
/// The virtual-clock model advances the PIT by `retired_delta /
/// RETIRED_PER_PIT_CLOCK` input clocks. `1` means one PIT clock per retired
/// instruction — the simplest deterministic mapping. It is a modelling
/// approximation, not the real ~1.193182 MHz OSC/12 timebase; goldens depend on
/// this constant, so changing it re-baselines them.
pub const RETIRED_PER_PIT_CLOCK: u64 = 1;

/// One wired connection from a device IRQ line to a PIC input (§5.1).
///
/// Declares *which* device (by its index in the chipset-declared device set)
/// drives *which* PIC input line. The machine edge-detects the device's line and
/// pulses that PIC input; it never names the concrete device type.
#[derive(Copy, Clone, Debug)]
pub struct IrqLine {
    /// Index of the source device in the bus's device set.
    pub device: usize,
    /// The PIC input line this device's OUT is wired to (e.g. `0` for the PIT).
    pub irq: u8,
}

/// One wired coupling between a bus-master device and a DMA controller (§7.7).
///
/// A device such as the floppy controller cannot reach guest memory itself; it
/// queues a [`device::DmaRequest`] and relies on a DMA controller (the 8237) to
/// move the bytes. This declares *which* device masters the bus (`requester`) and
/// *which* controller runs its transfers (`controller`), both by their index in
/// the bus's device set — the machine never names either concrete type, exactly
/// as [`IrqLine`] wires interrupts by index.
#[derive(Copy, Clone, Debug)]
pub struct DmaWiring {
    /// Index of the bus-master device (e.g. the floppy controller).
    pub requester: usize,
    /// Index of the DMA controller that services the requester's transfers.
    pub controller: usize,
}

/// Bridges the CPU's guest-memory access to the [`DmaMemory`] handle a DMA
/// controller expects, so every DMA byte goes through the `Vm` `note_write` path
/// (§4.3). Lives only for the duration of one transfer.
struct CpuDmaMemory<'a, C: Cpu> {
    cpu: &'a mut C,
}

impl<C: Cpu> DmaMemory for CpuDmaMemory<'_, C> {
    fn dma_read(&self, addr: u32) -> u8 {
        self.cpu.guest_dma_read(addr)
    }

    fn dma_write(&mut self, addr: u32, value: u8) {
        self.cpu.guest_dma_write(addr, value);
    }
}

/// Errors from running a [`Machine`] (§3.1).
#[derive(Debug, Error)]
pub enum MachineError {
    /// The CPU seam raised an error.
    #[error("cpu: {0}")]
    Cpu(#[from] CpuError),
    /// The bus seam raised an error (undecoded access or device fault).
    #[error("bus: {0}")]
    Bus(#[from] BusError),
    /// The guest raised a CPU exception the machine does not handle. In real
    /// mode most vectors deliver in-guest through the IVT; this surfaces only an
    /// exception the CPU core chose to escape.
    #[error("unhandled guest exception, vector {vector:#x}")]
    Exception {
        /// x86 exception vector.
        vector: u8,
    },
}

/// Why [`Machine::run_bounded`] returned control to the caller (§3.1).
///
/// The natural stop is [`RunOutcome::Halted`] — the guest halted and the
/// interrupt source fell quiescent, or there is nothing left to make progress.
/// [`RunOutcome::RetiredCapReached`] means a total retired-instruction cap was
/// hit first: a safety net so a guest that spins (a never-halting loop that keeps
/// retiring instructions) cannot run forever. Reaching the cap is a clean stop,
/// not an error, so any output captured up to that point is preserved.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// The guest halted with no more interrupts to service (the natural end).
    Halted,
    /// The total retired-instruction cap was reached before a natural stop.
    RetiredCapReached,
}

/// A running machine: a CPU seam, a bus seam, and an interrupt controller (§3.1).
pub struct Machine<C: Cpu> {
    cpu: C,
    bus: Bus,
    pic: Box<dyn InterruptController>,
    clock: VClock,
    quantum: Option<u64>,
    /// Device IRQ lines wired to PIC inputs, edge-detected each boundary.
    irq_lines: Vec<IrqLine>,
    /// Last observed level of each wired line (parallel to `irq_lines`), for
    /// rising-edge detection. `None` until the first sample (no spurious edge at
    /// reset).
    last_levels: Vec<Option<bool>>,
    /// Retired-instruction count at the previous boundary; the delta drives the
    /// clock.
    last_retired: u64,
    /// Fractional PIT clocks not yet emitted (when the ratio is > 1).
    clock_remainder: u64,
    /// Consecutive halted boundaries with no interrupt injected before the
    /// machine declares the interrupt source quiescent and stops. `None` never
    /// gives up (only safe when an interrupt is guaranteed to keep firing).
    idle_limit: Option<u64>,
    /// Bus-master → DMA-controller couplings drained each boundary (§7.7). Empty
    /// for a machine with no DMA (the serial and timer demos).
    dma_wirings: Vec<DmaWiring>,
}

impl<C: Cpu> Machine<C> {
    /// Assemble a machine from a CPU backend, a bus, and an interrupt controller.
    ///
    /// `quantum` is the cooperative run budget in `x86jit` block units; devices
    /// tick on each `run()` boundary, so it governs interrupt latency and goldens
    /// depend on it (§6.2). `None` runs each quantum to a non-budget stop.
    ///
    /// `irq_lines` declares which device lines feed which PIC inputs (§5.1);
    /// empty for a machine with no interrupts. `idle_limit` bounds how long the
    /// machine keeps a halted guest alive waiting for an interrupt before
    /// declaring quiescence (`None` = never terminate on idle).
    pub fn new(
        cpu: C,
        bus: Bus,
        pic: Box<dyn InterruptController>,
        quantum: Option<u64>,
        irq_lines: Vec<IrqLine>,
        idle_limit: Option<u64>,
    ) -> Self {
        let last_levels = vec![None; irq_lines.len()];
        Self {
            cpu,
            bus,
            pic,
            clock: VClock::ZERO,
            quantum,
            irq_lines,
            last_levels,
            last_retired: 0,
            clock_remainder: 0,
            idle_limit,
            dma_wirings: Vec::new(),
        }
    }

    /// Declare the bus-master → DMA-controller couplings this machine services
    /// (§7.7), returning the machine for chaining. A machine with no bus-master
    /// device leaves this empty (the default).
    #[must_use]
    pub fn with_dma_wirings(mut self, wirings: Vec<DmaWiring>) -> Self {
        self.dma_wirings = wirings;
        self
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

    /// The configured idle limit (halted boundaries before quiescence).
    pub fn idle_limit(&self) -> Option<u64> {
        self.idle_limit
    }

    /// Set the idle limit after construction — the "how we run" bound on how many
    /// consecutive halted boundaries pass with no injected interrupt before the
    /// machine declares the source quiescent and stops (`None` never gives up).
    ///
    /// The idle limit is a run concern, not part of what the machine *is*, so the
    /// run-policy layer above owns it and stamps it here before driving the loop.
    /// [`Machine::new`] still accepts an initial value so the pre-policy assembly
    /// paths stay byte-identical; this setter is how the policy relocates it.
    pub fn set_idle_limit(&mut self, idle_limit: Option<u64>) {
        self.idle_limit = idle_limit;
    }

    /// Run until the guest halts with no more interrupts to service (§3.1).
    ///
    /// Unbounded: equivalent to [`Machine::run_bounded`] with no cap, discarding
    /// the [`RunOutcome`] (an uncapped run can only end on [`RunOutcome::Halted`]).
    /// Every existing frontend uses this; the bounded form is for a guest that may
    /// spin without halting.
    pub fn run(&mut self) -> Result<(), MachineError> {
        self.run_bounded(None).map(|_| ())
    }

    /// Run until the guest halts, or until `max_retired` guest instructions have
    /// retired — whichever comes first (§3.1, §6).
    ///
    /// Each iteration runs the CPU for one quantum, services the resulting stop
    /// against the bus, drives the deterministic clock from the retired delta,
    /// ticks the devices, edge-detects the wired IRQ lines onto the PIC, and —
    /// when nothing is already in flight — acknowledges and injects a serviceable
    /// vector. A halted guest is kept alive until the interrupt source falls
    /// quiescent (see the module docs), then the machine stops with
    /// [`RunOutcome::Halted`].
    ///
    /// `max_retired` is a safety net measured in retired instructions (the same
    /// deterministic virtual-time base as the clock, never wall time): a guest
    /// that never halts — an unsupported instruction is not reached, and it keeps
    /// retiring in a loop — would otherwise run forever. When the cap is reached
    /// the machine stops cleanly with [`RunOutcome::RetiredCapReached`], leaving
    /// captured device output intact for the caller to drain. `None` imposes no
    /// cap.
    pub fn run_bounded(&mut self, max_retired: Option<u64>) -> Result<RunOutcome, MachineError> {
        let mut idle_halts: u64 = 0;
        loop {
            let stop = self.cpu.run(self.quantum)?;

            // Drive the clock and interrupt path on every boundary: tick the
            // devices one PIT clock at a time (edge-detecting the wired lines
            // after each), then acknowledge/inject a serviceable vector.
            self.advance_clock();
            let injected = self.service_interrupts();

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
                    // A trapped store leaves the CPU parked on the instruction;
                    // acknowledge it so the retried store skips the trap and the
                    // guest advances (a port `out`, by contrast, self-retires).
                    self.cpu.complete_mmio_write();
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
                    // A halted guest resumes only when an interrupt is delivered.
                    // Keep looping so the timer can fire; give up once the source
                    // is quiescent (no injection for `idle_limit` halted
                    // boundaries), which is how a masked-timer boot terminates.
                    if injected || self.cpu.has_pending_irq() {
                        idle_halts = 0;
                    } else {
                        idle_halts += 1;
                        match self.idle_limit {
                            Some(limit) if idle_halts >= limit => return Ok(RunOutcome::Halted),
                            // No idle limit: a halt with nothing pending and no
                            // way to make progress would spin forever, so stop.
                            None => return Ok(RunOutcome::Halted),
                            _ => {}
                        }
                    }
                }
            }

            // Run any DMA transfer a bus master queued while servicing the stop
            // (a floppy READ programs its transfer, then the 8237 moves the bytes
            // into guest RAM before the guest runs again).
            self.service_dma();

            // Safety net: a spinning guest never reaches a halt or an error, so
            // cap total retired instructions and stop cleanly, preserving output.
            if let Some(cap) = max_retired {
                if self.cpu.retired() >= cap {
                    return Ok(RunOutcome::RetiredCapReached);
                }
            }
        }
    }

    /// Advance the virtual clock from the retired-instruction delta, ticking the
    /// devices **one PIT input clock at a time** and edge-detecting the wired IRQ
    /// lines after each single clock (§6, §5.1).
    ///
    /// One clock at a time is required, not one big jump: a device drives a level
    /// that can pulse for a single clock (the PIT's mode-2 OUT-low pulse). A bulk
    /// `tick_all` across several clocks would collapse the pulse and lose the
    /// edge; sampling after every clock catches the rising edge exactly once per
    /// timer period.
    fn advance_clock(&mut self) {
        let retired = self.cpu.retired();
        let delta = retired.saturating_sub(self.last_retired);
        self.last_retired = retired;

        // Convert retired instructions → PIT input clocks by the fixed ratio,
        // carrying the remainder so no fractional clock is lost across calls. The
        // division/remainder is written for the general ratio even though the
        // default is 1 (one clock per instruction), so changing the constant
        // needs no code change — hence the `modulo_one` allow.
        let total = self.clock_remainder + delta;
        let pit_clocks = total / RETIRED_PER_PIT_CLOCK;
        #[allow(clippy::modulo_one)]
        {
            self.clock_remainder = total % RETIRED_PER_PIT_CLOCK;
        }

        for _ in 0..pit_clocks {
            self.clock = self.clock.advance(1);
            self.bus.tick_all(self.clock);
            self.edge_detect();
        }
    }

    /// Edge-detect the wired device IRQ lines onto the PIC (§5.1).
    ///
    /// A device drives a *level*; the PIC is *edge-triggered*. Pulse the PIC
    /// input on a rising edge (low→high) of the device line — matching real IRQ0
    /// wiring, one PIC request per timer period. The devices deliberately do not
    /// edge-detect; that is this integration layer's job.
    fn edge_detect(&mut self) {
        for (i, line) in self.irq_lines.iter().enumerate() {
            let Some(level) = self.bus.device_irq_line(line.device) else {
                continue;
            };
            let rising = matches!(self.last_levels[i], Some(false)) && level;
            self.last_levels[i] = Some(level);
            if rising {
                // Emit a full high→low pulse on the PIC input, not a sustained
                // high. The PIC latches a request on the rising edge; leaving the
                // input high would defeat its own edge detection, so a later timer
                // period would never latch a second request. The low half restores
                // the input so the next period's rising edge latches again.
                self.pic.set_irq_line(line.irq, true);
                self.pic.set_irq_line(line.irq, false);
            }
        }
    }

    /// Run each wired bus master's pending DMA transfer through its DMA
    /// controller (§7.7), the cross-device path a floppy transfer needs.
    ///
    /// The seam is symmetric. For a device→memory ([`DmaOp::ToMemory`]) request —
    /// what a floppy READ queues — the requester hands over the bytes it read from
    /// its backing image; the machine streams them through the controller's
    /// channel, writing each to guest RAM via the `note_write` path (§4.3), and
    /// reports the byte count back. For a memory→device ([`DmaOp::FromMemory`])
    /// request — what a floppy WRITE queues — the machine reads the requested
    /// length out of guest RAM through the same channel, truncates the buffer to
    /// what the channel actually transferred (terminal count may cut it short, and
    /// the requester must never see uninitialised tail bytes), and hands the bytes
    /// back. Either way the controller advances the channel's address and count,
    /// then the requester finishes its result phase and raises its interrupt.
    ///
    /// This is the only place a device's bytes cross to or from memory without the
    /// device holding a memory reference: the requester never sees an address, the
    /// controller never sees the disk, and the SMC-safe seam is preserved.
    fn service_dma(&mut self) {
        for i in 0..self.dma_wirings.len() {
            let wiring = self.dma_wirings[i];
            let request = self
                .bus
                .device_mut(wiring.requester)
                .and_then(|dev| dev.take_dma_request());
            let Some(request) = request else {
                continue;
            };
            let mut mem = CpuDmaMemory { cpu: &mut self.cpu };
            let outcome = match request.op {
                DmaOp::ToMemory(data) => {
                    let transferred = self
                        .bus
                        .device_mut(wiring.controller)
                        .map(|ctrl| ctrl.dma_service_write(request.channel, &mut mem, &data))
                        .unwrap_or(0);
                    DmaOutcome::ToMemory { transferred }
                }
                DmaOp::FromMemory { len } => {
                    let mut buf = vec![0u8; len];
                    let transferred = self
                        .bus
                        .device_mut(wiring.controller)
                        .map(|ctrl| ctrl.dma_service_read(request.channel, &mut mem, &mut buf))
                        .unwrap_or(0);
                    buf.truncate(transferred);
                    DmaOutcome::FromMemory(buf)
                }
            };
            if let Some(dev) = self.bus.device_mut(wiring.requester) {
                dev.dma_complete(outcome);
            }
        }
    }

    /// Acknowledge-at-inject (§5.1, §5.4): if the CPU has no injected IRQ in
    /// flight and the PIC has a serviceable vector, run INTA
    /// ([`InterruptController::acknowledge`], IRR→ISR) and inject the returned
    /// vector into the CPU. Returns whether a vector was injected this boundary.
    ///
    /// Only one vector is in flight at a time: while [`Cpu::has_pending_irq`] is
    /// set (the CPU has not yet delivered a prior injection) no further request is
    /// acknowledged, so the PIC's ISR and the CPU's queue stay in step and the
    /// guest's EOI (via a bus port write) matches the INTA that set the ISR.
    fn service_interrupts(&mut self) -> bool {
        if !self.cpu.has_pending_irq() {
            if let Some(vector) = self.pic.acknowledge() {
                self.cpu.inject_irq(vector);
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::{DecodeMap, PortRange};
    use device::{Device, DeviceError, DmaRequest, TraceEvent};
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

    /// A no-op controller for the interrupt-free loop tests.
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

    fn machine_with(program: Vec<CpuStop>) -> Machine<MockCpu> {
        Machine::new(
            MockCpu::new(program),
            bus_with_capture(),
            Box::new(NoIc),
            None,
            Vec::new(),
            Some(1),
        )
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
        let mut machine = machine_with(program);
        machine.run().expect("run");

        // Each MockCpu run() retires one instruction and the ratio is 1:1, so
        // three boundaries advance the clock to 3.
        assert_eq!(machine.clock(), VClock(3));
    }

    #[test]
    fn loop_completes_port_in() {
        let program = vec![CpuStop::PortIn {
            port: 0x3F8,
            size: 1,
        }];
        let mut machine = machine_with(program);
        machine.run().expect("run");
        // The capture device returns 0xAB for reads; the machine completes the
        // pending in with it.
        assert_eq!(machine.cpu().completions, vec![0xAB]);
    }

    #[test]
    fn loop_surfaces_exception() {
        let program = vec![CpuStop::Exception { vector: 6 }];
        let mut machine = machine_with(program);
        let err = machine.run().unwrap_err();
        assert!(matches!(err, MachineError::Exception { vector: 6 }));
    }

    #[test]
    fn retired_cap_terminates_a_spinning_guest() {
        // A guest that keeps retiring without ever halting: a long run of
        // budget-exhausted yields (far more than the cap). Without a cap the
        // machine would keep looping; the cap must stop it cleanly at the bound.
        let cap = 5;
        let program = vec![CpuStop::BudgetExhausted; 1000];
        let mut machine = machine_with(program);
        let outcome = machine.run_bounded(Some(cap)).expect("run");
        assert_eq!(outcome, RunOutcome::RetiredCapReached);
        // 1:1 retired-per-run, so the machine stops the first boundary at which
        // the retired count reaches the cap — exactly `cap` instructions in.
        assert_eq!(machine.clock(), VClock(cap));
    }

    #[test]
    fn retired_instruction_clock_advances_by_delta() {
        // A CPU that retires three instructions per run() must advance the clock
        // by three PIT clocks per boundary (1:1 ratio).
        let program = vec![CpuStop::BudgetExhausted, CpuStop::Halt];
        let cpu = MockCpu::new(program).with_retired_per_run(3);
        let mut machine = Machine::new(
            cpu,
            bus_with_capture(),
            Box::new(NoIc),
            None,
            Vec::new(),
            Some(1),
        );
        machine.run().expect("run");
        // Two boundaries × 3 retired = 6 PIT clocks.
        assert_eq!(machine.clock(), VClock(6));
    }

    /// A device with a scriptable IRQ line and EOI observation, standing in for
    /// the PIT/PIC pairing to test the wiring in isolation.
    #[derive(Default)]
    struct LineDevice {
        level: bool,
    }
    impl Device for LineDevice {
        fn name(&self) -> &'static str {
            "line"
        }
        fn port_read(&mut self, _port: u16, _size: u8) -> Result<u32, DeviceError> {
            Ok(0)
        }
        fn port_write(&mut self, _port: u16, _size: u8, _value: u32) -> Result<(), DeviceError> {
            Ok(())
        }
        fn mmio_read(&mut self, _addr: u64, _size: u8) -> Result<u64, DeviceError> {
            Ok(0)
        }
        fn mmio_write(&mut self, _addr: u64, _size: u8, _value: u64) -> Result<(), DeviceError> {
            Ok(())
        }
        fn tick(&mut self, now: VClock) {
            // Drive the line low for one clock every 4 ticks, else high — a
            // mode-2-like OUT so the rising edge recurs once per period.
            self.level = now.0 % 4 != 0;
        }
        fn irq_pending(&self) -> bool {
            self.level
        }
        fn irq_vector(&self) -> Option<u8> {
            None
        }
        fn reset(&mut self) {}
        fn save(&self) -> Vec<u8> {
            Vec::new()
        }
        fn restore(&mut self, _s: &[u8]) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    /// A counting interrupt controller: records set_irq_line pulses and hands out
    /// one serviceable vector per pulse, cleared by acknowledge.
    #[derive(Default)]
    struct CountingIc {
        pulses: u32,
        pending: u32,
    }
    impl InterruptController for CountingIc {
        fn set_irq_line(&mut self, _irq: u8, level: bool) {
            if level {
                self.pulses += 1;
                self.pending += 1;
            }
        }
        fn poll(&self) -> Option<u8> {
            (self.pending > 0).then_some(0x08)
        }
        fn acknowledge(&mut self) -> Option<u8> {
            if self.pending > 0 {
                self.pending -= 1;
                Some(0x08)
            } else {
                None
            }
        }
        fn end_of_interrupt(&mut self) {}
    }

    #[test]
    fn edge_detect_produces_one_request_per_period() {
        // Drive the machine so the line device is ticked across several periods
        // and assert exactly one PIC pulse per period. The line goes low at
        // ticks 4, 8, 12, … and high in between, so a rising edge occurs once per
        // 4-tick period.
        let program = vec![CpuStop::BudgetExhausted; 20];
        // Each run() retires one instruction (1 tick). We need the LineDevice at
        // a known bus index and a CountingIc to observe pulses. Rebuild a bus.
        let devices: Vec<Box<dyn Device>> = vec![Box::new(LineDevice::default())];
        let map = DecodeMap {
            ports: Vec::new(),
            mmio: Vec::new(),
        };
        let bus = Bus::new(devices, map);
        let mut machine = Machine::new(
            MockCpu::new(program),
            bus,
            Box::<CountingIc>::default(),
            None,
            vec![IrqLine { device: 0, irq: 0 }],
            Some(1),
        );
        machine.run().expect("run");
        // The line device is low at ticks 0, 4, 8, … and high in between, so a
        // rising edge (one PIC request per 4-tick period) occurs at ticks 5, 9,
        // 13, 17, 21. The final `Halt` boundary advances the clock to 21, giving
        // five rising edges → five injected vectors. We can't read the boxed IC's
        // private counter, so assert the CPU received the matching injections.
        assert_eq!(machine.cpu().injected.len(), 5);
        assert!(machine.cpu().injected.iter().all(|&v| v == 0x08));
    }

    /// A minimal CPU backend with a flat guest-memory buffer, so the DMA seam can
    /// be exercised end to end without x86jit. The DMA path reads and writes guest
    /// RAM through the `guest_dma_*` methods, which back onto this buffer.
    struct MemCpu {
        mem: Vec<u8>,
    }

    impl MemCpu {
        fn new(size: usize) -> Self {
            Self { mem: vec![0; size] }
        }

        fn seed(&mut self, addr: u32, bytes: &[u8]) {
            let at = addr as usize;
            self.mem[at..at + bytes.len()].copy_from_slice(bytes);
        }
    }

    impl Cpu for MemCpu {
        fn run(&mut self, _budget: Option<u64>) -> Result<CpuStop, CpuError> {
            Ok(CpuStop::Halt)
        }
        fn reg(&self, _reg: cpu::CpuReg) -> u64 {
            0
        }
        fn set_reg(&mut self, _reg: cpu::CpuReg, _value: u64) {}
        fn complete_port_in(&mut self, _value: u64) {}
        fn complete_mmio_read(&mut self, _value: u64) {}
        fn complete_mmio_write(&mut self) {}
        fn guest_dma_read(&self, addr: u32) -> u8 {
            self.mem[addr as usize]
        }
        fn guest_dma_write(&mut self, addr: u32, value: u8) {
            self.mem[addr as usize] = value;
        }
        fn retired(&self) -> u64 {
            0
        }
    }

    /// A bus master that queues a single memory→device transfer and captures the
    /// outcome the machine hands back, so a test can assert the seam delivered the
    /// right bytes without inspecting a concrete controller.
    #[derive(Default)]
    struct FromMemoryRequester {
        pending: Option<DmaRequest>,
        received: Option<DmaOutcome>,
    }

    impl Device for FromMemoryRequester {
        fn name(&self) -> &'static str {
            "from-memory-requester"
        }
        fn port_read(&mut self, _port: u16, _size: u8) -> Result<u32, DeviceError> {
            Ok(0)
        }
        fn port_write(&mut self, _port: u16, _size: u8, _value: u32) -> Result<(), DeviceError> {
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
        fn tick(&mut self, _now: VClock) {}
        fn irq_pending(&self) -> bool {
            false
        }
        fn irq_vector(&self) -> Option<u8> {
            None
        }
        fn reset(&mut self) {}
        fn save(&self) -> Vec<u8> {
            Vec::new()
        }
        fn restore(&mut self, _snapshot: &[u8]) -> Result<(), DeviceError> {
            Ok(())
        }
        fn take_dma_request(&mut self) -> Option<DmaRequest> {
            self.pending.take()
        }
        fn dma_complete(&mut self, outcome: DmaOutcome) {
            self.received = Some(outcome);
        }
        fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
            Some(self)
        }
    }

    /// DMA channel the mock requester runs on (the PC floppy channel).
    const READ_CHANNEL: u8 = 2;

    /// Build a real 8237 programmed for a memory→device (read) transfer on
    /// channel 2 at physical `address`, transferring `count + 1` bytes.
    fn programmed_read_dma(address: u16, count: u16) -> devices::Dma8237 {
        let mut dma = devices::Dma8237::new();
        dma.port_write(0x0A, 1, 0x06).unwrap(); // mask channel 2
        dma.port_write(0x0C, 1, 0).unwrap(); // clear flip-flop
        dma.port_write(0x0B, 1, 0x4A).unwrap(); // mode: ch2, read, single
        dma.port_write(0x04, 1, u32::from(address & 0xFF)).unwrap(); // addr low
        dma.port_write(0x04, 1, u32::from(address >> 8)).unwrap(); // addr high
        dma.port_write(0x81, 1, 0x00).unwrap(); // page latch (channel 2)
        dma.port_write(0x05, 1, u32::from(count & 0xFF)).unwrap(); // count low
        dma.port_write(0x05, 1, u32::from(count >> 8)).unwrap(); // count high
        dma.port_write(0x0A, 1, 0x02).unwrap(); // unmask channel 2
        dma
    }

    /// Assemble a machine whose bus holds the mock requester (index 0) and the
    /// programmed 8237 (index 1), wired together, over `cpu`'s guest memory.
    fn dma_machine(
        cpu: MemCpu,
        requester: FromMemoryRequester,
        dma: devices::Dma8237,
    ) -> Machine<MemCpu> {
        let devices: Vec<Box<dyn Device>> = vec![Box::new(requester), Box::new(dma)];
        let map = DecodeMap {
            ports: Vec::new(),
            mmio: Vec::new(),
        };
        Machine::new(
            cpu,
            Bus::new(devices, map),
            Box::new(NoIc),
            None,
            Vec::new(),
            Some(1),
        )
        .with_dma_wirings(vec![DmaWiring {
            requester: 0,
            controller: 1,
        }])
    }

    /// Read the outcome the requester captured after a serviced transfer.
    fn captured_outcome(machine: &mut Machine<MemCpu>) -> DmaOutcome {
        machine
            .bus
            .device_mut(0)
            .and_then(|dev| dev.as_any_mut())
            .and_then(|any| any.downcast_mut::<FromMemoryRequester>())
            .and_then(|req| req.received.take())
            .expect("the requester received a completion")
    }

    #[test]
    fn service_dma_from_memory_delivers_read_bytes() {
        const ADDRESS: u16 = 0x0010;
        let pattern: Vec<u8> = (0..8u8).map(|i| 0xB0 ^ i).collect();

        let mut cpu = MemCpu::new(0x1000);
        cpu.seed(u32::from(ADDRESS), &pattern);

        // count+1 == pattern.len() so the whole request transfers.
        let dma = programmed_read_dma(ADDRESS, (pattern.len() - 1) as u16);
        let requester = FromMemoryRequester {
            pending: Some(DmaRequest {
                channel: READ_CHANNEL,
                op: DmaOp::FromMemory { len: pattern.len() },
            }),
            received: None,
        };

        let mut machine = dma_machine(cpu, requester, dma);
        machine.service_dma();

        match captured_outcome(&mut machine) {
            DmaOutcome::FromMemory(bytes) => {
                assert_eq!(
                    bytes, pattern,
                    "the requester gets exactly the seeded bytes"
                );
            }
            other => panic!("expected a FromMemory outcome, got {other:?}"),
        }
    }

    #[test]
    fn service_dma_from_memory_truncates_at_terminal_count() {
        const ADDRESS: u16 = 0x0010;
        let pattern: Vec<u8> = (0..8u8).map(|i| 0xC0 ^ i).collect();
        // The requester asks for 8 bytes but the channel is programmed for only 4
        // (count 3 -> 4 bytes), so terminal count cuts the transfer short.
        const REQUESTED: usize = 8;
        const AVAILABLE: usize = 4;

        let mut cpu = MemCpu::new(0x1000);
        cpu.seed(u32::from(ADDRESS), &pattern);

        let dma = programmed_read_dma(ADDRESS, (AVAILABLE - 1) as u16);
        let requester = FromMemoryRequester {
            pending: Some(DmaRequest {
                channel: READ_CHANNEL,
                op: DmaOp::FromMemory { len: REQUESTED },
            }),
            received: None,
        };

        let mut machine = dma_machine(cpu, requester, dma);
        machine.service_dma();

        match captured_outcome(&mut machine) {
            DmaOutcome::FromMemory(bytes) => {
                assert_eq!(
                    bytes,
                    pattern[..AVAILABLE],
                    "the buffer is truncated to what the channel transferred; no uninitialised tail"
                );
            }
            other => panic!("expected a FromMemory outcome, got {other:?}"),
        }
    }
}
