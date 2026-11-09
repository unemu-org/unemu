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
//! instructions. While HLT suspends execution, each loop iteration advances one
//! PIT input clock without retiring instructions; the retired count and its
//! fractional clock remainder are preserved across the halt. [`Machine::virtual_time`]
//! includes both retired instructions and the instruction-equivalent halted clocks,
//! so a safety cap bounds a halted guest just as it bounds a busy-polling one.
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
//! - **HLT wakeup.** On [`CpuStop::Halt`] the machine suspends CPU execution
//!   until an injected vector is pending and guest IF is set. The next CPU run
//!   delivers the interrupt; IRET returns to the instruction after HLT (Intel
//!   80286 Programmer's Reference Manual, HLT instruction description). With IF
//!   clear it stops immediately, since NMI is not modelled. With IF set it ticks
//!   devices one PIT clock at a time, stopping after [`Machine::idle_limit`]
//!   consecutive idle clocks. `None` waits no clocks: only an immediately
//!   deliverable interrupt resumes execution. The idle count resets on wake-up.

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
/// [`RunOutcome::RetiredCapReached`] means a total virtual-time cap was
/// hit first: a safety net so a guest that spins or repeatedly wakes from HLT
/// cannot run forever. Reaching the cap is a clean stop,
/// not an error, so any output captured up to that point is preserved.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// The guest halted with no more interrupts to service (the natural end).
    Halted,
    /// The total virtual-time cap was reached before a natural stop.
    RetiredCapReached,
}

/// A running machine: a CPU seam, a bus seam, and an interrupt controller (§3.1).
pub struct Machine<C: Cpu> {
    cpu: C,
    bus: Bus,
    pic: Box<dyn InterruptController>,
    clock: VClock,
    quantum: Option<u64>,
    /// HLT remains suspended across runner calls until an IRQ can wake the CPU.
    halted: bool,
    /// Total PIT clocks spent halted, charged to the virtual-time safety cap.
    halted_clocks: u64,
    /// Consecutive halted PIT clocks since the CPU last ran or woke.
    idle_clocks: u64,
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
    /// Consecutive halted PIT clocks with no interrupt injected before stopping.
    /// `None` waits no clocks; only an immediately deliverable IRQ wakes HLT.
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
    /// declaring quiescence, in PIT clocks (`None` = no idle wait).
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
            halted: false,
            halted_clocks: 0,
            idle_clocks: 0,
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

    /// The current device clock, in PIT input clocks (§6).
    pub fn clock(&self) -> VClock {
        self.clock
    }

    /// Total virtual time in instruction-equivalent units (§6).
    ///
    /// Retired instructions plus halted PIT clocks converted by
    /// [`RETIRED_PER_PIT_CLOCK`]. Unlike [`Cpu::retired`], this advances during
    /// HLT too, so run caps and scheduled targets bound both busy and idle time.
    pub fn virtual_time(&self) -> u64 {
        self.cpu
            .retired()
            .saturating_add(self.halted_clocks.saturating_mul(RETIRED_PER_PIT_CLOCK))
    }

    /// The configured idle limit (halted PIT clocks before quiescence).
    pub fn idle_limit(&self) -> Option<u64> {
        self.idle_limit
    }

    /// Set the idle limit after construction — the "how we run" bound on how many
    /// consecutive halted PIT clocks pass with no injected interrupt before the
    /// machine stops (`None` = no idle wait). The count resets on each wake-up.
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

