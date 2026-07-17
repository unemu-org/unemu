---
id: task-6
title: Wire TomHarte 8088 SingleStepTests as a real-mode CPU oracle (in x86jit)
status: Done
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

- [x] Corpus loader + runner in `x86jit-tests` (`src/harte.rs`, `tests/harte8088.rs`);
      corpus fetched via `vendor/8088/fetch.sh`, gitignored (492 MB), never vendored.
      Added `Vcpu::step_instruction` (additive single-step primitive).
- [x] Runs against the `CpuMode::Real16` interpreter with per-opcode reporting.
      First run: **1,824,000 tests — 83.6% overall, 92.9% decided** pass rate.
- [x] Only architectural state is compared (not the cycle/bus trace); flag
      comparison uses the corpus's own per-opcode undefined-flag masks.
- [x] Gap list produced (drives task-8 / task-9), nothing silently skipped.

Merged to `pc-emulator` @ 20c8d62.

## Notes

Corpus: `SingleStepTests/ProcessorTests` (x86 = 8088). Do this after M2(c) so the
real-mode interpreter is complete enough to run it meaningfully.
