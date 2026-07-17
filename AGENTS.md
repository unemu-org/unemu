# AGENTS.md — unemupc98

Repo-specific notes for coding agents (Claude Code, Cursor, Aider, Copilot, …). Generic software-engineering advice is out of scope.

> **CLAUDE.md** at repo root is `@AGENTS.md` plus Claude-only overrides. Other agents read this file directly.

## What this is

`unemupc98` is a **low-level emulator (LLE) of an IBM PC/AT-class (Intel 80286)
computer**. The CPU core is the [`x86jit`](../x86jit) x86 JIT (consumed via a git
worktree, see below). It is a **research project**: the priorities are
**testability**, **swappability** (CPU / chipset / device / graphics behind
traits), and **dense documentation** (in-source doc-comments + `backlog/docs/`).
First end-to-end target is **headless serial** (16550 UART), captured
deterministically for golden tests; graphics come later as swappable modes.

## Language

- **Talk to the maintainer in Polish.**
- **Everything committed is English**: code, comments, doc-comments, docs under
  `backlog/docs/`, commit messages, branch names, task/decision titles, and all
  identifiers. No Polish in the repository.

## CPU core: x86jit (worktree + merge discipline — hard rule)

The CPU is **not vendored or copied**. `x86jit` (`~/src/x86jit`) is developed for
this project in a dedicated **git worktree** on branch `pc-emulator`
(`git worktree add`). This repo depends on it as a **pinned git dependency**
(branch/rev in `Cargo.toml`) with a local `[patch]` / `.cargo/config.toml`
override onto the worktree for development — a bare `path` dependency is banned
because it breaks `nix build` / CI.

`x86jit`'s primary consumer is **PS4 emulation**; changes there must never shift
its behaviour. Real-mode support lives behind `CpuMode::Real16`. A change is
**mergeable back to `x86jit` main** only when all hold:

1. It sits behind `CpuMode::Real16` / a new branch — generated code for
   `Long64` / `Compat32` is byte-identical (hash the codegen for a block corpus).
2. `CpuState` / `jit_abi` changes are **append-only** with an offset-assertion test.
3. `x86jit`'s full suite (interp / JIT / Unicorn / fuzz) is green, no PS4
   benchmark regression above threshold.
4. The seam merges **whole, with its tests** — never half a seam.

Pin the `x86jit` rev **per golden test**; golden traces are only valid against a
known CPU revision.

See `ls backlog/decisions/` — the worktree, real-mode extension, IRQ-injection
contract, virtual clock, and memory-write (SMC) decisions.

## Engineering hard rules (project-specific)

1. **Testability-first.** No device lands without isolation unit tests on a mock
   bus. No machine-level feature lands without a golden/integration test.
2. **Swappability-first.** New hardware goes behind the `Cpu` / `Chipset` /
   `Device` / `FrameSink` traits. The machine core never hardcodes a chipset,
   device, or graphics mode.
