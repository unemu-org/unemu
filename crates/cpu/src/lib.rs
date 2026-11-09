// SPDX-License-Identifier: AGPL-3.0-or-later
//! CPU swap seam (§2, §3.2 seam 1).
//!
//! [`Cpu`] is the first of the six swap seams from
//! `backlog/docs/architecture.md`: it abstracts the CPU core so the `machine`
//! composition root never names a concrete backend. The only backend is
//! [`X86JitCpu`], wrapping the `x86jit-core` recompiler; it runs either in
//! **Long64** mode (the serial walking skeleton) or in **Real16** mode (an
//! IBM PC/AT-style boot that takes hardware interrupts).
//!
//! The backend exploits the integration surface `x86jit` exposes: `Exit::PortIo`
//! with `complete_port_in` for ISA port I/O, and `RegionKind::Trap` MMIO (§2.1).
//! In real mode it additionally forwards hardware IRQ injection to the core
//! ([`Cpu::inject_irq`]), which delivers the vector in-guest through the IVT, and
//! exposes the retired-instruction counter that drives the virtual clock (§6).

pub mod x86jit_backend;

pub use x86jit_backend::X86JitCpu;

use thiserror::Error;

/// Guest registers the [`Cpu`] seam exposes (§3.1).
///
/// A minimal, backend-neutral subset; it grows with real-mode work (segment
/// registers, flags) later. Values are the full 64-bit register width.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CpuReg {
    /// Accumulator.
    Rax,
    /// Counter.
    Rcx,
    /// Data.
    Rdx,
    /// Base.
    Rbx,
    /// Stack pointer.
    Rsp,
    /// Base pointer.
    Rbp,
    /// Source index.
    Rsi,
    /// Destination index.
    Rdi,
    /// Instruction pointer.
    Rip,
}

/// Why [`Cpu::run`] or [`Cpu::run_steps`] returned control to the machine loop.
///
/// The backend-neutral counterpart of `x86jit`'s `Exit` (§5.2). The `machine`
/// loop matches on this and dispatches to the bus, never seeing the underlying
/// core's own exit type.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CpuStop {
    /// Guest executed `in` — the machine must supply the port value via
    /// [`Cpu::complete_port_in`] before the next [`Cpu::run`].
    PortIn {
        /// 16-bit port number.
        port: u16,
        /// Access width in bytes (1/2/4).
        size: u8,
    },
    /// Guest executed `out` — `value` holds the low `size` bytes written.
    PortOut {
        /// 16-bit port number.
        port: u16,
        /// Access width in bytes (1/2/4).
        size: u8,
        /// The value the guest wrote (low `size` bytes).
        value: u64,
    },
    /// Guest read a trapped (MMIO) address — supply the value via
    /// [`Cpu::complete_mmio_read`] before the next run.
    MmioRead {
        /// Guest physical address.
        addr: u64,
        /// Access width in bytes.
        size: u8,
    },
    /// Guest wrote a trapped (MMIO) address.
    MmioWrite {
        /// Guest physical address.
        addr: u64,
        /// Access width in bytes.
        size: u8,
        /// The value the guest wrote.
        value: u64,
    },
    /// Guest executed `hlt`.
    Halt,
    /// A guest CPU exception (`#DE`, `#UD`, `#BP`, …); `vector` is the x86
    /// exception vector (§5.2).
    Exception {
        /// x86 exception vector.
        vector: u8,
    },
    /// The run budget was exhausted: blocks for [`Cpu::run`], steps for
    /// [`Cpu::run_steps`] — a cooperative yield (§6.2).
    BudgetExhausted,
}

/// Result of a step-bounded CPU run (§6).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CpuStepRun {
    /// Completed scheduling units in this call; see [`Cpu::run_steps`].
    pub steps: u64,
    /// Why execution returned to the embedder.
    pub stop: CpuStepStop,
}

/// Why a step-bounded run returned (§6).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CpuStepStop {
    /// An ordinary CPU exit, including step-budget exhaustion.
    Exit(CpuStop),
    /// A completed boundary where a maskable IRQ can be accepted.
    /// No instruction is in flight and no vector has been acknowledged.
    IrqReady,
}

