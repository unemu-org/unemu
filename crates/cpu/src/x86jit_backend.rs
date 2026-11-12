// SPDX-License-Identifier: AGPL-3.0-or-later
//! `x86jit`-backed CPU (§2.1, §3.2 seam 1).
//!
//! [`X86JitCpu`] wraps an `x86jit-core` [`Vm`] + [`Vcpu`] and maps the core's
//! [`Exit`] variants onto the backend-neutral [`CpuStop`]. It runs in **Long64**
//! mode (adapter unit tests, [`X86JitCpu::new`]), **Real16** application mode
//! ([`X86JitCpu::new_real16_bare`]), or the **I286 machine** interpreter
//! ([`X86JitCpu::new_i286_bare`]). It uses the
//! `x86jit` integration surface: `Exit::PortIo` (`in`/`out` and each `ins`/`outs`
//! element), `RegionKind::Trap` MMIO, and — in real mode — IVT-vectored IRQ injection.
//!
//! `x86jit-core` is depended on through a git dependency pinned to an exact
//! revision rather than a plain path, so a clean checkout or CI resolves the same
//! CPU without any local x86jit checkout present.

use x86jit_core::{
    CpuMode, CpuModel, CpuState, ExecutionProfile, Exit, FpuModel, PortDir, Prot, Reg, RegionKind,
    ResetOptions, StepOptions, StepRunError, StepStop, Vcpu, Vm, VmConfig,
};

use crate::{Cpu, CpuError, CpuReg, CpuStepRun, CpuStepStop, CpuStop, DmaAccess, StepRunOptions};

/// A CPU backed by the `x86jit` interpreter core (§2.1).
///
/// Owns the guest [`Vm`] (its memory map) and the [`Vcpu`]. The machine loop
/// drives it through the [`Cpu`] trait; it never exposes `x86jit` types.
pub struct X86JitCpu {
    vm: Vm,
    cpu: Vcpu,
    dma_regions: Vec<(std::ops::Range<u64>, RegionKind)>,
    completed_steps: u64,
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
        Ok(Self {
            vm,
            cpu,
            dma_regions: vec![(0..ram_bytes, RegionKind::Ram)],
            completed_steps: 0,
        })
    }

    /// Build a **real-mode (16-bit)** CPU over a flat guest address space of
    /// `space_bytes` with **no regions mapped** — the embedder lays out the whole
    /// memory map itself with [`X86JitCpu::map_ram`], [`X86JitCpu::map_rom`], and
    /// [`X86JitCpu::map_mmio`].
    ///
    /// This is the constructor a full PC needs: its address space is a mosaic of
    /// conventional RAM, a trapped video window, an option-ROM scan area, and a
    /// masked BIOS ROM at the top — not the single flat RAM block the built-in
    /// demos use. Nothing is loaded here (a BIOS ROM is its own code, mapped with
    /// [`X86JitCpu::map_rom`]); this only seeds the segment and stack registers and
    /// points CS:IP at the reset entry.
    ///
    /// The segment registers are seeded here so that a guest entered at an
    /// arbitrary CS:IP (a test program or a reset vector) has defined DS, ES, SS
    /// and SP before its first instruction; the guest may reload them itself.
    pub fn new_real16_bare(
        space_bytes: u64,
        cs: u16,
        ip: u16,
        segments: RealSegments,
    ) -> Result<Self, CpuError> {
        let mut vm = Vm::new(VmConfig::flat(space_bytes));
        vm.set_cpu_mode(CpuMode::Real16);
        let mut cpu = vm.new_vcpu();
        cpu.set_reg(Reg::Cs, u64::from(cs));
        cpu.set_reg(Reg::Ds, u64::from(segments.ds));
        cpu.set_reg(Reg::Es, u64::from(segments.es));
        cpu.set_reg(Reg::Ss, u64::from(segments.ss));
        cpu.set_reg(Reg::Rsp, u64::from(segments.sp));
        cpu.set_reg(Reg::Rip, u64::from(ip));
        Ok(Self {
            vm,
            cpu,
            dma_regions: Vec::new(),
            completed_steps: 0,
        })
    }

    /// Build an Intel 80286 machine interpreter at a real-mode entry.
    ///
    /// Intel 80286 and 80287 Programmer's Reference Manual, §10.2–10.4:
    /// LMSW can enter protected mode; leaving it requires a processor reset.
    /// Unlike [`Self::new_real16_bare`], this profile retains descriptor caches
    /// and delivers real/protected exceptions in the guest through IVT/IDT.
    /// It enforces 286 instruction and segment limits. See
    /// `backlog/docs/design/protected-mode-gaps.md` §Context.
    /// The address space must include any reset ROM at physical FFFFF0h.
    pub fn new_i286_bare(
        space_bytes: u64,
        cs: u16,
        ip: u16,
        segments: RealSegments,
    ) -> Result<Self, CpuError> {
        let mut vm = Vm::new(VmConfig::flat(space_bytes));
        vm.set_cpu_model(CpuModel::I80286);
        vm.set_fpu_model(FpuModel::Present);
        let mut cpu = vm.new_vcpu();
        cpu.cpu = CpuState::legacy_real_entry([cs, segments.ds, segments.es, segments.ss, 0, 0]);
        cpu.cpu.legacy.cr0 = 0xFFF0;
        cpu.set_reg(Reg::Rsp, u64::from(segments.sp));
        cpu.set_reg(Reg::Rip, u64::from(ip));
        cpu.set_fpu_emulation(false);
        Ok(Self {
            vm,
            cpu,
            dma_regions: Vec::new(),
            completed_steps: 0,
        })
    }

    /// Map a conventional RAM window `[base, base + size)` as `RWX` `Ram`.
    ///
    /// For an embedder building a mosaic address space with
    /// [`X86JitCpu::new_real16_bare`]: low conventional memory and upper-memory
    /// probe areas can be RAM. Firmware scan space on catalogue boards uses
    /// read-only ROM mappings, including deliberately zero-filled gaps.
    pub fn map_ram(&mut self, base: u64, size: u64) -> Result<(), CpuError> {
        self.map_region(base, size as usize, Prot::RWX, RegionKind::Ram)
    }

    /// Map a masked **ROM** window at `base` sized to `blob` and load `blob` into
    /// it (a BIOS or option ROM).
    ///
    /// The region is `RegionKind::Rom`: fetches and reads run at RAM speed and
    /// code executes straight out of it, while a guest store into it is silently
    /// dropped (the masked-ROM contract) rather than faulting. Contents are loaded
    /// through the `Vm` write path, which bypasses the region kind — the only way
    /// to populate a ROM, which is immutable to the guest thereafter.
    pub fn map_rom(&mut self, base: u64, blob: &[u8]) -> Result<(), CpuError> {
        self.map_region(base, blob.len(), Prot::RX, RegionKind::Rom)?;
        self.vm
            .write_bytes(base, blob)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
    }

    /// Map one immutable region filled with a repeated byte, using at most
    /// 4 KiB of temporary storage. One region preserves accesses spanning fill
    /// chunks; mapping each chunk separately would introduce artificial faults.
    /// See `backlog/docs/design/machine-config.md` §MemoryConfig.
    pub fn map_rom_filled(&mut self, base: u64, size: u64, value: u8) -> Result<(), CpuError> {
        self.map_region(base, size as usize, Prot::RX, RegionKind::Rom)?;
        let fill = [value; 4096];
        for offset in (0..size).step_by(fill.len()) {
            let count = (size - offset).min(fill.len() as u64) as usize;
            self.write_mem(base + offset, &fill[..count])?;
        }
        Ok(())
    }

    /// Write `bytes` into guest RAM at linear address `addr` through the `Vm`
    /// API (so SMC tracking sees it). For an embedder that seeds the IVT or a
    /// device table into low memory before execution.
    pub fn write_mem(&mut self, addr: u64, bytes: &[u8]) -> Result<(), CpuError> {
        self.vm
            .write_bytes(addr, bytes)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
    }

    /// Read `buf.len()` bytes of guest memory at linear address `addr` through the
    /// `Vm` API. For a runner that reads back the bytes at the stopping CS:IP to
    /// disassemble where a run ended.
    pub fn read_mem(&self, addr: u64, buf: &mut [u8]) -> Result<(), CpuError> {
        self.vm
            .read_bytes(addr, buf)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
    }

    /// The current code-segment selector (real mode). The [`Cpu`] trait exposes
    /// only the general-purpose registers and RIP; a runner that wants the linear
    /// CS:IP of the stopping instruction reads the segment here.
    pub fn code_segment(&self) -> u64 {
        self.cpu.reg(Reg::Cs)
    }

    /// Map a memory-mapped-I/O (trapped) window `[base, base + size)`.
    ///
    /// Loads/stores in this range yield `Exit::MmioRead`/`Exit::MmioWrite`,
    /// surfaced as [`CpuStop::MmioRead`]/[`CpuStop::MmioWrite`] (§4.2, §5.2).
    pub fn map_mmio(&mut self, base: u64, size: u64) -> Result<(), CpuError> {
        self.map_region(base, size as usize, Prot::RW, RegionKind::Trap)
    }

    fn map_region(
        &mut self,
        base: u64,
        size: usize,
        prot: Prot,
        kind: RegionKind,
    ) -> Result<(), CpuError> {
        self.vm
            .map(base, size, prot, kind)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))?;
        // Vm's host loader API bypasses region kinds. Keep the successful maps
        // here so DMA cycles obey the guest map without involving any devices.
        self.dma_regions.push((base..base + size as u64, kind));
        Ok(())
    }

    fn dma_region_kind(&self, addr: u32) -> Option<RegionKind> {
        self.dma_regions
            .iter()
            .find(|(range, _)| range.contains(&u64::from(addr)))
            .map(|(_, kind)| *kind)
    }
}

