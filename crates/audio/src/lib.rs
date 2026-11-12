// SPDX-License-Identifier: AGPL-3.0-or-later
//! Integer PCM output beside the video seam (spec §6 and §9).
//!
//! No sound-producing hardware is modelled here. The Intel 8254 datasheet,
//! Clock Input and Mode 3, describes the clock domain used by future producers.
//! See `backlog/docs/design/audio.md` §Time base and §Mixing for the exact
//! board-clock convention and interval rendering contracts.

use std::num::NonZeroU64;

mod sources;
pub use sources::{EdgeSource, RateSource, SampleBuffer};

/// Exact stereo integral in signed PCM level times whole PIT clocks.
pub type SampleArea = [i128; 2];

use device::VClock;

/// Output frames per second. Each frame contains left, then right, signed i16.
pub const FRAMES_PER_SECOND: u32 = 48_000;
/// PIT clocks per output frame, numerator (105/88 MHz divided by 48 kHz).
pub const CLOCKS_PER_FRAME_NUMERATOR: u64 = 4_375;
/// PIT clocks per output frame, denominator.
pub const CLOCKS_PER_FRAME_DENOMINATOR: u64 = 176;

/// One stereo frame. Serialization always uses left then right, little endian.
pub type StereoFrame = [i16; 2];

/// Number of complete output frames at a whole PIT-clock boundary.
///
/// Frame n covers [ceil(n * 4375/176), ceil((n+1) * 4375/176)).
/// Emit it only when its end is reached. Absolute u128 arithmetic preserves
/// phase across observations, including at the end of the u64 clock range.
#[must_use]
pub fn completed_frames(at: VClock) -> u64 {
    (u128::from(at.0) * u128::from(CLOCKS_PER_FRAME_DENOMINATOR)
        / u128::from(CLOCKS_PER_FRAME_NUMERATOR)) as u64
}

fn frame_boundary(index: u64) -> u128 {
    (u128::from(index) * u128::from(CLOCKS_PER_FRAME_NUMERATOR))
        .div_ceil(u128::from(CLOCKS_PER_FRAME_DENOMINATOR))
}

/// A source's exact period, in PIT clocks per native sample.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SamplePeriod {
    numerator: NonZeroU64,
    denominator: NonZeroU64,
}

impl SamplePeriod {
    /// A positive rational period; no rounding or floating point is involved.
    #[must_use]
    pub const fn new(numerator: NonZeroU64, denominator: NonZeroU64) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    fn sample_index(self, elapsed: u64) -> u128 {
        u128::from(elapsed) * u128::from(self.denominator.get()) / u128::from(self.numerator.get())
    }
}

/// A virtual-time signal integrated over an exact, nonempty PIT interval.
///
/// Device-backed sources read timestamped edge or DMA history, never just the
/// latest register value. Assembly settles devices before rendering. See
/// `backlog/docs/design/audio.md` §Mixing and §Source lifecycle.
pub trait AudioSource {
    /// Notify device histories whether any output consumer needs past signal.
    /// Unobserved producers may retain only the latest incomplete frame.
    fn set_output_enabled(&mut self, _enabled: bool) {}
    /// Report the first producer error at the next observation, even without PCM.
    fn take_error(&mut self) -> Option<AudioError> {
        None
    }
    /// Return exact stereo level*clock areas, without division or rounding.
    fn render(&mut self, start: VClock, end: VClock) -> SampleArea;
    /// Notify the producer that this observation has passed, even without output.
    /// Timestamped PCM uses this frontier to reject samples arriving too late.
    fn advance(&mut self, _until: VClock) {}
    /// Release history strictly below this absolute PIT boundary. The active
    /// level/sample at the boundary must survive. Called even for disabled sinks
    /// and observations completing no frame, so silent output cannot leak history.
    fn release_before(&mut self, _retain_from: VClock) {}
    /// True retires this source permanently. No further render/release callback
    /// is made once it is removed; buffered sources finish after their tail drains.
    fn finished(&self) -> bool {
        false
    }
}

