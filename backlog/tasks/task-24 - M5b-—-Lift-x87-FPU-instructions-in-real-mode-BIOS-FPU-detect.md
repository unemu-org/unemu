---
id: TASK-24
title: M5b — Lift x87 FPU instructions in real mode (BIOS FPU-detect)
status: To Do
assignee: []
created_date: '2026-07-18 05:41'
labels:
  - x86jit
  - cpu
  - bios
  - dos
dependencies:
  - TASK-20
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The Kiselev BIOS POST reaches checkpoint 0x22 (detect FPU) and stops at x87 FNINIT (DB E3) at linear 0xFA85B — Exit::UnknownInstruction. x86jit already models x87 for Long64/Compat32; the Real16 lift path just doesn't route the x87 opcode range. Extend the Real16 lift to route the x87 instructions the BIOS uses (at minimum FNINIT/FINIT, then the FPU-detect readback — FNSTSW/FNSTCW, and whatever early POST + DOS touch: FLDCW, FSTENV/FLDENV, the D8-DF arithmetic already validated by the 286 corpus) to the existing x87 IR/model, Real16-gated. Note: the 286 corpus only covered D8, so DB/D9-DF are the untested surface — validate against the corpus where present and Unicorn/x87 model otherwise. x86jit worktree + merge discipline; re-pin after. This unblocks the BIOS past FPU detect toward INT 19h.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 FNINIT + the x87 control/status instructions the BIOS FPU-detect uses are lifted in Real16, routed to the existing x87 model
- [ ] #2 BIOS POST advances past checkpoint 0x22 (FPU detect) to the next stop
- [ ] #3 No Long64/Compat32 regression (jit/smc/jit_abi); Real16-gated; re-pin bumped
<!-- AC:END -->
