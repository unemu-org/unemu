---
id: task-11
title: Model the 80286 segment-limit #GP fault in real mode (in x86jit)
status: Done
assignee: []
created_date: 2026-07-17 00:00
labels: [x86jit, fidelity]
dependencies: [task-10]
priority: low
---

## Description

The 80286 corpus (task-10) showed a genuine target-CPU behaviour we do not model:
a memory access whose offset crosses the 64 KB segment limit raises **#GP
(vector 13)** on the 286, whereas we currently wrap 8086-style. This explains most
of the corpus's `#13` failures and the memory-form `DIV`/shift/`FF` divergences
(the corpus expects a pushed #GP frame; we produce a wrapped result instead).

This is real 286 fidelity, but a **large** change: it touches every effective-
address memory access (a limit check) plus a new in-guest #GP delivery path. It is
deliberately out of scope for the lift-gap effort (task-8).

## Acceptance Criteria

- [x] Modelled: enforce the 64 KB offset limit on real-mode effective-address
      memory access and vector `#GP(13)` in-guest, gated to real mode (`Code16` /
      `mode.wraps_16()`); Long64/Compat32 unchanged. Also the 286 >10-byte
      instruction-length `#GP`. Vector is `#GP(13)` even for SS-relative operands
      (corpus-confirmed, not `#SS`).
- [x] Also closed here: **`#UD` in-guest delivery** for genuinely-invalid-on-286
      opcodes (iced invalid-decode signal; unlifted-but-valid opcodes stay a real
      gap, not masked) — the task-8 residual — and **`BOUND`/`#BR`**.
- [x] 286 corpus: decided pass 0.9518 → **0.9598**, exception delivery 0.5940 →
      **0.9291** (#GP 25→3117, #UD 1264→9275, BOUND 17→3101). No vector regressed,
      no Long64 regression (full jit suite 203/0). Merged to x86jit `main` @ cbba7d9.

## Notes

Residual, out of the effective-address seam (separate follow-up if wanted): the
~341 remaining #6 are opcodes iced decodes but the 286 rejects; string-op (#GP)
and `leave`/`ins`/`outs` cases have their own addressing path.
