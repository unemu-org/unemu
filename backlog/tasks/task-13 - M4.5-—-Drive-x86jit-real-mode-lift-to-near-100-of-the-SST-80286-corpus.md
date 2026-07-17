---
id: TASK-13
title: M4.5 — Drive x86jit real-mode lift to near-100% of the SST/80286 corpus
status: To Do
assignee: []
created_date: '2026-07-17 20:28'
labels:
  - x86jit
  - cpu
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Corpus-first CPU completeness before booting a real BIOS. Enumerate every unlifted real-mode mnemonic in one sweep against SingleStepTests/80286, then land them in 2-3 batched x86jit merges (worktree/pc-emulator discipline), then ONE emulator pin bump. Confirmed-missing starting list: LES/LDS (C4/C5), LAHF/SAHF (9E/9F), PUSHA/POPA, ENTER, INS/OUTS (deliberately unlifted today), SMSW/LMSW/CLTS; plus an explicit ESC/FPU (D8-DF) policy (absent-287 vs no-op). Make the corpus a GATE, not a bystander oracle. Also fix the stale doc-comment in crates/cpu/src/x86jit_backend.rs claiming mov Sreg is unlifted.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 SST/80286 real-mode corpus decided pass rate raised to near-100% (document the residual + why)
- [ ] #2 LES/LDS, LAHF/SAHF, PUSHA/POPA, ENTER lifted and corpus-validated
- [ ] #3 INS/OUTS and ESC/FPU handled per an explicit documented policy
- [ ] #4 Batched into <=3 x86jit merges; emulator pin bumped ONCE; all existing goldens re-based and green
<!-- AC:END -->