/// The data-segment selectors and stack pointer a real-mode guest starts with
/// (§17.6).
///
/// CS and IP are passed separately to [`X86JitCpu::new_real16_bare`] (they fix the
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

impl X86JitCpu {
    fn map_exit(&self, exit: Exit) -> Result<CpuStop, CpuError> {
        match exit {
            Exit::PortIo {
                port,
                size,
                dir,
                value,
                ..
            } => match dir {
                PortDir::In => Ok(CpuStop::PortIn { port, size }),
                PortDir::Out => Ok(CpuStop::PortOut { port, size, value }),
            },
            // Each MMIO exit is one complete transfer at its own address and
            // width. The fields skipped here only place that transfer inside a
            // wider operand, which the core delivers as several exits at the same
            // instruction, so servicing `addr`/`size` alone is still exact.
            Exit::MmioRead { addr, size, .. } => Ok(CpuStop::MmioRead { addr, size }),
            Exit::MmioWrite {
                addr, size, value, ..
            } => Ok(CpuStop::MmioWrite { addr, size, value }),
            Exit::Hlt => Ok(CpuStop::Halt),
            Exit::Shutdown { .. } => Ok(CpuStop::Shutdown),
            Exit::Exception {
                addr,
                vector,
                ref metadata,
                ..
            } if self.vm.execution_profile() == ExecutionProfile::Machine => {
                Err(CpuError::Fault {
                    vector,
                    error_code: metadata.as_deref().and_then(|fault| {
                        // The 286 supplies a complete 16-bit error code. The
                        // core's 32-bit projection requires a wider known mask.
                        match fault.error_code() {
                            x86jit_core::fault::ErrorCode::Bits {
                                value, known_mask, ..
                            } if known_mask & 0xFFFF == 0xFFFF => Some(value & 0xFFFF),
                            _ => None,
                        }
                    }),
                    cs: self.cpu.cpu.cs,
                    ip: addr,
                })
            }
            Exit::Exception { vector, .. } => Ok(CpuStop::Exception { vector }),
            Exit::UnsupportedSystemContext {
                addr, operation, ..
            } => Err(CpuError::UnsupportedSystem {
                operation: format!("{operation:?}"),
                cs: self.cpu.cpu.cs,
                ip: addr,
            }),
            Exit::BudgetExhausted => Ok(CpuStop::BudgetExhausted),
            Exit::UnmappedMemory { addr, .. } => Err(CpuError::Unmapped { addr }),
            Exit::UnknownInstruction { addr, .. } => Err(CpuError::UnsupportedInstruction { addr }),
            // Only `syscall`, or `int 0x80` outside real mode, end up
            // here: in real mode every `int n` is delivered inside the guest
            // through the IVT, and the long-mode serial programs make no system
            // calls. The evidence retains the instruction address after RIP
            // has advanced, so diagnostics identify the opcode to implement.
            Exit::Syscall => {
                let evidence =
                    self.cpu
                        .syscall_evidence()
                        .ok_or_else(|| CpuError::UnexpectedExit {
                            what: "syscall exit without instruction evidence".to_string(),
                        })?;
                Err(CpuError::UnsupportedInstruction {
                    addr: evidence.rip(),
                })
            }
            // Nothing in this backend requests a host yield, and the interpreter
            // the machine runs on never fills a JIT code cache; either stop means
            // the embedding is not what this backend expects.
            Exit::CodeSpaceExhausted { addr, .. } => Err(CpuError::UnexpectedExit {
                what: format!("jit code space exhausted at {addr:#x}"),
            }),
            Exit::Interrupted => Err(CpuError::UnexpectedExit {
                what: "interrupted by a host yield request".to_string(),
            }),
            // The core may add exit kinds; report one by name rather than guess.
            other => Err(CpuError::UnexpectedExit {
                what: format!("{other:?}"),
            }),
        }
    }
}

impl Cpu for X86JitCpu {
    fn reset(&mut self) -> Result<(), CpuError> {
        self.cpu
            .reset_execution_state(ResetOptions::default().with_x87_init(true));
        let state = &mut self.cpu.cpu;
        if self.vm.execution_profile() == ExecutionProfile::Machine {
            // Reset must discard protected caches as well as pending handoffs.
            // Preserve GPRs, whose hardware reset contents are unspecified.
            let reset = CpuState::legacy_real_entry([0xF000, 0, 0, 0, 0, 0]);
            state.legacy = reset.legacy;
            state.legacy.cr0 = 0xFFF0;
            state.legacy.segments[0].base = 0xFF0000;
            state.legacy.reset_fetch = 1;
            state.fs = 0;
            state.gs = 0;
        }
        state.cs = 0xF000;
        state.rip = 0xFFF0;
        state.ds = 0;
        state.es = 0;
        state.ss = 0;
        state.flags.set_flags16(0x0002);
        state.rflags_system = 0;
        Ok(())
    }

    fn run(&mut self, budget: Option<u64>) -> Result<CpuStop, CpuError> {
        let exit = self.cpu.run(&self.vm, budget);
        self.map_exit(exit)
    }

