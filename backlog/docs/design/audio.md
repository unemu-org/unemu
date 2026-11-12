# Deterministic audio

The `audio` crate is the audio swap layer beside `video`. It provides an
`AudioSink`, an integer mixer and headless capture, implementing rules 1–4 of
[decision-20](../../decisions/decision-20%20-%20Audio%20through%20a%20virtual-time%20AudioSink%20seam%20with%20host%20output%20in%20the%20window.md).
It adds no sound-producing device, host playback, sound-card registers, DMA,
analogue filtering or synthesizer. The PC speaker and Sound Blaster are later
device work. No new external dependency is needed: only the existing `device`
and `thiserror` workspace dependencies are used.

## Output format

The one output format is **48,000 stereo frames/s**, signed i16, left then
right, little endian. A `StereoFrame` is `[i16; 2]`; `CaptureAudioSink::pcm_le`
serializes explicitly, independent of host endianness. One frame is four bytes,
not one channel sample.

## Time base

Audio uses the exact PIT input rate **105,000,000/88 Hz**, hence the output
period is **4375/176 PIT clocks**. This is the clock ratio required by the audio
contract. Intel 8254 Programmable Interval Timer datasheet, Clock Input and
Mode 3, defines the timer's input-clock counting and square-wave behavior; the
board frequency is a machine convention, not an 8254 silicon constant.

Frame `n` covers the whole-clock interval
`[ceil(n * 4375/176), ceil((n+1) * 4375/176))`. It is emitted only when its end
has been reached. At PIT boundary `t`, `F(t) = floor(t * 176/4375)` frames are
complete. Observation `[a, b)` receives frame indices `F(a)..F(b)` (frame ends
in `(a, b]`); a partial frame at `a` was not previously delivered. Empty
intervals or intervals with no complete frame are valid. Every calculation uses
absolute positions and u128 intermediates, including clocks near `u64::MAX`.
There is no carried rounded sample duration and no long-run drift.

