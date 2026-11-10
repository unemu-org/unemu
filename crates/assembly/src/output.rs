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

/// Presentation wiring, independent of hardware and observation cadence (§9).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct OutputConfig {
    /// Initial sink for every text route selected by the observation policy.
    pub frame_sink: FrameSinkKind,
}