    fn run_steps(
        &mut self,
        max_steps: u64,
        options: StepRunOptions,
    ) -> Result<CpuStepRun, CpuError> {
        let run = self
            .cpu
            .run_steps(
                &self.vm,
                max_steps,
                StepOptions::default().with_stop_when_irq_ready(options.stop_when_irq_ready),
            )
            .map_err(|error| match error {
                StepRunError::UnsupportedMode => CpuError::UnexpectedExit {
                    what: "step-bounded execution is unsupported by this backend or mode"
                        .to_string(),
                },
                StepRunError::PendingPortIn => CpuError::UnexpectedExit {
                    what: "pending port input must be completed before step-bounded execution"
                        .to_string(),
                },
                other => CpuError::UnexpectedExit {
                    what: format!("{other:?}"),
                },
            })?;
        let steps = run.steps();
        self.completed_steps += steps;
        let stop = match run.into_stop() {
            StepStop::Exit(exit) => CpuStepStop::Exit(self.map_exit(exit)?),
            StepStop::IrqReady => CpuStepStop::IrqReady,
            other => {
                return Err(CpuError::UnexpectedExit {
                    what: format!("{other:?}"),
                })
            }
        };
        Ok(CpuStepRun { steps, stop })
    }

    fn completed_steps(&self) -> u64 {
        self.completed_steps
    }

