---
id: TASK-31
title: M-cfg.1c — Presets + port the PC-family run_* to config (A/B equality)
status: Done
assignee: []
created_date: '2026-07-18 16:24'
updated_date: '2026-07-18 18:09'
labels:
  - architecture
  - config
  - cli
dependencies:
  - TASK-30
  - TASK-35
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Presets as plain functions emitting a MachineConfig (fn at_class(params) -> MachineConfig, fn serial(), etc.) — the ergonomic layer over the flat DeviceConfig list; the current AtChipset/SerialChipset become presets. Then port the PC-family run_* (test386, floppy_boot, bios, bios_boot, freedos) to build-a-config + assemble, KEEPING the old assembly paths alongside during migration. Guard each port with an A/B test asserting FULL BiosRun equality (post + screen + mda + com1 + stop), not just the golden files (goldens don't cover every observable). Order: run_test386/run_floppy_boot (simplest full mosaics) first. The Long64 run_serial and the pedagogical demos (run_boot/run_keyboard/run_rtc/run_vga_text) are OUT of mandatory scope — port opportunistically or leave; do not grow the config with a mode axis for one Long64 user.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Presets emit MachineConfig; AtChipset/SerialChipset equivalents exist as presets
- [ ] #2 test386/floppy_boot/bios/bios_boot/freedos run via config; A/B full-BiosRun-equality tests pass
- [ ] #3 Old assembly paths still present (deleted in M-cfg.2b); all goldens byte-identical
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fable fixes: (1) Depends on task-35 (A/B harness). (2) ALSO port the 4 Real16 demos (run_boot/run_keyboard/run_rtc/run_vga_text) to config here — they are small and the config covers them; leaving them makes task-33 (trait removal) unexecutable. run_serial is handled in task-28's decision (inline trait-free / delete). (3) AC#2 wording: run_test386 returns Test386Run and run_floppy_boot returns Vec<u8> (not BiosRun) -> 'full-RESULT equality per each runner's return type', not 'full-BiosRun-equality'. Order: test386/floppy_boot first (simplest), the BiosRun family next.
<!-- SECTION:NOTES:END -->
