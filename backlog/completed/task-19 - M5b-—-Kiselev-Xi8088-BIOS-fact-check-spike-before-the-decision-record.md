---
id: TASK-19
title: M5b — Kiselev Xi8088 BIOS fact-check spike (before the decision record)
status: Done
assignee: []
created_date: '2026-07-17 20:29'
updated_date: '2026-07-17 21:49'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done 2026-07-17. Xi8088 = master+slave cascade (no 0xA0 collision, AT_NMI puts NMI mask on 0x70). ROM 32KiB @ 0xF8000, reset F000:FFF0 (offset 0x7FF0). Floppy INT13h built-in (uPD765 0x3F2-0x3F7 + DMA ch2, page 0x81); HDD needs external XT-IDE option ROM (8KiB @ 0x320). Video = 6845 CGA: needs 0x3D8/0x3D9 + 0x3DA retrace toggle. Also: full 8042 AT command set, PS2_MOUSE probe must fail gracefully, CMOS sane defaults. Vendored fetch.sh @ pinned a4af458.
<!-- SECTION:NOTES:END -->