    fn irq_ready(&self) -> bool {
        self.cpu.irq_ready()
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

    fn complete_mmio_write(&mut self) {
        self.cpu.complete_mmio_write();
    }

    fn inject_irq(&mut self, vector: u8) {
        // The core queues vectors in every mode, but only Real16 delivers them
        // through the IVT once IF, completion and STI/SS-shadow gates allow it.
        self.cpu.inject_irq(vector);
    }

    fn interrupts_enabled(&self) -> bool {
        self.cpu.flags().if_
    }

    fn has_pending_irq(&self) -> bool {
        self.cpu.has_pending_irq()
    }

    fn guest_dma_read(&self, addr: u32) -> DmaAccess<u8> {
        if self.dma_region_kind(addr) == Some(RegionKind::Trap) {
            return DmaAccess::Mmio;
        }
        let mut buf = [0xFF; 1];
        let _ = self.vm.read_bytes(u64::from(addr), &mut buf);
        DmaAccess::Complete(buf[0])
    }

    fn guest_dma_write(&mut self, addr: u32, value: u8) -> DmaAccess<()> {
        match self.dma_region_kind(addr) {
            Some(RegionKind::Trap) => DmaAccess::Mmio,
            Some(RegionKind::Ram) => {
                // Vm records note_write, invalidating code overwritten by DMA.
                let _ = self.vm.write_bytes(u64::from(addr), &[value]);
                DmaAccess::Complete(())
            }
            _ => DmaAccess::Complete(()),
        }
    }

    fn retired(&self) -> u64 {
        self.cpu.retired_instructions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i286_guest(code: &[u8]) -> X86JitCpu {
        let mut cpu =
            X86JitCpu::new_i286_bare(0x100_0000, 0, 0x1000, RealSegments::flat(0)).unwrap();
        cpu.map_ram(0, 0x10000).unwrap();
        cpu.write_mem(0x1000, code).unwrap();
        cpu
    }

    #[test]
    fn i286_single_step_probe_traps_after_pop_ds_and_restores_the_stack() {
        // Intel 80286 Programmer's Reference Manual, §9.7.1 and Appendix B
        // MOV/POP: only SS loads inhibit the next single-step trap.
        let mut cpu = i286_guest(&[]);
        cpu.set_reg(CpuReg::Rip, 0xA7D0);
        cpu.write_mem(4, &[0x35, 0xA8, 0, 0]).unwrap();
        cpu.write_mem(
            0xA7D0,
            &[
                0x9C, 0x9C, 0x58, 0x0D, 0, 1, 0x50, 0x31, 0xC0, 0x9D, 0x90, 0x1E, 0x1F, 0x90, 0x9D,
                0xF4,
            ],
        )
        .unwrap();
        cpu.write_mem(
            0xA835,
            &[
                0x55, 0x89, 0xE5, 0x81, 0x7E, 2, 0xDD, 0xA7, 0x75, 4, 0xB0, 1, 0xEB, 9, 0x81, 0x7E,
                2, 0xDC, 0xA7, 0x75, 2, 0xB0, 2, 0x5D, 0xCF,
            ],
        )
        .unwrap();
        let mut returns = Vec::new();
        let mut halted = false;
        for _ in 0..100 {
            if cpu.reg(CpuReg::Rip) == 0xA835 {
                let mut frame = [0; 6];
                cpu.read_mem(cpu.reg(CpuReg::Rsp), &mut frame).unwrap();
                returns.push(u16::from_le_bytes([frame[0], frame[1]]));
                assert_eq!(cpu.cpu.cpu.rflags_system & 0x100, 0);
                assert!(!cpu.interrupts_enabled());
            }
            let run = cpu.run_steps(1, StepRunOptions::default()).unwrap();
            if run.stop == CpuStepStop::Exit(CpuStop::Halt) {
                halted = true;
                break;
            }
            assert_eq!(run.stop, CpuStepStop::Exit(CpuStop::BudgetExhausted));
        }
        assert!(halted);
        assert_eq!(returns, [0xA7DB, 0xA7DC, 0xA7DD, 0xA7DE, 0xA7DF]);
        assert_eq!(cpu.reg(CpuReg::Rax), 1);
        assert_eq!(cpu.reg(CpuReg::Rsp), u64::from(RealSegments::DEFAULT_SP));
        assert_eq!(cpu.completed_steps(), 54);
        assert_eq!(cpu.retired(), 54);
    }

    #[test]
    fn i286_real_aad_x87_and_rep_ins_resume_at_element_boundaries() {
        let mut cpu = i286_guest(&[
            0xB8, 1, 1, 0xD5, 0x10, 0xA3, 0, 5, // AAD with a nondecimal base
            0xDB, 0xE3, 0xD9, 0x3E, 2, 5, // FNINIT; FNSTCW [0502h]
            0xBA, 0x60, 0, 0xBF, 4, 5, 0xB9, 2, 0, 0xF3, 0x6C, 0xF4,
        ]);
        assert_eq!(cpu.vm.cpu_model(), Some(CpuModel::I80286));
        assert_eq!(
            cpu.vm.exception_delivery(),
            x86jit_core::ExceptionDelivery::InGuest
        );
        assert_eq!(cpu.vm.fpu_model(), FpuModel::Present);
        let options = StepRunOptions::default();
        let input = CpuStepStop::Exit(CpuStop::PortIn {
            port: 0x60,
            size: 1,
        });
        assert_eq!(cpu.run_steps(9, options).unwrap().stop, input);
        assert_eq!(cpu.retired(), 8);
        cpu.complete_port_in(0xA7);
        assert_eq!(cpu.run_steps(1, options).unwrap().stop, input);
        assert_eq!(cpu.retired(), 9);
        cpu.complete_port_in(0x5C);
        assert_eq!(
            cpu.run_steps(1, options).unwrap().stop,
            CpuStepStop::Exit(CpuStop::Halt)
        );
        let mut markers = [0; 6];
        cpu.read_mem(0x500, &mut markers).unwrap();
        assert_eq!(markers, [0x11, 0, 0x7F, 3, 0xA7, 0x5C]);
        assert_eq!(cpu.completed_steps(), 11);
        assert_eq!(cpu.retired(), 10);
    }

    #[test]
    fn i286_bad_protected_selector_returns_typed_fault() {
        let mut vm = Vm::new(VmConfig::flat(0x10000));
        vm.set_cpu_model(CpuModel::I80286);
        vm.set_exception_delivery(x86jit_core::ExceptionDelivery::Outward);
        let core = vm.new_vcpu();
        let mut cpu = X86JitCpu {
            vm,
            cpu: core,
            dma_regions: Vec::new(),
            completed_steps: 0,
        };
        cpu.map_ram(0, 0x10000).unwrap();
        cpu.set_reg(CpuReg::Rip, 0x1000);
        cpu.write_mem(
            0x1000,
            &[
                0xB8, 1, 0, 0x0F, 0x01, 0xF0, // MOV AX,1; LMSW AX
                0xEA, 0, 0x12, 8, 0, // JMP 0008:1200, beyond the empty GDT
            ],
        )
        .unwrap();
        let error = cpu.run_steps(20, StepRunOptions::default()).unwrap_err();
        assert!(
            matches!(
                error,
                CpuError::Fault {
                    vector: 13,
                    error_code: Some(8),
                    cs: 0,
                    ip: 0x1006
                }
            ),
            "{error}"
        );
        assert_eq!(cpu.retired(), 2);
        assert_eq!(cpu.completed_steps(), 2);
        assert_eq!(cpu.reg(CpuReg::Rip), 0x1006);
    }

    #[test]
    fn i286_real_word_at_ffff_delivers_gp_through_ivt_for_ds_and_ss() {
        // Intel 80286 and 80287 Programmer's Reference Manual, §9.6.2:
        // a word crossing a real-mode segment limit reports vector 13 even for SS.
        for code in [&[0xA1, 0xFF, 0xFF][..], &[0x36, 0xA1, 0xFF, 0xFF][..]] {
            let mut cpu = i286_guest(code);
            cpu.write_mem(13 * 4, &[0, 0x18, 0, 0]).unwrap();
            cpu.write_mem(0x1800, &[0xF4]).unwrap();
            let run = cpu.run_steps(1, StepRunOptions::default()).unwrap();
            assert_eq!(run.stop, CpuStepStop::Exit(CpuStop::Halt));
            assert_eq!(cpu.reg(CpuReg::Rip), 0x1801);
            assert_eq!(cpu.reg(CpuReg::Rax), 0);
            assert_eq!(
                cpu.reg(CpuReg::Rsp),
                u64::from(RealSegments::DEFAULT_SP) - 6
            );
            let mut frame = [0; 4];
            cpu.read_mem(cpu.reg(CpuReg::Rsp), &mut frame).unwrap();
            assert_eq!(frame, [0, 0x10, 0, 0]);
            assert_eq!(cpu.retired(), 1);
        }
    }

    #[test]
    fn i286_reset_discards_protected_caches_and_fetches_high_rom() {
        let mut cpu = i286_guest(&[0xB8, 1, 0, 0x0F, 0x01, 0xF0]);
        cpu.run_steps(2, StepRunOptions::default()).unwrap();
        assert_eq!(cpu.cpu.cpu.legacy.cr0 & 1, 1);
        cpu.cpu.cpu.legacy.segments[1].base = 0x123000;
        cpu.inject_irq(0x20);
        cpu.map_rom(0xFFFFF0, &[0xEA, 0, 0x15, 0, 0]).unwrap();
        cpu.write_mem(0x1500, &[0x0F, 0x01, 0xE0, 0xF4]).unwrap(); // SMSW AX; HLT
        let retired = cpu.retired();
        cpu.reset().unwrap();
        assert_eq!(cpu.retired(), retired);
        assert_eq!(cpu.cpu.cpu.legacy.cr0, 0xFFF0);
        assert_eq!(cpu.cpu.cpu.legacy.segments[0].base, 0xFF0000);
        assert_eq!(cpu.cpu.cpu.legacy.segments[1].base, 0);
        assert!(!cpu.has_pending_irq());
        assert_eq!(
            cpu.run_steps(3, StepRunOptions::default()).unwrap().stop,
            CpuStepStop::Exit(CpuStop::Halt)
        );
        assert_eq!(cpu.reg(CpuReg::Rax), 0xFFF0);
        assert_eq!(cpu.cpu.cpu.legacy.reset_fetch, 0);
        assert_eq!(cpu.retired(), retired + 3);
    }

    #[test]
    fn filled_rom_keeps_chunk_boundaries_inside_one_immutable_region() {
        let mut cpu =
            X86JitCpu::new_real16_bare(0x10000, 0, 0x1000, RealSegments::flat(0)).unwrap();
        cpu.map_ram(0, 0x2000).unwrap();
        cpu.map_rom_filled(0x2000, 0x2003, 0xBA).unwrap();
        cpu.write_mem(
            0x1000,
            &[0xC7, 0x06, 0xFF, 0x2F, 0x34, 0x12, 0xA1, 0xFF, 0x2F, 0xF4],
        )
        .unwrap();
        assert_eq!(cpu.run(Some(100)).unwrap(), CpuStop::Halt);
        assert_eq!(cpu.reg(CpuReg::Rax) & 0xFFFF, 0xBABA);
        assert_eq!(cpu.guest_dma_write(0x4002, 0), DmaAccess::Complete(()));
        let mut value = [0];
        cpu.read_mem(0x4002, &mut value).unwrap();
        assert_eq!(value, [0xBA]);
    }

    #[test]
    fn reset_discards_pending_ins_and_mid_rep_without_rewinding_accounting() {
        for answered in [true, false] {
            let mut cpu =
                X86JitCpu::new_real16_bare(0x10_0000, 0, 0x7000, RealSegments::flat(0)).unwrap();
            cpu.map_ram(0, 0x10_0000).unwrap();
            cpu.write_mem(0x7000, &[0xF3, 0x6C, 0xF4]).unwrap();
            cpu.write_mem(0xFFFF0, &[0xF4]).unwrap();
            cpu.set_reg(CpuReg::Rcx, 3);
            cpu.set_reg(CpuReg::Rdi, 0x500);
            cpu.set_reg(CpuReg::Rdx, 0x60);
            let run = cpu.run_steps(1, StepRunOptions::default()).unwrap();
            assert_eq!(
                run.stop,
                CpuStepStop::Exit(CpuStop::PortIn {
                    port: 0x60,
                    size: 1
                })
            );
            if answered {
                cpu.complete_port_in(0xA7);
            }
            cpu.inject_irq(0x20);
            cpu.cpu.cpu.fpu_cw = 0;
            cpu.cpu.cpu.fpu_empty = 0;
            let retired = cpu.retired();
            let steps = cpu.completed_steps();
            let gpr = cpu.cpu.cpu.gpr;
            cpu.reset().unwrap();
            assert_eq!(cpu.cpu.cpu.gpr, gpr);
            assert_eq!(cpu.cpu.cpu.flags.to_flags16(), 2);
            assert_eq!((cpu.cpu.cpu.cs, cpu.cpu.cpu.rip), (0xF000, 0xFFF0));
            assert_eq!((cpu.cpu.cpu.ds, cpu.cpu.cpu.es, cpu.cpu.cpu.ss), (0, 0, 0));
            assert_eq!(cpu.retired(), retired);
            assert_eq!(cpu.completed_steps(), steps);
            let run = cpu.run_steps(1, StepRunOptions::default()).unwrap();
            let mut marker = [0xFF; 3];
            cpu.read_mem(0x500, &mut marker).unwrap();
            assert_eq!(marker, [0; 3], "no deferred INS store may survive reset");
            assert_eq!(run.stop, CpuStepStop::Exit(CpuStop::Halt));
            assert_eq!(cpu.reg(CpuReg::Rcx), gpr[1]);
            assert_eq!(cpu.reg(CpuReg::Rdi), gpr[7]);
            assert_eq!(cpu.retired(), retired + 1);
            assert_eq!(cpu.completed_steps(), steps + 1);
            assert!(!cpu.has_pending_irq());
            assert_eq!(cpu.cpu.cpu.fpu_cw, 0x037F);
            assert_eq!(cpu.cpu.cpu.fpu_empty, 0xFF);
        }
    }

    #[test]
    fn answered_ins_store_fault_retries_without_another_port_read() {
        let mut cpu = X86JitCpu::new_real16_bare(
            0x30000,
            0,
            0x7C00,
            RealSegments {
                es: 0x2000,
                ..RealSegments::flat(0)
            },
        )
        .unwrap();
        cpu.map_ram(0, 0x10000).unwrap();
        // INSW; HLT. ES points to an unmapped destination until the retry.
        cpu.write_mem(0x7C00, &[0x6D, 0xF4]).unwrap();
        cpu.set_reg(CpuReg::Rdx, 0x300);
        let options = StepRunOptions::default();
        assert_eq!(
            cpu.run_steps(1, options).unwrap(),
            CpuStepRun {
                steps: 1,
                stop: CpuStepStop::Exit(CpuStop::PortIn {
                    port: 0x300,
                    size: 2
                }),
            }
        );
        cpu.complete_port_in(0xABCD);
        assert_eq!(cpu.reg(CpuReg::Rip), 0x7C00);
        assert_eq!(cpu.reg(CpuReg::Rdi), 0);
        assert!(matches!(
            cpu.run_steps(1, options),
            Err(CpuError::Unmapped { addr: 0x20000 })
        ));
        assert_eq!(cpu.completed_steps(), 1);
        cpu.map_ram(0x20000, 0x10000).unwrap();
        assert_eq!(
            cpu.run_steps(1, options).unwrap(),
            CpuStepRun {
                steps: 1,
                stop: CpuStepStop::Exit(CpuStop::Halt),
            }
        );
        let mut word = [0; 2];
        cpu.read_mem(0x20000, &mut word).unwrap();
        assert_eq!(word, [0xCD, 0xAB]);
        assert_eq!(cpu.reg(CpuReg::Rdi), 2);
        assert_eq!(cpu.reg(CpuReg::Rip), 0x7C02);
        assert_eq!(cpu.completed_steps(), 2);
        assert_eq!(cpu.retired(), 2);
    }

    #[test]
    fn readiness_option_stops_after_sti_protected_rep_iteration() {
        let mut cpu =
            X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).unwrap();
        cpu.map_ram(0, 0x10000).unwrap();
        // MOV CX, 3; STI; REP STOSB; HLT.
        cpu.write_mem(0x7C00, &[0xB9, 3, 0, 0xFB, 0xF3, 0xAA, 0xF4])
            .unwrap();
        cpu.set_reg(CpuReg::Rdi, 0x600);
        let options = StepRunOptions {
            stop_when_irq_ready: true,
        };
        assert_eq!(
            cpu.run_steps(64, options).unwrap(),
            CpuStepRun {
                steps: 3,
                stop: CpuStepStop::IrqReady,
            }
        );
        assert_eq!(cpu.retired(), 2);
        assert_eq!(cpu.reg(CpuReg::Rcx), 2);
        assert_eq!(cpu.reg(CpuReg::Rip), 0x7C04);
        assert_eq!(
            cpu.run_steps(0, options).unwrap(),
            CpuStepRun {
                steps: 0,
                stop: CpuStepStop::Exit(CpuStop::BudgetExhausted),
            }
        );
        assert_eq!(
            cpu.run_steps(64, options).unwrap(),
            CpuStepRun {
                steps: 0,
                stop: CpuStepStop::IrqReady,
            }
        );
        // Delivery precedes the ready stop and adds no step to the handler's HLT.
        cpu.write_mem(0x20, &[0, 0x80, 0, 0]).unwrap();
        cpu.write_mem(0x8000, &[0xF4]).unwrap();
        cpu.inject_irq(8);
        assert_eq!(
            cpu.run_steps(64, options).unwrap(),
            CpuStepRun {
                steps: 1,
                stop: CpuStepStop::Exit(CpuStop::Halt),
            }
        );
        assert!(!cpu.has_pending_irq());
        assert_eq!(cpu.completed_steps(), 4);
        assert_eq!(cpu.reg(CpuReg::Rcx), 2);
    }

    #[test]
    fn readiness_option_waits_for_port_and_mmio_completion() {
        let options = StepRunOptions {
            stop_when_irq_ready: true,
        };
        // The IN or MMIO load is the instruction protected by STI.
        let mut input = real_mode_cpu(&[0xFB, 0xE4, 0x80, 0xF4]);
        let run = input.run_steps(64, options).unwrap();
        assert_eq!(run.steps, 2);
        assert!(matches!(
            run.stop,
            CpuStepStop::Exit(CpuStop::PortIn { .. })
        ));
        assert!(!input.irq_ready());
        input.complete_port_in(0x42);
        assert_eq!(
            input.run_steps(64, options).unwrap(),
            CpuStepRun {
                steps: 0,
                stop: CpuStepStop::IrqReady,
            }
        );
        assert_eq!(input.reg(CpuReg::Rax), 0x42);

        let mut mmio = real_mode_cpu(&[0xFB, 0xA0, 0, 0, 0xF4]);
        let run = mmio.run_steps(64, options).unwrap();
        assert_eq!(run.steps, 1);
        assert!(matches!(
            run.stop,
            CpuStepStop::Exit(CpuStop::MmioRead { .. })
        ));
        assert!(!mmio.irq_ready());
        mmio.complete_mmio_read(0x43);
        assert_eq!(
            mmio.run_steps(64, options).unwrap(),
            CpuStepRun {
                steps: 1,
                stop: CpuStepStop::IrqReady,
            }
        );
        assert_eq!(mmio.reg(CpuReg::Rax), 0x43);
    }

    #[test]
    fn completed_steps_survives_error_exits_and_zero_budget_calls() {
        for unsupported in [false, true] {
            let mut cpu =
                X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).unwrap();
            cpu.map_ram(0, 0x8000).unwrap();
            // MOV CX, 3; REP STOSB; NOP, then a faulting unit with no charge.
            let mut code = vec![0xB9, 3, 0, 0xF3, 0xAA, 0x90];
            code.extend_from_slice(if unsupported {
                // MOV EAX, CR0 does not exist on an 80286 and this pin does not
                // lift it in Real16. If a future core lifts control-register
                // moves, the assertion below must fail loudly; choose another
                // unsupported encoding then.
                &[0x0F, 0x20, 0xC0]
            } else {
                &[0xA0, 0, 0x90]
            });
            cpu.write_mem(0x7C00, &code).unwrap();
            cpu.set_reg(CpuReg::Rdi, 0x600);
            assert_eq!(cpu.completed_steps(), 0);
            assert_eq!(
                cpu.run_steps(2, StepRunOptions::default()).unwrap().steps,
                2
            );
            assert_eq!(cpu.completed_steps(), 2);
            assert_eq!(cpu.retired(), 1);
            let error = cpu.run_steps(64, StepRunOptions::default()).unwrap_err();
            if unsupported {
                assert!(matches!(error, CpuError::UnsupportedInstruction { .. }));
            } else {
                assert!(matches!(error, CpuError::Unmapped { addr: 0x9000 }));
            }
            assert_eq!(cpu.completed_steps(), 5);
            assert_eq!(cpu.retired(), 3);
            assert_eq!(
                cpu.run_steps(0, StepRunOptions::default()).unwrap().steps,
                0
            );
            assert_eq!(cpu.completed_steps(), 5);
        }
    }

