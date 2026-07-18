---
id: TASK-39
title: FDC-W.2 — uPD765 WRITE DATA (0x05) + Drive write-protect
status: Done
assignee: []
created_date: '2026-07-18 21:59'
updated_date: '2026-07-18 22:32'
labels:
  - devices
  - fdc
dependencies:
  - TASK-38
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Add opcode 0x05 (WRITE DATA) to the uPD765: command_length(9, same params as READ) + execute dispatch + cmd_write_data. Resolve drive/geometry with the SAME fault paths as read via a shared command_fault (rename read_fault): empty bay -> ST0 not-ready; CHS out of geometry -> ST1 no-data. Enter Phase::Execution queuing DmaRequest{op: FromMemory{len = sector_count*512}}; the delivered bytes (via dma_complete(DmaOutcome::FromMemory)) are written into media.image at the CHS offset, then build the 7-byte result, advance next-sector, raise IRQ6. Datasheet fidelity (Fable D2): a write is atomic per sector — if the DMA delivered fewer than len bytes (TC mid-sector), zero-fill the remainder of the last sector and still write it; sectors-written advancement is CEIL(delivered/512) (read floors, correct for read). Short image (Fable D5): a write to a valid CHS beyond image.len() resizes the Vec to cover offset+512 (mirror of read's zero-pad). Write-protect (Fable D1 — the flag lives on Drive in THIS task): a read-only Drive terminates WRITE abnormally with ST0 abnormal + ST1 bit1 (NW), no image mutation, no DMA request queued. MSR stays BUSY-only during execution (Fable D6). Read path untouched. Fable-reviewed 2026-07-18.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 WRITE DATA writes a sector to memory-backed media; a subsequent READ DATA of the same CHS returns the written bytes byte-identical (unit test through the real 8237 + mock memory, mirroring read_data_transfers_through_the_8237)
- [ ] #2 Fault parity with read via shared command_fault: write to empty bay = ST0 not-ready; out-of-geometry = ST1 no-data; write-protected Drive = ST0 abnormal + ST1 NW (bit1), no mutation, no request queued (D1,D4)
- [ ] #3 TC mid-sector -> remainder of last sector zero-filled and still written, sectors-written = ceil(delivered/512) (D2); write beyond a short image resizes media.image to offset+512 (D5); MSR==BUSY-only during write execution asserted (D6)
- [ ] #4 Read path untouched (all existing upd765 tests pass); module coverage matrix updated: WRITE DATA now modelled; WRITE DELETED (0x09), SENSE DRIVE STATUS (0x04 ST3 WP), FORMAT TRACK, and MT-on-write all listed as deliberately not modelled (D7)
<!-- AC:END -->
