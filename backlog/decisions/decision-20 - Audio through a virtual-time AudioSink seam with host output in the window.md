---
id: decision-20
title: Audio through a virtual-time AudioSink seam with host output in the window
date: 2026-11-12
status: Accepted
deciders: Mikołaj Mikołajczyk
---

unemu maintains the application portions; unemupc maintains the core portions of this shared historical decision.

# decision-20 - Audio through a virtual-time AudioSink seam with host output in the window

## Status

Accepted — 2026-11-12. Part of phase C of [decision-16 (unemupc)](https://github.com/unemu-org/unemupc/blob/0f2795402e947e235a7b892c931910b05f05d61f/backlog/decisions/decision-16%20-%20Scope%20a%20period%20PC%20from%20DOS%20to%20Windows%2098%20SE%20with%203dfx%20Voodoo.md) (Sound Blaster 16
and PC speaker with an audio sink seam). Bound by the determinism rules of
[decision-14 (unemupc)](https://github.com/unemu-org/unemupc/blob/0f2795402e947e235a7b892c931910b05f05d61f/backlog/decisions/decision-14%20-%20Virtual%20time%20advances%20while%20the%20CPU%20is%20halted.md) (virtual time) and [decision-15 (unemupc)](https://github.com/unemu-org/unemupc/blob/0f2795402e947e235a7b892c931910b05f05d61f/backlog/decisions/decision-15%20-%20Deterministic%20device%20scheduler%20with%20instruction-exact%20interrupt%20boundaries.md) (device scheduler).

## Context

[decision-16 (unemupc)](https://github.com/unemu-org/unemupc/blob/0f2795402e947e235a7b892c931910b05f05d61f/backlog/decisions/decision-16%20-%20Scope%20a%20period%20PC%20from%20DOS%20to%20Windows%2098%20SE%20with%203dfx%20Voodoo.md) puts a DMA-driven Sound Blaster 16 and the PC speaker into
phase C, "with an audio sink seam beside FrameSink: deterministic headless
capture for goldens, host audio output only in the window crate". Video
already works this way: devices render into a `FrameSink`, headless runs
capture frames for golden hashes, and only the window crate talks to the
host display.

Audio adds two constraints that video does not have. Sound is a continuous
stream, so its time base must be exact: a sample position derived from
rounded floating-point time would drift and make captures differ between
hosts. And a host audio device consumes samples at its own wall-clock rate,
which must never feed back into emulated time.

## Decision

1. **One output format.** The machine produces a single audio stream:
   signed 16-bit little-endian, stereo interleaved, 48,000 frames per second.
2. **Virtual time base.** Sample positions derive from PIT input clocks
   (the board's 105/88 MHz timer clock) with exact integer rational
   arithmetic. Sample `n` belongs to the PIT clock interval that contains it;
   no floating point enters the mapping. Audio therefore follows the same
   virtual clock as every other device, including PIT clocks that pass while
   the CPU is halted.
3. **AudioSink seam.** An `AudioSink` trait sits beside `FrameSink`.
   Audio-producing devices render their own signal in virtual time; a mixer
   in the emulator combines them and converts device-native rates (the PC
   speaker's square wave, the SB16's programmable sample rate) to the output
   format. All mixing and resampling in the emulated path uses integer or
   fixed-point arithmetic, so the PCM is bit-identical on every host.
4. **Headless capture.** A capture sink records the stream for tests.
   Goldens assert a hash of the PCM (and its length in frames); short
   reference WAV files may be committed only for open content.
5. **Host output in the window only.** The window crate plays the stream
   through `cpal`. Its audio callback reads from a bounded ring buffer that
   the window fills at run boundaries. On underrun it plays silence; on
   overrun it drops the oldest samples. Neither event changes emulation:
   pacing stays with the window's existing host-clock policy, and audio
   never drives virtual time.
6. **Order of work.** First the seam with headless capture; then the PC
   speaker (PIT channel 2 OUT, gated by port 61h bit 0, enabled by bit 1);
   then the SB16 DSP with 8-bit DMA (single-cycle and auto-initialise), then
   16-bit DMA and the mixer. OPL3 FM synthesis is a separate bead. Its
   implementation source (our own from the YMF262 datasheet, or an existing
   core under a licence compatible with this project's AGPL-3.0-or-later) is
   decided when that bead is planned.

## Consequences

- Audio goldens are as strict as video goldens: a change in any device's
  timing or in the mixer moves a PCM hash, and the reason must be stated.
- Integer DSP is more work than floating point, but it removes a class of
  host-dependent differences.
- 48 kHz stereo costs about 192 KB per emulated second in captures; tests
  hash rather than store.
- A host without an audio device still runs: the window falls back to no
  output, and headless runs never open one.
- Devices that need a different native rate (a future Gravis or Voodoo-era
  card) render at their own rate and go through the same mixer.
