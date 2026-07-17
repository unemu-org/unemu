# 8088_bios — Xi8088 configuration (boot firmware)

`bios-xi8088.bin` is built by [`fetch.sh`](fetch.sh) from
[skiselev/8088_bios](https://github.com/skiselev/8088_bios), pinned at commit
`a4af458a72bc33c4be066b33bd76e8af2b24111c` (BIOS version 1.0.2). It is an
open-source PC-class BIOS; we build the **Xi8088** machine configuration
(`MACHINE_XI8088`) and use it as the boot firmware for the AT-style real-mode
machine.

## What the Xi8088 configuration is

An AT-class real-mode config (`src/config.inc`): two cascaded 8259 PICs
(`SECOND_PIC`), 8042 keyboard controller (`AT_KEYBOARD`), MC146818 RTC/CMOS
(`AT_RTC` + `AT_RTC_NVRAM`), NMI mask on port 70h (`AT_NMI`), delay calibration
off the port-61h refresh bit (`AT_DELAY`), PS/2 aux/mouse (`PS2_MOUSE`), and a
model byte of `0FCh` (pretends to be an IBM PC/AT).

## Image size and mapping

- The build product is a **32 KiB** raw image (`START = 8000h`, "upper 32 KiB of
  ROM").
- It maps **read-only at physical `0xF8000`** (segment:offset `F000:8000`),
  spanning `0xF8000`–`0xFFFFF`.
- The CPU reset entry `F000:FFF0` (`0xFFFF0`) lands at offset **`0x7FF0`** in the
  image and far-jumps to `cold_start` / POST.
- SHA-256 of the pinned build: `1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8`.

The upstream `Makefile` also emits 128 KiB `*.rom` flash-programmer images
(`bios-xi8088-noide.rom`, `bios-xi8088-xtide.rom`) that pad the BIOS and,
optionally, prepend the XT-IDE option ROM. **We do not use those** — the
emulator maps the raw 32 KiB `bios-xi8088.bin` directly, and (later) the XT-IDE
option ROM separately into option-ROM space.

## Hard-disk support (XT-IDE option ROM)

The Xi8088 BIOS has **built-in floppy INT 13h only** (µPD765 FDC at
`0x3F0`–`0x3F7`, 8237 DMA channel 2). Hard-disk INT 13h comes from a **separate
XTIDE Universal BIOS option ROM**, shipped pre-built in the same clone under
`8088_bios/xtide_r625/` (e.g. `ide_xt-cf-lite_320h.bin`, an 8 KiB option ROM for
I/O base `0x320`). The main BIOS finds it via the standard `0AA55h` option-ROM
scan and `call far` into its init entry.

## Provenance and licence

- Upstream: <https://github.com/skiselev/8088_bios> by Sergey Kiselev.
- Licence: **GPL-3.0-or-later** (see the upstream `LICENSE.txt`).
- Used here **unmodified**: it is assembled with `nasm` and loaded into guest ROM
  by the emulator. It is not linked into, copied into, or distributed with this
  project's source.

## Not committed

Neither the upstream clone (`8088_bios/`) nor the build product
(`bios-xi8088.bin`) is checked in — both are `.gitignore`d. Run `fetch.sh`
(which needs `git` and `nasm`, both in the flake devShell) to produce
`bios-xi8088.bin` locally.
