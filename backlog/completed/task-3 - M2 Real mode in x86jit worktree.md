---
id: task-3
title: M2 — Real mode in x86jit (worktree, three sub-seams)
status: Done
assignee: []
created_date: 2026-07-17 00:00
labels: [milestone-2]
dependencies: [task-2]
priority: high
---

## Description

Add real-mode support to x86jit behind `CpuMode::Real16`, in the `pc-emulator`
worktree, honouring the merge discipline (decision-2, decision-3). Three sub-seams
with independent exit criteria.

## Acceptance Criteria

- [x] (a) Segment registers (CS/DS/ES/SS) in `CpuState` (append-only, offset test)
      + 16-bit decode + `segment:offset` addressing with 64K/`SP` wrap; oracle:
      Unicorn 16-bit (6/6 differential). Merged to `pc-emulator` @ ce51d88.
- [x] (b) `INT` / `IRET` / IVT + `IF` in `Flags` (pad byte, ABI-safe) + `#DE`/`#UD`
      vectored through the IVT in real mode. Merged to `pc-emulator` @ ce369c8.
      Deferred: TF flag, STI shadow, far jmp/call/retf.
- [x] (c) `inject_irq` (between-run delivery, IF/pending/STI-shadow/HLT-wakeup gating)
      + retired-instruction counter on `Vcpu` (no `CpuState`/`jit_abi` change).
      Merged to `pc-emulator` @ b6f0082. cf16 differential 16/16.
- [x] Full x86jit suite green (only two pre-existing host-specific failures, which
      fail identically on pristine x86jit main); Long64/Compat32 paths mode-gated.

**Graduated to x86jit `main`** @ 40acdbc (merge of `pc-emulator`); `pc-emulator`
fast-forwarded to `main` — in sync, no drift.

- [x] Interpreter op-loop closure replaced with an inner `fn` (churn removed).
- [x] Long64/Compat32 paths mode-gated throughout; `jit_abi` offsets unchanged;
      new Real16 IR ops `unreachable!` in the JIT. Post-merge check of the engine's
      other modes: jit suite 202/205 (the 1 fail is a pre-existing AVX2 VEX fuzz bug
      on main, unrelated), i386 3/3, workspace builds. Formal codegen-hash +
      benchmark for the engine's other modes skipped by maintainer decision
      (mode-gating deemed sufficient).
- [x] Merged to x86jit `main`.

Full real-mode set validated: SingleStepTests/80286 silicon corpus (1.48M tests,
**89.7% overall / 95.2% decided**), TomHarte 8088, Unicorn MODE_16 (cf16 34/34).
Deferred to their own tasks: `#UD`-in-guest delivery (task-8 residual),
segment-limit `#GP` (task-11), BOUND lift.

## Notes

Largest, riskiest work item. Kept off the critical path of the emulator skeleton.
