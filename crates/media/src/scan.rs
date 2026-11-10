// SPDX-License-Identifier: AGPL-3.0-or-later
//! Command-atomic replay and fixed-stride recovery; persistent-media.md § Open and
//! tail recovery. This scanner never changes its inputs or performs host I/O.
//! Its byte-slice input requires the entire journal to be resident in memory,
//! including history that can grow until retention. A streaming `Read` + `Seek`
//! variant is deferred to the host-file sink.

use crate::codec::intact_generation_end;
use crate::{ClosingReason, Error, Frame, Header, Record, FRAME_LEN, HEADER_LEN, SECTOR_LEN};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    pub id: u64,
    pub reason: ClosingReason,
    /// Exclusive end offset, suitable for selecting a validated closed prefix.
    pub end_offset: usize,
    pub commands: u64,
    pub sectors: u64,
    pub last_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tip {
    pub generation: u64,
    pub commands: u64,
    pub sectors: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tail {
    pub failure_offset: usize,
    pub reason: Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub failure_offset: usize,
    pub reason: Error,
    pub later_generation_end: Option<usize>,
    pub incomplete_anchor: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Valid,
    /// Under the append-crash assumption, not proof of the discarded bytes' cause.
    TornTail(Tail),
    Refused(Refusal),
}

/// Read-only evidence and proposed recovery actions, not a durability receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationReport {
    pub header: Option<Header>,
    pub status: Status,
    pub generations: Vec<Generation>,
    pub unsealed_tip: Option<Tip>,
    /// No prefix is mountable on refusal, even if diagnostics describe earlier data.
    pub safe_offset: usize,
    pub discarded_bytes: usize,
    /// Last command/generation marker in the retained prefix. Host acknowledgments
    /// cannot be inferred from file contents; a pending SECTOR never advances this.
    pub last_durable_sequence: u64,
    /// Generation zero is available only without a retention anchor.
    pub last_closed_generation: Option<u64>,
}

impl VerificationReport {
    pub fn needs_recovery(&self) -> bool {
        matches!(self.status, Status::TornTail(_))
            || (matches!(self.status, Status::Valid) && self.unsealed_tip.is_some())
    }

    /// Proposed reason-3 marker after truncation to `safe_offset`. The caller must
    /// perform locking, truncation and durability operations before using it.
    pub fn recovery_seal(&self) -> Result<Option<Frame>, Error> {
        if matches!(self.status, Status::Refused(_)) {
            return Ok(None);
        }
        self.unsealed_tip
            .as_ref()
            .map(|tip| {
                Ok(Frame {
                    sequence: self
                        .last_durable_sequence
                        .checked_add(1)
                        .ok_or(Error::IntegerExhausted)?,
                    generation: tip.generation,
                    record: Record::GenerationEnd {
                        reason: ClosingReason::Recovered,
                    },
                })
            })
            .transpose()
    }
}

/// The reconstructed command-complete image is withheld on every refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    pub report: VerificationReport,
    pub image: Option<Vec<u8>>,
}

/// Validate a journal against the exact loaded base and configured image length.
/// Valid unsealed commands are replayed; pending commands and torn suffixes are
/// excluded. No bytes are repaired or sealed by this function.
pub fn scan(journal: &[u8], base: &[u8], geometry_length: u64) -> Scan {
    let mut report = VerificationReport {
        header: None,
        status: Status::Valid,
        generations: Vec::new(),
        unsealed_tip: None,
        safe_offset: 0,
        discarded_bytes: 0,
        last_durable_sequence: 0,
        last_closed_generation: None,
    };
    let header_bytes = &journal[..journal.len().min(HEADER_LEN)];
    let header = match Header::decode(header_bytes, base, geometry_length) {
        Ok(header) => header,
        Err(reason) => {
            report.status = Status::Refused(Refusal {
                failure_offset: 0,
                reason,
                later_generation_end: None,
                incomplete_anchor: false,
            });
            return Scan {
                report,
                image: None,
            };
        }
    };
    let mut state = State::new(&header, base);
    if header.anchor_generation == 0 {
        report.safe_offset = HEADER_LEN;
        report.last_closed_generation = Some(0);
    }
    report.header = Some(header);
    let mut offset = HEADER_LEN;
    for bytes in journal[HEADER_LEN..].chunks(FRAME_LEN) {
        let outcome = if bytes.len() != FRAME_LEN {
            Err(Error::PartialFrame)
        } else {
            Frame::decode(bytes)
                .and_then(|frame| state.accept(frame, offset + FRAME_LEN, &mut report))
        };
        if let Err(reason) = outcome {
            classify_failure(journal, offset, reason, &state, &mut report);
            return finish(state, report);
        }
        offset += FRAME_LEN;
    }
    if state.anchor_pending {
        classify_failure(
            journal,
            offset,
            Error::Anchor("incomplete anchor"),
            &state,
            &mut report,
        );
    } else if !state.pending.is_empty() {
        classify_failure(journal, offset, Error::PendingCommand, &state, &mut report);
    }
    finish(state, report)
}