The facade passes `Machine::clock()` (`VClock`), the same whole-PIT-clock
timeline the PIT consumes. It does not convert `virtual_time()` or `retired()`
back into clocks: the scheduler already carries the configured CPU rate's
fractional remainder and advances PIT time during HLT. CPU reset does not reset
the device clock or audio phase. See [scheduler.md §Adoption](scheduler.md#adoption).

The window and RTC currently describe their nominal clock with
`14,318,180/12 Hz`; this differs slightly from `105,000,000/88 Hz`. This seam
deliberately follows the audio contract without changing existing presentation,
RTC timing or goldens. Its frame counts are exact against PIT clocks at the
ratio above; this does not claim exact agreement with the window's host-second
conversion.

## Mixing

`AudioSource::render(start, end)` returns `SampleArea`, two signed i128
**PCM level × PIT clock** integrals over the exact nonempty interval. No source
divides or rounds its contribution. The mixer sums all source areas, divides
once by the full output frame's duration (toward zero), then saturates to
`[-32768,32767]`. Thus two sources at level 1 for 10 and 15 clocks of a 25-clock
frame produce exactly 1, including the symmetric negative case. No fixed-point
approximation loses fractions between sources.

Intervals use absolute timestamps. Source attachment clips the requested start
to the attachment time, so a source joining inside a frame contributes only its
active clocks; the mixer still divides by the full frame duration. Scratch
storage remains bounded at 1024 output frames. No device re-derives the output
grid and no floating point or host timing enters the path.

`EdgeSource` records ordered `(PIT clock, [left, right])` level changes and sums
`level * clocks` between edges. An edge at the start applies immediately; one
at the end applies to the next frame. Same-clock edges are allowed; the last
wins. Box filtering retains the duty cycle of fast square waves and PWM, with
the documented integer-clock quantization, rather than modelling analogue filters.

`RateSource` reads a shared `SampleBuffer` for the **whole device lifetime**.
The writer calls `push_segment(origin, period, frames)` to begin a rate/phase
segment and `push(frames)` to append samples to that segment. Every segment has
its own exact positive `SamplePeriod(P,Q)`. Native sample k covers
`[o + ceil(k*P/Q), o + ceil((k+1)*P/Q))`. A native rate H Hz uses
`P/Q = 105,000,000 / (88 * H)`.

The rate helper weights **every** native sample overlapping an output interval
by its whole PIT-clock overlap. It advances to the next native boundary, not the
next output sample point. Native sample indices and boundary products use u128;
unrepresentable timestamps are rejected on insertion. Periods below one PIT
clock may have zero-duration native samples; the lookup selects the last sample
active at each whole clock and skips those empty intervals.

A new segment cuts the previous held sample at its origin. `pause(at)` ends the
current segment exactly at at; the gap before the next segment is silence.
A rate change or resume appends a new origin/period to the **same buffer and
source**, including a change inside an output frame. Do not seal or attach a
replacement source for those events. Record playback history in time order;
do not pre-assign future sample times that a later device command may change.

`SampleBuffer` rejects a late sample or segment with `AudioError::HistoryOrder`:
push no later than the sample's start time, and devices push before the next
observation boundary. A rejected batch changes no history. The buffer tracks
both the safe history-release watermark and the full observation frontier:
`AudioSource::advance(until)` notifies it even for None/null output or an
observation completing no frame. Once an observation has passed a sample start,
that missing sample cannot silently be filled after silence was already rendered
or time was skipped. A sample starting exactly at the frontier is still allowed.
Sealed, paused, out-of-order and unrepresentable writes use the same error.
Clones are writer handles for one mixer consumer, not independent cursors.

## Source lifecycle

Every valid `Mixer::render_until` call invokes each active source's
`release_before(retain_from)`, including disabled/null output, repeated
boundaries and intervals completing no frame. The safe watermark is the start
of the next incomplete frame, `ceil(F(until) * 4375/176)`, not blindly `until`:
a subsequent observation still needs that frame's beginning. Device histories
may discard older records but must preserve the active level/native sample at
the watermark. Thus default output cannot accumulate history across observations;
producer-side memory between observations remains bounded by the caller's run
chunk size. Invalid backwards/future requests do not advance the release cursor.

`AudioSource::finished()` permanently retires a source. Finished sources are
removed before rendering and after release, and receive no further callbacks.
`SampleBuffer::finish()` seals a device's final native tail at the end of its
lifetime; rate changes and pauses do not seal it. Closed rate segments and old
native frames are released after they can no longer affect an incomplete output
frame. At most the latest empty appendable segment needs to survive after all
its PCM is released. `EdgeSource::finish_at` retains its existing timestamped
end behavior.

A started stream continues emitting silence after its last source retires,
preserving frame count and hash across observation splits. A mixer that never
had a source still emits nothing. Custom sources may implement the same finished
contract after their final pending tail has been rendered.

## Sinks and observation boundaries

`ObservationPolicy.output.audio_sink` selects `None` (default) or `Capture`,
beside `frame_sink`. Capture's explicit `audio_capture_frame_limit` defaults to
zero: hash and total frame count are always maintained, while PCM retention is
optional. Selecting capture creates no producer or mixer. External sinks attach
through `Observation::subscribe_audio`; the configured capture receives each
block first. Sinks return promptly and never reenter or alter machine execution.

`AssembledMachine::add_audio_source` attaches a source at the current PIT clock.
Inline hosts use `Observation::add_audio_source(origin, source)`. Native rate
segments and edge history carry their own absolute timestamps.
The first source creates the mixer at that boundary. Subsequent sources join
the existing stream and contribute only from their attachment times onward.
Sources read timestamped history recorded by the device (edges or DMA blocks),
not merely a device's latest pin/register state.

Rendering takes mutable machine access. `Observation::capture_audio(machine,
until)` rejects `until > machine.clock()` with `AudioError::FutureTime` before
syncing devices or reading sources. For a valid boundary with a mixer, it calls
`Bus::settle()` to bring dormant/lazily scheduled devices to the current clock,
then renders from their history. No mixer means no settle or PCM work. Earlier
completed boundaries are allowed only if they do not rewind the mixer cursor.
Direct machine runners call `AssembledMachine::capture_audio()` at observation
boundaries; that facade always uses the current machine clock.

The assembly run driver first obtains the machine result and performs its
post-run persistence check **even when execution failed**. It propagates a
machine error first, then a persistence error; audio is rendered only if both
succeeded. Thus an audio error cannot hide a machine failure or skip checking
journal failures. Reset handling follows successful capture. Prompt polling and
normal service boundaries use this same path. Audio adds no scheduler events or
per-instruction work. With no producer there is no manufactured silence. Disabled
receivers advance phase and release history without sampling or allocating PCM.

`CaptureAudioSink::new()` hashes/counts without retaining PCM;
`with_frame_limit(N)` retains at most the first N frames. Both keep a streaming
64-bit FNV-1a hash over **all** interleaved little-endian PCM bytes and a total
frame count, independent of retention and block splits. `frames()` borrows the
retained prefix; `pcm_le()` explicitly encodes that prefix only. The facade's
`Observation::captured_audio()` returns a borrowed guard, not a buffer clone.
Drop that guard before rendering or reconfiguring. Hash/length assertions require
no PCM allocation. Existing video hash helpers do not move.

`configure_output` keeps its replace-the-capture behavior, mirroring video:
it starts a fresh configured epoch while retaining mixer phase and external
subscribers. Flush to the desired epoch boundary before reconfiguring. Backwards
rendering returns a typed error and preserves the cursor. Repeated observations
emit no duplicate PCM, but still release source history.

## Validation

The exact time-base, capture-guard, post-error, HLT and device-sync tests remain.
Native constant/ramp/square expectations now use clock-weighted integration:
for example the PIT-rate ramp averages 0..24 to 12 in the first frame, and a
96 kHz square contributes both halves instead of only its initial level.
Dedicated 44.1 kHz and phase-offset 8 kHz cases check boundaries strictly inside
an output frame. The offset matters because a zero-origin 8 kHz grid aligns
with every sixth 48 kHz frame boundary.

A rate change and a pause inside one frame, their tails across later frames,
and a later resume all use one source. Repeated rate/pause segments with output
disabled retain at most two native samples and one segment at the chosen
observation cadence; those samples cover an incomplete frame and must survive.
Late writes fail without changing previously captured silence. The 10+15-clock
unit-level regression proves exact cross-source summation.

The frozen three-source, 176-frame hash changes from `1f672be3ca235cfd` to
`2b19e84103e1b4b2`: 44.1 kHz native transitions within a frame now contribute
clock-weighted blends, and sources no longer round separately in Q32. An
independent per-PIT-clock integer reference confirms the new hash. Whole and
split renders agree. Existing guest/video golden expectations are untouched.
