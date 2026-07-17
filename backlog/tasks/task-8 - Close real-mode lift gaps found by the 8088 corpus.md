---
id: task-8
title: Close the real-mode lift gaps the 8088 corpus surfaced (in x86jit)
status: To Do
assignee: []
created_date: 2026-07-17 00:00
labels: [x86jit, oracle]
dependencies: [task-6]
priority: medium
---

## Description

The TomHarte 8088 corpus (task-6) surfaced a concrete list of real-mode gaps in
x86jit. Close them. Work lives in `x86jit` on the `pc-emulator` worktree, same
merge discipline as the other real-mode seams; validate each with the corpus.

## Acceptance Criteria

- [x] Lifted `LOOP`/`LOOPE`/`LOOPNE`/`JCXZ`, the BCD family
      (`DAA`/`DAS`/`AAA`/`AAS`/`AAM`/`AAD`), `CLC`/`STC`/`CMC`, plus `SALC`/`XLAT`
      and `mov`/`push`/`pop Sreg` and far `JMP`/`CALL`/`RETF`. LOCK-prefix decode
      fixed (Real16 `NO_INVALID_CHECK` — 286 executes lock-on-register).
- [x] `/6` shift alias: no code needed — iced decodes `D0`–`D3` `/6` as `SAL`
      (= the 286 `/4` alias); the 8088-only `SETMO` quirk is deliberately not
      reproduced.
- [x] `DIV`/`IDIV`: no bug — register forms already 286-correct (quotient-overflow
      `#DE`, sign, remainder), verified vs Unicorn. The 8088 corpus diffs were 8088
      sign quirks + the `#DE` saved-IP difference; the memory-form diffs are the
      segment-limit `#GP` (task-11), not logic bugs.
- [x] 8088 affected-opcode decided pass rate 0.15 → 0.83; each op validated vs
      Unicorn MODE_16 (incl. a 4096-case BCD sweep). Merged to `pc-emulator` @ 2ca0d50.
- [x] No Long64/Compat32 regression; `cf16` 34/34; jit_abi offsets unchanged.

Residual (small follow-up, intentionally left so the oracle gap list stays honest):
`#UD`-in-guest delivery for unlifted/invalid Real16 opcodes (currently returns
`Exit::UnknownInstruction`), needing a per-opcode "valid on 286?" determination.
Tracked here for pickup with task-11.

## Notes

Far `JMP`/`CALL` (`FF` reg=3,5 — ~19k failures) are a separate, already-tracked
deferral (see `deferred.md`); they are needed for a real BIOS boot, not for this
task.

Reported failures also include 8088-vs-80286 differences where our target is
correct — those are task-9, not bugs to "fix".
