// SPDX-License-Identifier: AGPL-3.0-or-later
//! File-backed disk/CMOS journals and offline tools; see
//! `backlog/docs/design/persistent-media.md` § Offline commands and
//! `backlog/docs/design/core-contract.md` §12.5. The byte format and sector
//! replay and the record lifecycle belong to `journal`; file publication,
//! replacement and cleanup use the shared file engine. Failed appends are
//! rolled back; the journal latches the failure and stops output.

#[cfg(test)]
use journal::codec;
use journal::history;
mod journal_file;
pub mod offline;
#[doc(hidden)]
pub mod overlay;
mod sink;
#[cfg(test)]
mod temporary;

pub use journal::{
    scan, scan_overlay_reader, scan_reader, Generation, Refusal, Scan, SparseScan, Status, Tail,
    Tip, VerificationReport,
};
pub use journal::{ClosingReason, Error, Frame, Header, Record, FRAME_LEN, HEADER_LEN, SECTOR_LEN};
pub use journal_file::{check_journal_path, JournalFile};
pub use overlay::{SectorSource, SparseOverlay, VerifiedBase};
pub use sink::{JournalMedia, MaintenanceFailure, OpenError, SinkFailure, SinkStatus};
#[cfg(test)]
mod overlay_tests;
#[cfg(feature = "test-support")]
pub use overlay::{data_passes, hash_passes};

mod session;

pub mod create;
pub mod layer;
