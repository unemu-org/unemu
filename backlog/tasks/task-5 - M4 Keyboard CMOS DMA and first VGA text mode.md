---
id: task-5
title: M4 — 8042 keyboard, CMOS/RTC, 8237 DMA, first VGA text mode
status: In Progress
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

- [x] `I8042` keyboard controller (ports 0x60/0x64, self-test, scancode queue,
      IRQ1 line, A20 gate bit per decision-8) with unit tests. Merged @ 8c683e7.
- [x] `CmosRtc` (MC146818) with unit tests (NVRAM, time regs, BCD/12h, UIP tick,
      IRQ8 line). Merged @ 8c683e7.
- [x] `Dma8237` device; guest-RAM writes go through a `DmaMemory` handle (no held
      references, no raw writes) so the machine routes them through the
      `note_write`-safe `Vm` API (decision-6). Merged @ 8c683e7.
- [x] VGA text 80×25 as a `Device` (buffer 0xB8000 + CRTC) + the `FrameSink`
      presentation seam (new `video` crate, `Frame::Text`); rendered-frame golden
      ("HELLO"). Card ≠ FrameSink. Merged @ d76517633.

Device-level M4 complete (all four isolated + unit/golden tested; workspace 145
tests pass). **Remaining (integration follow-up):** wire them into the AT machine
— keyboard scancodes → I8042 → IRQ1 → PIC; VGA text buffer → renderer → a real
`FrameSink`; RTC on a slave PIC (IRQ8) once cascade is modelled. Tracked here.

## Notes

Confirms `FrameSink` ≠ card and that snapshot/replay works across the device set.
