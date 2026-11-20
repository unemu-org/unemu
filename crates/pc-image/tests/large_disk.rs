// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bounded host allocation for large raw media; machine-images §§2 and 5.

use pc_image::{import_machine, ImportOptions};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

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
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn sparse_disk_import_and_resolve_bound_every_allocation() {
    let Some(bios) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let Some(xtide) = fixtures::fixture("xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin") else {
        return;
    };
    let temp = Temp(std::env::temp_dir().join(format!("pc-image-large-{}", std::process::id())));
    fs::create_dir(&temp.0).unwrap();
    fs::write(temp.0.join("bios.bin"), bios).unwrap();
    fs::write(temp.0.join("xtide.bin"), xtide).unwrap();
    let disk = fs::File::create_new(temp.0.join("large.img")).unwrap();
    disk.set_len(512 * 1024 * 1024).unwrap();
    drop(disk);
    let path = temp.0.join("machine.toml");
    fs::write(&path,"schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[firmware.option_roms]]\nimage='xtide.bin'\nbase=0xC8000\nprovides=['fixed-disk']\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=2048,heads=16,sectors=32}\nimage='large.img'\nwrite='readonly'\n").unwrap();
    let store = image_store::Store::open(temp.0.join("store")).unwrap();
    LARGEST.store(0, Ordering::Relaxed);
    import_machine(&store, &path, "test:large", &ImportOptions::default()).unwrap();
    let import_peak = LARGEST.swap(0, Ordering::Relaxed);
    let owner = store.workspace("large-disk").unwrap();
    let config = pc_image::workspace::prepare_start(&owner, "test:large", false)
        .unwrap()
        .machine;
    let resolve_peak = LARGEST.load(Ordering::Relaxed);
    eprintln!("largest allocation: import={import_peak}, resolve={resolve_peak}");
    assert!(
        import_peak <= 2 * 1024 * 1024,
        "import allocated {import_peak}"
    );
    assert!(
        resolve_peak <= 2 * 1024 * 1024,
        "resolve allocated {resolve_peak}"
    );
    assert_eq!(config.drives[0].kind.image_bytes(), 512 * 1024 * 1024);
    assert!(matches!(
        config.drives[0].image,
        assembly::ImageSource::File(_)
    ));
    assert!(config.drives[0].expected_sha256.is_some());
}
