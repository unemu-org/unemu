---
id: decision-15
title: Deterministic device scheduler with instruction-exact interrupt boundaries
date: 2026-11-09
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-15 - Deterministic device scheduler with instruction-exact interrupt boundaries

## Status

Accepted — 2026-11-09. Extends [[decision-5]] and [[decision-14]]; replaces the
block-quantized interrupt latency and eager acknowledgement of [[decision-4]].
Design: `backlog/docs/design/scheduler.md` (the proposal) as amended by the
adversarial review recorded below; where they differ, this record wins.

## Context

The machine ticks every device once per PIT clock and maps one retired
instruction to one PIT clock, a hidden fixed CPU speed (the configured ratio is
never read). Interrupts enter only at run-quantum boundaries, and the CPU core's
budget counts blocks, so interrupt latency depends on the quantum and on how the
core forms blocks. The PIC is acknowledged eagerly, regardless of the guest's IF
and before the stopping instruction's port write reaches the bus.

A release-build profile (`backlog/docs/design/machine-time-profile.md`) shows real
DOS code is CPU-bound (86–89 % of wall time in the CPU core): this is a
correctness change first, not a speed-up.

An adversarial review of the proposal found one blocker: the CPU core does not
model the one-instruction interrupt inhibit after a load of SS (`mov ss` /
`pop ss`). Today interrupts land only at block boundaries, which accidentally
shields the ubiquitous `mov ss, ax / mov sp, bx` pair; instruction-exact delivery
would open that window and corrupt the guest stack.

## Decision

1. **Time stays in whole PIT input clocks.** `VClock(u64)` keeps its unit (no
   wider or finer instant). The CPU speed is a configurable rational rate
   (retired steps per PIT clock, A/B, default 1/1 = today) with a carried
   remainder. Every other clock domain (RTC 32 768 Hz, future audio) keeps an
   exact rational phase and rounds each event **up** to the next whole PIT clock;
   the added latency is at most one PIT clock, far below the model's other
   limits. Virtual time remains retired steps plus halted PIT clocks
   ([[decision-14]]).
2. **Devices declare their next event.** A device answers `NextEvent::
   {EveryPitClock, At(clock), Dormant}`; the default is `EveryPitClock`, so an
   unmigrated device behaves exactly as today. Scheduled devices synchronise
   lazily on access. Access-time information reaches devices through new
   default-delegating methods, not by changing the existing `Device` signatures.
3. **Only wired outputs are events.** A PIT channel whose OUT drives nothing
   (channel 1 refresh, channel 2 speaker until a speaker exists) is never a
   scheduled event; its counter is synchronised on read or latch. Channel 0
   (IRQ0) edges are scheduled.
4. **Interrupts are taken at completed instruction boundaries, only when the CPU
   can accept them.** The run loop completes the stopping instruction's bus
   effects (port write, MMIO, DMA) first, then acknowledges the PIC only if the
   core reports it is ready (IF set, no STI or SS-load shadow, no pending access)
   and delivers the vector at once. HLT jumps to the next event or bound.
5. **CPU work is bounded in steps, not blocks.** A step is one completed
   instruction, and each completed iteration of a REP string instruction is one
   step, so device time advances during long string operations and an interrupt
   can be taken between iterations (the core already resumes REP at the
   instruction).
6. **The CPU core provides, in its Real16 interpreter (stage 1):** an
   instruction-exact run budget in steps; counting of a self-referential
   `jmp $`; the one-instruction interrupt inhibit after an SS load; an
   interrupt-readiness query; and an optional stop at the first boundary where
   an interrupt becomes acceptable. Completion-only resumption, an acceptance
   callback and a zero-progress yield stay on this side. Corrected interrupt
   timing is **not enabled** before the SS-load inhibit exists.
7. **Migration keeps the legacy loop untouched** until the end. The per-clock
   engine and the new engine run under the existing outer loop; the
   single-clock reference with corrected semantics is a test-only oracle, never
   a production mode. A golden comparator normalises the documented systematic
   shifts (access timestamps taken at instruction start, exact cap endpoints),
   so reviewers see only real ordering or acknowledgement differences. Near-total
   movement of trace-bearing goldens under corrected timing is expected; each
   remaining difference is explained and accepted individually.

## Alternatives considered

- **The proposal's 128-bit instants with 2^32 units per PIT period** — buys no
  observable precision (the CPU only observes events at instruction boundaries)
  and forces trace projections and fractional halted credit. Rejected.
- **Event skipping on block budgets** — cannot enforce instruction deadlines.
  Rejected.
- **REP as a single step** — freezes device time across a 64 KiB copy and blocks
  interrupts inside it, while the core is already resumable between
  iterations. Rejected.
- **Cycle- or bus-approximate timing** — no calibrated oracle; deferred to its
  own decision.

## Consequences

- (+) Device periods and interrupt acceptance stop depending on the run quantum
  and on the core's block formation; clock domains can host the RTC's real rate
  and future audio without wall-clock coupling.
- (+) The INTA-ordering defect is fixed by construction.
- (−) Needs a scoped stage-1 change in the CPU core and a re-pin.
- (−) Most trace-bearing goldens move once, under review, when corrected timing
  is adopted.
- Known gaps carried over: instruction cycle costs, ISA bus timing and DMA
  contention are not modelled; level-triggered device lines are still pulsed
  (the "line must stay high until INTA" rule is not modelled); `INS`/`OUTS` are
  not lifted by the core yet (the IDE PIO path will need them).

## Trigger to revisit

A workload that needs instruction-specific cycle costs, ISA bus timing, DMA
arbitration, NMI or reset wake-up, or sub-PIT-clock precision; or measurements
showing the next-event scan or the extra CPU returns to be material.
