// SPDX-License-Identifier: AGPL-3.0-or-later
//! Version-1 floppy/IDE/CMOS journal codec, recovery scanner and host-side durable sink.
//!
//! An immutable, SHA-256-bound base image plus checksummed sector replacements
//! preserves command-complete recovery points without copying a floppy per write.
//! Fixed 640-byte frames let recovery inspect later generation markers without
//! searching guest data for magic strings. Replacements become visible only at
//! COMMAND_END; a retention anchor must also have its GENERATION_END.
//!
//! Format and fault assumptions: `backlog/docs/design/persistent-media.md`,
//! § Chosen format (byte layout), § Retention anchor and § Open and tail recovery.
//! `scan_overlay_reader` streams sparse replay with `Read` + `Seek`; `scan` also
//! accepts an in-memory slice for explicit full-image reconstruction.
//! `JournalMedia` owns a sparse overlay, a stable exclusive sidecar lock
//! and synchronous append/recovery. Assembly supplies the device-trait adapter;
//! devices and frontend policy remain outside this crate. This models a disk format, not controller hardware; complete
//! 512-byte replacement sectors include controller-supplied zero padding.
//! CMOS bases are exactly 64 bytes; the single block is zero padded to 512,
//! with an immutable clock/control prefix. See § CMOS NVRAM journals.
//! A valid checksum does not establish guest filesystem health or prove a host
//! sync was acknowledged.
//!
//! [`fat12`] builds new FAT12 diskette bases from host files and folders;
//! [`create`] publishes them. See machines/README.md § Preparing disks.

pub mod create;
pub mod fat12;
pub mod layer;
use journal_files::overlay;
pub use journal_files::{
    check_journal_path, offline, scan, scan_overlay_reader, scan_reader, ClosingReason, Error,
    Frame, Header, JournalFile, JournalMedia, MaintenanceFailure, OpenError, Record, Scan,
    SectorSource, SinkFailure, SinkStatus, SparseOverlay, Status, VerificationReport, VerifiedBase,
    FRAME_LEN, HEADER_LEN, SECTOR_LEN,
};
#[cfg(feature = "test-support")]
pub use journal_files::{data_passes, hash_passes};
