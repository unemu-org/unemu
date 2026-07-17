---
id: decision-4
title: Hardware IRQ injection contract
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-4 - Hardware IRQ injection contract

## Status

Accepted — 2026-07-17.

## Context

x86jit exposes exits (MMIO, PortIo, Exception, Syscall, Hlt) but **no way to
inject an asynchronous hardware interrupt**. A PC needs the 8259 PIC to deliver
IRQs to the CPU. Naïvely injecting mid-execution collides with JIT realities:
chained blocks don't return to the dispatcher; instructions may be mid-retry
(`pending_mmio` / `pending_port_in`); `sti` has a one-instruction shadow; `hlt`
must wake on a later IRQ.

## Decision

Define an injection contract on the `Cpu` seam:

- **Injection only between `run()` calls.** In the deterministic model devices
  tick on `run()` boundaries; interrupt latency = the budget quantum. The quantum
  value is recorded (goldens depend on it).
- **Forbidden while `pending_mmio` / `pending_port_in` is set** — `inject_irq`
  returns "deferred"; retried after completion.
- **STI shadow**: no interrupt immediately after `sti`.
- **HLT wakeup**: `Exit::Hlt` then a later IRQ resumes execution past the `hlt`.
- **INTA**: on INTR the CPU calls `irq_ack() -> vector` on the interrupt
  controller (PIC updates ISR/IRR); the vector dispatches through the real-mode IVT.

Requires the [[decision-3]] (b) work: `IF` in `Flags`, `sti`/`cli`/`iret`, IVT entry.

## Consequences

- (+) Deterministic, testable interrupt delivery; no mid-block injection hazards.
- (−) Interrupt latency is quantised to the budget; sub-quantum timing is not
  modelled. Acceptable for AT-class targets.

## Trigger to revisit

If a workload needs finer interrupt latency than one budget quantum, revisit the
between-`run()` restriction (e.g. a safe dispatcher-level poll point).
