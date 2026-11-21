// SPDX-License-Identifier: AGPL-3.0-or-later
//! The mount seam (core-contract.md §12.5 move 2, §14.4): a machine whose
//! images, disk bases and journals come from a mount in memory, with no host
//! file, runs as the same machine over files does and leaves the same journal
//! bytes in its records; the mount lives as long as the machine; a medium
//! appended by path still mounts as a host file.

use machine_file::FileMachineExt;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use assembly::{
    assemble_mounted, AssembledMachine, ConfigError, DeviceId, DeviceKind, DiskBase, DriveConfig,
    DriveKind, FloppyAction, FloppyMediaList, FloppyMedium, HardDiskGeometry, IdeCfg, ImageBytes,
    ImageSource, JournalStore, MachineConfig, Mount, MountError, ObservationPolicy, RunPolicy,
    WriteMode,
};
use machine_file::assemble;
use sha2::{Digest, Sha256};
use unemu_core_api::test_support::{MemoryContent, MemoryStorage};
use unemu_core_api::{Error, ErrorKind, Storage};

/// A mount that answers every path from memory: image bytes and bases from
/// tables, journals from records of a [`MemoryStorage`]. It logs each journal
/// it opens.
struct MemoryMount {
    images: BTreeMap<PathBuf, Vec<u8>>,
    bases: BTreeMap<PathBuf, Vec<u8>>,
    records: BTreeMap<PathBuf, &'static str>,
    storage: MemoryStorage,
    opened: Rc<RefCell<Vec<&'static str>>>,
}

impl Mount for MemoryMount {
    fn image(&self, path: &Path) -> Result<ImageBytes, MountError> {
        self.images
            .get(path)
            .map(|bytes| ImageBytes::new(bytes.clone()))
            .ok_or_else(|| Error::new(ErrorKind::Content, "no such content").into())
    }

    fn disk_base(
        &self,
        path: &Path,
        check_length: &dyn Fn(u64) -> Result<(), ConfigError>,
    ) -> Result<DiskBase, MountError> {
        let bytes = self
            .bases
            .get(path)
            .ok_or_else(|| Error::new(ErrorKind::Content, "no such base"))?;
        check_length(bytes.len() as u64)?;
        DiskBase::from_reader(
            Box::new(MemoryContent::new(bytes.clone())),
            Sha256::digest(bytes).into(),
            bytes.len() as u64,
        )
    }

    fn journal(&self, path: &Path) -> Result<JournalStore, MountError> {
        let name = self
            .records
            .get(path)
            .ok_or_else(|| Error::new(ErrorKind::Usage, "no such record"))?;
        self.opened.borrow_mut().push(name);
        Ok(JournalStore::record(self.storage.open(name)?))
    }

    fn check_outputs(&self, _media: &MachineConfig) -> Result<(), ConfigError> {
        Ok(())
    }
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "assembly-mount-seam-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn out(program: &mut Vec<u8>, port: u16, value: u8) {
    program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
}
fn dma(program: &mut Vec<u8>, write: bool) {
    for (port, value) in [
        (0x0A, 6),
        (0x0C, 0),
        (0x0B, if write { 0x4A } else { 0x46 }),
        (4, 0),
        (4, if write { 6 } else { 10 }),
        (0x81, 0),
        (5, 0xFF),
        (5, 0x01),
        (0x0A, 2),
    ] {
        out(program, port, value);
    }
}
fn command(program: &mut Vec<u8>, write: bool, result: u16) {
    for value in [if write { 5 } else { 6 }, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
        out(program, 0x3F5, value);
    }
    program.extend([0xBF, result as u8, (result >> 8) as u8]);
    for _ in 0..7 {
        program.extend([0xEC, 0xAA]);
    }
}
/// µPD765 WRITE DATA of one sector at C0 H0 R1 (when `write`), then READ DATA
/// of it into 0xA00, then HLT.
fn program(write: bool) -> Vec<u8> {
    let mut program = vec![0xFA, 0xFC];
    out(&mut program, 0x3F2, 0x1C);
    out(&mut program, 0x3F7, 2);
    if write {
        dma(&mut program, true);
        command(&mut program, true, 0x1000);
    }
    dma(&mut program, false);
    command(&mut program, false, 0x1010);
    program.push(0xF4);
    program
}
fn pattern() -> Vec<u8> {
    (0..512).map(|i| (i as u8) ^ 0xC7).collect()
}
fn floppy_base() -> Vec<u8> {
    vec![0xE5; 368_640]
}
fn ide_base() -> Vec<u8> {
    (0..32768).map(|i| (i ^ (i >> 9)) as u8).collect()
}
fn cmos_base() -> [u8; 64] {
    std::array::from_fn(|i| i as u8)
}

