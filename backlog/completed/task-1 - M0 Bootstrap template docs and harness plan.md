---
id: task-1
title: M0 — Bootstrap template, docs, and harness plan
status: Done
assignee: []
created_date: 2026-07-17 00:00
labels: [milestone-0]
dependencies: []
priority: high
---

## Description

Transplant the `agentic-project-template`, produce `AGENTS.md`/`CLAUDE.md` with
the project rules, and design the architecture + test harness as documentation
(no emulator code). Establishes collaboration rules and the plan for M1+.

## Acceptance Criteria

- [x] Template transplanted; `BOOTSTRAP.md` removed, `LICENSE` = AGPL-3.0-or-later,
      project `README.md`.
- [x] `AGENTS.md`: language rule (PL chat / EN artifacts), x86jit worktree + merge
      discipline, engineering hard rules (test/swap/docs/determinism).
- [x] `backlog/docs/architecture.md`: six swap seams, interrupt model, virtual
      clock model, memory/DMA (`note_write`) model.
- [x] `backlog/docs/testing.md`: unit + fuzz/property + coverage matrix + golden +
      SingleStepTests differential + snapshot/replay.
- [x] `backlog/docs/design/spec.md`: numbered spec outline.
- [x] `backlog/decisions/`: ADRs (target, worktree, real mode, IRQ contract,
      clock, note_write, traits, A20, ROM, 286, license).
- [x] `flake.nix` carries the Rust toolchain + nasm + qemu; `nix develop` enters.

## Notes

Doc-only milestone. No Rust code. Sequenced so the risky real-mode work (M2) does
not block the rest.
