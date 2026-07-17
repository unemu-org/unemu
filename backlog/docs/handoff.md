# Session handoff

Living snapshot of where the project is, how we work, and what's next. Read this
plus `backlog task list --plain` when picking up. Keep it current.

## What this is

`unemupc98` — a low-level emulator of an IBM PC/AT (Intel 80286) built on the
`x86jit` CPU core. Research project: testability, swappability, dense docs. See
`AGENTS.md`, `architecture.md`, `design/spec.md`, `ls backlog/decisions/`.

## Two repos, worktree model

- **`~/src/unemupc98`** (this repo) — the emulator. Default branch `main`.
- **`~/src/x86jit`** — the CPU core, a **separate repo**. Its `main` is the
  PS4-emulator production branch. Real-mode work happens on branch **`pc-emulator`**
  (dev worktree `~/src/x86jit-pc`). **Current: `x86jit main == pc-emulator`** — the
  full real-mode seam plus `RegionKind::Rom` are on `main` (3ff42a5), in sync, no drift.
- The emulator depends on `x86jit-core` as a **git dep** (`branch = pc-emulator`)
  with `Cargo.lock` pinning the rev — never a bare path dep (breaks CI/nix).

## How we work — the multi-agent loop

1. **Decompose** the next work into independent streams (max 3 agents at once;
   each agent may spawn ≤2 of its own). Pick splits that minimise file overlap.
2. For each stream, **create a git worktree + branch** off the target branch
   (emulator `main`, or x86jit `pc-emulator`). Devices/oracles are independent →
   parallelise; tight vertical slices (one workspace) go to a single agent.
3. **Brief each agent** with: where it works (its worktree, absolute paths), what
   to read (`AGENTS.md`, conventions, the pattern file), the exact deliverable,
   hard rules, gates, and "commit + report raw facts". Agents run in background.
4. **I (the orchestrator) review every result — never merge on the agent's word.**
   Independently: grep for comment jargon + SPDX, read the diff of ABI-critical /
   hot-path / seam files, run the gates myself (build/test/clippy/fmt), and for
   x86jit confirm no Long64/Compat32 regression (jit suite, jit_abi offsets
   unchanged, changes `CpuMode`/`Code16`-gated, new IR ops `unreachable!` in JIT).
5. **Merge** the branch into the target (`--no-ff`), resolving trivial conflicts
   (usually `devices/lib.rs` mod-line unions). Then remove the worktree + branch.
6. **Update backlog** (task status/AC, `status.md`) and commit docs.
7. For x86jit real-mode work: merge feature branch → `pc-emulator`, review, then
   fast-forward `main` to `pc-emulator` so they stay in sync (no drift).

### Hard rules (project + learned)
- **Commits are part of this workflow** (merge-based); the maintainer authorised
  it. Still: no pushes to any remote; only local branch advances. GPG-signed.
- **Comments are for humans** — no `M#`/`task-#`/`decision-#` jargon in source
  (see `conventions.md`). x86jit is a *different repo* with its own conventions
  (it uses `§`/task refs and NO SPDX headers — match its surrounding style).
- **Never `git add -A` in the x86jit worktree** — the maintainer has untracked
  work there (task-241 Doom regression, fuzz artifacts). Use `git add -u` / explicit
  paths; leave their untracked files alone.
- **Validate CPU against the target's silicon corpus**: SingleStepTests/**80286**
  (`x86jit-tests/tests/ss286.rs`, fetched via `vendor/80286/fetch.sh`, gitignored).
  The 8088 corpus was retired — near-compatible corpora emit false failures.
- **Determinism**: no wall-clock; virtual time = retired-instruction counter.
- Two pre-existing x86jit test failures are host-specific fuzz/AVX artefacts
  (`corpus_replays_on_jit`, `vex_alias…`, `native_matches_interp`,
  `every_vector…`) — they flake by fuzz-artifact state and fail on baseline too;
  confirm any "new" failure against a clean baseline before blaming our change.
