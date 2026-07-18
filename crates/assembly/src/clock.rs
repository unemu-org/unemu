// SPDX-License-Identifier: AGPL-3.0-or-later
//! Virtual-clock configuration (§6).

/// How the deterministic virtual clock is driven (§6).
///
/// Minimal on purpose — but the home for future timebase fields (a real bus
/// speed, a per-chip input frequency) so callers do not have to restructure the
/// config when those arrive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockConfig {
    /// Retired guest instructions per PIT input clock — a **modelling ratio**,
    /// not a frequency. `1` means one PIT clock per retired instruction, the
    /// simplest deterministic mapping and the value goldens are baselined
    /// against; it is deliberately not the real ~1.193182 MHz timebase. See
    /// [`machine::RETIRED_PER_PIT_CLOCK`], which this defaults to.
    pub retired_per_pit_clock: u64,
    /// Cooperative run budget in CPU-core block units, or `None` to run each
    /// quantum to a non-budget stop. Governs interrupt latency, so goldens
    /// depend on it.
    pub quantum: Option<u64>,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            retired_per_pit_clock: machine::RETIRED_PER_PIT_CLOCK,
            quantum: None,
        }
    }
}