/// Where each input and output of the machine is: a host directory, or keys
/// of a memory mount under [`memory_root`].
fn at(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

/// A floppy journal (µPD765), a fixed-disk journal (ATA) and a CMOS journal,
/// with the guest program read from an image file.
fn config(root: &Path, write: bool) -> MachineConfig {
    let mut config = assembly::at_fdc_write(vec![0xF4], pattern(), 0x600);
    config.images[0].source =
        ImageSource::File(at(root, if write { "write.bin" } else { "read.bin" }));
    config.drives[0].image = ImageSource::File(at(root, "floppy.img"));
    config.drives[0].write = WriteMode::journal(at(root, "floppy.upfj"));
    config
        .devices
        .iter_mut()
        .find(|d| d.id == DeviceId::Fdc)
        .unwrap()
        .irq = Some(6);
    let rtc = config
        .devices
        .iter_mut()
        .find(|d| matches!(d.kind, DeviceKind::Rtc(_)))
        .unwrap();
    let DeviceKind::Rtc(cmos) = &mut rtc.kind else {
        unreachable!()
    };
    cmos.nvram = assembly::NvramSource::Journal {
        image: at(root, "cmos.bin"),
        expected_sha256: Sha256::digest(cmos_base()).into(),
        journal: at(root, "cmos.upfj"),
        keep_generations: 2,
    };
    let ide = IdeCfg {
        model: assembly::AtaControllerModel::Ata,
        // The secondary channel: the floppy controller decodes 3F6h here.
        base: 0x170,
        control: 0x376,
    };
    config.devices.push(
        assembly::DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(ide), ide.port_windows())
            .with_irq(ide.irq()),
    );
    config.drives.push(DriveConfig {
        media: None,
        controller: DeviceId::Ide,
        unit: 0,
        kind: DriveKind::HardDisk {
            geometry: HardDiskGeometry {
                cylinders: 8,
                heads: 2,
                sectors_per_track: 4,
                bytes_per_sector: 512,
            },
        },
        image: ImageSource::File(at(root, "ide.img")),
        expected_sha256: Some(Sha256::digest(ide_base()).into()),
        write: WriteMode::journal(at(root, "ide.upfj")),
    });
    config
}

fn write_inputs(root: &Path) {
    std::fs::write(at(root, "write.bin"), program(true)).unwrap();
    std::fs::write(at(root, "read.bin"), program(false)).unwrap();
    std::fs::write(at(root, "floppy.img"), floppy_base()).unwrap();
    std::fs::write(at(root, "cmos.bin"), cmos_base()).unwrap();
    std::fs::write(at(root, "ide.img"), ide_base()).unwrap();
}

/// The memory mount's keys: absolute paths under a folder of the host's
/// temporary directory that no test creates.
fn memory_root() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::temp_dir().join(format!(
            "assembly-mount-seam-keys-{}-never-created",
            std::process::id()
        ))
    })
}

fn memory_mount(storage: &MemoryStorage) -> MemoryMount {
    let root = memory_root();
    MemoryMount {
        images: [
            (at(root, "write.bin"), program(true)),
            (at(root, "read.bin"), program(false)),
            (at(root, "cmos.bin"), cmos_base().to_vec()),
        ]
        .into(),
        bases: [
            (at(root, "floppy.img"), floppy_base()),
            (at(root, "ide.img"), ide_base()),
        ]
        .into(),
        records: [
            (at(root, "floppy.upfj"), "floppy.upfj"),
            (at(root, "cmos.upfj"), "cmos.upfj"),
            (at(root, "ide.upfj"), "ide.upfj"),
        ]
        .into(),
        storage: storage.clone(),
        opened: Rc::default(),
    }
}