fn classify_failure(
    journal: &[u8],
    offset: usize,
    reason: Error,
    state: &State,
    report: &mut VerificationReport,
) {
    let later_generation_end = journal[offset..]
        .chunks(FRAME_LEN)
        .enumerate()
        .skip(1)
        .find(|(_, bytes)| intact_generation_end(bytes))
        .map(|(index, _)| offset + index * FRAME_LEN);
    if state.anchor_pending || later_generation_end.is_some() {
        report.status = Status::Refused(Refusal {
            failure_offset: offset,
            reason,
            later_generation_end,
            incomplete_anchor: state.anchor_pending,
        });
        report.safe_offset = 0;
    } else {
        report.status = Status::TornTail(Tail {
            failure_offset: offset,
            reason,
        });
        report.discarded_bytes = journal.len() - report.safe_offset;
    }
}

fn finish(state: State, mut report: VerificationReport) -> Scan {
    if state.commands != 0 {
        report.unsealed_tip = Some(Tip {
            generation: state.generation,
            commands: state.commands,
            sectors: state.sectors,
        });
    }
    let image = if matches!(report.status, Status::Refused(_)) {
        None
    } else {
        Some(state.image)
    };
    Scan { report, image }
}

struct State<'a> {
    base: &'a [u8],
    image: Vec<u8>,
    pending: BTreeMap<u64, Box<[u8; SECTOR_LEN]>>,
    sequence: u64,
    generation: u64,
    commands: u64,
    sectors: u64,
    anchor_pending: bool,
    anchor_command_ended: bool,
}

impl<'a> State<'a> {
    fn new(header: &Header, base: &'a [u8]) -> Self {
        Self {
            base,
            image: base.to_vec(),
            pending: BTreeMap::new(),
            sequence: 1,
            generation: if header.anchor_generation == 0 {
                1
            } else {
                header.anchor_generation
            },
            commands: 0,
            sectors: 0,
            anchor_pending: header.anchor_generation != 0,
            anchor_command_ended: false,
        }
    }

