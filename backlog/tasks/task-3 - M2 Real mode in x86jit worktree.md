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
- [ ] (c) `inject_irq` / `irq_ack` per the decision-4 contract; retired-instruction
      counter for the virtual clock (decision-5).
- [ ] `Long64`/`Compat32` codegen byte-identical (hash test); full x86jit suite
      green; no PS4 benchmark regression.
- [ ] Documented merge back to x86jit `main` when the seam is whole.

## Notes

Largest, riskiest work item. Kept off the critical path of the emulator skeleton.