    #[test]
    fn run_steps_cuts_inside_cached_real16_block() {
        let mut code = vec![0x40; 100]; // inc ax
        code.extend_from_slice(&[0xE6, 0x80, 0xF4]);
        let mut cpu = real_mode_cpu(&code);
        assert!(matches!(
            run_to_stop(&mut cpu).unwrap(),
            CpuStop::PortOut { .. }
        ));
        cpu.set_reg(CpuReg::Rip, 0x7C00);
        cpu.set_reg(CpuReg::Rax, 0);
        let before = cpu.retired();
        for n in [0, 7, 1, 9] {
            let ip = cpu.reg(CpuReg::Rip);
            let ax = cpu.reg(CpuReg::Rax);
            let run = cpu.run_steps(n, StepRunOptions::default()).unwrap();
            assert_eq!(
                run,
                crate::CpuStepRun {
                    steps: n,
                    stop: crate::CpuStepStop::Exit(CpuStop::BudgetExhausted),
                }
            );
            assert_eq!(cpu.reg(CpuReg::Rip), ip + n);
            assert_eq!(cpu.reg(CpuReg::Rax), ax + n);
        }
        assert_eq!(cpu.retired() - before, 17);
    }

    #[test]
    fn run_steps_attributes_out_to_last_step_and_mmio_to_next_step() {
        let mut out = real_mode_cpu(&[0x90, 0xE6, 0x80, 0xF4]);
        let run = out.run_steps(2, StepRunOptions::default()).unwrap();
        assert_eq!(run.steps, 2);
        assert_eq!(
            run.stop,
            crate::CpuStepStop::Exit(CpuStop::PortOut {
                port: 0x80,
                size: 1,
                value: 0,
            })
        );
        assert_eq!(out.reg(CpuReg::Rip), 0x7C03);
        assert_eq!(out.retired(), 2);

        for (opcode, stop) in [
            (
                0xA0,
                CpuStop::MmioRead {
                    addr: VIDEO_BASE,
                    size: 1,
                },
            ),
            (
                0xA2,
                CpuStop::MmioWrite {
                    addr: VIDEO_BASE,
                    size: 1,
                    value: 0,
                },
            ),
        ] {
            let mut cpu = real_mode_cpu(&[0x90, opcode, 0, 0, 0xF4]);
            let run = cpu.run_steps(2, StepRunOptions::default()).unwrap();
            assert_eq!(run.steps, 1);
            assert_eq!(run.stop, crate::CpuStepStop::Exit(stop));
            assert_eq!(cpu.reg(CpuReg::Rip), 0x7C01);
            assert_eq!(cpu.retired(), 1);
            match stop {
                CpuStop::MmioRead { .. } => cpu.complete_mmio_read(0x42),
                CpuStop::MmioWrite { .. } => cpu.complete_mmio_write(),
                _ => unreachable!(),
            }
            let replay = cpu.run_steps(1, StepRunOptions::default()).unwrap();
            assert_eq!(replay.steps, 1);
            assert_eq!(
                replay.stop,
                crate::CpuStepStop::Exit(CpuStop::BudgetExhausted)
            );
            assert_eq!(cpu.reg(CpuReg::Rip), 0x7C04);
        }
    }

