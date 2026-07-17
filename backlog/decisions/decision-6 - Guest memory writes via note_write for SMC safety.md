---
id: decision-6
title: Guest memory writes via note_write (SMC safety)
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-6 - Guest memory writes via note_write (SMC safety)

## Status

Accepted — 2026-07-17.

## Context

x86jit tracks self-modifying code: writes into guest RAM must go through the `Vm`
API that runs `note_write` so translated blocks overlapping the written pages are
invalidated (`memory.rs`). Emulator components that write RAM outside the CPU —
the 8237 DMA controller, the BIOS loader, and future shadow RAM — could bypass
this by writing the raw buffer, silently corrupting block invalidation.

## Decision

**All guest-RAM writes go through the `Vm` API that performs `note_write`** —
never a raw buffer write. Devices do not hold references into guest memory; they
receive a memory handle in `tick` / `dma_transfer`. This is a hard rule
(also in `conventions.md`).

## Consequences

- (+) SMC/translation coherence holds for DMA, loaders, shadow RAM.
- (−) A small indirection cost on bulk device writes; must design device APIs to
  take a handle rather than a slice.

## Trigger to revisit

If bulk DMA throughput becomes a bottleneck, design a batched-write API that still
issues `note_write` for the touched range — not a raw bypass.
