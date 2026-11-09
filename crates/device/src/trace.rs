// SPDX-License-Identifier: AGPL-3.0-or-later
//! Optional bounded access diagnostics, independent of hardware/output state.
//!
//! See `backlog/docs/design/observation.md`, §Traces and later audio. These
//! buffers observe serviced accesses; they never drive emulated hardware.

use crate::TraceEvent;
use std::collections::VecDeque;

/// Run-scoped access recording; output capture never enables it implicitly.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TracePolicy {
    /// Discard access events without allocating history.
    #[default]
    Off,
    /// Keep the newest `capacity` events, counting overwritten events.
    /// Zero capacity counts every event as dropped.
    Ring { capacity: usize },
}

/// One recorder shared by all device implementations and the open bus.
///
/// Draining preserves the cumulative, saturating drop count. Reconfiguration
/// and `clear` start a fresh history. Hardware snapshots exclude this buffer,
/// its policy and its counters; restoring hardware does not replay diagnostics.
#[derive(Debug, Default)]
pub struct TraceBuffer {
    policy: TracePolicy,
    events: VecDeque<TraceEvent>,
    dropped: u64,
}

impl TraceBuffer {
    /// Select a policy and discard the previous history and drop count.
    pub fn set_policy(&mut self, policy: TracePolicy) {
        *self = Self {
            policy,
            ..Self::default()
        };
    }

    /// The active recording policy.
    pub fn policy(&self) -> TracePolicy {
        self.policy
    }

    /// Record an access without changing guest-visible state.
    pub fn push(&mut self, event: TraceEvent) {
        let TracePolicy::Ring { capacity } = self.policy else {
            return;
        };
        if self.events.len() == capacity {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        if capacity != 0 {
            self.events.push_back(event);
        }
    }

    /// Drain retained events in access order, leaving the drop count intact.
    pub fn take(&mut self) -> Vec<TraceEvent> {
        self.events.drain(..).collect()
    }

    /// Events lost since configuration or explicit history reset.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Start a fresh history while preserving the selected policy.
    pub fn clear(&mut self) {
        self.events.clear();
        self.dropped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccessDir, VClock};

    fn event(value: u32) -> TraceEvent {
        TraceEvent::Port {
            at: VClock::ZERO,
            port: 0x80,
            size: 1,
            dir: AccessDir::Write,
            value,
        }
    }

    #[test]
    fn zero_capacity_counts_all_events_and_reconfiguration_clears_history() {
        let mut trace = TraceBuffer::default();
        trace.push(event(0));
        assert_eq!(trace.events.capacity(), 0);
        assert_eq!(trace.dropped(), 0);
        trace.set_policy(TracePolicy::Ring { capacity: 0 });
        for i in 0..5 {
            trace.push(event(i));
        }
        assert!(trace.take().is_empty());
        assert_eq!(trace.dropped(), 5);
        trace.set_policy(TracePolicy::Ring { capacity: 2 });
        for i in 0..5 {
            trace.push(event(i));
        }
        assert_eq!(trace.take(), vec![event(3), event(4)]);
        assert_eq!(trace.dropped(), 3);
        trace.push(event(5));
        assert_eq!(trace.take(), vec![event(5)]);
        assert_eq!(trace.dropped(), 3);
        trace.set_policy(TracePolicy::Off);
        assert_eq!(trace.events.capacity(), 0);
        assert_eq!(trace.dropped(), 0);
    }
}
