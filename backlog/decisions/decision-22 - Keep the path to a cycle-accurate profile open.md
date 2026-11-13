---
id: decision-22
title: Keep the path to a cycle-accurate profile open
date: 2026-11-13
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-22 - Keep the path to a cycle-accurate profile open

## Status

Accepted — 2026-11-13. Builds on [[decision-14]] (virtual time) and
[[decision-15]] (device scheduler). Records the outcome of the
cycle-accurate readiness spike.

## Context

A cycle-exact CPU core is a separate future project behind the `Cpu` trait
([[decision-16]]). Cycle-exact effects also need the rest of the system to
be cycle-exact: bus arbitration (DMA, DRAM refresh), wait states, the PIT's
phase against the CPU clock, interrupt acknowledge timing, and video timed
by its dot or character clock. The maintainer wants unemupc98 never to need
a rewrite to host such a profile.

The spike audited unemupc98 against that profile, using MartyPC (MIT
licence, Daniel Balsom) as a checklist of timing details, not as a design
to copy. Findings:

- Device logic survives unchanged. Devices already report deadlines and
  catch up to any time through `sync(at)`; audio integrates exact areas;
  the machine-file schema already reserves wait-state and cycle-exact
  fields.
- `VClock` counts PIT input clocks, which is coarser than one CPU cycle.
  About 25 production sites in 17 files depend on one tick being one PIT
  clock; the other uses only carry the value.
- The board crystal is written four ways (105/88 MHz, 3,579,545/3 Hz,
  14,318,180 Hz, 1,193,182 Hz), differing by about 0.14 ppm.
- The VGA beam is derived from absolute time and the current registers,
  so reprogramming re-phases it as if the new timing had applied since
  time zero.
- Missing for later: a wait-state and bus-hold path, RAM and ROM accesses
  on the bus, partial DMA arbitration, interrupt acknowledge phases, a
  `Cpu` that reports bus cycles, and video captured at the guest's
  vertical sync instead of at host paint time.

## Decision

1. **Seams now** (cheap while the device count is small; no change in
   behaviour beyond item b):
   a. A clock-domain module holding each crystal as one exact rational
      constant, with exact edge arithmetic (`edges_between`, `edge_at`,
      `next_edge_after`, phase carried, edges rounded up) and named domains
      (PIT, video dot clocks, UART, RTC, OPL, audio output, the AT CPU
      crystal). One `VClock` tick stays one PIT clock for now; every
      duration and every "next clock" goes through a domain.
   b. The crystal spellings are unified on one constant now. Goldens that
      move only because of that are re-baselined with the reason stated.
   c. The `VClock` field becomes private (raw access only for snapshots),
      so new code cannot do raw tick arithmetic.
   d. A test-only build runs the device tests and selected goldens with
      several ticks per PIT clock, proving no hidden one-tick assumption.
   e. The VGA beam becomes stateful, anchored at the last timing or clock
      select write.
   f. Bus accesses carry their access time explicitly (behaviour unchanged).
2. **Target unit.** When a cycle-accurate profile arrives, `VClock` moves
   to a fine common unit chosen so that every board crystal (14.31818 MHz
   and the CPU crystals of the supported boards, such as 12, 16, 20, 25 and
   33 MHz) is an exact whole number of ticks. A plain master-oscillator tick
   (69.8 ns) is too coarse for a 16 MHz 286 cycle (62.5 ns). The exact unit
   is fixed at that flip.
3. **First cycle-exact machine:** an XT with CGA, the best documented case
   (MartyPC, the 8088 SingleStepTests, the 8088 demoscene).
4. **Later, additive, with the cycle-exact profile:** a wait-state query
   with a zero default, timing attributes on the decode map, refresh as a
   hold schedule, DMA arbitration, interrupt acknowledge phases, a `Cpu`
   cycle capability with its own machine loop, catch-up video rendering.
5. **Rules for new code, now:**
   - No device assumes one `VClock` tick is one PIT clock; durations come
     from a clock domain, never bare literals.
   - "Next clock" is `domain.next_edge_after(now)`, never `now + 1`.
   - Each crystal is an exact rational constant; round edges up and carry
     phase; never accumulate rounded periods or use floating point.
   - No new device that must be ticked every PIT clock; devices report
     deadlines and catch up idempotently in `sync(at)`.
   - Picture-affecting state is latched at a raster position in a stateful
     beam, never read live at capture or derived from absolute time.
   - No status phase advances per read.
   - DMA requesters accept partial transfers.
   - New run-policy and pacing features count virtual time, not
     instruction equivalents.
   - Bus width and wait states belong in the decode map or the machine
     file, never hidden inside a device.

## Consequences

- About six to eight days of worker time now; the later unit flip then
  costs one to two days instead of an estimated six to ten.
- The crystal unification moves a few goldens once, now, with the reason
  stated.
- Reviews check new code against the rules above.
