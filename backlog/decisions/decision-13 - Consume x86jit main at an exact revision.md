---
id: decision-13
title: Consume x86jit main at an exact revision; CPU work lands through x86jit's tracker
date: 2026-11-09
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-13 - Consume x86jit main at an exact revision

## Status

Accepted — 2026-11-09. Supersedes the mechanics of [[decision-2]] (the dedicated
worktree branch); the merge gates it listed now live in x86jit's own rules.

## Context

[[decision-2]] developed the real-mode CPU work on a dedicated x86jit branch,
`pc-emulator`, and pinned this repository to that branch through a git URL on one
development machine. Since then:

- the real-mode seam merged into x86jit `main` (`CpuMode::Real16`, interpreter-only);
- `pc-emulator` was deleted as an already-merged pointer — it had drifted and was
  hiding x86jit's own tasks;
- x86jit became a project with its own tracker (beads, prefix `xj`), Definition of
  Done and publication rule;
- the pin still named the dead branch at a host-local path, so a fresh checkout did
  not build, and x86jit's `Exit` API had moved on.

## Decision

- `x86jit-core` is a git dependency pinned to an **exact `rev` of x86jit `main`**,
  never to a branch. The URL is x86jit's origin on the maintainer's forge until
  x86jit's public republish; then it becomes the public URL.
- The forge requires authentication. Cargo fetches through the git CLI
  (`CARGO_NET_GIT_FETCH_WITH_CLI=true`) with credentials, or with a one-command
  `insteadOf` rewrite to the SSH remote (`dev-setup.md` § CPU core).
- Local development against an x86jit checkout uses a **gitignored**
  `.cargo/config.toml` `[patch]`; a bare `path` dependency in `Cargo.toml` stays
  banned — it breaks `nix build` / CI.
- A change the machine needs from the CPU is **filed as a bead in x86jit**, naming
  this project as the consumer, and lands under x86jit's gates (gated behind its CPU
  mode, `CpuState` / `jit_abi` append-only, the other modes' generated code
  unchanged, its full suite green, the seam merged whole with its tests).
- Moving the pin is **its own bead here**: the Definition of Done plus the slow
  goldens. A golden's doc comment names the rev it was recorded against; a re-pin
  re-runs it and re-baselines only when the trace really changes.

## Alternatives considered

- **Keep a long-lived consumer branch in x86jit** — it drifted, broke this build
  when deleted, and confused x86jit's tracker. Rejected.
- **Follow x86jit `main` without a rev** — goldens are only valid against a known
  CPU revision. Rejected.
- **Vendor or fork x86jit** — rejected for the reasons in [[decision-2]].

## Consequences

- (+) Any checkout with forge access builds; nothing depends on one machine's paths.
- (+) No branch to keep in sync; CPU work follows one workflow, in one tracker.
- (−) Fetching needs forge credentials (or the SSH rewrite) until the public republish.
- (−) x86jit's republish discards its history: a pin into today's history then
  resolves only from the archived ref, and this project re-pins to the republished
  commit with the same tree. x86jit's consumer list has to name this project.

## Trigger to revisit

x86jit's public republish (switch the URL), or a need to carry CPU work that is not
yet on x86jit `main` — then pin a short-lived feature-branch rev for the duration of
one bead, never a long-lived branch.
