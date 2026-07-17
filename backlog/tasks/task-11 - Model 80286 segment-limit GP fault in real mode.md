---
id: task-11
title: Model the 80286 segment-limit #GP fault in real mode (in x86jit)
status: To Do
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

- [ ] Decide whether to model it (weigh: 286 fidelity + corpus pass rate vs cost
      and any DOS/BIOS reliance on 8086-wrap). Record the decision.
- [ ] If yes: enforce the 64 KB offset limit on real-mode memory access and vector
      `#GP(13)` in-guest through the IVT (mirroring the `#DE` delivery path), gated
      to real mode; Long64/Compat32 unchanged.
- [ ] The 286 corpus `#13` and affected memory-form cases pass; no Long64 regression.

## Notes

Related unlifted `#UD`/`#BR` delivery gaps are handled by the lift-gap effort
(task-8), which wires unlifted-opcode → in-guest `#UD`. This task is specifically
the segment-limit `#GP`.
