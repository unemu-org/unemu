---
id: TASK-22
title: M7 — Boot FreeDOS to the C prompt (north star)
status: In Progress
assignee: []
created_date: '2026-07-17 20:29'
updated_date: '2026-07-18 07:48'
labels:
  - dos
dependencies:
  - TASK-21
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The north star. Put the FreeDOS 8086-build kernel (ke2044_86f16, FAT12/16) + an 8086 FreeCOM on the backing image (prefer a vendored prebuilt official FreeDOS image). Boot sector -> kernel.sys -> COMMAND.COM -> the DOS prompt. Close whatever INT-service / real-mode CPU gaps the kernel + shell surface. Golden captures the FreeDOS banner and the prompt via VGA text render and/or serial, fully deterministic.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 FreeDOS 8086 kernel + FreeCOM present on the backing image (vendored, GPL provenance)
- [ ] #2 Boot chain reaches the DOS command prompt
- [ ] #3 Deterministic golden: FreeDOS banner + prompt (VGA text and/or serial)
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Boot infra DONE 2026-07-18. Kiselev BIOS completes full POST, renders hardware-detection screen via INT 10h (to MDA 0xB0000 — BIOS no-video-BIOS path selects MDA), boots vendored FreeDOS 8086 floppy over uPD765+8237: 'Booting OS...' then INT 19h jumps to FreeDOS boot sector. FreeDOS LOADS but spins early (BudgetExhausted, not an unlifted instruction). run_freedos + MDA capture + #[ignore] golden. NORTH STAR (prompt) not yet reached — remaining work is the spin diagnosis (task-25).
<!-- SECTION:NOTES:END -->
