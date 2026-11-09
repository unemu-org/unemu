---
id: TASK-29
title: M-cfg.1a — Config data model + validation (new assembly crate)
status: Done
assignee: []
created_date: '2026-07-18 16:24'
updated_date: '2026-07-18 17:16'
labels:
  - architecture
  - config
dependencies:
  - TASK-28
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The declarative data model in a new crate (proposed name 'assembly' — it holds both the config types and the assembler; 'config' alone would mislead). Types: MachineConfig { cpu: CpuConfig, clock: ClockConfig, memory: MemoryConfig, devices/chipset: preset+DeviceConfig list, drives: Vec<DriveConfig>, images: Vec<LoadImage>, output/run-adjacent }. Each DeviceConfig carries a DeviceId, its port range(s), IRQ line, and DMA wiring. Merge the redundancies Fable flagged: ONE image list (LoadImage { base, bytes, kind: Rom | RamLoad }) — not firmware+memory.rom+cpu-blob; ONE drive home (top-level drives, routed by controller_slot) — not drives+Fdc{drives}. CpuConfig (modelled-chip marker + reset vector + seeded segments) and ClockConfig (retired_per_pit_clock as a documented ratio constant + quantum as a first-class field, decision-4) start MINIMAL but exist as the home for future bus-speed/CPU-type. Derive Eq/Debug (config must be identifiable for goldens/snapshots). A ConfigError catalog: port/MMIO overlap, Trap-vs-RAM overlap, ROM out of space, IRQ-line conflict, controller_slot with no controller, image size vs geometry mismatch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 New crate with MachineConfig + sub-structs incl. DeviceId; Eq/Debug derived
- [ ] #2 Image and drive redundancies merged to a single home each
- [ ] #3 ConfigError enumerates the validation failures; CpuConfig/ClockConfig minimal but extensible
- [ ] #4 No MHz field, no behavioural CpuType variant; retired_per_pit_clock documented as a ratio
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fable fix: validation LOGIC + its unit tests land HERE (the crate is standalone-testable) — give task-29 an executable AC: ConfigError-producing validators (overlap, ROM-out-of-space, IRQ/slot conflicts, image-vs-geometry) with unit tests. Removes the 29/30 'who owns validation tests' ambiguity.
<!-- SECTION:NOTES:END -->
