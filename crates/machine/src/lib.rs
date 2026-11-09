// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine composition root (§3.1).
//!
//! [`Machine`] owns a [`Cpu`], a [`Bus`], and an [`InterruptController`] and runs
//! the fetch/dispatch/tick/interrupt loop. It is the composition root: **nothing
//! here names a concrete device, chipset, or CPU backend** — it sees only the
//! [`Cpu`], [`Bus`], and [`InterruptController`] seams (§3.2). A frontend (`cli`)
//! builds the concrete parts and hands them in.
//!
//! ## The loop (§3.1, scheduler design §Interrupt acceptance)
//!
//! Only Real16 step-bounded CPUs are assembled. CPU work stops at the next
//! device deadline, policy cap, or cooperative step limit. One completed
//! instruction is a step; each REP iteration is also a step (Intel 80286
//! Programmer's Reference Manual, String Instructions and §3.11.2).
//!
//! Port/MMIO accesses use instruction-start time. Read/write completion and
//! immediate DMA settle before PIC acknowledgement. INTA occurs only when the
//! CPU reports IF set, no STI/SS shadow, and no pending instruction transaction
//! (Intel 8259A datasheet, Interrupt Sequence; Intel 80286 PRM, §9.2).
//!
//! The production deadline engine visits every observable wired transition;
//! `reference-scheduler` exposes a per-clock test oracle with identical CPU/IRQ
//! semantics. IRQ levels are edge-detected onto PIC inputs in timestamp order.
//!
//! ## Virtual clock (§6)
//!
//! Completed steps convert to whole PIT input clocks at a configured rational
//! rate, with carried remainder. While HLT suspends execution, time jumps to the
//! next event, idle bound, or cap, without retiring instructions or consuming
//! the running remainder. IF-clear HLT stops immediately because NMI is absent.
//! Virtual-time caps include accumulated halted instruction equivalents; active
//! caps are exact and halted caps round up to a whole PIT clock. No wall time
//! enters execution. See `backlog/docs/design/scheduler.md` §Adoption.

use std::num::NonZeroU64;

use bus::{Bus, BusError};
use cpu::{Cpu, CpuError, CpuStepStop, CpuStop};
use device::{DmaMemory, DmaOp, DmaOutcome, InterruptController, NextEvent, VClock};
use thiserror::Error;

/// Default completed guest steps per PIT input clock (§6).
///
/// The default CPU rate is `1/1`: one PIT clock per instruction or REP iteration.
/// This is a modelling approximation, not the real ~1.193182 MHz OSC/12
/// timebase. Non-default rates are set with [`Machine::with_cpu_rate`].
pub const RETIRED_PER_PIT_CLOCK: u64 = 1;

/// Device dispatch policy under the same CPU/IRQ loop (§6).
///
/// Production uses deadlines. The non-default `reference-scheduler` feature
/// exposes a per-clock oracle for integration tests, with the same CPU/IRQ rules.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SchedulerEngine {
    /// Tick every device at every whole PIT clock (the reference behavior).
    #[cfg(any(test, feature = "reference-scheduler"))]
    PerClock,
    /// Tick unmigrated devices every clock and scheduled devices only when due.
    #[default]
    Deadline,
}

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

/// Bridges DMA memory cycles to the CPU map (§4.3), keeping RAM writes on the
/// `note_write` path and queuing trapped bytes for bus dispatch. The controller
/// borrows the bus during a transfer, so MMIO is serviced after that borrow ends,
/// before the requester receives completion or any guest execution resumes.
struct CpuDmaMemory<'a, C: Cpu> {
    cpu: &'a mut C,
    read_offset: std::cell::Cell<usize>,
    mmio_reads: std::cell::RefCell<Vec<(usize, u32)>>,
    mmio_writes: Vec<(u32, u8)>,
}

impl<C: Cpu> DmaMemory for CpuDmaMemory<'_, C> {
    fn dma_read(&self, addr: u32) -> u8 {
        let offset = self.read_offset.get();
        self.read_offset.set(offset + 1);
        match self.cpu.guest_dma_read(addr) {
            cpu::DmaAccess::Complete(value) => value,
            cpu::DmaAccess::Mmio => {
                self.mmio_reads.borrow_mut().push((offset, addr));
                0xFF
            }
        }
    }

    fn dma_write(&mut self, addr: u32, value: u8) {
        if self.cpu.guest_dma_write(addr, value) == cpu::DmaAccess::Mmio {
            self.mmio_writes.push((addr, value));
        }
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
    /// A prompt was reached too late to grant the entire scripted command run.
    #[error("insufficient command budget: need {required} steps, only {remaining} remain")]
    InsufficientCommandBudget {
        /// Requested post-prompt steps.
        required: u64,
        /// Steps remaining before the overall cap.
        remaining: u64,
    },
    /// The guest raised a CPU exception the machine does not handle. In real
    /// mode most vectors deliver in-guest through the IVT; this surfaces only an
    /// exception the CPU core chose to escape.
    #[error("unhandled guest exception, vector {vector:#x}")]
    Exception {
        /// x86 exception vector.
        vector: u8,
    },
}

