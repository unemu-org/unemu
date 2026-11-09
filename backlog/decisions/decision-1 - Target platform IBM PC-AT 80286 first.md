---
id: decision-1
title: Target platform — IBM PC/AT (Intel 80286) first
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-1 - Target platform: IBM PC/AT (Intel 80286) first

## Status

Accepted — 2026-07-17. Scope extended by [[decision-16]] (2026-11-09): the 286
AT is the first machine profile, not the end target; the out-of-scope items
below are now scheduled in that decision's phases.

## Context

The emulator needs a first concrete machine to drive architecture and device
selection. Options ranged from a classic real-mode AT, through a 386/486/Pentium
PCI machine, to a modern long-mode machine that maximally reuses x86jit as-is.

## Decision

Target **IBM PC/AT-class (Intel 80286)** first: real-mode boot, ISA port I/O,
PIC 8259 / PIT 8253 / 8042, legacy BIOS, VGA text later. It is the canonical
"PC", and its device set is small and well documented — good for a research
project focused on testability and swappable hardware.

## Alternatives considered

- **Modern 64-bit machine** — least CPU work (x86jit already does long mode) but
  the least "PC-like"; deferred as it doesn't exercise the legacy hardware that
  makes this project interesting.
- **386/486/Pentium PCI** — needs protected mode, paging, PCI, APIC; far more CPU
  work before anything boots.

## Consequences

- (+) Small, well-specified device set; classic PC semantics.
- (−) Requires real-mode support in x86jit, which does not exist today (see
  [[decision-3]]). This is the biggest cost and shapes the roadmap.
- Protected mode, paging, APIC, PCI are out of scope for now (`deferred.md`).

## Trigger to revisit

If real-mode work in x86jit proves disproportionately expensive, reconsider a
long-mode-first machine as the initial target.
