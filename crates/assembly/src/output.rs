// SPDX-License-Identifier: AGPL-3.0-or-later
//! Output configuration (§9).

/// Which [`video::FrameSink`] the machine presents through (§9).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum FrameSinkKind {
    /// No presenter — a headless run (the serial-first target). Video memory is
    /// still modelled and can be captured through the trace seam.
    #[default]
    Headless,
    /// Capture finished frames for golden comparison.
    Capture,
}

/// The output section of a [`crate::MachineConfig`] (§9).
///
/// Kept deliberately minimal: how a machine is *observed* is mostly not part of
/// its identity (serial output is drained from the UART and video output is read
/// back through the trace seam, so there is no `serial_capture` flag here). What remains is the one
/// genuine machine-shaping choice — which [`video::FrameSink`] the frames go to —
/// and this struct is its home so a future presenter option does not force a
/// config restructure.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct OutputConfig {
    /// The presenter the machine's frames are handed to.
    pub frame_sink: FrameSinkKind,
}
