---
id: TASK-32
title: M-cfg.2a — RunPolicy split + port run_freedos_interactive (highest risk)
status: Done
assignee: []
created_date: '2026-07-18 16:24'
updated_date: '2026-07-18 18:35'
labels:
  - architecture
  - config
  - machine
  - cli
dependencies:
  - TASK-31
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Separate 'how we run' from 'what the machine is'. RunPolicy = stop conditions (retired_cap, stop-when-prompt) + external stimuli (keyboard script, injection schedule). idle_limit moves from Machine::new into RunPolicy (observer-only) — a machine-crate API change, not just cli. Criterion (Fable): MachineConfig = guest-visible execution INCLUDING quantum (interrupt latency, decision-4; run_bios vs run_freedos_interactive differ only in quantum = two configs of one topology); RunPolicy = stop + stimuli. Document that the poll/injection cadence (DOS_INTERACTIVE_POLL_STEP) is part of the golden identity (deterministic, not 'outside the machine'). Provide a generic run(machine, policy) -> RunResult. Then port run_freedos_interactive LAST/most-carefully: its cumulative cap ladder (boot_to_date_prompt from 0, phase 2 from machine.retired()), injection-on-poll-boundary, 80M command budget, fine quantum 64, MDA prompt detection must reproduce byte-identically; guard with the A/B BiosRun-equality test.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 RunPolicy type splits stop/stimuli from MachineConfig; idle_limit relocated; generic run(machine, policy)
- [ ] #2 run_freedos_interactive ported via config+policy; A/B BiosRun equality holds (A:\ + DIR)
- [ ] #3 Doc: quantum is config (guest-visible); poll/injection cadence is golden identity
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fable fix: SPLIT. This task is now 32a = RunPolicy mechanics ONLY: the RunPolicy type + generic run(machine, policy) -> RunResult (state which crate RunResult lives in and how BiosRun maps onto it), idle_limit relocated from Machine::new (shim the OLD Machine::new signature until task-33 so the A/B old-path stays byte-untouched). Prove 32a by re-expressing an already-ported BORING runner (run_bios) through run(machine, policy) with A/B green. run_freedos_interactive moves to task-36 (32b) — isolate the scary port (cumulative cap ladder, poll-boundary injection, quantum 64, MDA detection) from this API churn.
<!-- SECTION:NOTES:END -->