fn scheduler_error(what: &str) -> MachineError {
    CpuError::UnexpectedExit {
        what: format!("scheduler: {what}"),
    }
    .into()
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
    engine: SchedulerEngine,
    completed_steps: u64,
    step_limit: NonZeroU64,
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
    /// CPU rate A/B in completed steps per PIT input clock.
    cpu_rate_numerator: NonZeroU64,
    cpu_rate_denominator: NonZeroU64,
    /// Fractional PIT clocks not yet emitted, in units of 1/A.
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
    /// `irq_lines` declares which device lines feed which PIC inputs (§5.1);
    /// empty for a machine with no interrupts. `idle_limit` bounds how long the
    /// machine keeps a halted guest alive waiting for an interrupt before
    /// declaring quiescence, in PIT clocks (`None` = no idle wait).
    pub fn new(
        cpu: C,
        bus: Bus,
        pic: Box<dyn InterruptController>,
        irq_lines: Vec<IrqLine>,
        idle_limit: Option<u64>,
    ) -> Self {
        let last_levels = vec![None; irq_lines.len()];
        Self {
            cpu,
            bus,
            pic,
            clock: VClock::ZERO,
            engine: SchedulerEngine::default(),
            completed_steps: 0,
            step_limit: NonZeroU64::new(65_536).unwrap_or(NonZeroU64::MIN),
            halted: false,
            halted_clocks: 0,
            idle_clocks: 0,
            irq_lines,
            last_levels,
            cpu_rate_numerator: NonZeroU64::MIN,
            cpu_rate_denominator: NonZeroU64::MIN,
            clock_remainder: 0,
            idle_limit,
            dma_wirings: Vec::new(),
        }
    }

    /// Configure the construction-time CPU rate A/B (§6).
    ///
    /// `numerator` is A and `denominator` is B, in completed steps per PIT
    /// input clock. Both must be positive; the default is `1/1`. This is a
    /// modelling ratio, not a measured CPU frequency. Configure it before the
    /// first run; changing rate after time has elapsed has no defined meaning.
    #[must_use]
    pub fn with_cpu_rate(mut self, numerator: NonZeroU64, denominator: NonZeroU64) -> Self {
        self.cpu_rate_numerator = numerator;
        self.cpu_rate_denominator = denominator;
        self
    }

    /// Select device timer dispatch before running the machine (§6).
    #[must_use]
    #[cfg(any(test, feature = "reference-scheduler"))]
    pub fn with_scheduler_engine(mut self, engine: SchedulerEngine) -> Self {
        self.engine = engine;
        self
    }

    /// Bound host cooperation in steps. By contract, smaller calls cannot move
    /// device or IRQ boundaries; device-free CPU regressions hold the core to it
    /// (scheduler design §Adoption).
    #[must_use]
    pub fn with_step_limit(mut self, limit: NonZeroU64) -> Self {
        self.step_limit = limit;
        self
    }

    /// The selected device timer dispatch policy.
    pub fn scheduler_engine(&self) -> SchedulerEngine {
        self.engine
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
    /// For CPU rate A/B, this is the active count plus
    /// `floor(halted_clocks * A / B)`, saturating at `u64::MAX`. The floor applies
    /// to accumulated halted clocks, so fractional instruction equivalents survive
    /// separate halts and runner calls. Halted clocks do not consume the running
    /// CPU's fractional PIT-clock remainder. Unlike [`Cpu::retired`], this advances
    /// during HLT too, so run caps and scheduled targets bound busy and idle time.
    /// Active time counts completed steps, including individual REP iterations.
    pub fn virtual_time(&self) -> u64 {
        let halted = u128::from(self.halted_clocks) * u128::from(self.cpu_rate_numerator.get())
            / u128::from(self.cpu_rate_denominator.get());
        let active = self.completed_steps;
        (u128::from(active) + halted).min(u128::from(u64::MAX)) as u64
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
    /// The assembler starts without an idle wait; the run policy applies it here.
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

    fn next_deadline(&mut self) -> Result<Option<VClock>, MachineError> {
        #[cfg(any(test, feature = "reference-scheduler"))]
        let event = if self.engine == SchedulerEngine::PerClock {
            if self.bus.device(0).is_none() {
                return Ok(None);
            }
            NextEvent::EveryPitClock
        } else {
            self.bus.next_event()
        };
        #[cfg(not(any(test, feature = "reference-scheduler")))]
        let event = self.bus.next_event();
        let at = match event {
            NextEvent::Dormant => return Ok(None),
            NextEvent::EveryPitClock => VClock(
                self.clock
                    .0
                    .checked_add(1)
                    .ok_or_else(|| scheduler_error("clock exhausted"))?,
            ),
            NextEvent::At(at) => at,
        };
        if at <= self.clock {
            return Err(scheduler_error("device deadline did not advance"));
        }
        Ok(Some(at))
    }

    // Visit transitions at their own times so a short low pulse cannot vanish
    // inside a slow CPU step or a halted jump.
    fn advance_to(&mut self, target: VClock) -> Result<(), MachineError> {
        while self.clock < target {
            let at = self.next_deadline()?.map_or(target, |at| at.min(target));
            self.clock = at;
            match self.engine {
                #[cfg(any(test, feature = "reference-scheduler"))]
                SchedulerEngine::PerClock => self.bus.tick_all(at),
                SchedulerEngine::Deadline => {
                    self.bus.tick_due(at);
                }
            }
            self.edge_detect();
        }
        Ok(())
    }

    fn charge_steps(&mut self, steps: u64) -> Result<(), MachineError> {
        self.completed_steps = self
            .completed_steps
            .checked_add(steps)
            .ok_or_else(|| scheduler_error("step counter exhausted"))?;
        let total = u128::from(self.clock_remainder)
            + u128::from(steps) * u128::from(self.cpu_rate_denominator.get());
        let rate = u128::from(self.cpu_rate_numerator.get());
        let clocks =
            u64::try_from(total / rate).map_err(|_| scheduler_error("clock interval exhausted"))?;
        self.clock_remainder = (total % rate) as u64;
        let target = self
            .clock
            .0
            .checked_add(clocks)
            .ok_or_else(|| scheduler_error("clock exhausted"))?;
        self.advance_to(VClock(target))
    }

    fn settle(&mut self, accessed: bool) -> Result<(), MachineError> {
        if accessed {
            // An access can change a coupled device as well as its own pins.
            // Ordinary instruction boundaries have no such side effects.
            self.bus.settle();
            self.edge_detect();
        }
        for _ in 0..1024 {
            if !self.service_dma_pass()? {
                return Ok(());
            }
            self.bus.settle();
            self.edge_detect();
        }
        Err(scheduler_error("DMA completion made no progress"))
    }

    /// Run to a natural halt or an absolute virtual-time cap (§3.1, §6).
    ///
    /// The stopping instruction completes all bus/DMA effects before a ready
    /// interrupt is acknowledged. Active caps count exact completed steps;
    /// halted caps round up to a whole PIT clock at fractional CPU rates.
    /// `None` imposes no cap. Captured output survives either stopping outcome.
    pub fn run_bounded(&mut self, cap: Option<u64>) -> Result<RunOutcome, MachineError> {
        let mut completing = false;
        let mut no_progress = 0;
        self.settle(true)?;
        loop {
            if !completing {
                // Policy observations and hard caps win over a new INTA.
                if cap.is_some_and(|cap| self.virtual_time() >= cap) {
                    return Ok(RunOutcome::RetiredCapReached);
                }
                if self.cpu.irq_ready() && !self.cpu.has_pending_irq() && self.pic.poll().is_some()
                {
                    if let Some(vector) = self.pic.acknowledge() {
                        self.cpu.inject_irq(vector);
                    }
                }
                if self.halted {
                    if !self.cpu.interrupts_enabled() {
                        return Ok(RunOutcome::Halted);
                    }
                    if self.cpu.has_pending_irq() {
                        self.halted = false;
                        self.idle_clocks = 0;
                    } else {
                        let Some(limit) = self.idle_limit else {
                            return Ok(RunOutcome::Halted);
                        };
                        if self.idle_clocks >= limit {
                            return Ok(RunOutcome::Halted);
                        }
                        let mut clocks = limit - self.idle_clocks;
                        if let Some(deadline) = self.next_deadline()? {
                            clocks = clocks.min(deadline.0 - self.clock.0);
                        }
                        if let Some(cap) = cap {
                            let a = u128::from(self.cpu_rate_numerator.get());
                            let b = u128::from(self.cpu_rate_denominator.get());
                            let credit = u128::from(self.halted_clocks) * a % b;
                            let room = u128::from(cap - self.virtual_time()) * b - credit;
                            clocks =
                                clocks.min(u64::try_from(room.div_ceil(a)).unwrap_or(u64::MAX));
                        }
                        let target = self
                            .clock
                            .0
                            .checked_add(clocks)
                            .ok_or_else(|| scheduler_error("clock exhausted"))?;
                        self.advance_to(VClock(target))?;
                        self.halted_clocks = self
                            .halted_clocks
                            .checked_add(clocks)
                            .ok_or_else(|| scheduler_error("halted clock counter exhausted"))?;
                        self.idle_clocks += clocks;
                        self.settle(false)?;
                        continue;
                    }
                }
            }

            let mut budget = cap
                .map_or(u64::MAX, |cap| cap - self.virtual_time())
                .min(self.step_limit.get());
            let options = cpu::StepRunOptions {
                stop_when_irq_ready: self.pic.poll().is_some() && !self.cpu.irq_ready(),
            };
            if completing {
                budget = 1;
            } else if let Some(deadline) = self.next_deadline()? {
                let clocks = u128::from(deadline.0 - self.clock.0);
                let phase = clocks * u128::from(self.cpu_rate_numerator.get())
                    - u128::from(self.clock_remainder);
                let steps = phase.div_ceil(u128::from(self.cpu_rate_denominator.get()));
                budget = budget.min(u64::try_from(steps).unwrap_or(u64::MAX));
            }
            let before = self.cpu.completed_steps();
            let run = match self.cpu.run_steps(budget, options) {
                Ok(run) => run,
                Err(error) => {
                    self.charge_steps(self.cpu.completed_steps() - before)?;
                    self.settle(false)?;
                    return Err(error.into());
                }
            };
            if run.steps == 0 {
                no_progress += 1;
                if no_progress > 1024 {
                    return Err(scheduler_error("instruction completion made no progress"));
                }
            } else {
                no_progress = 0;
            }
            let stop = match run.stop {
                CpuStepStop::IrqReady => CpuStop::BudgetExhausted,
                CpuStepStop::Exit(stop) => stop,
            };
            let counted_stop = matches!(
                stop,
                CpuStop::PortOut { .. } | CpuStop::PortIn { .. } | CpuStop::Halt
            );
            let prefix = if counted_stop {
                run.steps
                    .checked_sub(1)
                    .ok_or_else(|| scheduler_error("counted stop completed no steps"))?
            } else {
                run.steps
            };
            self.charge_steps(prefix)?;
            let accessed = matches!(
                stop,
                CpuStop::PortOut { .. }
                    | CpuStop::PortIn { .. }
                    | CpuStop::MmioWrite { .. }
                    | CpuStop::MmioRead { .. }
            );
            completing = false;
            let access_result = match stop {
                CpuStop::PortOut { port, size, value } => {
                    self.bus.port_write(port, size, value as u32)
                }
                CpuStop::PortIn { port, size } => self.bus.port_read(port, size).map(|value| {
                    self.cpu.complete_port_in(u64::from(value));
                }),
                CpuStop::MmioWrite { addr, size, value } => {
                    self.bus.mmio_write(addr, size, value).map(|()| {
                        self.cpu.complete_mmio_write();
                        completing = true;
                    })
                }
                CpuStop::MmioRead { addr, size } => self.bus.mmio_read(addr, size).map(|value| {
                    self.cpu.complete_mmio_read(value);
                    completing = true;
                }),
                CpuStop::Exception { vector } => return Err(MachineError::Exception { vector }),
                CpuStop::Halt => {
                    self.halted = true;
                    Ok(())
                }
                CpuStop::BudgetExhausted => Ok(()),
            };
            if accessed {
                // A register can gate another device's timer. Refresh every
                // deadline before advancing through the stopping instruction.
                self.bus.invalidate_deadlines();
            }
            if counted_stop {
                self.charge_steps(1)?;
            }
            // Even a rejected access can have device side effects. Settle the
            // completed prefix and refresh coupled state before reporting it;
            // an unsuccessful deferred MMIO operation earns no extra step.
            self.settle(accessed)?;
            access_result?;
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
    /// channel, writing RAM via the `note_write` path (§4.3), dropping ROM
    /// writes, dispatching trapped windows through the MMIO bus, and reporting
    /// the byte count and TC signal back. For a memory→device request
    /// ([`DmaOp::FromMemory`]) — what a floppy WRITE queues — the machine reads
    /// the requested length from guest RAM, ROM or MMIO through the same channel,
    /// truncates the buffer to what the channel actually transferred (terminal
    /// count may cut it short, and the requester must never see uninitialised
    /// tail bytes), and hands the bytes
    /// back with the TC signal. Either way the controller advances the channel's
    /// address and count, then the requester finishes its result phase and raises
    /// its interrupt.
    ///
    /// This is the only place a device's bytes cross to or from memory without the
    /// device holding a memory reference: the requester never sees an address, the
    /// controller never sees the disk, and the SMC-safe seam is preserved.
    /// Bus faults propagate just like CPU MMIO faults; a failed transfer does
    /// not report a successful completion to the requester.
    #[cfg(test)]
    fn service_dma(&mut self) -> Result<(), BusError> {
        self.service_dma_pass().map(|_| ())
    }

    fn service_dma_pass(&mut self) -> Result<bool, BusError> {
        let mut transferred = false;
        for i in 0..self.dma_wirings.len() {
            let wiring = self.dma_wirings[i];
            let request = self
                .bus
                .device_mut(wiring.requester)
                .and_then(|dev| dev.take_dma_request());
            let Some(request) = request else {
                continue;
            };
            transferred = true;
            let mut mem = CpuDmaMemory {
                cpu: &mut self.cpu,
                read_offset: std::cell::Cell::new(0),
                mmio_reads: std::cell::RefCell::new(Vec::new()),
                mmio_writes: Vec::new(),
            };
            let outcome = match request.op {
                DmaOp::ToMemory(data) => {
                    let transfer = self
                        .bus
                        .device_mut(wiring.controller)
                        .map(|ctrl| ctrl.dma_service_write(request.channel, &mut mem, &data))
                        .unwrap_or_default();
                    for (addr, value) in mem.mmio_writes {
                        self.bus.mmio_write(u64::from(addr), 1, u64::from(value))?;
                    }
                    DmaOutcome::ToMemory {
                        transferred: transfer.transferred,
                        terminal_count: transfer.terminal_count,
                    }
                }
                DmaOp::FromMemory { len } => {
                    let mut buf = vec![0u8; len];
                    let transfer = self
                        .bus
                        .device_mut(wiring.controller)
                        .map(|ctrl| ctrl.dma_service_read(request.channel, &mut mem, &mut buf))
                        .unwrap_or_default();
                    buf.truncate(transfer.transferred);
                    for (offset, addr) in mem.mmio_reads.into_inner() {
                        buf[offset] = self.bus.mmio_read(u64::from(addr), 1)? as u8;
                    }
                    DmaOutcome::FromMemory {
                        bytes: buf,
                        terminal_count: transfer.terminal_count,
                    }
                }
            };
            if let Some(dev) = self.bus.device_mut(wiring.requester) {
                dev.dma_complete(outcome);
            }
        }
        Ok(transferred)
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

    fn instruction_cpu(program: Vec<CpuStop>, enabled: bool) -> MockCpu {
        let script = program
            .iter()
            .copied()
            .chain([CpuStop::Halt; 4])
            .map(|stop| {
                let steps = u64::from(!matches!(stop, CpuStop::Exception { .. }));
                testkit::MockCpuStep {
                    max_steps: 1,
                    run: cpu::CpuStepRun {
                        steps,
                        stop: CpuStepStop::Exit(stop),
                    },
                    retired: steps,
                    irq_ready: enabled,
                }
            })
            .collect();
        MockCpu::new(program)
            .with_interrupts_enabled(enabled)
            .with_step_program(script)
    }

    fn machine_with(program: Vec<CpuStop>) -> Machine<MockCpu> {
        Machine::new(
            instruction_cpu(program, false),
            bus_with_capture(),
            Box::new(NoIc),
            Vec::new(),
            Some(1),
        )
        .with_step_limit(NonZeroU64::MIN)
    }

    #[test]
    fn cpu_rate_conversion_is_independent_of_step_partitioning() {
        const TOTAL: u64 = 4095;
        for (numerator, denominator) in [(1, 1), (2, 1), (1, 3), (3, 2), (7, 3), (6, 4)] {
            for stride in [1, 3, 7, 15, 63, TOTAL] {
                let mut machine = machine_with(Vec::new()).with_cpu_rate(
                    NonZeroU64::new(numerator).unwrap(),
                    NonZeroU64::new(denominator).unwrap(),
                );
                for retired in (stride..=TOTAL).step_by(stride as usize) {
                    machine.charge_steps(stride).unwrap();
                    assert_eq!(machine.clock(), VClock(retired * denominator / numerator));
                    assert_eq!(machine.clock_remainder, retired * denominator % numerator);
                    assert_eq!(machine.virtual_time(), retired);
                    let before = machine.clock();
                    machine.charge_steps(0).unwrap();
                    assert_eq!(
                        machine.clock(),
                        before,
                        "zero retirement must not age devices"
                    );
                }
            }
        }
    }

    #[test]
    fn cpu_rate_remainder_survives_halts_and_wakeups() {
        const CYCLES: usize = 64;
        for (numerator, denominator) in [(1, 1), (2, 1), (1, 3), (3, 2), (7, 3)] {
            let program = [CpuStop::Halt, CpuStop::BudgetExhausted].repeat(CYCLES);
            let mut machine = machine_with(program.clone()).with_cpu_rate(
                NonZeroU64::new(numerator).unwrap(),
                NonZeroU64::new(denominator).unwrap(),
            );
            machine.cpu = instruction_cpu(program, true);
            machine.set_idle_limit(Some(1));

            for cycle in 0..CYCLES as u64 {
                assert_eq!(machine.run_bounded(None).expect("halt"), RunOutcome::Halted);
                let retired = 2 * cycle + 1;
                let halted = cycle + 1;
                assert_eq!(machine.cpu().retired(), retired);
                assert_eq!(
                    machine.clock(),
                    VClock(retired * denominator / numerator + halted)
                );
                assert_eq!(machine.clock_remainder, retired * denominator % numerator);
                assert_eq!(
                    machine.virtual_time(),
                    retired + halted * numerator / denominator
                );

                machine.cpu.inject_irq(0x08);
                let cap = machine.virtual_time() + 1;
                assert_eq!(
                    machine.run_bounded(Some(cap)).expect("wake"),
                    RunOutcome::RetiredCapReached
                );
                assert_eq!(machine.cpu().retired(), retired + 1);
                assert_eq!(
                    machine.clock(),
                    VClock((retired + 1) * denominator / numerator + halted)
                );
                assert_eq!(machine.virtual_time(), cap);
            }
        }
    }

    #[test]
    fn halted_cpu_rate_caps_preserve_fraction_across_runner_calls() {
        for (numerator, denominator) in [(2, 1), (1, 3), (3, 2)] {
            let mut machine = machine_with(vec![CpuStop::Halt]).with_cpu_rate(
                NonZeroU64::new(numerator).unwrap(),
                NonZeroU64::new(denominator).unwrap(),
            );
            machine.cpu = machine.cpu.with_interrupts_enabled(true);
            machine.set_idle_limit(Some(1000));
            for cap in [2, 3, 7, 19] {
                assert_eq!(
                    machine.run_bounded(Some(cap)).expect("cap"),
                    RunOutcome::RetiredCapReached
                );
                let halted = ((cap - 1) * denominator).div_ceil(numerator);
                assert_eq!(machine.cpu().retired(), 1);
                assert_eq!(machine.halted_clocks, halted);
                assert_eq!(machine.clock(), VClock(denominator / numerator + halted));
                assert_eq!(machine.clock_remainder, denominator % numerator);
                assert_eq!(machine.virtual_time(), 1 + halted * numerator / denominator);
                let before = machine.clock();
                machine.run_bounded(Some(cap)).expect("same cap");
                assert_eq!(machine.clock(), before);
            }
        }
    }

    #[test]
    fn cpu_rate_large_factors_do_not_overflow() {
        let mut machine = machine_with(vec![CpuStop::BudgetExhausted; 10]).with_cpu_rate(
            NonZeroU64::new(u64::MAX).unwrap(),
            NonZeroU64::new(u64::MAX - 1).unwrap(),
        );
        for retired in (2..=20).step_by(2) {
            machine.charge_steps(2).unwrap();
            assert_eq!(machine.clock(), VClock(retired - 1));
            assert_eq!(machine.clock_remainder, u64::MAX - retired);
        }

        let mut machine = machine_with(Vec::new()).with_cpu_rate(
            NonZeroU64::new(u64::MAX).unwrap(),
            NonZeroU64::new(u64::MAX / 2 + 1).unwrap(),
        );
        machine.halted_clocks = 2;
        assert_eq!(machine.virtual_time(), 3, "divide before saturating");
        machine.cpu_rate_denominator = NonZeroU64::MIN;
        assert_eq!(machine.virtual_time(), u64::MAX);
    }

    #[test]
    fn unmigrated_device_next_event_defaults_to_every_pit_clock() {
        let machine = machine_with(Vec::new());
        let device = machine.bus().device(0).expect("capture device");
        for now in [VClock::ZERO, VClock(17), VClock(u64::MAX)] {
            assert_eq!(device.next_event(now), device::NextEvent::EveryPitClock);
            assert!(device.save().is_empty());
        }
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

    /// A device with a scriptable IRQ line and EOI observation, standing in for
    /// the PIT/PIC pairing to test the wiring in isolation.
    struct LineDevice {
        level: bool,
        stop_after: Option<u64>,
        period: Option<u64>,
    }
    // Initial sampling now precedes the first clock: start on the steady high
    // level so this mock's first periodic rising edge remains at clock five.
    impl Default for LineDevice {
        fn default() -> Self {
            Self {
                level: true,
                stop_after: None,
                period: None,
            }
        }
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
            self.level = (now == VClock::ZERO || !now.0.is_multiple_of(self.period.unwrap_or(4)))
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
            instruction_cpu(program, true),
            bus,
            Box::<CountingIc>::default(),
            vec![IrqLine { device: 0, irq: 0 }],
            Some(1),
        )
        .with_step_limit(NonZeroU64::MIN);
        machine.run().expect("run");
        // The line device starts high, then is low at ticks 4, 8, … and high in between, so a
        // rising edge (one PIC request per 4-tick period) occurs at ticks 5, 9,
        // 13, 17, 21. The final `Halt` boundary advances the clock to 21, giving
        // five rising edges → five injected vectors. We can't read the boxed IC's
        // private counter, so assert the CPU received the matching injections.
        assert_eq!(machine.cpu().injected.len(), 5);
        assert!(machine.cpu().injected.iter().all(|&v| v == 0x08));
    }

    #[test]
    fn deadline_and_per_clock_preserve_every_pit_irq0_edge() {
        use devices::Pit8253;

        fn edges(engine: SchedulerEngine, mode: u8, reload: u16) -> Vec<VClock> {
            let bus = Bus::new(
                vec![Box::new(Pit8253::new())],
                DecodeMap {
                    ports: vec![PortRange {
                        ports: 0x40..=0x43,
                        device: 0,
                    }],
                    mmio: Vec::new(),
                },
            );
            let mut machine = Machine::new(
                MockCpu::new(Vec::new()),
                bus,
                Box::<CountingIc>::default(),
                vec![IrqLine { device: 0, irq: 0 }],
                None,
            )
            .with_scheduler_engine(engine);
            let mut edges = Vec::new();
            for clock in 1..=140000 {
                // Reprogram after a long dormant interval and midway through
                // an active period; no legacy device supplies extra samples.
                if [20, 97, 70000].contains(&clock) {
                    machine
                        .bus_mut()
                        .port_write(0x43, 1, 0x30 | u32::from(mode << 1))
                        .unwrap();
                    machine
                        .bus_mut()
                        .port_write(0x40, 1, u32::from(reload & 255))
                        .unwrap();
                    machine
                        .bus_mut()
                        .port_write(0x40, 1, u32::from(reload >> 8))
                        .unwrap();
                }
                machine.advance_to(machine.clock().advance(1)).unwrap();
                if machine.pic.acknowledge().is_some() {
                    edges.push(machine.clock());
                }
            }
            edges
        }

        for mode in [0, 2, 3] {
            for reload in [0, 1, 2, 3, 5, 8, 1193] {
                let reference = edges(SchedulerEngine::PerClock, mode, reload);
                assert_eq!(
                    reference,
                    edges(SchedulerEngine::Deadline, mode, reload),
                    "mode {mode}, reload {reload}"
                );
                if reload != 1 || mode != 2 {
                    assert!(!reference.is_empty());
                }
            }
        }
    }

    fn halted_machine(program: Vec<CpuStop>, limit: Option<u64>) -> Machine<MockCpu> {
        let bus = Bus::new(
            vec![
                Box::new(LineDevice {
                    level: true,
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
            instruction_cpu(program, true),
            bus,
            Box::<CountingIc>::default(),
            vec![IrqLine { device: 0, irq: 0 }],
            limit,
        )
        .with_step_limit(NonZeroU64::MIN)
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
            instruction_cpu(vec![CpuStop::Halt; 1000], true),
            bus,
            Box::<CountingIc>::default(),
            vec![IrqLine { device: 0, irq: 0 }],
            Some(100_000),
        )
        .with_step_limit(NonZeroU64::MIN);
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
    fn halted_reentry_with_if_clear_neither_ticks_nor_acknowledges() {
        for pending in [false, true] {
            let controller = CountingIc::default();
            let acknowledgements = controller.acknowledgements.clone();
            let mut machine =
                halted_machine(vec![CpuStop::Halt, CpuStop::BudgetExhausted], Some(100));
            machine.pic = Box::new(controller);
            machine.cpu = instruction_cpu(vec![CpuStop::Halt, CpuStop::BudgetExhausted], false);
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
        let mut machine = Machine::new(cpu, bus_with_capture(), Box::new(NoIc), Vec::new(), None);
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
        fn guest_dma_read(&self, addr: u32) -> cpu::DmaAccess<u8> {
            cpu::DmaAccess::Complete(self.mem[addr as usize])
        }
        fn guest_dma_write(&mut self, addr: u32, value: u8) -> cpu::DmaAccess<()> {
            self.mem[addr as usize] = value;
            cpu::DmaAccess::Complete(())
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
            Vec::new(),
            Some(1),
        )
        .with_dma_wirings(vec![DmaWiring {
            requester: 0,
            controller: 1,
        }])
    }

    /// Read the outcome the requester captured after a serviced transfer.
    fn captured_outcome<C: Cpu>(machine: &mut Machine<C>) -> DmaOutcome {
        machine
            .bus
            .device_mut(0)
            .and_then(|dev| dev.as_any_mut())
            .and_then(|any| any.downcast_mut::<FromMemoryRequester>())
            .and_then(|req| req.received.take())
            .expect("the requester received a completion")
    }

    /// A byte-wide memory-mapped device with observable read side effects.
    struct DmaMmioDevice {
        bytes: [u8; 2],
        reads: u8,
    }

    impl Device for DmaMmioDevice {
        fn name(&self) -> &'static str {
            "dma-mmio"
        }
        fn port_read(&mut self, _: u16, _: u8) -> Result<u32, DeviceError> {
            Ok(0)
        }
        fn port_write(&mut self, _: u16, _: u8, _: u32) -> Result<(), DeviceError> {
            Ok(())
        }
        fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
            assert_eq!(size, 1);
            self.reads += 1;
            Ok(u64::from(self.bytes[(addr - 0xB8000) as usize]))
        }
        fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
            assert_eq!(size, 1);
            self.bytes[(addr - 0xB8000) as usize] = value as u8;
            Ok(())
        }
        fn tick(&mut self, _: VClock) {}
        fn irq_pending(&self) -> bool {
            false
        }
        fn irq_vector(&self) -> Option<u8> {
            None
        }
        fn reset(&mut self) {}
        fn save(&self) -> Vec<u8> {
            vec![self.bytes[0], self.bytes[1], self.reads]
        }
        fn restore(&mut self, _: &[u8]) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    fn mapped_dma_machine(op: DmaOp, count: u16) -> Machine<cpu::X86JitCpu> {
        use cpu::x86jit_backend::RealSegments;
        let mut cpu = cpu::X86JitCpu::new_real16_bare(0x10_0000, 0, 0x7C00, RealSegments::flat(0))
            .expect("cpu");
        cpu.map_ram(0xB7FFE, 2).expect("ram before mmio");
        cpu.map_mmio(0xB8000, 2).expect("mmio");
        cpu.map_rom(0xB8002, &[0xA5, 0x5A]).expect("rom");
        cpu.map_ram(0xB8004, 2).expect("ram after rom");
        cpu.write_mem(0xB7FFE, &[0x11, 0x22]).expect("seed ram");
        cpu.write_mem(0xB8000, &[0xEE, 0xEE])
            .expect("seed mmio backing");
        cpu.write_mem(0xB8004, &[0x33, 0x44]).expect("seed ram");
        let mut dma = programmed_read_dma(0x7FFE, count);
        dma.port_write(0x81, 1, 0x0B).expect("page");
        if matches!(op, DmaOp::ToMemory(_)) {
            dma.port_write(0x0B, 1, 0x46).expect("write mode");
        }
        let requester = FromMemoryRequester {
            pending: Some(DmaRequest {
                channel: READ_CHANNEL,
                op,
            }),
            received: None,
        };
        let bus = Bus::new(
            vec![
                Box::new(requester),
                Box::new(dma),
                Box::new(DmaMmioDevice {
                    bytes: [0x66, 0x77],
                    reads: 0,
                }),
            ],
            DecodeMap {
                ports: Vec::new(),
                mmio: vec![bus::MmioRange {
                    addrs: 0xB8000..=0xB8001,
                    device: 2,
                }],
            },
        );
        Machine::new(cpu, bus, Box::new(NoIc), Vec::new(), Some(1)).with_dma_wirings(vec![
            DmaWiring {
                requester: 0,
                controller: 1,
            },
        ])
    }

    #[test]
    fn service_dma_write_routes_mmio_and_preserves_rom_across_boundaries() {
        let mut machine = mapped_dma_machine(DmaOp::ToMemory(vec![1, 2, 3, 4, 5, 6, 7, 8]), 6);
        machine.service_dma().expect("dma");
        assert_eq!(
            machine.bus().device(2).expect("mmio device").save(),
            vec![3, 4, 0]
        );
        let mut bytes = [0; 2];
        machine
            .cpu()
            .read_mem(0xB8000, &mut bytes)
            .expect("backing");
        assert_eq!(bytes, [0xEE, 0xEE]);
        machine.cpu().read_mem(0xB8002, &mut bytes).expect("rom");
        assert_eq!(bytes, [0xA5, 0x5A]);
        machine.cpu().read_mem(0xB7FFE, &mut bytes).expect("ram");
        assert_eq!(bytes, [1, 2]);
        machine.cpu().read_mem(0xB8004, &mut bytes).expect("ram");
        assert_eq!(bytes, [7, 0x44]);
        assert!(matches!(
            captured_outcome(&mut machine),
            DmaOutcome::ToMemory {
                transferred: 7,
                terminal_count: true
            }
        ));
    }

    #[test]
    fn service_dma_read_routes_mmio_and_reads_rom_across_boundaries() {
        let mut machine = mapped_dma_machine(DmaOp::FromMemory { len: 8 }, 6);
        machine.service_dma().expect("dma");
        assert_eq!(
            machine.bus().device(2).expect("mmio device").save(),
            vec![0x66, 0x77, 2]
        );
        match captured_outcome(&mut machine) {
            DmaOutcome::FromMemory {
                bytes,
                terminal_count,
            } => {
                assert!(terminal_count);
                assert_eq!(bytes, [0x11, 0x22, 0x66, 0x77, 0xA5, 0x5A, 0x33]);
            }
            other => panic!("expected memory read, got {other:?}"),
        }
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
        machine.service_dma().expect("dma");

        match captured_outcome(&mut machine) {
            DmaOutcome::FromMemory {
                bytes,
                terminal_count,
            } => {
                assert!(terminal_count);
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
        machine.service_dma().expect("dma");

        match captured_outcome(&mut machine) {
            DmaOutcome::FromMemory {
                bytes,
                terminal_count,
            } => {
                assert!(terminal_count);
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

#[cfg(test)]
mod corrected_tests;
