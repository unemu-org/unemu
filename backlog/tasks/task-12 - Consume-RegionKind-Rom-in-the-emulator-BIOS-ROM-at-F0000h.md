---
id: TASK-12
title: 'Consume RegionKind::Rom in the emulator (BIOS ROM at F0000h)'
status: To Do
assignee: []
created_date: '2026-07-17 19:06'
labels:
  - x86jit
  - machine
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The x86jit RegionKind::Rom seam landed on x86jit main (3ff42a5): read-only executable memory, reads/fetches inline at RAM speed, stores dropped on the interpreter path. Now consume it in the emulator: bump the x86jit pin to a rev with the seam, add a ROM region to the machine/CPU for a read-only executable BIOS at F0000h, and a loader path (write_bytes at map time). Blocks test386.asm (task-7). See decision-9.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 x86jit pin bumped to a rev exposing RegionKind::Rom; Cargo.lock updated
- [ ] #2 Machine/CPU can map a ROM region; a blob is loaded read-only executable at F0000h
- [ ] #3 Guest can fetch/execute from the ROM; a store into it is a no-op (golden)
- [ ] #4 No regression: full workspace test + clippy + fmt green
<!-- AC:END -->
