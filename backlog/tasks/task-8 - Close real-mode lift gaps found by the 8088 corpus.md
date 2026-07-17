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

- [ ] Lift the unsupported ops the corpus reports (~97k tests): `LOOP` (`E2`),
      the BCD family `DAA`/`DAS`/`AAA`/`AAS`/`AAM`/`AAD` (`27`/`2F`/`37`/`3F`/`D4`/`D5`),
      and `CLC`/`STC`/`CMC` (`F8`/`F9`/`F5`).
- [ ] Lift the undocumented `/6` shift alias (`D0`–`D3` reg=6 behaves as `/4` SHL)
      — ~26k reported failures.
- [ ] Investigate the `DIV`/`IDIV` divergences (`F6`/`F7` reg=6,7 — ~24k failures):
      determine per case whether it is a genuine interpreter bug or an 8088
      quirk/undefined behaviour our 286 target does not share. Fix the real bugs.
- [ ] Corpus pass rate improves and each closed gap is confirmed by the corpus.
- [ ] No Long64/Compat32 regression; `cf16` still green.

## Notes

Far `JMP`/`CALL` (`FF` reg=3,5 — ~19k failures) are a separate, already-tracked
deferral (see `deferred.md`); they are needed for a real BIOS boot, not for this
task.

Reported failures also include 8088-vs-80286 differences where our target is
correct — those are task-9, not bugs to "fix".
