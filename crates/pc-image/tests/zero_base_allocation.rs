// SPDX-License-Identifier: AGPL-3.0-or-later
//! A 504 MiB blank C: costs no large allocation and no large file, in the
//! store or the workspace, from Create through Start; machine-images §4.

use pc_image::{create::Chs, workspace};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
#[path = "support/open_image.rs"]
mod open_image;

static LARGEST: AtomicUsize = AtomicUsize::new(0);
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        LARGEST.fetch_max(size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn files(root: &Path, found: &mut BTreeMap<PathBuf, (u64, std::time::SystemTime)>) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let metadata = fs::symlink_metadata(entry.path()).unwrap();
        if metadata.is_dir() {
            files(&entry.path(), found);
        } else {
            found.insert(entry.path(), (metadata.len(), metadata.modified().unwrap()));
        }
    }
}

#[test]
fn create_and_start_a_504_mib_blank_disk_without_large_files_or_allocations() {
    let open = open_image::Open::new();
    let image = open.image([Some(Chs::new(1024, 16, 63)), None]);
    let mut before = BTreeMap::new();
    files(&open.path, &mut before);
    LARGEST.store(0, Ordering::Relaxed);
    pc_image::create::create(&open.store, &image, "open-at:1.0.0", None).unwrap();
    // Compression is on: the package ROMs are stored as zstd frames, which the
    // image shares; the zero base is never compressed, also after `compress`.
    let layers = open
        .store
        .resolve("open-at:1.0.0", &pc_image::PcProvider)
        .unwrap()
        .manifest
        .layers;
    assert!(layers
        .iter()
        .filter(|d| d.decoded_media_type() == image_store::media_type::FIRMWARE)
        .all(|d| d.is_compressed()));
    pc_image::compress_image(&open.store, "open-at:1.0.0", "open-at:compressed").unwrap();
    for tag in ["open-at:1.0.0", "open-at:compressed"] {
        let layers = open
            .store
            .resolve(tag, &pc_image::PcProvider)
            .unwrap()
            .manifest
            .layers;
        let zero: Vec<_> = layers
            .iter()
            .filter(|d| d.decoded_media_type() == pc_image::DISK_ZERO)
            .collect();
        assert_eq!(zero.len(), 1);
        assert!(!zero[0].is_compressed() && zero[0].size < 256);
    }
    let owner = open.store.workspace("compressed-start").unwrap();
    workspace::prepare_start(&owner, "open-at:compressed", false).unwrap();
    let owner = open.store.workspace("first-start").unwrap();
    let prepared = workspace::prepare_start(&owner, "open-at:1.0.0", false).unwrap();
    let peak = LARGEST.swap(0, Ordering::Relaxed);
    assert!(peak <= 2 * 1024 * 1024, "largest allocation {peak} bytes");
    let disk = prepared
        .machine
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    assert_eq!(disk.kind.image_bytes(), 1024 * 16 * 63 * 512);
    // Mounting opens the journal over the zero base; guest RAM is the only
    // large allocation of an assembled machine, so files are checked instead.
    let mut machine = assembly::assemble_with_verified_bases(
        &prepared.machine,
        &assembly::ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        &prepared.bases,
    )
    .unwrap();
    machine
        .run_session(&assembly::RunPolicy::natural(Some(10_000), None))
        .unwrap();
    machine.finish_persistence().unwrap();
    let mut after = BTreeMap::new();
    files(&open.path, &mut after);
    let written: Vec<_> = after
        .iter()
        .filter(|(path, stat)| before.get(*path) != Some(stat))
        .collect();
    assert!(!written.is_empty());
    for (path, (size, _)) in &written {
        assert!(
            *size <= 16 * 1024,
            "{} has {size} bytes",
            path.strip_prefix(&open.path).unwrap().display()
        );
    }
}
