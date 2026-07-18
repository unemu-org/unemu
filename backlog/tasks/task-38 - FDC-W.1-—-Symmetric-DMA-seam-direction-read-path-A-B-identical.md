---
id: TASK-38
title: 'FDC-W.1 — Symmetric DMA seam (direction), read path A/B-identical'
status: Done
assignee: []
created_date: '2026-07-18 21:58'
updated_date: '2026-07-18 22:22'
labels:
  - devices
  - dma
  - architecture
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Make the machine's DMA seam bidirectional so a bus master can request memory->device transfers, not only device->memory. Today device::DmaRequest{channel,data:Vec<u8>} + Device::dma_service_write + Device::dma_complete(usize) only move bytes device->memory (floppy READ). The 8237 ALREADY has an inherent service_read (memory->device) but it is not on the Device trait, so the machine cannot reach it. Add DmaRequest.op: DmaOp{ToMemory(Vec<u8>) | FromMemory{len}}; add Device::dma_service_read (8237 override delegating to the existing inherent service_read); REPLACE dma_complete(usize) with dma_complete(DmaOutcome{ToMemory{transferred} | FromMemory(Vec<u8>)}) — one completion lifecycle, no separate dma_supply hook (Fable Q2). machine::service_dma branches on op; the FromMemory branch allocs len, runs service_read, and MUST truncate the buffer to the serviced count before delivery (Fable D3). No FDC write behaviour in this task. Read path stays byte-identical (A/B). Fable-reviewed 2026-07-18.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ToMemory path byte-identical: existing upd765 read unit tests + read goldens unchanged
- [ ] #2 machine FromMemory branch runs 8237 service_read and delivers exactly 'transferred' bytes (buffer truncated to serviced count) to the requester via dma_complete(DmaOutcome::FromMemory)
- [ ] #3 Device::dma_service_read on the trait, 8237 override only, default no-op; unit-tested on mock requester + real 8237 + mock memory incl. a TC-clamped (count < len) delivery case
- [ ] #4 snapshot format unchanged (SNAPSHOT_VERSION stays 1; pending_request still dropped at rest, D10); 8237 coverage matrix notes mode/direction mismatch is unmodelled (D8); seam doc (§7.7) updated symmetric
<!-- AC:END -->