/// Invalid time ordering or a PCM block inconsistent with its interval.
#[derive(Copy, Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    /// Device history is not available beyond the completed machine boundary.
    #[error("audio boundary exceeds the machine PIT clock")]
    FutureTime,
    /// Producer history must follow the PIT timeline.
    #[error("audio history is out of order")]
    HistoryOrder,
    /// Rendering cannot rewind an already delivered stream.
    #[error("audio interval runs backwards")]
    TimeReversed,
    /// Each block must contain exactly the frames completed in its interval.
    #[error("audio frame count does not match its PIT interval")]
    FrameCount,
}

/// Borrowed PCM for an exact PIT interval [start, end).
///
/// Frames whose ends are in (start, end] belong to this block. A partial frame
/// is deferred to the next block, so empty blocks and arbitrary splits are safe.
#[derive(Copy, Clone, Debug)]
pub struct AudioBlock<'a> {
    start: VClock,
    end: VClock,
    frames: &'a [StereoFrame],
}

impl<'a> AudioBlock<'a> {
    /// Validate externally rendered PCM against the absolute output grid.
    pub fn new(start: VClock, end: VClock, frames: &'a [StereoFrame]) -> Result<Self, AudioError> {
        if end < start {
            return Err(AudioError::TimeReversed);
        }
        if frames.len() as u128 != u128::from(completed_frames(end) - completed_frames(start)) {
            return Err(AudioError::FrameCount);
        }
        Ok(Self { start, end, frames })
    }

    /// Beginning of the observation interval in whole PIT clocks.
    #[must_use]
    pub fn start(&self) -> VClock {
        self.start
    }
    /// End of the observation interval in whole PIT clocks.
    #[must_use]
    pub fn end(&self) -> VClock {
        self.end
    }
    /// Interleaved left/right frames in the fixed output format.
    #[must_use]
    pub fn frames(&self) -> &'a [StereoFrame] {
        self.frames
    }
}

/// A consumer of virtual-time PCM blocks, beside `video::FrameSink`.
///
/// Receivers must return promptly, never call back into the machine, and never
/// affect guest timing. Consecutive blocks must have touching PIT intervals.
/// See `backlog/docs/design/audio.md` §Sinks and observation boundaries.
pub trait AudioSink {
    /// Receive completed frames; retain a copy if needed beyond this call.
    fn present(&mut self, block: AudioBlock<'_>);
    /// False permits the mixer to skip sampling and allocation entirely.
    fn enabled(&self) -> bool {
        true
    }
}

/// Allocation-free sink that also suppresses rendering work.
#[derive(Default)]
pub struct NullAudioSink;

impl AudioSink for NullAudioSink {
    fn present(&mut self, _block: AudioBlock<'_>) {}
    fn enabled(&self) -> bool {
        false
    }
}

/// Streaming FNV-1a capture with optional, explicitly bounded PCM retention.
///
/// Block metadata is excluded so the same virtual interval has the same hash
/// regardless of observation cadence. The default retains no PCM. A caller may
/// retain a bounded prefix without affecting the full-stream hash or frame count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureAudioSink {
    frames: Vec<StereoFrame>,
    hash: u64,
    frame_count: u64,
    frame_limit: usize,
}

impl Default for CaptureAudioSink {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            hash: 0xcbf29ce484222325,
            frame_count: 0,
            frame_limit: 0,
        }
    }
}

impl CaptureAudioSink {
    /// Create an empty capture epoch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Retain at most this many leading frames; hash and count cover all PCM.
    #[must_use]
    pub fn with_frame_limit(frame_limit: usize) -> Self {
        Self {
            frame_limit,
            ..Self::default()
        }
    }
    /// Borrow the retained prefix without cloning its buffer.
    #[must_use]
    pub fn frames(&self) -> &[StereoFrame] {
        &self.frames
    }
    /// Length in stereo frames, not individual channel samples.
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
    /// Stable FNV-1a hash of signed i16 little-endian stereo PCM bytes.
    #[must_use]
    pub fn hash(&self) -> u64 {
        self.hash
    }
    /// Encode only the retained prefix in its portable wire format.
    #[must_use]
    pub fn pcm_le(&self) -> Vec<u8> {
        self.frames
            .iter()
            .flatten()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }
}

