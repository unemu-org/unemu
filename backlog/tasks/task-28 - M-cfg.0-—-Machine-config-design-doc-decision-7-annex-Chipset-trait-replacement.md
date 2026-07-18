---
id: TASK-28
title: >-
  M-cfg.0 — Machine-config design doc + decision-7 annex (Chipset trait
  replacement)
status: Done
assignee: []
created_date: '2026-07-18 16:23'
updated_date: '2026-07-18 17:03'
labels:
  - architecture
  - docs
  - config
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Docs-first foundation for the machine-composition layer. Write backlog/docs/design/machine-config.md capturing the Fable-reviewed design: a declarative MachineConfig assembled by a single data-driven assembler in a new crate, replacing the ~13 hand-written run_* assembly functions and the drifted triple-duplication (AtChipset::build vs assemble_bios_machine). Record the resolved decisions: replace the Chipset trait with data (DeviceConfig list + one assembler) — annex decision-7 (Chipset was seam #2); CMOS derived from MemoryConfig+drives, not configured; no MHz/frequency axis and no behavioural CpuType toggle now (retired-instruction determinism; no oracle for an 8086 mode) BUT CpuConfig/ClockConfig exist as minimal-but-extensible structs so bus-speed/CPU-type are later field additions, not a re-architecture; assembler lives above the machine crate (machine core must not name concrete devices/CPU — decision-7). Document A20 as a model constant, the option-ROM area as zeroed RAM, and that config must be identifiable (Eq/Debug) for goldens + future snapshots.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 backlog/docs/design/machine-config.md written: data model, assembler, presets, RunPolicy split, migration strategy, non-goals
- [ ] #2 decision-7 annex (or a new decision) records the Chipset-trait replacement and the assembler's placement above the machine crate
- [ ] #3 The freq/CPU-type stance is documented: extensible structs now, behavioural axes when a consumer + oracle exist
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fable task-review fixes: (1) Resolve the demos/run_serial fate HERE in non-goals — port the 4 Real16 demos (run_boot/keyboard/rtc/vga_text) to config in task-31; inline run_serial's Long64 assembly trait-free (no config, no mode axis) or delete it+its golden. This unblocks task-33's trait removal. (2) decision-7 annex single home: task-28 WRITES it; task-33 only verifies/updates it against final code. (3) Non-goals must name: snapshot deferral (add to deferred.md), the no-Long64-config-axis stance, any deleted demo. (4) Document the task-33 deletion gate (all A/B green on the pre-deletion commit) in the migration-strategy section.
<!-- SECTION:NOTES:END -->
