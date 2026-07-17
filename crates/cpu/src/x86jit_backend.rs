// SPDX-License-Identifier: AGPL-3.0-or-later
//! `x86jit`-backed CPU (§2.1, §3.2 seam 1).
//!
//! [`X86JitCpu`] wraps an `x86jit-core` [`Vm`] + [`Vcpu`] and maps the core's
//! [`Exit`] variants onto the backend-neutral [`CpuStop`]. It runs in **Long64**
//! mode (the serial skeleton, [`X86JitCpu::new`]) or in **Real16** mode (a PC/AT
//! boot that takes hardware interrupts, [`X86JitCpu::new_real16`]). It uses the
//! `x86jit` integration surface: `Exit::PortIo` (`in`/`out`), `RegionKind::Trap`
//! MMIO, and — in real mode — IVT-vectored hardware IRQ injection.
//!
//! `x86jit-core` is depended on through a git ref rather than a plain path so a
//! clean checkout or CI can resolve it without the local worktree present.

use x86jit_core::{CpuMode, Exit, PortDir, Prot, Reg, RegionKind, Vcpu, Vm, VmConfig};

use crate::{Cpu, CpuError, CpuReg, CpuStop};

/// A CPU backed by the `x86jit` interpreter core (§2.1).
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

    /// Build a **real-mode (16-bit)** CPU over a flat guest address space of
    /// `ram_bytes`, seed the four segment registers, load `code` at the linear
    /// address `(cs << 4) + ip`, and set the instruction pointer to `ip`.
    ///
    /// The whole space is mapped `RWX` `Ram` (the guest programs the IVT and the
    /// devices itself via ordinary stores/`out`); MMIO-trapped windows are added
    /// with [`X86JitCpu::map_mmio`] before execution. Code is written through the
    /// `Vm` API so the core's self-modifying-code tracking sees the write.
    ///
    /// In real mode a segment base is `selector << 4`; the embedder passes the
    /// selectors and this seeds CS/DS/ES/SS to the same value (the common flat
    /// low-memory layout a boot blob uses). The stack pointer is seeded from
    /// `segments.sp`. Hardware IRQs injected with [`Cpu::inject_irq`] are then
    /// delivered in-guest through the IVT (§5.2).
    ///
    /// The embedder seeds the segment and stack registers because the `x86jit`
    /// core does not lift `mov Sreg, r16` (it models segment *bases* for
    /// addressing, not selector-loading instructions); a real-mode boot blob
    /// therefore relies on this seeding instead of setting up its own segments.
    pub fn new_real16(
        ram_bytes: u64,
        cs: u16,
        ip: u16,
        code: &[u8],
        segments: RealSegments,
    ) -> Result<Self, CpuError> {
        let mut vm = Vm::new(VmConfig::flat(ram_bytes));
        vm.set_cpu_mode(CpuMode::Real16);
        vm.map(0, ram_bytes as usize, Prot::RWX, RegionKind::Ram)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))?;
        let load = (u64::from(cs) << 4) + u64::from(ip);
        vm.write_bytes(load, code)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))?;
        let mut cpu = vm.new_vcpu();
        cpu.set_reg(Reg::Cs, u64::from(cs));
        cpu.set_reg(Reg::Ds, u64::from(segments.ds));
        cpu.set_reg(Reg::Es, u64::from(segments.es));
        cpu.set_reg(Reg::Ss, u64::from(segments.ss));
        cpu.set_reg(Reg::Rsp, u64::from(segments.sp));
        cpu.set_reg(Reg::Rip, u64::from(ip));
        Ok(Self { vm, cpu })
    }

    /// Write `bytes` into guest RAM at linear address `addr` through the `Vm`
    /// API (so SMC tracking sees it). For an embedder that seeds the IVT or a
    /// device table into low memory before execution.
    pub fn write_mem(&mut self, addr: u64, bytes: &[u8]) -> Result<(), CpuError> {
        self.vm
            .write_bytes(addr, bytes)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
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

/// The data-segment selectors and stack pointer a real-mode guest starts with
/// (§17.6).
///
/// CS and IP are passed separately to [`X86JitCpu::new_real16`] (they fix the
/// code-fetch address); this groups DS/ES/SS and the initial SP. A boot blob that
/// runs from a single flat low-memory segment uses [`RealSegments::flat`], which
/// sets the three data segments equal to the code segment and picks a default
/// stack top clear of the IVT and typical boot code.
#[derive(Copy, Clone, Debug, Default)]
pub struct RealSegments {
    /// Data segment selector.
    pub ds: u16,
    /// Extra segment selector.
    pub es: u16,
    /// Stack segment selector.
    pub ss: u16,
    /// Initial stack pointer (offset within SS).
    pub sp: u16,
}

impl RealSegments {
    /// Default flat stack top: below typical boot code at `0x7C00` and above the
    /// IVT/BIOS data area, giving the guest a working stack without setup code.
    pub const DEFAULT_SP: u16 = 0x7000;

    /// All three data segments equal to `sel` with the default stack top — the
    /// flat low-memory layout a boot blob uses.
    pub fn flat(sel: u16) -> Self {
        Self {
            ds: sel,
            es: sel,
            ss: sel,
            sp: Self::DEFAULT_SP,
        }
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

    fn inject_irq(&mut self, vector: u8) {
        // Real mode only: the core queues the vector and delivers it in-guest
        // through the IVT at the next `run()` boundary once IF is set, no
        // completion is outstanding, and the STI shadow has elapsed. In Long64
        // this is inert (the core never sets `pending_irq` outside real mode).
        self.cpu.inject_irq(vector);
    }

    fn has_pending_irq(&self) -> bool {
        self.cpu.has_pending_irq()
    }

    fn retired(&self) -> u64 {
        self.cpu.retired_instructions()
    }
}