fn mounted(config: &MachineConfig, mount: MemoryMount) -> AssembledMachine {
    assemble_mounted(
        config,
        &ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        Rc::new(mount),
    )
    .unwrap()
}

fn ata_write(machine: &mut AssembledMachine, lba: u8, value: u16) {
    let bus = machine.machine.bus_mut();
    for (port, byte) in [
        (0x176, 0xE0),
        (0x172, 1),
        (0x173, lba),
        (0x174, 0),
        (0x175, 0),
        (0x177, 0x30),
    ] {
        bus.port_write(port, 1, byte.into()).unwrap();
    }
    for _ in 0..256 {
        bus.port_write(0x170, 2, value.into()).unwrap();
    }
}

fn ide_image(machine: &mut AssembledMachine) -> Vec<u8> {
    let index = machine.device_index[&DeviceId::Ide];
    machine
        .machine
        .bus_mut()
        .device_mut(index)
        .unwrap()
        .as_any_mut()
        .unwrap()
        .downcast_mut::<devices::Ata>()
        .unwrap()
        .drive(0)
        .unwrap()
        .image()
}

fn cmos(machine: &mut AssembledMachine, index: u8, value: Option<u8>) -> u8 {
    let bus = machine.machine.bus_mut();
    bus.port_write(0x70, 1, u32::from(index)).unwrap();
    if let Some(value) = value {
        bus.port_write(0x71, 1, u32::from(value)).unwrap();
    }
    bus.port_read(0x71, 1).unwrap() as u8
}

fn run(machine: &mut AssembledMachine) {
    assert_eq!(
        machine
            .run_session(&RunPolicy::natural(Some(10_000), None))
            .unwrap(),
        machine::RunOutcome::Halted
    );
}

fn readback(machine: &AssembledMachine) -> Vec<u8> {
    let mut bytes = vec![0; 512];
    machine.machine.cpu().read_mem(0xA00, &mut bytes).unwrap();
    bytes
}

/// One session: a host ATA write of LBA 3, the guest's floppy write (when
/// `write`) and read-back, a CMOS byte, then finish. Returns what the guest
/// read, the fixed disk's image and CMOS byte 10h before the CMOS write.
fn session(machine: &mut AssembledMachine, write: bool) -> (Vec<u8>, Vec<u8>, u8) {
    if write {
        ata_write(machine, 3, 0x1234);
    }
    run(machine);
    let before = cmos(machine, 0x10, None);
    if write {
        cmos(machine, 0x10, Some(0xA5));
    }
    let image = ide_image(machine);
    machine.finish_persistence().unwrap();
    (readback(machine), image, before)
}

#[test]
fn a_machine_mounted_from_memory_runs_and_its_records_hold_the_journal_files_bytes() {
    let temp = Temp::new();
    write_inputs(&temp.0);
    let storage = MemoryStorage::new();
    let names = ["floppy.upfj", "cmos.upfj", "ide.upfj"];

    for write in [true, false] {
        let mut files = assemble(&config(&temp.0, write)).unwrap();
        let mut memory = mounted(&config(memory_root(), write), memory_mount(&storage));
        assert_eq!(memory.persistence.len(), 3);
        let from_files = session(&mut files, write);
        let from_memory = session(&mut memory, write);
        assert_eq!(from_memory, from_files, "write={write}");
        assert_eq!(from_memory.0, pattern(), "the guest reads its own write");
        let mut written = ide_base();
        written[3 * 512..4 * 512].copy_from_slice(&0x1234u16.to_le_bytes().repeat(256));
        assert_eq!(from_memory.1, written);
        assert_eq!(from_memory.2, if write { 0x10 } else { 0xA5 });
        drop((files, memory));
        for name in names {
            assert_eq!(
                storage.contents(name).unwrap(),
                std::fs::read(temp.path(name)).unwrap(),
                "{name} after the session with write={write}"
            );
            assert_eq!(storage.durable(name), storage.contents(name), "{name}");
            assert!(
                !storage.is_open(name),
                "{name} is released with the machine"
            );
        }
    }
    // Two sessions: the second continued the first's journals.
    let scan = journal_files::scan(
        &storage.contents("floppy.upfj").unwrap(),
        &floppy_base(),
        368_640,
    );
    assert_eq!(scan.report.generations.len(), 2);
    assert_eq!(&scan.image.unwrap()[..512], pattern().as_slice());
    assert!(
        !memory_root().exists(),
        "the memory mount's keys are not host paths"
    );
}

