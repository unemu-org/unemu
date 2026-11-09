---
id: decision-14
title: Virtual time advances while the CPU is halted
date: 2026-11-09
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-14 - Virtual time advances while the CPU is halted

## Status

Accepted — 2026-11-09. Extends [[decision-5]]; does not replace it.

## Context

[[decision-5]] measures virtual time in retired instructions. That only works
while the CPU is running. The CPU core's `hlt` exits without keeping a halted
state, and the machine used to resume the guest right after it — so a halted
guest executed the code after `hlt` without an interrupt, `cli; hlt` never
stopped, and time passed only because the guest kept retiring its post-`hlt`
code. On real hardware the CPU stops fetching during HLT while the timer and
every other device keep running; an interrupt (IF set) or NMI/reset ends it.

Once the machine really stops the CPU, no instructions retire, so a clock made
only of retired instructions would freeze and nothing could ever wake the guest.

## Decision

- While the CPU is halted the machine advances virtual time **one PIT input
  clock at a time**, ticking devices and edge-detecting interrupt lines exactly
  as it does for running time. These clocks are not backed by retired
  instructions.
- **Virtual time = retired instructions + halted PIT clocks ×
  `RETIRED_PER_PIT_CLOCK`** (the instruction-equivalent of one halted clock).
  Run caps and run-policy schedule targets are measured in virtual time, so a
  guest that halts in a `sti; hlt` loop stops at the same virtual time as one
  that busy-polls.
- Halted time is still a pure function of guest state and the run policy — no
  wall clock enters the execution path.
- With IF clear the halted CPU cannot be woken (NMI is not modelled): the run
  ends at once.

## Alternatives considered

- **Keep re-running the CPU past `hlt`** — wrong x86 semantics (code after `hlt`
  runs before the interrupt). Rejected.
- **Jump straight to the next device event** — faster, but every device would
  have to report its next event time; ticking one clock at a time reuses the
  existing edge-detection path. Kept as a later optimisation if halted runs
  become a hotspot.
- **Count only retired instructions toward caps** — a halt-heavy guest could run
  unbounded host time under a fixed cap. Rejected.

## Consequences

- (+) Real HLT semantics; idle guests no longer burn host time on post-`hlt` code.
- (+) Caps bound host work for halting and busy-polling guests alike.
- (−) Virtual time is no longer one counter in the CPU core; the machine owns
  the halted part. Goldens that halt with a live timer may shift.

## Trigger to revisit

A guest workload where halted ticking dominates host time (then: next-event
skipping), or a need to model NMI.
