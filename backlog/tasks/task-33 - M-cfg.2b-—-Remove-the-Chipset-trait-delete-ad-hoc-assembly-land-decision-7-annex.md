---
id: TASK-33
title: >-
  M-cfg.2b — Remove the Chipset trait, delete ad-hoc assembly, land decision-7
  annex
status: To Do
assignee: []
created_date: '2026-07-18 16:24'
updated_date: '2026-07-18 16:34'
labels:
  - architecture
  - config
  - chipset
dependencies:
  - TASK-36
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Once all consumers are on the config+assembler path, remove the Chipset trait and AtChipset/SerialChipset (subsumed by preset functions), and delete the old hand-written assembly (assemble_bios_machine, run_bios_inner, the per-run device-set/DecodeMap/IrqLine/DmaWiring hardcoding, the BIOS_*_INDEX consts). Land the decision-7 annex (Chipset trait replaced by data + a single assembler; the decode-ownership invariant survives as 'preset declares (data) -> assembler translates -> bus executes', and the IRQ/DMA wiring the frontend used to declare now lives in the preset).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Chipset trait + AtChipset/SerialChipset removed; no dead hand-written assembly remains
- [ ] #2 BIOS_*_INDEX constants gone; all device addressing is by DeviceId
- [ ] #3 decision-7 annex committed; all goldens green
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fable fixes: (1) Depends on task-36 (both port tasks done). (2) PREREQUISITE: the 4 Real16 demos ported (task-31) — else AtChipset can't be removed. (3) The deletion commit contains ONLY deletions; no behaviour change smuggled in (unverifiable after A/B tests die). (4) A/B tests are removed TOGETHER with the old paths they compare; goldens remain the sole guard and must be green. (5) decision-7 annex: written in task-28; here just verify/update it against the final code state. Gate: all A/B tests passed on the final pre-deletion commit.
<!-- SECTION:NOTES:END -->
