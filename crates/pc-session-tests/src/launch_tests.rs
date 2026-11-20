// SPDX-License-Identifier: AGPL-3.0-or-later
//! Equality of the launch factory and the verified-file assembler, including
//! the version-1 journal bytes (core-contract.md §14.3).

use pc_config::{image::Digest, launch};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use unemu_core_api::{
    test_support::{MemoryContent, MemoryStorage},
    ConfigDocument, Content, ContentSource, CreateRequest, DisplayId, Error, ErrorKind, InputEvent,
    Machine, MediaAction, Outputs, RunLimit, SlotId, Storage,
};
use unemu_pc_core::{PcCore, PcMachine};

struct Source {
    blobs: BTreeMap<[u8; 32], Vec<u8>>,
    opened: RefCell<BTreeSet<([u8; 32], u64)>>,
}
impl ContentSource for Source {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        self.opened.borrow_mut().insert((*sha256, size));
        self.blobs
            .get(sha256)
            .filter(|bytes| bytes.len() as u64 == size)
            .map(|bytes| Content::Reader(Box::new(MemoryContent::new(bytes.clone()))))
            .ok_or_else(|| Error::new(ErrorKind::Content, "missing test content"))
    }
}

/// NEC µPD765A datasheet, READ/WRITE DATA, N=2, and Intel 8237A datasheet, single-transfer mode,
/// channel 2. Each command saves its result so a failed transfer cannot pass
/// merely because both start paths produced the same empty buffer.
fn floppy(program: &mut Vec<u8>, write: bool, sector: u8, address: u16, result: u16) {
    let out = |program: &mut Vec<u8>, port: u16, value: u8| {
        program.extend([0xba, port as u8, (port >> 8) as u8, 0xb0, value, 0xee]);
    };
    for (port, value) in [
        (0x0a, 6),
        (0x0c, 0),
        (0x0b, if write { 0x4a } else { 0x46 }),
        (4, address as u8),
        (4, (address >> 8) as u8),
        (0x81, 0),
        (5, 0xff),
        (5, 1),
        (0x0a, 2),
    ] {
        out(program, port, value);
    }
    for value in [
        if write { 5 } else { 6 },
        0,
        0,
        0,
        sector,
        2,
        sector,
        0x1b,
        0xff,
    ] {
        out(program, 0x3f5, value);
    }
    program.extend([0xbf, result as u8, (result >> 8) as u8]);
    for _ in 0..7 {
        program.extend([0xec, 0xaa]);
    }
}

