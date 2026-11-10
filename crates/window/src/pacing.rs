// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host presentation policy for the AT timer; no host clock is read here.
//!
//! IBM Personal Computer AT Technical Reference (March 1984), System Board,
//! "System Timers" and "Oscillator": the timer input is the 14.31818 MHz
//! oscillator divided by twelve. See `backlog/docs/design/observation.md`
//! § Window pacing for rounding, overload and instruction-boundary limits.

use std::{num::NonZeroU64, time::Duration};

const OSCILLATOR_HZ: u128 = 14_318_180;
const PIT_DIVISOR: u128 = 12;
const NANOS_PER_SECOND: u128 = 1_000_000_000;
/// Sixty paints per second keeps input responsive without an uncapped redraw loop.
const FRAME_HZ: u128 = 60;
/// At most 50 ms of guest time per paint prevents a growing catch-up backlog.
const MAX_GUEST_TIME: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
pub(crate) struct CpuRate {
    pub numerator: NonZeroU64,
    pub denominator: NonZeroU64,
}

pub(crate) fn pit_clocks(elapsed: Duration) -> u128 {
    elapsed.as_nanos() * OSCILLATOR_HZ / (PIT_DIVISOR * NANOS_PER_SECOND)
}

/// Return an absolute target and the cumulative debt to discard permanently.
/// Keeping the debt separate preserves fractional clocks in the wall epoch.
pub(crate) fn frame_target(elapsed: Duration, clock: u64, dropped: u128) -> (u64, u128) {
    let wall_target = pit_clocks(elapsed).saturating_sub(dropped);
    let target = wall_target.min(u128::from(clock) + pit_clocks(MAX_GUEST_TIME));
    (
        u64::try_from(target).unwrap_or(u64::MAX),
        dropped + wall_target - target,
    )
}

/// Pure conversion from a PIT target to an absolute virtual-time run cap.
/// Ceiling avoids stalling at sub-instruction clock distances. The scheduler
/// owns carried active/HLT fractions; callers recheck its clock after each run.
pub(crate) fn run_cap(target: u64, clock: u64, virtual_time: u64, rate: CpuRate) -> u64 {
    let steps = (u128::from(target.saturating_sub(clock)) * u128::from(rate.numerator.get()))
        .div_ceil(u128::from(rate.denominator.get()));
    virtual_time.saturating_add(u64::try_from(steps).unwrap_or(u64::MAX))
}

/// First 60 Hz deadline strictly after the supplied elapsed time. Computing
/// from the epoch avoids accumulating the rounded 16,666,667 ns frame period.
pub(crate) fn next_frame(elapsed: Duration) -> Duration {
    let index = elapsed.as_nanos() * FRAME_HZ / NANOS_PER_SECOND + 1;
    let nanos = (index * NANOS_PER_SECOND).div_ceil(FRAME_HZ);
    Duration::new(
        (nanos / NANOS_PER_SECOND) as u64,
        (nanos % NANOS_PER_SECOND) as u32,
    )
}

/// Decide whether a visible, running presenter needs a redraw. Advancing the
/// deadline before requesting one prevents queue_redraw wakeups from spinning
/// even if a platform delays delivering the corresponding paint.
pub(crate) fn wake(
    now: Duration,
    deadline: Option<Duration>,
    running: bool,
) -> (bool, Option<Duration>) {
    if !running {
        return (false, None);
    }
    match deadline {
        Some(deadline) if now >= deadline => (true, Some(next_frame(now))),
        deadline => (false, deadline),
    }
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
    fn rational_wall_target_preserves_fractions_over_long_runs() {
        assert_eq!(pit_clocks(Duration::from_secs(12)), 14_318_180);
        let years = Duration::from_secs(365 * 24 * 60 * 60 * 100);
        assert_eq!(
            pit_clocks(years),
            u128::from(years.as_secs()) * 14_318_180 / 12
        );
        assert_eq!(pit_clocks(Duration::from_nanos(838)), 0);
        assert_eq!(pit_clocks(Duration::from_nanos(839)), 1);
        assert!(pit_clocks(Duration::MAX) > u128::from(u64::MAX));
        assert_eq!(
            pit_clocks(Duration::from_nanos(1_290_600_000_000)),
            1_539_920_259
        );
    }

    #[test]
    fn overload_drops_debt_once_and_an_ahead_guest_does_no_work() {
        let (target, dropped) = frame_target(Duration::from_secs(5), 0, 0);
        assert_eq!(u128::from(target), pit_clocks(MAX_GUEST_TIME));
        assert_eq!(
            u128::from(target) + dropped,
            pit_clocks(Duration::from_secs(5))
        );
        assert_eq!(
            frame_target(Duration::from_secs(5), target, dropped),
            (target, dropped)
        );
        let (next, debt) = frame_target(Duration::from_millis(5010), target, dropped);
        assert_eq!(debt, dropped);
        assert_eq!(
            u128::from(next),
            pit_clocks(Duration::from_millis(5010)) - dropped
        );
        assert_eq!(run_cap(10, 20, 100, rate(1, 1)), 100);
        assert_eq!(run_cap(0, 0, 0, rate(1, 1)), 0);
    }

    #[test]
    fn caps_respect_configured_rates_and_saturate_without_overflow() {
        assert_eq!(run_cap(11, 1, 7, rate(5, 2)), 32);
        assert_eq!(run_cap(12, 1, 7, rate(2, 5)), 12);
        assert_eq!(run_cap(u64::MAX, 0, 1, rate(u64::MAX, 1)), u64::MAX);
        assert_eq!(run_cap(1, 0, u64::MAX, rate(1, 1)), u64::MAX);
        assert_eq!(frame_target(Duration::MAX, u64::MAX, 0).0, u64::MAX);
        assert_eq!(run_cap((1 << 53) + 1, 0, 7, rate(1, 1)), (1 << 53) + 8);
    }

    #[test]
    fn wake_advances_once_then_waits_through_redraw_queue_wakeups() {
        let now = Duration::from_secs(2);
        let (redraw, deadline) = wake(now, Some(Duration::from_secs(1)), true);
        assert!(redraw);
        assert_eq!(deadline, Some(next_frame(now)));
        for elapsed in [now, now + Duration::from_nanos(1)] {
            assert_eq!(wake(elapsed, deadline, true), (false, deadline));
        }
        let due = deadline.unwrap();
        assert_eq!(wake(due, deadline, true), (true, Some(next_frame(due))));
        assert_eq!(wake(now, None, true), (false, None));
        assert_eq!(wake(now, deadline, false), (false, None));
    }

    #[test]
    fn frame_deadlines_are_future_and_do_not_accumulate_rounding() {
        assert_eq!(next_frame(Duration::ZERO), Duration::from_nanos(16_666_667));
        assert_eq!(
            next_frame(Duration::from_nanos(16_666_667)),
            Duration::from_nanos(33_333_334)
        );
        assert_eq!(
            next_frame(Duration::from_nanos(999_999_999)),
            Duration::from_secs(1)
        );
        assert_eq!(
            next_frame(Duration::from_secs(86400)),
            Duration::from_nanos(86_400_016_666_667)
        );
    }
}
