// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host presentation policy for the AT timer; no host clock is read here.
//!
//! IBM Personal Computer AT Technical Reference (March 1984), System Board,
//! "System Timers" and "Oscillator": the timer input is the 14.31818 MHz
//! oscillator divided by twelve. See `backlog/docs/design/observation.md`
//! § Window pacing for rounding, overload and instruction-boundary limits.

use std::{num::NonZeroU64, time::Duration};

use device::clock::{PIT_HZ, TICKS_PER_PIT_CLOCK};
const NANOS_PER_SECOND: u128 = 1_000_000_000;
/// Sixty paints per second keeps input responsive without an uncapped redraw loop.
const FRAME_HZ: u128 = 60;
/// At 100%, at most 50 ms of guest time per paint bounds catch-up debt.
const MAX_GUEST_TIME: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
pub(crate) struct CpuRate {
    pub numerator: NonZeroU64,
    pub denominator: NonZeroU64,
}

/// Convert elapsed host time to virtual ticks on the exact board timebase.
pub(crate) fn virtual_ticks(elapsed: Duration) -> u128 {
    virtual_ticks_at_scale(elapsed, TICKS_PER_PIT_CLOCK)
}

fn virtual_ticks_at_scale(elapsed: Duration, ticks_per_pit: u64) -> u128 {
    elapsed
        .as_nanos()
        .checked_mul(u128::from(PIT_HZ.numerator()))
        .and_then(|n| n.checked_mul(u128::from(ticks_per_pit)))
        .map_or(u128::MAX, |n| {
            n / (u128::from(PIT_HZ.denominator()) * NANOS_PER_SECOND)
        })
}

/// Return an absolute target and the cumulative debt to discard permanently.
/// Keeping the debt separate preserves fractional clocks in the wall epoch.
pub(crate) fn frame_target(
    elapsed: Duration,
    clock: u64,
    dropped: u128,
    ratio: u32,
) -> (u64, u128) {
    let wall_target = virtual_ticks(elapsed.saturating_mul(ratio)).saturating_sub(dropped);
    let target =
        wall_target.min(u128::from(clock) + virtual_ticks(MAX_GUEST_TIME.saturating_mul(ratio)));
    (
        u64::try_from(target).unwrap_or(u64::MAX),
        dropped + wall_target - target,
    )
}

/// Pure conversion from a virtual-tick target to an absolute virtual-time run cap.
/// Ceiling avoids stalling at sub-instruction clock distances. The scheduler
/// owns carried active/HLT fractions; callers recheck its clock after each run.
pub(crate) fn run_cap(target: u64, clock: u64, virtual_time: u64, rate: CpuRate) -> u64 {
    let steps = (u128::from(target.saturating_sub(clock)) * u128::from(rate.numerator.get()))
        .div_ceil(u128::from(rate.denominator.get()) * u128::from(TICKS_PER_PIT_CLOCK));
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

/// Host-only acceleration; machine clock rates and input timestamps stay virtual.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Speed {
    #[default]
    Normal,
    Double,
    Quadruple,
    Unlimited,
}

impl Speed {
    pub const ALL: [Self; 4] = [Self::Normal, Self::Double, Self::Quadruple, Self::Unlimited];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "100%",
            Self::Double => "200%",
            Self::Quadruple => "400%",
            Self::Unlimited => "Unlimited",
        }
    }

    pub fn ratio(self) -> Option<u32> {
        match self {
            Self::Normal => Some(1),
            Self::Double => Some(2),
            Self::Quadruple => Some(4),
            Self::Unlimited => None,
        }
    }

    pub fn slice_budget(self, frame_period: Duration) -> Duration {
        if self == Self::Unlimited {
            frame_period.saturating_sub(Duration::from_millis(3))
        } else {
            frame_period
        }
    }

    pub fn audio_enabled(self) -> bool {
        self == Self::Normal
    }
}

/// Pure pacing state. Caller-supplied host durations never enter the machine.
#[derive(Default)]
pub(crate) struct Pacing {
    selected: Speed,
    held: bool,
    anchor_host: Duration,
    anchor_clock: u64,
    pub dropped: u128,
    samples: std::collections::VecDeque<(Duration, u64)>,
    last_frame: Option<Duration>,
}

