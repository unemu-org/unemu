---
id: decision-2
title: CPU core via x86jit git worktree + merge discipline
date: 2026-07-17
status: Superseded by decision-13
deciders: Mikołaj Mikołajczyk
---

# decision-2 - CPU core via x86jit git worktree + merge discipline

## Status

Accepted — 2026-07-17. **Superseded by [[decision-13]]** on 2026-11-09: the
`pc-emulator` branch is gone, real mode is on x86jit `main`, and the merge gates
below are now x86jit's own rules.

## Context

The CPU core is the existing `x86jit` JIT (`~/src/x86jit`). It needs new work for
this project (real mode, IRQ injection — [[decision-3]], [[decision-4]]), but its
primary consumer is **PS4 emulation**, which must not regress. We need a way to
evolve x86jit for the emulator without forking off or destabilising PS4 use.

## Decision

Develop x86jit for this project in a dedicated **git worktree** on branch
`pc-emulator` (`git worktree add`). This repo consumes it as a **pinned git
dependency** (branch/rev in `Cargo.toml`) with a local `[patch]` /
`.cargo/config.toml` override onto the worktree for editing. A bare `path`
dependency is banned — it breaks `nix build` / CI.

A change is **mergeable back to x86jit `main`** only when all hold:

1. It sits behind `CpuMode::Real16` / a new branch — generated code for `Long64`
   / `Compat32` is byte-identical (hash the codegen over a block corpus).
2. `CpuState` / `jit_abi` changes are append-only, with an offset-assertion test.
3. x86jit's full suite (interp / JIT / Unicorn / fuzz) is green; no PS4 benchmark
   regression above threshold.
4. The seam merges whole, with its tests — never half a seam.

A **"seam"** = a self-contained capability with its own tests (e.g. real-mode
addressing, IRQ injection, a new `RegionKind`). Golden tests pin the x86jit rev.

## Alternatives considered

- **Hard fork of x86jit** — diverges history, dooms PS4 sync. Rejected.
- **git subtree** — history divergence and merge pain. Rejected.
- **Bare path dependency** — non-reproducible; breaks CI/`nix build`. Rejected as
  the sole mechanism (fine only as the local override under a git-dep pin).

## Consequences

- (+) One origin, clean sync back to x86jit; PS4 path protected by explicit gates.
- (−) Extra ceremony (pin management, `[patch]` override, per-golden rev pinning).

## Trigger to revisit

If PS4 and emulator needs diverge enough that the byte-identical-codegen rule is
untenable, reconsider a maintained fork with periodic rebase.
