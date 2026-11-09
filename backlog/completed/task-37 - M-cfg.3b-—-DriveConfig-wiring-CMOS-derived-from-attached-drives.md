---
id: TASK-37
title: M-cfg.3b — DriveConfig wiring + CMOS derived from attached drives
status: Done
assignee: []
created_date: '2026-07-18 16:34'
updated_date: '2026-07-18 19:27'
labels:
  - architecture
  - config
  - devices
dependencies:
  - TASK-33
  - TASK-34
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The config-side of drives, after the device-level geometry work (task-34) and the trait removal (task-33). DriveConfig { kind: Floppy{geometry} | HardDisk{geometry}, image: ImageSource, controller_slot } as first-class MachineConfig data; the assembler routes each drive to its controller (uPD765 today) and DERIVES the CMOS floppy_types + equipment bits from the attached drives (no manual editing — the whole point of the maintainer's 'attach different drive types' vision). This is the attachment seam a future XT-IDE/ATA HDD device plugs into with no new assembler code.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 DriveConfig is first-class MachineConfig data; assembler routes drives by controller_slot
- [ ] #2 CMOS floppy_types/equipment derived from attached drives; adding a 2nd floppy needs no manual CMOS edit
- [ ] #3 Documented as the seam a future IDE/HDD plugs into; a guest sees the configured geometry
<!-- AC:END -->
