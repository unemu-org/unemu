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

/// Why [`Cpu::run`] returned control to the machine loop.
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
    /// The run budget (quantum) was exhausted — a cooperative yield (§6.2).
    BudgetExhausted,
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

    /// Inject a hardware interrupt request for `vector` (§5.2).
    ///
    /// The backend queues the vector and delivers it at the next `run()`
    /// boundary once the guest's `IF` is set, no memory/port completion is
    /// outstanding, and the one-instruction STI shadow has elapsed (that gating
    /// lives in the CPU core). Delivery is in-guest through the real-mode IVT: it
    /// pushes FLAGS/CS/IP, clears `IF`, and vectors the handler. A queued vector
    /// is held (never dropped) until it can be delivered, which is what wakes a
    /// `hlt`ed guest. The default is a no-op so a backend without an interrupt
    /// path still runs.
    fn inject_irq(&mut self, _vector: u8) {}

    /// Whether an injected vector is still queued and not yet delivered.
    ///
    /// The machine loop consults this so it only performs one PIC INTA →
    /// [`Cpu::inject_irq`] at a time: while a vector is in flight (masked by
    /// `IF`, held by the STI shadow, or waiting on a completion) no new one is
    /// acknowledged. The default is `false` for a backend that takes no
    /// interrupts.
    fn has_pending_irq(&self) -> bool {
        false
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