    #[test]
    fn irq_ready_waits_for_instruction_after_mov_ss() {
        let mut cpu = real_mode_cpu(&[0x8E, 0xD0, 0x90, 0xF4]); // mov ss, ax; nop; hlt
        let mut flags = cpu.cpu.flags();
        flags.if_ = true;
        cpu.cpu.set_flags(flags);
        assert!(cpu.irq_ready());
        assert_eq!(
            cpu.run_steps(1, StepRunOptions::default()).unwrap().steps,
            1
        );
        assert!(cpu.interrupts_enabled());
        assert!(!cpu.irq_ready());
        cpu.run_steps(0, StepRunOptions::default()).unwrap();
        assert!(!cpu.irq_ready());
        assert_eq!(
            cpu.run_steps(1, StepRunOptions::default()).unwrap().steps,
            1
        );
        assert!(cpu.irq_ready());
        assert!(!cpu.has_pending_irq());
    }

    #[test]
    fn run_steps_rejects_long64_without_guest_changes() {
        let mut cpu = X86JitCpu::new(0x1000, 0, &[0x90, 0xF4]).unwrap();
        for n in [0, 1] {
            assert!(matches!(
                cpu.run_steps(n, StepRunOptions::default()),
                Err(CpuError::UnexpectedExit { what }) if what == "step-bounded execution is unsupported by this backend or mode"
            ));
            assert_eq!(cpu.reg(CpuReg::Rip), 0);
            assert_eq!(cpu.retired(), 0);
            assert!(!cpu.irq_ready());
        }
    }

    #[test]
    fn run_steps_requires_port_in_completion() {
        let mut cpu = real_mode_cpu(&[0xE4, 0x80, 0xF4]);
        let mut flags = cpu.cpu.flags();
        flags.if_ = true;
        cpu.cpu.set_flags(flags);
        let run = cpu.run_steps(1, StepRunOptions::default()).unwrap();
        assert_eq!(run.steps, 1);
        assert_eq!(
            run.stop,
            crate::CpuStepStop::Exit(CpuStop::PortIn {
                port: 0x80,
                size: 1
            })
        );
        assert!(!cpu.irq_ready());
        assert_eq!(
            cpu.run_steps(0, StepRunOptions::default()).unwrap().steps,
            0
        );
        assert!(
            matches!(cpu.run_steps(1, StepRunOptions::default()), Err(CpuError::UnexpectedExit { what }) if what == "pending port input must be completed before step-bounded execution")
        );
        assert_eq!(cpu.reg(CpuReg::Rip), 0x7C02);
        assert_eq!(cpu.retired(), 1);
        cpu.complete_port_in(0x42);
        assert!(cpu.irq_ready());
        assert_eq!(cpu.retired(), 1);
        let run = cpu.run_steps(1, StepRunOptions::default()).unwrap();
        assert_eq!(run.steps, 1);
        assert_eq!(run.stop, crate::CpuStepStop::Exit(CpuStop::Halt));
        assert_eq!(cpu.reg(CpuReg::Rax), 0x42);
    }

    /// A masked BIOS ROM is fetch/read-executable but immutable to the guest: code
    /// runs straight out of a `map_rom` region, a read from it returns the ROM
    /// bytes, and a guest store aimed back into it is silently dropped rather than
    /// faulting. The program stores a word into its own ROM, reads the same
    /// location back into AX, then halts — if the store landed, AX would hold the
    /// stored value; because it is dropped, AX holds the original ROM bytes.
    #[test]
    fn rom_executes_and_reads_but_a_store_into_it_is_dropped() {
        // DS = CS = 0xF000, so DS:0x0000 resolves to phys 0xF0000 — the ROM's own
        // first bytes, which are this program's opcodes.
        //   C7 06 00 00 34 12   mov word [0x0000], 0x1234   (store into ROM)
        //   A1 00 00            mov ax, [0x0000]            (read it back)
        //   F4                  hlt
        #[rustfmt::skip]
        let code = [
            0xC7, 0x06, 0x00, 0x00, 0x34, 0x12,
            0xA1, 0x00, 0x00,
            0xF4,
        ];
        let mut cpu = X86JitCpu::new_real16_bare(
            0x10_0000,
            0xF000,
            0x0000,
            RealSegments {
                ds: 0xF000,
                es: 0xF000,
                ss: 0x0000,
                sp: 0x0F00,
            },
        )
        .expect("bare real-mode cpu");
        cpu.map_rom(0xF_0000, &code).expect("map rom");

        // Drive to the halt (a small budget per quantum; the program is a handful
        // of instructions, so this converges immediately).
        loop {
            match cpu.run(Some(64)).expect("run") {
                CpuStop::Halt => break,
                CpuStop::BudgetExhausted => continue,
                other => panic!("unexpected stop before hlt: {other:?}"),
            }
        }

        // The store was dropped: AX read back the original ROM bytes at 0xF0000,
        // which are the opcode `C7 06` → little-endian word 0x06C7, not 0x1234.
        assert_eq!(cpu.reg(CpuReg::Rax) & 0xFFFF, 0x06C7);

        // The backing is likewise untouched when read directly through the ROM.
        let mut buf = [0u8; 2];
        cpu.read_mem(0xF_0000, &mut buf).expect("read rom");
        assert_eq!(u16::from_le_bytes(buf), 0x06C7);
    }

