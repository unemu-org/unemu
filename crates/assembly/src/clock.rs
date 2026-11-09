// SPDX-License-Identifier: AGPL-3.0-or-later
//! Virtual-clock configuration (§6).

use std::num::NonZeroU64;

use crate::error::ConfigError;

/// How the deterministic virtual clock is driven (§6).
///
/// Minimal on purpose — but the home for future timebase fields (a real bus
/// speed, a per-chip input frequency) so callers do not have to restructure the
/// config when those arrive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockConfig {
    /// Numerator A of the CPU rate A/B, in retired steps per PIT input clock.
    /// B is [`Self::retired_per_pit_clock_denominator`]. Both must be positive;
    /// validation rejects zero. Each completed REP iteration counts as a step.
    /// This is a modelling ratio, not a frequency or cycle cost.
    /// The default `1/1` preserves the golden timebase. Fractional PIT clocks
    /// carry across CPU calls; HLT advances whole PIT clocks independently.
    pub retired_per_pit_clock: u64,
    /// Denominator B of the retired-steps-per-PIT-clock rate A/B, default `1`.
    /// For example, A=2, B=1 emits one PIT clock per two retired steps; A=1,
    /// B=3 emits three clocks per step. The ratio need not be reduced.
    pub retired_per_pit_clock_denominator: u64,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            retired_per_pit_clock: machine::RETIRED_PER_PIT_CLOCK,
            retired_per_pit_clock_denominator: 1,
        }
    }
}

impl ClockConfig {
    pub(crate) fn cpu_rate(&self) -> Result<(NonZeroU64, NonZeroU64), ConfigError> {
        match (
            NonZeroU64::new(self.retired_per_pit_clock),
            NonZeroU64::new(self.retired_per_pit_clock_denominator),
        ) {
            (Some(numerator), Some(denominator)) => Ok((numerator, denominator)),
            _ => Err(ConfigError::InvalidCpuRate {
                numerator: self.retired_per_pit_clock,
                denominator: self.retired_per_pit_clock_denominator,
            }),
        }
    }
}
