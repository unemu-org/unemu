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

## Annex (2026-07-18): seam 2 becomes data, not a trait

The `Chipset` **trait** (seam 2 above) is replaced by **data plus a single
data-driven assembler**. This does not weaken the decode-ownership invariant — it
strengthens it. Design: [`../docs/design/machine-config.md`](../docs/design/machine-config.md).

### What changed and why

`Chipset` was defined as the object that *declares* the port/MMIO decode map, the
device set, and the interrupt controller, with `Bus` executing what it declares.
In practice this produced drift, not clean declaration: `AtChipset::build`,
`SerialChipset::build`, and a third hand-written `assemble_bios_machine` in the
CLI diverged, and `AtChipset` could not even assemble the FreeDOS machine. The
IRQ/DMA wiring the frontend was supposed to receive from the chipset was in fact
declared ad-hoc in the CLI `run_*` bodies — a partial violation of the
"chipset declares, bus executes" invariant. A trait per machine variant is the
wrong tool: what varies between machines is *data*, not *behaviour*.

### The invariant survives, stronger

"Chipset declares, Bus executes" (§3.3) becomes:

> **preset declares (data) → the one assembler translates → Bus executes.**

A machine is a plain-data `MachineConfig` (a flat `DeviceConfig` list, each entry
owning its ports, MMIO, IRQ line, and DMA wiring). A **preset** function fills in
that data. A **single assembler**, living in a new crate *above* the `machine`
crate, is the only code that translates the data into concrete devices and a
decode map — so the machine core still never names a concrete device or CPU type
(the load-bearing rule from the Decision above is preserved). The Bus still
executes the resulting decode map and remains the single decode owner.

The IRQ/DMA wiring that used to leak into the CLI frontend now lives in the
preset's device data, closing the partial violation. Decode conflicts (e.g. the
`0x80` POST latch vs. the `0x81` DMA page register) are handled by a deterministic
sort plus a hard error on overlap, so decode is a pure function of the config.

The other five seams (`Cpu`, `Bus`, `Device`, `FrameSink`, tracing) are
unchanged. Only seam 2's *form* changes: a trait becomes data + one assembler.

### Handling

task-28 (this annex) records the decision. task-33 removes the `Chipset` trait
and the ad-hoc assembly and *verifies* this annex against the final code; it does
not re-decide anything here. A20-always-on (decision-8) and the
retired-instruction clock (decision-5) remain model constants, expressed by the
config rather than configurable knobs.
