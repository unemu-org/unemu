// SPDX-License-Identifier: AGPL-3.0-or-later
//! A tick deadline as a retired-instruction cap; no host clock is read here.
//!
//! A front end paces a machine in virtual ticks of the board timebase; the PC
//! scheduler runs to a cap in retired instructions. The machine's configured
//! rate (retired instructions per PIT input clock, an exact fraction) converts
//! one into the other. See `backlog/docs/design/observation.md` § Window
//! pacing for rounding and instruction-boundary limits.

use device::clock::TICKS_PER_PIT_CLOCK;
use std::num::NonZeroU64;

/// Retired instructions per PIT input clock, an exact fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuRate {
    pub numerator: NonZeroU64,
    pub denominator: NonZeroU64,
}

/// Pure conversion from a virtual-tick target to an absolute virtual-time run cap.
/// Ceiling avoids stalling at sub-instruction clock distances. The scheduler
/// owns carried active/HLT fractions; callers recheck its clock after each run.
pub fn run_cap(target: u64, clock: u64, virtual_time: u64, rate: CpuRate) -> u64 {
    let steps = (u128::from(target.saturating_sub(clock)) * u128::from(rate.numerator.get()))
        .div_ceil(u128::from(rate.denominator.get()) * u128::from(TICKS_PER_PIT_CLOCK));
    virtual_time.saturating_add(u64::try_from(steps).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(a: u64, b: u64) -> CpuRate {
        CpuRate {
            numerator: NonZeroU64::new(a).unwrap(),
            denominator: NonZeroU64::new(b).unwrap(),
        }
    }

    #[test]
    fn caps_respect_configured_rates_and_saturate_without_overflow() {
        assert_eq!(run_cap(11, 1, 7, rate(5, 2)), 32);
        assert_eq!(run_cap(12, 1, 7, rate(2, 5)), 12);
        assert_eq!(run_cap(u64::MAX, 0, 1, rate(u64::MAX, 1)), u64::MAX);
        assert_eq!(run_cap(1, 0, u64::MAX, rate(1, 1)), u64::MAX);
        assert_eq!(run_cap((1 << 53) + 1, 0, 7, rate(1, 1)), (1 << 53) + 8);
        // An ahead guest does no work.
        assert_eq!(run_cap(10, 20, 100, rate(1, 1)), 100);
        assert_eq!(run_cap(0, 0, 0, rate(1, 1)), 0);
    }
}
