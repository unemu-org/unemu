---
id: decision-42
title: CPU A20 is held open from a CPU-only reset until the first CS load
date: 2026-11-19
status: Proposed
deciders: Mikołaj Mikołajczyk
---

# decision-42 - CPU A20 is held open from a CPU-only reset until the first CS load

## Status

Proposed — 2026-11-19, `upc-f7j.20`. Implemented in `machine::Machine::with_reset_fetch_probe`
and attached by assembly to every machine with an A20 source. The board contract is in
`backlog/docs/design/machine-config.md` §A20 line and System Control Port A, "Reset fetch with
the gate off".

## Context

The 80286 starts after reset at CS F000h with hidden base FF0000h, so its first fetch is at
FFFFF0h, which the IBM 5170 decodes through its high ROM alias. The board's A20 gate is driven
by the 8042 output port, which a CPU-only reset (a shutdown or the 8042's reset pulse) does not
touch. Until this decision the CPU applied the gate to the reset fetch too: with the gate off the
fetch went to EFFFF0h, where the board decodes nothing, FFh bytes raised #UD, and with SS:SP
0000:0002 the exception frame had no room, so the processor shut down again before retiring
anything: "64 resets within 0 steps, the last shutdown at f000:fff0".

Note, 2026-11-20 (`upc-f7j.21`): that shutdown at SP 2 came from the CPU core of the time, which
required the whole real-mode frame to fit below SP. From x86jit `6cb817b8` on, a real-mode frame
on the 80286 wraps inside the segment and only SP 1, 3 or 5 shuts down (I286 PRM Appendix B, INT),
so at SS:SP 0000:0002 the #UD is delivered through the guest IVT; with the window off, a handler
that returns reloads CS with a real-mode base and reaches the low ROM copy, the reset fetch still
missing it. The open-BIOS test (`crates/pc-harness/tests/a20_off_reset_bios.rs`) therefore leaves
SP at 1: with the window disabled it stops again with "64 resets within 0 steps, the last shutdown
at f000:fff0", and with the window it resumes twice.

MEASURED by the maintainer: that stop with Windows 3.1 in Standard mode, at its DOS prompt, under
both the open AT BIOS and the user-supplied IBM 5170 BIOS. INFERRED: real hardware reaches the BIOS
at this point, because the same guest runs on AT-class hardware. Not ruled out: on hardware the gate
may be on at this reset, and an earlier divergence in the emulator leaves it off here. The
stop report shows the state at the reset, not how it was reached.

What the sources say:

- Intel 80286 Programmer's Reference Manual, Appendix C, Hardware Compatibility Considerations,
  "Address after Reset": A23–A20 are high for CS-relative bus cycles (code fetch or a CS
  override) after reset until the first far JMP or far CALL. "Physical Address Formation": an
  8086-compatible system forces A20 low externally in real mode.
- IBM Personal Computer AT Technical Reference, System Board, System Memory Map: system ROM at
  0F0000h–0FFFFFh, duplicated at FF0000h–FFFFFFh; 0E0000h–0EFFFFh reserved on the system board,
  duplicated at FE0000h–FEFFFFh; 100000h–FDFFFFh I/O channel memory up to 15 MB. Keyboard
  Controller, Output-Port Bit Definitions: bit 1 Gate A20, bit 0 System Reset.
- The system board's gate is a 74257 multiplexer that passes either the 80286's A20 or a low
  level to the rest of the board, selected by the 8042's P21; the system ROMs' chip select comes
  from the 28S42 PROM U72, which decodes A17–A23 (minuszerodegrees.net, "IBM 5170 – A20 Gate
  Circuitry" and "Generation of signal for CE pin on U27 and U47 ROMs", drawn from the Technical
  Reference's logic diagrams). IBM does not publish U72's contents.
- The IBM BIOS listing of 06/10/85 never resets with the gate off itself: the INT 15h AH=87h
  block move enables A20, resets, and disables A20 only in its shutdown-9 return (BIOS1,
  BLOCKMOVE and SHUT9).
- An integrated chipset documents the failure: AMD's ÉlanSC300/SC310 "GATEA20 Function
  Clarification" application note (publication 21811A): with the gate held low the boot
  address becomes EFFFF0h and ROMCS is not asserted, so the gate must be set before a CPU reset.

The documents are not conclusive for the 5170. If U72 sees the gated A20 and decodes ROM only
where the memory map says, a genuine 5170 would fail as the old model did; nothing in them
forces the gate on during reset.

## Decision

From a CPU-only reset, while CS still holds the 80286 reset image (selector F000h, hidden base
FF0000h), the machine holds the CPU's own A20 open, whatever the board line says. The first CS
load closes the window: the reset vector's far jump (so, for the usual far-JMP reset vector, the
window is that one instruction), or any interrupt or exception delivery. The board line then
applies again. The 8042 output port, port 92h, the board line (`devices::A20Line`) and the memory
map are untouched, so the guest reads back through 8042 command D0h the gate it wrote, and 1 MiB
wraps again right after the far jump. This is the window the 80286 manual gives for its high
CS-relative cycles; it is the model that lets such a reset reach the BIOS while changing nothing
else a guest can observe. The machine stops after each step while the window is open, which
moves no device or IRQ boundary. A diagnostic report taken inside the window says so ("CPU A20
held open by the reset window").

## Alternatives considered

- **Keep the gate on the reset fetch (the old model).** Matches the documents read literally, but
  contradicts the maintainer's evidence that the guest runs on hardware, and stops the machine.
- **A static ROM alias at EE0000h–EFFFFFh.** Contradicts the 15 MB I/O channel memory map and
  hides extended RAM configured there (the 5170 profile allows up to FDFFFFh).
- **An alias at EE0000h–EFFFFFh only while the gate is off.** Needs a remap of RAM on every gate
  change, and CPU accesses to EExxxx with the gate off would wrongly hit ROM.
- **Force the 8042 gate on at reset.** Changes the output port the guest reads back.

## Consequences

- A CPU-only reset with the gate off reaches the reset vector, through a shutdown and through the
  8042 pulse, under any BIOS.
- Data cycles inside the window are not gated either; reset code addresses data through bases
  below 1 MiB, so only a segment wrap at FFFF:xxxx among its first instructions could notice.
- A gate change the reset code makes before its first CS load takes effect at that load.
- The CPU stop report (`cpu::CpuStopReport`, built by the CPU backend alone) shows "a20 on"
  inside the window without the note; only the machine diagnostic report carries it.

## Trigger to revisit

A measurement on a genuine IBM 5170 (a CPU reset with the gate off, through a shutdown and through
command FEh), the contents of the U72 PROM, or evidence that the guest's gate is on at this reset
on hardware.
