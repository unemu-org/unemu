---
id: TASK-18
title: M5a — Seed CMOS/RTC NVRAM so BIOS POST passes its config checksum
status: Done
assignee: []
created_date: '2026-07-17 20:28'
updated_date: '2026-07-17 22:12'
labels:
  - devices
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The BIOS reads its configuration (floppy/HDD types, video type, memory size, equipment) from CMOS NVRAM and validates a checksum; a blank CMOS (our power-on state) fails it and POST stops at an interactive 'press F1 to run setup' wait — poison for a headless golden. Provide a way to seed the MC146818 NVRAM (equipment byte, drive types, base/extended memory size, valid checksum) at machine build time.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 CmosRtc NVRAM can be seeded with a valid config + correct checksum at build time
- [ ] #2 A documented default config matching our device set (no floppy or one HDD, memory size, video)
- [ ] #3 Unit test: seeded checksum validates by the standard CMOS checksum algorithm
<!-- AC:END -->
