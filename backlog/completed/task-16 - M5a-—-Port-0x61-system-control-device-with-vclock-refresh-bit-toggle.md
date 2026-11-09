---
id: TASK-16
title: M5a — Port 0x61 system-control device (with vclock refresh-bit toggle)
status: Done
assignee: []
created_date: '2026-07-17 20:28'
updated_date: '2026-07-17 22:11'
labels:
  - devices
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
BIOS and DOS touch port 0x61 (system control): PIT channel-2 gate + PC-speaker enable, and — critically — bit 4 (refresh detect) which XT-class BIOS delay loops busy-wait on. Model it as a Device, NOT a static latch: bit 4 must toggle off the virtual clock so refresh-timed delays terminate. Include speaker/PIT-gate bits and parity/NMI status bits read back sanely.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Port 0x61 decoded; PIT ch2 gate + speaker bits stored/readable
- [ ] #2 Bit 4 (refresh) toggles as a function of the virtual clock so delay loops that watch it terminate
- [ ] #3 Unit test: a loop polling bit 4 makes progress under the vclock
<!-- AC:END -->