impl AudioSink for CaptureAudioSink {
    fn present(&mut self, block: AudioBlock<'_>) {
        for byte in block
            .frames
            .iter()
            .flatten()
            .flat_map(|sample| sample.to_le_bytes())
        {
            self.hash = (self.hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        self.frame_count += block.frames.len() as u64;
        let retain = block.frames.len().min(self.frame_limit - self.frames.len());
        self.frames.extend_from_slice(&block.frames[..retain]);
    }
}

struct Source {
    origin: VClock,
    signal: Box<dyn AudioSource>,
}

/// Interval-average, saturating stereo mixer.
///
/// Each source contributes exact level*clock area over the output frame's PIT
/// interval. Sum in i128, divide once by frame duration, then clamp so
/// clipping is independent of source order. See `backlog/docs/design/audio.md`
/// §Mixing; spec §6 defines device time.
pub struct Mixer {
    until: VClock,
    sources: Vec<Source>,
    stream_started: bool,
}

impl Mixer {
    /// Propagate output selection without requiring a host render boundary.
    pub fn set_output_enabled(&mut self, enabled: bool) {
        for source in &mut self.sources {
            source.signal.set_output_enabled(enabled);
        }
    }
    /// Begin delivery at this boundary on the absolute output grid.
    #[must_use]
    pub fn new(start: VClock) -> Self {
        Self {
            until: start,
            sources: Vec::new(),
            stream_started: false,
        }
    }

    /// Attach a virtual-time signal. It is silent before its origin. The source
    /// must retain history from any not-yet-rendered output frame's start.
    pub fn add_source(&mut self, origin: VClock, signal: Box<dyn AudioSource>) {
        self.stream_started = true;
        self.sources.push(Source { origin, signal });
    }

    /// Render at a completed machine service boundary, never per instruction.
    /// Never-started streams and disabled sinks skip PCM allocation and sampling.
    /// After source retirement the started stream continues with silence, so
    /// its frame count cannot depend on observation splits.
    /// Batches use a bounded scratch buffer even for very long observations.
    pub fn render_until(
        &mut self,
        end: VClock,
        sink: &mut dyn AudioSink,
    ) -> Result<(), AudioError> {
        if end < self.until {
            return Err(AudioError::TimeReversed);
        }
        for source in &mut self.sources {
            if let Some(error) = source.signal.take_error() {
                self.release_history();
                return Err(error);
            }
        }
        self.sources.retain(|source| !source.signal.finished());
        if !self.stream_started || !sink.enabled() || end == self.until {
            self.until = end;
            self.release_history();
            return Ok(());
        }
        let mut index = completed_frames(self.until);
        let final_index = completed_frames(end);
        let mut scratch = [[0i16; 2]; 1024];
        loop {
            let count = (final_index - index).min(scratch.len() as u64) as usize;
            for frame in &mut scratch[..count] {
                let start = VClock(frame_boundary(index) as u64);
                let end = VClock(frame_boundary(index + 1) as u64);
                let duration = i128::from(end.0 - start.0);
                let mut sum = [0i128; 2];
                for source in &mut self.sources {
                    let active_start = start.max(source.origin);
                    if active_start < end {
                        let area = source.signal.render(active_start, end);
                        for channel in 0..2 {
                            sum[channel] += area[channel];
                        }
                    }
                }
                for channel in 0..2 {
                    frame[channel] = (sum[channel] / duration)
                        .clamp(i128::from(i16::MIN), i128::from(i16::MAX))
                        as i16;
                }
                index += 1;
            }
            let block_end = if index == final_index {
                end
            } else {
                VClock(frame_boundary(index) as u64)
            };
            sink.present(AudioBlock {
                start: self.until,
                end: block_end,
                frames: &scratch[..count],
            });
            self.until = block_end;
            if index == final_index {
                self.release_history();
                return Ok(());
            }
        }
    }
    fn release_history(&mut self) {
        // An incomplete frame still needs its beginning on the next observation.
        let retain_from = VClock(frame_boundary(completed_frames(self.until)) as u64);
        for source in &mut self.sources {
            source.signal.advance(self.until);
            source.signal.release_before(retain_from);
        }
        self.sources.retain(|source| !source.signal.finished());
    }
}

#[cfg(test)]
mod tests;