#[test]
fn the_mount_lives_with_the_machine_and_opens_a_catalogue_journal_at_insert() {
    let storage = MemoryStorage::new();
    let root = memory_root();
    let mut mount = memory_mount(&storage);
    mount
        .bases
        .insert(at(root, "second.img"), vec![0x5A; 368_640]);
    mount.records.insert(at(root, "second.upfj"), "second.upfj");
    let opened = mount.opened.clone();
    let mount = Rc::new(mount);
    let alive = Rc::downgrade(&mount);
    let mut config = config(root, true);
    let first = FloppyMedium {
        image: config.drives[0].image.clone(),
        expected_sha256: None,
        write: config.drives[0].write.clone(),
        name: "first".into(),
    };
    let second = FloppyMedium {
        image: ImageSource::File(at(root, "second.img")),
        expected_sha256: None,
        write: WriteMode::journal(at(root, "second.upfj")),
        name: "second".into(),
    };
    config.drives[0].media = Some(FloppyMediaList {
        entries: vec![first, second],
        empty: false,
        inserted: 0,
    });
    let mut machine = assemble_mounted(
        &config,
        &ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        mount,
    )
    .unwrap();
    assert!(alive.upgrade().is_some(), "the machine holds its mount");
    assert_eq!(*opened.borrow(), ["cmos.upfj", "floppy.upfj", "ide.upfj"]);
    assert!(storage.is_open("floppy.upfj"));
    assert!(!storage.names().contains(&"second.upfj".to_string()));

    run(&mut machine);
    machine
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(1))
        .unwrap();
    assert_eq!(opened.borrow().last(), Some(&"second.upfj"));
    assert!(storage.is_open("second.upfj"));
    assert!(
        !storage.is_open("floppy.upfj"),
        "the ejected medium's record is released"
    );
    let sealed = journal_files::scan(
        &storage.contents("floppy.upfj").unwrap(),
        &floppy_base(),
        368_640,
    );
    assert_eq!(sealed.report.generations.len(), 1);
    assert_eq!(&sealed.image.unwrap()[..512], pattern().as_slice());
    machine
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(0))
        .unwrap();
    assert_eq!(opened.borrow().last(), Some(&"floppy.upfj"));
    machine.finish_persistence().unwrap();
    drop(machine);
    assert!(alive.upgrade().is_none(), "the mount goes with the machine");
    assert!(storage.names().iter().all(|name| !storage.is_open(name)));
}

