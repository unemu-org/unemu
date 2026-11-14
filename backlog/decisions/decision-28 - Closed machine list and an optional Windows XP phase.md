---
id: decision-28
title: Closed machine list and an optional Windows XP phase
date: 2026-11-14
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-28 - Closed machine list and an optional Windows XP phase

## Status

Accepted — 2026-11-14. Amends [[decision-16]] (scope): it closes the list of
machine classes this emulator will model and turns the Windows XP non-goal
into an optional phase after the target.

## Context

decision-16 set the goal line (Windows 98 SE with a 3dfx Voodoo) and the
phases A–D, but not which machines the emulator covers on the way. Ideas kept
arriving: the NEC V30, 8-bit computers, Windows XP. The maintainer asked to
close the list so the scope is final.

The unemu family is split by role. CPU cores are libraries in their own
projects: x86jit for x86, x80 for the 8080/Z80 family, later possibly a 6502
core. Machine emulators use them: unemupc98 is the PC emulator on x86jit.
Machines from other families belong to other machine emulators, for example
one cycle-exact 8-bit machine emulator on x80 and a 6502 core, under the
shared front end. They are never added here.

## Decision

1. **In scope: IBM PC-compatible x86 machines, as machine profiles.** Each
   class is a profile in this emulator, never a fork:

   | # | Class | CPU | Examples | Phase |
   |---|-------|-----|----------|-------|
   | 1 | PC/XT, 8-bit ISA | 8088, NEC V20 | IBM 5150, 5160, Turbo XT clones (Xi8088) | XT work in A′ and later |
   | 2 | 16-bit XT | 8086, NEC V30 | a generic 8086 XT clone | after class 1 |
   | 3 | Tandy 1000 (SX/TX) | 8088/8086 | Tandy 1000 graphics and SN76489 sound | last of the 8-bit-bus profiles |
   | 4 | AT 286 | 80286 | IBM 5170, 5162 (XT/286), AMI/Award clones | A, A′ (exists) |
   | 5 | 386/486 ISA | 386SX/DX, 486 | generic ISA boards | B; a Windows 3.x milestone |
   | 6 | Pentium PCI | Pentium, MMX | i430-class chipset | C |
   | 7 | Pentium II/III with Voodoo | P2/P3 | i440BX-class chipset | D, the target |

   The V20 and V30 share one instruction-level model in x86jit (80186 real
   mode, NEC extensions, the 8080 emulation mode); the 8080 mode may come
   from x80 as a pinned dependency. Instruction-level timing stays functional
   (decision-16 point 3).

2. **Optional phase E: the Windows XP era, in this emulator.** After phase D,
   and only with its own decision, a Pentium 4 / Athlon XP-class profile (SSE,
   SSE2, APIC, ACPI, AGP) runs Windows XP. Graphics start with cards that have
   Windows XP drivers and a register model this project can afford (for
   example the Voodoo line and VBE 2D), which covers DirectX 7 and Glide games.
   A DirectX 9 shader GPU at register level is a separate decision because its
   size matches the rest of the project. Until phase D is done, phase E stays
   a non-goal.

3. **Out of scope, closed:**
   - x86 machines that are not IBM PC compatible: NEC PC-98 (PC-9801), FM
     Towns, Apricot, DEC Rainbow, Sirius;
   - the IBM PCjr and Micro Channel PS/2 models;
   - brand-specific variants such as the Amstrad PC1512/1640 video, laptop
     power management, and SMP (dual-processor) boards;
   - 64-bit (long mode) guests;
   - cycle-exact 8088 timing (demoscene effects, CGA snow), which remains a
     separate project (decision-16 amendment of 2026-11-10);
   - every non-x86 machine: those belong to other unemu machine emulators.

## Consequences

- A new machine idea is checked against this table. Anything outside it needs
  a new decision.
- Device models picked for classes 1–3 (CGA, Hercules, MDA, Tandy video,
  SN76489, AdLib) are written so a later 8-bit machine emulator could share a
  chip model through a family crate, without depending on this repository.
- decision-16's non-goal for Windows XP now reads: not before phase D, and
  only through phase E.
