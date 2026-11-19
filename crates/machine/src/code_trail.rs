// SPDX-License-Identifier: AGPL-3.0-or-later
//! The code trail: where the CPU stood at the last run-slice boundaries, for
//! a diagnostic report of a running machine (§3.1).
//!
//! The machine loop hands control to the CPU in slices: each
//! [`cpu::Cpu::run_steps`] call ends at a port or MMIO access, a HLT, a
//! device deadline, an interrupt becoming acceptable or a step budget. When
//! the trail is enabled ([`crate::Machine::enable_code_trail`]), the loop
//! records the CPU's CS:IP after every slice that returned. Consecutive
//! samples at the same CS:IP fold into one entry with a count, so a loop
//! that polls one port fills one entry, not the whole ring. The ring keeps
//! the last [`CODE_TRAIL_LEN`] entries.
//!
//! The trail lives in the machine, outside the CPU's execution path: the
//! sample reads two registers the CPU already holds after it returned, and
//! the CPU never sees it. Retired counts, virtual time and every guest
//! effect are the same with the trail on or off. A disabled trail costs one
//! branch per slice. See `backlog/docs/design/observation.md` §Diagnostic
//! report of a running machine.

/// How many entries the trail keeps.
pub const CODE_TRAIL_LEN: usize = 64;

/// One trail entry: a CS:IP and how many consecutive slice boundaries
/// found the CPU there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrailEntry {
    /// Visible code selector.
    pub cs: u16,
    /// Instruction pointer: where execution continues.
    pub ip: u16,
    /// Consecutive samples at this CS:IP, at least 1.
    pub samples: u32,
}

/// A ring of the last [`CODE_TRAIL_LEN`] trail entries.
#[derive(Clone, Debug)]
pub struct CodeTrail {
    entries: [TrailEntry; CODE_TRAIL_LEN],
    /// Index of the newest entry.
    newest: usize,
    len: usize,
    /// Every sample taken, folded or not.
    samples: u64,
}

impl Default for CodeTrail {
    fn default() -> Self {
        Self {
            entries: [TrailEntry::default(); CODE_TRAIL_LEN],
            newest: 0,
            len: 0,
            samples: 0,
        }
    }
}

impl CodeTrail {
    /// Record one sample: fold it into the newest entry when the CPU still
    /// stands there, else start a new entry, dropping the oldest when full.
    #[inline]
    pub fn record(&mut self, cs: u16, ip: u16) {
        self.samples = self.samples.wrapping_add(1);
        if self.len != 0 {
            let newest = &mut self.entries[self.newest];
            if newest.cs == cs && newest.ip == ip {
                newest.samples = newest.samples.saturating_add(1);
                return;
            }
            self.newest = (self.newest + 1) % CODE_TRAIL_LEN;
        }
        self.entries[self.newest] = TrailEntry { cs, ip, samples: 1 };
        self.len = (self.len + 1).min(CODE_TRAIL_LEN);
    }

    /// The kept entries, oldest first.
    pub fn entries(&self) -> Vec<TrailEntry> {
        let oldest = (self.newest + CODE_TRAIL_LEN + 1 - self.len) % CODE_TRAIL_LEN;
        (0..self.len)
            .map(|at| self.entries[(oldest + at) % CODE_TRAIL_LEN])
            .collect()
    }

    /// Every sample taken since the trail was enabled, folded ones included.
    pub fn samples(&self) -> u64 {
        self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_samples_fold_and_the_ring_keeps_the_newest_entries() {
        let mut trail = CodeTrail::default();
        assert!(trail.entries().is_empty());
        trail.record(0xF000, 0x10);
        trail.record(0xF000, 0x10);
        trail.record(0xF000, 0x12);
        assert_eq!(
            trail.entries(),
            [
                TrailEntry {
                    cs: 0xF000,
                    ip: 0x10,
                    samples: 2
                },
                TrailEntry {
                    cs: 0xF000,
                    ip: 0x12,
                    samples: 1
                },
            ]
        );
        for ip in 0..100u16 {
            trail.record(0x0008, ip);
        }
        let entries = trail.entries();
        assert_eq!(entries.len(), CODE_TRAIL_LEN);
        assert_eq!(entries.first().map(|entry| entry.ip), Some(100 - 64));
        assert_eq!(entries.last().map(|entry| entry.ip), Some(99));
        assert_eq!(trail.samples(), 103);
    }
}
