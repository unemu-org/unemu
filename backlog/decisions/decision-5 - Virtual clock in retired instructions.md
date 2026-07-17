---
id: decision-5
title: Virtual clock in retired instructions
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-5 - Virtual clock in retired instructions

## Status

Accepted — 2026-07-17.

## Context

Deterministic golden tests need a virtual time base. x86jit's `run(budget)`
counts **blocks** (`blocks_run`, an exact counter). But a block has a variable
instruction count, and **block boundaries depend on the lifter version** — so any
x86jit merge would shift `BudgetExhausted` points and break block-based goldens.
There is no wall-clock allowed in the execution path.

## Decision

Measure virtual time in **retired instructions**, not blocks. Add a cheap
instruction counter in the lifter as part of the real-mode seam. PIT/RTC tick off
this virtual clock. Each golden **pins the x86jit revision**; a documented
re-baseline procedure regenerates goldens when the pin moves.

## Alternatives considered

- **Block-based time** — free (counter exists) but brittle across lifter changes.
  Rejected.
- **Wall-clock** — non-deterministic; violates a core principle. Rejected.

## Consequences

- (+) Reproducible goldens stable across most x86jit changes.
- (−) Requires a small x86jit change (instruction counter); goldens still pin a
  rev and need occasional re-baseline.

## Trigger to revisit

If instruction-count determinism proves insufficient (e.g. cross-backend
divergence), consider a cycle-approximation model with an explicit ADR.
