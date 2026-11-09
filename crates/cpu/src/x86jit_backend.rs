// SPDX-License-Identifier: AGPL-3.0-or-later
//! `x86jit`-backed CPU (§2.1, §3.2 seam 1).
//!
//! [`X86JitCpu`] wraps an `x86jit-core` [`Vm`] + [`Vcpu`] and maps the core's
//! [`Exit`] variants onto the backend-neutral [`CpuStop`]. It runs in **Long64**
//! mode (the serial skeleton, [`X86JitCpu::new`]) or in **Real16** mode (a PC/AT
//! boot that takes hardware interrupts, [`X86JitCpu::new_real16_bare`]). It uses the
//! `x86jit` integration surface: `Exit::PortIo` (`in`/`out`), `RegionKind::Trap`
//! MMIO, and — in real mode — IVT-vectored hardware IRQ injection.
//!
//! `x86jit-core` is depended on through a git dependency pinned to an exact
//! revision rather than a plain path, so a clean checkout or CI resolves the same
//! CPU without any local x86jit checkout present.

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
    /// The segment registers are seeded here because the `x86jit` core models
    /// segment *bases* for addressing (it does not lift `mov Sreg, r16`); a
    /// real-mode guest starts from these rather than setting them up from nothing.
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
        Ok(Self { vm, cpu })
    }

    /// Map a conventional RAM window `[base, base + size)` as `RWX` `Ram`.
    ///
    /// For an embedder building a mosaic address space with
    /// [`X86JitCpu::new_real16_bare`]: the low 640 KiB, the upper-memory probe
    /// area, and the zeroed option-ROM scan region are all plain RAM.
    pub fn map_ram(&mut self, base: u64, size: u64) -> Result<(), CpuError> {
        self.vm
            .map(base, size as usize, Prot::RWX, RegionKind::Ram)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
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
        self.vm
            .map(base, blob.len(), Prot::RX, RegionKind::Rom)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))?;
        self.vm
            .write_bytes(base, blob)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
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
        self.vm
            .map(base, size as usize, Prot::RW, RegionKind::Trap)
            .map_err(|e| CpuError::Memory(format!("{e:?}")))
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

impl Cpu for X86JitCpu {
    fn run(&mut self, budget: Option<u64>) -> Result<CpuStop, CpuError> {
        match self.cpu.run(&self.vm, budget) {
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
            Exit::Exception { vector, .. } => Ok(CpuStop::Exception { vector }),
            Exit::BudgetExhausted => Ok(CpuStop::BudgetExhausted),
            Exit::UnmappedMemory { addr, .. } => Err(CpuError::Unmapped { addr }),
            Exit::UnknownInstruction { addr, .. } => Err(CpuError::UnsupportedInstruction { addr }),
            // Only `syscall`/`sysenter`, or `int 0x80` outside real mode, end up
            // here: in real mode every `int n` is delivered inside the guest
            // through the IVT, and the long-mode serial programs make no system
            // calls. RIP has already moved past the instruction.
            Exit::Syscall => Err(CpuError::UnsupportedInstruction {
                addr: self.cpu.reg(Reg::Rip),
            }),
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
        // Real mode only: the core queues the vector and delivers it in-guest
        // through the IVT at the next `run()` boundary once IF is set, no
        // completion is outstanding, and the STI shadow has elapsed. In Long64
        // this is inert (the core never sets `pending_irq` outside real mode).
        self.cpu.inject_irq(vector);
    }

    fn has_pending_irq(&self) -> bool {
        self.cpu.has_pending_irq()
    }

    fn guest_dma_read(&self, addr: u32) -> u8 {
        let mut buf = [0u8; 1];
        // A DMA target that falls outside mapped RAM reads as open bus, matching
        // an ISA master that no memory responds to.
        match self.vm.read_bytes(u64::from(addr), &mut buf) {
            Ok(()) => buf[0],
            Err(_) => 0xFF,
        }
    }

    fn guest_dma_write(&mut self, addr: u32, value: u8) {
        // Goes through the `Vm` write path so the core's self-modifying-code
        // tracking invalidates any translated block the transfer overwrites — the
        // reason a DMA controller never touches guest RAM directly.
        let _ = self.vm.write_bytes(u64::from(addr), &[value]);
    }

    fn retired(&self) -> u64 {
        self.cpu.retired_instructions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut cpu = X86JitCpu { vm, cpu: vcpu };

        match cpu.run(None) {
            Err(CpuError::UnexpectedExit { what }) => {
                assert!(what.contains("code space exhausted at 0x1000"), "{what}")
            }
            other => panic!("expected an unexpected-exit error, got {other:?}"),
        }
        assert_eq!(cpu.run(None).expect("run"), CpuStop::Halt);
    }
}
