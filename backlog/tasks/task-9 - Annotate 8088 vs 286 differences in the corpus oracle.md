---
id: task-9
title: Annotate 8088-vs-80286 differences in the corpus oracle so failures mean our bugs
status: To Do
assignee: []
created_date: 2026-07-17 00:00
labels: [x86jit, oracle]
dependencies: [task-6]
priority: medium
---

## Description

The TomHarte corpus captures a real **8088**; our target is an **80286**. Some
reported failures are architectural differences where our 286 behaviour is
*correct* and the 8088's is not what we want to model. Today they land in the
"fail" bucket and drown the real signal.

Annotate the known generation differences so a `fail` means "our bug".

## Acceptance Criteria

- [ ] Classify the known 8088-only behaviours into their own reported bucket
      (e.g. `generation_diff`), separate from `fail`. At minimum:
      - `PUSH SP` (`54`, ~9.4k): the 8088 pushes the already-decremented SP; the
        286+ pushes the original value. We model 286.
      - `RCL`/`RCR` and the shift family count masking (`D2`/`D3`, e.g. `D3.2`
        ~1.9k): the 8086/8088 use the full `CL` count; the 186+ mask it to 5 bits.
        We model 286.
- [ ] Document each entry with the datasheet/erratum reason — this is a research
      artifact, not a silence list.
- [ ] Once the buckets are clean, consider gating the test on a pass-rate
      threshold (a regression gate) instead of pure reporting.

## Notes

Do not "fix" these toward 8088 behaviour — the target machine is a 286
(decision-1). The point is to stop counting them as failures.