- Have design plans adversarially reviewed by a Fable subagent before executing.

## Status — done

- **M0** bootstrap/docs/ADRs. **M1** walking skeleton (serial "hello" golden).
- **M2 real mode** — on x86jit `main`: 16-bit decode + segmented addressing,
  INT/IRET/IVT/IF, IRQ injection + retired-instruction counter, full instruction
  set (mov/push/pop Sreg, far JMP/CALL/RETF, LOOP/JCXZ, BCD, CLC/STC/CMC,
  SALC/XLAT), and in-guest fault delivery (#UD/#BR/#GP). All `CpuMode::Real16`-gated;
  `jit_abi` unchanged; Long64/Compat32 byte-identical. **80286 corpus: 96.0%
  decided / 92.9% exception delivery.**
- **M3** timer-interrupt boot — real-mode guest programs the PIT, takes IRQ0 via
  the IVT, prints to serial (deterministic golden). `AtChipset` = UART + 8259 PIC +
  8253 PIT; PIT OUT → edge-detect → PIC → `inject_irq`; clock from retired count.
- **M4 devices (device level)** — 8042 KBC, MC146818 CMOS/RTC, 8237 DMA (via a
  `note_write`-safe `DmaMemory` handle), and VGA text behind the new `FrameSink`
  seam (`video` crate). All isolated + unit/golden tested.
- **M4 integration** (task-5, done) — the M4 devices are wired into the AT machine:
  keyboard scancodes → `I8042` → **IRQ1** → master PIC → guest handler; guest stores
  into the `0xB8000` text window trap as MMIO → `render_text_mode` → `FrameSink`;
  and the **8259 slave/cascade** carries **IRQ8–IRQ15**, with the RTC's update-ended
  interrupt reaching the CPU across the slave→master-IRQ2 cascade. `CascadedPic` is
  the 16-line controller; `AtChipset` = `[UART, master PIC, PIT, 8042, VGA, slave PIC, RTC]`.
  Added `Cpu::complete_mmio_write` (a trapped store is acknowledged so the guest
  advances). Keyboard / VGA-text / RTC-cascade goldens. **Workspace: 163 tests.**
- **`RegionKind::Rom` on x86jit `main`** — read-only executable memory (masked
  BIOS/option ROM): reads/fetches inline at RAM speed, stores dropped on the
  interpreter path. Byte-identical Long64/Compat32 codegen (jit 203/0, smc 15/0).
  **Not yet consumed by the emulator** — that wiring is the next ROM step.
- **Oracles**: 80286 corpus wired as the sole real-mode oracle; Unicorn MODE_16
  (`cf16`, 40 tests).

## Status — to do (next)

1. **Consume `RegionKind::Rom` in the emulator** (decision-9): bump the `x86jit`
   pin to a rev with the ROM seam, add a ROM region to the machine/CPU for a BIOS
   at `F0000h` (read-only, executable), and a loader. The x86jit seam already exists.
2. **test386.asm** (task-7) — bootable full-machine CPU validation ROM, once ROM
   loading + more boot infrastructure exist (depends on 1).
3. **Residual 286 fidelity** (optional): ~341 #6 opcodes iced decodes but 286
   rejects; string-op #GP; `leave`/`ins`/`outs`. Full A20 gate (decision-8).
   The RTC **periodic** interrupt (register A rate bits) is stored but not
   generated — only update-ended/alarm drive IRQ8 today.
4. **Real BIOS/DOS** (later): needs the deferred items in `deferred.md`.

## Key file map (emulator)

`crates/`: `cli` (frontend + goldens), `machine` (composition root + run loop),
`cpu` (trait + `X86JitCpu` backend), `bus`, `chipset` (`AtChipset`, `SharedPic`),
`device` (traits: `Device`, `InterruptController`), `devices/*` (uart16550,
pic8259, pit8253, i8042, cmos_rtc, dma8237, vga_text), `video` (`FrameSink` +
text renderer), `testkit` (mock bus/cpu, golden helpers).
