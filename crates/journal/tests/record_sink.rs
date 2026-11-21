// SPDX-License-Identifier: AGPL-3.0-or-later
//! The lifecycle over contract records only; persistent-media.md § Locking
//! and retention. No file adapter is linked by this test binary.

use journal::{ClosingReason, JournalMedia, SinkStatus, VerifiedBase, SECTOR_LEN};
use unemu_core_api::test_support::MemoryStorage;
use unemu_core_api::{Error, ErrorKind, Storage};

#[test]
fn retention_uses_the_supplied_record_replacement_and_preserves_closed_bytes_on_failure() {
    let storage = MemoryStorage::new();
    let base = VerifiedBase::from_bytes(&[0; SECTOR_LEN]).unwrap();
    let mut journal = JournalMedia::open_record(
        storage.open("disk").unwrap(),
        &base,
        SECTOR_LEN as u64,
        SinkStatus::default(),
    )
    .unwrap()
    .with_retention(1);
    journal.write_batch(&[(0, [7; SECTOR_LEN])]);
    journal.checkpoint().unwrap();
    journal.write_batch(&[(0, [9; SECTOR_LEN])]);
    let mut calls = 0;
    journal
        .close_with(ClosingReason::Checkpoint, &mut |_| {
            calls += 1;
            Err(Error::new(
                ErrorKind::Storage,
                "injected replacement refusal",
            ))
        })
        .unwrap();
    assert_eq!(calls, 1);
    let failure = journal.status().maintenance_failure().unwrap();
    assert!(!failure.uncertain);
    assert!(failure.message.contains("injected replacement refusal"));
    assert!(journal.status().failure().is_none());
    let before = storage.contents("disk").unwrap();
    assert_eq!(
        journal::scan(&before, &[0; SECTOR_LEN], SECTOR_LEN as u64)
            .image
            .unwrap(),
        [9; SECTOR_LEN]
    );
    journal.finish().unwrap();
    let bytes = storage.contents("disk").unwrap();
    let scan = journal::scan(&bytes, &[0; SECTOR_LEN], SECTOR_LEN as u64);
    assert_eq!(scan.report.generations.len(), 1);
    assert_eq!(scan.report.generations[0].id, 3);
    assert_eq!(scan.image.unwrap(), [9; SECTOR_LEN]);
}
