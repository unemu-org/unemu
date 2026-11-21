// SPDX-License-Identifier: AGPL-3.0-or-later
//! Journal sessions supplied by a mount; persistent-media.md § Integration.
use crate::{SinkFailure, SparseOverlay, VerificationReport, SECTOR_LEN};
/// A journal lifecycle with sector access and explicit boundary persistence.
/// Host adapters retain their record publication policy behind this interface.
pub trait JournalSession {
    fn set_retention(&mut self, keep: u32);
    fn mount_report(&self) -> &VerificationReport;
    fn check_base(&self);
    fn read_sector(&self, lba: u64) -> [u8; SECTOR_LEN];
    fn write_batch(&mut self, sectors: &[(u64, [u8; SECTOR_LEN])]);
    fn write_protected(&self) -> bool;
    fn byte_len(&self) -> u64;
    fn checkpoint(&mut self) -> Result<(), SinkFailure>;
    fn finish_readable(self: Box<Self>) -> (SparseOverlay, Result<(), SinkFailure>);
    #[cfg(feature = "test-support")]
    fn fail_output(self: Box<Self>) -> Box<dyn JournalSession>;
    /// File retention faults are separate from faults supplied by record storage.
    #[cfg(feature = "test-support")]
    fn fail_retention(self: Box<Self>, operation: &'static str) -> Box<dyn JournalSession>;
}
impl JournalSession for crate::JournalMedia {
    fn set_retention(&mut self, keep: u32) {
        self.set_retention_policy(keep);
    }
    fn mount_report(&self) -> &VerificationReport {
        crate::JournalMedia::mount_report(self)
    }
    fn check_base(&self) {
        crate::JournalMedia::check_base(self)
    }
    fn read_sector(&self, lba: u64) -> [u8; SECTOR_LEN] {
        crate::JournalMedia::read_sector(self, lba)
    }
    fn write_batch(&mut self, sectors: &[(u64, [u8; SECTOR_LEN])]) {
        crate::JournalMedia::write_batch(self, sectors)
    }
    fn write_protected(&self) -> bool {
        crate::JournalMedia::write_protected(self)
    }
    fn byte_len(&self) -> u64 {
        crate::JournalMedia::byte_len(self)
    }
    fn checkpoint(&mut self) -> Result<(), SinkFailure> {
        crate::JournalMedia::checkpoint(self)
    }
    fn finish_readable(self: Box<Self>) -> (SparseOverlay, Result<(), SinkFailure>) {
        crate::JournalMedia::finish_readable(*self)
    }
    #[cfg(feature = "test-support")]
    fn fail_output(self: Box<Self>) -> Box<dyn JournalSession> {
        Box::new((*self).with_failing_output())
    }
    #[cfg(feature = "test-support")]
    fn fail_retention(self: Box<Self>, _operation: &'static str) -> Box<dyn JournalSession> {
        // A record's storage supplies replacement faults, independently of files.
        self
    }
}