/// Errors from the [`Cpu`] seam (§2).
#[derive(Debug, Error)]
pub enum CpuError {
    /// A memory operation on the guest address space failed (mapping/write).
    #[error("guest memory error: {0}")]
    Memory(String),
    /// The guest attempted an unmapped access the machine could not service.
    #[error("unmapped guest access at {addr:#x}")]
    Unmapped {
        /// Faulting guest address.
        addr: u64,
    },
    /// The core hit an instruction it does not lift (surfaces the bytes so the
    /// missing opcode can be added).
    #[error("unsupported instruction at {addr:#x}")]
    UnsupportedInstruction {
        /// Guest RIP of the offending instruction.
        addr: u64,
    },
    /// The core stopped for a reason the machine has no handling for (a host
    /// yield request, exhausted JIT code space, or an exit kind newer than this
    /// backend), or a refused step-bounded call. Kept apart from
    /// [`CpuError::UnsupportedInstruction`] so such a stop is never mistaken for
    /// a missing opcode.
    #[error("unhandled cpu exit: {what}")]
    UnexpectedExit {
        /// Which exit the core returned, with its details.
        what: String,
    },
}

/// Result of one DMA byte cycle in the guest physical memory map (§4.3).
///
/// Intel 8237A datasheet, Pin Description (MEMR/MEMW): DMA drives memory bus
/// cycles independently of CPU instruction execution.
/// The CPU backend has no devices: a trapped address must be dispatched by the
/// machine through its MMIO bus before completing the request to the bus master.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[must_use]
pub enum DmaAccess<T> {
    /// Memory serviced the cycle, including a dropped ROM or unmapped write.
    Complete(T),
    /// A trapped window requires an MMIO byte cycle at the same address.
    Mmio,
}

/// The CPU swap seam (§3.2 seam 1).
///
/// A backend implements execution ([`Cpu::run`]), register access, the
/// completion handshakes for pending port/MMIO reads, the IRQ-injection contract
/// (§5.2), and the retired-instruction counter that measures virtual time (§6).
pub trait Cpu {
    /// Run guest code until an [`CpuStop`] is hit or `budget` blocks execute.
    ///
    /// `budget` is the cooperative quantum in `x86jit` block units (§6.2);
    /// `None` runs until a non-budget stop. Devices tick on `run()` boundaries,
    /// so this value governs interrupt latency and goldens depend on it.
    fn run(&mut self, budget: Option<u64>) -> Result<CpuStop, CpuError>;

    /// Run at most `max_steps` scheduling units (§6).
    ///
    /// One unit is a completed instruction or a completed REP iteration. An
    /// empty REP counts once. This is separate from [`Cpu::retired`], which
    /// counts whole instructions. A supported zero-budget call has no guest
    /// effects, including interrupt delivery or device completion consumption.
    /// The default returns [`CpuError::UnexpectedExit`] without execution.
    /// Unsupported modes and an outstanding IN value also return that error.
    ///
    /// The stopping instruction's share of [`CpuStepRun::steps`] is:
    ///
    /// - Port IN/OUT and HLT: the last counted unit; its start is `steps - 1`.
    ///   IN counts at handoff even though its value is still pending; supplying
    ///   that value adds no step. Complete it before another positive-budget run.
    /// - MMIO, faults, fetch failures and unsupported instructions: zero; all
    ///   counted units precede the incomplete unit, whose start is `steps`.
    ///   A REP exiting mid-iteration counts only its completed iterations.
    /// - Budget exhaustion or IRQ readiness: no unit is in flight; the next
    ///   unit starts at `steps`. An instruction's own exit wins on the last unit.
    ///
    /// Complete bus effects before acknowledging an interrupt. Errors carry no
    /// step count, so callers must propagate them rather than resume scheduling.
    /// REP restart follows Intel SDM Vol. 3A §7.6 (Program or Task Restart);
    /// the port-handoff count is an engine convention, not hardware timing.
    fn run_steps(&mut self, _max_steps: u64) -> Result<CpuStepRun, CpuError> {
        Err(CpuError::UnexpectedExit {
            what: "step-bounded execution is unsupported by this backend or mode".to_string(),
        })
    }

