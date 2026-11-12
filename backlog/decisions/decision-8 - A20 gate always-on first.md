---
id: decision-8
title: A20 gate — always-on for M1–M3, full gate later
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-8 - A20 gate: always-on for M1–M3, full gate later

## Status

Accepted — 2026-07-17.

## Context

The A20 gate masks address line 20, wrapping addresses past `FFFF:FFFF` (the HMA
region). Real BIOSes and programs rely on and probe this behaviour. With
`guest_base` baked into the JIT, implementing A20 masking is an architectural
choice (mask in the lifter vs an aliased region map), not a detail — so it must be
a conscious decision, not an oversight.

## Decision

For milestones M1–M3, model **A20 as always-on** (no wrap masking). Implement the
full gate (controlled via the 8042 keyboard controller) later, once boot flows
need it. Documented so it is a known limitation, not an accidental gap.

## Consequences

- (+) Unblocks early boot/serial work without lifter-level address masking.
- (−) Programs that depend on A20-disabled wrap (or that probe the gate) behave
  incorrectly until the full gate lands.

## Trigger to revisit

When a target boot flow (a real BIOS, or HIMEM-style code) requires A20 toggling,
implement the full gate via 8042 and update this decision.

## Amendment — 2026-11-12: full gate implemented

The always-on limitation is superseded on machines that have an A20 source.
The board A20 line (8042 output-port bit 1 OR port 92h bit 1; OR is an
assumed board policy until the generic ISA board is qualified) drives CPU
physical address bit 20 through the CPU core's gate (x86jit decision-22
(x86jit)), so masking happens in the core, not in an aliased memory map.
Assembly applies the initial line; the machine pushes every change to the
CPU after the port write that caused it, before the next instruction, and a
change counter suppresses redundant calls. CPU-only resets (8042, port 92h,
shutdown) keep the line, as on the board. DMA and host physical-memory
access bypass the gate.

Because the core honours the gate only in its machine models, ibm-5170 and
bare rigs with an 8042 or port 92h run the 80286 machine model with
in-guest exception delivery; bare rigs without an A20 source stay flat.
The open BIOS's single-step CPU probe on the bare POST rig changed its CPU
identification line (re-baselined; undoing only that line reproduces the
old hashes), and the user-supplied IBM 5170 BIOS, which toggles A20 through
the 8042, boots unchanged.