3. **Docs-first.** Every module and trait carries a doc-comment describing the
   modelled hardware and cross-referencing `backlog/docs/` (`§` style, like
   x86jit). Datasheet coverage (what is / isn't modelled) is tracked.
4. **Determinism.** No wall-clock in the execution path. Virtual time is measured
   in **retired instructions** (never in JIT blocks — block boundaries are a
   lifter implementation detail). Devices tick off the virtual clock.

## Where things live

| Need | Path | When to load |
|------|------|--------------|
| **Source of truth for roadmap, tasks, backlog** | Backlog.md — `backlog task list --plain` | Always. **Don't read roadmaps from markdown.** |
| Current repo shape, data flow, file map | [`backlog/docs/architecture.md`](backlog/docs/architecture.md) | When making structural changes or unfamiliar with module layout |
| Coding conventions, file naming, commit style, comment policy | [`backlog/docs/conventions.md`](backlog/docs/conventions.md) | Before writing or modifying code |
| Feature status (what works, what's in flight, what's broken) | [`backlog/docs/status.md`](backlog/docs/status.md) | When user asks "does X work?" or you're picking up work |
| Common dev commands (build, test, run, typecheck, lint) | [`backlog/docs/commands.md`](backlog/docs/commands.md) | When running build/test/dev loops |
| Tooling (devShell, direnv, pre-commit, static analysis) | [`backlog/docs/dev-setup.md`](backlog/docs/dev-setup.md) | When fixing tooling, adding hooks, or onboarding |
| Working on tasks (statuses, branch naming, session handoff) | [`backlog/docs/working-on-tasks.md`](backlog/docs/working-on-tasks.md) | Before picking up a task |
| Where to capture decisions (`backlog decision` vs task note) | [`backlog/docs/decisions.md`](backlog/docs/decisions.md) + `ls backlog/decisions/` | When making a non-trivial decision |
| Project glossary / domain terminology | [`backlog/docs/glossary.md`](backlog/docs/glossary.md) | When you hit an unfamiliar term |
| Things deliberately deferred — do NOT implement unprompted | [`backlog/docs/deferred.md`](backlog/docs/deferred.md) | Before adding features that "seem missing" |
| Backlog skill (`backlog` CLI + task/doc/decision workflow) | [`.agents/skills/backlog/SKILL.md`](.agents/skills/backlog/SKILL.md) | Auto-loaded by the backlog skill trigger; also when driving `backlog` manually |

> **Skills location.** Canonical: `.agents/skills/<name>/` (agent-agnostic, **vendored/committed**). `.claude/skills/` are symlinks created by `scripts/skills-bootstrap.sh` so Claude Code can auto-trigger them. Skills are not fetched from anywhere — to refresh the symlinks (e.g. after adding a skill), re-run `scripts/skills-bootstrap.sh`.

## Load-on-demand rule

Don't read every `backlog/docs/` file at session start. Pick the file matching the task — they are sized to be loaded individually. The table above tells you *when* to load *what*.

## Session handoff

When ending a session mid-task, record what's done, what's next, and any blocker on the active task:

```sh
backlog task edit <id> --notes "Session pause $(date -I). Done: <X>. Next: <Y>. Blocker: <Z|none>."
```

When starting a session, read the most-recently-touched in-progress task (`backlog task list -s "In Progress" --plain`, then `backlog task <id> --plain`) before doing anything else. Local, versioned in-repo, agent-agnostic.

Details: [`backlog/docs/working-on-tasks.md`](backlog/docs/working-on-tasks.md).

## Working on tasks

This repo tracks work **locally** with [Backlog.md](https://github.com/MrLesk/Backlog.md) — tasks are markdown under `backlog/`, no external tracker. Read [`.agents/skills/backlog/SKILL.md`](.agents/skills/backlog/SKILL.md) (and run `backlog instructions overview`) before driving `backlog`. It's **forge-agnostic**: git is plain branches merged into the default branch; a remote (GitHub/GitLab/Codeberg/Radicle/none) is an optional mirror, not an issue tracker.

## Quick dev loop

```sh
cargo build
cargo test                                              # unit + golden + integration
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt
```

Full list in [`backlog/docs/commands.md`](backlog/docs/commands.md).

## Hard rules (don't violate)

- **Never commit without explicit user request.** Even mid-task, after accepting a plan, stop and ask. Acceptance of plan ≠ acceptance of commit.
- **Don't add features, refactor, or introduce abstractions beyond what the task requires.** Bug fix = bug fix, not surrounding cleanup.
- **Don't pre-empt later milestones.** If a task is scoped to a later milestone (Backlog.md label/milestone), don't half-implement it during earlier work.
- **All project docs live under `backlog/docs/`.** That's the single knowledge tree (tasks, docs, and decisions all live under `backlog/`).

## Code ownership

Maintainer / sole deciding authority: **Mikołaj Mikołajczyk**
(<mikolajczyk.mikolajm@gmail.com>).
