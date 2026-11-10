// SPDX-License-Identifier: AGPL-3.0-or-later
//! Version-1 floppy journal codec, recovery scanner and host-side durable sink.
//!
//! An immutable, SHA-256-bound base image plus checksummed sector replacements
//! preserves command-complete recovery points without copying a floppy per write.
//! Fixed 640-byte frames let recovery inspect later generation markers without
//! searching guest data for magic strings. Replacements become visible only at
//! COMMAND_END; a retention anchor must also have its GENERATION_END.
//!
//! Format and fault assumptions: `backlog/docs/design/persistent-media.md`,
//! § Chosen format (byte layout), § Retention anchor and § Open and tail recovery.
//! `scan_reader` streams payload history with `Read` + `Seek`; `scan` also accepts
//! an in-memory slice. `JournalMedia` owns memory, a stable exclusive sidecar lock
//! and synchronous append/recovery. Assembly supplies the device-trait adapter;
//! devices, frontend policy, retention rewrites and offline tools remain outside
//! this crate. This models a disk format, not controller hardware; complete
//! 512-byte replacement sectors include controller-supplied zero padding.
//! A valid checksum does not establish guest filesystem health or prove a host
//! sync was acknowledged.

mod codec;
mod scan;
mod sink;

pub use codec::{ClosingReason, Error, Frame, Header, Record, FRAME_LEN, HEADER_LEN, SECTOR_LEN};
pub use scan::{
    scan, scan_reader, Generation, Refusal, Scan, Status, Tail, Tip, VerificationReport,
};

pub use sink::{JournalMedia, OpenError, SinkFailure, SinkStatus};
