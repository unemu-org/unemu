// SPDX-License-Identifier: AGPL-3.0-or-later
//! File session adapter; persistent-media.md § Integration.
use journal::{JournalSession, SinkFailure, SparseOverlay, VerificationReport, SECTOR_LEN};
impl JournalSession for crate::JournalMedia {
    fn set_retention(&mut self, keep: u32) {
        self.set_retention_policy(keep);
    }
    fn mount_report(&self) -> &VerificationReport {
        journal::JournalMedia::mount_report(std::ops::Deref::deref(self))
    }
    fn check_base(&self) {
        journal::JournalMedia::check_base(std::ops::Deref::deref(self))
    }
    fn read_sector(&self, lba: u64) -> [u8; SECTOR_LEN] {
        journal::JournalMedia::read_sector(std::ops::Deref::deref(self), lba)
    }
    fn write_batch(&mut self, sectors: &[(u64, [u8; SECTOR_LEN])]) {
        journal::JournalMedia::write_batch(std::ops::DerefMut::deref_mut(self), sectors)
    }
    fn write_protected(&self) -> bool {
        journal::JournalMedia::write_protected(std::ops::Deref::deref(self))
    }
    fn byte_len(&self) -> u64 {
        journal::JournalMedia::byte_len(std::ops::Deref::deref(self))
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
    fn fail_retention(self: Box<Self>, operation: &'static str) -> Box<dyn JournalSession> {
        Box::new((*self).with_failing_retention(operation))
    }
}
