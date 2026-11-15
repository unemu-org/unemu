// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sparse replay equivalence and immutable-base checks; persistent-media.md § Memory and hashing.

use crate::*;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "media-overlay-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn verified_base_is_hashed_once_across_mount_replay_and_retention() {
    let dir = Directory::new();
    let path = dir.0.join("base.img");
    std::fs::write(&path, [0; 4096]).unwrap();
    crate::overlay::HASH_PASSES.with(|n| n.set(0));
    let base = VerifiedBase::open(&path).unwrap();
    let journal = dir.0.join("disk.upfj");
    let mut disk = JournalMedia::open_verified(&journal, &base, 4096, SinkStatus::default())
        .unwrap()
        .with_retention(1);
    for i in 1..=3 {
        disk.write_batch(&[(2, [i; 512])]);
        disk.checkpoint().unwrap();
    }
    disk.finish().unwrap();
    crate::overlay::HASH_PASSES.with(|n| assert_eq!(n.get(), 1));
    let base = VerifiedBase::open(&path).unwrap();
    let disk = JournalMedia::open_verified(&journal, &base, 4096, SinkStatus::default()).unwrap();
    assert_eq!(disk.read_sector(2), [3; 512]);
    crate::overlay::HASH_PASSES.with(|n| assert_eq!(n.get(), 2));
}

#[test]
fn changed_base_is_rejected_for_base_and_overlay_reads_writes_and_finish() {
    for truncate in [false, true] {
        let dir = Directory::new();
        let path = dir.0.join("base.img");
        std::fs::write(&path, [0; 4096]).unwrap();
        let base = VerifiedBase::open(&path).unwrap();
        let status = SinkStatus::default();
        let mut disk =
            JournalMedia::open_verified(&dir.0.join("disk.upfj"), &base, 4096, status.clone())
                .unwrap();
        disk.write_batch(&[(2, [7; 512])]);
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        if truncate {
            file.set_len(1024).unwrap();
        } else {
            file.seek(SeekFrom::Start(512)).unwrap();
            file.write_all(&[3; 512]).unwrap();
            // A coarse filesystem clock (Linux stamps files from the timer tick)
            // can give a same-size rewrite this soon the verified timestamps;
            // the check can only see a change that moved them.
            file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                .unwrap();
        }
        assert!(base.read_sector(0).is_err());
        assert_eq!(disk.read_sector(2), [0; 512]);
        assert!(status
            .failure()
            .unwrap()
            .message
            .contains("base file changed"));
        disk.write_batch(&[(3, [9; 512])]);
        assert!(disk.finish().is_err());
    }
}

#[test]
fn source_chain_reads_the_nearest_replacement() {
    let base = VerifiedBase::from_bytes(&[1; 2048]).unwrap();
    let mut first = SparseOverlay::new(std::rc::Rc::new(base));
    first.replace(1, Box::new([2; 512]));
    let mut second = SparseOverlay::new(std::rc::Rc::new(first));
    second.replace(2, Box::new([3; 512]));
    second.replace(1, Box::new([4; 512]));
    assert_eq!(second.read_sector(0).unwrap(), [1; 512]);
    assert_eq!(second.read_sector(1).unwrap(), [4; 512]);
    assert_eq!(second.read_sector(2).unwrap(), [3; 512]);
    assert_eq!(second.written_sectors(), 2);
}

