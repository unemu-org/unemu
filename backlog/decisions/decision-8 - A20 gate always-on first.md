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
