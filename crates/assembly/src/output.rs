// SPDX-License-Identifier: AGPL-3.0-or-later
//! Presentation selection for assembly-owned observation (§9).
//!
//! See `backlog/docs/design/observation.md`, §Sinks, bounds and lifecycle.

/// The initial presentation subscriber for each requested display.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum FrameSinkKind {
    /// No presentation subscriber; observation and prompt detection stay enabled.
    #[default]
    Headless,
    /// Keep the last captured frame of each display in a `video::CaptureFrameSink`.
    Capture,
}

/// Initial receiver for the mixed PCM stream; no audio device is implied.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum AudioSinkKind {
    /// No capture or presentation; rendering skips all signal samples.
    #[default]
    None,
    /// Retain deterministic PCM for headless assertions.
    Capture,
}

/// Presentation wiring, independent of hardware and observation cadence (§9).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct OutputConfig {
    /// Initial sink for every text route selected by the observation policy.
    pub frame_sink: FrameSinkKind,
    /// Initial receiver for audio, independent of display routes.
    pub audio_sink: AudioSinkKind,
    /// Maximum leading PCM frames retained by Capture; zero keeps only hash/count.
    pub audio_capture_frame_limit: usize,
}
