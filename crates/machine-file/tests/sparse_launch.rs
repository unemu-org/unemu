// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bounded journal-disk launch through the frontend loader; persistent-media.md § Memory and hashing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
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
fn frontend_launch_hashes_each_disk_once_with_bounded_allocations() {
    let directory =
        Directory(std::env::temp_dir().join(format!("sparse-launch-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    std::fs::write(directory.0.join("bios.bin"), [0xF4; 0x8000]).unwrap();
    for (name, cylinders, heads, sectors) in [("small", 8u64, 2u64, 4u64), ("large", 1024, 16, 63)]
    {
        let size = cylinders * heads * sectors * 512;
        let path = directory.0.join(format!("{name}.img"));
        let file = File::create(&path).unwrap();
        if let Err(error) = file.set_len(size) {
            eprintln!("SKIP: filesystem cannot create sparse launch base: {error}");
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if size > 1024 * 1024 && file.metadata().unwrap().blocks() * 512 > size / 8 {
                eprintln!("SKIP: filesystem allocated the launch base densely");
                return;
            }
        }
        drop(file);
        let machine_path = directory.0.join(format!("{name}.toml"));
        std::fs::write(&machine_path, format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['fixed-disk']\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={{cylinders={cylinders},heads={heads},sectors={sectors}}}\nimage='{name}.img'\nwrite='journal'\njournal='{name}.upfj'\n")).unwrap();
        let baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        let hashes = journal_files::hash_passes();
        let loaded = machine_file::load_for_assembly(
            &machine_path,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        let mut assembled = loaded
            .assemble(&assembly::ObservationPolicy::default())
            .unwrap();
        assert_eq!(
            journal_files::hash_passes() - hashes,
            1,
            "hashes for {name} launch"
        );
        assembled.finish_persistence().unwrap();
        let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
        eprintln!(
            "{name} launch ({size} bytes): hash passes=1, peak additional live heap={peak} bytes"
        );
        assert!(
            peak < 32 * 1024 * 1024,
            "whole-image allocation in {name} launch: {peak}"
        );
        let mut changed = OpenOptions::new().write(true).open(&path).unwrap();
        changed.seek(SeekFrom::Start(0)).unwrap();
        changed.write_all(&[1; 512]).unwrap();
        // A coarse filesystem clock (Linux stamps files from the timer tick) can
        // give a same-size rewrite the verified timestamps; the check can only
        // see a change that moved them.
        changed
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        let error = match loaded.assemble(&assembly::ObservationPolicy::default()) {
            Ok(_) => panic!("changed verified base accepted"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("base file changed after verification"));
        assert_eq!(journal_files::hash_passes() - hashes, 1);
    }
}
