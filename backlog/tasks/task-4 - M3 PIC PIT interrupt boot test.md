---
id: task-4
title: M3 — 8259 PIC + 8253 PIT + interrupt boot test
status: To Do
assignee: []
created_date: 2026-07-17 00:00
labels: [milestone-3]
dependencies: [task-3]
priority: medium
---

## Description

First real-mode machine with timer interrupts. Wire the 8259 PIC and 8253 PIT
through the IRQ-injection contract and boot a real-mode program end to end.

## Acceptance Criteria

- [x] `Pic8259` device (single master: ICW1-4 init, IRR/ISR/IMR, fixed priority,
      EOI, edge-triggered) + `InterruptController` seam
      (`set_irq_line`/`poll`/`acknowledge`/`end_of_interrupt`) + coverage matrix.
      Merged @ ac28202. (Cascade/slave deferred.)
- [x] `Pit8253` device (counters 0/1/2, modes 0/2/3, divisor→period property test,
      counter-latch readback) with unit tests + coverage matrix. Merged @ b9ece59.
- [ ] Integration (M3.2): switch the machine to Real16, bump the x86jit dep to the
      `pc-emulator` rev, wire PIT OUT → PIC IRQ0 → `cpu.inject_irq`, drive the
      virtual clock from the retired-instruction counter.
- [ ] Boot golden test: a real-mode program programs the PIT, takes the timer IRQ,
      and prints to serial; deterministic output pinned to an x86jit rev.

## Notes

Depends on M2's `inject_irq` and IVT support.

Integration finding (both device agents, consistent): the PIC is **edge-triggered**
and the PIT drives IRQ0 as a **level** (counter 0 OUT). The integration layer must
edge-detect OUT (rising edge → `set_irq_line(0, true)` pulse), not pass the level
through — otherwise a held-high line latches only one interrupt.