fn fixture() -> (Vec<u8>, Source) {
    let mut bios = vec![0xf4; 32768];
    let mut program = vec![
        0xfa, 0xb0, 0x10, 0xe6, 0x70, 0xe4, 0x71, 0xfe, 0xc0, 0xe6, 0x71, 0xb0, 0xb6, 0xe6, 0x43,
        0xb0, 0xa9, 0xe6, 0x42, 0xb0, 0x04, 0xe6, 0x42, 0xb0, 0x03, 0xe6, 0x61, 0xba, 0xf2, 0x03,
        0xb0, 0x1c, 0xee, 0xb8, 0x00, 0xb0, 0x8e, 0xd8, 0xc7, 0x06, 0x00, 0x00, 0x41, 0x07,
    ];
    program.extend([0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0, 0xfc]);
    program.extend([0xb0, 0x02, 0xba, 0xf7, 0x03, 0xee]);
    for sector in 1..=2 {
        floppy(
            &mut program,
            false,
            sector,
            0x2000 + u16::from(sector - 1) * 512,
            0x1800 + u16::from(sector - 1) * 8,
        );
    }
    for sector in 1..=2 {
        let address = 0x2400 + u16::from(sector - 1) * 512;
        // Each boot writes a different full-sector pattern. The next boot's
        // initial reads must therefore come from the journal, even after pruning.
        program.extend([
            0xb0,
            0x10,
            0xe6,
            0x70,
            0xe4,
            0x71,
            0x34,
            (sector - 1) * 0x50,
        ]);
        program.extend([
            0xbf,
            address as u8,
            (address >> 8) as u8,
            0xb9,
            0,
            2,
            0xf3,
            0xaa,
        ]);
        floppy(
            &mut program,
            true,
            sector,
            address,
            0x1810 + u16::from(sector - 1) * 8,
        );
        floppy(
            &mut program,
            false,
            sector,
            0x2800 + u16::from(sector - 1) * 512,
            0x1820 + u16::from(sector - 1) * 8,
        );
    }
    program.extend([0xeb, 0xfe]);
    bios[..program.len()].copy_from_slice(&program);
    bios[0x7ff0..0x7ff5].copy_from_slice(&[0xea, 0, 0, 0, 0xf8]);
    let cmos = vec![0; 64];
    let disk = vec![0xf6; 368640];
    let [db, dc, dd] = [&bios, &cmos, &disk].map(|bytes| Digest::of(bytes));
    let text = format!(
        r#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[machine.board.rtc.nvram]
image = "assets/cmos.bin"
[machine.firmware.bios]
image = "assets/bios.bin"
provides = ["floppy"]
sha256 = "{bios_hash}"
[[machine.card]]
kind = "mda"
[[machine.card]]
kind = "vga-text"
[[machine.card]]
kind = "fdc-765"
[[machine.card.drives]]
unit = 0
type = "360K"
[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "{db}"
[[media]]
id = "a"
source = "assets/a.img"
name = "Disk A"
head = "{dd}"
policy = "journal"
geometry = {{ cylinders = 40, heads = 2, sectors = 9, sector_bytes = 512 }}
[[slots]]
controller = "fdc-765"
unit = 0
catalogue = ["a"]
catalogue_mode = true
inserted = "a"
[nvram]
mode = "journal"
source = "assets/cmos.bin"
head = "{dc}"
[[content]]
digest = "{db}"
sha256 = "{bios_hash}"
size = 32768
[[content]]
digest = "{dd}"
sha256 = "{disk_hash}"
size = 368640
[[content]]
digest = "{dc}"
sha256 = "{cmos_hash}"
size = 64
[[record]]
medium = "a"
name = "a.upfj"
keep_generations = 3
[[record]]
medium = "cmos"
name = "cmos.upfj"
keep_generations = 3
"#,
        bios_hash = db.hex(),
        disk_hash = dd.hex(),
        cmos_hash = dc.hex()
    );
    (
        text.into_bytes(),
        Source {
            blobs: [bios, cmos, disk]
                .into_iter()
                .map(|bytes| (Digest::of(&bytes).sha256(), bytes))
                .collect(),
            opened: RefCell::default(),
        },
    )
}
fn request<'a>(bytes: &'a [u8], source: &'a Source, storage: Rc<dyn Storage>) -> CreateRequest<'a> {
    CreateRequest::new(
        "ibm-pc",
        ConfigDocument::new(launch::MEDIA_TYPE, bytes),
        source,
        storage,
    )
    .with_options(&[
        ("allow-unlisted-firmware", "true"),
        ("speaker-trace", "true"),
    ])
}

fn policy() -> assembly::ObservationPolicy {
    assembly::ObservationPolicy {
        routes: [assembly::DeviceId::Mda, assembly::DeviceId::Vga]
            .into_iter()
            .map(|device| assembly::ObservationRequest {
                device,
                output: assembly::OutputKind::TextScanout,
                byte_limit: 0,
            })
            .collect(),
        speaker_trace: true,
        ..Default::default()
    }
}

fn cmos_byte(machine: &mut PcMachine) -> u8 {
    let assembled = machine.assembled();
    let index = assembled.device_index[&assembly::DeviceId::Rtc];
    let rtc = assembled.machine.bus_mut().device_mut(index).unwrap();
    rtc.port_write(0x70, 1, 0x10).unwrap();
    rtc.port_read(0x71, 1).unwrap() as u8
}

#[test]
fn launch_and_verified_file_sessions_have_identical_output_and_journal_bytes() {
    let (bytes, source) = fixture();
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    let lowered = launch::lower(&launch::decode(&bytes).unwrap(), &source, options).unwrap();
    let storage = Rc::new(MemoryStorage::new());
    let temp = assembly::test_support::TempDir::new("launch-comparison");
    let mut direct_config = lowered.machine.clone();
    let mut bases = BTreeMap::new();
    for (key, (sha256, _)) in &lowered.bases {
        let path = temp.path().join(key.file_name().unwrap());
        let data = &source.blobs[sha256];
        std::fs::write(&path, data).unwrap();
        bases.insert(path.clone(), media::VerifiedBase::from_bytes(data).unwrap());
        for drive in &mut direct_config.drives {
            if drive.image == assembly::ImageSource::File(key.clone()) {
                drive.image = assembly::ImageSource::File(path.clone());
            }
            if let Some(list) = &mut drive.media {
                for medium in &mut list.entries {
                    if medium.image == assembly::ImageSource::File(key.clone()) {
                        medium.image = assembly::ImageSource::File(path.clone());
                    }
                }
            }
        }
        for device in &mut direct_config.devices {
            if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                if let assembly::NvramSource::Journal { image, .. } = &mut rtc.nvram {
                    if image == key {
                        *image = path.clone();
                    }
                }
            }
        }
    }
    for drive in &mut direct_config.drives {
        if let assembly::WriteMode::Journal { path, .. } = &mut drive.write {
            *path = temp.path().join(path.file_name().unwrap());
        }
        if let Some(list) = &mut drive.media {
            for medium in &mut list.entries {
                if let assembly::WriteMode::Journal { path, .. } = &mut medium.write {
                    *path = temp.path().join(path.file_name().unwrap());
                }
            }
        }
    }
    for device in &mut direct_config.devices {
        if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
            if let assembly::NvramSource::Journal { journal, .. } = &mut rtc.nvram {
                *journal = temp.path().join(journal.file_name().unwrap());
            }
        }
    }
    for session in 1..=5 {
        let direct =
            assembly::assemble_with_verified_bases(&direct_config, &policy(), options, &bases)
                .unwrap();
        let mut direct = PcMachine::from_assembled(&direct_config, direct).unwrap();
        let mut created = Box::new(
            PcCore
                .create_machine(&request(&bytes, &source, storage.clone()))
                .unwrap(),
        );
        assert_eq!(cmos_byte(&mut direct), session - 1);
        for m in [&mut *created, &mut direct as &mut dyn Machine] {
            m.set_outputs(Outputs::NONE.with_audio(true));
        }
        for steps in [100, 200, 8000] {
            for m in [&mut *created, &mut direct as &mut dyn Machine] {
                m.input(InputEvent::ReleaseAll);
            }
            assert_eq!(
                created.run(RunLimit::steps(steps)),
                direct.run(RunLimit::steps(steps))
            );
            assert_eq!(
                (created.now(), created.steps()),
                (direct.now(), direct.steps())
            );
            for id in [DisplayId(0), DisplayId(1)] {
                let left = created.frame(id).unwrap();
                let right = direct.frame(id).unwrap();
                assert_eq!(
                    (
                        left.width,
                        left.height,
                        left.aspect,
                        left.visible,
                        left.serial,
                        left.pixels
                    ),
                    (
                        right.width,
                        right.height,
                        right.aspect,
                        right.visible,
                        right.serial,
                        right.pixels
                    )
                );
            }
            let mut left = Vec::new();
            let mut right = Vec::new();
            created.drain_audio(&mut left).unwrap();
            direct.drain_audio(&mut right).unwrap();
            assert_eq!(left, right);
            assert!(!left.is_empty());
            let created_slots = created.slot_states();
            let direct_slots = direct.slot_states();
            assert_eq!(created_slots, direct_slots);
            assert!(created_slots[0].active);
            let mut left = Vec::new();
            let mut right = Vec::new();
            created.drain_messages(&mut left);
            direct.drain_messages(&mut right);
            assert_eq!(left, right);
        }
        assert_eq!(cmos_byte(&mut direct), session);
        let mut previous = Vec::new();
        let mut written = Vec::new();
        for sector in 0..2 {
            previous.extend(vec![
                if session == 1 {
                    0xf6
                } else {
                    (session - 1) ^ (sector * 0x50)
                };
                512
            ]);
            written.extend(vec![session ^ (sector * 0x50); 512]);
        }
        for machine in [&mut *created, &mut direct] {
            let cpu = machine.assembled_ref().machine.cpu();
            let mut before = vec![0; 1024];
            let mut after = vec![0; 1024];
            cpu.read_mem(0x2000, &mut before).unwrap();
            cpu.read_mem(0x2800, &mut after).unwrap();
            assert_eq!(before, previous, "replayed sectors, session {session}");
            assert_eq!(after, written, "written sectors, session {session}");
            for result in (0x1800..0x1830).step_by(8) {
                let mut status = [0; 3];
                cpu.read_mem(result, &mut status).unwrap();
                assert_eq!(status, [0; 3], "floppy command at {result:#x}");
            }
        }
        assert_eq!(created.finish().unwrap(), direct.finish().unwrap());
        drop(created);
        drop(direct);
        for (key, name) in &lowered.records {
            let file = temp.path().join(key.file_name().unwrap());
            let journal = storage.contents(name).unwrap();
            assert_eq!(
                journal,
                std::fs::read(file).unwrap(),
                "{name}, session {session}"
            );
            let base = if name == "a.upfj" {
                vec![0xf6; 368640]
            } else {
                vec![0; 64]
            };
            let scan = media::scan(&journal, &base, base.len() as u64);
            assert_eq!(scan.report.status, media::Status::Valid);
            assert_eq!(scan.report.generations.len(), usize::from(session.min(3)));
            let anchor = if session > 3 {
                u64::from(session - 2)
            } else {
                0
            };
            assert_eq!(scan.report.header.unwrap().anchor_generation, anchor);
            if name == "a.upfj" {
                assert_eq!(&scan.image.unwrap()[..1024], written);
            }
        }
    }
}

