---
id: TASK-30
title: 'M-cfg.1b — The assembler: MachineConfig -> Machine (in the assembly crate)'
status: Done
assignee: []
created_date: '2026-07-18 16:24'
updated_date: '2026-07-18 17:30'
labels:
  - architecture
  - config
  - machine
dependencies:
  - TASK-29
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A single data-driven assembler that turns a validated MachineConfig into a running Machine, replacing assemble_bios_machine + the 3 CPU constructors' hardcoding. Steps: (1) build the CPU + memory map — MemoryConfig drives map_ram/map_rom/map_mmio (collapsing new_real16/new_real16_mmio/new_real16_bare), reset vector + segments from CpuConfig; (2) build the device set + DecodeMap + InterruptController + IrqLines + DmaWirings from the DeviceConfig list, and return a DeviceId->bus-index map so drain/inject paths address devices by id, NOT hardcoded indices (kills BIOS_UART_INDEX..BIOS_DMA_INDEX); (3) DERIVE the CMOS config (floppy_types, equipment, base/extended memory) from MemoryConfig + drives so the BIOS never sees a self-inconsistent machine (explicit override only for testing mismatch); (4) explicit decode-conflict policy — first-match order is semantics (0x80 POST vs DMA page latches 0x81), so sort deterministically and ERROR on overlap (with a declared exception for intentional overlaps). Lives ABOVE the machine crate (decision-7). Validation runs before assembly (ConfigError).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Assembler builds a Machine from MachineConfig; CPU/memory/devices/wiring all config-driven
- [ ] #2 Returns a DeviceId->index map; drain/inject use it, no hardcoded device indices
- [ ] #3 CMOS is derived from memory+drives; decode-conflict overlap is a hard error
- [ ] #4 Unit tests: a config round-trips to the expected device set + decode map + wiring; validation rejects bad configs
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fable fixes: (1) AC#2 unverifiable in 30 (drain/inject live in run_*, ported in 31) -> restate as 'assembler returns a DeviceId->index map; a unit test asserts the expected map for a reference config'. The global 'no hardcoded indices anywhere' belongs to task-33. (2) Reword 'collapsing new_real16*' -> 'BYPASSING them via config-driven mapping'; the 3 constructors are DELETED in task-33, not here (can't collapse while old paths call them). (3) Add AC: the DERIVED CmosConfig for the FreeDOS preset must equal the current CmosConfig::default() byte-for-byte (the spot derivation could silently shift CMOS and break the BIOS equipment probe).
<!-- SECTION:NOTES:END -->