impl Pacing {
    pub fn selected(&self) -> Speed {
        self.selected
    }

    pub fn effective(&self) -> Speed {
        if self.held {
            Speed::Unlimited
        } else {
            self.selected
        }
    }

    pub fn select(&mut self, speed: Speed, now: Duration, clock: u64) {
        if self.selected != speed {
            self.selected = speed;
            self.resync(now, clock);
        }
    }

    pub fn hold(&mut self, held: bool, now: Duration, clock: u64) {
        if self.held != held {
            self.held = held;
            self.resync(now, clock);
        }
    }

    pub fn resync(&mut self, now: Duration, clock: u64) {
        self.anchor_host = now;
        self.anchor_clock = clock;
        self.dropped = 0;
        self.last_frame = None;
        self.samples.clear();
        self.samples.push_back((now, clock));
    }

    pub fn target(&mut self, now: Duration, clock: u64) -> Option<u64> {
        let ratio = self.effective().ratio()?;
        let (target, dropped) = frame_target(
            now.saturating_sub(self.anchor_host),
            clock.saturating_sub(self.anchor_clock),
            self.dropped,
            ratio,
        );
        self.dropped = dropped;
        Some(self.anchor_clock.saturating_add(target))
    }

    /// Follow slower display cadences without turning a long stall into a long UI freeze.
    pub fn frame_period(&self, now: Duration) -> Duration {
        let minimum = next_frame(Duration::ZERO);
        self.last_frame.map_or(minimum, |last| {
            now.saturating_sub(last)
                .clamp(minimum, Duration::from_millis(50))
        })
    }

    pub fn slice_open(&self, spent: Duration, frame_period: Duration) -> bool {
        spent < self.effective().slice_budget(frame_period)
    }

    /// Short virtual chunks bound the delay before the next host-budget check.
    pub fn chunk_target(&self, clock: u64, target: Option<u64>) -> u64 {
        let chunk = clock.saturating_add(virtual_ticks(Duration::from_millis(1)) as u64);
        target.map_or(chunk, |target| target.min(chunk))
    }

    pub fn observe(&mut self, now: Duration, clock: u64) {
        self.last_frame = Some(now);
        self.samples.push_back((now, clock));
        let start = now.saturating_sub(Duration::from_secs(1));
        while self.samples.len() > 1 && self.samples[0].0 < start {
            self.samples.pop_front();
        }
    }