#[test]
fn launch_machine_accepts_a_verified_floppy_appended_by_path() {
    let (bytes, source) = fixture();
    let storage = Rc::new(MemoryStorage::new());
    let mut created = PcCore
        .create_machine(&request(&bytes, &source, storage))
        .unwrap();
    let lowered = launch::lower(
        &launch::decode(&bytes).unwrap(),
        &source,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    assert_eq!(created.config(), &lowered.machine);
    let temp = assembly::test_support::TempDir::new("launch-append");
    let path = temp.path().join("extra.img");
    let data = vec![0xe5; 368640];
    std::fs::write(&path, &data).unwrap();
    let base = media::VerifiedBase::open(&path).unwrap();
    let medium = assembly::FloppyMedium {
        image: assembly::ImageSource::File(path),
        expected_sha256: None,
        write: assembly::WriteMode::journal(temp.path().join("extra.upfj")),
        name: "Extra".into(),
    };
    let mut config = created.config().clone();
    let index = created
        .assembled()
        .append_floppy_verified(assembly::DeviceId::Fdc, 0, &mut config, &medium, base, None)
        .unwrap();
    assert_eq!(index, 1);
    created
        .change_medium(SlotId(0), MediaAction::Insert(index))
        .unwrap();
    created.run(RunLimit::steps(100)).unwrap();
    created.finish().unwrap();
    assert!(temp.path().join("extra.upfj").is_file());
}
