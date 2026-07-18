---
id: TASK-34
title: M-cfg.3 — DriveConfig + floppy geometries (the seam under IDE/HDD)
status: To Do
assignee: []
created_date: '2026-07-18 16:25'
updated_date: '2026-07-18 16:34'
labels:
  - architecture
  - config
  - devices
dependencies:
  - TASK-30
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Make drives first-class config data — the seam the maintainer's 'attach different drive types' vision plugs into. DriveConfig { kind: Floppy{geometry} | HardDisk{geometry}, image: ImageSource (bytes | file | blank), controller_slot }. Support the standard floppy geometries (1.44M 80x2x18, 1.2M 80x2x15, 720K 80x2x9, 360K 40x2x9) as data, and multiple floppies (drive 0/1). The assembler routes each DriveConfig to its controller (the uPD765 today) and the CMOS floppy_types + equipment bits are DERIVED from the attached drives (task-30). This establishes the attachment seam a future XT-IDE/ATA HDD device plugs into without new assembler code. Verify a guest sees the configured geometry (a floppy read at a valid CHS for the geometry works; out-of-geometry fails).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 DriveConfig with the 4 standard floppy geometries + multi-floppy attach; assembler routes by controller_slot
- [ ] #2 CMOS floppy_types/equipment derived from attached drives (no manual editing)
- [ ] #3 The attachment seam is documented as where a future IDE/HDD device plugs in; geometry honored by the FDC
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fable fix: SPLIT + re-sequence. This task is now 34a = DEVICE-LEVEL uPD765 work only: parameterize the CHS->offset formula + SECTORS_PER_TRACK for the 4 geometries (1.44M/1.2M/720K/360K) and support per-drive media (it already decodes 4 drive-select bits but holds a single image). Mock-bus isolation tests (testability-first). CRITICAL: keep the existing with_image / FDC constructor entry point BYTE-COMPATIBLE so the legacy assemble_bios_machine path (still A/B-compared during 31/32) is unperturbed — only then may 34a run parallel to 31/32. Requires >=1 non-1.44M geometry exercised by a real image read (mtools is in the devShell; a 360K image is trivial). The DriveConfig wiring (routing + CMOS-from-drives) moves to task-37 (after task-33).
<!-- SECTION:NOTES:END -->
