---
id: decision-9
title: ROM regions may require RegionKind::Rom (second x86jit seam)
date: 2026-07-17
status: Proposed
deciders: Mikołaj Mikołajczyk
---

# decision-9 - ROM regions may require RegionKind::Rom (second x86jit seam)

## Status

Proposed — 2026-07-17. (Confirmed once the BIOS stub needs executable ROM.)

## Context

The BIOS maps at `F0000h` and must be **read-only but executable**. x86jit's
`RegionKind` today is only `Ram` / `Trap`:

- `Ram` → writable ROM (unfaithful; a bug that writes ROM silently succeeds).
- `Trap` → every access exits as MMIO; code fetch from a Trap region likely does
  not work (the lifter reads code from `Ram`).

## Decision (proposed)

Add a **`RegionKind::Rom`** to x86jit — a second seam alongside real mode
([[decision-3]]), subject to the same [[decision-2]] merge discipline: readable
and executable, writes faulted (or ignored with a trace). Until then, load the
BIOS stub into a `Ram` region and rely on convention (no writes to ROM) for the
minimal stub.

## Consequences

- (+) Faithful ROM semantics; write-to-ROM bugs surface.
- (−) Another x86jit change to design, test, and merge back carefully.

## Trigger to revisit

Confirm and implement when the firmware stub or a real BIOS blob is loaded and
ROM write-protection matters for correctness.
