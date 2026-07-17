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

- [ ] `Pic8259` device (IRQ aggregation, `irq_ack`/INTA, EOI, priority) with
      isolation unit tests + coverage-matrix row.
- [ ] `Pit8253` device (counter modes, divisor→period property test) with unit tests.
- [ ] Boot golden test: a real-mode program programs the PIT, takes the timer IRQ,
      and prints to serial; deterministic output pinned to an x86jit rev.

## Notes

Depends on M2's `inject_irq`/`irq_ack` and IVT support.