    /// Run until the guest halts, or until the absolute `max_retired` virtual-time
    /// cap is reached — whichever comes first (§3.1, §6).
    ///
    /// Each iteration runs the CPU for one quantum, services the resulting stop
    /// against the bus, drives the deterministic clock from the retired delta,
    /// ticks the devices, edge-detects the wired IRQ lines onto the PIC, and —
    /// when nothing is already in flight — acknowledges and injects a serviceable
    /// vector. A halted guest is kept alive until the interrupt source falls
    /// quiescent (see the module docs), then the machine stops with
    /// [`RunOutcome::Halted`].
    ///
    /// `max_retired` bounds [`Machine::virtual_time`]: retired instructions plus
    /// the instruction-equivalent of every halted PIT clock, never wall time.
    /// The cap is checked after each CPU quantum and each halted clock; a CPU
    /// quantum may overshoot it. Reaching it returns
    /// [`RunOutcome::RetiredCapReached`], preserving captured device output.
    /// `None` imposes no cap.
    pub fn run_bounded(&mut self, max_retired: Option<u64>) -> Result<RunOutcome, MachineError> {
        loop {
            if max_retired.is_some_and(|cap| self.virtual_time() >= cap) {
                return Ok(RunOutcome::RetiredCapReached);
            }
            if self.halted {
                // NMI is not modelled, so nothing can wake HLT with IF clear.
                if !self.cpu.interrupts_enabled() {
                    return Ok(RunOutcome::Halted);
                }
                self.service_interrupts();
                if self.cpu.has_pending_irq() {
                    self.halted = false;
                    self.idle_clocks = 0;
                } else {
                    match self.idle_limit {
                        None => return Ok(RunOutcome::Halted),
                        Some(limit) if self.idle_clocks >= limit => return Ok(RunOutcome::Halted),
                        _ => {}
                    }
                    // Halted time is independent of retired instructions. Leave
                    // last_retired and the fractional instruction clock untouched
                    // so the next CPU run accounts only for its own instructions.
                    self.tick_clock();
                    self.halted_clocks += 1;
                    self.service_interrupts();
                    self.service_dma();
                    if !self.cpu.has_pending_irq() {
                        self.idle_clocks += 1;
                    }
                    if max_retired.is_some_and(|cap| self.virtual_time() >= cap) {
                        return Ok(RunOutcome::RetiredCapReached);
                    }
                    continue;
                }
            }
            self.idle_clocks = 0;
            let stop = self.cpu.run(self.quantum)?;

            // Drive the clock and interrupt path on every boundary: tick the
            // devices one PIT clock at a time (edge-detecting the wired lines
            // after each), then acknowledge/inject a serviceable vector.
            self.advance_clock();
            self.service_interrupts();

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
                    self.halted = true;
                    if !self.cpu.interrupts_enabled()
                        || (!self.cpu.has_pending_irq()
                            && matches!(self.idle_limit, None | Some(0)))
                    {
                        return Ok(RunOutcome::Halted);
                    }
                }
            }

