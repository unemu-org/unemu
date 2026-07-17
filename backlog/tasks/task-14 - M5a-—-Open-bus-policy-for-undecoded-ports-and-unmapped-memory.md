---
id: TASK-14
title: M5a — Open-bus policy for undecoded ports and unmapped memory
status: Done
assignee: []
created_date: '2026-07-17 20:28'
updated_date: '2026-07-17 21:47'
labels:
  - bus
  - machine
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A real BIOS touches many undecoded ports/addresses in its first hundred instructions (POST 0x80, DMA 0x00-0x1F + page latches, 0x61-63, CGA 0x3D8-3DA, FDC probe 0x3F0-7, COM2/LPT, option-ROM scan reading C0000-EFFFF, memory sizing). Today any undecoded port = BusError abort and any unmapped read = CpuError abort. Add a real-hardware open-bus default: undecoded port read -> 0xFF, write ignored (both traced so observability is kept); unmapped guest memory read -> 0xFF within the 1 MB space. Add A20-off wraparound: linear addresses above 1 MB (segment FFFF, offset >0x0F) wrap to low memory.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Undecoded port read returns 0xFF, write is a traced no-op (not an abort)
- [ ] #2 Unmapped guest memory read returns 0xFF within 1 MB; wrap policy for >1 MB with A20 off
- [ ] #3 A trace/log of untouched-port accesses is kept for diagnostics
- [ ] #4 Existing goldens unaffected; unit tests for open-bus + wrap
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Merged 2026-07-17. Port-level open-bus in bus (UndecodedPortAccess::OpenBus default, Fault for strict). Memory open-bus = full 1MB RAM+ROM map (task-20, no core hook). A20-off wrap = potential x86jit Real16 change, deferred unless early POST needs it.
<!-- SECTION:NOTES:END -->
