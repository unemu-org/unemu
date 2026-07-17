// SPDX-License-Identifier: AGPL-3.0-or-later
//! `x86jit`-backed CPU (§2.1, §3.2 seam 1).
//!
//! [`X86JitCpu`] wraps an `x86jit-core` [`Vm`] + [`Vcpu`] running in **Long64**
//! mode and maps the core's [`Exit`] variants onto the backend-neutral
//! [`CpuStop`]. It uses only the integration surface that already works in
//! `x86jit` today — `Exit::PortIo` (`in`/`out`) and `RegionKind::Trap` MMIO —
//! with no change to the CPU core.
//!
//! `x86jit-core` is depended on through a git ref rather than a plain path so a
//! clean checkout or CI can resolve it without the local worktree present.

use x86jit_core::{Exit, PortDir, Prot, Reg, RegionKind, Vcpu, Vm, VmConfig};

use crate::{Cpu, CpuError, CpuReg, CpuStop};

/// A CPU backed by the `x86jit` interpreter core, executing in Long64 (§2.1).
///
/// Owns the guest [`Vm`] (its memory map) and the [`Vcpu`]. The machine loop
/// drives it through the [`Cpu`] trait; it never exposes `x86jit` types.
pub struct X86JitCpu {
    vm: Vm,
    cpu: Vcpu,
}

impl X86JitCpu {
    /// Build a CPU over a flat guest address space of `ram_bytes`, load `code`
    /// at `entry`, and set RIP there.
    ///
    /// The whole space is mapped `RWX` `Ram`; MMIO-trapped windows are added
    /// with [`X86JitCpu::map_mmio`] before execution. Code is written through the
    /// `Vm` API so the core's self-modifying-code tracking sees the write — a raw
    /// buffer poke would silently break translated-block invalidation.
    pub fn new(ram_bytes: u64, entry: u64, code: &[u8]) -> Result<Self, CpuError> {
        let mut vm = Vm::new(VmConfig::flat(ram_bytes));
        vm.map(0, ram_bytes as usize, Prot::RWX, RegionKind::Ram)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))?;
        vm.write_bytes(entry, code)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))?;
        let mut cpu = vm.new_vcpu();
        cpu.set_reg(Reg::Rip, entry);
        Ok(Self { vm, cpu })
    }

    /// Map a memory-mapped-I/O (trapped) window `[base, base + size)`.
    ///
    /// Loads/stores in this range yield `Exit::MmioRead`/`Exit::MmioWrite`,
    /// surfaced as [`CpuStop::MmioRead`]/[`CpuStop::MmioWrite`] (§4.2, §5.2).
    pub fn map_mmio(&mut self, base: u64, size: u64) -> Result<(), CpuError> {
        self.vm
            .map(base, size as usize, Prot::RW, RegionKind::Trap)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
    }
}

/// Map the neutral [`CpuReg`] onto the core's [`Reg`].
fn to_core_reg(reg: CpuReg) -> Reg {
    match reg {
        CpuReg::Rax => Reg::Rax,
        CpuReg::Rcx => Reg::Rcx,
        CpuReg::Rdx => Reg::Rdx,
        CpuReg::Rbx => Reg::Rbx,
        CpuReg::Rsp => Reg::Rsp,
        CpuReg::Rbp => Reg::Rbp,
        CpuReg::Rsi => Reg::Rsi,
        CpuReg::Rdi => Reg::Rdi,
        CpuReg::Rip => Reg::Rip,
    }
}

impl Cpu for X86JitCpu {
    fn run(&mut self, budget: Option<u64>) -> Result<CpuStop, CpuError> {
        match self.cpu.run(&self.vm, budget) {
            Exit::PortIo {
                port,
                size,
                dir,
                value,
            } => match dir {
                PortDir::In => Ok(CpuStop::PortIn { port, size }),
                PortDir::Out => Ok(CpuStop::PortOut { port, size, value }),
            },
            Exit::MmioRead { addr, size } => Ok(CpuStop::MmioRead { addr, size }),
            Exit::MmioWrite { addr, size, value } => Ok(CpuStop::MmioWrite { addr, size, value }),
            Exit::Hlt => Ok(CpuStop::Halt),
            Exit::Exception { vector, .. } => Ok(CpuStop::Exception { vector }),
            Exit::BudgetExhausted => Ok(CpuStop::BudgetExhausted),
            Exit::UnmappedMemory { addr, .. } => Err(CpuError::Unmapped { addr }),
            Exit::UnknownInstruction { addr, .. } => Err(CpuError::UnsupportedInstruction { addr }),
            // The current serial programs contain no `syscall`/`int 0x80`, so
            // this cannot occur; guest-vectored interrupts are future work.
            Exit::Syscall => Err(CpuError::UnsupportedInstruction {
                addr: self.cpu.reg(Reg::Rip),
            }),
        }
    }

    fn reg(&self, reg: CpuReg) -> u64 {
        self.cpu.reg(to_core_reg(reg))
    }

    fn set_reg(&mut self, reg: CpuReg, value: u64) {
        self.cpu.set_reg(to_core_reg(reg), value);
    }

    fn complete_port_in(&mut self, value: u64) {
        self.cpu.complete_port_in(value);
    }

    fn complete_mmio_read(&mut self, value: u64) {
        self.cpu.complete_mmio_read(value);
    }
}