    fn accept(
        &mut self,
        frame: Frame,
        end_offset: usize,
        report: &mut VerificationReport,
    ) -> Result<(), Error> {
        if frame.sequence != self.sequence {
            return Err(Error::Sequence {
                expected: self.sequence,
                actual: frame.sequence,
            });
        }
        if frame.generation != self.generation {
            return Err(Error::Generation {
                expected: self.generation,
                actual: frame.generation,
            });
        }
        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Error::IntegerExhausted)?;
        match frame.record {
            Record::Sector { lba, bytes } => {
                if self.anchor_pending && self.anchor_command_ended {
                    return Err(Error::Anchor("more than one synthetic command"));
                }
                if lba >= (self.base.len() / SECTOR_LEN) as u64 {
                    return Err(Error::SectorBounds(lba));
                }
                if self.pending.contains_key(&lba) {
                    return Err(Error::DuplicateSector(lba));
                }
                if self.anchor_pending {
                    if self
                        .pending
                        .last_key_value()
                        .is_some_and(|(&last, _)| lba <= last)
                    {
                        return Err(Error::Anchor("sectors must be in increasing LBA order"));
                    }
                    let start = lba as usize * SECTOR_LEN;
                    if self.base[start..start + SECTOR_LEN] == *bytes {
                        return Err(Error::Anchor("unchanged base sector"));
                    }
                }
                self.pending.insert(lba, bytes);
            }
            Record::CommandEnd { sectors } => {
                if self.anchor_pending && self.anchor_command_ended {
                    return Err(Error::Anchor("more than one synthetic command"));
                }
                let expected = self.pending.len() as u64;
                if sectors != expected {
                    return Err(Error::CommandCount {
                        expected,
                        actual: sectors,
                    });
                }
                if sectors == 0 && !self.anchor_pending {
                    return Err(Error::EmptyCommand);
                }
                let commands = self
                    .commands
                    .checked_add(1)
                    .ok_or(Error::IntegerExhausted)?;
                let total_sectors = self
                    .sectors
                    .checked_add(sectors)
                    .ok_or(Error::IntegerExhausted)?;
                for (lba, bytes) in &self.pending {
                    let start = *lba as usize * SECTOR_LEN;
                    self.image[start..start + SECTOR_LEN].copy_from_slice(bytes.as_ref());
                }
                self.pending.clear();
                self.commands = commands;
                self.sectors = total_sectors;
                if self.anchor_pending {
                    self.anchor_command_ended = true;
                } else {
                    report.safe_offset = end_offset;
                    report.last_durable_sequence = frame.sequence;
                }
            }
            Record::GenerationEnd { reason } => {
                if !self.pending.is_empty() {
                    return Err(Error::GenerationInsideCommand);
                }
                if self.anchor_pending {
                    if !self.anchor_command_ended || reason != ClosingReason::RetentionAnchor {
                        return Err(Error::Anchor("missing synthetic command or anchor reason"));
                    }
                } else if reason == ClosingReason::RetentionAnchor {
                    return Err(Error::Anchor("anchor reason outside initial anchor"));
                }
                let next_generation = self
                    .generation
                    .checked_add(1)
                    .ok_or(Error::IntegerExhausted)?;
                report.generations.push(Generation {
                    id: self.generation,
                    reason,
                    end_offset,
                    commands: self.commands,
                    sectors: self.sectors,
                    last_sequence: frame.sequence,
                });
                report.last_closed_generation = Some(self.generation);
                report.safe_offset = end_offset;
                report.last_durable_sequence = frame.sequence;
                self.anchor_pending = false;
                self.commands = 0;
                self.sectors = 0;
                self.generation = next_generation;
            }
        }
        self.sequence = next_sequence;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_counters_never_wrap() {
        let base = [0; SECTOR_LEN];
        let header = Header::new(&base, 0).unwrap();
        let mut report = scan(&header.encode().unwrap(), &base, SECTOR_LEN as u64).report;
        let mut state = State::new(&header, &base);
        state.sequence = u64::MAX;
        let mut frame = Frame {
            sequence: u64::MAX,
            generation: 1,
            record: Record::GenerationEnd {
                reason: ClosingReason::Checkpoint,
            },
        };
        assert_eq!(
            state.accept(frame.clone(), 768, &mut report),
            Err(Error::IntegerExhausted)
        );
        state.sequence = 1;
        state.generation = u64::MAX;
        frame.sequence = 1;
        frame.generation = u64::MAX;
        assert_eq!(
            state.accept(frame, 768, &mut report),
            Err(Error::IntegerExhausted)
        );
        state.generation = 1;
        state.commands = u64::MAX;
        state.pending.insert(0, Box::new([1; SECTOR_LEN]));
        let frame = Frame {
            sequence: 1,
            generation: 1,
            record: Record::CommandEnd { sectors: 1 },
        };
        assert_eq!(
            state.accept(frame.clone(), 768, &mut report),
            Err(Error::IntegerExhausted)
        );
        state.commands = 0;
        state.sectors = u64::MAX;
        assert_eq!(
            state.accept(frame, 768, &mut report),
            Err(Error::IntegerExhausted)
        );
    }
}
