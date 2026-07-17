---
id: TASK-21
title: M6 — Disk controller + INT 13h boot-sector read
status: To Do
assignee: []
created_date: '2026-07-17 20:29'
updated_date: '2026-07-17 21:49'
labels:
  - devices
  - dos
dependencies:
  - TASK-19
  - TASK-20
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Add ONE disk controller and get the BIOS's INT 13h to read sector 0 of a FreeDOS image into 0x7C00, then INT 19h jumps to it. Path decided by the task-19 fact-check: (A) XT-IDE / 8-bit-ATA PIO task-file device (no DMA) + a prebuilt FreeDOS HDD FAT16 image; if the BIOS needs it, also vendor+map a config-matched XTIDE Universal BIOS option ROM and add option-ROM scanning. OR (B) NEC uPD765 FDC (0x3F0-0x3F7) wired to our existing 8237 DMA channel 2 + a 1.44M FreeDOS floppy image (no partition table, BIOS built-in INT 13h). Prefer a prebuilt official FreeDOS image. Backing image read via a deterministic handle, no wall clock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Disk controller Device implemented (path per task-19) with isolation tests
- [ ] #2 A backing FreeDOS image is read deterministically (sector granularity)
- [ ] #3 BIOS INT 13h reads sector 0 to 0x7C00 and INT 19h transfers control to it (observed)
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Disk path DECIDED (task-19): uPD765 floppy — built into the Xi8088 BIOS INT 13h, drives FDC 0x3F2-0x3F7 + our existing 8237 DMA channel 2 (page reg 0x81). Boot a FreeDOS 1.44M floppy image (no partition table/MBR). XT-IDE HDD (PIO + XUB 8KiB option ROM @ 0x320 + option-ROM scan) is a DEFERRED step 2. Model the uPD765 command/result phases, IRQ6 seek-complete, motor timing off the vclock.
<!-- SECTION:NOTES:END -->
