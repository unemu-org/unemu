# 8088_bios — Xi8088 configuration (boot firmware)

`bios-xi8088.bin` and `bios-xi8088-atreset.bin` are built by [`fetch.sh`](fetch.sh) from
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
- SHA-256 of the pinned build (checked by `fetch.sh`): `1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8`.

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
- The original image uses **unmodified** upstream source. The second applies
  our [`at-reset.patch`](at-reset.patch), a modification of GPL-3.0-or-later
  source, also licensed **GPL-3.0-or-later**. The patch is committed; upstream
  source and generated ROM bytes are not. Both images are assembled with
  `nasm` and loaded into guest ROM, without linking firmware into the emulator.

## AT reset compatibility image

`bios-xi8088-atreset.bin` differs only by `at-reset.patch` in `src/floppy2.inc`.
It dispatches AH=00h to the existing diskette reset procedure before validating
DL. Status AH=01h already bypasses drive validation and stays unchanged.
Every other function retains upstream validation and behaviour.

The [IBM PC AT Technical Reference, September 1985 BIOS listing](https://github.com/jeffpar/pcjs/tree/gh-pages/machines/pcx86/ibm/5170/rom/bios/1985-06-10/src)
exempts reset and status from drive validation in `DSKETTE.ASM:236–237`
(`OK_FUNC` / `OK_DRV`). `DISK.ASM:501–508` forwards a hard-disk reset
(AH=00h, DL≥80h) through INT 40h without changing DL. Upstream Kiselev
`src/floppy2.inc:111–127` rejects DL>1 before reset dispatch, unlike IBM.

XTIDE Universal BIOS r638 follows those AT semantics:
`XTIDE_Universal_BIOS/Src/Handlers/Int19h.asm:137–141` resets with DL=80h,
and `Src/Handlers/Int13h/AH0h_HReset.asm:39–44` forwards that request to the
previous handler. The patch lets Kiselev reset and sense the controller before
XUB boots the floppy. It adds no fixed-disk driver or CMOS drive-type handling.

Both images are 32,768 bytes and use the same pinned upstream commit:

| Output | SHA-256 |
|---|---|
| `bios-xi8088.bin` (original) | `1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8` |
| `bios-xi8088-atreset.bin` | `61589573c9809273198b137d9419c87242b0a5f63c2ad834a9565950c3d6becc` |

The original remains selected by `machines/at-freedos.toml` and the existing
goldens. `machines/at-ide.toml` selects the compatibility image and the configured
XT+ option ROM.

## Source archive

`fetch.sh` also writes `kiselev-bios-a4af458-atreset-src.tar`, the
corresponding source of both images. It is what a published firmware package
carries as its `source` layer: the images are GPL-3.0-or-later, so they may be
published only together with it. Its root directory,
`kiselev-bios-a4af458-atreset-src/`, holds:

- `8088_bios/`: upstream at the pinned commit with `at-reset.patch` applied.
  Upstream's prebuilt images are left out: `binaries/` and `xtide_r625/` are
  build outputs that neither image uses, and `xtide_r625/` holds XTIDE
  Universal BIOS r625 binaries whose source the archive does not carry.
  Upstream's flash-image targets that combine the BIOS with those binaries
  (the `*-xtide*.rom` images in `src/Makefile` and `CMakeLists.txt`)
  therefore do not build from the archive; the 32 KiB images do.
- `at-reset.patch`: the committed patch, unchanged.
- `NOTICE`: [`source-notice.txt`](source-notice.txt), unchanged. It names the
  upstream repository, commit and licence, both image hashes, the date of our
  change (2026-11-11), and the `nasm` commands that rebuild both images
  (reversing the patch restores the original's source).

The whole archive is **GPL-3.0-or-later**. It is an uncompressed POSIX ustar
file written by GNU tar: members sorted by name, every timestamp set to the
pinned commit's time (2026-07-16 17:18:55 UTC), owner and group 0 with no
names, and modes 0644 or 0755. `fetch.sh` writes it after both images. On
every run it first assembles the AT reset image from the staged tree and
compares it with the pinned image hash, so the archive always builds what we
ship; then it checks the archive's own SHA-256:

| Output | SHA-256 |
|---|---|
| `kiselev-bios-a4af458-atreset-src.tar` | `ced63707661747fc045d1e6e565a5cc9be73f95a9c944accedd77b19467fb5f9` |

MEASURED on 2026-11-15 on aarch64-darwin with GNU tar 1.35 and NASM 3.01:
repeated runs give this hash, and the `NOTICE` commands run in an extracted
archive rebuild both pinned image hashes. A Linux run has not been measured;
the archive holds no host-dependent field, and the devShell pins the same
GNU tar on both systems.

## Not committed

The upstream clone (`8088_bios/`), temporary exports (`build/`), both ROM
outputs and the source archive are `.gitignore`d. Run `fetch.sh` (which needs
`git`, `nasm`, `patch` and GNU `tar`, all in the flake devShell) to produce
them locally. Outside the devShell on macOS, the system's BSD `tar` cannot
write the archive deterministically, and the script stops with a message.

## Reproducible and offline builds

`fetch.sh` checks the output hash, so changes in assembler output cannot silently
change golden fixtures. A mismatch reports the file and both hashes. Check the
pinned sources and toolchain; deliberately rebuild and review the goldens'
expectations before updating the expected output hash.

The script exports the pinned commit into a temporary directory, assembles and
checks the original image, applies the patch with zero fuzz, then assembles and
checks the compatibility image. It then exports the commit again, applies the
patch and writes the source archive. Each output moves into place only after
its size and hash pass; the cached checkout is not modified.

The clone's work tree may remain on upstream's default branch. All source
paths and line references here refer to the pinned commit, which can be read
without changing that work tree. From the repository root, for example:

```sh
git -C vendor/kiselev-bios/8088_bios show a4af458a72bc33c4be066b33bd76e8af2b24111c:src/floppy2.inc
```

Use the same `git show` form for other cited `src/` files.

The script uses portable size checks and either `sha256sum` or `shasum -a 256`
(on macOS); the flake devShell supplies the build tools. When the clone already
contains the pinned commit, rebuilding skips the network, archive included. A
missing clone or commit is fetched as before, so the first run needs the
network.
