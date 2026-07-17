---
id: decision-7
title: Swappability via Cpu / Chipset / Device / FrameSink traits
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-7 - Swappability via Cpu / Chipset / Device / FrameSink traits

## Status

Accepted — 2026-07-17.

## Context

A core project goal is easy replacement of CPUs, chipsets, devices, and graphics
modes. This needs deliberate trait boundaries so nothing downstream names a
concrete type. Early design must avoid two traps: duplicating the address-decode
logic between "chipset" and "bus", and modelling graphics as a second dispatch
channel parallel to devices.

## Decision

Six swap seams:

1. **`Cpu`** — swap the CPU core. Contract: `run(budget)`, register + segment
   access, bind bus, `inject_irq`, `irq_ack() -> u8`, map `Exit` to bus events.
2. **`Chipset`** — **declares** the address/port decode map + device set +
   interrupt controller.
3. **`Bus`** — **executes** the decode the chipset declared (single decode owner).
4. **`Device`** — one hardware unit. Contract includes MMIO/port, `tick(vclock)`,
   IRQ line, `reset`, **`save`/`restore`**, and trace emission — **from the first
   device**, not retrofitted.
5. **`FrameSink`** — the presentation sink (framebuffer/text → consumer). A video
   card (MDA/CGA/VGA) is an ordinary `Device`; only the sink is swappable here.
6. **Tracing** — first-class seam; golden trace tests read from it.

The machine core depends on these traits, never on `devices/*` or x86jit types.

## Alternatives considered

- **`GraphicsAdapter` as a peer of `Device`** — creates a parallel dispatch
  channel; rejected in favour of card-as-Device + `FrameSink`.
- **Chipset and Bus both decoding** — ambiguous ownership; rejected. Chipset
  declares, Bus executes.
- **Adding `save`/`restore` later** — retrofitting serialization across N devices
  is painful; rejected. It's in the contract from day one.

## Consequences

- (+) Clean replacement of any layer; snapshot/replay works from the first device.
- (−) More upfront trait surface (reset/save/restore/trace) before the first
  device does anything visible.

## Trigger to revisit

If a device genuinely cannot fit the `Device` contract (e.g. a bus-master with
unusual timing), revisit the trait rather than bypassing it.
