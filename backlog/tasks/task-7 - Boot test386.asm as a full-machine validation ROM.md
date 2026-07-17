---
id: task-7
title: Boot test386.asm as a full-machine validation ROM
status: In Progress
assignee: []
created_date: '2026-07-17 00:00'
updated_date: '2026-07-17 19:55'
labels:
  - validation
dependencies:
  - task-4
priority: low
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Once the machine can actually boot real-mode code with working port I/O and a
serial/POST output path, bring up **test386.asm** (Jeff Parsons / PCjs) — a
self-checking CPU validation ROM that boots like a BIOS, exercises instructions,
and reports pass/fail per subsystem via POST codes / port writes. It is the x86
equivalent of the NES self-checking test ROMs: "boot it and watch it report".

Used by 86Box, MartyPC, PCem, PCjs as a machine-level CPU/firmware validator.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria

- [ ] Load test386.asm's ROM image into the BIOS region and boot it.
- [ ] Capture its POST-code / port-write reporting to the serial/debug sink.
- [ ] Turn its pass/fail output into a golden/integration test.
- [ ] Document which sections pass under our real-mode scope and which need
      protected mode / paging (deferred, later CPU work).

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Spike landed 2026-07-17. test386.bin boots from RAM @ 0xF0000 (reset F000:FFF0). After bumping the stale x86jit pin (a4f6ae5 -> 3ff42a5, which adds far-jmp/seg-load lift), it reaches POST [0x00, 0x01] and stops at SAHF (0x9E) @ F000:0085 in the jcc test (POST 1). PostPort(0x190) + run_test386 capture-on-error + machine retired-cap. Next seam to push further: lift SAHF/LAHF in x86jit Real16, then POST 2 (32-bit mul/div, likely works), POST 3 (mov fs -> needs FS/GS in CpuState), POST 8 (protected mode = wall). Golden pins [0x00,0x01] + stop kind. task-12 pin-bump AC now satisfied.
<!-- SECTION:NOTES:END -->

## Notes

test386.asm targets 386-class features (protected mode, paging); the early
real-mode sections run first and validate the PC/AT-era path we build now. The
full run is a goal for once the CPU seam grows beyond real mode. This lives in
`unemupc98` (it needs the whole machine), unlike the CPU-core oracle (task-6,
which lives in x86jit).
