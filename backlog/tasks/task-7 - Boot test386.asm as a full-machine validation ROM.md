---
id: task-7
title: Boot test386.asm as a full-machine validation ROM
status: To Do
assignee: []
created_date: 2026-07-17 00:00
labels: [validation]
dependencies: [task-4]
priority: low
---

## Description

Once the machine can actually boot real-mode code with working port I/O and a
serial/POST output path, bring up **test386.asm** (Jeff Parsons / PCjs) — a
self-checking CPU validation ROM that boots like a BIOS, exercises instructions,
and reports pass/fail per subsystem via POST codes / port writes. It is the x86
equivalent of the NES self-checking test ROMs: "boot it and watch it report".

Used by 86Box, MartyPC, PCem, PCjs as a machine-level CPU/firmware validator.

## Acceptance Criteria

- [ ] Load test386.asm's ROM image into the BIOS region and boot it.
- [ ] Capture its POST-code / port-write reporting to the serial/debug sink.
- [ ] Turn its pass/fail output into a golden/integration test.
- [ ] Document which sections pass under our real-mode scope and which need
      protected mode / paging (deferred, later CPU work).

## Notes

test386.asm targets 386-class features (protected mode, paging); the early
real-mode sections run first and validate the PC/AT-era path we build now. The
full run is a goal for once the CPU seam grows beyond real mode. This lives in
`unemupc98` (it needs the whole machine), unlike the CPU-core oracle (task-6,
which lives in x86jit).
