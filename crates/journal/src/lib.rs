// SPDX-License-Identifier: AGPL-3.0-or-later
//! Version-1 disk/CMOS journal format, recovery and sparse sector overlays.
//! Host-independent byte and record operations; see
//! `backlog/docs/design/persistent-media.md` § Chosen format and § Open and tail recovery,
//! and `backlog/docs/design/core-contract.md` §12.5.

#![forbid(unsafe_code)]

#[doc(hidden)]
pub mod codec;
#[doc(hidden)]
pub mod history;
mod overlay;
#[doc(hidden)]
pub mod scan;

pub use codec::{ClosingReason, Error, Frame, Header, Record, FRAME_LEN, HEADER_LEN, SECTOR_LEN};
pub use overlay::{SectorSource, SparseOverlay, VerifiedBase};
pub use scan::{
    scan, scan_overlay_reader, scan_reader, Generation, Refusal, Scan, SparseScan, Status, Tail,
    Tip, VerificationReport,
};

mod sink;
mod store;
pub use sink::{
    JournalMedia, MaintenanceFailure, OpenError, ReplaceRecord, SinkFailure, SinkStatus,
};
