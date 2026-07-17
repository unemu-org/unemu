---
id: task-10
title: Wire SingleStepTests/80286 as the authoritative real-mode oracle (in x86jit)
status: In Progress
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

- [ ] Loader + runner in `x86jit-tests` reading the 286 `v1_real_mode` corpus
      (MOO→JSON or direct MOO parse), run against the `CpuMode::Real16` interpreter
      via `Vcpu::step_instruction`.
- [ ] Compares final CPU state + memory + exception delivery (the 286 set records
      exceptions — validates our in-guest IVT `#DE`/`#UD`).
- [ ] Reports overall + per-opcode pass rate; genuine failures get a repro.
- [ ] Corpus fetched via a `fetch.sh`, gitignored; test skips cleanly when absent.

## Notes

Supersedes task-9's premise (no need to annotate 8088-vs-286 differences once the
286 corpus is the reference). The 8088 corpus stays as a secondary regression net
for shared behaviour. When the 286 protected-mode set lands upstream, it becomes
the oracle for that future CPU seam.

Part of the push to complete real-mode and graduate `pc-emulator` → x86jit `main`.
