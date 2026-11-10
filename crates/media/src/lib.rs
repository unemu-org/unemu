// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-independent version-1 floppy journal codec and read-only recovery scanner.
//!
//! An immutable, SHA-256-bound base image plus checksummed sector replacements
//! preserves command-complete recovery points without copying a floppy per write.
//! Fixed 640-byte frames let recovery inspect later generation markers without
//! searching guest data for magic strings. Replacements become visible only at
//! COMMAND_END; a retention anchor must also have its GENERATION_END.
//!
//! Format and fault assumptions: `backlog/docs/design/persistent-media.md`,
//! § Chosen format (byte layout), § Retention anchor and § Open and tail recovery.
//! The slice API requires the whole journal in memory alongside the base,
//! reconstructed image and one command buffer. History can grow until retention;
//! a streaming `Read` + `Seek` scanner is deferred to the host-file sink.
//! This crate models a disk format, not controller hardware. It accepts complete
//! 512-byte replacement sectors, including any controller-supplied zero padding.
//! Host files, locks, synchronization, truncation/sealing, retention rewrites,
//! devices, assembly and CLI policy are outside its scope. A valid checksum does
//! not establish guest filesystem health or prove a host sync was acknowledged.

mod codec;
mod scan;

pub use codec::{ClosingReason, Error, Frame, Header, Record, FRAME_LEN, HEADER_LEN, SECTOR_LEN};
pub use scan::{scan, Generation, Refusal, Scan, Status, Tail, Tip, VerificationReport};
