---
id: task-3
title: M2 — Real mode in x86jit (worktree, three sub-seams)
status: In Progress
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

Real-mode is functionally complete on `pc-emulator` — the emulator builds against
it now (M3 can proceed). Remaining before graduating `pc-emulator` → x86jit `main`
(a separate gate, not blocking the emulator):

- [ ] Formal byte-identical-codegen check for Long64/Compat32 + PS4 benchmark pass.
- [ ] Replace the interpreter op-loop closure with an inner `fn` (remove the
      whitespace churn in `interp/mod.rs` before the PS4-side review).
- [ ] Documented merge back to x86jit `main`.

## Notes

Largest, riskiest work item. Kept off the critical path of the emulator skeleton.