    /// Whether a maskable IRQ can be accepted at this completed boundary (§5.2).
    ///
    /// Requires IF, no STI or SS-load shadow, and no pending port/MMIO completion
    /// (including an answered MMIO operation awaiting replay). This query does
    /// not acknowledge, deliver or require a queued vector. Intel SDM Vol. 3A
    /// §§7.8.1, 7.8.3 describe IF and interrupt inhibition after loading SS.
    /// The default is `false` for backends without this capability.
    fn irq_ready(&self) -> bool {
        false
    }

    /// Read a guest register (full 64-bit width).
    fn reg(&self, reg: CpuReg) -> u64;

    /// Write a guest register (full 64-bit width).
    fn set_reg(&mut self, reg: CpuReg, value: u64);

    /// Supply the value for a pending [`CpuStop::PortIn`] before the next
    /// [`Cpu::run`].
    fn complete_port_in(&mut self, value: u64);

    /// Supply the value for a pending [`CpuStop::MmioRead`] before the next
    /// [`Cpu::run`].
    fn complete_mmio_read(&mut self, value: u64);

    /// Acknowledge a serviced [`CpuStop::MmioWrite`] before the next [`Cpu::run`].
    ///
    /// Unlike a port `out` (which retires immediately), a trapped store leaves the
    /// instruction pointer on the faulting instruction and the block re-executes
    /// from it. This tells the retried store the side effect is already done — so
    /// it does not trap a second time and execution moves on. Without this call a
    /// guest that writes MMIO spins on the same store forever.
    fn complete_mmio_write(&mut self);

    /// Whether guest FLAGS.IF permits maskable interrupts (Intel 80286
    /// Programmer's Reference Manual, HLT and CLI/STI instruction descriptions;
    /// §5.2). The machine uses this to decide whether HLT can wake.
    fn interrupts_enabled(&self) -> bool;

    /// Inject a hardware interrupt request for `vector` (§5.2).
    ///
    /// The backend queues the vector and delivers it at the next `run()`
    /// boundary once the guest's `IF` is set, no memory/port completion is
    /// outstanding, and the one-instruction STI or SS-load shadow has elapsed
    /// (that gating lives in the CPU core). Delivery is through the real-mode IVT: it
    /// pushes FLAGS/CS/IP, clears `IF`, and vectors the handler. A queued vector
    /// is held (never dropped) until it can be delivered, which is what wakes a
    /// `hlt`ed guest. The default is a no-op so a backend without an interrupt
    /// path still runs.
    fn inject_irq(&mut self, _vector: u8) {}

    /// Whether an injected vector is still queued and not yet delivered.
    ///
    /// The machine loop consults this so it only performs one PIC INTA →
    /// [`Cpu::inject_irq`] at a time: while a vector is in flight (masked by
    /// `IF`, held by an STI/SS-load shadow, or waiting on a completion) no new
    /// one is acknowledged. The default is `false` for a backend that takes no
    /// interrupts.
    fn has_pending_irq(&self) -> bool {
        false
    }

    /// Read one byte of guest physical memory for a DMA transfer (§4.3).
    ///
    /// RAM and ROM return their bytes; a trapped window returns
    /// [`DmaAccess::Mmio`] so the machine can call its bus's MMIO read. The
    /// default returns open-bus `0xFF` for a backend with no guest memory.
    fn guest_dma_read(&self, addr: u32) -> DmaAccess<u8> {
        let _ = addr;
        DmaAccess::Complete(0xFF)
    }

    /// Write one DMA byte according to the guest physical memory map (§4.3).
    ///
    /// RAM writes must run the core's `note_write` self-modifying-code hook.
    /// ROM writes are dropped; a trapped window returns [`DmaAccess::Mmio`]
    /// without modifying backing memory, so the machine can dispatch the write.
    /// The default drops the write for a backend with no guest memory.
    fn guest_dma_write(&mut self, addr: u32, value: u8) -> DmaAccess<()> {
        let _ = (addr, value);
        DmaAccess::Complete(())
    }

    /// Retired-instruction count since reset — the deterministic virtual-time
    /// base (§6, virtual-clock model in `architecture.md`).
    ///
    /// A monotone counter of guest instructions that have completed. The machine
    /// advances the virtual clock from the *delta* of this value across a
    /// `run()`, never from wall time, so device timing (and every golden) is
    /// reproducible. The default is `0` for a backend that does not count.
    fn retired(&self) -> u64 {
        0
    }
}
