---
id: task-5
title: M4 — 8042 keyboard, CMOS/RTC, 8237 DMA, first VGA text mode
status: To Do
assignee: []
created_date: 2026-07-17 00:00
labels: [milestone-4]
dependencies: [task-4]
priority: low
---

## Description

Broaden the device set and add the first graphics mode. VGA text lands as an
ordinary `Device` plus a `FrameSink`, proving the graphics seam.

## Acceptance Criteria

- [ ] `I8042` keyboard controller (and the A20 gate, decision-8) with unit tests.
- [ ] `CmosRtc` device with unit tests.
- [ ] `Dma8237` device; all RAM writes go through the `note_write` API (decision-6).
- [ ] VGA text 80×25 as a `Device` + a `FrameSink` presentation sink; snapshot test
      of a rendered text frame.

## Notes

Confirms `FrameSink` ≠ card and that snapshot/replay works across the device set.
