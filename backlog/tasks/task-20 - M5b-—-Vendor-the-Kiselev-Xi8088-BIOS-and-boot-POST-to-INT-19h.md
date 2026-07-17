---
id: TASK-20
title: M5b — Vendor the Kiselev Xi8088 BIOS and boot POST to INT 19h
status: To Do
assignee: []
created_date: '2026-07-17 20:29'
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
