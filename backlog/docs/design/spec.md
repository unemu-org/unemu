# unemupc98 specification

A growing, numbered specification for the emulator. Doc-comments in the source
cross-reference these sections by number (`§3.2`), the way x86jit's own
`backlog/docs/design/spec.md` does. This is a **living document** — sections are
added as subsystems are designed and built. As of M0 it is an outline; the
detailed prose is filled per milestone.

Related: [`../architecture.md`](../architecture.md) (current shape),
[`../testing.md`](../testing.md) (test harness), `../../decisions/` (the why).

## §1 Scope

§1.1 Target machine: IBM PC/AT-class (Intel 80286), real-mode boot.
§1.2 Priorities: testability, swappability, documentation (research project).
§1.3 Out of scope for now: protected mode, paging (386+), APIC, PCI, real BIOS,
graphics beyond serial. See [`../deferred.md`](../deferred.md).

## §2 CPU core (x86jit)

§2.1 Integration surface that works today (`RegionKind::Trap`, `Exit::PortIo`,
`Backend`, exact block counter).
§2.2 Real-mode gap: segment registers, `IF`, `segment:offset` addressing,
IVT-vectored `INT`/exceptions — added under `CpuMode::Real16`.
§2.3 ABI stability: `CpuState` is `#[repr(C)]`; append-only; offset-assertion test.
§2.4 Worktree + merge discipline (mergeability criteria).

## §3 Machine assembly

§3.1 `Config` → `Machine` composition root.
§3.2 Swap seams: `Cpu`, `Chipset`, `Bus`, `Device`, `FrameSink`, tracing.
§3.3 Chipset declares the decode map; Bus executes it.

## §4 Memory & bus

§4.1 Address-space and port-space decode.
§4.2 RAM / MMIO (`Trap`) / ROM regions.
§4.3 Writes via `note_write` (SMC safety) — DMA, BIOS loader, shadow RAM.
§4.4 A20 gate (always-on for now).

## §5 Interrupt model

§5.1 IRQ lines → 8259 PIC → INTR.
§5.2 `inject_irq` contract: between-`run()` only; deferred while pending completion.
§5.3 STI shadow, HLT wakeup.
§5.4 INTA / `irq_ack() -> vector`; IVT dispatch.

## §6 Virtual clock

§6.1 Completed-step time base, including REP iterations (not blocks).
§6.2 Step bounds, instruction-start accesses and IRQ-readiness ordering; see
[`scheduler.md`](scheduler.md#adoption) for the budget-independence contract.
§6.3 Golden pinning + re-baseline procedure.

## §7 Devices

§7.1 `Device` contract (mmio/port, `tick`, IRQ, `reset`, `save`/`restore`, trace).
§7.2 16550 UART (serial) — first device.
§7.3 8259 PIC. §7.4 8253 PIT. §7.5 8042 keyboard controller.
§7.6 CMOS/RTC. §7.7 8237 DMA. §7.8 µPD765 floppy controller (reads via §7.7 DMA).

## §8 Firmware

§8.1 Minimal BIOS stub (IVT setup + serial print).
§8.2 BIOS blob loader (later); 286 fidelity limits for a real BIOS.

## §9 Video

§9.1 `FrameSink` seam; cards are Devices.
§9.2 VGA text 80×25 (first graphics mode, later).

## §10 Testing

Mirrors [`../testing.md`](../testing.md): unit, fuzz/property, coverage matrix,
boot/integration, golden/trace, differential (SingleStepTests), snapshot/replay.
