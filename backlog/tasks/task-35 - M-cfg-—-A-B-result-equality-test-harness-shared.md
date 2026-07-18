---
id: TASK-35
title: M-cfg — A/B result-equality test harness (shared)
status: Done
assignee: []
created_date: '2026-07-18 16:33'
updated_date: '2026-07-18 19:27'
labels:
  - architecture
  - config
  - testing
dependencies:
  - TASK-30
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Shared deliverable both the port tasks (31, 36) depend on and which no task currently owns. BiosRun derives only Clone+Debug today. Add full-result comparison: PartialEq (or a diffing assert_*_eq with per-field diffs — better for debugging divergence) on BiosRun, TextGrid, StopReason, and the per-runner result types (Test386Run, the Vec<u8> serial results). A/B tests then assert the config-assembled machine produces a result byte-identical to the legacy hand-assembled one, per each runner's return type. Per-field diffing beats a bare == so a divergence names the field.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 BiosRun/TextGrid/StopReason + per-runner result types are fully comparable (PartialEq or a diffing helper)
- [ ] #2 A reusable assert_result_eq(legacy, config) helper with per-field diff output
<!-- AC:END -->