#[test]
fn a_machine_mounted_from_memory_takes_a_medium_appended_by_path() {
    let temp = Temp::new();
    let storage = MemoryStorage::new();
    let mut config = config(memory_root(), true);
    let mut machine = mounted(&config, memory_mount(&storage));
    std::fs::write(temp.path("host.img"), floppy_base()).unwrap();
    let appended = |journal: &str| FloppyMedium {
        image: ImageSource::File(temp.path("host.img")),
        expected_sha256: None,
        write: WriteMode::journal(temp.path(journal)),
        name: journal.into(),
    };
    let base = || journal_files::VerifiedBase::open(&temp.path("host.img")).unwrap();

    // The appended medium's paths are host files and are checked as such.
    let aliased = machine
        .append_floppy_verified(
            DeviceId::Fdc,
            0,
            &mut config,
            &appended("host.img"),
            base(),
            None,
        )
        .unwrap_err();
    assert!(
        aliased.to_string().contains("aliases mounted input"),
        "{aliased}"
    );
    let index = machine
        .append_floppy_verified(
            DeviceId::Fdc,
            0,
            &mut config,
            &appended("host.upfj"),
            base(),
            None,
        )
        .unwrap();
    assert_eq!(index, 1);
    let twice = machine
        .append_floppy_verified(
            DeviceId::Fdc,
            0,
            &mut config,
            &appended("host.upfj"),
            base(),
            None,
        )
        .unwrap_err();
    assert!(
        twice.to_string().contains("also another medium's journal"),
        "{twice}"
    );
    // Another spelling of the same file passes the key check; the host check
    // of the appended media finds it.
    std::fs::create_dir(temp.path("sub")).unwrap();
    let respelled = machine
        .append_floppy_verified(
            DeviceId::Fdc,
            0,
            &mut config,
            &appended("sub/../host.upfj"),
            base(),
            None,
        )
        .unwrap_err();
    assert!(
        respelled.to_string().contains("aliases another output"),
        "{respelled}"
    );

    machine
        .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(1))
        .unwrap();
    run(&mut machine);
    assert_eq!(readback(&machine), pattern());
    machine.finish_persistence().unwrap();
    drop(machine);

    let host = journal_files::scan(
        &std::fs::read(temp.path("host.upfj")).unwrap(),
        &floppy_base(),
        368_640,
    );
    assert_eq!(&host.image.unwrap()[..512], pattern().as_slice());
    assert!(
        temp.path("host.upfj.lock").exists(),
        "the journal's own lock"
    );
    let memory = journal_files::scan(
        &storage.contents("floppy.upfj").unwrap(),
        &floppy_base(),
        368_640,
    );
    assert!(memory.report.generations.iter().all(|g| g.commands == 0));
    assert_eq!(&memory.image.unwrap()[..512], &floppy_base()[..512]);
}

