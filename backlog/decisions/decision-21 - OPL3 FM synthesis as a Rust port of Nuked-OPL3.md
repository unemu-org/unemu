---
id: decision-21
title: OPL3 FM synthesis as a Rust port of Nuked-OPL3
date: 2026-11-12
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-21 - OPL3 FM synthesis as a Rust port of Nuked-OPL3

## Status

Accepted — 2026-11-12. Settles the open point of [[decision-20]] rule 6
(the OPL3 implementation source is decided when its bead is planned).

## Context

The Sound Blaster 16 carries a Yamaha YMF262 (OPL3) FM synthesizer, and
most DOS and Windows 9x games of the target era use it for music. The
emulated path must be deterministic and bit-identical on every host
([[decision-14]], [[decision-20]] rule 3), so the synthesizer must use
integer arithmetic only.

Three sources were considered:

- our own implementation from the YMF262 datasheet: the datasheet does not
  describe the log-sine and exponent tables, the envelope generator's exact
  rate behaviour or several rounding steps, so the result would sound close
  but not match the chip;
- ymfm (BSD-3-Clause), a general Yamaha FM library: integer-only and well
  written, slightly less exact for OPL3 than the alternative below;
- Nuked-OPL3 (LGPL-2.1-or-later): reconstructed from YMF262 die shots and
  decapsulated ROM tables, integer-only, and the reference other emulators
  measure against.

## Decision

1. **Source.** The OPL3 device is a Rust port of Nuked-OPL3 1.8, upstream
   commit `765ec962e473aeb767e4cba74ffdc8f588ffbfe8`. The port keeps the
   chip logic as upstream has it (operator, envelope, phase, rhythm, tremolo
   and vibrato behaviour, tables); Rust idioms are allowed where they do not
   change any output sample.
2. **Licence.** Ported files keep Nuked-OPL3's copyright notice and carry
   `SPDX-License-Identifier: LGPL-2.1-or-later`, an exception to the
   project's per-file AGPL header ([[decision-11]]). LGPL-2.1-or-later code
   may be combined into this AGPL-3.0-or-later project; the combined program
   stays AGPL-3.0-or-later. The attribution is also recorded in the device's
   documentation.
3. **Proof of the port.** The port is checked bit-exact against output of the
   unmodified C original for fixed register-write sequences. The reference
   vectors are generated outside the repository from the pinned upstream
   commit; only the vectors (and the program that produced them, if small)
   are committed.
4. **Time base.** The chip runs from the ISA 14.31818 MHz clock and produces
   one sample per 288 master clocks. The PIT input clock is that clock
   divided by 12, so one OPL3 sample is exactly 24 PIT clocks, and timer 1
   (4 samples) and timer 2 (16 samples) tick every 96 and 384 PIT clocks.
   Register writes take effect at the next sample boundary in virtual time.
   Native samples feed the existing mixer like every other source; the
   upstream resampler and write buffer are not used.

## Consequences

- FM music matches the reference emulator sample for sample, and audio
  goldens can pin it.
- A second licence appears in the source tree, limited to the ported files;
  the publication guard and licence checks must allow it there.
- Upstream fixes after the pinned commit are taken deliberately, with the
  reference vectors regenerated and the change in output stated.