    #[test]
    fn dma_rom_write_is_dropped_and_reads_return_rom_bytes() {
        let mut cpu = real_mode_cpu(&[0xF4]);
        cpu.map_rom(0xF_0000, &[0xA5, 0x5A]).expect("map rom");

        assert_eq!(cpu.guest_dma_write(0xF_0000, 0x11), DmaAccess::Complete(()));
        assert_eq!(cpu.guest_dma_write(0xF_0001, 0x22), DmaAccess::Complete(()));

        let mut bytes = [0; 2];
        cpu.read_mem(0xF_0000, &mut bytes).expect("read rom");
        assert_eq!(bytes, [0xA5, 0x5A]);
        assert_eq!(cpu.guest_dma_read(0xF_0000), DmaAccess::Complete(0xA5));
        assert_eq!(cpu.guest_dma_read(0xF_0001), DmaAccess::Complete(0x5A));
    }

    #[test]
    fn dma_ram_write_invalidates_cached_code() {
        // mov ax, 0x1234; out 0xE9, al; hlt
        let mut cpu = real_mode_cpu(&[0xB8, 0x34, 0x12, 0xE6, 0xE9, 0xF4]);
        assert!(matches!(
            run_to_stop(&mut cpu).expect("run"),
            CpuStop::PortOut { .. }
        ));
        assert_eq!(cpu.reg(CpuReg::Rax) & 0xFFFF, 0x1234);
        cpu.vm.watch_range(0x7C01, 1);

        assert_eq!(cpu.guest_dma_write(0x7C01, 0x78), DmaAccess::Complete(()));
        assert_eq!(cpu.guest_dma_read(0x7C01), DmaAccess::Complete(0x78));
        assert_eq!(cpu.vm.take_dirty_ranges(), vec![(0x7000, 0x1000)]);
        cpu.set_reg(CpuReg::Rip, 0x7C00);

        assert!(matches!(
            run_to_stop(&mut cpu).expect("rerun"),
            CpuStop::PortOut { .. }
        ));
        assert_eq!(cpu.reg(CpuReg::Rax) & 0xFFFF, 0x1278);
    }

    /// Base of the trapped colour text window the MMIO tests aim at.
    const VIDEO_BASE: u64 = 0xB_8000;

    /// A real-mode CPU that runs `code` from `0000:7C00` over conventional RAM,
    /// with a trapped window at [`VIDEO_BASE`] and DS/ES pointing into it, so a
    /// near `[disp16]` operand lands at `VIDEO_BASE + disp16`.
    fn real_mode_cpu(code: &[u8]) -> X86JitCpu {
        let mut cpu = X86JitCpu::new_real16_bare(
            0x10_0000,
            0x0000,
            0x7C00,
            RealSegments {
                ds: 0xB800,
                es: 0xB800,
                ss: 0x0000,
                sp: RealSegments::DEFAULT_SP,
            },
        )
        .expect("bare real-mode cpu");
        cpu.map_ram(0, 0xA_0000).expect("map ram");
        cpu.map_mmio(VIDEO_BASE, 0x8000).expect("map mmio");
        cpu.write_mem(0x7C00, code).expect("load code");
        cpu
    }

    /// Run until the first stop that is not a budget yield.
    fn run_to_stop(cpu: &mut X86JitCpu) -> Result<CpuStop, CpuError> {
        loop {
            match cpu.run(Some(64)) {
                Ok(CpuStop::BudgetExhausted) => continue,
                other => return other,
            }
        }
    }

    /// A real-mode word store into a trapped window reaches the machine as one
    /// `MmioWrite` with the guest's linear address, width and value. Once the
    /// machine acknowledges it, the retried store does not trap again and the
    /// guest runs on to its `hlt`.
    #[test]
    fn real_mode_store_to_trapped_window_surfaces_as_one_mmio_write() {
        //   B8 48 1F      mov ax, 0x1F48
        //   89 06 02 00   mov [0x0002], ax    ; DS:0x0002 = 0xB8002
        //   F4            hlt
        let mut cpu = real_mode_cpu(&[0xB8, 0x48, 0x1F, 0x89, 0x06, 0x02, 0x00, 0xF4]);

        assert_eq!(
            run_to_stop(&mut cpu).expect("run"),
            CpuStop::MmioWrite {
                addr: VIDEO_BASE + 2,
                size: 2,
                value: 0x1F48,
            }
        );
        cpu.complete_mmio_write();
        assert_eq!(run_to_stop(&mut cpu).expect("run"), CpuStop::Halt);
    }

    /// A real-mode word load from a trapped window reaches the machine as one
    /// `MmioRead`, and the value the machine supplies is what the guest receives.
    #[test]
    fn real_mode_load_from_trapped_window_surfaces_as_one_mmio_read() {
        //   A1 04 00      mov ax, [0x0004]    ; DS:0x0004 = 0xB8004
        //   F4            hlt
        let mut cpu = real_mode_cpu(&[0xA1, 0x04, 0x00, 0xF4]);

        assert_eq!(
            run_to_stop(&mut cpu).expect("run"),
            CpuStop::MmioRead {
                addr: VIDEO_BASE + 4,
                size: 2,
            }
        );
        cpu.complete_mmio_read(0xBEEF);
        assert_eq!(run_to_stop(&mut cpu).expect("run"), CpuStop::Halt);
        assert_eq!(cpu.reg(CpuReg::Rax) & 0xFFFF, 0xBEEF);
    }

    #[test]
    fn real_mode_mmio_read_and_write_resume_from_nonzero_code_segment() {
        let mut cpu = X86JitCpu::new_real16_bare(
            0x10_0000,
            0x07C0,
            0,
            RealSegments {
                ds: 0xB800,
                es: 0xB800,
                ..RealSegments::flat(0)
            },
        )
        .unwrap();
        cpu.map_ram(0, 0xA_0000).unwrap();
        cpu.map_mmio(VIDEO_BASE, 0x8000).unwrap();
        // mov ax, [4]; inc bx; mov [2], ax; inc cx; hlt
        cpu.write_mem(0x7C00, &[0xA1, 4, 0, 0x43, 0x89, 0x06, 2, 0, 0x41, 0xF4])
            .unwrap();

        assert_eq!(
            run_to_stop(&mut cpu).unwrap(),
            CpuStop::MmioRead {
                addr: VIDEO_BASE + 4,
                size: 2,
            }
        );
        assert_eq!(cpu.reg(CpuReg::Rip), 0);
        cpu.complete_mmio_read(0xBEEF);
        assert_eq!(
            cpu.run_steps(1, StepRunOptions::default()).unwrap(),
            CpuStepRun {
                steps: 1,
                stop: CpuStepStop::Exit(CpuStop::BudgetExhausted),
            }
        );
        assert_eq!(cpu.reg(CpuReg::Rip), 3);
        assert_eq!(cpu.reg(CpuReg::Rax), 0xBEEF);
        assert_eq!(cpu.reg(CpuReg::Rbx), 0);

        assert_eq!(
            run_to_stop(&mut cpu).unwrap(),
            CpuStop::MmioWrite {
                addr: VIDEO_BASE + 2,
                size: 2,
                value: 0xBEEF,
            }
        );
        assert_eq!(cpu.reg(CpuReg::Rip), 4);
        assert_eq!(cpu.reg(CpuReg::Rbx), 1);
        cpu.complete_mmio_write();
        assert_eq!(
            cpu.run_steps(1, StepRunOptions::default()).unwrap(),
            CpuStepRun {
                steps: 1,
                stop: CpuStepStop::Exit(CpuStop::BudgetExhausted),
            }
        );
        assert_eq!(cpu.reg(CpuReg::Rip), 8);
        assert_eq!(cpu.reg(CpuReg::Rcx), 0);
        assert_eq!(run_to_stop(&mut cpu).unwrap(), CpuStop::Halt);
        assert_eq!(cpu.reg(CpuReg::Rip), 10);
        assert_eq!(cpu.reg(CpuReg::Rcx), 1);
        assert_eq!(cpu.code_segment(), 0x07C0);
        assert_eq!(cpu.retired(), 5);
    }

