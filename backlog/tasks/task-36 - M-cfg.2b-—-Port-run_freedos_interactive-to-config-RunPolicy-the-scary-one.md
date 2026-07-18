---
id: TASK-36
title: M-cfg.2b — Port run_freedos_interactive to config + RunPolicy (the scary one)
status: To Do
assignee: []
created_date: '2026-07-18 16:33'
labels:
  - architecture
  - config
  - cli
dependencies:
  - TASK-32
  - TASK-35
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Port the highest-risk runner LAST and alone. run_freedos_interactive's cumulative retired-cap ladder (boot_to_date_prompt from 0, phase 2 continuing from machine.retired()), scancode injection on a poll boundary, the 80M command budget, the fine DOS_INTERACTIVE_QUANTUM=64, and MDA prompt detection must reproduce BYTE-IDENTICALLY through the config+RunPolicy path — the poll/injection cadence is part of the golden identity. Guard with the A/B result-equality harness (task-35): the config path must produce the identical BiosRun (reaching A: + DIR) as the legacy path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 run_freedos_interactive runs via config + RunPolicy; A/B BiosRun equality holds (A:\ prompt + DIR listing byte-identical)
- [ ] #2 The cap ladder / injection cadence / quantum reproduce the legacy behaviour exactly
<!-- AC:END -->