#[test]
fn sparse_workloads_match_dense_reference_journal_bytes_replay_and_exports() {
    // The reference uses the former whole-image replacement algorithm and emits
    // its original frame order independently of sparse replay and retention.
    for seed in 1..=32u64 {
        let dir = Directory::new();
        let path = dir.0.join("base.img");
        let journal = dir.0.join("disk.upfj");
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            state
        };
        let mut dense = (0..32 * 512)
            .map(|_| (next() >> 32) as u8)
            .collect::<Vec<_>>();
        let original = dense.clone();
        std::fs::write(&path, &dense).unwrap();
        let base = VerifiedBase::open(&path).unwrap();
        let mut disk =
            JournalMedia::open_verified(&journal, &base, dense.len() as u64, SinkStatus::default())
                .unwrap()
                .with_retention(100);
        let mut expected = Header::new(&original, 0)
            .unwrap()
            .encode()
            .unwrap()
            .to_vec();
        let mut sequence = 0;
        let mut emit = |generation, record| {
            sequence += 1;
            expected.extend_from_slice(
                &Frame {
                    sequence,
                    generation,
                    record,
                }
                .encode()
                .unwrap(),
            );
        };
        let mut snapshots = Vec::new();
        for generation in 1..=4 {
            for _ in 0..32 {
                let mut batch = Vec::new();
                for _ in 0..1 + next() % 8 {
                    let lba = next() % 32;
                    let bytes = std::array::from_fn(|_| (next() >> 32) as u8);
                    dense[lba as usize * 512..(lba as usize + 1) * 512].copy_from_slice(&bytes);
                    batch.push((lba, bytes));
                }
                disk.write_batch(&batch);
                for lba in 0..32 {
                    assert_eq!(
                        disk.read_sector(lba).as_slice(),
                        &dense[lba as usize * 512..(lba as usize + 1) * 512]
                    );
                }
                let mut seen = std::collections::BTreeSet::new();
                let mut unique = batch
                    .iter()
                    .rev()
                    .filter(|(lba, _)| seen.insert(*lba))
                    .collect::<Vec<_>>();
                unique.reverse();
                for (lba, bytes) in &unique {
                    emit(
                        generation,
                        Record::Sector {
                            lba: *lba,
                            bytes: Box::new(*bytes),
                        },
                    );
                }
                emit(
                    generation,
                    Record::CommandEnd {
                        sectors: unique.len() as u64,
                    },
                );
            }
            disk.checkpoint().unwrap();
            emit(
                generation,
                Record::GenerationEnd {
                    reason: ClosingReason::Checkpoint,
                },
            );
            snapshots.push(dense.clone());
        }
        disk.finish().unwrap();
        emit(
            5,
            Record::GenerationEnd {
                reason: ClosingReason::SessionEnd,
            },
        );
        assert_eq!(std::fs::read(&journal).unwrap(), expected, "seed {seed}");
        let disk =
            JournalMedia::open_verified(&journal, &base, dense.len() as u64, SinkStatus::default())
                .unwrap();
        assert_eq!(disk.image(), dense, "seed {seed}");
        drop(disk);
        let mut tool =
            offline::JournalTool::open(&path, &journal, offline::Access::Inspect).unwrap();
        for (index, snapshot) in snapshots.iter().enumerate() {
            let output = dir.0.join(format!("export-{index}.img"));
            tool.commit(index as u64 + 1, &output).unwrap();
            assert_eq!(
                std::fs::read(output).unwrap(),
                *snapshot,
                "seed {seed}, generation {index}"
            );
        }
    }
}

#[test]
fn retained_generation_exports_match_dense_snapshots_after_base_restores() {
    for seed in 1..=16u64 {
        let dir = Directory::new();
        let path = dir.0.join("base.img");
        let journal = dir.0.join("disk.upfj");
        let mut random = seed;
        let mut next = || {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            random >> 32
        };
        let original = (0..16 * 512).map(|_| next() as u8).collect::<Vec<_>>();
        let mut dense = original.clone();
        std::fs::write(&path, &original).unwrap();
        let base = VerifiedBase::open(&path).unwrap();
        let mut snapshots = vec![original.clone()];
        for generation in 1..=8u64 {
            let status = SinkStatus::default();
            let mut disk =
                JournalMedia::open_verified(&journal, &base, original.len() as u64, status.clone())
                    .unwrap()
                    .with_retention(2);
            for _ in 0..8 {
                let mut batch = Vec::new();
                for _ in 0..1 + next() % 4 {
                    let lba = 1 + next() % 15;
                    let start = lba as usize * 512;
                    let bytes = if next() % 3 == 0 {
                        original[start..start + 512].try_into().unwrap()
                    } else {
                        std::array::from_fn(|_| next() as u8)
                    };
                    dense[start..start + 512].copy_from_slice(&bytes);
                    batch.push((lba, bytes));
                }
                disk.write_batch(&batch);
            }
            // The first retained anchor with sector zero restored must omit its
            // redundant entry while preserving exactly the same readable state.
            let bytes = if generation % 2 == 0 {
                original[..512].try_into().unwrap()
            } else {
                [0xA5; 512]
            };
            dense[..512].copy_from_slice(&bytes);
            disk.write_batch(&[(0, bytes)]);
            disk.checkpoint().unwrap();
            assert!(status.failure().is_none(), "seed {seed}");
            assert!(
                status.maintenance_failure().is_none(),
                "seed {seed}: {:?}",
                status.maintenance_failure()
            );
            assert_eq!(disk.image(), dense, "seed {seed}, generation {generation}");
            snapshots.push(dense.clone());
            drop(disk);
            let mut tool =
                offline::JournalTool::open(&path, &journal, offline::Access::Inspect).unwrap();
            let scan = tool.verify().unwrap();
            let retained = scan
                .report
                .generations
                .iter()
                .map(|g| g.id)
                .collect::<Vec<_>>();
            assert_eq!(
                retained,
                (generation.saturating_sub(1).max(1)..=generation).collect::<Vec<_>>()
            );
            for id in retained {
                let output = dir.0.join(format!("export-{generation}-{id}.img"));
                tool.commit(id, &output).unwrap();
                assert_eq!(
                    std::fs::read(output).unwrap(),
                    snapshots[id as usize],
                    "seed {seed}, retained {id} after {generation}"
                );
            }
        }
    }
}
