---
id: decision-23
title: EMS on the 286 AT through a Lo-tech 2MB EMS board model
date: 2026-11-13
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-23 - EMS on the 286 AT through a Lo-tech 2MB EMS board model

## Status

Accepted — 2026-11-13. Records the outcome of the EMS spike. The maintainer
confirmed the driver terms the same day: the project emulates the board only;
LTEMM is the user's own driver, installed by whoever wants it, and the fetch
script is a convenience for local tests.

## Context

On a 286, expanded memory (LIM EMS 3.2/4.0) comes from an expanded-memory
board plus its driver. EMM386-style software EMS needs a 386. The spike
asked which board to model so that a driver usable without user-supplied
proprietary software exists.

Findings:

- **Programs on the maintainer's list.** None of them needs EMS on a 286
  with 640 KiB.
  - Commander Keen 4–6 maps up to 64 KiB of EMS as extra heap when present
    (reconstructed source, `ID_MM.C`).
  - Wolfenstein 3D caches pages in EMS and XMS when present (id Software
    source release, `ID_PM.C`).
  - Prince of Persia does not use EMS.
  - Windows 3.0 real mode uses LIM 4.0 EMS and gains most from a large page
    frame below 640 KiB. A board with a 64 KiB frame in upper memory gives
    it little.
  - Windows 3.1 (standard mode on a 286) and the Phar Lap 286 extender use
    extended memory, not EMS.
  - Whether the Monkey Island games cache in EMS is not established.
- **Lo-tech 2MB EMS board.** An open-hardware 8-bit ISA board with a
  published schematic (revision r02, discrete 74HCT logic). Its interface:
  - Four write-only 8-bit page registers at base+0..3. The base is set by
    jumper: 260h (default), 264h, 268h or 26Ch. Address lines A10–A15 are
    not decoded, so the ports alias every 400h. Nothing drives the data bus
    on reads, so the registers cannot be read back.
  - Register n selects the 16 KiB page shown at frame + n×4000h. The 64 KiB
    frame is set by jumper: C000h, D000h or E000h (default E000h).
  - Bits 0–4 select the page within a 512 KiB SRAM chip. Bits 5–7 select
    the chip; only chips 0–3 exist. So values 0–127 map RAM, and values
    128–255 select no chip and leave the window unbacked.
  - The latches have no reset input. Their contents are undefined at power
    on and survive a warm reboot.
- **LTEMM driver.** Lo-tech's LTEMM r01 (March 2014) implements LIM 4.0
  except functions 19 and 28, with a `/3` switch for EMS 3.2 only. It is
  8086 code with source included. Its README states that it is based on a
  1988 driver by Alex Tsourikov whose source was provided under the BSD
  3-Clause licence. The modified driver is provided subject to Lo-tech's
  general terms, which allow non-commercial hobbyist use and grant no
  redistribution right.
  - Pinned archive: `LTEMM-r01.zip`, SHA-256
    `0bcbcac26dd21da6125da79d74107905989c092f1639416ba90219001602383a`.
  - `LTEMM.EXE` SHA-256
    `457c9a849f87749a067d4ef23477106de4abd575eb4d5cd0ff8755aeaa80956d`.
  - An archived copy exists on the Internet Archive.
- **No clean open alternative for a common board.** FreeDOS ships no
  hardware-EMS board driver. Intel Above Board and AST RAMpage drivers are
  proprietary. The MIT-licensed MS-DOS 4.0 `XMA2EMS.SYS` targets IBM XMA
  cards, whose interface is large and mostly PS/2-specific.
- **Other emulators differ from the schematic.**
  - MartyPC masks the page value to 7 bits, so the unmap value FFh shows
    page 127.
  - 86Box returns readback values the board does not have.

## Decision

1. **Model the Lo-tech 2MB EMS board** as an ISA device in the machine
   file. It has a configurable port base, a configurable frame and 512 KiB
   to 2 MiB of populated RAM. Behaviour follows the r02 schematic, not
   other emulators:
   - The registers are write-only; port reads float.
   - Values 128–255 unmap the window; reads there float.
   - Ports alias every 400h within the decoded range.
   - The power-on register value is fixed for determinism, and the
     contents survive a CPU reset.
2. **LTEMM is a fetched fixture, never committed.** A fetch script
   downloads `LTEMM-r01.zip` from Lo-tech's pinned URL, with the archived
   copy as fallback. It checks both SHA-256 values and extracts
   `LTEMM.EXE` into a gitignored path. LTEMM is never placed inside a
   published or committed disk image.
3. **Priority stays behind the game-ready AT work.** No game on the list
   requires EMS; Keen and Wolfenstein 3D only use it when present.

## Consequences

- The FreeDOS kernel loads `DEVICE=LTEMM.EXE` as an EXE device driver.
  FreeDOS `MEM` (already on the tools floppy) shows the expanded memory, so
  an open golden can check the board without proprietary software.
- LTEMM's memory test toggles port 61h and polls the keyboard. `/n` skips
  the test where a golden needs a short boot.
- Asking Lo-tech for permission to redistribute LTEMM would remove the
  remaining doubt about the terms. Contacting them is the maintainer's
  step. If the terms ever block use, the fallback is a clean-room driver
  for the same board.
- A later 386 profile can still offer software EMS (an EMM386-class driver)
  next to this board; the two do not conflict.