    #[test]
    fn long64_mmio_moves_complete_each_part() {
        const MMIO: u64 = 0x4000;
        let value = 0x1122334455667788_99AABBCCDDEEFF00u128;
        for store in [false, true] {
            // Intel SDM Vol. 2B, MOVDQU, Operation (128-bit load/store).
            // Copy to RAM after the trapped move to observe the completed operand.
            let code = [
                0xF3,
                0x0F,
                if store { 0x7F } else { 0x6F },
                0x00,
                0xF3,
                0x0F,
                0x7F,
                0x03,
                0xF4,
            ];
            let vm = Vm::new(VmConfig::flat(0x10000));
            let vcpu = vm.new_vcpu();
            let mut cpu = X86JitCpu {
                vm,
                cpu: vcpu,
                dma_regions: Vec::new(),
                completed_steps: 0,
            };
            cpu.map_ram(0, MMIO).unwrap();
            cpu.map_mmio(MMIO, 0x1000).unwrap();
            cpu.write_mem(0x1000, &code).unwrap();
            cpu.set_reg(CpuReg::Rip, 0x1000);
            cpu.set_reg(CpuReg::Rax, MMIO);
            cpu.set_reg(CpuReg::Rbx, 0x2000);
            cpu.cpu.set_xmm(0, if store { value } else { u128::MAX });
            let mut written = [0u8; 16];
            for part in 0..2u8 {
                // Inspect the core metadata before the adapter discards it.
                let exit = cpu.cpu.run(&cpu.vm, None);
                let (addr, size, total_size, actual_part) = match &exit {
                    Exit::MmioRead {
                        addr,
                        size,
                        total_size,
                        part,
                        ..
                    } if !store => (*addr, *size, *total_size, *part),
                    Exit::MmioWrite {
                        addr,
                        size,
                        total_size,
                        part,
                        ..
                    } if store => (*addr, *size, *total_size, *part),
                    other => panic!("unexpected exit: {other:?}"),
                };
                assert_eq!(
                    (addr, size, total_size, actual_part),
                    (MMIO + u64::from(part) * 8, 8, 16, part)
                );
                assert_eq!(cpu.reg(CpuReg::Rip), 0x1000);
                let chunk = (value >> (u32::from(part) * 64)) as u64;
                if store {
                    let stop = cpu.map_exit(exit).unwrap();
                    let CpuStop::MmioWrite {
                        addr: write_addr,
                        size: write_size,
                        value: write_value,
                    } = stop
                    else {
                        panic!("unexpected stop: {stop:?}");
                    };
                    assert_eq!((write_addr, write_size, write_value), (addr, size, chunk));
                    let offset = usize::from(part) * 8;
                    written[offset..offset + 8].copy_from_slice(&write_value.to_le_bytes());
                    cpu.complete_mmio_write();
                } else {
                    assert_eq!(
                        cpu.map_exit(exit).unwrap(),
                        CpuStop::MmioRead { addr, size }
                    );
                    cpu.complete_mmio_read(chunk);
                }
            }
            assert_eq!(cpu.run(None).unwrap(), CpuStop::Halt);
            assert_eq!(cpu.cpu.xmm(0), value);
            let mut memory = [0; 16];
            cpu.read_mem(0x2000, &mut memory).unwrap();
            assert_eq!(memory, value.to_le_bytes());
            if store {
                assert_eq!(written, value.to_le_bytes());
            }
        }
    }

    #[test]
    fn syscall_error_reports_instruction_address_instead_of_resume_rip() {
        for opcode in [[0x0F, 0x05], [0xCD, 0x80]] {
            let mut cpu =
                X86JitCpu::new(0x2000, 0x1000, &[0x90, opcode[0], opcode[1], 0xF4]).unwrap();
            let error = cpu.run(None).unwrap_err();
            assert!(
                matches!(error, CpuError::UnsupportedInstruction { addr: 0x1001 }),
                "{opcode:02x?}: {error:?}"
            );
            assert_eq!(cpu.cpu.syscall_evidence().map(|e| e.rip()), Some(0x1001));
            assert_eq!(cpu.reg(CpuReg::Rip), 0x1003);
            assert_eq!(cpu.run(None).unwrap(), CpuStop::Halt);
        }
    }

    /// A host yield request is not something the guest did, so it must not be
    /// reported as a missing opcode. Guest state stays resumable: the next run
    /// carries on to the `hlt`.
    #[test]
    fn host_yield_request_is_an_unexpected_exit() {
        let mut cpu = real_mode_cpu(&[0xF4]);
        cpu.cpu.exit_request().request_exit();

        match cpu.run(None) {
            Err(CpuError::UnexpectedExit { what }) => {
                assert!(what.contains("interrupted"), "{what}")
            }
            other => panic!("expected an unexpected-exit error, got {other:?}"),
        }
        assert_eq!(run_to_stop(&mut cpu).expect("run"), CpuStop::Halt);
    }

    /// A backend whose JIT code storage has run out. It still hands blocks back
    /// for interpretation, which is what a real backend falls back to.
    struct ExhaustedCodeSpace;

    impl x86jit_core::Backend for ExhaustedCodeSpace {
        fn code_space_exhausted(&self) -> bool {
            true
        }

        fn materialize(
            &self,
            ir: &x86jit_core::IrBlock,
            _consistency: x86jit_core::MemConsistency,
            _mmio: Option<(u64, u64)>,
            _guest_base: u64,
        ) -> x86jit_core::CachedBlock {
            x86jit_core::CachedBlock::Interpreted(std::sync::Arc::new(ir.clone()))
        }
    }

    /// Exhausted JIT code space is reported once, as an unexpected exit that
    /// names it rather than as a missing opcode; execution then continues.
    #[test]
    fn exhausted_code_space_is_an_unexpected_exit() {
        let mut vm = Vm::with_backend(VmConfig::flat(0x2000), Box::new(ExhaustedCodeSpace));
        vm.map(0, 0x2000, Prot::RWX, RegionKind::Ram)
            .expect("map ram");
        vm.write_bytes(0x1000, &[0xF4]).expect("load hlt");
        let mut vcpu = vm.new_vcpu();
        vcpu.set_reg(Reg::Rip, 0x1000);
        let mut cpu = X86JitCpu {
            completed_steps: 0,
            vm,
            cpu: vcpu,
            dma_regions: vec![(0..0x2000, RegionKind::Ram)],
        };

        match cpu.run(None) {
            Err(CpuError::UnexpectedExit { what }) => {
                assert!(what.contains("code space exhausted at 0x1000"), "{what}")
            }
            other => panic!("expected an unexpected-exit error, got {other:?}"),
        }
        assert_eq!(cpu.run(None).expect("run"), CpuStop::Halt);
    }
}
