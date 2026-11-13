// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allocation-bounded large-image lifecycle; persistent-media.md § Memory and hashing.

use media::{
    offline::{Access, JournalTool},
    JournalMedia, SectorSource, SinkStatus, VerifiedBase,
};
use sha2::{Digest, Sha256};
use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            add(size);
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sparse_half_gib_lifecycle_and_small_streamed_export_are_bounded() {
    let directory =
        Directory(std::env::temp_dir().join(format!("media-large-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let path = directory.0.join("base.img");
    let journal = directory.0.join("disk.upfj");
    let output = directory.0.join("export.img");
    let mut file = File::create(&path).unwrap();
    let size = 512 * 1024 * 1024;
    if let Err(error) = file.set_len(size) {
        eprintln!("SKIP: filesystem cannot create a sparse 512 MiB base: {error}");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata().unwrap().blocks() * 512 > size / 8 {
            eprintln!("SKIP: filesystem allocated the sparse base densely");
            return;
        }
    }
    file.seek(SeekFrom::Start(size - 512)).unwrap();
    file.write_all(&[0x42; 512]).unwrap();
    drop(file);
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let base = VerifiedBase::open(&path).unwrap();
    let mut disk = JournalMedia::open_verified(&journal, &base, size, SinkStatus::default())
        .unwrap()
        .with_retention(2);
    assert_eq!(disk.written_sectors(), 0);
    assert_eq!(disk.read_sector(size / 512 - 1), [0x42; 512]);
    for generation in 1..=4 {
        disk.write_batch(&[(7, [generation; 512])]);
        disk.checkpoint().unwrap();
    }
    assert_eq!(disk.written_sectors(), 1);
    let finished = disk.finish_overlay().unwrap();
    assert_eq!(finished.read_sector(7).unwrap(), [4; 512]);
    drop(finished);
    let disk = JournalMedia::open_verified(&journal, &base, size, SinkStatus::default()).unwrap();
    assert_eq!(disk.written_sectors(), 1);
    assert_eq!(disk.read_sector(7), [4; 512]);
    drop(disk);
    let mut tool = JournalTool::open(&path, &journal, Access::Mutate).unwrap();
    let scan = tool.verify().unwrap();
    assert_eq!(scan.overlay.unwrap().written_sectors(), 1);
    let scan = tool.rollback(4, &directory.0.join("backup.upfj")).unwrap();
    assert_eq!(scan.overlay.unwrap().read_sector(7).unwrap(), [4; 512]);
    let unchanged = OpenOptions::new().read(true).open(&path).unwrap();
    assert_eq!(unchanged.metadata().unwrap().len(), size);
    assert_eq!(base.read_sector(7).unwrap(), [0; 512]);
    assert_eq!(base.read_sector(size / 512 - 1).unwrap(), [0x42; 512]);
    drop(tool);
    // Exercise publication with a smaller image; the large case above proves
    // mount/replay/retention bounds without writing a dense half-GiB temporary.
    let small_size = 8 * 1024 * 1024;
    let small_path = directory.0.join("small.img");
    let small_journal = directory.0.join("small.upfj");
    let mut file = File::create(&small_path).unwrap();
    file.set_len(small_size).unwrap();
    file.seek(SeekFrom::Start(small_size - 512)).unwrap();
    file.write_all(&[0x42; 512]).unwrap();
    drop(file);
    let small_base = VerifiedBase::open(&small_path).unwrap();
    let mut disk = JournalMedia::open_verified(
        &small_journal,
        &small_base,
        small_size,
        SinkStatus::default(),
    )
    .unwrap();
    disk.write_batch(&[(7, [4; 512])]);
    disk.finish().unwrap();
    let mut small_tool = JournalTool::open(&small_path, &small_journal, Access::Inspect).unwrap();
    small_tool.commit(1, &output).unwrap();
    let mut exported = File::open(output).unwrap();
    assert_eq!(exported.metadata().unwrap().len(), small_size);
    let mut actual = Sha256::new();
    let mut expected = Sha256::new();
    let mut chunk = [0; 64 * 1024];
    let mut reference = [0; 64 * 1024];
    for offset in (0..small_size).step_by(chunk.len()) {
        exported.read_exact(&mut chunk).unwrap();
        actual.update(chunk);
        reference.fill(0);
        for (index, sector) in reference.as_chunks_mut::<512>().0.iter_mut().enumerate() {
            let lba = offset / 512 + index as u64;
            if lba == 7 {
                sector.fill(4);
            }
            if lba == small_size / 512 - 1 {
                sector.fill(0x42);
            }
        }
        expected.update(reference);
    }
    assert_eq!(actual.finalize(), expected.finalize());
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    eprintln!("512 MiB lifecycle / 8 MiB export peak additional live allocation: {peak} bytes");
    assert!(PEAK.load(Ordering::Relaxed).saturating_sub(baseline) < 4 * 1024 * 1024);
}