    pub fn status(&self) -> String {
        let mode = if self.held {
            "Fast-forward".into()
        } else if self.selected == Speed::Unlimited {
            "Unlimited".into()
        } else {
            format!("Speed {}", self.selected.label())
        };
        let measured = self
            .samples
            .front()
            .zip(self.samples.back())
            .and_then(|(first, last)| {
                let ticks = virtual_ticks(last.0.saturating_sub(first.0));
                (ticks > 0).then(|| last.1.saturating_sub(first.1) as f64 / ticks as f64)
            });
        measured.map_or_else(|| mode.clone(), |ratio| format!("{mode} {ratio:.1}x"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_modes_have_scaled_targets_and_bounded_host_budgets() {
        for mode in Speed::ALL {
            let mut pacing = Pacing::default();
            pacing.select(mode, Duration::ZERO, 0);
            for period in [
                next_frame(Duration::ZERO),
                Duration::from_millis(20),
                Duration::from_millis(33),
            ] {
                let budget = mode.slice_budget(period);
                assert!(pacing.slice_open(Duration::from_millis(8), period));
                assert!(pacing.slice_open(budget - Duration::from_nanos(1), period));
                assert!(!pacing.slice_open(budget, period));
                if mode.ratio().is_some() {
                    assert!(pacing.slice_open(period - Duration::from_nanos(1), period));
                } else {
                    assert!(budget < period);
                }
            }
            let target = pacing.target(Duration::from_millis(10), 0);
            assert_eq!(
                target,
                mode.ratio()
                    .map(|ratio| virtual_ticks(Duration::from_millis(10) * ratio) as u64)
            );
            assert_eq!(mode.audio_enabled(), mode == Speed::Normal);
            if let Some(ratio) = mode.ratio() {
                assert_eq!(
                    pacing.target(Duration::from_secs(10), 0),
                    Some(virtual_ticks(MAX_GUEST_TIME * ratio) as u64)
                );
            }
        }
    }

    #[test]
    fn speed_frame_budget_tracks_display_cadence_and_resets_after_pause() {
        let mut pacing = Pacing::default();
        let initial = pacing.frame_period(Duration::ZERO);
        pacing.observe(Duration::ZERO, 0);
        assert_eq!(
            pacing.frame_period(Duration::from_millis(20)),
            Duration::from_millis(20)
        );
        assert_eq!(
            pacing.frame_period(Duration::from_nanos(33_333_333)),
            Duration::from_nanos(33_333_333)
        );
        assert_eq!(
            pacing.frame_period(Duration::from_secs(1)),
            Duration::from_millis(50)
        );
        pacing.resync(Duration::from_secs(1), 123);
        assert_eq!(pacing.frame_period(Duration::from_secs(1)), initial);
        assert!(
            Speed::Unlimited.slice_budget(Duration::from_millis(33))
                > Speed::Unlimited.slice_budget(initial)
        );
        assert_eq!(
            Speed::Unlimited.slice_budget(Duration::from_millis(2)),
            Duration::ZERO
        );
    }

    #[test]
    fn speed_switches_reanchor_without_catch_up() {
        for before in Speed::ALL {
            for after in Speed::ALL.into_iter().filter(|after| *after != before) {
                let mut pacing = Pacing::default();
                pacing.select(before, Duration::ZERO, 0);
                let now = Duration::from_secs(120);
                let clock = virtual_ticks(Duration::from_secs(20)) as u64;
                pacing.select(after, now, clock);
                assert_eq!(pacing.target(now, clock), after.ratio().map(|_| clock));
                assert_eq!(pacing.target(now + Duration::from_millis(1), clock),
                    after.ratio().map(|ratio| clock + virtual_ticks(Duration::from_millis(1) * ratio) as u64));
                pacing.resync(now + Duration::from_secs(3600), clock);
                assert_eq!(
                    pacing.target(now + Duration::from_secs(3600), clock),
                    after.ratio().map(|_| clock)
                );
            }
        }
    }

    #[test]
    fn speed_unlimited_yields_between_chunks_and_waits_for_the_next_frame() {
        let mut pacing = Pacing::default();
        pacing.select(Speed::Unlimited, Duration::ZERO, 0);
        assert_eq!(pacing.target(Duration::MAX, 100), None);
        assert_eq!(
            pacing.chunk_target(100, None),
            100 + virtual_ticks(Duration::from_millis(1)) as u64
        );
        assert_eq!(pacing.chunk_target(100, Some(101)), 101);
        assert_eq!(pacing.chunk_target(u64::MAX, None), u64::MAX);
        assert!(!pacing.slice_open(Duration::from_secs(1), next_frame(Duration::ZERO)));
        let (paint, next) = wake(Duration::ZERO, Some(Duration::ZERO), true);
        assert!(paint);
        assert_eq!(wake(Duration::from_millis(4), next, true), (false, next));
    }

    #[test]
    fn speed_hold_restores_each_selected_mode() {
        for selected in Speed::ALL {
            let mut pacing = Pacing::default();
            assert_eq!(pacing.effective(), Speed::Normal);
            pacing.select(selected, Duration::ZERO, 0);
            pacing.hold(true, Duration::from_secs(1), 12);
            pacing.hold(true, Duration::from_secs(2), 12);
            assert_eq!(pacing.selected(), selected);
            assert_eq!(pacing.effective(), Speed::Unlimited);
            pacing.hold(false, Duration::from_secs(3), 25);
            assert_eq!(pacing.effective(), selected);
            assert_eq!(
                pacing.target(Duration::from_secs(3), 25),
                selected.ratio().map(|_| 25)
            );
        }
    }

    #[test]
    fn speed_status_formats_modes_and_measures_only_the_last_second() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.status(), "Speed 100%");
        pacing.resync(Duration::ZERO, 0);
        let one_second = virtual_ticks(Duration::from_secs(1)) as u64;
        pacing.observe(Duration::from_secs(1), one_second);
        assert_eq!(pacing.status(), "Speed 100% 1.0x");
        pacing.select(Speed::Double, Duration::from_secs(1), one_second);
        pacing.observe(Duration::from_secs(2), 3 * one_second);
        assert_eq!(pacing.status(), "Speed 200% 2.0x");
        pacing.select(Speed::Quadruple, Duration::from_secs(2), 3 * one_second);
        assert_eq!(pacing.status(), "Speed 400%");
        pacing.hold(true, Duration::from_secs(3), 3 * one_second);
        pacing.observe(
            Duration::from_secs(4),
            3 * one_second + virtual_ticks(Duration::from_millis(3800)) as u64,
        );
        assert_eq!(pacing.status(), "Fast-forward 3.8x");
        pacing.hold(false, Duration::from_secs(5), 0);
        pacing.select(Speed::Unlimited, Duration::from_secs(5), 0);
        pacing.observe(
            Duration::from_secs(6),
            virtual_ticks(Duration::from_millis(9600)) as u64,
        );
        assert_eq!(pacing.status(), "Unlimited 9.6x");
        let clock = virtual_ticks(Duration::from_millis(9600)) as u64;
        pacing.observe(Duration::from_millis(6500), clock + one_second / 2);
        pacing.observe(Duration::from_secs(7), clock + one_second);
        assert_eq!(pacing.status(), "Unlimited 1.0x");
    }

    fn rate(a: u64, b: u64) -> CpuRate {
        CpuRate {
            numerator: NonZeroU64::new(a).unwrap(),
            denominator: NonZeroU64::new(b).unwrap(),
        }
    }

    #[test]
    fn rational_wall_target_preserves_fractions_over_long_runs() {
        let reference =
            |nanos: u128| nanos * 13_125_000 * u128::from(TICKS_PER_PIT_CLOCK) / 11_000_000_000;
        assert_eq!(
            virtual_ticks(Duration::from_secs(12)),
            reference(12_000_000_000)
        );
        let years = Duration::from_secs(365 * 24 * 60 * 60 * 100);
        assert_eq!(virtual_ticks(years), reference(years.as_nanos()));
        assert_eq!(virtual_ticks(Duration::from_nanos(838)), reference(838));
        assert_eq!(virtual_ticks(Duration::from_nanos(839)), reference(839));
        assert!(virtual_ticks(Duration::MAX) > u128::from(u64::MAX));
        assert_eq!(
            virtual_ticks(Duration::from_nanos(1_290_600_000_000)),
            reference(1_290_600_000_000)
        );
    }

    #[test]
    fn overload_drops_debt_once_and_an_ahead_guest_does_no_work() {
        let (target, dropped) = frame_target(Duration::from_secs(5), 0, 0, 1);
        assert_eq!(u128::from(target), virtual_ticks(MAX_GUEST_TIME));
        assert_eq!(
            u128::from(target) + dropped,
            virtual_ticks(Duration::from_secs(5))
        );
        assert_eq!(
            frame_target(Duration::from_secs(5), target, dropped, 1),
            (target, dropped)
        );
        let (next, debt) = frame_target(Duration::from_millis(5010), target, dropped, 1);
        assert_eq!(debt, dropped);
        assert_eq!(
            u128::from(next),
            virtual_ticks(Duration::from_millis(5010)) - dropped
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
        assert_eq!(frame_target(Duration::MAX, u64::MAX, 0, 1).0, u64::MAX);
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
    #[test]
    fn clock_domain_wall_conversion_saturates_without_wrapping() {
        let result = std::panic::catch_unwind(|| virtual_ticks_at_scale(Duration::MAX, u64::MAX));
        assert_eq!(result.ok(), Some(u128::MAX));
        assert_eq!(
            virtual_ticks(Duration::from_secs(11)),
            13_125_000 * u128::from(TICKS_PER_PIT_CLOCK)
        );
    }
}
