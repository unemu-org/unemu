---
id: decision-18
title: Board models and a firmware catalogue define machine classes
date: 2026-11-10
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-18 - Board models and a firmware catalogue define machine classes

## Status

Accepted — 2026-11-10. Part of phase A′ of [[decision-16]]; the file syntax is
[[decision-17]]. The survey, board catalogue, wiring tables and validation
rules are in
[`backlog/docs/design/firmware-and-machine-classes.md`](../docs/design/firmware-and-machine-classes.md).
Work lives in beads under the `consolidation` epic and, for phase B, the
`protected-mode` epic.

## Context

Real PC firmware is built for a motherboard and its chipset: it programs
chipset registers and expects chipset-integrated peripherals. A machine
description therefore cannot pair any BIOS with any device set. Users of
period hardware want XT-class and AT-class machines, and later chipset boards;
the goal line ([[decision-16]]) needs a hard disk soon, a 386 firmware for
phase B and a PCI board for phases C and D.

## Decision

1. **A machine names a board model.** A board model is data: its motherboard
   devices and wiring, any chipset register model, and the firmware it is
   known to run. The class (xt, at, at386) follows from the board. The first
   board is `ibm-5170` (generic AT). It is completed early with the parts it
   still lacks (the second 8237, the 80287 ports, the reset line) so the name
   means what it says. Later boards: `ibm-5160` (generic XT),
   `generic-386-isa` (phase B), `i440fx-piix3` (phases C and D).
2. **A firmware catalogue with a SHA-256 allowlist.** Every firmware or option
   ROM a board may run is a catalogue entry (hash, size, placement, the
   services it `provides` such as `fixed-disk`, its CPU and board
   requirements, how it is distributed). Images are checked at load; an
   unlisted image is an error that lists the known entries. A developer may
   pass the run flag `--allow-unlisted-firmware`, which warns and is never
   part of a machine file or a golden.
3. **A new firmware is a new profile.** A profile's firmware never changes
   under existing goldens; new firmware arrives as a new machine next to the
   old one with its own golden.
4. **Hard disk on the AT machine:** the Kiselev BIOS stays; the XTIDE
   Universal BIOS AT build is added as an option ROM at C8000h with a new ATA
   device at 1F0h–1F7h, 3F6h, IRQ 14. The option ROM is fetched from a pinned
   release, padded to 8 KiB and checksummed by the fetch script, like other
   fixtures. It needs string port I/O in the CPU core (requested from x86jit).
5. **Phase B firmware:** SeaBIOS built for an ISA-only machine on
   `generic-386-isa`, with its serial console first; a register-level VGA
   comes later. Bochs rombios is the fallback. *Amended by [[decision-19]]
   (2026-11-11):* an unmodified SeaBIOS needs an emulator-only fw_cfg port
   for that serial console, writable shadow RAM for its POST and a pristine
   high BIOS copy for its reboot.
6. **XT firmware:** GLaBIOS built for the 5160 is the default, the Kiselev
   XT build a second entry; the XT board waits for the 8088 CPU model in
   x86jit.
7. **Phases C and D:** i440FX + PIIX3 with SeaBIOS is the open path; boards
   that need a proprietary BIOS (e.g. 440BX) are optional extras whose ROMs
   users supply and the catalogue lists by hash only. Proprietary images are
   never shipped.
8. **CMOS NVRAM** can be loaded from and saved to a 64-byte image file; it is
   part of schema 1.

## Alternatives considered

- **A `class` field with free firmware choice.** Cannot express chipset
  registers or reject a BIOS that will not run; rejected.
- **Switching the default BIOS to one with fixed-disk support.** Every golden
  would move and no open 286 AT BIOS with the needed services was found;
  rejected for an option ROM on a new profile.
- **Proprietary board BIOSes as the main path.** Not redistributable; kept as
  user-supplied extras.

## Consequences

- (+) Machines are honest about what they model; incompatible combinations
  fail at validation with a clear message, not as a silent guest failure.
- (+) The open path reaches the goal line without proprietary firmware.
- (−) Each board is work: wiring tables, a catalogue entry per firmware build,
  and a golden per profile.
- (−) The hard disk and phase B depend on CPU-core work in x86jit.

## Trigger to revisit

SeaBIOS cannot be made to run phase B or the PCI board; a board needs
behaviour the board-model data cannot express; or the maintainer wants a
proprietary board on the main path.
