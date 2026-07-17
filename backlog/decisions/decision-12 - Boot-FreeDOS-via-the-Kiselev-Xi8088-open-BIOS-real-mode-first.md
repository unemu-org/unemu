---
id: decision-12
title: Boot FreeDOS via the Kiselev Xi8088 open BIOS (real-mode first)
date: '2026-07-17 20:30'
status: proposed
---
## Context

The project needs a concrete north-star target. **Booting FreeDOS** to a command
prompt is a natural one: it exercises the whole machine (CPU real mode, PIC/PIT,
keyboard, video, disk, the BIOS interrupt layer) and has an unambiguous success
signal. DOS runs entirely in **real mode**, so it fits the current 286-class core
with no protected mode.

We will **not write our own BIOS** — that is a separate project. We need a
ready-made, openly-licensed BIOS. A survey found:

- **SeaBIOS** and **Bochs `BIOS-bochs-legacy`** — the ideal *device* fit (dual
  PIC, 8042, CMOS, FDC, IDE) but both **require a 386 and switch to protected
  mode during POST**. Disqualified until we add protected mode.
- **GLaBIOS**, **8086tiny BIOS**, **Super PC/Turbo XT** — real-mode-only but
  XT-class: 8255 PPI at 0x60–0x63 (collides with our 8042), no CMOS. 8086tiny also
  needs two non-standard emulator opcodes in the CPU core.
- **Sergey Kiselev `8088_bios`, Xi8088 configuration** — real-mode-only 8088 code
  whose POST drives an **AT-style 8042 keyboard controller and MC146818 CMOS/RTC**,
  i.e. the chips we already model. Single-PIC interrupt topology (our slave 8259
  idles). GPLv3-or-later; redistributable as a built, gitignored fixture.

FreeDOS ships an **8086 kernel build** (`ke2044_86f16`, FAT12/16) that boots in
pure real mode and reaches a prompt without A20/HMA.

## Decision

Target **FreeDOS (8086 kernel build)**, booted by the **Kiselev `8088_bios`
Xi8088 ROM**, on our AT-class real-mode machine. **386 / protected mode (and the
BIOSes that need it) are deferred**, not abandoned — when protected mode lands,
Bochs `BIOS-bochs-legacy` becomes the natural full-AT option.

Sequencing (revised after an adversarial plan review) is **corpus-first**: drive
the x86jit real-mode lift to near-100% of the SingleStepTests/80286 corpus
*before* booting the BIOS, so the BIOS run is validation, not one-abort-at-a-time
discovery. Then harden the machine (open-bus policy, POST/0x61/CGA ports, CMOS
seeding, ROM mapping), then boot the BIOS to INT 19h, then add a disk controller,
then FreeDOS. Tasks: task-13 (M4.5) → task-14…18 + task-12 (M5a) → task-19/20
(M5b) → task-21 (M6) → task-22 (M7).

Four facts about the Xi8088 BIOS are **verified in a spike (task-19) before this
record is accepted**: single- vs dual-PIC (and the port-0xA0 NMI-mask collision
with our slave PIC if single), ROM size + map base, whether hard-disk support is
built in or needs the XTIDE Universal BIOS option ROM, and its CGA INT 10h port
surface. The **disk controller choice** (XT-IDE PIO HDD image vs µPD765 floppy +
DMA) is deferred to that spike and recorded separately.

## Consequences

- Keeps the machine AT-ish; no pivot to XT / no 8255 PPI. Our 8042 + CMOS + master
  PIC are exercised by the BIOS; the slave PIC idles (fine for FreeDOS).
- The BIOS/DOS run is the driver for closing real-mode CPU lift gaps, but the
  **SST/80286 corpus is the gate**, not the BIOS aborts.
- Requires machine hardening the demos never needed: an **open-bus default**
  (undecoded ports read 0xFF, unmapped reads 0xFF, A20-off wrap) — without it a
  real BIOS aborts within its first hundred instructions.
- Timing loops that watch **hardware feedback** (port 0x61 refresh bit, 0x3DA
  retrace bit) must toggle off the **virtual clock**, or the BIOS spins forever.
- BIOS binaries and the FreeDOS image are **GPL**, used unmodified as gitignored,
  script-fetched test fixtures (no linking); provenance recorded per fixture.
- Supersedes the "real BIOS is deferred" stance in `deferred.md` (decision-10) for
  the real-mode case; protected-mode BIOSes stay deferred.
