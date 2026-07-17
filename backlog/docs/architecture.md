# Architecture

Repo shape, layers, and the models that make this emulator testable and
swappable. Keep this **descriptive of the current state**, not aspirational — as
of M0 there is no emulator code yet, so sections below marked *(planned)* are the
agreed design, not the implemented reality. For *why*, see `ls backlog/decisions/`.
The numbered specification grows in [`design/spec.md`](design/spec.md).

## Design goals

Research project. In priority order: **testability**, **swappability** (CPU /
chipset / device / graphics), **documentation**. First end-to-end target:
headless serial (16550 UART), deterministic golden tests.

Target machine #1: **IBM PC/AT-class (Intel 80286)** — real-mode boot, ISA I/O
ports, PIC 8259 / PIT 8253 / 8042, legacy BIOS. Protected mode, paging (386+),
APIC, and PCI are out of scope for now (see [`deferred.md`](deferred.md)).

## Layout *(planned — Cargo workspace)*

```
crates/
  cli/          # frontend binary: parse config -> build Machine -> run; serial -> stdout
  machine/      # composition root: assembles a Machine from a Config (which cpu/chipset/devices)
  cpu/          # trait Cpu + the x86jit-backed implementation (adapts Vm/Vcpu to the Cpu trait)
  bus/          # executes the address/port decode map; wraps x86jit Vm memory + PortIo
  chipset/      # trait Chipset: declares the decode map + device set + interrupt controller
  device/       # trait Device + shared device plumbing (irq lines, trace, save/restore)
  devices/      # concrete devices: uart16550, pic8259, pit8253, i8042, cmos_rtc, dma8237, …
  video/        # trait FrameSink (presentation sink); video cards are ordinary Devices
  firmware/     # minimal BIOS stub (IVT + serial) / loader for a BIOS blob
  testkit/      # mock bus, golden runner, trace/replay, virtual-clock harness
backlog/        # tasks, docs, decisions (this tree)
```

The CPU core (`x86jit`) is an external pinned git dependency, developed in a
worktree — see `AGENTS.md` and the worktree decision.

## Swap seams (the swappability contract)

Six seams. Each is a trait boundary; nothing downstream names a concrete type.

1. **`Cpu`** — swap the CPU core (today: x86jit). Contract: `run(budget)`,
   register + **segment** access, bind the bus, `inject_irq(...)`,
   `irq_ack() -> u8` (INTA), and mapping the core's `Exit` variants to bus events.
2. **`Chipset`** — **declares** the address/port decode map, the device set, and
   the interrupt controller. One place owns the map.
3. **`Bus`** — **executes** the decode the chipset declared (Chipset declares,
   Bus executes — no duplicated decode logic).
4. **`Device`** — add/replace one piece of hardware. Contract includes MMIO
   read/write, port in/out, `tick(vclock)`, an IRQ line, `reset`,
   `save`/`restore`, and trace emission — **from the first device**, not retrofitted.
5. **`FrameSink`** — swap the presentation sink (framebuffer / text → consumer).
   A video card (MDA/CGA/VGA) is an ordinary `Device`; only the sink is the seam.
6. **Tracing** — a first-class seam: golden trace tests read traces from the
   `Device` contract, not from ad-hoc printlns.

## Integration surface with x86jit

Works **today**, no CPU changes needed (foundation of the M1 walking skeleton):

- `RegionKind::Trap` → `Exit::MmioRead` / `Exit::MmioWrite` — MMIO device hook.
- `Exit::PortIo { port, dir, size, value }` + `complete_port_in` — ISA port I/O
  (`IN`/`OUT`). Already works in `Long64`.
- Injected `Backend` trait — the core never names a concrete JIT.
- Dispatcher keeps `blocks_run` as an exact block counter (fuel/quantum).

Needs new work in x86jit (behind `CpuMode::Real16`, see decisions):

- **Segment registers** (CS/DS/ES/SS) in `CpuState` — absent today; needed for
  every real-mode memory access, far jmp/call, push/pop, string ops.
- **`IF`** (and `TF`/`IOPL`) in `Flags` — absent today; needed for `sti`/`cli`,
  `pushf`/`popf`, `iret`, and interrupt gating.
- Real-mode `segment:offset` addressing with 64K/`SP` wrap in the lifter.
- `INT n` / exceptions **vectored through the IVT inside the guest** — today
  `Exit::Exception` / `Exit::Syscall` exit to the embedder; this is mode-dependent.
- Hardware IRQ injection (see interrupt model).

⚠️ `CpuState` is `#[repr(C)]` with offsets baked into `jit_abi.rs` and generated
code: **layout changes are an ABI hazard for the PS4 path** — append-only, with
an offset-assertion test.

## Interrupt model *(planned)*

- Devices raise IRQ lines; the `InterruptController` (8259 PIC) aggregates them
  into a single INTR to the CPU.
- **Injection happens only between `run()` calls.** Chained blocks don't return to
  the dispatcher mid-run; in the deterministic model devices tick on `run()`
  boundaries, so interrupt latency = the budget quantum. The quantum value is
  recorded because goldens depend on it.
- **Forbidden while `pending_mmio` / `pending_port_in` is set** (instruction
  mid-retry at the faulting RIP); `inject_irq` returns "deferred" then.
- **STI shadow** (an interrupt must not enter immediately after `sti`) and
  **HLT wakeup** (`Exit::Hlt` then a later IRQ resumes past the `hlt`) are modelled.
- **INTA**: on INTR the CPU asks the PIC `irq_ack() -> vector`; the PIC updates
  its ISR/IRR. The vector is dispatched through the real-mode IVT.

## Virtual clock model *(planned)*

- Virtual time is measured in **retired instructions** (a cheap instruction
  counter added in the lifter as part of the real-mode seam), **not in blocks**.
  Block boundaries depend on the lifter version, so block-based goldens would
  break on every x86jit merge.
- PIT/RTC tick off the virtual clock → reproducible goldens. Each golden pins the
  x86jit revision; the re-baseline procedure is documented in the clock decision.

## Memory / ownership model *(planned)*

- The `Machine` owns the `Vm`; the flow is `Vcpu::run(&Vm)` → `Exit` → dispatch to
  a `&mut Device` → optional completion (`complete_mmio_read`, …) → re-enter.
- **DMA (8237), the BIOS loader, and shadow RAM write guest RAM ONLY through the
  `Vm` API that runs `note_write`** (SMC tracking in x86jit `memory.rs`). A raw
  write into the RAM buffer silently breaks translated-block invalidation.
- Devices do **not** hold references into guest memory; they receive a handle in
  `tick` / `dma_transfer`.

## Open hardware decisions (tracked as ADRs)

- **A20 gate**: always-on for M1–M3; full gate via 8042 later.
- **ROM regions**: BIOS at `F0000h` needs a read-only, executable region.
  `RegionKind` today is only `Ram`/`Trap` (`Ram` = writable ROM, unfaithful;
  `Trap` = code fetch likely broken). May require `RegionKind::Rom` — a **second
  x86jit seam**.
- **286 fidelity**: `pushf` bits 12–15, absence of `cpuid`, `popf` behaviour —
  period BIOSes probe the CPU this way; x86jit models a modern CPU.

## Layering rules

Downstream never names concrete hardware: `machine` depends on the `cpu`/`bus`/
`chipset`/`device`/`video` traits, not on `devices/*` or the x86jit types. Concrete
devices depend only on `device` + `bus`. Enforcement (e.g. `cargo deny` /
workspace lints) to be added with the first crates; link the decision here when it lands.
