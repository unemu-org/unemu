---
id: task-10
title: Wire SingleStepTests/80286 as the authoritative real-mode oracle (in x86jit)
status: Done
assignee: []
created_date: 2026-07-17 00:00
labels: [x86jit, oracle]
dependencies: [task-6]
priority: high
---

## Description

`SingleStepTests/80286` is a hardware-generated CPU test corpus for the **Intel
80286** (real Harris 80C286 via ArduinoX86, by Daniel Balsom) — real-mode set
`v1_real_mode`, ~326 instruction forms, ~1.5M executions, with exception data.

The 80286 is our exact target machine (decision-1), so this corpus is the
**authoritative** real-mode oracle: a failure means our bug, with no
8088-vs-286 generation-difference ambiguity. Wire it in x86jit alongside (and
above) the 8088 corpus.

Format is MOO (binary chunked) with a `moo2json.py` converter; fetched, never
vendored (gitignored), mirroring the 8088 setup.

## Acceptance Criteria

- [x] Loader + runner in `x86jit-tests` (direct MOO parser, skips cycle traces),
      run against the `CpuMode::Real16` interpreter via `Vcpu::step_instruction`.
- [x] Compares final CPU state + memory + in-guest IVT exception frames.
- [x] Reports overall + per-opcode pass rate with repros. First run: 1,478,000
      tests — **79.3% overall / 91.9% decided**.
- [x] Corpus fetched via `vendor/80286/fetch.sh`, gitignored; skips when absent.

Merged to `pc-emulator` @ 79e88da. Surfaced: LOCK-prefix mis-decode, missing 286
segment-limit `#GP` (→ task-11), under-delivered `#UD`, and the unlifted-opcode
list (→ task-8).

## Notes

Supersedes task-9's premise (no need to annotate 8088-vs-286 differences once the
286 corpus is the reference). The 8088 corpus stays as a secondary regression net
for shared behaviour. When the 286 protected-mode set lands upstream, it becomes
the oracle for that future CPU seam.

Part of the push to complete real-mode and graduate `pc-emulator` → x86jit `main`.
