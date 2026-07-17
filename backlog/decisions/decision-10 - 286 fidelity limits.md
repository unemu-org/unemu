---
id: decision-10
title: 286 fidelity limits (flags, cpuid) and consequences for a real BIOS
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-10 - 286 fidelity limits (flags, cpuid) and real-BIOS consequences

## Status

Accepted — 2026-07-17.

## Context

x86jit models a **modern** CPU (`features.rs` advertises recent ISA levels).
Period software detects CPU generation through quirks: `pushf` bits 12–15 on
16-bit CPUs, absence of `cpuid` on a true 286, specific `popf` behaviour, etc. A
faithful 80286 differs from what x86jit exposes.

## Decision

Accept **modern-CPU fidelity limits** for now. For our own minimal BIOS stub and
hand-written test-ROMs this is irrelevant (we don't run CPU-detection probes). The
limitation is recorded so that any future goal of running a **real, unmodified
BIOS** treats 286 detection quirks as explicit, known risks to address then.

## Consequences

- (+) No CPU-fidelity work blocks the current milestones.
- (−) A real period BIOS may misdetect the CPU or take unexpected paths; not
  supported until this is revisited.

## Trigger to revisit

When loading a real BIOS blob, or when a test-ROM legitimately needs 286-accurate
`pushf`/`popf`/no-`cpuid` behaviour.
