// SPDX-License-Identifier: AGPL-3.0-or-later
//! Output configuration (§9).

/// The output section of a [`crate::MachineConfig`] (§9).
///
/// Presentation is currently performed by the caller through the video trace
/// seam. There are no assembly-time output choices.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct OutputConfig {}