#[test]
fn a_record_held_elsewhere_fails_the_mount_and_leaves_no_session() {
    let storage = MemoryStorage::new();
    let held = storage.open("ide.upfj").unwrap();
    let error = assemble_mounted(
        &config(memory_root(), true),
        &ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        Rc::new(memory_mount(&storage)),
    )
    .err()
    .expect("a held record refuses the mount");
    assert!(
        matches!(error, assembly::AssemblyError::Journal { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("open journal record"), "{error}");
    drop(held);
    assert!(storage.names().iter().all(|name| !storage.is_open(name)));
}

#[test]
fn a_machine_without_journals_keeps_its_mount_too() {
    let storage = MemoryStorage::new();
    let mount = Rc::new(memory_mount(&storage));
    let alive = Rc::downgrade(&mount);
    let mut config = assembly::at_fdc_write(vec![0xF4], pattern(), 0x600);
    config.images[0].source = ImageSource::File(at(memory_root(), "read.bin"));
    let machine = assemble_mounted(
        &config,
        &ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        mount,
    )
    .unwrap();
    assert!(machine.persistence.is_empty());
    assert!(
        alive.upgrade().is_some(),
        "the machine itself holds its mount"
    );
    drop(machine);
    assert!(alive.upgrade().is_none());
}

#[test]
fn two_catalogue_media_with_one_journal_key_are_refused_on_a_record_mount() {
    let storage = MemoryStorage::new();
    let root = memory_root();
    let mut mount = memory_mount(&storage);
    mount
        .bases
        .insert(at(root, "second.img"), vec![0x5A; 368_640]);
    let mut config = config(root, true);
    let medium = |image: &str| FloppyMedium {
        image: ImageSource::File(at(root, image)),
        expected_sha256: None,
        write: WriteMode::journal(at(root, "floppy.upfj")),
        name: image.into(),
    };
    config.drives[0].media = Some(FloppyMediaList {
        entries: vec![medium("floppy.img"), medium("second.img")],
        empty: false,
        inserted: 0,
    });
    let error = assemble_mounted(
        &config,
        &ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        Rc::new(mount),
    )
    .err()
    .expect("one journal for two media is refused");
    assert!(
        matches!(
            error,
            assembly::AssemblyError::Config(ConfigError::Journal { drive_index: 0, .. })
        ),
        "{error}"
    );
    assert!(
        error.to_string().contains("also another medium's journal"),
        "{error}"
    );
    assert!(storage.names().is_empty(), "nothing was opened");

    // The fixed disk's journal key named again by the CMOS journal.
    let mut config = self::config(root, true);
    let rtc = config
        .devices
        .iter_mut()
        .find(|d| matches!(d.kind, DeviceKind::Rtc(_)))
        .unwrap();
    let DeviceKind::Rtc(cmos) = &mut rtc.kind else {
        unreachable!()
    };
    let assembly::NvramSource::Journal { journal, .. } = &mut cmos.nvram else {
        unreachable!()
    };
    *journal = at(root, "ide.upfj");
    let error = assemble_mounted(
        &config,
        &ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        Rc::new(memory_mount(&storage)),
    )
    .err()
    .expect("the CMOS journal cannot share the disk's");
    assert!(
        matches!(
            error,
            assembly::AssemblyError::Config(ConfigError::Nvram { .. })
        ),
        "{error}"
    );
}

#[test]
fn file_and_record_mounts_keep_retained_journal_bytes_identical() {
    let temp = Temp::new();
    write_inputs(&temp.0);
    let storage = MemoryStorage::new();
    for _ in 0..4 {
        let retention = |root: &Path| {
            let mut config = config(root, true);
            for drive in &mut config.drives {
                if let WriteMode::Journal {
                    keep_generations, ..
                } = &mut drive.write
                {
                    *keep_generations = 1;
                }
            }
            for device in &mut config.devices {
                if let DeviceKind::Rtc(cmos) = &mut device.kind {
                    if let assembly::NvramSource::Journal {
                        keep_generations, ..
                    } = &mut cmos.nvram
                    {
                        *keep_generations = 1;
                    }
                }
            }
            config
        };
        let mut files = assemble(&retention(&temp.0)).unwrap();
        let mut memory = mounted(&retention(memory_root()), memory_mount(&storage));
        assert_eq!(session(&mut files, true), session(&mut memory, true));
        drop((files, memory));
        for name in ["floppy.upfj", "cmos.upfj", "ide.upfj"] {
            assert_eq!(
                storage.contents(name).unwrap(),
                std::fs::read(temp.path(name)).unwrap(),
                "{name}"
            );
            assert_eq!(storage.durable(name), storage.contents(name));
            assert!(!storage.is_open(name));
        }
    }
    for (name, base) in [
        ("floppy.upfj", floppy_base()),
        ("cmos.upfj", cmos_base().to_vec()),
        ("ide.upfj", ide_base()),
    ] {
        let scan = journal_files::scan(&storage.contents(name).unwrap(), &base, base.len() as u64);
        assert_eq!(scan.report.generations.len(), 1, "{name}");
        assert!(scan.report.header.unwrap().anchor_generation > 0, "{name}");
    }
    assert_eq!(
        std::fs::read(temp.path("floppy.img")).unwrap(),
        floppy_base()
    );
    assert_eq!(std::fs::read(temp.path("ide.img")).unwrap(), ide_base());
    assert_eq!(std::fs::read(temp.path("cmos.bin")).unwrap(), cmos_base());
}

#[test]
fn file_retention_fault_injection_leaves_record_sessions_unchanged() {
    let storage = MemoryStorage::new();
    let mut machine = mounted(&config(memory_root(), true), memory_mount(&storage));
    for handle in &mut machine.persistence {
        handle.fail_retention("replace");
    }
    assert_eq!(session(&mut machine, true).0, pattern());
    drop(machine);
    for name in ["floppy.upfj", "cmos.upfj", "ide.upfj"] {
        assert!(!storage.is_open(name));
        assert_eq!(storage.durable(name), storage.contents(name));
    }
}