            // Run any DMA transfer a bus master queued while servicing the stop
            // (a floppy READ programs its transfer, then the 8237 moves the bytes
            // into guest RAM before the guest runs again).
            self.service_dma();
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
            self.tick_clock();
        }
    }

    fn tick_clock(&mut self) {
        self.clock = self.clock.advance(1);
        self.bus.tick_all(self.clock);
        self.edge_detect();
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
        stop_after: Option<u64>,
        period: Option<u64>,
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
            // Drive the line low for one clock every period, else high — a
            // mode-2-like OUT so the rising edge recurs once per period.
            self.level = !now.0.is_multiple_of(self.period.unwrap_or(4))
                && self.stop_after.is_none_or(|end| now.0 <= end);
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
        acknowledgements: std::rc::Rc<std::cell::Cell<u64>>,
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
            self.acknowledgements.set(self.acknowledgements.get() + 1);
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
            MockCpu::new(program).with_interrupts_enabled(true),
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

    fn halted_machine(program: Vec<CpuStop>, limit: Option<u64>) -> Machine<MockCpu> {
        let bus = Bus::new(
            vec![
                Box::new(LineDevice {
                    level: false,
                    stop_after: Some(9),
                    period: None,
                }),
                Box::<CaptureDevice>::default(),
            ],
            DecodeMap {
                ports: vec![PortRange {
                    ports: 0x3F8..=0x3FF,
                    device: 1,
                }],
                mmio: Vec::new(),
            },
        );
        Machine::new(
            MockCpu::new(program).with_interrupts_enabled(true),
            bus,
            Box::<CountingIc>::default(),
            None,
            vec![IrqLine { device: 0, irq: 0 }],
            limit,
        )
    }

    #[test]
    fn halt_waits_without_running_cpu_until_irq_delivery() {
        let mut machine = halted_machine(
            vec![
                CpuStop::Halt,
                CpuStop::PortOut {
                    port: 0x3F8,
                    size: 1,
                    value: b'P' as u64,
                },
            ],
            Some(4),
        );
        assert_eq!(
            machine.run_bounded(Some(6)).expect("run"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.cpu().retired(), 2);
        assert_eq!(machine.clock(), VClock(6));
        assert_eq!(machine.cpu().delivered, vec![0x08]);
        assert_eq!(machine.bus().device(1).expect("capture").save(), b"P");
    }

    #[test]
    fn halt_with_if_clear_stops_without_retiring_post_halt_code() {
        let mut machine = machine_with(vec![CpuStop::Halt, CpuStop::BudgetExhausted]);
        machine.set_idle_limit(Some(100));
        assert_eq!(
            machine.run_bounded(Some(2)).expect("run"),
            RunOutcome::Halted
        );
        assert_eq!(machine.cpu().retired(), 1);
        assert_eq!(machine.clock(), VClock(1));
        // Re-entering the runner must preserve HLT too.
        assert_eq!(
            machine.run_bounded(Some(2)).expect("rerun"),
            RunOutcome::Halted
        );
        assert_eq!(machine.cpu().retired(), 1);
    }

    #[test]
    fn halt_with_pending_irq_and_if_clear_stops_immediately() {
        let mut machine = machine_with(vec![CpuStop::Halt, CpuStop::BudgetExhausted]);
        machine.cpu.inject_irq(0x08);
        machine.set_idle_limit(Some(100));
        assert_eq!(
            machine.run_bounded(Some(2)).expect("run"),
            RunOutcome::Halted
        );
        assert_eq!(machine.cpu().retired(), 1);
        assert!(machine.cpu().has_pending_irq());
        assert!(machine.cpu().delivered.is_empty());
    }

    #[test]
    fn halt_idle_limit_counts_exactly_idle_pit_clocks() {
        for limit in [0, 1, 3] {
            let mut machine = halted_machine(vec![CpuStop::Halt], Some(limit));
            assert_eq!(
                machine.run_bounded(Some(20)).expect("run"),
                RunOutcome::Halted
            );
            assert_eq!(machine.cpu().retired(), 1);
            assert_eq!(machine.clock(), VClock(1 + limit));
            assert!(machine.cpu().injected.is_empty());
        }
    }

    #[test]
    fn halt_idle_counter_resets_after_each_wake() {
        let mut machine = halted_machine(
            vec![
                CpuStop::Halt,
                CpuStop::BudgetExhausted,
                CpuStop::Halt,
                CpuStop::PortOut {
                    port: 0x3F8,
                    size: 1,
                    value: b'P' as u64,
                },
                CpuStop::Halt,
            ],
            Some(4),
        );
        assert_eq!(
            machine.run_bounded(Some(20)).expect("run"),
            RunOutcome::Halted
        );
        assert_eq!(machine.cpu().retired(), 5);
        assert_eq!(machine.clock(), VClock(15));
        assert_eq!(machine.cpu().delivered, vec![0x08, 0x08]);
    }

    #[test]
    fn halt_without_idle_wait_only_resumes_for_an_immediate_irq() {
        let mut machine = halted_machine(vec![CpuStop::Halt, CpuStop::BudgetExhausted], None);
        assert_eq!(
            machine.run_bounded(Some(20)).expect("run"),
            RunOutcome::Halted
        );
        assert_eq!(machine.clock(), VClock(1));
        assert_eq!(
            machine.run_bounded(Some(2)).expect("still halted"),
            RunOutcome::Halted
        );
        assert_eq!(machine.cpu().retired(), 1);
        machine.cpu.inject_irq(0x08);
        assert_eq!(
            machine.run_bounded(Some(2)).expect("wake"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.cpu().delivered, vec![0x08]);
    }

    #[test]
    fn virtual_time_cap_bounds_a_periodic_halt_loop() {
        let bus = Bus::new(
            vec![Box::new(LineDevice {
                period: Some(65_536),
                ..LineDevice::default()
            })],
            DecodeMap::default(),
        );
        let mut machine = Machine::new(
            MockCpu::new(vec![CpuStop::Halt; 1000]).with_interrupts_enabled(true),
            bus,
            Box::<CountingIc>::default(),
            None,
            vec![IrqLine { device: 0, irq: 0 }],
            Some(100_000),
        );
        let cap = 1000;
        assert_eq!(
            machine.run_bounded(Some(cap)).expect("run"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.clock(), VClock(cap));
        assert_eq!(machine.virtual_time(), cap);
        assert_eq!(machine.cpu().retired(), 1);
        assert!(machine.cpu().delivered.is_empty());
    }

    #[test]
    fn halt_idle_counter_persists_across_runner_calls() {
        let mut machine = halted_machine(vec![CpuStop::Halt], Some(3));
        for _ in 0..2 {
            assert_eq!(machine.run_bounded(None).expect("run"), RunOutcome::Halted);
            assert_eq!(machine.clock(), VClock(4));
            assert_eq!(machine.cpu().retired(), 1);
        }
    }

    #[test]
    fn halted_cap_preserves_idle_count_across_runner_calls() {
        let mut machine = halted_machine(vec![CpuStop::Halt], Some(3));
        assert_eq!(
            machine.run_bounded(Some(3)).expect("first chunk"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 3);
        assert_eq!(machine.cpu().retired(), 1);
        assert_eq!(
            machine.run_bounded(Some(3)).expect("same cap"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 3);
        assert_eq!(
            machine.run_bounded(Some(20)).expect("rest"),
            RunOutcome::Halted
        );
        assert_eq!(machine.virtual_time(), 4);
        assert_eq!(machine.cpu().retired(), 1);
    }

    #[test]
    fn halt_delivers_vector_pending_at_halt_without_idle_wait() {
        for limit in [None, Some(0)] {
            let mut machine = halted_machine(vec![CpuStop::Halt, CpuStop::BudgetExhausted], limit);
            // The first run reaches the line's rising edge while returning HLT,
            // so the CPU has not yet had a chance to consume the injected vector.
            machine.cpu = machine.cpu.with_retired_per_run(5);
            assert_eq!(
                machine.run_bounded(Some(10)).expect("run"),
                RunOutcome::RetiredCapReached
            );
            assert_eq!(machine.cpu().delivered, vec![0x08]);
            assert_eq!(machine.cpu().retired(), 10);
        }
    }

    #[test]
    fn halted_reentry_with_if_clear_neither_ticks_nor_acknowledges() {
        for pending in [false, true] {
            let controller = CountingIc::default();
            let acknowledgements = controller.acknowledgements.clone();
            let mut machine =
                halted_machine(vec![CpuStop::Halt, CpuStop::BudgetExhausted], Some(100));
            machine.pic = Box::new(controller);
            machine.cpu = machine.cpu.with_interrupts_enabled(false);
            assert_eq!(
                machine.run_bounded(Some(20)).expect("run"),
                RunOutcome::Halted
            );
            if pending {
                machine.pic.set_irq_line(0, true);
            }
            let before = acknowledgements.get();
            assert_eq!(
                machine.run_bounded(Some(20)).expect("rerun"),
                RunOutcome::Halted
            );
            assert_eq!(machine.clock(), VClock(1));
            assert_eq!(machine.cpu().retired(), 1);
            assert_eq!(acknowledgements.get(), before);
            assert!(machine.cpu().injected.is_empty());
        }
    }

    /// Intel 80286 Programmer's Reference Manual, STI and HLT instruction
    /// descriptions: STI defers the pending interrupt until after HLT (§5.2).
    #[test]
    fn real_mode_sti_halt_delivers_preinjected_vector_without_idle_wait() {
        use cpu::x86jit_backend::{RealSegments, X86JitCpu};
        let mut cpu =
            X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).expect("cpu");
        cpu.map_ram(0, 0x10000).expect("ram");
        // Use DX for the capture port, since an immediate port is only 8 bits.
        cpu.write_mem(0x7C00, &[0xFB, 0xF4, 0xB0, b'P', 0xEE, 0xFA, 0xF4])
            .expect("program");
        cpu.set_reg(cpu::CpuReg::Rdx, 0x3F8);
        cpu.write_mem(0x8000, &[0xB0, b'I', 0xEE, 0xCF])
            .expect("handler");
        cpu.write_mem(0x20, &[0, 0x80, 0, 0]).expect("IVT vector 8");
        cpu.inject_irq(0x08);
        assert!(!cpu.interrupts_enabled());
        let mut machine = Machine::new(
            cpu,
            bus_with_capture(),
            Box::new(NoIc),
            Some(16),
            Vec::new(),
            None,
        );
        assert_eq!(
            machine.run_bounded(Some(100)).expect("run"),
            RunOutcome::Halted
        );
        assert_eq!(machine.bus().device(0).expect("capture").save(), b"IP");
        assert!(!machine.cpu().has_pending_irq());
    }

    /// Intel 80286 Programmer's Reference Manual, HLT: the interrupt handler
    /// must execute before the instruction following HLT. Intel 8254 datasheet,
    /// Mode 2: counter 0 produces the rising edge that wakes the processor (§5).
    #[test]
    fn real_mode_halt_takes_pit_irq_before_post_halt_out() {
        use cpu::x86jit_backend::{RealSegments, X86JitCpu};
        let mut cpu =
            X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).expect("cpu");
        cpu.map_ram(0, 0x10000).expect("ram");
        // CLI; program PIT mode 2, divisor 64; STI; HLT; OUT marker P; CLI; HLT.
        cpu.write_mem(
            0x7C00,
            &[
                0xFA, 0xB0, 0x34, 0xE6, 0x43, 0xB0, 64, 0xE6, 0x40, 0xB0, 0, 0xE6, 0x40, 0xFB,
                0xF4, 0xB0, b'P', 0xE6, 0xE9, 0xFA, 0xF4,
            ],
        )
        .expect("program");
        cpu.write_mem(0x8000, &[0xB0, b'I', 0xE6, 0xE9, 0xCF])
            .expect("handler");
        cpu.write_mem(0x20, &[0, 0x80, 0, 0]).expect("IVT vector 8");
        let bus = Bus::new(
            vec![
                Box::new(devices::Pit8253::new()),
                Box::<CaptureDevice>::default(),
            ],
            DecodeMap {
                ports: vec![
                    PortRange {
                        ports: 0x40..=0x43,
                        device: 0,
                    },
                    PortRange {
                        ports: 0xE9..=0xE9,
                        device: 1,
                    },
                ],
                mmio: Vec::new(),
            },
        );
        let mut machine = Machine::new(
            cpu,
            bus,
            Box::<CountingIc>::default(),
            Some(16),
            vec![IrqLine { device: 0, irq: 0 }],
            Some(128),
        );
        let outcome = machine.run_bounded(Some(1000)).expect("run");
        assert_eq!(machine.bus().device(1).expect("capture").save(), b"IP");
        assert_eq!(outcome, RunOutcome::Halted);
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
        fn interrupts_enabled(&self) -> bool {
            false
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
