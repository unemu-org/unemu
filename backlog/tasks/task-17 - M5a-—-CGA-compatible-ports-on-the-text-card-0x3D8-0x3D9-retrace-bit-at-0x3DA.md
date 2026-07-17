---
id: TASK-17
title: >-
  M5a — CGA-compatible ports on the text card (0x3D8/0x3D9 + retrace bit at
  0x3DA)
status: To Do
assignee: []
created_date: '2026-07-17 20:28'
labels:
  - devices
  - video
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A stock BIOS drives our text card as CGA via INT 10h: mode-control 0x3D8, color-select 0x3D9, and status register 0x3DA whose retrace bit INT 10h busy-waits on. VgaText decodes only 0x3D4/0x3D5 today, so the BIOS aborts on the first 0x3D8 write and (after open-bus) hangs on the 0x3DA retrace wait. Add CGA-compat registers and a 0x3DA status whose vertical/horizontal retrace bit toggles off the virtual clock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 0x3D8 mode-control and 0x3D9 color-select decoded (stored; text mode honored)
- [ ] #2 0x3DA status register: retrace bit toggles off the virtual clock so INT 10h retrace waits terminate
- [ ] #3 Unit test: a retrace-wait loop makes progress; existing VGA text golden unaffected
<!-- AC:END -->
