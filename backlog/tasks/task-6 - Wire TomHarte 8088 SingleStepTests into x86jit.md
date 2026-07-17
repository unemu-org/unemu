---
id: task-6
title: Wire TomHarte 8088 SingleStepTests as a real-mode CPU oracle (in x86jit)
status: To Do
assignee: []
created_date: 2026-07-17 00:00
labels: [x86jit, oracle]
dependencies: [task-3]
priority: medium
---

## Description

Add the **TomHarte / SingleStepTests 8088** corpus as a silicon-derived,
per-instruction oracle for real mode. Each test is JSON: initial CPU + memory
state, final state, and a cycle-by-cycle bus trace, generated from real 8088/V20
hardware. This is a stronger oracle than the current Unicorn `MODE_16`
differential (which validates against another emulator, not silicon).

**This work lives in `x86jit`, not `unemupc98`** — it is a general CPU-validation
asset for the JIT core (like its existing Unicorn/native oracles). Do it on the
`pc-emulator` worktree, same merge discipline as the real-mode sub-seams. Tracked
here because it is on our roadmap and gated on the real-mode work (task-3).

## Acceptance Criteria

- [ ] A corpus loader in `x86jit-tests` that reads the SingleStepTests 8088 JSON
      (fetched, not vendored into git) into the existing `VectorInput` harness.
- [ ] Run the corpus against the `CpuMode::Real16` interpreter; report per-opcode
      pass/fail; final CPU+memory state must match.
- [ ] (Stretch) Validate the cycle/bus trace where x86jit models it; otherwise
      document that only architectural state is checked, not timing.
- [ ] Triage: any opcode the corpus exercises that x86jit does not yet lift in
      real mode is listed (drives follow-up lift work), not silently skipped.

## Notes

Corpus: `SingleStepTests/ProcessorTests` (x86 = 8088). Do this after M2(c) so the
real-mode interpreter is complete enough to run it meaningfully.
