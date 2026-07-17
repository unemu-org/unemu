---
id: TASK-19
title: M5b — Kiselev Xi8088 BIOS fact-check spike (before the decision record)
status: To Do
assignee: []
created_date: '2026-07-17 20:29'
labels:
  - bios
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Verify four facts about the chosen open BIOS before writing the decision doc and picking the disk controller, per the plan review. (i) Single- vs dual-PIC: if single-PIC/XT, port 0xA0 is the NMI-mask latch and would COLLIDE with our slave PIC command port — resolve. (ii) ROM image size (8 vs 32 KiB) → mapping base + reset vector. (iii) Hard-disk support: XT-IDE/XT-CF built into the 8088_bios image, or does it require the XTIDE Universal BIOS option ROM (which needs option-ROM scanning + is itself LES/LDS-heavy real-mode code)? (iv) Does its CGA INT 10h path match our VgaText surface. Build the Xi8088 ROM from pinned source to inspect. The disk decision (task-22) and the BIOS decision record depend on this.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Single vs dual PIC confirmed; port-0xA0 collision resolved or ruled out
- [ ] #2 ROM size + map base + reset vector confirmed
- [ ] #3 HDD path determined: built-in XT-IDE vs external XUB option ROM
- [ ] #4 CGA INT 10h port/register surface enumerated vs our text card
<!-- AC:END -->
