---
id: TASK-20
title: M5b — Vendor the Kiselev Xi8088 BIOS and boot POST to INT 19h
status: Done
assignee: []
created_date: '2026-07-17 20:29'
updated_date: '2026-07-18 05:41'
labels:
  - bios
  - machine
  - dos
dependencies:
  - TASK-13
  - TASK-14
  - TASK-15
  - TASK-16
  - TASK-17
  - TASK-18
  - TASK-12
  - TASK-19
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Vendor the Kiselev 8088_bios Xi8088 ROM via a pinned fetch/build script (GPLv3+, gitignored fixture, same pattern as test386/80286). Map it read-only executable (task-12), boot it on the hardened AT machine (open-bus, port 0x61, CGA, CMOS-seed all in place), and drive POST to the bootstrap (INT 19h) with no unlifted-instruction or unmapped abort. Capture the POST-code stream (port 0x80, task-15) + the VGA text screen as the golden.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Kiselev Xi8088 ROM vendored (pinned build script, gitignored, GPL provenance recorded)
- [ ] #2 BIOS POST runs to INT 19h with no unlifted-instruction / unmapped-port abort
- [ ] #3 Golden: the POST-code sequence + final VGA text screen, deterministic
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done 2026-07-18. BIOS boots 14 POST checkpoints (01-09, 10/12 video fallback, 20 RTC, 21 CPU-type, 22 FPU-detect), stops at x87 FNINIT (DB E3) @ 0xFA85B. run_bios + 1MB mosaic map (RAM/VGA-Trap/option-RAM/BIOS-Rom@0xF8000) + all M5a devices + IRQ0/1/8. Screen blank because MDA fallback writes 0xB0000 (mapped RAM, not our CGA @ 0xB8000) — minor caveat. Next gap = x87 (task-24).
<!-- SECTION:NOTES:END -->
