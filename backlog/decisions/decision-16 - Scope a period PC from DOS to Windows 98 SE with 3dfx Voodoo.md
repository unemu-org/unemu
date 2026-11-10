---
id: decision-16
title: Scope — a period PC from DOS to Windows 98 SE with 3dfx Voodoo
date: 2026-11-09
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-16 - Scope: a period PC from DOS to Windows 98 SE with 3dfx Voodoo

## Status

Accepted — 2026-11-09. Extends [[decision-1]]: the IBM PC/AT (80286) stays the
first machine profile, but it is the starting skeleton, not the end target.
The open work for every phase below lives in beads, one epic per phase
(`bd list --type epic`, `bd list --type milestone`); this record holds the goal
and the reasons.

## Context

The machine boots an unmodified open BIOS and FreeDOS on a deterministic 286
model. Without a stated end target, every later choice (CPU features, buses,
video, audio, timing model) is argued from scratch, and the project drifts.
The maintainer's goal is a machine that runs the PC games of one era, with the
same priorities as today: testability, swappability, dense documentation and
determinism.

## Decision

1. **Target.** Run PC software from late DOS (about 1994) to **Windows 98 SE
   with a 3dfx Voodoo card** (about 1999). The goal line is Windows 98 SE on a
   Pentium II/III-class machine running a Glide or early Direct3D game through
   the emulated Voodoo, shown in a window.
2. **Non-goal.** Windows XP, Direct3D 9-class graphics, Pentium 4, SSE2 and
   long-mode guests are out of scope. If that ever happens, it is a new effort
   with its own decision, built on a finished Windows 98 SE machine.
3. **Functional timing, never cycle-exact.** Virtual time stays retired steps
   plus halted clocks ([[decision-5]], [[decision-14]], [[decision-15]]).
   Instruction cycle costs and bus timing are not modelled. Software that needs
   cycle-exact 8088/CGA timing (demoscene effects, CGA snow, PWM speaker audio)
   is out of scope: it needs a different clock contract and an interpreter-only
   CPU core.
4. **One emulator, several machine profiles.** Each hardware generation is a
   `MachineConfig` profile in the `assembly` crate — 286 AT, 386 protected
   mode, Pentium PCI, Pentium II with Voodoo — never a fork. New hardware goes
   behind the existing `Cpu` / `Device` / `InterruptController` / `FrameSink`
   traits ([[decision-7]]). The 286 AT profile stays as the smallest machine
   and test bed.
5. **Graphics are emulated at the chip, not at the API.** The Voodoo is a
   register-level `Device`; the guest's own driver programs it, so Glide,
   Direct3D 6/7 and miniGL work through the one chip with no per-API host
   bridge. This keeps the low-level, deterministic model.
6. **CPU features belong to the CPU core.** 386 protected mode, paging,
   protected-mode interrupts and Pentium-class instructions are filed and
   landed in x86jit under its own workflow, then re-pinned here
   ([[decision-13]]). This repository files the request and the machine-side
   bead.
7. **Phases, planned with decreasing detail.** Each phase is an epic; its exit
   is a milestone bead. A phase is broken into fine tasks only when the phase
   before it is close to done; later phases stay coarse on purpose. **No phase
   starts before the 286 foundation is finished**: a foundation milestone bead
   (corrected interrupt timing as the only loop, bounded opt-in traces, the
   remaining review defects, reproducible fixtures, backend tests) gates the
   first bead of phases A and B.

   | Phase | Epic key | Scope | Exit milestone |
   |-------|----------|-------|----------------|
   | A | `skeleton` | close the 286 machine: the display flows through a real card to a renderer and a window; host keyboard input | FreeDOS runs in a window from the host keyboard; every headless golden unchanged |
   | B | `protected-mode` | 386 protected mode, paging, protected-mode interrupts, the A20 gate, a 386 machine profile | a DOS-extended 32-bit program runs end to end |
   | C | `win9x-base` | PCI, VBE linear framebuffer, Sound Blaster 16, Pentium CPU features, IDE disk, mouse, a Pentium-class profile | Windows 98 SE boots to the 2D desktop |
   | D | `voodoo` | Pentium II/III top-up, the 3dfx Voodoo card, rasteriser performance | Windows 98 SE runs a Voodoo game in the window — **the target** |

   *Amended 2026-11-10:* a consolidation phase **A′** (epic key
   `consolidation`) sits between A and B. Phase B is mostly CPU-core work in
   x86jit, so the machine side uses that time to make machines description
   files (one versioned schema, TOML and YAML syntaxes), to make firmware and
   machine class (XT-class or AT-class BIOS plus its device set) a validated
   configuration choice, and to persist floppy writes incrementally with
   rollback; a hard disk follows once the firmware question is settled. Its
   exit milestone: the window and the CLI run machines from description files,
   firmware is configurable, floppy writes persist recoverably, every headless
   golden unchanged. The phase B gap analysis runs in parallel so the CPU-core
   requests reach x86jit early.

8. **Choices settled with this decision.**
   - The windowed presenter is its own crate (winit + softbuffer). Headless
     capture stays the default; the test and CI paths never need a window.
   - The window's paint and input loop is the only code allowed to read a host
     clock. It only reads frames, and it injects host input at run
     boundaries; it never ticks devices.
   - A bus abstraction with slots and variants waits for the PCI work in
     phase C; there is no second bus to shape it before then.
   - The Voodoo variant (Voodoo2 as a 3D-only card beside a 2D card, or
     Banshee / Voodoo3 with 2D and 3D on one card) is chosen when phase D is
     planned.

## Alternatives considered

- **Stay a 286 research machine.** Keeps the project small, but gives no
  direction for buses, video, audio or CPU work. Rejected: the maintainer's
  goal is a games machine.
- **Target the Windows XP / Direct3D 9 era.** Needs Pentium 4, SSE2 and a
  programmable-shader GPU; the scope would become open-ended. Rejected.
- **High-level graphics (Glide or Direct3D calls passed to the host GPU).**
  Faster to show results, but it breaks the low-level model, needs one bridge
  per API and makes output depend on the host GPU. Rejected.
- **Cycle-exact timing.** No calibrated oracle exists across this span, and it
  is too slow for Pentium-class guests. Rejected.
- **A separate emulator per generation.** Duplicates the device models and the
  test infrastructure. Rejected in favour of machine profiles.

## Consequences

- (+) Every later choice can be checked against one goal line and one non-goal.
- (+) The span forces the swap seams to work for real: CPU, bus, video and
  audio are all replaced at least once.
- (−) Large scope. The CPU core needs new seams of the same weight as the
  real-mode work: legacy protected mode with descriptor segmentation, 2-level
  paging and protected-mode interrupts.
- (−) Host image persistence and snapshot/restore become required in phase C
  (an installed Windows 98 SE lives on a disk image).
- The main technical risk is performance: software rasterisation of the Voodoo
  at playable frame rates. Open-source emulators of this era already do it, so
  the risk is bounded engineering, not feasibility.

## Trigger to revisit

The phase D milestone is reached; or a phase proves infeasible (for example the
CPU core cannot host legacy protected mode, or the rasteriser cannot reach
playable rates after its performance pass); or the maintainer moves the goal
line.
