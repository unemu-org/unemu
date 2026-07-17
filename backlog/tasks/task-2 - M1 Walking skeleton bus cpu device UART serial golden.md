---
id: task-2
title: M1 — Walking skeleton (bus, cpu trait, device trait, UART, serial golden)
status: Done
assignee: []
created_date: 2026-07-17 00:00
labels: [milestone-1]
dependencies: [task-1]
priority: high
---

## Description

Full vertical slice **without changing x86jit**. `Exit::PortIo` already works in
`Long64`, so a guest can drive a 16550 UART today. Proves CLI → Machine → Bus →
Device → golden end to end before touching the CPU.

## Acceptance Criteria

- [x] Cargo workspace with `cli`, `machine`, `cpu`, `bus`, `chipset`, `device`,
      `devices`, `testkit` crates (traits + minimal impls).
- [x] `cpu` trait with an x86jit `Long64` backend; `bus` executes a chipset-declared
      decode; `device` trait includes `tick`, IRQ line, `reset`, `save`/`restore`,
      trace — from day one.
- [x] `Uart16550` device with isolation unit tests on a mock bus.
- [x] Golden test: a `Long64` guest writes "hello" to the UART via `out dx, al`;
      captured serial output matches a committed golden.
- [x] Every source file has the AGPL SPDX header and module/trait doc-comments.

## Notes

No real mode here. This is the walking skeleton that de-risks the whole project.
