---
id: TASK-25
title: M7 — Diagnose and fix the FreeDOS early-boot spin
status: To Do
assignee: []
created_date: '2026-07-18 07:48'
labels:
  - dos
  - devices
  - cpu
dependencies:
  - TASK-22
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
FreeDOS boots via the BIOS (INT 19h loads + jumps to the FreeDOS boot sector) but spins early in its boot chain before printing a kernel banner or reaching the A: prompt — StopReason::BudgetExhausted (a loop, not an unlifted instruction). Diagnose: instrument run_freedos to capture CS:IP at the retired cap, disassemble the spin location, and map it to the FreeDOS boot sector / 8086 kernel source. Likely candidates: the boot sector's kernel-load loop hitting a uPD765 multi-sector/multi-track READ pattern we don't handle; an interrupt-delivery issue (kernel waits on a BIOS timer tick / IRQ0 during boot); or a BIOS INT service the kernel calls behaving wrong. Fix the identified cause; iterate (the spin may hide the next gap). Goal: FreeDOS kernel banner, then the A: prompt.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The spin location is identified (CS:IP + disassembly + FreeDOS source mapping)
- [ ] #2 Root cause fixed (FDC read pattern / interrupt delivery / BIOS service — whichever it is)
- [ ] #3 FreeDOS prints its kernel banner; ideally reaches the A: command prompt (golden)
<!-- AC:END -->
